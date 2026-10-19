//! The one Godot node: the adapter between the engine and the engine-free
//! game (`app`), as in Salt Line.
//!
//! ```text
//!   input():  key/pad down ──► Event      on_notification():
//!             finger       ──► Event        focus out / paused ──► Game::focus_lost
//!   back gesture           ──► Event::Back
//!   process(delta)
//!     ├─ gather   queued events + keys/pads held + stick ──► Intents
//!     ├─ Game::tick(delta, Intents) ──► effects applied in order
//!     │    (sounds: one AudioStreamPlayer per one-shot, the engine and
//!     │    tyre loops, the title music; all from art/audio, see audio.rs)
//!     ├─ frame cap: menus idle at frame::IDLE_FPS
//!     └─ Redraw::due(busy)? ──► screens::frame ──► Canvas ──► one ImageTexture
//!   draw(): the texture at the origin, 1:1; the viewport's canvas_items/keep
//!           stretch scales the 180-wide buffer to the window and letterboxes
//! ```
//!
//! Presses are events (a key tapped inside one frame still counts once);
//! holds are polled. Every binding is in `input.rs`, not project.godot.

use godot::classes::image::Format;
use godot::classes::notify::CanvasItemNotification;
use godot::classes::{
    AudioStream, AudioStreamPlayer, DisplayServer, Engine, INode2D, Image, ImageTexture, Input,
    InputEvent, InputEventJoypadButton, InputEventKey, InputEventScreenDrag, InputEventScreenTouch,
    Node2D, Os, ProjectSettings, ResourceLoader, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::obj::EngineEnum;
use godot::prelude::*;

use crate::app::{Effect, Game};
use crate::audio::{Loops, Sfx, Tune, AMBIENT_FILE, DRIFT_FILE, ENGINE_FILE};
use crate::builder::{generate, worker_count, Worker};
use crate::frame::{frame_cap, tighter_cap, Redraw};
use crate::input::{self, Event, Intents};
use crate::raster::Canvas;
use crate::screens;
use crate::sprites::Art;
use crate::track_store::TrackStore;

const KEYS: [(input::Key, Key); 13] = [
    (input::Key::Left, Key::LEFT),
    (input::Key::Right, Key::RIGHT),
    (input::Key::Up, Key::UP),
    (input::Key::Down, Key::DOWN),
    (input::Key::A, Key::A),
    (input::Key::D, Key::D),
    (input::Key::W, Key::W),
    (input::Key::S, Key::S),
    (input::Key::Space, Key::SPACE),
    (input::Key::Shift, Key::SHIFT),
    (input::Key::Enter, Key::ENTER),
    (input::Key::Escape, Key::ESCAPE),
    (input::Key::P, Key::P),
];

const PADS: [(input::Pad, JoyButton); 12] = [
    (input::Pad::Left, JoyButton::DPAD_LEFT),
    (input::Pad::Right, JoyButton::DPAD_RIGHT),
    (input::Pad::Up, JoyButton::DPAD_UP),
    (input::Pad::Down, JoyButton::DPAD_DOWN),
    (input::Pad::South, JoyButton::A),
    (input::Pad::East, JoyButton::B),
    (input::Pad::West, JoyButton::X),
    (input::Pad::North, JoyButton::Y),
    (input::Pad::L1, JoyButton::LEFT_SHOULDER),
    (input::Pad::R1, JoyButton::RIGHT_SHOULDER),
    (input::Pad::Start, JoyButton::START),
    (input::Pad::Back, JoyButton::BACK),
];

/// A trigger past this reads as pressed.
const TRIGGER_PULL: f32 = 0.5;

/// The title music sits under the effects.
const MUSIC_DB: f32 = -7.0;

/// The players: one per one-shot (in `Sfx::ALL` order), the three loops
/// and one per title tune (in `Tune::ALL` order). `None` where the file
/// failed to load: that sound is silent.
#[derive(Default)]
struct Players {
    sfx: Vec<Option<Gd<AudioStreamPlayer>>>,
    engine: Option<Gd<AudioStreamPlayer>>,
    drift: Option<Gd<AudioStreamPlayer>>,
    ambient: Option<Gd<AudioStreamPlayer>>,
    music: Vec<Option<Gd<AudioStreamPlayer>>>,
}

/// A linear volume in thousandths, in decibels.
fn thousandths_db(v: u16) -> f32 {
    if v == 0 {
        -80.0
    } else {
        (20.0 * (f32::from(v) / 1000.0).log10()).max(-80.0)
    }
}

/// Start, retune or stop a loop: silent is stopped.
fn set_loop(player: &mut Option<Gd<AudioStreamPlayer>>, volume: u16, pitch: Option<u16>) {
    let Some(p) = player else { return };
    if volume == 0 {
        if p.is_playing() {
            p.stop();
        }
        return;
    }
    p.set_volume_db(thousandths_db(volume));
    if let Some(pitch) = pitch {
        p.set_pitch_scale((f32::from(pitch) / 1000.0).max(0.05));
    }
    if !p.is_playing() {
        p.play();
    }
}

/// The game node.
#[derive(GodotClass)]
#[class(base = Node2D)]
pub struct Racer {
    base: Base<Node2D>,
    g: Game,
    art: Art,
    canvas: Canvas,
    events: Vec<Event>,
    intents: Intents,
    redraw: Redraw,
    image: Option<Gd<Image>>,
    texture: Option<Gd<ImageTexture>>,
    project_cap: i32,
    /// The window size the bottom inset was read for, and the inset: read
    /// again only when the window changes (it crosses into Java).
    inset_for: (i32, i32),
    inset: i32,
    /// The platform's cap from its power state (0 is none), set by
    /// `godot/platform_services.gd` from the `DevicePower` autoload.
    platform_cap: i32,
    /// Where finished runs are kept for playtest analysis: the app's
    /// external files directory on Android (adb can read it), `user://runs`
    /// elsewhere. Set by `godot/platform_services.gd`.
    runs_dir: String,
    cap: i32,
    audio: Players,
    #[cfg(feature = "profiling")]
    profile: crate::profiling::Profile,
}

#[godot_api]
impl INode2D for Racer {
    fn init(base: Base<Node2D>) -> Self {
        Racer {
            base,
            g: Game::new(Box::new(Worker::with(
                worker_count(),
                TrackStore::new(tracks_dir()).source(generate()),
            ))),
            art: Art::new(),
            canvas: Canvas::new(crate::layout::W, crate::layout::H_MIN),
            events: Vec::new(),
            intents: Intents::default(),
            redraw: Redraw::default(),
            image: None,
            texture: None,
            project_cap: 0,
            inset_for: (0, 0),
            inset: 0,
            platform_cap: 0,
            runs_dir: String::new(),
            cap: -1,
            audio: Players::default(),
            #[cfg(feature = "profiling")]
            profile: crate::profiling::Profile::default(),
        }
    }

    fn ready(&mut self) {
        self.project_cap = Engine::singleton().get_max_fps();
        if let Some(text) = load_ghosts() {
            self.g.load_ghosts(&text);
            godot_print!("racer: {} ghosts loaded", self.g.ghosts().len());
        }
        if let Ok(text) = std::fs::read_to_string(user_path(crate::daily::DAILY_FILE)) {
            self.g.load_daily(&text);
        }
        let slots = load_layouts_file(crate::editor::store::SLOTS_FILE);
        let draft = load_layouts_file(crate::editor::store::DRAFT_FILE);
        self.g.load_layouts(slots.as_deref(), draft.as_deref());
        self.setup_audio();
        self.dev_flags();
        godot_print!(
            "racer: sim epoch {} ({:#018x})",
            racer_sim::SIM_EPOCH,
            racer_sim::EPOCH_DIGEST
        );
    }

    fn process(&mut self, delta: f64) {
        #[cfg(feature = "profiling")]
        let started = std::time::Instant::now();
        self.gather();
        let fx = self.g.tick(delta, &self.intents);
        self.apply(fx);
        let cap = frame_cap(
            self.g.on_menu(),
            tighter_cap(self.project_cap, self.platform_cap),
        );
        if cap != self.cap {
            Engine::singleton().set_max_fps(cap);
            self.cap = cap;
        }
        if self.redraw.due(self.g.busy()) {
            #[cfg(feature = "profiling")]
            let raster = std::time::Instant::now();
            let buttons = screens::frame(&mut self.g, &mut self.canvas, &self.art);
            self.g.set_buttons(buttons);
            #[cfg(feature = "profiling")]
            self.profile.raster(raster.elapsed());
            self.upload();
        }
        #[cfg(feature = "profiling")]
        for line in self.profile.frame(delta, started.elapsed(), &self.g) {
            godot_print!("{line}");
        }
    }

    fn draw(&mut self) {
        if let Some(t) = self.texture.clone() {
            self.base_mut().draw_texture(&t, Vector2::ZERO);
        }
    }

    fn input(&mut self, event: Gd<InputEvent>) {
        let event = match event.try_cast::<InputEventKey>() {
            Ok(key) => {
                if key.is_pressed() && !key.is_echo() {
                    let code = key.get_keycode();
                    if let Some((k, _)) = KEYS.iter().find(|(_, g)| *g == code) {
                        self.events.push(Event::Key(*k));
                    }
                    // Letters and digits are also typed: names, seeds and
                    // the editor's shortcuts.
                    if let Some(c) = typed(code) {
                        self.events.push(Event::Key(input::Key::Char(c)));
                    } else if code == Key::BACKSPACE {
                        self.events.push(Event::Key(input::Key::Backspace));
                    }
                }
                return;
            }
            Err(e) => e,
        };
        let event = match event.try_cast::<InputEventJoypadButton>() {
            Ok(b) => {
                if b.is_pressed() {
                    let idx = b.get_button_index();
                    if let Some((p, _)) = PADS.iter().find(|(_, g)| *g == idx) {
                        self.events.push(Event::Pad(*p));
                    }
                }
                return;
            }
            Err(e) => e,
        };
        let event = match event.try_cast::<InputEventScreenDrag>() {
            Ok(drag) => {
                let pos = self.local(drag.clone().upcast(), drag.get_position());
                self.events.push(Event::FingerMoved {
                    index: drag.get_index(),
                    x: pos.x,
                    y: pos.y,
                });
                return;
            }
            Err(e) => e,
        };
        if let Ok(touch) = event.try_cast::<InputEventScreenTouch>() {
            let index = touch.get_index();
            if touch.is_pressed() {
                let pos = self.local(touch.clone().upcast(), touch.get_position());
                self.events.push(Event::FingerDown {
                    index,
                    x: pos.x,
                    y: pos.y,
                });
            } else {
                self.events.push(Event::FingerUp { index });
            }
        }
    }

    fn on_notification(&mut self, what: CanvasItemNotification) {
        match what {
            CanvasItemNotification::WM_GO_BACK_REQUEST => {
                self.events.push(Event::Back);
            }
            CanvasItemNotification::WM_WINDOW_FOCUS_OUT
            | CanvasItemNotification::APPLICATION_FOCUS_OUT
            | CanvasItemNotification::APPLICATION_PAUSED => {
                self.events.clear();
                let fx = self.g.focus_lost();
                self.apply(fx);
            }
            CanvasItemNotification::WM_WINDOW_FOCUS_IN
            | CanvasItemNotification::APPLICATION_FOCUS_IN
            | CanvasItemNotification::APPLICATION_RESUMED => self.redraw.mark(),
            _ => {}
        }
    }
}

/// The character a key types, for [`input::Key::Char`]: `A`-`Z`, `0`-`9`.
fn typed(code: Key) -> Option<u8> {
    let o = code.ord();
    if (Key::A.ord()..=Key::Z.ord()).contains(&o) {
        Some(b'A' + (o - Key::A.ord()) as u8)
    } else if (Key::KEY_0.ord()..=Key::KEY_9.ord()).contains(&o) {
        Some(b'0' + (o - Key::KEY_0.ord()) as u8)
    } else if (Key::KP_0.ord()..=Key::KP_9.ord()).contains(&o) {
        Some(b'0' + (o - Key::KP_0.ord()) as u8)
    } else {
        None
    }
}

/// `user://tracks` as a filesystem path: the built tracks kept between
/// launches (`track_store`), app-private, read and written by the builder
/// threads (which never call the engine: the path is resolved here).
fn tracks_dir() -> std::path::PathBuf {
    ProjectSettings::singleton()
        .globalize_path("user://tracks")
        .to_string()
        .into()
}

/// `user://layouts/<name>` as a filesystem path: the slots file and the
/// draft (DESIGN §9.11), app-private like the ghosts.
fn layouts_path(name: &str) -> std::path::PathBuf {
    let dir = ProjectSettings::singleton()
        .globalize_path("user://layouts")
        .to_string();
    std::path::Path::new(&dir).join(name)
}

/// Read a layouts file, if there is one. A file being replaced when the
/// app died (only the temporary copy is whole) is read from that copy.
fn load_layouts_file(name: &str) -> Option<String> {
    let path = layouts_path(name);
    std::fs::read_to_string(&path)
        .ok()
        .or_else(|| std::fs::read_to_string(path.with_extension("tmp")).ok())
}

/// Write (or with `None`, delete) a layouts file: to a temporary file, then
/// renamed over the old one, so a crash mid-write leaves the last good
/// file. Returns whether it worked; a failure is logged.
fn store_layouts_file(name: &str, text: Option<&str>) -> bool {
    let path = layouts_path(name);
    let done = match text {
        Some(text) => {
            let tmp = path.with_extension("tmp");
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&tmp, text))
                .and_then(|()| std::fs::rename(&tmp, &path))
        }
        None => {
            // A stale temporary copy would be read back as the file.
            let _ = std::fs::remove_file(path.with_extension("tmp"));
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    };
    if let Err(e) = &done {
        godot_warn!("racer: could not write {}: {e}", path.display());
    }
    done.is_ok()
}

/// `user://<name>` as a filesystem path.
fn user_path(name: &str) -> std::path::PathBuf {
    let dir = ProjectSettings::singleton()
        .globalize_path("user://")
        .to_string();
    std::path::Path::new(&dir).join(name)
}

/// Write `text` to a temporary file beside `path`, then rename it over
/// `path`, so a crash mid-write leaves the last good file.
fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, text))
        .and_then(|()| std::fs::rename(&tmp, path))
}

/// `user://ghosts/ghosts.txt` as a filesystem path: the app's private data
/// (the best per track is the player's, not playtest data, so it stays out
/// of the runs directory).
fn ghosts_path() -> std::path::PathBuf {
    let dir = ProjectSettings::singleton()
        .globalize_path("user://ghosts")
        .to_string();
    std::path::Path::new(&dir).join(crate::app::GHOSTS_FILE)
}

/// The stored ghosts' text, if there is a file.
fn load_ghosts() -> Option<String> {
    std::fs::read_to_string(ghosts_path()).ok()
}

/// Write the ghosts' text: to a temporary file, then renamed over the old
/// one, so a crash mid-write leaves the last good file. A failure is
/// logged, never fatal.
fn save_ghosts(text: &str) {
    let path = ghosts_path();
    let tmp = path.with_extension("tmp");
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, text))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        godot_warn!("racer: could not save {}: {e}", path.display());
    }
}

/// Density-independent pixels Android's gesture navigation keeps at the
/// bottom of the screen (Pixel 10: 84 px at 420 dpi).
const GESTURE_DP: i32 = 32;

/// Screen pixels at the bottom of a `win` window that touches must avoid.
/// On a phone, the larger of the display's safe area (a cutout or bar at
/// the bottom) and the gesture area, which the safe area does not cover; on
/// a desktop, none.
fn bottom_inset_px(win: (i32, i32)) -> i32 {
    if !Os::singleton().has_feature("mobile") {
        return 0;
    }
    let ds = DisplayServer::singleton();
    let safe = ds.get_display_safe_area();
    let safe_gap = if safe.size.y > 0 {
        (win.1 - safe.position.y - safe.size.y).clamp(0, win.1 / 4)
    } else {
        0
    };
    let gesture = GESTURE_DP * ds.screen_get_dpi().max(160) / 160;
    safe_gap.max(gesture)
}

#[godot_api]
impl Racer {
    /// The platform's frame cap from its power state (0 is none): see the
    /// `device_power` addon. The tighter of it and the project's cap is used
    /// while racing; menus still drop to `frame::IDLE_FPS`.
    /// The directory to keep finished runs in (an absolute path).
    #[func]
    fn set_runs_dir(&mut self, dir: GString) {
        self.runs_dir = dir.to_string();
    }

    #[func]
    fn set_platform_fps_cap(&mut self, cap: i32) {
        if cap == self.platform_cap {
            return;
        }
        self.platform_cap = cap;
        godot_print!(
            "racer: platform frame cap {cap}, race cap {}",
            tighter_cap(self.project_cap, cap)
        );
    }
}

impl Racer {
    /// Act on the developer flags (`cli.rs`): `--racer-season` forces a
    /// season on or off; `--racer-layout` starts a
    /// custom race on a code or a layout file. The layout is verified
    /// (proven again, tens of milliseconds) before it races timed.
    fn dev_flags(&mut self) {
        let os = Os::singleton();
        let args: Vec<String> = os
            .get_cmdline_args()
            .as_slice()
            .iter()
            .chain(os.get_cmdline_user_args().as_slice())
            .map(|s| s.to_string())
            .collect();
        let dev = crate::cli::DevArgs::parse(&args);
        if let Some(season) = dev.season {
            godot_print!("racer: --racer-season: {season:?}");
            self.g.forced_season = Some(season);
        }
        let Some(arg) = dev.layout else { return };
        let path = ProjectSettings::singleton()
            .globalize_path(&arg)
            .to_string();
        let text = std::fs::read_to_string(&path).unwrap_or(arg);
        match crate::custom::load(&text, true) {
            Ok((c, note)) => {
                godot_print!("racer: --racer-layout: {note}");
                self.g.start_custom(c);
            }
            Err(why) => {
                godot_warn!("racer: --racer-layout: {why}");
            }
        }
    }

    /// A player for `res://art/audio/<file>.wav`, a child of the node, at
    /// `db`. Imported loops loop by themselves: the generator writes their
    /// loop points into the file (tools/gen_audio.py).
    fn player(&mut self, file: &str, db: f32) -> Option<Gd<AudioStreamPlayer>> {
        let path = format!("res://art/audio/{file}.wav");
        let Some(stream) = ResourceLoader::singleton()
            .load(&path)
            .and_then(|r| r.try_cast::<AudioStream>().ok())
        else {
            godot_warn!("racer: could not load sound {path}");
            return None;
        };
        let mut p = AudioStreamPlayer::new_alloc();
        p.set_stream(&stream);
        p.set_volume_db(db);
        self.base_mut().add_child(&p);
        Some(p)
    }

    fn setup_audio(&mut self) {
        let sfx = Sfx::ALL
            .iter()
            .map(|s| self.player(s.file(), 0.0))
            .collect();
        self.audio = Players {
            sfx,
            engine: self.player(ENGINE_FILE, -80.0),
            drift: self.player(DRIFT_FILE, -80.0),
            ambient: self.player(AMBIENT_FILE, -80.0),
            music: Tune::ALL
                .iter()
                .map(|t| self.player(t.file(), MUSIC_DB))
                .collect(),
        };
    }

    /// Write a run file. A failure is logged, never fatal: playtest data is
    /// a nicety, the game goes on.
    fn save_run(&self, name: &str, text: &str) {
        let dir = if self.runs_dir.is_empty() {
            ProjectSettings::singleton()
                .globalize_path("user://runs")
                .to_string()
        } else {
            self.runs_dir.clone()
        };
        let path = std::path::Path::new(&dir).join(name);
        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)) {
            Ok(()) => {
                godot_print!("racer: run saved to {}", path.display());
            }
            Err(e) => {
                godot_warn!("racer: could not save {}: {e}", path.display());
            }
        }
    }

    /// An event's position in the node's space: buffer pixels, after the
    /// window stretch and letterbox.
    fn local(&self, event: Gd<InputEvent>, fallback: Vector2) -> Vector2 {
        let local = self.base().make_input_local(&event);
        if let Ok(t) = local.clone().try_cast::<InputEventScreenTouch>() {
            return t.get_position();
        }
        if let Ok(d) = local.try_cast::<InputEventScreenDrag>() {
            return d.get_position();
        }
        fallback
    }

    /// This frame's intents: the queued events and what is held now.
    fn gather(&mut self) {
        let input = Input::singleton();
        let mut i = std::mem::take(&mut self.intents);
        i.clear();
        i.events.append(&mut self.events);
        i.keys_held.extend(
            KEYS.iter()
                .filter(|(_, k)| input.is_key_pressed(*k))
                .map(|(c, _)| *c),
        );
        let mut stick = 0.0f32;
        for dev in input.get_connected_joypads().iter_shared() {
            let dev = dev as i32;
            for (c, b) in PADS {
                if input.is_joy_button_pressed(dev, b) && !i.pads_held.contains(&c) {
                    i.pads_held.push(c);
                }
            }
            if input.get_joy_axis(dev, JoyAxis::TRIGGER_LEFT) > TRIGGER_PULL {
                i.pads_held.push(input::Pad::L2);
            }
            if input.get_joy_axis(dev, JoyAxis::TRIGGER_RIGHT) > TRIGGER_PULL {
                i.pads_held.push(input::Pad::R2);
            }
            let x = input.get_joy_axis(dev, JoyAxis::LEFT_X);
            if x.abs() > stick.abs() {
                stick = x;
            }
        }
        i.stick_x = stick;
        let size = DisplayServer::singleton().window_get_size();
        i.window = (size.x, size.y);
        if i.window != self.inset_for {
            self.inset_for = i.window;
            self.inset = bottom_inset_px(i.window);
        }
        i.bottom_inset = self.inset;
        let t = Time::singleton();
        let now = t.get_unix_time_from_system();
        i.entropy = (now * 1e6) as u64 ^ t.get_ticks_usec();
        i.today = Some(crate::daily::day_of(now));
        if !i.events.is_empty() {
            self.redraw.mark();
        }
        self.intents = i;
    }

    fn apply(&mut self, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Redraw => self.redraw.mark(),
                Effect::Buffer { w, h } => {
                    self.canvas.resize(w, h);
                    if let Some(mut window) = self.base().get_window() {
                        window.set_content_scale_size(Vector2i::new(w, h));
                    }
                    self.redraw.mark();
                }
                Effect::KeepScreenOn(on) => DisplayServer::singleton().screen_set_keep_on(on),
                Effect::Quit => self.base().get_tree().quit(),
                Effect::SaveRun { name, text } => self.save_run(&name, &text),
                Effect::SaveGhosts { text } => save_ghosts(&text),
                Effect::SaveDaily { text } => {
                    let path = user_path(crate::daily::DAILY_FILE);
                    if let Err(e) = write_atomic(&path, &text) {
                        godot_warn!("racer: could not save {}: {e}", path.display());
                    }
                }
                Effect::Store { file, text } => {
                    let ok = store_layouts_file(file.name(), text.as_deref());
                    self.g.stored(file, ok);
                }
                Effect::CopyText(text) => DisplayServer::singleton().clipboard_set(&text),
                Effect::ReadClipboard => {
                    let text = DisplayServer::singleton().clipboard_get().to_string();
                    let fx = self.g.pasted(&text);
                    self.apply(fx);
                }
                Effect::Sound(sfx) => {
                    if let Some(p) = self
                        .audio
                        .sfx
                        .get_mut(sfx as usize)
                        .and_then(Option::as_mut)
                    {
                        p.play();
                    }
                }
                Effect::Loops(Loops {
                    engine_pitch,
                    engine_volume,
                    drift_volume,
                    ambient_volume,
                }) => {
                    set_loop(&mut self.audio.engine, engine_volume, Some(engine_pitch));
                    set_loop(&mut self.audio.drift, drift_volume, None);
                    set_loop(&mut self.audio.ambient, ambient_volume, None);
                }
                Effect::Music(tune) => {
                    for (t, p) in Tune::ALL.iter().zip(self.audio.music.iter_mut()) {
                        let Some(p) = p else { continue };
                        if Some(*t) == tune {
                            if !p.is_playing() {
                                p.play();
                            }
                        } else {
                            p.stop();
                        }
                    }
                }
            }
        }
    }

    /// Hand the canvas to the GPU: the image updated in place when the size
    /// is unchanged, made anew when it is not.
    fn upload(&mut self) {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let bytes = PackedByteArray::from(self.canvas.bytes());
        let same = self
            .image
            .as_ref()
            .is_some_and(|i| i.get_width() == w && i.get_height() == h);
        if same {
            if let (Some(image), Some(texture)) = (self.image.as_mut(), self.texture.as_mut()) {
                image.set_data(w, h, false, Format::RGBA8, &bytes);
                texture.update(&*image);
            }
        } else if let Some(image) = Image::create_from_data(w, h, false, Format::RGBA8, &bytes) {
            self.texture = ImageTexture::create_from_image(&image);
            self.image = Some(image);
        }
        self.base_mut().queue_redraw();
    }
}
