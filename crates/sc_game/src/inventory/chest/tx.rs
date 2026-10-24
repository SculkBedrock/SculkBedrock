//! 箱子槽位事务与持久化处理。
//!
//! 处理箱子物品的读取、写入、Move、Swap 操作，并在修改时触发
//! `BlockInventory` 的 ECS 脏标记，以便保存到 LevelDB 存储。
//!
//! Chest slot transactions and persistence.
//!
//! Handles reading, writing, Move and Swap operations on chest items and sets
//! the ECS dirty flag on `BlockInventory` when it changes, so edits reach the
//! LevelDB store.

use sc_ecs::world::World;
use sc_item::{ItemRegistry, ItemStack};
use sc_world::manager::MinecraftWorldId;

use crate::block_entity::{
    ensure_block_entity, BlockEntityIndex, BlockEntityKind, BlockInventory,
};
use crate::inventory::craft_inventory::{CraftContainer, CraftSlotRef};

/// 判断槽位引用是否属于箱子容器。
///
/// Whether the slot reference belongs to a chest container.
pub fn is_chest_slot(reference: &CraftSlotRef) -> bool {
    reference.container == CraftContainer::Chest
}

/// 校验并获取箱子本地槽位索引。
///
/// Validates a chest slot reference and returns its local slot index.
pub fn chest_slot_index(
    reference: &CraftSlotRef,
    window_id: u8,
    total_slots: u16,
    slots: &[ItemStack],
) -> Option<usize> {
    let index = reference.slot as usize;
    if index >= total_slots as usize || index >= slots.len() {
        return None;
    }
    // When dynamic_slot is present it must match the open window_id.
    if reference.dynamic.is_some_and(|d| d != window_id as u32) {
        return None;
    }
    // When the client carries a network id (> 0), the server slot must hold an item.
    if reference.net_id > 0 && slots[index].is_empty() {
        return None;
    }
    Some(index)
}

/// 读取单个方块实体的 `BlockInventory` 槽位，若不存在则补齐为对应槽位数。
///
/// Reads a single block entity's `BlockInventory` slots, padding to the
/// requested slot count when the entity or inventory is missing.
fn read_single_chest_slots(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
    capacity: usize,
) -> Vec<ItemStack> {
    ensure_block_entity(world, world_id.clone(), pos, BlockEntityKind::Chest);
    let entity = world
        .get_resource::<BlockEntityIndex>()
        .and_then(|index| index.get(world_id, pos));

    let Some(entity) = entity else {
        return vec![ItemStack::empty(); capacity];
    };

    let Some(inv) = world.get_component::<BlockInventory>(&entity) else {
        return vec![ItemStack::empty(); capacity];
    };

    let mut slots = inv.slots.clone();
    if slots.len() < capacity {
        slots.resize(capacity, ItemStack::empty());
    } else if slots.len() > capacity {
        slots.truncate(capacity);
    }
    slots
}

/// 载入（单个或双箱合并）箱子的全部槽位数据。
///
/// 单箱返回 27 格，双箱返回 54 格。
///
/// Loads all slot data for a single or paired (double) chest: 27 slots for a
/// single chest, 54 for a double chest.
pub fn load_chest_slots(
    world: &World,
    world_id: &MinecraftWorldId,
    left_pos: (i32, i32, i32),
    right_pos: Option<(i32, i32, i32)>,
) -> Vec<ItemStack> {
    let mut slots = read_single_chest_slots(world, world_id, left_pos, 27);
    if let Some(right) = right_pos {
        let right_slots = read_single_chest_slots(world, world_id, right, 27);
        slots.extend(right_slots);
    }
    slots
}

/// 将槽位修改提交回 ECS `BlockInventory`，并标记脏以便磁盘保存。
///
/// Commits slot edits back into the ECS `BlockInventory` and marks it dirty so
/// they are persisted to disk.
pub fn commit_chest_slots(
    world: &World,
    world_id: &MinecraftWorldId,
    left_pos: (i32, i32, i32),
    right_pos: Option<(i32, i32, i32)>,
    slots: &[ItemStack],
) {
    let commit_one = |pos: (i32, i32, i32), slice: &[ItemStack]| {
        let entity = ensure_block_entity(world, world_id.clone(), pos, BlockEntityKind::Chest);
        let Some(entity) = entity else {
            return;
        };
        let max_stack = world
            .get_component::<BlockInventory>(&entity)
            .map(|inv| inv.max_stack_size)
            .unwrap_or(64);
        world.add_component(
            &entity,
            BlockInventory {
                slots: slice.to_vec(),
                max_stack_size: max_stack,
            },
        );
        use crate::block_entity::BlockEntityWorldExt;
        world.mark_block_entity_dirty(&entity, true, false);
    };

    let left_len = 27.min(slots.len());
    commit_one(left_pos, &slots[..left_len]);

    if let Some(right) = right_pos {
        if slots.len() > 27 {
            let right_len = 54.min(slots.len());
            commit_one(right, &slots[27..right_len]);
        }
    }
}

fn same_item(a: &ItemStack, b: &ItemStack) -> bool {
    a.runtime_id == b.runtime_id
        && a.damage == b.damage
        && (a.block_runtime_id == b.block_runtime_id
            || a.block_runtime_id == 0
            || b.block_runtime_id == 0)
}

/// 执行涉及箱子槽位的 Move 操作。
///
/// Performs a Move operation involving chest slots.
#[allow(clippy::too_many_arguments)]
pub fn chest_move(
    valid_player: &dyn Fn(CraftSlotRef, usize, &[ItemStack]) -> bool,
    items: &ItemRegistry,
    main: usize,
    table: bool,
    window_id: u8,
    total_slots: u16,
    stacks: &mut [ItemStack],
    chest_slots: &mut [ItemStack],
    amount: u8,
    source: &CraftSlotRef,
    destination: &CraftSlotRef,
    op_id: u64,
) -> bool {
    enum Side {
        Player(usize),
        Chest(usize),
    }

    let src = if is_chest_slot(source) {
        chest_slot_index(source, window_id, total_slots, chest_slots).map(Side::Chest)
    } else {
        crate::inventory::craft_inventory::index(*source, main, table).map(Side::Player)
    };
    let dst = if is_chest_slot(destination) {
        chest_slot_index(destination, window_id, total_slots, chest_slots).map(Side::Chest)
    } else {
        crate::inventory::craft_inventory::index(*destination, main, table).map(Side::Player)
    };

    let (Some(src), Some(dst)) = (src, dst) else {
        log::debug!("[chest] op={op_id} Move rejected: invalid endpoint (src={source:?}, dst={destination:?})");
        return false;
    };

    let (src_stack, dst_stack) = match (&src, &dst) {
        (Side::Player(i), Side::Player(j)) => (&stacks[*i], &stacks[*j]),
        (Side::Player(i), Side::Chest(d)) => (&stacks[*i], &chest_slots[*d]),
        (Side::Chest(s), Side::Player(j)) => (&chest_slots[*s], &stacks[*j]),
        (Side::Chest(s), Side::Chest(d)) => {
            if s == d {
                log::debug!("[chest] op={op_id} Move rejected: same slot");
                return false;
            }
            (&chest_slots[*s], &chest_slots[*d])
        }
    };

    if let Side::Player(i) = src {
        if !valid_player(*source, i, stacks) {
            log::debug!("[chest] op={op_id} Move rejected: source net_id check failed");
            return false;
        }
    }
    if let Side::Player(j) = dst {
        if !valid_player(*destination, j, stacks) {
            log::debug!("[chest] op={op_id} Move rejected: destination net_id check failed");
            return false;
        }
    }

    if amount == 0 || src_stack.count < amount as u16 {
        log::debug!("[chest] op={op_id} Move rejected: illegal amount");
        return false;
    }
    if !dst_stack.is_empty() && !same_item(src_stack, dst_stack) {
        log::debug!("[chest] op={op_id} Move rejected: item mismatch");
        return false;
    }
    if dst_stack.count.saturating_add(amount as u16) > items.max_stack_size(src_stack.runtime_id) {
        log::debug!("[chest] op={op_id} Move rejected: over max stack size");
        return false;
    }

    let moving = ItemStack {
        count: 0,
        ..src_stack.clone()
    };

    match (&src, &dst) {
        (Side::Player(i), Side::Player(j)) => {
            if stacks[*j].is_empty() {
                stacks[*j] = moving;
            }
            stacks[*j].count += amount as u16;
            stacks[*i].count -= amount as u16;
            stacks[*i].clear_if_empty();
        }
        (Side::Player(i), Side::Chest(d)) => {
            if chest_slots[*d].is_empty() {
                chest_slots[*d] = moving;
            }
            chest_slots[*d].count += amount as u16;
            stacks[*i].count -= amount as u16;
            stacks[*i].clear_if_empty();
        }
        (Side::Chest(s), Side::Player(j)) => {
            if stacks[*j].is_empty() {
                stacks[*j] = moving;
            }
            stacks[*j].count += amount as u16;
            chest_slots[*s].count -= amount as u16;
            chest_slots[*s].clear_if_empty();
        }
        (Side::Chest(s), Side::Chest(d)) => {
            if chest_slots[*d].is_empty() {
                chest_slots[*d] = moving;
            }
            chest_slots[*d].count += amount as u16;
            chest_slots[*s].count -= amount as u16;
            chest_slots[*s].clear_if_empty();
        }
    }

    true
}

/// 执行涉及箱子槽位的 Swap 操作。
///
/// Performs a Swap operation involving chest slots.
#[allow(clippy::too_many_arguments)]
pub fn chest_swap(
    valid_player: &dyn Fn(CraftSlotRef, usize, &[ItemStack]) -> bool,
    _items: &ItemRegistry,
    main: usize,
    table: bool,
    window_id: u8,
    total_slots: u16,
    stacks: &mut [ItemStack],
    chest_slots: &mut [ItemStack],
    source: &CraftSlotRef,
    destination: &CraftSlotRef,
    op_id: u64,
) -> bool {
    let resolve = |reference: &CraftSlotRef| {
        if is_chest_slot(reference) {
            chest_slot_index(reference, window_id, total_slots, chest_slots)
                .map(|index| (true, index))
        } else {
            crate::inventory::craft_inventory::index(*reference, main, table)
                .map(|index| (false, index))
        }
    };

    let (Some((src_chest, src)), Some((dst_chest, dst))) =
        (resolve(source), resolve(destination))
    else {
        log::debug!("[chest] op={op_id} Swap rejected: bad endpoint");
        return false;
    };

    let incoming_src = if src_chest {
        chest_slots[src].clone()
    } else {
        if !valid_player(*source, src, stacks) {
            return false;
        }
        stacks[src].clone()
    };
    let incoming_dst = if dst_chest {
        chest_slots[dst].clone()
    } else {
        if !valid_player(*destination, dst, stacks) {
            return false;
        }
        stacks[dst].clone()
    };

    match (src_chest, dst_chest) {
        (false, false) => {
            stacks[src] = incoming_dst;
            stacks[dst] = incoming_src;
        }
        (false, true) => {
            stacks[src] = incoming_dst;
            chest_slots[dst] = incoming_src;
        }
        (true, false) => {
            chest_slots[src] = incoming_dst;
            stacks[dst] = incoming_src;
        }
        (true, true) => {
            chest_slots[src] = incoming_dst;
            chest_slots[dst] = incoming_src;
        }
    }

    true
}
