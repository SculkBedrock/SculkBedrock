//! Item-entity domain: generation-checked drop storage, merge and pickup.
//!
//! Drops are high-cardinality short-lived objects, so they live in a
//! per-world slot store with a spatial cell index instead of full ECS
//! entities. A future region owner can move one [`RegionItemDropStore`]
//! behind a region mailbox without changing network or item semantics.

pub mod drop;

pub use drop::{item_drop_tick, item_pickup_tick, DropHandle, ItemDrop, ItemDropStore, RegionItemDropStore};
