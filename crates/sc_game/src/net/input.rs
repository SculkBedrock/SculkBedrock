//! 入站玩家输入值（游戏侧，缓冲给游戏命令使用）。
//!
//! Inbound player input values (game side, buffered for game commands).
//!
//! 游戏域从不依赖协议类型；网络插件负责把客户端包解码为这些值。
//! The game domain never depends on protocol types; the network plugin
//! decodes client packets into these values.

/// 玩家输入值（游戏侧），缓冲给游戏命令使用。
///
/// Player input value (game side), buffered for game commands.
#[derive(Clone, Debug, Default)]
pub struct PlayerInput {
    pub feet_x: f32,
    pub feet_y: f32,
    pub feet_z: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub move_x: f32,
    pub move_z: f32,
    pub sneaking: bool,
    pub jumping: bool,
    pub sprinting: bool,
    pub flying: bool,
    pub start_swimming: bool,
    pub stop_swimming: bool,
    pub tick: u64,
}

