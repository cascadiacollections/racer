//! The portrait layout (DESIGN §5): a fixed 180 px wide buffer whose height
//! floats with the window's aspect, split top to bottom into the road view,
//! the HUD strip and the thumb-zone control pad.
//!
//! Salt Line fixes the short side and lets the long side float; this is the
//! same rule rotated. A window wider than it is tall (a desktop) gets the
//! shortest buffer, letterboxed left and right by the viewport's `keep`
//! stretch.

/// Buffer width, pixels. Fixed.
pub const W: i32 = 180;
/// Shortest buffer height (16:9 portrait and every landscape window).
pub const H_MIN: i32 = 320;
/// Tallest buffer height (about 9:20; taller phones letterbox top and
/// bottom).
pub const H_MAX: i32 = 400;
/// Height of the control pad at the bottom.
pub const PAD_H: i32 = 92;
/// Height of the HUD strip between the road and the pad.
pub const HUD_H: i32 = 22;

/// An axis-aligned rectangle in buffer pixels, y down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl Rect {
    /// A rectangle from its top-left corner and size.
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }

    /// Whether a point falls inside. Half-open, so two rectangles sharing an
    /// edge never both claim a touch on it. Floats, because touches arrive
    /// sub-pixel.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && y >= self.y as f32
            && x < (self.x + self.w) as f32
            && y < (self.y + self.h) as f32
    }

    /// Whether two rectangles share any pixel.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// The buffer size for a window of `win` screen pixels.
pub fn buffer_size(win: (i32, i32)) -> (i32, i32) {
    let (ww, wh) = (win.0.max(1), win.1.max(1));
    let h = (W as i64 * wh as i64 + ww as i64 / 2) / ww as i64;
    (W, (h as i32).clamp(H_MIN, H_MAX))
}

/// Where everything goes in a buffer of height `h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Buffer width.
    pub w: i32,
    /// Buffer height.
    pub h: i32,
    /// The pseudo-3D road view.
    pub road: Rect,
    /// The HUD strip.
    pub hud: Rect,
    /// The control pad.
    pub pad: Rect,
    /// Steer left.
    pub left: Rect,
    /// Steer right.
    pub right: Rect,
    /// Brake.
    pub brake: Rect,
    /// Drift (the biggest: it is held through every drift corner).
    pub drift: Rect,
}

impl Layout {
    /// The layout of a `W` by `h` buffer.
    pub fn new(h: i32) -> Layout {
        let h = h.clamp(H_MIN, H_MAX);
        let pad_y = h - PAD_H;
        let road_h = pad_y - HUD_H;
        let m = 4;
        let bh = PAD_H - 2 * m;
        Layout {
            w: W,
            h,
            road: Rect::new(0, 0, W, road_h),
            hud: Rect::new(0, road_h, W, HUD_H),
            pad: Rect::new(0, pad_y, W, PAD_H),
            left: Rect::new(m, pad_y + m, 40, bh),
            right: Rect::new(m + 44, pad_y + m, 40, bh),
            brake: Rect::new(96, pad_y + m, W - 96 - m, 28),
            drift: Rect::new(96, pad_y + m + 32, W - 96 - m, bh - 32),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_floats_with_aspect_and_is_clamped() {
        // Pixel 10: 1080 x 2424 is taller than 9:20, so it clamps.
        assert_eq!(buffer_size((1080, 2424)), (180, 400));
        assert_eq!(buffer_size((1080, 2160)), (180, 360));
        assert_eq!(buffer_size((1080, 1920)), (180, 320));
        assert_eq!(buffer_size((540, 960)), (180, 320));
        // Landscape letterboxes the shortest buffer.
        assert_eq!(buffer_size((1920, 1080)), (180, 320));
        assert_eq!(buffer_size((0, 0)), (180, 320));
    }

    #[test]
    fn regions_tile_the_buffer_and_buttons_fit_without_overlap() {
        for h in [H_MIN, 360, H_MAX] {
            let l = Layout::new(h);
            assert_eq!(l.road.h + l.hud.h + l.pad.h, h);
            assert_eq!(l.hud.y, l.road.h);
            assert_eq!(l.pad.y + l.pad.h, h);
            assert!(l.road.h >= 200, "road view {}", l.road.h);
            let b = [l.left, l.right, l.brake, l.drift];
            for (i, r) in b.iter().enumerate() {
                assert!(r.x >= 0 && r.x + r.w <= W && r.y >= l.pad.y && r.y + r.h <= h);
                assert!(r.w >= 36 && r.h >= 24, "thumb-sized: {r:?}");
                for o in &b[i + 1..] {
                    assert!(!r.overlaps(o), "{r:?} overlaps {o:?}");
                }
            }
            assert!(l.drift.w * l.drift.h > l.brake.w * l.brake.h);
        }
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(10, 10, 5, 5);
        assert!(r.contains(10.0, 10.0));
        assert!(r.contains(14.9, 14.9));
        assert!(!r.contains(15.0, 12.0));
        assert!(!r.contains(12.0, 15.0));
        assert!(!r.contains(9.9, 12.0));
    }
}
