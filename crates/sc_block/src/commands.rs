//! 方块核心调试命令（机制可观测，非玩法命令）。
//!
//! Block-core debug commands (mechanics observability, not gameplay commands).
//!
//! - `/blockat [x y z]`：显示该坐标处方块的标识符、states 与哈希/network id。
//!   不带参数的玩家查询脚下方块；控制台必须传坐标。
//! - `/setblock <x> <y> <z> <block>`：经单写者队列写入方块。
//!   引导模式下方块解析为该标识符的首见状态，未见过的即时哈希空 states。
//!
//! - `/blockat [x y z]`: shows the identifier, states, and hash/network id at the coordinates.
//!   A parameter-less player queries the block underfoot; the console must pass coordinates.
//! - `/setblock <x> <y> <z> <block>`: writes a block through the single-writer queue.
//!   In bootstrap mode the block resolves to that identifier's first-seen state, or hashes empty
//!   states on the fly when unseen.

use sc_command::events::{CommandFeedback, CommandInvocation, CommandOrigin};
use sc_command::registry::{
    CommandDefinition, CommandOverload, CommandParamType, CommandParameter, CommandPermissionLevel,
    CommandRegistry, CommandSource,
};
use sc_ecs::params::event::EventReader;
use sc_ecs::params::resource::ResMut;
use sc_ecs::world::World;
use sc_utils::game::client::MinecraftClient;
use sc_utils::world::r#type::WorldType;
use sc_world::block_dictionary::{BlockStateDictionary, BlockStateEntry};
use sc_world::chunk::BlockRuntimeId;
use sc_world::leveldb::block_hash::block_state_hash;
use sc_world::manager::{MinecraftWorldId, MinecraftWorldManager};

use crate::access::BlockRead;
use crate::position::BlockPosition;
use crate::registry::BlockStateRegistry;
use crate::write::{update_flags, BlockChange, BlockChangeCause, BlockChangeQueue};

/// 向命令注册表注册方块核心调试命令（`/blockat`、`/setblock`）。
///
/// Registers the block-core debug commands (`/blockat`, `/setblock`) into the
/// command registry.
///
/// - `registry`: 目标命令注册表
pub fn register_block_commands(registry: &mut CommandRegistry) {
    let _ = registry.register(
        CommandDefinition::new(
            "blockat",
            "Shows the block at the given coordinates",
            CommandSource::BuiltIn,
        )
        .with_permission(CommandPermissionLevel::Any)
        .with_overload(CommandOverload {
            parameters: vec![
                CommandParameter::optional("x", CommandParamType::Int),
                CommandParameter::optional("y", CommandParamType::Int),
                CommandParameter::optional("z", CommandParamType::Int),
            ],
        }),
    );
    let _ = registry.register(
        CommandDefinition::new(
            "setblock",
            "Sets a block at the given coordinates",
            CommandSource::BuiltIn,
        )
        // The player permission component is not in place yet, so every command stays
        // Any-level today; tightening early would break in-game use.
        .with_permission(CommandPermissionLevel::Any)
        .with_overload(CommandOverload {
            parameters: vec![
                CommandParameter::required("x", CommandParamType::Int),
                CommandParameter::required("y", CommandParamType::Int),
                CommandParameter::required("z", CommandParamType::Int),
                CommandParameter::required("block", CommandParamType::String),
            ],
        }),
    );
}

/// 归一化方块标识符：转小写，无命名空间时补 `minecraft:` 前缀。
///
/// Normalizes a block identifier: lowercase, prefixes `minecraft:` when no namespace is present.
///
/// - `raw`: 原始标识符
pub(crate) fn normalize_identifier(raw: &str) -> String {
    let lowered = raw.to_ascii_lowercase();
    if lowered.contains(':') {
        lowered
    } else {
        format!("minecraft:{lowered}")
    }
}

/// 引导模式下标识符到网络哈希的解析：优先取字典首见状态，
/// 未见过的即时哈希空 states（并登记，供 `/blockat` 反查）。
///
/// Bootstrap-mode identifier-to-network-hash resolution: prefers the dictionary's
/// first-seen state, hashing empty states on the fly when unseen (and registering it
/// for /blockat reverse lookup).
///
/// - `identifier`: 归一化后的方块标识符
pub(crate) fn resolve_block_state(identifier: &str) -> BlockRuntimeId {
    let dictionary = BlockStateDictionary::global();
    if let Some(hash) = dictionary.first_hash_of(identifier) {
        return BlockRuntimeId(hash);
    }
    let hash = block_state_hash(identifier, None);
    let name = identifier.to_string();
    dictionary.record_with(hash, || BlockStateEntry { name, states: None });
    BlockRuntimeId(hash)
}

/// 取调用者所在世界（玩家：其所在世界；控制台：主世界）。
///
/// World of the invoker (player: their world; console: the main world).
///
/// - `world`: ECS 世界，用于取组件与资源
/// - `origin`: 命令来源（玩家实体或控制台）
fn resolve_world_id(world: &World, origin: CommandOrigin) -> Option<MinecraftWorldId> {
    match origin {
        CommandOrigin::Player(entity) => world
            .get_component::<MinecraftWorldId>(&entity)
            .map(|world_id| (*world_id).clone()),
        CommandOrigin::Console => {
            let manager = world.get_resource::<MinecraftWorldManager>()?;
            manager
                .get_worlds_by_type(&WorldType::Overworld)
                .first()
                .map(|overworld| overworld.world_id.clone())
        }
    }
}

/// 解析 `/blockat` 的可选坐标参数。空 `args` 返回 `Ok(None)`（由调用方决定默认坐标）；
/// 个数或格式错误返回 `Err`。
///
/// Empty `args` returns Ok(None) (caller decides the default coordinates); wrong count
/// or format returns Err.
///
/// - `args`: 命令参数字符串切片
pub(crate) fn parse_coordinates(args: &[String]) -> Result<Option<BlockPosition>, String> {
    if args.is_empty() {
        return Ok(None);
    }
    if args.len() != 3 {
        return Err("Usage: /blockat [x y z]".to_string());
    }
    let mut parsed = [0i32; 3];
    for (index, argument) in args.iter().enumerate() {
        parsed[index] = argument
            .parse::<i32>()
            .map_err(|_| format!("'{argument}' is not a valid integer coordinate"))?;
    }
    Ok(Some(BlockPosition::new(parsed[0], parsed[1], parsed[2])))
}

/// `/blockat` 命令系统：解析坐标、读方块并回发标识符与哈希描述。
///
/// `/blockat` command system: resolves coordinates, reads the block and replies with
/// its identifier and hash description.
///
/// - `world`: ECS 世界
/// - `reader`: 命令调用事件读取器
pub fn blockat_command(world: World, mut reader: EventReader<CommandInvocation>) {
    for invocation in reader.read() {
        if invocation.command != "blockat" {
            continue;
        }

        let position = match parse_coordinates(&invocation.args) {
            Err(message) => {
                world.send_event(CommandFeedback::error_from(invocation, message));
                continue;
            }
            Ok(Some(position)) => position,
            Ok(None) => match invocation.origin {
                CommandOrigin::Player(entity) => {
                    let Some(client) = world.get_component::<MinecraftClient>(&entity) else {
                        continue;
                    };
                    let feet = client.data.read().position;
                    BlockPosition::from_float(feet.x, feet.y, feet.z)
                }
                CommandOrigin::Console => {
                    world.send_event(CommandFeedback::error_from(
                        invocation,
                        "Console must specify coordinates: /blockat <x> <y> <z>",
                    ));
                    continue;
                }
            },
        };

        let Some(world_manager) = world.get_resource::<MinecraftWorldManager>() else {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "World manager is not ready",
            ));
            continue;
        };
        let minecraft_world = match invocation.origin {
            CommandOrigin::Player(entity) => world
                .get_component::<MinecraftWorldId>(&entity)
                .and_then(|world_id| world_manager.get_world(world_id.as_ref())),
            CommandOrigin::Console => world_manager
                .get_worlds_by_type(&WorldType::Overworld)
                .first()
                .copied(),
        };
        let Some(minecraft_world) = minecraft_world else {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "No world available",
            ));
            continue;
        };

        let message = match minecraft_world.get_block(position) {
            Err(error) => format!("Failed to read block at {position}: {error}"),
            Ok(None) => format!("{position}: chunk not generated or y out of range"),
            Ok(Some(runtime_id)) => {
                let registry = world.get_resource::<BlockStateRegistry>();
                let described = registry
                    .as_ref()
                    .and_then(|registry| registry.describe(runtime_id));
                match described {
                    Some(entry) => format!("{position}: {entry} (0x{:08X})", runtime_id.0),
                    None => format!("{position}: unregistered state (0x{:08X})", runtime_id.0),
                }
            }
        };
        let success = !message.contains("Failed");
        world.send_event(CommandFeedback {
            origin: invocation.origin,
            messages: vec![message],
            success,
            request: invocation.request.clone(),
        });
    }
}

/// `/setblock` 命令系统：解析坐标与方块标识符，经单写者队列入队一次方块变更。
///
/// `/setblock` command system: resolves coordinates and block identifier, and pushes
/// one block change through the single-writer queue.
///
/// - `world`: ECS 世界
/// - `queue`: 方块变更队列（单写者）
/// - `reader`: 命令调用事件读取器
pub fn setblock_command(
    world: World,
    mut queue: ResMut<BlockChangeQueue>,
    mut reader: EventReader<CommandInvocation>,
) {
    for invocation in reader.read() {
        if invocation.command != "setblock" {
            continue;
        }
        if invocation.args.len() != 4 {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "Usage: /setblock <x> <y> <z> <block>",
            ));
            continue;
        }
        let position = match parse_coordinates(&invocation.args[0..3]) {
            Ok(Some(position)) => position,
            Ok(None) | Err(_) => {
                world.send_event(CommandFeedback::error_from(
                    invocation,
                    "Coordinates must be integers: /setblock <x> <y> <z> <block>",
                ));
                continue;
            }
        };
        let identifier = normalize_identifier(&invocation.args[3]);
        let state = resolve_block_state(&identifier);
        let Some(world_id) = resolve_world_id(&world, invocation.origin) else {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "No world available",
            ));
            continue;
        };
        let cause = match invocation.origin {
            CommandOrigin::Player(entity) => BlockChangeCause::Player(entity),
            CommandOrigin::Console => BlockChangeCause::Command,
        };
        let request_id = queue.push(BlockChange {
            world_id,
            position,
            layer: 0,
            state,
            cause,
            flags: update_flags::DEFAULT,
        });
        world.send_event(CommandFeedback::success_from(
            invocation,
            format!(
                "Set {position} to {identifier} (0x{:08X}, request #{request_id})",
                state.0
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn empty_args_are_ok_none() {
        assert_eq!(parse_coordinates(&[]).unwrap(), None);
    }

    #[test]
    fn three_integers_parse_including_negatives() {
        assert_eq!(
            parse_coordinates(&args(&["-5", "64", "128"])).unwrap(),
            Some(BlockPosition::new(-5, 64, 128)),
        );
    }

    #[test]
    fn wrong_arity_or_non_integer_is_error() {
        assert!(parse_coordinates(&args(&["1", "2"])).is_err());
        assert!(parse_coordinates(&args(&["1", "2", "x"])).is_err());
    }

    #[test]
    fn identifier_normalization_adds_namespace_and_lowers() {
        assert_eq!(normalize_identifier("Stone"), "minecraft:stone");
        assert_eq!(normalize_identifier("custom:Block"), "custom:block");
    }

    #[test]
    fn resolve_block_state_is_stable_and_registered() {
        let first = resolve_block_state("minecraft:sc_test_block");
        let second = resolve_block_state("minecraft:sc_test_block");
        assert_eq!(first, second);
        // On-the-fly states are registered, so /blockat can reverse-lookup them.
        assert_eq!(
            BlockStateDictionary::global().get(first.0).unwrap().name,
            "minecraft:sc_test_block",
        );
    }
}
