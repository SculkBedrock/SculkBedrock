//! Real-world data probe: validates the LevelDB-to-Chunk pipeline against the
//! repo's worlds/OverWorld/db.
//!
//! Requires local world data and takes the exclusive LevelDB LOCK (fails while
//! the server runs), so it is ignored by default:
//! `cargo test -p sc_world --test real_world_probe -- --ignored --nocapture`

use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::ChunkPosition;
use sc_world::data_reader::WorldDataReader;
use sc_world::leveldb::LevelDbWorldStorage;
use sc_world::storage::{ChunkKey, WorldStorage};
use std::path::PathBuf;

fn world_dir() -> PathBuf {
    // SC_PROBE_WORLD override (can point at a db copy while the server holds the LOCK).
    if let Ok(dir) = std::env::var("SC_PROBE_WORLD") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../worlds/OverWorld")
}

#[test]
#[ignore = "Needs a local worlds/OverWorld and the LevelDB lock; run with --ignored"]
fn probe_real_overworld_chunks() {
    let dir = world_dir();
    assert!(
        dir.join("db").is_dir(),
        "worlds/OverWorld/db is missing: {}",
        dir.display()
    );

    // level.dat: uses the spawn point to locate the chunk range holding data.
    let world_data = WorldDataReader::new(dir.join("level.dat")).expect("parse level.dat");
    let (spawn_x, spawn_y, spawn_z) = world_data.sanitized_spawn();
    println!("== level.dat ==");
    println!(
        "  name={:?} storage_version={} network_version={:?}",
        world_data.world_name, world_data.storage_version, world_data.last_opened_with_version
    );
    println!(
        "  spawn=({spawn_x}, {spawn_y}, {spawn_z}) generator={}",
        world_data.generator
    );

    let storage =
        LevelDbWorldStorage::open(dir.join("db")).expect("open LevelDB (is the server running?)");

    let spawn_chunk = ChunkPosition::from_world(spawn_x, spawn_z);
    let radius = 5;
    let mut present = 0usize;
    let mut absent = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let mut nonempty_subchunks = 0usize;
    let mut total_palette_entries = 0usize;
    let mut sample_printed = false;

    for offset_x in -radius..=radius {
        for offset_z in -radius..=radius {
            let position = ChunkPosition::new(spawn_chunk.x + offset_x, spawn_chunk.z + offset_z);
            match storage.load_chunk(ChunkKey::new(0, position)) {
                Ok(None) => absent += 1,
                Err(error) => errors.push(format!("{position}: {error}")),
                Ok(Some(chunk)) => {
                    present += 1;
                    for subchunk in &chunk.subchunks {
                        if !subchunk.layers.is_empty() {
                            nonempty_subchunks += 1;
                            for layer in &subchunk.layers {
                                total_palette_entries += match layer {
                                    sc_world::chunk::PalettedBlockStorage::Single(_) => 1,
                                    sc_world::chunk::PalettedBlockStorage::Paletted {
                                        palette,
                                        ..
                                    } => palette.len(),
                                };
                            }
                        }
                    }
                    if !sample_printed && chunk.subchunks.iter().any(|s| !s.layers.is_empty()) {
                        sample_printed = true;
                        println!("== sample chunk {position} ==");
                        println!(
                            "  subchunks={} biome_sections={} block_entities={}",
                            chunk.subchunks.len(),
                            chunk.biomes.len(),
                            chunk.block_entities.len()
                        );
                        for subchunk in chunk
                            .subchunks
                            .iter()
                            .filter(|s| !s.layers.is_empty())
                            .take(3)
                        {
                            println!(
                                "  subchunk y={} layers={}",
                                subchunk.index.y,
                                subchunk.layers.len()
                            );
                        }
                        // Spawn-point block.
                        if let Some(id) = chunk.block_at(
                            spawn_x.rem_euclid(16) as u8,
                            spawn_y - 1,
                            spawn_z.rem_euclid(16) as u8,
                        ) {
                            let name = BlockStateDictionary::global()
                                .get(id.0)
                                .map(|entry| entry.to_string())
                                .unwrap_or_else(|| "<unregistered>".into());
                            println!("  block under spawn feet: {name} (0x{:08X})", id.0);
                        }
                    }
                }
            }
        }
    }

    println!(
        "== stats (radius {radius}, {} chunks) ==",
        (2 * radius + 1) * (2 * radius + 1)
    );
    println!(
        "  present: {present} absent: {absent} errors: {}",
        errors.len()
    );
    println!("  non-empty subchunks: {nonempty_subchunks} total palette entries: {total_palette_entries}");
    println!(
        "  bootstrap dictionary size: {}",
        BlockStateDictionary::global().len()
    );
    for error in errors.iter().take(10) {
        println!("  [ERR] {error}");
    }

    assert!(errors.is_empty(), "chunk parse errors");
    assert!(
        present > 0,
        "no generated chunks around spawn; world data or key encoding is wrong"
    );
    assert!(
        nonempty_subchunks > 0,
        "all subchunks are empty; subchunk parsing or version support is wrong"
    );
}
