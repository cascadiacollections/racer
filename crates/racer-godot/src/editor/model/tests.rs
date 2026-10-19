use super::*;
use racer_sim::layout::{bundled, Standing};
use racer_sim::rng::Pcg32;

fn course() -> Draft {
    Draft::new(bundled::layouts().remove(0))
}

#[test]
fn a_generated_road_is_cut_where_it_turns_and_runs_tile_it() {
    let d = course();
    assert!(d.run_count() > 8, "{:?}", d.runs);
    assert_eq!(d.runs[0], 0);
    assert!(d.runs.windows(2).all(|w| w[1] - w[0] >= MIN_RUN as u32));
    let (_, end) = d.run_range(d.run_count() - 1);
    assert_eq!(end, d.layout.segments.len());
    for seg in [0, 10, 300, d.layout.segments.len() - 1] {
        let k = d.run_of(seg);
        let (a, b) = d.run_range(k);
        assert!((a..b).contains(&seg));
    }
    // A bend run reads a peak with the bend's sign; a straight reads 0.
    let k = (0..d.run_count()).find(|&k| d.bend(k) != 0).unwrap();
    let (a, b) = d.run_range(k);
    let max = d.layout.segments[a..b]
        .iter()
        .map(|s| s.curve_milli as i32)
        .max_by_key(|c| c.abs())
        .unwrap();
    assert_eq!(d.bend(k).signum(), max.signum());
    assert_eq!(Draft::blank().bend(0), 0);
    assert_eq!(Draft::blank().layout.check(), vec![]);
}

#[test]
fn stepping_a_bend_or_a_hill_up_and_back_gives_back_exactly_what_was_there() {
    let mut d = Draft::blank();
    for k in 0..d.run_count() {
        d.apply(&Edit::Bend { run: k, steps: 3 });
        d.apply(&Edit::Hill { run: k, steps: 2 });
    }
    let shaped = d.clone();
    assert_eq!(d.bend(1), 15);
    assert_eq!(d.hill(1), 20);
    for k in 0..d.run_count() {
        for _ in 0..7 {
            d.apply(&Edit::Bend { run: k, steps: 1 });
            d.apply(&Edit::Hill { run: k, steps: -1 });
        }
        for _ in 0..7 {
            d.apply(&Edit::Bend { run: k, steps: -1 });
            d.apply(&Edit::Hill { run: k, steps: 1 });
        }
    }
    assert_eq!(d, shaped);
    // A hill comes back down: the run ends at the height it began.
    let (a, b) = d.run_range(1);
    let rise: i32 = d.layout.segments[a..b]
        .iter()
        .map(|s| s.pitch_milli as i32)
        .sum();
    assert!(rise.abs() <= 2, "{rise}");
    // Limits hold.
    for _ in 0..100 {
        d.apply(&Edit::Bend { run: 1, steps: 1 });
        d.apply(&Edit::Hill { run: 1, steps: 1 });
        d.apply(&Edit::Width { run: 1, steps: -1 });
    }
    assert_eq!(d.bend(1), CURVE_MAX as i32);
    assert_eq!(d.hill(1), PITCH_MAX as i32);
    assert_eq!(d.width(1), HALF_WIDTH_RANGE.0);
    assert_eq!(d.layout.check(), vec![], "still provable");
}

#[test]
fn an_edit_to_the_road_drops_the_proof_and_a_rename_keeps_it() {
    let mut d = course();
    assert_eq!(d.layout.standing(), Standing::Proven);
    assert!(d.apply(&Edit::Name("MY LIGHT".into())));
    assert_eq!(d.layout.standing(), Standing::Proven);
    assert!(!d.apply(&Edit::Name("bad name".into())), "not a valid name");
    assert!(!d.apply(&Edit::Name("MY LIGHT".into())), "no change");
    assert!(d.apply(&Edit::Surface {
        run: 2,
        surface: Surface::Gravel
    }));
    assert_eq!(d.layout.standing(), Standing::Unproven);
    // An edit that changes nothing leaves no trace.
    let before = d.clone();
    assert!(!d.apply(&Edit::Surface {
        run: 2,
        surface: Surface::Gravel
    }));
    assert!(!d.apply(&Edit::Bend { run: 99, steps: 1 }));
    assert_eq!(d, before);
}

#[test]
fn add_delete_split_and_join_move_what_stands_on_the_road() {
    let mut d = Draft::blank();
    d.apply(&Edit::Car { run: 2, lane: 0 });
    d.apply(&Edit::Posts {
        run: 2,
        side: Side::Left,
    });
    d.apply(&Edit::Rock { run: 3, lane: 2 });
    let posts = d.objects_on(2);
    assert!(posts >= 15, "{posts}");
    assert_eq!((d.cars_on(2), d.objects_on(3)), (1, 1));
    // ADD after run 0: everything from run 1 on moves up 25 segments.
    let before = d.clone();
    assert!(d.apply(&Edit::Add { run: 0 }));
    assert_eq!(d.run_count(), 5);
    assert_eq!(d.run_range(1), (60, 85));
    assert_eq!(
        d.layout.checkpoints,
        vec![60, 145, 205],
        "the gate ending run 0 stays put"
    );
    assert_eq!(
        (d.cars_on(3), d.objects_on(3), d.objects_on(4)),
        (1, posts, 1)
    );
    assert_eq!(
        d.layout.traffic[0].z0_dm,
        before.layout.traffic[0].z0_dm + 1000
    );
    // DELETE it again: back where it was.
    assert!(d.apply(&Edit::Delete { run: 1 }));
    assert_eq!(d.layout, before.layout);
    assert_eq!(d.runs, before.runs);
    // DELETE a run with things on it: they go with it, and its gate.
    assert!(d.apply(&Edit::Delete { run: 2 }));
    assert_eq!(d.layout.traffic, vec![]);
    assert_eq!(d.objects_on(2), 1, "the rock moved down");
    assert_eq!(d.layout.checkpoints, vec![60, 120]);
    assert_eq!(d.layout.structure(), Ok(()));
    // SPLIT and JOIN only move cuts.
    let road = d.layout.clone();
    assert!(d.apply(&Edit::Split { run: 0 }));
    assert_eq!(d.runs[..2], [0, 30]);
    assert!(d.apply(&Edit::Join { run: 0 }));
    assert_eq!(d.layout, road);
    assert!(!d.apply(&Edit::Join {
        run: d.run_count() - 1
    }));
}

#[test]
fn gates_are_added_removed_and_moved_but_one_always_stays() {
    let mut d = Draft::blank();
    assert_eq!(d.gates_on(0), vec![0], "the gate at 60 ends run 0");
    assert!(d.apply(&Edit::Gate { run: 0 }));
    assert_eq!(d.layout.checkpoints, vec![120, 180]);
    assert!(d.apply(&Edit::Gate { run: 0 }));
    assert_eq!(d.layout.checkpoints, vec![60, 120, 180]);
    assert!(d.apply(&Edit::MoveGate { run: 1, segs: 5 }));
    assert_eq!(d.layout.checkpoints, vec![60, 125, 180]);
    // The gate is on run 2 now, the run it moved onto.
    assert_eq!(d.gates_on(2), vec![1, 2]);
    assert!(d.apply(&Edit::MoveGate { run: 2, segs: 500 }));
    assert_eq!(
        d.layout.checkpoints,
        vec![60, 179, 180],
        "stops at the next"
    );
    // The finish on the last run (200 segments, gate at 180 on run 2).
    let mut one = Draft::blank();
    one.layout.checkpoints = vec![180];
    assert!(!one.apply(&Edit::Gate { run: 2 }), "the last gate stays");
    assert!(one.apply(&Edit::Gate { run: 3 }));
    assert_eq!(one.layout.checkpoints, vec![180, 199]);
}

#[test]
fn lengthening_a_run_stretches_it_and_moves_the_road_after() {
    let mut d = Draft::blank();
    d.apply(&Edit::Bend { run: 1, steps: 4 });
    d.apply(&Edit::Car { run: 3, lane: 1 });
    let z0 = d.layout.traffic[0].z0_dm;
    assert!(d.apply(&Edit::Length { run: 1, steps: 2 }));
    assert_eq!(d.run_range(1), (60, 130));
    assert_eq!(d.layout.segments.len(), 210);
    assert_eq!(d.layout.checkpoints, vec![60, 130, 190]);
    assert_eq!(d.layout.traffic[0].z0_dm, z0 + 400);
    assert!(d.apply(&Edit::Length {
        run: 1,
        steps: -100
    }));
    assert_eq!(d.run_range(1), (60, 60 + MIN_RUN));
    assert_eq!(d.layout.structure(), Ok(()));
}

#[test]
fn undo_and_redo_are_exact_and_a_run_of_presses_is_one_step() {
    let mut d = course();
    let start = d.clone();
    let mut h = History::default();
    for _ in 0..4 {
        h.apply(&mut d, &Edit::Bend { run: 3, steps: 1 });
    }
    let bent = d.clone();
    h.apply(&mut d, &Edit::Width { run: 3, steps: 1 });
    h.apply(&mut d, &Edit::Width { run: 3, steps: 1 });
    let wide = d.clone();
    h.apply(&mut d, &Edit::Delete { run: 0 });
    assert!(h.undo(&mut d));
    assert_eq!(d, wide);
    assert!(h.undo(&mut d), "both widths at once");
    assert_eq!(d, bent);
    assert!(h.undo(&mut d), "all four bends at once");
    assert_eq!(d, start);
    assert!(!h.undo(&mut d));
    assert!(h.redo(&mut d) && h.redo(&mut d));
    assert_eq!(d, wide);
    // A new edit drops the redo.
    h.apply(&mut d, &Edit::Split { run: 0 });
    assert!(!h.can_redo());
    // Sealing ends a run of presses.
    let mut d2 = Draft::blank();
    let mut h2 = History::default();
    h2.apply(&mut d2, &Edit::Bend { run: 1, steps: 1 });
    h2.seal();
    h2.apply(&mut d2, &Edit::Bend { run: 1, steps: 1 });
    assert!(h2.undo(&mut d2));
    assert_eq!(d2.bend(1), 5);
    // Replacing the draft is one step too.
    let mut d3 = Draft::blank();
    let mut h3 = History::default();
    h3.replace(&mut d3, course());
    assert!(h3.undo(&mut d3));
    assert_eq!(d3, Draft::blank());
    // The history is bounded.
    let mut h4 = History::default();
    let mut d4 = Draft::blank();
    for i in 0..UNDO_DEPTH + 10 {
        h4.apply(
            &mut d4,
            &Edit::Surface {
                run: 0,
                surface: Surface::ALL[i % 4],
            },
        );
    }
    let mut n = 0;
    while h4.undo(&mut d4) {
        n += 1;
    }
    assert_eq!(n, UNDO_DEPTH);
}

/// A random edit for the soak test.
fn random_edit(rng: &mut Pcg32, d: &Draft) -> Edit {
    let run = rng.range_u32(0, d.run_count() as u32 + 1) as usize;
    let s = rng.range_u32(0, 6) as i32 - 3;
    match rng.range_u32(0, 17) {
        0 => Edit::Bend { run, steps: s },
        1 => Edit::Hill { run, steps: s },
        2 => Edit::Width { run, steps: s },
        3 => Edit::Surface {
            run,
            surface: Surface::ALL[rng.range_u32(0, 3) as usize],
        },
        4 => Edit::Drift {
            run,
            mark: [DriftMark::None, DriftMark::Hint, DriftMark::Required]
                [rng.range_u32(0, 2) as usize],
        },
        5 => Edit::Posts {
            run,
            side: if s < 0 { Side::Left } else { Side::Right },
        },
        6 => Edit::Rock {
            run,
            lane: rng.range_u32(0, 2) as usize,
        },
        7 => Edit::ClearObjects { run },
        8 => Edit::Car {
            run,
            lane: rng.range_u32(0, 2) as usize,
        },
        9 => Edit::ClearCars { run },
        10 => Edit::Gate { run },
        11 => Edit::MoveGate { run, segs: s * 3 },
        12 => Edit::Split { run },
        13 => Edit::Join { run },
        14 => Edit::Add { run },
        15 => Edit::Delete { run },
        _ => Edit::Length { run, steps: s },
    }
}

#[test]
fn any_sequence_of_edits_keeps_a_buildable_codeable_layout_and_undoes_exactly() {
    for seed in 0..12u64 {
        let mut rng = Pcg32::new(seed);
        let mut d = if seed % 2 == 0 {
            course()
        } else {
            Draft::blank()
        };
        let mut h = History::default();
        let mut seen = vec![d.clone()];
        for _ in 0..150 {
            let e = random_edit(&mut rng, &d);
            if h.apply(&mut d, &e) {
                seen.push(d.clone());
            }
            assert_eq!(d.layout.structure(), Ok(()), "{e:?}");
            assert_eq!(d.runs[0], 0);
            assert!(d.runs.windows(2).all(|w| w[0] < w[1]), "{:?}", d.runs);
            assert!((*d.runs.last().unwrap() as usize) < d.layout.segments.len());
            let code = d.layout.to_code().unwrap();
            assert_eq!(
                racer_sim::layout::Layout::from_code(&code).unwrap(),
                d.layout
            );
        }
        // Undo walks back through drafts that were each seen.
        while h.undo(&mut d) {
            assert!(seen.contains(&d));
        }
    }
}
