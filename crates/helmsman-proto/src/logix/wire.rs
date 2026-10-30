//! Request builders and reply decoders for the read services.
//!
//! [`ReadRequest`] is the only way this module builds a request, and its
//! constructors are all reads: Read Tag (`0x4C`), Read Tag Fragmented
//! (`0x52` on a symbolic path), Get Instance Attribute List (`0x55`) on the
//! Symbol object, `Get_Attribute_Single`, a Multiple Service Packet of
//! those, and an `Unconnected_Send` around any of them. There is no
//! constructor that takes a service code, so code holding only this type
//! cannot ask a controller to write.
//!
//! Layouts:
//! - Read Tag: service, path, element count `u16`. Reply data: type (2 or
//!   4 bytes, see [`DataType`]), then the elements. pycomm3
//!   `packets/logix.py` `ReadTagRequestPacket`, `packets/util.py`
//!   `parse_read_reply`.
//! - Read Tag Fragmented: as Read Tag plus a byte offset `u32`; general
//!   status `0x06` while more data remains. pycomm3 `packets/logix.py`
//!   `ReadTagFragmentedRequestPacket` / `...ResponsePacket`,
//!   `logix_driver.py` `_send_read_fragmented`.
//! - Multiple Service Packet: to the Message Router (class 2, instance 1),
//!   data is a count `u16`, one offset `u16` per request measured from the
//!   count field, then the requests; the reply has the same shape around
//!   whole Message Router replies. pycomm3 `packets/logix.py`
//!   `MultiServiceRequestPacket` / `MultiServiceResponsePacket`.
//! - Symbol browse: class `0x6B`, the instance to start from (8, 16 or
//!   32-bit logical segment, the narrowest that fits), data is
//!   an attribute count and attribute ids. This crate asks for attributes
//!   1 (name), 2 (symbol type) and 8 (dimensions); each instance in the reply
//!   is its instance id `u32`, then those attributes in that order: name as
//!   `u16` length + bytes, type `u16`, three `u32` dimensions. Status
//!   `0x06` means more instances follow. pycomm3 `logix_driver.py`
//!   `_get_instance_attribute_list_service`, `_parse_instance_attribute_list`,
//!   `_create_tag` (symbol type bits).
//! - `Unconnected_Send` (Connection Manager, class 6, instance 1, `0x52`):
//!   priority/tick `0x0A`, timeout ticks `0x05`, embedded length `u16`, the
//!   embedded request, a pad byte if odd, route path size in words, a
//!   reserved byte, the route (port 1 = backplane, link = slot). pycomm3
//!   `packets/util.py` `wrap_unconnected_send`, `const.py` `PRIORITY`,
//!   `TIMEOUT_TICKS`, `cip_driver.py` (route encoded with length and pad).

use crate::cip::{self, GeneralStatus, class};
use crate::codec::{DecodeError, Reader, Writer};
use crate::io::service::UNCONNECTED_SEND;

use super::path::{PathError, TagPath};
use super::types::{DataType, TagData};

/// Logix read service codes.
pub mod service {
    pub const READ_TAG: u8 = 0x4C;
    pub const READ_TAG_FRAGMENTED: u8 = 0x52;
    pub const GET_INSTANCE_ATTRIBUTE_LIST: u8 = 0x55;
}

/// The Symbol object: one instance per tag.
pub const SYMBOL_CLASS: u16 = 0x6B;

/// Symbol attributes this crate asks for, in this order.
pub const SYMBOL_ATTRIBUTES: [u16; 3] = [1, 2, 8];

/// `Unconnected_Send` priority/time-tick and timeout ticks (pycomm3
/// `const.py` `PRIORITY`, `TIMEOUT_TICKS`).
pub const PRIORITY_TICK: u8 = 0x0A;
pub const TIMEOUT_TICKS: u8 = 0x05;
/// Backplane port number in a route path.
pub const BACKPLANE_PORT: u8 = 0x01;

/// The services a [`ReadRequest`] may carry. Kept closed so a test can
/// enumerate them.
pub const READ_SERVICES: [u8; 5] = [
    service::READ_TAG,
    service::READ_TAG_FRAGMENTED,
    service::GET_INSTANCE_ATTRIBUTE_LIST,
    cip::service::GET_ATTRIBUTE_SINGLE,
    cip::service::MULTIPLE_SERVICE_PACKET,
];

/// A Message Router request that can only read. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRequest {
    /// Service code of the request as sent (`0x52` on the Connection
    /// Manager when routed).
    service: u8,
    /// What the reply's service should be, minus the reply bit: the inner
    /// request's service when routed (a successful `Unconnected_Send`
    /// returns the embedded reply).
    answers: u8,
    bytes: Vec<u8>,
    routed: bool,
    multiple: Option<usize>,
}

fn with_path(service: u8, path_words: u8, path: &[u8], data: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(2 + path.len() + data.len());
    w.u8(service).u8(path_words).bytes(path).bytes(data);
    w.into_inner()
}

impl ReadRequest {
    fn plain(service: u8, bytes: Vec<u8>) -> Self {
        Self {
            service,
            answers: service,
            bytes,
            routed: false,
            multiple: None,
        }
    }

    /// Read Tag: `elements` elements starting at `path`. The bit suffix of
    /// `path`, if any, is dropped; pick the bit out of the reply.
    #[must_use]
    pub fn read_tag(path: &TagPath, elements: u16) -> Self {
        let mut data = Writer::with_capacity(2);
        data.u16_le(elements);
        Self::plain(
            service::READ_TAG,
            with_path(
                service::READ_TAG,
                path.size_words(),
                &path.encode(),
                &data.into_inner(),
            ),
        )
    }

    /// Read Tag Fragmented: `elements` elements starting at `path`, reply
    /// data starting `offset` bytes in.
    #[must_use]
    pub fn read_tag_fragmented(path: &TagPath, elements: u16, offset: u32) -> Self {
        let mut data = Writer::with_capacity(6);
        data.u16_le(elements).u32_le(offset);
        Self::plain(
            service::READ_TAG_FRAGMENTED,
            with_path(
                service::READ_TAG_FRAGMENTED,
                path.size_words(),
                &path.encode(),
                &data.into_inner(),
            ),
        )
    }

    /// Get Instance Attribute List on the Symbol object from instance
    /// `start` on, controller scope or inside `program` (`"Program:Main"`).
    ///
    /// # Errors
    /// `program` is not a valid tag name.
    pub fn symbol_list(program: Option<&str>, start: u32) -> Result<Self, PathError> {
        let mut path = Writer::default();
        if let Some(p) = program {
            let p = TagPath::parse(p)?;
            if p.bit().is_some() || p.elements().len() != 1 || !p.elements()[0].index.is_empty() {
                return Err(PathError::BadName(p.to_string()));
            }
            path.bytes(&p.encode());
        }
        path.u8(0x20).u8(u8::try_from(SYMBOL_CLASS).unwrap_or(0x6B));
        if let Ok(v) = u8::try_from(start) {
            path.u8(0x24).u8(v);
        } else if let Ok(v) = u16::try_from(start) {
            path.u8(0x25).u8(0).u16_le(v);
        } else {
            path.u8(0x26).u8(0).u32_le(start);
        }
        let path = path.into_inner();
        let words = u8::try_from(path.len() / 2).map_err(|_| PathError::TooLong)?;
        let mut data = Writer::default();
        data.u16_le(u16::try_from(SYMBOL_ATTRIBUTES.len()).unwrap_or(0));
        for a in SYMBOL_ATTRIBUTES {
            data.u16_le(a);
        }
        let s = service::GET_INSTANCE_ATTRIBUTE_LIST;
        Ok(Self::plain(s, with_path(s, words, &path, &data.into_inner())))
    }

    /// `Get_Attribute_Single`: a read, so it is allowed here.
    #[must_use]
    pub fn get_attribute_single(class: u16, instance: u16, attribute: u16) -> Self {
        let req = cip::Request::get_attribute_single(class, instance, attribute);
        let mut w = Writer::default();
        req.encode(&mut w);
        Self::plain(req.service, w.into_inner())
    }

    /// A Multiple Service Packet of reads. Nested packets and routed
    /// requests are not allowed inside one.
    ///
    /// # Errors
    /// Empty, nested, routed, or too large for 16-bit offsets.
    pub fn multiple(requests: &[Self]) -> Result<Self, &'static str> {
        if requests.is_empty() {
            return Err("empty multiple service packet");
        }
        if requests.iter().any(|r| r.multiple.is_some() || r.routed) {
            return Err("only plain reads go in a multiple service packet");
        }
        let n = u16::try_from(requests.len()).map_err(|_| "too many requests")?;
        let mut data = Writer::default();
        data.u16_le(n);
        let mut offset = 2 + 2 * requests.len();
        for r in requests {
            data.u16_le(u16::try_from(offset).map_err(|_| "multiple service packet too large")?);
            offset += r.bytes.len();
        }
        for r in requests {
            data.bytes(&r.bytes);
        }
        let mr = cip::Path::new(class::MESSAGE_ROUTER, 1);
        let s = cip::service::MULTIPLE_SERVICE_PACKET;
        Ok(Self {
            service: s,
            answers: s,
            bytes: with_path(s, mr.size_words(), &mr.encode(), &data.into_inner()),
            routed: false,
            multiple: Some(requests.len()),
        })
    }

    /// Wrap in an `Unconnected_Send` routed out the backplane to `slot`
    /// (a `ControlLogix` chassis, or a `CompactLogix` CPU at slot 0).
    ///
    /// # Errors
    /// Already routed, or the request is too long to embed.
    pub fn routed(self, slot: u8) -> Result<Self, &'static str> {
        if self.routed {
            return Err("request is already routed");
        }
        let n = u16::try_from(self.bytes.len()).map_err(|_| "request too long to route")?;
        let mut data = Writer::default();
        data.u8(PRIORITY_TICK)
            .u8(TIMEOUT_TICKS)
            .u16_le(n)
            .bytes(&self.bytes);
        if self.bytes.len() % 2 == 1 {
            data.u8(0);
        }
        data.u8(1).u8(0).u8(BACKPLANE_PORT).u8(slot);
        let cm = cip::Path::new(class::CONNECTION_MANAGER, 1);
        Ok(Self {
            service: UNCONNECTED_SEND,
            answers: self.answers,
            bytes: with_path(
                UNCONNECTED_SEND,
                cm.size_words(),
                &cm.encode(),
                &data.into_inner(),
            ),
            routed: true,
            multiple: self.multiple,
        })
    }

    /// The service byte as sent.
    #[must_use]
    pub const fn service(&self) -> u8 {
        self.service
    }

    /// The service the reply should answer (the embedded one when routed).
    #[must_use]
    pub const fn answers(&self) -> u8 {
        self.answers
    }

    /// Requests inside, for a Multiple Service Packet.
    #[must_use]
    pub const fn multiple_count(&self) -> Option<usize> {
        self.multiple
    }

    /// The encoded Message Router request.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Check that `resp` answers a request for `service`.
///
/// # Errors
/// `Invalid` if the reply is for some other service.
pub fn check_service(resp: &cip::Response, service: u8) -> Result<(), DecodeError> {
    if resp.service == service | cip::service::REPLY_BIT {
        Ok(())
    } else {
        Err(DecodeError::Invalid("reply is for a different service"))
    }
}

/// Decode the data of a successful or partial (`0x06`) read reply: type,
/// then raw element bytes.
///
/// # Errors
/// `Truncated` without a complete type; `Invalid` on an unknown type code.
pub fn decode_read_data(data: &[u8]) -> Result<TagData, DecodeError> {
    let mut r = Reader::new(data);
    let ty = DataType::decode(&mut r)?;
    Ok(TagData {
        ty,
        bytes: r.rest().to_vec(),
    })
}

/// Encode a read reply's data (for a target).
#[must_use]
pub fn encode_read_data(ty: DataType, bytes: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(ty.header_len() + bytes.len());
    ty.encode(&mut w);
    w.bytes(bytes);
    w.into_inner()
}

/// Split a Multiple Service Packet body (request or reply direction) into
/// its parts. Offsets must point inside the body, past the offset table,
/// and be non-decreasing; each part runs to the next offset.
///
/// # Errors
/// `Truncated` on a short table; `Invalid` on a bad offset.
pub fn split_multiple(body: &[u8]) -> Result<Vec<&[u8]>, DecodeError> {
    let mut r = Reader::new(body);
    let n = usize::from(r.u16_le()?);
    if n == 0 {
        return Err(DecodeError::Invalid("multiple service packet with no services"));
    }
    let table_end = 2 + 2 * n;
    let mut offsets = Vec::with_capacity(n);
    for _ in 0..n {
        offsets.push(usize::from(r.u16_le()?));
    }
    let mut parts = Vec::with_capacity(n);
    for (i, &start) in offsets.iter().enumerate() {
        let end = offsets.get(i + 1).copied().unwrap_or(body.len());
        if start < table_end || start > end || end > body.len() {
            return Err(DecodeError::Invalid(
                "multiple service packet offset out of range",
            ));
        }
        parts.push(&body[start..end]);
    }
    Ok(parts)
}

/// Decode a Multiple Service Packet reply into its `expect` replies. The
/// packet's own status may be success or `0x1E` (an embedded service
/// failed); the embedded replies carry the detail.
///
/// # Errors
/// `Invalid` on the wrong service, a packet-level error, a count that is
/// not `expect`, or an embedded reply that does not decode.
pub fn decode_multiple(resp: &cip::Response, expect: usize) -> Result<Vec<cip::Response>, DecodeError> {
    check_service(resp, cip::service::MULTIPLE_SERVICE_PACKET)?;
    if !(resp.status.is_success() || resp.status == GeneralStatus::EMBEDDED_SERVICE_ERROR) {
        return Err(DecodeError::Invalid("multiple service packet failed"));
    }
    let parts = split_multiple(&resp.data)?;
    if parts.len() != expect {
        return Err(DecodeError::Invalid(
            "multiple service reply count does not match request",
        ));
    }
    parts.into_iter().map(cip::Response::decode).collect()
}

/// Encode a Multiple Service Packet body from encoded parts (for a target,
/// and for the request builder's tests).
///
/// # Panics
/// More than 65535 parts or 64 KiB of data.
#[must_use]
pub fn join_multiple(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut w = Writer::default();
    w.u16_le(u16::try_from(parts.len()).expect("too many parts"));
    let mut offset = 2 + 2 * parts.len();
    for p in parts {
        w.u16_le(u16::try_from(offset).expect("multiple service packet too large"));
        offset += p.len();
    }
    for p in parts {
        w.bytes(p);
    }
    w.into_inner()
}

/// One entry of the controller's symbol table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub instance: u32,
    pub name: String,
    /// The raw symbol type word; see the accessors.
    pub symbol_type: u16,
    pub dims: [u32; 3],
}

impl Symbol {
    const STRUCT: u16 = 0x8000;
    const DIMS: u16 = 0x6000;
    const SYSTEM: u16 = 0x1000;

    /// Build the symbol type word (for a target).
    #[must_use]
    pub fn type_word(ty: DataType, dims: usize) -> u16 {
        let d = u16::try_from(dims.min(3)).unwrap_or(3) << 13;
        match ty {
            DataType::Atomic(t) => u16::from(t.code()) | d,
            DataType::Struct { handle } => Self::STRUCT | (handle & 0x0FFF) | d,
        }
    }

    /// Bit 15: a structure.
    #[must_use]
    pub const fn is_struct(&self) -> bool {
        self.symbol_type & Self::STRUCT != 0
    }

    /// Bits 13-14: number of array dimensions, 0 for a scalar.
    #[must_use]
    pub const fn dim_count(&self) -> u8 {
        ((self.symbol_type & Self::DIMS) >> 13) as u8
    }

    /// Bit 12: a system / reserved symbol (pycomm3 skips these), or a name
    /// starting with `__` (controller internals).
    #[must_use]
    pub fn is_system(&self) -> bool {
        self.symbol_type & Self::SYSTEM != 0 || self.name.starts_with("__")
    }

    /// A program, whose tags are browsed with `program: Some(name)`.
    #[must_use]
    pub fn is_program(&self) -> bool {
        self.name.starts_with("Program:")
    }

    /// The atomic type, for a non-structure symbol whose low byte is a
    /// known type code.
    #[must_use]
    pub fn atomic(&self) -> Option<super::types::AtomicType> {
        if self.is_struct() {
            return None;
        }
        super::types::AtomicType::from_code((self.symbol_type & 0xFF) as u8)
    }

    /// The structure's template instance (bits 0-11), for a structure.
    #[must_use]
    pub const fn template(&self) -> Option<u16> {
        if self.is_struct() {
            Some(self.symbol_type & 0x0FFF)
        } else {
            None
        }
    }

    /// Dimensions in use, outermost first.
    #[must_use]
    pub fn shape(&self) -> &[u32] {
        &self.dims[..usize::from(self.dim_count())]
    }

    pub fn encode(&self, w: &mut Writer) {
        let name = self.name.as_bytes();
        w.u32_le(self.instance)
            .u16_le(u16::try_from(name.len()).unwrap_or(u16::MAX))
            .bytes(&name[..name.len().min(usize::from(u16::MAX))])
            .u16_le(self.symbol_type);
        for d in self.dims {
            w.u32_le(d);
        }
    }
}

/// Decode the instances in a Get Instance Attribute List reply's data.
///
/// # Errors
/// `Truncated` on a cut-off entry; `Invalid` on an empty or non-ASCII name.
pub fn decode_symbols(data: &[u8]) -> Result<Vec<Symbol>, DecodeError> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let instance = r.u32_le()?;
        let n = usize::from(r.u16_le()?);
        let name = r.bytes(n)?;
        if name.is_empty() || !name.iter().all(u8::is_ascii_graphic) {
            return Err(DecodeError::Invalid(
                "symbol name is empty or not printable ASCII",
            ));
        }
        let symbol_type = r.u16_le()?;
        let dims = [r.u32_le()?, r.u32_le()?, r.u32_le()?];
        out.push(Symbol {
            instance,
            name: String::from_utf8_lossy(name).into_owned(),
            symbol_type,
            dims,
        });
    }
    Ok(out)
}
