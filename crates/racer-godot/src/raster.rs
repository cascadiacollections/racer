//! The one software-rastered buffer (DESIGN §5) and its drawing primitives.
//!
//! Everything on screen is painted here, a pixel at a time, in palette
//! colours, then uploaded as one texture. Nothing is anti-aliased, nothing
//! is scaled or rotated at draw time: sprites are pre-drawn at a few sizes
//! ([`crate::sprites`]) and copied 1:1. The only blending is ordered
//! dithering between two palette colours ([`Canvas::dither_span`]).
//!
//! Drawing takes palette indices; the canvas turns each into RGBA through
//! its current table ([`Canvas::set_palette`]: the meadow's unless a biome's
//! is set), so a biome repaints the same indices without the drawing code
//! knowing.
//!
//! Engine-free: the bytes are handed to Godot by `game.rs`.

use crate::font;
use crate::palette::{Palette, Rgba, COLOURS};

/// 4x4 Bayer matrix: thresholds 0..16 spread as evenly as possible, so a
/// blend fraction lights an even scatter of pixels.
pub const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Whether pixel `(x, y)` takes the second colour of a dither at
/// `frac16`/16. 0 is never, 16 always.
#[inline]
pub fn dither_on(x: i32, y: i32, frac16: u8) -> bool {
    BAYER[y.rem_euclid(4) as usize][x.rem_euclid(4) as usize] < frac16
}

/// A pre-drawn image in palette indices, [`Sprite::CLEAR`] for holes.
/// Generated once at startup ([`crate::sprites`]) and only ever copied 1:1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
    /// Palette indices, row by row.
    pub px: Vec<u8>,
}

impl Sprite {
    /// The index that draws nothing.
    pub const CLEAR: u8 = 255;

    /// A `w` by `h` sprite, all clear.
    pub fn blank(w: i32, h: i32) -> Sprite {
        Sprite {
            w: w.max(1),
            h: h.max(1),
            px: vec![Sprite::CLEAR; (w.max(1) * h.max(1)) as usize],
        }
    }

    /// Set one pixel; off-sprite writes are dropped.
    pub fn set(&mut self, x: i32, y: i32, c: u8) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }

    /// The index at `(x, y)`, clear off the sprite.
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize]
        } else {
            Sprite::CLEAR
        }
    }

    /// Fill a rectangle, clipped.
    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, c);
            }
        }
    }
}

/// An RGBA8 image, row by row.
#[derive(Clone)]
pub struct Canvas {
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
    px: Vec<u8>,
    pal: Palette,
}

impl Canvas {
    /// A `w` by `h` canvas (at least 1 by 1), filled with palette ink.
    pub fn new(w: i32, h: i32) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        let mut c = Canvas {
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
            pal: COLOURS,
        };
        c.fill(crate::palette::INK);
        c
    }

    /// Change the size; the contents are undefined until redrawn.
    pub fn resize(&mut self, w: i32, h: i32) {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) != (self.w, self.h) {
            let pal = self.pal;
            *self = Canvas::new(w, h);
            self.pal = pal;
        }
    }

    /// Paint with `p` from now on: a biome's colours (DESIGN §5.1). What is
    /// already painted keeps its colours, so a frame sets its palette
    /// before it draws.
    pub fn set_palette(&mut self, p: &Palette) {
        self.pal = *p;
    }

    /// The colours it paints with.
    pub fn palette(&self) -> &Palette {
        &self.pal
    }

    /// The colour of index `i` in the current palette; an index past the
    /// last clamps to it rather than panic.
    #[inline]
    fn rgba(&self, i: u8) -> Rgba {
        self.pal[(i as usize).min(15)]
    }

    /// The bytes, `w * h * 4`, for the texture upload.
    pub fn bytes(&self) -> &[u8] {
        &self.px
    }

    /// Fill everything with palette colour `c`.
    pub fn fill(&mut self, c: u8) {
        let c = self.rgba(c);
        for p in self.px.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&c);
        }
    }

    /// Set one pixel; off-canvas writes are dropped.
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: u8) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = ((y * self.w + x) * 4) as usize;
            let c = self.rgba(c);
            self.px[i..i + 4].copy_from_slice(&c);
        }
    }

    /// The pixel at `(x, y)`; transparent black off the canvas.
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
        } else {
            [0, 0, 0, 0]
        }
    }

    /// Fill row `y` from `x0` up to but not including `x1`, clipped.
    pub fn span(&mut self, x0: i32, x1: i32, y: i32, c: u8) {
        if y < 0 || y >= self.h {
            return;
        }
        let (a, b) = (x0.max(0), x1.min(self.w));
        if a >= b {
            return;
        }
        let c = self.rgba(c);
        let row = (y * self.w) as usize * 4;
        for p in self.px[row + a as usize * 4..row + b as usize * 4]
            .as_chunks_mut::<4>()
            .0
        {
            p.copy_from_slice(&c);
        }
    }

    /// Like [`span`](Self::span), dithering between `a` and `b`: `frac16`
    /// sixteenths of the pixels take `b`, in the Bayer pattern.
    pub fn dither_span(&mut self, x0: i32, x1: i32, y: i32, a: u8, b: u8, frac16: u8) {
        if y < 0 || y >= self.h {
            return;
        }
        for x in x0.max(0)..x1.min(self.w) {
            self.put(x, y, if dither_on(x, y, frac16) { b } else { a });
        }
    }

    /// A filled rectangle, clipped.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        for yy in y..y + h {
            self.span(x, x + w, yy, c);
        }
    }

    /// A one-pixel rectangle outline.
    pub fn rect_outline(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.span(x, x + w, y, c);
        self.span(x, x + w, y + h - 1, c);
        for yy in y..y + h {
            self.put(x, yy, c);
            self.put(x + w - 1, yy, c);
        }
    }

    /// Copy `s` with its top-left at `(x, y)`, skipping transparent pixels
    /// and every row at or below `y_max` (the road nearer the camera that
    /// hides it: a crest). Mirrored left-to-right when `flip`.
    pub fn blit(&mut self, s: &Sprite, x: i32, y: i32, y_max: i32, flip: bool) {
        let y_end = (y + s.h).min(y_max).min(self.h);
        for yy in y.max(0)..y_end {
            let row = (yy - y) as usize * s.w as usize;
            for sx in 0..s.w {
                let src = if flip { s.w - 1 - sx } else { sx };
                let c = s.px[row + src as usize];
                if c != Sprite::CLEAR {
                    self.put(x + sx, yy, c);
                }
            }
        }
    }

    /// Text in the 3x5 face at `scale`, top-left at `(x, y)`. Returns the
    /// x after the last glyph's advance.
    pub fn text(&mut self, x: i32, y: i32, s: &str, c: u8, scale: i32) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(g) = font::small_glyph(ch) {
                self.glyph(cx, y, &g, c, scale);
            }
            cx += font::SMALL_ADVANCE as i32 * scale;
        }
        cx
    }

    /// Digits in the 5x7 face, top-left at `(x, y)`. Characters with no 5x7
    /// glyph fall back to the 3x5 face, bottom-aligned.
    pub fn text_big(&mut self, x: i32, y: i32, s: &str, c: u8) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(g) = font::big_glyph(ch) {
                self.glyph(cx, y, &g, c, 1);
                cx += font::BIG_ADVANCE as i32;
            } else {
                if let Some(g) = font::small_glyph(ch) {
                    self.glyph(cx, y + 2, &g, c, 1);
                }
                cx += font::SMALL_ADVANCE as i32;
            }
        }
        cx
    }

    /// Centred 3x5 text at `scale` on `cx`.
    pub fn text_centred(&mut self, cx: i32, y: i32, s: &str, c: u8, scale: i32) {
        let w = font::small_width(s) as i32 * scale;
        self.text(cx - w / 2, y, s, c, scale);
    }

    fn glyph(&mut self, x: i32, y: i32, rows: &[&str], c: u8, scale: i32) {
        for (ry, row) in rows.iter().enumerate() {
            for (rx, b) in row.bytes().enumerate() {
                if b == b'1' {
                    self.rect(
                        x + rx as i32 * scale,
                        y + ry as i32 * scale,
                        scale,
                        scale,
                        c,
                    );
                }
            }
        }
    }

    /// FNV-1a of the bytes, with the size: a frame's identity for golden
    /// tests.
    pub fn digest(&self) -> u64 {
        let mut h = racer_sim::hash::Hasher::new();
        h.write_i32(self.w);
        h.write_i32(self.h);
        for b in &self.px {
            h.write_u8(*b);
        }
        h.finish()
    }

    /// Whether every pixel is a colour of the current palette.
    pub fn palette_only(&self) -> bool {
        self.px
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| self.pal.contains(p))
    }

    /// The frame as a binary PPM, for eyeballing golden frames.
    pub fn ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
        for p in self.px.as_chunks::<4>().0 {
            out.extend_from_slice(&p[..3]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::*;

    #[test]
    fn a_palette_repaints_the_same_indices_and_survives_a_resize() {
        let mut c = Canvas::new(4, 4);
        c.put(0, 0, GRASS_LT);
        assert_eq!(c.get(0, 0), COLOURS[GRASS_LT as usize]);
        c.set_palette(&ALPINE);
        assert!(!c.palette_only(), "meadow grass is not an alpine colour");
        c.put(0, 0, GRASS_LT);
        c.resize(4, 4);
        assert_eq!(c.get(0, 0), ALPINE[GRASS_LT as usize]);
        c.fill(INK);
        c.put(1, 1, GRASS_LT);
        assert!(c.palette_only());
        c.resize(8, 8);
        assert_eq!(c.palette(), &ALPINE);
        assert_eq!(c.get(0, 0), rgba(INK), "ink is ink in every biome");
    }

    #[test]
    fn spans_clip_and_are_half_open() {
        let mut c = Canvas::new(8, 4);
        c.span(-3, 3, 1, RED);
        c.span(6, 20, 1, WHITE);
        c.span(0, 8, -1, WHITE);
        c.span(0, 8, 4, WHITE);
        assert_eq!(c.get(0, 1), rgba(RED));
        assert_eq!(c.get(2, 1), rgba(RED));
        assert_eq!(c.get(3, 1), rgba(INK));
        assert_eq!(c.get(5, 1), rgba(INK));
        assert_eq!(c.get(6, 1), rgba(WHITE));
        assert_eq!(c.get(7, 1), rgba(WHITE));
        assert_eq!(c.get(0, 0), rgba(INK));
        assert_eq!(c.get(0, 3), rgba(INK));
        assert_eq!(c.get(9, 1), [0, 0, 0, 0]);
    }

    #[test]
    fn sprites_skip_clear_pixels_flip_and_clip_below_y_max() {
        let s = Sprite {
            w: 2,
            h: 3,
            px: vec![RED, Sprite::CLEAR, WHITE, YELLOW, RED, RED],
        };
        let mut c = Canvas::new(6, 6);
        c.blit(&s, 1, 1, 3, false);
        assert_eq!(c.get(1, 1), rgba(RED));
        assert_eq!(c.get(2, 1), rgba(INK), "clear pixel left the canvas alone");
        assert_eq!(c.get(1, 2), rgba(WHITE));
        assert_eq!(c.get(2, 2), rgba(YELLOW));
        assert_eq!(c.get(1, 3), rgba(INK), "row at y_max is hidden");
        let mut c = Canvas::new(6, 6);
        c.blit(&s, -1, 0, 99, true);
        // Mirrored: column 1 of the sprite lands at x = -1 (off), column 0
        // at x = 0.
        assert_eq!(c.get(0, 0), rgba(RED));
        assert_eq!(c.get(0, 1), rgba(WHITE));
    }

    #[test]
    fn dither_extremes_are_solid() {
        let mut c = Canvas::new(8, 8);
        c.dither_span(0, 8, 2, RED, WHITE, 0);
        c.dither_span(0, 8, 3, RED, WHITE, 16);
        c.dither_span(0, 8, 4, RED, WHITE, 8);
        assert!((0..8).all(|x| c.get(x, 2) == rgba(RED)));
        assert!((0..8).all(|x| c.get(x, 3) == rgba(WHITE)));
        let whites = (0..8).filter(|&x| c.get(x, 4) == rgba(WHITE)).count();
        assert_eq!(whites, 4);
    }

    #[test]
    fn text_draws_glyph_pixels_at_scale() {
        let mut c = Canvas::new(20, 12);
        let end = c.text(0, 0, "T", WHITE, 2);
        assert_eq!(end, 8);
        // T's top bar is three pixels, at scale 2 six.
        assert!((0..6).all(|x| c.get(x, 0) == rgba(WHITE)));
        assert_eq!(c.get(0, 2), rgba(INK));
        assert_eq!(c.get(2, 9), rgba(WHITE));
        let end = c.text_big(0, 0, "1:", WHITE);
        assert_eq!(end, 12);
        assert!(c.palette_only());
    }

    #[test]
    fn digest_sees_one_pixel() {
        let mut c = Canvas::new(4, 4);
        let a = c.digest();
        c.put(3, 3, RED);
        assert_ne!(a, c.digest());
        assert_eq!(&c.ppm()[..11], b"P6\n4 4\n255\n");
    }
}
