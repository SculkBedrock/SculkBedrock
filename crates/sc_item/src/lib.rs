//! Authoritative server-side item model.
//!
//! This crate does not depend on the network protocol: the network layer only converts `ItemData`
//! to/from `ItemStack`, while gameplay validates transactions and commits inventory. Version packs can
//! therefore register new items without hardcoding item ids into the protocol or chunk core.

pub mod registry;
pub mod stack;
pub mod tag;

pub use registry::{ItemDefinition, ItemRegistry};
pub use stack::{ItemStack, PlayerInventory};
pub use tag::CreativeGrantTable;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_and_insert_obey_stack_limit() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(1, "minecraft:stone").max_stack_size(2));
        let inventory = PlayerInventory::new(2);
        inventory.set(0, ItemStack::new(1, 2));
        assert_eq!(inventory.reserve_one(0).unwrap().count, 2);
        assert_eq!(inventory.get(0).unwrap().count, 1);
        assert_eq!(inventory.insert(ItemStack::new(1, 4), &registry), 1);
        assert_eq!(inventory.get(0).unwrap().count, 2);
    }

    #[test]
    fn registered_block_mapping_is_data_driven() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(5, "minecraft:stone").block(123));
        assert_eq!(registry.block_runtime_id(5), Some(123));
    }

    #[test]
    fn restore_one_only_restores_a_single_reserved_item() {
        let inventory = PlayerInventory::new(1);
        inventory.set(0, ItemStack::new(1, 2));
        let reserved = inventory.reserve_one(0).unwrap();
        assert_eq!(inventory.get(0).unwrap().count, 1);
        assert!(inventory.restore_one(
            0,
            &ItemStack {
                count: 1,
                ..reserved
            }
        ));
        assert_eq!(inventory.get(0).unwrap().count, 2);
    }

    #[test]
    fn block_items_returns_only_placeable_sorted_by_runtime_id() {
        let registry = ItemRegistry::new();
        // Non-block items (e.g. swords) are excluded; block items sort by ascending runtime_id.
        registry.upsert(ItemDefinition::new(9, "minecraft:diamond_sword"));
        registry.upsert(ItemDefinition::new(7, "minecraft:dirt").block(200));
        registry.upsert(ItemDefinition::new(3, "minecraft:stone").block(100));
        let block_items = registry.block_items();
        assert_eq!(block_items.len(), 2);
        assert_eq!(block_items[0].runtime_id, 3);
        assert_eq!(block_items[1].runtime_id, 7);
        assert_eq!(block_items[0].default_block_runtime_id, Some(100));
    }

    #[test]
    fn dense_table_get_is_o1_and_holes_return_none() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(3, "minecraft:stone"));
        registry.upsert(ItemDefinition::new(257, "minecraft:apple"));
        // runtime_id is the index: direct lookup, no panic.
        assert_eq!(registry.get(3).unwrap().name.as_ref(), "minecraft:stone");
        assert_eq!(registry.get(257).unwrap().name.as_ref(), "minecraft:apple");
        // Hollow indices return None.
        assert!(registry.get(4).is_none());
        assert!(registry.get(999).is_none());
        assert_eq!(registry.len(), 2);
        assert!(!registry.is_empty());
    }

    #[test]
    fn name_lookup_tracks_registry() {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(3, "minecraft:stone"));
        registry.upsert(ItemDefinition::new(7, "minecraft:dirt"));
        assert_eq!(registry.runtime_id_by_name("minecraft:stone"), Some(3));
        assert_eq!(registry.runtime_id_by_name("minecraft:dirt"), Some(7));
        assert_eq!(registry.runtime_id_by_name("minecraft:unknown"), None);
        assert!(registry.contains_name("minecraft:stone"));
        assert!(!registry.contains_name("minecraft:air"));
        // Renames under the same runtime_id drop the old name.
        registry.upsert(ItemDefinition::new(3, "minecraft:granite"));
        assert_eq!(registry.runtime_id_by_name("minecraft:granite"), Some(3));
        assert_eq!(registry.runtime_id_by_name("minecraft:stone"), None);
    }

    #[test]
    fn register_rejects_duplicate_runtime_id() {
        let registry = ItemRegistry::new();
        assert!(registry
            .register(ItemDefinition::new(3, "minecraft:stone"))
            .is_ok());
        assert!(registry
            .register(ItemDefinition::new(3, "minecraft:granite"))
            .is_err());
    }
}
