//! `AppManager`：以标签为键统一管理多个 `App`。
//!
//! `AppManager`: manages multiple `App` instances keyed by label.
use crate::app::App;
use crate::define_label;
use crate::intern::Interned;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
pub use sc_ecs_macros::AppLabel;
use sc_ecs_macros::Resource;

define_label!(AppLabel, APP_LABEL_INTERNER);

/// App 管理器：以标签为键持有多个 `App`，支持统一构建与启动。
///
/// App manager: holds multiple `App` instances keyed by label, with
/// batch construction and startup.
#[derive(Resource, Clone)]
pub struct AppManager {
    /// 标签到 App 的映射。
    ///
    /// Map from interned label to shared app instance.
    apps: Arc<RwLock<HashMap<Interned<dyn AppLabel>, Arc<App>>>>,
}

impl AppManager {
    /// 创建一个空的 App 管理器。
    ///
    /// Creates an empty app manager.
    pub fn new() -> AppManager {
        AppManager {
            apps: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 插入一个 App，返回 `self` 以便链式调用。
    ///
    /// Inserts an app and returns `self` for chaining.
    ///
    /// - `key`: App 的标签键
    /// - `app`: 要插入的 App 实例
    pub fn insert(&self, key: impl AppLabel, app: App) -> &Self {
        self.apps.write().insert(key.intern(), Arc::new(app));
        self
    }

    /// 按标签获取 App 的共享引用，不存在时返回 `None`。
    ///
    /// Looks up an app by label, returning `None` if absent.
    ///
    /// - `key`: 已内部化的 App 标签
    pub fn get(&self, key: &Interned<dyn AppLabel>) -> Option<Arc<App>> {
        self.apps.read().get(key).cloned()
    }

    /// 对所有已注册的 App 执行构建（插入 `AppManager` 自身作为资源），
    /// 返回 `self` 以便链式调用。
    ///
    /// Builds all registered apps (inserting this `AppManager` itself as a
    /// resource) and returns `self` for chaining.
    pub fn build(&self) -> &Self {
        for app in self.apps.read().values() {
            app.insert_resource(self.clone());
        }
        self
    }

    /// 启动所有已注册的 App。
    ///
    /// Runs all registered apps.
    pub fn run(&self) {
        for app in self.apps.read().values() {
            app.run();
        }
    }
}
