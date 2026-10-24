//! sc_bootstrap：服务端组装入口（库形态，供宿主应用嵌入）。
//!
//! 独立运行使用 `sc_bootstrap::run()`（或直接使用本 crate 的二进制）；
//! GUI/移动宿主（如 sc_app）通过 [`build_app`] 取得组装好的 app，
//! 再按需替换 runner（宿主轮询加有序停止）。
//!
//! sc_bootstrap: server assembly entry (library form for host-app embedding).
//!
//! Standalone runs use `sc_bootstrap::run()` (or this crate's binary directly);
//! GUI/mobile hosts (e.g. sc_app) take the assembled app via [`build_app`],
//! then swap the runner (host poll plus orderly stop) as needed.

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

/// 组装完整服务端 app（全部插件，尚未运行）。
///
/// 插件顺序是加载前提：
/// 1. ScheduleRunnerPlugin：默认 20 TPS 主循环（宿主可用 `set_runner` 覆盖）。
/// 2. 命令核心必须先于网络层与版本包内置插件。
/// 3. 方块核心跟随命令核心（注册 /blockat 需要 CommandRegistry）。
/// 4. 游戏域（多世界中枢/跨世界总线/网络边界）最后组装：其 PostUpdate 系统
///    （跨世界分发/出站意图）在所有游戏逻辑之后运行。
/// 控制台桥在世界之后、游戏域之前组装：`Last` 按注册顺序执行，退出序列为
/// chunk 刷盘 -> 终端恢复 -> `process::exit(0)`（见 `console_bridge` 模块文档）。
///
/// Assembles the full server app (all plugins, not yet running).
///
/// Plugin order is load-bearing:
/// 1. ScheduleRunnerPlugin: default 20 TPS main loop (hosts may override via `set_runner`).
/// 2. The command core must come before the network layer and version-pack builtin plugins.
/// 3. The block core follows the command core (registering /blockat needs CommandRegistry).
/// 4. The game domain (multi-world hub/cross-world bus/network boundary) assembles last:
///    its PostUpdate systems (cross-world dispatch/outbound intents) run after all game logic.
///
/// The console bridge assembles after world, before the game domain: `Last` runs in
/// registration order, so the exit sequence is chunk flush
/// (`SCWorldPlugin::on_exit_flush_chunks`) -> terminal restore -> `process::exit(0)`
/// (see the `console_bridge` module docs).
pub fn build_app() -> App {
    // First start in an empty directory: create directories and default config
    // before anything reads them (the console bridge reads the config during
    // `build`, PreStartup `init_properties` loads it; both run after this).
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

/// 独立运行：组装完整服务端 app 并进入主循环（阻塞当前线程）。
///
/// Standalone run: assembles and enters the main loop (blocks the current thread).
pub fn run() -> sc_ecs::app::AppExit {
    build_app().run()
}
