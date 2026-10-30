//! A small HTTP/1.1 client on std (plus rustls for `https`): one request
//! per connection, `Connection: close`, a response body that may be
//! streamed (`Content-Length`, chunked, or until close).
//!
//! Every read is bounded twice: an idle timeout (a peer that stops
//! sending, such as a relay that hangs mid-stream) and one deadline for
//! the whole exchange. The response head is capped at [`MAX_HEAD`];
//! redirects are never followed. Errors name kinds, never header values
//! or body bytes, so a token cannot reach a log through one.

use std::fmt;
use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Most bytes of status line and headers read.
pub const MAX_HEAD: usize = 16 * 1024;
/// Longest chunk-size line.
const MAX_CHUNK_LINE: usize = 256;

/// Where a client sends: scheme, host, port and a path prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Without a trailing `/`; empty for the root.
    pub prefix: String,
}

impl Url {
    /// Parses `http://host[:port][/prefix]` or `https://...`. No user
    /// info, query or fragment.
    ///
    /// # Errors
    /// What is wrong with it, in words.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (https, rest) = if let Some(r) = s.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("http://") {
            (false, r)
        } else {
            return Err("the URL must start with http:// or https://".into());
        };
        if rest.bytes().any(|b| !b.is_ascii_graphic()) || rest.contains(['@', '?', '#']) {
            return Err("the URL may not carry user info, a query, a fragment or blanks".into());
        }
        let (authority, path) = rest.find('/').map_or((rest, ""), |k| rest.split_at(k));
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
                let port = p.parse::<u16>().map_err(|_| "bad port".to_string())?;
                (h, port)
            }
            _ => (authority, if https { 443 } else { 80 }),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() || port == 0 {
            return Err("the URL needs a host and a port above 0".into());
        }
        Ok(Self {
            https,
            host: host.to_string(),
            port,
            prefix: path.trim_end_matches('/').to_string(),
        })
    }

    /// The `Host` header value.
    #[must_use]
    pub fn host_header(&self) -> String {
        let h = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == if self.https { 443 } else { 80 } {
            h
        } else {
            format!("{h}:{}", self.port)
        }
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = if self.https { "https" } else { "http" };
        write!(f, "{scheme}://{}{}", self.host_header(), self.prefix)
    }
}

/// How long an exchange may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    /// Longest wait for the next byte.
    pub idle: Duration,
    /// The whole exchange, connect to last byte.
    pub total: Duration,
}

/// Why an exchange failed. Never carries a header value or body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Resolving or connecting failed: the peer is not reachable.
    Unreachable(String),
    /// No byte for [`Timeouts::idle`], or past [`Timeouts::total`].
    Timeout,
    Tls(String),
    /// An `https` URL in a build without the `https` feature.
    NoTls,
    /// The peer broke HTTP.
    Protocol(&'static str),
    Io(io::ErrorKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(why) => write!(f, "unreachable: {why}"),
            Self::Timeout => f.write_str("timed out"),
            Self::Tls(e) => write!(f, "TLS: {e}"),
            Self::NoTls => f.write_str("https:// needs a build with the https feature"),
            Self::Protocol(e) => write!(f, "bad response: {e}"),
            Self::Io(k) => write!(f, "connection failed: {k}"),
        }
    }
}

impl std::error::Error for Error {}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

/// Maps a read or write error to an [`Error`].
#[must_use]
pub fn io_error(e: &io::Error) -> Error {
    if is_timeout(e) {
        Error::Timeout
    } else if e.kind() == io::ErrorKind::InvalidData {
        Error::Protocol("invalid data on the connection")
    } else {
        Error::Io(e.kind())
    }
}

/// A socket whose reads wait at most the idle timeout and never past the
/// deadline; writes likewise.
struct Timed {
    s: TcpStream,
    idle: Duration,
    deadline: Instant,
}

impl Timed {
    fn wait(&self) -> io::Result<Duration> {
        let left = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
        Ok(left.min(self.idle))
    }
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.s.set_read_timeout(Some(self.wait()?))?;
        self.s.read(buf)
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.s.set_write_timeout(Some(self.wait()?))?;
        self.s.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.s.flush()
    }
}

/// A connection, plain or TLS.
trait Conn: Read + Write + Send {}
impl<T: Read + Write + Send> Conn for T {}

/// A response whose body is still on the wire.
pub struct Response {
    pub status: u16,
    /// Header names lowercased; values as sent.
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn BufRead + Send>,
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Response {
    /// A header's value, by lowercase name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Reads the whole body, at most `cap` bytes.
    ///
    /// # Errors
    /// A read error, or a body over `cap`.
    pub fn read_body(mut self, cap: usize) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
        (&mut self.body)
            .take(limit)
            .read_to_end(&mut out)
            .map_err(|e| io_error(&e))?;
        if out.len() > cap {
            return Err(Error::Protocol("response body too large"));
        }
        Ok(out)
    }
}

/// One request.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    /// Absolute path, already percent-encoded.
    pub path: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    /// Sent with `Content-Type: application/json` when present.
    pub json: Option<&'a [u8]>,
}

/// Sends `req` to `url` (its prefix is not added: `req.path` is whole)
/// and reads the response head.
///
/// # Errors
/// [`Error`].
pub fn send(url: &Url, req: &Request<'_>, t: Timeouts) -> Result<Response, Error> {
    let deadline = Instant::now() + t.total;
    let addrs: Vec<SocketAddr> = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|e| Error::Unreachable(format!("cannot resolve {}: {}", url.host, e.kind())))?
        .collect();
    let mut last = Error::Unreachable(format!("{} resolves to no address", url.host));
    let mut tcp = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, t.connect.min(t.total)) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last = Error::Unreachable(format!("cannot connect to {a}: {}", e.kind())),
        }
    }
    let tcp = tcp.ok_or(last)?;
    let _ = tcp.set_nodelay(true);
    let timed = Timed {
        s: tcp,
        idle: t.idle,
        deadline,
    };
    let mut conn: Box<dyn Conn> = if url.https {
        tls::wrap(timed, &url.host)?
    } else {
        Box::new(timed)
    };
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: helmsman-agent\r\nAccept: */*\r\nConnection: close\r\n",
        req.method,
        req.path,
        url.host_header()
    );
    for (n, v) in req.headers {
        head.push_str(n);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if let Some(body) = req.json {
        head.push_str("Content-Type: application/json\r\n");
        let _ = write!(head, "Content-Length: {}\r\n", body.len());
    } else if req.method != "GET" {
        head.push_str("Content-Length: 0\r\n");
    }
    head.push_str("\r\n");
    conn.write_all(head.as_bytes()).map_err(|e| io_error(&e))?;
    if let Some(body) = req.json {
        conn.write_all(body).map_err(|e| io_error(&e))?;
    }
    conn.flush().map_err(|e| io_error(&e))?;
    read_head(BufReader::new(conn))
}

fn read_line(r: &mut impl BufRead, cap: usize) -> Result<Vec<u8>, Error> {
    let mut line = Vec::new();
    loop {
        let buf = r.fill_buf().map_err(|e| io_error(&e))?;
        if buf.is_empty() {
            return Err(Error::Protocol("connection closed mid-line"));
        }
        let (n, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(k) => (k + 1, true),
            None => (buf.len(), false),
        };
        line.extend_from_slice(&buf[..n]);
        r.consume(n);
        if line.len() > cap {
            return Err(Error::Protocol("line too long"));
        }
        if done {
            while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                line.pop();
            }
            return Ok(line);
        }
    }
}

fn read_head(mut r: BufReader<Box<dyn Conn>>) -> Result<Response, Error> {
    let mut used = 0;
    let status_line = read_line(&mut r, MAX_HEAD)?;
    used += status_line.len();
    let status = parse_status(&status_line).ok_or(Error::Protocol("no HTTP status line"))?;
    let mut headers = Vec::new();
    loop {
        let line = read_line(&mut r, MAX_HEAD)?;
        used += line.len() + 2;
        if used > MAX_HEAD {
            return Err(Error::Protocol("response head over 16 KiB"));
        }
        if line.is_empty() {
            break;
        }
        let line = String::from_utf8(line).map_err(|_| Error::Protocol("header not UTF-8"))?;
        let Some((n, v)) = line.split_once(':') else {
            return Err(Error::Protocol("header without a colon"));
        };
        headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let find = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let chunked = find("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let body: Box<dyn Read + Send> = if chunked {
        Box::new(Chunked {
            r,
            left: 0,
            done: false,
        })
    } else if let Some(n) = find("content-length") {
        let n: u64 = n.parse().map_err(|_| Error::Protocol("bad Content-Length"))?;
        Box::new(r.take(n))
    } else {
        Box::new(r)
    };
    Ok(Response {
        status,
        headers,
        body: Box::new(BufReader::new(body)),
    })
}

fn parse_status(line: &[u8]) -> Option<u16> {
    let rest = line.strip_prefix(b"HTTP/1.")?;
    let (&minor, rest) = rest.split_first()?;
    if !minor.is_ascii_digit() {
        return None;
    }
    let code = rest.strip_prefix(b" ")?.get(..3)?;
    let s = std::str::from_utf8(code).ok()?;
    s.parse::<u16>().ok().filter(|n| (100..=599).contains(n))
}

/// A chunked body (RFC 9112 §7.1); trailers are read and dropped.
struct Chunked<R> {
    r: R,
    left: u64,
    done: bool,
}

fn bad(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let line = read_line(&mut self.r, MAX_CHUNK_LINE).map_err(|_| bad("bad chunk size line"))?;
            let line = std::str::from_utf8(&line).map_err(|_| bad("bad chunk size line"))?;
            let size = line.split(';').next().unwrap_or("").trim();
            let n = u64::from_str_radix(size, 16).map_err(|_| bad("bad chunk size"))?;
            if n == 0 {
                // Trailers, then the blank line.
                loop {
                    let l = read_line(&mut self.r, MAX_CHUNK_LINE).map_err(|_| bad("bad trailer"))?;
                    if l.is_empty() {
                        break;
                    }
                }
                self.done = true;
                return Ok(0);
            }
            self.left = n;
        }
        let want = usize::try_from(self.left).unwrap_or(usize::MAX).min(buf.len());
        let n = self.r.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        self.left -= n as u64;
        if self.left == 0 {
            let crlf = read_line(&mut self.r, 2).map_err(|_| bad("chunk not followed by CRLF"))?;
            if !crlf.is_empty() {
                return Err(bad("chunk not followed by CRLF"));
            }
        }
        Ok(n)
    }
}

#[cfg(feature = "https")]
mod tls {
    use std::sync::{Arc, OnceLock};

    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

    use super::{Conn, Error, Timed, io_error};

    fn config() -> Result<Arc<ClientConfig>, Error> {
        static CFG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
        CFG.get_or_init(|| {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map(|b| Arc::new(b.with_root_certificates(roots).with_no_client_auth()))
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(Error::Tls)
    }

    pub(super) fn wrap(s: Timed, host: &str) -> Result<Box<dyn Conn>, Error> {
        let name = ServerName::try_from(host.to_string()).map_err(|e| Error::Tls(e.to_string()))?;
        let conn = ClientConnection::new(config()?, name).map_err(|e| Error::Tls(e.to_string()))?;
        let mut stream = StreamOwned::new(conn, s);
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::InvalidData => Error::Tls(e.to_string()),
                    _ => io_error(&e),
                })?;
        }
        Ok(Box::new(stream))
    }
}

#[cfg(not(feature = "https"))]
mod tls {
    use super::{Conn, Error, Timed};

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn wrap(_: Timed, _: &str) -> Result<Box<dyn Conn>, Error> {
        Err(Error::NoTls)
    }
}

/// Percent-encodes one path segment (everything but unreserved bytes),
/// so a name can never add a segment, a query or a `..`.
#[must_use]
pub fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
