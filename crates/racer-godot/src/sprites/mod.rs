//! Every sprite, drawn procedurally once at startup.
//!
//! DESIGN §5's rule is that nothing is scaled or rotated at draw time: a
//! sprite seen at some depth is drawn from the nearest of a few pre-drawn
//! sizes ([`SCALES`]), copied 1:1. Each size is drawn for itself, so a
//! small sprite is designed small rather than shrunk from a big one.
//!
//! - The cars ([`car`], [`traffic`]) are little solid models
//!   ([`solid`]) drawn once per pose and size: the player's car in every
//!   lean, drift and spin frame, brake lamps off and lit; traffic and the
//!   ghost at every step, from the angle the camera sees that depth at.
//! - Roadside things ([`roadside`]) are shapes in metres sampled at
//!   every size, lit and edged once a size can carry it.
//!
//! Every pixel is a palette index; the shapes only ever pick from
//! [`crate::palette`]. The cars use only the indices every biome shares.

pub mod car;
pub mod roadside;
pub mod solid;
pub mod traffic;

use racer_sim::biome::Biome;

pub use car::{CarArt, CarFrame, CarSprite, Fx, SPIN_FRAMES};

use crate::palette::*;
use crate::raster::Sprite;

/// The pre-drawn sizes, in pixels per metre at the sprite's depth. The
/// projection gives `focal / depth`; the nearest step (by ratio) is drawn.
pub const SCALES: [f64; 7] = [1.0, 1.6, 2.6, 4.2, 6.6, 10.0, 15.0];

/// One kind of sprite at every step in [`SCALES`].
#[derive(Clone, Debug)]
pub struct Steps {
    /// Size in metres, width and height.
    pub size_m: (f64, f64),
    /// One sprite per scale step.
    pub sprites: Vec<Sprite>,
}

impl Steps {
    /// Sample `shape` at every step for a thing `w_m` by `h_m` metres.
    pub fn new(w_m: f64, h_m: f64, shape: impl Fn(f64, f64, i32, i32) -> u8) -> Steps {
        let sprites = SCALES
            .iter()
            .map(|s| {
                let w = (w_m * s).round().max(1.0) as i32;
                let h = (h_m * s).round().max(1.0) as i32;
                sample(w, h, &shape)
            })
            .collect();
        Steps {
            size_m: (w_m, h_m),
            sprites,
        }
    }

    /// The step for `scale` pixels per metre: the nearest by ratio, or
    /// nothing when the thing would be under half the smallest step.
    pub fn pick(&self, scale: f64) -> Option<&Sprite> {
        if scale < SCALES[0] * 0.5 {
            return None;
        }
        let i = SCALES
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (scale / a.1)
                    .ln()
                    .abs()
                    .total_cmp(&(scale / b.1).ln().abs())
            })
            .map_or(0, |(i, _)| i);
        self.sprites.get(i)
    }
}

/// Sample a shape function at each pixel's centre: `shape(u, v, w, h)`
/// with `u, v` in `0..1` returns a palette index or [`Sprite::CLEAR`].
fn sample(w: i32, h: i32, shape: &impl Fn(f64, f64, i32, i32) -> u8) -> Sprite {
    let mut s = Sprite::blank(w, h);
    for y in 0..h {
        for x in 0..w {
            let u = (x as f64 + 0.5) / w as f64;
            let v = (y as f64 + 0.5) / h as f64;
            s.set(x, y, shape(u, v, w, h));
        }
    }
    s
}

/// The camera's angle down at something `scale` pixels a metre away,
/// radians: steep close to the camera, nearly level far off. Pre-drawn
/// cars at a scale step are drawn from this angle, capped at the player's
/// low chase angle ([`car::PLAYER_PITCH`]): every car shows mostly its
/// tail under a thin band of roof, never a top-down view.
pub fn pitch_at(scale: f64) -> f64 {
    let cam = crate::project::Camera::DEFAULT;
    let dz = cam.focal / scale.max(1e-3);
    (cam.height / dz).atan().min(car::PLAYER_PITCH)
}

/// Every sprite the game draws.
#[derive(Clone, Debug)]
pub struct Art {
    /// Marker posts.
    pub post: Steps,
    /// Boulders.
    pub rock: Steps,
    /// Roadside trees (decor): the meadow's flora.
    pub tree: Steps,
    /// COAST's palms.
    pub palm: Steps,
    /// DESERT DUSK's cacti.
    pub cactus: Steps,
    /// ALPINE's pines.
    pub pine: Steps,
    /// NIGHT CITY's street lights.
    pub lamp: Steps,
    /// ALPINE's marker posts.
    pub snow_post: Steps,
    /// NIGHT CITY's marker posts.
    pub city_post: Steps,
    /// Bend-warning boards; pointing right, mirrored for left.
    pub chevron: Steps,
    /// Traffic: every vehicle variant ([`traffic::VARIANTS`]).
    pub traffic: Vec<Steps>,
    /// The ghost of the player's best run: the player's car, pale and
    /// dithered, with a solid outline.
    pub ghost: Steps,
    /// The player's car, every pose.
    pub car: CarArt,
    /// Tyre smoke, the boost flame and the ghost's marker.
    pub fx: Fx,
}

impl Art {
    /// Draw everything. Called once.
    pub fn new() -> Art {
        Art {
            post: roadside::post(),
            rock: roadside::rock(),
            tree: roadside::tree(),
            palm: roadside::palm(),
            cactus: roadside::cactus(),
            pine: roadside::pine(),
            lamp: roadside::lamp(),
            snow_post: roadside::snow_post(),
            city_post: roadside::city_post(),
            chevron: roadside::chevron(),
            traffic: traffic::all(),
            ghost: Steps {
                size_m: (1.8, 1.2),
                sprites: SCALES.iter().map(|&s| car::ghost(s, pitch_at(s))).collect(),
            },
            car: CarArt::new(),
            fx: Fx::new(),
        }
    }
}

impl Art {
    /// What stands beside a road in biome `b`: the decor that was the
    /// meadow's trees.
    pub fn flora(&self, b: Biome) -> &Steps {
        match b {
            Biome::Meadow => &self.tree,
            Biome::Coast => &self.palm,
            Biome::Alpine => &self.pine,
            Biome::Desert => &self.cactus,
            Biome::Night => &self.lamp,
        }
    }

    /// The marker posts (the sim's `ObjectKind::Post`) in biome `b`. Every
    /// one is the same size: a post's footprint is the sim's, and only its
    /// paint changes.
    pub fn post_for(&self, b: Biome) -> &Steps {
        match b {
            Biome::Alpine => &self.snow_post,
            Biome::Night => &self.city_post,
            _ => &self.post,
        }
    }
}

impl Art {
    /// One set for the whole test run: drawing it is cheap in release
    /// but tens of milliseconds in a debug build.
    #[cfg(test)]
    pub fn shared() -> &'static Art {
        static ART: std::sync::OnceLock<Art> = std::sync::OnceLock::new();
        ART.get_or_init(Art::new)
    }
}

impl Default for Art {
    fn default() -> Self {
        Art::new()
    }
}

impl Art {
    /// Every sprite laid out on one sheet, for looking at: the player's
    /// car in every pose (lamps off, then lit), its effects, traffic and
    /// the ghost at every step on tarmac, then each biome's roadside
    /// things at every step on its own verge, in its own colours.
    pub fn sheet(&self) -> crate::raster::Canvas {
        type Row<'a> = (&'a crate::palette::Palette, u8, Vec<&'a Sprite>);
        let mut rows: Vec<Row> = Vec::new();
        let meadow = &crate::palette::COLOURS;
        rows.push((
            meadow,
            ASPHALT_LT,
            self.car.frames.iter().map(|f| &f.body).collect(),
        ));
        rows.push((
            meadow,
            ASPHALT_DK,
            self.car.frames.iter().map(|f| &f.braking).collect(),
        ));
        let fx: Vec<&Sprite> = self
            .fx
            .smoke
            .iter()
            .chain(self.fx.flame.iter())
            .chain([&self.fx.marker])
            .collect();
        rows.push((meadow, ASPHALT_DK, fx));
        for t in self.traffic_kinds() {
            rows.push((meadow, ASPHALT_LT, t.sprites.iter().collect()));
        }
        rows.push((meadow, ASPHALT_LT, self.ghost.sprites.iter().collect()));
        rows.push((meadow, ASPHALT_DK, self.ghost.sprites.iter().collect()));
        for b in Biome::ALL {
            let p = crate::palette::palette(b);
            for k in [self.flora(b), self.post_for(b), &self.rock, &self.chevron] {
                rows.push((p, GRASS_LT, k.sprites.iter().collect()));
            }
        }
        let gap = 3;
        let w = rows
            .iter()
            .map(|r| r.2.iter().map(|s| s.w + gap).sum::<i32>() + gap)
            .max()
            .unwrap_or(1);
        let h: i32 = rows
            .iter()
            .map(|r| r.2.iter().map(|s| s.h).max().unwrap_or(0) + 2 * gap)
            .sum();
        let mut c = crate::raster::Canvas::new(w, h);
        let mut y = 0;
        for (pal, bg, sprites) in rows {
            let rh = sprites.iter().map(|s| s.h).max().unwrap_or(0) + 2 * gap;
            c.set_palette(pal);
            c.rect(0, y, w, rh, bg);
            let mut x = gap;
            for s in sprites {
                c.blit(s, x, y + rh - gap - s.h, i32::MAX, false);
                x += s.w + gap;
            }
            y += rh;
        }
        c
    }

    /// Pixels held by every sprite, one byte each.
    pub fn bytes(&self) -> usize {
        let steps = [
            &self.post,
            &self.rock,
            &self.tree,
            &self.palm,
            &self.cactus,
            &self.pine,
            &self.lamp,
            &self.snow_post,
            &self.city_post,
            &self.chevron,
            &self.ghost,
        ];
        let px = |s: &Sprite| s.px.len();
        steps
            .iter()
            .chain(self.traffic_kinds().iter())
            .flat_map(|k| k.sprites.iter())
            .map(px)
            .sum::<usize>()
            + self
                .car
                .frames
                .iter()
                .map(|f| px(&f.body) + px(&f.braking))
                .sum::<usize>()
            + self
                .fx
                .smoke
                .iter()
                .chain(self.fx.flame.iter())
                .chain([&self.fx.marker])
                .map(px)
                .sum::<usize>()
    }

    /// Each kind of traffic, every livery.
    pub fn traffic_kinds(&self) -> Vec<&Steps> {
        self.traffic.iter().collect()
    }

    /// The vehicle traffic car `t` is drawn as: a variant picked by a hash
    /// of where it starts, its lane and its speed, so it keeps its looks
    /// all race and every replay. Presentation only.
    pub fn traffic_for(&self, t: &racer_sim::traffic::TrafficCar) -> &Steps {
        let h = racer_sim::rng::mix64(
            (t.z0_dm as u64) << 32 ^ (t.lane_cm as u16 as u64) << 16 ^ t.speed_dms as u64,
        );
        &self.traffic[(h % self.traffic.len() as u64) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_grow_and_pick_the_nearest() {
        let art = Art::new();
        let s = &art.traffic[0];
        assert_eq!(s.sprites.len(), SCALES.len());
        assert!(s.sprites.windows(2).all(|p| p[1].w > p[0].w));
        assert!(s.pick(0.3).is_none(), "too far to draw");
        assert_eq!(s.pick(1.0).unwrap().w, 2);
        let big = s.sprites.last().unwrap();
        assert_eq!(s.pick(15.0).unwrap(), big);
        assert_eq!(s.pick(100.0).unwrap(), big, "nearest is the largest");
        // Between 6.6 and 10 the ratio decides: 8 is nearer 6.6 (x1.21)
        // than 10 (x1.25).
        assert_eq!(s.pick(8.0).unwrap(), &s.sprites[4]);
    }

    #[test]
    fn sprites_use_only_palette_indices() {
        let art = Art::new();
        let all = [
            &art.post,
            &art.rock,
            &art.tree,
            &art.palm,
            &art.cactus,
            &art.pine,
            &art.lamp,
            &art.snow_post,
            &art.city_post,
            &art.chevron,
            &art.ghost,
        ];
        for k in all.into_iter().chain(art.traffic.iter()) {
            for s in &k.sprites {
                assert!(s.px.iter().all(|&c| c == Sprite::CLEAR || c < 16));
                assert!(s.px.iter().any(|&c| c != Sprite::CLEAR), "empty sprite");
            }
        }
    }

    #[test]
    fn every_biome_has_its_flora_and_posts_the_size_of_the_meadows() {
        let art = Art::new();
        for b in Biome::ALL {
            assert_eq!(art.post_for(b).size_m, art.post.size_m, "{b:?}");
            let f = art.flora(b);
            assert!(f.size_m.1 >= 5.0, "{b:?} stands tall");
            if b != Biome::Meadow {
                assert_ne!(f.sprites, art.tree.sprites, "{b:?}");
            }
        }
        // A street light's lamp hangs out over the road: lit pixels left
        // of its post.
        let big = art.lamp.sprites.last().unwrap();
        let lit: Vec<i32> = (0..big.w)
            .filter(|&x| (0..big.h).any(|y| big.at(x, y) == YELLOW))
            .collect();
        assert!(!lit.is_empty() && lit.iter().all(|&x| x < big.w / 2));
    }

    /// `cargo test -p racer-godot dump_sprite_sheet -- --ignored`: every
    /// sprite on one sheet, `target/frames/sprite_sheet.ppm`.
    #[test]
    #[ignore]
    fn dump_sprite_sheet() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/frames");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".gdignore"), "").unwrap();
        std::fs::write(dir.join("sprite_sheet.ppm"), Art::new().sheet().ppm()).unwrap();
    }

    /// The cost of drawing everything, once, at startup: `cargo test
    /// --release -p racer-godot art_build_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing, run by hand in release"]
    fn art_build_cost() {
        let n = 20;
        let start = std::time::Instant::now();
        let mut bytes = 0;
        for _ in 0..n {
            let a = Art::new();
            bytes = a.bytes();
        }
        let ms = start.elapsed().as_secs_f64() * 1e3 / n as f64;
        println!("art_build_cost: {ms:.2} ms per Art::new ({bytes} bytes of sprites)");
    }

    #[test]
    fn the_ghost_is_pale_see_through_and_outlined() {
        let art = Art::new();
        for s in &art.ghost.sprites {
            let mut holes = 0;
            for y in 0..s.h {
                for x in 0..s.w {
                    let c = s.at(x, y);
                    assert!(
                        c == Sprite::CLEAR || [ICE, WATER, WHITE, INK].contains(&c),
                        "{c}"
                    );
                    // Inside the outline, every other pixel is a hole.
                    let inside = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .all(|&(dx, dy)| s.at(x + dx, y + dy) != Sprite::CLEAR);
                    if c == Sprite::CLEAR && (1..s.w - 1).contains(&x) {
                        holes += usize::from(inside);
                    }
                }
            }
            assert!(holes > 0 || s.w < 8, "see-through: {}x{}", s.w, s.h);
        }
        let big = art.ghost.sprites.last().unwrap();
        assert!(big.px.contains(&ICE), "the body is ghost-blue");
        assert!(!big.px.contains(&RED), "never the player's red");
        // The far steps keep a solid pale outline, so a few pixels still
        // read as a car-shaped thing.
        let far = &art.ghost.sprites[2];
        let solid = far.px.iter().filter(|&&c| c == WHITE).count();
        assert!(solid >= far.w as usize, "{solid} outline pixels");
    }
}
