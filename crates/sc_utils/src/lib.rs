//! sc_utils: shared helpers and game struct definitions.
//!
//! Network-independent common types: game structs (position/server properties/MOTD),
//! client data (MinecraftClient/MinecraftClientData), world types, and schedule
//! labels (SC*Schedule, see schedule.rs).

pub mod app_label;
pub mod host_mode;
pub mod schedule;

pub mod color;
pub mod components;
pub mod event;
pub mod game;
pub mod material;
pub mod nbt;
pub mod tempdir;
pub mod world;

pub trait BoolByte {
    fn to_byte(&self) -> u8;
}

impl BoolByte for bool {
    fn to_byte(&self) -> u8 {
        if *self {
            1
        } else {
            0
        }
    }
}
