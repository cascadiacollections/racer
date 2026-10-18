//! Godot 4 presentation layer for racer: the pseudo-3D renderer, the
//! portrait layout, touch, keys and pad, and the game's screens. No game
//! rules: everything that decides anything is in `racer-sim`, which has no
//! idea this crate exists (DESIGN §6).
//!
//! Built the way Salt Line is built. Follow one frame: [`game`]'s `process`
//! gathers input into [`input::Intents`], hands it to [`app::Game`], applies
//! the [`app::Effect`]s that come back, and if [`frame::Redraw`] says the
//! frame changed, [`screens`] (with [`render`]) paints the game into one
//! [`raster::Canvas`], which is uploaded as one texture.
//!
//! ```text
//! adapter    game           the one Godot node (Racer): engine events in, effects out
//! core       app            modes, the run, pause, DEMO (engine-free)
//!            input, touch   bindings and quantisation; on-screen buttons
//!            builder        tracks built on a worker thread
//! drawing    project        the pseudo-3D road projection, pinned in metres
//!            render         a race painted: road, sprites, car, HUD, pad
//!            screens        title, fork cards, results, pause; their buttons
//!            raster, font,  the canvas, the bitmap fonts, the 16 colours,
//!            palette,       procedural placeholder sprites at fixed sizes
//!            sprites
//! pacing     frame, layout  redraw flag, menu frame cap; the portrait layout
//! dev        profiling      opt-in (`--features profiling`) frame timing
//! ```
//!
//! Everything but `game` (and `lib`) is engine-free and runs under
//! `cargo test`.

use godot::prelude::*;

pub mod app;
pub mod builder;
pub mod font;
pub mod frame;
pub mod game;
pub mod input;
pub mod layout;
pub mod palette;
#[cfg(feature = "profiling")]
pub mod profiling;
pub mod project;
pub mod raster;
pub mod render;
pub mod screens;
pub mod sprites;
pub mod touch;

struct RacerExtension;

#[gdextension]
unsafe impl ExtensionLibrary for RacerExtension {}
