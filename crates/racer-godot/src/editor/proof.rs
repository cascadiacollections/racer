//! PROVE on the builder thread, and what it found in plain words (DESIGN
//! §9.11). Engine-free.
//!
//! ```text
//!               PROVE                  result for this road: passed
//!   Unproven ─────────► Proving(job) ─────────────────────────────► Proven
//!      ▲  ▲                  │ failed                                  │
//!      │  │                  ▼                                         │
//!      │  └─── edit ──── Failed(words) ◄──── (a verify that failed) ──┐ │
//!      └────────────── any edit to the road ◄─────────────────────────┼─┘
//!   paste / open a code with an untrusted proof ──► Verifying(job) ───┘
//! ```
//!
//! The UI never waits: PROVE sends the layout to the worker and returns;
//! PROVING… shows until the worker answers. An edit while proving makes the
//! answer stale: it is ignored when it comes (it carries a job number, and
//! its road's digest must match the draft's), so a proof can never land on
//! a road it was not made for. A pasted share code's proof is not trusted:
//! VERIFYING proves it again, and it races with a timer only once that
//! comes back (with this build's timers if they differ); if it fails, the
//! draft has no timer.

use racer_sim::layout::{Layout, Problem, ProveError, Standing};
use racer_sim::solver::{Failure, Report};

use crate::builder::{Build, Proved};
use crate::render::secs;

/// Where the draft's proof stands.
#[derive(Clone, Debug, PartialEq)]
pub enum ProofState {
    /// No proof: free drive, copies as a draft code.
    Unproven,
    /// PROVE pressed: job `job` is on the worker.
    Proving(u64),
    /// A pasted or opened code's proof being proven again: job `job`.
    Verifying(u64),
    /// Proven by this build: the draft carries its timers.
    Proven(Summary),
    /// The last proof failed: why, in words.
    Failed(Vec<String>),
}

/// What a passed proof found, for the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// The timer, ticks: every section's allotment.
    pub timer: u32,
    /// The perfect bot's time, ticks; 0 when not known (a stamp trusted).
    pub perfect: u32,
    /// A typical (median) driver's time, ticks; 0 when not known.
    pub typical: u32,
}

impl Summary {
    /// The summary of `layout`'s proof and, if there is one, the report.
    pub fn of(layout: &Layout, report: Option<&Report>) -> Summary {
        Summary {
            timer: layout
                .proof
                .as_ref()
                .map_or(0, |p| p.allot_ticks.iter().sum()),
            perfect: report.map_or(0, |r| r.perfect_ticks),
            typical: report.map_or(0, |r| r.human_ticks),
        }
    }

    /// One line: `TIMER 71.5S  PERFECT 58.2S`.
    pub fn line(&self) -> String {
        if self.perfect == 0 {
            return format!("TIMER {}S", secs(self.timer));
        }
        format!(
            "TIMER {}S  BEST {}S  TYPICAL {}S",
            secs(self.timer),
            secs(self.perfect),
            secs(self.typical)
        )
    }
}

/// The proof state machine and its job numbers.
#[derive(Clone, Debug)]
pub struct Prover {
    /// Where the proof stands.
    pub state: ProofState,
    next_job: u64,
    /// The digest of the road the job in flight is for.
    road: u64,
}

impl Default for Prover {
    fn default() -> Self {
        Prover {
            state: ProofState::Unproven,
            next_job: 1,
            road: 0,
        }
    }
}

/// What a finished proof meant, for a note.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Proven: the layout to keep (with its proof).
    Proven(Layout),
    /// A pasted proof proven again to the timers it carried.
    Confirmed,
    /// A pasted proof proven again to other timers: the layout to keep.
    Reproved(Layout),
    /// Failed; the draft keeps no proof.
    Failed,
}

impl Prover {
    /// The state for a draft just loaded: proven if its stamp is this
    /// build's and `trusted` (saved by this build, bundled), else unproven
    /// (a draft) or in need of checking ([`Prover::verify`]).
    pub fn loaded(layout: &Layout, trusted: bool) -> (Prover, bool) {
        let mut p = Prover::default();
        let needs = match layout.standing() {
            Standing::Unproven => false,
            Standing::Proven if trusted => {
                p.state = ProofState::Proven(Summary::of(layout, None));
                false
            }
            _ => true,
        };
        (p, needs)
    }

    /// Whether a proof is under way.
    pub fn busy(&self) -> bool {
        matches!(
            self.state,
            ProofState::Proving(_) | ProofState::Verifying(_)
        )
    }

    /// PROVE: send `layout`'s road to the worker. Does nothing if proving
    /// already, or if the layout is proven.
    pub fn prove(&mut self, layout: &Layout, build: &mut dyn Build) -> bool {
        if self.busy() || matches!(self.state, ProofState::Proven(_)) {
            return false;
        }
        let job = self.send(layout, build);
        self.state = ProofState::Proving(job);
        true
    }

    /// Check an imported proof: prove the road again.
    pub fn verify(&mut self, layout: &Layout, build: &mut dyn Build) {
        let job = self.send(layout, build);
        self.state = ProofState::Verifying(job);
    }

    fn send(&mut self, layout: &Layout, build: &mut dyn Build) -> u64 {
        let job = self.next_job;
        self.next_job += 1;
        self.road = layout.hash();
        build.prove(job, layout.unproven());
        job
    }

    /// The road changed: whatever was proven or being proven no longer
    /// applies.
    pub fn edited(&mut self) {
        self.state = ProofState::Unproven;
    }

    /// A proof came back: job `job`, `result`, for the draft `draft` (whose
    /// proof it set, carried, or which carried none). Returns what it meant,
    /// or `None` for a stale answer (another job, or another road).
    pub fn finished(&mut self, job: u64, result: Proved, draft: &Layout) -> Option<Outcome> {
        let verifying = match self.state {
            ProofState::Proving(j) if j == job => false,
            ProofState::Verifying(j) if j == job => true,
            _ => return None,
        };
        if draft.hash() != self.road {
            // The road changed under it (an edit not reported): drop it.
            self.state = ProofState::Unproven;
            return None;
        }
        match result {
            Ok((proven, report)) => {
                let mut keep = draft.clone();
                keep.proof = proven.proof;
                self.state = ProofState::Proven(Summary::of(&keep, Some(&report)));
                Some(if !verifying {
                    Outcome::Proven(keep)
                } else if keep.proof == draft.proof {
                    Outcome::Confirmed
                } else {
                    Outcome::Reproved(keep)
                })
            }
            Err(e) => {
                self.state = ProofState::Failed(words(&e, draft));
                Some(Outcome::Failed)
            }
        }
    }

    /// The badge for the top bar.
    pub fn badge(&self) -> &'static str {
        match self.state {
            ProofState::Unproven => "DRAFT",
            ProofState::Proving(_) => "PROVING...",
            ProofState::Verifying(_) => "VERIFYING...",
            ProofState::Proven(_) => "PROVEN",
            ProofState::Failed(_) => "NOT PROVEN",
        }
    }
}

/// Where segment `seg` is, in metres, for a message.
fn at(seg: u32) -> String {
    format!("{} M", seg * 4)
}

/// A problem in a few words.
pub fn problem_words(p: Problem, layout: &Layout) -> String {
    let obj = |i: u32| layout.objects.get(i as usize).map_or(0, |o| o.seg);
    let car = |i: u32| layout.traffic.get(i as usize).map_or(0, |c| c.z0_dm / 10);
    match p {
        Problem::TooShort => "TOO SHORT: 500 M AT LEAST".into(),
        Problem::NoLaunch => "FIRST GATE NEEDS 100 M OF RUN-UP".into(),
        Problem::Curve(i) => format!("TOO SHARP AT {}", at(i)),
        Problem::Pitch(i) => format!("TOO STEEP AT {}", at(i)),
        Problem::Width(i) => format!("TOO NARROW OR WIDE AT {}", at(i)),
        Problem::ObjectOffRoad(i) => format!("OBJECT OFF THE ROAD AT {}", at(obj(i))),
        Problem::TrafficStart(i) => format!("CAR AT {} M: TOO EARLY OR PAST FINISH", car(i)),
        Problem::TrafficSpeed(i) => format!("CAR AT {} M TOO SLOW OR FAST", car(i)),
        Problem::TrafficLane(i) => format!("CAR AT {} M IS OFF THE ROAD", car(i)),
        other => format!("NOT A LAYOUT ({other:?})").to_uppercase(),
    }
}

/// Why a proof failed, in plain words: a line or two.
pub fn words(e: &ProveError, layout: &Layout) -> Vec<String> {
    match e {
        ProveError::Problems(ps) => {
            let mut out: Vec<String> = ps
                .iter()
                .take(2)
                .map(|&p| problem_words(p, layout))
                .collect();
            if ps.len() > 2 {
                out.push(format!("AND {} MORE", ps.len() - 2));
            }
            out
        }
        ProveError::Failed(f, _) => {
            let band =
                racer_sim::layout::perfect_band(layout.checkpoints.last().copied().unwrap_or(0));
            match *f {
                Failure::Undrivable => vec!["A TYPICAL DRIVER COULD NOT FINISH".into()],
                Failure::CrashProne => vec![
                    "TOO MANY CRASHES".into(),
                    "CLEAR THE ROAD OR WIDEN IT".into(),
                ],
                Failure::PerfectFailed => vec!["EVEN A PERFECT RUN RAN OUT OF TIME".into()],
                Failure::PerfectBand(left) if left < band.0 => vec![
                    format!("TOO TIGHT: A PERFECT RUN HAS {}S LEFT", secs(left)),
                    format!("IT NEEDS {}S IN HAND", secs(band.0)),
                ],
                Failure::PerfectBand(left) => vec![
                    format!("TOO EASY: A PERFECT RUN HAS {}S LEFT", secs(left)),
                    "ADD BENDS, HILLS OR HAZARDS".into(),
                ],
                Failure::SkillGap(g) if g < racer_sim::gen::difficulty::SKILL_GAP.0 => vec![
                    "TOO PLAIN: SKILL MAKES NO DIFFERENCE".into(),
                    "ADD CORNERS OR CRESTS".into(),
                ],
                Failure::SkillGap(_) => vec![
                    "TOO WILD: LUCK BEATS SKILL".into(),
                    "EASE THE HARDEST CORNERS".into(),
                ],
                Failure::DriftCorner => vec![
                    "A DRIFT CORNER IS TOO GENTLE".into(),
                    "SHARPEN IT OR UNMARK IT".into(),
                ],
                Failure::Margins { section, .. } => vec![
                    format!("GATE {} IS TOO TIGHT", section + 1),
                    "FOR A TYPICAL DRIVER".into(),
                ],
            }
        }
    }
}

/// Notes on a draft: what would stop it proving, and what is worth a
/// look. Never a block: the draft can always be driven.
pub fn notes(layout: &Layout) -> Vec<String> {
    let mut out: Vec<String> = layout
        .check()
        .into_iter()
        .map(|p| problem_words(p, layout))
        .collect();
    let km = layout.length() / 1000.0;
    if km > 4.0 {
        out.push(format!("LONG ROAD: {km:.1} KM, SLOW TO PROVE"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::Inline;
    use racer_sim::layout::bundled;
    use racer_sim::track::fixtures;

    fn worker() -> Inline<impl FnMut(crate::builder::Key) -> racer_sim::track::Track> {
        Inline::new(|_| fixtures::drag_strip())
    }

    fn course() -> Layout {
        bundled::layouts().remove(0)
    }

    #[test]
    fn prove_runs_on_the_worker_and_lands_on_the_road_it_was_made_for() {
        let draft = course().unproven();
        let mut w = worker();
        w.hold = true;
        let mut p = Prover::default();
        assert!(p.prove(&draft, &mut w));
        assert_eq!(p.state, ProofState::Proving(1));
        assert_eq!(p.badge(), "PROVING...");
        assert!(!p.prove(&draft, &mut w), "one at a time");
        assert!(w.proofs().is_empty(), "the worker is slow: nothing yet");
        w.hold = false;
        let (job, result) = w.proofs().remove(0);
        match p.finished(job, result, &draft) {
            Some(Outcome::Proven(l)) => {
                assert_eq!(l, course(), "the bundled timers");
                assert_eq!(l.standing(), Standing::Proven);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&p.state, ProofState::Proven(s) if s.perfect > 0 && s.timer > s.perfect));
        assert!(!p.prove(&course(), &mut w), "already proven");
    }

    #[test]
    fn an_edit_while_proving_makes_the_answer_stale() {
        let draft = course().unproven();
        let mut w = worker();
        let mut p = Prover::default();
        p.prove(&draft, &mut w);
        p.edited();
        assert_eq!(p.state, ProofState::Unproven);
        let (job, result) = w.proofs().remove(0);
        assert_eq!(p.finished(job, result.clone(), &draft), None, "stale job");
        // Proving again, but the road changed before the answer came.
        p.prove(&draft, &mut w);
        let mut edited = draft.clone();
        edited.segments[30].curve_milli += 5;
        let (job, result) = w.proofs().remove(0);
        assert_eq!(p.finished(job, result, &edited), None, "another road");
        // A rename is not another road.
        p.prove(&draft, &mut w);
        let renamed = Layout {
            name: "RENAMED".into(),
            ..draft
        };
        let (job, result) = w.proofs().remove(0);
        assert!(matches!(
            p.finished(job, result, &renamed),
            Some(Outcome::Proven(l)) if l.name == "RENAMED"
        ));
    }

    #[test]
    fn a_pasted_proof_is_verified_and_a_failure_takes_the_timer_away() {
        let mut w = worker();
        // Trusted (saved by this build, bundled): proven at once.
        let (p, needs) = Prover::loaded(&course(), true);
        assert!(!needs && matches!(p.state, ProofState::Proven(_)));
        // Pasted: verify.
        let (mut p, needs) = Prover::loaded(&course(), false);
        assert!(needs);
        p.verify(&course(), &mut w);
        assert_eq!(p.badge(), "VERIFYING...");
        let (job, result) = w.proofs().remove(0);
        assert_eq!(p.finished(job, result, &course()), Some(Outcome::Confirmed));
        // A loosened timer: proven again to this build's.
        let mut loose = course();
        loose.proof.as_mut().unwrap().allot_ticks[0] += 120;
        let (mut p, needs) = Prover::loaded(&loose, true);
        assert!(needs, "a bad stamp is never trusted");
        p.verify(&loose, &mut w);
        let (job, result) = w.proofs().remove(0);
        assert_eq!(
            p.finished(job, result, &loose),
            Some(Outcome::Reproved(course()))
        );
        // A proof that does not hold: failed, in words.
        let mut w = worker().proving_with(|_| {
            Err(ProveError::Failed(
                Failure::Margins {
                    passes: 3,
                    section: 1,
                },
                9,
            ))
        });
        let (mut p, _) = Prover::loaded(&course(), false);
        p.verify(&course(), &mut w);
        let (job, result) = w.proofs().remove(0);
        assert_eq!(p.finished(job, result, &course()), Some(Outcome::Failed));
        assert_eq!(
            p.state,
            ProofState::Failed(vec![
                "GATE 2 IS TOO TIGHT".into(),
                "FOR A TYPICAL DRIVER".into()
            ])
        );
        assert_eq!(p.badge(), "NOT PROVEN");
    }

    #[test]
    fn every_failure_has_words_that_fit_the_screen() {
        let l = course();
        let fails = [
            Failure::Undrivable,
            Failure::CrashProne,
            Failure::PerfectFailed,
            Failure::PerfectBand(100),
            Failure::PerfectBand(5000),
            Failure::SkillGap(0.01),
            Failure::SkillGap(0.5),
            Failure::DriftCorner,
            Failure::Margins {
                passes: 1,
                section: 0,
            },
        ];
        for f in fails {
            let w = words(&ProveError::Failed(f, 0), &l);
            assert!(!w.is_empty());
            for line in &w {
                assert!(crate::font::small_width(line) <= 172, "{line}");
            }
        }
        let mut bad = l.unproven();
        bad.segments[3].curve_milli = 999;
        bad.segments[9].pitch_milli = 999;
        bad.segments[12].half_width_cm = 10;
        let w = words(&ProveError::Problems(bad.check()), &bad);
        assert_eq!(
            w,
            vec!["TOO SHARP AT 12 M", "TOO STEEP AT 36 M", "AND 1 MORE"]
        );
        assert_eq!(notes(&bad).len(), 3);
        assert!(notes(&l).is_empty());
    }
}
