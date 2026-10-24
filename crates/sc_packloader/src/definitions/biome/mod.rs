//! 生物群系定义：描述、生成器组件与 NBT 导出。
//!
//! Biome definitions: descriptions, spawner components, and NBT export.

pub mod component;
pub mod manager;

use crate::definitions::biome::component::*;
use crate::{biome_nbt_export, components_export};
use sc_ecs::component::Component;
use sc_ecs::entity::EntityId;
use sc_ecs::world::World;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::writer::NbtCustomWrite;
use sc_nbt::NbtValue;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use std::fmt::{Debug, Formatter};
use std::io;
use std::io::{Error, ErrorKind};

/// 生物群系 ECS 组件：格式版本 + 描述。
///
/// Biome ECS component: format version plus description.
#[derive(Component, Debug)]
pub struct MinecraftBiome {
    /// JSON 格式版本。
    ///
    /// JSON format version.
    pub format_version: String,
    /// 生物群系描述。
    ///
    /// Biome description.
    pub description: MinecraftBiomeDescription,
}

/// 生物群系定义数据：反序列化入口，可生成 ECS 实体或挂载组件。
///
/// Biome definition data: deserialization entry point; can spawn an ECS
/// entity or attach components to one.
#[derive(Deserialize, Clone)]
pub struct MinecraftBiomeSpawner {
    /// JSON 格式版本（不从 JSON 读取，由加载方填充）。
    ///
    /// JSON format version (not read from JSON; filled in by the loader).
    #[serde(skip_deserializing)]
    pub format_version: String,
    /// 生物群系描述。
    ///
    /// Biome description.
    pub description: MinecraftBiomeDescription,
    /// 可选的生成组件集合。
    ///
    /// Optional generation component set.
    pub components: Option<BiomeComponents>,
}

impl MinecraftBiomeSpawner {
    /// 生成一个携带生物群系组件（及生成组件）的 ECS 实体。
    ///
    /// Spawns an ECS entity carrying the biome component (and generation components).
    ///
    /// - `world`: 目标世界
    pub fn spawn(&self, world: &World) -> EntityId {
        let entity = world.spawn(MinecraftBiome {
            format_version: self.format_version.clone(),
            description: self.description.clone(),
        });
        if let Some(components) = self.components.clone() {
            components.insert(world, &entity);
        }
        entity
    }

    /// 将生物群系组件（及生成组件）挂载到已有实体。
    ///
    /// Attaches the biome component (and generation components) to an existing entity.
    ///
    /// - `world`: 目标世界
    /// - `entity`: 目标实体
    pub fn insert(&self, world: &World, entity: &EntityId) {
        world.add_component(
            entity,
            MinecraftBiome {
                format_version: self.format_version.clone(),
                description: self.description.clone(),
            },
        );
        if let Some(components) = self.components.clone() {
            components.insert(world, entity);
        }
    }
}

impl Debug for MinecraftBiomeSpawner {
    /// 按 `MinecraftBiome` 结构格式化输出（与组件形态保持一致）。
    ///
    /// Formats as the `MinecraftBiome` struct (matching the component shape).
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("MinecraftBiome");
        f.field("format_version", &self.format_version)
            .field("description", &self.description)
            .field("components", &self.components)
            .finish()
    }
}

/// 生物群系描述：标识名。
///
/// Biome description: identifier.
#[derive(Deserialize, Debug, Clone)]
pub struct MinecraftBiomeDescription {
    /// 生物群系标识名（如 `minecraft:plains`）。
    ///
    /// Biome identifier (e.g. `minecraft:plains`).
    pub identifier: String,
}

components_export![
    BiomeComponents,
    CappedSurface = "minecraft:capped_surface",
    Climate = "minecraft:climate",
    SurfaceParameters = "minecraft:surface_parameters",
    FrozenOceanSurface = "minecraft:frozen_ocean_surface",
    MesaSurface = "minecraft:mesa_surface",
    MountainParameters = "minecraft:mountain_parameters",
    MultiNoiseGenerationRules = "minecraft:multinoise_generation_rules",
    OverworldGenerationRules = "minecraft:overworld_generation_rules",
    OverworldHeight = "minecraft:overworld_height",
    SurfaceMaterialAdjustments = "minecraft:surface_material_adjustments",
    SurfaceBuilder = "minecraft:surface_builder",
    SwampSurface = "minecraft:swamp_surface",
    Tags = "minecraft:tags",
    TheEndSurface = "minecraft:the_end_surface",
];

biome_nbt_export![
    BiomeComponents,
    MultiNoiseGenerationRules,
    OverworldGenerationRules,
    Tags
];
