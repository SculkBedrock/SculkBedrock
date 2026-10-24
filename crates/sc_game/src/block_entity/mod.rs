//! 方块实体的组件与空间索引。
//!
//! 包含位置与种类标识、坐标到实体的索引、容器槽位、脏跟踪与未映射 NBT 保留。
//!
//! Components and spatial index for block entities.
//!
//! Covers position and kind identity, coordinate-to-entity indexing, container slots, dirty tracking, and preservation of unmapped NBT.

pub mod dirty;
pub mod ext;
pub mod identity;
pub mod index;
pub mod inventory;
pub mod kind;
pub mod nbt;

#[cfg(test)]
mod tests;

pub use dirty::BlockEntityDirty;
pub use ext::BlockEntityWorldExt;
pub use identity::BlockEntityRef;
pub use index::BlockEntityIndex;
pub use inventory::BlockInventory;
pub use kind::BlockEntityKind;
pub use nbt::{BlockCustomName, BlockEntityCodec, BlockLock, BlockUnknownNbt};

use sc_block::block_json::BlockJsonRegistry;
use sc_block::registry::BlockStateRegistry;
use sc_block::workstation::SlotRole;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_world::chunk::BlockRuntimeId;
use sc_world::manager::MinecraftWorldId;

/// 某位置方块的加工参数：时长加槽位角色，均从其
/// `sc:process` / `sc:workstation` 声明解析（角色保持声明中的槽位顺序；
/// `advance` 按角色解析，因此顺序无关紧要）。
/// `None` = 无法解析（区块未加载、注册表/快照缺失或没有加工声明）：
/// 调用方按失败关闭处理。
///
/// Process params for the block at a position: duration plus slot roles,
/// both resolved from its `sc:process` / `sc:workstation` decls (roles stay
/// in decl slot order; `advance` resolves by role, so order never matters).
/// `None` = unresolvable (unloaded chunk, missing registries/snapshot, or
/// no process): the caller fails closed.
///
/// - `world`: ECS 世界
/// - `world_id`: 存档世界 id
/// - `pos`: 方块坐标
///
/// # Returns
/// `(加工 tick 数, 槽位角色列表)`；无法解析时为 `None`。
/// `(cook ticks, slot roles)`; `None` when unresolvable.
pub fn process_params_at(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<(u16, Vec<SlotRole>)> {
    let hash = crate::furnace::block_hash_at(world, world_id, pos)?;
    let blocks = world.get_resource::<BlockStateRegistry>()?;
    let snapshot = world.get_resource::<BlockJsonRegistry>()?.get()?;
    let dense = blocks.by_runtime_id(BlockRuntimeId(hash))?;
    let furnace = snapshot.furnace_of(dense.0)?;
    Some((
        furnace.cook_ticks,
        furnace.slots.iter().map(|slot| slot.role).collect(),
    ))
}

/// 确保有状态方块的加工实体存在（有状态方块返回 `Some(entity)`，幂等：
/// 重复调用返回同一实体）。`None` = 准入被拒（该世界索引已满）或加工声明
/// 无法解析——调用方回退到既有的失败路径（打开/移动走重同步，tick 跳过），
/// 与当前存储有界时的行为一致。
///
/// Ensure the process entity for a stateful block (`Some` entity, idempotent:
/// repeat calls return the same entity). `None` = admission refused (index
/// full for this world) or unresolvable process decl — the caller falls back
/// to its existing failure path (resync for open/move, skip for tick),
/// exactly like the store bound today.
///
/// - `world`: ECS 世界
/// - `world_id`: 存档世界 id
/// - `pos`: 方块坐标
/// - `kind`: 方块实体种类
pub fn ensure_block_entity(
    world: &World,
    world_id: MinecraftWorldId,
    pos: (i32, i32, i32),
    kind: BlockEntityKind,
) -> Option<EntityId> {
    // Fast path: already tracked (idempotent, no chunk read).
    if let Some(existing) = world
        .get_resource::<BlockEntityIndex>()
        .and_then(|index| index.get(&world_id, pos))
    {
        return Some(existing);
    }
    // Admission before any world read (a full index refuses without touching
    // chunks or registries).
    let full = world
        .get_resource::<BlockEntityIndex>()
        .is_none_or(|index| index.count_for(&world_id) >= BlockEntityIndex::MAX_PER_WORLD);
    if full {
        return None;
    }
    let mut index = world.get_resource_mut::<BlockEntityIndex>()?;
    // Re-check under the mutable guard (single-threaded world: no race, but
    // the double check keeps the invariant local to this function).
    if let Some(existing) = index.get(&world_id, pos) {
        return Some(existing);
    }
    if index.count_for(&world_id) >= BlockEntityIndex::MAX_PER_WORLD {
        return None;
    }
    let entity = match kind {
        BlockEntityKind::Furnace(_) => {
            let (cook_ticks, roles) = process_params_at(world, &world_id, pos)?;
            world.spawn((
                BlockEntityRef::new(world_id.clone(), pos, kind),
                crate::inventory::crafting::BlockProcessState::new(kind.station(), roles, cook_ticks),
                BlockEntityDirty::CLEAN,
            ))
        }
        BlockEntityKind::Chest => {
            let slots = crate::inventory::crafting::chest_decl_at(world, &world_id, pos)
                .map(|decl| decl.slots as usize)
                .unwrap_or(27);
            world.spawn((
                BlockEntityRef::new(world_id.clone(), pos, kind),
                BlockInventory::new(slots),
                BlockEntityDirty::CLEAN,
            ))
        }
        BlockEntityKind::Sign => {
            world.spawn((
                BlockEntityRef::new(world_id.clone(), pos, kind),
                BlockEntityDirty::CLEAN,
            ))
        }
    };
    index.insert(world_id, pos, entity);
    Some(entity)
}

/// 销毁方块的加工实体（仅破坏路径；调用方先取出内容物作掉落物）。
/// 索引中没有对应条目时返回 `None`（幂等清理，绝不 panic）。tick 从不调用
/// 本函数（幽灵实体只跳过，与存储路径规则相同）。
///
/// Destroy the process entity for a block (break path only; the caller takes
/// out the contents for drops first). Missing entries are `None`
/// (idempotent cleanup, never a panic). The tick never calls this (ghosts
/// only skip, same rule as the store path).
///
/// - `world`: ECS 世界
/// - `world_id`: 存档世界 id
/// - `pos`: 方块坐标
pub fn destroy_block_entity(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<EntityId> {
    let removed = world
        .get_resource_mut::<BlockEntityIndex>()
        .and_then(|mut index| index.remove(world_id, pos))?;
    world.despawn(&removed);
    Some(removed)
}

/// 读取被跟踪方块的加工状态（未跟踪时为 `None`：从未被跟踪，或已被破坏
/// 路径销毁）。只读调用方（打开/移动/观看者同步）使用本函数；写入方先经过
/// [`ensure_block_entity`]。返回共享句柄（与 tick 持有同一个 `Arc`；
/// 调用期间不持守卫）。
///
/// Process state for a tracked block (`None` when untracked: never tracked,
/// or destroyed by the break path). Read-only callers (open/move/viewer
/// sync) use this; writers go through [`ensure_block_entity`] first.
/// Returned as the shared handle (same `Arc` the tick holds; no guard is
/// kept across the call).
///
/// - `world`: ECS 世界
/// - `world_id`: 存档世界 id
/// - `pos`: 方块坐标
pub fn process_state_at(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<std::sync::Arc<crate::inventory::crafting::BlockProcessState>> {
    let entity = world
        .get_resource::<BlockEntityIndex>()
        .and_then(|index| index.get(world_id, pos))?;
    world.get_component::<crate::inventory::crafting::BlockProcessState>(&entity)
}

/// 取出被跟踪方块的加工内容物用于破坏掉落（仅生存模式）。
///
/// Take a tracked block's process contents for break drops (survival only).
///
/// 按声明顺序读槽位、销毁标识，并返回非空物品栈。`None` = 未跟踪
/// （普通方块，无钩子）。调用方把这些堆叠追加到破坏掷出的掉落物上，
/// 使容器内容物共用 pending/backpressure 路径；燃烧/烹饪进度本身会丢失
/// （与原版一致）。创造模式破坏跳过本调用（无掉落），但仍会经过
/// [`destroy_block_entity`] 销毁。
/// Reads the slots in decl order, destroys the identity, and returns the
/// non-empty stacks. `None` = untracked (ordinary block, no hook). The
/// caller appends these to the break's rolled drops so container contents
/// share the pending/backpressure path; burn/cook progress itself is lost
/// (same as vanilla). Creative breaks skip this call (drop nothing) but
/// still destroy via [`destroy_block_entity`].
///
/// - `world`: ECS 世界
/// - `world_id`: 存档世界 id
/// - `pos`: 方块坐标
pub fn take_break_contents(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<Vec<sc_item::ItemStack>> {
    let entity = world
        .get_resource::<BlockEntityIndex>()
        .and_then(|index| index.get(world_id, pos))?;

    // 1. Workstation / process state (e.g. furnace)
    if let Some(process) = world.get_component::<crate::inventory::crafting::BlockProcessState>(&entity) {
        let contents: Vec<sc_item::ItemStack> = process
            .snapshot()
            .slots
            .into_iter()
            .filter_map(|(_, stack)| (!stack.is_empty()).then_some(stack))
            .collect();
        destroy_block_entity(world, world_id, pos);
        return Some(contents);
    }

    // 2. Generic BlockInventory (e.g. chest, hopper)
    if let Some(inv) = world.get_component::<BlockInventory>(&entity) {
        let contents: Vec<sc_item::ItemStack> = inv
            .slots
            .iter()
            .filter_map(|stack| (!stack.is_empty()).then_some(stack.clone()))
            .collect();
        destroy_block_entity(world, world_id, pos);
        return Some(contents);
    }

    destroy_block_entity(world, world_id, pos);
    None
}
