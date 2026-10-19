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
//! DAILY on the title (DESIGN §1.4, `daily`) is today's run, the same for
//! everyone: its seed is the UTC day's, which the adapter hands in with
//! each frame's [`Intents`]. As soon as the day is known (the first frame)
//! the whole of its run, track 1 and both sides of every fork, is asked of
//! the builder in the background ([`Build::prefetch`]), so DAILY starts at
//! once and its forks never wait. Today's tries and best are a
//! [`DailyRecord`], kept as `daily.txt` through [`Effect::SaveDaily`]; a
//! daily run's file says `daily <date>`. RETRY on its results is another
//! try of the same daily.
//!
//! A custom race (DESIGN §9.6, `custom.rs`) is a run of one layout's track,
//! started by [`Game::start_custom`]: no forks, no builder, timed only when
//! the layout is proven, its ghost kept under the layout's digest. RETRY on
//! its results races the same layout again.
//!
//! The editor and the TRACKS list (DESIGN §9.11, `editor/`) are one more
//! mode, [`Mode::Editor`]: the title's TRACKS opens it, FORK THIS TRACK on
//! the pause menu and the results opens it on the track just raced, and a
//! TEST DRIVE (a custom race marked `test`) comes back to it. Its files and
//! the clipboard go out as effects ([`Effect::Store`], [`Effect::CopyText`],
//! [`Effect::ReadClipboard`]); proving and FROM SEED's track go to the
//! builder thread. DEMO never reaches it.
//!
//! Sound is decided here too (`audio`): one-shots from what changed between
//! sim ticks, the engine and tyre loops from the car each frame, the title
//! music from the mode. DEMO asks for none of it.

use std::collections::BTreeSet;
use std::sync::Arc;

use racer_sim::biome::Biome;
use racer_sim::bot::human::{HumanBot, HumanPlan};
use racer_sim::bot::Driver;
use racer_sim::ghost::{Ghost, GhostRun, GhostStore, TrackKey};
use racer_sim::input::Input;
use racer_sim::layout::{Provenance, CUSTOM_LOADOUT};
use racer_sim::replay::{encode_run, Ending, Replay, RunFile};
use racer_sim::rng::{mix64, substream};
use racer_sim::run::{RunPhase, RunState, TrackCache};
use racer_sim::track::{fixtures, Track};
use racer_sim::DT;

use crate::audio::{loop_targets, Loops, Mixer, Sfx, Watch};
use crate::builder::{Build, Key};
use crate::custom::Custom;
use crate::daily::{self, DailyRecord, Score};
use crate::editor::store::StoreFile;
use crate::editor::{Act, Editor, Out};
use crate::input::{bind, pressed, Controls, Event, Intents, SteerRamp};
use crate::layout::{buffer_size, inset_rows, Layout, H_MIN};
use crate::render::{Decor, Held};
use crate::season::{Look, Season};
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
    /// The TRACKS list and the editor (`editor`).
    Editor,
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
    /// Write a layouts file (`user://layouts/<file>`) atomically, or delete
    /// it (`None`), then report with [`Game::stored`]. Never for DEMO.
    Store {
        file: StoreFile,
        text: Option<String>,
    },
    /// Put this text on the system clipboard.
    CopyText(String),
    /// Read the system clipboard and hand it to [`Game::pasted`].
    ReadClipboard,
    /// Today's daily record changed: write `text` ([`DailyRecord::encode`])
    /// as [`daily::DAILY_FILE`] in the user data directory. Never for DEMO.
    SaveDaily { text: String },
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
    /// The biome of the track on screen (DESIGN §5.1): the track index's,
    /// or a custom race's layout's.
    pub biome: Biome,
    /// The track behind the title screen, and its decor.
    pub title_track: Arc<Track>,
    /// Decor of the title track, per biome ([`Biome::ALL`] order): the
    /// title shows each in turn.
    pub title_decor: Vec<Decor>,
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
    /// The layout being raced, in a custom race.
    pub custom: Option<Custom>,
    /// The TRACKS list and the editor.
    pub editor: Editor,
    /// Today, in UTC days since the Unix epoch, once the adapter has said.
    pub today: Option<u64>,
    /// The day of the daily run being raced; `None` for any other run.
    pub daily: Option<u64>,
    /// Today's daily tries and best.
    pub daily_record: DailyRecord,
    /// A season forced on or off by `--racer-season` (DESIGN §5.2),
    /// whatever today is.
    pub forced_season: Option<Season>,
}

/// Seconds the title shows each biome before the next.
pub const TITLE_BIOME_SECS: f64 = 6.0;

impl Game {
    /// The biome behind the title screen now: each in turn, from the meadow.
    pub fn title_biome(&self) -> Biome {
        Biome::ALL[(self.clock / TITLE_BIOME_SECS) as usize % Biome::ALL.len()]
    }

    /// The season now (DESIGN §5.2): the flag's, else today's (the daily's
    /// UTC day), else none. Presentation only: nothing simulated, stored
    /// or shared reads it.
    pub fn season(&self) -> Season {
        Season::resolve(self.forced_season, self.today)
    }

    /// How biome `b` is painted now: its twin in a season.
    pub fn look(&self, b: Biome) -> Look {
        Look::of(b, self.season())
    }

    /// A presentation clock for the scenery's animation (bats), 30 steps
    /// a second from launch: never the sim's tick.
    pub fn anim(&self) -> u32 {
        (self.clock * 30.0) as u32
    }

    /// A game on its title screen, building tracks with `builder`.
    pub fn new(builder: Box<dyn Build>) -> Game {
        let title_track = Arc::new(fixtures::sweep_and_hairpin());
        let title_decor = Biome::ALL
            .iter()
            .map(|&b| Decor::new(&title_track, b))
            .collect();
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
            biome: Biome::Meadow,
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
            custom: None,
            editor: Editor::new(),
            today: None,
            daily: None,
            daily_record: DailyRecord::default(),
            forced_season: None,
        }
    }

    /// Read the daily record ([`daily::DAILY_FILE`]'s text) at launch.
    pub fn load_daily(&mut self, text: &str) {
        self.daily_record = DailyRecord::decode(text);
    }

    /// Today's best daily run, if today is known and has one.
    pub fn daily_best(&self) -> Option<Score> {
        self.daily_record.on(self.today?).best
    }

    /// Ask the builder, in the background, for every track of `day`'s
    /// daily run that is not built yet.
    fn prebuild_daily(&mut self, day: u64) {
        for key in daily::keys(daily::daily_seed(day)) {
            if !self.built(key) {
                self.builder.prefetch(key);
            }
        }
    }

    /// Start a try of `day`'s daily run.
    fn start_daily(&mut self, day: u64) {
        self.attempt = self.daily_record.start(day);
        self.fx.push(Effect::SaveDaily {
            text: self.daily_record.encode(),
        });
        self.start_seed(daily::daily_seed(day), false);
        self.daily = Some(day);
    }

    /// The daily run on screen ended: keep it if it is the day's best.
    fn daily_ended(&mut self) {
        let (Some(day), Some(run), None) = (self.daily, &self.run, &self.demo) else {
            return;
        };
        let lost = usize::from(run.phase() == RunPhase::Lost);
        let cleared = &run.results()[..run.results().len().saturating_sub(lost)];
        let score = Score {
            cleared: cleared.len() as u32,
            ticks: cleared.iter().map(|r| r.ticks).sum(),
        };
        if self.daily_record.offer(day, score) {
            self.fx.push(Effect::SaveDaily {
                text: self.daily_record.encode(),
            });
        }
    }

    /// Read the layouts files at launch (`None` where there is none):
    /// the slots and the draft (`editor::store`).
    pub fn load_layouts(&mut self, slots: Option<&str>, draft: Option<&str>) {
        self.editor.load(slots, draft);
    }

    /// A layouts file was written (`ok`) or not, after [`Effect::Store`].
    pub fn stored(&mut self, file: StoreFile, ok: bool) {
        if !ok {
            self.editor.sticky = Some(match file {
                StoreFile::Slots => "COULD NOT SAVE THE SLOTS".into(),
                StoreFile::Draft => "COULD NOT KEEP THE DRAFT".into(),
            });
            self.fx.push(Effect::Redraw);
        }
    }

    /// The clipboard's text, after [`Effect::ReadClipboard`].
    pub fn pasted(&mut self, text: &str) -> Vec<Effect> {
        if self.mode == Mode::Editor && self.demo.is_none() {
            let out = self.editor.pasted(text, self.builder.as_mut());
            self.carry_out(out);
        }
        self.fx.push(Effect::Redraw);
        std::mem::take(&mut self.fx)
    }

    /// Do what the editor asked.
    fn carry_out(&mut self, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Store(file, text) => self.fx.push(Effect::Store { file, text }),
                Out::Copy(text) => self.fx.push(Effect::CopyText(text)),
                Out::Paste => self.fx.push(Effect::ReadClipboard),
                Out::Race(c) => self.start_custom(*c),
                Out::Title => self.go_to_title(),
                Out::Build(key) => {
                    if let Some(t) = self.track(key) {
                        let more = self.editor.forked(key, &t, self.builder.as_mut());
                        self.carry_out(more);
                    } else {
                        self.request(key);
                    }
                }
                Out::Click => self.sfx.push(Sfx::Click),
            }
        }
        self.fx.push(Effect::Redraw);
    }

    /// Open the TRACKS list.
    fn open_tracks(&mut self) {
        self.go_to_title();
        self.mode = Mode::Editor;
        self.editor.open_tracks(self.builder.as_mut());
    }

    /// FORK THIS TRACK: the track on screen (or the layout raced) goes to
    /// the editor as the draft.
    fn fork_this(&mut self) {
        let Some(run) = &self.run else { return };
        let track = run.race().track().clone();
        let (seed, index, branch) = (run.run_seed(), run.index(), run.branch());
        let out = match self.custom.clone() {
            Some(c) => self.editor.fork_layout(&c, self.builder.as_mut()),
            None => {
                let origin = Provenance {
                    run_seed: seed,
                    index,
                    branch,
                    epoch: racer_sim::SIM_EPOCH,
                };
                self.editor
                    .fork_track(&track, Some(origin.to_string()), self.builder.as_mut())
            }
        };
        self.go_to_title();
        self.mode = Mode::Editor;
        self.carry_out(out);
    }

    /// Back to the editor after a test drive.
    fn back_to_editor(&mut self) {
        self.go_to_title();
        self.mode = Mode::Editor;
        self.editor.open_edit();
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
            Mode::Editor => self.editor.animating(),
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
        let music = !quiet && matches!(self.mode, Mode::Title | Mode::Building | Mode::Editor);
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
        if let Some(day) = i.today.filter(|d| self.today != Some(*d)) {
            self.today = Some(day);
            self.prebuild_daily(day);
            self.fx.push(Effect::Redraw);
        }
        self.editor.tick(self.clock);
        for (key, track) in self.builder.poll() {
            self.cache.insert(key.0, key.1, key.2, track.clone());
            self.ready.insert(key);
            self.fx.push(Effect::Redraw);
            // FROM SEED's track: the draft, kept on disk (the editor may
            // have been left meanwhile; it opens on the fork next time).
            if self.editor.pending_fork == Some(key) {
                let out = self.editor.forked(key, &track, self.builder.as_mut());
                self.carry_out(out);
            }
        }
        let proofs = self.builder.proofs();
        if !proofs.is_empty() {
            let out = self.editor.proofs(proofs);
            self.carry_out(out);
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
                let typed = |c| i.events.contains(&Event::Key(crate::input::Key::Char(c)));
                let tracks = typed(b'T');
                let daily = self.touch.pressed(Action::Daily)
                    || typed(b'D')
                    || i.events.iter().any(|e| pressed(e, &[], bind::PAD_DAILY));
                if confirm || self.touch.pressed(Action::Start) {
                    self.sfx.push(Sfx::Click);
                    self.start(i.entropy, false);
                } else if let Some(day) = self.today.filter(|_| daily) {
                    self.sfx.push(Sfx::Click);
                    self.start_daily(day);
                } else if self.touch.pressed(Action::Tracks) || tracks {
                    self.sfx.push(Sfx::Click);
                    self.open_tracks();
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
            Mode::Editor => self.editing(i, back),
        }
    }

    /// A frame in the editor: its buttons, keys, pad and the map's touches.
    fn editing(&mut self, i: &Intents, back: bool) {
        let mut acts: Vec<Act> = self
            .touch
            .presses()
            .iter()
            .filter_map(|a| match a {
                Action::Edit(a) if !matches!(a, Act::Map | Act::Strip) => Some(*a),
                _ => None,
            })
            .collect();
        for e in &i.events {
            match *e {
                Event::FingerDown { index, x, y } | Event::FingerMoved { index, x, y } => {
                    if let Some(Action::Edit(on @ (Act::Map | Act::Strip))) =
                        self.touch.finger(index)
                    {
                        self.editor.touch(on, x, y, &self.layout);
                    }
                }
                Event::Key(_) | Event::Pad(_) => acts.extend(self.editor.key(e)),
                _ => {}
            }
        }
        if back {
            acts.push(Act::Back);
        }
        for a in acts {
            if self.mode != Mode::Editor {
                break;
            }
            let out = self.editor.act(a, self.builder.as_mut(), self.clock);
            self.carry_out(out);
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

    /// Whether the results and pause offer FORK THIS TRACK: a player's race
    /// (not DEMO's), not a test drive of the draft itself.
    pub fn can_fork(&self) -> bool {
        self.demo.is_none() && !self.custom.as_ref().is_some_and(|c| c.test)
    }

    fn results(&mut self, i: &Intents, confirm: bool) {
        let any = |keys: &[_], pads: &[_]| i.events.iter().any(|e| pressed(e, keys, pads));
        let sel = self.results_sel;
        let top = if self.can_fork() { 2 } else { 1 };
        if any(bind::MENU_UP, bind::PAD_UP) {
            self.results_sel = (self.results_sel + 1).min(top);
        }
        if any(bind::MENU_DOWN, bind::PAD_DOWN) {
            self.results_sel = self.results_sel.saturating_sub(1);
        }
        if self.results_sel != sel {
            self.sfx.push(Sfx::Click);
        }
        let fork = self.touch.pressed(Action::ForkTrack) || (confirm && self.results_sel == 2);
        if fork && self.can_fork() {
            self.fork_this();
            return;
        }
        let retry = self.touch.pressed(Action::Retry) || (confirm && self.results_sel == 1);
        if retry && self.demo.is_none() {
            self.sfx.push(Sfx::Click);
            let (seed, attempt, daily) = (self.seed, self.attempt + 1, self.daily);
            match (self.custom.take(), daily) {
                (Some(c), _) => {
                    self.start_custom(c);
                    self.attempt = attempt;
                }
                // Another try of the same day's daily, counted as one.
                (None, Some(day)) => {
                    self.go_to_title();
                    self.start_daily(day);
                }
                (None, None) => {
                    self.go_to_title();
                    self.start_seed(seed, false);
                    self.attempt = attempt;
                }
            }
        } else if confirm || self.touch.pressed(Action::Continue) {
            self.sfx.push(Sfx::Click);
            if self.custom.as_ref().is_some_and(|c| c.test) {
                self.back_to_editor();
            } else {
                self.go_to_title();
            }
        }
    }

    fn back(&mut self) {
        match self.mode {
            Mode::Title => self.fx.push(Effect::Quit),
            Mode::Racing => self.set_paused(!self.paused),
            Mode::Editor => {}
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
        // A custom race carries its layout as raced: with the proof whose
        // timers it raced under, or none for a free drive (a version 2 run
        // file, DESIGN §9.6).
        let layout = self.custom.as_ref().and_then(|c| {
            let raced = if c.timed {
                c.layout.clone()
            } else {
                c.layout.unproven()
            };
            raced.to_code().ok()
        });
        let kind = if layout.is_some() {
            "run-layout"
        } else {
            "run"
        };
        let f = RunFile {
            epoch: racer_sim::SIM_EPOCH,
            ending,
            replay: Replay::of(run),
            layout,
            daily: self.daily.map(daily::date),
        };
        let name = match self.attempt {
            0 => format!("{kind}-{:016x}.txt", f.replay.run_seed),
            n => format!("{kind}-{:016x}-{n}.txt", f.replay.run_seed),
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
        self.custom = None;
        self.daily = None;
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

    /// Race `c`'s layout alone, at once: a custom race (DESIGN §9.6).
    pub fn start_custom(&mut self, c: Custom) {
        self.go_to_title();
        self.attempt = 0;
        self.seed = c.layout.hash();
        self.run = Some(RunState::single(self.seed, c.track.clone(), CUSTOM_LOADOUT));
        self.custom = Some(c);
        self.begin_race();
    }

    /// The ghost store's key for the track being raced.
    fn track_key(&self) -> Option<TrackKey> {
        let run = self.run.as_ref()?;
        Some(match &self.custom {
            Some(c) => c.ghost_key(),
            None => (self.seed, run.index(), run.branch()),
        })
    }

    fn begin_race(&mut self) {
        let Some(key) = self.track_key() else { return };
        let Some(run) = &self.run else { return };
        let index = run.index();
        let track = run.race().track().clone();
        self.watch = Some(Watch::new(run.race()));
        self.biome = match &self.custom {
            Some(c) => c.layout.biome,
            None => Biome::of_track(index),
        };
        self.decor = Decor::new(&track, self.biome);
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
        if index + 1 < run.tracks() {
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
        let Some(key) = self.track_key() else { return };
        let (Some(run), None) = (&self.run, &self.demo) else {
            return;
        };
        let Some(g) = Ghost::of(run.race(), run.loadout()) else {
            return;
        };
        if self.ghosts.offer(key, g) {
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
                if self.custom.as_ref().is_some_and(|c| c.test) {
                    self.back_to_editor();
                } else {
                    self.go_to_title();
                }
            } else if self.touch.pressed(Action::ForkTrack) && self.can_fork() {
                self.save_run(Ending::Quit);
                self.fork_this();
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
                self.daily_ended();
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

    #[test]
    fn a_custom_race_is_one_track_free_unless_proven_with_its_own_ghost() {
        use racer_sim::layout::Layout;
        let mut g = game(30.0);
        frame(&mut g, vec![], vec![]);
        // An unproven layout: free drive, no timer, one track.
        let draft = Layout::from_track(&fixtures::drag_strip());
        let (c, _) = crate::custom::load(&draft.to_code().unwrap(), false).unwrap();
        assert!(!c.timed);
        let key = c.ghost_key();
        g.start_custom(c);
        assert_eq!(g.mode, Mode::Racing);
        assert!(g
            .run
            .as_ref()
            .unwrap()
            .race()
            .track()
            .allot_ticks()
            .is_empty());
        let mut fx = Vec::new();
        for _ in 0..2000 {
            fx.extend(frame(&mut g, vec![], vec![]));
            if g.mode != Mode::Racing {
                break;
            }
        }
        assert_eq!(g.mode, Mode::Results, "finished the strip");
        // Saved as a version 2 run file that replays from its layout alone.
        let saved: Vec<_> = fx
            .iter()
            .filter_map(|e| match e {
                Effect::SaveRun { name, text } => Some((name.clone(), text.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].0, format!("run-layout-{:016x}.txt", key.0));
        let f = racer_sim::replay::decode_run(&saved[0].1).unwrap();
        let l = Layout::from_code(f.layout.as_deref().unwrap()).unwrap();
        let back = f
            .replay
            .reconstruct_single(Arc::new(l.raced_track().unwrap()), CUSTOM_LOADOUT, |_| {})
            .unwrap();
        assert_eq!(
            racer_sim::run::state_hash(&back),
            racer_sim::run::state_hash(g.run.as_ref().unwrap())
        );
        assert_eq!(g.run.as_ref().unwrap().phase(), RunPhase::Won);
        assert!(g.ghosts().get(key).is_some(), "kept under the layout's key");
        // RETRY COURSE races it again, with its ghost.
        g.results_sel = 1;
        frame(&mut g, vec![Event::Key(K::Enter)], vec![]);
        assert_eq!(g.mode, Mode::Racing);
        assert!(g.custom.is_some() && g.ghost().is_some());
        // A proven layout races timed.
        let course = racer_sim::layout::bundled::layouts().remove(0);
        let (c, _) = crate::custom::load(&course.to_code().unwrap(), false).unwrap();
        g.start_custom(c);
        let race = g.run.as_ref().unwrap().race();
        assert_eq!(race.timer(), course.proof.unwrap().allot_ticks[0]);
    }
}
