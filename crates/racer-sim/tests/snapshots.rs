//! Snapshot tests (insta). Each pins output that should only ever change on
//! purpose; `cargo insta review` shows the diff and accepts it.
//!
//! A snapshot asserts nothing about what is correct, only that the output is
//! what it was. That is the right tool for tracks: there is no oracle for
//! what a track should look like, but there is a promise that it stays the
//! same (DESIGN §2.2). Property tests check the rules; this checks the
//! promise.

mod common;

use racer_sim::solver::{build, Effort};
use racer_sim::track::{fixtures, track_hash};
use racer_sim::viz::ascii_strip;

#[test]
fn fixture_strips_are_pinned() {
    insta::assert_snapshot!(
        "sweep_and_hairpin",
        ascii_strip(&fixtures::sweep_and_hairpin())
    );
    insta::assert_snapshot!("blind_crest", ascii_strip(&fixtures::blind_crest()));
}

#[test]
fn fixture_digests_are_pinned() {
    let out: String = [
        ("drag_strip", fixtures::drag_strip()),
        ("sweep_and_hairpin", fixtures::sweep_and_hairpin()),
        ("blind_crest", fixtures::blind_crest()),
        ("hazards", fixtures::hazards()),
    ]
    .iter()
    .map(|(name, t)| format!("{name:<18} {:016x}\n", track_hash(t)))
    .collect();
    insta::assert_snapshot!(out);
}

/// Digests of generated, calibrated tracks: every track of run seed 1 and
/// the last fork of seed 2. These are what seed sharing promises — the same
/// seed is the same track, timer included, on every platform and build —
/// and CI runs this on Linux, macOS and Windows. They are folded into the
/// epoch digest too (`golden_runs.rs`), so a change here needs a
/// `SIM_EPOCH` bump (DESIGN §8.3).
#[test]
fn generated_track_digests_are_pinned() {
    let mut out = String::new();
    for (seed, index, branch) in common::pinned_tracks() {
        let b = build(seed, index, branch, Effort::default());
        out.push_str(&format!(
            "seed {seed} track {index}.{branch} {:<10} {:016x}\n",
            format!("{:?}", b.rung),
            track_hash(&b.track)
        ));
    }
    insta::assert_snapshot!(out);
}
