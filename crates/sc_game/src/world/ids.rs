//! 预留的世界/region 标识类型。
//!
//! Reserved world/region identity types.
//!
//! 这些类型区分了绝不能共用同一个 `u64` 纪元的概念：
//! - [`WorldInstanceId`]：一次存档路径的物理开启；重开同一路径会铸造新 id，
//!   使过时的工作线程结果无法污染新实例；
//!   [`WorldInstanceId`]: one physical open of a save path; reopening the
//!   same path mints a new id so stale worker results cannot pollute it;
//! - [`RegionShardId`]：稳定的存储分片，不受调度器 region 合并/拆分影响；
//!   [`RegionShardId`]: stable storage sharding, unaffected by scheduler
//!   region merge/split;
//! - [`RegionEpoch`]：所有权围栏令牌，每次所有者转移或 region 重组时递增；
//!   纪元不匹配的过时任务会被拒绝；
//!   [`RegionEpoch`]: ownership fencing token, bumped on every owner
//!   transfer or region reorganisation; stale tasks are rejected on mismatch;
//! - [`RegionTick`]：region 自身的 tick 序列，不是服务器墙钟。
//!   [`RegionTick`]: a region's own tick sequence, not a server wall clock.
//!
//! 这些 id 预留给 RegionScheduler / 区块管线的标识工作；在调度器迁移到它们
//! 之前，`GameWorldId` 仍是当前的活路由键。
//! These ids are reserved for the RegionScheduler / chunk-pipeline identity
//! work; `GameWorldId` remains the live routing key until the scheduler
//! migrates to these ids.

/// 已开启世界的物理实例（重开 ⇒ 新 id）。
///
/// Physical instance of an opened world (reopen ⇒ new id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct WorldInstanceId(pub u128);

impl WorldInstanceId {
    /// 用给定 id 构造。
    ///
    /// Constructs from the given id.
    ///
    /// - `id`: 实例 id
    pub fn new(id: u128) -> Self {
        Self(id)
    }
}

/// 稳定的存储分片（区块网格 ⇒ 分片，与调度器无关）。
///
/// Stable storage shard (chunk grid ⇒ shard, scheduler-independent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct RegionShardId(pub u64);

/// 所有权围栏令牌（转移/合并/拆分时递增）。
///
/// Ownership fencing token (bumped on transfer/merge/split).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct RegionEpoch(pub u64);

impl RegionEpoch {
    /// 递增到下一个纪元（回绕）。
    ///
    /// Advances to the next epoch (wrapping).
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// 一个 region 自身的 tick 序列。
///
/// One region's own tick sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct RegionTick(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_advances_and_orders() {
        assert!(RegionEpoch(1).next() > RegionEpoch(1));
        assert_ne!(
            WorldInstanceId::new(1),
            WorldInstanceId::new(2),
            "reopens must not alias"
        );
    }
}
