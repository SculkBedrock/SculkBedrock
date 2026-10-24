//! Workstation state: block processors, window allocation and station lookup.
//!
//! Moved verbatim from `crafting.rs` (P4 split); re-exported from [`super`]
//! so existing paths keep compiling.

use std::sync::Arc;
use parking_lot::RwLock;
use sc_block::workstation::SlotRole;
use sc_ecs::component::Component;
use sc_item::ItemStack;
use sc_recipe::StationKind;
use crate::inventory::ContainerOpen;
use crate::net::{NetworkOutbox, OpenCraftingStation};

// ===================== block-entity processors =====================

/// 区域本地维护的时长型加工状态（熔炉、高炉、烟熏炉、营火、酿造台与锻造台）。
///
/// 按槽位数量泛型化，槽位与角色和方块的 sc:workstation 声明一一对应。
///
/// Region-local duration-based processing state for furnaces, blast furnaces, smokers, campfires, brewing stands, and smithing tables.
///
/// Generic over slot count; slots and roles correspond one-to-one with the block's sc:workstation declaration.
#[derive(Component, Clone, Debug)]
pub struct BlockProcessState {
    inner: Arc<RwLock<ProcessData>>,
}

#[derive(Clone, Debug)]
struct ProcessData {
    kind: StationKind,
    slots: Vec<ItemStack>,
    roles: Vec<SlotRole>,
    burn_remaining: u32,
    burn_max: u32,
    cook_progress: u16,
    cook_total: u16,
    dirty: bool,
    saved_generation: u64,
    generation: u64,
}

/// 用于持久化的全槽位快照。
///
/// Full-slot snapshot used for persistence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub generation: u64,
    pub dirty: bool,
    pub kind: StationKind,
    pub slots: Vec<(SlotRole, ItemStack)>,
    pub burn_remaining: u32,
    pub burn_max: u32,
    pub cook_progress: u16,
    pub cook_total: u16,
}

impl BlockProcessState {
    pub fn new(kind: StationKind, roles: Vec<SlotRole>, cook_total: u16) -> Self {
        let slots = vec![ItemStack::empty(); roles.len()];
        Self {
            inner: Arc::new(RwLock::new(ProcessData {
                kind,
                slots,
                roles,
                burn_remaining: 0,
                burn_max: 0,
                cook_progress: 0,
                cook_total: cook_total.max(1),
                dirty: false,
                saved_generation: 0,
                generation: 0,
            })),
        }
    }
    pub fn kind(&self) -> StationKind {
        self.inner.read().kind
    }
    pub fn is_burning(&self) -> bool {
        self.inner.read().burn_remaining > 0
    }
    pub fn burn_remaining(&self) -> u32 {
        self.inner.read().burn_remaining
    }
    pub fn burn_max(&self) -> u32 {
        self.inner.read().burn_max
    }
    pub fn cook_progress(&self) -> u16 {
        self.inner.read().cook_progress
    }
    pub fn cook_total(&self) -> u16 {
        self.inner.read().cook_total
    }
    /// Lit-time progress for `ContainerSetData` property 1 (`FURNACE_LIT_TIME`).
    /// In Bedrock protocol, property 1 is the remaining fuel burn ticks, and
    /// property 2 is the total fuel burn duration (`FURNACE_LIT_DURATION`).
    /// The client renders flame height from the ratio (lit_ticks / lit_duration).
    pub fn lit_progress(&self) -> u16 {
        self.inner.read().burn_remaining.min(u16::MAX as u32) as u16
    }
    /// Slots in furnace window order `[input, fuel, result]` (missing roles
    /// read as empty; decls always carry all three for fuel processes, so a
    /// missing role here only fires on invalid packs or role-less kinds).
    pub fn furnace_slots(&self) -> [ItemStack; 3] {
        let data = self.inner.read();
        let at = |role: SlotRole| {
            Self::role_index(&data, role)
                .map(|index| data.slots[index].clone())
                .unwrap_or_else(ItemStack::empty)
        };
        [
            at(SlotRole::Input),
            at(SlotRole::Fuel),
            at(SlotRole::Result),
        ]
    }
    fn role_index(data: &ProcessData, role: SlotRole) -> Option<usize> {
        data.roles.iter().position(|candidate| *candidate == role)
    }
    /// Cloned slot by role (`None` when the decl carries no such role;
    /// never panics on role absence).
    pub fn slot(&self, role: SlotRole) -> Option<ItemStack> {
        let data = self.inner.read();
        Self::role_index(&data, role).map(|index| data.slots[index].clone())
    }
    /// Overwrite a slot by role (false when the decl carries no such role).
    pub fn set_slot(&self, role: SlotRole, stack: ItemStack) -> bool {
        let mut data = self.inner.write();
        let Some(index) = Self::role_index(&data, role) else {
            return false;
        };
        if data.slots[index] != stack {
            data.slots[index] = stack;
            data.generation = data.generation.wrapping_add(1);
            data.dirty = true;
        }
        true
    }
    /// Take a slot by role, leaving it empty (`None` when the decl carries
    /// no such role; `Some` possibly-empty stack otherwise).
    pub fn take_slot(&self, role: SlotRole) -> Option<ItemStack> {
        let mut data = self.inner.write();
        let index = Self::role_index(&data, role)?;
        let taken = std::mem::replace(&mut data.slots[index], ItemStack::empty());
        if !taken.is_empty() {
            data.generation = data.generation.wrapping_add(1);
            data.dirty = true;
        }
        Some(taken)
    }
    /// Insert input if the input slot is free and bounded (one stack max).
    pub fn insert_input(&self, stack: ItemStack) -> bool {
        let mut data = self.inner.write();
        let Some(index) = Self::role_index(&data, SlotRole::Input) else {
            return false;
        };
        if !data.slots[index].is_empty() {
            return false;
        }
        data.slots[index] = stack;
        data.generation = data.generation.wrapping_add(1);
        data.dirty = true;
        true
    }
    /// One owner tick of burn/cook (vanilla furnace semantics: burn decrements
    /// per tick, cook advances while smeltable, completion moves one output
    /// into the result slot, fuel exhaustion resets progress). `output` is the current smelt
    /// result for the input (`None` = nothing smeltable); `max_stack` caps
    /// the result slot. Duration comes from the decl-fed `cook_total`, never
    /// from the station here.
    pub fn advance(
        &self,
        output: Option<&ItemStack>,
        fuel_ticks: u32,
        max_stack: u16,
    ) -> crate::furnace::TickOutcome {
        use crate::furnace::TickOutcome;
        let mut data = self.inner.write();
        let was_burning = data.burn_remaining > 0;
        let mut outcome = TickOutcome::default();
        let input = Self::role_index(&data, SlotRole::Input);
        let fuel = Self::role_index(&data, SlotRole::Fuel);
        let result = Self::role_index(&data, SlotRole::Result);
        let can_smelt = match (input, result) {
            (Some(input), Some(result)) => output.is_some_and(|out| {
                !data.slots[input].is_empty()
                    && (data.slots[result].is_empty()
                        || (data.slots[result].runtime_id == out.runtime_id
                            && data.slots[result].count.saturating_add(out.count)
                                <= max_stack.max(1)))
            }),
            _ => false,
        };
        if data.burn_remaining == 0 && can_smelt && fuel_ticks > 0 {
            if let Some(fuel) = fuel {
                if !data.slots[fuel].is_empty() {
                    data.slots[fuel].count = data.slots[fuel].count.saturating_sub(1);
                    data.slots[fuel].clear_if_empty();
                    data.burn_remaining = fuel_ticks;
                    data.burn_max = fuel_ticks;
                    outcome.consumed_fuel = true;
                }
            }
        }
        if data.burn_remaining > 0 {
            data.burn_remaining = data.burn_remaining.saturating_sub(1);
            if data.burn_remaining == 0 {
                data.burn_max = 0;
                data.cook_progress = 0;
            }
            if can_smelt {
                if let Some(out) = output {
                    data.cook_progress = data.cook_progress.saturating_add(1);
                    if data.cook_progress >= data.cook_total {
                        if let (Some(input), Some(result)) = (input, result) {
                            if data.slots[result].is_empty() {
                                data.slots[result] = out.clone();
                            } else {
                                data.slots[result].count =
                                    data.slots[result].count.saturating_add(out.count);
                            }
                            data.slots[input].count = data.slots[input].count.saturating_sub(1);
                            data.slots[input].clear_if_empty();
                            data.cook_progress = data.cook_progress.saturating_sub(data.cook_total);
                            outcome.completed = true;
                        }
                    }
                }
            }
        } else {
            // Out of fuel with nothing burning: progress resets (furnace turn-off).
            data.cook_progress = 0;
            data.burn_max = 0;
        }
        if outcome.consumed_fuel || outcome.completed {
            data.generation = data.generation.wrapping_add(1);
            data.dirty = true;
        }
        outcome.lit_changed = was_burning != (data.burn_remaining > 0);
        outcome
    }
    pub fn take_output(&self) -> Option<ItemStack> {
        self.take_slot(SlotRole::Result)
    }
    pub fn set_output(&self, stack: ItemStack) {
        self.set_slot(SlotRole::Result, stack);
    }
    /// Full-slot snapshot for persistence without holding the guard across IO.
    pub fn snapshot(&self) -> ProcessSnapshot {
        let data = self.inner.read();
        ProcessSnapshot {
            generation: data.generation,
            dirty: data.dirty,
            kind: data.kind,
            slots: data
                .roles
                .iter()
                .zip(data.slots.iter())
                .map(|(role, slot)| (*role, slot.clone()))
                .collect(),
            burn_remaining: data.burn_remaining,
            burn_max: data.burn_max,
            cook_progress: data.cook_progress,
            cook_total: data.cook_total,
        }
    }
    /// Save acknowledgement: only clears dirty versions `<= generation` and
    /// only when nothing newer arrived (same contract as chunk writeback).
    pub fn acknowledge_save(&self, generation: u64) {
        let mut data = self.inner.write();
        if data.generation == generation {
            data.dirty = false;
            data.saved_generation = generation;
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.inner.read().dirty
    }
}


/// 玩家容器窗口 ID 分配器：从 1 开始递增，按 100 取模并跳过 0。
///
/// Player container window ID allocator: starts at 1 and increments modulo 100 while skipping 0.
#[derive(Component, Debug, Default)]
pub struct ContainerWindowAllocator {
    pub(super) next: parking_lot::Mutex<u8>,
}

impl ContainerWindowAllocator {
    pub fn allocate(&self) -> u8 {
        let mut next = self.next.lock();
        // Allocate max(1, ++cnt % 100).
        let id = (*next).wrapping_add(1) % 100;
        let id = id.max(1);
        *next = id;
        id
    }
}

pub fn open_crafting_station(
    world: &sc_ecs::world::World,
    entity: sc_ecs::entity::EntityId,
    position: sc_block::position::BlockPosition,
    outbox: &mut NetworkOutbox,
) -> bool {
    // One client holds at most one container window: return early when a
    // window is already open, before touching world data.
    if let Some(open) = world.get_component::<ContainerOpen>(&entity) {
        log::debug!(
            "[craft] workstation kept closed: window already open (window={} type={})",
            open.window_id,
            open.kind_name()
        );
        return true;
    }
    let Some(world_id) = world.get_component::<sc_world::manager::MinecraftWorldId>(&entity) else {
        return false;
    };
    let pos = (position.x, position.y, position.z);
    let (station, container_type) = if let Some(table) = crafting_table_decl_at(world, &world_id, pos) {
        let station = StationKind::from_tag(&table.station);
        if station == StationKind::Unknown {
            return false;
        }
        (station, table.container_type)
    } else if let Some(furnace) = furnace_decl_at(world, &world_id, pos) {
        let station = StationKind::from_tag(&furnace.kind);
        if station == StationKind::Unknown {
            return false;
        }
        (station, furnace.container_type)
    } else {
        return false;
    };
    // Requires the OPEN_CONTAINERS ability.
    if !world
        .get_component::<sc_entity::player::adventure_settings::AdventureSettings>(&entity)
        .is_some_and(|settings| {
            settings.get(
                &sc_entity::player::adventure_settings::AdventureSettingsType::OpenContainers,
            )
        })
    {
        return false;
    }
    let Some(transform) = world.get_component::<sc_entity::motion::Transform>(&entity) else {
        return true;
    };
    // Player feet position: use the always-current client position, since
    // Transform stays stale until the first movement packet.
    let feet = world
        .get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        .map(|client| {
            let data = client.data.read();
            (data.position.x, data.position.y, data.position.z)
        })
        .unwrap_or_else(|| {
            let transform = transform.read();
            (
                transform.position.x,
                transform.position.y,
                transform.position.z,
            )
        });
    let distance = (feet.0 - position.x as f32 - 0.5).powi(2)
        + (feet.1 - position.y as f32 - 0.5).powi(2)
        + (feet.2 - position.z as f32 - 0.5).powi(2);
    if distance > 36.0 {
        log::debug!(
            "[craft] workstation kept closed: out of reach {distance:.1} (feet {feet:?}, block {pos:?})"
        );
        return true;
    }
    let Some(actor) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
        return true;
    };
    // Window ids allocate incrementally per player.
    if world
        .get_component::<ContainerWindowAllocator>(&entity)
        .is_none()
    {
        world.add_component(&entity, ContainerWindowAllocator::default());
    }
    let window_id = world
        .get_component::<ContainerWindowAllocator>(&entity)
        .map(|allocator| allocator.allocate())
        .unwrap_or(1);
    world.add_component(
        &entity,
        ContainerOpen::at_workstation(window_id, station, world_id.as_ref().clone(), pos),
    );
    log::debug!(
        "[craft] workstation opened {station:?} (block {pos:?}, window {window_id}, feet {feet:?}, dist2={distance:.1})"
    );
    let mut faults = world.get_resource_mut::<crate::net::PendingConnectionFaults>();
    let mut publisher = crate::net::IntentPublisher::new(outbox)
        .with_world(world)
        .maybe_with_faults(faults.as_deref_mut());
    publisher.publish(OpenCraftingStation {
        entity_id: actor.0,
        station,
        window_id,
        container_type,
        x: position.x,
        y: position.y,
        z: position.z,
    });
    if crate::furnace::is_furnace_station(station) {
        // Identity ensure (creation-only here; reads below go through the
        // component). A refused ensure opens with empty slots, exactly like
        // the old store-full path.
        crate::block_entity::ensure_block_entity(
            world,
            world_id.as_ref().clone(),
            pos,
            crate::block_entity::BlockEntityKind::Furnace(station),
        );
        let (slots, cook, lit, lit_max) =
            if let Some(process) = crate::block_entity::process_state_at(world, world_id.as_ref(), pos) {
                (
                    process.furnace_slots(),
                    process.cook_progress(),
                    process.lit_progress(),
                    process.burn_max().min(u16::MAX as u32) as u16,
                )
            } else {
                (Default::default(), 0, 0, 0)
            };
        publisher.publish(crate::net::SyncFurnaceWindow {
            entity_id: actor.0,
            window_id,
            station,
            input: slots[0].clone(),
            fuel: slots[1].clone(),
            result: slots[2].clone(),
        });
        publisher.publish(crate::net::SyncFurnaceProgress {
            entity_id: actor.0,
            window_id,
            cook_ticks: cook,
            lit_ticks: lit,
            lit_duration: lit_max,
        });
    }
    true
}

pub(crate) fn station_block_kind(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<StationKind> {
    if let Some(table) = crafting_table_decl_at(world, world_id, pos) {
        let station = StationKind::from_tag(&table.station);
        if station != StationKind::Unknown {
            return Some(station);
        }
    }
    if let Some(furnace) = furnace_decl_at(world, world_id, pos) {
        let station = StationKind::from_tag(&furnace.kind);
        if station != StationKind::Unknown {
            return Some(station);
        }
    }
    None
}

/// Crafting table declaration for the block at a position (`None` when the
/// chunk is unloaded, the cell is empty, or the block is not a crafting table).
pub(crate) fn crafting_table_decl_at(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<sc_block::workstation::CraftingTableDecl> {
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
    let state = chunk.block_at(target.local_x(), target.y, target.local_z())?;
    let registry = world.get_resource::<sc_block::registry::BlockStateRegistry>()?;
    let snapshot = world
        .get_resource::<sc_block::block_json::BlockJsonRegistry>()
        .and_then(|registry| registry.get())?;
    let dense = registry.by_runtime_id(state)?;
    snapshot.crafting_table_of(dense.0).cloned()
}

/// Furnace declaration for the block at a position (`None` when the
/// chunk is unloaded, the cell is empty, or the block is not a furnace).
pub(crate) fn furnace_decl_at(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<sc_block::workstation::FurnaceDecl> {
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
    let state = chunk.block_at(target.local_x(), target.y, target.local_z())?;
    let registry = world.get_resource::<sc_block::registry::BlockStateRegistry>()?;
    let snapshot = world
        .get_resource::<sc_block::block_json::BlockJsonRegistry>()
        .and_then(|registry| registry.get())?;
    let dense = registry.by_runtime_id(state)?;
    snapshot.furnace_of(dense.0).cloned()
}

/// Chest declaration for the block at a position (`None` when the
/// chunk is unloaded, the cell is empty, or the block is not a chest).
pub(crate) fn chest_decl_at(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<sc_block::workstation::ChestDecl> {
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
    let state = chunk.block_at(target.local_x(), target.y, target.local_z())?;
    let registry = world.get_resource::<sc_block::registry::BlockStateRegistry>()?;
    let snapshot = world
        .get_resource::<sc_block::block_json::BlockJsonRegistry>()
        .and_then(|registry| registry.get())?;
    let dense = registry.by_runtime_id(state)?;
    snapshot.chest_of(dense.0).cloned()
}

/// Bedrock container type for the block at a position (`None` when unresolvable).
pub(crate) fn container_type_at(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<i8> {
    if let Some(table) = crafting_table_decl_at(world, world_id, pos) {
        return Some(table.container_type);
    }
    if let Some(furnace) = furnace_decl_at(world, world_id, pos) {
        return Some(furnace.container_type);
    }
    if let Some(chest) = chest_decl_at(world, world_id, pos) {
        return Some(chest.container_type);
    }
    None
}

/// Find a usable recipe workstation near the player.
///
/// Scan nearby blocks for each declared station, returning the first hit.
/// Unloaded chunks count as having no workstation (fail-closed).
///
/// Reserved for workstation-proximity validation once callers migrate off the
/// open-window check (see `open_crafting_station`).
#[allow(dead_code)]
fn find_nearby_station(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    feet: (f32, f32, f32),
    stations: &[StationKind],
) -> Option<(StationKind, (i32, i32, i32))> {
    if stations.is_empty() {
        return None;
    }
    let (fx, fy, fz) = feet;
    let min_x = (fx - 6.0).floor() as i32;
    let max_x = (fx + 6.0).floor() as i32;
    let min_y = (fy - 6.0).floor() as i32;
    let max_y = (fy + 6.0).floor() as i32;
    let min_z = (fz - 6.0).floor() as i32;
    let max_z = (fz + 6.0).floor() as i32;
    // Deterministic order: scan by ascending squared distance.
    for wanted in stations {
        let mut best: Option<((i32, i32, i32), f32)> = None;
        let mut x = min_x;
        while x <= max_x {
            let mut y = min_y;
            while y <= max_y {
                let mut z = min_z;
                while z <= max_z {
                    let dx = (x as f32 + 0.5) - fx;
                    let dy = (y as f32 + 0.5) - fy;
                    let dz = (z as f32 + 0.5) - fz;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq <= 36.0 {
                        let dominated = best.map(|(_, d)| dist_sq >= d).unwrap_or(false);
                        if !dominated
                            && station_block_kind(world, world_id, (x, y, z)).as_ref()
                                == Some(wanted)
                        {
                            best = Some(((x, y, z), dist_sq));
                        }
                    }
                    z += 1;
                }
                y += 1;
            }
            x += 1;
        }
        if let Some((pos, _)) = best {
            return Some((*wanted, pos));
        }
    }
    None
}