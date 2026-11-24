//! Every screen, painted from `&Game`, and the buttons it lays out: what
//! the next frame's touches are tested against (an immediate-mode UI, as in
//! Salt Line). Racing is [`crate::render`]; this adds the title, the fork
//! cards, the results and the pause overlay. Engine-free.

use racer_sim::biome::Biome;
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
use crate::season::Season;
use crate::sprites::Art;
use crate::touch::{Action, Button};

/// Paint the frame for `g` and return its buttons.
pub fn frame(g: &mut Game, c: &mut Canvas, art: &Art) -> Vec<Button> {
    let l = g.layout;
    c.resize(l.w, l.h);
    if g.settings {
        return settings(g, c, &l);
    }
    // Menus and the editor in the meadow's colours; a road view sets its
    // biome's (`render::road`).
    c.set_palette(&COLOURS);
    match g.mode {
        Mode::Title | Mode::Building => title(g, c, art, &l),
        Mode::Racing => racing(g, c, art, &l),
        Mode::Fork => fork(g, c, &l),
        Mode::Results => results(g, c, &l),
        Mode::Editor => crate::editor::draw::frame(&g.editor, c, &l, g.clock),
    }
}

/// Settings stay above the portrait gesture inset and are also accessible
/// while a race is paused. Switching schemes releases all captured touches.
fn settings_button(c: &mut Canvas, out: &mut Vec<Button>) {
    let r = Rect::new(128, 2, 50, 22);
    plate(c, r, false);
    c.text_centred(r.x + r.w / 2, r.y + 8, "SETTINGS", WHITE, 1);
    button(out, r, Action::Settings);
}

fn settings(g: &Game, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.set_palette(&COLOURS);
    c.fill(INK);
    banner(c, l.w / 2, 30, "SETTINGS", WHITE, 3);
    c.text_centred(l.w / 2, 76, "DRIVING CONTROLS", ICE, 1);
    let mut out = Vec::new();
    labelled(
        c,
        &mut out,
        Rect::new(10, 96, l.w - 20, 40),
        if g.modern_touch {
            "ONE THUMB  ON"
        } else {
            "BUTTONS  ON"
        },
        Action::ToggleControls,
        true,
    );
    for (y, text) in if g.modern_touch {
        [
            (154, "DRAG LEFT / RIGHT TO STEER"),
            (170, "PUSH UP TO DRIFT"),
            (186, "PULL DOWN TO BRAKE"),
            (202, "CENTRE TO ACCELERATE"),
            (218, "TOUCH DOWN ANYWHERE BELOW HUD"),
        ]
    } else {
        [
            (154, "ORIGINAL TOUCH BUTTONS"),
            (170, "LEFT / RIGHT STEER"),
            (186, "HOLD BRAKE TO SLOW"),
            (202, "HOLD DRIFT THROUGH A TURN"),
            (218, "KEYBOARD AND PAD ALSO WORK"),
        ]
    } {
        c.text_centred(l.w / 2, y, text, WHITE, 1);
    }
    labelled(
        c,
        &mut out,
        Rect::new(30, l.bottom() - 50, l.w - 60, 34),
        "DONE",
        Action::CloseSettings,
        false,
    );
    out
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
        biome: g.title_biome(),
        season: g.season(),
        anim: g.anim(),
    };
    render::road(c, l, art, &g.title_decor[scene.biome as usize], &scene);
    let cx = l.w / 2;
    if scene.season == Season::Halloween {
        halloween_banner(c, art, cx, g.anim());
    }
    banner(c, cx, 30, "RACER", YELLOW, 5);
    banner(c, cx, 62, "EVERY ROAD PROVEN", WHITE, 1);
    // Which of the five looks is rolling by (a twin's, in a season).
    banner(c, cx, 74, scene.look().name(), ICE, 1);
    if let Some(note) = &g.road_note {
        banner(c, cx, 86, note, RED, 1);
    }
    if let Some(note) = &g.save_note {
        banner(c, cx, 96, note, RED, 1);
    }
    let h = l.hud;
    c.rect(h.x, h.y, h.w, h.h, INK);
    c.text_centred(
        cx,
        h.y + 3,
        if g.modern_touch {
            "ONE THUMB: DRAG TO STEER"
        } else {
            "ARROWS STEER  DOWN BRAKE"
        },
        ASPHALT_LT,
        1,
    );
    c.text_centred(
        cx,
        h.y + 12,
        if g.modern_touch {
            "UP DRIFT   DOWN BRAKE"
        } else {
            "SPACE OR SHIFT DRIFTS"
        },
        ASPHALT_LT,
        1,
    );
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
    // Today's daily: its date and the best of today's tries.
    if let Some(day) = g.today {
        let best = match g.daily_best() {
            Some(b) => format!("BEST {}/{TRACKS}  {}S", b.cleared, secs(b.ticks)),
            None => "NOT RACED YET".to_string(),
        };
        banner(
            c,
            cx,
            86,
            &format!("DAILY {}", crate::daily::date(day)),
            WHITE,
            1,
        );
        banner(c, cx, 97, &best, WHITE, 1);
    }
    let m = 6;
    // START and DAILY share the top row: a fresh run, or today's (the same
    // for everyone).
    let half = (l.w - 3 * m) / 2;
    let start = Rect::new(m, p.y + m, half, 46);
    let daily = Rect::new(2 * m + half, p.y + m, l.w - 3 * m - half, 46);
    let row = start.y + start.h + 6;
    let row_h = p.h - start.h - 2 * m - 6;
    // TRACKS: the bundled courses, your layouts and the editor.
    let tracks = Rect::new(m, row, 100, row_h);
    let demo = Rect::new(
        tracks.x + tracks.w + 6,
        row,
        l.w - m - (tracks.x + tracks.w + 6),
        row_h,
    );
    labelled(c, &mut out, start, "START", Action::Start, true);
    labelled(c, &mut out, daily, "DAILY", Action::Daily, false);
    if g.tier.is_shareware() {
        // No editor in the shareware edition (`tier.rs`): the plate stays,
        // dimmed and without a button, so the title keeps its layout.
        plate(c, tracks, false);
        let cx = tracks.x + tracks.w / 2;
        c.text_centred(cx, tracks.y + tracks.h / 2 - 9, "TRACKS", ASPHALT_LT, 2);
        c.text_centred(
            cx,
            tracks.y + tracks.h / 2 + 8,
            crate::tier::FULL_GAME_ONLY,
            ASPHALT_LT,
            1,
        );
    } else {
        labelled(c, &mut out, tracks, "TRACKS", Action::Tracks, false);
    }
    labelled(c, &mut out, demo, "DEMO", Action::Demo, false);
    settings_button(c, &mut out);
    out
}

/// The title's Halloween treatment (DESIGN §5.2): HAPPY HALLOWEEN over
/// the RACER banner between two jack-o'-lanterns, their faces flickering
/// by the presentation clock.
fn halloween_banner(c: &mut Canvas, art: &Art, cx: i32, anim: u32) {
    let text = "HAPPY HALLOWEEN";
    let w = crate::font::small_width(text) as i32;
    let y = 18;
    c.rect(cx - w / 2 - 3, y - 2, w + 6, 9, INK);
    c.text_centred(cx, y, text, YELLOW, 1);
    // A pumpkin either side: the pile's big lantern, the far side mirrored.
    let p = &art.pumpkins.sprites[6];
    let flicker = (anim / 5) % 7 == 3;
    for (x, flip) in [(cx - w / 2 - 5 - p.w, false), (cx + w / 2 + 5, true)] {
        c.blit(p, x, y + 9 - p.h, i32::MAX, flip);
        if flicker {
            // A gust: the faces go dark for a frame.
            for yy in y + 9 - p.h..y + 9 {
                for xx in x..x + p.w {
                    if c.get(xx, yy) == c.palette()[YELLOW as usize] {
                        c.put(xx, yy, RED_DK);
                    }
                }
            }
        }
    }
}

fn racing(g: &Game, c: &mut Canvas, art: &Art, l: &Layout) -> Vec<Button> {
    let Some(run) = &g.run else {
        return Vec::new();
    };
    let mut scene = RaceScene::of(run.race(), run.index(), run.tracks(), g.bg.round() as i32);
    scene.biome = g.biome;
    scene.season = g.season();
    scene.anim = g.anim();
    scene.ghost = g.ghost().map(|gh| (gh.car().z, gh.car().x));
    scene.ghost_note = g.ghost_note();
    let held = if g.paused { Held::default() } else { g.held };
    if g.modern_touch {
        render::road(c, l, art, &g.decor, &scene);
        render::hud(c, l, &scene);
        render::touch_pad(c, l, held, if g.paused { None } else { g.touch.gesture() });
    } else {
        render::race_frame(c, l, art, &g.decor, &scene, held);
    }
    let mut out = Vec::new();
    if g.demo() {
        let w = crate::font::small_width("DEMO  TAP TO PLAY") as i32;
        c.rect(2, 2, w + 4, 9, INK);
        c.text(4, 4, "DEMO  TAP TO PLAY", YELLOW, 1);
        return out;
    }
    if g.paused {
        let fork = g.can_fork();
        let r = Rect::new(16, 50, l.w - 32, if fork { 162 } else { 120 });
        c.rect(r.x, r.y, r.w, r.h, INK);
        c.rect_outline(r.x, r.y, r.w, r.h, ASPHALT_LT);
        banner(c, l.w / 2, r.y + 10, "PAUSED", WHITE, 2);
        let resume = Rect::new(r.x + 10, r.y + 34, r.w - 20, 34);
        let quit = Rect::new(r.x + 10, r.y + 76, r.w - 20, 34);
        labelled(c, &mut out, resume, "RESUME", Action::Resume, true);
        let test = g.custom.as_ref().is_some_and(|cu| cu.test);
        labelled(
            c,
            &mut out,
            quit,
            if test { "EDITOR" } else { "QUIT" },
            Action::Quit,
            false,
        );
        if fork {
            // The track on screen, into the editor (DESIGN §9.11).
            let f = Rect::new(r.x + 10, r.y + 118, r.w - 20, 34);
            labelled(c, &mut out, f, "FORK TRACK", Action::ForkTrack, false);
        }
        settings_button(c, &mut out);
        return out;
    }
    // The pause button: top-left of the road view.
    let pr = Rect::new(2, 2, 16, 14);
    c.rect(pr.x, pr.y, pr.w, pr.h, INK);
    c.rect(pr.x + 5, pr.y + 3, 2, 8, WHITE);
    c.rect(pr.x + 9, pr.y + 3, 2, 8, WHITE);
    // Generous hit area: a thumb is bigger than the icon.
    button(&mut out, Rect::new(0, 0, 28, 24), Action::Pause);
    if g.modern_touch {
        button(&mut out, l.controls, Action::Drive);
    } else {
        button(&mut out, l.left, Action::Left);
        button(&mut out, l.right, Action::Right);
        button(&mut out, l.brake, Action::Brake);
        button(&mut out, l.drift, Action::Drift);
    }
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
                // The road's scenery first (DESIGN §5.1), then its numbers.
                let y0 = pr.y + pr.h + 6;
                let look = g.look(Biome::of_track(index + 1));
                c.text(r.x + 5, y0, look.name(), ICE, 1);
                for (k, line) in lines.iter().enumerate() {
                    c.text(r.x + 5, y0 + (k as i32 + 1) * 9, line, WHITE, 1);
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
                if g.road_refused(key) {
                    c.text_centred(cx, r.y + 70, "UNPROVEN", RED, 1);
                } else {
                    let dots = ".".repeat((g.clock * 3.0) as usize % 4);
                    c.text_centred(cx, r.y + 70, &format!("BUILDING{dots}"), ASPHALT_LT, 1);
                }
            }
        }
    }
    let hint = if g.demo() {
        "DEMO PICKS IN A MOMENT"
    } else {
        "TAP A ROAD TO TAKE IT"
    };
    c.text_centred(l.w / 2, l.bottom() - 18, hint, ASPHALT_LT, 1);
    if let Some(note) = &g.road_note {
        c.text_centred(l.w / 2, l.bottom() - 8, note, RED, 1);
    }
    out
}

fn results(g: &Game, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    let Some(run) = &g.run else { return out };
    let won = run.phase() == RunPhase::Won;
    let capped = g.shareware_ended();
    banner(
        c,
        l.w / 2,
        16,
        match (won, &g.custom) {
            _ if capped => "SHAREWARE END",
            (true, None) => "RUN CLEAR",
            (true, Some(c)) if c.timed => "COURSE CLEAR",
            (true, Some(_)) => "FINISHED",
            (false, _) => "TIME UP",
        },
        if won || capped { YELLOW } else { RED },
        if capped { 2 } else { 3 },
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
    } else if let Some(day) = g.daily {
        // Today's run: the same for everyone; tries and best kept on disk.
        let rec = g.daily_record.on(day);
        let best = rec.best.map_or("-".to_string(), |b| {
            format!("{}/{TRACKS} {}S", b.cleared, secs(b.ticks))
        });
        c.text(
            10,
            142,
            &format!("DAILY {}  TRY {}", crate::daily::date(day), rec.tries),
            YELLOW,
            1,
        );
        c.text(10, 152, &format!("TODAY'S BEST {best}"), YELLOW, 1);
    } else {
        // The seed, to fork any of its tracks FROM SEED.
        c.text(10, 142, &format!("SEED {}", run.run_seed()), ASPHALT_LT, 1);
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
    if g.can_fork() {
        // The track just raced, into the editor (DESIGN §9.11).
        let fork = Rect::new(6, r.y - 80, l.w - 12, 34);
        labelled(
            c,
            &mut out,
            fork,
            "FORK TRACK",
            Action::ForkTrack,
            g.results_sel == 2,
        );
    }
    let test = g.custom.as_ref().is_some_and(|cu| cu.test);
    labelled(
        c,
        &mut out,
        r,
        if test { "EDITOR" } else { "CONTINUE" },
        Action::Continue,
        g.results_sel == 0,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Effect;
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
        let art = Art::shared();
        let mut c = Canvas::new(1, 1);
        let b = frame(g, &mut c, art);
        g.set_buttons(b.clone());
        (c, b)
    }

    /// One frame on `day` (the node passes today's UTC day), keeping the
    /// effects the core asked for.
    fn step_on(g: &mut Game, events: Vec<Event>, day: u64) -> (Canvas, Vec<Button>, Vec<Effect>) {
        let i = Intents {
            events,
            window: (1080, 2160),
            entropy: 9,
            today: Some(day),
            ..Intents::default()
        };
        let fx = g.tick(1.0 / 30.0, &i);
        let art = Art::shared();
        let mut c = Canvas::new(1, 1);
        let b = frame(g, &mut c, art);
        g.set_buttons(b.clone());
        (c, b, fx)
    }

    /// 2026-10-19, UTC: the first day of the Halloween season.
    const DAY: u64 = 20_745;

    #[test]
    fn the_season_follows_the_day_and_the_flag_and_names_the_twins() {
        let mut g = game();
        step(&mut g, vec![]);
        assert_eq!(g.season(), Season::None, "no day yet: the normal art");
        step_on(&mut g, vec![], DAY);
        assert_eq!(g.season(), Season::Halloween);
        assert_eq!(g.look(Biome::Coast).name(), "GHOST COVE");
        assert_eq!(g.look(Biome::Night).name(), "MONSTER CITY");
        // Two weeks on, it is over.
        step_on(&mut g, vec![], DAY + 14);
        assert_eq!(g.season(), Season::None);
        assert_eq!(g.look(Biome::Coast).name(), "COAST");
        // The flag wins either way.
        g.forced_season = Some(Season::Halloween);
        assert_eq!(g.season(), Season::Halloween);
        step_on(&mut g, vec![], DAY);
        g.forced_season = Some(Season::None);
        assert_eq!(g.season(), Season::None);
        // A title frame in the season differs from one forced off, and
        // both are palette-only.
        let mut on = game();
        let (a, _, _) = step_on(&mut on, vec![], DAY);
        let mut off = game();
        off.forced_season = Some(Season::None);
        let (b, _, _) = step_on(&mut off, vec![], DAY);
        assert!(a.palette_only() && b.palette_only());
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn daily_races_todays_seed_and_keeps_its_record_and_run_file() {
        let mut g = game();
        let (_, b, _) = step_on(&mut g, vec![], DAY);
        assert!(has(&b, Action::Daily));
        let mut fx = Vec::new();
        fx.extend(step_on(&mut g, vec![Event::Key(Key::Char(b'D'))], DAY).2);
        while g.mode != Mode::Racing {
            fx.extend(step_on(&mut g, vec![], DAY).2);
        }
        assert_eq!(g.daily, Some(DAY));
        assert_eq!(
            g.run.as_ref().unwrap().run_seed(),
            racer_sim::run::daily_seed(DAY)
        );
        assert!(
            fx.iter().any(|e| matches!(e, Effect::SaveDaily { .. })),
            "the try is counted on disk"
        );
        while g.mode == Mode::Racing || g.mode == Mode::Fork || g.mode == Mode::Building {
            if g.mode == Mode::Fork {
                fx.extend(step_on(&mut g, vec![Event::Key(Key::Enter)], DAY).2);
            } else {
                fx.extend(step_on(&mut g, vec![], DAY).2);
            }
        }
        assert_eq!(g.mode, Mode::Results);
        let run_file = fx.iter().find_map(|e| match e {
            Effect::SaveRun { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert!(
            run_file.unwrap().contains("daily 2026-10-19\n"),
            "a daily run's file says so"
        );
        assert!(g.daily_record.on(DAY).tries >= 1);
    }

    #[test]
    fn demo_never_touches_the_daily() {
        let mut g = game();
        step_on(&mut g, vec![], DAY);
        let before = g.daily_record;
        g.set_buttons(vec![Button {
            rect: crate::layout::Rect::new(0, 0, 180, 400),
            action: Action::Demo,
        }]);
        let tap = vec![Event::FingerDown {
            index: 0,
            x: 5.0,
            y: 5.0,
        }];
        let mut fx = step_on(&mut g, tap, DAY).2;
        fx.extend(step_on(&mut g, vec![Event::FingerUp { index: 0 }], DAY).2);
        for _ in 0..300 {
            fx.extend(step_on(&mut g, vec![], DAY).2);
        }
        assert!(g.demo());
        assert_eq!(g.daily, None);
        assert_eq!(g.daily_record, before);
        assert!(!fx.iter().any(|e| matches!(e, Effect::SaveDaily { .. })));
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
        assert!(has(&b, Action::Start) && has(&b, Action::Daily) && has(&b, Action::Demo));
        out.push(("title", c));
        // The title on a day with a daily already raced: its date and best.
        let mut d = game();
        let mut rec = crate::daily::DailyRecord::default();
        rec.start(DAY);
        rec.offer(
            DAY,
            crate::daily::Score {
                cleared: 3,
                ticks: racer_sim::ticks(190.5),
            },
        );
        d.load_daily(&rec.encode());
        // DAY is in the Halloween season: this golden pins it off (the
        // flag's way) so it shows the daily alone.
        d.forced_season = Some(Season::None);
        out.push(("title_daily", step_on(&mut d, vec![], DAY).0));
        // The same day as the node would pass it, nothing forced: the
        // season comes from the date (DESIGN §5.2), so the title has its
        // Halloween treatment and the rolling road is PUMPKIN PATCH.
        let mut h = game();
        let (c, _, _) = step_on(&mut h, vec![], DAY);
        assert_eq!(h.season(), Season::Halloween);
        out.push(("title_halloween", c));
        step(&mut g, vec![Event::Key(Key::Enter)]);
        let (c, b) = step(&mut g, vec![]);
        assert_eq!(g.mode, Mode::Racing);
        assert!(has(&b, Action::Drive) && has(&b, Action::Pause));
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
        assert_eq!(g.biome, Biome::Meadow, "the first track is a meadow");
        step(&mut g, vec![Event::Key(Key::Right)]);
        step(&mut g, vec![Event::Key(Key::Enter)]);
        assert_eq!(g.biome, Biome::Coast, "the second track the coast");
        while g.mode == Mode::Racing {
            step(&mut g, vec![]);
        }
        let (c, b) = step(&mut g, vec![]);
        assert_eq!(g.mode, Mode::Results);
        assert!(has(&b, Action::Continue) && has(&b, Action::Retry));
        out.push(("time_up", c));
        out.extend(editor_screens());
        out
    }

    /// A press of the editor button `a`, wherever it is drawn.
    fn press(g: &mut Game, b: &[Button], a: crate::editor::Act) -> (Canvas, Vec<Button>) {
        let r = b
            .iter()
            .rev()
            .find(|x| x.action == Action::Edit(a))
            .unwrap_or_else(|| panic!("{a:?}"))
            .rect;
        let (x, y) = ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
        step(g, vec![Event::FingerDown { index: 0, x, y }]);
        step(g, vec![Event::FingerUp { index: 0 }])
    }

    /// The editor's pages (DESIGN §9.11), on RIDGE RUN, for the golden
    /// digests and the site.
    pub(crate) fn editor_screens() -> Vec<(&'static str, Canvas)> {
        use crate::editor::{Act, Item, Tool};
        use racer_sim::layout::ProveError;
        use racer_sim::solver::Failure;
        let builder =
            Inline::new(|_: crate::builder::Key| fixtures::drag_strip()).proving_with(|_| {
                Err(ProveError::Failed(
                    Failure::Margins {
                        passes: 12,
                        section: 1,
                    },
                    0,
                ))
            });
        let hold = builder.hold.clone();
        let mut g = Game::new(Box::new(builder));
        let mut s = crate::editor::store::Slots::default();
        s.put(
            1,
            &racer_sim::layout::bundled::layouts()[3].unproven(),
            None,
        );
        g.load_layouts(s.encode().as_deref(), None);
        let mut out = Vec::new();
        step(&mut g, vec![]);
        let (_, b) = step(&mut g, vec![Event::Key(Key::Char(b'T'))]);
        assert_eq!(g.mode, Mode::Editor);
        press(&mut g, &b, Act::Row(1));
        let (c, b) = step(&mut g, vec![]);
        assert!(has(&b, Action::Edit(Act::Item(Item::TestDrive))));
        out.push(("tracks", c));
        // EDIT RIDGE RUN, the sixth run, the bend.
        press(&mut g, &b, Act::Item(Item::Name));
        for _ in 0..5 {
            step(&mut g, vec![Event::Key(Key::Right)]);
        }
        let (c, b) = step(&mut g, vec![]);
        out.push(("editor_bend", c));
        // A hill on it: proof gone, a draft. PROVE: held on the worker.
        let (_, b) = press(&mut g, &b, Act::Tool(Tool::Hill));
        let (_, b) = press(&mut g, &b, Act::Plus);
        let (_, b) = press(&mut g, &b, Act::Plus);
        hold.set(true);
        press(&mut g, &b, Act::Prove);
        let (c, b) = step(&mut g, vec![]);
        out.push(("editor_proving", c));
        // The worker says no: the words, over the map.
        hold.set(false);
        step(&mut g, vec![]);
        press(&mut g, &b, Act::Tool(Tool::Objects));
        let (c, b) = step(&mut g, vec![]);
        out.push(("editor_failed", c));
        press(&mut g, &b, Act::Menu);
        let (c, b) = step(&mut g, vec![]);
        out.push(("editor_menu", c));
        press(&mut g, &b, Act::Item(Item::Save));
        let (c, b) = step(&mut g, vec![]);
        out.push(("editor_save", c));
        let (_, b) = press(&mut g, &b, Act::Back);
        let (_, _) = press(&mut g, &b, Act::Item(Item::Name));
        for _ in 0..4 {
            step(&mut g, vec![Event::Key(Key::Backspace)]);
        }
        for k in *b"2X2" {
            step(&mut g, vec![Event::Key(Key::Char(k))]);
        }
        let (c, b) = step(&mut g, vec![]);
        out.push(("editor_name", c));
        let (_, b) = press(&mut g, &b, Act::Back);
        let (_, _) = press(&mut g, &b, Act::Item(Item::Seed));
        for k in b"20261018" {
            step(&mut g, vec![Event::Key(Key::Char(*k))]);
        }
        step(&mut g, vec![Event::Key(Key::Right)]);
        step(&mut g, vec![Event::Key(Key::Right)]);
        let (c, _) = step(&mut g, vec![]);
        out.push(("editor_seed", c));
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
