use std::net::SocketAddr;

/// 24 位掩码：RakNet 序列号只使用低 24 位。
///
/// 24-bit mask: RakNet sequence numbers use only the low 24 bits.
pub const U24_MASK: u32 = 0x00ff_ffff;
/// 24 位序号半程：比较新旧时区分“向前”与“向后”的阈值。
///
/// Half range of 24-bit sequence numbers: threshold separating "forward"
/// from "backward" when comparing recency.
pub const U24_HALF_RANGE: u32 = 0x0080_0000;

/// 把任意 32 位值截断为 24 位序号。
///
/// Truncates an arbitrary 32-bit value into a 24-bit sequence number.
#[inline]
pub const fn seq24(value: u32) -> u32 {
    value & U24_MASK
}

/// 在 24 位序号上加指定增量（自动回绕）。
///
/// Adds an amount to a 24-bit sequence number (wraps automatically).
#[inline]
pub const fn add24(value: u32, amount: u32) -> u32 {
    value.wrapping_add(amount) & U24_MASK
}

/// 计算从 `from` 到 `to` 的 24 位向前距离。
///
/// Computes the 24-bit forward distance from `from` to `to`.
#[inline]
pub const fn forward_distance24(from: u32, to: u32) -> u32 {
    (to.wrapping_sub(from)) & U24_MASK
}

/// 判断 `candidate` 是否比 `reference` 更新（回绕安全）。
///
/// Whether `candidate` is newer than `reference` (wrap-safe).
#[inline]
pub const fn is_newer24(candidate: u32, reference: u32) -> bool {
    let distance = forward_distance24(reference, candidate);
    distance != 0 && distance < U24_HALF_RANGE
}

/// 判断 `value` 是否落在 24 位闭区间 `[start, end]` 内。
///
/// Whether `value` lies within the inclusive 24-bit range `[start, end]`.
#[inline]
pub const fn contains_inclusive24(start: u32, end: u32, value: u32) -> bool {
    forward_distance24(start, value) <= forward_distance24(start, end)
}

/// 序号生成器：每次 `next()` 在原值上加一（回绕安全）。
///
/// Sequence generator: each `next()` increments the previous value
/// (wrap-safe).
#[derive(Debug, Clone)]
pub struct SafeGenerator<T> {
    /// 当前序号，仅在生成器内部维护。
    ///
    /// Current sequence value, maintained only inside the generator.
    pub(crate) sequence: T,
}

impl<T> SafeGenerator<T>
where
    T: Default,
{
    /// 以类型默认值（通常为 0）创建生成器。
    ///
    /// Creates a generator starting from the type default (usually 0).
    pub fn new() -> Self {
        Self {
            sequence: T::default(),
        }
    }
}

macro_rules! impl_gen {
    ($n: ty) => {
        impl SafeGenerator<$n> {
            /// 生成下一个序号：先自增再返回（不会返回初值）。
            ///
            /// Produces the next sequence: pre-increments, then returns
            /// (never returns the initial value).
            pub fn next(&mut self) -> $n {
                self.sequence = self.sequence.wrapping_add(1);
                return self.sequence;
            }

            /// 读取当前序号（不推进）。
            ///
            /// Reads the current sequence without advancing.
            pub fn get(&self) -> $n {
                self.sequence
            }
        }
    };
}

impl_gen!(u8);
impl_gen!(u16);
impl_gen!(u32);
impl_gen!(u64);
impl_gen!(u128);
impl_gen!(usize);

/// `loop_exec!` 宏的控制流结果。
///
/// Control-flow result consumed by the `loop_exec!` macro.
pub enum LoopResult {
    /// 继续执行循环。
    ///
    /// Continue executing the loop.
    Continue,
    /// 跳出循环。
    ///
    /// Break out of the loop.
    Break,
}

/// 把 `LoopResult` 转换为 `continue`/`break` 控制流的小宏。
///
/// 用法：`loop_exec! { ... }`，其中表达式需返回 `LoopResult`。
///
/// Tiny macro turning a `LoopResult` into `continue`/`break` control flow.
///
/// Usage: `loop_exec! { ... }` where the expression yields `LoopResult`.
#[macro_export]
macro_rules! loop_exec {
    ($code: expr) => {
        match $code {
            LoopResult::Continue => continue,
            LoopResult::Break => break,
        }
    };
}

/// 把 `SocketAddr` 格式化为 `ip:port` 形式的连接标识。
///
/// Formats a `SocketAddr` into an `ip:port` connection token.
pub fn to_address_token(remote: SocketAddr) -> String {
    let mut address = remote.ip().to_string();
    address.push_str(":");
    address.push_str(remote.port().to_string().as_str());
    address
}

#[cfg(test)]
mod tests {
    use super::{add24, contains_inclusive24, forward_distance24, is_newer24, seq24};

    #[test]
    fn sequence_arithmetic_wraps_at_u24() {
        assert_eq!(seq24(0x01ff_ffff), 0x00ff_ffff);
        assert_eq!(add24(0x00ff_fffe, 1), 0x00ff_ffff);
        assert_eq!(add24(0x00ff_ffff, 1), 0);
        assert_eq!(forward_distance24(0x00ff_fffe, 1), 3);
    }

    #[test]
    fn sequence_comparison_uses_half_range() {
        assert!(is_newer24(1, 0));
        assert!(is_newer24(0, 0x00ff_ffff));
        assert!(!is_newer24(0x0080_0000, 0));
        assert!(contains_inclusive24(0x00ff_fffe, 1, 0));
    }
}
