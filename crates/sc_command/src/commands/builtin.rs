//! SC core builtin commands.
//!
//! Only server-lifecycle commands independent of version-pack data (/help, /stop);
//! vanilla gameplay commands (/say, /gamemode, /give, ...) depend on the version-pack registry
//! and are registered/implemented by version-pack plugins (e.g. vanilla_command).

use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_utils::event::{SCExit, SCExitReason, SCExitType};

use crate::events::{CommandFeedback, CommandInvocation};
use crate::registry::{CommandDefinition, CommandPermissionLevel, CommandRegistry, CommandSource};

/// Registers all builtin command definitions.
pub fn register_builtin_commands(registry: &mut CommandRegistry) {
    let _ = registry.register(
        CommandDefinition::new(
            "help",
            "Shows the list of available commands",
            CommandSource::BuiltIn,
        )
        .with_alias("?")
        .with_permission(CommandPermissionLevel::Any),
    );
    let _ = registry.register(
        CommandDefinition::new("stop", "Stops the server", CommandSource::BuiltIn)
            .with_permission(CommandPermissionLevel::Host),
    );
    let _ = registry.register(
        CommandDefinition::new(
            "scversion",
            "Shows the server core version",
            CommandSource::BuiltIn,
        )
        .with_permission(CommandPermissionLevel::Any),
    );
    let _ = registry.register(
        CommandDefinition::new(
            "gamemode",
            "Sets a player's game mode",
            CommandSource::BuiltIn,
        )
        // Before the player permission component lands, dispatch allows all as Any; the OP gate checks is_op at execution.
        .with_permission(CommandPermissionLevel::Any)
        .with_overload(crate::registry::CommandOverload {
            parameters: vec![
                crate::registry::CommandParameter::required(
                    "gameMode",
                    crate::registry::CommandParamType::String,
                ),
                crate::registry::CommandParameter::optional(
                    "player",
                    crate::registry::CommandParamType::Target,
                ),
            ],
        }),
    );
}

pub fn help_command(world: World, mut reader: EventReader<CommandInvocation>) {
    for invocation in reader.read() {
        if invocation.command != "help" {
            continue;
        }
        let Some(registry) = world.get_resource::<CommandRegistry>() else {
            continue;
        };
        let mut lines: Vec<String> = registry
            .iter()
            .map(|definition| format!("/{} - {}", definition.name, definition.description))
            .collect();
        lines.sort();
        world.send_event(CommandFeedback {
            origin: invocation.origin,
            messages: lines,
            success: true,
            request: invocation.request.clone(),
        });
    }
}

pub fn stop_command(world: World, mut reader: EventReader<CommandInvocation>) {
    for invocation in reader.read() {
        if invocation.command != "stop" {
            continue;
        }
        world.send_event(CommandFeedback::success_from(
            invocation,
            "Stopping the server...",
        ));
        world.send_event(SCExit::new(SCExitReason::Success, SCExitType::Shutdown));
    }
}

/// /scversion: shows the server core version (gold/cyan styling).
pub fn scversion_command(world: World, mut reader: EventReader<CommandInvocation>) {
    for invocation in reader.read() {
        if invocation.command != "scversion" {
            continue;
        }
        let message = "§e服务端版本：\n\n§a当前服务器正在运行 §bSculkBedrock 1.0.0 Alpha(fjord) §a服务器核心\n\n§e游戏版本：§bVanilla 1.26.40(Protocol Version: 2168)\n\n§e世界生成器：§bVanilla";
        world.send_event(CommandFeedback::success_from(invocation, message));
    }
}
