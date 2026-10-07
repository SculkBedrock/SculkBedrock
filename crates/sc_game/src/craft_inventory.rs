//! Owner-local inventory, crafting-grid and cursor transactions.
use crate::container::ContainerOpen;
use crate::crafting::{CraftRequestEvent, SharedItemTags, SharedRecipeRegistry};
use parking_lot::Mutex;
use sc_ecs::{component::Component, entity::EntityId, world::World};
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
use sc_log::t_log;
use sc_recipe::{MatchInput, RecipeBody, StationKind};
use std::collections::{BTreeSet, HashMap, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CraftContainer {
    Inventory,
    Hotbar,
    CombinedInventory,
    Grid,
    Cursor,
    Output,
}

#[derive(Clone, Copy, Debug)]
pub struct CraftSlotRef {
    pub container: CraftContainer,
    pub slot: u8,
    pub net_id: i32,
    /// FullContainerName echo (window id for grid cells only).
    pub dynamic: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum InventoryAction {
    Move {
        amount: u8,
        source: CraftSlotRef,
        destination: CraftSlotRef,
    },
    Swap {
        source: CraftSlotRef,
        destination: CraftSlotRef,
    },
    Craft {
        recipe_id: u32,
        times: u8,
        automatic: bool,
    },
    Consume,
    Create,
    Results,
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct CraftSlotUpdate {
    pub container: CraftContainer,
    pub slot: u8,
    pub stack: ItemStack,
    pub net_id: i32,
    /// Same echo for responses (grid only).
    pub dynamic: Option<u32>,
}

#[derive(Clone)]
struct State {
    extra: Vec<ItemStack>,
    ids: HashMap<usize, (u16, u32, i32)>,
    receipts: HashMap<u64, bool>,
    order: VecDeque<u64>,
    next_id: i32,
    /// Slot indices dirtied by the last successful execution.
    /// `updates()` returns only these slots.
    dirty: BTreeSet<usize>,
    /// Grid echo dynamic, recorded from requests and cleared on close.
    grid_dynamic: Option<u32>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            extra: vec![ItemStack::empty(); 11],
            ids: HashMap::new(),
            receipts: HashMap::new(),
            order: VecDeque::new(),
            next_id: 1,
            dirty: BTreeSet::new(),
            grid_dynamic: None,
        }
    }
}

#[derive(Component, Default)]
pub struct CraftInventory {
    state: Mutex<State>,
}

fn ensure(world: &World, entity: EntityId) -> Option<std::sync::Arc<CraftInventory>> {
    if world.get_component::<CraftInventory>(&entity).is_none() {
        world.add_component(&entity, CraftInventory::default());
    }
    world.get_component::<CraftInventory>(&entity)
}

fn index(slot: CraftSlotRef, main: usize, table: bool) -> Option<usize> {
    match slot.container {
        CraftContainer::Inventory | CraftContainer::CombinedInventory
            if (slot.slot as usize) < main =>
        {
            Some(slot.slot as usize)
        }
        CraftContainer::Hotbar if slot.slot < 9 && (slot.slot as usize) < main => {
            Some(slot.slot as usize)
        }
        CraftContainer::Grid => {
            let start = if table { 32 } else { 28 };
            let count = if table { 9 } else { 4 };
            if slot.slot >= start && slot.slot < start + count {
                Some(main + (slot.slot - start) as usize)
            } else {
                None
            }
        }
        CraftContainer::Cursor if slot.slot == 0 => Some(main + 9),
        CraftContainer::Output if slot.slot == 0 || slot.slot == 50 => Some(main + 10),
        _ => None,
    }
}

fn same_item(a: &ItemStack, b: &ItemStack) -> bool {
    a.runtime_id == b.runtime_id && a.damage == b.damage && a.block_runtime_id == b.block_runtime_id
}

/// Current crafting context: workstation type plus 3x3 flag.
///
/// Falls back to 2x2 with no workstation window; returns `None` (reject)
/// when the player moved away, changed worlds, or the block changed.
fn station(world: &World, entity: EntityId) -> Option<(StationKind, bool)> {
    let Some(open) = world.get_component::<ContainerOpen>(&entity) else {
        return Some((StationKind::CraftingTable, false));
    };
    let (station, open_world_id, open_position) = open.workstation()?;
    let id = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity)?;
    let transform = world.get_component::<sc_entity::motion::Transform>(&entity)?;
    let pos = transform.read().position;
    let distance = (pos.x - open_position.0 as f32 - 0.5).powi(2)
        + (pos.y - open_position.1 as f32 - 0.5).powi(2)
        + (pos.z - open_position.2 as f32 - 0.5).powi(2);
    if *id.as_ref() != open_world_id
        || distance > 36.0
        || crate::crafting::station_block_kind(world, &id, open_position) != Some(station)
    {
        return None;
    }
    Some((station, station == StationKind::CraftingTable))
}

fn input(stack: &ItemStack, items: &ItemRegistry) -> Option<MatchInput> {
    let definition = items.get(stack.runtime_id)?;
    Some(MatchInput::new(
        definition.name.to_string(),
        stack.damage as i32,
        stack.count,
    ))
}

fn craft(
    world: &World,
    entity: EntityId,
    stacks: &mut [ItemStack],
    main: usize,
    recipe_id: u32,
    times: u8,
    automatic: bool,
    items: &ItemRegistry,
) -> bool {
    if times == 0 || times > 64 {
        return false;
    }
    let Some(shared) = world.get_resource::<SharedRecipeRegistry>() else {
        return false;
    };
    let snapshot = shared.snapshot();
    let Some(recipe) = recipe_id
        .checked_sub(2)
        .and_then(|id| snapshot.get_by_index(id))
    else {
        return false;
    };
    let Some((station, table)) = station(world, entity) else {
        return false;
    };
    if !recipe.stations.is_empty() && !recipe.stations.contains(&station) {
        return false;
    }
    if !crate::crafting::recipe_allowed(world, entity, recipe) {
        return false;
    }
    let tags = world
        .get_resource::<SharedItemTags>()
        .map(|tags| tags.resolver())
        .unwrap_or_else(sc_recipe::MapTagResolver::empty);
    let size = if table { 3 } else { 2 };
    let grid: Vec<Option<MatchInput>> = stacks[main..main + size * size]
        .iter()
        .map(|stack| {
            if stack.is_empty() {
                None
            } else {
                input(stack, items)
            }
        })
        .collect();
    let requirements = match &recipe.body {
        RecipeBody::Shaped(body) => {
            if body.width as usize > size || body.height as usize > size {
                return false;
            }
            if !automatic
                && !sc_recipe::match_shaped(
                    recipe, &grid, size as u8, size as u8, station, &tags, 1024,
                )
            {
                return false;
            }
            body.grid
                .iter()
                .flatten()
                .map(|spec| (spec.clone(), spec.count))
                .collect::<Vec<_>>()
        }
        RecipeBody::Shapeless(body) => {
            if station == StationKind::CraftingTable && body.ingredients.len() > size * size {
                return false;
            }
            if !automatic
                && !sc_recipe::match_shapeless(
                    recipe,
                    &grid.iter().flatten().cloned().collect::<Vec<_>>(),
                    station,
                    &tags,
                    1024,
                )
            {
                return false;
            }
            body.ingredients
                .iter()
                .map(|spec| (spec.clone(), spec.count))
                .collect()
        }
        _ => return false,
    };
    let outputs = match &recipe.body {
        RecipeBody::Shaped(body) => &body.results,
        RecipeBody::Shapeless(body) => &body.results,
        _ => return false,
    };
    // The transient output slot represents one stack; multi-output recipes stay fail-closed.
    if outputs.len() != 1 {
        return false;
    }
    let output = &outputs[0];
    let Some(id) = items.runtime_id_by_name(&output.identifier) else {
        return false;
    };
    let Some(count) = output.count.checked_mul(times as u16) else {
        return false;
    };
    let mut produced = ItemStack::new(id, count);
    produced.damage = output.data.unwrap_or(0).max(0) as u32;
    produced.block_runtime_id = items.block_runtime_id(id).unwrap_or(0);
    let old = stacks[main + 10].clone();
    if (!old.is_empty() && !same_item(&old, &produced))
        || old.count.saturating_add(count) > items.max_stack_size(id)
    {
        return false;
    }
    let requirements = requirements
        .into_iter()
        .map(|(spec, count)| count.checked_mul(times as u16).map(|count| (spec, count)))
        .collect::<Option<Vec<_>>>();
    let Some(requirements) = requirements else {
        return false;
    };
    let range = if automatic {
        0..main
    } else {
        main..main + size * size
    };
    let live = range
        .filter_map(|i| {
            if stacks[i].is_empty() {
                None
            } else {
                input(&stacks[i], items).map(|input| (i, input))
            }
        })
        .collect::<Vec<_>>();
    let Some(plan) = sc_recipe::plan_consumption(&requirements, &live, &tags, 1024) else {
        return false;
    };
    for (i, count) in plan {
        stacks[i].count -= count;
        stacks[i].clear_if_empty();
    }
    produced.count += old.count;
    stacks[main + 10] = produced;
    true
}

pub fn execute(world: &World, request: &CraftRequestEvent) -> bool {
    let entity = request.entity;
    let (Some(inventory), Some(items), Some(component)) = (
        world.get_component::<PlayerInventory>(&entity),
        world.get_resource::<ItemRegistry>(),
        ensure(world, entity),
    ) else {
        return false;
    };
    let mut state = component.state.lock();
    if let Some(success) = state.receipts.get(&request.operation_id) {
        return *success;
    }
    let before = inventory.snapshot();
    let main = before.len();
    if main > 36 {
        return false;
    }
    let table = world
        .get_component::<ContainerOpen>(&entity)
        .is_some_and(|open| open.is_station(StationKind::CraftingTable));
    let mut stacks = before.clone();
    let old_extra = state.extra.clone();
    stacks.extend(old_extra.iter().cloned());
    let valid_ref = |reference: CraftSlotRef, i: usize, current: &[ItemStack]| {
        // Stack network id check: mismatch means client>0 and server id differs;
        // zero/negative ids always pass.
        if reference.net_id <= 0 {
            return true;
        }
        match state.ids.get(&i) {
            // Issued ids for the same item must match; stale in-flight ids
            // reject the whole packet.
            Some((runtime, damage, id))
                if *runtime == current[i].runtime_id && *damage == current[i].damage =>
            {
                *id == reference.net_id
            }
            // Slots without issued ids only check non-empty.
            _ => {
                if i < main {
                    !current[i].is_empty()
                } else {
                    false
                }
            }
        }
    };
    let op_id = request.operation_id;
    // Grid dynamic seen in this packet, recorded for response echo.
    let mut grid_dynamic_seen: Option<Option<u32>> = None;
    let success = (|| {
        let mut crafted = false;
        for action in &request.actions {
            match action {
                InventoryAction::Move {
                    amount,
                    source,
                    destination,
                } => {
                    let (Some(src), Some(dst)) = (
                        index(*source, main, table),
                        index(*destination, main, table),
                    ) else {
                        log::debug!(
                            "[craft] op={op_id} Move rejected: slot map failed src={:?} dst={:?} table={table} main={main}",
                            source,
                            destination,
                        );
                        return false;
                    };
                    if !valid_ref(*source, src, &stacks)
                        || !valid_ref(*destination, dst, &stacks)
                        || dst == main + 10
                    {
                        log::debug!(
                            "[craft] op={op_id} Move rejected: net_id check failed src={:?}(i={src}) dst={:?}(i={dst})",
                            source,
                            destination,
                        );
                        return false;
                    }
                    if src == dst || *amount == 0 || stacks[src].count < *amount as u16 {
                        log::debug!(
                            "[craft] op={op_id} Move rejected: illegal amount src_count={} amount={amount}",
                            stacks[src].count,
                        );
                        return false;
                    }
                    if !stacks[dst].is_empty() && !same_item(&stacks[src], &stacks[dst]) {
                        log::debug!("[craft] op={op_id} Move rejected: target slot item mismatch");
                        return false;
                    }
                    if stacks[dst].count.saturating_add(*amount as u16)
                        > items.max_stack_size(stacks[src].runtime_id)
                    {
                        log::debug!("[craft] op={op_id} Move rejected: over stack cap");
                        return false;
                    }
                    for reference in [source, destination] {
                        if reference.container == CraftContainer::Grid {
                            grid_dynamic_seen = Some(reference.dynamic);
                        }
                    }
                    if stacks[dst].is_empty() {
                        stacks[dst] = ItemStack {
                            count: 0,
                            ..stacks[src].clone()
                        };
                    }
                    stacks[dst].count += *amount as u16;
                    stacks[src].count -= *amount as u16;
                    stacks[src].clear_if_empty();
                }
                InventoryAction::Swap {
                    source,
                    destination,
                } => {
                    let (Some(src), Some(dst)) = (
                        index(*source, main, table),
                        index(*destination, main, table),
                    ) else {
                        log::debug!(
                            "[craft] op={op_id} Swap rejected: slot map failed src={:?} dst={:?}",
                            source,
                            destination,
                        );
                        return false;
                    };
                    if !valid_ref(*source, src, &stacks) || !valid_ref(*destination, dst, &stacks) {
                        log::debug!("[craft] op={op_id} Swap rejected: net_id check failed");
                        return false;
                    }
                    if src >= main + 10 || dst >= main + 10 {
                        log::debug!("[craft] op={op_id} Swap rejected: output slot not swappable");
                        return false;
                    }
                    for reference in [source, destination] {
                        if reference.container == CraftContainer::Grid {
                            grid_dynamic_seen = Some(reference.dynamic);
                        }
                    }
                    stacks.swap(src, dst);
                }
                InventoryAction::Craft {
                    recipe_id,
                    times,
                    automatic,
                } => {
                    if crafted
                        || !craft(
                            world,
                            entity,
                            &mut stacks,
                            main,
                            *recipe_id,
                            *times,
                            *automatic,
                            &items,
                        )
                    {
                        log::debug!(
                            "[craft] op={op_id} rejected: recipe execution failed recipe={recipe_id} times={times} auto={automatic}"
                        );
                        return false;
                    }
                    crafted = true;
                }
                InventoryAction::Consume | InventoryAction::Create | InventoryAction::Results => {
                    if !crafted {
                        log::debug!("[craft] op={op_id} rejected: Consume/Create/Results without leading Craft");
                        return false;
                    }
                }
                InventoryAction::Unsupported => {
                    log::debug!("[craft] op={op_id} rejected: contains Unsupported actions");
                    return false;
                }
            }
        }
        !request.actions.is_empty()
    })();
    log::debug!(
        "[craft] op={op_id} execution result success={success} actions={:?}",
        request.actions,
    );
    if success {
        let old: Vec<ItemStack> = before.iter().cloned().chain(old_extra.iter().cloned()).collect();
        state.dirty.extend(
            stacks
                .iter()
                .enumerate()
                .filter(|(i, stack)| old.get(*i) != Some(stack))
                .map(|(i, _)| i),
        );
        if let Some(dynamic) = grid_dynamic_seen {
            state.grid_dynamic = dynamic;
        }
        *inventory.slots.write() = stacks[..main].to_vec();
        state.extra = stacks[main..].to_vec();
        crate::crafting::bump_inventory_revision(world, &entity);
    }
    if state.order.len() >= 128 {
        if let Some(old) = state.order.pop_front() {
            state.receipts.remove(&old);
        }
    }
    state.order.push_back(request.operation_id);
    state.receipts.insert(request.operation_id, success);
    success
}

pub fn updates(world: &World, entity: EntityId) -> Vec<CraftSlotUpdate> {
    let (Some(inventory), Some(component)) = (
        world.get_component::<PlayerInventory>(&entity),
        ensure(world, entity),
    ) else {
        return Vec::new();
    };
    let mut state = component.state.lock();
    let main = inventory.snapshot();
    let len = main.len();
    let table = world
        .get_component::<ContainerOpen>(&entity)
        .is_some_and(|open| open.is_station(StationKind::CraftingTable));
    let mut stacks = main;
    stacks.extend(state.extra.clone());
    // Only dirty slots are returned; replays return nothing.
    let dirty = std::mem::take(&mut state.dirty);
    let mut updates = Vec::new();
    for i in dirty {
        let Some(stack) = stacks.get(i).cloned() else {
            continue;
        };
        if i >= len && i < len + 9 && i - len >= if table { 9 } else { 4 } {
            continue;
        }
        let (container, slot) = if i < len {
            // Hotbar is 0-8, inventory is 9 and up; responses use the split
            // containers (28/29).
            if i < 9 {
                (CraftContainer::Hotbar, i as u8)
            } else {
                (CraftContainer::Inventory, i as u8)
            }
        } else if i < len + 9 {
            (
                CraftContainer::Grid,
                (if table { 32 } else { 28 }) + (i - len) as u8,
            )
        } else if i == len + 9 {
            (CraftContainer::Cursor, 0)
        } else {
            (CraftContainer::Output, 50)
        };
        let net_id = if stack.is_empty() {
            state.ids.remove(&i);
            0
        } else {
            match state.ids.get(&i).copied() {
                Some((id, damage, net_id)) if id == stack.runtime_id && damage == stack.damage => {
                    net_id
                }
                _ => {
                    let id = state.next_id;
                    state.next_id = state.next_id.saturating_add(1);
                    state.ids.insert(i, (stack.runtime_id, stack.damage, id));
                    id
                }
            }
        };
        updates.push(CraftSlotUpdate {
            container,
            slot,
            stack,
            net_id,
            dynamic: if container == CraftContainer::Grid {
                state.grid_dynamic
            } else {
                None
            },
        });
    }
    updates
}

/// Current grid snapshot for window sync: read-only, no net id allocation.
/// Workstation grid is 9 slots (32-40), 2x2 is 4 slots (28-31).
pub fn grid_snapshot(world: &World, entity: EntityId) -> Vec<(u8, ItemStack)> {
    let Some(component) = world.get_component::<CraftInventory>(&entity) else {
        return Vec::new();
    };
    let table = world
        .get_component::<ContainerOpen>(&entity)
        .is_some_and(|open| open.is_station(StationKind::CraftingTable));
    let state = component.state.lock();
    let (start, count) = if table { (32u8, 9) } else { (28u8, 4) };
    (0..count)
        .map(|k| {
            (
                start + k as u8,
                state.extra.get(k).cloned().unwrap_or_else(ItemStack::empty),
            )
        })
        .collect()
}

pub fn return_items(world: &World, entity: EntityId) -> bool {    let Some(component) = world.get_component::<CraftInventory>(&entity) else {
        return true;
    };
    let (Some(inventory), Some(items)) = (
        world.get_component::<PlayerInventory>(&entity),
        world.get_resource::<ItemRegistry>(),
    ) else {
        return false;
    };
    let mut state = component.state.lock();
    let temporary = PlayerInventory::new(inventory.len());
    *temporary.slots.write() = inventory.snapshot();
    for stack in &state.extra {
        if !stack.is_empty() && temporary.insert_tracked(stack.clone(), &items).0 != 0 {
            log::warn!("{}", t_log!("console.game.craft_no_return_space", entity = format!("{entity:?}")));
            return false;
        }
    }
    *inventory.slots.write() = temporary.snapshot();
    state.extra.fill(ItemStack::empty());
    state.ids.retain(|index, _| *index < inventory.len());
    state.grid_dynamic = None;
    crate::crafting::bump_inventory_revision(world, &entity);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn move_into_grid_craft_take_result_and_replay_are_atomic() {
        let world = World::new();
        let items = ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(1, "minecraft:log"));
        items.upsert(sc_item::ItemDefinition::new(2, "minecraft:planks"));
        world.insert_resource(items);
        let raw = serde_json::json!({"format_version":"1.12","minecraft:recipe_shapeless": {
            "description":{"identifier":"minecraft:test"},"tags":["crafting_table"],
            "ingredients":[{"item":"minecraft:log"}],"result":{"item":"minecraft:planks","count":4}
        }});
        let bytes = serde_json::to_vec(&raw).unwrap();
        let source = sc_recipe::SourceRecipe::new("test", 0, "test.json", "1.12", raw, &bytes);
        let (snapshot, _) = sc_recipe::RecipeRegistrySnapshot::compile(
            &[source],
            &sc_recipe::CompileBudgets::default(),
            false,
        )
        .unwrap();
        world.insert_resource(SharedRecipeRegistry::new(snapshot));
        let inventory = PlayerInventory::new(36);
        inventory.set(0, ItemStack::new(1, 5));
        let entity = world.spawn(inventory);
        let reference = |container, slot, net_id| CraftSlotRef {
            container,
            slot,
            net_id,
            dynamic: None,
        };
        let request = |id, actions| CraftRequestEvent {
            entity,
            recipe_network_id: 0,
            requested_count: 0,
            operation_id: (id as i32) as u64,
            actions,
        };
        assert!(execute(
            &world,
            &request(
                -1,
                vec![InventoryAction::Move {
                    amount: 1,
                    source: reference(CraftContainer::Inventory, 0, 0),
                    destination: reference(CraftContainer::Grid, 30, 0)
                }]
            )
        ));
        let slots = updates(&world, entity);
        assert_eq!(
            slots
                .iter()
                .find(|slot| slot.container == CraftContainer::Grid && slot.slot == 30)
                .unwrap()
                .stack
                .count,
            1
        );
        let craft = request(
            -3,
            vec![
                InventoryAction::Craft {
                    recipe_id: 2,
                    times: 1,
                    automatic: false,
                },
                InventoryAction::Consume,
                InventoryAction::Move {
                    amount: 4,
                    source: reference(CraftContainer::Output, 50, -3),
                    destination: reference(CraftContainer::Inventory, 1, 0),
                },
            ],
        );
        assert!(execute(&world, &craft));
        assert!(execute(&world, &craft));
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert_eq!(inventory.get(0).unwrap().count, 4);
        assert_eq!(inventory.get(1).unwrap().count, 4);
        assert!(updates(&world, entity)
            .iter()
            .filter(|slot| slot.container == CraftContainer::Grid)
            .all(|slot| slot.stack.is_empty()));
        assert!(!execute(
            &world,
            &request(
                -5,
                vec![InventoryAction::Move {
                    amount: 255,
                    source: reference(CraftContainer::Inventory, 0, 0),
                    destination: reference(CraftContainer::Grid, 30, 0)
                }]
            )
        ));
        assert_eq!(inventory.get(0).unwrap().count, 4);
    }

    /// Real client session: take then place, echoing server net ids;
    /// non-positive net id references always pass.
    #[test]
    fn take_to_cursor_and_place_to_backpack_with_netid_echo() {
        let world = World::new();
        let items = ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(1, "minecraft:log"));
        world.insert_resource(items);
        let inventory = PlayerInventory::new(36);
        inventory.set(9, ItemStack::new(1, 5));
        let entity = world.spawn(inventory);
        let reference = |container, slot, net_id| CraftSlotRef {
            container,
            slot,
            net_id,
            dynamic: None,
        };
        let request = |id, actions| CraftRequestEvent {
            entity,
            recipe_network_id: 0,
            requested_count: 0,
            operation_id: (id as i32) as u64,
            actions,
        };
        use CraftContainer as C;
        // Take 5 from inventory slot 9 to the cursor.
        assert!(execute(
            &world,
            &request(
                -1,
                vec![InventoryAction::Move {
                    amount: 5,
                    source: reference(C::Inventory, 9, 0),
                    destination: reference(C::Cursor, 0, 0),
                }]
            )
        ));
        let cursor_updates = updates(&world, entity);
        // Only dirty slots return; inventory slots use the Inventory container.
        assert_eq!(cursor_updates.len(), 2);
        assert!(cursor_updates.iter().any(|slot| slot.container == C::Inventory
            && slot.slot == 9
            && slot.stack.is_empty()));
        let cursor_net = cursor_updates
            .iter()
            .find(|slot| slot.container == C::Cursor)
            .unwrap()
            .net_id;
        assert!(cursor_net > 0);
        // Place into inventory slot 10, echoing the server net id.
        assert!(execute(
            &world,
            &request(
                -3,
                vec![InventoryAction::Move {
                    amount: 5,
                    source: reference(C::Cursor, 0, cursor_net),
                    destination: reference(C::Inventory, 10, 0),
                }]
            )
        ));
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert!(inventory.get(9).unwrap().is_empty());
        assert_eq!(inventory.get(10).unwrap().count, 5);
        // Hotbar slots (0-8) use the Hotbar container.
        inventory.set(5, ItemStack::new(1, 3));
        assert!(execute(
            &world,
            &request(
                -7,
                vec![InventoryAction::Move {
                    amount: 1,
                    source: reference(C::Hotbar, 5, 0),
                    destination: reference(C::Inventory, 11, 0),
                }]
            )
        ));
        let hotbar_updates = updates(&world, entity);
        assert!(hotbar_updates.iter().any(|slot| slot.container == C::Hotbar
            && slot.slot == 5
            && slot.stack.count == 2));
        // Negative net ids pass: move 2 back to slot 9.
        assert!(execute(
            &world,
            &request(
                -5,
                vec![InventoryAction::Move {
                    amount: 2,
                    source: reference(C::Inventory, 10, -5),
                    destination: reference(C::Inventory, 9, -5),
                }]
            )
        ));
        assert_eq!(inventory.get(10).unwrap().count, 3);
        assert_eq!(inventory.get(9).unwrap().count, 2);
    }

    /// Dark oak planks in 2x2 with a pseudo-name ingredient craft once.
    #[test]
    fn pseudo_family_planks_craft_workbench() {
        let world = World::new();
        let items = ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(2, "minecraft:dark_oak_planks"));
        items.upsert(sc_item::ItemDefinition::new(3, "minecraft:crafting_table"));
        world.insert_resource(items);
        let raw = serde_json::json!({"format_version":"1.12","minecraft:recipe_shaped": {
            "description":{"identifier":"minecraft:crafting_table"},
            "tags":["crafting_table"],
            "pattern":["AA","AA"],
            "key":{"A":{"item":"minecraft:planks"}},
            "result":{"item":"minecraft:crafting_table"}
        }});
        let bytes = serde_json::to_vec(&raw).unwrap();
        let source = sc_recipe::SourceRecipe::new("test", 0, "test.json", "1.12", raw, &bytes);
        let (snapshot, _) = sc_recipe::RecipeRegistrySnapshot::compile(
            &[source],
            &sc_recipe::CompileBudgets::default(),
            false,
        )
        .unwrap();
        world.insert_resource(SharedRecipeRegistry::new(snapshot));
        world.insert_resource(SharedItemTags::new(std::collections::HashMap::from([(
            "minecraft:planks".to_string(),
            vec![
                "minecraft:oak_planks".to_string(),
                "minecraft:dark_oak_planks".to_string(),
            ],
        )])));
        let inventory = PlayerInventory::new(36);
        inventory.set(9, ItemStack::new(2, 4));
        let entity = world.spawn(inventory);
        let reference = |container, slot, net_id| CraftSlotRef {
            container,
            slot,
            net_id,
            dynamic: None,
        };
        let request = |id, actions| CraftRequestEvent {
            entity,
            recipe_network_id: 0,
            requested_count: 0,
            operation_id: (id as i32) as u64,
            actions,
        };
        use CraftContainer as C;
        // Four planks fill the 2x2 grid (28-31).
        for (id, slot) in [28u8, 29, 30, 31].iter().enumerate() {
            assert!(execute(
                &world,
                &request(
                    -(1 + id as i32 * 2),
                    vec![InventoryAction::Move {
                        amount: 1,
                        source: reference(C::Inventory, 9, 0),
                        destination: reference(C::Grid, *slot, 0),
                    }]
                )
            ));
        }
        // Craft and take the output.
        assert!(execute(
            &world,
            &request(
                -11,
                vec![
                    InventoryAction::Craft {
                        recipe_id: 2,
                        times: 1,
                        automatic: false,
                    },
                    InventoryAction::Consume,
                    InventoryAction::Move {
                        amount: 1,
                        source: reference(C::Output, 50, -11),
                        destination: reference(C::Inventory, 10, 0),
                    },
                ]
            )
        ));
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert_eq!(inventory.get(10).unwrap().runtime_id, 3);
    }
}
