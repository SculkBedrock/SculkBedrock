use crate::app::App;
use crate::define_label;
use crate::intern::Interned;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
pub use sc_ecs_macros::AppLabel;
use sc_ecs_macros::Resource;

define_label!(AppLabel, APP_LABEL_INTERNER);

#[derive(Resource, Clone)]
pub struct AppManager {
    apps: Arc<RwLock<HashMap<Interned<dyn AppLabel>, Arc<App>>>>,
}

impl AppManager {
    pub fn new() -> AppManager {
        AppManager {
            apps: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn insert(&self, key: impl AppLabel, app: App) -> &Self {
        self.apps.write().insert(key.intern(), Arc::new(app));
        self
    }

    pub fn get(&self, key: &Interned<dyn AppLabel>) -> Option<Arc<App>> {
        self.apps.read().get(key).cloned()
    }

    pub fn build(&self) -> &Self {
        for app in self.apps.read().values() {
            app.insert_resource(self.clone());
        }
        self
    }

    pub fn run(&self) {
        for app in self.apps.read().values() {
            app.run();
        }
    }
}
