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

#[derive(Component, Debug)]
pub struct MinecraftBiome {
    pub format_version: String,
    pub description: MinecraftBiomeDescription,
}

#[derive(Deserialize, Clone)]
pub struct MinecraftBiomeSpawner {
    #[serde(skip_deserializing)]
    pub format_version: String,
    pub description: MinecraftBiomeDescription,
    pub components: Option<BiomeComponents>,
}

impl MinecraftBiomeSpawner {
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
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut f = f.debug_struct("MinecraftBiome");
        f.field("format_version", &self.format_version)
            .field("description", &self.description)
            .field("components", &self.components)
            .finish()
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct MinecraftBiomeDescription {
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
