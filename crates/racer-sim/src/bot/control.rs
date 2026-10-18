//! The hands both bots share: steering to a line, and picking a lane
//! through traffic and roadside objects.
//!
//! **Steering** is a cascaded controller on the car's offset with a
//! feedforward term. The road's push is known, `κ·v²` outward, so the
//! controller asks for exactly that much plus what moves the car toward the
//! target line at a capped sideways speed, and divides by the steering
//! authority to get a fraction of full lock. That
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
/// A lane change across a lane must find that lane clear this long, s.
const TRANSIT_TIME: f64 = 0.8;
/// With no free lane, the driver still moves over if another lane stays
/// clear this many seconds longer than its own.
const CHANGE_GAIN: f64 = 0.3;

/// The centre of lane `lane` on road of half width `hw`.
#[inline]
pub fn lane_x(lane: usize, hw: f64) -> f64 {
    (lane as f64 - 1.0) * hw * LANE_FRACTION
}

/// A cascaded steering controller quantised to the player's seven levels.
///
/// The outer loop turns the offset error into a wanted sideways speed,
/// capped so a lane change is brisk but never a lunge; the inner loop asks
/// for the acceleration that reaches that speed, plus what the bend's push
/// and the tyres' damping will take away. Capping the sideways speed is what
/// keeps a driver acting on 150 ms old information from overshooting its
/// lane.
#[derive(Clone, Copy, Debug)]
pub struct Steering {
    /// Wanted sideways speed per metre of offset error, 1/s.
    pub k_pos: f64,
    /// Most sideways speed it asks for, m/s.
    pub vx_max: f64,
    /// Acceleration per m/s of sideways speed error, 1/s.
    pub k_vel: f64,
    /// Levels the wanted steering must move by before the held level
    /// changes. 0.5 is plain rounding.
    pub hysteresis: f64,
    level: i8,
}

impl Steering {
    /// A controller holding straight ahead.
    pub const fn new(k_pos: f64, vx_max: f64, k_vel: f64, hysteresis: f64) -> Self {
        Steering {
            k_pos,
            vx_max,
            k_vel,
            hysteresis,
            level: 0,
        }
    }

    /// The steering level that moves the car toward `target_x`, given its
    /// offset `x`, sideways speed `vx`, speed `v`, the road's curvature, the
    /// lateral grip `cap` available and the tyres' damping `damp`.
    #[allow(clippy::too_many_arguments)]
    pub fn level(
        &mut self,
        target_x: f64,
        x: f64,
        vx: f64,
        v: f64,
        curvature: f64,
        cap: f64,
        damp: f64,
    ) -> i8 {
        let authority = cap * clamp(v / V_STEER_FULL, 0.0, 1.0);
        let want = if authority < 1e-6 {
            0.0
        } else {
            let vx_want = clamp(self.k_pos * (target_x - x), -self.vx_max, self.vx_max);
            let a = curvature * v * v + damp * vx_want + self.k_vel * (vx_want - vx);
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
/// sideways band `lo..=hi`, and that obstacle's speed; 0 if one is
/// alongside, infinity if none.
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
/// blocked by traffic. `lane` is the lane held now; `bias` moves every lane
/// toward the inside of a bend, as the driver will aim (see [`lane_pos`]);
/// `horizon` is how many seconds ahead the driver checks. A roadside object is never followed: it
/// does not move, so following it means stopping.
pub fn choose_lane(
    lane: usize,
    z: f64,
    v: f64,
    hw: f64,
    bias: f64,
    horizon: f64,
    obs: &[Obstacle],
) -> (usize, Option<f64>) {
    let in_lane = |l: usize| {
        let lx = lane_pos(l, hw, bias);
        conflict_time(z, v, lx, lx, obs)
    };
    let (t_here, follow) = in_lane(lane);
    if t_here > horizon {
        return (lane, None);
    }
    // Other lanes, and how long each stays clear (capped at twice the
    // horizon: beyond that, all are equal). A lane two over must have the
    // one between clear for the crossing. The longest-clear wins, then the
    // nearest, so the driver does not dart into a lane clear only for now.
    let mut best: Option<(usize, f64, usize)> = None;
    for l in 0..LANES {
        if l == lane {
            continue;
        }
        let between = (l.min(lane) + 1..l.max(lane)).all(|m| in_lane(m).0 > TRANSIT_TIME);
        if !between {
            continue;
        }
        let t = in_lane(l).0.min(2.0 * horizon);
        let d = l.abs_diff(lane);
        if best.is_none_or(|(_, bt, bd)| t > bt || (t == bt && d < bd)) {
            best = Some((l, t, d));
        }
    }
    match best {
        Some((l, t, _)) if t > horizon || t > t_here + CHANGE_GAIN => (l, None),
        // Boxed in: match the traffic ahead, early enough to brake for it.
        _ => (lane, (follow > 0.0).then_some(follow)),
    }
}

/// Where the driver aims in lane `lane`: its centre moved `bias` metres
/// (toward the inside of a bend), kept clear of the road's edges.
#[inline]
pub fn lane_pos(lane: usize, hw: f64, bias: f64) -> f64 {
    clamp(lane_x(lane, hw) + bias, -(hw - EDGE), hw - EDGE)
}

/// Closest the driver aims to the road's edge, metres.
const EDGE: f64 = 1.2;

/// Seconds until the car, where it is now, reaches traffic ahead, and that
/// traffic's speed: what to brake for while a lane change is still under
/// way.
pub fn closing_on(z: f64, v: f64, x: f64, obs: &[Obstacle]) -> Option<(f64, f64)> {
    let (t, speed) = conflict_time(z, v, x, x, obs);
    (t.is_finite() && speed > 0.0).then_some((t, speed))
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
        let mut s = Steering::new(0.0, 0.0, 0.0, 0.5);
        // On the line, the push outward is all there is to cancel.
        assert_eq!(s.level(0.0, 0.0, 0.0, 30.0, 0.01, 27.0, 5.0), 1);
        assert_eq!(s.level(0.0, 0.0, 0.0, 30.0, 0.0, 27.0, 5.0), 0);
        // Stopped, steering does nothing, so the controller does not ask.
        assert_eq!(s.level(5.0, 0.0, 0.0, 0.0, 0.0, 20.0, 5.0), 0);
    }

    #[test]
    fn a_lane_change_asks_for_capped_sideways_speed() {
        let mut s = Steering::new(10.0, 1.0, 0.0, 0.5);
        // Far or near, the wanted sideways speed is capped at 1 m/s, which
        // the damping alone would take 5 m/s² to hold: 0.75 of a level.
        assert_eq!(s.level(8.0, 0.0, 0.0, 20.0, 0.0, 20.0, 5.0), 1);
        assert_eq!(s.level(80.0, 0.0, 0.0, 20.0, 0.0, 20.0, 5.0), 1);
    }

    #[test]
    fn hysteresis_holds_a_level_until_the_want_moves_clearly() {
        let mut s = Steering::new(1.0, 10.0, 0.0, 0.8);
        assert_eq!(s.level(0.0, 0.0, 0.0, 20.0, 0.0, 20.0, 5.0), 0);
        // 0.5 m of error asks 0.5 m/s, 2.5 m/s², under 0.4 of a level: held.
        assert_eq!(s.level(0.5, 0.0, 0.0, 20.0, 0.0, 20.0, 5.0), 0);
        assert_eq!(s.level(3.0, 0.0, 0.0, 20.0, 0.0, 20.0, 5.0), 2);
    }

    #[test]
    fn a_slow_car_ahead_moves_the_driver_over() {
        let obs = [car_at(60.0, 0.0, 20.0)];
        // Closing at 20 m/s on a car 60 m ahead: about 2.6 s away.
        let (t, v) = conflict_time(0.0, 40.0, 0.0, 0.0, &obs);
        assert!((2.0..3.0).contains(&t) && v == 20.0, "{t}");
        let (lane, follow) = choose_lane(1, 0.0, 40.0, 6.0, 0.0, 3.0, &obs);
        assert_ne!(lane, 1);
        assert_eq!(follow, None);
        // Far enough ahead, it stays put.
        assert_eq!(choose_lane(1, -200.0, 40.0, 6.0, 0.0, 3.0, &obs), (1, None));
    }

    #[test]
    fn with_every_lane_blocked_the_driver_follows() {
        let obs = [
            car_at(20.0, -4.0, 18.0),
            car_at(20.0, 0.0, 18.0),
            car_at(20.0, 4.0, 18.0),
        ];
        let (lane, follow) = choose_lane(1, 0.0, 30.0, 6.0, 0.0, 3.0, &obs);
        assert_eq!((lane, follow), (1, Some(18.0)));
    }

    #[test]
    fn a_car_alongside_blocks_the_way_across() {
        // Blocked ahead in the left lane, a car alongside in the centre:
        // the right lane is free but only reachable through the centre.
        let obs = [car_at(30.0, -4.0, 10.0), car_at(0.0, 0.0, 30.0)];
        let (lane, _) = choose_lane(0, 0.0, 30.0, 6.0, 0.0, 3.0, &obs);
        assert_eq!(lane, 0);
    }

    #[test]
    fn a_post_row_at_the_edge_is_left_by_the_lane_beside_it() {
        // Posts a metre inside the right edge pinch the right lane only.
        let posts: Vec<Obstacle> = (0..6)
            .map(|k| Obstacle {
                z: 20.0 + 8.0 * k as f64,
                speed: 0.0,
                x: 5.0,
                half_len: 0.25,
                half_width: 0.25,
            })
            .collect();
        assert_eq!(choose_lane(2, 0.0, 40.0, 6.0, 0.0, 3.0, &posts), (1, None));
        // Aiming a metre inside a right-hander, the centre lane is clear too.
        assert_eq!(choose_lane(1, 0.0, 40.0, 6.0, 1.0, 3.0, &posts), (1, None));
    }
}
