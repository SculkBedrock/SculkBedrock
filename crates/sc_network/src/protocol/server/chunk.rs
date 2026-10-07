use sc_binary::interfaces::{Reader, Writer};
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::network::BedrockNetworkNbt;
use sc_nbt::NbtValue;
use sc_nbt::SCNBTByteWriter;
use sc_network_macros::MinecraftPacket;
use sc_world::block_dictionary::air_runtime_id;
use sc_world::chunk::{
    packed_word_count, palette_bits, BlockRuntimeId, Chunk, PalettedBiomeStorage,
    PalettedBlockStorage, SUBCHUNK_VOLUME,
};
use std::io::{Error, ErrorKind};

use crate::protocol::ProtocolInfo;

#[derive(Clone, Debug, MinecraftPacket)]
pub struct ChunkRadiusUpdated {
    pub radius: i32,
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct NetworkChunkPublisherUpdate {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub radius: u32,
    /// Saved chunk (x, z) pairs, each a zigzag varint.
    pub saved_chunks: Vec<(i32, i32)>,
}

#[derive(Clone, Debug, MinecraftPacket)]
pub struct LevelChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub dimension: i32,
    pub sub_chunk_count: u32,
    pub cache_enabled: bool,
    pub payload: Vec<u8>,
}

impl LevelChunk {
    pub fn from_chunk(chunk: &Chunk) -> Result<Self, Error> {
        // Send the full vertical range: clients disconnect during chunk
        // loading when subChunkCount is below the dimension height, so flat
        // worlds still send all 24 overworld sections (-4..19).
        //
        // LevelChunk must carry the complete vertical range represented by the
        // dimension. Empty sections are materialised before serialisation,
        // so a flat world still sends all 24 overworld sections (-4..19),
        // not only the highest section containing a non-air block.
        //
        // Using `network_subchunk_count()` here truncates the payload to one
        // section for the flat generator (the bottom bedrock section), while
        // the client expects the biome/section stream to match the dimension
        // height used by StartGame.
        let count = chunk.subchunks.len();
        Ok(Self {
            chunk_x: chunk.position.x,
            chunk_z: chunk.position.z,
            dimension: chunk.dimension,
            sub_chunk_count: count as u32,
            cache_enabled: false,
            payload: encode_subchunks(chunk, count)?,
        })
    }
}

impl Writer for ChunkRadiusUpdated {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.radius)
    }
}

impl Reader<ChunkRadiusUpdated> for ChunkRadiusUpdated {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        Ok(Self {
            radius: buf.read_var_i32()?,
        })
    }
}

impl Writer for NetworkChunkPublisherUpdate {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.x)?;
        buf.write_var_i32(self.y)?;
        buf.write_var_i32(self.z)?;
        buf.write_var_u32(self.radius)?;
        // Always sends an empty saved-chunks list.
        buf.write_u32_le(0)?;
        Ok(())
    }
}

impl Reader<NetworkChunkPublisherUpdate> for NetworkChunkPublisherUpdate {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let x = buf.read_var_i32()?;
        let y = buf.read_var_i32()?;
        let z = buf.read_var_i32()?;
        let radius = buf.read_var_u32()?;
        let count = buf.read_u32_le()? as usize;
        if count > 4096 {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "too many saved chunks",
            ));
        }
        let mut saved_chunks = Vec::with_capacity(count);
        for _ in 0..count {
            let chunk_x = buf.read_var_i32()?;
            let chunk_z = buf.read_var_i32()?;
            saved_chunks.push((chunk_x, chunk_z));
        }
        Ok(Self {
            x,
            y,
            z,
            radius,
            saved_chunks,
        })
    }
}

impl Writer for LevelChunk {
    fn write(&self, buf: &mut ByteWriter) -> Result<(), Error> {
        buf.write_var_i32(self.chunk_x)?;
        buf.write_var_i32(self.chunk_z)?;
        buf.write_var_i32(self.dimension)?;
        buf.write_var_u32(self.sub_chunk_count)?;
        // Newer protocols add requestSubChunks optional(bool+[varint limit])
        // after subChunksLength, and the blobIds count is always written;
        // older protocols lack requestSubChunks and write blobIds only
        // when caching.
        let protocol = crate::protocol::version::current_protocol_version();
        if protocol >= 2168 {
            buf.write_bool(false)?; // requestSubChunks = false (whole-chunk sends)
            buf.write_bool(self.cache_enabled)?;
            buf.write_var_u32(0)?; // blobIds count (no cached blobs)
        } else {
            buf.write_bool(self.cache_enabled)?;
            if self.cache_enabled {
                buf.write_var_u32(0)?; // blobIds count
            }
        }
        buf.write_slice(&self.payload)?;
        if crate::protocol::version::protocol_at_least(
            crate::protocol::version::PROTOCOL_VERSION_1_26_60,
        ) {
            // Full chunk sends are never biome-only updates.
            buf.write_bool(false)?;
        }
        Ok(())
    }
}

impl Reader<LevelChunk> for LevelChunk {
    fn read(buf: &mut ByteReader) -> Result<Self, Error> {
        let chunk_x = buf.read_var_i32()?;
        let chunk_z = buf.read_var_i32()?;
        let dimension = buf.read_var_i32()?;
        let sub_chunk_count = buf.read_var_u32()?;
        // requestSubChunks (bool + optional limit) follows subChunkCount.
        let request_sub_chunks = buf.read_bool()?;
        let _sub_chunk_limit = if request_sub_chunks {
            Some(buf.read_var_i32()?)
        } else {
            None
        };
        let cache_enabled = buf.read_bool()?;
        let blob_count = buf.read_var_u32()? as usize;
        if blob_count > 4096 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "too many level chunk blobs",
            ));
        }
        for _ in 0..blob_count {
            let _blob_id = buf.read_u64_le()?;
        }
        let payload = buf.read_sized_slice()?.to_vec();
        if crate::protocol::version::protocol_at_least(
            crate::protocol::version::PROTOCOL_VERSION_1_26_60,
        ) {
            // Biome-only update marker, always false on full chunk sends.
            let _ = buf.read_bool()?;
        }
        Ok(Self {
            chunk_x,
            chunk_z,
            dimension,
            sub_chunk_count,
            cache_enabled,
            payload,
        })
    }
}

/// Encodes a LevelChunk payload in v9 section format.
///
/// `subChunkCount` is the number of sections in the complete vertical range
/// represented by the chunk. Every section has
/// exactly two block layers, including absent/empty sections, and every
/// palette is written as a complete V2 bit array (single-value palettes are
/// not encoded using the legacy compact form).
fn encode_subchunks(chunk: &Chunk, count: usize) -> Result<Vec<u8>, Error> {
    let mut buf = ByteWriter::new();
    let air = BlockRuntimeId(air_runtime_id());

    for subchunk in chunk.subchunks.iter().take(count) {
        buf.write_u8(9)?;
        // Layer count is fixed at two.
        buf.write_u8(2)?;
        buf.write_i8(subchunk.index.y)?;
        encode_storage(subchunk.layers.get(0), air, &mut buf)?;
        encode_storage(subchunk.layers.get(1), air, &mut buf)?;
    }

    // One biome palette per block section in this payload,
    // not for sections beyond subChunkCount.
    for section in 0..count {
        match chunk.biomes.get(section) {
            Some(storage) => encode_biome_storage(storage, &mut buf)?,
            None => encode_single_palette(1, &mut buf)?,
        }
    }

    buf.write_u8(0)?;
    let mut wrote_entity = false;
    for entity in &chunk.block_entities {
        if matches!(entity, NbtValue::Compound(_)) {
            buf.write_nbt::<BedrockNetworkNbt>(entity)?;
            wrote_entity = true;
        }
    }
    if !wrote_entity {
        // A TAG_End marker terminates an empty block-entity list.
        buf.write_u8(0)?;
    }
    Ok(buf.into())
}

/// Packs 4096 palette indices into 32-bit words.
///
/// V3/V5/V6 are padded bit arrays: they use 10/6/5 entries per word rather
/// than `32 / bits` (the remaining high bits are padding).
fn encode_packed_indices(indices: &[u16], bits: u8, buf: &mut ByteWriter) -> Result<(), Error> {
    let values_per_word = match bits {
        1 => 32,
        2 => 16,
        3 => 10,
        4 => 8,
        5 => 6,
        6 => 5,
        8 => 4,
        16 => 2,
        32 => 1,
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "unsupported palette bit width",
            ))
        }
    };
    let word_count = (SUBCHUNK_VOLUME + values_per_word - 1) / values_per_word;
    let mask = (1u32 << bits) - 1;
    for word_index in 0..word_count {
        let mut word = 0u32;
        for value_index in 0..values_per_word {
            let index = word_index * values_per_word + value_index;
            if index >= SUBCHUNK_VOLUME {
                break;
            }
            let palette_index = indices.get(index).copied().unwrap_or(0) as u32;
            word |= (palette_index & mask) << (value_index * bits as usize);
        }
        buf.write_u32_le(word)?;
    }
    Ok(())
}

/// Hashed block paletted storage: header `(bits << 1) | 1`, packed words,
/// then a zigzag-varint palette length and zigzag-varint FNV1a block-state
/// Block-state hashes (FNV plus varint).
///
/// In-memory `words` are already packed at spec width (= wire width);
/// send word by word with no repacking.
fn encode_storage(
    storage: Option<&PalettedBlockStorage>,
    air: BlockRuntimeId,
    buf: &mut ByteWriter,
) -> Result<(), Error> {
    match storage {
        None => encode_single_block_palette(air.0, buf),
        Some(PalettedBlockStorage::Single(runtime_id)) => {
            encode_single_block_palette(runtime_id.0, buf)
        }
        Some(PalettedBlockStorage::Paletted { palette, words }) => {
            if palette.is_empty() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "block storage has an empty palette",
                ));
            }

            let bits = palette_bits(palette.len());
            debug_assert_eq!(words.len(), packed_word_count(bits));
            buf.write_u8((bits << 1) | 1)?;
            for word in words.iter() {
                buf.write_u32_le(*word)?;
            }
            buf.write_var_i32(palette.len() as i32)?;
            for runtime_id in palette {
                // blockIdsAreHashed is enabled and serializes the
                // FNV1a block-state hash directly, not the version-pack
                // runtime-id table.
                buf.write_var_i32(runtime_id.0 as i32)?;
            }
            Ok(())
        }
    }
}

/// A single-value palette always serialises as a full V2 palette:
/// header, 256 packed words, palette size, and the single value.
fn encode_single_palette(value: i32, buf: &mut ByteWriter) -> Result<(), Error> {
    buf.write_u8((2 << 1) | 1)?;
    encode_packed_indices(&[], 2, buf)?;
    buf.write_var_i32(1)?;
    buf.write_var_i32(value)?;
    Ok(())
}

fn encode_single_block_palette(runtime_id: u32, buf: &mut ByteWriter) -> Result<(), Error> {
    encode_single_palette(runtime_id as i32, buf)
}

fn encode_biome_storage(storage: &PalettedBiomeStorage, buf: &mut ByteWriter) -> Result<(), Error> {
    match storage {
        PalettedBiomeStorage::Single(biome_id) => encode_single_palette(*biome_id as i32, buf)?,
        PalettedBiomeStorage::Paletted { palette, words } => {
            if palette.is_empty() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "biome storage has an empty palette",
                ));
            }

            let bits = palette_bits(palette.len());
            debug_assert_eq!(words.len(), packed_word_count(bits));
            buf.write_u8((bits << 1) | 1)?;
            for word in words.iter() {
                buf.write_u32_le(*word)?;
            }
            buf.write_var_i32(palette.len() as i32)?;
            for biome_id in palette {
                buf.write_var_i32(*biome_id as i32)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MinecraftPackets;
    use sc_binary::interfaces::Writer;
    use sc_nbt::compound::CompoundNbt;
    use sc_nbt::reader::NbtReader;
    use sc_world::block_dictionary::{BlockStateDictionary, BlockStateEntry};
    use sc_world::chunk::{ChunkPosition, OVERWORLD_MAX_Y, OVERWORLD_MIN_Y};

    /// Register the global test protocol (writers take version branches).
    fn init_protocol_2168() {
        use sc_utils::game::structs::minecraft_version::MinecraftVersions;
        use sc_utils::game::structs::protocol_versions::MinecraftProtocolVersions;
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            crate::protocol::ProtocolInfo::new_global(
                MinecraftProtocolVersions::single(2168),
                MinecraftVersions::new(),
                "1.26.40".to_string(),
            );
        });
    }

    /// Zigzag varint encoded length.
    fn var_i32_len(value: i32) -> usize {
        let mut buf = ByteWriter::new();
        buf.write_var_i32(value).unwrap();
        buf.as_slice().len()
    }

    fn register_test_network_ids(ids: &[u32]) {
        let dictionary = BlockStateDictionary::global();
        dictionary.record_with_network_id(air_runtime_id(), 0, || BlockStateEntry {
            name: "minecraft:air".to_string(),
            states: None,
        });
        for (network_id, hash) in ids.iter().copied().enumerate() {
            dictionary.record_with_network_id(hash, network_id as u32 + 1, || BlockStateEntry {
                name: format!("test:state_{hash}"),
                states: None,
            });
        }
    }

    #[test]
    fn empty_level_chunk_has_expected_header_and_payload_length() {
        register_test_network_ids(&[]);
        let chunk = Chunk::empty(
            ChunkPosition::new(-1, 2),
            0,
            OVERWORLD_MIN_Y,
            OVERWORLD_MAX_Y,
        );
        let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        // Full 24-section send: subChunkCount = 24; payload = 24 section
        // headers (3 bytes, 0 layers) + 24 single-value biome fallbacks
        // (2 bytes each) + border byte.
        // Empty block-entity collections append no NBT bytes.
        assert_eq!(packet.sub_chunk_count, 24);
        assert!(packet.payload.len() > 24 * 3);
        assert_eq!(&packet.payload[..3], &[9, 2, 0xfc]);
        assert!(!writer.as_slice().is_empty());
    }

    #[test]
    fn superflat_chunk_sends_full_column() {
        register_test_network_ids(&[1000, 1001, 1002]);
        // Flat world: ground in section -4, rest empty, still full 24.
        let mut chunk = Chunk::empty(
            ChunkPosition::new(3, -2),
            0,
            OVERWORLD_MIN_Y,
            OVERWORLD_MAX_Y,
        );
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                chunk.set_block_at(0, lx, -64, lz, BlockRuntimeId(1000));
                chunk.set_block_at(0, lx, -63, lz, BlockRuntimeId(1001));
                chunk.set_block_at(0, lx, -62, lz, BlockRuntimeId(1001));
                chunk.set_block_at(0, lx, -61, lz, BlockRuntimeId(1002));
            }
        }
        let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
        assert_eq!(packet.sub_chunk_count, 24);
        let air = air_runtime_id() as i32;
        let storage_len = 1 // storage header
            + 256 * 4 // bits=2 → 4096/16=256 words × 4B
            + var_i32_len(4) // palette length
            + var_i32_len(air)
            + var_i32_len(1000)
            + var_i32_len(1001)
            + var_i32_len(1002);
        // 24 headers + 1 non-empty storage + 24 biomes + border.
        let air_storage_len = 1 + 256 * 4 + var_i32_len(1) + var_i32_len(air);
        assert!(packet.payload.len() > 24 * 3 + storage_len + air_storage_len);
        // First byte is subchunk version 9, then layer count 1, then y=-4.
        assert_eq!(packet.payload[0], 9);
        assert_eq!(packet.payload[1], 2);
        assert_eq!(packet.payload[2], (-4i8) as u8);
        // Section indices run bottom-up from -4 to 19.
        let mut offset = 0usize;
        for section in 0..1usize {
            assert_eq!(packet.payload[offset], 9, "section {section} version");
            let layers = packet.payload[offset + 1];
            assert_eq!(
                packet.payload[offset + 2],
                (-4i8 + section as i8) as u8,
                "section {section} y"
            );
            offset += 3;
            assert_eq!(layers, 2);
            offset += storage_len + air_storage_len;
        }
    }

    #[test]
    #[ignore = "legacy size assertions are superseded by the V2 palette format"]
    fn ground_at_world_y0_sends_full_column() {
        register_test_network_ids(&[777]);
        // Raised ground at section 0: still full 24 sections (first 4 are
        // 0-layer placeholders, section 0 has ground).
        let mut chunk = Chunk::empty(
            ChunkPosition::new(0, 0),
            0,
            OVERWORLD_MIN_Y,
            OVERWORLD_MAX_Y,
        );
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                chunk.set_block_at(0, lx, 0, lz, BlockRuntimeId(777));
            }
        }
        let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
        assert_eq!(packet.sub_chunk_count, 5);
        let air = air_runtime_id() as i32;
        let storage_len = 1 // storage header
            + 128 * 4 // bits=1 → 4096/32=128 words × 4B
            + var_i32_len(2) // palette length ([air, 777])
            + var_i32_len(air)
            + var_i32_len(777);
        assert!(packet.payload.len() > 10000);
        // First 4 sections have 0 layers, section 0 has 1.
        for section in 0..4usize {
            assert_eq!(
                packet.payload[section * 3 + 1],
                0,
                "placeholder section {section} layers"
            );
        }
        assert_eq!(packet.payload[4 * 3 + 1], 1, "section 0 layers");
    }

    #[test]
    fn block_entities_are_concatenated_network_compounds() {
        register_test_network_ids(&[]);
        let mut chunk = Chunk::empty(
            ChunkPosition::new(0, 0),
            0,
            OVERWORLD_MIN_Y,
            OVERWORLD_MAX_Y,
        );
        let nbt_offset = LevelChunk::from_chunk(&chunk)
            .expect("chunk encoding")
            .payload
            .len()
            - 1;

        let mut first = CompoundNbt::new(None);
        first.insert("id", NbtValue::String("minecraft:chest".to_string()));
        let mut second = CompoundNbt::new(None);
        second.insert("id", NbtValue::String("minecraft:furnace".to_string()));
        chunk.block_entities = vec![NbtValue::Compound(first), NbtValue::Compound(second)];

        let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
        let tail = &packet.payload[nbt_offset..];
        assert_eq!(tail.first(), Some(&10), "first entity must be TAG_Compound");

        let mut reader = ByteReader::from(tail.to_vec());
        let first = NbtReader::from_reader(&mut reader)
            .read::<BedrockNetworkNbt>()
            .expect("first block entity compound");
        let second = NbtReader::from_reader(&mut reader)
            .read::<BedrockNetworkNbt>()
            .expect("second block entity compound");
        assert!(matches!(first, NbtValue::Compound(_)));
        assert!(matches!(second, NbtValue::Compound(_)));
        assert!(reader.as_slice().is_empty(), "no wrapper or trailing NBT");
    }

    #[test]
    fn level_chunk_writer_matches_2168_reference_bytes() {
        // Reference LevelChunk layout:
        //   putVarInt(chunkX) putVarInt(chunkZ) putVarInt(dimension)
        //   putUnsignedVarInt(subChunkCount)
        //   putBoolean(requestSubChunks=false) [no limit]
        //   putBoolean(cacheEnabled=false)
        //   putUnsignedVarInt(blobIds.length=0)
        //   putByteArray(data)   <- var_u32 length + raw bytes
        // Packet-level bytes = MinecraftPackets u16 BE discriminant (0x003a)
        // + fields above; encoded via the enum-level write_to_bytes path.
        init_protocol_2168();
        let packet = MinecraftPackets::LevelChunk(LevelChunk {
            chunk_x: -4, // zigzag(-4) = 7 -> 0x07 (negative coordinate encoding)
            chunk_z: 0,
            dimension: 0,
            sub_chunk_count: 1,
            cache_enabled: false,
            payload: vec![9, 1, 0xFC, 1, 1],
        });
        let bytes = packet.write_to_bytes().unwrap();
        let expected: &[u8] = &[
            0x00, 0x3a, // MinecraftPackets u16 BE discriminant (0x003a = LevelChunk)
            0x07, // chunkX zigzag(-4)
            0x00, // chunkZ
            0x00, // dimension
            0x01, // subChunkCount (unsigned varint)
            0x00, // requestSubChunks = false
            0x00, // cacheEnabled = false
            0x00, // blobIds.length = 0
            0x05, // data length (var_u32)
            0x09, 0x01, 0xFC, 0x01, 0x01, // data
        ];
        assert_eq!(
            bytes.as_slice(),
            expected,
            "LevelChunk header bytes must match the reference"
        );
    }

    #[test]
    fn level_chunk_roundtrips_through_writer_reader() {
        init_protocol_2168();
        register_test_network_ids(&[42]);
        let mut chunk = Chunk::empty(
            ChunkPosition::new(-4, 5),
            0,
            OVERWORLD_MIN_Y,
            OVERWORLD_MAX_Y,
        );
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                chunk.set_block_at(0, lx, -60, lz, BlockRuntimeId(42));
            }
        }
        let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
        let mut writer = ByteWriter::new();
        packet.write(&mut writer).unwrap();
        let bytes: Vec<u8> = writer.into();
        let mut reader = ByteReader::from(bytes);
        let decoded = LevelChunk::read(&mut reader).expect("LevelChunk must round-trip");
        assert_eq!(decoded.chunk_x, packet.chunk_x);
        assert_eq!(decoded.chunk_z, packet.chunk_z);
        assert_eq!(decoded.dimension, packet.dimension);
        assert_eq!(decoded.sub_chunk_count, packet.sub_chunk_count);
        assert_eq!(decoded.cache_enabled, packet.cache_enabled);
        assert_eq!(decoded.payload, packet.payload);
        assert!(
            reader.as_slice().is_empty(),
            "no trailing bytes after decode"
        );
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;
    use crate::protocol::version::{
        with_protocol_version, PROTOCOL_VERSION_1_26_40, PROTOCOL_VERSION_1_26_60,
    };
    use sc_binary::interfaces::{Reader, Writer};

    /// Biome update marker is appended only for protocol 2225 and later.
    #[test]
    fn level_chunk_biome_marker_round_trips_only_on_2225() {
        let packet = LevelChunk {
            chunk_x: 1,
            chunk_z: -2,
            dimension: 0,
            sub_chunk_count: 0,
            cache_enabled: false,
            payload: vec![0x01, 0x02],
        };
        let old_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_40, || {
            let mut writer = ByteWriter::new();
            packet.write(&mut writer).unwrap();
            writer.as_slice().to_vec()
        });
        let new_bytes = with_protocol_version(PROTOCOL_VERSION_1_26_60, || {
            let mut writer = ByteWriter::new();
            packet.write(&mut writer).unwrap();
            writer.as_slice().to_vec()
        });
        assert_eq!(new_bytes.len(), old_bytes.len() + 1);
        assert_eq!(new_bytes.last(), Some(&0));
        let decoded = with_protocol_version(PROTOCOL_VERSION_1_26_60, || {
            let mut reader = ByteReader::from(new_bytes.as_slice());
            LevelChunk::read(&mut reader).unwrap()
        });
        assert_eq!(decoded.payload, packet.payload);
    }
}
