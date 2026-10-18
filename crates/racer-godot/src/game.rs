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
    DisplayServer, Engine, INode2D, Image, ImageTexture, Input, InputEvent, InputEventJoypadButton,
    InputEventKey, InputEventScreenDrag, InputEventScreenTouch, Node2D, Time,
};
use godot::global::{JoyAxis, JoyButton, Key};
use godot::prelude::*;

use crate::app::{Effect, Game};
use crate::builder::Worker;
use crate::frame::{frame_cap, tighter_cap, Redraw};
use crate::input::{self, Event, Intents};
use crate::raster::Canvas;
use crate::screens;
use crate::sprites::Art;

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
    /// The platform's cap from its power state (0 is none), set by
    /// `godot/platform_services.gd` from the `DevicePower` autoload.
    platform_cap: i32,
    cap: i32,
    #[cfg(feature = "profiling")]
    profile: crate::profiling::Profile,
}

#[godot_api]
impl INode2D for Racer {
    fn init(base: Base<Node2D>) -> Self {
        Racer {
            base,
            g: Game::new(Box::new(Worker::spawn())),
            art: Art::new(),
            canvas: Canvas::new(crate::layout::W, crate::layout::H_MIN),
            events: Vec::new(),
            intents: Intents::default(),
            redraw: Redraw::default(),
            image: None,
            texture: None,
            project_cap: 0,
            platform_cap: 0,
            cap: -1,
            #[cfg(feature = "profiling")]
            profile: crate::profiling::Profile::default(),
        }
    }

    fn ready(&mut self) {
        self.project_cap = Engine::singleton().get_max_fps();
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

#[godot_api]
impl Racer {
    /// The platform's frame cap from its power state (0 is none): see the
    /// `device_power` addon. The tighter of it and the project's cap is used
    /// while racing; menus still drop to `frame::IDLE_FPS`.
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
        let t = Time::singleton();
        i.entropy = (t.get_unix_time_from_system() * 1e6) as u64 ^ t.get_ticks_usec();
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
