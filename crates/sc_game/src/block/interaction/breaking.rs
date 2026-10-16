//! Commit-break path (`STOP_BREAK` / creative destroy validation + enqueue).
//!
//! Moved verbatim from `interaction.rs` (P4 split); shared types and helpers
//! live in [`super`] and are re-exported there so existing paths keep compiling.

use super::*;
use log::{info, warn};
use sc_block::write::{
    update_flags, BlockBreakContext, BlockChange, BlockChangeCause, BlockChangeQueue,
};
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_world::chunk::BlockRuntimeId;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use std::time::Instant;
use crate::net::NetworkOutbox;

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

pub(super) fn break_block(
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
