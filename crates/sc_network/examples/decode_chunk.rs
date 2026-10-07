//! Decode a LevelChunk payload: verify the block id at the player position (29,-60,14).
//! Expect air hash 0xDBF44120 at -60/-58.
//!
//! Note: palette length/entries are zigzag varint (matches write_var_i32);
//! reading with read_var_u32 misaligns later section headers (div-by-zero/OOB).
use sc_binary::ByteReader;
use sc_network::protocol::server::chunk::LevelChunk;
use sc_world::block_dictionary::air_runtime_id;
use sc_world::chunk::{BlockRuntimeId, Chunk, ChunkPosition};

fn main() {
    // Build a superflat chunk (1,0) (same generator logic)
    let mut chunk = Chunk::empty(ChunkPosition::new(1, 0), 0, -64, 319);
    let grass = BlockRuntimeId(sc_world::leveldb::block_hash::block_state_hash(
        "minecraft:grass_block",
        None,
    ));
    let dirt = BlockRuntimeId(sc_world::leveldb::block_hash::block_state_hash(
        "minecraft:dirt",
        None,
    ));
    let bedrock = BlockRuntimeId(sc_world::leveldb::block_hash::block_state_hash(
        "minecraft:bedrock",
        None,
    ));
    for x in 0..16u8 {
        for z in 0..16u8 {
            chunk.set_block_at(0, x, -64, z, bedrock);
            chunk.set_block_at(0, x, -63, z, dirt);
            chunk.set_block_at(0, x, -62, z, dirt);
            chunk.set_block_at(0, x, -61, z, grass);
        }
    }
    let lc = LevelChunk::from_chunk(&chunk).expect("encode chunk");
    println!(
        "sub_chunk_count={} payload_len={}",
        lc.sub_chunk_count,
        lc.payload.len()
    );
    println!(
        "payload head: {:02X?}",
        lc.payload.iter().take(16).collect::<Vec<_>>()
    );
    let mut r = ByteReader::from(lc.payload.as_slice());
    let mut sections = Vec::new();
    for _ in 0..lc.sub_chunk_count {
        let ver = r.read_u8().unwrap();
        let layer_count = r.read_u8().unwrap();
        let index = r.read_i8().unwrap();
        let mut layers = Vec::new();
        for _ in 0..layer_count {
            let header = r.read_u8().unwrap();
            let bits = header >> 1;
            let runtime = header & 1;
            assert_eq!(runtime, 1, "network storage requires the runtime bit");
            let mut palette = Vec::new();
            let mut words = Vec::new();
            if bits == 0 {
                let id = r.read_var_i32().unwrap();
                palette.push(id);
            } else {
                assert!(bits <= 32, "illegal bit width {bits}");
                let vpw = match bits {
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
                let word_count = (4096 + vpw - 1) / vpw;
                for _ in 0..word_count {
                    words.push(r.read_u32_le().unwrap());
                }
                let palette_size = r.read_var_i32().unwrap() as usize; // zigzag varint
                for _ in 0..palette_size {
                    palette.push(r.read_var_i32().unwrap());
                }
            }
            layers.push((bits, words, palette));
        }
        sections.push((ver, layer_count, index, layers));
    }
    println!("sections={}", sections.len());
    for (ver, layer_count, index, layers) in &sections {
        let palette_desc: Vec<String> = layers
            .iter()
            .map(|(_, _, palette)| {
                palette
                    .iter()
                    .map(|id| format!("0x{:08X}", *id as u32))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect();
        println!(
            "  section ver={ver} layers={layer_count} index={index} palettes=[{}]",
            palette_desc.join(" | ")
        );
    }
    // Player section index -4; local positions (13, 4, 14) and (13, 6, 14) (head)
    let air = air_runtime_id() as i32;
    for (y_world, label) in [(-60i32, "foot"), (-58i32, "head")] {
        let mut found = false;
        for (_, _, index, layers) in &sections {
            if *index != -4 {
                continue;
            }
            let y_local = (y_world - (-64)) as u32; // Base Y is -64
            let linear = (13u32 << 8) | (14 << 4) | y_local; // XZY: (x<<8)|(z<<4)|y
            for (bits, words, palette) in layers {
                found = true;
                if *bits == 0 {
                    let id = palette[0];
                    println!(
                        "{label} y={y_world}: single id=0x{:08X} {} air",
                        id as u32,
                        if id == air { "==" } else { "!=" }
                    );
                } else {
                    let vpw = match *bits {
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
                    let word = words[linear as usize / vpw];
                    let offset = (linear as usize % vpw) * *bits as usize;
                    let mask = (1u32 << bits) - 1;
                    let idx = ((word >> offset) & mask) as usize;
                    let id = palette[idx];
                    println!(
                        "{label} y={y_world}: palette[{}]=0x{:08X} {} air",
                        idx,
                        id as u32,
                        if id == air { "==" } else { "!=" }
                    );
                }
            }
            if !found {
                println!("{label} y={y_world}: NO LAYERS (0 layer)");
            }
        }
        if !found {
            println!("{label} y={y_world}: section -4 not found!");
        }
    }
    // Air hash check
    println!("air_runtime_id = 0x{:08X}", air);
}
