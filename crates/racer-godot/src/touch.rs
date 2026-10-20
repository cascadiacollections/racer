//! Engine-free touch: on-screen buttons and the fingers holding them,
//! after Salt Line's `touch.rs`.
//!
//! The renderer lays out the buttons as it draws them; the next frame's
//! touches are hit-tested against that layout, the one the player was
//! looking at. A finger is bound to the button it went down on until it
//! lifts (capture on down), so a finger held through a screen change never
//! presses whatever appears under it.
//!
//! The default Drive surface captures one thumb at its touch-down point:
//! sideways drag steers, up holds drift and down brakes. Lift resets every
//! axis. A second thumb never steals the active gesture.
//!
//! In the original button scheme, one exception for driving: a finger on a steering button that slides
//! onto the other steering button re-binds to it. Steering is one thumb
//! rocking between L and R; lifting it to change direction would cost a
//! frame or two at the worst moment. Sliding onto any other button still
//! does nothing.
//!
//! Presses are latched as they arrive and published once per frame by
//! [`TouchPad::update`], so a tap that goes down and up between two frames
//! is still seen once.

use crate::layout::Rect;

/// What a button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Floating one-thumb steering and vertical brake/drift gesture.
    Drive,
    Settings,
    ToggleControls,
    CloseSettings,
    Left,
    Right,
    Brake,
    Drift,
    Pause,
    Start,
    /// The title's DAILY: today's run, the same for everyone.
    Daily,
    Demo,
    PickEasy,
    PickHard,
    Resume,
    Quit,
    Continue,
    Retry,
    /// The title's TRACKS: the courses, the slots and the editor.
    Tracks,
    /// FORK THIS TRACK (pause and results): edit the track just raced.
    ForkTrack,
    /// A button on an editor page (DESIGN §9.11).
    Edit(crate::editor::Act),
}

impl Action {
    fn steers(self) -> bool {
        matches!(self, Action::Left | Action::Right)
    }
}

/// An on-screen button: where it is and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub rect: Rect,
    pub action: Action,
}

/// The fingers on the screen and what they hold.
#[derive(Debug, Default)]
pub struct TouchPad {
    fingers: Vec<(i32, Option<Action>)>,
    drive: Option<DriveGesture>,
    went_down: Vec<Action>,
    pressed: Vec<Action>,
}

/// Captured thumb, relative to where it touched down. A second thumb
/// cannot steal control; releasing the owner returns every axis to neutral.
#[derive(Clone, Copy, Debug)]
pub struct DriveGesture {
    pub finger: i32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub x: f32,
    pub y: f32,
    vertical: i8,
}

impl TouchPad {
    /// Raw stick axis; the normal stick dead zone and seven-level
    /// quantisation apply, so touch still records the same replay byte.
    pub fn drive_axis(&self) -> f32 {
        self.drive
            .map_or(0.0, |d| ((d.x - d.origin_x) / 30.0).clamp(-1.0, 1.0))
    }

    pub fn drive_brake(&self) -> bool {
        self.drive.is_some_and(|d| d.vertical == 1)
    }

    pub fn drive_drift(&self) -> bool {
        self.drive.is_some_and(|d| d.vertical == -1)
    }

    pub fn gesture(&self) -> Option<DriveGesture> {
        self.drive
    }

    /// A finger touched down at `(x, y)`; `buttons` is the layout last
    /// drawn. Later buttons are drawn over earlier ones, so the last one
    /// containing the point wins.
    pub fn finger_down(&mut self, index: i32, x: f32, y: f32, buttons: &[Button]) {
        let action = hit(buttons, x, y);
        if self.drive.is_some_and(|d| d.finger == index) {
            self.drive = None;
        }
        if action == Some(Action::Drive) && self.drive.is_none() {
            self.drive = Some(DriveGesture {
                finger: index,
                origin_x: x,
                origin_y: y,
                x,
                y,
                vertical: 0,
            });
        }
        self.fingers.retain(|(i, _)| *i != index);
        self.fingers.push((index, action));
        if let Some(a) = action {
            self.went_down.push(a);
        }
    }

    /// A finger moved. Only a steering finger can change button: onto the
    /// other steering button.
    pub fn finger_moved(&mut self, index: i32, x: f32, y: f32, buttons: &[Button]) {
        if let Some(d) = self.drive.as_mut().filter(|d| d.finger == index) {
            d.x = x;
            d.y = y;
            let dy = y - d.origin_y;
            // Hysteresis prevents brake/drift chatter at the boundary.
            d.vertical = if dy >= 14.0 {
                1
            } else if dy <= -14.0 {
                -1
            } else if (d.vertical == 1 && dy > 8.0) || (d.vertical == -1 && dy < -8.0) {
                d.vertical
            } else {
                0
            };
        }
        let Some(f) = self.fingers.iter_mut().find(|(i, _)| *i == index) else {
            return;
        };
        if let (Some(held), Some(now)) = (f.1, hit(buttons, x, y)) {
            if held.steers() && now.steers() {
                f.1 = Some(now);
            }
        }
    }

    /// A finger lifted.
    pub fn finger_up(&mut self, index: i32) {
        if self.drive.is_some_and(|d| d.finger == index) {
            self.drive = None;
        }
        self.fingers.retain(|(i, _)| *i != index);
    }

    /// Let go of everything: focus lost, or the screen changed under the
    /// fingers.
    pub fn release_all(&mut self) {
        self.drive = None;
        self.fingers.clear();
        self.went_down.clear();
        self.pressed.clear();
    }

    /// Once per frame: publish the presses since the last update.
    pub fn update(&mut self) {
        self.pressed = std::mem::take(&mut self.went_down);
    }

    /// Every press this frame, in order.
    pub fn presses(&self) -> &[Action] {
        &self.pressed
    }

    /// Whether `a` was pressed this frame.
    pub fn pressed(&self, a: Action) -> bool {
        self.pressed.contains(&a)
    }

    /// Whether a finger is holding `a` now.
    pub fn held(&self, a: Action) -> bool {
        self.fingers.iter().any(|(_, f)| *f == Some(a))
    }

    /// Whether any finger is down.
    pub fn active(&self) -> bool {
        !self.fingers.is_empty()
    }

    /// What finger `index` is holding, if it is down on a button.
    pub fn finger(&self, index: i32) -> Option<Action> {
        self.fingers.iter().find(|(i, _)| *i == index)?.1
    }
}

fn hit(buttons: &[Button], x: f32, y: f32) -> Option<Action> {
    buttons
        .iter()
        .rev()
        .find(|b| b.rect.contains(x, y))
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture_pad() -> [Button; 1] {
        [Button {
            rect: Rect::new(0, 0, 180, 92),
            action: Action::Drive,
        }]
    }

    #[test]
    fn one_thumb_steers_brakes_and_drifts_with_hysteresis() {
        let mut t = TouchPad::default();
        let b = gesture_pad();
        t.finger_down(0, 90.0, 46.0, &b);
        assert_eq!(t.drive_axis(), 0.0);
        t.finger_moved(0, 105.0, 30.0, &b);
        assert_eq!(t.drive_axis(), 0.5);
        assert!(t.drive_drift() && !t.drive_brake());
        t.finger_moved(0, 120.0, 35.0, &b);
        assert!(t.drive_drift(), "hysteresis retains drift");
        t.finger_moved(0, 120.0, 40.0, &b);
        assert!(!t.drive_drift() && !t.drive_brake());
        t.finger_moved(0, 60.0, 65.0, &b);
        assert_eq!(t.drive_axis(), -1.0);
        assert!(t.drive_brake() && !t.drive_drift());
        t.finger_moved(0, 60.0, 35.0, &b);
        assert!(
            !t.drive_brake() && !t.drive_drift(),
            "crossing neutral releases brake even between motion events"
        );
        t.finger_up(0);
        assert_eq!(t.drive_axis(), 0.0);
        assert!(!t.drive_brake() && !t.drive_drift());
    }

    #[test]
    fn a_second_finger_cannot_steal_steering_and_cancel_releases_it() {
        let mut t = TouchPad::default();
        let b = gesture_pad();
        t.finger_down(2, 40.0, 46.0, &b);
        t.finger_down(3, 140.0, 46.0, &b);
        t.finger_moved(3, 170.0, 66.0, &b);
        assert_eq!(t.drive_axis(), 0.0);
        assert!(!t.drive_brake());
        t.finger_moved(2, -100.0, -100.0, &b);
        assert_eq!(t.drive_axis(), -1.0);
        assert!(t.drive_drift(), "capture continues outside the surface");
        t.finger_up(3);
        assert!(t.gesture().is_some());
        t.release_all();
        assert!(t.gesture().is_none() && !t.active());
        t.finger_moved(2, 80.0, 80.0, &b);
        assert_eq!(t.drive_axis(), 0.0);
    }

    fn pad() -> Vec<Button> {
        vec![
            Button {
                rect: Rect::new(0, 0, 10, 10),
                action: Action::Left,
            },
            Button {
                rect: Rect::new(10, 0, 10, 10),
                action: Action::Right,
            },
            Button {
                rect: Rect::new(20, 0, 10, 10),
                action: Action::Drift,
            },
        ]
    }

    #[test]
    fn a_finger_holds_the_button_it_went_down_on() {
        let b = pad();
        let mut t = TouchPad::default();
        t.finger_down(0, 5.0, 5.0, &b);
        t.update();
        assert!(t.pressed(Action::Left) && t.held(Action::Left));
        t.update();
        assert!(!t.pressed(Action::Left), "a press lasts one frame");
        assert!(t.held(Action::Left));
        t.finger_up(0);
        assert!(!t.held(Action::Left) && !t.active());
    }

    #[test]
    fn hit_testing_is_half_open_and_the_last_button_wins() {
        let mut b = pad();
        let mut t = TouchPad::default();
        t.finger_down(0, 10.0, 5.0, &b);
        assert!(
            t.held(Action::Right),
            "the shared edge belongs to the right"
        );
        b.push(Button {
            rect: Rect::new(0, 0, 30, 10),
            action: Action::Pause,
        });
        t.finger_down(1, 5.0, 5.0, &b);
        assert!(t.held(Action::Pause));
        t.finger_down(2, 50.0, 50.0, &b);
        assert!(t.active());
    }

    #[test]
    fn steering_slides_between_left_and_right_but_nothing_else_rebinds() {
        let b = pad();
        let mut t = TouchPad::default();
        t.finger_down(0, 5.0, 5.0, &b);
        t.finger_moved(0, 15.0, 5.0, &b);
        assert!(t.held(Action::Right) && !t.held(Action::Left));
        t.finger_moved(0, 25.0, 5.0, &b);
        assert!(t.held(Action::Right), "sliding onto drift keeps the steer");
        t.finger_moved(0, 99.0, 99.0, &b);
        assert!(t.held(Action::Right), "sliding off keeps it too");
        t.update();
        assert!(!t.pressed(Action::Right), "a slide is not a press");
        t.finger_down(1, 25.0, 5.0, &b);
        t.finger_moved(1, 5.0, 5.0, &b);
        assert!(t.held(Action::Drift) && !t.held(Action::Left));
    }

    #[test]
    fn a_tap_inside_one_frame_still_counts_and_release_all_clears() {
        let b = pad();
        let mut t = TouchPad::default();
        t.finger_down(3, 25.0, 1.0, &b);
        t.finger_up(3);
        t.update();
        assert!(t.pressed(Action::Drift));
        assert!(!t.held(Action::Drift));
        t.finger_down(4, 1.0, 1.0, &b);
        t.finger_down(5, 11.0, 1.0, &b);
        t.release_all();
        t.update();
        assert!(!t.active() && !t.pressed(Action::Left));
    }
}
