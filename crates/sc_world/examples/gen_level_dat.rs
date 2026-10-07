//! Regenerates level.dat: moves Spawn to flat-world surface (0,-60,0) and zeroes timestamps.
//! Usage: cargo run -p sc_world --example gen_level_dat -- <level.dat>
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::local::BedrockLocalNbt;
use sc_nbt::reader::NbtReadTrait;
use sc_nbt::reader::NbtReader;
use sc_nbt::writer::NbtWriteTrait;
use sc_nbt::writer::NbtWriter;
use sc_nbt::NbtValue;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: gen_level_dat <level.dat>");
    let bytes = std::fs::read(&path).expect("read level.dat");
    assert!(bytes.len() > 8, "level.dat too short");
    let header = &bytes[..8];
    let mut reader = ByteReader::from(&bytes[8..]);
    let mut nbt_reader = NbtReader::from_reader(&mut reader);
    let mut root = nbt_reader.read::<BedrockLocalNbt>().expect("parse nbt");
    if let NbtValue::Compound(c) = &mut root {
        // Flat-world surface y=-61 (player feet at -60). Clears the SpawnY=32767
        // sentinel and the limited-world-origin sentinel.
        c.insert("SpawnX", NbtValue::Int(0));
        c.insert("SpawnY", NbtValue::Int(-60));
        c.insert("SpawnZ", NbtValue::Int(0));
        c.insert("LimitedWorldOriginX", NbtValue::Int(0));
        c.insert("LimitedWorldOriginY", NbtValue::Int(-60));
        c.insert("LimitedWorldOriginZ", NbtValue::Int(0));
        c.insert("currentTick", NbtValue::Long(0));
        c.insert("Time", NbtValue::Long(0));
        println!("Spawn = (0, -60, 0)");
    } else {
        panic!("root not compound");
    }
    match &root {
        sc_nbt::NbtValue::Compound(c) => println!(
            "DEBUG fields={} keys={:?}",
            c.iter().count(),
            c.iter().map(|(k, _)| k.clone()).take(5).collect::<Vec<_>>()
        ),
        _ => println!("DEBUG root not compound"),
    }
    let mut writer = ByteWriter::new();
    writer.write(header).expect("write header");
    let mut nbt_writer = NbtWriter::from_writer(&mut writer);
    nbt_writer
        .write::<BedrockLocalNbt>(&root)
        .expect("write nbt");
    std::fs::write(&path, writer.as_slice()).expect("save level.dat");
    println!("level.dat regenerated: {} bytes", writer.as_slice().len());
}
