//! Playtest statistics: how real drivers fared next to the human-like bot.
//! DESIGN §2.4, NOTES.md ("Playtest capture").
//!
//! The game keeps every finished or abandoned player run as a run file
//! (`replay::encode_run`); `just pull-runs` copies them off a phone. This
//! replays each one, which is exact under the same `SIM_EPOCH`, and prints per
//! track:
//!
//! - the player's time, closest checkpoint margin and spins, and whether the
//!   timer ran out;
//! - the human-like bot on the same track and loadout, 16 holdout trials (its
//!   own noise seeds, not the solver's): how many ran out of time, and its
//!   closest-margin p10 and p50.
//!
//! The summary compares the two by track index. The bot is calibrated to fail
//! a few percent of runs with comfortable margins; if players spin more, fail
//! more or keep much thinner margins, the bot is too good a driver and the
//! timers it sets are too tight (NOTES.md, M1 "tuning concerns").
//!
//! ```text
//! cargo run --release -p racer-sim --bin playstats -- target/runs/*.txt
//! ```
//!
//! A run the player quit is counted up to the track they abandoned, which is
//! shown but not counted: its replay runs on past the quit.
//!
//! A custom race (a version 2 run file, DESIGN §9.6) carries its layout's
//! code: it is replayed on that layout's track as it was raced (timed with
//! the timers its code carries, or free drive), the bot drives the same
//! track with the custom loadout, and its rows sum into a `custom` summary
//! row of their own. A free drive's margin is the race cap's, not a timer's.
//!
//! Exit status 2 on a bad argument or no readable run, 0 otherwise. A file
//! from another epoch is skipped with a note: its replay would not be exact.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;

use racer_sim::bot::drive;
use racer_sim::bot::human::{HumanBot, HumanPlan};
use racer_sim::layout::{Layout, CUSTOM_LOADOUT};
use racer_sim::race::{Phase, Race};
use racer_sim::replay::{decode_run, Ending};
use racer_sim::rng::substream;
use racer_sim::run::{RunState, TrackCache, Tracks};
use racer_sim::track::Track;
use racer_sim::upgrade::Loadout;
use racer_sim::{SIM_EPOCH, TICKS_PER_SEC};

/// Bot trials per track.
const TRIALS: u64 = 16;
/// Noise stream tag for the bot here: a holdout, not the solver's seeds.
const PLAYSTATS_TAG: u64 = 0x504c_4159;

/// One track as the player drove it.
#[derive(Default)]
struct Driven {
    branch: u8,
    loadout: Option<Loadout>,
    spins: u32,
    was_spinning: bool,
}

/// What the bot did on a track.
struct Bot {
    fails: u32,
    p10: u32,
    p50: u32,
}

/// Totals per track index across every run.
#[derive(Default)]
struct Totals {
    tracks: u32,
    player_fails: u32,
    player_spins: u32,
    player_margin: u64,
    bot_runs: u32,
    bot_fails: u32,
    bot_margin: u64,
}

fn secs(ticks: u32) -> f64 {
    ticks as f64 / TICKS_PER_SEC as f64
}

/// The human-like bot's holdout trials on `track` with `loadout`; `key`
/// picks its noise.
fn bot_on(track: &Arc<Track>, key: u64, loadout: Loadout) -> Bot {
    let params = loadout.params();
    let plan = HumanPlan::new(track, &params);
    let mut margins = Vec::new();
    let mut fails = 0;
    for trial in 0..TRIALS {
        let mut race = Race::new(track.clone(), params);
        let noise = substream(key, PLAYSTATS_TAG + trial);
        drive(&mut race, &mut HumanBot::new(plan.clone(), noise));
        if race.phase() == Phase::Finished {
            margins.push(race.min_margin());
        } else {
            fails += 1;
            margins.push(0);
        }
    }
    margins.sort_unstable();
    let at = |q: usize| margins[(margins.len() - 1) * q / 100];
    Bot {
        fails,
        p10: at(10),
        p50: at(50),
    }
}

/// Where a summary row's tracks came from: a generated track index, or a
/// custom race on a layout (a version 2 run file).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Index(u32),
    Custom,
}

/// A custom race's layout, from a version 2 run file: its track as it was
/// raced, and its name for the report.
fn custom_track(code: &str, seed: u64) -> Result<(Arc<Track>, String), String> {
    let layout = Layout::from_code(code).map_err(|e| format!("its layout does not read ({e})"))?;
    if layout.hash() != seed {
        return Err("its seed is not its layout's digest".into());
    }
    let track = layout
        .raced_track()
        .map_err(|p| format!("its layout cannot be built ({p:?})"))?;
    let name = if layout.name.is_empty() {
        "unnamed layout".to_string()
    } else {
        layout.name
    };
    Ok((Arc::new(track), name))
}

fn main() -> ExitCode {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() || files.iter().any(|f| f.starts_with('-')) {
        eprintln!("usage: playstats RUN_FILE...");
        return ExitCode::from(2);
    }
    let mut tracks = TrackCache::new();
    let mut totals: BTreeMap<Kind, Totals> = BTreeMap::new();
    let mut read = 0;
    for path in &files {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let f = match decode_run(&text) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{path}: not a run file ({e:?})");
                continue;
            }
        };
        if f.epoch != SIM_EPOCH {
            eprintln!(
                "{path}: played under epoch {}, this sim is epoch {SIM_EPOCH}: skipped",
                f.epoch
            );
            continue;
        }
        let seed = f.replay.run_seed;
        let custom = match f.layout.as_deref().map(|code| custom_track(code, seed)) {
            None => None,
            Some(Ok(c)) => Some(c),
            Some(Err(why)) => {
                eprintln!("{path}: {why}: skipped");
                continue;
            }
        };
        let mut driven: BTreeMap<u32, Driven> = BTreeMap::new();
        let observe = |r: &RunState| {
            let d = driven.entry(r.index()).or_default();
            d.branch = r.branch();
            d.loadout.get_or_insert(r.loadout());
            let spinning = r.race().car().spinning();
            if spinning && !d.was_spinning {
                d.spins += 1;
            }
            d.was_spinning = spinning;
        };
        let replayed = match &custom {
            None => f.replay.reconstruct_observed(&mut tracks, observe),
            Some((track, _)) => f
                .replay
                .reconstruct_single(track.clone(), CUSTOM_LOADOUT, observe),
        };
        let run = match replayed {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{path}: does not replay ({e:?})");
                continue;
            }
        };
        read += 1;
        let ending = match f.ending {
            Ending::Won => "won",
            Ending::Lost => "lost",
            Ending::Quit => "quit",
        };
        match &custom {
            None => println!("{path}  seed {seed:#018x}  {ending}"),
            Some((track, name)) => println!(
                "{path}  layout {name} {seed:#018x} ({})  {ending}",
                if track.allot_ticks().is_empty() {
                    "free drive"
                } else {
                    "timed"
                }
            ),
        }
        println!("  track  you: time  margin  spins  | bot x{TRIALS}: out  margin p10  p50");
        let last = run.results().len().saturating_sub(1);
        for (i, r) in run.results().iter().enumerate() {
            let Some(d) = driven.get(&r.index) else {
                continue;
            };
            // A replay drives an abandoned race on to its end with the last
            // input, so the quit track's time, margin and spins describe the
            // replay, not the player: show it, count nothing.
            if f.ending == Ending::Quit && i == last {
                println!(
                    "  {}/{}    QUIT (abandoned here; not counted)",
                    r.index + 1,
                    r.branch
                );
                continue;
            }
            let loadout = d.loadout.unwrap_or_default();
            let (bot, kind) = match &custom {
                None => {
                    let track = tracks.track(seed, r.index, d.branch);
                    let key = seed ^ ((r.index as u64) << 8 | d.branch as u64);
                    (bot_on(&track, key, loadout), Kind::Index(r.index))
                }
                Some((track, _)) => (bot_on(track, seed, loadout), Kind::Custom),
            };
            // A free drive has no timer to run out of: its margin is the
            // race cap's, so only its time and spins mean anything.
            let out = r.margin == 0;
            println!(
                "  {}/{}    {:6.1}s {:6.2}s {:5}{} | {:>8}/{TRIALS} {:9.2}s {:5.2}s",
                r.index + 1,
                r.branch,
                secs(r.ticks),
                secs(r.margin),
                d.spins,
                if out { "  OUT" } else { "     " },
                bot.fails,
                secs(bot.p10),
                secs(bot.p50),
            );
            let t = totals.entry(kind).or_default();
            t.tracks += 1;
            t.player_fails += out as u32;
            t.player_spins += d.spins;
            t.player_margin += r.margin as u64;
            t.bot_runs += TRIALS as u32;
            t.bot_fails += bot.fails;
            t.bot_margin += bot.p50 as u64;
        }
    }
    if read == 0 {
        eprintln!("no readable run files");
        return ExitCode::from(2);
    }
    println!();
    println!("summary by track ({read} runs)");
    println!("  track  driven  you: out  spins/track  mean margin | bot: out  mean p50 margin");
    for (kind, t) in &totals {
        let label = match kind {
            Kind::Index(i) => format!("{:5}", i + 1),
            Kind::Custom => "custom".to_string(),
        };
        println!(
            "  {label}  {:6}  {:5.0}%  {:11.2}  {:10.2}s | {:5.1}%  {:13.2}s",
            t.tracks,
            100.0 * t.player_fails as f64 / t.tracks as f64,
            t.player_spins as f64 / t.tracks as f64,
            secs((t.player_margin / t.tracks as u64) as u32),
            100.0 * t.bot_fails as f64 / t.bot_runs as f64,
            secs((t.bot_margin / t.tracks as u64) as u32),
        );
    }
    ExitCode::SUCCESS
}
