//! 双箱合并与配对规则。
//!
//! 规则：
//! 1. 只有同朝向（`minecraft:cardinal_direction`）、同为可合并（`pairable == true`）的箱子可合并；
//! 2. 合并仅在与面向方向垂直的水平轴（左右两侧）发生；
//! 3. 规范排序（左/右）：
//!    - 面向北（North）：西侧（-X）为左（槽位 0..27），东侧（+X）为右（槽位 27..54）；
//!    - 面向南（South）：东侧（+X）为左（槽位 0..27），西侧（-X）为右（槽位 27..54）；
//!    - 面向西（West）：南侧（+Z）为左（槽位 0..27），北侧（-Z）为右（槽位 27..54）；
//!    - 面向东（East）：北侧（-Z）为左（槽位 0..27），南侧（+Z）为右（槽位 27..54）。
//!
//! Double-chest pairing rules.
//!
//! 1. Only chests with the same facing (`minecraft:cardinal_direction`) and
//!    `pairable == true` may merge;
//! 2. Merging happens only on the horizontal axis perpendicular to the facing
//!    (the left and right sides);
//! 3. Canonical (left/right) ordering:
//!    - facing North: west (-X) is left (slots 0..27), east (+X) is right (27..54);
//!    - facing South: east (+X) is left (slots 0..27), west (-X) is right (27..54);
//!    - facing West: south (+Z) is left (slots 0..27), north (-Z) is right (27..54);
//!    - facing East: north (-Z) is left (slots 0..27), south (+Z) is right (27..54).

use sc_block::position::BlockPosition;
use sc_block::registry::BlockStateRegistry;
use sc_ecs::world::World;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::ChunkKey;

/// 查询指定位置方块的水平朝向（"north", "south", "east", "west"）。
///
/// Reads the horizontal facing of the block at a position
/// ("north", "south", "east", "west").
pub fn chest_facing_at(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<&'static str> {
    let manager = world.get_resource::<MinecraftWorldManager>()?;
    let minecraft_world = manager.get_world(world_id)?;
    let target = BlockPosition::new(pos.0, pos.1, pos.2);
    let key = ChunkKey::new(
        minecraft_world.world_data.get_dimension(),
        target.chunk_position(),
    );
    let column = minecraft_world.chunk_provider.cached_chunk(key)?;
    let chunk = column.read();
    let runtime_id = chunk.block_at(target.local_x(), target.y, target.local_z())?;

    let entry = if let Some(registry) = world.get_resource::<BlockStateRegistry>() {
        registry.describe(runtime_id)
    } else {
        None
    }
    .or_else(|| BlockStateDictionary::global().get(runtime_id.0))?;

    let states = entry.states.as_ref()?;
    if let Some(sc_nbt::NbtValue::String(s)) = states.get("minecraft:cardinal_direction") {
        match s.as_str() {
            "north" => return Some("north"),
            "south" => return Some("south"),
            "east" => return Some("east"),
            "west" => return Some("west"),
            _ => {}
        }
    }
    if let Some(val) = states.get("facing_direction") {
        let dir = match val {
            sc_nbt::NbtValue::Int(i) => *i,
            sc_nbt::NbtValue::Byte(b) => *b as i32,
            _ => -1,
        };
        match dir {
            2 => return Some("north"),
            3 => return Some("south"),
            4 => return Some("west"),
            5 => return Some("east"),
            _ => {}
        }
    }
    Some("north")
}

/// 计算面向垂直方向的左右两侧邻居坐标 `(left_neighbor, right_neighbor)`。
///
/// Computes the `(left_neighbor, right_neighbor)` coordinates perpendicular to
/// the facing.
pub fn perpendicular_neighbors(
    facing: &str,
    pos: (i32, i32, i32),
) -> ((i32, i32, i32), (i32, i32, i32)) {
    match facing {
        "north" => ((pos.0 - 1, pos.1, pos.2), (pos.0 + 1, pos.1, pos.2)),
        "south" => ((pos.0 + 1, pos.1, pos.2), (pos.0 - 1, pos.1, pos.2)),
        "west" => ((pos.0, pos.1, pos.2 + 1), (pos.0, pos.1, pos.2 - 1)),
        "east" => ((pos.0, pos.1, pos.2 - 1), (pos.0, pos.1, pos.2 + 1)),
        _ => ((pos.0 - 1, pos.1, pos.2), (pos.0 + 1, pos.1, pos.2)),
    }
}

/// 查找能与当前箱子合并的相邻箱子坐标。
///
/// 若当前箱子不支持 pairable，或两侧均无有效配对箱子，返回 `None`。
///
/// Finds the adjacent chest that can merge with this one. Returns `None` when
/// the chest is not pairable or neither side holds a valid partner.
pub fn find_paired_chest(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> Option<(i32, i32, i32)> {
    let decl = crate::inventory::crafting::chest_decl_at(world, world_id, pos)?;
    if !decl.pairable {
        return None;
    }

    let facing = chest_facing_at(world, world_id, pos).unwrap_or("north");
    let (left_pos, right_pos) = perpendicular_neighbors(facing, pos);

    let is_valid_pair = |candidate: (i32, i32, i32)| -> bool {
        let Some(c_decl) = crate::inventory::crafting::chest_decl_at(world, world_id, candidate) else {
            return false;
        };
        if !c_decl.pairable {
            return false;
        }
        if chest_facing_at(world, world_id, candidate) != Some(facing) {
            return false;
        }
        // Prevent triple chests: check whether an aligned chest already sits
        // on the candidate's outer side.
        let (c_left, c_right) = perpendicular_neighbors(facing, candidate);
        let outer = if c_left == pos { c_right } else { c_left };
        if let Some(outer_decl) = crate::inventory::crafting::chest_decl_at(world, world_id, outer) {
            if outer_decl.pairable && chest_facing_at(world, world_id, outer) == Some(facing) {
                return false;
            }
        }
        true
    };

    if is_valid_pair(left_pos) {
        return Some(left_pos);
    }
    if is_valid_pair(right_pos) {
        return Some(right_pos);
    }

    None
}

/// 确定两个已配对箱子的规范顺序 `(left_pos, right_pos)`。
///
/// `left_pos` 对应合并大箱子的前半部分（槽位 0..27），
/// `right_pos` 对应后半部分（槽位 27..54）。
///
/// Determines the canonical `(left_pos, right_pos)` order for a paired chest.
/// `left_pos` is the first half of the merged chest (slots 0..27), `right_pos`
/// the second half (slots 27..54).
pub fn canonical_chest_order(
    facing: &str,
    pos_a: (i32, i32, i32),
    pos_b: (i32, i32, i32),
) -> ((i32, i32, i32), (i32, i32, i32)) {
    match facing {
        "north" => {
            if pos_a.0 <= pos_b.0 {
                (pos_a, pos_b)
            } else {
                (pos_b, pos_a)
            }
        }
        "south" => {
            if pos_a.0 >= pos_b.0 {
                (pos_a, pos_b)
            } else {
                (pos_b, pos_a)
            }
        }
        "west" => {
            if pos_a.2 >= pos_b.2 {
                (pos_a, pos_b)
            } else {
                (pos_b, pos_a)
            }
        }
        "east" => {
            if pos_a.2 <= pos_b.2 {
                (pos_a, pos_b)
            } else {
                (pos_b, pos_a)
            }
        }
        _ => (pos_a.min(pos_b), pos_a.max(pos_b)),
    }
}
