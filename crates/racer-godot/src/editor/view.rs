//! Where the editor's things are on screen, and what a touch there means
//! (DESIGN §9.11). Engine-free: the drawing (`draw.rs`) and the input
//! (`mod.rs`) share these rectangles and projections, so a touch always
//! means what the player saw under it.
//!
//! ```text
//!   ┌──────────────────────────┐ 0
//!   │ name             BADGE   │ top bar
//!   │                          │
//!   │      top-down map        │ the road from above (heading integrated
//!   │   (tap: pick a run)      │ from the curve, as trackviz draws it)
//!   │                          │
//!   ├──────────────────────────┤
//!   │   elevation strip        │ height along the road (tap: pick)
//!   ├──◀─── run 3/14 ─────▶────┤ info
//!   │BEND HILL WIDE SURF DRIFT │ tools
//!   │OBJ  CARS GATE RUN  MENU  │
//!   │  -      R 40      +      │ the tool's options
//!   │UNDO  REDO  TEST  PROVE   │ actions
//!   └──────────────────────────┘ bottom (above the gesture inset)
//! ```
//!
//! Every control is in the bottom 110 rows, the thumb zone; the map and
//! the strip above are for looking and picking.

use racer_sim::dmath;
use racer_sim::track::{Segment, SEG_LEN};

use crate::layout::{Layout, Rect, W};

/// The editor's tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Bend,
    Hill,
    Width,
    Surface,
    Drift,
    Objects,
    Cars,
    Gate,
    Run,
}

impl Tool {
    /// Every tool, in the grid's order.
    pub const ALL: [Tool; 9] = [
        Tool::Bend,
        Tool::Hill,
        Tool::Width,
        Tool::Surface,
        Tool::Drift,
        Tool::Objects,
        Tool::Cars,
        Tool::Gate,
        Tool::Run,
    ];

    /// Its button's label.
    pub fn label(self) -> &'static str {
        match self {
            Tool::Bend => "BEND",
            Tool::Hill => "HILL",
            Tool::Width => "WIDTH",
            Tool::Surface => "SURF",
            Tool::Drift => "DRIFT",
            Tool::Objects => "OBJ",
            Tool::Cars => "CARS",
            Tool::Gate => "GATE",
            Tool::Run => "RUN",
        }
    }

    /// Whether it is a stepper (minus, value, plus) rather than options.
    pub fn stepper(self) -> bool {
        matches!(self, Tool::Bend | Tool::Hill | Tool::Width)
    }

    /// Its options' labels (none for a stepper).
    pub fn options(self) -> &'static [&'static str] {
        match self {
            Tool::Surface => &["ASPHALT", "GRAVEL", "ICE", "WATER"],
            Tool::Drift => &["NONE", "HINT", "MUST"],
            Tool::Objects => &["POST L", "POST R", "ROCK L", "ROCK C", "ROCK R", "CLEAR"],
            Tool::Cars => &["CAR L", "CAR C", "CAR R", "CLEAR"],
            Tool::Gate => &["GATE", "< MOVE", "MOVE >"],
            Tool::Run => &["SPLIT", "JOIN", "ADD", "DEL", "LEN-", "LEN+"],
            _ => &[],
        }
    }
}

/// What a button on an editor page does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Pick a tool.
    Tool(Tool),
    /// The tool's option `i`.
    Opt(u8),
    /// The stepper's minus.
    Minus,
    /// The stepper's plus.
    Plus,
    /// The run before.
    PrevRun,
    /// The run after.
    NextRun,
    Undo,
    Redo,
    Test,
    Prove,
    /// The editor's menu.
    Menu,
    /// A menu item.
    Item(Item),
    /// Back a page.
    Back,
    /// A keypad key: a character, or 8 (delete) or 13 (done).
    Key(u8),
    /// Slot `n` (saving), or a list row (the tracks list).
    Slot(u8),
    /// The tracks list's row `i` on this page.
    Row(u8),
    /// The tracks list's page up or down.
    Page(i8),
    /// SEED page: track index down or up.
    Index(i8),
    /// SEED page: easy or hard road.
    Branch,
    /// The map (a touch there picks a run).
    Map,
    /// The elevation strip.
    Strip,
}

/// The editor's menu items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    TestDrive,
    Prove,
    Save,
    Copy,
    Name,
    Biome,
    Paste,
    Seed,
    Blank,
    Discard,
    Leave,
}

impl Item {
    /// The menu, in order: two columns.
    pub const ALL: [Item; 11] = [
        Item::TestDrive,
        Item::Prove,
        Item::Save,
        Item::Copy,
        Item::Name,
        Item::Biome,
        Item::Paste,
        Item::Seed,
        Item::Blank,
        Item::Discard,
        Item::Leave,
    ];

    /// Its label.
    pub fn label(self) -> &'static str {
        match self {
            Item::TestDrive => "TEST DRIVE",
            Item::Prove => "PROVE",
            Item::Save => "SAVE",
            Item::Copy => "COPY CODE",
            Item::Name => "NAME",
            Item::Biome => "BIOME",
            Item::Paste => "PASTE CODE",
            Item::Seed => "FROM SEED",
            Item::Blank => "NEW BLANK",
            Item::Discard => "DISCARD",
            Item::Leave => "TRACKS",
        }
    }
}

/// Heights of the editing page's rows, from the bottom up.
const ACTIONS_H: i32 = 22;
const OPTS_H: i32 = 24;
const TOOL_H: i32 = 18;
const INFO_H: i32 = 18;
const STRIP_H: i32 = 28;
/// The top bar.
pub const TOP_H: i32 = 14;

/// The editing page's rectangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditRects {
    pub top: Rect,
    pub map: Rect,
    pub strip: Rect,
    pub info: Rect,
    pub prev: Rect,
    pub next: Rect,
    /// The tool grid: two rows of five, the last [`Act::Menu`].
    pub tools: [Rect; 10],
    pub opts: Rect,
    pub undo: Rect,
    pub redo: Rect,
    pub test: Rect,
    pub prove: Rect,
}

impl EditRects {
    /// The editing page in layout `l`.
    pub fn of(l: &Layout) -> EditRects {
        let b = l.bottom();
        let act_y = b - ACTIONS_H - 2;
        let opt_y = act_y - OPTS_H - 2;
        let tool_y = opt_y - 2 * TOOL_H - 3;
        let info_y = tool_y - INFO_H - 1;
        let strip_y = info_y - STRIP_H - 1;
        let mut tools = [Rect::default(); 10];
        for (i, t) in tools.iter_mut().enumerate() {
            let (col, row) = (i as i32 % 5, i as i32 / 5);
            *t = Rect::new(col * 36, tool_y + row * (TOOL_H + 1), 36, TOOL_H);
        }
        let aw = W / 4;
        EditRects {
            top: Rect::new(0, 0, W, TOP_H),
            map: Rect::new(0, TOP_H, W, strip_y - 1 - TOP_H),
            strip: Rect::new(0, strip_y, W, STRIP_H),
            info: Rect::new(0, info_y, W, INFO_H),
            prev: Rect::new(0, info_y, 32, INFO_H),
            next: Rect::new(W - 32, info_y, 32, INFO_H),
            tools,
            opts: Rect::new(0, opt_y, W, OPTS_H),
            undo: Rect::new(0, act_y, aw, ACTIONS_H),
            redo: Rect::new(aw, act_y, aw, ACTIONS_H),
            test: Rect::new(2 * aw, act_y, aw, ACTIONS_H),
            prove: Rect::new(3 * aw, act_y, W - 3 * aw, ACTIONS_H),
        }
    }

    /// The tool's option buttons (or minus, value, plus for a stepper).
    pub fn options(&self, tool: Tool) -> Vec<(Rect, Act)> {
        let r = self.opts;
        if tool.stepper() {
            return vec![
                (Rect::new(r.x, r.y, 44, r.h), Act::Minus),
                (Rect::new(r.x + r.w - 44, r.y, 44, r.h), Act::Plus),
            ];
        }
        let n = tool.options().len() as i32;
        (0..n)
            .map(|i| {
                let x0 = r.x + i * r.w / n;
                let x1 = r.x + (i + 1) * r.w / n;
                (Rect::new(x0, r.y, x1 - x0, r.h), Act::Opt(i as u8))
            })
            .collect()
    }
}

/// The road from above: one point per segment boundary, metres, `x` right
/// and `y` up the screen's way (north is up), starting at the origin
/// heading north. Platform-stable trig, so a drawn map is the same pixels
/// everywhere.
pub fn plan(segments: &[Segment]) -> Vec<(f64, f64)> {
    let mut pts = Vec::with_capacity(segments.len() + 1);
    let (mut x, mut y, mut th) = (0.0f64, 0.0f64, 0.0f64);
    pts.push((x, y));
    for s in segments {
        th += s.curvature() * SEG_LEN;
        x += dmath::sin(th) * SEG_LEN;
        y -= dmath::cos(th) * SEG_LEN;
        pts.push((x, y));
    }
    pts
}

/// A projection of [`plan`]'s points into a rectangle, keeping the aspect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    x0: f64,
    y0: f64,
    k: f64,
    ox: f64,
    oy: f64,
}

impl Fit {
    /// Fit `pts` into `r`, `pad` pixels in from its edges.
    pub fn new(pts: &[(f64, f64)], r: Rect, pad: i32) -> Fit {
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &(x, y) in pts {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        let (w, h) = ((r.w - 2 * pad) as f64, (r.h - 2 * pad) as f64);
        let k = (w / (x1 - x0).max(1.0)).min(h / (y1 - y0).max(1.0));
        Fit {
            x0,
            y0,
            k,
            ox: r.x as f64 + (r.w as f64 - (x1 - x0) * k) / 2.0,
            oy: r.y as f64 + (r.h as f64 - (y1 - y0) * k) / 2.0,
        }
    }

    /// A point's pixel.
    pub fn px(&self, p: (f64, f64)) -> (i32, i32) {
        (
            dmath::round(self.ox + (p.0 - self.x0) * self.k) as i32,
            dmath::round(self.oy + (p.1 - self.y0) * self.k) as i32,
        )
    }

    /// Pixels per metre.
    pub fn scale(&self) -> f64 {
        self.k
    }
}

/// The segment drawn nearest the touch `(x, y)` on the map.
pub fn map_pick(pts: &[(f64, f64)], fit: &Fit, x: f32, y: f32) -> Option<usize> {
    let n = pts.len().checked_sub(1)?;
    (0..n)
        .map(|i| {
            let (a, b) = (fit.px(pts[i]), fit.px(pts[i + 1]));
            let (mx, my) = ((a.0 + b.0) as f32 / 2.0, (a.1 + b.1) as f32 / 2.0);
            let d = (mx - x) * (mx - x) + (my - y) * (my - y);
            (i, d)
        })
        .min_by(|p, q| p.1.total_cmp(&q.1))
        .map(|(i, _)| i)
}

/// The column of the elevation strip `r` segment `seg` of `n` is drawn in.
pub fn strip_x(r: Rect, seg: usize, n: usize) -> i32 {
    r.x + 2 + (seg as i64 * (r.w - 4) as i64 / n.max(1) as i64) as i32
}

/// The segment under column `x` of the elevation strip `r`.
pub fn strip_pick(r: Rect, n: usize, x: f32) -> usize {
    let f = ((x - (r.x + 2) as f32) / (r.w - 4) as f32).clamp(0.0, 1.0);
    ((f * n as f32) as usize).min(n.saturating_sub(1))
}

/// The road's height at every segment boundary, metres.
pub fn heights(segments: &[Segment]) -> Vec<f64> {
    let mut h = 0.0;
    let mut out = Vec::with_capacity(segments.len() + 1);
    out.push(h);
    for s in segments {
        h += s.pitch() * SEG_LEN;
        out.push(h);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{H_MAX, H_MIN, INSET_MAX};

    #[test]
    fn every_control_is_thumb_sized_in_the_thumb_zone_and_none_overlap() {
        for h in [H_MIN, 360, H_MAX] {
            for inset in [0, INSET_MAX] {
                let l = Layout::with_inset(h, inset);
                let e = EditRects::of(&l);
                let mut all: Vec<Rect> = vec![e.prev, e.next, e.undo, e.redo, e.test, e.prove];
                all.extend(e.tools);
                for t in Tool::ALL {
                    all.extend(e.options(t).iter().map(|o| o.0));
                }
                for r in &all {
                    assert!(r.x >= 0 && r.x + r.w <= W, "{r:?}");
                    assert!(r.y + r.h <= l.bottom(), "{r:?} in the inset");
                    assert!(r.y >= l.bottom() - 112, "{r:?} out of the thumb zone");
                    assert!(r.w >= 28 && r.h >= 18, "{r:?} too small for a thumb");
                }
                // The fixed controls never overlap one another.
                let fixed: Vec<Rect> = [e.prev, e.next, e.undo, e.redo, e.test, e.prove]
                    .into_iter()
                    .chain(e.tools)
                    .chain(std::iter::once(e.opts))
                    .collect();
                for (i, a) in fixed.iter().enumerate() {
                    for b in &fixed[i + 1..] {
                        assert!(!a.overlaps(b), "{a:?} {b:?}");
                    }
                }
                assert!(e.map.h >= 150, "a usable map: {}", e.map.h);
                assert!(e.map.y + e.map.h < e.strip.y && e.strip.y + e.strip.h <= e.info.y);
            }
        }
    }

    #[test]
    fn a_tap_on_the_map_or_the_strip_picks_the_segment_under_it() {
        let course = racer_sim::layout::bundled::layouts().remove(0);
        let pts = plan(&course.segments);
        let l = Layout::new(360);
        let e = EditRects::of(&l);
        let fit = Fit::new(&pts, e.map, 4);
        for seg in [0, 57, 300, course.segments.len() - 1] {
            let a = fit.px(pts[seg]);
            let b = fit.px(pts[seg + 1]);
            let (x, y) = ((a.0 + b.0) as f32 / 2.0, (a.1 + b.1) as f32 / 2.0);
            let got = map_pick(&pts, &fit, x, y).unwrap();
            // Near enough: within the few segments one pixel holds.
            let per_px = (1.0 / (fit.scale() * SEG_LEN)).ceil() as usize + 1;
            assert!(got.abs_diff(seg) <= per_px, "{seg} picked {got}");
            // The whole road is inside the map.
            assert!(e.map.contains(a.0 as f32, a.1 as f32));
        }
        let n = course.segments.len();
        for seg in [0, 100, n - 1] {
            let x = strip_x(e.strip, seg, n) as f32 + 0.5;
            assert!(strip_pick(e.strip, n, x).abs_diff(seg) <= n / (e.strip.w as usize - 4) + 1);
        }
        assert_eq!(strip_pick(e.strip, n, -50.0), 0);
        assert_eq!(strip_pick(e.strip, n, 999.0), n - 1);
    }

    #[test]
    fn the_plan_turns_the_way_the_road_does() {
        use racer_sim::track::Segment;
        let right = vec![
            Segment {
                curve_milli: 50,
                ..Segment::default()
            };
            40
        ];
        let p = plan(&right);
        assert!(p.last().unwrap().0 > 0.0, "a right bend ends east");
        assert!(p.last().unwrap().1 < 0.0, "having gone north (up)");
        let h = heights(&[Segment {
            pitch_milli: 100,
            ..Segment::default()
        }]);
        assert_eq!(h, vec![0.0, 0.4]);
    }
}
