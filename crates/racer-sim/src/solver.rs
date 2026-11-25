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
//!    lose at least [`DRIFT_LOSS`] seconds over that corner, or run out of
//!    time while it is inside the corner's window (DESIGN §2.5). A timeout
//!    on an earlier gate is not that corner's failure. The loss is then
//!    measured on a second grip-only drive whose timer cannot cut the
//!    corner off, so a later corner is still timed. A corner that was
//!    driven and is not demanding enough is tightened ("repair κ") and the
//!    track recalibrated, up to [`DRIFT_REPAIRS`] times. A corner the grip
//!    bot never reaches, even then, fails the draft.
//! 5. **Floor.** Against *every* reachable loadout, the human bot, with
//!    noise seeds calibration never saw, must keep the required margin at
//!    every checkpoint in at least 15 of 16 trials ([`MARGIN_PASSES`]).
//!
//! A track that fails climbs down the ladder, as in Salt Line: up to
//! [`ATTEMPTS`] fresh draws, then up to [`REPAIRS`] softenings of the best
//! failed draw's worst section, then the gentle fallback, which is still
//! calibrated and must still pass the floor (its difficulty bands are only
//! reported: a gentle track is allowed to be easy). A fallback that failed
//! the floor too is *unvalidated*. [`build`] still returns that rung, so the
//! batch gate can require none. [`playable`] redraws with fresh attempts the
//! ladder never uses, and if those fail too it refuses the track.
//! [`crate::run::Generate`] returns that refusal: nothing unvalidated is
//! raced. Nothing in that path is given a zero timer: a list of zero
//! allotments would end the race on tick 1.

use std::sync::Arc;

use crate::bot::human::{HumanBot, HumanPlan};
use crate::bot::perfect::{perfect_drive, perfect_run, PerfectRun, LEAD};
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
/// Further fresh draws [`playable`] makes after the ladder, including its
/// fallback, has failed. Numbered from [`ATTEMPTS`], so a track the ladder
/// accepts is the same track as before.
pub const REDRAWS: u32 = 8;
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
    /// Zero when the window was not driven by both bots.
    pub loss: f64,
    /// The grip-only bot reached this corner's approach and ran out of
    /// time before clearing its window. A timeout anywhere else is not
    /// this corner's failure.
    pub grip_failed: bool,
    /// The grip-only bot reached the approach, so the window was actually
    /// driven. False when a timeout earlier on the track is all the trace
    /// shows.
    pub reached: bool,
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
    // No allotments. Zero would be a timer that ends the race on the first
    // tick; empty means the track was never calibrated. [`playable`] refuses
    // to hand either to a race.
    unproven(draft, cost, failures)
}

/// The ladder's failure: an uncalibrated fallback, marked unvalidated.
fn unproven(draft: Draft, cost: u64, failures: Vec<Failure>) -> Built {
    Built {
        track: Arc::new(draft.track()),
        rung: Rung::Unvalidated,
        report: Report {
            cost,
            ..Report::default()
        },
        failures,
    }
}

/// Why [`playable`] has no track to race.
#[derive(Clone, Debug)]
pub struct Unproven {
    /// Why each candidate failed, the ladder's and the redraw's.
    pub failures: Vec<Failure>,
}

/// A track the game may race.
///
/// The ladder first ([`build`]). A track it validates is returned as it
/// stands, so every seed that already proves is unchanged. An unvalidated
/// ladder is redrawn with [`REDRAWS`] fresh attempts numbered from
/// [`ATTEMPTS`], which the ladder never generates. If those fail too, the
/// caller gets [`Err`] and must not start a race: there is no zero-timer
/// stand-in.
pub fn playable(run_seed: u64, index: u32, branch: u8, effort: Effort) -> Result<Built, Unproven> {
    let built = build(run_seed, index, branch, effort);
    if built.rung != Rung::Unvalidated {
        return Ok(built);
    }
    extra_draws(
        run_seed,
        index,
        branch,
        effort,
        ATTEMPTS..ATTEMPTS + REDRAWS,
        built.failures,
    )
}

/// Validate `attempts` as fresh draws. An empty iterator refuses at once.
fn extra_draws(
    run_seed: u64,
    index: u32,
    branch: u8,
    effort: Effort,
    attempts: impl IntoIterator<Item = u32>,
    mut failures: Vec<Failure>,
) -> Result<Built, Unproven> {
    let seed = track_seed(run_seed, index, branch);
    let loadouts = reachable(run_seed, index, branch);
    let mut cost = 0u64;
    for attempt in attempts {
        let draft = generate(run_seed, index, branch, attempt);
        match validate(&draft, index, &loadouts, seed, effort, true) {
            Ok((track, mut report)) => {
                cost += report.cost;
                report.cost = cost;
                return Ok(Built {
                    track,
                    rung: Rung::Drawn(attempt),
                    report,
                    failures,
                });
            }
            Err((f, c)) => {
                cost += c;
                failures.push(f);
            }
        }
    }
    Err(Unproven { failures })
}

#[cfg(test)]
thread_local! {
    static FAIL_FLOOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While this guard is alive, every candidate fails the floor without being
/// driven, the fallback included. Tests use it to force a refusal.
#[cfg(test)]
pub(crate) struct FailFloor;

#[cfg(test)]
impl Drop for FailFloor {
    fn drop(&mut self) {
        FAIL_FLOOR.with(|c| c.set(false));
    }
}

/// Fail the floor of every candidate, including the fallback, until dropped.
#[cfg(test)]
pub(crate) fn fail_the_floor() -> FailFloor {
    FAIL_FLOOR.with(|c| c.set(true));
    FailFloor
}

fn floor_is_forced() -> bool {
    #[cfg(test)]
    {
        FAIL_FLOOR.with(|c| c.get())
    }
    #[cfg(not(test))]
    {
        false
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
    if floor_is_forced() {
        return Err((
            Failure::Margins {
                passes: 0,
                section: 0,
            },
            0,
        ));
    }
    let rules = Rules {
        margin: required_margin(index),
        loadouts,
        seed,
        effort,
        strict,
        perfect_left: PERFECT_LEFT,
    };
    let mut cost = 0;
    let mut draft = draft.clone();
    let mut drift_repairs = 0;
    loop {
        match check(&draft.track(), &rules, drift_repairs, &mut cost) {
            Checked::Passed(track, report) => return Ok((track, report)),
            Checked::Failed(f) => return Err((f, cost)),
            Checked::WeakDrift(start) => {
                if drift_repairs == DRIFT_REPAIRS {
                    return Err((Failure::DriftCorner, cost));
                }
                drift_repairs += 1;
                tighten(&mut draft, start);
            }
        }
    }
}

/// What a track is held to: the margin every gate must keep, the loadouts
/// it can be met with, the seed its noise comes from, how hard to look, and
/// whether the difficulty bands and the drift-corner test apply.
///
/// A generated track's rules come from its key ([`build`]); a layout's are
/// the same for every custom race (`layout::prove`, DESIGN §9.4).
#[derive(Clone, Copy, Debug)]
pub struct Rules<'a> {
    /// Ticks every human verification trial must keep at every gate.
    pub margin: u32,
    /// Every loadout the track can be met with; calibration picks the
    /// weakest.
    pub loadouts: &'a [Loadout],
    /// The seed calibration and verification noise is drawn from.
    pub seed: u64,
    /// Trial counts.
    pub effort: Effort,
    /// Apply the difficulty bands and the drift-corner test.
    pub strict: bool,
    /// The perfect bot's time left at the finish must fall in this band,
    /// ticks, when `strict`: [`PERFECT_LEFT`] for every generated track; a
    /// layout's scales with its length (`layout::perfect_band`).
    pub perfect_left: (u32, u32),
}

/// How [`check`] ended.
#[derive(Clone, Debug)]
pub enum Checked {
    /// Every check passed: the calibrated track and the evidence.
    Passed(Arc<Track>, Report),
    /// A check failed.
    Failed(Failure),
    /// Everything before the drift corners passed, but the drift-required
    /// corner starting at this segment is not demanding enough. The ladder
    /// tightens it and checks again; a layout fails
    /// ([`Failure::DriftCorner`]).
    WeakDrift(usize),
}

/// One pass of every check on an untimed track, in DESIGN §2.4's order:
/// calibrate on the weakest loadout, the ceiling and skill-gap bands, the
/// drift corners, and the floor on every loadout. `drift_repairs` is only
/// recorded in the report. Adds the ticks simulated to `cost`, and a
/// passing report's `cost` is that running total.
pub fn check(untimed: &Track, rules: &Rules, drift_repairs: u32, cost: &mut u64) -> Checked {
    let Rules {
        margin,
        loadouts,
        seed,
        effort,
        strict,
        perfect_left: band,
    } = *rules;
    // 1. Calibrate on the weakest loadout. With one loadout there is nothing
    // to choose between, so the perfect bot is not sent out to choose.
    let untimed = Arc::new(untimed.clone());
    let (calib_on, searched) = match loadouts {
        [only] => (*only, None),
        _ => {
            let w = weakest(&untimed, loadouts);
            *cost += w.cost;
            (w.loadout, Some(w.search))
        }
    };
    let cal = match calibrate(&untimed, calib_on, seed, margin, effort.calibration) {
        Ok(cal) => cal,
        Err(spent) => {
            *cost += spent;
            return Checked::Failed(Failure::Undrivable);
        }
    };
    *cost += cal.cost;
    if cal.crashed * 4 > effort.calibration {
        return Checked::Failed(Failure::CrashProne);
    }
    let mut timed = (*untimed).clone();
    timed.set_allotments(cal.allot.clone());
    let track = Arc::new(timed);

    // 2. Ceiling. Choosing the loadout already searched this car's drift
    // plan on the untimed road. If the timer would have stopped none of
    // that search's drives, searching again would find the same plan, so
    // only the drive is made again, on the timer.
    let params = calib_on.params();
    let best = match searched {
        Some(s) if s.envelope.fits(track.allot_ticks()) => perfect_drive(&track, &params, s.plan),
        _ => perfect_run(&track, &params, true),
    };
    *cost += best.cost;
    if best.race.phase() != Phase::Finished {
        return Checked::Failed(Failure::PerfectFailed);
    }
    let perfect_left = *best.race.margins().last().unwrap();
    let human_ticks = cal.finishes[cal.finishes.len() / 2];
    let gap = (human_ticks as f64 - best.race.tick() as f64) / best.race.tick() as f64;
    if strict {
        if !(band.0..=band.1).contains(&perfect_left) {
            return Checked::Failed(Failure::PerfectBand(perfect_left));
        }
        if !(SKILL_GAP.0..=SKILL_GAP.1).contains(&gap) {
            return Checked::Failed(Failure::SkillGap(gap));
        }
    }

    // 3. Drift-required corners.
    let mut drift = Vec::new();
    if strict {
        let grip = perfect_run(&track, &params, false);
        *cost += grip.cost;
        // The timed grip run stops at the first gate it cannot pay for, which
        // may be before a later drift corner. The loss is a time through the
        // corner, so measure it on a drive the timer does not cut short.
        // Skip that drive when the timed trace already covers every window:
        // the two drives match until the timer expires.
        let loss_run = if drift_windows_open(&track, &grip) {
            let run = perfect_run(&with_free_timer(&track), &params, false);
            *cost += run.cost;
            Some(run)
        } else {
            None
        };
        let loss_trace = loss_run.as_ref().map(|r| &r.trace).unwrap_or(&grip.trace);
        drift = drift_checks(&track, &best, &grip, loss_trace, drift_repairs);
        // The first corner that does not pass. One the grip bot never
        // reached, even with the timer out of the way, cannot be tightened
        // into evidence. One it did reach, but lost too little on, is
        // tightened and checked again.
        if let Some(bad) = drift
            .iter()
            .find(|d| !d.reached || (!d.grip_failed && d.loss < DRIFT_LOSS))
        {
            if !bad.reached {
                return Checked::Failed(Failure::DriftCorner);
            }
            return Checked::WeakDrift(bad.start);
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
            *cost += race.tick() as u64;
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
            return Checked::Failed(Failure::Margins { passes, section });
        }
    }

    Checked::Passed(
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
            cost: *cost,
        },
    )
}

/// A copy of `track` whose timer cannot run out, so a grip-only drive
/// records every corner instead of stopping at the first gate it misses.
///
/// Each section gets an equal share of the `u32` timer. The shares sum to
/// at most `u32::MAX`, so crossing a gate cannot overflow it, and one
/// section is still far longer than a race.
fn with_free_timer(track: &Track) -> Arc<Track> {
    let mut free = track.clone();
    let n = free.checkpoints().len().max(1);
    let allot = u32::MAX / n as u32;
    free.set_allotments(vec![allot; n]);
    Arc::new(free)
}

/// Segment range the drift corner is timed over: [`LEAD`] segments of
/// approach through [`DRIFT_TAIL`] of exit, cut off at the finish line.
/// The runoff past the finish is never driven, so a window that ended
/// there would be missing from both traces and the loss would read as zero.
fn drift_window(c: &crate::bot::Corner, finish: usize) -> (usize, usize) {
    (
        c.start.saturating_sub(LEAD),
        (c.end + DRIFT_TAIL).min(finish),
    )
}

/// Whether the timed grip run stopped before clearing some drift window.
fn drift_windows_open(track: &Track, grip: &PerfectRun) -> bool {
    let finish = track.checkpoints().last().copied().unwrap_or(0) as usize;
    drift_corners(track, grip.race.params()).any(|c| {
        let (a, b) = drift_window(&c, finish);
        grip.trace.ticks_between(a, b).is_none()
    })
}

/// Drift-required corners for `params`, in track order.
fn drift_corners<'a>(
    track: &'a Track,
    params: &crate::car::CarParams,
) -> impl Iterator<Item = crate::bot::Corner> + 'a {
    corners(track, params).into_iter().filter(move |c| {
        track.segments()[c.start..c.end]
            .iter()
            .any(|s| s.has(flags::DRIFT_REQ))
    })
}

/// The two-bot test on every drift-required corner.
///
/// `grip` is the timed grip-only run: "ran out of time" counts only when
/// that run entered this corner's window and did not clear it. A timeout
/// on an earlier gate is not this corner's failure.
///
/// `loss_trace` is a grip-only trace that was not cut short by the timer
/// (the timed trace itself, when it already covers every window). The loss
/// is that trace's time across the window minus the drifting bot's.
fn drift_checks(
    track: &Track,
    best: &PerfectRun,
    grip: &PerfectRun,
    loss_trace: &crate::bot::Trace,
    repairs: u32,
) -> Vec<DriftCheck> {
    let finish = track.checkpoints().last().copied().unwrap_or(0) as usize;
    drift_corners(track, best.race.params())
        .map(|c| {
            let (a, b) = drift_window(&c, finish);
            let in_range = a <= b;
            let entered = |trace: &crate::bot::Trace, seg: usize| {
                trace.seg_tick.get(seg).is_some_and(|&t| t != u32::MAX)
            };
            let grip_failed = in_range
                && entered(&grip.trace, a)
                && !entered(&grip.trace, b)
                && grip.race.phase() == Phase::TimeUp;
            let reached = in_range && entered(loss_trace, a);
            let with = best.trace.ticks_between(a, b);
            let without = loss_trace.ticks_between(a, b);
            let loss = match (with, without) {
                (Some(w), Some(g)) if in_range => (g as f64 - w as f64) * crate::DT,
                _ => 0.0,
            };
            DriftCheck {
                start: c.start,
                loss,
                grip_failed,
                reached,
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
                assert!(
                    d.reached && (d.grip_failed || d.loss >= DRIFT_LOSS),
                    "{d:?}"
                );
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

    /// A grip-only timeout that happens before the corner used to be
    /// reported as that corner failing the timer. The corner was then
    /// accepted without anyone driving it.
    #[test]
    fn a_timeout_before_the_corner_does_not_pass_it() {
        use crate::bot::perfect::PerfectRun;
        use crate::bot::Trace;
        use crate::car::CarParams;
        use crate::input::Input;
        use crate::track::Builder;

        let mut b = Builder::new();
        // Long enough that the approach sits well down the road, then a
        // tight bend, then a finish before the 40-segment exit runoff, so
        // the window has to be clamped to the finish line.
        b.straight(100);
        let corner = b.len();
        b.bend(50, -130, 6);
        b.flag(corner, 50, flags::DRIFT_REQ);
        b.straight(10).checkpoint().straight(80);
        let mut track = b.build();
        let finish = *track.checkpoints().last().unwrap() as usize;
        assert!(
            corner + 50 + DRIFT_TAIL > finish,
            "the fixture's exit runoff must lie past the finish"
        );
        track.set_allotments(vec![1]);
        let track = Arc::new(track);
        let params = CarParams::BASE;

        let mut timed_out = Race::new(track.clone(), params);
        timed_out.step(Input::NEUTRAL);
        assert_eq!(timed_out.phase(), Phase::TimeUp);

        let n = track.len();
        let mut early = vec![u32::MAX; n];
        // Died on the opening straight, short of the approach.
        for (i, slot) in early.iter_mut().enumerate().take(40) {
            *slot = i as u32;
        }
        let grip_early = PerfectRun {
            plan: vec![],
            race: timed_out.clone(),
            trace: Trace {
                seg_tick: early,
                samples: vec![],
            },
            cost: 0,
        };
        let best_trace = {
            let mut seg_tick = vec![u32::MAX; n];
            for (i, slot) in seg_tick.iter_mut().enumerate().take(finish + 1) {
                *slot = i as u32;
            }
            Trace {
                seg_tick,
                samples: vec![],
            }
        };
        // The drifting bot finished. Its race only needs a phase the loss
        // path does not read; the trace is what the window is measured on.
        let finished = Race::new(track.clone(), params);
        // Phase is not read when both traces contain the window. The trace
        // is the drive.
        let best = PerfectRun {
            plan: vec![true],
            race: finished.clone(),
            trace: best_trace.clone(),
            cost: 0,
        };
        let missed = drift_checks(&track, &best, &grip_early, &grip_early.trace, 0);
        assert_eq!(missed.len(), 1, "the flagged bend is one corner");
        assert!(!missed[0].reached, "{:?}", missed[0]);
        assert!(!missed[0].grip_failed, "{:?}", missed[0]);
        assert_eq!(missed[0].loss, 0.0);

        // Same timeout, but the grip bot did enter the approach and died
        // inside the window: that is the corner's failure.
        let mut mid = vec![u32::MAX; n];
        let a = missed[0].start.saturating_sub(LEAD);
        for (i, slot) in mid.iter_mut().enumerate().take(a + 1) {
            *slot = i as u32;
        }
        let grip_mid = PerfectRun {
            plan: vec![],
            race: timed_out,
            trace: Trace {
                seg_tick: mid,
                samples: vec![],
            },
            cost: 0,
        };
        let died_here = drift_checks(&track, &best, &grip_mid, &grip_mid.trace, 0);
        assert!(
            died_here[0].reached && died_here[0].grip_failed,
            "{:?}",
            died_here[0]
        );
        assert!(died_here[0].loss < DRIFT_LOSS);

        // Both bots clear a window that ends at the finish, not in the
        // undriven runoff. After the approach the grip bot is 200 ticks
        // slower: 1.67 s across the corner. Shifting the whole trace would
        // leave the time *between* the two ends unchanged.
        let mut slow = best_trace.seg_tick.clone();
        let approach = missed[0].start.saturating_sub(LEAD);
        for (i, t) in slow.iter_mut().enumerate() {
            if *t != u32::MAX && i > approach {
                *t += 200;
            }
        }
        let grip_slow = PerfectRun {
            plan: vec![],
            race: finished,
            trace: Trace {
                seg_tick: slow,
                samples: vec![],
            },
            cost: 0,
        };
        let measured = drift_checks(&track, &best, &grip_slow, &grip_slow.trace, 0);
        assert!(measured[0].reached, "{:?}", measured[0]);
        assert!(!measured[0].grip_failed, "{:?}", measured[0]);
        let loss = 200.0 * crate::DT;
        assert!(
            (measured[0].loss - loss).abs() < 1e-9,
            "loss {} wanted {loss}",
            measured[0].loss
        );
        assert!(measured[0].loss >= DRIFT_LOSS);

        // The case the timed run gets wrong: it died before the corner, but
        // the free-timer trace shows the corner itself costs 1.67 s. That
        // is a pass, not a credit for the earlier timeout and not a reject.
        let rescued = drift_checks(&track, &best, &grip_early, &grip_slow.trace, 0);
        assert!(rescued[0].reached, "{:?}", rescued[0]);
        assert!(!rescued[0].grip_failed, "{:?}", rescued[0]);
        assert!((rescued[0].loss - loss).abs() < 1e-9, "{:?}", rescued[0]);
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

    #[test]
    fn an_unvalidated_track_is_not_a_one_tick_race() {
        use crate::car::CarParams;
        use crate::input::Input;
        let draft = crate::gen::fallback(1, 0, 0);
        let zeros = {
            let mut t = draft.track();
            t.set_allotments(vec![0; t.checkpoints().len()]);
            Arc::new(t)
        };
        // Stepping this overflows the timer decrement. That is the bug:
        // a zero list is not a calibration.
        assert_eq!(Race::new(zeros, CarParams::BASE).timer(), 0);
        let bare = unproven(draft, 0, Vec::new());
        assert_eq!(bare.rung, Rung::Unvalidated);
        assert!(bare.track.allot_ticks().is_empty());
        let mut race = Race::new(bare.track, CarParams::BASE);
        assert!(race.timer() > 1);
        race.step(Input::NEUTRAL);
        assert_eq!(race.phase(), Phase::Racing);
    }

    #[test]
    fn a_proven_track_is_unchanged_and_a_redraw_with_no_attempts_refuses() {
        let built = build(1, 0, 0, Effort::default());
        let played = playable(1, 0, 0, Effort::default()).expect("seed 1 proves");
        assert_eq!(played.rung, built.rung);
        assert_eq!(
            crate::track::track_hash(&played.track),
            crate::track::track_hash(&built.track)
        );
        // The redraw machinery, pointed at a draw the ladder already accepts.
        let again = extra_draws(1, 0, 0, Effort::default(), 0..1, Vec::new()).unwrap();
        assert!(matches!(again.rung, Rung::Drawn(0)));
        // Nothing left to draw is a refusal, not a zero-timer track.
        assert!(extra_draws(1, 0, 0, Effort::default(), 0..0, Vec::new()).is_err());
    }
}
