use crate::definitions::biome::manager::MinecraftBiomeManager;
use crate::definitions::biome::MinecraftBiomeSpawner;
use crate::entity::MinecraftEntitySpawner;
use crate::item::MinecraftItemSpawner;
use crate::pack::pack_manager::ResourcePackManager;
use crate::pack_loader::zipped_loader::ResourcePackZippedLoader;
use crate::pack_loader::PackLoaderTrait;
use crate::version_control::loader::VersionPackLoader;
use crate::version_control::runtime::MinecraftRuntimeManager;
use chrono::Local;
use definitions::attribute::DefaultEntityAttributes;
use log::{error, info, trace, warn};
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_log::{t, t_log};
use sc_utils::schedule::SCPreLoad;
use serde::Deserialize;
use std::borrow::Cow;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter};

pub mod block;
pub mod definitions;
pub mod entity;
pub mod ident;
pub mod item;
pub mod r#macro;
pub mod pack;
pub mod pack_loader;
pub mod recipe_source;
#[cfg(test)]
mod test;
pub mod types;
pub mod version_control;

types_export![
    MinecraftJsonDeserializer,
    MinecraftJson,
    MinecraftEntitySpawner = "minecraft:entity",
    MinecraftItemSpawner = "minecraft:item",
    MinecraftBiomeSpawner = "minecraft:biome",
];

#[derive(Debug)]
pub enum PackLoaderError {
    VersionPackNotFound(Cow<'static, str>),
}

impl Display for PackLoaderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for PackLoaderError {}

pub struct SCPackLoaderPlugin;

impl Plugin for SCPackLoaderPlugin {
    fn build(&self, app: &App) {
        app.add_systems(SCPreLoad, (load_version, load_pack));
    }
}

fn load_version(world: World) {
    let start = Local::now().timestamp_millis();
    let Ok(mut current_dir) = env::current_dir() else {
        error!("{}", t_log!("console.pack.cwd_fail"));
        return;
    };
    current_dir.push("version_packs");
    let packs = {
        let loader = VersionPackLoader::new(current_dir);
        loader.get_versions()
    };
    let end = Local::now().timestamp_millis();
    info!(
        "{}",
        t_log!(
            "console.version_pack",
            count = packs.len(),
            ms = end - start
        )
    );

    let Some(mut select_version) = packs.into_iter().max_by(|left, right| {
        left.manifest
            .protocol_version
            .cmp(&right.manifest.protocol_version)
            .then_with(|| {
                left.manifest
                    .minecraft_version
                    .cmp(&right.manifest.minecraft_version)
            })
    }) else {
        error!("{}", t!("console.version_pack.not_found"));
        return;
    };
    info!(
        "{}",
        t_log!(
            "console.version_pack.load.default",
            name = &select_version.manifest.name,
            version = select_version.manifest.minecraft_version
        )
    );

    // Load biomes
    let start = Local::now().timestamp_millis();
    let biomes = select_version.take_biomes();
    let biomes_len = biomes.len();
    let mut biome_manager = MinecraftBiomeManager::from_vec(biomes);
    biome_manager.set_biome_ids(select_version.take_biome_ids());
    biome_manager.build_nbt();
    let end = Local::now().timestamp_millis();
    info!(
        "{}",
        t_log!(
            "console.version_pack.load_biomes",
            count = biomes_len,
            ms = end - start
        )
    );

    // Load attributes
    let attributes = select_version.take_attributes();
    let default_attributes = DefaultEntityAttributes::new(attributes);

    // Load behavior packs
    let start = Local::now().timestamp_millis();
    let mut packs = select_version.take_behavior_packs();
    packs.sort_by(|(version_a, _), (version_b, _)| version_a.cmp(version_b)); // Sort ascending so higher versions overwrite lower ones

    // Recipe sources: read `recipes/**/*.json` from nested behavior-pack zips (excluding `__brarchive/`),
    // keeping pack identity/path/fingerprint; kind is read from the JSON root key at compile time.
    let recipe_budgets = crate::recipe_source::RecipeSourceBudgets::default();
    let mut recipe_files = Vec::new();
    let mut recipe_errors = Vec::new();
    for (order, (_, pack)) in packs.iter().enumerate() {
        let (mut files, mut errors) =
            crate::recipe_source::read_recipes_from_resource_pack(pack, order, &recipe_budgets);
        recipe_files.append(&mut files);
        recipe_errors.append(&mut errors);
    }
    let recipe_file_count = recipe_files.len();
    let recipe_error_count = recipe_errors.len();

    // Load plugins
    let plugin_count = select_version.plugins_len();
    let end = Local::now().timestamp_millis();
    info!(
        "{}",
        t_log!(
            "console.version_pack.load_plugins",
            count = plugin_count,
            ms = end - start
        )
    );

    let mut runtime_manager = MinecraftRuntimeManager::new(select_version.take_runtime_id());
    for (version, pack) in packs {
        trace!("PackLoader >> Version Pack >> Game Version: {}", version);
        runtime_manager.push_item_map(pack.items);
        runtime_manager.push_entity_map(pack.entities);
    }

    // Load vanilla entity-identifier NBT (from the entity_identifiers.dat file)
    // Load the full vanilla entity identifier set from the resource file
    let entity_identifiers_dat = select_version.take_entity_identifiers();
    if let Err(error) = runtime_manager.load_entity_identifiers(entity_identifiers_dat) {
        error!("{}", t_log!("console.pack.entity_ids_fail", error = error));
        return;
    }

    // Load the vanilla creative-item table (from creative_items.json, Kaooot format, optional)
    if let Some(creative_items_json) = select_version.take_creative_items() {
        runtime_manager.load_creative_items(creative_items_json);
    }

    let end = Local::now().timestamp_millis();
    info!(
        "{}",
        t_log!("console.version_pack.load_pack", ms = end - start)
    );

    world
        .insert_resource(select_version)
        .insert_resource(runtime_manager)
        .insert_resource(biome_manager)
        .insert_resource(default_attributes)
        .insert_resource(crate::recipe_source::RawRecipeSources {
            files: recipe_files,
            errors: recipe_errors,
        });
    if recipe_error_count > 0 {
        warn!(
            "{}",
            t_log!(
                "console.pack.recipe_sources",
                loaded = recipe_file_count,
                skipped = recipe_error_count
            )
        );
    } else {
        info!("{}", t_log!("console.pack.recipe_files", count = recipe_file_count));
    }
}

fn load_pack0(manager: &mut ResourcePackManager, name: &str) {
    let start = Local::now().timestamp_millis();
    let Ok(mut current_dir) = env::current_dir() else {
        error!("{}", t_log!("console.pack.res_cwd_fail"));
        return;
    };
    current_dir.push(format!("{}s", name));
    let zipped_loader = ResourcePackZippedLoader::new(current_dir);
    let packs = zipped_loader.get_resource_packs();
    let count = packs.len();
    manager.extends(packs);
    let end = Local::now().timestamp_millis();
    let console_name = format!("console.{}", name);
    info!(
        "{}",
        t_log!(console_name.as_str(), count = count, ms = end - start)
    );
}

fn load_pack(world: World, mut runtime_manager: ResMut<MinecraftRuntimeManager>) {
    let mut manager = ResourcePackManager::new();
    load_pack0(&mut manager, "resource_pack"); //resource packs
    load_pack0(&mut manager, "behavior_pack"); //behavior packs

    runtime_manager.push_entity_map(manager.get_entities());
    runtime_manager.push_item_map(manager.get_items());
    // Parsed tables are merged into the runtime registry: the manager keeps only manifest + zip bytes
    // (for client download), releasing the duplicate entities/items copies.
    manager.clear_parsed_content();

    info!(
        "{}",
        t_log!(
            "console.resource_pack.entities",
            count = runtime_manager.entities_len()
        )
    );
    info!(
        "{}",
        t_log!(
            "console.resource_pack.items",
            count = runtime_manager.items_len()
        )
    );

    if let Err(error) = runtime_manager.generate_palette() {
        error!("{}", t_log!("console.pack.runtime_palette_fail", error = error));
    }
    // entity_network_nbt was already loaded in load_version via load_entity_identifiers from
    // the entity_identifiers.dat file; no manual build needed here
    world.insert_resource(manager);
}
