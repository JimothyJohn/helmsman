//! Logix data types as they appear on the wire: the type code that leads
//! every Read Tag reply, and the values it describes.
//!
//! A reply's data starts with a 16-bit type: an atomic type code (`0x00C1`
//! BOOL .. `0x00D4` LWORD), or `0x02A0` followed by a 16-bit structure
//! handle for a structure. Values are little-endian, elements packed.
//!
//! References: pycomm3 `pycomm3/cip/data_types.py` (the `code` of each
//! elementary type), `pycomm3/const.py` (`STRUCTURE_READ_REPLY = b"\xa0\x02"`),
//! `pycomm3/packets/logix.py` (`ReadTagFragmentedResponsePacket`: 2-byte type
//! for atomics, 4 bytes for structures). See `docs/SOURCES.md`.

use std::fmt;

use crate::codec::{DecodeError, Reader, Writer};

/// The abbreviated type that marks a structure in a read reply; a 16-bit
/// structure handle follows it.
pub const STRUCTURE: u16 = 0x02A0;

/// The atomic (elementary) types a Logix controller can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AtomicType {
    Bool,
    Sint,
    Int,
    Dint,
    Lint,
    Usint,
    Uint,
    Udint,
    Ulint,
    Real,
    Lreal,
    Byte,
    Word,
    Dword,
    Lword,
}

impl AtomicType {
    pub const ALL: [Self; 15] = [
        Self::Bool,
        Self::Sint,
        Self::Int,
        Self::Dint,
        Self::Lint,
        Self::Usint,
        Self::Uint,
        Self::Udint,
        Self::Ulint,
        Self::Real,
        Self::Lreal,
        Self::Byte,
        Self::Word,
        Self::Dword,
        Self::Lword,
    ];

    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Bool => 0xC1,
            Self::Sint => 0xC2,
            Self::Int => 0xC3,
            Self::Dint => 0xC4,
            Self::Lint => 0xC5,
            Self::Usint => 0xC6,
            Self::Uint => 0xC7,
            Self::Udint => 0xC8,
            Self::Ulint => 0xC9,
            Self::Real => 0xCA,
            Self::Lreal => 0xCB,
            Self::Byte => 0xD1,
            Self::Word => 0xD2,
            Self::Dword => 0xD3,
            Self::Lword => 0xD4,
        }
    }

    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.code() == code)
    }

    /// Bytes per element.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            Self::Bool | Self::Sint | Self::Usint | Self::Byte => 1,
            Self::Int | Self::Uint | Self::Word => 2,
            Self::Dint | Self::Udint | Self::Real | Self::Dword => 4,
            Self::Lint | Self::Ulint | Self::Lreal | Self::Lword => 8,
        }
    }

    /// The type's name as Logix spells it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bool => "BOOL",
            Self::Sint => "SINT",
            Self::Int => "INT",
            Self::Dint => "DINT",
            Self::Lint => "LINT",
            Self::Usint => "USINT",
            Self::Uint => "UINT",
            Self::Udint => "UDINT",
            Self::Ulint => "ULINT",
            Self::Real => "REAL",
            Self::Lreal => "LREAL",
            Self::Byte => "BYTE",
            Self::Word => "WORD",
            Self::Dword => "DWORD",
            Self::Lword => "LWORD",
        }
    }

    /// Case-insensitive inverse of [`Self::name`].
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(s))
    }

    /// Integer types, signed or not, and bit strings: what a bit suffix
    /// (`Tag.5`) can address.
    #[must_use]
    pub const fn is_integer(self) -> bool {
        !matches!(self, Self::Bool | Self::Real | Self::Lreal)
    }

    /// Decode one element. `bytes` must be exactly [`Self::size`] long.
    ///
    /// # Errors
    /// `Truncated` if `bytes` is short; `Invalid` if it is long.
    pub fn decode(self, bytes: &[u8]) -> Result<Value, DecodeError> {
        if bytes.len() > self.size() {
            return Err(DecodeError::Invalid("element longer than its type"));
        }
        let mut r = Reader::new(bytes);
        let b = r.bytes(self.size())?;
        let mut a8 = [0u8; 8];
        a8[..b.len()].copy_from_slice(b);
        let u = u64::from_le_bytes(a8);
        Ok(match self {
            Self::Bool => Value::Bool(b[0] != 0),
            Self::Sint => Value::Sint(i8::from_le_bytes([b[0]])),
            Self::Int => Value::Int(i16::from_le_bytes([b[0], b[1]])),
            Self::Dint => Value::Dint(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Self::Lint => Value::Lint(i64::from_le_bytes(a8)),
            Self::Usint => Value::Usint(b[0]),
            Self::Uint => Value::Uint(u16::from_le_bytes([b[0], b[1]])),
            Self::Udint => Value::Udint(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Self::Ulint => Value::Ulint(u),
            Self::Real => Value::Real(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Self::Lreal => Value::Lreal(f64::from_le_bytes(a8)),
            Self::Byte => Value::Byte(b[0]),
            Self::Word => Value::Word(u16::from_le_bytes([b[0], b[1]])),
            Self::Dword => Value::Dword(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Self::Lword => Value::Lword(u),
        })
    }
}

impl fmt::Display for AtomicType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The type at the head of a read reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    Atomic(AtomicType),
    /// A structure (UDT, STRING, timer, ...), by its handle. Its layout
    /// lives in a template this crate does not read.
    Struct {
        handle: u16,
    },
}

impl DataType {
    /// Bytes this type takes at the head of a reply.
    #[must_use]
    pub const fn header_len(self) -> usize {
        match self {
            Self::Atomic(_) => 2,
            Self::Struct { .. } => 4,
        }
    }

    pub fn encode(self, w: &mut Writer) {
        match self {
            Self::Atomic(t) => {
                w.u16_le(u16::from(t.code()));
            }
            Self::Struct { handle } => {
                w.u16_le(STRUCTURE).u16_le(handle);
            }
        }
    }

    /// # Errors
    /// `Truncated` on short input; `Invalid` on a code that is neither an
    /// atomic type this crate knows nor the structure marker.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let code = r.u16_le()?;
        if code == STRUCTURE {
            return Ok(Self::Struct { handle: r.u16_le()? });
        }
        u8::try_from(code)
            .ok()
            .and_then(AtomicType::from_code)
            .map(Self::Atomic)
            .ok_or(DecodeError::Invalid("unknown data type code"))
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Atomic(t) => write!(f, "{t}"),
            Self::Struct { handle } => write!(f, "struct 0x{handle:04X}"),
        }
    }
}

/// One decoded element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Bool(bool),
    Sint(i8),
    Int(i16),
    Dint(i32),
    Lint(i64),
    Usint(u8),
    Uint(u16),
    Udint(u32),
    Ulint(u64),
    Real(f32),
    Lreal(f64),
    Byte(u8),
    Word(u16),
    Dword(u32),
    Lword(u64),
}

impl Value {
    /// An integer value widened to `i128`, so every signed and unsigned
    /// type fits; `None` for BOOL and floating point.
    #[must_use]
    pub fn as_integer(self) -> Option<i128> {
        Some(match self {
            Self::Sint(v) => i128::from(v),
            Self::Int(v) => i128::from(v),
            Self::Dint(v) => i128::from(v),
            Self::Lint(v) => i128::from(v),
            Self::Usint(v) | Self::Byte(v) => i128::from(v),
            Self::Uint(v) | Self::Word(v) => i128::from(v),
            Self::Udint(v) | Self::Dword(v) => i128::from(v),
            Self::Ulint(v) | Self::Lword(v) => i128::from(v),
            Self::Bool(_) | Self::Real(_) | Self::Lreal(_) => return None,
        })
    }

    /// Bit `n` of an integer value; `None` for non-integers or a bit past
    /// the type's width.
    #[must_use]
    pub fn bit(self, n: u8) -> Option<bool> {
        let width = match self {
            Self::Sint(_) | Self::Usint(_) | Self::Byte(_) => 8,
            Self::Int(_) | Self::Uint(_) | Self::Word(_) => 16,
            Self::Dint(_) | Self::Udint(_) | Self::Dword(_) => 32,
            Self::Lint(_) | Self::Ulint(_) | Self::Lword(_) => 64,
            Self::Bool(_) | Self::Real(_) | Self::Lreal(_) => return None,
        };
        if u32::from(n) >= width {
            return None;
        }
        // Two's complement bits of the value at its own width.
        let raw = self.as_integer()? & ((1i128 << width) - 1);
        Some(raw >> n & 1 == 1)
    }
}

/// What a read returned: the type and the raw element bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagData {
    pub ty: DataType,
    pub bytes: Vec<u8>,
}

impl TagData {
    /// Every element, decoded.
    ///
    /// # Errors
    /// `Invalid` for a structure, or a byte count that is not a whole number
    /// of elements.
    pub fn values(&self) -> Result<Vec<Value>, DecodeError> {
        let DataType::Atomic(t) = self.ty else {
            return Err(DecodeError::Invalid("structure values need a template"));
        };
        if !self.bytes.len().is_multiple_of(t.size()) {
            return Err(DecodeError::Invalid("data is not a whole number of elements"));
        }
        self.bytes.chunks(t.size()).map(|c| t.decode(c)).collect()
    }

    /// The first element.
    ///
    /// # Errors
    /// As [`Self::values`], or no element at all.
    pub fn value(&self) -> Result<Value, DecodeError> {
        let DataType::Atomic(t) = self.ty else {
            return Err(DecodeError::Invalid("structure values need a template"));
        };
        if self.bytes.len() < t.size() {
            return Err(DecodeError::Truncated {
                needed: t.size(),
                available: self.bytes.len(),
            });
        }
        t.decode(&self.bytes[..t.size()])
    }
}
