//! Cosmetic effect intents (tolerate one miss, always overwritable state).

use sc_game_macros::GameIntent;
use sc_world::manager::MinecraftWorldId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundCue {
    BlockBreak,
    BlockPlace,
    ItemPickup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleCue {
    BlockBreak,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockBreakProgressCue {
    Start,
    Update,
    Stop,
}

/// Semantic sound cue; protocol ids stay in `sc_network`.
/// Extra data for level sound events.
#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct PlaySound {
    pub world_id: MinecraftWorldId,
    pub cue: SoundCue,
    pub data: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct PlayParticle {
    pub world_id: MinecraftWorldId,
    pub cue: ParticleCue,
    pub data: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Debug, GameIntent)]
#[intent(reliability = "state", entity = "none")]
pub struct BlockBreakProgress {
    pub world_id: MinecraftWorldId,
    pub cue: BlockBreakProgressCue,
    pub data: i32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
