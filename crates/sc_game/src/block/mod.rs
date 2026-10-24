//! 权威方块域：单所有者提交、破坏计时与掉落物。
//!
//! Authoritative block domain: single-owner commits, break timing and drops.
//!
//! - [`facts`]：消费可靠的方块写入事实，发布游戏性与网络后果
//!   （掉落物、声音、粒子）；
//!   [`facts`]: consumes reliable block-write facts and publishes gameplay
//!   and network consequences (drops, sounds, particles);
//! - [`interaction`]：放置/破坏边界事件与内核默认校验（生存两段式破坏、
//!   创造绕过、预留占位）。
//!   [`interaction`]: place/break boundary events and kernel-default
//!   validation (two-phase survival breaks, creative bypass, reservations).
//!
//! Region 规则：目标方块所在区块的所有者在提交时校验
//! `owner epoch + expected state/generation + permission`；输入阶段的校验
//! 永远不能替代它。
//! Region rule: the owner of the target block's chunk validates
//! `owner epoch + expected state/generation + permission` at commit time;
//! input-stage checks never replace it.

pub mod facts;
pub mod facing;
pub mod interaction;

pub use facts::{block_changed_to_outbox, harvested_for_hand, roll_break_drops, BreakSettlementReceipts};
pub use interaction::{
    advance_block_break, break_progress_step, drain_mining_actions, handle_abort_break,
    handle_break_block, handle_close_inventory, handle_held_slot_changed, handle_open_inventory,
    handle_place_block, handle_start_break, hand_snapshot, mining_context_epoch, mining_profile,
    mining_profile_with_hand, resolve_block_placement_reservations, AbortBreakRequest,
    BlockPlacementReservations, BreakBlockRequest, BreakingState, CloseInventoryRequest,
    HeldSlotChanged, MiningAction, MiningInbox, MiningProfile, OpenInventoryRequest,
    OrderedMiningAction, PlaceBlockRequest, StartBreakRequest, BREAK_EARLY_TOLERANCE_TICKS,
    FALLBACK_BREAK_TIME_TICKS,
};
