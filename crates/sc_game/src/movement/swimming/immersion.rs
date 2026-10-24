//! 水体与流体浸润探测器。
//!
//! Fluid immersion detector for entities.
//!
//! 支持 Layer 0（主方块）与 Layer 1（流体层/含水方块 waterlogged），
//! 区分静止水、流动水与气泡柱。提供头部站立空间校验（can_stand）。

use sc_entity::motion::{Aabb, Position};
use sc_world::block_dictionary::BlockStateDictionary;
use sc_world::chunk::ChunkPosition;
use sc_world::storage::ChunkKey;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};

/// 实体在水体中的浸润程度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImmersionDepth {
    /// 实体完全在空气中，任何部位均无水。
    None,
    /// 浅水浸润（仅脚底微量触水）：只能涉水行走，无法保持或开始游泳。
    FeetOnly,
    /// 躯干/胸部浸水（半身浸润）：在水面漂浮或浅层游泳。
    Torso,
    /// 头部/眼部完全浸水（完全潜水）：完全沉浸。
    Submerged,
}

impl ImmersionDepth {
    /// 实体是否处于足够深的水中以支持游泳（躯干或完全潜水）。
    #[inline]
    pub fn can_swim(&self) -> bool {
        matches!(self, Self::Torso | Self::Submerged)
    }

    /// 实体是否已脱离可游泳水体（无水或仅脚底浅水）。
    #[inline]
    pub fn is_out_of_water(&self) -> bool {
        matches!(self, Self::None | Self::FeetOnly)
    }
}

/// 检查方块名称是否属于水体流体（静止水、流动水、气泡柱）。
#[inline]
pub fn is_water_fluid_name(name: &str) -> bool {
    matches!(
        name,
        "minecraft:water"
            | "minecraft:flowing_water"
            | "minecraft:bubble_column"
    )
}

/// 探测指定坐标的世界方块（包含 Layer 0 与 Layer 1 含水）是否为水。
pub fn is_water_block_at(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    bx: i32,
    by: i32,
    bz: i32,
) -> bool {
    let Some(minecraft_world) = world_manager.get_world(world_id) else {
        return false;
    };
    let dimension = minecraft_world.world_data.get_dimension();
    let key = ChunkKey::new(dimension, ChunkPosition::from_world(bx, bz));
    let Some(column) = minecraft_world.chunk_provider.cached_chunk(key) else {
        return false;
    };
    let chunk = column.read();
    let lx = bx.rem_euclid(16) as u8;
    let lz = bz.rem_euclid(16) as u8;

    let dict = BlockStateDictionary::global();
    let is_fluid = |id: Option<sc_world::chunk::BlockRuntimeId>| {
        id.is_some_and(|id| {
            dict.get(id.0)
                .is_some_and(|entry| is_water_fluid_name(&entry.name))
        })
    };

    let b0 = chunk.block_at_layer(0, lx, by, lz);
    let b1 = chunk.block_at_layer(1, lx, by, lz);
    is_fluid(b0) || is_fluid(b1)
}

/// 探测实体在当前位置的流体浸润深度。
///
/// - `feet`: 脚底 `y = pos.y`
/// - `torso`: 躯干 `y = pos.y + 0.4 * height`
/// - `head`: 头部 `y = pos.y + 0.8 * height`
pub fn detect_immersion(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    pos: &Position,
    height: f32,
) -> ImmersionDepth {
    let bx = pos.x.floor() as i32;
    let bz = pos.z.floor() as i32;

    let feet_y = pos.y.floor() as i32;
    let torso_y = (pos.y + 0.4 * height).floor() as i32;
    let head_y = (pos.y + 0.8 * height).floor() as i32;

    let head_in_water = is_water_block_at(world_manager, world_id, bx, head_y, bz);
    if head_in_water {
        return ImmersionDepth::Submerged;
    }

    let torso_in_water = is_water_block_at(world_manager, world_id, bx, torso_y, bz);
    if torso_in_water {
        return ImmersionDepth::Torso;
    }

    let feet_in_water = is_water_block_at(world_manager, world_id, bx, feet_y, bz);
    if feet_in_water {
        return ImmersionDepth::FeetOnly;
    }

    ImmersionDepth::None
}

/// 校验实体上方是否有足够空间完全站立（从 `pos.y + 0.6` 到 `pos.y + 1.8` 无非流体实体碰撞方块）。
pub fn has_headroom(
    world_manager: &MinecraftWorldManager,
    world_id: &MinecraftWorldId,
    pos: &Position,
    world_collides_fn: impl Fn(&MinecraftWorldManager, &MinecraftWorldId, &Aabb) -> bool,
) -> bool {
    let standing_top = Aabb {
        min_x: pos.x - 0.3,
        max_x: pos.x + 0.3,
        min_y: pos.y + 0.6,
        max_y: pos.y + 1.8,
        min_z: pos.z - 0.3,
        max_z: pos.z + 0.3,
    };
    !world_collides_fn(world_manager, world_id, &standing_top)
}
