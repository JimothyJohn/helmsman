//! A session and its services: discovery, browse, read, the write that is
//! always refused, and subscriptions.
//!
//! **Read-only.** Write answers `BadNotWritable` for every node that
//! exists (`BadNodeIdUnknown` otherwise); every other service that could
//! change something (`AddNodes`, `Call`, `HistoryUpdate`, ...) is not
//! implemented and answers `BadServiceUnsupported`. Nothing a client sends
//! reaches the runtime: the server only ever reads the published
//! [`super::feed::Snapshot`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::Ctx;
use super::codec::{DataValue, NodeId, Reader, Writer, date_time};
use super::space::{self, Class, Space, Stamps, attr};
use super::status as sc;

/// Service request and response encoding ids (Part 6 `NodeIds.csv`).
pub mod svc {
    pub const SERVICE_FAULT: u32 = 397;
    pub const FIND_SERVERS: u32 = 422;
    pub const GET_ENDPOINTS: u32 = 428;
    pub const OPEN_SECURE_CHANNEL: u32 = 446;
    pub const OPEN_SECURE_CHANNEL_RESPONSE: u32 = 449;
    pub const CLOSE_SECURE_CHANNEL: u32 = 452;
    pub const CREATE_SESSION: u32 = 461;
    pub const ACTIVATE_SESSION: u32 = 467;
    pub const CLOSE_SESSION: u32 = 473;
    pub const CANCEL: u32 = 479;
    pub const BROWSE: u32 = 527;
    pub const BROWSE_NEXT: u32 = 533;
    pub const TRANSLATE: u32 = 554;
    pub const REGISTER_NODES: u32 = 560;
    pub const UNREGISTER_NODES: u32 = 566;
    pub const READ: u32 = 631;
    pub const WRITE: u32 = 673;
    pub const CREATE_MONITORED_ITEMS: u32 = 751;
    pub const MODIFY_MONITORED_ITEMS: u32 = 763;
    pub const SET_MONITORING_MODE: u32 = 769;
    pub const DELETE_MONITORED_ITEMS: u32 = 781;
    pub const CREATE_SUBSCRIPTION: u32 = 787;
    pub const MODIFY_SUBSCRIPTION: u32 = 793;
    pub const SET_PUBLISHING_MODE: u32 = 799;
    pub const PUBLISH: u32 = 826;
    pub const REPUBLISH: u32 = 832;
    pub const DELETE_SUBSCRIPTIONS: u32 = 847;
    pub const DATA_CHANGE_NOTIFICATION: u32 = 811;
    pub const DATA_CHANGE_FILTER: u32 = 724;
    pub const ANONYMOUS_IDENTITY_TOKEN: u32 = 321;
    pub const USER_NAME_IDENTITY_TOKEN: u32 = 324;
    pub const X509_IDENTITY_TOKEN: u32 = 327;
    pub const ISSUED_IDENTITY_TOKEN: u32 = 940;

    /// A request's response encoding id is three past its own.
    #[must_use]
    pub const fn response(request: u32) -> u32 {
        request + 3
    }
}

pub const SECURITY_NONE: &str = "http://opcfoundation.org/UA/SecurityPolicy#None";
const TRANSPORT_PROFILE: &str = "http://opcfoundation.org/UA-Profile/Transport/uatcp-uasc-uabinary";
const ANONYMOUS_POLICY: &str = "anonymous";

/// A request's header: what the server uses of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub auth: NodeId,
    pub handle: u32,
    pub timeout_hint: u32,
}

impl Header {
    /// # Errors
    /// Malformed.
    pub fn decode(r: &mut Reader<'_>) -> super::codec::Result<Self> {
        let auth = r.node_id()?;
        r.i64()?;
        let handle = r.u32()?;
        r.u32()?;
        r.string()?;
        let timeout_hint = r.u32()?;
        r.extension_object()?;
        Ok(Self {
            auth,
            handle,
            timeout_hint,
        })
    }
}

/// A response to send on the channel: its body starts with the response
/// type's encoding id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub request_id: u32,
    pub handle: u32,
    pub body: Vec<u8>,
}

/// The start of a response: its type and a Good header.
#[must_use]
pub fn response(ty: u32, handle: u32, now_ms: i64) -> Writer {
    response_with(ty, handle, sc::GOOD, now_ms)
}

fn response_with(ty: u32, handle: u32, status: u32, now_ms: i64) -> Writer {
    let mut w = Writer::new();
    w.node_id(&NodeId::ns0(ty))
        .i64(date_time(now_ms))
        .u32(handle)
        .u32(status)
        .no_diagnostics()
        .null_array()
        .extension_object(&NodeId::ns0(0), None);
    w
}

/// A `ServiceFault` with `status`.
#[must_use]
pub fn fault(handle: u32, status: u32, now_ms: i64) -> Vec<u8> {
    response_with(svc::SERVICE_FAULT, handle, status, now_ms).buf
}

fn application_description(w: &mut Writer, url: &str) {
    w.string(Some(space::APP_URI))
        .string(Some("urn:helmsman"))
        .localized_text(Some("Helmsman PLC"))
        .u32(0)
        .string(None)
        .string(None)
        .len(1)
        .string(Some(url));
}

fn endpoint_description(w: &mut Writer, url: &str) {
    w.string(Some(url));
    application_description(w, url);
    w.bytes(None)
        .u32(1)
        .string(Some(SECURITY_NONE))
        .len(1)
        .string(Some(ANONYMOUS_POLICY))
        .u32(0)
        .string(None)
        .string(None)
        .string(None)
        .string(Some(TRANSPORT_PROFILE))
        .u8(0);
}

/// The URL to describe endpoints with: the one the client used, when it
/// is a plausible `opc.tcp` URL, so a box bound to `0.0.0.0` answers with
/// the address the client reached it at.
fn endpoint_url<'a>(asked: Option<&'a str>, ctx: &'a Ctx) -> &'a str {
    asked
        .filter(|u| u.len() <= 4096 && u.starts_with("opc.tcp://"))
        .unwrap_or(&ctx.endpoint)
}

/// `GetEndpoints` and `FindServers`, which need no session.
///
/// # Errors
/// The status for a service fault.
pub fn discovery(ctx: &Ctx, ty: u32, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
    let asked = r.string().map_err(|_| sc::BAD_DECODING_ERROR)?;
    r.vec(Reader::string).map_err(|_| sc::BAD_DECODING_ERROR)?;
    let filter = r.vec(Reader::string).map_err(|_| sc::BAD_DECODING_ERROR)?;
    let url = endpoint_url(asked.as_deref(), ctx);
    let mut w = response(svc::response(ty), hdr.handle, ctx.now_ms());
    if ty == svc::GET_ENDPOINTS {
        let wanted = filter.is_empty() || filter.iter().any(|p| p.as_deref() == Some(TRANSPORT_PROFILE));
        if wanted {
            w.len(1);
            endpoint_description(&mut w, url);
        } else {
            w.len(0);
        }
    } else {
        w.len(1);
        application_description(&mut w, url);
    }
    Ok(w.buf)
}

/// A publish request waiting for something to answer it with.
#[derive(Debug)]
struct Pending {
    request_id: u32,
    handle: u32,
    acks: Vec<u32>,
    deadline: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Disabled,
    Sampling,
    Reporting,
}

impl Mode {
    const fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Disabled),
            1 => Some(Self::Sampling),
            2 => Some(Self::Reporting),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Item {
    id: u32,
    node: NodeId,
    handle: u32,
    mode: Mode,
    stamps: Stamps,
    /// Value and status last sampled; `None` before the first sample.
    last: Option<(Option<super::codec::Variant>, u32)>,
    /// Queue of one: the newest change not yet reported.
    queued: Option<DataValue>,
}

#[derive(Debug)]
struct Sub {
    id: u32,
    interval: Duration,
    keep_alive: u32,
    lifetime: u32,
    per_publish: u32,
    enabled: bool,
    items: Vec<Item>,
    next_item: u32,
    /// The sequence number the next notification gets.
    seq: u32,
    due: Instant,
    /// Intervals since the last message sent.
    quiet: u32,
    /// Intervals without a publish request to answer with.
    starved: u32,
    /// Something to send (data or a keep-alive) but no publish request.
    late: bool,
    /// Sent notification messages a client may still acknowledge or ask
    /// again for, newest last.
    retained: VecDeque<(u32, Vec<u8>)>,
}

/// Messages kept per subscription for `Republish`.
const RETAINED: usize = 10;

impl Sub {
    fn next_seq(&mut self) -> u32 {
        let s = self.seq;
        self.seq = self.seq.wrapping_add(1).max(1);
        s
    }
}

/// A browse continuation point: references not yet returned.
#[derive(Debug)]
struct Point {
    id: Vec<u8>,
    max: usize,
    rest: Vec<Vec<u8>>,
}

/// One session, on the secure channel that created it.
#[derive(Debug)]
pub struct Session {
    pub id: NodeId,
    pub token: NodeId,
    pub active: bool,
    pub timeout: Duration,
    pub last: Instant,
    subs: Vec<Sub>,
    publish: VecDeque<Pending>,
    points: Vec<Point>,
    next_point: u64,
}

fn decode<T>(v: super::codec::Result<T>) -> Result<T, u32> {
    v.map_err(|_| sc::BAD_DECODING_ERROR)
}

/// A client-supplied millisecond duration as a `Duration` within `lo..=hi`
/// (`NaN`, negative or zero take `lo`).
fn millis(v: f64, lo: Duration, hi: Duration) -> Duration {
    if !v.is_finite() || v <= 0.0 {
        return lo;
    }
    let ms = v.min(hi.as_secs_f64() * 1000.0);
    Duration::from_secs_f64(ms / 1000.0).clamp(lo, hi)
}

fn ms_f64(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Session {
    /// `CreateSession`: a new session, not yet activated, and the response.
    ///
    /// # Errors
    /// The status for a service fault.
    pub fn create(ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>, now: Instant) -> Result<(Self, Vec<u8>), u32> {
        // ApplicationDescription
        decode(r.string())?;
        decode(r.string())?;
        decode(r.localized_text())?;
        decode(r.u32())?;
        decode(r.string())?;
        decode(r.string())?;
        decode(r.vec(Reader::string))?;
        decode(r.string())?;
        let asked = decode(r.string())?;
        decode(r.string())?;
        decode(r.bytes())?;
        decode(r.bytes())?;
        let timeout = millis(
            decode(r.f64())?,
            Duration::from_secs(10),
            Duration::from_secs(600),
        );
        decode(r.u32())?;
        let id = NodeId::Num(1, ctx.next_id());
        let token = NodeId::Bytes(0, ctx.random(32));
        let mut w = response(svc::response(svc::CREATE_SESSION), hdr.handle, ctx.now_ms());
        w.node_id(&id)
            .node_id(&token)
            .f64(ms_f64(timeout))
            .bytes(Some(&ctx.random(32)))
            .bytes(None)
            .len(1);
        endpoint_description(&mut w, endpoint_url(asked.as_deref(), ctx));
        // serverSoftwareCertificates, serverSignature, maxRequestMessageSize
        w.null_array()
            .string(None)
            .bytes(None)
            .u32(ctx.limits.max_message);
        let s = Self {
            id,
            token,
            active: false,
            timeout,
            last: now,
            subs: Vec::new(),
            publish: VecDeque::new(),
            points: Vec::new(),
            next_point: 0,
        };
        Ok((s, w.buf))
    }

    /// `ActivateSession`: anonymous only.
    ///
    /// # Errors
    /// The status for a service fault.
    pub fn activate(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        decode(r.string())?;
        decode(r.bytes())?;
        decode(r.vec(|r| {
            r.bytes()?;
            r.bytes()
        }))?;
        decode(r.vec(Reader::string))?;
        let (ty, body) = decode(r.extension_object())?;
        decode(r.string())?;
        decode(r.bytes())?;
        match ty.ns0_id() {
            Some(0) if body.is_none() => {}
            Some(svc::ANONYMOUS_IDENTITY_TOKEN) => {
                let policy = body
                    .as_deref()
                    .map(|b| Reader::new(b).string())
                    .transpose()
                    .map_err(|_| sc::BAD_IDENTITY_TOKEN_INVALID)?
                    .flatten();
                if !policy
                    .as_deref()
                    .is_none_or(|p| p.is_empty() || p == ANONYMOUS_POLICY)
                {
                    return Err(sc::BAD_IDENTITY_TOKEN_INVALID);
                }
            }
            Some(svc::USER_NAME_IDENTITY_TOKEN | svc::X509_IDENTITY_TOKEN | svc::ISSUED_IDENTITY_TOKEN) => {
                return Err(sc::BAD_IDENTITY_TOKEN_REJECTED);
            }
            _ => return Err(sc::BAD_IDENTITY_TOKEN_INVALID),
        }
        self.active = true;
        let mut w = response(svc::response(svc::ACTIVATE_SESSION), hdr.handle, ctx.now_ms());
        w.bytes(Some(&ctx.random(32))).null_array().null_array();
        Ok(w.buf)
    }

    /// Answers every waiting publish request with `status`: the session is
    /// going away.
    pub fn close(&mut self, ctx: &Ctx, status: u32, out: &mut Vec<Reply>) {
        for p in self.publish.drain(..) {
            out.push(Reply {
                request_id: p.request_id,
                handle: p.handle,
                body: fault(p.handle, status, ctx.now_ms()),
            });
        }
        self.subs.clear();
    }

    /// One service of an activated session. `Publish` may answer later
    /// (from [`Session::tick`]), so replies go to `out`.
    ///
    /// # Errors
    /// The status for a service fault.
    #[allow(clippy::too_many_arguments)] // one request's parts, and where its replies go
    pub fn service(
        &mut self,
        ctx: &Ctx,
        ty: u32,
        hdr: &Header,
        request_id: u32,
        r: &mut Reader<'_>,
        now: Instant,
        out: &mut Vec<Reply>,
    ) -> Result<(), u32> {
        let (snap, space) = ctx.view();
        let rc = space::ReadCtx {
            snap: &snap,
            about: &ctx.about,
            now_ms: ctx.now_ms(),
            sampling_ms: ms_f64(ctx.limits.min_interval),
        };
        let body = match ty {
            svc::BROWSE => self.browse(ctx, &space, hdr, r)?,
            svc::BROWSE_NEXT => self.browse_next(ctx, hdr, r)?,
            svc::READ => read(ctx, &space, &rc, hdr, r)?,
            svc::WRITE => write(ctx, &space, hdr, r)?,
            svc::TRANSLATE => translate(ctx, &space, hdr, r)?,
            svc::REGISTER_NODES => {
                let ids = decode(r.vec(Reader::node_id))?;
                check_ops(ctx, ids.len())?;
                let mut w = response(svc::response(ty), hdr.handle, rc.now_ms);
                w.len(ids.len());
                for id in &ids {
                    w.node_id(id);
                }
                w.buf
            }
            svc::UNREGISTER_NODES => {
                decode(r.vec(Reader::node_id))?;
                response(svc::response(ty), hdr.handle, rc.now_ms).buf
            }
            svc::CANCEL => {
                decode(r.u32())?;
                let mut w = response(svc::response(ty), hdr.handle, rc.now_ms);
                w.u32(0);
                w.buf
            }
            svc::CREATE_SUBSCRIPTION => self.create_subscription(ctx, hdr, r, now)?,
            svc::MODIFY_SUBSCRIPTION => self.modify_subscription(ctx, hdr, r)?,
            svc::DELETE_SUBSCRIPTIONS => self.delete_subscriptions(ctx, hdr, r)?,
            svc::SET_PUBLISHING_MODE => self.set_publishing_mode(ctx, hdr, r)?,
            svc::CREATE_MONITORED_ITEMS => self.create_items(ctx, &space, hdr, r)?,
            svc::MODIFY_MONITORED_ITEMS => self.modify_items(ctx, hdr, r)?,
            svc::DELETE_MONITORED_ITEMS => self.delete_items(ctx, hdr, r)?,
            svc::SET_MONITORING_MODE => self.set_monitoring_mode(ctx, hdr, r)?,
            svc::REPUBLISH => self.republish(ctx, hdr, r)?,
            svc::PUBLISH => {
                self.publish(ctx, hdr, request_id, r, now, out)?;
                return Ok(());
            }
            _ => return Err(sc::BAD_SERVICE_UNSUPPORTED),
        };
        out.push(Reply {
            request_id,
            handle: hdr.handle,
            body,
        });
        Ok(())
    }

    // ---- browse ---------------------------------------------------------------

    fn store_point(&mut self, ctx: &Ctx, max: usize, rest: Vec<Vec<u8>>) -> Result<Vec<u8>, u32> {
        if self.points.len() >= ctx.limits.continuation_points {
            return Err(sc::BAD_NO_CONTINUATION_POINTS);
        }
        self.next_point += 1;
        let id = self.next_point.to_le_bytes().to_vec();
        self.points.push(Point {
            id: id.clone(),
            max,
            rest,
        });
        Ok(id)
    }

    /// A `BrowseResult` of `refs`, at most `max` of them (0: all), the rest
    /// behind a continuation point.
    fn browse_result(&mut self, ctx: &Ctx, w: &mut Writer, mut refs: Vec<Vec<u8>>, max: usize) {
        let mut point = None;
        let mut status = sc::GOOD;
        if max > 0 && refs.len() > max {
            let rest = refs.split_off(max);
            match self.store_point(ctx, max, rest) {
                Ok(id) => point = Some(id),
                Err(s) => {
                    status = s;
                    refs.clear();
                }
            }
        }
        w.u32(status).bytes(point.as_deref()).len(refs.len());
        for r in &refs {
            w.raw(r);
        }
    }

    fn browse(&mut self, ctx: &Ctx, space: &Space, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let view = decode(r.node_id())?;
        decode(r.i64())?;
        decode(r.u32())?;
        if !view.is_null() {
            return Err(sc::BAD_VIEW_ID_UNKNOWN);
        }
        let max = usize::try_from(decode(r.u32())?).unwrap_or(usize::MAX);
        let max = if max == 0 {
            ctx.limits.references
        } else {
            max.min(ctx.limits.references)
        };
        let nodes = decode(r.vec(|r| {
            Ok((
                r.node_id()?,
                r.u32()?,
                r.node_id()?,
                r.bool()?,
                r.u32()?,
                r.u32()?,
            ))
        }))?;
        check_ops(ctx, nodes.len())?;
        let mut w = response(svc::response(svc::BROWSE), hdr.handle, ctx.now_ms());
        w.len(nodes.len());
        for (id, dir, ref_type, subtypes, class_mask, result_mask) in nodes {
            match browse_refs(space, &id, dir, &ref_type, subtypes, class_mask, result_mask) {
                Ok(refs) => self.browse_result(ctx, &mut w, refs, max),
                Err(s) => {
                    w.u32(s).bytes(None).null_array();
                }
            }
        }
        w.null_array();
        Ok(w.buf)
    }

    fn browse_next(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let release = decode(r.bool())?;
        let points = decode(r.vec(Reader::bytes))?;
        check_ops(ctx, points.len())?;
        let mut w = response(svc::response(svc::BROWSE_NEXT), hdr.handle, ctx.now_ms());
        w.len(points.len());
        for p in points {
            let found = p
                .as_deref()
                .and_then(|p| self.points.iter().position(|x| x.id == p))
                .map(|i| self.points.remove(i));
            match found {
                None => {
                    w.u32(sc::BAD_CONTINUATION_POINT_INVALID).bytes(None).null_array();
                }
                Some(_) if release => {
                    w.u32(sc::GOOD).bytes(None).null_array();
                }
                Some(pt) => self.browse_result(ctx, &mut w, pt.rest, pt.max),
            }
        }
        w.null_array();
        Ok(w.buf)
    }

    // ---- subscriptions --------------------------------------------------------

    fn sub(&mut self, id: u32) -> Result<&mut Sub, u32> {
        self.subs
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(sc::BAD_SUBSCRIPTION_ID_INVALID)
    }

    /// Revised (interval, lifetime, keep-alive).
    fn revise(ctx: &Ctx, interval: f64, lifetime: u32, keep_alive: u32) -> (Duration, u32, u32) {
        let interval = millis(interval, ctx.limits.min_interval, Duration::from_secs(60));
        let keep_alive = if keep_alive == 0 { 10 } else { keep_alive.min(1000) };
        let lifetime = lifetime.max(keep_alive.saturating_mul(3)).min(100_000);
        (interval, lifetime, keep_alive)
    }

    fn create_subscription(
        &mut self,
        ctx: &Ctx,
        hdr: &Header,
        r: &mut Reader<'_>,
        now: Instant,
    ) -> Result<Vec<u8>, u32> {
        let interval = decode(r.f64())?;
        let lifetime = decode(r.u32())?;
        let keep_alive = decode(r.u32())?;
        let per_publish = decode(r.u32())?;
        let enabled = decode(r.bool())?;
        decode(r.u8())?;
        if self.subs.len() >= ctx.limits.subscriptions {
            return Err(sc::BAD_TOO_MANY_SUBSCRIPTIONS);
        }
        let (interval, lifetime, keep_alive) = Self::revise(ctx, interval, lifetime, keep_alive);
        let id = ctx.next_id();
        self.subs.push(Sub {
            id,
            interval,
            keep_alive,
            lifetime,
            per_publish,
            enabled,
            items: Vec::new(),
            next_item: 1,
            seq: 1,
            due: now + interval,
            quiet: 0,
            starved: 0,
            late: false,
            retained: VecDeque::new(),
        });
        let mut w = response(svc::response(svc::CREATE_SUBSCRIPTION), hdr.handle, ctx.now_ms());
        w.u32(id).f64(ms_f64(interval)).u32(lifetime).u32(keep_alive);
        Ok(w.buf)
    }

    fn modify_subscription(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let id = decode(r.u32())?;
        let interval = decode(r.f64())?;
        let lifetime = decode(r.u32())?;
        let keep_alive = decode(r.u32())?;
        let per_publish = decode(r.u32())?;
        decode(r.u8())?;
        let (interval, lifetime, keep_alive) = Self::revise(ctx, interval, lifetime, keep_alive);
        let s = self.sub(id)?;
        s.interval = interval;
        s.lifetime = lifetime;
        s.keep_alive = keep_alive;
        s.per_publish = per_publish;
        let mut w = response(svc::response(svc::MODIFY_SUBSCRIPTION), hdr.handle, ctx.now_ms());
        w.f64(ms_f64(interval)).u32(lifetime).u32(keep_alive);
        Ok(w.buf)
    }

    fn delete_subscriptions(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let ids = decode(r.vec(Reader::u32))?;
        check_ops(ctx, ids.len())?;
        let mut w = response(svc::response(svc::DELETE_SUBSCRIPTIONS), hdr.handle, ctx.now_ms());
        w.len(ids.len());
        for id in ids {
            let before = self.subs.len();
            self.subs.retain(|s| s.id != id);
            w.u32(if self.subs.len() < before {
                sc::GOOD
            } else {
                sc::BAD_SUBSCRIPTION_ID_INVALID
            });
        }
        w.null_array();
        Ok(w.buf)
    }

    fn set_publishing_mode(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let enabled = decode(r.bool())?;
        let ids = decode(r.vec(Reader::u32))?;
        check_ops(ctx, ids.len())?;
        let mut w = response(svc::response(svc::SET_PUBLISHING_MODE), hdr.handle, ctx.now_ms());
        w.len(ids.len());
        for id in ids {
            w.u32(match self.sub(id) {
                Ok(s) => {
                    s.enabled = enabled;
                    sc::GOOD
                }
                Err(e) => e,
            });
        }
        w.null_array();
        Ok(w.buf)
    }

    fn create_items(
        &mut self,
        ctx: &Ctx,
        space: &Space,
        hdr: &Header,
        r: &mut Reader<'_>,
    ) -> Result<Vec<u8>, u32> {
        let sub_id = decode(r.u32())?;
        let stamps = Stamps::from_u32(decode(r.u32())?);
        let items = decode(r.vec(|r| {
            let (node, attribute, range, _) = read_value_id(r)?;
            let mode = r.u32()?;
            let params = monitoring_parameters(r)?;
            Ok((node, attribute, range, mode, params))
        }))?;
        let stamps = stamps.ok_or(sc::BAD_TIMESTAMPS_TO_RETURN_INVALID)?;
        check_ops(ctx, items.len())?;
        let limit = ctx.limits.items;
        let sampling = ms_f64(ctx.limits.min_interval);
        let s = self.sub(sub_id)?;
        let mut w = response(
            svc::response(svc::CREATE_MONITORED_ITEMS),
            hdr.handle,
            ctx.now_ms(),
        );
        w.len(items.len());
        for (node, attribute, range, mode, params) in items {
            let status = match (space.node(&node), Mode::from_u32(mode)) {
                _ if s.items.len() >= limit => sc::BAD_TOO_MANY_MONITORED_ITEMS,
                (None, _) => sc::BAD_NODE_ID_UNKNOWN,
                (Some(n), _) if attribute != attr::VALUE || n.class != Class::Variable => {
                    sc::BAD_ATTRIBUTE_ID_INVALID
                }
                _ if range.is_some_and(|r| !r.is_empty()) => sc::BAD_INDEX_RANGE_NO_DATA,
                (_, None) => sc::BAD_MONITORING_MODE_INVALID,
                _ if params.filter_status != sc::GOOD => params.filter_status,
                (Some(_), Some(mode)) => {
                    let id = s.next_item;
                    s.next_item = s.next_item.wrapping_add(1).max(1);
                    s.items.push(Item {
                        id,
                        node,
                        handle: params.handle,
                        mode,
                        stamps,
                        last: None,
                        queued: None,
                    });
                    w.u32(sc::GOOD).u32(id).f64(params.sampling.max(sampling)).u32(1);
                    w.extension_object(&NodeId::ns0(0), None);
                    continue;
                }
            };
            w.u32(status)
                .u32(0)
                .f64(0.0)
                .u32(0)
                .extension_object(&NodeId::ns0(0), None);
        }
        w.null_array();
        Ok(w.buf)
    }

    fn modify_items(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let sub_id = decode(r.u32())?;
        let stamps = Stamps::from_u32(decode(r.u32())?);
        let items = decode(r.vec(|r| Ok((r.u32()?, monitoring_parameters(r)?))))?;
        let stamps = stamps.ok_or(sc::BAD_TIMESTAMPS_TO_RETURN_INVALID)?;
        check_ops(ctx, items.len())?;
        let sampling = ms_f64(ctx.limits.min_interval);
        let s = self.sub(sub_id)?;
        let mut w = response(
            svc::response(svc::MODIFY_MONITORED_ITEMS),
            hdr.handle,
            ctx.now_ms(),
        );
        w.len(items.len());
        for (id, params) in items {
            match s.items.iter_mut().find(|i| i.id == id) {
                Some(_) if params.filter_status != sc::GOOD => {
                    w.u32(params.filter_status).f64(0.0).u32(0);
                }
                Some(item) => {
                    item.handle = params.handle;
                    item.stamps = stamps;
                    w.u32(sc::GOOD).f64(params.sampling.max(sampling)).u32(1);
                }
                None => {
                    w.u32(sc::BAD_MONITORED_ITEM_ID_INVALID).f64(0.0).u32(0);
                }
            }
            w.extension_object(&NodeId::ns0(0), None);
        }
        w.null_array();
        Ok(w.buf)
    }

    fn delete_items(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let sub_id = decode(r.u32())?;
        let ids = decode(r.vec(Reader::u32))?;
        check_ops(ctx, ids.len())?;
        let s = self.sub(sub_id)?;
        let mut w = response(
            svc::response(svc::DELETE_MONITORED_ITEMS),
            hdr.handle,
            ctx.now_ms(),
        );
        w.len(ids.len());
        for id in ids {
            let before = s.items.len();
            s.items.retain(|i| i.id != id);
            w.u32(if s.items.len() < before {
                sc::GOOD
            } else {
                sc::BAD_MONITORED_ITEM_ID_INVALID
            });
        }
        w.null_array();
        Ok(w.buf)
    }

    fn set_monitoring_mode(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let sub_id = decode(r.u32())?;
        let mode = decode(r.u32())?;
        let ids = decode(r.vec(Reader::u32))?;
        check_ops(ctx, ids.len())?;
        let mode = Mode::from_u32(mode).ok_or(sc::BAD_MONITORING_MODE_INVALID)?;
        let s = self.sub(sub_id)?;
        let mut w = response(svc::response(svc::SET_MONITORING_MODE), hdr.handle, ctx.now_ms());
        w.len(ids.len());
        for id in ids {
            w.u32(match s.items.iter_mut().find(|i| i.id == id) {
                Some(item) => {
                    item.mode = mode;
                    if mode != Mode::Reporting {
                        item.queued = None;
                    }
                    sc::GOOD
                }
                None => sc::BAD_MONITORED_ITEM_ID_INVALID,
            });
        }
        w.null_array();
        Ok(w.buf)
    }

    fn republish(&mut self, ctx: &Ctx, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
        let sub_id = decode(r.u32())?;
        let seq = decode(r.u32())?;
        let s = self.sub(sub_id)?;
        let msg = s
            .retained
            .iter()
            .find(|(n, _)| *n == seq)
            .map(|(_, m)| m.clone())
            .ok_or(sc::BAD_MESSAGE_NOT_AVAILABLE)?;
        let mut w = response(svc::response(svc::REPUBLISH), hdr.handle, ctx.now_ms());
        w.raw(&msg);
        Ok(w.buf)
    }

    fn publish(
        &mut self,
        ctx: &Ctx,
        hdr: &Header,
        request_id: u32,
        r: &mut Reader<'_>,
        now: Instant,
        out: &mut Vec<Reply>,
    ) -> Result<(), u32> {
        let acks = decode(r.vec(|r| Ok((r.u32()?, r.u32()?))))?;
        check_ops(ctx, acks.len())?;
        if self.subs.is_empty() {
            return Err(sc::BAD_NO_SUBSCRIPTION);
        }
        let acks = acks
            .into_iter()
            .map(|(sub, seq)| match self.sub(sub) {
                Err(e) => e,
                Ok(s) => match s.retained.iter().position(|(n, _)| *n == seq) {
                    Some(i) => {
                        s.retained.remove(i);
                        sc::GOOD
                    }
                    None => sc::BAD_SEQUENCE_NUMBER_UNKNOWN,
                },
            })
            .collect();
        self.publish.push_back(Pending {
            request_id,
            handle: hdr.handle,
            acks,
            deadline: (hdr.timeout_hint > 0)
                .then(|| now + Duration::from_millis(u64::from(hdr.timeout_hint))),
        });
        while self.publish.len() > ctx.limits.publish_queue {
            if let Some(p) = self.publish.pop_front() {
                out.push(Reply {
                    request_id: p.request_id,
                    handle: p.handle,
                    body: fault(p.handle, sc::BAD_TOO_MANY_PUBLISH_REQUESTS, ctx.now_ms()),
                });
            }
        }
        // A subscription with something waiting answers at once.
        for i in 0..self.subs.len() {
            if self.subs[i].late && !self.publish.is_empty() {
                self.send(ctx, i, out);
            }
        }
        Ok(())
    }

    /// Answers the oldest publish request with subscription `i`'s queued
    /// changes, or a keep-alive if it has none.
    fn send(&mut self, ctx: &Ctx, i: usize, out: &mut Vec<Reply>) {
        let Some(p) = self.publish.pop_front() else { return };
        let now_ms = ctx.now_ms();
        let s = &mut self.subs[i];
        s.late = false;
        s.quiet = 0;
        s.starved = 0;
        let max = if s.per_publish == 0 {
            usize::MAX
        } else {
            usize::try_from(s.per_publish).unwrap_or(usize::MAX)
        };
        let mut changes = Vec::new();
        let mut more = false;
        if s.enabled {
            for item in &mut s.items {
                if item.queued.is_none() {
                    continue;
                }
                if changes.len() == max {
                    more = true;
                    break;
                }
                if let Some(dv) = item.queued.take() {
                    changes.push((item.handle, dv));
                }
            }
        }
        let mut msg = Writer::new();
        if changes.is_empty() {
            // Keep-alive: the next sequence number, not consumed.
            msg.u32(s.seq).i64(date_time(now_ms)).len(0);
        } else {
            let seq = s.next_seq();
            let mut body = Writer::new();
            body.len(changes.len());
            for (handle, dv) in &changes {
                body.u32(*handle).data_value(dv);
            }
            body.null_array();
            msg.u32(seq).i64(date_time(now_ms)).len(1);
            msg.extension_object(&NodeId::ns0(svc::DATA_CHANGE_NOTIFICATION), Some(&body.buf));
            s.retained.push_back((seq, msg.buf.clone()));
            while s.retained.len() > RETAINED {
                s.retained.pop_front();
            }
        }
        let mut w = response(svc::response(svc::PUBLISH), p.handle, now_ms);
        w.u32(s.id).len(s.retained.len());
        for (n, _) in &s.retained {
            w.u32(*n);
        }
        w.bool(more).raw(&msg.buf).len(p.acks.len());
        for a in &p.acks {
            w.u32(*a);
        }
        w.null_array();
        if more {
            s.late = true;
        }
        out.push(Reply {
            request_id: p.request_id,
            handle: p.handle,
            body: w.buf,
        });
    }

    /// Time passing: expired publish requests, and each subscription's
    /// publishing cycle (sample, then notify or keep alive).
    pub fn tick(&mut self, ctx: &Ctx, now: Instant, out: &mut Vec<Reply>) {
        let now_ms = ctx.now_ms();
        let mut kept = VecDeque::with_capacity(self.publish.len());
        for p in self.publish.drain(..) {
            if p.deadline.is_some_and(|d| now >= d) {
                out.push(Reply {
                    request_id: p.request_id,
                    handle: p.handle,
                    body: fault(p.handle, sc::BAD_TIMEOUT, now_ms),
                });
            } else {
                kept.push_back(p);
            }
        }
        self.publish = kept;
        if self.subs.is_empty() {
            return;
        }
        let (snap, space) = ctx.view();
        let rc = space::ReadCtx {
            snap: &snap,
            about: &ctx.about,
            now_ms,
            sampling_ms: ms_f64(ctx.limits.min_interval),
        };
        let mut i = 0;
        while i < self.subs.len() {
            let s = &mut self.subs[i];
            if now < s.due {
                i += 1;
                continue;
            }
            s.due += s.interval;
            if s.due <= now {
                s.due = now + s.interval;
            }
            for item in &mut s.items {
                if item.mode != Mode::Disabled {
                    sample(item, &space, &rc);
                }
            }
            let waiting = s.enabled && s.items.iter().any(|it| it.queued.is_some());
            s.quiet += 1;
            if self.publish.is_empty() {
                s.starved += 1;
                if s.starved >= s.lifetime {
                    // Nobody is collecting it: it expires.
                    self.subs.remove(i);
                    continue;
                }
                if waiting || s.quiet >= s.keep_alive {
                    s.late = true;
                }
            } else if waiting || s.quiet >= s.keep_alive || s.late {
                self.send(ctx, i, out);
            }
            i += 1;
        }
    }
}

/// Samples `item` against the snapshot: a new value or status is queued
/// (a queue of one: the newest wins) when it is reporting.
fn sample(item: &mut Item, space: &Space, rc: &space::ReadCtx<'_>) {
    let dv = match space.node(&item.node).and_then(|n| n.source.as_ref()) {
        Some(src) => space::value(src, rc, item.stamps),
        None => DataValue {
            status: sc::BAD_NODE_ID_UNKNOWN,
            ..DataValue::default()
        },
    };
    let key = (dv.value.clone(), dv.status);
    let same = match (&item.last, &key) {
        (Some((Some(super::codec::Variant::F64(a)), sa)), (Some(super::codec::Variant::F64(b)), sb)) => {
            a.to_bits() == b.to_bits() && sa == sb
        }
        (Some(l), k) => l == k,
        (None, _) => false,
    };
    if same {
        return;
    }
    item.last = Some(key);
    if item.mode == Mode::Reporting {
        item.queued = Some(dv);
    }
}

/// Monitoring parameters, with what the filter means for the item.
struct Params {
    handle: u32,
    sampling: f64,
    filter_status: u32,
}

fn monitoring_parameters(r: &mut Reader<'_>) -> super::codec::Result<Params> {
    let handle = r.u32()?;
    let sampling = r.f64()?;
    let (ty, body) = r.extension_object()?;
    r.u32()?;
    r.bool()?;
    let filter_status = match (ty.ns0_id(), body) {
        (Some(0), None) => sc::GOOD,
        // A data change filter without a deadband is what the server does
        // anyway (it reports value and status changes).
        (Some(svc::DATA_CHANGE_FILTER), Some(b)) => {
            let mut f = Reader::new(&b);
            let trigger = f.u32()?;
            let deadband = f.u32()?;
            f.f64()?;
            if deadband == 0 && trigger <= 2 {
                sc::GOOD
            } else {
                sc::BAD_MONITORED_ITEM_FILTER_UNSUPPORTED
            }
        }
        _ => sc::BAD_MONITORED_ITEM_FILTER_UNSUPPORTED,
    };
    let sampling = if sampling.is_finite() { sampling } else { 0.0 };
    Ok(Params {
        handle,
        sampling,
        filter_status,
    })
}

fn check_ops(ctx: &Ctx, n: usize) -> Result<(), u32> {
    if n == 0 {
        Err(sc::BAD_NOTHING_TO_DO)
    } else if n > ctx.limits.operations {
        Err(sc::BAD_TOO_MANY_OPERATIONS)
    } else {
        Ok(())
    }
}

/// A `ReadValueId`: node, attribute, index range, data encoding.
type ReadValueId = (NodeId, u32, Option<String>, (u16, Option<String>));

fn read_value_id(r: &mut Reader<'_>) -> super::codec::Result<ReadValueId> {
    Ok((r.node_id()?, r.u32()?, r.string()?, r.qualified_name()?))
}

fn read(
    ctx: &Ctx,
    space: &Space,
    rc: &space::ReadCtx<'_>,
    hdr: &Header,
    r: &mut Reader<'_>,
) -> Result<Vec<u8>, u32> {
    let max_age = decode(r.f64())?;
    let stamps = decode(r.u32())?;
    let nodes = decode(r.vec(read_value_id))?;
    if max_age.is_nan() || max_age < 0.0 {
        return Err(sc::BAD_MAX_AGE_INVALID);
    }
    let stamps = Stamps::from_u32(stamps).ok_or(sc::BAD_TIMESTAMPS_TO_RETURN_INVALID)?;
    check_ops(ctx, nodes.len())?;
    let mut w = response(svc::response(svc::READ), hdr.handle, ctx.now_ms());
    w.len(nodes.len());
    for (id, attribute, range, (enc_ns, enc)) in nodes {
        let bad = |status| DataValue {
            status,
            ..DataValue::default()
        };
        let dv = match space.node(&id) {
            None => bad(sc::BAD_NODE_ID_UNKNOWN),
            Some(_) if range.is_some_and(|r| !r.is_empty()) => bad(sc::BAD_INDEX_RANGE_NO_DATA),
            Some(_)
                if enc
                    .as_deref()
                    .is_some_and(|e| !(enc_ns == 0 && e == "Default Binary")) =>
            {
                bad(sc::BAD_DATA_ENCODING_INVALID)
            }
            Some(n) => space::read(n, attribute, rc, stamps),
        };
        w.data_value(&dv);
    }
    w.null_array();
    Ok(w.buf)
}

/// Write: refused, always. Known nodes are not writable; unknown ones are
/// unknown. Nothing is decoded past what is needed to say which.
fn write(ctx: &Ctx, space: &Space, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
    let nodes = decode(r.vec(|r| {
        let id = r.node_id()?;
        r.u32()?;
        r.string()?;
        r.skip_data_value(0)?;
        Ok(id)
    }))?;
    check_ops(ctx, nodes.len())?;
    let mut w = response(svc::response(svc::WRITE), hdr.handle, ctx.now_ms());
    w.len(nodes.len());
    for id in nodes {
        w.u32(if space.node(&id).is_some() {
            sc::BAD_NOT_WRITABLE
        } else {
            sc::BAD_NODE_ID_UNKNOWN
        });
    }
    w.null_array();
    Ok(w.buf)
}

/// The references of `id` a browse asks for, each encoded as a
/// `ReferenceDescription`.
fn browse_refs(
    space: &Space,
    id: &NodeId,
    dir: u32,
    ref_type: &NodeId,
    subtypes: bool,
    class_mask: u32,
    result_mask: u32,
) -> Result<Vec<Vec<u8>>, u32> {
    let node = space.node(id).ok_or(sc::BAD_NODE_ID_UNKNOWN)?;
    let (fwd, inv) = match dir {
        0 => (true, false),
        1 => (false, true),
        2 => (true, true),
        _ => return Err(sc::BAD_BROWSE_DIRECTION_INVALID),
    };
    let filter = if ref_type.is_null() {
        None
    } else {
        match ref_type.ns0_id() {
            Some(t) if space::known_ref_type(t) => Some(t),
            _ => return Err(sc::BAD_REFERENCE_TYPE_ID_INVALID),
        }
    };
    let mut out = Vec::new();
    for r in &node.refs {
        if (r.forward && !fwd) || (!r.forward && !inv) {
            continue;
        }
        if filter.is_some_and(|f| !space::ref_matches(r.ty, f, subtypes)) {
            continue;
        }
        let Some(t) = space.target(&r.target) else {
            continue;
        };
        if class_mask != 0 && class_mask & (t.class as u32) == 0 {
            continue;
        }
        let mut w = Writer::new();
        w.node_id(&NodeId::ns0(if result_mask & 0x01 != 0 { r.ty } else { 0 }))
            .bool(r.forward)
            .expanded_node_id(&r.target);
        if result_mask & 0x08 != 0 {
            w.qualified_name(t.browse.0, &t.browse.1);
        } else {
            w.u16(0).string(None);
        }
        w.localized_text((result_mask & 0x10 != 0).then_some(t.display.as_str()));
        w.u32(if result_mask & 0x04 != 0 {
            t.class as u32
        } else {
            0
        });
        let td = t
            .type_definition
            .filter(|_| result_mask & 0x20 != 0 && matches!(t.class, Class::Object | Class::Variable))
            .unwrap_or(NodeId::ns0(0));
        w.expanded_node_id(&td);
        out.push(w.buf);
    }
    Ok(out)
}

fn translate(ctx: &Ctx, space: &Space, hdr: &Header, r: &mut Reader<'_>) -> Result<Vec<u8>, u32> {
    let paths = decode(r.vec(|r| {
        let start = r.node_id()?;
        let elems = r.vec(|r| Ok((r.node_id()?, r.bool()?, r.bool()?, r.qualified_name()?)))?;
        Ok((start, elems))
    }))?;
    check_ops(ctx, paths.len())?;
    let mut w = response(svc::response(svc::TRANSLATE), hdr.handle, ctx.now_ms());
    w.len(paths.len());
    for (start, elems) in paths {
        let (status, targets) = if space.node(&start).is_none() {
            (sc::BAD_NODE_ID_UNKNOWN, Vec::new())
        } else if elems.is_empty() {
            (sc::BAD_NOTHING_TO_DO, Vec::new())
        } else {
            let mut at = vec![start];
            for (ref_type, inverse, subtypes, (ns, name)) in &elems {
                let mut next = Vec::new();
                for id in &at {
                    let Some(n) = space.node(id) else { continue };
                    for r in &n.refs {
                        if r.forward == *inverse {
                            continue;
                        }
                        if let Some(f) = ref_type.ns0_id().filter(|f| *f != 0)
                            && !space::ref_matches(r.ty, f, *subtypes)
                        {
                            continue;
                        }
                        let hit = space
                            .target(&r.target)
                            .is_some_and(|t| t.browse.0 == *ns && Some(&t.browse.1) == name.as_ref());
                        if hit && !next.contains(&r.target) {
                            next.push(r.target.clone());
                        }
                    }
                }
                at = next;
            }
            if at.is_empty() {
                (sc::BAD_NO_MATCH, at)
            } else {
                (sc::GOOD, at)
            }
        };
        w.u32(status).len(targets.len());
        for t in &targets {
            w.expanded_node_id(t).u32(u32::MAX);
        }
    }
    w.null_array();
    Ok(w.buf)
}
