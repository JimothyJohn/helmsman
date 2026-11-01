//! The box API: one local HTTP/1.1 + JSON surface for the canvas, the
//! agent's tools and MCP ([`mcp`]). Testable without a UI.
//!
//! ```text
//! GET  /api                               this list
//! GET  /api/signals                       every signal: type, unit, dir, binding, writer, value, quality
//! GET  /api/signals/{name}
//! GET  /api/modules                       every module: schedule, windows, status, stats
//! GET  /api/modules/{name}                + ports, source, manifest
//! GET  /api/modules/{name}/trace          asks for a traced scan; the newest one (200), or 202 until one lands
//! GET  /api/modules/{name}/trace/events   event stream of traced scans
//! GET  /api/traces/events                 event stream of every module's traced scans (the Monitor's one stream)
//! GET  /api/outputs                       output signals' commands and safe values, the %Q image
//! GET  /api/devices                       device links: up, down (why), unknown
//! GET  /api/proposals                     drafts awaiting approval
//! GET  /api/proposals/{id}
//! POST /api/proposals                     token: a module draft, a param change, or a new io_map / calendar file, checked, stored as a draft
//! POST /api/proposals/check               token: check a draft as POST /api/proposals would; nothing stored
//! POST /api/proposals/replay              token: sim replay of a draft {draft, minutes}; nothing stored, not evidence
//! POST /api/proposals/{id}/replay         token: sim replay of the draft against what runs; evidence for apply
//! POST /api/login                         {user, password} (accounts) or {role, pin} (none): a session, as a token or a cookie (see [`login`])
//! POST /api/logout                        session: ends it (and clears its cookie)
//! GET  /api/session                       who the session sent is (cookie or bearer), or 401
//! POST /api/proposals/{id}/apply          session + {replay?}, or token + {role, pin, replay?} without accounts: apply through the approval policy
//! GET  /api/audit?after=N&limit=M         every mutating call, append-only, naming snapshot ids
//! GET  /api/snapshots                     snapshot history (with --store), head, the running change set's id
//! GET  /api/snapshots/head
//! GET  /api/io_map                        the running I/O map file (to draft a new one from; card `dropin-pid`)
//! GET  /api/snapshots/{id}                a snapshot's files
//! GET  /api/snapshots/{a}/diff/{b}
//! GET  /api/snapshots/{id}/bundle         that snapshot as an unsigned bundle file (see [`bundle`])
//! POST /api/bundles                       token: a bundle file, verified, bound by name, stored as a draft
//! POST /api/snapshots/{id}/replay         token: sim replay of a rollback to it
//! POST /api/snapshots/{id}/rollback       as apply: roll back through the approval policy
//! GET  /api/replays/{n}                   a replay kept as evidence
//! GET  /api/policy                        the policy this box enforces
//! GET  /api/events                        event stream of live values and module status
//! POST /api/chat                          token: one message to the agent; its progress as an event stream
//! POST /api/explain                       token: why a variable is in its state: the box's own answer, then the agent's (read-only tools)
//! GET  /api/chat/status                   the agent: online, offline since T (why), unknown; offline chat reads param edits by rule
//! GET  /api/site                          this box and the boxes it heard on the LAN (with --site NAME; see [`crate::site`])
//! POST /api/dropins?name=N&kind=K         token: a document, a photo or a PDF for the agent, stored in memory (see [`dropin`], [`photo`], [`pdf`])
//! GET  /api/dropins                       the drop-ins held
//! GET  /api/dropins/{id}?chunk=K          one chunk of a drop-in's text
//! GET  /api/dropins/{id}/plc?view=V...    a PLC drop-in's inventory (overview, tags, routine, names)
//! POST /api/devices/{name}/browse         token: an observed controller's tag list, read-only
//! GET  /api/projects ...                  the editor's projects; PUT, DELETE, deploy and uploads need an engineer session (see [`projects`])
//! GET  /api/running                       the project deployed last, and whether it is what runs
//! GET  /api/forces                        forced inputs; POST and DELETE need an engineer session (see [`forces`])
//! GET  /api/drivers ...                   the box's WASM driver files; list, PUT and DELETE need an engineer session (see [`drivers`])
//! POST /mcp                               token: MCP Streamable HTTP, the agent's tools for an engineer's own client (see [`mcp`])
//! GET  /, /{file}                          the canvas (with --ui DIR; see [`ui`])
//! ```
//!
//! **What it cannot do.** There is no route that writes a signal, an
//! output, a param or a module, with or without the token. Proposals are
//! stored drafts; the only way one runs is `apply` (and the only way back
//! is `rollback`), both through [`approve`]: tier, policy, role, PIN and
//! sim evidence, checked by the host ([`crate::policy`]). A project
//! deploy ([`projects`]) is an apply of the project's build, through the
//! same path. The token alone
//! (what the agent holds) can draft and replay, never apply. Limits are
//! enforced here, by the host, never by whatever is calling.
//!
//! **Transport.** With [`Api::start_secure`] the port serves HTTPS
//! ([`tls`]: a self-signed certificate whose fingerprint the person
//! checks), and plain HTTP only to this host; from elsewhere plain HTTP
//! is redirected, never served ([`to_https`]).
//!
//! **Auth.** Reads (`GET`) are open to whoever can reach the socket (the
//! default bind is loopback; the LAN only when asked). Every other method
//! needs `Authorization: Bearer <token>` ([`token`]), or a login session
//! from `POST /api/login` ([`login`], which takes a session wherever it
//! takes the token, except `/mcp`): as a bearer, or as the browser's
//! `HttpOnly` cookie with a same-origin proof ([`login::same_origin`];
//! an `Authorization` header, when sent, is the credential and the cookie
//! is not read). Each call is written to the audit log whatever its
//! outcome, naming the session's person; without a credential it is
//! refused with 401 before any routing, and a cookie without the proof
//! with 403. A token in the query string is refused
//! outright (400), on any request, so it never lands in a log or a
//! browser history.
//!
//! **The scan never waits on the API.** The scheduler publishes a
//! [`live::View`] with a `try_lock` and the API reads only that, the
//! [`catalog::Catalog`] fixed at startup, and the trace taps (whose
//! scheduler side is a `try_lock` too). No API thread ever takes the
//! signal table's lock or touches the runtime.
//!
//! **Bounded.** Thread per connection, at most [`Limits::workers`] at
//! once (503 beyond), event streams at most [`Limits::streams`] of those.
//! One request per connection. The whole request must arrive within
//! [`Limits::deadline`]; a stalled writer is dropped after
//! [`Limits::write_timeout`], and an event stream ends after
//! [`Limits::stream_life`] (the browser reconnects). Sizes are capped
//! ([`Limits`]).

pub mod approve;
pub mod bundle;
pub mod catalog;
pub mod chat;
pub mod drivers;
pub mod dropin;
pub mod explain;
pub mod forces;
pub mod http;
pub mod live;
pub mod login;
pub mod mcp;
pub mod pdf;
pub mod photo;
pub mod projects;
pub mod proposal;
pub mod render;
pub mod sim;
pub mod store;
pub mod swap;
#[cfg(feature = "https")]
pub mod tls;
pub mod token;
pub mod trend;
pub mod ui;

use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as Json, json};

use crate::bundle::TrustStore;
use crate::policy::Role;
use crate::policy::account::{Logins, Mode, SessionRefused};
use crate::store::{self as snapshots, ChangeSet, SnapshotId, Store};

pub use approve::{Approval, Replays};
pub use catalog::{Catalog, Devices, ModuleInfo, SignalInfo};
pub use http::{Conn, Limits, Request, Response};
pub use live::{Live, ModuleLive, Publisher, View};
pub use store::{Audit, AuditEntry, MemAudit, MemProposals, Proposals};
pub use swap::{Plant, SwapHandle, Swapper};

/// How often the scheduler publishes a view while the API runs, and so
/// the longest it sleeps between ticks.
pub const PUBLISH_EVERY: Duration = Duration::from_millis(100);

/// Event streams send a comment line this often when nothing changed, so
/// a dead peer is noticed.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// Longest text logged from a request (a path), in characters.
const LOG_MAX: usize = 120;

/// Where log lines go (the journal, via stderr, in the binary).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// How the API is served.
#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    /// What `Authorization: Bearer` must carry for any mutating call.
    pub token: String,
    pub limits: Limits,
    /// How often an event stream checks for something new to send.
    pub stream_every: Duration,
    /// The default `%M` size, for checking module drafts.
    pub memory: usize,
    pub log: Log,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

/// What the API serves from.
pub struct Shared {
    pub catalog: Catalog,
    pub live: Arc<Live>,
    pub devices: Arc<Devices>,
    pub audit: Box<dyn Audit>,
    pub proposals: Box<dyn Proposals>,
    /// What the box runs, as a change set: the store's head, or the
    /// module directories, map and calendar it was started from. `None`
    /// when bare programs or `--module` specs are part of it (they are not
    /// module directories, so no snapshot can hold them).
    pub running: Option<ChangeSet>,
    /// The snapshot store (`--store`): read here, and committed to by an
    /// approved apply or rollback. Its calls block on `SQLite`, so only API
    /// worker threads make them.
    pub snapshots: Option<Mutex<Store>>,
    /// The approval path: policy, PINs, the box's shape for replays, and
    /// the scan thread's swap channel. `None`: apply and rollback answer
    /// 501.
    pub approval: Option<Approval>,
    /// The relay and limits for `POST /api/chat`; `None`: it answers 503.
    pub chat: Option<chat::Setup>,
}

/// What a box may serve besides its state; none of it changes while the
/// box runs.
#[derive(Default)]
pub struct Extras {
    /// The canvas, at `/`.
    pub ui: Option<ui::Ui>,
    /// Publishers whose bundles import (`POST /api/bundles`); `None`:
    /// import answers 501.
    pub trust: Option<TrustStore>,
}

/// What a box running the editor serves besides [`Extras`] (a struct of
/// its own so that adding to it breaks no caller of [`Api::start_with`]).
#[derive(Debug, Default)]
pub struct More {
    /// The projects folder ([`crate::project`], `--projects`): the
    /// project routes ([`projects`]).
    pub projects: Option<crate::project::Projects>,
    /// Forced inputs ([`crate::force`]): the scan thread applies them
    /// through a [`crate::force::Reader`] on the same set.
    pub forces: Option<Arc<crate::force::Forces>>,
    /// The box's WASM driver folder (`<state-dir>/drivers/`,
    /// [`crate::driver_dir`]): where a map's `wasm:NAME` is found after
    /// the project's, and the driver routes ([`drivers`]). `None`: neither.
    pub drivers: Option<crate::driver_dir::DriverDir>,
    /// The WASM drivers the running map was loaded with (its
    /// [`crate::layout::Layout::drivers`]), which a reload compares the
    /// files it finds with.
    pub running_drivers: crate::drivers::Drivers,
}

/// The box's HTTPS: what it serves with, and the certificate's
/// fingerprint, shown at `GET /api/policy` (`transport`).
#[derive(Clone)]
pub struct Tls {
    #[cfg(feature = "https")]
    config: Arc<rustls::ServerConfig>,
    fingerprint: String,
}

impl Tls {
    /// Serving `id` ([`tls::load_or_create`]).
    ///
    /// # Errors
    /// rustls refuses its key or certificate.
    #[cfg(feature = "https")]
    pub fn new(id: &tls::Identity) -> Result<Self, String> {
        Ok(Self {
            config: id.server_config()?,
            fingerprint: id.fingerprint(),
        })
    }

    /// The certificate's SHA-256 fingerprint ([`tls::fingerprint`]).
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// A traced scan's number and its JSON.
type Rendered = (u64, Arc<Json>);

struct Ctx {
    cfg: Config,
    /// Its `catalog`, `running` and `approval` are moved out to the fields
    /// below at start.
    box_: Shared,
    /// What the API describes; replaced when an apply lands.
    catalog: RwLock<Arc<Catalog>>,
    /// The change set the box runs; replaced when an apply lands.
    running: Mutex<Option<ChangeSet>>,
    approval: Option<Approval>,
    replays: Replays,
    /// The table and recording the box runs, for replays; replaced by a
    /// reload. `None` without an approval path.
    shape: Mutex<Option<approve::Shape>>,
    /// Held for the whole of an apply or rollback.
    applying: Mutex<()>,
    /// Per module name: the newest traced scan, rendered, and its scan
    /// number.
    traces: Mutex<BTreeMap<String, Rendered>>,
    /// Per module name: the newest traced scan itself (explain mode
    /// explains from it).
    samples: Mutex<BTreeMap<String, Arc<crate::Sample>>>,
    stop: Arc<AtomicBool>,
    streams: AtomicUsize,
    chat: Option<chat::Chat>,
    ui: Option<ui::Ui>,
    /// Publishers whose bundles import; `None`: import answers 501.
    trust: Option<TrustStore>,
    /// Drop-in documents (see [`dropin`]).
    dropins: Mutex<dropin::Store>,
    /// Held while a device browse runs (one at a time).
    browsing: Mutex<()>,
    /// The site this box announces itself in (`--site`), and what it heard.
    site: Option<crate::site::Handle>,
    /// The MCP route's drafts (see [`mcp`]).
    mcp: Mutex<mcp::Workspace>,
    /// The editor's projects (see [`projects`]); `None`: those routes
    /// answer 404.
    projects: Option<crate::project::Projects>,
    /// Forced inputs (see [`forces`]); `None`: those routes answer 404.
    forces: Option<Arc<crate::force::Forces>>,
    /// The box's driver folder (see [`drivers`]); `None`: those routes
    /// answer 404, and only a project's drivers load.
    driver_dir: Option<crate::driver_dir::DriverDir>,
    /// The WASM drivers the running map was loaded with; replaced by a
    /// reload.
    running_drivers: Mutex<crate::drivers::Drivers>,
    /// Pinned signals' history ([`trend`]).
    trends: Mutex<trend::Trends>,
    /// The box's HTTPS ([`Api::start_secure`]); `None`: plain HTTP only.
    tls: Option<Tls>,
}

impl Ctx {
    fn catalog(&self) -> Arc<Catalog> {
        Arc::clone(&self.catalog.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn trends(&self) -> MutexGuard<'_, trend::Trends> {
        self.trends.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn dropins(&self) -> MutexGuard<'_, dropin::Store> {
        self.dropins.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn running(&self) -> Option<ChangeSet> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_running(&self, cs: ChangeSet, catalog: Catalog) {
        *self.running.lock().unwrap_or_else(PoisonError::into_inner) = Some(cs);
        *self.catalog.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(catalog);
        self.traces.lock().unwrap_or_else(PoisonError::into_inner).clear();
        self.samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn shape(&self) -> approve::Shape {
        self.shape
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| approve::Shape {
                table: crate::SignalTable::default(),
                record: None,
            })
    }

    fn set_shape(&self, shape: approve::Shape) {
        *self.shape.lock().unwrap_or_else(PoisonError::into_inner) = Some(shape);
    }

    /// Where a map's `wasm:NAME` is found (`crate::driver_dir`): the
    /// folder of the project being deployed, else of the one recorded as
    /// running, then the box's driver folder.
    fn sources(&self, deploying: Option<&str>) -> crate::driver_dir::Sources {
        let project = self.projects.as_ref().and_then(|p| {
            let name = deploying
                .map(str::to_string)
                .or_else(|| p.running().map(|r| r.name))?;
            p.folder(&name).map(|dir| (name, dir))
        });
        crate::driver_dir::Sources {
            project,
            boxed: self.driver_dir.as_ref().map(|d| d.root().to_path_buf()),
        }
    }

    fn running_drivers(&self) -> crate::drivers::Drivers {
        self.running_drivers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_running_drivers(&self, drivers: crate::drivers::Drivers) {
        *self
            .running_drivers
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = drivers;
    }

    /// A reload started and ended device links: the device list follows.
    fn links_changed(&self, links: &crate::link::Finished) {
        for name in &links.removed {
            self.box_.devices.remove(name);
        }
        for name in &links.added {
            self.box_.devices.add(name);
        }
    }
}

/// What an audit entry names beside the call itself.
#[derive(Clone, Copy, Debug, Default)]
struct Note {
    snapshot: Option<SnapshotId>,
    base: Option<SnapshotId>,
    role: Option<Role>,
}

impl Note {
    const fn target(snapshot: SnapshotId, base: Option<SnapshotId>) -> Self {
        Self {
            snapshot: Some(snapshot),
            base,
            role: None,
        }
    }

    const fn by(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }
}

/// A running API; stops (and joins its acceptor) when dropped. Open
/// connections end on their own deadlines; event streams see the stop
/// within one [`Config::stream_every`].
pub struct Api {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The chat's relay watcher ([`chat::Chat::watch`]).
    watcher: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// Decrements a counter when dropped, so a panicking worker still frees
/// its slot.
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Takes one of `cap` slots, if one is free.
fn take(n: &AtomicUsize, cap: usize) -> Option<Slot<'_>> {
    let mut v = n.load(Ordering::Acquire);
    while v < cap {
        match n.compare_exchange_weak(v, v + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(Slot(n)),
            Err(now) => v = now,
        }
    }
    None
}

impl Api {
    /// Binds `cfg.bind` and serves `shared` on a thread of its own.
    ///
    /// # Errors
    /// The bind fails.
    pub fn start(cfg: Config, shared: Shared) -> io::Result<Self> {
        Self::start_with_ui(cfg, shared, None)
    }

    /// [`Api::start`], also serving the canvas from `ui` at `/`.
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_with_ui(cfg: Config, shared: Shared, ui: Option<ui::Ui>) -> io::Result<Self> {
        Self::start_with_trust(cfg, shared, ui, None)
    }

    /// [`Api::start_with_ui`], also importing bundles signed by a
    /// publisher in `trust` (`POST /api/bundles`).
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_with_trust(
        cfg: Config,
        shared: Shared,
        ui: Option<ui::Ui>,
        trust: Option<TrustStore>,
    ) -> io::Result<Self> {
        Self::start_with(cfg, shared, Extras { ui, trust })
    }

    /// [`Api::start`], with what a box may serve besides its state.
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_with(cfg: Config, shared: Shared, extras: Extras) -> io::Result<Self> {
        Self::start_in_site(cfg, shared, extras, None)
    }

    /// [`Api::start_with`], also serving `GET /api/site` from a running
    /// [`crate::site::Site`]. With a site, `/api/site` may be read from
    /// other origins (the site view on another box's canvas) and the
    /// canvas may fetch from other boxes (see [`site_json`]).
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_in_site(
        cfg: Config,
        shared: Shared,
        extras: Extras,
        site: Option<crate::site::Handle>,
    ) -> io::Result<Self> {
        Self::start_box(cfg, shared, extras, site, More::default())
    }

    /// [`Api::start_in_site`], with what a box running the editor serves
    /// besides ([`More`]).
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_box(
        cfg: Config,
        shared: Shared,
        extras: Extras,
        site: Option<crate::site::Handle>,
        more: More,
    ) -> io::Result<Self> {
        Self::start_secure(cfg, shared, extras, site, more, None)
    }

    /// [`Api::start_box`], serving HTTPS with `tls` ([`Tls`]): TLS on the
    /// API's port, and plain HTTP there only from this host (from anywhere
    /// else it is sent to `https://`, never served). `None`: plain HTTP
    /// for everyone, as [`Api::start_box`].
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_secure(
        cfg: Config,
        mut shared: Shared,
        extras: Extras,
        site: Option<crate::site::Handle>,
        more: More,
        tls: Option<Tls>,
    ) -> io::Result<Self> {
        let Extras { ui, trust } = extras;
        let More {
            projects,
            forces,
            drivers: driver_dir,
            running_drivers,
        } = more;
        let listener = TcpListener::bind(cfg.bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let chat = shared
            .chat
            .take()
            .map(|setup| chat::Chat::new(setup, addr, cfg.token.clone()));
        let ctx = Arc::new(Ctx {
            chat,
            catalog: RwLock::new(Arc::new(std::mem::take(&mut shared.catalog))),
            running: Mutex::new(shared.running.take()),
            shape: Mutex::new(shared.approval.as_ref().map(|a| approve::Shape {
                table: a.plant.table.clone(),
                record: a.plant.record.clone(),
            })),
            approval: shared.approval.take(),
            replays: Replays::new(1000),
            applying: Mutex::new(()),
            traces: Mutex::new(BTreeMap::new()),
            samples: Mutex::new(BTreeMap::new()),
            box_: shared,
            cfg,
            stop: Arc::clone(&stop),
            streams: AtomicUsize::new(0),
            ui,
            trust,
            dropins: Mutex::new(dropin::Store::default()),
            browsing: Mutex::new(()),
            site,
            mcp: Mutex::new(mcp::Workspace::default()),
            projects,
            forces,
            driver_dir,
            running_drivers: Mutex::new(running_drivers),
            trends: Mutex::new(trend::Trends::default()),
            tls,
        });
        let watcher = ctx.chat.as_ref().map(|c| c.watch(Arc::clone(&stop)));
        let thread = thread::Builder::new()
            .name("api".into())
            .spawn(move || accept(&listener, &ctx))?;
        Ok(Self {
            addr,
            stop,
            thread: Some(thread),
            watcher,
        })
    }

    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.watcher.take() {
            let _ = t.join();
        }
    }
}

fn accept(listener: &TcpListener, ctx: &Arc<Ctx>) {
    let workers = Arc::new(AtomicUsize::new(0));
    let mut swept = Instant::now();
    let mut sampled = Instant::now();
    while !ctx.stop.load(Ordering::Acquire) {
        if swept.elapsed() >= Duration::from_secs(1) {
            forces::sweep(ctx);
            swept = Instant::now();
        }
        if sampled.elapsed() >= ctx.cfg.stream_every {
            trend::sample(ctx);
            sampled = Instant::now();
        }
        let (stream, peer) = match listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let accepted = Instant::now();
        // Accepted sockets inherit non-blocking on BSDs; workers use timeouts.
        if stream.set_nonblocking(false).is_err() {
            continue;
        }
        let limits = ctx.cfg.limits;
        if workers.load(Ordering::Acquire) >= limits.workers {
            // Over HTTPS the client may be mid-handshake: a plain 503 would
            // be noise to it, and a handshake here would stall accepting.
            if !ctx.serves_tls() {
                let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                let _ = http::respond(&mut Conn::Plain(stream), &Response::error(503, "busy; try again"));
            }
            continue;
        }
        workers.fetch_add(1, Ordering::AcqRel);
        let (ctx, n) = (Arc::clone(ctx), Arc::clone(&workers));
        let spawned = thread::Builder::new().name("api-conn".into()).spawn(move || {
            let _slot = Slot(&n);
            serve(stream, peer, &ctx, accepted);
        });
        if spawned.is_err() {
            workers.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// `s` safe for a one-line log: control characters escaped, cut short.
#[must_use]
pub fn clean(s: &str) -> String {
    let mut out: String = s.chars().take(LOG_MAX).flat_map(char::escape_debug).collect();
    if s.chars().count() > LOG_MAX {
        out.push_str("...");
    }
    out
}

fn wall() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a request turns into.
enum Reply {
    Once(Response),
    /// The values stream.
    Values,
    /// A module's trace stream.
    Traces(usize),
    /// Every module's traces, one stream.
    AllTraces,
    /// One chat message, its conversation taken out.
    Chat(chat::Taken),
    /// One explanation for the agent.
    Explain(explain::Job),
    /// A file of the UI.
    File(ui::Asset),
}

/// How a connection is served, once its first byte is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// TLS (the box's HTTPS), or plain HTTP on a box without it.
    Served,
    /// Plain HTTP to a box that serves HTTPS, from another host: told
    /// where the HTTPS is, never served.
    Redirected,
}

impl Ctx {
    /// Whether the box serves HTTPS.
    fn serves_tls(&self) -> bool {
        self.tls.is_some()
    }

    /// Its certificate's fingerprint, when it does.
    fn fingerprint(&self) -> Option<&str> {
        self.tls.as_ref().map(Tls::fingerprint)
    }
}

/// Whether `peer` is this host: loopback, or the address it reached us
/// on (the box's own agent calls its API that way when bound to one IP).
#[cfg(feature = "https")]
fn this_host(stream: &TcpStream, peer: SocketAddr) -> bool {
    peer.ip().is_loopback() || stream.local_addr().is_ok_and(|me| me.ip() == peer.ip())
}

/// The connection as the box serves it: on a box with HTTPS, a TLS
/// handshake (`0x16`, a TLS record carrying a handshake, RFC 8446 5.1)
/// is served as TLS; plain HTTP from this host is served as ever (local
/// tools, scripts, an SSH tunnel), and from anywhere else only redirected.
/// `None`: the client sent nothing before the deadline, or went away.
fn open(stream: TcpStream, peer: SocketAddr, ctx: &Ctx, deadline: Instant) -> Option<(Conn, Channel)> {
    #[cfg(feature = "https")]
    if let Some(tls) = &ctx.tls {
        let mut first = [0u8; 1];
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())?;
            stream
                .set_read_timeout(Some(left.min(Duration::from_millis(250))))
                .ok()?;
            match stream.peek(&mut first) {
                Ok(0) => return None,
                Ok(_) => break,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return None,
            }
        }
        if first[0] == 0x16 {
            let conn = rustls::ServerConnection::new(Arc::clone(&tls.config)).ok()?;
            let tls = Conn::Tls(Box::new(rustls::StreamOwned::new(conn, stream)));
            return Some((tls, Channel::Served));
        }
        let channel = if this_host(&stream, peer) {
            Channel::Served
        } else {
            Channel::Redirected
        };
        return Some((Conn::Plain(stream), channel));
    }
    let _ = (peer, ctx, deadline);
    Some((Conn::Plain(stream), Channel::Served))
}

/// What plain HTTP from another host gets from a box serving HTTPS: a
/// `GET` or `HEAD` is sent to the same path over `https://` on the Host
/// it asked for (when that is a plain host name or address), anything
/// else is refused, so nothing is served, and nothing changed, in clear.
fn to_https(req: &Request) -> Response {
    let mut hosts = req.header("host");
    let host = match (hosts.next(), hosts.next()) {
        (Some(h), None)
            if !h.is_empty()
                && h.len() <= 260
                && h.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')) =>
        {
            Some(h)
        }
        _ => None,
    };
    let why = "this box serves its API and page over HTTPS: use https://";
    match host {
        Some(h) if req.method == "GET" || req.method == "HEAD" => {
            Response::error(301, why).with("Location", format!("https://{h}{}", req.path))
        }
        _ => Response::error(403, why),
    }
}

fn serve(stream: TcpStream, peer: SocketAddr, ctx: &Ctx, accepted: Instant) {
    let limits = ctx.cfg.limits;
    if stream.set_write_timeout(Some(limits.write_timeout)).is_err() {
        return;
    }
    let Some((mut stream, channel)) = open(stream, peer, ctx, accepted + limits.deadline) else {
        return;
    };
    let reply = match http::read_request(&mut stream, &limits, accepted) {
        Ok(req) if channel == Channel::Redirected => Reply::Once(to_https(&req)),
        Ok(mut req) => {
            req.tls = stream.is_tls();
            handle(&mut req, ctx)
        }
        // A TLS client that never finished its handshake gets nothing.
        Err(_) if stream.handshaking() => return,
        Err(http::ReadError::Status(code, why)) => Reply::Once(Response::error(code, why)),
        Err(http::ReadError::Closed) => return,
    };
    let _ = stream.set_read_timeout(None);
    match reply {
        Reply::Once(r) => {
            let _ = http::respond(&mut stream, &r);
        }
        Reply::Explain(job) => {
            if let Some(chat) = &ctx.chat {
                explain::serve(chat, &mut stream, job);
            }
        }
        Reply::Chat(taken) => {
            if let Some(chat) = &ctx.chat {
                chat.serve(&mut stream, taken, &ctx.catalog());
            }
        }
        Reply::File(asset) => {
            let _ = ui::send(&mut stream, asset);
        }
        Reply::Values | Reply::Traces(_) | Reply::AllTraces => {
            let Some(_slot) = take(&ctx.streams, limits.streams) else {
                let _ = http::respond(&mut stream, &Response::error(503, "too many event streams"));
                return;
            };
            let _ = match reply {
                Reply::Traces(k) => trace_stream(&mut stream, ctx, k),
                Reply::AllTraces => all_traces_stream(&mut stream, ctx),
                Reply::Values => value_stream(&mut stream, ctx),
                Reply::Once(_) | Reply::Chat(_) | Reply::Explain(_) | Reply::File(_) => Ok(()),
            };
        }
    }
}

/// Query keys that would carry a credential.
fn secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "token",
        "access_token",
        "auth",
        "authorization",
        "bearer",
        "key",
        "api_key",
        "password",
        "pin",
        "session",
    ]
    .contains(&k.as_str())
}

/// `GET /api/policy`: the policy, and what approvals take on this box now
/// (`accounts`: a login session; `pins`: a role PIN; `unavailable`: an
/// accounts file that cannot be used), and `transport`: whether the box
/// serves HTTPS and its certificate's SHA-256 fingerprint.
fn policy_json(a: &Approval, ctx: &Ctx) -> Json {
    let mut j = approve::policy_json(&a.policy);
    // How the page reached the box: HTTPS, and the certificate's fingerprint
    // for a person to compare with what the browser shows (`box-https`).
    j["transport"] = json!({
        "https": ctx.serves_tls(),
        "fingerprint": ctx.fingerprint(),
    });
    j["credentials"] = json!(match a.gate.logins().map(Logins::mode) {
        None | Some(Mode::Pins) => "pins",
        Some(Mode::Accounts(_)) => "accounts",
        Some(Mode::Broken(_)) => "unavailable",
    });
    j
}

/// The login sessions, when the box has an approval path with accounts
/// wired in.
fn logins(ctx: &Ctx) -> Option<&crate::policy::account::Logins> {
    ctx.approval.as_ref().and_then(|a| a.gate.logins())
}

fn handle(req: &mut Request, ctx: &Ctx) -> Reply {
    if req.query.iter().any(|(k, _)| secret(k)) {
        return Reply::Once(Response::error(
            400,
            "credentials go in the Authorization header, never the URL",
        ));
    }
    let mcp_route = req.segments.len() == 1 && req.segments[0] == "mcp";
    if mcp_route && req.method != "POST" {
        // Streamable HTTP 2026-07-28: no GET stream, no sessions to DELETE.
        return Reply::Once(
            mcp::reject(
                405,
                &mcp::error(
                    &Json::Null,
                    mcp::INVALID_REQUEST,
                    "the MCP endpoint takes POST only",
                ),
            )
            .with("Allow", "POST".into()),
        );
    }
    if req.method == "GET" {
        return get(req, ctx);
    }
    if req.method == "POST" && req.segments == ["api", "login"] {
        // Its own credential: no token, no session.
        return Reply::Once(login::login(req, ctx));
    }
    // An Authorization header is the credential whenever one is sent: the
    // cookie is then not read (see [`login`]). MCP never takes a cookie.
    let authed = if req.authorization || mcp_route || req.cookies.is_empty() {
        by_bearer(req, ctx, mcp_route)
    } else {
        login::by_cookie(req, ctx)
    };
    if let Err(r) = authed {
        return Reply::Once(r);
    }
    if req.method == "POST"
        && req.segments.len() == 2
        && req.segments[0] == "api"
        && req.segments[1] == "chat"
    {
        return start_chat(req, ctx);
    }
    if req.method == "POST"
        && req.segments.len() == 2
        && req.segments[0] == "api"
        && req.segments[1] == "explain"
    {
        return start_explain(req, ctx);
    }
    if mcp_route {
        return Reply::Once(mcp::post(req, ctx));
    }
    Reply::Once(mutate(req, ctx))
}

/// Authenticates `req` by its `Authorization: Bearer`: the box token, or
/// a login session (not on `/mcp`, which takes the box token only: its
/// calls are the box's, never a person's, and are audited with no user).
fn by_bearer(req: &mut Request, ctx: &Ctx, mcp_route: bool) -> Result<(), Response> {
    let bearer = req.bearer.as_deref();
    let box_token = bearer.is_some_and(|t| token::eq(t, &ctx.cfg.token));
    let session = match (box_token, bearer, logins(ctx)) {
        (false, Some(t), Some(l)) => Some(l.session(t)),
        _ => None,
    };
    match session {
        Some(Ok(who)) if !mcp_route => {
            req.session = Some(who);
            Ok(())
        }
        _ if box_token => Ok(()),
        refused => {
            (ctx.cfg.log)(&format!(
                "api: refused {} {} (no or wrong token)",
                clean(&req.method),
                clean(&req.path)
            ));
            let why = match refused {
                Some(Err(SessionRefused::Expired)) => "session expired; log in again",
                Some(Err(SessionRefused::Revoked)) => {
                    "session ended: the account was disabled or changed; log in again"
                }
                _ if req.authorization => "wrong token",
                _ => "this call needs Authorization: Bearer <token>, or a login session",
            };
            Err(Response::error(401, why).with("WWW-Authenticate", "Bearer".into()))
        }
    }
}

/// `GET /api/chat/status`: `unconfigured` without a relay.
fn chat_status(ctx: &Ctx) -> Json {
    ctx.chat.as_ref().map_or_else(
        || {
            json!({
                "state": "unconfigured", "since": null, "kind": null, "reason": null,
                "seq": 0, "fallback": false,
                "message": "no relay configured on this box (--relay); the agent is offline",
            })
        },
        chat::Chat::status,
    )
}

/// `POST /api/chat`: audited when it starts; the turn itself streams.
fn start_chat(req: &Request, ctx: &Ctx) -> Reply {
    let Some(chat) = &ctx.chat else {
        let r = Response::error(
            503,
            "no relay configured on this box (--relay); the agent is offline",
        );
        return Reply::Once(match audit(ctx, req, 503, "chat: no relay configured") {
            Some(_) => r,
            None => audit_full(),
        });
    };
    let taken = chat
        .take(&req.body)
        .and_then(|t| chat.attach(t, |id| ctx.dropins().attachment(id)));
    match taken {
        Err((r, detail)) => Reply::Once(match audit(ctx, req, r.status, &detail) {
            Some(_) => r,
            None => audit_full(),
        }),
        Ok(taken) => {
            let detail = format!(
                "chat: session {}, message of {} characters",
                taken.id,
                taken.message.chars().count()
            );
            if audit(ctx, req, 200, &detail).is_none() {
                chat.give_back(&taken.id, taken.conv);
                return Reply::Once(audit_full());
            }
            Reply::Chat(taken)
        }
    }
}

/// `POST /api/explain`: checked and built here (so a bad request is a
/// plain 4xx), audited, then streamed like a chat.
fn start_explain(req: &Request, ctx: &Ctx) -> Reply {
    let refuse = |r: Response, detail: &str| {
        Reply::Once(match audit(ctx, req, r.status, detail) {
            Some(_) => r,
            None => audit_full(),
        })
    };
    let ask = match explain::parse(&req.body) {
        Ok(a) => a,
        Err((r, detail)) => return refuse(r, &detail),
    };
    let c = ctx.catalog();
    let Some(k) = c.module(&ask.module) else {
        return refuse(Response::error(404, "no such module"), "explain: no such module");
    };
    if ctx.chat.is_none() {
        return refuse(
            Response::error(
                503,
                "no relay configured on this box (--relay); the agent is offline. The box's own explanation still works.",
            ),
            "explain: no relay configured",
        );
    }
    let Some(sample) = sample(ctx, k) else {
        return refuse(
            Response::error(503, "no traced scan of this module yet; ask again"),
            "explain: no trace yet",
        );
    };
    let m = &c.modules[k];
    let job = match explain::build(&m.name, &m.source, &sample, &ask.path, &c) {
        Ok(j) => j,
        Err(e) => {
            return refuse(
                Response::error(
                    409,
                    &format!("the module changed under its trace ({e}); ask again"),
                ),
                "explain: source and trace differ",
            );
        }
    };
    let detail = format!("explain: module {}, scan {}", m.name, sample.scan);
    if audit(ctx, req, 200, &detail).is_none() {
        return Reply::Once(audit_full());
    }
    Reply::Explain(job)
}

/// The newest traced scan of module `k`, waiting up to a second for one.
fn sample(ctx: &Ctx, k: usize) -> Option<Arc<crate::Sample>> {
    let name = ctx.catalog().modules.get(k)?.name.clone();
    for _ in 0..50 {
        latest_trace(ctx, k);
        if let Some(s) = ctx
            .samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&name)
        {
            return Some(Arc::clone(s));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Records a mutating call; `None` if the log is full (the call must not
/// take effect).
fn audit(ctx: &Ctx, req: &Request, status: u16, detail: &str) -> Option<u64> {
    audit_with(ctx, req, status, detail, None)
}

/// [`audit`], naming the snapshot the call concerns.
fn audit_with(ctx: &Ctx, req: &Request, status: u16, detail: &str, snapshot: Option<String>) -> Option<u64> {
    append(ctx, req, status, detail, snapshot, None, None)
}

/// [`audit`] with a [`Note`]: the snapshots and the role.
fn record(ctx: &Ctx, req: &Request, status: u16, detail: &str, note: Note) -> Option<u64> {
    append(
        ctx,
        req,
        status,
        detail,
        note.snapshot.map(|s| s.to_string()),
        note.base.map(|s| s.to_string()),
        note.role,
    )
}

/// Appends an entry; the person and role are the session's, when the
/// call came with one (a role the call names itself, as a PIN does, wins).
fn append(
    ctx: &Ctx,
    req: &Request,
    status: u16,
    detail: &str,
    snapshot: Option<String>,
    base: Option<String>,
    role: Option<Role>,
) -> Option<u64> {
    let s = req.session.as_ref();
    let role = role.or(s.map(|p| p.role));
    append_as(
        ctx,
        req,
        status,
        detail,
        (snapshot, base),
        s.and_then(|p| p.person().map(str::to_string)),
        role,
    )
}

/// [`append`], naming `user` whatever the request carried (a login).
fn append_as(
    ctx: &Ctx,
    req: &Request,
    status: u16,
    detail: &str,
    (snapshot, base): (Option<String>, Option<String>),
    user: Option<String>,
    role: Option<Role>,
) -> Option<u64> {
    let seq = ctx
        .box_
        .audit
        .append(AuditEntry {
            seq: 0,
            at: wall(),
            method: req.method.clone(),
            path: req.path.clone(),
            status,
            detail: detail.into(),
            snapshot,
            base,
            role: role.map(|r| r.as_str().to_string()),
            source: req.source.into(),
            user,
        })
        .ok();
    (ctx.cfg.log)(&format!(
        "api: {} {} -> {status}: {}{}",
        clean(&req.method),
        clean(&req.path),
        clean(detail),
        if seq.is_some() { "" } else { " (audit full)" }
    ));
    seq
}

fn audit_full() -> Response {
    Response::error(507, "audit log full; nothing was done")
}

/// Answers an authenticated non-GET request; every outcome is audited.
fn mutate(req: &Request, ctx: &Ctx) -> Response {
    let segs: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    let (r, detail) = match (req.method.as_str(), segs.as_slice()) {
        (_, ["api", "projects", ..]) => return projects::mutate(req, ctx, &segs[1..]),
        (_, ["api", "forces", ..]) => return forces::mutate(req, ctx, &segs[1..]),
        (_, ["api", "drivers", ..]) => return drivers::mutate(req, ctx, &segs[1..]),
        ("PUT", ["api", "trends", "pins"]) => return trend::put_pins(req, ctx),
        ("POST", ["api", "proposals"]) => return create(req, ctx),
        ("POST", ["api", "logout"]) => return login::logout(req, ctx),
        ("POST", ["api", "bundles"]) => return bundle::import(req, ctx),
        ("POST", ["api", "dropins"]) => return dropin::post(req, ctx),
        ("POST", ["api", "devices", name, "browse"]) => return dropin::browse(req, ctx, name),
        ("POST", ["api", "proposals", "check"]) => return check_only(req, ctx),
        ("POST", ["api", "proposals", "replay"]) => return replay_draft(req, ctx),
        ("POST", ["api", "proposals", id, "apply"]) => {
            return approve::apply(req, ctx, approve::Target::Proposal(id));
        }
        ("POST", ["api", "proposals", id, "replay"]) => {
            return approve::replay(req, ctx, approve::Target::Proposal(id));
        }
        ("POST", ["api", "snapshots", id, "rollback"]) => {
            return approve::apply(req, ctx, approve::Target::Snapshot(id));
        }
        ("POST", ["api", "snapshots", id, "replay"]) => {
            return approve::replay(req, ctx, approve::Target::Snapshot(id));
        }
        (_, path) => match allowed(path) {
            Some(allow) => (
                Response::error(405, "method not allowed").with("Allow", allow.into()),
                "method not allowed".to_string(),
            ),
            None => (
                Response::error(404, "no such resource"),
                "no such resource".to_string(),
            ),
        },
    };
    match audit(ctx, req, r.status, &detail) {
        Some(_) => r,
        None => audit_full(),
    }
}

/// The methods a known path takes.
fn allowed(path: &[&str]) -> Option<&'static str> {
    if let Some(allow) = projects::allowed(path) {
        return Some(allow);
    }
    match path {
        ["api", "forces"] => return Some("GET, POST, DELETE"),
        ["api", "forces", _] => return Some("DELETE"),
        ["api", "drivers"] => return Some("GET"),
        ["api", "drivers", _] => return Some("PUT, DELETE"),
        ["api", "trends"] => return Some("GET"),
        ["api", "trends", "pins"] => return Some("GET, PUT"),
        _ => {}
    }
    match path {
        ["api", "proposals" | "dropins"] => Some("GET, POST"),
        ["api", "proposals", _, "apply" | "replay"]
        | ["api", "snapshots", _, "rollback" | "replay"]
        | ["api", "proposals", "check" | "replay"]
        | ["api", "chat" | "explain" | "bundles" | "login" | "logout"]
        | ["api", "devices", _, "browse"] => Some("POST"),
        ["api"]
        | [
            "api",
            "signals" | "modules" | "outputs" | "devices" | "audit" | "events" | "snapshots" | "policy"
            | "session" | "site" | "io_map",
        ]
        | [
            "api",
            "signals" | "modules" | "proposals" | "snapshots" | "replays" | "dropins",
            _,
        ]
        | ["api", "snapshots", _, "diff", _]
        | ["api", "snapshots", _, "bundle"]
        | ["api", "dropins", _, "plc"]
        | ["api", "modules", _, "trace"]
        | ["api", "chat", "status"]
        | ["api", "modules", _, "trace", "events"]
        | ["api", "traces", "events"] => Some("GET"),
        _ => None,
    }
}

fn proposal_id(s: &str) -> Option<u64> {
    if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Answers with `status` and `body` once the call is audited.
fn audited(ctx: &Ctx, req: &Request, status: u16, body: &Json, detail: &str) -> Response {
    match audit(ctx, req, status, detail) {
        Some(_) => Response::json(status, body),
        None => audit_full(),
    }
}

fn is_json(req: &Request) -> bool {
    req.content_type
        .as_deref()
        .is_some_and(|t| t == "application/json" || t.starts_with("application/json;"))
}

/// `POST /api/proposals/check`: the checks `POST /api/proposals` runs,
/// with nothing stored.
fn check_only(req: &Request, ctx: &Ctx) -> Response {
    if !is_json(req) {
        return audited(
            ctx,
            req,
            415,
            &json!({ "error": "Content-Type must be application/json" }),
            "check: not JSON",
        );
    }
    let new: proposal::New = match serde_json::from_slice(&req.body) {
        Ok(n) => n,
        Err(e) => {
            return audited(
                ctx,
                req,
                400,
                &json!({ "error": format!("malformed proposal: {e}") }),
                "check: malformed proposal",
            );
        }
    };
    let running = ctx.running();
    match checked(ctx, &new, running.as_ref()) {
        Ok((mut p, _)) => {
            p["stored"] = json!(false);
            audited(ctx, req, 200, &p, "check: clean, not stored")
        }
        Err((status, body)) => audited(ctx, req, status, &body, "check: refused by the checks"),
    }
}

/// [`proposal::check`], after the drop-in rule: a proposal that names a
/// PLC drop-in, or one the box no longer holds, must be a watch module
/// (see [`dropin`]).
fn checked(
    ctx: &Ctx,
    new: &proposal::New,
    running: Option<&ChangeSet>,
) -> Result<(Json, Option<ChangeSet>), proposal::Refusal> {
    let cited = new.dropins.as_deref().unwrap_or_default();
    if cited.len() > 64 {
        return Err((400, json!({ "error": "a proposal names at most 64 drop-ins" })));
    }
    let plc: Vec<u64> = {
        let store = ctx.dropins();
        cited
            .iter()
            .copied()
            .filter(|id| store.kind(*id).is_none_or(dropin::Kind::is_plc))
            .collect()
    };
    if !plc.is_empty() && (new.kind != "module" || new.watch != Some(true)) {
        let problems: Vec<String> = plc
            .iter()
            .map(|id| format!("drop-in {id} is an existing PLC program (or no longer on the box)"))
            .collect();
        return Err((
            422,
            json!({
                "error": "a proposal drawn from an existing PLC program must be a watch module (kind module, watch: true): it reads and reports, and drives no output",
                "problems": problems,
            }),
        ));
    }
    proposal::check(new, &ctx.catalog(), ctx.cfg.memory, running, &ctx.sources(None))
}

/// Minutes of recording a draft replay covers when the request does not say.
const REPLAY_MINUTES: u64 = 10;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayDraft {
    draft: proposal::New,
    #[serde(default)]
    minutes: Option<u64>,
}

/// `POST /api/proposals/replay`: a draft replayed beside what runs; the
/// draft is checked as `POST /api/proposals` would, and nothing is stored.
fn replay_draft(req: &Request, ctx: &Ctx) -> Response {
    if !is_json(req) {
        return audited(
            ctx,
            req,
            415,
            &json!({ "error": "Content-Type must be application/json" }),
            "replay: not JSON",
        );
    }
    let r: ReplayDraft = match serde_json::from_slice(&req.body) {
        Ok(r) => r,
        Err(e) => {
            return audited(
                ctx,
                req,
                400,
                &json!({ "error": format!("malformed replay request: {e}") }),
                "replay of a draft: malformed request",
            );
        }
    };
    let minutes = r.minutes.unwrap_or(REPLAY_MINUTES);
    if !(1..=sim::MAX_MINUTES).contains(&minutes) {
        let body = json!({ "error": format!("minutes must be 1 to {}", sim::MAX_MINUTES) });
        return audited(ctx, req, 400, &body, "replay of a draft: bad minutes");
    }
    if r.draft.kind != "module" {
        let body = json!({ "error": "replay takes a module draft" });
        return audited(ctx, req, 400, &body, "replay of a draft: not a module");
    }
    let running = ctx.running();
    let target = match checked(ctx, &r.draft, running.as_ref()) {
        Ok((_, target)) => target,
        Err((status, body)) => {
            return audited(
                ctx,
                req,
                status,
                &body,
                "replay of a draft: refused by the checks",
            );
        }
    };
    let (Some(appr), Some(running), Some(target)) = (&ctx.approval, running, target) else {
        let body =
            json!({ "error": "this box cannot replay drafts: it runs no change set or has no recording" });
        return audited(ctx, req, 409, &body, "replay of a draft: no change set");
    };
    match sim::run(&appr.plant, &running, &target, minutes) {
        Ok(j) => audited(
            ctx,
            req,
            200,
            &j,
            &format!("replay of a draft: {minutes} min, read-only, not stored"),
        ),
        Err((status, body)) => audited(ctx, req, status, &body, "replay of a draft: refused"),
    }
}

fn create(req: &Request, ctx: &Ctx) -> Response {
    let refused = |status: u16, body: Json, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::json(status, &body),
        None => audit_full(),
    };
    let json_type = req
        .content_type
        .as_deref()
        .is_some_and(|t| t == "application/json" || t.starts_with("application/json;"));
    if !json_type {
        return refused(
            415,
            json!({ "error": "Content-Type must be application/json" }),
            "not JSON",
        );
    }
    let new: proposal::New = match serde_json::from_slice(&req.body) {
        Ok(n) => n,
        Err(e) => {
            return refused(
                400,
                json!({ "error": format!("malformed proposal: {e}") }),
                "malformed proposal",
            );
        }
    };
    let running = ctx.running();
    let checked = checked(ctx, &new, running.as_ref());
    let (mut p, change) = match checked {
        Ok(p) => p,
        Err((status, body)) => return refused(status, body, "proposal refused by the checks"),
    };
    let Ok(id) = ctx.box_.proposals.reserve() else {
        return refused(507, json!({ "error": "too many proposals" }), "proposals full");
    };
    let what = match (p["kind"].as_str(), p["name"].as_str(), p["module"].as_str()) {
        (Some("module"), Some(name), _) => format!("proposal {id}: module {name}"),
        (_, _, Some(module)) => format!(
            "proposal {id}: param {}.{}",
            module,
            p["param"].as_str().unwrap_or_default()
        ),
        _ => format!("proposal {id}"),
    };
    p["id"] = json!(id);
    p["status"] = json!("draft");
    p["at"] = json!(wall());
    p["approval"] = match (&running, &change) {
        (Some(base), Some(cs)) => approve::tier_json(ctx, base, cs, None),
        _ => Json::Null,
    };
    let snap = change.as_ref().map(|cs| cs.id().to_string());
    let what = match (&snap, p["base"].as_str()) {
        (Some(to), Some(base)) => format!("{what}: snapshot {base} -> {to}"),
        _ => what,
    };
    if audit_with(ctx, req, 201, &what, snap).is_none() {
        return audit_full();
    }
    ctx.box_.proposals.insert(store::Stored {
        id,
        json: p.clone(),
        change,
    });
    Response::json(201, &p).with("Location", format!("/api/proposals/{id}"))
}

/// A file of the canvas (`--ui`); with a site, its page may fetch peers.
fn ui_file(req: &Request, ctx: &Ctx) -> Reply {
    match &ctx.ui {
        Some(ui) => match ui.resolve(&req.segments) {
            Ok(mut asset) => {
                if ctx.site.is_some() {
                    asset.allow_peers();
                }
                Reply::File(asset)
            }
            Err(r) => Reply::Once(r),
        },
        None => Reply::Once(Response::error(
            404,
            "no such resource (no UI on this box: --ui DIR)",
        )),
    }
}

fn get(req: &Request, ctx: &Ctx) -> Reply {
    if req.segments.first().is_none_or(|s| s != "api") {
        return ui_file(req, ctx);
    }
    let c = &*ctx.catalog();
    let segs: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    let view = ctx.box_.live.view();
    let module = |name: &str| c.module(name);
    let r = match segs.as_slice() {
        ["api"] => Response::json(200, &index(ctx.chat.is_some())),
        ["api", "signals"] => Response::json(200, &forces::mark(ctx, render::signals(c, &view))),
        ["api", "signals", name] => match c.signal(name) {
            Some(i) => Response::json(
                200,
                &forces::mark(
                    ctx,
                    render::signal(
                        &c.signals[i],
                        view.signals.get(i).copied().flatten(),
                        view.errors.get(i).and_then(Option::as_deref),
                    ),
                ),
            ),
            None => Response::error(404, "no such signal"),
        },
        ["api", "modules"] => Response::json(200, &render::modules(c, &view)),
        ["api", "modules", name] => match module(name) {
            Some(k) => Response::json(200, &render::module_detail(&c.modules[k], view.modules.get(k))),
            None => Response::error(404, "no such module"),
        },
        ["api", "modules", name, "trace"] => match module(name) {
            Some(k) => match latest_trace(ctx, k) {
                Some((_, j)) => Response::json(200, &with_forces(ctx, &j)),
                None => Response::json(
                    202,
                    &json!({ "pending": true, "why": "the module's next scan is traced; ask again" }),
                ),
            },
            None => Response::error(404, "no such module"),
        },
        ["api", "modules", name, "trace", "events"] => match module(name) {
            Some(k) => return Reply::Traces(k),
            None => Response::error(404, "no such module"),
        },
        ["api", "outputs"] => Response::json(200, &render::outputs(c, &view)),
        ["api", "devices"] => Response::json(200, &devices(&ctx.box_.devices)),
        ["api", "io_map"] => io_map(ctx),
        ["api", "proposals"] => Response::json(
            200,
            &Json::Array(ctx.box_.proposals.list().into_iter().map(|p| p.json).collect()),
        ),
        ["api", "proposals", id] => match proposal_id(id).and_then(|id| ctx.box_.proposals.get(id)) {
            Some(p) => Response::json(200, &p.json),
            None => Response::error(404, "no such proposal"),
        },
        ["api", "audit"] => audit_list(req, ctx),
        ["api", "snapshots"] => with_store(ctx, |s| {
            let entries = s.list()?;
            // Newest first: head leads.
            let head = entries.first().map(|e| e.id.to_string());
            Ok(json!({
                "head": head,
                "running": ctx.running().map(|cs| cs.id().to_string()),
                "log": entries.iter().map(render::entry).collect::<Vec<_>>(),
            }))
        }),
        ["api", "snapshots", "head"] => {
            with_store(ctx, |s| Ok(s.head()?.as_ref().map_or(Json::Null, render::entry)))
        }
        ["api", "snapshots", id] => match snapshot(ctx, id) {
            Ok(cs) => Response::json(200, &render::change_set(&cs)),
            Err(r) => r,
        },
        ["api", "snapshots", id, "bundle"] => bundle::export(ctx, id),
        ["api", "snapshots", a, "diff", b] => match (snapshot(ctx, a), snapshot(ctx, b)) {
            (Ok(a), Ok(b)) => Response::json(200, &render::diff(&snapshots::diff(&a, &b))),
            (Err(r), _) | (_, Err(r)) => r,
        },
        ["api", "events"] => return Reply::Values,
        ["api", "traces", "events"] => return Reply::AllTraces,
        ["api", "projects", ..] | ["api", "running"] => projects::get(ctx, &segs[1..]),
        ["api", "forces"] => forces::get(ctx),
        ["api", "drivers"] => drivers::list(req, ctx),
        ["api", "trends", rest @ ..] => trend::get(req, ctx, rest),
        ["api", "dropins", rest @ ..] => dropin::get(ctx, req, rest),
        ["api", "chat", "status"] => Response::json(200, &chat_status(ctx)),
        ["api", "site"] => site_route(ctx, c, &view),
        ["api", "policy"] => match &ctx.approval {
            Some(a) => Response::json(200, &policy_json(a, ctx)),
            None => Response::error(404, "no approval path on this box"),
        },
        ["api", "session"] => login::whoami(req, ctx),
        ["api", "replays", id] => match proposal_id(id).and_then(|id| ctx.replays.get(id)) {
            Some(e) => Response::json(200, &e.json),
            None => Response::error(404, "no such replay"),
        },
        _ => Response::error(404, "no such resource"),
    };
    Reply::Once(r)
}

/// Runs `f` on the store; 404 without one, 500 if it fails.
fn with_store(ctx: &Ctx, f: impl FnOnce(&Store) -> Result<Json, snapshots::Error>) -> Response {
    let Some(store) = &ctx.box_.snapshots else {
        return Response::error(404, "no snapshot store on this box (--store)");
    };
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    match f(&store) {
        Ok(j) => Response::json(200, &j),
        Err(e) => store_error(&e),
    }
}

fn store_error(e: &snapshots::Error) -> Response {
    match e {
        snapshots::Error::NotFound(_) => Response::error(404, "no such snapshot"),
        e => Response::error(500, &e.to_string()),
    }
}

/// A snapshot's content by id (64 hex digits): from the store, or the
/// running change set when it has that id.
/// `GET /api/io_map`: the running map file, to draft a new one from.
fn io_map(ctx: &Ctx) -> Response {
    match ctx.running() {
        Some(cs) => Response::json(
            200,
            &json!({ "snapshot": cs.id().to_string(), "text": cs.io_map.unwrap_or_default() }),
        ),
        None => Response::error(
            404,
            "this box does not run a change set (module directories or a store), so its map cannot be read or proposed",
        ),
    }
}

fn snapshot(ctx: &Ctx, id: &str) -> Result<ChangeSet, Response> {
    let Some(id) = SnapshotId::parse(id) else {
        return Err(Response::error(404, "no such snapshot"));
    };
    if let Some(cs) = ctx.running().filter(|cs| cs.id() == id) {
        return Ok(cs);
    }
    let Some(store) = &ctx.box_.snapshots else {
        return Err(Response::error(404, "no such snapshot"));
    };
    let store = store.lock().unwrap_or_else(PoisonError::into_inner);
    let loaded = store.load(&id).map_err(|e| store_error(&e))?;
    if !loaded.ignored_drivers.is_empty() {
        (ctx.cfg.log)(&format!(
            "store: snapshot {id} holds WASM drivers from an older build ({}); they are ignored: \
             drivers are files now (drivers/NAME.wasm on the box or in the project)",
            loaded.ignored_drivers.join(", ")
        ));
    }
    Ok(loaded.content)
}

fn index(agent: bool) -> Json {
    let e = |method: &str, path: &str, token: bool, what: &str| json!({ "method": method, "path": path, "token": token, "what": what });
    json!({
        "service": "helmsman box API",
        "endpoints": [
            e("GET", "/api/signals", false, "every signal with its live value and quality"),
            e("GET", "/api/signals/{name}", false, "one signal"),
            e("GET", "/api/modules", false, "every module: schedule, status, stats"),
            e("GET", "/api/modules/{name}", false, "one module with ports, source and manifest"),
            e("GET", "/api/modules/{name}/trace", false, "request a traced scan; 200 with the newest, 202 until one lands"),
            e("GET", "/api/modules/{name}/trace/events", false, "event stream of traced scans"),
            e(
                "GET",
                "/api/traces/events",
                false,
                "event stream of every module's traced scans",
            ),
            e("GET", "/api/outputs", false, "output commands, safe values, the %Q image"),
            e("GET", "/api/devices", false, "device links up or down"),
            e("GET", "/api/io_map", false, "the running I/O map file and its snapshot id"),
            e("GET", "/api/proposals", false, "drafts awaiting approval"),
            e("GET", "/api/proposals/{id}", false, "one proposal"),
            e("POST", "/api/proposals", true, "store a checked draft: {kind: module, manifest, source or wasm (base64)}, {kind: param, module, param, value}, or {kind: io_map|calendar, text}; any of them may cite dropins: [ids] and, of those, refused_dropins: the ones the relay refused"),
            e("POST", "/api/proposals/check", true, "check a draft as POST /api/proposals would, storing nothing"),
            e("POST", "/api/proposals/replay", true, "sim replay of a module draft {draft: {kind: module, manifest, source}, minutes}; nothing stored, not evidence"),
            e("POST", "/api/proposals/{id}/replay", true, "sim replay of the draft against what runs (evidence)"),
            e("POST", "/api/login", false, "log in {user, password} (with an accounts file) or {role, pin} (without): a session token for Authorization: Bearer, or with X-Helmsman-Request: 1 from the box's own page an HttpOnly cookie"),
            e("POST", "/api/logout", true, "end the session it is sent with (and clear its cookie)"),
            e("GET", "/api/session", false, "who the session sent (cookie or bearer) is, or 401"),
            e("POST", "/api/proposals/{id}/apply", true, "apply: with a session {replay?}, else {role, pin, replay?} (PINs only while the box has no accounts), checked against the policy"),
            e("GET", "/api/audit?after=N&limit=M", false, "every mutating call"),
            e("GET", "/api/snapshots", false, "snapshot history (needs --store), head, and the running change set's id"),
            e("GET", "/api/snapshots/head", false, "the head entry"),
            e("GET", "/api/snapshots/{id}", false, "a snapshot's files"),
            e("GET", "/api/snapshots/{a}/diff/{b}", false, "what changed from a to b"),
            e("GET", "/api/snapshots/{id}/bundle", false, "that snapshot as an unsigned bundle file (sign it off the box)"),
            e("POST", "/api/bundles", true, "import a bundle file (application/octet-stream): verified against the box's trusted publishers, bound to its signals by name, stored as a draft"),
            e("POST", "/api/snapshots/{id}/replay", true, "sim replay of a rollback to it (evidence)"),
            e("POST", "/api/snapshots/{id}/rollback", true, "roll back: as apply, checked against the policy"),
            e("GET", "/api/replays/{n}", false, "a replay kept as evidence"),
            e("GET", "/api/policy", false, "the approval policy this box enforces, and its transport: https and the certificate's SHA-256 fingerprint"),
            e("GET", "/api/events", false, "event stream of live values and module status"),
            e("POST", "/api/chat", true, "one message to the agent {message, session?, dropins?}; an event stream of its progress"),
            e("POST", "/api/explain", true, "why a variable is in its state {module, path}: the box's explanation, then the agent's, streamed; read-only"),
            e("POST", "/api/dropins?name=N&kind=K", true, "a document for the agent (text, Markdown, L5X, PLCopen XML, ST; at most 1 MiB), a photo (PNG, JPEG, WebP; at most 512 KiB, 8000 px a side) or a PDF (at most 2 MiB, passed to the model unread): stored in memory, 201 with its id"),
            e("GET", "/api/dropins", false, "the drop-ins held: id, name, kind, size, chunks"),
            e("GET", "/api/dropins/{id}?chunk=K", false, "one chunk of a drop-in's text"),
            e("GET", "/api/dropins/{id}/plc?view=V&name=N&offset=O", false, "a PLC drop-in's inventory: overview, tags, routine, names"),
            e("POST", "/api/devices/{name}/browse", true, "an observed Logix controller's tag list, read-only"),
            e("GET", "/api/chat/status", false, "the agent: online, offline since T and why, or unknown; offline, chat reads param edits by rule"),
            e("GET", "/api/site", false, "this box's status and the boxes it heard on the LAN (with --site); readable from other origins"),
            e("GET", "/api/projects", false, "the editor's projects (with --projects) and which one runs"),
            e("POST", "/api/projects", true, "engineer session: {name} a new empty project, or {name, from} a copy"),
            e("GET", "/api/projects/{p}", false, "a project's files: name, bytes, version (SHA-256)"),
            e("DELETE", "/api/projects/{p}", true, "engineer session: delete a project (never the running one)"),
            e("POST", "/api/projects/{p}/rename", true, "engineer session: {to}"),
            e("GET", "/api/projects/{p}/files/{f}", false, "one file: {name, text, version}"),
            e("PUT", "/api/projects/{p}/files/{f}", true, "engineer session: save {text, version?} (a draft; version: what was read, null: a new file)"),
            e("DELETE", "/api/projects/{p}/files/{f}", true, "engineer session: delete a file"),
            e("POST", "/api/projects/{p}/check", true, "build the project as a deploy would: its modules and tier, or every problem with file and line; nothing changes"),
            e("POST", "/api/projects/{p}/replay", true, "engineer session: sim replay of deploying the project (evidence for deploy)"),
            e("POST", "/api/projects/{p}/deploy", true, "engineer session: {replay?}: build, apply through the approval policy, commit a snapshot, hot-swap between scans"),
            e("GET", "/api/projects/{p}/zip", false, "the project as a .zip (its .st files, io.map, project.settings)"),
            e("POST", "/api/projects/{p}/zip?replace=true", true, "engineer session: a .zip (application/zip) as project p, new or (replace=true) in its place"),
            e("POST", "/api/projects/{p}/import?file=F&replace=true", true, "engineer session: one file into the project: F.st as it is, F.xml (PLCopen TC6) lowered to F.st"),
            e("GET", "/api/trends?signals=a,b", false, "each signal's history (10 min, on change) on the scheduler's clock; keeps them tracked a day: {now_ms, window_ms, signals: [{name, points: [[t_ms, value]]}], unknown, refused}"),
            e("GET", "/api/trends/pins", false, "the signed-in person's pinned signals"),
            e("PUT", "/api/trends/pins", true, "a login session: {signals: [...]} (at most 16), that person's pins"),
            e("GET", "/api/forces", false, "active forced inputs: {id, signal | address, value, by, at}"),
            e("POST", "/api/forces", true, "engineer session: force an input {signal, value} or {address: %IX0.0 | %IBn | %IWn | %IDn, value}; ends at that session's logout or expiry and on deploy"),
            e("DELETE", "/api/forces/{id}", true, "engineer session: release one force"),
            e("DELETE", "/api/forces", true, "engineer session: release every force"),
            e("GET", "/api/drivers", true, "engineer session: the box's WASM driver files (drivers/NAME.wasm): name, bytes, sha256, problems"),
            e("PUT", "/api/drivers/{name}", true, "engineer session: the driver's .wasm bytes, checked as a load checks them, written atomically; runs from the next deploy or map reload"),
            e("DELETE", "/api/drivers/{name}", true, "engineer session: delete a driver file (not one the running map loads from the box)"),
            e("GET", "/api/running", false,"the project deployed last, its snapshot, and whether it is what runs"),
            e("POST", "/mcp", true,"MCP (Streamable HTTP, 2026-07-28 or 2025-11-25): the agent's read, draft, check, simulate and propose tools for an engineer's own client; no apply"),
        ],
        "agent": agent,
        "writes": "no endpoint writes a signal, an output or a module; a proposal runs only through apply, which needs a role PIN the policy allows (and a sim replay for logic and output-affecting changes)",
    })
}

fn site_route(ctx: &Ctx, c: &Catalog, view: &View) -> Response {
    match &ctx.site {
        Some(site) => Response::json(200, &site_json(site, c, view, &ctx.box_.devices))
            .with("Access-Control-Allow-Origin", "*".into()),
        None => Response::error(404, "this box is in no site (--site NAME)"),
    }
}

/// `GET /api/site`: this box (its beacon and a status summary: module
/// and device-link counts by state, no names, no values) and the peers it
/// heard, with what it dropped.
///
/// **Read from other origins.** The site view on one box's canvas asks
/// each peer for this directly, so it carries
/// `Access-Control-Allow-Origin: *`, and nothing else does: no other
/// route, no credentials (a wildcard origin never carries cookies, and the
/// API's token is a header no other origin can set), no preflight (any
/// other method still needs the token). A page from anywhere that the
/// browser of someone on the floor opens can therefore read this body:
/// it holds only what the beacons already tell the whole LAN, plus counts.
/// The other choice, this box fetching its peers for the browser, would
/// make every box open connections to addresses learned from untrusted
/// beacons; here a box never acts on what it heard.
fn site_json(site: &crate::site::Handle, c: &Catalog, view: &View, devices: &Devices) -> Json {
    let me = site.me();
    let (peers, counts, now) = site.snapshot();
    let (mut running, mut held, mut faulted) = (0usize, 0usize, 0usize);
    for k in 0..c.modules.len() {
        match view.modules.get(k).map(|m| m.status) {
            Some(crate::Status::Running) => running += 1,
            Some(crate::Status::Held) => held += 1,
            Some(crate::Status::Faulted(_)) => faulted += 1,
            None => {}
        }
    }
    let (mut up, mut down, mut unknown) = (0usize, 0usize, 0usize);
    for (_, state) in devices.list() {
        match state {
            None => unknown += 1,
            Some(Ok(())) => up += 1,
            Some(Err(_)) => down += 1,
        }
    }
    let age = |t: Duration| u64::try_from(now.saturating_sub(t).as_millis()).unwrap_or(u64::MAX);
    json!({
        "site": me.site,
        "self": {
            "id": me.id,
            "name": me.name,
            "api_port": me.api,
            "version": me.version,
            "status": {
                "published": view.seq > 0,
                "modules": c.modules.len(),
                "running": running,
                "held": held,
                "faulted": faulted,
                "devices": { "up": up, "down": down, "unknown": unknown },
            },
        },
        "peers": peers.iter().map(|p| json!({
            "id": p.id,
            "name": p.name,
            "version": p.version,
            "address": p.addr.to_string(),
            "last_seen_ms": age(p.last),
            "first_seen_ms": age(p.first),
            "conflict": p.conflict.map(|ip| ip.to_string()),
        })).collect::<Vec<_>>(),
        "max_peers": crate::site::MAX_PEERS,
        "dropped": counts,
    })
}

fn devices(d: &Devices) -> Json {
    Json::Array(
        d.list()
            .into_iter()
            .map(|(name, state)| match state {
                None => json!({ "name": name, "state": "unknown" }),
                Some(Ok(())) => json!({ "name": name, "state": "up" }),
                Some(Err(why)) => json!({ "name": name, "state": "down", "error": why }),
            })
            .collect(),
    )
}

fn audit_list(req: &Request, ctx: &Ctx) -> Response {
    let mut after = 0u64;
    let mut limit = 100usize;
    for (k, v) in &req.query {
        let n = (!v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse::<u64>().ok())
            .flatten();
        match (k.as_str(), n) {
            ("after", Some(n)) => after = n,
            ("limit", Some(n)) => limit = usize::try_from(n.min(1000)).unwrap_or(1000),
            ("after" | "limit", None) => return Response::error(400, "after and limit are whole numbers"),
            _ => {}
        }
    }
    let entries: Vec<Json> = ctx
        .box_
        .audit
        .list(after, limit)
        .into_iter()
        .map(|e| {
            json!({
                "seq": e.seq, "at": e.at, "method": e.method, "path": e.path,
                "status": e.status, "detail": e.detail, "snapshot": e.snapshot,
                "base": e.base, "role": e.role, "source": e.source, "user": e.user,
            })
        })
        .collect();
    Response::json(200, &Json::Array(entries))
}

/// Asks module `k` for a traced scan and returns the newest one seen.
fn latest_trace(ctx: &Ctx, k: usize) -> Option<Rendered> {
    let c = ctx.catalog();
    let m = c.modules.get(k)?;
    let tap = m.tap.as_ref()?;
    tap.request();
    let sample = tap.take();
    let fresh = sample
        .as_ref()
        .map(|s| (s.scan, Arc::new(render::trace(&m.name, s))));
    if let Some(s) = sample {
        ctx.samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(m.name.clone(), Arc::new(s));
    }
    let mut traces = ctx.traces.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(f) = fresh {
        traces.insert(m.name.clone(), f);
    }
    traces.get(&m.name).cloned()
}

fn event(stream: &mut Conn, name: &str, data: &Json) -> io::Result<()> {
    let body = serde_json::to_string(data).unwrap_or_default();
    write!(stream, "event: {name}\ndata: {body}\n\n")?;
    stream.flush()
}

/// Streams until the client goes, a write stalls past the write timeout,
/// or the API stops. `next` yields what to send, if anything is new.
fn pump(
    stream: &mut Conn,
    ctx: &Ctx,
    mut next: impl FnMut() -> Option<(&'static str, Json)>,
) -> io::Result<()> {
    pump_many(stream, ctx, || next().into_iter().collect())
}

/// [`pump`], sending every event `next` yields at once.
fn pump_many(
    stream: &mut Conn,
    ctx: &Ctx,
    mut next: impl FnMut() -> Vec<(&'static str, Json)>,
) -> io::Result<()> {
    http::start_stream(stream)?;
    stream.write_all(b"retry: 2000\n\n")?;
    let mut quiet = Instant::now();
    let opened = Instant::now();
    while !ctx.stop.load(Ordering::Acquire) && opened.elapsed() < ctx.cfg.limits.stream_life {
        let batch = next();
        if !batch.is_empty() {
            for (name, data) in &batch {
                event(stream, name, data)?;
            }
            quiet = Instant::now();
        } else if quiet.elapsed() >= KEEPALIVE {
            stream.write_all(b": keepalive\n\n")?;
            stream.flush()?;
            quiet = Instant::now();
        }
        thread::sleep(ctx.cfg.stream_every);
    }
    Ok(())
}

fn value_stream(stream: &mut Conn, ctx: &Ctx) -> io::Result<()> {
    let mut last = None;
    let mut agent = None;
    pump(stream, ctx, || {
        if let Some(c) = &ctx.chat {
            let seq = c.status_seq();
            if agent != Some(seq) {
                agent = Some(seq);
                return Some(("agent", c.status()));
            }
        }
        let view = ctx.box_.live.view();
        if last == Some(view.seq) {
            return None;
        }
        last = Some(view.seq);
        let c = &*ctx.catalog();
        Some((
            "values",
            json!({
                "seq": view.seq,
                "at_ms": u64::try_from(view.at.as_millis()).unwrap_or(u64::MAX),
                "signals": forces::mark(ctx, render::signals(c, &view)),
                "modules": render::modules(c, &view),
                "outputs": view.outputs,
                "forces": forces::list(ctx),
            }),
        ))
    })
}

fn trace_stream(stream: &mut Conn, ctx: &Ctx, k: usize) -> io::Result<()> {
    let mut last = None;
    pump(stream, ctx, || {
        let (scan, j) = latest_trace(ctx, k)?;
        if last == Some(scan) {
            return None;
        }
        last = Some(scan);
        Some(("trace", with_forces(ctx, &j)))
    })
}

/// Every module's traced scans on one stream, each module's newest as it
/// lands (the same `trace` events as a module's own stream): a screen
/// watching every program holds one stream slot, not one per module.
/// Modules come and go with deploys; each is followed by name.
fn all_traces_stream(stream: &mut Conn, ctx: &Ctx) -> io::Result<()> {
    let mut last: BTreeMap<String, u64> = BTreeMap::new();
    pump_many(stream, ctx, || {
        let names: Vec<String> = ctx.catalog().modules.iter().map(|m| m.name.clone()).collect();
        last.retain(|n, _| names.contains(n));
        let mut out = Vec::new();
        for (k, name) in names.iter().enumerate() {
            let Some((scan, j)) = latest_trace(ctx, k) else {
                continue;
            };
            if last.get(name) == Some(&scan) {
                continue;
            }
            last.insert(name.clone(), scan);
            out.push(("trace", with_forces(ctx, &j)));
        }
        out
    })
}

/// A rendered trace with the active forces beside it, so a viewer marks
/// forced points FORCED.
fn with_forces(ctx: &Ctx, trace: &Json) -> Json {
    let mut j = trace.clone();
    if ctx.forces.is_some() {
        j["forces"] = forces::list(ctx);
    }
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(head: &str) -> Request {
        http::parse_head(head.as_bytes(), &Limits::default()).unwrap().0
    }

    fn location(r: &Response) -> Option<&str> {
        r.headers
            .iter()
            .find(|(n, _)| *n == "Location")
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn plain_http_from_the_lan_is_sent_to_https_and_never_served() {
        let r = to_https(&req("GET /api/modules?x=1 HTTP/1.1\r\nHost: box-1:8080"));
        assert_eq!(r.status, 301);
        assert_eq!(location(&r), Some("https://box-1:8080/api/modules"), "{r:?}");
        let r = to_https(&req("HEAD / HTTP/1.1\r\nHost: [fe80::1]:8080"));
        assert_eq!(location(&r), Some("https://[fe80::1]:8080/"), "{r:?}");
        // A change is refused, not redirected: nothing happens in clear.
        for m in ["POST", "PUT", "DELETE"] {
            let r = to_https(&req(&format!("{m} /api/proposals HTTP/1.1\r\nHost: box")));
            assert_eq!((r.status, location(&r)), (403, None), "{m}");
        }
        // A Host that is not a plain name or address gets no Location.
        for host in ["", "evil.example/x", "box@evil", "box%0d%0a"] {
            let r = to_https(&req(&format!("GET / HTTP/1.1\r\nHost: {host}")));
            assert_eq!((r.status, location(&r)), (403, None), "{host:?}");
        }
        let r = to_https(&req("GET / HTTP/1.1\r\nHost: a\r\nHost: b"));
        assert_eq!(r.status, 403, "two Hosts");
        let r = to_https(&req("GET / HTTP/1.1"));
        assert_eq!(r.status, 403, "no Host");
    }
}
