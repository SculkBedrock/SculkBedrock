//! Example command plugin for the version pack.
//!
//! Convention for implementing command behavior in a version-pack plugin:
//! 1. On `enable`, register the command definition (data) with
//!    [`CommandRegistry`], tagged with `CommandSource::Plugin`, and
//!    unregister by source in bulk on disable;
//! 2. Command behavior is a plain ECS system that reads
//!    [`CommandInvocation`], filters by name, and runs on the
//!    `SCPluginEventUpdate` schedule;
//! 3. Feedback is sent back via [`CommandFeedback`] events, routed by the
//!    kernel to the console log or a CommandOutput packet.

use log::info;
use sc_command::events::{CommandFeedback, CommandInvocation};
use sc_command::registry::{
    CommandDefinition, CommandOverload, CommandParamType, CommandParameter, CommandPermissionLevel,
    CommandRegistry, CommandSource,
};
use sc_ecs::app::App;
use sc_ecs::params::event::EventReader;
use sc_ecs::world::World;
use sc_log::t_log;
use sc_plugin::SCPlugin;
use sc_utils::schedule::SCPluginEventUpdate;

// Exports the `sc_plugin_log_bridge` symbol: the host injects a log bridge
// at load time so `log::info!`/`warn!` in this cdylib forward to the host
// logger (with a plugin-name prefix).
sc_plugin::declare_log_bridge!();

const PLUGIN_SOURCE: &str = "vanilla_command";

fn plugin_source() -> CommandSource {
    CommandSource::Plugin(PLUGIN_SOURCE.to_string())
}

#[derive(SCPlugin)]
pub struct VanillaCommand;

impl SCPlugin for VanillaCommand {
    fn new() -> Self
    where
        Self: Sized,
    {
        Self
    }

    fn enable(&self, app: &App) {
        // Console language follows the host (SCULK_LOCALE set by bootstrap
        // from server_properties [server] language before dlopen).
        sc_log::set_locale(
            &std::env::var("SCULK_LOCALE").unwrap_or_else(|_| "zh-CN".to_string()),
        );
        let mut registered = false;
        if let Some(mut registry) = app.world().get_resource_mut::<CommandRegistry>() {
            if let Err(error) = registry.register(
                CommandDefinition::new(
                    "say",
                    "Broadcasts a message to the server",
                    plugin_source(),
                )
                .with_permission(CommandPermissionLevel::Any)
                .with_overload(CommandOverload {
                    parameters: vec![CommandParameter::required(
                        "message",
                        CommandParamType::Message,
                    )],
                }),
            ) {
                log::warn!("{}", t_log!("console.command.say_reg_fail", error = error));
            } else {
                registered = true;
            }
        } else {
            log::warn!("{}", t_log!("console.command.registry_missing"));
        }
        app.add_systems(SCPluginEventUpdate, say_command);
        log::info!("{}", t_log!("console.command.loaded", registered = registered));
    }

    fn disable(&self, app: &App) {
        if let Some(mut registry) = app.world().get_resource_mut::<CommandRegistry>() {
            registry.unregister_source(&plugin_source());
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn get_plugin() -> Box<dyn SCPlugin> {
    Box::new(VanillaCommand::new())
}

fn say_command(world: World, mut reader: EventReader<CommandInvocation>) {
    for invocation in reader.read() {
        if invocation.command != "say" {
            continue;
        }
        if invocation.args.is_empty() {
            world.send_event(CommandFeedback::error_from(
                invocation,
                "Usage: /say <message>",
            ));
            continue;
        }
        let message = invocation.args.join(" ");
        // Until player broadcast infrastructure lands, print to the console
        // and reply to the invoker.
        info!("[Server] {message}");
        world.send_event(CommandFeedback::success_from(
            invocation,
            format!("[Server] {message}"),
        ));
    }
}
