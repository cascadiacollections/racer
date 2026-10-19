//! The ghost of the player's best run on a track: the store of best races
//! and the second race that drives one back beside the player.
//!
//! A track is its key `(run_seed, index, branch)`: the same key always
//! builds the same track (`run.rs`). When the player finishes a track, the
//! race's own input log, the loadout it was driven with and the finishing
//! tick are a complete record of that drive, because the sim is
//! deterministic (`replay.rs`). [`GhostStore`] keeps the fastest such record
//! per key; [`GhostRun`] replays one tick at a time, in lockstep with the
//! player's race, so the game can draw it.
//!
//! A ghost is display only. It is its own [`Race`] on the shared track: it
//! never touches the player's race, so the player's run, its log, its
//! replay and every hash are the same with or without one.
//!
//! # File format
//!
//! `ghosts.txt`, plain text, one ghost per line after a header:
//!
//! ```text
//! racer-ghosts 1
//! g <epoch> <run seed, 0x + 16 hex> <index> <branch> <stacks> <ticks> <log>
//! ```
//!
//! `<stacks>` is the loadout as one digit per upgrade in
//! [`Upgrade::ALL`] order (`01020`: one turbo, two brakes); `<ticks>` the
//! finishing tick; `<log>` the race's input changes in
//! [`encode_log`]'s form (`v1 40:R2D 3:CB`), which never contains a fork.
//! Lines from another [`crate::SIM_EPOCH`] are dropped (their logs would
//! drive differently), and so is any line that does not parse: a damaged
//! file loses the damaged ghosts, never the game. A ghost that parses but
//! does not replay to its recorded finishing tick is rejected when it is
//! raced ([`GhostRun::new`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::car::Car;
use crate::input::Input;
use crate::race::{Phase, Race};
use crate::replay::{decode_log, encode_log, replay_race, Event};
use crate::track::Track;
use crate::upgrade::{Loadout, Upgrade, UPGRADES};

/// Version of the ghost file, the header's second token.
pub const GHOST_FILE: u32 = 1;

/// A track's identity: `(run_seed, index, branch)`.
pub type TrackKey = (u64, u32, u8);

/// One finished race, enough to drive it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ghost {
    /// The loadout it was driven with (its [`Loadout::params`] is the car).
    pub loadout: Loadout,
    /// The finishing tick.
    pub ticks: u32,
    /// The race's input changes: [`Race::log`], no fork markers.
    pub log: Vec<Event>,
}

impl Ghost {
    /// The record of `race`, if it finished.
    pub fn of(race: &Race, loadout: Loadout) -> Option<Ghost> {
        (race.phase() == Phase::Finished).then(|| Ghost {
            loadout,
            ticks: race.tick(),
            log: race.log().to_vec(),
        })
    }
}

/// The fastest finish per track, for one sim epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GhostStore {
    best: BTreeMap<TrackKey, Ghost>,
}

impl GhostStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The best on track `key`.
    pub fn get(&self, key: TrackKey) -> Option<&Ghost> {
        self.best.get(&key)
    }

    /// How many tracks have a ghost.
    pub fn len(&self) -> usize {
        self.best.len()
    }

    /// Whether no track has a ghost.
    pub fn is_empty(&self) -> bool {
        self.best.is_empty()
    }

    /// Keep `g` for `key` if it is the first finish there or faster than the
    /// one kept. Returns whether it was kept. A tie keeps the older one.
    pub fn offer(&mut self, key: TrackKey, g: Ghost) -> bool {
        match self.best.get(&key) {
            Some(old) if old.ticks <= g.ticks => false,
            _ => {
                self.best.insert(key, g);
                true
            }
        }
    }

    /// The text form (module docs), every ghost under `epoch`. Ordered by
    /// key, so the same store always writes the same bytes.
    pub fn encode(&self, epoch: u32) -> String {
        let mut out = format!("racer-ghosts {GHOST_FILE}\n");
        for (&(seed, index, branch), g) in &self.best {
            let stacks: String = Upgrade::ALL
                .iter()
                .map(|&u| char::from(b'0' + g.loadout.stacks(u).min(9)))
                .collect();
            out.push_str(&format!(
                "g {epoch} {seed:#018x} {index} {branch} {stacks} {} {}\n",
                g.ticks,
                encode_log(&g.log)
            ));
        }
        out
    }

    /// Read [`GhostStore::encode`]'s text, keeping only ghosts recorded
    /// under `epoch`. Lenient: a wrong header gives an empty store, a bad
    /// line is skipped, and of two lines for one key the faster is kept.
    pub fn decode(text: &str, epoch: u32) -> GhostStore {
        let mut store = GhostStore::new();
        let mut lines = text.lines();
        if lines.next().map(str::trim_end) != Some(&*format!("racer-ghosts {GHOST_FILE}")) {
            return store;
        }
        for line in lines {
            if let Some((e, key, g)) = decode_line(line) {
                if e == epoch {
                    store.offer(key, g);
                }
            }
        }
        store
    }
}

/// One `g` line: its epoch, key and ghost.
fn decode_line(line: &str) -> Option<(u32, TrackKey, Ghost)> {
    let mut t = line.split_whitespace();
    if t.next()? != "g" {
        return None;
    }
    let epoch = t.next()?.parse().ok()?;
    let seed = u64::from_str_radix(t.next()?.strip_prefix("0x")?, 16).ok()?;
    let index = t.next()?.parse().ok()?;
    let branch: u8 = t.next()?.parse().ok()?;
    let stacks = t.next()?;
    if branch > 1 || stacks.len() != UPGRADES {
        return None;
    }
    let mut loadout = Loadout::BASE;
    for (u, d) in Upgrade::ALL.iter().zip(stacks.bytes()) {
        if !d.is_ascii_digit() {
            return None;
        }
        for _ in 0..d - b'0' {
            loadout = loadout.with(*u);
        }
    }
    let ticks = t.next()?.parse().ok()?;
    let rest: Vec<&str> = t.collect();
    let log = decode_log(&rest.join(" ")).ok()?;
    if log.iter().any(|e| matches!(e, Event::Fork { .. })) {
        return None;
    }
    Some((
        epoch,
        (seed, index, branch),
        Ghost {
            loadout,
            ticks,
            log,
        },
    ))
}

/// A ghost being driven, one tick per player tick.
///
/// Stepping applies the log exactly as `replay::feed` does: an event's
/// input takes effect after `delta` ticks of the input before it, and the
/// last input holds to the end. So the ghost's race is, tick for tick, the
/// race that was recorded.
#[derive(Clone, Debug)]
pub struct GhostRun {
    race: Race,
    log: Vec<Event>,
    /// The next event to apply.
    next: usize,
    /// Ticks to run before `log[next]` applies.
    wait: u32,
    input: Input,
    /// The tick each gate was crossed on, from a full replay up front: the
    /// player can reach a gate before the ghost does.
    splits: Vec<u32>,
    ticks: u32,
}

impl GhostRun {
    /// Ready `g` to race on `track`. Replays it once in full first and
    /// returns `None` unless it finishes on exactly its recorded tick: a
    /// damaged or foreign record is never shown.
    pub fn new(track: Arc<Track>, g: &Ghost) -> Option<GhostRun> {
        let params = g.loadout.params();
        let (done, used) = replay_race(track.clone(), params, &g.log).ok()?;
        if used != g.log.len() || done.phase() != Phase::Finished || done.tick() != g.ticks {
            return None;
        }
        let wait = delta(&g.log, 0);
        Some(GhostRun {
            race: Race::new(track, params),
            log: g.log.clone(),
            next: 0,
            wait,
            input: Input::NEUTRAL,
            splits: done.splits().to_vec(),
            ticks: g.ticks,
        })
    }

    /// One tick. Does nothing once the ghost has finished.
    pub fn step(&mut self) {
        if !self.racing() {
            return;
        }
        while self.next < self.log.len() && self.wait == 0 {
            if let Event::Input { input, .. } = self.log[self.next] {
                self.input = input;
            }
            self.next += 1;
            self.wait = delta(&self.log, self.next);
        }
        self.race.step(self.input);
        self.wait = self.wait.saturating_sub(1);
    }

    /// Whether the ghost is still driving (and so drawn).
    pub fn racing(&self) -> bool {
        self.race.phase() == Phase::Racing
    }

    /// The ghost's car.
    pub fn car(&self) -> &Car {
        self.race.car()
    }

    /// The input it holds this tick, for its lean.
    pub fn input(&self) -> Input {
        self.race.input()
    }

    /// The ghost's race.
    pub fn race(&self) -> &Race {
        &self.race
    }

    /// The tick the ghost crosses gate `k` on.
    pub fn split(&self, k: usize) -> Option<u32> {
        self.splits.get(k).copied()
    }

    /// The finishing tick it was recorded with.
    pub fn best_ticks(&self) -> u32 {
        self.ticks
    }
}

/// `log[i]`'s delta, or "never" past the end.
fn delta(log: &[Event], i: usize) -> u32 {
    match log.get(i) {
        Some(Event::Input { delta, .. }) => *delta,
        _ => u32::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hasher;
    use crate::track::fixtures;

    /// Weave and brake across the sweep and hairpin until it is finished.
    fn drive(track: &Arc<Track>, loadout: Loadout, salt: u32) -> Race {
        let mut race = Race::new(track.clone(), loadout.params());
        let mut n = 0u32;
        while race.phase() == Phase::Racing {
            n += 1;
            let x = race.car().x;
            let aim = if x > 0.5 {
                -1
            } else if x < -0.5 {
                1
            } else {
                ((n + salt) / 41 % 3) as i8 - 1
            };
            race.step(Input::new(aim, (n + salt) % 211 < 9, n % 400 < 30));
        }
        race
    }

    fn sample() -> (TrackKey, Ghost, Arc<Track>) {
        let track = Arc::new(fixtures::sweep_and_hairpin());
        let loadout = Loadout::BASE.with(Upgrade::Turbo).with(Upgrade::Brakes);
        let race = drive(&track, loadout, 0);
        let g = Ghost::of(&race, loadout).expect("the fixture is finished");
        ((0xDEAD_BEEF, 2, 1), g, track)
    }

    #[test]
    fn the_store_round_trips_through_its_text() {
        let (key, g, _) = sample();
        let mut s = GhostStore::new();
        assert!(s.offer(key, g.clone()));
        assert!(s.offer(
            (7, 0, 0),
            Ghost {
                loadout: Loadout::BASE,
                ticks: 900,
                log: vec![],
            }
        ));
        let text = s.encode(3);
        assert!(text.starts_with("racer-ghosts 1\ng 3 0x0000000000000007 0 0 00000 900 v1\n"));
        assert!(text.contains("g 3 0x00000000deadbeef 2 1 01010 "), "{text}");
        assert_eq!(GhostStore::decode(&text, 3), s);
        assert_eq!(GhostStore::decode(&text, 3).encode(3), text);
    }

    #[test]
    fn other_epochs_bad_lines_and_bad_headers_are_ignored() {
        let (key, g, _) = sample();
        let mut s = GhostStore::new();
        s.offer(key, g);
        let good = s.encode(1);
        assert!(GhostStore::decode(&good, 2).is_empty(), "another epoch");
        assert!(
            GhostStore::decode(&good.replacen("racer-ghosts 1", "racer-ghosts 9", 1), 1).is_empty()
        );
        assert!(GhostStore::decode("", 1).is_empty());
        let junk = [
            "",
            "g",
            "garbage line",
            "g 1 0x1 0 0 00000 10",           // no log
            "g 1 12 0 0 00000 10 v1",         // seed not hex
            "g 1 0x1 0 2 00000 10 v1",        // no third road
            "g 1 0x1 0 0 0000 10 v1",         // short loadout
            "g 1 0x1 0 0 0a000 10 v1",        // not a digit
            "g 1 0x1 0 0 00000 10 v1 3:C >1", // a fork in a race log
            "g 1 0x1 0 0 00000 10 v1 05:C",   // non-canonical log
            "g 1 0x1 0 0 00000 -4 v1",        // negative ticks
        ];
        let mut text = good.clone();
        for j in junk {
            text.push_str(j);
            text.push('\n');
        }
        // A truncated last line too.
        text.push_str(&good.lines().nth(1).unwrap()[..20]);
        assert_eq!(GhostStore::decode(&text, 1), s);
    }

    #[test]
    fn only_a_faster_finish_replaces_the_best() {
        let (key, g, _) = sample();
        let mut s = GhostStore::new();
        assert!(s.offer(key, g.clone()));
        let slower = Ghost {
            ticks: g.ticks + 1,
            ..g.clone()
        };
        assert!(!s.offer(key, slower));
        assert!(!s.offer(key, g.clone()), "a tie keeps the old one");
        let faster = Ghost {
            ticks: g.ticks - 1,
            ..g.clone()
        };
        assert!(s.offer(key, faster));
        assert_eq!(s.get(key).unwrap().ticks, g.ticks - 1);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn a_ghost_drives_exactly_the_stored_race() {
        let (_, g, track) = sample();
        let (want, _) = replay_race(track.clone(), g.loadout.params(), &g.log).unwrap();
        let mut ghost = GhostRun::new(track, &g).expect("it replays");
        let mut ticks = 0;
        while ghost.racing() {
            ghost.step();
            ticks += 1;
        }
        assert_eq!(ticks, g.ticks);
        assert_eq!(ghost.race().tick(), g.ticks, "finishes on its stored time");
        assert_eq!(ghost.race().log(), &g.log[..], "and logs what was stored");
        let (mut a, mut b) = (Hasher::new(), Hasher::new());
        want.hash_into(&mut a);
        ghost.race().hash_into(&mut b);
        assert_eq!(a.finish(), b.finish());
        assert_eq!(ghost.split(1), Some(g.ticks));
        // Finished: further steps change nothing.
        ghost.step();
        assert_eq!(ghost.race().tick(), g.ticks);
    }

    #[test]
    fn a_ghost_of_a_generated_track_finishes_on_its_stored_tick() {
        use crate::bot::human::{HumanBot, HumanPlan};
        use crate::bot::Driver;
        use crate::run::{Generate, Tracks};
        let track = Generate.track(11, 0, 0);
        let loadout = Loadout::BASE;
        let mut race = Race::new(track.clone(), loadout.params());
        let mut bot = HumanBot::new(HumanPlan::new(&track, race.params()), 3);
        while race.phase() == Phase::Racing {
            let input = bot.input(&race);
            race.step(input);
        }
        let g = Ghost::of(&race, loadout).expect("the human bot finishes track 1");
        let mut store = GhostStore::new();
        store.offer((11, 0, 0), g);
        let back = GhostStore::decode(&store.encode(crate::SIM_EPOCH), crate::SIM_EPOCH);
        let g = back.get((11, 0, 0)).unwrap();
        let mut ghost = GhostRun::new(track, g).expect("it replays");
        while ghost.racing() {
            ghost.step();
        }
        assert_eq!(ghost.race().tick(), race.tick());
        assert_eq!(ghost.race().splits(), race.splits());
    }

    #[test]
    fn a_record_that_does_not_replay_to_its_time_is_refused() {
        let (_, g, track) = sample();
        let wrong = Ghost {
            ticks: g.ticks + 3,
            ..g.clone()
        };
        assert!(GhostRun::new(track.clone(), &wrong).is_none());
        let other_car = Ghost {
            loadout: Loadout::BASE,
            ..g.clone()
        };
        assert!(GhostRun::new(track.clone(), &other_car).is_none());
        let unfinished = Ghost {
            log: vec![Event::Input {
                delta: 0,
                input: Input::new(0, true, false),
            }],
            ticks: 100,
            ..g
        };
        let mut timed = (*track).clone();
        timed.set_allotments(vec![60, 60]);
        assert!(GhostRun::new(Arc::new(timed), &unfinished).is_none());
    }
}
