use crate::compound::CompoundNbt;
use crate::reader::NbtReadTrait;
use crate::writer::NbtWriteTrait;
use crate::NbtValue;
use std::collections::HashMap;
use std::io;
use sc_binary::{ByteReader, ByteWriter};

pub struct BedrockLocalNbt;

const MAX_NBT_DEPTH: usize = 512;
const MAX_NBT_CONTAINER_LEN: usize = 1_000_000;

fn ensure_depth(depth: usize) -> io::Result<()> {
    if depth > MAX_NBT_DEPTH {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NBT recursion depth exceeded",
        ))
    } else {
        Ok(())
    }
}

fn checked_len_i16(len: i16, remaining: usize) -> io::Result<usize> {
    checked_len_i32(i32::from(len), remaining, 1)
}

fn checked_len_i32(len: i32, remaining: usize, min_item_bytes: usize) -> io::Result<usize> {
    if len < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Negative NBT length",
        ));
    }
    let len = len as usize;
    if len > MAX_NBT_CONTAINER_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NBT length exceeds container limit",
        ));
    }
    if min_item_bytes > 0 {
        let required = len
            .checked_mul(min_item_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "NBT length overflows"))?;
        if required > remaining {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "NBT length exceeds remaining input",
            ));
        }
    }
    Ok(len)
}

fn tag_min_payload_bytes(type_id: u8) -> usize {
    match type_id {
        1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 4,
        6 => 8,
        7 => 4,
        8 => 2,
        9 => 5,
        10 => 1,
        11 => 4,
        12 => 4,
        _ => 1,
    }
}

impl BedrockLocalNbt {
    #[inline]
    fn read_string(reader: &mut ByteReader) -> io::Result<String> {
        let len = checked_len_i16(reader.read_i16_le()?, reader.as_slice().len())?;
        let vec = reader.read_bytes(len as usize)?.to_vec();
        String::from_utf8(vec).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    #[inline]
    fn read_i8_array(reader: &mut ByteReader) -> io::Result<Vec<i8>> {
        let len = checked_len_i32(reader.read_i32_le()?, reader.as_slice().len(), 1)?;
        Ok(reader.read_i8_bytes(len)?)
    }

    #[inline]
    fn read_i32_le_array(reader: &mut ByteReader) -> io::Result<Vec<i32>> {
        let len = checked_len_i32(reader.read_i32_le()?, reader.as_slice().len(), 4)?;
        Ok(reader.read_i32_le_array(len)?)
    }

    fn read_i64_le_array(reader: &mut ByteReader) -> io::Result<Vec<i64>> {
        let len = checked_len_i32(reader.read_i32_le()?, reader.as_slice().len(), 8)?;
        Ok(reader.read_i64_le_array(len)?)
    }
    #[inline]
    fn read_any(reader: &mut ByteReader, type_id: u8, depth: usize) -> io::Result<NbtValue> {
        ensure_depth(depth)?;
        match type_id {
            1 => Ok(NbtValue::Byte(reader.read_i8()?)),
            2 => Ok(NbtValue::Short(reader.read_i16_le()?)),
            3 => Ok(NbtValue::Int(reader.read_i32_le()?)),
            4 => Ok(NbtValue::Long(reader.read_i64_le()?)),
            5 => Ok(NbtValue::Float(reader.read_f32_le()?)),
            6 => Ok(NbtValue::Double(reader.read_f64_le()?)),
            7 => Ok(NbtValue::ByteArray(Self::read_i8_array(reader)?)),
            8 => Ok(NbtValue::String(Self::read_string(reader)?)),
            9 => Ok(NbtValue::List(Self::read_list(reader, depth + 1)?)),
            10 => Ok(NbtValue::Compound(Self::read_compound(
                reader,
                None,
                depth + 1,
            )?)),
            11 => Ok(NbtValue::IntArray(Self::read_i32_le_array(reader)?)),
            12 => Ok(NbtValue::LongArray(Self::read_i64_le_array(reader)?)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid NBT type: {:?}", type_id),
            )),
        }
    }
    #[inline]
    fn read_list(reader: &mut ByteReader, depth: usize) -> io::Result<Vec<NbtValue>> {
        ensure_depth(depth)?;
        let type_id = reader.read_u8()?;
        let len = checked_len_i32(
            reader.read_i32_le()?,
            reader.as_slice().len(),
            tag_min_payload_bytes(type_id),
        )?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(Self::read_any(reader, type_id, depth + 1)?);
        }
        Ok(list)
    }

    #[inline]
    fn read_compound(
        reader: &mut ByteReader,
        name: Option<String>,
        depth: usize,
    ) -> io::Result<CompoundNbt> {
        ensure_depth(depth)?;
        let mut map = HashMap::new();
        loop {
            let type_id = reader.read_u8()?;
            if type_id == 0 {
                break;
            }
            let key = Self::read_string(reader)?;
            let value = Self::read_any(reader, type_id, depth + 1)?;
            map.insert(key, value);
        }
        Ok(CompoundNbt::from_map(name, map))
    }
}

impl NbtReadTrait for BedrockLocalNbt {
    fn read(reader: &mut ByteReader) -> io::Result<NbtValue> {
        match reader.read_u8()? {
            9 => Ok(NbtValue::List(Self::read_list(reader, 0)?)),
            10 => {
                let name = Self::read_string(reader)?;
                Ok(NbtValue::Compound(Self::read_compound(
                    reader,
                    Some(name),
                    0,
                )?))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid NBT type",
            )),
        }
    }
}

impl BedrockLocalNbt {
    /// Write a string: u16 LE length + UTF-8 bytes (mirrors read_string).
    #[inline]
    fn write_string(writer: &mut ByteWriter, string: &str) -> io::Result<()> {
        let bytes = string.as_bytes();
        if bytes.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "NBT string too long",
            ));
        }
        writer.write_u16_le(bytes.len() as u16)?;
        writer.write_raw(bytes)
    }

    #[inline]
    fn write_any(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()> {
        match value {
            NbtValue::Byte(v) => writer.write_i8(*v),
            NbtValue::Short(v) => writer.write_i16_le(*v),
            NbtValue::Int(v) => writer.write_i32_le(*v),
            NbtValue::Long(v) => writer.write_i64_le(*v),
            NbtValue::Float(v) => writer.write_f32_le(*v),
            NbtValue::Double(v) => writer.write_f64_le(*v),
            NbtValue::ByteArray(v) => {
                writer.write_i32_le(v.len() as i32)?;
                writer.write_raw(bytemuck_slice(v))
            }
            NbtValue::String(v) => Self::write_string(writer, v),
            NbtValue::List(v) => Self::write_list(writer, v),
            NbtValue::Compound(v) => Self::write_compound_content(writer, v),
            NbtValue::IntArray(v) => {
                writer.write_i32_le(v.len() as i32)?;
                for item in v {
                    writer.write_i32_le(*item)?;
                }
                Ok(())
            }
            NbtValue::LongArray(v) => {
                writer.write_i32_le(v.len() as i32)?;
                for item in v {
                    writer.write_i64_le(*item)?;
                }
                Ok(())
            }
        }
    }

    /// Write a List: element type (u8) + i32 LE length + element payloads.
    #[inline]
    fn write_list(writer: &mut ByteWriter, vec: &[NbtValue]) -> io::Result<()> {
        if vec.is_empty() {
            writer.write_u8(1)?; // Empty lists default to element type TAG_Byte
            writer.write_i32_le(0)?;
            return Ok(());
        }
        let tag = vec[0].tag();
        if !vec.iter().all(|item| item.tag() == tag) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "List tag not same",
            ));
        }
        writer.write_u8(tag)?;
        writer.write_i32_le(vec.len() as i32)?;
        for item in vec {
            Self::write_any(writer, item)?;
        }
        Ok(())
    }

    /// Write compound content (entries + TAG_End) without the name.
    #[inline]
    fn write_compound_content(writer: &mut ByteWriter, compound: &CompoundNbt) -> io::Result<()> {
        for (key, value) in compound.iter() {
            writer.write_u8(value.tag())?;
            Self::write_string(writer, key)?;
            Self::write_any(writer, value)?;
        }
        writer.write_u8(0)?; // TAG_End
        Ok(())
    }
}

impl NbtWriteTrait for BedrockLocalNbt {
    /// Write the root tag: tag type (u8) + root name (u16 LE string) + payload.
    fn write(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()> {
        match value {
            NbtValue::List(_) => {
                writer.write_u8(9)?; // TAG_List
                Self::write_string(writer, "")?;
                Self::write_any(writer, value)
            }
            NbtValue::Compound(data) => {
                writer.write_u8(10)?; // TAG_Compound
                let name = data.name.as_deref().unwrap_or("");
                Self::write_string(writer, name)?;
                Self::write_any(writer, value)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Root NBT must be Compound or List",
            )),
        }
    }
}

/// Reinterpret an i8 slice as a u8 slice (NBT byte arrays share the layout; zero-copy).
#[inline]
fn bytemuck_slice(v: &[i8]) -> &[u8] {
    // SAFETY: i8/u8 are both 1 byte with identical layout; only the type label differs.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len()) }
}

pub struct JavaLocalNbt;

impl JavaLocalNbt {
    #[inline]
    fn read_string(reader: &mut ByteReader) -> io::Result<String> {
        let len = checked_len_i16(reader.read_i16()?, reader.as_slice().len())?;
        let vec = reader.read_bytes(len)?.to_vec();
        String::from_utf8(vec).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    #[inline]
    fn read_i8_array(reader: &mut ByteReader) -> io::Result<Vec<i8>> {
        let len = checked_len_i32(reader.read_i32()?, reader.as_slice().len(), 1)?;
        Ok(reader.read_i8_bytes(len)?)
    }

    #[inline]
    fn read_i32_array(reader: &mut ByteReader) -> io::Result<Vec<i32>> {
        let len = checked_len_i32(reader.read_i32()?, reader.as_slice().len(), 4)?;
        Ok(reader.read_i32_array(len)?)
    }

    fn read_i64_array(reader: &mut ByteReader) -> io::Result<Vec<i64>> {
        let len = checked_len_i32(reader.read_i32()?, reader.as_slice().len(), 8)?;
        Ok(reader.read_i64_array(len)?)
    }
    #[inline]
    fn read_any(reader: &mut ByteReader, type_id: u8, depth: usize) -> io::Result<NbtValue> {
        ensure_depth(depth)?;
        match type_id {
            1 => Ok(NbtValue::Byte(reader.read_i8()?)),
            2 => Ok(NbtValue::Short(reader.read_i16()?)),
            3 => Ok(NbtValue::Int(reader.read_i32()?)),
            4 => Ok(NbtValue::Long(reader.read_i64()?)),
            5 => Ok(NbtValue::Float(reader.read_f32()?)),
            6 => Ok(NbtValue::Double(reader.read_f64()?)),
            7 => Ok(NbtValue::ByteArray(Self::read_i8_array(reader)?)),
            8 => Ok(NbtValue::String(Self::read_string(reader)?)),
            9 => Ok(NbtValue::List(Self::read_list(reader, depth + 1)?)),
            10 => Ok(NbtValue::Compound(Self::read_compound(
                reader,
                None,
                depth + 1,
            )?)),
            11 => Ok(NbtValue::IntArray(Self::read_i32_array(reader)?)),
            12 => Ok(NbtValue::LongArray(Self::read_i64_array(reader)?)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid NBT type: {:?}", type_id),
            )),
        }
    }
    #[inline]
    fn read_list(reader: &mut ByteReader, depth: usize) -> io::Result<Vec<NbtValue>> {
        ensure_depth(depth)?;
        let type_id = reader.read_u8()?;
        // Local NBT spec: list length is big-endian int32.
        let len = checked_len_i32(
            reader.read_i32()?,
            reader.as_slice().len(),
            tag_min_payload_bytes(type_id),
        )?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(Self::read_any(reader, type_id, depth + 1)?);
        }
        Ok(list)
    }

    #[inline]
    fn read_compound(
        reader: &mut ByteReader,
        name: Option<String>,
        depth: usize,
    ) -> io::Result<CompoundNbt> {
        ensure_depth(depth)?;
        let mut map = HashMap::new();
        loop {
            let type_id = reader.read_u8()?;
            if type_id == 0 {
                break;
            }
            let key = Self::read_string(reader)?;
            let value = Self::read_any(reader, type_id, depth + 1)?;
            map.insert(key, value);
        }
        Ok(CompoundNbt::from_map(name, map))
    }
}

impl NbtReadTrait for JavaLocalNbt {
    fn read(reader: &mut ByteReader) -> io::Result<NbtValue> {
        match reader.read_u8()? {
            9 => {
                let _name = Self::read_string(reader)?;
                Ok(NbtValue::List(Self::read_list(reader, 0)?))
            }
            10 => {
                let name = Self::read_string(reader)?;
                Ok(NbtValue::Compound(Self::read_compound(
                    reader,
                    Some(name),
                    0,
                )?))
            }
            x => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid NBT type: {:?}", x),
            )),
        }
    }
}

impl JavaLocalNbt {
    #[inline]
    fn write_string(writer: &mut ByteWriter, string: &str) -> io::Result<()> {
        let bytes = string.as_bytes();
        if bytes.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "NBT string too long",
            ));
        }
        writer.write_u16(bytes.len() as u16)?;
        writer.write_raw(bytes)
    }

    #[inline]
    fn write_any(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()> {
        match value {
            NbtValue::Byte(v) => writer.write_i8(*v),
            NbtValue::Short(v) => writer.write_i16(*v),
            NbtValue::Int(v) => writer.write_i32(*v),
            NbtValue::Long(v) => writer.write_i64(*v),
            NbtValue::Float(v) => writer.write_f32(*v),
            NbtValue::Double(v) => writer.write_f64(*v),
            NbtValue::ByteArray(v) => {
                writer.write_i32(v.len() as i32)?;
                writer.write_raw(bytemuck_slice(v))
            }
            NbtValue::String(v) => Self::write_string(writer, v),
            NbtValue::List(v) => Self::write_list(writer, v),
            NbtValue::Compound(v) => Self::write_compound_content(writer, v),
            NbtValue::IntArray(v) => {
                writer.write_i32(v.len() as i32)?;
                for item in v {
                    writer.write_i32(*item)?;
                }
                Ok(())
            }
            NbtValue::LongArray(v) => {
                writer.write_i32(v.len() as i32)?;
                for item in v {
                    writer.write_i64(*item)?;
                }
                Ok(())
            }
        }
    }

    #[inline]
    fn write_list(writer: &mut ByteWriter, values: &[NbtValue]) -> io::Result<()> {
        if values.is_empty() {
            writer.write_u8(1)?;
            writer.write_i32(0)?;
            return Ok(());
        }
        let tag = values[0].tag();
        if !values.iter().all(|value| value.tag() == tag) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "List tag not same",
            ));
        }
        writer.write_u8(tag)?;
        writer.write_i32(values.len() as i32)?;
        for value in values {
            Self::write_any(writer, value)?;
        }
        Ok(())
    }

    #[inline]
    fn write_compound_content(writer: &mut ByteWriter, compound: &CompoundNbt) -> io::Result<()> {
        for (key, value) in compound.iter() {
            writer.write_u8(value.tag())?;
            Self::write_string(writer, key)?;
            Self::write_any(writer, value)?;
        }
        writer.write_u8(0)
    }
}

impl NbtWriteTrait for JavaLocalNbt {
    fn write(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()> {
        let NbtValue::Compound(compound) = value else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Java local NBT root must be a Compound",
            ));
        };
        writer.write_u8(10)?;
        Self::write_string(writer, compound.name.as_deref().unwrap_or(""))?;
        Self::write_compound_content(writer, compound)
    }
}
