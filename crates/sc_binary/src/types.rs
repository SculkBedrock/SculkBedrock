//! Custom numeric types: i24/u24 (3 bytes) and the varint family (varu32/varu64/vari32/vari64).
//!
//! Provides `From` conversions plus `Writer`/`Reader` support for direct use in protocol fields.

use core::fmt;

macro_rules! impl_type {
    (
        $ty: ty, $for_ty: ty
    ) => {
        impl From<$ty> for $for_ty {
            fn from(val: $ty) -> Self {
                val.0
            }
        }

        impl From<$for_ty> for $ty {
            fn from(val: $for_ty) -> Self {
                Self::new(val)
            }
        }

        impl std::ops::Deref for $ty {
            type Target = $for_ty;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl std::ops::DerefMut for $ty {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

/// Little Endian (LE) wrapper type
/// 标记字段以小端格式读写，主要配合 `BinaryIo` 派生宏使用。
///
/// Marks a field as little-endian; intended for use with the `BinaryIo` derive.
///
/// # Example
/// ```rust ignore
/// use sc_binary::types::LE;
/// use sc_binary::BinaryIo;
///
/// #[derive(BinaryIo)]
/// struct MyStruct {
///    tests: LE<u32>,
/// }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LE<T>(pub T);

impl<T> LE<T> {
    pub fn new(val: T) -> Self {
        Self(val)
    }
}
impl_type!(LE<u16>, u16);
impl_type!(LE<u24>, u24);
impl_type!(LE<u32>, u32);
impl_type!(LE<u64>, u64);
impl_type!(LE<u128>, u128);
impl_type!(LE<i16>, i16);
impl_type!(LE<i24>, i24);
impl_type!(LE<i32>, i32);
impl_type!(LE<i64>, i64);
impl_type!(LE<i128>, i128);
impl_type!(LE<f32>, f32);
impl_type!(LE<f64>, f64);

/// 标记字段以大端格式读写，主要配合 `BinaryIo` 派生宏使用。
///
/// Marks a field as big-endian; intended for use with the `BinaryIo` derive.
///
/// # Example
/// ```rust ignore
/// use sc_binary::types::BE;
/// use sc_binary::BinaryIo;
///
/// #[derive(BinaryIo)]
/// struct MyStruct {
///   tests: BE<u32>,
/// }
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BE<T>(pub T);

impl<T> BE<T> {
    pub fn new(val: T) -> Self {
        Self(val)
    }
}

impl_type!(BE<u16>, u16);
impl_type!(BE<u24>, u24);
impl_type!(BE<u32>, u32);
impl_type!(BE<u64>, u64);
impl_type!(BE<u128>, u128);
impl_type!(BE<i16>, i16);
impl_type!(BE<i24>, i24);
impl_type!(BE<i32>, i32);
impl_type!(BE<i64>, i64);
impl_type!(BE<i128>, i128);
impl_type!(BE<f32>, f32);
impl_type!(BE<f64>, f64);

/// 无符号 24 位整数，用于 `BinaryIo` 派生。
///
/// Unsigned 24-bit integer, used with the `BinaryIo` derive.
///
/// # Example
/// ```rust ignore
/// use sc_binary::types::u24;
/// use sc_binary::BinaryIo;
///
/// #[derive(BinaryIo)]
/// struct MyStruct {
///    tests: u24,
/// }
/// ```
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct u24(pub u32);

impl u24 {
    pub fn new(val: u32) -> Self {
        if val <= 0xFFFFFF {
            Self(val)
        } else {
            panic!("u24: value out of range")
        }
    }
}

impl fmt::Display for u24 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl_type!(u24, u32);

/// 有符号 24 位整数，用于 `BinaryIo` 派生。
///
/// Signed 24-bit integer, used with the `BinaryIo` derive.
///
/// # Example
/// ```rust ignore
/// use sc_binary::types::i24;
/// use sc_binary::BinaryIo;
///
/// #[derive(BinaryIo)]
/// struct MyStruct {
///   tests: i24,
/// }
/// ```
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct i24(pub i32);

impl i24 {
    pub fn new(val: i32) -> Self {
        if val >= -0x800000 && val <= 0x7FFFFF {
            Self(val)
        } else {
            panic!("i24: value out of range")
        }
    }
}

impl fmt::Display for i24 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl_type!(i24, i32);

/// 最长 32 位的无符号变长整数，用于 `BinaryIo` 派生。直接读写变长整数时请使用
/// `ByteReader` / `ByteWriter` 的 `read_var_u32` / `write_var_u32`。
///
/// Unsigned variable-length integer of up to 32 bits, used with the `BinaryIo`
/// derive. For direct reads and writes use the `ByteReader` / `ByteWriter`
/// methods `read_var_u32` / `write_var_u32`.
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct varu32(pub u32);

impl varu32 {
    pub fn new(val: u32) -> Self {
        Self(val)
    }
}
impl_type!(varu32, u32);

/// 最长 32 位的有符号变长整数，用于 `BinaryIo` 派生。直接读写变长整数时请使用
/// `ByteReader` / `ByteWriter` 的 `read_var_i32` / `write_var_i32`。
///
/// Signed variable-length integer of up to 32 bits, used with the `BinaryIo`
/// derive. For direct reads and writes use the `ByteReader` / `ByteWriter`
/// methods `read_var_i32` / `write_var_i32`.
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct vari32(pub i32);

impl vari32 {
    pub fn new(val: i32) -> Self {
        Self(val)
    }
}
impl_type!(vari32, i32);

/// 最长 64 位的无符号变长整数，用于 `BinaryIo` 派生。直接读写变长整数时请使用
/// `ByteReader` / `ByteWriter` 的 `read_var_u64` / `write_var_u64`。
///
/// Unsigned variable-length integer of up to 64 bits, used with the `BinaryIo`
/// derive. For direct reads and writes use the `ByteReader` / `ByteWriter`
/// methods `read_var_u64` / `write_var_u64`.
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct varu64(pub u64);

impl varu64 {
    pub fn new(val: u64) -> Self {
        Self(val)
    }
}

impl_type!(varu64, u64);

/// 最长 64 位的有符号变长整数，用于 `BinaryIo` 派生。直接读写变长整数时请使用
/// `ByteReader` / `ByteWriter` 的 `read_var_i64` / `write_var_i64`。
///
/// Signed variable-length integer of up to 64 bits, used with the `BinaryIo`
/// derive. For direct reads and writes use the `ByteReader` / `ByteWriter`
/// methods `read_var_i64` / `write_var_i64`.
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct vari64(pub i64);

impl vari64 {
    pub fn new(val: i64) -> Self {
        Self(val)
    }
}

impl_type!(vari64, i64);
