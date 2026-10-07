use crate::loader::SCPluginLoaderError;
use crate::manifest::{SCPluginDependency, SCPluginManifest};
use crate::SCPluginBoxer;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use sc_ecs::app::App;
use sc_ecs::resource::Resource;
use sc_log::{t, t_log};

pub enum SCPluginDependenciesError {
    VersionNotMatch(SCPluginDependency, SCPluginDependency, SCPluginDependency),
    MissingDependency(SCPluginDependency, SCPluginDependency),
    CircularDependency(
        SCPluginDependency,
        Vec<SCPluginDependency>,
        SCPluginDependency,
    ),
    DependencyFailed(SCPluginDependency, SCPluginDependency),
    LoaderError(String, SCPluginLoaderError),
}

impl Display for SCPluginDependenciesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SCPluginDependenciesError::VersionNotMatch(plugin, dependency, real) => {
                write!(
                    f,
                    "{}",
                    t!(
                        "console.plugin.load_error.version",
                        name = plugin.name,
                        version = dependency.version,
                        actual = real.version
                    )
                )
            }
            SCPluginDependenciesError::MissingDependency(plugin, dependency) => {
                write!(
                    f,
                    "{}",
                    t!(
                        "console.plugin.load_error.depend",
                        name = plugin.name,
                        depend = dependency.name
                    )
                )
            }
            SCPluginDependenciesError::CircularDependency(plugin, _, dependency) => {
                write!(
                    f,
                    "{}",
                    t!(
                        "console.plugin.load_error.circular",
                        name = plugin.name,
                        depend = dependency.name
                    )
                )
            }
            SCPluginDependenciesError::DependencyFailed(plugin, dependency) => {
                write!(
                    f,
                    "plugin {} depends on {}, but that dependency failed validation",
                    plugin.name, dependency.name
                )
            }
            SCPluginDependenciesError::LoaderError(plugin, e) => {
                write!(
                    f,
                    "{}",
                    t!(
                        "console.plugin.load_error.error",
                        name = plugin,
                        error = format!("{:?}", e)
                    )
                )
            }
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
                        errors.push(SCPluginDependenciesError::VersionNotMatch(
                            plugin.clone(),
                            dependency.clone(),
                            real.clone(),
                        ));
                    }
                    None => {
                        invalid.insert(name.clone());
                        errors.push(SCPluginDependenciesError::MissingDependency(
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
                    errors.push(SCPluginDependenciesError::DependencyFailed(
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
                        errors.push(SCPluginDependenciesError::CircularDependency(
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
