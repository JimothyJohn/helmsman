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
//! box is called unreachable (still retrying). Two cases are not
//! failures: the box ends every stream at 600 s by design, so a stream
//! that was delivering is reopened at once and the link stays connected;
//! and a stream that delivers nothing for [`SILENT_AFTER`] seconds (a
//! half-open connection raises no error) is closed and opened again, which
//! does count.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use helmsman_diagram::{Module, Scan, StmtPos, lower_table};
use helmsman_iec::Value;
use helmsman_iec::trace::StmtId;
use serde_json::Value as Json;

/// Failures in a row (of the values stream, or of the module list fetch
/// before anything ever worked) after which the box is "unreachable".
pub const UNREACHABLE_AFTER: u32 = 3;

/// Seconds without a values event after which the stream is taken for
/// dead and opened again. The box publishes every 100 ms, and the Monitor
/// greys out at [`crate::monitor::STALE_AFTER`], so this is later than
/// that: the engineer is told before the reconnect.
pub const SILENT_AFTER: f64 = 5.0;

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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Fetch {
    Modules,
    Module(String),
    /// A module's source for the Monitor ([`BoxState::programs`]).
    Source(String),
    /// `GET /api/devices`: each device link's state (the Devices tab).
    Devices,
    /// `GET /api/io_map`: the map the box runs (the Devices tab, when
    /// no project's own map is open).
    IoMap,
}

/// How often the Devices tab's link states are asked for, s.
const DEVICES_EVERY: f64 = 5.0;

/// Where the Devices tab's two reads stand.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DevicesPoll {
    /// Never asked (`None`), or last answered at this page time.
    Answered(Option<f64>),
    /// Asked; whether the devices and the map are still on their way.
    /// The next round waits for both, so a slow link never has two of
    /// either in flight (card canvas-slow-link).
    Asking { devices: bool, io_map: bool },
}

impl DevicesPoll {
    /// One of the two answered at `now`.
    fn answered(&mut self, devices: bool, now: f64) {
        if let Self::Asking {
            devices: d,
            io_map: m,
        } = self
        {
            if devices {
                *d = false;
            } else {
                *m = false;
            }
            if !*d && !*m {
                *self = Self::Answered(Some(now));
            }
        }
    }
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
    /// The SSE event names the stream carries: the values stream also
    /// pushes the agent's state (`agent`) when it changes.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[must_use]
    pub const fn events(self) -> &'static [&'static str] {
        match self {
            Self::Values => &["values", "agent"],
            Self::Trace | Self::Traces => &["trace"],
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
    /// The box's build stamp, as a reply's [`crate::stamp::HEADER`] said.
    Build(String),
}

/// What the transport has heard and the canvas has not taken yet. A
/// page that is not drawing (a hidden tab) takes nothing for a while, and
/// the box sends ten values events a second, so the newest of the events
/// that replace each other is all that is kept: the values (each is the
/// whole picture), the agent's state, and each module's trace. Messages
/// that mean something in sequence (opened, failed, fetches) all stay, and
/// an event is only replaced by a newer one of its stream that follows the
/// same open.
#[derive(Debug, Default)]
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub struct Inbox {
    msgs: Vec<Msg>,
}

/// What a later event of a stream makes redundant.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
enum Replaces {
    Values,
    Agent,
    Trace(String),
}

#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
impl Replaces {
    fn of(stream: Stream, name: &str, data: &str) -> Option<Self> {
        #[derive(serde::Deserialize)]
        struct Module {
            module: String,
        }
        match (stream, name) {
            (Stream::Values, "values") => Some(Self::Values),
            (Stream::Values, "agent") => Some(Self::Agent),
            (Stream::Trace | Stream::Traces, "trace") => serde_json::from_str::<Module>(data)
                .ok()
                .map(|m| Self::Trace(m.module)),
            _ => None,
        }
    }
}

#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
impl Inbox {
    pub fn push(&mut self, msg: Msg) {
        let key = match &msg {
            Msg::Event(s, n, d) => Replaces::of(*s, n, d).map(|k| (*s, k)),
            _ => None,
        };
        if let Some((stream, key)) = key {
            for old in self.msgs.iter_mut().rev() {
                match old {
                    // An open or a failure of the stream is a boundary.
                    Msg::Opened(s) | Msg::Failed(s) if *s == stream => break,
                    Msg::Event(s, n, d) if *s == stream && Replaces::of(*s, n, d).as_ref() == Some(&key) => {
                        *old = msg;
                        return;
                    }
                    _ => {}
                }
            }
        }
        self.msgs.push(msg);
    }

    /// What arrived since the last call.
    pub fn take(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.msgs)
    }
}

/// Which reply to a fetch is the newest asked for: a slow, old reply must
/// not overwrite a newer one that already landed.
#[derive(Debug, Default)]
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
pub struct Gets {
    issued: HashMap<Fetch, u64>,
    applied: HashMap<Fetch, u64>,
    next: u64,
}

#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
impl Gets {
    /// A number for a fetch being sent.
    pub fn issue(&mut self, what: &Fetch) -> u64 {
        self.next += 1;
        self.issued.insert(what.clone(), self.next);
        self.next
    }

    /// Whether the reply to fetch number `n` should be applied: not if a
    /// newer reply of the same kind already was.
    pub fn accept(&mut self, what: &Fetch, n: u64) -> bool {
        if self.applied.get(what).is_some_and(|&a| a > n) {
            return false;
        }
        self.applied.insert(what.clone(), n);
        true
    }
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
    /// `starting`, `running`, `held`, `faulted` or `stopped` (by an
    /// engineer, card monitor-box-run-stop).
    pub state: String,
    pub fault: Option<String>,
    /// Who stopped it, when it is stopped.
    pub stopped_by: Option<String>,
    pub scans: u64,
    /// The scan period, ms.
    pub scan_ms: Option<f64>,
    /// Deadlines missed because a scan ran late.
    pub overruns: u64,
    /// How long before the box's last publication it last overran, s;
    /// `None`: never, or a box too old to say.
    pub overrun_ago: Option<f64>,
    /// Scan load over the newest scans; `None` before the first scan, or
    /// from a box too old to measure it.
    pub timing: Option<ScanTiming>,
}

/// Smallest, mean, largest and 99th percentile of a set of times, µs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpreadUs {
    pub min: f64,
    pub mean: f64,
    pub max: f64,
    pub p99: f64,
}

impl SpreadUs {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            min: j["min"].as_f64()?,
            mean: j["mean"].as_f64()?,
            max: j["max"].as_f64()?,
            p99: j["p99"].as_f64()?,
        })
    }
}

/// A module's scan load as the box measured it (`timing` on
/// `/api/modules`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ScanTiming {
    /// Scans the numbers cover: the module's newest, up to 1000.
    pub window: u64,
    /// How long each scan ran.
    pub exec: SpreadUs,
    /// How late each scan started after its deadline.
    pub late: SpreadUs,
}

impl ScanTiming {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            window: j["window"].as_u64().filter(|&n| n > 0)?,
            exec: SpreadUs::parse(&j["exec_us"])?,
            late: SpreadUs::parse(&j["late_us"])?,
        })
    }
}

impl ModuleRow {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            name: j["name"].as_str()?.to_string(),
            state: j["status"]["state"].as_str().unwrap_or("?").to_string(),
            fault: j["status"]["fault"]["text"].as_str().map(str::to_string),
            stopped_by: j["status"]["stopped_by"].as_str().map(str::to_string),
            scans: j["stats"]["scans"].as_u64().unwrap_or(0),
            scan_ms: j["scan_ms"].as_f64(),
            overruns: j["stats"]["overruns"].as_u64().unwrap_or(0),
            overrun_ago: j["stats"]["last_overrun_ms_ago"].as_f64().map(|ms| ms / 1000.0),
            timing: ScanTiming::parse(&j["timing"]),
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
    /// Asks for the source again (after a deploy), unless it is on its way.
    fn refetch(&mut self, name: &str, queue: &mut Vec<Cmd>) {
        if !self.asking && self.scan.is_some() {
            self.asking = true;
            queue.push(Cmd::Get(Fetch::Source(name.into()), module_url(name)));
        }
    }

    /// Lowers the source against the newest scan when either is new;
    /// asks for the source (again, if the program changed under it).
    fn relower(&mut self, name: &str, generation: &mut u64, queue: &mut Vec<Cmd>) {
        let Some(scan) = self.scan.as_ref() else {
            return;
        };
        // What was lowered against other statements reads these values
        // wrongly: gone until the source for these arrives.
        if self.lowered.as_ref().is_some_and(|l| l.table != scan.table) {
            self.lowered = None;
        }
        if self
            .lowered
            .as_ref()
            .is_some_and(|l| self.source.as_deref().is_none_or(|s| s == l.src))
        {
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

/// A device link as the box lists it (`GET /api/devices`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub name: String,
    /// `up`, `down` or `unknown`.
    pub state: String,
    /// Why it is down.
    pub error: Option<String>,
    /// Where the box found it (a `mac=` or `cip=` endpoint resolved).
    pub address: Option<String>,
}

impl DeviceRow {
    pub(crate) fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            name: j["name"].as_str()?.to_string(),
            state: j["state"].as_str().unwrap_or("unknown").to_string(),
            error: j["error"].as_str().map(str::to_string),
            address: j["address"].as_str().map(str::to_string),
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
// Each flag is its own fact (what is on show, what is old, what is open).
#[allow(clippy::struct_excessive_bools)]
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
    /// Each device link's state, as last listed.
    pub devices: Vec<DeviceRow>,
    /// The map the box runs, as last read (`None` until read, or when the
    /// box runs none).
    pub io_map: Option<String>,
    /// The last read of each failed, so what shows is old.
    devices_stale: bool,
    io_map_stale: bool,
    devs: DevicesPoll,
    /// The values stream's last `seq`.
    pub seq: Option<u64>,
    /// Page time the last `values` event arrived, s.
    pub values_at: Option<f64>,
    /// The box's clock at the newest values event (`at_ms`).
    pub values_at_ms: Option<u64>,
    /// Counts the box's restarts as the values stream shows them (its
    /// clock and `seq` start again): a series on the old clock is void.
    epoch: u64,
    /// The agent's state, as the values stream's `agent` event last said,
    /// until the view takes it.
    agent: Option<Json>,
    /// The box's build stamp, as its newest reply named it (card
    /// build-match); `None` until one has.
    pub build: Option<String>,
    /// Every module, as the Monitor watches it (while it shows).
    pub programs: BTreeMap<String, Watched>,
    /// The Monitor shows: every module's traces are streamed.
    monitor: bool,
    traces: Retry,
    connected: bool,
    ever_connected: bool,
    /// The current values stream has delivered an event.
    streaming: bool,
    /// Page time of the last sign of life of the values stream (it opened,
    /// or delivered); `None` while it is down or being opened (the browser
    /// times a connect out by itself).
    heard: Option<f64>,
    /// Nobody is looking at the box: its streams are closed.
    paused: bool,
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
            devices: Vec::new(),
            io_map: None,
            devices_stale: false,
            io_map_stale: false,
            devs: DevicesPoll::Answered(None),
            seq: None,
            values_at: None,
            values_at_ms: None,
            epoch: 0,
            agent: None,
            build: None,
            programs: BTreeMap::new(),
            monitor: false,
            traces: Retry::default(),
            connected: false,
            ever_connected: false,
            streaming: false,
            heard: None,
            paused: false,
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
        if self.paused {
            // Closing is all that is left to do for a box nobody watches.
            self.queue.retain(|c| matches!(c, Cmd::Close(_)));
            return std::mem::take(&mut self.queue);
        }
        if self.values.due(now) {
            self.queue.push(Cmd::Listen(Stream::Values, "/api/events".into()));
        }
        if self.list.due(now) {
            self.queue.push(Cmd::Get(Fetch::Modules, "/api/modules".into()));
        }
        if let DevicesPoll::Answered(at) = self.devs
            && at.is_none_or(|t| now - t >= DEVICES_EVERY)
        {
            self.devs = DevicesPoll::Asking {
                devices: true,
                io_map: true,
            };
            self.queue.push(Cmd::Get(Fetch::Devices, "/api/devices".into()));
            self.queue.push(Cmd::Get(Fetch::IoMap, "/api/io_map".into()));
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
        self.watch_values(now);
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

    /// A values stream that has said nothing for [`SILENT_AFTER`] is dead
    /// without having failed (a half-open connection: the power cut, the
    /// laptop's Wi-Fi asleep): close it, open it again, and count it.
    fn watch_values(&mut self, now: f64) {
        // Waiting out a backoff is a failure already counted.
        if self.values.at.is_none() && self.heard.is_some_and(|h| now - h > SILENT_AFTER) {
            self.connected = false;
            self.streaming = false;
            self.values.failures += 1;
            self.heard = None;
            self.queue.push(Cmd::Close(Stream::Values));
            self.queue.push(Cmd::Listen(Stream::Values, "/api/events".into()));
        }
    }

    /// Nobody looks at the box now (the page simulates in the browser):
    /// close its streams and stop retrying them.
    pub fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        self.queue.extend([
            Cmd::Close(Stream::Values),
            Cmd::Close(Stream::Trace),
            Cmd::Close(Stream::Traces),
        ]);
        self.values.at = None;
        self.trace.at = None;
        self.traces.at = None;
    }

    /// Whether [`Self::pause`]d.
    #[must_use]
    pub const fn paused(&self) -> bool {
        self.paused
    }

    /// The box is looked at again: its streams reopen, its list is read.
    pub fn resume(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        self.heard = None;
        self.streaming = false;
        self.queue.push(Cmd::Get(Fetch::Modules, "/api/modules".into()));
        self.queue.push(Cmd::Listen(Stream::Values, "/api/events".into()));
        if self.monitor {
            self.queue.push(Cmd::Listen(Stream::Traces, TRACES_URL.into()));
        } else if let Some(name) = self.selected.clone() {
            self.queue.push(Cmd::Listen(Stream::Trace, trace_url(&name)));
        }
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
        let r = if self.ever_connected {
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

    /// Reads the module list again, and the sources shown (after a deploy
    /// changed them): a literal or a threshold edited in place leaves the
    /// statements as they were, and nothing else would say the source moved.
    pub fn relist(&mut self) {
        self.queue.push(Cmd::Get(Fetch::Modules, "/api/modules".into()));
        if let Some(name) = self.selected.clone() {
            self.queue
                .push(Cmd::Get(Fetch::Module(name.clone()), module_url(&name)));
        }
        for (name, w) in &mut self.programs {
            w.refetch(name, &mut self.queue);
        }
    }

    /// Whether the devices' states are the last read's, which the newest
    /// read failed to renew.
    #[must_use]
    pub const fn devices_stale(&self) -> bool {
        self.devices_stale
    }

    /// Whether the map shown is the last read's, which the newest read
    /// failed to renew.
    #[must_use]
    pub const fn io_map_stale(&self) -> bool {
        self.io_map_stale
    }

    /// How many times the box was seen to restart: its clock went back.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The agent's state the values stream pushed, once.
    pub fn take_agent(&mut self) -> Option<Json> {
        self.agent.take()
    }

    /// Keeps the chosen module while the box still runs it, else chooses
    /// the first.
    fn reselect(&mut self) {
        let keep = self
            .selected
            .as_ref()
            .is_some_and(|s| self.modules.iter().any(|m| &m.name == s));
        if !keep && let Some(first) = self.modules.first().map(|m| m.name.clone()) {
            self.select(&first);
        }
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
                        // The box answered: it is a box, whatever its
                        // streams do (all its slots may be taken).
                        self.ever_connected = true;
                        self.modules = list;
                        self.prune();
                        self.reselect();
                    }
                    None => self.list.failed(now),
                }
            }
            Msg::Got(Fetch::Modules, Err(_)) => self.list.failed(now),
            Msg::Got(Fetch::Devices, r) => {
                self.devs.answered(true, now);
                let list = r
                    .ok()
                    .as_deref()
                    .and_then(parse)
                    .and_then(|j| rows(&j, DeviceRow::parse));
                // A failed read leaves the last one on show, marked old.
                self.devices_stale = list.is_none();
                if let Some(list) = list {
                    self.devices = list;
                }
            }
            Msg::Got(Fetch::IoMap, r) => {
                self.devs.answered(false, now);
                // 404: the box runs no map, which is an answer. Any other
                // failure keeps the map that was read, marked old.
                if r.as_ref().is_err_and(|e| e == "HTTP 404") {
                    self.io_map = None;
                    self.io_map_stale = false;
                } else {
                    let text = r
                        .ok()
                        .as_deref()
                        .and_then(parse)
                        .and_then(|j| j["text"].as_str().map(str::to_string));
                    self.io_map_stale = text.is_none();
                    if text.is_some() {
                        self.io_map = text;
                    }
                }
            }
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
                // The failures count starts over with the first event, not
                // here: a stream that opens and fails at once is no better
                // than one that never opened.
                self.heard = Some(now);
                self.ever_connected = true;
                if self.values.failures == 0 {
                    self.connected = true;
                }
            }
            Msg::Opened(Stream::Trace) => self.trace.ok(),
            Msg::Opened(Stream::Traces) => self.traces.ok(),
            Msg::Failed(Stream::Values) => {
                self.heard = None;
                if std::mem::take(&mut self.streaming) {
                    // It was delivering, so this is the box ending it (it
                    // does at 600 s) or a blip: open it again at once, and
                    // keep saying connected. A second failure is the box
                    // being gone, and backs off.
                    self.queue.push(Cmd::Listen(Stream::Values, "/api/events".into()));
                } else {
                    self.connected = false;
                    self.values.failed(now);
                }
            }
            Msg::Failed(Stream::Trace) => self.trace.failed(now),
            Msg::Failed(Stream::Traces) => self.traces.failed(now),
            Msg::Event(Stream::Values, name, data) if name == "agent" => {
                self.agent = parse(&data);
            }
            Msg::Event(Stream::Values, name, data) if name == "values" => {
                if let Some(j) = parse(&data) {
                    self.values.ok();
                    self.connected = true;
                    self.ever_connected = true;
                    self.streaming = true;
                    self.heard = Some(now);
                    let (seq, at_ms) = (j["seq"].as_u64(), j["at_ms"].as_u64());
                    if at_ms.zip(self.values_at_ms).is_some_and(|(n, o)| n < o)
                        || seq.zip(self.seq).is_some_and(|(n, o)| n < o)
                    {
                        self.epoch += 1;
                    }
                    self.seq = seq;
                    self.values_at = Some(now);
                    self.values_at_ms = at_ms;
                    if let Some(s) = rows(&j["signals"], SignalRow::parse) {
                        self.signals = s;
                    }
                    if let Some(m) = rows(&j["modules"], ModuleRow::parse) {
                        self.modules = m;
                        self.prune();
                        self.reselect();
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
            Msg::Build(b) => self.build = Some(b),
        }
    }

    /// Lowers the source against the newest scan's table, when either is
    /// new; fetches the source again if the table changed under it.
    fn relower(&mut self) {
        let (Some(name), Some(scan)) = (self.selected.clone(), self.scan.as_ref()) else {
            return;
        };
        if self.lowered.as_ref().is_some_and(|l| l.table != scan.table) {
            self.lowered = None;
        }
        if self
            .lowered
            .as_ref()
            .is_some_and(|l| self.source.as_deref().is_none_or(|s| s == l.src))
        {
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
                Cmd::Listen(Stream::Values, "/api/events".into()),
                // The Devices tab's two reads, every DEVICES_EVERY.
                Cmd::Get(Fetch::Devices, "/api/devices".into()),
                Cmd::Get(Fetch::IoMap, "/api/io_map".into()),
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
        // The stream was delivering: its end is reopened at once, quietly.
        s.apply(Msg::Failed(Stream::Values), now);
        assert_eq!(s.link(), Link::Connected);
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
    fn a_slow_io_map_is_not_asked_for_again_while_it_is_on_its_way() {
        // Over a slow link the map's reply can trail the devices' by more
        // than DEVICES_EVERY: the next round waits for both, or the reads
        // stack up behind each other (card canvas-slow-link).
        let mut s = BoxState::new();
        let gets = |cmds: Vec<Cmd>| -> Vec<Fetch> {
            cmds.into_iter()
                .filter_map(|c| match c {
                    Cmd::Get(f @ (Fetch::Devices | Fetch::IoMap), _) => Some(f),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(gets(s.poll(0.0)), [Fetch::Devices, Fetch::IoMap]);
        s.apply(Msg::Got(Fetch::Devices, Ok("[]".into())), 0.3);
        for now in [5.3, 10.0, 60.0] {
            assert_eq!(gets(s.poll(now)), [], "the map is still on its way at {now}");
        }
        s.apply(Msg::Got(Fetch::IoMap, Err("HTTP 404".into())), 61.0);
        assert_eq!(
            gets(s.poll(65.9)),
            [],
            "a round every DEVICES_EVERY after the last answer"
        );
        assert_eq!(gets(s.poll(66.0)), [Fetch::Devices, Fetch::IoMap]);
        // Either may come first.
        s.apply(Msg::Got(Fetch::IoMap, Err("HTTP 404".into())), 66.2);
        assert_eq!(gets(s.poll(80.0)), [], "the devices are still on their way");
        s.apply(Msg::Got(Fetch::Devices, Ok("[]".into())), 80.0);
        assert_eq!(gets(s.poll(85.0)), [Fetch::Devices, Fetch::IoMap]);
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
