use crate::app::App;

/// `Send + Sync` bound: the plugin registry sits behind a real `RwLock`, and
/// `App` must stay `Sync` (it is stored in resources and cloned across threads).
pub trait Plugin: Send + Sync {
    /// Configures the [`App`] to which this plugin is added.
    fn build(&self, app: &App);
    fn ready(&self, _app: &App) -> bool {
        true
    }

    /// Finish adding this plugin to the [`App`], once all plugins registered are ready. This can
    /// be useful for plugins that depends on another plugin asynchronous setup, like the renderer.
    fn finish(&self, _app: &App) {}

    /// Runs after all plugins are built and finished, but before the app schedule is executed.
    /// This can be useful if you have some resource that other plugins need during their build step,
    /// but after build you want to remove it and send it to another thread.
    fn cleanup(&self, _app: &App) {}

    /// Configures a name for the [`Plugin`] which is primarily used for checking plugin
    /// uniqueness and debugging.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// If the plugin can be meaningfully instantiated several times in an [`App`],
    /// override this method to return `false`.
    fn is_unique(&self) -> bool {
        true
    }
}

impl<T: Fn(&App) + Send + Sync + 'static> Plugin for T {
    fn build(&self, app: &App) {
        self(app);
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy, PartialOrd, Ord)]
pub enum PluginsState {
    /// Plugins are being added.
    Adding,
    /// All plugins already added are ready.
    Ready,
    /// Finish has been executed for all plugins added.
    Finished,
    /// Cleanup has been executed for all plugins added.
    Cleaned,
}

/// A dummy plugin that's to temporarily occupy an entry in an app's plugin registry.
pub(crate) struct PlaceholderPlugin;

impl Plugin for PlaceholderPlugin {
    fn build(&self, _app: &App) {}
}

pub trait Plugins<Marker>: sealed::Plugins<Marker> {}

impl<Marker, T> Plugins<Marker> for T where T: sealed::Plugins<Marker> {}

mod sealed {
    use crate::app::plugin::Plugin;
    use crate::app::{App, AppError};
    use sc_ecs_macros::all_tuples;

    pub trait Plugins<Marker> {
        fn add_to_app(self, app: &App);
    }

    pub struct PluginMarker;
    #[allow(dead_code)]
    pub struct PluginGroupMarker;
    pub struct PluginsTupleMarker;

    impl<P: Plugin + 'static> Plugins<PluginMarker> for P {
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

    macro_rules! impl_plugins_tuples {
        ($(($param: ident, $plugins: ident)),*) => {
            impl<$($param, $plugins),*> Plugins<(PluginsTupleMarker, $($param,)*)> for ($($plugins,)*)
            where
                $($plugins: Plugins<$param>),*
            {
                #[allow(non_snake_case, unused_variables)]
                #[track_caller]
                fn add_to_app(self, app: &App) {
                    let ($($plugins,)*) = self;
                    $($plugins.add_to_app(app);)*
                }
            }
        }
    }

    all_tuples!(impl_plugins_tuples, 0, 20, P, S);
}
