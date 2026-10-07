use sc_binary::ByteWriter;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::local::BedrockLocalNbt;
use sc_nbt::writer::NbtWriteTrait;
use sc_nbt::writer::NbtWriter;
use sc_nbt::NbtValue;
fn main() {
    let mut c = CompoundNbt::new(Some("".into()));
    c.insert("SpawnY", NbtValue::Int(-60));
    c.insert("LevelName", NbtValue::String("我的世界".into()));
    c.insert("flag", NbtValue::Byte(1));
    let root = NbtValue::Compound(c);
    let mut w = ByteWriter::new();
    let mut nw = NbtWriter::from_writer(&mut w);
    nw.write::<BedrockLocalNbt>(&root).unwrap();
    let b = w.as_slice();
    println!("{} bytes: {:02x?}", b.len(), &b[..b.len().min(40)]);
    // Read back to verify.
    use sc_binary::ByteReader;
    use sc_nbt::reader::NbtReadTrait;
    let mut r = ByteReader::from(b);
    match BedrockLocalNbt::read(&mut r) {
        Ok(v) => println!("readback OK: {v:?}"),
        Err(e) => println!("readback ERR: {e}"),
    }
}
