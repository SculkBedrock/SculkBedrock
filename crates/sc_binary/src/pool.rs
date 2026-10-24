//! 字节切片池：复用切片以减少重复分配。
//!
//! 归还的切片会立即被复用；不要在多个线程间共享（会增加锁竞争）。
//!
//! Byte-slice pool that reuses slices to avoid repeated allocation.
//!
//! Returned slices are reused immediately; do not share across threads (adds
//! lock contention).

/// 复用字节切片以避免重复分配。
///
/// Byte-slice pool that reuses slices instead of allocating new ones.
pub struct BytePool {}
