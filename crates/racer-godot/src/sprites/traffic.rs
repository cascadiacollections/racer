//! Traffic: three kinds of vehicle in a few liveries, at every scale step.
//!
//! A hatchback, a box van and a pickup, each a small solid model
//! ([`super::solid`]) drawn from the angle the camera sees that depth at.
//! The three smallest steps (two to five pixels wide) are not rendered:
//! they are drawn by hand for each kind ([`tiny`]), since at that size a
//! renderer's sampling loses the one thing that matters, the red tail
//! lamps either side of a body-coloured block.
//!
//! Liveries use only the indices every biome shares (blue, yellow,
//! white), never the player's red, so a car on the road reads the same in
//! every biome and never as the player.

use super::solid::{crop_centred, render, Model, Paint, Style, View};
use super::{pitch_at, Steps, SCALES};
use crate::palette::*;
use crate::raster::Sprite;

/// A kind of vehicle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A short hatchback with a big rear window.
    Hatch,
    /// A tall box van with twin rear doors.
    Van,
    /// A pickup: a cab and an open bed.
    Pickup,
}

/// A paint job: a ramp from shadow to highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Livery(pub &'static [u8]);

/// Blue.
pub const BLUE: Livery = Livery(&[INK, WATER, WATER, ICE]);
/// Taxi yellow.
pub const YELLOW_L: Livery = Livery(&[INK, GRAVEL, YELLOW, YELLOW]);
/// White.
pub const WHITE_L: Livery = Livery(&[INK, ASPHALT_LT, WHITE, WHITE]);

/// Every variant on the road, `(kind, livery)`.
pub const VARIANTS: [(Kind, Livery); 7] = [
    (Kind::Hatch, BLUE),
    (Kind::Hatch, YELLOW_L),
    (Kind::Hatch, WHITE_L),
    (Kind::Van, WHITE_L),
    (Kind::Van, BLUE),
    (Kind::Pickup, YELLOW_L),
    (Kind::Pickup, WHITE_L),
];

const GLASS: &[u8] = &[INK, ASPHALT_DK, ASPHALT_DK, ASPHALT_LT];
const TRIM: &[u8] = &[INK, INK, ASPHALT_DK];

/// The model of `kind` in `livery`.
fn model(kind: Kind, livery: Livery) -> Model {
    let body = Paint::Ramp(livery.0);
    let glass = Paint::Ramp(GLASS);
    let trim = Paint::Ramp(TRIM);
    let lamp = Paint::Flat(RED);
    let mut m = Model::default();
    let (hw, hl) = match kind {
        Kind::Hatch => (0.85, 1.95),
        Kind::Van => (0.95, 2.3),
        Kind::Pickup => (0.92, 2.4),
    };
    // Tyres, under everything.
    for (x0, x1) in [(-hw - 0.02, -hw + 0.28), (hw - 0.28, hw + 0.02)] {
        m.cuboid((x0, x1), (-hl + 0.35, -hl + 1.05), (0.0, 0.6), trim);
        m.cuboid((x0, x1), (hl - 1.05, hl - 0.35), (0.0, 0.6), trim);
    }
    let back = -hl - 0.001;
    match kind {
        Kind::Hatch => {
            m.cuboid((-hw, hw), (-hl, hl), (0.22, 0.78), body);
            m.frustum(
                (-hw + 0.05, hw - 0.05),
                (-hl + 0.05, 0.75),
                (0.78, 1.45),
                (-hw + 0.18, hw - 0.18),
                (-hl + 0.3, 0.35),
                [body, glass, glass, glass],
            );
            for s in [-1.0f64, 1.0] {
                let (a, b) = (s * (hw - 0.05), s * (hw - 0.3));
                m.back_decal((a.min(b), a.max(b)), back, (0.5, 0.72), lamp);
            }
            m.back_decal((-0.22, 0.22), back, (0.38, 0.5), Paint::Flat(WHITE));
            m.cuboid((-hw, hw), (-hl - 0.06, -hl + 0.1), (0.15, 0.32), trim);
        }
        Kind::Van => {
            // The box, then the cab in front of it.
            m.cuboid((-hw, hw), (-hl, 1.0), (0.25, 2.2), body);
            m.frustum(
                (-hw, hw),
                (1.0, hl),
                (0.25, 1.6),
                (-hw + 0.05, hw - 0.05),
                (1.0, hl - 0.6),
                [body, body, body, glass],
            );
            // Twin doors: a seam, two small windows, tall lamps low down.
            m.back_decal((-0.05, 0.05), back, (0.3, 2.1), Paint::Flat(INK));
            for s in [-1.0f64, 1.0] {
                let (a, b) = (s * 0.18, s * 0.7);
                m.back_decal((a.min(b), a.max(b)), back, (1.45, 1.95), glass);
                let (a, b) = (s * (hw - 0.02), s * (hw - 0.2));
                m.back_decal((a.min(b), a.max(b)), back, (0.35, 1.0), lamp);
            }
            m.cuboid((-hw, hw), (-hl - 0.08, -hl + 0.1), (0.15, 0.32), trim);
        }
        Kind::Pickup => {
            // Bed: a dark floor between body-coloured walls and tailgate.
            m.cuboid((-hw + 0.1, hw - 0.1), (-hl + 0.1, -0.2), (0.25, 0.6), trim);
            for (x0, x1) in [(-hw, -hw + 0.1), (hw - 0.1, hw)] {
                m.cuboid((x0, x1), (-hl, -0.2), (0.25, 1.0), body);
            }
            m.cuboid((-hw, hw), (-hl, -hl + 0.1), (0.25, 1.0), body);
            // Cab and bonnet.
            m.frustum(
                (-hw, hw),
                (-0.2, 1.1),
                (0.25, 1.75),
                (-hw + 0.1, hw - 0.1),
                (-0.15, 0.9),
                [body, glass, glass, glass],
            );
            m.cuboid((-hw, hw), (1.1, hl), (0.25, 1.0), body);
            for s in [-1.0f64, 1.0] {
                let (a, b) = (s * (hw - 0.02), s * (hw - 0.2));
                m.back_decal((a.min(b), a.max(b)), back, (0.45, 0.9), lamp);
            }
            m.cuboid((-hw, hw), (-hl - 0.08, -hl + 0.1), (0.15, 0.3), trim);
        }
    }
    m.shadow = vec![
        (-hw, -hl - 0.15),
        (hw + 0.12, -hl - 0.15),
        (hw + 0.12, hl),
        (-hw, hl),
    ];
    m
}

/// The three smallest steps of `kind`, by hand: rows of `B` body, `b`
/// body shadow, `G` glass, `K` ink, `R` tail lamp, `W` plate, `.` clear.
fn tiny(kind: Kind, step: usize) -> &'static [&'static str] {
    match (kind, step) {
        (Kind::Hatch, 0) => &["bb", "RR"],
        (Kind::Hatch, 1) => &[".b.", "bGb", "RBR"],
        (Kind::Hatch, _) => &[".bbb.", "bGGGb", "BBBBB", "RBWBR", "K...K"],
        (Kind::Van, 0) => &["bb", "BB", "RR"],
        (Kind::Van, 1) => &["bbb", "BBB", "BKB", "RKR"],
        (Kind::Van, _) => &["bbbbb", "BGKGB", "BBKBB", "RBKBR", "RBKBR", "K...K"],
        (Kind::Pickup, 0) => &["bb", "RR"],
        (Kind::Pickup, 1) => &[".b.", "bKb", "RBR"],
        (Kind::Pickup, _) => &[".bbb.", ".GGG.", "bKKKb", "RBBBR", "K...K"],
    }
}

/// Paint a hand-drawn step in `livery`.
fn from_rows(rows: &[&str], livery: Livery) -> Sprite {
    let ramp = livery.0;
    let w = rows.iter().map(|r| r.len()).max().unwrap_or(1) as i32;
    let mut s = Sprite::blank(w, rows.len() as i32);
    for (y, r) in rows.iter().enumerate() {
        for (x, ch) in r.bytes().enumerate() {
            let c = match ch {
                b'B' => ramp[ramp.len() - 2],
                b'b' => ramp[1],
                b'G' => ASPHALT_DK,
                b'K' => INK,
                b'R' => RED,
                b'W' => WHITE,
                _ => continue,
            };
            s.set(x as i32, y as i32, c);
        }
    }
    s
}

/// Steps under this many pixels a metre are drawn by hand.
const HAND_DRAWN: usize = 3;

/// One variant at every step.
pub fn steps(kind: Kind, livery: Livery) -> Steps {
    let m = model(kind, livery);
    let sprites = SCALES
        .iter()
        .enumerate()
        .map(|(i, &scale)| {
            if i < HAND_DRAWN {
                return from_rows(tiny(kind, i), livery);
            }
            let v = View {
                yaw: 0.0,
                pitch: pitch_at(scale),
                scale,
                squash: 1.0,
            };
            let style = Style {
                outline: scale >= 6.0,
                creases: if scale >= 6.0 { 0.4 } else { 0.0 },
                lift: 0.0,
            };
            crop_centred(&render(&m, &v, style))
        })
        .collect();
    let h = match kind {
        Kind::Van => 2.2,
        Kind::Pickup => 1.75,
        Kind::Hatch => 1.45,
    };
    Steps {
        size_m: (2.0, h),
        sprites,
    }
}

/// Every variant, in [`VARIANTS`] order.
pub fn all() -> Vec<Steps> {
    VARIANTS.iter().map(|&(k, l)| steps(k, l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_grows_step_by_step_in_the_shared_colours() {
        let all = all();
        assert_eq!(all.len(), VARIANTS.len());
        for (i, v) in all.iter().enumerate() {
            assert_eq!(v.sprites.len(), SCALES.len());
            assert!(
                v.sprites.windows(2).all(|p| p[1].w > p[0].w),
                "{:?}: {:?}",
                VARIANTS[i],
                v.sprites.iter().map(|s| s.w).collect::<Vec<_>>()
            );
            for s in &v.sprites {
                assert!(s
                    .px
                    .iter()
                    .all(|&c| c == Sprite::CLEAR || FIXED.contains(&c)));
                assert!(!s.px.contains(&RED_DK), "no player red");
                // Tail lamps at every size, even two pixels wide.
                assert!(s.px.contains(&RED), "{:?} {}px", VARIANTS[i], s.w);
            }
            // Never wider than a car and its mirrors at the biggest step.
            assert!(v.sprites.last().unwrap().w <= 36);
        }
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert!(all[i].sprites[4] != all[j].sprites[4], "{i} and {j}");
            }
        }
    }

    #[test]
    fn a_van_stands_taller_than_a_hatch() {
        let h = steps(Kind::Hatch, BLUE);
        let v = steps(Kind::Van, BLUE);
        for k in 0..SCALES.len() {
            assert!(v.sprites[k].h > h.sprites[k].h, "step {k}");
        }
    }
}
