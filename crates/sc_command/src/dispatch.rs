//! Dispatch system: raw input to normalized invocation.
//!
//! Handles mechanics only (tokenize/alias/permission gate) with no command semantics;
//! each command is a separate ECS system mounted by the builtin module or version-pack plugin.

use log::info;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;

use crate::events::{CommandFeedback, CommandInvocation, CommandOrigin, RawCommandInput};
use crate::registry::{CommandPermissionLevel, CommandRegistry};
use sc_log::t_log;

/// Quote-aware tokenize: `say "hello world" x` becomes `["say", "hello world", "x"]`.
/// Inside double quotes supports `\"` and `\\` escapes; an unterminated quote consumes the rest.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if in_quotes => escaped = true,
            '"' => {
                if in_quotes {
                    tokens.push(std::mem::take(&mut current));
                    in_quotes = false;
                } else {
                    in_quotes = true;
                }
            }
            character if character.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() || in_quotes {
        tokens.push(current);
    }
    tokens
}

/// Permission level of the invoker.
/// Before the player permission component lands, players are treated as [`CommandPermissionLevel::Any`];
/// the console holds the highest permission.
fn origin_permission_level(origin: CommandOrigin) -> CommandPermissionLevel {
    match origin {
        CommandOrigin::Console => CommandPermissionLevel::Owner,
        CommandOrigin::Player(_) => CommandPermissionLevel::Any,
    }
}

pub fn dispatch_commands(world: World, mut reader: EventReader<RawCommandInput>) {
    for input in reader.read() {
        let raw = input.raw.trim();
        let raw = raw.strip_prefix('/').unwrap_or(raw);
        let tokens = tokenize(raw);
        let Some((name, args)) = tokens.split_first() else {
            continue;
        };

        let Some(registry) = world.get_resource::<CommandRegistry>() else {
            continue;
        };
        let Some(definition) = registry.resolve(name) else {
            world.send_event(CommandFeedback {
                origin: input.origin,
                messages: vec![format!(
                    "Unknown command: {name}. Please check that the command exists."
                )],
                success: false,
                request: input.request.clone(),
            });
            continue;
        };

        if origin_permission_level(input.origin) < definition.permission {
            world.send_event(CommandFeedback {
                origin: input.origin,
                messages: vec![format!(
                    "You do not have permission to use /{}.",
                    definition.name
                )],
                success: false,
                request: input.request.clone(),
            });
            continue;
        }

        if let CommandOrigin::Player(entity) = input.origin {
            info!("{}", t_log!("console.command.issued", entity = format!("{entity:?}"), raw = raw));
        }
        world.send_event(CommandInvocation {
            origin: input.origin,
            command: definition.name.clone(),
            args: args.to_vec(),
            request: input.request.clone(),
        });
    }
}

/// Console feedback routing: sc_command does not depend on the network layer; player feedback
/// is handled by the sc_network CommandOutput sender, this only covers console-side log output.
pub fn console_feedback(mut reader: EventReader<CommandFeedback>) {
    for feedback in reader.read() {
        if feedback.origin == CommandOrigin::Console {
            for message in &feedback.messages {
                info!("{}", t_log!("console.command.feedback", message = message));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn splits_on_whitespace() {
        assert_eq!(
            tokenize("tp alice 1 2 3"),
            vec!["tp", "alice", "1", "2", "3"]
        );
    }

    #[test]
    fn keeps_quoted_segments_together() {
        assert_eq!(
            tokenize("say \"hello world\" done"),
            vec!["say", "hello world", "done"]
        );
    }

    #[test]
    fn supports_escapes_inside_quotes() {
        assert_eq!(tokenize(r#"say "a \"b\" \\c""#), vec!["say", r#"a "b" \c"#]);
    }

    #[test]
    fn unterminated_quote_takes_rest_of_line() {
        assert_eq!(tokenize("say \"tail of line"), vec!["say", "tail of line"]);
    }

    #[test]
    fn empty_and_whitespace_input_yield_no_tokens() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
    }
}
