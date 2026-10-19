//! The daily run (DESIGN §1.4): one run a day, the same for everyone.
//!
//! The day is a whole UTC day since the Unix epoch, as in Salt Line: the
//! adapter reads the clock and hands the day in ([`crate::input::Intents`]);
//! the seed is [`daily_seed`] of it, a pure function. A day starts at
//! midnight UTC everywhere, so two players on the same date in different
//! time zones can, for a few hours, be on different dailies; the title says
//! which date it is.
//!
//! What is kept per player is [`DailyRecord`]: today's tries and best, one
//! line in `user://daily.txt`, replaced when the day changes. The tracks
//! themselves are never kept here: they are the seed's, and the track store
//! keeps them built.

use racer_sim::gen::difficulty::TRACKS;
pub use racer_sim::run::daily_seed;

use crate::builder::Key;

/// The record file's name, in the adapter's `user://`.
pub const DAILY_FILE: &str = "daily.txt";

/// The UTC day of `unix_secs` seconds since the Unix epoch (0 before it).
pub fn day_of(unix_secs: f64) -> u64 {
    (unix_secs / 86_400.0).floor().max(0.0) as u64
}

/// The calendar date of UTC day `day`, `YYYY-MM-DD` (proleptic Gregorian;
/// Howard Hinnant's `civil_from_days`).
pub fn date(day: u64) -> String {
    let (y, m, d) = crate::season::civil(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Every track run `seed` can reach: track 1 and both sides of every fork,
/// in the order a run meets them.
pub fn keys(seed: u64) -> Vec<Key> {
    std::iter::once((seed, 0, 0))
        .chain((1..TRACKS).flat_map(|i| [(seed, i, 0), (seed, i, 1)]))
        .collect()
}

/// How far a run got: tracks cleared, and their time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Score {
    /// Tracks finished in time, of [`TRACKS`].
    pub cleared: u32,
    /// Their total time, ticks.
    pub ticks: u32,
}

impl Score {
    /// Whether this beats `other`: more tracks, or as many sooner.
    pub fn beats(self, other: Score) -> bool {
        self.cleared > other.cleared || (self.cleared == other.cleared && self.ticks < other.ticks)
    }
}

/// Today's tries and best: one day's, and only this sim epoch's (another
/// epoch's daily is another run).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DailyRecord {
    /// The day it is for.
    pub day: u64,
    /// Daily runs started that day.
    pub tries: u32,
    /// The best finished run that day, if any.
    pub best: Option<Score>,
}

impl DailyRecord {
    /// Read [`DailyRecord::encode`]'s text; anything else, or another
    /// epoch's, is no record.
    pub fn decode(text: &str) -> DailyRecord {
        DailyRecord::parse(text).unwrap_or_default()
    }

    fn parse(text: &str) -> Option<DailyRecord> {
        let mut lines = text.lines();
        if lines.next()? != "racer-daily 1" {
            return None;
        }
        let f: Vec<&str> = lines.next()?.split(' ').collect();
        let [day, epoch, tries, cleared, ticks] = f[..] else {
            return None;
        };
        if epoch.parse::<u32>().ok()? != racer_sim::SIM_EPOCH {
            return None;
        }
        let best = match (cleared, ticks) {
            ("-", "-") => None,
            _ => Some(Score {
                cleared: cleared.parse().ok()?,
                ticks: ticks.parse().ok()?,
            }),
        };
        Some(DailyRecord {
            day: day.parse().ok()?,
            tries: tries.parse().ok()?,
            best,
        })
    }

    /// The text form: `racer-daily 1`, then `<day> <epoch> <tries>
    /// <cleared> <ticks>` (`- -` with no best).
    pub fn encode(&self) -> String {
        let best = match self.best {
            Some(s) => format!("{} {}", s.cleared, s.ticks),
            None => "- -".into(),
        };
        format!(
            "racer-daily 1\n{} {} {} {best}\n",
            self.day,
            racer_sim::SIM_EPOCH,
            self.tries
        )
    }

    /// `day`'s record: this one, or an empty one if it is for another day.
    pub fn on(&self, day: u64) -> DailyRecord {
        if self.day == day {
            *self
        } else {
            DailyRecord {
                day,
                ..DailyRecord::default()
            }
        }
    }

    /// A daily run of `day` starts: returns how many were started that day
    /// before it.
    pub fn start(&mut self, day: u64) -> u32 {
        *self = self.on(day);
        self.tries += 1;
        self.tries - 1
    }

    /// A daily run of `day` ended with `score`: returns whether it is the
    /// new best.
    pub fn offer(&mut self, day: u64, score: Score) -> bool {
        *self = self.on(day);
        if self.best.is_none_or(|b| score.beats(b)) {
            self.best = Some(score);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_utc_calendar_dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(20_744), "2026-10-18");
        assert_eq!(date(11_016), "2000-02-29");
        assert_eq!(date(11_017), "2000-03-01");
        assert_eq!(day_of(0.0), 0);
        assert_eq!(day_of(-5.0), 0);
        // 2026-10-18 00:00:00 UTC and the second before it.
        assert_eq!(day_of(1_792_281_600.0), 20_744);
        assert_eq!(day_of(1_792_281_599.9), 20_743);
    }

    #[test]
    fn the_daily_is_the_same_all_day_and_new_the_next() {
        let today = day_of(1_792_281_600.0 + 3600.0);
        let tonight = day_of(1_792_281_600.0 + 86_399.0);
        assert_eq!(daily_seed(today), daily_seed(tonight));
        assert_ne!(daily_seed(today), daily_seed(today + 1));
        let k = keys(daily_seed(today));
        assert_eq!(k.len(), 9);
        assert_eq!(k[0], (daily_seed(today), 0, 0));
        assert_eq!(k[8], (daily_seed(today), 4, 1));
    }

    #[test]
    fn the_record_keeps_todays_tries_and_best() {
        let mut r = DailyRecord::default();
        assert_eq!(r.start(100), 0);
        assert_eq!(r.start(100), 1);
        assert!(r.offer(
            100,
            Score {
                cleared: 2,
                ticks: 9000
            }
        ));
        assert!(!r.offer(
            100,
            Score {
                cleared: 2,
                ticks: 9100
            }
        ));
        assert!(r.offer(
            100,
            Score {
                cleared: 2,
                ticks: 8000
            }
        ));
        assert!(r.offer(
            100,
            Score {
                cleared: 3,
                ticks: 20_000
            }
        ));
        let text = r.encode();
        assert_eq!(text, "racer-daily 1\n100 1 2 3 20000\n");
        assert_eq!(DailyRecord::decode(&text), r);
        // The next day starts afresh.
        assert_eq!(
            r.on(101),
            DailyRecord {
                day: 101,
                tries: 0,
                best: None
            }
        );
        assert_eq!(r.start(101), 0);
        assert_eq!(DailyRecord::decode(&r.encode()), r);
        // Damage and other epochs are no record.
        for bad in [
            "",
            "racer-daily 2\n1 1 1 - -\n",
            "racer-daily 1\n1 1 x - -\n",
        ] {
            assert_eq!(DailyRecord::decode(bad), DailyRecord::default());
        }
        let other = format!("racer-daily 1\n100 {} 2 - -\n", racer_sim::SIM_EPOCH + 1);
        assert_eq!(DailyRecord::decode(&other), DailyRecord::default());
    }
}
