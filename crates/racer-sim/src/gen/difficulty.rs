//! The difficulty curve: one number per track, and what it turns into.
//! DESIGN §2.5.
//!
//! `D(index, branch)` runs from about 0.15 on the first track to about 0.85
//! on the hard side of the last fork. The fork's two sides straddle the
//! index's value, so the route choice is also a difficulty choice: branch 0
//! is the easier road, branch 1 the harder one. Every generator knob below
//! reads `D`, never the index directly, except where a mechanic is
//! *introduced* at a given track (drift-required corners from track 3, ice
//! from track 3), which is a teaching decision rather than a difficulty one.

use crate::TICKS_PER_SEC;

/// Tracks in a run.
pub const TRACKS: u32 = 5;

/// The difficulty of track `(index, branch)`, in `0..1`.
pub fn difficulty(index: u32, branch: u8) -> f64 {
    const BASE: [f64; TRACKS as usize] = [0.15, 0.32, 0.5, 0.65, 0.78];
    let base = BASE[index.min(TRACKS - 1) as usize];
    if index == 0 {
        base
    } else if branch == 0 {
        base - 0.06
    } else {
        base + 0.06
    }
}

/// The margin, ticks, the human bot must keep at every checkpoint of track
/// `index` (DESIGN §2.4): 2 s on the first track down to 1 s on the last.
/// Early tracks forgive more.
pub fn required_margin(index: u32) -> u32 {
    let m = 2.0 - 0.25 * index.min(TRACKS - 1) as f64;
    crate::ticks(m)
}

/// Drift-required corners on track `index`: none on the first two, one from
/// the third, two on the hard side of the last fork. DESIGN §2.5.
pub fn drift_required(index: u32, d: f64) -> usize {
    match index {
        0 | 1 => 0,
        _ if d > 0.8 => 2,
        _ => 1,
    }
}

/// The track length aimed for, segments: about a minute of driving.
pub const TARGET_SEGS: usize = 600;

/// The perfect bot's time left at the finish must fall in this band, ticks
/// (DESIGN §2.4): less is too hard to master, more is too lax to matter.
pub const PERFECT_LEFT: (u32, u32) = (6 * TICKS_PER_SEC, 18 * TICKS_PER_SEC);

/// The human bot's time over the perfect bot's must fall in this band, as a
/// fraction of the perfect time (DESIGN §2.4).
pub const SKILL_GAP: (f64, f64) = (0.08, 0.30);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difficulty_rises_with_the_index_and_forks_straddle_it() {
        for i in 1..TRACKS {
            assert!(difficulty(i, 0) < difficulty(i, 1));
            assert!(difficulty(i - 1, 1) < difficulty(i, 1));
        }
        assert!(difficulty(TRACKS - 1, 1) < 1.0);
    }

    #[test]
    fn margins_relax_from_two_seconds_to_one() {
        assert_eq!(required_margin(0), 240);
        assert_eq!(required_margin(4), 120);
        assert_eq!(required_margin(9), 120);
    }
}
