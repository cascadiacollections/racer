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
//!
//! A scene carries its [`Biome`] (DESIGN §5.1): the frame is painted in the
//! biome's palette, under its backdrop ([`crate::scenery`]), with its
//! flora and posts. The road, its kerbs and surfaces, the cars, the HUD
//! and the pad are drawn in the same indices everywhere.

use racer_sim::biome::Biome;
use racer_sim::car::{Car, Drift, MINI_CHARGE, SPIN_TICKS, SUPER_CHARGE};
use racer_sim::input::Input;
use racer_sim::race::Race;
use racer_sim::rng::mix64;
use racer_sim::track::{flags, ObjectKind, Surface, Track, SEG_LEN, VERGE};

use crate::layout::{Layout, Rect};
use crate::palette::*;
use crate::project::{half_width, Camera, Placed, RoadView, Row, STRIPE_SEGS};
use crate::raster::{Canvas, Sprite};
use crate::scenery;
use crate::sprites::{Art, CarFrame, Steps, SPIN_FRAMES};

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
    /// Whether the race has a timer: false is a custom race's free drive
    /// (DESIGN §9.6), which shows FREE in place of the time.
    pub timed: bool,
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
    /// The scenery: the palette, backdrop and roadside things.
    pub biome: Biome,
}

impl<'a> RaceScene<'a> {
    /// The scene of a live race, track `index` of a run, in that track's
    /// biome ([`Biome::of_track`]; a custom race sets its layout's).
    pub fn of(race: &'a Race, index: u32, tracks: u32, bg_shift: i32) -> Self {
        RaceScene {
            track: race.track(),
            car: *race.car(),
            tick: race.tick(),
            timer: race.timer(),
            timed: !race.track().allot_ticks().is_empty(),
            next_gate: race.next_gate(),
            input: race.input(),
            index,
            tracks,
            bg_shift,
            ghost: None,
            ghost_note: None,
            biome: Biome::of_track(index),
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

/// Roadside decoration that is not in the sim: trees (or the biome's
/// flora), and the warning boards before drift corners. Presentation only, placed from a hash of
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
    /// A tree: drawn as the biome's flora ([`Art::flora`]).
    Tree,
    /// A bend board pointing right.
    ChevronRight,
    /// A bend board pointing left.
    ChevronLeft,
}

/// Segments of warning boards before a drift-required corner.
pub const SIGN_LEAD: usize = 15;

impl Decor {
    /// The decor of `track` in `biome`. The boards are the same in every
    /// biome; the flora's spacing is the biome's: the meadow's trees on one
    /// segment in five, as before biomes, palms one in six, cacti one in
    /// eight, pines one in three, and street lights in a regular row either
    /// side, every 48 m.
    pub fn new(track: &Track, biome: Biome) -> Decor {
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
        let every = match biome {
            Biome::Meadow => 5,
            Biome::Coast => 6,
            Biome::Desert => 8,
            Biome::Alpine => 3,
            Biome::Night => 0,
        };
        for (s, seg) in segs.iter().enumerate() {
            if every == 0 {
                // Street lights: a regular row, staggered either side, just
                // past the verge.
                let side = match s % 12 {
                    0 => -1.0,
                    6 => 1.0,
                    _ => continue,
                };
                items.push(DecorItem {
                    seg: s as u32,
                    x: side * (seg.half_width() + VERGE + 1.5),
                    kind: DecorKind::Tree,
                });
                continue;
            }
            let h = mix64(s as u64 ^ 0x7EE5_0000);
            if h.is_multiple_of(every) {
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

/// Sky, clouds (in a biome that has them), the road row by row, then the
/// biome's backdrop on the horizon ([`scenery::backdrop`]). Each layer
/// scrolls with its own share of `bg_shift` (parallax): clouds a third,
/// the far range all of it, the near layer twice it.
fn road_rows(
    c: &mut Canvas,
    v: &RoadView,
    track: &Track,
    bg_shift: i32,
    gates: &[u32],
    biome: Biome,
) {
    let w = v.w;
    let clouds = scenery::has_clouds(biome);
    for y in 0..v.h {
        match v.rows[y as usize] {
            Some(r) => road_row(c, y, w, &r, track, gates),
            None if y >= v.horizon => c.span(0, w, y, GRASS_DK),
            None => {
                let frac = ((y * 16) / v.horizon.max(1)).clamp(0, 16) as u8;
                c.dither_span(0, w, y, SKY_HI, SKY_LO, frac);
                if !clouds {
                    continue;
                }
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
    // The horizon's layers, under any road that rises above it.
    scenery::backdrop(c, v, bg_shift, biome);
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
        /// Rows above its spot it floats: the ghost's marker.
        lift: i32,
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
            out.push(Thing::Sprite {
                sprite,
                at,
                flip,
                lift: 0,
            });
        }
    }
}

/// Paint the road view (`l.road`) and return its projection. Sets the
/// canvas to the scene's biome's palette first, so everything drawn over
/// it this frame (the HUD, the pad, a menu) is in that palette too.
pub fn road(c: &mut Canvas, l: &Layout, art: &Art, decor: &Decor, s: &RaceScene) -> RoadView {
    c.set_palette(palette(s.biome));
    let v = RoadView::build(
        s.track,
        s.car.z,
        s.car.x,
        l.road.w,
        l.road.h,
        Camera::DEFAULT,
    );
    road_rows(c, &v, s.track, s.bg_shift, s.track.checkpoints(), s.biome);
    let (from, to) = (v.base, v.base + v.cam.draw_segs as i64);
    let mut things: Vec<Thing> = Vec::new();
    for d in decor.between(from, to) {
        let at = v.place(d.seg as f64 * SEG_LEN + 0.01, d.x);
        match d.kind {
            DecorKind::Tree => stand(&mut things, art.flora(s.biome), at, d.x < 0.0),
            DecorKind::ChevronRight => stand(&mut things, &art.chevron, at, false),
            DecorKind::ChevronLeft => stand(&mut things, &art.chevron, at, true),
        }
    }
    let (fa, ta) = (from.max(0) as usize, to.max(0) as usize);
    for o in s.track.objects_in(fa, ta) {
        let steps = match o.kind {
            ObjectKind::Post => art.post_for(s.biome),
            ObjectKind::Rock => &art.rock,
        };
        stand(&mut things, steps, v.place(o.z(), o.x()), false);
    }
    for t in s.track.traffic() {
        stand(
            &mut things,
            art.traffic_for(t),
            v.place(t.z_at(s.tick), t.x()),
            false,
        );
    }
    // The ghost is one more thing on the road, sorted by depth with the
    // rest and clipped by crests like them. Behind the camera it has no
    // place; under the player's car it is drawn first and covered.
    if let Some((z, x)) = s.ghost {
        let at = v.place(z, x);
        stand(&mut things, &art.ghost, at, false);
        // Far off, a marker floats over it, drawn just after it: a car a
        // few pixels wide is easy to lose on the road. Only while some of
        // the ghost itself shows over the road in front of it.
        if let Some(at) = at.filter(|a| a.scale < GHOST_MARKER_SCALE) {
            let bottom = l.road.y + l.road.h;
            if let Some(g) = art.ghost.pick(at.scale) {
                if at.y - g.h < at.clip.min(bottom) {
                    things.push(Thing::Sprite {
                        sprite: &art.fx.marker,
                        at: Placed {
                            dz: at.dz - 1e-3,
                            ..at
                        },
                        flip: false,
                        lift: g.h + 2,
                    });
                }
            }
        }
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
            Thing::Sprite {
                sprite,
                at,
                flip,
                lift,
            } => {
                let y_max = at.clip.min(bottom);
                let y = at.y - sprite.h - lift;
                c.blit(sprite, at.x - sprite.w / 2, y, y_max, flip);
            }
            Thing::Gate { at, hw, finish } => gate(c, &at, hw, finish, bottom),
        }
    }
    player(c, &v, s, art, bottom);
    v
}

/// A checkpoint gate: two posts and a banner over the road. The posts are
/// white with a grey shadow side and red bands; the banner is yellow (the
/// finish: chequered) inside an ink border, and once it is tall enough
/// it names itself, CHECKPOINT or FINISH, in ink on a plate of its own.
fn gate(c: &mut Canvas, at: &Placed, hw: f64, finish: bool, bottom: i32) {
    let s = at.scale;
    let y_max = at.clip.min(bottom);
    let reach = ((hw + 0.8) * s).round() as i32;
    let post = ((0.4 * s).round() as i32).max(1);
    let (top, band) = ((5.5 * s).round() as i32, ((1.2 * s).round() as i32).max(1));
    let stripe = ((0.6 * s).round() as i32).max(1);
    let y0 = at.y - top;
    for y in y0.max(0)..at.y.min(y_max) {
        // Red bands up the posts, every other stripe.
        let red = post >= 2 && ((y - y0) / stripe) % 2 == 1;
        for (x0, x1) in [
            (at.x - reach - post, at.x - reach),
            (at.x + reach, at.x + reach + post),
        ] {
            c.span(x0, x1, y, if red { RED } else { WHITE });
            if post >= 3 {
                c.put(x1 - 1, y, if red { RED_DK } else { ASPHALT_LT });
            }
        }
    }
    let (bx0, bx1) = (at.x - reach - post, at.x + reach + post);
    let border = band >= 4;
    for y in y0.max(0)..(y0 + band).min(y_max) {
        let edge_row = border && (y == y0 || y == y0 + band - 1);
        for x in bx0..bx1 {
            let ink = if edge_row || (border && (x == bx0 || x == bx1 - 1)) {
                INK
            } else {
                let cell =
                    ((x - at.x).div_euclid(band.max(2) / 2 + 1) + (y - y0) / (band / 2 + 1)) & 1;
                match (finish, cell == 0) {
                    (true, true) => WHITE,
                    (true, false) => INK,
                    (false, _) => YELLOW,
                }
            };
            c.put(x, y, ink);
        }
    }
    // A checkpoint's banner, too short for a word, gets ink chevrons.
    let label = if finish { "FINISH" } else { "CHECKPOINT" };
    let tw = crate::font::small_width(label) as i32;
    if band >= 7 && tw + 6 <= bx1 - bx0 {
        let (plate, ink) = if finish { (WHITE, INK) } else { (YELLOW, INK) };
        let ty = y0 + (band - 5) / 2;
        if ty + 5 < y_max {
            c.rect(at.x - tw / 2 - 2, ty - 1, tw + 4, 7, plate);
            c.text(at.x - tw / 2, ty, label, ink, 1);
        }
    } else if !finish && band >= 5 {
        // Arrows: ">" marks across the banner, `h` rows each way.
        let h = (band - 2) / 2;
        let mid = y0 + band / 2;
        let mut x = bx0 + 2;
        while x + h < bx1 - 2 {
            for d in 1 - h..h {
                if mid + d < y_max {
                    c.put(x + h - 1 - d.abs(), mid + d, INK);
                }
            }
            x += h + 3;
        }
    }
}

/// Pixels a metre below which the ghost gets a marker over it: past
/// about 15 m ahead of the player's car.
pub const GHOST_MARKER_SCALE: f64 = 3.0;

/// Which pose the car is drawn in. Steering leans it one step, full lock
/// two; a drift turns it the way it slides; a spin turns it round one and
/// a half times, fast at first, slowing as the spin runs out.
pub fn car_frame(car: &Car, input: Input) -> CarFrame {
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
    let lean = if steer.abs() >= racer_sim::input::STEER_LEVELS {
        2
    } else {
        1
    };
    match car.drift {
        Drift::Spin(n) => {
            let t = 1.0 - n.min(SPIN_TICKS) as f64 / SPIN_TICKS as f64;
            let turns = 1.5 * (1.0 - (1.0 - t) * (1.0 - t));
            let k = (turns * SPIN_FRAMES as f64) as usize % SPIN_FRAMES;
            // Sliding left, it spins the other way.
            let k = if car.vx < -0.5 {
                (SPIN_FRAMES - k) % SPIN_FRAMES
            } else {
                k
            };
            CarFrame::Spin(k as u8)
        }
        Drift::Drifting | Drift::Entry(_) if side < 0 => CarFrame::DriftLeft,
        Drift::Drifting | Drift::Entry(_) if side > 0 => CarFrame::DriftRight,
        _ if steer < 0 => CarFrame::LeanLeft(lean),
        _ if steer > 0 => CarFrame::LeanRight(lean),
        _ => CarFrame::Straight,
    }
}

/// The player's car, bottom-centre, on the road under it: its shadow and
/// lamps are in its frames; tyre smoke while sliding and the flame while
/// boosting are drawn over it.
fn player(c: &mut Canvas, v: &RoadView, s: &RaceScene, art: &Art, bottom: i32) {
    let ground = v
        .place(s.car.z, s.car.x)
        .map_or(v.car_row(), |p| p.y)
        .min(bottom - 2);
    let frame = car_frame(&s.car, s.input);
    // Every frame turns about the car's centre, placed so the straight
    // frame's lowest row sits two rows under the road at the car.
    let straight = art.car.get(CarFrame::Straight);
    let cy = ground + 2 - (straight.body.h - straight.origin.1);
    let f = art.car.get(frame);
    let (x, y) = (v.w / 2 - f.origin.0, cy - f.origin.1);
    let sprite = if s.input.brake() { &f.braking } else { &f.body };
    c.blit(sprite, x, y, bottom, false);
    if s.car.boost_ticks > 0 {
        let flame = &art.fx.flame[(s.tick / 2 % 2) as usize];
        for (px, py) in f.pipes {
            c.blit(flame, x + px - flame.w / 2, y + py, bottom, false);
        }
    }
    if s.car.drifting() || s.car.spinning() {
        // Puffs leave each rear tyre in turn, grow, and drift out and back.
        let smoke = &art.fx.smoke;
        let life = 4 * smoke.len() as u32;
        for i in 0..4u32 {
            let age = (s.tick + i * life / 4) % life;
            let puff = &smoke[(age / 4) as usize];
            let side = (i % 2) as usize;
            let (tx, ty) = f.tyres[side];
            let out = if side == 0 { -1 } else { 1 } * (age as i32 / 3);
            c.blit(
                puff,
                x + tx + out - puff.w / 2,
                y + ty + age as i32 / 4 - puff.h + 2,
                bottom,
                false,
            );
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

/// The ink of good news on the HUD (the ghost behind you) in biome `b`: a
/// green that reads on ink in that biome's palette.
pub fn ahead_ink(b: Biome) -> u8 {
    match b {
        Biome::Meadow => GRASS_LT,
        Biome::Coast | Biome::Alpine | Biome::Desert => HILLS,
        Biome::Night => ICE,
    }
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
    let low = s.timed && s.timer < 5 * racer_sim::TICKS_PER_SEC;
    let flash = s.tick % 30 < 15;
    c.text(r.x + 3, r.y + 2, "TIME", WHITE, 1);
    let tc = if low && flash { RED } else { YELLOW };
    if s.timed {
        c.text_big(r.x + 3, r.y + 9, &secs(s.timer), tc);
    } else {
        c.text_big(r.x + 3, r.y + 9, "FREE", WHITE);
    }
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
        let ink = if d <= 0 { ahead_ink(s.biome) } else { RED };
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
            timed: true,
            next_gate: 0,
            input: Input::new(steer, false, false),
            index: 1,
            tracks: 5,
            bg_shift: 0,
            ghost: None,
            ghost_note: None,
            biome: Biome::Meadow,
        }
    }

    fn draw(s: &RaceScene, h: i32) -> Canvas {
        let l = Layout::new(h);
        let mut c = Canvas::new(l.w, l.h);
        let art = Art::shared();
        let decor = Decor::new(s.track, s.biome);
        race_frame(&mut c, &l, art, &decor, s, Held::default());
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
        let half = Art::shared().car.get(CarFrame::Straight).body.w / 2;
        let tarmac = c.get(90 - half - 4, row);
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
    fn every_biome_paints_in_its_own_sixteen_colours() {
        let show = showcase();
        let frames: Vec<(Biome, Canvas)> = Biome::ALL
            .iter()
            .map(|&b| (b, draw(&biome_scene(&show, b), 360)))
            .collect();
        for (b, c) in &frames {
            assert!(c.palette_only(), "{b:?}");
            assert_eq!(c.palette(), palette(*b));
            // A biome's scenery shows: some pixel is one of its own
            // scenery colours, not one any other biome has.
            if *b != Biome::Meadow {
                let mut meadow = c.clone();
                meadow.set_palette(&COLOURS);
                assert!(!meadow.palette_only(), "{b:?} shows its colours");
            }
        }
        for (i, (a, x)) in frames.iter().enumerate() {
            for (b, y) in &frames[i + 1..] {
                assert_ne!(x.digest(), y.digest(), "{a:?} and {b:?}");
            }
        }
        // The road, the car and the HUD are the same indices everywhere:
        // the pad and the HUD strip are byte for byte the meadow's.
        let l = Layout::new(360);
        for (b, c) in &frames {
            for y in l.hud.y..l.h {
                for x in 0..l.w {
                    assert_eq!(c.get(x, y), frames[0].1.get(x, y), "{b:?} at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn the_good_news_ink_reads_on_the_hud_in_every_biome() {
        for b in Biome::ALL {
            let p = palette(b);
            let ink = p[ahead_ink(b) as usize];
            assert!(contrast(ink, p[INK as usize]) >= 4.5, "{b:?}");
            assert!(distance(ink, p[RED as usize]) >= 80.0, "{b:?}: not the red");
        }
    }

    #[test]
    fn night_has_street_lights_in_rows_and_stars_not_clouds() {
        let show = showcase();
        let d = Decor::new(&show, Biome::Night);
        let lamps: Vec<_> = d
            .items()
            .iter()
            .filter(|i| i.kind == DecorKind::Tree)
            .collect();
        assert!(lamps.iter().all(|i| i.seg % 6 == 0));
        assert!(lamps.iter().any(|i| i.x < 0.0) && lamps.iter().any(|i| i.x > 0.0));
        for i in &lamps {
            let hw = show.segments()[i.seg as usize].half_width();
            assert!(i.x.abs() > hw + VERGE, "beyond the verge");
        }
        // The meadow's trees are where they always were: one segment in
        // five by the same hash; other biomes are denser or sparser.
        let trees = |b| {
            Decor::new(&show, b)
                .items()
                .iter()
                .filter(|i| i.kind == DecorKind::Tree)
                .count()
        };
        assert!(trees(Biome::Alpine) > trees(Biome::Meadow));
        assert!(trees(Biome::Desert) < trees(Biome::Meadow));
        assert!(!scenery::has_clouds(Biome::Night) && !scenery::has_clouds(Biome::Desert));
        // At night the sky's top rows hold stars (white) on the dark.
        let c = draw(&biome_scene(&show, Biome::Night), 360);
        let stars = (0..40)
            .flat_map(|y| (0..180).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) == rgba(WHITE))
            .count();
        assert!(stars >= 5, "{stars} stars");
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
        let d = Decor::new(&t, Biome::Meadow);
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
        assert_eq!(car_frame(&car, Input::NEUTRAL), CarFrame::Straight);
        assert_eq!(
            car_frame(&car, Input::new(-3, false, false)),
            CarFrame::LeanLeft(2)
        );
        assert_eq!(
            car_frame(&car, Input::new(-1, false, false)),
            CarFrame::LeanLeft(1)
        );
        assert_eq!(
            car_frame(&car, Input::new(2, false, false)),
            CarFrame::LeanRight(1)
        );
        car.drift = Drift::Drifting;
        assert_eq!(
            car_frame(&car, Input::new(3, false, true)),
            CarFrame::DriftRight
        );
        car.vx = -2.0;
        assert_eq!(
            car_frame(&car, Input::new(0, false, true)),
            CarFrame::DriftLeft
        );
        // A spin runs through every rotation frame, the first turn fast.
        car.vx = 0.0;
        let frames: Vec<CarFrame> = (0..=SPIN_TICKS)
            .rev()
            .map(|n| {
                car.drift = Drift::Spin(n);
                car_frame(&car, Input::NEUTRAL)
            })
            .collect();
        for k in 0..SPIN_FRAMES as u8 {
            assert!(frames.contains(&CarFrame::Spin(k)), "frame {k}");
        }
        assert_eq!(frames[0], CarFrame::Spin(0));
        let changes = |f: &[CarFrame]| f.windows(2).filter(|p| p[0] != p[1]).count();
        let half = frames.len() / 2;
        assert!(changes(&frames[..half]) > changes(&frames[half..]));
        // Sliding left, it turns the other way.
        car.vx = -2.0;
        car.drift = Drift::Spin(SPIN_TICKS - 12);
        let left = car_frame(&car, Input::NEUTRAL);
        car.vx = 2.0;
        let right = car_frame(&car, Input::NEUTRAL);
        assert_ne!(left, right);
    }

    #[test]
    fn the_car_lights_its_brakes_and_smokes_and_flames() {
        let t = fixtures::drag_strip();
        let base = scene(&t, 40.0, 0.0, 0);
        let plain = draw(&base, 320);
        let count = |c: &Canvas, k: u8| {
            (0..c.h)
                .flat_map(|y| (0..c.w).map(move |x| (x, y)))
                .filter(|&(x, y)| c.get(x, y) == rgba(k))
                .count()
        };
        let mut braking = base;
        braking.input = Input::new(0, true, false);
        let b = draw(&braking, 320);
        assert!(count(&b, WHITE) > count(&plain, WHITE), "lamps lit");
        let mut boost = base;
        boost.car.boost_ticks = 30;
        let f = draw(&boost, 320);
        assert!(count(&f, YELLOW) > count(&plain, YELLOW), "a flame");
        let mut drift = base;
        drift.car.drift = Drift::Drifting;
        drift.input = Input::new(2, false, true);
        let d = draw(&drift, 320);
        assert_ne!(d.digest(), plain.digest());
        assert!(count(&d, ICE) > count(&plain, ICE), "smoke");
        for c in [&b, &f, &d] {
            assert!(c.palette_only());
        }
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
                .filter(|&(x, y)| {
                    [rgba(ICE), rgba(WHITE)].contains(&c.get(x, y)) && none.get(x, y) != c.get(x, y)
                })
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
            .any(|x| [rgba(ICE), rgba(WHITE)].contains(&c.get(x, y))
                && none.get(x, y) != c.get(x, y))));
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
                let art = Art::shared();
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
        // The ghost down a straight at 10, 20 and 40 m: how far it reads
        // (the marker shows past ~15 m).
        let strip = fixtures::drag_strip();
        for (name, d) in [
            ("ghost_10m", 10.0),
            ("ghost_20m", 20.0),
            ("ghost_40m", 40.0),
        ] {
            let mut s = scene(&strip, 40.0, 0.0, 0);
            s.ghost = Some((40.0 + d, 1.0));
            out.push((name, draw(&s, 360)));
        }
        // A checkpoint gate close ahead, braking for it; the finish
        // close ahead, boosting at it.
        let mut s = scene(&show, 120.0 * SEG_LEN - 6.0, 0.0, 0);
        s.input = Input::new(0, true, false);
        out.push(("gate_ahead", draw(&s, 360)));
        let mut s = scene(&show, 180.0 * SEG_LEN - 9.0, 0.5, 0);
        s.next_gate = 1;
        s.car.boost_ticks = 30;
        out.push(("finish_ahead", draw(&s, 360)));
        // One scene per biome but the meadow's (every scene above is a
        // meadow): the showcase's run-up, posts, rock and traffic ahead,
        // the backdrop scrolled to show its layers.
        for b in &Biome::ALL[1..] {
            out.push((biome_scene_name(*b), draw(&biome_scene(&show, *b), 360)));
        }
        out
    }

    /// The golden scene of biome `b`.
    pub(crate) fn biome_scene(show: &Track, b: Biome) -> RaceScene<'_> {
        let mut s = scene(show, 22.0 * SEG_LEN, -0.5, 0);
        s.biome = b;
        s.bg_shift = 37;
        s
    }

    /// Its name among the golden frames.
    pub(crate) fn biome_scene_name(b: Biome) -> &'static str {
        match b {
            Biome::Meadow => "biome_meadow",
            Biome::Coast => "biome_coast",
            Biome::Alpine => "biome_alpine",
            Biome::Desert => "biome_desert",
            Biome::Night => "biome_night",
        }
    }

    /// A desktop raster micro-benchmark: `cargo test --release -p
    /// racer-godot raster_bench -- --ignored --nocapture`. Paints a racing
    /// frame (road, sprites, HUD, pad) at 180x360 over and over, moving
    /// the car so every frame differs, and prints the mean per frame.
    #[test]
    #[ignore = "timing, run by hand in release"]
    fn raster_bench() {
        let t = fixtures::sweep_and_hairpin();
        let l = Layout::new(360);
        let art = Art::new();
        let mut c = Canvas::new(l.w, l.h);
        let n = 4000;
        for biome in Biome::ALL {
            let decor = Decor::new(&t, biome);
            let mut s = scene(&t, 20.0 * SEG_LEN, 0.5, 1);
            s.biome = biome;
            let start = std::time::Instant::now();
            for i in 0..n {
                s.car.z = 20.0 * SEG_LEN + (i % 400) as f64 * 0.5;
                s.bg_shift = i / 4;
                race_frame(&mut c, &l, &art, &decor, &s, Held::default());
            }
            let us = start.elapsed().as_secs_f64() * 1e6 / n as f64;
            println!("raster_bench {biome:?}: {us:.1} us per frame ({n} frames)");
            assert!(c.palette_only());
        }
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
        // The meadow's biome scene too, beside the others for comparison.
        let show = showcase();
        let c = draw(&biome_scene(&show, Biome::Meadow), 360);
        std::fs::write(dir.join("biome_meadow.ppm"), c.ppm()).unwrap();
    }
}
