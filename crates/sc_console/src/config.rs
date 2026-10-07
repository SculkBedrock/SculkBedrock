//! Console configuration: mode selection plus all bounded budgets.
//!
//! Environment variables (read by `ConsoleConfig::from_env`; explicit construction wins):
//!
//! - `SC_CONSOLE` / `SC_CONSOLE_MODE`: `auto | gui | plain | off`
//!   (default `auto`; `SC_CONSOLE` wins when both are set).
//! - `SC_CONSOLE_BOOT_ANIM`: boot animation switch, `1/true/on/yes` enables,
//!   `0/false/off/no/none/disabled` disables (default on; unknown values warn and keep the default).
//! - `SC_CONSOLE_WELCOME`: welcome screen switch (same booleans, default on; off enters the console directly).
//! - `SC_CONSOLE_LANG`: console UI language (`en-US` / `zh-CN`, default `en-US`;
//!   skips the welcome language selector; first-run prompt still gates server load).

/// Canonical console locale codes.
pub const LANG_EN_US: &str = "en-US";
pub const LANG_ZH_CN: &str = "zh-CN";
/// Default console language: the welcome screen boots in English until the user picks.
pub const DEFAULT_LANG: &str = LANG_EN_US;
/// Supported console locales, in selector order (adding a language: append the
/// code here plus a whole `locales/{code}.yml` with the full `tui.*` key set;
/// missing keys fall back to en-US, and the completeness test fails until whole).
pub const SUPPORTED_LANGS: &[&str] = &[LANG_EN_US, LANG_ZH_CN];
/// Env override for the console language (never persisted).
pub(crate) const LANG_ENV: &str = "SC_CONSOLE_LANG";
/// Sidecar marker in the working directory: existence means the user already
/// chose a console language on the welcome screen (later launches skip
/// press-any-key and the selector and auto-enter after the animation).
/// Delete it to choose again.
///
/// This marker carries NO language id (empty file): the id lives only in
/// `server_properties.toml` (`[server] language`), which the TUI reads via
/// the host (`ConsoleConfig::initial_lang`) and updates through the host on
/// confirm. The console thread never parses or rewrites host-owned config.
pub(crate) const MARKER_FILE_NAME: &str = ".sc_initialled";

/// Normalize user input to a supported canonical code (`None` = unsupported).
///
/// Case/separator-insensitive (`en_us` works); `en`/`zh` are conveniences for
/// the two built-ins, new languages use their full codes.
pub fn normalize_lang(s: &str) -> Option<String> {
    let n = s.trim().replace('_', "-");
    let lo = n.to_ascii_lowercase();
    if lo == "en" || lo == "us" || lo == "english" {
        return Some(LANG_EN_US.to_string());
    }
    if lo == "zh" || lo == "cn" || lo == "chinese" || lo == "simplified" || n.trim() == "中文" || n.contains("简体") {
        return Some(LANG_ZH_CN.to_string());
    }
    SUPPORTED_LANGS
        .iter()
        .find(|c| c.to_ascii_lowercase() == lo)
        .map(|c| c.to_string())
}

/// Marker path (working-directory anchor; `None` when undeterminable).
pub(crate) fn marker_file_path() -> Option<std::path::PathBuf> {
    std::env::current_dir().ok().map(|d| d.join(MARKER_FILE_NAME))
}

/// Marker check at an explicit path (unit-testable; production uses [`has_chosen_marker`]).
pub(crate) fn marker_exists_at(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok()
}

/// Create the marker at an explicit path (empty file, overwrites).
pub(crate) fn create_marker_at(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(path, "")
}

/// Whether the user already chose (working-directory marker present).
pub(crate) fn has_chosen_marker() -> bool {
    marker_file_path().is_some_and(|p| marker_exists_at(&p))
}

/// Record the choice (working-directory marker; the caller warns on failure).
pub(crate) fn create_chosen_marker() -> std::io::Result<()> {
    match marker_file_path() {
        Some(p) => create_marker_at(&p),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "working directory unavailable",
        )),
    }
}

/// Resolve the effective language: env/code override > host-provided initial
/// (from `server_properties.toml`) > `en-US`.
pub(crate) fn resolve_lang(forced: Option<String>, initial: Option<String>) -> String {
    forced
        .and_then(|s| normalize_lang(&s))
        .or_else(|| initial.and_then(|s| normalize_lang(&s)))
        .unwrap_or_else(|| DEFAULT_LANG.to_string())
}

/// Selector order: supported codes that actually loaded, mains first.
///
/// `available_locales!` resolves against this crate's `i18n!` table, so a code
/// listed without its yml never appears (honest by construction).
pub(crate) fn selector_order() -> Vec<String> {
    let loaded: Vec<String> = rust_i18n::available_locales!()
        .into_iter()
        .map(|l| l.to_string())
        .collect();
    SUPPORTED_LANGS
        .iter()
        .filter(|c| loaded.iter().any(|l| l.as_str() == **c))
        .map(|c| c.to_string())
        .collect()
}

/// Console run mode (explicitly selected).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConsoleMode {
    /// Auto: hosted mode turns off; TTY uses the GUI; otherwise plain-text stdin.
    #[default]
    Auto,
    /// Force the TUI (falls back to plain text with a warning when not a TTY).
    Gui,
    /// Plain-text stdin line reading (no TUI, terminal untouched).
    Plain,
    /// Fully off (when the host UI owns input).
    Off,
}

impl ConsoleMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "gui" | "tui" => Some(Self::Gui),
            "plain" | "line" | "stdin" => Some(Self::Plain),
            "off" | "none" | "disabled" | "0" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Resolved effective run mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedMode {
    Gui,
    Plain,
    Off,
}

/// All queue/cache caps (architecture rule: every queue/cache needs limits).
#[derive(Clone, Debug)]
pub struct ConsoleConfig {
    pub mode: ConsoleMode,
    /// Log window retained rows (ring-truncated).
    pub max_log_lines: usize,
    /// Input history entries.
    pub max_history: usize,
    /// Per-line input char cap (overflows truncate with a notice).
    pub max_input_chars: usize,
    /// Console-to-host pending command queue capacity.
    pub input_queue: usize,
    /// Log-to-console pending render queue capacity (drops and counts when full).
    pub log_queue: usize,
    /// Completion popup display cap.
    pub max_completions: usize,
    /// UI frame interval (event poll timeout).
    pub frame_ms: u64,
    /// Per-tick/per-frame drain cap (shared by log rendering and input send; avoids long stalls).
    pub drain_per_frame: usize,
    /// TUI boot animation switch (default on): clears the screen first, then the title/input bars
    /// ease out from left to edge, followed by the title typewriter plus CPU/MEM fade-in
    /// (input content fades in together without waiting for the title; the normal boot ripple follows after ~1.1s).
    /// When off, enters the normal boot state directly (no stretch/type/fade).
    pub boot_anim: bool,
    /// Welcome screen switch (default on): the GUI first shows a fullscreen logo with flowing background,
    /// entering the console on any key (boot animation still plays). Ineffective in plain/off modes.
    pub welcome: bool,
    /// Console UI language override (canonical code, e.g. `en-US`), usually from
    /// `SC_CONSOLE_LANG`: `Some` skips the welcome language selector (ephemeral,
    /// never persisted; the first-run prompt still gates server load).
    pub lang: Option<String>,
    /// Initial console language from the host (`server_properties.toml`
    /// `[server] language`, best-effort read at spawn; `None` falls back to `en-US`).
    /// Render seed only: the selector still shows on first run unless `lang`
    /// overrides or the `.sc_initialled` marker exists. Standalone users leave `None`.
    pub initial_lang: Option<String>,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            mode: ConsoleMode::Auto,
            max_log_lines: 2000,
            max_history: 100,
            max_input_chars: 512,
            input_queue: 128,
            log_queue: 1024,
            max_completions: 9,
            frame_ms: 50,
            drain_per_frame: 64,
            boot_anim: true,
            welcome: true,
            lang: None,
            initial_lang: None,
        }
    }
}

/// Parse a boolean switch (for `SC_CONSOLE_BOOT_ANIM`; case-insensitive).
///
/// Pure function for easy unit tests: true values are `1/true/yes/on`, false values
/// are `0/false/no/off/none/disabled`; anything else returns `None` (caller warns and keeps the default).
pub fn parse_bool_switch(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" | "none" | "disabled" => Some(false),
        _ => None,
    }
}

impl ConsoleConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        // Language first: later warnings render in the chosen language.
        let lang_raw = std::env::var(LANG_ENV).unwrap_or_default();
        if !lang_raw.is_empty() {
            match normalize_lang(&lang_raw) {
                Some(code) => cfg.lang = Some(code),
                None => eprintln!(
                    "{}",
                    rust_i18n::t!("tui.unknown_lang", locale = DEFAULT_LANG, v = lang_raw)
                ),
            }
        }
        // Warning locale: explicit env choice, else the default (the saved file
        // is read later by the console thread, not here).
        let wl = cfg.lang.as_deref().unwrap_or(DEFAULT_LANG);
        let raw = std::env::var("SC_CONSOLE")
            .or_else(|_| std::env::var("SC_CONSOLE_MODE"))
            .unwrap_or_default();
        if !raw.is_empty() {
            match ConsoleMode::parse(&raw) {
                Some(mode) => cfg.mode = mode,
                None => eprintln!("{}", rust_i18n::t!("tui.unknown_console", locale = wl, v = raw)),
            }
        }
        let boot_raw = std::env::var("SC_CONSOLE_BOOT_ANIM").unwrap_or_default();
        if !boot_raw.is_empty() {
            match parse_bool_switch(&boot_raw) {
                Some(on) => cfg.boot_anim = on,
                None => eprintln!(
                    "{}",
                    rust_i18n::t!("tui.unknown_boot", locale = wl, v = boot_raw)
                ),
            }
        }
        let welcome_raw = std::env::var("SC_CONSOLE_WELCOME").unwrap_or_default();
        if !welcome_raw.is_empty() {
            match parse_bool_switch(&welcome_raw) {
                Some(on) => cfg.welcome = on,
                None => eprintln!(
                    "{}",
                    rust_i18n::t!("tui.unknown_welcome", locale = wl, v = welcome_raw)
                ),
            }
        }
        cfg
    }

    /// Resolve the final mode for the host environment.
    ///
    /// - `hosted`: the host UI (Tauri/mobile) already owns input/output, so always `Off`;
    /// - `stdin_tty`: whether stdin is a terminal (`std::io::IsTerminal`);
    /// - `Gui` falls back to `Plain` when not a TTY (caller should warn once).
    pub fn resolve(&self, hosted: bool, stdin_tty: bool) -> ResolvedMode {
        if hosted {
            return ResolvedMode::Off;
        }
        match self.mode {
            ConsoleMode::Off => ResolvedMode::Off,
            ConsoleMode::Plain => ResolvedMode::Plain,
            ConsoleMode::Gui => {
                if stdin_tty {
                    ResolvedMode::Gui
                } else {
                    ResolvedMode::Plain
                }
            }
            ConsoleMode::Auto => {
                if stdin_tty {
                    ResolvedMode::Gui
                } else {
                    ResolvedMode::Plain
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_gui_on_tty() {
        let cfg = ConsoleConfig::default();
        assert_eq!(cfg.resolve(false, true), ResolvedMode::Gui);
        assert_eq!(cfg.resolve(false, false), ResolvedMode::Plain);
    }

    #[test]
    fn hosted_always_off() {
        let cfg = ConsoleConfig {
            mode: ConsoleMode::Gui,
            ..Default::default()
        };
        assert_eq!(cfg.resolve(true, true), ResolvedMode::Off);
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(ConsoleMode::parse("GUI"), Some(ConsoleMode::Gui));
        assert_eq!(ConsoleMode::parse("off"), Some(ConsoleMode::Off));
        assert_eq!(ConsoleMode::parse("bogus"), None);
    }

    #[test]
    fn lang_normalizes_codes_and_shorts() {
        for s in ["en-US", "en_us", "EN", " en ", "english"] {
            assert_eq!(normalize_lang(s).as_deref(), Some(LANG_EN_US), "{s:?}");
        }
        for s in ["zh-CN", "ZH_cn", "zh", "cn", "CHINESE", "中文", "简体"] {
            assert_eq!(normalize_lang(s).as_deref(), Some(LANG_ZH_CN), "{s:?}");
        }
        for s in ["", "fr", "jp", "zh-TW", "en-UK"] {
            assert_eq!(normalize_lang(s), None, "{s:?}");
        }
        assert_eq!(DEFAULT_LANG, LANG_EN_US);
    }

    #[test]
    fn lang_marker_roundtrip() {
        // Unique temp paths (no `set_current_dir`, parallel-safe).
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir()
            .join(format!("sculk-marker-test-{}-{n}", std::process::id()));
        assert!(!marker_exists_at(&p), "absent marker reads false");
        create_marker_at(&p).unwrap();
        assert!(marker_exists_at(&p), "created marker reads true");
        // Marker carries no language id (empty file by construction).
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn lang_resolve_precedence() {
        // forced > initial > default; invalid entries fall through, never empty.
        assert_eq!(
            resolve_lang(Some(LANG_ZH_CN.to_string()), Some(LANG_EN_US.to_string())).as_str(),
            LANG_ZH_CN
        );
        assert_eq!(
            resolve_lang(None, Some(LANG_ZH_CN.to_string())).as_str(),
            LANG_ZH_CN
        );
        assert_eq!(
            resolve_lang(Some("fr-FR".to_string()), Some(LANG_ZH_CN.to_string())).as_str(),
            LANG_ZH_CN
        );
        assert_eq!(
            resolve_lang(None, None).as_str(),
            DEFAULT_LANG
        );
        assert!(!resolve_lang(None, None).is_empty());
    }

    #[test]
    fn selector_lists_loaded_locales_mains_first() {
        let order = selector_order();
        assert!(order.contains(&LANG_EN_US.to_string()));
        assert!(order.contains(&LANG_ZH_CN.to_string()));
        let en = order.iter().position(|l| l == LANG_EN_US).unwrap();
        let zh = order.iter().position(|l| l == LANG_ZH_CN).unwrap();
        assert!(en < zh, "English first, then Chinese, then future languages");
    }

    /// All `tui.*` keys (must mirror `locales/*.yml`).
    const TUI_KEYS: &[&str] = &[
        "tui.language_name",
        "tui.hint_back_to_bottom",
        "tui.exit_confirm",
        "tui.hint_keys",
        "tui.search_prefix",
        "tui.search_placeholder",
        "tui.search_no_match",
        "tui.players_title",
        "tui.players_empty",
        "tui.term_too_small",
        "tui.new_logs",
        "tui.alias_tag",
        "tui.no_packs",
        "tui.status_complete",
        "tui.status_reviewing",
        "tui.status_dropped",
        "tui.status_locked",
        "tui.server_starting",
        "tui.input_placeholder",
        "tui.console_ready",
        "tui.copy_ok",
        "tui.copy_fail_clipboard",
        "tui.copy_fail_write",
        "tui.copy_fail_spawn",
        "tui.input_truncated",
        "tui.queue_full",
        "tui.plain_ready",
        "tui.not_tty",
        "tui.take_over_fail",
        "tui.unknown_console",
        "tui.unknown_boot",
        "tui.unknown_welcome",
        "tui.unknown_lang",
    ];

    #[test]
    fn every_key_exists_in_every_locale() {
        // No raw-key leaks: a missing key would render as `tui.…` itself.
        // (Fallback covers display, but this test forces translation completeness.)
        for loc in rust_i18n::available_locales!().into_iter() {
            for key in TUI_KEYS.iter().copied() {
                let text = rust_i18n::t!(key, locale = loc).to_string();
                assert_ne!(text, key, "{key} missing in {loc}");
            }
        }
    }

    #[test]
    fn zh_translations_actually_differ_from_en() {
        // Paired with the test above: a zh key falling back to en would pass
        // "exists" but fail here (all 33 keys genuinely differ today).
        for key in TUI_KEYS.iter().copied() {
            let en = rust_i18n::t!(key, locale = LANG_EN_US).to_string();
            let zh = rust_i18n::t!(key, locale = LANG_ZH_CN).to_string();
            assert_ne!(en, zh, "{key} untranslated in zh-CN");
        }
    }

    #[test]
    fn interpolation_markers_are_well_formed() {
        // Templates with an argument carry exactly one `%{n}` / `%{v}`;
        // plain strings carry no braces at all (native `t!` vars, no panics possible).
        let with_arg = [
            ("tui.players_title", "%{n}"),
            ("tui.new_logs", "%{n}"),
            ("tui.status_complete", "%{n}"),
            ("tui.status_reviewing", "%{n}"),
            ("tui.status_dropped", "%{n}"),
            ("tui.copy_ok", "%{n}"),
            ("tui.input_truncated", "%{n}"),
            ("tui.unknown_console", "%{v}"),
            ("tui.unknown_boot", "%{v}"),
            ("tui.unknown_welcome", "%{v}"),
            ("tui.unknown_lang", "%{v}"),
        ];
        for loc in rust_i18n::available_locales!().into_iter() {
            for (key, marker) in with_arg {
                let raw = rust_i18n::t!(key, locale = loc).to_string();
                assert_eq!(raw.matches(marker).count(), 1, "{key} in {loc}");
            }
            for key in TUI_KEYS.iter().copied() {
                if with_arg.iter().any(|(k, _)| *k == key) {
                    continue;
                }
                let raw = rust_i18n::t!(key, locale = loc).to_string();
                assert!(
                    !raw.contains('{') && !raw.contains('}'),
                    "{key} in {loc}"
                );
            }
            // Native interpolation works end to end.
            let title = rust_i18n::t!("tui.players_title", locale = loc, n = 128).to_string();
            assert!(title.contains("128"), "{loc}");
            assert!(!title.contains("%{"), "{loc}");
        }
    }

    #[test]
    fn boot_anim_defaults_on_and_switch_parses() {        assert!(ConsoleConfig::default().boot_anim, "开场动画默认开启");
        assert!(ConsoleConfig::default().welcome, "欢迎屏默认开启");
        for s in ["1", "true", "TRUE", " yes ", "on", "ON"] {
            assert_eq!(parse_bool_switch(s), Some(true), "{s:?}");
        }
        for s in ["0", "false", "no", "off", "none", "disabled", "OFF"] {
            assert_eq!(parse_bool_switch(s), Some(false), "{s:?}");
        }
        assert_eq!(parse_bool_switch("bogus"), None);
        assert_eq!(parse_bool_switch(""), None);
    }
}
