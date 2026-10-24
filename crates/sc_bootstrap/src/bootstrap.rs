use std::env;
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

/// 收到关闭信号（Ctrl+C/终端关闭/kill）时置位；主循环检查后触发有序退出。
///
/// Set on shutdown signals (Ctrl+C/terminal close/kill); the main loop
/// triggers the orderly exit sequence after checking it.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// 信号处理器：只置位原子标志（异步信号安全；其余逻辑在主循环中执行）。
///
/// Signal handler: only sets the atomic flag (async-signal-safe; remaining
/// logic runs in the main loop).
extern "C" fn handle_shutdown_signal(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
    sc_utils::event::note_shutdown_requested();
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

/// 启动时间戳资源：Startup 与启动收尾之间的起止毫秒时间戳。
///
/// Startup timestamp resource: start/end millisecond timestamps between
/// Startup and the startup-finish system.
#[derive(Resource, Default)]
pub struct StartupTimestamp {
    start: i64,
    end: i64,
}

/// 启动必需的工作目录布局（相对 cwd；`logs/` 由 sc_log 落盘时自建，
/// `diagnostics/` 由各 dump 点按需自建，这里只保证启动期就会读取的目录）。
///
/// Required working-directory layout at startup (relative to cwd; `logs/`
/// is created by sc_log when flushing and `diagnostics/` by each dump point
/// on demand; only directories read during startup are guaranteed here).
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
///
/// Default configuration written when `server_properties.toml` is missing.
///
/// Must stay in sync with the required keys of [`ServerProperties::parse`]
/// ([game]/[server]/[experimental_gameplay]/[world] are all required; a
/// missing key fails startup), so update both together (locked by the
/// `default_config_parses` regression test).
const DEFAULT_SERVER_PROPERTIES: &str = r#"[server]
enable_snappy = false
xbox_auth = true
validate_encryption = true
force_gamemode = false
enable_version_pack_plugins = true
language = "zh-CN"
enable_hitokoto = false

[game]
name = "SculkBedrock"
motd = "Powered by SculkBedrock"
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
chunks_per_tick = 128
spawn_threshold = 56

[log]
file_name_format = "sc_log_%Y-%m-%d_%H-%M-%S.log"
flush_interval_ms = 200
# Only info/debug
log_level = "info"

[debug]
# Outbound-blocked packet ids for live-disconnect bisection (decimal or TOML
# hex like 0x34 both accepted). Empty in production.
blocked_packets = [
]
"#;

/// 确保运行目录布局存在：建缺失目录；缺失主配置时写默认文件。
///
/// 幂等、无害：已存在的一律不动（不覆盖用户配置）。在 `build_app()` 头部
/// 调用一次——早于 console 桥 `build` 期读配置和 PreStartup
/// `init_properties`，空目录首次启动也能进到正常报错（缺世界/缺版本包），
/// 而不是死在读不到路径上。
///
/// Ensures the runtime directory layout exists: creates missing directories
/// and writes the default config when the main config is absent.
///
/// Idempotent and harmless: existing entries are never touched (user config
/// is never overwritten). Called once at the head of `build_app()` — before
/// the console bridge reads the config during `build` and before PreStartup
/// `init_properties`, so a first start in an empty directory still reaches
/// normal errors (missing world/version pack) instead of dying on
/// unreadable paths.
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

/// 引导插件：注册启动期系统与资源（目录布局、服务器属性、方块/物品/配方
/// 注册、版本包插件加载、世界生成准备与绑定、关闭信号处理）。
///
/// Bootstrap plugin: registers startup systems and resources (directory
/// layout, server properties, block/item/recipe registration,
/// version-pack plugin loading, worldgen preparation and binding,
/// shutdown-signal handling).
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
            .insert_resource(WorldgenPreparedData::default())
            .insert_resource(sc_packloader::item::ItemComponentTable::default())
            .insert_resource(sc_packloader::item::WearTable::default())
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
                (
                    load_version_pack_plugins,
                    prepare_worldgen,
                    bind_worldgen,
                    release_version_pack_bulk_data,
                ),
            )
            .add_systems(PostStartup, finish_startup)
            .add_systems(sc_ecs::schedule::PostUpdate, check_shutdown_signal);
    }
}

/// 初始化日志区域设置；本地日志文件在启动（SCStartup）时初始化，需要
/// [log] 配置（ServerProperties）。
///
/// Initializes the log locale; the local log file initializes at startup
/// (SCStartup) and needs the [log] config (ServerProperties).
fn init_log() {
    sc_log::set_locale("zh-CN");
}

/// 注册关闭信号处理器：Ctrl+C（SIGINT）、终端关闭/挂断（SIGHUP）、
/// kill（SIGTERM）均置位原子标志（处理器只存储，异步信号安全）。
///
/// Registers shutdown signal handlers: Ctrl+C (SIGINT), terminal
/// close/hangup (SIGHUP), and kill (SIGTERM) all set the atomic flag
/// (handlers only store, async-signal-safe).
fn register_shutdown_signal_handlers() {
    unsafe {
        let handler = handle_shutdown_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        #[cfg(unix)]
        libc::signal(libc::SIGHUP, handler);
    }
    log::info!("{}", t_log!("console.bootstrap.signal_registered"));
}

/// 每 tick 检查关闭信号：置位时发送 SCExit（消费方停止区域线程、落盘日志并退出）。
///
/// Checks shutdown signals each tick: when set, sends SCExit (consumers
/// stop region threads, flush logs, and exit).
fn check_shutdown_signal(world: World) {
    if SHUTDOWN_REQUESTED.swap(false, Ordering::Relaxed) {
        log::info!("{}", t_log!("console.bootstrap.shutdown_signal"));
        world.send_event(sc_utils::event::SCExit::new(
            sc_utils::event::SCExitReason::Success,
            sc_utils::event::SCExitType::Shutdown,
        ));
    }
}

/// 物品注册表：从 [`MinecraftRuntimeManager`] 构建 [`sc_item::ItemRegistry`]
/// （物品名 → 网络 id，数据来自版本包 `definitions/runtime.json`），并把
/// 方块物品（标识符命中方块名的物品）映射到该方块的默认状态哈希。
///
/// Item registry: builds [`sc_item::ItemRegistry`] from
/// [`MinecraftRuntimeManager`] (item name to network id, sourced from the
/// version-pack `definitions/runtime.json`) and maps block items
/// (identifiers matching a block name) to that block's default-state hash.
///
/// 读 [`MinecraftRuntimeManager`]，而不是 `SCVersionPack.runtime_id`：
/// packloader 在 SCLoad 把 runtime id 倒进 manager，随后
/// `clear_runtime_id()` 释放版本包内存，包字段只剩空表。
///
/// Reads [`MinecraftRuntimeManager`], not `SCVersionPack.runtime_id`:
/// packloader pours runtime ids into the manager at SCLoad, then
/// `clear_runtime_id()` frees version-pack memory, leaving the pack field
/// empty.
///
/// 必须在 [`load_block_palette`]（SCLoad 顺序）之后运行：方块默认状态哈希来自
/// `BlockStateDictionary.first_hash_of`，需要调色板注册完成后才可用。
///
/// Must run after [`load_block_palette`] (SCLoad order): block default-state
/// hashes come from `BlockStateDictionary.first_hash_of`, which needs
/// palette registration to finish.
///
/// 数据驱动：不硬编码任何物品/方块名；标识符命中方块字典
/// （`first_hash_of` 返回 Some）即为方块物品。非方块物品（剑、食物等）
/// `default_block_runtime_id=None`，放置时被交互层跳过。
///
/// Data-driven: hardcodes no item/block names; an item is a block item when
/// its identifier hits the block dictionary (`first_hash_of` returns Some).
/// Non-block items (swords, food, etc.) get `default_block_runtime_id=None`
/// and are skipped by the interaction layer when placing.
fn load_item_registry(
    world: World,
    runtime_manager: Res<sc_packloader::version_control::runtime::MinecraftRuntimeManager>,
    registry: ResMut<sc_item::ItemRegistry>,
    mut item_components: ResMut<sc_packloader::item::ItemComponentTable>,
    mut wear_table: ResMut<sc_packloader::item::WearTable>,
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
    // Item wear table: compiles `minecraft:durability` (capacity/chance) plus
    // `sc:durability_costs` (server-only per-action costs)
    // into a runtime_id-indexed read-only dense table for break settlement.
    wear_table.build(&runtime_manager);
    info!(
        "item wear table: {} items declare durability (dense runtime_id table)",
        wear_table.count()
    );
    let _ = world; // Keeps the World param to match the system signature convention.
}

/// 配方注册表：把 `RawRecipeSources`（SCPreLoad 从行为包收集）编译为
/// 不可变快照（SCLoad）。
///
/// Recipe registry: compiles `RawRecipeSources` (collected from behavior
/// packs at SCPreLoad) into an immutable snapshot (SCLoad).
///
/// - 按包序从低到高合并，高层完全覆盖同标识符；**同层重复是硬错误**
///   （整表失败，注册表保持为空并打错误日志，绝不会留下半张表）；
/// - 未知配方类型默认拒绝（编译错误同样保持表为空）；
/// - 配料组表来自版本包 `definitions/recipe_groups.json`（物品标签加旧版
///   伪名家族，离线生成后随包内置），只对照物品注册表过滤，失败回退空表
///   （标签配方失败关闭，精确匹配不受影响）。
///
/// - Merges low to high pack order with higher layers fully overwriting the
///   same identifier, and **same-layer duplicates are hard errors** (the
///   whole table fails, the registry stays empty with an error log, never a
///   half table);
/// - Unknown recipe types are rejected by default (compile errors likewise
///   keep the table empty);
/// - The ingredient-group table comes from the version-pack
///   `definitions/recipe_groups.json` (item tags plus legacy pseudo-name
///   families, generated offline and vendored), filtered only against the
///   item registry, falling back to an empty table on failure (tag recipes
///   fail closed, exact matches unaffected).
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
            // Ingredient-group table: version-pack `definitions/recipe_groups.json` (item_tags
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

/// 用物品注册表过滤版本包配料组表：丢弃未知成员和排空后的组。
///
/// Filters the version-pack ingredient-group table against the item
/// registry: drops unknown members and emptied groups.
///
/// 只校验、不合成：成员顺序保持文件原序（`data` 索引依赖它）；新变体
/// （如白杨）进入注册表需要重新生成文件（tools/gen_recipe_groups.py），
/// 绝不在启动时自动猜测。
///
/// Validates only, never synthesizes: member order stays as filed since
/// `data` indices depend on it; new variants (e.g. poplar) joining the
/// registry require regenerating the file (tools/gen_recipe_groups.py),
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

/// 方块 bundle：从版本包 `definitions/blocks/**/*.block.json` 编译不可变
/// 注册表（第一个 SCLoad 系统，必须先于 `load_block_palette` /
/// `load_item_registry`）。
///
/// Block bundle: compiles the immutable registry from version-pack
/// `definitions/blocks/**/*.block.json` (first SCLoad system; must precede
/// `load_block_palette` / `load_item_registry`).
///
/// 成功：完整通过 `sc_block` 私有 builder 校验后原子发布（稠密注册表整体
/// 替换、能力表重建、快照资源发布、全局字典注册、派生调色板/标签适配器）。
/// 失败：记录日志并跳过（新格式已声明时绝不回退旧调色板；旧表保持为空，
/// 大声失败而非静默）。
///
/// On success: atomically publishes after full `sc_block` private-builder
/// validation (wholesale dense-registry replacement, capability-table
/// rebuild, snapshot resource publish, global dictionary registration,
/// derived palette/tag adapters).
/// On failure: logs and skips (with the new format declared, never falls
/// back to the legacy palette; the old table stays empty, loudly rather
/// than silently).
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
            // Workstation/process declarations resolve against live game
            // knowledge here (unknown station tags, dangling lit variants);
            // failure skips publish exactly like a compile failure below.
            if let Err(e) = sc_game::furnace::validate_workstations(&snapshot) {
                error!("{}", t_log!("console.block.bundle_compile_fail", error = e));
                return;
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
                "block mining data: {declared} states with declared seconds / {unbreakable} unbreakable / {} undeclared (fallback {} ticks); sc:mining {} states / loot-table drops {} states",
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

/// 方块调色板：从版本包 `definitions/block_palette.nbt` 注册全部方块状态
/// （调色板优先双模式，缺失时回退引导字典）。SCLoad 在 sc_packloader
/// load_version（SCPreLoad 已插入 SCVersionPack 资源）之后运行。
///
/// Block palette: registers all block states from version-pack
/// `definitions/block_palette.nbt` (palette-first dual mode, falling back
/// to the bootstrap dictionary when missing). SCLoad runs after the
/// sc_packloader load_version (SCPreLoad already inserted SCVersionPack as
/// a resource).
///
/// 调色板加载后，基于稠密 [`sc_block::state::BlockStateId`] 构建稠密能力
/// 位掩码表（`BlockComponentFlags`），供随机 tick 过滤、红石信号等 O(1)
/// 逻辑热路径使用。
///
/// After palette load, builds the dense capability-bitmask table
/// (`BlockComponentFlags`) over dense [`sc_block::state::BlockStateId`] for
/// O(1) logic hot paths such as random-tick filtering and redstone signals.
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

/// 已加载的宿主 ABI 插件（持有引用以保证 dlopen 的 .so 保持加载）。
///
/// Loaded host ABI plugins (reference held so dlopened .so files stay
/// loaded).
#[derive(Resource, Default)]
pub struct HostPlugins(pub Vec<LoadedHostPlugin>);

/// 宿主 World 指针（插件注册函数通过它触及宿主资源）。
///
/// Host World pointer (plugin registration functions reach host resources
/// through it).
static HOST_WORLD: OnceLock<World> = OnceLock::new();

/// 设置宿主 World 指针（仅首次调用生效）。
///
/// Sets the host World pointer (first call wins).
fn set_host_world(world: &World) {
    let _ = HOST_WORLD.set(world.clone());
}

/// 读取宿主 World 指针（未设置时为 `None`）。
///
/// Returns the host World pointer (`None` when unset).
fn host_world() -> Option<&'static World> {
    HOST_WORLD.get()
}

/// 版本包内插件加载（SCPostLoad）。
///
/// In-version-pack plugin loading (SCPostLoad).
///
/// 按 manifest.kind 分派：
/// - `"rust"`：**进程内** Rust 插件（仅清单打包），与编入服务器的工厂
///   （`BUILTIN_PLUGIN_FACTORIES`）按名匹配，直接以 sc_*/ECS 启用；
/// - `"cabi"`（默认）：**跨语言** cdylib 插件（含 plugin.so），经 C-ABI
///   动态加载（`SCPluginLoader::load_host_plugin` 加 `HostApiV1`）。
///
/// Dispatches on manifest.kind:
/// - `"rust"`: **in-process** Rust plugins (manifest-only packaging),
///   name-matched against factories compiled into the server
///   (`BUILTIN_PLUGIN_FACTORIES`), enabled directly with sc_*/ECS;
/// - `"cabi"` (default): **cross-language** cdylib plugins (containing
///   plugin.so), dynamically loaded via C-ABI
///   (`SCPluginLoader::load_host_plugin` plus `HostApiV1`).
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

/// 准备好的内置世界生成数据：选定根对应的自有 JSON 源、已解析闭包与
/// 编译后的不可变 DensityPlan。[`prepare_worldgen`] 成功前为空。
///
/// Prepared built-in worldgen data: the owned JSON sources plus the
/// resolved closure and compiled immutable DensityPlan for the selected
/// root. Empty until [`prepare_worldgen`] succeeds.
#[derive(Resource, Default)]
pub struct WorldgenPreparedData {
    pub sources: Option<Arc<sc_worldgen::WorldgenSources>>,
    pub prepared: Option<Arc<sc_worldgen::PreparedWorldgen>>,
    pub compiled: Option<Arc<sc_worldgen::CompiledWorldgen>>,
}

impl WorldgenPreparedData {
    /// 只有准备与编译都成功后才允许进入生成（预处理失败的世界一律拒绝生成）。
    ///
    /// Generation may only be admitted once preparation and compilation
    /// succeeded (worlds whose preprocessing failed never accept
    /// generation).
    pub fn is_ready(&self) -> bool {
        self.compiled.is_some()
    }
}

/// 世界生成准备与编译步骤（在插件加载与 bulk 数据释放之间运行）：把版本包
/// 世界生成 bundle 转为自有源，从选定根解析可达闭包，链接 noise settings，
/// 并编译不可变 DensityPlan。失败是响亮的——服务器继续提供 console/协议
/// 服务，但生成一律拒绝（内置生成器之外没有可静默填充地形的后备）。
///
/// Worldgen prepare & compile step (runs between plugin loading and
/// bulk-data release): converts the version-pack worldgen bundle into owned
/// sources, resolves the reachable closure from the selected root, links
/// noise settings, and compiles the immutable DensityPlan. Failure is loud —
/// the server keeps serving console/protocol but generation stays refused
/// (there is no fallback generator to silently fill terrain).
fn prepare_worldgen(
    mut version_pack: ResMut<SCVersionPack>,
    mut prepared: ResMut<WorldgenPreparedData>,
) {
    let bundle = version_pack.take_worldgen();
    let to_btree = |map: std::collections::HashMap<String, Vec<u8>>| -> std::collections::BTreeMap<String, Vec<u8>> {
        map.into_iter().collect()
    };
    let sources = sc_worldgen::WorldgenSources {
        density_functions: to_btree(bundle.density_functions),
        noises: to_btree(bundle.noises),
        noise_settings: to_btree(bundle.noise_settings),
    };
    if sources.is_empty() {
        error!(
            "worldgen data missing from version pack (definitions/worldgen/): \
             generation will be refused for every world"
        );
        return;
    }
    // Selected root and data version are pinned by the descriptor contract
    // (`sc_world::generation::descriptor::DATA_VERSION`).
    let start = std::time::Instant::now();
    match sc_worldgen::CompiledWorldgen::compile(&sources, "minecraft:overworld") {
        Ok(compiled) => {
            let elapsed = start.elapsed();
            let closure = compiled.prepared();
            info!(
                "worldgen prepared & compiled in {:.2?}: {} source files, closure {} nodes / {} edges, \
                 {} router roots, {} noise tables, {} plan nodes, \
                 content fingerprint {:016x}, closure fingerprint {:016x}",
                elapsed,
                sources.len(),
                closure.closure.len(),
                closure.edges.len(),
                compiled.settings().router.len(),
                compiled.settings().noises.len(),
                compiled.plan().node_count(),
                closure.content_fingerprint,
                closure.closure_fingerprint
            );
            *prepared = WorldgenPreparedData {
                prepared: Some(Arc::new(closure.clone())),
                compiled: Some(Arc::new(compiled)),
                sources: Some(Arc::new(sources)),
            };
        }
        Err(error) => {
            error!("worldgen preparation/compilation failed: {error}; generation stays refused");
        }
    }
}

/// 把编译后的世界生成资源绑定到已加载的主世界实例。
///
/// Binds compiled worldgen assets to loaded Overworld instances.
fn bind_worldgen(
    mut manager: ResMut<MinecraftWorldManager>,
    prepared: Res<WorldgenPreparedData>,
) {
    let Some(compiled) = prepared.compiled.as_ref() else {
        log::error!("worldgen: compiled worldgen missing; generation stays refused");
        return;
    };

    let world_infos: Vec<(MinecraftWorldId, i64)> = manager
        .get_worlds_by_type(&WorldType::Overworld)
        .iter()
        .map(|w| (w.world_id.clone(), w.world_data.world_seed))
        .collect();

    for (world_id, seed) in world_infos {
        let generator = Arc::new(sc_worldgen::BuiltinWorldGenerator::new(compiled, seed));
        let descriptor = generator.descriptor().clone();
        let land_spawn = generator.find_land_spawn();

        match manager.set_world_generator_with_descriptor(&world_id, generator, descriptor, 0) {
            Ok(true) => {
                log::info!("worldgen >> 已为世界装配内置三层生成器 (seed={seed})");
                if let Some((x, y, z)) = land_spawn {
                    if let Some(world) = manager.get_world_mut(&world_id) {
                        world.world_data.spawn_x = x;
                        world.world_data.spawn_y = y;
                        world.world_data.spawn_z = z;
                        log::info!("worldgen >> 出生点已校准: ({x}, {y}, {z})");
                    }
                }
            }
            Ok(false) => {
                log::warn!("worldgen >> 未找到世界实例: {world_id:?}");
            }
            Err(error) => {
                log::error!("worldgen >> 世界装配生成器失败: {error}");
            }
        }
    }
}

/// 释放版本包加载期的大块原始数据（SCPostLoad 末尾，所有消费方就绪后）：
///
/// - `worldgen`：被早一个系统的 [`prepare_worldgen`] 消费（自有源/闭包现由
///   [`WorldgenPreparedData`] 持有）；
/// - `block_tags` / `biome_features`：无运行时消费方（世界生成器使用内置
///   Rust 实现加硬编码标签表）；
/// - `block_palette`：宿主注册表注册（SCLoad `load_block_palette`）与 DLL
///   插件字典注册（`load_version_pack_plugins` 启用）此时都已执行完。
///
/// Releases bulky version-pack load-time raw data (end of SCPostLoad, once
/// all consumers are ready):
///
/// - `worldgen`: consumed by [`prepare_worldgen`] one system earlier (the
///   owned sources/closure live in [`WorldgenPreparedData`] now);
/// - `block_tags` / `biome_features`: no runtime consumers (the world
///   generator uses the built-in Rust implementation plus hardcoded tag
///   lists);
/// - `block_palette`: host registry registration (SCLoad
///   `load_block_palette`) and DLL plugin dictionary registration
///   (`load_version_pack_plugins` enable) have both already run.
///
/// 释放后 `SCVersionPack` 只保留清单与取空的表，常驻内存减少数 MB
/// （世界生成 JSON bundle 加 17k 状态调色板字节）。
///
/// After release `SCVersionPack` keeps only the manifest and taken-empty
/// tables, cutting resident memory by several MB (worldgen JSON bundle plus
/// 17k-state palette bytes).
fn release_version_pack_bulk_data(mut version_pack: ResMut<SCVersionPack>) {
    let block_tags = version_pack.take_block_tags();
    let biome_features = version_pack.take_biome_features();
    let block_palette = version_pack.take_block_palette();

    let mut bytes = 0usize;
    let tag_count = block_tags.len();
    let mut tag_members = 0usize;
    for members in block_tags.values() {
        tag_members += members.len();
        bytes += members.iter().map(String::len).sum::<usize>();
    }
    if let Some(palette) = block_palette.as_deref() {
        bytes += palette.len();
    }
    drop(block_tags);
    drop(biome_features);
    drop(block_palette);

    log::debug!(
        "version pack load-time data released: block_tags {tag_count} tags ({tag_members} members) / biome_features cleared / block_palette freed (~{}KB)",
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

/// 方块运行时 id：FNV1a-32(name + states_json)。states_json 为空即无状态；
/// `{"snowy":false}` 等 JSON 对象转换为 CompoundNbt。
///
/// Block runtime id: FNV1a-32(name + states_json). Empty states_json means
/// stateless; JSON objects such as `{"snowy":false}` convert to CompoundNbt.
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

/// JSON 状态解析：只支持 bool/int/string 值（覆盖插件的规范状态）。
///
/// Parses a JSON state object: only bool/int/string values (enough for
/// plugin canonical states).
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

/// 已退役的世界生成注册：内置生成器为静态链接，外部注册不得覆盖。
/// `HostApiV1` 表布局不变（不升 ABI 版本）；该能力显式拒绝并报告废弃，
/// 不安装任何东西。`world_kind` 0=主世界 1=下界 2=末地。
///
/// Retired worldgen registration: the built-in generator is statically
/// linked and no external registration may override it. The `HostApiV1`
/// table layout is unchanged (no ABI version bump); this capability
/// explicitly rejects and reports deprecation instead of installing
/// anything. `world_kind` 0=Overworld 1=Nether 2=End.
extern "C" fn host_register_world_generator(_world_kind: i32, _generate: GenerateFn) {
    log::error!(
        "retired HostApiV1.register_world_generator call rejected: world \
         generation is built into the server (builtin_density_three_layer_v1); \
         external worldgen registration is deprecated and never installed"
    );
}

/// 把插件 C-ABI 生成回调适配成宿主的 `WorldGenerator`。
///
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

/// 插件 set_block 回调：写入正在构建的 chunk。
///
/// Plugin set_block callback: writes into the chunk under construction.
unsafe extern "C" fn set_block_cb(ctx: *mut c_void, x: u8, y: i32, z: u8, runtime_id: u32) {
    let chunk = unsafe { &mut *(ctx as *mut Chunk) };
    let _ = chunk.set_block_at(0, x, y, z, BlockRuntimeId(runtime_id));
}

/// 启动系统：注册关闭信号处理器、初始化本地日志落盘与日志级别、
/// 创建 Server 并记录启动时间戳。
///
/// Startup system: registers shutdown signal handlers, initializes the
/// local log-file sink and log level, creates the Server, and records the
/// startup timestamp.
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
    // Console/file output level comes from `[log] log_level` (info/debug, default debug).
    let level_filter = world
        .get_resource::<ServerProperties>()
        .map(|properties| match properties.log_level.as_str() {
            "info" => LevelFilter::Info,
            _ => LevelFilter::Debug,
        })
        .unwrap_or(LevelFilter::Debug);
    // The global-logger race may already be lost: GUI console installs its
    // own logger at plugin-build time (before the config loads), so the
    // `try_init` below can silently skip. Drive both gates from the config
    // unconditionally so `log_level` works in every console mode.
    log::set_max_level(level_filter);
    sc_console::set_console_max_level(level_filter);
    let _ = pretty_env_logger::formatted_timed_builder()
        .filter_level(level_filter)
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

/// 加载 `server_properties.toml`：应用进程级控制台语言并插入资源；
/// 加载失败只记录错误（由后续系统处理缺失）。
///
/// Loads `server_properties.toml`: applies the process-wide console
/// language and inserts the resource; a load failure only logs an error
/// (later systems handle the absence).
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

/// 启动收尾：计算并记录启动总耗时。
///
/// Finishes startup: computes and logs the total startup time.
pub fn finish_startup(mut ms: ResMut<StartupTimestamp>) {
    ms.end = chrono::Local::now().timestamp_millis();
    let ms = ms.end - ms.start;
    info!("{}", t_log!("console.finish", ms = ms));
}

#[cfg(test)]
mod tests {
    use super::filter_recipe_groups_by_registry;
    use std::collections::{HashMap, HashSet};

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
    ///
    /// The default config template must load fully with the current parser
    /// (update the template together with the parser's required keys).
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
        assert_eq!(properties.log_level, "info");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `log_level` 缺省即 debug；非法值直接拒绝（不静默跑错级别）。
    ///
    /// `log_level` defaults to debug; invalid values are rejected instead of
    /// silently running at the wrong level.
    #[test]
    fn log_level_defaults_and_rejects_garbage() {
        use sc_utils::game::structs::server_properties::ServerProperties;
        let dir = std::env::temp_dir().join(format!(
            "sculk_log_level_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("测试目录创建失败");
        let missing = dir.join("missing.toml");
        std::fs::write(
            &missing,
            super::DEFAULT_SERVER_PROPERTIES.replace("log_level = \"info\"\n", ""),
        )
        .expect("测试配置写入失败");
        let properties =
            ServerProperties::load(&missing).expect("缺 log_level 必须可解析");
        assert_eq!(properties.log_level, "debug");

        let bad = dir.join("bad.toml");
        std::fs::write(
            &bad,
            super::DEFAULT_SERVER_PROPERTIES.replace("log_level = \"info\"", "log_level = \"verbose\""),
        )
        .expect("测试配置写入失败");
        assert!(
            ServerProperties::load(&bad).is_err(),
            "非法 log_level 必须拒绝"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

}
