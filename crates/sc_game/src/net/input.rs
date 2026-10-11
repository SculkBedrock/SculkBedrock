//! Inbound player input values (game side, buffered for game commands).
//!
//! The game domain never depends on protocol types; the network plugin
//! decodes client packets into these values.

/// Player input value (game side), buffered for game commands.
#[derive(Clone, Debug)]
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
    pub tick: u64,
}
