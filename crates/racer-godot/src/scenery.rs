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

use racer_sim::biome::Biome;

use crate::palette::*;
use crate::project::RoadView;
use crate::raster::Canvas;

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

/// A biome's layers, far first.
fn layers(b: Biome) -> &'static [Layer] {
    match b {
        Biome::Meadow => &[
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
        Biome::Coast => &[
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
        Biome::Alpine => &[
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
        Biome::Desert => &[
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
        Biome::Night => &[
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
    }
}

/// Whether a biome's sky has clouds (drawn with the sky rows).
pub fn has_clouds(b: Biome) -> bool {
    matches!(b, Biome::Meadow | Biome::Coast | Biome::Alpine)
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
/// dusk), then the biome's silhouette layers, over sky rows only: a road
/// that rises above the horizon hides them.
pub fn backdrop(c: &mut Canvas, v: &RoadView, bg_shift: i32, b: Biome) {
    let (w, horizon) = (v.w, v.horizon);
    let sky = |y: i32| y >= 0 && y < horizon.min(v.h) && v.rows[y as usize].is_none();
    let sky_x = |bx: i32| (bx - bg_shift.div_euclid(3)).rem_euclid(SKY_PERIOD);
    match b {
        Biome::Night => {
            for (sx, y64, ink) in stars() {
                let y = y64 * horizon / 64;
                let x = sky_x(sx);
                if x < w && sky(y) {
                    c.put(x, y, ink);
                }
            }
            // A moon: a white disc, its shadowed side pale.
            disc(c, v, sky_x(250), horizon * 12 / 64, 5, |dx, _| {
                if dx > 2 {
                    ICE
                } else {
                    WHITE
                }
            });
        }
        Biome::Desert => {
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
        for b in Biome::ALL {
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
}
