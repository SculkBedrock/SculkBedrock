//! NBT 读取入口：`NbtReader` 及 `NbtReadTrait` 实现（网络/本地变体）。
//!
//! NBT read entry point: `NbtReader` plus `NbtReadTrait` implementations (network/local variants).

use crate::NbtValue;
use std::io;
use sc_binary::ByteReader;

pub trait NbtReadTrait {
    fn read(reader: &mut ByteReader) -> io::Result<NbtValue>;
}

pub struct NbtReader<'a> {
    reader: &'a mut ByteReader,
}

impl<'a> NbtReader<'a> {
    pub fn from_reader(reader: &'a mut ByteReader) -> NbtReader<'a> {
        Self { reader }
    }

    pub fn read<T: NbtReadTrait>(&mut self) -> io::Result<NbtValue> {
        T::read(self.reader)
    }
}
