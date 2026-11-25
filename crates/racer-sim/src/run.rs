//! A run: five tracks, a fork after each, upgrades that stack. DESIGN §1.
//!
//! The run is a small state machine around the current [`Race`]:
//!
//! ```text
//! Racing --finish--> Forking --fork(branch)--> Racing ... --finish track 5--> Won
//!    \--timer out--> Lost
//! ```
//!
//! At each fork the player picks one of two tracks, each carrying a
//! different upgrade (`upgrade.rs`); the pick takes that upgrade for the
//! track and the rest of the run. Tracks depend only on `(run_seed, index,
//! branch)`, so both sides of a fork can be built while the player is still
//! driving the track before it.
//!
//! Track building runs the solver's bots and takes a few hundred
//! milliseconds a track, so the run never builds one itself: it asks a
//! [`Tracks`] source. [`Generate`] builds every time; [`TrackCache`]
//! remembers what it built. Because building is a pure function of the key,
//! the two are interchangeable: a cache can only make things faster, never
//! different.
//!
//! The run's log is the races' input logs end to end, with a fork marker
//! between them. That, with the seed, is the whole persisted run (`replay.rs`).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::gen::difficulty::TRACKS;
use crate::hash::Hasher;
use crate::input::Input;
use crate::race::{Phase, Race};
use crate::replay::{hash_events, Event};
use crate::solver::Effort;
use crate::track::Track;
use crate::upgrade::{fork_upgrades, upgrade_of, Loadout, Upgrade, UPGRADES};

/// Where the run is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunPhase {
    /// Driving the current track.
    Racing,
    /// Finished a track; choosing the next.
    Forking,
    /// Finished the last track.
    Won,
    /// The timer ran out.
    Lost,
}

/// Somewhere to get proven tracks from.
pub trait Tracks {
    /// The track `(index, branch)` of run `run_seed`, calibrated and proven.
    ///
    /// [`Err`] means the ladder and a redraw both failed. The caller must
    /// not race a stand-in: there is no track.
    fn track(
        &mut self,
        run_seed: u64,
        index: u32,
        branch: u8,
    ) -> Result<Arc<Track>, crate::solver::Unproven>;
}

/// Builds every track it is asked for, with the solver's default effort.
#[derive(Clone, Copy, Debug, Default)]
pub struct Generate;

impl Generate {
    /// The proven track, or why the ladder and a redraw both failed.
    /// [`Err`] is a refusal: there is no track to race, zero-timer or not.
    pub fn proven(
        run_seed: u64,
        index: u32,
        branch: u8,
    ) -> Result<Arc<Track>, crate::solver::Unproven> {
        crate::solver::playable(run_seed, index, branch, Effort::default()).map(|b| b.track)
    }
}

impl Tracks for Generate {
    fn track(
        &mut self,
        run_seed: u64,
        index: u32,
        branch: u8,
    ) -> Result<Arc<Track>, crate::solver::Unproven> {
        Self::proven(run_seed, index, branch)
    }
}

/// Remembers every track it builds. Ordered, so iterating it is
/// deterministic.
#[derive(Clone, Debug, Default)]
pub struct TrackCache {
    built: BTreeMap<(u64, u32, u8), Arc<Track>>,
}

impl TrackCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Put a track built elsewhere (another thread, say) in the cache.
    pub fn insert(&mut self, run_seed: u64, index: u32, branch: u8, track: Arc<Track>) {
        self.built.insert((run_seed, index, branch), track);
    }
}

impl Tracks for TrackCache {
    fn track(
        &mut self,
        run_seed: u64,
        index: u32,
        branch: u8,
    ) -> Result<Arc<Track>, crate::solver::Unproven> {
        if let Some(t) = self.built.get(&(run_seed, index, branch)) {
            return Ok(t.clone());
        }
        let t = Generate.track(run_seed, index, branch)?;
        self.built.insert((run_seed, index, branch), t.clone());
        Ok(t)
    }
}

/// Tag for the daily run's seed: "RACERDAY" in ASCII.
const DAILY_TAG: u64 = 0x5241_4345_5244_4159;

/// The daily run's seed for day `day`, counted in whole UTC days since the
/// Unix epoch (Salt Line's rule, DESIGN §1.4): everyone who plays that day,
/// anywhere, gets this run. A pure function of the day; the clock is read
/// by the engine adapter, never here. Never 0.
pub fn daily_seed(day: u64) -> u64 {
    crate::rng::mix64(DAILY_TAG ^ day).max(1)
}

/// How one track of the run went.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrackResult {
    /// Track index.
    pub index: u32,
    /// The branch taken to it.
    pub branch: u8,
    /// Finishing tick, or the tick the timer ran out.
    pub ticks: u32,
    /// Smallest margin kept, ticks (0 if the timer ran out).
    pub margin: u32,
}

/// A run in progress.
#[derive(Clone, Debug)]
pub struct RunState {
    run_seed: u64,
    index: u32,
    branch: u8,
    loadout: Loadout,
    /// Tracks in this run: [`TRACKS`], or 1 for a custom race
    /// ([`RunState::single`]). Not hashed: it follows from how the run
    /// began.
    tracks: u32,
    phase: RunPhase,
    race: Race,
    results: Vec<TrackResult>,
    /// The log of every race before the current one, with fork markers.
    past: Vec<Event>,
}

impl RunState {
    /// A run from `run_seed`, on the start line of its first track.
    ///
    /// # Errors
    ///
    /// The first track did not pass the prover. No run is started.
    pub fn new(run_seed: u64, tracks: &mut impl Tracks) -> Result<Self, crate::solver::Unproven> {
        let race = Race::new(tracks.track(run_seed, 0, 0)?, Loadout::BASE.params());
        Ok(RunState {
            run_seed,
            index: 0,
            branch: 0,
            loadout: Loadout::BASE,
            tracks: TRACKS,
            phase: RunPhase::Racing,
            race,
            results: Vec::new(),
            past: Vec::new(),
        })
    }

    /// A run of one track, `track`, driven with `loadout`: a custom race on
    /// a layout (DESIGN §9). Finishing it wins the run; there is no fork.
    /// `key` stands in for the run seed (a layout's digest), so a ghost or
    /// a state hash can tell custom races apart.
    pub fn single(key: u64, track: Arc<Track>, loadout: Loadout) -> Self {
        RunState {
            run_seed: key,
            index: 0,
            branch: 0,
            loadout,
            tracks: 1,
            phase: RunPhase::Racing,
            race: Race::new(track, loadout.params()),
            results: Vec::new(),
            past: Vec::new(),
        }
    }

    /// Advance the race one tick. Does nothing unless racing.
    pub fn step(&mut self, input: Input) {
        if self.phase != RunPhase::Racing {
            return;
        }
        self.race.step(input);
        match self.race.phase() {
            Phase::Racing => {}
            end => {
                self.results.push(TrackResult {
                    index: self.index,
                    branch: self.branch,
                    ticks: self.race.tick(),
                    margin: self.race.min_margin(),
                });
                self.past.extend_from_slice(self.race.log());
                self.phase = if end == Phase::TimeUp {
                    RunPhase::Lost
                } else if self.index + 1 >= self.tracks {
                    RunPhase::Won
                } else {
                    RunPhase::Forking
                };
            }
        }
    }

    /// The upgrades on the two roads ahead, while forking.
    pub fn offers(&self) -> Option<[Upgrade; 2]> {
        (self.phase == RunPhase::Forking).then(|| fork_upgrades(self.run_seed, self.index + 1))
    }

    /// Take fork `branch` (0 or 1) onto the next track. `Ok(false)`, and
    /// nothing changes, unless the run is forking and the branch exists.
    ///
    /// # Errors
    ///
    /// The next track did not pass the prover. The run stays forking.
    pub fn fork(
        &mut self,
        branch: u8,
        tracks: &mut impl Tracks,
    ) -> Result<bool, crate::solver::Unproven> {
        if self.phase != RunPhase::Forking || branch > 1 || self.index + 1 >= self.tracks {
            return Ok(false);
        }
        let track = tracks.track(self.run_seed, self.index + 1, branch)?;
        self.index += 1;
        self.branch = branch;
        if let Some(u) = upgrade_of(self.run_seed, self.index, branch) {
            self.loadout = self.loadout.with(u);
        }
        self.past.push(Event::Fork { branch });
        self.race = Race::new(track, self.loadout.params());
        self.phase = RunPhase::Racing;
        Ok(true)
    }

    /// The run's seed.
    #[inline]
    pub fn run_seed(&self) -> u64 {
        self.run_seed
    }

    /// The current track's index.
    #[inline]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The branch taken to the current track.
    #[inline]
    pub fn branch(&self) -> u8 {
        self.branch
    }

    /// Tracks in this run: [`TRACKS`], or 1 for a custom race.
    #[inline]
    pub fn tracks(&self) -> u32 {
        self.tracks
    }

    /// Upgrades held.
    #[inline]
    pub fn loadout(&self) -> Loadout {
        self.loadout
    }

    /// Where the run is.
    #[inline]
    pub fn phase(&self) -> RunPhase {
        self.phase
    }

    /// The current (or last) race.
    #[inline]
    pub fn race(&self) -> &Race {
        &self.race
    }

    /// How each track so far went.
    #[inline]
    pub fn results(&self) -> &[TrackResult] {
        &self.results
    }

    /// The whole log: every race's inputs, with fork markers between.
    pub fn log(&self) -> Vec<Event> {
        let mut log = self.past.clone();
        if self.phase == RunPhase::Racing {
            log.extend_from_slice(self.race.log());
        }
        log
    }
}

impl crate::replay::Steppable for RunState {
    fn step(&mut self, input: Input) {
        RunState::step(self, input);
    }
    fn racing(&self) -> bool {
        self.phase == RunPhase::Racing
    }
}

/// Tag for the autoplayer's fork picks: "PICK".
const PICKS: u64 = 0x5049_434B;

/// A deterministic stand-in for a player: the human-like bot drives every
/// track (its noise from `noise`), and forks are picked at random from the
/// same seed. Not a good player on purpose: it is the human bot, so it
/// sometimes loses. `bin/determinism` and the golden runs play with it.
pub fn autoplay(
    run_seed: u64,
    noise: u64,
    tracks: &mut impl Tracks,
) -> Result<RunState, crate::solver::Unproven> {
    use crate::bot::human::{HumanBot, HumanPlan};
    use crate::bot::Driver;
    use crate::rng::{substream, Pcg32};
    let mut picks = Pcg32::new(substream(noise, PICKS));
    let mut run = RunState::new(run_seed, tracks)?;
    loop {
        match run.phase() {
            RunPhase::Racing => {
                let plan = HumanPlan::new(run.race().track(), run.race().params());
                let mut bot = HumanBot::new(plan, substream(noise, run.index() as u64));
                while run.phase() == RunPhase::Racing {
                    let input = bot.input(run.race());
                    run.step(input);
                }
            }
            RunPhase::Forking => {
                if !run.fork(picks.range_u32(0, 1) as u8, tracks)? {
                    return Ok(run);
                }
            }
            RunPhase::Won | RunPhase::Lost => return Ok(run),
        }
    }
}

/// Hash of the run state, every float by bit pattern: the run's identity,
/// the loadout, the phase, every result, the current race and the whole
/// log. Compared at race boundaries — after a race ends, after a fork — a
/// live run and its replay must agree exactly (DESIGN §8).
///
/// Changing what is hashed, or its order, moves every golden-run hash and
/// the epoch digest, so it needs the `SIM_EPOCH` procedure (DESIGN §8.3).
pub fn state_hash(s: &RunState) -> u64 {
    let mut h = Hasher::new();
    h.write_u64(s.run_seed);
    h.write_u32(s.index);
    h.write_u8(s.branch);
    for u in Upgrade::ALL {
        h.write_u8(s.loadout.stacks(u));
    }
    h.write_usize(UPGRADES);
    h.write_u8(s.phase as u8);
    h.write_usize(s.results.len());
    for r in &s.results {
        h.write_u32(r.index);
        h.write_u8(r.branch);
        h.write_u32(r.ticks);
        h.write_u32(r.margin);
    }
    crate::track::hash_track(&mut h, s.race.track());
    s.race.hash_into(&mut h);
    hash_events(&mut h, &s.log());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daily_seed_is_fixed_per_day_and_new_each_day() {
        // 2026-10-18 is UTC day 20744. Pinned: a change would hand every
        // player a different daily run from the one their friends had.
        assert_eq!(daily_seed(20744), daily_seed(20744));
        assert_eq!(daily_seed(20744), 0x77d6_3255_4758_2204);
        let ten_years: std::collections::BTreeSet<u64> = (20_000..23_650).map(daily_seed).collect();
        assert_eq!(ten_years.len(), 3650, "no two days share a run");
        assert!(!ten_years.contains(&0));
    }

    /// Drive straight down the middle, braking at random, until the race
    /// ends: bad driving, enough to finish a gentle track or run out of time
    /// on a hard one.
    fn drive(run: &mut RunState) {
        let mut n = 0u32;
        while run.phase() == RunPhase::Racing {
            n += 1;
            let x = run.race().car().x;
            let steer = if x > 0.5 {
                -1
            } else if x < -0.5 {
                1
            } else {
                0
            };
            run.step(Input::new(steer, n % 97 < 30, false));
        }
    }

    #[test]
    fn a_run_forks_and_stacks_upgrades() {
        let mut tracks = TrackCache::new();
        let mut run = RunState::new(5, &mut tracks).expect("seed 5 proves");
        assert!(!run.fork(0, &mut tracks).unwrap(), "no fork while racing");
        assert_eq!(run.offers(), None);
        // Teleport-free: just drive the first track to its end.
        drive(&mut run);
        if run.phase() == RunPhase::Lost {
            return; // the crude driver lost; nothing more to check here
        }
        let offers = run.offers().expect("forking");
        assert!(!run.fork(2, &mut tracks).unwrap());
        assert!(run.fork(1, &mut tracks).unwrap());
        assert_eq!((run.index(), run.branch()), (1, 1));
        assert_eq!(run.loadout(), Loadout::BASE.with(offers[1]));
        assert_eq!(run.race().params(), &run.loadout().params());
        assert!(matches!(run.log().last(), Some(Event::Fork { branch: 1 })));
    }

    #[test]
    fn a_run_that_runs_out_of_time_is_lost() {
        let mut tracks = TrackCache::new();
        let mut run = RunState::new(5, &mut tracks).expect("seed 5 proves");
        // Brake the whole way: the timer must run out.
        while run.phase() == RunPhase::Racing {
            run.step(Input::new(0, true, false));
        }
        assert_eq!(run.phase(), RunPhase::Lost);
        assert_eq!(run.results().len(), 1);
        assert_eq!(run.results()[0].margin, 0);
        // A lost run ignores input and forks.
        let h = state_hash(&run);
        run.step(Input::NEUTRAL);
        assert!(!run.fork(0, &mut tracks).unwrap());
        assert_eq!(state_hash(&run), h);
    }

    #[test]
    fn generate_refuses_when_the_fallback_fails_the_floor() {
        let _hook = crate::solver::fail_the_floor();
        let built = crate::solver::build(1, 0, 0, Effort::default());
        assert_eq!(built.rung, crate::solver::Rung::Unvalidated);
        assert!(
            built.track.allot_ticks().is_empty(),
            "an unvalidated fallback is not calibrated"
        );
        let err = Generate::proven(1, 0, 0).expect_err("the redraws fail too");
        assert!(!err.failures.is_empty());
        let mut gen = Generate;
        assert!(gen.track(1, 0, 0).is_err());
        assert!(RunState::new(1, &mut gen).is_err(), "no race is started");
        // The refusal is not a zero-timer track. There is nothing to step.
        assert!(Generate.track(1, 0, 0).is_err());
    }
}
