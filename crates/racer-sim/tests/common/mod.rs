//! Shared by the integration tests.

use std::sync::{Mutex, OnceLock};

use racer_sim::run::TrackCache;

/// One track cache per test binary: building a track runs the solver's
/// bots, and the tests in a binary ask for many of the same tracks.
pub fn tracks() -> &'static Mutex<TrackCache> {
    static CACHE: OnceLock<Mutex<TrackCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(TrackCache::new()))
}

/// The generated tracks whose digests are pinned: every track of run seed 1
/// (track 0 and both sides of every fork) and the last fork of seed 2.
pub fn pinned_tracks() -> Vec<(u64, u32, u8)> {
    let mut out = vec![(1, 0, 0)];
    for index in 1..5 {
        out.push((1, index, 0));
        out.push((1, index, 1));
    }
    out.push((2, 4, 0));
    out.push((2, 4, 1));
    out
}
