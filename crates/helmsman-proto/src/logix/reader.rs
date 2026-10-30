//! [`TagReader`]: a session that can only read from a Logix controller.
//!
//! It owns its [`Session`] and never hands it back, and every request it
//! sends is a [`ReadRequest`]. Replies are treated as hostile: each one must
//! answer the service that was sent, carry a known type, and hold exactly
//! the bytes the element count implies; fragmented reads must make progress
//! and keep one type; a browse must move forward through instances. Any
//! breach is an [`Error`], never a panic or a loop.

use std::fmt;
use std::net::ToSocketAddrs;
use std::time::Duration;

use crate::cip::{self, GeneralStatus};
use crate::codec::DecodeError;
use crate::io::service::UNCONNECTED_SEND;
use crate::session::{self, Session};

use super::path::{PathError, TagPath};
use super::types::{DataType, TagData};
use super::wire::{self, ReadRequest, Symbol, service};

/// The most bytes one tag read may return before the reader gives up.
pub const MAX_TAG_BYTES: usize = 1 << 20;
/// The most symbols one browse may return.
pub const MAX_SYMBOLS: usize = 1 << 16;
/// Encoded request bytes per Multiple Service Packet; leaves room under the
/// usual ~500-byte unconnected message limit for the packet header and an
/// `Unconnected_Send` wrapper.
pub const MAX_BATCH_BYTES: usize = 440;

#[derive(Debug)]
pub enum Error {
    /// Transport, encapsulation, or a reply that does not frame.
    Session(session::Error),
    /// A reply's bytes do not decode.
    Decode(DecodeError),
    /// The controller answered with a CIP error.
    Cip {
        status: GeneralStatus,
        additional: Vec<u16>,
    },
    /// The reply decodes but breaks the protocol (length lie, wrong type
    /// mid-transfer, no progress).
    Protocol(&'static str),
    Path(PathError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(e) => write!(f, "{e}"),
            Self::Decode(e) => write!(f, "reply: {e}"),
            Self::Cip { status, additional } => {
                write!(f, "cip status {status}")?;
                if !additional.is_empty() {
                    write!(f, " additional {additional:04X?}")?;
                }
                Ok(())
            }
            Self::Protocol(m) => write!(f, "protocol: {m}"),
            Self::Path(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<session::Error> for Error {
    fn from(e: session::Error) -> Self {
        Self::Session(e)
    }
}

impl From<DecodeError> for Error {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

impl From<PathError> for Error {
    fn from(e: PathError) -> Self {
        Self::Path(e)
    }
}

fn cip_error(resp: cip::Response) -> Error {
    Error::Cip {
        status: resp.status,
        additional: resp.additional_status,
    }
}

/// A read-only session to a Logix controller.
#[derive(Debug)]
pub struct TagReader {
    session: Session,
    slot: Option<u8>,
}

impl TagReader {
    /// Connect and register a session.
    ///
    /// # Errors
    /// As [`Session::connect`].
    pub fn connect<A: ToSocketAddrs>(addr: A, timeout: Duration) -> Result<Self, Error> {
        Ok(Self::new(Session::connect(addr, timeout)?))
    }

    /// Take over a registered session. It is not given back.
    #[must_use]
    pub const fn new(session: Session) -> Self {
        Self { session, slot: None }
    }

    /// Route every request out the backplane to `slot` with
    /// `Unconnected_Send` (needed when the Ethernet port is a separate
    /// module from the controller; harmless for a `CompactLogix` at slot 0).
    #[must_use]
    pub const fn with_slot(mut self, slot: u8) -> Self {
        self.slot = Some(slot);
        self
    }

    fn send(&mut self, req: ReadRequest) -> Result<cip::Response, Error> {
        let req = match self.slot {
            Some(slot) => req.routed(slot).map_err(Error::Protocol)?,
            None => req,
        };
        let resp = self.session.exchange(req.as_bytes())?;
        // A routing failure comes back as an Unconnected_Send reply.
        if self.slot.is_some()
            && req.answers() != UNCONNECTED_SEND
            && resp.service == UNCONNECTED_SEND | cip::service::REPLY_BIT
            && !resp.status.is_success()
        {
            return Err(cip_error(resp));
        }
        Ok(resp)
    }

    /// Read `elements` elements at `path` (Read Tag). A reply too large for
    /// one packet is finished with Read Tag Fragmented. A bit suffix on
    /// `path` is ignored here: the whole integer comes back.
    ///
    /// # Errors
    /// Transport, a CIP error, or a reply that breaks the protocol.
    pub fn read_tag(&mut self, path: &TagPath, elements: u16) -> Result<TagData, Error> {
        let path = path.without_bit();
        let resp = self.send(ReadRequest::read_tag(&path, elements))?;
        self.finish(&path, elements, service::READ_TAG, resp)
    }

    /// Read with Read Tag Fragmented from offset 0 (for arrays known to be
    /// larger than one packet).
    ///
    /// # Errors
    /// As [`Self::read_tag`].
    pub fn read_tag_fragmented(&mut self, path: &TagPath, elements: u16) -> Result<TagData, Error> {
        let path = path.without_bit();
        let resp = self.send(ReadRequest::read_tag_fragmented(&path, elements, 0))?;
        self.finish(&path, elements, service::READ_TAG_FRAGMENTED, resp)
    }

    /// Read many tags, batched into Multiple Service Packets. One result
    /// per item, in order: one tag failing does not fail the others. The
    /// outer `Err` is for failures that leave no per-item answer
    /// (transport, a malformed packet).
    ///
    /// # Errors
    /// See above.
    pub fn read_tags(&mut self, items: &[(TagPath, u16)]) -> Result<Vec<Result<TagData, Error>>, Error> {
        let items: Vec<(TagPath, u16)> = items.iter().map(|(p, n)| (p.without_bit(), *n)).collect();
        let mut out = Vec::with_capacity(items.len());
        let mut start = 0;
        while start < items.len() {
            let mut end = start;
            let mut size = 0;
            let mut reqs = Vec::new();
            while end < items.len() {
                let r = ReadRequest::read_tag(&items[end].0, items[end].1);
                let add = r.as_bytes().len() + 2;
                if !reqs.is_empty() && size + add > MAX_BATCH_BYTES {
                    break;
                }
                size += add;
                reqs.push(r);
                end += 1;
            }
            if reqs.len() == 1 {
                let (path, n) = &items[start];
                let resp = self.send(reqs.remove(0))?;
                out.push(self.finish(path, *n, service::READ_TAG, resp));
            } else {
                let packet = ReadRequest::multiple(&reqs).map_err(Error::Protocol)?;
                let resp = self.send(packet)?;
                let replies = wire::decode_multiple(&resp, reqs.len())?;
                for ((path, n), reply) in items[start..end].iter().zip(replies) {
                    out.push(self.finish(path, *n, service::READ_TAG, reply));
                }
            }
            start = end;
        }
        Ok(out)
    }

    /// Turn a first reply into the whole value, fetching the rest with Read
    /// Tag Fragmented while the controller says `0x06`.
    fn finish(
        &mut self,
        path: &TagPath,
        elements: u16,
        sent: u8,
        resp: cip::Response,
    ) -> Result<TagData, Error> {
        wire::check_service(&resp, sent)?;
        let mut more = resp.status == GeneralStatus::PARTIAL_TRANSFER;
        if !(resp.status.is_success() || more) {
            return Err(cip_error(resp));
        }
        // A partial reply with no room even for the type (a full Multiple
        // Service Packet) is "nothing yet": fetch it all fragmented.
        let mut ty = None;
        let mut bytes = Vec::new();
        if !(more && resp.data.is_empty()) {
            let first = wire::decode_read_data(&resp.data)?;
            ty = Some(first.ty);
            bytes = first.bytes;
        }
        let expect = |ty: Option<DataType>| match ty {
            Some(DataType::Atomic(t)) => Some(t.size() * usize::from(elements)),
            _ => None,
        };
        let too_long = |n: usize, ty| n > MAX_TAG_BYTES || expect(ty).is_some_and(|e| n > e);
        if too_long(bytes.len(), ty) {
            return Err(Error::Protocol(
                "reply holds more data than the elements requested",
            ));
        }
        while more {
            let offset = u32::try_from(bytes.len()).map_err(|_| Error::Protocol("offset overflow"))?;
            let resp = self.send(ReadRequest::read_tag_fragmented(path, elements, offset))?;
            wire::check_service(&resp, service::READ_TAG_FRAGMENTED)?;
            more = resp.status == GeneralStatus::PARTIAL_TRANSFER;
            if !(resp.status.is_success() || more) {
                return Err(cip_error(resp));
            }
            let frag = wire::decode_read_data(&resp.data)?;
            if ty.is_some_and(|t| t != frag.ty) {
                return Err(Error::Protocol("data type changed between fragments"));
            }
            ty = Some(frag.ty);
            if more && frag.bytes.is_empty() {
                return Err(Error::Protocol("partial transfer made no progress"));
            }
            bytes.extend_from_slice(&frag.bytes);
            if too_long(bytes.len(), ty) {
                return Err(Error::Protocol(
                    "reply holds more data than the elements requested",
                ));
            }
        }
        let ty = ty.ok_or(Error::Protocol("read reply carries no data type"))?;
        if expect(Some(ty)).is_some_and(|e| bytes.len() != e) {
            return Err(Error::Protocol(
                "reply holds less data than the elements requested",
            ));
        }
        Ok(TagData { ty, bytes })
    }

    /// Every symbol in controller scope, or in `program` (`"Program:Main"`),
    /// via the Symbol object. Programs show up in controller scope as
    /// `Program:<name>` entries; system symbols are included, filter with
    /// [`Symbol::is_system`].
    ///
    /// # Errors
    /// Transport, a CIP error, a bad program name, a reply that does not
    /// decode, instances that do not move forward, or more than
    /// [`MAX_SYMBOLS`] symbols.
    pub fn list_tags(&mut self, program: Option<&str>) -> Result<Vec<Symbol>, Error> {
        let mut out: Vec<Symbol> = Vec::new();
        let mut start = 0u32;
        loop {
            let resp = self.send(ReadRequest::symbol_list(program, start)?)?;
            wire::check_service(&resp, service::GET_INSTANCE_ATTRIBUTE_LIST)?;
            let more = resp.status == GeneralStatus::PARTIAL_TRANSFER;
            if !(resp.status.is_success() || more) {
                return Err(cip_error(resp));
            }
            let symbols = wire::decode_symbols(&resp.data)?;
            if more && symbols.is_empty() {
                return Err(Error::Protocol("partial symbol reply with no symbols"));
            }
            for s in symbols {
                if s.instance < start {
                    return Err(Error::Protocol("symbol instances do not move forward"));
                }
                start = s
                    .instance
                    .checked_add(1)
                    .ok_or(Error::Protocol("symbol instance overflow"))?;
                out.push(s);
                if out.len() > MAX_SYMBOLS {
                    return Err(Error::Protocol("symbol table too large"));
                }
            }
            if !more {
                return Ok(out);
            }
        }
    }

    /// `Get_Attribute_Single`, routed like every other request.
    ///
    /// # Errors
    /// Transport or a CIP error.
    pub fn get_attribute_single(
        &mut self,
        class: u16,
        instance: u16,
        attribute: u16,
    ) -> Result<Vec<u8>, Error> {
        let req = ReadRequest::get_attribute_single(class, instance, attribute);
        let answers = req.answers();
        let resp = self.send(req)?;
        wire::check_service(&resp, answers)?;
        if resp.status.is_success() {
            Ok(resp.data)
        } else {
            Err(cip_error(resp))
        }
    }

    /// Unregister and close.
    pub fn close(self) {
        self.session.close();
    }
}
