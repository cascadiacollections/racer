//! Solvability: every track is proven drivable before it is shown.
//! DESIGN §2.4.
//!
//! "Drivable" has a precise meaning here, checked in this order:
//!
//! 1. **Calibrate.** Find the weakest loadout the track can be met with and
//!    set the timer from the human bot's times on it (`gen::calibrate`).
//!    A track where more than a quarter of those drives hit something is
//!    rejected here: its timer would be calibrated around a crash, and a
//!    crash a typical driver cannot avoid is a flaw, not a challenge.
//! 2. **Ceiling.** The perfect bot, on that loadout, must finish with
//!    between 6 and 18 s left ([`PERFECT_LEFT`]): less means even flawless
//!    driving barely makes it, more means the timer never bites.
//! 3. **Skill gap.** The human's median time must be 8–30% over the
//!    perfect bot's ([`SKILL_GAP`]): enough that skill shows, not so much
//!    that the track is a lottery.
//! 4. **Drift corners.** Every drift-required corner is checked by two
//!    perfect bots: one that may drift must pass, and one held to grip must
//!    fail the timer or lose at least [`DRIFT_LOSS`] seconds there. A
//!    corner that is not demanding enough is tightened ("repair κ") and the
//!    track recalibrated, up to [`DRIFT_REPAIRS`] times.
//! 5. **Floor.** Against *every* reachable loadout, the human bot, with
//!    noise seeds calibration never saw, must keep the required margin at
//!    every checkpoint in at least 15 of 16 trials ([`MARGIN_PASSES`]).
//!
//! A track that fails climbs down the ladder, as in Salt Line: up to
//! [`ATTEMPTS`] fresh draws, then up to [`REPAIRS`] softenings of the best
//! failed draw's worst section, then the gentle fallback, which is still
//! calibrated and must still pass the floor (its difficulty bands are only
//! reported: a gentle track is allowed to be easy). A fallback that failed
//! the floor too would be *unvalidated*; the batch gate requires none.

use std::sync::Arc;

use crate::bot::human::{HumanBot, HumanPlan};
use crate::bot::perfect::{perfect_run, PerfectRun, LEAD};
use crate::bot::{corners, drive};
use crate::gen::calibrate::{calibrate, weakest, CALIBRATION_TRIALS};
use crate::gen::difficulty::{required_margin, PERFECT_LEFT, SKILL_GAP};
use crate::gen::{fallback, generate, soften, Draft};
use crate::race::{Phase, Race};
use crate::rng::{substream, track_seed};
use crate::track::{flags, Track};
use crate::upgrade::{reachable, Loadout};

/// Fresh draws before repairing.
pub const ATTEMPTS: u32 = 8;
/// Softenings of the best failed draw before the fallback.
pub const REPAIRS: u32 = 2;
/// Tightenings of a drift corner that is not demanding enough.
pub const DRIFT_REPAIRS: u32 = 2;
/// Seconds the grip-only perfect bot must lose on a drift-required corner.
pub const DRIFT_LOSS: f64 = 1.5;
/// Human trials per loadout in verification.
pub const VERIFY_TRIALS: u64 = 16;
/// Of [`VERIFY_TRIALS`], how many must keep the margin.
pub const MARGIN_PASSES: u64 = 15;
/// Segments after a drift corner over which its time is measured, so the
/// boost out of it counts.
pub const DRIFT_TAIL: usize = 40;

/// Tag for verification noise seeds: "VERI" in ASCII.
const VERIFY_TAG: u64 = 0x5645_5249;

/// How hard to look: trial counts, so a batch can trade confidence for time.
#[derive(Clone, Copy, Debug)]
pub struct Effort {
    /// Human trials for calibration.
    pub calibration: u64,
    /// Human trials per loadout for verification.
    pub verify: u64,
    /// Of those, how many must keep the margin.
    pub passes: u64,
}

impl Default for Effort {
    fn default() -> Self {
        Effort {
            calibration: CALIBRATION_TRIALS,
            verify: VERIFY_TRIALS,
            passes: MARGIN_PASSES,
        }
    }
}

/// Which rung of the ladder a track came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rung {
    /// Fresh draw number `n` (0-based) passed.
    Drawn(u32),
    /// A softened draw passed.
    Repaired,
    /// The gentle fallback passed the floor.
    Fallback,
    /// Nothing passed, not even the fallback's floor. Never acceptable.
    Unvalidated,
}

/// Why a candidate failed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Failure {
    /// A calibration trial did not finish.
    Undrivable,
    /// Too many calibration trials hit something.
    CrashProne,
    /// The perfect bot did not finish in time.
    PerfectFailed,
    /// The perfect bot's time left, ticks, is outside the band.
    PerfectBand(u32),
    /// The skill gap is outside the band.
    SkillGap(f64),
    /// A drift-required corner stayed undemanding after its repairs.
    DriftCorner,
    /// A loadout's human trials kept the margin too rarely; the section
    /// where most of them fell short.
    Margins {
        /// Trials that kept the margin.
        passes: u64,
        /// The section with the most shortfalls.
        section: usize,
    },
}

/// The two-bot test of one drift-required corner.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DriftCheck {
    /// The corner's first segment.
    pub start: usize,
    /// Seconds the grip-only bot lost over the corner and its exit.
    pub loss: f64,
    /// Whether the grip-only bot ran out of time.
    pub grip_failed: bool,
    /// Times the corner was tightened.
    pub repairs: u32,
}

/// The evidence a validated track carries.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The loadout the timer was calibrated on.
    pub calibrated_on: Loadout,
    /// Reachable loadouts verified.
    pub loadouts: usize,
    /// Perfect bot's time left at the finish, ticks.
    pub perfect_left: u32,
    /// Perfect bot's finishing tick.
    pub perfect_ticks: u32,
    /// Human median finishing tick, calibration trials.
    pub human_ticks: u32,
    /// `(human − perfect) / perfect`.
    pub skill_gap: f64,
    /// Smallest margin any verification trial kept, ticks (0 if one ran
    /// out of time).
    pub worst_margin: u32,
    /// Fewest passes over the loadouts.
    pub fewest_passes: u64,
    /// The drift-required corners' checks.
    pub drift: Vec<DriftCheck>,
    /// Ticks simulated proving this track, every rung included.
    pub cost: u64,
}

/// A validated track and how it got there.
#[derive(Clone, Debug)]
pub struct Built {
    /// The calibrated track.
    pub track: Arc<Track>,
    /// The rung it came from.
    pub rung: Rung,
    /// The evidence.
    pub report: Report,
    /// Why each failed candidate before it failed, in order.
    pub failures: Vec<Failure>,
}

/// Build and prove track `(index, branch)` of run `run_seed`.
pub fn build(run_seed: u64, index: u32, branch: u8, effort: Effort) -> Built {
    let seed = track_seed(run_seed, index, branch);
    let loadouts = reachable(run_seed, index, branch);
    let mut failures = Vec::new();
    let mut cost = 0;
    let mut best: Option<(Draft, u64, usize)> = None;
    let done =
        |draft: &Draft, rung, strict, failures: &mut Vec<Failure>, cost: &mut u64| match validate(
            draft, index, &loadouts, seed, effort, strict,
        ) {
            Ok((track, mut report)) => {
                *cost += report.cost;
                report.cost = *cost;
                Some((track, rung, report))
            }
            Err((f, c)) => {
                *cost += c;
                failures.push(f);
                None
            }
        };
    for attempt in 0..ATTEMPTS {
        let draft = generate(run_seed, index, branch, attempt);
        if let Some((track, rung, report)) =
            done(&draft, Rung::Drawn(attempt), true, &mut failures, &mut cost)
        {
            return Built {
                track,
                rung,
                report,
                failures,
            };
        }
        if let Some(Failure::Margins { passes, section }) = failures.last().copied() {
            if best.as_ref().is_none_or(|b| passes > b.1) {
                best = Some((draft, passes, section));
            }
        }
    }
    if let Some((mut draft, _, mut section)) = best {
        for _ in 0..REPAIRS {
            draft = soften(&draft, section);
            if let Some((track, rung, report)) =
                done(&draft, Rung::Repaired, true, &mut failures, &mut cost)
            {
                return Built {
                    track,
                    rung,
                    report,
                    failures,
                };
            }
            match failures.last() {
                Some(Failure::Margins { section: s, .. }) => section = *s,
                _ => break,
            }
        }
    }
    let draft = fallback(run_seed, index, branch);
    if let Some((track, rung, report)) =
        done(&draft, Rung::Fallback, false, &mut failures, &mut cost)
    {
        return Built {
            track,
            rung,
            report,
            failures,
        };
    }
    let mut track = draft.track();
    track.set_allotments(vec![0; track.checkpoints().len()]);
    Built {
        track: Arc::new(track),
        rung: Rung::Unvalidated,
        report: Report {
            cost,
            ..Report::default()
        },
        failures,
    }
}

/// Run every check on a draft. `strict` applies the difficulty bands and
/// the drift-corner test; the fallback runs without them. On failure,
/// returns why and the ticks it cost to find out.
fn validate(
    draft: &Draft,
    index: u32,
    loadouts: &[Loadout],
    seed: u64,
    effort: Effort,
    strict: bool,
) -> Result<(Arc<Track>, Report), (Failure, u64)> {
    let margin = required_margin(index);
    let mut cost = 0;
    let mut draft = draft.clone();
    let mut drift_repairs = 0;
    loop {
        // 1. Calibrate on the weakest loadout.
        let untimed = Arc::new(draft.track());
        let (calib_on, _, c) = weakest(&untimed, loadouts);
        cost += c;
        let Some(cal) = calibrate(&untimed, calib_on, seed, margin, effort.calibration) else {
            return Err((Failure::Undrivable, cost));
        };
        cost += cal.cost;
        if cal.crashed * 4 > effort.calibration {
            return Err((Failure::CrashProne, cost));
        }
        let mut timed = draft.track();
        timed.set_allotments(cal.allot.clone());
        let track = Arc::new(timed);

        // 2. Ceiling.
        let params = calib_on.params();
        let best = perfect_run(&track, &params, true);
        cost += best.cost;
        if best.race.phase() != Phase::Finished {
            return Err((Failure::PerfectFailed, cost));
        }
        let perfect_left = *best.race.margins().last().unwrap();
        let human_ticks = cal.finishes[cal.finishes.len() / 2];
        let gap = (human_ticks as f64 - best.race.tick() as f64) / best.race.tick() as f64;
        if strict {
            if !(PERFECT_LEFT.0..=PERFECT_LEFT.1).contains(&perfect_left) {
                return Err((Failure::PerfectBand(perfect_left), cost));
            }
            if !(SKILL_GAP.0..=SKILL_GAP.1).contains(&gap) {
                return Err((Failure::SkillGap(gap), cost));
            }
        }

        // 3. Drift-required corners.
        let mut drift = Vec::new();
        if strict {
            let grip = perfect_run(&track, &params, false);
            cost += grip.cost;
            drift = drift_checks(&track, &best, &grip, drift_repairs);
            if let Some(weak) = drift.iter().find(|d| !d.grip_failed && d.loss < DRIFT_LOSS) {
                if drift_repairs == DRIFT_REPAIRS {
                    return Err((Failure::DriftCorner, cost));
                }
                drift_repairs += 1;
                tighten(&mut draft, weak.start);
                continue;
            }
        }

        // 4. Floor, on every reachable loadout.
        let mut worst_margin = u32::MAX;
        let mut fewest = effort.verify;
        for &l in loadouts {
            let p = l.params();
            let plan = HumanPlan::new(&track, &p);
            let mut passes = 0;
            let mut short = vec![0u32; track.checkpoints().len()];
            for trial in 0..effort.verify {
                let mut race = Race::new(track.clone(), p);
                drive(
                    &mut race,
                    &mut HumanBot::new(plan.clone(), substream(seed, VERIFY_TAG + trial)),
                );
                cost += race.tick() as u64;
                worst_margin = worst_margin.min(race.min_margin());
                if race.phase() == Phase::Finished && race.min_margin() >= margin {
                    passes += 1;
                } else {
                    let k = race
                        .margins()
                        .iter()
                        .position(|&m| m < margin)
                        .unwrap_or(race.margins().len());
                    let last = short.len() - 1;
                    short[k.min(last)] += 1;
                }
                if trial + 1 - passes > effort.verify - effort.passes {
                    break;
                }
            }
            fewest = fewest.min(passes);
            if passes < effort.passes {
                let section = (0..short.len())
                    .max_by_key(|&k| (short[k], usize::MAX - k))
                    .unwrap();
                return Err((Failure::Margins { passes, section }, cost));
            }
        }

        return Ok((
            track,
            Report {
                calibrated_on: calib_on,
                loadouts: loadouts.len(),
                perfect_left,
                perfect_ticks: best.race.tick(),
                human_ticks,
                skill_gap: gap,
                worst_margin,
                fewest_passes: fewest,
                drift,
                cost,
            },
        ));
    }
}

/// The two-bot test on every drift-required corner: time from the corner's
/// approach to past its exit, grip-only against the best plan.
fn drift_checks(
    track: &Track,
    best: &PerfectRun,
    grip: &PerfectRun,
    repairs: u32,
) -> Vec<DriftCheck> {
    let params = best.race.params();
    corners(track, params)
        .iter()
        .filter(|c| {
            track.segments()[c.start..c.end]
                .iter()
                .any(|s| s.has(flags::DRIFT_REQ))
        })
        .map(|c| {
            let (a, b) = (
                c.start.saturating_sub(LEAD),
                (c.end + DRIFT_TAIL).min(track.len() - 1),
            );
            let with = best.trace.ticks_between(a, b);
            let without = grip.trace.ticks_between(a, b);
            let grip_failed = grip.race.phase() == Phase::TimeUp && without.is_none();
            let loss = match (with, without) {
                (Some(w), Some(g)) => (g as f64 - w as f64) * crate::DT,
                _ => 0.0,
            };
            DriftCheck {
                start: c.start,
                loss,
                grip_failed,
                repairs,
            }
        })
        .collect()
}

/// Tighten the drift corner starting near segment `start` by 15%.
fn tighten(draft: &mut Draft, start: usize) {
    let segs = draft.layout.builder.segments_mut();
    let mut i = start;
    while i > 0 && segs[i - 1].has(flags::DRIFT_REQ) {
        i -= 1;
    }
    while i < segs.len() && (segs[i].has(flags::DRIFT_REQ) || i < start) {
        let c = segs[i].curve_milli as i32;
        segs[i].curve_milli = (c * 115 / 100).clamp(-170, 170) as i16;
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_required_corners_pass_the_two_bot_test() {
        // Tracks from the third on carry drift-required corners; every one
        // on an accepted track must cost a grip-only perfect driver the
        // timer or at least DRIFT_LOSS seconds.
        let mut checked = 0;
        for (seed, index, branch) in [(1, 2, 0), (2, 3, 1), (3, 4, 1)] {
            let b = build(seed, index, branch, Effort::default());
            assert!(
                matches!(b.rung, Rung::Drawn(_) | Rung::Repaired),
                "{:?}",
                b.failures
            );
            for d in &b.report.drift {
                assert!(d.grip_failed || d.loss >= DRIFT_LOSS, "{d:?}");
                checked += 1;
            }
            let marked = b
                .track
                .segments()
                .windows(2)
                .filter(|w| !w[0].has(flags::DRIFT_REQ) && w[1].has(flags::DRIFT_REQ));
            assert_eq!(marked.count(), b.report.drift.len());
        }
        assert!(checked >= 3);
    }

    #[test]
    fn the_first_track_of_a_run_is_proven() {
        let b = build(1, 0, 0, Effort::default());
        assert!(
            matches!(b.rung, Rung::Drawn(_) | Rung::Repaired),
            "{:?}: {:?}",
            b.rung,
            b.failures
        );
        assert_eq!(b.track.allot_ticks().len(), b.track.checkpoints().len());
        assert!(b.report.fewest_passes >= MARGIN_PASSES);
        assert!(b.report.worst_margin > 0);
    }
}
