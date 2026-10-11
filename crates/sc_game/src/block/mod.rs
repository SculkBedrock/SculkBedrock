//! Authoritative block domain: single-owner commits, break timing and drops.
//!
//! - [`facts`]: consumes reliable block-write facts and publishes gameplay
//!   and network consequences (drops, sounds, particles);
//! - [`interaction`]: place/break boundary events and kernel-default
//!   validation (two-phase survival breaks, creative bypass, reservations).
//!
//! Region rule (§4, §6 unified design): the owner of the target block's
//! chunk validates `owner epoch + expected state/generation + permission`
//! at commit time; input-stage checks never replace it.

pub mod facts;
pub mod facing;
pub mod interaction;

pub use facts::{block_changed_to_outbox, harvested_for_hand, roll_break_drops, DropReceipts};
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
