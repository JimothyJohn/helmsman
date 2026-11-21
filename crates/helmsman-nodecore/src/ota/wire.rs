//! The one request a node makes (`GET` the image) and the parser for the
//! head of the answer, which frames a body that goes straight into flash.
//! HTTP/1.1, `Connection: close`, the body read to its announced length.
//! Strict where it matters: a duplicate or disagreeing `Content-Length`,
//! a missing one, chunked encoding and a length over what a slot can hold
//! are all refused. Ported from supervictor's `ota/http.rs`.

use alloc::string::String;

use super::request::Fetch;

/// Most header bytes the node will buffer before giving up.
pub const MAX_HEAD_LEN: usize = 1024;

/// `GET <path>` with the headers a file server wants.
#[must_use]
pub fn image_request(fetch: &Fetch) -> String {
    alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: helmsman-node\r\nAccept: application/octet-stream\r\nConnection: close\r\n\r\n",
        fetch.path,
        fetch.authority()
    )
}

/// What the node needs from a response head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub content_length: Option<u32>,
}

/// Why a head could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadError {
    /// No blank line yet: read more (unless the buffer is full).
    Incomplete,
    /// Not `HTTP/1.x ddd`, a header without a colon, a length that is not
    /// a number.
    Malformed,
    /// Two `Content-Length` headers that disagree, or `Transfer-Encoding`.
    Unsupported,
    /// A length over `limit`.
    TooLarge,
}

impl core::fmt::Display for HeadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Incomplete => write!(f, "the response head did not end"),
            Self::Malformed => write!(f, "the response is not HTTP"),
            Self::Unsupported => write!(
                f,
                "the server framed the file in a way a node does not read (chunked, or two lengths)"
            ),
            Self::TooLarge => write!(f, "the file is larger than a slot"),
        }
    }
}

/// Parses the head at the start of `buf`; returns it and how many bytes it
/// took (the body starts there). Header names match case-insensitively and
/// unknown headers are skipped. `limit` is the largest `Content-Length`
/// accepted.
///
/// # Errors
/// [`HeadError`].
pub fn parse_head(buf: &[u8], limit: u32) -> Result<(Head, usize), HeadError> {
    let end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(HeadError::Incomplete)?;
    let head = core::str::from_utf8(&buf[..end]).map_err(|_| HeadError::Malformed)?;
    let mut lines = head.split("\r\n");
    let status = parse_status_line(lines.next().ok_or(HeadError::Malformed)?)?;
    let mut content_length: Option<u32> = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(HeadError::Malformed)?;
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-length") {
            let n = parse_u32(value).ok_or(HeadError::Malformed)?;
            if n > limit {
                return Err(HeadError::TooLarge);
            }
            match content_length {
                Some(prev) if prev != n => return Err(HeadError::Unsupported),
                _ => content_length = Some(n),
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(HeadError::Unsupported);
        }
    }
    Ok((
        Head {
            status,
            content_length,
        },
        end + 4,
    ))
}

fn parse_status_line(line: &str) -> Result<u16, HeadError> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().ok_or(HeadError::Malformed)?;
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(HeadError::Malformed);
    }
    let code = parts.next().ok_or(HeadError::Malformed)?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HeadError::Malformed);
    }
    code.parse().map_err(|_| HeadError::Malformed)
}

fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}
