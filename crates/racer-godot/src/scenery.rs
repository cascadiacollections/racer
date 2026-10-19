//! The backdrop behind the road, per biome (DESIGN §5.1): what hangs in
//! the sky (clouds are drawn with the sky rows in `render`; stars, a moon
//! or a setting sun here) and two silhouette layers on the horizon, a far
//! one and a near one that scrolls twice as fast (parallax).
//!
//! Every layer is a height per background column and a colour per pixel
//! from small integer arithmetic: one height (two for the far layer's
//! shading) per screen column per frame, then a span of rows, so a biome
//! costs what the meadow's hills cost. Nothing is anti-aliased; the
//! shading is a second palette index, never a blend.
//!
//! The Halloween twins (DESIGN §5.2) have layers of their own, a big moon
//! each, and a few bats: two-frame pixel maps drifting with the sky's
//! parallax, animated by the presentation clock (`anim`), never by
//! anything the sim computes.

use crate::palette::*;
use crate::project::RoadView;
use crate::raster::{dither_on, Canvas};
use crate::season::Look;

/// A triangle wave of period `p`, 0 to 64.
fn tri(x: i32, p: i32) -> i32 {
    let m = x.rem_euclid(p);
    (p / 2 - (m - p / 2).abs()) * 2 * 64 / p
}

/// Height of the meadow's far hills at background column `x`, pixels.
pub fn hill_height(x: i32) -> i32 {
    4 + (tri(x, 157) * 14 + tri(x + 40, 61) * 7 + tri(x + 13, 23) * 2) / 64
}

/// Height of the near, green hills at background column `x`, pixels: lower
/// and rounder than the far range, so the two read as separate distances.
pub fn near_hill_height(x: i32) -> i32 {
    let bump = |x: i32, p: i32| -> i32 {
        // A parabola per period: rounded tops, sharp valleys.
        let m = x.rem_euclid(p) - p / 2;
        (p * p / 4 - m * m) * 64 / (p * p / 4)
    };
    1 + (bump(x, 97) * 5 + bump(x + 31, 53) * 3) / 64
}

/// COAST: a headland now and then, out at sea (0 where there is none).
pub fn headland_height(x: i32) -> i32 {
    (hill_height(x) - 13).max(0)
}

/// COAST: the sea's height above the horizon line, pixels.
pub const SEA: i32 = 6;

/// COAST: the low dunes in front of the sea.
pub fn dune_height(x: i32) -> i32 {
    1 + near_hill_height(x) / 2
}

/// ALPINE: sharp peaks, much taller than the meadow's hills.
pub fn peak_height(x: i32) -> i32 {
    6 + (tri(x, 131) * 20 + tri(x + 50, 47) * 9 + tri(x + 7, 17) * 3) / 64
}

/// ALPINE: snow lies above this height on the peaks, pixels (give or take
/// a pixel, so the line is ragged).
pub const SNOW_LINE: i32 = 17;

/// ALPINE: the pine forest at the foot of the peaks, a saw edge of
/// treetops.
pub fn forest_height(x: i32) -> i32 {
    let m = x.rem_euclid(6);
    2 + (3 - (m - 3).abs()) * 2 + tri(x, 41) * 3 / 64
}

/// DESERT DUSK: flat-topped mesas with steep sides over a low ridge.
pub fn mesa_height(x: i32) -> i32 {
    let mesa = |x: i32, p: i32, top: i32, h: i32| {
        let d = (x.rem_euclid(p) - p / 2).abs();
        if d <= top {
            h
        } else {
            (h - (d - top) * 3).max(0)
        }
    };
    3.max(mesa(x, 151, 22, 16)).max(mesa(x + 60, 97, 9, 10))
}

/// NIGHT CITY: a column's building: `(height, cell, column in the cell)`.
/// Buildings are 9 columns wide with a 1-column gap; a cell's height is a
/// hash of its number.
fn building(x: i32) -> (i32, u32, i32) {
    let cell = x.div_euclid(10);
    let within = x.rem_euclid(10);
    let h = (cell as u32).wrapping_mul(0x9E37_79B1) >> 16;
    let height = if within == 9 { 3 } else { 7 + (h % 23) as i32 };
    (height, h, within)
}

/// NIGHT CITY: the skyline's height, with a mast on the tallest towers.
pub fn skyline_height(x: i32) -> i32 {
    let (h, _, within) = building(x);
    if h >= 26 && within == 4 {
        h + 4
    } else {
        h
    }
}

/// NIGHT CITY: a skyline pixel `d` rows above the horizon at column `x`:
/// the building, a lit window, the mast or its red light.
fn skyline_ink(x: i32, d: i32, top: i32) -> u8 {
    let (h, hash, within) = building(x);
    if d > h {
        // The mast: grey, a red light on top.
        return if d == top { RED } else { ASPHALT_DK };
    }
    let window = within % 2 == 0 && (2..=7).contains(&within) && d % 3 == 0 && d < h - 1;
    let lit = (hash >> ((d / 3) % 11)) ^ (within as u32 >> 1);
    if window && lit & 3 != 0 {
        YELLOW
    } else {
        HILLS
    }
}

/// One silhouette layer.
struct Layer {
    /// Height above the horizon at a background column.
    height: fn(i32) -> i32,
    /// How many times `bg_shift` it scrolls.
    shift: i32,
    /// The ink of a pixel: `(column, rows above the horizon from 1, the
    /// column's height, whether the ground falls away to the right)`.
    ink: fn(i32, i32, i32, bool) -> Option<u8>,
}

/// A look's layers, far first.
fn layers(b: Look) -> &'static [Layer] {
    match b {
        Look::Meadow => &[
            Layer {
                height: hill_height,
                shift: 1,
                ink: |_, _, _, _| Some(HILLS),
            },
            Layer {
                height: near_hill_height,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
        Look::Coast => &[
            Layer {
                height: headland_height,
                shift: 1,
                ink: |_, _, _, _| Some(HILLS),
            },
            Layer {
                height: |_| SEA,
                shift: 1,
                // Glints on the water, in a scatter that scrolls with it.
                ink: |x, d, _, _| {
                    Some(if (x * 7 + d * 5).rem_euclid(19) == 0 {
                        ICE
                    } else {
                        WATER
                    })
                },
            },
            Layer {
                height: dune_height,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
        Look::Alpine => &[
            Layer {
                height: peak_height,
                shift: 1,
                // Rock below the snow line, snow above; the faces that
                // fall away to the right in shade.
                ink: |x, d, _, face| {
                    let snow = d > SNOW_LINE + (x * 5).rem_euclid(3);
                    Some(match (snow, face) {
                        (true, false) => WHITE,
                        (true, true) => ICE,
                        (false, false) => ASPHALT_LT,
                        (false, true) => ASPHALT_DK,
                    })
                },
            },
            Layer {
                height: forest_height,
                shift: 2,
                ink: |_, _, _, _| Some(HILLS),
            },
        ],
        Look::Desert => &[
            Layer {
                height: mesa_height,
                shift: 1,
                // Red rock, its top edge lit by the low sun.
                ink: |_, d, h, _| Some(if d == h && h > 3 { GRASS_LT } else { BROWN }),
            },
            Layer {
                height: dune_height,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
        Look::Night => &[
            Layer {
                height: skyline_height,
                shift: 1,
                ink: |x, d, top, _| Some(skyline_ink(x, d, top)),
            },
            Layer {
                height: |x| 1 + near_hill_height(x) / 3,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
        Look::Pumpkin => &[
            Layer {
                height: hill_height,
                shift: 1,
                ink: |_, _, _, _| Some(HILLS),
            },
            Layer {
                height: dune_height,
                shift: 2,
                // The patch: rows of pumpkins (red and yellow side by
                // side, orange at a glance) along the field's top.
                ink: |x, d, h, _| {
                    Some(match (d >= h - 1, x.rem_euclid(7)) {
                        (true, 0) => RED,
                        (true, 1) => YELLOW,
                        _ => GRASS_DK,
                    })
                },
            },
        ],
        Look::GhostCove => &[
            Layer {
                height: |x| headland_height(x).max(wreck_height(x)),
                shift: 1,
                ink: |_, _, _, _| Some(HILLS),
            },
            Layer {
                height: |_| SEA,
                shift: 1,
                // A black sea with sparse moonlit glints.
                ink: |x, d, _, _| {
                    Some(if (x * 7 + d * 5).rem_euclid(23) == 0 {
                        WHITE
                    } else {
                        WATER
                    })
                },
            },
            Layer {
                height: dune_height,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
        Look::HauntedPines => &[
            Layer {
                height: peak_height,
                shift: 1,
                // Dark rock, snow pale under the moon.
                ink: |x, d, _, face| {
                    let snow = d > SNOW_LINE + (x * 5).rem_euclid(3);
                    Some(match (snow, face) {
                        (true, false) => ICE,
                        (true, true) => ASPHALT_LT,
                        (false, false) => ASPHALT_DK,
                        (false, true) => INK,
                    })
                },
            },
            Layer {
                height: dead_pine_height,
                shift: 2,
                ink: |_, _, _, _| Some(HILLS),
            },
        ],
        Look::BloodMoon => &[
            Layer {
                height: mesa_height,
                shift: 1,
                // Black rock, its top edge lit red by the moon.
                ink: |_, d, h, _| Some(if d == h && h > 3 { RED_DK } else { HILLS }),
            },
            Layer {
                height: dune_height,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
        Look::MonsterCity => &[
            Layer {
                height: skyline_height,
                shift: 1,
                ink: |x, d, top, _| Some(monster_ink(x, d, top)),
            },
            Layer {
                height: |x| 1 + near_hill_height(x) / 3,
                shift: 2,
                ink: |_, _, _, _| Some(GRASS_DK),
            },
        ],
    }
}

/// GHOST COVE: a shipwreck out at sea every 240 columns: a hull listing
/// into the water, a broken mainmast and a stump of a foremast with a
/// spar across it (0 where there is none).
pub fn wreck_height(x: i32) -> i32 {
    match x.rem_euclid(240) {
        // The mainmast, broken off; the foremast and its spar.
        6 | 7 => SEA + 19,
        8 => SEA + 15,
        15 => SEA + 14,
        c @ 12..=18 => SEA + 11 - (c - 15).abs() / 2,
        // The stern, high, then the hull falling away to the bow.
        c @ 0..=3 => SEA + 4 - c / 3,
        c @ 4..=21 => SEA + 3 - (c - 4) / 6,
        _ => 0,
    }
}

/// HAUNTED PINES: dead pines in front of the peaks, a bare trunk in most
/// nine-column cells (where and how tall a hash of the cell) with broken branch
/// stubs either side, over a low ragged undergrowth.
pub fn dead_pine_height(x: i32) -> i32 {
    let cell = x.div_euclid(9);
    let hash = (cell as u32).wrapping_mul(0x9E37_79B1) >> 8;
    let undergrowth = 2 + i32::from((x * 5).rem_euclid(3) == 0);
    if hash & 3 == 0 {
        // A gap in the trees.
        return undergrowth;
    }
    let h = 7 + (hash >> 4) as i32 % 10;
    let dx = x.rem_euclid(9) - (2 + (hash >> 8) as i32 % 5);
    match dx.abs() {
        0 => h,
        1 if ((hash >> 12) as i32 + dx) & 1 == 0 => h * 2 / 3,
        1 => h / 2,
        2 if (hash >> 14) & 1 == 1 => h / 3 + 1,
        _ => undergrowth,
    }
}

/// MONSTER CITY: a skyline pixel, as [`skyline_ink`] but the lit windows
/// orange (red and yellow side by side) and the mast lights green.
fn monster_ink(x: i32, d: i32, top: i32) -> u8 {
    match skyline_ink(x, d, top) {
        YELLOW => {
            if (x + d) & 1 == 0 {
                YELLOW
            } else {
                RED
            }
        }
        RED => SKY_LO,
        ASPHALT_DK => INK,
        k => k,
    }
}

/// Whether a look's sky has clouds (drawn with the sky rows): the three
/// daylight biomes', never a twin's.
pub fn has_clouds(b: impl Into<Look>) -> bool {
    matches!(b.into(), Look::Meadow | Look::Coast | Look::Alpine)
}

/// Background columns before the sky's stars, moon and sun repeat.
pub const SKY_PERIOD: i32 = 320;

/// NIGHT CITY's stars: `(column, height in 1/64ths of the sky, ink)`,
/// a fixed scatter in the upper sky.
fn stars() -> impl Iterator<Item = (i32, i32, u8)> {
    (0u32..44).map(|i| {
        let h = i.wrapping_mul(0x2C1B_3C6D).rotate_left(13) ^ i.wrapping_mul(0x297A_2D39);
        let x = (h % SKY_PERIOD as u32) as i32;
        let y = ((h >> 12) % 40) as i32;
        (x, y, if i % 3 == 0 { ICE } else { WHITE })
    })
}

/// Paint what hangs in the sky (stars and the moon at night, the sun at
/// dusk, a twin's moon), then the look's silhouette layers, then (in a
/// twin) mist and bats, over sky rows only: a road that rises above the
/// horizon hides them. `anim` is the presentation clock the bats flap by.
pub fn backdrop(c: &mut Canvas, v: &RoadView, bg_shift: i32, b: Look, anim: u32) {
    let (w, horizon) = (v.w, v.horizon);
    let sky = |y: i32| y >= 0 && y < horizon.min(v.h) && v.rows[y as usize].is_none();
    let sky_x = |bx: i32| (bx - bg_shift.div_euclid(3)).rem_euclid(SKY_PERIOD);
    match b {
        Look::Night | Look::MonsterCity => {
            for (sx, y64, ink) in stars() {
                let y = y64 * horizon / 64;
                let x = sky_x(sx);
                if x < w && sky(y) {
                    c.put(x, y, ink);
                }
            }
            if b == Look::Night {
                // A moon: a white disc, its shadowed side pale.
                disc(c, v, sky_x(250), horizon * 12 / 64, 5, |dx, _| {
                    if dx > 2 {
                        ICE
                    } else {
                        WHITE
                    }
                });
            } else {
                // A moon going green in the glow, its right limb shaded.
                disc(c, v, sky_x(120), horizon * 14 / 64, 8, |dx, dy| {
                    if dx > 4 {
                        SKY_LO
                    } else {
                        moon_ink(dx, dy, WHITE, ICE)
                    }
                });
            }
        }
        Look::Desert => {
            // The sun, setting behind the mesas, cut by bands of sky.
            let cy = horizon - 6;
            disc(c, v, sky_x(96), cy, 14, |_, dy| {
                if dy > 0 && dy % 3 == 0 {
                    SKY_LO
                } else {
                    YELLOW
                }
            });
        }
        Look::Pumpkin => {
            // A low harvest moon.
            disc(c, v, sky_x(70), horizon - 18, 12, |dx, dy| {
                moon_ink(dx, dy, YELLOW, GRAVEL)
            });
        }
        Look::GhostCove => {
            // A pale moon high over the sea.
            disc(c, v, sky_x(150), horizon * 18 / 64, 9, |dx, dy| {
                moon_ink(dx, dy, WHITE, ICE)
            });
        }
        Look::HauntedPines => {
            // A cold moon over the peaks.
            disc(c, v, sky_x(140), horizon * 16 / 64, 10, |dx, dy| {
                moon_ink(dx, dy, ICE, ASPHALT_LT)
            });
        }
        Look::BloodMoon => {
            // A huge red moon rising behind the mesas.
            disc(c, v, sky_x(110), horizon - 12, 24, |dx, dy| {
                moon_ink(dx, dy, RED, RED_DK)
            });
        }
        _ => {}
    }
    for layer in layers(b) {
        let shift = bg_shift * layer.shift;
        for x in 0..w {
            let bx = x + shift;
            let h = (layer.height)(bx);
            if h <= 0 {
                continue;
            }
            let face = (layer.height)(bx + 1) < h;
            for y in (horizon - h).max(0)..horizon.min(v.h) {
                if v.rows[y as usize].is_none() {
                    if let Some(k) = (layer.ink)(bx, horizon - y, h, face) {
                        c.put(x, y, k);
                    }
                }
            }
        }
    }
    match b {
        Look::MonsterCity => crooked_tower(c, v, bg_shift),
        // Fog bands drifting over the sea and the headland; mist in the
        // dead pines.
        Look::GhostCove => mist(c, v, bg_shift / 2, &[(2, 4), (7, 3), (12, 2)]),
        Look::HauntedPines => mist(c, v, bg_shift / 2, &[(1, 5), (3, 4), (6, 2)]),
        _ => {}
    }
    if b.spooky() {
        bats(c, v, bg_shift, anim);
    }
}

/// A moon's pixel at `(dx, dy)` from its centre: `body`, with a few dark
/// `shade` patches (its seas).
fn moon_ink(dx: i32, dy: i32, body: u8, shade: u8) -> u8 {
    let sea = |cx: i32, cy: i32, r: i32| (dx - cx).pow(2) + (dy - cy).pow(2) <= r * r;
    if sea(-3, -2, 2) || sea(2, 3, 2) || sea(4, -4, 1) {
        shade
    } else {
        body
    }
}

/// Fog: `(rows above the horizon, sixteenths)` bands of pale dither over
/// sky rows, drifting with `shift`.
fn mist(c: &mut Canvas, v: &RoadView, shift: i32, bands: &[(i32, u8)]) {
    for &(d, frac) in bands {
        let y = v.horizon - d;
        if y < 0 || y >= v.h || v.rows[y as usize].is_some() {
            continue;
        }
        for x in 0..v.w {
            if dither_on(x + shift, y, frac) {
                c.put(x, y, ICE);
            }
        }
    }
}

/// MONSTER CITY's crooked tower, scrolling with the skyline: it leans as
/// it rises and tapers to a bent spire; one green window glows near the
/// top, orange ones below.
fn crooked_tower(c: &mut Canvas, v: &RoadView, bg_shift: i32) {
    const PERIOD: i32 = 300;
    let base = (70 - bg_shift).rem_euclid(PERIOD);
    for d in 1..=40 {
        let y = v.horizon - d;
        if y < 0 || y >= v.h || v.rows[y as usize].is_some() {
            continue;
        }
        let lean = d * d / 160;
        let half = (5 - d / 9).max(0);
        for k in [base - PERIOD, base, base + PERIOD] {
            let cx = k + lean;
            for x in cx - half..=cx + half {
                if !(0..v.w).contains(&x) {
                    continue;
                }
                let window = (26..=28).contains(&d) && (x - cx).abs() <= 1;
                let lit = d % 5 == 0 && (x - cx).abs() == 2 && d < 24;
                let ink = if window {
                    SKY_LO
                } else if lit {
                    if d % 10 == 0 {
                        RED
                    } else {
                        YELLOW
                    }
                } else {
                    HILLS
                };
                c.put(x, y, ink);
            }
        }
    }
}

/// Bat pixel maps, wings up and wings down, 7 by 3.
const BAT: [[&str; 3]; 2] = [
    ["X.....X", "XX.X.XX", ".XXXXX."],
    ["..X.X..", ".XXXXX.", "XX...XX"],
];

/// The bats: `(column, height in 1/64ths of the sky, speed in eighths of
/// a column per step)`.
const BATS: [(i32, i32, i32); 5] = [
    (40, 30, 3),
    (95, 22, -2),
    (170, 40, 4),
    (230, 26, -3),
    (290, 46, 2),
];

/// A few bats in the sky, drifting with half the background's parallax
/// and flapping by the presentation clock.
fn bats(c: &mut Canvas, v: &RoadView, bg_shift: i32, anim: u32) {
    let t = (anim % (SKY_PERIOD as u32 * 64)) as i32;
    for (i, &(bx, y64, speed)) in BATS.iter().enumerate() {
        let x0 = (bx + t * speed / 8 - bg_shift / 2).rem_euclid(SKY_PERIOD);
        let y0 = y64 * v.horizon / 64 + tri(t + 13 * i as i32, 48) / 16;
        let frame = &BAT[((t / 4 + i as i32) & 1) as usize];
        for (dy, row) in frame.iter().enumerate() {
            let y = y0 + dy as i32;
            if y < 0 || y >= v.horizon.min(v.h) || v.rows[y as usize].is_some() {
                continue;
            }
            for (dx, ch) in row.bytes().enumerate() {
                if ch != b'X' {
                    continue;
                }
                for x in [x0 + dx as i32 - SKY_PERIOD, x0 + dx as i32] {
                    if (0..v.w).contains(&x) {
                        c.put(x, y, INK);
                    }
                }
            }
        }
    }
}

/// A disc of radius `r` about `(cx, cy)` (wrapped by [`SKY_PERIOD`], so a
/// disc at the period's edge shows on both sides), on sky rows only, each
/// pixel's ink from its offset.
fn disc(c: &mut Canvas, v: &RoadView, cx: i32, cy: i32, r: i32, ink: impl Fn(i32, i32) -> u8) {
    for dy in -r..=r {
        let y = cy + dy;
        if y < 0 || y >= v.horizon.min(v.h) || v.rows[y as usize].is_some() {
            continue;
        }
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            for x in [cx + dx - SKY_PERIOD, cx + dx, cx + dx + SKY_PERIOD] {
                if (0..v.w).contains(&x) {
                    c.put(x, y, ink(dx, dy));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_near_hills_sit_lower_than_the_far_range() {
        let far = (0..1000).map(hill_height).max().unwrap();
        let near = (0..1000).map(near_hill_height).max().unwrap();
        assert!(near < far, "near {near} vs far {far}");
        assert!((0..1000).all(|x| near_hill_height(x) >= 1));
        assert!((0..180).all(|x| hill_height(x) > 0 && hill_height(x) < 30));
    }

    #[test]
    fn every_biomes_layers_are_far_then_near_and_fit_the_sky() {
        for b in Look::ALL {
            let ls = layers(b);
            assert!(ls.len() >= 2, "{b:?}");
            assert!(ls.windows(2).all(|p| p[0].shift <= p[1].shift), "{b:?}");
            for l in ls {
                assert!(
                    (0..2000).all(|x| (0..=40).contains(&(l.height)(x))),
                    "{b:?}"
                );
            }
        }
        // The peaks tower over the meadow; the sea is flat; the city has
        // towers and gaps; mesas have flat tops.
        let max = |f: fn(i32) -> i32| (0..2000).map(f).max().unwrap();
        assert!(max(peak_height) > max(hill_height) + 5);
        assert!(max(peak_height) > SNOW_LINE + 6, "the peaks carry snow");
        assert!(max(skyline_height) >= 30 && (0..2000).any(|x| skyline_height(x) == 3));
        let tops = (0..2000).filter(|&x| mesa_height(x) == 16).count();
        assert!(tops > 100, "flat mesa tops: {tops}");
        assert!(max(headland_height) > 0 && (0..2000).any(|x| headland_height(x) == 0));
    }

    #[test]
    fn the_city_has_lit_and_dark_windows() {
        let mut lit = 0;
        let mut dark = 0;
        for x in 0..2000 {
            let (h, _, within) = building(x);
            for d in 1..=h {
                let window =
                    within % 2 == 0 && (2..=7).contains(&within) && d % 3 == 0 && d < h - 1;
                match skyline_ink(x, d, h) {
                    YELLOW => lit += 1,
                    HILLS if window => dark += 1,
                    _ => {}
                }
            }
        }
        assert!(lit > 0 && dark > 0 && lit > dark, "{lit} lit, {dark} dark");
    }

    #[test]
    fn stars_keep_to_the_upper_sky() {
        let s: Vec<_> = stars().collect();
        assert_eq!(s.len(), 44);
        assert!(s
            .iter()
            .all(|&(x, y, _)| (0..SKY_PERIOD).contains(&x) && y < 40));
    }

    #[test]
    fn the_twins_have_a_wreck_dead_pines_and_orange_windows() {
        assert!((0..240).filter(|&x| wreck_height(x) > 0).count() >= 15);
        assert!((0..2000).map(wreck_height).max().unwrap() > SEA + 15);
        // Dead pines: tall bare trunks over low undergrowth.
        let tall = (0..700).filter(|&x| dead_pine_height(x) >= 6).count();
        assert!(tall >= 100 && (0..700).all(|x| dead_pine_height(x) >= 2));
        assert_ne!(BAT[0], BAT[1]);
        for f in BAT {
            assert!(f.iter().all(|r| r.len() == 7));
        }
        // Lit windows go orange: red and yellow side by side.
        let inks: Vec<u8> = (0..400)
            .flat_map(|x| (1..=34).map(move |d| monster_ink(x, d, skyline_height(x))))
            .collect();
        assert!(inks.contains(&RED) && inks.contains(&YELLOW) && inks.contains(&SKY_LO));
        assert!(!has_clouds(Look::Pumpkin) && has_clouds(Look::Meadow));
    }
}
