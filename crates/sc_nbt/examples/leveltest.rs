// Read a backed-up level.dat and check which reader decodes it: BedrockLocalNbt vs JavaLocalNbt.
use sc_binary::ByteReader;
use sc_nbt::local::{BedrockLocalNbt, JavaLocalNbt};
use sc_nbt::reader::NbtReadTrait;
fn main() {
    let bytes =
        std::fs::read("/workspace/SculkBedrock_backup/worlds/OverWorld/level.dat").unwrap();
    for (name, t) in [("Bedrock", 0u8), ("Java", 1u8)] {
        let mut r = ByteReader::from(&bytes[8..]);
        let v = if t == 0 {
            BedrockLocalNbt::read(&mut r)
        } else {
            JavaLocalNbt::read(&mut r)
        };
        match v {
            Ok(v) => {
                let s = format!("{v:?}");
                let levelname = s.find("LevelName").map(|i| &s[i..i + 120]);
                println!("{name} OK len={} LevelName附近: {:?}", s.len(), levelname);
            }
            Err(e) => println!("{name} ERR: {e}"),
        }
    }
}
