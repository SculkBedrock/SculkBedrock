//! Reserved world/region identity types (unified-design §3, §8).
//!
//! These distinguish concepts that must never share one `u64` epoch:
//! - [`WorldInstanceId`]: one physical open of a save path; reopening the
//!   same path mints a new id so stale worker results cannot pollute it;
//! - [`RegionShardId`]: stable storage sharding, unaffected by scheduler
//!   region merge/split;
//! - [`RegionEpoch`]: ownership fencing token, bumped on every owner
//!   transfer or region reorganisation; stale tasks are rejected on mismatch;
//! - [`RegionTick`]: a region's own tick sequence, not a server wall clock.
//!
//! Status: reserved for the RegionScheduler / chunk-pipeline identity work
//! (phases 1, 4). `GameWorldId` remains the live routing key until the
//! scheduler migrates to these ids.

/// Physical instance of an opened world (reopen ⇒ new id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct WorldInstanceId(pub u128);

impl WorldInstanceId {
    pub fn new(id: u128) -> Self {
        Self(id)
    }
}

/// Stable storage shard (chunk grid ⇒ shard, scheduler-independent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct RegionShardId(pub u64);

/// Ownership fencing token (bumped on transfer/merge/split).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct RegionEpoch(pub u64);

impl RegionEpoch {
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

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
