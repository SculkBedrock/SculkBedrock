//! Console bridge: the only contact point between `sc_console` (pure terminal UI) and game ECS.
//!
//! # Why a separate file
//!
//! `sc_console` depends on no game crate; this file is the sole reverse dependency
//! (`sc_bootstrap` -> `sc_console`), translating terminal IO into game-boundary events:
//!
//! ```text
//! Console thread --String (owned, bounded)--> drain_console_commands (PreUpdate)
//!   -> RawCommandInput { origin: Console } -> dispatch_commands -> command systems
//! Registry/version snapshot -> sync_console_meta -> TUI completion plus top version bar
//! Startup done (PostStartup) -> mark_server_running -> TUI sweep plus input unlock
//! SCExit -> chunk flush (world plugin) -> shutdown_console_on_exit (Last) -> terminal restore
//! (before process::exit)
//! ```
//!
//! # Required ordering
//!
//! - This plugin must be added **after** [`sc_command::SCCommandPlugin`],
//!   **after** [`sc_world::SCWorldPlugin`], and **before** [`sc_game::SCGamePlugin`] in
//!   `build_app` (see `sc_bootstrap::build_app`):
//!   `Last` runs in registration order, so the exit sequence must be chunk flush -> this file's
//!   shutdown system restoring the terminal -> `std::process::exit(0)` in `on_exit_shutdown_regions`.
//!   Flushing must precede terminal restore: only a live TUI can show flush completion;
//!   restoring first would hide flushing in the background, looking like an instant exit (data still saved).
//!   Terminal restore must precede `process::exit(0)`, otherwise a standalone exit leaves the terminal garbled
//!   alternate screen / raw mode.
//! - Terminal restore also has an `atexit` fallback inside `sc_console`; this system is the primary path.
//!
//! # Backpressure
//!
//! - Console to game: bounded channel (`ConsoleConfig::input_queue`, default 128), draining at most
//!   [`DRAIN_PER_TICK`] lines per tick with the remainder left for later ticks;
//! - Game to console logs: `try_send` never blocks (drops and counts when full, visible in the TUI status bar).

use sc_log::t_log;
use std::sync::{Arc, Mutex, OnceLock};

use sc_command::events::{CommandOrigin, RawCommandInput};
use sc_command::registry::CommandRegistry;
use sc_console::{
    install_logger, shared_provider, spawn_console_hosted, CommandEntry, ConsoleConfig,
    ConsoleDriver, ConsoleHandle, ConsoleHeader, ResolvedMode, StaticCompletionProvider,
    WelcomeChoice,
};
use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::ResMut;
use sc_ecs::resource::Resource;
use sc_ecs::world::World;
use sc_packloader::version_control::SCVersionPack;
use sc_utils::event::SCExit;
use sc_utils::game::structs::server::Server;
use sc_utils::game::structs::server_properties::ServerProperties;

/// Max console lines drained per tick (bounded, avoids long single-tick occupation).
const DRAIN_PER_TICK: usize = 8;

struct BridgeInner {
    driver: Option<ConsoleDriver>,
}

static BRIDGE: OnceLock<Mutex<BridgeInner>> = OnceLock::new();

fn bridge() -> &'static Mutex<BridgeInner> {
    BRIDGE.get_or_init(|| Mutex::new(BridgeInner { driver: None }))
}

fn lock_bridge() -> std::sync::MutexGuard<'static, BridgeInner> {
    bridge()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Plugin entry (see the module docs for ordering).
pub struct ConsoleBridgePlugin;

impl Plugin for ConsoleBridgePlugin {
    fn build(&self, app: &App) {
        init_console();
        app.insert_resource(ConsoleMetaCache::default())
            .add_systems(sc_ecs::schedule::PreUpdate, drain_console_commands)
            .add_systems(sc_ecs::schedule::PreUpdate, sync_console_meta)
            // Unlocks TUI input once startup completes (PostStartup runs once; this system runs
            // after bootstrap finish_startup, ordered by plugin registration).
            .add_systems(sc_ecs::schedule::PostStartup, mark_server_running)
            .add_systems(sc_ecs::schedule::Last, shutdown_console_on_exit);
        // Welcome-screen gate: GUI plus welcome screen blocks until a keypress, deferring server reload
        // (world/save/network-listen startup systems) until after it; other modes see `welcome_done()`
        // as always true and pass through. Set `SC_CONSOLE_WELCOME=0` for automated starts
        // to skip the screen (otherwise a TTY waits for a keypress).
        wait_for_welcome_gate();
        // Welcome choice (if the selector confirmed one): persist into
        // `server_properties.toml` now — this runs in `build`, before PreStartup
        // `init_properties` loads the file, so the server reads the choice.
        // The same choice drives the quote fetch below.
        let choice = console_handle().and_then(|h| h.take_welcome_choice());
        apply_console_language_choice(choice.clone());
        maybe_start_hitokoto(choice);
    }
}

/// Blocks until the welcome screen ends (see [`ConsoleHandle::welcome_done`]).
///
/// Waits only when a welcome screen faces the user; releases immediately when the console
/// requests exit, never wedging shutdown.
fn wait_for_welcome_gate() {
    let Some(handle) = console_handle() else {
        return;
    };
    if handle.welcome_done() {
        return;
    }
    // Never `eprintln!` here: the TUI owns the alternate screen, so direct terminal writes
    // would flash under the welcome screen. Use the log channel, visible in the log pane
    // once inside the console.
    log::info!("{}", t_log!("console.console.wait_welcome"));
    while !handle.welcome_done() {
        if !handle.is_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Best-effort read of `[server] language` (the TUI's render seed at spawn).
///
/// Failures fall back to the console default and are reported later by
/// `init_properties`, which owns config errors.
fn read_properties_language() -> Option<String> {
    let mut path = std::env::current_dir().ok()?;
    path.push("server_properties.toml");
    ServerProperties::load(&path).ok().map(|p| p.language)
}

/// Persist the welcome choice (if any) into `server_properties.toml`.
///
/// Single source of truth for both ids: the console thread never touches the
/// file (no TOML there, comments must survive); the host applies the one-shot
/// choice handed over via the handle. `take` is done by the caller so the same
/// choice also drives the quote fetch below.
fn apply_console_language_choice(choice: Option<sc_console::WelcomeChoice>) {
    let Some(choice) = choice else {
        return;
    };
    let code = choice.lang;
    // The server only accepts these today (`ServerProperties::load` validates);
    // never write anything else, or the next boot breaks on config load.
    if code != "zh-CN" && code != "en-US" {
        log::warn!("{}", t_log!("console.console.lang_unsupported", lang = code));
        return;
    }
    sc_log::set_locale(&code);
    std::env::set_var("SCULK_LOCALE", &code);
    // The checkbox counts only for Chinese (the console already forced English
    // confirms off); written as a bare TOML boolean.
    let hitokoto = if choice.hitokoto { "true" } else { "false" };
    let path = std::env::current_dir()
        .map(|mut d| {
            d.push("server_properties.toml");
            d
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("server_properties.toml"));
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            log::warn!("{}", t_log!("console.console.lang_no_file", lang = code));
            return;
        }
    };
    let Some(new) = patch_server_keys(&old, &[("language", &code), ("enable_hitokoto", hitokoto)]) else {
        log::warn!(
            "{}",
            t_log!(
                "console.console.lang_patch_fail",
                error = "no [server] section"
            )
        );
        return;
    };
    if new == old {
        return; // already set, no write churn
    }
    match std::fs::write(&path, new) {
        Ok(()) => log::info!("{}", t_log!("console.console.lang_saved", lang = code)),
        Err(error) => log::warn!(
            "{}",
            t_log!("console.console.lang_patch_fail", error = error)
        ),
    }
}

/// Rewrite `[server]` keys inside existing TOML text, preserving everything
/// else byte-for-byte (comments, order, formatting).
///
/// Each pair is `key = value` under `[server]` (booleans pass `"true"` /
/// `"false"` and are written bare): an existing key has its value replaced
/// with the trailing comment kept; a missing key is inserted right after the
/// section header, in pair order. No `[server]` section: `None` (the file is
/// unloadable anyway; the caller warns instead of fabricating sections).
fn patch_server_keys(text: &str, pairs: &[(&str, &str)]) -> Option<String> {
    /// Split a line into code plus trailing `#` comment (outside quotes).
    fn split_comment(line: &str) -> (&str, &str) {
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for (i, c) in line.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (Some('"'), '\\') => escaped = true,
                (None, '#') => return (&line[..i], &line[i..]),
                _ => {}
            }
        }
        (line, "")
    }

    /// Render one value: booleans bare, everything else double-quoted.
    fn render_value(value: &str) -> String {
        if value == "true" || value == "false" {
            value.to_string()
        } else {
            format!("\"{value}\"")
        }
    }

    /// Index of the first `[server]` header (`None` = unloadable file).
    fn server_header(lines: &[&str]) -> Option<usize> {
        lines.iter().position(|l| l.trim() == "[server]")
    }

    /// Index of `key` under `[server]` in the working copy (`None` = insert).
    fn server_key(out: &[String], key: &str) -> Option<usize> {
        let mut in_server = false;
        for (i, line) in out.iter().enumerate() {
            let t = line.trim();
            if t.starts_with('[') {
                in_server = t == "[server]";
                continue;
            }
            if !in_server {
                continue;
            }
            let (code_part, _) = split_comment(line);
            if let Some((k, _)) = code_part.split_once('=') {
                if k.trim() == key {
                    return Some(i);
                }
            }
        }
        None
    }

    let trailing_newline = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let server_at = server_header(&lines)?;
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let mut inserted = 0usize;
    for (key, value) in pairs {
        match server_key(&out, key) {
            Some(i) => {
                let line = out[i].clone();
                let (code_part, comment) = split_comment(&line);
                let indent: String = code_part
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .collect();
                // One space before a kept trailing comment (normalized).
                let comment = if comment.is_empty() {
                    String::new()
                } else {
                    format!(" {comment}")
                };
                out[i] = format!("{indent}{key} = {}{comment}", render_value(value));
            }
            None => {
                out.insert(
                    server_at + 1 + inserted,
                    format!("{key} = {}", render_value(value)),
                );
                inserted += 1;
            }
        }
    }
    let mut text = out.join("\n");
    if trailing_newline {
        text.push('\n');
    }
    Some(text)
}

/// Metadata synced to the TUI: completion snapshot version plus top bar (core/version-pack versions)
/// plus the online player list (side panel).
#[derive(Resource, Default)]
struct ConsoleMetaCache {
    revision: u64,
    core: String,
    pack: String,
    players: Vec<String>,
}

/// Starts the console thread (idempotent; reuses it when already running).
fn init_console() {
    {
        let slot = lock_bridge();
        if slot
            .driver
            .as_ref()
            .is_some_and(|driver| driver.handle().is_running())
        {
            return;
        }
    }
    let mut config = ConsoleConfig::from_env();
    // TUI render seed: `[server] language` when the file already says so
    // (env override in `config.lang` still wins downstream, and the welcome
    // selector can still change it for this run).
    if config.initial_lang.is_none() {
        config.initial_lang = read_properties_language();
    }
    let hosted = sc_utils::host_mode::is_hosted();
    match spawn_console_hosted(config, hosted) {
        Ok(driver) => {
            let handle = driver.handle();
            let gui = driver.mode() == ResolvedMode::Gui;
            lock_bridge().driver = Some(driver);
            if gui {
                // File-sink callback: matches the pretty_env_logger file line format
                // (`timestamp level target message`), silently dropped without a sink.
                let extra: sc_console::ExtraSink = Arc::new(|level, target, message| {
                    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f");
                    sc_log::file::write_line_with_level(
                        format!("{ts} {level:<5} {target} {message}\n"),
                        level,
                    );
                });
                match install_logger(&handle, Some(extra)) {
                    // A global logger exists right after install, so this line enters the TUI log pane directly.
                    Ok(()) => log::info!("{}", t_log!("console.console.gui_started")),
                    Err(_) => {
                        log::warn!("{}", t_log!("console.console.logger_exists"))
                    }
                }
            } else {
                // No global logger is installed yet (startup systems install it), so reports the mode
                // choice via stderr, following the early-bootstrap eprintln plus log convention.
                eprintln!("[console_bridge] 控制台非 GUI 模式（plain 输入 / off 关闭）");
                log::info!("{}", t_log!("console.console.plain_mode"));
            }
        }
        Err(error) => {
            eprintln!("[console_bridge] 控制台启动失败: {error}");
        }
    }
}

/// Start the daily-quote subtitle when enabled (post-gate; GUI only).
///
/// Source of truth: this run's selector checkbox, else `server.properties`
/// `[server] enable_hitokoto` (default off). Fetch is skipped entirely when
/// off — no API call, no rotation thread, the title row stays empty.
/// Quote: synchronously fetches once (1s timeout) so the first frame is
/// already correct with no later flicker, falling back to a builtin; a
/// background thread then rotates every 30s. The TUI renders it with a
/// typewriter animation, replayed only on content change.
fn maybe_start_hitokoto(choice: Option<WelcomeChoice>) {
    let on = match choice {
        Some(c) => c.hitokoto,
        None => read_properties_hitokoto(),
    };
    if !on {
        return;
    }
    let Some(handle) = console_handle() else {
        return;
    };
    let gui = lock_bridge()
        .driver
        .as_ref()
        .is_some_and(|d| d.mode() == ResolvedMode::Gui);
    if !gui {
        return;
    }
    let first =
        fetch_hitokoto(std::time::Duration::from_secs(1)).unwrap_or_else(|| pick_fallback_hitokoto().to_string());
    handle.set_subtitle(&first);
    let hitokoto_handle = handle.clone();
    let _ = std::thread::Builder::new()
        .name("sc-hitokoto".into())
        .spawn(move || {
            hitokoto_loop(hitokoto_handle);
        });
}

/// Best-effort read of `[server] enable_hitokoto` (default off on any failure).
fn read_properties_hitokoto() -> bool {
    let mut path = match std::env::current_dir() {
        Ok(mut d) => {
            d.push("server_properties.toml");
            d
        }
        Err(_) => return false,
    };
    ServerProperties::load(&path).map(|p| p.enable_hitokoto).unwrap_or(false)
}

/// Builtin quote fallbacks (used when the API is unreachable; rotated regularly).
const FALLBACK_HITOKOTO: &[&str] = &[
    "「只想去往山野和清风来邂逅」",
    "「心有山海，静而不争」",
    "「风起于青萍之末」",
    "「行到水穷处，坐看云起时」",
];

fn pick_fallback_hitokoto() -> &'static str {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    FALLBACK_HITOKOTO[(secs as usize) % FALLBACK_HITOKOTO.len()]
}

/// Quote rotation loop (background thread): startup already fetched the first entry; rotates every 30s here.
///
/// Uses the API on success, otherwise the next builtin in order (guarantees a fresh line each half minute).
/// The thread dies with the process, no explicit stop needed; unchanged `set_subtitle` content replays no animation.
fn hitokoto_loop(handle: ConsoleHandle) {
    let mut fallback_idx = 0usize;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
        match fetch_hitokoto(std::time::Duration::from_secs(4)) {
            Some(text) => handle.set_subtitle(&text),
            None => {
                handle.set_subtitle(FALLBACK_HITOKOTO[fallback_idx % FALLBACK_HITOKOTO.len()]);
                fallback_idx = fallback_idx.wrapping_add(1);
            }
        }
    }
}

/// Fetches a quote (`https://v1.hitokoto.cn/?c=f`), returning `None` on failure.
fn fetch_hitokoto(timeout: std::time::Duration) -> Option<String> {
    let config = ureq::config::Config::builder()
        .timeout_global(Some(timeout))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let body = agent
        .get("https://v1.hitokoto.cn/?c=f")
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let text = value.get("hitokoto")?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if text.contains('「') || text.contains('」') {
        Some(text.to_string())
    } else {
        Some(format!("「{text}」"))
    }
}

/// Current console handle (`None` while the TUI is down).
pub fn console_handle() -> Option<ConsoleHandle> {
    lock_bridge().driver.as_ref().map(ConsoleDriver::handle)
}

/// Non-blockingly drains up to `max` lines of user input.
fn drain_lines(max: usize) -> Vec<String> {
    let slot = lock_bridge();
    let mut out = Vec::new();
    if let Some(driver) = slot.driver.as_ref() {
        while out.len() < max {
            match driver.try_recv_line() {
                Some(line) => out.push(line),
                None => break,
            }
        }
    }
    out
}

/// Restores the terminal and reaps the console thread (idempotent).
pub fn shutdown_console() {
    let driver = lock_bridge().driver.take();
    // `ConsoleDriver::drop` restores the terminal and joins the thread.
    drop(driver);
}

/// PreUpdate: console input to game-boundary events (`CommandOrigin::Console`).
///
/// Performs mechanics-only conversion (empty lines already filtered by the driver); permissions/aliases/tokenizing
/// are handled later the same tick by `dispatch_commands` (dispatch runs in `SCConnectionUpdate`, see schedule).
fn drain_console_commands(world: World) {
    for raw in drain_lines(DRAIN_PER_TICK) {
        world.send_event(RawCommandInput {
            origin: CommandOrigin::Console,
            raw,
            request: None,
        });
    }
}

/// PreUpdate: pushes metadata snapshots (completion plus top version bar) to the TUI, only on change.
///
/// - Completion: a `CommandRegistry::revision` change rebuilds the snapshot; plugin commands registered
///   at `enable` time appear in completion automatically without a console restart;
/// - Version bar: core version (`Server::global`) and pack tag (`SCVersionPack::manifest`)
///   push once ready (late pack loads fill in on the first valued tick).
fn sync_console_meta(world: World, mut cache: ResMut<ConsoleMetaCache>) {
    let Some(handle) = console_handle() else {
        return;
    };
    if let Some(registry) = world.get_resource::<CommandRegistry>() {
        if registry.revision() != cache.revision {
            cache.revision = registry.revision();
            let entries: Vec<CommandEntry> = registry
                .iter()
                .map(|definition| CommandEntry {
                    name: definition.name.clone(),
                    aliases: definition.aliases.clone(),
                    description: definition.description.clone(),
                })
                .collect();
            handle.set_provider(shared_provider(StaticCompletionProvider::new(entries)));
        }
    }
    let core = Server::global()
        .map(|server| server.server_version.clone())
        .unwrap_or_default();
    let pack = world
        .get_resource::<SCVersionPack>()
        .map(|pack| {
            let manifest = &pack.manifest;
            format!(
                "{} {} · protocol {}",
                manifest.name,
                manifest.network_version(),
                manifest.protocol_version
            )
        })
        .unwrap_or_default();
    if core != cache.core || pack != cache.pack {
        cache.core.clone_from(&core);
        cache.pack.clone_from(&pack);
        handle.set_header(ConsoleHeader::new(&core, &pack));
    }
    // Online players (`DisplayName` mounts only at player spawn, so entities are online players):
    // pushes only on sorted change (panel refreshes on join/leave, zero cost otherwise).
    let mut players: Vec<String> = world
        .entities_with_component::<sc_utils::components::DisplayName>()
        .iter()
        .filter_map(|entity| world.get_component::<sc_utils::components::DisplayName>(entity))
        .map(|name| name.0.clone())
        .collect();
    players.sort();
    if players != cache.players {
        cache.players.clone_from(&players);
        handle.set_online_players(players);
    }
}

/// PostStartup (once): server startup done, so the TUI plays the green sweep and unlocks input.
fn mark_server_running() {
    if let Some(handle) = console_handle() {
        handle.set_server_running();
    }
}

/// Last: restores the terminal on `SCExit` (must run before the game domain `process::exit`,
/// ordered by plugin registration, see module docs).
fn shutdown_console_on_exit(mut reader: EventReader<SCExit>) {
    if reader.read().next().is_some() {
        shutdown_console();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_replaces_value_and_keeps_comment() {
        let old = "[server]\nenable_snappy = false\nlanguage = \"zh-CN\" # console language\nxbox_auth = false\n";
        let new =
            patch_server_keys(old, &[("language", "en-US")]).expect("patched");
        assert!(new.contains("language = \"en-US\" # console language"), "{new:?}");
        assert!(new.contains("enable_snappy = false"), "untouched");
        assert!(new.contains("xbox_auth = false"), "untouched");
        assert!(new.ends_with('\n'), "trailing newline preserved");
    }

    #[test]
    fn patch_inserts_missing_key_after_header() {
        let old = "[server]\nenable_snappy = false\n\n[game]\nname = \"x\"\n";
        let new =
            patch_server_keys(old, &[("language", "en-US")]).expect("patched");
        let mut lines = new.lines();
        assert_eq!(lines.next(), Some("[server]"));
        assert_eq!(lines.next(), Some("language = \"en-US\""));
        assert!(new.contains("[game]"), "other sections kept");
    }

    #[test]
    fn patch_ignores_other_sections_and_missing_header() {
        let other = "[game]\nlanguage = \"xx\"\n";
        assert_eq!(patch_server_keys(other, &[("language", "en-US")]), None);
        // A `language` lookalike elsewhere (e.g. motd text) is not touched.
        let tricky = "[server]\nmotd = \"my language = x\"\nlanguage=\"zh-CN\"\n";
        let new =
            patch_server_keys(tricky, &[("language", "en-US")]).expect("patched");
        assert!(new.contains("motd = \"my language = x\""), "{new:?}");
        assert!(new.contains("language = \"en-US\""), "{new:?}");
    }

    #[test]
    fn patch_without_trailing_newline_stays_without() {
        let old = "[server]\nlanguage = \"zh-CN\"";
        let new =
            patch_server_keys(old, &[("language", "en-US")]).expect("patched");
        assert_eq!(new, "[server]\nlanguage = \"en-US\"");
    }

    #[test]
    fn patch_writes_booleans_bare_and_keeps_pair_order() {
        // The welcome confirm writes language plus the quote switch together.
        let old = "[server]\nlanguage = \"en-US\"\n";
        let new = patch_server_keys(
            old,
            &[("language", "zh-CN"), ("enable_hitokoto", "true")],
        )
        .expect("patched");
        assert!(new.contains("language = \"zh-CN\""), "{new:?}");
        assert!(new.contains("enable_hitokoto = true"), "{new:?} bare bool");
        assert!(!new.contains("\"true\""), "bools are never quoted");
        // Both keys land under [server] (TOML is order-free; existing keys
        // stay in place, missing keys go right under the header).
        let server_at = new.find("[server]").expect("section");
        assert!(new.find("language = ").unwrap() > server_at);
        assert!(new.find("enable_hitokoto = ").unwrap() > server_at);
        // Existing bool flips in place with its comment kept.
        let old2 = "[server]\nenable_hitokoto = true # quote subtitle\nlanguage = \"zh-CN\"\n";
        let new2 = patch_server_keys(
            old2,
            &[("language", "zh-CN"), ("enable_hitokoto", "false")],
        )
        .expect("patched");
        assert!(
            new2.contains("enable_hitokoto = false # quote subtitle"),
            "{new2:?}"
        );
    }
}
