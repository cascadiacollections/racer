//! Look at a track. DESIGN §2.
//!
//! ```text
//! cargo run --release -p racer-sim --bin trackviz -- --fixture blind_crest --ascii
//! cargo run --release -p racer-sim --bin trackviz -- --fixture hazards --png out.png
//! ```
//!
//! `--ascii` prints the strip chart from `racer_sim::viz`. `--png PATH`
//! drives the track with the perfect bot and `--trials N` human-bot trials
//! (default 8) and draws three panels:
//!
//! 1. the track from above (the heading integrated from each segment's
//!    turn), coloured by surface, with drift-required corners in red, crests
//!    as yellow dots, checkpoints as white dots and objects as black ones,
//!    and the first human trial's path coloured by speed;
//! 2. speed against distance: the perfect bot in white, the human trials in
//!    orange, over the elevation profile in grey;
//! 3. one bar group per section: each human trial's time lost to the
//!    perfect bot there, in seconds (the grid lines are 1 s apart).
//!
//! It also prints the times. Exit status 2 on a bad flag.

mod canvas;
mod png;

use std::sync::Arc;

use canvas::{ramp, Canvas, Rgb};
use racer_sim::bot::human::{HumanBot, HumanPlan};
use racer_sim::bot::perfect::perfect_run;
use racer_sim::bot::{drive_traced, Trace};
use racer_sim::car::CarParams;
use racer_sim::dmath;
use racer_sim::race::Race;
use racer_sim::track::{fixtures, flags, Surface, Track, SEG_LEN};
use racer_sim::viz::ascii_strip;
use racer_sim::DT;

fn fixture(name: &str) -> Option<Track> {
    Some(match name {
        "drag_strip" => fixtures::drag_strip(),
        "sweep_and_hairpin" => fixtures::sweep_and_hairpin(),
        "blind_crest" => fixtures::blind_crest(),
        "hazards" => fixtures::hazards(),
        _ => return None,
    })
}

fn usage(msg: &str) -> ! {
    eprintln!("{msg}");
    eprintln!("usage: trackviz --fixture NAME [--ascii] [--png PATH] [--trials N]");
    std::process::exit(2);
}

const W: i32 = 1000;
const MAP_H: i32 = 640;
const SPEED_H: i32 = 200;
const BARS_H: i32 = 160;
const BG: Rgb = [24, 24, 32];
const PANEL: Rgb = [34, 34, 46];
const WHITE: Rgb = [240, 240, 240];
const ORANGE: Rgb = [250, 150, 40];

fn surface_colour(s: Surface) -> Rgb {
    match s {
        Surface::Asphalt => [110, 110, 124],
        Surface::Gravel => [176, 136, 80],
        Surface::Ice => [168, 216, 240],
        Surface::Puddle => [72, 120, 192],
    }
}

/// The centre line from above: one point per segment boundary, and the
/// heading at each.
fn layout(t: &Track) -> Vec<(f64, f64, f64)> {
    let mut out = vec![(0.0, 0.0, 0.0)];
    let (mut x, mut y, mut th) = (0.0, 0.0, 0.0);
    for s in t.segments() {
        // Positive turns right: clockwise, with y up.
        th -= s.curve_milli as f64 / 1000.0;
        x += SEG_LEN * dmath::cos(th);
        y += SEG_LEN * dmath::sin(th);
        out.push((x, y, th));
    }
    out
}

struct Drives {
    perfect: Race,
    perfect_trace: Trace,
    humans: Vec<(Race, Trace)>,
}

fn drive_all(track: &Arc<Track>, trials: u64) -> Drives {
    let p = CarParams::BASE;
    let best = perfect_run(track, &p, true);
    let plan = HumanPlan::new(track, &p);
    let humans = (0..trials)
        .map(|seed| {
            let mut race = Race::new(track.clone(), p);
            let trace = drive_traced(&mut race, &mut HumanBot::new(plan.clone(), seed));
            (race, trace)
        })
        .collect();
    Drives {
        perfect: best.race,
        perfect_trace: best.trace,
        humans,
    }
}

fn draw_map(c: &mut Canvas, t: &Track, d: &Drives) {
    let pts = layout(t);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y, _) in &pts {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let pad = 30.0;
    let scale = ((W as f64 - 2.0 * pad) / (x1 - x0).max(1.0))
        .min((MAP_H as f64 - 2.0 * pad) / (y1 - y0).max(1.0));
    let ox = (W as f64 - (x1 - x0) * scale) / 2.0;
    let oy = (MAP_H as f64 - (y1 - y0) * scale) / 2.0;
    let to_px = |x: f64, y: f64| {
        (
            ox + (x - x0) * scale,
            MAP_H as f64 - (oy + (y - y0) * scale),
        )
    };
    // A point `off` metres right of the centre line at distance `z`.
    let at = |z: f64, off: f64| {
        let i = ((z / SEG_LEN) as usize).min(pts.len() - 2);
        let f = (z / SEG_LEN - i as f64).clamp(0.0, 1.0);
        let (a, b) = (pts[i], pts[i + 1]);
        let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        // Right of the heading is the heading turned a quarter clockwise.
        to_px(x + off * dmath::sin(b.2), y - off * dmath::cos(b.2))
    };
    for (i, s) in t.segments().iter().enumerate() {
        let (a, b) = (
            at(i as f64 * SEG_LEN, 0.0),
            at((i as f64 + 1.0) * SEG_LEN, 0.0),
        );
        let w = (2.0 * s.half_width() * scale).max(3.0);
        c.line(a, b, w, surface_colour(s.surface));
        if s.has(flags::DRIFT_REQ) {
            c.line(a, b, (w * 0.5).max(2.0), [220, 50, 50]);
        } else if s.has(flags::DRIFT_HINT) {
            c.line(a, b, (w * 0.5).max(2.0), [240, 140, 40]);
        }
    }
    for (i, s) in t.segments().iter().enumerate() {
        if s.has(flags::CREST) {
            let (x, y) = at(i as f64 * SEG_LEN, 0.0);
            c.disc(x, y, 4.0, [250, 230, 60]);
        }
    }
    if let Some((_, trace)) = d.humans.first() {
        let vmax = CarParams::BASE.vmax;
        for &(z, x, v) in &trace.samples {
            let (px, py) = at(z, x);
            c.disc(px, py, 1.2, ramp(v / vmax));
        }
    }
    for o in t.objects() {
        let (x, y) = at(o.z(), o.x());
        c.disc(x, y, 2.0, [10, 10, 10]);
    }
    let (sx, sy) = at(0.0, 0.0);
    c.disc(sx, sy, 6.0, [60, 210, 80]);
    for k in 0..t.checkpoints().len() {
        let (x, y) = at(t.checkpoint_z(k), 0.0);
        c.disc(x, y, 6.0, WHITE);
    }
}

fn draw_speed(c: &mut Canvas, t: &Track, d: &Drives) {
    let top = MAP_H;
    c.rect(0, top, W - 1, top + SPEED_H - 1, PANEL);
    let len = t.length();
    let px = |z: f64| 20.0 + z / len * (W as f64 - 40.0);
    let (emin, emax) = (0..=t.len()).fold((f64::MAX, f64::MIN), |(lo, hi), i| {
        let e = t.elevation_at(i as f64 * SEG_LEN);
        (lo.min(e), hi.max(e))
    });
    let base = (top + SPEED_H - 10) as f64;
    for i in 0..t.len() {
        let e = t.elevation_at(i as f64 * SEG_LEN);
        let h = if emax > emin {
            (e - emin) / (emax - emin) * 60.0
        } else {
            0.0
        };
        let x = px(i as f64 * SEG_LEN);
        c.line((x, base), (x, base - h), 2.0, [70, 70, 84]);
        if t.segments()[i].has(flags::DRIFT_REQ) {
            c.line((x, base), (x, base - 6.0), 2.0, [220, 50, 50]);
        }
    }
    for k in 0..t.checkpoints().len() {
        let x = px(t.checkpoint_z(k));
        c.line((x, top as f64 + 5.0), (x, base), 1.0, [90, 90, 110]);
    }
    let vy = |v: f64| base - v / 60.0 * (SPEED_H as f64 - 20.0);
    let mut plot = |trace: &Trace, col: Rgb, w: f64| {
        for pair in trace.samples.windows(2) {
            c.line(
                (px(pair[0].0), vy(pair[0].2)),
                (px(pair[1].0), vy(pair[1].2)),
                w,
                col,
            );
        }
    };
    for (_, tr) in &d.humans {
        plot(tr, ORANGE, 1.0);
    }
    plot(&d.perfect_trace, WHITE, 1.5);
}

/// Section times, seconds, from a race's splits.
fn sections(r: &Race) -> Vec<f64> {
    let mut last = 0;
    r.splits()
        .iter()
        .map(|&s| {
            let d = (s - last) as f64 * DT;
            last = s;
            d
        })
        .collect()
}

fn draw_bars(c: &mut Canvas, groups: &[Vec<f64>]) {
    let top = MAP_H + SPEED_H;
    c.rect(0, top, W - 1, top + BARS_H - 1, BG);
    let max = groups.iter().flatten().fold(1.0f64, |m, &v| m.max(v));
    let base = (top + BARS_H - 10) as f64;
    let unit = (BARS_H as f64 - 25.0) / max.ceil();
    for s in 0..=max.ceil() as i32 {
        let y = base - s as f64 * unit;
        c.line((20.0, y), (W as f64 - 20.0, y), 1.0, [50, 50, 64]);
    }
    let gw = (W as f64 - 40.0) / groups.len().max(1) as f64;
    for (g, bars) in groups.iter().enumerate() {
        let bw = (gw - 20.0) / bars.len().max(1) as f64;
        for (b, &v) in bars.iter().enumerate() {
            let x = 30.0 + g as f64 * gw + b as f64 * bw;
            let col = if v < 0.0 { [220, 50, 50] } else { ORANGE };
            c.rect(
                x as i32,
                base as i32,
                (x + bw - 2.0) as i32,
                (base - v.max(0.05) * unit) as i32,
                col,
            );
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let (mut track, mut ascii, mut png, mut trials) = (None, false, None, 8u64);
    let mut args = argv.iter();
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .unwrap_or_else(|| usage("missing value"))
        };
        match flag.as_str() {
            "--fixture" => {
                track = Some(fixture(&value()).unwrap_or_else(|| usage("unknown fixture")))
            }
            "--ascii" => ascii = true,
            "--png" => png = Some(value()),
            "--trials" => trials = value().parse().unwrap_or_else(|_| usage("bad --trials")),
            other => usage(&format!("unknown flag {other}")),
        }
    }
    let track = Arc::new(track.unwrap_or_else(|| usage("no track chosen")));
    if ascii || png.is_none() {
        print!("{}", ascii_strip(&track));
    }
    let Some(path) = png else { return };

    let d = drive_all(&track, trials);
    let best = sections(&d.perfect);
    let groups: Vec<Vec<f64>> = (0..best.len())
        .map(|k| {
            d.humans
                .iter()
                .map(|(r, _)| sections(r).get(k).map_or(0.0, |s| s - best[k]))
                .collect()
        })
        .collect();
    let mut human: Vec<u32> = d.humans.iter().map(|(r, _)| r.tick()).collect();
    human.sort_unstable();
    let p = d.perfect.tick() as f64 * DT;
    let med = human[human.len() / 2] as f64 * DT;
    println!(
        "perfect {p:.2} s, human median {med:.2} s over {trials} trials, gap {:.1}%",
        (med - p) / p * 100.0
    );

    let mut c = Canvas::new(W, MAP_H + SPEED_H + BARS_H, BG);
    draw_map(&mut c, &track, &d);
    draw_speed(&mut c, &track, &d);
    draw_bars(&mut c, &groups);
    std::fs::write(&path, png::encode(c.w as u32, c.h as u32, &c.px)).unwrap_or_else(|e| {
        eprintln!("writing {path}: {e}");
        std::process::exit(1);
    });
    println!("wrote {path}");
}
