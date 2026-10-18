//! The bots that prove a track drivable. DESIGN §2.4.
//!
//! Two drivers share one set of hands ([`control`]) and differ in what they
//! know and how well they act on it:
//!
//! - [`perfect`] sees the whole track and the exact traffic schedule, reacts
//!   instantly, follows a forward/backward speed profile and searches which
//!   corners to drift. It sets the *ceiling*: the best time the track allows.
//! - [`human`] sees late (150 ms), sees only as far as `1.2 s` of travel, a
//!   crest or 120 m allow, aims a little off its line, sometimes lapses, and
//!   drifts only where a person would. It sets the *floor* the timer is
//!   calibrated against, and the margin every track must leave it.
//!
//! The solver (`solver.rs`) runs both: the timer comes from the human's
//! times, the difficulty band from the gap between the two.
//!
//! Both drivers are deterministic. The human's noise comes from its own
//! PCG32 seeded per trial, so a trial replays exactly from its seed, and a
//! trial is also an ordinary race with an ordinary input log.

pub mod control;
pub mod human;
pub mod perfect;
pub mod profile;

use crate::car::CarParams;
use crate::input::Input;
use crate::race::{Phase, Race};
use crate::track::{Segment, Track, SEG_LEN};

/// Anything that can drive a race: given the race so far, the next input.
pub trait Driver {
    /// The input for the coming tick.
    fn input(&mut self, race: &Race) -> Input;
}

/// What a bot's drive left behind, for the solver's per-corner timing and
/// for `trackviz`.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    /// The race tick on which the car first reached each segment;
    /// `u32::MAX` for segments never reached.
    pub seg_tick: Vec<u32>,
    /// `(z, x, v)` every [`SAMPLE_TICKS`] ticks.
    pub samples: Vec<(f64, f64, f64)>,
}

/// Ticks between [`Trace::samples`].
pub const SAMPLE_TICKS: u32 = 6;

impl Trace {
    /// Ticks the car took from segment `a` to segment `b`, if it reached
    /// both.
    pub fn ticks_between(&self, a: usize, b: usize) -> Option<u32> {
        let (ta, tb) = (*self.seg_tick.get(a)?, *self.seg_tick.get(b)?);
        (ta != u32::MAX && tb != u32::MAX).then(|| tb.saturating_sub(ta))
    }
}

/// Drive `race` to its end with `driver`.
pub fn drive(race: &mut Race, driver: &mut impl Driver) {
    while race.phase() == Phase::Racing {
        let input = driver.input(race);
        race.step(input);
    }
}

/// Drive `race` to its end, recording a [`Trace`].
pub fn drive_traced(race: &mut Race, driver: &mut impl Driver) -> Trace {
    let n = race.track().len();
    let mut trace = Trace {
        seg_tick: vec![u32::MAX; n],
        samples: Vec::new(),
    };
    let mut reached = 0usize;
    trace.seg_tick[0] = 0;
    while race.phase() == Phase::Racing {
        let input = driver.input(race);
        race.step(input);
        let i = race.track().index_at(race.car().z);
        while reached < i {
            reached += 1;
            trace.seg_tick[reached] = race.tick();
        }
        if race.tick().is_multiple_of(SAMPLE_TICKS) {
            let c = race.car();
            trace.samples.push((c.z, c.x, c.v));
        }
    }
    trace
}

/// A corner: a run of segments where grip, not top speed, limits the car.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Corner {
    /// First segment.
    pub start: usize,
    /// One past the last segment.
    pub end: usize,
    /// +1 for a right-hander, -1 for a left-hander.
    pub sign: f64,
    /// The lowest grip speed through the corner, m/s.
    pub min_grip_speed: f64,
    /// Whether the generator marked it drift-required or drift-hinted.
    pub signed: bool,
}

/// A segment's grip speed binds below this fraction of its top speed.
const BINDS: f64 = 0.97;

/// Segments of a gap that still joins two bends of the same hand into one
/// corner.
const JOIN_GAP: usize = 3;

/// The corners of `track` for a car of `params`. A better-gripping car sees
/// fewer and shorter corners: a sweep it can take flat out is not one.
pub fn corners(track: &Track, params: &CarParams) -> Vec<Corner> {
    let binds = |s: &Segment| params.grip_speed(s) < BINDS * params.vmax_on(s);
    let segs = track.segments();
    let mut out: Vec<Corner> = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if !binds(&segs[i]) {
            i += 1;
            continue;
        }
        let sign = if segs[i].curve_milli > 0 { 1.0 } else { -1.0 };
        let start = i;
        let mut min = f64::INFINITY;
        let mut signed = false;
        while i < segs.len() && binds(&segs[i]) && (segs[i].curve_milli > 0) == (sign > 0.0) {
            min = min.min(params.grip_speed(&segs[i]));
            signed |= segs[i].has(crate::track::flags::DRIFT_REQ | crate::track::flags::DRIFT_HINT);
            i += 1;
        }
        match out.last_mut() {
            Some(prev) if prev.sign == sign && start - prev.end <= JOIN_GAP => {
                prev.end = i;
                prev.min_grip_speed = prev.min_grip_speed.min(min);
                prev.signed |= signed;
            }
            _ => out.push(Corner {
                start,
                end: i,
                sign,
                min_grip_speed: min,
                signed,
            }),
        }
    }
    out
}

/// Length of a corner, metres.
#[inline]
pub fn corner_len(c: &Corner) -> f64 {
    (c.end - c.start) as f64 * SEG_LEN
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::fixtures;

    #[test]
    fn the_hairpin_is_a_corner_and_the_sweep_is_not() {
        let t = fixtures::sweep_and_hairpin();
        let cs = corners(&t, &CarParams::BASE);
        assert_eq!(cs.len(), 1, "{cs:?}");
        let c = cs[0];
        assert_eq!(c.sign, -1.0);
        assert!(c.start >= 130 && c.end <= 170);
        assert!(c.min_grip_speed < 30.0);
        assert!(!c.signed);
    }

    #[test]
    fn more_grip_shrinks_the_corners() {
        let t = fixtures::sweep_and_hairpin();
        let base = corners(&t, &CarParams::BASE);
        let grippy = corners(
            &t,
            &CarParams {
                grip: 30.0,
                ..CarParams::BASE
            },
        );
        assert!(corner_len(&grippy[0]) <= corner_len(&base[0]));
    }
}
