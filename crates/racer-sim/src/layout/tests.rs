use proptest::prelude::*;

use super::code::{DRAFT_PREFIX, SHARE_PREFIX};
use super::*;
use crate::codec::{base32, crc16, ALPHABET};
use crate::input::Input;
use crate::race::Race;
use crate::run::{state_hash, RunPhase, RunState};
use crate::track::{fixtures, ObjectKind, Surface};

#[test]
fn a_layouts_ghost_is_kept_under_its_road_and_survives_the_file() {
    use crate::ghost::{Ghost, GhostStore};
    let l = hand_made();
    let key = l.ghost_key();
    assert_eq!(key, (l.hash(), GHOST_INDEX, 0));
    let renamed = Layout {
        name: "X".into(),
        ..l.clone()
    };
    assert_eq!(renamed.ghost_key(), key);
    let mut store = GhostStore::new();
    let g = Ghost {
        loadout: CUSTOM_LOADOUT,
        ticks: 100,
        log: Vec::new(),
    };
    assert!(store.offer(key, g.clone()));
    let back = GhostStore::decode(&store.encode(crate::SIM_EPOCH), crate::SIM_EPOCH);
    assert_eq!(back.get(key), Some(&g));
}

/// A hand-made layout: the sweep-and-hairpin fixture with a name.
fn hand_made() -> Layout {
    Layout {
        name: "HAIRPIN".into(),
        ..Layout::from_track(&fixtures::sweep_and_hairpin())
    }
}

/// The first bundled course, without its proof: a layout that proves.
fn course() -> Layout {
    bundled::layouts()[0].unproven()
}

/// A scripted driver: weave, brake now and then, drift on a pattern.
fn script(n: u32) -> Input {
    let steer = [0, 1, 2, 3, 0, -1, -2, -3][(n / 37 % 8) as usize];
    Input::new(steer, n % 211 < 20, n % 401 < 60)
}

/// Drive a one-track run with [`script`] to its end and hash it.
fn drive_hash(key: u64, track: Arc<Track>) -> u64 {
    let mut run = RunState::single(key, track, CUSTOM_LOADOUT);
    let mut n = 0;
    while run.phase() == RunPhase::Racing {
        run.step(script(n));
        n += 1;
    }
    state_hash(&run)
}

#[test]
fn a_fork_is_the_generated_road_exactly_and_races_identically() {
    for (seed, index, branch) in [(1, 0, 0), (7, 2, 1), (3, 4, 1)] {
        let built = build(seed, index, branch, Effort::default());
        let f = fork(seed, index, branch);
        assert_eq!(f.provenance.to_string().split(':').count(), 5);
        // Decoded from its code, the layout builds the generated track
        // exactly once the generated timers are put back.
        let decoded = Layout::from_code(&f.layout.to_code().unwrap()).unwrap();
        assert_eq!(decoded, f.layout);
        let mut t = decoded.track().unwrap();
        t.set_allotments(built.track.allot_ticks().to_vec());
        assert_eq!(t, *built.track);
        assert_eq!(
            crate::track::track_hash(&t),
            crate::track::track_hash(&built.track)
        );
        // So the same inputs drive to the same state hash.
        assert_eq!(
            drive_hash(9, Arc::new(t)),
            drive_hash(9, built.track.clone())
        );
    }
}

#[test]
fn a_fork_has_no_problems_and_its_code_is_within_budget() {
    for seed in 0..6 {
        for index in 0..crate::gen::difficulty::TRACKS {
            let l = fork(seed, index, 1).layout;
            assert_eq!(l.check(), vec![], "{seed} {index}");
            let code = l.to_code().unwrap();
            assert!(code.starts_with(DRAFT_PREFIX));
            // DESIGN §9.5: typical codes are a few hundred characters,
            // every one under 1 KB.
            assert!(code.len() < 1024, "{} characters", code.len());
        }
    }
}

#[test]
fn an_unproven_layout_drives_with_no_timer() {
    let l = hand_made();
    assert_eq!(l.standing(), Standing::Unproven);
    assert!(l.timed_track().is_none());
    let (track, timed) = race_track(&l).unwrap();
    assert!(!timed);
    assert!(track.allot_ticks().is_empty());
    // Untimed is the race's own cap, as calibration drives.
    let race = Race::new(track, CUSTOM_LOADOUT.params());
    assert_eq!(race.timer(), crate::race::MAX_RACE_TICKS);
    assert_eq!(l.verify(), Verified::Unproven);
}

#[test]
fn a_proof_is_stamped_and_the_stamp_is_checked() {
    let (proven, report) = course().prove().expect("the course proves");
    assert_eq!(report.loadouts, 1);
    assert_eq!(report.calibrated_on, CUSTOM_LOADOUT);
    assert_eq!(proven.standing(), Standing::Proven);
    let p = proven.proof.clone().unwrap();
    assert_eq!(p.epoch, crate::SIM_EPOCH);
    assert_eq!(p.allot_ticks.len(), proven.checkpoints.len());

    // Proven: timed, shared as RL, and it reads back proven.
    let (track, timed) = race_track(&proven).unwrap();
    assert!(timed);
    assert_eq!(track.allot_ticks(), &p.allot_ticks[..]);
    let code = proven.to_code().unwrap();
    assert!(code.starts_with(SHARE_PREFIX));
    let back = Layout::from_code(&code).unwrap();
    assert_eq!(back, proven);
    assert_eq!(back.standing(), Standing::Proven);
    assert_eq!(back.verify(), Verified::Confirmed);

    // Renaming keeps the proof: the name is not the road.
    let renamed = Layout {
        name: "OTHER".into(),
        ..proven.clone()
    };
    assert_eq!(renamed.standing(), Standing::Proven);

    // An edit after proving breaks the stamp, and races untimed.
    let mut edited = proven.clone();
    edited.segments[50].curve_milli += 1;
    assert_eq!(edited.check(), vec![]);
    assert_eq!(edited.standing(), Standing::BadStamp);
    assert!(edited.timed_track().is_none());
    // Timers loosened by hand break it too.
    let mut loose = proven.clone();
    loose.proof.as_mut().unwrap().allot_ticks[0] += 120;
    assert_eq!(loose.standing(), Standing::BadStamp);
    assert!(matches!(loose.verify(), Verified::Reproved(l) if *l == proven));
    // Another epoch's proof is stale, not bad: prove again.
    let mut old = proven.clone();
    let hash = old.hash();
    let q = old.proof.as_mut().unwrap();
    q.epoch += 1;
    q.stamp = stamp(hash, &q.allot_ticks, q.epoch);
    assert_eq!(old.standing(), Standing::Stale);
    assert!(old.timed_track().is_none());
    assert!(matches!(old.verify(), Verified::Reproved(l) if *l == proven));
}

#[test]
fn proving_is_deterministic_and_ignores_an_old_proof() {
    let a = course().prove().unwrap().0;
    let b = a.prove().unwrap().0;
    assert_eq!(a, b);
}

#[test]
fn a_layout_that_cannot_be_driven_is_not_proven() {
    // A wall of rocks across the road a few segments past the first gate.
    let mut l = course();
    for x in [-5.0, -3.0, -1.0, 1.0, 3.0, 5.0] {
        l.objects.push(Object {
            seg: l.checkpoints[0] + 20,
            x_cm: (x * 100.0) as i16,
            kind: ObjectKind::Rock,
        });
    }
    l.objects.sort_by_key(|o| o.seg);
    assert_eq!(l.check(), vec![]);
    match l.prove() {
        Err(ProveError::Failed(f, cost)) => {
            assert!(cost > 0);
            assert!(
                matches!(f, Failure::CrashProne | Failure::Undrivable),
                "{f:?}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn problems_name_what_is_wrong() {
    let mut l = hand_made();
    l.segments[3].curve_milli = 400;
    l.segments[4].pitch_milli = -400;
    l.segments[5].half_width_cm = 100;
    l.objects.push(Object {
        seg: 199,
        x_cm: 2000,
        kind: ObjectKind::Post,
    });
    l.traffic.push(TrafficCar::new(20.0, 20.0, 0.0));
    l.traffic.push(TrafficCar::new(200.0, 90.0, 9.0));
    assert_eq!(
        l.check(),
        vec![
            Problem::Curve(3),
            Problem::Pitch(4),
            Problem::Width(5),
            Problem::ObjectOffRoad(0),
            Problem::TrafficStart(0),
            Problem::TrafficSpeed(1),
            Problem::TrafficLane(1),
        ]
    );
    assert!(matches!(l.prove(), Err(ProveError::Problems(p)) if p.len() == 7));

    // Structural problems come alone, and stop the build and the code.
    let mut s = hand_made();
    s.checkpoints = vec![50, 40];
    assert_eq!(s.check(), vec![Problem::Gates]);
    assert_eq!(s.track(), Err(Problem::Gates));
    assert_eq!(s.to_code(), Err(CodeError::Unencodable));
    let mut f = hand_made();
    f.segments[7].flags = flags::CREST;
    assert_eq!(f.structure(), Err(Problem::Flags(7)));
    for bad in [" A", "A ", "A  B", "a", "ABCDEFGHIJKLM", "Ä"] {
        assert!(!valid_name(bad), "{bad:?}");
    }
    assert!(valid_name("") && valid_name("RIVER RUN 2"));
    let mut short = hand_made();
    short.segments.truncate(100);
    short.checkpoints = vec![10, 99];
    assert_eq!(short.check(), vec![Problem::TooShort, Problem::NoLaunch]);
}

#[test]
fn a_hand_made_code_is_pinned() {
    // Moves only with LAYOUT_FORMAT: the road is fixed data.
    let code = hand_made().to_code().unwrap();
    assert_eq!(code, PINNED_DRAFT, "{} characters", code.len());
}

/// [`a_hand_made_code_is_pinned`]'s code.
const PINNED_DRAFT: &str =
    "RD047GE70224Y86Y8QZR4290080G0ZZ0010200J87Z51Z0F0A2RY8ZJ3ZMRZJ7J5H8F0FR1FZZWAW5QR09CZ40GC1DC1V9M";

#[test]
fn every_single_character_typo_is_caught() {
    let code = hand_made().to_code().unwrap();
    let body = &code.as_bytes()[DRAFT_PREFIX.len()..];
    for i in 0..body.len() {
        for &c in ALPHABET {
            if c == body[i] {
                continue;
            }
            let mut typo = code.clone().into_bytes();
            typo[DRAFT_PREFIX.len() + i] = c;
            let typo = String::from_utf8(typo).unwrap();
            assert!(Layout::from_code(&typo).is_err(), "{i} {}", c as char);
        }
    }
}

#[test]
fn codes_are_forgiven_what_retyping_does() {
    let l = hand_made();
    let code = l.to_code().unwrap();
    let loose = format!(
        "  {}-{}\n",
        code[..20].to_ascii_lowercase(),
        code[20..].replace('0', "O")
    );
    assert_eq!(Layout::from_code(&loose), Ok(l));
    assert_eq!(Layout::from_code("XX123"), Err(CodeError::Prefix));
    assert_eq!(Layout::from_code("RDU"), Err(CodeError::Character));
    assert_eq!(
        Layout::from_code(&"R".repeat(MAX_CODE_CHARS + 1)),
        Err(CodeError::TooLong)
    );
    // A draft's bytes under the share prefix (proof bit clear) are refused.
    let swapped = code.replacen(DRAFT_PREFIX, SHARE_PREFIX, 1);
    assert_eq!(Layout::from_code(&swapped), Err(CodeError::Malformed));
}

#[test]
fn layout_files_and_provenance_read_back() {
    let p = Provenance {
        run_seed: 0xDEAD_BEEF,
        index: 3,
        branch: 1,
        epoch: 1,
    };
    let text = p.to_string();
    assert_eq!(text, "fork:0x00000000deadbeef:3:1:e1");
    assert_eq!(Provenance::parse(&text), Some(p));
    for bad in [
        "fork:0xdeadbeef:3:1:e1",
        "fork:0x00000000deadbeef:3:1",
        "seed:1",
    ] {
        assert_eq!(Provenance::parse(bad), None, "{bad}");
    }
    let code = hand_made().to_code().unwrap();
    let f = LayoutFile {
        code: code.clone(),
        origin: Some(text),
    };
    let encoded = f.encode();
    assert_eq!(LayoutFile::parse(&encoded), Some(f));
    let commented = format!(
        "# a course\n\n{}\norigin later:format\n",
        code.to_ascii_lowercase()
    );
    let g = LayoutFile::parse(&commented).unwrap();
    assert_eq!(g.origin.as_deref(), Some("later:format"));
    assert_eq!(Layout::from_code(&g.code), Ok(hand_made()));
    assert_eq!(LayoutFile::parse("# nothing here\n"), None);
}

// ---------------------------------------------------------------- property

fn segment() -> impl Strategy<Value = Segment> {
    (
        prop_oneof![-200i16..200, any::<i16>()],
        prop_oneof![-300i16..300, any::<i16>()],
        prop_oneof![Just(600u16), Just(450u16), any::<u16>()],
        0usize..4,
        0u8..8,
    )
        .prop_map(|(c, p, w, s, f)| Segment {
            curve_milli: c,
            pitch_milli: p,
            half_width_cm: w,
            surface: Surface::ALL[s],
            flags: f & AUTHORED_FLAGS,
        })
}

fn runs_of_segments() -> impl Strategy<Value = Vec<Segment>> {
    // Runs of equal segments, as editors and generators make them.
    prop::collection::vec((segment(), 1usize..40), 1..30).prop_map(|runs| {
        runs.into_iter()
            .flat_map(|(s, n)| std::iter::repeat_n(s, n))
            .take(MAX_SEGMENTS)
            .collect()
    })
}

fn layout() -> impl Strategy<Value = Layout> {
    (
        runs_of_segments(),
        prop::collection::vec(any::<u32>(), 1..=MAX_GATES),
        prop::collection::vec((any::<u32>(), any::<i16>(), any::<bool>()), 0..40),
        prop::collection::vec((0..=MAX_TRAFFIC_Z0_DM, any::<u16>(), any::<i16>()), 0..10),
        prop::collection::vec(0usize..NAME_CHARSET.len() - 1, 0..=NAME_MAX),
        prop::option::of((
            prop::collection::vec(0..=crate::race::MAX_RACE_TICKS, MAX_GATES),
            0u32..1000,
            any::<u32>(),
        )),
    )
        .prop_map(|(segments, gates, objects, traffic, name, proof)| {
            let n = segments.len() as u32;
            let mut checkpoints: Vec<u32> = if n < 2 {
                Vec::new()
            } else {
                gates.iter().map(|g| 1 + g % (n - 1)).collect()
            };
            checkpoints.sort_unstable();
            checkpoints.dedup();
            let mut objects: Vec<Object> = objects
                .into_iter()
                .map(|(s, x, rock)| Object {
                    seg: s % n,
                    x_cm: x,
                    kind: if rock {
                        ObjectKind::Rock
                    } else {
                        ObjectKind::Post
                    },
                })
                .collect();
            objects.sort_by_key(|o| o.seg);
            let traffic = traffic
                .into_iter()
                .map(|(z, v, lane)| TrafficCar {
                    z0_dm: z,
                    speed_dms: v,
                    lane_cm: lane,
                })
                .collect();
            let name: String = name.iter().map(|&i| NAME_CHARSET[i] as char).collect();
            let gates = checkpoints.len();
            let proof = proof.map(|(allot, epoch, stamp)| Proof {
                allot_ticks: allot[..gates].to_vec(),
                epoch,
                stamp,
            });
            Layout {
                name,
                segments,
                checkpoints,
                objects,
                traffic,
                proof,
            }
        })
        .prop_filter("structurally sound", |l| l.structure().is_ok())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn any_layout_round_trips_through_its_code(l in layout()) {
        let code = l.to_code().unwrap();
        prop_assert!(code.len() <= MAX_CODE_CHARS);
        let back = Layout::from_code(&code).unwrap();
        prop_assert_eq!(&back, &l);
        prop_assert_eq!(back.to_code().unwrap(), code);
        // It always builds a track.
        prop_assert!(back.track().is_ok());
    }

    #[test]
    fn any_text_is_a_layout_or_an_error(text in ".{0,300}") {
        let _ = Layout::from_code(&text);
        let _ = Layout::from_code(&format!("RD{text}"));
        let _ = Layout::from_code(&format!("RL{text}"));
    }

    #[test]
    fn any_framed_body_is_a_layout_or_an_error(body in prop::collection::vec(any::<u8>(), 0..400)) {
        // Bodies with a valid checksum reach the bit-level decoder.
        let mut bytes = vec![LAYOUT_FORMAT];
        bytes.extend(&body);
        let crc = crc16(&bytes);
        bytes.extend(crc.to_be_bytes());
        for prefix in [DRAFT_PREFIX, SHARE_PREFIX] {
            if let Ok(l) = Layout::from_code(&format!("{prefix}{}", base32(&bytes))) {
                prop_assert!(l.structure().is_ok());
                prop_assert!(l.track().is_ok());
                prop_assert_eq!(Layout::from_code(&l.to_code().unwrap()), Ok(l));
            }
        }
    }

    #[test]
    fn a_mutated_code_is_never_a_panic(l in layout(), at in any::<prop::sample::Index>(), c in 0usize..32) {
        let mut code = l.to_code().unwrap().into_bytes();
        let i = 2 + at.index(code.len() - 2);
        code[i] = ALPHABET[c];
        let _ = Layout::from_code(&String::from_utf8(code).unwrap());
    }
}
