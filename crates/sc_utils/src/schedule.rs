use sc_ecs::app::plugin::Plugin;
use sc_ecs::app::App;
use sc_ecs::schedule::{PreStartup, ScheduleLabel, Update};

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCPreStartup;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCStartup;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCPreLoad;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCLoad;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCPostLoad;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCPluginConnectionUpdate;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCConnectionUpdate;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCPluginEventUpdate;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCEventUpdate;

// ---- Movement systems ----
/// Consumes the player input buffer (PlayerAuthInput) and emits validation events without applying movement.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCMovementInput;
/// Threshold-dedup broadcast of MovePlayer / MoveEntityAbsolute / SetEntityMotion plus viewer sync.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCMovementBroadcast;

// ---- Chunk pipeline ----
/// Per-player chunk reorder (spiral load_queue walk / NetworkChunkPublisherUpdate / unload scheduling).
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCChunkReorder;
/// Budgeted per-tick load_queue send (async load + LevelChunkCache encode + PLAYER_SPAWN threshold).
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SCChunkSend;

pub struct SCSchedulePlugin;

impl Plugin for SCSchedulePlugin {
    fn build(&self, app: &App) {
        app.insert_startup_schedule_after(PreStartup, SCPreStartup)
            .insert_startup_schedule_after(SCPreStartup, SCStartup)
            .insert_startup_schedule_after(SCStartup, SCPreLoad)
            .insert_startup_schedule_after(SCPreLoad, SCLoad)
            .insert_startup_schedule_after(SCLoad, SCPostLoad)
            .insert_schedule_before(Update, SCMovementInput)
            .insert_schedule_after(Update, SCMovementBroadcast)
            .insert_schedule_after(SCMovementBroadcast, SCChunkReorder)
            .insert_schedule_after(SCChunkReorder, SCChunkSend)
            .insert_schedule_after(SCChunkSend, SCPluginConnectionUpdate)
            .insert_schedule_after(SCPluginConnectionUpdate, SCConnectionUpdate)
            .insert_schedule_after(SCConnectionUpdate, SCPluginEventUpdate)
            .insert_schedule_after(SCPluginEventUpdate, SCEventUpdate);
    }
}
