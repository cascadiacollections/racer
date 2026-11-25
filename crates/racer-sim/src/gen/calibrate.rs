//! Timer calibration: the checkpoint allotments, from how the human bot
//! drives the track. DESIGN §2.4 step 2.
//!
//! The arcade timer is the game's only fail state, so its allotments are
//! the difficulty knob that matters most, and setting them by hand per
//! generated track is impossible. Instead the human bot drives the track
//! untimed, [`CALIBRATION_TRIALS`] times with different noise, and each
//! gate's allotment is set so that a typical human (the 75th percentile of
//! the cumulative time to that gate) reaches it with the required margin
//! plus [`SLACK`] to spare. Allotments are rounded *up* to a quarter second,
//! a number a player can read off the timer.
//!
//! Calibration runs on the *weakest* loadout the track can be met with (the
//! one the perfect bot is slowest on), so every reachable loadout has at
//! least the time the weakest one needs. Whether that holds for every
//! loadout, and for drivers it was not calibrated on, is the verifier's job
//! (`solver.rs`), with fresh noise seeds.

use std::sync::Arc;

use crate::bot::drive;
use crate::bot::human::{HumanBot, HumanPlan};
use crate::bot::perfect::{perfect_drive, perfect_search, Search};
use crate::race::{Phase, Race};
use crate::rng::substream;
use crate::track::Track;
use crate::upgrade::Loadout;

/// Human trials per calibration.
pub const CALIBRATION_TRIALS: u64 = 16;

/// Seconds added to every gate beyond the required margin.
pub const SLACK: f64 = 0.5;

/// Allotments are rounded up to this many ticks: a quarter second.
pub const QUANTUM: u32 = 30;

/// Tag for calibration noise seeds: "CALI" in ASCII.
pub const CALIBRATION_TAG: u64 = 0x4341_4C49;

/// The loadout the perfect bot is slowest on ([`weakest`]).
#[derive(Clone, Debug)]
pub struct Weakest {
    /// The loadout.
    pub loadout: Loadout,
    /// The ticks the perfect bot took on it.
    pub ticks: u32,
    /// Its drift search, which the solver reuses once the track has a timer
    /// (`bot::perfect::Envelope`).
    pub search: Search,
    /// Ticks simulated over every loadout.
    pub cost: u64,
}

/// The loadout among `loadouts` on which the perfect bot is slowest. Ties
/// go to the first, so the choice is deterministic.
///
/// # Panics
///
/// If `loadouts` is empty.
pub fn weakest(track: &Arc<Track>, loadouts: &[Loadout]) -> Weakest {
    let mut worst: Option<Weakest> = None;
    let mut cost = 0u64;
    for &l in loadouts {
        let params = l.params();
        let search = perfect_search(track, &params);
        let run = perfect_drive(track, &params, search.plan.clone());
        let ticks = run.race.tick();
        cost += search.cost + run.cost;
        if worst.as_ref().is_none_or(|w| ticks > w.ticks) {
            worst = Some(Weakest {
                loadout: l,
                ticks,
                search,
                cost: 0,
            });
        }
    }
    let mut worst = worst.expect("a track is met with at least one loadout");
    worst.cost = cost;
    worst
}

/// The result of a calibration.
#[derive(Clone, Debug)]
pub struct Calibrated {
    /// Allotment per section, ticks.
    pub allot: Vec<u32>,
    /// Each calibration trial's finishing tick, sorted.
    pub finishes: Vec<u32>,
    /// Trials that hit something.
    pub crashed: u64,
    /// Ticks simulated.
    pub cost: u64,
}

/// Calibrate `track` (untimed) for `loadout` with `margin` ticks of required
/// margin. `Err` if any trial fails to finish: the human cannot drive it.
/// The error is the ticks simulated finding that out.
pub fn calibrate(
    track: &Arc<Track>,
    loadout: Loadout,
    seed: u64,
    margin: u32,
    trials: u64,
) -> Result<Calibrated, u64> {
    let params = loadout.params();
    let plan = HumanPlan::new(track, &params);
    let gates = track.checkpoints().len();
    let mut splits: Vec<Vec<u32>> = vec![Vec::new(); gates];
    let mut finishes = Vec::new();
    let mut crashed = 0;
    let mut cost = 0;
    for trial in 0..trials {
        let mut race = Race::new(track.clone(), params);
        drive(
            &mut race,
            &mut HumanBot::new(plan.clone(), substream(seed, CALIBRATION_TAG + trial)),
        );
        cost += race.tick() as u64;
        if race.phase() != Phase::Finished {
            return Err(cost);
        }
        for (k, &s) in race.splits().iter().enumerate() {
            splits[k].push(s);
        }
        finishes.push(race.tick());
        crashed += u64::from(race.hits() > 0);
    }
    finishes.sort_unstable();
    let slack = crate::ticks(SLACK);
    let mut allot = Vec::with_capacity(gates);
    let mut before = 0;
    for s in &mut splits {
        s.sort_unstable();
        let p75 = s[(s.len() * 3).div_ceil(4) - 1];
        let at = (p75 + margin + slack).div_ceil(QUANTUM) * QUANTUM;
        let at = at.max(before + QUANTUM);
        allot.push(at - before);
        before = at;
    }
    Ok(Calibrated {
        allot,
        finishes,
        crashed,
        cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::perfect::perfect_run;
    use crate::car::CarParams;
    use crate::track::fixtures;

    #[test]
    fn allotments_are_quarter_seconds_that_cover_the_typical_human() {
        let t = Arc::new(fixtures::sweep_and_hairpin());
        let c = calibrate(&t, Loadout::BASE, 1, 240, 8).expect("drivable");
        assert_eq!(c.allot.len(), 2);
        assert!(c.allot.iter().all(|a| a % QUANTUM == 0));
        let total: u32 = c.allot.iter().sum();
        let median = c.finishes[c.finishes.len() / 2];
        assert!(total >= median + 240, "{total} vs {median}");
    }

    #[test]
    fn the_weakest_loadout_is_the_slowest_one() {
        let t = Arc::new(fixtures::sweep_and_hairpin());
        let fast = Loadout::BASE.with(crate::upgrade::Upgrade::Engine);
        let w = weakest(&t, &[fast, Loadout::BASE]);
        assert_eq!(w.loadout, Loadout::BASE);
        assert_eq!(w.ticks, perfect_run(&t, &CarParams::BASE, true).race.tick());
    }
}
