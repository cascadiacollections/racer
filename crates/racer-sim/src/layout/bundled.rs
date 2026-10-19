//! The bundled courses: proven layouts that ship with the game. DESIGN §9.7.
//!
//! Each is a generated track forked from a seed and lightly tuned by hand
//! (its [`Course::tuning`]), then proven under the custom-race rules. The
//! codes are the courses: fixed data, compiled in (as Salt Line's are), so
//! they are the same text everywhere and testable without an engine. Where
//! each came from is recorded beside its code ([`Course::origin`]), never in
//! it.
//!
//! The recipes are in this module's tests: a fork, then the tuning. Under
//! the epoch the courses were made in ([`MADE_IN`]) a test holds every code
//! to its recipe, proven again; under a later epoch the recipes fork other
//! roads, and the codes, not the recipes, are what ships. Every epoch, a
//! test proves every course again and requires the timers it carries
//! (or, after a bump, the course re-proven: `layout prove` with the code).
//!
//! Each course keeps the biome its fork had (DESIGN §5.1): FIRST LIGHT a
//! meadow (its code is the format-1 code it always was), RIDGE RUN alpine,
//! GRAVEL PASS desert dusk, LONG DUSK night city. Adding the biomes changed
//! only the last three codes' format byte and tail: their stamps and
//! timers are the same.
//!
//! To remake the codes after changing a recipe:
//! `cargo test -p racer-sim --lib layout::bundled -- --ignored --nocapture`.

use super::Layout;

/// One bundled course.
#[derive(Clone, Copy, Debug)]
pub struct Course {
    /// Its share code: a proven layout, named.
    pub code: &'static str,
    /// Where it was forked from ([`super::Provenance`]'s text).
    pub origin: &'static str,
    /// What was changed by hand after the fork.
    pub tuning: &'static str,
}

/// The `SIM_EPOCH` the courses were made and proven in.
pub const MADE_IN: u32 = 1;

/// The courses, easiest first.
pub const COURSES: [Course; 4] = [
    Course {
        code: concat!(
            "RL04MSP544A9745J0REKW9ZZZZZ00HP0Z0ZH492FHFR20H7RQW1141Y1XST01Y4H",
            "C9Y40FC824Z207P44G7P03WSTJ4A441J0FY068G2192CR7G9E7B2R1YG5GSEAJGC",
            "Q5R70JWEP5G3ZZ9MC0JZW4GGAFZG1G4G869K21ZZM9H4RC493J7WG0ZK4CH7WR0Z",
            "G412FSG1Z0820ZG03YCHZZZX0CH021J4SGFZ8K68GR8G681ZZ93CB0GCQ6W7ZZ1K",
            "EBGR8P6R5ZZZ028803JC3R4K3G0GG48W044G3G14401S650G840G00QZT8008084",
            "23ZZZKG5ZZZZW00ZZSFJR888A4G90120008CC8H2CRMK96PJJJSFZY3ZZ1DPJS9B",
            "6PJ54RKH2080J09022H5462399YVZFZZZZR0BVZZ9ZMWQDJWPB5JJPPCSH208048",
            "JJJ31GRE31TE3ZDZYZZZZZG06N015JW0X4GTW0F750JA06Y06J06H2R1E005F092",
            "8J4H4928J4P00S2R02JH00A6D1GDQC787N5W",
        ),
        origin: "fork:0x0000000000000003:0:0:e1",
        tuning: "a puddle on the straight after the first gate",
    },
    Course {
        code: concat!(
            "RL08M7JH40RR948N1QH7ZZX6BJ5G5GBMHC864K4H0RJW7TCQ4B0B0QZX8EGT10MT",
            "KN2ZZZZZ006D9TKA5238EG9ZZZZW01PG60C049H0Z8K28G0RP5S203300Z030602",
            "4RGF8001018G87ZW210M810000R52G80QZR80G8M84P432BGEH5S2R2GN4B0B07T",
            "0P432BG09019GZZMRD2090QZZMSQ5RC4B3C2ZT8V2R435SWSZHFX14S3YHZN4S4F",
            "T7YMK4SZAZT2BZY26RP219EDRFZZ9KEBH8GP6R5ZZYY0ZZZZZG0ABYDVMJS0M842",
            "0RGR88J5RWMCCWMX2H8NA5EKDR38S61GB4J94CC481GJHKKMTD6QCB2SPM9S4HJ9",
            "0CCC8C448400101GGGHJHHKJKKKMACX6WKTFZZZZ01F202B48GY82D06982Q0HP6",
            "M16Q0BN043C0MW0K80M40HM2404700AY8J4H4928J400AY0928J4H49280NR015G",
            "KKKKG2M0348J4H4928J4H4928J4H800AD3G1G40680014402K01J201VJ0C2009D",
            "C0V80CGR02YQ00YYG0F7R07G53NB8TA94AB8",
        ),
        origin: "fork:0x0000000000000005:2:1:e1",
        tuning: "none: the generated road as it is",
    },
    Course {
        code: concat!(
            "RL08MRP68G2M8BJ3R14JW9ZZZ8JWHC1C0R0P432AJGGC9E1YH5S2R3G7T7WS349Y",
            "DZ4824Z6ZJ44G7S7WRY08G261ZZR6180A20ZZZZY00K13J68C76CWRFZZZZGSSK6",
            "7334E85ZZZZW01NK768J0WGBW4E8G49SGZZZR4K3G0GJ0E04GG07490401S61ZG9",
            "6701140X218W0G0FZW008C2H0ZZZZC1Z007WS349Z607W108KYC0FR21G7W00ZK4",
            "FZZZZG049R2ZZZZY0127VFJMGGGG8H4040G0102448RH2C9H96CTAAAAVW7TZZZZ",
            "Y00X7ZJZVE319MMMR4MG8J0003H24A9K9B6SBBDNPBJZTFZ3YZDWM5452880G0G2",
            "GGHHK2CA8MMKB9B9F1YQZZZFZVFZ5X71TEK1MTAA884A0CSMMNNPVDJXQDSFDZW7",
            "ZXFZZZZ00WE02B4Y3S09C0JD0F40AJ44Q06P83SC0QR0KM0HC0K8243W02QG4H49",
            "28J400AY4928J4H4903G004P0EEEE04B0340J4H4928G01BRH4928J4H4905001B",
            "RJ000KQQ02X80CGG04YJ1CG02AC05A01J0G05GA00GD806AM03AYJHP4BB0SRPY",
        ),
        origin: "fork:0x0000000000000007:3:0:e1",
        tuning: "gravel laid over the drift corner",
    },
    Course {
        code: concat!(
            "RL08NVJB738T83A4HBJ5ZZZ9HY8WHCA3G7R1FJGZ0E0M5HS3WGZZTCF4DG0R3R1E",
            "23R3006RWHR3006RY8FDHWHP030F05ZZYJ3Z0FG21HY8ZS1ZZX67Y8Z4623W1ZG2",
            "ZXP7V3CD1YGB91YGT3CFP3M0X10S3P1T4ERJC1R3J4K0EG203M434ER0084J8FTC",
            "29420863D0QGG9P31010J91ZDG94G81C90G8004EW000212FZ11YGY0G4HWHYRFP",
            "RZCF4920Y0Z8094G10HYG4204J9ZWE0QZZZZG01VY3RB99996D49H77HGS90410H",
            "444457JZ7ZZZZR044ZW7TDPV5PJS9B6CSRR924991GGTC6K9YVZKZZZZW01BZYVY",
            "V9GRC55454420G044C8MH6JJPJPPSCPBDZ9ZTZZZZY02MW04P9W11M13G1CS0470",
            "VP5VG105041C0S80Q40JM0HC3P08G034660049G4H4928J4H4800NW8J4H4928J4",
            "G23H4928J4H4928J4H4928J4H4928J4H40GW928J4H4928G01BR14928J04G0349",
            "SSSSRG005N880J2034401SV04A0340026A1K01J300BTW017CG0E28066XEGNY28",
            "20MMH0",
        ),
        origin: "fork:0x0000000000000008:4:0:e1",
        tuning: "the last traffic car taken off",
    },
];

/// Every course, decoded. A code that does not decode is a build error
/// caught by the tests, so the game can rely on these.
pub fn layouts() -> Vec<Layout> {
    COURSES
        .iter()
        .filter_map(|c| Layout::from_code(c.code).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::Biome;
    use crate::layout::{fork, Provenance, Standing, Verified};
    use crate::track::{flags, Surface};

    /// Course `k` as its recipe makes it, unproven.
    fn recipe(k: usize) -> Layout {
        let p = Provenance::parse(COURSES[k].origin).unwrap();
        let mut l = fork(p.run_seed, p.index, p.branch).layout;
        match k {
            0 => {
                l.name = "FIRST LIGHT".into();
                let from = l.checkpoints[0] as usize + 12;
                for s in &mut l.segments[from..from + 6] {
                    s.surface = Surface::Puddle;
                }
            }
            1 => l.name = "RIDGE RUN".into(),
            2 => {
                l.name = "GRAVEL PASS".into();
                for s in &mut l.segments {
                    if s.has(flags::DRIFT_REQ) {
                        s.surface = Surface::Gravel;
                    }
                }
            }
            _ => {
                l.name = "LONG DUSK".into();
                l.traffic.pop();
            }
        }
        l
    }

    #[test]
    fn every_course_decodes_proven_and_without_problems() {
        assert_eq!(layouts().len(), COURSES.len());
        for (c, l) in COURSES.iter().zip(layouts()) {
            assert!(c.code.starts_with(super::super::SHARE_PREFIX));
            assert_eq!(l.check(), vec![], "{}", l.name);
            assert!(Provenance::parse(c.origin).is_some());
            if crate::SIM_EPOCH == MADE_IN {
                assert_eq!(l.standing(), Standing::Proven, "{}", l.name);
            }
        }
        let names: Vec<String> = layouts().into_iter().map(|l| l.name).collect();
        assert_eq!(
            names,
            ["FIRST LIGHT", "RIDGE RUN", "GRAVEL PASS", "LONG DUSK"]
        );
        // Each keeps its fork's scenery (the track index's biome); only
        // the meadow is still a format-1 code.
        let biomes: Vec<Biome> = layouts().into_iter().map(|l| l.biome).collect();
        assert_eq!(
            biomes,
            [Biome::Meadow, Biome::Alpine, Biome::Desert, Biome::Night]
        );
    }

    #[test]
    fn every_course_proves_to_the_timers_it_carries() {
        for l in layouts() {
            assert_eq!(l.verify(), Verified::Confirmed, "{}", l.name);
            let (_, report) = l.prove().unwrap();
            // The same gates as a generated track.
            let (lo, hi) = crate::gen::difficulty::PERFECT_LEFT;
            assert!((lo..=hi).contains(&report.perfect_left), "{}", l.name);
            assert!(report.fewest_passes >= crate::solver::MARGIN_PASSES);
            assert!(report.drift.iter().all(|d| d.grip_failed || d.loss >= 1.5));
        }
    }

    #[test]
    fn every_code_is_its_recipe_proven() {
        if crate::SIM_EPOCH != MADE_IN {
            return;
        }
        for (k, l) in layouts().into_iter().enumerate() {
            let (proven, _) = recipe(k).prove().unwrap();
            assert_eq!(proven, l, "course {k}");
        }
    }

    #[test]
    #[ignore = "prints the codes to paste into COURSES"]
    fn print_codes() {
        for k in 0..COURSES.len() {
            match recipe(k).prove() {
                Ok((p, r)) => println!(
                    "{}: {} ({} chars, perfect left {}, gap {:.3})",
                    p.name,
                    p.to_code().unwrap(),
                    p.to_code().unwrap().len(),
                    r.perfect_left,
                    r.skill_gap
                ),
                Err(e) => println!("course {k}: {e}"),
            }
        }
    }
}
