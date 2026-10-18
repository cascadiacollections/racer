//! Racer — deterministic simulation core.
//!
//! This crate is the whole game: track generation, the car, the race timer,
//! the run and its forks, and the bots that prove every generated track can
//! be driven. The Godot crate (`racer-godot`) will render what this crate
//! says is true, read input and play sound (DESIGN §6). Keeping every rule on
//! this side of the split is what makes a run reproducible from its seed and
//! its inputs, and what lets the rules be tested headless, in bulk, without
//! an engine.
//!
//! Invariants (see design/DESIGN.md):
//!   * No Godot dependency. Builds and tests headless.
//!   * Fixed timestep [`DT`], semi-implicit Euler, `f64` throughout; time
//!     itself is an integer tick count.
//!   * Explicit PRNG only. No thread_rng, no HashMap iteration order,
//!     no system time in any state-affecting path.
//!   * No libm: trig and friends come from [`dmath`].
//!
//! Units used throughout: metres, seconds and radians. A track is a list of
//! 4 m segments whose shape is stored as small integers, so a track is the
//! same on every platform by construction, not by luck of float rounding.

pub mod dmath; // platform-stable trig, so the sim never calls libm
pub mod hash; // FNV-1a, for track digests and the run state hash
pub mod rng; // SplitMix64, PCG32, named substreams
pub mod track; // segments, checkpoints, sight lines, the builder
pub mod viz; // ASCII strip of a track, for tuning and snapshots

/// Fixed simulation timestep: 120 ticks a second. Never make this variable.
pub const DT: f64 = 1.0 / 120.0;

/// Ticks per second, for converting the integer clock to and from seconds.
pub const TICKS_PER_SEC: u32 = 120;

/// Seconds to whole ticks, rounding to the nearest tick.
#[inline]
pub fn ticks(seconds: f64) -> u32 {
    dmath::round(seconds * TICKS_PER_SEC as f64).max(0.0) as u32
}

/// Whole ticks to seconds.
#[inline]
pub fn seconds(ticks: i64) -> f64 {
    ticks as f64 * DT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_and_seconds_round_trip() {
        assert_eq!(ticks(1.0), TICKS_PER_SEC);
        assert_eq!(ticks(0.25), 30);
        assert_eq!(ticks(-3.0), 0);
        assert_eq!(seconds(ticks(60.0) as i64), 60.0);
    }
}
