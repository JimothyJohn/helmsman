//! The box API: one local HTTP/1.1 + JSON surface for the canvas, the
//! agent's tools and (later) MCP. Testable without a UI.
//!
//! ```text
//! GET  /api                               this list
//! GET  /api/signals                       every signal: type, unit, dir, binding, writer, value, quality
//! GET  /api/signals/{name}
//! GET  /api/modules                       every module: schedule, windows, status, stats
//! GET  /api/modules/{name}                + ports, source, manifest
//! GET  /api/modules/{name}/trace          asks for a traced scan; the newest one (200), or 202 until one lands
//! GET  /api/modules/{name}/trace/events   event stream of traced scans
//! GET  /api/outputs                       output signals' commands and safe values, the %Q image
//! GET  /api/devices                       device links: up, down (why), unknown
//! GET  /api/proposals                     drafts awaiting approval
//! GET  /api/proposals/{id}
//! POST /api/proposals                     token: a module draft, a param change, or a new io_map / calendar file, checked, stored as a draft
//! POST /api/proposals/check               token: check a draft as POST /api/proposals would; nothing stored
//! POST /api/proposals/replay              token: sim replay of a draft {draft, minutes}; nothing stored, not evidence
//! POST /api/proposals/{id}/replay         token: sim replay of the draft against what runs; evidence for apply
//! POST /api/proposals/{id}/apply          token + {role, pin, replay?}: apply through the approval policy
//! GET  /api/audit?after=N&limit=M         every mutating call, append-only, naming snapshot ids
//! GET  /api/snapshots                     snapshot history (with --store), head, the running change set's id
//! GET  /api/snapshots/head
//! GET  /api/snapshots/{id}                a snapshot's files
//! GET  /api/snapshots/{a}/diff/{b}
//! GET  /api/snapshots/{id}/bundle         that snapshot as an unsigned bundle file (see [`bundle`])
//! POST /api/bundles                       token: a bundle file, verified, bound by name, stored as a draft
//! POST /api/snapshots/{id}/replay         token: sim replay of a rollback to it
//! POST /api/snapshots/{id}/rollback       token + {role, pin, replay?}: roll back through the approval policy
//! GET  /api/replays/{n}                   a replay kept as evidence
//! GET  /api/policy                        the policy this box enforces
//! GET  /api/events                        event stream of live values and module status
//! POST /api/chat                          token: one message to the agent; its progress as an event stream
//! GET  /api/screen                        the floor screen's layout (with --screen FILE; see [`crate::screen`])
//! GET  /, /{file}                          the canvas (with --ui DIR; see [`ui`])
//! ```
//!
//! **What it cannot do.** There is no route that writes a signal, an
//! output, a param or a module, with or without the token. Proposals are
//! stored drafts; the only way one runs is `apply` (and the only way back
//! is `rollback`), both through [`approve`]: tier, policy, role, PIN and
//! sim evidence, checked by the host ([`crate::policy`]). The token alone
//! (what the agent holds) can draft and replay, never apply. Limits are
//! enforced here, by the host, never by whatever is calling.
//!
//! **Auth.** Reads (`GET`) are open to whoever can reach the socket (the
//! default bind is loopback; the LAN only when asked). Every other method
//! needs `Authorization: Bearer <token>` ([`token`]) and is written to the
//! audit log whatever its outcome; without the token it is refused with
//! 401 before any routing. A token in the query string is refused
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
pub mod http;
pub mod live;
pub mod proposal;
pub mod render;
pub mod sim;
pub mod store;
pub mod swap;
pub mod token;
pub mod ui;

use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as Json, json};

use crate::bundle::TrustStore;
use crate::policy::Role;
use crate::store::{self as snapshots, ChangeSet, SnapshotId, Store};

pub use approve::{Approval, Replays};
pub use catalog::{Catalog, Devices, ModuleInfo, SignalInfo};
pub use http::{Limits, Request, Response};
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
    /// The floor screen's layout, at `/api/screen`.
    pub screen: Option<Screen>,
    /// Publishers whose bundles import (`POST /api/bundles`); `None`:
    /// import answers 501.
    pub trust: Option<TrustStore>,
}

/// A floor screen layout as loaded: the file, byte for byte, and what it
/// parsed to (already checked against the I/O map).
#[derive(Debug, Clone)]
pub struct Screen {
    pub source: String,
    pub layout: crate::screen::Layout,
}

/// `GET /api/screen`: the layout, its source (the canvas parses that with
/// the same parser) and the fixed palette.
fn screen_json(s: &Screen) -> Json {
    let hex = |c: [u8; 3]| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    let palette: serde_json::Map<String, Json> = crate::screen::Colour::ALL
        .iter()
        .map(|c| {
            (
                c.name().to_string(),
                json!({ "fill": hex(c.fill()), "ink": hex(c.ink()) }),
            )
        })
        .collect();
    json!({
        "source": s.source,
        "layout": serde_json::to_value(&s.layout).unwrap_or(Json::Null),
        "palette": palette,
    })
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
    stop: Arc<AtomicBool>,
    streams: AtomicUsize,
    chat: Option<chat::Chat>,
    ui: Option<ui::Ui>,
    /// `GET /api/screen`'s body, rendered once.
    screen: Option<Json>,
    /// The floor screen's layout, which a reloaded I/O map must still fit.
    screen_layout: Option<crate::screen::Layout>,
    /// Publishers whose bundles import; `None`: import answers 501.
    trust: Option<TrustStore>,
}

impl Ctx {
    fn catalog(&self) -> Arc<Catalog> {
        Arc::clone(&self.catalog.read().unwrap_or_else(PoisonError::into_inner))
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
    }

    /// Whether the floor screen (or its absence) fits `map`, as the box
    /// checks at startup: every tile's signals are screen outputs on it and
    /// every screen output is on a tile; without a screen, the map has no
    /// screen outputs.
    fn screen_fits(&self, map: Option<&helmsman_io::IoMap>) -> Result<(), Vec<String>> {
        let empty = helmsman_io::IoMap::default();
        let map = map.unwrap_or(&empty);
        match &self.screen_layout {
            Some(l) => l.check(map),
            None if map
                .signals
                .iter()
                .any(|s| matches!(s.binding, helmsman_io::Binding::Screen { .. })) =>
            {
                Err(vec![
                    "the I/O map declares screen tiles, but this box shows no floor screen (--screen FILE)"
                        .into(),
                ])
            }
            None => Ok(()),
        }
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
        Self::start_with(
            cfg,
            shared,
            Extras {
                ui,
                screen: None,
                trust,
            },
        )
    }

    /// [`Api::start`], with what a box may serve besides its state.
    ///
    /// # Errors
    /// The bind fails.
    pub fn start_with(cfg: Config, mut shared: Shared, extras: Extras) -> io::Result<Self> {
        let Extras { ui, screen, trust } = extras;
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
            box_: shared,
            cfg,
            stop: Arc::clone(&stop),
            streams: AtomicUsize::new(0),
            ui,
            screen_layout: screen.as_ref().map(|s| s.layout.clone()),
            screen: screen.map(|s| screen_json(&s)),
            trust,
        });
        let thread = thread::Builder::new()
            .name("api".into())
            .spawn(move || accept(&listener, &ctx))?;
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

impl Drop for Api {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn accept(listener: &TcpListener, ctx: &Arc<Ctx>) {
    let workers = Arc::new(AtomicUsize::new(0));
    while !ctx.stop.load(Ordering::Acquire) {
        let (mut stream, _) = match listener.accept() {
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
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let _ = http::respond(&mut stream, &Response::error(503, "busy; try again"));
            continue;
        }
        workers.fetch_add(1, Ordering::AcqRel);
        let (ctx, n) = (Arc::clone(ctx), Arc::clone(&workers));
        let spawned = thread::Builder::new().name("api-conn".into()).spawn(move || {
            let _slot = Slot(&n);
            serve(stream, &ctx, accepted);
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
    /// One chat message, its conversation taken out.
    Chat(chat::Taken),
    /// A file of the UI.
    File(ui::Asset),
}

fn serve(mut stream: TcpStream, ctx: &Ctx, accepted: Instant) {
    let limits = ctx.cfg.limits;
    if stream.set_write_timeout(Some(limits.write_timeout)).is_err() {
        return;
    }
    let reply = match http::read_request(&mut stream, &limits, accepted) {
        Ok(req) => handle(&req, ctx),
        Err(http::ReadError::Status(code, why)) => Reply::Once(Response::error(code, why)),
        Err(http::ReadError::Closed) => return,
    };
    let _ = stream.set_read_timeout(None);
    match reply {
        Reply::Once(r) => {
            let _ = http::respond(&mut stream, &r);
        }
        Reply::Chat(taken) => {
            if let Some(chat) = &ctx.chat {
                chat.serve(&mut stream, taken);
            }
        }
        Reply::File(asset) => {
            let _ = ui::send(&mut stream, asset);
        }
        Reply::Values | Reply::Traces(_) => {
            let Some(_slot) = take(&ctx.streams, limits.streams) else {
                let _ = http::respond(&mut stream, &Response::error(503, "too many event streams"));
                return;
            };
            let _ = match reply {
                Reply::Traces(k) => trace_stream(&mut stream, ctx, k),
                Reply::Values => value_stream(&mut stream, ctx),
                Reply::Once(_) | Reply::Chat(_) | Reply::File(_) => Ok(()),
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
    ]
    .contains(&k.as_str())
}

fn handle(req: &Request, ctx: &Ctx) -> Reply {
    if req.query.iter().any(|(k, _)| secret(k)) {
        return Reply::Once(Response::error(
            400,
            "credentials go in the Authorization header, never the URL",
        ));
    }
    if req.method == "GET" {
        return get(req, ctx);
    }
    let authorized = req
        .bearer
        .as_deref()
        .is_some_and(|t| token::eq(t, &ctx.cfg.token));
    if !authorized {
        (ctx.cfg.log)(&format!(
            "api: refused {} {} (no or wrong token)",
            clean(&req.method),
            clean(&req.path)
        ));
        let why = if req.authorization {
            "wrong token"
        } else {
            "this call needs Authorization: Bearer <token>"
        };
        return Reply::Once(Response::error(401, why).with("WWW-Authenticate", "Bearer".into()));
    }
    if req.method == "POST"
        && req.segments.len() == 2
        && req.segments[0] == "api"
        && req.segments[1] == "chat"
    {
        return start_chat(req, ctx);
    }
    Reply::Once(mutate(req, ctx))
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
    match chat.take(&req.body) {
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

fn append(
    ctx: &Ctx,
    req: &Request,
    status: u16,
    detail: &str,
    snapshot: Option<String>,
    base: Option<String>,
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
        ("POST", ["api", "proposals"]) => return create(req, ctx),
        ("POST", ["api", "bundles"]) => return bundle::import(req, ctx),
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
    match path {
        ["api", "proposals"] => Some("GET, POST"),
        ["api", "proposals", _, "apply" | "replay"]
        | ["api", "snapshots", _, "rollback" | "replay"]
        | ["api", "proposals", "check" | "replay"]
        | ["api", "chat" | "bundles"] => Some("POST"),
        ["api"]
        | [
            "api",
            "signals" | "modules" | "outputs" | "devices" | "audit" | "events" | "snapshots" | "policy"
            | "screen",
        ]
        | [
            "api",
            "signals" | "modules" | "proposals" | "snapshots" | "replays",
            _,
        ]
        | ["api", "snapshots", _, "diff", _]
        | ["api", "snapshots", _, "bundle"]
        | ["api", "modules", _, "trace"]
        | ["api", "modules", _, "trace", "events"] => Some("GET"),
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
    match proposal::check(&new, &ctx.catalog(), ctx.cfg.memory, running.as_ref()) {
        Ok((mut p, _)) => {
            p["stored"] = json!(false);
            audited(ctx, req, 200, &p, "check: clean, not stored")
        }
        Err((status, body)) => audited(ctx, req, status, &body, "check: refused by the checks"),
    }
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
    let target = match proposal::check(&r.draft, &ctx.catalog(), ctx.cfg.memory, running.as_ref()) {
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
    let checked = proposal::check(&new, &ctx.catalog(), ctx.cfg.memory, running.as_ref());
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
        (Some(base), Some(cs)) => approve::tier_json(base, cs, ctx.approval.as_ref().map(|a| &a.policy)),
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

fn get(req: &Request, ctx: &Ctx) -> Reply {
    if req.segments.first().is_none_or(|s| s != "api") {
        return match &ctx.ui {
            Some(ui) => match ui.resolve(&req.segments) {
                Ok(asset) => Reply::File(asset),
                Err(r) => Reply::Once(r),
            },
            None => Reply::Once(Response::error(
                404,
                "no such resource (no UI on this box: --ui DIR)",
            )),
        };
    }
    let c = &*ctx.catalog();
    let segs: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    let view = ctx.box_.live.view();
    let module = |name: &str| c.module(name);
    let r = match segs.as_slice() {
        ["api"] => Response::json(200, &index()),
        ["api", "signals"] => Response::json(200, &render::signals(c, &view)),
        ["api", "signals", name] => match c.signal(name) {
            Some(i) => Response::json(
                200,
                &render::signal(
                    &c.signals[i],
                    view.signals.get(i).copied().flatten(),
                    view.errors.get(i).and_then(Option::as_deref),
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
                Some((_, j)) => Response::json(200, &j),
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
        ["api", "screen"] => match &ctx.screen {
            Some(j) => Response::json(200, j),
            None => Response::error(404, "no floor screen on this box (--screen FILE)"),
        },
        ["api", "policy"] => match &ctx.approval {
            Some(a) => Response::json(200, &approve::policy_json(&a.policy)),
            None => Response::error(404, "no approval path on this box"),
        },
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
    store.get(&id).map_err(|e| store_error(&e))
}

fn index() -> Json {
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
            e("GET", "/api/outputs", false, "output commands, safe values, the %Q image"),
            e("GET", "/api/devices", false, "device links up or down"),
            e("GET", "/api/proposals", false, "drafts awaiting approval"),
            e("GET", "/api/proposals/{id}", false, "one proposal"),
            e("POST", "/api/proposals", true, "store a checked draft: {kind: module, manifest, source}, {kind: param, module, param, value}, or {kind: io_map|calendar, text}"),
            e("POST", "/api/proposals/check", true, "check a draft as POST /api/proposals would, storing nothing"),
            e("POST", "/api/proposals/replay", true, "sim replay of a module draft {draft: {kind: module, manifest, source}, minutes}; nothing stored, not evidence"),
            e("POST", "/api/proposals/{id}/replay", true, "sim replay of the draft against what runs (evidence)"),
            e("POST", "/api/proposals/{id}/apply", true, "apply: {role, pin, replay?}, checked against the policy"),
            e("GET", "/api/audit?after=N&limit=M", false, "every mutating call"),
            e("GET", "/api/snapshots", false, "snapshot history (needs --store), head, and the running change set's id"),
            e("GET", "/api/snapshots/head", false, "the head entry"),
            e("GET", "/api/snapshots/{id}", false, "a snapshot's files"),
            e("GET", "/api/snapshots/{a}/diff/{b}", false, "what changed from a to b"),
            e("GET", "/api/snapshots/{id}/bundle", false, "that snapshot as an unsigned bundle file (sign it off the box)"),
            e("POST", "/api/bundles", true, "import a bundle file (application/octet-stream): verified against the box's trusted publishers, bound to its signals by name, stored as a draft"),
            e("POST", "/api/snapshots/{id}/replay", true, "sim replay of a rollback to it (evidence)"),
            e("POST", "/api/snapshots/{id}/rollback", true, "roll back: {role, pin, replay?}, checked against the policy"),
            e("GET", "/api/replays/{n}", false, "a replay kept as evidence"),
            e("GET", "/api/policy", false, "the approval policy this box enforces"),
            e("GET", "/api/events", false, "event stream of live values and module status"),
            e("POST", "/api/chat", true, "one message to the agent {message, session?}; an event stream of its progress"),
            e("GET", "/api/screen", false, "the floor screen's layout: tiles, state codes, colours (with --screen)"),
        ],
        "writes": "no endpoint writes a signal, an output or a module; a proposal runs only through apply, which needs a role PIN the policy allows (and a sim replay for logic and output-affecting changes)",
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
                "base": e.base, "role": e.role,
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
    let fresh = tap.take().map(|s| (s.scan, Arc::new(render::trace(&m.name, &s))));
    let mut traces = ctx.traces.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(f) = fresh {
        traces.insert(m.name.clone(), f);
    }
    traces.get(&m.name).cloned()
}

fn event(stream: &mut TcpStream, name: &str, data: &Json) -> io::Result<()> {
    let body = serde_json::to_string(data).unwrap_or_default();
    write!(stream, "event: {name}\ndata: {body}\n\n")?;
    stream.flush()
}

/// Streams until the client goes, a write stalls past the write timeout,
/// or the API stops. `next` yields what to send, if anything is new.
fn pump(
    stream: &mut TcpStream,
    ctx: &Ctx,
    mut next: impl FnMut() -> Option<(&'static str, Json)>,
) -> io::Result<()> {
    http::start_stream(stream)?;
    stream.write_all(b"retry: 2000\n\n")?;
    let mut quiet = Instant::now();
    let opened = Instant::now();
    while !ctx.stop.load(Ordering::Acquire) && opened.elapsed() < ctx.cfg.limits.stream_life {
        if let Some((name, data)) = next() {
            event(stream, name, &data)?;
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

fn value_stream(stream: &mut TcpStream, ctx: &Ctx) -> io::Result<()> {
    let mut last = None;
    pump(stream, ctx, || {
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
                "signals": render::signals(c, &view),
                "modules": render::modules(c, &view),
                "outputs": view.outputs,
            }),
        ))
    })
}

fn trace_stream(stream: &mut TcpStream, ctx: &Ctx, k: usize) -> io::Result<()> {
    let mut last = None;
    pump(stream, ctx, || {
        let (scan, j) = latest_trace(ctx, k)?;
        if last == Some(scan) {
            return None;
        }
        last = Some(scan);
        Some(("trace", (*j).clone()))
    })
}
