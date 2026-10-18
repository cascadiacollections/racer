//! Overlays: surfaces, roadside objects, traffic and narrows, placed on a
//! laid-out track. DESIGN §2.6.
//!
//! Each layer draws from its own substream, so adding a draw to one never
//! moves another. The rules that keep a track fair live here:
//!
//! - Nothing is placed on the launch, the closing straights at the gates or
//!   the finish.
//! - A drift-required corner and its approach get no ice and no traffic, so
//!   the corner is the only thing being taught there (DESIGN §2.5); gravel
//!   on it, which rewards the drift, comes only after its first track.
//! - Traffic is timed to meet the player on straights and sweeps, never in
//!   the middle of a tight corner.

use super::grammar::{Kind, Layout, Span};
use crate::rng::Pcg32;
use crate::track::{ObjectKind, Surface, SEG_LEN};
use crate::traffic::TrafficCar;

/// Pace used to time traffic: about the human bot's average speed, m/s.
/// Only where a pack meets the player depends on it, never whether it can
/// be passed.
const MEET_PACE: f64 = 38.0;

/// Traffic starts at least this far up the road, metres, clear of the grid.
const MIN_START: f64 = 120.0;

/// Whether `s` is a span overlays may use.
fn open(s: &Span) -> bool {
    !matches!(s.kind, Kind::Launch | Kind::Finish) && s.end - s.start > 10
}

/// Whether segments `a..b` come near a drift-required corner: from 40
/// segments before it to 20 after.
fn near_drift(spans: &[Span], a: usize, b: usize) -> bool {
    spans
        .iter()
        .any(|s| s.kind == Kind::Carousel && a < s.end + 20 && b + 40 > s.start)
}

/// Lay surfaces: gravel on bends, puddles on straights, ice (from track 3)
/// on straights and sweeps.
pub fn surfaces(r: &mut Pcg32, lay: &mut Layout, d: f64, index: u32) {
    let spans = lay.spans.clone();
    for s in spans.iter().filter(|s| open(s)) {
        let len = s.end - s.start;
        match s.kind {
            Kind::Carousel if index >= 3 && r.next_f64() < 0.35 => {
                lay.builder.surface(s.start, len, Surface::Gravel);
            }
            Kind::Hairpin | Kind::SBend | Kind::Sweep if r.next_f64() < 0.1 + 0.25 * d => {
                lay.builder.surface(s.start, len, Surface::Gravel);
            }
            Kind::Straight | Kind::Sweep
                if index >= 2 && r.next_f64() < 0.15 * d && !near_drift(&spans, s.start, s.end) =>
            {
                let segs = r.range_u32(8, 14) as usize;
                let at = s.start + r.range_u32(0, (len.saturating_sub(segs)) as u32) as usize;
                lay.builder.surface(at, segs, Surface::Ice);
            }
            Kind::Straight | Kind::Dip if r.next_f64() < 0.25 => {
                let segs = r.range_u32(3, 5) as usize;
                let at = s.start + r.range_u32(0, (len - segs) as u32) as usize;
                lay.builder.surface(at, segs, Surface::Puddle);
            }
            _ => {}
        }
    }
}

/// Stand objects: posts along the outside of bends (punishing running
/// wide), post rows pinching a lane on straights, and rocks in a lane (from
/// track 2).
pub fn objects(r: &mut Pcg32, lay: &mut Layout, d: f64, index: u32) {
    let spans = lay.spans.clone();
    for s in spans.iter().filter(|s| open(s)) {
        let hw = lay.builder.segments_mut()[s.start].half_width();
        match s.kind {
            Kind::Hairpin | Kind::Carousel | Kind::SBend if r.next_f64() < 0.2 + 0.4 * d => {
                for seg in (s.start..s.end).step_by(3) {
                    let curve = lay.builder.segments_mut()[seg].curve_milli;
                    if curve != 0 {
                        // Outside of a right-hander is the left.
                        let side = if curve > 0 { -1.0 } else { 1.0 };
                        lay.builder.object(seg, side * (hw + 1.0), ObjectKind::Post);
                    }
                }
            }
            Kind::Straight if index >= 1 && r.next_f64() < 0.15 + 0.3 * d => {
                let side = if r.next_u32() & 1 == 0 { 1.0 } else { -1.0 };
                let segs = r.range_u32(6, 12) as usize;
                let len = s.end - s.start;
                let at = s.start + r.range_u32(0, len.saturating_sub(segs) as u32) as usize;
                for seg in (at..at + segs).step_by(2) {
                    lay.builder.object(seg, side * (hw - 1.0), ObjectKind::Post);
                }
            }
            Kind::Straight | Kind::Dip if index >= 2 && r.next_f64() < 0.3 * d => {
                let lane = r.range_u32(0, 2) as usize;
                let x = super::super::bot::control::lane_x(lane, hw);
                let at = s.start + (s.end - s.start) / 2;
                lay.builder.object(at, x, ObjectKind::Rock);
            }
            _ => {}
        }
    }
}

/// Place traffic packs: up to `round(3·d)`, each one to three cars in
/// different lanes, timed to meet the player on a straight or sweep.
pub fn traffic(r: &mut Pcg32, lay: &mut Layout, d: f64) {
    let packs = crate::dmath::round(3.0 * d) as usize;
    let spots: Vec<Span> = lay
        .spans
        .iter()
        .filter(|s| open(s) && matches!(s.kind, Kind::Straight | Kind::Sweep | Kind::Dip))
        .filter(|s| !near_drift(&lay.spans, s.start, s.end))
        .copied()
        .collect();
    if spots.is_empty() {
        return;
    }
    for _ in 0..packs {
        let s = spots[r.range_u32(0, spots.len() as u32 - 1) as usize];
        let meet = (s.start + (s.end - s.start) / 2) as f64 * SEG_LEN;
        let hw = lay.builder.segments_mut()[s.start].half_width();
        let mut lanes = [0usize, 1, 2];
        r.shuffle(&mut lanes);
        let cars = 1 + r.range_u32(0, (1.0 + 2.0 * d) as u32) as usize;
        for &lane in lanes.iter().take(cars.min(2)) {
            let v = r.range_f64(14.0, 24.0);
            let t = meet / MEET_PACE + 2.0;
            let z0 = meet - v * t + r.range_f64(-15.0, 15.0);
            // A car that would start behind the line meets the player too
            // early to be the planned encounter; leave it out rather than
            // park it on the grid.
            if z0 >= MIN_START {
                let x = super::super::bot::control::lane_x(lane, hw);
                lay.builder.traffic(TrafficCar::new(z0, v, x));
            }
        }
    }
}

/// Narrow a few straights to 4.5 m half width (harder tracks only).
pub fn narrows(r: &mut Pcg32, lay: &mut Layout, d: f64) {
    for s in lay
        .spans
        .iter()
        .filter(|s| open(s) && s.kind == Kind::Straight)
    {
        if r.next_f64() < 0.3 * d - 0.1 {
            for seg in &mut lay.builder.segments_mut()[s.start..s.end] {
                seg.half_width_cm = 450;
            }
        }
    }
}
