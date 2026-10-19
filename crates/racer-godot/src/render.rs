//! A race painted into the canvas: sky, parallax hills, the road row by
//! row, roadside things and traffic back to front, the player's car, the
//! HUD strip and the control pad. Engine-free; `app` adds the menus.
//!
//! The road comes from [`RoadView`]: each row knows its segment, its
//! centre and its half width, so a row is a few spans. Kerbs, lane dashes,
//! grass and tarmac alternate every [`STRIPE_SEGS`] segments, which is
//! what makes speed readable. Sprites are drawn far to near, each from its
//! nearest pre-drawn size and clipped at the hilltop its segment was drawn
//! under (DESIGN §5).

use racer_sim::car::{Car, Drift, MINI_CHARGE, SUPER_CHARGE};
use racer_sim::input::Input;
use racer_sim::race::Race;
use racer_sim::rng::mix64;
use racer_sim::track::{flags, ObjectKind, Surface, Track, SEG_LEN, VERGE};

use crate::layout::{Layout, Rect};
use crate::palette::*;
use crate::project::{half_width, Camera, Placed, RoadView, Row, STRIPE_SEGS};
use crate::raster::{Canvas, Sprite};
use crate::sprites::{Art, CarFrame, Steps, CAR_H, CAR_W};

/// How far ahead a drift-required corner is announced, metres.
pub const DRIFT_WARN: f64 = 150.0;

/// What the renderer needs of a race. Built from a live [`Race`] with
/// [`RaceScene::of`], or by hand in a test.
#[derive(Clone, Copy, Debug)]
pub struct RaceScene<'a> {
    /// The track.
    pub track: &'a Track,
    /// The car.
    pub car: Car,
    /// Race tick (traffic positions follow it).
    pub tick: u32,
    /// Ticks left on the timer.
    pub timer: u32,
    /// The next checkpoint to cross.
    pub next_gate: usize,
    /// This tick's input, for the car's lean.
    pub input: Input,
    /// Track number in the run, from 0.
    pub index: u32,
    /// Tracks in the run.
    pub tracks: u32,
    /// The background's sideways scroll, whole pixels.
    pub bg_shift: i32,
    /// The ghost of the player's best run here, while it is still driving:
    /// its `(z, x)`, metres.
    pub ghost: Option<(f64, f64)>,
    /// The player's time against the ghost's at the last gate, ticks
    /// (negative is ahead), while it is shown.
    pub ghost_note: Option<i64>,
}

impl<'a> RaceScene<'a> {
    /// The scene of a live race.
    pub fn of(race: &'a Race, index: u32, tracks: u32, bg_shift: i32) -> Self {
        RaceScene {
            track: race.track(),
            car: *race.car(),
            tick: race.tick(),
            timer: race.timer(),
            next_gate: race.next_gate(),
            input: race.input(),
            index,
            tracks,
            bg_shift,
            ghost: None,
            ghost_note: None,
        }
    }

    /// Metres to the next drift-required corner within [`DRIFT_WARN`], if
    /// any. Inside one it is 0.
    pub fn drift_ahead(&self) -> Option<f64> {
        let segs = self.track.segments();
        let from = self.track.index_at(self.car.z);
        let to = (from + (DRIFT_WARN / SEG_LEN) as usize).min(segs.len());
        (from..to)
            .find(|&i| segs[i].has(flags::DRIFT_REQ))
            .map(|i| (i as f64 * SEG_LEN - self.car.z).max(0.0))
    }
}

/// Which pad buttons are held, for drawing them pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// Steer left.
    pub left: bool,
    /// Steer right.
    pub right: bool,
    /// Brake.
    pub brake: bool,
    /// Drift.
    pub drift: bool,
}

/// Roadside decoration that is not in the sim: trees, and the warning
/// boards before drift corners. Presentation only, placed from a hash of
/// the segment, beyond the barrier where nothing can hit it.
#[derive(Clone, Debug, Default)]
pub struct Decor {
    items: Vec<DecorItem>,
}

/// One piece of decor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecorItem {
    /// Segment it stands on (at the segment's start).
    pub seg: u32,
    /// Offset from the centre line, metres.
    pub x: f64,
    /// What it is.
    pub kind: DecorKind,
}

/// Kinds of decor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorKind {
    /// A tree.
    Tree,
    /// A bend board pointing right.
    ChevronRight,
    /// A bend board pointing left.
    ChevronLeft,
}

/// Segments of warning boards before a drift-required corner.
pub const SIGN_LEAD: usize = 15;

impl Decor {
    /// The decor of `track`.
    pub fn new(track: &Track) -> Decor {
        let segs = track.segments();
        let mut items = Vec::new();
        // Boards along every drift corner and its approach, on the outside.
        let mut i = 0;
        while i < segs.len() {
            let flag = if segs[i].has(flags::DRIFT_REQ) {
                flags::DRIFT_REQ
            } else if segs[i].has(flags::DRIFT_HINT) {
                flags::DRIFT_HINT
            } else {
                i += 1;
                continue;
            };
            let start = i;
            let mut turn = 0i64;
            while i < segs.len() && segs[i].has(flag) {
                turn += segs[i].curve_milli as i64;
                i += 1;
            }
            let (lead, step) = if flag == flags::DRIFT_REQ {
                (SIGN_LEAD, 3)
            } else {
                (0, 5)
            };
            let right = turn >= 0;
            for s in (start.saturating_sub(lead)..i).step_by(step) {
                let hw = segs[s].half_width();
                items.push(DecorItem {
                    seg: s as u32,
                    x: if right { -(hw + 2.0) } else { hw + 2.0 },
                    kind: if right {
                        DecorKind::ChevronRight
                    } else {
                        DecorKind::ChevronLeft
                    },
                });
            }
        }
        for (s, seg) in segs.iter().enumerate() {
            let h = mix64(s as u64 ^ 0x7EE5_0000);
            if h.is_multiple_of(5) {
                let side = if (h >> 8) & 1 == 0 { -1.0 } else { 1.0 };
                let d = seg.half_width() + VERGE + 2.5 + ((h >> 16) % 80) as f64 / 10.0;
                items.push(DecorItem {
                    seg: s as u32,
                    x: side * d,
                    kind: DecorKind::Tree,
                });
            }
        }
        items.sort_by_key(|d| d.seg);
        Decor { items }
    }

    /// Decor on segments `from..to`.
    pub fn between(&self, from: i64, to: i64) -> &[DecorItem] {
        let a = self.items.partition_point(|d| (d.seg as i64) < from);
        let b = self.items.partition_point(|d| (d.seg as i64) < to);
        &self.items[a..b.max(a)]
    }

    /// Every item.
    pub fn items(&self) -> &[DecorItem] {
        &self.items
    }
}

// ------------------------------------------------------------------ the road

/// A road surface's two stripe bands: `(a, b, sixteenths of b)` each.
fn surface_ink(s: Surface, light: bool) -> (u8, u8, u8) {
    match (s, light) {
        (Surface::Asphalt, true) => (ASPHALT_LT, ASPHALT_LT, 0),
        (Surface::Asphalt, false) => (ASPHALT_DK, ASPHALT_DK, 0),
        (Surface::Gravel, true) => (GRAVEL, GRAVEL, 0),
        (Surface::Gravel, false) => (GRAVEL, BROWN, 5),
        (Surface::Ice, true) => (ICE, ICE, 0),
        (Surface::Ice, false) => (ICE, WHITE, 8),
        (Surface::Puddle, true) => (WATER, WATER, 0),
        (Surface::Puddle, false) => (WATER, ASPHALT_DK, 6),
    }
}

/// Height of the far hills at background column `x`, pixels.
fn hill_height(x: i32) -> i32 {
    let tri = |x: i32, p: i32| -> i32 {
        let m = x.rem_euclid(p);
        (p / 2 - (m - p / 2).abs()) * 2 * 64 / p
    };
    4 + (tri(x, 157) * 14 + tri(x + 40, 61) * 7 + tri(x + 13, 23) * 2) / 64
}

/// Height of the near, green hills at background column `x`, pixels: lower
/// and rounder than the far range, so the two read as separate distances.
fn near_hill_height(x: i32) -> i32 {
    let bump = |x: i32, p: i32| -> i32 {
        // A parabola per period: rounded tops, sharp valleys.
        let m = x.rem_euclid(p) - p / 2;
        (p * p / 4 - m * m) * 64 / (p * p / 4)
    };
    1 + (bump(x, 97) * 5 + bump(x + 31, 53) * 3) / 64
}

/// One cloud: a few overlapping ellipses, in sky coordinates where x is a
/// background column (repeating every [`CLOUD_PERIOD`]) and y is a fraction
/// of the sky's height in 1/64ths.
struct Cloud {
    /// Centres and radii `(cx, cy64, rx, ry)`: the first is the flat base,
    /// the rest the puffs on top.
    blobs: &'static [(i32, i32, i32, i32)],
}

/// Background columns before the cloud pattern repeats.
const CLOUD_PERIOD: i32 = 320;

/// The sky's clouds. Few and high, so the road keeps the eye; their bases
/// are shaded pale blue (ICE), the classic flat-bottomed cumulus.
const CLOUDS: &[Cloud] = &[
    Cloud {
        blobs: &[(30, 14, 22, 5), (22, 11, 9, 6), (36, 10, 11, 8)],
    },
    Cloud {
        blobs: &[(118, 24, 16, 4), (112, 21, 7, 5), (124, 20, 8, 6)],
    },
    Cloud {
        blobs: &[
            (196, 9, 26, 5),
            (186, 6, 10, 7),
            (202, 5, 12, 9),
            (214, 8, 8, 5),
        ],
    },
    Cloud {
        blobs: &[(272, 30, 12, 3), (270, 28, 6, 4)],
    },
];

/// The blobs that reach sky row `y` of a sky `sky_h` rows tall, with their
/// centres in rows: `(cx, cy, rx, ry)`. Most rows have none, so a frame
/// tests each sky pixel against a handful of blobs at most.
fn cloud_row(y: i32, sky_h: i32) -> Vec<(i32, i32, i32, i32)> {
    CLOUDS
        .iter()
        .flat_map(|c| c.blobs.iter())
        .map(|&(cx, cy64, rx, ry)| (cx, cy64 * sky_h / 64, rx, ry))
        .filter(|&(_, cy, _, ry)| (y - cy).abs() <= ry)
        .collect()
}

/// The cloud colour at background column `bx` in sky row `y`, given that
/// row's blobs: WHITE, or ICE in a blob's lower third.
fn cloud_at(bx: i32, y: i32, blobs: &[(i32, i32, i32, i32)]) -> Option<u8> {
    let x = bx.rem_euclid(CLOUD_PERIOD);
    let mut hit = None;
    for &(cx, cy, rx, ry) in blobs {
        // Wrap so a cloud straddling the period's edge stays whole.
        let dx = (x - cx + CLOUD_PERIOD / 2).rem_euclid(CLOUD_PERIOD) - CLOUD_PERIOD / 2;
        let dy = y - cy;
        if dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry {
            let shade = dy * 3 > ry;
            // White wins: a puff's body covers the base's shadow.
            hit = match (hit, shade) {
                (Some(WHITE), _) | (_, false) => Some(WHITE),
                _ => Some(ICE),
            };
        }
    }
    hit
}

/// Sky, clouds, the far and near hills, then the road row by row. Each
/// layer scrolls with its own share of `bg_shift` (parallax): clouds a
/// third, the far range all of it, the near hills twice it.
fn road_rows(c: &mut Canvas, v: &RoadView, track: &Track, bg_shift: i32, gates: &[u32]) {
    let w = v.w;
    for y in 0..v.h {
        match v.rows[y as usize] {
            Some(r) => road_row(c, y, w, &r, track, gates),
            None if y >= v.horizon => c.span(0, w, y, GRASS_DK),
            None => {
                let frac = ((y * 16) / v.horizon.max(1)).clamp(0, 16) as u8;
                c.dither_span(0, w, y, SKY_HI, SKY_LO, frac);
                let blobs = cloud_row(y, v.horizon);
                if !blobs.is_empty() {
                    for x in 0..w {
                        if let Some(k) = cloud_at(x + bg_shift.div_euclid(3), y, &blobs) {
                            c.put(x, y, k);
                        }
                    }
                }
            }
        }
    }
    // Hills on the horizon, under any road that rises above it: the far
    // range, then the near green hills in front of it.
    for (height, shift, colour) in [
        (hill_height as fn(i32) -> i32, bg_shift, HILLS),
        (near_hill_height as fn(i32) -> i32, bg_shift * 2, GRASS_DK),
    ] {
        for x in 0..w {
            let top = v.horizon - height(x + shift);
            for y in top.max(0)..v.horizon.min(v.h) {
                if v.rows[y as usize].is_none() {
                    c.put(x, y, colour);
                }
            }
        }
    }
}

fn road_row(c: &mut Canvas, y: i32, w: i32, r: &Row, track: &Track, gates: &[u32]) {
    let light = r.seg.div_euclid(STRIPE_SEGS) & 1 == 0;
    let seg =
        (r.seg >= 0 && (r.seg as usize) < track.len()).then(|| track.segments()[r.seg as usize]);
    let surface = seg.map_or(Surface::Asphalt, |s| s.surface);
    let drift = seg.is_some_and(|s| s.has(flags::DRIFT_REQ));
    let s = r.scale_x256 as f64 / 256.0;
    let (l, rt) = (r.cx - r.half, r.cx + r.half);
    c.span(0, w, y, if light { GRASS_LT } else { GRASS_DK });
    let kerb = ((0.8 * s).round() as i32).max(1);
    let kerb_c = match (light, drift) {
        (true, _) => RED,
        (false, true) => YELLOW,
        (false, false) => WHITE,
    };
    c.span(l - kerb, l, y, kerb_c);
    c.span(rt, rt + kerb, y, kerb_c);
    let (a, b, f) = surface_ink(surface, light);
    if f == 0 {
        c.span(l, rt, y, a);
    } else {
        c.dither_span(l, rt, y, a, b, f);
    }
    let gate = gates.iter().position(|&g| g as i64 == r.seg);
    if let Some(k) = gate {
        if k + 1 == gates.len() {
            // The finish: a chequered line.
            for x in l.max(0)..rt.min(w) {
                c.put(
                    x,
                    y,
                    if ((x - l) / 2 + y) & 1 == 0 {
                        WHITE
                    } else {
                        INK
                    },
                );
            }
        } else {
            c.span(l, rt, y, WHITE);
        }
        return;
    }
    if light {
        let hw = seg.map_or(6.0, |s| s.half_width());
        let lanes = if hw >= 5.5 { 3 } else { 2 };
        let dash = ((0.2 * s).round() as i32).max(1);
        for k in 1..lanes {
            let xm = -hw + 2.0 * hw * k as f64 / lanes as f64;
            let x = r.cx + (xm * s).round() as i32;
            c.span(x - dash / 2, x - dash / 2 + dash, y, WHITE);
        }
    }
}

/// Something to draw on the road, with its depth for sorting.
enum Thing<'a> {
    Sprite {
        sprite: &'a Sprite,
        at: Placed,
        flip: bool,
    },
    Gate {
        at: Placed,
        hw: f64,
        finish: bool,
    },
}

impl Thing<'_> {
    fn dz(&self) -> f64 {
        match self {
            Thing::Sprite { at, .. } | Thing::Gate { at, .. } => at.dz,
        }
    }
}

fn stand<'a>(out: &mut Vec<Thing<'a>>, steps: &'a Steps, at: Option<Placed>, flip: bool) {
    if let Some(at) = at {
        if let Some(sprite) = steps.pick(at.scale) {
            out.push(Thing::Sprite { sprite, at, flip });
        }
    }
}

/// Paint the road view (`l.road`) and return its projection.
pub fn road(c: &mut Canvas, l: &Layout, art: &Art, decor: &Decor, s: &RaceScene) -> RoadView {
    let v = RoadView::build(
        s.track,
        s.car.z,
        s.car.x,
        l.road.w,
        l.road.h,
        Camera::DEFAULT,
    );
    road_rows(c, &v, s.track, s.bg_shift, s.track.checkpoints());
    let (from, to) = (v.base, v.base + v.cam.draw_segs as i64);
    let mut things: Vec<Thing> = Vec::new();
    for d in decor.between(from, to) {
        let at = v.place(d.seg as f64 * SEG_LEN + 0.01, d.x);
        match d.kind {
            DecorKind::Tree => stand(&mut things, &art.tree, at, d.x < 0.0),
            DecorKind::ChevronRight => stand(&mut things, &art.chevron, at, false),
            DecorKind::ChevronLeft => stand(&mut things, &art.chevron, at, true),
        }
    }
    let (fa, ta) = (from.max(0) as usize, to.max(0) as usize);
    for o in s.track.objects_in(fa, ta) {
        let steps = match o.kind {
            ObjectKind::Post => &art.post,
            ObjectKind::Rock => &art.rock,
        };
        stand(&mut things, steps, v.place(o.z(), o.x()), false);
    }
    for t in s.track.traffic() {
        stand(
            &mut things,
            &art.traffic,
            v.place(t.z_at(s.tick), t.x()),
            false,
        );
    }
    // The ghost is one more thing on the road, sorted by depth with the
    // rest and clipped by crests like them. Behind the camera it has no
    // place; under the player's car it is drawn first and covered.
    if let Some((z, x)) = s.ghost {
        stand(&mut things, &art.ghost, v.place(z, x), false);
    }
    let gates = s.track.checkpoints();
    for (k, &g) in gates.iter().enumerate().skip(s.next_gate) {
        if let Some(at) = v.place(g as f64 * SEG_LEN + 0.01, 0.0) {
            things.push(Thing::Gate {
                at,
                hw: half_width(s.track, g as i64),
                finish: k + 1 == gates.len(),
            });
        }
    }
    things.sort_by(|a, b| b.dz().total_cmp(&a.dz()));
    let bottom = l.road.y + l.road.h;
    for t in &things {
        match *t {
            Thing::Sprite { sprite, at, flip } => {
                let y_max = at.clip.min(bottom);
                c.blit(sprite, at.x - sprite.w / 2, at.y - sprite.h, y_max, flip);
            }
            Thing::Gate { at, hw, finish } => gate(c, &at, hw, finish, bottom),
        }
    }
    player(c, &v, s, art, bottom);
    v
}

/// A checkpoint gate: two posts and a banner over the road.
fn gate(c: &mut Canvas, at: &Placed, hw: f64, finish: bool, bottom: i32) {
    let s = at.scale;
    let y_max = at.clip.min(bottom);
    let reach = ((hw + 0.8) * s).round() as i32;
    let post = ((0.4 * s).round() as i32).max(1);
    let (top, band) = ((5.5 * s).round() as i32, ((1.2 * s).round() as i32).max(1));
    for y in (at.y - top).max(0)..at.y.min(y_max) {
        c.span(at.x - reach - post, at.x - reach, y, WHITE);
        c.span(at.x + reach, at.x + reach + post, y, WHITE);
    }
    for y in (at.y - top).max(0)..(at.y - top + band).min(y_max) {
        for x in at.x - reach - post..at.x + reach + post {
            let cell = ((x - at.x).div_euclid(band.max(2) / 2 + 1)
                + (y - at.y + top) / (band / 2 + 1))
                & 1;
            let ink = match (finish, cell == 0) {
                (true, true) => WHITE,
                (true, false) => INK,
                (false, true) => YELLOW,
                (false, false) => INK,
            };
            c.put(x, y, ink);
        }
    }
}

/// Which frame the car is drawn with.
pub fn car_frame(car: &Car, input: Input, tick: u32) -> CarFrame {
    let steer = input.steer_level();
    let side = if steer != 0 {
        steer.signum() as i32
    } else if car.vx > 0.5 {
        1
    } else if car.vx < -0.5 {
        -1
    } else {
        0
    };
    match car.drift {
        Drift::Spin(_) => [
            CarFrame::DriftLeft,
            CarFrame::LeanLeft,
            CarFrame::DriftRight,
            CarFrame::LeanRight,
        ][(tick / 6 % 4) as usize],
        Drift::Drifting | Drift::Entry(_) if side < 0 => CarFrame::DriftLeft,
        Drift::Drifting | Drift::Entry(_) if side > 0 => CarFrame::DriftRight,
        _ if steer < 0 => CarFrame::LeanLeft,
        _ if steer > 0 => CarFrame::LeanRight,
        _ => CarFrame::Straight,
    }
}

/// The player's car, bottom-centre, on the road under it.
fn player(c: &mut Canvas, v: &RoadView, s: &RaceScene, art: &Art, bottom: i32) {
    let ground = v
        .place(s.car.z, s.car.x)
        .map_or(v.car_row(), |p| p.y)
        .min(bottom - 2);
    let x = v.w / 2 - CAR_W / 2;
    let y = ground - CAR_H + 2;
    let frame = car_frame(&s.car, s.input, s.tick);
    // Tyre smoke while sliding, flames while boosting: both flicker.
    if s.car.drifting() || s.car.spinning() {
        for i in 0..6 {
            let dx = ((s.tick as i32 * 3 + i * 7) % 9) - 4;
            let side = if i % 2 == 0 { -1 } else { 1 };
            c.put(v.w / 2 + side * 13 + dx, ground - (i % 3), WHITE);
        }
    }
    c.blit(&art.car[frame as usize], x, y, bottom, false);
    if s.car.boost_ticks > 0 && s.tick % 4 < 2 {
        for dx in [-8, 7] {
            c.rect(v.w / 2 + dx, ground - 4, 2, 2, YELLOW);
            c.put(v.w / 2 + dx, ground - 2, RED);
        }
    }
}

// ----------------------------------------------------------------- the HUD

/// A gate's time against the ghost's, `delta` ticks (positive is behind
/// it): `GHOST +1.2S`, `GHOST -0.8S`, to the nearest tenth.
pub fn ghost_delta(delta: i64) -> String {
    let tenths = (delta.unsigned_abs() * 10 + racer_sim::TICKS_PER_SEC as u64 / 2)
        / racer_sim::TICKS_PER_SEC as u64;
    let sign = match (delta.signum(), tenths) {
        (_, 0) => "",
        (1, _) => "+",
        _ => "-",
    };
    format!("GHOST {sign}{}.{}S", tenths / 10, tenths % 10)
}

/// Seconds as `12.3`.
pub fn secs(ticks: u32) -> String {
    let tenths = (ticks as u64 * 10).div_ceil(racer_sim::TICKS_PER_SEC as u64);
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// The HUD strip, and the warnings over the road view.
pub fn hud(c: &mut Canvas, l: &Layout, s: &RaceScene) {
    let r = l.hud;
    c.rect(r.x, r.y, r.w, r.h, INK);
    let low = s.timer < 5 * racer_sim::TICKS_PER_SEC;
    let flash = s.tick % 30 < 15;
    c.text(r.x + 3, r.y + 2, "TIME", WHITE, 1);
    let tc = if low && flash { RED } else { YELLOW };
    c.text_big(r.x + 3, r.y + 9, &secs(s.timer), tc);
    let kmh = (s.car.v * 3.6).round() as i32;
    c.text(r.x + 62, r.y + 2, "KM/H", WHITE, 1);
    c.text_big(r.x + 62, r.y + 9, &format!("{kmh}"), WHITE);
    c.text(r.x + 118, r.y + 2, "TRACK", WHITE, 1);
    c.text_big(
        r.x + 118,
        r.y + 9,
        &format!("{}/{}", s.index + 1, s.tracks),
        WHITE,
    );
    // Drift charge, with marks at the mini and super boosts.
    let (bx, by, bw) = (r.x + 2, r.y + r.h - 3, r.w - 4);
    c.rect(bx, by, bw, 2, ASPHALT_DK);
    let fill = ((s.car.charge / SUPER_CHARGE).min(1.0) * bw as f64).round() as i32;
    let charged = if s.car.charge >= SUPER_CHARGE {
        RED
    } else if s.car.charge >= MINI_CHARGE {
        YELLOW
    } else {
        WHITE
    };
    c.rect(bx, by, fill, 2, charged);
    let mini = bx + (MINI_CHARGE / SUPER_CHARGE * bw as f64).round() as i32;
    c.rect(mini, by - 1, 1, 3, WHITE);
    if s.car.boost_ticks > 0 && flash {
        c.rect(bx, by, bw, 2, YELLOW);
    }
    // Over the road: the next gate, and a drift corner coming up.
    let gates = s.track.checkpoints();
    if s.next_gate < gates.len() {
        let d = (s.track.checkpoint_z(s.next_gate) - s.car.z).max(0.0);
        let label = if s.next_gate + 1 == gates.len() {
            "FINISH"
        } else {
            "GATE"
        };
        let text = format!("{label} {}M", d.round() as i32);
        let w = crate::font::small_width(&text) as i32;
        c.rect(l.road.w - w - 5, 2, w + 4, 9, INK);
        c.text(l.road.w - w - 3, 4, &text, WHITE, 1);
    }
    if let Some(d) = s.ghost_note {
        let text = ghost_delta(d);
        let w = crate::font::small_width(&text) as i32;
        let ink = if d <= 0 { GRASS_LT } else { RED };
        c.rect(l.road.w - w - 5, 12, w + 4, 9, INK);
        c.text(l.road.w - w - 3, 14, &text, ink, 1);
    }
    if s.drift_ahead().is_some() && flash {
        let text = "DRIFT!";
        let w = crate::font::small_width(text) as i32 * 2;
        let x = l.road.w / 2 - w / 2;
        c.rect(x - 3, 16, w + 6, 14, INK);
        c.text(x, 18, text, YELLOW, 2);
    }
    if s.car.spinning() {
        c.text_centred(l.road.w / 2, 34, "SPIN", RED, 2);
    }
}

// ------------------------------------------------------------------ the pad

/// A pad button's plate, pressed or not.
pub fn plate(c: &mut Canvas, r: Rect, held: bool) {
    c.rect(r.x, r.y, r.w, r.h, if held { RED_DK } else { ASPHALT_DK });
    c.rect_outline(r.x, r.y, r.w, r.h, if held { YELLOW } else { ASPHALT_LT });
}

/// A filled arrow pointing left (`dir` -1) or right (1), centred on `(x, y)`.
fn arrow(c: &mut Canvas, x: i32, y: i32, dir: i32, ink: u8) {
    for i in 0..12 {
        let xx = x + dir * (6 - i);
        let half = i * 3 / 4;
        for yy in y - half..=y + half {
            c.put(xx, yy, ink);
        }
    }
}

/// The four thumb buttons.
pub fn pad(c: &mut Canvas, l: &Layout, held: Held) {
    let p = l.pad;
    c.rect(p.x, p.y, p.w, p.h, INK);
    for (r, h, dir) in [(l.left, held.left, -1), (l.right, held.right, 1)] {
        plate(c, r, h);
        arrow(
            c,
            r.x + r.w / 2,
            r.y + r.h / 2,
            dir,
            if h { YELLOW } else { WHITE },
        );
    }
    for (r, h, label) in [
        (l.brake, held.brake, "BRAKE"),
        (l.drift, held.drift, "DRIFT"),
    ] {
        plate(c, r, h);
        c.text_centred(
            r.x + r.w / 2,
            r.y + r.h / 2 - 5,
            label,
            if h { YELLOW } else { WHITE },
            2,
        );
    }
}

/// A whole racing frame.
pub fn race_frame(
    c: &mut Canvas,
    l: &Layout,
    art: &Art,
    decor: &Decor,
    s: &RaceScene,
    held: Held,
) -> RoadView {
    let v = road(c, l, art, decor, s);
    hud(c, l, s);
    pad(c, l, held);
    v
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use racer_sim::track::{fixtures, Builder};
    use racer_sim::traffic::TrafficCar;

    fn scene(track: &Track, z: f64, x: f64, steer: i8) -> RaceScene<'_> {
        let car = Car {
            z,
            x,
            v: 30.0,
            ..Car::default()
        };
        RaceScene {
            track,
            car,
            tick: 600,
            timer: 20 * 120,
            next_gate: 0,
            input: Input::new(steer, false, false),
            index: 1,
            tracks: 5,
            bg_shift: 0,
            ghost: None,
            ghost_note: None,
        }
    }

    fn draw(s: &RaceScene, h: i32) -> Canvas {
        let l = Layout::new(h);
        let mut c = Canvas::new(l.w, l.h);
        let art = Art::new();
        let decor = Decor::new(s.track);
        race_frame(&mut c, &l, &art, &decor, s, Held::default());
        c
    }

    /// A drift corner with a gravel run-in, a crest and traffic: one of
    /// everything the renderer knows.
    pub(crate) fn showcase() -> Track {
        let mut b = Builder::new();
        b.straight(40)
            .bend(50, 120, 8)
            .straight(30)
            .checkpoint()
            .straight(60)
            .checkpoint()
            .straight(20);
        b.flag(48, 34, flags::DRIFT_REQ);
        b.surface(20, 12, Surface::Gravel);
        b.surface(130, 10, Surface::Ice);
        b.surface(150, 6, Surface::Puddle);
        b.hill(120, 40, 7.0);
        b.object(30, 7.0, ObjectKind::Post);
        b.object(34, 7.0, ObjectKind::Post);
        b.object(36, -3.0, ObjectKind::Rock);
        b.traffic(TrafficCar::new(60.0, 10.0, 4.0));
        b.build()
    }

    #[test]
    fn clouds_stay_high_wrap_seamlessly_and_have_shaded_bases() {
        let sky = 200;
        let mut white = 0;
        let mut shade = 0;
        for x in 0..CLOUD_PERIOD {
            for y in 0..sky {
                let blobs = cloud_row(y, sky);
                match cloud_at(x, y, &blobs) {
                    Some(WHITE) => white += 1,
                    Some(ICE) => shade += 1,
                    Some(k) => panic!("cloud colour {k}"),
                    None => {}
                }
                if y > sky / 2 {
                    assert!(blobs.is_empty(), "clouds keep to the upper sky");
                }
                assert_eq!(
                    cloud_at(x, y, &blobs),
                    cloud_at(x + CLOUD_PERIOD, y, &blobs)
                );
            }
        }
        assert!(
            white > 0 && shade > 0 && white > shade,
            "{white} white, {shade} shaded"
        );
    }

    #[test]
    fn the_near_hills_sit_lower_than_the_far_range() {
        let far = (0..1000).map(hill_height).max().unwrap();
        let near = (0..1000).map(near_hill_height).max().unwrap();
        assert!(near < far, "near {near} vs far {far}");
        assert!((0..1000).all(|x| near_hill_height(x) >= 1));
    }

    #[test]
    fn a_frame_is_palette_only_with_sky_on_top_grass_aside_and_road_under_the_car() {
        let t = fixtures::drag_strip();
        let s = scene(&t, 40.0, 0.0, 0);
        let c = draw(&s, 320);
        assert!(c.palette_only());
        let l = Layout::new(320);
        let v = RoadView::build(&t, 40.0, 0.0, l.road.w, l.road.h, Camera::DEFAULT);
        let top = c.get(90, 0);
        assert!(top == rgba(SKY_HI) || top == rgba(SKY_LO));
        let edge = c.get(0, v.car_row());
        assert!(edge == rgba(GRASS_LT) || edge == rgba(GRASS_DK));
        // Under the car's nose, between its tyres: tarmac shows only at the
        // sides of the car sprite; check just outside it.
        let row = v.car_row() - 2;
        let tarmac = c.get(90 - CAR_W / 2 - 4, row);
        assert!(
            tarmac == rgba(ASPHALT_LT) || tarmac == rgba(ASPHALT_DK) || tarmac == rgba(WHITE),
            "{tarmac:?}"
        );
        // The car is drawn bottom-centre in its body colour.
        assert!((0..l.road.h).any(|y| c.get(90, y) == rgba(RED)));
        // The pad is ink with plates.
        assert_eq!(c.get(1, l.pad.y + 1), rgba(INK));
        assert_eq!(c.get(l.left.x, l.left.y), rgba(ASPHALT_LT));
    }

    #[test]
    fn surfaces_and_kerbs_take_their_colours() {
        let t = showcase();
        // Car on the gravel run-in: the road under it is gravel.
        let s = scene(&t, 22.0 * SEG_LEN, 0.0, 0);
        let l = Layout::new(320);
        let c = draw(&s, 320);
        let v = RoadView::build(&t, s.car.z, 0.0, l.road.w, l.road.h, Camera::DEFAULT);
        let y = v.car_row() - 25;
        let r = v.rows[y as usize].unwrap();
        assert!(r.cx - r.half > 4, "the kerb is on screen");
        let px = c.get(r.cx - r.half + 1, y);
        assert!(px == rgba(GRAVEL) || px == rgba(BROWN), "{px:?}");
        let kerb = c.get(r.cx - r.half - 1, y);
        assert!([rgba(RED), rgba(WHITE), rgba(YELLOW)].contains(&kerb));
    }

    #[test]
    fn decor_signs_the_outside_of_drift_corners_ahead_of_them() {
        let t = showcase();
        let d = Decor::new(&t);
        let signs: Vec<_> = d
            .items()
            .iter()
            .filter(|i| i.kind != DecorKind::Tree)
            .collect();
        assert!(!signs.is_empty());
        assert!(signs.iter().all(|s| s.kind == DecorKind::ChevronRight));
        assert!(signs.iter().all(|s| s.x < 0.0), "outside of a right bend");
        assert_eq!(signs[0].seg as usize, 48 - SIGN_LEAD);
        // Trees stand beyond the barrier.
        for i in d.items().iter().filter(|i| i.kind == DecorKind::Tree) {
            assert!(i.x.abs() > 6.0 + VERGE);
        }
        assert_eq!(d.between(0, 1000).len(), d.items().len());
        let s = scene(&t, 30.0 * SEG_LEN, 0.0, 0);
        assert!(s.drift_ahead().is_some());
        let s = scene(&t, 120.0 * SEG_LEN, 0.0, 0);
        assert!(s.drift_ahead().is_none());
    }

    #[test]
    fn car_frames_follow_steer_drift_and_spin() {
        let mut car = Car::default();
        assert_eq!(car_frame(&car, Input::NEUTRAL, 0), CarFrame::Straight);
        assert_eq!(
            car_frame(&car, Input::new(-3, false, false), 0),
            CarFrame::LeanLeft
        );
        assert_eq!(
            car_frame(&car, Input::new(2, false, false), 0),
            CarFrame::LeanRight
        );
        car.drift = Drift::Drifting;
        assert_eq!(
            car_frame(&car, Input::new(3, false, true), 0),
            CarFrame::DriftRight
        );
        car.vx = -2.0;
        assert_eq!(
            car_frame(&car, Input::new(0, false, true), 0),
            CarFrame::DriftLeft
        );
        car.drift = Drift::Spin(40);
        let frames: Vec<_> = (0..24)
            .step_by(6)
            .map(|t| car_frame(&car, Input::NEUTRAL, t))
            .collect();
        assert_eq!(frames.len(), 4);
        assert!(frames.windows(2).all(|p| p[0] != p[1]));
    }

    #[test]
    fn a_ghost_ahead_is_drawn_and_one_behind_the_camera_is_not() {
        let t = fixtures::drag_strip();
        let plain = scene(&t, 40.0, 0.0, 0);
        let none = draw(&plain, 320);
        // Ghost-blue pixels that are not in the frame without a ghost (the
        // clouds' bases are the same colour).
        let ghost_px = |c: &Canvas| {
            (0..c.h)
                .flat_map(|y| (0..c.w).map(move |x| (x, y)))
                .filter(|&(x, y)| c.get(x, y) == rgba(ICE) && none.get(x, y) != rgba(ICE))
                .count()
        };
        let mut ahead = plain;
        ahead.ghost = Some((40.0 + 12.0, 1.5));
        let c = draw(&ahead, 320);
        assert!(c.palette_only());
        assert_ne!(c.digest(), none.digest());
        assert!(
            ghost_px(&c) >= 8,
            "a ghost 12 m ahead shows: {}",
            ghost_px(&c)
        );
        // Right of centre, above the player's car, on the road.
        let l = Layout::new(320);
        let v = RoadView::build(&t, 40.0, 0.0, l.road.w, l.road.h, Camera::DEFAULT);
        let at = v.place(52.0, 1.5).unwrap();
        assert!(at.x > l.road.w / 2 && at.y < v.car_row());
        assert!((at.y - 12..at.y).any(|y| (at.x - 8..at.x + 8)
            .any(|x| c.get(x, y) == rgba(ICE) && none.get(x, y) != rgba(ICE))));
        // Behind the camera (5 m back) it has no place on screen at all.
        let mut behind = plain;
        behind.ghost = Some((40.0 - 8.0, 0.0));
        assert_eq!(draw(&behind, 320).digest(), none.digest());
    }

    #[test]
    fn a_ghost_over_a_crest_is_hidden_like_everything_else() {
        let t = fixtures::blind_crest();
        let s = scene(&t, 34.0 * SEG_LEN, 0.0, 0);
        let l = Layout::new(400);
        let v = RoadView::build(&t, s.car.z, 0.0, l.road.w, l.road.h, Camera::DEFAULT);
        // A point down the far side of the crest, deep enough that even the
        // car's roof is under the hilltop.
        let hidden = (40..120)
            .map(|k| k as f64 * SEG_LEN + 0.5)
            .find(|&z| {
                let art = Art::new();
                !v.visible((z / SEG_LEN) as i64)
                    && v.place(z, 0.0).is_some_and(|p| {
                        art.ghost
                            .pick(p.scale)
                            .is_some_and(|g| p.y - g.h >= p.clip.min(l.road.h))
                    })
            })
            .expect("the crest hides some road");
        let mut g = s;
        g.ghost = Some((hidden, 0.0));
        assert_eq!(draw(&g, 400).digest(), draw(&s, 400).digest());
    }

    #[test]
    fn the_ghost_delta_reads_to_the_tenth_with_its_sign() {
        assert_eq!(ghost_delta(144), "GHOST +1.2S");
        assert_eq!(ghost_delta(-96), "GHOST -0.8S");
        assert_eq!(ghost_delta(0), "GHOST 0.0S");
        assert_eq!(ghost_delta(5), "GHOST 0.0S", "under a twentieth is even");
        assert_eq!(ghost_delta(6), "GHOST +0.1S");
        assert_eq!(ghost_delta(-1200), "GHOST -10.0S");
    }

    #[test]
    fn the_hud_shows_the_ghost_delta_in_its_colour() {
        let t = fixtures::drag_strip();
        let mut s = scene(&t, 40.0, 0.0, 0);
        s.ghost_note = Some(-96);
        let c = draw(&s, 320);
        assert!((14..19).any(|y| (90..180).any(|x| c.get(x, y) == rgba(GRASS_LT))));
        s.ghost_note = Some(144);
        let c = draw(&s, 320);
        assert!((14..19).any(|y| (90..180).any(|x| c.get(x, y) == rgba(RED))));
    }

    #[test]
    fn timer_text_rounds_up_to_the_tenth() {
        assert_eq!(secs(0), "0.0");
        assert_eq!(secs(1), "0.1");
        assert_eq!(secs(120), "1.0");
        assert_eq!(secs(20 * 120 + 7), "20.1");
    }

    #[test]
    fn the_far_hills_scroll_with_the_background() {
        let t = fixtures::drag_strip();
        let mut s = scene(&t, 40.0, 0.0, 0);
        let a = draw(&s, 320);
        s.bg_shift = 30;
        let b = draw(&s, 320);
        assert_ne!(a.digest(), b.digest());
        assert!((0..180).all(|x| hill_height(x) > 0 && hill_height(x) < 30));
    }

    /// FNV digests of fixed scenes. A change to the renderer that moves a
    /// pixel moves a digest; look at the frames (`dump_golden_frames`)
    /// and accept with `cargo insta review` when the change was meant.
    #[test]
    fn golden_frames() {
        let out = golden_scenes()
            .iter()
            .map(|(name, c)| format!("{name}: {:#018x}", c.digest()))
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(out);
    }

    pub(crate) fn golden_scenes() -> Vec<(&'static str, Canvas)> {
        let sweep = fixtures::sweep_and_hairpin();
        let crest = fixtures::blind_crest();
        let hazards = fixtures::hazards();
        let show = showcase();
        let mut out = vec![
            (
                "sweep_mid_curve",
                draw(&scene(&sweep, 70.0 * SEG_LEN, 1.0, 2), 320),
            ),
            (
                "hairpin_entry",
                draw(&scene(&sweep, 124.0 * SEG_LEN, -2.0, -3), 360),
            ),
            (
                "crest_climbing",
                draw(&scene(&crest, 34.0 * SEG_LEN, 0.0, 0), 400),
            ),
            (
                "crest_over",
                draw(&scene(&crest, 50.0 * SEG_LEN, 0.0, 0), 400),
            ),
            (
                "hazards",
                draw(&scene(&hazards, 50.0 * SEG_LEN, 0.0, 0), 320),
            ),
        ];
        let mut s = scene(&show, 36.0 * SEG_LEN, 1.5, 3);
        s.car.drift = Drift::Drifting;
        s.car.charge = 0.9;
        s.car.boost_ticks = 0;
        out.push(("drift_corner_signed", draw(&s, 400)));
        // The only golden scene with a ghost: the player's best, 7 m up the
        // road through the sweep, and how the player stood at the last gate.
        let mut s = scene(&sweep, 60.0 * SEG_LEN, 0.5, 1);
        s.ghost = Some((60.0 * SEG_LEN + 7.0, -2.0));
        s.ghost_note = Some(-96);
        out.push(("ghost_ahead", draw(&s, 360)));
        out
    }

    #[test]
    #[ignore]
    fn dump_golden_frames() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/frames");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".gdignore"), "").unwrap();
        for (name, c) in golden_scenes() {
            std::fs::write(dir.join(format!("{name}.ppm")), c.ppm()).unwrap();
        }
    }
}
