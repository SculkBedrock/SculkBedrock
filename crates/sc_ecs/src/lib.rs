//! sc_ecs：异步 ECS 框架。
//!
//! 核心概念：`World`（实体/组件/资源/事件）、`App`（插件组装 + 调度循环）、
//! `Schedule`（按标签排序的系统表，20 TPS 主循环）、`SystemParam`（依赖注入）。
//!
//! sc_ecs: an async ECS framework.
//!
//! Core concepts: `World` (entities/components/resources/events), `App`
//! (plugin assembly + schedule loop), `Schedule` (label-ordered system table,
//! 20 TPS main loop), `SystemParam` (dependency injection).
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
pub mod macros;
pub mod params;
pub mod region;
pub mod resource;
pub mod schedule;
pub mod system;
#[cfg(test)]
mod tests;
pub mod world;
