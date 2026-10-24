//! 系统参数集合：可注入系统的各类参数实现。
//!
//! 每个参数类型都配有一份 `SystemParamState`（保存跨帧的持久状态）与一个
//! `SystemParam` 实现（描述本次调用的借用类型）。
//!
//! The system parameter set: the parameter implementations injectable into
//! systems.
//!
//! Each parameter type comes with a `SystemParamState` (holding persistent
//! state across runs) and a `SystemParam` implementation (describing the
//! borrowed type of a single call).
pub mod component;
pub mod event;
pub mod local;
pub mod query;
pub mod resource;
