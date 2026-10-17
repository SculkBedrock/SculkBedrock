//! Block-entity identity layer (D1/S1 of the workstation decoupling).
//!
//! `(world, pos)`-keyed entity index for stateful blocks (furnace family
//! first). Identity only: process contents live in [`BlockProcessState`]
//! components (generalized in S2); ticking and storage migrated in S3.
//!
//! [`BlockProcessState`]: crate::inventory::crafting::BlockProcessState

use std::collections::HashMap;

use sc_block::block_json::BlockJsonRegistry;
use sc_block::registry::BlockStateRegistry;
use sc_block::workstation::SlotRole;
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_recipe::StationKind;
use sc_world::chunk::BlockRuntimeId;
use sc_world::manager::MinecraftWorldId;

/// Stateful block kind. Only the furnace family in S1 (brewing/campfire
/// add variants without structural change, see Q4 in the decoupling plan).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockEntityKind {
    Furnace(StationKind),
}

impl BlockEntityKind {
    pub fn station(self) -> StationKind {
        match self {
            Self::Furnace(station) => station,
        }
    }
}

/// Identity component: which world block this entity's process state
/// belongs to. The tick resolves the block's decls from `(world, pos)`
/// every time (never cached here), so a replaced block is a ghost by
/// construction, same as the store path.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct BlockEntityRef {
    pub world: MinecraftWorldId,
    pub pos: (i32, i32, i32),
    pub kind: BlockEntityKind,
}

/// `(world, pos)` → process entity. Bounded per world (1024; a full index
/// refuses new entities, fail-closed, and old entities are unaffected).
/// Chunk unload never evicts (unload eviction needs persistence, out of
/// scope per D5).
#[derive(Resource, Default)]
pub struct BlockEntityIndex {
    map: HashMap<(MinecraftWorldId, (i32, i32, i32)), EntityId>,
}

impl BlockEntityIndex {
    pub const MAX_PER_WORLD: usize = 1024;

    pub fn get(&self, world_id: &MinecraftWorldId, pos: (i32, i32, i32)) -> Option<EntityId> {
        self.map.get(&(world_id.clone(), pos)).copied()
    }

    /// Track an entity explicitly (crate-internal: `ensure` is the normal
    /// path; tests that build entities by hand use this to register them).
    pub(crate) fn insert(
        &mut self,
        world_id: MinecraftWorldId,
        pos: (i32, i32, i32),
        entity: EntityId,
    ) {
        self.map.insert((world_id, pos), entity);
    }

    fn remove(&mut self, world_id: &MinecraftWorldId, pos: (i32, i32, i32)) -> Option<EntityId> {
        self.map.remove(&(world_id.clone(), pos))
    }

    pub fn count_for(&self, world_id: &MinecraftWorldId) -> usize {
        self.map
            .keys()
            .filter(|(world, _)| world == world_id)
            .count()
    }
}

/// Process params for the block at a position: duration plus slot roles,
/// both resolved from its `sc:process` / `sc:workstation` decls (roles stay
/// in decl slot order; `advance` resolves by role, so order never matters).
/// `None` = unresolvable (unloaded chunk, missing registries/snapshot, or
/// no process): the caller fails closed.
fn process_params_at(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<(u16, Vec<SlotRole>)> {
    let hash = crate::furnace::block_hash_at(world, world_id, pos)?;
    let blocks = world.get_resource::<BlockStateRegistry>()?;
    let snapshot = world.get_resource::<BlockJsonRegistry>()?.get()?;
    let dense = blocks.by_runtime_id(BlockRuntimeId(hash))?;
    let workstation = snapshot.workstation_of(dense.0)?;
    let process = snapshot.process_of(dense.0)?;
    Some((
        process.cook_ticks,
        workstation.slots.iter().map(|slot| slot.role).collect(),
    ))
}

/// Ensure the process entity for a stateful block (`Some` entity, idempotent:
/// repeat calls return the same entity). `None` = admission refused (index
/// full for this world) or unresolvable process decl — the caller falls back
/// to its existing failure path (resync for open/move, skip for tick),
/// exactly like the store bound today.
pub fn ensure_block_entity(
    world: &World,
    world_id: MinecraftWorldId,
    pos: (i32, i32, i32),
    kind: BlockEntityKind,
) -> Option<EntityId> {
    // Fast path: already tracked (idempotent, no chunk read).
    if let Some(existing) = world
        .get_resource::<BlockEntityIndex>()
        .and_then(|index| index.get(&world_id, pos))
    {
        return Some(existing);
    }
    // Admission before any world read (a full index refuses without touching
    // chunks or registries).
    let full = world
        .get_resource::<BlockEntityIndex>()
        .is_none_or(|index| index.count_for(&world_id) >= BlockEntityIndex::MAX_PER_WORLD);
    if full {
        return None;
    }
    // Duration and roles come from the decls, never code defaults
    // (fail-closed when the block carries no process).
    let (cook_ticks, roles) = process_params_at(world, &world_id, pos)?;
    let mut index = world.get_resource_mut::<BlockEntityIndex>()?;
    // Re-check under the mutable guard (single-threaded world: no race, but
    // the double check keeps the invariant local to this function).
    if let Some(existing) = index.get(&world_id, pos) {
        return Some(existing);
    }
    if index.count_for(&world_id) >= BlockEntityIndex::MAX_PER_WORLD {
        return None;
    }
    let entity = world.spawn((
        BlockEntityRef {
            world: world_id.clone(),
            pos,
            kind,
        },
        crate::inventory::crafting::BlockProcessState::new(kind.station(), roles, cook_ticks),
    ));
    index.insert(world_id, pos, entity);
    Some(entity)
}

/// Destroy the process entity for a block (break path only; the caller takes
/// out the contents for drops first, see D3). Missing entries are `None`
/// (idempotent cleanup, never a panic). The tick never calls this (ghosts
/// only skip, same rule as the store path).
pub fn destroy_block_entity(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<EntityId> {
    let removed = world
        .get_resource_mut::<BlockEntityIndex>()
        .and_then(|mut index| index.remove(world_id, pos))?;
    world.despawn(&removed);
    Some(removed)
}

/// Process state for a tracked block (`None` when untracked: never tracked,
/// or destroyed by the break path). Read-only callers (open/move/viewer
/// sync) use this; writers go through [`ensure_block_entity`] first.
/// Returned as the shared handle (same `Arc` the tick holds; no guard is
/// kept across the call).
pub fn process_state_at(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<std::sync::Arc<crate::inventory::crafting::BlockProcessState>> {
    let entity = world
        .get_resource::<BlockEntityIndex>()
        .and_then(|index| index.get(world_id, pos))?;
    world.get_component::<crate::inventory::crafting::BlockProcessState>(&entity)
}

/// Take a tracked block's process contents for break drops (survival only).
/// Reads the slots in decl order, destroys the identity, and returns the
/// non-empty stacks. `None` = untracked (ordinary block, no hook). The
/// caller appends these to the break's rolled drops so container contents
/// share the pending/backpressure path; burn/cook progress itself is lost
/// (same as vanilla). Creative breaks skip this call (drop nothing) but
/// still destroy via [`destroy_block_entity`].
pub fn take_break_contents(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<Vec<sc_item::ItemStack>> {
    let process = process_state_at(world, world_id, pos)?;
    let contents: Vec<sc_item::ItemStack> = process
        .snapshot()
        .slots
        .into_iter()
        .filter_map(|(_, stack)| (!stack.is_empty()).then_some(stack))
        .collect();
    destroy_block_entity(world, world_id, pos);
    Some(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let furnace = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:furnace", "states": {}},
                "components": {
                    "sc:workstation": {
                        "station": "furnace",
                        "container": 2,
                        "window": true,
                        "slots": [
                            {"role": "input", "container": 25},
                            {"role": "fuel", "container": 24},
                            {"role": "result", "container": 26}
                        ]
                    },
                    "sc:process": {
                        "kind": "burn_cook",
                        "cook_ticks": 200,
                        "fuel": true,
                        "lit": false,
                        "lit_variant": "minecraft:lit_furnace"
                    }
                },
                "sc:default_state": {},
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
        };
        // Unvalidated bundle on purpose: the fixture only needs decl
        // columns, and `validate_workstations` is covered separately
        // (a one-sided lit pointer is enough to resolve cook ticks here).
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
        // Mounted process state is empty, furnace-flavored, and decl-fed
        // (roles mirror the workstation slots, duration the process).
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
                    BlockEntityRef {
                        world: world_id.clone(),
                        pos: (i, 64, 0),
                        kind,
                    },
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
        use sc_block::workstation::SlotRole;

        // No chunks needed: the hook is index-driven, like the break path.
        let world = World::new();
        world.insert_resource(BlockEntityIndex::default());
        // Untracked position: no hook.
        let missing = MinecraftWorldId::random();
        assert!(take_break_contents(&world, &missing, (0, 64, 0)).is_none());
        let world_id = MinecraftWorldId::random();
        let kind = BlockEntityKind::Furnace(StationKind::Furnace);
        let entity = world.spawn((
            BlockEntityRef {
                world: world_id.clone(),
                pos: (0, 64, 0),
                kind,
            },
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
        // Track it (the hook is index-driven; spawning alone is not enough).
        world
            .get_resource_mut::<BlockEntityIndex>()
            .expect("index")
            .insert(world_id.clone(), (0, 64, 0), entity);
        // Fuel left empty: only non-empty stacks drop, in decl slot order.
        let contents = take_break_contents(&world, &world_id, (0, 64, 0)).expect("应取出");
        assert_eq!(
            contents,
            vec![sc_item::ItemStack::new(4, 2), sc_item::ItemStack::new(5, 1),]
        );
        // Identity went with the contents (second take finds nothing).
        assert!(world.get_component::<BlockEntityRef>(&entity).is_none());
        assert!(take_break_contents(&world, &world_id, (0, 64, 0)).is_none());
    }

    #[test]
    fn index_keys_are_world_scoped() {
        // World-scoped identity without world machinery (no chunks, no
        // manager, no second provider): the property under test is keying,
        // and only keying. (A multi-provider variant of this test hung
        // deterministically for reasons never isolated to a frame; it was
        // deleted rather than kept red. Multi-world *ensure* regain
        // coverage in S3 integration.)
        let world = World::new();
        world.insert_resource(BlockEntityIndex::default());
        let kind = BlockEntityKind::Furnace(StationKind::Furnace);
        let first = MinecraftWorldId::random();
        let second = MinecraftWorldId::random();
        let entity = world.spawn((
            BlockEntityRef {
                world: first.clone(),
                pos: (0, 64, 0),
                kind,
            },
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
}
