//! Shared by the integration tests.

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
