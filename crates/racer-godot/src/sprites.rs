//! Placeholder pixel sprites, generated procedurally once at startup.
//!
//! DESIGN §5's rule is that nothing is scaled or rotated at draw time: a
//! sprite seen at some depth is drawn from the nearest of a few pre-drawn
//! sizes ([`SCALES`]), copied 1:1. Until there is real art, each kind is a
//! shape function over the unit square, sampled at every size here, so a
//! small sprite is drawn small rather than shrunk from a big one. The player
//! car has fixed-size frames for straight, leaning and drifting either way.
//!
//! Every pixel is a palette index; the shapes only ever pick from
//! [`crate::palette`].

use crate::palette::*;
use crate::raster::Sprite;

/// The pre-drawn sizes, in pixels per metre at the sprite's depth. The
/// projection gives `focal / depth`; the nearest step (by ratio) is drawn.
pub const SCALES: [f64; 7] = [1.0, 1.6, 2.6, 4.2, 6.6, 10.0, 15.0];

/// One kind of sprite at every step in [`SCALES`].
#[derive(Clone, Debug)]
pub struct Steps {
    /// Size in metres, width and height.
    pub size_m: (f64, f64),
    /// One sprite per scale step.
    pub sprites: Vec<Sprite>,
}

impl Steps {
    /// Sample `shape` at every step for a thing `w_m` by `h_m` metres.
    pub fn new(w_m: f64, h_m: f64, shape: impl Fn(f64, f64, i32, i32) -> u8) -> Steps {
        let sprites = SCALES
            .iter()
            .map(|s| {
                let w = (w_m * s).round().max(1.0) as i32;
                let h = (h_m * s).round().max(1.0) as i32;
                sample(w, h, &shape)
            })
            .collect();
        Steps {
            size_m: (w_m, h_m),
            sprites,
        }
    }

    /// The step for `scale` pixels per metre: the nearest by ratio, or
    /// nothing when the thing would be under half the smallest step.
    pub fn pick(&self, scale: f64) -> Option<&Sprite> {
        if scale < SCALES[0] * 0.5 {
            return None;
        }
        let i = SCALES
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (scale / a.1)
                    .ln()
                    .abs()
                    .total_cmp(&(scale / b.1).ln().abs())
            })
            .map_or(0, |(i, _)| i);
        self.sprites.get(i)
    }
}

/// Sample a shape function at each pixel's centre: `shape(u, v, w, h)`
/// with `u, v` in `0..1` returns a palette index or [`Sprite::CLEAR`].
fn sample(w: i32, h: i32, shape: &impl Fn(f64, f64, i32, i32) -> u8) -> Sprite {
    let mut s = Sprite::blank(w, h);
    for y in 0..h {
        for x in 0..w {
            let u = (x as f64 + 0.5) / w as f64;
            let v = (y as f64 + 0.5) / h as f64;
            s.set(x, y, shape(u, v, w, h));
        }
    }
    s
}

const C: u8 = Sprite::CLEAR;

/// A marker post: white, a red band at the top.
fn post(_u: f64, v: f64, _w: i32, _h: i32) -> u8 {
    if v < 0.25 {
        RED
    } else {
        WHITE
    }
}

/// A boulder: a rounded lump, lit from the top left.
fn rock(u: f64, v: f64, _w: i32, _h: i32) -> u8 {
    let (dx, dy) = (u - 0.5, (v - 1.0) / 1.9);
    let r = dx * dx + dy * dy;
    if r > 0.25 {
        C
    } else if dx < -0.05 && dy < -0.25 {
        GRAVEL
    } else {
        BROWN
    }
}

/// A roadside tree: a trunk under a round crown.
fn tree(u: f64, v: f64, _w: i32, _h: i32) -> u8 {
    let (dx, dy) = (u - 0.5, v - 0.38);
    if dx * dx + dy * dy * 0.9 < 0.13 {
        if dx < -0.1 && dy < -0.08 {
            GRASS_LT
        } else {
            GRASS_DK
        }
    } else if v > 0.6 && (u - 0.5).abs() < 0.09 {
        BROWN
    } else {
        C
    }
}

/// A bend-warning board on a post: a yellow board with a black chevron
/// pointing right (mirrored for a left bend).
fn chevron(u: f64, v: f64, _w: i32, _h: i32) -> u8 {
    if v < 0.6 {
        let (x, y) = (u, v / 0.6);
        // A ">" from two strokes meeting at the right.
        let arm = (x - 0.25 - 0.45 * (1.0 - (2.0 * y - 1.0).abs())).abs();
        if arm < 0.13 && x > 0.12 && x < 0.88 {
            INK
        } else {
            YELLOW
        }
    } else if (u - 0.5).abs() < 0.1 {
        GRAVEL
    } else {
        C
    }
}

/// A car's colours.
#[derive(Clone, Copy)]
struct Livery {
    body: u8,
    shade: u8,
    lights: u8,
}

/// A car from behind in `livery`. `skew` is
/// how many pixels the roof leans over (negative left) and `side` shows a
/// strip of the car's flank on the left (-1) or right (1): a drift.
fn car(u: f64, v: f64, w: i32, livery: Livery, skew: f64, side: i32) -> u8 {
    let Livery {
        body,
        shade,
        lights,
    } = livery;
    // Lean the top: shift u by up to `skew` pixels at v = 0.
    let u = u - skew * (1.0 - v) / w as f64;
    let (l, r) = (0.04, 0.96);
    if v > 0.86 {
        return if u > 0.02 && u < 0.98 { INK } else { C };
    }
    if v > 0.66 && (u < 0.17 || u > 0.83) && u > l && u < r {
        return INK; // tyres
    }
    if v >= 0.34 && u > l && u < r {
        let flank = match side {
            -1 => u < 0.16,
            1 => u > 0.84,
            _ => false,
        };
        if flank {
            return shade;
        }
        if v > 0.44 && v < 0.56 && (u < 0.22 || u > 0.78) {
            return lights;
        }
        return if v > 0.74 { shade } else { body };
    }
    // Cabin: narrower towards the roof.
    let inset = 0.17 + 0.12 * (0.34 - v) / 0.34;
    if v < 0.34 && u > inset && u < 1.0 - inset {
        return if v > 0.1 && u > inset + 0.06 && u < 1.0 - inset - 0.06 {
            ASPHALT_LT
        } else {
            body
        };
    }
    C
}

const PLAYER: Livery = Livery {
    body: RED,
    shade: RED_DK,
    lights: YELLOW,
};
const TRAFFIC: Livery = Livery {
    body: WATER,
    shade: INK,
    lights: RED,
};
/// The ghost of the player's best: pale, and see-through by dithering.
const GHOST: Livery = Livery {
    body: ICE,
    shade: WATER,
    lights: WHITE,
};

/// `s` with every other pixel cleared in a checkerboard: how a 16-colour
/// raster with no blending draws something translucent.
fn checkerboard(mut s: Sprite) -> Sprite {
    for y in 0..s.h {
        for x in 0..s.w {
            if (x + y) & 1 == 1 {
                s.set(x, y, C);
            }
        }
    }
    s
}

/// The player car's frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarFrame {
    /// Driving straight.
    Straight = 0,
    /// Steering left.
    LeanLeft = 1,
    /// Steering right.
    LeanRight = 2,
    /// Drifting with the nose left.
    DriftLeft = 3,
    /// Drifting with the nose right.
    DriftRight = 4,
}

/// The player car's size, pixels.
pub const CAR_W: i32 = 30;
/// The player car's height, pixels.
pub const CAR_H: i32 = 20;

/// Every sprite the game draws.
#[derive(Clone, Debug)]
pub struct Art {
    /// Marker posts.
    pub post: Steps,
    /// Boulders.
    pub rock: Steps,
    /// Roadside trees (decor).
    pub tree: Steps,
    /// Bend-warning boards; pointing right, mirrored for left.
    pub chevron: Steps,
    /// Traffic cars from behind.
    pub traffic: Steps,
    /// The ghost of the player's best run: a car, pale and dithered.
    pub ghost: Steps,
    /// The player's car, by [`CarFrame`].
    pub car: [Sprite; 5],
}

impl Art {
    /// Draw everything. Called once.
    pub fn new() -> Art {
        let frame = |skew: f64, side: i32| {
            sample(CAR_W, CAR_H, &|u, v, w, _h| {
                car(u, v, w, PLAYER, skew, side)
            })
        };
        Art {
            post: Steps::new(0.4, 1.4, post),
            rock: Steps::new(2.2, 1.3, rock),
            tree: Steps::new(4.0, 7.0, tree),
            chevron: Steps::new(1.6, 2.4, chevron),
            traffic: Steps::new(2.0, 1.5, |u, v, w, _h| car(u, v, w, TRAFFIC, 0.0, 0)),
            ghost: {
                let mut g = Steps::new(2.0, 1.5, |u, v, w, _h| car(u, v, w, GHOST, 0.0, 0));
                g.sprites = g.sprites.into_iter().map(checkerboard).collect();
                g
            },
            car: [
                frame(0.0, 0),
                frame(-2.0, 0),
                frame(2.0, 0),
                frame(-4.0, 1),
                frame(4.0, -1),
            ],
        }
    }
}

impl Default for Art {
    fn default() -> Self {
        Art::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_grow_and_pick_the_nearest() {
        let art = Art::new();
        let s = &art.traffic;
        assert_eq!(s.sprites.len(), SCALES.len());
        assert!(s.sprites.windows(2).all(|p| p[1].w > p[0].w));
        assert!(s.pick(0.3).is_none(), "too far to draw");
        assert_eq!(s.pick(1.0).unwrap().w, 2);
        assert_eq!(s.pick(15.0).unwrap().w, 30);
        assert_eq!(s.pick(100.0).unwrap().w, 30, "nearest is the largest");
        // Between 6.6 and 10 the ratio decides: 8 is nearer 6.6 (x1.21)
        // than 10 (x1.25).
        assert_eq!(s.pick(8.0).unwrap(), &s.sprites[4]);
    }

    #[test]
    fn sprites_use_only_palette_indices() {
        let art = Art::new();
        let all = [
            &art.post,
            &art.rock,
            &art.tree,
            &art.chevron,
            &art.traffic,
            &art.ghost,
        ];
        for k in all {
            for s in &k.sprites {
                assert!(s.px.iter().all(|&c| c == Sprite::CLEAR || c < 16));
                assert!(s.px.iter().any(|&c| c != Sprite::CLEAR), "empty sprite");
            }
        }
        for f in &art.car {
            assert_eq!((f.w, f.h), (CAR_W, CAR_H));
            assert!(f.px.iter().all(|&c| c == Sprite::CLEAR || c < 16));
        }
    }

    #[test]
    fn the_ghost_is_a_pale_checkerboard() {
        let art = Art::new();
        for s in &art.ghost.sprites {
            for y in 0..s.h {
                for x in 0..s.w {
                    let c = s.at(x, y);
                    if (x + y) & 1 == 1 {
                        assert_eq!(c, Sprite::CLEAR, "see-through on odd cells");
                    }
                    assert!(
                        c == Sprite::CLEAR || [ICE, WATER, WHITE, INK, ASPHALT_LT].contains(&c)
                    );
                }
            }
        }
        let big = art.ghost.sprites.last().unwrap();
        assert!(big.px.contains(&ICE), "the body is ghost-blue");
        assert!(!big.px.contains(&RED), "never the player's red");
    }

    #[test]
    fn car_frames_differ_and_lean_their_way() {
        let art = Art::new();
        for i in 0..5 {
            for j in i + 1..5 {
                assert_ne!(art.car[i], art.car[j], "frames {i} and {j}");
            }
        }
        // The roof's leftmost pixel moves left when leaning left.
        let left_edge = |s: &Sprite| (0..s.w).find(|&x| s.at(x, 2) != Sprite::CLEAR).unwrap();
        let straight = left_edge(&art.car[CarFrame::Straight as usize]);
        assert!(left_edge(&art.car[CarFrame::LeanLeft as usize]) < straight);
        assert!(left_edge(&art.car[CarFrame::LeanRight as usize]) > straight);
    }
}
