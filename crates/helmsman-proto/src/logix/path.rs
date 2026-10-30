//! Symbolic tag paths: `Program:Main.Recipe[3].Temp`, `Matrix[1,2]`,
//! `Status.5` (bit 5 of an integer tag).
//!
//! Encoding, per element: an ANSI extended symbolic segment (`0x91`, name
//! length, name bytes, a pad byte when the length is odd), then one member
//! segment per array index: `0x28 v` (8-bit), `0x29 0x00 v16` (16-bit,
//! padded), `0x2A 0x00 v32` (32-bit, padded). A bit suffix is not part of
//! the path: the controller is asked for the whole integer and the bit is
//! picked out here.
//!
//! References: pycomm3 `pycomm3/cip/data_types.py` (`DataSegment`
//! `extended_symbol = 0x91`, odd-length pad; `LogicalSegment` member type
//! `0b000_010_00`), `pycomm3/packets/util.py` (`tag_request_path`: split on
//! `.`, indexes as member segments); libplctag
//! `src/libplctag/protocols/ab/cip.c` (`parse_symbolic_segment`,
//! `parse_numeric_segment`: 0x28/0x29/0x2A with a pad byte before 16- and
//! 32-bit values; at most three dimensions; bit suffix kept out of the
//! path). Read for layouts only; no code taken. See `docs/SOURCES.md`.

use std::fmt;

use crate::codec::{DecodeError, Reader, Writer};

/// ANSI extended symbolic segment type.
pub const SEGMENT_SYMBOLIC: u8 = 0x91;
/// Member (element index) logical segments, by width.
pub const SEGMENT_MEMBER_8: u8 = 0x28;
pub const SEGMENT_MEMBER_16: u8 = 0x29;
pub const SEGMENT_MEMBER_32: u8 = 0x2A;

/// Logix arrays have at most three dimensions (the symbol type word has two
/// bits for the dimension count; libplctag `cip_encode_tag_name` stops at 3).
pub const MAX_DIMS: usize = 3;
/// A symbolic segment's length is one byte.
pub const MAX_NAME_LEN: usize = 255;
/// Bits in the widest integer a bit suffix can address (LINT).
pub const MAX_BIT: u8 = 63;

/// Why a tag path string or byte sequence is not a valid path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    /// A name that is empty, too long, or has a character Logix does not
    /// allow (`[A-Za-z_]` first, then `[A-Za-z0-9_:]`).
    BadName(String),
    /// An index list that does not parse, has more than three entries, or
    /// overflows 32 bits.
    BadIndex(String),
    /// A bit suffix out of range, or not at the end.
    BadBit(String),
    /// The encoded path would not fit in a request (255 words).
    TooLong,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty tag path"),
            Self::BadName(n) => write!(f, "bad tag name {n:?}"),
            Self::BadIndex(i) => write!(f, "bad array index {i:?}"),
            Self::BadBit(b) => write!(f, "bad bit number {b:?} (0..={MAX_BIT}, last)"),
            Self::TooLong => write!(f, "tag path longer than 255 words"),
        }
    }
}

impl std::error::Error for PathError {}

/// One `.`-separated part of a path: a name and its array indexes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Element {
    pub name: String,
    pub index: Vec<u32>,
}

/// A parsed tag path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagPath {
    elements: Vec<Element>,
    bit: Option<u8>,
}

fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= MAX_NAME_LEN
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn parse_element(part: &str) -> Result<Element, PathError> {
    let (name, index) = match part.find('[') {
        None => (part, Vec::new()),
        Some(open) => {
            let inner = part[open + 1..]
                .strip_suffix(']')
                .ok_or_else(|| PathError::BadIndex(part.into()))?;
            let index = inner
                .split(',')
                .map(|n| {
                    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(PathError::BadIndex(part.into()));
                    }
                    n.parse::<u32>().map_err(|_| PathError::BadIndex(part.into()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if index.len() > MAX_DIMS {
                return Err(PathError::BadIndex(part.into()));
            }
            (&part[..open], index)
        }
    };
    if !valid_name(name) {
        return Err(PathError::BadName(name.into()));
    }
    Ok(Element {
        name: name.into(),
        index,
    })
}

impl TagPath {
    /// Parse `Name[.Member][...]` with optional `[i]`, `[i,j]`, `[i,j,k]`
    /// after any name and an optional trailing `.N` bit number.
    ///
    /// # Errors
    /// Anything outside that grammar, or a path too long to encode.
    pub fn parse(s: &str) -> Result<Self, PathError> {
        if s.is_empty() {
            return Err(PathError::Empty);
        }
        let parts: Vec<&str> = s.split('.').collect();
        let mut elements = Vec::with_capacity(parts.len());
        let mut bit = None;
        for (i, part) in parts.iter().enumerate() {
            let numeric = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
            if numeric && i > 0 {
                if i != parts.len() - 1 {
                    return Err(PathError::BadBit((*part).into()));
                }
                let n = part
                    .parse::<u8>()
                    .ok()
                    .filter(|n| *n <= MAX_BIT)
                    .ok_or_else(|| PathError::BadBit((*part).into()))?;
                bit = Some(n);
            } else {
                elements.push(parse_element(part)?);
            }
        }
        let path = Self { elements, bit };
        if path.encoded_len() > usize::from(u8::MAX) * 2 {
            return Err(PathError::TooLong);
        }
        Ok(path)
    }

    /// Build from elements, checking names and index counts.
    ///
    /// # Errors
    /// As [`Self::parse`].
    pub fn from_elements(elements: Vec<Element>, bit: Option<u8>) -> Result<Self, PathError> {
        if elements.is_empty() {
            return Err(PathError::Empty);
        }
        for e in &elements {
            if !valid_name(&e.name) {
                return Err(PathError::BadName(e.name.clone()));
            }
            if e.index.len() > MAX_DIMS {
                return Err(PathError::BadIndex(format!("{:?}", e.index)));
            }
        }
        if bit.is_some_and(|b| b > MAX_BIT) {
            return Err(PathError::BadBit(format!("{bit:?}")));
        }
        let path = Self { elements, bit };
        if path.encoded_len() > usize::from(u8::MAX) * 2 {
            return Err(PathError::TooLong);
        }
        Ok(path)
    }

    #[must_use]
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    /// The bit number of a `Tag.N` suffix.
    #[must_use]
    pub const fn bit(&self) -> Option<u8> {
        self.bit
    }

    /// The same path without its bit suffix: what goes on the wire.
    #[must_use]
    pub fn without_bit(&self) -> Self {
        Self {
            elements: self.elements.clone(),
            bit: None,
        }
    }

    fn encoded_len(&self) -> usize {
        self.elements
            .iter()
            .map(|e| {
                let name = 2 + e.name.len() + e.name.len() % 2;
                let idx: usize = e
                    .index
                    .iter()
                    .map(|i| match i {
                        0..=0xFF => 2,
                        0x100..=0xFFFF => 4,
                        _ => 6,
                    })
                    .sum();
                name + idx
            })
            .sum()
    }

    /// Path bytes without the size prefix; always an even length.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.encoded_len());
        for e in &self.elements {
            let name = e.name.as_bytes();
            // Lengths are checked at construction.
            w.u8(SEGMENT_SYMBOLIC)
                .u8(u8::try_from(name.len()).unwrap_or(u8::MAX))
                .bytes(name);
            if name.len() % 2 == 1 {
                w.u8(0);
            }
            for &i in &e.index {
                if let Ok(v) = u8::try_from(i) {
                    w.u8(SEGMENT_MEMBER_8).u8(v);
                } else if let Ok(v) = u16::try_from(i) {
                    w.u8(SEGMENT_MEMBER_16).u8(0).u16_le(v);
                } else {
                    w.u8(SEGMENT_MEMBER_32).u8(0).u32_le(i);
                }
            }
        }
        w.into_inner()
    }

    /// Size of [`Self::encode`] in 16-bit words.
    #[must_use]
    pub fn size_words(&self) -> u8 {
        u8::try_from(self.encoded_len() / 2).unwrap_or(u8::MAX)
    }
}

impl fmt::Display for TagPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, e) in self.elements.iter().enumerate() {
            if n > 0 {
                f.write_str(".")?;
            }
            f.write_str(&e.name)?;
            if !e.index.is_empty() {
                let idx: Vec<String> = e.index.iter().map(u32::to_string).collect();
                write!(f, "[{}]", idx.join(","))?;
            }
        }
        if let Some(b) = self.bit {
            write!(f, ".{b}")?;
        }
        Ok(())
    }
}

/// One decoded request path segment, as a target sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Symbolic(String),
    Class(u32),
    Instance(u32),
    Member(u32),
    Attribute(u32),
}

/// Decode a request path of `words` 16-bit words into segments: symbolic
/// and logical (class, instance, member, attribute; 8/16/32-bit). For a
/// target (the test fixture) and for checking what this crate encodes.
///
/// # Errors
/// `Truncated` on short input; `Invalid` on any other segment type, an
/// empty or non-ASCII symbol, or a non-zero pad byte.
pub fn decode_segments(r: &mut Reader<'_>, words: u8) -> Result<Vec<Segment>, DecodeError> {
    let mut seg = r.sub(usize::from(words) * 2)?;
    let mut out = Vec::new();
    while !seg.is_empty() {
        let head = seg.u8()?;
        if head == SEGMENT_SYMBOLIC {
            let n = usize::from(seg.u8()?);
            if n == 0 {
                return Err(DecodeError::Invalid("empty symbolic segment"));
            }
            let name = seg.bytes(n)?;
            if !name.iter().all(u8::is_ascii_graphic) {
                return Err(DecodeError::Invalid("symbolic segment is not printable ASCII"));
            }
            if n % 2 == 1 && seg.u8()? != 0 {
                return Err(DecodeError::Invalid("symbolic segment pad byte is not zero"));
            }
            out.push(Segment::Symbolic(String::from_utf8_lossy(name).into_owned()));
            continue;
        }
        if head & 0xE0 != 0x20 {
            return Err(DecodeError::Invalid("unsupported path segment type"));
        }
        let value = match head & 0x03 {
            0 => u32::from(seg.u8()?),
            1 => {
                if seg.u8()? != 0 {
                    return Err(DecodeError::Invalid("logical segment pad byte is not zero"));
                }
                u32::from(seg.u16_le()?)
            }
            2 => {
                if seg.u8()? != 0 {
                    return Err(DecodeError::Invalid("logical segment pad byte is not zero"));
                }
                seg.u32_le()?
            }
            _ => return Err(DecodeError::Invalid("reserved logical segment format")),
        };
        out.push(match head & 0x1C {
            0x00 => Segment::Class(value),
            0x04 => Segment::Instance(value),
            0x08 => Segment::Member(value),
            0x10 => Segment::Attribute(value),
            _ => return Err(DecodeError::Invalid("unsupported logical segment type")),
        });
    }
    Ok(out)
}

impl TagPath {
    /// Rebuild a tag path from decoded segments: symbols, each followed by
    /// its member (index) segments. Anything else is `None`.
    #[must_use]
    pub fn from_segments(segments: &[Segment]) -> Option<Self> {
        let mut elements: Vec<Element> = Vec::new();
        for s in segments {
            match s {
                Segment::Symbolic(name) => elements.push(Element {
                    name: name.clone(),
                    index: Vec::new(),
                }),
                Segment::Member(i) => elements.last_mut()?.index.push(*i),
                _ => return None,
            }
        }
        Self::from_elements(elements, None).ok()
    }
}
