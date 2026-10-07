//! sc_bootstrap: server assembly entry (library form for host-app embedding).
//!
//! Standalone runs use `sc_bootstrap::run()` (or this crate's binary directly);
//! GUI/mobile hosts (e.g. sc_app) take the assembled app via [`build_app`],
//! then swap the runner (host poll plus graceful stop) as needed.

pub mod bootstrap;
pub mod console_bridge;

use sc_block::SCBlockPlugin;
use sc_command::SCCommandPlugin;
use sc_ecs::app::schedule_runner::ScheduleRunnerPlugin;
use sc_ecs::app::App;
use sc_entity::SCEntityPlugin;
use sc_game::SCGamePlugin;
use sc_network::SCNetworkPlugin;
use sc_packloader::SCPackLoaderPlugin;
use sc_utils::schedule::SCSchedulePlugin;
use sc_world::SCWorldPlugin;
use std::time::Duration;

use crate::bootstrap::SCBootStrapPlugin;

/// Assembles the full server app (all plugins, not yet running).
///
/// Plugin order is load-bearing:
/// 1. ScheduleRunnerPlugin: default 20 TPS main loop (hosts may override via `set_runner`).
/// 2. The command core must come before the network layer and version-pack builtin plugins.
/// 3. The block core follows the command core (registering /blockat needs CommandRegistry).
/// 4. The game domain (multi-world hub/cross-world bus/network boundary) assembles last:
///    its PostUpdate systems (cross-world dispatch/outbound intents) run after all game logic.
///
/// The console bridge assembles after world, before the game domain: `Last` runs in registration order,
/// so the exit sequence is chunk flush (`SCWorldPlugin::on_exit_flush_chunks`, TUI still alive and
/// showing flush completion) -> terminal restore -> `process::exit(0)`.
/// If the bridge ran before world, the terminal would restore first and flushing would run hidden,
/// looking like an instant exit (see the `console_bridge` module docs).
pub fn build_app() -> App {
    // 空目录首次启动：先建目录/默认配置，再做任何读取（console 桥 build 期
    // 读配置、PreStartup init_properties 加载配置都排在这之后）。
    crate::bootstrap::ensure_runtime_layout();
    // This ECS `add_plugins` signature is `&self -> &Self` (shared interior lock), so chained
    // calls would borrow a temporary; bind a local first, then clone it back.
    let app = App::new();
    app.add_plugins(ScheduleRunnerPlugin::run_loop(Some(
        Duration::from_secs_f64(1.0 / 20.0),
    )))
    .add_plugins(SCSchedulePlugin)
    .add_plugins(SCBootStrapPlugin)
    .add_plugins(SCCommandPlugin)
    .add_plugins(SCBlockPlugin)
    .add_plugins(SCNetworkPlugin)
    .add_plugins(SCPackLoaderPlugin)
    .add_plugins(SCEntityPlugin)
    .add_plugins(SCWorldPlugin)
    .add_plugins(crate::console_bridge::ConsoleBridgePlugin)
    .add_plugins(SCGamePlugin);
    app.clone()
}

/// Standalone run: assembles and enters the main loop (blocks the current thread).
pub fn run() -> sc_ecs::app::AppExit {
    build_app().run()
}
