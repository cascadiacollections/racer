//! The developer flags on the command line, with no engine in them, as in
//! Salt Line's `cli.rs`.
//!
//! The node collects the arguments (Godot splits them either side of `--`,
//! and both halves are read) and [`DevArgs::parse`] reads them. A flag
//! with a value takes it as the next argument or after `=`
//! (`--racer-layout RL04…`, `--racer-layout=course.txt`); the first
//! occurrence wins, and a next argument that starts `--` is another flag,
//! not the value.

/// Every developer flag, parsed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DevArgs {
    /// `--racer-layout <code|path>`: race that layout at once, as a custom
    /// race (DESIGN §9.6): timed if it is proven, free drive if not. A path
    /// (a layout file or a bare code; `res://` and `user://` work) is read
    /// if it exists, otherwise the text is the code.
    pub layout: Option<String>,
    /// `--racer-season halloween|none`: force a season on or off whatever
    /// the date (DESIGN §5.2), for testing and screenshots. An unknown
    /// value is ignored.
    pub season: Option<crate::season::Season>,
    /// `--racer-perf`: show the debug frame-time overlay from launch, on a
    /// desktop too (`perf::Hud`). Debug builds only; a phone shows it anyway.
    pub perf: bool,
    /// `--racer-bench`: the soak benchmark (`perf::Bench`): the demo bot on
    /// `perf::BENCH_SEED` with fixed fork picks, every frame written as CSV
    /// under `user://bench/`, then quit.
    pub bench: bool,
    /// `--racer-bench-seconds <s>`: how long `--racer-bench` runs;
    /// `perf::BENCH_SECONDS` when absent or not a positive number.
    pub bench_seconds: Option<f64>,
}

impl DevArgs {
    /// Read the flags out of `args`; anything else is ignored.
    pub fn parse(args: &[String]) -> DevArgs {
        DevArgs {
            layout: value(args, "--racer-layout"),
            season: value(args, "--racer-season")
                .as_deref()
                .and_then(crate::season::Season::parse),
            perf: flag(args, "--racer-perf"),
            bench: flag(args, "--racer-bench"),
            bench_seconds: value(args, "--racer-bench-seconds")
                .and_then(|v| v.parse().ok())
                .filter(|s: &f64| s.is_finite() && *s > 0.0),
        }
    }
}

/// Whether the valueless `flag` is present, exactly.
fn flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// The value of `flag` in `args`, if given one.
fn value(args: &[String], flag: &str) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix(flag) {
            if let Some(v) = v.strip_prefix('=') {
                return Some(v.to_string());
            }
            if v.is_empty() {
                return args.get(i + 1).filter(|n| !n.starts_with("--")).cloned();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_layout_flag_takes_a_value_either_way() {
        let p = |s| DevArgs::parse(&args(s)).layout;
        assert_eq!(p("--racer-layout RL04 --x"), Some("RL04".into()));
        assert_eq!(
            p("-- --racer-layout=res://c.txt"),
            Some("res://c.txt".into())
        );
        assert_eq!(p("--racer-layout --other"), None);
        assert_eq!(p("--racer-layoutX y"), None);
        assert_eq!(p(""), None);
        assert_eq!(p("--racer-layout A --racer-layout B"), Some("A".into()));
    }

    #[test]
    fn the_season_flag_forces_a_season_on_or_off() {
        use crate::season::Season;
        let p = |s| DevArgs::parse(&args(s)).season;
        assert_eq!(p("-- --racer-season halloween"), Some(Season::Halloween));
        assert_eq!(p("--racer-season=none"), Some(Season::None));
        assert_eq!(p("--racer-season spring"), None);
        assert_eq!(p("--racer-layout RL04"), None);
        let both = DevArgs::parse(&args("--racer-layout RL04 --racer-season=halloween"));
        assert_eq!(both.layout, Some("RL04".into()));
        assert_eq!(both.season, Some(Season::Halloween));
    }

    #[test]
    fn the_bench_flags_parse_and_a_bad_length_is_ignored() {
        let a = DevArgs::parse(&args(
            "-- --racer-bench --racer-bench-seconds 30 --racer-perf",
        ));
        assert!(a.bench && a.perf);
        assert_eq!(a.bench_seconds, Some(30.0));
        let b = DevArgs::parse(&args("--racer-bench-seconds=-5 --racer-benchX"));
        assert!(!b.bench && !b.perf);
        assert_eq!(b.bench_seconds, None);
        assert_eq!(DevArgs::parse(&[]), DevArgs::default());
    }
}
