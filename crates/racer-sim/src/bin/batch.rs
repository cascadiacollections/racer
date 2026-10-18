//! Batch solvability harness and gate. DESIGN §2.4.
//!
//! Builds every track of many runs (track 0 and both sides of every fork:
//! nine per run), each through the solver's full ladder, then plays each
//! run [`HOLDOUT_RUNS`] times with the human bot on noise seeds the solver
//! never used, taking forks at random. Prints the report and checks the gates:
//!
//! - no unvalidated tracks;
//! - at most 2% of tracks from the fallback rung;
//! - a human run failure rate between 3% and 15%.
//!
//! The per-track bands (perfect bot's time left, skill gap, drift corners,
//! margins) are enforced by the solver on every accepted draw, so here they
//! are reported, not re-gated.
//!
//! ```text
//! cargo run --release -p racer-sim --bin batch -- --seeds 200
//! cargo run --release -p racer-sim --bin batch -- --seeds 50 --k 8 --threads 4
//! ```
//!
//! Flags: `--seeds N` (default 200), `--k K` human trials per calibration and
//! per loadout in verification (default 16; with fewer, the 15-of-16 margin
//! rule scales to allow the same one miss), `--threads T` (default: every
//! core) and `-v` (one line per track). The report is deterministic: work
//! is split across threads but results are gathered in seed order. Only the
//! wall-clock timings vary. Exit status 1 if a gate fails, 2 on a bad flag.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use racer_sim::bot::drive;
use racer_sim::bot::human::{HumanBot, HumanPlan};
use racer_sim::gen::difficulty::{PERFECT_LEFT, SKILL_GAP, TRACKS};
use racer_sim::race::{Phase, Race};
use racer_sim::rng::{mix64, substream, Pcg32};
use racer_sim::solver::{build, Built, Effort, Failure, Rung, DRIFT_LOSS};
use racer_sim::upgrade::{upgrade_of, Loadout};
use racer_sim::DT;

/// Gate: fallback tracks, at most this share.
const MAX_FALLBACK: f64 = 0.02;
/// Gate: human run failure rate band.
const RUN_FAILURE: (f64, f64) = (0.03, 0.15);

/// Holdout runs played per run seed. Runs are cheap next to building the
/// tracks, and more of them steady the failure rate the gate reads.
const HOLDOUT_RUNS: u64 = 5;

/// Tag for the holdout drives: "HOLD".
const HOLDOUT: u64 = 0x484F_4C44;
/// Tag for the route picks: "PICK".
const PICKS: u64 = 0x5049_434B;

/// The run seed for batch seed `n`: the first N of a fixed schedule.
fn run_seed(n: u64) -> u64 {
    mix64(0xBA7C_0000_0000_0000 ^ n)
}

/// Everything learnt from one run seed.
struct SeedResult {
    /// `tracks[index][branch]`; track 0 has only branch 0.
    tracks: Vec<Vec<(Built, f64)>>,
    /// For each holdout run, the index of the track it lost on, if it did.
    lost_on: Vec<Option<u32>>,
}

fn one_seed(n: u64, effort: Effort) -> SeedResult {
    let seed = run_seed(n);
    let tracks: Vec<Vec<(Built, f64)>> = (0..TRACKS)
        .map(|index| {
            let branches = if index == 0 { 1 } else { 2 };
            (0..branches)
                .map(|branch| {
                    let t0 = Instant::now();
                    let b = build(seed, index, branch, effort);
                    (b, t0.elapsed().as_secs_f64())
                })
                .collect()
        })
        .collect();

    // The holdout runs: random forks, fresh noise.
    let lost_on = (0..HOLDOUT_RUNS)
        .map(|run| {
            let run_tag = substream(seed, HOLDOUT ^ (run << 32));
            let mut picks = Pcg32::new(substream(run_tag, PICKS));
            let mut loadout = Loadout::BASE;
            for index in 0..TRACKS {
                let branch = if index == 0 {
                    0
                } else {
                    picks.range_u32(0, 1) as u8
                };
                if let Some(u) = upgrade_of(seed, index, branch) {
                    loadout = loadout.with(u);
                }
                let track = &tracks[index as usize][branch as usize].0.track;
                let p = loadout.params();
                let mut race = Race::new(track.clone(), p);
                let noise = substream(run_tag, index as u64);
                drive(
                    &mut race,
                    &mut HumanBot::new(HumanPlan::new(track, &p), noise),
                );
                if race.phase() != Phase::Finished {
                    return Some(index);
                }
            }
            None
        })
        .collect();
    SeedResult { tracks, lost_on }
}

fn usage(msg: &str) -> ! {
    eprintln!("{msg}");
    eprintln!("usage: batch [--seeds N] [--k K] [--threads T] [-v]");
    std::process::exit(2);
}

/// min, mean, max of a list.
fn spread(xs: &[f64]) -> (f64, f64, f64) {
    let min = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (min, xs.iter().sum::<f64>() / xs.len().max(1) as f64, max)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut seeds = 200u64;
    let mut k = 16u64;
    let mut threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut verbose = false;
    let mut args = argv.iter();
    while let Some(flag) = args.next() {
        let mut num = || -> u64 {
            args.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| usage(&format!("{flag} wants a number")))
        };
        match flag.as_str() {
            "--seeds" => seeds = num(),
            "--k" => k = num().max(2),
            "--threads" => threads = num().max(1) as usize,
            "-v" | "--verbose" => verbose = true,
            other => usage(&format!("unknown flag {other}")),
        }
    }
    let effort = Effort {
        calibration: k,
        verify: k,
        passes: k - 1,
    };

    println!("racer batch: {seeds} runs x 9 tracks, K = {k} trials, {threads} threads");
    let t0 = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<SeedResult>>> = Mutex::new((0..seeds).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let n = next.fetch_add(1, Ordering::Relaxed) as u64;
                if n >= seeds {
                    break;
                }
                let r = one_seed(n, effort);
                results.lock().unwrap()[n as usize] = Some(r);
            });
        }
    });
    let wall = t0.elapsed().as_secs_f64();
    let results: Vec<SeedResult> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();

    // ---------------------------------------------------------------- tally
    let mut total = 0usize;
    let (mut first, mut later, mut repaired, mut fallback, mut unvalidated) = (0, 0, 0, 0, 0);
    let mut crash_prone = 0;
    let (mut undrivable, mut perfect_failed, mut band, mut gap_fail, mut drift_fail, mut margins) =
        (0, 0, 0, 0, 0, 0);
    let mut per_index: Vec<Vec<&Built>> = vec![Vec::new(); TRACKS as usize];
    let mut cost_ticks = Vec::new();
    let mut secs = Vec::new();
    for (n, r) in results.iter().enumerate() {
        for (index, side) in r.tracks.iter().enumerate() {
            for (branch, (b, t)) in side.iter().enumerate() {
                total += 1;
                match b.rung {
                    Rung::Drawn(0) => first += 1,
                    Rung::Drawn(_) => later += 1,
                    Rung::Repaired => repaired += 1,
                    Rung::Fallback => fallback += 1,
                    Rung::Unvalidated => unvalidated += 1,
                }
                for f in &b.failures {
                    match f {
                        Failure::Undrivable => undrivable += 1,
                        Failure::CrashProne => crash_prone += 1,
                        Failure::PerfectFailed => perfect_failed += 1,
                        Failure::PerfectBand(_) => band += 1,
                        Failure::SkillGap(_) => gap_fail += 1,
                        Failure::DriftCorner => drift_fail += 1,
                        Failure::Margins { .. } => margins += 1,
                    }
                }
                if !matches!(b.rung, Rung::Unvalidated) {
                    per_index[index].push(b);
                }
                cost_ticks.push(b.report.cost as f64);
                secs.push(*t);
                if verbose {
                    let rp = &b.report;
                    println!(
                        "  seed {n:>3} track {index}.{branch} {:<10} left {:>5.1}s gap {:>5.1}% worst margin {:>5.2}s passes {:>2} on {} ({} loadouts) {:.0} ms",
                        format!("{:?}", b.rung),
                        rp.perfect_left as f64 * DT,
                        rp.skill_gap * 100.0,
                        rp.worst_margin as f64 * DT,
                        rp.fewest_passes,
                        rp.calibrated_on.describe(),
                        rp.loadouts,
                        t * 1000.0
                    );
                }
            }
        }
    }
    let runs = seeds * HOLDOUT_RUNS;
    let mut lost = 0;
    let mut lost_by_index = vec![0; TRACKS as usize];
    for i in results.iter().flat_map(|r| r.lost_on.iter().flatten()) {
        lost += 1;
        lost_by_index[*i as usize] += 1;
    }

    println!("\ntracks: {total} built");
    println!(
        "  rungs: first draw {first}, later draw {later}, repaired {repaired}, fallback {fallback}, unvalidated {unvalidated}"
    );
    println!(
        "  rejected candidates: undrivable {undrivable}, crash-prone {crash_prone}, perfect failed {perfect_failed}, perfect band {band}, skill gap {gap_fail}, drift corner {drift_fail}, margins {margins}"
    );
    println!("\nper track index (accepted tracks):");
    println!(
        "  idx  n    perfect left s (min/mean/max)   skill gap % (min/mean/max)   worst human margin s   human s  perfect s"
    );
    for (i, bs) in per_index.iter().enumerate() {
        if bs.is_empty() {
            continue;
        }
        let left: Vec<f64> = bs
            .iter()
            .map(|b| b.report.perfect_left as f64 * DT)
            .collect();
        let gap: Vec<f64> = bs.iter().map(|b| b.report.skill_gap * 100.0).collect();
        let worst: Vec<f64> = bs
            .iter()
            .map(|b| b.report.worst_margin as f64 * DT)
            .collect();
        let human: Vec<f64> = bs
            .iter()
            .map(|b| b.report.human_ticks as f64 * DT)
            .collect();
        let perfect: Vec<f64> = bs
            .iter()
            .map(|b| b.report.perfect_ticks as f64 * DT)
            .collect();
        let (l, g, w) = (spread(&left), spread(&gap), spread(&worst));
        println!(
            "  {i}  {:>4}   {:>5.1} {:>5.1} {:>5.1}            {:>5.1} {:>5.1} {:>5.1}              min {:>5.2}           {:>5.1}    {:>5.1}",
            bs.len(),
            l.0,
            l.1,
            l.2,
            g.0,
            g.1,
            g.2,
            w.0,
            spread(&human).1,
            spread(&perfect).1
        );
    }

    let drift: Vec<_> = per_index
        .iter()
        .flatten()
        .flat_map(|b| b.report.drift.iter())
        .collect();
    let tightened = drift.iter().filter(|d| d.repairs > 0).count();
    let grip_failed = drift.iter().filter(|d| d.grip_failed).count();
    let losses: Vec<f64> = drift
        .iter()
        .filter(|d| !d.grip_failed)
        .map(|d| d.loss)
        .collect();
    let (lmin, lmean, lmax) = spread(&losses);
    println!(
        "\ndrift-required corners on accepted tracks: {} verified ({} first time, {} after tightening), grip-only bot ran out of time on {}, lost {lmin:.2}/{lmean:.2}/{lmax:.2} s (min/mean/max, need >= {DRIFT_LOSS}) on the rest; {drift_fail} candidates rejected for a corner that stayed undemanding",
        drift.len(),
        drift.len() - tightened,
        tightened,
        grip_failed
    );

    let (cmin, cmean, cmax) = spread(&cost_ticks);
    let (_, smean, smax) = spread(&secs);
    println!(
        "\ncost per track: {:.2}M ticks mean ({:.2}M min, {:.2}M max), {:.0} ms mean, {:.0} ms max on one thread; {wall:.1} s wall for the batch",
        cmean / 1e6,
        cmin / 1e6,
        cmax / 1e6,
        smean * 1000.0,
        smax * 1000.0
    );

    // ---------------------------------------------------------------- gates
    let fallback_share = fallback as f64 / total as f64;
    let failure = lost as f64 / runs as f64;
    println!("\ngates:");
    let mut ok = true;
    let mut gate = |name: &str, pass: bool, detail: String| {
        println!("  {} {name}: {detail}", if pass { "PASS" } else { "FAIL" });
        ok &= pass;
    };
    gate(
        "unvalidated tracks",
        unvalidated == 0,
        format!("{unvalidated} (must be 0)"),
    );
    gate(
        "fallback share",
        fallback_share <= MAX_FALLBACK,
        format!(
            "{:.2}% (at most {:.0}%)",
            fallback_share * 100.0,
            MAX_FALLBACK * 100.0
        ),
    );
    gate(
        "human run failure",
        (RUN_FAILURE.0..=RUN_FAILURE.1).contains(&failure),
        format!(
            "{lost}/{runs} runs = {:.1}% (band {:.0}..{:.0}%), lost on track index {lost_by_index:?}",
            failure * 100.0,
            RUN_FAILURE.0 * 100.0,
            RUN_FAILURE.1 * 100.0
        ),
    );
    println!(
        "  (per track, enforced by the solver: perfect left {}..{} s, skill gap {:.0}..{:.0}%, margins kept in {}/{k})",
        PERFECT_LEFT.0 as f64 * DT,
        PERFECT_LEFT.1 as f64 * DT,
        SKILL_GAP.0 * 100.0,
        SKILL_GAP.1 * 100.0,
        k - 1
    );
    if !ok {
        std::process::exit(1);
    }
}
