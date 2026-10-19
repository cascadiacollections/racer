//! Biomes: what a track looks like, never how it drives. DESIGN §5.1.
//!
//! A biome picks the scenery a track is painted with (its sixteen colours,
//! its skyline, its roadside things). It lives in the sim crate only
//! because a [`crate::layout::Layout`] carries one; nothing the sim
//! computes reads it. It is not part of a track, its digest, a layout's
//! digest or a proof stamp, and no state hash, replay or golden run sees
//! it, so choosing or changing a biome never needs a `SIM_EPOCH` bump.
//!
//! **Generated tracks** take theirs from the track index alone
//! ([`Biome::of_track`]): a run is a day on the road, from a morning
//! meadow to a city at night, so the five tracks of every run look
//! distinct and DEMO shows every biome in a run. Both branches of a fork
//! share the index's biome. **Layouts** carry theirs ([`Biome::code`] in
//! the share code; a fork takes its track's).

/// The scenery a track is painted with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Biome {
    /// Green hills and trees under a morning sky: the original look.
    #[default]
    Meadow = 0,
    /// A sea horizon, sand verges and palms at midday.
    Coast = 1,
    /// Snowy peaks, pines and white verges.
    Alpine = 2,
    /// Mesas and cacti under an orange and purple dusk.
    Desert = 3,
    /// A skyline of lit windows, stars and street lights.
    Night = 4,
}

impl Biome {
    /// Every biome, in a run's order.
    pub const ALL: [Biome; 5] = [
        Biome::Meadow,
        Biome::Coast,
        Biome::Alpine,
        Biome::Desert,
        Biome::Night,
    ];

    /// The biome of generated track `index` of any run: [`Biome::ALL`] in
    /// order, from the first track (wrapping, should a run ever be longer).
    /// A function of the index only: no RNG is drawn, so generation is
    /// untouched, and both roads of a fork share it.
    pub fn of_track(index: u32) -> Biome {
        Biome::ALL[index as usize % Biome::ALL.len()]
    }

    /// Its name, as the title and the fork cards print it.
    pub fn name(self) -> &'static str {
        match self {
            Biome::Meadow => "MEADOW",
            Biome::Coast => "COAST",
            Biome::Alpine => "ALPINE",
            Biome::Desert => "DESERT DUSK",
            Biome::Night => "NIGHT CITY",
        }
    }

    /// Its number in a layout code.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The biome numbered `c` in a layout code, if any.
    pub fn from_code(c: u8) -> Option<Biome> {
        Biome::ALL.get(c as usize).copied()
    }

    /// The next biome, wrapping: the editor's BIOME item.
    pub fn next(self) -> Biome {
        Biome::ALL[(self as usize + 1) % Biome::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_shows_every_biome_once_from_the_meadow() {
        let run: Vec<Biome> = (0..crate::gen::difficulty::TRACKS)
            .map(Biome::of_track)
            .collect();
        assert_eq!(run, Biome::ALL);
        assert_eq!(Biome::of_track(0), Biome::Meadow);
        assert_eq!(Biome::of_track(5), Biome::Meadow);
        assert_eq!(Biome::default(), Biome::Meadow);
    }

    #[test]
    fn codes_and_names_round_trip() {
        for b in Biome::ALL {
            assert_eq!(Biome::from_code(b.code()), Some(b));
            assert!(!b.name().is_empty());
        }
        assert_eq!(Biome::from_code(5), None);
        let mut b = Biome::Meadow;
        for _ in 0..5 {
            b = b.next();
        }
        assert_eq!(b, Biome::Meadow);
    }
}
