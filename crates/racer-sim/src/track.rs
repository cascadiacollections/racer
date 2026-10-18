//! A track as plain integer data. DESIGN §2.1.
//!
//! A track is a list of 4 m [`Segment`]s, driven from the first to the last.
//! Each segment stores its shape as small integers — how much the road turns
//! (`curve_milli`), how steeply it climbs (`pitch_milli`), how wide it is
//! (`half_width_cm`) — plus its [`Surface`] and a few flags. Integers make a
//! track the same on every platform by construction, give it a cheap exact
//! digest ([`track_hash`]), and keep a generated track small enough to log or
//! share.
//!
//! The renderer (M2) draws these segments as the classic pseudo-3D road:
//! each segment is a slice of road whose screen offset accumulates the
//! curvature and whose height accumulates the pitch. The sim needs no
//! geometry beyond that: a bend is a sideways push on the car (`car.rs`), a
//! hill is gravity along the road and a limit on how far ahead you can see.
//!
//! Some data is *derived* rather than authored: elevation, the sight
//! distance from every segment, and the [`flags::CREST`] marks. [`Track::new`]
//! computes it once, so the bots can look it up every tick for free, and
//! [`track_hash`] leaves it out (it follows from what is hashed).

use crate::hash::Hasher;
use crate::traffic::TrafficCar;

/// Length of one segment, metres. The road's resolution: every shape change,
/// checkpoint and object sits on a segment boundary.
pub const SEG_LEN: f64 = 4.0;

/// Default half width of the road: 6 m either side of the centre line, three
/// lanes of 4 m.
pub const HALF_WIDTH_CM: u16 = 600;

/// Grass verge past each road edge before the barrier, metres. Off the road
/// but inside the verge the car is slowed (DESIGN §3.3); at the barrier it
/// stops sliding outward.
pub const VERGE: f64 = 5.0;

/// The driver's eye height above the road, metres, for sight lines.
pub const EYE_HEIGHT: f64 = 1.1;

/// How much of the road beyond a crest must show before it counts as seen,
/// metres above the surface. A driver needs to see the road, not just air.
pub const SIGHT_TARGET: f64 = 0.4;

/// Sight distances are computed out to this far, metres. Beyond it, nobody
/// plans anyway: it is the human bot's look-ahead cap (DESIGN §2.4).
pub const SIGHT_MAX: f64 = 160.0;

/// Segment flags.
pub mod flags {
    /// A corner that must be drifted to keep pace (DESIGN §2.5). Set by the
    /// generator on every segment of the corner, verified by the two-bot test.
    pub const DRIFT_REQ: u8 = 1;
    /// A blind crest: the segment is at a summit that hides the road beyond
    /// it. Derived by [`super::Track::new`] from the elevation.
    pub const CREST: u8 = 2;
    /// The tutorial corner on the first track: drifting it pays, though it is
    /// not required. Lets the renderer (M2) sign it.
    pub const DRIFT_HINT: u8 = 4;
}

/// What the road is made of. Each surface changes grip, drift grip, top
/// speed and drag (`car::SurfaceParams`, DESIGN §3.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Surface {
    /// Ordinary road.
    #[default]
    Asphalt = 0,
    /// Loose: little grip, but a drift holds well. Rewards drifting.
    Gravel = 1,
    /// Almost no grip either way. Punishes drifting.
    Ice = 2,
    /// Standing water: brief, slippery, slows the car a little.
    Puddle = 3,
}

impl Surface {
    /// Every surface, in discriminant order.
    pub const ALL: [Surface; 4] = [
        Surface::Asphalt,
        Surface::Gravel,
        Surface::Ice,
        Surface::Puddle,
    ];

    /// One letter for the ASCII strip: `.` for asphalt, then g, i, p.
    pub fn letter(self) -> char {
        match self {
            Surface::Asphalt => '.',
            Surface::Gravel => 'g',
            Surface::Ice => 'i',
            Surface::Puddle => 'p',
        }
    }
}

/// A roadside object. Hitting one spins the car (DESIGN §2.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Object {
    /// The segment it stands on; it sits at the segment's middle.
    pub seg: u32,
    /// Offset from the centre line, centimetres, positive right.
    pub x_cm: i16,
    /// What it is, which sets its size.
    pub kind: ObjectKind,
}

/// The kinds of roadside object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ObjectKind {
    /// A marker post: thin, easy to slip between.
    Post = 0,
    /// A boulder: wide enough to block most of a lane.
    Rock = 1,
}

impl ObjectKind {
    /// Radius, metres.
    pub const fn radius(self) -> f64 {
        match self {
            ObjectKind::Post => 0.25,
            ObjectKind::Rock => 1.0,
        }
    }
}

impl Object {
    /// Distance along the track, metres: the middle of its segment.
    #[inline]
    pub fn z(&self) -> f64 {
        (self.seg as f64 + 0.5) * SEG_LEN
    }

    /// Offset from the centre line, metres.
    #[inline]
    pub fn x(&self) -> f64 {
        self.x_cm as f64 / 100.0
    }
}

/// One 4 m slice of road.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Segment {
    /// How far the road turns across this segment, in milliradians. Positive
    /// turns right. 100 is a 40 m radius; 10 is a 400 m sweep.
    pub curve_milli: i16,
    /// Grade, in thousandths: metres climbed per metre travelled. Positive
    /// climbs.
    pub pitch_milli: i16,
    /// Half the road's width, centimetres.
    pub half_width_cm: u16,
    /// What the road is made of.
    pub surface: Surface,
    /// A bit set of [`flags`].
    pub flags: u8,
}

impl Default for Segment {
    fn default() -> Self {
        Segment {
            curve_milli: 0,
            pitch_milli: 0,
            half_width_cm: HALF_WIDTH_CM,
            surface: Surface::Asphalt,
            flags: 0,
        }
    }
}

impl Segment {
    /// Curvature, 1/m: the turn per metre. A car at speed `v` feels `κ·v²`
    /// of sideways push (DESIGN §3.2).
    #[inline]
    pub fn curvature(&self) -> f64 {
        self.curve_milli as f64 / 1000.0 / SEG_LEN
    }

    /// Grade as a fraction: rise over run.
    #[inline]
    pub fn pitch(&self) -> f64 {
        self.pitch_milli as f64 / 1000.0
    }

    /// Half the road's width, metres.
    #[inline]
    pub fn half_width(&self) -> f64 {
        self.half_width_cm as f64 / 100.0
    }

    /// Whether `flag` (one of [`flags`]) is set.
    #[inline]
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// A generated or hand-written track, plus what is derived from it.
#[derive(Clone, PartialEq, Debug)]
pub struct Track {
    segments: Vec<Segment>,
    /// Segment index of each checkpoint gate, ascending. The last one is the
    /// finish line.
    checkpoints: Vec<u32>,
    /// Timer allotment for each section, ticks: `allot_ticks[k]` is the time
    /// added at the start of section `k`, which ends at `checkpoints[k]`.
    /// Set by calibration (DESIGN §2.4); empty until then.
    allot_ticks: Vec<u32>,
    /// Roadside objects, sorted by segment.
    objects: Vec<Object>,
    /// Slow traffic, each car on a closed-form schedule (`traffic.rs`).
    traffic: Vec<TrafficCar>,

    // Derived by `Track::new`; never hashed.
    /// Elevation at the start of every segment, metres, plus one past the end.
    elevation: Vec<f64>,
    /// How far ahead the road is visible from each segment, metres, capped at
    /// [`SIGHT_MAX`].
    sight: Vec<f64>,
    /// `objects[object_start[i]..object_start[i + 1]]` stand on segment `i`.
    object_start: Vec<u32>,
}

impl Track {
    /// Build a track and derive its elevation, sight lines, crest flags and
    /// object index. Objects are sorted by segment (stably, so objects on one
    /// segment keep their order).
    ///
    /// # Panics
    ///
    /// If there are no segments, no checkpoints, the checkpoints are not
    /// strictly ascending inside the track, or an object stands past the end.
    /// A track like that is a generator bug, not something to drive.
    pub fn new(
        mut segments: Vec<Segment>,
        checkpoints: Vec<u32>,
        mut objects: Vec<Object>,
        traffic: Vec<TrafficCar>,
    ) -> Self {
        assert!(!segments.is_empty(), "a track needs segments");
        assert!(!checkpoints.is_empty(), "a track needs a finish line");
        assert!(
            checkpoints.windows(2).all(|w| w[0] < w[1]),
            "checkpoints must ascend: {checkpoints:?}"
        );
        assert!(
            (*checkpoints.last().unwrap() as usize) < segments.len(),
            "the finish line is past the end of the track"
        );
        objects.sort_by_key(|o| o.seg);
        assert!(
            objects.iter().all(|o| (o.seg as usize) < segments.len()),
            "an object stands past the end of the track"
        );
        let mut object_start = Vec::with_capacity(segments.len() + 1);
        let mut k = 0;
        for i in 0..=segments.len() {
            while k < objects.len() && (objects[k].seg as usize) < i {
                k += 1;
            }
            object_start.push(k as u32);
        }
        let elevation = elevation_profile(&segments);
        let sight = sight_lines(&elevation);
        mark_crests(&mut segments, &elevation, &sight);
        Track {
            segments,
            checkpoints,
            allot_ticks: Vec::new(),
            objects,
            traffic,
            elevation,
            sight,
            object_start,
        }
    }

    /// Every segment, in driving order.
    #[inline]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Number of segments.
    #[inline]
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Always false: a track has at least one segment.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Total length, metres.
    #[inline]
    pub fn length(&self) -> f64 {
        self.segments.len() as f64 * SEG_LEN
    }

    /// The segment under distance `z` along the track. Clamped to the track,
    /// so a car that has run past the end reads the last segment.
    #[inline]
    pub fn index_at(&self, z: f64) -> usize {
        let i = crate::dmath::floor(z / SEG_LEN);
        if i <= 0.0 {
            0
        } else {
            (i as usize).min(self.segments.len() - 1)
        }
    }

    /// The segment under distance `z`; see [`Track::index_at`].
    #[inline]
    pub fn segment_at(&self, z: f64) -> &Segment {
        &self.segments[self.index_at(z)]
    }

    /// Checkpoint gates as segment indices; the last is the finish.
    #[inline]
    pub fn checkpoints(&self) -> &[u32] {
        &self.checkpoints
    }

    /// Distance of checkpoint `k` along the track, metres.
    #[inline]
    pub fn checkpoint_z(&self, k: usize) -> f64 {
        self.checkpoints[k] as f64 * SEG_LEN
    }

    /// Distance of the finish line, metres.
    #[inline]
    pub fn finish_z(&self) -> f64 {
        self.checkpoint_z(self.checkpoints.len() - 1)
    }

    /// Timer allotment per section, ticks. Empty until calibrated.
    #[inline]
    pub fn allot_ticks(&self) -> &[u32] {
        &self.allot_ticks
    }

    /// Set the timer allotments, one per checkpoint.
    ///
    /// # Panics
    ///
    /// If the count does not match the checkpoints.
    pub fn set_allotments(&mut self, allot_ticks: Vec<u32>) {
        assert_eq!(allot_ticks.len(), self.checkpoints.len());
        self.allot_ticks = allot_ticks;
    }

    /// Every roadside object, sorted by segment.
    #[inline]
    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    /// The objects standing on segments `from..to` (clamped to the track).
    pub fn objects_in(&self, from: usize, to: usize) -> &[Object] {
        let n = self.segments.len();
        let (a, b) = (from.min(n), to.min(n).max(from.min(n)));
        &self.objects[self.object_start[a] as usize..self.object_start[b] as usize]
    }

    /// The traffic.
    #[inline]
    pub fn traffic(&self) -> &[TrafficCar] {
        &self.traffic
    }

    /// Elevation at distance `z`, metres, interpolated within the segment.
    pub fn elevation_at(&self, z: f64) -> f64 {
        let i = self.index_at(z);
        let f = crate::dmath::clamp(z / SEG_LEN - i as f64, 0.0, 1.0);
        self.elevation[i] + (self.elevation[i + 1] - self.elevation[i]) * f
    }

    /// How far ahead the road is visible from segment `i`, metres.
    #[inline]
    pub fn sight(&self, i: usize) -> f64 {
        self.sight[i]
    }
}

/// Integrate the pitch into an elevation per segment boundary.
fn elevation_profile(segments: &[Segment]) -> Vec<f64> {
    let mut out = Vec::with_capacity(segments.len() + 1);
    let mut y = 0.0;
    out.push(y);
    for s in segments {
        y += s.pitch() * SEG_LEN;
        out.push(y);
    }
    out
}

/// For every segment, the distance to the first point of road hidden from a
/// driver standing there: the classic "running maximum slope" line-of-sight
/// scan along the elevation profile.
///
/// Looking forward from eye height, a road point is hidden when some nearer
/// road point subtends a steeper slope. Keeping the steepest slope seen so
/// far makes each point an O(1) test, so the whole scan is O(n·k) for a
/// look-ahead of k segments. Curves do not hide the road: in a pseudo-3D view
/// the road ahead swings sideways but stays on screen.
fn sight_lines(elevation: &[f64]) -> Vec<f64> {
    let n = elevation.len() - 1;
    let reach = (SIGHT_MAX / SEG_LEN) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let eye = elevation[i] + EYE_HEIGHT;
        let mut steepest = f64::NEG_INFINITY;
        let mut seen = SIGHT_MAX;
        for j in 1..=reach {
            let Some(&y) = elevation.get(i + j) else {
                break;
            };
            let d = j as f64 * SEG_LEN;
            if (y + SIGHT_TARGET - eye) / d < steepest {
                seen = (j - 1) as f64 * SEG_LEN;
                break;
            }
            steepest = steepest.max((y - eye) / d);
        }
        out.push(seen);
    }
    out
}

/// Sight distance below which a summit counts as blind, metres.
pub const BLIND_SIGHT: f64 = 60.0;

/// Mark [`flags::CREST`] on summits that hide the road: segments that sit at
/// a local maximum of elevation and from whose approach the sight distance is
/// short. Recomputed from scratch, so the flag cannot disagree with the
/// elevation it describes.
fn mark_crests(segments: &mut [Segment], elevation: &[f64], sight: &[f64]) {
    for (i, s) in segments.iter_mut().enumerate() {
        s.flags &= !flags::CREST;
        let summit = i > 0 && elevation[i] > elevation[i - 1] && elevation[i] >= elevation[i + 1];
        // Judge blindness from a few segments before the summit, where the
        // driver is still climbing and the road beyond is hidden.
        let before = i.saturating_sub(4);
        if summit && sight[before] < BLIND_SIGHT {
            s.flags |= flags::CREST;
        }
    }
}

/// Digest of everything authored about a track: segments, checkpoints and
/// allotments. Derived data is left out, since it follows from these.
///
/// What goes in, and in what order, is part of every pinned track digest and
/// of the epoch digest (DESIGN §8.3).
pub fn track_hash(t: &Track) -> u64 {
    let mut h = Hasher::new();
    hash_track(&mut h, t);
    h.finish()
}

/// Fold a track into a running hash; see [`track_hash`].
pub fn hash_track(h: &mut Hasher, t: &Track) {
    h.write_usize(t.segments.len());
    for s in &t.segments {
        h.write_i32(s.curve_milli as i32);
        h.write_i32(s.pitch_milli as i32);
        h.write_u32(s.half_width_cm as u32);
        h.write_u8(s.surface as u8);
        h.write_u8(s.flags);
    }
    h.write_usize(t.checkpoints.len());
    for &c in &t.checkpoints {
        h.write_u32(c);
    }
    h.write_usize(t.allot_ticks.len());
    for &a in &t.allot_ticks {
        h.write_u32(a);
    }
    h.write_usize(t.objects.len());
    for o in &t.objects {
        h.write_u32(o.seg);
        h.write_i32(o.x_cm as i32);
        h.write_u8(o.kind as u8);
    }
    h.write_usize(t.traffic.len());
    for c in &t.traffic {
        h.write_u32(c.z0_dm);
        h.write_u32(c.speed_dms as u32);
        h.write_i32(c.lane_cm as i32);
    }
}

// ------------------------------------------------------------------ builder

/// Lays a track out element by element. The generator (`gen/`) and the
/// hand-written fixtures below both build with it, so a fixture is shaped
/// exactly like a generated track.
///
/// Geometry is appended in driving order ([`Builder::straight`],
/// [`Builder::bend`]); hills and surfaces are overlays on segments already
/// laid ([`Builder::hill`], [`Builder::surface`]), because a crest can sit on
/// a bend and a gravel patch can span a straight and a corner.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    segments: Vec<Segment>,
    checkpoints: Vec<u32>,
    objects: Vec<Object>,
    traffic: Vec<TrafficCar>,
}

impl Builder {
    /// An empty track.
    pub fn new() -> Self {
        Self::default()
    }

    /// Segments laid so far: the index the next element starts at.
    #[inline]
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// True before anything is laid.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Segments laid so far, for overlays that read the geometry.
    pub fn segments_mut(&mut self) -> &mut [Segment] {
        &mut self.segments
    }

    /// `segs` segments of straight road.
    pub fn straight(&mut self, segs: usize) -> &mut Self {
        self.segments
            .extend(std::iter::repeat_n(Segment::default(), segs));
        self
    }

    /// A bend of `segs` segments peaking at `peak_milli` of turn per segment
    /// (positive turns right). The turn eases in and out over `ease` segments
    /// at each end with [`crate::dmath::smoothstep`], so the car is never
    /// asked to change its line in a single segment.
    pub fn bend(&mut self, segs: usize, peak_milli: i16, ease: usize) -> &mut Self {
        let ease = ease.min(segs / 2);
        for k in 0..segs {
            let from_edge = k.min(segs - 1 - k);
            let f = if from_edge < ease {
                crate::dmath::smoothstep((from_edge as f64 + 0.5) / ease as f64)
            } else {
                1.0
            };
            let curve = crate::dmath::round(peak_milli as f64 * f) as i16;
            self.segments.push(Segment {
                curve_milli: curve,
                ..Segment::default()
            });
        }
        self
    }

    /// A gate at the current end of the track. The last gate added is the
    /// finish line.
    pub fn checkpoint(&mut self) -> &mut Self {
        self.checkpoints.push(self.segments.len() as u32);
        self
    }

    /// Overlay a hill of `height` metres (negative for a dip) on `segs`
    /// segments starting at `start`. The elevation follows
    /// [`crate::dmath::bump`]; each segment's pitch is the slope across it,
    /// quantised to thousandths.
    pub fn hill(&mut self, start: usize, segs: usize, height: f64) -> &mut Self {
        let r = segs as f64 * SEG_LEN / 2.0;
        let mid = start as f64 * SEG_LEN + r;
        let end = (start + segs).min(self.segments.len());
        for i in start..end {
            let z0 = i as f64 * SEG_LEN;
            let y0 = height * crate::dmath::bump(z0 - mid, r);
            let y1 = height * crate::dmath::bump(z0 + SEG_LEN - mid, r);
            let p = crate::dmath::round((y1 - y0) / SEG_LEN * 1000.0);
            let s = &mut self.segments[i];
            s.pitch_milli = (s.pitch_milli as f64 + p).clamp(-300.0, 300.0) as i16;
        }
        self
    }

    /// Lay `surface` on segments `start..start+segs`.
    pub fn surface(&mut self, start: usize, segs: usize, surface: Surface) -> &mut Self {
        let end = (start + segs).min(self.segments.len());
        for s in &mut self.segments[start..end] {
            s.surface = surface;
        }
        self
    }

    /// Set `flag` on segments `start..start+segs`.
    pub fn flag(&mut self, start: usize, segs: usize, flag: u8) -> &mut Self {
        let end = (start + segs).min(self.segments.len());
        for s in &mut self.segments[start..end] {
            s.flags |= flag;
        }
        self
    }

    /// Stand an object on segment `seg`, `x` metres from the centre line.
    pub fn object(&mut self, seg: usize, x: f64, kind: ObjectKind) -> &mut Self {
        self.objects.push(Object {
            seg: seg as u32,
            x_cm: crate::dmath::round(x * 100.0) as i16,
            kind,
        });
        self
    }

    /// Add a traffic car.
    pub fn traffic(&mut self, car: TrafficCar) -> &mut Self {
        self.traffic.push(car);
        self
    }

    /// Keep only the objects for which `keep` is true.
    pub fn retain_objects(&mut self, keep: impl FnMut(&Object) -> bool) -> &mut Self {
        self.objects.retain(keep);
        self
    }

    /// Move every traffic car that starts between `from` and `to` metres
    /// `by` metres further up the road.
    pub fn shift_traffic(&mut self, from: f64, to: f64, by: f64) -> &mut Self {
        for c in &mut self.traffic {
            let z = c.z0_dm as f64 / 10.0;
            if (from..to).contains(&z) {
                *c = TrafficCar::new(z + by, c.speed(), c.x());
            }
        }
        self
    }

    /// Finish: derive the track's elevation, sight lines, crests and object
    /// index.
    pub fn build(self) -> Track {
        Track::new(self.segments, self.checkpoints, self.objects, self.traffic)
    }
}

// ----------------------------------------------------------------- fixtures

/// Hand-written tracks for tests and tuning. Each isolates one feature, so a
/// test failing on one of them says which part of the sim moved.
pub mod fixtures {
    use super::*;

    /// 400 m of straight road and a finish line: the car alone.
    pub fn drag_strip() -> Track {
        let mut b = Builder::new();
        b.straight(100).checkpoint().straight(20);
        b.build()
    }

    /// Straight, a long right sweep, straight, a tight left hairpin,
    /// straight. Two checkpoints.
    pub fn sweep_and_hairpin() -> Track {
        let mut b = Builder::new();
        b.straight(40)
            .bend(60, 18, 10)
            .straight(30)
            .checkpoint()
            .bend(40, -110, 6)
            .straight(50)
            .checkpoint()
            .straight(20);
        b.build()
    }

    /// A blind crest with a bend just past it, the hazard a driver cannot
    /// see coming (DESIGN §2.5).
    pub fn blind_crest() -> Track {
        let mut b = Builder::new();
        b.straight(60).bend(24, 70, 4).straight(40).checkpoint();
        b.straight(20);
        b.hill(30, 36, 9.0);
        b.build()
    }

    /// A straight with every hazard on it, one at a time: a row of posts
    /// pinching the right lane, a boulder in the left lane, a gravel patch,
    /// an ice patch and a slow car in the centre lane.
    pub fn hazards() -> Track {
        let mut b = Builder::new();
        b.straight(300).checkpoint().straight(20);
        for seg in (60..80).step_by(2) {
            b.object(seg, 4.0, ObjectKind::Post);
        }
        b.object(100, -4.0, ObjectKind::Rock);
        b.surface(130, 20, Surface::Gravel);
        b.surface(170, 15, Surface::Ice);
        b.traffic(TrafficCar::new(400.0, 20.0, 0.0));
        b.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_units_are_as_documented() {
        let s = Segment {
            curve_milli: 100,
            pitch_milli: -50,
            ..Segment::default()
        };
        // 100 milliradians per 4 m is a 40 m radius.
        assert!((1.0 / s.curvature() - 40.0).abs() < 1e-9);
        assert_eq!(s.pitch(), -0.05);
        assert_eq!(s.half_width(), 6.0);
    }

    #[test]
    fn index_at_clamps_to_the_track() {
        let t = fixtures::drag_strip();
        assert_eq!(t.index_at(-5.0), 0);
        assert_eq!(t.index_at(3.99), 0);
        assert_eq!(t.index_at(4.0), 1);
        assert_eq!(t.index_at(1e9), t.len() - 1);
    }

    #[test]
    fn a_bend_eases_in_and_out() {
        let mut b = Builder::new();
        b.bend(20, 100, 5);
        let c: Vec<i16> = b.segments.iter().map(|s| s.curve_milli).collect();
        assert!(c[0] > 0 && c[0] < 100, "eases in: {c:?}");
        assert!(c.windows(2).take(5).all(|w| w[0] <= w[1]));
        assert_eq!(c[10], 100);
        assert_eq!(c[0], c[19], "symmetric: {c:?}");
    }

    #[test]
    fn a_hill_returns_to_its_starting_height() {
        let mut b = Builder::new();
        b.straight(80).checkpoint().straight(1).hill(10, 40, 8.0);
        let t = b.build();
        let top = t.elevation_at(30.0 * SEG_LEN);
        assert!((top - 8.0).abs() < 0.2, "summit {top}");
        assert!(t.elevation_at(79.0 * SEG_LEN).abs() < 0.2);
    }

    #[test]
    fn flat_road_is_seen_to_the_cap() {
        let t = fixtures::drag_strip();
        assert_eq!(t.sight(0), SIGHT_MAX);
        // Near the end the scan runs out of road, which counts as seen.
        assert_eq!(t.sight(t.len() - 1), SIGHT_MAX);
    }

    #[test]
    fn a_crest_hides_the_road_beyond_it() {
        let t = fixtures::blind_crest();
        let crest: Vec<usize> = (0..t.len())
            .filter(|&i| t.segments()[i].has(flags::CREST))
            .collect();
        assert_eq!(crest.len(), 1, "one summit: {crest:?}");
        let summit = crest[0];
        assert!((46..=50).contains(&summit), "summit at {summit}");
        // Climbing toward it the sight line is cut short; past it the road
        // drops away in view again.
        assert!(t.sight(summit - 6) < BLIND_SIGHT);
        assert_eq!(t.sight(summit + 20), SIGHT_MAX);
    }

    #[test]
    fn crest_flags_are_derived_not_trusted() {
        let mut segs = fixtures::drag_strip().segments().to_vec();
        segs[10].flags |= flags::CREST;
        let t = Track::new(segs, vec![100], vec![], vec![]);
        assert!(!t.segments()[10].has(flags::CREST));
    }

    #[test]
    fn the_hash_covers_authored_data_only() {
        let a = fixtures::sweep_and_hairpin();
        let mut b = a.clone();
        assert_eq!(track_hash(&a), track_hash(&b));
        b.set_allotments(vec![1, 2]);
        assert_ne!(track_hash(&a), track_hash(&b));
        let mut segs = a.segments().to_vec();
        segs[3].surface = Surface::Gravel;
        let c = Track::new(segs, a.checkpoints().to_vec(), vec![], vec![]);
        assert_ne!(track_hash(&a), track_hash(&c));
    }

    #[test]
    fn objects_are_indexed_by_segment() {
        let t = fixtures::hazards();
        assert_eq!(t.objects_in(0, 60).len(), 0);
        assert_eq!(t.objects_in(60, 61).len(), 1);
        assert_eq!(t.objects_in(60, 80).len(), 10);
        assert_eq!(t.objects_in(100, 101)[0].kind, ObjectKind::Rock);
        assert_eq!(t.objects_in(250, 9999).len(), 0);
        assert_eq!(t.objects_in(9999, 10).len(), 0);
    }

    #[test]
    #[should_panic(expected = "checkpoints must ascend")]
    fn checkpoints_must_ascend() {
        Track::new(vec![Segment::default(); 10], vec![5, 5], vec![], vec![]);
    }
}
