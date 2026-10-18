//! Engine-free frame pacing, after Salt Line's `frame.rs`: whether a frame
//! needs drawing, the engine's frame cap on menus, and frame-time
//! statistics for the profiling build.
//!
//! The frame is one software-rastered buffer, so an unchanged frame is pure
//! waste on a phone: a raster pass and a texture upload for pixels already
//! on screen. Racing changes every frame; menus change only on input.

/// Menus run at this rate at most, frames per second.
pub const IDLE_FPS: i32 = 30;

/// Decides whether this frame needs rasterising and uploading: when
/// something marked it, while anything moves, and once after the last
/// moving frame, so an effect that ran out is wiped rather than frozen.
#[derive(Debug)]
pub struct Redraw {
    dirty: bool,
    was_busy: bool,
}

impl Default for Redraw {
    /// The first frame is always drawn.
    fn default() -> Self {
        Redraw {
            dirty: true,
            was_busy: false,
        }
    }
}

impl Redraw {
    /// Something on screen changed.
    pub fn mark(&mut self) {
        self.dirty = true;
    }

    /// Once per frame; true means draw this frame.
    pub fn due(&mut self, busy: bool) -> bool {
        let due = self.dirty || busy || self.was_busy;
        self.dirty = false;
        self.was_busy = busy;
        due
    }
}

/// The engine's frame cap: [`IDLE_FPS`] on menus, the project's own cap
/// (0 is none) while racing. A project cap tighter than `IDLE_FPS` is kept.
pub fn frame_cap(on_menu: bool, project_cap: i32) -> i32 {
    match (on_menu, project_cap) {
        (false, cap) => cap,
        (true, cap) if cap > 0 => cap.min(IDLE_FPS),
        (true, _) => IDLE_FPS,
    }
}

/// The tighter of two frame caps where 0 means "no cap": the project's own
/// and the platform's power policy (the `device_power` addon: power source,
/// battery saver, thermal status and Android game mode).
pub fn tighter_cap(a: i32, b: i32) -> i32 {
    match (a > 0, b > 0) {
        (true, true) => a.min(b),
        (true, false) => a,
        (false, true) => b,
        (false, false) => 0,
    }
}

/// Durations in milliseconds, summarised as mean and 95th percentile.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    samples: Vec<f64>,
}

impl Stats {
    /// Record one sample, milliseconds.
    pub fn push(&mut self, ms: f64) {
        self.samples.push(ms);
    }

    /// How many samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Mean, 0 when empty.
    pub fn mean(&self) -> f64 {
        if self.samples.is_empty() {
            0.0
        } else {
            self.samples.iter().sum::<f64>() / self.samples.len() as f64
        }
    }

    /// The `p`-th percentile (nearest rank), 0 when empty.
    pub fn percentile(&self, p: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut s = self.samples.clone();
        s.sort_by(f64::total_cmp);
        let rank = ((p / 100.0) * s.len() as f64).ceil() as usize;
        s[rank.clamp(1, s.len()) - 1]
    }

    /// Forget every sample.
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_draws_then_an_idle_one_does_not() {
        let mut r = Redraw::default();
        assert!(r.due(false));
        assert!(!r.due(false));
        r.mark();
        assert!(r.due(false));
        assert!(!r.due(false));
    }

    #[test]
    fn a_busy_spell_draws_throughout_and_once_after() {
        let mut r = Redraw::default();
        r.due(false);
        assert!(r.due(true));
        assert!(r.due(true));
        assert!(r.due(false));
        assert!(!r.due(false));
    }

    #[test]
    fn menus_cap_and_racing_keeps_the_project_cap() {
        assert_eq!(frame_cap(true, 0), IDLE_FPS);
        assert_eq!(frame_cap(false, 0), 0);
        assert_eq!(frame_cap(true, 120), IDLE_FPS);
        assert_eq!(frame_cap(true, 20), 20);
        assert_eq!(frame_cap(false, 120), 120);
    }

    #[test]
    fn the_tighter_cap_wins_and_zero_is_no_cap() {
        assert_eq!(tighter_cap(0, 0), 0);
        assert_eq!(tighter_cap(0, 60), 60);
        assert_eq!(tighter_cap(144, 0), 144);
        assert_eq!(tighter_cap(45, 60), 45);
        assert_eq!(tighter_cap(-1, 60), 60);
        // Menus still drop below a platform cap.
        assert_eq!(frame_cap(true, tighter_cap(0, 60)), IDLE_FPS);
        assert_eq!(frame_cap(false, tighter_cap(0, 60)), 60);
    }

    #[test]
    fn stats_mean_and_p95() {
        let mut s = Stats::default();
        assert_eq!(s.mean(), 0.0);
        assert_eq!(s.percentile(95.0), 0.0);
        for i in 1..=100 {
            s.push(i as f64);
        }
        assert_eq!(s.mean(), 50.5);
        assert_eq!(s.percentile(95.0), 95.0);
        assert_eq!(s.percentile(100.0), 100.0);
        assert_eq!(s.len(), 100);
        s.clear();
        assert!(s.is_empty());
    }
}
