//! SC command system core.
//!
//! Design:
//! - **Data**: [`registry::CommandRegistry`] (Resource) holds command definitions,
//!   shared by AvailableCommands packet generation and dispatch validation;
//! - **Systems**: each command is a plain ECS system filtering
//!   [`events::CommandInvocation`] by command name;
//! - **Event flow**: `RawCommandInput -> dispatch -> CommandInvocation -> CommandFeedback`.
//!
//! Layering: command bodies live in version-pack plugins (on `enable` they register
//! definitions and mount their own systems, tagged [`registry::CommandSource::Plugin`]);
//! the SC core only ships lifecycle commands such as /help and /stop.
//!
//! Scheduling: dispatch runs in `SCConnectionUpdate` (right after packet handling); plugin command
//! systems conventionally mount on `SCPluginEventUpdate`, builtin commands and console feedback on `SCEventUpdate`.

pub mod commands;
pub mod dispatch;
pub mod events;
pub mod registry;

use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_utils::schedule::{SCConnectionUpdate, SCEventUpdate};

use crate::commands::builtin;
use crate::events::{CommandFeedback, CommandInvocation, RawCommandInput};
use crate::registry::CommandRegistry;

pub struct SCCommandPlugin;

impl Plugin for SCCommandPlugin {
    fn build(&self, app: &App) {
        let mut registry = CommandRegistry::new();
        builtin::register_builtin_commands(&mut registry);

        app.insert_resource(registry)
            .add_event::<RawCommandInput>()
            .add_event::<CommandInvocation>()
            .add_event::<CommandFeedback>()
            .add_systems(SCConnectionUpdate, dispatch::dispatch_commands)
            .add_systems(
                SCEventUpdate,
                (
                    builtin::help_command,
                    builtin::stop_command,
                    builtin::scversion_command,
                    dispatch::console_feedback,
                ),
            );
    }
}
