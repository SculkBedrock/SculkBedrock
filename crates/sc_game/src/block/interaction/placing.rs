//! Block placement: face offset, item mapping, spend, reserve and reconcile.
//!
//! Moved verbatim from `interaction.rs` (P4 split); shared types and helpers
//! live in [`super`] and are re-exported there so existing paths keep compiling.

use super::*;
use log::{info, warn};
use sc_block::position::BlockPosition;
use sc_block::registry::BlockStateRegistry;
use sc_block::write::{
    update_flags, BlockChange, BlockChangeCause, BlockChangeQueue,
    BlockChangeResult,
};
use sc_ecs::entity::EntityId;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Transform};
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
use sc_world::chunk::BlockRuntimeId;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use crate::net::NetworkOutbox;

fn target_is_replaceable(
    manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    target: &BlockPosition,
) -> bool {
    block_at(manager, world_id, target)
        == Some(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
}

fn target_intersects_player(world: &World, entity: &EntityId, target: &BlockPosition) -> bool {
    let Some(transform) = world.get_component::<Transform>(entity) else {
        return false;
    };
    let physics = world
        .get_component::<PhysicsBody>(entity)
        .map(|body| body.as_ref().clone())
        .unwrap_or_else(PhysicsBody::player);
    let pos = { transform.read().position };
    let player_aabb = physics.aabb.at(pos.x, pos.y, pos.z);
    let block_aabb = sc_entity::motion::Aabb {
        min_x: target.x as f32,
        min_y: target.y as f32,
        min_z: target.z as f32,
        max_x: target.x as f32 + 1.0,
        max_y: target.y as f32 + 1.0,
        max_z: target.z as f32 + 1.0,
    };
    player_aabb.intersects(&block_aabb)
}

/// 尝试与目标方块交互（如打开箱子、工作台、熔炉等容器/操作台）。
///
/// 若成功触发交互或条件不满足但已吞掉该事件，返回 `true`；
/// 若目标方块非可交互方块，返回 `false`。
///
/// Tries to interact with the target block (opening a chest, crafting table,
/// furnace or other container/workstation). Returns `true` when the
/// interaction fired or the event was consumed despite unmet conditions;
/// returns `false` when the block is not interactive.
pub fn try_interact_block(
    world: &World,
    entity: EntityId,
    position: BlockPosition,
    outbox: &mut NetworkOutbox,
) -> bool {
    if crate::inventory::chest::open_chest_container(
        world,
        entity,
        position,
        outbox,
    ) {
        return true;
    }
    if crate::inventory::crafting::open_crafting_station(
        world,
        entity,
        position,
        outbox,
    ) {
        return true;
    }
    false
}

// Placement systems.

/// Placement: face offset, item-to-block mapping (ItemRegistry), inventory spend, then enqueue (branch A).
///
/// Failure paths (item has no block / empty inventory) never enqueue;
/// inventory spend reserves one first, then enqueues; callers roll back when PostUpdate apply fails.
pub fn handle_place_block(
    world: World,
    mut reader: EventReader<PlaceBlockRequest>,
    mut queue: ResMut<BlockChangeQueue>,
    item_registry: Res<ItemRegistry>,
    mut outbox: ResMut<NetworkOutbox>,
    mut reservations: ResMut<BlockPlacementReservations>,
) {
    for request in reader.read() {
        info!(
            "[interaction] PlaceBlockRequest: click={} face={} hotbar={} item_id={}",
            request.position, request.face, request.hotbar_slot, request.item_id
        );
        // When sneaking, skip block interaction (chests/crafting tables/furnaces
        // are not opened) and place the held item against the block directly;
        // sneaking with an empty hand triggers nothing.
        let is_sneaking = crate::movement::is_player_sneaking(&world, &request.entity);
        if !is_sneaking {
            if try_interact_block(&world, request.entity, request.position, &mut outbox) {
                continue;
            }
        }
        // Empty hands cannot place (clients never predict empty-hand placement, no resync needed).
        if request.item_id == 0 {
            continue;
        }
        // Item runtime id to block network runtime id (ItemRegistry version-pack driven).
        let Ok(item_runtime_id) = u16::try_from(request.item_id) else {
            warn!(
                "[interaction] invalid item runtime id {}, skipping place and resyncing",
                request.item_id
            );
            request_inventory_resync(&world, &mut outbox, &request.entity);
            continue;
        };
        let Some(block_runtime_id) = item_registry.block_runtime_id(item_runtime_id) else {
            warn!(
                "[interaction] 物品 {} 无对应方块（ItemRegistry），跳过放置并回同步",
                request.item_id
            );
            request_inventory_resync(&world, &mut outbox, &request.entity);
            continue;
        };

        // Target cell = clicked block plus face offset.
        let (dx, dy, dz) = face_offset(request.face);
        let target = BlockPosition::new(
            request.position.x + dx,
            request.position.y + dy,
            request.position.z + dz,
        );

        // Distance check (target cell).
        if !in_reach(&world, &request.entity, &target) {
            warn!(
                "[interaction] PlaceBlockRequest 超出交互距离，忽略并回同步: target={}",
                target
            );
            request_inventory_resync(&world, &mut outbox, &request.entity);
            continue;
        }

        let Some(world_id) = world.get_component::<MinecraftWorldId>(&request.entity) else {
            continue;
        };
        let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
            request_inventory_resync(&world, &mut outbox, &request.entity);
            continue;
        };
        if !target_is_replaceable(&world_manager, &world_id, &target) {
            warn!(
                "[interaction] target block is not replaceable or not loaded; skipping place: target={}",
                target
            );
            request_inventory_resync(&world, &mut outbox, &request.entity);
            continue;
        }
        if target_intersects_player(&world, &request.entity, &target) {
            warn!(
                "[interaction] target block intersects player AABB; skipping place: target={}",
                target
            );
            request_inventory_resync(&world, &mut outbox, &request.entity);
            continue;
        }

        let reservation = if is_creative(&world, &request.entity) {
            None
        } else {
            let Some(inventory) = world.get_component::<PlayerInventory>(&request.entity) else {
                warn!(
                    "[interaction] player has no inventory; skipping place: target={}",
                    target
                );
                request_inventory_resync(&world, &mut outbox, &request.entity);
                continue;
            };
            let slot = inventory.selected_slot() as usize;
            let Some(held_item) = inventory.get(slot) else {
                warn!(
                    "[interaction] selected hotbar slot {} is invalid; skipping place: target={}",
                    slot, target
                );
                request_inventory_resync(&world, &mut outbox, &request.entity);
                continue;
            };
            if held_item.runtime_id != item_runtime_id || held_item.is_empty() {
                warn!(
                    "[interaction] client item {} does not match server slot {} item {}; skipping place",
                    item_runtime_id,
                    slot,
                    held_item.runtime_id
                );
                request_inventory_resync(&world, &mut outbox, &request.entity);
                continue;
            }
            let Some(reserved_item) = inventory.reserve_one(slot) else {
                warn!(
                    "[interaction] hotbar slot {} is empty; skipping place: target={}",
                    slot, target
                );
                request_inventory_resync(&world, &mut outbox, &request.entity);
                continue;
            };
            // Authoritative revision tracks inventory changes (craft transactions compare it at execution).
            crate::inventory::crafting::bump_inventory_revision(&world, &request.entity);
            Some(InventoryReservation {
                entity: request.entity,
                inventory: (*inventory).clone(),
                slot,
                item: ItemStack {
                    count: 1,
                    ..reserved_item
                },
            })
        };

        let request_id = queue.push(BlockChange {
            world_id: world_id.as_ref().clone(),
            position: target,
            layer: 0,
            state: BlockRuntimeId(resolve_placement_state(
                &world,
                &request.entity,
                block_runtime_id,
                request.face,
                request.click_y,
                &target,
            )),
            cause: BlockChangeCause::Player(request.entity),
            flags: update_flags::DEFAULT,
        });
        if let Some(reservation) = reservation {
            reservations.pending.insert(request_id, reservation);
        }
    }
}

/// Placement orientation: data-driven facing from the player transform
/// (see [`crate::block::facing`]). Any missing input (no transform/registry)
/// keeps the item-mapped default state, so orientation never blocks a
/// place that used to succeed.
fn resolve_placement_state(
    world: &World,
    entity: &EntityId,
    base_hash: u32,
    face: u8,
    click_y: f32,
    target: &BlockPosition,
) -> u32 {
    let Some(transform) = world.get_component::<Transform>(entity) else {
        return base_hash;
    };
    let data = transform.read();
    let eye_height = world
        .get_component::<PhysicsBody>(entity)
        .map(|body| body.base_offset)
        .unwrap_or(1.62);
    let Some(registry) = world.get_resource::<BlockStateRegistry>() else {
        return base_hash;
    };
    let click_frac = if click_y.is_finite() {
        Some(click_y - click_y.floor())
    } else {
        None
    };
    let clicked_face = crate::block::facing::BlockFace::from_index(face)
        .unwrap_or(crate::block::facing::BlockFace::Up);
    let ctx = crate::block::facing::PlacementContext::new(
        clicked_face,
        data.position,
        *target,
        eye_height,
        data.rotation.yaw,
        click_frac,
    );
    crate::block::facing::orient_state(&registry, base_hash, &ctx)
}

/// Consume authoritative block-write results and roll back only rejected
/// inventory reservations.
pub fn resolve_block_placement_reservations(
    world: World,
    mut reader: EventReader<BlockChangeResult>,
    mut reservations: ResMut<BlockPlacementReservations>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    for result in reader.read() {
        let Some(reservation) = reservations.pending.remove(&result.request_id) else {
            continue;
        };
        if result.applied {
            continue;
        }
        if !reservation
            .inventory
            .restore_one(reservation.slot, &reservation.item)
        {
            warn!(
                "[interaction] failed to merge inventory rollback: request_id={} entity={:?}",
                result.request_id, reservation.entity
            );
        }
        crate::inventory::crafting::bump_inventory_revision(&world, &reservation.entity);
        request_inventory_resync(&world, &mut outbox, &reservation.entity);
    }
}
