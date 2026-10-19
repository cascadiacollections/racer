//! The game itself, with no engine in it (Salt Line's core/adapter split).
//!
//! [`Game`] owns the mode, the run, the track builder, touch and the frame
//! clock. It is stepped by [`Game::tick`] with a frame's [`Intents`] and
//! answers with the [`Effect`]s the engine must carry out. It reads no
//! clock, file or device: the frame's length, the window and the entropy
//! for a new seed come in as arguments.
//!
//! ```text
//!  Intents ──► Game::tick ──► Vec<Effect> ──► game.rs carries them out
//!                  │
//!                  ▼
//!            &Game ──► screens::frame ──► Canvas + Vec<Button>
//!                                            │          │
//!                                   texture upload   set_buttons: next
//!                                                    frame's touches
//! ```
//!
//! The run is the sim's [`RunState`], stepped at 120 Hz from a frame-time
//! accumulator with the frame's input held for every tick in it. Modes:
//!
//! ```text
//! Title ─start─► Building ─track 1 built─► Racing ─finish─► Fork ─pick─► Racing ...
//!   ▲                                        │ time up / track 5              │
//!   └──────────────── Results ◄──────────────┘◄────────────────────────────────┘
//! ```
//!
//! Racing pauses on focus loss and on the back gesture; DEMO lets the sim's
//! human-like bot drive and pick forks, and any input ends it.
//!
//! The ghost of the player's best (`racer_sim::ghost`) is decided here too:
//! a finished track's race is offered to the [`GhostStore`] (kept as
//! `ghosts.txt` by the adapter through [`Effect::SaveGhosts`]), and a race on
//! a track with a stored best drives a [`GhostRun`] one tick per player
//! tick. It only ever reads the player's race, never steps or changes it.
//! RETRY SEED on the results screen starts the same seed again: the same
//! tracks, so the ghosts of the last attempt are there to race.
//!
//! Sound is decided here too (`audio`): one-shots from what changed between
//! sim ticks, the engine and tyre loops from the car each frame, the title
//! music from the mode. DEMO asks for none of it.

use std::collections::BTreeSet;
use std::sync::Arc;

use racer_sim::bot::human::{HumanBot, HumanPlan};
use racer_sim::bot::Driver;
use racer_sim::gen::difficulty::TRACKS;
use racer_sim::ghost::{Ghost, GhostRun, GhostStore};
use racer_sim::input::Input;
use racer_sim::replay::{encode_run, Ending, Replay, RunFile};
use racer_sim::rng::{mix64, substream};
use racer_sim::run::{RunPhase, RunState, TrackCache};
use racer_sim::track::{fixtures, Track};
use racer_sim::DT;

use crate::audio::{loop_targets, Loops, Mixer, Sfx, Watch};
use crate::builder::{Build, Key};
use crate::input::{bind, pressed, Controls, Event, Intents, SteerRamp};
use crate::layout::{buffer_size, inset_rows, Layout, H_MIN};
use crate::render::{Decor, Held};
use crate::touch::{Action, Button, TouchPad};

/// The longest frame the clock accepts, seconds. A longer gap (a hitch, a
/// debugger) runs as this much, so the sim never races to catch up.
pub const MAX_FRAME: f64 = 0.1;

/// How far the background scrolls per radian the road turns, pixels.
pub const BG_PX_PER_RAD: f64 = 150.0;

/// How long DEMO looks at a fork before picking, seconds.
pub const DEMO_FORK_WAIT: f64 = 2.0;

/// How long a gate's time against the ghost stays up, race ticks.
pub const GHOST_NOTE_TICKS: u32 = 2 * racer_sim::TICKS_PER_SEC;

/// The ghost file's name, in the adapter's `user://ghosts`.
pub const GHOSTS_FILE: &str = "ghosts.txt";

/// Where the game is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The title screen.
    Title,
    /// Waiting for the first track to be built.
    Building,
    /// Driving (maybe paused).
    Racing,
    /// Choosing the next track.
    Fork,
    /// The run is over: won or out of time.
    Results,
}

/// What the engine must do for the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Something on screen changed: draw the next frame.
    Redraw,
    /// The buffer is `w` by `h`: resize the canvas and the window's content
    /// scale.
    Buffer { w: i32, h: i32 },
    /// Hold the screen on (racing) or let it sleep.
    KeepScreenOn(bool),
    /// Leave the game.
    Quit,
    /// Keep a finished or abandoned player run for playtest analysis:
    /// write `text` (racer_sim::replay::encode_run) as file `name` in the
    /// runs directory. Never for DEMO.
    SaveRun { name: String, text: String },
    /// Play a one-shot sound once.
    Sound(Sfx),
    /// Set the engine and tyre loops: start, stop or retune them.
    Loops(Loops),
    /// Start or stop the title music.
    Music(bool),
    /// The best-per-track store changed: write `text`
    /// ([`GhostStore::encode`]) as [`GHOSTS_FILE`] in the user data
    /// directory. Never for DEMO.
    SaveGhosts { text: String },
}

/// The DEMO driver.
struct Demo {
    bot: Option<HumanBot>,
    fork_wait: f64,
}

/// The game.
pub struct Game {
    /// Where the game is.
    pub mode: Mode,
    /// Racing, but paused.
    pub paused: bool,
    /// The layout of the current buffer.
    pub layout: Layout,
    /// The run, from the first track being built until the next title.
    pub run: Option<RunState>,
    /// The run's seed.
    pub seed: u64,
    /// The fork card selected by keys or pad.
    pub fork_sel: u8,
    /// The results button selected by keys or pad: 0 is CONTINUE, 1 is
    /// RETRY SEED.
    pub results_sel: u8,
    /// Background scroll, pixels (fractional; drawn snapped).
    pub bg: f64,
    /// Seconds since launch, for menu animation.
    pub clock: f64,
    /// Touch state.
    pub touch: TouchPad,
    /// What is held, for drawing the pad.
    pub held: Held,
    /// Decor of the track on screen.
    pub decor: Decor,
    /// The track behind the title screen, and its decor.
    pub title_track: Arc<Track>,
    /// Decor of the title track.
    pub title_decor: Decor,
    cache: TrackCache,
    ready: BTreeSet<Key>,
    builder: Box<dyn Build>,
    buttons: Vec<Button>,
    acc: f64,
    /// Builds the player's held direction up to full lock, per sim tick.
    ramp: SteerRamp,
    demo: Option<Demo>,
    window: (i32, i32),
    /// The bottom inset the layout was made for, screen pixels.
    inset_px: i32,
    fx: Vec<Effect>,
    /// One-shots asked for this frame, deduplicated when sent.
    sfx: Vec<Sfx>,
    /// Watches the race on screen for one-shots.
    watch: Option<Watch>,
    mixer: Mixer,
    /// Whether the title music was last asked to play.
    music: bool,
    /// The fastest finish per track.
    ghosts: GhostStore,
    /// The ghost on the track being raced, if it has a stored best.
    ghost: Option<GhostRun>,
    /// The last gate's time against the ghost, ticks, and the race tick it
    /// is shown until.
    ghost_note: Option<(i64, u32)>,
    /// Gates crossed on this track so far, to notice the next.
    gates_seen: usize,
    /// Retries of this seed before this attempt, for the run file's name.
    attempt: u32,
}

impl Game {
    /// A game on its title screen, building tracks with `builder`.
    pub fn new(builder: Box<dyn Build>) -> Game {
        let title_track = Arc::new(fixtures::sweep_and_hairpin());
        let title_decor = Decor::new(&title_track);
        Game {
            mode: Mode::Title,
            paused: false,
            layout: Layout::new(H_MIN),
            run: None,
            seed: 0,
            fork_sel: 0,
            bg: 0.0,
            clock: 0.0,
            touch: TouchPad::default(),
            held: Held::default(),
            decor: Decor::default(),
            title_track,
            title_decor,
            cache: TrackCache::new(),
            ready: BTreeSet::new(),
            builder,
            buttons: Vec::new(),
            acc: 0.0,
            ramp: SteerRamp::default(),
            demo: None,
            window: (0, 0),
            inset_px: 0,
            fx: Vec::new(),
            sfx: Vec::new(),
            watch: None,
            mixer: Mixer::default(),
            music: false,
            results_sel: 0,
            ghosts: GhostStore::new(),
            ghost: None,
            ghost_note: None,
            gates_seen: 0,
            attempt: 0,
        }
    }

    /// Read the stored ghosts ([`GHOSTS_FILE`]'s text). Ghosts from another
    /// sim epoch and damaged lines are dropped.
    pub fn load_ghosts(&mut self, text: &str) {
        self.ghosts = GhostStore::decode(text, racer_sim::SIM_EPOCH);
    }

    /// The stored ghosts.
    pub fn ghosts(&self) -> &GhostStore {
        &self.ghosts
    }

    /// The ghost to draw: on this track, and still driving.
    pub fn ghost(&self) -> Option<&GhostRun> {
        self.ghost.as_ref().filter(|g| g.racing())
    }

    /// The last gate's time against the ghost while it is shown, ticks
    /// (negative is ahead of it).
    pub fn ghost_note(&self) -> Option<i64> {
        let (d, until) = self.ghost_note?;
        let tick = self.run.as_ref()?.race().tick();
        (tick < until).then_some(d)
    }

    /// The buttons drawn last frame: what this frame's touches hit.
    pub fn set_buttons(&mut self, buttons: Vec<Button>) {
        self.buttons = buttons;
    }

    /// Whether the DEMO bot is driving.
    pub fn demo(&self) -> bool {
        self.demo.is_some()
    }

    /// Whether track `key` has been built.
    pub fn built(&self, key: Key) -> bool {
        self.ready.contains(&key)
    }

    /// A built track.
    pub fn track(&mut self, key: Key) -> Option<Arc<Track>> {
        use racer_sim::run::Tracks;
        self.built(key)
            .then(|| self.cache.track(key.0, key.1, key.2))
    }

    /// Whether anything moves on its own this frame.
    pub fn busy(&self) -> bool {
        match self.mode {
            Mode::Racing => !self.paused,
            Mode::Title | Mode::Building => true,
            Mode::Fork => self.demo.is_some() || !self.fork_ready(),
            Mode::Results => false,
        }
    }

    /// Whether the engine should idle at the menu frame rate.
    pub fn on_menu(&self) -> bool {
        self.mode != Mode::Racing || self.paused
    }

    /// The two tracks of the coming fork, if both are built.
    pub fn fork_ready(&self) -> bool {
        let Some(run) = &self.run else { return false };
        (0..2).all(|b| self.built((self.seed, run.index() + 1, b)))
    }

    /// One frame. `delta` is the frame's length in seconds.
    pub fn tick(&mut self, delta: f64, i: &Intents) -> Vec<Effect> {
        let delta = delta.clamp(0.0, MAX_FRAME);
        self.step(delta, i);
        self.sound(delta);
        std::mem::take(&mut self.fx)
    }

    /// Whether the game is silent: DEMO.
    pub fn quiet(&self) -> bool {
        self.demo.is_some()
    }

    /// The frame's sound effects: its one-shots, the loops and the music.
    fn sound(&mut self, delta: f64) {
        let quiet = self.quiet();
        let mut sfx = std::mem::take(&mut self.sfx);
        if quiet {
            sfx.clear();
        }
        let mut seen = Vec::with_capacity(sfx.len());
        for s in sfx.drain(..) {
            if !seen.contains(&s) {
                seen.push(s);
                self.fx.push(Effect::Sound(s));
            }
        }
        self.sfx = sfx;
        let target = match (&self.run, self.mode, self.paused, quiet) {
            (Some(run), Mode::Racing, false, false) if run.phase() == RunPhase::Racing => {
                Some(loop_targets(run.race().car()))
            }
            _ => None,
        };
        if let Some(l) = self.mixer.update(delta, target) {
            self.fx.push(Effect::Loops(l));
        }
        let music = !quiet && matches!(self.mode, Mode::Title | Mode::Building);
        if music != self.music {
            self.music = music;
            self.fx.push(Effect::Music(music));
        }
    }

    fn step(&mut self, delta: f64, i: &Intents) {
        self.clock += delta;
        if (i.window != self.window || i.bottom_inset != self.inset_px) && i.window.0 > 0 {
            self.window = i.window;
            self.inset_px = i.bottom_inset;
            let (w, h) = buffer_size(i.window);
            self.layout = Layout::with_inset(h, inset_rows(i.bottom_inset, i.window.1, h));
            self.fx.push(Effect::Buffer { w, h });
            self.fx.push(Effect::Redraw);
        }
        for (key, track) in self.builder.poll() {
            self.cache.insert(key.0, key.1, key.2, track);
            self.ready.insert(key);
            self.fx.push(Effect::Redraw);
        }
        let mut back = false;
        for e in &i.events {
            match *e {
                Event::FingerDown { index, x, y } => {
                    self.touch.finger_down(index, x, y, &self.buttons)
                }
                Event::FingerMoved { index, x, y } => {
                    self.touch.finger_moved(index, x, y, &self.buttons)
                }
                Event::FingerUp { index } => self.touch.finger_up(index),
                Event::Back => back = true,
                Event::Key(_) | Event::Pad(_) => {}
            }
        }
        self.touch.update();
        if !i.events.is_empty() {
            self.fx.push(Effect::Redraw);
        }
        let any = |keys, pads| i.events.iter().any(|e| pressed(e, keys, pads));
        let confirm = any(bind::CONFIRM, bind::PAD_CONFIRM);
        let pause = any(bind::PAUSE, bind::PAD_PAUSE);

        if self.demo.is_some() && self.mode != Mode::Title {
            let woke = i
                .events
                .iter()
                .any(|e| matches!(e, Event::Key(_) | Event::Pad(_) | Event::FingerDown { .. }));
            if woke || back {
                self.go_to_title();
                return;
            }
        }
        if back {
            self.back();
        }
        match self.mode {
            Mode::Title => {
                if confirm || self.touch.pressed(Action::Start) {
                    self.sfx.push(Sfx::Click);
                    self.start(i.entropy, false);
                } else if self.touch.pressed(Action::Demo) {
                    self.start(i.entropy, true);
                }
            }
            Mode::Building => {
                let key = (self.seed, 0, 0);
                if self.built(key) {
                    let run = RunState::new(self.seed, &mut self.cache);
                    self.run = Some(run);
                    self.begin_race();
                }
            }
            Mode::Racing => self.racing(delta, i, confirm, pause),
            Mode::Fork => self.forking(delta, i, confirm),
            Mode::Results => self.results(i, confirm),
        }
    }

    /// The window lost focus or the app was sent to the background: let go
    /// of every finger and pause a race.
    pub fn focus_lost(&mut self) -> Vec<Effect> {
        self.touch.release_all();
        if self.mode == Mode::Racing && !self.paused && self.demo.is_none() {
            self.paused = true;
            self.fx.push(Effect::KeepScreenOn(false));
        }
        if let Some(l) = self.mixer.silence() {
            self.fx.push(Effect::Loops(l));
        }
        self.fx.push(Effect::Redraw);
        std::mem::take(&mut self.fx)
    }

    fn results(&mut self, i: &Intents, confirm: bool) {
        let any = |keys: &[_], pads: &[_]| i.events.iter().any(|e| pressed(e, keys, pads));
        let sel = self.results_sel;
        if any(bind::MENU_UP, bind::PAD_UP) {
            self.results_sel = 1;
        }
        if any(bind::MENU_DOWN, bind::PAD_DOWN) {
            self.results_sel = 0;
        }
        if self.results_sel != sel {
            self.sfx.push(Sfx::Click);
        }
        let retry = self.touch.pressed(Action::Retry) || (confirm && self.results_sel == 1);
        if retry && self.demo.is_none() {
            self.sfx.push(Sfx::Click);
            let (seed, attempt) = (self.seed, self.attempt + 1);
            self.go_to_title();
            self.start_seed(seed, false);
            self.attempt = attempt;
        } else if confirm || self.touch.pressed(Action::Continue) {
            self.sfx.push(Sfx::Click);
            self.go_to_title();
        }
    }

    fn back(&mut self) {
        match self.mode {
            Mode::Title => self.fx.push(Effect::Quit),
            Mode::Racing => self.set_paused(!self.paused),
            _ => self.go_to_title(),
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.sfx.push(Sfx::Click);
        self.touch.release_all();
        self.fx.push(Effect::KeepScreenOn(!paused));
        self.fx.push(Effect::Redraw);
    }

    /// Queue the current run for saving, unless DEMO drove it.
    fn save_run(&mut self, ending: Ending) {
        let (Some(run), None) = (&self.run, &self.demo) else {
            return;
        };
        let f = RunFile {
            epoch: racer_sim::SIM_EPOCH,
            ending,
            replay: Replay::of(run),
        };
        let name = match self.attempt {
            0 => format!("run-{:016x}.txt", f.replay.run_seed),
            n => format!("run-{:016x}-{n}.txt", f.replay.run_seed),
        };
        self.fx.push(Effect::SaveRun {
            name,
            text: encode_run(&f),
        });
    }

    fn go_to_title(&mut self) {
        self.mode = Mode::Title;
        self.paused = false;
        self.demo = None;
        self.run = None;
        self.watch = None;
        self.ghost = None;
        self.ghost_note = None;
        self.results_sel = 0;
        self.touch.release_all();
        self.held = Held::default();
        self.fx.push(Effect::KeepScreenOn(false));
        self.fx.push(Effect::Redraw);
    }

    fn start(&mut self, entropy: u64, demo: bool) {
        self.attempt = 0;
        self.start_seed(mix64(entropy).max(1), demo);
    }

    /// Start a run of `seed`: a new one, or the same again (RETRY SEED; a
    /// daily seed later). The same seed is the same tracks, so whatever was
    /// already built is used at once.
    pub fn start_seed(&mut self, seed: u64, demo: bool) {
        self.seed = seed;
        self.request((seed, 0, 0));
        self.mode = Mode::Building;
        self.demo = demo.then_some(Demo {
            bot: None,
            fork_wait: DEMO_FORK_WAIT,
        });
        self.touch.release_all();
        self.fx.push(Effect::Redraw);
    }

    fn begin_race(&mut self) {
        let Some(run) = &self.run else { return };
        let index = run.index();
        let track = run.race().track().clone();
        self.watch = Some(Watch::new(run.race()));
        self.decor = Decor::new(&track);
        let key = (self.seed, index, run.branch());
        self.ghost = match (&self.demo, self.ghosts.get(key)) {
            (None, Some(g)) => GhostRun::new(track.clone(), g),
            _ => None,
        };
        self.ghost_note = None;
        self.gates_seen = 0;
        if let Some(d) = &mut self.demo {
            let plan = HumanPlan::new(&track, run.race().params());
            d.bot = Some(HumanBot::new(plan, substream(self.seed, index as u64)));
            d.fork_wait = DEMO_FORK_WAIT;
        }
        if index + 1 < TRACKS {
            for b in 0..2 {
                self.request((self.seed, index + 1, b));
            }
        }
        self.mode = Mode::Racing;
        self.paused = false;
        self.acc = 0.0;
        self.touch.release_all();
        self.fx.push(Effect::KeepScreenOn(true));
        self.fx.push(Effect::Redraw);
    }

    /// Ask for a track unless it is already built.
    fn request(&mut self, key: Key) {
        if !self.built(key) {
            self.builder.request(key);
        }
    }

    /// The player finished the track: keep the race as its ghost if it is
    /// the best there, and have the store written if so. Never for DEMO.
    fn record_best(&mut self) {
        let (Some(run), None) = (&self.run, &self.demo) else {
            return;
        };
        let Some(g) = Ghost::of(run.race(), run.loadout()) else {
            return;
        };
        if self.ghosts.offer((self.seed, run.index(), run.branch()), g) {
            self.fx.push(Effect::SaveGhosts {
                text: self.ghosts.encode(racer_sim::SIM_EPOCH),
            });
        }
    }

    /// Touch and every other device, combined.
    fn controls(&self, i: &Intents) -> Controls {
        let t = &self.touch;
        Controls {
            left: t.held(Action::Left),
            right: t.held(Action::Right),
            brake: t.held(Action::Brake),
            drift: t.held(Action::Drift),
            stick: 0.0,
        }
        .or(Controls::held(i))
    }

    fn racing(&mut self, delta: f64, i: &Intents, confirm: bool, pause: bool) {
        if pause || self.touch.pressed(Action::Pause) {
            self.set_paused(!self.paused);
            return;
        }
        if self.paused {
            if confirm || self.touch.pressed(Action::Resume) {
                self.set_paused(false);
            } else if self.touch.pressed(Action::Quit) {
                self.sfx.push(Sfx::Click);
                self.save_run(Ending::Quit);
                self.go_to_title();
            }
            return;
        }
        let controls = self.controls(i);
        let player = controls.quantise();
        let Some(run) = &mut self.run else { return };
        self.acc += delta;
        let mut last = player;
        while self.acc >= DT {
            self.acc -= DT;
            let input = match self.demo.as_mut().and_then(|d| d.bot.as_mut()) {
                Some(bot) => bot.input(run.race()),
                None => self.ramp.shape(player, controls.left != controls.right),
            };
            run.step(input);
            // The ghost takes its tick beside the player's, from its own log.
            if let Some(g) = &mut self.ghost {
                g.step();
            }
            let splits = run.race().splits();
            while self.gates_seen < splits.len() {
                let k = self.gates_seen;
                self.gates_seen += 1;
                if let Some(at) = self.ghost.as_ref().and_then(|g| g.split(k)) {
                    let d = i64::from(splits[k]) - i64::from(at);
                    self.ghost_note = Some((d, splits[k] + GHOST_NOTE_TICKS));
                }
            }
            if let Some(w) = &mut self.watch {
                w.step(run.race(), &mut self.sfx);
            }
            last = input;
            let car = run.race().car();
            let k = run.race().track().segment_at(car.z).curvature();
            self.bg += k * car.v * DT * BG_PX_PER_RAD;
            if run.phase() != RunPhase::Racing {
                break;
            }
        }
        self.held = if self.demo.is_some() {
            held_of(last)
        } else {
            Held {
                left: controls.left,
                right: controls.right,
                brake: controls.brake,
                drift: controls.drift,
            }
        };
        if matches!(run.phase(), RunPhase::Forking | RunPhase::Won) {
            self.record_best();
        }
        let Some(run) = &self.run else { return };
        match run.phase() {
            RunPhase::Racing => {}
            RunPhase::Forking => {
                self.mode = Mode::Fork;
                self.fork_sel = 0;
                self.touch.release_all();
                self.fx.push(Effect::KeepScreenOn(false));
            }
            RunPhase::Won | RunPhase::Lost => {
                let ending = if run.phase() == RunPhase::Won {
                    Ending::Won
                } else {
                    Ending::Lost
                };
                // The fanfare or the sting stands in for the last chime.
                self.sfx.retain(|s| *s != Sfx::Chime);
                self.sfx.push(if ending == Ending::Won {
                    Sfx::Win
                } else {
                    Sfx::TimeUp
                });
                self.mode = Mode::Results;
                self.touch.release_all();
                self.fx.push(Effect::KeepScreenOn(false));
                self.save_run(ending);
            }
        }
    }

    fn forking(&mut self, delta: f64, i: &Intents, confirm: bool) {
        let any = |keys| i.events.iter().any(|e| pressed(e, keys, &[]));
        let pad_any = |pads| i.events.iter().any(|e| pressed(e, &[], pads));
        let sel = self.fork_sel;
        if any(bind::MENU_LEFT) || pad_any(bind::PAD_LEFT) {
            self.fork_sel = 0;
        }
        if any(bind::MENU_RIGHT) || pad_any(bind::PAD_RIGHT) {
            self.fork_sel = 1;
        }
        if self.fork_sel != sel {
            self.sfx.push(Sfx::Click);
        }
        if !self.fork_ready() {
            return;
        }
        let pick = if let Some(d) = &mut self.demo {
            d.fork_wait -= delta;
            (d.fork_wait <= 0.0).then(|| (mix64(self.seed ^ self.clock.to_bits()) & 1) as u8)
        } else if self.touch.pressed(Action::PickEasy) {
            Some(0)
        } else if self.touch.pressed(Action::PickHard) {
            Some(1)
        } else if confirm {
            Some(self.fork_sel)
        } else {
            None
        };
        if let Some(b) = pick {
            self.sfx.push(Sfx::Click);
            if let Some(run) = &mut self.run {
                run.fork(b, &mut self.cache);
                self.begin_race();
            }
        }
    }
}

/// What a bot's input would look like held on the pad.
fn held_of(i: Input) -> Held {
    Held {
        left: i.steer_level() < 0,
        right: i.steer_level() > 0,
        brake: i.brake(),
        drift: i.drift(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{Inline, Key};
    use crate::input::Key as K;
    use racer_sim::track::fixtures;

    /// A builder that hands out the drag strip with `allot` seconds per
    /// section.
    fn game(allot: f64) -> Game {
        Game::new(Box::new(Inline::new(move |_| {
            let mut t = fixtures::drag_strip();
            let n = t.checkpoints().len();
            t.set_allotments(vec![racer_sim::ticks(allot); n]);
            t
        })))
    }

    /// A builder that hands out two gates' worth of straight road.
    fn two_gates() -> Game {
        Game::new(Box::new(Inline::new(|_| {
            let mut b = racer_sim::track::Builder::new();
            b.straight(60)
                .checkpoint()
                .straight(60)
                .checkpoint()
                .straight(20);
            let mut t = b.build();
            t.set_allotments(vec![racer_sim::ticks(30.0); 2]);
            t
        })))
    }

    fn sounds(fx: &[Effect]) -> Vec<Sfx> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Sound(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    fn loops(fx: &[Effect]) -> Option<Loops> {
        fx.iter().rev().find_map(|e| match e {
            Effect::Loops(l) => Some(*l),
            _ => None,
        })
    }

    fn frame(g: &mut Game, events: Vec<Event>, keys: Vec<K>) -> Vec<Effect> {
        let i = Intents {
            events,
            keys_held: keys,
            window: (1080, 2400),
            entropy: 42,
            ..Intents::default()
        };
        g.tick(1.0 / 60.0, &i)
    }

    fn start(g: &mut Game) {
        frame(g, vec![Event::Key(K::Enter)], vec![]);
        assert_eq!(g.mode, Mode::Building);
        frame(g, vec![], vec![]);
        frame(g, vec![], vec![]);
        assert_eq!(g.mode, Mode::Racing);
    }

    #[test]
    fn the_first_frame_sizes_the_buffer_from_the_window() {
        let mut g = game(30.0);
        let fx = frame(&mut g, vec![], vec![]);
        assert!(fx.contains(&Effect::Buffer { w: 180, h: 400 }));
        assert_eq!(g.layout.h, 400);
        let fx = frame(&mut g, vec![], vec![]);
        assert!(!fx.iter().any(|e| matches!(e, Effect::Buffer { .. })));
    }

    #[test]
    fn title_builds_then_races_and_the_car_moves_on_auto_throttle() {
        let mut g = game(30.0);
        start(&mut g);
        for _ in 0..60 {
            frame(&mut g, vec![], vec![K::Right]);
        }
        let race = g.run.as_ref().unwrap().race();
        assert!(race.car().z > 5.0, "auto-throttle moved the car");
        assert!(race.car().x > 0.0, "steered right");
        assert!(g.held.right && !g.held.left);
        // About a second of frames is about 120 ticks.
        assert!((115..=125).contains(&race.tick()), "{}", race.tick());
    }

    #[test]
    fn focus_loss_pauses_and_back_toggles_pause() {
        let mut g = game(30.0);
        start(&mut g);
        frame(&mut g, vec![], vec![]);
        g.focus_lost();
        assert!(g.paused);
        let tick = g.run.as_ref().unwrap().race().tick();
        frame(&mut g, vec![], vec![]);
        assert_eq!(g.run.as_ref().unwrap().race().tick(), tick, "paused");
        frame(&mut g, vec![Event::Back], vec![]);
        assert!(!g.paused);
        frame(&mut g, vec![Event::Back], vec![]);
        assert!(g.paused);
        frame(&mut g, vec![Event::Key(K::Enter)], vec![]);
        assert!(!g.paused, "confirm resumes");
        frame(&mut g, vec![Event::Key(K::Escape)], vec![]);
        assert!(g.paused);
    }

    #[test]
    fn back_on_the_title_quits() {
        let mut g = game(30.0);
        let fx = frame(&mut g, vec![Event::Back], vec![]);
        assert!(fx.contains(&Effect::Quit));
    }

    #[test]
    fn a_finished_track_forks_and_a_pick_starts_the_next() {
        let mut g = game(60.0);
        start(&mut g);
        let mut n = 0;
        while g.mode == Mode::Racing && n < 60 * 60 {
            frame(&mut g, vec![], vec![]);
            n += 1;
        }
        assert_eq!(g.mode, Mode::Fork);
        assert!(g.fork_ready(), "both sides were built while racing");
        frame(&mut g, vec![Event::Key(K::Right)], vec![]);
        assert_eq!(g.fork_sel, 1);
        frame(&mut g, vec![Event::Key(K::Enter)], vec![]);
        assert_eq!(g.mode, Mode::Racing);
        let run = g.run.as_ref().unwrap();
        assert_eq!((run.index(), run.branch()), (1, 1));
    }

    #[test]
    fn running_out_of_time_ends_the_run_on_the_results_screen() {
        let mut g = game(2.0);
        start(&mut g);
        for _ in 0..200 {
            frame(&mut g, vec![], vec![]);
        }
        assert_eq!(g.mode, Mode::Results);
        assert_eq!(g.run.as_ref().unwrap().phase(), RunPhase::Lost);
        frame(&mut g, vec![Event::Key(K::Space)], vec![]);
        assert_eq!(g.mode, Mode::Title);
    }

    #[test]
    fn touch_buttons_drive_like_keys() {
        use crate::layout::Layout;
        let mut g = game(30.0);
        start(&mut g);
        let l = Layout::new(400);
        g.set_buttons(vec![Button {
            rect: l.left,
            action: Action::Left,
        }]);
        let (x, y) = ((l.left.x + 5) as f32, (l.left.y + 5) as f32);
        frame(&mut g, vec![Event::FingerDown { index: 0, x, y }], vec![]);
        for _ in 0..30 {
            frame(&mut g, vec![], vec![]);
        }
        assert!(g.held.left);
        assert!(g.run.as_ref().unwrap().race().car().x < 0.0);
        frame(&mut g, vec![Event::FingerUp { index: 0 }], vec![]);
        assert!(!g.held.left);
    }

    #[test]
    fn the_demo_drives_by_itself_and_any_input_ends_it() {
        let mut g = game(60.0);
        g.set_buttons(vec![Button {
            rect: crate::layout::Rect::new(0, 0, 180, 400),
            action: Action::Demo,
        }]);
        frame(
            &mut g,
            vec![Event::FingerDown {
                index: 0,
                x: 5.0,
                y: 5.0,
            }],
            vec![],
        );
        frame(&mut g, vec![Event::FingerUp { index: 0 }], vec![]);
        frame(&mut g, vec![], vec![]);
        assert_eq!(g.mode, Mode::Racing);
        assert!(g.demo());
        for _ in 0..120 {
            frame(&mut g, vec![], vec![]);
        }
        assert!(g.run.as_ref().unwrap().race().car().z > 20.0);
        frame(&mut g, vec![Event::Key(K::Left)], vec![]);
        assert_eq!(g.mode, Mode::Title);
        assert!(!g.demo());
    }

    #[test]
    fn each_gate_chimes_exactly_once() {
        let mut g = two_gates();
        start(&mut g);
        let mut heard = Vec::new();
        let mut n = 0;
        while g.mode == Mode::Racing && n < 60 * 60 {
            let fx = frame(&mut g, vec![], vec![]);
            let s = sounds(&fx);
            assert!(s.iter().filter(|s| **s == Sfx::Chime).count() <= 1);
            heard.extend(s);
            n += 1;
        }
        assert_eq!(g.mode, Mode::Fork);
        assert_eq!(
            heard,
            vec![Sfx::Chime, Sfx::Chime],
            "one per gate, nothing else"
        );
        let fx = frame(&mut g, vec![Event::Key(K::Enter)], vec![]);
        assert_eq!(sounds(&fx), vec![Sfx::Click], "a fork pick clicks");
    }

    #[test]
    fn the_title_has_music_and_racing_has_the_engine_instead() {
        let mut g = game(30.0);
        let fx = frame(&mut g, vec![], vec![]);
        assert!(fx.contains(&Effect::Music(true)));
        let fx = frame(&mut g, vec![Event::Key(K::Enter)], vec![]);
        assert_eq!(sounds(&fx), vec![Sfx::Click]);
        let mut fx = frame(&mut g, vec![], vec![]);
        fx.extend(frame(&mut g, vec![], vec![]));
        assert_eq!(g.mode, Mode::Racing);
        assert!(fx.contains(&Effect::Music(false)));
        let started = loops(&fx).expect("the engine starts with the race");
        assert!(started.engine_volume > 0 && started.drift_volume == 0);
        let mut last = started;
        for _ in 0..120 {
            if let Some(l) = loops(&frame(&mut g, vec![], vec![])) {
                last = l;
            }
        }
        assert!(last.engine_pitch > started.engine_pitch, "it revs up");
        assert!(last.engine_volume > started.engine_volume);
    }

    #[test]
    fn pausing_fades_the_loops_out_and_resuming_brings_them_back() {
        let mut g = game(30.0);
        start(&mut g);
        for _ in 0..30 {
            frame(&mut g, vec![], vec![]);
        }
        let fx = frame(&mut g, vec![Event::Key(K::Escape)], vec![]);
        assert!(g.paused);
        assert_eq!(sounds(&fx), vec![Sfx::Click]);
        let mut last = None;
        for _ in 0..15 {
            if let Some(l) = loops(&frame(&mut g, vec![], vec![])) {
                last = Some(l);
            }
        }
        assert!(last.unwrap().silent(), "faded out within a quarter second");
        assert!((0..60).all(|_| loops(&frame(&mut g, vec![], vec![])).is_none()));
        frame(&mut g, vec![Event::Key(K::Enter)], vec![]);
        let back = (0..10).find_map(|_| loops(&frame(&mut g, vec![], vec![])));
        assert!(back.unwrap().engine_volume > 0);
        // Losing focus silences at once.
        let fx = g.focus_lost();
        assert!(loops(&fx).unwrap().silent());
    }

    #[test]
    fn running_out_of_time_ticks_then_stings_and_the_title_music_returns() {
        let mut g = game(3.0);
        start(&mut g);
        let mut heard = Vec::new();
        let mut music = Vec::new();
        for _ in 0..300 {
            let fx = frame(&mut g, vec![], vec![]);
            heard.extend(sounds(&fx));
            music.extend(fx.iter().filter(|e| matches!(e, Effect::Music(_))).cloned());
        }
        assert_eq!(g.mode, Mode::Results);
        assert_eq!(heard, vec![Sfx::Tick, Sfx::Tick, Sfx::TimeUp]);
        assert!(music.is_empty(), "no music on the results screen");
        assert!(g.mixer.sent().silent(), "the engine stopped");
        let fx = frame(&mut g, vec![Event::Key(K::Space)], vec![]);
        assert_eq!(sounds(&fx), vec![Sfx::Click]);
        assert!(fx.contains(&Effect::Music(true)));
    }

    /// Track 1 is two gates of straight road with time to spare; every
    /// later track runs out of time at once. One finish, then the results.
    fn one_finish() -> Game {
        Game::new(Box::new(Inline::new(|k: Key| {
            let mut b = racer_sim::track::Builder::new();
            b.straight(60)
                .checkpoint()
                .straight(60)
                .checkpoint()
                .straight(20);
            let mut t = b.build();
            let allot = if k.1 == 0 { 30.0 } else { 1.0 };
            t.set_allotments(vec![racer_sim::ticks(allot); 2]);
            t
        })))
    }

    fn ghosts_saved(fx: &[Effect]) -> Option<String> {
        fx.iter().rev().find_map(|e| match e {
            Effect::SaveGhosts { text } => Some(text.clone()),
            _ => None,
        })
    }

    fn run_names(fx: &[Effect]) -> Vec<String> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::SaveRun { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Weave and brake for the first second, then straight: a slower drive
    /// than a ghost that went straight all the way.
    fn weave(n: usize) -> Vec<K> {
        match n {
            0..=20 => vec![K::Right],
            21..=45 => vec![K::Left],
            46..=60 => vec![K::Down],
            _ => vec![],
        }
    }

    /// Finish track 1 (keeping its ghost), run out of time on track 2 and
    /// return the effects.
    fn finish_then_lose(g: &mut Game, keys: impl Fn(usize) -> Vec<K>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut n = 0;
        while g.mode == Mode::Racing {
            fx.extend(frame(g, vec![], keys(n)));
            n += 1;
        }
        assert_eq!(g.mode, Mode::Fork);
        fx.extend(frame(g, vec![Event::Key(K::Enter)], vec![]));
        while g.mode == Mode::Racing {
            fx.extend(frame(g, vec![], vec![]));
        }
        assert_eq!(g.mode, Mode::Results);
        fx
    }

    /// RETRY SEED from the results, by keys.
    fn retry(g: &mut Game) {
        frame(g, vec![Event::Key(K::Up)], vec![]);
        assert_eq!(g.results_sel, 1);
        frame(g, vec![Event::Key(K::Enter)], vec![]);
        frame(g, vec![], vec![]);
        assert_eq!(g.mode, Mode::Racing, "the track is already built");
    }

    #[test]
    fn a_finish_is_kept_as_the_tracks_ghost_and_only_a_faster_one_replaces_it() {
        let mut g = one_finish();
        start(&mut g);
        let fx = finish_then_lose(&mut g, weave);
        let text = ghosts_saved(&fx).expect("the finish was kept");
        assert!(text.starts_with("racer-ghosts 1\ng 1 "), "{text}");
        let key = (g.seed, 0, 0);
        let first = g.ghosts().get(key).unwrap().ticks;
        assert_eq!(first, g.run.as_ref().unwrap().results()[0].ticks);
        assert_eq!(g.ghosts().len(), 1, "a lost track keeps nothing");
        // The same weave again: a tie, nothing written.
        retry(&mut g);
        let fx = finish_then_lose(&mut g, weave);
        assert_eq!(ghosts_saved(&fx), None);
        // Straight down the road is quicker: replaced. Retry by touch.
        g.set_buttons(vec![Button {
            rect: crate::layout::Rect::new(0, 0, 180, 40),
            action: Action::Retry,
        }]);
        let tap = Event::FingerDown {
            index: 0,
            x: 5.0,
            y: 5.0,
        };
        frame(&mut g, vec![tap], vec![]);
        frame(&mut g, vec![Event::FingerUp { index: 0 }], vec![]);
        assert_eq!(g.mode, Mode::Racing);
        let fx = finish_then_lose(&mut g, |_| vec![]);
        assert!(ghosts_saved(&fx).is_some());
        assert!(g.ghosts().get(key).unwrap().ticks < first);
        assert_eq!(
            run_names(&fx),
            vec![format!("run-{:016x}-2.txt", g.seed)],
            "a retry's run file does not overwrite the first attempt's"
        );
    }

    #[test]
    fn retry_seed_races_the_same_tracks_again() {
        let mut g = game(2.0);
        start(&mut g);
        let seed = g.seed;
        let first = g.run.as_ref().unwrap().race().track().clone();
        while g.mode == Mode::Racing {
            frame(&mut g, vec![], vec![]);
        }
        assert_eq!(g.mode, Mode::Results);
        retry(&mut g);
        assert_eq!(g.seed, seed);
        let run = g.run.as_ref().unwrap();
        assert_eq!(run.run_seed(), seed);
        assert!(
            Arc::ptr_eq(run.race().track(), &first),
            "the very track, from the cache"
        );
        assert!(g.ghost().is_none(), "nothing was finished: no ghost");
        // And with the real generator: the same seed builds the same track.
        use racer_sim::run::{Generate, Tracks};
        let a = Generate.track(seed, 0, 0);
        let b = Generate.track(seed, 0, 0);
        assert_eq!(*a, *b);
    }

    #[test]
    fn the_ghost_races_its_best_beside_the_player_and_reports_each_gate() {
        let mut g = one_finish();
        start(&mut g);
        finish_then_lose(&mut g, |_| vec![]);
        let best = g.ghosts().get((g.seed, 0, 0)).unwrap().clone();
        retry(&mut g);
        assert!(g.ghost().is_some(), "a stored best: a ghost");
        // Slower this time: the ghost pulls ahead.
        let mut n = 0;
        let mut notes = Vec::new();
        let mut ghost_done_at = None;
        while g.mode == Mode::Racing {
            frame(&mut g, vec![], weave(n));
            n += 1;
            let race = g.run.as_ref().unwrap().race();
            if let (Some(gh), true) = (g.ghost(), race.tick() > 120) {
                assert!(gh.car().z > race.car().z, "the faster ghost leads");
            }
            if g.ghost().is_none() && ghost_done_at.is_none() {
                ghost_done_at = Some(race.tick());
            }
            if let Some(d) = g.ghost_note() {
                notes.push(d);
            }
        }
        // It finished on its stored tick (seen at the end of that frame),
        // and then was gone.
        let done = ghost_done_at.expect("the ghost finished first");
        assert!(
            done >= best.ticks && done < best.ticks + 3,
            "{done} vs {}",
            best.ticks
        );
        assert!(!notes.is_empty(), "the first gate's delta was shown");
        assert!(notes.iter().all(|&d| d > 0), "behind the ghost: {notes:?}");
        // The note is the player's split minus the ghost's.
        let run = g.run.as_ref().unwrap();
        let (probe, _) = racer_sim::replay::replay_race(
            run.race().track().clone(),
            best.loadout.params(),
            &best.log,
        )
        .unwrap();
        let want = i64::from(run.race().splits()[0]) - i64::from(probe.splits()[0]);
        assert_eq!(notes[0], want);
    }

    #[test]
    fn a_ghost_never_changes_the_players_run() {
        use racer_sim::run::state_hash;
        let mut first = one_finish();
        start(&mut first);
        let fx = finish_then_lose(&mut first, |_| vec![]);
        let text = ghosts_saved(&fx).unwrap();
        let mut with = one_finish();
        let mut without = one_finish();
        with.load_ghosts(&text);
        start(&mut with);
        start(&mut without);
        assert_eq!(with.seed, first.seed);
        assert!(with.ghost().is_some() && without.ghost().is_none());
        let mut n = 0;
        while with.mode == Mode::Racing {
            frame(&mut with, vec![], weave(n));
            frame(&mut without, vec![], weave(n));
            n += 1;
            let (a, b) = (with.run.as_ref().unwrap(), without.run.as_ref().unwrap());
            assert_eq!(state_hash(a), state_hash(b), "frame {n}");
        }
        assert_eq!(without.mode, Mode::Fork);
    }

    #[test]
    fn demo_neither_keeps_nor_races_ghosts() {
        // The ghost of a straight drive of the DEMO's own first track.
        let mut probe = one_finish();
        start(&mut probe);
        let text = ghosts_saved(&finish_then_lose(&mut probe, |_| vec![])).unwrap();
        let mut g = one_finish();
        g.load_ghosts(&text);
        g.set_buttons(vec![Button {
            rect: crate::layout::Rect::new(0, 0, 180, 400),
            action: Action::Demo,
        }]);
        let down = Event::FingerDown {
            index: 0,
            x: 5.0,
            y: 5.0,
        };
        let mut fx = frame(&mut g, vec![down], vec![]);
        fx.extend(frame(&mut g, vec![Event::FingerUp { index: 0 }], vec![]));
        assert!(g.demo() && g.mode == Mode::Racing);
        assert_eq!(g.seed, probe.seed);
        assert!(g.ghost().is_none());
        while g.mode == Mode::Racing {
            fx.extend(frame(&mut g, vec![], vec![]));
        }
        assert_eq!(ghosts_saved(&fx), None);
    }

    #[test]
    fn the_demo_is_silent() {
        let mut g = game(60.0);
        frame(&mut g, vec![], vec![]);
        g.set_buttons(vec![Button {
            rect: crate::layout::Rect::new(0, 0, 180, 400),
            action: Action::Demo,
        }]);
        let down = Event::FingerDown {
            index: 0,
            x: 5.0,
            y: 5.0,
        };
        let mut fx = frame(&mut g, vec![down], vec![]);
        fx.extend(frame(&mut g, vec![Event::FingerUp { index: 0 }], vec![]));
        assert!(g.demo());
        // Through a whole track, the fork and into the next.
        for _ in 0..60 * 60 {
            fx.extend(frame(&mut g, vec![], vec![]));
        }
        assert!(g.run.as_ref().unwrap().index() >= 1, "DEMO reached a fork");
        assert!(sounds(&fx).is_empty(), "{:?}", sounds(&fx));
        assert!(!fx.contains(&Effect::Music(true)));
        assert!(fx
            .iter()
            .all(|e| !matches!(e, Effect::Loops(l) if !l.silent())));
        assert!(fx.contains(&Effect::Music(false)), "the title music stops");
    }
}
