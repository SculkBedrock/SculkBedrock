//! sc_data_extractor: offline data conversion tool.
//!
//! Converts an external block palette file to the SC canonical `definitions/block_palette.nbt`:
//!
//! ```text
//! Input (GZIP-compressed Java-style local NBT).
//!   src/main/resources/block_palette_729.nbt
//!   or src/main/resources/runtime_block_states.dat
//! Output (uncompressed SC local NBT = BedrockLocalNbt).
//!   definitions/block_palette.nbt  →  root { blocks: List<{name, states, version}> }
//! ```
//!
//! Pipeline: gzip-decompress, parse, drop meta fields such as `name_hash/network_id/block_id/id/data/runtimeId/`
//! and `stateOverload`, keep `name+states+version` (canonical FNV1a-32 hash input), then rewrite as
//! BedrockLocalNbt. At runtime `sc_block` (SCLoad) reads `BedrockLocalNbt`, falling back to the bootstrap
//! dictionary (reverse-derived from maps) when missing.

use std::env;
use std::fs;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use sc_binary::{ByteReader, ByteWriter};
use sc_nbt::compound::CompoundNbt;
use sc_nbt::local::JavaLocalNbt;
use sc_nbt::reader::{NbtReadTrait, NbtReader};
use sc_nbt::writer::{NbtWriteTrait, NbtWriter};
use sc_nbt::NbtValue;

mod extract_defaults;
mod extract_hardness;

/// Meta fields in block_palette_*.nbt (stripped; only name/states/version are kept).
const META_KEYS: &[&str] = &[
    "name_hash",
    "protocol_runtime_id",
    "runtime_id",
    "network_id",
    "block_id",
    "id",
    "data",
    "runtimeId",
    "stateOverload",
];

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 && args[1] == "split-blocks" {
        split_blocks(&args[1..]);
        return;
    }
    if args.len() >= 2 && args[1] == "extract-defaults" {
        extract_defaults::run(&args[1..]);
        return;
    }
    if args.len() >= 2 && args[1] == "extract-hardness" {
        extract_hardness::run(&args[1..]);
        return;
    }
    if args.len() < 3 {
        eprintln!(
            "usage: sc_data_extractor <input gz nbt> <output block_palette.nbt>\n\
             e.g. sc_data_extractor block_palette_729.nbt definitions/block_palette.nbt\n\
             \n\
             sc_data_extractor split-blocks <block_palette.nbt> <output dir> [--defaults defaults.json] [--hardness hardness.json]\n\
             Split the legacy palette by identifier into definitions/blocks/<ns>/<path>.block.json\n\
             (stock format_version + minecraft:block layout + sc:default_state/sc:protocol_runtime_ids).\n\
             Multi-state defaults must come from --defaults (missing entries report TODO, never guessed).\n\
             \n\
             sc_data_extractor extract-defaults --pnx <src/main/java> --palette <block_palette.nbt> --out <defaults.json>\n\
             Extract default states from upstream Java block sources (getDefaultState semantics),\n\
             each default combination must hit the palette exactly, otherwise reported loudly and skipped.\n\
             \n\
             sc_data_extractor extract-hardness --pnx <src/main/java> --palette <block_palette.nbt> --out <hardness.json>\n\
             Extract hardness from upstream Java block sources (getHardness semantics, -1 means unbreakable)."
        );
        std::process::exit(2);
    }
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);

    let raw = fs::read(input).unwrap_or_else(|e| {
        eprintln!("failed to read input {}: {e}", input.display());
        std::process::exit(1);
    });

    // 1. Decompresses gzip.
    let mut decompressed = Vec::new();
    GzDecoder::new(&raw[..])
        .read_to_end(&mut decompressed)
        .unwrap_or_else(|e| {
            eprintln!("gzip decompress failed {}: {e}", input.display());
            std::process::exit(1);
        });
    eprintln!(
        "gzip decompressed: {} -> {} bytes",
        raw.len(),
        decompressed.len()
    );

    // 2. Parses Java-style local NBT (u16 BE strings plus LE numbers).
    let mut reader = ByteReader::from(decompressed.as_slice());
    let root = NbtReader::from_reader(&mut reader)
        .read::<JavaLocalNbt>()
        .unwrap_or_else(|e| {
            eprintln!("NBT parse failed: {e}");
            std::process::exit(1);
        });

    // 3. Extracts the blocks list (accepts both root shapes: Compound{blocks} or List; moved, never deep-copied).
    let mut blocks: Vec<NbtValue> = match root {
        NbtValue::Compound(mut c) => match c.remove("blocks") {
            Some(NbtValue::List(list)) => list,
            other => {
                eprintln!("root Compound lacks blocks List (actual: {other:?})");
                std::process::exit(1);
            }
        },
        NbtValue::List(list) => list,
        other => {
            eprintln!("unsupported root NBT type: {other:?}");
            std::process::exit(1);
        }
    };
    eprintln!("read {} block states", blocks.len());

    // 4. Cleans entries: keeps name/states/version, strips meta fields (in-place remove, no clone).
    let mut cleaned = Vec::with_capacity(blocks.len());
    let mut without_name = 0usize;
    for block in blocks.drain(..) {
        let NbtValue::Compound(mut compound) = block else {
            eprintln!("blocks element is not a Compound");
            std::process::exit(1);
        };
        let name = compound.remove("name");
        if name.is_none() {
            without_name += 1;
        }
        let states = compound.remove("states");
        let version = compound.remove("version");
        let protocol_runtime_id = compound
            .remove("runtimeId")
            .or_else(|| compound.remove("runtime_id"))
            .or_else(|| compound.remove("protocol_runtime_id"));

        let mut entry = CompoundNbt::new(None);
        if let Some(name) = name {
            entry.insert("name", name);
        }
        if let Some(states) = states {
            entry.insert("states", states);
        }
        if let Some(version) = version {
            entry.insert("version", version);
        }
        if let Some(protocol_runtime_id) = protocol_runtime_id {
            entry.insert("protocol_runtime_id", protocol_runtime_id);
        } else {
            eprintln!("block state is missing runtimeId");
            std::process::exit(1);
        }
        // Keeps unknown fields (superset fallback), strips only known meta fields.
        let keys: Vec<String> = compound.iter().map(|(key, _)| key.clone()).collect();
        for key in keys {
            if META_KEYS.contains(&key.as_str()) {
                continue;
            }
            if let Some(value) = compound.remove(&key) {
                entry.insert(&key, value);
            }
        }
        cleaned.push(NbtValue::Compound(entry));
    }
    eprintln!(
        "cleaned: {} entries ({} without name)",
        cleaned.len(),
        without_name
    );

    // 5. Assembles the root { blocks: [...] } and writes it as BedrockLocalNbt (SC local format).
    let mut root_compound = CompoundNbt::new(Some("".to_string()));
    root_compound.insert("blocks", NbtValue::List(cleaned));

    let mut writer = ByteWriter::new();
    NbtWriter::from_writer(&mut writer)
        .write::<JavaLocalNbt>(&NbtValue::Compound(root_compound))
        .unwrap_or_else(|e| {
            eprintln!("NBT write failed: {e}");
            std::process::exit(1);
        });
    let bytes = writer.as_slice().to_vec();

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, &bytes).unwrap_or_else(|e| {
        eprintln!("failed to write output {}: {e}", output.display());
        std::process::exit(1);
    });
    eprintln!("wrote {} bytes -> {}", bytes.len(), output.display());

    // 6. Self-check: reads back in the same format to confirm round-trip.
    let mut check_reader = ByteReader::from(bytes.as_slice());
    let roundtrip = NbtReader::from_reader(&mut check_reader)
        .read::<JavaLocalNbt>()
        .expect("read-back check failed");
    if let NbtValue::Compound(c) = &roundtrip {
        if let Some(NbtValue::List(list)) = c.get("blocks") {
            eprintln!("self-check passed: read back {} blocks", list.len());
        }
    }
    eprintln!("done. Place the file at definitions/block_palette.nbt in the version pack");
}

/// `split-blocks` subcommand: legacy palette to one `.block.json` per block (vanilla structure).
///
/// Usage: `split-blocks <block_palette.nbt> <output dir> [--defaults defaults.json] [--hardness hardness.json]`.
/// - Input palettes accept gzip or raw NBT (detected by magic);
/// - `--defaults` is `{identifier: split state key}` JSON (authoritative defaults);
/// - `--hardness` is `{identifier: {"hardness": n} | {"unbreakable": true}}`;
/// - Multi-state blocks missing defaults print a `TODO_DEFAULT <identifier>` list and exit 1,
///   writing no half-finished files and never substituting the first state;
/// - Output uses the vanilla envelope (`format_version` plus `minecraft:block`:
///   `description` / `components` / `permutations`)+ `sc:default_state` /
///   `sc:protocol_runtime_ids`; undeclared components are never written (nothing guessed).
/// - Block tags no longer live in block files (the vanilla structure has no slot for them; tags still come
///   from the version-pack `definitions/block_tags.json`).
fn split_blocks(args: &[String]) {
    use std::collections::HashMap;
    use sc_packloader::block::{
        parse_hardness_json, parse_legacy_palette_entries, split_legacy_palette,
        BlockBundleBudgets, SplitError,
    };

    if args.len() < 3 {
        eprintln!("usage: split-blocks <block_palette.nbt> <output dir> [--defaults defaults.json] [--hardness hardness.json]");
        std::process::exit(2);
    }
    let input = Path::new(&args[1]);
    let out_dir = Path::new(&args[2]);
    let mut defaults_path: Option<&str> = None;
    let mut hardness_path: Option<&str> = None;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--defaults" => {
                i += 1;
                defaults_path = args.get(i).map(String::as_str);
            }
            "--hardness" => {
                i += 1;
                hardness_path = args.get(i).map(String::as_str);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let raw = fs::read(input).unwrap_or_else(|e| {
        eprintln!("failed to read input {}: {e}", input.display());
        std::process::exit(1);
    });
    // gzip magic 1f 8b means decompress, otherwise handles raw NBT.
    let nbt_bytes = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut decompressed = Vec::new();
        GzDecoder::new(&raw[..])
            .read_to_end(&mut decompressed)
            .unwrap_or_else(|e| {
                eprintln!("gzip decompress failed {}: {e}", input.display());
                std::process::exit(1);
            });
        decompressed
    } else {
        raw
    };
    let entries = parse_legacy_palette_entries(&nbt_bytes).unwrap_or_else(|e| {
        eprintln!("legacy palette parse failed: {e}");
        std::process::exit(1);
    });
    eprintln!("read {} legacy states", entries.len());

    let defaults: HashMap<String, String> = match defaults_path {
        None => HashMap::new(),
        Some(p) => {
            let bytes = fs::read(p).unwrap_or_else(|e| {
                eprintln!("failed to read defaults {p}: {e}");
                std::process::exit(1);
            });
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("defaults JSON parse failed {p}: {e}");
                std::process::exit(1);
            })
        }
    };
    let hardness = match hardness_path {
        None => HashMap::new(),
        Some(p) => {
            let bytes = fs::read(p).unwrap_or_else(|e| {
                eprintln!("failed to read hardness {p}: {e}");
                std::process::exit(1);
            });
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("hardness JSON parse failed {p}: {e}");
                std::process::exit(1);
            });
            parse_hardness_json(&value).unwrap_or_else(|e| {
                eprintln!("hardness semantics invalid {p}: {e}");
                std::process::exit(1);
            })
        }
    };

    let budgets = BlockBundleBudgets::default();
    let files = match split_legacy_palette(&entries, &defaults, &hardness, &budgets) {
        Ok(files) => files,
        Err(SplitError::MissingDefaults(ids)) => {
            for id in ids.iter() {
                println!("TODO_DEFAULT {id}");
            }
            eprintln!(
                "{} states lack authoritative defaults (multi-state defaults are never guessed; rerun with --defaults)",
                ids.len()
            );
            std::process::exit(1);
        }
        Err(SplitError::Invalid(m)) => {
            eprintln!("split failed: {m}");
            std::process::exit(1);
        }
    };
    for file in files.iter() {
        let dest = out_dir.join(&file.zip_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|e| {
                eprintln!("failed to create dir {}: {e}", parent.display());
                std::process::exit(1);
            });
        }
        fs::write(&dest, &file.bytes).unwrap_or_else(|e| {
            eprintln!("failed to write {}: {e}", dest.display());
            std::process::exit(1);
        });
    }
    eprintln!(
        "split done: {} files -> {} (stock envelope + sc:default_state/sc:protocol_runtime_ids; undeclared components stay undeclared)",
        files.len(),
        out_dir.display()
    );
}
