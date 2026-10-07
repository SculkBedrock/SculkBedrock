use crate::chunk::{dimension_bounds, ChunkPosition};
use crate::manager::MinecraftWorldId;
use crate::storage::{ChunkColumn, ChunkKey, WorldChunkProvider, WorldStorageError};
use sc_log::t_log;
use sc_utils::game::structs::position::MinecraftPosition;
use sc_utils::world::data::MinecraftWorldData;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct MinecraftWorld {
    pub world_id: MinecraftWorldId,
    pub world_name: String,
    pub world_path: PathBuf,
    pub world_data: MinecraftWorldData,
    pub chunk_provider: WorldChunkProvider,
}

impl MinecraftWorld {
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

    pub fn get_safe_spawn_position(&self) -> MinecraftPosition {
        let (x, y, z) = self.world_data.sanitized_spawn();
        let (_, max_y) = self.vertical_bounds();
        // SpawnY=32767 in level.dat is the "spawn not set" sentinel;
        // sanitized_spawn clamps it to the world top. Vanilla semantics land on
        // the surface: scan the spawn column for the highest non-air block.
        let y = if y >= max_y {
            self.surface_y(x, z).map(|top| top + 1).unwrap_or(y)
        } else {
            y
        };
        // Spawn column is all water/all empty (ocean): spiral-search the spawn
        // radius for the nearest land.
        if self.surface_y(x, z).is_none() {
            for radius in 1i32..=16 {
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx.abs() != radius && dz.abs() != radius {
                            continue;
                        }
                        if let Some(top) = self.surface_y(x + dx, z + dz) {
                            log::info!(
                                "{}",
                                t_log!(
                                    "console.world.spawn_relocated",
                                    x = x + dx,
                                    y = z + dz,
                                    top = top + 1,
                                    ox = x,
                                    oy = z
                                )
                            );
                            return MinecraftPosition::new(
                                (x + dx) as f32,
                                (top + 1) as f32,
                                (z + dz) as f32,
                            );
                        }
                    }
                }
            }
        }
        MinecraftPosition::new(x as f32, y as f32, z as f32)
    }

    /// Surface y of the spawn column (highest non-air block). Returns None when the chunk is ungenerated or fully empty.
    fn surface_y(&self, x: i32, z: i32) -> Option<i32> {
        let column = self.load_chunk(ChunkPosition::from_world(x, z)).ok()??;
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

    pub fn vertical_bounds(&self) -> (i32, i32) {
        dimension_bounds(self.world_data.get_dimension())
    }

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

    /// Loads a chunk; creates and caches an all-air empty chunk when missing.
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
