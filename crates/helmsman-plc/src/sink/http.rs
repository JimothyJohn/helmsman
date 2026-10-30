//! A minimal HTTP/1.1 client for one thing: POST a small JSON body and read
//! the status. Written on std (plus rustls for `https`, behind the `https`
//! feature) so every safety rule is ours to enforce:
//!
//! - the host is resolved once, every address is checked
//!   ([`super::url::refused`]), and the socket connects to a checked
//!   address, never to the name again;
//! - one deadline covers connect, TLS, write and read;
//! - `Connection: close`, one request per connection;
//! - the response head is capped at [`MAX_HEAD`] and the body is never
//!   read, so a hostile receiver cannot make the box buffer anything;
//! - a 3xx is never followed, to any host: a redirect is a misconfigured
//!   sink, and following one is how SSRF gets past an address check;
//! - errors name kinds and statuses, never header values, the URL's path
//!   or the response body.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use super::secret::Secret;
use super::url::{Scheme, Url, refused};

/// Most bytes of response status line and headers read.
pub const MAX_HEAD: usize = 16 * 1024;

/// One POST.
pub struct Request<'a> {
    pub url: &'a Url,
    pub headers: &'a [(String, Secret)],
    /// Sent as `Idempotency-Key`.
    pub key: &'a str,
    pub body: &'a [u8],
}

/// What happened to one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 2xx: the receiver has it.
    Delivered(u16),
    /// 400, 413, 415, 422: the receiver will never take these bytes, so
    /// retrying them would only block the queue.
    Rejected(u16),
    /// Anything else: try again later.
    Retry(SendError),
}

/// Why an attempt did not reach a 2xx. Never carries a header value, a
/// URL path or response bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// `localhost` without `allow_local`.
    Localhost,
    Resolve(io::ErrorKind),
    NoAddress,
    Refused {
        addr: IpAddr,
        why: &'static str,
    },
    Connect(io::ErrorKind),
    Timeout,
    Io(io::ErrorKind),
    Tls(String),
    /// An `https` URL in a build without the `https` feature.
    NoTls,
    Protocol(&'static str),
    /// A 3xx, not followed.
    Redirect(u16),
    Status(u16),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Localhost => f.write_str("localhost refused (set allow_local=true to allow)"),
            Self::Resolve(k) => write!(f, "cannot resolve host: {k}"),
            Self::NoAddress => f.write_str("host resolves to no address"),
            Self::Refused { addr, why } => write!(f, "address {addr} refused: {why}"),
            Self::Connect(k) => write!(f, "cannot connect: {k}"),
            Self::Timeout => f.write_str("timed out"),
            Self::Io(k) => write!(f, "connection failed: {k}"),
            Self::Tls(e) => write!(f, "TLS: {e}"),
            Self::NoTls => f.write_str("https:// needs a build with the https feature"),
            Self::Protocol(e) => write!(f, "bad response: {e}"),
            Self::Redirect(s) => write!(f, "redirect (status {s}) not followed"),
            Self::Status(s) => write!(f, "status {s}"),
        }
    }
}

impl std::error::Error for SendError {}

/// Resolves a host and port; [`std_resolve`] in the box, a fake in tests.
pub type Resolve<'a> = &'a dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>>;

/// The system resolver.
///
/// # Errors
/// What the lookup returns.
pub fn std_resolve(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    Ok((host, port).to_socket_addrs()?.collect())
}

/// POSTs `req`, all within `timeout`.
#[must_use]
pub fn post(req: &Request<'_>, timeout: Duration, allow_local: bool, resolve: Resolve<'_>) -> Outcome {
    match attempt(req, timeout, allow_local, resolve) {
        Ok(status) => classify(status),
        Err(e) => Outcome::Retry(e),
    }
}

fn classify(status: u16) -> Outcome {
    match status {
        200..=299 => Outcome::Delivered(status),
        400 | 413 | 415 | 422 => Outcome::Rejected(status),
        300..=399 => Outcome::Retry(SendError::Redirect(status)),
        _ => Outcome::Retry(SendError::Status(status)),
    }
}

/// The addresses `url` may be reached at.
///
/// # Errors
/// A refused name or address (any one refuses the URL), or a failed lookup.
pub fn addresses(url: &Url, allow_local: bool, resolve: Resolve<'_>) -> Result<Vec<SocketAddr>, SendError> {
    if url.is_localhost() && !allow_local {
        return Err(SendError::Localhost);
    }
    let addrs = match url.host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, url.port)],
        Err(_) => resolve(&url.host, url.port).map_err(|e| SendError::Resolve(e.kind()))?,
    };
    if addrs.is_empty() {
        return Err(SendError::NoAddress);
    }
    for a in &addrs {
        if let Some(why) = refused(a.ip(), allow_local) {
            return Err(SendError::Refused { addr: a.ip(), why });
        }
    }
    Ok(addrs)
}

fn attempt(
    req: &Request<'_>,
    timeout: Duration,
    allow_local: bool,
    resolve: Resolve<'_>,
) -> Result<u16, SendError> {
    let deadline = Instant::now() + timeout;
    let addrs = addresses(req.url, allow_local, resolve)?;
    let mut last = SendError::NoAddress;
    let mut tcp = None;
    for a in addrs {
        let left = left(deadline).map_err(|_| SendError::Timeout)?;
        match TcpStream::connect_timeout(&a, left) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) if is_timeout(&e) => last = SendError::Timeout,
            Err(e) => last = SendError::Connect(e.kind()),
        }
    }
    let tcp = tcp.ok_or(last)?;
    let _ = tcp.set_nodelay(true);
    let stream = Timed { s: tcp, deadline };
    match req.url.scheme {
        Scheme::Http => exchange(stream, req),
        Scheme::Https => tls::exchange(stream, req),
    }
}

fn exchange<S: Read + Write>(mut s: S, req: &Request<'_>) -> Result<u16, SendError> {
    s.write_all(&head(req)).map_err(|e| io_err(&e))?;
    s.write_all(req.body).map_err(|e| io_err(&e))?;
    s.flush().map_err(|e| io_err(&e))?;
    read_status(&mut s)
}

/// The request line and headers.
fn head(req: &Request<'_>) -> Vec<u8> {
    let mut h = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: helmsman-plc\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIdempotency-Key: {}\r\nConnection: close\r\n",
        req.url.path(),
        req.url.host_header(),
        req.body.len(),
        req.key,
    );
    for (name, value) in req.headers {
        h.push_str(name);
        h.push_str(": ");
        h.push_str(value.expose());
        h.push_str("\r\n");
    }
    h.push_str("\r\n");
    h.into_bytes()
}

/// Reads the response head (at most [`MAX_HEAD`] bytes) and returns the
/// status. The body is left unread.
fn read_status<R: Read>(r: &mut R) -> Result<u16, SendError> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = find_end(&buf) {
            buf.truncate(end);
            break;
        }
        if buf.len() >= MAX_HEAD {
            return Err(SendError::Protocol("response head over 16 KiB"));
        }
        let room = (MAX_HEAD - buf.len()).min(chunk.len());
        match r.read(&mut chunk[..room]) {
            Ok(0) => {
                if buf.is_empty() {
                    return Err(SendError::Protocol("connection closed without a response"));
                }
                // A head cut short still has a status line worth reading.
                break;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_err(&e)),
        }
    }
    status_line(&buf).ok_or(SendError::Protocol("no HTTP status line"))
}

fn find_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn status_line(head: &[u8]) -> Option<u16> {
    let line = head.split(|&b| b == b'\n').next()?;
    let rest = line.strip_prefix(b"HTTP/1.")?;
    let (&minor, rest) = rest.split_first()?;
    if !minor.is_ascii_digit() {
        return None;
    }
    let rest = rest.strip_prefix(b" ")?;
    let code = rest.get(..3)?;
    if !code.iter().all(u8::is_ascii_digit) || rest.get(3).is_some_and(|&b| b != b' ' && b != b'\r') {
        return None;
    }
    let n = u16::from(code[0] - b'0') * 100 + u16::from(code[1] - b'0') * 10 + u16::from(code[2] - b'0');
    (100..=599).contains(&n).then_some(n)
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

fn io_err(e: &io::Error) -> SendError {
    if is_timeout(e) {
        SendError::Timeout
    } else {
        SendError::Io(e.kind())
    }
}

fn left(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))
}

/// A socket whose every read and write is bounded by one deadline.
struct Timed {
    s: TcpStream,
    deadline: Instant,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.s.set_read_timeout(Some(left(self.deadline)?))?;
        self.s.read(buf)
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.s.set_write_timeout(Some(left(self.deadline)?))?;
        self.s.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.s.flush()
    }
}

#[cfg(feature = "https")]
mod tls {
    use std::sync::{Arc, OnceLock};

    use rustls::pki_types::ServerName;
    use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

    use super::{Request, SendError, Timed};

    fn config() -> Result<Arc<ClientConfig>, SendError> {
        static CFG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
        CFG.get_or_init(|| {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map(|b| Arc::new(b.with_root_certificates(roots).with_no_client_auth()))
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(SendError::Tls)
    }

    pub(super) fn exchange(s: Timed, req: &Request<'_>) -> Result<u16, SendError> {
        let name = ServerName::try_from(req.url.host.clone()).map_err(|e| SendError::Tls(e.to_string()))?;
        let conn = ClientConnection::new(config()?, name).map_err(|e| SendError::Tls(e.to_string()))?;
        let mut stream = StreamOwned::new(conn, s);
        // Drive the handshake first so a TLS failure reads as one.
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::InvalidData => SendError::Tls(e.to_string()),
                    _ => super::io_err(&e),
                })?;
        }
        super::exchange(stream, req)
    }
}

#[cfg(not(feature = "https"))]
mod tls {
    use super::{Request, SendError, Timed};

    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn exchange(_: Timed, _: &Request<'_>) -> Result<u16, SendError> {
        Err(SendError::NoTls)
    }
}
