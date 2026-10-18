//! Seed plus input log to verified reconstruction. DESIGN §8.
//!
//! Nothing else is persisted. A race is its track and its log, where each
//! log entry is a change of input and the ticks since the previous change;
//! a run (`run.rs`) is its seed and the races' logs end to end, with a fork
//! marker for each route choice. [`Replay`] rebuilds a run from exactly
//! that.
//!
//! # Event sourcing
//!
//! A replay stores inputs, never state. Everything else, from the car's
//! position to the timer, is recomputed by feeding the log back through the
//! same [`Race::step`] the live game calls. That only works because the sim
//! is deterministic to the bit (see the crate docs), and it is what keeps a
//! whole 60-second race to a few hundred bytes of text. The trade-off is that
//! a log is only meaningful to a sim that drives identically, which is what
//! [`crate::SIM_EPOCH`] tracks (DESIGN §8.3).
//!
//! Logging changes rather than ticks is the compression: a player holds a
//! steering level for many ticks, so a race of 7,200 ticks is a few hundred
//! events.

use std::sync::Arc;

use crate::car::CarParams;
use crate::hash::Hasher;
use crate::input::{Input, STEER_LEVELS};
use crate::race::{Phase, Race};
use crate::run::{state_hash, RunPhase, RunState, Tracks};
use crate::track::Track;

/// One entry of an input log.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// From `delta` ticks after the previous input change (or the race's
    /// start) the input is `input`. The input before any change is
    /// [`Input::NEUTRAL`].
    Input {
        /// Ticks since the previous change.
        delta: u32,
        /// The new input.
        input: Input,
    },
    /// The route choice after a race: take fork `branch` (DESIGN §1.3).
    Fork {
        /// 0 or 1.
        branch: u8,
    },
}

/// Fold a log into a running hash.
pub fn hash_events(h: &mut Hasher, log: &[Event]) {
    h.write_usize(log.len());
    for e in log {
        match *e {
            Event::Input { delta, input } => {
                h.write_u8(0);
                h.write_u32(delta);
                h.write_u8(input.bits());
            }
            Event::Fork { branch } => {
                h.write_u8(1);
                h.write_u8(branch);
            }
        }
    }
}

/// Why a log failed to replay. `at` is the index of the first event that
/// could not be applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayError {
    /// An input change logged after the race had already ended.
    InputAfterEnd {
        /// Index into the log.
        at: usize,
    },
    /// A fork where none can be taken: mid-race, after a lost race or after
    /// the last track, or a branch other than 0 or 1.
    UnexpectedFork {
        /// Index into the log.
        at: usize,
    },
}

/// Something a log of input changes can drive: a race, or a run's current
/// race.
pub trait Steppable {
    /// Advance one tick with `input`.
    fn step(&mut self, input: Input);
    /// Whether a race is under way.
    fn racing(&self) -> bool;
}

impl Steppable for Race {
    fn step(&mut self, input: Input) {
        Race::step(self, input);
    }
    fn racing(&self) -> bool {
        self.phase() == Phase::Racing
    }
}

/// Feed one race's input changes from the front of `events` to `s`, then
/// run on with the last input until the race ends. Returns how many events
/// it used: it stops at the first fork marker. `at` is the index of
/// `events[0]` in the whole log, for errors.
fn feed(s: &mut impl Steppable, events: &[Event], at: usize) -> Result<usize, ReplayError> {
    let mut used = 0;
    let mut input = Input::NEUTRAL;
    for (i, e) in events.iter().enumerate() {
        let Event::Input { delta, input: next } = *e else {
            break;
        };
        for _ in 0..delta {
            s.step(input);
        }
        if !s.racing() {
            return Err(ReplayError::InputAfterEnd { at: at + i });
        }
        input = next;
        used += 1;
    }
    while s.racing() {
        s.step(input);
    }
    Ok(used)
}

/// Replay one race's input changes, `events`, on `track`, then let the race
/// run on with its last input until it ends. Returns the finished race and
/// how many events it consumed: a run's log continues with a fork marker.
///
/// A log entry that does not apply ends the reconstruction with an error
/// rather than being skipped: a log that does not replay cleanly is a
/// determinism bug, and it should be visible as one.
///
/// # Errors
///
/// [`ReplayError::InputAfterEnd`] if an input change comes after the race
/// has ended; the index is relative to `events`.
pub fn replay_race(
    track: Arc<Track>,
    params: CarParams,
    events: &[Event],
) -> Result<(Race, usize), ReplayError> {
    let mut race = Race::new(track, params);
    let used = feed(&mut race, events, 0)?;
    Ok((race, used))
}

/// A complete, replayable run: its seed and its log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    /// The run's seed.
    pub run_seed: u64,
    /// Every input change and fork, in order.
    pub log: Vec<Event>,
}

impl Replay {
    /// Snapshot a run's identity and log. Its reconstruction matches the
    /// live run at race boundaries: once the current race has ended, or a
    /// fork has just been taken.
    pub fn of(run: &RunState) -> Self {
        Replay {
            run_seed: run.run_seed(),
            log: run.log(),
        }
    }

    /// Rebuild the run, race by race, getting tracks from `tracks`. Each
    /// race is driven by its logged inputs and then runs on with the last
    /// input to its end; each fork marker takes its branch.
    ///
    /// # Errors
    ///
    /// A [`ReplayError`] naming the first log entry that cannot be applied.
    pub fn reconstruct(&self, tracks: &mut impl Tracks) -> Result<RunState, ReplayError> {
        let mut run = RunState::new(self.run_seed, tracks);
        let mut at = 0;
        loop {
            at += feed(&mut run, &self.log[at..], at)?;
            match self.log.get(at) {
                None => return Ok(run),
                Some(&Event::Fork { branch }) => {
                    if run.phase() != RunPhase::Forking || !run.fork(branch, tracks) {
                        return Err(ReplayError::UnexpectedFork { at });
                    }
                    at += 1;
                }
                Some(Event::Input { .. }) => unreachable!("feed stops only at a fork or the end"),
            }
        }
    }

    /// Reconstruct and hash. This is the number CI compares across platforms.
    ///
    /// # Errors
    ///
    /// As [`Replay::reconstruct`].
    pub fn final_hash(&self, tracks: &mut impl Tracks) -> Result<u64, ReplayError> {
        self.reconstruct(tracks).map(|r| state_hash(&r))
    }
}

// ------------------------------------------------------------- text form

/// Version of the text form, written as a leading `v1` token.
pub const LOG_FORMAT: u32 = 1;

/// The steering token for each level, left to right.
const STEER_TOKENS: [&str; 7] = ["L3", "L2", "L1", "C", "R1", "R2", "R3"];

/// Encode a log as space-separated tokens:
///
/// - `v1` comes first: the format version, [`LOG_FORMAT`].
/// - `>B` is a fork to branch `B`.
/// - `D:S` is an input change `D` ticks after the previous one, where `S`
///   is the steering (`L3`..`L1`, `C`, `R1`..`R3`) followed by `B` if the
///   brake is held and then `D` if drift is held: `40:R2D`, `3:CB`.
pub fn encode_log(log: &[Event]) -> String {
    let mut out = vec![format!("v{LOG_FORMAT}")];
    for e in log {
        out.push(match *e {
            Event::Fork { branch } => format!(">{branch}"),
            Event::Input { delta, input } => {
                let mut t = format!(
                    "{delta}:{}",
                    STEER_TOKENS[(input.steer_level() + STEER_LEVELS) as usize]
                );
                if input.brake() {
                    t.push('B');
                }
                if input.drift() {
                    t.push('D');
                }
                t
            }
        });
    }
    out.join(" ")
}

/// Why a text log was rejected. `at` is the token index, counting the
/// version token as 0.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogError {
    /// The text does not start with a `vN` token.
    MissingVersion,
    /// A version other than [`LOG_FORMAT`].
    UnsupportedVersion(u32),
    /// Not a token of the format, or a number not written the way
    /// [`encode_log`] writes it (`+5`, `05`).
    BadToken {
        /// Token index.
        at: usize,
    },
}

/// Parse [`encode_log`]'s format. Only canonical text is accepted, so for any
/// text this accepts, `encode_log(&decode_log(text)?)` is the same tokens.
///
/// Strict on purpose, as in Salt Line: with one canonical spelling a log can
/// be compared, deduplicated or hashed as text, and a corrupted line is
/// rejected instead of being read as something nearby. Whitespace is the one
/// allowance. Decoding checks the syntax only; whether the events make a
/// legal run is for the replay to decide.
///
/// # Errors
///
/// A [`LogError`] for the first token that breaks the format.
pub fn decode_log(text: &str) -> Result<Vec<Event>, LogError> {
    let mut toks = text.split_whitespace().enumerate();
    let version = toks
        .next()
        .and_then(|(_, t)| t.strip_prefix('v'))
        .ok_or(LogError::MissingVersion)?;
    let version: u32 = canonical(version).ok_or(LogError::MissingVersion)?;
    if version != LOG_FORMAT {
        return Err(LogError::UnsupportedVersion(version));
    }
    let mut log = Vec::new();
    for (at, tok) in toks {
        let bad = LogError::BadToken { at };
        if let Some(b) = tok.strip_prefix('>') {
            log.push(Event::Fork {
                branch: canonical(b).ok_or(bad)?,
            });
            continue;
        }
        let (d, rest) = tok.split_once(':').ok_or(bad)?;
        let delta = canonical(d).ok_or(bad)?;
        let (rest, drift) = match rest.strip_suffix('D') {
            Some(r) => (r, true),
            None => (rest, false),
        };
        let (steer, brake) = match rest.strip_suffix('B') {
            Some(r) => (r, true),
            None => (rest, false),
        };
        let level = STEER_TOKENS.iter().position(|&s| s == steer).ok_or(bad)?;
        log.push(Event::Input {
            delta,
            input: Input::new(level as i8 - STEER_LEVELS, brake, drift),
        });
    }
    Ok(log)
}

/// Parse a number only if it is written exactly as `Display` writes it.
fn canonical<T: core::str::FromStr + ToString>(s: &str) -> Option<T> {
    let v: T = s.parse().ok()?;
    (v.to_string() == s).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::fixtures;

    fn left() -> Input {
        Input::new(-2, false, true)
    }

    #[test]
    fn the_text_form_reads_as_documented() {
        let log = vec![
            Event::Input {
                delta: 40,
                input: Input::new(2, false, true),
            },
            Event::Input {
                delta: 3,
                input: Input::new(0, true, false),
            },
            Event::Fork { branch: 1 },
            Event::Input {
                delta: 0,
                input: left(),
            },
        ];
        let text = encode_log(&log);
        assert_eq!(text, "v1 40:R2D 3:CB >1 0:L2D");
        assert_eq!(decode_log(&text), Ok(log));
    }

    #[test]
    fn the_text_form_rejects_anything_it_would_not_write() {
        assert_eq!(decode_log(""), Err(LogError::MissingVersion));
        assert_eq!(decode_log("v2"), Err(LogError::UnsupportedVersion(2)));
        assert_eq!(decode_log("v01"), Err(LogError::MissingVersion));
        for bad in ["05:C", "+5:C", "5:X", "5:CDB", "5:R4", ">01", "5", "5:"] {
            assert_eq!(
                decode_log(&format!("v1 {bad}")),
                Err(LogError::BadToken { at: 1 }),
                "{bad}"
            );
        }
        assert_eq!(
            decode_log("  v1   3:C  "),
            Ok(vec![Event::Input {
                delta: 3,
                input: Input::NEUTRAL
            }])
        );
    }

    #[test]
    fn a_race_replays_from_its_log() {
        let track = Arc::new(fixtures::sweep_and_hairpin());
        let mut live = Race::new(track.clone(), CarParams::BASE);
        let mut n = 0u32;
        while live.phase() == Phase::Racing {
            n += 1;
            let steer = ((n / 37) % 7) as i8 - 3;
            live.step(Input::new(steer, n % 300 < 20, n % 500 < 100));
        }
        let (rebuilt, used) = replay_race(track, CarParams::BASE, live.log()).unwrap();
        assert_eq!(used, live.log().len());
        let (mut a, mut b) = (Hasher::new(), Hasher::new());
        live.hash_into(&mut a);
        rebuilt.hash_into(&mut b);
        assert_eq!(a.finish(), b.finish());
    }

    #[test]
    fn a_run_replays_from_its_seed_and_log() {
        let mut tracks = crate::run::TrackCache::new();
        let live = crate::run::autoplay(11, 3, &mut tracks);
        assert!(matches!(live.phase(), RunPhase::Won | RunPhase::Lost));
        let replay = Replay::of(&live);
        assert!(
            replay.log.iter().any(|e| matches!(e, Event::Fork { .. })) || live.results().len() == 1
        );
        let text = encode_log(&replay.log);
        assert_eq!(decode_log(&text).as_ref(), Ok(&replay.log));
        assert_eq!(replay.final_hash(&mut tracks), Ok(state_hash(&live)));
    }

    #[test]
    fn a_fork_out_of_place_is_an_error() {
        let mut tracks = crate::run::TrackCache::new();
        // Braking the whole first race loses it: no fork follows a loss.
        let brake = Event::Input {
            delta: 0,
            input: Input::new(0, true, false),
        };
        let after_loss = Replay {
            run_seed: 11,
            log: vec![brake, Event::Fork { branch: 0 }],
        };
        assert_eq!(
            after_loss.reconstruct(&mut tracks).err(),
            Some(ReplayError::UnexpectedFork { at: 1 })
        );
        // And there is never a third road.
        let live = crate::run::autoplay(11, 3, &mut tracks);
        if live.results().len() > 1 {
            let mut log = Replay::of(&live).log;
            let at = log
                .iter()
                .position(|e| matches!(e, Event::Fork { .. }))
                .unwrap();
            log[at] = Event::Fork { branch: 2 };
            let bad = Replay { run_seed: 11, log };
            assert_eq!(
                bad.reconstruct(&mut tracks).err(),
                Some(ReplayError::UnexpectedFork { at })
            );
        }
    }

    #[test]
    fn an_input_after_the_end_is_an_error() {
        let track = Arc::new(fixtures::drag_strip());
        let log = [Event::Input {
            delta: crate::race::MAX_RACE_TICKS,
            input: left(),
        }];
        assert_eq!(
            replay_race(track, CarParams::BASE, &log).err(),
            Some(ReplayError::InputAfterEnd { at: 0 })
        );
    }
}
