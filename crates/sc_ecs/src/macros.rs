/// 为插件元组实现 `Plugins`：把每个插件依次添加到 App。
///
/// Implements `Plugins` for tuples of plugins: adds each plugin to the
/// app in order.
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

/// 为普通函数实现 `SystemFunction`：支持以原始参数或注入参数调用。
///
/// Implements `SystemFunction` for plain functions: supports calling with
/// either raw or injected parameters.
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

/// 为系统函数元组实现 `IntoSystem`：转换为按顺序运行的 `TupleFunctionSystem`。
///
/// Implements `IntoSystem` for tuples of system functions: converts them
/// into a `TupleFunctionSystem` that runs in order.
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

/// 为系统参数元组实现 `SystemParam`：元组整体作为一个参数注入。
///
/// Implements `SystemParam` for tuples of parameters: the tuple is
/// injected as a single parameter.
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

/// 为系统参数状态元组实现 `SystemParamState`：任一参数缺失则整体跳过。
///
/// Implements `SystemParamState` for tuples of parameter states: the
/// whole tuple is skipped when any parameter is missing.
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

/// 为组件元组实现 `Bundle`：元组作为组件束一次性附加。
///
/// Implements `Bundle` for tuples of components: the tuple is attached
/// as a bundle in one shot.
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

/// 为整数类型实现 `SparseIndex`：直接以自身值作为索引。
///
/// Implements `SparseIndex` for integer types: uses the value itself as
/// the index.
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

/// 定义一个标签 trait 及其驻留器：标签可跨 DLL 按类型名比较、哈希与驻留。
///
/// Defines a label trait together with its interner: labels can be
/// compared, hashed and interned by type name across DLLs.
///
/// 两种形式：基础形式使用空的额外方法；扩展形式通过 `extra_methods` /
/// `extra_methods_impl` 注入 trait 与驻留实现的额外方法。
///
/// Two forms: the basic form uses empty extra methods; the extended form
/// injects extra trait/impl methods via `extra_methods` /
/// `extra_methods_impl`.
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

            /// 克隆本标签（装箱为 trait 对象）。
            ///
            /// Clones this `
            #[doc = stringify!($label_trait_name)]
            ///`.
            fn dyn_clone(&self) -> ::std::boxed::Box<dyn $label_trait_name>;

            /// 将本值转换为可与其他类型擦除值比较的形式。
            ///
            /// Casts this value to a form where it can be compared with other type-erased values.
            fn as_dyn_eq(&self) -> &dyn $crate::dyn_method::DynEq;

            /// 稳定的跨 DLL 标识：具体标签类型的 `std::any::type_name`。
            /// `TypeId` 与指针地址在宿主与本 crate 的静态链接 cdylib 副本之间
            /// 不一致，因此相等性与哈希必须基于该名称。
            ///
            /// Stable cross-DLL identity: `std::any::type_name` of the
            /// concrete label type. TypeId and pointer addresses differ
            /// between the host and a statically-linked cdylib copy of this
            /// crate, so equality and hashing must use this name.
            fn label_name(&self) -> &'static str;

            /// 将本值送入给定的 [`Hasher`]。
            ///
            /// Feeds this value into the given [`Hasher`].
            fn dyn_hash(&self, state: &mut dyn ::std::hash::Hasher);

            /// 返回与 `self` 对应的 [`Interned`] 值。
            ///
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

            /// 将本值转换为可与其他类型擦除值比较的形式。
            ///
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
