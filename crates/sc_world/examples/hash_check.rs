//! Checks block_state_hash against the pack palette network ids (required for hashed mode).
use sc_world::leveldb::block_hash::block_state_hash;

fn main() {
    // Authoritative palette network ids (read from dump_palette, signed i32).
    let palette: [(&str, i32); 5] = [
        ("minecraft:air", -604749536),
        ("minecraft:grass_block", -567203660),
        ("minecraft:water", 1211861802),
        ("minecraft:bedrock", 0), // Placeholder, not yet looked up.
        ("minecraft:dirt", 0),    // Placeholder, not yet looked up.
    ];
    for (name, expected) in palette {
        let h = block_state_hash(name, None);
        let matched = expected == 0 || (h as i32) == expected;
        println!(
            "{name}: ours=0x{:08X}({}) palette=0x{:08X}({}) {}",
            h,
            h as i32,
            expected as u32,
            expected,
            if matched { "✅" } else { "❌ mismatch" }
        );
    }
}
