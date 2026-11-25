//! Slow traffic on a closed-form schedule. DESIGN §2.6.
//!
//! Every traffic car drives one lane at one constant speed, so where it is
//! on any tick is a formula, `z = z0 + v·t`, not a simulation. Two things
//! follow. Traffic costs nothing to step and can never drift out of sync
//! between a run and its replay. And a bot can look ahead *exactly*: where a
//! car will be when the player arrives is the same formula, which is what
//! lets the solver prove a track passable rather than hope (DESIGN §2.4).
//! The human-like bot is handicapped on purpose, by seeing traffic late
//! (`bot/human.rs`), not by any uncertainty in the traffic itself.
//!
//! Traffic does not react to the player. It is scenery that moves.

use crate::DT;

/// Length of a car, metres (the player's and traffic's alike).
pub const CAR_LEN: f64 = 4.5;
/// Half the width of a car, metres.
pub const CAR_HALF_WIDTH: f64 = 0.9;

/// One traffic car, stored as integers like the rest of the track.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrafficCar {
    /// Where it is when the race starts, decimetres along the track.
    pub z0_dm: u32,
    /// Its constant speed, decimetres per second.
    pub speed_dms: u16,
    /// Its lane: offset from the centre line, centimetres, positive right.
    pub lane_cm: i16,
}

impl TrafficCar {
    /// A car starting `z0` metres along the track at `speed` m/s, `lane`
    /// metres from the centre line, each rounded to the stored resolution.
    pub fn new(z0: f64, speed: f64, lane: f64) -> Self {
        TrafficCar {
            z0_dm: crate::dmath::round(z0 * 10.0).max(0.0) as u32,
            speed_dms: crate::dmath::round(speed * 10.0).max(0.0) as u16,
            lane_cm: crate::dmath::round(lane * 100.0) as i16,
        }
    }

    /// Speed, m/s.
    #[inline]
    pub fn speed(&self) -> f64 {
        self.speed_dms as f64 / 10.0
    }

    /// Lane offset, metres.
    #[inline]
    pub fn x(&self) -> f64 {
        self.lane_cm as f64 / 100.0
    }

    /// Where the car is on race tick `tick`, metres along the track.
    #[inline]
    pub fn z_at(&self, tick: u32) -> f64 {
        self.lane().z_at(tick)
    }

    /// Whether a player car at `(z, x)` overlaps this car on `tick`.
    #[inline]
    pub fn hits(&self, tick: u32, z: f64, x: f64) -> bool {
        self.lane().hits(tick, z, x)
    }

    /// The car's schedule as floats.
    #[inline]
    pub fn lane(&self) -> Lane {
        Lane {
            z0: self.z0_dm as f64 / 10.0,
            speed: self.speed(),
            x: self.x(),
        }
    }
}

/// A traffic car's schedule converted to metres and seconds once, so the
/// per-tick lookups divide nothing. `Track` keeps one per car; the values
/// and the arithmetic are [`TrafficCar`]'s own, so positions are the same
/// bits either way.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Lane {
    /// Where the car is when the race starts, metres along the track.
    pub z0: f64,
    /// Its constant speed, m/s.
    pub speed: f64,
    /// Offset from the centre line, metres, positive right.
    pub x: f64,
}

impl Lane {
    /// Where the car is on race tick `tick`, metres along the track.
    #[inline]
    pub fn z_at(&self, tick: u32) -> f64 {
        self.z0 + self.speed * (tick as f64 * DT)
    }

    /// Whether a player car at `(z, x)` overlaps this car on `tick`.
    #[inline]
    pub fn hits(&self, tick: u32, z: f64, x: f64) -> bool {
        crate::dmath::abs(z - self.z_at(tick)) < CAR_LEN
            && crate::dmath::abs(x - self.x) < 2.0 * CAR_HALF_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICKS_PER_SEC;

    #[test]
    fn position_is_a_formula_of_the_tick() {
        let c = TrafficCar::new(100.0, 20.0, -4.0);
        assert_eq!(c.z_at(0), 100.0);
        assert!((c.z_at(TICKS_PER_SEC) - 120.0).abs() < 1e-9);
        assert!((c.z_at(10 * TICKS_PER_SEC) - 300.0).abs() < 1e-9);
        assert_eq!(c.x(), -4.0);
    }

    #[test]
    fn a_car_in_the_next_lane_is_missed() {
        let c = TrafficCar::new(100.0, 20.0, 0.0);
        assert!(c.hits(0, 97.0, 1.0));
        assert!(!c.hits(0, 97.0, 4.0), "next lane");
        assert!(!c.hits(0, 90.0, 0.0), "well behind");
    }
}
