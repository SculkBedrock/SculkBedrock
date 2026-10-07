use std::fmt;
use std::sync::Arc;

/// Lossless resource identifier split.
///
/// Namespace and path are always kept together; never collapse `minecraft:stone`, `ur:stone`
/// or third-party namespaces into one short string. This type is used for boundary checks and
/// later interning; existing packloader APIs keep accepting full `&str` identifiers.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QualifiedId {
    pub namespace: Arc<str>,
    pub path: Arc<str>,
}

impl QualifiedId {
    pub fn parse(value: &str) -> Self {
        let (namespace, path) = value.split_once(':').unwrap_or(("", value));
        Self {
            namespace: Arc::from(namespace),
            path: Arc::from(path),
        }
    }

    pub fn as_str(&self) -> String {
        if self.namespace.is_empty() {
            self.path.to_string()
        } else {
            format!("{}:{}", self.namespace, self.path)
        }
    }
}

impl fmt::Display for QualifiedId {
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
