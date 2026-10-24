//! `LocalManager`：进程级共享的本地值存储。
//!
//! 本地值是与某个 id 绑定的类型擦除单例，常用于在系统之间共享跨帧可变的
//! 局部状态。
//!
//! `LocalManager`: the process-wide shared storage of local values.
//!
//! A local value is a type-erased singleton bound to an id, commonly used to
//! share mutable per-frame local state between systems.
use ahash::{HashMap, HashMapExt};
use parking_lot::Mutex;
use std::any::Any;
use std::sync::OnceLock;

/// 本地值 id：标识一个已注册的全局本地值。
///
/// Local id: identifies a registered global local value.
#[derive(Debug, Copy, Clone, Eq, Hash, PartialEq)]
pub struct LocalId {
    /// 自增序号：由管理器分配的唯一编号。
    ///
    /// The monotonic index: the unique number assigned by the manager.
    id: usize,
}

static LOCAL_MANAGER: OnceLock<Mutex<LocalManager>> = OnceLock::new();

/// 本地值管理器：以 id 为键存放类型擦除的全局本地值。
///
/// Local manager: stores type-erased global local values keyed by id.
pub struct LocalManager {
    /// 本地值表：按 id 存放类型擦除的值实例。
    ///
    /// Local value table: type-erased value instances keyed by id.
    locals: HashMap<LocalId, Box<dyn Any + Send + Sync>>,
    /// 下一个待分配的自增序号。
    ///
    /// The next monotonic index to hand out.
    local_index: usize,
}

impl LocalManager {
    /// 获取进程级共享的本地值管理器。
    ///
    /// Returns the process-wide shared local manager.
    pub fn global() -> &'static Mutex<LocalManager> {
        LOCAL_MANAGER.get_or_init(|| Mutex::new(LocalManager::new()))
    }

    /// 创建一个空的本地值管理器。
    ///
    /// Creates an empty local manager.
    pub fn new() -> LocalManager {
        Self {
            locals: HashMap::new(),
            local_index: 0,
        }
    }

    /// 按 id 获取本地值的不可变引用；不存在或类型不匹配时返回 `None`。
    ///
    /// Returns a shared reference to the local value for `id`; returns
    /// `None` if it is absent or of a different type.
    ///
    /// - `id`: 本地值 id
    pub fn get<T: Any>(&self, id: LocalId) -> Option<&T> {
        self.locals
            .get(&id)
            .and_then(|local| local.downcast_ref::<T>())
    }

    /// 按 id 获取本地值的可变引用；不存在或类型不匹配时返回 `None`。
    ///
    /// Returns a mutable reference to the local value for `id`; returns
    /// `None` if it is absent or of a different type.
    ///
    /// - `id`: 本地值 id
    pub fn get_mut<T: Any>(&mut self, id: LocalId) -> Option<&mut T> {
        self.locals
            .get_mut(&id)
            .and_then(|local| local.downcast_mut::<T>())
    }

    /// 存入一个本地值并返回其 id。
    ///
    /// Stores a local value and returns its id.
    ///
    /// - `local`: 要存入的本地值
    pub fn push<T: Any + Send + Sync>(&mut self, local: T) -> LocalId {
        let id = LocalId {
            id: self.local_index,
        };
        self.local_index += 1;
        self.locals.insert(id, Box::new(local));
        id
    }
}
