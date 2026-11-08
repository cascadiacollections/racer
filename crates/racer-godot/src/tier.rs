//! Which edition this build is: the free shareware build or the full game,
//! as in Salt Line's `tier.rs`.
//!
//! The code is the same in both. An export preset with the `shareware`
//! custom feature (or `--racer-shareware` on the command line) makes the
//! node start the game as [`Tier::Shareware`]. That edition:
//!
//! - ends a run, daily runs included, at the fork after
//!   [`SHAREWARE_TRACKS`] tracks, saved and scored as far as it went, on a
//!   results screen that says so;
//! - leaves the TRACKS list and the editor out. Shared layouts
//!   (`--racer-layout`) still race.
//!
//! The simulation never sees the tier. Tracks depend only on `(run_seed,
//! index, branch)`, so a seed plays the same roads in both editions up to
//! the cap, and ghosts and run files stay compatible.

/// Tracks a shareware run gets, of the full run's five.
pub const SHAREWARE_TRACKS: usize = 2;

/// The note shown when the shareware build is asked for a full-game feature.
pub const FULL_GAME_ONLY: &str = "FULL GAME ONLY";

/// The edition of the game this build is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tier {
    /// Everything, no cap. The default, so tests and dev runs see the whole
    /// game unless they ask otherwise.
    #[default]
    Full,
    /// The free build: capped runs, no editor.
    Shareware,
}

impl Tier {
    /// The tier for a build with these feature flags: shareware if the export
    /// preset or the command line asked for it.
    pub fn detect(export_feature: bool, dev_flag: bool) -> Self {
        if export_feature || dev_flag {
            Self::Shareware
        } else {
            Self::Full
        }
    }

    pub fn is_shareware(self) -> bool {
        self == Self::Shareware
    }

    /// Whether a run that has finished `tracks_done` tracks has reached this
    /// edition's end.
    pub fn run_capped(self, tracks_done: usize) -> bool {
        self.is_shareware() && tracks_done >= SHAREWARE_TRACKS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_is_the_default_and_never_caps() {
        assert_eq!(Tier::default(), Tier::Full);
        assert!(!Tier::Full.run_capped(usize::MAX));
    }

    #[test]
    fn either_flag_selects_shareware() {
        assert_eq!(Tier::detect(false, false), Tier::Full);
        assert_eq!(Tier::detect(true, false), Tier::Shareware);
        assert_eq!(Tier::detect(false, true), Tier::Shareware);
    }

    #[test]
    fn shareware_caps_at_the_limit_not_before() {
        assert!(!Tier::Shareware.run_capped(SHAREWARE_TRACKS - 1));
        assert!(Tier::Shareware.run_capped(SHAREWARE_TRACKS));
    }
}
