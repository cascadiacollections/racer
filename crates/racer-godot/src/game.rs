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
    Node2D, Os, ProjectSettings, RenderingServer, ResourceLoader, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::obj::EngineEnum;
use godot::prelude::*;

use crate::app::{Effect, Game, Mode};
use crate::audio::{Loops, Sfx, Tune, AMBIENT_FILE, DRIFT_FILE, ENGINE_FILE};
use crate::builder::{generate, worker_count, Worker};
use crate::daily::DailyRecord;
use crate::frame::{frame_cap, tighter_cap, Redraw};
use crate::input::{self, Event, Intents};
use crate::perf;
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
    /// A damaged file could not be moved aside, so nothing may be written
    /// over the save paths until the next launch.
    hold_saves: bool,
    cap: i32,
    /// The root viewport renders direct to the screen, which Godot draws
    /// upside down, so the texture is drawn mirrored (`direct_to_screen`).
    flip_y: bool,
    audio: Players,
    #[cfg(feature = "profiling")]
    profile: crate::profiling::Profile,
    /// When this frame's `process` began, to time the frame (`perf`).
    frame_started: Option<std::time::Instant>,
    device_reads: perf::DeviceReads,
    /// The debug frame-time overlay (`perf::Hud`); absent from release
    /// builds, and until `ready`.
    #[cfg(debug_assertions)]
    hud: Option<perf::Hud>,
    /// Show the overlay: on a phone, or with `--racer-perf` or
    /// `--racer-bench`.
    #[cfg(debug_assertions)]
    hud_shown: bool,
    /// `--racer-bench`: the soak benchmark, while it runs.
    bench: Option<perf::Bench>,
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
            flip_y: false,
            audio: Players::default(),
            #[cfg(feature = "profiling")]
            profile: crate::profiling::Profile::default(),
            frame_started: None,
            device_reads: perf::DeviceReads::default(),
            #[cfg(debug_assertions)]
            hud: None,
            #[cfg(debug_assertions)]
            hud_shown: false,
            bench: None,
            hold_saves: false,
        }
    }

    fn ready(&mut self) {
        self.project_cap = Engine::singleton().get_max_fps();
        self.direct_to_screen();
        self.load_saved_ghosts();
        self.load_saved_daily();
        if crate::persist::keep_aside_damaged_runs(&self.runs_directory()) > 0 {
            self.g.note_damaged_save();
        }
        let slots = load_layouts_file(crate::editor::store::SLOTS_FILE);
        let draft = load_layouts_file(crate::editor::store::DRAFT_FILE);
        self.g.load_layouts(slots.as_deref(), draft.as_deref());
        self.g.modern_touch = std::fs::read_to_string(user_path("controls.txt"))
            .map_or(true, |s| s.trim() != "buttons");
        self.setup_audio();
        self.dev_flags();
        godot_print!(
            "racer: sim epoch {} ({:#018x})",
            racer_sim::SIM_EPOCH,
            racer_sim::EPOCH_DIGEST
        );
    }

    fn process(&mut self, delta: f64) {
        let started = std::time::Instant::now();
        let frame_ms = self
            .frame_started
            .replace(started)
            .map(|then| (started - then).as_secs_f64() * 1000.0);
        if self.bench.is_some() && matches!(self.g.mode, Mode::Title | Mode::Results) {
            // The benchmark drives the same seed again whenever a run ends.
            self.g.start_seed(perf::BENCH_SEED, true);
        }
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
        let rendered = self.redraw.due(self.g.busy());
        if rendered {
            #[cfg(feature = "profiling")]
            let raster = std::time::Instant::now();
            let buttons = screens::frame(&mut self.g, &mut self.canvas, &self.art);
            self.g.set_buttons(buttons);
            #[cfg(feature = "profiling")]
            self.profile.raster(raster.elapsed());
            #[cfg(feature = "profiling")]
            let upload = std::time::Instant::now();
            self.upload();
            #[cfg(feature = "profiling")]
            self.profile.upload(upload.elapsed());
        }
        #[cfg(feature = "profiling")]
        for line in self.profile.frame(delta, started.elapsed(), &self.g) {
            godot_print!("{line}");
        }
        if let Some(frame_ms) = frame_ms {
            self.measure(frame_ms, started.elapsed(), rendered, delta);
        }
    }

    fn exit_tree(&mut self) {
        self.end_bench("closed");
        // Stop every voice while the audio driver is still running. Godot
        // schedules a fade-out on the mixer thread rather than releasing
        // playback synchronously (godotengine/godot#95484).
        for player in self
            .audio
            .sfx
            .iter_mut()
            .chain(self.audio.music.iter_mut())
            .chain([
                &mut self.audio.engine,
                &mut self.audio.drift,
                &mut self.audio.ambient,
            ])
            .flatten()
        {
            player.stop();
            player.set_stream(Gd::<AudioStream>::null_arg());
        }
        // Give the mixer a short grace period to consume the stops before
        // SceneTree teardown shuts the driver down. stop() alone still
        // leaked the active title WAV on Godot 4.7.2 / Windows ARM64.
        Os::singleton().delay_msec(100);
    }

    fn draw(&mut self) {
        let Some(t) = self.texture.clone() else {
            return;
        };
        if self.flip_y {
            // Mirror the drawing (not the node, so input is unchanged): row
            // y lands on row h - y.
            let h = t.get_height() as f32;
            self.base_mut()
                .draw_set_transform_ex(Vector2::new(0.0, h))
                .scale(Vector2::new(1.0, -1.0))
                .done();
        }
        self.base_mut().draw_texture(&t, Vector2::ZERO);
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
            if touch.is_pressed() && !touch.is_canceled() {
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
                if what == CanvasItemNotification::APPLICATION_PAUSED {
                    self.end_bench("paused");
                }
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

impl Racer {
    /// One frame's timing, for the overlay and the benchmark (`perf`). The
    /// monitors read here (draw calls, video memory) describe the frame the
    /// renderer last finished, one behind the one just processed.
    fn measure(&mut self, frame_ms: f64, process: std::time::Duration, rendered: bool, delta: f64) {
        #[cfg(not(debug_assertions))]
        if self.bench.is_none() {
            return;
        }
        let mut sample = perf::Sample::read(frame_ms, process.as_secs_f64() * 1000.0, rendered);
        self.device_reads.fill(&mut sample);
        #[cfg(debug_assertions)]
        if let Some(hud) = self.hud.as_mut() {
            hud.update(self.hud_shown, &sample, delta);
        }
        #[cfg(not(debug_assertions))]
        let _ = delta;
        if let Some(bench) = self.bench.as_mut() {
            bench.record(sample);
            if bench.finish_if_due() {
                self.end_bench("done");
                // iOS ignores `SceneTree::quit`; the file is written, so the
                // process can go.
                if cfg!(target_os = "ios") {
                    std::process::exit(0);
                }
                self.base().get_tree().quit();
            }
        }
    }

    /// Write the benchmark's frames, if one is running, and stop it. `why`
    /// names the file: `done` for a full run, `paused` or `closed` for one
    /// cut short, which is still worth reading.
    fn end_bench(&mut self, why: &str) {
        let Some(bench) = self.bench.take() else {
            return;
        };
        self.g.fixed_forks = false;
        godot_print!("{}", bench.summary_line());
        let stamp = Time::singleton().get_unix_time_from_system() as u64;
        let path = user_path(&format!("bench/bench-{stamp}-{why}.csv"));
        match write_atomic(&path, &bench.csv()) {
            Ok(()) => {
                godot_print!("racer-bench: wrote {}", path.display());
            }
            Err(e) => {
                godot_warn!("racer-bench: could not write {}: {e}", path.display());
            }
        }
    }
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

/// Screen pixels at the bottom of a `win` window that touches must avoid
/// (`layout::bottom_inset_px`); on a desktop, none.
fn bottom_inset_px(win: (i32, i32)) -> i32 {
    let os = Os::singleton();
    if !os.has_feature("mobile") {
        return 0;
    }
    let ds = DisplayServer::singleton();
    let safe = ds.get_display_safe_area();
    let safe_gap = if safe.size.y > 0 {
        win.1 - safe.position.y - safe.size.y
    } else {
        0
    };
    crate::layout::bottom_inset_px(
        win.1,
        safe_gap,
        ds.screen_get_dpi(),
        os.has_feature("android"),
    )
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
        if crate::persist::keep_aside_damaged_runs(std::path::Path::new(&self.runs_dir)) > 0 {
            self.g.note_damaged_save();
        }
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
    /// Draw the root viewport straight into the window's framebuffer when
    /// the canvas fills the window. By default the Compatibility renderer
    /// draws the canvas into a window-sized render target and then copies
    /// that to the screen: two full-screen passes (2.5 Mpx each on a
    /// 1080x2340 phone) for one scaled texture. Drawn direct, the copy pass
    /// and its render target go (NOTES.md "Direct to screen").
    ///
    /// Godot 4.7.2's direct path has two quirks this works around. It draws
    /// upside down (render targets are drawn flipped and the copy pass is
    /// what flips them back), so `draw` mirrors the texture; the mirroring
    /// is in the draw call, not the node, so input is untouched. And it
    /// ignores the letterbox margin (the canvas lands in the bottom-left
    /// corner), so it is only used while the stretched canvas has none:
    /// called again whenever the window or the buffer changes size.
    ///
    /// Android only, where it was measured; desktops keep the copy (on
    /// Windows, ANGLE already flips the screen).
    fn direct_to_screen(&mut self) {
        if !Os::singleton().has_feature("android") {
            return;
        }
        let Some(window) = self.base().get_window() else {
            return;
        };
        let direct = window.get_final_transform().origin == Vector2::ZERO;
        if direct == self.flip_y {
            return;
        }
        RenderingServer::singleton()
            .viewport_set_render_direct_to_screen(window.get_viewport_rid(), direct);
        self.flip_y = direct;
        self.redraw.mark();
        godot_print!("racer: direct to screen {direct}");
    }

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
        // A fixed-seed bot run makes on-device profiling captures comparable.
        // This entry point is absent from ordinary builds.
        #[cfg(feature = "profiling")]
        if let Some(seed) = args
            .iter()
            .find_map(|a| a.strip_prefix("--racer-profile-seed=")?.parse::<u64>().ok())
        {
            godot_print!("racer-prof: starting demo seed {seed}");
            self.g.start_seed(seed, true);
        }
        let dev = crate::cli::DevArgs::parse(&args);
        self.g.tier =
            crate::tier::Tier::detect(Os::singleton().has_feature("shareware"), dev.shareware);
        if self.g.tier.is_shareware() {
            godot_print!("racer: shareware edition");
        }
        if dev.bench {
            let seconds = dev.bench_seconds.unwrap_or(perf::BENCH_SECONDS);
            self.g.fixed_forks = true;
            self.g.start_seed(perf::BENCH_SEED, true);
            self.bench = Some(perf::Bench::new(seconds));
            godot_print!(
                "racer-bench: starting; seed={} seconds={seconds:.0} renderer={}",
                perf::BENCH_SEED,
                RenderingServer::singleton().get_current_rendering_method()
            );
        }
        #[cfg(debug_assertions)]
        {
            self.hud_shown = dev.perf || dev.bench || Os::singleton().has_feature("mobile");
            let (hud, layer) = perf::Hud::new();
            self.base_mut().add_child(&layer);
            self.hud = Some(hud);
        }
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
    /// a nicety, the game goes on. An existing file that does not parse is
    /// renamed aside first, and the write itself is atomic.
    fn save_run(&mut self, name: &str, text: &str) {
        let dir = self.runs_directory();
        let path = dir.join(name);
        if self.hold_saves {
            godot_warn!(
                "racer: not writing {}, a damaged save is still in place",
                path.display()
            );
            return;
        }
        if path.is_file() {
            if let Ok(old) = std::fs::read_to_string(&path) {
                if racer_sim::replay::decode_run(&old).is_err() {
                    self.keep_aside(&path);
                }
            }
        }
        if self.hold_saves {
            return;
        }
        match crate::persist::write_atomic(&path, text) {
            Ok(()) => {
                godot_print!("racer: run saved to {}", path.display());
            }
            Err(e) => {
                godot_warn!("racer: could not save {}: {e}", path.display());
            }
        }
    }

    fn runs_directory(&self) -> std::path::PathBuf {
        let dir = if self.runs_dir.is_empty() {
            ProjectSettings::singleton()
                .globalize_path("user://runs")
                .to_string()
        } else {
            self.runs_dir.clone()
        };
        std::path::PathBuf::from(dir)
    }

    /// Move `path` to a `.corrupt` sidecar. False if it could not be moved:
    /// further saves are held so the bytes stay put.
    fn keep_aside(&mut self, path: &std::path::Path) -> bool {
        match crate::persist::keep_aside(path) {
            Ok(dest) => {
                godot_warn!(
                    "racer: kept damaged {} aside at {}",
                    path.display(),
                    dest.display()
                );
                self.g.note_damaged_save();
                true
            }
            Err(e) => {
                godot_warn!(
                    "racer: could not move {} aside ({e}); not replacing it",
                    path.display()
                );
                self.hold_saves = true;
                self.g.note_damaged_save();
                false
            }
        }
    }

    fn load_saved_ghosts(&mut self) {
        use racer_sim::ghost::GhostText;
        let path = ghosts_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match racer_sim::ghost::GhostStore::classify(&text, racer_sim::SIM_EPOCH) {
            GhostText::Clean(store) => {
                let n = store.len();
                self.g.load_ghost_store(store);
                godot_print!("racer: {n} ghosts loaded");
            }
            GhostText::Recovered(store) => {
                if !self.keep_aside(&path) {
                    return;
                }
                let clean = store.encode(racer_sim::SIM_EPOCH);
                if let Err(e) = crate::persist::write_atomic(&path, &clean) {
                    godot_warn!("racer: could not rewrite {}: {e}", path.display());
                }
                let n = store.len();
                self.g.load_ghost_store(store);
                godot_print!("racer: {n} ghosts kept from a damaged file");
            }
            GhostText::Damaged => {
                self.keep_aside(&path);
            }
        }
    }

    fn load_saved_daily(&mut self) {
        use crate::daily::DailyText;
        let path = user_path(crate::daily::DAILY_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match DailyRecord::classify(&text) {
            DailyText::Record(_) | DailyText::OtherEpoch => self.g.load_daily(&text),
            DailyText::Damaged => {
                self.keep_aside(&path);
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
            self.direct_to_screen();
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
                Effect::SaveControls(modern) => {
                    let path = user_path("controls.txt");
                    let tmp = path.with_extension("tmp");
                    let text = if modern { "touch\n" } else { "buttons\n" };
                    if let Err(e) =
                        std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path))
                    {
                        godot_warn!("racer: could not save controls: {e}");
                    }
                }
                Effect::Redraw => self.redraw.mark(),
                Effect::Buffer { w, h } => {
                    self.canvas.resize(w, h);
                    if let Some(mut window) = self.base().get_window() {
                        window.set_content_scale_size(Vector2i::new(w, h));
                    }
                    self.direct_to_screen();
                    self.redraw.mark();
                }
                Effect::KeepScreenOn(on) => DisplayServer::singleton().screen_set_keep_on(on),
                Effect::Quit => self.base().get_tree().quit(),
                Effect::SaveRun { name, text } => self.save_run(&name, &text),
                Effect::SaveGhosts { text } => {
                    if self.hold_saves {
                        godot_warn!("racer: not writing ghosts, a damaged save is still in place");
                    } else {
                        save_ghosts(&text);
                    }
                }
                Effect::SaveDaily { text } => {
                    if self.hold_saves {
                        godot_warn!(
                            "racer: not writing the daily, a damaged save is still in place"
                        );
                    } else {
                        let path = user_path(crate::daily::DAILY_FILE);
                        if let Err(e) = write_atomic(&path, &text) {
                            godot_warn!("racer: could not save {}: {e}", path.display());
                        }
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
