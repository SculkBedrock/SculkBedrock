//! `App`: application assembly and schedule execution: plugin registration,
//! schedule table management, main-loop runner.
//!
//! Plugins register systems and resources during `build`; `ScheduleManager`
//! runs systems in take/run/restore mode (no lock held during execution, and
//! runtime `add_systems` from dynamic plugins lands in a pending queue that
//! merges next tick).
use crate::app::plugin::{PlaceholderPlugin, Plugin, Plugins, PluginsState};
use crate::event::{Event, EventUpdaters, Events};
use crate::intern::Interned;
use crate::resource::Resource;
use crate::schedule::{ScheduleLabel, ScheduleManager};
use crate::system::{IntoSystem, System};
use crate::world::World;
use log::debug;
use parking_lot::{Mutex, RwLock};
use std::collections::HashSet;
use std::error::Error;
use std::num::NonZeroU8;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::process::{ExitCode, Termination};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub mod plugin;
pub mod schedule_runner;

fn run_once(app: &App) -> AppExit {
    app.finish();
    app.cleanup();
    app.update();
    app.should_exit().unwrap_or(AppExit::Success)
}

#[derive(Event, Debug, Clone, Default, PartialEq, Eq)]
pub enum AppExit {
    /// [`App`] exited without any problems.
    #[default]
    Success,
    /// The [`App`] experienced an unhandleable error.
    /// Holds the exit code we expect our app to return.
    Error(NonZeroU8),
}

impl AppExit {
    /// Creates a [`AppExit::Error`] with a error code of 1.
    #[must_use]
    pub const fn error() -> Self {
        Self::Error(NonZeroU8::MIN)
    }

    /// Returns `true` if `self` is a [`AppExit::Success`].
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, AppExit::Success)
    }

    /// Returns `true` if `self` is a [`AppExit::Error`].
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, AppExit::Error(_))
    }

    /// Creates a [`AppExit`] from a code.
    ///
    /// When `code` is 0 a [`AppExit::Success`] is constructed otherwise a
    /// [`AppExit::Error`] is constructed.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match NonZeroU8::new(code) {
            Some(code) => Self::Error(code),
            None => Self::Success,
        }
    }
}

impl From<u8> for AppExit {
    #[must_use]
    fn from(value: u8) -> Self {
        Self::from_code(value)
    }
}

impl Termination for AppExit {
    fn report(self) -> ExitCode {
        match self {
            AppExit::Success => ExitCode::SUCCESS,
            AppExit::Error(value) => ExitCode::from(value.get()),
        }
    }
}

#[derive(Debug)]
pub(crate) enum AppError {
    DuplicatePlugin { plugin_name: String },
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for AppError {}

/// Schedule handle: `inner` is never locked while systems run (take/run/restore
/// mode), and `pending` catches the rare registration that collides with a
/// write-lock window (merged at the next sync point).
pub struct ScheduleHandle {
    inner: RwLock<ScheduleManager>,
    pending: Mutex<Vec<(Interned<dyn ScheduleLabel>, Box<dyn System>)>>,
}

impl ScheduleHandle {
    fn new() -> Self {
        Self {
            inner: RwLock::new(ScheduleManager::new()),
            pending: Mutex::new(Vec::new()),
        }
    }

    fn add_system_boxed(&self, label: Interned<dyn ScheduleLabel>, system: Box<dyn System>) {
        match self.inner.try_write() {
            Some(mut inner) => inner.add_boxed(label, system),
            None => self.pending.lock().push((label, system)),
        }
    }

    fn apply_pending(&self) {
        let drained: Vec<_> = std::mem::take(&mut *self.pending.lock());
        if drained.is_empty() {
            return;
        }
        let mut inner = self.inner.write();
        for (label, system) in drained {
            inner.add_boxed(label, system);
        }
    }
}

#[derive(Clone)]
pub struct App {
    world: World,
    schedule: Arc<ScheduleHandle>,
    pub(crate) plugin_registry: Arc<RwLock<Vec<Box<dyn Plugin>>>>,
    pub(crate) plugin_names: Arc<RwLock<HashSet<String>>>,
    pub(crate) plugin_build_depth: Arc<AtomicUsize>,
    pub(crate) plugins_state: Arc<RwLock<PluginsState>>,
    pub(crate) runner: Arc<Mutex<Option<Box<dyn FnOnce(&App) -> AppExit + Send>>>>,
}

impl App {
    pub fn new() -> Self {
        let world = World::new();
        world.insert_resource(EventUpdaters::new());
        Self {
            world,
            schedule: Arc::new(ScheduleHandle::new()),
            plugin_registry: Arc::new(RwLock::new(vec![])),
            plugin_names: Arc::new(RwLock::new(HashSet::new())),
            plugin_build_depth: Arc::new(AtomicUsize::new(0)),
            plugins_state: Arc::new(RwLock::new(PluginsState::Adding)),
            runner: Arc::new(Mutex::new(Some(Box::new(run_once)))),
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn add_systems<S, Marker>(&self, schedule: impl ScheduleLabel, systems: S) -> &Self
    where
        S: IntoSystem<S, Marker> + 'static,
        Marker: 'static,
    {
        let label = schedule.intern();
        let boxed: Box<dyn System> = Box::new(systems.into_system());
        self.schedule.add_system_boxed(label, boxed);
        self
    }

    pub fn insert_resource<R: Resource + 'static + Send + Sync>(&self, resource: R) -> &Self {
        self.world.insert_resource(resource);
        self
    }

    pub fn remove_resource<R: Resource + 'static + Send + Sync>(&self) -> &Self {
        self.world.remove_resource::<R>();
        self
    }

    pub fn add_event<T: Event + 'static + Send + Sync>(&self) -> &Self {
        self.insert_resource(Events::<T>::new());
        // Register a type-erased update closure so that App::update() can
        // call Events::<T>::update() every tick. Without this, low-frequency
        // events (DropConnection, CreatePlayer, etc.) accumulate in
        // new_events forever — one entry per player join/leave cycle.
        if let Some(mut updaters) = self.world.get_resource_mut::<EventUpdaters>() {
            updaters.register::<T>();
        }
        self
    }

    /// Schedule label edits are only allowed during the plugin build phase (while no system is running).
    pub fn insert_schedule_before(
        &self,
        before: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) -> &Self {
        self.schedule
            .inner
            .try_write()
            .expect("cannot modify schedule ordering while schedules are running")
            .insert_before(before, schedule);
        self
    }

    pub fn insert_schedule_after(
        &self,
        after: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) -> &Self {
        self.schedule
            .inner
            .try_write()
            .expect("cannot modify schedule ordering while schedules are running")
            .insert_after(after, schedule);
        self
    }

    pub fn insert_startup_schedule_before(
        &self,
        before: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) -> &Self {
        self.schedule
            .inner
            .try_write()
            .expect("cannot modify schedule ordering while schedules are running")
            .insert_startup_before(before, schedule);
        self
    }

    pub fn insert_startup_schedule_after(
        &self,
        after: impl ScheduleLabel,
        schedule: impl ScheduleLabel,
    ) -> &Self {
        self.schedule
            .inner
            .try_write()
            .expect("cannot modify schedule ordering while schedules are running")
            .insert_startup_after(after, schedule);
        self
    }

    #[inline]
    pub fn plugins_state(&self) -> PluginsState {
        let overall_plugins_state = match self.plugins_state.read().clone() {
            PluginsState::Adding => {
                let mut state = PluginsState::Ready;
                let plugins = self.plugin_registry.read();
                for plugin in plugins.iter() {
                    // plugins installed to main need to see all sub-apps
                    if !plugin.ready(self) {
                        state = PluginsState::Adding;
                        break;
                    }
                }
                state
            }
            state => state,
        };

        overall_plugins_state
    }

    pub(crate) fn add_boxed_plugin(&self, plugin: Box<dyn Plugin>) -> Result<&Self, AppError> {
        debug!("added plugin: {}", plugin.name());
        if plugin.is_unique() && self.plugin_names.read().contains(plugin.name()) {
            Err(AppError::DuplicatePlugin {
                plugin_name: plugin.name().to_string(),
            })?;
        }

        // Reserve position in the plugin registry. If the plugin adds more plugins,
        // they'll all end up in insertion order.
        let index = self.plugin_registry.read().len();
        self.plugin_registry
            .write()
            .push(Box::new(PlaceholderPlugin));

        self.plugin_build_depth.fetch_add(1, Ordering::Relaxed);
        let result = catch_unwind(AssertUnwindSafe(|| plugin.build(self)));
        self.plugin_names.write().insert(plugin.name().to_string());
        self.plugin_build_depth.fetch_sub(1, Ordering::Relaxed);

        if let Err(payload) = result {
            resume_unwind(payload);
        }

        self.plugin_registry.write()[index] = plugin;
        Ok(self)
    }

    pub fn add_plugins<M>(&self, plugins: impl Plugins<M>) -> &Self {
        if matches!(
            self.plugins_state(),
            PluginsState::Cleaned | PluginsState::Finished
        ) {
            panic!(
                "Plugins cannot be added after App::cleanup() or App::finish() has been called."
            );
        }
        plugins.add_to_app(self);
        self
    }

    pub fn finish(&self) {
        for plugin in self.plugin_registry.write().iter_mut() {
            plugin.finish(self);
        }
        *self.plugins_state.write() = PluginsState::Finished;
    }
    pub fn cleanup(&self) {
        for plugin in self.plugin_registry.write().iter_mut() {
            plugin.cleanup(self);
        }
        *self.plugins_state.write() = PluginsState::Cleaned;
    }

    pub(crate) fn is_building_plugins(&self) -> bool {
        self.plugin_build_depth.load(Ordering::Relaxed) > 0
    }

    pub fn update(&self) {
        if self.is_building_plugins() {
            panic!("App::update() was called while a plugin was building.");
        }
        // Swap event buffers BEFORE running systems. This moves last tick's
        // new_events into old_events and drops events from two ticks ago.
        if let Some(updaters) = self.world.get_resource::<EventUpdaters>() {
            updaters.run_all(self.world());
        }

        // take/run/restore: the schedule lock is not held while systems run.
        // Systems registered mid-run land directly in the (emptied) slot or
        // in pending, and are merged on restore, effective next tick.
        self.schedule.apply_pending();
        let plan = self.schedule.inner.write().plan();
        for label in plan {
            let taken = self.schedule.inner.write().take_label(&label);
            let Some(mut systems) = taken else {
                continue;
            };
            systems.run(self.world());
            self.schedule.inner.write().restore_label(label, systems);
            self.schedule.apply_pending();
        }
    }

    pub fn should_exit(&self) -> Option<AppExit> {
        None
    }

    pub fn set_runner(&self, f: impl FnOnce(&App) -> AppExit + Send + 'static) -> &Self {
        *self.runner.lock() = Some(Box::new(f));
        self
    }

    pub fn run(&self) -> AppExit {
        self.runner.lock().take().unwrap()(self)
    }
}
