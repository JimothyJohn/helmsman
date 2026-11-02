//! The box as a data source: what the canvas knows about a box it watches
//! over its API, and what it asks for next. No I/O here: the browser's
//! `fetch` and `EventSource` (`web.rs`) carry out [`Cmd`]s and hand back
//! [`Msg`]s, so everything below runs (and is tested) natively, against
//! JSON the real API produced.
//!
//! - `GET /api/modules`: the module picker.
//! - `GET /api/modules/{name}`: the chosen module's source, lowered with
//!   `helmsman-diagram` against the statement table its traces carry.
//! - `/api/modules/{name}/trace/events`: traced scans, rebuilt into a
//!   [`BoxScan`] that the diagram annotates like an in-process trace.
//! - `/api/events`: live values and module status; this stream's health
//!   is the connection status.
//! - `/api/traces/events`, while the Monitor shows: every module's traced
//!   scans on one stream, kept per module in [`BoxState::programs`] with
//!   each module's source (`GET /api/modules/{name}`), lowered.
//!
//! Reconnects work like `EventSource`'s own, but here, so the canvas can
//! say what is happening: a failed stream is closed and reopened after
//! [`backoff`] ([`RETRY_SECS`], the same every time, counted down on the
//! status line), and after [`UNREACHABLE_AFTER`] failures in a row the
//! box is called unreachable (still retrying).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use helmsman_diagram::{Module, Scan, StmtPos, lower_table};
use helmsman_iec::Value;
use helmsman_iec::trace::StmtId;
use serde_json::Value as Json;

/// Failures in a row (of the values stream, or of the module list fetch
/// before anything ever worked) after which the box is "unreachable".
pub const UNREACHABLE_AFTER: u32 = 3;

/// Seconds between retries: a steady 5 s, so the countdown on the status
/// line reads the same every time (Nick, 2026-11-01; it was 2 s doubling
/// to 30, which read as a hang).
pub const RETRY_SECS: f64 = 5.0;

/// Seconds to wait before retry number `failures` (1-based).
#[must_use]
pub fn backoff(_failures: u32) -> f64 {
    RETRY_SECS
}

/// What to fetch once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    Modules,
    Module(String),
    /// A module's source for the Monitor ([`BoxState::programs`]).
    Source(String),
}

/// An event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// `/api/events`: `values` events.
    Values,
    /// `/api/modules/{name}/trace/events`: `trace` events.
    Trace,
    /// `/api/traces/events`: every module's `trace` events (the Monitor).
    Traces,
}

impl Stream {
    /// The SSE event name the stream carries.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[must_use]
    pub const fn event(self) -> &'static str {
        match self {
            Self::Values => "values",
            Self::Trace | Self::Traces => "trace",
        }
    }
}

/// Something for the transport to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// `GET url`; answer with [`Msg::Got`].
    Get(Fetch, String),
    /// Open (replacing any open one) the stream at `url`; answer with
    /// [`Msg::Opened`], [`Msg::Event`]s, and [`Msg::Failed`] once if it
    /// fails, after which the transport has closed it.
    Listen(Stream, String),
    Close(Stream),
}

/// What the transport saw. (Natively only tests make these: the native
/// canvas runs the simulation.)
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A fetch's body (2xx), or why it failed.
    Got(Fetch, Result<String, String>),
    Opened(Stream),
    /// An event's name and data.
    Event(Stream, String, String),
    Failed(Stream),
}

/// How the canvas is doing at reaching the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Link {
    /// Nothing has worked or failed yet.
    Connecting,
    Connected,
    /// It worked, or is trying; the last attempt failed. Retrying at `at`.
    Reconnecting {
        failures: u32,
        at: f64,
    },
    /// [`UNREACHABLE_AFTER`] failures in a row; still retrying at `at`.
    Unreachable {
        failures: u32,
        at: f64,
    },
}

/// A traced scan from the box, as the diagram reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxScan {
    pub module: String,
    pub scan: u64,
    pub now_ms: u64,
    /// The fault that ended the scan, as text.
    pub fault: Option<String>,
    /// Row `k` is `StmtId(k)`, as the box compiled the module.
    pub table: Vec<StmtPos>,
    counts: Vec<Counts>,
    values: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Counts {
    hits: u32,
    arm_hits: Vec<u32>,
    passes: Option<u32>,
}

/// The upper-cased, space-free key [`helmsman_iec::trace::Trace::get`] uses.
fn key(path: &str) -> String {
    path.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// An ST value from the API's JSON: booleans, integers (past 64 bits as
/// decimal strings), reals; `null` (a non-finite real) is no value.
#[must_use]
pub fn st_value(j: &Json) -> Option<Value> {
    match j {
        Json::Bool(b) => Some(Value::Bool(*b)),
        Json::Number(n) => n
            .as_i64()
            .map(|i| Value::Int(i128::from(i)))
            .or_else(|| n.as_u64().map(|u| Value::Int(i128::from(u))))
            .or_else(|| n.as_f64().map(Value::Real)),
        Json::String(s) => s.parse::<i128>().ok().map(Value::Int),
        _ => None,
    }
}

fn u32_of(j: &Json) -> Option<u32> {
    j.as_u64().and_then(|n| u32::try_from(n).ok())
}

impl BoxScan {
    /// A `trace` event's data, or `GET /api/modules/{name}/trace`'s body.
    ///
    /// # Errors
    /// Not the shape the API renders (`api::render::trace`).
    pub fn parse(j: &Json) -> Result<Self, String> {
        let stmts = j["stmts"].as_array().ok_or("trace without stmts")?;
        let mut table = Vec::with_capacity(stmts.len());
        let mut counts = Vec::with_capacity(stmts.len());
        for (k, s) in stmts.iter().enumerate() {
            if s["id"].as_u64() != Some(k as u64) {
                return Err(format!("trace statement {k} out of order"));
            }
            let field =
                |name: &str| u32_of(&s[name]).ok_or_else(|| format!("trace statement {k}: no {name}"));
            table.push(StmtPos {
                pou: s["pou"]
                    .as_str()
                    .ok_or("trace statement without pou")?
                    .to_string(),
                system: s["system"].as_bool().unwrap_or(false),
                line: field("line")?,
                col: field("col")?,
            });
            counts.push(Counts {
                hits: field("hits")?,
                arm_hits: s["arm_hits"]
                    .as_array()
                    .map(|a| a.iter().map(|n| u32_of(n).unwrap_or(0)).collect())
                    .unwrap_or_default(),
                passes: u32_of(&s["passes"]),
            });
        }
        let values = j["values"]
            .as_object()
            .ok_or("trace without values")?
            .iter()
            .filter_map(|(path, v)| Some((key(path), st_value(v)?)))
            .collect();
        Ok(Self {
            module: j["module"].as_str().ok_or("trace without module")?.to_string(),
            scan: j["scan"].as_u64().ok_or("trace without scan")?,
            now_ms: j["now_ms"].as_u64().unwrap_or(0),
            fault: j["fault"]["text"].as_str().map(str::to_string),
            table,
            counts,
            values,
        })
    }

    fn counts(&self, id: StmtId) -> Option<&Counts> {
        self.counts.get(id.0 as usize)
    }
}

impl Scan for BoxScan {
    fn hits(&self, id: StmtId) -> u32 {
        self.counts(id).map_or(0, |c| c.hits)
    }

    fn arm_hits(&self, id: StmtId) -> &[u32] {
        self.counts(id).map_or(&[], |c| c.arm_hits.as_slice())
    }

    fn passes(&self, id: StmtId) -> Option<u32> {
        self.counts(id)?.passes
    }

    fn get(&self, path: &str) -> Option<Value> {
        self.values.get(&key(path)).copied()
    }

    fn line(&self, id: StmtId) -> Option<u32> {
        self.table.get(id.0 as usize).map(|s| s.line)
    }
}

/// A module as the picker lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleRow {
    pub name: String,
    /// `starting`, `running`, `held` or `faulted`.
    pub state: String,
    pub fault: Option<String>,
    pub scans: u64,
    /// The scan period, ms.
    pub scan_ms: Option<f64>,
    /// Deadlines missed because a scan ran late.
    pub overruns: u64,
}

impl ModuleRow {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            name: j["name"].as_str()?.to_string(),
            state: j["status"]["state"].as_str().unwrap_or("?").to_string(),
            fault: j["status"]["fault"]["text"].as_str().map(str::to_string),
            scans: j["stats"]["scans"].as_u64().unwrap_or(0),
            scan_ms: j["scan_ms"].as_f64(),
            overruns: j["stats"]["overruns"].as_u64().unwrap_or(0),
        })
    }
}

/// One module as the Monitor watches it: its newest traced scan, when
/// that came, and its source lowered against the scan's statements.
#[derive(Debug, Default)]
pub struct Watched {
    pub scan: Option<BoxScan>,
    /// Page time the newest scan arrived, s.
    pub at: Option<f64>,
    pub lowered: Option<Lowered>,
    /// Why it cannot be drawn.
    pub problem: Option<String>,
    source: Option<String>,
    /// The source fetch is in flight.
    asking: bool,
    fetched_for: Option<Vec<StmtPos>>,
    detail: Retry,
}

impl Watched {
    /// Lowers the source against the newest scan when either is new;
    /// asks for the source (again, if the program changed under it).
    fn relower(&mut self, name: &str, generation: &mut u64, queue: &mut Vec<Cmd>) {
        let Some(scan) = self.scan.as_ref() else {
            return;
        };
        if self.lowered.as_ref().is_some_and(|l| l.table == scan.table) {
            return;
        }
        let stale = self.fetched_for.as_ref().is_some_and(|t| *t != scan.table);
        if self.source.is_none() || stale {
            if stale {
                self.source = None;
            }
            if !self.asking && self.detail.at.is_none() {
                self.asking = true;
                self.fetched_for = Some(scan.table.clone());
                queue.push(Cmd::Get(Fetch::Source(name.into()), module_url(name)));
            }
            return;
        }
        let Some(src) = self.source.clone() else {
            return;
        };
        if src.trim_start().starts_with('<') {
            self.problem = Some("a PLCopen XML project; the Monitor reads Structured Text".into());
            return;
        }
        match lower_table(&src, &scan.table) {
            Ok(module) => {
                *generation += 1;
                self.problem = None;
                self.lowered = Some(Lowered {
                    src,
                    module,
                    table: scan.table.clone(),
                    generation: *generation,
                });
            }
            Err(e) => {
                self.lowered = None;
                self.problem = Some(format!("cannot read {name}: {e}"));
            }
        }
    }
}

/// A signal with its live value.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalRow {
    pub name: String,
    /// `in` or `out`.
    pub dir: String,
    pub unit: Option<String>,
    pub value: Option<Value>,
    /// `good`, `missing` or `commanded`.
    pub quality: String,
    pub point_error: Option<String>,
    /// An engineer forced it (the box's `forced`).
    pub forced: bool,
}

impl SignalRow {
    pub(crate) fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            name: j["name"].as_str()?.to_string(),
            dir: j["dir"].as_str().unwrap_or("?").to_string(),
            unit: j["unit"].as_str().map(str::to_string),
            value: st_value(&j["value"]),
            quality: j["quality"].as_str().unwrap_or("?").to_string(),
            point_error: j["point_error"].as_str().map(str::to_string),
            forced: j["forced"].as_bool().unwrap_or(false),
        })
    }
}

/// A forced input as the box lists it (`GET /api/forces`, the values
/// stream's `forces`).
#[derive(Debug, Clone, PartialEq)]
pub struct ForceRow {
    pub id: u64,
    /// The input signal, or `None` for an address force.
    pub signal: Option<String>,
    /// The input image address (`%IX0.0`), for an address force.
    pub address: Option<String>,
    pub value: Option<Value>,
    /// Who forced it: `alice (engineer)`.
    pub by: String,
}

impl ForceRow {
    pub(crate) fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            id: j["id"].as_u64()?,
            signal: j["signal"].as_str().map(str::to_string),
            address: j["address"].as_str().map(str::to_string),
            value: st_value(&j["value"]),
            by: j["by"].as_str().unwrap_or("?").to_string(),
        })
    }

    /// What is forced: the signal's name or the address.
    #[must_use]
    pub fn point(&self) -> &str {
        self.signal.as_deref().or(self.address.as_deref()).unwrap_or("?")
    }
}

pub(crate) fn rows<T>(j: &Json, parse: impl Fn(&Json) -> Option<T>) -> Option<Vec<T>> {
    j.as_array().map(|a| a.iter().filter_map(parse).collect())
}

/// The chosen module, lowered.
#[derive(Debug, Clone)]
pub struct Lowered {
    pub src: String,
    pub module: Module,
    table: Vec<StmtPos>,
    /// Bumped on every new lowering, so a view knows to rebuild.
    pub generation: u64,
}

/// One retried thing: failures in a row and when to try again.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Retry {
    pub failures: u32,
    pub at: Option<f64>,
}

impl Retry {
    pub(crate) fn failed(&mut self, now: f64) {
        self.failures += 1;
        self.at = Some(now + backoff(self.failures));
    }

    pub(crate) fn ok(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn due(&mut self, now: f64) -> bool {
        if self.at.is_some_and(|t| now >= t) {
            self.at = None;
            return true;
        }
        false
    }
}

/// Percent-encodes a name for one path segment.
#[must_use]
pub fn segment(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Everything the canvas knows about the box it watches.
#[derive(Debug)]
pub struct BoxState {
    pub modules: Vec<ModuleRow>,
    pub selected: Option<String>,
    /// The selected module's source, once fetched.
    source: Option<String>,
    pub lowered: Option<Lowered>,
    /// Why the selected module cannot be drawn.
    pub problem: Option<String>,
    pub scan: Option<BoxScan>,
    pub signals: Vec<SignalRow>,
    /// The box's forced inputs, as the values stream last said.
    pub forces: Vec<ForceRow>,
    /// The values stream's last `seq`.
    pub seq: Option<u64>,
    /// Page time the last `values` event arrived, s.
    pub values_at: Option<f64>,
    /// The box's clock at the newest values event (`at_ms`).
    pub values_at_ms: Option<u64>,
    /// Every module, as the Monitor watches it (while it shows).
    pub programs: BTreeMap<String, Watched>,
    /// The Monitor shows: every module's traces are streamed.
    monitor: bool,
    traces: Retry,
    connected: bool,
    ever_connected: bool,
    values: Retry,
    trace: Retry,
    list: Retry,
    detail: Retry,
    /// The statement table the source was last fetched for.
    fetched_for: Option<Vec<StmtPos>>,
    generation: u64,
    queue: Vec<Cmd>,
}

impl Default for BoxState {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxState {
    /// Starts watching: the module list and the values stream.
    #[must_use]
    pub fn new() -> Self {
        Self {
            modules: Vec::new(),
            selected: None,
            source: None,
            lowered: None,
            problem: None,
            scan: None,
            signals: Vec::new(),
            forces: Vec::new(),
            seq: None,
            values_at: None,
            values_at_ms: None,
            programs: BTreeMap::new(),
            monitor: false,
            traces: Retry::default(),
            connected: false,
            ever_connected: false,
            values: Retry::default(),
            trace: Retry::default(),
            list: Retry::default(),
            detail: Retry::default(),
            fetched_for: None,
            generation: 0,
            queue: vec![
                Cmd::Get(Fetch::Modules, "/api/modules".into()),
                Cmd::Listen(Stream::Values, "/api/events".into()),
            ],
        }
    }

    /// What to do now: queued commands and retries that are due.
    pub fn poll(&mut self, now: f64) -> Vec<Cmd> {
        if self.values.due(now) {
            self.queue.push(Cmd::Listen(Stream::Values, "/api/events".into()));
        }
        if self.list.due(now) {
            self.queue.push(Cmd::Get(Fetch::Modules, "/api/modules".into()));
        }
        if self.monitor && self.traces.due(now) {
            self.queue.push(Cmd::Listen(Stream::Traces, TRACES_URL.into()));
        }
        let mut generation = self.generation;
        for (name, w) in &mut self.programs {
            if w.detail.due(now) {
                w.relower(name, &mut generation, &mut self.queue);
            }
        }
        self.generation = generation;
        if let Some(name) = self.selected.clone() {
            if !self.monitor && self.trace.due(now) {
                self.queue.push(Cmd::Listen(Stream::Trace, trace_url(&name)));
            }
            if self.detail.due(now) {
                self.queue
                    .push(Cmd::Get(Fetch::Module(name.clone()), module_url(&name)));
            }
        }
        std::mem::take(&mut self.queue)
    }

    /// The soonest retry, for scheduling a repaint.
    #[must_use]
    pub fn next_retry(&self) -> Option<f64> {
        [self.values, self.trace, self.list, self.detail, self.traces]
            .iter()
            .chain(self.programs.values().map(|w| &w.detail))
            .filter_map(|r| r.at)
            .reduce(f64::min)
    }

    /// Whether this box ever answered: a page that never reached one is
    /// probably not served by a box at all.
    #[must_use]
    pub const fn ever_connected(&self) -> bool {
        self.ever_connected
    }

    #[must_use]
    pub fn link(&self) -> Link {
        let r = if self.ever_connected || self.values.failures > 0 {
            self.values
        } else {
            // Nothing streamed yet: a failing module list says as much.
            Retry {
                failures: self.values.failures.max(self.list.failures),
                at: self.values.at.or(self.list.at),
            }
        };
        match (self.connected, r.failures) {
            (true, _) => Link::Connected,
            (false, 0) => Link::Connecting,
            (false, n) if n >= UNREACHABLE_AFTER => Link::Unreachable {
                failures: n,
                at: r.at.unwrap_or(0.0),
            },
            (false, n) => Link::Reconnecting {
                failures: n,
                at: r.at.unwrap_or(0.0),
            },
        }
    }

    /// Whether the Monitor shows. On: every module's traces on one stream
    /// (the selected module's own stream closes, a slot the box has few
    /// of); off: back to the selected module's.
    pub fn set_monitor(&mut self, on: bool) {
        if self.monitor == on {
            return;
        }
        self.monitor = on;
        if on {
            self.traces.ok();
            self.queue.push(Cmd::Close(Stream::Trace));
            self.queue.push(Cmd::Listen(Stream::Traces, TRACES_URL.into()));
        } else {
            self.queue.push(Cmd::Close(Stream::Traces));
            if let Some(name) = self.selected.clone() {
                self.trace.ok();
                self.queue.push(Cmd::Listen(Stream::Trace, trace_url(&name)));
            }
        }
    }

    /// Forgets watched modules the box no longer runs.
    fn prune(&mut self) {
        let names: Vec<&str> = self.modules.iter().map(|m| m.name.as_str()).collect();
        self.programs.retain(|n, _| names.contains(&n.as_str()));
    }

    /// Reads the module list again (after a deploy changed it).
    pub fn relist(&mut self) {
        self.queue.push(Cmd::Get(Fetch::Modules, "/api/modules".into()));
    }

    /// Watch `name` instead.
    pub fn select(&mut self, name: &str) {
        if self.selected.as_deref() == Some(name) {
            return;
        }
        self.selected = Some(name.to_string());
        self.source = None;
        self.lowered = None;
        self.problem = None;
        self.scan = None;
        self.fetched_for = None;
        self.trace.ok();
        self.detail.ok();
        self.queue.push(Cmd::Close(Stream::Trace));
        self.queue
            .push(Cmd::Get(Fetch::Module(name.into()), module_url(name)));
        if !self.monitor {
            self.queue.push(Cmd::Listen(Stream::Trace, trace_url(name)));
        }
    }

    /// Takes in what the transport saw at page time `now` (seconds).
    pub fn apply(&mut self, msg: Msg, now: f64) {
        match msg {
            Msg::Got(Fetch::Modules, Ok(body)) => {
                match parse(&body).as_ref().and_then(|j| rows(j, ModuleRow::parse)) {
                    Some(list) => {
                        self.list.ok();
                        self.modules = list;
                        self.prune();
                        let keep = self
                            .selected
                            .as_ref()
                            .is_some_and(|s| self.modules.iter().any(|m| &m.name == s));
                        if !keep && let Some(first) = self.modules.first().map(|m| m.name.clone()) {
                            self.select(&first);
                        }
                    }
                    None => self.list.failed(now),
                }
            }
            Msg::Got(Fetch::Modules, Err(_)) => self.list.failed(now),
            Msg::Got(Fetch::Source(name), r) => {
                let Some(w) = self.programs.get_mut(&name) else {
                    return;
                };
                w.asking = false;
                let src = r
                    .ok()
                    .as_deref()
                    .and_then(parse)
                    .and_then(|j| j["source"].as_str().map(str::to_string));
                if let Some(src) = src {
                    w.detail.ok();
                    w.source = Some(src);
                    let mut generation = self.generation;
                    w.relower(&name, &mut generation, &mut self.queue);
                    self.generation = generation;
                } else {
                    w.fetched_for = None;
                    w.detail.failed(now);
                }
            }
            Msg::Got(Fetch::Module(name), r) => {
                if self.selected.as_deref() != Some(&name) {
                    return;
                }
                let src = r
                    .ok()
                    .as_deref()
                    .and_then(parse)
                    .and_then(|j| j["source"].as_str().map(str::to_string));
                match src {
                    Some(src) => {
                        self.detail.ok();
                        self.source = Some(src);
                        self.relower();
                    }
                    None => self.detail.failed(now),
                }
            }
            Msg::Opened(Stream::Values) => {
                self.values.ok();
                self.connected = true;
                self.ever_connected = true;
            }
            Msg::Opened(Stream::Trace) => self.trace.ok(),
            Msg::Opened(Stream::Traces) => self.traces.ok(),
            Msg::Failed(s) => {
                let r = match s {
                    Stream::Values => {
                        self.connected = false;
                        &mut self.values
                    }
                    Stream::Trace => &mut self.trace,
                    Stream::Traces => &mut self.traces,
                };
                r.failed(now);
            }
            Msg::Event(Stream::Values, name, data) if name == "values" => {
                if let Some(j) = parse(&data) {
                    self.values.ok();
                    self.connected = true;
                    self.ever_connected = true;
                    self.seq = j["seq"].as_u64();
                    self.values_at = Some(now);
                    self.values_at_ms = j["at_ms"].as_u64();
                    if let Some(s) = rows(&j["signals"], SignalRow::parse) {
                        self.signals = s;
                    }
                    if let Some(m) = rows(&j["modules"], ModuleRow::parse) {
                        self.modules = m;
                        self.prune();
                    }
                    if let Some(f) = rows(&j["forces"], ForceRow::parse) {
                        self.forces = f;
                    }
                }
            }
            Msg::Event(Stream::Trace, name, data) if name == "trace" => {
                let Some(scan) = parse(&data).and_then(|j| BoxScan::parse(&j).ok()) else {
                    return;
                };
                if self.selected.as_deref() != Some(scan.module.as_str()) {
                    return;
                }
                self.scan = Some(scan);
                self.relower();
            }
            Msg::Event(Stream::Traces, name, data) if name == "trace" => {
                let Some(scan) = parse(&data).and_then(|j| BoxScan::parse(&j).ok()) else {
                    return;
                };
                let name = scan.module.clone();
                // A module the list does not name yet is kept: the list
                // catches up, and prunes it if it is really gone.
                let w = self.programs.entry(name.clone()).or_default();
                w.scan = Some(scan);
                w.at = Some(now);
                let mut generation = self.generation;
                w.relower(&name, &mut generation, &mut self.queue);
                self.generation = generation;
            }
            // Any other stream's events are not this view's.
            Msg::Event(..) => {}
        }
    }

    /// Lowers the source against the newest scan's table, when either is
    /// new; fetches the source again if the table changed under it.
    fn relower(&mut self) {
        let (Some(name), Some(scan)) = (self.selected.clone(), self.scan.as_ref()) else {
            return;
        };
        if self.lowered.as_ref().is_some_and(|l| l.table == scan.table) {
            return;
        }
        let Some(src) = self.source.clone() else {
            return;
        };
        match self.fetched_for.as_ref() {
            None => self.fetched_for = Some(scan.table.clone()),
            Some(t) if *t != scan.table => {
                // A different program than the source we hold: fetch it again.
                self.fetched_for = Some(scan.table.clone());
                self.source = None;
                self.queue
                    .push(Cmd::Get(Fetch::Module(name.clone()), module_url(&name)));
                return;
            }
            Some(_) => {}
        }
        if src.trim_start().starts_with('<') {
            self.problem =
                Some("this module is a PLCopen XML project; the live view draws Structured Text".into());
            return;
        }
        match lower_table(&src, &scan.table) {
            Ok(module) => {
                self.generation += 1;
                self.problem = None;
                self.lowered = Some(Lowered {
                    src,
                    module,
                    table: scan.table.clone(),
                    generation: self.generation,
                });
            }
            Err(e) => {
                self.lowered = None;
                self.problem = Some(format!("cannot draw {name}: {e}"));
            }
        }
    }
}

pub(crate) fn parse(body: &str) -> Option<Json> {
    serde_json::from_str(body).ok()
}

#[must_use]
pub fn module_url(name: &str) -> String {
    format!("/api/modules/{}", segment(name))
}

/// Every module's traces, one stream (the Monitor's).
pub const TRACES_URL: &str = "/api/traces/events";

#[must_use]
pub fn trace_url(name: &str) -> String {
    format!("/api/modules/{}/trace/events", segment(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retry_wait_is_the_same_every_time() {
        let d: Vec<f64> = (1..=7).map(backoff).collect();
        assert_eq!(d, [5.0; 7]);
        assert!((backoff(u32::MAX) - RETRY_SECS).abs() < f64::EPSILON);
    }

    #[test]
    fn names_are_encoded_into_one_segment() {
        assert_eq!(module_url("line_3"), "/api/modules/line_3");
        assert_eq!(trace_url("a/../b c"), "/api/modules/a%2F..%2Fb%20c/trace/events");
        assert_eq!(module_url("é"), "/api/modules/%C3%A9");
    }

    #[test]
    fn values_parse_like_the_api_renders_them() {
        assert_eq!(st_value(&serde_json::json!(true)), Some(Value::Bool(true)));
        assert_eq!(st_value(&serde_json::json!(-3)), Some(Value::Int(-3)));
        assert_eq!(
            st_value(&serde_json::json!(u64::MAX)),
            Some(Value::Int(i128::from(u64::MAX)))
        );
        assert_eq!(
            st_value(&serde_json::json!("170141183460469231731687303715884105727")),
            Some(Value::Int(i128::MAX))
        );
        assert_eq!(st_value(&serde_json::json!(1.0)), Some(Value::Real(1.0)));
        assert_eq!(st_value(&Json::Null), None);
        assert_eq!(st_value(&serde_json::json!("x")), None);
        assert_eq!(st_value(&serde_json::json!([1])), None);
    }

    #[test]
    fn malformed_traces_are_refused_not_half_read() {
        for bad in [
            serde_json::json!({}),
            serde_json::json!({ "stmts": [], "module": "m", "scan": 1 }),
            serde_json::json!({ "stmts": [{ "id": 1, "pou": "P", "line": 1, "col": 1, "hits": 0 }], "values": {}, "module": "m", "scan": 1 }),
            serde_json::json!({ "stmts": [{ "id": 0, "pou": "P", "line": -1, "col": 1, "hits": 0 }], "values": {}, "module": "m", "scan": 1 }),
            serde_json::json!({ "stmts": [{ "id": 0, "pou": "P", "line": 1, "col": 1, "hits": 1e10 }], "values": {}, "module": "m", "scan": 1 }),
            serde_json::json!({ "stmts": [], "values": {}, "scan": 1 }),
        ] {
            assert!(BoxScan::parse(&bad).is_err(), "{bad}");
        }
    }

    fn failed_values(s: &mut BoxState, n: u32, mut now: f64) -> f64 {
        for _ in 0..n {
            s.apply(Msg::Failed(Stream::Values), now);
            now += 100.0;
            assert!(
                s.poll(now)
                    .contains(&Cmd::Listen(Stream::Values, "/api/events".into()))
            );
        }
        now
    }

    #[test]
    fn the_link_goes_connecting_connected_reconnecting_unreachable_and_back() {
        let mut s = BoxState::new();
        let first = s.poll(0.0);
        assert_eq!(
            first,
            [
                Cmd::Get(Fetch::Modules, "/api/modules".into()),
                Cmd::Listen(Stream::Values, "/api/events".into())
            ]
        );
        assert_eq!(s.link(), Link::Connecting);
        assert_eq!(s.poll(0.0), [], "nothing twice");
        s.apply(Msg::Opened(Stream::Values), 0.1);
        assert_eq!(s.link(), Link::Connected);

        s.apply(Msg::Failed(Stream::Values), 10.0);
        assert_eq!(
            s.link(),
            Link::Reconnecting {
                failures: 1,
                at: 15.0
            }
        );
        assert_eq!(s.poll(14.9), [], "not before the backoff");
        assert_eq!(s.next_retry(), Some(15.0));
        assert_eq!(s.poll(15.0), [Cmd::Listen(Stream::Values, "/api/events".into())]);
        assert_eq!(s.next_retry(), None);

        let now = failed_values(&mut s, 2, 13.0);
        assert!(
            matches!(s.link(), Link::Unreachable { failures: 3, .. }),
            "{:?}",
            s.link()
        );
        s.apply(
            Msg::Event(Stream::Values, "values".into(), r#"{"seq":9}"#.into()),
            now,
        );
        assert_eq!(s.link(), Link::Connected, "an event is as good as an open");
        assert_eq!(s.seq, Some(9));
        s.apply(Msg::Failed(Stream::Values), now);
        assert!(
            matches!(s.link(), Link::Reconnecting { failures: 1, .. }),
            "the count starts over"
        );
    }

    #[test]
    fn a_box_that_never_answers_is_unreachable() {
        let mut s = BoxState::new();
        s.poll(0.0);
        for k in 0..3 {
            s.apply(
                Msg::Got(Fetch::Modules, Err("network".into())),
                f64::from(k) * 100.0,
            );
            let cmds = s.poll(f64::from(k) * 100.0 + 99.0);
            assert!(
                cmds.contains(&Cmd::Get(Fetch::Modules, "/api/modules".into())),
                "{cmds:?}"
            );
        }
        assert!(
            matches!(s.link(), Link::Unreachable { failures: 3, .. }),
            "{:?}",
            s.link()
        );
        // Garbage instead of JSON counts as a failure too.
        s.apply(Msg::Got(Fetch::Modules, Ok("<html>".into())), 400.0);
        assert!(
            matches!(s.link(), Link::Unreachable { failures: 4, .. }),
            "{:?}",
            s.link()
        );
    }

    #[test]
    fn stale_answers_for_another_module_are_ignored() {
        let mut s = BoxState::new();
        s.apply(
            Msg::Got(Fetch::Modules, Ok(r#"[{"name":"a"},{"name":"b"}]"#.into())),
            0.0,
        );
        assert_eq!(s.selected.as_deref(), Some("a"), "the first module is chosen");
        s.select("b");
        let cmds = s.poll(0.0);
        assert!(cmds.contains(&Cmd::Close(Stream::Trace)), "{cmds:?}");
        assert!(
            cmds.contains(&Cmd::Listen(Stream::Trace, trace_url("b"))),
            "{cmds:?}"
        );
        s.apply(
            Msg::Got(
                Fetch::Module("a".into()),
                Ok(r#"{"source":"PROGRAM a END_PROGRAM"}"#.into()),
            ),
            0.0,
        );
        assert_eq!(s.source, None);
        let trace = r#"{"module":"a","scan":1,"stmts":[],"values":{}}"#;
        s.apply(Msg::Event(Stream::Trace, "trace".into(), trace.into()), 0.0);
        assert_eq!(s.scan, None);
        // The list refreshing keeps the choice.
        s.apply(
            Msg::Got(Fetch::Modules, Ok(r#"[{"name":"a"},{"name":"b"}]"#.into())),
            1.0,
        );
        assert_eq!(s.selected.as_deref(), Some("b"));
    }
}
