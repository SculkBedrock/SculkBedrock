//! One-shot tool: read Bedrock level.dat, patch one top-level Byte field, write back.
//! Usage: cargo run -p sc_nbt --example patch_level_dat -- <input> <output> <field> <byte_value>
//! Example: set commandsEnabled=1
//!   cargo run -p sc_nbt --example patch_level_dat -- worlds/OverWorld/level.dat worlds/OverWorld/level.dat commandsEnabled 1

use std::fs;
use sc_binary::interfaces::Reader;
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::local::BedrockLocalNbt;
use sc_nbt::reader::{NbtReadTrait, NbtReader};
use sc_nbt::writer::NbtWriteTrait;
use sc_nbt::NbtValue;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: patch_level_dat <input> <output> <field> <byte_value>");
        std::process::exit(1);
    }
    let input = &args[1];
    let output = &args[2];
    let field = args[3].clone();
    let value: i8 = args[4].parse().expect("byte value");

    let bytes = fs::read(input).expect("read input");
    let mut reader = ByteReader::from(bytes.clone());
    // 8-byte level.dat header (written back verbatim).
    let magic = reader.read_bytes(8).expect("magic");
    let nbt = NbtReader::from_reader(&mut reader)
        .read::<BedrockLocalNbt>()
        .expect("parse NBT");
    let NbtValue::Compound(mut compound) = nbt else {
        panic!("root is not Compound");
    };
    let before = compound.get(&field).cloned();
    println!("field '{field}' before: {before:?}");
    compound.insert(&field, NbtValue::Byte(value));

    let mut writer = ByteWriter::new();
    writer.write_raw(&magic).expect("write magic");
    BedrockLocalNbt::write(&mut writer, &NbtValue::Compound(compound)).expect("write NBT");
    fs::write(output, writer.as_slice()).expect("write output");
    println!("patched {field}={value} -> {output}");
}
