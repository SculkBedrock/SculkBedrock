//! 命令补全接口。
//!
//! 宿主（服务端）实现 [`CompletionProvider`]，并通过
//! [`ConsoleHandle::set_provider`][crate::driver::ConsoleHandle::set_provider]
//! 热替换；控制台只调用并展示结果，绝不缓存或解释命令语义。
//!
//! 内置实现：
//!
//! - [`NoopCompletionProvider`]：默认占位，无补全（零开销）。
//! - [`StaticCompletionProvider`]：基于命令快照（名称/别名/描述）的前缀补全；
//!   宿主按注册表快照重建，插件命令因此自动出现。
//!
//! 触发规则（Minecraft 风格，见 [`trigger_at`]）：仅在 `/`（命令位）或
//! `:`（命名空间位）之后出现补全；应用补全只替换触发词之后的文本，
//! `/` 与 `:` 前缀原样保留（如 `/st` 变为 `/stop`）。
//!
//! Command completion interface.
//!
//! The host (server) implements [`CompletionProvider`] and hot-swaps it via
//! [`ConsoleHandle::set_provider`][crate::driver::ConsoleHandle::set_provider]
//! `set_provider`; the console only invokes and displays it, never caching or interpreting command semantics.
//!
//! Built-in implementations:
//!
//! - [`NoopCompletionProvider`]: default placeholder, no completions (zero cost).
//! - [`StaticCompletionProvider`]: prefix completion over a command snapshot (names/aliases/descriptions);
//!   the host rebuilds it from registry snapshots so plugin commands appear automatically.
//!
//! Trigger rule (Minecraft-style, see [`trigger_at`]): completions only appear after `/` (command slot) or
//! `:` (namespace slot); applying a completion only replaces the text after the trigger,
//! keeping the `/` and `:` prefixes verbatim (e.g. `/st` becomes `/stop`).

use std::sync::Arc;

/// 一个补全候选项。
///
/// One completion candidate.
#[derive(Clone, Debug)]
pub struct CompletionItem {
    /// Display name (usually the full command name, e.g. `teleport` or `minecraft:stop`).
    pub display: String,
    /// Applying the completion replaces text in `[anchor, cursor)` (`anchor` comes from [`trigger_at`]
    /// and sits after the trigger, so the `/` and `:` prefixes survive).
    pub replace: String,
    /// Trailing hint text (usually the command description; may be empty).
    pub description: String,
    /// Whether this came from an alias match (may be annotated in display).
    pub from_alias: bool,
}

impl CompletionItem {
    pub fn new(display: &str, replace: &str, description: &str, from_alias: bool) -> Self {
        Self {
            display: display.to_string(),
            replace: replace.to_string(),
            description: description.to_string(),
            from_alias,
        }
    }
}

/// 命令补全接口：给定整行与光标（字符索引），返回候选项。
///
/// 调用方与实现方共同遵守的约定：
///
/// - `cursor` 是 `char` 索引而非字节索引，调用方保证不切断字符边界；
/// - 返回的候选项已按展示优先级排好序，控制台只按配置上限截断；
/// - 该方法在控制台 UI 线程同步调用：实现必须快速返回
///   （只读快照/静态表，不做 IO、不取游戏锁，游戏状态需提前快照）。
///
/// Command completion interface: given a full line plus cursor (char index), return candidates.
///
/// Contract shared by callers and implementations:
///
/// - `cursor` is a `char` index, not a byte index; the caller never splits a char boundary;
/// - Returned candidates are pre-sorted by display priority; the console only truncates to the configured limit;
/// - This method is called synchronously on the console UI thread: implementations must return fast
///   (read snapshots/static tables only; no IO and no game locks; snapshot any game state up front).
pub trait CompletionProvider: Send + Sync + 'static {
    /// Return candidates at the current cursor (already priority-sorted).
    fn complete(&self, line: &str, cursor: usize) -> Vec<CompletionItem>;

    /// Implementation name (for debugging/status line).
    fn name(&self) -> &'static str {
        "unknown"
    }
}

/// 默认占位：始终返回空。
///
/// Default placeholder: always returns empty.
pub struct NoopCompletionProvider;

impl CompletionProvider for NoopCompletionProvider {
    fn complete(&self, _line: &str, _cursor: usize) -> Vec<CompletionItem> {
        Vec::new()
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}

/// 快照中的一条命令（含别名与描述）。
///
/// One command in the snapshot (with aliases and description).
#[derive(Clone, Debug)]
pub struct CommandEntry {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
}

impl CommandEntry {
    pub fn new(name: &str, aliases: &[&str], description: &str) -> Self {
        Self {
            name: name.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            description: description.to_string(),
        }
    }
}

/// 基于静态快照的前缀补全（宿主按注册表内容周期性重建）。
///
/// 匹配规则：
///
/// - 只补全第一个 token（命令名位置）；光标在别处时返回空；
/// - 前缀匹配不区分大小写，命令名排在别名之前；
/// - token 为空（全新控制台 / 单独的 `/`）时返回全部命令（由调用方截断）。
///
/// Prefix completion over a static snapshot (rebuilt periodically by the host from registry contents).
///
/// Match rules:
///
/// - Only the first token (command-name position) is completed; returns empty when the cursor is elsewhere;
/// - Case-insensitive prefix match, command names rank above aliases;
/// - An empty token (fresh console / bare `/`) returns all commands (caller truncates).
#[derive(Clone, Debug, Default)]
pub struct StaticCompletionProvider {
    entries: Vec<CommandEntry>,
}

impl StaticCompletionProvider {
    pub fn new(entries: Vec<CommandEntry>) -> Self {
        let mut sorted = entries;
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        Self { entries: sorted }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl CompletionProvider for StaticCompletionProvider {
    fn complete(&self, line: &str, cursor: usize) -> Vec<CompletionItem> {
        // Minecraft-style trigger: complete only after `/` (command slot) or `:` (namespace slot).
        let Some((_, query)) = trigger_at(line, cursor) else {
            return Vec::new();
        };
        let query = query.to_ascii_lowercase();

        // Three priority tiers: command name, then alias, then namespace suffix (e.g. `stop` in `minecraft:stop`).
        let mut names = Vec::new();
        let mut aliases = Vec::new();
        let mut namespaced = Vec::new();
        for entry in &self.entries {
            let lower = entry.name.to_ascii_lowercase();
            if lower.starts_with(&query) {
                names.push(CompletionItem::new(
                    &entry.name,
                    &entry.name,
                    &entry.description,
                    false,
                ));
                continue;
            }
            let mut hit = false;
            for alias in &entry.aliases {
                if alias.to_ascii_lowercase().starts_with(&query) {
                    aliases.push(CompletionItem::new(
                        alias,
                        &entry.name,
                        &entry.description,
                        true,
                    ));
                    hit = true;
                    break;
                }
            }
            if hit {
                continue;
            }
            if let Some(suffix) = lower.split(':').next_back() {
                if suffix != lower && suffix.starts_with(&query) {
                    // `replace` only substitutes the text after the trigger; prefixes (e.g. namespaces) are kept.
                    namespaced.push(CompletionItem::new(
                        &entry.name,
                        suffix,
                        &entry.description,
                        false,
                    ));
                }
            }
        }
        names.extend(aliases);
        names.extend(namespaced);
        names
    }

    fn name(&self) -> &'static str {
        "static"
    }
}

/// Take the token under the cursor (whitespace-delimited); returns `(token start char index, token text)`.
///
/// `cursor` is a char index; the returned text spans token start to cursor.
pub fn current_token(line: &str, cursor: usize) -> (usize, String) {
    let chars: Vec<char> = line.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = cursor;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    (start, chars[start..cursor].iter().collect())
}

/// Completion trigger: returns `(replacement start char index, query)`; `None` when not triggered.
///
/// Minecraft-style rules (shared by callers and implementations):
///
/// - First token starts with `/`: command slot, query is the text after `/`;
///   applying the completion replaces only that part, keeping `/` (e.g. `/st` becomes `/stop`);
/// - Token contains `:`: namespace slot, query is the text after the last `:`;
///   applying replaces only that part, keeping the prefix (e.g. `minecraft:st` becomes `minecraft:stop`);
/// - Anything else (plain text, argument slot, inside quotes) yields `None` with no popup;
/// - An empty query only triggers on `/` (showing everything); a lone `:` never triggers.
///
/// `cursor` is a char index; all indices are char indices (CJK safe).
pub fn trigger_at(line: &str, cursor: usize) -> Option<(usize, String)> {
    let (start, token) = current_token(line, cursor);
    if token.is_empty() {
        return None;
    }
    // No trigger inside quotes (avoids popups for argument text like `"a:b`).
    if token.starts_with('"') {
        return None;
    }
    // Namespace slot takes precedence: after the last ':'.
    if let Some(rel) = token
        .char_indices()
        .filter(|(_, c)| *c == ':')
        .last()
        .map(|(byte_idx, _)| token[..byte_idx].chars().count())
    {
        let query: String = token.chars().skip(rel + 1).collect();
        if query.is_empty() {
            return None;
        }
        return Some((start + rel + 1, query));
    }
    // Command slot: first token starting with '/'.
    let chars: Vec<char> = line.chars().collect();
    let is_first = chars[..start].iter().all(|c| c.is_whitespace());
    if is_first && token.starts_with('/') {
        let query: String = token.chars().skip(1).collect();
        return Some((start + 1, query));
    }
    None
}

/// Common prefix of several candidates (char-boundary safe; for progressive Tab completion).
pub fn common_prefix(a: &str, b: &str) -> String {
    a.chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x)
        .collect()
}

/// Box a provider (for the host `set_provider` call).
pub fn shared_provider<P: CompletionProvider>(p: P) -> Arc<dyn CompletionProvider> {
    Arc::new(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StaticCompletionProvider {
        StaticCompletionProvider::new(vec![
            CommandEntry::new("teleport", &["tp"], "Teleport somewhere"),
            CommandEntry::new("stop", &[], "Stops the server"),
            CommandEntry::new("say", &[], "Broadcast a message"),
            CommandEntry::new("help", &["?"], "Shows help"),
            CommandEntry::new("minecraft:stop", &[], "Namespaced stop"),
        ])
    }

    #[test]
    fn no_trigger_without_slash_or_colon() {
        // Plain text shows no popup (requires `/` or `:` first).
        assert!(sample().complete("st", 2).is_empty());
        assert!(sample().complete("", 0).is_empty());
        assert!(sample().complete("say hello", 9).is_empty());
    }

    #[test]
    fn slash_triggers_command_completion() {
        let items = sample().complete("/st", 3);
        // Command names rank first, aliases/namespace suffixes follow.
        assert_eq!(items[0].replace, "stop");
        assert!(!items[0].from_alias);
        assert!(items.iter().any(|i| i.display == "minecraft:stop"));
    }

    #[test]
    fn slash_alone_shows_all() {
        let items = sample().complete("/", 1);
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].replace, "help");
    }

    #[test]
    fn alias_maps_to_canonical() {
        let items = sample().complete("/t", 2);
        assert!(items.iter().any(|i| i.replace == "teleport"));
        let via_alias = sample().complete("/tp", 3);
        assert!(via_alias
            .iter()
            .any(|i| i.from_alias && i.replace == "teleport"));
    }

    #[test]
    fn colon_triggers_namespace_suffix_completion() {
        // `minecraft:st` replaces only the part after the colon, keeping the prefix.
        let items = sample().complete("/minecraft:st", 13);
        assert!(items
            .iter()
            .any(|i| i.replace == "stop" && i.display == "minecraft:stop"));
        // No completion when the token after the cursor has no trigger.
        assert!(sample().complete("/stop x", 7).is_empty());
    }

    #[test]
    fn trigger_positions() {
        assert_eq!(trigger_at("/st", 3), Some((1, "st".to_string())));
        assert_eq!(trigger_at("/", 1), Some((1, "".to_string())));
        assert_eq!(trigger_at("st", 2), None);
        assert_eq!(trigger_at("", 0), None);
        // Namespace slot: anchor sits after the last ':'.
        assert_eq!(trigger_at("minecraft:st", 12), Some((10, "st".to_string())));
        assert_eq!(trigger_at(":", 1), None, "单独 : 不触发");
        assert_eq!(trigger_at("say \"a:b", 7), None, "引号内不触发");
        assert_eq!(trigger_at("say hello", 9), None, "参数位不触发");
    }

    #[test]
    fn current_token_splits_on_whitespace() {
        assert_eq!(current_token("say hello", 9), (4, "hello".to_string()));
        assert_eq!(current_token("say hello", 4), (4, "".to_string()));
        assert_eq!(current_token("stop", 4), (0, "stop".to_string()));
    }

    #[test]
    fn common_prefix_is_char_safe() {
        assert_eq!(common_prefix("stop", "store"), "sto");
        assert_eq!(common_prefix("help", "stop"), "");
        assert_eq!(common_prefix("你好a", "你好b"), "你好");
    }
}
