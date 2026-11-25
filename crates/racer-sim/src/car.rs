//! The car: speed along the road, offset across it, and the drift state
//! machine. DESIGN §3.
//!
//! The model is deliberately small, because two bots will drive it millions
//! of times per generated track and must be able to reason about it in
//! closed form. The car has four numbers of motion:
//!
//! - `z`, metres along the track, and `v`, its forward speed;
//! - `x`, metres from the centre line (positive right), and `vx`.
//!
//! **Longitudinal.** The throttle is automatic:
//!
//! ```text
//! v' = ACC·(1 − v/vmax_eff) − BRAKE·b − G·pitch − scrub
//! ```
//!
//! so speed rises toward `vmax_eff` with a time constant of `vmax/ACC`,
//! and anything that lowers `vmax_eff` (grass, gravel) slows the car
//! smoothly rather than with a wall of drag.
//!
//! **Lateral.** A bend of curvature `κ` pushes the car outward by `κ·v²`
//! (the centripetal acceleration it would need to follow the road), and
//! steering pushes back with at most the tyres' grip `μ`:
//!
//! ```text
//! vx' = steer·A(v) − κ·v² − DAMP·vx,     A(v) = μ_eff·min(1, v / V_STEER_FULL)
//! ```
//!
//! Full lock holds the line only while `κ·v² ≤ μ`, that is below
//! `v = sqrt(μ/κ)` ([`CarParams::grip_speed`]). Above it the car slides
//! outward predictably, however hard it steers. That one bound is what lets
//! the bots plan a corner in closed form (DESIGN §2.4).
//!
//! **Drift.** Holding drift with some steering, above [`DRIFT_MIN_SPEED`],
//! runs the state machine `Grip → Entry → Drifting → Grip`. While drifting
//! the lateral cap rises to `μ·DRIFT_CAP` (a faster corner), damping drops
//! (the car slides more freely) and the tyres scrub speed. Time spent
//! drifting with the wheel turned charges a boost, released as a mini or
//! super boost when drift is let go. Hitting something, or the barrier
//! while drifting, spins the car: [`Car::spin`].
//!
//! No trig: the car never has a heading. In a pseudo-3D racer the road's
//! direction is the view's direction, so a bend is just a sideways push.

use crate::input::Input;
use crate::track::{Segment, Surface, Track, VERGE};
use crate::DT;

/// Gravity, m/s², for hills.
pub const G: f64 = 9.81;

/// Below this speed steering authority fades to zero: a stopped car cannot
/// slide sideways by turning the wheel. m/s.
pub const V_STEER_FULL: f64 = 6.0;

/// Lateral damping in grip, 1/s: the tyres kill sideways motion quickly.
pub const DAMP_GRIP: f64 = 5.0;
/// Lateral damping while drifting, 1/s: the car keeps sliding.
pub const DAMP_DRIFT: f64 = 1.5;

/// Drift engages only above this speed, m/s.
pub const DRIFT_MIN_SPEED: f64 = 14.0;
/// Ticks between pressing drift and the drift taking hold.
pub const ENTRY_TICKS: u8 = 6;
/// Speed kept on entering a drift: the flick costs 3%.
pub const ENTRY_KEEP: f64 = 0.97;
/// Lateral grip while drifting, as a multiple of the tyres' grip. DESIGN §3.3.
pub const DRIFT_CAP: f64 = 1.45;
/// Speed scrubbed by the tyres while drifting, m/s².
pub const DRIFT_SCRUB: f64 = 2.5;

/// Drift charge (seconds of steered drifting) for a mini boost.
pub const MINI_CHARGE: f64 = 0.6;
/// Drift charge for a super boost.
pub const SUPER_CHARGE: f64 = 1.5;
/// Boost length, ticks: mini then super.
pub const BOOST_TICKS: [u16; 2] = [72, 150];
/// Extra acceleration while boosting, m/s².
pub const BOOST_ACC: f64 = 8.0;
/// Extra top speed while boosting, m/s.
pub const BOOST_VMAX: f64 = 6.0;

/// Ticks a spin lasts, with no control.
pub const SPIN_TICKS: u8 = 90;
/// Speed kept when a spin starts.
pub const SPIN_KEEP: f64 = 0.15;
/// Deceleration while spinning, m/s².
pub const SPIN_DECEL: f64 = 8.0;

/// Off the road (inside the verge): grip, top speed and extra drag.
pub const OFFROAD_GRIP: f64 = 0.75;
/// Top speed multiplier off the road.
pub const OFFROAD_VMAX: f64 = 0.6;
/// Extra drag off the road, m/s².
pub const OFFROAD_DRAG: f64 = 4.0;
/// Drag while scraping the barrier, m/s².
pub const BARRIER_DRAG: f64 = 12.0;

/// How a surface changes the car. DESIGN §3.4.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SurfaceParams {
    /// Multiplier on grip.
    pub grip: f64,
    /// Multiplier on drift grip: gravel holds a drift almost as well as
    /// asphalt, which is why it rewards drifting.
    pub drift_grip: f64,
    /// Multiplier on top speed.
    pub vmax: f64,
    /// Extra drag, m/s².
    pub drag: f64,
}

impl SurfaceParams {
    /// The parameters for `s`.
    pub const fn of(s: Surface) -> SurfaceParams {
        match s {
            Surface::Asphalt => SurfaceParams {
                grip: 1.0,
                drift_grip: 1.0,
                vmax: 1.0,
                drag: 0.0,
            },
            Surface::Gravel => SurfaceParams {
                grip: 0.65,
                drift_grip: 0.95,
                vmax: 0.9,
                drag: 1.5,
            },
            Surface::Ice => SurfaceParams {
                grip: 0.4,
                drift_grip: 0.42,
                vmax: 1.0,
                drag: 0.0,
            },
            Surface::Puddle => SurfaceParams {
                grip: 0.55,
                drift_grip: 0.6,
                vmax: 0.92,
                drag: 2.0,
            },
        }
    }
}

/// The car's performance. Upgrades (`upgrade.rs`) scale these; the bots read
/// them to plan.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CarParams {
    /// Top speed on asphalt, m/s.
    pub vmax: f64,
    /// Acceleration from standstill, m/s².
    pub accel: f64,
    /// Tyre grip: the most sideways acceleration steering can supply, m/s².
    pub grip: f64,
    /// Braking deceleration, m/s².
    pub brake: f64,
    /// Drift charge gained per second of steered drifting.
    pub charge_rate: f64,
}

impl CarParams {
    /// The car with no upgrades.
    pub const BASE: CarParams = CarParams {
        vmax: 52.0,
        accel: 16.0,
        grip: 20.0,
        brake: 20.0,
        charge_rate: 1.0,
    };

    /// Lateral grip on segment `s` in grip mode, on the road. m/s².
    #[inline]
    pub fn grip_on(&self, s: &Segment) -> f64 {
        self.grip * SurfaceParams::of(s.surface).grip
    }

    /// Lateral grip on segment `s` while drifting, on the road. m/s².
    #[inline]
    pub fn drift_grip_on(&self, s: &Segment) -> f64 {
        self.grip * DRIFT_CAP * SurfaceParams::of(s.surface).drift_grip
    }

    /// The fastest the car can hold the line through `s` without drifting:
    /// `sqrt(μ/κ)`, or infinity on a straight.
    #[inline]
    pub fn grip_speed(&self, s: &Segment) -> f64 {
        limit_speed(self.grip_on(s), s.curvature())
    }

    /// The fastest the car can hold the line through `s` while drifting.
    #[inline]
    pub fn drift_speed(&self, s: &Segment) -> f64 {
        limit_speed(self.drift_grip_on(s), s.curvature())
    }

    /// Top speed on segment `s`, on the road, without boost. m/s.
    #[inline]
    pub fn vmax_on(&self, s: &Segment) -> f64 {
        self.vmax * SurfaceParams::of(s.surface).vmax
    }
}

/// `sqrt(a/|κ|)`: the speed at which a bend's push equals the grip `a`.
#[inline]
pub fn limit_speed(a: f64, curvature: f64) -> f64 {
    let k = crate::dmath::abs(curvature);
    if k < 1e-9 {
        f64::INFINITY
    } else {
        crate::dmath::sqrt(a / k)
    }
}

/// Where the car is in the drift state machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Drift {
    /// Ordinary driving.
    #[default]
    Grip,
    /// Drift pressed; it takes hold when the count reaches zero.
    Entry(u8),
    /// Sliding: more lateral grip, less damping, scrubbing speed, charging.
    Drifting,
    /// Spun out: no control until the count reaches zero.
    Spin(u8),
}

/// A boost earned by a drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Boost {
    /// Short boost: [`MINI_CHARGE`] or more.
    Mini = 0,
    /// Long boost: [`SUPER_CHARGE`] or more.
    Super = 1,
}

/// The car's state. Everything here is hashed into the run state.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Car {
    /// Distance along the track, metres.
    pub z: f64,
    /// Offset from the centre line, metres, positive right.
    pub x: f64,
    /// Forward speed, m/s, never negative.
    pub v: f64,
    /// Sideways speed, m/s.
    pub vx: f64,
    /// Drift state.
    pub drift: Drift,
    /// Drift charge, seconds of steered drifting.
    pub charge: f64,
    /// Boost ticks left.
    pub boost_ticks: u16,
    /// The last boost earned, for the HUD and the tests.
    pub last_boost: Option<Boost>,
}

impl Car {
    /// A car on the start line, stopped, on the centre line.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the car is spinning.
    #[inline]
    pub fn spinning(&self) -> bool {
        matches!(self.drift, Drift::Spin(_))
    }

    /// Whether the car is drifting (not entering).
    #[inline]
    pub fn drifting(&self) -> bool {
        self.drift == Drift::Drifting
    }

    /// Spin out: lose most of the speed and all control for [`SPIN_TICKS`].
    /// Called by the race on a hit; a spin already under way is not
    /// restarted.
    pub fn spin(&mut self) {
        if self.spinning() {
            return;
        }
        self.drift = Drift::Spin(SPIN_TICKS);
        self.v *= SPIN_KEEP;
        self.charge = 0.0;
        self.boost_ticks = 0;
    }

    /// Advance one tick. Semi-implicit Euler: velocities first, then
    /// positions from the new velocities.
    pub fn step(&mut self, input: Input, track: &Track, p: &CarParams) {
        let i = track.index_at(self.z);
        let shape = track.shape(i);
        let surf = SurfaceParams::of(track.segments()[i].surface);
        let hw = shape.half_width;
        let off = crate::dmath::abs(self.x) > hw;
        let off_grip = if off { OFFROAD_GRIP } else { 1.0 };

        self.update_drift(input, p);

        // --- longitudinal
        let boosting = self.boost_ticks > 0;
        let mut vmax = p.vmax * surf.vmax * if off { OFFROAD_VMAX } else { 1.0 };
        let mut acc = p.accel;
        if boosting {
            vmax += BOOST_VMAX;
            acc += BOOST_ACC;
            self.boost_ticks -= 1;
        }
        let mut a = -G * shape.pitch - surf.drag;
        if off {
            a -= OFFROAD_DRAG;
        }
        let spinning = self.spinning();
        if spinning {
            a -= SPIN_DECEL;
        } else {
            a += acc * (1.0 - self.v / vmax);
            if input.brake() {
                a -= p.brake;
            }
            if self.drifting() {
                a -= DRIFT_SCRUB;
            }
        }

        // --- lateral
        let (cap, damp) = match self.drift {
            Drift::Drifting => (p.grip * DRIFT_CAP * surf.drift_grip, DAMP_DRIFT),
            _ => (p.grip * surf.grip, DAMP_GRIP),
        };
        let authority = cap * off_grip * crate::dmath::clamp(self.v / V_STEER_FULL, 0.0, 1.0);
        let steer = if spinning { 0.0 } else { input.steer() };
        let ax = steer * authority - shape.curvature * self.v * self.v - damp * self.vx;

        self.v = (self.v + a * DT).max(0.0);
        self.vx += ax * DT;
        self.z += self.v * DT;
        self.x += self.vx * DT;

        // --- barrier
        let limit = hw + VERGE;
        if crate::dmath::abs(self.x) > limit {
            self.x = crate::dmath::clamp(self.x, -limit, limit);
            if self.x * self.vx > 0.0 {
                self.vx = 0.0;
            }
            self.v = (self.v - BARRIER_DRAG * DT).max(0.0);
            if self.drifting() {
                self.spin();
            }
        }
    }

    /// The drift state machine, before the physics of the tick.
    fn update_drift(&mut self, input: Input, p: &CarParams) {
        let steered = input.steer_level() != 0;
        self.drift = match self.drift {
            Drift::Spin(1) => Drift::Grip,
            Drift::Spin(n) => Drift::Spin(n - 1),
            Drift::Grip => {
                if input.drift() && steered && self.v >= DRIFT_MIN_SPEED {
                    self.v *= ENTRY_KEEP;
                    self.charge = 0.0;
                    Drift::Entry(ENTRY_TICKS)
                } else {
                    Drift::Grip
                }
            }
            Drift::Entry(n) => {
                if !input.drift() {
                    Drift::Grip
                } else if n <= 1 {
                    Drift::Drifting
                } else {
                    Drift::Entry(n - 1)
                }
            }
            Drift::Drifting => {
                if !input.drift() || self.v < DRIFT_MIN_SPEED * 0.8 {
                    self.release();
                    Drift::Grip
                } else {
                    if steered {
                        self.charge += DT * p.charge_rate;
                    }
                    Drift::Drifting
                }
            }
        };
    }

    /// Turn the drift charge into a boost.
    fn release(&mut self) {
        let tier = if self.charge >= SUPER_CHARGE {
            Some(Boost::Super)
        } else if self.charge >= MINI_CHARGE {
            Some(Boost::Mini)
        } else {
            None
        };
        if let Some(b) = tier {
            self.boost_ticks = BOOST_TICKS[b as usize];
            self.last_boost = Some(b);
        }
        self.charge = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{fixtures, Builder};

    const P: CarParams = CarParams::BASE;

    fn run(car: &mut Car, track: &Track, input: Input, ticks: u32) {
        for _ in 0..ticks {
            car.step(input, track, &P);
        }
    }

    /// A long constant bend of `milli` per segment, for cornering tests.
    fn ring(milli: i16) -> Track {
        let mut b = Builder::new();
        b.bend(400, milli, 0).checkpoint().straight(1);
        b.build()
    }

    fn at_speed(v: f64) -> Car {
        Car { v, ..Car::new() }
    }

    #[test]
    fn the_throttle_approaches_top_speed_and_never_passes_it() {
        let t = fixtures::drag_strip();
        let mut car = Car::new();
        let mut last = 0.0;
        for _ in 0..(10 * 120) {
            car.step(Input::NEUTRAL, &t, &P);
            assert!(car.v >= last && car.v < P.vmax);
            last = car.v;
        }
        assert!(car.v > 0.95 * P.vmax, "after 10 s: {}", car.v);
    }

    #[test]
    fn braking_slows_the_car() {
        let t = fixtures::drag_strip();
        let mut car = at_speed(40.0);
        run(&mut car, &t, Input::new(0, true, false), 120);
        assert!(car.v < 40.0 - 0.5 * P.brake, "{}", car.v);
    }

    #[test]
    fn a_climb_lowers_the_speed_the_throttle_settles_at() {
        let mut b = Builder::new();
        b.straight(1000).checkpoint().straight(1);
        for s in b.segments_mut() {
            s.pitch_milli = 80;
        }
        let hill = b.build();
        let mut car = Car::new();
        run(&mut car, &hill, Input::NEUTRAL, 40 * 120);
        // Equilibrium of ACC·(1 − v/vmax) = G·pitch.
        let settle = P.vmax * (1.0 - G * 0.08 / P.accel);
        assert!((car.v - settle).abs() < 0.05, "{} vs {settle}", car.v);
    }

    #[test]
    fn a_bend_pushes_an_unsteered_car_outward() {
        let t = ring(60); // right-hander
        let mut car = at_speed(30.0);
        run(&mut car, &t, Input::NEUTRAL, 60);
        assert!(car.x < -0.5, "slid left, outward: {}", car.x);
    }

    #[test]
    fn full_lock_holds_below_the_grip_speed_and_not_above_it() {
        let t = ring(60);
        let bound = P.grip_speed(&t.segments()[0]);
        for (scale, holds) in [(0.9, true), (1.1, false)] {
            let mut car = at_speed(bound * scale);
            // Hold the speed fixed so only the lateral model is under test.
            for _ in 0..60 {
                let v = car.v;
                car.step(Input::new(3, false, false), &t, &P);
                car.v = v;
            }
            assert_eq!(car.vx > 0.0, holds, "at {scale} x bound: vx {}", car.vx);
        }
    }

    #[test]
    fn drift_lifts_the_cornering_limit_by_its_cap() {
        let s = ring(60).segments()[0];
        let ratio = P.drift_speed(&s) / P.grip_speed(&s);
        assert!((ratio - DRIFT_CAP.sqrt()).abs() < 1e-12);
        assert_eq!(
            P.grip_speed(&fixtures::drag_strip().segments()[0]),
            f64::INFINITY
        );
        let gravel = Segment {
            surface: Surface::Gravel,
            ..s
        };
        assert!(
            P.drift_speed(&gravel) / P.grip_speed(&gravel) > ratio,
            "gravel rewards drifting"
        );
    }

    #[test]
    fn a_drift_enters_after_its_delay_and_costs_speed() {
        let t = ring(40);
        let mut car = at_speed(30.0);
        let drift = Input::new(3, false, true);
        car.step(drift, &t, &P);
        assert_eq!(car.drift, Drift::Entry(ENTRY_TICKS));
        assert!(car.v < 30.0 * ENTRY_KEEP + 0.2);
        run(&mut car, &t, drift, ENTRY_TICKS as u32);
        assert_eq!(car.drift, Drift::Drifting);
    }

    #[test]
    fn a_drift_needs_speed_and_steering() {
        let t = ring(40);
        let mut slow = at_speed(DRIFT_MIN_SPEED - 1.0);
        slow.step(Input::new(3, false, true), &t, &P);
        assert_eq!(slow.drift, Drift::Grip);
        let mut straight = at_speed(30.0);
        straight.step(Input::new(0, false, true), &t, &P);
        assert_eq!(straight.drift, Drift::Grip);
    }

    #[test]
    fn letting_go_early_in_entry_earns_nothing() {
        let t = ring(40);
        let mut car = at_speed(30.0);
        run(&mut car, &t, Input::new(3, false, true), 3);
        car.step(Input::new(3, false, false), &t, &P);
        assert_eq!(car.drift, Drift::Grip);
        assert_eq!(car.boost_ticks, 0);
    }

    #[test]
    fn charge_buys_mini_then_super_boosts() {
        let t = ring(60);
        for (secs, want) in [
            (0.3, None),
            (1.0, Some(Boost::Mini)),
            (2.0, Some(Boost::Super)),
        ] {
            let mut car = at_speed(35.0);
            let ticks = ENTRY_TICKS as u32 + 1 + (secs * 120.0) as u32;
            // Two-thirds lock roughly balances this bend while drifting.
            run(&mut car, &t, Input::new(2, false, true), ticks);
            assert_eq!(car.drift, Drift::Drifting, "still drifting after {secs}s");
            car.step(Input::new(2, false, false), &t, &P);
            assert_eq!(car.last_boost, want, "after {secs}s");
            assert_eq!(car.boost_ticks > 0, want.is_some());
        }
    }

    #[test]
    fn a_boost_passes_top_speed() {
        let t = fixtures::drag_strip();
        let mut car = at_speed(P.vmax - 0.5);
        car.boost_ticks = BOOST_TICKS[1];
        run(&mut car, &t, Input::NEUTRAL, BOOST_TICKS[1] as u32);
        assert!(car.v > P.vmax + 1.0, "{}", car.v);
        assert_eq!(car.boost_ticks, 0);
    }

    #[test]
    fn a_spin_takes_speed_and_control_for_its_length() {
        let t = fixtures::drag_strip();
        let mut car = at_speed(40.0);
        car.spin();
        assert!((car.v - 40.0 * SPIN_KEEP).abs() < 1e-9);
        let x = car.x;
        run(
            &mut car,
            &t,
            Input::new(3, false, false),
            SPIN_TICKS as u32 - 1,
        );
        assert!(car.spinning());
        assert_eq!(car.x, x, "no steering while spinning");
        car.step(Input::NEUTRAL, &t, &P);
        assert_eq!(car.drift, Drift::Grip);
        // A second spin while spinning changes nothing.
        let mut twice = at_speed(40.0);
        twice.spin();
        twice.spin();
        assert!((twice.v - 40.0 * SPIN_KEEP).abs() < 1e-9);
    }

    #[test]
    fn the_barrier_stops_the_slide_and_spins_a_drifter() {
        let t = ring(100);
        let mut car = at_speed(40.0);
        run(&mut car, &t, Input::NEUTRAL, 240);
        let limit = t.segments()[0].half_width() + VERGE;
        assert_eq!(car.x, -limit, "pinned to the outside barrier");
        assert!(!car.spinning());

        let mut drifter = at_speed(45.0);
        drifter.x = -limit + 0.5;
        let mut spun = false;
        for _ in 0..60 {
            drifter.step(Input::new(1, false, true), &t, &P);
            spun |= drifter.spinning();
        }
        assert!(spun, "drifting into the barrier spins");
    }

    #[test]
    fn each_surface_settles_at_its_own_top_speed() {
        for surface in Surface::ALL {
            let mut b = Builder::new();
            b.straight(1500).checkpoint().straight(1);
            b.surface(0, 1500, surface);
            let t = b.build();
            let mut car = Car::new();
            run(&mut car, &t, Input::NEUTRAL, 40 * 120);
            // Equilibrium of ACC·(1 − v/vmax_eff) = drag.
            let sp = SurfaceParams::of(surface);
            let settle = P.vmax * sp.vmax * (1.0 - sp.drag / P.accel);
            assert!(
                (car.v - settle).abs() < 0.05,
                "{surface:?}: {} vs {settle}",
                car.v
            );
        }
    }

    #[test]
    fn grass_is_slower_than_road() {
        let t = fixtures::drag_strip();
        let (mut road, mut grass) = (Car::new(), Car::new());
        grass.x = 8.0;
        run(&mut road, &t, Input::NEUTRAL, 600);
        run(&mut grass, &t, Input::NEUTRAL, 600);
        assert!(grass.v < OFFROAD_VMAX * P.vmax + 0.1, "{}", grass.v);
        assert!(road.v > grass.v + 10.0);
    }
}
