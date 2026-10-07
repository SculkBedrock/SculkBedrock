use crate::item::MinecraftItemSpawner;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

pub struct ItemNbt;

impl ItemNbt {
    pub fn write_lore(nbt: &mut CompoundNbt, lore: Vec<String>) -> Option<()> {
        let lore = lore
            .into_iter()
            .map(|s| NbtValue::String(s))
            .collect::<Vec<_>>();
        let lore = NbtValue::List(lore);
        if let Some(display) = nbt.get_mut("display") {
            if let Some(display) = display.as_compound_mut() {
                display.insert("Lore", lore);
                Some(())
            } else {
                None
            }
        } else {
            nbt.insert(
                "display",
                NbtValue::Compound(CompoundNbt::new_with_value(None, "Lore", lore)),
            );
            Some(())
        }
    }

    pub fn write_name(nbt: &mut CompoundNbt, name: String) -> Option<()> {
        if name.is_empty() {
            return Self::clear_name(nbt);
        }
        let name = NbtValue::String(name);
        if let Some(display) = nbt.get_mut("display") {
            if let Some(display) = display.as_compound_mut() {
                display.insert("Name", name);
                Some(())
            } else {
                None
            }
        } else {
            nbt.insert(
                "display",
                NbtValue::Compound(CompoundNbt::new_with_value(None, "Name", name)),
            );
            Some(())
        }
    }

    pub fn clear_name(nbt: &mut CompoundNbt) -> Option<()> {
        if let Some(display) = nbt.get_mut("display") {
            if let Some(display) = display.as_compound_mut() {
                display.remove("Name");
                if display.is_empty() {
                    nbt.remove("display");
                }
                Some(())
            } else {
                None
            }
        } else {
            Some(())
        }
    }
    pub fn build_nbt(_item: &mut MinecraftItemSpawner) {
        //let mut nbt = CompoundNbt::new(None);
    }
}
