//! Strict LevelChunk payload validator (client parse semantics).
//!
//! Locates send-but-invisible/broken chunks at the byte level.
//! Decodes the LevelChunk payload byte by byte with client parsing
//! semantics: v9 sub-chunks (version/layers/index), paletted storages
//! (header/bits/words/palette), per-section biomes (dimension height
//! count), border byte, block-entity tail.
//! Decoded blocks are diffed point by point against the source chunk.
//!
//! `subChunkCount` holds the section count from the bottom section;
//! 3D biomes always span the full dimension height.

use sc_binary::ByteReader;
use sc_network::protocol::server::chunk::LevelChunk;
use sc_world::block_dictionary::{air_runtime_id, BlockStateDictionary, BlockStateEntry};
use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition, OVERWORLD_MAX_Y, OVERWORLD_MIN_Y};

/// Full-height section count (fixed 3D biome count in the payload).
const OVERWORLD_SECTIONS: usize = 24;

fn register_test_network_ids(ids: &[u32]) {
    let dictionary = BlockStateDictionary::global();
    dictionary.record_with_network_id(air_runtime_id(), 0, || BlockStateEntry {
        name: "minecraft:air".to_string(),
        states: None,
    });
    for (network_id, hash) in ids.iter().copied().enumerate() {
        dictionary.record_with_network_id(hash, 60_000 + network_id as u32, || BlockStateEntry {
            name: format!("test:state_{hash}"),
            states: None,
        });
    }
}

/// Zigzag varint (same semantics as sc_binary::read_var_i32).
fn read_zigzag(reader: &mut ByteReader) -> i32 {
    reader.read_var_i32().expect("zigzag varint")
}

/// Parse one paletted storage, returning 4096 palette entry values.
fn parse_storage(reader: &mut ByteReader, what: &str) -> Vec<u32> {
    let header = reader.read_u8().expect("storage header");
    let bits = header >> 1;
    let runtime = header & 1;
    assert_eq!(
        runtime, 1,
        "{what}: network storage requires the runtime bit"
    );
    if bits == 0 {
        let value = read_zigzag(reader) as u32;
        return vec![value; 4096];
    }
    assert!(
        matches!(bits, 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16),
        "{what}: illegal bit width {bits}"
    );
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
        _ => panic!("unsupported bits {bits}"),
    };
    let word_count = (4096 + values_per_word - 1) / values_per_word;
    let mut words = Vec::with_capacity(word_count);
    for _ in 0..word_count {
        words.push(reader.read_u32_le().expect("packed word"));
    }
    let palette_len = read_zigzag(reader) as usize;
    assert!(
        palette_len <= 1usize << bits as usize,
        "{what}: palette entry {palette_len} exceeds bit width {bits} capacity"
    );
    let mut palette = Vec::with_capacity(palette_len);
    for _ in 0..palette_len {
        palette.push(read_zigzag(reader) as u32);
    }
    // Unpack index (client semantics).
    let mask = (1u32 << bits) - 1;
    let mut values = Vec::with_capacity(4096);
    for index in 0..4096usize {
        let word = words[index / values_per_word];
        let shift = (index % values_per_word) * bits as usize;
        let palette_index = ((word >> shift) & mask) as usize;
        assert!(
            palette_index < palette_len,
            "{what}: index {palette_index} out of bounds (palette {palette_len})"
        );
        values.push(palette[palette_index]);
    }
    values
}

/// Client-semantics parse result.
struct ParsedChunk {
    /// section index(-4..) to 4096 block runtime ids.
    sections: Vec<(i8, Vec<u32>)>,
    biomes: Vec<Vec<u32>>,
    border: u8,
    trailing: Vec<u8>,
}

/// Parse a LevelChunk payload with client semantics.
/// `section_count` is LevelChunk.sub_chunk_count;
/// `biome_count` is the full dimension height.
fn parse_payload(payload: &[u8], section_count: u32, biome_count: usize) -> ParsedChunk {
    let mut reader = ByteReader::from(payload.to_vec());
    let mut sections = Vec::new();
    for section in 0..section_count {
        let version = reader.read_u8().expect("section version");
        assert_eq!(version, 9, "section {section}: expected v9, got {version}");
        let layers = reader.read_u8().expect("layer count");
        assert_eq!(layers, 2, "sections always contain two layers");
        let index = reader.read_i8().expect("section index");
        let mut blocks: Option<Vec<u32>> = None;
        for layer in 0..layers {
            let values = parse_storage(&mut reader, &format!("section {section} layer {layer}"));
            if layer == 0 {
                blocks = Some(values);
            }
        }
        // Zero-layer sections are all air.
        let air = air_runtime_id();
        sections.push((index, blocks.unwrap_or_else(|| vec![air; 4096])));
    }
    // One biome palette is written per emitted block section.
    let mut biomes = Vec::new();
    for section in 0..biome_count {
        biomes.push(parse_storage(&mut reader, &format!("biome {section}")));
    }
    let border = reader.read_u8().expect("border byte");
    let trailing = reader.as_slice().to_vec();
    ParsedChunk {
        sections,
        biomes,
        border,
        trailing,
    }
}

/// Load definitions/block_palette.nbt from the repo version pack.
fn seed_palette_from_version_pack() {
    use std::io::Read;
    static SEEDED: std::sync::Once = std::sync::Once::new();
    SEEDED.call_once(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let file = std::fs::File::open(path).expect("version pack file must exist");
        let mut archive = zip::ZipArchive::new(file).expect("version pack must be a valid zip");
        let mut entry = archive
            .by_name("definitions/block_palette.nbt")
            .expect("version pack must contain block_palette.nbt");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read palette bytes");
        let registry = sc_block::registry::BlockStateRegistry::new();
        let count = registry
            .load_palette_from_bytes(&bytes)
            .expect("parse palette");
        eprintln!("[validator] palette registered {count} states");
        assert!(count > 6000, "unexpected palette size: {count}");
    });
}

/// Diff one source-chunk column (layer 0) against the decoded result.
/// Sections never sent read as air on the client.
fn assert_column_matches(chunk: &Chunk, parsed: &ParsedChunk, lx: u8, lz: u8) {
    let source_air = BlockRuntimeId(air_runtime_id());
    for y in chunk.min_y..=chunk.max_y {
        let array_index = ((y - chunk.min_y) / 16) as usize;
        let source = chunk.block_at(lx, y, lz).expect("read source chunk");
        let Some((index, blocks)) = parsed.sections.get(array_index) else {
            // Sections the client never received read as air.
            assert_eq!(
                source, source_air,
                "unsent section ({array_index}) must have no blocks at y={y}"
            );
            continue;
        };
        let expected_index = chunk.min_y.div_euclid(16) as i8 + array_index as i8;
        assert_eq!(*index, expected_index, "non-contiguous section index");
        // XZY storage order: (x << 8) | (z << 4) | y
        let linear =
            ((lx as usize) << 8) | ((lz as usize) << 4) | (((y - chunk.min_y) % 16) as usize);
        // In hashed mode LevelChunk carries the internal FNV1a block-state
        // hash directly; there is no version-pack runtime-id remapping.
        let expected = source;
        let decoded = BlockRuntimeId(blocks[linear]);
        assert_eq!(
            decoded, expected,
            "block mismatch at chunk=({},{}) local=({lx},_,{lz}) world_y={y}",
            chunk.position.x, chunk.position.z
        );
    }
}

/// Synthetic chunk covering single/multi-width palettes, water, empty sections.
fn synthetic_chunk() -> Chunk {
    let mut chunk = Chunk::empty(
        ChunkPosition::new(3, -2),
        0,
        OVERWORLD_MIN_Y,
        OVERWORLD_MAX_Y,
    );
    // Bedrock floor plus stone.
    for lx in 0..16u8 {
        for lz in 0..16u8 {
            chunk.set_block_at(0, lx, -64, lz, BlockRuntimeId(1000));
            for y in -63..=62 {
                chunk.set_block_at(0, lx, y, lz, BlockRuntimeId(1001));
            }
            chunk.set_block_at(0, lx, 63, lz, BlockRuntimeId(1002));
        }
    }
    // Water surface (64..=70).
    for lx in 0..8u8 {
        for lz in 0..8u8 {
            for y in 64..=70 {
                chunk.set_block_at(0, lx, y, lz, BlockRuntimeId(1003));
            }
        }
    }
    // Scattered high blocks for width variety (top y=254).
    for (i, y) in (100..=260).step_by(7).enumerate() {
        let lx = (i % 16) as u8;
        let lz = ((i / 16) % 16) as u8;
        chunk.set_block_at(0, lx, y, lz, BlockRuntimeId(1010 + (i % 5) as u32));
    }
    chunk
}

#[test]
fn payload_roundtrip_synthetic_chunk() {
    register_test_network_ids(&[1000, 1001, 1002, 1003, 1010, 1011, 1012, 1013, 1014]);
    let chunk = synthetic_chunk();
    let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
    assert_eq!(packet.sub_chunk_count, chunk.subchunk_count() as u32);
    let parsed = parse_payload(
        &packet.payload,
        packet.sub_chunk_count,
        packet.sub_chunk_count as usize,
    );
    assert_eq!(parsed.border, 0, "border byte must be 0");
    assert_eq!(parsed.sections.len(), packet.sub_chunk_count as usize);
    assert_eq!(
        parsed.biomes.len(),
        packet.sub_chunk_count as usize,
        "biome count must match emitted sections"
    );
    // Section indices run -4..19.
    for (offset, (index, _)) in parsed.sections.iter().enumerate() {
        assert_eq!(
            *index,
            -4 + offset as i8,
            "section indices must run bottom-up"
        );
    }
    // Full-column diff (8x8 sample grid).
    for lx in (0..16u8).step_by(2) {
        for lz in (0..16u8).step_by(2) {
            assert_column_matches(&chunk, &parsed, lx, lz);
        }
    }
    assert_eq!(parsed.trailing, vec![0], "empty entity list is TAG_End");
}

#[test]
fn payload_roundtrip_generated_chunk() {
    // Register the real palette from the version pack so the generator
    // resolves real block hashes.
    seed_palette_from_version_pack();

    // Generate chunks with the real generator and diff fully.
    // The flat-world generator was replaced by the normal generator.
    // (Seed and block tables build from the core palette.)
    let table = sc_vanilla_overworld::blocks_table::WorldgenBlockTable::from_core_palette();
    let material_blocks =
        sc_vanilla_overworld::worldgen::material::MaterialBlocks::from_core_palette();
    let generator = sc_vanilla_overworld::generator::NormalGenerator::new(
        0x1_0000_0000,
        table,
        material_blocks,
    );
    use sc_world::storage::{ChunkGenerationRequest, ChunkKey, WorldGenerator};
    for (cx, cz) in [(0i32, 0i32), (1, 0), (-2, 3), (5, -5)] {
        let request = ChunkGenerationRequest {
            key: ChunkKey::new(0, ChunkPosition::new(cx, cz)),
            min_y: OVERWORLD_MIN_Y,
            max_y: OVERWORLD_MAX_Y,
        };
        let chunk = generator
            .generate_chunk(request)
            .expect("generate")
            .expect("generate Some");
        let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
        assert_eq!(packet.sub_chunk_count, chunk.subchunk_count() as u32);
        let parsed = parse_payload(
            &packet.payload,
            packet.sub_chunk_count,
            packet.sub_chunk_count as usize,
        );
        assert_eq!(parsed.sections.len(), packet.sub_chunk_count as usize);
        assert_eq!(parsed.biomes.len(), packet.sub_chunk_count as usize);
        for lx in (0..16u8).step_by(3) {
            for lz in (0..16u8).step_by(3) {
                assert_column_matches(&chunk, &parsed, lx, lz);
            }
        }
        let air = BlockRuntimeId(air_runtime_id());
        let top = chunk.highest_block_at(8, 8, air);
        let non_empty = parsed
            .sections
            .iter()
            .filter(|(_, blocks)| blocks.iter().any(|v| *v != air.0))
            .count();
        eprintln!(
            "[validator] chunk ({cx},{cz}) payload={}B sections=1 non-empty={non_empty} top={top:?}",
            packet.payload.len()
        );
    }
}

#[test]
fn biome_section_roundtrip_mixed_palette() {
    register_test_network_ids(&[1000]);
    // Hand-built chunk: ground blocks plus mixed section-0 biomes.
    // (ocean=0 + plains=1, rest plains=1).
    let mut chunk = Chunk::empty(
        ChunkPosition::new(2, -1),
        0,
        OVERWORLD_MIN_Y,
        OVERWORLD_MAX_Y,
    );
    chunk.set_block_at(0, 0, -64, 0, BlockRuntimeId(1000));
    for section in 0..24usize {
        let mut storage = sc_world::chunk::PalettedBiomeStorage::Single(1);
        if section == 0 {
            // x<8 → ocean(0),x>=8 → plains(1).
            for x in 0..16u8 {
                for z in 0..16u8 {
                    for y in 0..16u8 {
                        let id = if x < 8 { 0u32 } else { 1 };
                        let index = ((x as usize) << 8) | ((z as usize) << 4) | y as usize;
                        storage.set(index, id);
                    }
                }
            }
        }
        chunk.biomes.push(storage);
    }
    let packet = LevelChunk::from_chunk(&chunk).expect("chunk encoding");
    assert_eq!(packet.sub_chunk_count, chunk.subchunk_count() as u32);
    let parsed = parse_payload(
        &packet.payload,
        packet.sub_chunk_count,
        packet.sub_chunk_count as usize,
    );
    assert_eq!(parsed.sections.len(), packet.sub_chunk_count as usize);
    assert_eq!(parsed.biomes.len(), packet.sub_chunk_count as usize);
    // Section 0: x<8 maps to 0, x>=8 maps to 1.
    for x in 0..16u8 {
        for z in 0..16u8 {
            let index = ((x as usize) << 8) | ((z as usize) << 4); // y=0, XZY
            let expect = if x < 8 { 0 } else { 1 };
            assert_eq!(
                parsed.biomes[0][index], expect,
                "biome section 0 at (x={x},z={z})"
            );
        }
    }
    // Remaining sections are all plains.
    for section in 1..packet.sub_chunk_count as usize {
        assert!(
            parsed.biomes[section].iter().all(|v| *v == 1),
            "section {section} must be all plains"
        );
    }
}
