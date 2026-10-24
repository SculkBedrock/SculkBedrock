//! 玩家动作包（0x24）：客户端方块操作（破坏、放置、攀爬等）。
//!
//! Player action packet (0x24): client block actions (break/place/climb).
//!
//! Layout: `runtimeEntityId(varulong) + action(varint) + blockPos(3x zigzag
//! varint) + face(varint)`. Action values: see [`PlayerActionType`].

use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// 客户端动作类型常量。
///
/// Client action type constants.
#[allow(non_snake_case)]
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

/// 玩家动作包，携带一次方块交互的完整信息。
///
/// Player action packet carrying the full information of one block
/// interaction.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct PlayerAction {
    /// 发起动作的实体运行时 ID。
    ///
    /// The runtime entity id of the acting entity.
    pub runtime_entity_id: u64,
    /// 动作类型，取值见 [`PlayerActionType`]。
    ///
    /// The action type; see [`PlayerActionType`] for the values.
    pub action: i32,
    /// 目标方块的 X 坐标。
    ///
    /// The X coordinate of the target block.
    pub block_x: i32,
    /// 目标方块的 Y 坐标。
    ///
    /// The Y coordinate of the target block.
    pub block_y: i32,
    /// 目标方块的 Z 坐标。
    ///
    /// The Z coordinate of the target block.
    pub block_z: i32,
    /// 结果方块位置的 X 坐标。
    ///
    /// The X coordinate of the result block position.
    pub result_x: i32,
    /// 结果方块位置的 Y 坐标。
    ///
    /// The Y coordinate of the result block position.
    pub result_y: i32,
    /// 结果方块位置的 Z 坐标。
    ///
    /// The Z coordinate of the result block position.
    pub result_z: i32,
    /// 玩家朝向的方块面。
    ///
    /// The block face the player is facing.
    pub face: i32,
}

impl Reader<PlayerAction> for PlayerAction {
    /// 从字节流中解码一个玩家动作包。
    ///
    /// Decodes a player action packet from a byte stream.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或变长整数格式非法时返回 IO 错误。
    fn read(buf: &mut ByteReader) -> Result<PlayerAction, Error> {
        let runtime_entity_id = buf.read_var_u64()?;
        let action = buf.read_var_i32()?;
        let block_x = buf.read_var_i32()?;
        let block_y = buf.read_var_i32()?;
        let block_z = buf.read_var_i32()?;
        // Result block position: 3 zigzag varints.
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
    /// 将玩家动作包编码写入字节流。
    ///
    /// Encodes the player action packet into a byte stream.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入失败时返回 IO 错误。
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
