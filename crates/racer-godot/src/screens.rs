//! Every screen, painted from `&Game`, and the buttons it lays out: what
//! the next frame's touches are tested against (an immediate-mode UI, as in
//! Salt Line). Racing is [`crate::render`]; this adds the title, the fork
//! cards, the results and the pause overlay. Engine-free.

use racer_sim::car::Car;
use racer_sim::gen::difficulty::TRACKS;
use racer_sim::input::Input;
use racer_sim::run::RunPhase;
use racer_sim::track::{flags, Track, SEG_LEN};
use racer_sim::upgrade::{upgrade_of, Upgrade};

use crate::app::{Game, Mode};
use crate::layout::{Layout, Rect};
use crate::palette::*;
use crate::raster::Canvas;
use crate::render::{self, plate, secs, Held, RaceScene};
use crate::sprites::Art;
use crate::touch::{Action, Button};

/// Paint the frame for `g` and return its buttons.
pub fn frame(g: &mut Game, c: &mut Canvas, art: &Art) -> Vec<Button> {
    let l = g.layout;
    c.resize(l.w, l.h);
    match g.mode {
        Mode::Title | Mode::Building => title(g, c, art, &l),
        Mode::Racing => racing(g, c, art, &l),
        Mode::Fork => fork(g, c, &l),
        Mode::Results => results(g, c, &l),
    }
}

fn button(out: &mut Vec<Button>, rect: Rect, action: Action) {
    out.push(Button { rect, action });
}

/// A labelled plate, and its button.
fn labelled(c: &mut Canvas, out: &mut Vec<Button>, r: Rect, label: &str, a: Action, hot: bool) {
    plate(c, r, false);
    if hot {
        c.rect_outline(r.x, r.y, r.w, r.h, YELLOW);
    }
    c.text_centred(r.x + r.w / 2, r.y + r.h / 2 - 5, label, WHITE, 2);
    button(out, r, a);
}

/// Text with a one-pixel ink shadow, centred.
fn banner(c: &mut Canvas, cx: i32, y: i32, s: &str, ink: u8, scale: i32) {
    c.text_centred(cx + 1, y + 1, s, INK, scale);
    c.text_centred(cx, y, s, ink, scale);
}

fn title(g: &Game, c: &mut Canvas, art: &Art, l: &Layout) -> Vec<Button> {
    // An empty road rolling behind the title.
    let t = &g.title_track;
    let span = (t.length() - 300.0).max(1.0);
    let car = Car {
        z: 20.0 + (g.clock * 24.0) % span,
        v: 24.0,
        ..Car::default()
    };
    let scene = RaceScene {
        track: t,
        car,
        tick: (g.clock * 120.0) as u32,
        timer: 0,
        timed: true,
        next_gate: t.checkpoints().len(),
        input: Input::NEUTRAL,
        index: 0,
        tracks: TRACKS,
        bg_shift: (g.clock * 4.0) as i32,
        ghost: None,
        ghost_note: None,
    };
    render::road(c, l, art, &g.title_decor, &scene);
    let cx = l.w / 2;
    banner(c, cx, 30, "RACER", YELLOW, 5);
    banner(c, cx, 62, "EVERY ROAD PROVEN", WHITE, 1);
    let h = l.hud;
    c.rect(h.x, h.y, h.w, h.h, INK);
    c.text_centred(cx, h.y + 3, "ARROWS STEER  DOWN BRAKE", ASPHALT_LT, 1);
    c.text_centred(cx, h.y + 12, "SPACE OR SHIFT DRIFTS", ASPHALT_LT, 1);
    c.rect(l.pad.x, l.pad.y, l.pad.w, l.pad.h, INK);
    // Buttons stay above the inset; the pad's background fills it.
    let p = l.controls;
    let mut out = Vec::new();
    if g.mode == Mode::Building {
        let dots = ".".repeat((g.clock * 3.0) as usize % 4);
        c.text_centred(
            cx,
            p.y + p.h / 2 - 5,
            &format!("BUILDING TRACK{dots}"),
            WHITE,
            1,
        );
        return out;
    }
    let m = 6;
    let start = Rect::new(m, p.y + m, l.w - 2 * m, 46);
    let demo = Rect::new(
        m,
        start.y + start.h + 6,
        l.w - 2 * m,
        p.h - start.h - 2 * m - 6,
    );
    labelled(c, &mut out, start, "START", Action::Start, true);
    labelled(c, &mut out, demo, "DEMO", Action::Demo, false);
    out
}

fn racing(g: &Game, c: &mut Canvas, art: &Art, l: &Layout) -> Vec<Button> {
    let Some(run) = &g.run else {
        return Vec::new();
    };
    let mut scene = RaceScene::of(run.race(), run.index(), run.tracks(), g.bg.round() as i32);
    scene.ghost = g.ghost().map(|gh| (gh.car().z, gh.car().x));
    scene.ghost_note = g.ghost_note();
    let held = if g.paused { Held::default() } else { g.held };
    render::race_frame(c, l, art, &g.decor, &scene, held);
    let mut out = Vec::new();
    if g.demo() {
        let w = crate::font::small_width("DEMO  TAP TO PLAY") as i32;
        c.rect(2, 2, w + 4, 9, INK);
        c.text(4, 4, "DEMO  TAP TO PLAY", YELLOW, 1);
        return out;
    }
    if g.paused {
        let r = Rect::new(16, 50, l.w - 32, 120);
        c.rect(r.x, r.y, r.w, r.h, INK);
        c.rect_outline(r.x, r.y, r.w, r.h, ASPHALT_LT);
        banner(c, l.w / 2, r.y + 10, "PAUSED", WHITE, 2);
        let resume = Rect::new(r.x + 10, r.y + 34, r.w - 20, 34);
        let quit = Rect::new(r.x + 10, r.y + 76, r.w - 20, 34);
        labelled(c, &mut out, resume, "RESUME", Action::Resume, true);
        labelled(c, &mut out, quit, "QUIT", Action::Quit, false);
        return out;
    }
    // The pause button: top-left of the road view.
    let pr = Rect::new(2, 2, 16, 14);
    c.rect(pr.x, pr.y, pr.w, pr.h, INK);
    c.rect(pr.x + 5, pr.y + 3, 2, 8, WHITE);
    c.rect(pr.x + 9, pr.y + 3, 2, 8, WHITE);
    // Generous hit area: a thumb is bigger than the icon.
    button(&mut out, Rect::new(0, 0, 28, 24), Action::Pause);
    button(&mut out, l.left, Action::Left);
    button(&mut out, l.right, Action::Right);
    button(&mut out, l.brake, Action::Brake);
    button(&mut out, l.drift, Action::Drift);
    out
}

/// What a fork card says about a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackStats {
    /// To the finish, metres.
    pub length: u32,
    /// Checkpoints, the finish included.
    pub gates: usize,
    /// Drift-required corners.
    pub drift: usize,
    /// Blind crests.
    pub crests: usize,
    /// Traffic cars.
    pub cars: usize,
    /// The first section's allotment, ticks.
    pub first_allot: u32,
}

impl TrackStats {
    /// The stats of `t`.
    pub fn of(t: &Track) -> TrackStats {
        let segs = t.segments();
        let runs = |flag: u8| {
            (0..segs.len())
                .filter(|&i| segs[i].has(flag) && (i == 0 || !segs[i - 1].has(flag)))
                .count()
        };
        TrackStats {
            length: t.finish_z().round() as u32,
            gates: t.checkpoints().len(),
            drift: runs(flags::DRIFT_REQ),
            crests: runs(flags::CREST),
            cars: t.traffic().len(),
            first_allot: t.allot_ticks().first().copied().unwrap_or(0),
        }
    }
}

/// What an upgrade does, for its card.
pub fn upgrade_text(u: Upgrade) -> &'static str {
    match u {
        Upgrade::Engine => "+5% TOP SPEED",
        Upgrade::Turbo => "+12% ACCEL",
        Upgrade::Tyres => "+7% GRIP",
        Upgrade::Brakes => "+20% BRAKES",
        Upgrade::Charger => "+30% CHARGE",
    }
}

/// A top-down sketch of `t` fitted into `r`: the road white, drift
/// corners yellow, crests red.
fn preview(c: &mut Canvas, t: &Track, r: Rect) {
    let mut pts = Vec::with_capacity(t.len() + 1);
    let (mut x, mut y, mut th) = (0.0f64, 0.0f64, 0.0f64);
    pts.push((x, y));
    for s in t.segments() {
        th += s.curvature() * SEG_LEN;
        x += th.sin() * SEG_LEN;
        y -= th.cos() * SEG_LEN;
        pts.push((x, y));
    }
    let (x0, x1) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
    let (y0, y1) = pts
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
    let k = ((r.w - 4) as f64 / (x1 - x0).max(1.0)).min((r.h - 4) as f64 / (y1 - y0).max(1.0));
    let ox = r.x as f64 + (r.w as f64 - (x1 - x0) * k) / 2.0;
    let oy = r.y as f64 + (r.h as f64 - (y1 - y0) * k) / 2.0;
    let at = |p: (f64, f64)| {
        (
            (ox + (p.0 - x0) * k).round() as i32,
            (oy + (p.1 - y0) * k).round() as i32,
        )
    };
    for (i, s) in t.segments().iter().enumerate() {
        let (px, py) = at(pts[i]);
        let ink = if s.has(flags::DRIFT_REQ) {
            YELLOW
        } else {
            WHITE
        };
        c.put(px, py, ink);
        if s.has(flags::CREST) {
            c.rect(px - 1, py - 1, 2, 2, RED);
        }
    }
    let (sx, sy) = at(pts[0]);
    c.rect(sx - 1, sy - 1, 3, 3, GRASS_LT);
}

fn fork(g: &mut Game, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    let (seed, index, margin) = match &g.run {
        Some(run) => (
            run.run_seed(),
            run.index(),
            run.results().last().map_or(0, |r| r.margin),
        ),
        None => return out,
    };
    banner(
        c,
        l.w / 2,
        6,
        &format!("TRACK {} CLEAR", index + 1),
        YELLOW,
        2,
    );
    c.text_centred(
        l.w / 2,
        22,
        &format!("CLOSEST GATE {}S LEFT", secs(margin)),
        WHITE,
        1,
    );
    c.text_centred(l.w / 2, 32, "CHOOSE YOUR ROAD", ASPHALT_LT, 1);
    let top = 44;
    let card_h = (l.bottom() - top - 30).min(250);
    for b in 0..2u8 {
        let r = Rect::new(4 + b as i32 * 88, top, 84, card_h);
        let hot = g.fork_sel == b;
        c.rect(r.x, r.y, r.w, r.h, ASPHALT_DK);
        c.rect_outline(r.x, r.y, r.w, r.h, if hot { YELLOW } else { ASPHALT_LT });
        let cx = r.x + r.w / 2;
        c.text_centred(
            cx,
            r.y + 4,
            if b == 0 { "EASIER" } else { "HARDER" },
            if b == 0 { GRASS_LT } else { RED },
            1,
        );
        if let Some(u) = upgrade_of(seed, index + 1, b) {
            c.text_centred(cx, r.y + 14, &u.name().to_uppercase(), YELLOW, 2);
            c.text_centred(cx, r.y + 28, upgrade_text(u), WHITE, 1);
        }
        let key = (seed, index + 1, b);
        match g.track(key) {
            Some(t) => {
                let s = TrackStats::of(&t);
                let pr = Rect::new(r.x + 3, r.y + 38, r.w - 6, 70);
                c.rect(pr.x, pr.y, pr.w, pr.h, INK);
                preview(c, &t, pr);
                let lines = [
                    format!("LEN {}.{}KM", s.length / 1000, s.length % 1000 / 100),
                    format!("GATES {}", s.gates),
                    format!("DRIFT {}", s.drift),
                    format!("CRESTS {}", s.crests),
                    format!("CARS {}", s.cars),
                    format!("TIME {}S", secs(s.first_allot)),
                ];
                for (k, line) in lines.iter().enumerate() {
                    c.text(r.x + 5, pr.y + pr.h + 6 + k as i32 * 9, line, WHITE, 1);
                }
                button(
                    &mut out,
                    r,
                    if b == 0 {
                        Action::PickEasy
                    } else {
                        Action::PickHard
                    },
                );
            }
            None => {
                let dots = ".".repeat((g.clock * 3.0) as usize % 4);
                c.text_centred(cx, r.y + 70, &format!("BUILDING{dots}"), ASPHALT_LT, 1);
            }
        }
    }
    let hint = if g.demo() {
        "DEMO PICKS IN A MOMENT"
    } else {
        "TAP A ROAD TO TAKE IT"
    };
    c.text_centred(l.w / 2, l.bottom() - 18, hint, ASPHALT_LT, 1);
    out
}

fn results(g: &Game, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    let Some(run) = &g.run else { return out };
    let won = run.phase() == RunPhase::Won;
    banner(
        c,
        l.w / 2,
        16,
        match (won, &g.custom) {
            (true, None) => "RUN CLEAR",
            (true, Some(c)) if c.timed => "COURSE CLEAR",
            (true, Some(_)) => "FINISHED",
            (false, _) => "TIME UP",
        },
        if won { YELLOW } else { RED },
        3,
    );
    c.text(10, 48, "TRACK  ROAD    TIME   GATE", ASPHALT_LT, 1);
    let mut total = 0;
    for (k, r) in run.results().iter().enumerate() {
        total += r.ticks;
        let road = if r.index == 0 {
            "-"
        } else if r.branch == 0 {
            "EASY"
        } else {
            "HARD"
        };
        let line = format!(
            "{:<6} {:<6} {:>6} {:>5}",
            r.index + 1,
            road,
            secs(r.ticks),
            secs(r.margin)
        );
        c.text(
            10,
            60 + k as i32 * 10,
            &line,
            if r.margin == 0 { RED } else { WHITE },
            1,
        );
    }
    c.text(10, 118, &format!("TOTAL {}S", secs(total)), YELLOW, 1);
    c.text(10, 130, &run.loadout().describe().to_uppercase(), WHITE, 1);
    if let Some(cu) = &g.custom {
        let what = if cu.timed { "PROVEN" } else { "FREE DRIVE" };
        let name = if cu.layout.name.is_empty() {
            "LAYOUT"
        } else {
            &cu.layout.name
        };
        c.text(10, 142, &format!("{name}  {what}"), ASPHALT_LT, 1);
    }
    let r = Rect::new(6, l.bottom() - 52, l.w - 12, 46);
    if !g.demo() {
        // The same seed again: the same roads, with this run's best on each
        // track as its ghost.
        let retry = Rect::new(6, r.y - 40, l.w - 12, 34);
        labelled(
            c,
            &mut out,
            retry,
            if g.custom.is_some() {
                "RETRY COURSE"
            } else {
                "RETRY SEED"
            },
            Action::Retry,
            g.results_sel == 1,
        );
    }
    labelled(
        c,
        &mut out,
        r,
        "CONTINUE",
        Action::Continue,
        g.results_sel == 0,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::Inline;
    use crate::input::{Event, Intents, Key};
    use racer_sim::track::fixtures;

    fn game() -> Game {
        Game::new(Box::new(Inline::new(|k: crate::builder::Key| {
            let mut t = if k.2 == 0 {
                fixtures::drag_strip()
            } else {
                crate::render::tests::showcase()
            };
            let n = t.checkpoints().len();
            t.set_allotments(vec![racer_sim::ticks(if k.1 == 0 { 40.0 } else { 3.0 }); n]);
            t
        })))
    }

    fn step(g: &mut Game, events: Vec<Event>) -> (Canvas, Vec<Button>) {
        let i = Intents {
            events,
            window: (1080, 2160),
            entropy: 9,
            ..Intents::default()
        };
        g.tick(1.0 / 30.0, &i);
        let art = Art::new();
        let mut c = Canvas::new(1, 1);
        let b = frame(g, &mut c, &art);
        g.set_buttons(b.clone());
        (c, b)
    }

    fn has(b: &[Button], a: Action) -> bool {
        b.iter().any(|x| x.action == a)
    }

    /// Title, racing, paused, fork and results frames, for the golden
    /// digests and for eyeballing.
    pub(crate) fn screens() -> Vec<(&'static str, Canvas)> {
        let mut g = game();
        let mut out = Vec::new();
        let (c, b) = step(&mut g, vec![]);
        assert!(has(&b, Action::Start) && has(&b, Action::Demo));
        out.push(("title", c));
        step(&mut g, vec![Event::Key(Key::Enter)]);
        let (c, b) = step(&mut g, vec![]);
        assert_eq!(g.mode, Mode::Racing);
        assert!(has(&b, Action::Left) && has(&b, Action::Drift) && has(&b, Action::Pause));
        out.push(("racing_start", c));
        let (c, b) = step(&mut g, vec![Event::Back]);
        assert!(has(&b, Action::Resume) && !has(&b, Action::Left));
        out.push(("paused", c));
        step(&mut g, vec![Event::Back]);
        while g.mode == Mode::Racing {
            step(&mut g, vec![]);
        }
        let (c, b) = step(&mut g, vec![]);
        assert_eq!(g.mode, Mode::Fork);
        assert!(has(&b, Action::PickEasy) && has(&b, Action::PickHard));
        out.push(("fork", c));
        step(&mut g, vec![Event::Key(Key::Right)]);
        step(&mut g, vec![Event::Key(Key::Enter)]);
        while g.mode == Mode::Racing {
            step(&mut g, vec![]);
        }
        let (c, b) = step(&mut g, vec![]);
        assert_eq!(g.mode, Mode::Results);
        assert!(has(&b, Action::Continue) && has(&b, Action::Retry));
        out.push(("time_up", c));
        out
    }

    #[test]
    fn every_screen_lays_out_its_buttons_inside_the_buffer_in_palette_colours() {
        for (name, c) in screens() {
            assert!(c.palette_only(), "{name}");
            assert_eq!((c.w, c.h), (180, 360), "{name}");
        }
    }

    #[test]
    fn stats_count_what_the_track_has() {
        let t = crate::render::tests::showcase();
        let s = TrackStats::of(&t);
        assert_eq!(s.gates, 2);
        assert_eq!(s.drift, 1);
        assert_eq!(s.cars, 1);
        assert_eq!(s.length, t.finish_z() as u32);
    }

    #[test]
    fn golden_screens() {
        let out = screens()
            .iter()
            .map(|(name, c)| format!("{name}: {:#018x}", c.digest()))
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(out);
    }

    #[test]
    #[ignore]
    fn dump_screens() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/frames");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".gdignore"), "").unwrap();
        for (name, c) in screens() {
            std::fs::write(dir.join(format!("screen_{name}.ppm")), c.ppm()).unwrap();
        }
    }
}
