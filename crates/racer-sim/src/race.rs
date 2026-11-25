//! One race on one track: the car, the checkpoint timer and the input log.
//! DESIGN §1.2.
//!
//! The timer is the arcade kind. A race starts with the first section's
//! allotment; each checkpoint crossed adds the next section's, and whatever
//! was left over carries forward. The timer reaching zero before the next
//! gate is the only way to lose ([`Phase::TimeUp`]). The time left when a
//! gate is crossed is that checkpoint's *margin*: the solver's measure of
//! how close a driver came to losing there (DESIGN §2.4).
//!
//! Time is an integer tick count, never an accumulated float, so "the timer
//! ran out on tick 5,000" means the same thing on every platform.
//!
//! Collisions are the race's job, not the car's: the car knows the road
//! under it, the race knows what else is on it. Hitting a roadside object or
//! a traffic car spins the car (DESIGN §2.6); a spinning car is not hit
//! again, so one mistake costs one spin.
//!
//! The race records its own input log as it is driven: one entry per change
//! of input, with the ticks since the previous change. Any driver — a player,
//! a bot — produces a replayable log without doing anything (`replay.rs`).

use std::sync::Arc;

use crate::car::{Car, CarParams};
use crate::hash::Hasher;
use crate::input::Input;
use crate::replay::Event;
use crate::track::Track;
use crate::traffic::{CAR_HALF_WIDTH, CAR_LEN};

/// Hard cap on a race's length, ticks: four minutes. Only an untimed race
/// (a track not yet calibrated, DESIGN §2.4) can get near it; a timed race
/// ends on its timer long before. A track longer than
/// [`RACE_CAP_SPEED`] × four minutes gets longer ([`race_cap`]).
pub const MAX_RACE_TICKS: u32 = 240 * crate::TICKS_PER_SEC;

/// The average speed, m/s, the race cap allows a long track ([`race_cap`]):
/// far below any driver's, so the cap never ends a race that is moving.
pub const RACE_CAP_SPEED: f64 = 16.0;

/// The hard cap on a race on `track`, ticks: [`MAX_RACE_TICKS`], or for a
/// track whose finish is more than 3,840 m away (a long layout, DESIGN
/// §9.4; no generated track is that long) the time to the finish at
/// [`RACE_CAP_SPEED`].
pub fn race_cap(track: &Track) -> u32 {
    let long = (track.finish_z() / RACE_CAP_SPEED * crate::TICKS_PER_SEC as f64).ceil() as u32;
    MAX_RACE_TICKS.max(long)
}

/// Where the race is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Driving.
    Racing,
    /// Crossed the finish line.
    Finished,
    /// The timer ran out before the next gate.
    TimeUp,
}

/// A race in progress or finished. Cheap to clone: the track is shared, so
/// a bot can fork a race to try a line and throw the copy away.
#[derive(Clone, Debug)]
pub struct Race {
    track: Arc<Track>,
    params: CarParams,
    car: Car,
    tick: u32,
    timer: u32,
    next_gate: usize,
    phase: Phase,
    /// Ticks left on the timer as each gate was crossed.
    margins: Vec<u32>,
    /// The race tick on which each gate was crossed.
    splits: Vec<u32>,
    /// Collisions so far.
    hits: u32,
    log: Vec<Event>,
    input: Input,
    last_change: u32,
    /// [`race_cap`] of the track, kept: derived, never hashed.
    cap: u32,
}

impl Race {
    /// A race on `track` with a car of `params`, on the start line.
    ///
    /// An uncalibrated track (no allotments) races untimed, capped at
    /// [`race_cap`]: that is how calibration measures it.
    pub fn new(track: Arc<Track>, params: CarParams) -> Self {
        let timer = track
            .allot_ticks()
            .first()
            .copied()
            .unwrap_or_else(|| race_cap(&track));
        let cap = race_cap(&track);
        Race {
            track,
            params,
            car: Car::new(),
            tick: 0,
            timer,
            next_gate: 0,
            phase: Phase::Racing,
            margins: Vec::new(),
            splits: Vec::new(),
            hits: 0,
            log: Vec::new(),
            input: Input::NEUTRAL,
            last_change: 0,
            cap,
        }
    }

    /// Advance one tick with `input`. Does nothing once the race is over.
    pub fn step(&mut self, input: Input) {
        if self.phase != Phase::Racing {
            return;
        }
        if input != self.input {
            self.log.push(Event::Input {
                delta: self.tick - self.last_change,
                input,
            });
            self.input = input;
            self.last_change = self.tick;
        }
        self.car.step(input, &self.track, &self.params);
        // Every tick costs a tick of timer; a gate reached on the tick the
        // timer empties still counts, with a margin of zero.
        self.tick += 1;
        self.collide();
        self.timer -= 1;
        self.cross_gates();
        if self.phase == Phase::Racing && (self.timer == 0 || self.tick >= self.cap) {
            self.phase = Phase::TimeUp;
        }
    }

    /// Spin the car if it overlaps an object or a traffic car. The car
    /// glances off an object, pushed sideways clear of it, so a spin never
    /// leaves it parked inside one. A rear-ended car also takes the player
    /// down to below its own speed, so the player comes out of the spin
    /// behind it rather than in it.
    fn collide(&mut self) {
        if self.car.spinning() {
            return;
        }
        let (z, x) = (self.car.z, self.car.x);
        let seg = self.track.index_at(z);
        let near = self.track.placed_in(seg.saturating_sub(1), seg + 2);
        let object = near.iter().find(|o| {
            crate::dmath::abs(z - o.z) < CAR_LEN / 2.0 + o.radius
                && crate::dmath::abs(x - o.x) < CAR_HALF_WIDTH + o.radius
        });
        let rear_ended = self
            .track
            .lanes()
            .iter()
            .find(|c| c.hits(self.tick, z, x))
            .map(|c| c.speed);
        if let Some(o) = object {
            let clear = o.radius + CAR_HALF_WIDTH + 0.05;
            self.car.x = if x < o.x { o.x - clear } else { o.x + clear };
            self.car.vx = 0.0;
        }
        if object.is_some() || rear_ended.is_some() {
            self.hits += 1;
            self.car.spin();
        }
        if let Some(v) = rear_ended {
            self.car.v = self.car.v.min(0.9 * v);
        }
    }

    /// Record every gate the car has reached this tick (a fast car can
    /// reach two only on a hand-written track; the loop costs nothing).
    fn cross_gates(&mut self) {
        let gates = self.track.checkpoints().len();
        while self.next_gate < gates && self.car.z >= self.track.checkpoint_z(self.next_gate) {
            self.margins.push(self.timer);
            self.splits.push(self.tick);
            self.next_gate += 1;
            if self.next_gate == gates {
                self.phase = Phase::Finished;
            } else if let Some(&a) = self.track.allot_ticks().get(self.next_gate) {
                self.timer += a;
            }
        }
    }

    /// The track.
    #[inline]
    pub fn track(&self) -> &Arc<Track> {
        &self.track
    }

    /// The car's performance in this race.
    #[inline]
    pub fn params(&self) -> &CarParams {
        &self.params
    }

    /// The car.
    #[inline]
    pub fn car(&self) -> &Car {
        &self.car
    }

    /// Ticks since the start.
    #[inline]
    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// Ticks left on the timer.
    #[inline]
    pub fn timer(&self) -> u32 {
        self.timer
    }

    /// Where the race is.
    #[inline]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Index of the next gate to cross.
    #[inline]
    pub fn next_gate(&self) -> usize {
        self.next_gate
    }

    /// Timer ticks left at each gate crossed so far.
    #[inline]
    pub fn margins(&self) -> &[u32] {
        &self.margins
    }

    /// The tick each gate was crossed on.
    #[inline]
    pub fn splits(&self) -> &[u32] {
        &self.splits
    }

    /// The input log so far: one event per change of input.
    #[inline]
    pub fn log(&self) -> &[Event] {
        &self.log
    }

    /// Collisions so far.
    #[inline]
    pub fn hits(&self) -> u32 {
        self.hits
    }

    /// The input in effect.
    #[inline]
    pub fn input(&self) -> Input {
        self.input
    }

    /// Smallest margin over the gates crossed, ticks; 0 if the timer ran out.
    pub fn min_margin(&self) -> u32 {
        if self.phase == Phase::TimeUp {
            return 0;
        }
        self.margins.iter().copied().min().unwrap_or(0)
    }

    /// Fold the race into a running hash: every float by bit pattern, the
    /// clock, the gates and the log. Part of the run state hash (DESIGN §8).
    pub fn hash_into(&self, h: &mut Hasher) {
        let c = &self.car;
        for f in [c.z, c.x, c.v, c.vx, c.charge] {
            h.write_f64(f);
        }
        h.write_u8(match c.drift {
            crate::car::Drift::Grip => 0,
            crate::car::Drift::Entry(n) => 0x10 | n,
            crate::car::Drift::Drifting => 2,
            crate::car::Drift::Spin(n) => n.wrapping_add(0x80),
        });
        h.write_u32(c.boost_ticks as u32);
        h.write_u8(c.last_boost.map_or(0, |b| b as u8 + 1));
        h.write_u32(self.tick);
        h.write_u32(self.timer);
        h.write_usize(self.next_gate);
        h.write_u8(self.phase as u8);
        h.write_u32(self.hits);
        for (&m, &s) in self.margins.iter().zip(&self.splits) {
            h.write_u32(m);
            h.write_u32(s);
        }
        crate::replay::hash_events(h, &self.log);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::fixtures;
    use crate::TICKS_PER_SEC;

    fn timed(allot: &[u32]) -> Arc<Track> {
        let mut t = fixtures::sweep_and_hairpin();
        t.set_allotments(allot.to_vec());
        Arc::new(t)
    }

    /// Drive with a crude lane-keeper: steer toward the centre line.
    fn drive(race: &mut Race) {
        while race.phase() == Phase::Racing {
            let x = race.car().x;
            let steer = if x > 0.5 {
                -1
            } else if x < -0.5 {
                1
            } else {
                0
            };
            race.step(Input::new(steer, false, false));
        }
    }

    #[test]
    fn an_untimed_race_records_its_splits() {
        let mut race = Race::new(Arc::new(fixtures::sweep_and_hairpin()), CarParams::BASE);
        drive(&mut race);
        assert_eq!(race.phase(), Phase::Finished);
        assert_eq!(race.splits().len(), 2);
        assert!(race.splits()[0] < race.splits()[1]);
    }

    #[test]
    fn leftover_time_carries_into_the_next_section() {
        let mut probe = Race::new(Arc::new(fixtures::sweep_and_hairpin()), CarParams::BASE);
        drive(&mut probe);
        let s = probe.splits().to_vec();
        let slack = 2 * TICKS_PER_SEC;
        let mut race = Race::new(timed(&[s[0] + slack, s[1] - s[0]]), CarParams::BASE);
        drive(&mut race);
        assert_eq!(race.phase(), Phase::Finished);
        // Same driving, so the same splits; the first section's slack is
        // still on the clock at the finish.
        assert_eq!(race.splits(), &s[..]);
        assert_eq!(race.margins(), &[slack, slack]);
        assert_eq!(race.min_margin(), slack);
    }

    #[test]
    fn the_timer_running_out_ends_the_race() {
        let mut race = Race::new(timed(&[TICKS_PER_SEC, 1000]), CarParams::BASE);
        drive(&mut race);
        assert_eq!(race.phase(), Phase::TimeUp);
        assert_eq!(race.tick(), TICKS_PER_SEC);
        assert_eq!(race.min_margin(), 0);
        // A finished race ignores further input.
        let before = race.tick();
        race.step(Input::NEUTRAL);
        assert_eq!(race.tick(), before);
    }

    /// Drive the hazards fixture holding a lane, `x` metres off centre.
    fn hold_lane(x: f64, ticks: u32) -> Race {
        let mut race = Race::new(Arc::new(fixtures::hazards()), CarParams::BASE);
        for _ in 0..ticks {
            let dx = x - race.car().x;
            let steer = if dx > 0.3 {
                1
            } else if dx < -0.3 {
                -1
            } else {
                0
            };
            race.step(Input::new(steer, false, false));
        }
        race
    }

    #[test]
    fn the_post_row_spins_a_car_in_the_right_lane() {
        let race = hold_lane(4.0, 8 * TICKS_PER_SEC);
        assert!(race.hits() >= 1);
        // Down the centre, posts, rock and traffic are all clear until the
        // slow car ahead is caught.
        let clear = hold_lane(0.0, 5 * TICKS_PER_SEC);
        assert_eq!(clear.hits(), 0);
    }

    #[test]
    fn an_object_hit_glances_the_car_clear_of_it() {
        let race = hold_lane(4.0, 8 * TICKS_PER_SEC);
        assert!(race.hits() >= 1);
        // Pushed inward, clear of the post row at 4 m.
        assert!(race.car().x < 4.0 - 1.1, "{}", race.car().x);
        assert!(race.car().v > 0.0);
    }

    #[test]
    fn rear_ending_traffic_spins_and_slows_the_car() {
        let mut race = hold_lane(0.0, 0);
        let car = race.track().traffic()[0];
        let mut hit_at = None;
        for _ in 0..(40 * TICKS_PER_SEC) {
            race.step(Input::NEUTRAL);
            if race.hits() > 0 {
                hit_at = Some(race.tick());
                break;
            }
        }
        let tick = hit_at.expect("caught the slow car");
        assert!(race.car().spinning());
        assert!(race.car().v <= 0.9 * car.speed());
        // Spinning is a grace period: no second hit while it lasts.
        for _ in 0..40 {
            race.step(Input::NEUTRAL);
        }
        assert_eq!(race.hits(), 1, "hit once at tick {tick}");
    }

    #[test]
    fn only_changes_of_input_are_logged() {
        let mut race = Race::new(Arc::new(fixtures::drag_strip()), CarParams::BASE);
        for _ in 0..10 {
            race.step(Input::NEUTRAL);
        }
        assert!(race.log().is_empty(), "neutral is the starting input");
        let left = Input::new(-1, false, false);
        for _ in 0..5 {
            race.step(left);
        }
        race.step(Input::NEUTRAL);
        assert_eq!(
            race.log(),
            &[
                Event::Input {
                    delta: 10,
                    input: left
                },
                Event::Input {
                    delta: 5,
                    input: Input::NEUTRAL
                },
            ]
        );
    }
}
