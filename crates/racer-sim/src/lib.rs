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
//! Where to start reading:
//!
//! - [`car`]: the car's two axes of motion and the drift state machine
//!   (DESIGN §3).
//! - [`race`] and [`run`]: the checkpoint timer, and five tracks with forks
//!   and upgrades between them (§1).
//! - [`gen`] and [`solver`]: how a track is drafted, timed and proven
//!   drivable before anyone sees it (§2).
//! - [`bot`]: the perfect and human-like drivers the solver proves with.
//! - [`replay`]: a run persisted as seed plus input log and rebuilt from it,
//!   and [`SIM_EPOCH`]/[`EPOCHS`], which version that promise (§8).
//!
//! Units used throughout: metres, seconds and radians. A track is a list of
//! 4 m segments whose shape is stored as small integers, so a track is the
//! same on every platform by construction, not by luck of float rounding.

pub mod biome; // what a track looks like (presentation only, never simulated)
pub mod bot; // the perfect and human-like drivers that prove a track
pub mod car; // speed, offset, the drift state machine
pub mod codec; // bits, Rice and gamma codes, base32, CRC-16: share codes
pub mod dmath; // platform-stable trig, so the sim never calls libm
pub mod gen; // grammar, overlays, timer calibration
pub mod ghost; // the best race per track, stored and driven back as a ghost
pub mod hash; // FNV-1a, for track digests and the run state hash
pub mod input; // one byte per tick: seven steer levels, brake, drift
pub mod layout; // a track written down: edit, prove, share, race
pub mod race; // one race: the car, the checkpoint timer, the input log
pub mod replay; // input log -> verified reconstruction, and its text form
pub mod rng; // SplitMix64, PCG32, named substreams
pub mod run; // five tracks, forks, stacking upgrades
pub mod solver; // calibrate, verify, and the reject/repair/fallback ladder
pub mod track; // segments, checkpoints, sight lines, the builder
pub mod traffic; // slow cars on closed-form schedules
pub mod upgrade; // upgrades, loadouts, reachable loadouts per track
pub mod viz; // ASCII strip of a track, for tuning and snapshots

/// Version of the sim's outcomes. DESIGN §8.3.
///
/// Bumped on any change that alters a replay's outcome: generation,
/// calibration, the bots that prove a track, the car, the race, the run, or
/// what the log records. A log recorded under one epoch is only replayed by
/// a sim of the same epoch; under any other it keeps its seed and nothing
/// else.
///
/// Enforced rather than remembered: `tests/golden_runs.rs` hashes the pinned
/// track digests together with the golden-run corpus and asserts the result
/// is this epoch's entry in [`EPOCHS`]. Every bump gets a line in NOTES.md.
pub const SIM_EPOCH: u32 = 2;

/// Every epoch's digest, oldest first. Append-only: a sim change appends
/// `(SIM_EPOCH, digest)` after bumping `SIM_EPOCH`; an existing entry is never
/// edited, so a digest cannot be replaced without a bump. The tests check
/// that the last entry is `SIM_EPOCH`, that no digest repeats, and that each
/// is recorded in NOTES.md.
pub const EPOCHS: &[(u32, u64)] = &[(1, 0x7038_998b_dfbb_e7ab), (2, 0x47fb_6d4a_4a46_7568)];

/// The digest `SIM_EPOCH` vouches for: the last entry of [`EPOCHS`].
pub const EPOCH_DIGEST: u64 = EPOCHS[EPOCHS.len() - 1].1;

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
