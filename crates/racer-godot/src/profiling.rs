//! Frame timing for device measurements (`--features profiling`; absent
//! from every normal build).
//!
//! Every two seconds of racing, one log line: the raster (painting the
//! canvas and laying out its buttons) mean and 95th percentile, the whole
//! `process` call, the frame interval the engine delivered and the fps it
//! implies, plus where the car is (track, segment, CREST/DRIFT flags), so a
//! screenshot can be timed from the log. On Android it lands in logcat
//! under the `godot` tag.

use std::time::Duration;

use racer_sim::track::flags;

use crate::app::{Game, Mode};
use crate::frame::Stats;

/// Seconds of racing per summary line.
pub const WINDOW: f64 = 2.0;

/// Accumulated timings.
#[derive(Debug, Default)]
pub struct Profile {
    raster: Stats,
    process: Stats,
    interval: Stats,
    clock: f64,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Profile {
    /// One raster pass took `d`.
    pub fn raster(&mut self, d: Duration) {
        self.raster.push(ms(d));
    }

    /// A frame ended: `delta` since the last, `process` spent in it. Returns
    /// a summary line every [`WINDOW`] seconds of unpaused racing.
    pub fn frame(&mut self, delta: f64, process: Duration, g: &Game) -> Option<String> {
        if g.mode != Mode::Racing || g.paused {
            self.raster.clear();
            self.process.clear();
            self.interval.clear();
            self.clock = 0.0;
            return None;
        }
        self.process.push(ms(process));
        self.interval.push(delta * 1000.0);
        self.clock += delta;
        if self.clock < WINDOW {
            return None;
        }
        let run = g.run.as_ref()?;
        let car = run.race().car();
        let t = run.race().track();
        let i = t.index_at(car.z);
        let seg = t.segments()[i];
        let mut tags = String::new();
        if seg.has(flags::CREST) {
            tags.push_str(" CREST");
        }
        if seg.has(flags::DRIFT_REQ) {
            tags.push_str(" DRIFT_REQ");
        }
        if seg.curve_milli.abs() >= 20 {
            tags.push_str(" CURVE");
        }
        let fps = 1000.0 / self.interval.mean().max(0.001);
        let line = format!(
            "racer-prof: raster mean {:.3} p95 {:.3} ms | process mean {:.3} p95 {:.3} ms | \
             frame mean {:.2} p95 {:.2} ms, {:.1} fps | n {} | track {} seg {} curve {} \
             elev {:.1}{}",
            self.raster.mean(),
            self.raster.percentile(95.0),
            self.process.mean(),
            self.process.percentile(95.0),
            self.interval.mean(),
            self.interval.percentile(95.0),
            fps,
            self.interval.len(),
            run.index() + 1,
            i,
            seg.curve_milli,
            t.elevation_at(car.z),
            tags
        );
        self.raster.clear();
        self.process.clear();
        self.interval.clear();
        self.clock = 0.0;
        Some(line)
    }
}
