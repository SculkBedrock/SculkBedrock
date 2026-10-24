//! 资源包清单（manifest.json）模型与 JSON 转换。
//!
//! Resource pack manifest (manifest.json) model and JSON conversion.

use crate::pack::sub_pack::SubPackInformation;
use crate::pack::ResourcePackType;
use sc_utils::game::structs::minecraft_version::{MinecraftVersion, MinecraftVersionEnum};
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

/// 包作用域：世界包或全局包。
///
/// Pack scope: world pack or global pack.
#[derive(Clone, Debug)]
pub enum PackScope {
    World,
    Any,
}

/// 包能力标记；未知能力保留原始字符串。
///
/// Pack capability flags; unknown capabilities keep the raw string.
#[derive(Clone, Debug)]
pub enum ResourcePackCapabilities {
    Unknown(String),
    Chemistry,
    EditorExtension,
    ExperimentalCustomUI,
    Raytraced,
}

/// 依赖描述：模块名、UUID、版本（均可空）。
///
/// Dependency description: module name, UUID, version (all optional).
#[derive(Clone, Debug)]
pub struct ResourcePackDependencies {
    /// 依赖的模块名。
    ///
    /// Module name of the dependency.
    pub module_name: Option<String>,
    /// 依赖的包 UUID。
    ///
    /// UUID of the dependency pack.
    pub uuid: Option<Uuid>,
    /// 依赖的包版本。
    ///
    /// Version of the dependency pack.
    pub version: Option<MinecraftVersion>,
}

/// 包头部信息（manifest header）。
///
/// Pack header information (manifest header).
#[derive(Clone, Debug)]
pub struct ResourcePackInformation {
    /// 包描述文本。
    ///
    /// Pack description text.
    pub description: String,
    /// 包版本。
    ///
    /// Pack version.
    pub version: MinecraftVersion,
    /// 包 UUID。
    ///
    /// Pack UUID.
    pub uuid: Uuid,
    /// 包字节长度。
    ///
    /// Pack byte length.
    pub pack_len: usize,
    /// 加密密钥（未加密时为空）。
    ///
    /// Encryption key (empty when unencrypted).
    pub encryption_key: String,
    /// 包内容 SHA-256。
    ///
    /// SHA-256 of the pack contents.
    pub sha256: Vec<u8>,
}

/// 清单 metadata 段（作者、许可证、生成工具等，均可缺省）。
///
/// Manifest metadata section (authors, license, generators, etc.; all optional).
#[derive(Deserialize, Clone, Debug)]
pub struct ResourcePackMetadata {
    /// 作者列表。
    ///
    /// Author list.
    #[serde(default)]
    pub authors: Vec<String>,
    /// 许可证。
    ///
    /// License.
    #[serde(default)]
    pub license: Option<String>,
    /// 生成工具信息。
    ///
    /// Generator information.
    #[serde(default)]
    pub generated_with: Vec<HashMap<String, Vec<String>>>,
    /// 产品类型。
    ///
    /// Product type.
    #[serde(default)]
    pub product_type: Option<String>,
    /// 项目链接。
    ///
    /// Project URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// 规范化后的资源包清单。
///
/// Normalized resource pack manifest.
#[derive(Clone, Debug)]
pub struct ResourcePackManifest {
    /// 清单格式版本。
    ///
    /// Manifest format version.
    pub format_version: u16,
    /// 包名。
    ///
    /// Pack name.
    pub name: String,
    /// 头部信息。
    ///
    /// Header information.
    pub information: ResourcePackInformation,
    /// 最低引擎版本。
    ///
    /// Minimum engine version.
    pub min_engine_version: Option<MinecraftVersion>,
    /// 子包列表。
    ///
    /// Sub-pack list.
    pub sub_packs: Vec<SubPackInformation>,
    /// 是否允许随机种子。
    ///
    /// Whether a random seed is allowed.
    pub allow_random_seed: bool,
    /// 基础游戏版本。
    ///
    /// Base game version.
    pub base_game_version: Option<MinecraftVersion>,
    /// 是否锁定模板选项。
    ///
    /// Whether template options are locked.
    pub lock_template_options: bool,
    /// 包作用域。
    ///
    /// Pack scope.
    pub pack_scope: Option<PackScope>,
    /// 依赖列表。
    ///
    /// Dependency list.
    pub dependencies: Vec<ResourcePackDependencies>,
    /// 能力列表。
    ///
    /// Capability list.
    pub capabilities: Vec<ResourcePackCapabilities>,
    /// 附加元数据。
    ///
    /// Additional metadata.
    pub metadata: Option<ResourcePackMetadata>,
    /// 包版本（与 `information.version` 一致，便于直接访问）。
    ///
    /// Pack version (same as `information.version`, kept for direct access).
    pub version: MinecraftVersion,
}

/// manifest.json 的 header 段原始结构。
///
/// Raw header section of manifest.json.
#[derive(Deserialize, Debug)]
pub(crate) struct ResourcePackManifestJsonHeader {
    /// 是否允许随机种子。
    ///
    /// Whether a random seed is allowed.
    #[serde(default)]
    pub allow_random_seed: bool,
    /// 基础游戏版本。
    ///
    /// Base game version.
    #[serde(default)]
    pub base_game_version: Option<MinecraftVersionEnum>,
    /// 包描述文本。
    ///
    /// Pack description text.
    #[serde(default)]
    pub description: String,
    /// 是否锁定模板选项。
    ///
    /// Whether template options are locked.
    #[serde(default)]
    pub lock_template_options: bool,
    /// 最低引擎版本。
    ///
    /// Minimum engine version.
    #[serde(default)]
    pub min_engine_version: Option<MinecraftVersionEnum>,
    /// 包名。
    ///
    /// Pack name.
    #[serde(default)]
    pub name: String,
    /// 包作用域字符串。
    ///
    /// Pack scope string.
    #[serde(default)]
    pub pack_scope: Option<String>,
    /// 包 UUID 字符串。
    ///
    /// Pack UUID string.
    #[serde(default)]
    pub uuid: String,
    /// 包版本。
    ///
    /// Pack version.
    pub version: MinecraftVersionEnum,
}

/// manifest.json 的 modules 段原始结构（schema/数据模型字段为兼容保留）。
///
/// Raw modules section of manifest.json (schema/data-model fields are kept
/// for compatibility).
#[derive(Deserialize, Debug)]
pub(crate) struct ResourcePackManifestJsonModules {
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    pub description: Option<String>,
    /// 模块类型（如 `resources`/`data`）。
    ///
    /// Module type (e.g. `resources`/`data`).
    #[serde(alias = "type")]
    #[serde(default)]
    pub module_type: String,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    pub uuid: String,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    pub version: Option<MinecraftVersionEnum>,
    // Data-pack schema field, kept for compatibility
    #[allow(dead_code)]
    #[serde(default)]
    pub language: Option<String>,
}

/// manifest.json 的 dependencies 段原始结构。
///
/// Raw dependencies section of manifest.json.
#[derive(Deserialize, Debug)]
pub(crate) struct ResourcePackManifestJsonDependencies {
    /// 依赖 UUID 字符串。
    ///
    /// Dependency UUID string.
    #[serde(default)]
    pub uuid: Option<String>,
    /// 依赖模块名。
    ///
    /// Dependency module name.
    #[serde(default)]
    pub module_name: Option<String>,
    /// 依赖版本。
    ///
    /// Dependency version.
    #[serde(default)]
    pub version: Option<MinecraftVersionEnum>,
}

/// manifest.json 整体原始结构。
///
/// Raw overall structure of manifest.json.
#[derive(Deserialize, Debug)]
pub(crate) struct ResourcePackManifestJson {
    /// 清单格式版本。
    ///
    /// Manifest format version.
    pub format_version: u16,
    /// header 段。
    ///
    /// Header section.
    #[serde(default)]
    pub header: Option<ResourcePackManifestJsonHeader>,
    /// modules 段。
    ///
    /// Modules section.
    #[serde(default)]
    pub modules: Vec<ResourcePackManifestJsonModules>,
    /// capabilities 段。
    ///
    /// Capabilities section.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// dependencies 段。
    ///
    /// Dependencies section.
    #[serde(default)]
    pub dependencies: Vec<ResourcePackManifestJsonDependencies>,
    /// metadata 段。
    ///
    /// Metadata section.
    #[serde(default)]
    pub metadata: Option<ResourcePackMetadata>,
}

impl ResourcePackManifestJson {
    /// 将可选版本枚举转换为规范化版本；解析失败视为无。
    ///
    /// Converts an optional version enum into a normalized version; a parse
    /// failure counts as absent.
    fn option_version(version: Option<MinecraftVersionEnum>) -> Option<MinecraftVersion> {
        if let Some(version) = version {
            version.to_version().ok()
        } else {
            None
        }
    }

    /// 按首个模块类型判定包类型（data/client_data/skin_pack → 行为包，
    /// resources → 资源包；缺省按资源包）。
    ///
    /// Determines the pack type from the first module type
    /// (data/client_data/skin_pack → behavior, resources → resource;
    /// defaults to resource).
    pub fn get_pack_type(&self) -> ResourcePackType {
        for module in &self.modules {
            match module.module_type.as_str() {
                "data" | "client_data" | "skin_pack" => return ResourcePackType::Behavior,
                "resources" => return ResourcePackType::Resource,
                _ => {}
            }
        }
        ResourcePackType::Resource
    }

    /// 转换为规范化清单；header 缺失、UUID/版本非法或依赖解析失败时返回错误串。
    ///
    /// Converts into a normalized manifest; returns an error string when the
    /// header is missing, the UUID/version is invalid, or a dependency fails
    /// to parse.
    ///
    /// # Errors
    /// header 缺失、UUID/版本非法或依赖解析失败时返回描述错误。
    pub fn to_manifest(self) -> Result<ResourcePackManifest, String> {
        let header = self
            .header
            .ok_or_else(|| "manifest header is missing".to_string())?;
        let uuid = Uuid::parse_str(&header.uuid)
            .map_err(|error| format!("invalid manifest header uuid: {error}"))?;
        let version = header
            .version
            .clone()
            .to_version()
            .map_err(|error| format!("invalid manifest version: {error}"))?;
        let pack_scope = match header.pack_scope {
            None => None,
            Some(pack_scope) => {
                if pack_scope == "world" {
                    Some(PackScope::World)
                } else {
                    Some(PackScope::Any)
                }
            }
        };

        // Convert JsonDependencies into Dependencies
        let dependencies: Vec<ResourcePackDependencies> = self
            .dependencies
            .iter()
            .map(|dependency| -> Result<ResourcePackDependencies, String> {
                let uuid = dependency
                    .uuid
                    .as_ref()
                    .map(|uuid| {
                        Uuid::parse_str(uuid)
                            .map_err(|error| format!("invalid dependency uuid `{uuid}`: {error}"))
                    })
                    .transpose()?;
                let module_name = dependency.module_name.clone();
                let version = dependency.version.clone();
                Ok(ResourcePackDependencies {
                    module_name,
                    uuid,
                    version: Self::option_version(version),
                })
            })
            .collect::<Result<_, _>>()?;

        // Convert JsonCapabilities into Capabilities
        let capabilities: Vec<ResourcePackCapabilities> = self
            .capabilities
            .iter()
            .map(|capability| match capability.as_str() {
                "chemistry" => ResourcePackCapabilities::Chemistry,
                "editor_extension" => ResourcePackCapabilities::EditorExtension,
                "experimental_custom_ui" => ResourcePackCapabilities::ExperimentalCustomUI,
                "raytraced" => ResourcePackCapabilities::Raytraced,
                _ => ResourcePackCapabilities::Unknown(capability.to_string()),
            })
            .collect();

        Ok(ResourcePackManifest {
            format_version: self.format_version,
            name: header.name,
            information: ResourcePackInformation {
                description: header.description,
                version,
                uuid,
                pack_len: 0,
                encryption_key: String::new(),
                sha256: vec![],
            },
            min_engine_version: Self::option_version(header.min_engine_version),
            sub_packs: vec![],
            allow_random_seed: header.allow_random_seed,
            base_game_version: Self::option_version(header.base_game_version),
            lock_template_options: header.lock_template_options,
            pack_scope,
            dependencies,
            capabilities,
            metadata: self.metadata,
            version: header
                .version
                .clone()
                .to_version()
                .map_err(|error| format!("invalid manifest version: {error}"))?,
        })
    }
}
