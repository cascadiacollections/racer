//! Roadside things: each biome's flora, the marker posts, rocks and the
//! bend boards, drawn at every scale step.
//!
//! Each is a shape in metres, sampled at every pixel centre of a step
//! ([`Px`]), lit from the upper left like the cars: a lit tone, a body
//! tone and a shadow tone, with a checkerboard where two meet. Steps big
//! enough to carry one then get an edge: dark on the shadow side, so a
//! thing stands off a verge of its own colour. The smallest steps are the
//! same shapes with the detail switched off by size, never a big sprite
//! shrunk. Sizes (and so the sim's footprints) are as before.

use super::{Steps, SCALES};
use crate::palette::*;
use crate::raster::Sprite;

const C: u8 = Sprite::CLEAR;

/// One pixel being painted: its centre in metres from the sprite's top
/// left, its pixel coordinates, and the step's size.
#[derive(Clone, Copy, Debug)]
pub struct Px {
    /// Metres right of the left edge.
    pub x: f64,
    /// Metres down from the top.
    pub y: f64,
    /// Pixel column.
    pub px: i32,
    /// Pixel row.
    pub py: i32,
    /// Sprite width, pixels.
    pub w: i32,
    /// Pixels per metre.
    pub s: f64,
}

impl Px {
    /// Checkerboard: `a` or `b` by the pixel's parity.
    fn check(&self, a: u8, b: u8) -> u8 {
        if (self.px + self.py) & 1 == 0 {
            a
        } else {
            b
        }
    }

    /// A tone from light `l` (about -1 shadow to 1 lit): `[lit, body,
    /// shadow]` with checkerboards between.
    fn tone(&self, l: f64, t: [u8; 3]) -> u8 {
        if l > 0.5 {
            t[0]
        } else if l > 0.2 {
            self.check(t[0], t[1])
        } else if l > -0.35 {
            t[1]
        } else if l > -0.6 {
            self.check(t[1], t[2])
        } else {
            t[2]
        }
    }

    /// Light on a ball centred `(cx, cy)` of radius `r` at this pixel,
    /// from the upper left; `None` outside it.
    fn ball(&self, cx: f64, cy: f64, r: f64) -> Option<f64> {
        let (dx, dy) = ((self.x - cx) / r, (self.y - cy) / r);
        (dx * dx + dy * dy <= 1.0).then(|| -0.7 * dx - 0.8 * dy)
    }
}

/// Draw `shape` at every step for a thing `w_m` by `h_m` metres, then
/// edge each step at least `edge_from` pixels wide: a pixel at the
/// silhouette takes `edge(colour)` when it is on the shadow side (its
/// right or lower neighbour clear).
fn draw(
    w_m: f64,
    h_m: f64,
    shape: impl Fn(&Px) -> u8,
    edge_from: i32,
    edge: impl Fn(u8) -> u8,
) -> Steps {
    let sprites = SCALES
        .iter()
        .map(|&s| {
            let w = (w_m * s).round().max(1.0) as i32;
            let h = (h_m * s).round().max(1.0) as i32;
            let mut out = Sprite::blank(w, h);
            for py in 0..h {
                for px in 0..w {
                    let p = Px {
                        x: (px as f64 + 0.5) / w as f64 * w_m,
                        y: (py as f64 + 0.5) / h as f64 * h_m,
                        px,
                        py,
                        w,
                        s,
                    };
                    out.set(px, py, shape(&p));
                }
            }
            if w >= edge_from {
                let base = out.clone();
                for y in 0..h {
                    for x in 0..w {
                        let c = base.at(x, y);
                        if c == C {
                            continue;
                        }
                        let shadow_side =
                            base.at(x + 1, y) == C || base.at(x, y + 1) == C || x + 1 == w;
                        if shadow_side {
                            out.set(x, y, edge(c));
                        }
                    }
                }
            }
            out
        })
        .collect();
    Steps {
        size_m: (w_m, h_m),
        sprites,
    }
}

/// The edge every thing gets: ink, but light things go to their shade.
fn ink_edge(_c: u8) -> u8 {
    INK
}

// ------------------------------------------------------------------ posts

/// A marker post: white with a grey shadow side, a black band near the
/// top holding a red reflector.
pub fn post() -> Steps {
    draw(
        0.4,
        1.4,
        |p| {
            let right = p.px + 1 == p.w && p.w >= 2;
            if p.y < 0.12 && p.w >= 3 {
                return if right { ASPHALT_LT } else { WHITE };
            }
            if p.y < 0.4 {
                if p.w < 3 {
                    return RED;
                }
                let mid = p.px > 0 && p.px + 1 < p.w;
                return if mid && p.y > 0.18 && p.y < 0.34 {
                    RED
                } else {
                    INK
                };
            }
            if right {
                ASPHALT_LT
            } else {
                WHITE
            }
        },
        i32::MAX,
        ink_edge,
    )
}

/// A snow pole (ALPINE's marker post): red and white bands, shaded.
pub fn snow_post() -> Steps {
    draw(
        0.4,
        1.4,
        |p| {
            let red = ((p.y / 1.4 * 5.0) as i32) % 2 == 0;
            let right = p.px + 1 == p.w && p.w >= 2;
            match (red, right) {
                (true, false) => RED,
                (true, true) => RED_DK,
                (false, false) => WHITE,
                (false, true) => ASPHALT_LT,
            }
        },
        i32::MAX,
        ink_edge,
    )
}

/// A city bollard (NIGHT CITY's marker post): a black cap over a yellow
/// reflector band on a white body.
pub fn city_post() -> Steps {
    draw(
        0.4,
        1.4,
        |p| {
            let right = p.px + 1 == p.w && p.w >= 2;
            if p.y < 0.1 && p.w >= 3 {
                INK
            } else if p.y < 0.36 {
                YELLOW
            } else if p.y < 0.44 && p.w >= 2 {
                INK
            } else if right {
                ASPHALT_LT
            } else {
                WHITE
            }
        },
        i32::MAX,
        ink_edge,
    )
}

/// A boulder: two lumps, lit top left, a crack down the big one.
pub fn rock() -> Steps {
    draw(
        2.2,
        1.3,
        |p| {
            let a = p.ball(0.95, 1.35, 0.95);
            let b = p.ball(1.6, 1.4, 0.65);
            let Some(l) = a.or(b) else {
                return C;
            };
            // The crack: a dark zigzag on the big lump.
            if p.w >= 14 && a.is_some() && b.is_none() {
                let zig = 0.9 + 0.12 * ((p.y * 6.0).sin());
                if (p.x - zig).abs() < 0.5 / p.s && p.y > 0.75 {
                    return INK;
                }
            }
            p.tone(l, [GRAVEL, BROWN, INK])
        },
        6,
        ink_edge,
    )
}

/// A bend-warning board on a grey post: yellow, a black border, black
/// chevrons pointing right (two on a big board, one on a small); mirrored
/// for a left bend.
pub fn chevron() -> Steps {
    draw(
        1.6,
        2.4,
        |p| {
            let board = 1.45;
            if p.y >= board {
                let (x0, x1) = (0.68, 0.92);
                if p.x >= x0 && p.x < x1 {
                    return if p.x > 0.8 && p.w >= 6 {
                        ASPHALT_DK
                    } else {
                        ASPHALT_LT
                    };
                }
                return C;
            }
            let px1 = 1.0 / p.s;
            let border =
                p.w >= 8 && (p.x < px1 || p.x > 1.6 - px1 || p.y < px1 || p.y > board - px1 * 1.5);
            if border {
                return INK;
            }
            // Chevrons: ">" strokes, x offset by the distance from the
            // board's middle row.
            let n = if p.w >= 14 { 2 } else { 1 };
            let span = 1.6 / n as f64;
            let local = p.x.rem_euclid(span) / span;
            let dy = ((p.y / board) - 0.5).abs() * 2.0;
            let arm = (local - 0.25 - 0.4 * (1.0 - dy)).abs();
            if arm < 0.17 && dy < 0.8 {
                INK
            } else {
                YELLOW
            }
        },
        i32::MAX,
        ink_edge,
    )
}

// ------------------------------------------------------------------ flora

/// The meadow's tree: a broad crown with a lumpy edge over a trunk, lit
/// top left; from mid sizes up the crown breaks into leaf clumps, each
/// lit on its own.
pub fn tree() -> Steps {
    draw(
        4.0,
        7.0,
        |p| {
            let (cx, cy, rx, ry) = (2.0, 2.65, 1.92, 2.5);
            let (dx, dy) = ((p.x - cx) / rx, (p.y - cy) / ry);
            let r = (dx * dx + dy * dy).sqrt();
            let a = dy.atan2(dx);
            let edge = 1.0 - 0.07 * (1.0 + (a * 7.0).sin()) * f64::from(p.w >= 10);
            if r <= edge {
                let mut l = 0.15 - 0.7 * dx - 0.8 * dy;
                if p.w >= 17 {
                    // Leaf clumps: a grid of small balls, rows staggered.
                    let size = 0.85;
                    let row = (p.y / size).floor();
                    let shift = if row as i64 % 2 == 0 { 0.0 } else { size / 2.0 };
                    let col = ((p.x + shift) / size).floor();
                    let (ux, uy) = (
                        (p.x + shift - (col + 0.5) * size) / (size / 2.0),
                        (p.y - (row + 0.5) * size) / (size / 2.0),
                    );
                    // Each clump shades its own lower right: a crescent.
                    l -= 0.55 * (0.7 * ux + 0.8 * uy - 0.35).max(0.0);
                }
                // A dark rim all round, so the lit side stands off a verge
                // of the same green.
                if p.w >= 6 && r > edge - 1.0 / (p.s * ry) {
                    return GRASS_DK;
                }
                return p.tone(l, [GRASS_LT, GRASS_DK, INK]);
            }
            // Trunk, flaring at the foot, darker on its right.
            let half = 0.24 + if p.y > 6.6 { 0.12 } else { 0.0 };
            if p.y > 4.0 && (p.x - 2.0).abs() < half {
                return if p.x > 2.05 && p.w >= 10 {
                    p.check(BROWN, INK)
                } else {
                    BROWN
                };
            }
            C
        },
        10,
        ink_edge,
    )
}

/// COAST's palm: a ringed trunk curving up to a crown of five drooping
/// fronds, serrated at the bigger sizes, over a cluster of coconuts.
pub fn palm() -> Steps {
    draw(
        4.0,
        8.0,
        |p| {
            let (cx, cy) = (2.1, 1.7);
            // Fronds: (direction, rise, length). Each arcs up from the
            // crown and droops, thinning to its tip; its underside is in
            // shadow and, big enough, cut into leaflets.
            let fronds: [(f64, f64, f64); 5] = [
                (1.0, 1.2, 1.85),
                (-1.0, 1.2, 1.85),
                (1.0, 0.3, 1.5),
                (-1.0, 0.3, 1.5),
                (0.3, 1.7, 0.9),
            ];
            for (dir, rise, len) in fronds {
                let t = (p.x - cx) * dir.signum() / (len * dir.abs());
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let mid = cy - rise * t + 1.9 * t * t;
                let half = 0.3 * (1.0 - t).sqrt() + 0.5 / p.s;
                let d = p.y - mid;
                if d.abs() < half {
                    return if d > 0.0 { p.check(HILLS, INK) } else { HILLS };
                }
                let leaflet = p.s >= 6.0 && d > 0.0 && d < half + 0.35 * (1.0 - t);
                if leaflet && (t * len * 5.0).fract() < 0.45 {
                    return if (t * len * 5.0).fract() < 0.2 {
                        HILLS
                    } else {
                        INK
                    };
                }
            }
            if ((p.x - cx).powi(2) + (p.y - cy - 0.3).powi(2)) < 0.1 {
                return if p.x > cx { INK } else { BROWN };
            }
            // Trunk: from the foot at x 1.7, bowing right then back up
            // to the crown.
            let up = (8.0 - p.y) / (8.0 - cy);
            let centre = 1.7 + 0.4 * up + 0.35 * (up * std::f64::consts::PI).sin();
            let half = 0.2 + 0.06 * (1.0 - up);
            if p.y > cy && (p.x - centre).abs() < half {
                let ring = p.s >= 4.0 && ((p.y * 2.6) as i32) % 2 == 0;
                return if ring {
                    GRAVEL
                } else if p.x > centre + 0.04 && p.w >= 10 {
                    p.check(BROWN, INK)
                } else {
                    BROWN
                };
            }
            C
        },
        10,
        ink_edge,
    )
}

/// ALPINE's pine: four sawtooth tiers, snow along each tier's top,
/// shaded on the right; a short trunk.
pub fn pine() -> Steps {
    draw(
        3.4,
        8.0,
        |p| {
            let dx = p.x - 1.7;
            if p.y > 6.9 {
                return if dx.abs() < 0.22 { BROWN } else { C };
            }
            let tiers = [
                (0.0, 2.5, 0.75),
                (1.4, 4.0, 1.15),
                (2.9, 5.6, 1.45),
                (4.3, 7.0, 1.7),
            ];
            for (top, bottom, half) in tiers {
                if p.y < top || p.y >= bottom {
                    continue;
                }
                let k = (p.y - top) / (bottom - top);
                // The sawtooth: the edge steps back in and out.
                let saw = if p.s >= 4.0 {
                    0.12 * ((k * 6.0).fract())
                } else {
                    0.0
                };
                let reach = half * k + saw;
                if dx.abs() >= reach {
                    continue;
                }
                let snow = k < 0.35 && dx.abs() > reach - 0.35 && dx < 0.3;
                if snow && p.s >= 2.0 {
                    return if dx > 0.0 { ICE } else { WHITE };
                }
                let l = -dx / half - 0.2;
                return p.tone(l, [HILLS, HILLS, INK]);
            }
            C
        },
        10,
        |c| if c == WHITE || c == ICE { ICE } else { INK },
    )
}

/// DESERT DUSK's saguaro: a rounded ribbed column with two arms, lit
/// warm on its left by the low sun, a red flower at the top.
pub fn cactus() -> Steps {
    draw(
        2.4,
        5.0,
        |p| {
            // Column and arms as capsules: (x0, y0, x1, y1, r).
            let parts = [
                (1.2, 0.35, 1.2, 5.0, 0.32),
                (0.45, 1.6, 0.45, 2.9, 0.2),
                (0.45, 2.9, 1.0, 2.9, 0.2),
                (1.95, 1.0, 1.95, 2.3, 0.2),
                (1.4, 2.3, 1.95, 2.3, 0.2),
            ];
            for (k, (x0, y0, x1, y1, r)) in parts.into_iter().enumerate() {
                let (vx, vy) = (x1 - x0, y1 - y0);
                let t = (((p.x - x0) * vx + (p.y - y0) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
                let (cx, cy) = (x0 + vx * t, y0 + vy * t);
                let (dx, dy) = (p.x - cx, p.y - cy);
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                if k == 0 && p.y < 0.5 && dx.abs() < 0.12 && p.s >= 6.0 {
                    return RED;
                }
                // A rib down the column's right.
                if k == 0 && p.s >= 6.0 && (dx - 0.12).abs() < 0.5 / p.s {
                    return p.check(HILLS, INK);
                }
                let l = -dx / r - 0.1;
                return if l > 0.6 {
                    p.check(YELLOW, HILLS)
                } else {
                    p.tone(l, [HILLS, HILLS, INK])
                };
            }
            C
        },
        6,
        ink_edge,
    )
}

/// NIGHT CITY's street light: a grey post, wider at the foot, an arm out
/// to the left (over the road when it stands right of it; mirrored on the
/// left) and a lit lamp throwing a dithered cone of light below it.
pub fn lamp() -> Steps {
    draw(
        3.0,
        8.0,
        |p| {
            let post = (p.x - 2.55).abs() < 0.13 + if p.y > 7.4 { 0.12 } else { 0.0 };
            if post && p.y > 0.3 {
                return if p.x > 2.6 { ASPHALT_DK } else { ASPHALT_LT };
            }
            // The arm, curving from the post's top.
            if p.y > 0.25 && p.y < 0.25 + 1.0 / p.s.max(1.0) + 0.1 && p.x > 0.6 && p.x < 2.7 {
                return ASPHALT_LT;
            }
            // The lamp head: a dark hood over the light.
            if p.x > 0.35 && p.x < 1.25 {
                if p.y >= 0.2 && p.y < 0.45 {
                    return INK;
                }
                if p.y >= 0.45 && p.y < 0.65 {
                    return if p.x > 0.55 && p.x < 1.05 {
                        WHITE
                    } else {
                        YELLOW
                    };
                }
            }
            // The cone of light: sparse, fading out a third of the way
            // down.
            if p.y > 0.65 && p.s >= 4.0 {
                let k = (p.y - 0.65) / 2.6;
                let half = 0.35 + 0.4 * k;
                if k < 1.0 && (p.x - 0.8).abs() < half {
                    let frac = (4.0 * (1.0 - k)).ceil() as u8;
                    if crate::raster::dither_on(p.px, p.py, frac) && p.x < 1.45 {
                        return YELLOW;
                    }
                }
            }
            C
        },
        i32::MAX,
        ink_edge,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_is_painted_and_grows() {
        for (name, k) in [
            ("post", post()),
            ("snow", snow_post()),
            ("city", city_post()),
            ("rock", rock()),
            ("chevron", chevron()),
            ("tree", tree()),
            ("palm", palm()),
            ("pine", pine()),
            ("cactus", cactus()),
            ("lamp", lamp()),
        ] {
            for s in &k.sprites {
                assert!(s.px.iter().any(|&c| c != C), "{name} {}x{}", s.w, s.h);
                assert!(s.px.iter().all(|&c| c == C || c < 16));
            }
            assert!(k.sprites.windows(2).all(|p| p[1].h >= p[0].h), "{name}");
        }
    }

    #[test]
    fn big_things_are_edged_and_shaded() {
        let t = tree();
        let big = t.sprites.last().unwrap();
        for c in [GRASS_LT, GRASS_DK, INK, BROWN] {
            assert!(big.px.contains(&c), "{c}");
        }
        // The lit side is on the left: more light tone left of centre.
        let lit = |x0: i32, x1: i32| {
            (x0..x1)
                .flat_map(|x| (0..big.h).map(move |y| (x, y)))
                .filter(|&(x, y)| big.at(x, y) == GRASS_LT)
                .count()
        };
        assert!(lit(0, big.w / 2) > lit(big.w / 2, big.w));
        // A board's chevron is ink on yellow, two on the biggest board.
        let b = chevron();
        let big = b.sprites.last().unwrap();
        assert!(big.px.contains(&INK) && big.px.contains(&YELLOW));
    }
}
