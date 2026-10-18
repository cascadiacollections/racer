//! One tick of player input, as one byte. DESIGN §3.1.
//!
//! Every way of playing — touch zones, keys, a pad's stick — quantises to the
//! same [`Input`]: seven steering levels, a brake bit and a drift bit. The
//! throttle is automatic. Quantising at the edge is what makes the sim
//! indifferent to the device: a run played on a phone and one played on a
//! keyboard log the same kind of thing, and a replay cannot tell them apart.
//!
//! Layout of the byte: bits 0–2 hold the steering level `0..=6` (3 is
//! straight ahead), bit 3 brakes, bit 4 drifts. Bits 5–7 are always clear.

/// Steering levels either side of centre. Seven in all: full, two-thirds and
/// one-third lock each way, and straight ahead.
pub const STEER_LEVELS: i8 = 3;

const STEER_MASK: u8 = 0b0000_0111;
const BRAKE_BIT: u8 = 0b0000_1000;
const DRIFT_BIT: u8 = 0b0001_0000;

/// A tick of input. See the module docs for the bit layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Input(u8);

impl Default for Input {
    /// Straight ahead, no brake, no drift.
    fn default() -> Self {
        Input::NEUTRAL
    }
}

impl Input {
    /// Straight ahead, no brake, no drift.
    pub const NEUTRAL: Input = Input(STEER_LEVELS as u8);

    /// An input from a steering level (clamped to `-3..=3`, negative left)
    /// and the two buttons.
    pub const fn new(steer: i8, brake: bool, drift: bool) -> Input {
        let s = if steer < -STEER_LEVELS {
            -STEER_LEVELS
        } else if steer > STEER_LEVELS {
            STEER_LEVELS
        } else {
            steer
        };
        let mut b = (s + STEER_LEVELS) as u8;
        if brake {
            b |= BRAKE_BIT;
        }
        if drift {
            b |= DRIFT_BIT;
        }
        Input(b)
    }

    /// Keys or d-pad: either direction is full lock; both cancel out.
    pub const fn from_keys(left: bool, right: bool, brake: bool, drift: bool) -> Input {
        let steer = match (left, right) {
            (true, false) => -STEER_LEVELS,
            (false, true) => STEER_LEVELS,
            _ => 0,
        };
        Input::new(steer, brake, drift)
    }

    /// A stick or a touch drag, `axis` in `-1..=1` (negative left), rounded
    /// to the nearest of the seven levels. NaN steers straight.
    pub fn from_axis(axis: f64, brake: bool, drift: bool) -> Input {
        let a = if axis.is_nan() {
            0.0
        } else {
            crate::dmath::clamp(axis, -1.0, 1.0)
        };
        let level = crate::dmath::round(a * STEER_LEVELS as f64) as i8;
        Input::new(level, brake, drift)
    }

    /// The byte, as logged.
    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Decode a logged byte. `None` for a steering level past 6 or any of the
    /// unused high bits set: a log holding one is corrupt, not lenient.
    pub const fn from_bits(b: u8) -> Option<Input> {
        if b & !(STEER_MASK | BRAKE_BIT | DRIFT_BIT) != 0 || (b & STEER_MASK) > 6 {
            None
        } else {
            Some(Input(b))
        }
    }

    /// Steering level, `-3..=3`, negative left.
    #[inline]
    pub const fn steer_level(self) -> i8 {
        (self.0 & STEER_MASK) as i8 - STEER_LEVELS
    }

    /// Steering as a fraction of full lock, `-1..=1`.
    #[inline]
    pub fn steer(self) -> f64 {
        self.steer_level() as f64 / STEER_LEVELS as f64
    }

    /// Brake held.
    #[inline]
    pub const fn brake(self) -> bool {
        self.0 & BRAKE_BIT != 0
    }

    /// Drift held.
    #[inline]
    pub const fn drift(self) -> bool {
        self.0 & DRIFT_BIT != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_valid_byte_round_trips() {
        let mut valid = 0;
        for b in 0..=u8::MAX {
            if let Some(i) = Input::from_bits(b) {
                valid += 1;
                assert_eq!(i.bits(), b);
                assert_eq!(Input::new(i.steer_level(), i.brake(), i.drift()), i);
            }
        }
        // Seven levels, times brake, times drift.
        assert_eq!(valid, 7 * 2 * 2);
    }

    #[test]
    fn devices_quantise_to_the_same_levels() {
        assert_eq!(
            Input::from_keys(true, false, false, false).steer_level(),
            -3
        );
        assert_eq!(Input::from_keys(true, true, false, false), Input::NEUTRAL);
        assert_eq!(Input::from_axis(0.5, false, false).steer_level(), 2);
        assert_eq!(Input::from_axis(-0.1, false, false), Input::NEUTRAL);
        assert_eq!(Input::from_axis(9.0, true, true), Input::new(3, true, true));
        assert_eq!(Input::from_axis(f64::NAN, false, false), Input::NEUTRAL);
        assert_eq!(Input::default(), Input::NEUTRAL);
    }

    #[test]
    fn steer_is_a_fraction_of_full_lock() {
        assert_eq!(Input::new(-3, false, false).steer(), -1.0);
        assert_eq!(Input::new(1, false, false).steer(), 1.0 / 3.0);
        assert_eq!(Input::new(99, false, false).steer_level(), 3);
    }
}
