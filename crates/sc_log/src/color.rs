//! 段落码（section code）到 ANSI 转义的转换，以及构建期生成的 i18n 查表。
//!
//! - `colorize_literal!`（见 `sc_log_macros`）在编译期完成转换；
//! - `contains_minecraft_code` 判断字符串是否含段落码；
//! - `minecraft_to_ansi` / `strip_minecraft_codes` 单趟逐字节转换；
//! - `ColorizingWriter` 包装 `io::Write`，逐块扫描：无码块原样写出，有码块转换；
//! - `ansi_enabled` 经 `OnceLock` 探测一次环境，之后不再读环境变量。
//!
//! Section-code to ANSI conversion plus the build-time generated i18n lookup tables.
//!
//! - `colorize_literal!` (see `sc_log_macros`) converts at compile time;
//! - `contains_minecraft_code` reports whether a string carries section codes;
//! - `minecraft_to_ansi` / `strip_minecraft_codes` convert in one byte-level pass;
//! - `ColorizingWriter` wraps `io::Write` and scans chunk by chunk: codeless chunks are
//!   written as-is, chunks with codes are converted;
//! - `ansi_enabled` probes the environment once via `OnceLock` and reads no env vars after that.

use std::borrow::Cow;
use std::io;
use std::sync::OnceLock;

// Build-time i18n tables: COLORED_LOCALES (codes to ANSI) / PLAIN_LOCALES (codes stripped).
// Generated at compile time by sc_log/build.rs from locales/*.yml.
include!(concat!(env!("OUT_DIR"), "/colored_locales.rs"));

/// i18n 查询：优先命中构建期预着色/去码表；未知键回退原始 rust_i18n
/// （原样保留段落码，由 ColorizingWriter 在下游转换）。
///
/// 返回 `Cow<'static, str>`：命中表时为 `Borrowed`，回退分支为 `Owned`，
/// 因此键无需 `'static` 生命周期。
///
/// i18n lookup: prefers the build-time pre-colored/stripped tables;
/// unknown keys fall back to raw rust_i18n (verbatim codes, converted downstream by ColorizingWriter).
///
/// Keys need no 'static lifetime: the fallback branch returns `Cow::Owned` while table hits return `Cow::Borrowed`.
pub fn translate(locale: &str, key: &str) -> Cow<'static, str> {
    let table = if ansi_enabled() {
        COLORED_LOCALES
    } else {
        PLAIN_LOCALES
    };
    if let Some(value) = lookup(table, locale, key) {
        return Cow::Borrowed(value);
    }
    Cow::Owned(crate::_rust_i18n_translate(locale, key).into_owned())
}

/// 线性查表（构建期生成的表很小：数十个键乘少数语言）。
///
/// Linear lookup over the generated tables (tens of keys times a few locales).
#[inline]
fn lookup<'a>(table: &'a [(&str, &[(&str, &str)])], locale: &str, key: &str) -> Option<&'a str> {
    for (table_locale, entries) in table {
        if *table_locale == locale {
            for (table_key, value) in *entries {
                if *table_key == key {
                    return Some(value);
                }
            }
            return None;
        }
    }
    None
}

/// UTF-8 encoding of the section mark (U+00A7).
const SECTION: [u8; 2] = [0xC2, 0xA7];

/// 从 `start` 起扫描首个段落标记（U+00A7）的字节下标，不存在时返回 None。
/// 先匹配 UTF-8 前导字节 0xC2，再校验 0xA7。
///
/// Byte index of the first section mark from `start`; None when absent.
/// Matches the UTF-8 lead byte 0xC2 first, then validates 0xA7.
#[inline]
fn find_section(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == SECTION[0] {
            if bytes[i + 1] == SECTION[1] {
                return Some(i);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

/// 字符串是否包含段落码。
///
/// Whether a string contains section codes.
#[inline]
pub fn contains_minecraft_code(input: &str) -> bool {
    find_section(input.as_bytes(), 0).is_some()
}

/// 段落码转 ANSI 转义（查找前把码字符转小写；与 sc_log_macros 编译期表一致）。
///
/// Section code to ANSI escape (code char lowercased; matches the sc_log_macros compile-time table).
#[inline]
fn ansi_for_code(code: u8) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase() {
        b'0' => "\x1b[30m", // black
        b'1' => "\x1b[34m", // dark blue
        b'2' => "\x1b[32m", // dark green
        b'3' => "\x1b[36m", // dark cyan
        b'4' => "\x1b[31m", // dark red
        b'5' => "\x1b[35m", // dark purple
        b'6' => "\x1b[33m", // gold
        b'7' => "\x1b[37m", // gray
        b'8' => "\x1b[90m", // dark gray
        b'9' => "\x1b[94m", // blue
        b'a' => "\x1b[92m", // green
        b'b' => "\x1b[96m", // cyan
        b'c' => "\x1b[91m", // red
        b'd' => "\x1b[95m", // magenta
        b'e' => "\x1b[93m", // yellow
        b'f' => "\x1b[97m", // white
        b'l' => "\x1b[1m",  // bold
        b'o' => "\x1b[3m",  // italic
        b'n' => "\x1b[4m",  // underline
        b'm' => "\x1b[9m",  // strikethrough
        b'k' => "\x1b[5m",  // blink
        b'r' => "\x1b[0m",  // reset
        _ => return None,
    })
}

/// 把段落码替换为 ANSI 转义。
///
/// Replaces section codes with ANSI escapes.
///
/// Returns `Cow::Borrowed` when the input has no codes (no allocation); with codes it
/// precomputes capacity and allocates once. Lone or unknown codes stay verbatim and
/// either case is accepted.
pub fn minecraft_to_ansi(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    if find_section(bytes, 0).is_none() {
        return Cow::Borrowed(input);
    }
    // Precompute capacity: count code sequences (each replacement is at most ~6 ANSI bytes, with margin).
    let mut count = 0usize;
    let mut i = 0;
    while let Some(pos) = find_section(bytes, i) {
        count += 1;
        i = pos + 2;
    }
    let mut out = String::with_capacity(bytes.len() + count * 8);
    let mut cursor = 0usize;
    while let Some(pos) = find_section(bytes, cursor) {
        out.push_str(&input[cursor..pos]);
        let code_pos = pos + 2;
        if code_pos < bytes.len() {
            let code = bytes[code_pos];
            if code.is_ascii() {
                match ansi_for_code(code) {
                    Some(ansi) => {
                        out.push_str(ansi);
                        cursor = code_pos + 1;
                        continue;
                    }
                    None => {
                        // Unknown code: keeps the code pair verbatim.
                        out.push('§');
                        out.push(code as char);
                        cursor = code_pos + 1;
                        continue;
                    }
                }
            }
            // Non-ASCII code byte: keep the mark verbatim, resume scanning from the code byte.
            out.push_str("§");
            cursor = code_pos;
            continue;
        }
        // Trailing lone mark ([C2 A7] with no code byte).
        out.push('§');
        cursor = code_pos;
    }
    out.push_str(&input[cursor..]);
    Cow::Owned(out)
}

/// 删除全部段落码；输入不含段落码时直接返回借用的输入。
///
/// Strips all section codes; returns the input borrowed when none are present.
pub fn strip_minecraft_codes(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    if find_section(bytes, 0).is_none() {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(bytes.len());
    let mut cursor = 0usize;
    while let Some(pos) = find_section(bytes, cursor) {
        out.push_str(&input[cursor..pos]);
        let code_pos = pos + 2;
        cursor = if code_pos < bytes.len() && bytes[code_pos].is_ascii() {
            // Drops the mark plus ASCII format code together.
            code_pos + 1
        } else {
            // Non-ASCII code byte (multibyte lead byte): drops only the mark, keeps the char.
            code_pos
        };
    }
    out.push_str(&input[cursor..]);
    Cow::Owned(out)
}

/// 环境是否适合 ANSI 彩色输出（经 `OnceLock` 缓存一次）。
///
/// Whether the environment suits ANSI color output (cached once via `OnceLock`).
///
/// Follows the `NO_COLOR` spec; `TERM=dumb` counts as a colorless terminal.
pub fn ansi_enabled() -> bool {
    *ANSI_SUPPORTED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        match std::env::var("TERM") {
            Ok(term) => !term.eq_ignore_ascii_case("dumb"),
            Err(_) => true,
        }
    })
}

static ANSI_SUPPORTED: OnceLock<bool> = OnceLock::new();

/// 便捷入口：按终端能力把码转 ANSI，不支持时剥离。
///
/// Convenience entry: converts codes to ANSI per terminal capability, strips when unsupported.
pub fn colorize(input: &str) -> String {
    colorize_cow(input).into_owned()
}

/// 剥离 ANSI 转义序列（`\x1b[...m` CSI 及同类；UTF-8 安全）。
///
/// Strips ANSI escape sequences (`\x1b[...m` CSI and friends; UTF-8 safe).
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // CSI: skips param/intermediate bytes until the final byte (0x40..=0x7E).
                for c in chars.by_ref() {
                    let b = c as u32;
                    if (0x40..=0x7E).contains(&b) {
                        break;
                    }
                }
            }
            // Other ESC sequences: drops the byte.
        } else {
            out.push(c);
        }
    }
    out
}

/// 日志文件的纯文本：先剥离 ANSI，再剥离段落码。
///
/// Plain text for log files: strips ANSI first, then section codes.
pub fn plain_text(input: &str) -> String {
    strip_minecraft_codes(&strip_ansi(input)).into_owned()
}

/// 便捷入口：按环境返回着色或去码结果；无段落码时返回借用的输入。
///
/// Convenience entry: colors or strips according to the environment, returning the input
/// borrowed when it carries no codes.
pub fn colorize_cow(input: &str) -> Cow<'_, str> {
    if ansi_enabled() {
        minecraft_to_ansi(input)
    } else {
        strip_minecraft_codes(input)
    }
}

/// 输出模式。
///
/// Output mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    /// Codes to ANSI escapes.
    Ansi,
    /// Strips codes.
    Strip,
}

/// 流式着色写入器：包装任意 `io::Write`，逐块扫描段落码。
///
/// 无码块直接原样写出；有码块按 `mode` 转换后再写。
///
/// Streaming colorizing writer: wraps any `io::Write`, scanning chunk by chunk for codes.
///
/// Codeless chunks pass through untouched; chunks with codes are converted according to
/// `mode` before writing.
pub struct ColorizingWriter<'a, W: io::Write> {
    inner: &'a mut W,
    mode: ColorMode,
    /// Caches a dangling 0xC2 across chunks (rare case where the mark lead byte straddles chunks).
    pending_c2: bool,
}

impl<'a, W: io::Write> ColorizingWriter<'a, W> {
    pub fn new(inner: &'a mut W, mode: ColorMode) -> Self {
        Self {
            inner,
            mode,
            pending_c2: false,
        }
    }

    pub fn with_auto(inner: &'a mut W) -> Self {
        Self::new(
            inner,
            if ansi_enabled() {
                ColorMode::Ansi
            } else {
                ColorMode::Strip
            },
        )
    }
}

impl<W: io::Write> io::Write for ColorizingWriter<'_, W> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut data = buf;
        if self.pending_c2 {
            self.pending_c2 = false;
            if !data.is_empty() && data[0] == SECTION[1] {
                // Full mark across chunks: needs the trailing code byte.
                if data.len() >= 2 && data[1].is_ascii() {
                    match ansi_for_code(data[1]) {
                        Some(ansi) => {
                            self.inner.write_all(ansi.as_bytes())?;
                            data = &data[2..];
                        }
                        None => {
                            // Unknown code: writes the mark pair verbatim.
                            self.inner.write_all(&SECTION)?;
                            self.inner.write_all(&data[1..2])?;
                            data = &data[2..];
                        }
                    }
                } else {
                    // Non-ASCII code byte: emits the mark alone, routes the code byte to the normal path.
                    self.inner.write_all(&SECTION)?;
                    data = &data[1..];
                }
            } else {
                self.inner.write_all(&[SECTION[0]])?;
            }
        }
        if data.is_empty() {
            return Ok(buf.len());
        }
        // Handles a trailing dangling 0xC2 (mark may straddle chunks).
        if data[data.len() - 1] == SECTION[0] {
            self.pending_c2 = true;
            data = &data[..data.len() - 1];
        }
        if data.is_empty() {
            return Ok(buf.len());
        }
        // Codeless chunks pass through directly (zero-alloc fast path).
        if find_section(data, 0).is_none() {
            self.inner.write_all(data)?;
            return Ok(buf.len());
        }
        // With codes: handles as a string (chunks from write_str should be valid UTF-8).
        let s = std::str::from_utf8(data).unwrap_or("");
        match self.mode {
            ColorMode::Ansi => match minecraft_to_ansi(s) {
                Cow::Borrowed(plain) => self.inner.write_all(plain.as_bytes())?,
                Cow::Owned(colored) => self.inner.write_all(colored.as_bytes())?,
            },
            ColorMode::Strip => match strip_minecraft_codes(s) {
                Cow::Borrowed(plain) => self.inner.write_all(plain.as_bytes())?,
                Cow::Owned(stripped) => self.inner.write_all(stripped.as_bytes())?,
            },
        }
        Ok(buf.len())
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        if self.pending_c2 {
            self.inner.write_all(&[SECTION[0]])?;
            self.pending_c2 = false;
        }
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// 每个语言文件必须携带完全相同的键集（无缺失或
    ///
    /// Every locale file must carry exactly the same key set (no missing or
    /// extra keys per language). Parses the flat `key: "value"` format.
    #[test]
    fn locale_files_share_identical_key_sets() {
        fn keys_of(path: &std::path::Path) -> std::collections::BTreeSet<String> {
            let content = std::fs::read_to_string(path).expect("locale file must exist");
            let mut keys = std::collections::BTreeSet::new();
            for raw_line in content.lines() {
                let line = raw_line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some(idx) = line.find(':') else { continue };
                let key = line[..idx].trim();
                if key.is_empty() || key == "_version" {
                    continue;
                }
                keys.insert(key.to_string());
            }
            keys
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
        let mut entries = std::fs::read_dir(&dir)
            .expect("locales dir must exist")
            .map(|e| e.expect("locale entry").path())
            .filter(|p| p.extension().is_some_and(|e| e == "yml"))
            .collect::<Vec<_>>();
        entries.sort();
        assert!(
            entries.len() >= 2,
            "expected at least zh-CN and en-US, got {entries:?}"
        );
        let baseline = keys_of(&entries[0]);
        assert!(!baseline.is_empty(), "baseline locale has no keys");
        for path in &entries[1..] {
            let keys = keys_of(path);
            let missing: Vec<_> = baseline.difference(&keys).collect();
            let extra: Vec<_> = keys.difference(&baseline).collect();
            assert!(missing.is_empty(), "{path:?} misses keys: {missing:?}");
            assert!(extra.is_empty(), "{path:?} has extra keys: {extra:?}");
        }
    }

    #[test]
    fn fast_path_zero_alloc() {
        let plain = "no codes here";
        // Borrowed means zero allocation.
        assert!(matches!(minecraft_to_ansi(plain), Cow::Borrowed(_)));
        assert!(matches!(strip_minecraft_codes(plain), Cow::Borrowed(_)));
    }

    #[test]
    fn converts_basic_color_codes() {
        assert_eq!(minecraft_to_ansi("§aHello"), "\x1b[92mHello");
        assert_eq!(minecraft_to_ansi("§cRed"), "\x1b[91mRed");
        assert_eq!(minecraft_to_ansi("§A"), minecraft_to_ansi("§a"));
    }

    #[test]
    fn handles_mixed_and_reset() {
        assert_eq!(minecraft_to_ansi("§a绿§r白"), "\x1b[92m绿\x1b[0m白");
        assert_eq!(minecraft_to_ansi("§l粗"), "\x1b[1m粗");
    }

    #[test]
    fn preserves_lone_or_unknown_codes() {
        assert_eq!(minecraft_to_ansi("100§"), "100§");
        assert_eq!(minecraft_to_ansi("§x"), "§x");
        // Non-ASCII code bytes stay verbatim.
        assert_eq!(minecraft_to_ansi("§你"), "§你");
    }

    #[test]
    fn strips_all_codes() {
        assert_eq!(strip_minecraft_codes("§aHello §lWorld§r!"), "Hello World!");
        assert_eq!(strip_minecraft_codes("no codes"), "no codes");
        assert_eq!(strip_minecraft_codes("§你"), "你");
    }

    #[test]
    fn macro_level_colorize() {
        // Compile-time macro matches the runtime result (sc_log_macros).
        assert_eq!(
            crate::colorize_literal!("§aHello §cWorld!"),
            "\x1b[92mHello \x1b[91mWorld!"
        );
        assert_eq!(crate::colorize_literal!("plain"), "plain");
    }

    #[test]
    fn precolored_locale_table_built_at_compile_time() {
        // Build-time table: console.startup in COLORED_LOCALES already contains ANSI.
        assert!(COLORED_LOCALES.iter().any(|(_, entries)| {
            entries
                .iter()
                .any(|(k, v)| *k == "console.startup" && v.contains("\u{1b}["))
        }));
        // PLAIN_LOCALES already strips codes.
        assert!(PLAIN_LOCALES.iter().any(|(_, entries)| {
            entries
                .iter()
                .any(|(k, v)| *k == "console.startup" && !v.contains('§'))
        }));
        // translate hits; no codes remain whether ANSI is on or off (converted at build time).
        let s = translate("zh-CN", "console.startup");
        assert!(s.contains("欢迎使用"), "key must hit: {s:?}");
        assert!(
            !s.contains('§'),
            "no section marks may remain after pre-render: {s:?}"
        );
        // Unknown keys fall back to raw i18n.
        let miss = translate("zh-CN", "no.such.key");
        assert_eq!(
            miss.as_ref(),
            crate::_rust_i18n_translate("zh-CN", "no.such.key").as_ref()
        );
    }

    #[test]
    fn t_keeps_plain_while_t_log_colors() {
        crate::set_locale("zh-CN");
        // t! keeps verbatim codes (for player-visible text).
        let plain = crate::t!("console.startup");
        assert!(plain.contains('§'), "t! must keep section codes: {plain:?}");
        // t_log! is pre-colored at build time (ANSI or stripped), leaving no codes.
        let log_msg = crate::t_log!("console.startup");
        assert!(
            !log_msg.contains('§'),
            "t_log! must not retain section codes: {log_msg:?}"
        );
        // The variable version behaves the same.
        let plain_var = crate::t!("console.startup", version = "1.2.3");
        let log_var = crate::t_log!("console.startup", version = "1.2.3");
        assert!(plain_var.contains('§') && plain_var.contains("1.2.3"));
        assert!(!log_var.contains('§') && log_var.contains("1.2.3"));
    }

    #[test]
    fn streaming_writer_converts_and_passes_through() {
        let mut out = Vec::new();
        {
            let mut w = ColorizingWriter::new(&mut out, ColorMode::Ansi);
            w.write_all(b"plain chunk").unwrap();
            w.write_all("§aHello".as_bytes()).unwrap();
            w.flush().unwrap();
        }
        assert_eq!(String::from_utf8(out).unwrap(), "plain chunk\x1b[92mHello");
    }

    #[test]
    fn streaming_writer_strips_in_strip_mode() {
        let mut out = Vec::new();
        {
            let mut w = ColorizingWriter::new(&mut out, ColorMode::Strip);
            w.write_all("§aHello §r".as_bytes()).unwrap();
        }
        assert_eq!(String::from_utf8(out).unwrap(), "Hello ");
    }

    #[test]
    fn streaming_writer_handles_pending_section_byte() {
        // Mark bytes split across chunks: 0xC2 ends the previous chunk, 0xA7 starts the next.
        let mut out = Vec::new();
        {
            let mut w = ColorizingWriter::new(&mut out, ColorMode::Ansi);
            w.write_all(b"a\xc2").unwrap();
            w.write_all(b"\xa7bHi").unwrap();
            w.flush().unwrap();
        }
        // Code b is cyan (ANSI 96); the code char is consumed, leaving "Hi".
        assert_eq!(String::from_utf8(out).unwrap(), "a\x1b[96mHi");
    }

    /// 两份内置语言在运行时都能解析（显式语言，无全局
    ///
    /// Both shipped locales resolve at runtime (explicit locale, no global
    /// mutation): zh-CN keeps the Chinese text, en-US the English text.
    #[test]
    fn both_locales_resolve() {
        let zh = crate::color::translate("zh-CN", "console.warning");
        let en = crate::color::translate("en-US", "console.warning");
        assert!(zh.contains("Alpha"), "zh-CN text missing: {zh:?}");
        assert!(en.contains("Alpha"), "en-US text missing: {en:?}");
        assert!(zh.contains("生产环境"), "zh-CN not Chinese: {zh:?}");
        assert!(!en.contains("生产环境"), "en-US leaked Chinese: {en:?}");
    }
}
