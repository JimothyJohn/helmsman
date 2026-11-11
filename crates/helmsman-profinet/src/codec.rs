//! Bounds-checked reader and writer.
//!
//! PROFINET's own fields are big-endian: the real-time frames, DCP and
//! every block of the context-management PDUs. The one exception is the
//! DCE/RPC envelope those blocks travel in (the 80-byte header and the
//! NDR argument header), whose integers follow the sender's data
//! representation; [`Order`] carries that choice.

use alloc::vec::Vec;
use core::fmt;

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

impl core::error::Error for DecodeError {}

/// Byte order of an integer on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Big,
    Little,
}

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

    /// Bytes consumed so far.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
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

    /// Take `N` bytes as an array.
    ///
    /// # Errors
    /// `Truncated` if fewer than `N` bytes remain.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0; N];
        out.copy_from_slice(self.bytes(N)?);
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

    /// A big-endian `u16`.
    ///
    /// # Errors
    /// `Truncated` if fewer than two bytes remain.
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    /// A big-endian `u32`.
    ///
    /// # Errors
    /// `Truncated` if fewer than four bytes remain.
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// A `u16` in the given order.
    ///
    /// # Errors
    /// `Truncated` if fewer than two bytes remain.
    pub fn u16_in(&mut self, order: Order) -> Result<u16, DecodeError> {
        let b = self.array()?;
        Ok(match order {
            Order::Big => u16::from_be_bytes(b),
            Order::Little => u16::from_le_bytes(b),
        })
    }

    /// A `u32` in the given order.
    ///
    /// # Errors
    /// `Truncated` if fewer than four bytes remain.
    pub fn u32_in(&mut self, order: Order) -> Result<u32, DecodeError> {
        let b = self.array()?;
        Ok(match order {
            Order::Big => u32::from_be_bytes(b),
            Order::Little => u32::from_le_bytes(b),
        })
    }

    /// Take exactly `n` bytes and return a sub-reader over them, so a
    /// length-prefixed item cannot read past its own declared length.
    ///
    /// # Errors
    /// `Truncated` if fewer than `n` bytes remain.
    pub fn sub(&mut self, n: usize) -> Result<Self, DecodeError> {
        Ok(Self::new(self.bytes(n)?))
    }
}

/// Growable output buffer, big-endian unless an [`Order`] says otherwise.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: Vec::new() }
    }

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

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u16_in(&mut self, order: Order, v: u16) -> &mut Self {
        match order {
            Order::Big => self.buf.extend_from_slice(&v.to_be_bytes()),
            Order::Little => self.buf.extend_from_slice(&v.to_le_bytes()),
        }
        self
    }

    pub fn u32_in(&mut self, order: Order, v: u32) -> &mut Self {
        match order {
            Order::Big => self.buf.extend_from_slice(&v.to_be_bytes()),
            Order::Little => self.buf.extend_from_slice(&v.to_le_bytes()),
        }
        self
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// `n` zero bytes.
    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.buf.resize(self.buf.len() + n, 0);
        self
    }

    /// Overwrites the big-endian `u16` at `at` (a length known only once
    /// what it counts has been written).
    ///
    /// # Panics
    /// If `at + 2` is past what has been written: a bug in the caller.
    pub fn set_u16(&mut self, at: usize, v: u16) {
        self.buf[at..at + 2].copy_from_slice(&v.to_be_bytes());
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.buf.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// A length that must fit a `u16` field.
///
/// # Errors
/// `Invalid(what)` when it does not.
pub fn len16(n: usize, what: &'static str) -> Result<u16, DecodeError> {
    u16::try_from(n).map_err(|_| DecodeError::Invalid(what))
}
