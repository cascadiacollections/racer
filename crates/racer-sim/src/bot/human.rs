//! The human-like bot: the floor a track must leave room for. DESIGN §2.4.
//!
//! It drives with the same hands as the perfect bot ([`super::control`]) but
//! with a person's limits, each one a constant below so it can be tuned
//! against playtests (M2):
//!
//! - **Late.** It acts on what it saw [`DELAY`] ticks (150 ms) ago, give or
//!   take [`JITTER`]: its own speed and slide, and the traffic.
//! - **Short-sighted.** It plans its speed over `min(1.2 s of travel,
//!   120 m, the crest's sight line)`; traffic and objects it sees out to
//!   `min(120 m, the sight line)`, since a car on the road ahead stands out
//!   long before the bend it is on needs planning. When a crest cuts the view short it assumes the
//!   unseen road might need [`BLIND_SPEED`], and slows for it.
//! - **Careful.** It uses [`CAUTION`] of the tyres' grip and brakes for a
//!   corner as if the brakes were [`PLAN_BRAKE`] as strong.
//! - **Imprecise.** It aims up to a lane-quarter off its line, a fresh error
//!   every half second, and holds a steering level until the one it wants is
//!   clearly different (hysteresis, as a thumb on a touch zone does).
//! - **Overconfident, now and then.** Each corner, with chance
//!   [`OVERCOOK_CHANCE`] (drawn once per corner per trial), it misjudges the
//!   speed and carries [`OVERCOOK`] times too much in, past what the tyres
//!   can hold, and runs wide.
//! - **Fallible.** Now and then ([`LAPSE_PER_SEC`]) it stops updating its
//!   input for a moment and holds whatever it was doing.
//! - **Plain.** It drifts only corners a person would: signed ones, and long
//!   tight ones.
//!
//! All of its randomness comes from one PCG32 per trial, so a trial is as
//! reproducible as the sim, and different trials are different drivers on a
//! different day.

use std::sync::Arc;

use super::control::{choose_lane, closing_on, lane_pos, obstacles, Obstacle, Steering};
use super::perfect::corner_index;
use super::profile::{drift_mask, limits};
use super::{corner_len, corners, Corner, Driver};
use crate::car::{CarParams, Drift, DAMP_DRIFT, DAMP_GRIP};
use crate::dmath::sqrt;
use crate::input::Input;
use crate::race::Race;
use crate::rng::Pcg32;
use crate::track::{Track, SEG_LEN};
use crate::traffic::CAR_LEN;

/// Reaction delay, ticks: 150 ms.
pub const DELAY: u32 = 18;
/// Reaction delay varies by up to this many ticks either way.
pub const JITTER: u32 = 4;
/// Look-ahead in seconds of travel.
pub const LOOK_TIME: f64 = 1.2;
/// Look-ahead cap, metres.
pub const LOOK_MAX: f64 = 120.0;
/// Speed it assumes the unseen road past a crest might need, m/s.
pub const BLIND_SPEED: f64 = 30.0;
/// Share of the tyres' grip it plans to use.
pub const CAUTION: f64 = 0.85;
/// Braking it plans with, as a share of the brakes.
pub const PLAN_BRAKE: f64 = 0.6;
/// Chance per corner that it carries too much speed in.
pub const OVERCOOK_CHANCE: f64 = 0.04;
/// How much too much: a multiple of the speed it meant to take.
pub const OVERCOOK: f64 = 1.15;
/// Standard deviation of its aim, as a share of the road's half width.
pub const AIM_SIGMA: f64 = 0.08;
/// Ticks between fresh draws of aim error, delay and braking.
pub const RESAMPLE: u32 = 60;
/// Chance per second of a lapse.
pub const LAPSE_PER_SEC: f64 = 0.01;
/// A lapse lasts this many ticks, low to high.
pub const LAPSE_TICKS: (u32, u32) = (36, 96);
/// It drifts a corner whose grip speed is below this, m/s ...
pub const DRIFT_BELOW: f64 = 30.0;
/// ... and that is at least this long, metres.
pub const DRIFT_MIN_LEN: f64 = 60.0;
/// Seconds ahead it checks lanes.
pub const HORIZON: f64 = 2.5;
/// Metres toward the inside of a corner it aims.
pub const APEX: f64 = 1.0;

/// What the human knows about a track and car before it drives: shared by
/// every trial on that track and loadout, so it is computed once.
#[derive(Debug)]
pub struct HumanPlan {
    corners: Vec<Corner>,
    drift: Vec<bool>,
    corner_of: Vec<Option<u16>>,
    limits: Vec<f64>,
}

impl HumanPlan {
    /// The plan for `track` and `params`.
    pub fn new(track: &Track, params: &CarParams) -> Arc<Self> {
        let cs = corners(track, params);
        let drift: Vec<bool> = cs
            .iter()
            .map(|c| c.signed || (c.min_grip_speed < DRIFT_BELOW && corner_len(c) >= DRIFT_MIN_LEN))
            .collect();
        let mask = drift_mask(track.len(), &cs, &drift);
        let limits = limits(track, params, CAUTION, &mask);
        let corner_of = corner_index(track.len(), &cs, super::perfect::LEAD);
        Arc::new(HumanPlan {
            corners: cs,
            drift,
            corner_of,
            limits,
        })
    }

    /// Which corners it drifts.
    pub fn drift(&self) -> &[bool] {
        &self.drift
    }
}

/// What the human saw on one tick.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    tick: u32,
    z: f64,
    x: f64,
    v: f64,
    vx: f64,
}

/// Ring of past observations; long enough for the longest delay.
const MEMORY: usize = 32;

/// One human-like driver, for one trial.
#[derive(Clone, Debug)]
pub struct HumanBot {
    plan: Arc<HumanPlan>,
    /// The plan's limits with this driver's misjudgement of each corner,
    /// squared: the look-ahead only ever needs the square.
    limits_sq: Vec<f64>,
    rng: Pcg32,
    memory: [Seen; MEMORY],
    delay: u32,
    aim: f64,
    decel_share: f64,
    lapse: u32,
    last: Input,
    steer: Steering,
    lane: usize,
    obs: Vec<Obstacle>,
}

impl HumanBot {
    /// A driver following `plan`, its noise drawn from `seed`.
    pub fn new(plan: Arc<HumanPlan>, seed: u64) -> Self {
        let mut limits = plan.limits.clone();
        let mut bot = HumanBot {
            limits_sq: Vec::new(),
            plan,
            rng: Pcg32::new(seed),
            memory: [Seen::default(); MEMORY],
            delay: DELAY,
            aim: 0.0,
            decel_share: PLAN_BRAKE,
            lapse: 0,
            last: Input::NEUTRAL,
            steer: Steering::new(2.5, 3.5, 6.0, 0.8),
            lane: 1,
            obs: Vec::new(),
        };
        for k in 0..bot.plan.corners.len() {
            if bot.rng.next_f64() < OVERCOOK_CHANCE {
                let c = bot.plan.corners[k];
                for l in &mut limits[c.start..c.end] {
                    *l *= OVERCOOK;
                }
            }
        }
        for l in &mut limits {
            *l *= *l;
        }
        bot.limits_sq = limits;
        bot
    }

    /// A roughly normal draw: four uniforms summed, centred and scaled to a
    /// standard deviation of one. No `exp` or `ln` needed.
    fn normal(&mut self) -> f64 {
        let s: f64 = (0..4).map(|_| self.rng.next_f64()).sum();
        (s - 2.0) / sqrt(4.0 / 12.0)
    }

    /// New aim error, delay and braking for the next stretch.
    fn resample(&mut self, hw: f64) {
        self.aim = self.normal() * AIM_SIGMA * hw;
        self.delay = DELAY - JITTER + self.rng.range_u32(0, 2 * JITTER);
        self.decel_share = PLAN_BRAKE * self.rng.range_f64(0.9, 1.1);
    }

    /// The speed it is willing to carry here: the most from which every
    /// limit it can see is reachable braking as it plans to.
    fn target_speed(&self, track: &Track, seen: &Seen, decel: f64) -> f64 {
        let i = track.index_at(seen.z);
        let sight = track.sight(i);
        let wanted = (LOOK_TIME * seen.v).min(LOOK_MAX);
        let look = wanted.min(sight);
        // The lowest of `sqrt(limit² + 2·decel·d)` over the road in view.
        // The square root is exactly rounded, so it never reorders two
        // values: the root of the lowest square is the lowest root, bit for
        // bit, and one root does for the whole look-ahead.
        let mut v2 = f64::INFINITY;
        let last = track.index_at(seen.z + look);
        let mut edge = i as f64 * SEG_LEN;
        for &lim2 in self.limits_sq.get(i..=last).unwrap_or(&[]) {
            let d = (edge - seen.z).max(0.0);
            let reach2 = lim2 + 2.0 * decel * d;
            // `f64::min` by hand: no number here is NaN, and the plain
            // comparison is a single instruction.
            if reach2 < v2 {
                v2 = reach2;
            }
            edge += SEG_LEN;
        }
        if sight < wanted {
            v2 = v2.min(BLIND_SPEED * BLIND_SPEED + 2.0 * decel * look);
        }
        sqrt(v2)
    }
}

impl Driver for HumanBot {
    fn input(&mut self, race: &Race) -> Input {
        let (car, track, p) = (race.car(), race.track(), race.params());
        let tick = race.tick();
        self.memory[tick as usize % MEMORY] = Seen {
            tick,
            z: car.z,
            x: car.x,
            v: car.v,
            vx: car.vx,
        };
        if tick.is_multiple_of(RESAMPLE) {
            self.resample(track.shape(track.index_at(car.z)).half_width);
        }
        if self.lapse > 0 {
            self.lapse -= 1;
            return self.last;
        }
        if self.rng.next_f64() < LAPSE_PER_SEC * crate::DT {
            self.lapse = self.rng.range_u32(LAPSE_TICKS.0, LAPSE_TICKS.1);
            return self.last;
        }

        let seen = self.memory[tick.saturating_sub(self.delay) as usize % MEMORY];
        let i = track.index_at(seen.z);
        let seg = track.segments()[i];
        let shape = track.shape(i);
        let hw = shape.half_width;
        let decel = p.brake * self.decel_share;

        let look = (LOOK_TIME * seen.v).min(LOOK_MAX).min(track.sight(i));
        obstacles(
            track,
            seen.tick,
            seen.z - CAR_LEN,
            seen.z + look,
            &mut self.obs,
        );
        let corner = self.plan.corner_of[i].map(|k| (k as usize, self.plan.corners[k as usize]));
        let bias = corner.map_or(0.0, |(_, c)| c.sign * APEX);
        let (lane, follow) = choose_lane(self.lane, seen.z, seen.v, hw, bias, HORIZON, &self.obs);
        self.lane = lane;
        let target_x = lane_pos(lane, hw, bias) + self.aim;

        let mut target_v = self.target_speed(track, &seen, decel);
        if let Some(f) = follow {
            target_v = target_v.min(f - 1.0);
        }
        // Still in the path of traffic mid lane change: brake in time for it.
        if let Some((t, speed)) = closing_on(seen.z, seen.v, seen.x, &self.obs) {
            if t < (seen.v - speed) / decel + 0.5 {
                target_v = target_v.min(speed - 1.0);
            }
        }
        let brake = seen.v > target_v + 0.5;

        let (cap, damp) = if car.drifting() {
            (p.drift_grip_on(&seg), DAMP_DRIFT)
        } else {
            (p.grip_on(&seg), DAMP_GRIP)
        };
        let mut level = self.steer.level(
            target_x,
            seen.x,
            seen.vx,
            seen.v,
            shape.curvature,
            cap,
            damp,
        );
        let drift = match corner {
            Some((k, c)) => self.plan.drift[k] && i >= c.start && i < c.end,
            None => false,
        };
        if let Some((_, c)) = corner {
            if drift && car.drift == Drift::Grip && level as f64 * c.sign <= 0.0 {
                level = c.sign as i8;
            }
        }
        self.last = Input::new(level, brake, drift);
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::drive;
    use crate::bot::perfect::perfect_run;
    use crate::race::Phase;
    use crate::track::fixtures;

    fn human_ticks(track: &Arc<Track>, seed: u64) -> Race {
        let plan = HumanPlan::new(track, &CarParams::BASE);
        let mut race = Race::new(track.clone(), CarParams::BASE);
        drive(&mut race, &mut HumanBot::new(plan, seed));
        race
    }

    #[test]
    fn it_finishes_the_fixtures_slower_than_the_perfect_bot() {
        for t in [
            fixtures::sweep_and_hairpin(),
            fixtures::blind_crest(),
            fixtures::hazards(),
        ] {
            let t = Arc::new(t);
            let best = perfect_run(&t, &CarParams::BASE, true).race.tick();
            for seed in 0..4 {
                let r = human_ticks(&t, seed);
                assert_eq!(r.phase(), Phase::Finished);
                assert!(r.tick() > best, "seed {seed}: {} vs {best}", r.tick());
            }
        }
    }

    #[test]
    fn a_trial_is_reproducible_from_its_seed() {
        let t = Arc::new(fixtures::sweep_and_hairpin());
        let (a, b, c) = (human_ticks(&t, 7), human_ticks(&t, 7), human_ticks(&t, 8));
        assert_eq!(a.log(), b.log());
        assert_ne!(a.log(), c.log());
    }

    #[test]
    fn a_blind_crest_costs_the_human_more_than_the_perfect_bot() {
        let crest = Arc::new(fixtures::blind_crest());
        let mut flat_b = crate::track::Builder::new();
        flat_b
            .straight(60)
            .bend(24, 70, 4)
            .straight(40)
            .checkpoint()
            .straight(20);
        let flat = Arc::new(flat_b.build());
        let ratio = |t: &Arc<Track>| {
            let h: u32 = (0..4).map(|s| human_ticks(t, s).tick()).sum();
            h as f64 / (4 * perfect_run(t, &CarParams::BASE, true).race.tick()) as f64
        };
        assert!(
            ratio(&crest) > ratio(&flat),
            "{} vs {}",
            ratio(&crest),
            ratio(&flat)
        );
    }
}
