//! NBT write entry point: `NbtWriter` plus `NbtWriteTrait` implementations; `NbtCustomWrite`
//! provides the NBT representation for custom types such as bool.

use crate::{write_impl, NbtValue};
use std::io;
use sc_binary::ByteWriter;
pub use sc_nbt_macros::NbtWrite;

pub trait NbtWriteTrait {
    fn write(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()>;
}

pub struct NbtWriter<'a> {
    writer: &'a mut ByteWriter,
}

impl<'a> NbtWriter<'a> {
    pub fn from_writer(writer: &'a mut ByteWriter) -> NbtWriter<'a> {
        Self { writer }
    }

    pub fn write<T: NbtWriteTrait>(&mut self, value: &NbtValue) -> io::Result<()> {
        T::write(self.writer, value)
    }

    pub fn as_slice(&self) -> &[u8] {
        self.writer.as_slice()
    }
}

pub trait NbtCustomWrite {
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> io::Result<()>;
    fn to_nbt(&self) -> Option<NbtValue>;
}

write_impl![
    Byte: i8,
    Short: i16,
    Int: i32,
    Long: i64,
    Float: f32,
    Double: f64,
    String: String
];

impl NbtCustomWrite for bool {
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> io::Result<()> {
        writer.write::<T>(&self.to_nbt().unwrap())
    }
    fn to_nbt(&self) -> Option<NbtValue> {
        Some(NbtValue::Byte(if *self { 1 } else { 0 }))
    }
}

impl<W> NbtCustomWrite for Vec<W>
where
    W: NbtCustomWrite + 'static,
{
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> io::Result<()> {
        let value = self.to_nbt().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "cannot encode an empty or heterogeneous NBT vector",
            )
        })?;
        writer.write::<T>(&value)
    }

    fn to_nbt(&self) -> Option<NbtValue> {
        if self.is_empty() {
            return None;
        }

        //Check Type
        let mat = self.get(0)?.to_nbt()?;
        match mat {
            NbtValue::Byte(_) => {
                let mut vec = Vec::new();
                for v in self {
                    let Some(value) = v.to_nbt() else {
                        return None;
                    };
                    vec.push(value.as_i8()?);
                }
                return Some(NbtValue::ByteArray(vec));
            }
            NbtValue::Int(_) => {
                let mut vec = Vec::new();
                for v in self {
                    let Some(value) = v.to_nbt() else {
                        return None;
                    };
                    vec.push(value.as_i32()?);
                }
                return Some(NbtValue::IntArray(vec));
            }
            NbtValue::Long(_) => {
                let mut vec = Vec::new();
                for v in self {
                    let Some(value) = v.to_nbt() else {
                        return None;
                    };
                    vec.push(value.as_i64()?);
                }
                return Some(NbtValue::LongArray(vec));
            }
            _ => {}
        }

        let mut vec = Vec::new();
        for v in self {
            if let Some(value) = v.to_nbt() {
                vec.push(value);
            }
        }
        Some(NbtValue::List(vec))
    }
}

impl<W: NbtCustomWrite> NbtCustomWrite for Option<W> {
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> io::Result<()> {
        if let Some(v) = self.to_nbt() {
            writer.write::<T>(&v)?;
        }
        Ok(())
    }

    fn to_nbt(&self) -> Option<NbtValue> {
        match self {
            Some(v) => v.to_nbt(),
            None => None,
        }
    }
}

impl NbtCustomWrite for NbtValue {
    fn write<T: NbtWriteTrait>(&self, writer: &mut NbtWriter) -> io::Result<()> {
        writer.write::<T>(self)
    }
    fn to_nbt(&self) -> Option<NbtValue> {
        Some(self.clone())
    }
}
