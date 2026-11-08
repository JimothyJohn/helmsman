//! A bounded HTTP/1.1 reader and writer for one request per connection.
//!
//! The node answers one request and closes (`Connection: close`), as
//! supervictor's device server does; a browser's `EventSource` holds its
//! own connection for the values stream ([`sse_head`], [`sse_event`]).
//! Everything is bounded: a head over [`MAX_HEAD`] bytes or a body over
//! [`MAX_BODY`] is refused before it is read, so a hostile peer on the LAN
//! cannot make the node allocate its heap away.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Most bytes in the request line and headers.
pub const MAX_HEAD: usize = 4096;
/// Most bytes in a body (a deployed program, or a settings form).
pub const MAX_BODY: usize = 16 * 1024;

/// Why a request could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    /// No blank line within [`MAX_HEAD`] bytes.
    HeadTooLong,
    /// `Content-Length` past [`MAX_BODY`].
    BodyTooLong,
    /// Not a request line, or not UTF-8.
    Malformed,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeadTooLong => write!(f, "request head over {MAX_HEAD} bytes"),
            Self::BodyTooLong => write!(f, "request body over {MAX_BODY} bytes"),
            Self::Malformed => write!(f, "malformed request"),
        }
    }
}

impl core::error::Error for HttpError {}

/// One request, read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The path without its query.
    pub path: String,
    /// What followed `?`, if anything.
    pub query: Option<String>,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// A header's value, by lower-case name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The bearer token of an `Authorization` header, if there is one.
    #[must_use]
    pub fn bearer(&self) -> Option<&str> {
        let v = self.header("authorization")?;
        let (scheme, token) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
    }

    /// The path split on `/`, empty segments dropped: `/api/modules/x`
    /// is `["api", "modules", "x"]`.
    #[must_use]
    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }
}

/// What a read of `buf` so far says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// Not a whole request yet; read more.
    Incomplete,
    /// One request, and the bytes it used.
    Done(Request, usize),
}

/// Reads one request from the start of `buf`.
///
/// # Errors
/// A head or body over its bound, or a malformed request line; the
/// connection should answer 400 or 413 and close.
pub fn parse(buf: &[u8]) -> Result<Read, HttpError> {
    let Some(head_end) = find(buf, b"\r\n\r\n") else {
        return if buf.len() >= MAX_HEAD {
            Err(HttpError::HeadTooLong)
        } else {
            Ok(Read::Incomplete)
        };
    };
    if head_end > MAX_HEAD {
        return Err(HttpError::HeadTooLong);
    }
    let head = core::str::from_utf8(&buf[..head_end]).map_err(|_| HttpError::Malformed)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(HttpError::Malformed)?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .filter(|m| !m.is_empty())
        .ok_or(HttpError::Malformed)?;
    let target = parts.next().ok_or(HttpError::Malformed)?;
    let version = parts.next().ok_or(HttpError::Malformed)?;
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(HttpError::Malformed);
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(String::from(q))),
        None => (target, None),
    };
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(HttpError::Malformed);
        };
        let name = name.trim().to_ascii_lowercase();
        let value = String::from(value.trim());
        if name == "content-length" {
            content_length = value.parse().map_err(|_| HttpError::Malformed)?;
            if content_length > MAX_BODY {
                return Err(HttpError::BodyTooLong);
            }
        }
        headers.push((name, value));
    }
    let body_start = head_end + 4;
    let body_end = body_start + content_length;
    if buf.len() < body_end {
        return Ok(Read::Incomplete);
    }
    Ok(Read::Done(
        Request {
            method: String::from(method),
            path: String::from(path),
            query,
            headers,
            body: buf[body_start..body_end].to_vec(),
        },
        body_end,
    ))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// One reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Response {
    #[must_use]
    pub fn new(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: Vec::new(),
            body,
        }
    }

    /// A JSON body.
    #[must_use]
    pub fn json(status: u16, j: &serde_json::Value) -> Self {
        Self::new(status, "application/json", j.to_string().into_bytes())
    }

    /// `{"error": reason}` with `status`.
    #[must_use]
    pub fn error(status: u16, reason: &str) -> Self {
        Self::json(status, &serde_json::json!({ "error": reason }))
    }

    /// HTML.
    #[must_use]
    pub fn html(status: u16, body: String) -> Self {
        Self::new(status, "text/html; charset=utf-8", body.into_bytes())
    }

    /// Plain text.
    #[must_use]
    pub fn text(status: u16, body: &str) -> Self {
        Self::new(status, "text/plain; charset=utf-8", body.as_bytes().to_vec())
    }

    /// `303 See Other` to `location` after a form post.
    #[must_use]
    pub fn see_other(location: &str) -> Self {
        Self::new(303, "text/plain; charset=utf-8", Vec::new()).with("Location", String::from(location))
    }

    #[must_use]
    pub fn with(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }

    /// The reply's bytes: status line, headers, the CORS headers every
    /// node reply carries (a canvas served by a box or a dev server reads
    /// a node from another origin; the token is a header, so a wildcard
    /// origin gains nothing a LAN peer did not already have), a body.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.body.len());
        out.extend_from_slice(b"HTTP/1.1 ");
        out.extend_from_slice(self.status.to_string().as_bytes());
        out.extend_from_slice(b" ");
        out.extend_from_slice(reason(self.status).as_bytes());
        out.extend_from_slice(b"\r\nContent-Type: ");
        out.extend_from_slice(self.content_type.as_bytes());
        out.extend_from_slice(b"\r\nContent-Length: ");
        out.extend_from_slice(self.body.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\nConnection: close\r\nCache-Control: no-store\r\n");
        cors(&mut out);
        for (name, value) in &self.headers {
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

fn cors(out: &mut Vec<u8>) {
    out.extend_from_slice(
        b"Access-Control-Allow-Origin: *\r\n\
          Access-Control-Allow-Methods: GET, PUT, POST, OPTIONS\r\n\
          Access-Control-Allow-Headers: Authorization, Content-Type, X-Helmsman-Request\r\n\
          Access-Control-Max-Age: 600\r\n",
    );
}

/// The reason phrase for the statuses the node uses.
#[must_use]
pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// The head of an event stream reply; the connection then stays open and
/// carries [`sse_event`]s.
#[must_use]
pub fn sse_head() -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n",
    );
    cors(&mut out);
    out.extend_from_slice(b"\r\n");
    out
}

/// One `event: name` with its JSON `data` line, as the box frames them
/// (`crates/helmsman-plc/src/api/chat.rs::event`).
#[must_use]
pub fn sse_event(name: &str, data: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(b"event: ");
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\ndata: ");
    out.extend_from_slice(data.to_string().as_bytes());
    out.extend_from_slice(b"\n\n");
    out
}

/// Decodes `application/x-www-form-urlencoded` into pairs.
#[must_use]
pub fn form_pairs(body: &[u8]) -> Vec<(String, String)> {
    let Ok(text) = core::str::from_utf8(body) else {
        return Vec::new();
    };
    text.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (url_decode(k), url_decode(v))
        })
        .collect()
}

/// `%XX` and `+` decoding; bad escapes are kept as written.
#[must_use]
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Escapes text for an HTML attribute or element body.
#[must_use]
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use proptest::prelude::*;

    #[test]
    fn a_get_parses_with_its_query_and_headers() {
        let raw = b"GET /api/modules?x=1 HTTP/1.1\r\nHost: node\r\nAuthorization: Bearer abc\r\n\r\nextra";
        let Read::Done(req, used) = parse(raw).unwrap() else {
            panic!("incomplete")
        };
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/modules");
        assert_eq!(req.query.as_deref(), Some("x=1"));
        assert_eq!(req.header("host"), Some("node"));
        assert_eq!(req.bearer(), Some("abc"));
        assert_eq!(req.segments(), ["api", "modules"]);
        assert_eq!(used, raw.len() - 5);
        assert_eq!(req.body, b"");
    }

    #[test]
    fn a_body_waits_for_its_content_length() {
        let head = b"PUT /api/modules/chase HTTP/1.1\r\nContent-Length: 5\r\n\r\n";
        assert_eq!(parse(head).unwrap(), Read::Incomplete);
        let mut whole = head.to_vec();
        whole.extend_from_slice(b"PROGR");
        let Read::Done(req, used) = parse(&whole).unwrap() else {
            panic!("incomplete")
        };
        assert_eq!(req.body, b"PROGR");
        assert_eq!(used, whole.len());
    }

    #[test]
    fn bounds_hold() {
        let long = vec![b'a'; MAX_HEAD + 1];
        assert_eq!(parse(&long), Err(HttpError::HeadTooLong));
        let big = alloc::format!("PUT /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(parse(big.as_bytes()), Err(HttpError::BodyTooLong));
        assert_eq!(parse(b"nonsense\r\n\r\n"), Err(HttpError::Malformed));
        assert_eq!(parse(b"GET x HTTP/1.1\r\n\r\n"), Err(HttpError::Malformed));
        assert_eq!(
            parse(b"GET /x HTTP/1.1\r\nbad header\r\n\r\n"),
            Err(HttpError::Malformed)
        );
        assert_eq!(
            parse(b"GET /x HTTP/1.1\r\nContent-Length: nope\r\n\r\n"),
            Err(HttpError::Malformed)
        );
        assert_eq!(parse(b"GET /x HTTP/1."), Ok(Read::Incomplete));
    }

    #[test]
    fn replies_carry_length_cors_and_close() {
        let r = Response::error(404, "no").encode();
        let text = core::str::from_utf8(&r).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("Content-Length: 14\r\n"), "{text}");
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.contains("Access-Control-Allow-Origin: *\r\n"));
        assert!(text.ends_with("\r\n\r\n{\"error\":\"no\"}"));
        let s = Response::see_other("/settings").encode();
        assert!(
            core::str::from_utf8(&s)
                .unwrap()
                .contains("Location: /settings\r\n")
        );
    }

    #[test]
    fn sse_frames_as_the_box_does() {
        assert_eq!(
            sse_event("values", &serde_json::json!({ "seq": 1 })),
            b"event: values\ndata: {\"seq\":1}\n\n"
        );
        assert!(
            core::str::from_utf8(&sse_head())
                .unwrap()
                .contains("text/event-stream")
        );
    }

    #[test]
    fn forms_decode() {
        let pairs = form_pairs(b"name=press+4&site=line%2Da&empty=&token=a%3Db");
        assert_eq!(
            pairs,
            vec![
                ("name".into(), "press 4".into()),
                ("site".into(), "line-a".into()),
                ("empty".into(), String::new()),
                ("token".into(), "a=b".into()),
            ]
        );
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
        assert_eq!(
            html_escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    proptest! {
        #[test]
        fn parse_never_panics(buf in proptest::collection::vec(any::<u8>(), 0..600)) {
            let _ = parse(&buf);
        }

        #[test]
        fn decoding_never_panics(s in "[ -~]{0,64}") {
            let _ = url_decode(&s);
            let _ = form_pairs(s.as_bytes());
        }
    }
}
