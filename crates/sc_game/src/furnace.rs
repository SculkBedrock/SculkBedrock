//! Server-authoritative furnace state and smelting.
//!
//! One [`BlockEntityRef`] entity per lit-or-loaded furnace, tracked in the
//! [`BlockEntityIndex`] by `(world, x, y, z)` (per-world partition like
//! [`ItemDropStore`], bounded). Ticking follows the vanilla furnace update:
//! burn decrements per tick, cook advances
//! while smeltable, `readyAt` from the `sc:process` decl (furnace 200,
//! blast/smoker 100), completion moves one output into the result slot.
//!
//! Fuel durations are data, never code: [`fuel_ticks`] reads the
//! `minecraft:fuel` `duration` (seconds) component from the item component
//! table. Items without the component burn 0 ticks.
//!
//! [`BlockEntityIndex`]: crate::block_entity::BlockEntityIndex
//! [`BlockEntityRef`]: crate::block_entity::BlockEntityRef
//! [`ItemDropStore`]: crate::item::ItemDropStore

use std::collections::HashMap;
use std::sync::Arc;

use sc_block::block_json::BlockJsonRegistry;
use sc_block::position::BlockPosition;
use sc_block::registry::BlockStateRegistry;
use sc_block::state::BlockPropertyValue;
use sc_block::workstation::SlotRole;
use sc_block::write::{update_flags, BlockChange, BlockChangeCause, BlockChangeQueue};
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::world::World;
use sc_entity::MinecraftEntityId;
use sc_item::{ItemRegistry, ItemStack};
use sc_packloader::item::ItemComponentTable;
use sc_recipe::{CompiledRecipe, MatchInput, RecipeBody, RecipeRegistrySnapshot, StationKind};
use sc_world::manager::MinecraftWorldId;

use crate::block_entity::BlockEntityRef;
use crate::inventory::crafting::{BlockProcessState, SharedItemTags, SharedRecipeRegistry};
use crate::inventory::ContainerOpen;
use crate::net::{IntentPublisher, NetworkOutbox, SyncFurnaceProgress, SyncFurnaceWindow};

/// Furnace window slots (Bedrock order): 0=input, 1=fuel, 2=result.
/// Slot indices, not container enums: the container bytes live in the
/// workstation decl slots (outbound) and the protocol vocabulary table
/// (inbound `translate_inventory_action`, permissive union over all
/// furnace stations).
pub const INPUT_SLOT: u8 = 0;
pub const FUEL_SLOT: u8 = 1;
pub const RESULT_SLOT: u8 = 2;
pub const SLOT_COUNT: usize = 3;

/// Whether the station opens a furnace-style window.
pub fn is_furnace_station(station: StationKind) -> bool {
    matches!(
        station,
        StationKind::Furnace | StationKind::BlastFurnace | StationKind::Smoker
    )
}

/// What one tick changed (drives lit-swap + viewer resync).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickOutcome {
    pub consumed_fuel: bool,
    pub completed: bool,
    /// Lit edge in either direction (swap block + resync viewers).
    pub lit_changed: bool,
}

/// Burn ticks for one fuel item (0 = not fuel).
///
/// Pure data lookup: `minecraft:fuel` `duration` (seconds) from the item
/// component table, `round(duration * 20)` ticks (fuel-time scaling
/// semantics). Items without the component burn 0 ticks; no durations live
/// in code. Fuel data comes from operator behavior-pack item JSONs through
/// the existing overlay pipeline (no shipped defaults).
pub fn fuel_ticks(components: &sc_packloader::item::ItemComponentTable, stack: &ItemStack) -> u32 {
    use sc_packloader::item::component::Fuel;
    if stack.is_empty() {
        return 0;
    }
    components
        .get::<Fuel>(stack.runtime_id)
        .map(|fuel| (fuel.duration * 20.0).round().max(0.0) as u32)
        .unwrap_or(0)
}

/// Current smelt output for `input` at `station` (`None` = nothing
/// smeltable). Mirrors the grid-crafting query path (`recipes_for_station`
/// + `match_process_input`).
pub fn smelt_output(
    recipes: &RecipeRegistrySnapshot,
    station: StationKind,
    input: &ItemStack,
    items: &ItemRegistry,
    tags: &dyn sc_recipe::TagResolver,
) -> Option<ItemStack> {
    if input.is_empty() {
        return None;
    }
    let def = items.get(input.runtime_id)?;
    let query = MatchInput::new(def.name.to_string(), input.damage as i32, input.count);
    let recipe: &CompiledRecipe = recipes
        .recipes_for_station(station)
        .into_iter()
        .filter(|r| matches!(r.body, RecipeBody::Furnace(_)))
        .find(|r| match &r.body {
            RecipeBody::Furnace(body) => {
                sc_recipe::match_process_input(&body.input, &query, tags, 64)
            }
            _ => false,
        })?;
    let RecipeBody::Furnace(body) = &recipe.body else {
        return None;
    };
    let runtime_id = items
        .runtime_id_by_name(&body.output.identifier)
        .or_else(|| {
            sc_recipe::rename_alias(tags, &body.output.identifier)
                .and_then(|alias| items.runtime_id_by_name(&alias))
        })?;
    Some(ItemStack {
        runtime_id,
        count: body.output.count.max(1),
        damage: body.output.data.unwrap_or(0).max(0) as u32,
        block_runtime_id: 0,
    })
}

/// Validate workstation/process declarations against live game knowledge.
/// Called once per snapshot at load (bootstrap), failing loudly:
/// - every `sc:workstation` station tag must map to a known `StationKind`
///   (unknown tags would silently degrade to `Unknown` and reject
///   everything downstream);
/// - a furnace-kind workstation requires its `sc:process` (the tick reads
///   cook duration from it; a missing process would wedge the furnace in
///   a silent ghost-skip), and a process requires its workstation;
/// - every `sc:process` lit variant must name a compiled block type
///   carrying a process that points back with the opposite `lit` bit (a
///   dangling or one-way swap target would wedge the lit state machine).
pub fn validate_workstations(
    snapshot: &sc_block::block_json::BlockJsonSnapshot,
) -> Result<(), String> {
    for (index, decl) in snapshot.workstation.iter().enumerate() {
        let Some(decl) = decl.as_ref() else {
            continue;
        };
        let station = StationKind::from_tag(&decl.station);
        if station == StationKind::Unknown {
            return Err(format!(
                "state {index}: unknown workstation station {:?}",
                decl.station.as_ref()
            ));
        }
        if is_furnace_station(station) && snapshot.process.get(index).is_none_or(|p| p.is_none()) {
            return Err(format!(
                "state {index}: furnace-kind workstation {:?} has no sc:process",
                decl.station.as_ref()
            ));
        }
    }
    for (index, process) in snapshot.process.iter().enumerate() {
        let Some(process) = process.as_ref() else {
            continue;
        };
        if snapshot.workstation.get(index).is_none_or(|w| w.is_none()) {
            return Err(format!("state {index}: sc:process without sc:workstation"));
        }
        if let Some(target) = process.lit_variant.as_ref() {
            if !snapshot.defaults.contains_key(target.as_ref()) {
                return Err(format!(
                    "state {index}: lit variant {target:?} names no compiled block type"
                ));
            }
            // Self-swaps would enqueue no-op lit changes every edge.
            let own = snapshot
                .states
                .get(index)
                .and_then(|state| snapshot.block_type(state.type_id))
                .map(|kind| kind.identifier.as_ref());
            if own == Some(target.as_ref()) {
                return Err(format!(
                    "state {index}: lit variant {target:?} points at its own block type"
                ));
            }
            // Lit pairs are two-way with opposite member bits; a one-way
            // pointer strands the target in the wrong lit state (the tick
            // only follows pointers, never guesses the way back). Columns
            // align with `states` by row, so the target type's first row
            // represents the pair shape (per-state uniformity is covered
            // by the parity gate on the live pack).
            let back = snapshot.states.iter().enumerate().find_map(|(row, state)| {
                snapshot
                    .block_type(state.type_id)
                    .filter(|kind| kind.identifier.as_ref() == target.as_ref())
                    .and_then(|_| snapshot.process.get(row)?.as_ref())
            });
            match back {
                Some(target_process) => {
                    if target_process.lit_variant.as_deref() != own {
                        return Err(format!(
                            "state {index}: lit variant {target:?} does not point back"
                        ));
                    }
                    if target_process.lit == process.lit {
                        return Err(format!(
                            "state {index}: lit pair {target:?} shares the same lit bit"
                        ));
                    }
                }
                None => {
                    return Err(format!(
                        "state {index}: lit variant {target:?} carries no sc:process"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Block hash at a position (`None` when the chunk is unloaded or the cell
/// is empty). One cached chunk read; mirrors `station_block_kind`.
pub(crate) fn block_hash_at(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<u32> {
    use sc_block::position::BlockPosition;
    use sc_world::storage::ChunkKey;
    let manager = world.get_resource::<sc_world::manager::MinecraftWorldManager>()?;
    let minecraft_world = manager.get_world(world_id)?;
    let target = BlockPosition::new(pos.0, pos.1, pos.2);
    let key = ChunkKey::new(
        minecraft_world.world_data.get_dimension(),
        target.chunk_position(),
    );
    let column = minecraft_world.chunk_provider.cached_chunk(key)?;
    let chunk = column.read();
    chunk
        .block_at(target.local_x(), target.y, target.local_z())
        .map(|state| state.0)
}

/// Decl inputs for one tick position: block hash plus the furnace station
/// and process duration. `None` = ghost (unloaded chunk, air,
/// non-workstation, non-furnace station, unpublished snapshot, or a
/// workstation without its process): skip, never delete here.
fn furnace_decls(
    world: &World,
    blocks: &BlockStateRegistry,
    snapshot: &sc_block::block_json::BlockJsonSnapshot,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<(u32, StationKind, u16)> {
    let hash = block_hash_at(world, world_id, pos)?;
    let dense = blocks.by_runtime_id(sc_world::chunk::BlockRuntimeId(hash))?;
    let workstation = snapshot.workstation_of(dense.0)?;
    let station = StationKind::from_tag(&workstation.station);
    if !is_furnace_station(station) {
        return None;
    }
    let process = snapshot.process_of(dense.0)?;
    Some((hash, station, process.cook_ticks))
}

/// Resolve the lit-swap target hash preserving `minecraft:cardinal_direction`.
/// `None` keeps the current block (never a guessed state): no process, no
/// lit variant, or the block already matches the burn state (idempotent —
/// the `lit` member bit gives the swap direction, so a stale edge never
/// swaps backwards).
fn lit_swap_hash(
    blocks: &BlockStateRegistry,
    snapshot: &sc_block::block_json::BlockJsonSnapshot,
    current_hash: u32,
    burning: bool,
) -> Option<u32> {
    let dense = blocks.by_runtime_id(sc_world::chunk::BlockRuntimeId(current_hash))?;
    let process = snapshot.process_of(dense.0)?;
    if process.lit == burning {
        return None;
    }
    let target = process.lit_variant.as_deref()?;
    let entry = blocks.state(dense)?;
    let facing = match entry
        .states
        .as_ref()
        .and_then(|states| states.get("minecraft:cardinal_direction"))
    {
        Some(sc_nbt::NbtValue::String(direction)) => direction.clone(),
        _ => return None,
    };
    let target_id = blocks.state_of(
        target,
        &[(
            "minecraft:cardinal_direction",
            BlockPropertyValue::Enum(Arc::from(facing)),
        )],
    )?;
    blocks.runtime_id(target_id).map(|runtime| runtime.0)
}

/// Slot container enums for a viewer's furnace window (`[input, fuel,
/// result]`), resolved from the workstation decl at the bound block.
/// `None` when unresolvable (broken window; caller skips sync).
pub fn window_slot_containers(world: &World, viewer: &EntityId) -> Option<[u8; 3]> {
    let open = world.get_component::<ContainerOpen>(viewer)?;
    let (_, world_id, pos) = open.workstation()?;
    let decl = crate::inventory::crafting::workstation_decl_at(world, &world_id, pos)?;
    let mut containers = [0u8; 3];
    let mut seen = [false; 3];
    for slot in decl.slots.iter() {
        // `Extra` roles (campfire-style) have no position in the furnace
        // triple; they sync through their own window path, never here.
        let index = match slot.role {
            SlotRole::Input => 0,
            SlotRole::Fuel => 1,
            SlotRole::Result => 2,
            SlotRole::Extra => continue,
        };
        containers[index] = slot.container;
        seen[index] = true;
    }
    seen.into_iter()
        .all(|present| present)
        .then_some(containers)
}

/// One viewer of a furnace window: runtime id plus window id.
struct FurnaceViewer {
    entity_id: u64,
    window_id: u8,
}

/// Component-based furnace tick: burn/cook every tracked process entity
/// once per game tick.
///
/// Tick-design compliance (§4-§6 unified design, same shape as
/// `item_drop_tick`):
/// - read-only shared inputs snapshot once per tick (recipe snapshot is an
///   `Arc` clone, tag resolver one small map); per-furnace work is O(1)
///   and idle entities publish nothing;
/// - the tick never writes chunks: lit swaps enqueue [`BlockChange`] for
///   the owning region to validate at commit time (cause `Command`: a
///   server simulation step, never a player break — no drops, no
///   inventory rollback);
/// - the tick never sends packets: viewers get `SyncFurnaceWindow` slot
///   facts and coalesced `SyncFurnaceProgress` state;
/// - entities whose block is gone are skipped, never deleted here (break
///   path owns removal plus drops).
pub fn process_tick(
    world: World,
    items: Res<ItemRegistry>,
    components: Res<ItemComponentTable>,
    recipes: Res<SharedRecipeRegistry>,
    tags: Res<SharedItemTags>,
    blocks: Res<BlockStateRegistry>,
    json: Res<BlockJsonRegistry>,
    mut queue: ResMut<BlockChangeQueue>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    let snapshot = recipes.snapshot();
    let resolver = tags.resolver();
    let Some(block_snapshot) = json.get() else {
        return;
    };
    // Viewers once per tick: players with a furnace workstation binding.
    // Bounded by player count, never by furnace count.
    let mut viewers: HashMap<(MinecraftWorldId, (i32, i32, i32)), Vec<FurnaceViewer>> =
        HashMap::new();
    for entity in world.entities_with_component::<ContainerOpen>() {
        let (Some(open), Some(actor)) = (
            world.get_component::<ContainerOpen>(&entity),
            world.get_component::<MinecraftEntityId>(&entity),
        ) else {
            continue;
        };
        if let Some((station, world_id, pos)) = open.workstation() {
            if is_furnace_station(station) {
                viewers
                    .entry((world_id, pos))
                    .or_default()
                    .push(FurnaceViewer {
                        entity_id: actor.0,
                        window_id: open.window_id,
                    });
            }
        }
    }
    let mut faults = world.get_resource_mut::<crate::net::PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    // Deterministic order: sort entity refs by (world, pos) so multi-entity
    // ticks replay identically (region scheduling lands here later, see Q5).
    // The entity id rides along so the hot loop reads components directly.
    let mut tracked: Vec<(MinecraftWorldId, (i32, i32, i32), EntityId)> = Vec::new();
    for entity in world.entities_with_component::<BlockProcessState>() {
        let Some(reference) = world.get_component::<BlockEntityRef>(&entity) else {
            continue;
        };
        if !matches!(
            reference.kind,
            crate::block_entity::BlockEntityKind::Furnace(_)
        ) {
            continue;
        }
        tracked.push((reference.world.clone(), reference.pos, entity));
    }
    tracked.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    for (world_id, pos, entity) in tracked {
        // Ghost check + decl params in one chunk read (same helper as the
        // store path, so ghost semantics are identical by construction).
        let Some((hash, _decl_station, _cook_ticks)) =
            furnace_decls(&world, &blocks, &block_snapshot, &world_id, pos)
        else {
            continue;
        };
        let Some(process) = world.get_component::<BlockProcessState>(&entity) else {
            continue;
        };
        // Recipe tags come from the component kind (set from the decl
        // station at ensure; same mixing behavior as the store path when a
        // block was replaced under a live entry).
        let kind = process.kind();
        let before = process.furnace_slots();
        let was_burning = process.is_burning();
        let output = smelt_output(&snapshot, kind, &before[0], &items, &resolver);
        let fuel = fuel_ticks(&components, &before[1]);
        let max_stack = items.max_stack_size(before[2].runtime_id);
        let outcome = process.advance(output.as_ref(), fuel, max_stack);
        // Diagnostic: burn/cook transitions with item names (debug only).
        if outcome.consumed_fuel || outcome.completed {
            let name_of = |stack: &ItemStack| {
                items
                    .get(stack.runtime_id)
                    .map(|def| def.name.to_string())
                    .unwrap_or_else(|| format!("runtime:{}", stack.runtime_id))
            };
            log::debug!(
                "[furnace] {world_id:?} {pos:?} consumed_fuel={} completed={} burn={}/{} cook={} input={} fuel={} result={}",
                outcome.consumed_fuel,
                outcome.completed,
                process.burn_remaining(),
                process.burn_max(),
                process.cook_progress(),
                name_of(&before[0]),
                name_of(&before[1]),
                name_of(&process.furnace_slots()[2]),
            );
        }
        let (slots, cook, lit, lit_max, burning) = (
            process.furnace_slots(),
            process.cook_progress(),
            process.lit_progress(),
            process.burn_max().min(u16::MAX as u32) as u16,
            process.is_burning(),
        );
        let watching = viewers.get(&(world_id.clone(), pos));
        // Slot facts only when slots changed (completion/fuel burn).
        if outcome.completed || outcome.consumed_fuel {
            if let Some(viewing) = watching {
                for viewer in viewing.iter() {
                    publisher.publish(SyncFurnaceWindow {
                        entity_id: viewer.entity_id,
                        window_id: viewer.window_id,
                        station: kind,
                        input: slots[0].clone(),
                        fuel: slots[1].clone(),
                        result: slots[2].clone(),
                    });
                }
            }
        }
        // Progress state while anything burns/cooks (coalesced per
        // viewer window; pressure drops self-heal next tick).
        if let Some(viewing) = watching {
            if !viewing.is_empty() && (was_burning || burning || cook > 0) {
                for viewer in viewing.iter() {
                    publisher.publish(SyncFurnaceProgress {
                        entity_id: viewer.entity_id,
                        window_id: viewer.window_id,
                        cook_ticks: cook,
                        lit_ticks: lit,
                        lit_duration: lit_max,
                    });
                }
            }
        }
        // Lit edge: same helper as the store path (identical target by
        // construction; parity asserts the enqueue decision).
        if outcome.lit_changed {
            if let Some(target) = lit_swap_hash(&blocks, &block_snapshot, hash, burning) {
                let _ = queue.push(BlockChange {
                    world_id: world_id.clone(),
                    position: BlockPosition::new(pos.0, pos.1, pos.2),
                    layer: 0,
                    state: sc_world::chunk::BlockRuntimeId(target),
                    cause: BlockChangeCause::Command,
                    flags: update_flags::DEFAULT,
                });
            } else {
                log::debug!("[furnace] lit swap skipped, no counterpart for {pos:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_recipe::{CompileBudgets, RecipeRegistrySnapshot, SourceRecipe};

    fn items() -> ItemRegistry {
        let registry = ItemRegistry::new();
        for (id, name) in [
            (1, "minecraft:coal"),
            (2, "minecraft:stick"),
            (3, "minecraft:oak_log"),
            (4, "minecraft:potato"),
            (5, "minecraft:baked_potato"),
            (6, "minecraft:oak_boat"),
            (7, "minecraft:stone"),
        ] {
            registry.upsert(sc_item::ItemDefinition::new(id, name));
        }
        registry
    }

    /// Component table built through the real pipeline (item JSON ->
    /// spawner -> runtime manager -> dense table), not hand-made entries.
    fn fuel_table() -> sc_packloader::item::ItemComponentTable {
        use sc_packloader::item::MinecraftItemSpawner;
        use sc_packloader::version_control::runtime::{
            MinecraftRuntimeJson, MinecraftRuntimeManager,
        };
        let ids = [
            ("minecraft:coal", 1),
            ("minecraft:stick", 2),
            ("minecraft:oak_log", 3),
            ("minecraft:potato", 4),
            ("minecraft:baked_potato", 5),
            ("minecraft:oak_boat", 6),
            ("minecraft:stone", 7),
        ];
        let manager = MinecraftRuntimeManager::new(
            ids.iter()
                .map(|(name, id)| MinecraftRuntimeJson {
                    name: name.to_string(),
                    id: *id,
                    version: 1,
                    component_based: true,
                })
                .collect(),
        );
        // Durations in seconds (vanilla `minecraft:fuel` semantics).
        let fuels: &[(&str, f32)] = &[
            ("minecraft:coal", 80.0),
            ("minecraft:stick", 5.0),
            ("minecraft:oak_log", 15.0),
            ("minecraft:oak_boat", 60.0),
        ];
        let mut spawners = std::collections::HashMap::new();
        for (name, _) in ids {
            let components = fuels.iter().find(|(fuel_name, _)| *fuel_name == name).map(
                |(_, seconds)| serde_json::json!({ "minecraft:fuel": { "duration": seconds } }),
            );
            let mut json = serde_json::json!({ "description": { "identifier": name } });
            if let Some(components) = components {
                json["components"] = components;
            }
            let spawner: MinecraftItemSpawner = serde_json::from_value(json).unwrap();
            spawners.insert(name.to_string(), spawner);
        }
        let mut manager = manager;
        manager.push_item_map(spawners);
        let mut table = sc_packloader::item::ItemComponentTable::new();
        table.build(&manager);
        table
    }

    #[test]
    fn fuel_ticks_come_from_the_item_component() {
        let table = fuel_table();
        let ticks = |id: u16| fuel_ticks(&table, &ItemStack::new(id, 1));
        assert_eq!(ticks(1), 1600);
        assert_eq!(ticks(2), 100);
        assert_eq!(ticks(3), 300);
        assert_eq!(ticks(6), 1200);
        // No component (or unknown item) = not fuel, never a code default.
        assert_eq!(ticks(4), 0);
        assert_eq!(ticks(7), 0);
        assert_eq!(fuel_ticks(&table, &ItemStack::EMPTY), 0);
    }

    fn furnace_snapshot() -> RecipeRegistrySnapshot {
        let json = serde_json::json!({
            "format_version": "1.12",
            "minecraft:recipe_furnace": {
                "description": {"identifier": "minecraft:furnace_potato"},
                "tags": ["furnace", "smoker"],
                "input": "minecraft:potato",
                "output": "minecraft:baked_potato"
            }
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let src = SourceRecipe::new("p", 0, "r.json", "1.12", json, &bytes);
        RecipeRegistrySnapshot::compile(&[src], &CompileBudgets::default(), false)
            .unwrap()
            .0
    }

    #[test]
    fn smelt_query_matches_input_to_output() {
        let items = items();
        let snapshot = furnace_snapshot();
        let tags = sc_recipe::MapTagResolver::empty();
        let out = smelt_output(
            &snapshot,
            StationKind::Furnace,
            &ItemStack::new(4, 1),
            &items,
            &tags,
        )
        .expect("potato smelts");
        assert_eq!(out.runtime_id, 5);
        // Wrong station tag: no match (recipe tags are furnace+smoker only).
        assert!(smelt_output(
            &snapshot,
            StationKind::BlastFurnace,
            &ItemStack::new(4, 1),
            &items,
            &tags,
        )
        .is_none());
        // Non-smeltable input: no match.
        assert!(smelt_output(
            &snapshot,
            StationKind::Furnace,
            &ItemStack::new(7, 1),
            &items,
            &tags,
        )
        .is_none());
    }

    fn furnace_roles() -> Vec<sc_block::workstation::SlotRole> {
        use sc_block::workstation::SlotRole;
        vec![SlotRole::Input, SlotRole::Fuel, SlotRole::Result]
    }

    #[test]
    fn tick_cooks_and_completes_into_result() {
        use crate::inventory::crafting::BlockProcessState;
        let items = items();
        let fuels = fuel_table();
        let snapshot = furnace_snapshot();
        let tags = sc_recipe::MapTagResolver::empty();
        let state = BlockProcessState::new(StationKind::Furnace, furnace_roles(), 200);
        state.set_slot(sc_block::workstation::SlotRole::Input, ItemStack::new(4, 2));
        state.set_slot(sc_block::workstation::SlotRole::Fuel, ItemStack::new(1, 1));
        // Furnace cooks in 200 ticks; fuel lights on the first tick.
        for _ in 0..200 {
            let input = state
                .slot(sc_block::workstation::SlotRole::Input)
                .unwrap_or_default();
            let fuel_stack = state
                .slot(sc_block::workstation::SlotRole::Fuel)
                .unwrap_or_default();
            let out = smelt_output(&snapshot, state.kind(), &input, &items, &tags);
            let fuel = fuel_ticks(&fuels, &fuel_stack);
            state.advance(out.as_ref(), fuel, 64);
        }
        assert_eq!(
            state.slot(sc_block::workstation::SlotRole::Result),
            Some(ItemStack::new(5, 1))
        );
        assert_eq!(
            state
                .slot(sc_block::workstation::SlotRole::Input)
                .map(|stack| stack.count),
            Some(1)
        );
        assert!(
            state
                .slot(sc_block::workstation::SlotRole::Fuel)
                .is_some_and(|stack| stack.is_empty()),
            "one coal consumed"
        );
        assert!(state.is_burning(), "coal still burning");
    }

    #[test]
    fn tick_cooks_faster_with_smaller_cook_total() {
        use crate::inventory::crafting::BlockProcessState;
        use sc_block::workstation::SlotRole;
        let items = items();
        let fuels = fuel_table();
        let snapshot = furnace_snapshot();
        let tags = sc_recipe::MapTagResolver::empty();
        let drive = |station: StationKind, cook: u16, ticks: u32| {
            let state = BlockProcessState::new(station, furnace_roles(), cook);
            state.set_slot(SlotRole::Input, ItemStack::new(4, 1));
            state.set_slot(SlotRole::Fuel, ItemStack::new(1, 1));
            for _ in 0..ticks {
                let input = state.slot(SlotRole::Input).unwrap_or_default();
                let fuel_stack = state.slot(SlotRole::Fuel).unwrap_or_default();
                let out = smelt_output(&snapshot, state.kind(), &input, &items, &tags);
                let fuel = fuel_ticks(&fuels, &fuel_stack);
                state.advance(out.as_ref(), fuel, 64);
            }
            state
        };
        // Same inputs, smoker duration (100): finishes in half the ticks
        // (the recipe tags cover furnace+smoker, so smoker smelts here).
        let fast = drive(StationKind::Smoker, 100, 100);
        assert_eq!(fast.slot(SlotRole::Result), Some(ItemStack::new(5, 1)));
        // Furnace duration (200) would still be cooking at tick 100.
        let slow = drive(StationKind::Furnace, 200, 100);
        assert_eq!(slow.slot(SlotRole::Result), Some(ItemStack::empty()));
    }

    #[test]
    fn tick_resets_progress_when_fuel_runs_out() {
        use crate::inventory::crafting::BlockProcessState;
        use sc_block::workstation::SlotRole;
        let fuels = fuel_table();
        let state = BlockProcessState::new(StationKind::Furnace, furnace_roles(), 200);
        state.set_slot(SlotRole::Input, ItemStack::new(4, 1));
        state.set_slot(SlotRole::Fuel, ItemStack::new(2, 1));
        // One stick = 100 burn ticks; furnace needs 200 to finish.
        let out = Some(ItemStack::new(5, 1));
        for _ in 0..101 {
            let fuel_stack = state.slot(SlotRole::Fuel).unwrap_or_default();
            let fuel = fuel_ticks(&fuels, &fuel_stack);
            state.advance(out.as_ref(), fuel, 64);
        }
        assert!(!state.is_burning());
        assert_eq!(
            state.slot(SlotRole::Result),
            Some(ItemStack::empty()),
            "cook never finished"
        );
        assert_eq!(state.cook_progress(), 0, "progress resets without burn");
    }

    #[test]
    fn furnace_station_set_is_stable() {
        // The furnace-family set stays code (recipe-side station language,
        // see §8); the decls carry the per-station params.
        assert!(is_furnace_station(StationKind::Furnace));
        assert!(is_furnace_station(StationKind::BlastFurnace));
        assert!(is_furnace_station(StationKind::Smoker));
        assert!(!is_furnace_station(StationKind::CraftingTable));
        assert!(!is_furnace_station(StationKind::Stonecutter));
    }

    /// Synthetic furnace bundle (air + furnace + lit_furnace, cardinal
    /// states, workstation/process decls). Self-consistent hashes, no pack
    /// files: registry built FROM this snapshot, so dense ids align with
    /// column rows by construction.
    fn tick_bundle() -> (
        sc_block::block_json::BlockJsonRegistry,
        sc_block::registry::BlockStateRegistry,
    ) {
        use sc_block::block_json::compile_bundle;
        use sc_packloader::block::{
            fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
        };
        fn file(identifier: &str, lit_variant: &str, lit: bool, cook: u16, base_id: u32) -> String {
            format!(
                r#"{{
            "format_version": "1.10.0",
            "minecraft:block": {{
                "description": {{
                    "identifier": "{identifier}",
                    "states": {{"minecraft:cardinal_direction": ["east", "north", "south", "west"]}}
                }},
                "components": {{
                    "sc:workstation": {{
                        "station": "furnace",
                        "container": 2,
                        "window": true,
                        "slots": [
                            {{"role": "input", "container": 25}},
                            {{"role": "fuel", "container": 24}},
                            {{"role": "result", "container": 26}}
                        ]
                    }},
                    "sc:process": {{
                        "kind": "burn_cook",
                        "cook_ticks": {cook},
                        "fuel": true,
                        "lit": {lit},
                        "lit_variant": "{lit_variant}"
                    }}
                }},
                "sc:default_state": {{"minecraft:cardinal_direction": "south"}},
                "sc:protocol_runtime_ids": [{base_id}, {b1}, {b2}, {b3}]
            }}
        }}"#,
                b1 = base_id + 1,
                b2 = base_id + 2,
                b3 = base_id + 3,
            )
        }
        let air = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let raws: Vec<(&str, String)> = vec![
            (
                "blocks/minecraft/air.json",
                air.to_string(),
            ),
            (
                "blocks/minecraft/furnace.json",
                file(
                    "minecraft:furnace",
                    "minecraft:lit_furnace",
                    false,
                    200,
                    100,
                ),
            ),
            (
                "blocks/minecraft/lit_furnace.json",
                file("minecraft:lit_furnace", "minecraft:furnace", true, 200, 110),
            ),
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
        let (snapshot, _) =
            compile_bundle(&bundle, "test", &budgets, &|_| None, &|_| true).expect("bundle 应编译");
        let blocks = sc_block::registry::BlockStateRegistry::from_block_snapshot(&snapshot);
        let json = sc_block::block_json::BlockJsonRegistry::new();
        json.publish(std::sync::Arc::new(snapshot));
        (json, blocks)
    }

    fn tick_hash(registry: &sc_block::registry::BlockStateRegistry, name: &str) -> u32 {
        registry
            .runtime_id(
                registry
                    .state_of(
                        name,
                        &[(
                            "minecraft:cardinal_direction",
                            sc_block::state::BlockPropertyValue::Enum("south".into()),
                        )],
                    )
                    .unwrap_or_else(|| panic!("{name} 应在 bundle 中")),
            )
            .expect("runtime id")
            .0
    }

    /// Tick harness: one-chunk overworld with a real furnace block plus
    /// all resources `process_tick` needs. Returns world, world id and a
    /// viewer bound to the furnace window.
    fn tick_world(furnace_hash: u32) -> (sc_ecs::world::World, MinecraftWorldId) {
        use sc_world::chunk::{BlockRuntimeId, ChunkPosition};
        use sc_world::storage::{ChunkKey, EmptyWorldStorage, WorldChunkProvider};
        let world = sc_ecs::world::World::new();
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
        (world, world_id)
    }

    #[test]
    fn lit_swap_follows_decl_and_stays_idempotent() {
        let (json, registry) = tick_bundle();
        let snapshot = json.get().expect("快照已发布");
        let furnace = tick_hash(&registry, "minecraft:furnace");
        let lit = lit_swap_hash(&registry, &snapshot, furnace, true).expect("lit counterpart");
        assert_eq!(lit, tick_hash(&registry, "minecraft:lit_furnace"));
        // Unlit edge swaps back.
        let back = lit_swap_hash(&registry, &snapshot, lit, false).expect("unlit counterpart");
        assert_eq!(back, furnace);
        // Already matching: no redundant swap (the `lit` member bit gives
        // the direction, so a stale edge never swaps backwards).
        assert_eq!(lit_swap_hash(&registry, &snapshot, furnace, false), None);
        assert_eq!(lit_swap_hash(&registry, &snapshot, lit, true), None);
        // Blast/smoker pairs come from their own decls, not code.
        assert_eq!(
            snapshot
                .process_of(
                    registry
                        .by_runtime_id(sc_world::chunk::BlockRuntimeId(furnace))
                        .expect("dense")
                        .0
                )
                .expect("process")
                .lit_variant
                .as_deref(),
            Some("minecraft:lit_furnace")
        );
    }

    #[test]
    fn lit_swap_hash_preserves_facing() {
        let (json, registry) = tick_bundle();
        let snapshot = json.get().expect("快照已发布");
        let furnace = tick_hash(&registry, "minecraft:furnace");
        let lit = lit_swap_hash(&registry, &snapshot, furnace, true).expect("lit counterpart");
        assert_eq!(lit, tick_hash(&registry, "minecraft:lit_furnace"));
        // Unlit edge swaps back.
        let back = lit_swap_hash(&registry, &snapshot, lit, false).expect("unlit counterpart");
        assert_eq!(back, furnace);
    }

    #[test]
    fn tick_ignites_and_syncs_viewer() {
        use sc_block::workstation::SlotRole;
        use sc_ecs::system::{IntoSystem, System};
        let (json, blocks) = tick_bundle();
        let furnace = tick_hash(&blocks, "minecraft:furnace");
        let (world, world_id) = tick_world(furnace);
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items());
        world.insert_resource(fuel_table());
        world.insert_resource(crate::inventory::crafting::SharedRecipeRegistry::new(
            furnace_snapshot(),
        ));
        world.insert_resource(crate::inventory::crafting::SharedItemTags::default());
        world.insert_resource(sc_block::write::BlockChangeQueue::default());
        world.insert_resource(crate::net::NetworkOutbox::with_limits(64, 64, usize::MAX));
        let entity = world.spawn((
            crate::block_entity::BlockEntityRef {
                world: world_id.clone(),
                pos: (0, 64, 0),
                kind: crate::block_entity::BlockEntityKind::Furnace(StationKind::Furnace),
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
        process.set_slot(SlotRole::Input, ItemStack::new(4, 2));
        process.set_slot(SlotRole::Fuel, ItemStack::new(1, 1));
        world.spawn((
            crate::inventory::ContainerOpen::at_workstation(
                3,
                StationKind::Furnace,
                world_id.clone(),
                (0, 64, 0),
            ),
            sc_entity::MinecraftEntityId(7),
        ));
        process_tick.into_system().run(&world);

        // Ignited: burning, one lit swap queued, viewer synced slots + progress.
        assert!(process.is_burning());
        assert!(
            process
                .slot(SlotRole::Fuel)
                .is_some_and(|stack| stack.is_empty()),
            "one coal consumed"
        );
        let queue = world
            .get_resource::<sc_block::write::BlockChangeQueue>()
            .unwrap();
        assert_eq!(queue.len(), 1, "lit swap enqueued");
        let intents = world
            .get_resource_mut::<crate::net::NetworkOutbox>()
            .unwrap()
            .drain();
        assert!(
            intents.iter().any(|intent| matches!(
                intent,
                crate::net::NetworkIntent::SyncFurnaceWindow(payload)
                    if payload.entity_id == 7 && payload.window_id == 3
            )),
            "viewer gets slot facts"
        );
        assert!(
            intents.iter().any(|intent| matches!(
                intent,
                crate::net::NetworkIntent::SyncFurnaceProgress(payload)
                    if payload.entity_id == 7 && payload.window_id == 3
            )),
            "viewer gets progress state"
        );
    }

    #[test]
    fn tick_completes_ready_cook() {
        use sc_block::workstation::SlotRole;
        use sc_ecs::system::{IntoSystem, System};
        let (json, blocks) = tick_bundle();
        let furnace = tick_hash(&blocks, "minecraft:furnace");
        let (world, world_id) = tick_world(furnace);
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items());
        world.insert_resource(fuel_table());
        world.insert_resource(crate::inventory::crafting::SharedRecipeRegistry::new(
            furnace_snapshot(),
        ));
        world.insert_resource(crate::inventory::crafting::SharedItemTags::default());
        world.insert_resource(sc_block::write::BlockChangeQueue::default());
        world.insert_resource(crate::net::NetworkOutbox::with_limits(64, 64, usize::MAX));
        let entity = world.spawn((
            crate::block_entity::BlockEntityRef {
                world: world_id.clone(),
                pos: (0, 64, 0),
                kind: crate::block_entity::BlockEntityKind::Furnace(StationKind::Furnace),
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
        // One tick away from completion: drive 199 pure advances (the
        // `advance` unit tests cover multi-tick behavior in microseconds).
        process.set_slot(SlotRole::Input, ItemStack::new(4, 2));
        process.set_slot(SlotRole::Fuel, ItemStack::new(1, 1));
        let items_ref = world.get_resource::<ItemRegistry>().expect("items");
        let fuels_ref = world
            .get_resource::<sc_packloader::item::ItemComponentTable>()
            .expect("fuels");
        let recipes_ref = world
            .get_resource::<crate::inventory::crafting::SharedRecipeRegistry>()
            .expect("recipes");
        let snapshot = recipes_ref.snapshot();
        let tags = sc_recipe::MapTagResolver::empty();
        for _ in 0..199 {
            let input = process.slot(SlotRole::Input).unwrap_or_default();
            let fuel_stack = process.slot(SlotRole::Fuel).unwrap_or_default();
            let out = smelt_output(&snapshot, StationKind::Furnace, &input, &items_ref, &tags);
            let fuel = fuel_ticks(&fuels_ref, &fuel_stack);
            process.advance(out.as_ref(), fuel, 64);
        }
        assert_eq!(process.cook_progress(), 199);
        world.spawn((
            crate::inventory::ContainerOpen::at_workstation(
                3,
                StationKind::Furnace,
                world_id.clone(),
                (0, 64, 0),
            ),
            sc_entity::MinecraftEntityId(7),
        ));
        process_tick.into_system().run(&world);

        assert_eq!(process.slot(SlotRole::Result), Some(ItemStack::new(5, 1)));
        assert_eq!(
            process.slot(SlotRole::Input).map(|stack| stack.count),
            Some(1)
        );
        assert_eq!(process.cook_progress(), 0);
        let intents = world
            .get_resource_mut::<crate::net::NetworkOutbox>()
            .unwrap()
            .drain();
        assert!(
            intents.iter().any(|intent| matches!(
                intent,
                crate::net::NetworkIntent::SyncFurnaceWindow(payload)
                    if payload.entity_id == 7 && payload.window_id == 3
            )),
            "completion syncs slots"
        );
    }

    #[test]
    fn validate_workstations_rejects_unknown_station_and_dangling_lit() {
        use sc_block::workstation::{ProcessDecl, ProcessKind, WorkstationDecl};
        use std::collections::HashMap;
        fn ws(station: &str) -> WorkstationDecl {
            WorkstationDecl {
                station: station.into(),
                container: 2,
                window: true,
                grid: None,
                slots: Vec::new(),
            }
        }
        fn proc(lit: bool, lit_variant: Option<&str>) -> ProcessDecl {
            ProcessDecl {
                kind: ProcessKind::BurnCook,
                cook_ticks: 200,
                fuel: true,
                lit,
                lit_variant: lit_variant.map(|name| name.into()),
            }
        }
        fn typed(name: &str, type_id: u32) -> sc_block::block_json::SnapType {
            sc_block::block_json::SnapType {
                identifier: name.into(),
                file: "".into(),
                type_id,
                properties: Vec::new(),
                default_state: 0,
                state_ids: vec![type_id],
            }
        }
        fn state(index: u32, type_id: u32) -> sc_block::block_json::SnapState {
            sc_block::block_json::SnapState {
                index,
                type_id,
                key: "".into(),
                hash: 100 + index,
                network_id: 100 + index,
                typed_key: "".into(),
                props: Vec::new(),
            }
        }
        // Empty snapshot validates (nothing declared).
        let empty = sc_block::block_json::BlockJsonSnapshot::default();
        assert!(validate_workstations(&empty).is_ok());
        // Unknown station tag fails.
        let mut bad_station = sc_block::block_json::BlockJsonSnapshot::default();
        bad_station.workstation = vec![Some(ws("nope"))];
        assert!(validate_workstations(&bad_station).is_err());
        // Furnace-kind workstation without a process fails.
        let mut missing_process = sc_block::block_json::BlockJsonSnapshot::default();
        missing_process.workstation = vec![Some(ws("furnace"))];
        assert!(validate_workstations(&missing_process).is_err());
        // Process without a workstation fails.
        let mut missing_ws = sc_block::block_json::BlockJsonSnapshot::default();
        missing_ws.process = vec![Some(proc(false, None))];
        assert!(validate_workstations(&missing_ws).is_err());
        // Dangling lit variant fails (defaults has no such type).
        let mut bad_lit = sc_block::block_json::BlockJsonSnapshot::default();
        bad_lit.defaults = HashMap::from([("minecraft:furnace".into(), 0)]);
        bad_lit.workstation = vec![Some(ws("furnace"))];
        bad_lit.process = vec![Some(proc(false, Some("minecraft:missing")))];
        assert!(validate_workstations(&bad_lit).is_err());
        // One-way pointer fails (target carries no process).
        let mut one_way = sc_block::block_json::BlockJsonSnapshot::default();
        one_way.defaults = HashMap::from([
            ("minecraft:furnace".into(), 0),
            ("minecraft:lit_furnace".into(), 1),
        ]);
        one_way.types = vec![
            typed("minecraft:furnace", 0),
            typed("minecraft:lit_furnace", 1),
        ];
        one_way.states = vec![state(0, 0), state(1, 1)];
        one_way.workstation = vec![Some(ws("furnace")), Some(ws("furnace"))];
        one_way.process = vec![Some(proc(false, Some("minecraft:lit_furnace"))), None];
        assert!(validate_workstations(&one_way).is_err());
        // Same lit bit fails.
        let mut same_bit = one_way.clone();
        same_bit.process = vec![
            Some(proc(false, Some("minecraft:lit_furnace"))),
            Some(proc(false, Some("minecraft:furnace"))),
        ];
        same_bit.workstation = vec![Some(ws("furnace")), Some(ws("furnace"))];
        assert!(validate_workstations(&same_bit).is_err());
        // Known station plus a two-way pair with opposite bits passes.
        let mut good = sc_block::block_json::BlockJsonSnapshot::default();
        good.defaults = HashMap::from([
            ("minecraft:furnace".into(), 0),
            ("minecraft:lit_furnace".into(), 1),
        ]);
        good.types = vec![
            typed("minecraft:furnace", 0),
            typed("minecraft:lit_furnace", 1),
        ];
        good.states = vec![state(0, 0), state(1, 1)];
        good.workstation = vec![Some(ws("furnace")), Some(ws("furnace"))];
        good.process = vec![
            Some(proc(false, Some("minecraft:lit_furnace"))),
            Some(proc(true, Some("minecraft:furnace"))),
        ];
        assert!(validate_workstations(&good).is_ok());
    }

    /// End-to-end repro of the live furnace flow on the current code path:
    /// bind a window, move smeltable + fuel in through `execute` (the real
    /// ItemStackRequest path, including write-back), then tick once and
    /// require ignition. Anything swallowing items between the move and
    /// the tick fails here instead of on the server.
    #[test]
    fn move_into_furnace_roundtrips_and_ignites() {
        use crate::inventory::craft_inventory::{
            execute, CraftContainer, CraftSlotRef, InventoryAction,
        };
        use crate::inventory::crafting::CraftRequestEvent;
        use sc_block::workstation::SlotRole;
        use sc_ecs::system::{IntoSystem, System};

        let (json, blocks) = tick_bundle();
        let furnace = tick_hash(&blocks, "minecraft:furnace");
        let (world, world_id) = tick_world(furnace);
        world.insert_resource(json);
        world.insert_resource(blocks);
        world.insert_resource(items());
        world.insert_resource(fuel_table());
        world.insert_resource(crate::inventory::crafting::SharedRecipeRegistry::new(
            furnace_snapshot(),
        ));
        world.insert_resource(crate::inventory::crafting::SharedItemTags::default());
        world.insert_resource(sc_block::write::BlockChangeQueue::default());
        world.insert_resource(crate::net::NetworkOutbox::with_limits(64, 64, usize::MAX));
        world.insert_resource(crate::block_entity::BlockEntityIndex::default());
        // Player standing at the furnace with potato x2 (slot 0) + coal x1.
        let inventory = sc_item::PlayerInventory::new(36);
        inventory.set(0, ItemStack::new(4, 2));
        inventory.set(1, ItemStack::new(1, 1));
        let player = world.spawn(inventory);
        world.add_component(&player, world_id.clone());
        world.add_component(
            &player,
            sc_entity::motion::Transform::new(
                sc_entity::motion::Position {
                    x: 0.5,
                    y: 64.0,
                    z: 0.5,
                },
                sc_entity::motion::Rotation {
                    yaw: 0.0,
                    pitch: 0.0,
                    head_yaw: 0.0,
                },
            ),
        );
        world.add_component(
            &player,
            crate::inventory::ContainerOpen::at_workstation(
                3,
                StationKind::Furnace,
                world_id.clone(),
                (0, 64, 0),
            ),
        );
        world.add_component(&player, sc_entity::MinecraftEntityId(7));
        // Real open-path ensure (not a hand-built entity).
        let ensured = crate::block_entity::ensure_block_entity(
            &world,
            world_id.clone(),
            (0, 64, 0),
            crate::block_entity::BlockEntityKind::Furnace(StationKind::Furnace),
        );
        assert!(ensured.is_some(), "open ensure 应创建实体");
        let inv_ref = |slot: u8| CraftSlotRef {
            container: CraftContainer::Inventory,
            slot,
            net_id: 0,
            dynamic: None,
        };
        let furnace_ref = |slot: u8| CraftSlotRef {
            container: CraftContainer::Furnace,
            slot,
            net_id: 0,
            dynamic: None,
        };
        // Potato into input.
        assert!(execute(
            &world,
            &CraftRequestEvent {
                entity: player,
                actions: vec![InventoryAction::Move {
                    amount: 2,
                    source: inv_ref(0),
                    destination: furnace_ref(INPUT_SLOT),
                }],
                recipe_network_id: 0,
                requested_count: 1,
                operation_id: 1,
            }
        ));
        let process = crate::block_entity::process_state_at(&world, &world_id, (0, 64, 0))
            .expect("实体应存在");
        assert_eq!(
            process.slot(SlotRole::Input),
            Some(ItemStack::new(4, 2)),
            "input 应写入组件"
        );
        // Coal into fuel (fuel check runs against the real component table).
        assert!(execute(
            &world,
            &CraftRequestEvent {
                entity: player,
                actions: vec![InventoryAction::Move {
                    amount: 1,
                    source: inv_ref(1),
                    destination: furnace_ref(FUEL_SLOT),
                }],
                recipe_network_id: 0,
                requested_count: 1,
                operation_id: 2,
            }
        ));
        assert!(
            process
                .slot(SlotRole::Fuel)
                .is_some_and(|stack| stack == ItemStack::new(1, 1)),
            "fuel 应写入组件"
        );
        // Player slots drained exactly once (no duplication, no loss).
        let held = world
            .get_component::<sc_item::PlayerInventory>(&player)
            .expect("背包");
        assert!(held.get(0).is_some_and(|stack| stack.is_empty()));
        assert!(held.get(1).is_some_and(|stack| stack.is_empty()));
        // One tick must ignite (same world the moves wrote to).
        process_tick.into_system().run(&world);
        assert!(process.is_burning(), "有点火物应点燃");
    }

    #[test]
    fn tick_skips_ghost_when_block_is_gone() {
        use sc_block::workstation::SlotRole;
        use sc_ecs::system::{IntoSystem, System};
        // No palette needed: air is a registry builtin, and a ghost only
        // needs "not a furnace block". The empty JSON registry (no
        // published snapshot) resolves nothing, so the tick skips.
        let blocks = sc_block::registry::BlockStateRegistry::new();
        let air = blocks.air().0;
        let (world, world_id) = tick_world(air);
        world.insert_resource(sc_block::block_json::BlockJsonRegistry::new());
        world.insert_resource(blocks);
        world.insert_resource(items());
        world.insert_resource(fuel_table());
        world.insert_resource(crate::inventory::crafting::SharedRecipeRegistry::new(
            furnace_snapshot(),
        ));
        world.insert_resource(crate::inventory::crafting::SharedItemTags::default());
        world.insert_resource(sc_block::write::BlockChangeQueue::default());
        world.insert_resource(crate::net::NetworkOutbox::with_limits(64, 64, usize::MAX));
        let entity = world.spawn((
            crate::block_entity::BlockEntityRef {
                world: world_id.clone(),
                pos: (0, 64, 0),
                kind: crate::block_entity::BlockEntityKind::Furnace(StationKind::Furnace),
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
        process.set_slot(SlotRole::Input, ItemStack::new(4, 2));
        process.set_slot(SlotRole::Fuel, ItemStack::new(1, 1));
        process_tick.into_system().run(&world);

        // Nothing burned, nothing queued, nothing published, entity kept
        // for the break path (tick never deletes).
        assert!(!process.is_burning());
        assert!(world
            .get_component::<crate::block_entity::BlockEntityRef>(&entity)
            .is_some());
        assert!(world
            .get_resource::<sc_block::write::BlockChangeQueue>()
            .unwrap()
            .is_empty());
        assert!(world
            .get_resource_mut::<crate::net::NetworkOutbox>()
            .unwrap()
            .drain()
            .is_empty());
    }

    /// Live-pack decl gate: every shipped workstation block state carries
    /// exactly the declared station/container/window/slots/process below.
    /// Runs against the live pack (skips when absent). The byte values are
    /// wire-verified container enums (protocol facts,
    /// same bar as packet ids); the decls own the station→byte attribution
    /// the hardcoded matches used to own.
    #[test]
    fn decl_tables_match_live_pack_workstations() {
        use sc_block::workstation::SlotRole;

        let Some(mut source) = sc_packloader::test_pack::open_test_pack() else {
            eprintln!("[skip] 版本包缺失，跳过测试");
            return;
        };
        let mut pack =
            sc_packloader::version_control::loader::ZippedVersionPack::get_version(&mut source)
                .expect("版本包解析失败");
        let Some(bundle) = pack.take_block_json_bundle() else {
            eprintln!("[skip] 包无 block bundle，跳过测试");
            return;
        };
        let (snapshot, _) = sc_block::block_json::compile_bundle(
            &bundle,
            "test",
            &sc_packloader::block::BlockBundleBudgets::default(),
            &sc_block::block_json::global_preexisting_lookup,
            &|_| true,
        )
        .expect("bundle 应编译");
        // Live snapshot validates (unknown stations, dangling lit targets).
        validate_workstations(&snapshot).expect("工作站声明应合法");
        // Production path: registry built FROM the snapshot, so dense ids
        // align with column rows (same invariant `mining_of` relies on).
        let palette = sc_block::registry::BlockStateRegistry::from_block_snapshot(&snapshot);

        /// Expected decl per workstation block: station tag, container byte,
        /// window bit, grid, slots in decl order, and process
        /// (cook ticks, fuel, lit bit, lit variant).
        struct Expected {
            station: &'static str,
            container: i8,
            window: bool,
            grid: Option<u8>,
            slots: &'static [(SlotRole, u8)],
            process: Option<(u16, bool, bool, &'static str)>,
        }
        fn expected(name: &str) -> Option<Expected> {
            use SlotRole::{Fuel, Input, Result};
            match name {
                "minecraft:furnace" => Some(Expected {
                    station: "furnace",
                    container: 2,
                    window: true,
                    grid: None,
                    slots: &[(Input, 25), (Fuel, 24), (Result, 26)],
                    process: Some((200, true, false, "minecraft:lit_furnace")),
                }),
                "minecraft:lit_furnace" => Some(Expected {
                    station: "furnace",
                    container: 2,
                    window: true,
                    grid: None,
                    slots: &[(Input, 25), (Fuel, 24), (Result, 26)],
                    process: Some((200, true, true, "minecraft:furnace")),
                }),
                "minecraft:blast_furnace" => Some(Expected {
                    station: "blast_furnace",
                    container: 27,
                    window: true,
                    grid: None,
                    slots: &[(Input, 45), (Fuel, 24), (Result, 26)],
                    process: Some((100, true, false, "minecraft:lit_blast_furnace")),
                }),
                "minecraft:lit_blast_furnace" => Some(Expected {
                    station: "blast_furnace",
                    container: 27,
                    window: true,
                    grid: None,
                    slots: &[(Input, 45), (Fuel, 24), (Result, 26)],
                    process: Some((100, true, true, "minecraft:blast_furnace")),
                }),
                "minecraft:smoker" => Some(Expected {
                    station: "smoker",
                    container: 28,
                    window: true,
                    grid: None,
                    slots: &[(Input, 46), (Fuel, 24), (Result, 26)],
                    process: Some((100, true, false, "minecraft:lit_smoker")),
                }),
                "minecraft:lit_smoker" => Some(Expected {
                    station: "smoker",
                    container: 28,
                    window: true,
                    grid: None,
                    slots: &[(Input, 46), (Fuel, 24), (Result, 26)],
                    process: Some((100, true, true, "minecraft:smoker")),
                }),
                "minecraft:crafting_table" => Some(Expected {
                    station: "crafting_table",
                    container: 1,
                    window: true,
                    grid: Some(3),
                    slots: &[],
                    process: None,
                }),
                "minecraft:stonecutter_block" => Some(Expected {
                    station: "stonecutter",
                    container: 29,
                    window: true,
                    grid: None,
                    slots: &[],
                    process: None,
                }),
                // Addressable but never opening (open gate reads `window`).
                "minecraft:brewing_stand" => Some(Expected {
                    station: "brewing_stand",
                    container: 4,
                    window: false,
                    grid: None,
                    slots: &[],
                    process: None,
                }),
                "minecraft:smithing_table" => Some(Expected {
                    station: "smithing_table",
                    container: 33,
                    window: false,
                    grid: None,
                    slots: &[],
                    process: None,
                }),
                "minecraft:campfire" => Some(Expected {
                    station: "campfire",
                    container: 0,
                    window: false,
                    grid: None,
                    slots: &[],
                    process: None,
                }),
                "minecraft:soul_campfire" => Some(Expected {
                    station: "soul_campfire",
                    container: 0,
                    window: false,
                    grid: None,
                    slots: &[],
                    process: None,
                }),
                _ => None,
            }
        }

        let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
        for state in snapshot.states.iter() {
            // Production path: palette hash -> registry dense id -> column.
            let dense = palette
                .by_runtime_id(sc_world::chunk::BlockRuntimeId(state.hash))
                .unwrap_or_else(|| panic!("hash {} 无 dense id", state.hash));
            let entry = palette.describe(sc_world::chunk::BlockRuntimeId(state.hash));
            let Some(entry) = entry.as_ref() else {
                continue;
            };
            let name = entry.name.as_ref();
            let index = dense.0;
            let Some(want) = expected(name) else {
                // No decl expected for non-workstations (this also pins
                // the `stonecutter` vs `stonecutter_block` trap: only the
                // block form serves the station).
                assert!(
                    snapshot.workstation_of(index).is_none(),
                    "{name} 不应有 workstation 声明"
                );
                continue;
            };
            covered.insert(name.to_string());
            let decl = snapshot
                .workstation_of(index)
                .unwrap_or_else(|| panic!("{name} 应有 workstation 声明"));
            assert_eq!(decl.station.as_ref(), want.station, "{name} station");
            // The tag must resolve (load validation guarantees it; double
            // entry here so the gate fails loudly even if validation is
            // ever bypassed in a test path).
            assert_ne!(
                sc_recipe::StationKind::from_tag(&decl.station),
                sc_recipe::StationKind::Unknown,
                "{name} station 未知"
            );
            assert_eq!(decl.container, want.container, "{name} container");
            assert_eq!(decl.window, want.window, "{name} window");
            assert_eq!(decl.grid, want.grid, "{name} grid");
            let mut slots: Vec<(SlotRole, u8)> = decl
                .slots
                .iter()
                .map(|slot| (slot.role, slot.container))
                .collect();
            slots.sort_by_key(|(role, _)| *role as u8);
            let mut want_slots = want.slots.to_vec();
            want_slots.sort_by_key(|(role, _)| *role as u8);
            assert_eq!(slots, want_slots, "{name} slots");
            match (snapshot.process_of(index), want.process) {
                (Some(process), Some((cook, fuel, lit, variant))) => {
                    assert_eq!(process.cook_ticks, cook, "{name} cook_ticks");
                    assert_eq!(process.fuel, fuel, "{name} fuel");
                    assert_eq!(process.lit, lit, "{name} lit");
                    assert_eq!(
                        process.lit_variant.as_deref(),
                        Some(variant),
                        "{name} lit_variant"
                    );
                }
                (None, None) => {}
                (got, want) => panic!("{name} process 不一致：实际 {got:?}，期望 {want:?}"),
            }
        }
        for name in [
            "minecraft:furnace",
            "minecraft:lit_furnace",
            "minecraft:blast_furnace",
            "minecraft:lit_blast_furnace",
            "minecraft:smoker",
            "minecraft:lit_smoker",
            "minecraft:crafting_table",
            "minecraft:stonecutter_block",
            "minecraft:brewing_stand",
            "minecraft:smithing_table",
            "minecraft:campfire",
            "minecraft:soul_campfire",
        ] {
            assert!(covered.contains(name), "{name} 未覆盖");
        }
    }
}
