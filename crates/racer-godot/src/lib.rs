//! Godot 4 presentation layer for racer. M1: a stub that proves the
//! extension builds and loads. DESIGN §6.
//!
//! Everything that decides anything is in `racer-sim`, which has no idea
//! this crate exists. In M2 this crate grows the pseudo-3D software
//! renderer, the portrait layout, touch zones and pad input (DESIGN §5); for
//! now it registers one node, [`SimInfo`], which reports the sim epoch the
//! build carries, so a scene can check it is talking to the sim it expects.

use godot::prelude::*;

pub mod font;
pub mod frame;
pub mod layout;
pub mod palette;
pub mod raster;

struct RacerExtension;

#[gdextension]
unsafe impl ExtensionLibrary for RacerExtension {}

/// Reports the simulation's version: `SIM_EPOCH` and the digest it vouches
/// for (DESIGN §8.3). Prints both when it enters the tree.
#[derive(GodotClass)]
#[class(base = Node)]
pub struct SimInfo {
    base: Base<Node>,
}

#[godot_api]
impl SimInfo {
    /// The sim's outcome version: a replay recorded under another epoch is
    /// not replayed.
    #[func]
    fn sim_epoch(&self) -> i64 {
        racer_sim::SIM_EPOCH as i64
    }

    /// The digest the epoch names, as hex text (Godot has no unsigned
    /// 64-bit integer).
    #[func]
    fn epoch_digest(&self) -> GString {
        format!("{:#018x}", racer_sim::EPOCH_DIGEST).as_str().into()
    }
}

#[godot_api]
impl INode for SimInfo {
    fn init(base: Base<Node>) -> Self {
        SimInfo { base }
    }

    fn ready(&mut self) {
        godot_print!(
            "racer sim epoch {} ({:#018x})",
            racer_sim::SIM_EPOCH,
            racer_sim::EPOCH_DIGEST
        );
    }
}
