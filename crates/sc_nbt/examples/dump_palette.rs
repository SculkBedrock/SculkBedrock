//! One-shot tool: parse a pack block_palette.nbt (big-endian Java-style NBT).
use std::io::Read as _;
use sc_binary::ByteReader;

fn read_string(r: &mut ByteReader) -> String {
    let len = r.read_u16().unwrap() as usize;
    String::from_utf8(r.read_bytes(len).unwrap().to_vec()).unwrap()
}

fn read_compound_fields(r: &mut ByteReader) -> Vec<(String, String)> {
    let mut out = Vec::new();
    loop {
        let tag = r.read_u8().unwrap();
        if tag == 0 {
            break;
        }
        let k = read_string(r);
        let v = read_value(r, tag);
        out.push((k, v));
    }
    out
}

fn read_value(r: &mut ByteReader, tag: u8) -> String {
    if tag == 10 {
        return format!("compound{{{:?}}}", read_compound_fields(r));
    }
    match tag {
        1 => format!("byte:{}", r.read_i8().unwrap()),
        2 => format!("short:{}", r.read_i16().unwrap()),
        3 => format!("int:{}", r.read_i32().unwrap()),
        4 => format!("long:{}", r.read_i64().unwrap()),
        5 => format!("float:{}", r.read_f32().unwrap()),
        6 => format!("double:{}", r.read_f64().unwrap()),
        8 => format!("str:{:?}", read_string(r)),
        _ => format!("tag{tag}"),
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let filter = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "grass_block".into());
    let mut f = std::fs::File::open(&path).unwrap();
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).unwrap();
    let mut r = ByteReader::from(bytes.as_slice());
    assert_eq!(r.read_u8().unwrap(), 10, "root tag");
    assert_eq!(r.read_u16().unwrap(), 0, "root name len");
    assert_eq!(r.read_u8().unwrap(), 9, "blocks list tag");
    let key = read_string(&mut r);
    println!("root key: {key}");
    assert_eq!(r.read_u8().unwrap(), 10, "list elem tag");
    let len = r.read_i32().unwrap();
    println!("list len: {len}");
    let mut found = 0;
    for i in 0..len {
        // Parse one entry compound.
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut name = String::new();
        loop {
            let tag = r.read_u8().unwrap();
            if tag == 0 {
                break;
            }
            let k = read_string(&mut r);
            let v = read_value(&mut r, tag);
            if k == "name" {
                name = v.replace("str:", "").replace('"', "");
            }
            if k == "name_hash"
                || k == "network_id"
                || k == "block_id"
                || k == "version"
                || k == "states"
            {
                fields.push((k, v));
            }
        }
        if name.contains(&filter) {
            println!("=== {name} ===");
            for (k, v) in &fields {
                println!("  {k} = {v}");
            }
            found += 1;
            if found >= 3 {
                break;
            }
        }
        if i % 3000 == 0 {
            eprintln!("progress {i}");
        }
    }
    println!("found={found}");
}
