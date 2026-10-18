//! The `playstats` bin reads the game's run files and reports per track.

use std::process::Command;

use racer_sim::replay::{encode_run, Ending, Replay, RunFile};
use racer_sim::run::{autoplay, RunPhase, TrackCache};

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

#[test]
fn playstats_refuses_no_input() {
    let out = Command::new(env!("CARGO_BIN_EXE_playstats"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
