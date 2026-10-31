//! A read-only OPC UA server (`sink-opcua`): the box's signals and module
//! outputs as an address space a SCADA or MES pulls from.
//!
//! `helmsman-plc --opcua ADDR` (default `127.0.0.1:4840`, `off` to turn it
//! off). OPC UA TCP with the binary encoding, hand-written on `std::net`
//! (no new crates; see docs/ARCHITECTURE.md "OPC UA server" for the
//! dependency review that chose this):
//!
//! - **Security: `SecurityPolicy#None` only, anonymous only.** Nothing is
//!   signed or encrypted, so anyone on the path can read and alter what a
//!   client sees. It is on by default only on loopback; binding it to the
//!   plant LAN (`--opcua 0.0.0.0:4840`) is a choice to trust that LAN.
//!   `Sign` and `SignAndEncrypt` are an open question (TODO.md
//!   `opcua-security`).
//! - **Read-only.** Every variable's `AccessLevel` is `CurrentRead`; Write
//!   answers `BadNotWritable`, and no service that changes anything is
//!   offered. The server reads only the published [`feed::Snapshot`].
//! - **The scan never waits.** The scan thread offers a snapshot with a
//!   `try_lock` at most every [`PUBLISH_EVERY`] ([`feed::FeedPublisher`]);
//!   connection threads read that, and build the address space from it.
//! - **Bounded.** [`Limits`]: 16 connections (beyond: `ERR`
//!   `BadTcpServerTooBusy`), 64 KiB chunks, 1 MiB messages, one session per
//!   connection, 8 subscriptions per session, 1000 monitored items each,
//!   1000 operations per request, 10 queued publish requests. Malformed
//!   input is refused, never a panic (`tests/opcua_fuzz.rs`).
//!
//! The address space is described in [`space`].

pub mod codec;
pub mod conn;
pub mod feed;
pub mod session;
pub mod space;
pub mod status;

use std::io::{self, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub use conn::Conn;
pub use feed::{Feed, FeedPublisher, Shape, Snapshot};
pub use space::{About, Space};

/// How often the scan thread publishes a snapshot, and so the fastest
/// sampling the server offers.
pub const PUBLISH_EVERY: Duration = Duration::from_millis(100);

/// The server's limits.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Connections at once.
    pub connections: usize,
    /// Largest chunk either way.
    pub buffer: u32,
    /// Largest request, all chunks together.
    pub max_message: u32,
    /// Most chunks in one request.
    pub max_chunks: u32,
    /// Time to send Hello (and twice that to open a channel).
    pub hello_timeout: Duration,
    pub subscriptions: usize,
    /// Monitored items per subscription.
    pub items: usize,
    /// Operations (nodes to read, browse, ...) per request.
    pub operations: usize,
    pub publish_queue: usize,
    pub continuation_points: usize,
    /// References per browse result (more go behind a continuation point).
    pub references: usize,
    /// Fastest publishing interval.
    pub min_interval: Duration,
    /// Longest a write to a client may stall before it is dropped.
    pub write_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            connections: 16,
            buffer: 65_536,
            max_message: 1 << 20,
            max_chunks: 64,
            hello_timeout: Duration::from_secs(5),
            subscriptions: 8,
            items: 1000,
            operations: 1000,
            publish_queue: 10,
            continuation_points: 8,
            references: 1000,
            min_interval: PUBLISH_EVERY,
            write_timeout: Duration::from_secs(2),
        }
    }
}

/// The wall clock, Unix ms.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The system clock as a [`Clock`].
#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0)
    })
}

/// What every connection shares.
pub struct Ctx {
    pub feed: Arc<Feed>,
    pub about: About,
    /// `opc.tcp://ADDR/`, for endpoint descriptions when the client's own
    /// URL is unusable.
    pub endpoint: String,
    pub limits: Limits,
    clock: Clock,
    space: Mutex<Option<(Arc<Shape>, Arc<Space>)>>,
    next: AtomicU32,
    rng: ring::rand::SystemRandom,
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("endpoint", &self.endpoint)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Ctx {
    #[must_use]
    pub fn new(feed: Arc<Feed>, endpoint: String, limits: Limits, clock: Clock) -> Self {
        let started_ms = clock();
        Self {
            feed,
            about: About {
                started_ms,
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            endpoint,
            limits,
            clock,
            space: Mutex::new(None),
            next: AtomicU32::new(1),
            rng: ring::rand::SystemRandom::new(),
        }
    }

    #[must_use]
    pub fn now_ms(&self) -> i64 {
        (self.clock)()
    }

    /// A fresh id (channels, sessions, subscriptions), never 0.
    pub fn next_id(&self) -> u32 {
        loop {
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                return id;
            }
        }
    }

    /// `n` random bytes (session tokens and nonces).
    #[must_use]
    pub fn random(&self, n: usize) -> Vec<u8> {
        use ring::rand::SecureRandom as _;
        let mut b = vec![0; n];
        // The system source does not fail on the platforms the box runs
        // on; were it to, a token would still be unique per process.
        if self.rng.fill(&mut b).is_err() {
            let id = self.next_id().to_le_bytes();
            for (i, x) in b.iter_mut().enumerate() {
                *x = id[i % 4];
            }
        }
        b
    }

    /// The newest snapshot and the address space for its shape (rebuilt
    /// here, on a server thread, when the shape changed).
    pub fn view(&self) -> (Arc<Snapshot>, Arc<Space>) {
        let snap = self.feed.snapshot();
        let mut cache = self.space.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((shape, space)) = &*cache
            && Arc::ptr_eq(shape, &snap.shape)
        {
            return (snap, Arc::clone(space));
        }
        let space = Arc::new(Space::build(&snap.shape));
        *cache = Some((Arc::clone(&snap.shape), Arc::clone(&space)));
        (snap, space)
    }
}

/// How the server is served.
#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub limits: Limits,
    pub clock: Clock,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Config {
    #[must_use]
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            limits: Limits::default(),
            clock: system_clock(),
        }
    }
}

/// The running server; dropping it stops accepting.
#[derive(Debug)]
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Binds and serves `feed`.
    ///
    /// # Errors
    /// The bind fails.
    pub fn start(cfg: Config, feed: Arc<Feed>) -> io::Result<Self> {
        let listener = TcpListener::bind(cfg.bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let ctx = Arc::new(Ctx::new(
            feed,
            format!("opc.tcp://{addr}/"),
            cfg.limits,
            cfg.clock,
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("opcua".into())
            .spawn(move || accept(&listener, &ctx, &s))?;
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

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn accept(listener: &TcpListener, ctx: &Arc<Ctx>, stop: &Arc<AtomicBool>) {
    let open = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Acquire) {
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if stream.set_nonblocking(false).is_err() {
            continue;
        }
        if open.load(Ordering::Acquire) >= ctx.limits.connections {
            let mut w = codec::Writer::new();
            w.u32(status::BAD_TCP_SERVER_TOO_BUSY)
                .string(Some("too many connections"));
            let mut msg = b"ERRF".to_vec();
            msg.extend_from_slice(&u32::try_from(w.buf.len() + 8).unwrap_or(0).to_le_bytes());
            msg.extend_from_slice(&w.buf);
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let _ = stream.write_all(&msg);
            continue;
        }
        open.fetch_add(1, Ordering::AcqRel);
        let slot = Slot(Arc::clone(&open));
        let (ctx, stop) = (Arc::clone(ctx), Arc::clone(stop));
        let _ = thread::Builder::new().name("opcua-conn".into()).spawn(move || {
            let _slot = slot;
            serve(stream, ctx, &stop);
        });
    }
}

/// One connection: socket bytes into the [`Conn`], its output back out,
/// ticking it between reads.
fn serve(mut stream: TcpStream, ctx: Arc<Ctx>, stop: &AtomicBool) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
    let _ = stream.set_write_timeout(Some(ctx.limits.write_timeout));
    let mut conn = Conn::new(ctx, Instant::now());
    let mut buf = vec![0; 16 * 1024];
    while !stop.load(Ordering::Acquire) {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => conn.feed(&buf[..n], Instant::now()),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        conn.tick(Instant::now());
        let out = conn.take_output();
        if !out.is_empty() && stream.write_all(&out).is_err() {
            break;
        }
        if conn.is_closed() {
            break;
        }
    }
}
