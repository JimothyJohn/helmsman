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
use helmsman_nodecore::router::Control;

/// Bytes of the image the I/O task moves each way (the E1213: DI bits in
/// byte 0 of the inputs, DO bits in byte 0 of the outputs).
pub const IO_BYTES: usize = 4;
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

/// The device link.
#[derive(Default)]
pub struct DeviceLive {
    pub name: String,
    pub address: String,
    /// `None` before the first attempt.
    pub up: Option<bool>,
    pub error: Option<String>,
    pub reconnects: u32,
}

/// What a request asked the scan loop to do; cleared once done.
pub enum Pending {
    Deploy {
        name: String,
        source: String,
        program: Program,
    },
    Control(Control),
}

#[derive(Default)]
pub struct Live {
    pub inputs: [u8; IO_BYTES],
    pub outputs: [u8; IO_BYTES],
    pub module: ModuleLive,
    pub device: DeviceLive,
    /// Publication count and time, bumped by the scan loop when the
    /// signals were refreshed.
    pub seq: u64,
    pub at: Option<Instant>,
    pub pending: Option<Pending>,
    /// The last control's resulting state, for the request's reply.
    pub controlled: Option<&'static str>,
    pub link_up: bool,
    pub ip: Option<[u8; 4]>,
}

pub type Shared = Mutex<CriticalSectionRawMutex, RefCell<Live>>;

/// Runs `f` with the state.
pub fn with<R>(shared: &Shared, f: impl FnOnce(&mut Live) -> R) -> R {
    shared.lock(|cell| f(&mut cell.borrow_mut()))
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
        let devices = if live.device.name.is_empty() {
            Vec::new()
        } else {
            alloc::vec![api::DeviceView {
                name: live.device.name.clone(),
                state: match live.device.up {
                    None => "unknown",
                    Some(true) => "up",
                    Some(false) => "down",
                },
                error: live.device.error.clone(),
                address: Some(live.device.address.clone()),
            }]
        };
        let io_map = if live.device.name.is_empty() {
            String::new()
        } else {
            alloc::format!("device {} modbus-tcp {}\n", live.device.name, live.device.address)
        };
        api::NodeView {
            module,
            signals: live.module.signals.clone(),
            devices,
            seq: live.seq,
            at_ms: live.at.map_or(0, |t| t.as_millis()),
            uptime_ms: now.as_millis(),
            heap_used: esp_alloc::HEAP.used(),
            heap_peak: esp_alloc::HEAP.stats().max_usage,
            heap_size,
            mac,
            ip: live.ip,
            link_up: live.link_up,
            version: helmsman_nodecore::VERSION,
            board,
            io_map,
        }
    })
}

/// A program's located variables and ports as signals, with their values.
#[must_use]
pub fn signals_of(program: &Program) -> Vec<SignalView> {
    program
        .ports()
        .iter()
        .map(|p| {
            let value = match program.get(&p.name) {
                Some(helmsman_iec::Value::Bool(b)) => Some(SignalValue::Bool(b)),
                Some(helmsman_iec::Value::Int(i)) => {
                    Some(SignalValue::Int(i64::try_from(i).unwrap_or(i64::MAX)))
                }
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
            }
        })
        .collect()
}
