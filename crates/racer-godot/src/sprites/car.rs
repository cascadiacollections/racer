//! The player's car (and its ghost): a model drawn once per frame pose.
//!
//! An original design, a short-tailed hatch-coupe in the player's red:
//! wide rear arches, a glasshouse set well forward, a full-width black
//! tail panel with two pairs of round lamps, a yellow pinstripe pair over
//! the roof and deck, a black wing on two struts, twin pipes left of
//! centre. Seen from behind and above, it is the brightest red thing on
//! the road and nothing on the road is shaped like it.
//!
//! Poses ([`CarFrame`]): straight; leaning (steering) two steps each way,
//! the body turned a little; drifting either way, turned further so the
//! flank shows; and [`SPIN_FRAMES`] turns of a spin. Every pose is drawn
//! twice, brake lamps off and lit. Tyre smoke and the boost flame are
//! separate small sprites ([`Fx`]) placed at points each frame records
//! (the rear tyres, the pipes).

use std::f64::consts::TAU;

use super::solid::{render, v3, Model, Paint, Render, Style, View, V3};
use crate::palette::*;
use crate::raster::{dither_on, Sprite};

/// The red body: shadowed, mid, lit.
const BODY: &[u8] = &[INK, RED_DK, RED_DK, RED, RED, RED];
/// Glass: dark tint with a pale sky reflection on the lit faces.
const GLASS: &[u8] = &[INK, ASPHALT_DK, ASPHALT_LT, ASPHALT_LT, ICE];
/// Tyres and trim.
const RUBBER: &[u8] = &[INK, INK, ASPHALT_DK];
/// The wing and the tail panel: black, lit to dark grey.
const TRIM: &[u8] = &[INK, INK, ASPHALT_DK, ASPHALT_DK, ASPHALT_LT];

/// The ghost's colours: pale blue glass-like body, never the player's red.
const GHOST_BODY: &[u8] = &[ICE, ICE, ICE, WHITE];
const GHOST_GLASS: &[u8] = &[WATER];

/// Pixels per metre the player's car is drawn at: about what the
/// projection gives a car this far in front of the camera.
pub const PLAYER_SCALE: f64 = 14.0;
/// Camera pitch the player's car is drawn at, radians. Gentler than the
/// camera's true angle down at the car (about 58°), so it shows its tail
/// as well as its roof: the classic arcade cheat.
pub const PLAYER_PITCH: f64 = 0.45;

/// Turns of the body, radians, for the lean and drift poses.
const LEAN: [f64; 2] = [0.10, 0.20];
const DRIFT: f64 = 0.42;
/// Rotation frames in a spin.
pub const SPIN_FRAMES: usize = 8;

/// A pose of the player's car.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarFrame {
    /// Driving straight.
    Straight,
    /// Steering left: 1 a little, 2 hard.
    LeanLeft(u8),
    /// Steering right.
    LeanRight(u8),
    /// Drifting with the nose left.
    DriftLeft,
    /// Drifting with the nose right.
    DriftRight,
    /// Spinning: turn `k` of [`SPIN_FRAMES`], clockwise from straight.
    Spin(u8),
}

impl CarFrame {
    /// Every pose, in [`CarArt::frames`] order.
    pub fn all() -> Vec<CarFrame> {
        let mut v = vec![
            CarFrame::Straight,
            CarFrame::LeanLeft(1),
            CarFrame::LeanLeft(2),
            CarFrame::LeanRight(1),
            CarFrame::LeanRight(2),
            CarFrame::DriftLeft,
            CarFrame::DriftRight,
        ];
        v.extend((0..SPIN_FRAMES as u8).map(CarFrame::Spin));
        v
    }

    /// Its place in [`CarFrame::all`].
    pub fn index(self) -> usize {
        match self {
            CarFrame::Straight => 0,
            CarFrame::LeanLeft(k) => k.clamp(1, 2) as usize,
            CarFrame::LeanRight(k) => 2 + k.clamp(1, 2) as usize,
            CarFrame::DriftLeft => 5,
            CarFrame::DriftRight => 6,
            CarFrame::Spin(k) => 7 + (k as usize % SPIN_FRAMES),
        }
    }

    /// The body's turn, radians, nose right positive.
    pub fn yaw(self) -> f64 {
        match self {
            CarFrame::Straight => 0.0,
            CarFrame::LeanLeft(k) => -LEAN[(k.clamp(1, 2) - 1) as usize],
            CarFrame::LeanRight(k) => LEAN[(k.clamp(1, 2) - 1) as usize],
            CarFrame::DriftLeft => -DRIFT,
            CarFrame::DriftRight => DRIFT,
            CarFrame::Spin(k) => TAU * (k as usize % SPIN_FRAMES) as f64 / SPIN_FRAMES as f64,
        }
    }
}

/// One pose of the player's car.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarSprite {
    /// Brake lamps off.
    pub body: Sprite,
    /// Brake lamps lit.
    pub braking: Sprite,
    /// The pixel of the car's centre on the road, in both sprites.
    pub origin: (i32, i32),
    /// The two pipes' mouths: where a boost flame starts.
    pub pipes: [(i32, i32); 2],
    /// The rear tyres' contact patches, left then right: where smoke rises.
    pub tyres: [(i32, i32); 2],
}

/// Half the car's width and length, metres (the sim's car is 1.8 by 4.5).
const HW: f64 = 0.9;
const HL: f64 = 2.2;

/// The model. `lit` lights the brake lamps; `ghost` paints it pale.
fn model(lit: bool, ghost: bool) -> Model {
    let (body, glass) = if ghost {
        (GHOST_BODY, GHOST_GLASS)
    } else {
        (BODY, GLASS)
    };
    let (body, glass, rubber, trim) = (
        Paint::Ramp(body),
        Paint::Ramp(glass),
        Paint::Ramp(if ghost { GHOST_GLASS } else { RUBBER }),
        Paint::Ramp(if ghost { GHOST_GLASS } else { TRIM }),
    );
    let mut m = Model::default();
    // Tyres first: the arches cover their tops.
    for (x0, x1) in [(-HW - 0.04, -HW + 0.3), (HW - 0.3, HW + 0.04)] {
        m.cuboid((x0, x1), (-1.85, -1.05), (0.0, 0.62), rubber);
        m.cuboid((x0 + 0.03, x1 - 0.03), (1.05, 1.75), (0.0, 0.6), rubber);
    }
    // Lower body: full width, sitting on the axles.
    m.frustum(
        (-HW, HW),
        (-HL, HL),
        (0.22, 0.66),
        (-HW, HW),
        (-HL + 0.02, HL - 0.05),
        [body, body, body, body],
    );
    // Shoulders: the wide rear arches taper in towards the deck.
    m.frustum(
        (-HW, HW),
        (-HL + 0.02, HL - 0.05),
        (0.66, 0.84),
        (-HW + 0.1, HW - 0.1),
        (-HL + 0.12, HL - 0.35),
        [body, body, body, body],
    );
    // The glasshouse, set forward; its roof in the body colour.
    m.frustum(
        (-HW + 0.18, HW - 0.18),
        (-1.3, 0.75),
        (0.84, 1.3),
        (-HW + 0.32, HW - 0.32),
        (-0.85, 0.2),
        [body, glass, glass, glass],
    );
    // Yellow pinstripes over the roof and down the deck.
    let stripe = Paint::Flat(if ghost { WHITE } else { YELLOW });
    for (x0, x1) in [(-0.2, -0.1), (0.1, 0.2)] {
        m.top_decal((x0, x1), (-0.85, 0.2), 1.3, stripe);
        m.top_decal((x0, x1), (-HL + 0.12, -1.45), 0.84, stripe);
    }
    // The tail: a black panel across the back with two pairs of lamps,
    // a diffuser under it and twin pipes left of centre.
    let back = -HL - 0.001;
    let panel = if ghost { trim } else { Paint::Flat(INK) };
    m.back_decal((-HW + 0.05, HW - 0.05), back, (0.34, 0.64), panel);
    let (lamp, lamp_in) = match (ghost, lit) {
        (true, _) => (Paint::Flat(WHITE), Paint::Flat(ICE)),
        (false, false) => (Paint::Flat(RED), Paint::Flat(RED)),
        (false, true) => (Paint::Flat(WHITE), Paint::Flat(YELLOW)),
    };
    for s in [-1.0, 1.0] {
        let (a, b) = (s * (HW - 0.12), s * (HW - 0.34));
        m.back_decal((a.min(b), a.max(b)), back - 0.002, (0.38, 0.6), lamp);
        let (a, b) = (s * (HW - 0.38), s * (HW - 0.56));
        m.back_decal((a.min(b), a.max(b)), back - 0.002, (0.42, 0.56), lamp_in);
    }
    m.cuboid((-0.72, 0.72), (-HL - 0.1, -HL + 0.2), (0.12, 0.3), trim);
    for x in [-0.42, -0.2] {
        m.cuboid(
            (x - 0.08, x + 0.08),
            (-HL - 0.16, -HL),
            (0.16, 0.28),
            if ghost {
                trim
            } else {
                Paint::Ramp(&[ASPHALT_DK, ASPHALT_LT, WHITE])
            },
        );
    }
    // Front: lamps and a black grille, for the spin's front-on turns.
    let front = HL - 0.05 + 0.001;
    m.front_decal((-0.5, 0.5), front, (0.3, 0.48), trim);
    for s in [-1.0, 1.0] {
        let (a, b) = (s * (HW - 0.1), s * (HW - 0.4));
        m.front_decal(
            (a.min(b), a.max(b)),
            front + 0.002,
            (0.5, 0.62),
            Paint::Flat(if ghost { WHITE } else { YELLOW }),
        );
    }
    // The wing: a black blade over the tail on two struts, light catching
    // its end plates.
    for x in [-0.5, 0.5] {
        m.cuboid(
            (x - 0.04, x + 0.04),
            (-HL + 0.08, -HL + 0.2),
            (0.84, 1.12),
            trim,
        );
    }
    m.cuboid(
        (-HW + 0.04, HW - 0.04),
        (-HL + 0.02, -HL + 0.3),
        (1.12, 1.2),
        trim,
    );
    let plate = Paint::Ramp(if ghost {
        GHOST_BODY
    } else {
        &[RED_DK, RED, RED]
    });
    for s in [-1.0, 1.0] {
        let (a, b) = (s * (HW - 0.04), s * (HW + 0.02));
        m.cuboid((a.min(b), a.max(b)), (-HL, -HL + 0.34), (1.06, 1.24), plate);
    }
    // The shadow: the footprint, nudged right and back, away from the light.
    m.shadow = vec![
        (-HW + 0.05, -HL - 0.15),
        (HW + 0.25, -HL - 0.15),
        (HW + 0.25, HL - 0.1),
        (-HW + 0.05, HL - 0.1),
    ];
    m
}

/// The rear tyres' outer contact points and the pipes' mouths, metres.
const TYRES: [V3; 2] = [v3(-HW + 0.1, -1.45, 0.0), v3(HW - 0.1, -1.45, 0.0)];
const PIPES: [V3; 2] = [v3(-0.42, -HL - 0.16, 0.22), v3(-0.2, -HL - 0.16, 0.22)];

/// Draw one pose of the player's car.
fn pose(f: CarFrame) -> CarSprite {
    let v = View {
        yaw: f.yaw(),
        pitch: PLAYER_PITCH,
        scale: PLAYER_SCALE,
    };
    let style = Style {
        outline: true,
        creases: 0.35,
        lift: 0.0,
    };
    let off = render(&model(false, false), &v, style);
    let on = render(&model(true, false), &v, style);
    let on = glow(on, &v);
    debug_assert_eq!(off.origin, on.origin);
    CarSprite {
        pipes: PIPES.map(|p| off.at(&v, p)),
        tyres: TYRES.map(|p| off.at(&v, p)),
        origin: off.origin,
        body: off.sprite,
        braking: on.sprite,
    }
}

/// Lit lamps glow: a scatter of red round each lamp's lit pixels, on the
/// body and the road below it.
fn glow(r: Render, _v: &View) -> Render {
    let mut s = r.sprite.clone();
    let lit: Vec<(i32, i32)> = (0..s.h)
        .flat_map(|y| (0..s.w).map(move |x| (x, y)))
        .filter(|&(x, y)| r.sprite.at(x, y) == WHITE)
        .collect();
    for (x, y) in lit {
        for (dx, dy) in [(-1, 0), (1, 0), (0, 1), (0, 2), (-1, 1), (1, 1)] {
            let (xx, yy) = (x + dx, y + dy);
            let c = r.sprite.at(xx, yy);
            if c != WHITE && c != YELLOW && (dy < 2 || dither_on(xx, yy, 8)) {
                s.set(xx, yy, RED);
            }
        }
    }
    Render {
        sprite: s,
        origin: r.origin,
    }
}

/// Every pose of the player's car, in [`CarFrame::all`] order.
#[derive(Clone, Debug)]
pub struct CarArt {
    /// One per pose.
    pub frames: Vec<CarSprite>,
}

impl CarArt {
    /// Draw them all.
    pub fn new() -> CarArt {
        CarArt {
            frames: CarFrame::all().into_iter().map(pose).collect(),
        }
    }

    /// Pose `f`.
    pub fn get(&self, f: CarFrame) -> &CarSprite {
        &self.frames[f.index()]
    }
}

impl Default for CarArt {
    fn default() -> Self {
        CarArt::new()
    }
}

/// The ghost: the player's own car, pale, at a depth's `scale` and
/// `pitch`, every other body pixel cleared, but its outline kept whole
/// so it still reads as a shape on any road. Cropped bottom-centre like
/// the other roadside sprites.
pub fn ghost(scale: f64, pitch: f64) -> Sprite {
    let v = View {
        yaw: 0.0,
        pitch,
        scale,
    };
    let style = Style {
        outline: true,
        creases: 0.0,
        lift: 0.0,
    };
    let mut m = model(false, true);
    m.shadow.clear();
    let r = render(&m, &v, style);
    let mut s = super::solid::crop_centred(&r);
    // The outline: ink pixels at the silhouette's edge become pale and
    // solid; the dark parts inside (glass, tyres, the wing) stay solid so
    // it keeps a car's face; the pale body becomes a checkerboard.
    let base = s.clone();
    for y in 0..s.h {
        for x in 0..s.w {
            let c = base.at(x, y);
            if c == Sprite::CLEAR {
                continue;
            }
            let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|&(dx, dy)| base.at(x + dx, y + dy) == Sprite::CLEAR);
            if edge {
                s.set(x, y, if c == INK { WHITE } else { c });
            } else if c == INK || c == WATER {
                s.set(x, y, WATER);
            } else if (x + y) & 1 == 1 {
                s.set(x, y, Sprite::CLEAR);
            }
        }
    }
    s
}

/// The small effects drawn with the car: tyre smoke puffs (small to big,
/// as a puff grows and drifts away) and the boost flame (two flickers).
#[derive(Clone, Debug)]
pub struct Fx {
    /// Smoke puffs, smallest first.
    pub smoke: Vec<Sprite>,
    /// The boost flame, two frames, pointing down (back, towards the
    /// camera) from its top-centre.
    pub flame: [Sprite; 2],
    /// The ghost's marker: a pale down-pointing tick drawn over it when it
    /// is too far to make out.
    pub marker: Sprite,
}

impl Fx {
    /// Draw them.
    pub fn new() -> Fx {
        let smoke = [(3, 3), (5, 4), (7, 6), (9, 7)]
            .iter()
            .map(|&(w, h)| puff(w, h))
            .collect();
        Fx {
            smoke,
            flame: [flame(4, 7, 0), flame(4, 6, 1)],
            marker: marker(),
        }
    }
}

impl Default for Fx {
    fn default() -> Self {
        Fx::new()
    }
}

/// A puff of tyre smoke `w` by `h`: a rough ball, white on top, its
/// underside grey, its edge broken by a checkerboard so it reads as
/// vapour over the road.
fn puff(w: i32, h: i32) -> Sprite {
    let mut s = Sprite::blank(w, h);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f64 + 0.5 - cx) / cx, (y as f64 + 0.5 - cy) / cy);
            let r = dx * dx + dy * dy;
            if r > 1.0 {
                continue;
            }
            let c = if dy > 0.35 {
                ASPHALT_LT
            } else if dx < -0.1 && dy < -0.1 {
                WHITE
            } else {
                ICE
            };
            // The rim is a checkerboard; small puffs keep their core.
            if r > 0.55 && (x + y) & 1 == 1 && w > 3 {
                continue;
            }
            s.set(x, y, c);
        }
    }
    s
}

/// The boost flame `w` by `h`, flicker `k`: a white core, yellow round
/// it, red at the ragged tip.
fn flame(w: i32, h: i32, k: i32) -> Sprite {
    let mut s = Sprite::blank(w, h);
    for y in 0..h {
        let t = y as f64 / h as f64;
        let half = (w as f64 / 2.0) * (1.0 - t * 0.7);
        for x in 0..w {
            let dx = (x as f64 + 0.5 - w as f64 / 2.0).abs();
            if dx > half {
                continue;
            }
            let c = if t < 0.35 && dx < half * 0.5 {
                WHITE
            } else if t < 0.7 {
                YELLOW
            } else if (x + y + k) & 1 == 0 {
                RED
            } else {
                continue;
            };
            s.set(x, y, c);
        }
    }
    s
}

/// The ghost's marker: a 7 by 5 white down-arrow, ink-edged, with an ice
/// centre.
fn marker() -> Sprite {
    let rows = [".KKKKK.", "KWWWWWK", ".KWIWK.", "..KWK..", "...K..."];
    let mut s = Sprite::blank(7, 5);
    for (y, r) in rows.iter().enumerate() {
        for (x, ch) in r.bytes().enumerate() {
            let c = match ch {
                b'K' => INK,
                b'W' => WHITE,
                b'I' => ICE,
                _ => continue,
            };
            s.set(x as i32, y as i32, c);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pose_is_drawn_lit_and_unlit_and_distinct() {
        let art = CarArt::new();
        let all = CarFrame::all();
        assert_eq!(art.frames.len(), all.len());
        for (i, f) in all.iter().enumerate() {
            assert_eq!(f.index(), i);
            let c = &art.frames[i];
            assert_eq!((c.body.w, c.body.h), (c.braking.w, c.braking.h));
            let tail_away = matches!(f, CarFrame::Spin(k) if (2..=6).contains(k));
            assert!(
                tail_away || c.body != c.braking,
                "{f:?}: the brake lamps light"
            );
            for s in [&c.body, &c.braking] {
                assert!(s.px.iter().all(|&p| p == Sprite::CLEAR || p < 16));
                // Only the fixed indices: the car is the same in every biome.
                assert!(s
                    .px
                    .iter()
                    .all(|&p| p == Sprite::CLEAR || FIXED.contains(&p)));
            }
            for j in (0..i).filter(|&j| (all[j], *f) != (CarFrame::Straight, CarFrame::Spin(0))) {
                assert!(art.frames[j].body != c.body, "{:?} and {f:?}", all[j]);
            }
        }
        let straight = art.get(CarFrame::Straight);
        assert!(straight.body.w <= 40 && straight.body.h <= 40);
        assert!(straight.body.px.contains(&RED), "the car is red");
        assert!(straight.braking.px.contains(&WHITE), "lit lamps");
    }

    #[test]
    fn leaning_and_drifting_turn_the_tail_the_right_way() {
        let art = CarArt::new();
        // Nose left swings the tail (and its pipes) right of the car's
        // centre, further the harder it turns; nose right the other way.
        let tail = |f: CarFrame| {
            let c = art.get(f);
            (c.pipes[0].0 + c.pipes[1].0) / 2 - c.origin.0
        };
        let order = [
            CarFrame::DriftRight,
            CarFrame::LeanRight(2),
            CarFrame::LeanRight(1),
            CarFrame::Straight,
            CarFrame::LeanLeft(1),
            CarFrame::LeanLeft(2),
            CarFrame::DriftLeft,
        ];
        let xs: Vec<i32> = order.iter().map(|&f| tail(f)).collect();
        assert!(xs.windows(2).all(|p| p[0] < p[1]), "{xs:?}");
        // Straight is symmetric about its centre.
        let s = &art.get(CarFrame::Straight).body;
        let o = art.get(CarFrame::Straight).origin.0;
        let painted = |x: i32| (0..s.h).any(|y| s.at(x, y) != Sprite::CLEAR && s.at(x, y) != INK);
        let xs: Vec<i32> = (0..s.w).filter(|&x| painted(x)).collect();
        assert!(((o - xs[0]) - (xs[xs.len() - 1] + 1 - o)).abs() <= 2);
    }

    #[test]
    fn anchors_land_on_the_sprite() {
        let art = CarArt::new();
        for c in &art.frames {
            for (x, y) in c.pipes.iter().chain(c.tyres.iter()).chain([&c.origin]) {
                assert!(*x >= 0 && *x < c.body.w && *y >= 0 && *y < c.body.h);
            }
        }
        let s = art.get(CarFrame::Straight);
        assert!(s.tyres[0].0 < s.origin.0 && s.tyres[1].0 > s.origin.0);
        assert!(
            s.pipes.iter().all(|p| p.0 < s.origin.0),
            "pipes left of centre"
        );
    }

    #[test]
    fn effects_are_small_and_in_the_fixed_colours() {
        let fx = Fx::new();
        let all = fx.smoke.iter().chain(fx.flame.iter()).chain([&fx.marker]);
        for s in all {
            assert!(s.w <= 9 && s.h <= 9);
            assert!(s.px.iter().any(|&p| p != Sprite::CLEAR));
            assert!(s
                .px
                .iter()
                .all(|&p| p == Sprite::CLEAR || FIXED.contains(&p)));
        }
        assert!(fx.smoke.windows(2).all(|p| p[1].w > p[0].w));
    }
}
