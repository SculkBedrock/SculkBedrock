//! 物品 NBT 显示字段（名称、描述行）的写入辅助。
//!
//! Helpers for writing item NBT display fields (name, lore).

use crate::item::MinecraftItemSpawner;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;

/// 物品 NBT 写入辅助（仅关联函数，无状态）。
///
/// Item NBT writing helpers (associated functions only, stateless).
pub struct ItemNbt;

impl ItemNbt {
    /// 写入物品描述行（Lore）；`display` 存在但不是复合体时返回 `None`。
    ///
    /// Writes item lore lines; returns `None` when `display` exists but is
    /// not a compound.
    ///
    /// - `nbt`: 目标 NBT 复合体
    /// - `lore`: 描述行列表
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

    /// 写入物品显示名；名称为空时转为清除名称。
    ///
    /// Writes the item display name; an empty name clears the name instead.
    ///
    /// - `nbt`: 目标 NBT 复合体
    /// - `name`: 显示名
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

    /// 清除物品显示名；`display` 变空时一并移除。
    ///
    /// Clears the item display name; removes `display` entirely when it
    /// becomes empty.
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
    /// 物品 NBT 构建入口（预留，暂不产出内容）。
    ///
    /// Item NBT build entry point (reserved; produces nothing yet).
    pub fn build_nbt(_item: &mut MinecraftItemSpawner) {
        //let mut nbt = CompoundNbt::new(None);
    }
}
