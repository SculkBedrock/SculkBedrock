use crate::{MinecraftJson, MinecraftJsonDeserializer};
use sc_nbt::writer::NbtCustomWrite;
use std::io::Read;
#[test]
#[ignore = "手动调试用：依赖本机 Desktop 下的 taiga.biome.json，cargo test -- --ignored 手动运行"]
fn test_biome() {
    // Read the biome file
    let mut file = std::fs::File::open("C:\\Users\\NiuBi\\Desktop\\taiga.biome.json").unwrap();
    let mut file_string = String::new();
    file.read_to_string(&mut file_string).unwrap();
    let json = serde_json::from_str::<MinecraftJsonDeserializer>(&file_string).unwrap();
    let json = json.get();
    match json {
        MinecraftJson::MinecraftBiomeSpawner(biome) => {
            println!("{:#?}", biome);
            println!(
                "{:#?}",
                biome.components.as_ref().unwrap().to_nbt().unwrap()
            );
        }
        _ => {}
    }
}
