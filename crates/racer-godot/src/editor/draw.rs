//! The editor's pages, painted (DESIGN §9.11), and the buttons each lays
//! out: an immediate-mode UI, like every other screen. Engine-free; the
//! golden screens (`screens.rs`) pin them.

use racer_sim::layout::Standing;
use racer_sim::track::{flags, ObjectKind, Surface};

use super::model::{metres, DriftMark};
use super::proof::ProofState;
use super::store::MAX_SLOTS;
use super::view::{self, EditRects, Fit};
use super::{Act, Editor, Entry, Item, Page, Tool, LIST_ROWS};
use crate::font;
use crate::layout::{Layout, Rect, W};
use crate::palette::*;
use crate::raster::Canvas;
use crate::render::plate;
use crate::touch::{Action, Button};

/// Pixels between the map's edge and the road.
pub const MAP_PAD: i32 = 6;

fn push(out: &mut Vec<Button>, rect: Rect, a: Act) {
    out.push(Button {
        rect,
        action: Action::Edit(a),
    });
}

/// A plate with a label, as a button. `hot`: selected or focused.
fn key(c: &mut Canvas, out: &mut Vec<Button>, r: Rect, label: &str, a: Act, hot: bool, dim: bool) {
    let inner = Rect::new(r.x + 1, r.y + 1, r.w - 2, r.h - 2);
    plate(c, inner, hot);
    let ink = if dim {
        ASPHALT_LT
    } else if hot {
        YELLOW
    } else {
        WHITE
    };
    c.text_centred(r.x + r.w / 2, r.y + r.h / 2 - 2, label, ink, 1);
    push(out, r, a);
}

/// A big labelled plate (scale-2 text where it fits).
fn big_key(c: &mut Canvas, out: &mut Vec<Button>, r: Rect, label: &str, a: Act, hot: bool) {
    plate(c, r, false);
    if hot {
        c.rect_outline(r.x, r.y, r.w, r.h, YELLOW);
    }
    let scale = if font::small_width(label) as i32 * 2 <= r.w - 6 {
        2
    } else {
        1
    };
    c.text_centred(
        r.x + r.w / 2,
        r.y + r.h / 2 - 5 * scale / 2,
        label,
        WHITE,
        scale,
    );
    push(out, r, a);
}

/// A thick line: a `(2r+1)`-pixel brush along Bresenham's line.
fn line(c: &mut Canvas, a: (i32, i32), b: (i32, i32), r: i32, ink: u8) {
    let (mut x, mut y) = a;
    let (dx, dy) = ((b.0 - a.0).abs(), -(b.1 - a.1).abs());
    let (sx, sy) = (
        if a.0 < b.0 { 1 } else { -1 },
        if a.1 < b.1 { 1 } else { -1 },
    );
    let mut err = dx + dy;
    loop {
        c.rect(x - r, y - r, 2 * r + 1, 2 * r + 1, ink);
        if (x, y) == b {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Text on an ink box, top-left at `(x, y)`.
fn tag(c: &mut Canvas, x: i32, y: i32, s: &str, ink: u8) {
    let w = font::small_width(s) as i32;
    c.rect(x - 1, y - 1, w + 2, 7, INK);
    c.text(x, y, s, ink, 1);
}

/// The road surface's ink on the map.
fn surface_ink(s: Surface) -> u8 {
    match s {
        Surface::Asphalt => ASPHALT_LT,
        Surface::Gravel => GRAVEL,
        Surface::Ice => ICE,
        Surface::Puddle => WATER,
    }
}

/// Paint the editor's page for `ed` and return its buttons.
pub fn frame(ed: &Editor, c: &mut Canvas, l: &Layout, clock: f64) -> Vec<Button> {
    match ed.page {
        Page::Edit => edit(ed, c, l, clock),
        Page::Menu => menu(ed, c, l),
        Page::Save => save(ed, c, l),
        Page::Name => name(ed, c, l),
        Page::Seed => seed(ed, c, l, clock),
        Page::Tracks => tracks(ed, c, l),
    }
}

/// The badge's text and colour.
fn badge(ed: &Editor, clock: f64) -> (String, u8) {
    let dots = ".".repeat((clock * 3.0) as usize % 4);
    match &ed.prover.state {
        ProofState::Unproven => ("DRAFT".into(), ASPHALT_LT),
        ProofState::Proving(_) => (format!("PROVING{dots}"), YELLOW),
        ProofState::Verifying(_) => (format!("VERIFYING{dots}"), YELLOW),
        ProofState::Proven(_) => ("PROVEN".into(), GRASS_LT),
        ProofState::Failed(_) => ("NOT PROVEN".into(), RED),
    }
}

/// The draft's title: its name, or where it came from.
fn title(ed: &Editor) -> String {
    let l = &ed.draft.layout;
    if !l.name.is_empty() {
        return l.name.clone();
    }
    match ed
        .origin
        .as_deref()
        .and_then(racer_sim::layout::Provenance::parse)
    {
        Some(p) if p.index == 0 => "SEED TRACK 1".into(),
        Some(p) => format!(
            "SEED TRACK {} {}",
            p.index + 1,
            ["EASY", "HARD"][p.branch.min(1) as usize]
        ),
        None => "UNNAMED".into(),
    }
}

/// The note at the bottom of a page, if one is up.
fn note_line(ed: &Editor, c: &mut Canvas, y: i32) {
    if let Some(n) = ed.note() {
        let w = font::small_width(n) as i32;
        let x = (W - w) / 2;
        c.rect(x - 3, y - 2, w + 6, 9, INK);
        c.rect_outline(x - 3, y - 2, w + 6, 9, YELLOW);
        c.text(x, y, n, YELLOW, 1);
    }
}

fn edit(ed: &Editor, c: &mut Canvas, l: &Layout, clock: f64) -> Vec<Button> {
    let e = EditRects::of(l);
    let d = &ed.draft;
    let segs = &d.layout.segments;
    let n = segs.len();
    let (sa, sb) = d.run_range(ed.sel);
    let mut out = Vec::new();
    c.fill(INK);

    // The map.
    let m = e.map;
    c.rect(m.x, m.y, m.w, m.h, GRASS_DK);
    let pts = view::plan(segs);
    let fit = Fit::new(&pts, m, MAP_PAD);
    let px: Vec<(i32, i32)> = pts.iter().map(|&p| fit.px(p)).collect();
    for i in sa..sb {
        line(c, px[i], px[i + 1], 2, YELLOW);
    }
    for (i, s) in segs.iter().enumerate() {
        let ink = if (sa..sb).contains(&i) {
            WHITE
        } else {
            surface_ink(s.surface)
        };
        line(c, px[i], px[i + 1], 1, ink);
        if s.surface != Surface::Asphalt && (sa..sb).contains(&i) {
            c.put(px[i].0, px[i].1, surface_ink(s.surface));
        }
    }
    for (i, s) in segs.iter().enumerate() {
        if s.has(flags::DRIFT_REQ) {
            c.put(px[i].0, px[i].1, RED);
        } else if s.has(flags::DRIFT_HINT) {
            c.put(px[i].0, px[i].1, YELLOW);
        }
    }
    // What stands by the road: beside it, on the side it stands.
    let side = |seg: usize, x_m: f64| -> (i32, i32) {
        let (a, b) = (pts[seg], pts[(seg + 1).min(n)]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(1e-9);
        let off = (x_m.abs() * fit.scale()).max(2.0) * x_m.signum();
        // The road's right, seen from above heading (dx, dy), is (-dy, dx).
        let p = (
            a.0 + (-dy / len) * off / fit.scale(),
            a.1 + (dx / len) * off / fit.scale(),
        );
        fit.px(p)
    };
    for o in &d.layout.objects {
        let (x, y) = side(o.seg as usize, o.x());
        let ink = if o.kind == ObjectKind::Rock {
            BROWN
        } else {
            WHITE
        };
        c.put(x, y, ink);
    }
    for car in &d.layout.traffic {
        let seg = ((car.z0_dm / 40) as usize).min(n - 1);
        let (x, y) = side(seg, car.x() + 0.01);
        c.rect(x - 1, y - 1, 2, 2, WATER);
    }
    let gates = &d.layout.checkpoints;
    for (k, &g) in gates.iter().enumerate() {
        let (x, y) = px[g as usize];
        if k + 1 == gates.len() {
            for (dx, dy) in [
                (-2, -2),
                (0, -2),
                (-1, -1),
                (1, -1),
                (-2, 0),
                (0, 0),
                (-1, 1),
                (1, 1),
            ] {
                c.put(x + dx, y + dy, WHITE);
            }
        } else {
            c.rect(x - 2, y - 1, 5, 3, WHITE);
            c.rect(x - 1, y, 3, 1, INK);
        }
    }
    let (x0, y0) = px[0];
    c.rect_outline(x0 - 2, y0 - 2, 5, 5, WHITE);
    push(&mut out, m, Act::Map);

    // Over the map: what the proof found, and what would stop one.
    let mut lines: Vec<(String, u8)> = Vec::new();
    match &ed.prover.state {
        ProofState::Failed(w) => lines.extend(w.iter().map(|s| (s.clone(), RED))),
        ProofState::Proven(s) => lines.push((s.line(), GRASS_LT)),
        _ => {}
    }
    let notes = super::proof::notes(&d.layout);
    if let Some(first) = notes.first() {
        lines.push((first.clone(), YELLOW));
        if notes.len() > 1 {
            lines.push((format!("AND {} MORE", notes.len() - 1), YELLOW));
        }
    }
    for (k, (s, ink)) in lines.iter().enumerate() {
        tag(c, m.x + 3, m.y + 3 + k as i32 * 8, s, *ink);
    }
    tag(
        c,
        m.x + 3,
        m.y + m.h - 9,
        &format!(
            "{:.2} KM  {} GATES",
            d.layout.length() / 1000.0,
            gates.len()
        ),
        ASPHALT_LT,
    );

    // The top bar.
    c.rect(e.top.x, e.top.y, e.top.w, e.top.h, INK);
    c.text(3, 4, &title(ed), WHITE, 1);
    let (b, ink) = badge(ed, clock);
    let bw = font::small_width("VERIFYING...") as i32;
    c.text(W - 3 - bw, 4, &b, ink, 1);

    // The elevation strip.
    let s = e.strip;
    c.rect(s.x, s.y, s.w, s.h, INK);
    let xa = view::strip_x(s, sa, n);
    let xb = view::strip_x(s, sb, n);
    c.rect(xa, s.y, (xb - xa).max(1), s.h, ASPHALT_DK);
    let hs = view::heights(segs);
    let (lo, hi) = hs
        .iter()
        .fold((f64::MAX, f64::MIN), |a, &h| (a.0.min(h), a.1.max(h)));
    let span = (hi - lo).max(4.0);
    let mid = (hi + lo) / 2.0;
    let y_of = |h: f64| -> i32 {
        let f = (h - mid) / span;
        s.y + s.h / 2 - (f * (s.h - 6) as f64).round() as i32
    };
    for x in s.x + 2..s.x + s.w - 2 {
        let seg = view::strip_pick(s, n, x as f32);
        let y = y_of(hs[seg]);
        let ink = if (sa..sb).contains(&seg) {
            YELLOW
        } else {
            HILLS
        };
        c.rect(
            x,
            y,
            1,
            s.y + s.h - y,
            if ink == YELLOW { GRAVEL } else { ASPHALT_DK },
        );
        c.put(x, y, ink);
    }
    for &g in gates {
        let x = view::strip_x(s, g as usize, n);
        c.rect(x, s.y, 1, s.h, WHITE);
    }
    push(&mut out, s, Act::Strip);

    // The info row.
    let i = e.info;
    c.rect(i.x, i.y, i.w, i.h, INK);
    key(c, &mut out, e.prev, "<", Act::PrevRun, false, ed.sel == 0);
    key(
        c,
        &mut out,
        e.next,
        ">",
        Act::NextRun,
        false,
        ed.sel + 1 == d.run_count(),
    );
    let info = format!(
        "RUN {}/{}  {}-{} M",
        ed.sel + 1,
        d.run_count(),
        metres(sa),
        metres(sb)
    );
    c.text_centred(W / 2, i.y + 3, &info, WHITE, 1);
    let extra = format!(
        "{} OBJ  {} CARS  {} GATE{}",
        d.objects_on(ed.sel),
        d.cars_on(ed.sel),
        d.gates_on(ed.sel).len(),
        if d.gates_on(ed.sel).len() == 1 {
            ""
        } else {
            "S"
        }
    );
    c.text_centred(W / 2, i.y + 10, &extra, ASPHALT_LT, 1);

    // Tools.
    for (k, r) in e.tools.iter().enumerate() {
        match Tool::ALL.get(k) {
            Some(&t) => key(
                c,
                &mut out,
                *r,
                t.label(),
                Act::Tool(t),
                ed.tool == t,
                false,
            ),
            None => key(c, &mut out, *r, "MENU", Act::Menu, false, false),
        }
    }

    // The tool's options.
    let o = e.opts;
    c.rect(o.x, o.y, o.w, o.h, INK);
    let opts = e.options(ed.tool);
    if ed.tool.stepper() {
        for &(r, a) in &opts {
            big_key(
                c,
                &mut out,
                r,
                if a == Act::Plus { "+" } else { "-" },
                a,
                false,
            );
        }
        let value = match ed.tool {
            Tool::Bend => match d.bend(ed.sel) {
                0 => "STRAIGHT".to_string(),
                v if v > 0 => format!("RIGHT {v}"),
                v => format!("LEFT {}", -v),
            },
            Tool::Hill => match d.hill(ed.sel) {
                0 => "FLAT".to_string(),
                v if v > 0 => format!("CREST {}%", v / 10),
                v => format!("DIP {}%", -v / 10),
            },
            _ => {
                let w = d.width(ed.sel) as u32 * 2;
                format!("{}.{} M WIDE", w / 100, w % 100 / 10)
            }
        };
        c.text_centred(W / 2, o.y + o.h / 2 - 5, &value, YELLOW, 2);
    } else {
        let current = match ed.tool {
            Tool::Surface => Surface::ALL.iter().position(|s| *s == d.surface(ed.sel)),
            Tool::Drift => Some(match d.drift(ed.sel) {
                DriftMark::None => 0,
                DriftMark::Hint => 1,
                DriftMark::Required => 2,
            }),
            _ => None,
        };
        for (k, &(r, a)) in opts.iter().enumerate() {
            let mut label = ed.tool.options()[k].to_string();
            if ed.tool == Tool::Gate && k == 0 {
                label = if d.gates_on(ed.sel).is_empty() {
                    "ADD GATE".into()
                } else {
                    "DEL GATE".into()
                };
            }
            key(c, &mut out, r, &label, a, current == Some(k), false);
            if ed.focus == k {
                c.rect_outline(r.x, r.y, r.w, r.h, WHITE);
            }
        }
    }

    // Actions.
    key(
        c,
        &mut out,
        e.undo,
        "UNDO",
        Act::Undo,
        false,
        !ed.history.can_undo(),
    );
    key(
        c,
        &mut out,
        e.redo,
        "REDO",
        Act::Redo,
        false,
        !ed.history.can_redo(),
    );
    key(c, &mut out, e.test, "TEST", Act::Test, false, false);
    let proven = matches!(ed.prover.state, ProofState::Proven(_));
    key(
        c,
        &mut out,
        e.prove,
        if proven { "PROVEN" } else { "PROVE" },
        Act::Prove,
        ed.prover.busy(),
        proven,
    );
    if ed.note().is_some() {
        note_line(ed, c, m.y + m.h - 20);
    }
    out
}

/// A page's header: a banner and the draft's title.
fn header(c: &mut Canvas, text: &str, sub: &str) {
    c.text_centred(W / 2 + 1, 7, text, INK, 2);
    c.text_centred(W / 2, 6, text, YELLOW, 2);
    c.text_centred(W / 2, 20, sub, ASPHALT_LT, 1);
}

/// BACK at the bottom of a page.
fn back(c: &mut Canvas, out: &mut Vec<Button>, l: &Layout) -> Rect {
    let r = Rect::new(6, l.bottom() - 30, W - 12, 26);
    big_key(c, out, r, "BACK", Act::Back, false);
    r
}

fn menu(ed: &Editor, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    let (b, _) = badge(ed, 0.0);
    header(c, "EDITOR", &format!("{}  {}", title(ed), b));
    // Down in the thumb zone, above BACK.
    let rows = Item::ALL.len().div_ceil(2) as i32;
    let top = (l.bottom() - 36 - rows * 33).max(44);
    for (k, item) in Item::ALL.iter().enumerate() {
        let (col, row) = (k as i32 % 2, k as i32 / 2);
        let r = Rect::new(4 + col * 87, top + row * 33, 85, 30);
        let dim = match item {
            Item::Prove => matches!(ed.prover.state, ProofState::Proven(_)) || ed.prover.busy(),
            _ => false,
        };
        plate(c, r, false);
        if ed.focus == k {
            c.rect_outline(r.x, r.y, r.w, r.h, YELLOW);
        }
        let ink = if dim { ASPHALT_LT } else { WHITE };
        if *item == Item::Biome {
            // The label, and the biome it is on, which a tap moves on.
            c.text_centred(r.x + r.w / 2, r.y + 7, item.label(), ink, 1);
            c.text_centred(
                r.x + r.w / 2,
                r.y + 17,
                ed.draft.layout.biome.name(),
                ICE,
                1,
            );
        } else {
            c.text_centred(r.x + r.w / 2, r.y + r.h / 2 - 2, item.label(), ink, 1);
        }
        push(&mut out, r, Act::Item(*item));
    }
    let hint = if ed.timed() {
        "PROVEN: COPY GIVES A SHARE CODE"
    } else {
        "UNPROVEN: FREE DRIVE, DRAFT CODE"
    };
    c.text_centred(W / 2, top - 12, hint, ASPHALT_LT, 1);
    back(c, &mut out, l);
    note_line(ed, c, 32);
    out
}

fn save(ed: &Editor, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    header(c, "SAVE", &title(ed));
    let top = (l.bottom() - 36 - 6 * 30).max(32);
    for n in 1..=MAX_SLOTS {
        let k = n as i32 - 1;
        let r = Rect::new(3 + (k % 4) * 44, top + (k / 4) * 30, 42, 28);
        let slot = ed.slots.get(n);
        let editable = ed.slots.editable(n);
        let mine = ed.slot == Some(n);
        let armed = ed.armed == Some(super::Armed::Overwrite(n));
        plate(c, r, armed);
        let outline = if mine {
            GRASS_LT
        } else if ed.focus == k as usize {
            YELLOW
        } else {
            ASPHALT_LT
        };
        c.rect_outline(r.x, r.y, r.w, r.h, outline);
        c.text(r.x + 3, r.y + 3, &n.to_string(), WHITE, 1);
        let label = match slot {
            None if ed.slots.taken(n) => "KEPT".to_string(),
            None => "EMPTY".into(),
            Some(s) => match &s.layout {
                None => "NEWER".into(),
                Some(l) if l.name.is_empty() => "UNNAMED".into(),
                Some(l) => l.name.chars().take(9).collect(),
            },
        };
        c.text(
            r.x + 3,
            r.y + 14,
            &label,
            if editable { WHITE } else { ASPHALT_LT },
            1,
        );
        push(&mut out, r, Act::Slot(n as u8));
    }
    back(c, &mut out, l);
    note_line(ed, c, 32);
    out
}

/// The keys of the NAME keypad: letters, digits, space; then delete, done.
pub const NAME_KEYS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 \x08\x0d";

fn name(ed: &Editor, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    header(c, "NAME", "12 LETTERS AND DIGITS");
    let field = Rect::new(10, 32, W - 20, 22);
    c.rect(field.x, field.y, field.w, field.h, ASPHALT_DK);
    c.rect_outline(field.x, field.y, field.w, field.h, ASPHALT_LT);
    let shown = format!("{}_", ed.name_buf);
    c.text_centred(W / 2, field.y + 6, &shown, YELLOW, 2);
    let top = (l.bottom() - 36 - 6 * 25).max(60);
    for (k, &ch) in NAME_KEYS.iter().enumerate() {
        let (col, row) = (k as i32 % 7, k as i32 / 7);
        let r = Rect::new(3 + col * 25, top + row * 25, 24, 23);
        let label = match ch {
            b' ' => "SPC".to_string(),
            8 => "DEL".into(),
            13 => "OK".into(),
            c => (c as char).to_string(),
        };
        key(c, &mut out, r, &label, Act::Key(ch), ch == 13, false);
    }
    back(c, &mut out, l);
    out
}

fn seed(ed: &Editor, c: &mut Canvas, l: &Layout, clock: f64) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    header(c, "FROM SEED", "FORK A TRACK OF ANY RUN");
    let field = Rect::new(6, 32, W - 12, 20);
    c.rect(field.x, field.y, field.w, field.h, ASPHALT_DK);
    c.rect_outline(field.x, field.y, field.w, field.h, ASPHALT_LT);
    c.text(field.x + 4, field.y + 2, "SEED", ASPHALT_LT, 1);
    let shown = if ed.seed_buf.is_empty() {
        "_".to_string()
    } else {
        ed.seed_buf.clone()
    };
    c.text_big(field.x + 4, field.y + 10, &shown, YELLOW);
    // Track and road, then the keypad, down in the thumb zone.
    let row = (l.bottom() - 36 - 30 - 4 * 30).max(56);
    let minus = Rect::new(6, row, 30, 24);
    let plus = Rect::new(84, row, 30, 24);
    big_key(c, &mut out, minus, "-", Act::Index(-1), false);
    big_key(c, &mut out, plus, "+", Act::Index(1), false);
    c.text_centred(60, row + 3, "TRACK", ASPHALT_LT, 1);
    c.text_centred(60, row + 11, &format!("{}", ed.seed_index + 1), WHITE, 2);
    let road = Rect::new(118, row, W - 124, 24);
    let label = match (ed.seed_index, ed.seed_branch) {
        (0, _) => "FIRST",
        (_, 0) => "EASY",
        _ => "HARD",
    };
    key(
        c,
        &mut out,
        road,
        label,
        Act::Branch,
        false,
        ed.seed_index == 0,
    );
    // The keypad.
    let keys: [(u8, &str); 12] = [
        (b'1', "1"),
        (b'2', "2"),
        (b'3', "3"),
        (b'4', "4"),
        (b'5', "5"),
        (b'6', "6"),
        (b'7', "7"),
        (b'8', "8"),
        (b'9', "9"),
        (8, "DEL"),
        (b'0', "0"),
        (13, "FORK"),
    ];
    let top = row + 30;
    for (k, (ch, label)) in keys.iter().enumerate() {
        let (col, rr) = (k as i32 % 3, k as i32 / 3);
        let r = Rect::new(6 + col * 57, top + rr * 30, 54, 28);
        big_key(c, &mut out, r, label, Act::Key(*ch), *ch == 13);
    }
    if ed.pending_fork.is_some() {
        let dots = ".".repeat((clock * 3.0) as usize % 4);
        c.text_centred(W / 2, 58, &format!("BUILDING TRACK{dots}"), YELLOW, 1);
    }
    back(c, &mut out, l);
    note_line(ed, c, 70);
    out
}

/// A listed layout's badge.
fn entry_badge(ed: &Editor, e: Entry) -> (&'static str, u8) {
    if ed.checking(e) {
        return ("CHECKING", YELLOW);
    }
    if ed.failed.contains(&e) {
        return ("FAILS", RED);
    }
    match ed.entry_layout(e).map(|l| l.standing()) {
        None => ("NEWER", ASPHALT_LT),
        Some(Standing::Proven) => ("PROVEN", GRASS_LT),
        Some(Standing::Unproven) => ("FREE", WHITE),
        Some(_) => ("CHECK", YELLOW),
    }
}

fn tracks(ed: &Editor, c: &mut Canvas, l: &Layout) -> Vec<Button> {
    c.fill(INK);
    let mut out = Vec::new();
    header(c, "TRACKS", "COURSES AND YOUR SAVED LAYOUTS");
    let entries = ed.entries();
    let pages = entries.len().div_ceil(LIST_ROWS).max(1);
    let top = 30;
    for row in 0..LIST_ROWS {
        let at = ed.list_page * LIST_ROWS + row;
        let Some(&e) = entries.get(at) else { break };
        let r = Rect::new(4, top + row as i32 * 20, W - 8, 19);
        let hot = at == ed.list_sel;
        c.rect(r.x, r.y, r.w, r.h, if hot { ASPHALT_DK } else { INK });
        c.rect_outline(r.x, r.y, r.w, r.h, if hot { YELLOW } else { ASPHALT_DK });
        let (tag_text, name) = match (e, ed.entry_layout(e)) {
            (Entry::Course(i), Some(l)) => (format!("C{}", i + 1), l.name.clone()),
            (Entry::Slot(n), Some(l)) if l.name.is_empty() => (format!("{n}"), "UNNAMED".into()),
            (Entry::Slot(n), Some(l)) => (format!("{n}"), l.name.clone()),
            (Entry::Slot(n), None) => (format!("{n}"), "UNREADABLE".into()),
            (Entry::Course(i), None) => (format!("C{}", i + 1), String::new()),
        };
        c.text(r.x + 3, r.y + 3, &tag_text, ASPHALT_LT, 1);
        c.text(r.x + 18, r.y + 3, &name, WHITE, 1);
        if let Some(l) = ed.entry_layout(e) {
            let km = format!("{:.1} KM", l.length() / 1000.0);
            c.text(r.x + 18, r.y + 11, &km, ASPHALT_LT, 1);
        }
        let (b, ink) = entry_badge(ed, e);
        let bw = font::small_width(b) as i32;
        c.text(r.x + r.w - bw - 3, r.y + 7, b, ink, 1);
        push(&mut out, r, Act::Row(row as u8));
    }
    if pages > 1 {
        let up = Rect::new(4, top + LIST_ROWS as i32 * 20 + 1, 40, 16);
        let down = Rect::new(W - 44, up.y, 40, 16);
        key(
            c,
            &mut out,
            up,
            "UP",
            Act::Page(-1),
            false,
            ed.list_page == 0,
        );
        key(
            c,
            &mut out,
            down,
            "DOWN",
            Act::Page(1),
            false,
            ed.list_page + 1 == pages,
        );
        c.text_centred(
            W / 2,
            up.y + 5,
            &format!("{}/{}", ed.list_page + 1, pages),
            ASPHALT_LT,
            1,
        );
    }
    // Acting on the selected row, then the rest.
    let b = l.bottom();
    let row1 = Rect::new(0, b - 58, W, 26);
    let row2 = Rect::new(0, b - 30, W, 26);
    let deletable = matches!(entries.get(ed.list_sel), Some(Entry::Slot(_)));
    let armed = matches!(ed.armed, Some(super::Armed::Delete(_)));
    let acts: [(&str, Item, bool); 3] = [
        ("RACE", Item::TestDrive, false),
        ("EDIT", Item::Name, false),
        ("DELETE", Item::Discard, !deletable),
    ];
    for (k, (label, item, dim)) in acts.iter().enumerate() {
        let r = Rect::new(4 + k as i32 * 58, row1.y, 56, row1.h);
        key(
            c,
            &mut out,
            r,
            label,
            Act::Item(*item),
            *item == Item::Discard && armed,
            *dim,
        );
    }
    let more: [(&str, Act); 4] = [
        ("EDITOR", Act::Item(Item::Leave)),
        ("PASTE", Act::Item(Item::Paste)),
        ("SEED", Act::Item(Item::Seed)),
        ("BACK", Act::Back),
    ];
    for (k, (label, a)) in more.iter().enumerate() {
        let r = Rect::new(k as i32 * 45, row2.y, 45, row2.h);
        key(c, &mut out, r, label, *a, false, false);
    }
    note_line(ed, c, row1.y - 12);
    out
}
