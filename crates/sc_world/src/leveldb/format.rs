//! Bedrock LevelDB key layout and value parsers.
//!
//! Parsing is versioned and lenient where vanilla is known to vary: unknown
//! subchunk versions and legacy pre-1.13 block formats surface as
//! `WorldStorageError::Unsupported` instead of panicking, and unknown NBT
//! fields inside palette entries or block entities are preserved untouched.

use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::local::BedrockLocalNbt;
use sc_nbt::reader::{NbtReadTrait, NbtReader};
use sc_nbt::writer::NbtWriter;
use sc_nbt::NbtValue;

use crate::block_dictionary::{BlockStateDictionary, BlockStateEntry};
use crate::chunk::{
    packed_word_count, palette_bits, read_packed_word, BlockRuntimeId, PalettedBiomeStorage,
    PalettedBlockStorage, SubChunk, SubChunkIndex, SUBCHUNK_VOLUME,
};
use crate::storage::{BlockSpilloverWrite, SpilloverOperationId};
use crate::storage::{ChunkKey, WorldStorageError};

use super::block_hash::block_state_hash;

// ---------------------------------------------------------------------------
// Key layout
// ---------------------------------------------------------------------------

/// 3D biomes + heightmap (1.18+).
pub const TAG_DATA_3D: u8 = 0x2B;
/// Chunk version (1.16.100+); older worlds use [`TAG_LEGACY_CHUNK_VERSION`].
pub const TAG_CHUNK_VERSION: u8 = 0x2C;
/// Legacy 2D heightmap + biomes.
pub const TAG_DATA_2D: u8 = 0x2D;
/// One subchunk of block storage; key carries the subchunk y index.
pub const TAG_SUBCHUNK_PREFIX: u8 = 0x2F;
/// Concatenated block entity NBT compounds.
pub const TAG_BLOCK_ENTITY: u8 = 0x31;
/// Chunk version tag used before 1.16.100.
pub const TAG_LEGACY_CHUNK_VERSION: u8 = 0x76;

/// Private, versioned namespace for accepted cross-column generation facts.
/// It is deliberately outside every Bedrock chunk key/tag layout.
pub const SPILLOVER_JOURNAL_NAMESPACE: &[u8] = b"SCSP";
pub const SPILLOVER_JOURNAL_VERSION: u8 = 1;

pub fn spillover_journal_target_prefix(key: ChunkKey) -> Vec<u8> {
    let mut out = Vec::with_capacity(17);
    out.extend_from_slice(SPILLOVER_JOURNAL_NAMESPACE);
    out.push(SPILLOVER_JOURNAL_VERSION);
    out.extend_from_slice(&key.dimension.to_le_bytes());
    out.extend_from_slice(&key.position.x.to_le_bytes());
    out.extend_from_slice(&key.position.z.to_le_bytes());
    out
}

pub fn spillover_journal_key(key: ChunkKey, operation_id: SpilloverOperationId) -> Vec<u8> {
    let mut out = spillover_journal_target_prefix(key);
    out.extend_from_slice(&operation_id.source.dimension.to_le_bytes());
    out.extend_from_slice(&operation_id.source.position.x.to_le_bytes());
    out.extend_from_slice(&operation_id.source.position.z.to_le_bytes());
    out.extend_from_slice(&operation_id.ordinal.to_le_bytes());
    out
}

pub fn parse_spillover_journal_key(bytes: &[u8]) -> Option<(ChunkKey, SpilloverOperationId)> {
    const KEY_LENGTH: usize = 33;
    if bytes.len() != KEY_LENGTH
        || !bytes.starts_with(SPILLOVER_JOURNAL_NAMESPACE)
        || bytes.get(4).copied()? != SPILLOVER_JOURNAL_VERSION
    {
        return None;
    }
    let read_i32 = |offset: usize| {
        let bytes: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
        Some(i32::from_le_bytes(bytes))
    };
    let target = ChunkKey::new(
        read_i32(5)?,
        crate::chunk::ChunkPosition::new(read_i32(9)?, read_i32(13)?),
    );
    let source = ChunkKey::new(
        read_i32(17)?,
        crate::chunk::ChunkPosition::new(read_i32(21)?, read_i32(25)?),
    );
    let ordinal = u32::from_le_bytes(bytes.get(29..33)?.try_into().ok()?);
    Some((target, SpilloverOperationId::new(source, ordinal)))
}

pub fn encode_spillover_journal_value(write: BlockSpilloverWrite) -> Vec<u8> {
    let mut out = Vec::with_capacity(17);
    out.extend_from_slice(&write.x.to_le_bytes());
    out.extend_from_slice(&write.y.to_le_bytes());
    out.extend_from_slice(&write.z.to_le_bytes());
    out.push(write.layer as u8);
    out.extend_from_slice(&write.block.0.to_le_bytes());
    out
}

pub fn decode_spillover_journal_value(
    target: ChunkKey,
    bytes: &[u8],
) -> Option<BlockSpilloverWrite> {
    if bytes.len() != 17 {
        return None;
    }
    let read_i32 = |offset: usize| {
        let bytes: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
        Some(i32::from_le_bytes(bytes))
    };
    Some(BlockSpilloverWrite {
        key: target,
        x: read_i32(0)?,
        y: read_i32(4)?,
        z: read_i32(8)?,
        layer: bytes[12] as usize,
        block: crate::chunk::BlockRuntimeId(u32::from_le_bytes(
            bytes.get(13..17)?.try_into().ok()?,
        )),
    })
}

/// Builds a LevelDB key for a chunk-scoped record:
/// `x:i32 LE, z:i32 LE, [dimension:i32 LE when not overworld], tag, [subchunk y:i8]`.
pub fn chunk_record_key(key: ChunkKey, tag: u8, subchunk_y: Option<i8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(14);
    out.extend_from_slice(&key.position.x.to_le_bytes());
    out.extend_from_slice(&key.position.z.to_le_bytes());
    if key.dimension != 0 {
        out.extend_from_slice(&key.dimension.to_le_bytes());
    }
    out.push(tag);
    if let Some(y) = subchunk_y {
        out.push(y as u8);
    }
    out
}

// ---------------------------------------------------------------------------
// Paletted block storage (persistent encoding)
// ---------------------------------------------------------------------------

const MAX_LAYERS: usize = 16;
const MAX_PALETTE: u32 = SUBCHUNK_VOLUME as u32;
/// Header byte meaning "same as previous storage" (biomes only).
const COPY_PREVIOUS_HEADER: u8 = 0xFF;

fn corrupt(message: impl Into<String>) -> WorldStorageError {
    WorldStorageError::Corrupt(message.into())
}

fn read_error(context: &str, error: std::io::Error) -> WorldStorageError {
    corrupt(format!("{context}: {error}"))
}

/// Read on-disk packed words (4096 palette indices packed `bits` wide into u32 LE words).
/// Word layout matches the in-memory canonical form; only at 2 or fewer entries does disk use 1 bit
/// while canonical uses 2 bits, repacked by [`PalettedBlockStorage::from_packed`].
fn read_packed_words(reader: &mut ByteReader, bits: u8) -> Result<Vec<u32>, WorldStorageError> {
    if !matches!(bits, 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16) {
        return Err(corrupt(format!(
            "invalid paletted storage bit width {bits}"
        )));
    }
    let word_count = packed_word_count(bits);
    let mut words = Vec::with_capacity(word_count);
    for _ in 0..word_count {
        words.push(
            reader
                .read_u32_le()
                .map_err(|error| read_error("paletted storage word", error))?,
        );
    }
    Ok(words)
}

/// Check that every packed index is inside the palette range.
fn packed_indices_in_range(words: &[u32], bits: u8, palette_len: usize) -> bool {
    if words.len() != packed_word_count(bits) {
        return false;
    }
    (0..SUBCHUNK_VOLUME).all(|index| {
        read_packed_word(words, bits, index).map_or(false, |value| (value as usize) < palette_len)
    })
}

/// Reads one persistent block storage: header, packed indices, then a
/// little-endian NBT palette. Palette entries are hashed into network
/// runtime ids; the `version` field and unknown fields do not affect the
/// hash and are ignored without error.
fn read_block_storage(reader: &mut ByteReader) -> Result<PalettedBlockStorage, WorldStorageError> {
    let header = reader
        .read_u8()
        .map_err(|error| read_error("block storage header", error))?;
    if header & 1 != 0 {
        return Err(corrupt(
            "block storage has network flag set in persistent data",
        ));
    }
    let bits = header >> 1;

    let words = if bits == 0 {
        Vec::new()
    } else {
        read_packed_words(reader, bits)?
    };

    let palette_len = if bits == 0 {
        1
    } else {
        let len = reader
            .read_u32_le()
            .map_err(|error| read_error("block palette length", error))?;
        if len == 0 || len > MAX_PALETTE {
            return Err(corrupt(format!("block palette length {len} out of range")));
        }
        len
    };

    let mut palette = Vec::with_capacity(palette_len as usize);
    for _ in 0..palette_len {
        palette.push(read_palette_entry(reader)?);
    }

    if palette.len() == 1 {
        return Ok(PalettedBlockStorage::Single(palette[0]));
    }
    if !packed_indices_in_range(&words, bits, palette.len()) {
        return Err(corrupt("block storage index out of palette range"));
    }
    Ok(PalettedBlockStorage::from_packed(palette, words, bits))
}

fn read_palette_entry(reader: &mut ByteReader) -> Result<BlockRuntimeId, WorldStorageError> {
    let nbt = NbtReader::from_reader(reader)
        .read::<BedrockLocalNbt>()
        .map_err(|error| read_error("block palette NBT", error))?;
    let compound = nbt
        .as_compound()
        .ok_or_else(|| corrupt("block palette entry is not a compound"))?;
    let name = compound
        .get("name")
        .and_then(NbtValue::as_string)
        .ok_or_else(|| corrupt("block palette entry has no name"))?;
    let states = compound.get("states").and_then(NbtValue::as_compound);
    let hash = block_state_hash(name, states);
    // Bootstrap dictionary: the only place holding both the hash and raw name+states.
    // The hit path takes one read-lock lookup with zero allocation; see the block_dictionary docs.
    crate::block_dictionary::BlockStateDictionary::global().record_with(hash, || {
        crate::block_dictionary::BlockStateEntry {
            name: name.clone(),
            states: states.cloned(),
        }
    });
    Ok(BlockRuntimeId(hash))
}

// ---------------------------------------------------------------------------
// Subchunks
// ---------------------------------------------------------------------------

/// Parses one `SubChunkPrefix` value. `key_y` is the subchunk index taken
/// from the LevelDB key; format version 9 embeds its own index and wins.
pub fn parse_subchunk(bytes: &[u8], key_y: i8) -> Result<SubChunk, WorldStorageError> {
    let mut reader = ByteReader::from(bytes.to_vec());
    let version = reader
        .read_u8()
        .map_err(|error| read_error("subchunk version", error))?;
    match version {
        1 => {
            let layer = read_block_storage(&mut reader)?;
            Ok(SubChunk {
                index: SubChunkIndex::new(key_y),
                layers: vec![layer],
            })
        }
        8 | 9 => {
            let layer_count = reader
                .read_u8()
                .map_err(|error| read_error("subchunk layer count", error))?
                as usize;
            let y = if version == 9 {
                reader
                    .read_i8()
                    .map_err(|error| read_error("subchunk y index", error))?
            } else {
                key_y
            };
            if layer_count > MAX_LAYERS {
                return Err(corrupt(format!("subchunk has {layer_count} layers")));
            }
            let mut layers = Vec::with_capacity(layer_count);
            for _ in 0..layer_count {
                layers.push(read_block_storage(&mut reader)?);
            }
            Ok(SubChunk {
                index: SubChunkIndex::new(y),
                layers,
            })
        }
        // 0 and 2..=7 are pre-1.13 block-id/metadata formats.
        version if version <= 7 => Err(WorldStorageError::Unsupported(format!(
            "legacy subchunk format version {version}"
        ))),
        version => Err(WorldStorageError::Unsupported(format!(
            "unknown subchunk format version {version}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Biomes
// ---------------------------------------------------------------------------

fn read_biome_storage(
    reader: &mut ByteReader,
    previous: Option<&PalettedBiomeStorage>,
) -> Result<PalettedBiomeStorage, WorldStorageError> {
    let header = reader
        .read_u8()
        .map_err(|error| read_error("biome storage header", error))?;
    if header == COPY_PREVIOUS_HEADER {
        return previous
            .cloned()
            .ok_or_else(|| corrupt("biome storage copies previous but none exists"));
    }
    let bits = header >> 1;
    if bits == 0 {
        let id = reader
            .read_i32_le()
            .map_err(|error| read_error("biome id", error))?;
        return Ok(PalettedBiomeStorage::Single(id as u32));
    }

    let words = read_packed_words(reader, bits)?;
    let palette_len = reader
        .read_u32_le()
        .map_err(|error| read_error("biome palette length", error))?;
    if palette_len == 0 || palette_len > MAX_PALETTE {
        return Err(corrupt(format!(
            "biome palette length {palette_len} out of range"
        )));
    }
    let mut palette = Vec::with_capacity(palette_len as usize);
    for _ in 0..palette_len {
        palette.push(
            reader
                .read_i32_le()
                .map_err(|error| read_error("biome palette entry", error))? as u32,
        );
    }
    if palette.len() == 1 {
        return Ok(PalettedBiomeStorage::Single(palette[0]));
    }
    if !packed_indices_in_range(&words, bits, palette.len()) {
        return Err(corrupt("biome storage index out of palette range"));
    }
    Ok(PalettedBiomeStorage::from_packed(palette, words, bits))
}

/// Parses a `Data3D` value: a 512-byte heightmap followed by one biome
/// storage per subchunk section (with 0xFF "copy previous" markers).
/// The result is normalized to exactly `section_count` entries.
pub fn parse_data_3d(
    bytes: &[u8],
    section_count: usize,
) -> Result<Vec<PalettedBiomeStorage>, WorldStorageError> {
    let mut reader = ByteReader::from(bytes.to_vec());
    reader
        .read_bytes(512)
        .map_err(|error| read_error("data3d heightmap", error))?;

    let mut sections: Vec<PalettedBiomeStorage> = Vec::new();
    while reader.peek_ahead(0).is_ok() && sections.len() < section_count.max(64) {
        let storage = read_biome_storage(&mut reader, sections.last())?;
        sections.push(storage);
    }
    if sections.is_empty() {
        return Err(corrupt("data3d has no biome sections"));
    }
    normalize_sections(&mut sections, section_count);
    Ok(sections)
}

/// Parses the legacy `Data2D` value (512-byte heightmap + 256 column biome
/// bytes) into per-section 3D storages by replicating each column vertically.
pub fn parse_data_2d(
    bytes: &[u8],
    section_count: usize,
) -> Result<Vec<PalettedBiomeStorage>, WorldStorageError> {
    let mut reader = ByteReader::from(bytes.to_vec());
    reader
        .read_bytes(512)
        .map_err(|error| read_error("data2d heightmap", error))?;
    let columns = reader
        .read_bytes(256)
        .map_err(|error| read_error("data2d biomes", error))?;

    let mut palette: Vec<u32> = Vec::new();
    let mut column_palette_index = [0u16; 256];
    for column in 0..256 {
        let id = columns[column] as u32;
        let index = match palette.iter().position(|entry| *entry == id) {
            Some(index) => index,
            None => {
                palette.push(id);
                palette.len() - 1
            }
        };
        column_palette_index[column] = index as u16;
    }

    let storage = if palette.len() == 1 {
        PalettedBiomeStorage::Single(palette[0])
    } else {
        let mut indices = vec![0u16; SUBCHUNK_VOLUME];
        for x in 0..16usize {
            for z in 0..16usize {
                // Legacy 2D biome bytes are stored per column as (z << 4) | x.
                let palette_index = column_palette_index[(z << 4) | x];
                for y in 0..16usize {
                    indices[(x << 8) | (z << 4) | y] = palette_index;
                }
            }
        }
        PalettedBiomeStorage::from_indices(palette, &indices)
    };

    Ok(vec![storage; section_count.max(1)])
}

fn normalize_sections(sections: &mut Vec<PalettedBiomeStorage>, section_count: usize) {
    if section_count == 0 {
        return;
    }
    if sections.len() > section_count {
        sections.truncate(section_count);
    }
    while sections.len() < section_count {
        let last = sections
            .last()
            .cloned()
            .unwrap_or(PalettedBiomeStorage::Single(0));
        sections.push(last);
    }
}

/// Encode a complete persistent Data3D record. Heightmap bytes are supplied
/// opaquely by the caller so existing Bedrock values can be preserved exactly.
pub fn encode_data_3d(
    heightmap: &[u8; 512],
    sections: &[PalettedBiomeStorage],
    section_count: usize,
) -> Result<Vec<u8>, WorldStorageError> {
    if section_count == 0 || sections.len() != section_count {
        return Err(WorldStorageError::Unsupported(format!(
            "Data3D requires exactly {section_count} biome sections, got {}",
            sections.len()
        )));
    }
    let mut writer = ByteWriter::new();
    writer
        .write(heightmap)
        .map_err(|error| read_error("data3d heightmap", error))?;
    let mut previous = None;
    for storage in sections {
        write_biome_storage(storage, previous, &mut writer)?;
        previous = Some(storage);
    }
    Ok(writer.as_slice().to_vec())
}

fn write_biome_storage(
    storage: &PalettedBiomeStorage,
    previous: Option<&PalettedBiomeStorage>,
    writer: &mut ByteWriter,
) -> Result<(), WorldStorageError> {
    if previous == Some(storage) {
        writer
            .write_u8(COPY_PREVIOUS_HEADER)
            .map_err(|error| read_error("biome copy-previous header", error))?;
        return Ok(());
    }

    let write_single = |id: u32, writer: &mut ByteWriter| {
        writer
            .write_u8(0)
            .map_err(|error| read_error("biome storage header", error))?;
        writer
            .write_i32_le(id as i32)
            .map_err(|error| read_error("biome id", error))
    };
    match storage {
        PalettedBiomeStorage::Single(id) => write_single(*id, writer),
        PalettedBiomeStorage::Paletted { palette, .. } if palette.len() == 1 => {
            write_single(palette[0], writer)
        }
        PalettedBiomeStorage::Paletted { palette, words } => {
            if palette.is_empty() || palette.len() > MAX_PALETTE as usize {
                return Err(WorldStorageError::Unsupported(format!(
                    "biome palette size {} is out of range",
                    palette.len()
                )));
            }
            let memory_bits = palette_bits(palette.len());
            if words.len() != packed_word_count(memory_bits)
                || !packed_indices_in_range(words, memory_bits, palette.len())
            {
                return Err(corrupt("biome storage words do not match palette"));
            }
            let disk_bits = persistent_bits(palette.len()).ok_or_else(|| {
                WorldStorageError::Unsupported(format!(
                    "biome palette size {} exceeds persistent bit-width",
                    palette.len()
                ))
            })?;
            writer
                .write_u8(disk_bits << 1)
                .map_err(|error| read_error("biome storage header", error))?;
            if disk_bits == memory_bits {
                write_packed_words(words, writer)?;
            } else {
                let repacked = crate::chunk::repack_packed_words(words, memory_bits, disk_bits);
                write_packed_words(&repacked, writer)?;
            }
            writer
                .write_u32_le(palette.len() as u32)
                .map_err(|error| read_error("biome palette length", error))?;
            for id in palette {
                writer
                    .write_i32_le(*id as i32)
                    .map_err(|error| read_error("biome palette entry", error))?;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Block entities
// ---------------------------------------------------------------------------

/// Parses a `BlockEntity` value: little-endian NBT compounds back to back.
pub fn parse_block_entities(bytes: &[u8]) -> Result<Vec<NbtValue>, WorldStorageError> {
    let mut reader = ByteReader::from(bytes.to_vec());
    let mut entities = Vec::new();
    while reader.peek_ahead(0).is_ok() {
        let nbt = NbtReader::from_reader(&mut reader)
            .read::<BedrockLocalNbt>()
            .map_err(|error| read_error("block entity NBT", error))?;
        entities.push(nbt);
    }
    Ok(entities)
}

// ---------------------------------------------------------------------------
// Write side: persistent subchunk encoding.
// ---------------------------------------------------------------------------

/// Persistent palette bit width (read-side supported set; widths above 16 bits are unsupported, returns None).
fn persistent_bits(palette_len: usize) -> Option<u8> {
    let mut bits = 1u8;
    while (1usize << bits) < palette_len && bits < 16 {
        bits += 1;
    }
    match bits {
        1 | 2 | 3 | 4 | 5 | 6 => Some(bits),
        7 | 8 => Some(8),
        9..=16 => Some(16),
        _ => None,
    }
}

/// Packed words to u32 LE byte stream (direct copy when disk and canonical widths agree).
fn write_packed_words(words: &[u32], writer: &mut ByteWriter) -> Result<(), WorldStorageError> {
    for word in words {
        writer
            .write_u32_le(*word)
            .map_err(|error| read_error("packed indices word", error))?;
    }
    Ok(())
}

/// Write one palette-entry NBT: {name, states, version:0} (read side hashes name+states,
/// same FNV algorithm as the network; version is excluded from the hash).
fn write_palette_entry(
    writer: &mut ByteWriter,
    entry: &BlockStateEntry,
) -> Result<(), WorldStorageError> {
    let mut compound = CompoundNbt::new(None);
    compound.insert("name", NbtValue::String(entry.name.clone()));
    if let Some(states) = &entry.states {
        compound.insert("states", NbtValue::Compound(states.clone()));
    }
    compound.insert("version", NbtValue::Int(0));
    NbtWriter::from_writer(writer)
        .write::<BedrockLocalNbt>(&NbtValue::Compound(compound))
        .map_err(|error| read_error("palette entry NBT", error))
}

/// Persistent block storage (mirror of read_block_storage).
fn encode_block_storage_persistent(
    storage: &PalettedBlockStorage,
    writer: &mut ByteWriter,
) -> Result<(), WorldStorageError> {
    match storage {
        PalettedBlockStorage::Single(runtime_id) => {
            // header: bits=0 (single value), followed by one palette NBT entry.
            writer
                .write_u8(0)
                .map_err(|error| read_error("storage header", error))?;
            let entry = BlockStateDictionary::global()
                .get(runtime_id.0)
                .ok_or_else(|| {
                    WorldStorageError::Unsupported(format!(
                        "hash 0x{:08X} 不在自举字典，无法写回（先读后写）",
                        runtime_id.0
                    ))
                })?;
            write_palette_entry(writer, &entry)
        }
        PalettedBlockStorage::Paletted { palette, words } => {
            if palette.is_empty() {
                return Err(WorldStorageError::Unsupported(
                    "empty palette cannot be persisted".to_string(),
                ));
            }
            let bits = persistent_bits(palette.len()).ok_or_else(|| {
                WorldStorageError::Unsupported(format!(
                    "palette 大小 {} 超出持久化位宽上限（>16 位）",
                    palette.len()
                ))
            })?;
            writer
                .write_u8(bits << 1)
                .map_err(|error| read_error("storage header", error))?;
            // Canonical and disk widths only diverge at 2 or fewer entries (2 bits vs 1 bit): repack.
            if bits == palette_bits(palette.len()) {
                write_packed_words(words, writer)?;
            } else {
                let repacked =
                    crate::chunk::repack_packed_words(words, palette_bits(palette.len()), bits);
                write_packed_words(&repacked, writer)?;
            }
            writer
                .write_u32_le(palette.len() as u32)
                .map_err(|error| read_error("palette length", error))?;
            for runtime_id in palette {
                let entry = BlockStateDictionary::global()
                    .get(runtime_id.0)
                    .ok_or_else(|| {
                        WorldStorageError::Unsupported(format!(
                            "hash 0x{:08X} 不在自举字典，无法写回（先读后写）",
                            runtime_id.0
                        ))
                    })?;
                write_palette_entry(writer, &entry)?;
            }
            Ok(())
        }
    }
}

/// Persistent subchunk (v8: version 8 + layer_count + layers; index comes from key_y).
pub fn encode_subchunk_persistent(subchunk: &SubChunk) -> Result<Vec<u8>, WorldStorageError> {
    let mut writer = ByteWriter::new();
    writer
        .write_u8(8)
        .map_err(|error| read_error("subchunk version", error))?;
    writer
        .write_u8(subchunk.layers.len().min(u8::MAX as usize) as u8)
        .map_err(|error| read_error("subchunk layer count", error))?;
    for layer in &subchunk.layers {
        encode_block_storage_persistent(layer, &mut writer)?;
    }
    Ok(writer.as_slice().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkPosition;

    fn le_nbt_block(name: &str) -> Vec<u8> {
        // tag 10, empty root name, {name: <name>, states: {}}, end.
        let mut out = vec![10u8, 0, 0];
        out.push(8);
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(b"name");
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(10);
        out.extend_from_slice(&6u16.to_le_bytes());
        out.extend_from_slice(b"states");
        out.push(0);
        out.push(0);
        out
    }

    /// name to FNV1a state hash (no states).
    fn palette_hash(name: &str) -> u32 {
        block_state_hash(name, None)
    }

    #[test]
    fn overworld_key_has_no_dimension() {
        let key = ChunkKey::new(0, ChunkPosition::new(1, -2));
        assert_eq!(
            chunk_record_key(key, TAG_CHUNK_VERSION, None),
            vec![1, 0, 0, 0, 0xFE, 0xFF, 0xFF, 0xFF, TAG_CHUNK_VERSION],
        );
    }

    #[test]
    fn nether_key_embeds_dimension_and_subchunk_y() {
        let key = ChunkKey::new(1, ChunkPosition::new(0, 0));
        assert_eq!(
            chunk_record_key(key, TAG_SUBCHUNK_PREFIX, Some(-4)),
            vec![
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                0,
                0,
                0,
                TAG_SUBCHUNK_PREFIX,
                0xFC
            ],
        );
    }

    #[test]
    fn parses_version_9_single_storage_subchunk() {
        let mut bytes = vec![9u8, 1, 0xFC]; // version 9, 1 layer, y = -4
        bytes.push(0); // header: bits 0, persistent
        bytes.extend_from_slice(&le_nbt_block("minecraft:air"));
        let subchunk = parse_subchunk(&bytes, 0).unwrap();
        assert_eq!(subchunk.index.y, -4);
        assert_eq!(subchunk.layers.len(), 1);
        assert!(subchunk.layers[0].is_single());
    }

    #[test]
    fn parses_two_entry_palette_with_one_bit_indices() {
        let mut bytes = vec![8u8, 1]; // version 8, 1 layer
        bytes.push(1 << 1); // 1 bit per index, persistent
                            // 4096 one-bit indices → 128 words; word 0 = 0b10 → index 1 at block 1.
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for _ in 1..128 {
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        bytes.extend_from_slice(&2u32.to_le_bytes()); // palette length
        bytes.extend_from_slice(&le_nbt_block("minecraft:air"));
        bytes.extend_from_slice(&le_nbt_block("minecraft:stone"));

        let subchunk = parse_subchunk(&bytes, 3).unwrap();
        assert_eq!(subchunk.index.y, 3);
        match &subchunk.layers[0] {
            PalettedBlockStorage::Paletted { palette, words } => {
                assert_eq!(palette.len(), 2);
                // Disk 1 bit to in-memory canonical 2 bits (256 words).
                assert_eq!(words.len(), 256);
                assert_ne!(palette[0], palette[1]);
            }
            other => panic!("expected paletted storage, got {other:?}"),
        }
        // Semantic check: block 1 points at palette entry 1 (stone hash), the rest are air.
        assert_eq!(
            subchunk.layers[0].get(1),
            Some(BlockRuntimeId(palette_hash("minecraft:stone")))
        );
        assert_eq!(
            subchunk.layers[0].get(0),
            Some(BlockRuntimeId(palette_hash("minecraft:air")))
        );
    }

    #[test]
    fn persistent_encode_roundtrips_through_parse() {
        use crate::chunk::LocalBlockPosition;
        // Build: two layers with 2 entries (disk 1 bit) and 5 entries (disk 3 bits).
        let mut subchunk = crate::chunk::SubChunk::empty(
            crate::chunk::SubChunkIndex::new(-4),
            BlockRuntimeId(crate::block_dictionary::air_runtime_id()),
        );
        subchunk.set_block(
            0,
            LocalBlockPosition::new(1, 2, 3).unwrap(),
            BlockRuntimeId(palette_hash("minecraft:stone")),
        );
        let air = BlockRuntimeId(crate::block_dictionary::air_runtime_id());
        let stone = BlockRuntimeId(palette_hash("minecraft:stone"));
        let dirt = BlockRuntimeId(palette_hash("minecraft:dirt"));
        subchunk.layers.push({
            let mut layer = PalettedBlockStorage::single(air);
            layer.set(0, stone);
            layer.set(1, dirt);
            layer.set(2, stone);
            layer
        });
        // Register the dictionary first (writeback needs name+states).
        for name in ["minecraft:air", "minecraft:stone", "minecraft:dirt"] {
            crate::block_dictionary::BlockStateDictionary::global().record_with(
                palette_hash(name),
                || crate::block_dictionary::BlockStateEntry {
                    name: name.to_string(),
                    states: None,
                },
            );
        }

        let encoded = encode_subchunk_persistent(&subchunk).unwrap();
        let parsed = parse_subchunk(&encoded, -4).unwrap();
        assert_eq!(parsed.index.y, -4);
        assert_eq!(parsed.layers.len(), subchunk.layers.len());
        for (original, roundtripped) in subchunk.layers.iter().zip(parsed.layers.iter()) {
            for index in 0..SUBCHUNK_VOLUME {
                assert_eq!(
                    original.get(index),
                    roundtripped.get(index),
                    "index {index}"
                );
            }
        }
        // Re-encoding stays byte-stable (no bit-width drift).
        let encoded_again = encode_subchunk_persistent(&parsed).unwrap();
        assert_eq!(encoded, encoded_again);
    }

    #[test]
    fn saturated_palette_compaction_remains_bedrock_persistable() {
        let old_hash = block_state_hash("minecraft:sc_compaction_old", None);
        let new_hash = block_state_hash("minecraft:sc_compaction_new", None);
        for (hash, name) in [
            (old_hash, "minecraft:sc_compaction_old"),
            (new_hash, "minecraft:sc_compaction_new"),
        ] {
            BlockStateDictionary::global().record_with(hash, || BlockStateEntry {
                name: name.to_owned(),
                states: None,
            });
        }

        let mut stale_palette = (0..SUBCHUNK_VOLUME)
            .map(|index| BlockRuntimeId(index as u32 + 900_000))
            .collect::<Vec<_>>();
        stale_palette[0] = BlockRuntimeId(old_hash);
        let indices = vec![0u16; SUBCHUNK_VOLUME];
        let mut layer = PalettedBlockStorage::from_indices(stale_palette, &indices);
        assert_eq!(
            layer.set(0, BlockRuntimeId(new_hash)),
            Some(BlockRuntimeId(old_hash))
        );
        match &layer {
            PalettedBlockStorage::Paletted { palette, .. } => {
                assert_eq!(
                    palette.len(),
                    2,
                    "unused historical palette entries compact"
                );
                assert!(palette.contains(&BlockRuntimeId(old_hash)));
                assert!(palette.contains(&BlockRuntimeId(new_hash)));
            }
            PalettedBlockStorage::Single(_) => unreachable!(),
        }

        let subchunk = SubChunk {
            index: SubChunkIndex::new(0),
            layers: vec![layer],
        };
        let encoded = encode_subchunk_persistent(&subchunk).expect("persist compacted palette");
        let parsed = parse_subchunk(&encoded, 0).expect("load compacted palette");
        assert_eq!(parsed.layers[0].get(0), Some(BlockRuntimeId(new_hash)));
        assert_eq!(
            parsed.layers[0].get(1),
            Some(BlockRuntimeId(old_hash)),
            "all previously represented blocks keep their state"
        );
    }

    #[test]
    fn legacy_subchunk_version_is_unsupported_not_corrupt() {
        let bytes = vec![0u8, 0, 0];
        match parse_subchunk(&bytes, 0) {
            Err(WorldStorageError::Unsupported(_)) => {}
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn parses_data3d_with_copy_previous_sections() {
        let mut bytes = vec![0u8; 512]; // heightmap
        bytes.push(0); // section 0: single biome
        bytes.extend_from_slice(&1i32.to_le_bytes());
        bytes.push(0xFF); // section 1: copy previous
        let sections = parse_data_3d(&bytes, 4).unwrap();
        assert_eq!(sections.len(), 4);
        for section in &sections {
            assert_eq!(section.get(0), Some(1));
        }
    }

    #[test]
    fn data3d_encoder_roundtrips_biomes_and_preserves_heightmap_prefix() {
        let heightmap = std::array::from_fn(|index| (index as u8).wrapping_mul(17));
        let mut indices = vec![0u16; SUBCHUNK_VOLUME];
        indices[1] = 1;
        let paletted = PalettedBiomeStorage::from_indices(vec![1, 7], &indices);
        let sections = vec![PalettedBiomeStorage::Single(1), paletted.clone(), paletted];

        let encoded = encode_data_3d(&heightmap, &sections, sections.len()).expect("encode Data3D");
        assert_eq!(&encoded[..512], &heightmap, "heightmap bytes remain opaque");
        assert_eq!(encoded.last(), Some(&COPY_PREVIOUS_HEADER));
        assert_eq!(
            parse_data_3d(&encoded, sections.len()).expect("parse Data3D"),
            sections
        );
    }

    #[test]
    fn parses_data2d_column_biomes() {
        let mut bytes = vec![0u8; 512];
        let mut columns = [0u8; 256];
        columns[(3 << 4) | 5] = 7; // z=3, x=5 → biome 7
        bytes.extend_from_slice(&columns);
        let sections = parse_data_2d(&bytes, 24).unwrap();
        assert_eq!(sections.len(), 24);
        let index = (5usize << 8) | (3 << 4) | 9; // x=5, z=3, any y
        assert_eq!(sections[0].get(index), Some(7));
        assert_eq!(sections[0].get(0), Some(0));
    }

    #[test]
    fn parses_concatenated_block_entities() {
        let mut bytes = le_nbt_block("minecraft:chest");
        bytes.extend_from_slice(&le_nbt_block("minecraft:furnace"));
        let entities = parse_block_entities(&bytes).unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(
            entities[0]
                .as_compound()
                .unwrap()
                .get("name")
                .unwrap()
                .as_string()
                .unwrap(),
            "minecraft:chest",
        );
    }
}
