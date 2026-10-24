//! 方块实体的 NBT 保留层与序列化 trait。
//!
//! NBT preservation layer and serialization traits for block entities.

use sc_ecs::component::Component;
use sc_nbt::compound::CompoundNbt;

/// 为往返持久化保留未知、扩展或自定义的 NBT 标签。
///
/// Preserves unknown, extended, or custom NBT tags for round-trip persistence.
///
/// 确保 SculkBedrock 修改方块实体并重新保存到 LevelDB 时，不会丢失
/// 原版客户端或第三方编辑器写入的标签。
/// Ensures vanilla client or third-party editor tags are not lost when
/// SculkBedrock modifies and re-saves the block entity to LevelDB.
#[derive(Component, Clone, Debug)]
pub struct BlockUnknownNbt(pub CompoundNbt);

impl Default for BlockUnknownNbt {
    fn default() -> Self {
        Self(CompoundNbt::new(None))
    }
}

impl BlockUnknownNbt {
    /// 从 NBT 复合标签构造。
    ///
    /// Constructs from an NBT compound.
    ///
    /// - `compound`: NBT 复合标签
    pub fn new(compound: CompoundNbt) -> Self {
        Self(compound)
    }

    /// 保留的标签是否为空。
    ///
    /// Whether the preserved tags are empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 自定义显示名（如来自铁砧或编辑器，序列化为 `"CustomName"`）。
///
/// Custom display name (e.g. from an anvil or editor, serialized to
/// `"CustomName"`).
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockCustomName(pub String);

impl BlockCustomName {
    /// 从字符串构造。
    ///
    /// Constructs from a string.
    ///
    /// - `name`: 显示名
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

/// 容器锁钥匙（序列化为 `"Lock"`）。
///
/// Container lock key (serialized to `"Lock"`).
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockLock(pub String);

impl BlockLock {
    /// 从字符串构造。
    ///
    /// Constructs from a string.
    ///
    /// - `key`: 锁钥匙
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

/// 定义一个方块实体家族的 Bedrock NBT 序列化/反序列化 trait。
///
/// Trait defining Bedrock NBT serialization and deserialization for a block
/// entity family.
pub trait BlockEntityCodec {
    /// 规范的 Bedrock 方块实体 ID（如 "Chest"、"Furnace"）。
    ///
    /// Canonical Bedrock block entity ID (e.g. "Chest", "Furnace").
    fn nbt_id(&self) -> &'static str;

    /// 把实体状态序列化为 Bedrock NBT 复合标签，并与保留的未知标签合并。
    ///
    /// Serialize the entity state into a Bedrock NBT compound, merging with
    /// preserved unknown tags.
    ///
    /// - `unknown`: 保留的未知标签（可为 `None`）
    fn serialize_to_nbt(&self, unknown: Option<&BlockUnknownNbt>) -> CompoundNbt;
}
