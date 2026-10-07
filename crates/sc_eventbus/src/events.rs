use sc_ecs::app::App;
pub use sc_eventbus_macros::{SCCancellableEvent, SCEnumEvents, SCEvent};

pub trait SCEventTrait {
    fn cancellable() -> bool;
}

pub trait SCEnumEvents {
    fn add_events(app: &App);
}
