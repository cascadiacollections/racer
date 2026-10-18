//! The perfect bot: the ceiling of what a track allows. DESIGN §2.4.
//!
//! It knows everything and reacts instantly. It follows a whole-track speed
//! profile ([`super::profile`]) using nearly all of the tyres' grip, picks
//! lanes from the exact traffic schedule, and chooses which corners to drift
//! by a beam search: at each corner, every surviving plan is tried both
//! ways, driven on to the next corner, and the [`BEAM`] plans that got there
//! soonest survive. Drifting a corner is not always faster — the entry flick
//! and the scrub cost speed, and a short corner pays back too little boost —
//! so the search is what finds out.
//!
//! Lanes are not searched. With traffic on a fixed schedule the earliest
//! clear lane is known exactly, so the greedy choice in [`super::control`]
//! is already the informed one; searching it would multiply the cost for
//! little.

use std::sync::Arc;

use super::control::{choose_lane, closing_on, lane_pos, obstacles, Obstacle, Steering};
use super::profile::{backward, drift_mask, limits, target_at};
use super::{corners, drive_traced, Corner, Driver, Trace};
use crate::car::{CarParams, Drift, DAMP_DRIFT, DAMP_GRIP};
use crate::input::Input;
use crate::race::{Phase, Race};
use crate::track::{Track, SEG_LEN};
use crate::traffic::CAR_LEN;

/// Share of the tyres' grip the perfect bot plans to use. Not all of it:
/// at the limit, full lock only just cancels the bend and leaves nothing to
/// correct with.
pub const CAUTION: f64 = 0.93;
/// Braking it plans with, as a share of the brakes.
pub const PLAN_BRAKE: f64 = 0.85;
/// Seconds ahead it checks lanes.
pub const HORIZON: f64 = 3.0;
/// Plans kept per corner by the drift search.
pub const BEAM: usize = 4;
/// Segments before a corner where its drift choice is made: far enough out
/// that the braking for it has not begun.
pub const LEAD: usize = 30;
/// Metres toward the inside of a corner the bot aims, to leave room to slide.
pub const APEX: f64 = 1.0;

/// The perfect driver for one plan: which corners to drift.
#[derive(Clone, Debug)]
pub struct PerfectBot {
    corners: Arc<Vec<Corner>>,
    /// Corner index for each segment, from [`LEAD`] segments before it.
    corner_of: Arc<Vec<Option<u16>>>,
    drift: Vec<bool>,
    limits: Vec<f64>,
    profile: Vec<f64>,
    decel: f64,
    caution: f64,
    steer: Steering,
    lane: usize,
    obs: Vec<Obstacle>,
}

impl PerfectBot {
    /// A bot for `track` and `params` that drifts the corners marked in
    /// `drift` (one entry per corner of [`super::corners`]; missing entries
    /// grip).
    pub fn new(track: &Track, params: &CarParams, drift: Vec<bool>) -> Self {
        Self::with_caution(track, params, drift, CAUTION)
    }

    /// As [`PerfectBot::new`] with a chosen share of grip: the human bot's
    /// clean-driving baseline uses this too.
    pub fn with_caution(
        track: &Track,
        params: &CarParams,
        mut drift: Vec<bool>,
        caution: f64,
    ) -> Self {
        let cs = corners(track, params);
        drift.resize(cs.len(), false);
        let corner_of = corner_index(track.len(), &cs, LEAD);
        let decel = params.brake * PLAN_BRAKE;
        let mut bot = PerfectBot {
            corners: Arc::new(cs),
            corner_of: Arc::new(corner_of),
            drift,
            limits: Vec::new(),
            profile: Vec::new(),
            decel,
            caution,
            steer: Steering::new(3.0, 4.5, 10.0, 0.5),
            lane: 1,
            obs: Vec::new(),
        };
        bot.replan(track, params);
        bot
    }

    /// Recompute the speed profile after a change of plan.
    fn replan(&mut self, track: &Track, params: &CarParams) {
        let mask = drift_mask(track.len(), &self.corners, &self.drift);
        self.limits = limits(track, params, self.caution, &mask);
        self.profile = backward(&self.limits, self.decel);
    }

    /// The plan: whether each corner is drifted.
    pub fn plan(&self) -> &[bool] {
        &self.drift
    }

    /// The corners the plan is over.
    pub fn corners(&self) -> &[Corner] {
        &self.corners
    }
}

impl Driver for PerfectBot {
    fn input(&mut self, race: &Race) -> Input {
        let (car, track, p) = (race.car(), race.track(), race.params());
        let i = track.index_at(car.z);
        let seg = track.segments()[i];
        let hw = seg.half_width();

        obstacles(
            track,
            race.tick(),
            car.z - CAR_LEN,
            car.z + car.v * HORIZON + 10.0,
            &mut self.obs,
        );
        let corner = self.corner_of[i].map(|k| (k as usize, self.corners[k as usize]));
        let bias = corner.map_or(0.0, |(_, c)| c.sign * APEX);
        let (lane, follow) = choose_lane(self.lane, car.z, car.v, hw, bias, HORIZON, &self.obs);
        self.lane = lane;
        let target_x = lane_pos(lane, hw, bias);

        let mut target_v = target_at(&self.profile, &self.limits, car.z, self.decel);
        if let Some(f) = follow {
            target_v = target_v.min(f - 0.5);
        }
        // Still in the path of traffic mid lane change: brake in time for it.
        if let Some((t, speed)) = closing_on(car.z, car.v, car.x, &self.obs) {
            if t < (car.v - speed) / self.decel + 0.5 {
                target_v = target_v.min(speed - 0.5);
            }
        }
        let brake = car.v > target_v + 0.1;

        let (cap, damp) = if car.drifting() {
            (p.drift_grip_on(&seg), DAMP_DRIFT)
        } else {
            (p.grip_on(&seg), DAMP_GRIP)
        };
        let mut level =
            self.steer
                .level(target_x, car.x, car.vx, car.v, seg.curvature(), cap, damp);
        let drift = match corner {
            Some((k, c)) => self.drift[k] && i >= c.start && i < c.end,
            None => false,
        };
        if let Some((_, c)) = corner {
            // A drift is entered steering into the corner.
            if drift && car.drift == Drift::Grip && level as f64 * c.sign <= 0.0 {
                level = c.sign as i8;
            }
        }
        Input::new(level, brake, drift)
    }
}

/// For each segment, the corner it belongs to or approaches: from `lead`
/// segments before a corner's start (but not before the previous corner's
/// end) to its end.
pub fn corner_index(n: usize, corners: &[Corner], lead: usize) -> Vec<Option<u16>> {
    let mut out = vec![None; n];
    let mut prev_end = 0;
    for (k, c) in corners.iter().enumerate() {
        let from = c.start.saturating_sub(lead).max(prev_end);
        out[from..c.end].fill(Some(k as u16));
        prev_end = c.end;
    }
    out
}

/// The result of a perfect drive.
#[derive(Clone, Debug)]
pub struct PerfectRun {
    /// The drift plan driven.
    pub plan: Vec<bool>,
    /// The finished race.
    pub race: Race,
    /// Its trace.
    pub trace: Trace,
    /// Ticks simulated, the search included.
    pub cost: u64,
}

/// The perfect drive of `track`: with `allow_drift`, the best drift plan the
/// beam search finds; without, grip only. The two together are the two-bot
/// test for drift-required corners (DESIGN §2.5).
pub fn perfect_run(track: &Arc<Track>, params: &CarParams, allow_drift: bool) -> PerfectRun {
    let n_corners = corners(track, params).len();
    let (plan, searched) = if allow_drift && n_corners > 0 {
        search(track, params, n_corners)
    } else {
        (vec![false; n_corners], 0)
    };
    let mut bot = PerfectBot::new(track, params, plan.clone());
    let mut race = Race::new(track.clone(), *params);
    let trace = drive_traced(&mut race, &mut bot);
    let cost = searched + race.tick() as u64;
    PerfectRun {
        plan,
        race,
        trace,
        cost,
    }
}

/// Beam search over drift choices, corner by corner. Returns the best plan
/// and the ticks simulated finding it.
fn search(track: &Arc<Track>, params: &CarParams, n_corners: usize) -> (Vec<bool>, u64) {
    let mut cost = 0;
    let root = PerfectBot::new(track, params, Vec::new());
    let starts: Vec<usize> = root
        .corners()
        .iter()
        .map(|c| c.start.saturating_sub(LEAD))
        .collect();
    let mut beam = vec![(Race::new(track.clone(), *params), root)];
    for k in 0..n_corners {
        // Drive each choice on to where the next corner's choice is made.
        let until = starts
            .get(k + 1)
            .map_or(f64::INFINITY, |&s| s as f64 * SEG_LEN);
        let mut next = Vec::with_capacity(beam.len() * 2);
        for (race, bot) in &beam {
            for choice in [false, true] {
                let (mut race, mut bot) = (race.clone(), bot.clone());
                bot.drift[k] = choice;
                bot.replan(track, params);
                while race.phase() == Phase::Racing && race.car().z < until {
                    let input = bot.input(&race);
                    race.step(input);
                    cost += 1;
                }
                next.push((race, bot));
            }
        }
        // Soonest there wins; a race that ran out of time loses. The sort is
        // stable, so ties keep their order and the search is deterministic.
        next.sort_by_key(|(r, _)| (r.phase() == Phase::TimeUp, r.tick()));
        next.truncate(BEAM);
        beam = next;
    }
    (beam.swap_remove(0).1.drift, cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{fixtures, Builder};

    #[test]
    fn it_finishes_the_fixtures_cleanly() {
        for t in [
            fixtures::sweep_and_hairpin(),
            fixtures::blind_crest(),
            fixtures::hazards(),
        ] {
            let run = perfect_run(&Arc::new(t), &CarParams::BASE, true);
            assert_eq!(run.race.phase(), Phase::Finished);
            assert_eq!(run.race.hits(), 0);
        }
    }

    #[test]
    fn it_passes_the_slow_car_without_touching_it() {
        let run = perfect_run(&Arc::new(fixtures::hazards()), &CarParams::BASE, false);
        let car = run.race.track().traffic()[0];
        // The traffic car is 400 m up the road at 20 m/s; the bot ends far
        // ahead of it.
        assert!(run.race.car().z > car.z_at(run.race.tick()) + 100.0);
    }

    #[test]
    fn a_long_tight_corner_is_drifted_and_drifting_it_pays() {
        let mut b = Builder::new();
        b.straight(60)
            .bend(70, 120, 6)
            .straight(60)
            .checkpoint()
            .straight(10);
        let t = Arc::new(b.build());
        let grip = perfect_run(&t, &CarParams::BASE, false);
        let drift = perfect_run(&t, &CarParams::BASE, true);
        assert_eq!(drift.plan, vec![true]);
        assert!(
            drift.race.tick() < grip.race.tick(),
            "{} vs {}",
            drift.race.tick(),
            grip.race.tick()
        );
    }
}
