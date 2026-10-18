//! An RGB canvas with the few primitives the track picture needs.

/// A colour.
pub type Rgb = [u8; 3];

/// A width x height RGB image.
pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: i32, h: i32, bg: Rgb) -> Self {
        let mut px = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..w * h {
            px.extend(bg);
        }
        Canvas { w, h, px }
    }

    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            let i = ((y * self.w + x) * 3) as usize;
            self.px[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        for y in y0.min(y1)..=y0.max(y1) {
            for x in x0.min(x1)..=x0.max(x1) {
                self.set(x, y, c);
            }
        }
    }

    pub fn disc(&mut self, cx: f64, cy: f64, r: f64, c: Rgb) {
        let ri = r.ceil() as i32;
        let (x0, y0) = (cx.round() as i32, cy.round() as i32);
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let (fx, fy) = ((x0 + dx) as f64 - cx, (y0 + dy) as f64 - cy);
                if fx * fx + fy * fy <= r * r + 0.25 {
                    self.set(x0 + dx, y0 + dy, c);
                }
            }
        }
    }

    /// A line `width` pixels wide: discs stamped every half pixel.
    pub fn line(&mut self, a: (f64, f64), b: (f64, f64), width: f64, c: Rgb) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let steps = ((dx * dx + dy * dy).sqrt() * 2.0).ceil().max(1.0) as i32;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            self.disc(a.0 + dx * t, a.1 + dy * t, width / 2.0, c);
        }
    }
}

/// Blue (slow) through green and yellow to red (fast), for `t` in 0..=1.
pub fn ramp(t: f64) -> Rgb {
    const STOPS: [Rgb; 5] = [
        [40, 80, 230],
        [40, 200, 220],
        [60, 210, 80],
        [240, 220, 50],
        [235, 60, 40],
    ];
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f64;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f64;
    let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * f).round() as u8;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}
