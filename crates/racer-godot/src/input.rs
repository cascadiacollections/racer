//! Engine-free input: keys, pad buttons and touches as the core sees them,
//! every binding, and the quantisation to the sim's one-byte [`Input`].
//!
//! All bindings live here, in Rust; project.godot has no input map. The
//! node (`game.rs`) turns Godot's events into [`Event`]s and polls what is
//! held into [`Intents`] once a frame; nothing here names a Godot type.
//!
//! Touch and keys are equally first-class and end in the same byte:
//! a key or a touch button is full lock, a stick is proportional through
//! [`Input::from_axis`], so a replay cannot tell the devices apart
//! (DESIGN §3.1).

use racer_sim::input::Input;

/// The keys the game reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    A,
    D,
    W,
    S,
    Space,
    Shift,
    Enter,
    Escape,
    P,
}

/// Pad buttons the game reads (Godot's SDL layout: `South` is Xbox A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pad {
    Left,
    Right,
    Up,
    Down,
    South,
    East,
    West,
    North,
    L1,
    R1,
    L2,
    R2,
    Start,
    Back,
}

/// Something the player did, oldest first within a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// A key went down (not a repeat).
    Key(Key),
    /// A pad button went down.
    Pad(Pad),
    /// A finger touched down at buffer pixel `(x, y)`.
    FingerDown { index: i32, x: f32, y: f32 },
    /// A finger moved.
    FingerMoved { index: i32, x: f32, y: f32 },
    /// A finger lifted.
    FingerUp { index: i32 },
    /// Android's back gesture.
    Back,
}

/// One frame of input as the node saw it, and the outside world the core
/// may not read for itself.
#[derive(Clone, Debug, Default)]
pub struct Intents {
    /// Everything since the last frame, oldest first.
    pub events: Vec<Event>,
    /// Keys down now.
    pub keys_held: Vec<Key>,
    /// Pad buttons down now, on any pad.
    pub pads_held: Vec<Pad>,
    /// The left stick's x, `-1..=1`, already dead-zoned by the node.
    pub stick_x: f32,
    /// The window, screen pixels.
    pub window: (i32, i32),
    /// Entropy for a new run's seed (the node reads the clock).
    pub entropy: u64,
}

impl Intents {
    /// Nothing pressed or held; the window and entropy are kept.
    pub fn clear(&mut self) {
        self.events.clear();
        self.keys_held.clear();
        self.pads_held.clear();
        self.stick_x = 0.0;
    }

    fn key(&self, k: &[Key]) -> bool {
        self.keys_held.iter().any(|h| k.contains(h))
    }

    fn pad(&self, p: &[Pad]) -> bool {
        self.pads_held.iter().any(|h| p.contains(h))
    }
}

/// Stick deflection below this reads as centred.
pub const STICK_DEAD: f32 = 0.2;

/// Keys and pad buttons for each control. Touch adds to these in `app`.
pub mod bind {
    use super::{Key, Pad};
    pub const LEFT: &[Key] = &[Key::Left, Key::A];
    pub const RIGHT: &[Key] = &[Key::Right, Key::D];
    pub const BRAKE: &[Key] = &[Key::Down, Key::S];
    pub const DRIFT: &[Key] = &[Key::Space, Key::Shift];
    pub const CONFIRM: &[Key] = &[Key::Enter, Key::Space];
    pub const PAUSE: &[Key] = &[Key::Escape, Key::P];
    pub const PAD_LEFT: &[Pad] = &[Pad::Left];
    pub const PAD_RIGHT: &[Pad] = &[Pad::Right];
    pub const PAD_BRAKE: &[Pad] = &[Pad::Down, Pad::West, Pad::L1, Pad::L2];
    pub const PAD_DRIFT: &[Pad] = &[Pad::South, Pad::R1, Pad::R2];
    pub const PAD_CONFIRM: &[Pad] = &[Pad::South, Pad::Start];
    pub const PAD_PAUSE: &[Pad] = &[Pad::Start, Pad::Back];
    pub const MENU_LEFT: &[Key] = &[Key::Left, Key::A];
    pub const MENU_RIGHT: &[Key] = &[Key::Right, Key::D];
}

/// What the player is holding, from every device but touch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    pub left: bool,
    pub right: bool,
    pub brake: bool,
    pub drift: bool,
    /// Stick x, `-1..=1`.
    pub stick: f32,
}

impl Controls {
    /// Keys and pads held in `i`.
    pub fn held(i: &Intents) -> Controls {
        Controls {
            left: i.key(bind::LEFT) || i.pad(bind::PAD_LEFT),
            right: i.key(bind::RIGHT) || i.pad(bind::PAD_RIGHT),
            brake: i.key(bind::BRAKE) || i.pad(bind::PAD_BRAKE),
            drift: i.key(bind::DRIFT) || i.pad(bind::PAD_DRIFT),
            stick: if i.stick_x.abs() < STICK_DEAD {
                0.0
            } else {
                i.stick_x.clamp(-1.0, 1.0)
            },
        }
    }

    /// Either source held: `self` or `other`; a pushed stick is kept.
    pub fn or(self, other: Controls) -> Controls {
        Controls {
            left: self.left || other.left,
            right: self.right || other.right,
            brake: self.brake || other.brake,
            drift: self.drift || other.drift,
            stick: if self.stick != 0.0 {
                self.stick
            } else {
                other.stick
            },
        }
    }

    /// The sim's byte. A digital direction is full lock and overrides the
    /// stick; both directions cancel out to straight; otherwise the stick,
    /// rescaled past the dead zone, picks one of the seven levels.
    pub fn quantise(self) -> Input {
        if self.left || self.right {
            return Input::from_keys(self.left, self.right, self.brake, self.drift);
        }
        let s = self.stick;
        let axis = if s == 0.0 {
            0.0
        } else {
            s.signum() * (s.abs() - STICK_DEAD) / (1.0 - STICK_DEAD)
        };
        Input::from_axis(axis as f64, self.brake, self.drift)
    }
}

/// Whether `e` is a press of one of `keys` or `pads`.
pub fn pressed(e: &Event, keys: &[Key], pads: &[Pad]) -> bool {
    match e {
        Event::Key(k) => keys.contains(k),
        Event::Pad(p) => pads.contains(p),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(keys: &[Key], stick: f32) -> Input {
        let i = Intents {
            keys_held: keys.to_vec(),
            stick_x: stick,
            ..Intents::default()
        };
        Controls::held(&i).quantise()
    }

    #[test]
    fn keys_are_full_lock_and_cancel() {
        assert_eq!(held(&[Key::Left], 0.0).steer_level(), -3);
        assert_eq!(held(&[Key::D], 0.0).steer_level(), 3);
        assert_eq!(held(&[Key::Left, Key::Right], 0.0).steer_level(), 0);
        assert_eq!(held(&[], 0.0), Input::NEUTRAL);
        let i = held(&[Key::Down, Key::Shift], 0.0);
        assert!(i.brake() && i.drift());
        assert!(held(&[Key::Space], 0.0).drift());
    }

    #[test]
    fn the_stick_is_proportional_past_its_dead_zone() {
        assert_eq!(held(&[], 0.15).steer_level(), 0);
        assert_eq!(held(&[], -0.15).steer_level(), 0);
        assert_eq!(held(&[], 0.5).steer_level(), 1);
        assert_eq!(held(&[], 0.75).steer_level(), 2);
        assert_eq!(held(&[], 1.0).steer_level(), 3);
        assert_eq!(held(&[], -1.0).steer_level(), -3);
        // A key overrides the stick.
        assert_eq!(held(&[Key::Left], 1.0).steer_level(), -3);
    }

    #[test]
    fn pads_bind_like_keys() {
        let i = Intents {
            pads_held: vec![Pad::Right, Pad::South, Pad::L2],
            ..Intents::default()
        };
        let q = Controls::held(&i).quantise();
        assert_eq!(q.steer_level(), 3);
        assert!(q.drift() && q.brake());
        assert!(pressed(
            &Event::Pad(Pad::Start),
            bind::CONFIRM,
            bind::PAD_CONFIRM
        ));
        assert!(pressed(
            &Event::Key(Key::Escape),
            bind::PAUSE,
            bind::PAD_PAUSE
        ));
        assert!(!pressed(&Event::Back, bind::PAUSE, bind::PAD_PAUSE));
    }

    #[test]
    fn controls_combine() {
        let a = Controls {
            left: true,
            ..Controls::default()
        };
        let b = Controls {
            drift: true,
            stick: 0.6,
            ..Controls::default()
        };
        let c = a.or(b);
        assert!(c.left && c.drift);
        assert_eq!(c.stick, 0.6);
    }
}
