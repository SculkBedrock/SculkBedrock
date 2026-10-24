//! 插件 trait 与插件组：应用组装的扩展单元。
//!
//! 插件在 `build` 阶段向 [`App`] 注册系统与资源；`ready` 用于等待异步初始化，
//! `finish` 与 `cleanup` 分别在全部插件就绪后、以及调度执行前执行。
//!
//! The plugin trait and plugin groups: the extension units of application
//! assembly.
//!
//! Plugins register systems and resources with an [`App`] during `build`;
//! `ready` waits for asynchronous setup, while `finish` and `cleanup` run
//! after all plugins are ready and before schedule execution respectively.
use crate::app::App;

/// 插件 trait：应用组装单元，可注册系统/资源。
///
/// Plugin trait: the application assembly unit that registers systems
/// and resources.
pub trait Plugin: Send + Sync {
    /// 配置本插件被加入的 [`App`]。
    ///
    /// Configures the [`App`] to which this plugin is added.
    fn build(&self, app: &App);

    /// 插件是否就绪（默认就绪）；依赖异步初始化时重写。
    ///
    /// Whether the plugin is ready (ready by default); override when it
    /// depends on asynchronous setup.
    ///
    /// - `app`: 目标应用
    fn ready(&self, _app: &App) -> bool {
        true
    }

    /// 所有已注册插件就绪后，完成本插件的添加。
    ///
    /// Finish adding this plugin to the [`App`], once all plugins registered
    /// are ready.
    ///
    /// - `app`: 目标应用
    fn finish(&self, _app: &App) {}
    /// 在所有插件 build/finish 之后、调度执行之前运行。
    ///
    /// Runs after all plugins are built and finished, but before the app
    /// schedule is executed.
    ///
    /// - `app`: 目标应用
    fn cleanup(&self, _app: &App) {}

    /// 插件名称，用于唯一性检查与调试（默认为类型名）。
    ///
    /// Configures a name for the [`Plugin`] which is primarily used for checking plugin
    /// uniqueness and debugging.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// 插件是否可在同一个 [`App`] 中重复实例化；允许重复时重写为 `false`。
    ///
    /// If the plugin can be meaningfully instantiated several times in an [`App`],
    /// override this method to return `false`.
    fn is_unique(&self) -> bool {
        true
    }
}

/// 函数/闭包可直接作为插件（`build` 即调用自身）。
///
/// Functions/closures can act as plugins directly (`build` calls self).
impl<T: Fn(&App) + Send + Sync + 'static> Plugin for T {
    /// 把闭包本身作为 `build` 调用。
    ///
    /// Calls the closure itself as `build`.
    ///
    /// - `app`: 目标应用
    fn build(&self, app: &App) {
        self(app);
    }
}

/// 插件集合的聚合状态。
///
/// The aggregate state of a plugin set.
#[derive(PartialEq, Eq, Debug, Clone, Copy, PartialOrd, Ord)]
pub enum PluginsState {
    /// 插件正在添加中。
    ///
    /// Plugins are being added.
    Adding,
    /// 所有已添加插件均已就绪。
    ///
    /// All plugins already added are ready.
    Ready,
    /// 所有已添加插件的 finish 已执行。
    ///
    /// Finish has been executed for all plugins added.
    Finished,
    /// 所有已添加插件的 cleanup 已执行。
    ///
    /// Cleanup has been executed for all plugins added.
    Cleaned,
}

/// 占位插件：临时占据插件注册表中的条目。
///
/// A dummy plugin that's to temporarily occupy an entry in an app's plugin registry.
pub(crate) struct PlaceholderPlugin;

impl Plugin for PlaceholderPlugin {
    /// 空实现：不注册任何系统或资源。
    ///
    /// A no-op implementation that registers no systems or resources.
    fn build(&self, _app: &App) {}
}

/// 插件组 trait（对实现者封闭的公共门面）。
///
/// Plugins trait: the public facade of the sealed plugin-group trait.
pub trait Plugins<Marker>: sealed::Plugins<Marker> {}

impl<Marker, T> Plugins<Marker> for T where T: sealed::Plugins<Marker> {}

/// 封闭模块：插件组的具体实现（单个插件与插件元组）。
///
/// Sealed module: the concrete implementations of plugin groups (single
/// plugins and tuples of plugins).
mod sealed {
    use crate::app::plugin::Plugin;
    use crate::app::{App, AppError};
    use crate::impl_plugins_tuples;
    use sc_ecs_macros::all_tuples;

    /// 把自身（一个或一组插件）添加到 App。
    ///
    /// Adds self (one plugin or a group) to the app.
    ///
    /// - `self`: 要添加的插件或插件组
    /// - `app`: 目标应用
    pub trait Plugins<Marker> {
        fn add_to_app(self, app: &App);
    }

    /// 单个插件的 Marker。
    ///
    /// Marker for a single plugin.
    pub struct PluginMarker;

    /// 插件组的 Marker（预留）。
    ///
    /// Marker for plugin groups (reserved).
    #[allow(dead_code)]
    pub struct PluginGroupMarker;

    /// 插件元组的 Marker。
    ///
    /// Marker for tuples of plugins.
    pub struct PluginsTupleMarker;

    impl<P: Plugin + 'static> Plugins<PluginMarker> for P {
        /// 把单个插件加入应用；重复添加唯一插件时 panic。
        ///
        /// Adds a single plugin to the app; panics when a unique plugin is
        /// added twice.
        ///
        /// - `self`: 要添加的插件
        /// - `app`: 目标应用
        #[track_caller]
        fn add_to_app(self, app: &App) {
            if let Err(AppError::DuplicatePlugin { plugin_name }) =
                app.add_boxed_plugin(Box::new(self))
            {
                panic!(
                    "Error adding plugin {plugin_name}: : plugin was already added in application"
                )
            }
        }
    }

    all_tuples!(impl_plugins_tuples, 0, 20, P, S);
}
