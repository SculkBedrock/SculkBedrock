//! Bedrock network NBT (`BedrockNetworkNbt`): varint-length strings,
//! zigzag varint / LE numbers. Used for NBT embedded in network packets (block entities, items, etc.).
//!
//! Also contains `JavaNetworkNbt` (BE strings + BE numbers) and network/local conversion helpers.

use crate::compound::CompoundNbt;
use crate::reader::NbtReadTrait;
use crate::writer::NbtWriteTrait;
use crate::NbtValue;
use std::io;
use sc_binary::{ByteReader, ByteWriter};

pub struct BedrockNetworkNbt;

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
        3 => 1,
        4 => 1,
        5 => 4,
        6 => 8,
        7 => 1,
        8 => 1,
        9 => 2,
        10 => 1,
        11 => 1,
        12 => 1,
        _ => 1,
    }
}

/// Read an unsigned varint and zigzag-decode it to i32.
/// The logical right shift must happen on the unsigned value: shifting after
/// casting to i32 would be an arithmetic shift and would sign-extend raw values
/// with the high bit set (|value| >= 2^30) into wrong results.
#[inline]
fn read_zigzag_var_i32(reader: &mut ByteReader) -> io::Result<i32> {
    let raw = reader.read_var_u32()?;
    Ok(((raw >> 1) as i32) ^ -((raw & 1) as i32))
}

/// Read an unsigned varint and zigzag-decode it to i64 (same logical-shift rule as above).
#[inline]
fn read_zigzag_var_i64(reader: &mut ByteReader) -> io::Result<i64> {
    let raw = reader.read_var_u64()?;
    Ok(((raw >> 1) as i64) ^ -((raw & 1) as i64))
}

impl BedrockNetworkNbt {
    /// Write a string: unsigned varint length + UTF-8 bytes.
    #[inline]
    fn write_string(writer: &mut ByteWriter, string: &str) -> io::Result<()> {
        writer.write_var_u32(string.len() as u32)?;
        writer.write(string.as_bytes())?;
        Ok(())
    }

    /// Write a ByteArray: zigzag varint length + raw bytes.
    #[inline]
    fn write_i8_array(writer: &mut ByteWriter, array: &Vec<i8>) -> io::Result<()> {
        writer.write_var_i32(array.len() as i32)?;
        writer.write(
            array
                .iter()
                .map(|x| *x as u8)
                .collect::<Vec<u8>>()
                .as_slice(),
        )?;
        Ok(())
    }

    /// Write an IntArray: zigzag varint length + one zigzag varint per element.
    #[inline]
    fn write_i32_array(writer: &mut ByteWriter, array: &Vec<i32>) -> io::Result<()> {
        writer.write_var_i32(array.len() as i32)?;
        for v in array {
            writer.write_var_i32(*v)?;
        }
        Ok(())
    }

    /// Write a LongArray: zigzag varint length + one zigzag varlong per element.
    fn write_i64_array(writer: &mut ByteWriter, array: &Vec<i64>) -> io::Result<()> {
        writer.write_var_i32(array.len() as i32)?;
        for v in array {
            writer.write_var_i64(*v)?;
        }
        Ok(())
    }

    /// Write an NBT value payload (no tag type or name).
    #[inline]
    fn write_any(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()> {
        match value {
            // TAG_Byte: fixed 1 byte
            NbtValue::Byte(x) => writer.write_i8(*x)?,
            // TAG_Short: fixed 2 bytes LE
            NbtValue::Short(x) => writer.write_i16_le(*x)?,
            // TAG_Int: zigzag varint in network mode
            NbtValue::Int(x) => writer.write_var_i32(*x)?,
            // TAG_Long: zigzag varlong in network mode
            NbtValue::Long(x) => writer.write_var_i64(*x)?,
            // TAG_Float: fixed 4 bytes LE
            NbtValue::Float(x) => writer.write_f32_le(*x)?,
            // TAG_Double: fixed 8 bytes LE
            NbtValue::Double(x) => writer.write_f64_le(*x)?,
            // TAG_Byte_Array
            NbtValue::ByteArray(x) => Self::write_i8_array(writer, &x)?,
            // TAG_Int_Array
            NbtValue::IntArray(x) => Self::write_i32_array(writer, &x)?,
            // TAG_Long_Array
            NbtValue::LongArray(x) => Self::write_i64_array(writer, &x)?,
            // TAG_String: writeUTF (unsigned varint length + UTF-8 bytes)
            NbtValue::String(x) => Self::write_string(writer, &x)?,
            // TAG_List
            NbtValue::List(x) => Self::write_list(writer, &x)?,
            // TAG_Compound: content only (entries + end marker); the parent writes the name
            NbtValue::Compound(data) => Self::write_compound_content(writer, &data)?,
        }
        Ok(())
    }

    /// Write a List: element type (u8) + zigzag varint length + element payloads.
    #[inline]
    fn write_list(writer: &mut ByteWriter, vec: &Vec<NbtValue>) -> io::Result<()> {
        if vec.is_empty() {
            // Empty lists default to element type TAG_Byte(1)
            writer.write_u8(1)?;
            writer.write_var_i32(0)?;
            return Ok(());
        }
        let tag = vec.first().unwrap().tag();
        if !vec.iter().all(|x| x.tag() == tag) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "List tag not same",
            ));
        }
        writer.write_u8(tag)?; // Element type
        writer.write_var_i32(vec.len() as i32)?; // Length as zigzag varint
        for i in vec {
            Self::write_any(writer, i)?;
        }
        Ok(())
    }

    /// Write compound content (entries + end marker) without the name.
    #[inline]
    fn write_compound_content(writer: &mut ByteWriter, compound: &CompoundNbt) -> io::Result<()> {
        for (key, value) in compound.iter() {
            writer.write_u8(value.tag())?; // tag type (via writeNamedTag)
            Self::write_string(writer, key)?; // key name (via writeNamedTag -> writeUTF)
            Self::write_any(writer, value)?; // value payload
        }
        writer.write_u8(0)?; // TAG_End
        Ok(())
    }

    fn read_string(reader: &mut ByteReader) -> io::Result<String> {
        let len = reader.read_var_u32()? as usize;
        let vec = reader.read_bytes(len)?.to_vec();
        String::from_utf8(vec).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn read_i8_array(reader: &mut ByteReader) -> io::Result<Vec<i8>> {
        let len = checked_len_i32(read_zigzag_var_i32(reader)?, reader.as_slice().len(), 1)?;
        reader.read_i8_bytes(len)
    }

    fn read_i32_array(reader: &mut ByteReader) -> io::Result<Vec<i32>> {
        let len = checked_len_i32(read_zigzag_var_i32(reader)?, reader.as_slice().len(), 1)?;
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(read_zigzag_var_i32(reader)?);
        }
        Ok(vec)
    }

    fn read_i64_array(reader: &mut ByteReader) -> io::Result<Vec<i64>> {
        let len = checked_len_i32(read_zigzag_var_i32(reader)?, reader.as_slice().len(), 1)?;
        let mut vec = Vec::with_capacity(len);
        for _ in 0..len {
            vec.push(read_zigzag_var_i64(reader)?);
        }
        Ok(vec)
    }

    fn read_any(reader: &mut ByteReader, type_id: u8, depth: usize) -> io::Result<NbtValue> {
        ensure_depth(depth)?;
        Ok(match type_id {
            1 => NbtValue::Byte(reader.read_i8()?),
            2 => NbtValue::Short(reader.read_i16_le()?),
            3 => NbtValue::Int(read_zigzag_var_i32(reader)?),
            4 => NbtValue::Long(read_zigzag_var_i64(reader)?),
            5 => NbtValue::Float(reader.read_f32_le()?),
            6 => NbtValue::Double(reader.read_f64_le()?),
            7 => NbtValue::ByteArray(Self::read_i8_array(reader)?),
            8 => NbtValue::String(Self::read_string(reader)?),
            9 => NbtValue::List(Self::read_list(reader, depth + 1)?),
            10 => NbtValue::Compound(Self::read_compound_content(reader, depth + 1)?),
            11 => NbtValue::IntArray(Self::read_i32_array(reader)?),
            12 => NbtValue::LongArray(Self::read_i64_array(reader)?),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown tag id")),
        })
    }

    fn read_compound_content(reader: &mut ByteReader, depth: usize) -> io::Result<CompoundNbt> {
        ensure_depth(depth)?;
        let mut compound = CompoundNbt::new(None);
        loop {
            let tag_id = reader.read_u8()?;
            if tag_id == 0 {
                break;
            }
            let name = Self::read_string(reader)?;
            let value = Self::read_any(reader, tag_id, depth + 1)?;
            compound.insert(&name, value);
        }
        Ok(compound)
    }

    fn read_list(reader: &mut ByteReader, depth: usize) -> io::Result<Vec<NbtValue>> {
        ensure_depth(depth)?;
        let type_id = reader.read_u8()?;
        let len = checked_len_i32(
            read_zigzag_var_i32(reader)?,
            reader.as_slice().len(),
            tag_min_payload_bytes(type_id),
        )?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            let value = Self::read_any(reader, type_id, depth + 1)?;
            list.push(value);
        }
        Ok(list)
    }
}

impl NbtWriteTrait for BedrockNetworkNbt {
    /// Write the root tag: tag type (u8) + root name (string) + payload.
    fn write(writer: &mut ByteWriter, value: &NbtValue) -> io::Result<()> {
        match value {
            NbtValue::List(_) => {
                writer.write_u8(9)?; // TAG_List
                Self::write_string(writer, "")?; // Root name (empty string)
                Self::write_any(writer, value)
            }
            NbtValue::Compound(data) => {
                writer.write_u8(10)?; // TAG_Compound
                let name = data.name.as_deref().unwrap_or("");
                Self::write_string(writer, name)?; // Root name
                Self::write_any(writer, value)
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Root NBT must be Compound or List",
                ))
            }
        }
    }
}

impl NbtReadTrait for BedrockNetworkNbt {
    fn read(reader: &mut ByteReader) -> io::Result<NbtValue> {
        let tag_type = reader.read_u8()?;
        match tag_type {
            9 => {
                let _name = Self::read_string(reader)?; // Root name (ignored for List roots)
                Ok(NbtValue::List(Self::read_list(reader, 0)?))
            }
            10 => {
                let name = Self::read_string(reader)?; // Root name
                let mut compound = Self::read_compound_content(reader, 0)?;
                compound.name = Some(name);
                Ok(NbtValue::Compound(compound))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unknown root tag type",
            )),
        }
    }
}

pub struct JavaNetworkNbt;

impl JavaNetworkNbt {
    fn read_string(reader: &mut ByteReader) -> io::Result<String> {
        let len = reader.read_u16()? as usize;
        String::from_utf8(reader.read_bytes(len)?.to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    #[inline]
    fn read_i8_array(reader: &mut ByteReader) -> io::Result<Vec<i8>> {
        let len = checked_len_i32(reader.read_i32()?, reader.as_slice().len(), 1)?;
        reader.read_i8_bytes(len)
    }
    #[inline]
    fn read_i32_array(reader: &mut ByteReader) -> io::Result<Vec<i32>> {
        let len = checked_len_i32(reader.read_i32()?, reader.as_slice().len(), 4)?;
        reader.read_i32_array(len)
    }
    #[inline]
    fn read_i64_array(reader: &mut ByteReader) -> io::Result<Vec<i64>> {
        let len = checked_len_i32(reader.read_i32()?, reader.as_slice().len(), 8)?;
        reader.read_i64_array(len)
    }

    #[inline]
    fn read_any(reader: &mut ByteReader, type_id: u8, depth: usize) -> io::Result<NbtValue> {
        ensure_depth(depth)?;
        Ok(match type_id {
            1 => NbtValue::Byte(reader.read_i8()?),
            2 => NbtValue::Short(reader.read_i16()?),
            3 => NbtValue::Int(reader.read_i32()?),
            4 => NbtValue::Long(reader.read_i64()?),
            5 => NbtValue::Float(reader.read_f32()?),
            6 => NbtValue::Double(reader.read_f64()?),
            7 => NbtValue::ByteArray(Self::read_i8_array(reader)?),
            8 => NbtValue::String(Self::read_string(reader)?),
            9 => NbtValue::List(Self::read_list(reader, depth + 1)?),
            10 => NbtValue::Compound(Self::read_compound(reader, depth + 1)?),
            11 => NbtValue::IntArray(Self::read_i32_array(reader)?),
            12 => NbtValue::LongArray(Self::read_i64_array(reader)?),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown tag id")),
        })
    }
    #[inline]
    fn read_compound(reader: &mut ByteReader, depth: usize) -> io::Result<CompoundNbt> {
        ensure_depth(depth)?;
        let mut compound = CompoundNbt::new(None);
        loop {
            let tag_id = reader.read_u8()?;
            if tag_id == 0 {
                break;
            }
            let name = Self::read_string(reader)?;
            let value = Self::read_any(reader, tag_id, depth + 1)?;
            compound.insert(&name, value);
        }
        Ok(compound)
    }
    #[inline]
    fn read_list(reader: &mut ByteReader, depth: usize) -> io::Result<Vec<NbtValue>> {
        ensure_depth(depth)?;
        let type_id = reader.read_u8()?;
        let len = checked_len_i32(
            reader.read_i32()?,
            reader.as_slice().len(),
            tag_min_payload_bytes(type_id),
        )?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            let value = Self::read_any(reader, type_id, depth + 1)?;
            list.push(value);
        }
        Ok(list)
    }
}

impl NbtReadTrait for JavaNetworkNbt {
    fn read(reader: &mut ByteReader) -> io::Result<NbtValue> {
        match reader.read_u8()? {
            10 => Ok(NbtValue::Compound(Self::read_compound(reader, 0)?)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown type")),
        }
    }
}
