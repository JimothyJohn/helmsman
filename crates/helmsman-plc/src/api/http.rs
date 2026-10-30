//! Just enough HTTP/1.1 for the box API, on `std::net`: one request per
//! connection, `Content-Length` bodies only, every size and every wait
//! bounded. Anything outside that subset is refused rather than guessed
//! at: this parser faces the LAN.
//!
//! - The request line is `METHOD SP /target SP HTTP/1.x`, the method upper
//!   case letters, the target visible ASCII.
//! - Header lines are `name: value` with a token name and visible-ASCII
//!   value; folded lines, bare CR or LF, duplicate `Content-Length` or
//!   `Authorization`, and any `Transfer-Encoding` are refused.
//! - The whole request (head and body) must arrive within
//!   [`Limits::deadline`] of the connection being accepted, however
//!   slowly it trickles in: a client sending one byte a second holds a
//!   worker for at most that long.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Bounds on what one connection may cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Request line plus headers, bytes.
    pub head: usize,
    /// Body, bytes.
    pub body: usize,
    /// Headers, count.
    pub headers: usize,
    /// From accept to the last byte of the body.
    pub deadline: Duration,
    /// Connections served at once; the next gets a 503.
    pub workers: usize,
    /// Of those, event streams open at once.
    pub streams: usize,
    /// How long one write may block before the client is dropped (a client
    /// that stops reading an event stream).
    pub write_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            head: 16 * 1024,
            body: 1 << 20,
            headers: 64,
            deadline: Duration::from_secs(5),
            workers: 32,
            streams: 8,
            write_timeout: Duration::from_secs(2),
        }
    }
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The target before `?`, as sent (still percent-encoded).
    pub path: String,
    /// The path's segments after the leading `/`, percent-decoded.
    pub segments: Vec<String>,
    /// Query pairs, percent-decoded (`+` is not a space here).
    pub query: Vec<(String, String)>,
    /// The token of an `Authorization: Bearer` header.
    pub bearer: Option<String>,
    /// Whether any `Authorization` header was sent.
    pub authorization: bool,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Why no request could be read: the status to answer with, or nothing
/// (the peer went away).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    Status(u16, &'static str),
    Closed,
}

fn bad(why: &'static str) -> ReadError {
    ReadError::Status(400, why)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Reads into `buf` until `done(buf)` or the deadline.
fn fill(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    deadline: Instant,
    cap: usize,
    mut done: impl FnMut(&[u8]) -> bool,
) -> Result<(), ReadError> {
    let mut chunk = [0u8; 4096];
    while !done(buf) {
        if buf.len() > cap {
            return Err(ReadError::Status(431, "request too large"));
        }
        let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
        else {
            return Err(ReadError::Status(408, "request took too long"));
        };
        stream
            .set_read_timeout(Some(left.min(Duration::from_millis(250))))
            .map_err(|_| ReadError::Closed)?;
        match stream.read(&mut chunk) {
            Ok(0) => return Err(ReadError::Closed),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ReadError::Closed),
        }
    }
    Ok(())
}

const fn tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Percent-decodes one path segment or query part into UTF-8.
///
/// # Errors
/// A `%` not followed by two hex digits, or bytes that are not UTF-8.
pub fn decode(s: &str) -> Result<String, &'static str> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3).ok_or("bad percent-encoding")?;
            let hex = std::str::from_utf8(hex).map_err(|_| "bad percent-encoding")?;
            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err("bad percent-encoding");
            }
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "bad percent-encoding")?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "path is not UTF-8")
}

/// Parses the head (everything before the blank line): the request
/// without its body, and the body's length.
///
/// # Errors
/// The status and reason for anything outside the subset in the
/// [module docs](self).
pub fn parse_head(head: &[u8], limits: &Limits) -> Result<(Request, usize), ReadError> {
    let head = std::str::from_utf8(head).map_err(|_| bad("head is not ASCII"))?;
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("malformed request line"));
    };
    if method.is_empty() || method.len() > 16 || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(bad("malformed method"));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ReadError::Status(505, "HTTP/1.0 or HTTP/1.1 only"));
    }
    if !target.starts_with('/') || target.len() > 2048 || !target.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(bad("malformed target"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path.contains('#') || query.contains('#') {
        return Err(bad("fragment in target"));
    }
    let segments = path[1..]
        .split('/')
        .map(decode)
        .collect::<Result<Vec<_>, _>>()
        .map_err(bad)?;
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Ok((decode(k)?, decode(v)?))
        })
        .collect::<Result<Vec<_>, &'static str>>()
        .map_err(bad)?;
    let mut req = Request {
        method: method.into(),
        path: path.into(),
        segments,
        query,
        bearer: None,
        authorization: false,
        content_type: None,
        body: Vec::new(),
    };
    let mut length: Option<usize> = None;
    let mut count = 0;
    for line in lines {
        count += 1;
        if count > limits.headers {
            return Err(ReadError::Status(431, "too many headers"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(bad("malformed header"));
        };
        if name.is_empty() || !name.bytes().all(tchar) {
            return Err(bad("malformed header name"));
        }
        if !value
            .bytes()
            .all(|b| b == b' ' || b == b'\t' || b.is_ascii_graphic())
        {
            return Err(bad("malformed header value"));
        }
        let value = value.trim_matches([' ', '\t']);
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                if length.is_some()
                    || value.is_empty()
                    || value.len() > 10
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(bad("malformed Content-Length"));
                }
                length = Some(value.parse().map_err(|_| bad("malformed Content-Length"))?);
            }
            "transfer-encoding" => return Err(ReadError::Status(501, "Transfer-Encoding is not supported")),
            "authorization" => {
                if req.authorization {
                    return Err(bad("more than one Authorization header"));
                }
                req.authorization = true;
                req.bearer = value
                    .split_once(' ')
                    .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
                    .map(|(_, t)| t.trim().to_string());
            }
            "content-type" => req.content_type = Some(value.to_ascii_lowercase()),
            _ => {}
        }
    }
    let length = length.unwrap_or(0);
    if length > limits.body {
        return Err(ReadError::Status(413, "body too large"));
    }
    Ok((req, length))
}

/// Reads one request from `stream`, within `limits`.
///
/// # Errors
/// See [`ReadError`].
pub fn read_request(
    stream: &mut TcpStream,
    limits: &Limits,
    accepted: Instant,
) -> Result<Request, ReadError> {
    let deadline = accepted + limits.deadline;
    let mut buf = Vec::with_capacity(1024);
    fill(stream, &mut buf, deadline, limits.head, |b| {
        find(b, b"\r\n\r\n").is_some()
    })?;
    let end = find(&buf, b"\r\n\r\n").unwrap_or(buf.len());
    if end > limits.head {
        return Err(ReadError::Status(431, "request head too large"));
    }
    let (mut req, length) = parse_head(&buf[..end], limits)?;
    let mut body = buf.split_off(end + 4);
    fill(stream, &mut body, deadline, limits.body, |b| b.len() >= length)?;
    body.truncate(length);
    req.body = body;
    Ok(req)
}

/// What to send back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    /// Extra headers. Values are built by the server from numbers and
    /// constants only; one carrying a control character is dropped.
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    #[must_use]
    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        let mut body = serde_json::to_vec(body).unwrap_or_default();
        body.push(b'\n');
        Self {
            status,
            content_type: "application/json",
            headers: Vec::new(),
            body,
        }
    }

    #[must_use]
    pub fn error(status: u16, why: &str) -> Self {
        Self::json(status, &serde_json::json!({ "error": why }))
    }

    #[must_use]
    pub fn with(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }
}

#[must_use]
pub const fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

/// The fixed headers every response carries.
const COMMON: &str = "Connection: close\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\n\
Referrer-Policy: no-referrer\r\nCache-Control: no-store\r\n";

/// Writes `r` with a `Content-Length`, within the stream's write timeout.
///
/// # Errors
/// The socket's.
pub fn respond(stream: &mut TcpStream, r: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{COMMON}",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    for (name, value) in &r.headers {
        if value.bytes().all(|b| b == b' ' || b.is_ascii_graphic()) {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

/// Starts an event stream (`text/event-stream`, no length, closed by
/// either side).
///
/// # Errors
/// The socket's.
pub fn start_stream(stream: &mut TcpStream) -> io::Result<()> {
    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n{COMMON}\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()
}
