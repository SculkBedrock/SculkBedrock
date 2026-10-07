use sc_ecs::event::Event;
use std::error::Error;
use std::num::NonZeroU8;

pub enum SCExitReason {
    Success,
    ErrorCode(NonZeroU8),
    Error(Box<dyn Error + Send + Sync + 'static>),
}

pub enum SCExitType {
    Shutdown,
    Restart,
}

#[derive(Event)]
pub struct SCExit {
    pub reason: SCExitReason,
    pub exit_type: SCExitType,
}

impl SCExit {
    pub fn new(reason: SCExitReason, exit_type: SCExitType) -> Self {
        Self { reason, exit_type }
    }
}
