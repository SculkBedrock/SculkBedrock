//! 插件到宿主的日志桥接。
//!
//! cdylib 插件静态链接自己的 `log` crate 副本，其全局 logger 未初始化，插件内的
//! `log::info!` 调用会被丢弃。本模块通过 C-ABI 函数指针把插件侧日志调用转发给宿主
//! 进程的 logger：
//!
//! - 宿主侧：`host_log_bridge`（转发器）加加载期注入（loader 调用插件导出的
//!   `sc_plugin_log_bridge` 符号）；
//! - 插件侧：`declare_log_bridge!()` 宏（生成导出符号）加 `install`
//!   （把 `BridgeLogger` 装为全局 logger）。
//!
//! 桥接只以三段 UTF-8 字节串传递 level/target/message，不传递 Rust 类型，因此可跨
//! DLL 边界使用（其他语言的插件可实现同一协议）。
//!
//! Plugin-to-host log bridge.
//!
//! A cdylib plugin statically links its own copy of the `log` crate whose global logger
//! is uninitialized, so `log::info!` calls inside the plugin are dropped. This module
//! forwards plugin-side log calls to the host process logger through a C-ABI function pointer:
//!
//! - Host side: `host_log_bridge` (forwarder) plus load-time injection (the loader calls
//!   the plugin-exported `sc_plugin_log_bridge` symbol);
//! - Plugin side: the `declare_log_bridge!()` macro (emits the export symbol) plus
//!   `install` (installs `BridgeLogger` as the global logger).
//!
//! The bridge only passes level/target/message as three UTF-8 byte strings with no Rust
//! types, so it is safe across DLL boundaries (plugins in other languages can implement
//! the same protocol).

use std::sync::RwLock;

/// 桥接函数签名（C ABI）。
///
/// - `level`：1=error 2=warn 3=info 4=debug 5=trace（其余按 trace 处理）；
/// - `target`/`msg`：UTF-8 字节串指针加长度，仅在本次调用期间有效。
///
/// Bridge function signature (C ABI).
///
/// - `level`: 1=error 2=warn 3=info 4=debug 5=trace (others handled as trace);
/// - `target`/`msg`: UTF-8 byte-string pointer plus length, valid only for the call.
pub type LogBridgeFn = extern "C" fn(
    level: u8,
    target: *const u8,
    target_len: usize,
    msg: *const u8,
    msg_len: usize,
);

/// 日志字节转 log crate 的 Level。
///
/// Log byte to log-crate Level.
fn level_from_u8(level: u8) -> log::Level {
    match level {
        1 => log::Level::Error,
        2 => log::Level::Warn,
        3 => log::Level::Info,
        4 => log::Level::Debug,
        _ => log::Level::Trace,
    }
}

/// 宿主转发器：把插件日志转发到宿主日志器。
///
/// Host forwarder: plugin logs to host logger.
///
/// Injected into the plugin by the loader; any plugin thread may call back through the pointer.
pub extern "C" fn host_log_bridge(
    level: u8,
    target: *const u8,
    target_len: usize,
    msg: *const u8,
    msg_len: usize,
) {
    fn read_str(ptr: *const u8, len: usize) -> String {
        if ptr.is_null() || len == 0 {
            return String::new();
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        String::from_utf8_lossy(bytes).into_owned()
    }
    let target = read_str(target, target_len);
    let msg = read_str(msg, msg_len);
    log::log!(target: &target, level_from_u8(level), "{msg}");
}

// ---------------------------------------------------------------------------
// Plugin side
// ---------------------------------------------------------------------------

static BRIDGE: RwLock<Option<LogBridgeFn>> = RwLock::new(None);

/// Plugin name (injected by the host, from manifest.name) used as the forwarded log target,
/// so host output reads `... sc_vanilla_overworld >> message`, identifying the source plugin at a glance.
static PLUGIN_NAME: RwLock<String> = RwLock::new(String::new());

/// 插件侧 logger：把日志调用转发给宿主注入的桥接函数。
///
/// Plugin-side logger: forwards log calls to the host-injected bridge function.
struct BridgeLogger;

impl log::Log for BridgeLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Filtering stays on the host (the bridge always forwards).
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        let level = match record.level() {
            log::Level::Error => 1,
            log::Level::Warn => 2,
            log::Level::Info => 3,
            log::Level::Debug => 4,
            log::Level::Trace => 5,
        };
        let Some(bridge) = BRIDGE.read().ok().and_then(|guard| *guard) else {
            // Not injected: fall back to stderr so logs are never silently lost.
            eprintln!("[plugin:{}:{}] {}", record.target(), record.level(), record.args());
            return;
        };
        let name = PLUGIN_NAME
            .read()
            .map(|n| n.clone())
            .unwrap_or_else(|_| record.target().to_string());
        // target = plugin name; msg gets a ">> " prefix (host format `{ts} {level} {target} {msg}`).
        let target = name.as_bytes();
        let msg = format!(">> {}", record.args());
        let msg = msg.as_bytes();
        bridge(
            level,
            target.as_ptr(),
            target.len(),
            msg.as_ptr(),
            msg.len(),
        );
    }

    fn flush(&self) {}
}

/// 安装桥接函数并接管插件内 `log` 全局日志器。
///
/// Installs the bridge function and takes over the in-plugin `log` global logger.
///
/// Idempotent: repeat calls update the bridge function and plugin name; a logger-install failure (already taken) is ignored.
pub fn install(bridge: LogBridgeFn, name: &str) {
    if let Ok(mut slot) = BRIDGE.write() {
        *slot = Some(bridge);
    }
    if let Ok(mut slot) = PLUGIN_NAME.write() {
        *slot = name.to_string();
    }
    // Skip reinstalling once installed (repeat loads of one DLL each own an independent static copy).
    if log::set_boxed_logger(Box::new(BridgeLogger)).is_ok() {
        log::set_max_level(log::LevelFilter::Trace);
    }
}

/// Call at the plugin crate root: emits the export symbol the host uses to inject the log bridge.
///
/// After loading the DLL, the host loader calls `sc_plugin_log_bridge(bridge, name_ptr, name_len)`
/// to inject the host forwarder and the plugin name (manifest.name).
///
/// ```ignore
/// sc_plugin::declare_log_bridge!();
/// ```
#[macro_export]
macro_rules! declare_log_bridge {
    () => {
        #[no_mangle]
        pub extern "C" fn sc_plugin_log_bridge(
            bridge: $crate::log_bridge::LogBridgeFn,
            name: *const u8,
            name_len: usize,
        ) {
            let name = if name.is_null() || name_len == 0 {
                String::new()
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(name, name_len) };
                String::from_utf8_lossy(bytes).into_owned()
            };
            $crate::log_bridge::install(bridge, &name);
        }
    };
}
