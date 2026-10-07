//! Vanilla overworld generator (in-process Rust plugin).
//!
//! Registers [`generator::NormalGenerator`] — the [`WorldGenerator`](sc_world::storage::WorldGenerator)
//! adapter over the Normal stage chain.
//!
//! The seed is injected from `MinecraftWorldData.world_seed` (level.dat
//! `RandomSeed`); it always comes from the world, never hardcoded.
//! Generation failures return `Err` (no silent flat-world fallback).

use std::sync::Arc;
use sc_ecs::app::App;
use sc_log::t_log;
use sc_plugin::SCPlugin;
use sc_utils::world::r#type::WorldType;
use sc_world::manager::MinecraftWorldManager;

// Exports the `sc_plugin_log_bridge` symbol: the host injects a log bridge
// at load time so `log::info!`/`warn!` in this cdylib forward to the host
// logger.
sc_plugin::declare_log_bridge!();

// Vanilla world-generation building blocks (RNG / math / noise / density
// functions / materials / stage chain).
pub mod worldgen;

/// Block tables + biome surface-material mappings.
pub mod blocks_table;

/// `NormalGenerator`: [`WorldGenerator`](sc_world::storage::WorldGenerator)
/// adapter that assembles the stage chain and runs it synchronously.
pub mod generator;

// ---------------------------------------------------------------------------
// Plugin entry
// ---------------------------------------------------------------------------

pub struct OverworldPlugin;

impl SCPlugin for OverworldPlugin {
    fn new() -> Self {
        Self
    }

    fn enable(&self, app: &App) {
        // Console language follows the host (SCULK_LOCALE set by bootstrap
        // from server_properties [server] language before dlopen).
        sc_log::set_locale(
            &std::env::var("SCULK_LOCALE").unwrap_or_else(|_| "zh-CN".to_string()),
        );
        // Logs forward through the host-injected bridge
        // (`declare_log_bridge!` + loader injection); without an injected
        // bridge the logger falls back to stderr instead of dropping
        // records silently.
        log::debug!("enable() entered");

        // Palette registration: a cdylib plugin statically links its own
        // copy of the world crate, so `BlockStateDictionary::global()`
        // (a `OnceLock` static) exists once in the host and once here —
        // the host load phase only registers the host copy. Take the
        // palette bytes from the version-pack resources and register them
        // with this DLL's dictionary copy (ECS resources are visible
        // across DLLs via layout `ResourceId`).
        if let Some(pack) = app
            .world()
            .get_resource::<sc_packloader::version_control::SCVersionPack>()
        {
            match pack.block_palette_bytes() {
                Some(bytes) => {
                    let registry = sc_block::registry::BlockStateRegistry::new();
                    match registry.load_palette_from_bytes(bytes) {
                        Ok(count) => log::debug!("palette registered with {count} states (this DLL dictionary copy)"),
                        Err(e) => log::warn!("{}", t_log!("console.worldgen.palette_fail", error = e)),
                    }
                }
                None => log::warn!("{}", t_log!("console.worldgen.palette_no_pack")),
            }
        } else {
            log::warn!("{}", t_log!("console.worldgen.pack_missing"));
        }

        let Some(mut manager) = app.world().get_resource_mut::<MinecraftWorldManager>() else {
            log::warn!("{}", t_log!("console.worldgen.manager_missing"));
            return;
        };

        // Collect (world_id, world_seed): seeds come from level.dat
        // `RandomSeed` (always the world seed, never hardcoded).
        let world_infos: Vec<(sc_world::manager::MinecraftWorldId, i64)> = manager
            .get_worlds_by_type(&WorldType::Overworld)
            .iter()
            .map(|world| (world.world_id.clone(), world.world_data.world_seed))
            .collect();
        log::info!(
            "{}",
            t_log!(
                "console.worldgen.worlds",
                count = world_infos.len(),
                seeds = format!("{:?}", world_infos.iter().map(|(_, s)| *s).collect::<Vec<_>>())
            )
        );

        // Block tables + material blocks: prefer the bundle snapshot
        // (authoritative defaults) over the core palette. A snapshot
        // exists when the version pack declares block data and compiles
        // cleanly; the derived palette bytes are written into the pack
        // resources by the host, the dictionary copy registered above
        // makes them visible here, and multi-state lookups keep working.
        let snapshot = app
            .world()
            .get_resource::<sc_block::block_json::BlockJsonRegistry>()
            .and_then(|registry| registry.get());
        if let Some(snap) = snapshot.as_ref() {
            log::debug!(
                "block tables use block bundle snapshot ({} types / {} states, fingerprint={:#x})",
                snap.type_count(),
                snap.state_count(),
                snap.fingerprint
            );
        }
        let table = match snapshot.as_ref() {
            Some(snap) => blocks_table::WorldgenBlockTable::from_block_snapshot(snap),
            None => blocks_table::WorldgenBlockTable::from_core_palette(),
        };
        let material_blocks = match snapshot.as_ref() {
            Some(snap) => worldgen::material::MaterialBlocks::from_block_snapshot(snap),
            None => worldgen::material::MaterialBlocks::from_core_palette(),
        };

        let mut installed = 0;
        for (world_id, seed) in world_infos {
            // Build one NormalGenerator per world (different worlds may have
            // different seeds). The stage chain is assembled at
            // construction; `generate_chunk` runs it synchronously.
            let generator = Arc::new(generator::NormalGenerator::new_with_snapshot(
                seed,
                table.clone(),
                material_blocks.clone(),
                snapshot.clone(),
            ));
            // Stopgap spawn strategy: ignore the level.dat spawn (the area
            // around (0,0) is often ocean under current worldgen) and use a
            // coarse noise scan to find an inland spawn on the largest
            // landmass. Only the in-memory world data is overwritten;
            // level.dat is left untouched.
            let land_spawn =
                generator::find_land_spawn(&generator, &material_blocks, table.air, table.water);
            if manager.set_world_generator(&world_id, generator) {
                installed += 1;
                match land_spawn {
                    Some((x, y, z)) => {
                        if let Some(world) = manager.get_world_mut(&world_id) {
                            world.world_data.spawn_x = x;
                            world.world_data.spawn_y = y;
                            world.world_data.spawn_z = z;
                        }
                        log::info!(
                            "{}",
                            t_log!("console.worldgen.spawn_found", x = x, y = y, z = z)
                        );
                    }
                    None => log::warn!("{}", t_log!("console.worldgen.spawn_search_fail")),
                }
            }
        }
        log::info!(
            "{}",
            t_log!("console.worldgen.installed", count = installed)
        );
    }

    fn disable(&self, _app: &App) {
        log::info!("{}", t_log!("console.worldgen.uninstalled"));
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn get_plugin() -> Box<dyn SCPlugin> {
    Box::new(OverworldPlugin::new())
}
