//! Custom races: one layout, raced alone. DESIGN §9.6.
//!
//! A custom race is a run of one track (`RunState::single`), driven with
//! the custom loadout. It is timed only if the layout is proven: a proven
//! layout carries its timers, an unproven one is free drive, with no timer
//! but the race's own cap. Its best finish is kept as a ghost under the
//! layout's digest (`Layout::ghost_key`), so the same road meets the same
//! ghost whatever it is called.
//!
//! The TRACKS list (bundled courses and saved slots), the editor's TEST
//! DRIVE and the `--racer-layout` developer flag (`cli.rs`) start them.
//! [`load`] is the engine-free part of the flag: text in, a race to start
//! (or why not) out.

use std::sync::Arc;

use racer_sim::ghost::TrackKey;
use racer_sim::layout::{race_track, Layout, LayoutFile, Standing, Verified};
use racer_sim::track::Track;

/// A layout ready to race.
#[derive(Clone, Debug)]
pub struct Custom {
    /// The layout, with the proof it races under (if any).
    pub layout: Layout,
    /// Its track: timed if `timed`.
    pub track: Arc<Track>,
    /// Whether the race has a timer: the layout is proven.
    pub timed: bool,
    /// Where the layout came from (a fork's provenance, a course's), for
    /// FORK THIS TRACK.
    pub origin: Option<String>,
    /// A test drive from the editor: its results and pause go back there.
    pub test: bool,
}

impl Custom {
    /// `layout` ready to race: timed only if its stamp is this build's
    /// (`Standing::Proven`), free drive otherwise.
    pub fn of(layout: Layout, origin: Option<String>, test: bool) -> Result<Custom, String> {
        let (track, timed) = race_track(&layout).map_err(|p| format!("{p:?}"))?;
        Ok(Custom {
            layout,
            track,
            timed,
            origin,
            test,
        })
    }

    /// Where its ghost is kept.
    pub fn ghost_key(&self) -> TrackKey {
        self.layout.ghost_key()
    }
}

/// Read a layout from `text` (a code, or a layout file) and get it ready to
/// race, with a note for the console saying how it stands.
///
/// Trust, then verify: with `verify`, a proven layout is proven again and
/// races with the timers this build gives it; without, a stamp from this
/// epoch is trusted. A stale or bad stamp is always proven again. A layout
/// that does not prove is still raced, as free drive. Proving takes about
/// 60–80 ms on a desktop (NOTES.md), so this is not for a frame callback.
pub fn load(text: &str, verify: bool) -> Result<(Custom, String), String> {
    let file = LayoutFile::parse(text).ok_or("no RL or RD code found")?;
    let mut layout = Layout::from_code(&file.code).map_err(|e| e.to_string())?;
    let standing = layout.standing();
    let note = match standing {
        Standing::Unproven => "unproven: free drive, no timer".to_string(),
        Standing::Proven if !verify => "proven: timed (stamp trusted)".to_string(),
        _ => match layout.verify() {
            Verified::Confirmed => "proven: timed (verified)".to_string(),
            Verified::Reproved(l) => {
                layout = *l;
                format!("{standing:?} proof re-proven: timed with this build's timers")
            }
            Verified::Failed(e) => {
                layout = layout.unproven();
                format!("{standing:?} proof does not hold ({e}): free drive, no timer")
            }
            Verified::Unproven => unreachable!("a proof was there"),
        },
    };
    let (track, timed) = race_track(&layout).map_err(|p| format!("{p:?}"))?;
    let name = if layout.name.is_empty() {
        "an unnamed layout".to_string()
    } else {
        layout.name.clone()
    };
    Ok((
        Custom {
            layout,
            track,
            timed,
            origin: file.origin,
            test: false,
        },
        format!("{name}: {note}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use racer_sim::layout::bundled;

    #[test]
    fn a_bundled_course_loads_timed_and_its_draft_free() {
        let course = bundled::layouts().remove(0);
        let code = course.to_code().unwrap();
        let (c, note) = load(&code, false).unwrap();
        assert!(c.timed, "{note}");
        assert_eq!(
            c.track.allot_ticks(),
            &course.proof.as_ref().unwrap().allot_ticks[..]
        );
        assert!(note.starts_with("FIRST LIGHT: proven"), "{note}");
        let (v, _) = load(&code, true).unwrap();
        assert_eq!(v.layout, c.layout);

        let draft = course.unproven().to_code().unwrap();
        let (d, note) = load(&format!("# a draft\n{draft}\n"), true).unwrap();
        assert!(!d.timed && d.track.allot_ticks().is_empty(), "{note}");
        assert_eq!(d.ghost_key(), c.ghost_key(), "one road, one ghost");
    }

    #[test]
    fn a_layout_edited_after_proving_is_proven_again_or_driven_free() {
        let mut l = bundled::layouts().remove(0);
        l.segments[40].curve_milli += 2;
        let (c, note) = load(&l.to_code().unwrap(), false).unwrap();
        assert!(note.contains("BadStamp"), "{note}");
        // Re-proven for the edited road: its own stamp.
        if c.timed {
            assert_eq!(c.layout.standing(), Standing::Proven);
        }
        assert!(load("hello", false).is_err());
        assert!(load("RD0000", false).is_err());
    }
}
