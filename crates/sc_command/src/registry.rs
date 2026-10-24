//! 命令注册表：纯数据驱动的命令定义。
//!
//! 这里的命令 = 数据（本注册表）+ 系统（每条命令自己的 ECS 系统）+ 事件流。
//! 注册表有两个消费方：
//! 1. 网络层据此生成 `AvailableCommands`（0x4c）包（客户端补全）；
//! 2. 分发系统据此解析别名并校验权限。
//!
//! 命令实体由版本包插件注册（[`CommandSource::Plugin`]）；SC 核心只带少量与版本包
//! 数据无关的生命周期命令（[`CommandSource::BuiltIn`]）。
//!
//! Command registry: purely data-driven command definitions.
//!
//! A command here is data (this registry) plus systems (each command's own ECS system) plus the event flow.
//! The registry serves two consumers:
//! 1. The network layer generates the `AvailableCommands` (0x4c) packet from it (client completion);
//! 2. The dispatch system uses it for alias resolution and permission checks.
//!
//! Command bodies are registered by version-pack plugins ([`CommandSource::Plugin`]); the SC core only
//! ships a few lifecycle commands independent of version-pack data ([`CommandSource::BuiltIn`]).

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use sc_ecs::resource::Resource;
use sc_error::{ScError, ScSite, capture_site};

/// Bedrock 命令权限级别，对应 `AvailableCommands` 中的 permission 字段。
///
/// Bedrock command permission level, matching the permission string in `AvailableCommands`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandPermissionLevel {
    Any,
    GameDirectors,
    Admin,
    Host,
    Owner,
}

impl CommandPermissionLevel {
    /// `AvailableCommands` 包中的权限字段。
    ///
    /// The permission field in the `AvailableCommands` packet.
    /// All versions (1.20.x-1.21.x) use a single-byte enum: 0=Any, 1=GameDirectors, 2=Admin, 3=Host, 4=Owner.
    pub fn network_id(self) -> u8 {
        match self {
            Self::Any => 0,
            Self::GameDirectors => 1,
            Self::Admin => 2,
            Self::Host => 3,
            Self::Owner => 4,
        }
    }

    /// v898+（1.20.40+）的 `AvailableCommands` 权限字段是字符串
    ///
    /// The v898+ (1.20.40+) `AvailableCommands` permission field is a **string**
    /// (`CommandPermissionLevel.getId()`: any/gamedirectors/admin/host/owner).
    pub fn string_id(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::GameDirectors => "gamedirectors",
            Self::Admin => "admin",
            Self::Host => "host",
            Self::Owner => "owner",
        }
    }
}

/// 命令参数类型。仅用于服务端语义描述与后续 AvailableCommands 编码；在客户端补全
/// 落地前不影响分发（分发只做分词并转发）。
///
/// Command parameter type. Only for server-side semantic description and future AvailableCommands encoding;
/// dispatch is unaffected until client completion lands (dispatch only tokenizes and forwards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandParamType {
    Int,
    Float,
    Value,
    Target,
    String,
    Position,
    Message,
    Text,
    Json,
    Command,
}

#[derive(Clone, Debug)]
pub struct CommandParameter {
    pub name: String,
    pub param_type: CommandParamType,
    pub optional: bool,
}

impl CommandParameter {
    pub fn required(name: &str, param_type: CommandParamType) -> Self {
        Self {
            name: name.to_string(),
            param_type,
            optional: false,
        }
    }

    pub fn optional(name: &str, param_type: CommandParamType) -> Self {
        Self {
            name: name.to_string(),
            param_type,
            optional: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommandOverload {
    pub parameters: Vec<CommandParameter>,
}

/// 命令注册来源。热重载时按来源批量注销。
///
/// Command registration source. Bulk-unregisters by source on hot swap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandSource {
    /// SC core builtin command (server-lifecycle command independent of version-pack data).
    BuiltIn,
    /// Plugin-registered command carrying the plugin name. Version-pack commands come from here.
    Plugin(String),
}

#[derive(Clone, Debug)]
pub struct CommandDefinition {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub permission: CommandPermissionLevel,
    pub overloads: Vec<CommandOverload>,
    pub source: CommandSource,
}

impl CommandDefinition {
    pub fn new(name: &str, description: &str, source: CommandSource) -> Self {
        Self {
            name: name.to_ascii_lowercase(),
            description: description.to_string(),
            aliases: Vec::new(),
            permission: CommandPermissionLevel::Any,
            overloads: Vec::new(),
            source,
        }
    }

    pub fn with_alias(mut self, alias: &str) -> Self {
        self.aliases.push(alias.to_ascii_lowercase());
        self
    }

    pub fn with_permission(mut self, permission: CommandPermissionLevel) -> Self {
        self.permission = permission;
        self
    }

    pub fn with_overload(mut self, overload: CommandOverload) -> Self {
        self.overloads.push(overload);
        self
    }
}

#[derive(Debug, ScError)]
pub enum CommandRegisterError {
    /// Command name is already registered.
    #[sc_error(code = "command.duplicate_name")]
    DuplicateName {
        /// Rejected command name.
        name: String,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Command alias is already registered.
    #[sc_error(code = "command.duplicate_alias")]
    DuplicateAlias {
        /// Rejected command alias.
        alias: String,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl CommandRegisterError {
    /// 构造重名错误并捕获调用位置。
    ///
    /// Build a duplicate-name error, capturing the call site.
    #[track_caller]
    pub fn duplicate_name(name: String) -> Self {
        let detail = format!("command name '{name}' already registered");
        Self::DuplicateName {
            name,
            detail,
            site: capture_site(),
        }
    }

    /// 构造重别名错误并捕获调用位置。
    ///
    /// Build a duplicate-alias error, capturing the call site.
    #[track_caller]
    pub fn duplicate_alias(alias: String) -> Self {
        let detail = format!("command alias '{alias}' already registered");
        Self::DuplicateAlias {
            alias,
            detail,
            site: capture_site(),
        }
    }
}

impl Display for CommandRegisterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateName { detail, site, .. } => {
                write!(formatter, "[command.duplicate_name] {detail} (at {site})")
            }
            Self::DuplicateAlias { detail, site, .. } => {
                write!(formatter, "[command.duplicate_alias] {detail} (at {site})")
            }
        }
    }
}

impl std::error::Error for CommandRegisterError {}


#[derive(Resource, Clone, Default)]
pub struct CommandRegistry {
    commands: HashMap<String, CommandDefinition>,
    aliases: HashMap<String, String>,
    /// Incremented on every register/unregister. The network layer uses it to decide whether online clients
    /// need a fresh AvailableCommands (hot-swap case).
    revision: u64,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, definition: CommandDefinition) -> Result<(), CommandRegisterError> {
        let name = definition.name.clone();
        if self.commands.contains_key(&name) || self.aliases.contains_key(&name) {
            return Err(CommandRegisterError::duplicate_name(name));
        }
        for alias in &definition.aliases {
            if self.commands.contains_key(alias) || self.aliases.contains_key(alias) {
                return Err(CommandRegisterError::duplicate_alias(alias.clone()));
            }
        }
        for alias in &definition.aliases {
            self.aliases.insert(alias.clone(), name.clone());
        }
        self.commands.insert(name, definition);
        self.revision += 1;
        Ok(())
    }

    /// 按来源批量注销（用于插件 hot_disable）。
    ///
    /// Bulk-unregisters by source (for plugin hot_disable).
    pub fn unregister_source(&mut self, source: &CommandSource) -> usize {
        let removed: Vec<String> = self
            .commands
            .values()
            .filter(|definition| &definition.source == source)
            .map(|definition| definition.name.clone())
            .collect();
        for name in &removed {
            if let Some(definition) = self.commands.remove(name) {
                for alias in &definition.aliases {
                    self.aliases.remove(alias);
                }
            }
        }
        if !removed.is_empty() {
            self.revision += 1;
        }
        removed.len()
    }

    /// 名称或别名到命令定义。
    ///
    /// Name or alias to command definition.
    pub fn resolve(&self, name_or_alias: &str) -> Option<&CommandDefinition> {
        let key = name_or_alias.to_ascii_lowercase();
        match self.commands.get(&key) {
            Some(definition) => Some(definition),
            None => self
                .aliases
                .get(&key)
                .and_then(|name| self.commands.get(name)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &CommandDefinition> {
        self.commands.values()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_resolve_by_alias() {
        let mut registry = CommandRegistry::new();
        registry
            .register(
                CommandDefinition::new("teleport", "tp", CommandSource::Plugin("vanilla".into()))
                    .with_alias("tp"),
            )
            .unwrap();
        assert_eq!(registry.resolve("TP").unwrap().name, "teleport");
        assert_eq!(registry.resolve("teleport").unwrap().name, "teleport");
        assert!(registry.resolve("tpx").is_none());
    }

    #[test]
    fn duplicate_name_or_alias_is_rejected() {
        let mut registry = CommandRegistry::new();
        registry
            .register(CommandDefinition::new("help", "", CommandSource::BuiltIn).with_alias("?"))
            .unwrap();
        assert!(registry
            .register(CommandDefinition::new("help", "", CommandSource::BuiltIn))
            .is_err());
        assert!(registry
            .register(CommandDefinition::new("?", "", CommandSource::BuiltIn))
            .is_err());
    }

    #[test]
    fn unregister_source_removes_commands_and_aliases() {
        let mut registry = CommandRegistry::new();
        let source = CommandSource::Plugin("vanilla".into());
        registry
            .register(CommandDefinition::new("say", "", source.clone()).with_alias("broadcast"))
            .unwrap();
        registry
            .register(CommandDefinition::new("stop", "", CommandSource::BuiltIn))
            .unwrap();
        let revision_before = registry.revision();
        assert_eq!(registry.unregister_source(&source), 1);
        assert!(registry.resolve("say").is_none());
        assert!(registry.resolve("broadcast").is_none());
        assert!(registry.resolve("stop").is_some());
        assert!(registry.revision() > revision_before);
    }
}
