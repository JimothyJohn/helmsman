//! A test stand-in for a Logix controller's tag services, since `OpENer`
//! cannot serve symbolic tags. Sans-IO like [`crate::Device`]: give it a
//! Message Router request's bytes, get a reply. Attach it to a device with
//! [`crate::Device::with_logix`] to serve it over the network.
//!
//! What it answers, and how (layouts as in `helmsman_proto::logix`; error
//! choices follow the general status meanings in pycomm3
//! `cip/status_info.py` and are this fixture's reading of them, not
//! verified against a controller):
//!
//! - Read Tag (`0x4C`) and Read Tag Fragmented (`0x52`) on symbolic paths
//!   with array indexes and structure members, controller or program scope
//!   (`Program:Main.Tag`), names case-insensitive. Unknown tag or member:
//!   `0x05`. Index on a non-array, wrong index count, member of an array
//!   without an index: `0x04`. Index past the end, or more elements than
//!   remain: `0xFF` with extended status `0x2105`. Missing request data:
//!   `0x13`. A reply that does not fit `max_reply`: `0x06` with what fits
//!   (whole elements when one fits).
//! - Multiple Service Packet (`0x0A`) to the Message Router: replies share
//!   the size budget; any embedded failure makes the packet status `0x1E`.
//! - Get Instance Attribute List (`0x55`) on the Symbol object (`0x6B`),
//!   attributes 1 (name), 2 (type), 8 (dimensions) in the order asked;
//!   `0x06` while more symbols remain. Instance ids are sparse, as on a
//!   real controller.
//! - `Unconnected_Send` (`0x52` on the Connection Manager) whose route is
//!   port 1 (backplane), link = `slot`; other routes get `0x01` with
//!   extended status `0x0311` (port) or `0x0312` (link).
//! - Anything else on a symbolic path, Write Tag included: `0x08`. Every
//!   service code that arrives is counted, so a test can prove a client
//!   never asked to write.

use std::collections::BTreeMap;

use helmsman_proto::cip::{GeneralStatus, Response, class, service};
use helmsman_proto::codec::{DecodeError, Reader, Writer};
use helmsman_proto::io::service::UNCONNECTED_SEND;
use helmsman_proto::logix::path::{Segment, decode_segments};
use helmsman_proto::logix::wire::{self, SYMBOL_CLASS, Symbol, service as logix_service};
use helmsman_proto::logix::{AtomicType, DataType, TagPath};

/// Extended status: access beyond end of the object.
pub const EXT_BEYOND_END: u16 = 0x2105;
/// General status for "see extended status".
pub const GENERAL_ERROR: GeneralStatus = GeneralStatus(0xFF);
/// Bytes of a Message Router reply header without additional status.
const REPLY_HEADER: usize = 4;

/// What a tag (or a structure member) holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Atomic(AtomicType),
    /// Members laid out back to back, no padding.
    Struct {
        handle: u16,
        members: Vec<(String, TagDef)>,
    },
}

/// A tag's type and shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDef {
    pub kind: Kind,
    /// Empty for a scalar; at most three.
    pub dims: Vec<u32>,
}

impl TagDef {
    #[must_use]
    pub const fn atomic(t: AtomicType) -> Self {
        Self {
            kind: Kind::Atomic(t),
            dims: Vec::new(),
        }
    }

    #[must_use]
    pub fn array(t: AtomicType, dims: &[u32]) -> Self {
        Self {
            kind: Kind::Atomic(t),
            dims: dims.to_vec(),
        }
    }

    #[must_use]
    pub fn structure(handle: u16, members: Vec<(&str, Self)>) -> Self {
        Self {
            kind: Kind::Struct {
                handle,
                members: members.into_iter().map(|(n, d)| (n.to_string(), d)).collect(),
            },
            dims: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_dims(mut self, dims: &[u32]) -> Self {
        self.dims = dims.to_vec();
        self
    }

    fn data_type(&self) -> DataType {
        match &self.kind {
            Kind::Atomic(t) => DataType::Atomic(*t),
            Kind::Struct { handle, .. } => DataType::Struct { handle: *handle },
        }
    }

    fn elem_size(&self) -> usize {
        match &self.kind {
            Kind::Atomic(t) => t.size(),
            Kind::Struct { members, .. } => members.iter().map(|(_, d)| d.size()).sum(),
        }
    }

    fn count(&self) -> usize {
        self.dims.iter().map(|d| *d as usize).product()
    }

    fn size(&self) -> usize {
        self.elem_size() * self.count()
    }
}

#[derive(Debug, Clone)]
struct Tag {
    name: String,
    instance: u32,
    def: TagDef,
    data: Vec<u8>,
}

/// Where a path lands inside a tag.
struct Loc {
    scope: Option<usize>,
    tag: usize,
    ty: DataType,
    offset: usize,
    elem: usize,
    avail: usize,
}

fn fail(service: u8, status: GeneralStatus, ext: &[u16]) -> Response {
    let mut r = Response::error(service, status);
    r.additional_status = ext.to_vec();
    r
}

fn reply_len(r: &Response) -> usize {
    REPLY_HEADER + 2 * r.additional_status.len() + r.data.len()
}

/// The fixture controller.
#[derive(Debug, Clone)]
pub struct Controller {
    tags: Vec<Tag>,
    programs: Vec<(String, u32, Vec<Tag>)>,
    next_instance: u32,
    /// Largest Message Router reply, in bytes, header included.
    pub max_reply: usize,
    /// Backplane slot an `Unconnected_Send` must route to.
    pub slot: u8,
    seen: BTreeMap<u8, usize>,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tags: Vec::new(),
            programs: Vec::new(),
            next_instance: 0x21,
            max_reply: 500,
            slot: 0,
            seen: BTreeMap::new(),
        }
    }

    fn instance(&mut self) -> u32 {
        let i = self.next_instance;
        // Sparse, as on a controller: a client must continue from the last
        // instance it saw, not count.
        self.next_instance += 0x0101;
        i
    }

    /// Add a controller-scope tag, zeroed.
    pub fn add(&mut self, name: &str, def: TagDef) -> &mut Self {
        let instance = self.instance();
        self.tags.push(Tag {
            name: name.into(),
            instance,
            data: vec![0; def.size()],
            def,
        });
        self
    }

    /// Add a tag in program `program` (without the `Program:` prefix).
    pub fn add_program_tag(&mut self, program: &str, name: &str, def: TagDef) -> &mut Self {
        let instance = self.instance();
        let tag = Tag {
            name: name.into(),
            instance,
            data: vec![0; def.size()],
            def,
        };
        if let Some(p) = self
            .programs
            .iter_mut()
            .find(|p| p.0.eq_ignore_ascii_case(program))
        {
            p.2.push(tag);
        } else {
            let pi = self.instance();
            self.programs.push((program.into(), pi, vec![tag]));
        }
        self
    }

    /// Overwrite the bytes at `path` (the controller's own logic changing a
    /// value; nothing a client can do).
    ///
    /// # Errors
    /// A path that does not resolve, or bytes that run past the tag.
    pub fn set(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let p = TagPath::parse(path).map_err(|e| e.to_string())?;
        let loc = self.resolve(&p).map_err(|(s, _)| format!("{path}: {s}"))?;
        let data = self.data_mut(loc.scope, loc.tag);
        let end = loc.offset + bytes.len();
        if end > data.len() {
            return Err(format!("{path}: {} bytes past the end", end - data.len()));
        }
        data[loc.offset..end].copy_from_slice(bytes);
        Ok(())
    }

    /// How many times each service code has arrived, embedded ones included.
    #[must_use]
    pub const fn services_seen(&self) -> &BTreeMap<u8, usize> {
        &self.seen
    }

    fn data(&self, scope: Option<usize>, tag: usize) -> &[u8] {
        match scope {
            None => &self.tags[tag].data,
            Some(p) => &self.programs[p].2[tag].data,
        }
    }

    fn data_mut(&mut self, scope: Option<usize>, tag: usize) -> &mut Vec<u8> {
        match scope {
            None => &mut self.tags[tag].data,
            Some(p) => &mut self.programs[p].2[tag].data,
        }
    }

    fn resolve(&self, path: &TagPath) -> Result<Loc, (GeneralStatus, Vec<u16>)> {
        let not_found = || (GeneralStatus::PATH_DESTINATION_UNKNOWN, vec![]);
        let syntax = || (GeneralStatus::PATH_SEGMENT_ERROR, vec![]);
        let els = path.elements();
        let (scope, tags, rest) = match els[0].name.get(..8) {
            Some(p) if p.eq_ignore_ascii_case("Program:") => {
                if !els[0].index.is_empty() || els.len() < 2 {
                    return Err(syntax());
                }
                let name = &els[0].name[8..];
                let pi = self
                    .programs
                    .iter()
                    .position(|p| p.0.eq_ignore_ascii_case(name))
                    .ok_or_else(not_found)?;
                (Some(pi), &self.programs[pi].2, &els[1..])
            }
            _ => (None, &self.tags, els),
        };
        let ti = tags
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(&rest[0].name))
            .ok_or_else(not_found)?;
        let mut def = &tags[ti].def;
        let mut offset = 0usize;
        let mut avail;
        let mut indexed;
        let index = |def: &TagDef,
                     idx: &[u32],
                     offset: &mut usize|
         -> Result<(usize, bool), (GeneralStatus, Vec<u16>)> {
            if idx.is_empty() {
                return Ok((def.count(), def.dims.is_empty()));
            }
            if idx.len() != def.dims.len() {
                return Err(syntax());
            }
            let mut flat = 0usize;
            for (i, d) in idx.iter().zip(&def.dims) {
                if i >= d {
                    return Err((GENERAL_ERROR, vec![EXT_BEYOND_END]));
                }
                flat = flat * (*d as usize) + *i as usize;
            }
            *offset += flat * def.elem_size();
            Ok((def.count() - flat, true))
        };
        (avail, indexed) = index(def, &rest[0].index, &mut offset)?;
        for el in &rest[1..] {
            let Kind::Struct { members, .. } = &def.kind else {
                return Err(not_found());
            };
            if !indexed {
                return Err(syntax());
            }
            let mut moff = 0;
            let mut found = None;
            for (name, m) in members {
                if name.eq_ignore_ascii_case(&el.name) {
                    found = Some(m);
                    break;
                }
                moff += m.size();
            }
            def = found.ok_or_else(not_found)?;
            offset += moff;
            (avail, indexed) = index(def, &el.index, &mut offset)?;
        }
        Ok(Loc {
            scope,
            tag: ti,
            ty: def.data_type(),
            offset,
            elem: def.elem_size(),
            avail,
        })
    }

    /// Answer a request if it is one of this fixture's; `None` hands it back
    /// to the device (Identity, assemblies, `Forward_Open`, ...).
    pub fn handle(&mut self, mr: &[u8]) -> Option<Response> {
        if !Self::is_ours(mr, 0) {
            return None;
        }
        Some(self.dispatch(mr, self.max_reply.saturating_sub(REPLY_HEADER), 0))
    }

    fn split(mr: &[u8]) -> Result<(u8, Vec<Segment>, &[u8]), DecodeError> {
        let mut r = Reader::new(mr);
        let svc = r.u8()?;
        let words = r.u8()?;
        let segs = decode_segments(&mut r, words)?;
        Ok((svc, segs, r.rest()))
    }

    fn is_ours(mr: &[u8], depth: u8) -> bool {
        let Ok((svc, segs, data)) = Self::split(mr) else {
            // A symbolic path we cannot even decode is still ours to refuse.
            return mr.get(2) == Some(&0x91);
        };
        match segs.as_slice() {
            [Segment::Class(c), Segment::Instance(1)] if *c == u32::from(class::MESSAGE_ROUTER) => {
                svc == service::MULTIPLE_SERVICE_PACKET
            }
            [Segment::Class(c), Segment::Instance(1)] if *c == u32::from(class::CONNECTION_MANAGER) => {
                svc == UNCONNECTED_SEND
                    && depth == 0
                    && Self::unwrap_send(data).is_ok_and(|(inner, _)| Self::is_ours(inner, 1))
            }
            s => s
                .iter()
                .any(|x| matches!(x, Segment::Symbolic(_)) || *x == Segment::Class(u32::from(SYMBOL_CLASS))),
        }
    }

    fn unwrap_send(data: &[u8]) -> Result<(&[u8], &[u8]), DecodeError> {
        let mut r = Reader::new(data);
        let _tick = r.u8()?;
        let _ticks = r.u8()?;
        let n = usize::from(r.u16_le()?);
        let inner = r.bytes(n)?;
        if n % 2 == 1 {
            r.u8()?;
        }
        let words = usize::from(r.u8()?);
        let _reserved = r.u8()?;
        let route = r.bytes(words * 2)?;
        Ok((inner, route))
    }

    /// `budget` is reply data bytes allowed (header excluded).
    fn dispatch(&mut self, mr: &[u8], budget: usize, depth: u8) -> Response {
        let svc = mr.first().copied().unwrap_or(0) & !service::REPLY_BIT;
        *self.seen.entry(svc).or_default() += 1;
        let (svc, segs, data) = match Self::split(mr) {
            Ok(x) => x,
            Err(DecodeError::Truncated { .. }) => return fail(svc, GeneralStatus::NOT_ENOUGH_DATA, &[]),
            Err(DecodeError::Invalid(_)) => return fail(svc, GeneralStatus::PATH_SEGMENT_ERROR, &[]),
        };
        let mr_class = u32::from(class::MESSAGE_ROUTER);
        let cm_class = u32::from(class::CONNECTION_MANAGER);
        match segs.as_slice() {
            [Segment::Class(c), Segment::Instance(1)]
                if *c == mr_class && svc == service::MULTIPLE_SERVICE_PACKET =>
            {
                if depth > 0 {
                    return fail(svc, GeneralStatus::SERVICE_NOT_SUPPORTED, &[]);
                }
                self.multiple(data, budget)
            }
            [Segment::Class(c), Segment::Instance(1)] if *c == cm_class && svc == UNCONNECTED_SEND => {
                if depth > 0 {
                    return fail(svc, GeneralStatus::PATH_SEGMENT_ERROR, &[]);
                }
                self.unconnected_send(data, budget)
            }
            [prefix @ .., Segment::Class(c), Segment::Instance(start)] if *c == u32::from(SYMBOL_CLASS) => {
                if svc != logix_service::GET_INSTANCE_ATTRIBUTE_LIST {
                    return fail(svc, GeneralStatus::SERVICE_NOT_SUPPORTED, &[]);
                }
                let scope = match prefix {
                    [] => None,
                    [Segment::Symbolic(p)] => {
                        let name = p
                            .get(8..)
                            .filter(|_| p.get(..8).is_some_and(|x| x.eq_ignore_ascii_case("Program:")));
                        match name
                            .and_then(|n| self.programs.iter().position(|q| q.0.eq_ignore_ascii_case(n)))
                        {
                            Some(i) => Some(i),
                            None => return fail(svc, GeneralStatus::PATH_DESTINATION_UNKNOWN, &[]),
                        }
                    }
                    _ => return fail(svc, GeneralStatus::PATH_SEGMENT_ERROR, &[]),
                };
                self.symbols(scope, *start, data, budget)
            }
            segs => {
                let Some(path) = TagPath::from_segments(segs) else {
                    return fail(svc, GeneralStatus::PATH_SEGMENT_ERROR, &[]);
                };
                match svc {
                    logix_service::READ_TAG => self.read(&path, data, budget, false),
                    logix_service::READ_TAG_FRAGMENTED => self.read(&path, data, budget, true),
                    _ => fail(svc, GeneralStatus::SERVICE_NOT_SUPPORTED, &[]),
                }
            }
        }
    }

    fn read(&self, path: &TagPath, data: &[u8], budget: usize, fragmented: bool) -> Response {
        let svc = if fragmented {
            logix_service::READ_TAG_FRAGMENTED
        } else {
            logix_service::READ_TAG
        };
        let mut r = Reader::new(data);
        let Ok(n) = r.u16_le() else {
            return fail(svc, GeneralStatus::NOT_ENOUGH_DATA, &[]);
        };
        let offset = if fragmented {
            match r.u32_le() {
                Ok(o) => o as usize,
                Err(_) => return fail(svc, GeneralStatus::NOT_ENOUGH_DATA, &[]),
            }
        } else {
            0
        };
        if !r.is_empty() {
            return fail(svc, GeneralStatus::TOO_MUCH_DATA, &[]);
        }
        let loc = match self.resolve(path) {
            Ok(l) => l,
            Err((s, ext)) => return fail(svc, s, &ext),
        };
        let n = usize::from(n);
        if n == 0 || n > loc.avail {
            return fail(svc, GENERAL_ERROR, &[EXT_BEYOND_END]);
        }
        let total = n * loc.elem;
        if offset > total {
            return fail(svc, GENERAL_ERROR, &[EXT_BEYOND_END]);
        }
        let all = &self.data(loc.scope, loc.tag)[loc.offset + offset..loc.offset + total];
        let room = budget.saturating_sub(loc.ty.header_len());
        let (chunk, status) = if all.len() <= room {
            (all, GeneralStatus::SUCCESS)
        } else {
            let whole = if loc.elem <= room {
                room - room % loc.elem
            } else {
                room
            };
            (&all[..whole], GeneralStatus::PARTIAL_TRANSFER)
        };
        if budget < loc.ty.header_len() {
            return fail(svc, GeneralStatus::PARTIAL_TRANSFER, &[]);
        }
        Response {
            service: svc | service::REPLY_BIT,
            status,
            additional_status: vec![],
            data: wire::encode_read_data(loc.ty, chunk),
        }
    }

    fn multiple(&mut self, data: &[u8], budget: usize) -> Response {
        let svc = service::MULTIPLE_SERVICE_PACKET;
        let parts = match wire::split_multiple(data) {
            Ok(p) => p,
            Err(DecodeError::Truncated { .. }) => return fail(svc, GeneralStatus::NOT_ENOUGH_DATA, &[]),
            Err(DecodeError::Invalid(_)) => return fail(svc, GeneralStatus::INVALID_PARAMETER, &[]),
        };
        let mut left = budget.saturating_sub(2 + 2 * parts.len());
        let mut replies = Vec::with_capacity(parts.len());
        let mut any_failed = false;
        for p in parts {
            let r = self.dispatch(p, left.saturating_sub(REPLY_HEADER), 1);
            left = left.saturating_sub(reply_len(&r));
            any_failed |= !r.status.is_success();
            let mut w = Writer::default();
            r.encode(&mut w);
            replies.push(w.into_inner());
        }
        Response {
            service: svc | service::REPLY_BIT,
            status: if any_failed {
                GeneralStatus::EMBEDDED_SERVICE_ERROR
            } else {
                GeneralStatus::SUCCESS
            },
            additional_status: vec![],
            data: wire::join_multiple(&replies),
        }
    }

    fn unconnected_send(&mut self, data: &[u8], budget: usize) -> Response {
        let Ok((inner, route)) = Self::unwrap_send(data) else {
            return fail(UNCONNECTED_SEND, GeneralStatus::NOT_ENOUGH_DATA, &[]);
        };
        match route {
            [1, link] if *link == self.slot => self.dispatch(inner, budget, 0),
            [1, _] => fail(UNCONNECTED_SEND, GeneralStatus::CONNECTION_FAILURE, &[0x0312]),
            _ => fail(UNCONNECTED_SEND, GeneralStatus::CONNECTION_FAILURE, &[0x0311]),
        }
    }

    fn symbols(&self, scope: Option<usize>, start: u32, data: &[u8], budget: usize) -> Response {
        let svc = logix_service::GET_INSTANCE_ATTRIBUTE_LIST;
        let mut r = Reader::new(data);
        let attrs: Result<Vec<u16>, DecodeError> = (|| {
            let n = r.u16_le()?;
            (0..n).map(|_| r.u16_le()).collect()
        })();
        let Ok(attrs) = attrs else {
            return fail(svc, GeneralStatus::NOT_ENOUGH_DATA, &[]);
        };
        if attrs.is_empty() || attrs.iter().any(|a| !matches!(a, 1 | 2 | 8)) {
            return fail(svc, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED, &[]);
        }
        let mut entries: Vec<Symbol> = Vec::new();
        let tags = match scope {
            None => &self.tags,
            Some(p) => &self.programs[p].2,
        };
        for t in tags {
            let mut dims = [0u32; 3];
            for (d, v) in dims.iter_mut().zip(&t.def.dims) {
                *d = *v;
            }
            entries.push(Symbol {
                instance: t.instance,
                name: t.name.clone(),
                symbol_type: Symbol::type_word(t.def.data_type(), t.def.dims.len()),
                dims,
            });
        }
        if scope.is_none() {
            for (name, instance, _) in &self.programs {
                entries.push(Symbol {
                    instance: *instance,
                    name: format!("Program:{name}"),
                    // Programs are reported with a structure type; the
                    // handle is the fixture's own.
                    symbol_type: 0x8068,
                    dims: [0; 3],
                });
            }
        }
        entries.sort_by_key(|s| s.instance);
        let mut w = Writer::default();
        let mut status = GeneralStatus::SUCCESS;
        for s in entries.iter().filter(|s| s.instance >= start) {
            let mut e = Writer::default();
            e.u32_le(s.instance);
            for a in &attrs {
                match a {
                    1 => {
                        e.u16_le(u16::try_from(s.name.len()).unwrap_or(0))
                            .bytes(s.name.as_bytes());
                    }
                    2 => {
                        e.u16_le(s.symbol_type);
                    }
                    _ => {
                        for d in s.dims {
                            e.u32_le(d);
                        }
                    }
                }
            }
            if w.len() + e.len() > budget {
                status = GeneralStatus::PARTIAL_TRANSFER;
                break;
            }
            w.bytes(&e.into_inner());
        }
        Response {
            service: svc | service::REPLY_BIT,
            status,
            additional_status: vec![],
            data: w.into_inner(),
        }
    }
}
