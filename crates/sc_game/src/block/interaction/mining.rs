//! Survival mining: two-phase breaks, abort handling and authoritative advance.
//!
//! Moved verbatim from `interaction.rs` (P4 split); shared types and helpers
//! live in [`super`] and are re-exported there so existing paths keep compiling.

use super::*;
use log::{info, warn};
use sc_block::write::{
    update_flags, BlockBreakContext, BlockChange, BlockChangeCause, BlockChangeQueue,
};
use sc_ecs::entity::EntityId;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_entity::MinecraftEntityId;
use sc_world::chunk::BlockRuntimeId;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use std::time::Instant;
use crate::net::{
    IntentPublisher,
    NetworkOutbox, SyncHunger,
};

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

pub(super) fn start_break(
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

pub(super) fn abort_break(
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
        } else if !state.creative {
            // Landed survival break: mining is exhausting (feeds hunger).
            // Sync hunger immediately on change (the tick only covers
            // players it does not skip).
            if crate::hunger::add_exhaustion(
                &world,
                &entity,
                crate::hunger::MINE_EXHAUSTION_PER_BLOCK,
            ) {
                if let Some(actor) = world.get_component::<MinecraftEntityId>(&entity) {
                    IntentPublisher::new(&mut outbox).publish(SyncHunger {
                        entity_id: actor.0,
                    });
                }
            }
        }
        world.remove_component::<BreakingState>(&entity);
    }
}
