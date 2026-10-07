use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SCPluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub dependencies: Vec<SCPluginDependency>,
    /// Plugin kind:
    /// - `"rust"` (Rust plugin compiled into the server, in-process sc_* use, manifest-only packaging);
    /// - `"cabi"` (default, cross-language cdylib plugin, zip contains plugin.so, loaded via C-ABI).
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "cabi".to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SCPluginDependency {
    pub name: String,
    pub version: String,
}
