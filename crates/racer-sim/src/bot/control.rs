//! The hands both bots share: steering to a line, and picking a lane
//! through traffic and roadside objects.
//!
//! **Steering** is a PD controller on the car's offset with a feedforward
//! term. The road's push is known, `κ·v²` outward, so the controller asks
//! for exactly that much plus a correction toward the target line, and
//! divides by the steering authority to get a fraction of full lock. That
//! fraction is quantised to the seven levels a player has; the human bot
//! adds hysteresis, holding its level until the wanted one is clearly
//! different, which is what a thumb on a touch zone does.
//!
//! **Lanes** are three, at the centre and two-thirds of the half width
//! either side. A lane is in conflict when the car, holding its speed, would
//! reach an obstacle in it within the horizon; traffic closes at the speed
//! difference, an object at the car's own speed. The driver keeps its lane
//! until it conflicts, then moves to a free one whose path across is also
//! clear. If none is free it follows: the speed of the obstacle ahead
//! becomes its speed limit.

use crate::car::V_STEER_FULL;
use crate::dmath::{abs, clamp, round};
use crate::input::STEER_LEVELS;
use crate::track::Track;
use crate::traffic::{CAR_HALF_WIDTH, CAR_LEN};

/// Lanes per road: left, centre, right.
pub const LANES: usize = 3;
/// Lane centres as a fraction of the half width.
pub const LANE_FRACTION: f64 = 2.0 / 3.0;

/// Lateral clearance kept from an obstacle, metres, beyond touching.
const SIDE_MARGIN: f64 = 0.5;
/// Longitudinal clearance kept from an obstacle, metres.
const GAP_MARGIN: f64 = 2.0;
/// A lane change must find the road across it clear for this long, s.
const TRANSIT_TIME: f64 = 0.8;
/// Closer than this, in seconds, to the obstacle ahead in a lane with no way
/// round, the driver matches its speed.
const FOLLOW_TIME: f64 = 2.0;

/// The centre of lane `lane` on road of half width `hw`.
#[inline]
pub fn lane_x(lane: usize, hw: f64) -> f64 {
    (lane as f64 - 1.0) * hw * LANE_FRACTION
}

/// A PD steering controller quantised to the player's seven levels.
#[derive(Clone, Copy, Debug)]
pub struct Steering {
    /// Proportional gain on the offset error, 1/s².
    pub kp: f64,
    /// Derivative gain on the sideways speed, 1/s.
    pub kd: f64,
    /// Levels the wanted steering must move by before the held level
    /// changes. 0.5 is plain rounding.
    pub hysteresis: f64,
    level: i8,
}

impl Steering {
    /// A controller holding straight ahead.
    pub const fn new(kp: f64, kd: f64, hysteresis: f64) -> Self {
        Steering {
            kp,
            kd,
            hysteresis,
            level: 0,
        }
    }

    /// The steering level that moves the car toward `target_x`, given its
    /// offset `x`, sideways speed `vx`, speed `v`, the road's curvature and
    /// the lateral grip `cap` available.
    pub fn level(
        &mut self,
        target_x: f64,
        x: f64,
        vx: f64,
        v: f64,
        curvature: f64,
        cap: f64,
    ) -> i8 {
        let authority = cap * clamp(v / V_STEER_FULL, 0.0, 1.0);
        let want = if authority < 1e-6 {
            0.0
        } else {
            let a = curvature * v * v + self.kp * (target_x - x) - self.kd * vx;
            clamp(a / authority * STEER_LEVELS as f64, -3.0, 3.0)
        };
        if abs(want - self.level as f64) > self.hysteresis {
            self.level = round(want) as i8;
        }
        self.level
    }
}

/// Something in the road to steer round: a traffic car or a roadside object.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Obstacle {
    /// Centre, metres along the track, on the tick it was observed.
    pub z: f64,
    /// Speed along the track, m/s.
    pub speed: f64,
    /// Centre offset, metres.
    pub x: f64,
    /// Half its length, metres.
    pub half_len: f64,
    /// Half its width, metres.
    pub half_width: f64,
}

/// Every obstacle between `from` and `to` metres along `track` as of race
/// tick `tick`, into `out` (cleared first).
pub fn obstacles(track: &Track, tick: u32, from: f64, to: f64, out: &mut Vec<Obstacle>) {
    out.clear();
    for c in track.traffic() {
        let z = c.z_at(tick);
        if z + CAR_LEN >= from && z - CAR_LEN <= to {
            out.push(Obstacle {
                z,
                speed: c.speed(),
                x: c.x(),
                half_len: CAR_LEN / 2.0,
                half_width: CAR_HALF_WIDTH,
            });
        }
    }
    let first = track.index_at(from.max(0.0));
    let last = track.index_at(to.max(0.0)) + 1;
    for o in track.objects_in(first, last) {
        let r = o.kind.radius();
        out.push(Obstacle {
            z: o.z(),
            speed: 0.0,
            x: o.x(),
            half_len: r,
            half_width: r,
        });
    }
}

/// Seconds until a car at `z` doing `v` reaches an obstacle overlapping the
/// sideways band `lo..=hi`; 0 if one is alongside, infinity if none.
pub fn conflict_time(z: f64, v: f64, lo: f64, hi: f64, obs: &[Obstacle]) -> (f64, f64) {
    let mut first = (f64::INFINITY, f64::INFINITY);
    for o in obs {
        let reach = o.half_width + CAR_HALF_WIDTH + SIDE_MARGIN;
        if o.x + reach < lo || o.x - reach > hi {
            continue;
        }
        let ahead = o.z - z;
        let touch = CAR_LEN / 2.0 + o.half_len;
        if ahead < -touch {
            continue; // already passed
        }
        let gap = ahead - touch - GAP_MARGIN;
        let t = if gap <= 0.0 {
            0.0
        } else if v - o.speed > 0.01 {
            gap / (v - o.speed)
        } else {
            continue;
        };
        if t < first.0 {
            first = (t, o.speed);
        }
    }
    first
}

/// The lane to drive in, and the speed to follow at if every lane is
/// blocked. `lane` is the lane held now; `horizon` is how many seconds
/// ahead the driver checks.
pub fn choose_lane(
    lane: usize,
    x: f64,
    z: f64,
    v: f64,
    hw: f64,
    horizon: f64,
    obs: &[Obstacle],
) -> (usize, Option<f64>) {
    let in_lane = |l: usize| {
        let lx = lane_x(l, hw);
        conflict_time(z, v, lx, lx, obs)
    };
    let (t_here, follow) = in_lane(lane);
    if t_here > horizon {
        return (lane, None);
    }
    // Free lanes, nearest first; the path across must be clear too.
    let mut best: Option<(usize, f64)> = None;
    for l in 0..LANES {
        if l == lane {
            continue;
        }
        let lx = lane_x(l, hw);
        let (t, _) = in_lane(l);
        let (t_across, _) = conflict_time(z, v, x.min(lx), x.max(lx), obs);
        if t > horizon && t_across > TRANSIT_TIME {
            let d = abs(lx - x);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((l, d));
            }
        }
    }
    match best {
        Some((l, _)) => (l, None),
        None => (lane, (t_here < FOLLOW_TIME).then_some(follow)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car_at(z: f64, x: f64, speed: f64) -> Obstacle {
        Obstacle {
            z,
            speed,
            x,
            half_len: CAR_LEN / 2.0,
            half_width: CAR_HALF_WIDTH,
        }
    }

    #[test]
    fn steering_feeds_the_bend_forward() {
        let mut s = Steering::new(0.0, 0.0, 0.5);
        // On the line, the push outward is all there is to cancel.
        assert_eq!(s.level(0.0, 0.0, 0.0, 30.0, 0.01, 27.0), 1);
        assert_eq!(s.level(0.0, 0.0, 0.0, 30.0, 0.0, 27.0), 0);
        // Stopped, steering does nothing, so the controller does not ask.
        assert_eq!(s.level(5.0, 0.0, 0.0, 0.0, 0.0, 20.0), 0);
    }

    #[test]
    fn hysteresis_holds_a_level_until_the_want_moves_clearly() {
        let mut s = Steering::new(1.0, 0.0, 0.8);
        assert_eq!(s.level(0.0, 0.0, 0.0, 20.0, 0.0, 20.0), 0);
        // 1 m of error is 1 m/s², 0.15 of a level: held.
        assert_eq!(s.level(1.0, 0.0, 0.0, 20.0, 0.0, 20.0), 0);
        assert_eq!(s.level(10.0, 0.0, 0.0, 20.0, 0.0, 20.0), 2);
    }

    #[test]
    fn a_slow_car_ahead_moves_the_driver_over() {
        let obs = [car_at(60.0, 0.0, 20.0)];
        // Closing at 20 m/s on a car 60 m ahead: about 2.6 s away.
        let (t, v) = conflict_time(0.0, 40.0, 0.0, 0.0, &obs);
        assert!((2.0..3.0).contains(&t) && v == 20.0, "{t}");
        let (lane, follow) = choose_lane(1, 0.0, 0.0, 40.0, 6.0, 3.0, &obs);
        assert_ne!(lane, 1);
        assert_eq!(follow, None);
        // Far enough ahead, it stays put.
        assert_eq!(choose_lane(1, 0.0, -200.0, 40.0, 6.0, 3.0, &obs), (1, None));
    }

    #[test]
    fn with_every_lane_blocked_the_driver_follows() {
        let obs = [
            car_at(20.0, -4.0, 18.0),
            car_at(20.0, 0.0, 18.0),
            car_at(20.0, 4.0, 18.0),
        ];
        let (lane, follow) = choose_lane(1, 0.0, 0.0, 30.0, 6.0, 3.0, &obs);
        assert_eq!((lane, follow), (1, Some(18.0)));
    }

    #[test]
    fn a_car_alongside_blocks_the_way_across() {
        // Blocked ahead in the left lane, a car alongside in the centre:
        // the right lane is free but only reachable through the centre.
        let obs = [car_at(30.0, -4.0, 10.0), car_at(0.0, 0.0, 30.0)];
        let (lane, _) = choose_lane(0, -4.0, 0.0, 30.0, 6.0, 3.0, &obs);
        assert_eq!(lane, 0);
    }
}
