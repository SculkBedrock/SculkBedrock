use std::net::SocketAddr;

pub const U24_MASK: u32 = 0x00ff_ffff;
pub const U24_HALF_RANGE: u32 = 0x0080_0000;

#[inline]
pub const fn seq24(value: u32) -> u32 {
    value & U24_MASK
}

#[inline]
pub const fn add24(value: u32, amount: u32) -> u32 {
    value.wrapping_add(amount) & U24_MASK
}

#[inline]
pub const fn forward_distance24(from: u32, to: u32) -> u32 {
    (to.wrapping_sub(from)) & U24_MASK
}

#[inline]
pub const fn is_newer24(candidate: u32, reference: u32) -> bool {
    let distance = forward_distance24(reference, candidate);
    distance != 0 && distance < U24_HALF_RANGE
}

#[inline]
pub const fn contains_inclusive24(start: u32, end: u32, value: u32) -> bool {
    forward_distance24(start, value) <= forward_distance24(start, end)
}

#[derive(Debug, Clone)]
pub struct SafeGenerator<T> {
    pub(crate) sequence: T,
}

impl<T> SafeGenerator<T>
where
    T: Default,
{
    pub fn new() -> Self {
        Self {
            sequence: T::default(),
        }
    }
}

macro_rules! impl_gen {
    ($n: ty) => {
        impl SafeGenerator<$n> {
            pub fn next(&mut self) -> $n {
                self.sequence = self.sequence.wrapping_add(1);
                return self.sequence;
            }

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

pub enum LoopResult {
    Continue,
    Break,
}

#[macro_export]
macro_rules! loop_exec {
    ($code: expr) => {
        match $code {
            LoopResult::Continue => continue,
            LoopResult::Break => break,
        }
    };
}

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
