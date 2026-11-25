//! What the scan loop, the I/O task and the HTTP server share: the live
//! image, the module's counters, the device link's state, and the one
//! slot a request puts a deploy or a control into for the scan loop to
//! take at its next scan. One blocking mutex (critical section); every
//! holder copies out and releases, nothing awaits while holding it.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use helmsman_iec::Program;
use helmsman_nodecore::api::{self, SignalValue, SignalView, Spread, State, Timing};
use helmsman_nodecore::iomap::{Dir, IoMap};
use helmsman_nodecore::ioplan;
use helmsman_nodecore::router::Control;

/// Bytes of the process image the I/O tasks move each way: the map's bool
/// windows and numeric area (`helmsman_nodecore::iomap::IMAGE_BYTES`).
pub const IO_BYTES: usize = helmsman_nodecore::iomap::IMAGE_BYTES;
/// Scans the timing numbers cover.
pub const WINDOW: usize = 128;

/// A ring of the newest scans' times, µs.
pub struct Ring {
    buf: [u32; WINDOW],
    n: usize,
    at: usize,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            buf: [0; WINDOW],
            n: 0,
            at: 0,
        }
    }
}

impl Ring {
    pub fn push(&mut self, us: u32) {
        self.buf[self.at] = us;
        self.at = (self.at + 1) % WINDOW;
        self.n = (self.n + 1).min(WINDOW);
    }

    #[must_use]
    pub fn spread(&self) -> Option<Spread> {
        if self.n == 0 {
            return None;
        }
        let mut sorted = [0u32; WINDOW];
        sorted[..self.n].copy_from_slice(&self.buf[..self.n]);
        sorted[..self.n].sort_unstable();
        let s = &sorted[..self.n];
        let sum: u64 = s.iter().map(|v| u64::from(*v)).sum();
        #[allow(clippy::cast_precision_loss)]
        Some(Spread {
            min: f64::from(s[0]),
            mean: sum as f64 / self.n as f64,
            max: f64::from(s[self.n - 1]),
            p99: f64::from(s[(self.n * 99 / 100).min(self.n - 1)]),
        })
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.n
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.n == 0
    }
}

/// The one module.
#[derive(Default)]
pub struct ModuleLive {
    pub name: String,
    pub source: String,
    pub state: Option<State>,
    pub scans: u64,
    pub late: u64,
    pub over: u64,
    pub last_overrun: Option<Instant>,
    pub exec: Ring,
    pub lateness: Ring,
    pub signals: Vec<SignalView>,
}

/// One device's link.
#[derive(Default)]
pub struct DeviceLive {
    pub name: String,
    pub address: String,
    /// `None` before the first attempt.
    pub up: Option<bool>,
    pub error: Option<String>,
    pub reconnects: u32,
}

/// The firmware half of the live state (`GET /api/firmware`).
pub struct FwLive {
    /// The partition table has two app slots and the trial record.
    pub available: bool,
    pub slot: Option<u8>,
    pub trial: helmsman_nodecore::ota::status::Trial,
    pub fetch: helmsman_nodecore::ota::status::Fetch,
}

impl Default for FwLive {
    fn default() -> Self {
        Self {
            available: false,
            slot: None,
            trial: helmsman_nodecore::ota::status::Trial::Confirmed,
            fetch: helmsman_nodecore::ota::status::Fetch::Idle,
        }
    }
}

/// What a request asked the scan loop to do; cleared once done.
pub enum Pending {
    /// Compile and run `source` as `name`. The scan loop compiles, not the
    /// server: the running program is dropped first, so only one compiled
    /// program and one compile peak are ever in the WROOM's heap at once
    /// (compiling a second program beside the first ran it out of memory
    /// on 2026-11-08).
    Deploy {
        name: String,
        source: String,
    },
    Control(Control),
}

/// What became of a deploy: `Ok` runs, `Err` lists the checker's lines
/// and the previous program runs again (or nothing, if that failed too).
pub type DeployResult = Result<(), Vec<String>>;

pub struct Live {
    pub inputs: [u8; IO_BYTES],
    pub outputs: [u8; IO_BYTES],
    pub module: ModuleLive,
    /// One link a device of the map, in the map's order.
    pub devices: Vec<DeviceLive>,
    /// The I/O map text the node runs (card node-io-map): what
    /// `/api/io_map` and the settings page show.
    pub io_map: String,
    /// Publication count and time, bumped by the scan loop when the
    /// signals were refreshed.
    pub seq: u64,
    pub at: Option<Instant>,
    pub pending: Option<Pending>,
    /// The last control's resulting state, for the request's reply.
    pub controlled: Option<&'static str>,
    /// The last deploy's outcome, for the request's reply.
    pub deployed: Option<DeployResult>,
    pub link_up: bool,
    pub ip: Option<[u8; 4]>,
    pub fw: FwLive,
    /// The comms check: the last one, or the one running.
    pub check: helmsman_nodecore::check::Check,
}

impl Default for Live {
    /// Arrays past 32 bytes have no derived default.
    fn default() -> Self {
        Self {
            inputs: [0; IO_BYTES],
            outputs: [0; IO_BYTES],
            module: ModuleLive::default(),
            devices: Vec::new(),
            io_map: String::new(),
            seq: 0,
            at: None,
            pending: None,
            controlled: None,
            deployed: None,
            link_up: false,
            ip: None,
            fw: FwLive::default(),
            check: helmsman_nodecore::check::Check::default(),
        }
    }
}

pub type Shared = Mutex<CriticalSectionRawMutex, RefCell<Live>>;

/// Runs `f` with the state.
pub fn with<R>(shared: &Shared, f: impl FnOnce(&mut Live) -> R) -> R {
    shared.lock(|cell| f(&mut cell.borrow_mut()))
}

/// The heap by region: bytes in use in the internal region(s), and the
/// external region's (PSRAM) `(size, used)` when the image added one.
#[must_use]
pub fn heap() -> (usize, Option<(usize, usize)>) {
    let mut internal = 0;
    let mut external = None;
    for region in esp_alloc::HEAP.stats().region_stats.iter().flatten() {
        if region
            .capabilities
            .contains(esp_alloc::MemoryCapability::External)
        {
            external = Some((region.size, region.used));
        } else {
            internal += region.used;
        }
    }
    (internal, external)
}

/// The box API view of the state right now, for the router and the stream.
#[must_use]
pub fn view(
    shared: &Shared,
    mac: [u8; 6],
    board: &'static str,
    heap_size: usize,
    scan_ms: f64,
) -> api::NodeView {
    let now = Instant::now();
    with(shared, |live| {
        let module = live.module.state.as_ref().map(|state| api::ModuleView {
            name: live.module.name.clone(),
            scan_ms,
            state: state.clone(),
            scans: live.module.scans,
            overruns: live.module.late,
            last_overrun_ms_ago: live
                .module
                .last_overrun
                .map(|t| now.saturating_duration_since(t).as_millis()),
            timing: match (live.module.exec.spread(), live.module.lateness.spread()) {
                (Some(exec), Some(late)) => Some(Timing {
                    window: live.module.exec.len() as u64,
                    exec,
                    late,
                }),
                _ => None,
            },
            source: live.module.source.clone(),
        });
        let devices = live
            .devices
            .iter()
            .map(|d| api::DeviceView {
                name: d.name.clone(),
                state: match d.up {
                    None => "unknown",
                    Some(true) => "up",
                    Some(false) => "down",
                },
                error: d.error.clone(),
                address: Some(d.address.clone()),
            })
            .collect();
        let io_map = live.io_map.clone();
        api::NodeView {
            module,
            signals: live.module.signals.clone(),
            devices,
            seq: live.seq,
            at_ms: live.at.map_or(0, |t| t.as_millis()),
            uptime_ms: now.as_millis(),
            heap_used: heap().0,
            heap_peak: esp_alloc::HEAP.stats().max_usage,
            heap_size,
            heap_external: heap().1,
            mac,
            ip: live.ip,
            link_up: live.link_up,
            version: helmsman_nodecore::VERSION,
            board,
            io_map,
            one_block: false,
            check: Some(live.check.clone()),
            firmware: live
                .fw
                .available
                .then(|| helmsman_nodecore::ota::status::Firmware {
                    version: helmsman_nodecore::VERSION,
                    build: crate::ota::BUILD,
                    chip: helmsman_nodecore::ota::image::Chip::Esp32P4.label(),
                    slot: live.fw.slot,
                    trial: live.fw.trial,
                    fetch: live.fw.fetch.clone(),
                }),
        }
    })
}

/// How a device's link stands, for the points on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkState {
    /// The link is down (a point on it shows the last value it had).
    pub down: bool,
    pub error: Option<String>,
}

/// The links as the signals list needs them, one a device.
#[must_use]
pub fn link_states(shared: &Shared) -> Vec<LinkState> {
    with(shared, |live| {
        live.devices
            .iter()
            .map(|d| LinkState {
                down: d.up == Some(false),
                error: if d.up == Some(false) {
                    d.error.clone()
                } else {
                    None
                },
            })
            .collect()
    })
}

/// The node's signals: the map's points as the I/O tasks move them (an
/// input from `inputs`, an output from `outputs`), by name, in the
/// order the map's image gives them, then the program's ports with their
/// values (a port bound to a point by name is that point, listed once). A point on a device whose link is down keeps its last value
/// and says so (`stale`, with the link's error). The runtime does not
/// list located variables (`AT %IX0.0`), so the points are read here from
/// the image.
#[must_use]
pub fn signals_of(
    program: &Program,
    map: &IoMap,
    inputs: &[u8; IO_BYTES],
    outputs: &[u8; IO_BYTES],
    links: &[LinkState],
) -> Vec<SignalView> {
    let mut out: Vec<SignalView> = Vec::with_capacity(map.points.len());
    for dir in [Dir::In, Dir::Out] {
        let image = if dir == Dir::In { inputs } else { outputs };
        for point in map.in_image_order(dir) {
            let link = links.get(point.device).filter(|l| l.down);
            out.push(SignalView {
                name: point.name.clone(),
                dir: if dir == Dir::In { "in" } else { "out" },
                value: ioplan::value_at(image, point),
                quality: if link.is_some() { "stale" } else { "good" },
                point_error: link.and_then(|l| l.error.clone()),
                sim: point.is_sim(),
            });
        }
    }
    // A port bound to a point by name (bind.rs) is that point: listed once.
    let bound = |name: &str| map.points.iter().any(|pt| pt.name.eq_ignore_ascii_case(name));
    out.extend(program.ports().iter().filter(|p| !bound(&p.name)).map(|p| {
        let value = match program.get(&p.name) {
            Some(helmsman_iec::Value::Bool(b)) => Some(SignalValue::Bool(b)),
            Some(helmsman_iec::Value::Int(i)) => Some(SignalValue::Int(i64::try_from(i).unwrap_or(i64::MAX))),
            _ => None,
        };
        SignalView {
            name: p.name.clone(),
            dir: match p.dir {
                helmsman_iec::PortDir::Input => "in",
                helmsman_iec::PortDir::Output => "out",
            },
            value,
            quality: "good",
            point_error: None,
            sim: false,
        }
    }));
    out
}
