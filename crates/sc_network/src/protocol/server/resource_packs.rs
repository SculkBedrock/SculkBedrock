use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_network_macros::MinecraftPacket;
use sc_packloader::pack::ResourcePack;
use sc_utils::game::experiment::ExperimentData;
use std::io::Error;
use uuid::Uuid;

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackInfo {
    pub protocol_version: u32,
    pub must_accept: bool,
    pub scripting: bool,
    /// Whether to force-disable vibrant visuals.
    pub force_disable_vibrant_visuals: bool,
    pub world_template_id: Uuid,
    pub world_template_version: String,
    pub has_addon_packs: bool,
    pub force_server_packs: bool,
    pub behavior_packs: Vec<ResourcePack>,
    pub resource_packs: Vec<ResourcePack>,
}

fn encode_packs(
    buf: &mut ByteWriter,
    packs: &Vec<ResourcePack>,
    behavior: bool,
    protocol_version: u32,
) -> Result<(), Error> {
    // Pack count is var u32 from 2168 on (LE short before).
    if protocol_version >= 2168 {
        buf.write_var_u32(packs.len() as u32)?;
    } else {
        buf.write_i16_le(packs.len() as i16)?;
    }
    for pack in packs {
        let information = &pack.manifest.information;
        buf.write_uuid(&information.uuid)?;
        buf.write_string(&information.version.to_string())?;
        buf.write_u64_le(information.pack_len as u64)?;
        buf.write_string(&information.encryption_key)?;
        buf.write_string("")?; //TODO: sub-pack name
        if !information.encryption_key.is_empty() {
            buf.write_string(&information.uuid.to_string())?;
        } else {
            buf.write_string("")?;
        }
        buf.write_bool(false)?; //scripting
        if protocol_version >= 729 {
            // Addon-pack and raytracing flags (no behavior/resource split).
            buf.write_bool(false)?; //is addon pack
            buf.write_bool(false)?; //raytracing capable
            if protocol_version >= 748 {
                // CDN URL field.
                buf.write_string("")?; //TODO: cdnUrl
            }
        } else {
            if !behavior {
                buf.write_bool(false)?; //raytracing capable
            }
        }
    }
    Ok(())
}

impl Writer for ResourcePackInfo {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        log::debug!(
            "ResourcePackInfo write: protocol={}, behavior_packs={}, resource_packs={}",
            self.protocol_version,
            self.behavior_packs.len(),
            self.resource_packs.len()
        );
        for (i, pack) in self.behavior_packs.iter().enumerate() {
            log::debug!(
                "  behavior[{}]: uuid={}, version={}, pack_len={}, sha256_len={}",
                i,
                pack.manifest.information.uuid,
                pack.manifest.information.version,
                pack.manifest.information.pack_len,
                pack.manifest.information.sha256.len()
            );
        }
        for (i, pack) in self.resource_packs.iter().enumerate() {
            log::debug!(
                "  resource[{}]: uuid={}, version={}, pack_len={}, sha256_len={}",
                i,
                pack.manifest.information.uuid,
                pack.manifest.information.version,
                pack.manifest.information.pack_len,
                pack.manifest.information.sha256.len()
            );
        }
        buf.write_bool(self.must_accept)?;
        buf.write_bool(self.has_addon_packs)?;
        buf.write_bool(self.scripting)?;
        // forceDisableVibrantVisuals sits between scripting and
        // world_template_id; omitting it shifts all following fields.
        if self.protocol_version >= 818 {
            buf.write_bool(self.force_disable_vibrant_visuals)?;
        }
        if self.protocol_version >= 766 {
            // World template id and version.
            buf.write_uuid(&self.world_template_id)?;
            buf.write_string(&self.world_template_version)?;
        }
        if self.protocol_version < 729 {
            // Legacy layout still carries force_server_packs plus both stacks.
            buf.write_bool(self.force_server_packs)?;
            encode_packs(buf, &self.behavior_packs, true, self.protocol_version)?;
            encode_packs(buf, &self.resource_packs, false, self.protocol_version)?;
        } else {
            // Only the resource pack stack is sent.
            encode_packs(buf, &self.resource_packs, false, self.protocol_version)?;
        }
        if self.protocol_version < 748 {
            // Legacy CDN entries trailer.
            buf.write_var_u32(0)?; //cdn entries
        }
        Ok(())
    }
}

impl Reader<ResourcePackInfo> for ResourcePackInfo {
    fn read(_buf: &mut ByteReader) -> Result<ResourcePackInfo, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "resource pack decode is unsupported",
        ))
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackStack {
    pub protocol_version: u32,
    pub must_accept: bool,
    pub behavior_pack_stack: Vec<ResourcePack>,
    pub resource_pack_stack: Vec<ResourcePack>,
    pub experiments: Vec<ExperimentData>,
    pub game_version: String,
    pub is_has_editor_packs: bool,
}

impl Writer for ResourcePackStack {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_bool(self.must_accept)?;
        // The behavior pack stack was removed; only the resource stack follows.
        if self.protocol_version < 898 {
            buf.write_var_u32(self.behavior_pack_stack.len() as u32)?;
            for pack in &self.behavior_pack_stack {
                let information = &pack.manifest.information;
                buf.write_string(&information.uuid.to_string())?;
                buf.write_string(&information.version.to_string())?;
                buf.write_string("")?; //TODO: sub-pack name
            }
        }

        buf.write_var_u32(self.resource_pack_stack.len() as u32)?;
        for pack in &self.resource_pack_stack {
            let information = &pack.manifest.information;
            buf.write_string(&information.uuid.to_string())?;
            buf.write_string(&information.version.to_string())?;
            buf.write_string("")?; //TODO: sub-pack name
        }

        buf.write_string(self.game_version.as_str())?;
        buf.write_i32_le(self.experiments.len() as i32)?;
        for experiment in &self.experiments {
            buf.write_string(experiment.name)?;
            buf.write_bool(experiment.is_enabled)?;
        }
        buf.write_bool(!self.experiments.is_empty())?; // Were experiments previously toggled
        buf.write_bool(self.is_has_editor_packs)?;
        Ok(())
    }
}

impl Reader<ResourcePackStack> for ResourcePackStack {
    fn read(_buf: &mut ByteReader) -> Result<ResourcePackStack, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "resource pack decode is unsupported",
        ))
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackDataInfo {
    pub pack_id: Uuid,
    pub pack_version: String,
    pub max_chunk_size: u32,
    pub chunk_count: u32,
    pub compressed_pack_size: u64,
    pub sha256: Vec<u8>,
    pub is_premium: bool,
    pub pack_type: u8,
}

impl Writer for ResourcePackDataInfo {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        let pos_start = buf.as_slice().len();
        buf.write_string(&format!("{}_{}", self.pack_id, self.pack_version))?;
        let pos_after_uuid = buf.as_slice().len();
        buf.write_u32_le(self.max_chunk_size)?;
        buf.write_u32_le(self.chunk_count)?;
        buf.write_u64_le(self.compressed_pack_size)?;
        buf.write_slice(&self.sha256)?;
        buf.write_bool(self.is_premium)?;
        buf.write_u8(self.pack_type)?;
        let total = buf.as_slice().len() - pos_start;
        log::debug!("ResourcePackDataInfo write: pack_id={}, max_chunk_size={}, chunk_count={}, compressed_size={}, sha256_len={}, is_premium={}, pack_type={}, uuid_str_len={}, total_bytes={}",
            self.pack_id, self.max_chunk_size, self.chunk_count, self.compressed_pack_size, self.sha256.len(), self.is_premium, self.pack_type,
            pos_after_uuid - pos_start, total);
        Ok(())
    }
}

impl Reader<ResourcePackDataInfo> for ResourcePackDataInfo {
    fn read(_buf: &mut ByteReader) -> Result<ResourcePackDataInfo, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "resource pack decode is unsupported",
        ))
    }
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ResourcePackChunkData {
    pub pack_id: Uuid,
    pub pack_version: String,
    pub chunk_index: u32,
    pub progress: u64,
    pub data: Vec<u8>,
}

impl Writer for ResourcePackChunkData {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_string(&format!("{}_{}", self.pack_id, self.pack_version))?;
        buf.write_u32_le(self.chunk_index)?;
        buf.write_u64_le(self.progress)?;
        buf.write_slice(&self.data)?;
        Ok(())
    }
}

impl Reader<ResourcePackChunkData> for ResourcePackChunkData {
    fn read(_buf: &mut ByteReader) -> Result<ResourcePackChunkData, Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "resource pack decode is unsupported",
        ))
    }
}
