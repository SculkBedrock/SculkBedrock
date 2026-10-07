//! Outbound intents -> protocol packets (network-domain mapping over `sc_game::NetworkOutbox`).
//!
//! Runs in `Last` (after PostUpdate), once the game domain has pushed this tick intents.
//! Two interception levels:
//! 1. **Intent level** (`sc_game::net::IntentSendHooks`): game-layer hooks, awaited one
//!    by one before translation; a veto means no translation (no packet);
//! 2. **Packet level** (`sc_network::packet_hooks::PacketSendHooks`): protocol-layer hooks,
//!    inside `send_packet` (see player_connection).
//! Recipient resolution: an intent carries a target entity (runtime id) -> the mapper looks
//! up its world -> sends to all InGame players in that world (the mover own MovePlayer is not sent to self).

use sc_ecs::async_manager::SCECSAsync;
use sc_ecs::entity::EntityId;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::MinecraftEntityId;
use sc_game::net::{
    BlockBreakProgressCue, IntentSendHook, IntentSendHooks, NetworkIntent, NetworkOutbox,
    ParticleCue, PlayerMoveMode, SoundCue,
};
use sc_log::t_log;
use sc_world::manager::MinecraftWorldId;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use crate::player_connection::{PacketSendOutcome, PlayerConnection, PlayerConnectionStatus};
use crate::protocol::client::container::ContainerClose;
use crate::protocol::client::transaction::ItemData;
use crate::protocol::server::block::UpdateBlock;
use crate::protocol::server::effects::{LevelEvent, LevelSoundEvent};
use crate::protocol::server::entity::{actor_event, EntityEvent};
use crate::protocol::server::inventory::{ContainerOpen, InventoryContent, InventorySlot};
use crate::protocol::server::item_entity::{AddItemEntity, TakeItemEntity};
use crate::protocol::server::misc::EntityMetadataEntry;
use crate::protocol::server::movement::{
    MoveEntityAbsolute, MovePlayer, MovePlayerMode, RemoveEntity, SetEntityMotion,
};
use sc_world::chunk::ChunkPosition;
use sc_world::chunk_view::ChunkView;
use sc_world::storage::ChunkKey;

/// A packet waiting to be sent (local enum, avoids Box<dyn>).
#[derive(Clone)]
enum OutPacket {
    ItemStackResponse(crate::protocol::server::crafting_response::ItemStackResponse),
    MovePlayer(MovePlayer),
    MoveEntityAbsolute(MoveEntityAbsolute),
    SetEntityMotion(SetEntityMotion),
    RemoveEntity(RemoveEntity),
    AddItemEntity(AddItemEntity),
    TakeItemEntity(TakeItemEntity),
    EntityEvent(EntityEvent),
    LevelEvent(LevelEvent),
    LevelSoundEvent(LevelSoundEvent),
    UpdateBlock(UpdateBlock, Option<BlockDeliveryTicket>),
    ContainerOpen(ContainerOpen),
    ContainerClose(ContainerClose),
    InventoryContent(InventoryContent),
    InventorySlot(InventorySlot),
}

#[derive(Clone)]
struct BlockDeliveryTicket {
    view: Arc<ChunkView>,
    world_id: MinecraftWorldId,
    key: ChunkKey,
    epoch: u64,
    incarnation: u128,
    generation: u64,
}

/// One ordered outbound batch: a single drain of intents + hook snapshot + World handle.
struct OrderedOutboxBatch {
    intents: Vec<NetworkIntent>,
    intent_hooks: Vec<Arc<dyn IntentSendHook>>,
    world: World,
    _byte_permit: OutboxBytePermit,
}

/// Globally ordered outbound channel: a **single resident consumer** translates and sends batches FIFO.
///
/// Per-tick spawned tasks used to send concurrently with no cross-task ordering, so entity
/// lifecycle packets (AddItemEntity -> TakeItemEntity -> RemoveEntity, ...) could arrive
/// out of order (e.g. Remove before Move: the client moves an already-removed entity).
/// An mpsc queue plus a single serial consumer is used instead: in-batch intent push order,
/// cross-batch tick order, one awaited send per packet (matching docs/game_logic_schedule.md,
/// "same-source-to-same-target order is preserved", upgraded here to globally ordered).
const ORDERED_OUTBOX_QUEUE_CAPACITY: usize = 64;
const ORDERED_OUTBOX_MAX_RETAINED_BYTES: usize = 8 * 1024 * 1024;
/// Per-batch entry ceiling. Bounds fan-out working memory for one translation
/// step without shrinking the queue's durable capacity: intents beyond this
/// prefix stay queued and are translated by a later batch, never dropped.
const ORDERED_OUTBOX_MAX_BATCH_ENTRIES: usize = 2048;
/// Per-batch byte target. Kept well below the retained budget so a saturated
/// queue still admits smaller batches instead of livelocking on one oversized
/// prefix request.
const ORDERED_OUTBOX_MAX_BATCH_BYTES: usize = ORDERED_OUTBOX_MAX_RETAINED_BYTES / 8;

struct OrderedOutboxQueue {
    tx: tokio::sync::mpsc::Sender<OrderedOutboxBatch>,
    retained_bytes: Arc<AtomicUsize>,
}

struct OutboxBytePermit {
    retained_bytes: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for OutboxBytePermit {
    fn drop(&mut self) {
        self.retained_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Global ordered outbox: one resident consumer preserves per-batch ordering.
/// Its byte permit remains charged while hooks/sends are in flight, not only
/// while the batch occupies a channel slot.
static ORDERED_OUTBOX_TX: OnceLock<OrderedOutboxQueue> = OnceLock::new();

fn try_reserve_outbox_bytes(
    retained_bytes: &Arc<AtomicUsize>,
    requested: usize,
    limit: usize,
) -> Option<OutboxBytePermit> {
    let mut current = retained_bytes.load(Ordering::Acquire);
    loop {
        let next = current.checked_add(requested)?;
        if next > limit {
            return None;
        }
        match retained_bytes.compare_exchange_weak(
            current,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return Some(OutboxBytePermit {
                    retained_bytes: Arc::clone(retained_bytes),
                    bytes: requested,
                });
            }
            Err(observed) => current = observed,
        }
    }
}

pub(crate) fn outbox_to_packets(world: World, mut outbox: ResMut<NetworkOutbox>) {
    if outbox.is_empty() {
        return;
    }
    // Intent hook snapshot (owned, never holds resource guards across await).
    let intent_hooks: Vec<Arc<dyn IntentSendHook>> = world
        .get_resource::<IntentSendHooks>()
        .map(|hooks| hooks.snapshot())
        .unwrap_or_default();

    let queue = ORDERED_OUTBOX_TX.get_or_init(|| {
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<OrderedOutboxBatch>(ORDERED_OUTBOX_QUEUE_CAPACITY);
        let retained_bytes = Arc::new(AtomicUsize::new(0));
        SCECSAsync::runtime().spawn(async move {
            while let Some(batch) = rx.recv().await {
                process_outbox_batch(batch).await;
            }
        });
        OrderedOutboxQueue { tx, retained_bytes }
    });

    // 1) Reserve a send slot before taking intents: without a slot the whole batch stays
    //    in the outbox for a retry next tick (drain-then-try_send would silently drop batches when full).
    let slot = match queue.tx.clone().try_reserve_owned() {
        Ok(slot) => slot,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            log::debug!(
                "ordered outbox slots saturated ({}); {} intents stay queued",
                ORDERED_OUTBOX_QUEUE_CAPACITY,
                outbox.pending()
            );
            return;
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            report_outbox_consumer_gone(outbox.pending());
            return;
        }
    };

    // 2) Admit the affordable prefix within the batch budget; leftover intents stay in the outbox (order kept).
    let admitted = outbox.admitted_prefix_len(
        ORDERED_OUTBOX_MAX_BATCH_ENTRIES,
        ORDERED_OUTBOX_MAX_BATCH_BYTES,
    );
    if admitted == 0 {
        drop(slot);
        return;
    }
    let retained_bytes = outbox
        .prefix_estimated_bytes(admitted)
        .saturating_add(
            intent_hooks
                .capacity()
                .saturating_mul(std::mem::size_of::<Arc<dyn IntentSendHook>>()),
        )
        .saturating_add(std::mem::size_of::<World>())
        .saturating_add(std::mem::size_of::<OrderedOutboxBatch>());
    let Some(byte_permit) = try_reserve_outbox_bytes(
        &queue.retained_bytes,
        retained_bytes,
        ORDERED_OUTBOX_MAX_RETAINED_BYTES,
    ) else {
        log::debug!(
            "ordered outbox byte budget saturated ({} bytes); {} intents stay queued",
            ORDERED_OUTBOX_MAX_RETAINED_BYTES,
            outbox.pending()
        );
        drop(slot);
        return;
    };
    let intents = outbox.take_admitted_prefix(
        ORDERED_OUTBOX_MAX_BATCH_ENTRIES,
        ORDERED_OUTBOX_MAX_BATCH_BYTES,
    );
    debug_assert_eq!(intents.len(), admitted);
    let batch = OrderedOutboxBatch {
        intents,
        intent_hooks,
        world: world.clone(),
        _byte_permit: byte_permit,
    };
    // The reserved slot cannot be lost between admission and send.
    slot.send(batch);
}

/// Rate-limited diagnostic for columns whose block deltas skip content versions.
fn report_delta_gap(key: ChunkKey, tracked_columns: usize) {
    log::warn!(
        "{}",
        t_log!(
            "console.outbox.gapped_column",
            column = format!("{key:?}"),
            tracked = tracked_columns
        )
    );
}

/// The ordered consumer only disappears when its runtime is shutting down.
/// Report it as a terminal state instead of a per-tick warning flood: the
/// intents stay queued and become visible through outbox admission receipts.
fn report_outbox_consumer_gone(pending: usize) {
    static REPORTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seen = REPORTED.fetch_add(1, Ordering::Relaxed);
    if seen % 200 != 0 {
        return;
    }
    log::error!(
        "{}",
        t_log!(
            "console.outbox.consumer_gone",
            pending = pending,
            reports = seen + 1
        )
    );
}

/// Consumes one intent batch in order (veto -> map -> send packet by packet).
async fn process_outbox_batch(batch: OrderedOutboxBatch) {
    let OrderedOutboxBatch {
        intents,
        intent_hooks,
        world,
        _byte_permit: byte_permit,
    } = batch;
    // 1) Intent-level veto (game-layer hooks, awaited one by one; vetoed means untranslated).
    let mut kept: Vec<NetworkIntent> = Vec::with_capacity(intents.len());
    for intent in intents {
        if !intent_hooks.is_empty()
            && sc_game::net::dispatch_intent_hooks(&intent_hooks, &intent).await
        {
            continue;
        }
        kept.push(intent);
    }
    if kept.is_empty() {
        return;
    }

    // 2) Map intents -> packets (recipient resolution runs inside the task, fully synchronous).
    let players: Vec<EntityId> = world.entities_with_component::<PlayerConnection>();
    let runtime_routes = build_runtime_routes(&world, &kept);
    let mut sends: Vec<(EntityId, OutPacket)> = Vec::new();

    for intent in kept {
        match intent {
            NetworkIntent::MovePlayer {
                entity_id,
                x,
                y,
                z,
                yaw,
                pitch,
                head_yaw,
                on_ground,
                mode,
            } => {
                let Some(world_id) = runtime_world(&runtime_routes, entity_id) else {
                    continue;
                };
                let packet = MovePlayer {
                    entity_id,
                    x,
                    y,
                    z,
                    pitch,
                    yaw,
                    head_yaw,
                    mode: to_move_mode(mode),
                    on_ground,
                    riding_entity_id: 0,
                    tick: 0,
                };
                match mode {
                    // Correction/teleport packets (RESPAWN/TELEPORT) go to the **player itself**
                    // (revertClientMotion / teleport only target the corrected player).
                    PlayerMoveMode::Reset | PlayerMoveMode::Teleport => {
                        if let Some(player) = runtime_entity(&runtime_routes, entity_id) {
                            sends.push((player, OutPacket::MovePlayer(packet)));
                        }
                    }
                    // Normal: broadcast to same-world viewers (excluding self).
                    PlayerMoveMode::Normal => {
                        for player in players.iter() {
                            let is_self = world
                                .get_component::<MinecraftEntityId>(player)
                                .map(|id| id.0 == entity_id)
                                .unwrap_or(false);
                            if !is_self && same_world(&world, player, &world_id) {
                                sends.push((*player, OutPacket::MovePlayer(packet.clone())));
                            }
                        }
                    }
                }
            }
            NetworkIntent::MoveEntityAbsolute {
                entity_id,
                x,
                y,
                z,
                yaw,
                pitch,
                head_yaw,
                on_ground,
            } => {
                let Some(world_id) = runtime_world(&runtime_routes, entity_id) else {
                    continue;
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((
                        *player,
                        OutPacket::MoveEntityAbsolute(MoveEntityAbsolute {
                            entity_id,
                            x,
                            y,
                            z,
                            pitch,
                            head_yaw,
                            yaw,
                            on_ground,
                            teleport: false,
                            force_move_local_entity: false,
                            force_completion: false,
                        }),
                    ));
                }
            }
            NetworkIntent::SetEntityMotion { entity_id, x, y, z } => {
                let Some(world_id) = runtime_world(&runtime_routes, entity_id) else {
                    continue;
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((
                        *player,
                        OutPacket::SetEntityMotion(SetEntityMotion {
                            entity_id,
                            motion_x: x,
                            motion_y: y,
                            motion_z: z,
                            tick: 0,
                        }),
                    ));
                }
            }
            NetworkIntent::RemoveEntity { entity_id } => {
                let Some(world_id) = runtime_world(&runtime_routes, entity_id) else {
                    continue;
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::RemoveEntity(RemoveEntity { entity_id })));
                }
            }
            NetworkIntent::UpdateBlock {
                world_id,
                dimension,
                incarnation,
                generation,
                x,
                y,
                z,
                runtime_id,
                flags,
                layer,
            } => {
                let key = ChunkKey::new(dimension, ChunkPosition::from_world(x, z));
                for player in players
                    .iter()
                    .filter(|p| same_chunk_world(&world, p, &world_id))
                {
                    let ticket = if let Some(view) = world.get_component::<ChunkView>(player) {
                        let mut data = view.write();
                        if data.world_id != world_id || data.dimension != dimension {
                            continue;
                        }
                        let Some(baseline) = data.delivery_ledger.get(&key).copied() else {
                            // A later baseline must either include this
                            // fact or fail its final generation check.
                            continue;
                        };
                        if baseline.incarnation != incarnation {
                            data.request_refresh(key);
                            continue;
                        }
                        if generation <= baseline.generation {
                            continue;
                        }
                        // §10.3: a delta that skips content versions can only carry
                        // its own block, so an older edit of another position in the
                        // same column stays unapplied until the column is refreshed.
                        // Count it (bounded per connection) instead of assuming the
                        // case never happens; the counter is the trigger evidence a
                        // bounded delta journal would need.
                        if generation > baseline.generation + 1 && data.note_delta_gap(key) {
                            report_delta_gap(key, data.delta_gap_column_count());
                        }
                        let epoch = data.epoch;
                        drop(data);
                        Some(BlockDeliveryTicket {
                            view,
                            world_id: world_id.clone(),
                            key,
                            epoch,
                            incarnation,
                            generation,
                        })
                    } else if same_world(&world, player, &world_id) {
                        None
                    } else {
                        continue;
                    };
                    sends.push((
                        *player,
                        OutPacket::UpdateBlock(
                            UpdateBlock {
                                x,
                                y,
                                z,
                                block_runtime_id: runtime_id,
                                flags,
                                layer,
                            },
                            ticket,
                        ),
                    ));
                }
            }
            NetworkIntent::PlaySound {
                world_id,
                cue,
                data,
                x,
                y,
                z,
            } => {
                let packet = LevelSoundEvent {
                    sound: sound_name(cue).to_owned(),
                    x,
                    y,
                    z,
                    // Place sound data = block runtime id.
                    extra_data: data,
                    // The default actor identifier is ":".
                    entity_type: ":".to_owned(),
                    is_baby_mob: false,
                    is_global: false,
                    entity_unique_id: -1,
                    fire_at_position: None,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::LevelSoundEvent(packet.clone())));
                }
            }
            NetworkIntent::PlayParticle {
                world_id,
                cue,
                data,
                x,
                y,
                z,
            } => {
                let packet = LevelEvent {
                    event_id: particle_event_id(cue),
                    x,
                    y,
                    z,
                    data,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::LevelEvent(packet.clone())));
                }
            }
            NetworkIntent::BlockBreakProgress {
                world_id,
                cue,
                data,
                x,
                y,
                z,
            } => {
                let packet = LevelEvent {
                    event_id: block_break_event_id(cue),
                    x,
                    y,
                    z,
                    data,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::LevelEvent(packet.clone())));
                }
            }
            NetworkIntent::SpawnItemEntity {
                world_id,
                runtime_id,
                x,
                y,
                z,
                motion_x,
                motion_y,
                motion_z,
                stack,
            } => {
                let packet = AddItemEntity {
                    entity_unique_id: runtime_id as i64,
                    entity_runtime_id: runtime_id,
                    item: stack_to_item_data(stack),
                    x,
                    // Network y = entity y +
                    // baseOffset (EntityItem = 0.125, the item actor render origin).
                    y: y + ITEM_ACTOR_BASE_OFFSET,
                    z,
                    motion_x,
                    motion_y,
                    motion_z,
                    metadata: item_actor_metadata(),
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::AddItemEntity(packet.clone())));
                }
            }
            NetworkIntent::MoveItemEntity {
                world_id,
                runtime_id,
                x,
                y,
                z,
                on_ground,
            } => {
                // Ordered existence check (game domain is authoritative): dropped items already
                // removed (picked up / expired / merged / lifetime) are skipped, so the client
                let exists = world
                    .get_resource::<sc_game::item_drop::ItemDropStore>()
                    .map(|store| store.contains(&world_id, runtime_id))
                    .unwrap_or(false);
                if !exists {
                    continue;
                }
                let packet = MoveEntityAbsolute {
                    entity_id: runtime_id,
                    x,
                    // Non-player entities carry baseOffset on network y
                    // (same y as AddItemActor).
                    y: y + ITEM_ACTOR_BASE_OFFSET,
                    z,
                    pitch: 0.0,
                    head_yaw: 0.0,
                    yaw: 0.0,
                    on_ground,
                    teleport: false,
                    force_move_local_entity: false,
                    force_completion: false,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::MoveEntityAbsolute(packet.clone())));
                }
            }
            NetworkIntent::DespawnItemEntity {
                world_id,
                runtime_id,
            } => {
                let packet = RemoveEntity {
                    entity_id: runtime_id,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::RemoveEntity(packet.clone())));
                }
            }
            NetworkIntent::UpdateItemStackSize {
                world_id,
                runtime_id,
                count,
            } => {
                // Item merge: broadcast ActorEvent UPDATE_STACK_SIZE to sync
                // the new stack size of surviving drops (no entity respawn).
                let packet = EntityEvent {
                    entity_runtime_id: runtime_id,
                    event_type: actor_event::UPDATE_STACK_SIZE,
                    data: count as i32,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::EntityEvent(packet.clone())));
                }
            }
            NetworkIntent::TakeItemEntity {
                world_id,
                runtime_id,
                target_entity_id,
            } => {
                let packet = TakeItemEntity {
                    item_entity_id: runtime_id,
                    target_entity_id,
                };
                for player in players.iter().filter(|p| same_world(&world, p, &world_id)) {
                    sends.push((*player, OutPacket::TakeItemEntity(packet.clone())));
                }
            }
            NetworkIntent::UpdateInventorySlot {
                entity_id,
                slot,
                stack,
            } => {
                // Per-slot InventorySlotPacket, sent only to the backpack owner
                // (picked-up items show up immediately).
                let Some(player) = runtime_entity(&runtime_routes, entity_id) else {
                    continue;
                };
                sends.push((
                    player,
                    OutPacket::InventorySlot(InventorySlot {
                        full_container_name_id: None,
                        full_container_dynamic_id: None,
                        container_id: InventoryContent::SPECIAL_INVENTORY,
                        slot,
                        item: stack_to_item_data(stack),
                    }),
                ));
            }
            NetworkIntent::ResyncInventory { entity_id } => {
                // Authoritative resync: resolve the player entity from the runtime id, read its
                // PlayerInventory -> encode a 36-slot InventoryContent back to itself (rolls back client prediction).
                let Some(player) = runtime_entity(&runtime_routes, entity_id) else {
                    continue;
                };
                let Some(inventory) = world.get_component::<sc_item::PlayerInventory>(&player)
                else {
                    continue;
                };
                sends.push((player, full_inventory_content(&inventory)));
            }
            NetworkIntent::OpenPlayerInventory { entity_id } => {
                let Some(player) = runtime_entity(&runtime_routes, entity_id) else {
                    continue;
                };
                let Some(transform) = world.get_component::<sc_entity::motion::Transform>(&player)
                else {
                    continue;
                };
                let position = transform.read().position;
                sends.push((
                    player,
                    OutPacket::ContainerOpen(ContainerOpen::player_inventory(
                        position.x.floor() as i32,
                        position.y.floor() as i32,
                        position.z.floor() as i32,
                        entity_id,
                    )),
                ));
                let Some(inventory) = world.get_component::<sc_item::PlayerInventory>(&player)
                else {
                    continue;
                };
                sends.push((player, full_inventory_content(&inventory)));
            }
            NetworkIntent::ClosePlayerInventory {
                entity_id,
                window_id,
                container_type,
            } => {
                let Some(player) = runtime_entity(&runtime_routes, entity_id) else {
                    continue;
                };
                sends.push((
                    player,
                    OutPacket::ContainerClose(ContainerClose {
                        window_id,
                        container_type,
                        // A client-confirmation close packet carries serverInitiated=false.
                        // serverInitiated=false.
                        was_server_initiated: false,
                    }),
                ));
            }
            NetworkIntent::OpenCraftingStation {
                entity_id,
                station,
                window_id,
                x,
                y,
                z,
            } => {
                let container_type = match station {
                    sc_recipe::StationKind::CraftingTable => 1,
                    sc_recipe::StationKind::Stonecutter => 29,
                    _ => continue,
                };
                if let Some(player) = runtime_entity(&runtime_routes, entity_id) {
                    sends.push((
                        player,
                        OutPacket::ContainerOpen(ContainerOpen {
                            window_id,
                            container_type,
                            x,
                            y,
                            z,
                            // ContainerOpenPacket defaults targetActorID to -1;
                            // a block holder (e.g. workbench) leaves it unset, so -1 is sent as-is.
                            // Only the player-backpack packet carries the real runtime id.
                            entity_id: -1,
                        }),
                    ));
                    // Opening a crafting table sends containerId=window id with the 9 current
                    // grid slots (see the FCN note on `workbench_grid_content`). This path
                    // does not resend main inventory/armor/offhand.
                    if station == sc_recipe::StationKind::CraftingTable {
                        let slots: Vec<ItemData> =
                            sc_game::craft_inventory::grid_snapshot(&world, player)
                                .into_iter()
                                .map(|(_, stack)| stack_to_item_data(stack))
                                .collect();
                        sends.push((player, workbench_grid_content(window_id, slots)));
                    }
                }
            }
            NetworkIntent::CraftResponse {
                slots,
                entity_id,
                request_id,
                success,
            } => {
                if let Some(player) = runtime_entity(&runtime_routes, entity_id) {
                    sends.push((
                        player,
                        OutPacket::ItemStackResponse(
                            crate::protocol::server::crafting_response::ItemStackResponse {
                                slots: slots.clone(),
                                request_id,
                                success,
                            },
                        ),
                    ));
                    for slot in slots {
                        if success {
                            continue;
                        }
                        use sc_game::craft_inventory::CraftContainer as C;
                        let (container_id, full_container_name_id) = match slot.container {
                            C::Inventory | C::CombinedInventory | C::Hotbar => (0, 29),
                            C::Grid => (0x7c, 13),
                            C::Cursor => (0x7c, 59),
                            C::Output => (0x7c, 60),
                        };
                        let mut item = stack_to_item_data(slot.stack);
                        item.has_net_id = slot.net_id != 0;
                        item.net_id = slot.net_id;
                        sends.push((
                            player,
                            OutPacket::InventorySlot(InventorySlot {
                                container_id,
                                slot: slot.slot,
                                item,
                                full_container_name_id: Some(full_container_name_id),
                                full_container_dynamic_id: slot.dynamic,
                            }),
                        ));
                    }
                }
            }
            NetworkIntent::CorrectBlockPrediction {
                entity_id,
                x,
                y,
                z,
                runtime_id,
                flags,
                layer,
                ..
            } => {
                // Only the predicting player itself: other viewers always see authoritative
                // state and need no rollback. Deliberately ticketless (a revert is current
                if let Some(player) = runtime_entity(&runtime_routes, entity_id) {
                    sends.push((
                        player,
                        OutPacket::UpdateBlock(
                            UpdateBlock {
                                x,
                                y,
                                z,
                                block_runtime_id: runtime_id,
                                flags,
                                layer,
                            },
                            None,
                        ),
                    ));
                }
            }
        }
    }

    if sends.is_empty() {
        return;
    }
    // 3) Send packets (packet-level hooks run inside send_packet).
    //
    // One failed connection only terminates that connection (admit_prepared already spent
    // the encryption counter and scheduled the close); never `break` the whole batch, or one
    let mut failed_targets = 0usize;
    for (target, packet) in sends {
        let Some(connection) = world.get_component::<PlayerConnection>(&target) else {
            continue;
        };
        let result = match packet {
            OutPacket::ItemStackResponse(p) => connection.send_packet(p, true).await,
            OutPacket::MovePlayer(p) => connection.send_packet(p, true).await,
            OutPacket::MoveEntityAbsolute(p) => connection.send_packet(p, true).await,
            OutPacket::SetEntityMotion(p) => connection.send_packet(p, true).await,
            OutPacket::RemoveEntity(p) => connection.send_packet(p, true).await,
            OutPacket::AddItemEntity(p) => connection.send_packet(p, true).await,
            OutPacket::TakeItemEntity(p) => connection.send_packet(p, true).await,
            OutPacket::EntityEvent(p) => connection.send_packet(p, true).await,
            OutPacket::LevelEvent(p) => connection.send_packet(p, true).await,
            OutPacket::LevelSoundEvent(p) => connection.send_packet(p, true).await,
            OutPacket::UpdateBlock(p, Some(ticket)) => connection
                .send_packet_with_checked_outcome(p, true, |plain, permit, trace| {
                    let mut data = ticket.view.write();
                    if data.epoch != ticket.epoch
                        || data.world_id != ticket.world_id
                        || data.dimension != ticket.key.dimension
                    {
                        return Ok(PacketSendOutcome::StaleContext);
                    }
                    let Some(baseline) = data.delivery_ledger.get(&ticket.key).copied() else {
                        return Ok(PacketSendOutcome::StaleContent);
                    };
                    if baseline.incarnation != ticket.incarnation {
                        data.request_refresh(ticket.key);
                        return Ok(PacketSendOutcome::StaleContent);
                    }
                    if ticket.generation <= baseline.generation {
                        return Ok(PacketSendOutcome::StaleContent);
                    }
                    connection.admit_prepared(plain, true, permit, trace)?;
                    Ok(PacketSendOutcome::Queued)
                })
                .await
                .map(|_| ()),
            OutPacket::UpdateBlock(p, None) => connection.send_packet(p, true).await,
            OutPacket::ContainerOpen(p) => connection.send_packet(p, true).await,
            OutPacket::ContainerClose(p) => connection.send_packet(p, true).await,
            OutPacket::InventoryContent(p) => connection.send_packet(p, true).await,
            OutPacket::InventorySlot(p) => connection.send_packet(p, true).await,
        };
        if let Err(error) = result {
            failed_targets += 1;
            log::debug!("[outbox] target {target} rejected packet: {error}");
        }
    }
    if failed_targets > 0 {
        log::debug!("[outbox] {failed_targets} target(s) did not accept this batch");
    }
    drop(byte_permit);
}
/// Maps an intent move mode to the protocol move mode.
fn to_move_mode(mode: PlayerMoveMode) -> MovePlayerMode {
    match mode {
        PlayerMoveMode::Normal => MovePlayerMode::Normal,
        PlayerMoveMode::Reset => MovePlayerMode::Reset,
        PlayerMoveMode::Teleport => MovePlayerMode::Teleport,
    }
}

#[derive(Clone)]
struct RuntimeEntityRoute {
    entity: EntityId,
    world_id: Option<MinecraftWorldId>,
}

/// Snapshot only ECS routes referenced by this outbox batch. Query the component
/// index once, rather than allocating/scanning it for every target intent.
fn build_runtime_routes(
    world: &World,
    intents: &[NetworkIntent],
) -> HashMap<u64, RuntimeEntityRoute> {
    let mut requested = HashSet::new();
    for intent in intents {
        if let Some(runtime_id) = runtime_route_id(intent) {
            requested.insert(runtime_id);
        }
    }
    if requested.is_empty() {
        return HashMap::new();
    }

    let entities = world.entities_with_component::<MinecraftEntityId>();
    let mut routes = HashMap::with_capacity(requested.len());
    for entity in entities {
        let Some(runtime_id) = world.get_component::<MinecraftEntityId>(&entity) else {
            continue;
        };
        if !requested.contains(&runtime_id.0) {
            continue;
        }
        let route = RuntimeEntityRoute {
            entity,
            world_id: world
                .get_component::<MinecraftWorldId>(&entity)
                .map(|world_id| (*world_id).clone()),
        };
        // Preserve the existing lookup contract if a malformed world contains
        // duplicate runtime IDs: the first entity in the ECS query wins.
        routes.entry(runtime_id.0).or_insert(route);
        if routes.len() == requested.len() {
            break;
        }
    }
    routes
}

fn runtime_route_id(intent: &NetworkIntent) -> Option<u64> {
    match intent {
        NetworkIntent::MovePlayer { entity_id, .. }
        | NetworkIntent::MoveEntityAbsolute { entity_id, .. }
        | NetworkIntent::SetEntityMotion { entity_id, .. }
        | NetworkIntent::RemoveEntity { entity_id }
        | NetworkIntent::UpdateInventorySlot { entity_id, .. }
        | NetworkIntent::ResyncInventory { entity_id }
        | NetworkIntent::CraftResponse { entity_id, .. }
        | NetworkIntent::CorrectBlockPrediction { entity_id, .. }
        | NetworkIntent::OpenCraftingStation { entity_id, .. }
        | NetworkIntent::OpenPlayerInventory { entity_id }
        | NetworkIntent::ClosePlayerInventory { entity_id, .. } => Some(*entity_id),
        _ => None,
    }
}

fn runtime_entity(routes: &HashMap<u64, RuntimeEntityRoute>, runtime_id: u64) -> Option<EntityId> {
    routes.get(&runtime_id).map(|route| route.entity)
}

fn runtime_world(
    routes: &HashMap<u64, RuntimeEntityRoute>,
    runtime_id: u64,
) -> Option<MinecraftWorldId> {
    routes.get(&runtime_id)?.world_id.clone()
}

/// Server-side item stack -> network item descriptor.
fn stack_to_item_data(stack: sc_item::ItemStack) -> ItemData {
    ItemData {
        runtime_id: stack.runtime_id,
        count: stack.count,
        damage: stack.damage,
        // Empty slots normalize through the writer as `ItemData.AIR` (runtime 0 / no net id /
        // block 0); non-empty items use to-network semantics (usingNetId always true, see writer).
        // (`Item.toNetwork()` semantics: usingNetId always true, see writer).
        has_net_id: false,
        net_id: 0,
        block_runtime_id: stack.block_runtime_id,
        user_data: None,
    }
}

/// The authoritative full backpack = **one** 36-slot `InventoryContent` (containerId 0,
/// FullContainerName from `InventoryContent::FULL_CONTAINER_INVENTORY`).
///
/// Armor/offhand containers (ARMOR=120 / OFFHAND=119) are deliberately **excluded**:
/// the server has no player armor or offhand storage yet, so fixed 4+1 air slots would wipe
/// the equipped helmet/chest/legs/boots and shield to air on every rollback. Only actual
/// equipment changes send those containers; without that fact, they are not sent.
fn full_inventory_content(inventory: &sc_item::PlayerInventory) -> OutPacket {
    let items: Vec<ItemData> = inventory
        .snapshot()
        .into_iter()
        .map(stack_to_item_data)
        .collect();
    OutPacket::InventoryContent(InventoryContent::new(0, normalize_slots(items, 36)))
}

/// The 9 grid slots of an open workbench: containerId=window id with the 9 current slots;
/// FullContainerName uses `CRAFTING_INPUT(13) + dynamic=window id`
/// (matching the `ItemStackResponse` Grid container).
/// The protocol-library default `(ANVIL_INPUT=0, None)` was used here by mistake: the client
/// would route the content to an anvil instead of the crafting grid, leaving the grid empty.
/// Shares `normalize_slots` semantics with the backpack (pad air when short, truncate when long); only the slot count differs.
fn workbench_grid_content(window_id: u8, slots: Vec<ItemData>) -> OutPacket {
    OutPacket::InventoryContent(
        InventoryContent::new(window_id, normalize_slots(slots, 9))
            .with_full_container_name(InventoryContent::FULL_CONTAINER_CRAFTING_INPUT)
            .with_full_container_dynamic_id(Some(window_id as u32)),
    )
}

fn normalize_slots(mut items: Vec<ItemData>, slot_count: usize) -> Vec<ItemData> {
    items.truncate(slot_count);
    items.resize(slot_count, ItemData::default());
    items
}

/// Item actor network-y offset.
const ITEM_ACTOR_BASE_OFFSET: f32 = 0.125;

/// Standard item-actor metadata (default entity data entries:
/// 0.25/0.25 size, Health 5, GRAVITY flag).
///
/// Empty metadata leaves the client entity state incomplete (no pickup animation/sound).
fn item_actor_metadata() -> Vec<EntityMetadataEntry> {
    use crate::protocol::server::entity_metadata::{
        EntityFlags as F, EntityKeys as K, EntityMetadataExt as _,
    };
    let mut metadata: Vec<EntityMetadataEntry> = Vec::new();
    metadata.set_flag(F::GRAVITY, true);
    metadata.set_int(K::HEALTH, 5);
    metadata.set_string(K::NAMETAG, String::new());
    metadata.set_short(K::AIR, 400);
    metadata.set_long(K::LEAD_HOLDER_EID, -1);
    metadata.set_float(K::SCALE, 1.0);
    metadata.set_short(K::MAX_AIR, 400);
    metadata.set_float(K::BOUNDING_BOX_WIDTH, 0.25);
    metadata.set_float(K::BOUNDING_BOX_HEIGHT, 0.25);
    metadata
}

fn sound_name(cue: SoundCue) -> &'static str {
    match cue {
        SoundCue::BlockBreak => "break",
        SoundCue::BlockPlace => "place",
        SoundCue::ItemPickup => "pickup",
    }
}

fn particle_event_id(cue: ParticleCue) -> u32 {
    match cue {
        ParticleCue::BlockBreak => 2_001,
    }
}

fn block_break_event_id(cue: BlockBreakProgressCue) -> u32 {
    match cue {
        BlockBreakProgressCue::Start => 3_600,
        BlockBreakProgressCue::Stop => 3_601,
        BlockBreakProgressCue::Update => 3_602,
    }
}

/// Whether the player is in the target world and InGame.
fn same_world(world: &World, player: &EntityId, world_id: &MinecraftWorldId) -> bool {
    let Some(connection) = world.get_component::<PlayerConnection>(player) else {
        return false;
    };
    if !matches!(
        connection.get_status(),
        PlayerConnectionStatus::InGame | PlayerConnectionStatus::Spawned,
    ) {
        return false;
    }
    world
        .get_component::<MinecraftWorldId>(player)
        .map(|id| id.as_ref() == world_id)
        .unwrap_or(false)
}

fn same_chunk_world(world: &World, player: &EntityId, world_id: &MinecraftWorldId) -> bool {
    world
        .get_component::<PlayerConnection>(player)
        .is_some_and(|connection| connection.get_status().can_send_chunks())
        && world
            .get_component::<MinecraftWorldId>(player)
            .is_some_and(|id| id.as_ref() == world_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_byte_permit_is_bounded_and_released_on_drop() {
        let retained = Arc::new(AtomicUsize::new(0));
        let first = try_reserve_outbox_bytes(&retained, 60, 100).expect("first reservation");
        assert_eq!(retained.load(Ordering::Acquire), 60);
        assert!(try_reserve_outbox_bytes(&retained, 41, 100).is_none());
        assert_eq!(retained.load(Ordering::Acquire), 60);
        let second = try_reserve_outbox_bytes(&retained, 40, 100).expect("remaining budget");
        assert_eq!(retained.load(Ordering::Acquire), 100);
        drop(first);
        assert_eq!(retained.load(Ordering::Acquire), 40);
        drop(second);
        assert_eq!(retained.load(Ordering::Acquire), 0);
    }

    #[test]
    fn concurrent_outbox_byte_reservations_do_not_exceed_limit() {
        let retained = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::new();
        for _ in 0..8 {
            let retained = Arc::clone(&retained);
            workers.push(std::thread::spawn(move || {
                (0..32)
                    .filter_map(|_| try_reserve_outbox_bytes(&retained, 32, 1024))
                    .collect::<Vec<_>>()
            }));
        }
        let permits = workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("reservation worker"))
            .collect::<Vec<_>>();
        assert!(retained.load(Ordering::Acquire) <= 1024);
        drop(permits);
        assert_eq!(retained.load(Ordering::Acquire), 0);
    }

    #[test]
    fn runtime_route_snapshot_resolves_only_requested_ids_and_keeps_first_duplicate() {
        let world = World::new();
        world.spawn((MinecraftEntityId(42), MinecraftWorldId::random()));
        world.spawn((MinecraftEntityId(42), MinecraftWorldId::random()));
        world.spawn(MinecraftEntityId(99));
        let expected_first = world
            .entities_with_component::<MinecraftEntityId>()
            .into_iter()
            .find(|entity| {
                world
                    .get_component::<MinecraftEntityId>(entity)
                    .is_some_and(|id| id.0 == 42)
            })
            .expect("first matching entity from ECS query");
        let routes =
            build_runtime_routes(&world, &[NetworkIntent::ResyncInventory { entity_id: 42 }]);

        let route = routes.get(&42).expect("runtime id route");
        assert_eq!(route.entity, expected_first);
        assert!(route.world_id.is_some());
        assert!(
            !routes.contains_key(&99),
            "unrequested runtime IDs stay out of the snapshot"
        );
        assert!(runtime_entity(&routes, 100).is_none());
        assert!(runtime_world(&routes, 42).is_some());
    }

    /// The authoritative full backpack must be **one** 36-slot `InventoryContent`.
    ///
    /// Armor/offhand containers used to be fixed 4+1 air slots, wiping the equipped
    /// helmet/chest/legs/boots and shield to air on every rollback; those three packets
    /// also landed in the same flush as `ContainerOpen` (opening a station only sends
    #[test]
    fn full_inventory_content_is_a_single_36_slot_packet_without_armor_or_offhand() {
        let mut inventory = sc_item::PlayerInventory::new(36);
        inventory.set(0, sc_item::ItemStack::new(7, 3));
        let packet = full_inventory_content(&inventory);
        let OutPacket::InventoryContent(content) = packet else {
            panic!("resync must be a single InventoryContent");
        };
        assert_eq!(content.container_id, 0);
        assert_eq!(content.items.len(), 36);
        assert_eq!(content.full_container_name_id, 28);
        assert_eq!(content.items[0].runtime_id, 7);
        assert_eq!(content.items[0].count, 3);
    }

    /// Workbench grid content: containerId=window id, 9 slots, FCN=`CRAFTING_INPUT(13)` +
    /// dynamic=window id (matching the `ItemStackResponse` Grid container).
    #[test]
    fn workbench_grid_content_uses_window_id_nine_slots_and_default_name() {
        let slots = vec![ItemData::default(); 9];
        let packet = workbench_grid_content(5, slots);
        let OutPacket::InventoryContent(content) = packet else {
            panic!("grid must be a single InventoryContent");
        };
        assert_eq!(content.container_id, 5);
        assert_eq!(content.items.len(), 9);
        assert_eq!(
            content.full_container_name_id,
            InventoryContent::FULL_CONTAINER_CRAFTING_INPUT
        );
        assert_eq!(content.full_container_dynamic_id, Some(5));
    }

    #[test]
    fn block_break_progress_uses_level_event_ids() {
        assert_eq!(block_break_event_id(BlockBreakProgressCue::Start), 3_600);
        assert_eq!(block_break_event_id(BlockBreakProgressCue::Stop), 3_601);
        assert_eq!(block_break_event_id(BlockBreakProgressCue::Update), 3_602);
    }
}
