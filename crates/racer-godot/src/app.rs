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

use std::collections::BTreeSet;
use std::sync::Arc;

use racer_sim::bot::human::{HumanBot, HumanPlan};
use racer_sim::bot::Driver;
use racer_sim::gen::difficulty::TRACKS;
use racer_sim::input::Input;
use racer_sim::rng::{mix64, substream};
use racer_sim::run::{RunPhase, RunState, TrackCache};
use racer_sim::track::{fixtures, Track};
use racer_sim::DT;

use crate::builder::{Build, Key};
use crate::input::{bind, pressed, Controls, Event, Intents};
use crate::layout::{buffer_size, Layout, H_MIN};
use crate::render::{Decor, Held};
use crate::touch::{Action, Button, TouchPad};

/// The longest frame the clock accepts, seconds. A longer gap (a hitch, a
/// debugger) runs as this much, so the sim never races to catch up.
pub const MAX_FRAME: f64 = 0.1;

/// How far the background scrolls per radian the road turns, pixels.
pub const BG_PX_PER_RAD: f64 = 150.0;

/// How long DEMO looks at a fork before picking, seconds.
pub const DEMO_FORK_WAIT: f64 = 2.0;

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
    demo: Option<Demo>,
    window: (i32, i32),
    fx: Vec<Effect>,
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
            demo: None,
            window: (0, 0),
            fx: Vec::new(),
        }
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
        self.clock += delta;
        if i.window != self.window && i.window.0 > 0 {
            self.window = i.window;
            let (w, h) = buffer_size(i.window);
            self.layout = Layout::new(h);
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
                return std::mem::take(&mut self.fx);
            }
        }
        if back {
            self.back();
        }
        match self.mode {
            Mode::Title => {
                if confirm || self.touch.pressed(Action::Start) {
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
            Mode::Results => {
                if confirm || self.touch.pressed(Action::Continue) {
                    self.go_to_title();
                }
            }
        }
        std::mem::take(&mut self.fx)
    }

    /// The window lost focus or the app was sent to the background: let go
    /// of every finger and pause a race.
    pub fn focus_lost(&mut self) -> Vec<Effect> {
        self.touch.release_all();
        if self.mode == Mode::Racing && !self.paused && self.demo.is_none() {
            self.paused = true;
            self.fx.push(Effect::KeepScreenOn(false));
        }
        self.fx.push(Effect::Redraw);
        std::mem::take(&mut self.fx)
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
        self.touch.release_all();
        self.fx.push(Effect::KeepScreenOn(!paused));
        self.fx.push(Effect::Redraw);
    }

    fn go_to_title(&mut self) {
        self.mode = Mode::Title;
        self.paused = false;
        self.demo = None;
        self.run = None;
        self.touch.release_all();
        self.held = Held::default();
        self.fx.push(Effect::KeepScreenOn(false));
        self.fx.push(Effect::Redraw);
    }

    fn start(&mut self, entropy: u64, demo: bool) {
        self.seed = mix64(entropy).max(1);
        self.builder.request((self.seed, 0, 0));
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
        self.decor = Decor::new(&track);
        if index + 1 < TRACKS {
            for b in 0..2 {
                self.builder.request((self.seed, index + 1, b));
            }
        }
        if let Some(d) = &mut self.demo {
            let plan = HumanPlan::new(&track, run.race().params());
            d.bot = Some(HumanBot::new(plan, substream(self.seed, index as u64)));
            d.fork_wait = DEMO_FORK_WAIT;
        }
        self.mode = Mode::Racing;
        self.paused = false;
        self.acc = 0.0;
        self.touch.release_all();
        self.fx.push(Effect::KeepScreenOn(true));
        self.fx.push(Effect::Redraw);
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
                None => player,
            };
            run.step(input);
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
        match run.phase() {
            RunPhase::Racing => {}
            RunPhase::Forking => {
                self.mode = Mode::Fork;
                self.fork_sel = 0;
                self.touch.release_all();
                self.fx.push(Effect::KeepScreenOn(false));
            }
            RunPhase::Won | RunPhase::Lost => {
                self.mode = Mode::Results;
                self.touch.release_all();
                self.fx.push(Effect::KeepScreenOn(false));
            }
        }
    }

    fn forking(&mut self, delta: f64, i: &Intents, confirm: bool) {
        let any = |keys| i.events.iter().any(|e| pressed(e, keys, &[]));
        let pad_any = |pads| i.events.iter().any(|e| pressed(e, &[], pads));
        if any(bind::MENU_LEFT) || pad_any(bind::PAD_LEFT) {
            self.fork_sel = 0;
        }
        if any(bind::MENU_RIGHT) || pad_any(bind::PAD_RIGHT) {
            self.fork_sel = 1;
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
    use crate::builder::Inline;
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
}
