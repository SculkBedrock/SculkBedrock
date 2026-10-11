use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use std::io::Error;

#[derive(Clone, Debug, MinecraftPacket)]
pub struct RequestChunkRadius {
    pub radius: i32,
    pub max_radius: u8,
}

impl Reader<RequestChunkRadius> for RequestChunkRadius {
    fn read(buf: &mut ByteReader) -> Result<RequestChunkRadius, Error> {
        Ok(Self {
            radius: buf.read_var_i32()?,
            max_radius: buf.read_u8()?,
        })
    }
}

impl Writer for RequestChunkRadius {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.radius)?;
        buf.write_u8(self.max_radius)
    }
}

/// Bedrock v712+ ServerboundLoadingScreen (0x138).
///
/// SC only needs the packet as a state-machine signal during the initial
/// loading flow. Keeping the payload raw avoids coupling the login pipeline to
/// protocol-library enum details while still preserving the packet for dumps
/// and future typed parsing.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ServerboundLoadingScreen {
    pub payload: Vec<u8>,
}

impl Reader<ServerboundLoadingScreen> for ServerboundLoadingScreen {
    fn read(buf: &mut ByteReader) -> Result<ServerboundLoadingScreen, Error> {
        Ok(Self {
            payload: buf.as_slice().to_vec(),
        })
    }
}

impl Writer for ServerboundLoadingScreen {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write(&self.payload)
    }
}

/// Cursor item drag state (uint8): Start = 0, Stop = 1.
// Wire-protocol namespace mirrors official docs.
#[allow(non_snake_case)]
pub mod CursorItemDragState {
    pub const START: u8 = 0;
    pub const STOP: u8 = 1;
}

/// ServerboundCursorItemDrag (0x166). Cursor item split drag signal.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ServerboundCursorItemDrag {
    pub state: u8,
}
impl Reader<ServerboundCursorItemDrag> for ServerboundCursorItemDrag {
    fn read(buf: &mut ByteReader) -> Result<ServerboundCursorItemDrag, Error> {
        Ok(Self {
            state: buf.read_u8()?,
        })
    }
}

impl Writer for ServerboundCursorItemDrag {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.state)
    }
}

/// ServerboundStonecutterSetRecipe (0x162). Recipe index selection.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ServerboundStonecutterSetRecipe {
    pub container_id: u8,
    pub recipe_index: i32,
}

impl Reader<ServerboundStonecutterSetRecipe> for ServerboundStonecutterSetRecipe {
    fn read(buf: &mut ByteReader) -> Result<ServerboundStonecutterSetRecipe, Error> {
        Ok(Self {
            container_id: buf.read_u8()?,
            recipe_index: buf.read_var_i32()?,
        })
    }
}

impl Writer for ServerboundStonecutterSetRecipe {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_u8(self.container_id)?;
        buf.write_var_i32(self.recipe_index)
    }
}

/// ServerboundMatchmakingCancel (0x164). Empty cancellation signal.
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ServerboundMatchmakingCancel;
impl Reader<ServerboundMatchmakingCancel> for ServerboundMatchmakingCancel {
    fn read(_buf: &mut ByteReader) -> Result<ServerboundMatchmakingCancel, Error> {
        Ok(Self)
    }
}

impl Writer for ServerboundMatchmakingCancel {
    fn write(&self, _buf: &mut ByteWriter) -> Result<(), Error> {
        Ok(())
    }
}

/// ClientCameraAimAssist (0x141). Client-side aim-assist activation signal.
///
/// Official layout (Endstone protocol-docs, BDS-extracted): camera preset id
/// string, action u8, allow-aim-assist bool. Observed live wire bytes
/// `[00, 01, 00]` decode exactly as empty preset + action 1 + disallow.
///
/// No handler system reads this packet: activation is client-side cosmetic
/// and the server has no aim-assist preset registry, so decoded packets are
/// deliberately ignored (previously they failed decode and warned per batch).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ClientCameraAimAssist {
    pub camera_preset_id: String,
    /// Raw action discriminant (official enum values unverified; kept raw).
    pub action: u8,
    pub allow_aim_assist: bool,
}

impl Reader<ClientCameraAimAssist> for ClientCameraAimAssist {
    fn read(buf: &mut ByteReader) -> Result<ClientCameraAimAssist, Error> {
        Ok(Self {
            camera_preset_id: buf.read_string()?,
            action: buf.read_u8()?,
            allow_aim_assist: buf.read_bool()?,
        })
    }
}

impl Writer for ClientCameraAimAssist {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_string(&self.camera_preset_id)?;
        buf.write_u8(self.action)?;
        buf.write_bool(self.allow_aim_assist)
    }
}

/// Audio content registration entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioContentRegistrationEntry {
    pub audio_content_id: String,
    pub shared_metadata_jwt: String,
    pub server_content_jwt: String,
    pub playback_content_jwt: String,
}

/// ServerboundRegisterAudioContent (0x168).
#[derive(Clone, Debug, MinecraftPacket)]
pub struct ServerboundRegisterAudioContent {
    pub registrations: Vec<AudioContentRegistrationEntry>,
}

impl Reader<ServerboundRegisterAudioContent> for ServerboundRegisterAudioContent {
    fn read(buf: &mut ByteReader) -> Result<ServerboundRegisterAudioContent, Error> {
        let count = buf.read_var_u32()? as usize;
        if count > 64 {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "too many audio content registrations",
            ));
        }
        let mut registrations = Vec::with_capacity(count);
        for _ in 0..count {
            registrations.push(AudioContentRegistrationEntry {
                audio_content_id: buf.read_string()?,
                shared_metadata_jwt: buf.read_string()?,
                server_content_jwt: buf.read_string()?,
                playback_content_jwt: buf.read_string()?,
            });
        }
        Ok(Self { registrations })
    }
}

impl Writer for ServerboundRegisterAudioContent {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_u32(self.registrations.len() as u32)?;
        for entry in &self.registrations {
            buf.write_string(&entry.audio_content_id)?;
            buf.write_string(&entry.shared_metadata_jwt)?;
            buf.write_string(&entry.server_content_jwt)?;
            buf.write_string(&entry.playback_content_jwt)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod protocol_2225_tests {
    use super::*;
    use sc_binary::interfaces::{Reader, Writer};

    #[test]
    fn cursor_drag_round_trip() {
        let packet = ServerboundCursorItemDrag {
            state: CursorItemDragState::START,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        assert_eq!(writer.as_slice(), &[0]);
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ServerboundCursorItemDrag::read(&mut reader).unwrap();
        assert_eq!(decoded.state, CursorItemDragState::START);
    }

    #[test]
    fn stonecutter_set_recipe_round_trip() {
        let packet = ServerboundStonecutterSetRecipe {
            container_id: 29,
            recipe_index: 2,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ServerboundStonecutterSetRecipe::read(&mut reader).unwrap();
        assert_eq!(decoded.container_id, 29);
        assert_eq!(decoded.recipe_index, 2);
    }

    #[test]
    fn client_camera_aim_assist_decodes_observed_wire_bytes() {
        // Live client bytes at login (empty preset + action 1 + disallow).
        let mut reader = ByteReader::from([0x00u8, 0x01, 0x00].as_slice());
        let decoded = ClientCameraAimAssist::read(&mut reader).unwrap();
        assert_eq!(decoded.camera_preset_id, "");
        assert_eq!(decoded.action, 1);
        assert!(!decoded.allow_aim_assist);
        assert!(reader.as_slice().is_empty(), "payload fully consumed");

        let packet = ClientCameraAimAssist {
            camera_preset_id: "test:preset".to_string(),
            action: 1,
            allow_aim_assist: true,
        };
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let mut reader = ByteReader::from(writer.as_slice());
        let decoded = ClientCameraAimAssist::read(&mut reader).unwrap();
        assert_eq!(decoded.camera_preset_id, "test:preset");
        assert_eq!(decoded.action, 1);
        assert!(decoded.allow_aim_assist);
    }
}
