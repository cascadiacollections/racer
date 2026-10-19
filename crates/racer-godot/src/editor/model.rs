//! The draft and every edit: engine-free, exact, undoable (DESIGN §9.11).
//!
//! A [`Draft`] is a layout and the *runs* the editor shows it as: the road
//! cut into stretches, each edited as one piece. A generated road comes
//! with no runs, so [`auto_runs`] cuts it where it changes from straight to
//! a left or a right bend; SPLIT, JOIN, ADD and DELETE change the cuts
//! after that. Runs are the editor's, not the layout's: they are not in the
//! code, and a pasted code is cut afresh.
//!
//! Every edit ([`Edit`]) is a whole number of steps on a run: a bend's
//! peak in [`BEND_STEP`] milliradians, a hill's in [`HILL_STEP`]
//! thousandths, the width in [`WIDTH_STEP`] cm; a surface, a flag, a post
//! row, a car. A bend and a hill are written in one canonical shape (a
//! smoothstep-eased arc; one sine period of grade, so the road climbs and
//! comes back down), so stepping up and back down gives back exactly what
//! was there, and a generated bend is snapped to that shape the first time
//! it is touched. Any edit to the road drops the proof (it no longer
//! stands); renaming does not (the name is not the road).
//!
//! [`History`] keeps whole-draft snapshots ([`UNDO_DEPTH`] of them); a run
//! of presses of the same edit on the same run is one step.

use racer_sim::biome::Biome;
use racer_sim::dmath;
use racer_sim::layout::{
    Layout, AUTHORED_FLAGS, CURVE_MAX, HALF_WIDTH_RANGE, MAX_GATES, MAX_OBJECTS, MAX_SEGMENTS,
    MAX_TRAFFIC, PITCH_MAX, TRAFFIC_MIN_START,
};
use racer_sim::track::{flags, Object, ObjectKind, Segment, Surface, SEG_LEN};
use racer_sim::traffic::TrafficCar;

/// A bend's peak moves in steps of this, milliradians per segment.
pub const BEND_STEP: i32 = 5;
/// A hill's peak grade moves in steps of this, thousandths.
pub const HILL_STEP: i32 = 10;
/// The half width moves in steps of this, centimetres.
pub const WIDTH_STEP: i32 = 50;
/// A run is lengthened or shortened by this many segments (20 m).
pub const LEN_STEP: i32 = 5;
/// ADD inserts a straight this long, segments (100 m).
pub const ADD_SEGS: usize = 25;
/// The shortest run.
pub const MIN_RUN: usize = 2;
/// A bend eases in and out over at most this many segments.
pub const EASE_MAX: usize = 8;
/// Undo steps kept.
pub const UNDO_DEPTH: usize = 64;
/// Posts stand every this many segments along a row.
pub const POST_EVERY: u32 = 3;
/// Posts stand this far outside the road's edge, metres.
pub const POST_OUT: f64 = 1.0;
/// A placed traffic car's speed, decimetres a second (18 m/s).
pub const CAR_SPEED_DMS: u16 = 180;
/// Lanes a car or a rock is placed in, centimetres from the centre line.
pub const LANES_CM: [i16; 3] = [-400, 0, 400];

/// Which side of the road.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// The drift marking of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriftMark {
    /// No flag.
    None,
    /// `DRIFT_HINT`: drifting pays, nothing requires it.
    Hint,
    /// `DRIFT_REQ`: a drift-required corner (proven by the two-bot test).
    Required,
}

impl DriftMark {
    /// The flags it stands for.
    pub fn flags(self) -> u8 {
        match self {
            DriftMark::None => 0,
            DriftMark::Hint => flags::DRIFT_HINT,
            DriftMark::Required => flags::DRIFT_REQ,
        }
    }
}

/// One edit to the draft. `run` is a run index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Move the run's bend peak by `steps` of [`BEND_STEP`] (+ is right).
    Bend { run: usize, steps: i32 },
    /// Move the run's hill peak by `steps` of [`HILL_STEP`] (+ climbs first).
    Hill { run: usize, steps: i32 },
    /// Widen (+) or narrow the run by `steps` of [`WIDTH_STEP`].
    Width { run: usize, steps: i32 },
    /// Pave the run with `surface`.
    Surface { run: usize, surface: Surface },
    /// Mark the run.
    Drift { run: usize, mark: DriftMark },
    /// A row of posts along the run's `side`.
    Posts { run: usize, side: Side },
    /// A rock in lane `lane` (0..3, left to right) at the run's middle.
    Rock { run: usize, lane: usize },
    /// Clear the run's objects.
    ClearObjects { run: usize },
    /// A traffic car in lane `lane` starting at the run's start.
    Car { run: usize, lane: usize },
    /// Remove the cars that start on the run.
    ClearCars { run: usize },
    /// A gate at the run's end, or remove the run's gates if it has any.
    Gate { run: usize },
    /// Move the run's first gate by `segs` segments.
    MoveGate { run: usize, segs: i32 },
    /// Cut the run in two at its middle.
    Split { run: usize },
    /// Join the run with the next.
    Join { run: usize },
    /// Insert a straight of [`ADD_SEGS`] after the run.
    Add { run: usize },
    /// Delete the run's road and everything on it.
    Delete { run: usize },
    /// Lengthen (+) or shorten the run by `steps` of [`LEN_STEP`].
    Length { run: usize, steps: i32 },
    /// Rename the layout.
    Name(String),
    /// Paint the layout in another biome (DESIGN §5.1): scenery only, so
    /// the proof stays.
    Biome(Biome),
}

impl Edit {
    /// The run it acts on, if any.
    pub fn run(&self) -> Option<usize> {
        match *self {
            Edit::Bend { run, .. }
            | Edit::Hill { run, .. }
            | Edit::Width { run, .. }
            | Edit::Surface { run, .. }
            | Edit::Drift { run, .. }
            | Edit::Posts { run, .. }
            | Edit::Rock { run, .. }
            | Edit::ClearObjects { run }
            | Edit::Car { run, .. }
            | Edit::ClearCars { run }
            | Edit::Gate { run }
            | Edit::MoveGate { run, .. }
            | Edit::Split { run }
            | Edit::Join { run }
            | Edit::Add { run }
            | Edit::Delete { run }
            | Edit::Length { run, .. } => Some(run),
            Edit::Name(_) | Edit::Biome(_) => None,
        }
    }

    /// Repeated presses of the same stepped edit on the same run coalesce
    /// into one undo step: this is what must match.
    fn coalesce_key(&self) -> Option<(u8, usize)> {
        match *self {
            Edit::Bend { run, .. } => Some((0, run)),
            Edit::Hill { run, .. } => Some((1, run)),
            Edit::Width { run, .. } => Some((2, run)),
            Edit::MoveGate { run, .. } => Some((3, run)),
            Edit::Length { run, .. } => Some((4, run)),
            Edit::Name(_) => Some((5, 0)),
            Edit::Biome(_) => Some((6, 0)),
            _ => None,
        }
    }
}

/// A layout being edited, and its runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The layout. Its proof is kept until the road changes.
    pub layout: Layout,
    /// Run starts, ascending, the first 0: run `k` is
    /// `runs[k]..runs[k + 1]` (the last runs to the end of the road).
    pub runs: Vec<u32>,
}

/// Cut a road into runs: where it changes between straight (a turn of at
/// most 2 milliradians a segment), left and right. A change within 4
/// segments of the last cut makes no new run (a bend's eased ends stay on
/// it), and the last run is at least [`MIN_RUN`] long.
pub fn auto_runs(segments: &[Segment]) -> Vec<u32> {
    let class = |s: &Segment| match s.curve_milli {
        c if c > 2 => 1i8,
        c if c < -2 => -1,
        _ => 0,
    };
    let mut runs = vec![0u32];
    for i in 1..segments.len() {
        if class(&segments[i]) != class(&segments[i - 1]) {
            let last = *runs.last().unwrap() as usize;
            if i - last >= 4 {
                runs.push(i as u32);
            }
        }
    }
    // The last run must be long enough too.
    if runs.len() > 1 && segments.len() - (*runs.last().unwrap() as usize) < MIN_RUN {
        runs.pop();
    }
    runs
}

/// Gates thinned to the eight a layout holds, keeping the finish.
fn clamp_gates(gates: &mut Vec<u32>) {
    gates.sort_unstable();
    gates.dedup();
    while gates.len() > MAX_GATES {
        gates.remove(gates.len() - 2);
    }
}

/// The canonical bend profile at segment `i` of `n`: eased in and out.
fn bend_shape(i: usize, n: usize) -> f64 {
    let ease = EASE_MAX.min(n / 3).max(1);
    let a = (i as f64 + 0.5) / ease as f64;
    let b = (n as f64 - i as f64 - 0.5) / ease as f64;
    dmath::smoothstep(a.min(b).clamp(0.0, 1.0))
}

/// The canonical hill profile at segment `i` of `n`: one period of a sine,
/// so the road rises and returns to where it began.
fn hill_shape(i: usize, n: usize) -> f64 {
    dmath::sin(dmath::TWO_PI * (i as f64 + 0.5) / n as f64)
}

/// The amplitude of `values` along `shape`, snapped to whole `step`s: the
/// least-squares fit, so a canonical profile reads back exactly.
fn amplitude(values: impl Iterator<Item = i32>, shape: impl Fn(usize) -> f64, step: i32) -> i32 {
    let (mut num, mut den) = (0.0, 0.0);
    for (i, v) in values.enumerate() {
        let s = shape(i);
        num += v as f64 * s;
        den += s * s;
    }
    if den == 0.0 {
        return 0;
    }
    let a = num / den / step as f64;
    dmath::round(a) as i32 * step
}

impl Draft {
    /// A draft of `layout`, cut into runs by [`auto_runs`].
    pub fn new(layout: Layout) -> Draft {
        let runs = auto_runs(&layout.segments);
        Draft { layout, runs }
    }

    /// A blank layout: 200 segments (800 m) of straight, three gates.
    pub fn blank() -> Draft {
        let layout = Layout {
            segments: vec![Segment::default(); 200],
            checkpoints: vec![60, 120, 180],
            ..Layout::default()
        };
        Draft {
            layout,
            runs: vec![0, 60, 120, 180],
        }
    }

    /// Runs.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Run `k`'s segments, `start..end`.
    pub fn run_range(&self, k: usize) -> (usize, usize) {
        let start = self.runs[k] as usize;
        let end = self
            .runs
            .get(k + 1)
            .map_or(self.layout.segments.len(), |&e| e as usize);
        (start, end)
    }

    /// The run holding segment `seg`.
    pub fn run_of(&self, seg: usize) -> usize {
        self.runs
            .partition_point(|&s| s as usize <= seg)
            .saturating_sub(1)
    }

    /// Run `k`'s bend peak, milliradians a segment (as the editor shows and
    /// steps it).
    pub fn bend(&self, k: usize) -> i32 {
        let (a, b) = self.run_range(k);
        let segs = &self.layout.segments[a..b];
        amplitude(
            segs.iter().map(|s| s.curve_milli as i32),
            |i| bend_shape(i, b - a),
            1,
        )
    }

    /// Run `k`'s hill peak grade, thousandths.
    pub fn hill(&self, k: usize) -> i32 {
        let (a, b) = self.run_range(k);
        let segs = &self.layout.segments[a..b];
        amplitude(
            segs.iter().map(|s| s.pitch_milli as i32),
            |i| hill_shape(i, b - a),
            HILL_STEP,
        )
    }

    /// Run `k`'s half width at its start, centimetres.
    pub fn width(&self, k: usize) -> u16 {
        self.layout.segments[self.runs[k] as usize].half_width_cm
    }

    /// Run `k`'s surface at its start.
    pub fn surface(&self, k: usize) -> Surface {
        self.layout.segments[self.runs[k] as usize].surface
    }

    /// Run `k`'s drift marking: the strongest flag on it.
    pub fn drift(&self, k: usize) -> DriftMark {
        let (a, b) = self.run_range(k);
        let segs = &self.layout.segments[a..b];
        if segs.iter().any(|s| s.has(flags::DRIFT_REQ)) {
            DriftMark::Required
        } else if segs.iter().any(|s| s.has(flags::DRIFT_HINT)) {
            DriftMark::Hint
        } else {
            DriftMark::None
        }
    }

    /// The gates (indices into the checkpoints) on run `k`: past its start,
    /// up to and including its end (a gate on a boundary belongs to the run
    /// it ends).
    pub fn gates_on(&self, k: usize) -> Vec<usize> {
        let (a, b) = self.run_range(k);
        self.layout
            .checkpoints
            .iter()
            .enumerate()
            .filter(|(_, &c)| (c as usize) > a && (c as usize) <= b)
            .map(|(i, _)| i)
            .collect()
    }

    /// Objects on run `k`.
    pub fn objects_on(&self, k: usize) -> usize {
        let (a, b) = self.run_range(k);
        self.layout
            .objects
            .iter()
            .filter(|o| (a..b).contains(&(o.seg as usize)))
            .count()
    }

    /// Cars starting on run `k`.
    pub fn cars_on(&self, k: usize) -> usize {
        let (a, b) = self.run_range(k);
        let (za, zb) = (a as u32 * 40, b as u32 * 40);
        self.layout
            .traffic
            .iter()
            .filter(|c| (za..zb).contains(&c.z0_dm))
            .count()
    }

    /// Apply `edit`. Returns whether anything changed; a change to the road
    /// drops the proof.
    pub fn apply(&mut self, edit: &Edit) -> bool {
        let before = self.clone();
        if edit.run().is_some_and(|k| k >= self.runs.len()) {
            return false;
        }
        match edit {
            Edit::Name(n) => {
                if !racer_sim::layout::valid_name(n) || *n == self.layout.name {
                    return false;
                }
                self.layout.name = n.clone();
                return true;
            }
            Edit::Biome(b) => {
                if *b == self.layout.biome {
                    return false;
                }
                self.layout.biome = *b;
                return true;
            }
            &Edit::Bend { run, steps } => {
                let peak = (self.bend(run) + steps * BEND_STEP)
                    .clamp(-(CURVE_MAX as i32), CURVE_MAX as i32);
                let (a, b) = self.run_range(run);
                for (i, s) in self.layout.segments[a..b].iter_mut().enumerate() {
                    s.curve_milli = dmath::round(peak as f64 * bend_shape(i, b - a)) as i16;
                }
            }
            &Edit::Hill { run, steps } => {
                let peak = (self.hill(run) + steps * HILL_STEP)
                    .clamp(-(PITCH_MAX as i32), PITCH_MAX as i32);
                let (a, b) = self.run_range(run);
                for (i, s) in self.layout.segments[a..b].iter_mut().enumerate() {
                    s.pitch_milli = dmath::round(peak as f64 * hill_shape(i, b - a)) as i16;
                }
            }
            &Edit::Width { run, steps } => {
                let (lo, hi) = HALF_WIDTH_RANGE;
                let w = (self.width(run) as i32 + steps * WIDTH_STEP).clamp(lo as i32, hi as i32);
                let (a, b) = self.run_range(run);
                for s in &mut self.layout.segments[a..b] {
                    s.half_width_cm = w as u16;
                }
                self.fit_to_road(a, b);
            }
            &Edit::Surface { run, surface } => {
                let (a, b) = self.run_range(run);
                for s in &mut self.layout.segments[a..b] {
                    s.surface = surface;
                }
            }
            &Edit::Drift { run, mark } => {
                let (a, b) = self.run_range(run);
                for s in &mut self.layout.segments[a..b] {
                    s.flags = (s.flags & !AUTHORED_FLAGS) | mark.flags();
                }
            }
            &Edit::Posts { run, side } => {
                let (a, b) = self.run_range(run);
                let mut seg = a as u32 + 1;
                while (seg as usize) < b && self.layout.objects.len() < MAX_OBJECTS {
                    let hw = self.layout.segments[seg as usize].half_width();
                    let x = match side {
                        Side::Left => -(hw + POST_OUT),
                        Side::Right => hw + POST_OUT,
                    };
                    let x_cm = dmath::round(x * 100.0) as i16;
                    let o = Object {
                        seg,
                        x_cm,
                        kind: ObjectKind::Post,
                    };
                    if !self.layout.objects.contains(&o) {
                        self.layout.objects.push(o);
                    }
                    seg += POST_EVERY;
                }
                self.sort_objects();
            }
            &Edit::Rock { run, lane } => {
                let (a, b) = self.run_range(run);
                let o = Object {
                    seg: ((a + b) / 2) as u32,
                    x_cm: LANES_CM[lane.min(2)],
                    kind: ObjectKind::Rock,
                };
                if self.layout.objects.len() >= MAX_OBJECTS || self.layout.objects.contains(&o) {
                    return false;
                }
                self.layout.objects.push(o);
                self.sort_objects();
            }
            &Edit::ClearObjects { run } => {
                let (a, b) = self.run_range(run);
                self.layout
                    .objects
                    .retain(|o| !(a..b).contains(&(o.seg as usize)));
            }
            &Edit::Car { run, lane } => {
                let (a, _) = self.run_range(run);
                let min = (TRAFFIC_MIN_START * 10.0) as u32;
                let car = TrafficCar {
                    z0_dm: (a as u32 * 40).max(min),
                    speed_dms: CAR_SPEED_DMS,
                    lane_cm: LANES_CM[lane.min(2)],
                };
                if self.layout.traffic.len() >= MAX_TRAFFIC || self.layout.traffic.contains(&car) {
                    return false;
                }
                self.layout.traffic.push(car);
            }
            &Edit::ClearCars { run } => {
                let (a, b) = self.run_range(run);
                let (za, zb) = (a as u32 * 40, b as u32 * 40);
                self.layout.traffic.retain(|c| !(za..zb).contains(&c.z0_dm));
            }
            &Edit::Gate { run } => {
                let on = self.gates_on(run);
                if on.is_empty() {
                    if self.layout.checkpoints.len() >= MAX_GATES {
                        return false;
                    }
                    let (_, b) = self.run_range(run);
                    let at = b.min(self.layout.segments.len() - 1) as u32;
                    if at == 0 {
                        return false;
                    }
                    self.layout.checkpoints.push(at);
                    clamp_gates(&mut self.layout.checkpoints);
                } else {
                    if on.len() >= self.layout.checkpoints.len() {
                        return false; // the last gate stays
                    }
                    let mut i = 0;
                    self.layout.checkpoints.retain(|_| {
                        i += 1;
                        !on.contains(&(i - 1))
                    });
                }
            }
            &Edit::MoveGate { run, segs } => {
                let Some(&g) = self.gates_on(run).first() else {
                    return false;
                };
                let cps = &mut self.layout.checkpoints;
                let lo = if g == 0 { 1 } else { cps[g - 1] as i64 + 1 };
                let hi = cps
                    .get(g + 1)
                    .map_or(self.layout.segments.len() as i64 - 1, |&n| n as i64 - 1);
                cps[g] = (cps[g] as i64 + segs as i64).clamp(lo, hi) as u32;
            }
            &Edit::Split { run } => {
                let (a, b) = self.run_range(run);
                if b - a < 2 * MIN_RUN {
                    return false;
                }
                self.runs.insert(run + 1, ((a + b) / 2) as u32);
            }
            &Edit::Join { run } => {
                if run + 1 >= self.runs.len() {
                    return false;
                }
                self.runs.remove(run + 1);
            }
            &Edit::Add { run } => {
                if self.layout.segments.len() + ADD_SEGS > MAX_SEGMENTS {
                    return false;
                }
                let (_, b) = self.run_range(run);
                let like = self.layout.segments[b - 1];
                let seg = Segment {
                    curve_milli: 0,
                    pitch_milli: 0,
                    flags: 0,
                    ..like
                };
                self.splice(b, 0, &[seg; ADD_SEGS]);
                self.runs.insert(run + 1, b as u32);
                for r in &mut self.runs[run + 2..] {
                    *r += ADD_SEGS as u32;
                }
            }
            &Edit::Delete { run } => {
                if self.runs.len() < 2 {
                    return false;
                }
                let (a, b) = self.run_range(run);
                self.splice(a, b - a, &[]);
                self.runs.remove(run);
                for r in &mut self.runs[run..] {
                    *r -= (b - a) as u32;
                }
                self.runs[0] = 0;
                self.repair_gates();
            }
            &Edit::Length { run, steps } => {
                let (a, b) = self.run_range(run);
                let n = (b - a) as i64;
                let m = (n + (steps * LEN_STEP) as i64).max(MIN_RUN as i64) as usize;
                let total = self.layout.segments.len() - (b - a) + m;
                if m == b - a || total > MAX_SEGMENTS {
                    return false;
                }
                self.resample(run, m);
            }
        }
        if *self == before {
            return false;
        }
        if self.layout.segments != before.layout.segments
            || self.layout.checkpoints != before.layout.checkpoints
            || self.layout.objects != before.layout.objects
            || self.layout.traffic != before.layout.traffic
        {
            self.layout.proof = None;
        }
        true
    }

    /// Keep objects in segment order (stable within a segment).
    fn sort_objects(&mut self) {
        self.layout.objects.sort_by_key(|o| o.seg);
    }

    /// After the road narrowed on `a..b`, move objects there back inside
    /// the verge and posts to the new edge.
    fn fit_to_road(&mut self, a: usize, b: usize) {
        let segs = &self.layout.segments;
        for o in &mut self.layout.objects {
            let i = o.seg as usize;
            if !(a..b).contains(&i) {
                continue;
            }
            let hw = segs[i].half_width();
            if o.kind == ObjectKind::Post && o.x().abs() > hw {
                let x = (hw + POST_OUT).copysign(o.x());
                o.x_cm = dmath::round(x * 100.0) as i16;
            }
        }
    }

    /// Replace `len` segments at `at` with `with`, moving everything past
    /// them: gates, objects and traffic on the replaced road are dropped.
    ///
    /// A gate is a line across the road at a segment's start, so a gate at
    /// `at` (the end of the run before) stays where it is and one at the end
    /// of the replaced road moves with the road after it. Objects and cars
    /// belong to the segment they stand or start on.
    fn splice(&mut self, at: usize, len: usize, with: &[Segment]) {
        let shift = with.len() as i64 - len as i64;
        let end = at + len;
        self.layout.segments.splice(at..end, with.iter().copied());
        let gate = |c: u32| -> Option<u32> {
            let c = c as usize;
            if c <= at {
                Some(c as u32)
            } else if c >= end {
                Some((c as i64 + shift) as u32)
            } else {
                None
            }
        };
        let seg = |s: u32| -> Option<u32> {
            let s = s as usize;
            if s < at {
                Some(s as u32)
            } else if s >= end {
                Some((s as i64 + shift) as u32)
            } else {
                None
            }
        };
        self.layout.checkpoints = self
            .layout
            .checkpoints
            .iter()
            .filter_map(|&c| gate(c))
            .collect();
        self.layout.objects = self
            .layout
            .objects
            .iter()
            .filter_map(|o| seg(o.seg).map(|seg| Object { seg, ..*o }))
            .collect();
        let (za, ze) = (at as u32 * 40, end as u32 * 40);
        self.layout.traffic = self
            .layout
            .traffic
            .iter()
            .filter_map(|c| {
                if c.z0_dm < za {
                    Some(*c)
                } else if c.z0_dm >= ze {
                    Some(TrafficCar {
                        z0_dm: (c.z0_dm as i64 + shift * 40) as u32,
                        ..*c
                    })
                } else {
                    None
                }
            })
            .collect();
    }

    /// Run `k` resampled to `m` segments (nearest neighbour), and what
    /// stands on it moved in proportion.
    fn resample(&mut self, k: usize, m: usize) {
        let (a, b) = self.run_range(k);
        let n = b - a;
        let old: Vec<Segment> = self.layout.segments[a..b].to_vec();
        let new: Vec<Segment> = (0..m).map(|j| old[j * n / m]).collect();
        let scale = |i: u32| -> u32 {
            let i = i as usize;
            if i < a {
                i as u32
            } else if i < b {
                (a + (i - a) * m / n) as u32
            } else {
                (i + m - n) as u32
            }
        };
        self.layout.segments.splice(a..b, new);
        let len = self.layout.segments.len() as u32;
        let mut gates: Vec<u32> = self
            .layout
            .checkpoints
            .iter()
            .map(|&c| scale(c).clamp(1, len - 1))
            .collect();
        clamp_gates(&mut gates);
        self.layout.checkpoints = gates;
        for o in &mut self.layout.objects {
            o.seg = scale(o.seg).min(len - 1);
        }
        self.sort_objects();
        for c in &mut self.layout.traffic {
            let seg = c.z0_dm / 40;
            let within = c.z0_dm % 40;
            c.z0_dm = scale(seg) * 40 + within;
        }
        for r in &mut self.runs[k + 1..] {
            *r = (*r as usize + m - n) as u32;
        }
    }

    /// After a delete: at least one gate, none at 0 or past the road.
    fn repair_gates(&mut self) {
        let len = self.layout.segments.len() as u32;
        let cps = &mut self.layout.checkpoints;
        cps.retain(|&c| c > 0 && c < len);
        cps.dedup();
        if cps.is_empty() && len > 1 {
            cps.push(len - 1);
        }
    }
}

/// Undo and redo: whole-draft snapshots.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Draft>,
    redo: Vec<Draft>,
    /// The last edit's coalescing key, while presses of it coalesce.
    last: Option<(u8, usize)>,
}

impl History {
    /// Apply `edit` to `draft`, keeping the step to undo. Repeated presses
    /// of a stepped edit on one run are one step.
    pub fn apply(&mut self, draft: &mut Draft, edit: &Edit) -> bool {
        let before = draft.clone();
        if !draft.apply(edit) {
            return false;
        }
        let key = edit.coalesce_key();
        if key.is_none() || key != self.last {
            self.push(before);
        }
        self.last = key;
        self.redo.clear();
        true
    }

    /// Replace the whole draft (a fork, a paste, a layout opened): one
    /// step, so UNDO brings the old draft back.
    pub fn replace(&mut self, draft: &mut Draft, with: Draft) {
        if *draft == with {
            return;
        }
        self.push(std::mem::replace(draft, with));
        self.redo.clear();
        self.last = None;
    }

    fn push(&mut self, d: Draft) {
        self.undo.push(d);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    /// End a run of coalescing presses: the next press is a new step.
    pub fn seal(&mut self) {
        self.last = None;
    }

    /// Step back. Returns whether there was a step.
    pub fn undo(&mut self, draft: &mut Draft) -> bool {
        let Some(d) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(draft, d));
        self.last = None;
        true
    }

    /// Step forward again.
    pub fn redo(&mut self, draft: &mut Draft) -> bool {
        let Some(d) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(draft, d));
        self.last = None;
        true
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        *self = History::default();
    }
}

/// Distance along the road of segment `seg`'s start, metres.
pub fn metres(seg: usize) -> u32 {
    (seg as f64 * SEG_LEN) as u32
}

#[cfg(test)]
mod tests;
