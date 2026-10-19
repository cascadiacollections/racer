//! Layouts: a track written down as data, to edit, share and race.
//! DESIGN §9.
//!
//! A generated track is a function of its key `(run_seed, index, branch)`:
//! nothing about it is stored, because the generator can always make it
//! again. A [`Layout`] is the opposite. It stores the track itself, the
//! same integers a [`Track`] is made of (segments, gates, roadside objects,
//! traffic), and generates nothing. That costs a few hundred characters
//! instead of a seed, and buys what a seed cannot:
//!
//! - **It can be edited.** A seed means what the generator makes of it, all
//!   or nothing; a layout is just numbers.
//! - **It outlives the generator.** A seed means a different track after
//!   any change to generation (a `SIM_EPOCH` bump); a layout means the same
//!   road for ever. Only how a car drives on it can change.
//!
//! Any generated track can be written down as a layout ([`fork`]: "fork
//! from a seed"); where it came from ([`Provenance`]) is kept beside the
//! code, never inside it, so a shared code says no more than the road.
//!
//! **Proof.** A layout is driven freely, with no timer, until it is
//! *proven*: [`Layout::prove`] holds it to the solver's own checks
//! (DESIGN §2.4) under the rules every custom race shares ([`RULES`]:
//! the base loadout, [`CUSTOM_MARGIN`]) and sets its checkpoint timers. A
//! proven layout carries its timers and a [`Proof`] stamp tying them to
//! this road, these rules and this `SIM_EPOCH`; only a proven layout races
//! with a timer or is shared as a code ([`Layout::to_code`]). An importer
//! trusts a stamp of its own epoch ([`Layout::standing`]) and proves again
//! when the epoch differs; proving again is deterministic, so it can also
//! check a stamp outright ([`Layout::verify`]).
//!
//! The share code is in `layout/code.rs`, the bundled courses in
//! `layout/bundled.rs`.

pub mod bundled;
pub mod code;

use std::fmt;
use std::sync::Arc;

use crate::gen::difficulty::{PERFECT_LEFT, TARGET_SEGS};
use crate::hash::Hasher;
use crate::race::MAX_RACE_TICKS;
use crate::rng::substream;
use crate::solver::{build, check, Checked, Effort, Failure, Report, Rules, Rung};
use crate::track::{flags, Object, Segment, Track, SEG_LEN, VERGE};
use crate::traffic::TrafficCar;
use crate::upgrade::Loadout;
use crate::TICKS_PER_SEC;

pub use code::{
    CodeError, CODE_PREFIXES, DRAFT_PREFIX, LAYOUT_FORMAT, MAX_CODE_CHARS, SHARE_PREFIX,
};

/// The most segments a layout holds: 8 km, over eight minutes of road.
pub const MAX_SEGMENTS: usize = 2048;
/// The fewest segments a raceable layout has: 500 m.
pub const MIN_SEGMENTS: usize = 125;
/// The most checkpoint gates, the finish included.
pub const MAX_GATES: usize = 8;
/// The most roadside objects.
pub const MAX_OBJECTS: usize = 512;
/// The most traffic cars.
pub const MAX_TRAFFIC: usize = 64;
/// The furthest a traffic car can start, decimetres: 100 km, far past
/// any road, and small enough that its steps code in 32 bits.
pub const MAX_TRAFFIC_Z0_DM: u32 = 1_000_000;
/// The longest name, characters.
pub const NAME_MAX: usize = 12;
/// What a name is spelt with: what a keypad can type, and a space.
pub const NAME_CHARSET: &[u8; 37] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ";

/// The flags a layout's author sets. [`flags::CREST`] is derived from the
/// elevation by [`Track::new`], so a layout never stores it.
pub const AUTHORED_FLAGS: u8 = flags::DRIFT_REQ | flags::DRIFT_HINT;

/// The sharpest turn a raceable layout has, milliradians per segment (a
/// 22 m radius). The generator's drift repair stops at 170.
pub const CURVE_MAX: i16 = 180;
/// The steepest grade, thousandths: the builder's own clamp.
pub const PITCH_MAX: i16 = 300;
/// Half widths a raceable layout keeps to, centimetres: 3 m to 8 m.
pub const HALF_WIDTH_RANGE: (u16, u16) = (300, 800);
/// The first gate stands at least this far up the road, segments (100 m):
/// a launch to reach speed in.
pub const LAUNCH_SEGS: u32 = 25;
/// Traffic starts at least this far up the road, metres (DESIGN §2.6).
pub const TRAFFIC_MIN_START: f64 = 120.0;
/// Traffic speeds a raceable layout keeps to, decimetres a second.
pub const TRAFFIC_SPEED_DMS: (u16, u16) = (50, 400);

/// The margin every gate of a custom race must keep, ticks: 1.5 s, the
/// middle of the run's 2.0 s to 1.0 s (DESIGN §2.4).
pub const CUSTOM_MARGIN: u32 = 3 * TICKS_PER_SEC / 2;

/// The loadout custom races are proven for and driven with.
pub const CUSTOM_LOADOUT: Loadout = Loadout::BASE;

/// Version of the custom-race proof rules ([`CUSTOM_MARGIN`],
/// [`CUSTOM_LOADOUT`], [`perfect_band`], the checks run). Folded into every
/// stamp, so a change to the rules leaves old stamps [`Standing::Stale`],
/// as an epoch bump does.
///
/// 1. Phase 1: the generated tracks' perfect-bot band (6–18 s) at any
///    length.
/// 2. Phase 2: the band scales with the layout's length ([`perfect_band`]).
///    Every layout that proved under 1 proves under 2 to the same timers:
///    the band only widened, and the timers do not depend on it.
pub const PROOF_RULES: u32 = 2;

/// The track index a layout's ghosts are kept under ([`Layout::ghost_key`]):
/// past any run's, so a layout's ghost never meets a generated track's.
pub const GHOST_INDEX: u32 = u32::MAX;

/// Tag for a layout's proving noise: "LYPR" in ASCII.
const PROVE_TAG: u64 = 0x4C59_5052;
/// Tag folded into a layout's digest: "LAYT".
const HASH_TAG: u64 = 0x4C41_5954;
/// Tag folded into a proof stamp: "STMP".
const STAMP_TAG: u64 = 0x5354_4D50;

/// The length the generated tracks' perfect-bot band ([`PERFECT_LEFT`]) is
/// set for, segments to the finish: about a minute of road
/// ([`TARGET_SEGS`]).
pub const BAND_REF_SEGS: u32 = TARGET_SEGS as u32;

/// The perfect bot's band for a layout whose finish is `finish_segs`
/// segments up the road, ticks (DESIGN §9.4).
///
/// The generated band, 6–18 s left, is set for a minute of road. The time a
/// perfect driver has in hand grows with the road (the timers are set from
/// a typical driver's times, 8–30% over the perfect one), so a fixed band
/// fails every long layout at the top and short ones at the bottom. So the
/// band stretches: past [`BAND_REF_SEGS`] its top edge grows in proportion
/// to the length (three minutes of road: 18 s becomes 54 s); below it, its
/// bottom edge shrinks in proportion (a 30 s sprint needs 3 s in hand, not
/// 6). The other edge stays, so the band only ever widens, and a layout of
/// a minute is held to exactly the generated band. Generated tracks never
/// use this: their band is [`PERFECT_LEFT`] whatever their length.
pub fn perfect_band(finish_segs: u32) -> (u32, u32) {
    let (lo, hi) = PERFECT_LEFT;
    let (f, r) = (finish_segs as u64, BAND_REF_SEGS as u64);
    if f < r {
        ((lo as u64 * f / r) as u32, hi)
    } else {
        (lo, (hi as u64 * f / r) as u32)
    }
}

/// The rules every custom race is proven under: [`CUSTOM_MARGIN`], the
/// base loadout only, the solver's default effort, and the difficulty bands
/// and drift-corner test applied as to a generated track, with the
/// perfect-bot band scaled to the layout's length ([`perfect_band`] of
/// `finish_segs`, the finish gate's segment). `seed` is the layout's own
/// noise seed.
pub fn rules(seed: u64, finish_segs: u32) -> Rules<'static> {
    const LOADOUTS: &[Loadout] = &[CUSTOM_LOADOUT];
    Rules {
        margin: CUSTOM_MARGIN,
        loadouts: LOADOUTS,
        seed,
        effort: Effort::default(),
        strict: true,
        perfect_left: perfect_band(finish_segs),
    }
}

/// The rules [`rules`] gives, for documentation and the tools.
pub const RULES: &str =
    "base loadout, 1.5 s margin at every gate, default effort, strict bands (perfect band scaled with length)";

/// A track written down. Every field is an integer the [`Track`] it builds
/// holds as it is; nothing is derived until [`Layout::track`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Layout {
    /// Up to [`NAME_MAX`] characters of [`NAME_CHARSET`], no leading,
    /// trailing or doubled space. Not part of the digest: renaming a layout
    /// keeps its ghosts.
    pub name: String,
    /// The road, in driving order. Flags are [`AUTHORED_FLAGS`] only.
    pub segments: Vec<Segment>,
    /// Gate segments, strictly ascending; the last is the finish line.
    pub checkpoints: Vec<u32>,
    /// Roadside objects, in segment order (objects on one segment keep
    /// their order).
    pub objects: Vec<Object>,
    /// Traffic, in the order it was written.
    pub traffic: Vec<TrafficCar>,
    /// The timers and stamp a proof set; `None` until proven.
    pub proof: Option<Proof>,
}

/// What [`Layout::prove`] certifies: the checkpoint timers, and a stamp
/// over them, the layout, [`PROOF_RULES`] and the epoch they were proven
/// in.
///
/// The stamp is a digest, not a signature: it catches a layout edited
/// after it was proven, timers from another epoch or other rules, and
/// damage the code's checksum missed, not a forger. A forged stamp gains
/// nothing a re-prove ([`Layout::verify`]) does not take away.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Proof {
    /// Timer allotment per section, ticks, one per checkpoint.
    pub allot_ticks: Vec<u32>,
    /// The `SIM_EPOCH` it was proven in.
    pub epoch: u32,
    /// [`stamp`] of the layout, the timers and the epoch.
    pub stamp: u32,
}

/// Where a proof stands, as an importer sees it without proving anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Standing {
    /// No proof: free drive only.
    Unproven,
    /// Proven in this epoch, under these rules, and the stamp matches.
    Proven,
    /// Proven in another epoch (or under other rules, which the stamp
    /// shows). The road is the same; whether the timers still hold is not
    /// known until it is proven again.
    Stale,
    /// The stamp does not match the layout and timers: edited after it was
    /// proven, or damaged. Prove again.
    BadStamp,
}

/// Something about a layout that stops it being built ([`Problem::structural`])
/// or raced with a timer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    // Structural: [`Layout::track`] refuses, the code cannot hold it.
    /// No segments, or more than [`MAX_SEGMENTS`].
    SegmentCount,
    /// No gates, more than [`MAX_GATES`], not strictly ascending, one on
    /// the first segment or past the last.
    Gates,
    /// More than [`MAX_OBJECTS`], one past the end, or not in segment order.
    Objects,
    /// More than [`MAX_TRAFFIC`] traffic cars, or one starting past
    /// [`MAX_TRAFFIC_Z0_DM`].
    Traffic,
    /// A flag outside [`AUTHORED_FLAGS`] on this segment.
    Flags(u32),
    /// A name too long, outside [`NAME_CHARSET`], or with a space at
    /// either end or two together.
    Name,
    /// A proof whose timer count is not the gate count, with a timer past
    /// [`MAX_RACE_TICKS`], or an epoch of `u32::MAX`.
    ProofShape,

    // Gameplay: the road can be driven freely but not proven.
    /// Fewer than [`MIN_SEGMENTS`].
    TooShort,
    /// The first gate stands within [`LAUNCH_SEGS`] of the start.
    NoLaunch,
    /// A turn sharper than [`CURVE_MAX`] on this segment.
    Curve(u32),
    /// A grade steeper than [`PITCH_MAX`] on this segment.
    Pitch(u32),
    /// A half width outside [`HALF_WIDTH_RANGE`] on this segment.
    Width(u32),
    /// This object stands past the verge.
    ObjectOffRoad(u32),
    /// This traffic car starts within [`TRAFFIC_MIN_START`] of the start, or
    /// past the finish.
    TrafficStart(u32),
    /// This traffic car's speed is outside [`TRAFFIC_SPEED_DMS`].
    TrafficSpeed(u32),
    /// This traffic car's lane is off the road where it starts.
    TrafficLane(u32),
}

impl Problem {
    /// Whether the layout cannot even be built as a track.
    pub fn structural(self) -> bool {
        matches!(
            self,
            Problem::SegmentCount
                | Problem::Gates
                | Problem::Objects
                | Problem::Traffic
                | Problem::Flags(_)
                | Problem::Name
                | Problem::ProofShape
        )
    }
}

/// Why [`Layout::prove`] did not prove a layout.
#[derive(Clone, Debug, PartialEq)]
pub enum ProveError {
    /// The layout has problems ([`Layout::check`]); nothing was simulated.
    Problems(Vec<Problem>),
    /// The solver's checks failed, and the ticks it took to find out.
    Failed(Failure, u64),
}

impl fmt::Display for ProveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ProveError::Problems(p) => write!(f, "layout problems: {p:?}"),
            ProveError::Failed(why, cost) => write!(f, "failed {why:?} after {cost} ticks"),
        }
    }
}

/// How [`Layout::verify`] found a layout's proof.
#[derive(Clone, Debug, PartialEq)]
pub enum Verified {
    /// No proof to verify.
    Unproven,
    /// Proven again to exactly the timers it carries.
    Confirmed,
    /// Proven again, to other timers (another epoch's, or a bad stamp's):
    /// the layout with its new proof.
    Reproved(Box<Layout>),
    /// It no longer passes; free drive only.
    Failed(ProveError),
}

/// Whether `name` is a layout name: see [`Layout::name`].
pub fn valid_name(name: &str) -> bool {
    name.len() <= NAME_MAX
        && name.bytes().all(|c| NAME_CHARSET.contains(&c))
        && !name.starts_with(' ')
        && !name.ends_with(' ')
        && !name.contains("  ")
}

/// The stamp over a layout's digest `hash`, its timers and `epoch`, under
/// [`PROOF_RULES`].
pub fn stamp(hash: u64, allot_ticks: &[u32], epoch: u32) -> u32 {
    stamp_under(PROOF_RULES, hash, allot_ticks, epoch)
}

/// [`stamp`] under rules version `rules`: how a stamp from older rules is
/// told apart ([`Standing::Stale`]) from a bad one.
pub fn stamp_under(rules: u32, hash: u64, allot_ticks: &[u32], epoch: u32) -> u32 {
    let mut h = Hasher::new();
    h.write_u64(STAMP_TAG);
    h.write_u32(rules);
    h.write_u32(epoch);
    h.write_u64(hash);
    h.write_usize(allot_ticks.len());
    for &a in allot_ticks {
        h.write_u32(a);
    }
    let d = h.finish();
    (d ^ (d >> 32)) as u32
}

impl Layout {
    /// Write `track` down: its segments (crest flags dropped, as they are
    /// derived), gates, objects and traffic. Its timers are not kept: a
    /// layout's timers come only from [`Layout::prove`].
    pub fn from_track(track: &Track) -> Layout {
        Layout {
            name: String::new(),
            segments: track
                .segments()
                .iter()
                .map(|s| Segment {
                    flags: s.flags & AUTHORED_FLAGS,
                    ..*s
                })
                .collect(),
            checkpoints: track.checkpoints().to_vec(),
            objects: track.objects().to_vec(),
            traffic: track.traffic().to_vec(),
            proof: None,
        }
    }

    /// The digest of the road: segments, gates, objects and traffic. Not
    /// the name and not the proof, so a renamed or re-proven layout is the
    /// same road (and keeps its ghosts, DESIGN §9.6).
    pub fn hash(&self) -> u64 {
        let mut h = Hasher::new();
        h.write_u64(HASH_TAG);
        h.write_usize(self.segments.len());
        for s in &self.segments {
            h.write_i32(s.curve_milli as i32);
            h.write_i32(s.pitch_milli as i32);
            h.write_u32(s.half_width_cm as u32);
            h.write_u8(s.surface as u8);
            h.write_u8(s.flags);
        }
        h.write_usize(self.checkpoints.len());
        for &c in &self.checkpoints {
            h.write_u32(c);
        }
        h.write_usize(self.objects.len());
        for o in &self.objects {
            h.write_u32(o.seg);
            h.write_i32(o.x_cm as i32);
            h.write_u8(o.kind as u8);
        }
        h.write_usize(self.traffic.len());
        for c in &self.traffic {
            h.write_u32(c.z0_dm);
            h.write_u32(c.speed_dms as u32);
            h.write_i32(c.lane_cm as i32);
        }
        h.finish()
    }

    /// The first structural problem, if any: one that stops the layout
    /// being built as a track or written as a code.
    pub fn structure(&self) -> Result<(), Problem> {
        let n = self.segments.len();
        if n == 0 || n > MAX_SEGMENTS {
            return Err(Problem::SegmentCount);
        }
        let cps = &self.checkpoints;
        if cps.is_empty()
            || cps.len() > MAX_GATES
            || cps[0] == 0
            || !cps.windows(2).all(|w| w[0] < w[1])
            || *cps.last().unwrap() as usize >= n
        {
            return Err(Problem::Gates);
        }
        if self.objects.len() > MAX_OBJECTS
            || self.objects.iter().any(|o| o.seg as usize >= n)
            || !self.objects.windows(2).all(|w| w[0].seg <= w[1].seg)
        {
            return Err(Problem::Objects);
        }
        if self.traffic.len() > MAX_TRAFFIC
            || self.traffic.iter().any(|c| c.z0_dm > MAX_TRAFFIC_Z0_DM)
        {
            return Err(Problem::Traffic);
        }
        if let Some(i) = self
            .segments
            .iter()
            .position(|s| s.flags & !AUTHORED_FLAGS != 0)
        {
            return Err(Problem::Flags(i as u32));
        }
        if !valid_name(&self.name) {
            return Err(Problem::Name);
        }
        if let Some(p) = &self.proof {
            if p.allot_ticks.len() != cps.len()
                || p.allot_ticks.iter().any(|&a| a > MAX_RACE_TICKS)
                || p.epoch == u32::MAX
            {
                return Err(Problem::ProofShape);
            }
        }
        Ok(())
    }

    /// Every problem, in a fixed order: a structural one alone (nothing
    /// after it can be judged), else every gameplay one, each naming the
    /// segment, object or car at fault. Empty means the layout can be
    /// proven (whether it passes is [`Layout::prove`]'s question).
    pub fn check(&self) -> Vec<Problem> {
        if let Err(p) = self.structure() {
            return vec![p];
        }
        let mut out = Vec::new();
        let n = self.segments.len();
        if n < MIN_SEGMENTS {
            out.push(Problem::TooShort);
        }
        if self.checkpoints[0] < LAUNCH_SEGS {
            out.push(Problem::NoLaunch);
        }
        let (wmin, wmax) = HALF_WIDTH_RANGE;
        for (i, s) in self.segments.iter().enumerate() {
            let i = i as u32;
            if s.curve_milli.unsigned_abs() > CURVE_MAX as u16 {
                out.push(Problem::Curve(i));
            }
            if s.pitch_milli.unsigned_abs() > PITCH_MAX as u16 {
                out.push(Problem::Pitch(i));
            }
            if !(wmin..=wmax).contains(&s.half_width_cm) {
                out.push(Problem::Width(i));
            }
        }
        for (i, o) in self.objects.iter().enumerate() {
            let s = &self.segments[o.seg as usize];
            if o.x().abs() > s.half_width() + VERGE {
                out.push(Problem::ObjectOffRoad(i as u32));
            }
        }
        let finish = *self.checkpoints.last().unwrap() as f64 * SEG_LEN;
        let (vmin, vmax) = TRAFFIC_SPEED_DMS;
        for (i, c) in self.traffic.iter().enumerate() {
            let i = i as u32;
            let z = c.z_at(0);
            if z < TRAFFIC_MIN_START || z >= finish {
                out.push(Problem::TrafficStart(i));
            }
            if !(vmin..=vmax).contains(&c.speed_dms) {
                out.push(Problem::TrafficSpeed(i));
            }
            let seg = ((z / SEG_LEN) as usize).min(n - 1);
            if c.x().abs() > self.segments[seg].half_width() {
                out.push(Problem::TrafficLane(i));
            }
        }
        out
    }

    /// The track to drive, untimed: free drive. A timed track is
    /// [`Layout::timed_track`].
    pub fn track(&self) -> Result<Track, Problem> {
        self.structure()?;
        Ok(Track::new(
            self.segments.clone(),
            self.checkpoints.clone(),
            self.objects.clone(),
            self.traffic.clone(),
        ))
    }

    /// The track with its proven timers: only for a layout whose
    /// [`Layout::standing`] is [`Standing::Proven`]. Anything else races
    /// untimed or is proven again first.
    pub fn timed_track(&self) -> Option<Track> {
        if self.standing() != Standing::Proven {
            return None;
        }
        let mut t = self.track().ok()?;
        t.set_allotments(self.proof.as_ref()?.allot_ticks.clone());
        Some(t)
    }

    /// The track a saved custom race replays on (a run file's layout,
    /// `replay::RunFile::layout`): with the timers its proof carries,
    /// whatever its [`Layout::standing`], because those are the timers it
    /// was raced with; untimed without one. The game saves a timed race's
    /// layout with the proof it raced under and a free drive's without.
    pub fn raced_track(&self) -> Result<Track, Problem> {
        let mut t = self.track()?;
        if let Some(p) = &self.proof {
            t.set_allotments(p.allot_ticks.clone());
        }
        Ok(t)
    }

    /// The layout's noise seed for proving: a pure function of the road.
    pub fn seed(&self) -> u64 {
        substream(self.hash(), PROVE_TAG)
    }

    /// Where its proof stands, without simulating anything.
    pub fn standing(&self) -> Standing {
        let Some(p) = &self.proof else {
            return Standing::Unproven;
        };
        if p.allot_ticks.len() != self.checkpoints.len() {
            return Standing::BadStamp;
        }
        if p.epoch != crate::SIM_EPOCH {
            return Standing::Stale;
        }
        let hash = self.hash();
        if p.stamp == stamp(hash, &p.allot_ticks, p.epoch) {
            Standing::Proven
        } else if (1..PROOF_RULES).any(|r| p.stamp == stamp_under(r, hash, &p.allot_ticks, p.epoch))
        {
            // Proven under older rules: genuine, not this build's proof.
            Standing::Stale
        } else {
            Standing::BadStamp
        }
    }

    /// Prove the layout: the solver's checks under the custom-race
    /// [`rules`], with no repairs (a layout is what its author made; a
    /// drift corner that is not demanding enough fails rather than being
    /// tightened). On success, the layout with its timers and a fresh
    /// stamp, and the evidence.
    pub fn prove(&self) -> Result<(Layout, Report), ProveError> {
        self.prove_with(Effort::default())
    }

    /// [`Layout::prove`] at another effort, for tests and the tools. A
    /// proof at any effort but the default is not what an importer checks
    /// against ([`Layout::verify`]).
    pub fn prove_with(&self, effort: Effort) -> Result<(Layout, Report), ProveError> {
        let problems = self.check();
        if !problems.is_empty() {
            return Err(ProveError::Problems(problems));
        }
        let track = self.track().map_err(|p| ProveError::Problems(vec![p]))?;
        let rules = Rules {
            effort,
            ..rules(self.seed(), self.checkpoints.last().copied().unwrap_or(0))
        };
        let mut cost = 0;
        match check(&track, &rules, 0, &mut cost) {
            Checked::Passed(timed, report) => {
                let allot = timed.allot_ticks().to_vec();
                let proof = Proof {
                    stamp: stamp(self.hash(), &allot, crate::SIM_EPOCH),
                    allot_ticks: allot,
                    epoch: crate::SIM_EPOCH,
                };
                let proven = Layout {
                    proof: Some(proof),
                    ..self.clone()
                };
                Ok((proven, report))
            }
            Checked::Failed(f) => Err(ProveError::Failed(f, cost)),
            Checked::WeakDrift(_) => Err(ProveError::Failed(Failure::DriftCorner, cost)),
        }
    }

    /// Trust, then verify: prove the layout again and compare. Proving is
    /// deterministic, so a proof from this epoch and these rules comes back
    /// [`Verified::Confirmed`]; one from another epoch, or a bad stamp,
    /// comes back with the timers this build gives it, or failing.
    pub fn verify(&self) -> Verified {
        let Some(old) = &self.proof else {
            return Verified::Unproven;
        };
        match self.prove() {
            Ok((proven, _)) if proven.proof.as_ref() == Some(old) => Verified::Confirmed,
            Ok((proven, _)) => Verified::Reproved(Box::new(proven)),
            Err(e) => Verified::Failed(e),
        }
    }

    /// The layout without its proof: what an edit leaves.
    pub fn unproven(&self) -> Layout {
        Layout {
            proof: None,
            ..self.clone()
        }
    }

    /// The layout's key in the ghost store: its digest as the "run seed",
    /// and [`GHOST_INDEX`], which no generated track has. Proven or not,
    /// renamed or not, the same road keeps the same ghost.
    pub fn ghost_key(&self) -> crate::ghost::TrackKey {
        (self.hash(), GHOST_INDEX, 0)
    }

    /// Length to the finish line, metres.
    pub fn length(&self) -> f64 {
        self.checkpoints.last().map_or(0.0, |&c| c as f64 * SEG_LEN)
    }
}

/// Where a forked layout came from: a generated track's key and the epoch
/// that generated it. Kept beside a layout's code (a file's `origin` line),
/// never inside it (DESIGN §9.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Provenance {
    /// The run seed.
    pub run_seed: u64,
    /// The track index in the run, from 0.
    pub index: u32,
    /// The branch taken to it.
    pub branch: u8,
    /// The `SIM_EPOCH` that generated it: the same key forks to another
    /// road in another epoch.
    pub epoch: u32,
}

impl fmt::Display for Provenance {
    /// `fork:<run seed, 0x + 16 hex>:<index>:<branch>:e<epoch>`.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "fork:{:#018x}:{}:{}:e{}",
            self.run_seed, self.index, self.branch, self.epoch
        )
    }
}

impl Provenance {
    /// Read [`Provenance`]'s text form; `None` for anything else.
    pub fn parse(text: &str) -> Option<Provenance> {
        let mut t = text.trim().strip_prefix("fork:")?.split(':');
        let seed = t.next()?.strip_prefix("0x")?;
        if seed.len() != 16 {
            return None;
        }
        let p = Provenance {
            run_seed: u64::from_str_radix(seed, 16).ok()?,
            index: t.next()?.parse().ok()?,
            branch: t.next()?.parse().ok()?,
            epoch: t.next()?.strip_prefix('e')?.parse().ok()?,
        };
        t.next().is_none().then_some(p)
    }
}

/// A forked layout and how the generator made its track.
#[derive(Clone, Debug)]
pub struct Fork {
    /// The layout, unproven.
    pub layout: Layout,
    /// Where it came from.
    pub provenance: Provenance,
    /// The ladder rung the generated track came from.
    pub rung: Rung,
}

/// Fork from a seed: generated track `(index, branch)` of run `run_seed`,
/// proven by the solver as a run would meet it, written down as a layout.
/// The layout is the road exactly (a layout's track equals the generated
/// one bar its timers); its timers are not kept, because they were set for
/// the loadouts a run reaches that track with, not a custom race's.
pub fn fork(run_seed: u64, index: u32, branch: u8) -> Fork {
    let built = build(run_seed, index, branch, Effort::default());
    Fork {
        layout: Layout::from_track(&built.track),
        provenance: Provenance {
            run_seed,
            index,
            branch,
            epoch: crate::SIM_EPOCH,
        },
        rung: built.rung,
    }
}

/// A layout read from a file: a code line and, optionally, its origin.
///
/// ```text
/// # comments and blank lines are skipped
/// RL… or RD…               the code (one line)
/// origin fork:0x…:2:1:e1    where it came from (optional)
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LayoutFile {
    /// The code's text.
    pub code: String,
    /// The origin line's text, if any (kept verbatim: a newer build's
    /// origin is not lost by an older one).
    pub origin: Option<String>,
}

impl LayoutFile {
    /// Read a layout file, or a bare code: the first line that starts with
    /// either code prefix (any case) is the code. `None` without one.
    pub fn parse(text: &str) -> Option<LayoutFile> {
        let mut code = None;
        let mut origin = None;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(o) = line.strip_prefix("origin ") {
                origin.get_or_insert_with(|| o.trim().to_string());
            } else if CODE_PREFIXES.iter().any(|pre| {
                line.get(..pre.len())
                    .is_some_and(|p| p.eq_ignore_ascii_case(pre))
            }) {
                code.get_or_insert_with(|| line.to_string());
            }
        }
        Some(LayoutFile {
            code: code?,
            origin,
        })
    }

    /// The text form: the code, then the origin line if any.
    pub fn encode(&self) -> String {
        match &self.origin {
            Some(o) => format!("{}\norigin {o}\n", self.code),
            None => format!("{}\n", self.code),
        }
    }
}

/// A layout's track behind an `Arc`, timed when it is proven: how the game
/// races one.
pub fn race_track(layout: &Layout) -> Result<(Arc<Track>, bool), Problem> {
    match layout.timed_track() {
        Some(t) => Ok((Arc::new(t), true)),
        None => Ok((Arc::new(layout.track()?), false)),
    }
}

#[cfg(test)]
mod tests;
