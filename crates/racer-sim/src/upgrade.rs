//! Upgrades, the loadout they stack into, and which loadouts a track can
//! meet. DESIGN §4.
//!
//! After each track the route forks between two tracks, and each side of
//! the fork carries a different upgrade. Taking a side takes its upgrade for
//! that track and every track after; upgrades stack, so the same one twice
//! counts twice.
//!
//! Every upgrade only helps in M1, each along one axis, so a loadout is a
//! small, legible set of numbers. "Upgrades that can hurt" (top speed bought
//! with grip) is an open item for later milestones (DESIGN §4.2).
//!
//! The solver needs the *reachable* loadouts of a track: everything a
//! player can hold on arriving there. Because each fork's two upgrades
//! depend only on `(run_seed, index)`, that set is known before anyone
//! drives, and every one of them is checked (DESIGN §2.4).

use crate::car::CarParams;
use crate::rng::{mix64, Pcg32};

/// The upgrades, one axis each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Upgrade {
    /// Top speed +5%.
    Engine = 0,
    /// Acceleration +12%.
    Turbo = 1,
    /// Grip +7%.
    Tyres = 2,
    /// Brakes +20%.
    Brakes = 3,
    /// Drift charge +30%: boosts come sooner.
    Charger = 4,
}

/// How many upgrades there are.
pub const UPGRADES: usize = 5;

impl Upgrade {
    /// Every upgrade, in discriminant order.
    pub const ALL: [Upgrade; UPGRADES] = [
        Upgrade::Engine,
        Upgrade::Turbo,
        Upgrade::Tyres,
        Upgrade::Brakes,
        Upgrade::Charger,
    ];

    /// The factor one stack multiplies its axis by.
    pub const fn factor(self) -> f64 {
        match self {
            Upgrade::Engine => 1.05,
            Upgrade::Turbo => 1.12,
            Upgrade::Tyres => 1.07,
            Upgrade::Brakes => 1.20,
            Upgrade::Charger => 1.30,
        }
    }

    /// Short name, for reports.
    pub const fn name(self) -> &'static str {
        match self {
            Upgrade::Engine => "engine",
            Upgrade::Turbo => "turbo",
            Upgrade::Tyres => "tyres",
            Upgrade::Brakes => "brakes",
            Upgrade::Charger => "charger",
        }
    }
}

/// Stacks held of each upgrade.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Loadout {
    stacks: [u8; UPGRADES],
}

impl Loadout {
    /// No upgrades.
    pub const BASE: Loadout = Loadout {
        stacks: [0; UPGRADES],
    };

    /// This loadout plus one stack of `u`.
    pub fn with(mut self, u: Upgrade) -> Loadout {
        self.stacks[u as usize] += 1;
        self
    }

    /// Stacks of `u` held.
    pub fn stacks(&self, u: Upgrade) -> u8 {
        self.stacks[u as usize]
    }

    /// The car this loadout drives: each axis times its factor per stack.
    pub fn params(&self) -> CarParams {
        let f = |u: Upgrade| {
            let mut x = 1.0;
            for _ in 0..self.stacks(u) {
                x *= u.factor();
            }
            x
        };
        let b = CarParams::BASE;
        CarParams {
            vmax: b.vmax * f(Upgrade::Engine),
            accel: b.accel * f(Upgrade::Turbo),
            grip: b.grip * f(Upgrade::Tyres),
            brake: b.brake * f(Upgrade::Brakes),
            charge_rate: b.charge_rate * f(Upgrade::Charger),
        }
    }

    /// e.g. `tyres+engine x2`, or `base`.
    pub fn describe(&self) -> String {
        let parts: Vec<String> = Upgrade::ALL
            .iter()
            .filter(|&&u| self.stacks(u) > 0)
            .map(|&u| match self.stacks(u) {
                1 => u.name().to_string(),
                n => format!("{} x{n}", u.name()),
            })
            .collect();
        if parts.is_empty() {
            "base".into()
        } else {
            parts.join("+")
        }
    }
}

/// Tag for the fork-upgrade draw: "FORK" in ASCII.
const FORK_TAG: u64 = 0x464F_524B;

/// The two upgrades on offer at the fork into track `index` (1 and up): two
/// different upgrades, branch 0's first. Track 0 has no fork.
pub fn fork_upgrades(run_seed: u64, index: u32) -> [Upgrade; 2] {
    let mut r = Pcg32::new(mix64(run_seed ^ mix64(FORK_TAG ^ index as u64)));
    let mut all = Upgrade::ALL;
    r.shuffle(&mut all);
    [all[0], all[1]]
}

/// The upgrade that comes with track `(index, branch)`, if any.
pub fn upgrade_of(run_seed: u64, index: u32, branch: u8) -> Option<Upgrade> {
    (index > 0).then(|| fork_upgrades(run_seed, index)[branch as usize])
}

/// Every loadout a player can hold on track `(index, branch)`: that track's
/// own upgrade plus one side of every earlier fork. Distinct and sorted, so
/// the order is the same everywhere. At most `2^(index-1)` of them.
pub fn reachable(run_seed: u64, index: u32, branch: u8) -> Vec<Loadout> {
    let mut set = vec![Loadout::BASE];
    for j in 1..index {
        let [a, b] = fork_upgrades(run_seed, j);
        set = set.iter().flat_map(|l| [l.with(a), l.with(b)]).collect();
    }
    if let Some(u) = upgrade_of(run_seed, index, branch) {
        set = set.iter().map(|l| l.with(u)).collect();
    }
    set.sort_unstable();
    set.dedup();
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_stack_multiplicatively() {
        let l = Loadout::BASE.with(Upgrade::Tyres).with(Upgrade::Tyres);
        assert!((l.params().grip - CarParams::BASE.grip * 1.07 * 1.07).abs() < 1e-12);
        assert_eq!(l.params().vmax, CarParams::BASE.vmax);
        assert_eq!(Loadout::BASE.params(), CarParams::BASE);
        assert_eq!(l.describe(), "tyres x2");
        assert_eq!(Loadout::BASE.describe(), "base");
    }

    #[test]
    fn a_fork_offers_two_different_upgrades() {
        for seed in 0..200 {
            for index in 1..5 {
                let [a, b] = fork_upgrades(seed, index);
                assert_ne!(a, b);
                assert_eq!(fork_upgrades(seed, index), [a, b]);
            }
        }
    }

    #[test]
    fn reachable_loadouts_grow_with_the_forks_behind() {
        for seed in 0..50 {
            assert_eq!(reachable(seed, 0, 0), vec![Loadout::BASE]);
            assert_eq!(
                reachable(seed, 1, 1),
                vec![Loadout::BASE.with(fork_upgrades(seed, 1)[1])]
            );
            for index in 2..5u32 {
                let r = reachable(seed, index, 0);
                assert!(!r.is_empty() && r.len() <= 1 << (index - 1));
                let own = fork_upgrades(seed, index)[0];
                assert!(r.iter().all(|l| l.stacks(own) >= 1));
                let total: u8 = Upgrade::ALL.iter().map(|&u| r[0].stacks(u)).sum();
                assert_eq!(total as u32, index);
            }
        }
    }
}
