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
    lamp_lit([YELLOW, WHITE, ASPHALT_LT, ASPHALT_DK])
}

/// MONSTER CITY's street light: the same, black, throwing a green light.
pub fn green_lamp() -> Steps {
    lamp_lit([SKY_LO, ICE, ASPHALT_DK, INK])
}

/// A street light in `[light, its core, the post lit, the post shaded]`.
fn lamp_lit([light, core, lit, shade]: [u8; 4]) -> Steps {
    draw(
        3.0,
        8.0,
        |p| {
            let post = (p.x - 2.55).abs() < 0.13 + if p.y > 7.4 { 0.12 } else { 0.0 };
            if post && p.y > 0.3 {
                return if p.x > 2.6 { shade } else { lit };
            }
            // The arm, curving from the post's top.
            if p.y > 0.25 && p.y < 0.25 + 1.0 / p.s.max(1.0) + 0.1 && p.x > 0.6 && p.x < 2.7 {
                return lit;
            }
            // The lamp head: a dark hood over the light.
            if p.x > 0.35 && p.x < 1.25 {
                if p.y >= 0.2 && p.y < 0.45 {
                    return INK;
                }
                if p.y >= 0.45 && p.y < 0.65 {
                    return if p.x > 0.55 && p.x < 1.05 {
                        core
                    } else {
                        light
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
                        return light;
                    }
                }
            }
            C
        },
        i32::MAX,
        ink_edge,
    )
}

// --------------------------------------------------------------- Halloween
//
// The twins' roadside things (DESIGN §5.2): the same sizes as the things
// they stand in for, so nothing about where the sim's posts and rocks are,
// or how big, changes.

/// A capsule from `(x0, y0)` to `(x1, y1)` of radius `r`: the light on it
/// at this pixel (lit upper left), or `None` outside it.
fn capsule(p: &Px, (x0, y0, x1, y1, r): (f64, f64, f64, f64, f64)) -> Option<f64> {
    let (vx, vy) = (x1 - x0, y1 - y0);
    let len2 = (vx * vx + vy * vy).max(1e-9);
    let t = (((p.x - x0) * vx + (p.y - y0) * vy) / len2).clamp(0.0, 1.0);
    let (dx, dy) = (p.x - x0 - vx * t, p.y - y0 - vy * t);
    // Thin limbs stay at least a pixel wide at every size.
    let r = r.max(0.5 / p.s);
    (dx * dx + dy * dy <= r * r).then(|| -(dx / r) * 0.8 - (dy / r) * 0.3)
}

/// A jack-o'-lantern on a dark stake: the marker post's Halloween paint,
/// its size. The pumpkin is red going orange (a red and yellow
/// checkerboard) on its lit side, its face cut out and glowing yellow.
pub fn lantern_post() -> Steps {
    draw(
        0.4,
        1.4,
        |p| {
            if p.y < 0.5 {
                if p.w < 3 {
                    return if p.y < 0.25 { YELLOW } else { RED };
                }
                // The stem.
                if p.y < 0.08 {
                    return if p.px * 2 + 1 == p.w || p.px * 2 == p.w {
                        BROWN
                    } else {
                        C
                    };
                }
                let edge = p.px == 0 || p.px + 1 == p.w;
                let face = !edge && (p.y > 0.18 && p.y < 0.24 || p.y > 0.32 && p.y < 0.4);
                return if face {
                    YELLOW
                } else if edge {
                    RED_DK
                } else {
                    p.check(RED, YELLOW)
                };
            }
            let right = p.px + 1 == p.w && p.w >= 2;
            if right {
                INK
            } else {
                BROWN
            }
        },
        i32::MAX,
        ink_edge,
    )
}

/// Two pumpkins, a big jack-o'-lantern and a small plain one: a rock's
/// Halloween paint (PUMPKIN PATCH, BLOOD MOON MESA), its size.
pub fn pumpkins() -> Steps {
    draw(
        2.2,
        1.3,
        |p| {
            let a = p.ball(0.9, 0.78, 0.62).map(|l| (l, 0.9));
            let b = p.ball(1.72, 0.98, 0.38).map(|l| (l, 1.72));
            let Some((l, cx)) = a.or(b) else {
                // Stems.
                let stem = |x: f64, top: f64| (p.x - x).abs() < 0.07 && p.y > top;
                return if stem(0.92, 0.06) && p.y < 0.2 || stem(1.74, 0.5) && p.y < 0.62 {
                    BROWN
                } else {
                    C
                };
            };
            // The big one's face, glowing: two eyes and a jagged grin.
            if a.is_some() && p.w >= 9 {
                let (dx, dy) = (p.x - 0.9, p.y - 0.78);
                let eye = (dx.abs() - 0.22).abs() < 0.09 && (-0.24..-0.06).contains(&dy);
                let grin = dx.abs() < 0.36
                    && dy > 0.1
                    && dy
                        < 0.24
                            + if (dx * 9.0).rem_euclid(1.0) < 0.5 {
                                0.0
                            } else {
                                -0.08
                            };
                if eye || grin {
                    return YELLOW;
                }
            }
            // Ribs, shaded on their right.
            if p.w >= 14 && ((p.x - cx) * 5.0).rem_euclid(1.0) < 0.18 {
                return if l > 0.2 { RED } else { RED_DK };
            }
            if l > 0.35 {
                p.check(YELLOW, RED)
            } else {
                p.tone(l, [RED, RED, RED_DK])
            }
        },
        6,
        |_| INK,
    )
}

/// A gravestone and a leaning cross: a rock's Halloween paint (GHOST
/// COVE, HAUNTED PINES, MONSTER CITY), its size. Grey stone lit from the
/// left on a low mound.
pub fn gravestone() -> Steps {
    draw(
        2.2,
        1.3,
        |p| {
            // The mound.
            if p.y > 1.12 && p.x > 0.15 && p.x < 2.05 {
                return BROWN;
            }
            // The headstone: a slab with a round top.
            let (x0, x1, top) = (0.3, 1.25, 0.22);
            let r = (x1 - x0) / 2.0;
            let cx = (x0 + x1) / 2.0;
            let in_slab = p.x >= x0 && p.x < x1 && p.y >= top + r;
            let in_top = (p.x - cx).powi(2) + (p.y - top - r).powi(2) <= r * r;
            if in_slab || in_top {
                // Its inscription, a cross, from mid sizes.
                if p.w >= 14 {
                    let (dx, dy) = (p.x - cx, p.y - 0.62);
                    if (dx.abs() < 0.05 && dy.abs() < 0.22) || (dy.abs() < 0.04 && dx.abs() < 0.14)
                    {
                        return INK;
                    }
                }
                let l = -(p.x - cx) / r - 0.1;
                return p.tone(l, [ASPHALT_LT, ASPHALT_DK, INK]);
            }
            // A wooden cross, leaning right.
            let lean = (1.12 - p.y) * 0.18;
            let (dx, dy) = (p.x - 1.72 - lean, p.y - 0.62);
            if (dx.abs() < 0.07 && (-0.3..0.5).contains(&dy))
                || (dy.abs() < 0.06 && dx.abs() < 0.22)
            {
                return if dx > 0.02 { INK } else { BROWN };
            }
            C
        },
        6,
        ink_edge,
    )
}

/// PUMPKIN PATCH's scarecrow: a sack head under a black pointed hat, arms
/// out on a crossbar in a red checked shirt, straw at the cuffs, on a pole.
pub fn scarecrow() -> Steps {
    draw(
        3.6,
        6.4,
        |p| {
            let cx = 1.8;
            let dx = p.x - cx;
            // The hat: a brim and a bent cone.
            if p.y > 0.95 && p.y < 1.15 && dx.abs() < 0.75 {
                return INK;
            }
            if p.y <= 0.95 && dx.abs() < (p.y - 0.1).max(0.0) * 0.45 + 0.04 {
                return if dx > 0.1 { INK } else { HILLS };
            }
            // The head: a sack, two stitched eyes and a mouth.
            if let Some(l) = p.ball(cx, 1.55, 0.45) {
                if p.w >= 14 {
                    let (ex, ey) = (dx.abs(), p.y - 1.5);
                    if (ex - 0.17).abs() < 0.07 && ey.abs() < 0.07 {
                        return INK;
                    }
                    if p.y > 1.7 && p.y < 1.77 && ex < 0.22 {
                        return INK;
                    }
                }
                return p.tone(l, [GRAVEL, GRAVEL, BROWN]);
            }
            // The shirt: shoulders to the cuffs, and a body, checked.
            let arm = p.y > 2.05 && p.y < 2.6 && dx.abs() < 1.45;
            let body = p.y >= 2.05 && p.y < 3.7 && dx.abs() < 0.48 + (p.y - 2.05) * 0.06;
            if arm || body {
                // Straw at the cuffs and the hem.
                if (arm && dx.abs() > 1.3) || (body && !arm && p.y > 3.55) {
                    return if (p.px + p.py / 2) % 2 == 0 {
                        YELLOW
                    } else {
                        GRAVEL
                    };
                }
                let plaid = p.s >= 6.0 && (((p.x * 4.0) as i32 + (p.y * 4.0) as i32) % 3 == 0);
                let l = -dx * 0.8 - (p.y - 2.3) * 0.3;
                return if plaid {
                    INK
                } else {
                    p.tone(l, [RED, RED, RED_DK])
                };
            }
            // The pole.
            if p.y >= 3.7 && dx.abs() < 0.12 {
                return if dx > 0.02 { INK } else { BROWN };
            }
            C
        },
        10,
        ink_edge,
    )
}

/// GHOST COVE's (and the pumpkin patch's odd) dead tree: a crooked trunk
/// and bare, twisting branches, no leaves.
pub fn dead_tree() -> Steps {
    draw(
        4.0,
        7.0,
        |p| {
            let limbs = [
                (2.1, 7.0, 1.9, 4.2, 0.26),
                (1.9, 4.2, 2.3, 2.6, 0.17),
                (2.3, 2.6, 3.4, 1.4, 0.1),
                (2.3, 2.6, 1.9, 0.5, 0.09),
                (1.95, 4.0, 0.7, 2.6, 0.12),
                (0.7, 2.6, 0.35, 1.5, 0.07),
                (0.7, 2.6, 1.2, 1.7, 0.06),
                (2.15, 3.4, 3.0, 3.1, 0.07),
                (3.4, 1.4, 3.75, 1.7, 0.05),
                (1.9, 0.5, 1.5, 0.2, 0.05),
            ];
            for (k, l) in limbs.into_iter().enumerate() {
                if k >= 7 && p.s < 4.0 {
                    break;
                }
                if let Some(light) = capsule(p, l) {
                    return p.tone(light, [BROWN, BROWN, INK]);
                }
            }
            C
        },
        10,
        ink_edge,
    )
}

/// HAUNTED PINES' dead pine: a black trunk with drooping, broken tiers
/// of bare branches, a little snow left on two of them.
pub fn dead_pine() -> Steps {
    draw(
        3.4,
        8.0,
        |p| {
            let dx = p.x - 1.7;
            if dx.abs() < 0.14 + p.y * 0.012 && p.y > 0.2 {
                return if dx > 0.04 { INK } else { HILLS };
            }
            // Tiers: (height, reach left, reach right), drooping.
            let tiers = [
                (1.4, 0.5, 0.7),
                (2.6, 1.1, 0.4),
                (3.9, 0.7, 1.3),
                (5.1, 1.5, 1.1),
                (6.1, 0.4, 1.6),
            ];
            for (k, (y, left, right)) in tiers.into_iter().enumerate() {
                let reach = if dx < 0.0 { left } else { right };
                let droop = y + dx.abs() * 0.45;
                let thick = (0.5 / p.s).max(0.09);
                if dx.abs() <= reach && (p.y - droop).abs() < thick {
                    let tip = dx.abs() > reach - 0.35;
                    return if tip && k % 2 == 1 && p.s >= 2.6 {
                        ICE
                    } else {
                        HILLS
                    };
                }
            }
            C
        },
        10,
        |c| if c == ICE { ICE } else { INK },
    )
}

/// BLOOD MOON MESA's withered saguaro: the cactus gone brown and black,
/// a crow on its arm.
pub fn withered_cactus() -> Steps {
    draw(
        2.4,
        5.0,
        |p| {
            // The crow, perched on the left arm.
            if p.s >= 4.0 {
                let crow = p.ball(0.45, 1.38, 0.17).is_some()
                    || p.ball(0.6, 1.2, 0.1).is_some()
                    || (p.y > 1.3 && p.y < 1.45 && p.x > 0.15 && p.x < 0.3);
                if crow {
                    return if p.x > 0.66 && p.y < 1.25 {
                        YELLOW
                    } else {
                        INK
                    };
                }
            }
            let parts = [
                (1.2, 0.5, 1.25, 5.0, 0.3),
                (0.45, 1.6, 0.42, 2.9, 0.18),
                (0.42, 2.9, 1.0, 2.95, 0.18),
                (1.95, 1.4, 2.0, 2.3, 0.18),
                (1.4, 2.3, 2.0, 2.3, 0.18),
            ];
            for part in parts {
                if let Some(l) = capsule(p, part) {
                    return p.tone(l, [BROWN, BROWN, INK]);
                }
            }
            C
        },
        6,
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
            ("green lamp", green_lamp()),
            ("lantern post", lantern_post()),
            ("pumpkins", pumpkins()),
            ("gravestone", gravestone()),
            ("scarecrow", scarecrow()),
            ("dead tree", dead_tree()),
            ("dead pine", dead_pine()),
            ("withered cactus", withered_cactus()),
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
