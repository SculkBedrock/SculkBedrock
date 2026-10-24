//! 二进制读写工具：无 panic 的字节流读写、派生宏与变长整数类型。
//!
//! 提供：
//! - [`io`]：`ByteReader` / `ByteWriter`，基于 `bytes::Buf` / `BufMut` 的手动读写；
//! - [`interfaces`]：`Reader` / `Writer` trait，用于自定义类型的读写；
//! - [`BinaryIo`]：为类型派生 `Reader` 与 `Writer` 的宏；
//! - [`types`]：`u24`、`i24`、`varu32`、`vari32`、`varu64`、`vari64`、`LE`、`BE` 等非原生类型。
//!
//! Binary read/write utilities: panic-free byte-stream IO, derive macros and
//! variable-length integer types.
//!
//! Provides:
//! - [`io`]: `ByteReader` / `ByteWriter` over `bytes::Buf` / `BufMut` for manual reads and writes;
//! - [`interfaces`]: the `Reader` / `Writer` traits for custom types;
//! - [`BinaryIo`]: a derive macro implementing `Reader` and `Writer`;
//! - [`types`]: non-primitive types such as `u24`, `i24`, `varu32`, `vari32`, `varu64`, `vari64`, `LE` and `BE`.

/// 提供无 panic 的二进制读写。各方法按 protobuf 编码规范处理变长整数。
///
/// Provides panic-free binary reads and writes. Variable-length integers follow
/// the protobuf encoding specification.
///
/// ## Example
/// ```no_run
/// use sc_binary::io::ByteReader;
///
/// const VARINT: &[u8] = &[255, 255, 255, 255, 7]; // 2147483647
/// fn main() {
///     let mut buf = ByteReader::from(&VARINT[..]);
///     assert_eq!(buf.read_var_u32().unwrap(), 2147483647);
/// }
/// ```
pub mod interfaces;

/// 实现 `Reader` 与 `Writer` 的派生宏。
///
/// Derive macro implementing `Reader` and `Writer`.
pub use sc_binary_macros::*;
/// 为 `bytes::Buf` 与 `bytes::BufMut` 提供读写实现。
///
/// Read/write implementations for `bytes::Buf` and `bytes::BufMut`.
///
/// Example:
/// ```no_run
/// use sc_binary::io::ByteReader;
///
/// fn main() {
///    const VARINT: &[u8] = &[255, 255, 255, 255, 7]; // 2147483647
///    let mut buf = ByteReader::from(&VARINT[..]);
///    assert_eq!(buf.read_var_u32().unwrap(), 2147483647);
/// }
/// ```
pub mod io;
pub mod mc;
pub mod pool;
/// 提供 `u24`、`i24`、`varu32` 等非原生整数类型的读写。
///
/// Read/write support for non-primitive integer types such as `u24`, `i24`
/// and `varu32`.
pub mod types;

/// 遗留错误类型（已弃用，使用 `std::io::Error`）。
///
/// Legacy error type (deprecated; use `std::io::Error`).
#[deprecated = "This module is deprecated in favor of std::io::Error."]
pub mod error {
    /// 二进制读写错误（已弃用）。
    ///
    /// Binary read/write error (deprecated).
    #[derive(Debug, PartialEq)]
    pub enum BinaryError {
        /// Offset is out of bounds
        ///
        /// **Tuple Values:**
        /// - `usize` = Given Offset.
        /// - `usize` = Stream length.
        /// - `&'static str` = Message to add on to the error.
        OutOfBounds(usize, usize, &'static str),

        /// Similar to `OutOfBounds` except it means;
        /// the stream tried to read more than possible.
        ///
        /// **Tuple Values:**
        /// - `usize` = Stream length.
        EOF(usize),

        /// A known error that was recoverable to safely proceed the stack.
        RecoverableKnown(String),

        /// An unknown error occurred, but it wasn't critical,
        /// we can safely proceed on the stack.
        RecoverableUnknown,
    }

    impl BinaryError {
        pub fn get_message(&self) -> String {
            match self {
                Self::OutOfBounds(offset, length, append) => {
                    format!("Offset {} out of range for a buffer size with: {}. {}", offset, length, append)
                },
                Self::EOF(length) => format!("Buffer reached End Of File at offset: {}", length),
                Self::RecoverableKnown(msg) => msg.clone(),
                Self::RecoverableUnknown => "An interruption occurred when performing a binary operation, however this error was recovered safely.".to_string()
            }
        }
    }

    impl From<std::io::Error> for BinaryError {
        fn from(_error: std::io::Error) -> Self {
            Self::RecoverableUnknown
        }
    }

    impl std::fmt::Display for BinaryError {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{}", self.get_message())
        }
    }
}

#[allow(deprecated)]
pub use interfaces::Streamable;
pub use io::{ByteReader, ByteWriter};
