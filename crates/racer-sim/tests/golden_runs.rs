//! Golden runs and the sim epoch. DESIGN §8.3, §8.4.
//!
//! `golden_runs.txt` holds recorded runs, each with the epoch it was recorded
//! under and the `state_hash` its replay must reach. They are the regression
//! corpus for replays: a sim change that alters any of them breaks every
//! saved log, which is what `SIM_EPOCH` exists to say.
//!
//! Regenerate the corpus, only alongside a `SIM_EPOCH` bump, with:
//!
//! ```text
//! cargo test -p racer-sim --test golden_runs -- --ignored regenerate
//! ```
//!
//! which rewrites the file and prints the digest to append to `EPOCHS`.
//!
//! How the pieces fit:
//!
//! - The pinned track digests (`snapshots.rs`) cover generation, calibration
//!   and the solver alone.
//! - The golden runs cover everything a log touches after that: the car,
//!   collisions, the timer, forks and upgrades.
//! - `epoch_digest` hashes both into one number. `SIM_EPOCH` names that
//!   number and `EPOCHS` keeps every past one, so an outcome change cannot
//!   land without a visible, append-only version bump.
//!
//! The runs are not recorded by hand: `SPECS` feeds a run seed and a noise
//! seed to `run::autoplay`, the human-like bot, so the corpus can be
//! regenerated exactly. A corpus of hashes catches any change but cannot say
//! what changed; the example and property tests are where a failure gets
//! explained.

mod common;

use racer_sim::hash::Hasher;
use racer_sim::input::Input;
use racer_sim::replay::{decode_log, encode_log, Event, Replay};
use racer_sim::run::{autoplay, state_hash, RunPhase, RunState, Tracks};
use racer_sim::track::track_hash;
use racer_sim::{EPOCHS, EPOCH_DIGEST, SIM_EPOCH};

const CORPUS: &str = include_str!("golden_runs.txt");
const NOTES: &str = include_str!("../../../NOTES.md");

/// The recorded runs: `(run seed, noise seed)` for the autoplayer. Chosen to
/// win and to lose (on different tracks), and to take both sides of forks;
/// `the_corpus_covers_what_it_should` holds it to that.
const SPECS: [(u64, u64); 6] = [(1, 0), (1, 1), (2, 5), (3, 1), (4, 2), (7, 16)];

/// One parsed corpus line.
struct Golden {
    /// 1-based line number in `golden_runs.txt`, for failure messages.
    line: usize,
    epoch: u32,
    replay: Replay,
    hash: u64,
}

fn hex(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

/// Lines are `<epoch> <seed> <hash> <log…>`; `#` starts a comment.
fn parse_line(l: &str) -> Result<(u32, Replay, u64), String> {
    let mut parts = l.splitn(4, ' ');
    let mut field = |name: &str| parts.next().ok_or(format!("no {name}"));
    let epoch = field("epoch")?.parse().map_err(|_| "bad epoch")?;
    let run_seed = hex(field("seed")?).ok_or("bad seed")?;
    let hash = hex(field("hash")?).ok_or("bad hash")?;
    let log = decode_log(field("log")?).map_err(|e| format!("log: {e:?}"))?;
    Ok((epoch, Replay { run_seed, log }, hash))
}

fn corpus() -> Vec<Golden> {
    CORPUS
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|(i, l)| {
            let (epoch, replay, hash) =
                parse_line(l).unwrap_or_else(|e| panic!("golden_runs.txt:{}: {e}", i + 1));
            Golden {
                line: i + 1,
                epoch,
                replay,
                hash,
            }
        })
        .collect()
}

fn replayed(g: &Golden) -> RunState {
    let mut tracks = common::tracks().lock().unwrap();
    g.replay
        .reconstruct(&mut *tracks)
        .unwrap_or_else(|e| panic!("golden_runs.txt:{}: {e:?}", g.line))
}

/// Each recorded run must be from the current epoch and replay to its
/// recorded hash. The epoch check comes first so a stale corpus fails with
/// "regenerate", not with a confusing hash mismatch.
#[test]
fn golden_runs_reproduce() {
    let runs = corpus();
    assert_eq!(runs.len(), SPECS.len(), "corpus and SPECS disagree");
    for g in &runs {
        assert_eq!(
            g.epoch, SIM_EPOCH,
            "golden_runs.txt:{}: recorded under epoch {}, but SIM_EPOCH is \
             {SIM_EPOCH}. Regenerate the corpus (see the top of \
             tests/golden_runs.rs).",
            g.line, g.epoch
        );
        assert_eq!(
            state_hash(&replayed(g)),
            g.hash,
            "golden_runs.txt:{}: the run replays to a different state. If the \
             sim change is intended, bump SIM_EPOCH and regenerate (see the top \
             of tests/golden_runs.rs).",
            g.line
        );
    }
}

/// A regression corpus is only as good as what it exercises. After a
/// regeneration the runs could, by chance, stop losing or stop drifting;
/// this makes that a failure instead of a silent loss of coverage.
#[test]
fn the_corpus_covers_what_it_should() {
    let runs = corpus();
    let states: Vec<_> = runs.iter().map(replayed).collect();
    assert!(
        states.iter().any(|s| s.phase() == RunPhase::Won),
        "no golden run wins"
    );
    assert!(
        states.iter().any(|s| s.phase() == RunPhase::Lost),
        "no golden run loses"
    );
    let inputs = || runs.iter().flat_map(|g| g.replay.log.iter());
    let used = |f: fn(Input) -> bool| {
        inputs().any(|e| matches!(e, Event::Input { input, .. } if f(*input)))
    };
    assert!(used(Input::drift), "no golden run drifts");
    assert!(used(Input::brake), "no golden run brakes");
    for b in 0..2 {
        assert!(
            inputs().any(|e| *e == Event::Fork { branch: b }),
            "no golden run takes branch {b}"
        );
    }
}

/// Everything a replay's outcome depends on, in one number: the pinned
/// track digests and the golden runs' final state hashes.
fn epoch_digest(run_hashes: impl IntoIterator<Item = u64>) -> u64 {
    let mut h = Hasher::new();
    let mut tracks = common::tracks().lock().unwrap();
    for (seed, index, branch) in common::pinned_tracks() {
        h.write_u64(track_hash(&tracks.track(seed, index, branch)));
    }
    for hash in run_hashes {
        h.write_u64(hash);
    }
    h.finish()
}

/// `EPOCHS` is append-only history, so a digest cannot be quietly replaced
/// under the same epoch: the current epoch is the last entry, every epoch
/// has one digest, no digest serves two epochs, and each is written down in
/// NOTES.md with the reason for the bump.
#[test]
fn the_epoch_history_is_consistent() {
    let (last, digest) = *EPOCHS.last().expect("EPOCHS is empty");
    assert_eq!(
        last, SIM_EPOCH,
        "the last EPOCHS entry is epoch {last}, but SIM_EPOCH is {SIM_EPOCH}"
    );
    assert_eq!(digest, EPOCH_DIGEST);
    for w in EPOCHS.windows(2) {
        assert!(w[0].0 < w[1].0, "EPOCHS out of order at epoch {}", w[1].0);
    }
    for (i, &(e, d)) in EPOCHS.iter().enumerate() {
        assert!(
            EPOCHS[..i].iter().all(|&(_, other)| other != d),
            "epoch {e} reuses an earlier epoch's digest"
        );
        assert!(
            NOTES.contains(&format!("{d:#018x}")),
            "NOTES.md does not record epoch {e}'s digest {d:#018x}"
        );
    }
}

#[test]
fn sim_epoch_matches_its_digest() {
    // The reconstructed states, not the recorded hashes: the digest must
    // move when the sim does, even if the corpus was regenerated.
    let hashes: Vec<u64> = corpus().iter().map(|g| state_hash(&replayed(g))).collect();
    let digest = epoch_digest(hashes);
    assert!(
        digest == EPOCH_DIGEST,
        "the sim's outcomes changed under SIM_EPOCH {SIM_EPOCH}: the digest over \
         the pinned tracks and golden runs is {digest:#018x}, not \
         {EPOCH_DIGEST:#018x}.\n\
         Saved runs from this epoch would replay differently. If the change is \
         intended: bump SIM_EPOCH, regenerate golden_runs.txt (see the top of \
         tests/golden_runs.rs), append (SIM_EPOCH, new digest) to EPOCHS in \
         src/lib.rs -- never edit an existing entry -- and note the bump, its \
         reason and the digest in NOTES.md. If it is not intended, it is a \
         determinism bug."
    );
}

/// Rewrites `golden_runs.txt` from `SPECS` and prints the new digest.
#[test]
#[ignore = "rewrites tests/golden_runs.txt; run by hand alongside a SIM_EPOCH bump"]
fn regenerate() {
    let mut out = String::from(HEADER);
    let mut hashes = Vec::new();
    for &(seed, noise) in &SPECS {
        let played = {
            let mut tracks = common::tracks().lock().unwrap();
            autoplay(seed, noise, &mut *tracks)
        };
        let hash = state_hash(&played);
        let replay = Replay::of(&played);
        let mut tracks = common::tracks().lock().unwrap();
        assert_eq!(replay.final_hash(&mut *tracks), Ok(hash), "seed {seed:#x}");
        drop(tracks);
        hashes.push(hash);
        out.push_str(&format!(
            "{SIM_EPOCH} {seed:#x} {hash:#018x} {}\n",
            encode_log(&replay.log)
        ));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden_runs.txt");
    std::fs::write(path, out).expect("writing golden_runs.txt");
    println!("wrote {path}");
    println!(
        "digest for epoch {SIM_EPOCH}: {:#018x}",
        epoch_digest(hashes)
    );
}

const HEADER: &str = "\
# Golden runs: recorded runs and the state_hash each must replay to.
# Checked by tests/golden_runs.rs, and folded into the epoch digest (see
# SIM_EPOCH and EPOCHS in src/lib.rs). Generated; do not edit by hand.
# Regenerate, only alongside a SIM_EPOCH bump, with
#
#   cargo test -p racer-sim --test golden_runs -- --ignored regenerate
#
# One run per line: <epoch> <seed> <final state_hash> <log>
#
# The log is replay::encode_log's text form:
#   v1     the format version, always first
#   D:S    an input change D ticks after the previous one (or the race's
#          start): steering L3..L1, C, R1..R3, then B if braking, D if
#          drifting
#   >B     the fork taken after a race: branch B (0 or 1)
";
