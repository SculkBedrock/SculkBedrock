//! 物品实体域：带代际检查的掉落存储、合并与拾取。
//!
//! Item-entity domain: generation-checked drop storage, merge and pickup.
//!
//! 掉落物是高基数、短生命周期的对象，因此它们存放在按世界划分的槽位存储中，
//! 并配有空间格子索引，而不是完整的 ECS 实体。未来的 region 所有者可以把一个
//! [`RegionItemDropStore`] 移到 region 邮箱之后，而不改变网络或物品语义。
//! Drops are high-cardinality short-lived objects, so they live in a
//! per-world slot store with a spatial cell index instead of full ECS
//! entities. A future region owner can move one [`RegionItemDropStore`]
//! behind a region mailbox without changing network or item semantics.

pub mod drop;

pub use drop::{item_drop_tick, item_pickup_tick, DropHandle, ItemDrop, ItemDropStore, RegionItemDropStore};
