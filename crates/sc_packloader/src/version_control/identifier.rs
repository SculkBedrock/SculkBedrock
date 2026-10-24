//! 资源标识符的无损拆分（命名空间 + 路径）。
//!
//! Lossless resource identifier split.

use std::fmt;
use std::sync::Arc;

/// 无损的资源标识符拆分。
///
/// Lossless resource identifier split.
///
/// 命名空间与路径始终成对保留；绝不把 `minecraft:stone`、`ur:stone` 或第三方
/// 命名空间塌缩成单个短字符串。该类型用于边界检查与标识符驻留；既有包加载
/// API 继续接受完整 `&str` 标识符。
///
/// Namespace and path are always kept together; never collapse `minecraft:stone`, `ur:stone`
/// or third-party namespaces into one short string. This type is used for boundary checks and
/// identifier interning; existing packloader APIs keep accepting full `&str` identifiers.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QualifiedId {
    /// 命名空间（无冒号时为空串）。
    ///
    /// Namespace (empty when there is no colon).
    pub namespace: Arc<str>,
    /// 路径部分。
    ///
    /// Path part.
    pub path: Arc<str>,
}

impl QualifiedId {
    /// 从 `<namespace>:<path>` 形式解析标识符；无冒号时命名空间为空。
    ///
    /// Parses an identifier from its `<namespace>:<path>` form; the
    /// namespace is empty when there is no colon.
    ///
    /// - `value`: 原始字符串
    pub fn parse(value: &str) -> Self {
        let (namespace, path) = value.split_once(':').unwrap_or(("", value));
        Self {
            namespace: Arc::from(namespace),
            path: Arc::from(path),
        }
    }

    /// 重组为字符串（命名空间为空时仅返回路径）。
    ///
    /// Reassembles into a string (path only when the namespace is empty).
    pub fn as_str(&self) -> String {
        if self.namespace.is_empty() {
            self.path.to_string()
        } else {
            format!("{}:{}", self.namespace, self.path)
        }
    }
}

impl fmt::Display for QualifiedId {
    /// 以 `<namespace>:<path>`（或无命名空间时的路径）格式化。
    ///
    /// Formats as `<namespace>:<path>` (or just the path without a namespace).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.namespace.is_empty() {
            f.write_str(&self.path)
        } else {
            write!(f, "{}:{}", self.namespace, self.path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::QualifiedId;

    #[test]
    fn namespace_and_path_are_lossless() {
        let ids = ["minecraft:stone", "sc:stone", "plugin_a:stone", "stone"];
        let parsed = ids
            .iter()
            .map(|id| QualifiedId::parse(id))
            .collect::<Vec<_>>();
        assert_eq!(parsed[0], QualifiedId::parse("minecraft:stone"));
        assert_ne!(parsed[0], parsed[1]);
        assert_ne!(parsed[1], parsed[2]);
        assert_eq!(parsed[3].as_str(), "stone");
        assert_eq!(parsed[2].to_string(), "plugin_a:stone");
    }
}
