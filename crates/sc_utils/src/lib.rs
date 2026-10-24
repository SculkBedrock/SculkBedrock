//! sc_utils：共享辅助类型与游戏结构体定义。
//!
//! 与网络无关的公共类型：游戏结构体（坐标/服务端属性/MOTD）、客户端数据
//! （MinecraftClient/MinecraftClientData）、世界类型与调度标签
//! （SC*Schedule，见 schedule.rs）。
//!
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
