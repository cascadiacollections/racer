//! Track generation: grammar, overlays, and the draft the solver checks.
//! DESIGN §2.3.
//!
//! A track depends only on `(run_seed, index, branch)` and, inside the
//! solver's ladder, on the attempt number: each attempt is a fresh draw from
//! its own seed. Within an attempt every layer — geometry, surfaces,
//! objects, traffic, narrows — draws from its own named substream.
//!
//! This module only *drafts*. A draft has no timer until calibration sets
//! one, and is not shown to anyone until the solver has proved it
//! (`solver.rs`).

pub mod calibrate;
pub mod difficulty;
pub mod grammar;
pub mod placement;

use difficulty::{difficulty, drift_required};
use grammar::{lay_out, Kind, Layout};

use crate::rng::{substream, track_seed, Pcg32};
use crate::track::Track;

/// Substream tags, one per layer, each the layer's name in ASCII.
const GEOMETRY: u64 = 0x4745_4F4D; // "GEOM"
const SURFACES: u64 = 0x5355_5246; // "SURF"
const OBJECTS: u64 = 0x4F42_4A53; // "OBJS"
const TRAFFIC: u64 = 0x5452_4146; // "TRAF"
const NARROWS: u64 = 0x4E41_5252; // "NARR"
const ATTEMPT: u64 = 0x4154_5054; // "ATPT"

/// A generated track before calibration, with the layout the repair step
/// reads.
#[derive(Clone, Debug)]
pub struct Draft {
    /// The geometry and overlays.
    pub layout: Layout,
    /// The difficulty it was drawn at.
    pub difficulty: f64,
}

impl Draft {
    /// Build the (uncalibrated) track.
    pub fn track(&self) -> Track {
        self.layout.builder.clone().build()
    }
}

/// The seed of attempt `attempt` at track `(index, branch)`.
pub fn attempt_seed(run_seed: u64, index: u32, branch: u8, attempt: u32) -> u64 {
    substream(
        track_seed(run_seed, index, branch),
        ATTEMPT ^ attempt as u64,
    )
}

/// Draft attempt `attempt` of track `(index, branch)`.
pub fn generate(run_seed: u64, index: u32, branch: u8, attempt: u32) -> Draft {
    let seed = attempt_seed(run_seed, index, branch, attempt);
    let d = difficulty(index, branch);
    let stream = |tag| Pcg32::new(substream(seed, tag));
    let mut layout = lay_out(
        &mut stream(GEOMETRY),
        d,
        drift_required(index, d),
        index == 0,
        true,
    );
    placement::narrows(&mut stream(NARROWS), &mut layout, d);
    placement::surfaces(&mut stream(SURFACES), &mut layout, d, index);
    placement::objects(&mut stream(OBJECTS), &mut layout, d, index);
    placement::traffic(&mut stream(TRAFFIC), &mut layout, d);
    Draft {
        layout,
        difficulty: d,
    }
}

/// The sharpest turn the fallback keeps, milliradians per segment.
const FALLBACK_MILLI: i16 = 15;

/// The gentle fallback for track `(index, branch)`: the grammar at the
/// lowest difficulty, no crests, no drift corners, no overlays. The last
/// rung of the solver's ladder (DESIGN §2.4); still calibrated and verified.
pub fn fallback(run_seed: u64, index: u32, branch: u8) -> Draft {
    let seed = attempt_seed(run_seed, index, branch, u32::MAX);
    let mut layout = lay_out(
        &mut Pcg32::new(substream(seed, GEOMETRY)),
        0.0,
        0,
        false,
        false,
    );
    // The lowest difficulty still draws hairpins; ease every bend to at most
    // a 270 m radius, which every loadout takes flat out: nothing to brake
    // for, nothing to misjudge.
    for s in layout.builder.segments_mut() {
        s.curve_milli = s.curve_milli.clamp(-FALLBACK_MILLI, FALLBACK_MILLI);
    }
    Draft {
        layout,
        difficulty: 0.0,
    }
}

/// Soften section `section` of a draft: ease its bends by a quarter, halve
/// its hills, clear its objects and move its traffic on. The repair rung of
/// the solver's ladder.
pub fn soften(draft: &Draft, section: usize) -> Draft {
    let mut out = draft.clone();
    let spans: Vec<_> = out
        .layout
        .spans
        .iter()
        .filter(|s| s.section == section && s.kind != Kind::Launch)
        .copied()
        .collect();
    let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return out;
    };
    let (a, b) = (first.start, last.end);
    for s in &mut out.layout.builder.segments_mut()[a..b] {
        s.curve_milli = (s.curve_milli as i32 * 3 / 4) as i16;
        s.pitch_milli /= 2;
    }
    out.layout
        .builder
        .retain_objects(|o| (o.seg as usize) < a || (o.seg as usize) >= b);
    let (za, zb) = (
        a as f64 * crate::track::SEG_LEN,
        b as f64 * crate::track::SEG_LEN,
    );
    out.layout.builder.shift_traffic(za, zb, 120.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{flags, track_hash};

    #[test]
    fn a_draft_depends_only_on_its_key() {
        let a = generate(7, 2, 1, 0).track();
        assert_eq!(track_hash(&a), track_hash(&generate(7, 2, 1, 0).track()));
        assert_ne!(track_hash(&a), track_hash(&generate(7, 2, 1, 1).track()));
        assert_ne!(track_hash(&a), track_hash(&generate(7, 2, 0, 0).track()));
        assert_ne!(track_hash(&a), track_hash(&generate(8, 2, 1, 0).track()));
    }

    #[test]
    fn tracks_are_about_a_minute_long_with_three_or_four_gates() {
        for seed in 0..20 {
            for index in 0..5 {
                let t = generate(seed, index, 1, 0).track();
                assert!((450..800).contains(&t.len()), "{} segments", t.len());
                assert!((3..=4).contains(&t.checkpoints().len()));
            }
        }
    }

    #[test]
    fn drift_corners_arrive_when_the_design_says() {
        for seed in 0..20 {
            let count = |index, flag| {
                let t = generate(seed, index, 1, 0).track();
                let segs = t.segments();
                (1..segs.len())
                    .filter(|&i| segs[i].has(flag) && !segs[i - 1].has(flag))
                    .count()
            };
            assert_eq!(count(0, flags::DRIFT_HINT), 1);
            assert_eq!(count(0, flags::DRIFT_REQ), 0);
            assert_eq!(count(1, flags::DRIFT_REQ), 0);
            assert!(count(2, flags::DRIFT_REQ) >= 1);
            assert!(count(4, flags::DRIFT_REQ) >= 1);
        }
    }

    #[test]
    fn a_drift_corner_is_approached_without_a_crest() {
        for seed in 0..30 {
            for index in 2..5 {
                let t = generate(seed, index, 1, 0).track();
                let segs = t.segments();
                for i in 1..segs.len() {
                    if segs[i].has(flags::DRIFT_REQ) && !segs[i - 1].has(flags::DRIFT_REQ) {
                        let approach = &segs[i.saturating_sub(grammar::DRIFT_APPROACH)..i];
                        assert!(approach
                            .iter()
                            .all(|s| !s.has(flags::CREST) && s.curve_milli == 0));
                    }
                }
            }
        }
    }

    #[test]
    fn the_fallback_is_gentle() {
        let t = fallback(3, 4, 1).track();
        assert!(t
            .segments()
            .iter()
            .all(|s| s.curve_milli.abs() <= 66 && s.flags == 0));
        assert!(t.objects().is_empty() && t.traffic().is_empty());
    }
}
