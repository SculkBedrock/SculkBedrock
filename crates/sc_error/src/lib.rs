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

/// Common interface implemented by all domain errors.
pub trait ScError: std::error::Error {
    /// Stable code such as `"world.storage.io"`. Codes are append-only.
    fn code(&self) -> ScCode;
    /// Construction site, if captured.
    fn site(&self) -> Option<ScSite>;
    /// English detail explaining the cause.
    fn detail(&self) -> &str;
}
