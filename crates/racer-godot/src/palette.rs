//! The sixteen colours. DESIGN §5: every pixel the game draws is one of
//! these, and the tests hold every rendered frame to it.
//!
//! A new palette rather than Salt Line's (sand, rock and sea): a road game
//! needs asphalt, grass, sky, kerb red and a car body. Colours are straight
//! RGBA bytes so the rasteriser never touches a float. Sprites are stored as
//! indices into this table ([`crate::sprites`]), so recolouring the game is
//! one edit here.
//!
//! **Biomes** (DESIGN §5.1) each have their own sixteen: [`palette`] gives
//! a biome's table, chosen when a frame is painted
//! ([`crate::raster::Canvas::set_palette`]). The indices keep their roles;
//! only the scenery ones ([`SCENERY`]: grass, sky, hills, brown) change
//! colour. Everything the HUD, the pad, the text and the road's surfaces
//! are drawn in ([`FIXED`]) is the same colour in every biome, so what
//! reads in the meadow reads everywhere.
//!
//! **Seasons** (DESIGN §5.2): each biome's Halloween twin ([`Look`]) is
//! one more such table, changing the same six indices and no others.

use crate::season::Look;

/// One colour as `[r, g, b, a]`, the layout of an RGBA8 image row. Palette
/// colours are opaque.
pub type Rgba = [u8; 4];

/// Sixteen colours, by index.
pub type Palette = [Rgba; 16];

/// Ink: outlines, the HUD background, the letterbox.
pub const INK: u8 = 0;
/// Asphalt, dark band.
pub const ASPHALT_DK: u8 = 1;
/// Asphalt, light band; also the car's glass.
pub const ASPHALT_LT: u8 = 2;
/// White: lane dashes, kerbs, text.
pub const WHITE: u8 = 3;
/// Red: kerbs, the player's car, alerts.
pub const RED: u8 = 4;
/// Dark red: the car's shadow side, a held button.
pub const RED_DK: u8 = 5;
/// Grass, dark band; trees.
pub const GRASS_DK: u8 = 6;
/// Grass, light band.
pub const GRASS_LT: u8 = 7;
/// Sky at the top of the screen.
pub const SKY_HI: u8 = 8;
/// Sky at the horizon.
pub const SKY_LO: u8 = 9;
/// Far hills.
pub const HILLS: u8 = 10;
/// Gravel; also sign posts.
pub const GRAVEL: u8 = 11;
/// Ice.
pub const ICE: u8 = 12;
/// Water: puddles, traffic cars.
pub const WATER: u8 = 13;
/// Yellow: drift signs, the drift charge, highlights.
pub const YELLOW: u8 = 14;
/// Brown: tree trunks, rocks.
pub const BROWN: u8 = 15;

/// The meadow's colours (every frame's before biomes), indexed by the
/// constants above.
pub const COLOURS: Palette = [
    [16, 18, 28, 255],
    [74, 76, 88, 255],
    [98, 100, 112, 255],
    [238, 236, 226, 255],
    [204, 50, 54, 255],
    [122, 30, 42, 255],
    [50, 108, 58, 255],
    [86, 148, 72, 255],
    [68, 116, 196, 255],
    [150, 190, 232, 255],
    [92, 108, 152, 255],
    [178, 142, 98, 255],
    [198, 230, 242, 255],
    [42, 82, 152, 255],
    [246, 198, 50, 255],
    [96, 62, 40, 255],
];

/// The indices a biome recolours: its ground, sky, hills and brown.
pub const SCENERY: [u8; 6] = [GRASS_DK, GRASS_LT, SKY_HI, SKY_LO, HILLS, BROWN];

/// The indices every biome keeps: ink, tarmac, kerbs and text, the car,
/// the surfaces and the highlights.
pub const FIXED: [u8; 10] = [
    INK, ASPHALT_DK, ASPHALT_LT, WHITE, RED, RED_DK, GRAVEL, ICE, WATER, YELLOW,
];

/// A biome's colours: [`COLOURS`] with its [`SCENERY`] indices replaced,
/// `[grass dk, grass lt, sky hi, sky lo, hills, brown]`.
const fn biome_palette(s: [Rgba; 6]) -> Palette {
    let mut p = COLOURS;
    p[GRASS_DK as usize] = s[0];
    p[GRASS_LT as usize] = s[1];
    p[SKY_HI as usize] = s[2];
    p[SKY_LO as usize] = s[3];
    p[HILLS as usize] = s[4];
    p[BROWN as usize] = s[5];
    p
}

/// COAST at midday: sand verges, a bright sky, palm green, palm trunks.
pub const COAST: Palette = biome_palette([
    [210, 188, 128, 255],
    [238, 222, 170, 255],
    [52, 140, 214, 255],
    [150, 206, 238, 255],
    [64, 148, 80, 255],
    [128, 88, 54, 255],
]);

/// ALPINE: snow verges (shade and sun), a deep blue sky, pine green and
/// dark bark. The peaks are tarmac grey under white snow.
pub const ALPINE: Palette = biome_palette([
    [176, 192, 212, 255],
    [242, 246, 250, 255],
    [40, 84, 168, 255],
    [136, 176, 224, 255],
    [72, 146, 92, 255],
    [112, 80, 62, 255],
]);

/// DESERT DUSK: sand in the last light, a purple sky over an orange
/// horizon, cactus sage and the mesas' red rock.
pub const DESERT: Palette = biome_palette([
    [166, 86, 70, 255],
    [214, 120, 80, 255],
    [84, 52, 120, 255],
    [240, 150, 90, 255],
    [110, 150, 80, 255],
    [140, 72, 56, 255],
]);

/// NIGHT CITY: dark verges, a night sky with a purple glow at the horizon,
/// the skyline's blue-black and a brown darkened by the night.
pub const NIGHT: Palette = biome_palette([
    [34, 56, 48, 255],
    [44, 92, 56, 255],
    [22, 24, 64, 255],
    [60, 44, 96, 255],
    [28, 30, 48, 255],
    [88, 58, 44, 255],
]);

/// PUMPKIN PATCH (MEADOW's Halloween twin): stubble fields in the dusk,
/// a purple sky over an orange glow, far hills in silhouette, pumpkin
/// brown.
pub const PUMPKIN: Palette = biome_palette([
    [54, 44, 30, 255],
    [100, 84, 40, 255],
    [62, 36, 88, 255],
    [222, 120, 52, 255],
    [34, 24, 48, 255],
    [124, 66, 32, 255],
]);

/// GHOST COVE (COAST's): wet dark sand and pale foggy sand, a night sky
/// over a fog-grey horizon, a black headland, driftwood.
pub const GHOST_COVE: Palette = biome_palette([
    [44, 52, 40, 255],
    [136, 136, 104, 255],
    [36, 48, 86, 255],
    [78, 96, 118, 255],
    [28, 36, 58, 255],
    [110, 72, 48, 255],
]);

/// HAUNTED PINES (ALPINE's): snow in moonlight and moon-shadow, a cold
/// night sky, black pines, dead bark.
pub const HAUNTED_PINES: Palette = biome_palette([
    [120, 136, 168, 255],
    [168, 182, 206, 255],
    [30, 38, 76, 255],
    [56, 66, 108, 255],
    [22, 30, 36, 255],
    [70, 52, 44, 255],
]);

/// BLOOD MOON MESA (DESERT DUSK's): red earth, a purple sky going
/// purple-red at the horizon, near-black mesas, dried-blood rock.
pub const BLOOD_MOON: Palette = biome_palette([
    [86, 34, 40, 255],
    [138, 62, 58, 255],
    [72, 22, 64, 255],
    [134, 36, 76, 255],
    [38, 14, 32, 255],
    [100, 44, 36, 255],
]);

/// MONSTER CITY (NIGHT CITY's): purple verges, a green-black sky with a
/// sickly green glow at the horizon, a black skyline.
pub const MONSTER_CITY: Palette = biome_palette([
    [36, 20, 44, 255],
    [58, 32, 70, 255],
    [26, 50, 46, 255],
    [52, 126, 66, 255],
    [20, 16, 30, 255],
    [96, 56, 40, 255],
]);

/// The colours of a look: a biome (`palette(Biome::Coast)`) or a seasonal
/// twin (`palette(Look::GhostCove)`).
pub fn palette(l: impl Into<Look>) -> &'static Palette {
    match l.into() {
        Look::Meadow => &COLOURS,
        Look::Coast => &COAST,
        Look::Alpine => &ALPINE,
        Look::Desert => &DESERT,
        Look::Night => &NIGHT,
        Look::Pumpkin => &PUMPKIN,
        Look::GhostCove => &GHOST_COVE,
        Look::HauntedPines => &HAUNTED_PINES,
        Look::BloodMoon => &BLOOD_MOON,
        Look::MonsterCity => &MONSTER_CITY,
    }
}

/// WCAG contrast ratio of two colours, 1 to 21: how the tests hold text
/// and HUD colours to legibility in every biome.
pub fn contrast(a: Rgba, b: Rgba) -> f64 {
    fn lum(c: Rgba) -> f64 {
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Euclidean distance of two colours in RGB, 0 to 441: whether two
/// neighbours on the road read apart.
pub fn distance(a: Rgba, b: Rgba) -> f64 {
    (0..3)
        .map(|i| (a[i] as f64 - b[i] as f64).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// The colour at palette index `i`. Out-of-range indices clamp to the last
/// colour rather than panic, so a bad index shows on screen instead of
/// crashing a frame.
#[inline]
pub fn rgba(i: u8) -> Rgba {
    COLOURS[(i as usize).min(15)]
}

/// The palette index of `c`, if it is a palette colour.
pub fn index_of(c: Rgba) -> Option<u8> {
    COLOURS.iter().position(|p| *p == c).map(|i| i as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use racer_sim::biome::Biome;

    #[test]
    fn every_biome_has_sixteen_distinct_opaque_colours_and_keeps_the_fixed_ones() {
        for b in Look::ALL {
            let p = palette(b);
            for (i, a) in p.iter().enumerate() {
                assert_eq!(a[3], 255, "{b:?}");
                for c in &p[i + 1..] {
                    assert_ne!(a, c, "{b:?} index {i}");
                }
            }
            for i in FIXED {
                assert_eq!(p[i as usize], COLOURS[i as usize], "{b:?} index {i}");
            }
            if b != Look::Meadow {
                for i in SCENERY {
                    assert_ne!(p[i as usize], COLOURS[i as usize], "{b:?} index {i}");
                }
            }
        }
        assert_eq!(palette(Biome::Meadow), &COLOURS);
    }

    #[test]
    fn every_twin_changes_only_the_scenery_indices_of_its_biome() {
        for b in Biome::ALL {
            let twin = Look::of(b, crate::season::Season::Halloween);
            let (n, t) = (palette(b), palette(twin));
            for i in 0..16u8 {
                if SCENERY.contains(&i) {
                    assert_ne!(n[i as usize], t[i as usize], "{twin:?} index {i}");
                } else {
                    assert_eq!(n[i as usize], t[i as usize], "{twin:?} index {i}");
                }
            }
        }
        // Each twin's tables differ from every other look's.
        for (i, a) in Look::ALL.iter().enumerate() {
            for b in &Look::ALL[i + 1..] {
                assert_ne!(palette(*a), palette(*b), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn text_and_hud_colours_read_on_ink_in_every_biome() {
        for b in Look::ALL {
            let p = palette(b);
            let c = |i: u8| p[i as usize];
            // Body text and the timer: AA for small text.
            for ink in [WHITE, YELLOW, ICE] {
                assert!(contrast(c(ink), c(INK)) >= 4.5, "{b:?} {ink}");
            }
            // Large or secondary text (SPIN, TIME UP, hints) and the
            // plates' outlines: AA for large text.
            for ink in [RED, ASPHALT_LT] {
                assert!(contrast(c(ink), c(INK)) >= 3.0, "{b:?} {ink}");
            }
            // Pad labels on their plates, held and not.
            assert!(contrast(c(WHITE), c(ASPHALT_DK)) >= 4.5);
            assert!(contrast(c(YELLOW), c(RED_DK)) >= 4.5);
            // Bend boards: ink on yellow.
            assert!(contrast(c(INK), c(YELLOW)) >= 4.5);
        }
    }

    #[test]
    fn the_road_reads_against_every_biomes_verges() {
        for b in Look::ALL {
            let p = palette(b);
            let c = |i: u8| p[i as usize];
            for verge in [GRASS_DK, GRASS_LT] {
                // Every surface the road can be, and the kerbs (red on the
                // light band, white on the dark), stand out from the verge.
                for road in [ASPHALT_DK, ASPHALT_LT, GRAVEL, ICE, WATER, RED] {
                    assert!(
                        distance(c(road), c(verge)) >= 40.0,
                        "{b:?} road {road} on verge {verge}"
                    );
                }
            }
            assert!(distance(c(WHITE), c(GRASS_DK)) >= 40.0, "{b:?}");
            // Rocks (brown) on any tarmac.
            for road in [ASPHALT_DK, ASPHALT_LT] {
                assert!(distance(c(BROWN), c(road)) >= 40.0, "{b:?}");
            }
        }
    }

    #[test]
    fn colours_are_distinct_and_opaque() {
        for (i, a) in COLOURS.iter().enumerate() {
            assert_eq!(a[3], 255);
            for b in &COLOURS[i + 1..] {
                assert_ne!(a, b);
            }
            assert_eq!(index_of(*a), Some(i as u8));
        }
        assert_eq!(rgba(200), COLOURS[15]);
    }
}
