//! Look at a track. DESIGN §2.
//!
//! ```text
//! cargo run --release -p racer-sim --bin trackviz -- --fixture blind_crest --ascii
//! ```
//!
//! `--ascii` prints the strip chart from `racer_sim::viz`. Exit status 2 on a
//! bad flag.

use racer_sim::track::{fixtures, Track};
use racer_sim::viz::ascii_strip;

fn fixture(name: &str) -> Option<Track> {
    Some(match name {
        "drag_strip" => fixtures::drag_strip(),
        "sweep_and_hairpin" => fixtures::sweep_and_hairpin(),
        "blind_crest" => fixtures::blind_crest(),
        _ => return None,
    })
}

fn usage(msg: &str) -> ! {
    eprintln!("{msg}");
    eprintln!("usage: trackviz --fixture NAME [--ascii]");
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut track = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--fixture" => {
                i += 1;
                let name = argv.get(i).map(String::as_str).unwrap_or("");
                track = Some(fixture(name).unwrap_or_else(|| usage("unknown fixture")));
            }
            "--ascii" => {}
            other => usage(&format!("unknown flag {other}")),
        }
        i += 1;
    }
    let track = track.unwrap_or_else(|| usage("no track chosen"));
    print!("{}", ascii_strip(&track));
}
