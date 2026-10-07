//! Player interaction (place/break) boundary events and game-domain consumption.
//!
//! Event flow (`docs/block_system_design.md` §5):
//! Network decoding goes to per-player bounded ordered [`MiningInbox`] input / place boundary events,
//! then plugin behavior systems (SCPluginEventUpdate, may Deny/Handle),
//! then this module's kernel default (SCEventUpdate): validate, then [`BlockChangeQueue`] enqueue (branch A),
//! then PostUpdate apply lands chunks, then BlockChanged, then broadcast (branch B).
//!
//! Full implementation: face offset math, item-to-block mapping (ItemRegistry), inventory drain (PlayerInventory), 
//! distance checks (Transform to target-block distance), two-phase breaks (START_BREAK records, STOP_BREAK commits).
//!
//! Break duration is **block-data driven**: target state goes to `BlockStateId`, then the block JSON snapshot's
//! fixed dense columns (`minecraft:destructible_by_mining` seconds x 20 rounded up;
//! `sc:unbreakable` denies survival; undeclared states use [`FALLBACK_BREAK_TIME_TICKS`]).
//! Survival mode lands breaks on authoritative server timing ([`advance_block_break`]); client
//! early STOP_BREAK commits break nothing; creative mode ignores hardness.

use log::{debug, info, warn};
use sc_block::block_json::BlockJsonRegistry;
use sc_block::position::BlockPosition;
use sc_block::registry::BlockStateRegistry;
use sc_block::write::{
    update_flags, BlockBreakContext, BlockChange, BlockChangeCause, BlockChangeQueue,
    BlockChangeResult, BlockExpectation,
};
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::world::World;
use sc_entity::motion::{PhysicsBody, Transform};
use sc_entity::MinecraftEntityId;
use sc_item::{ItemRegistry, ItemStack, PlayerInventory};
use sc_utils::game::client::MinecraftClient;
use sc_world::chunk::BlockRuntimeId;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::ChunkKey;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use crate::container::ContainerOpen;
use crate::net::{NetworkIntent, NetworkOutbox};
use crate::net_backpressure::{IntentPublisher, PendingInventoryResync};
use crate::net_faults::PendingConnectionFaults;

/// Seconds-to-ticks conversion base (Bedrock runs 20 ticks/second).
const TICKS_PER_SECOND: f32 = 20.0;

/// **Explicit** fallback duration (ticks) for states without `minecraft:destructible_by_mining`.
///
/// Not a guess: the constant is named explicitly and startup logs the undeclared-state count (see
/// `sc_bootstrap::load_block_bundle`); the fallback vanishes once authoritative data arrives.
pub const FALLBACK_BREAK_TIME_TICKS: i32 = 20;

/// `BlockBreakProgress` progress denominator (protocol-fixed 0..=65535).
const BREAK_PROGRESS_MAX: i32 = 65_535;

/// Tolerance between client prediction and authoritative server timing (ticks; 5 ticks = 250ms).
///
/// Measured (2026-10-07 live logs): clients always submit 1-3 ticks earlier than the server
/// (`need 60 ticks, have 58 ticks` / `need 350 ticks, have 347 ticks`) because server-side
/// `started_at` starts on the tick the START packet arrives, later than the client press by
/// network half-trip plus tick quantization. In-tolerance completions land directly (no rollback);
/// out-of-tolerance ones still roll back as cheating-level early submits and keep timing.
pub const BREAK_EARLY_TOLERANCE_TICKS: i32 = 5;

/// Max break/place interaction distance (blocks); beyond it inputs are ignored (anti-cheat).
const MAX_INTERACT_DISTANCE_SQ: f32 = 36.0; // 6² = 36

/// Face to offset vector (0=down, 1=up, 2=north, 3=south, 4=west, 5=east).
/// Face ordinals: DOWN=0, UP=1, NORTH=2, SOUTH=3, WEST=4, EAST=5.
const FACE_OFFSETS: [(i32, i32, i32); 6] = [
    (0, -1, 0), // 0: DOWN
    (0, 1, 0),  // 1: UP
    (0, 0, -1), // 2: NORTH
    (0, 0, 1),  // 3: SOUTH
    (-1, 0, 0), // 4: WEST
    (1, 0, 0),  // 5: EAST
];

/// Offset by face id; out-of-range faces default to up.
fn face_offset(face: u8) -> (i32, i32, i32) {
    FACE_OFFSETS
        .get(face as usize)
        .copied()
        .unwrap_or((0, 1, 0))
}

// Boundary events.

/// Start-break request (sent after the network layer decodes `PlayerAction START_BREAK`).
///
/// Survival two-phase break, phase one: record [`BreakingState`] only, break nothing.
#[derive(Event, Clone, Debug)]
pub struct StartBreakRequest {
    pub entity: EntityId,
    pub position: BlockPosition,
    pub face: u8,
}

/// Abort-break request (sent after the network layer decodes `PlayerAction ABORT_BREAK`).
///
/// Clear the matching [`BreakingState`]; `None` is only for targetless legacy-plugin cancels.
#[derive(Event, Clone, Debug)]
pub struct AbortBreakRequest {
    pub entity: EntityId,
    pub position: Option<BlockPosition>,
}

#[derive(Clone, Debug)]
pub enum MiningAction {
    Start {
        position: BlockPosition,
        face: u8,
    },
    Abort {
        position: BlockPosition,
    },
    Break {
        position: BlockPosition,
        face: u8,
        require_breaking_state: bool,
    },
    HeldSlot {
        slot: u8,
    },
}

#[derive(Clone, Debug)]
pub struct OrderedMiningAction {
    pub world_id: MinecraftWorldId,
    pub context_epoch: Option<u64>,
    pub action: MiningAction,
}

#[derive(Component, Default)]
pub struct MiningInbox {
    pending: Mutex<VecDeque<OrderedMiningAction>>,
}

impl MiningInbox {
    pub const MAX_ACTIONS: usize = 128;

    pub fn try_push(&self, action: OrderedMiningAction) -> bool {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.len() >= Self::MAX_ACTIONS {
            return false;
        }
        pending.push_back(action);
        true
    }

    fn take(&self) -> VecDeque<OrderedMiningAction> {
        std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }
}

pub fn mining_context_epoch(world: &World, entity: &EntityId) -> Option<u64> {
    world
        .get_component::<sc_world::chunk_view::ChunkView>(entity)
        .map(|view| view.read().epoch)
}

pub fn drain_mining_actions(
    world: World,
    mut queue: ResMut<BlockChangeQueue>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    for entity in world.entities_with_component::<MiningInbox>() {
        let Some(inbox) = world.get_component::<MiningInbox>(&entity) else {
            continue;
        };
        for request in inbox.take() {
            let current_world = world.get_component::<MinecraftWorldId>(&entity);
            if current_world.as_deref() != Some(&request.world_id)
                || mining_context_epoch(&world, &entity) != request.context_epoch
            {
                continue;
            }
            match request.action {
                MiningAction::Start { position, face } => start_break(
                    &world,
                    &StartBreakRequest {
                        entity,
                        position,
                        face,
                    },
                    &mut queue,
                    &mut outbox,
                ),
                MiningAction::Abort { position } => abort_break(
                    &world,
                    &AbortBreakRequest {
                        entity,
                        position: Some(position),
                    },
                    &mut queue,
                    &mut outbox,
                ),
                MiningAction::Break {
                    position,
                    face,
                    require_breaking_state,
                } => break_block(
                    &world,
                    &BreakBlockRequest {
                        entity,
                        position,
                        face,
                        require_breaking_state,
                    },
                    &mut queue,
                    &mut outbox,
                ),
                MiningAction::HeldSlot { slot } => {
                    if slot <= 8 {
                        if let Some(inventory) = world.get_component::<PlayerInventory>(&entity) {
                            if inventory.selected_slot() != slot
                                && inventory.set_selected_slot(slot)
                            {
                                cancel_break(&world, &entity, &mut outbox);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Commit-break request (sent after the network layer decodes `PlayerAction STOP_BREAK` or
/// `CREATIVE_PLAYER_DESTROY_BLOCK`).
///
/// With `require_breaking_state=true` (survival STOP_BREAK), the [`BreakingState`]
/// position must match; with `false` (creative instant destroy), skip validation and break.
#[derive(Event, Clone, Debug)]
pub struct BreakBlockRequest {
    pub entity: EntityId,
    pub position: BlockPosition,
    pub face: u8,
    /// Whether BreakingState must match (survival STOP_BREAK=true, creative destroy=false).
    pub require_breaking_state: bool,
}

/// Place-block request (sent after the network layer decodes `InventoryTransaction USE_ITEM`).
#[derive(Event, Clone, Debug)]
pub struct PlaceBlockRequest {
    pub entity: EntityId,
    /// Clicked block coordinates (place target = these plus the face offset).
    pub position: BlockPosition,
    pub face: u8,
    pub hotbar_slot: i32,
    /// Held item runtime id (0 = air).
    pub item_id: i32,
}

/// Hotbar slot switch request (sent after the network layer decodes `MobEquipment`).
#[derive(Event, Clone, Debug)]
pub struct HeldSlotChanged {
    pub entity: EntityId,
    /// New hotbar slot (0..8).
    pub slot: u8,
}

#[derive(Event, Clone, Debug)]
pub struct OpenInventoryRequest {
    pub entity: EntityId,
    pub target_entity_id: u64,
}

#[derive(Event, Clone, Debug)]
pub struct CloseInventoryRequest {
    pub entity: EntityId,
    pub window_id: u8,
    pub container_type: i8,
    pub was_server_initiated: bool,
}

// Components.

/// Inventory reservations waiting for the authoritative block write.
#[derive(sc_ecs::resource::Resource, Default)]
pub struct BlockPlacementReservations {
    pending: HashMap<u64, InventoryReservation>,
}

struct InventoryReservation {
    entity: EntityId,
    inventory: PlayerInventory,
    slot: usize,
    item: ItemStack,
}

/// Block state a player is breaking (component on the player entity).
///
/// Survival: START_BREAK starts authoritative server timing (durations from the block JSON dense columns,
/// see [`MiningProfile`]); each tick [`advance_block_break`] advances it, and expiry
/// lands the break. Client STOP_BREAK only commits once timing completes; early commits are rejected.
/// Creative: CREATIVE_PLAYER_DESTROY_BLOCK commits directly (no BreakingState,
/// no mining-time check, matching creative bedrock breaking).
#[derive(Component, Clone, Debug, Default)]
pub struct BreakingState {
    /// Position under breaking (None = not breaking).
    pub position: Option<BlockPosition>,
    /// Face at break start (for logs/debugging).
    pub face: u8,
    /// Target block at break start (rechecked on commit/expiry so changed targets never break).
    pub target: Option<BlockRuntimeId>,
    /// Ticks to break this state (from the block JSON mining-seconds column).
    pub required_ticks: i32,
    /// Ticks converted from monotonic elapsed time, diagnostics only.
    pub elapsed_ticks: i32,
    pub world_id: Option<MinecraftWorldId>,
    pub context_epoch: Option<u64>,
    pub incarnation: u128,
    pub hand: sc_block::mining_drops::HandSnapshot,
    pub creative: bool,
    /// Monotonic time when the owner accepted the break start; client clocks cannot advance it.
    pub started_at: Option<Instant>,
}

impl BreakingState {
    fn elapsed_at(&self, now: Instant) -> i32 {
        self.started_at
            .map(|start| {
                (now.saturating_duration_since(start).as_millis() / 50).min(i32::MAX as u128) as i32
            })
            .unwrap_or(0)
    }

    fn complete_at(&self, now: Instant) -> bool {
        self.elapsed_at(now) >= self.required_ticks.max(1)
    }
}

// Break durations (block JSON data-driven).

/// Mining decision for one state (from fixed block JSON dense columns, not hardcoded tables).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningProfile {
    /// Ticks to mine (`minecraft:destructible_by_mining` seconds x 20 rounded up,
    /// or `sc:mining` `base/(tool*enchant)` conversion).
    pub required_ticks: i32,
    /// `sc:unbreakable`: unbreakable in survival.
    pub unbreakable: bool,
    /// Whether the duration comes from declared data (false = undeclared, uses the explicit fallback).
    pub declared: bool,
    /// `sc:mining` `can_mine` (`false` = this tool is refused in survival; creative bypasses).
    pub can_mine: bool,
}

impl MiningProfile {
    /// Explicit fallback for undeclared mining seconds (constant and countable).
    pub const fn fallback() -> Self {
        Self {
            required_ticks: FALLBACK_BREAK_TIME_TICKS,
            unbreakable: false,
            declared: false,
            can_mine: true,
        }
    }

    /// Unbreakable (`sc:unbreakable`).
    pub const fn unbreakable() -> Self {
        Self {
            required_ticks: i32::MAX,
            unbreakable: true,
            declared: true,
            can_mine: true,
        }
    }

    /// Tool cannot mine (`sc:mining` `can_mine=false`; refused in survival, creative bypasses).
    pub const fn denied() -> Self {
        Self {
            required_ticks: i32::MAX,
            unbreakable: false,
            declared: true,
            can_mine: false,
        }
    }

    /// Seconds to ticks (rounded up, at least 1 tick; non-finite/negative counts as undeclared).
    pub fn from_seconds(seconds: f32) -> Self {
        if !seconds.is_finite() || seconds < 0.0 {
            return Self::fallback();
        }
        let ticks = (seconds * TICKS_PER_SECOND).ceil() as i32;
        Self {
            required_ticks: ticks.max(1),
            unbreakable: false,
            declared: true,
            can_mine: true,
        }
    }
}

/// Break-progress packet `event_data` value: **per-tick increments** (not absolute progress).
///
/// `BLOCK_START_BREAK` speed parameter is `65535 / breakTick`.
/// Stock blocks show client-predicted cracks; no per-tick UPDATE_BREAK here.
///
/// Steps are at least 1 (even very long breaks must visibly advance); illegal tick counts degrade to full,
/// never dividing by zero or going negative.
pub fn break_progress_step(required_ticks: i32) -> i32 {
    if required_ticks <= 0 {
        return BREAK_PROGRESS_MAX;
    }
    (BREAK_PROGRESS_MAX / required_ticks).max(1)
}

/// Target-block mining decision: `runtime_id` goes to `BlockStateId`, then block JSON dense columns.
///
/// Missing snapshot/registry (old packs without declared block_data) or unregistered states fall back
/// to the explicit [`MiningProfile::fallback`]: never silently guessing values, with startup logging the scale.
///
/// Legacy format compatibility: without `sc:mining`, keep `minecraft:destructible_by_mining` (or fall back).
pub fn mining_profile(world: &World, runtime_id: BlockRuntimeId) -> MiningProfile {
    mining_profile_with_hand(
        world,
        runtime_id,
        &sc_block::mining_drops::HandSnapshot::default(),
    )
}

/// Hand snapshot (by-value item stack copy from the break request; holds no inventory lock).
///
/// - Empty hands always take the `default` rule, matching no `tools[].items`;
/// - Enchant levels are a limited first implementation: the protocol has no enchant table yet,
///   so live paths use `0` (no bonus, no error); tests inject levels through pure functions.
///   Undeclared enchants do nothing; unknown enchant keys are rejected at load and never run.
pub fn hand_snapshot(world: &World, entity: &EntityId) -> sc_block::mining_drops::HandSnapshot {
    let Some(inventory) = world.get_component::<PlayerInventory>(entity) else {
        return sc_block::mining_drops::HandSnapshot::default();
    };
    let slot = inventory.selected_slot() as usize;
    let Some(stack) = inventory.get(slot) else {
        return sc_block::mining_drops::HandSnapshot::default();
    };
    if stack.is_empty() {
        return sc_block::mining_drops::HandSnapshot::default();
    }
    let item = world
        .get_resource::<ItemRegistry>()
        .and_then(|registry| registry.get(stack.runtime_id))
        .map(|def| def.name.to_string());
    sc_block::mining_drops::HandSnapshot {
        item,
        efficiency_level: 0,
        fortune_level: 0,
    }
}

/// Tool/enchant-aware mining decision (tool multipliers and efficiency when `sc:mining` exists).
pub fn mining_profile_with_hand(
    world: &World,
    runtime_id: BlockRuntimeId,
    hand: &sc_block::mining_drops::HandSnapshot,
) -> MiningProfile {
    let Some(snapshot) = world
        .get_resource::<BlockJsonRegistry>()
        .and_then(|registry| registry.get())
    else {
        return MiningProfile::fallback();
    };
    let Some(state_id) = world
        .get_resource::<BlockStateRegistry>()
        .and_then(|registry| registry.by_runtime_id(runtime_id))
    else {
        return MiningProfile::fallback();
    };
    if snapshot.is_unbreakable(state_id.0) {
        return MiningProfile::unbreakable();
    }
    if let Some(compiled) = snapshot.mining_of(state_id.0) {
        match sc_block::mining_drops::mining_decision(compiled, hand) {
            sc_block::mining_drops::MineDecision::Deny => return MiningProfile::denied(),
            sc_block::mining_drops::MineDecision::Mine { required_ticks, .. } => {
                return MiningProfile {
                    required_ticks,
                    unbreakable: false,
                    declared: true,
                    can_mine: true,
                };
            }
        }
    }
    match snapshot.mining_seconds_of(state_id.0) {
        Some(seconds) => MiningProfile::from_seconds(seconds),
        None => MiningProfile::fallback(),
    }
}

// Validation helpers.

/// Distance check: player-to-target-center distance squared must stay within MAX_INTERACT_DISTANCE_SQ.
/// Entities without Transform (not yet initialized) skip validation and may act.
fn in_reach(world: &World, entity: &EntityId, target: &BlockPosition) -> bool {
    let Some(transform) = world.get_component::<Transform>(entity) else {
        return true; // 无位置信息，跳过校验
    };
    let pos = { transform.read().position };
    let dx = pos.x - (target.x as f32 + 0.5);
    let dy = pos.y - (target.y as f32 + 0.5);
    let dz = pos.z - (target.z as f32 + 0.5);
    dx * dx + dy * dy + dz * dz <= MAX_INTERACT_DISTANCE_SQ
}

/// Whether the player is in creative (creative placement never spends server inventory).
/// Entities without MinecraftClient count as non-creative (inventory is spent).
fn is_creative(world: &World, entity: &EntityId) -> bool {
    world
        .get_component::<MinecraftClient>(entity)
        .map(|client| client.data.read().gamemode.is_creative())
        .unwrap_or(false)
}

/// Entity runtime id (routes ResyncInventory intents; None without the component).
fn runtime_id_of(world: &World, entity: &EntityId) -> Option<u64> {
    world
        .get_component::<MinecraftEntityId>(entity)
        .map(|id| id.0)
}

/// Push one authoritative-inventory-resync intent to the network layer (rolls back client prediction when placement fails validation).
///
/// When budget runs out, escalate to a bounded deferred resync request (see `net_backpressure`) so the client
/// still receives authoritative inventory state instead of losing the rollback fact.
fn request_inventory_resync(world: &World, outbox: &mut NetworkOutbox, entity: &EntityId) {
    let Some(runtime_id) = runtime_id_of(world, entity) else {
        return;
    };
    let mut pending = world.get_resource_mut::<PendingInventoryResync>();
    let mut faults = world.get_resource_mut::<PendingConnectionFaults>();
    IntentPublisher::new(outbox)
        .with_world(world)
        .maybe_with_pending_resync(pending.as_deref_mut())
        .maybe_with_faults(faults.as_deref_mut())
        .publish(NetworkIntent::ResyncInventory {
            entity_id: runtime_id,
        });
}

/// Read the block at coordinates (None when unloaded or out of bounds).
fn block_at(
    manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    target: &BlockPosition,
) -> Option<BlockRuntimeId> {
    block_snapshot(manager, world_id, target).map(|snapshot| snapshot.state)
}

fn block_snapshot(
    manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    target: &BlockPosition,
) -> Option<BlockExpectation> {
    let minecraft_world = manager.get_world(world_id)?;
    let (min_y, max_y) = minecraft_world.vertical_bounds();
    if target.y < min_y || target.y > max_y {
        return None;
    }
    let key = ChunkKey::new(
        minecraft_world.world_data.get_dimension(),
        target.chunk_position(),
    );
    let column = minecraft_world.chunk_provider.cached_chunk(key)?;
    let chunk = column.read();
    let state = chunk.block_at(target.local_x(), target.y, target.local_z())?;
    Some(BlockExpectation {
        state,
        incarnation: column.incarnation(),
        generation: column.generation(),
    })
}

fn correct_predicted_break(world: &World, entity: &EntityId, position: BlockPosition) {
    let Some(world_id) = world.get_component::<MinecraftWorldId>(entity) else {
        return;
    };
    let Some(view) = world.get_component::<sc_world::chunk_view::ChunkView>(entity) else {
        return;
    };
    let dimension = view.read().dimension;
    crate::net_backpressure::request_column_refresh(
        world,
        &world_id,
        dimension,
        position.chunk_position(),
    );
}

/// Single-block prediction rollback: send the authoritative state at this position only to the predicting player.
///
/// A full-column refresh resends the whole chunk (74KB plus a full redraw, a visible flicker),
/// so it stays reserved for blind spots with no authoritative state. All other rejections come here: the client only predicted
/// cracks (Stop clears progress) or one ghost block, healed by a single rollback packet.
fn revert_single_block(
    world: &World,
    outbox: &mut NetworkOutbox,
    entity: &EntityId,
    position: BlockPosition,
) {
    let (Some(world_id), Some(manager)) = (
        world.get_component::<MinecraftWorldId>(entity),
        world.get_resource::<MinecraftWorldManager>(),
    ) else {
        return;
    };
    let Some(state) = block_at(&manager, &world_id, &position) else {
        correct_predicted_break(world, entity, position);
        return;
    };
    let Some(actor) = runtime_id_of(world, entity) else {
        return;
    };
    let Some(minecraft_world) = manager.get_world(world_id.as_ref()) else {
        return;
    };
    crate::net_backpressure::IntentPublisher::new(outbox)
        .with_world(world)
        .publish(NetworkIntent::CorrectBlockPrediction {
            entity_id: actor,
            world_id: world_id.as_ref().clone(),
            dimension: minecraft_world.world_data.get_dimension(),
            x: position.x,
            y: position.y,
            z: position.z,
            runtime_id: state.0,
            flags: update_flags::DEFAULT,
            layer: 0,
        });
}

fn cancel_break(world: &World, entity: &EntityId, outbox: &mut NetworkOutbox) {
    if let Some(position) = world
        .get_component::<BreakingState>(entity)
        .and_then(|state| state.position)
    {
        emit_break_progress(
            world,
            outbox,
            entity,
            crate::net::BlockBreakProgressCue::Stop,
            position,
            0,
        );
        // Aborting/switching targets shows at most cracks (Stop clears them); server blocks never move,
        // so no column refresh (whole-column resends visibly flicker).
    }
    world.remove_component::<BreakingState>(entity);
}

/// Finish an old break inside the tolerance (called on ABORT release / START target switch).
///
/// Client prediction always runs 1-3 ticks ahead of the server (network half-trip plus tick
/// quantization, see [`BREAK_EARLY_TOLERANCE_TICKS`]): when the player releases on seeing the block
/// vanish, `advance_block_break` usually still needs 1-3 ticks. Cancelling outright would lose this break
/// forever while the earlier ghost rollback flashes. When remaining time fits the tolerance and target/tool/epoch/
/// distance/mode still hold, land the break directly and return true; otherwise return false with no effects.
fn try_grace_complete_break(
    world: &World,
    entity: &EntityId,
    queue: &mut BlockChangeQueue,
    outbox: &mut NetworkOutbox,
) -> bool {
    let now = Instant::now();
    let Some(state) = world
        .get_component::<BreakingState>(entity)
        .map(|state| state.as_ref().clone())
    else {
        return false;
    };
    let (Some(position), Some(world_id)) = (
        state.position,
        world.get_component::<MinecraftWorldId>(entity),
    ) else {
        return false;
    };
    if state.elapsed_at(now) + BREAK_EARLY_TOLERANCE_TICKS < state.required_ticks.max(1) {
        return false;
    }
    let manager = world.get_resource::<MinecraftWorldManager>();
    let target = manager.as_ref().and_then(|manager| {
        block_snapshot(manager, &world_id, &position)
    });
    let Some(target) = target else {
        return false;
    };
    let hand = hand_snapshot(world, entity);
    let creative = is_creative(world, entity);
    if !same_mining_target(
        &state,
        position,
        &world_id,
        mining_context_epoch(world, entity),
        target,
        &hand,
    ) || !in_reach(world, entity, &position)
        || creative != state.creative
    {
        return false;
    }
    log::debug!(
        "[interaction] grace-finishing break (need {} ticks, have {} ticks): pos={}",
        state.required_ticks,
        state.elapsed_at(now),
        position
    );
    emit_break_progress(
        world,
        outbox,
        entity,
        crate::net::BlockBreakProgressCue::Stop,
        position,
        0,
    );
    let admitted = queue.try_push_break(
        BlockChange {
            world_id: world_id.as_ref().clone(),
            position,
            layer: 0,
            state: BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()),
            cause: BlockChangeCause::Player(*entity),
            flags: update_flags::DEFAULT,
        },
        target,
        BlockBreakContext {
            hand: state.hand.clone(),
            creative: state.creative,
        },
    );
    world.remove_component::<BreakingState>(entity);
    if admitted.is_none() {
        // Queue full: the break never landed but the client shows air; single-block rollback (no column refresh).
        revert_single_block(world, outbox, entity, position);
    }
    true
}

fn target_is_replaceable(
    manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    target: &BlockPosition,
) -> bool {
    block_at(manager, world_id, target)
        == Some(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
}

fn emit_break_progress(
    world: &World,
    outbox: &mut NetworkOutbox,
    entity: &EntityId,
    cue: crate::net::BlockBreakProgressCue,
    position: BlockPosition,
    data: i32,
) {
    let Some(world_id) = world.get_component::<MinecraftWorldId>(entity) else {
        return;
    };
    // Break progress is presentation state: safely ignored when budget runs out (rebroadcast next tick).
    IntentPublisher::new(outbox).publish(NetworkIntent::BlockBreakProgress {
        world_id: world_id.as_ref().clone(),
        cue,
        data,
        x: position.x as f32,
        y: position.y as f32,
        z: position.z as f32,
    });
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

// Two-phase break systems.

/// START_BREAK: start authoritative server timing from block JSON mining durations (survival phase one).
///
/// - The target block's data decides the duration (`minecraft:destructible_by_mining` seconds x 20
///   rounded up); `sc:unbreakable` is refused outright in survival;
/// - Early/repeated client START_BREAKs (`CONTINUE_DESTROY_BLOCK` maps here too)
///   only refresh presentation progress, never resetting timing.
pub fn handle_start_break(
    world: World,
    mut reader: EventReader<StartBreakRequest>,
    mut queue: ResMut<BlockChangeQueue>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    for request in reader.read() {
        start_break(&world, request, &mut queue, &mut outbox);
    }
}

fn start_break(
    world: &World,
    request: &StartBreakRequest,
    queue: &mut BlockChangeQueue,
    outbox: &mut NetworkOutbox,
) {
    info!(
        "[interaction] StartBreakRequest: pos={} face={}",
        request.position, request.face
    );
    if !in_reach(&world, &request.entity, &request.position) {
        warn!(
            "[interaction] StartBreakRequest 超出交互距离，忽略: pos={}",
            request.position
        );
        // Out of reach: the client only shows cracks (Stop clears them), no column refresh.
        cancel_break(world, &request.entity, outbox);
        return;
    }
    let (Some(world_id), Some(manager)) = (
        world.get_component::<MinecraftWorldId>(&request.entity),
        world.get_resource::<MinecraftWorldManager>(),
    ) else {
        return;
    };
    let Some(target_snapshot) = block_snapshot(&manager, &world_id, &request.position) else {
        cancel_break(world, &request.entity, outbox);
        correct_predicted_break(world, &request.entity, request.position);
        return;
    };
    if target_snapshot.state.0 == sc_world::block_dictionary::air_runtime_id() {
        // Target is already air: both sides agree, no rollback.
        cancel_break(world, &request.entity, outbox);
        return;
    }
    let hand = hand_snapshot(world, &request.entity);
    let creative = is_creative(world, &request.entity);
    let epoch = mining_context_epoch(world, &request.entity);
    let previous = world
        .get_component::<BreakingState>(&request.entity)
        .map(|state| state.as_ref().clone());
    if let Some(previous) = previous {
        if same_mining_target(
            &previous,
            request.position,
            &world_id,
            epoch,
            target_snapshot,
            &hand,
        ) && previous.creative == creative
        {
            // Only the tick advances cracks; CONTINUE packets cannot add progress.
            world.add_component(
                &request.entity,
                BreakingState {
                    face: request.face,
                    ..previous
                },
            );
            return;
        }
        // Switching targets: when the old target fits the tolerance (ticks from done), land it first,
        // then open the new one; otherwise holding to chain-mine would lose the old block under the new START.
        // Only out-of-tolerance switches count as real target switches: the old target only showed cracks, no refresh.
        if try_grace_complete_break(world, &request.entity, queue, outbox) {
            // Old break already landed (Stop sent, state cleared); open the new target without another Stop.
        } else if let Some(position) = previous.position {
            emit_break_progress(
                &world,
                outbox,
                &request.entity,
                crate::net::BlockBreakProgressCue::Stop,
                position,
                0,
            );
            // Switching targets: the old target only showed cracks, no column refresh.
        }
    }

    // Authoritative duration for the target block (block JSON dense columns; tool/efficiency-aware `sc:mining`, explicit fallback when undeclared/unregistered).
    let target = Some(target_snapshot.state);
    // Hand snapshot (by value; empty hands take default; live enchant path is 0, see hand_snapshot).
    let profile = mining_profile_with_hand(world, target_snapshot.state, &hand);
    // Creative never reads mining times (stock creative breaks bedrock; no drops either).
    if creative {
        world.remove_component::<BreakingState>(&request.entity);
        world.add_component(
            &request.entity,
            BreakingState {
                position: Some(request.position),
                face: request.face,
                target,
                required_ticks: 1,
                elapsed_ticks: 0,
                world_id: Some(world_id.as_ref().clone()),
                context_epoch: epoch,
                incarnation: target_snapshot.incarnation,
                hand,
                creative,
                started_at: Some(Instant::now()),
            },
        );
        emit_break_progress(
            &world,
            outbox,
            &request.entity,
            crate::net::BlockBreakProgressCue::Start,
            request.position,
            break_progress_step(1),
        );
        return;
    }
    if profile.unbreakable {
        warn!(
            "[interaction] 目标不可破坏（sc:unbreakable），拒绝: pos={}",
            request.position
        );
        emit_break_progress(
            &world,
            outbox,
            &request.entity,
            crate::net::BlockBreakProgressCue::Stop,
            request.position,
            0,
        );
        world.remove_component::<BreakingState>(&request.entity);
        // Bedrock-likes: the client only shows cracks (Stop clears them), no column refresh.
        return;
    }
    if !profile.can_mine {
        warn!(
            "[interaction] 目标工具不可挖（sc:mining can_mine=false），拒绝: pos={}",
            request.position
        );
        emit_break_progress(
            &world,
            outbox,
            &request.entity,
            crate::net::BlockBreakProgressCue::Stop,
            request.position,
            0,
        );
        world.remove_component::<BreakingState>(&request.entity);
        // Wrong tool: the client only shows cracks (Stop clears them), no column refresh.
        return;
    }
    if !profile.declared {
        warn!(
            "[interaction] 目标未声明挖掘秒数，回退 {} tick: pos={}",
            FALLBACK_BREAK_TIME_TICKS, request.position
        );
    }

    world.remove_component::<BreakingState>(&request.entity);
    world.add_component(
        &request.entity,
        BreakingState {
            position: Some(request.position),
            face: request.face,
            target,
            required_ticks: profile.required_ticks,
            elapsed_ticks: 0,
            world_id: Some(world_id.as_ref().clone()),
            context_epoch: epoch,
            incarnation: target_snapshot.incarnation,
            hand,
            creative,
            started_at: Some(Instant::now()),
        },
    );
    emit_break_progress(
        &world,
        outbox,
        &request.entity,
        crate::net::BlockBreakProgressCue::Start,
        request.position,
        break_progress_step(profile.required_ticks),
    );
}

fn same_mining_target(
    state: &BreakingState,
    position: BlockPosition,
    world_id: &MinecraftWorldId,
    epoch: Option<u64>,
    target: BlockExpectation,
    hand: &sc_block::mining_drops::HandSnapshot,
) -> bool {
    state.position == Some(position)
        && state.world_id.as_ref() == Some(world_id)
        && state.context_epoch == epoch
        && state.target == Some(target.state)
        && state.incarnation == target.incarnation
        && &state.hand == hand
}

/// ABORT_BREAK: clear BreakingState (player releases / switches target).
///
/// Releasing inside the tolerance (letting go on seeing the block vanish while the server needs a few ticks) counts as done:
/// land the break instead of cancelling, or this break is lost forever (the normal release-to-stop case).
pub fn handle_abort_break(
    world: World,
    mut reader: EventReader<AbortBreakRequest>,
    mut queue: ResMut<BlockChangeQueue>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    for request in reader.read() {
        abort_break(&world, request, &mut queue, &mut outbox);
    }
}

fn abort_break(
    world: &World,
    request: &AbortBreakRequest,
    queue: &mut BlockChangeQueue,
    outbox: &mut NetworkOutbox,
) {
    info!(
        "[interaction] AbortBreakRequest: entity={:?}",
        request.entity
    );
    let position = world
        .get_component::<BreakingState>(&request.entity)
        .and_then(|state| state.position);
    if request.position.is_some() && request.position != position {
        return;
    }
    if try_grace_complete_break(world, &request.entity, queue, outbox) {
        return;
    }
    cancel_break(world, &request.entity, outbox);
}

/// Authoritative server mining advance (every tick).
///
/// Block data sets the duration; the server lands breaks itself on expiry (client STOP_BREAK is only
/// an early/sync signal, never the sole authority). When the target changes mid-break (broken/replaced by another),
/// abort immediately and never harm the new block.
pub fn advance_block_break(
    world: World,
    mut queue: ResMut<BlockChangeQueue>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    for entity in world.entities_with_component::<BreakingState>() {
        let Some(state) = world
            .get_component::<BreakingState>(&entity)
            .map(|state| state.as_ref().clone())
        else {
            continue;
        };
        let (Some(position), Some(world_id)) = (
            state.position,
            world.get_component::<MinecraftWorldId>(&entity),
        ) else {
            world.remove_component::<BreakingState>(&entity);
            continue;
        };
        let target = world
            .get_resource::<MinecraftWorldManager>()
            .and_then(|manager| block_snapshot(&manager, &world_id, &position));
        let hand = hand_snapshot(&world, &entity);
        let Some(target) = target else {
            cancel_break(&world, &entity, &mut outbox);
            continue;
        };
        if !same_mining_target(
            &state,
            position,
            &world_id,
            mining_context_epoch(&world, &entity),
            target,
            &hand,
        ) || !in_reach(&world, &entity, &position)
            || is_creative(&world, &entity) != state.creative
        {
            cancel_break(&world, &entity, &mut outbox);
            continue;
        }
        let elapsed = state.elapsed_at(Instant::now());
        if elapsed < state.required_ticks {
            world.add_component(
                &entity,
                BreakingState {
                    elapsed_ticks: elapsed,
                    ..state.clone()
                },
            );
            // Vanilla clients predict crack progress from their own mining rules.
            // Repeated UPDATE_BREAK packets are reserved for custom diggers.
            continue;
        }
        // On expiry: the server lands the break (branch A enqueue, PostUpdate apply, BlockChanged broadcast).
        emit_break_progress(
            &world,
            &mut outbox,
            &entity,
            crate::net::BlockBreakProgressCue::Stop,
            position,
            0,
        );
        let admitted = queue.try_push_break(
            BlockChange {
                world_id: world_id.as_ref().clone(),
                position,
                layer: 0,
                state: BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()),
                cause: BlockChangeCause::Player(entity),
                flags: update_flags::DEFAULT,
            },
            target,
            BlockBreakContext {
                hand: state.hand.clone(),
                creative: state.creative,
            },
        );
        if admitted.is_none() {
            // Queue full: the break never landed but the client may show air; single-block rollback (no column refresh).
            revert_single_block(&world, &mut outbox, &entity, position);
        }
        world.remove_component::<BreakingState>(&entity);
    }
}

/// STOP_BREAK / CREATIVE_DESTROY: validate, then enqueue air (branch A).
///
/// Survival (`require_breaking_state=true`): beyond BreakingState position match, the
/// authoritative server timer must cover the block JSON mining duration. Clients always submit 1-3 ticks
/// early (network half-trip plus tick quantization): remaining time inside [`BREAK_EARLY_TOLERANCE_TICKS`]
/// counts as on time and lands directly (no rollback, or every break would flash once);
/// out-of-tolerance still rolls back and keeps timing (anti-instabreak).
/// Creative (`false`): break immediately, ignoring hardness (stock creative breaks bedrock).
pub fn handle_break_block(
    world: World,
    mut reader: EventReader<BreakBlockRequest>,
    mut queue: ResMut<BlockChangeQueue>,
    mut outbox: ResMut<NetworkOutbox>,
) {
    for request in reader.read() {
        break_block(&world, request, &mut queue, &mut outbox);
    }
}

fn break_block(
    world: &World,
    request: &BreakBlockRequest,
    queue: &mut BlockChangeQueue,
    outbox: &mut NetworkOutbox,
) {
    info!(
        "[interaction] BreakBlockRequest: pos={} face={} require_state={}",
        request.position, request.face, request.require_breaking_state
    );

    // Survival: validate the BreakingState match (reject forged STOP_BREAK).
    if request.require_breaking_state {
        let state = world
            .get_component::<BreakingState>(&request.entity)
            .map(|state| state.as_ref().clone());
        if state
            .as_ref()
            .map(|state| state.position == Some(request.position))
            != Some(true)
        {
            // Trailing STOPs after completion (state cleared, block already air) are normal packets;
            // let them through silently; only real mismatches roll back single blocks. Column refresh would flicker.
            let moved_on = world
                .get_resource::<MinecraftWorldManager>()
                .and_then(|manager| {
                    world
                        .get_component::<MinecraftWorldId>(&request.entity)
                        .and_then(|world_id| block_at(&manager, &world_id, &request.position))
                });
            if moved_on.is_some_and(|state| {
                state.0 != sc_world::block_dictionary::air_runtime_id()
            }) {
                warn!(
                    "[interaction] BreakBlockRequest BreakingState 不匹配，单块回滚: pos={}",
                    request.position
                );
                revert_single_block(world, outbox, &request.entity, request.position);
            }
            return;
        }
        if let Some(state) = state.as_ref() {
            if !state.complete_at(Instant::now()) {
                let now = Instant::now();
                if state.elapsed_at(now) + BREAK_EARLY_TOLERANCE_TICKS
                    >= state.required_ticks.max(1)
                {
                    // In-tolerance early submits (measured always 1-3 ticks early) count as on time;
                    // fall into the enqueue path below, no rollback (rollback flashes the client).
                    log::debug!(
                        "[interaction] early submit inside tolerance (need {} ticks, have {} ticks): pos={}",
                        state.required_ticks,
                        state.elapsed_at(now),
                        request.position
                    );
                } else {
                    // Genuinely early (cheating-level): keep timing, ghost air rolls back single-block.
                    // Log at debug (normal races are common); column refresh would flicker.
                    log::debug!(
                        "[interaction] 客户端提前提交破坏（需 {} tick，已 {} tick），继续计时: pos={}",
                        state.required_ticks,
                        state.elapsed_at(now),
                        request.position
                    );
                    revert_single_block(world, outbox, &request.entity, request.position);
                    return;
                }
            }
        }
        if let Some(position) = state.and_then(|state| state.position) {
            emit_break_progress(
                &world,
                outbox,
                &request.entity,
                crate::net::BlockBreakProgressCue::Stop,
                position,
                0,
            );
        }
        // Clear BreakingState after commit.
        // Removed after the final target/tool validation below.
    }

    // Distance check.
    if !in_reach(&world, &request.entity, &request.position) {
        warn!(
            "[interaction] BreakBlockRequest 超出交互距离，单块回滚: pos={}",
            request.position
        );
        revert_single_block(world, outbox, &request.entity, request.position);
        return;
    }

    let Some(world_id) = world.get_component::<MinecraftWorldId>(&request.entity) else {
        return;
    };
    let target = world
        .get_resource::<MinecraftWorldManager>()
        .and_then(|manager| block_snapshot(&manager, &world_id, &request.position));
    let Some(target) = target else {
        cancel_break(world, &request.entity, outbox);
        correct_predicted_break(world, &request.entity, request.position);
        return;
    };
    let hand = hand_snapshot(world, &request.entity);
    let creative = is_creative(world, &request.entity);
    if request.require_breaking_state {
        let valid = world
            .get_component::<BreakingState>(&request.entity)
            .is_some_and(|state| {
                same_mining_target(
                    &state,
                    request.position,
                    &world_id,
                    mining_context_epoch(world, &request.entity),
                    target,
                    &hand,
                ) && state.creative == creative
            });
        if !valid {
            cancel_break(world, &request.entity, outbox);
            // Target/tool/epoch changed: roll back single-block to current authority (blind spots auto-upgrade to column).
            revert_single_block(world, outbox, &request.entity, request.position);
            return;
        }
    } else if !creative {
        // Creative wire actions never bypass survival timing.
        revert_single_block(world, outbox, &request.entity, request.position);
        return;
    }
    let admitted = queue.try_push_break(
        BlockChange {
            world_id: world_id.as_ref().clone(),
            position: request.position,
            layer: 0,
            state: BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()),
            cause: BlockChangeCause::Player(request.entity),
            flags: update_flags::DEFAULT,
        },
        target,
        BlockBreakContext { hand, creative },
    );
    world.remove_component::<BreakingState>(&request.entity);
    if admitted.is_none() {
        // Queue full: the break never landed but the client may show air; single-block rollback.
        revert_single_block(world, outbox, &request.entity, request.position);
    }
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
        // Empty hands cannot place (clients never predict empty-hand placement, no resync needed).
        if crate::crafting::open_crafting_station(
            &world,
            request.entity,
            request.position,
            &mut outbox,
        ) {
            continue;
        }
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
            crate::crafting::bump_inventory_revision(&world, &request.entity);
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
            state: BlockRuntimeId(block_runtime_id),
            cause: BlockChangeCause::Player(request.entity),
            flags: update_flags::DEFAULT,
        });
        if let Some(reservation) = reservation {
            reservations.pending.insert(request_id, reservation);
        }
    }
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
        crate::crafting::bump_inventory_revision(&world, &reservation.entity);
        request_inventory_resync(&world, &mut outbox, &reservation.entity);
    }
}

// Hotbar systems.

/// Hotbar slot switch: update PlayerInventory.selected_slot.
///
/// Sent after the network layer decodes `MobEquipment` (0x1f) as [`HeldSlotChanged`];
/// this system only updates server state; the network outbox translation layer broadcasts.
pub fn handle_held_slot_changed(world: World, mut reader: EventReader<HeldSlotChanged>) {
    for event in reader.read() {
        let slot = event.slot;
        if slot > 8 {
            warn!("[interaction] HeldSlotChanged 槽位越界: {}", slot);
            continue;
        }
        if let Some(inventory) = world.get_component::<PlayerInventory>(&event.entity) {
            if inventory.set_selected_slot(slot) {
                info!("[interaction] 热栏切换: slot={}", slot);
            }
        }
    }
}

pub fn handle_open_inventory(world: World, mut reader: EventReader<OpenInventoryRequest>) {
    for request in reader.read() {
        let Some(runtime_id) = runtime_id_of(&world, &request.entity) else {
            continue;
        };
        if runtime_id != request.target_entity_id {
            warn!(
                "[interaction] OpenInventoryRequest target mismatch: player={} target={}",
                runtime_id, request.target_entity_id
            );
            continue;
        }
        if world
            .get_component::<PlayerInventory>(&request.entity)
            .is_none()
        {
            warn!(
                "[interaction] player has no PlayerInventory: entity={:?}",
                request.entity
            );
            continue;
        }
        // One window per client, other half: no workstation window while the inventory window is open.
        // Component presence means "window open", so clear old window state before registering the new one.
        if let Some(previous) = world.get_component::<ContainerOpen>(&request.entity).map(|open| (*open).clone()) {
            debug!(
                "[interaction] 打开背包前先关掉旧容器窗口 window={} type={}",
                previous.window_id,
                previous.kind_name()
            );
            crate::craft_inventory::return_items(&world, request.entity);
            world.remove_component::<ContainerOpen>(&request.entity);
        }
        world.add_component(&request.entity, ContainerOpen::player_inventory());
        outbox_for_open_inventory(&world, runtime_id);
    }
}

/// Close the container window a player currently has open.
///
/// [`ContainerOpen`] is the single source of truth: a present component gets removed, and
/// an absent one means the server never opened a window, so the client close is dropped. Nothing
/// `window_id` / `container_type` ——
///
/// - `window_id` allocates incrementally per player (1..99); whatever the
/// client echoes never changes our recorded id (replies use the recorded `open.window_id`);
/// - `container_type` may echo `NONE` (-9, as vanilla does closing workstations),
///   and matching on it would drop closes on id mismatch, leaving `ContainerOpen`
///   on the entity forever so the player can never open containers again.
pub fn handle_close_inventory(world: World, mut reader: EventReader<CloseInventoryRequest>) {
    for request in reader.read() {
        close_open_container(&world, &request);
    }
}

/// Close the container window this player has open (when one is open).
///
/// One close handled per function call, so tests can pin the "presence closes, client bytes never parse" invariant directly.
fn close_open_container(world: &World, request: &CloseInventoryRequest) {
    let Some(open) = world.get_component::<ContainerOpen>(&request.entity).map(|open| (*open).clone()) else {
        debug!(
            "[interaction] 丢弃无对应窗口的 ContainerClose: window_id={} container_type={}",
            request.window_id, request.container_type
        );
        return;
    };
    crate::craft_inventory::return_items(world, request.entity);
    // Component presence means "window open"; removing it closes the window. This cannot fail,
    // so no path leaves a stuck marker behind.
    world.remove_component::<ContainerOpen>(&request.entity);
    if let Some(mut outbox) = world.get_resource_mut::<NetworkOutbox>() {
        request_inventory_resync(world, &mut outbox, &request.entity);
    }
    if request.window_id != open.window_id {
        debug!(
            "[interaction] ContainerClose window_id={} 与服务端窗口 {} 不一致，按服务端窗口关闭",
            request.window_id, open.window_id
        );
    }
    let Some(runtime_id) = runtime_id_of(world, &request.entity) else {
        return;
    };
    if request.was_server_initiated {
        return;
    }
    let Some(mut outbox) = world.get_resource_mut::<NetworkOutbox>() else {
        return;
    };
    // Reply type uses the server-registered real type, never reflected client bytes (see
    // `ContainerOpen::protocol_type`): vanilla closes workstations with NONE (-9), and reflecting it
    // would corrupt client window tracking so later windows close instantly.
    IntentPublisher::new(&mut outbox).publish(NetworkIntent::ClosePlayerInventory {
        entity_id: runtime_id,
        window_id: open.window_id,
        container_type: open.protocol_type(),
    });
}

fn outbox_for_open_inventory(world: &World, runtime_id: u64) {
    let Some(mut outbox) = world.get_resource_mut::<NetworkOutbox>() else {
        return;
    };
    IntentPublisher::new(&mut outbox).publish(NetworkIntent::OpenPlayerInventory {
        entity_id: runtime_id,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_ecs::system::{IntoSystem, System};

    #[test]
    fn monotonic_deadline_is_independent_of_update_frequency() {
        let start = Instant::now();
        let state = BreakingState {
            started_at: Some(start),
            required_ticks: 150,
            ..Default::default()
        };
        assert!(!state.complete_at(start + Duration::from_millis(7499)));
        assert!(state.complete_at(start + Duration::from_millis(7500)));
        assert_eq!(state.elapsed_at(start + Duration::from_millis(7518)), 150);
        for _ in 0..1000 {
            assert!(!state.complete_at(start));
        }
    }

    #[test]
    fn slow_tick_completes_without_waiting_for_150_system_calls() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        let state = world.get_component::<BreakingState>(&entity).unwrap();
        world.add_component(
            &entity,
            BreakingState {
                required_ticks: 150,
                elapsed_ticks: 139,
                started_at: Some(Instant::now() - Duration::from_millis(7518)),
                ..state.as_ref().clone()
            },
        );
        advance_block_break.into_system().run(&world);
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
        assert!(world.get_component::<BreakingState>(&entity).is_none());
    }

    #[test]
    fn vanilla_mining_does_not_emit_per_tick_update_break() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        for _ in 0..5 {
            advance(&world);
        }
        let intents = world.get_resource_mut::<NetworkOutbox>().unwrap().drain();
        assert_eq!(
            intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    NetworkIntent::BlockBreakProgress {
                        cue: crate::net::BlockBreakProgressCue::Start,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(!intents.iter().any(|intent| matches!(
            intent,
            NetworkIntent::BlockBreakProgress {
                cue: crate::net::BlockBreakProgressCue::Update,
                ..
            }
        )));
    }

    fn mining_world() -> (
        World,
        EntityId,
        std::sync::Arc<sc_world::storage::ChunkColumn>,
    ) {
        use sc_world::chunk::ChunkPosition;
        use sc_world::storage::{EmptyWorldStorage, WorldChunkProvider};
        let world = World::new();
        let provider = WorldChunkProvider::new(std::sync::Arc::new(EmptyWorldStorage));
        let column = provider
            .insert_empty_chunk(ChunkKey::new(0, ChunkPosition::new(0, 0)), -64, 319)
            .unwrap();
        {
            let mut chunk = column.write();
            for x in 0..3 {
                chunk.set_block_at(0, x, 64, 0, BlockRuntimeId(32000));
            }
        }
        let mut manager = MinecraftWorldManager::new();
        let id = manager.push_world(
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
        let view = sc_world::chunk_view::ChunkView::new(id.clone(), 0, 2, ChunkPosition::new(0, 0));
        view.write()
            .desired_chunks
            .insert(ChunkKey::new(0, ChunkPosition::new(0, 0)));
        let entity = world.spawn((
            id,
            view,
            MiningInbox::default(),
            PlayerInventory::new(36),
            sc_entity::MinecraftEntityId(7),
        ));
        world.insert_resource(manager);
        world.insert_resource(BlockChangeQueue::default());
        world.insert_resource(sc_block::write::BlockChangedQueue::default());
        world.insert_resource(sc_block::write::PendingBlockChangeLoads::default());
        world.insert_resource(NetworkOutbox::default());
        world.insert_resource(ItemRegistry::new());
        (world, entity, column)
    }

    fn enqueue(world: &World, entity: EntityId, action: MiningAction) {
        let id = world.get_component::<MinecraftWorldId>(&entity).unwrap();
        assert!(world
            .get_component::<MiningInbox>(&entity)
            .unwrap()
            .try_push(OrderedMiningAction {
                world_id: id.as_ref().clone(),
                context_epoch: mining_context_epoch(world, &entity),
                action,
            }));
    }

    fn drain(world: &World) {
        drain_mining_actions.into_system().run(world);
    }
    fn advance(world: &World) {
        for entity in world.entities_with_component::<BreakingState>() {
            let state = world.get_component::<BreakingState>(&entity).unwrap();
            let mut next = state.as_ref().clone();
            next.started_at = next
                .started_at
                .map(|start| start - Duration::from_millis(50));
            world.add_component(&entity, next);
        }
        advance_block_break.into_system().run(world);
    }
    fn apply(world: &World) {
        sc_block::write::apply_block_changes
            .into_system()
            .run(world);
    }

    #[test]
    fn changing_face_preserves_progress_without_extra_crack_packets() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 2 });
        drain(&world);
        for _ in 0..17 {
            advance(&world);
        }
        let count = world.get_resource::<NetworkOutbox>().unwrap().pending();
        enqueue(&world, entity, MiningAction::Start { position, face: 5 });
        drain(&world);
        let state = world.get_component::<BreakingState>(&entity).unwrap();
        assert_eq!(state.elapsed_ticks, 17);
        assert_eq!(state.face, 5);
        assert_eq!(
            world.get_resource::<NetworkOutbox>().unwrap().pending(),
            count
        );
        for _ in 0..3 {
            advance(&world);
        }
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
    }

    #[test]
    fn ordered_completion_then_new_start_keeps_both_operations() {
        let (world, entity, column) = mining_world();
        let a = BlockPosition::new(0, 64, 0);
        let b = BlockPosition::new(1, 64, 0);
        enqueue(
            &world,
            entity,
            MiningAction::Start {
                position: a,
                face: 1,
            },
        );
        drain(&world);
        let state = world.get_component::<BreakingState>(&entity).unwrap();
        world.add_component(
            &entity,
            BreakingState {
                elapsed_ticks: state.required_ticks,
                started_at: Some(
                    Instant::now() - Duration::from_millis(state.required_ticks as u64 * 50),
                ),
                ..state.as_ref().clone()
            },
        );
        enqueue(
            &world,
            entity,
            MiningAction::Break {
                position: a,
                face: 1,
                require_breaking_state: true,
            },
        );
        enqueue(
            &world,
            entity,
            MiningAction::Start {
                position: b,
                face: 1,
            },
        );
        drain(&world);
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .position,
            Some(b)
        );
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
        apply(&world);
        assert_eq!(
            column.read().block_at(0, 64, 0),
            Some(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
        );
        assert_eq!(
            column.read().block_at(1, 64, 0),
            Some(BlockRuntimeId(32000))
        );
    }

    #[test]
    fn final_timer_tick_completes_before_next_input_phase_changes_target() {
        let (world, entity, column) = mining_world();
        let a = BlockPosition::new(0, 64, 0);
        let b = BlockPosition::new(1, 64, 0);
        enqueue(
            &world,
            entity,
            MiningAction::Start {
                position: a,
                face: 1,
            },
        );
        drain(&world);
        for _ in 0..19 {
            advance(&world);
        }
        enqueue(
            &world,
            entity,
            MiningAction::Break {
                position: a,
                face: 1,
                require_breaking_state: true,
            },
        );
        enqueue(
            &world,
            entity,
            MiningAction::Start {
                position: b,
                face: 1,
            },
        );
        // Match the existing schedule: Update timer, SCEventUpdate input, PostUpdate write.
        advance(&world);
        drain(&world);
        apply(&world);
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .position,
            Some(b)
        );
        assert_eq!(
            column.read().block_at(0, 64, 0),
            Some(BlockRuntimeId(sc_world::block_dictionary::air_runtime_id()))
        );
        assert_eq!(
            world
                .get_resource::<sc_block::write::BlockChangedQueue>()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn stale_abort_does_not_cancel_a_new_target() {
        let (world, entity, _) = mining_world();
        let a = BlockPosition::new(0, 64, 0);
        let b = BlockPosition::new(1, 64, 0);
        enqueue(
            &world,
            entity,
            MiningAction::Start {
                position: a,
                face: 1,
            },
        );
        enqueue(&world, entity, MiningAction::Abort { position: a });
        enqueue(
            &world,
            entity,
            MiningAction::Start {
                position: b,
                face: 1,
            },
        );
        enqueue(&world, entity, MiningAction::Abort { position: a });
        drain(&world);
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .position,
            Some(b)
        );
    }

    #[test]
    fn start_and_predict_in_one_batch_keep_authoritative_timing_and_resync() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        enqueue(
            &world,
            entity,
            MiningAction::Break {
                position,
                face: 1,
                require_breaking_state: true,
            },
        );
        drain(&world);
        assert!(world.get_resource::<BlockChangeQueue>().unwrap().is_empty());
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .elapsed_ticks,
            0
        );
        // Early submit: the server keeps timing; ghost air rolls back single-block, never a column refresh.
        // (Whole-column resends visibly flicker on clients).
        assert!(world
            .get_component::<sc_world::chunk_view::ChunkView>(&entity)
            .unwrap()
            .read()
            .refresh_required
            .is_empty());
        let intents = world
            .get_resource_mut::<NetworkOutbox>()
            .unwrap()
            .drain();
        assert!(intents.iter().any(|intent| matches!(
            intent,
            NetworkIntent::CorrectBlockPrediction {
                x: 0,
                y: 64,
                z: 0,
                runtime_id: 32000,
                ..
            }
        )));
        for _ in 0..20 {
            advance(&world);
        }
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
    }

    /// Measured live (2026-10-07): clients always finish prediction 1-3 ticks early;
    /// (`need 60 ticks, have 58 ticks`). In-tolerance early submits must land directly, 
    /// never rolling back, or every break would flash the old block once.
    #[test]
    fn predict_within_tolerance_completes_without_revert() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        // Fallback duration 20 ticks: mined 17 (3 left, within tolerance 5).
        backdate_breaking(&world, entity, 17 * 50);
        enqueue(
            &world,
            entity,
            MiningAction::Break {
                position,
                face: 1,
                require_breaking_state: true,
            },
        );
        drain(&world);
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
        assert!(world.get_component::<BreakingState>(&entity).is_none());
        let intents = world
            .get_resource_mut::<NetworkOutbox>()
            .unwrap()
            .drain();
        assert!(
            !intents.iter().any(|intent| matches!(
                intent,
                NetworkIntent::CorrectBlockPrediction { .. }
            )),
            "in-tolerance completions must not roll back (rollback flashes)"
        );
    }

    /// Out-of-tolerance early submits are still cheating-level: roll back and keep timing (no leniency on instabreak).
    #[test]
    fn predict_beyond_tolerance_still_reverts_and_keeps_mining() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        // Mined 10 ticks (10 left, over tolerance 5): genuinely early.
        backdate_breaking(&world, entity, 10 * 50);
        enqueue(
            &world,
            entity,
            MiningAction::Break {
                position,
                face: 1,
                require_breaking_state: true,
            },
        );
        drain(&world);
        assert!(world.get_resource::<BlockChangeQueue>().unwrap().is_empty());
        assert!(world.get_component::<BreakingState>(&entity).is_some());
        let intents = world
            .get_resource_mut::<NetworkOutbox>()
            .unwrap()
            .drain();
        assert!(intents.iter().any(|intent| matches!(
            intent,
            NetworkIntent::CorrectBlockPrediction { .. }
        )));
    }

    /// Letting go on seeing the block vanish (prediction sent, ABORT right after) is a normal finish:
    /// in-tolerance releases land directly, losing no break.
    #[test]
    fn abort_within_tolerance_finishes_the_break() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        backdate_breaking(&world, entity, 18 * 50);
        enqueue(&world, entity, MiningAction::Abort { position });
        drain(&world);
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
        assert!(world.get_component::<BreakingState>(&entity).is_none());
    }

    /// Releasing far too early still cancels: nothing breaks.
    #[test]
    fn abort_beyond_tolerance_still_cancels() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        backdate_breaking(&world, entity, 5 * 50);
        enqueue(&world, entity, MiningAction::Abort { position });
        drain(&world);
        assert!(world.get_resource::<BlockChangeQueue>().unwrap().is_empty());
        assert!(world.get_component::<BreakingState>(&entity).is_none());
    }

    /// Chain-mining while holding: when the new START covers the old target inside the tolerance, land the old break first,
    /// then open the new target: the old block is never lost and fresh cracks appear.
    #[test]
    fn target_switch_within_tolerance_lands_old_and_starts_new() {
        let (world, entity, _) = mining_world();
        let a = BlockPosition::new(0, 64, 0);
        let b = BlockPosition::new(1, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position: a, face: 1 });
        drain(&world);
        backdate_breaking(&world, entity, 17 * 50);
        enqueue(&world, entity, MiningAction::Start { position: b, face: 1 });
        drain(&world);
        assert_eq!(world.get_resource::<BlockChangeQueue>().unwrap().len(), 1);
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .position,
            Some(b)
        );
        // The new target gets START crack broadcasts (cracks never break).
        let intents = world
            .get_resource_mut::<NetworkOutbox>()
            .unwrap()
            .drain();
        assert!(intents.iter().any(|intent| matches!(
            intent,
            NetworkIntent::BlockBreakProgress {
                cue: crate::net::BlockBreakProgressCue::Start,
                ..
            }
        )));
    }

    /// Move `BreakingState.started_at` back by milliseconds (simulating mined time).
    fn backdate_breaking(world: &World, entity: EntityId, millis: u64) {
        let state = world.get_component::<BreakingState>(&entity).unwrap();
        world.add_component(
            &entity,
            BreakingState {
                started_at: Some(
                    Instant::now() - Duration::from_millis(millis),
                ),
                ..state.as_ref().clone()
            },
        );
    }

    #[test]
    fn tool_switch_cancels_old_progress_but_duplicate_equipment_does_not() {
        let (world, entity, _) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        advance(&world);
        enqueue(&world, entity, MiningAction::HeldSlot { slot: 0 });
        drain(&world);
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .elapsed_ticks,
            1
        );
        enqueue(&world, entity, MiningAction::HeldSlot { slot: 1 });
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        assert_eq!(
            world
                .get_component::<BreakingState>(&entity)
                .unwrap()
                .elapsed_ticks,
            0
        );
    }

    #[test]
    fn replaced_target_and_retired_context_cannot_finish_old_mining() {
        let (world, entity, column) = mining_world();
        let position = BlockPosition::new(0, 64, 0);
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        drain(&world);
        column
            .write()
            .set_block_at(0, 0, 64, 0, BlockRuntimeId(32001));
        advance(&world);
        assert!(world.get_component::<BreakingState>(&entity).is_none());
        enqueue(&world, entity, MiningAction::Start { position, face: 1 });
        world
            .get_component::<sc_world::chunk_view::ChunkView>(&entity)
            .unwrap()
            .write()
            .epoch += 1;
        drain(&world);
        assert!(world.get_component::<BreakingState>(&entity).is_none());
        assert!(world.get_resource::<BlockChangeQueue>().unwrap().is_empty());
    }

    #[test]
    fn mining_inbox_rejects_overflow_without_evicting_accepted_actions() {
        let inbox = MiningInbox::default();
        let world_id = MinecraftWorldId::random();
        for x in 0..MiningInbox::MAX_ACTIONS {
            assert!(inbox.try_push(OrderedMiningAction {
                world_id: world_id.clone(),
                context_epoch: None,
                action: MiningAction::Start {
                    position: BlockPosition::new(x as i32, 64, 0),
                    face: 1
                }
            }));
        }
        assert!(!inbox.try_push(OrderedMiningAction {
            world_id,
            context_epoch: None,
            action: MiningAction::Abort {
                position: BlockPosition::new(0, 64, 0)
            }
        }));
        let actions = inbox.take();
        assert_eq!(actions.len(), MiningInbox::MAX_ACTIONS);
        assert!(
            matches!(actions.front().unwrap().action, MiningAction::Start { position, .. } if position.x == 0)
        );
    }

    /// [`ContainerOpen`] is the container window's single source of truth, so closing never parses
    /// `window_id` / `container_type`: even a client `NONE` (-9) echo still closes.
    ///
    /// Regression guard: this used to match `window_id == server id || window_id == 1 &&
    /// type in {1,29}`; past window id 2 nothing matched anymore and `ContainerOpen`
    /// stayed on the entity forever so the player could never open containers.
    #[test]
    fn close_inventory_clears_the_component_regardless_of_client_window_bytes() {
        fn workstation_entity(world: &World) -> EntityId {
            world.spawn(ContainerOpen::at_workstation(
                7,
                sc_recipe::StationKind::CraftingTable,
                sc_world::manager::MinecraftWorldId::random(),
                (1, 2, 3),
            ))
        }

        // Client echoes window_id=7 but container_type=NONE (-9): vanilla workstation close
        // behavior. Still closes.
        let world = World::new();
        let entity = workstation_entity(&world);
        assert!(world.get_component::<ContainerOpen>(&entity).is_some());
        close_open_container(
            &world,
            &CloseInventoryRequest {
                entity,
                window_id: 7,
                container_type: -9,
                was_server_initiated: false,
            },
        );
        assert!(world.get_component::<ContainerOpen>(&entity).is_none());

        // Fully mismatched window ids (client 3, server 7) still close.
        let entity = workstation_entity(&world);
        close_open_container(
            &world,
            &CloseInventoryRequest {
                entity,
                window_id: 3,
                container_type: 0,
                was_server_initiated: false,
            },
        );
        assert!(world.get_component::<ContainerOpen>(&entity).is_none());

        // Server never opened a window: drop it, state unchanged (nothing created or cleared).
        let entity = workstation_entity(&world);
        let stranger = world.spawn(sc_utils::components::DisplayName("stranger".to_string()));
        close_open_container(
            &world,
            &CloseInventoryRequest {
                entity: stranger,
                window_id: 1,
                container_type: 1,
                was_server_initiated: false,
            },
        );
        assert!(world.get_component::<ContainerOpen>(&entity).is_some());
    }

    /// Close replies must carry the server-registered real window type (workstation=1), never reflected
    /// client NONE (-9): close replies take the registered window type, and the comment
    /// "Client always wants a response. If not sent, inventores won't open
    /// anymore." A wrong reply type corrupts client window tracking, so later windows close instantly.
    #[test]
    fn close_inventory_echoes_server_window_type_not_client_none() {
        let world = World::new();
        world.insert_resource(NetworkOutbox::default());
        let entity = world.spawn((
            ContainerOpen::at_workstation(
                7,
                sc_recipe::StationKind::CraftingTable,
                sc_world::manager::MinecraftWorldId::random(),
                (1, 2, 3),
            ),
            MinecraftEntityId(42),
        ));
        close_open_container(
            &world,
            &CloseInventoryRequest {
                entity,
                window_id: 7,
                container_type: -9,
                was_server_initiated: false,
            },
        );
        let outbox = world
            .get_resource_mut::<NetworkOutbox>()
            .expect("outbox resource");
        let mut outbox = outbox;
        let intents = outbox.drain();
        let close = intents.iter().find_map(|intent| match intent {
            NetworkIntent::ClosePlayerInventory {
                window_id,
                container_type,
                ..
            } => Some((*window_id, *container_type)),
            _ => None,
        });
        assert_eq!(close, Some((7, 1)), "回显窗口号 7、真实类型 WORKBENCH(1)");
    }

    /// Break progress is a **per-tick increment** (accumulated by clients); the step formula matches `65535 / breakTick`.
    #[test]
    fn break_progress_step_matches_pnx_formula() {
        assert_eq!(break_progress_step(20), 65_535 / 20); // 旧常量行为（回退时长）
        assert_eq!(break_progress_step(30), 65_535 / 30); // stone 1.5s
        assert_eq!(break_progress_step(60), 65_535 / 60); // coal_ore 3s
        assert_eq!(break_progress_step(2000), 65_535 / 2000); // water 100s
        assert_eq!(break_progress_step(1), 65_535);
        // At least 1: even very long breaks must visibly advance (never a 0 step).
        assert_eq!(break_progress_step(1_000_000), 1);
        // Illegal inputs never degrade to 0 and never divide by zero.
        assert_eq!(break_progress_step(0), 65_535);
        assert_eq!(break_progress_step(-5), 65_535);
        assert_eq!(break_progress_step(i32::MAX), 1);
    }

    /// Client accumulation invariant: `required` increments must land near 65535 (never early-full, never overflowing).
    #[test]
    fn accumulated_progress_reaches_full_at_required_ticks() {
        for required in [1, 2, 12, 20, 30, 60, 700, 2000, 65_535] {
            let step = break_progress_step(required);
            let accumulated = step as i64 * required as i64;
            assert!(
                accumulated <= 65_535,
                "{required} tick 累加溢出：{accumulated}"
            );
            assert!(
                accumulated > 65_535 - required as i64,
                "{required} tick 累加不足（裂纹到不了最后阶段）：{accumulated}"
            );
        }
    }

    /// Mining durations come from block JSON seconds: rounded up to ticks, at least 1 tick.
    #[test]
    fn mining_profile_converts_declared_seconds_to_ticks() {
        // stone: minecraft:destructible_by_mining 1.5s → 30 tick.
        let stone = MiningProfile::from_seconds(1.5);
        assert_eq!(stone.required_ticks, 30);
        assert!(stone.declared);
        assert!(!stone.unbreakable);
        // Sub-tick durations still take 1 tick (0 seconds goes to 1 tick, never an instant break).
        assert_eq!(MiningProfile::from_seconds(0.0).required_ticks, 1);
        assert_eq!(MiningProfile::from_seconds(0.01).required_ticks, 1);
        // Non-finite/negative values are not guesses: fall back to the explicit constant and mark undeclared.
        for bad in [f32::NAN, f32::INFINITY, -1.0] {
            let profile = MiningProfile::from_seconds(bad);
            assert_eq!(profile.required_ticks, FALLBACK_BREAK_TIME_TICKS);
            assert!(!profile.declared);
        }
        // sc:unbreakable goes to unbreakable.
        let bedrock = MiningProfile::unbreakable();
        assert!(bedrock.unbreakable);
        assert!(bedrock.declared);
        // Undeclared goes to the explicit fallback (countable, never silent).
        assert_eq!(MiningProfile::fallback().required_ticks, 20);
        assert!(!MiningProfile::fallback().declared);
    }

    /// End to end: break durations really come from `.block.json` dense columns (not hardcoded tables).
    #[test]
    fn mining_profile_reads_json_snapshot_columns() {
        use sc_block::block_json::{compile_bundle, BlockJsonRegistry};
        use sc_packloader::block::{
            fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
        };

        let air = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {"minecraft:destructible_by_mining": {"value": 0.0}},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let stone = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:stone", "states": {}},
                "components": {"minecraft:destructible_by_mining": {"value": 1.5}},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [7]
            }
        }"#;
        let bedrock = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:bedrock", "states": {}},
                "components": {"sc:unbreakable": {}},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [9]
            }
        }"#;
        let undeclared = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "test:undeclared", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [11]
            }
        }"#;
        let raws: Vec<(&str, &str)> = vec![
            ("definitions/blocks/minecraft/air.block.json", air),
            ("definitions/blocks/minecraft/bedrock.block.json", bedrock),
            ("definitions/blocks/minecraft/stone.block.json", stone),
            ("definitions/blocks/test/undeclared.block.json", undeclared),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b, &budgets).expect("夹具应合法"))
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
        };
        let snapshot = compile_bundle(&bundle, "test", &budgets, &|_| None, &|_| true)
            .expect("应编译")
            .0;
        let registry = BlockStateRegistry::from_block_snapshot(&snapshot);
        // Take all runtime ids first, then move the snapshot (never borrow across move).
        let runtime_of = |id: &str| {
            let state = snapshot
                .default_state_idx(id)
                .unwrap_or_else(|| panic!("{id} 应有默认态"));
            registry
                .runtime_id(sc_block::state::BlockStateId(state))
                .unwrap_or_else(|| panic!("{id} 应有 runtime id"))
        };
        let stone_runtime = runtime_of("minecraft:stone");
        let air_runtime = runtime_of("minecraft:air");
        let bedrock_runtime = runtime_of("minecraft:bedrock");
        let undeclared_runtime = runtime_of("test:undeclared");

        let world = World::new();
        world.insert_resource(BlockStateRegistry::from_block_snapshot(&snapshot));
        let published = BlockJsonRegistry::new();
        published.publish(std::sync::Arc::new(snapshot));
        world.insert_resource(published);

        // stone 1.5s goes to 30 ticks; air 0s goes to 1 tick; bedrock is unbreakable.
        let stone_profile = mining_profile(&world, stone_runtime);
        assert_eq!(stone_profile.required_ticks, 30);
        assert!(stone_profile.declared);
        assert!(!stone_profile.unbreakable);
        let air_profile = mining_profile(&world, air_runtime);
        assert_eq!(air_profile.required_ticks, 1);
        assert!(air_profile.declared);
        assert!(mining_profile(&world, bedrock_runtime).unbreakable);
        // Undeclared goes to the explicit fallback constant (never guessing values).
        let fallback = mining_profile(&world, undeclared_runtime);
        assert_eq!(fallback.required_ticks, FALLBACK_BREAK_TIME_TICKS);
        assert!(!fallback.declared);
        // Unregistered runtime ids (old packs/bootstrap dictionary) take the same explicit fallback, never panic.
        assert_eq!(
            mining_profile(&world, BlockRuntimeId(u32::MAX)).required_ticks,
            FALLBACK_BREAK_TIME_TICKS
        );
    }

    /// Tool matching and default rules: empty hands/misses take default, hits take tool entries (exact ids, no wildcards).
    #[test]
    fn tool_matching_uses_default_for_empty_and_miss() {
        use sc_block::block_json::{compile_bundle, BlockJsonRegistry};
        use sc_block::mining_drops::HandSnapshot;
        use sc_packloader::block::{
            fingerprint_bundle, parse_block_file, BlockBundleBudgets, BlockJsonBundle,
        };
        let air = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "minecraft:air", "states": {}},
                "components": {},
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [0]
            }
        }"#;
        let ore = r#"{
            "format_version": "1.10.0",
            "minecraft:block": {
                "description": {"identifier": "example:ore", "states": {}},
                "components": {
                    "sc:mining": {
                        "formula_version": 1,
                        "base_time_seconds": 3.0,
                        "default": {"can_mine": false, "harvest": false, "speed_multiplier": 1.0},
                        "tools": [
                            {"items": ["minecraft:iron_pickaxe", "minecraft:diamond_pickaxe"], "can_mine": true, "harvest": true, "speed_multiplier": 5.0}
                        ]
                    }
                },
                "sc:default_state": {},
                "sc:protocol_runtime_ids": [32000]
            }
        }"#;
        let raws: Vec<(&str, &str)> = vec![
            ("definitions/blocks/minecraft/air.block.json", air),
            ("definitions/blocks/example/ore.block.json", ore),
        ];
        let budgets = BlockBundleBudgets::default();
        let mut refs: Vec<(&str, &[u8])> = raws.iter().map(|(p, b)| (*p, b.as_bytes())).collect();
        refs.sort_by(|a, b| a.0.cmp(b.0));
        let files = refs
            .iter()
            .map(|(p, b)| parse_block_file("test", p, b, &budgets).unwrap())
            .collect();
        let bundle = BlockJsonBundle {
            schema_version: 1,
            network_id_mode: "hashed".to_string(),
            fingerprint: fingerprint_bundle(&refs, "hashed"),
            files,
        };
        // Item existence: test items pass.
        let allow = |id: &str| {
            matches!(
                id,
                "minecraft:iron_pickaxe" | "minecraft:diamond_pickaxe" | "minecraft:wooden_pickaxe"
            )
        };
        let snapshot = compile_bundle(&bundle, "test", &budgets, &|_| None, &allow)
            .expect("应编译")
            .0;
        let registry = BlockStateRegistry::from_block_snapshot(&snapshot);
        let ore_runtime = {
            let idx = snapshot.default_state_idx("example:ore").unwrap();
            registry
                .runtime_id(sc_block::state::BlockStateId(idx))
                .unwrap()
        };
        let world = World::new();
        world.insert_resource(BlockStateRegistry::from_block_snapshot(&snapshot));
        let published = BlockJsonRegistry::new();
        published.publish(std::sync::Arc::new(snapshot));
        world.insert_resource(published);

        // Empty hands go to default (can_mine=false) and are refused.
        let empty = mining_profile_with_hand(&world, ore_runtime, &HandSnapshot::default());
        assert!(!empty.can_mine);
        assert_eq!(empty, MiningProfile::denied());
        // Wooden-pickaxe misses take default too and are refused.
        let miss = mining_profile_with_hand(
            &world,
            ore_runtime,
            &HandSnapshot {
                item: Some("minecraft:wooden_pickaxe".to_string()),
                efficiency_level: 0,
                fortune_level: 0,
            },
        );
        assert!(!miss.can_mine);
        // Iron-pickaxe hits: 3/5=0.6s goes to 12 ticks.
        let hit = mining_profile_with_hand(
            &world,
            ore_runtime,
            &HandSnapshot {
                item: Some("minecraft:iron_pickaxe".to_string()),
                efficiency_level: 0,
                fortune_level: 0,
            },
        );
        assert!(hit.can_mine);
        assert_eq!(hit.required_ticks, 18);
        // Diamond-pickaxe hits (multi-item entries) also take 18 ticks (exact ids, no priority guessing).
        let hit2 = mining_profile_with_hand(
            &world,
            ore_runtime,
            &HandSnapshot {
                item: Some("minecraft:diamond_pickaxe".to_string()),
                efficiency_level: 0,
                fortune_level: 0,
            },
        );
        assert_eq!(hit2.required_ticks, 18);
    }

    /// Mining time conversion (exact formula): `time=base*branch/(tool+eff)` goes to `max(1, ceil(time*20))`.
    #[test]
    fn mining_time_conversion_matches_formula() {
        use sc_block::mining_drops::{
            mining_decision, EfficiencyCompiled, HandSnapshot, MineDecision, MiningCompiled,
            ToolRuleCompiled,
        };
        use std::collections::HashMap;
        let mut tools = HashMap::new();
        tools.insert(
            Box::from("minecraft:iron_pickaxe"),
            ToolRuleCompiled {
                can_mine: true,
                harvest: true,
                speed_multiplier: 5.0,
            },
        );
        let compiled = MiningCompiled {
            base_seconds: 3.0,
            base_from_destructible: false,
            penalty: 5.0,
            default_rule: ToolRuleCompiled {
                can_mine: true,
                harvest: false,
                speed_multiplier: 1.0,
            },
            tools,
            efficiency: Some(EfficiencyCompiled { max_level: 5 }),
            formula_version: 1,
        };
        // No enchants: 3*1.5/5=0.9s goes to 18 ticks (harvested).
        assert_eq!(
            mining_decision(
                &compiled,
                &HandSnapshot {
                    item: Some("minecraft:iron_pickaxe".to_string()),
                    efficiency_level: 0,
                    fortune_level: 0
                }
            ),
            MineDecision::Mine {
                required_ticks: 18,
                harvested: true
            }
        );
        // Efficiency III (exact formula): speed=5+(9+1)=15, 4.5/15=0.3s goes to 6 ticks.
        assert_eq!(
            mining_decision(
                &compiled,
                &HandSnapshot {
                    item: Some("minecraft:iron_pickaxe".to_string()),
                    efficiency_level: 3,
                    fortune_level: 0
                }
            ),
            MineDecision::Mine {
                required_ticks: 6,
                harvested: true
            }
        );
        // Unmatched tools take default (unharvested): 3*5/1=15s goes to 300 ticks.
        assert_eq!(
            mining_decision(
                &compiled,
                &HandSnapshot {
                    item: Some("minecraft:stick".to_string()),
                    efficiency_level: 0,
                    fortune_level: 0
                }
            ),
            MineDecision::Mine {
                required_ticks: 300,
                harvested: false
            }
        );
        // base 0 goes to the 1-tick floor.
        let mut instant = compiled.clone();
        instant.base_seconds = 0.0;
        assert_eq!(
            mining_decision(&instant, &HandSnapshot::default()),
            MineDecision::Mine {
                required_ticks: 1,
                harvested: false
            }
        );
    }
}
