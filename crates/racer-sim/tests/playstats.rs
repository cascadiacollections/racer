//! The `playstats` bin reads the game's run files and reports per track.

use std::process::Command;
use std::sync::Arc;

use racer_sim::replay::{encode_run, Ending, Replay, RunFile};
use racer_sim::run::{autoplay, RunPhase, RunState, TrackCache};

#[test]
fn playstats_reports_every_driven_track_of_a_finished_run() {
    let mut tracks = TrackCache::new();
    let run = autoplay(11, 3, &mut tracks);
    let ending = match run.phase() {
        RunPhase::Won => Ending::Won,
        _ => Ending::Lost,
    };
    let f = RunFile {
        epoch: racer_sim::SIM_EPOCH,
        ending,
        replay: Replay::of(&run),
        layout: None,
    };
    let dir = std::env::temp_dir().join(format!("racer-playstats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run-test.txt");
    std::fs::write(&path, encode_run(&f)).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_playstats"))
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // One row per driven track, then a summary row per track index.
    let rows = text.lines().filter(|l| l.contains("/16 ")).count();
    assert_eq!(rows, run.results().len(), "{text}");
    assert!(text.contains("summary by track (1 runs)"), "{text}");
    assert!(!text.contains("QUIT"), "{text}");
}

/// A custom race on the first bundled course, timed or free, driven by the
/// human-like bot, as a version 2 run file.
fn custom_run(timed: bool) -> (String, RunState) {
    use racer_sim::bot::human::{HumanBot, HumanPlan};
    use racer_sim::bot::Driver;
    use racer_sim::layout::{bundled, CUSTOM_LOADOUT};
    let course = bundled::layouts().remove(0);
    let raced = if timed { course } else { course.unproven() };
    let track = Arc::new(raced.raced_track().unwrap());
    let mut run = RunState::single(raced.hash(), track.clone(), CUSTOM_LOADOUT);
    let plan = HumanPlan::new(&track, &CUSTOM_LOADOUT.params());
    let mut bot = HumanBot::new(plan, 5);
    while run.phase() == RunPhase::Racing {
        let input = bot.input(run.race());
        run.step(input);
    }
    let f = RunFile {
        epoch: racer_sim::SIM_EPOCH,
        ending: if run.phase() == RunPhase::Won {
            Ending::Won
        } else {
            Ending::Lost
        },
        replay: Replay::of(&run),
        layout: Some(raced.to_code().unwrap()),
    };
    (encode_run(&f), run)
}

#[test]
fn playstats_replays_custom_races_from_their_layout_and_still_reads_v1() {
    let dir = std::env::temp_dir().join(format!("racer-playstats-c-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut paths = Vec::new();
    for timed in [true, false] {
        let (text, _) = custom_run(timed);
        assert!(text.starts_with("racer-run 2\n"));
        let path = dir.join(format!("run-layout-{timed}.txt"));
        std::fs::write(&path, text).unwrap();
        paths.push(path);
    }
    // A version 1 file, as every build before layouts wrote.
    let mut tracks = TrackCache::new();
    let run = autoplay(4, 1, &mut tracks);
    let v1 = encode_run(&RunFile {
        epoch: racer_sim::SIM_EPOCH,
        ending: Ending::Lost,
        replay: Replay::of(&run),
        layout: None,
    });
    assert!(v1.starts_with("racer-run 1\n"));
    let path = dir.join("run-v1.txt");
    std::fs::write(&path, v1).unwrap();
    paths.push(path);
    // And a damaged layout line, skipped with a note.
    let (text, _) = custom_run(true);
    let bad = text.replacen("layout RL", "layout RLZZ", 1);
    let path = dir.join("run-bad.txt");
    std::fs::write(&path, bad).unwrap();
    paths.push(path);

    let out = Command::new(env!("CARGO_BIN_EXE_playstats"))
        .args(&paths)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}\n{err}");
    assert!(text.contains("layout FIRST LIGHT"), "{text}");
    assert!(
        text.contains("(timed)") && text.contains("(free drive)"),
        "{text}"
    );
    assert!(text.contains("summary by track (3 runs)"), "{text}");
    let custom = text
        .lines()
        .find(|l| l.trim_start().starts_with("custom "))
        .expect("a custom row");
    assert!(custom.contains("      2  "), "two custom races: {custom}");
    assert!(
        err.contains("run-bad.txt: its layout does not read"),
        "{err}"
    );
}

#[test]
fn playstats_refuses_no_input() {
    let out = Command::new(env!("CARGO_BIN_EXE_playstats"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
