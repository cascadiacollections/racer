//! Snapshot tests (insta). Each pins output that should only ever change on
//! purpose; `cargo insta review` shows the diff and accepts it.
//!
//! A snapshot asserts nothing about what is correct, only that the output is
//! what it was. That is the right tool for tracks: there is no oracle for
//! what a track should look like, but there is a promise that it stays the
//! same (DESIGN §2.2). Property tests check the rules; this checks the
//! promise.

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
    ]
    .iter()
    .map(|(name, t)| format!("{name:<18} {:016x}\n", track_hash(t)))
    .collect();
    insta::assert_snapshot!(out);
}
