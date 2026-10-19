//! What the game sounds like, decided from the sim's state (engine-free).
//!
//! Every sound is a file `tools/gen_audio.py` makes (`art/audio/*.wav`).
//! The core asks for them through three effects, and `game.rs` plays them:
//!
//! ```text
//!  sim tick ──► Watch::step ──► Sfx (one-shots) ──► Effect::Sound(sfx)
//!  each frame: car ──► targets ──► Mixer (fade, ≤ 30 updates/s) ──► Effect::Loops
//!  mode: title (not DEMO) ──► Effect::Music(on)
//! ```
//!
//! One-shots come from changes between sim ticks, so a frame that runs two
//! ticks still hears a gate once. The engine and tyre loops follow the car
//! continuously; the mixer eases them in and out (pause, fork, results,
//! title) and sends a new level only when it moved, at most
//! [`LOOP_UPDATE_HZ`] times a second. DEMO is silent: the core asks for
//! nothing while the bot drives.

use racer_sim::car::Car;
use racer_sim::race::{Phase, Race};
use racer_sim::TICKS_PER_SEC;

/// A one-shot sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sfx {
    /// A menu choice: start, pause, resume, a fork pick, continue.
    Click,
    /// A checkpoint gate passed.
    Chime,
    /// Each of the timer's last five seconds.
    Tick,
    /// A mini boost paid out.
    BoostMini,
    /// A super boost paid out.
    BoostSuper,
    /// A spin: a hit, or the barrier while drifting.
    Spin,
    /// The timer ran out: the run is lost.
    TimeUp,
    /// The last track finished: the run is won.
    Win,
}

impl Sfx {
    /// Every one-shot, in declaration order: the node keeps one player per
    /// entry and finds a sound's at `sfx as usize`.
    pub const ALL: [Sfx; 8] = [
        Sfx::Click,
        Sfx::Chime,
        Sfx::Tick,
        Sfx::BoostMini,
        Sfx::BoostSuper,
        Sfx::Spin,
        Sfx::TimeUp,
        Sfx::Win,
    ];

    /// Its file under `res://art/audio/`, without `.wav`.
    pub fn file(self) -> &'static str {
        match self {
            Sfx::Click => "click",
            Sfx::Chime => "chime",
            Sfx::Tick => "tick",
            Sfx::BoostMini => "boost_mini",
            Sfx::BoostSuper => "boost_super",
            Sfx::Spin => "spin",
            Sfx::TimeUp => "time_up",
            Sfx::Win => "win",
        }
    }
}

/// The looping files, under `res://art/audio/`.
pub const ENGINE_FILE: &str = "engine";
/// The tyre screech loop.
pub const DRIFT_FILE: &str = "drift";
/// The title music loop.
pub const MUSIC_FILE: &str = "music";

/// The engine and tyre loops' levels, in thousandths: pitch scale (1000 is
/// the file's own pitch) and linear volume (1000 is full). A volume of zero
/// means stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loops {
    /// The engine's `pitch_scale`, thousandths.
    pub engine_pitch: u16,
    /// The engine's volume, thousandths.
    pub engine_volume: u16,
    /// The tyre screech's volume, thousandths.
    pub drift_volume: u16,
}

impl Loops {
    /// Whether both loops are silent.
    pub fn silent(&self) -> bool {
        self.engine_volume == 0 && self.drift_volume == 0
    }
}

/// The speed the engine's pitch is scaled against, m/s: the base car's top
/// speed. An upgraded car revs higher.
pub const V_REF: f64 = 52.0;
/// Engine pitch at a standstill and the rise to [`V_REF`].
pub const PITCH_IDLE: f64 = 0.55;
/// See [`PITCH_IDLE`].
pub const PITCH_RANGE: f64 = 1.25;
/// What a boost adds to the engine's pitch and volume.
pub const BOOST_LIFT: (f64, f64) = (0.12, 0.15);
/// The tyres' sideways speed at full screech, m/s.
pub const SLIP_FULL: f64 = 5.0;
/// How fast the loops' volume eases in and out, per second (so 0 to full in
/// an eighth of a second, and out in a sixth).
pub const FADE_IN: f64 = 8.0;
/// See [`FADE_IN`].
pub const FADE_OUT: f64 = 6.0;
/// The most loop updates sent per second.
pub const LOOP_UPDATE_HZ: f64 = 30.0;

/// What the loops should be doing for this car: (pitch, engine volume,
/// tyre volume), unquantised.
pub fn loop_targets(car: &Car) -> (f64, f64, f64) {
    let r = (car.v / V_REF).clamp(0.0, 1.3);
    let (lift_p, lift_v) = if car.boost_ticks > 0 {
        BOOST_LIFT
    } else {
        (0.0, 0.0)
    };
    let pitch = PITCH_IDLE + PITCH_RANGE * r + lift_p;
    let engine = (0.45 + 0.35 * r + lift_v).min(1.0);
    let drift = if car.drifting() {
        0.35 + 0.65 * (car.vx.abs() / SLIP_FULL).min(1.0)
    } else {
        0.0
    };
    (pitch, engine, drift)
}

/// Watches one race tick by tick for the moments that make a sound.
#[derive(Clone, Copy, Debug)]
pub struct Watch {
    gate: usize,
    spinning: bool,
    boost_ticks: u16,
    timer: u32,
}

/// The countdown ticks start this many seconds from the end.
pub const COUNTDOWN_S: u32 = 5;

impl Watch {
    /// Watching `race` from where it is now.
    pub fn new(race: &Race) -> Watch {
        let car = race.car();
        Watch {
            gate: race.next_gate(),
            spinning: car.spinning(),
            boost_ticks: car.boost_ticks,
            timer: race.timer(),
        }
    }

    /// After one sim tick: push the sounds it made onto `out`.
    pub fn step(&mut self, race: &Race, out: &mut Vec<Sfx>) {
        let car = race.car();
        if race.next_gate() > self.gate {
            out.push(Sfx::Chime);
        }
        if car.spinning() && !self.spinning {
            out.push(Sfx::Spin);
        }
        if car.boost_ticks > self.boost_ticks {
            out.push(match car.last_boost {
                Some(racer_sim::car::Boost::Super) => Sfx::BoostSuper,
                _ => Sfx::BoostMini,
            });
        }
        let t = race.timer();
        if race.phase() == Phase::Racing
            && t < self.timer
            && t.is_multiple_of(TICKS_PER_SEC)
            && (1..=COUNTDOWN_S).contains(&(t / TICKS_PER_SEC))
        {
            out.push(Sfx::Tick);
        }
        *self = Watch {
            gate: race.next_gate(),
            spinning: car.spinning(),
            boost_ticks: car.boost_ticks,
            timer: t,
        };
    }
}

/// Eases the loops toward their targets and decides when to tell the
/// engine.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mixer {
    pitch: f64,
    engine: f64,
    drift: f64,
    sent: Loops,
    since: f64,
}

fn thousandths(v: f64) -> u16 {
    (v * 1000.0).round().clamp(0.0, f64::from(u16::MAX)) as u16
}

fn approach(from: f64, to: f64, delta: f64) -> f64 {
    let rate = if to > from { FADE_IN } else { FADE_OUT };
    let step = rate * delta;
    if (to - from).abs() <= step {
        to
    } else {
        from + step * (to - from).signum()
    }
}

impl Mixer {
    /// The levels last sent.
    pub fn sent(&self) -> Loops {
        self.sent
    }

    /// One frame of `delta` seconds toward `target` (pitch, engine, tyres),
    /// or toward silence. The levels to send, if it is time to.
    pub fn update(&mut self, delta: f64, target: Option<(f64, f64, f64)>) -> Option<Loops> {
        let (pitch, engine, drift) = target.unwrap_or((self.pitch, 0.0, 0.0));
        // Pitch follows the car directly: its speed is already smooth, and
        // a silent engine keeps its last pitch to fade back in from.
        if self.engine <= 0.0 || target.is_some() {
            self.pitch = pitch;
        }
        self.engine = approach(self.engine, engine, delta);
        self.drift = approach(self.drift, drift, delta);
        self.since += delta;
        let next = Loops {
            engine_pitch: thousandths(self.pitch),
            engine_volume: thousandths(self.engine),
            drift_volume: thousandths(self.drift),
        };
        if next == self.sent {
            return None;
        }
        // Starting or stopping a loop goes out at once; anything else waits
        // its turn.
        let edge = (next.engine_volume == 0) != (self.sent.engine_volume == 0)
            || (next.drift_volume == 0) != (self.sent.drift_volume == 0);
        if !edge && self.since < 1.0 / LOOP_UPDATE_HZ {
            return None;
        }
        self.since = 0.0;
        self.sent = next;
        Some(next)
    }

    /// Silence now, without a fade (the app lost focus). The levels to send
    /// if anything was playing.
    pub fn silence(&mut self) -> Option<Loops> {
        self.engine = 0.0;
        self.drift = 0.0;
        let next = Loops {
            engine_volume: 0,
            drift_volume: 0,
            ..self.sent
        };
        (next != self.sent).then(|| {
            self.sent = next;
            next
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sfx_has_a_generated_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../art/audio");
        let files = Sfx::ALL
            .iter()
            .map(|s| s.file())
            .chain([ENGINE_FILE, DRIFT_FILE, MUSIC_FILE]);
        for f in files {
            let p = dir.join(format!("{f}.wav"));
            assert!(p.is_file(), "{} missing: run `just gen-audio`", p.display());
        }
        for (i, s) in Sfx::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i, "Sfx::ALL is in declaration order");
        }
    }

    use racer_sim::input::Input;
    use racer_sim::track::fixtures;
    use std::sync::Arc;

    fn race() -> Race {
        Race::new(
            Arc::new(fixtures::drag_strip()),
            racer_sim::car::CarParams::BASE,
        )
    }

    /// Drive `race` for `ticks` with `input`, collecting the sounds.
    fn drive(race: &mut Race, w: &mut Watch, input: Input, ticks: u32) -> Vec<Sfx> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            race.step(input);
            w.step(race, &mut out);
        }
        out
    }

    #[test]
    fn a_released_drift_pays_a_whoosh_and_the_barrier_spins_with_a_thud() {
        let mut r = race();
        let mut w = Watch::new(&r);
        assert!(drive(&mut r, &mut w, Input::NEUTRAL, 240).is_empty());
        assert!(r.car().v > 18.0);
        // A gentle steered drift charges a mini boost; letting go pays it.
        let mut heard = drive(&mut r, &mut w, Input::new(1, false, true), 100);
        assert!(r.car().drifting());
        heard.extend(drive(&mut r, &mut w, Input::new(-1, false, false), 2));
        assert_eq!(heard, vec![Sfx::BoostMini]);
        // Full lock into the barrier while drifting spins the car, once.
        let heard = drive(&mut r, &mut w, Input::new(3, false, true), 240);
        assert_eq!(
            heard.iter().filter(|s| **s == Sfx::Spin).count(),
            1,
            "{heard:?}"
        );
    }

    #[test]
    fn the_last_five_seconds_tick() {
        let mut t = fixtures::drag_strip();
        t.set_allotments(vec![racer_sim::ticks(7.0)]);
        let mut r = Race::new(Arc::new(t), racer_sim::car::CarParams::BASE);
        let mut w = Watch::new(&r);
        // Braking hard, the car never reaches the gate: the timer runs out.
        let heard = drive(&mut r, &mut w, Input::new(0, true, false), 7 * 120);
        assert_eq!(r.phase(), Phase::TimeUp);
        assert_eq!(heard, vec![Sfx::Tick; 5]);
    }

    #[test]
    fn the_engine_revs_with_speed_and_a_boost_lifts_it() {
        let mut car = Car::new();
        let (idle, idle_vol, tyres) = loop_targets(&car);
        assert_eq!(tyres, 0.0);
        car.v = 40.0;
        let (fast, fast_vol, _) = loop_targets(&car);
        assert!(fast > idle && fast_vol > idle_vol);
        car.boost_ticks = 10;
        let (boost, boost_vol, _) = loop_targets(&car);
        assert!(boost > fast && boost_vol > fast_vol);
        car.drift = racer_sim::car::Drift::Drifting;
        car.vx = 1.0;
        let (_, _, a) = loop_targets(&car);
        car.vx = -4.0;
        let (_, _, b) = loop_targets(&car);
        assert!(a > 0.0 && b > a, "the screech follows the slip");
    }

    #[test]
    fn the_mixer_fades_in_rate_limits_and_fades_out() {
        let mut m = Mixer::default();
        let dt = 1.0 / 120.0;
        let first = m.update(dt, Some((1.0, 1.0, 0.0))).unwrap();
        assert!(first.engine_volume > 0 && first.engine_volume < 1000);
        assert_eq!(first.engine_pitch, 1000);
        // At 120 fps, at most one update in every four frames.
        let sent = (0..120)
            .filter(|_| m.update(dt, Some((1.0, 1.0, 0.0))).is_some())
            .count();
        assert!((3..=31).contains(&sent), "{sent}");
        assert_eq!(m.sent().engine_volume, 1000);
        // Steady levels send nothing.
        assert!((0..30).all(|_| m.update(dt, Some((1.0, 1.0, 0.0))).is_none()));
        // Silence fades out within a fifth of a second and then stops.
        let mut last = None;
        for _ in 0..24 {
            if let Some(l) = m.update(dt, None) {
                last = Some(l);
            }
        }
        let last = last.unwrap();
        assert!(last.silent());
        assert_eq!(last.engine_pitch, 1000, "the pitch is kept");
        assert!(m.silence().is_none(), "already silent");
    }
}
