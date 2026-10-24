//! SculkBedrock 共享错误原语。
//!
//! 每个领域错误暴露三部分：稳定的机器可读 [`ScCode`]、英文人类可读的 `detail`
//! 字符串，以及构造位置 [`ScSite`]。匹配必须使用 `code()`；`detail()` 仅用于日志
//! 与诊断。
//!
//! Shared error primitives for SculkBedrock.
//!
//! Every domain error exposes three things: a stable machine-readable
//! [`ScCode`], an English human-readable `detail` string, and the
//! [`ScSite`] where it was constructed. Matching must use `code()`;
//! `detail()` is for logs and diagnostics only.

mod code;
mod site;

pub mod ffi;

pub use code::ScCode;
pub use sc_error_macros::ScError;
pub use site::{capture_site, ScSite};
pub use thiserror::Error;

/// 所有领域错误共同实现的接口。
///
/// Common interface implemented by all domain errors.
pub trait ScError: std::error::Error {
    /// 稳定的错误码，例如 `"world.storage.io"`。码只增不改。
    ///
    /// Stable code such as `"world.storage.io"`. Codes are append-only.
    fn code(&self) -> ScCode;
    /// 调用位置（若已捕获）。
    ///
    /// Construction site, if captured.
    fn site(&self) -> Option<ScSite>;
    /// 解释成因的英文详情。
    ///
    /// English detail explaining the cause.
    fn detail(&self) -> &str;
}
