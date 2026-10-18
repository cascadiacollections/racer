//! Text pictures of a track, for tuning and for snapshot tests.
//!
//! The ASCII strip is the quickest way to read a generated track: one row per
//! 20 m, with the bend drawn as a bar either side of the centre line. It is
//! engine-free tooling, not presentation; the game draws nothing from here.
//! `bin/trackviz.rs` prints it (`--ascii`) and also draws the PNG picture.

use crate::track::{flags, Track, SEG_LEN};

/// Segments summarised per row of the strip: 5 segments, 20 m.
pub const ROW_SEGS: usize = 5;

/// Half width of the bend bar, characters. A full bar is a 120-milliradian
/// bend (a 33 m radius) or tighter.
const BAR_HALF: usize = 10;
const BAR_FULL_MILLI: f64 = 120.0;

/// The track as a strip chart, one row per [`ROW_SEGS`] segments:
///
/// ```text
///  z(m)       left|right        elev sight surf  flags
///   160           |#####         3.2   148 ..ggg D
/// ```
///
/// The bar shows the mean turn across the row, to the left for left-handers.
/// `elev` is metres at the row's start and `sight` how far ahead the road is
/// visible from there. `surf` is one letter per segment (`.` asphalt, `g`
/// gravel, `i` ice, `p` puddle). Flags: `D` drift required, `H` drift hint,
/// `C` blind crest, `o` roadside objects. Checkpoint rows end with `<- CP n`, the finish with
/// `<- FINISH`.
pub fn ascii_strip(t: &Track) -> String {
    let mut out = format!(
        "{:>5} {:>w$}|{:<w$} {:>6} {:>5} {:<5} flags\n",
        "z(m)",
        "left",
        "right",
        "elev",
        "sight",
        "surf",
        w = BAR_HALF
    );
    let segs = t.segments();
    let last_cp = t.checkpoints().len() - 1;
    for start in (0..segs.len()).step_by(ROW_SEGS) {
        let row = &segs[start..(start + ROW_SEGS).min(segs.len())];
        let mean = row.iter().map(|s| s.curve_milli as f64).sum::<f64>() / row.len() as f64;
        let n = ((mean.abs() / BAR_FULL_MILLI) * BAR_HALF as f64).ceil() as usize;
        let n = n.min(BAR_HALF);
        let (left, right) = if mean < 0.0 {
            (
                " ".repeat(BAR_HALF - n) + &"#".repeat(n),
                " ".repeat(BAR_HALF),
            )
        } else {
            (
                " ".repeat(BAR_HALF),
                "#".repeat(n) + &" ".repeat(BAR_HALF - n),
            )
        };
        let surf: String = row.iter().map(|s| s.surface.letter()).collect();
        let mut fl = String::new();
        for (flag, c) in [
            (flags::DRIFT_REQ, 'D'),
            (flags::DRIFT_HINT, 'H'),
            (flags::CREST, 'C'),
        ] {
            if row.iter().any(|s| s.has(flag)) {
                fl.push(c);
            }
        }
        if !t.objects_in(start, start + ROW_SEGS).is_empty() {
            fl.push('o');
        }
        let z = start as f64 * SEG_LEN;
        let mut line = format!(
            "{:>5} {left}|{right} {:>6.1} {:>5.0} {surf:<5} {fl:<3}",
            z as u32,
            t.elevation_at(z),
            t.sight(start)
        );
        let end = (start + ROW_SEGS) as u32;
        for (k, &cp) in t.checkpoints().iter().enumerate() {
            if (start as u32..end).contains(&cp) {
                if k == last_cp {
                    line.push_str(" <- FINISH");
                } else {
                    line.push_str(&format!(" <- CP {}", k + 1));
                }
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::fixtures;

    #[test]
    fn one_row_per_twenty_metres_and_a_header() {
        let t = fixtures::drag_strip();
        let s = ascii_strip(&t);
        assert_eq!(s.lines().count(), 1 + t.len().div_ceil(ROW_SEGS));
        assert!(s.contains("<- FINISH"));
    }

    #[test]
    fn bends_draw_on_their_own_side() {
        let s = ascii_strip(&fixtures::sweep_and_hairpin());
        let right = s.lines().find(|l| l.contains("|#")).expect("right bend");
        let left = s.lines().find(|l| l.contains("#|")).expect("left bend");
        assert_ne!(right, left);
    }
}
