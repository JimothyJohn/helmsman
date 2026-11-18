//! What `POST /api/firmware` asks for: a URL to fetch and the SHA-256 the
//! image must have. The URL is plain `http://a.b.c.d[:port]/path` with an
//! IPv4 literal: a node on a plant network has no DNS to ask and no TLS to
//! check a certificate with, so the digest in the (token-guarded) request
//! is what makes the image trustworthy, not the connection.

use alloc::string::String;
use core::fmt;

/// Longest path (with its query) accepted.
pub const MAX_PATH: usize = 200;

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// Not JSON, or `url` or `sha256` missing or not a string.
    Shape,
    /// Not `http://a.b.c.d[:port]/path`.
    Url(&'static str),
    /// Not 64 hex characters.
    Digest,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shape => write!(
                f,
                "want {{\"url\": \"http://a.b.c.d:port/file.bin\", \"sha256\": \"<64 hex>\"}}"
            ),
            Self::Url(why) => write!(f, "url: {why}"),
            Self::Digest => write!(f, "sha256: 64 hex characters"),
        }
    }
}

/// A validated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub host: [u8; 4],
    pub port: u16,
    /// Starts with `/`; printable ASCII without spaces.
    pub path: String,
    pub sha256: [u8; 32],
}

impl Fetch {
    /// Reads the JSON body of `POST /api/firmware`.
    ///
    /// # Errors
    /// What is wrong with it.
    pub fn from_json(body: &[u8]) -> Result<Self, Invalid> {
        let j: serde_json::Value = serde_json::from_slice(body).map_err(|_| Invalid::Shape)?;
        let url = j["url"].as_str().ok_or(Invalid::Shape)?;
        let digest = j["sha256"].as_str().ok_or(Invalid::Shape)?;
        Self::new(url, digest)
    }

    /// Checks a URL and a hex digest.
    ///
    /// # Errors
    /// What is wrong with either.
    pub fn new(url: &str, digest: &str) -> Result<Self, Invalid> {
        let sha256 = decode_hex32(digest).ok_or(Invalid::Digest)?;
        let rest = url
            .strip_prefix("http://")
            .ok_or(Invalid::Url("only http:// (a node has no TLS for this)"))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => return Err(Invalid::Url("no path after the address")),
        };
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => (h, parse_port(p)?),
            None => (authority, 80),
        };
        let host = parse_ipv4(host)?;
        if path.len() > MAX_PATH {
            return Err(Invalid::Url("path too long"));
        }
        if !path.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Invalid::Url("path: printable characters without spaces"));
        }
        if path.contains("..") || path.contains('#') {
            return Err(Invalid::Url("path: no '..' and no fragment"));
        }
        Ok(Self {
            host,
            port,
            path: String::from(path),
            sha256,
        })
    }

    /// `a.b.c.d:port`, for the `Host` header and the log.
    #[must_use]
    pub fn authority(&self) -> String {
        alloc::format!(
            "{}.{}.{}.{}:{}",
            self.host[0],
            self.host[1],
            self.host[2],
            self.host[3],
            self.port
        )
    }
}

fn parse_port(s: &str) -> Result<u16, Invalid> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Invalid::Url("port is not a number"));
    }
    match s.parse::<u16>() {
        Ok(p) if p != 0 => Ok(p),
        _ => Err(Invalid::Url("port out of range")),
    }
}

fn parse_ipv4(s: &str) -> Result<[u8; 4], Invalid> {
    const NOT_V4: Invalid = Invalid::Url("the host must be an IPv4 address (a node has no DNS)");
    let mut out = [0u8; 4];
    let mut parts = s.split('.');
    for slot in &mut out {
        let p = parts.next().ok_or(NOT_V4)?;
        // No leading zeros: "010" is octal in some parsers, a trap.
        if p.is_empty()
            || p.len() > 3
            || !p.bytes().all(|b| b.is_ascii_digit())
            || (p.len() > 1 && p.starts_with('0'))
        {
            return Err(NOT_V4);
        }
        *slot = p.parse::<u8>().map_err(|_| NOT_V4)?;
    }
    if parts.next().is_some() {
        return Err(NOT_V4);
    }
    Ok(out)
}

/// 64 hex characters to 32 bytes; anything else is `None`.
#[must_use]
pub fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (o, pair) in out.iter_mut().zip(b.as_chunks::<2>().0) {
        *o = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

/// Lowercase hex of a digest.
#[must_use]
pub fn encode_hex32(d: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(64);
    for b in d {
        s.push(char::from(HEX[usize::from(b >> 4)]));
        s.push(char::from(HEX[usize::from(b & 0xf)]));
    }
    s
}
