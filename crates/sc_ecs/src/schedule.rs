use crate::define_label;
use crate::intern::Interned;
use crate::system::{IntoSystem, System, SystemManager};
use ahash::{HashMap, HashMapExt};
pub use sc_ecs_macros::ScheduleLabel;

define_label!(ScheduleLabel, SCHEDULE_LABEL_INTERNER);

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct First;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreUpdate;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunFixedMainLoop;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Update;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostUpdate;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Last;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreStartup;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Startup;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostStartup;

/// Schedule table. Not an ECS resource: held by `App` through `ScheduleHandle`
/// (`app/mod.rs`). The lock on this struct is never held while systems run, so
/// dynamic plugins registering systems during startup `enable()` cannot deadlock.
pub struct ScheduleManager {
    schedules: HashMap<Interned<dyn ScheduleLabel>, SystemManager>,
    labels: Vec<Interned<dyn ScheduleLabel>>,
    startup_labels: Vec<Interned<dyn ScheduleLabel>>,
    first_run: bool,
}

impl ScheduleManager {
    pub fn new() -> Self {
        Self {
            schedules: HashMap::new(),
            labels: vec![
                First.intern(),
                PreUpdate.intern(),
                RunFixedMainLoop.intern(),
                Update.intern(),
                PostUpdate.intern(),
                Last.intern(),
            ],
            startup_labels: vec![PreStartup.intern(), Startup.intern(), PostStartup.intern()],
            first_run: false,
        }
    }

    pub fn insert_before(&mut self, before: impl ScheduleLabel, schedule: impl ScheduleLabel) {
        let index = self
            .labels
            .iter()
            .position(|current| (**current).eq(&before))
            .unwrap_or_else(|| panic!("Expected {before:?} to exist"));
        self.labels.insert(index, schedule.intern());
    }

    pub fn insert_after(&mut self, after: impl ScheduleLabel, schedule: impl ScheduleLabel) {
        let index = self
            .labels
            .iter()
            .position(|current| (**current).eq(&after))
            .unwrap_or_else(|| panic!("Expected {after:?} to exist"));
        self.labels.insert(index + 1, schedule.intern());
    }

    pub fn insert_startup_before(
        &mut self,
        before: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) {
        let index = self
            .startup_labels
            .iter()
            .position(|current| (**current).eq(&before))
            .unwrap_or_else(|| panic!("Expected {before:?} to exist"));
        self.startup_labels.insert(index, schedule.intern());
    }

    pub fn insert_startup_after(
        &mut self,
        after: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) {
        let index = self
            .startup_labels
            .iter()
            .position(|current| (**current).eq(&after))
            .unwrap_or_else(|| panic!("Expected {after:?} to exist"));
        self.startup_labels.insert(index + 1, schedule.intern());
    }

    pub(crate) fn add_systems<S, Marker>(&mut self, schedule: impl ScheduleLabel, system: S)
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        self.schedules
            .entry(schedule.intern())
            .or_insert_with(SystemManager::new)
            .add_systems(system);
    }

    pub(crate) fn add_boxed(
        &mut self,
        label: Interned<dyn ScheduleLabel>,
        system: Box<dyn System>,
    ) {
        self.schedules
            .entry(label)
            .or_insert_with(SystemManager::new)
            .add_boxed(system);
    }

    /// Label sequence to run this tick (startup labels on the first run, main-loop labels after).
    pub(crate) fn plan(&mut self) -> Vec<Interned<dyn ScheduleLabel>> {
        if !self.first_run {
            self.first_run = true;
            self.startup_labels.clone()
        } else {
            self.labels.clone()
        }
    }

    /// Takes out one label's system set (leaving an empty placeholder) for lock-free execution.
    pub(crate) fn take_label(
        &mut self,
        label: &Interned<dyn ScheduleLabel>,
    ) -> Option<SystemManager> {
        self.schedules
            .get_mut(label)
            .map(|systems| std::mem::replace(systems, SystemManager::new()))
    }

    /// Returns the taken-out system set; systems registered to this label
    /// during the run are appended at the end (effective next tick).
    pub(crate) fn restore_label(
        &mut self,
        label: Interned<dyn ScheduleLabel>,
        mut systems: SystemManager,
    ) {
        let entry = self
            .schedules
            .entry(label)
            .or_insert_with(SystemManager::new);
        let added_during_run = std::mem::replace(entry, SystemManager::new());
        systems.append(added_during_run);
        *entry = systems;
    }
}
