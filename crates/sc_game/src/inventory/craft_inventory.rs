//! Owner-local inventory, crafting-grid and cursor transactions.
use crate::inventory::ContainerOpen;
use crate::inventory::crafting::{CraftRequestEvent, SharedItemTags, SharedRecipeRegistry};
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
    /// Furnace window slot (`slot` = 0 input / 1 fuel / 2 result).
    /// Ingredient container enums differ per station (25/45/46) but all map
    /// to slot 0; the open station validates at execution.
    Furnace,
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
    /// Creative-mode materialization (client `CraftCreative` take): grants
    /// the table template for `creative_id` into the scratch Output slot,
    /// drained by the request's own Moves. Only valid for creative players;
    /// the grant must be fully consumed within the same request (a leftover
    /// Output rejects the whole packet, so grants cannot leak across
    /// requests into later free takes).
    Grant {
        creative_id: u32,
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

/// Fuel check through the item component table (missing table or
/// component = not fuel, never a code default).
fn is_fuel(
    fuel: Option<&sc_packloader::item::ItemComponentTable>,
    stack: &ItemStack,
) -> bool {
    fuel.is_some_and(|table| crate::furnace::fuel_ticks(table, stack) > 0)
}

fn is_furnace_slot(reference: &CraftSlotRef) -> bool {
    reference.container == CraftContainer::Furnace
}

/// Furnace-side endpoint validation: slot range, window binding (None or the
/// open window id) and stack presence for id-checked requests. Furnace slots
/// carry no issued net ids, so presence is the only server-side check.
fn furnace_slot(
    reference: &CraftSlotRef,
    window_id: u8,
    slots: &[ItemStack; 3],
) -> Option<usize> {
    let index = reference.slot as usize;
    if index >= crate::furnace::SLOT_COUNT {
        return None;
    }
    if reference
        .dynamic
        .is_some_and(|dynamic| dynamic != window_id as u32)
    {
        return None;
    }
    if reference.net_id > 0 && slots[index].is_empty() {
        return None;
    }
    Some(index)
}

/// Move with at least one furnace endpoint (vanilla slot rules: the result
/// slot is take-only, the fuel slot takes fuel only; input takes anything).
/// Player endpoints reuse the caller's net-id validation.
#[allow(clippy::too_many_arguments)]
fn furnace_move(
    valid_player: &dyn Fn(CraftSlotRef, usize, &[ItemStack]) -> bool,
    items: &ItemRegistry,
    fuel: Option<&sc_packloader::item::ItemComponentTable>,
    main: usize,
    table: bool,
    _station: StationKind,
    window_id: u8,
    stacks: &mut [ItemStack],
    slots: &mut [ItemStack; 3],
    amount: u8,
    source: &CraftSlotRef,
    destination: &CraftSlotRef,
    op_id: u64,
) -> bool {
    enum Side {
        Player(usize),
        Furnace(usize),
    }
    let Some(src) = (if is_furnace_slot(source) {
        furnace_slot(source, window_id, slots).map(Side::Furnace)
    } else {
        index(*source, main, table).map(Side::Player)
    }) else {
        log::debug!("[craft] op={op_id} furnace Move rejected: bad source {source:?}");
        return false;
    };
    let Some(dst) = (if is_furnace_slot(destination) {
        furnace_slot(destination, window_id, slots).map(Side::Furnace)
    } else {
        index(*destination, main, table).map(Side::Player)
    }) else {
        log::debug!("[craft] op={op_id} furnace Move rejected: bad destination {destination:?}");
        return false;
    };
    // Result is take-only: it is never a move destination.
    if matches!(dst, Side::Furnace(d) if d == crate::furnace::RESULT_SLOT as usize) {
        log::debug!("[craft] op={op_id} furnace Move rejected: result is take-only");
        return false;
    }
    let (src_stack, dst_stack) = match (&src, &dst) {
        (Side::Player(i), Side::Player(j)) => (&stacks[*i], &stacks[*j]),
        (Side::Player(i), Side::Furnace(d)) => (&stacks[*i], &slots[*d]),
        (Side::Furnace(s), Side::Player(j)) => (&slots[*s], &stacks[*j]),
        (Side::Furnace(s), Side::Furnace(d)) => {
            let (left, right) = if s == d {
                log::debug!("[craft] op={op_id} furnace Move rejected: same slot");
                return false;
            } else {
                (&slots[*s], &slots[*d])
            };
            (left, right)
        }
    };
    if let Side::Player(i) = src {
        if !valid_player(*source, i, stacks) {
            log::debug!("[craft] op={op_id} furnace Move rejected: source net_id check failed");
            return false;
        }
    }
    if let Side::Player(j) = dst {
        if !valid_player(*destination, j, stacks) {
            log::debug!("[craft] op={op_id} furnace Move rejected: destination net_id check failed");
            return false;
        }
    }
    if amount == 0 || src_stack.count < amount as u16 {
        log::debug!("[craft] op={op_id} furnace Move rejected: illegal amount");
        return false;
    }
    if !dst_stack.is_empty() && !same_item(src_stack, dst_stack) {
        log::debug!("[craft] op={op_id} furnace Move rejected: target slot item mismatch");
        return false;
    }
    if dst_stack.count.saturating_add(amount as u16) > items.max_stack_size(src_stack.runtime_id) {
        log::debug!("[craft] op={op_id} furnace Move rejected: over stack cap");
        return false;
    }
    // Fuel slot takes fuel only.
    if matches!(dst, Side::Furnace(d) if d == crate::furnace::FUEL_SLOT as usize)
        && !is_fuel(fuel, src_stack)
    {
        log::debug!(
            "[craft] op={op_id} furnace Move rejected: not fuel ({})",
            items
                .get(src_stack.runtime_id)
                .map(|def| def.name.to_string())
                .unwrap_or_else(|| format!("runtime:{}", src_stack.runtime_id)),
        );
        return false;
    }
    let moving = ItemStack {
        count: 0,
        ..src_stack.clone()
    };
    // Diagnostic: what is about to move (identifier + amount + direction).
    // Keep at debug: one line per furnace move, off unless log hunting.
    log::debug!(
        "[craft] op={op_id} furnace Move applied: {} x{} {} -> {}",
        items
            .get(src_stack.runtime_id)
            .map(|def| def.name.to_string())
            .unwrap_or_else(|| format!("runtime:{}", src_stack.runtime_id)),
        amount,
        match &src {
            Side::Player(i) => format!("player[{i}]"),
            Side::Furnace(d) => format!(
                "furnace[{}]",
                match d {
                    0 => "input",
                    1 => "fuel",
                    _ => "result",
                }
            ),
        },
        match &dst {
            Side::Player(i) => format!("player[{i}]"),
            Side::Furnace(d) => format!(
                "furnace[{}]",
                match d {
                    0 => "input",
                    1 => "fuel",
                    _ => "result",
                }
            ),
        },
    );
    match (&src, &dst) {
        (Side::Player(i), Side::Player(j)) => {
            if stacks[*j].is_empty() {
                stacks[*j] = moving;
            }
            stacks[*j].count += amount as u16;
            stacks[*i].count -= amount as u16;
            stacks[*i].clear_if_empty();
        }
        (Side::Player(i), Side::Furnace(d)) => {
            if slots[*d].is_empty() {
                slots[*d] = moving;
            }
            slots[*d].count += amount as u16;
            stacks[*i].count -= amount as u16;
            stacks[*i].clear_if_empty();
        }
        (Side::Furnace(s), Side::Player(j)) => {
            if stacks[*j].is_empty() {
                stacks[*j] = moving;
            }
            stacks[*j].count += amount as u16;
            slots[*s].count -= amount as u16;
            slots[*s].clear_if_empty();
        }
        (Side::Furnace(s), Side::Furnace(d)) => {
            if slots[*d].is_empty() {
                slots[*d] = moving;
            }
            slots[*d].count += amount as u16;
            slots[*s].count -= amount as u16;
            slots[*s].clear_if_empty();
        }
    }
    true
}

/// Swap with at least one furnace endpoint (same slot rules as moves).
#[allow(clippy::too_many_arguments)]
fn furnace_swap(
    valid_player: &dyn Fn(CraftSlotRef, usize, &[ItemStack]) -> bool,
    _items: &ItemRegistry,
    fuel: Option<&sc_packloader::item::ItemComponentTable>,
    main: usize,
    table: bool,
    _station: StationKind,
    window_id: u8,
    stacks: &mut [ItemStack],
    slots: &mut [ItemStack; 3],
    source: &CraftSlotRef,
    destination: &CraftSlotRef,
    op_id: u64,
) -> bool {
    let resolve = |reference: &CraftSlotRef| {
        if is_furnace_slot(reference) {
            furnace_slot(reference, window_id, slots).map(|index| (true, index))
        } else {
            index(*reference, main, table).map(|index| (false, index))
        }
    };
    let (Some((src_furnace, src)), Some((dst_furnace, dst))) =
        (resolve(source), resolve(destination))
    else {
        log::debug!("[craft] op={op_id} furnace Swap rejected: bad endpoint");
        return false;
    };
    // Result is take-only: Takes move it out, Swaps never touch it.
    if (src_furnace && src == crate::furnace::RESULT_SLOT as usize)
        || (dst_furnace && dst == crate::furnace::RESULT_SLOT as usize)
    {
        log::debug!("[craft] op={op_id} furnace Swap rejected: result is take-only");
        return false;
    }
    let incoming_src = if src_furnace {
        slots[src].clone()
    } else {
        if !valid_player(*source, src, stacks) {
            return false;
        }
        stacks[src].clone()
    };
    let incoming_dst = if dst_furnace {
        slots[dst].clone()
    } else {
        if !valid_player(*destination, dst, stacks) {
            return false;
        }
        stacks[dst].clone()
    };
    // Fuel slot takes fuel only (either direction).
    if dst_furnace
        && dst == crate::furnace::FUEL_SLOT as usize
        && !incoming_src.is_empty()
        && !is_fuel(fuel, &incoming_src)
    {
        return false;
    }
    if src_furnace
        && src == crate::furnace::FUEL_SLOT as usize
        && !incoming_dst.is_empty()
        && !is_fuel(fuel, &incoming_dst)
    {
        return false;
    }
    if src_furnace {
        slots[src] = incoming_dst;
    } else {
        stacks[src] = incoming_dst;
    }
    if dst_furnace {
        slots[dst] = incoming_src;
    } else {
        stacks[dst] = incoming_src;
    }
    true
}

/// Current crafting context: workstation type plus 3x3 flag.
///
/// No open window and the player-inventory window both mean 2x2 crafting
/// with no station block (the client opens its own inventory to use the
/// 2x2 grid). Returns `None` (reject) only when a workstation window is
/// open but stale (moved away, changed worlds, block changed).
fn station(world: &World, entity: EntityId) -> Option<(StationKind, bool)> {
    let Some(open) = world.get_component::<ContainerOpen>(&entity) else {
        return Some((StationKind::InventoryCrafting, false));
    };
    if open.is_player_inventory() {
        return Some((StationKind::InventoryCrafting, false));
    }
    let (station, open_world_id, open_position) = open.workstation()?;
    let id = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity)?;
    let transform = world.get_component::<sc_entity::motion::Transform>(&entity)?;
    let pos = transform.read().position;
    let distance = (pos.x - open_position.0 as f32 - 0.5).powi(2)
        + (pos.y - open_position.1 as f32 - 0.5).powi(2)
        + (pos.z - open_position.2 as f32 - 0.5).powi(2);
    if *id.as_ref() != open_world_id
        || distance > 36.0
        || crate::inventory::crafting::station_block_kind(world, &id, open_position) != Some(station)
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
        log::debug!("[craft] op=? rejected: recipe id {recipe_id} out of range");
        return false;
    };
    let Some((station, table)) = station(world, entity) else {
        log::debug!(
            "[craft] rejected: no station for {} (moved/world/block changed)",
            recipe.identifier
        );
        return false;
    };
    if !recipe.stations.is_empty() && !recipe.stations.contains(&station) {
        log::debug!(
            "[craft] rejected: {} needs stations {:?}, at {:?}",
            recipe.identifier,
            recipe.stations,
            station
        );
        return false;
    }
    if !crate::inventory::crafting::recipe_allowed(world, entity, recipe) {
        log::debug!("[craft] rejected: {} not allowed (unlock/gamemode)", recipe.identifier);
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
                log::debug!(
                    "[craft] rejected: {} shaped {}x{} exceeds {}x{} grid",
                    recipe.identifier,
                    body.width,
                    body.height,
                    size,
                    size
                );
                return false;
            }
            if !automatic
                && !sc_recipe::match_shaped(
                    recipe, &grid, size as u8, size as u8, station, &tags, 1024,
                )
            {
                log::debug!(
                    "[craft] rejected: {} shaped grid mismatch (grid={:?})",
                    recipe.identifier,
                    grid.iter()
                        .map(|cell| cell.as_ref().map(|input| input.identifier.clone()))
                        .collect::<Vec<_>>()
                );
                return false;
            }
            body.grid
                .iter()
                .flatten()
                .map(|spec| (spec.clone(), spec.count))
                .collect::<Vec<_>>()
        }
        RecipeBody::Shapeless(body) => {
            if matches!(
                station,
                StationKind::CraftingTable | StationKind::InventoryCrafting
            ) && body.ingredients.len() > size * size {
                log::debug!(
                    "[craft] rejected: {} shapeless needs {} slots, grid is {}x{}",
                    recipe.identifier,
                    body.ingredients.len(),
                    size,
                    size
                );
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
                log::debug!(
                    "[craft] rejected: {} shapeless multiset mismatch (have={:?})",
                    recipe.identifier,
                    grid.iter()
                        .flatten()
                        .map(|input| input.identifier.clone())
                        .collect::<Vec<_>>()
                );
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
        log::debug!(
            "[craft] rejected: {} has {} outputs, need exactly 1",
            recipe.identifier,
            outputs.len()
        );
        return false;
    }
    let output = &outputs[0];
    let Some(id) = items
        .runtime_id_by_name(&output.identifier)
        .or_else(|| {
            sc_recipe::rename_alias(&tags, &output.identifier)
                .and_then(|alias| items.runtime_id_by_name(&alias))
        })
    else {
        log::debug!(
            "[craft] rejected: {} output '{}' not in item registry",
            recipe.identifier,
            output.identifier
        );
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
        log::debug!(
            "[craft] rejected: {} output slot blocked (old={:?}, produced={:?})",
            recipe.identifier,
            old,
            produced
        );
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
        log::debug!(
            "[craft] rejected: {} consumption plan failed (requirements={:?}, live={:?})",
            recipe.identifier,
            requirements
                .iter()
                .map(|(spec, count)| (spec.clone(), count))
                .collect::<Vec<_>>(),
            live.iter()
                .map(|(i, input)| (*i, input.identifier.clone(), input.count))
                .collect::<Vec<_>>()
        );
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

/// Creative-mode materialization for one `Grant` action: resolves the table
/// template for `creative_id` and fills the scratch Output slot with exactly
/// what this request's Moves consume from it (capped at the item stack cap).
/// Fail-closed: non-creative players, unknown ids, an occupied Output, a
/// missing table, or no consuming Move all reject. The caller additionally
/// rejects any leftover Output at end of request, so granted items cannot
/// leak across requests into later free takes.
#[allow(clippy::too_many_arguments)]
fn grant_creative(
    world: &World,
    entity: EntityId,
    items: &ItemRegistry,
    stacks: &mut [ItemStack],
    main: usize,
    request: &CraftRequestEvent,
    creative_id: u32,
    op_id: u64,
) -> bool {
    let creative = world
        .get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        .is_some_and(|client| client.data.read().gamemode.is_creative());
    if !creative {
        log::debug!("[craft] op={op_id} Grant rejected: non-creative player");
        return false;
    }
    let output = main + 10;
    if !stacks[output].is_empty() {
        log::debug!("[craft] op={op_id} Grant rejected: output occupied");
        return false;
    }
    let Some(table) = world.get_resource::<sc_item::CreativeGrantTable>() else {
        log::debug!("[craft] op={op_id} Grant rejected: no grant table");
        return false;
    };
    let Some(template) = table.get(creative_id) else {
        log::debug!("[craft] op={op_id} Grant rejected: unknown creative id {creative_id}");
        return false;
    };
    let wanted: u16 = request
        .actions
        .iter()
        .filter_map(|action| match action {
            InventoryAction::Move { amount, source, .. }
                if source.container == CraftContainer::Output =>
            {
                Some(*amount as u16)
            }
            _ => None,
        })
        .sum();
    let granted = wanted.min(items.max_stack_size(template.runtime_id));
    if granted == 0 {
        log::debug!("[craft] op={op_id} Grant rejected: no consuming move");
        return false;
    }
    stacks[output] = ItemStack {
        count: granted,
        ..template
    };
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
    // Furnace window binding (validated like `station()`; stale windows
    // reject furnace moves). Working slots load only when touched.
    let furnace_binding = match station(world, entity) {
        Some((bound, _)) if crate::furnace::is_furnace_station(bound) => world
            .get_component::<ContainerOpen>(&entity)
            .and_then(|open| {
                open.workstation().map(|(station, world_id, pos)| {
                    (world_id, pos, station, open.window_id)
                })
            }),
        _ => None,
    };
    let touches_furnace = request.actions.iter().any(|action| match action {
        InventoryAction::Move {
            source,
            destination,
            ..
        } => is_furnace_slot(source) || is_furnace_slot(destination),
        InventoryAction::Swap {
            source,
            destination,
        } => is_furnace_slot(source) || is_furnace_slot(destination),
        _ => false,
    });
    let mut furnace_slots: Option<[ItemStack; 3]> = None;
    if touches_furnace {
        if let Some((world_id, pos, _, _)) = furnace_binding.as_ref() {
            furnace_slots = crate::block_entity::process_state_at(world, world_id, *pos)
                .map(|process| process.furnace_slots());
        }
    }
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
        // Set by a Grant action: the scratch Output must be empty again by
        // end of request (grants cannot leak across requests).
        let mut grant_output = false;
        for action in &request.actions {
            match action {
                InventoryAction::Move {
                    amount,
                    source,
                    destination,
                } => {
                    if is_furnace_slot(source) || is_furnace_slot(destination) {
                        let Some((_, _, station, window_id)) = &furnace_binding else {
                            log::debug!(
                                "[craft] op={op_id} Move rejected: furnace slot without a bound furnace window"
                            );
                            return false;
                        };
                        let Some(slots) = furnace_slots.as_mut() else {
                            log::debug!(
                                "[craft] op={op_id} Move rejected: furnace state missing"
                            );
                            return false;
                        };
                        let fuel_table =
                            world.get_resource::<sc_packloader::item::ItemComponentTable>();
                        if !furnace_move(
                            &valid_ref,
                            &items,
                            fuel_table.as_deref(),
                            main,
                            table,
                            *station,
                            *window_id,
                            &mut stacks,
                            slots,
                            *amount,
                            source,
                            destination,
                            op_id,
                        ) {
                            return false;
                        }
                        continue;
                    }
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
                    if is_furnace_slot(source) || is_furnace_slot(destination) {
                        let Some((_, _, station, window_id)) = &furnace_binding else {
                            log::debug!(
                                "[craft] op={op_id} Swap rejected: furnace slot without a bound furnace window"
                            );
                            return false;
                        };
                        let Some(slots) = furnace_slots.as_mut() else {
                            log::debug!(
                                "[craft] op={op_id} Swap rejected: furnace state missing"
                            );
                            return false;
                        };
                        let fuel_table =
                            world.get_resource::<sc_packloader::item::ItemComponentTable>();
                        if !furnace_swap(
                            &valid_ref,
                            &items,
                            fuel_table.as_deref(),
                            main,
                            table,
                            *station,
                            *window_id,
                            &mut stacks,
                            slots,
                            source,
                            destination,
                            op_id,
                        ) {
                            return false;
                        }
                        continue;
                    }
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
                InventoryAction::Grant { creative_id } => {
                    if crafted
                        || !grant_creative(
                            world,
                            entity,
                            &items,
                            &mut stacks,
                            main,
                            request,
                            *creative_id,
                            op_id,
                        )
                    {
                        return false;
                    }
                    crafted = true;
                    grant_output = true;
                }
                InventoryAction::Unsupported => {
                    log::debug!("[craft] op={op_id} rejected: contains Unsupported actions");
                    return false;
                }
            }
        }
        if grant_output && !stacks[main + 10].is_empty() {
            log::debug!(
                "[craft] op={op_id} rejected: Grant output left over (count={})",
                stacks[main + 10].count,
            );
            return false;
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
        // Furnace working slots write back to the bound process state
        // (viewer sync goes out with the response, see `publish_response`).
        // Missing entities are ensured first (move path creates on touch,
        // same as the old store path); a refused ensure drops the write
        // (fail-closed, the response already resyncs the viewer).
        if let (Some(slots), Some((world_id, pos, station, _))) =
            (furnace_slots, furnace_binding.as_ref())
        {
            use sc_block::workstation::SlotRole;
            crate::block_entity::ensure_block_entity(
                world,
                world_id.clone(),
                *pos,
                crate::block_entity::BlockEntityKind::Furnace(*station),
            );
            if let Some(process) = crate::block_entity::process_state_at(world, world_id, *pos) {
                process.set_slot(SlotRole::Input, slots[0].clone());
                process.set_slot(SlotRole::Fuel, slots[1].clone());
                process.set_slot(SlotRole::Result, slots[2].clone());
            }
        }
        crate::inventory::crafting::bump_inventory_revision(world, &entity);
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
    crate::inventory::crafting::bump_inventory_revision(world, &entity);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn player_inventory_window_counts_as_stationless_2x2() {
        use crate::inventory::ContainerOpen;
        let world = World::new();
        let entity = world.spawn(sc_utils::components::DisplayName("craft-test".to_string()));
        // No window open: 2x2 with no station.
        assert!(matches!(
            station(&world, entity),
            Some((StationKind::InventoryCrafting, false))
        ));
        // Player's own inventory open (the 2x2 crafting case): same context,
        // not a rejection. This is the live log->planks regression.
        world.add_component(&entity, ContainerOpen::player_inventory());
        assert!(matches!(
            station(&world, entity),
            Some((StationKind::InventoryCrafting, false))
        ));
    }
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

    fn creative_player(
        world: &World,
        gamemode: sc_utils::game::gamemode::Gamemode,
    ) -> sc_ecs::entity::EntityId {
        use sc_utils::game::client::MinecraftClient;
        use sc_utils::world::client_data::MinecraftClientData;
        let entity = world.spawn(PlayerInventory::new(36));
        let data = MinecraftClientData {
            gamemode,
            ..MinecraftClientData::default()
        };
        world.add_component(&entity, MinecraftClient::new(entity, world.clone(), data));
        entity
    }

    fn creative_table(world: &World, runtime_id: u16) {
        let mut table = sc_item::CreativeGrantTable::default();
        let id = table.push(ItemStack::new(runtime_id, 1));
        assert_eq!(id, 1);
        world.insert_resource(table);
    }

    fn grant_take(
        grant_id: u32,
        amount: u8,
        op: u64,
        entity: sc_ecs::entity::EntityId,
    ) -> CraftRequestEvent {
        use CraftContainer as C;
        let reference = |container, slot, net_id| CraftSlotRef {
            container,
            slot,
            net_id,
            dynamic: None,
        };
        CraftRequestEvent {
            entity,
            recipe_network_id: 0,
            requested_count: 0,
            operation_id: op,
            actions: vec![
                InventoryAction::Grant {
                    creative_id: grant_id,
                },
                InventoryAction::Results,
                InventoryAction::Move {
                    amount,
                    source: reference(C::Output, 50, -3),
                    destination: reference(C::Hotbar, 0, 0),
                },
            ],
        }
    }

    /// Live creative-take shape (action 12 + Results + Output->hotbar Move):
    /// the grant materializes exactly the moved amount and the hotbar fills.
    #[test]
    fn creative_grant_materializes_into_hotbar() {
        use sc_utils::game::gamemode::Gamemode;
        let world = World::new();
        let items = ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(2, "minecraft:oak_planks"));
        world.insert_resource(items);
        world.insert_resource(SharedRecipeRegistry::new(
            sc_recipe::RecipeRegistrySnapshot::compile(
                &[],
                &sc_recipe::CompileBudgets::default(),
                false,
            )
            .unwrap()
            .0,
        ));
        world.insert_resource(SharedItemTags::default());
        creative_table(&world, 2);
        let entity = creative_player(&world, Gamemode::Creative);
        assert!(execute(&world, &grant_take(1, 64, 1, entity)));
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        let stack = inventory.get(0).unwrap();
        assert_eq!(stack.runtime_id, 2);
        assert_eq!(stack.count, 64);
    }

    /// Survival players never materialize: the whole request rejects and the
    /// hotbar stays empty (no free items outside creative).
    #[test]
    fn creative_grant_rejects_non_creative_player() {
        use sc_utils::game::gamemode::Gamemode;
        let world = World::new();
        let items = ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(2, "minecraft:oak_planks"));
        world.insert_resource(items);
        world.insert_resource(SharedRecipeRegistry::new(
            sc_recipe::RecipeRegistrySnapshot::compile(
                &[],
                &sc_recipe::CompileBudgets::default(),
                false,
            )
            .unwrap()
            .0,
        ));
        world.insert_resource(SharedItemTags::default());
        creative_table(&world, 2);
        let entity = creative_player(&world, Gamemode::Survival);
        assert!(!execute(&world, &grant_take(1, 64, 1, entity)));
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert!(inventory
            .get(0)
            .map(|stack| stack.is_empty())
            .unwrap_or(true));
    }

    /// Unknown creative ids reject, and granted output cannot leak across
    /// requests: a follow-up drain without a fresh Grant finds Output empty.
    #[test]
    fn creative_grant_rejects_unknown_id_and_leftovers() {
        use sc_utils::game::gamemode::Gamemode;
        use CraftContainer as C;
        let world = World::new();
        let items = ItemRegistry::new();
        items.upsert(sc_item::ItemDefinition::new(2, "minecraft:oak_planks"));
        world.insert_resource(items);
        world.insert_resource(SharedRecipeRegistry::new(
            sc_recipe::RecipeRegistrySnapshot::compile(
                &[],
                &sc_recipe::CompileBudgets::default(),
                false,
            )
            .unwrap()
            .0,
        ));
        world.insert_resource(SharedItemTags::default());
        creative_table(&world, 2);
        let entity = creative_player(&world, Gamemode::Creative);
        // Unknown id.
        assert!(!execute(&world, &grant_take(9, 64, 1, entity)));
        // Legit take fills hotbar 0...
        assert!(execute(&world, &grant_take(1, 64, 2, entity)));
        // ...but a later drain without a fresh Grant rejects (Output empty).
        let reference = |container, slot, net_id| CraftSlotRef {
            container,
            slot,
            net_id,
            dynamic: None,
        };
        assert!(!execute(
            &world,
            &CraftRequestEvent {
                entity,
                recipe_network_id: 0,
                requested_count: 0,
                operation_id: 3,
                actions: vec![InventoryAction::Move {
                    amount: 1,
                    source: reference(C::Output, 50, -3),
                    destination: reference(C::Hotbar, 1, 0),
                }],
            }
        ));
        let inventory = world.get_component::<PlayerInventory>(&entity).unwrap();
        assert!(inventory
            .get(1)
            .map(|stack| stack.is_empty())
            .unwrap_or(true));
    }
}
