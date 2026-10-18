//! Speed profiles: how fast a driver means to be at every point of a track.
//!
//! The classic two-pass construction. First, a *limit* per segment: the
//! fastest the car can hold its line there (`car::CarParams::grip_speed`,
//! or the drift speed in a corner the driver means to drift), scaled down by
//! the driver's caution. Then a *backward pass*: walking from the finish to
//! the start, each segment's target is the lower of its own limit and the
//! speed from which the car could still brake to the next segment's target,
//! `sqrt(v_next² + 2·a·d)`. The result is the speed a driver should never
//! exceed at each point; the throttle supplies the forward pass for free,
//! since speed can only rise as fast as the engine allows.
//!
//! The perfect bot follows a whole-track profile. The human bot builds the
//! same thing on the fly, but only over the road it can see (`human.rs`).

use super::Corner;
use crate::car::CarParams;
use crate::dmath::sqrt;
use crate::track::{Track, SEG_LEN};

/// Which segments the driver means to drift, from a choice per corner.
pub fn drift_mask(n: usize, corners: &[Corner], drift: &[bool]) -> Vec<bool> {
    let mut mask = vec![false; n];
    for (c, &d) in corners.iter().zip(drift) {
        if d {
            mask[c.start..c.end].fill(true);
        }
    }
    mask
}

/// The speed limit of every segment for a driver of `caution` (the share of
/// the tyres' grip it is willing to use, so `sqrt(caution)` of the limit
/// speed) drifting the segments in `drift`.
pub fn limits(track: &Track, params: &CarParams, caution: f64, drift: &[bool]) -> Vec<f64> {
    let k = sqrt(caution);
    track
        .segments()
        .iter()
        .zip(drift)
        .map(|(s, &d)| {
            let corner = if d {
                params.drift_speed(s)
            } else {
                params.grip_speed(s)
            };
            (corner * k).min(params.vmax_on(s))
        })
        .collect()
}

/// The backward pass: the most speed allowed at the start of each segment
/// such that braking at `decel` reaches every later limit in time.
pub fn backward(limits: &[f64], decel: f64) -> Vec<f64> {
    let mut out = limits.to_vec();
    for i in (0..out.len().saturating_sub(1)).rev() {
        let reach = sqrt(out[i + 1] * out[i + 1] + 2.0 * decel * SEG_LEN);
        out[i] = out[i].min(reach);
    }
    out
}

/// The target speed at distance `z` along a profile: the segment's own limit,
/// or what still brakes in time for the next segment, whichever is lower.
pub fn target_at(profile: &[f64], limits: &[f64], z: f64, decel: f64) -> f64 {
    let n = profile.len();
    let i = ((z / SEG_LEN).max(0.0) as usize).min(n - 1);
    if i + 1 >= n {
        return limits[i];
    }
    let to_next = (i + 1) as f64 * SEG_LEN - z;
    let reach = sqrt(profile[i + 1] * profile[i + 1] + 2.0 * decel * to_next.max(0.0));
    limits[i].min(reach)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::corners;
    use crate::track::fixtures;

    #[test]
    fn the_backward_pass_brakes_in_time_for_the_hairpin() {
        let t = fixtures::sweep_and_hairpin();
        let p = CarParams::BASE;
        let cs = corners(&t, &p);
        let none = drift_mask(t.len(), &cs, &[false]);
        let lim = limits(&t, &p, 1.0, &none);
        let prof = backward(&lim, 15.0);
        let c = cs[0];
        // At the corner, the profile is the grip speed; well before it, more.
        assert!((prof[c.start + 10] - lim[c.start + 10]).abs() < 1e-9);
        assert!(prof[c.start - 20] > prof[c.start] + 5.0);
        // Never above the limit, and never rising faster than braking allows
        // when read backwards.
        for i in 0..prof.len() - 1 {
            assert!(prof[i] <= lim[i] + 1e-9);
            assert!(prof[i] * prof[i] <= prof[i + 1] * prof[i + 1] + 2.0 * 15.0 * SEG_LEN + 1e-6);
        }
    }

    #[test]
    fn drifting_raises_the_limit_and_caution_lowers_it() {
        let t = fixtures::sweep_and_hairpin();
        let p = CarParams::BASE;
        let cs = corners(&t, &p);
        let apex = cs[0].start + 10;
        let grip = limits(&t, &p, 1.0, &drift_mask(t.len(), &cs, &[false]));
        let drift = limits(&t, &p, 1.0, &drift_mask(t.len(), &cs, &[true]));
        let careful = limits(&t, &p, 0.85, &drift_mask(t.len(), &cs, &[false]));
        assert!(drift[apex] > grip[apex]);
        assert!(careful[apex] < grip[apex]);
        assert_eq!(grip[0], p.vmax, "straights are limited by top speed");
    }

    #[test]
    fn the_target_eases_toward_the_next_segment() {
        let lim = vec![50.0, 20.0];
        let prof = backward(&lim, 10.0);
        let at_start = target_at(&prof, &lim, 0.0, 10.0);
        let at_end = target_at(&prof, &lim, 3.99, 10.0);
        assert!(at_start > at_end && at_end > 20.0);
        assert_eq!(target_at(&prof, &lim, 6.0, 10.0), 20.0);
    }
}
