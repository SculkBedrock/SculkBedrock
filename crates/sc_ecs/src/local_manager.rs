use ahash::{HashMap, HashMapExt};
use parking_lot::Mutex;
use std::any::Any;
use std::sync::OnceLock;

#[derive(Debug, Copy, Clone, Eq, Hash, PartialEq)]
pub struct LocalId {
    id: usize,
}

static LOCAL_MANAGER: OnceLock<Mutex<LocalManager>> = OnceLock::new();

pub struct LocalManager {
    locals: HashMap<LocalId, Box<dyn Any + Send + Sync>>,
    local_index: usize,
}

impl LocalManager {
    pub fn global() -> &'static Mutex<LocalManager> {
        LOCAL_MANAGER.get_or_init(|| Mutex::new(LocalManager::new()))
    }

    pub fn new() -> LocalManager {
        Self {
            locals: HashMap::new(),
            local_index: 0,
        }
    }

    pub fn get<T: Any>(&self, id: LocalId) -> Option<&T> {
        self.locals
            .get(&id)
            .and_then(|local| local.downcast_ref::<T>())
    }

    pub fn get_mut<T: Any>(&mut self, id: LocalId) -> Option<&mut T> {
        self.locals
            .get_mut(&id)
            .and_then(|local| local.downcast_mut::<T>())
    }

    pub fn push<T: Any + Send + Sync>(&mut self, local: T) -> LocalId {
        let id = LocalId {
            id: self.local_index,
        };
        self.local_index += 1;
        self.locals.insert(id, Box::new(local));
        id
    }
}
