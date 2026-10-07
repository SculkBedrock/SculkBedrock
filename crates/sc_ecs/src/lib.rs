//! sc_ecs: async ECS framework (Bevy-inspired).
//!
//! Core concepts: `World` (entities/components/resources/events), `App`
//! (plugin assembly + schedule loop), `Schedule` (label-ordered system table,
//! 20 TPS main loop), `SystemParam` (dependency injection).
//! Concurrency rules live in `world.rs` and docs/ecs_concurrency.md: all
//! mutable component/resource state goes through interior locks
//! (`Arc<RwLock<..>>`), and `get_component -> Arc<C>` may be held across await.
pub mod app;
pub mod app_manager;
pub mod async_manager;
pub mod bundle;
pub mod component;
pub mod dyn_method;
pub mod entity;
pub mod event;
pub mod intern;
pub mod local_manager;
pub mod r#macro;
pub mod params;
pub mod region;
pub mod resource;
pub mod schedule;
pub mod system;
#[cfg(test)]
mod tests;
pub mod world;
