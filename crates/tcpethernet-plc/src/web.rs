//! Browser programming interface over a small std-only HTTP/1.1 server.
//!
//! - `GET /`, `/app.js`, `/app.css`: the editor page (no auth; it holds no data).
//! - Everything under `/api/` needs `Authorization: Bearer <token>`. A
//!   header, not a cookie, so another site cannot ride a logged-in browser.
//!   - `GET /api/state`: JSON snapshot (identity, run, I/Q/M, connections, stats).
//!   - `GET /api/program`: the program source.
//!   - `PUT /api/program`: compile and swap in; 422 with errors otherwise.
//!   - `PUT /api/memory/<n>`: body is a number 0..=255, writes `M[n]`.
//!
//! One request per connection, bounded sizes and time: headers 16 KiB,
//! body 64 KiB, 5 s to send the request, 32 connections at once.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tcpethernet_adapter::PointKind;
use tcpethernet_adapter::device::lock_ref;

use crate::engine::Engine;
use crate::json;

const INDEX: &str = include_str!("../web/index.html");
const APP_JS: &str = include_str!("../web/app.js");
const APP_CSS: &str = include_str!("../web/app.css");
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 32;
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

pub type SessionCount = Arc<dyn Fn() -> usize + Send + Sync>;

pub struct Web {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Web {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Web")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

struct Ctx {
    engine: Arc<Engine>,
    token: String,
    sessions: SessionCount,
}

impl Web {
    /// Serve on `bind`.
    ///
    /// # Errors
    /// Bind failure.
    pub fn start(
        bind: SocketAddr,
        engine: Arc<Engine>,
        token: String,
        sessions: SessionCount,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(Ctx {
            engine,
            token,
            sessions,
        });
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new().name("plc-web".into()).spawn(move || {
                let active = Arc::new(AtomicUsize::new(0));
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                                let _ = respond(&stream, 503, "text/plain", b"busy\n", false);
                                continue;
                            }
                            active.fetch_add(1, Ordering::Relaxed);
                            let (ctx, done) = (Arc::clone(&ctx), Arc::clone(&active));
                            let spawned = thread::Builder::new().name("plc-http".into()).spawn(move || {
                                let _ = serve(stream, &ctx);
                                done.fetch_sub(1, Ordering::Relaxed);
                            });
                            if spawned.is_err() {
                                active.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                        Err(_) => thread::sleep(Duration::from_millis(10)),
                    }
                }
            })?
        };
        Ok(Self {
            addr,
            stop,
            thread: Some(thread),
        })
    }

    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Request {
    method: String,
    path: String,
    auth: Option<String>,
    body: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `Err` carries the status to answer with.
fn read_request(stream: &mut TcpStream) -> Result<Request, u16> {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|_| 500u16)?;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err(431);
        }
        if Instant::now() > deadline {
            return Err(408);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => return Err(400),
        }
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split(' ');
    let (method, path, version) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    if method.is_empty() || !path.starts_with('/') || !version.starts_with("HTTP/1.") {
        return Err(400);
    }
    let mut auth = None;
    let mut length = 0usize;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            return Err(400);
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "authorization" => auth = v.strip_prefix("Bearer ").map(str::to_string),
            "content-length" => length = v.parse().map_err(|_| 400u16)?,
            "transfer-encoding" => return Err(501),
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        if Instant::now() > deadline {
            return Err(408);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => return Err(400),
        }
    }
    body.truncate(length);
    Ok(Request {
        method: method.to_string(),
        path: path.split('?').next().unwrap_or("/").to_string(),
        auth,
        body,
    })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        422 => "Unprocessable Content",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn respond(mut stream: &TcpStream, code: u16, ctype: &str, body: &[u8], page: bool) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\
         X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
         Cache-Control: no-store\r\n",
        reason(code),
        body.len()
    );
    if page {
        head.push_str(
            "Content-Security-Policy: default-src 'self'; frame-ancestors 'none'; base-uri 'none'\r\n",
        );
    }
    if code == 401 {
        head.push_str("WWW-Authenticate: Bearer\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

/// Equal-time comparison so the token can't be guessed byte by byte.
fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn serve(mut stream: TcpStream, ctx: &Ctx) -> io::Result<()> {
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(code) => return respond(&stream, code, "text/plain", b"bad request\n", false),
    };
    let (code, ctype, body) = route(&req, ctx);
    respond(&stream, code, ctype, &body, ctype.starts_with("text/html"))
}

fn route(req: &Request, ctx: &Ctx) -> (u16, &'static str, Vec<u8>) {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => return (200, "text/html; charset=utf-8", INDEX.as_bytes().to_vec()),
        ("GET", "/app.js") => return (200, "text/javascript; charset=utf-8", APP_JS.as_bytes().to_vec()),
        ("GET", "/app.css") => return (200, "text/css; charset=utf-8", APP_CSS.as_bytes().to_vec()),
        _ => {}
    }
    if !req.path.starts_with("/api/") {
        return (404, "text/plain", b"not found\n".to_vec());
    }
    if !req.auth.as_deref().is_some_and(|t| token_eq(t, &ctx.token)) {
        return (
            401,
            "application/json",
            br#"{"error":"missing or wrong token"}"#.to_vec(),
        );
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/state") => (200, "application/json", state_json(ctx).into_bytes()),
        ("GET", "/api/program") => (
            200,
            "text/plain; charset=utf-8",
            ctx.engine.state().source.clone().into_bytes(),
        ),
        ("PUT", "/api/program") => {
            let Ok(src) = std::str::from_utf8(&req.body) else {
                return (
                    400,
                    "application/json",
                    br#"{"error":"program must be UTF-8"}"#.to_vec(),
                );
            };
            match ctx.engine.set_program(src) {
                Ok(()) => (200, "application/json", br#"{"ok":true}"#.to_vec()),
                Err(errs) => {
                    let mut out = String::from(r#"{"ok":false,"errors":["#);
                    for (i, e) in errs.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        let _ = write!(
                            out,
                            r#"{{"line":{},"col":{},"msg":{}}}"#,
                            e.line,
                            e.col,
                            json::string(&e.msg)
                        );
                    }
                    out.push_str("]}");
                    (422, "application/json", out.into_bytes())
                }
            }
        }
        ("PUT", p) if p.starts_with("/api/memory/") => {
            let at = p["/api/memory/".len()..].parse::<usize>();
            let val = std::str::from_utf8(&req.body)
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok());
            match (at, val) {
                (Ok(at), Some(v)) => match ctx.engine.set_memory(at, v) {
                    Ok(()) => (200, "application/json", br#"{"ok":true}"#.to_vec()),
                    Err(e) => (
                        422,
                        "application/json",
                        format!(r#"{{"error":{}}}"#, json::string(&e)).into_bytes(),
                    ),
                },
                _ => (
                    400,
                    "application/json",
                    br#"{"error":"PUT /api/memory/<n> with a body 0..=255"}"#.to_vec(),
                ),
            }
        }
        (_, "/api/state" | "/api/program") => (405, "text/plain", b"method not allowed\n".to_vec()),
        _ => (
            404,
            "application/json",
            br#"{"error":"no such endpoint"}"#.to_vec(),
        ),
    }
}

fn state_json(ctx: &Ctx) -> String {
    let e = &ctx.engine;
    let device = e.device();
    let (identity, run, i, q, conns, stats) = {
        let d = lock_ref(&device);
        let id = d.identity().clone();
        let now = Instant::now();
        let conns: Vec<String> = d
            .connections()
            .iter()
            .map(|c| {
                let kind = match c.point.kind {
                    PointKind::ExclusiveOwner => "exclusive owner",
                    PointKind::InputOnly => "input only",
                    PointKind::ListenOnly => "listen only",
                };
                format!(
                    r#"{{"kind":"{kind}","originator":"{}","o_to_t":"0x{:08X}","t_to_o":"0x{:08X}","rpi_us":{},"packets_in":{},"packets_out":{},"age_ms":{}}}"#,
                    c.originator,
                    c.o_to_t_id,
                    c.t_to_o_id,
                    c.o_to_t_rpi.as_micros(),
                    c.packets_in,
                    c.packets_out,
                    now.duration_since(c.opened).as_millis()
                )
            })
            .collect();
        (
            id,
            d.scanner_running(),
            d.assembly(e.cfg.input_assembly).unwrap_or_default().to_vec(),
            d.assembly(e.cfg.output_assembly).unwrap_or_default().to_vec(),
            conns,
            d.stats.clone(),
        )
    };
    let st = e.state();
    format!(
        concat!(
            r#"{{"identity":{{"vendor_id":{},"device_type":{},"product_code":{},"revision":"{}.{}","serial":"0x{:08X}","name":{}}},"#,
            r#""uptime_ms":{},"run":{},"input_assembly":{},"output_assembly":{},"#,
            r#""scan":{{"count":{},"last_us":{},"max_us":{},"faults":{},"overruns":{},"period_us":{}}},"#,
            r#""i":{},"q":{},"m":{},"connections":[{}],"#,
            r#""adapter":{{"sessions":{},"requests":{},"forward_opens":{},"refusals":{},"timeouts":{},"io_dropped":{}}}}}"#
        ),
        identity.vendor_id,
        identity.device_type,
        identity.product_code,
        identity.revision.0,
        identity.revision.1,
        identity.serial_number,
        json::string(&identity.product_name),
        e.uptime().as_millis(),
        run,
        e.cfg.input_assembly,
        e.cfg.output_assembly,
        st.stats.scans,
        st.stats.last_us,
        st.stats.max_us,
        st.stats.faults,
        st.stats.overruns,
        e.cfg.period.as_micros(),
        json::bytes(&i),
        json::bytes(&q),
        json::bytes(&st.memory),
        conns.join(","),
        (ctx.sessions)(),
        stats.requests,
        stats.forward_opens,
        stats.refusals,
        stats.timeouts,
        stats.io_dropped,
    )
}
