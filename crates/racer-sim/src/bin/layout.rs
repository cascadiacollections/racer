//! Host tools for layouts. DESIGN §9.
//!
//! ```text
//! layout export --seed S [--index I] [--branch B] [--name NAME]
//! layout prove <code|file> [--name NAME]
//! layout show <code|file>
//! layout check [--verify] <code|file>...
//! layout survey [--seeds N]
//! layout bundled
//! ```
//!
//! - `export` forks generated track `(S, I, B)` (run seed in decimal or
//!   `0x` hex) into a layout and prints it as a layout file: its draft code
//!   (`RD…`) and its `origin` line (the provenance, kept beside the code).
//! - `prove` proves a layout under the custom-race rules, prints the gate
//!   report and the wall time, and on success the proven file: the share
//!   code (`RL…`) and the origin it came with. Exit status 1 if it fails.
//! - `show` prints the layout's summary and the ASCII strip (`viz.rs`).
//! - `check` decodes each argument, lists its problems and its proof's
//!   standing; with `--verify`, proves each proven one again and requires
//!   the same timers. Exit status 1 if any is unreadable, has a problem, or
//!   (with `--verify`) fails its proof.
//! - `survey` forks every track of N run seeds (the batch's schedule) and
//!   reports code sizes, and how many prove under the custom rules, how
//!   long proving takes and why the rest fail: the numbers DESIGN §9 and
//!   NOTES.md quote.
//!
//! - `bundled` lists the bundled courses (`layout/bundled.rs`) with their
//!   origins and tuning, and verifies each (proves it again and requires
//!   the timers it carries). Exit status 1 if one does not.
//!
//! A `<code|file>` is a path if one exists, otherwise the text itself.
//! Exit status 2 on a bad flag.

use std::time::Instant;

use racer_sim::layout::{fork, Layout, LayoutFile, ProveError, Standing, Verified, RULES};
use racer_sim::rng::mix64;
use racer_sim::solver::Report;
use racer_sim::viz::ascii_strip;

fn usage(msg: &str) -> ! {
    eprintln!("{msg}");
    eprintln!(
        "usage: layout export --seed S [--index I] [--branch B] [--name NAME]\n       \
         layout prove <code|file> [--name NAME]\n       \
         layout show <code|file>\n       \
         layout check [--verify] <code|file>...\n       \
         layout survey [--seeds N]\n       \
         layout bundled"
    );
    std::process::exit(2);
}

fn parse_u64(s: &str) -> u64 {
    let v = match s.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => s.parse(),
    };
    v.unwrap_or_else(|_| usage(&format!("not a number: {s}")))
}

/// The text of `arg`: a file's contents if it names one, else itself.
fn read(arg: &str) -> String {
    std::fs::read_to_string(arg).unwrap_or_else(|_| arg.to_string())
}

/// A layout file from `arg`, decoded.
fn load(arg: &str) -> Result<(Layout, LayoutFile), String> {
    let text = read(arg);
    let file = LayoutFile::parse(&text).ok_or("no RL or RD code found")?;
    let layout = Layout::from_code(&file.code).map_err(|e| e.to_string())?;
    Ok((layout, file))
}

fn summary(l: &Layout) -> String {
    let mut s = format!(
        "name {:?}\nsegments {} ({:.0} m to the finish), gates {:?}\nobjects {}, traffic {}\ndigest {:#018x}\nstanding {:?}",
        l.name,
        l.segments.len(),
        l.length(),
        l.checkpoints,
        l.objects.len(),
        l.traffic.len(),
        l.hash(),
        l.standing(),
    );
    if let Some(p) = &l.proof {
        let secs: Vec<String> = p
            .allot_ticks
            .iter()
            .map(|&t| format!("{:.2}", t as f64 / racer_sim::TICKS_PER_SEC as f64))
            .collect();
        s.push_str(&format!(
            "\ntimers {} s, epoch {}, stamp {:08x}",
            secs.join(" + "),
            p.epoch,
            p.stamp
        ));
    }
    s
}

fn report_text(r: &Report) -> String {
    let s = |t: u32| t as f64 / racer_sim::TICKS_PER_SEC as f64;
    let mut out = format!(
        "calibrated on {}\nperfect bot {:.2} s, {:.2} s left (band 6-18)\nhuman median {:.2} s, skill gap {:.1}% (band 8-30%)\nworst margin {:.2} s, fewest passes {}/16\n",
        r.calibrated_on.describe(),
        s(r.perfect_ticks),
        s(r.perfect_left),
        s(r.human_ticks),
        r.skill_gap * 100.0,
        s(r.worst_margin),
        r.fewest_passes,
    );
    for d in &r.drift {
        out.push_str(&format!(
            "drift corner at segment {}: grip-only loses {:.2} s{}\n",
            d.start,
            d.loss,
            if d.grip_failed {
                " (ran out of time)"
            } else {
                ""
            }
        ));
    }
    out.push_str(&format!("{} ticks simulated", r.cost));
    out
}

fn export(args: &[String]) {
    let (mut seed, mut index, mut branch, mut name) = (None, 0, 0, String::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_else(|| usage("missing value"));
        match a.as_str() {
            "--seed" => seed = Some(parse_u64(&val())),
            "--index" => index = parse_u64(&val()) as u32,
            "--branch" => branch = parse_u64(&val()) as u8,
            "--name" => name = val().to_ascii_uppercase(),
            other => usage(&format!("unknown flag {other}")),
        }
    }
    let seed = seed.unwrap_or_else(|| usage("--seed is required"));
    if index >= racer_sim::gen::difficulty::TRACKS || branch > 1 || (index == 0 && branch == 1) {
        usage("no such track");
    }
    let f = fork(seed, index, branch);
    let mut layout = f.layout;
    layout.name = name;
    if let Err(p) = layout.structure() {
        usage(&format!("bad name: {p:?}"));
    }
    let code = layout.to_code().expect("a fork is structurally sound");
    eprintln!("{}", summary(&layout));
    eprintln!(
        "generated on rung {:?}; code {} characters",
        f.rung,
        code.len()
    );
    print!(
        "{}",
        LayoutFile {
            code,
            origin: Some(f.provenance.to_string()),
        }
        .encode()
    );
}

fn prove(args: &[String]) {
    let (mut src, mut name) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => {
                name = Some(
                    it.next()
                        .unwrap_or_else(|| usage("missing value"))
                        .to_ascii_uppercase(),
                )
            }
            other if src.is_none() => src = Some(other.to_string()),
            other => usage(&format!("unexpected {other}")),
        }
    }
    let src = src.unwrap_or_else(|| usage("prove what?"));
    let (mut layout, file) = load(&src).unwrap_or_else(|e| {
        eprintln!("{src}: {e}");
        std::process::exit(1)
    });
    if let Some(n) = name {
        layout.name = n;
    }
    eprintln!("rules: {RULES}");
    let t0 = Instant::now();
    let result = layout.unproven().prove();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    match result {
        Ok((proven, report)) => {
            eprintln!("{}", report_text(&report));
            eprintln!("PROVEN in {ms:.0} ms");
            eprintln!("{}", summary(&proven));
            let code = proven.to_code().expect("proven layouts encode");
            eprintln!("share code {} characters", code.len());
            print!(
                "{}",
                LayoutFile {
                    code,
                    origin: file.origin,
                }
                .encode()
            );
        }
        Err(e) => {
            eprintln!("NOT PROVEN in {ms:.0} ms: {e}");
            std::process::exit(1);
        }
    }
}

fn show(args: &[String]) {
    let [src] = args else {
        usage("show one layout")
    };
    let (layout, file) = load(src).unwrap_or_else(|e| {
        eprintln!("{src}: {e}");
        std::process::exit(1)
    });
    println!("{}", summary(&layout));
    if let Some(o) = &file.origin {
        println!("origin {o}");
    }
    let problems = layout.check();
    if !problems.is_empty() {
        println!("problems {problems:?}");
    }
    match racer_sim::layout::race_track(&layout) {
        Ok((t, _)) => print!("{}", ascii_strip(&t)),
        Err(p) => println!("cannot build: {p:?}"),
    }
}

fn check_all(args: &[String]) {
    let verify = args.iter().any(|a| a == "--verify");
    let files: Vec<&String> = args.iter().filter(|a| *a != "--verify").collect();
    if files.is_empty() {
        usage("check what?");
    }
    let mut bad = 0;
    for src in files {
        let (layout, _) = match load(src) {
            Ok(l) => l,
            Err(e) => {
                println!("{src}: {e}");
                bad += 1;
                continue;
            }
        };
        let problems = layout.check();
        let standing = layout.standing();
        let mut line = format!(
            "{src}: {:?} {} segments, {:?}",
            layout.name,
            layout.segments.len(),
            standing
        );
        if !problems.is_empty() {
            line.push_str(&format!(", problems {problems:?}"));
            bad += 1;
        }
        if verify && standing != Standing::Unproven {
            let t0 = Instant::now();
            let v = layout.verify();
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            line.push_str(&format!(", verify {ms:.0} ms: "));
            match v {
                Verified::Confirmed => line.push_str("confirmed"),
                Verified::Reproved(_) => {
                    line.push_str("proves to other timers");
                    bad += 1;
                }
                Verified::Failed(e) => {
                    line.push_str(&format!("fails: {e}"));
                    bad += 1;
                }
                Verified::Unproven => {}
            }
        }
        println!("{line}");
    }
    if bad > 0 {
        std::process::exit(1);
    }
}

fn survey(args: &[String]) {
    let mut seeds = 20u64;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seeds" => seeds = parse_u64(it.next().unwrap_or_else(|| usage("missing value"))),
            other => usage(&format!("unknown flag {other}")),
        }
    }
    let mut keys = Vec::new();
    for n in 0..seeds {
        let seed = mix64(0xBA7C_0000_0000_0000 ^ n);
        for index in 0..racer_sim::gen::difficulty::TRACKS {
            for branch in 0..if index == 0 { 1 } else { 2 } {
                keys.push((seed, index, branch));
            }
        }
    }
    type Row = (u32, usize, usize, Result<(), String>, f64);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let rows: std::sync::Mutex<Vec<(usize, Row)>> = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(&(seed, index, branch)) = keys.get(i) else {
                    break;
                };
                let layout = fork(seed, index, branch).layout;
                let draft = layout.to_code().unwrap().len();
                let t0 = Instant::now();
                let (result, share) = match layout.prove() {
                    Ok((p, _)) => (Ok(()), p.to_code().unwrap().len()),
                    Err(ProveError::Failed(f, _)) => (Err(format!("{f:?}")), 0),
                    Err(ProveError::Problems(p)) => (Err(format!("{p:?}")), 0),
                };
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                rows.lock()
                    .unwrap()
                    .push((i, (index, draft, share, result, ms)));
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let rows: Vec<Row> = rows.into_iter().map(|r| r.1).collect();
    let pct = |v: &mut Vec<f64>, q: f64| {
        v.sort_by(f64::total_cmp);
        v[((v.len() - 1) as f64 * q) as usize]
    };
    let mut drafts: Vec<f64> = rows.iter().map(|r| r.1 as f64).collect();
    let mut shares: Vec<f64> = rows
        .iter()
        .filter(|r| r.2 > 0)
        .map(|r| r.2 as f64)
        .collect();
    let mut ms: Vec<f64> = rows.iter().map(|r| r.4).collect();
    println!(
        "{} tracks from {seeds} run seeds; rules: {RULES}",
        rows.len()
    );
    println!(
        "draft code characters: median {:.0}, p95 {:.0}, max {:.0}",
        pct(&mut drafts, 0.5),
        pct(&mut drafts, 0.95),
        pct(&mut drafts, 1.0)
    );
    if !shares.is_empty() {
        println!(
            "share code characters: median {:.0}, max {:.0}",
            pct(&mut shares, 0.5),
            pct(&mut shares, 1.0)
        );
    }
    println!(
        "prove wall time (ms, {threads} threads busy): median {:.0}, p95 {:.0}, max {:.0}",
        pct(&mut ms, 0.5),
        pct(&mut ms, 0.95),
        pct(&mut ms, 1.0)
    );
    for index in 0..racer_sim::gen::difficulty::TRACKS {
        let of: Vec<&Row> = rows.iter().filter(|r| r.0 == index).collect();
        let ok = of.iter().filter(|r| r.3.is_ok()).count();
        let mut why = std::collections::BTreeMap::<String, usize>::new();
        for r in &of {
            if let Err(e) = &r.3 {
                let key = e.split(['(', ' ', '{']).next().unwrap_or(e).to_string();
                *why.entry(key).or_default() += 1;
            }
        }
        println!(
            "track {}: {ok}/{} prove; failures {why:?}",
            index + 1,
            of.len()
        );
    }
}

fn bundled() {
    use racer_sim::layout::bundled::COURSES;
    let mut bad = 0;
    for c in COURSES {
        let layout = match Layout::from_code(c.code) {
            Ok(l) => l,
            Err(e) => {
                println!("unreadable: {e}");
                bad += 1;
                continue;
            }
        };
        let t0 = Instant::now();
        let v = layout.verify();
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let ok = v == Verified::Confirmed;
        bad += usize::from(!ok);
        println!(
            "{:<12} {} chars, {:.0} m, {:?}, verify {ms:.0} ms: {}\n  origin {}\n  tuning {}",
            layout.name,
            c.code.len(),
            layout.length(),
            layout.standing(),
            if ok {
                "confirmed".to_string()
            } else {
                format!("{v:?}")
            },
            c.origin,
            c.tuning
        );
    }
    if bad > 0 {
        std::process::exit(1);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        usage("which command?")
    };
    match cmd.as_str() {
        "export" => export(rest),
        "prove" => prove(rest),
        "show" => show(rest),
        "check" => check_all(rest),
        "survey" => survey(rest),
        "bundled" => bundled(),
        other => usage(&format!("unknown command {other}")),
    }
}
