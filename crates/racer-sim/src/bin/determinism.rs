//! Cross-platform determinism check. DESIGN §6.
//!
//! Plays 200 runs with the human-like bot (`run::autoplay`): 10 run seeds,
//! 20 noise seeds each. Replays each from `(run_seed, log)` alone, asserts
//! the reconstruction hashes match, and prints one digest over all 200 final
//! hashes and the tracks they were played on.
//!
//! CI runs this on Linux, macOS and Windows and compares the digest; M2 adds
//! Android and iOS, as in Salt Line. If the digests differ there is a
//! determinism leak — a libm call, a `HashMap` iteration, a variable
//! timestep — and it must be fixed rather than papered over with a
//! tolerance.
//!
//! ```text
//! cargo run --release -p racer-sim --bin determinism
//! cargo run --release -p racer-sim --bin determinism -- --expect 0x...
//! cargo run --release -p racer-sim --bin determinism -- --verbose
//! ```
//!
//! Two checks in one: live play against its own replay catches state the
//! log fails to capture, and the digest compared across machines catches
//! floating-point or ordering differences between platforms. Building a
//! track runs the solver, so the tracks are built first, on every core, into
//! a cache both the play and the replays draw from; building is a pure
//! function of the key, so threads cannot change the result, and the digest
//! includes every track's own digest. Exit status is 1 on any divergence or
//! digest mismatch, 2 on a bad flag.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use racer_sim::gen::difficulty::TRACKS;
use racer_sim::hash::Hasher;
use racer_sim::replay::Replay;
use racer_sim::rng::mix64;
use racer_sim::run::{autoplay, state_hash, RunPhase, TrackCache, Tracks};
use racer_sim::solver::{build, Effort, Rung};
use racer_sim::track::track_hash;

/// Run seeds.
const SEEDS: u64 = 10;
/// Noise seeds (players) per run seed.
const PLAYERS: u64 = 20;

/// Fixed, well-spread run seeds.
fn run_seed(n: u64) -> u64 {
    mix64(0xD373_0000_0000_0000 ^ n)
}

/// Build every track the runs can reach, on every core.
fn build_all() -> TrackCache {
    let keys: Vec<(u64, u32, u8)> = (0..SEEDS)
        .flat_map(|n| {
            (0..TRACKS).flat_map(move |i| {
                (0..if i == 0 { 1 } else { 2 }).map(move |b| (run_seed(n), i, b))
            })
        })
        .collect();
    let next = AtomicUsize::new(0);
    let cache = Mutex::new(TrackCache::new());
    let refused = Mutex::new(None);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(seed, index, branch)) = keys.get(k) else {
                    break;
                };
                let built = build(seed, index, branch, Effort::default());
                if built.rung == Rung::Unvalidated {
                    *refused.lock().unwrap() = Some((seed, index, branch));
                    break;
                }
                cache
                    .lock()
                    .unwrap()
                    .insert(seed, index, branch, built.track);
            });
        }
    });
    if let Some((seed, index, branch)) = refused.into_inner().unwrap() {
        eprintln!("refusing unvalidated track seed {seed} track {index}.{branch}");
        std::process::exit(1);
    }
    cache.into_inner().unwrap()
}

fn main() {
    let mut expect: Option<u64> = None;
    let mut verbose = false;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut args = argv.iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--expect" => {
                let v = args.next().cloned().unwrap_or_default();
                expect = u64::from_str_radix(v.trim_start_matches("0x"), 16).ok();
                if expect.is_none() {
                    eprintln!("--expect wants a hex digest, got '{v}'");
                    std::process::exit(2);
                }
            }
            "--verbose" | "-v" => verbose = true,
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
    }

    let runs = SEEDS * PLAYERS;
    println!("racer determinism: {runs} runs ({SEEDS} run seeds x {PLAYERS} players)");
    println!("target: {}", std::env::consts::ARCH);
    println!("os:     {}", std::env::consts::OS);

    let mut tracks = build_all();
    let mut digest = Hasher::new();
    for n in 0..SEEDS {
        for i in 0..TRACKS {
            for b in 0..if i == 0 { 1 } else { 2 } {
                let track = tracks.track(run_seed(n), i, b).unwrap_or_else(|e| {
                    eprintln!(
                        "refusing unproven track seed {} track {i}.{b}: {e:?}",
                        run_seed(n)
                    );
                    std::process::exit(1);
                });
                digest.write_u64(track_hash(&track));
            }
        }
    }

    let (mut diverged, mut won, mut events) = (0u64, 0u64, 0usize);
    for n in 0..SEEDS {
        for player in 0..PLAYERS {
            let seed = run_seed(n);
            let played = autoplay(seed, player, &mut tracks).unwrap_or_else(|e| {
                eprintln!("run {n}/{player}: refusing an unproven track ({e:?})");
                std::process::exit(1);
            });
            let played_hash = state_hash(&played);
            let replay = Replay::of(&played);
            match replay.final_hash(&mut tracks) {
                Ok(h) if h == played_hash => {}
                Ok(h) => {
                    diverged += 1;
                    eprintln!("run {n}/{player}: replay {h:#018x} != live {played_hash:#018x}");
                }
                Err(e) => {
                    diverged += 1;
                    eprintln!("run {n}/{player}: replay failed: {e:?}");
                }
            }
            won += u64::from(played.phase() == RunPhase::Won);
            events += replay.log.len();
            digest.write_u64(played_hash);
            if verbose {
                println!(
                    "  run {n:>2}/{player:>2} seed {seed:#018x} {:?} on track {} hash {played_hash:#018x}",
                    played.phase(),
                    played.index()
                );
            }
        }
    }

    let digest = digest.finish();
    println!("\nlogged events: {events}, runs won: {won}/{runs}");
    println!("replay divergences: {diverged}");
    println!("digest: {digest:#018x}");

    if diverged > 0 {
        std::process::exit(1);
    }
    if let Some(want) = expect {
        if want != digest {
            eprintln!("digest mismatch: expected {want:#018x}, got {digest:#018x}");
            std::process::exit(1);
        }
        println!("digest matches the expected value");
    }
}
