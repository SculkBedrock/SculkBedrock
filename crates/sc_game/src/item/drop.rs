//! Lightweight item-drop storage.
//!
//! Ordinary drops are high-cardinality, short-lived objects. They therefore
//! live in a generation-checked slot store with a cell index instead of
//! becoming full ECS entities with Transform/Physics components.
//!
//! Item merge, pickup, and drop-delay behavior:
//!
//! - merge: every 60 ticks, two on-ground survivors with room in the
//!   stack merge wholesale (no splits) when they hold the same item;
//!   the absorbed side closes, the survivor keeps runtime id, position,
//!   and age, and an event syncs the new count.
//! - pickup: every tick, drops inside the grown player box with expired
//!   pickup delay move into the bag (creative requires full fit); the
//!   take packet broadcasts first, then bag insert, then close.
//! - drop delay: 10 ticks (0.5s) before pickup is allowed.

use std::collections::HashMap;

use sc_ecs::resource::Resource;
use sc_entity::motion::{Aabb, Position, Velocity};
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};

use crate::net::{
    DespawnItemEntity, IntentPublisher, MoveItemEntity, NetworkOutbox,
    PendingConnectionFaults, PendingInventoryResync, SpawnItemEntity, TakeItemEntity,
    UpdateInventorySlot, UpdateItemStackSize,
};

const CELL_SIZE: f32 = 2.0;
const GRAVITY: f32 = 0.04;
const DRAG: f32 = 0.98;
const TERMINAL_VELOCITY: f32 = 0.4;
const LIFETIME_TICKS: u32 = 6_000;
/// Default pickup delay (10 ticks = 0.5s).
const INITIAL_PICKUP_DELAY: u16 = 10;
/// Merge attempt every 60 ticks (3s).
const MERGE_INTERVAL_TICKS: u32 = 60;
/// Merge search range: item box grown to |d| <= 1.25 per axis.
const MERGE_AXIS_RANGE: f32 = 1.25;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DropHandle {
    pub slot: u32,
    pub generation: u32,
}

#[derive(Clone, Debug)]
pub struct ItemDrop {
    pub handle: DropHandle,
    pub runtime_id: u64,
    pub stack: ItemStack,
    pub position: Position,
    pub velocity: Velocity,
    pub age_ticks: u32,
    pub pickup_delay: u16,
    pub sleeping: bool,
    pub on_ground: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CellKey {
    x: i32,
    y: i32,
    z: i32,
}

impl CellKey {
    fn from_position(position: Position) -> Self {
        Self {
            x: (position.x / CELL_SIZE).floor() as i32,
            y: (position.y / CELL_SIZE).floor() as i32,
            z: (position.z / CELL_SIZE).floor() as i32,
        }
    }
}

struct DropSlot {
    generation: u32,
    value: Option<ItemDrop>,
}

/// Per-world authoritative storage for ordinary item drops.
#[derive(Default)]
pub struct RegionItemDropStore {
    slots: Vec<DropSlot>,
    free_slots: Vec<u32>,
    by_cell: HashMap<CellKey, Vec<DropHandle>>,
    /// runtime_id to handle (existence check against game authority).
    by_runtime_id: HashMap<u64, DropHandle>,
}

impl RegionItemDropStore {
    fn is_empty(&self) -> bool {
        self.by_runtime_id.is_empty()
    }

    fn active_handles(&self) -> Vec<DropHandle> {
        self.slots
            .iter()
            .filter_map(|slot| slot.value.as_ref().map(|drop| drop.handle))
            .collect()
    }

    fn get(&self, handle: DropHandle) -> Option<&ItemDrop> {
        let slot = self.slots.get(handle.slot as usize)?;
        (slot.generation == handle.generation)
            .then_some(slot.value.as_ref())
            .flatten()
    }

    fn get_mut(&mut self, handle: DropHandle) -> Option<&mut ItemDrop> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_mut()
    }

    fn allocate(
        &mut self,
        runtime_id: u64,
        stack: ItemStack,
        position: Position,
        velocity: Velocity,
    ) -> ItemDrop {
        let slot_index = if let Some(slot) = self.free_slots.pop() {
            slot
        } else {
            let slot = self.slots.len() as u32;
            self.slots.push(DropSlot {
                generation: 0,
                value: None,
            });
            slot
        };
        let generation = self.slots[slot_index as usize].generation;
        let handle = DropHandle {
            slot: slot_index,
            generation,
        };
        let drop = ItemDrop {
            handle,
            runtime_id,
            stack,
            position,
            velocity,
            age_ticks: 0,
            pickup_delay: INITIAL_PICKUP_DELAY,
            sleeping: false,
            on_ground: false,
        };
        self.slots[slot_index as usize].value = Some(drop.clone());
        self.by_cell
            .entry(CellKey::from_position(position))
            .or_default()
            .push(handle);
        self.by_runtime_id.insert(runtime_id, handle);
        drop
    }

    fn remove(&mut self, handle: DropHandle) -> Option<ItemDrop> {
        let slot = self.slots.get_mut(handle.slot as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        let value = slot.value.take()?;
        let cell = CellKey::from_position(value.position);
        if let Some(handles) = self.by_cell.get_mut(&cell) {
            handles.retain(|candidate| *candidate != handle);
            if handles.is_empty() {
                self.by_cell.remove(&cell);
            }
        }
        self.by_runtime_id.remove(&value.runtime_id);
        slot.generation = slot.generation.wrapping_add(1);
        self.free_slots.push(handle.slot);
        Some(value)
    }

    /// Whether this runtime id's drop still lives in this store.
    fn contains_runtime_id(&self, runtime_id: u64) -> bool {
        self.by_runtime_id
            .get(&runtime_id)
            .and_then(|handle| self.get(*handle))
            .is_some()
    }

    fn move_to(&mut self, handle: DropHandle, position: Position) {
        let Some(old_position) = self.get(handle).map(|drop| drop.position) else {
            return;
        };
        let old_cell = CellKey::from_position(old_position);
        let new_cell = CellKey::from_position(position);
        if let Some(drop) = self.get_mut(handle) {
            drop.position = position;
        }
        if old_cell == new_cell {
            return;
        }
        if let Some(handles) = self.by_cell.get_mut(&old_cell) {
            handles.retain(|candidate| *candidate != handle);
            if handles.is_empty() {
                self.by_cell.remove(&old_cell);
            }
        }
        self.by_cell.entry(new_cell).or_default().push(handle);
    }

    /// Merge on `age % 60 == 0` for on-ground survivors with stack room,
    /// absorbing in-range same-item on-ground neighbors wholesale.
    fn absorb_neighbors(
        &mut self,
        handle: DropHandle,
        world_id: &MinecraftWorldId,
        registry: &ItemRegistry,
        publisher: &mut IntentPublisher<'_>,
    ) {
        loop {
            let Some(drop) = self.get(handle) else {
                return;
            };
            let (runtime_id, stack, position, on_ground, age) = (
                drop.runtime_id,
                drop.stack.clone(),
                drop.position,
                drop.on_ground,
                drop.age_ticks,
            );
            // Merge gate: on-ground, on a 60-tick boundary, with stack room.
            let max_stack = registry.max_stack_size(stack.runtime_id).max(1);
            if !on_ground || age % MERGE_INTERVAL_TICKS != 0 || stack.count >= max_stack {
                return;
            }
            let Some(partner) = self.find_merge_partner(handle, position, &stack, max_stack) else {
                return;
            };
            let Some(absorbed) = self.remove(partner) else {
                continue;
            };
            let new_count = stack.count + absorbed.stack.count;
            if let Some(survivor) = self.get_mut(handle) {
                survivor.stack.count = new_count;
            }
            // Close first (RemoveEntity), then broadcast the new count.
            publisher.publish(DespawnItemEntity {
                world_id: world_id.clone(),
                runtime_id: absorbed.runtime_id,
            });
            publisher.publish(UpdateItemStackSize {
                world_id: world_id.clone(),
                runtime_id,
                count: new_count,
            });
        }
    }

    /// Neighbors within range, on ground, same item, fitting stacks.
    fn find_merge_partner(
        &self,
        survivor: DropHandle,
        position: Position,
        survivor_stack: &ItemStack,
        max_stack: u16,
    ) -> Option<DropHandle> {
        let center = CellKey::from_position(position);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let cell = CellKey {
                        x: center.x + dx,
                        y: center.y + dy,
                        z: center.z + dz,
                    };
                    let Some(handles) = self.by_cell.get(&cell) else {
                        continue;
                    };
                    for handle in handles {
                        if *handle == survivor {
                            continue;
                        }
                        let Some(drop) = self.get(*handle) else {
                            continue;
                        };
                        let same_item = drop.stack.runtime_id == survivor_stack.runtime_id
                            && drop.stack.damage == survivor_stack.damage
                            && drop.stack.block_runtime_id == survivor_stack.block_runtime_id;
                        let other = drop.position;
                        if !same_item
                            || !drop.on_ground
                            || drop.stack.count.saturating_add(survivor_stack.count) > max_stack
                            || (other.x - position.x).abs() > MERGE_AXIS_RANGE
                            || (other.y - position.y).abs() > MERGE_AXIS_RANGE
                            || (other.z - position.z).abs() > MERGE_AXIS_RANGE
                        {
                            continue;
                        }
                        return Some(*handle);
                    }
                }
            }
        }
        None
    }
}

/// Merge pass (each survivor attempts one merge).
fn merge_pass(
    store: &mut RegionItemDropStore,
    world_id: &MinecraftWorldId,
    registry: &ItemRegistry,
    publisher: &mut IntentPublisher<'_>,
) {
    for handle in store.active_handles() {
        store.absorb_neighbors(handle, world_id, registry, publisher);
    }
}

/// Root-owned collection of per-world drop stores.
///
/// The store is intentionally partitioned by world and spatial cell. A future
/// region owner can move one `RegionItemDropStore` behind a region mailbox
/// without changing the network or item semantics.
#[derive(Resource, Default)]
pub struct ItemDropStore {
    worlds: HashMap<MinecraftWorldId, RegionItemDropStore>,
    next_runtime_id: u64,
}

impl ItemDropStore {
    /// Max live drops per world (bounded backpressure).
    pub const MAX_ACTIVE_DROPS_PER_WORLD: usize = 4096;

    pub fn spawn(
        &mut self,
        world_id: MinecraftWorldId,
        stack: ItemStack,
        position: Position,
        velocity: Velocity,
        registry: &ItemRegistry,
        publisher: &mut IntentPublisher<'_>,
    ) {
        let _ = self.spawn_checked(world_id, stack, position, velocity, registry, publisher);
    }

    /// Bounded spawn: `true` on success, `false` on busy (committed
    /// facts are kept, never silently dropped).
    pub fn spawn_checked(
        &mut self,
        world_id: MinecraftWorldId,
        mut stack: ItemStack,
        position: Position,
        velocity: Velocity,
        registry: &ItemRegistry,
        publisher: &mut IntentPublisher<'_>,
    ) -> bool {
        if stack.is_empty() {
            return true;
        }
        let max_stack = registry.max_stack_size(stack.runtime_id).max(1);
        // Group count by stack cap, total unchanged.
        let needed = stack.count.div_ceil(max_stack) as usize;
        let active = self
            .worlds
            .get(&world_id)
            .map(|s| s.by_runtime_id.len())
            .unwrap_or(0);
        if active.saturating_add(needed) > Self::MAX_ACTIVE_DROPS_PER_WORLD {
            return false;
        }
        let mut next_runtime_id = self.next_runtime_id.max(1_000_000);
        let store = self.worlds.entry(world_id.clone()).or_default();

        // No spawn-time merge: drops fall independently and merge on
        // tick by stack-max grouping only.
        while stack.count > 0 {
            let count = max_stack.min(stack.count);
            stack.count -= count;
            next_runtime_id = next_runtime_id.wrapping_add(1);
            let drop = store.allocate(
                next_runtime_id,
                ItemStack {
                    count,
                    ..stack.clone()
                },
                position,
                velocity,
            );
            publisher.publish(SpawnItemEntity {
                world_id: world_id.clone(),
                runtime_id: drop.runtime_id,
                x: drop.position.x,
                y: drop.position.y,
                z: drop.position.z,
                motion_x: drop.velocity.x,
                motion_y: drop.velocity.y,
                motion_z: drop.velocity.z,
                stack: drop.stack,
            });
        }
        self.next_runtime_id = next_runtime_id;
        true
    }

    pub fn tick(
        &mut self,
        manager: Option<&MinecraftWorldManager>,
        registry: &ItemRegistry,
        publisher: &mut IntentPublisher<'_>,
    ) {
        for (world_id, store) in self.worlds.iter_mut() {
            // Merge checks run before physics.
            merge_pass(store, &world_id, registry, publisher);
            let handles = store.active_handles();
            for handle in handles {
                let Some(snapshot) = store.get(handle).map(|drop| {
                    (
                        drop.runtime_id,
                        drop.position,
                        drop.velocity,
                        drop.age_ticks,
                        drop.pickup_delay,
                        drop.sleeping,
                    )
                }) else {
                    continue;
                };
                let (runtime_id, position, velocity, age, pickup_delay, sleeping) = snapshot;
                if age >= LIFETIME_TICKS {
                    store.remove(handle);
                    publisher.publish(DespawnItemEntity {
                        world_id: world_id.clone(),
                        runtime_id,
                    });
                    continue;
                }

                let next_age = age.saturating_add(1);
                let next_pickup_delay = pickup_delay.saturating_sub(1);
                let mut next_position = position;
                let mut next_velocity = velocity;
                let mut next_sleeping = sleeping;
                let mut on_ground = sleeping;

                if sleeping {
                    // Sleeping drops re-validate every tick: wake when the
                    // support is gone, eject when entombed. Otherwise a
                    // broken floor leaves floating drops behind forever.
                    if cell_is_solid(
                        manager,
                        &world_id,
                        position.x.floor() as i32,
                        position.y.floor() as i32,
                        position.z.floor() as i32,
                    ) {
                        if let Some(freed) =
                            eject_up_to_air(manager, &world_id, position)
                        {
                            next_position = freed;
                            next_velocity = Velocity::default();
                            next_sleeping = false;
                            on_ground = false;
                        }
                    } else if support_top_below(
                        manager,
                        &world_id,
                        next_position.x,
                        next_position.y,
                        next_position.z,
                    )
                    .is_none()
                    {
                        next_sleeping = false;
                        next_velocity = Velocity::default();
                        on_ground = false;
                    }
                }

                if !next_sleeping {
                    next_velocity.y = (next_velocity.y - GRAVITY).max(-TERMINAL_VELOCITY);
                    next_velocity.x *= DRAG;
                    next_velocity.y *= DRAG;
                    next_velocity.z *= DRAG;

                    // Eject first when starting inside a solid (break cell
                    // neighbors, re-logged or replaced blocks).
                    if cell_is_solid(
                        manager,
                        &world_id,
                        next_position.x.floor() as i32,
                        next_position.y.floor() as i32,
                        next_position.z.floor() as i32,
                    ) {
                        if let Some(freed) =
                            eject_up_to_air(manager, &world_id, next_position)
                        {
                            next_position = freed;
                            next_velocity = Velocity::default();
                        }
                    }

                    // Per-axis horizontal collision: never drift into solids.
                    let probe_x = next_position.x + next_velocity.x;
                    if cell_is_solid(
                        manager,
                        &world_id,
                        probe_x.floor() as i32,
                        next_position.y.floor() as i32,
                        next_position.z.floor() as i32,
                    ) {
                        next_velocity.x = 0.0;
                    } else {
                        next_position.x = probe_x;
                    }
                    let probe_z = next_position.z + next_velocity.z;
                    if cell_is_solid(
                        manager,
                        &world_id,
                        next_position.x.floor() as i32,
                        next_position.y.floor() as i32,
                        probe_z.floor() as i32,
                    ) {
                        next_velocity.z = 0.0;
                    } else {
                        next_position.z = probe_z;
                    }

                    // Vertical move with landing on the support top.
                    let probe_y = next_position.y + next_velocity.y;
                    if next_velocity.y <= 0.0
                        && cell_is_solid(
                            manager,
                            &world_id,
                            next_position.x.floor() as i32,
                            probe_y.floor() as i32,
                            next_position.z.floor() as i32,
                        )
                    {
                        next_position.y = probe_y.floor() + 1.0 + ITEM_REST_OFFSET;
                        next_velocity = Velocity::default();
                        next_sleeping = true;
                        on_ground = true;
                    } else {
                        next_position.y = probe_y;
                        if cell_is_solid(
                            manager,
                            &world_id,
                            next_position.x.floor() as i32,
                            next_position.y.floor() as i32,
                            next_position.z.floor() as i32,
                        ) {
                            // Moving up through a ceiling (or still wedged):
                            // bonk in place rather than phasing through.
                            next_position.y = position.y;
                            next_velocity.y = 0.0;
                        } else {
                            on_ground = false;
                        }
                    }
                }

                store.move_to(handle, next_position);
                if let Some(drop) = store.get_mut(handle) {
                    drop.velocity = next_velocity;
                    drop.age_ticks = next_age;
                    drop.pickup_delay = next_pickup_delay;
                    drop.sleeping = next_sleeping;
                    drop.on_ground = on_ground;
                }

                // Publish moves, including the landing transition itself:
                // without it the client keeps the last in-air position and
                // the item visually floats above its rest spot.
                if position.distance_squared(&next_position) > 0.0001
                    && !(sleeping && next_sleeping)
                {
                    publisher.publish(MoveItemEntity {
                        world_id: world_id.clone(),
                        runtime_id,
                        x: next_position.x,
                        y: next_position.y,
                        z: next_position.z,
                        on_ground,
                    });
                }
            }
        }
        self.worlds.retain(|_, store| !store.is_empty());
    }

    /// Pickup drops inside the grown collector box with expired pickup
    /// delay. Non-creative mode requires full fit; the take packet
    /// broadcasts first, then bag insert, then close.
    pub fn try_pickup(
        &mut self,
        world_id: &MinecraftWorldId,
        collector_runtime_id: u64,
        collector_box: &Aabb,
        inventory: &PlayerInventory,
        creative: bool,
        registry: &ItemRegistry,
        publisher: &mut IntentPublisher<'_>,
    ) {
        let Some(store) = self.worlds.get_mut(world_id) else {
            return;
        };
        for handle in store.handles_in_box(collector_box) {
            let Some((runtime_id, stack, pickup_delay, position)) = store.get(handle).map(|drop| {
                (
                    drop.runtime_id,
                    drop.stack.clone(),
                    drop.pickup_delay,
                    drop.position,
                )
            }) else {
                continue;
            };
            // Pickup needs pickupDelay <= 0 within the player box.
            if pickup_delay > 0
                || position.x < collector_box.min_x
                || position.x > collector_box.max_x
                || position.y < collector_box.min_y
                || position.y > collector_box.max_y
                || position.z < collector_box.min_z
                || position.z > collector_box.max_z
            {
                continue;
            }
            if !creative && !inventory.can_add(stack.clone(), registry) {
                continue;
            }
            if store.remove(handle).is_none() {
                continue;
            }
            // Creative mode also fills the server bag (no exemption).
            let (_leftover, changed_slots) = inventory.insert_tracked(stack, registry);
            // Order: take packet, bag insert, close. The client plays
            // the absorb animation on TakeItemEntity; close needs no delay.
            publisher.publish(TakeItemEntity {
                world_id: world_id.clone(),
                runtime_id,
                target_entity_id: collector_runtime_id,
            });
            for slot in changed_slots {
                if let Some(current) = inventory.get(slot) {
                    publisher.publish(UpdateInventorySlot {
                        entity_id: collector_runtime_id,
                        slot: slot as u8,
                        stack: current,
                    });
                }
            }
            publisher.publish(DespawnItemEntity {
                world_id: world_id.clone(),
                runtime_id,
            });
        }
    }

    /// Whether the drop still lives in this world.
    ///
    /// Existence check before translating movement: skipped
    /// broadcasts for removed drops keep stale move packets away.
    pub fn contains(&self, world_id: &MinecraftWorldId, runtime_id: u64) -> bool {
        self.worlds
            .get(world_id)
            .is_some_and(|store| store.contains_runtime_id(runtime_id))
    }

    pub fn active_count(&self) -> usize {
        self.worlds
            .values()
            .map(|store| store.active_handles().len())
            .sum()
    }
}

impl RegionItemDropStore {
    /// Handles of drops in cells overlapped by an AABB (pickup candidates).
    fn handles_in_box(&self, box_: &Aabb) -> Vec<DropHandle> {
        let min_cell = CellKey {
            x: (box_.min_x / CELL_SIZE).floor() as i32,
            y: (box_.min_y / CELL_SIZE).floor() as i32,
            z: (box_.min_z / CELL_SIZE).floor() as i32,
        };
        let max_cell = CellKey {
            x: (box_.max_x / CELL_SIZE).floor() as i32,
            y: (box_.max_y / CELL_SIZE).floor() as i32,
            z: (box_.max_z / CELL_SIZE).floor() as i32,
        };
        let mut handles = Vec::new();
        for x in min_cell.x..=max_cell.x {
            for y in min_cell.y..=max_cell.y {
                for z in min_cell.z..=max_cell.z {
                    if let Some(cell_handles) = self.by_cell.get(&CellKey { x, y, z }) {
                        handles.extend(cell_handles.iter().copied());
                    }
                }
            }
        }
        handles
    }
}

/// Rest height above a supporting block top (item-actor center).
const ITEM_REST_OFFSET: f32 = 0.15;
/// Upward eject scan height when a drop starts inside a solid.
const EJECT_SCAN_HEIGHT: i32 = 3;

fn cell_is_solid(
    manager: Option<&MinecraftWorldManager>,
    world_id: &MinecraftWorldId,
    bx: i32,
    by: i32,
    bz: i32,
) -> bool {
    let Some(manager) = manager else {
        return false;
    };
    let Some(world) = manager.get_world(world_id) else {
        return false;
    };
    let key = sc_world::storage::ChunkKey::new(
        world.world_data.get_dimension(),
        sc_world::chunk::ChunkPosition::from_world(bx, bz),
    );
    let Some(column) = world.chunk_provider.cached_chunk(key) else {
        return false;
    };
    let chunk = column.read();
    let lx = bx.rem_euclid(16) as u8;
    let lz = bz.rem_euclid(16) as u8;
    chunk
        .block_at(lx, by, lz)
        .map(|runtime_id| runtime_id.0 != sc_world::block_dictionary::air_runtime_id())
        .unwrap_or(false)
}

/// Highest supporting block top at or below the position (up to 2 cells
/// down). `None` means free fall (or unloaded columns, which count as air).
fn support_top_below(
    manager: Option<&MinecraftWorldManager>,
    world_id: &MinecraftWorldId,
    x: f32,
    y: f32,
    z: f32,
) -> Option<f32> {
    let base_y = (y - 0.2).floor() as i32;
    for offset in 0..3 {
        let by = base_y - offset;
        if cell_is_solid(
            manager,
            world_id,
            x.floor() as i32,
            by,
            z.floor() as i32,
        ) {
            return Some(by as f32 + 1.0);
        }
    }
    None
}

/// Pop a drop out of a solid cell to the first air cell above (up to
/// [`EJECT_SCAN_HEIGHT`]), keeping x/z and centering on the freed cell.
fn eject_up_to_air(
    manager: Option<&MinecraftWorldManager>,
    world_id: &MinecraftWorldId,
    position: Position,
) -> Option<Position> {
    let bx = position.x.floor() as i32;
    let by = position.y.floor() as i32;
    let bz = position.z.floor() as i32;
    for dy in 1..=EJECT_SCAN_HEIGHT {
        if !cell_is_solid(manager, world_id, bx, by + dy, bz) {
            return Some(Position::new(
                position.x,
                (by + dy) as f32 + 0.5,
                position.z,
            ));
        }
    }
    None
}

/// Update all ordinary drops. The store, not an async task per item, owns the
/// tick and can later be invoked by a region owner for its local cells.
pub fn item_drop_tick(
    world: sc_ecs::world::World,
    mut store: sc_ecs::params::resource::ResMut<ItemDropStore>,
    registry: sc_ecs::params::resource::Res<ItemRegistry>,
    mut outbox: sc_ecs::params::resource::ResMut<NetworkOutbox>,
) {
    let manager = world.get_resource::<MinecraftWorldManager>();
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .maybe_with_faults(faults.as_deref_mut());
    store.tick(manager.as_deref(), &registry, &mut publisher);
}

/// Pickup check for non-spectator players every tick.
pub fn item_pickup_tick(
    world: sc_ecs::world::World,
    mut store: sc_ecs::params::resource::ResMut<ItemDropStore>,
    registry: sc_ecs::params::resource::Res<ItemRegistry>,
    mut outbox: sc_ecs::params::resource::ResMut<NetworkOutbox>,
    mut pending_resync: sc_ecs::params::resource::ResMut<PendingInventoryResync>,
) {
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    let mut publisher = IntentPublisher::new(&mut outbox)
        .with_world(&world)
        .with_pending_resync(&mut pending_resync)
        .maybe_with_faults(faults.as_deref_mut());
    for entity in world.entities_with_component::<sc_utils::game::client::MinecraftClient>() {
        let Some(client) = world.get_component::<sc_utils::game::client::MinecraftClient>(&entity)
        else {
            continue;
        };
        let (creative, spectator) = {
            let data = client.data.read();
            (data.gamemode.is_creative(), data.gamemode.is_spectator())
        };
        // Spectators never pick up.
        if spectator {
            continue;
        }
        let Some(transform) = world.get_component::<sc_entity::motion::Transform>(&entity) else {
            continue;
        };
        let position = {
            let data = transform.read();
            data.position
        };
        let Some(entity_id) = world.get_component::<sc_entity::MinecraftEntityId>(&entity) else {
            continue;
        };
        let Some(world_id) = world.get_component::<MinecraftWorldId>(&entity) else {
            continue;
        };
        let Some(inventory) = world.get_component::<PlayerInventory>(&entity) else {
            continue;
        };
        // Player box grown by (1, 0.5, 1).
        let physics = world
            .get_component::<sc_entity::motion::PhysicsBody>(&entity)
            .map(|body| body.as_ref().clone())
            .unwrap_or_else(sc_entity::motion::PhysicsBody::player);
        let player_box = physics.aabb.at(position.x, position.y, position.z);
        let grown = Aabb {
            min_x: player_box.min_x - 1.0,
            min_y: player_box.min_y - 0.5,
            min_z: player_box.min_z - 1.0,
            max_x: player_box.max_x + 1.0,
            max_y: player_box.max_y + 0.5,
            max_z: player_box.max_z + 1.0,
        };
        // Pickup changes advance the authoritative revision.
        let before = inventory.snapshot();
        store.try_pickup(
            world_id.as_ref(),
            entity_id.0,
            &grown,
            inventory.as_ref(),
            creative,
            &registry,
            &mut publisher,
        );
        if inventory.snapshot() != before {
            crate::inventory::crafting::bump_inventory_revision(&world, &entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::NetworkIntent;
    use sc_item::ItemDefinition;

    fn registry(max_stack: u16) -> ItemRegistry {
        let registry = ItemRegistry::new();
        registry.upsert(ItemDefinition::new(1, "minecraft:stone").max_stack_size(max_stack));
        registry
    }

    fn resting(store: &mut RegionItemDropStore, handle: DropHandle) {
        if let Some(drop) = store.get_mut(handle) {
            drop.on_ground = true;
            drop.sleeping = true;
            drop.age_ticks = MERGE_INTERVAL_TICKS;
            drop.pickup_delay = 0;
        }
    }

    #[test]
    fn handles_recycle_with_generation_change() {
        let mut store = RegionItemDropStore::default();
        let first = store.allocate(
            1,
            ItemStack::new(1, 1),
            Position::new(0.0, 0.0, 0.0),
            Velocity::default(),
        );
        let first_handle = first.handle;
        assert!(store.remove(first_handle).is_some());
        let second = store.allocate(
            2,
            ItemStack::new(1, 1),
            Position::new(0.0, 0.0, 0.0),
            Velocity::default(),
        );
        assert_eq!(first_handle.slot, second.handle.slot);
        assert_ne!(first_handle.generation, second.handle.generation);
        assert!(store.get(first_handle).is_none());
        assert!(store.get(second.handle).is_some());
    }

    #[test]
    fn merge_absorbs_neighbor_and_updates_stack_size() {
        let mut store = RegionItemDropStore::default();
        let a = store.allocate(
            10,
            ItemStack::new(1, 1),
            Position::new(0.0, 64.0, 0.0),
            Velocity::default(),
        );
        let b = store.allocate(
            11,
            ItemStack::new(1, 2),
            Position::new(1.0, 64.0, 0.0),
            Velocity::default(),
        );
        resting(&mut store, a.handle);
        resting(&mut store, b.handle);

        let registry = registry(64);
        let mut outbox = NetworkOutbox::default();
        merge_pass(
            &mut store,
            &MinecraftWorldId::random(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );

        // Survivors keep runtime id and position with a merged count of 3.
        let survivor = store.get(a.handle).expect("survivor kept");
        assert_eq!(survivor.runtime_id, 10);
        assert_eq!(survivor.stack.count, 3);
        assert_eq!(survivor.position, Position::new(0.0, 64.0, 0.0));
        assert!(store.get(b.handle).is_none());

        let intents = outbox.drain();
        assert!(matches!(
            intents[0],
            NetworkIntent::DespawnItemEntity(ref p) if p.runtime_id == 11
        ));
        assert!(matches!(
            intents[1],
            NetworkIntent::UpdateItemStackSize(ref p) if p.runtime_id == 10 && p.count == 3
        ));
    }

    #[test]
    fn merge_skips_when_total_exceeds_max_stack() {
        let mut store = RegionItemDropStore::default();
        let a = store.allocate(
            10,
            ItemStack::new(1, 2),
            Position::new(0.0, 64.0, 0.0),
            Velocity::default(),
        );
        let b = store.allocate(
            11,
            ItemStack::new(1, 2),
            Position::new(0.5, 64.0, 0.0),
            Velocity::default(),
        );
        resting(&mut store, a.handle);
        resting(&mut store, b.handle);

        // max stack = 3: 2 + 2 = 4 > 3 skips partial merge.
        let registry = registry(3);
        let mut outbox = NetworkOutbox::default();
        merge_pass(
            &mut store,
            &MinecraftWorldId::random(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );

        assert!(store.get(a.handle).is_some());
        assert!(store.get(b.handle).is_some());
        assert!(outbox.drain().is_empty());
    }

    #[test]
    fn merge_requires_on_ground_and_interval() {
        let mut store = RegionItemDropStore::default();
        let a = store.allocate(
            10,
            ItemStack::new(1, 1),
            Position::new(0.0, 64.0, 0.0),
            Velocity::default(),
        );
        let b = store.allocate(
            11,
            ItemStack::new(1, 1),
            Position::new(0.2, 64.0, 0.2),
            Velocity::default(),
        );
        // One landed drop at age 61 (off the 60 multiple); other airborne.
        resting(&mut store, a.handle);
        if let Some(drop) = store.get_mut(a.handle) {
            drop.age_ticks = MERGE_INTERVAL_TICKS + 1;
        }

        let registry = registry(64);
        let mut outbox = NetworkOutbox::default();
        merge_pass(
            &mut store,
            &MinecraftWorldId::random(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        assert!(store.get(a.handle).is_some());
        assert!(store.get(b.handle).is_some());

        // Both landed with b at a 60 multiple merges (b absorbs a;
        // each drop initiates independently, survivor varies).
        resting(&mut store, b.handle);
        merge_pass(
            &mut store,
            &MinecraftWorldId::random(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        let survivors: Vec<_> = [a.handle, b.handle]
            .into_iter()
            .filter(|handle| store.get(*handle).is_some())
            .collect();
        assert_eq!(survivors.len(), 1);
        assert_eq!(store.get(survivors[0]).unwrap().stack.count, 2);
    }

    #[test]
    fn spawn_does_not_merge_and_splits_by_max_stack() {
        let mut drops = ItemDropStore::default();
        let registry = registry(2);
        let mut outbox = NetworkOutbox::default();
        drops.spawn(
            MinecraftWorldId::random(),
            ItemStack::new(1, 5),
            Position::new(0.0, 64.0, 0.0),
            Velocity::default(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        // max stack 2: 5 splits into 3 groups; no spawn-time merge.
        assert_eq!(drops.active_count(), 3);
        let spawn_intents = outbox
            .drain()
            .into_iter()
            .filter(|intent| matches!(intent, NetworkIntent::SpawnItemEntity(_)))
            .count();
        assert_eq!(spawn_intents, 3);
    }

    #[test]
    fn try_pickup_collects_drop_into_inventory() {
        let world_id = MinecraftWorldId::random();
        let mut drops = ItemDropStore::default();
        let registry = registry(64);
        let mut outbox = NetworkOutbox::default();
        drops.spawn(
            world_id.clone(),
            ItemStack::new(1, 3),
            Position::new(0.5, 64.5, 0.5),
            Velocity::default(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        outbox.drain();
        // Skip the initial pickup delay.
        {
            let store = drops.worlds.get_mut(&world_id).unwrap();
            let handle = store.active_handles()[0];
            if let Some(drop) = store.get_mut(handle) {
                drop.pickup_delay = 0;
            }
        }

        // Player box grown: x/z +-1.3, y -0.5..+2.3.
        let box_ = Aabb {
            min_x: -1.3,
            min_y: 63.5,
            min_z: -1.3,
            max_x: 1.3,
            max_y: 66.3,
            max_z: 1.3,
        };
        let inventory = PlayerInventory::new(36);
        drops.try_pickup(
            &world_id,
            7,
            &box_,
            &inventory,
            false,
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        assert_eq!(drops.active_count(), 0);
        assert_eq!(inventory.get(0).map(|stack| stack.count), Some(3));

        let intents = outbox.drain();
        assert!(matches!(
            intents[0],
            NetworkIntent::TakeItemEntity(ref p) if p.target_entity_id == 7
        ));
        // Slot sync after bag insert (slot 0 emits an intent).
        assert!(matches!(
            intents[1],
            NetworkIntent::UpdateInventorySlot(ref p) if p.entity_id == 7 && p.slot == 0
        ));
        // Close syncs after pickup: TakeItem, sendSlot, RemoveEntity.
        assert!(matches!(
            intents[2],
            NetworkIntent::DespawnItemEntity(ref p) if p.runtime_id == 1_000_001
        ));
        assert_eq!(intents.len(), 3);
        // Removed right after pickup: gone from queries, nothing to pick.
        assert!(!drops.contains(&world_id, 1_000_001));
        let second_inventory = PlayerInventory::new(36);
        drops.try_pickup(
            &world_id,
            7,
            &box_,
            &second_inventory,
            false,
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        assert!(second_inventory
            .get(0)
            .is_some_and(|stack| stack.count == 0));
        assert!(outbox.drain().is_empty());
    }

    #[test]
    fn try_pickup_respects_delay_and_full_inventory() {
        let world_id = MinecraftWorldId::random();
        let registry = registry(64);
        let mut drops = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        drops.spawn(
            world_id.clone(),
            ItemStack::new(1, 2),
            Position::new(0.5, 64.5, 0.5),
            Velocity::default(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        outbox.drain();
        let box_ = Aabb {
            min_x: -1.3,
            min_y: 63.5,
            min_z: -1.3,
            max_x: 1.3,
            max_y: 66.3,
            max_z: 1.3,
        };

        // pickupDelay (10 ticks) not yet reached: no pickup.
        let inventory = PlayerInventory::new(36);
        drops.try_pickup(
            &world_id,
            7,
            &box_,
            &inventory,
            false,
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        assert_eq!(drops.active_count(), 1);
        assert!(outbox.drain().is_empty());

        // Delay expired but the bag is full: no pickup.
        {
            let store = drops.worlds.get_mut(&world_id).unwrap();
            let handle = store.active_handles()[0];
            if let Some(drop) = store.get_mut(handle) {
                drop.pickup_delay = 0;
            }
        }
        let small = PlayerInventory::new(1);
        small.set(0, ItemStack::new(2, 2));
        drops.try_pickup(
            &world_id,
            7,
            &box_,
            &small,
            false,
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        assert_eq!(drops.active_count(), 1);
        assert!(outbox.drain().is_empty());

        // Creative mode skips the capacity check: pickup succeeds.
        drops.try_pickup(
            &world_id,
            7,
            &box_,
            &small,
            true,
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        assert_eq!(drops.active_count(), 0);
    }

    /// One-column test world: floor at y=64 plus caller-supplied solids,
    /// returning the store world id, the manager-backed tick inputs and the
    /// shared chunk column for block edits between ticks.
    fn physics_world(
        solids: &[(i32, i32, i32)],
    ) -> (
        MinecraftWorldId,
        MinecraftWorldManager,
        std::sync::Arc<sc_world::storage::ChunkColumn>,
        ItemRegistry,
    ) {
        use sc_world::chunk::{BlockRuntimeId, ChunkPosition};
        use sc_world::storage::{ChunkKey, EmptyWorldStorage, WorldChunkProvider};
        let provider = WorldChunkProvider::new(std::sync::Arc::new(EmptyWorldStorage));
        let column = provider
            .insert_empty_chunk(ChunkKey::new(0, ChunkPosition::new(0, 0)), -64, 319)
            .unwrap();
        {
            let mut chunk = column.write();
            for x in -1..3 {
                chunk.set_block_at(0, x as u8, 64, 0, BlockRuntimeId(32000));
            }
            for (x, y, z) in solids {
                chunk.set_block_at(0, *x as u8, *y, *z as u8, BlockRuntimeId(32000));
            }
        }
        let mut manager = MinecraftWorldManager::new();
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
        (world_id, manager, column, registry(64))
    }

    fn tick_once(
        drops: &mut ItemDropStore,
        manager: &MinecraftWorldManager,
        registry: &ItemRegistry,
        outbox: &mut NetworkOutbox,
    ) {
        drops.tick(
            Some(manager),
            registry,
            &mut IntentPublisher::new(outbox),
        );
    }

    fn drop_state(
        drops: &ItemDropStore,
        world_id: &MinecraftWorldId,
    ) -> (Position, Velocity, bool) {
        let store = drops.worlds.get(world_id).unwrap();
        let handle = store.active_handles()[0];
        let drop = store.get(handle).unwrap();
        (drop.position, drop.velocity, drop.sleeping)
    }

    #[test]
    fn drop_stops_at_wall_instead_of_phasing() {
        let (world_id, manager, _column, registry) = physics_world(&[(1, 65, 0)]);
        let mut drops = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        drops.spawn(
            world_id.clone(),
            ItemStack::new(1, 1),
            Position::new(0.5, 65.15, 0.5),
            Velocity::new(0.5, 0.0, 0.0),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        for _ in 0..3 {
            tick_once(&mut drops, &manager, &registry, &mut outbox);
        }
        let (pos, vel, _) = drop_state(&drops, &world_id);
        // Never entered the wall cell (x=1); horizontal motion died there.
        // (Landing on the floor and sleeping is fine.)
        assert!(pos.x < 1.0, "phased into wall: {pos:?}");
        assert_eq!(vel.x, 0.0);
    }

    #[test]
    fn drop_ejects_when_starting_inside_solid() {
        let (world_id, manager, _column, registry) =
            physics_world(&[(0, 65, 0), (0, 66, 0)]);
        let mut drops = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        drops.spawn(
            world_id.clone(),
            ItemStack::new(1, 1),
            Position::new(0.5, 65.2, 0.5),
            Velocity::default(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        tick_once(&mut drops, &manager, &registry, &mut outbox);
        let (pos, _, sleeping) = drop_state(&drops, &world_id);
        // Popped above the 2-high solid column, awake and falling.
        assert!(pos.y > 67.0, "still entombed: {pos:?}");
        assert!(!sleeping);
    }

    #[test]
    fn sleeping_drop_wakes_when_support_is_broken() {
        use sc_world::chunk::BlockRuntimeId;
        let (world_id, manager, column, registry) = physics_world(&[]);
        let mut drops = ItemDropStore::default();
        let mut outbox = NetworkOutbox::default();
        drops.spawn(
            world_id.clone(),
            ItemStack::new(1, 1),
            Position::new(0.5, 66.0, 0.5),
            Velocity::default(),
            &registry,
            &mut IntentPublisher::new(&mut outbox),
        );
        for _ in 0..30 {
            tick_once(&mut drops, &manager, &registry, &mut outbox);
        }
        let (landed, _, sleeping) = drop_state(&drops, &world_id);
        assert!(sleeping, "never landed");
        assert!(
            (landed.y - 65.15).abs() < 1e-4,
            "rest height off support top: {landed:?}"
        );
        // Break the floor: the drop must wake instead of floating.
        column.write().set_block_at(
            0,
            0,
            64,
            0,
            BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()),
        );
        tick_once(&mut drops, &manager, &registry, &mut outbox);
        let (_, _, sleeping) = drop_state(&drops, &world_id);
        assert!(!sleeping, "sleeps floating over broken support");
    }

}
