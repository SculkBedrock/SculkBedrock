//! 方块实体空间标识组件。
//!
//! Block-entity spatial identity component.

use super::kind::BlockEntityKind;
use sc_ecs::component::Component;
use sc_world::manager::MinecraftWorldId;

/// 标识组件：本实体的状态属于哪个世界的哪个方块。
///
/// Identity component: which world block this entity's state belongs to.
///
/// tick 与交互系统从 `(world, pos)` 动态解析方块的声明，确保幽灵检测与
/// region 所有权不变式得以维持。
/// The tick and interaction systems resolve the block's declarations from
/// `(world, pos)` dynamically, ensuring ghost detection and region
/// ownership invariants are maintained.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct BlockEntityRef {
    /// 所属存档世界 id。
    ///
    /// Owning save-world id.
    pub world: MinecraftWorldId,
    /// 方块坐标。
    ///
    /// Block position.
    pub pos: (i32, i32, i32),
    /// 方块实体种类。
    ///
    /// Block-entity kind.
    pub kind: BlockEntityKind,
}

impl BlockEntityRef {
    /// 构造标识组件。
    ///
    /// Constructs the identity component.
    ///
    /// - `world`: 存档世界 id
    /// - `pos`: 方块坐标
    /// - `kind`: 方块实体种类
    pub fn new(world: MinecraftWorldId, pos: (i32, i32, i32), kind: BlockEntityKind) -> Self {
        Self { world, pos, kind }
    }
}
