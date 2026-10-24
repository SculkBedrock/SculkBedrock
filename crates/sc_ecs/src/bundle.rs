//! `Bundle`：将多个组件打包，供一次附加调用使用（`spawn(bundle)`）。
//!
//! 单个组件自动实现本 trait；元组通过 `sc_ecs_macros::all_tuples` 批量实现。
//!
//! `Bundle`: packs several components for one attach call (`spawn(bundle)`).
//!
//! Single components implement this trait automatically; tuples are
//! implemented in bulk via `sc_ecs_macros::all_tuples`.

use crate::component::Component;
use crate::impl_bundle;
use sc_ecs_macros::all_tuples;

/// 组件束：可被一次性展开为若干（组件，名称）对的组合。
///
/// A bundle: a combination that expands to several (component, name) pairs
/// in one shot.
pub trait Bundle {
    /// 消费自身，返回组件与对应名称的列表。
    ///
    /// Consumes self and returns the packed components with their names.
    fn get_components(self) -> Vec<(Box<dyn Component>, String)>;
}

impl<C: Component> Bundle for C {
    /// 将单个组件包装成（组件, 名称）列表。
    ///
    /// Wraps a single component into a (component, name) list.
    fn get_components(self) -> Vec<(Box<dyn Component>, String)> {
        vec![(Box::new(self), C::name())]
    }
}

all_tuples!(impl_bundle, 2, 20, P, p);
