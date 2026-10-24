use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// MovePlayerPacket (0x13)，服务端 → 客户端：玩家位置同步。
///
/// MovePlayerPacket (0x13), server -> client player position sync.
///
/// 布局：eid、位置、pitch/yaw/head_yaw、mode（字节）、on_ground、
/// riding_eid、[mode==Teleport 时：teleport_cause/item]、tick。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct MovePlayer {
    /// 实体运行时 id。
    ///
    /// Entity runtime id.
    pub entity_id: u64,
    /// 目标位置 X 坐标。
    ///
    /// Target X coordinate.
    pub x: f32,
    /// 目标位置 Y 坐标。
    ///
    /// Target Y coordinate.
    pub y: f32,
    /// 目标位置 Z 坐标。
    ///
    /// Target Z coordinate.
    pub z: f32,
    /// 俯仰角。
    ///
    /// Pitch.
    pub pitch: f32,
    /// 偏航角。
    ///
    /// Yaw.
    pub yaw: f32,
    /// 头部偏航角。
    ///
    /// Head yaw.
    pub head_yaw: f32,
    /// 移动模式，见 [`MovePlayerMode`]。
    ///
    /// Movement mode, see [`MovePlayerMode`].
    pub mode: MovePlayerMode,
    /// 是否着地。
    ///
    /// Whether the entity is on the ground.
    pub on_ground: bool,
    /// 骑乘坐骑的实体 id（未骑乘时为 0）。
    ///
    /// Riding entity id (0 when not riding).
    pub riding_entity_id: u64,
    /// 客户端 tick（服务端可为 0）。
    ///
    /// Client tick (may be 0 server-side).
    pub tick: u64,
}

/// 移动模式枚举。
///
/// Movement mode enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i8)]
pub enum MovePlayerMode {
    /// 普通移动。
    ///
    /// Normal movement.
    Normal = 0,
    /// 重置位置。
    ///
    /// Reset position.
    Reset = 1,
    /// 传送。
    ///
    /// Teleport.
    Teleport = 2,
    /// 仅俯仰角。
    ///
    /// Pitch only.
    Pitch = 3,
    /// 仅旋转。
    ///
    /// Rotation only.
    Rotation = 4,
}

impl MovePlayerMode {
    /// 从线上字节序数还原移动模式；未知值回落为 [`MovePlayerMode::Normal`]。
    ///
    /// Restores the movement mode from the wire ordinal; unknown values
    /// fall back to [`MovePlayerMode::Normal`].
    ///
    /// - `ordinal`: 线上序数（i8）
    pub fn from(ordinal: i8) -> MovePlayerMode {
        match ordinal {
            1 => Self::Reset,
            2 => Self::Teleport,
            3 => Self::Pitch,
            4 => Self::Rotation,
            _ => Self::Normal,
        }
    }
}

impl MovePlayer {
    /// 构造用于玩家广播的 NORMAL 位置同步包。
    ///
    /// Builds a NORMAL position sync packet for player broadcast.
    ///
    /// - `entity_id`: 实体运行时 id
    /// - `x`/`y`/`z`: 目标位置
    /// - `yaw`/`pitch`/`head_yaw`: 朝向角
    /// - `on_ground`: 是否着地
    pub fn normal(
        entity_id: u64,
        x: f32,
        y: f32,
        z: f32,
        yaw: f32,
        pitch: f32,
        head_yaw: f32,
        on_ground: bool,
    ) -> Self {
        Self {
            entity_id,
            x,
            y,
            z,
            pitch,
            yaw,
            head_yaw,
            mode: MovePlayerMode::Normal,
            on_ground,
            riding_entity_id: 0,
            tick: 0,
        }
    }
}

impl Writer for MovePlayer {
    /// 写入 MovePlayer 包体（按协议版本处理 Teleport 可选块）。
    ///
    /// Writes the MovePlayer body (the Teleport optional block is handled
    /// per protocol version).
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let protocol = crate::protocol::version::current_protocol_version();
        buf.write_var_u64(self.entity_id)?;
        buf.write_f32_le(self.x)?;
        buf.write_f32_le(self.y)?;
        buf.write_f32_le(self.z)?;
        buf.write_f32_le(self.pitch)?;
        buf.write_f32_le(self.yaw)?;
        buf.write_f32_le(self.head_yaw)?;
        buf.write_i8(self.mode as i8)?;
        buf.write_bool(self.on_ground)?;
        buf.write_var_u64(self.riding_entity_id)?;
        let is_teleport = self.mode == MovePlayerMode::Teleport;
        // Teleport block is a writeOptional: bool(mode==TELEPORT) first,
        // then cause/entityType when set. Older protocols lack the bool.
        if protocol >= 2168 {
            buf.write_bool(is_teleport)?;
        }
        if is_teleport {
            buf.write_i32_le(0)?; // teleport_cause
            buf.write_i32_le(0)?; // entity_type
        }
        buf.write_var_u64(self.tick)
    }
}

impl Reader<MovePlayer> for MovePlayer {
    /// 解码 MovePlayer 包体（按协议版本处理 Teleport 可选块）。
    ///
    /// Decodes a MovePlayer body (the Teleport optional block is handled
    /// per protocol version).
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let entity_id = buf.read_var_u64()?;
        let x = buf.read_f32_le()?;
        let y = buf.read_f32_le()?;
        let z = buf.read_f32_le()?;
        let pitch = buf.read_f32_le()?;
        let yaw = buf.read_f32_le()?;
        let head_yaw = buf.read_f32_le()?;
        let mode = MovePlayerMode::from(buf.read_i8()?);
        let on_ground = buf.read_bool()?;
        let riding_entity_id = buf.read_var_u64()?;
        let protocol = crate::protocol::version::current_protocol_version();
        if protocol >= 2168 {
            if buf.read_bool()? {
                let _ = buf.read_i32_le()?;
                let _ = buf.read_i32_le()?;
            }
        } else if mode == MovePlayerMode::Teleport {
            let _ = buf.read_i32_le()?;
            let _ = buf.read_i32_le()?;
        }
        let tick = buf.read_var_u64()?;
        Ok(Self {
            entity_id,
            x,
            y,
            z,
            pitch,
            yaw,
            head_yaw,
            mode,
            on_ground,
            riding_entity_id,
            tick,
        })
    }
}

/// MoveEntityAbsolutePacket (0x12)，服务端 → 客户端：实体绝对位置同步。
///
/// MoveEntityAbsolutePacket (0x12), server -> client entity sync.
///
/// 布局：eid、flags（字节）、位置（Vec3f）、pitch/yaw/head_yaw
/// （各一字节，编码为 度数/(360/256)）。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct MoveEntityAbsolute {
    /// 实体运行时 id。
    ///
    /// Entity runtime id.
    pub entity_id: u64,
    /// 目标位置 X 坐标。
    ///
    /// Target X coordinate.
    pub x: f32,
    /// 目标位置 Y 坐标。
    ///
    /// Target Y coordinate.
    pub y: f32,
    /// 目标位置 Z 坐标。
    ///
    /// Target Z coordinate.
    pub z: f32,
    /// 俯仰角（度数）。
    ///
    /// Pitch in degrees.
    pub pitch: f32,
    /// 头部偏航角（度数）。
    ///
    /// Head yaw in degrees.
    pub head_yaw: f32,
    /// 偏航角（度数）。
    ///
    /// Yaw in degrees.
    pub yaw: f32,
    /// 是否着地（flags 位 0x01）。
    ///
    /// Whether the entity is on the ground (flags bit 0x01).
    pub on_ground: bool,
    /// 是否为传送（flags 位 0x02）。
    ///
    /// Whether this is a teleport (flags bit 0x02).
    pub teleport: bool,
    /// 是否强制移动本地实体（flags 位 0x04）。
    ///
    /// Whether to force-move the local entity (flags bit 0x04).
    pub force_move_local_entity: bool,
    /// 是否强制完成移动（flags 位 0x08）。
    ///
    /// Whether to force completion of the move (flags bit 0x08).
    pub force_completion: bool,
}

impl MoveEntityAbsolute {
    /// 度数转客户端旋转字节。
    ///
    /// Degrees to client rotation byte.
    #[inline]
    fn encode_rotation(v: f32) -> i8 {
        (v / (360.0 / 256.0)) as i8
    }
}

impl Writer for MoveEntityAbsolute {
    /// 写入 MoveEntityAbsolute 包体。
    ///
    /// Writes the MoveEntityAbsolute packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_id)?;
        let mut flags = 0u8;
        if self.on_ground {
            flags |= 0x01;
        }
        if self.teleport {
            flags |= 0x02;
        }
        if self.force_move_local_entity {
            flags |= 0x04;
        }
        if self.force_completion {
            flags |= 0x08;
        }
        buf.write_u8(flags)?;
        buf.write_f32_le(self.x)?;
        buf.write_f32_le(self.y)?;
        buf.write_f32_le(self.z)?;
        buf.write_i8(Self::encode_rotation(self.pitch))?;
        buf.write_i8(Self::encode_rotation(self.yaw))?;
        buf.write_i8(Self::encode_rotation(self.head_yaw))
    }
}

impl Reader<MoveEntityAbsolute> for MoveEntityAbsolute {
    /// 解码 MoveEntityAbsolute 包体。
    ///
    /// Decodes a MoveEntityAbsolute packet body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let entity_id = buf.read_var_u64()?;
        let flags = buf.read_u8()?;
        let x = buf.read_f32_le()?;
        let y = buf.read_f32_le()?;
        let z = buf.read_f32_le()?;
        // Rotation bytes back to degrees (decode mirrors encode).
        let pitch = buf.read_i8()? as f32 * (360.0 / 256.0);
        let yaw = buf.read_i8()? as f32 * (360.0 / 256.0);
        let head_yaw = buf.read_i8()? as f32 * (360.0 / 256.0);
        Ok(Self {
            entity_id,
            x,
            y,
            z,
            pitch,
            head_yaw,
            yaw,
            on_ground: flags & 0x01 != 0,
            teleport: flags & 0x02 != 0,
            force_move_local_entity: flags & 0x04 != 0,
            force_completion: flags & 0x08 != 0,
        })
    }
}

/// SetEntityMotionPacket (0x28)，服务端 → 客户端：实体速度。
///
/// SetEntityMotionPacket (0x28), server -> client entity velocity.
///
/// 布局：eid、速度（Vec3f）、tick。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetEntityMotion {
    /// 实体运行时 id。
    ///
    /// Entity runtime id.
    pub entity_id: u64,
    /// 速度 X 分量。
    ///
    /// Velocity X component.
    pub motion_x: f32,
    /// 速度 Y 分量。
    ///
    /// Velocity Y component.
    pub motion_y: f32,
    /// 速度 Z 分量。
    ///
    /// Velocity Z component.
    pub motion_z: f32,
    /// 客户端 tick。
    ///
    /// Client tick.
    pub tick: u64,
}

impl Writer for SetEntityMotion {
    /// 写入 SetEntityMotion 包体。
    ///
    /// Writes the SetEntityMotion packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_id)?;
        buf.write_f32_le(self.motion_x)?;
        buf.write_f32_le(self.motion_y)?;
        buf.write_f32_le(self.motion_z)?;
        buf.write_var_u64(self.tick)
    }
}

impl Reader<SetEntityMotion> for SetEntityMotion {
    /// 解码 SetEntityMotion 包体。
    ///
    /// Decodes a SetEntityMotion packet body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let entity_id = buf.read_var_u64()?;
        let motion_x = buf.read_f32_le()?;
        let motion_y = buf.read_f32_le()?;
        let motion_z = buf.read_f32_le()?;
        let tick = buf.read_var_u64()?;
        Ok(Self {
            entity_id,
            motion_x,
            motion_y,
            motion_z,
            tick,
        })
    }
}

/// RemoveEntityPacket (0x0e)，服务端 → 客户端：移除实体。
///
/// RemoveEntityPacket (0x0e), server -> client entity removal.
///
/// entityUniqueId 使用有符号 zigzag varlong 编码。
#[derive(Clone, Debug, MinecraftPacket)]
pub struct RemoveEntity {
    /// 要移除的实体运行时 id。
    ///
    /// Runtime id of the entity to remove.
    pub entity_id: u64,
}

impl Writer for RemoveEntity {
    /// 写入 RemoveEntity 包体。
    ///
    /// Writes the RemoveEntity packet body.
    ///
    /// - `buf`: 输出字节写入器
    ///
    /// # Errors
    /// 写入缓冲区失败时返回 IO 错误。
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i64(self.entity_id as i64)
    }
}

impl Reader<RemoveEntity> for RemoveEntity {
    /// 解码 RemoveEntity 包体。
    ///
    /// Decodes a RemoveEntity packet body.
    ///
    /// - `buf`: 输入字节读取器
    ///
    /// # Errors
    /// 缓冲区不足或数据损坏时返回解码错误。
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            entity_id: buf.read_var_i64()? as u64,
        })
    }
}
