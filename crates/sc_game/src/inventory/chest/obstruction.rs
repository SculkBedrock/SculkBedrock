//! 箱子开启阻挡检测。
//!
//! 当箱子正上方 `(x, y + 1, z)` 为完整实心方块（如泥土、石头、木板等）时，
//! 箱盖无法打开，阻止开启界面；空气、水、玻璃、台阶、楼梯、树叶、栅栏、
//! 另一个箱子等非完整实心/透明方块不阻挡开启。
//!
//! Chest-open obstruction check.
//!
//! When the block directly above a chest at `(x, y + 1, z)` is a solid opaque
//! full cube (dirt, stone, planks, etc.), the lid cannot open and the UI is
//! blocked; air, water, glass, slabs, stairs, leaves, fences and another chest
//! — non-full or transparent blocks — do not obstruct opening.

use sc_block::position::BlockPosition;
use sc_block::registry::BlockStateRegistry;
use sc_block::block_json::BlockJsonRegistry;
use sc_ecs::world::World;
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::ChunkKey;

/// 检测指定箱子正上方是否存在实心方块阻挡。
///
/// Returns `true` when the block at `(x, y + 1, z)` is a solid opaque full cube
/// that physically obstructs the chest lid.
pub fn is_chest_obstructed(
    world: &World,
    world_id: &MinecraftWorldId,
    pos: (i32, i32, i32),
) -> bool {
    // Above-block world coordinate.
    let above_y = pos.1.saturating_add(1);
    if above_y > 320 || above_y < -64 {
        return false;
    }

    let Some(manager) = world.get_resource::<MinecraftWorldManager>() else {
        return false;
    };
    let Some(minecraft_world) = manager.get_world(world_id) else {
        return false;
    };

    let target = BlockPosition::new(pos.0, above_y, pos.2);
    let key = ChunkKey::new(
        minecraft_world.world_data.get_dimension(),
        target.chunk_position(),
    );
    let Some(column) = minecraft_world.chunk_provider.cached_chunk(key) else {
        return false;
    };

    let chunk = column.read();
    let Some(runtime_id) = chunk.block_at(target.local_x(), target.y, target.local_z()) else {
        return false;
    };

    if runtime_id.0 == 0 {
        return false;
    }

    // 1. Quick identifier/name filter via the global block dictionary.
    let dict = BlockStateDictionary::global();
    if let Some(entry) = dict.get(runtime_id.0) {
        let name = entry.name.as_str();
        if matches!(
            name,
            "minecraft:air"
                | "minecraft:water"
                | "minecraft:flowing_water"
                | "minecraft:lava"
                | "minecraft:flowing_lava"
                | "minecraft:bubble_column"
        ) {
            return false;
        }

        // Transparent or non-solid blocks (slabs, stairs, glass, leaves,
        // fences, chests, etc.) do not obstruct opening.
        if name.ends_with("_slab")
            || name.ends_with("_stairs")
            || name.ends_with("_fence")
            || name.ends_with("_fence_gate")
            || name.ends_with("_wall")
            || name.ends_with("_leaves")
            || name.contains("glass")
            || name.contains("chest")
            || name.contains("shulker_box")
            || name.contains("hopper")
            || name.contains("door")
            || name.contains("trapdoor")
            || name.contains("torch")
            || name.contains("lantern")
            || name.contains("carpet")
            || name.contains("flower")
            || name.contains("sapling")
            || name.contains("sign")
            || name.contains("banner")
            || name.contains("bed")
        {
            return false;
        }
    }

    // 2. Collision-box check: a box under 16 pixels (one block tall) is not a
    // full cube.
    if let (Some(registry), Some(snapshot_reg)) = (
        world.get_resource::<BlockStateRegistry>(),
        world.get_resource::<BlockJsonRegistry>(),
    ) {
        if let Some(snapshot) = snapshot_reg.get() {
            if let Some(dense) = registry.by_runtime_id(runtime_id) {
                if let Some(boxes) = snapshot.collision_of(dense.0) {
                    if boxes.is_empty() {
                        return false;
                    }
                    // Box format [ox, oy, oz, sx, sy, sz]; sy < 16.0 means
                    // under one block tall, so no obstruction.
                    let has_full_height = boxes.iter().any(|b| b[4] >= 16.0);
                    if !has_full_height {
                        return false;
                    }
                }
            }
        }
    }

    // All remaining cases are full solid blocks: obstructed.
    true
}
