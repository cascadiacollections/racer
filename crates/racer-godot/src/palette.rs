//! The sixteen colours. DESIGN §5: every pixel the game draws is one of
//! these, and the tests hold every rendered frame to it.
//!
//! A new palette rather than Salt Line's (sand, rock and sea): a road game
//! needs asphalt, grass, sky, kerb red and a car body. Colours are straight
//! RGBA bytes so the rasteriser never touches a float. Sprites are stored as
//! indices into this table ([`crate::sprites`]), so recolouring the game is
//! one edit here.

/// One colour as `[r, g, b, a]`, the layout of an RGBA8 image row. Palette
/// colours are opaque.
pub type Rgba = [u8; 4];

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

/// The colours, indexed by the constants above.
pub const COLOURS: [Rgba; 16] = [
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
