//! The pseudo-3D road projection, pinned in the sim's metres (DESIGN §5).
//!
//! The classic scanline road. The camera sits [`Camera::back`] metres behind
//! the car and [`Camera::height`] metres above the road under it, looking
//! along the road; it follows the car sideways, so the car stays
//! bottom-centre and the road swings around it. Walking the segments from
//! the camera outward:
//!
//! * **Curves** bend the road's centre line sideways. Each segment turns the
//!   heading by `κ·len` and moves the centre by `heading·len + κ·len²/2`, so
//!   the offset at distance `s` is the double integral of the curvature, as
//!   a car following the road would see it. Positive curvature (a right
//!   bend) moves the far road right.
//! * **Hills** move it up and down: a point of the road projects to
//!   `horizon + (camera height − elevation)·F/dz`. Elevation is the sim's
//!   own integrated pitch ([`Track::elevation_at`]).
//! * **Crests** hide what is behind them. Segments are filled near to far,
//!   and a segment only takes rows *above* the highest row filled so far
//!   (`max_y`, "Lou's clip"). Road that projects below a nearer hilltop gets
//!   no rows: it is behind the hill. Each segment remembers the clip line it
//!   was drawn under, so a sprite standing on it is cut off at the same
//!   hilltop.
//!
//! Everything comes out in whole pixels. The sim has no geometry beyond its
//! integers: this is a presentation of them, not part of the rules.

use racer_sim::track::{Track, HALF_WIDTH_CM, SEG_LEN};

/// The camera's fixed parameters. Units are metres and buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Height above the road under the camera, metres.
    pub height: f64,
    /// Distance behind the car, metres.
    pub back: f64,
    /// Focal length: pixels per metre at one metre's depth.
    pub focal: f64,
    /// The horizon's row, as a fraction of the view's height from the top.
    pub horizon: f64,
    /// Nothing nearer than this is projected, metres.
    pub near: f64,
    /// Segments drawn ahead of the camera.
    pub draw_segs: usize,
}

impl Camera {
    /// The game's camera. The road at the car is about 150 px of the 180 px
    /// buffer wide, and the car sits `height·focal/back` rows below the
    /// horizon.
    pub const DEFAULT: Camera = Camera {
        height: 6.0,
        back: 5.0,
        focal: 60.0,
        horizon: 0.38,
        near: 0.5,
        draw_segs: 120,
    };

    /// Rows from the horizon down to the road under the car on the flat.
    pub fn car_drop(&self) -> f64 {
        self.height * self.focal / self.back
    }
}

/// Segments per stripe band: kerbs, lane dashes and the grass alternate
/// every this many segments (12 m).
pub const STRIPE_SEGS: i64 = 3;

/// What one row of the road view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// The segment under this row. Negative before the start line; at or
    /// past the track's length after the end.
    pub seg: i64,
    /// The road's centre, buffer x.
    pub cx: i32,
    /// Half the road's width (to the edge of the tarmac), pixels.
    pub half: i32,
    /// Pixels per metre on this row.
    pub scale_x256: i32,
}

/// A point on the centre line, in the camera's frame.
#[derive(Clone, Copy, Debug)]
struct Pt {
    /// Distance along the track, metres.
    z: f64,
    /// The road centre's sideways offset from the camera's line of sight.
    off: f64,
    /// Road elevation.
    elev: f64,
}

/// A thing on the road, projected: where to stand a sprite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Buffer x of the point.
    pub x: i32,
    /// Buffer y of the ground under it.
    pub y: i32,
    /// Pixels per metre at its depth.
    pub scale: f64,
    /// Rows at and below this are hidden by nearer road (a crest).
    pub clip: i32,
    /// Depth from the camera, metres.
    pub dz: f64,
}

/// One frame's projection of the road.
#[derive(Clone, Debug)]
pub struct RoadView {
    /// The camera used.
    pub cam: Camera,
    /// View width and height, pixels.
    pub w: i32,
    /// View height.
    pub h: i32,
    /// The horizon's row.
    pub horizon: i32,
    /// One entry per row, top to bottom: the road on it, or sky/background.
    pub rows: Vec<Option<Row>>,
    /// The first segment drawn (the one under the camera).
    pub base: i64,
    pts: Vec<Pt>,
    /// Per segment drawn (index from `base`), the clip line it was drawn
    /// under.
    clips: Vec<i32>,
    cam_z: f64,
    cam_x: f64,
    cam_y: f64,
}

/// Curvature of segment `i`, flat and straight off either end.
fn curvature(track: &Track, i: i64) -> f64 {
    if i < 0 || i >= track.len() as i64 {
        0.0
    } else {
        track.segments()[i as usize].curvature()
    }
}

/// Half width of segment `i`, metres; normal width off either end.
pub fn half_width(track: &Track, i: i64) -> f64 {
    if i < 0 || i >= track.len() as i64 {
        HALF_WIDTH_CM as f64 / 100.0
    } else {
        track.segments()[i as usize].half_width()
    }
}

impl RoadView {
    /// Project `track` for a car at `car_z`, `car_x` into a `w` by `h` view.
    pub fn build(track: &Track, car_z: f64, car_x: f64, w: i32, h: i32, cam: Camera) -> RoadView {
        let cam_z = car_z - cam.back;
        let cam_y = track.elevation_at(cam_z.max(0.0)) + cam.height;
        let horizon = (h as f64 * cam.horizon).round() as i32;
        let base = (cam_z / SEG_LEN).floor() as i64;
        let frac = cam_z / SEG_LEN - base as f64;
        let mut v = RoadView {
            cam,
            w,
            h,
            horizon,
            rows: vec![None; h.max(0) as usize],
            base,
            pts: Vec::with_capacity(cam.draw_segs + 1),
            clips: Vec::with_capacity(cam.draw_segs),
            cam_z,
            cam_x: car_x,
            cam_y,
        };
        let (mut off, mut heading) = (0.0, 0.0);
        v.pts.push(Pt {
            z: cam_z,
            off,
            elev: track.elevation_at(cam_z.max(0.0)),
        });
        let mut max_y = h;
        for n in 0..cam.draw_segs {
            let seg = base + n as i64;
            let len = if n == 0 {
                (1.0 - frac) * SEG_LEN
            } else {
                SEG_LEN
            };
            let k = curvature(track, seg);
            off += heading * len + 0.5 * k * len * len;
            heading += k * len;
            let z = (seg + 1) as f64 * SEG_LEN;
            v.pts.push(Pt {
                z,
                off,
                elev: track.elevation_at(z.max(0.0)),
            });
            v.clips.push(max_y);
            if max_y <= 0 {
                continue;
            }
            let (a, b) = (v.pts[n], v.pts[n + 1]);
            let (ya, xa, sa) = v.project(a.z, a.off, a.elev);
            let (yb, xb, sb) = v.project(b.z, b.off, b.elev);
            if yb >= ya {
                // Running away downhill out of sight, or edge-on.
                continue;
            }
            let top = yb.round() as i32;
            if top >= max_y {
                // Behind a nearer hilltop.
                continue;
            }
            let hw = half_width(track, seg);
            for y in top.max(0)..max_y.min(ya.round() as i32 + 1).min(h) {
                let t = ((ya - (y as f64 + 0.5)) / (ya - yb)).clamp(0.0, 1.0);
                let s = sa + (sb - sa) * t;
                let cx = xa + (xb - xa) * t;
                v.rows[y as usize] = Some(Row {
                    seg,
                    cx: cx.round() as i32,
                    half: (hw * s).round() as i32,
                    scale_x256: (s * 256.0).round() as i32,
                });
            }
            max_y = top.max(0);
        }
        v
    }

    /// Project a point `off` metres sideways of the camera's line of sight,
    /// at elevation `elev` and distance `z`: `(y, x, pixels per metre)`.
    fn project(&self, z: f64, off: f64, elev: f64) -> (f64, f64, f64) {
        let dz = (z - self.cam_z).max(self.cam.near);
        let s = self.cam.focal / dz;
        let y = self.horizon as f64 + (self.cam_y - elev) * s;
        let x = self.w as f64 / 2.0 + (off - self.cam_x) * s;
        (y, x, s)
    }

    /// Where a thing at distance `z`, `x` metres off the centre line,
    /// stands on screen, if it is within the drawn road and in front of the
    /// camera.
    pub fn place(&self, z: f64, x: f64) -> Option<Placed> {
        let dz = z - self.cam_z;
        if dz < self.cam.near {
            return None;
        }
        let n = self
            .pts
            .windows(2)
            .position(|p| z >= p[0].z && z < p[1].z)?;
        let (a, b) = (self.pts[n], self.pts[n + 1]);
        let t = ((z - a.z) / (b.z - a.z)).clamp(0.0, 1.0);
        let off = a.off + (b.off - a.off) * t;
        let elev = a.elev + (b.elev - a.elev) * t;
        let (y, cx, s) = self.project(z, off + x, elev);
        Some(Placed {
            x: cx.round() as i32,
            y: y.round() as i32,
            scale: s,
            clip: self.clips[n],
            dz,
        })
    }

    /// Whether any row shows segment `seg`. A far segment can be visible
    /// and still get no row of its own: less than a pixel tall.
    pub fn shows(&self, seg: i64) -> bool {
        self.rows.iter().flatten().any(|r| r.seg == seg)
    }

    /// Whether segment `seg` is in view: drawn, and its far end not below
    /// the nearer road's highest row by more than rounding. Road running
    /// away down the far side of a crest drops below that row, so it is
    /// hidden; a summit too far away to cover a whole row is not.
    pub fn visible(&self, seg: i64) -> bool {
        let n = seg - self.base;
        if n < 0 || n as usize >= self.clips.len() {
            return false;
        }
        let b = self.pts[n as usize + 1];
        let (yb, _, _) = self.project(b.z, b.off, b.elev);
        yb < self.clips[n as usize] as f64 + 0.5
    }

    /// The row the road under the car is on, on the flat: where the car
    /// sprite stands.
    pub fn car_row(&self) -> i32 {
        self.horizon + self.cam.car_drop().round() as i32
    }

    /// Distance from the camera to the far end of the drawn road, metres.
    pub fn reach(&self) -> f64 {
        self.pts.last().map_or(0.0, |p| p.z - self.cam_z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use racer_sim::track::fixtures;

    const W: i32 = 180;
    const H: i32 = 240;

    fn view(t: &Track, z: f64, x: f64) -> RoadView {
        RoadView::build(t, z, x, W, H, Camera::DEFAULT)
    }

    /// Rows from the bottom up, with the row index.
    fn up(v: &RoadView) -> impl Iterator<Item = (i32, Row)> + '_ {
        (0..v.h)
            .rev()
            .filter_map(move |y| v.rows[y as usize].map(|r| (y, r)))
    }

    #[test]
    fn on_the_flat_distance_rises_up_the_screen_and_stops_at_the_horizon() {
        let t = fixtures::drag_strip();
        let v = view(&t, 40.0, 0.0);
        let rows: Vec<_> = up(&v).collect();
        assert!(rows.len() > 60);
        assert_eq!(rows[0].0, H - 1, "the road reaches the bottom of the view");
        let mut last = i64::MIN;
        for (y, r) in &rows {
            assert!(*y >= v.horizon, "road above the horizon on the flat: {y}");
            assert!(r.seg >= last, "segments go backwards at row {y}");
            last = r.seg;
        }
        // Rows are contiguous from the bottom to the far end: no gaps on
        // the flat.
        let ys: Vec<i32> = rows.iter().map(|(y, _)| *y).collect();
        assert!(ys.windows(2).all(|p| p[1] == p[0] - 1));
    }

    #[test]
    fn the_road_narrows_with_distance() {
        let t = fixtures::drag_strip();
        let v = view(&t, 40.0, 0.0);
        let halves: Vec<i32> = up(&v).map(|(_, r)| r.half).collect();
        assert!(halves.windows(2).all(|p| p[1] <= p[0]));
        assert!(halves[0] > halves[halves.len() - 1]);
        // Straight and the camera centred: the road is centred.
        assert!(up(&v).all(|(_, r)| r.cx == W / 2));
    }

    #[test]
    fn the_road_under_the_car_is_tarmac_wide_and_at_the_car_row() {
        let t = fixtures::drag_strip();
        let v = view(&t, 42.0, 0.0);
        let r = v.rows[v.car_row() as usize].expect("road at the car");
        assert_eq!(r.seg, 10, "z = 42 m is segment 10");
        let half = 6.0 * Camera::DEFAULT.focal / Camera::DEFAULT.back;
        assert!((r.half - half.round() as i32).abs() <= 1);
        assert!(v.car_row() < H);
    }

    #[test]
    fn a_curve_shifts_the_far_road_its_way() {
        let t = fixtures::sweep_and_hairpin();
        // In the right sweep (segments 40..100).
        let v = view(&t, 50.0 * SEG_LEN, 0.0);
        let rows: Vec<_> = up(&v).collect();
        let near = rows[0].1.cx;
        let far = rows[rows.len() - 1].1.cx;
        assert!(far > near + 10, "right bend: far {far} vs near {near}");
        // Approaching the left hairpin (segments 130..170).
        let v = view(&t, 125.0 * SEG_LEN, 0.0);
        let rows: Vec<_> = up(&v).collect();
        let far = rows[rows.len() - 1].1.cx;
        assert!(far < rows[0].1.cx - 10, "left hairpin: far {far}");
    }

    #[test]
    fn moving_the_car_right_moves_the_road_left() {
        let t = fixtures::drag_strip();
        let a = view(&t, 40.0, 0.0);
        let b = view(&t, 40.0, 3.0);
        let row = a.car_row() as usize;
        let (ra, rb) = (a.rows[row].unwrap(), b.rows[row].unwrap());
        assert!(rb.cx < ra.cx);
        assert_eq!(rb.half, ra.half);
    }

    #[test]
    fn a_crest_hides_the_road_behind_it_until_the_car_is_over() {
        let t = fixtures::blind_crest();
        // The hill spans segments 30..66 and peaks near 48; the bend just
        // past it is the hazard.
        let peak = (30..66)
            .max_by(|a, b| {
                t.elevation_at(*a as f64 * SEG_LEN)
                    .total_cmp(&t.elevation_at(*b as f64 * SEG_LEN))
            })
            .unwrap() as i64;
        let climbing = view(&t, 34.0 * SEG_LEN, 0.0);
        assert!(climbing.visible(peak - 1), "the summit is in view");
        let hidden: Vec<i64> = (peak + 1..peak + 20)
            .filter(|s| !climbing.visible(*s))
            .collect();
        assert!(
            hidden.len() >= 5,
            "road past the summit should be hidden, hidden: {hidden:?}"
        );
        // Further road, rising again into view beyond the dip, is drawn
        // higher than the summit: never through it.
        let over = view(&t, (peak as f64 + 2.0) * SEG_LEN, 0.0);
        assert!(
            (peak + 3..peak + 12).all(|s| over.visible(s)),
            "over the top the road ahead is visible again"
        );
        // A sprite standing behind the crest is clipped by it.
        let behind = climbing.place((peak + 6) as f64 * SEG_LEN, 0.0).unwrap();
        assert!(behind.y > behind.clip, "its foot is below the clip line");
        let near = climbing.place(36.0 * SEG_LEN, 0.0).unwrap();
        assert!(
            near.y <= near.clip,
            "a sprite on the near slope stands on visible road"
        );
    }

    #[test]
    fn the_camera_sees_at_least_as_far_as_the_driver() {
        // Our camera is higher than the sim's 1.1 m eye, so the road it
        // hides is never nearer than what the driver cannot see.
        let t = fixtures::blind_crest();
        for car_seg in 20..60usize {
            let v = view(&t, car_seg as f64 * SEG_LEN, 0.0);
            let first_hidden = (car_seg as i64 + 1..car_seg as i64 + 40)
                .find(|s| !v.visible(*s) && *s < t.len() as i64);
            if let Some(s) = first_hidden {
                let gap = (s - car_seg as i64) as f64 * SEG_LEN;
                assert!(
                    gap + 2.0 * SEG_LEN >= t.sight(car_seg),
                    "at {car_seg}: hidden from {gap} m, driver sees {}",
                    t.sight(car_seg)
                );
            }
        }
    }

    #[test]
    fn each_row_is_one_segment_and_the_start_and_end_are_safe() {
        let t = fixtures::drag_strip();
        for z in [0.0, 2.0, 200.0, t.length() - 1.0, t.length() + 50.0] {
            let v = view(&t, z, 0.0);
            assert_eq!(v.rows.len(), H as usize);
            assert!(v.rows.iter().flatten().count() > 20, "at z {z}");
        }
        let v = view(&t, 0.0, 0.0);
        assert!(v.shows(-1), "behind the start line is drawn as road");
        assert!((0..60).all(|s| v.visible(s)), "the flat is all in view");
        assert!(v.reach() > 400.0);
    }
}
