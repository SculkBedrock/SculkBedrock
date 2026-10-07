pub mod loader;
pub mod log_bridge;
pub mod manager;
pub mod manifest;

use crate::manifest::SCPluginManifest;
use std::path::PathBuf;
use std::sync::Arc;
use sc_ecs::app::App;
pub use sc_plugin_macros::SCPlugin;

pub trait SCPlugin: Sync + Send {
    fn new() -> Self
    where
        Self: Sized;
    fn enable(&self, app: &App);

    fn disable(&self, app: &App);
    fn hot_enable(&self, app: &App) {
        self.enable(app);
    }
    fn hot_disable(&self, app: &App) {
        self.disable(app);
    }
}

pub struct SCPluginBoxer {
    plugin: Option<Box<dyn SCPlugin>>,
    app: App,
    pub manifest: SCPluginManifest,
    // Temporary design: Rust trait objects cross the DLL boundary. Keep the
    // module loaded until the trait object has been destroyed.
    _library: Option<Arc<libloading::Library>>,
    _library_path: Option<PathBuf>,
}

impl SCPluginBoxer {
    pub fn new(plugin: Box<dyn SCPlugin>, app: App, manifest: SCPluginManifest) -> Self {
        Self {
            plugin: Some(plugin),
            app,
            manifest,
            _library: None,
            _library_path: None,
        }
    }

    pub(crate) fn new_dynamic(
        plugin: Box<dyn SCPlugin>,
        app: App,
        manifest: SCPluginManifest,
        library: Arc<libloading::Library>,
        library_path: PathBuf,
    ) -> Self {
        Self {
            plugin: Some(plugin),
            app,
            manifest,
            _library: Some(library),
            _library_path: Some(library_path),
        }
    }

    pub fn enable(&self) {
        if let Some(plugin) = self.plugin.as_ref() {
            plugin.enable(&self.app);
        }
    }

    pub fn disable(&self) {
        if let Some(plugin) = self.plugin.as_ref() {
            plugin.disable(&self.app);
        }
    }

    pub fn hot_enable(&self) {
        if let Some(plugin) = self.plugin.as_ref() {
            plugin.hot_enable(&self.app);
        }
    }

    pub fn hot_disable(&self) {
        if let Some(plugin) = self.plugin.as_ref() {
            plugin.hot_disable(&self.app);
        }
    }
}

impl Drop for SCPluginBoxer {
    fn drop(&mut self) {
        // Drop the trait object before releasing the DLL. This is required
        // because its destructor and vtable live in the dynamically loaded
        // module.
        drop(self.plugin.take());
        drop(self._library.take());
        if let Some(path) = self._library_path.take() {
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log::debug!(
                        "plugin-loader >> failed to remove temporary library {}: {error}",
                        path.display()
                    );
                }
            }
        }
    }
}
