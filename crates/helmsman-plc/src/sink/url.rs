//! Sink URLs and the addresses a sink may reach.
//!
//! A sink URL is written by an integrator into box config, but it is still
//! treated as untrusted: a sink is a way to make the box send requests,
//! which is exactly what SSRF abuses.
//!
//! - Schemes: `http` and `https` only.
//! - No user info (`user:pass@`): credentials belong in a header loaded
//!   from a file, never in a URL that ends up in config listings.
//! - No fragment, no whitespace or control characters, a port in 1..=65535.
//! - Every address the host resolves to is checked ([`refused`]) before
//!   connecting, and the connection goes to a checked address, so a name
//!   cannot resolve to something safe for the check and something else
//!   for the connect. If any resolved address is refused, the URL is.
//! - Redirects are never followed (see [`crate::sink::http`]).
//!
//! Plant-LAN addresses (`10/8`, `172.16/12`, `192.168/16`, `fc00::/7`) are allowed:
//! a MES on the plant network is the main receiver. Loopback is refused
//! unless the sink says `allow_local=true`.
//!
//! Only [`Url::origin`] is ever logged: a webhook URL's path or query often
//! *is* the credential (`/hooks/T0KEN`).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

/// A parsed sink URL. `Debug` leaves the path out.
#[derive(Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: Scheme,
    /// Lowercase; an IPv6 literal without brackets.
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Scheme,
    UserInfo,
    Fragment,
    Chars,
    Host,
    Port,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Scheme => "only http:// and https:// URLs",
            Self::UserInfo => "no user:password@ in a URL; put credentials in a header",
            Self::Fragment => "no #fragment in a URL",
            Self::Chars => "only printable ASCII without spaces in a URL",
            Self::Host => "missing or malformed host",
            Self::Port => "port must be 1..=65535",
        })
    }
}

impl std::error::Error for UrlError {}

impl Url {
    /// Parses a sink URL.
    ///
    /// # Errors
    /// Why the URL is refused; the error never repeats the URL.
    pub fn parse(s: &str) -> Result<Self, UrlError> {
        if !s.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(UrlError::Chars);
        }
        let (scheme, rest) = match s.split_once("://") {
            Some((sc, rest)) if sc.eq_ignore_ascii_case("http") => (Scheme::Http, rest),
            Some((sc, rest)) if sc.eq_ignore_ascii_case("https") => (Scheme::Https, rest),
            _ => return Err(UrlError::Scheme),
        };
        if rest.contains('#') {
            return Err(UrlError::Fragment);
        }
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if authority.contains('@') {
            return Err(UrlError::UserInfo);
        }
        let path = match tail.chars().next() {
            None => "/".to_string(),
            Some('?') => format!("/{tail}"),
            Some(_) => tail.to_string(),
        };
        let default = match scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        };
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or(UrlError::Host)?;
            h.parse::<Ipv6Addr>().map_err(|_| UrlError::Host)?;
            let port = match after {
                "" => default,
                p => port(p.strip_prefix(':').ok_or(UrlError::Host)?)?,
            };
            (h.to_ascii_lowercase(), port)
        } else {
            let (h, p) = match authority.rsplit_once(':') {
                Some((h, p)) => (h, port(p)?),
                None => (authority, default),
            };
            if !host_ok(h) {
                return Err(UrlError::Host);
            }
            (h.to_ascii_lowercase(), p)
        };
        Ok(Self {
            scheme,
            host,
            port,
            path,
        })
    }

    /// `scheme://host:port`: the only part of a URL that is ever logged.
    #[must_use]
    pub fn origin(&self) -> String {
        let s = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        if self.host.contains(':') {
            format!("{s}://[{}]:{}", self.host, self.port)
        } else {
            format!("{s}://{}:{}", self.host, self.port)
        }
    }

    /// Path and query, for the request line only.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The `Host` header value.
    #[must_use]
    pub fn host_header(&self) -> String {
        let default = match self.scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        };
        let h = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == default {
            h
        } else {
            format!("{h}:{}", self.port)
        }
    }

    /// Whether the host is a name for this machine (`localhost`,
    /// `*.localhost`), refused before any lookup unless local is allowed.
    #[must_use]
    pub fn is_localhost(&self) -> bool {
        self.host == "localhost" || self.host.ends_with(".localhost")
    }
}

impl fmt::Debug for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Url({}/…)", self.origin())
    }
}

fn port(p: &str) -> Result<u16, UrlError> {
    if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
        return Err(UrlError::Port);
    }
    p.parse::<u16>().ok().filter(|&n| n != 0).ok_or(UrlError::Port)
}

/// A DNS name or dotted IPv4: labels of letters, digits and `-`, not
/// starting or ending with `-`, 1..=253 characters in all.
fn host_ok(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Why a sink may not connect to `ip`, or `None` if it may.
#[must_use]
pub fn refused(ip: IpAddr, allow_local: bool) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => refused_v4(v4, allow_local),
        IpAddr::V6(v6) => refused_v6(v6, allow_local),
    }
}

fn refused_v4(ip: Ipv4Addr, allow_local: bool) -> Option<&'static str> {
    let o = ip.octets();
    if ip.is_loopback() {
        return (!allow_local).then_some("loopback (set allow_local=true to allow)");
    }
    if ip.is_link_local() {
        return Some("link-local, where cloud metadata services live");
    }
    if o[0] == 0 {
        return Some("unspecified (0.0.0.0/8)");
    }
    if ip.is_broadcast() || ip.is_multicast() || o[0] >= 240 {
        return Some("broadcast, multicast or reserved");
    }
    if o == [100, 100, 100, 200] || o == [192, 0, 0, 192] {
        return Some("a cloud metadata address");
    }
    None
}

fn refused_v6(ip: Ipv6Addr, allow_local: bool) -> Option<&'static str> {
    let s = ip.segments();
    if ip.is_loopback() {
        return (!allow_local).then_some("loopback (set allow_local=true to allow)");
    }
    if ip.is_unspecified() {
        return Some("unspecified (::)");
    }
    if ip.is_multicast() {
        return Some("multicast");
    }
    // fe80::/10 link-local, fec0::/10 deprecated site-local.
    if s[0] & 0xffc0 == 0xfe80 || s[0] & 0xffc0 == 0xfec0 {
        return Some("link-local");
    }
    if ip == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
        return Some("a cloud metadata address");
    }
    // An IPv4 address carried in IPv6: mapped (::ffff:a.b.c.d), compatible
    // (::a.b.c.d) and NAT64 (64:ff9b::a.b.c.d) all reach that IPv4 host.
    let embedded = match s {
        [0, 0, 0, 0, 0, 0 | 0xffff, ..] | [0x64, 0xff9b, 0, 0, 0, 0, ..] => Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        )),
        _ => None,
    };
    embedded.and_then(|v4| refused_v4(v4, allow_local))
}
