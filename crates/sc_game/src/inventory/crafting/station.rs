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

/// Owner-local duration-process state (furnace / blast / smoker / campfire /
/// brewing / smithing). Bounded, ticked by the owning region; async workers
/// never write this directly.
///
/// Generic over slot count: `slots`/`roles` mirror the block's
/// `sc:workstation` decl slots one-to-one (furnace 3, campfire 4+ with
/// `Extra`). Dual clocks are mandatory (burn runs on its own, cook only
/// advances while smeltable; one `progress_ticks` cannot express a furnace).
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

/// Full-slot snapshot for persistence without holding the guard across IO
/// (storage wiring is a later slice; this only defines the interface).
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
    /// Lit-time progress for `ContainerSetData` property 1
    /// (`ceil(burn/max * cook_total)`, burn-duration scaling). Duration is
    /// owned (decl-fed), so no parameter unlike the store-side version.
    pub fn lit_progress(&self) -> u16 {
        let data = self.inner.read();
        if data.burn_max == 0 || data.burn_remaining == 0 {
            return 0;
        }
        let ready = data.cook_total as u32;
        ((data.burn_remaining as u64 * ready as u64 + data.burn_max as u64 - 1)
            / data.burn_max as u64) as u16
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


/// Player container window id allocator.
///
/// First window is 1, then increments with `% 100` wraparound skipping 0.
/// The inventory window (0) never allocates here.
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
    let Some(decl) = workstation_decl_at(world, &world_id, pos) else {
        return false;
    };
    let station = StationKind::from_tag(&decl.station);
    let container_type = decl.container;
    // Only windowed workstations open (brewing/smithing/campfire decls
    // address but never open); unknown tags fail closed (load validation
    // rejects such packs, this is the defense in depth).
    if station == StationKind::Unknown || !decl.window {
        return false;
    }
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
    crate::net::IntentPublisher::new(outbox)
        .with_world(world)
        .maybe_with_faults(faults.as_deref_mut())
        .publish(OpenCraftingStation {
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
        let slots = crate::block_entity::process_state_at(world, world_id.as_ref(), pos)
            .map(|process| process.furnace_slots())
            .unwrap_or_default();
        crate::net::IntentPublisher::new(outbox)
            .with_world(world)
            .publish(crate::net::SyncFurnaceWindow {
                entity_id: actor.0,
                window_id,
                station,
                input: slots[0].clone(),
                fuel: slots[1].clone(),
                result: slots[2].clone(),
            });
    }
    true
}

pub(crate) fn station_block_kind(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<StationKind> {
    let decl = workstation_decl_at(world, world_id, pos)?;
    let station = StationKind::from_tag(&decl.station);
    (station != StationKind::Unknown).then_some(station)
}

/// Workstation declaration for the block at a position (`None` when the
/// chunk is unloaded, the cell is empty, or the block serves no station).
/// Single chunk-read behind `station_block_kind` and container resolution.
pub(crate) fn workstation_decl_at(
    world: &sc_ecs::world::World,
    world_id: &sc_world::manager::MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<sc_block::workstation::WorkstationDecl> {
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
    snapshot.workstation_of(dense.0).cloned()
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