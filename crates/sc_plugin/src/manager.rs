use crate::loader::SCPluginLoaderError;
use crate::manifest::{SCPluginDependency, SCPluginManifest};
use crate::SCPluginBoxer;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use sc_ecs::app::App;
use sc_ecs::resource::Resource;
use sc_log::{t, t_log};

use sc_error::{ScError, ScSite, capture_site};

#[derive(Debug, ScError)]
pub enum SCPluginDependenciesError {
    /// Required dependency version does not match.
    #[sc_error(code = "plugin.deps.version_mismatch")]
    VersionNotMatch {
        /// Plugin declaring the dependency.
        plugin: SCPluginDependency,
        /// Required dependency version.
        dependency: SCPluginDependency,
        /// Loaded dependency version.
        real: SCPluginDependency,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Required dependency is missing.
    #[sc_error(code = "plugin.deps.missing")]
    MissingDependency {
        /// Plugin declaring the dependency.
        plugin: SCPluginDependency,
        /// Missing dependency.
        dependency: SCPluginDependency,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Plugins depend on each other in a cycle.
    #[sc_error(code = "plugin.deps.circular")]
    CircularDependency {
        /// Plugin in the cycle.
        plugin: SCPluginDependency,
        /// Dependency stack of the cycle.
        stack: Vec<SCPluginDependency>,
        /// Dependency closing the cycle.
        dependency: SCPluginDependency,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// A dependency failed validation.
    #[sc_error(code = "plugin.deps.failed")]
    DependencyFailed {
        /// Plugin declaring the dependency.
        plugin: SCPluginDependency,
        /// Failed dependency.
        dependency: SCPluginDependency,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Plugin archive failed to load.
    #[sc_error(code = "plugin.deps.loader")]
    LoaderError {
        /// Plugin name.
        plugin: String,
        /// Underlying loader error.
        source: SCPluginLoaderError,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl SCPluginDependenciesError {
    /// 构造版本不匹配错误并捕获调用位置。
    ///
    /// Build a version mismatch error, capturing the call site.
    #[track_caller]
    pub fn version_not_match(
        plugin: SCPluginDependency,
        dependency: SCPluginDependency,
        real: SCPluginDependency,
    ) -> Self {
        let detail = format!(
            "plugin '{}' requires '{}' version '{}' but found '{}'",
            plugin.name, dependency.name, dependency.version, real.version
        );
        Self::VersionNotMatch {
            plugin,
            dependency,
            real,
            detail,
            site: capture_site(),
        }
    }

    /// 构造缺失依赖错误并捕获调用位置。
    ///
    /// Build a missing dependency error, capturing the call site.
    #[track_caller]
    pub fn missing_dependency(
        plugin: SCPluginDependency,
        dependency: SCPluginDependency,
    ) -> Self {
        let detail = format!(
            "plugin '{}' requires missing dependency '{}'",
            plugin.name, dependency.name
        );
        Self::MissingDependency {
            plugin,
            dependency,
            detail,
            site: capture_site(),
        }
    }

    /// 构造循环依赖错误并捕获调用位置。
    ///
    /// Build a circular dependency error, capturing the call site.
    #[track_caller]
    pub fn circular_dependency(
        plugin: SCPluginDependency,
        stack: Vec<SCPluginDependency>,
        dependency: SCPluginDependency,
    ) -> Self {
        let detail = format!(
            "plugin '{}' has a circular dependency on '{}'",
            plugin.name, dependency.name
        );
        Self::CircularDependency {
            plugin,
            stack,
            dependency,
            detail,
            site: capture_site(),
        }
    }

    /// 构造依赖失败错误并捕获调用位置。
    ///
    /// Build a failed dependency error, capturing the call site.
    #[track_caller]
    pub fn dependency_failed(
        plugin: SCPluginDependency,
        dependency: SCPluginDependency,
    ) -> Self {
        let detail = format!(
            "plugin '{}' depends on '{}' which failed validation",
            plugin.name, dependency.name
        );
        Self::DependencyFailed {
            plugin,
            dependency,
            detail,
            site: capture_site(),
        }
    }

    /// 构造加载器错误并捕获调用位置。
    ///
    /// Build a loader error, capturing the call site.
    #[track_caller]
    pub fn loader_error(plugin: String, source: SCPluginLoaderError) -> Self {
        let detail = format!("failed to load plugin '{plugin}': {}", source.detail());
        Self::LoaderError {
            plugin,
            source,
            detail,
            site: capture_site(),
        }
    }
}

impl Display for SCPluginDependenciesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionNotMatch {
                plugin,
                dependency,
                real,
                site,
                ..
            } => {
                write!(
                    f,
                    "{} (at {site})",
                    t!(
                        "console.plugin.load_error.version",
                        name = plugin.name,
                        version = dependency.version,
                        actual = real.version
                    )
                )
            }
            Self::MissingDependency {
                plugin,
                dependency,
                site,
                ..
            } => {
                write!(
                    f,
                    "{} (at {site})",
                    t!(
                        "console.plugin.load_error.depend",
                        name = plugin.name,
                        depend = dependency.name
                    )
                )
            }
            Self::CircularDependency {
                plugin,
                dependency,
                site,
                ..
            } => {
                write!(
                    f,
                    "{} (at {site})",
                    t!(
                        "console.plugin.load_error.circular",
                        name = plugin.name,
                        depend = dependency.name
                    )
                )
            }
            Self::DependencyFailed {
                plugin,
                dependency,
                site,
                ..
            } => {
                write!(
                    f,
                    "plugin {} depends on {}, but that dependency failed validation (at {site})",
                    plugin.name, dependency.name
                )
            }
            Self::LoaderError {
                plugin,
                source,
                site,
                ..
            } => {
                write!(
                    f,
                    "{} (at {site})",
                    t!(
                        "console.plugin.load_error.error",
                        name = plugin,
                        error = format!("{:?}", source)
                    )
                )
            }
        }
    }
}

impl std::error::Error for SCPluginDependenciesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LoaderError { source, .. } => Some(source),
            _ => None,
        }
    }
}


pub struct SCPluginDependenciesManager {
    dependencies: HashMap<SCPluginDependency, Vec<SCPluginDependency>>,
    loaded_plugins: HashMap<String, SCPluginDependency>,
}

impl SCPluginDependenciesManager {
    pub fn new() -> Self {
        Self {
            dependencies: HashMap::new(),
            loaded_plugins: HashMap::new(),
        }
    }

    pub fn add_plugin(&mut self, manifest: SCPluginManifest) {
        let plugin = SCPluginDependency {
            name: manifest.name,
            version: manifest.version,
        };
        self.dependencies
            .insert(plugin.clone(), manifest.dependencies.clone());
        self.loaded_plugins.insert(plugin.name.clone(), plugin);
    }

    pub fn check(&self) -> (Vec<String>, Vec<SCPluginDependenciesError>) {
        let mut errors = Vec::new();
        let mut invalid = HashSet::new();
        let mut plugin_names: Vec<String> = self
            .dependencies
            .keys()
            .map(|plugin| plugin.name.clone())
            .collect();
        plugin_names.sort();

        let dependencies_by_name: HashMap<String, Vec<SCPluginDependency>> = self
            .dependencies
            .iter()
            .map(|(plugin, dependencies)| (plugin.name.clone(), dependencies.clone()))
            .collect();

        for name in &plugin_names {
            let Some(plugin) = self.loaded_plugins.get(name) else {
                invalid.insert(name.clone());
                continue;
            };
            let Some(dependencies) = dependencies_by_name.get(name) else {
                continue;
            };

            for dependency in dependencies {
                match self.loaded_plugins.get(&dependency.name) {
                    Some(real) if real.version == dependency.version => {}
                    Some(real) => {
                        invalid.insert(name.clone());
                        errors.push(SCPluginDependenciesError::version_not_match(
                            plugin.clone(),
                            dependency.clone(),
                            real.clone(),
                        ));
                    }
                    None => {
                        invalid.insert(name.clone());
                        errors.push(SCPluginDependenciesError::missing_dependency(
                            plugin.clone(),
                            dependency.clone(),
                        ));
                    }
                }
            }
        }

        loop {
            let mut changed = false;
            for name in &plugin_names {
                if invalid.contains(name) {
                    continue;
                }
                let Some(plugin) = self.loaded_plugins.get(name) else {
                    continue;
                };
                let Some(dependencies) = dependencies_by_name.get(name) else {
                    continue;
                };
                if let Some(dependency) = dependencies
                    .iter()
                    .find(|dependency| invalid.contains(&dependency.name))
                {
                    invalid.insert(name.clone());
                    errors.push(SCPluginDependenciesError::dependency_failed(
                        plugin.clone(),
                        dependency.clone(),
                    ));
                    changed = true;
                }
            }

            if !changed {
                break;
            }
        }

        let mut remaining: HashMap<String, HashSet<String>> = plugin_names
            .iter()
            .filter(|name| !invalid.contains(*name))
            .map(|name| {
                let dependencies = dependencies_by_name
                    .get(name)
                    .map(|dependencies| {
                        dependencies
                            .iter()
                            .map(|dependency| dependency.name.clone())
                            .collect::<HashSet<_>>()
                    })
                    .unwrap_or_default();
                (name.clone(), dependencies)
            })
            .collect();

        let mut finish_load_plugin = Vec::new();
        while !remaining.is_empty() {
            let mut ready: Vec<String> = remaining
                .iter()
                .filter(|(_, dependencies)| dependencies.is_empty())
                .map(|(name, _)| name.clone())
                .collect();
            ready.sort();

            if ready.is_empty() {
                let mut cyclic_names: Vec<String> = remaining.keys().cloned().collect();
                cyclic_names.sort();
                for name in cyclic_names {
                    if let Some(plugin) = self.loaded_plugins.get(&name) {
                        let stack = dependencies_by_name.get(&name).cloned().unwrap_or_default();
                        let dependency = stack.first().cloned().unwrap_or_else(|| plugin.clone());
                        errors.push(SCPluginDependenciesError::circular_dependency(
                            plugin.clone(),
                            stack,
                            dependency,
                        ));
                    }
                }
                break;
            }

            for name in ready {
                if remaining.remove(&name).is_none() {
                    continue;
                }
                finish_load_plugin.push(name.clone());
                for dependencies in remaining.values_mut() {
                    dependencies.remove(&name);
                }
            }
        }

        (finish_load_plugin, errors)
    }

    pub fn retain_plugins(&mut self, names: &HashSet<String>) {
        self.dependencies
            .retain(|plugin, _| names.contains(&plugin.name));
        self.loaded_plugins.retain(|name, _| names.contains(name));
    }
}

#[derive(Resource)]
pub struct SCPluginManager {
    plugins: HashMap<String, SCPluginBoxer>,
    dependencies: SCPluginDependenciesManager,
    enabled: HashSet<String>,
    enable_order: Vec<String>,
    pub(crate) app: App,
}

impl SCPluginManager {
    pub fn new(app: App) -> Self {
        Self {
            plugins: HashMap::new(),
            dependencies: SCPluginDependenciesManager::new(),
            enabled: HashSet::new(),
            enable_order: Vec::new(),
            app,
        }
    }

    pub fn app(&self) -> App {
        self.app.clone()
    }

    pub fn add_plugin_and_enable(
        &mut self,
        plugin: SCPluginBoxer,
    ) -> Vec<SCPluginDependenciesError> {
        self.add_plugin(plugin);
        let errors = self.check_dependencies();
        if errors.is_empty() {
            self.enable_checked_plugins();
        }
        errors
    }

    pub fn add_plugin(&mut self, plugin: SCPluginBoxer) {
        let manifest = plugin.manifest.clone();
        if self.plugins.contains_key(&manifest.name) {
            log::warn!(
                "{}",
                t_log!("console.plugin.duplicate", name = manifest.name)
            );
            return;
        }
        self.dependencies.add_plugin(manifest.clone());
        self.plugins.insert(manifest.name, plugin);
    }

    pub fn check_dependencies(&mut self) -> Vec<SCPluginDependenciesError> {
        let (finish_load, errors) = self.dependencies.check();
        let finish_set: HashSet<String> = finish_load.iter().cloned().collect();
        let rejected: Vec<String> = self
            .plugins
            .keys()
            .filter(|name| !finish_set.contains(*name))
            .cloned()
            .collect();
        for name in rejected {
            if self.enabled.remove(&name) {
                if let Some(plugin) = self.plugins.get(&name) {
                    plugin.disable();
                }
            }
        }
        self.plugins.retain(|name, _| finish_set.contains(name));
        self.dependencies.retain_plugins(&finish_set);
        self.enable_order = finish_load;
        errors
    }

    pub fn enable_checked_plugins(&mut self) -> Vec<(String, String)> {
        let names = if self.enable_order.is_empty() {
            let mut names: Vec<String> = self.plugins.keys().cloned().collect();
            names.sort();
            names
        } else {
            self.enable_order.clone()
        };

        let mut enabled = Vec::new();
        for name in names {
            if self.enabled.contains(&name) {
                continue;
            }
            let Some(plugin) = self.plugins.get(&name) else {
                continue;
            };
            plugin.enable();
            self.enabled.insert(name.clone());
            enabled.push((name, plugin.manifest.version.clone()));
        }
        enabled
    }

    pub fn get_plugins(&self) -> Vec<&SCPluginBoxer> {
        let names = if self.enable_order.is_empty() {
            let mut names: Vec<String> = self.plugins.keys().cloned().collect();
            names.sort();
            names
        } else {
            self.enable_order.clone()
        };

        names
            .iter()
            .filter_map(|name| self.plugins.get(name))
            .collect()
    }
}

impl Drop for SCPluginManager {
    fn drop(&mut self) {
        // Disable in reverse dependency order while the dynamic library and
        // plugin object are still alive.
        for name in self.enable_order.iter().rev() {
            if self.enabled.remove(name) {
                if let Some(plugin) = self.plugins.get(name) {
                    plugin.disable();
                }
            }
        }
        self.plugins.clear();
    }
}
