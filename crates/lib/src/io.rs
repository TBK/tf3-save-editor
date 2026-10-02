//! Little-endian primitive reader/writer used by every save section.

use crate::{Error, Result};

/// Cursor over the decompressed save stream.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(Error::UnexpectedEof { offset: self.pos, wanted: len });
        }
        let out = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().expect("length checked"))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    /// A `u32` element count, sanity-checked against the bytes left so a
    /// corrupt count fails fast instead of allocating gigabytes.
    pub fn count(&mut self, min_item_size: usize) -> Result<usize> {
        let offset = self.pos;
        let n = self.u32()? as usize;
        if n.saturating_mul(min_item_size.max(1)) > self.remaining() {
            return Err(Error::Malformed { offset, what: format!("element count {n} exceeds remaining data") });
        }
        Ok(n)
    }

    /// `u32` length followed by raw bytes (std::string serialisation).
    pub fn string(&mut self) -> Result<Vec<u8>> {
        let len = self.count(1)?;
        Ok(self.bytes(len)?.to_vec())
    }
}

/// Append-only little-endian writer.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { buf: Vec::with_capacity(capacity) }
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn count(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("collection too large for save format"));
    }

    pub fn string(&mut self, s: &[u8]) {
        self.count(s.len());
        self.bytes(s);
    }
}
