//! What the API knows that does not change while the box runs: each
//! signal's name, type, direction, unit, binding and writer; each module's
//! source, manifest, schedule, windows and trace tap; the device links.
//! Built once at startup, before the scheduler takes the runtime.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use helmsman_iec::{Port, PortDir};
use helmsman_io::{Binding, Dir, IoMap, Ty, Value};
use serde_json::{Value as Json, json};

use crate::manifest::Manifest;
use crate::signals::SignalTable;
use crate::{Runtime, TraceTap, Window};

/// One signal.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalInfo {
    pub name: String,
    pub ty: Ty,
    pub dir: Dir,
    pub unit: Option<String>,
    /// Where it comes from or goes to, as JSON (`{"kind":"device",...}`).
    pub binding: Json,
    /// Who drives it: a module, `calendar`, or nobody (`None`): an input
    /// read from a device, or an output no module drives (it stays safe).
    pub writer: Option<String>,
    /// Outputs: the value it goes to when its writer stops.
    pub safe: Option<Value>,
}

/// One module.
#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub name: String,
    /// Where it was loaded from (a file or a module directory).
    pub origin: String,
    pub period: Duration,
    pub i: Window,
    pub q: Window,
    pub source: String,
    /// Modules loaded from a directory have one.
    pub manifest: Option<Manifest>,
    /// The program's `VAR_INPUT`s and `VAR_OUTPUT`s.
    pub ports: Vec<Port>,
    pub tap: Option<TraceTap>,
}

/// Everything fixed at startup.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// In [`SignalTable`] order.
    pub signals: Vec<SignalInfo>,
    /// In scan order ([`Runtime::names`]).
    pub modules: Vec<ModuleInfo>,
}

impl Catalog {
    /// The signals of `table` described from `map` (the I/O map it was
    /// built from, if any) and the modules of `rt`. Signals past the
    /// map's are the box's own inputs (the calendar's).
    #[must_use]
    pub fn signals(table: &SignalTable, map: Option<&IoMap>, modules: &[ModuleInfo]) -> Vec<SignalInfo> {
        (0..table.len())
            .filter_map(|i| {
                let name = table.name(i)?.to_string();
                let (dir, ty) = table.signal(i)?;
                let mapped = map.and_then(|m| m.signals.get(i)).filter(|s| s.name == name);
                let binding = match mapped.map(|s| &s.binding) {
                    Some(Binding::Device { device, point }) => {
                        json!({ "kind": "device", "device": device, "point": point })
                    }
                    Some(Binding::Local { channel }) => json!({ "kind": "box", "channel": channel }),
                    Some(Binding::Module { module }) => json!({ "kind": "module", "module": module }),
                    None => json!({ "kind": "calendar" }),
                };
                let driver = || {
                    modules
                        .iter()
                        .find(|m| {
                            m.ports
                                .iter()
                                .any(|p| p.dir == PortDir::Output && p.name.eq_ignore_ascii_case(&name))
                        })
                        .map(|m| m.name.clone())
                };
                let writer = match dir {
                    Dir::Out => table.writer(i).map(str::to_string).or_else(driver),
                    Dir::In => table.source(i).map(str::to_string),
                };
                Some(SignalInfo {
                    unit: table.unit(i).map(str::to_string),
                    safe: table.safe(i),
                    name,
                    ty,
                    dir,
                    binding,
                    writer,
                })
            })
            .collect()
    }

    /// Index of a module by exact name.
    #[must_use]
    pub fn module(&self, name: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.name == name)
    }

    /// Index of a signal by name in any ASCII case, as ST and the table
    /// match them.
    #[must_use]
    pub fn signal(&self, name: &str) -> Option<usize> {
        self.signals
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }
}

impl ModuleInfo {
    /// A module as the runtime holds it, with its source and manifest.
    #[must_use]
    pub fn of(
        rt: &Runtime,
        name: &str,
        origin: String,
        source: String,
        manifest: Option<Manifest>,
    ) -> Option<Self> {
        let (period, i, q) = rt.schedule(name)?;
        Some(Self {
            name: name.into(),
            origin,
            period,
            i,
            q,
            source,
            manifest,
            ports: rt.program(name)?.ports(),
            tap: rt.tap(name),
        })
    }
}

/// A link's last state: up (`Ok`), down with why, or `None` before it reports.
pub type LinkState = Option<Result<(), String>>;

/// Each device link's last state, set by the link threads.
#[derive(Debug, Default)]
pub struct Devices {
    links: Mutex<Vec<(String, LinkState)>>,
}

impl Devices {
    /// Links called `names`, state unknown until they report.
    #[must_use]
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            links: Mutex::new(names.into_iter().map(|n| (n, None)).collect()),
        }
    }

    /// Records a link going up (`None`) or down with its error.
    pub fn set(&self, name: &str, err: Option<String>) {
        let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        let state = Some(err.map_or(Ok(()), Err));
        match links.iter_mut().find(|(n, _)| n == name) {
            Some(l) => l.1 = state,
            None => links.push((name.into(), state)),
        }
    }

    /// A link that a reload started: state unknown until it reports. A
    /// name already listed is left as it is.
    pub fn add(&self, name: &str) {
        let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        if !links.iter().any(|(n, _)| n == name) {
            links.push((name.into(), None));
        }
    }

    /// A link that a reload ended for good.
    pub fn remove(&self, name: &str) {
        self.links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(n, _)| n != name);
    }

    /// Every link: name, then up (`Some(Ok)`), down with why, or not yet known.
    #[must_use]
    pub fn list(&self) -> Vec<(String, LinkState)> {
        self.links.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
