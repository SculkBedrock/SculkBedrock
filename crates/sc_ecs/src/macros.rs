#[macro_export]
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

#[macro_export]
macro_rules! impl_system_function {
    ($(($P:ident, $p:ident)),*) => {
        impl<F, $($P: SystemParam),*> SystemFunction<fn($($P),*)> for F
        where
            F: FnMut($($P),*) + FnMut($($P::This<'_>),*) + 'static + Send + Sync,
        {
            type Param = ($($P,)*);

            fn run(
                &mut self,
                ($($p,)*): <Self::Param as SystemParam>::This<'_>
            ) {
                (self)($($p),*)
            }
        }
    }
}

#[macro_export]
macro_rules! impl_tuple_system_function {
    ($(($P:ident, $p:ident, $M:ident)),*) => {
        impl <$($P),*, $($M: 'static + Send + Sync),*> IntoSystem<($($P),*), ($($M),*)> for ($($P),*)
        where
            $($P: SystemFunction<$M>),*
        {
            type System = TupleFunctionSystem;

            fn into_system(self) -> Self::System {
                let ($($p),*) = self;
                TupleFunctionSystem::new(vec![$(Box::new($p.into_system())),*])
            }
        }
    }
}

#[macro_export]
macro_rules! impl_system_param_tuple {
    ($(($P:ident, $p:ident)),*) => {
        #[allow(unused_variables, clippy::unused_unit)]
        unsafe impl<$($P: SystemParam),*> SystemParam for ($($P,)*) {
            type State = ($($P::State,)*);
            type This<'a> = ($($P::This<'a>,)*);
        }
    }
}

#[macro_export]
macro_rules! impl_system_param_state_tuple {
    ($(($P:ident, $p:ident)),*) => {
        #[allow(unused_variables, non_snake_case, clippy::unused_unit)]
        impl<$($P),*> SystemParamState for ($($P,)*)
        where
            $($P: SystemParamState,)*
        {
            type Item = ($($P::Item,)*);

            fn init() -> Self{
                ($($P::init(),)*)
            }

            fn get_param<'a>(
                state: &'a mut Self,
                world: &'a World
            ) -> Option<<<Self as SystemParamState>::Item as SystemParam>::This<'a>> {
                let ($($P,)*) = state;
                Some(($($P::get_param($P,world)?,)*))
            }
        }
    }
}

#[macro_export]
macro_rules! impl_bundle {
    ($(($P:ident, $p:ident)),*) => {
        #[allow(unused_variables, non_snake_case, clippy::unused_unit)]
        impl<$($P: 'static + Component + Send + Sync),*> Bundle for ($($P,)*) {
            fn get_components(self) -> Vec<(Box<dyn Component>, String)> {
                let ($($p),*) = self;
                vec![$((Box::new($p), $P::name())),*]
            }
        }
    }
}

#[macro_export]
macro_rules! impl_sparse_index {
    ($t:ty) => {
        #[allow(trivial_numeric_casts)]
        unsafe impl SparseIndex for $t {
            const MAX: Self = Self::MAX;

            #[inline]
            fn index(self) -> usize {
                self as usize
            }

            #[inline]
            fn from_index(idx: usize) -> Self {
                debug_assert!(TryInto::<Self>::try_into(idx).is_ok(), "{idx} out of range");

                idx as Self
            }
        }
    };
}

#[macro_export]
macro_rules! define_label {
    (
        $(#[$label_attr:meta])*
        $label_trait_name:ident,
        $interner_name:ident
    ) => {
        $crate::define_label!(
            $(#[$label_attr])*
            $label_trait_name,
            $interner_name,
            extra_methods: {},
            extra_methods_impl: {}
        );
    };
    (
        $(#[$label_attr:meta])*
        $label_trait_name:ident,
        $interner_name:ident,
        extra_methods: { $($trait_extra_methods:tt)* },
        extra_methods_impl: { $($interned_extra_methods_impl:tt)* }
    ) => {

        $(#[$label_attr])*
        pub trait $label_trait_name: 'static + Send + Sync + ::std::fmt::Debug {

            $($trait_extra_methods)*

            /// Clones this `
            #[doc = stringify!($label_trait_name)]
            ///`.
            fn dyn_clone(&self) -> ::std::boxed::Box<dyn $label_trait_name>;

            /// Casts this value to a form where it can be compared with other type-erased values.
            fn as_dyn_eq(&self) -> &dyn $crate::dyn_method::DynEq;

            /// Stable cross-DLL identity: `std::any::type_name` of the
            /// concrete label type. Pointer address and `TypeId` both differ
            /// between the host and a statically-linked cdylib copy of this
            /// crate (see `resource::ResourceId`), so label equality and
            /// hashing must use this name: otherwise systems a plugin
            /// registers via `add_systems` land in an orphaned schedule slot
            /// that the host tick loop never visits (silent no-run).
            fn label_name(&self) -> &'static str;

            /// Feeds this value into the given [`Hasher`].
            fn dyn_hash(&self, state: &mut dyn ::std::hash::Hasher);

            /// Returns an [`Interned`] value corresponding to `self`.
            fn intern(&self) -> $crate::intern::Interned<dyn $label_trait_name>
            where Self: Sized {
                $interner_name.intern(self)
            }
        }

        impl $label_trait_name for $crate::intern::Interned<dyn $label_trait_name> {

            $($interned_extra_methods_impl)*

            fn dyn_clone(&self) -> ::std::boxed::Box<dyn $label_trait_name> {
                (**self).dyn_clone()
            }

            /// Casts this value to a form where it can be compared with other type-erased values.
            fn as_dyn_eq(&self) -> &dyn $crate::dyn_method::DynEq {
                (**self).as_dyn_eq()
            }

            fn label_name(&self) -> &'static str {
                (**self).label_name()
            }

            fn dyn_hash(&self, state: &mut dyn ::std::hash::Hasher) {
                (**self).dyn_hash(state);
            }

            fn intern(&self) -> Self {
                *self
            }
        }

        impl PartialEq for dyn $label_trait_name {
            fn eq(&self, other: &Self) -> bool {
                self.label_name() == other.label_name()
            }
        }

        impl Eq for dyn $label_trait_name {}

        impl ::std::hash::Hash for dyn $label_trait_name {
            fn hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                self.label_name().hash(state);
            }
        }

        impl $crate::intern::Internable for dyn $label_trait_name {
            fn leak(&self) -> &'static Self {
                Box::leak(self.dyn_clone())
            }

            fn ref_eq(&self, other: &Self) -> bool {
                // Name-based: pointer address and TypeId are per-module (host
                // vs cdylib statics/vtables differ), so same-source labels
                // from different modules must still match. `Hash` is kept in
                // sync below.
                self.label_name() == other.label_name()
            }

            fn ref_hash<H: ::std::hash::Hasher>(&self, state: &mut H) {
                use ::std::hash::Hash;

                self.label_name().hash(state);
            }
        }

        static $interner_name: $crate::intern::Interner<dyn $label_trait_name> =
            $crate::intern::Interner::new();
    };
}
