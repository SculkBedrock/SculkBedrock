//! 世界句柄：id/名称/路径/世界数据 + 区块提供器，以及出生点求解。
//!
//! World handle: id/name/path/world data plus a chunk provider, and spawn
//! point resolution.

use crate::chunk::{dimension_bounds, ChunkPosition};
use crate::manager::MinecraftWorldId;
use crate::storage::{ChunkColumn, ChunkKey, WorldChunkProvider, WorldStorageError};
use sc_log::t_log;
use sc_utils::game::structs::position::MinecraftPosition;
use sc_utils::world::data::MinecraftWorldData;
use std::path::PathBuf;
use std::sync::Arc;

/// 一个已加载的 Minecraft 世界。
///
/// One loaded Minecraft world.
#[derive(Clone)]
pub struct MinecraftWorld {
    /// 世界 id（管理器分配）。
    ///
    /// World id (assigned by the manager).
    pub world_id: MinecraftWorldId,
    /// 世界名（来自 `levelname.txt`）。
    ///
    /// World name (from `levelname.txt`).
    pub world_name: String,
    /// 世界目录路径。
    ///
    /// World directory path.
    pub world_path: PathBuf,
    /// 世界数据（`level.dat` 解析结果）。
    ///
    /// World data (parsed from `level.dat`).
    pub world_data: MinecraftWorldData,
    /// 区块提供器（存储 + 生成器 + 缓存）。
    ///
    /// Chunk provider (storage + generator + cache).
    pub chunk_provider: WorldChunkProvider,
}

impl MinecraftWorld {
    /// 由各部件构造世界。
    ///
    /// Builds a world from its parts.
    ///
    /// - `world_id`: 世界 id
    /// - `world_name`: 世界名
    /// - `world_path`: 世界目录路径
    /// - `world_data`: 世界数据
    /// - `chunk_provider`: 区块提供器
    pub fn new(
        world_id: MinecraftWorldId,
        world_name: String,
        world_path: PathBuf,
        world_data: MinecraftWorldData,
        chunk_provider: WorldChunkProvider,
    ) -> Self {
        Self {
            world_id,
            world_name,
            world_path,
            world_data,
            chunk_provider,
        }
    }

    /// 求一个安全的出生点位置。
    ///
    /// Resolves a safe spawn position.
    ///
    /// 优先级：`level.dat` 中已校准的合法 spawnY → 出生列最高方块上方 →
    /// 邻近区块螺旋搜索（半径 4、步长 16）找最近陆地；都不行退回原值。
    /// 出生 Y 已设置且合法（`< max_y`）时立即返回（启动期已校准或读自
    /// 存档；避免登录时同步加载区块）。`level.dat` 中 spawnY=32767 是
    /// 「未设置」哨兵，`sanitized_spawn` 会把它夹到世界顶部；原版语义
    /// 要求落到地表：扫描出生列最高非空气方块。出生列全水/全空（海洋）
    /// 时螺旋搜索附近区块找最近陆地。
    ///
    /// Priority order: a calibrated legal spawnY from `level.dat`, then the
    /// surface of the spawn column, then a spiral search of nearby chunks
    /// (radius 4, step 16) for the nearest land; falls back to the raw value.
    /// A set and legal spawn Y (`< max_y`) returns immediately (calibrated
    /// during startup or read from save; avoid synchronous chunk loading
    /// during login). spawnY=32767 in level.dat is the "spawn not set"
    /// sentinel; sanitized_spawn clamps it to the world top. Vanilla
    /// semantics land on the surface: scan the spawn column for the highest
    /// non-air block. The spawn column being all water/all empty (ocean):
    /// spiral-search nearby chunks for the nearest land (step by 16 blocks
    /// to cover chunks, bounded to radius 4).
    pub fn get_safe_spawn_position(&self) -> MinecraftPosition {
        let (x, y, z) = self.world_data.sanitized_spawn();
        let (_, max_y) = self.vertical_bounds();
        // If spawn Y is already set and valid (< max_y), return immediately
        // (calibrated during startup or read from save; avoid synchronous chunk loading during login).
        if y < max_y {
            return MinecraftPosition::new(x as f32, y as f32, z as f32);
        }

        // SpawnY=32767 in level.dat is the "spawn not set" sentinel;
        // sanitized_spawn clamps it to the world top. Vanilla semantics land on
        // the surface: scan the spawn column for the highest non-air block.
        if let Some(top) = self.surface_y(x, z) {
            return MinecraftPosition::new(x as f32, (top + 1) as f32, z as f32);
        }

        // Spawn column is all water/all empty (ocean): spiral-search nearby chunks
        // for the nearest land (step by 16 blocks to cover chunks, bounded to radius 4).
        for radius in 1i32..=4 {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() != radius && dz.abs() != radius {
                        continue;
                    }
                    let wx = x + dx * 16;
                    let wz = z + dz * 16;
                    if let Some(top) = self.surface_y(wx, wz) {
                        log::info!(
                            "{}",
                            t_log!(
                                "console.world.spawn_relocated",
                                x = wx,
                                y = wz,
                                top = top + 1,
                                ox = x,
                                oy = z
                            )
                        );
                        return MinecraftPosition::new(
                            wx as f32,
                            (top + 1) as f32,
                            wz as f32,
                        );
                    }
                }
            }
        }
        MinecraftPosition::new(x as f32, y as f32, z as f32)
    }

    /// 出生列的地表 y（最高非空气方块）。区块未生成或全空时返回 `None`。
    ///
    /// Surface y of the spawn column (highest non-air block). Returns None when the chunk is ungenerated or fully empty.
    ///
    /// 自顶向下扫描：跳过空气与水面，取第一块陆地（非空气非水）方块。
    ///
    /// Scans top-down: skips air and water surface, takes the first land
    /// (non-air, non-water) block.
    ///
    /// - `x` / `z`: 世界坐标
    fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        let column = match self.load_chunk(ChunkPosition::from_world(x, z)) {
            Ok(Some(col)) => col,
            Ok(None) => return None,
            Err(e) => {
                log::warn!("surface_y: load_chunk failed for ({x}, {z}): {e}");
                return None;
            }
        };
        let chunk = column.read();
        let air = crate::chunk::BlockRuntimeId(crate::block_dictionary::air_runtime_id());
        let water = crate::chunk::BlockRuntimeId(
            crate::block_dictionary::BlockStateDictionary::global()
                .first_hash_of("minecraft:water")
                .unwrap_or(air.0),
        );
        // Scans top-down: skips air and water surface, takes the first land (non-air, non-water) block.
        let mut top =
            chunk.highest_block_at(x.rem_euclid(16) as u8, z.rem_euclid(16) as u8, air)?;
        while top > chunk.min_y {
            let id =
                chunk.block_at_layer(0, x.rem_euclid(16) as u8, top, z.rem_euclid(16) as u8)?;
            if id != water {
                return Some(top);
            }
            top -= 1;
        }
        None
    }

    /// 该世界的竖直边界（按维度）。
    ///
    /// The world's vertical bounds (per dimension).
    pub fn vertical_bounds(&self) -> (i32, i32) {
        dimension_bounds(self.world_data.get_dimension())
    }

    /// 加载一个区块（经区块提供器，带缓存/生成）。
    ///
    /// Loads a chunk through the chunk provider (with cache/generation).
    ///
    /// - `position`: 区块列坐标
    ///
    /// 区块不存在返回 `Ok(None)`；存储/生成失败返回错误。
    ///
    /// Returns `Ok(None)` when the chunk does not exist; a storage/generation
    /// error otherwise.
    pub fn load_chunk(
        &self,
        position: ChunkPosition,
    ) -> Result<Option<Arc<ChunkColumn>>, WorldStorageError> {
        let (min_y, max_y) = self.vertical_bounds();
        self.chunk_provider.load_chunk(
            ChunkKey::new(self.world_data.get_dimension(), position),
            min_y,
            max_y,
        )
    }

    /// 加载一个区块；缺失时创建并缓存一个全空空气区块。
    ///
    /// Loads a chunk; creates and caches an all-air empty chunk when missing.
    ///
    /// - `position`: 区块列坐标
    pub fn ensure_chunk(
        &self,
        position: ChunkPosition,
    ) -> Result<Arc<ChunkColumn>, WorldStorageError> {
        let (min_y, max_y) = self.vertical_bounds();
        self.chunk_provider.ensure_chunk(
            ChunkKey::new(self.world_data.get_dimension(), position),
            min_y,
            max_y,
        )
    }
}
