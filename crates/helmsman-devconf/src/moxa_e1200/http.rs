//! The console's HTTP client, on std: one request per connection,
//! `Connection: close`, one deadline for the whole exchange, a capped
//! response head and body, no redirects. Errors name kinds and never carry
//! a header value or body bytes (a cookie or a token cannot reach a log
//! through one). The same style as helmsman-agent's `http.rs`.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use super::{Config, Error};

/// Most cookies kept.
const MAX_COOKIES: usize = 8;

pub(super) struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub(super) struct Http {
    host: String,
    port: u16,
    cfg: Config,
    cookies: Vec<(String, String)>,
    last: Option<Instant>,
}

/// `host` or `host:port`; hostnames and IPv4 only (no IPv6 literal).
pub(super) fn parse_addr(addr: &str) -> Result<(String, u16), Error> {
    let bad = || Error::Refused("the address is host or host:port (a name or an IPv4 address)".into());
    let (host, port) = match addr.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok().filter(|&p| p != 0).ok_or_else(bad)?),
        None => (addr, 80),
    };
    let ok = !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['-', '.'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if ok {
        Ok((host.to_string(), port))
    } else {
        Err(bad())
    }
}

impl Http {
    pub(super) fn new(host: String, port: u16, cfg: Config) -> Self {
        Self {
            host,
            port,
            cfg,
            cookies: Vec::new(),
            last: None,
        }
    }

    pub(super) fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether TCP `port` on the console's host accepts a connection.
    pub(super) fn tcp_open(&self, port: u16, timeout: Duration) -> bool {
        (self.host.as_str(), port)
            .to_socket_addrs()
            .is_ok_and(|mut a| a.any(|a| TcpStream::connect_timeout(&a, timeout).is_ok()))
    }

    pub(super) fn forget_cookies(&mut self) {
        self.cookies.clear();
    }

    pub(super) fn cfg(&self) -> &Config {
        &self.cfg
    }

    /// A GET, retried once on a transport failure (the console sometimes
    /// drops a connection opened right after the last one closed). Only
    /// reads go through here.
    pub(super) fn get(&mut self, path: &str) -> Result<Reply, Error> {
        match self.once("GET", path, None, None) {
            Err(e) if e.is_transport() => {
                std::thread::sleep(self.cfg.retry_wait);
                self.once("GET", path, None, None)
            }
            r => r,
        }
    }

    /// One attempt, never retried: every write goes through here.
    pub(super) fn once(
        &mut self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
        total: Option<Duration>,
    ) -> Result<Reply, Error> {
        if path.bytes().any(|b| !b.is_ascii_graphic()) || !path.starts_with('/') {
            return Err(Error::Refused(
                "internal: a request path with blanks or controls".into(),
            ));
        }
        if let Some(t) = self.last {
            let since = t.elapsed();
            if since < self.cfg.gap {
                std::thread::sleep(self.cfg.gap.saturating_sub(since));
            }
        }
        let r = self.exchange(method, path, body, total.unwrap_or(self.cfg.request));
        self.last = Some(Instant::now());
        r
    }

    fn exchange(
        &mut self,
        method: &str,
        path: &str,
        body: Option<(&str, &[u8])>,
        total: Duration,
    ) -> Result<Reply, Error> {
        let deadline = Instant::now() + total;
        let addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| Error::Unreachable(format!("cannot resolve the host: {}", e.kind())))?;
        let mut last = Error::Unreachable("the host resolves to no address".into());
        let mut tcp = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, self.cfg.connect.min(total)) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last = Error::Unreachable(format!("cannot connect: {}", e.kind())),
            }
        }
        let mut tcp = tcp.ok_or(last)?;
        let _ = tcp.set_nodelay(true);

        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: helmsman-devconf\r\nAccept: */*\r\nConnection: close\r\n",
            self.host_header()
        );
        if !self.cookies.is_empty() {
            let jar: Vec<String> = self.cookies.iter().map(|(n, v)| format!("{n}={v}")).collect();
            head.push_str("Cookie: ");
            head.push_str(&jar.join("; "));
            head.push_str("\r\n");
        }
        if let Some((ct, b)) = body {
            let _ = write!(head, "Content-Type: {ct}\r\nContent-Length: {}\r\n", b.len());
        }
        head.push_str("\r\n");
        let write = |tcp: &mut TcpStream, bytes: &[u8]| -> Result<(), Error> {
            tcp.set_write_timeout(Some(left(deadline)?))
                .map_err(|e| io_err(&e))?;
            tcp.write_all(bytes).map_err(|e| io_err(&e))
        };
        write(&mut tcp, head.as_bytes())?;
        if let Some((_, b)) = body {
            write(&mut tcp, b)?;
        }
        let (reply, set_cookies) = read_response(&mut tcp, deadline, self.cfg.max_head, self.cfg.max_body)?;
        for c in set_cookies {
            self.store_cookie(&c);
        }
        Ok(reply)
    }

    fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// `Set-Cookie: name=value; attributes`: the pair only, and only if it
    /// is made of plain characters.
    fn store_cookie(&mut self, header: &str) {
        let pair = header.split(';').next().unwrap_or("");
        let Some((n, v)) = pair.split_once('=') else {
            return;
        };
        let (n, v) = (n.trim(), v.trim());
        let name_ok = !n.is_empty()
            && n.len() <= 64
            && n.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b));
        let value_ok = v.len() <= 256 && v.bytes().all(|b| b.is_ascii_graphic() && !b";,\"\\".contains(&b));
        if !name_ok || !value_ok {
            return;
        }
        if let Some(slot) = self.cookies.iter_mut().find(|(k, _)| k == n) {
            slot.1 = v.to_string();
        } else if self.cookies.len() < MAX_COOKIES {
            self.cookies.push((n.to_string(), v.to_string()));
        }
    }
}

fn left(deadline: Instant) -> Result<Duration, Error> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(Error::Timeout)
}

fn io_err(e: &io::Error) -> Error {
    if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
        Error::Timeout
    } else {
        Error::Io(e.kind())
    }
}

/// A read under the deadline. `got_any` says whether the peer has sent a
/// byte yet: a close or reset before that is "no response", the dropped
/// connection the console is known for.
fn read_some(tcp: &mut TcpStream, buf: &mut [u8], deadline: Instant, got_any: bool) -> Result<usize, Error> {
    tcp.set_read_timeout(Some(left(deadline)?))
        .map_err(|e| io_err(&e))?;
    match tcp.read(buf) {
        Ok(0) if !got_any => Err(Error::NoResponse),
        Ok(n) => Ok(n),
        Err(e)
            if !got_any
                && matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::BrokenPipe
                ) =>
        {
            Err(Error::NoResponse)
        }
        Err(e) => Err(io_err(&e)),
    }
}

fn read_response(
    tcp: &mut TcpStream,
    deadline: Instant,
    max_head: usize,
    max_body: usize,
) -> Result<(Reply, Vec<String>), Error> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    // The head: up to a blank line, at most max_head bytes.
    let (head_end, body_start) = loop {
        if let Some(found) = head_terminator(&buf) {
            break found;
        }
        if buf.len() > max_head {
            return Err(Error::Protocol("response head too large"));
        }
        let n = read_some(tcp, &mut chunk, deadline, !buf.is_empty())?;
        if n == 0 {
            return Err(Error::Protocol("connection closed inside the response head"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    if head_end > max_head {
        return Err(Error::Protocol("response head too large"));
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let status = parse_status(lines.next().unwrap_or("")).ok_or(Error::Protocol("no HTTP status line"))?;
    let mut length: Option<usize> = None;
    let mut cookies = Vec::new();
    for line in lines {
        let Some((n, v)) = line.split_once(':') else {
            return Err(Error::Protocol("a header without a colon"));
        };
        let (n, v) = (n.trim().to_ascii_lowercase(), v.trim());
        match n.as_str() {
            "content-length" => {
                let l: usize = v.parse().map_err(|_| Error::Protocol("bad Content-Length"))?;
                if length.is_some_and(|p| p != l) {
                    return Err(Error::Protocol("conflicting Content-Length headers"));
                }
                length = Some(l);
            }
            "transfer-encoding" => return Err(Error::Protocol("Transfer-Encoding is not supported")),
            "set-cookie" if cookies.len() < 2 * MAX_COOKIES => cookies.push(v.to_string()),
            _ => {}
        }
    }
    if (300..400).contains(&status) {
        return Err(Error::Redirect(status));
    }
    let mut body = buf.split_off(body_start);
    match length {
        Some(l) if l > max_body => return Err(Error::TooLarge),
        Some(l) => {
            while body.len() < l {
                let n = read_some(tcp, &mut chunk, deadline, true)?;
                if n == 0 {
                    return Err(Error::Protocol("body shorter than its Content-Length"));
                }
                body.extend_from_slice(&chunk[..n]);
            }
            body.truncate(l);
        }
        None => loop {
            if body.len() > max_body {
                return Err(Error::TooLarge);
            }
            let n = read_some(tcp, &mut chunk, deadline, true)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        },
    }
    if body.len() > max_body {
        return Err(Error::TooLarge);
    }
    Ok((Reply { status, body }, cookies))
}

/// `(end of the head, start of the body)` once a blank line has arrived.
fn head_terminator(buf: &[u8]) -> Option<(usize, usize)> {
    (0..buf.len()).find_map(|i| {
        if buf[i..].starts_with(b"\r\n\r\n") {
            Some((i, i + 4))
        } else if buf[i..].starts_with(b"\n\n") {
            Some((i, i + 2))
        } else {
            None
        }
    })
}

fn parse_status(line: &str) -> Option<u16> {
    let rest = line.strip_prefix("HTTP/1.")?;
    let rest = rest.strip_prefix(['0', '1'])?.strip_prefix(' ')?;
    let code: u16 = rest.get(..3)?.parse().ok()?;
    (100..=599).contains(&code).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(parse_addr("192.0.2.5").unwrap(), ("192.0.2.5".into(), 80));
        assert_eq!(
            parse_addr("moxa-1.lan:8080").unwrap(),
            ("moxa-1.lan".into(), 8080)
        );
        for bad in [
            "",
            ":80",
            "a b",
            "a:0",
            "a:99999",
            "a/b",
            "-x",
            "[::1]:80",
            "a:b",
            "http://x",
            "x\r\nHost: y",
        ] {
            assert!(parse_addr(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn status_lines() {
        assert_eq!(parse_status("HTTP/1.1 200 OK"), Some(200));
        assert_eq!(parse_status("HTTP/1.0 404"), Some(404));
        for bad in [
            "",
            "HTTP/2 200",
            "HTTP/1.1 20",
            "HTTP/1.1 999",
            "HTTP/1.1 abc",
            "200 OK",
            "HTTP/1.9 200",
        ] {
            assert_eq!(parse_status(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn cookies_keep_the_pair_and_drop_the_rest() {
        let mut h = Http::new("x".into(), 80, Config::default());
        h.store_cookie("ChallID=4739; path=/; HttpOnly");
        h.store_cookie("MoxaPass=ab12cd");
        h.store_cookie("bad name=1");
        h.store_cookie("x=a\"b");
        h.store_cookie("noequals");
        h.store_cookie("ChallID=99");
        assert_eq!(h.cookie("ChallID"), Some("99"));
        assert_eq!(h.cookie("MoxaPass"), Some("ab12cd"));
        assert_eq!(h.cookies.len(), 2);
        for i in 0..20 {
            h.store_cookie(&format!("c{i}=1"));
        }
        assert_eq!(h.cookies.len(), MAX_COOKIES);
    }

    #[test]
    fn heads_end_at_a_blank_line() {
        assert_eq!(
            head_terminator(b"HTTP/1.1 200 OK\r\nA: b\r\n\r\nbody"),
            Some((21, 25))
        );
        assert_eq!(head_terminator(b"HTTP/1.1 200 OK\n\nbody"), Some((15, 17)));
        assert_eq!(head_terminator(b"HTTP/1.1 200 OK\r\nA: b\r\n"), None);
    }
}
