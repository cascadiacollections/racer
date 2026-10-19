//! Seasons: a stretch of the calendar when the game dresses up
//! (DESIGN §5.2). Presentation only, like biomes: a season changes how a
//! track is painted and what the title says, never a track, a layout, a
//! code, a proof, a ghost, a daily or a run file.
//!
//! A season is judged by the same UTC day the daily run uses
//! ([`crate::input::Intents::today`]), so it starts and ends at 00:00 UTC
//! everywhere, and recurs every year. `--racer-season halloween|none`
//! ([`crate::cli::DevArgs`]) forces one on or off for testing and
//! screenshots. With neither a day nor a flag (the tests' scenes), there is
//! no season and every frame is the normal art.
//!
//! During a season every [`Biome`] is painted as its [`Look`]: HALLOWEEN
//! turns each into its spooky twin.

use racer_sim::biome::Biome;

/// A season of the year.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Season {
    /// No season: the normal art.
    #[default]
    None,
    /// HALLOWEEN: October 19 to November 1, every year.
    Halloween,
}

impl Season {
    /// Every season that has a window.
    pub const ALL: [Season; 1] = [Season::Halloween];

    /// Its calendar window, inclusive at both ends: `((month, day),
    /// (month, day))`. A window may wrap the new year.
    pub fn window(self) -> Option<((u32, u32), (u32, u32))> {
        match self {
            Season::None => None,
            Season::Halloween => Some(((10, 19), (11, 1))),
        }
    }

    /// Whether this season is on during UTC day `day` (days since the
    /// Unix epoch), in any year.
    pub fn covers(self, day: u64) -> bool {
        let Some((from, to)) = self.window() else {
            return false;
        };
        let (_, m, d) = civil(day);
        let md = (m, d);
        if from <= to {
            from <= md && md <= to
        } else {
            md >= from || md <= to
        }
    }

    /// The season on UTC day `day`, or [`Season::None`].
    pub fn on(day: u64) -> Season {
        Season::ALL
            .into_iter()
            .find(|s| s.covers(day))
            .unwrap_or(Season::None)
    }

    /// The season for a frame: the developer flag's if given, else the
    /// day's if the day is known, else none.
    pub fn resolve(forced: Option<Season>, today: Option<u64>) -> Season {
        forced.unwrap_or_else(|| today.map_or(Season::None, Season::on))
    }

    /// The season a `--racer-season` value names: `halloween` or `none`
    /// (any case).
    pub fn parse(s: &str) -> Option<Season> {
        match s.to_ascii_lowercase().as_str() {
            "halloween" => Some(Season::Halloween),
            "none" | "off" => Some(Season::None),
            _ => None,
        }
    }
}

/// The calendar date of UTC day `day`: `(year, month, day)` (proleptic
/// Gregorian; Howard Hinnant's `civil_from_days`).
pub fn civil(day: u64) -> (i64, u32, u32) {
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m as u32, d as u32)
}

/// How a track is painted: its biome's normal look, or (in a season) the
/// biome's twin. Everything the renderer picks by scenery (the palette,
/// the backdrop, the flora, the posts and rocks) it picks by look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Look {
    /// MEADOW.
    #[default]
    Meadow,
    /// COAST.
    Coast,
    /// ALPINE.
    Alpine,
    /// DESERT DUSK.
    Desert,
    /// NIGHT CITY.
    Night,
    /// PUMPKIN PATCH, the meadow's Halloween twin: stubble fields under a
    /// harvest moon, scarecrows and jack-o'-lanterns.
    Pumpkin,
    /// GHOST COVE, the coast's: a night sea in fog bands, a shipwreck,
    /// a pale moon, dead trees on the dunes.
    GhostCove,
    /// HAUNTED PINES, the alpine's: dead black pines in mist on moonlit
    /// snow.
    HauntedPines,
    /// BLOOD MOON MESA, the desert's: dark mesas under a huge red moon in a
    /// purple-red sky, withered cacti.
    BloodMoon,
    /// MONSTER CITY, the night city's: orange windows, a crooked tower, a
    /// green glow, green street lights.
    MonsterCity,
}

impl Look {
    /// The five normal looks, [`Biome::ALL`] order.
    pub const NORMAL: [Look; 5] = [
        Look::Meadow,
        Look::Coast,
        Look::Alpine,
        Look::Desert,
        Look::Night,
    ];

    /// The five Halloween twins, [`Biome::ALL`] order.
    pub const TWINS: [Look; 5] = [
        Look::Pumpkin,
        Look::GhostCove,
        Look::HauntedPines,
        Look::BloodMoon,
        Look::MonsterCity,
    ];

    /// Every look.
    pub const ALL: [Look; 10] = [
        Look::Meadow,
        Look::Coast,
        Look::Alpine,
        Look::Desert,
        Look::Night,
        Look::Pumpkin,
        Look::GhostCove,
        Look::HauntedPines,
        Look::BloodMoon,
        Look::MonsterCity,
    ];

    /// How biome `b` is painted in season `s`.
    pub fn of(b: Biome, s: Season) -> Look {
        match s {
            Season::None => Look::NORMAL[b as usize],
            Season::Halloween => Look::TWINS[b as usize],
        }
    }

    /// The biome it paints.
    pub fn biome(self) -> Biome {
        Biome::ALL[self as usize % 5]
    }

    /// Whether it is a Halloween twin.
    pub fn spooky(self) -> bool {
        self as usize >= 5
    }

    /// Its name, as the title and the fork cards print it.
    pub fn name(self) -> &'static str {
        match self {
            Look::Pumpkin => "PUMPKIN PATCH",
            Look::GhostCove => "GHOST COVE",
            Look::HauntedPines => "HAUNTED PINES",
            Look::BloodMoon => "BLOOD MOON MESA",
            Look::MonsterCity => "MONSTER CITY",
            l => l.biome().name(),
        }
    }
}

impl From<Biome> for Look {
    fn from(b: Biome) -> Look {
        Look::of(b, Season::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTC day number of a date.
    fn day(y: i64, m: u32, d: u32) -> u64 {
        (0..200_000u64)
            .find(|&k| civil(k) == (y, m, d))
            .expect("a date after 1970")
    }

    #[test]
    fn civil_dates_are_the_calendars() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(59), (1970, 3, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(day(2026, 10, 19), 20_745);
        assert_eq!(crate::daily::date(20_745), "2026-10-19");
    }

    #[test]
    fn halloween_runs_october_19_to_november_1_every_year() {
        for y in [2026, 2027, 2028, 2030, 2100] {
            let on = |m, d| Season::on(day(y, m, d));
            assert_eq!(on(10, 18), Season::None, "{y}-10-18");
            assert_eq!(on(10, 19), Season::Halloween, "{y}-10-19");
            assert_eq!(on(10, 25), Season::Halloween, "{y}-10-25");
            assert_eq!(on(10, 31), Season::Halloween, "{y}-10-31");
            assert_eq!(on(11, 1), Season::Halloween, "{y}-11-01");
            assert_eq!(on(11, 2), Season::None, "{y}-11-02");
            assert_eq!(on(1, 1), Season::None);
            assert_eq!(on(7, 4), Season::None);
            assert_eq!(on(12, 31), Season::None);
        }
        // Exactly fourteen days a year.
        let y2027 = day(2027, 1, 1)..day(2028, 1, 1);
        assert_eq!(y2027.filter(|&k| Season::on(k) != Season::None).count(), 14);
        assert!(!Season::None.covers(day(2026, 10, 31)));
    }

    #[test]
    fn the_flag_overrides_the_day_and_no_day_is_no_season() {
        let oct = Some(day(2026, 10, 31));
        let june = Some(day(2026, 6, 1));
        assert_eq!(Season::resolve(None, oct), Season::Halloween);
        assert_eq!(Season::resolve(None, june), Season::None);
        assert_eq!(Season::resolve(None, None), Season::None);
        assert_eq!(
            Season::resolve(Some(Season::None), oct),
            Season::None,
            "forced off in the season"
        );
        assert_eq!(
            Season::resolve(Some(Season::Halloween), june),
            Season::Halloween
        );
        assert_eq!(Season::parse("halloween"), Some(Season::Halloween));
        assert_eq!(Season::parse("HALLOWEEN"), Some(Season::Halloween));
        assert_eq!(Season::parse("none"), Some(Season::None));
        assert_eq!(Season::parse("xmas"), None);
    }

    #[test]
    fn every_biome_has_a_distinct_twin_that_paints_it() {
        let twins: Vec<Look> = Biome::ALL
            .iter()
            .map(|&b| Look::of(b, Season::Halloween))
            .collect();
        assert_eq!(twins, Look::TWINS);
        let names: Vec<&str> = twins.iter().map(|l| l.name()).collect();
        assert_eq!(
            names,
            [
                "PUMPKIN PATCH",
                "GHOST COVE",
                "HAUNTED PINES",
                "BLOOD MOON MESA",
                "MONSTER CITY"
            ]
        );
        for b in Biome::ALL {
            let n = Look::of(b, Season::None);
            let t = Look::of(b, Season::Halloween);
            assert_eq!(Look::from(b), n);
            assert_eq!(n.biome(), b);
            assert_eq!(t.biome(), b);
            assert!(!n.spooky() && t.spooky());
            assert_eq!(n.name(), b.name());
            assert_ne!(t.name(), b.name());
        }
    }
}
