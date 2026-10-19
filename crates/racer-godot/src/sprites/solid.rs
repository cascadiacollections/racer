//! A tiny flat-shaded solid renderer, run only at startup.
//!
//! The cars are modelled as a few boxes and tapered boxes in metres and
//! drawn here once per pre-drawn frame: a yaw (the body turned on the road)
//! and a pitch (how steeply the camera looks down at that depth), at one
//! scale. That is how a spin gets its rotation frames and a car seen far
//! off shows its tail rather than its roof, while nothing is ever rotated
//! or scaled at draw time (DESIGN §5): the raster only copies the result.
//!
//! Each face is lit by one fixed light (high, from the left, a little
//! behind the camera) and painted from a short ramp of palette indices,
//! with a checkerboard between neighbouring steps: dithering, never a
//! blend. Then the silhouette gets a one-pixel ink outline, and a ground
//! shadow (an ink checkerboard) goes under it. Orthographic, a z-buffer,
//! pixel centres sampled: deterministic, and integer-exact in its output.

use crate::palette::INK;
use crate::raster::Sprite;

/// A point on a car, metres: `x` right, `y` forward (the nose), `h` up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct V3 {
    /// Right.
    pub x: f64,
    /// Forward.
    pub y: f64,
    /// Up.
    pub h: f64,
}

/// A point, `(x, y, h)`.
pub const fn v3(x: f64, y: f64, h: f64) -> V3 {
    V3 { x, y, h }
}

/// How a face is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    /// Lit along a ramp of indices from dark to light.
    Ramp(&'static [u8]),
    /// One index whatever the light: lamps, plates, stripes.
    Flat(u8),
}

/// A flat convex polygon, its points counter-clockwise seen from outside.
#[derive(Clone, Debug)]
pub struct Face {
    /// Corners.
    pub pts: Vec<V3>,
    /// Paint.
    pub paint: Paint,
}

/// A model: faces, and the ground shadow's outline (`x, y` at `h` 0).
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// Every face. Faces turned away from the camera are skipped.
    pub faces: Vec<Face>,
    /// The shadow on the road, a convex outline; empty for none.
    pub shadow: Vec<(f64, f64)>,
}

impl Model {
    /// A box `x0..x1` by `y0..y1` by `h0..h1`, one paint on every side.
    pub fn cuboid(&mut self, x: (f64, f64), y: (f64, f64), h: (f64, f64), p: Paint) {
        self.frustum(x, y, h, (x.0, x.1), (y.0, y.1), [p; 4]);
    }

    /// A tapered box: the bottom `x` by `y` at `h.0`, the top `tx` by `ty`
    /// at `h.1`. Paints: `[top, sides, back, front]` (bottom never shows).
    pub fn frustum(
        &mut self,
        x: (f64, f64),
        y: (f64, f64),
        h: (f64, f64),
        tx: (f64, f64),
        ty: (f64, f64),
        paint: [Paint; 4],
    ) {
        let [top, side, back, front] = paint;
        // Bottom corners b, top corners t: rear-left, rear-right,
        // front-right, front-left.
        let b = [
            v3(x.0, y.0, h.0),
            v3(x.1, y.0, h.0),
            v3(x.1, y.1, h.0),
            v3(x.0, y.1, h.0),
        ];
        let t = [
            v3(tx.0, ty.0, h.1),
            v3(tx.1, ty.0, h.1),
            v3(tx.1, ty.1, h.1),
            v3(tx.0, ty.1, h.1),
        ];
        let mut face = |pts: [V3; 4], paint: Paint| {
            self.faces.push(Face {
                pts: pts.to_vec(),
                paint,
            })
        };
        face([t[0], t[1], t[2], t[3]], top);
        face([b[0], b[1], t[1], t[0]], back);
        face([b[1], b[2], t[2], t[1]], side);
        face([b[2], b[3], t[3], t[2]], front);
        face([b[3], b[0], t[0], t[3]], side);
    }

    /// A flat rectangle on the back (`y` = `y0`, facing backwards) from
    /// `x.0..x.1`, `h.0..h.1`: a lamp, a plate, a door seam.
    pub fn back_decal(&mut self, x: (f64, f64), y0: f64, h: (f64, f64), p: Paint) {
        self.faces.push(Face {
            pts: vec![
                v3(x.0, y0, h.0),
                v3(x.1, y0, h.0),
                v3(x.1, y0, h.1),
                v3(x.0, y0, h.1),
            ],
            paint: p,
        });
    }

    /// The same on the front (`y` = `y1`, facing forwards).
    pub fn front_decal(&mut self, x: (f64, f64), y1: f64, h: (f64, f64), p: Paint) {
        self.faces.push(Face {
            pts: vec![
                v3(x.1, y1, h.0),
                v3(x.0, y1, h.0),
                v3(x.0, y1, h.1),
                v3(x.1, y1, h.1),
            ],
            paint: p,
        });
    }

    /// A flat rectangle on a top surface at height `h`: a stripe.
    pub fn top_decal(&mut self, x: (f64, f64), y: (f64, f64), h: f64, p: Paint) {
        self.faces.push(Face {
            pts: vec![
                v3(x.0, y.0, h),
                v3(x.1, y.0, h),
                v3(x.1, y.1, h),
                v3(x.0, y.1, h),
            ],
            paint: p,
        });
    }
}

/// How a model is seen: turned `yaw` radians (positive turns the nose
/// right), the camera `pitch` radians down from level, `scale` pixels a
/// metre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Nose right of straight ahead, radians.
    pub yaw: f64,
    /// Camera angle below the horizontal, radians.
    pub pitch: f64,
    /// Pixels per metre.
    pub scale: f64,
}

/// Where the light comes from, in view space (`x` right, `y` away from
/// the camera, `h` up): high, from the left, a little behind the camera.
const LIGHT: V3 = v3(-0.5, -0.35, 0.8);

impl View {
    /// A point turned by the yaw: still metres, `y` now along the view.
    fn turn(&self, p: V3) -> V3 {
        let (s, c) = self.yaw.sin_cos();
        v3(p.x * c + p.y * s, -p.x * s + p.y * c, p.h)
    }

    /// Screen position (pixels from the model's origin, `y` down) and
    /// depth (metres, larger is farther) of `p`.
    pub fn project(&self, p: V3) -> (f64, f64, f64) {
        let q = self.turn(p);
        let (sp, cp) = self.pitch.sin_cos();
        (
            q.x * self.scale,
            -(q.h * cp + q.y * sp) * self.scale,
            q.y * cp - q.h * sp,
        )
    }

    /// The light on a face with normal `n` (unit, model space): 0 in
    /// shadow to 1 square to the light; and whether it faces the camera.
    fn light(&self, n: V3) -> (f64, bool) {
        let q = self.turn(n);
        let (sp, cp) = self.pitch.sin_cos();
        let towards = q.y * cp - q.h * sp < -1e-6;
        let len = (LIGHT.x * LIGHT.x + LIGHT.y * LIGHT.y + LIGHT.h * LIGHT.h).sqrt();
        let l = (q.x * LIGHT.x + q.y * LIGHT.y + q.h * LIGHT.h) / len;
        (0.2 + 0.8 * l.max(0.0), towards)
    }
}

/// A rendered frame: the sprite and where the model's origin landed in
/// it, so a caller can place the car by its centre and find its lamps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Render {
    /// The pixels.
    pub sprite: Sprite,
    /// The model origin's pixel, `(x, y)`.
    pub origin: (i32, i32),
}

impl Render {
    /// Where model point `p` lands in the sprite, rounded to a pixel.
    pub fn at(&self, v: &View, p: V3) -> (i32, i32) {
        let (x, y, _) = v.project(p);
        (
            self.origin.0 + x.floor() as i32,
            self.origin.1 + y.floor() as i32,
        )
    }
}

/// Paint index `t` (0..1) along `ramp` at pixel `(x, y)`: the nearest
/// step, or a checkerboard of two neighbours when it falls between them.
pub fn ramp_at(ramp: &[u8], t: f64, x: i32, y: i32) -> u8 {
    let n = ramp.len();
    if n == 1 {
        return ramp[0];
    }
    let f = t.clamp(0.0, 1.0) * (n - 1) as f64;
    let lo = (f.floor() as usize).min(n - 2);
    let frac = f - lo as f64;
    let upper = frac > 0.7 || (frac >= 0.3 && (x + y) & 1 == 0);
    ramp[lo + usize::from(upper)]
}

/// Fill a convex polygon of screen points into `cover`, calling `hit` for
/// each pixel whose centre is inside, with the barycentric weights of the
/// fan triangle it fell in and that triangle's corner indices.
fn fill(pts: &[(f64, f64)], w: i32, h: i32, mut hit: impl FnMut(i32, i32, [usize; 3], [f64; 3])) {
    for k in 1..pts.len().saturating_sub(1) {
        let tri = [0, k, k + 1];
        let [a, b, c] = tri.map(|i| pts[i]);
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-9 {
            continue;
        }
        let x0 = (a.0.min(b.0).min(c.0).floor() as i32).max(0);
        let x1 = (a.0.max(b.0).max(c.0).ceil() as i32).min(w);
        let y0 = (a.1.min(b.1).min(c.1).floor() as i32).max(0);
        let y1 = (a.1.max(b.1).max(c.1).ceil() as i32).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = (x as f64 + 0.5, y as f64 + 0.5);
                let e = |u: (f64, f64), v: (f64, f64)| {
                    ((v.0 - u.0) * (p.1 - u.1) - (v.1 - u.1) * (p.0 - u.0)) / area
                };
                let (wa, wb, wc) = (e(b, c), e(c, a), e(a, b));
                if wa >= -1e-9 && wb >= -1e-9 && wc >= -1e-9 {
                    hit(x, y, tri, [wa, wb, wc]);
                }
            }
        }
    }
}

/// The unit normal of a face, from its first three corners.
fn normal(f: &Face) -> V3 {
    let (a, b, c) = (f.pts[0], f.pts[1], f.pts[2]);
    let (u, v) = (
        v3(b.x - a.x, b.y - a.y, b.h - a.h),
        v3(c.x - a.x, c.y - a.y, c.h - a.h),
    );
    let n = v3(
        u.y * v.h - u.h * v.y,
        u.h * v.x - u.x * v.h,
        u.x * v.y - u.y * v.x,
    );
    let l = (n.x * n.x + n.y * n.y + n.h * n.h).sqrt().max(1e-12);
    v3(n.x / l, n.y / l, n.h / l)
}

/// Options for [`render`].
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Ink the silhouette's edge, one pixel outside it.
    pub outline: bool,
    /// Ink where one part stands clear in front of another (a wing over
    /// the deck): a depth step of at least this many metres; 0 for none.
    pub creases: f64,
    /// Lift every face's light by this much (0..1): paler, flatter.
    pub lift: f64,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            outline: true,
            creases: 0.0,
            lift: 0.0,
        }
    }
}

/// Draw `m` as seen by `v`.
pub fn render(m: &Model, v: &View, style: Style) -> Render {
    // Bounds over every corner and the shadow.
    let mut pts: Vec<(f64, f64)> = m
        .faces
        .iter()
        .flat_map(|f| f.pts.iter())
        .map(|&p| {
            let (x, y, _) = v.project(p);
            (x, y)
        })
        .collect();
    let shadow: Vec<(f64, f64)> = m
        .shadow
        .iter()
        .map(|&(x, y)| {
            let (sx, sy, _) = v.project(v3(x, y, 0.0));
            (sx, sy)
        })
        .collect();
    pts.extend(&shadow);
    let pad = if style.outline { 1 } else { 0 };
    let half = pts.iter().map(|p| p.0.abs()).fold(0.0f64, f64::max).ceil() as i32 + pad;
    let top = (-pts.iter().map(|p| p.1).fold(0.0f64, f64::min)).ceil() as i32 + pad;
    let bottom = pts.iter().map(|p| p.1).fold(0.0f64, f64::max).ceil() as i32 + pad;
    let (w, h) = (2 * half, top + bottom);
    let origin = (half, top);
    let to_px = |(x, y): (f64, f64)| (x + origin.0 as f64, y + origin.1 as f64);

    let n = (w * h) as usize;
    let mut depth = vec![f64::INFINITY; n];
    let mut ink = vec![Sprite::CLEAR; n];
    for f in &m.faces {
        let nrm = normal(f);
        let (lit, towards) = v.light(nrm);
        if !towards {
            continue;
        }
        let t = (lit + style.lift).min(1.0);
        let proj: Vec<(f64, f64, f64)> = f.pts.iter().map(|&p| v.project(p)).collect();
        let scr: Vec<(f64, f64)> = proj.iter().map(|&(x, y, _)| to_px((x, y))).collect();
        fill(&scr, w, h, |x, y, tri, wt| {
            let d = wt[0] * proj[tri[0]].2 + wt[1] * proj[tri[1]].2 + wt[2] * proj[tri[2]].2;
            let i = (y * w + x) as usize;
            // Ties go to the later face: decals drawn after their panel.
            if d <= depth[i] + 1e-6 {
                depth[i] = d;
                ink[i] = match f.paint {
                    Paint::Ramp(r) => ramp_at(r, t, x, y),
                    Paint::Flat(c) => c,
                };
            }
        });
    }
    let mut s = Sprite::blank(w, h);
    s.px.clone_from(&ink);
    if style.creases > 0.0 {
        // A pixel nearer than the one above or beside it by a step: the
        // farther one is the edge of a part behind.
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if ink[i] == Sprite::CLEAR {
                    continue;
                }
                for (dx, dy) in [(1, 0), (-1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx < 0 || yy < 0 || xx >= w || yy >= h {
                        continue;
                    }
                    let j = (yy * w + xx) as usize;
                    if ink[j] != Sprite::CLEAR && depth[j] - depth[i] > style.creases {
                        s.px[j] = INK;
                    }
                }
            }
        }
    }
    if style.outline {
        let solid = s.clone();
        for y in 0..h {
            for x in 0..w {
                if solid.at(x, y) != Sprite::CLEAR {
                    continue;
                }
                let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|&(dx, dy)| solid.at(x + dx, y + dy) != Sprite::CLEAR);
                if edge {
                    s.set(x, y, INK);
                }
            }
        }
    }
    if shadow.len() >= 3 {
        let scr: Vec<(f64, f64)> = shadow.iter().map(|&p| to_px(p)).collect();
        let mut ordered = scr.clone();
        // Fan from a convex outline in either winding.
        let area: f64 = (0..ordered.len())
            .map(|i| {
                let (a, b) = (ordered[i], ordered[(i + 1) % ordered.len()]);
                a.0 * b.1 - a.1 * b.0
            })
            .sum();
        if area < 0.0 {
            ordered.reverse();
        }
        let base = s.clone();
        fill(&ordered, w, h, |x, y, _, _| {
            if base.at(x, y) == Sprite::CLEAR && (x + y) & 1 == 0 {
                s.set(x, y, INK);
            }
        });
    }
    Render { sprite: s, origin }
}

/// `r` cropped to its painted pixels, keeping the origin's column in the
/// middle: what [`super::Steps`] stores, drawn bottom-centre on its spot.
pub fn crop_centred(r: &Render) -> Sprite {
    let s = &r.sprite;
    let rows: Vec<i32> = (0..s.h)
        .filter(|&y| (0..s.w).any(|x| s.at(x, y) != Sprite::CLEAR))
        .collect();
    let cols: Vec<i32> = (0..s.w)
        .filter(|&x| (0..s.h).any(|y| s.at(x, y) != Sprite::CLEAR))
        .collect();
    let (Some(&y0), Some(&y1), Some(&x0), Some(&x1)) =
        (rows.first(), rows.last(), cols.first(), cols.last())
    else {
        return Sprite::blank(1, 1);
    };
    let half = (r.origin.0 - x0).max(x1 + 1 - r.origin.0);
    let (w, h) = (2 * half, y1 + 1 - y0);
    let mut out = Sprite::blank(w, h);
    let dx = half - r.origin.0;
    for y in 0..h {
        for x in 0..s.w {
            out.set(x + dx, y, s.at(x, y + y0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::*;

    const RAMP: &[u8] = &[INK, RED_DK, RED];

    fn cube() -> Model {
        let mut m = Model::default();
        m.cuboid((-1.0, 1.0), (-1.0, 1.0), (0.0, 1.0), Paint::Ramp(RAMP));
        m
    }

    #[test]
    fn a_box_from_behind_shows_its_back_and_top_lit_and_outlined() {
        let v = View {
            yaw: 0.0,
            pitch: 0.5,
            scale: 8.0,
        };
        let r = render(&cube(), &v, Style::default());
        let s = &r.sprite;
        assert_eq!(s.w % 2, 0);
        assert!(s.px.iter().all(|&c| c == Sprite::CLEAR || c < 16));
        // Ink all round the edge; the middle painted.
        let mid = s.at(s.w / 2, s.h / 2);
        assert!(mid == RED || mid == RED_DK, "{mid}");
        assert!((0..s.w).any(|x| s.at(x, s.h / 2) == INK));
        // Symmetric about the origin's column when not turned.
        for y in 0..s.h {
            let painted = |x| s.at(x, y) != Sprite::CLEAR;
            for x in 0..s.w {
                assert_eq!(painted(x), painted(s.w - 1 - x), "row {y}");
            }
        }
    }

    #[test]
    fn turning_shows_a_flank_and_is_deterministic() {
        let a = render(
            &cube(),
            &View {
                yaw: 0.0,
                pitch: 0.4,
                scale: 8.0,
            },
            Style::default(),
        );
        let b = render(
            &cube(),
            &View {
                yaw: 0.6,
                pitch: 0.4,
                scale: 8.0,
            },
            Style::default(),
        );
        assert_ne!(a.sprite, b.sprite);
        assert!(b.sprite.w > a.sprite.w, "a turned box is wider");
        let again = render(
            &cube(),
            &View {
                yaw: 0.6,
                pitch: 0.4,
                scale: 8.0,
            },
            Style::default(),
        );
        assert_eq!(b, again);
    }

    #[test]
    fn ramps_dither_between_neighbours_only() {
        assert_eq!(ramp_at(RAMP, 0.0, 0, 0), INK);
        assert_eq!(ramp_at(RAMP, 1.0, 0, 0), RED);
        let mid: Vec<u8> = (0..4).map(|x| ramp_at(RAMP, 0.75, x, 0)).collect();
        assert!(mid.contains(&RED) && mid.contains(&RED_DK) && !mid.contains(&INK));
    }

    #[test]
    fn crops_keep_the_origin_centred() {
        let v = View {
            yaw: 0.3,
            pitch: 0.4,
            scale: 6.0,
        };
        let r = render(&cube(), &v, Style::default());
        let c = crop_centred(&r);
        assert!(c.h <= r.sprite.h && c.w % 2 == 0);
        assert!((0..c.w).any(|x| c.at(x, 0) != Sprite::CLEAR));
        assert!((0..c.w).any(|x| c.at(x, c.h - 1) != Sprite::CLEAR));
    }
}
