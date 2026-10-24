//! Comprehensive unit tests for the block_entity subsystem.

use super::*;
use sc_block::workstation::SlotRole;
use sc_ecs::world::World;
use sc_recipe::StationKind;
use sc_world::manager::MinecraftWorldId;

/// Minimal world: one chunk with a furnace-kind block at (0, 64, 0),
/// registries published from a synthetic bundle (same fixture shape as
/// the furnace `tick_bundle`, property-less single state).
fn fixture() -> (World, MinecraftWorldId, u32) {
    use sc_block::block_json::{compile_bundle, BlockJsonRegistry};
    use sc_packloader::block::{
        fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
    };
    use sc_world::chunk::ChunkPosition;
    use sc_world::storage::{ChunkKey, EmptyWorldStorage, WorldChunkProvider};

    let air = r#"{
        "format_version": "1.10.0",
        "minecraft:block": {
            "description": {"identifier": "minecraft:air", "states": {}},
            "components": {},
            "sc:protocol_runtime_ids": [0]
        }
    }"#;
    let furnace = r#"{
        "format_version": "1.10.0",
        "minecraft:block": {
            "description": {"identifier": "minecraft:furnace", "states": {}},
            "components": {
                "sc:furnace": {
                    "kind": "furnace",
                    "cook_ticks": 200,
                    "fuel": true,
                    "lit": false
                }
            },
            "sc:protocol_runtime_ids": [50]
        }
    }"#;
    let raws: Vec<(&str, &str)> = vec![
        ("blocks/minecraft/air.json", air),
        ("blocks/minecraft/furnace.json", furnace),
    ];
    let budgets = BlockBundleBudgets::default();
    let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
    refs.sort_by(|a, b| a.0.cmp(b.0));
    let files = refs
        .iter()
        .map(|(p, b)| parse_block_file("test", p, b).expect("夹具应合法"))
        .collect();
    let bundle = BlockJsonBundle {
        schema_version: 1,
        network_id_mode: "hashed".to_string(),
        fingerprint: fingerprint_bundle(&refs, "hashed"),
        files,
        loot_tables: Default::default(),
    };
    let (snapshot, _) =
        compile_bundle(&bundle, "test", &budgets, &|_| None, &|_| true).expect("bundle 应编译");
    let blocks = BlockStateRegistry::from_block_snapshot(&snapshot);
    let furnace_hash = blocks
        .runtime_id(
            blocks
                .state_of("minecraft:furnace", &[])
                .expect("熔炉应在 bundle 中"),
        )
        .expect("runtime id")
        .0;

    let world = World::new();
    let provider = WorldChunkProvider::new(std::sync::Arc::new(EmptyWorldStorage));
    let column = provider
        .insert_empty_chunk(ChunkKey::new(0, ChunkPosition::new(0, 0)), -64, 319)
        .unwrap();
    column
        .write()
        .set_block_at(0, 0, 64, 0, BlockRuntimeId(furnace_hash));
    let mut manager = sc_world::manager::MinecraftWorldManager::new();
    let world_id = manager.push_world(
        sc_utils::world::r#type::WorldType::Overworld,
        sc_world::world::MinecraftWorld::new(
            MinecraftWorldId::random(),
            "test".into(),
            std::path::PathBuf::new(),
            sc_utils::world::data::MinecraftWorldData {
                world_type: Some(sc_utils::world::r#type::WorldType::Overworld),
                ..Default::default()
            },
            provider,
        ),
    );
    world.insert_resource(manager);
    world.insert_resource(blocks);
    let json = BlockJsonRegistry::new();
    json.publish(std::sync::Arc::new(snapshot));
    world.insert_resource(json);
    world.insert_resource(BlockEntityIndex::default());
    (world, world_id, furnace_hash)
}

#[test]
fn ensure_is_idempotent_and_position_scoped() {
    let (world, world_id, _) = fixture();
    let kind = BlockEntityKind::Furnace(StationKind::Furnace);
    let first =
        ensure_block_entity(&world, world_id.clone(), (0, 64, 0), kind).expect("应创建");
    let second = ensure_block_entity(&world, world_id.clone(), (0, 64, 0), kind)
        .expect("重复 ensure 幂等");
    assert_eq!(first, second);
    // Identity component carries world/pos/kind.
    let reference = world
        .get_component::<BlockEntityRef>(&first)
        .expect("应挂载身份");
    assert_eq!(reference.pos, (0, 64, 0));
    assert_eq!(reference.kind, kind);
    assert_eq!(reference.world, world_id);
    // Mounted process state is empty, furnace-flavored, and decl-fed.
    let process = world
        .get_component::<crate::inventory::crafting::BlockProcessState>(&first)
        .expect("应挂载空 process");
    assert_eq!(process.kind(), StationKind::Furnace);
    assert!(!process.is_dirty());
    assert_eq!(process.cook_total(), 200);
    let snapshot = process.snapshot();
    let roles: Vec<SlotRole> = snapshot.slots.iter().map(|(role, _)| *role).collect();
    assert_eq!(
        roles,
        vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result]
    );
    assert!(snapshot.slots.iter().all(|(_, slot)| slot.is_empty()));
}

#[test]
fn ensure_fails_closed_without_index_or_process() {
    let (world, world_id, _) = fixture();
    let kind = BlockEntityKind::Furnace(StationKind::Furnace);
    // No index resource: refuse, never create.
    let bare = World::new();
    assert!(ensure_block_entity(&bare, world_id.clone(), (0, 64, 0), kind).is_none());
    // Air (no process decl) at an undeclared cell: refuse.
    assert!(ensure_block_entity(&world, world_id.clone(), (5, 64, 5), kind).is_none());
    // Nothing was tracked by the refusals.
    let index = world.get_resource::<BlockEntityIndex>().expect("index");
    assert_eq!(index.count_for(&world_id), 0);
}

#[test]
fn index_bound_refuses_new_entities_and_keeps_old() {
    let (world, world_id, _) = fixture();
    let kind = BlockEntityKind::Furnace(StationKind::Furnace);
    let live = ensure_block_entity(&world, world_id.clone(), (0, 64, 0), kind).expect("应创建");
    // Fill the world partition directly (spawn needs no registry).
    {
        let mut index = world.get_resource_mut::<BlockEntityIndex>().expect("index");
        for i in 1..BlockEntityIndex::MAX_PER_WORLD as i32 {
            let entity = world.spawn((
                BlockEntityRef::new(world_id.clone(), (i, 64, 0), kind),
                crate::inventory::crafting::BlockProcessState::new(
                    StationKind::Furnace,
                    vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result],
                    200,
                ),
            ));
            index.insert(world_id.clone(), (i, 64, 0), entity);
        }
    }
    assert!(ensure_block_entity(&world, world_id.clone(), (9_999, 64, 0), kind).is_none());
    // Old entities are unaffected (repeat ensure still idempotent).
    assert_eq!(
        ensure_block_entity(&world, world_id.clone(), (0, 64, 0), kind),
        Some(live)
    );
}

#[test]
fn destroy_removes_identity_and_rebuild_creates_new_entity() {
    let (world, world_id, _) = fixture();
    let kind = BlockEntityKind::Furnace(StationKind::Furnace);
    let first =
        ensure_block_entity(&world, world_id.clone(), (0, 64, 0), kind).expect("应创建");
    assert_eq!(
        destroy_block_entity(&world, &world_id, (0, 64, 0)),
        Some(first)
    );
    // Components went with the entity.
    assert!(world.get_component::<BlockEntityRef>(&first).is_none());
    // Destroying again is None (idempotent cleanup).
    assert!(destroy_block_entity(&world, &world_id, (0, 64, 0)).is_none());
    // Rebuilding mints a fresh entity (ids are never reused).
    let second =
        ensure_block_entity(&world, world_id.clone(), (0, 64, 0), kind).expect("应重建");
    assert_ne!(first, second);
}

#[test]
fn take_break_contents_returns_slots_and_destroys() {
    let world = World::new();
    world.insert_resource(BlockEntityIndex::default());
    let missing = MinecraftWorldId::random();
    assert!(take_break_contents(&world, &missing, (0, 64, 0)).is_none());
    let world_id = MinecraftWorldId::random();
    let kind = BlockEntityKind::Furnace(StationKind::Furnace);
    let entity = world.spawn((
        BlockEntityRef::new(world_id.clone(), (0, 64, 0), kind),
        crate::inventory::crafting::BlockProcessState::new(
            StationKind::Furnace,
            vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result],
            200,
        ),
    ));
    let process = world
        .get_component::<crate::inventory::crafting::BlockProcessState>(&entity)
        .expect("刚挂载");
    process.set_slot(SlotRole::Input, sc_item::ItemStack::new(4, 2));
    process.set_slot(SlotRole::Result, sc_item::ItemStack::new(5, 1));
    world
        .get_resource_mut::<BlockEntityIndex>()
        .expect("index")
        .insert(world_id.clone(), (0, 64, 0), entity);
    let contents = take_break_contents(&world, &world_id, (0, 64, 0)).expect("应取出");
    assert_eq!(
        contents,
        vec![sc_item::ItemStack::new(4, 2), sc_item::ItemStack::new(5, 1),]
    );
    assert!(world.get_component::<BlockEntityRef>(&entity).is_none());
    assert!(take_break_contents(&world, &world_id, (0, 64, 0)).is_none());
}

#[test]
fn take_break_contents_supports_generic_block_inventory() {
    let world = World::new();
    world.insert_resource(BlockEntityIndex::default());
    let world_id = MinecraftWorldId::random();
    let mut inv = BlockInventory::new(27);
    inv.set(0, sc_item::ItemStack::new(264, 10)); // 10 diamonds
    inv.set(4, sc_item::ItemStack::new(265, 5));  // 5 iron ingots

    let entity = world.spawn((
        BlockEntityRef::new(world_id.clone(), (10, 64, 10), BlockEntityKind::Chest),
        inv,
    ));
    world
        .get_resource_mut::<BlockEntityIndex>()
        .expect("index")
        .insert(world_id.clone(), (10, 64, 10), entity);

    let contents = take_break_contents(&world, &world_id, (10, 64, 10)).expect("应取出箱子掉落");
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0], sc_item::ItemStack::new(264, 10));
    assert_eq!(contents[1], sc_item::ItemStack::new(265, 5));
    assert!(world.get_component::<BlockEntityRef>(&entity).is_none());
}

#[test]
fn index_keys_are_world_scoped() {
    let world = World::new();
    world.insert_resource(BlockEntityIndex::default());
    let kind = BlockEntityKind::Furnace(StationKind::Furnace);
    let first = MinecraftWorldId::random();
    let second = MinecraftWorldId::random();
    let entity = world.spawn((
        BlockEntityRef::new(first.clone(), (0, 64, 0), kind),
        crate::inventory::crafting::BlockProcessState::new(
            StationKind::Furnace,
            vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result],
            200,
        ),
    ));
    world
        .get_resource_mut::<BlockEntityIndex>()
        .expect("index")
        .insert(first.clone(), (0, 64, 0), entity);
    let index = world.get_resource::<BlockEntityIndex>().expect("index");
    assert_eq!(index.get(&first, (0, 64, 0)), Some(entity));
    assert_eq!(index.get(&second, (0, 64, 0)), None);
    assert_eq!(index.count_for(&first), 1);
    assert_eq!(index.count_for(&second), 0);
    drop(index);
    // Destroy is world-scoped too.
    assert!(destroy_block_entity(&world, &second, (0, 64, 0)).is_none());
    assert_eq!(
        destroy_block_entity(&world, &first, (0, 64, 0)),
        Some(entity)
    );
    assert!(world.get_component::<BlockEntityRef>(&entity).is_none());
}

#[test]
fn world_ext_syntax_sugar_operates_cleanly() {
    let world = World::new();
    world.insert_resource(BlockEntityIndex::default());
    let world_id = MinecraftWorldId::random();
    let pos = (12, 64, -5);

    let entity = world.spawn((
        BlockEntityRef::new(world_id.clone(), pos, BlockEntityKind::Chest),
        BlockCustomName::new("Old Vault"),
        BlockInventory::new(27),
    ));
    world
        .get_resource_mut::<BlockEntityIndex>()
        .unwrap()
        .insert(world_id.clone(), pos, entity);

    // 1. Query by coordinates using sugar
    assert_eq!(world.block_entity_at(&world_id, pos), Some(entity));

    // 2. Query identity ref
    let reference = world.block_entity_ref(&entity).expect("identity exists");
    assert_eq!(reference.kind, BlockEntityKind::Chest);

    // 3. Mutate component with automatic dirty flag attachment
    let changed = world.mutate_block_entity(&world_id, pos, |name: &mut BlockCustomName| {
        name.0 = "New Vault".to_string();
        true
    });
    assert_eq!(changed, Some(true));

    let updated_name = world.get_component::<BlockCustomName>(&entity).unwrap();
    assert_eq!(updated_name.0, "New Vault");

    let dirty = world.get_component::<BlockEntityDirty>(&entity).unwrap();
    assert!(dirty.save_dirty);
    assert!(dirty.network_dirty);
}
