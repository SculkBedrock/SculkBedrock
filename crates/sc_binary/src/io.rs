//! `ByteReader` / `ByteWriter`: binary read/write over `bytes` (panic-free).
//!
//! Big/little-endian and varint/zigzag primitive read/write; the `BinaryIo` derive macro
//! generates struct/enum serialization on top of this module.

use crate::interfaces::{Reader, Writer};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::io::{Read, Seek, SeekFrom};
use std::{
    collections::VecDeque,
    io::{Error, IoSlice},
};
use sc_utils::game::gamerules::{GameRuleType, GameRules};
use sc_utils::game::structs::position::MinecraftPosition;
use uuid::Uuid;

pub const ERR_EOB: &str = "No more bytes left to be read in buffer";
pub const ERR_EOM: &str = "Buffer is full, cannot write more bytes";
pub const ERR_VARINT_TOO_LONG: &str = "Varint is too long to be written to buffer";

macro_rules! can_read {
    ($self: ident, $size: expr) => {
        $self.buf.remaining() >= $size
    };
}

macro_rules! can_write {
    ($self: ident, $size: expr) => {
        $self.buf.remaining_mut() >= $size
    };
}

macro_rules! read_fn {
    ($name: ident, $typ: ident, $fn_name: ident, $byte_size: literal) => {
        #[inline]
        pub fn $name(&mut self) -> Result<$typ, std::io::Error> {
            if can_read!(self, $byte_size) {
                self.pos += $byte_size;
                return Ok(self.buf.$fn_name());
            } else {
                return Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB));
            }
        }
    };
}

macro_rules! write_fn {
    ($name: ident, $typ: ident, $fn_name: ident, $byte_size: literal) => {
        #[inline]
        pub fn $name(&mut self, num: $typ) -> Result<(), std::io::Error> {
            if can_write!(self, $byte_size) {
                self.buf.$fn_name(num);
                return Ok(());
            } else {
                return Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM));
            }
        }
    };
}

/// ByteReader is a panic-free way to read bytes from the `byte::Buf` trait.
///
/// ## Example
/// ```rust
/// use sc_binary::io::ByteReader;
///
/// fn main() {
///    let mut buf = ByteReader::from(&[0, 253, 255, 255, 255, 15][..]);
///    assert_eq!(buf.read_u8().unwrap(), 0);
///    assert_eq!(buf.read_var_i32().unwrap(), -2147483647);
/// }
/// ```
///
/// ## Peek Ahead
/// `ByteReader` also provides a utility `peek_ahead` function that allows you to
/// "peek ahead" at the next byte in the stream without advancing the stream.
///
/// Do not confuse this with any sort of "peek" function. This function does not
/// increment the read position of the stream, but rather copies the byte at the
/// specified position.
/// ```rust
/// use sc_binary::io::ByteReader;
///
/// fn main() {
///    let mut buf = ByteReader::from(&[253, 255, 14, 255, 255, 15][..]);
///    if buf.peek_ahead(3).unwrap() != 255 {
///        // buffer is corrupted!
///    } else {
///        // read the varint
///        let num = buf.read_var_i32().unwrap();
///    }
/// }
/// ```
///
/// ## Reading a struct without `BinaryDecoder`
/// This is useful if you are trying to read a struct or optional type and validate the type before
/// reading the rest of the struct.
/// ```rust
/// use sc_binary::io::ByteReader;
///
/// struct PingPacket {
///    pub id: u8,
///    pub time: u64,
///    pub ack_id: Option<i32>
/// }
///
/// fn main() {
///     let mut buf = ByteReader::from(&[0, 253, 255, 255, 255, 255, 255, 255, 255, 0][..]);
///
///     // Read the id
///     let id = buf.read_u8().unwrap();
///
///     if id == 0 {
///         // Read the time
///        let time = buf.read_u64().unwrap();
///        // read ack
///        if buf.read_bool().unwrap() {
///            let ack_id = buf.read_var_i32().unwrap();
///            let packet = PingPacket { id, time, ack_id: Some(ack_id) };
///        } else {
///            let packet = PingPacket { id, time, ack_id: None };
///        }
///    }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ByteReader {
    pub(crate) raw_buf: Bytes,
    pub(crate) buf: Bytes,
    pub(crate) pos: usize,
}

impl From<ByteWriter> for ByteReader {
    fn from(writer: ByteWriter) -> Self {
        let bytes = writer.buf.freeze();
        Self {
            raw_buf: bytes.clone(),
            buf: bytes.clone(),
            pos: 0,
        }
    }
}

impl Into<Bytes> for ByteReader {
    fn into(self) -> Bytes {
        self.buf
    }
}

impl Into<Vec<u8>> for ByteReader {
    fn into(self) -> Vec<u8> {
        self.buf.to_vec()
    }
}

impl Into<VecDeque<u8>> for ByteReader {
    fn into(self) -> VecDeque<u8> {
        self.buf.to_vec().into()
    }
}

impl From<Bytes> for ByteReader {
    fn from(buf: Bytes) -> Self {
        Self {
            raw_buf: buf.clone(),
            buf,
            pos: 0,
        }
    }
}

impl From<Vec<u8>> for ByteReader {
    /// `Vec` to `Bytes` is **zero-copy** (`Bytes::from(Vec)` takes over the allocation).
    ///
    /// A `Vec` clone would heap-copy the whole buffer (palettes, entity ids, every subchunk),
    /// so construction avoids cloning entirely.
    fn from(buf: Vec<u8>) -> Self {
        let bytes = Bytes::from(buf);
        Self {
            // `Bytes::clone` only bumps an atomic refcount.
            raw_buf: bytes.clone(),
            buf: bytes,
            pos: 0,
        }
    }
}

impl From<&[u8]> for ByteReader {
    /// Copies via `to_vec()` exactly once.
    fn from(buf: &[u8]) -> Self {
        let bytes = Bytes::from(buf.to_vec());
        Self {
            raw_buf: bytes.clone(),
            buf: bytes,
            pos: 0,
        }
    }
}

impl Read for ByteReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read(buf)?;
        self.pos += buf.len();
        Ok(buf.len())
    }
}

impl Seek for ByteReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match pos {
            SeekFrom::Start(n) => {
                self.position(n as usize)?;
                Ok(n)
            }
            SeekFrom::End(n) => {
                let pos = (self.buf.len() as i64 + n) as usize;
                self.position(pos)?;
                Ok(pos as u64)
            }
            SeekFrom::Current(n) => {
                let pos = (self.pos as i64 + n) as usize;
                self.position(pos)?;
                Ok(pos as u64)
            }
        }
    }
}

impl ByteReader {
    /// `ByteReader` also provides a utility `peek_ahead` function that allows you to
    /// "peek ahead" at the next byte in the stream without advancing the stream.
    ///
    /// Do not confuse this with any sort of "peek" function. This function does not
    /// increment the read position of the stream, but rather copies the byte at the
    /// specified position.
    /// ```rust
    /// use sc_binary::io::ByteReader;
    ///
    /// fn main() {
    ///    let mut buf = ByteReader::from(&[253, 255, 14, 255, 255, 15][..]);
    ///    if buf.peek_ahead(3).unwrap() != 255 {
    ///        // buffer is corrupted, varints can never have a leading byte less than 255 if
    ///        // There are bytes remaining!
    ///    } else {
    ///        // read the varint
    ///        let num = buf.read_var_i32().unwrap();
    ///    }
    /// }
    /// ```

    pub fn peek_ahead(&mut self, pos: usize) -> Result<u8, Error> {
        // Reading the byte at index pos needs pos + 1 readable bytes;
        // otherwise indexing would panic when the buffer is exactly exhausted.
        if can_read!(self, pos + 1) {
            Ok(self.buf.chunk()[pos])
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn position(&mut self, pos: usize) -> Result<(), Error> {
        if pos <= self.raw_buf.len() {
            self.buf = self.raw_buf.slice(pos..);
            self.pos = pos;
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn advance(&mut self, len: usize) {
        self.buf.advance(len);
        self.pos += len;
    }

    read_fn!(read_u8, u8, get_u8, 1);
    read_fn!(read_i8, i8, get_i8, 1);
    read_fn!(read_u16, u16, get_u16, 2);
    read_fn!(read_u16_le, u16, get_u16_le, 2);
    read_fn!(read_i16, i16, get_i16, 2);
    read_fn!(read_i16_le, i16, get_i16_le, 2);

    /// Reads a 3-byte unsigned integer from the stream.
    pub fn read_u24(&mut self) -> Result<u32, Error> {
        if can_read!(self, 3) {
            if let Ok(num) = self.read_uint(3) {
                Ok(num as u32)
            } else {
                Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
            }
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    /// Reads a 3-byte unsigned integer from the stream in little endian.
    /// This is the same as `read_u24` but in little endian.
    pub fn read_u24_le(&mut self) -> Result<u32, Error> {
        if can_read!(self, 3) {
            if let Ok(num) = self.read_uint_le(3) {
                Ok(num as u32)
            } else {
                Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
            }
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_i24(&mut self) -> Result<i32, Error> {
        if can_read!(self, 3) {
            if let Ok(num) = self.read_int(3) {
                Ok(num as i32)
            } else {
                Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
            }
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_i24_le(&mut self) -> Result<i32, Error> {
        if can_read!(self, 3) {
            if let Ok(num) = self.read_int_le(3) {
                Ok(num as i32)
            } else {
                Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
            }
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    read_fn!(read_u32, u32, get_u32, 4);
    read_fn!(read_u32_le, u32, get_u32_le, 4);
    read_fn!(read_f32, f32, get_f32, 4);
    read_fn!(read_f32_le, f32, get_f32_le, 4);

    /// Reads a var-int 32-bit unsigned integer from the stream.
    /// This is a variable length integer that can be 1, 2, 3, or 4 bytes long.
    ///
    /// This function is recoverable, meaning that if the stream ends before the
    /// var-int is fully read, it will return an error, and will not consume the
    /// bytes that were read.
    #[inline]
    pub fn read_var_u32(&mut self) -> Result<u32, Error> {
        let mut num = 0u32;
        let mut interval = 0_usize;
        for i in (0..35).step_by(7) {
            let byte = self.peek_ahead(interval)?;

            num |= ((byte & 0x7F) as u32) << i;
            interval += 1;

            if byte & 0x80 == 0 {
                self.advance(interval);
                return Ok(num);
            }
        }
        Err(Error::new(
            std::io::ErrorKind::Other,
            "Varint overflow's 32-bit integer",
        ))
    }

    read_fn!(read_i32, i32, get_i32, 4);
    read_fn!(read_i32_le, i32, get_i32_le, 4);

    /// Reads a var-int 32-bit signed integer from the stream.
    /// Symmetric with `write_var_i32`: the raw LEB128 value is zigzag-decoded
    /// (Bedrock SignedVarInt).
    pub fn read_var_i32(&mut self) -> Result<i32, Error> {
        let mut value: u32 = 0;
        let mut size = 0u32;
        loop {
            // Guard length before shifting: 32-bit takes at most 5 bytes (shifts 0/7/14/21/28);
            // a 6th byte errors out instead of overflowing the shift in debug builds.
            if size >= 5 {
                return Err(Error::new(
                    std::io::ErrorKind::Other,
                    "Varint overflow's 32-bit integer",
                ));
            }
            let byte = self.read_u8()?;
            value |= ((byte & 0b0111_1111) as u32) << (size * 7);
            size += 1;
            if (byte & 0b1000_0000) == 0 {
                break;
            }
        }
        // zigzag decode
        Ok(((value >> 1) as i32) ^ -((value & 1) as i32))
    }

    read_fn!(read_u64, u64, get_u64, 8);
    read_fn!(read_u64_le, u64, get_u64_le, 8);
    read_fn!(read_i64, i64, get_i64, 8);
    read_fn!(read_i64_le, i64, get_i64_le, 8);
    read_fn!(read_f64, f64, get_f64, 8);
    read_fn!(read_f64_le, f64, get_f64_le, 8);

    /// Reads a var-int 64-bit unsigned integer from the stream.
    /// This is a variable length integer that can be 1, 2, 3, 4, 5, 6, 7, or 8 bytes long.
    #[inline]
    pub fn read_var_u64(&mut self) -> Result<u64, Error> {
        let mut num = 0u64;
        let mut interval = 0_usize;
        for i in (0..70).step_by(7) {
            let byte = self.peek_ahead(interval)?;

            num |= ((byte & 0x7F) as u64) << i;
            interval += 1;

            if byte & 0x80 == 0 {
                self.advance(interval);
                return Ok(num);
            }
        }
        Err(Error::new(
            std::io::ErrorKind::Other,
            "Varint overflow's 64-bit integer",
        ))
    }

    /// Reads a var-int 64-bit signed integer from the stream.
    /// This method is the same as `read_var_u64` but it will return a signed integer.
    ///
    /// For more information on how this works, see `read_var_i32`.
    #[inline]
    /// Reads a var-int 64-bit signed integer from the stream.
    /// Symmetric with `write_var_i64`: the raw LEB128 value is zigzag-decoded
    /// (Bedrock SignedVarLong).
    pub fn read_var_i64(&mut self) -> Result<i64, Error> {
        let mut value: u64 = 0;
        let mut size = 0u32;
        loop {
            // 64-bit takes at most 10 bytes (shifts 0..=63); an 11th byte errors out
            // instead of overflowing the shift in debug builds.
            if size >= 10 {
                return Err(Error::new(
                    std::io::ErrorKind::Other,
                    "Varint overflow's 64-bit integer",
                ));
            }
            let byte = self.read_u8()?;
            value |= ((byte & 0b0111_1111) as u64) << (size * 7);
            size += 1;
            if (byte & 0b1000_0000) == 0 {
                break;
            }
        }
        // zigzag decode
        Ok(((value >> 1) as i64) ^ -((value & 1) as i64))
    }

    read_fn!(read_u128, u128, get_u128, 16);
    read_fn!(read_u128_le, u128, get_u128_le, 16);
    read_fn!(read_i128, i128, get_i128, 16);
    read_fn!(read_i128_le, i128, get_i128_le, 16);

    /// Reads an unsigned integer from the stream with a varying size
    /// indicated by the `size` parameter.
    pub fn read_uint(&mut self, size: usize) -> Result<u64, Error> {
        if can_read!(self, size) {
            Ok(self.buf.get_uint(size))
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    /// Reads an unsigned integer from the stream with a varying size in little endian
    /// indicated by the `size` parameter.
    pub fn read_uint_le(&mut self, size: usize) -> Result<u64, Error> {
        if can_read!(self, size) {
            Ok(self.buf.get_uint_le(size))
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_int(&mut self, size: usize) -> Result<i64, Error> {
        if can_read!(self, size) {
            Ok(self.buf.get_int(size))
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_int_le(&mut self, size: usize) -> Result<i64, Error> {
        if can_read!(self, size) {
            Ok(self.buf.get_int_le(size))
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_char(&mut self) -> Result<char, Error> {
        let c = self.read_u32()?;

        if let Some(c) = char::from_u32(c) {
            Ok(c)
        } else {
            Err(Error::new(std::io::ErrorKind::InvalidData, "Invalid char"))
        }
    }

    pub fn read_bool(&mut self) -> Result<bool, Error> {
        if can_read!(self, 1) {
            Ok(self.buf.get_u8() != 0)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    /// Reads a string from the stream.
    /// This is a reversible operation, meaning if it fails,
    /// the stream will be in the same state as before.
    pub fn read_string(&mut self) -> Result<String, Error> {
        // todo: Make this reversible
        let len = self.read_var_u64()?;
        if len > usize::MAX as u64 {
            return Err(Error::new(
                std::io::ErrorKind::InvalidData,
                "String length overflows usize",
            ));
        }
        let len = len as usize;
        if !can_read!(self, len) {
            return Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB));
        }
        let bytes = self.read_bytes(len)?.to_vec();
        String::from_utf8(bytes).map_err(|e| Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Reads an `Option` of `T` from the stream.
    /// `T` must implement the `Reader` trait and be sized.
    ///
    /// This operation is not recoverable and will corrupt the stream if it fails.
    /// If this behavior is desired, you should use `peek_ahead` when implementing
    /// the `Reader` trait.
    ///
    /// # Example
    /// ```rust
    /// use sc_binary::io::ByteReader;
    /// use sc_binary::interfaces::Reader;
    ///
    /// pub struct HelloWorld {
    ///     pub magic: u32
    /// }
    ///
    /// impl Reader<HelloWorld> for HelloWorld {
    ///     fn read(reader: &mut ByteReader) -> Result<HelloWorld, std::io::Error> {
    ///         Ok(HelloWorld {
    ///             magic: reader.read_u32()?
    ///         })
    ///     }
    /// }
    ///
    /// fn main() {
    ///     // Nothing is here!
    ///     let mut reader = ByteReader::from(&[0x00][..]);
    ///     let hello_world = reader.read_option::<HelloWorld>().unwrap();
    ///     assert_eq!(hello_world.is_some(), false);
    /// }
    /// ```
    pub fn read_option<T: Reader<T>>(&mut self) -> Result<Option<T>, Error> {
        if self.read_bool()? {
            Ok(Some(T::read(self)?))
        } else {
            Ok(None)
        }
    }

    /// Reads a varu32 sized slice from the stream.
    /// For reading a slice of raw bytes, use `read` instead.
    pub fn read_sized_slice(&mut self) -> Result<Bytes, Error> {
        let len = self.read_var_u32()?;

        if can_read!(self, len as usize) {
            let b = self.buf.slice(..len as usize);
            self.advance(len as usize);
            Ok(b)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_bytes(&mut self, len: usize) -> Result<Bytes, Error> {
        if can_read!(self, len) {
            let b = self.buf.slice(..len);
            self.advance(len);
            Ok(b)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    /// Reads a UUID: LE u64 pair (mirrors `ByteWriter::write_uuid`, 16 bytes).
    pub fn read_uuid(&mut self) -> Result<Uuid, Error> {
        let most = self.read_u64_le()?;
        let least = self.read_u64_le()?;
        Ok(Uuid::from_u64_pair(most, least))
    }

    pub fn read_i8_bytes(&mut self, len: usize) -> Result<Vec<i8>, Error> {
        if can_read!(self, len) {
            let b = self.buf.slice(..len);
            let b = b.iter().map(|&n| n as i8).collect();
            self.advance(len);
            Ok(b)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_i32_le_array(&mut self, len: usize) -> Result<Vec<i32>, Error> {
        let vec_len = len.checked_mul(4).ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "i32 array length overflows usize",
            )
        })?;
        if can_read!(self, vec_len) {
            let b = self.buf.slice(..vec_len);

            let mut vec = Vec::with_capacity(len);
            for i in (0..vec_len).step_by(4) {
                let mut array = [0u8; 4];
                array.copy_from_slice(&b[i..i + 4]);
                vec.push(i32::from_le_bytes(array));
            }

            self.advance(vec_len);
            Ok(vec)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_i32_array(&mut self, len: usize) -> Result<Vec<i32>, Error> {
        let vec_len = len.checked_mul(4).ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "i32 array length overflows usize",
            )
        })?;
        if can_read!(self, vec_len) {
            let b = self.buf.slice(..vec_len);

            let mut vec = Vec::with_capacity(len);
            for i in (0..vec_len).step_by(4) {
                let mut array = [0u8; 4];
                array.copy_from_slice(&b[i..i + 4]);
                vec.push(i32::from_be_bytes(array));
            }

            self.advance(vec_len);
            Ok(vec)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_i64_le_array(&mut self, len: usize) -> Result<Vec<i64>, Error> {
        let vec_len = len.checked_mul(8).ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "i64 array length overflows usize",
            )
        })?;
        if can_read!(self, vec_len) {
            let b = self.buf.slice(..vec_len);

            let mut vec = Vec::with_capacity(len);
            for i in (0..vec_len).step_by(8) {
                let mut array = [0u8; 8];
                array.copy_from_slice(&b[i..i + 8]);
                vec.push(i64::from_le_bytes(array));
            }

            self.advance(vec_len);
            Ok(vec)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    pub fn read_i64_array(&mut self, len: usize) -> Result<Vec<i64>, Error> {
        let vec_len = len.checked_mul(8).ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "i64 array length overflows usize",
            )
        })?;
        if can_read!(self, vec_len) {
            let b = self.buf.slice(..vec_len);

            let mut vec = Vec::with_capacity(len);
            for i in (0..vec_len).step_by(8) {
                let mut array = [0u8; 8];
                array.copy_from_slice(&b[i..i + 8]);
                vec.push(i64::from_be_bytes(array));
            }

            self.advance(vec_len);
            Ok(vec)
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    /// Reads a slice from the stream into the slice passed by the caller.
    /// For reading a prefixed sized slice, use `read_sized_slice` instead.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<(), Error> {
        if can_read!(self, buffer.len()) {
            self.buf.copy_to_slice(buffer);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::UnexpectedEof, ERR_EOB))
        }
    }

    /// Reads `T` from the stream.
    /// `T` must implement the `Reader` trait and be sized.
    ///
    /// # Deprecated
    ///
    /// This function is deprecated and will be removed in `v0.3.4`.
    #[deprecated(note = "Use `read_type` instead")]
    pub fn read_struct<T: Reader<T>>(&mut self) -> Result<T, Error> {
        self.read_type::<T>()
    }

    /// Reads `T` from the stream.
    /// `T` must implement the `Reader` trait and be sized.
    pub fn read_type<T: Reader<T>>(&mut self) -> Result<T, Error> {
        T::read(self)
    }

    /// Returns the remaining bytes in the stream.
    pub fn as_slice(&self) -> &[u8] {
        self.buf.chunk()
    }
}

/// ByteWriter is a panic-free way to write bytes to a `BufMut` trait.
///
/// ## Example
/// A generic example of how to use the `ByteWriter` struct.
/// ```rust
/// use sc_binary::io::ByteWriter;
/// use sc_binary::io::ByteReader;
///
/// fn main() {
///    let mut writer = ByteWriter::new();
///    writer.write_string("Hello World!").unwrap();
///    writer.write_var_u32(65536).unwrap();
///    writer.write_u8(0).unwrap();
///
///    println!("Bytes: {:?}", writer.as_slice());
/// }
/// ```
///
/// `ByteWriter` also implements the `Into` trait to convert the `ByteWriter` into a `BytesMut` or `Bytes` structs.
/// ```rust
/// use sc_binary::io::ByteWriter;
/// use sc_binary::io::ByteReader;
///
/// fn main() {
///     let mut writer = ByteWriter::new();
///     let _ = writer.write_u8(1);
///     let _ = writer.write_u8(2);
///     let _ = writer.write_u8(3);
///
///     let mut reader: ByteReader = writer.into();
///     assert_eq!(reader.read_u8().unwrap(), 1);
///     assert_eq!(reader.read_u8().unwrap(), 2);
///     assert_eq!(reader.read_u8().unwrap(), 3);
/// }
/// ```
///
/// #### ByteWriter Implementation Notice
/// While most of the methods are reversible, some are not.
/// Meaning there is a chance that if you call a method in a edge case, it will corrupt the stream.
///
/// For example, `write_var_u32` is not reversible because we currently do not
/// allocate a buffer to store the bytes before writing them to the buffer.
/// While you should never encounter this issue, it is possible when you run out of memory.
/// This issue is marked as a todo, but is low priority.
#[derive(Debug, Clone)]
pub struct ByteWriter {
    pub(crate) buf: BytesMut,
}

impl Into<BytesMut> for ByteWriter {
    fn into(self) -> BytesMut {
        self.buf
    }
}

impl Into<Bytes> for ByteWriter {
    fn into(self) -> Bytes {
        self.buf.freeze()
    }
}

impl Into<Vec<u8>> for ByteWriter {
    fn into(self) -> Vec<u8> {
        self.buf.to_vec()
    }
}

impl Into<VecDeque<u8>> for ByteWriter {
    fn into(self) -> VecDeque<u8> {
        self.buf.to_vec().into()
    }
}

impl From<IoSlice<'_>> for ByteWriter {
    fn from(slice: IoSlice) -> Self {
        let mut buf = BytesMut::with_capacity(slice.len());
        buf.put_slice(&slice);
        Self { buf }
    }
}

impl From<&[u8]> for ByteWriter {
    fn from(slice: &[u8]) -> Self {
        let mut buf = BytesMut::with_capacity(slice.len());
        buf.put_slice(slice);
        Self { buf }
    }
}

impl From<ByteReader> for ByteWriter {
    fn from(reader: ByteReader) -> Self {
        Self {
            buf: reader.buf.chunk().into(),
        }
    }
}

impl ByteWriter {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::new(),
        }
    }

    write_fn!(write_u8, u8, put_u8, 1);
    write_fn!(write_i8, i8, put_i8, 1);
    write_fn!(write_u16, u16, put_u16, 2);
    write_fn!(write_u16_le, u16, put_u16_le, 2);
    write_fn!(write_i16, i16, put_i16, 2);
    write_fn!(write_i16_le, i16, put_i16_le, 2);

    pub fn write_u24<I: Into<u32>>(&mut self, num: I) -> Result<(), Error> {
        self.write_uint(num.into().into(), 3)
    }

    pub fn write_u24_le<I: Into<u32>>(&mut self, num: I) -> Result<(), Error> {
        self.write_uint_le(num.into().into(), 3)
    }

    pub fn write_i24<I: Into<i32>>(&mut self, num: I) -> Result<(), Error> {
        self.write_int(num.into().into(), 3)
    }

    pub fn write_i24_le<I: Into<i32>>(&mut self, num: I) -> Result<(), Error> {
        self.write_int_le(num.into().into(), 3)
    }

    write_fn!(write_u32, u32, put_u32, 4);
    write_fn!(write_u32_le, u32, put_u32_le, 4);
    write_fn!(write_i32, i32, put_i32, 4);
    write_fn!(write_i32_le, i32, put_i32_le, 4);
    write_fn!(write_f32, f32, put_f32, 4);
    write_fn!(write_f32_le, f32, put_f32_le, 4);

    // todo: write_var_u32, write_var_i32 should be reversible and should not corrupt the stream on failure
    pub fn write_var_u32(&mut self, num: u32) -> Result<(), Error> {
        let mut x = num;
        while x >= 0x80 {
            self.write_u8((x as u8) | 0x80)?;
            x >>= 7;
        }
        self.write_u8(x as u8)?;
        Ok(())
    }

    pub fn write_var_i32(&mut self, num: i32) -> Result<(), Error> {
        if num < 0 {
            let num = num as u32;
            self.write_var_u32(!(num << 1))
        } else {
            let num = num as u32;
            self.write_var_u32(num << 1)
        }
    }

    write_fn!(write_u64, u64, put_u64, 8);
    write_fn!(write_u64_le, u64, put_u64_le, 8);
    write_fn!(write_i64, i64, put_i64, 8);
    write_fn!(write_i64_le, i64, put_i64_le, 8);
    write_fn!(write_f64, f64, put_f64, 8);
    write_fn!(write_f64_le, f64, put_f64_le, 8);

    pub fn write_var_u64(&mut self, num: u64) -> Result<(), Error> {
        let mut x = (num) & u64::MAX;
        for _ in (0..70).step_by(7) {
            if x >> 7 == 0 {
                self.write_u8(x as u8)?;
                return Ok(());
            } else {
                self.write_u8(((x & 0x7F) | 0x80) as u8)?;
                x >>= 7;
            }
        }

        Err(Error::new(
            std::io::ErrorKind::InvalidData,
            ERR_VARINT_TOO_LONG,
        ))
    }

    pub fn write_var_i64(&mut self, num: i64) -> Result<(), Error> {
        if num < 0 {
            let num = num as u64;
            self.write_var_u64(!(num << 1))
        } else {
            let num = num as u64;
            self.write_var_u64(num << 1)
        }
    }

    write_fn!(write_u128, u128, put_u128, 16);
    write_fn!(write_u128_le, u128, put_u128_le, 16);
    write_fn!(write_i128, i128, put_i128, 16);
    write_fn!(write_i128_le, i128, put_i128_le, 16);

    pub fn write_uint(&mut self, num: u64, size: usize) -> Result<(), Error> {
        if can_write!(self, size) {
            self.buf.put_uint(num, size);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_uint_le(&mut self, num: u64, size: usize) -> Result<(), Error> {
        if can_write!(self, size) {
            self.buf.put_uint_le(num, size);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_int(&mut self, num: i64, size: usize) -> Result<(), Error> {
        if can_write!(self, size) {
            self.buf.put_int(num, size);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_int_le(&mut self, num: i64, size: usize) -> Result<(), Error> {
        if can_write!(self, size) {
            self.buf.put_int_le(num, size);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_char(&mut self, c: char) -> Result<(), Error> {
        self.write_u32(c as u32)
    }

    pub fn write_bool(&mut self, b: bool) -> Result<(), Error> {
        if can_write!(self, 1) {
            self.buf.put_u8(b as u8);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    /// Write a string to the buffer
    /// The string is written as a var_u32 length followed by the bytes of the string.
    /// Uses <https://protobuf.dev/programming-guides/encoding/#length-types> for length encoding
    pub fn write_string(&mut self, string: &str) -> Result<(), Error> {
        // https://protobuf.dev/programming-guides/encoding/#length-types
        if can_write!(self, string.len()) {
            self.write_var_u32(string.len() as u32)?;
            self.buf.put_slice(string.as_bytes());
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    /// Writes an `Option` to the buffer. The option must implement the `Writer` trait.
    ///
    /// ## Example
    /// ```rust
    /// use sc_binary::io::ByteWriter;
    /// use sc_binary::interfaces::Writer;
    ///
    /// pub struct HelloWorld {
    ///     pub magic: u32
    /// }
    ///
    /// impl Writer for HelloWorld {
    ///     fn write(&self, buf: &mut ByteWriter) -> Result<(), std::io::Error> {
    ///         buf.write_u32(self.magic)?;
    ///         Ok(())
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let hello = HelloWorld { magic: 0xCAFEBABE };
    ///     let mut buf = hello.write_to_bytes().unwrap();
    ///
    ///     println!("Hello World: {:?}", buf);
    /// }
    /// ```
    pub fn write_option(&mut self, option: &Option<impl Writer>) -> Result<(), Error> {
        if let Some(option) = option {
            self.write_bool(true)?;
            option.write(self)?;
        } else {
            self.write_bool(false)?;
        }
        Ok(())
    }

    /// Writes raw bytes to the buffer without any length prefix.
    pub fn write_raw(&mut self, slice: &[u8]) -> Result<(), Error> {
        if can_write!(self, slice.len()) {
            self.buf.put_slice(slice);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    /// Writes a size-prefixed slice of bytes to the buffer. The slice is prefixed with a var_u32 length.
    pub fn write_slice(&mut self, slice: &[u8]) -> Result<(), Error> {
        if can_write!(self, slice.len()) {
            self.write_var_u32(slice.len() as u32)?;
            self.buf.put_slice(slice);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_position_f32(&mut self, position: MinecraftPosition) -> Result<(), Error> {
        self.write_position_vec_f32(position.to_vec())
    }

    pub fn write_position_vec_f32(&mut self, vec: Vec<f32>) -> Result<(), Error> {
        if can_write!(self, 12) {
            self.write_f32_le(vec[0])?;
            self.write_f32_le(vec[1])?;
            self.write_f32_le(vec[2])?;
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_position_i32(&mut self, position: MinecraftPosition) -> Result<(), Error> {
        self.write_position_vec_i32(position.to_vec_i32())
    }

    pub fn write_position_vec_i32(&mut self, vec: Vec<i32>) -> Result<(), Error> {
        if can_write!(self, 12) {
            self.write_var_i32(vec[0])?;
            self.write_var_i32(vec[1])?;
            self.write_var_i32(vec[2])?;
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_uuid(&mut self, uuid: &Uuid) -> Result<(), Error> {
        if can_write!(self, 16) {
            let (most, least) = uuid.as_u64_pair();
            self.write_u64_le(most)?;
            self.write_u64_le(least)?;
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    pub fn write_game_rules(&mut self, gamerules: &GameRules) -> Result<(), Error> {
        self.write_game_rules_with_mode(gamerules, true)
    }

    /// Writes game rules with a specified mode.
    /// When `start_game` is true (used in StartGame packet), INTEGER rules are encoded as zigzag varint.
    /// When `start_game` is false (used in GameRuleChanged packet), INTEGER rules are encoded as LInt (4 bytes LE).
    pub fn write_game_rules_with_mode(
        &mut self,
        gamerules: &GameRules,
        _start_game: bool,
    ) -> Result<(), Error> {
        self.write_var_u32(gamerules.len() as u32)?;
        for (key, value) in gamerules.iter() {
            // Game rule names encode lowercased.
            self.write_string(&key.name().to_lowercase())?;
            self.write_bool(value.can_be_changed)?;
            // Type index encodes as unsigned varint.
            self.write_var_u32(value.value.index() as u32)?;
            match &value.value {
                GameRuleType::Unknown => {}
                GameRuleType::Bool(v) => {
                    self.write_bool(*v)?;
                }
                // Integer game rules always encode as little-endian i32, for both
                // StartGame and GameRulesChanged, never as zigzag varint.
                GameRuleType::Int(v) => {
                    self.write_i32_le(*v)?;
                }
                GameRuleType::Float(v) => {
                    self.write_f32_le(*v)?;
                }
            }
        }
        Ok(())
    }

    pub fn write_utf8(&mut self, string: &String) -> Result<(), Error> {
        let len = i16::try_from(string.len()).map_err(|_| {
            Error::new(
                std::io::ErrorKind::InvalidData,
                "UTF-8 string is too long for an i16 length prefix",
            )
        })?;
        self.write_i16_le(len)?;
        self.write_raw(string.as_bytes())
    }

    /// Writes a slice of bytes to the buffer
    /// This is not the same as a size-prefixed slice, this is just a raw slice of bytes.
    ///
    /// For automatically size-prefixed slices, use `write_slice`.
    pub fn write(&mut self, buf: &[u8]) -> Result<(), Error> {
        if can_write!(self, buf.len()) {
            self.buf.put_slice(buf);
            Ok(())
        } else {
            Err(Error::new(std::io::ErrorKind::OutOfMemory, ERR_EOM))
        }
    }

    /// Writes `T` to the buffer. `T` must implement the `Writer` trait.
    /// This is the same as calling `T.write(self)`.
    /// ```rust
    /// use sc_binary::interfaces::{Reader, Writer};
    /// use sc_binary::io::{ByteReader, ByteWriter};
    ///
    /// pub struct HelloPacket {
    ///     pub name: String,
    ///     pub age: u8,
    ///     pub is_cool: bool,
    ///     pub friends: Vec<String>,
    /// }
    ///
    /// impl Reader<HelloPacket> for HelloPacket {
    ///     fn read(buf: &mut ByteReader) -> std::io::Result<Self> {
    ///         Ok(Self {
    ///             name: buf.read_string()?,
    ///             age: buf.read_u8()?,
    ///             is_cool: buf.read_bool()?,
    ///             friends: Vec::<String>::read(buf)?
    ///         })
    ///     }
    /// }
    ///
    /// impl Writer for HelloPacket {
    ///     fn write(&self, buf: &mut ByteWriter) -> std::io::Result<()> {
    ///         let _ = buf.write_string(&self.name);
    ///         let _ = buf.write_u8(self.age);
    ///         let _ = buf.write_bool(self.is_cool);
    ///         self.friends.write(buf)?;
    ///         Ok(())
    ///     }
    /// }
    ///
    /// fn main() {
    ///     let mut buf = ByteWriter::new();
    ///     let packet = HelloPacket {
    ///         name: "John".to_string(),
    ///         age: 18,
    ///         is_cool: true,
    ///         friends: vec!["Bob".to_string(), "Joe".to_string()]
    ///     };
    ///     buf.write_type(&packet).unwrap();
    /// }
    /// ```
    pub fn write_type<T: Writer>(&mut self, t: &T) -> Result<(), Error> {
        t.write(self)
    }

    pub fn as_slice(&self) -> &[u8] {
        self.buf.chunk()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteReader, ByteWriter};
    use bytes::Bytes;
    use std::io::{Read, Seek, SeekFrom};

    /// After `From<Vec<u8>>` / `From<&[u8]>` became zero-copy plus refcount sharing,
    /// `buf` and `raw_buf` must still point at the same bytes and stay readable after `seek`.
    ///
    /// `raw_buf` is only read by `seek` (see `Seek for ByteReader`); both handles
    /// always expose identical bytes.
    #[test]
    fn vec_and_slice_constructors_share_the_same_bytes() {
        let data: Vec<u8> = (0u8..=255).collect();

        for mut reader in [ByteReader::from(data.clone()), ByteReader::from(&data[..])] {
            assert_eq!(reader.buf.as_ref(), data.as_slice());
            assert_eq!(reader.raw_buf.as_ref(), data.as_slice());
            assert_eq!(reader.pos, 0);

            // Sequential read.
            let mut first = [0u8; 4];
            reader.read_exact(&mut first).unwrap();
            assert_eq!(first, [0, 1, 2, 3]);

            // seek relies on raw_buf: seeking back to start must re-read identical bytes.
            reader.seek(SeekFrom::Start(0)).unwrap();
            assert_eq!(reader.read_u8().unwrap(), 0);

            // Seek to the end.
            reader.seek(SeekFrom::End(0)).unwrap();
            assert_eq!(reader.read_u8().unwrap(), 255);
        }
    }

    /// The existing `From<Bytes>` behavior (refcount sharing) is unaffected by the `From<Vec>` path.
    #[test]
    fn bytes_constructor_keeps_refcount_sharing() {
        let bytes = Bytes::from_static(b"abcdef");
        let reader = ByteReader::from(bytes);
        assert_eq!(reader.buf.as_ref(), b"abcdef");
        assert_eq!(reader.raw_buf.as_ref(), b"abcdef");
    }

    #[test]
    fn var_i32_round_trips_zigzag() {
        for value in [
            0i32,
            1,
            -1,
            3,
            -3,
            127,
            -128,
            65535,
            -65536,
            i32::MAX,
            i32::MIN,
        ] {
            let mut writer = ByteWriter::new();
            writer.write_var_i32(value).unwrap();
            let mut reader = ByteReader::from(writer.as_slice());
            assert_eq!(
                reader.read_var_i32().unwrap(),
                value,
                "round trip of {value}"
            );
        }
    }

    #[test]
    fn var_i64_round_trips_zigzag() {
        for value in [
            0i64,
            1,
            -1,
            300,
            -300,
            i64::from(i32::MAX) + 1,
            i64::MAX,
            i64::MIN,
        ] {
            let mut writer = ByteWriter::new();
            writer.write_var_i64(value).unwrap();
            let mut reader = ByteReader::from(writer.as_slice());
            assert_eq!(
                reader.read_var_i64().unwrap(),
                value,
                "round trip of {value}"
            );
        }
    }

    #[test]
    fn var_i32_wire_format_matches_bedrock_signed_varint() {
        // zigzag(3) = 6 → single byte 0x06 on the wire.
        let mut writer = ByteWriter::new();
        writer.write_var_i32(3).unwrap();
        assert_eq!(writer.as_slice(), &[0x06]);
        let mut reader = ByteReader::from(&[0x06][..]);
        assert_eq!(reader.read_var_i32().unwrap(), 3);
    }
}
