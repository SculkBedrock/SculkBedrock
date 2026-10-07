//! `Bundle`: packs several components for one attach call (`spawn(bundle)`).
//!
//! Single components implement this trait automatically; tuples are
//! implemented in bulk via `sc_ecs_macros::all_tuples`.

use crate::component::Component;
use crate::impl_bundle;
use sc_ecs_macros::all_tuples;

pub trait Bundle {
    fn get_components(self) -> Vec<(Box<dyn Component>, String)>;
}

impl<C: Component> Bundle for C {
    fn get_components(self) -> Vec<(Box<dyn Component>, String)> {
        vec![(Box::new(self), C::name())]
    }
}

all_tuples!(impl_bundle, 2, 20, P, p);
