use std::env;
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

/// Set on shutdown signals (Ctrl+C/terminal close/kill); the main loop triggers graceful exit after checking it.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Signal handler: only sets the atomic flag (async-signal-safe; remaining logic runs in the main loop).
extern "C" fn handle_shutdown_signal(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}
use chrono::Local;
use log::{error, info, warn, LevelFilter};
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::params::resource::{Res, ResMut};
use sc_ecs::resource::Resource;
use sc_ecs::schedule::{PostStartup, PreStartup};
use sc_ecs::world::World;
use sc_log::t_log;
use sc_nbt::compound::CompoundNbt;
use sc_nbt::NbtValue;
use sc_packloader::version_control::SCVersionPack;
use sc_plugin::loader::{LoadedHostPlugin, SCPluginLoader};
use sc_plugin::manager::SCPluginManager;
use sc_plugin_api::{ChunkGenRequest, GenerateFn, HostApiV1};
use sc_utils::game::structs::server::Server;
use sc_utils::game::structs::server_properties::ServerProperties;
use sc_utils::schedule::{SCLoad, SCPostLoad, SCStartup};
use sc_utils::tempdir::SCTempDir;
use sc_utils::world::r#type::WorldType;
use sc_world::chunk::{BlockRuntimeId, Chunk};
use sc_world::leveldb::block_hash::block_state_hash;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};
use sc_world::storage::{ChunkGenerationRequest, WorldGenerator, WorldStorageError};

#[derive(Resource, Default)]
pub struct StartupTimestamp {
    start: i64,
    end: i64,
}

/// 启动必需的工作目录布局（相对 cwd；`logs/` 由 sc_log 落盘时自建，
/// `diagnostics/` 由各 dump 点按需自建，这里只保证启动期就会读取的目录）。
const REQUIRED_DIRS: [&str; 5] = [
    "version_packs",
    "worlds",
    "resource_packs",
    "behavior_packs",
    "plugins",
];

/// 缺失 `server_properties.toml` 时写入的默认配置。
///
/// 必须与 [`ServerProperties::parse`] 的必填键保持一致（[game]/[server]/
/// [experimental_gameplay]/[world] 全必填，缺键即启动失败），改解析器时
/// 同步改这里（`default_config_parses` 回归锁定）。
const DEFAULT_SERVER_PROPERTIES: &str = r#"[server]
enable_snappy = false
xbox_auth = false
validate_encryption = true
force_gamemode = false
enable_version_pack_plugins = true
language = "zh-CN"
enable_hitokoto = false

[game]
name = "SculkBedrock"
motd = "SculkBedrock Server"
max_player = 20
gamemode = "survival"
ipv4_port = 19132
ipv6_port = 19132
demo_systems = false

[experimental_gameplay]
data_driven_items = true
data_driven_biomes = true
upcoming_creator_features = true
gametest = true
experimental_molang_features = true
cameras = true

[world]
overworld_name = "OverWorld"
the_nether_name = "TheNether"
the_end_name = "TheEnd"
seed = "random"
generator = "default"
achievements_disable = false
hide_seed = true

[chunk]
view_distance = 10
chunks_per_tick = 4
spawn_threshold = 56

[log]
file_name_format = "sc_log_%Y-%m-%d_%H-%M-%S.log"
flush_interval_ms = 200

[debug]
blocked_packets = []
"#;

/// 确保运行目录布局存在：建缺失目录；缺失主配置时写默认文件。
///
/// 幂等、无害：已存在的一律不动（不覆盖用户配置）。在 `build_app()` 头部
/// 调用一次——早于 console 桥 `build` 期读配置和 PreStartup
/// `init_properties`，空目录首次启动也能进到正常报错（缺世界/缺版本包），
/// 而不是死在读不到路径上。
pub fn ensure_runtime_layout() {
    let Ok(base) = env::current_dir() else {
        eprintln!("bootstrap: failed to resolve current directory");
        return;
    };
    for dir in REQUIRED_DIRS {
        let path = base.join(dir);
        if let Err(error) = std::fs::create_dir_all(&path) {
            eprintln!(
                "bootstrap: failed to create {}: {error}",
                path.display()
            );
            log::error!(
                "{}",
                t_log!(
                    "console.bootstrap.dir_create_fail",
                    path = path.display(),
                    error = error
                )
            );
        }
    }
    let config = base.join("server_properties.toml");
    if !config.exists() {
        match std::fs::write(&config, DEFAULT_SERVER_PROPERTIES) {
            Ok(()) => {
                eprintln!(
                    "bootstrap: created default {}",
                    config.display()
                );
                log::info!(
                    "{}",
                    t_log!(
                        "console.bootstrap.config_created",
                        path = config.display()
                    )
                );
            }
            Err(error) => {
                eprintln!(
                    "bootstrap: failed to create {}: {error}",
                    config.display()
                );
                log::error!(
                    "{}",
                    t_log!(
                        "console.bootstrap.config_create_fail",
                        path = config.display(),
                        error = error
                    )
                );
            }
        }
    }
}

pub struct SCBootStrapPlugin;

impl Plugin for SCBootStrapPlugin {
    fn build(&self, app: &App) {
        let app = match SCTempDir::new() {
            Ok(temp_dir) => app.insert_resource(temp_dir),
            Err(error) => {
                eprintln!("bootstrap: failed to create plugin temp directory: {error}");
                log::error!(
                    "{}",
                    t_log!("console.bootstrap.plugin_tmp_fail", error = error)
                );
                app
            }
        };
        app.insert_resource(SCPluginManager::new(app.clone()))
            .insert_resource(HostPlugins::default())
            .insert_resource(sc_packloader::item::ItemComponentTable::default())
            // SCExit event registration (read by region-thread shutdown after the stop command).
            .add_event::<sc_utils::event::SCExit>()
            .add_systems(PreStartup, (init_log, init_properties))
            .add_systems(SCStartup, startup)
            .add_systems(SCLoad, load_block_bundle)
            .add_systems(SCLoad, load_block_palette)
            .add_systems(SCLoad, load_item_registry)
            .add_systems(SCLoad, load_recipe_registry)
            .add_systems(
                SCPostLoad,
                (load_version_pack_plugins, release_version_pack_bulk_data),
            )
            .add_systems(PostStartup, finish_startup)
            .add_systems(sc_ecs::schedule::PostUpdate, check_shutdown_signal);
    }
}

fn init_log() {
    sc_log::set_locale("zh-CN");
    // Local log file initializes at startup (SCStartup) and needs the [log] config (ServerProperties).
}

/// Registers shutdown signal handlers: Ctrl+C (SIGINT), terminal close/hangup (SIGHUP), kill (SIGTERM)
/// set the atomic flag (handlers only store, async-signal-safe).
fn register_shutdown_signal_handlers() {
    unsafe {
        let handler = handle_shutdown_signal as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        #[cfg(unix)]
        libc::signal(libc::SIGHUP, handler);
    }
    log::info!("{}", t_log!("console.bootstrap.signal_registered"));
}

/// Checks shutdown signals each tick: when set, sends SCExit (consumers stop region threads, flush logs, exit).
fn check_shutdown_signal(world: World) {
    if SHUTDOWN_REQUESTED.swap(false, Ordering::Relaxed) {
        log::info!("{}", t_log!("console.bootstrap.shutdown_signal"));
        world.send_event(sc_utils::event::SCExit::new(
            sc_utils::event::SCExitReason::Success,
            sc_utils::event::SCExitType::Shutdown,
        ));
    }
}

/// Rust in-process plugin factory registry (name to factory).
///
/// **Enabled by name-matching `kind:"rust"` entries under the version-pack `plugins/` dir**;
/// Rust plugins belong to the version pack (manifest-only packaging) with implementations compiled into the server.
/// Item registry: builds [`sc_item::ItemRegistry`] from [`MinecraftRuntimeManager`] (item name to network id,
/// sourced from the version-pack `definitions/runtime.json`) and maps block items (identifiers matching a
/// block name) to that block's default-state hash.
///
/// Reads [`MinecraftRuntimeManager`], not `SCVersionPack.runtime_id`: packloader pours runtime ids into the
/// manager at SCLoad, then `clear_runtime_id()` frees version-pack memory, leaving the pack field empty.
///
/// Must run after [`load_block_palette`] (SCLoad order): block default-state hashes come from
/// `BlockStateDictionary.first_hash_of`, which needs palette registration to finish.
///
/// Data-driven: hardcodes no item/block names; an item is a block item when its identifier hits the block
/// dictionary (`first_hash_of` returns Some). Non-block items (swords, food, etc.) get
/// `default_block_runtime_id=None` and are skipped by the interaction layer when placing.
fn load_item_registry(
    world: World,
    runtime_manager: Res<sc_packloader::version_control::runtime::MinecraftRuntimeManager>,
    registry: ResMut<sc_item::ItemRegistry>,
    mut item_components: ResMut<sc_packloader::item::ItemComponentTable>,
    json_registry: Res<sc_block::block_json::BlockJsonRegistry>,
) {
    // Empty-item wire values are always `ItemData.AIR` (runtime 0 / block 0 / no net id), never taken from the
    // version-pack palette (air=-158 in `runtime.json` is a palette-internal id). This only checks existence:
    // missing air means a corrupt version pack.
    if runtime_manager.get_runtime_id("minecraft:air").is_none() {
        error!("{}", t_log!("console.item.missing_air"));
    }

    // Shield runtime id: clients switch on ShieldItemID in the ItemRegistry to append
    // ItemExtraDataWithBlockingTick (extra 8-byte blocking tick).
    match runtime_manager.get_runtime_id("minecraft:shield") {
        Some(shield_runtime_id) => {
            sc_network::protocol::client::transaction::configure_shield_item_runtime(
                shield_runtime_id as u16,
            );
            info!(
                "{}",
                t_log!("console.item.shield", runtime = shield_runtime_id)
            );
        }
        None => error!("{}", t_log!("console.item.missing_shield")),
    }

    let dictionary = sc_world::block_dictionary::BlockStateDictionary::global();
    // When the new format is published, block-item mapping takes snapshot defaults (authoritative); otherwise it
    // falls back to legacy first-seen hashes. Both are data-driven identifier-hit checks.
    let snapshot = json_registry.get();
    if snapshot.is_some() {
        info!("{}", t_log!("console.item.snapshot_source"));
    }
    let mut total = 0usize;
    let mut block_items = 0usize;
    for (name, id) in runtime_manager.runtime_entries() {
        // runtime.json ids are signed i16; storing/writing them as u16 shortLE bytes is equivalent (two's complement).
        let runtime_id = id as u16;
        if runtime_id == 0 {
            continue; // Air stays out of the registry.
        }
        let mut definition = sc_item::ItemDefinition::new(runtime_id, name.clone());
        // Block item: identifier hits the block dictionary, so link the default block-state hash.
        let block_hash = snapshot
            .as_ref()
            .and_then(|snap| snap.default_hash(&name))
            .or_else(|| dictionary.first_hash_of(&name));
        if let Some(block_hash) = block_hash {
            definition = definition.block(block_hash);
            block_items += 1;
        }
        registry.upsert(definition);
        total += 1;
    }
    info!(
        "{}",
        t_log!(
            "console.item.registered",
            total = total,
            block_items = block_items
        )
    );
    // Item component dense table: compiles version-pack items/*.json plus behavior-pack components into a
    // runtime_id-indexed queryable resource (has::<Food>() / get::<Food>(), etc.) for gameplay systems.
    item_components.build(&runtime_manager);
    log::debug!(
        "item component table: compiled {} item components (dense runtime_id table)",
        item_components.count()
    );
    let _ = world; // Keeps the World param to match the system signature convention.
}

/// Recipe registry: compiles `RawRecipeSources` (collected from behavior packs at SCPreLoad) into an
/// immutable snapshot (SCLoad).
///
/// - Merges low to high pack order with higher layers fully overwriting the same identifier, and
///   **same-layer duplicates are hard errors** (the whole table fails, the registry stays empty with an
///   error log, never a half table);
/// - Unknown recipe types are rejected by default (compile errors likewise keep the table empty);
/// - The ingredient-group table comes from the version-pack `definitions/recipe_groups.json`
///   (upstream tags plus legacy pseudo-name families, generated offline and vendored), filtered only
///   against the item registry, falling back to an empty table on failure
///   (tag recipes fail closed, exact matches unaffected).
fn load_recipe_registry(world: World) {
    let insert_empty = |reason: &str| {
        world.insert_resource(sc_game::SharedRecipeRegistry::default());
        world.insert_resource(sc_game::SharedItemTags::default());
        error!("{}", t_log!("console.recipe.empty", reason = reason));
    };
    let Some(sources) = world.get_resource::<sc_packloader::recipe_source::RawRecipeSources>()
    else {
        insert_empty("无配方源");
        return;
    };
    if !sources.errors.is_empty() {
        warn!(
            "{}",
            t_log!(
                "console.recipe.skipped_sources",
                count = sources.errors.len(),
                first = sources
                    .errors
                    .first()
                    .map(|e| e.to_string())
                    .unwrap_or_default()
            )
        );
    }
    let inputs: Vec<sc_recipe::SourceRecipe> = sources
        .files
        .iter()
        .map(|file| {
            sc_recipe::SourceRecipe::from_parts(
                file.pack_name.clone(),
                file.pack_order,
                file.relative_path.clone(),
                file.format_version.clone(),
                file.raw.clone(),
                file.fingerprint,
                file.raw_bytes,
            )
        })
        .collect();
    drop(sources);
    match sc_recipe::RecipeRegistrySnapshot::compile(
        &inputs,
        &sc_recipe::CompileBudgets::default(),
        false,
    ) {
        Ok((snapshot, diagnostics)) => {
            let fingerprint = snapshot.fingerprint().hex();
            let count = diagnostics.recipe_count;
            let disabled = diagnostics.disabled.len();
            // Ingredient-group table: version-pack `definitions/recipe_groups.json` (upstream item_tags
            // plus legacy pseudo-name families, generated offline and vendored, see tools/gen_recipe_groups.py).
            // Only filters against the registry here (drops unknown members and emptied groups), never
            // synthesizes or guesses; missing files fall back to an empty table (tag/pseudo-name queries
            // fail closed, exact matches unaffected).
            let recipe_groups = world
                .get_resource_mut::<sc_packloader::version_control::SCVersionPack>()
                .map(|mut pack| pack.take_recipe_groups())
                .unwrap_or_default();
            let groups = {
                use std::collections::HashSet;
                let registry_names: HashSet<String> = world
                    .get_resource::<sc_item::ItemRegistry>()
                    .map(|registry| registry.names().into_iter().collect())
                    .unwrap_or_default();
                filter_recipe_groups_by_registry(recipe_groups, &registry_names)
            };
            log::debug!(
                "recipe registry: compiled {count} recipes (disabled {disabled}, fingerprint {fingerprint}), {n} ingredient groups",
                n = groups.len(),
            );
            {
                let mut keys: Vec<&String> = groups.keys().collect();
                keys.sort();
                log::debug!("ingredient groups: {keys:?}");
            }
            world.insert_resource(sc_game::SharedRecipeRegistry::new(snapshot));
            world.insert_resource(sc_game::SharedItemTags::new(groups));
            for entry in diagnostics.disabled.iter().take(8) {
                warn!(
                    "{}",
                    t_log!(
                        "console.recipe.disabled",
                        identifier = entry.identifier,
                        reason = entry.reason
                    )
                );
            }
        }
        Err(error) => insert_empty(&format!("编译失败：{error}")),
    }
}

/// Filters the version-pack ingredient-group table against the item registry: drops unknown members and emptied groups.
///
/// Validates only, never synthesizes: member order stays as filed since `data` indices depend on it; new
/// variants (e.g. poplar) joining the registry require regenerating the file (tools/gen_recipe_groups.py),
/// never auto-guessed at startup.
fn filter_recipe_groups_by_registry(
    groups: std::collections::HashMap<String, Vec<String>>,
    registry_names: &std::collections::HashSet<String>,
) -> std::collections::HashMap<String, Vec<String>> {
    groups
        .into_iter()
        .filter_map(|(key, members)| {
            let kept: Vec<String> = members
                .into_iter()
                .filter(|name| registry_names.contains(name))
                .collect();
            if kept.is_empty() {
                None
            } else {
                Some((key, kept))
            }
        })
        .collect()
}

/// Block bundle: compiles the immutable registry from version-pack `definitions/blocks/**/*.block.json`
/// (first SCLoad system; must precede `load_block_palette` / `load_item_registry`).
///
/// On success: atomically publishes after full `sc_block` private-builder validation (wholesale dense-registry
/// replacement, capability-table rebuild, snapshot resource publish, global dictionary registration, derived
/// palette/tag adapters).
/// On failure: logs and skips (with the new format declared, never falls back to the legacy palette;
/// the old table stays empty, loudly rather than silently).
fn load_block_bundle(
    world: World,
    mut version_pack: ResMut<SCVersionPack>,
    mut registry: ResMut<sc_block::registry::BlockStateRegistry>,
    mut component_flags: ResMut<sc_block::state::BlockComponentFlags>,
    json_registry: ResMut<sc_block::block_json::BlockJsonRegistry>,
    runtime_manager: Res<sc_packloader::version_control::runtime::MinecraftRuntimeManager>,
) {
    let Some(bundle) = version_pack.take_block_json_bundle() else {
        return;
    };
    let pack_id = version_pack.manifest.name.clone();
    // Tool/drop item existence must hold in the current version-pack item registry (`runtime.json`).
    let item_exists = |id: &str| runtime_manager.get_runtime_id(id).is_some();
    match sc_block::block_json::compile_bundle(
        &bundle,
        &pack_id,
        &sc_packloader::block::BlockBundleBudgets::default(),
        &sc_block::block_json::global_preexisting_lookup,
        &item_exists,
    ) {
        Ok((snapshot, warnings)) => {
            for w in warnings.iter() {
                log::warn!("{}", t_log!("console.block.bundle_warning", warning = w));
            }
            let state_count = snapshot.states.len();
            let type_count = snapshot.types.len();
            let fingerprint = snapshot.fingerprint;
            let snapshot = Arc::new(snapshot);
            // Atomic publish: wholesale replacement (failure paths never reach here).
            *registry = sc_block::registry::BlockStateRegistry::from_block_snapshot(&snapshot);
            component_flags.build_from_snapshot(&snapshot);
            json_registry.publish(Arc::clone(&snapshot));
            // Compat-period derived adapter: independent dictionary copies of dynamic plugins still read palette bytes
            // (contents match the snapshot, never mixed sources).
            match snapshot.encode_legacy_palette_bytes() {
                Ok(bytes) => version_pack.set_derived_block_palette(bytes),
                Err(e) => error!("{}", t_log!("console.block.palette_encode_fail", error = e)),
            }
            log::debug!("block bundle: published {type_count} types / {state_count} states (fingerprint={fingerprint:#x})");
            // Break-time data coverage (mine seconds come from block JSON; undeclared uses the explicit fallback).
            let declared = snapshot
                .mining_seconds
                .iter()
                .filter(|v| v.is_some())
                .count();
            let unbreakable = snapshot.unbreakable.iter().filter(|v| **v).count();
            let mining_profiles = snapshot.mining.iter().filter(|v| v.is_some()).count();
            let drop_profiles = snapshot
                .drops
                .iter()
                .filter(|v| v.as_ref().is_some_and(|d| d.enabled))
                .count();
            log::debug!(
                "block mining data: {declared} states with declared seconds / {unbreakable} unbreakable / {} undeclared (fallback {} ticks); sc:mining {} states / sc:drops(enabled) {} states",
                state_count - declared - unbreakable,
                sc_game::interaction::FALLBACK_BREAK_TIME_TICKS,
                mining_profiles,
                drop_profiles
            );
        }
        Err(e) => error!("{}", t_log!("console.block.bundle_compile_fail", error = e)),
    }
    let _ = world;
}

/// Block palette: registers all block states from version-pack `definitions/block_palette.nbt`
/// (palette-first dual mode, falling back to the bootstrap dictionary when missing). SCLoad runs after the
/// sc_packloader load_version (SCPreLoad already inserted SCVersionPack as a resource).
///
/// After palette load, builds the dense capability-bitmask table (`BlockComponentFlags`) over dense
/// [`sc_block::state::BlockStateId`] for O(1) logic hot paths such as random-tick filtering and redstone signals.
fn load_block_palette(
    world: World,
    version_pack: ResMut<SCVersionPack>,
    registry: ResMut<sc_block::registry::BlockStateRegistry>,
    mut component_flags: ResMut<sc_block::state::BlockComponentFlags>,
) {
    // When the new format is declared, takes the bundle path: even a failed bundle compile never falls back
    // to legacy files (legacy and new sources must never mix).
    if version_pack.has_declared_block_json() {
        info!("{}", t_log!("console.block.palette_bundle_path"));
        return;
    }
    // Borrows instead of taking: palette bytes stay in the resource so dlopened rust-kind plugins can register
    // them into their own dictionary copies (independent OnceLock inside the DLL) at enable time.
    let Some(bytes) = version_pack.block_palette_bytes() else {
        info!("{}", t_log!("console.block.palette_no_pack"));
        return;
    };
    match registry.load_palette_from_bytes(bytes) {
        Ok(count) => {
            log::debug!(
                "block palette: registered {count} states (FNV1a-32 hash + dense BlockStateId)"
            );
            component_flags.build(&registry);
            log::debug!(
                "block capability mask: built {} dense entries (by BlockStateId)",
                component_flags.len()
            );
        }
        Err(e) => error!("{}", t_log!("console.block.palette_parse_fail", error = e)),
    }
    let _ = world;
}

/// Loaded host ABI plugins (reference held so dlopened .so files stay loaded).
#[derive(Resource, Default)]
pub struct HostPlugins(pub Vec<LoadedHostPlugin>);

/// Host World pointer (plugin registration functions reach host resources through it).
static HOST_WORLD: OnceLock<World> = OnceLock::new();

fn set_host_world(world: &World) {
    let _ = HOST_WORLD.set(world.clone());
}

fn host_world() -> Option<&'static World> {
    HOST_WORLD.get()
}

/// In-version-pack plugin loading (SCPostLoad).
///
/// Dispatches on manifest.kind:
/// - `"rust"`: **in-process** Rust plugins (manifest-only packaging), name-matched against factories
///   compiled into the server (`BUILTIN_PLUGIN_FACTORIES`), enabled directly with sc_*/ECS;
/// - `"cabi"` (default): **cross-language** cdylib plugins (containing plugin.so), dynamically loaded via
///   C-ABI (`SCPluginLoader::load_host_plugin` plus `HostApiV1`).
fn load_version_pack_plugins(
    world: World,
    mut manager: ResMut<SCPluginManager>,
    mut version_pack: ResMut<SCVersionPack>,
    server_properties: Res<ServerProperties>,
    mut host_plugins: ResMut<HostPlugins>,
) {
    if !server_properties.enable_version_pack_plugins {
        info!("{}", t_log!("console.plugin.disabled"));
        return;
    }
    set_host_world(&world);
    let api = HostApiV1 {
        register_world_generator: host_register_world_generator,
        hash_block_state: host_hash_block_state,
        log_info: host_log_info,
        log_warn: host_log_warn,
    };
    let plugins = version_pack.take_plugins_bytes();
    // Rust plugins are enabled below and may read SCVersionPack (the
    // overworld generator uses its worldgen bundle). Release the mutable
    // resource guard before calling plugin code to avoid a self-deadlock.
    drop(version_pack);
    if plugins.is_empty() {
        return;
    }
    let start = Local::now().timestamp_millis();
    let mut count = 0usize;
    let app = manager.app();
    for bytes in plugins {
        let manifest = match SCPluginLoader::read_manifest(&bytes) {
            Ok(manifest) => manifest,
            Err(e) => {
                error!("{}", t_log!("console.plugin.manifest_fail", error = e));
                continue;
            }
        };
        if manifest.kind == "rust" {
            let name = manifest.name.clone();
            match SCPluginLoader::load_rust_plugin(bytes, app.clone(), manifest) {
                Ok(plugin) => {
                    manager.add_plugin(plugin);
                    info!("{}", t_log!("console.plugin.rust_registered", name = name));
                    count += 1;
                }
                Err(e) => {
                    error!(
                        "{}",
                        t_log!("console.plugin.rust_load_fail", name = name, error = e)
                    );
                }
            }
        } else {
            // Cross-language cabi plugin (plugin.so plus C-ABI).
            match SCPluginLoader::load_host_plugin(bytes, &api) {
                Ok(plugin) => {
                    host_plugins.0.push(plugin);
                    count += 1;
                }
                Err(e) => {
                    error!("{}", t_log!("console.plugin.host_load_fail", error = e));
                }
            }
        }
    }
    for dependency_error in manager.check_dependencies() {
        error!(
            "{}",
            t_log!("console.plugin.dependency_error", error = dependency_error)
        );
    }
    for (name, version) in manager.enable_checked_plugins() {
        info!(
            "{}",
            t_log!(
                "console.plugin.rust_enabled",
                name = name,
                version = version
            )
        );
    }
    if count > 0 {
        info!(
            "{}",
            t_log!(
                "console.plugin.loaded",
                count = count,
                ms = Local::now().timestamp_millis() - start
            )
        );
    }
}

/// Releases bulky version-pack load-time raw data (end of SCPostLoad, once all consumers are ready):
///
/// - `worldgen` / `block_tags` / `biome_features`: no runtime consumers (the world generator uses the
///   compile-time ported Rust implementation plus hardcoded tag lists), purely resident waste;
/// - `block_palette`: host registry registration (SCLoad `load_block_palette`) and DLL plugin dictionary
///   registration (`load_version_pack_plugins` enable) have both completed.
///
/// After release `SCVersionPack` keeps only the manifest and taken-empty tables, cutting resident memory
/// by several MB (worldgen JSON bundle plus 17k-state palette bytes).
fn release_version_pack_bulk_data(mut version_pack: ResMut<SCVersionPack>) {
    let worldgen = version_pack.take_worldgen();
    let block_tags = version_pack.take_block_tags();
    let biome_features = version_pack.take_biome_features();
    let block_palette = version_pack.take_block_palette();

    let mut entries = 0usize;
    let mut bytes = 0usize;
    for bundle in [
        worldgen.density_functions.values(),
        worldgen.noises.values(),
        worldgen.noise_settings.values(),
    ] {
        for value in bundle {
            entries += 1;
            bytes += value.len();
        }
    }
    let tag_count = block_tags.len();
    let mut tag_members = 0usize;
    for members in block_tags.values() {
        tag_members += members.len();
        bytes += members.iter().map(String::len).sum::<usize>();
    }
    if let Some(palette) = block_palette.as_deref() {
        bytes += palette.len();
    }
    drop(worldgen);
    drop(block_tags);
    drop(biome_features);
    drop(block_palette);

    log::debug!(
        "version pack load-time data released: worldgen {entries} entries / block_tags {tag_count} tags ({tag_members} members) / biome_features cleared / block_palette freed (~{}KB)",
        bytes / 1024
    );
}

// ===================== Host ABI implementation (HostApiV1 function pointers) =====================

extern "C" fn host_log_info(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let msg = unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy();
    info!("{}", t_log!("console.plugin.forward_info", msg = msg));
}

extern "C" fn host_log_warn(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let msg = unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy();
    log::warn!("{}", t_log!("console.plugin.forward_warn", msg = msg));
}

/// Block runtime id: FNV1a-32(name + states_json). Empty states_json means stateless;
/// JSON objects such as `{"snowy":false}` convert to CompoundNbt.
extern "C" fn host_hash_block_state(name: *const c_char, states_json: *const c_char) -> u32 {
    let name = unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    let states_json = unsafe { std::ffi::CStr::from_ptr(states_json) }
        .to_string_lossy()
        .into_owned();
    let states = if states_json.is_empty() {
        None
    } else {
        parse_states_json(&states_json)
    };
    block_state_hash(&name, states.as_ref())
}

/// Minimal JSON state parsing: only bool/int/string values (enough for plugin canonical states).
fn parse_states_json(json: &str) -> Option<CompoundNbt> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let object = value.as_object()?;
    let mut states = CompoundNbt::new(None);
    for (key, value) in object {
        match value {
            serde_json::Value::Bool(b) => {
                states.insert(key, NbtValue::Byte(*b as i8));
            }
            serde_json::Value::Number(n) if n.is_i64() => {
                if let Some(value) = n.as_i64().and_then(|value| i32::try_from(value).ok()) {
                    states.insert(key, NbtValue::Int(value));
                }
            }
            serde_json::Value::String(s) => {
                states.insert(key, NbtValue::String(s.clone()));
            }
            _ => {}
        }
    }
    Some(states)
}

/// Registers a world generator: world_kind 0=Overworld 1=Nether 2=End.
extern "C" fn host_register_world_generator(world_kind: i32, generate: GenerateFn) {
    let world_type = match world_kind {
        0 => WorldType::Overworld,
        1 => WorldType::TheNether,
        2 => WorldType::TheEnd,
        _ => {
            log::warn!(
                "{}",
                t_log!("console.hostapi.unknown_world", kind = world_kind)
            );
            return;
        }
    };
    let Some(world) = host_world() else {
        log::warn!("{}", t_log!("console.hostapi.no_world"));
        return;
    };
    let Some(mut manager) = world.get_resource_mut::<MinecraftWorldManager>() else {
        log::warn!("{}", t_log!("console.hostapi.manager_missing"));
        return;
    };
    let world_ids: Vec<MinecraftWorldId> = manager
        .get_worlds_by_type(&world_type)
        .iter()
        .map(|world| world.world_id.clone())
        .collect();
    let mut installed = 0;
    for world_id in world_ids {
        if manager.set_world_generator(&world_id, Arc::new(CAbiGenerator { generate })) {
            installed += 1;
        }
    }
    info!(
        "{}",
        t_log!(
            "console.hostapi.generator_registered",
            kind = world_kind,
            count = installed
        )
    );
}

/// Adapts a plugin C-ABI generation callback into a host `WorldGenerator`.
struct CAbiGenerator {
    generate: GenerateFn,
}

impl WorldGenerator for CAbiGenerator {
    fn generate_chunk(
        &self,
        request: ChunkGenerationRequest,
    ) -> Result<Option<Chunk>, WorldStorageError> {
        let mut chunk = Chunk::empty(
            request.key.position,
            request.key.dimension,
            request.min_y,
            request.max_y,
        );
        let req = ChunkGenRequest {
            x: request.key.position.x,
            z: request.key.position.z,
            dimension: request.key.dimension,
            min_y: request.min_y,
            max_y: request.max_y,
        };
        let chunk_ptr = &mut chunk as *mut Chunk as *mut c_void;
        unsafe { (self.generate)(chunk_ptr, &req, set_block_cb) };
        Ok(Some(chunk))
    }
}

/// Plugin set_block callback: writes into the chunk under construction.
unsafe extern "C" fn set_block_cb(ctx: *mut c_void, x: u8, y: i32, z: u8, runtime_id: u32) {
    let chunk = unsafe { &mut *(ctx as *mut Chunk) };
    let _ = chunk.set_block_at(0, x, y, z, BlockRuntimeId(runtime_id));
}

fn startup(world: World) {
    let mut ms = StartupTimestamp::default();
    // Manual shutdown (Ctrl+C/terminal close/SIGTERM) triggers graceful exit (SCExit flow).
    register_shutdown_signal_handlers();
    // Local log file: filename format/live-flush interval come from the [log] config (strftime subset).
    if let Some(properties) = world.get_resource::<ServerProperties>() {
        sc_log::file::init_file_sink_with_name(
            &properties.log_file_name_format,
            PathBuf::from("logs"),
            Duration::from_millis(properties.log_flush_interval_ms),
        );
    }
    // try_init: a host (GUI/mobile app) may install its own log capturer first, in which case this
    // silently skips; standalone behavior matches init() exactly.
    let _ = pretty_env_logger::formatted_timed_builder()
        .filter_level(LevelFilter::Debug)
        // Custom format: keeps the pretty_env_logger default layout (timestamp plus colored level), while
        // messages stream through sc_log::color::ColorizingWriter converting section codes to colorful ANSI
        // output (codeless chunks pass through allocation-free; NO_COLOR/TERM=dumb strips).
        .format(|buf, record| {
            use std::io::Write as _;
            // File lines (no ANSI/codes; async batched writes, main thread only non-blocking pushes).
            sc_log::file::write_line_with_level(
                format!(
                    "{} {:<5} {} {}\n",
                    buf.timestamp(),
                    record.level(),
                    record.target(),
                    sc_log::color::plain_text(&record.args().to_string())
                ),
                record.level(),
            );
            // Terminal: keeps the pretty_env_logger default layout (timestamp plus colored level), while
            // messages stream through sc_log::color::ColorizingWriter converting section codes to colorful ANSI
            // output (codeless chunks pass through allocation-free; NO_COLOR/TERM=dumb strips).
            writeln!(
                buf,
                "{} {:<5} ",
                buf.timestamp(),
                buf.default_styled_level(record.level())
            )?;
            let mut writer = sc_log::color::ColorizingWriter::with_auto(buf);
            writer.write_fmt(*record.args())?;
            writer.write_all(b"\n")
        })
        .try_init();
    ms.start = chrono::Local::now().timestamp_millis();
    let server = Server::default(world.clone());
    info!(
        "{}",
        t_log!("console.startup", version = server.server_version)
    );
    info!("{}", t_log!("console.warning"));
    world.insert_resource(ms);
    Server::init(server);
}

fn init_properties(world: World) {
    let Ok(mut current_dir) = env::current_dir() else {
        eprintln!("bootstrap: failed to resolve current directory");
        log::error!("{}", t_log!("console.bootstrap.cwd_fail"));
        return;
    };
    current_dir.push("server_properties.toml");
    match ServerProperties::load(&current_dir) {
        Ok(server_properties) => {
            // Console language applies process-wide and propagates to
            // dlopened plugins via the environment (see SCULK_LOCALE).
            sc_log::set_locale(&server_properties.language);
            std::env::set_var("SCULK_LOCALE", &server_properties.language);
            world.insert_resource(server_properties);
        }
        Err(error) => {
            eprintln!(
                "bootstrap: failed to load {}: {error}",
                current_dir.display()
            );
            log::error!(
                "{}",
                t_log!(
                    "console.bootstrap.config_load_fail",
                    path = current_dir.display(),
                    error = error
                )
            );
        }
    }
}

pub fn finish_startup(mut ms: ResMut<StartupTimestamp>) {
    ms.end = chrono::Local::now().timestamp_millis();
    let ms = ms.end - ms.start;
    info!("{}", t_log!("console.finish", ms = ms));
}

#[cfg(test)]
mod tests {
    use super::filter_recipe_groups_by_registry;
    use std::collections::{HashMap, HashSet};
    use std::io::Cursor;

    #[test]
    fn recipe_group_filter_drops_unknown_members_and_empty_groups_but_keeps_order() {
        let groups: HashMap<String, Vec<String>> = HashMap::from([
            (
                "minecraft:planks".to_string(),
                vec![
                    "minecraft:oak_planks".to_string(),
                    "minecraft:poplar_planks".to_string(),
                ],
            ),
            (
                "minecraft:ghost".to_string(),
                vec!["minecraft:nope".to_string()],
            ),
        ]);
        let registry: HashSet<String> = HashSet::from(["minecraft:oak_planks".to_string()]);
        let filtered = filter_recipe_groups_by_registry(groups, &registry);
        // Member order preserved (data indices depend on it); drops unknown members and emptied groups.
        assert_eq!(
            filtered.get("minecraft:planks"),
            Some(&vec!["minecraft:oak_planks".to_string()])
        );
        assert!(!filtered.contains_key("minecraft:ghost"));
    }

    /// 默认配置模板必须能被当前解析器完整加载（改解析器必填键时同步改模板）。
    #[test]
    fn default_config_parses() {
        let dir = std::env::temp_dir().join(format!(
            "sculk_bootstrap_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("测试目录创建失败");
        let path = dir.join("server_properties.toml");
        std::fs::write(&path, super::DEFAULT_SERVER_PROPERTIES).expect("测试配置写入失败");
        let properties = sc_utils::game::structs::server_properties::ServerProperties::load(&path)
            .expect("默认配置必须可解析");
        assert_eq!(properties.ipv4_port, 19132);
        assert_eq!(properties.overworld_name, "OverWorld");
        assert!(properties.debug_blocked_packets.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-pack regression test: `recipe_groups.json` must cover every ingredient id in the compiled snapshot
    /// (genuine registry items or file groups), otherwise crafting regresses where legacy logic matched but the
    /// new file lacks the group. The data source matches production (same pack, compile, and registry).
    #[test]
    fn recipe_groups_file_covers_snapshot_ingredients() {
        let pack_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../version_packs/[SC原版版本包] Vanilla-1.26.40.scver"
        );
        let bytes = std::fs::read(pack_path).expect("读取版本包失败");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip 打开失败");
        let mut pack =
            sc_packloader::version_control::loader::ZippedVersionPack::get_version(&mut zip)
                .expect("版本包解析失败");

        // Recipe sources use the same read path as SCPackLoaderPlugin.load_version
        // (behavior packs sorted low to high before pack_order assignment).
        let budgets = sc_packloader::recipe_source::RecipeSourceBudgets::default();
        let mut behavior_packs = pack.take_behavior_packs();
        behavior_packs.sort_by(|(version_a, _), (version_b, _)| version_a.cmp(version_b));
        let mut files = Vec::new();
        for (order, (_, resource_pack)) in behavior_packs.iter().enumerate() {
            let (mut part, _) = sc_packloader::recipe_source::read_recipes_from_resource_pack(
                resource_pack,
                order,
                &budgets,
            );
            files.append(&mut part);
        }
        let sources: Vec<sc_recipe::SourceRecipe> = files
            .iter()
            .map(|file| {
                sc_recipe::SourceRecipe::from_parts(
                    file.pack_name.clone(),
                    file.pack_order,
                    file.relative_path.clone(),
                    file.format_version.clone(),
                    file.raw.clone(),
                    file.fingerprint,
                    file.raw_bytes,
                )
            })
            .collect();
        let (snapshot, _) = sc_recipe::RecipeRegistrySnapshot::compile(
            &sources,
            &sc_recipe::CompileBudgets::default(),
            false,
        )
        .expect("配方编译失败");

        // Registry names share the load_item_registry source (runtime entries, skipping air).
        let mut runtime_pack =
            sc_packloader::version_control::loader::ZippedVersionPack::get_version(
                &mut zip::ZipArchive::new(Cursor::new(
                    std::fs::read(pack_path).expect("读取版本包失败"),
                ))
                .expect("zip 打开失败"),
            )
            .expect("版本包解析失败");
        let manager = sc_packloader::version_control::runtime::MinecraftRuntimeManager::new(
            runtime_pack.take_runtime_id(),
        );
        let registry_names: HashSet<String> = manager
            .runtime_entries()
            .into_iter()
            .filter(|(_, id)| (*id as u16) != 0)
            .map(|(name, _)| name)
            .collect();

        // Snapshot ingredient ids match the removed collect_ingredient_ids semantics.
        let mut ingredient_ids = HashSet::new();
        {
            use sc_recipe::{IngredientChoice, RecipeBody};
            let mut spec_ids = |spec: &sc_recipe::IngredientSpec| {
                for choice in &spec.choices {
                    match choice {
                        IngredientChoice::Item { identifier, .. } => {
                            ingredient_ids.insert(identifier.clone());
                        }
                        IngredientChoice::Tag { tag } => {
                            ingredient_ids.insert(tag.clone());
                        }
                    }
                }
            };
            for recipe in snapshot.in_network_order() {
                match &recipe.body {
                    RecipeBody::Shaped(body) => {
                        for cell in body.grid.iter().flatten() {
                            spec_ids(cell);
                        }
                    }
                    RecipeBody::Shapeless(body) => {
                        for spec in &body.ingredients {
                            spec_ids(spec);
                        }
                    }
                    RecipeBody::Furnace(body) | RecipeBody::FurnaceMaterial(body) => {
                        spec_ids(&body.input);
                    }
                    RecipeBody::SmithingTransform(body) => {
                        spec_ids(&body.template);
                        spec_ids(&body.base);
                        spec_ids(&body.addition);
                    }
                    RecipeBody::SmithingTrim(body) => {
                        spec_ids(&body.template);
                        spec_ids(&body.base);
                        spec_ids(&body.addition);
                    }
                    RecipeBody::BrewingMix(body) | RecipeBody::BrewingContainer(body) => {
                        spec_ids(&body.input);
                        spec_ids(&body.reagent);
                    }
                    RecipeBody::MaterialReducer(body) => {
                        spec_ids(&body.input);
                    }
                }
                for spec in &recipe.unlock {
                    spec_ids(spec);
                }
            }
        }

        let groups = filter_recipe_groups_by_registry(pack.take_recipe_groups(), &registry_names);
        assert!(!groups.is_empty(), "recipe_groups 不得为空");
        // Intentionally uncovered legacy forms (also table-less in legacy logic, behavior parity, follow up later):
        // - potion_type:*: brewing input/output, needs the potion userdata system;
        // - stone_slab{,2,3,4}: retired legacy slab numeric ids, damage mapping still open;
        // - muttonRaw/horsearmorgold/horsearmoriron: pre-flattening names with no modern mapping.
        // All other snapshot ingredients must be covered by registry or file.
        fn allowlisted(id: &str) -> bool {
            id.starts_with("minecraft:potion_type:")
                || matches!(
                    id,
                    "minecraft:stone_slab"
                        | "minecraft:stone_slab2"
                        | "minecraft:stone_slab3"
                        | "minecraft:stone_slab4"
                        | "minecraft:muttonRaw"
                        | "minecraft:horsearmorgold"
                        | "minecraft:horsearmoriron"
                )
        }
        let mut uncovered = Vec::new();
        for id in &ingredient_ids {
            if registry_names.contains(id) {
                continue;
            }
            match groups.get(id) {
                Some(members)
                    if !members.is_empty()
                        && members.iter().all(|m| registry_names.contains(m)) =>
                {
                    // ok
                }
                _ if allowlisted(id) => {}
                _ => uncovered.push(id.clone()),
            }
        }
        assert!(
            uncovered.is_empty(),
            "recipe_groups 未覆盖快照配料: {uncovered:?}"
        );
    }
}
