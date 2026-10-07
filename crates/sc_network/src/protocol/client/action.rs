//! PlayerActionPacket (0x24): client block actions (break/place/climb).
//!
//! Layout: `runtimeEntityId(varulong) + action(varint) + blockPos(3x zigzag
//! varint) + face(varint)`. Action values: see [`PlayerActionType`].

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// Client action types.
pub mod PlayerActionType {
    pub const START_BREAK: i32 = 0;
    pub const ABORT_BREAK: i32 = 1;
    pub const STOP_BREAK: i32 = 2;
    pub const GET_UPDATED_BLOCK: i32 = 3;
    pub const DROP_ITEM: i32 = 4;
    pub const START_SLEEPING: i32 = 5;
    pub const STOP_SLEEPING: i32 = 6;
    pub const RESPAWN: i32 = 7;
    pub const JUMP: i32 = 8;
    pub const START_SPRINT: i32 = 9;
    pub const STOP_SPRINT: i32 = 10;
    pub const START_SNEAK: i32 = 11;
    pub const STOP_SNEAK: i32 = 12;
    pub const CREATIVE_PLAYER_DESTROY_BLOCK: i32 = 13;
    pub const DIMENSION_CHANGE_ACK: i32 = 14;
    pub const START_GLIDE: i32 = 15;
    pub const STOP_GLIDE: i32 = 16;
    pub const BUILD_DENIED: i32 = 17;
    pub const CONTINUE_BREAK: i32 = 18;
    pub const CHANGE_SKIN: i32 = 19;
    pub const SET_ENCHANTMENT_SEED: i32 = 20;
    pub const START_SWIMMING: i32 = 21;
    pub const STOP_SWIMMING: i32 = 22;
    pub const START_SPIN_ATTACK: i32 = 23;
    pub const STOP_SPIN_ATTACK: i32 = 24;
    pub const INTERACT_BLOCK: i32 = 25;
    pub const PREDICT_DESTROY_BLOCK: i32 = 26;
    pub const CONTINUE_DESTROY_BLOCK: i32 = 27;
    pub const START_ITEM_USE_ON: i32 = 28;
    pub const STOP_ITEM_USE_ON: i32 = 29;
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerAction {
    pub runtime_entity_id: u64,
    pub action: i32,
    pub block_x: i32,
    pub block_y: i32,
    pub block_z: i32,
    pub result_x: i32,
    pub result_y: i32,
    pub result_z: i32,
    pub face: i32,
}

impl Reader<PlayerAction> for PlayerAction {
    fn read(buf: &mut ByteReader) -> Result<PlayerAction, Error> {
        let runtime_entity_id = buf.read_var_u64()?;
        let action = buf.read_var_i32()?;
        let block_x = buf.read_var_i32()?;
        let block_y = buf.read_var_i32()?;
        let block_z = buf.read_var_i32()?;
        // resultPos (3x zigzag varint).
        let result_x = buf.read_var_i32()?;
        let result_y = buf.read_var_i32()?;
        let result_z = buf.read_var_i32()?;
        let face = buf.read_var_i32()?;
        Ok(Self {
            runtime_entity_id,
            action,
            block_x,
            block_y,
            block_z,
            result_x,
            result_y,
            result_z,
            face,
        })
    }
}

impl Writer for PlayerAction {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.runtime_entity_id)?;
        buf.write_var_i32(self.action)?;
        buf.write_var_i32(self.block_x)?;
        buf.write_var_i32(self.block_y)?;
        buf.write_var_i32(self.block_z)?;
        buf.write_var_i32(self.result_x)?;
        buf.write_var_i32(self.result_y)?;
        buf.write_var_i32(self.result_z)?;
        buf.write_var_i32(self.face)
    }
}
