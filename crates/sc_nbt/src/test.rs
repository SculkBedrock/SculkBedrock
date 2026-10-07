use crate::network::BedrockNetworkNbt;
use crate::reader::NbtReader;
use std::io::Read;
use sc_binary::ByteReader;

#[test]
#[ignore = "Manual debug helper: depends on a local entity_identifiers.dat; run with cargo test -- --ignored"]
fn test() {
    let mut file =
        std::fs::File::open("C:\\Users\\NiuBi\\Downloads\\entity_identifiers.dat").unwrap();
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).unwrap();
    let mut reader = ByteReader::from(buf);
    let mut reader = NbtReader::from_reader(&mut reader);
    let compound = reader.read::<BedrockNetworkNbt>().unwrap();
    println!("{:#?}", compound);

    // Convert the NBT tree to JSON and write it to a file.
    // NbtValue derives Serialize (untagged) and CompoundNbt serializes as a map,
    // so serde_json::to_string_pretty converts the whole tree directly.
    let json = serde_json::to_string_pretty(&compound).unwrap();
    std::fs::write("C:\\Users\\NiuBi\\Downloads\\entity_identifiers.json", json).unwrap();
}
