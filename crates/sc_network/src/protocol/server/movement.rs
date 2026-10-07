use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

/// MovePlayerPacket (0x13), server -> client player position sync.
///
/// Layout: eid, pos, pitch/yaw/head_yaw, mode (byte), on_ground,
/// riding_eid, [mode==Teleport: teleport_cause/item], tick.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct MovePlayer {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    pub mode: MovePlayerMode,
    pub on_ground: bool,
    /// Riding entity id (0 when not riding).
    pub riding_entity_id: u64,
    /// Client tick (may be 0 server-side).
    pub tick: u64,
}

/// Movement modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i8)]
pub enum MovePlayerMode {
    Normal = 0,
    Reset = 1,
    Teleport = 2,
    Pitch = 3,
    Rotation = 4,
}

impl MovePlayerMode {
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
    /// Build a NORMAL position sync packet for player broadcast.
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

/// MoveEntityAbsolutePacket (0x12), server -> client entity sync.
///
/// Layout: eid, flags (byte), pos (Vec3f), pitch/yaw/head_yaw (one byte
/// each, encoded as degrees/(360/256)).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct MoveEntityAbsolute {
    pub entity_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub yaw: f32,
    pub on_ground: bool,
    pub teleport: bool,
    pub force_move_local_entity: bool,
    pub force_completion: bool,
}

impl MoveEntityAbsolute {
    /// Degrees to client rotation byte.
    #[inline]
    fn encode_rotation(v: f32) -> i8 {
        (v / (360.0 / 256.0)) as i8
    }
}

impl Writer for MoveEntityAbsolute {
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

/// SetEntityMotionPacket (0x28), server -> client entity velocity.
///
/// Layout: eid, motion (Vec3f), tick.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct SetEntityMotion {
    pub entity_id: u64,
    pub motion_x: f32,
    pub motion_y: f32,
    pub motion_z: f32,
    pub tick: u64,
}

impl Writer for SetEntityMotion {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u64(self.entity_id)?;
        buf.write_f32_le(self.motion_x)?;
        buf.write_f32_le(self.motion_y)?;
        buf.write_f32_le(self.motion_z)?;
        buf.write_var_u64(self.tick)
    }
}

impl Reader<SetEntityMotion> for SetEntityMotion {
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

/// RemoveEntityPacket (0x0e), server -> client entity removal.
///
/// entityUniqueId uses signed zigzag varlong.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct RemoveEntity {
    pub entity_id: u64,
}

impl Writer for RemoveEntity {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i64(self.entity_id as i64)
    }
}

impl Reader<RemoveEntity> for RemoveEntity {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            entity_id: buf.read_var_i64()? as u64,
        })
    }
}
