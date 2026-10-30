//! Bounds-checked little-endian reader and writer.
//!
//! The encapsulation layer and CIP are little-endian (PUB00213R0 p.23,
//! "Data format for the Encapsulation Protocol is Little-Endian"). The one
//! exception is the socket-address item, which carries the BSD `sockaddr_in`
//! fields in network (big-endian) order; see [`crate::identity`].

use std::fmt;

/// Why a byte sequence could not be decoded.
///
/// Every decoder in this crate returns one of these instead of panicking,
/// whatever the input. That is the contract the property tests enforce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended before a field was complete.
    Truncated {
        /// Bytes the field needed.
        needed: usize,
        /// Bytes that were left.
        available: usize,
    },
    /// A field carried a value the protocol does not allow here.
    Invalid(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, available } => {
                write!(f, "truncated: needed {needed} bytes, {available} available")
            }
            Self::Invalid(what) => write!(f, "invalid: {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Cursor over an input slice. Every read is bounds-checked.
#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Take `n` bytes.
    ///
    /// # Errors
    /// `Truncated` if fewer than `n` bytes remain.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let available = self.remaining();
        if available < n {
            return Err(DecodeError::Truncated { needed: n, available });
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Take everything that is left.
    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.buf[self.pos..];
        self.pos = self.buf.len();
        out
    }

    /// # Errors
    /// `Truncated` if no byte remains.
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.bytes(1)?[0])
    }

    /// # Errors
    /// `Truncated` if fewer than two bytes remain.
    pub fn u16_le(&mut self) -> Result<u16, DecodeError> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// # Errors
    /// `Truncated` if fewer than four bytes remain.
    pub fn u32_le(&mut self) -> Result<u32, DecodeError> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// # Errors
    /// `Truncated` if fewer than two bytes remain.
    pub fn u16_be(&mut self) -> Result<u16, DecodeError> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// # Errors
    /// `Truncated` if fewer than four bytes remain.
    pub fn u32_be(&mut self) -> Result<u32, DecodeError> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Take exactly `n` bytes and return a sub-reader over them, so a
    /// length-prefixed item cannot read past its own declared length.
    ///
    /// # Errors
    /// `Truncated` if fewer than `n` bytes remain.
    pub fn sub(&mut self, n: usize) -> Result<Reader<'a>, DecodeError> {
        Ok(Reader::new(self.bytes(n)?))
    }
}

/// Growable output buffer with little-endian helpers.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    #[must_use]
    pub fn with_capacity(n: usize) -> Self {
        Self {
            buf: Vec::with_capacity(n),
        }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u16_le(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32_le(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u16_be(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u32_be(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}
