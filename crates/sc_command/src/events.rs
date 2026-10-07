//! Command event flow: `RawCommandInput -> CommandInvocation -> CommandFeedback`.
//!
//! The network layer (CommandRequest packet) and the console submit raw input as [`RawCommandInput`];
//! dispatch tokenizes, resolves aliases, checks permissions, then emits [`CommandInvocation`];
//! each command system (builtin or plugin) consumes invocations and emits [`CommandFeedback`];
//! feedback is routed separately by the console logger and the network CommandOutput sender.

use sc_ecs::entity::EntityId;
use sc_ecs::event::Event;
use uuid::Uuid;

/// Command invoker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOrigin {
    Player(EntityId),
    Console,
}

/// Receipt context from the CommandRequest packet, echoed verbatim by the CommandOutput packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRequestContext {
    pub origin_type: u32,
    pub uuid: Uuid,
    pub request_id: String,
}

#[derive(Event, Clone, Debug)]
pub struct RawCommandInput {
    pub origin: CommandOrigin,
    /// Raw input, with or without a leading '/'.
    pub raw: String,
    pub request: Option<CommandRequestContext>,
}

#[derive(Event, Clone, Debug)]
pub struct CommandInvocation {
    pub origin: CommandOrigin,
    /// Normalized command name (alias resolved, lowercased).
    pub command: String,
    /// Quote-aware tokenized arguments.
    pub args: Vec<String>,
    pub request: Option<CommandRequestContext>,
}

#[derive(Event, Clone, Debug)]
pub struct CommandFeedback {
    pub origin: CommandOrigin,
    pub messages: Vec<String>,
    pub success: bool,
    pub request: Option<CommandRequestContext>,
}

impl CommandFeedback {
    pub fn success_from(invocation: &CommandInvocation, message: impl Into<String>) -> Self {
        Self {
            origin: invocation.origin,
            messages: vec![message.into()],
            success: true,
            request: invocation.request.clone(),
        }
    }

    pub fn error_from(invocation: &CommandInvocation, message: impl Into<String>) -> Self {
        Self {
            origin: invocation.origin,
            messages: vec![message.into()],
            success: false,
            request: invocation.request.clone(),
        }
    }
}
