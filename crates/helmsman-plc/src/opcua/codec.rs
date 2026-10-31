//! OPC UA binary encoding (Part 6 §5.2), the subset this server speaks.
//!
//! Decoding is the attack surface: every length is checked against what
//! is left of the message before anything is allocated, arrays are capped
//! ([`MAX_ARRAY`]), and the structures that nest (variants in variants,
//! diagnostic infos, data values) stop at [`MAX_DEPTH`]. A malformed
//! message is a [`DecodeError`], never a panic.

use std::fmt;

/// Most elements any decoded array may hold.
pub const MAX_ARRAY: usize = 10_000;
/// Deepest a variant, data value or diagnostic info may nest.
pub const MAX_DEPTH: u32 = 8;

/// The message is not valid OPC UA binary, or exceeds a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError;

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("malformed OPC UA binary")
    }
}

impl std::error::Error for DecodeError {}

pub type Result<T> = std::result::Result<T, DecodeError>;

/// A `NodeId`. The null node is `Num(0, 0)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    Num(u16, u32),
    Str(u16, String),
    Guid(u16, [u8; 16]),
    Bytes(u16, Vec<u8>),
}

impl NodeId {
    /// A node in namespace 0 (the standard's).
    #[must_use]
    pub const fn ns0(id: u32) -> Self {
        Self::Num(0, id)
    }

    /// A string node in namespace 1 (this box's).
    #[must_use]
    pub fn box_(s: impl Into<String>) -> Self {
        Self::Str(1, s.into())
    }

    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Num(0, 0))
    }

    /// The standard numeric id, if this is one in namespace 0.
    #[must_use]
    pub const fn ns0_id(&self) -> Option<u32> {
        match self {
            Self::Num(0, id) => Some(*id),
            _ => None,
        }
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Num(ns, id) => write!(f, "ns={ns};i={id}"),
            Self::Str(ns, s) => write!(f, "ns={ns};s={s}"),
            Self::Guid(ns, _) => write!(f, "ns={ns};g=..."),
            Self::Bytes(ns, _) => write!(f, "ns={ns};b=..."),
        }
    }
}

/// A cursor over one message body.
#[derive(Debug)]
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

    /// The next `n` bytes.
    ///
    /// # Errors
    /// Fewer than `n` are left.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(DecodeError)?;
        let s = self.buf.get(self.pos..end).ok_or(DecodeError)?;
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        let mut a = [0; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    /// # Errors
    /// The message ends first.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    /// Any non-zero byte is true, as the standard says.
    ///
    /// # Errors
    /// The message ends first.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    /// # Errors
    /// The message ends first.
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// # Errors
    /// The message ends first.
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// # Errors
    /// The message ends first.
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    /// # Errors
    /// The message ends first.
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// # Errors
    /// The message ends first.
    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    /// A length prefix: `None` for a null (any negative), else at most
    /// `limit` and at most what is left of the message (each element is
    /// at least one byte).
    fn len(&mut self, limit: usize) -> Result<Option<usize>> {
        let n = self.i32()?;
        if n < 0 {
            return Ok(None);
        }
        let n = usize::try_from(n).map_err(|_| DecodeError)?;
        if n > limit || n > self.remaining() {
            return Err(DecodeError);
        }
        Ok(Some(n))
    }

    /// A `ByteString`; `None` when null.
    ///
    /// # Errors
    /// Truncated.
    pub fn bytes(&mut self) -> Result<Option<Vec<u8>>> {
        match self.len(usize::MAX)? {
            None => Ok(None),
            Some(n) => Ok(Some(self.take(n)?.to_vec())),
        }
    }

    /// A `String`; `None` when null.
    ///
    /// # Errors
    /// Truncated, or not UTF-8.
    pub fn string(&mut self) -> Result<Option<String>> {
        match self.len(usize::MAX)? {
            None => Ok(None),
            Some(n) => std::str::from_utf8(self.take(n)?)
                .map(|s| Some(s.to_string()))
                .map_err(|_| DecodeError),
        }
    }

    /// An array's element count; `None` when null.
    ///
    /// # Errors
    /// Longer than [`MAX_ARRAY`] or than the message could hold.
    pub fn count(&mut self) -> Result<Option<usize>> {
        self.len(MAX_ARRAY)
    }

    /// An array of `T`, null read as empty.
    ///
    /// # Errors
    /// What `item` refuses, or a bad count.
    pub fn vec<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.count()?.unwrap_or(0);
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(item(self)?);
        }
        Ok(v)
    }

    fn node_body(&mut self, enc: u8) -> Result<NodeId> {
        Ok(match enc {
            0 => NodeId::Num(0, u32::from(self.u8()?)),
            1 => {
                let ns = u16::from(self.u8()?);
                NodeId::Num(ns, u32::from(self.u16()?))
            }
            2 => {
                let ns = self.u16()?;
                NodeId::Num(ns, self.u32()?)
            }
            3 => {
                let ns = self.u16()?;
                NodeId::Str(ns, self.string()?.unwrap_or_default())
            }
            4 => {
                let ns = self.u16()?;
                NodeId::Guid(ns, self.array()?)
            }
            5 => {
                let ns = self.u16()?;
                NodeId::Bytes(ns, self.bytes()?.unwrap_or_default())
            }
            _ => return Err(DecodeError),
        })
    }

    /// # Errors
    /// An unknown encoding, or truncated.
    pub fn node_id(&mut self) -> Result<NodeId> {
        let enc = self.u8()?;
        self.node_body(enc)
    }

    /// An `ExpandedNodeId`, keeping only the node: one with a namespace
    /// URI or another server's index is not a node of this server, so it
    /// comes back as `None`.
    ///
    /// # Errors
    /// An unknown encoding, or truncated.
    pub fn expanded_node_id(&mut self) -> Result<Option<NodeId>> {
        let enc = self.u8()?;
        let id = self.node_body(enc & 0x3F)?;
        let mut local = true;
        if enc & 0x80 != 0 {
            local &= self.string()?.is_none_or(|s| s.is_empty());
        }
        if enc & 0x40 != 0 {
            local &= self.u32()? == 0;
        }
        Ok(local.then_some(id))
    }

    /// # Errors
    /// Truncated.
    pub fn qualified_name(&mut self) -> Result<(u16, Option<String>)> {
        let ns = self.u16()?;
        Ok((ns, self.string()?))
    }

    /// The text of a `LocalizedText` (its locale is dropped).
    ///
    /// # Errors
    /// Truncated.
    pub fn localized_text(&mut self) -> Result<Option<String>> {
        let mask = self.u8()?;
        if mask & 0x01 != 0 {
            self.string()?;
        }
        if mask & 0x02 != 0 {
            return self.string();
        }
        Ok(None)
    }

    /// An `ExtensionObject`: its type and binary body (`None` when it has
    /// none; an XML body is skipped and reads as none).
    ///
    /// # Errors
    /// An unknown body encoding, or truncated.
    pub fn extension_object(&mut self) -> Result<(NodeId, Option<Vec<u8>>)> {
        let ty = self.node_id()?;
        match self.u8()? {
            0 => Ok((ty, None)),
            1 => Ok((ty, self.bytes()?)),
            2 => {
                self.bytes()?;
                Ok((ty, None))
            }
            _ => Err(DecodeError),
        }
    }

    /// Skips one value of built-in type `ty` (Part 6 Table 1).
    fn skip_builtin(&mut self, ty: u8, depth: u32) -> Result<()> {
        match ty {
            1..=3 => {
                self.take(1)?;
            }
            4 | 5 => {
                self.take(2)?;
            }
            6 | 7 | 10 | 19 => {
                self.take(4)?;
            }
            8 | 9 | 11 | 13 => {
                self.take(8)?;
            }
            14 => {
                self.take(16)?;
            }
            12 | 15 | 16 => {
                self.bytes()?;
            }
            17 => {
                self.node_id()?;
            }
            18 => {
                self.expanded_node_id()?;
            }
            20 => {
                self.qualified_name()?;
            }
            21 => {
                self.localized_text()?;
            }
            22 => {
                self.extension_object()?;
            }
            23 => self.skip_data_value(depth + 1)?,
            24 => self.skip_variant(depth + 1)?,
            25 => self.skip_diagnostic_info(depth + 1)?,
            _ => return Err(DecodeError),
        }
        Ok(())
    }

    /// Skips a `Variant`, however it nests (up to [`MAX_DEPTH`]).
    ///
    /// # Errors
    /// Malformed, too deep, or an array too long.
    pub fn skip_variant(&mut self, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(DecodeError);
        }
        let mask = self.u8()?;
        let ty = mask & 0x3F;
        if mask & 0x80 == 0 {
            if mask & 0x40 != 0 {
                return Err(DecodeError);
            }
            if ty == 0 {
                return Ok(());
            }
            return self.skip_builtin(ty, depth);
        }
        if ty == 0 {
            return Err(DecodeError);
        }
        let n = self.count()?.unwrap_or(0);
        for _ in 0..n {
            self.skip_builtin(ty, depth)?;
        }
        if mask & 0x40 != 0 {
            self.vec(Reader::i32)?;
        }
        Ok(())
    }

    /// Skips a `DataValue`.
    ///
    /// # Errors
    /// Malformed or too deep.
    pub fn skip_data_value(&mut self, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(DecodeError);
        }
        let mask = self.u8()?;
        if mask & 0x01 != 0 {
            self.skip_variant(depth + 1)?;
        }
        if mask & 0x02 != 0 {
            self.take(4)?;
        }
        if mask & 0x04 != 0 {
            self.take(8)?;
        }
        if mask & 0x10 != 0 {
            self.take(2)?;
        }
        if mask & 0x08 != 0 {
            self.take(8)?;
        }
        if mask & 0x20 != 0 {
            self.take(2)?;
        }
        Ok(())
    }

    /// Skips a `DiagnosticInfo`.
    ///
    /// # Errors
    /// Malformed or too deep.
    pub fn skip_diagnostic_info(&mut self, depth: u32) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(DecodeError);
        }
        let mask = self.u8()?;
        for bit in [0x01, 0x02, 0x08, 0x04] {
            if mask & bit != 0 {
                self.take(4)?;
            }
        }
        if mask & 0x10 != 0 {
            self.string()?;
        }
        if mask & 0x20 != 0 {
            self.take(4)?;
        }
        if mask & 0x40 != 0 {
            self.skip_diagnostic_info(depth + 1)?;
        }
        Ok(())
    }
}

/// A value this server sends (a `Variant`'s subset).
#[derive(Debug, Clone, PartialEq)]
pub enum Variant {
    Bool(bool),
    Byte(u8),
    I32(i32),
    U32(u32),
    I64(i64),
    F64(f64),
    Str(String),
    /// `DateTime`, in 100 ns ticks since 1601 ([`date_time`]).
    Time(i64),
    Node(NodeId),
    QName(u16, String),
    Text(String),
    StrArray(Vec<String>),
    U32Array(Vec<u32>),
    /// An `ExtensionObject` with a binary body, by encoding id.
    Ext(NodeId, Vec<u8>),
}

/// A `DataValue` as this server sends it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DataValue {
    pub value: Option<Variant>,
    /// Omitted when Good (0).
    pub status: u32,
    pub source_time: Option<i64>,
    pub server_time: Option<i64>,
}

/// Unix milliseconds as an OPC UA `DateTime` (100 ns since 1601-01-01).
#[must_use]
pub const fn date_time(unix_ms: i64) -> i64 {
    const EPOCH_1601: i64 = 116_444_736_000_000_000;
    unix_ms.saturating_mul(10_000).saturating_add(EPOCH_1601)
}

/// Appends OPC UA binary to a buffer.
#[derive(Debug, Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(u8::from(v))
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// An array or string length: the count as an `Int32`, saturating
    /// (nothing this server sends comes near).
    pub fn len(&mut self, n: usize) -> &mut Self {
        self.i32(i32::try_from(n).unwrap_or(i32::MAX))
    }

    /// A `ByteString` (`None`: null).
    pub fn bytes(&mut self, v: Option<&[u8]>) -> &mut Self {
        match v {
            None => self.i32(-1),
            Some(b) => {
                self.len(b.len());
                self.buf.extend_from_slice(b);
                self
            }
        }
    }

    /// A `String` (`None`: null).
    pub fn string(&mut self, v: Option<&str>) -> &mut Self {
        self.bytes(v.map(str::as_bytes))
    }

    /// A null array.
    pub fn null_array(&mut self) -> &mut Self {
        self.i32(-1)
    }

    pub fn node_id(&mut self, id: &NodeId) -> &mut Self {
        self.node_with(id, 0)
    }

    fn node_with(&mut self, id: &NodeId, flags: u8) -> &mut Self {
        match id {
            NodeId::Num(0, n) if *n <= 0xFF => {
                self.u8(flags).u8(u8::try_from(*n).unwrap_or(0));
            }
            NodeId::Num(ns, n) if *ns <= 0xFF && *n <= 0xFFFF => {
                self.u8(flags | 1)
                    .u8(u8::try_from(*ns).unwrap_or(0))
                    .u16(u16::try_from(*n).unwrap_or(0));
            }
            NodeId::Num(ns, n) => {
                self.u8(flags | 2).u16(*ns).u32(*n);
            }
            NodeId::Str(ns, s) => {
                self.u8(flags | 3).u16(*ns).string(Some(s));
            }
            NodeId::Guid(ns, g) => {
                self.u8(flags | 4).u16(*ns);
                self.buf.extend_from_slice(g);
            }
            NodeId::Bytes(ns, b) => {
                self.u8(flags | 5).u16(*ns).bytes(Some(b));
            }
        }
        self
    }

    /// An `ExpandedNodeId` of a node of this server.
    pub fn expanded_node_id(&mut self, id: &NodeId) -> &mut Self {
        self.node_with(id, 0)
    }

    pub fn qualified_name(&mut self, ns: u16, name: &str) -> &mut Self {
        self.u16(ns).string(Some(name))
    }

    /// A `LocalizedText` with no locale (`None`: empty).
    pub fn localized_text(&mut self, text: Option<&str>) -> &mut Self {
        match text {
            None => self.u8(0),
            Some(t) => self.u8(0x02).string(Some(t)),
        }
    }

    /// An `ExtensionObject` with a binary body (`None`: null object).
    pub fn extension_object(&mut self, ty: &NodeId, body: Option<&[u8]>) -> &mut Self {
        match body {
            None => self.node_id(&NodeId::ns0(0)).u8(0),
            Some(b) => self.node_id(ty).u8(1).bytes(Some(b)),
        }
    }

    /// An empty `DiagnosticInfo`.
    pub fn no_diagnostics(&mut self) -> &mut Self {
        self.u8(0)
    }

    pub fn variant(&mut self, v: &Variant) -> &mut Self {
        match v {
            Variant::Bool(b) => self.u8(1).bool(*b),
            Variant::Byte(b) => self.u8(3).u8(*b),
            Variant::I32(n) => self.u8(6).i32(*n),
            Variant::U32(n) => self.u8(7).u32(*n),
            Variant::I64(n) => self.u8(8).i64(*n),
            Variant::F64(x) => self.u8(11).f64(*x),
            Variant::Str(s) => self.u8(12).string(Some(s)),
            Variant::Time(t) => self.u8(13).i64(*t),
            Variant::Node(n) => self.u8(17).node_id(n),
            Variant::QName(ns, s) => self.u8(20).qualified_name(*ns, s),
            Variant::Text(s) => self.u8(21).localized_text(Some(s)),
            Variant::StrArray(a) => {
                self.u8(0x80 | 0x0C).len(a.len());
                for s in a {
                    self.string(Some(s));
                }
                self
            }
            Variant::U32Array(a) => {
                self.u8(0x80 | 0x07).len(a.len());
                for n in a {
                    self.u32(*n);
                }
                self
            }
            Variant::Ext(ty, body) => self.u8(22).extension_object(ty, Some(body)),
        }
    }

    pub fn data_value(&mut self, v: &DataValue) -> &mut Self {
        let mut mask = 0;
        if v.value.is_some() {
            mask |= 0x01;
        }
        if v.status != 0 {
            mask |= 0x02;
        }
        if v.source_time.is_some() {
            mask |= 0x04;
        }
        if v.server_time.is_some() {
            mask |= 0x08;
        }
        self.u8(mask);
        if let Some(x) = &v.value {
            self.variant(x);
        }
        if v.status != 0 {
            self.u32(v.status);
        }
        if let Some(t) = v.source_time {
            self.i64(t);
        }
        if let Some(t) = v.server_time {
            self.i64(t);
        }
        self
    }

    /// Appends bytes as they are.
    pub fn raw(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(id: &NodeId) -> NodeId {
        let mut w = Writer::new();
        w.node_id(id);
        let mut r = Reader::new(&w.buf);
        let back = r.node_id().unwrap();
        assert_eq!(r.remaining(), 0);
        back
    }

    #[test]
    fn node_ids_round_trip_in_every_encoding() {
        for id in [
            NodeId::ns0(84),
            NodeId::ns0(2253),
            NodeId::Num(1, 70_000),
            NodeId::Num(300, 5),
            NodeId::box_("signals/level"),
            NodeId::Guid(2, [7; 16]),
            NodeId::Bytes(0, vec![1, 2, 3]),
        ] {
            assert_eq!(round(&id), id);
        }
    }

    #[test]
    fn lengths_past_the_message_are_refused_before_allocating() {
        let mut r = Reader::new(&[0xFF, 0xFF, 0xFF, 0x7F, b'a']);
        assert_eq!(r.string(), Err(DecodeError));
        let mut r = Reader::new(&[0x10, 0x27, 0, 0]);
        assert_eq!(r.count(), Err(DecodeError));
    }

    #[test]
    fn nesting_stops_at_the_depth_limit() {
        // A variant holding a variant holding a variant...
        let mut deep = vec![24u8; 64];
        deep.push(0);
        assert_eq!(Reader::new(&deep).skip_variant(0), Err(DecodeError));
        let mut shallow = vec![24u8; 3];
        shallow.push(0);
        assert_eq!(Reader::new(&shallow).skip_variant(0), Ok(()));
    }

    #[test]
    fn date_time_counts_from_1601() {
        assert_eq!(date_time(0), 116_444_736_000_000_000);
    }
}
