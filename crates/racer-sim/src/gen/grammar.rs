//! The track grammar: the shape of a track before any hazard is placed.
//! DESIGN §2.3.
//!
//! ```text
//! Track   := Launch Straight (Section Checkpoint){3|4} Finish
//! Section := Element+ Straight
//! Element := Straight | Sweep | Hairpin | SBend | Chicane | Crest | Dip | Carousel
//! ```
//!
//! Each section is filled with elements drawn by weight until it reaches its
//! share of the track's length, then closed with a short straight so every
//! checkpoint gate stands on straight road. Drift corners (carousels) are
//! slotted into chosen sections, always behind a long crest-free straight
//! so the driver sees them coming (DESIGN §2.5).
//!
//! The grammar lays geometry only: bends and hills. Surfaces, objects and
//! traffic are overlays placed afterwards (`placement.rs`) from their own
//! random streams, so a change to one layer never reshuffles another.

use crate::rng::Pcg32;
use crate::track::{flags, Builder};

/// What an element is, for the overlays and the repair step to read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The standing start and the first straight.
    Launch,
    /// Straight road.
    Straight,
    /// A long gentle bend, flat out or nearly.
    Sweep,
    /// A tight bend that needs braking.
    Hairpin,
    /// Two medium bends, opposite hands.
    SBend,
    /// Two short sharp bends, opposite hands.
    Chicane,
    /// A blind crest, sometimes with a bend just past it.
    Crest,
    /// A dip: down and up again.
    Dip,
    /// A long tight corner made for drifting; drift-required or a hint.
    Carousel,
    /// The run-off after the finish line.
    Finish,
}

/// One laid element: its kind and segments `start..end`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Span {
    /// What it is.
    pub kind: Kind,
    /// First segment.
    pub start: usize,
    /// One past the last segment.
    pub end: usize,
    /// The section it is in (0-based); the launch is in section 0.
    pub section: usize,
}

/// A track's geometry, ready for overlays.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The builder holding the geometry and the checkpoints.
    pub builder: Builder,
    /// Every element, in order.
    pub spans: Vec<Span>,
    /// How many sections (and checkpoints).
    pub sections: usize,
}

/// Straight segments a drift corner needs before it with no crest: about
/// two seconds at speed.
pub const DRIFT_APPROACH: usize = 25;

fn sign(r: &mut Pcg32) -> i16 {
    if r.next_u32() & 1 == 0 {
        1
    } else {
        -1
    }
}

fn lerp(r: &mut Pcg32, lo: f64, hi: f64) -> f64 {
    r.range_f64(lo, hi)
}

/// A drift corner behind its crest-free approach straight: drift-required
/// (long and tight) or the first track's hint (shorter, gentler).
fn carousel(r: &mut Pcg32, b: &mut Builder, spans: &mut Vec<Span>, section: usize, required: bool) {
    let start = b.len();
    b.straight(DRIFT_APPROACH);
    mark(b, spans, Kind::Straight, start, section);
    let start = b.len();
    let (segs, milli, flag) = if required {
        (
            r.range_u32(45, 65) as usize,
            lerp(r, 115.0, 140.0) as i16,
            flags::DRIFT_REQ,
        )
    } else {
        (36, 100, flags::DRIFT_HINT)
    };
    b.bend(segs, milli * sign(r), 6);
    b.flag(start, segs, flag);
    mark(b, spans, Kind::Carousel, start, section);
}

/// Record the element laid from `start` to the builder's end.
fn mark(b: &Builder, spans: &mut Vec<Span>, kind: Kind, start: usize, section: usize) {
    spans.push(Span {
        kind,
        start,
        end: b.len(),
        section,
    });
}

/// Lay out a track of difficulty `d` with `drift_req` drift-required
/// corners (and, with `hint`, one drift-hint corner), drawing from `r`.
/// Without `hills` it lays no crests or dips.
pub fn lay_out(r: &mut Pcg32, d: f64, drift_req: usize, hint: bool, hills: bool) -> Layout {
    let sections = if r.next_f64() < 0.3 + 0.4 * d { 4 } else { 3 };
    let mut b = Builder::new();
    let mut spans = Vec::new();

    b.straight(30);
    mark(&b, &mut spans, Kind::Launch, 0, 0);

    // Which sections get the carousels: distinct, drawn without bias.
    let mut order: Vec<usize> = (0..sections).collect();
    r.shuffle(&mut order);
    let carousels = drift_req + usize::from(hint);
    let carousel_in: Vec<usize> = order.into_iter().take(carousels.min(sections)).collect();

    let per = (super::difficulty::TARGET_SEGS - 30 - 25) / sections;
    let weights = [
        30,                                              // Straight
        30,                                              // Sweep
        (10.0 + 30.0 * d) as u32,                        // Hairpin
        (10.0 + 20.0 * d) as u32,                        // SBend
        (20.0 * d) as u32,                               // Chicane
        if hills { (5.0 + 20.0 * d) as u32 } else { 0 }, // Crest
        if hills { 10 } else { 0 },                      // Dip
    ];
    let mut required_left = drift_req;
    for section in 0..sections {
        let goal = b.len() + per;
        // A carousel goes in as this section's first or second element.
        let mut carousel_at = carousel_in.contains(&section).then(|| r.range_u32(0, 1));
        let mut placed = 0;
        while b.len() + 8 < goal || carousel_at.is_some() {
            if carousel_at.is_some_and(|at| at <= placed || b.len() + 8 >= goal) {
                carousel_at = None;
                let required = required_left > 0;
                required_left = required_left.saturating_sub(1);
                carousel(r, &mut b, &mut spans, section, required);
                placed += 1;
                continue;
            }
            let start = b.len();
            let kind = match r.weighted(&weights) {
                0 => {
                    b.straight(r.range_u32(15, 40) as usize);
                    Kind::Straight
                }
                1 => {
                    let segs = r.range_u32(25, 50) as usize;
                    b.bend(segs, lerp(r, 10.0, 28.0) as i16 * sign(r), segs / 4);
                    Kind::Sweep
                }
                2 => {
                    let peak = 70.0 + 60.0 * d * r.next_f64();
                    b.bend(r.range_u32(28, 40) as usize, peak as i16 * sign(r), 6);
                    Kind::Hairpin
                }
                3 => {
                    let s = sign(r);
                    let peak = lerp(r, 35.0, 45.0 + 25.0 * d) as i16;
                    b.bend(r.range_u32(16, 24) as usize, peak * s, 5);
                    b.bend(r.range_u32(16, 24) as usize, -peak * s, 5);
                    Kind::SBend
                }
                4 => {
                    let s = sign(r);
                    let peak = lerp(r, 60.0, 70.0 + 20.0 * d) as i16;
                    b.bend(r.range_u32(8, 10) as usize, peak * s, 3);
                    b.straight(2);
                    b.bend(r.range_u32(8, 10) as usize, -peak * s, 3);
                    Kind::Chicane
                }
                5 => {
                    let segs = r.range_u32(30, 45) as usize;
                    b.straight(segs);
                    b.hill(start, segs, lerp(r, 4.0, 5.0 + 5.0 * d));
                    if r.next_f64() < d - 0.3 {
                        // A bend just over the top: the blind corner.
                        b.bend(20, lerp(r, 40.0, 70.0) as i16 * sign(r), 4);
                    }
                    Kind::Crest
                }
                _ => {
                    let segs = r.range_u32(30, 40) as usize;
                    b.straight(segs);
                    b.hill(start, segs, -lerp(r, 3.0, 7.0));
                    Kind::Dip
                }
            };
            mark(&b, &mut spans, kind, start, section);
            placed += 1;
        }
        let start = b.len();
        b.straight(8).checkpoint();
        mark(&b, &mut spans, Kind::Straight, start, section);
    }
    let start = b.len();
    b.straight(25);
    mark(&b, &mut spans, Kind::Finish, start, sections - 1);
    Layout {
        builder: b,
        spans,
        sections,
    }
}
