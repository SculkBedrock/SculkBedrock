//! `App`：应用组装与调度执行：插件注册、调度表管理、主循环 runner。
//!
//! `App`: application assembly and schedule execution: plugin registration,
//! schedule table management, main-loop runner.
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

/// 执行一轮默认 runner：finish -> cleanup -> update。
///
/// Runs one round of the default runner: finish -> cleanup -> update.
///
/// - `app`: 目标应用
fn run_once(app: &App) -> AppExit {
    app.finish();
    app.cleanup();
    app.update();
    app.should_exit().unwrap_or(AppExit::Success)
}

/// 应用退出码：成功或携带退出码的错误。
///
/// App exit code: success, or an error carrying an exit code.
#[derive(Event, Debug, Clone, Default, PartialEq, Eq)]
pub enum AppExit {
    /// [`App`] 无任何问题地退出。
    ///
    /// [`App`] exited without any problems.
    #[default]
    Success,
    /// [`App`] 遇到无法处理的错误；持有期望的退出码。
    ///
    /// The [`App`] experienced an unhandleable error.
    /// Holds the exit code we expect our app to return.
    Error(NonZeroU8),
}

impl AppExit {
    /// 创建退出码为 1 的 [`AppExit::Error`]。
    ///
    /// Creates a [`AppExit::Error`] with a error code of 1.
    #[must_use]
    pub const fn error() -> Self {
        Self::Error(NonZeroU8::MIN)
    }

    /// 是否为 [`AppExit::Success`]，是则返回 `true`。
    ///
    /// Returns `true` if `self` is a [`AppExit::Success`].
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, AppExit::Success)
    }

    /// 是否为 [`AppExit::Error`]，是则返回 `true`。
    ///
    /// Returns `true` if `self` is a [`AppExit::Error`].
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, AppExit::Error(_))
    }

    /// 由退出码创建 [`AppExit`]：0 对应 [`AppExit::Success`]，否则为
    /// [`AppExit::Error`]。
    ///
    /// Creates an [`AppExit`] from a code: `0` constructs
    /// [`AppExit::Success`], anything else constructs [`AppExit::Error`].
    ///
    /// - `code`: 进程退出码 / the process exit code
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match NonZeroU8::new(code) {
            Some(code) => Self::Error(code),
            None => Self::Success,
        }
    }
}

impl From<u8> for AppExit {
    /// 由退出码创建 `AppExit`。
    ///
    /// Creates an `AppExit` from an exit code.
    fn from(value: u8) -> Self {
        Self::from_code(value)
    }
}

impl Termination for AppExit {
    /// 转换为进程退出码。
    ///
    /// Converts into a process exit code.
    fn report(self) -> ExitCode {
        match self {
            AppExit::Success => ExitCode::SUCCESS,
            AppExit::Error(value) => ExitCode::from(value.get()),
        }
    }
}

/// 应用层错误。
///
/// Application-level errors.
#[derive(Debug)]
pub(crate) enum AppError {
    /// 重复添加了唯一插件。
    ///
    /// A unique plugin was added more than once.
    DuplicatePlugin {
        /// 重复添加的插件名。
        ///
        /// The name of the duplicated plugin.
        plugin_name: String,
    },
}

impl std::fmt::Display for AppError {
    /// 以调试格式输出错误。
    ///
    /// Formats the error in debug format.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for AppError {}

/// 调度句柄：写锁被占用时的注册进入 `pending` 队列，在下一个同步点合并。
///
/// Schedule handle: registrations that collide with the write lock go to the
/// `pending` queue and merge at the next sync point.
pub struct ScheduleHandle {
    /// 调度管理器本体。
    ///
    /// The schedule manager itself.
    inner: RwLock<ScheduleManager>,
    /// 与写锁冲突时暂存待注册系统。
    ///
    /// Buffers registrations that collide with the write lock.
    pending: Mutex<Vec<(Interned<dyn ScheduleLabel>, Box<dyn System>)>>,
}

impl ScheduleHandle {
    /// 创建一个空的调度句柄。
    ///
    /// Creates an empty schedule handle.
    fn new() -> Self {
        Self {
            inner: RwLock::new(ScheduleManager::new()),
            pending: Mutex::new(Vec::new()),
        }
    }

    /// 注册一个盒装系统；写锁被占用时转入 pending 队列。
    ///
    /// Registers a boxed system; defers to the pending queue when the
    /// write lock is busy.
    ///
    /// - `label`: 目标调度标签
    /// - `system`: 要注册的系统
    fn add_system_boxed(&self, label: Interned<dyn ScheduleLabel>, system: Box<dyn System>) {
        match self.inner.try_write() {
            Some(mut inner) => inner.add_boxed(label, system),
            None => self.pending.lock().push((label, system)),
        }
    }

    /// 将 pending 队列中的系统合并进调度表。
    ///
    /// Merges the pending systems into the schedule table.
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

/// 应用：插件组装 + 世界 + 调度 + 主循环入口，可克隆共享。
///
/// App: plugin assembly + world + schedule + main-loop entry, cloneable
/// and shareable.
#[derive(Clone)]
pub struct App {
    /// 应用的世界。
    ///
    /// The world of the app.
    world: World,
    /// 调度句柄（共享）。
    ///
    /// The schedule handle (shared).
    schedule: Arc<ScheduleHandle>,
    /// 插件注册表。
    ///
    /// The plugin registry.
    pub(crate) plugin_registry: Arc<RwLock<Vec<Box<dyn Plugin>>>>,
    /// 已注册插件名集合（唯一性检查）。
    ///
    /// Set of registered plugin names (uniqueness check).
    pub(crate) plugin_names: Arc<RwLock<HashSet<String>>>,
    /// 插件 build 递归深度（>0 表示正在构建插件）。
    ///
    /// Plugin build recursion depth (>0 means plugins are building).
    pub(crate) plugin_build_depth: Arc<AtomicUsize>,
    /// 插件集合状态。
    ///
    /// The plugin-set state.
    pub(crate) plugins_state: Arc<RwLock<PluginsState>>,
    /// 主循环 runner。
    ///
    /// The main-loop runner.
    pub(crate) runner: Arc<Mutex<Option<Box<dyn FnOnce(&App) -> AppExit + Send>>>>,
}

impl App {
    /// 创建一个新应用（含默认事件更新器资源与默认 runner）。
    ///
    /// Creates a new app (with the default event-updater resource and
    /// the default runner).
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

    /// 获取应用的 World 引用。
    ///
    /// Returns a reference to the app's World.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// 向指定调度注册系统，返回 `self` 以便链式调用。
    ///
    /// Registers systems into the given schedule and returns `self` for
    /// chaining.
    ///
    /// - `schedule`: 目标调度标签
    /// - `systems`: 要注册的系统源
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

    /// 插入一个资源，返回 `self` 以便链式调用。
    ///
    /// Inserts a resource and returns `self` for chaining.
    ///
    /// - `resource`: 要插入的资源实例
    pub fn insert_resource<R: Resource + 'static + Send + Sync>(&self, resource: R) -> &Self {
        self.world.insert_resource(resource);
        self
    }

    /// 移除某个类型的资源，返回 `self` 以便链式调用。
    ///
    /// Removes the resource of a type and returns `self` for chaining.
    ///
    /// - `R`: 要移除的资源类型
    pub fn remove_resource<R: Resource + 'static + Send + Sync>(&self) -> &Self {
        self.world.remove_resource::<R>();
        self
    }

    /// 注册一个事件类型（创建事件流并挂上每 tick 的更新闭包），
    /// 返回 `self` 以便链式调用。
    ///
    /// Registers an event type (creating the event stream and hooking
    /// the per-tick update closure) and returns `self` for chaining.
    ///
    /// - `T`: 要注册的事件类型
    pub fn add_event<T: Event + 'static + Send + Sync>(&self) -> &Self {
        self.insert_resource(Events::<T>::new());
        // Register a per-tick update closure so App::update() swaps this
        // event type's buffers every tick; without it events would
        // accumulate in new_events forever.
        if let Some(mut updaters) = self.world.get_resource_mut::<EventUpdaters>() {
            updaters.register::<T>();
        }
        self
    }

    /// 调度顺序编辑仅允许在插件构建阶段进行（无系统运行时）。
    ///
    /// Schedule label edits are only allowed during the plugin build phase (while no system is running).
    ///
    /// - `before`: 参照调度标签
    /// - `schedule`: 要插入的调度标签；调度运行期间调用会 panic
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

    /// 在指定调度之后插入新调度，返回 `self` 以便链式调用。
    ///
    /// Inserts a schedule after the given one and returns `self` for
    /// chaining.
    ///
    /// - `after`: 参照调度标签
    /// - `schedule`: 要插入的调度标签；调度运行期间调用会 panic
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

    /// 在指定启动调度之前插入新调度，返回 `self` 以便链式调用。
    ///
    /// Inserts a startup schedule before the given one and returns `self`
    /// for chaining.
    ///
    /// - `before`: 参照启动调度标签
    /// - `schedule`: 要插入的调度标签；调度运行期间调用会 panic
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

    /// 在指定启动调度之后插入新调度，返回 `self` 以便链式调用。
    ///
    /// Inserts a startup schedule after the given one and returns `self`
    /// for chaining.
    ///
    /// - `after`: 参照启动调度标签
    /// - `schedule`: 要插入的调度标签；调度运行期间调用会 panic
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

    /// 查询插件集合整体状态（`Adding` 时会逐插件调用 `ready`）。
    ///
    /// Queries the overall plugin-set state (polls `ready` on each plugin
    /// while `Adding`).
    #[inline]
    pub fn plugins_state(&self) -> PluginsState {
        let overall_plugins_state = match *self.plugins_state.read() {
            PluginsState::Adding => {
                let mut state = PluginsState::Ready;
                let plugins = self.plugin_registry.read();
                for plugin in plugins.iter() {
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

    /// 注册一个已盒装的插件；唯一插件重复添加时返回
    /// [`AppError::DuplicatePlugin`]。
    ///
    /// Registers an already-boxed plugin; returns
    /// [`AppError::DuplicatePlugin`] when a unique plugin is added twice.
    ///
    /// - `plugin`: 要注册的插件 / the plugin to register
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

    /// 添加一个（或一组）插件，返回 `self` 以便链式调用；
    /// cleanup/finish 之后添加会 panic。
    ///
    /// Adds a plugin (or a group of plugins) and returns `self` for
    /// chaining; panics when called after cleanup/finish.
    ///
    /// - `plugins`: 要添加的插件或插件组
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

    /// 对所有插件执行 finish。
    ///
    /// Runs `finish` on all plugins.
    pub fn finish(&self) {
        for plugin in self.plugin_registry.write().iter_mut() {
            plugin.finish(self);
        }
        *self.plugins_state.write() = PluginsState::Finished;
    }

    /// 对所有插件执行 cleanup。
    ///
    /// Runs `cleanup` on all plugins.
    pub fn cleanup(&self) {
        for plugin in self.plugin_registry.write().iter_mut() {
            plugin.cleanup(self);
        }
        *self.plugins_state.write() = PluginsState::Cleaned;
    }

    /// 是否正在构建插件（插件 build 递归深度 > 0）。
    ///
    /// Whether plugins are currently building (build recursion depth > 0).
    pub(crate) fn is_building_plugins(&self) -> bool {
        self.plugin_build_depth.load(Ordering::Relaxed) > 0
    }

    /// 执行一个 tick：先交换事件缓冲，再按调度计划运行全部系统。
    ///
    /// Runs one tick: swaps event buffers first, then runs all systems
    /// according to the schedule plan.
    pub fn update(&self) {
        if self.is_building_plugins() {
            panic!("App::update() was called while a plugin was building.");
        }
        // Swap event buffers before running systems (last tick's
        // new_events move to old_events; events from two ticks ago drop).
        if let Some(updaters) = self.world.get_resource::<EventUpdaters>() {
            updaters.run_all(self.world());
        }

        // Schedule lock is not held while systems run; systems registered
        // mid-run are merged on restore, effective next tick.
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

    /// 是否应退出；默认永不退出（由 runner 决定）。
    ///
    /// Whether the app should exit; never exits by default (the runner
    /// decides).
    pub fn should_exit(&self) -> Option<AppExit> {
        None
    }

    /// 设置主循环 runner，返回 `self` 以便链式调用。
    ///
    /// Sets the main-loop runner and returns `self` for chaining.
    ///
    /// - `f`: runner 函数（只运行一次，`run` 时取出）
    pub fn set_runner(&self, f: impl FnOnce(&App) -> AppExit + Send + 'static) -> &Self {
        *self.runner.lock() = Some(Box::new(f));
        self
    }

    /// 运行应用直至退出，返回退出码。
    ///
    /// Runs the app until exit and returns the exit code.
    pub fn run(&self) -> AppExit {
        self.runner.lock().take().unwrap()(self)
    }
}
