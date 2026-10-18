//! Property tests: the invariants the example tests pin at a few points,
//! checked across random tracks and inputs.
//!
//! Only the public API is used, the same surface the presentation layer and
//! the replay format depend on. On a failure proptest shrinks the input and
//! records its seed in `properties.proptest-regressions`, next to this file;
//! commit that file so the case stays covered.
//!
//! How the test kinds divide the work:
//!
//! - Example tests (`#[cfg(test)]` in each module) pin one known case each
//!   and document intent.
//! - Property tests (here) state a rule that must hold for every input and
//!   search for a counterexample. They find the case nobody thought to
//!   write down.
//! - Snapshot and golden tests (`snapshots.rs`, `golden_runs.rs`) pin exact
//!   outputs, so they catch any change at all, intended or not.

use proptest::prelude::*;
use racer_sim::car::{Car, CarParams, BOOST_VMAX};
use racer_sim::input::Input;
use racer_sim::track::{fixtures, Builder, Surface, Track, VERGE};

/// Any valid input byte.
fn input() -> impl Strategy<Value = Input> {
    (-3i8..=3, any::<bool>(), any::<bool>()).prop_map(|(s, b, d)| Input::new(s, b, d))
}

/// A held input and how long it is held: players hold buttons, so runs of
/// one input reach the states (long drifts, full slides) single ticks miss.
fn held_inputs() -> impl Strategy<Value = Vec<(Input, u16)>> {
    prop::collection::vec((input(), 1u16..90), 1..60)
}

/// A short random track: bends of any sharpness, hills and surfaces.
fn track() -> impl Strategy<Value = Track> {
    prop::collection::vec((-150i16..=150, 5usize..40, -12.0f64..12.0, 0usize..4), 1..8).prop_map(
        |parts| {
            let mut b = Builder::new();
            b.straight(10);
            for (curve, segs, height, surface) in parts {
                let start = b.len();
                b.bend(segs, curve, segs / 4);
                b.hill(start, segs, height);
                b.surface(start, segs, Surface::ALL[surface]);
            }
            b.checkpoint().straight(10);
            b.build()
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Whatever the player does, the car's numbers stay finite, its speed
    /// stays between zero and boosted top speed, and it never leaves the
    /// barriers.
    #[test]
    fn the_car_stays_finite_and_inside_the_barriers(
        t in prop_oneof![track(), Just(fixtures::sweep_and_hairpin()), Just(fixtures::blind_crest())],
        inputs in held_inputs(),
    ) {
        let p = CarParams::BASE;
        let mut car = Car::new();
        for (input, n) in inputs {
            for _ in 0..n {
                car.step(input, &t, &p);
                let limit = t.segment_at(car.z).half_width() + VERGE;
                prop_assert!(car.z.is_finite() && car.x.is_finite());
                prop_assert!(car.v.is_finite() && car.vx.is_finite() && car.charge.is_finite());
                prop_assert!(car.v >= 0.0 && car.v < p.vmax + BOOST_VMAX + 1.0, "v {}", car.v);
                // The barrier can move between segments; allow one tick of slide.
                prop_assert!(car.x.abs() <= limit + 1.0, "x {} past {limit}", car.x);
            }
        }
    }
}
