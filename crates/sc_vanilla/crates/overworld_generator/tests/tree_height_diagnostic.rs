//! Diagnostic: log-column height distribution with the real version-pack palette and world seed.
//! Purpose: locate tall-tree visuals (removable after the run).

use sc_vanilla_overworld::blocks_table::WorldgenBlockTable;
use sc_vanilla_overworld::generator::NormalGenerator;
use sc_vanilla_overworld::worldgen::material::MaterialBlocks;
use sc_world::chunk::{BlockRuntimeId, ChunkPosition, OVERWORLD_MAX_Y, OVERWORLD_MIN_Y};
use sc_world::storage::{ChunkGenerationRequest, ChunkKey, WorldGenerator};

fn load_palette_from_file() {
    let palette_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/block_palette.nbt"
    );
    let bytes = std::fs::read(palette_path).expect("read block_palette.nbt");
    let registry = sc_block::registry::BlockStateRegistry::new();
    let count = registry
        .load_palette_from_bytes(&bytes)
        .expect("register palette");
    println!("palette registered: {count}");
}

#[test]
fn tree_height_diagnostic() {
    load_palette_from_file();

    // Real world seed (level.dat RandomSeed)
    let seed: i64 = -4046542736718616516;
    let table = WorldgenBlockTable::from_core_palette();
    let material_blocks = MaterialBlocks::from_core_palette();
    let gen = NormalGenerator::new(seed, table, material_blocks);

    // Runtime ids for each log kind
    let dictionary = sc_world::block_dictionary::BlockStateDictionary::global();
    let log_ids: Vec<(String, BlockRuntimeId)> = [
        "oak_log",
        "spruce_log",
        "birch_log",
        "jungle_log",
        "dark_oak_log",
        "acacia_log",
        "cherry_log",
        "mangrove_log",
        "pale_oak_log",
    ]
    .iter()
    .filter_map(|name| {
        dictionary
            .first_hash_of(&format!("minecraft:{name}"))
            .map(|id| (name.to_string(), BlockRuntimeId(id)))
    })
    .collect();
    println!("log kinds found: {}", log_ids.len());

    // Generate 5x5 chunks near spawn (spawn ~= (1312,3120) -> chunk (82,195)),
    // expanded to 21x21 to cover more biomes; record the center biome per chunk.
    let mut total_logs = 0usize;
    let mut column_heights: Vec<(String, i32)> = Vec::new();
    for cx in 74..=90 {
        for cz in 187..=203 {
            let request = ChunkGenerationRequest {
                key: ChunkKey {
                    dimension: 0,
                    position: ChunkPosition::new(cx, cz),
                },
                min_y: OVERWORLD_MIN_Y,
                max_y: OVERWORLD_MAX_Y,
            };
            let chunk = gen.generate_chunk(request).unwrap().unwrap();
            let wc = sc_vanilla_overworld::worldgen::chunk::WorldgenChunk::new(chunk);
            let center_biome = wc.biome_id(8, 80, 8);
            if total_logs == 0 && wc.x() % 3 == 0 && wc.z() % 3 == 0 {
                println!(
                    "chunk ({}, {}) center biome = {}",
                    wc.x(),
                    wc.z(),
                    center_biome
                );
            }

            // Find all log blocks
            let mut logs: Vec<(u8, i32, u8, BlockRuntimeId)> = Vec::new();
            for y in OVERWORLD_MIN_Y..=OVERWORLD_MAX_Y {
                for lx in 0u8..16 {
                    for lz in 0u8..16 {
                        let b = wc.block_state(lx, y, lz, 0);
                        if log_ids.iter().any(|(_, id)| *id == b) {
                            logs.push((lx, y, lz, b));
                        }
                    }
                }
            }
            total_logs += logs.len();

            // Per column (x,z) x per type: contiguous run length down from the top log
            use std::collections::{BTreeMap, HashMap, HashSet};
            let mut cols: HashMap<(u8, u8, BlockRuntimeId), HashSet<i32>> = HashMap::new();
            for (lx, y, lz, b) in &logs {
                cols.entry((*lx, *lz, *b)).or_default().insert(*y);
            }
            for ((lx, lz, b), ys) in &cols {
                let name = log_ids
                    .iter()
                    .find(|(_, id)| id == b)
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                let max_y = *ys.iter().max().unwrap();
                let mut contiguous = 0;
                let mut y = max_y;
                while ys.contains(&y) {
                    contiguous += 1;
                    y -= 1;
                }
                column_heights.push((name, contiguous));
            }
        }
    }

    println!("total log blocks in 5x5 chunks: {total_logs}");
    use std::collections::BTreeMap;
    let mut dist: BTreeMap<String, BTreeMap<i32, usize>> = BTreeMap::new();
    for (name, h) in &column_heights {
        *dist.entry(name.clone())
            .or_default()
            .entry(*h)
            .or_insert(0) += 1;
    }
    for (name, d) in &dist {
        let samples: Vec<String> = d.iter().map(|(h, c)| format!("h={h}×{c}")).collect();
        println!("  {name}: {}", samples.join(", "));
    }
    let max_h = column_heights.iter().map(|(_, h)| *h).max().unwrap_or(0);
    println!("max contiguous log height: {max_h}");
}
