//! One client connection, without the socket: OPC UA TCP (Hello and
//! Acknowledge, chunking) and a secure channel with `SecurityPolicy#None`
//! (Part 6 §6.7, §7.1), carrying one session.
//!
//! [`Conn::feed`] takes whatever bytes arrived, [`Conn::tick`] lets time
//! pass, and [`Conn::take_output`] hands back the bytes to send; the
//! server's thread does the socket. So the whole protocol is testable, and
//! fuzzable, as a function of bytes in to bytes out.
//!
//! Anything the server cannot take ends the connection with an `ERR`
//! message: a message larger than the negotiated buffer, a message type it
//! does not expect, a security policy other than None, a security mode
//! other than None, an unknown channel or token, a sequence number out of
//! order. A request it cannot decode, or a service it does not offer, gets
//! a `ServiceFault` and the connection stays.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::Ctx;
use super::codec::{DecodeError, NodeId, Reader, Writer, date_time};
use super::session::{self, Header, Reply, SECURITY_NONE, Session, svc};
use super::status as sc;

/// The smallest buffer the standard lets either side offer.
const MIN_BUFFER: u32 = 8192;
/// Longest endpoint URL a Hello may carry.
const MAX_URL: usize = 4096;
/// Bytes of a symmetric `MSG` chunk before its body: message header (8),
/// channel id (4), token id (4), sequence header (8).
const MSG_OVERHEAD: usize = 24;

#[derive(Debug, Clone, Copy)]
struct Negotiated {
    /// Largest chunk the client takes (what this side sends).
    send: usize,
    /// Largest message the client takes; 0: no limit.
    max_message: usize,
    /// Most chunks the client takes in one message; 0: no limit.
    max_chunks: usize,
}

#[derive(Debug)]
struct Channel {
    id: u32,
    token: u32,
    previous: Option<u32>,
    lifetime: Duration,
    renewed: Instant,
    recv_seq: Option<u32>,
    send_seq: u32,
}

#[derive(Debug)]
struct Partial {
    request_id: u32,
    body: Vec<u8>,
    chunks: usize,
}

/// What an `OpenSecureChannel` asks for.
struct Open {
    channel_id: u32,
    policy: Option<String>,
    seq: u32,
    request_id: u32,
    ty: NodeId,
    hdr: Header,
    request_type: u32,
    mode: u32,
    lifetime: u32,
}

impl Open {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let channel_id = r.u32()?;
        let policy = r.string()?;
        r.bytes()?;
        r.bytes()?;
        let seq = r.u32()?;
        let request_id = r.u32()?;
        let ty = r.node_id()?;
        let hdr = Header::decode(r)?;
        r.u32()?;
        let request_type = r.u32()?;
        let mode = r.u32()?;
        r.bytes()?;
        let lifetime = r.u32()?;
        Ok(Self {
            channel_id,
            policy,
            seq,
            request_id,
            ty,
            hdr,
            request_type,
            mode,
            lifetime,
        })
    }
}

/// One connection's protocol state.
pub struct Conn {
    ctx: Arc<Ctx>,
    inbuf: Vec<u8>,
    out: Vec<u8>,
    closed: bool,
    opened: Instant,
    hello: Option<Negotiated>,
    channel: Option<Channel>,
    session: Option<Session>,
    partial: Option<Partial>,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("closed", &self.closed)
            .field("channel", &self.channel)
            .finish_non_exhaustive()
    }
}

fn to_usize(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

impl Conn {
    #[must_use]
    pub fn new(ctx: Arc<Ctx>, now: Instant) -> Self {
        Self {
            ctx,
            inbuf: Vec::new(),
            out: Vec::new(),
            closed: false,
            opened: now,
            hello: None,
            channel: None,
            session: None,
            partial: None,
        }
    }

    /// Whether the connection is over (after any output is sent).
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whether a session is activated on it.
    #[must_use]
    pub fn active(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.active)
    }

    /// Bytes to send, emptied.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Bytes from the client, in any split.
    pub fn feed(&mut self, data: &[u8], now: Instant) {
        if self.closed {
            return;
        }
        self.inbuf.extend_from_slice(data);
        while !self.closed && self.inbuf.len() >= 8 {
            let size = u32::from_le_bytes([self.inbuf[4], self.inbuf[5], self.inbuf[6], self.inbuf[7]]);
            if size < 8 || size > self.ctx.limits.buffer {
                self.error(sc::BAD_TCP_MESSAGE_TOO_LARGE, "message size out of range");
                return;
            }
            let size = to_usize(size);
            if self.inbuf.len() < size {
                return;
            }
            let msg: Vec<u8> = self.inbuf.drain(..size).collect();
            self.message(&msg, now);
        }
    }

    /// Time passing: deadlines, and the session's subscriptions.
    pub fn tick(&mut self, now: Instant) {
        if self.closed {
            return;
        }
        let limits = &self.ctx.limits;
        let since = now.saturating_duration_since(self.opened);
        if (self.hello.is_none() && since > limits.hello_timeout)
            || (self.channel.is_none() && since > limits.hello_timeout * 2)
        {
            self.closed = true;
            return;
        }
        if let Some(c) = &self.channel
            && now.saturating_duration_since(c.renewed) > c.lifetime + c.lifetime / 4
        {
            self.error(sc::BAD_SECURE_CHANNEL_TOKEN_UNKNOWN, "secure channel expired");
            return;
        }
        let mut replies = Vec::new();
        if let Some(s) = &mut self.session {
            if now.saturating_duration_since(s.last) > s.timeout {
                s.close(&self.ctx, sc::BAD_SESSION_CLOSED, &mut replies);
                self.session = None;
            } else if s.active {
                s.tick(&self.ctx, now, &mut replies);
            }
        }
        for r in replies {
            self.reply(r);
        }
    }

    fn error(&mut self, code: u32, reason: &str) {
        let mut body = Writer::new();
        body.u32(code).string(Some(reason));
        self.frame(*b"ERRF", &body.buf);
        self.closed = true;
    }

    fn frame(&mut self, head: [u8; 4], body: &[u8]) {
        let size = u32::try_from(body.len() + 8).unwrap_or(u32::MAX);
        self.out.extend_from_slice(&head);
        self.out.extend_from_slice(&size.to_le_bytes());
        self.out.extend_from_slice(body);
    }

    fn message(&mut self, msg: &[u8], now: Instant) {
        let kind = &msg[..3];
        let chunk = msg[3];
        let body = &msg[8..];
        match (kind, self.hello.is_some()) {
            (b"HEL", false) if chunk == b'F' => self.hello(body),
            (b"OPN", true) if chunk == b'F' => self.open(body, now),
            (b"MSG", true) => self.msg(chunk, body, now),
            (b"CLO", true) => {
                let mut replies = Vec::new();
                if let Some(mut s) = self.session.take() {
                    s.close(&self.ctx, sc::BAD_SESSION_CLOSED, &mut replies);
                }
                self.closed = true;
            }
            _ => self.error(sc::BAD_TCP_MESSAGE_TYPE_INVALID, "unexpected message type"),
        }
    }

    fn hello(&mut self, body: &[u8]) {
        let mut r = Reader::new(body);
        let parsed =
            (|| Ok::<_, DecodeError>((r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.string()?)))();
        let Ok((_version, client_recv, client_send, max_message, max_chunks, url)) = parsed else {
            self.error(sc::BAD_DECODING_ERROR, "malformed Hello");
            return;
        };
        if url.as_ref().is_some_and(|u| u.len() > MAX_URL) {
            self.error(sc::BAD_TCP_ENDPOINT_URL_INVALID, "endpoint URL too long");
            return;
        }
        if client_recv < MIN_BUFFER || client_send < MIN_BUFFER {
            self.error(sc::BAD_TCP_NOT_ENOUGH_RESOURCES, "buffers below 8192 bytes");
            return;
        }
        let limits = &self.ctx.limits;
        let recv = client_send.min(limits.buffer);
        let send = client_recv.min(limits.buffer);
        self.hello = Some(Negotiated {
            send: to_usize(send),
            max_message: to_usize(max_message),
            max_chunks: to_usize(max_chunks),
        });
        let mut ack = Writer::new();
        ack.u32(0)
            .u32(recv)
            .u32(send)
            .u32(limits.max_message)
            .u32(limits.max_chunks);
        self.frame(*b"ACKF", &ack.buf);
    }

    /// Checks and records the client's sequence number: one more than the
    /// last, or a wrap back below 1024 from near the top.
    fn sequence(&mut self, seq: u32) -> bool {
        let Some(c) = &mut self.channel else { return true };
        let ok = c
            .recv_seq
            .is_none_or(|p| seq == p.wrapping_add(1) || (p > u32::MAX - 1024 && seq < 1024));
        c.recv_seq = Some(seq);
        ok
    }

    fn open(&mut self, body: &[u8], now: Instant) {
        let Ok(o) = Open::decode(&mut Reader::new(body)) else {
            self.error(sc::BAD_DECODING_ERROR, "malformed OpenSecureChannel");
            return;
        };
        if o.policy.as_deref() != Some(SECURITY_NONE) {
            self.error(
                sc::BAD_SECURITY_POLICY_REJECTED,
                "only SecurityPolicy#None is offered",
            );
            return;
        }
        if o.ty != NodeId::ns0(svc::OPEN_SECURE_CHANNEL) {
            self.error(sc::BAD_TCP_MESSAGE_TYPE_INVALID, "OPN without OpenSecureChannel");
            return;
        }
        if o.mode != 1 {
            self.error(
                sc::BAD_SECURITY_MODE_REJECTED,
                "only MessageSecurityMode None is offered",
            );
            return;
        }
        let lifetime = Duration::from_millis(u64::from(o.lifetime))
            .clamp(Duration::from_secs(10), Duration::from_secs(3600));
        match (o.request_type, &mut self.channel) {
            (0, None) => {
                self.channel = Some(Channel {
                    id: self.ctx.next_id(),
                    token: 1,
                    previous: None,
                    lifetime,
                    renewed: now,
                    recv_seq: Some(o.seq),
                    send_seq: 1,
                });
            }
            (1, Some(c)) if c.id == o.channel_id => {
                c.previous = Some(c.token);
                c.token = c.token.wrapping_add(1).max(1);
                c.lifetime = lifetime;
                c.renewed = now;
                if !self.sequence(o.seq) {
                    self.error(sc::BAD_SEQUENCE_NUMBER_INVALID, "sequence number out of order");
                    return;
                }
            }
            _ => {
                self.error(
                    sc::BAD_REQUEST_TYPE_INVALID,
                    "Issue on an open channel, or Renew of another",
                );
                return;
            }
        }
        let now_ms = self.ctx.now_ms();
        let Some(c) = &mut self.channel else { return };
        let mut w = session::response(svc::OPEN_SECURE_CHANNEL_RESPONSE, o.hdr.handle, now_ms);
        w.u32(0)
            .u32(c.id)
            .u32(c.token)
            .i64(date_time(now_ms))
            .u32(u32::try_from(c.lifetime.as_millis()).unwrap_or(u32::MAX))
            .bytes(Some(&[]));
        let mut msg = Writer::new();
        msg.u32(c.id)
            .string(Some(SECURITY_NONE))
            .bytes(None)
            .bytes(None)
            .u32(c.send_seq)
            .u32(o.request_id)
            .raw(&w.buf);
        c.send_seq = c.send_seq.wrapping_add(1).max(1);
        self.frame(*b"OPNF", &msg.buf);
    }

    fn msg(&mut self, chunk: u8, body: &[u8], now: Instant) {
        let mut r = Reader::new(body);
        let Ok((channel_id, token, seq, request_id)) =
            (|| Ok::<_, DecodeError>((r.u32()?, r.u32()?, r.u32()?, r.u32()?)))()
        else {
            self.error(sc::BAD_DECODING_ERROR, "malformed message header");
            return;
        };
        let Some(c) = &self.channel else {
            self.error(sc::BAD_TCP_SECURE_CHANNEL_UNKNOWN, "no secure channel");
            return;
        };
        if c.id != channel_id {
            self.error(sc::BAD_TCP_SECURE_CHANNEL_UNKNOWN, "unknown secure channel");
            return;
        }
        if token != c.token && Some(token) != c.previous {
            self.error(sc::BAD_SECURE_CHANNEL_TOKEN_UNKNOWN, "unknown security token");
            return;
        }
        if !self.sequence(seq) {
            self.error(sc::BAD_SEQUENCE_NUMBER_INVALID, "sequence number out of order");
            return;
        }
        let rest = &body[16..];
        let max_message = to_usize(self.ctx.limits.max_message);
        let max_chunks = to_usize(self.ctx.limits.max_chunks);
        match chunk {
            b'A' => self.partial = None,
            b'C' | b'F' => {
                let mut p = match self.partial.take() {
                    Some(p) if p.request_id == request_id => p,
                    _ => Partial {
                        request_id,
                        body: Vec::new(),
                        chunks: 0,
                    },
                };
                p.chunks += 1;
                p.body.extend_from_slice(rest);
                if p.body.len() > max_message || p.chunks > max_chunks {
                    self.error(sc::BAD_TCP_MESSAGE_TOO_LARGE, "request too large");
                    return;
                }
                if chunk == b'C' {
                    self.partial = Some(p);
                } else {
                    self.request(request_id, &p.body, now);
                }
            }
            _ => self.error(sc::BAD_TCP_MESSAGE_TYPE_INVALID, "unknown chunk type"),
        }
    }

    fn request(&mut self, request_id: u32, body: &[u8], now: Instant) {
        let now_ms = self.ctx.now_ms();
        let mut r = Reader::new(body);
        let decoded = r.node_id().and_then(|ty| Ok((ty, Header::decode(&mut r)?)));
        let Ok((ty, hdr)) = decoded else {
            self.reply(Reply {
                request_id,
                handle: 0,
                body: session::fault(0, sc::BAD_DECODING_ERROR, now_ms),
            });
            return;
        };
        let ty = ty.ns0_id().unwrap_or(0);
        if ty == svc::CLOSE_SECURE_CHANNEL {
            self.closed = true;
            return;
        }
        let mut replies = Vec::new();
        let result = self.service(ty, &hdr, request_id, &mut r, now, &mut replies);
        for rep in replies {
            self.reply(rep);
        }
        if let Err(status) = result {
            self.reply(Reply {
                request_id,
                handle: hdr.handle,
                body: session::fault(hdr.handle, status, now_ms),
            });
        }
    }

    fn service(
        &mut self,
        ty: u32,
        hdr: &Header,
        request_id: u32,
        r: &mut Reader<'_>,
        now: Instant,
        out: &mut Vec<Reply>,
    ) -> Result<(), u32> {
        let ctx = Arc::clone(&self.ctx);
        let reply = |body| Reply {
            request_id,
            handle: hdr.handle,
            body,
        };
        match ty {
            svc::GET_ENDPOINTS | svc::FIND_SERVERS => {
                out.push(reply(session::discovery(&ctx, ty, hdr, r)?));
                return Ok(());
            }
            svc::CREATE_SESSION => {
                if self.session.is_some() {
                    return Err(sc::BAD_TOO_MANY_SESSIONS);
                }
                let (s, body) = Session::create(&ctx, hdr, r, now)?;
                self.session = Some(s);
                out.push(reply(body));
                return Ok(());
            }
            _ => {}
        }
        let s = match &mut self.session {
            Some(s) if s.token == hdr.auth => s,
            _ => return Err(sc::BAD_SESSION_ID_INVALID),
        };
        s.last = now;
        match ty {
            svc::ACTIVATE_SESSION => {
                out.push(reply(s.activate(&ctx, hdr, r)?));
                Ok(())
            }
            svc::CLOSE_SESSION => {
                r.bool().map_err(|_| sc::BAD_DECODING_ERROR)?;
                s.close(&ctx, sc::BAD_SESSION_CLOSED, out);
                self.session = None;
                out.push(reply(
                    session::response(svc::response(svc::CLOSE_SESSION), hdr.handle, ctx.now_ms()).buf,
                ));
                Ok(())
            }
            _ if !s.active => Err(sc::BAD_SESSION_NOT_ACTIVATED),
            _ => s.service(&ctx, ty, hdr, request_id, r, now, out),
        }
    }

    /// Sends a response in as many chunks as the client's buffer needs;
    /// one the client could not take becomes `BadResponseTooLarge`.
    fn reply(&mut self, rep: Reply) {
        let (Some(neg), Some(_)) = (self.hello, &self.channel) else {
            return;
        };
        let room = neg.send.saturating_sub(MSG_OVERHEAD).max(1);
        let chunks = rep.body.len().div_ceil(room).max(1);
        let too_big = (neg.max_message != 0 && rep.body.len() > neg.max_message)
            || (neg.max_chunks != 0 && chunks > neg.max_chunks);
        let body = if too_big {
            session::fault(rep.handle, sc::BAD_RESPONSE_TOO_LARGE, self.ctx.now_ms())
        } else {
            rep.body
        };
        let Some(c) = &mut self.channel else { return };
        let parts: Vec<&[u8]> = if body.is_empty() {
            vec![&[][..]]
        } else {
            body.chunks(room).collect()
        };
        let last = parts.len() - 1;
        for (i, part) in parts.iter().enumerate() {
            let mut w = Writer::new();
            w.u32(c.id)
                .u32(c.token)
                .u32(c.send_seq)
                .u32(rep.request_id)
                .raw(part);
            c.send_seq = c.send_seq.wrapping_add(1).max(1);
            let head = if i == last { b"MSGF" } else { b"MSGC" };
            let size = u32::try_from(w.buf.len() + 8).unwrap_or(u32::MAX);
            self.out.extend_from_slice(head);
            self.out.extend_from_slice(&size.to_le_bytes());
            self.out.extend_from_slice(&w.buf);
        }
    }
}
