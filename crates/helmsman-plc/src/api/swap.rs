//! How an approved change reaches the running box. The scan thread owns
//! the [`Runtime`] and the API never touches it, so an apply is a job
//! handed to the scan thread: a [`Swapper`] there takes it between ticks
//! (as the swap contract wants), runs [`Runtime::reconfigure`], rebuilds
//! the [`Catalog`] the API serves, and answers. A refused job changes
//! nothing. The API side ([`SwapHandle::run`]) waits a bounded time; a job
//! the scan thread has not yet taken when that runs out is withdrawn, so
//! it can never land after the API has answered.
//!
//! A change set that brings another I/O map or calendar is a *reload*
//! ([`SwapHandle::reload`]), in this order, under the device links' lock:
//!
//! 1. Everything that can fail without touching the box already has (the
//!    caller built the new [`Layout`] and checked the modules against
//!    it); the links the new map needs are built with their drivers
//!    opened ([`LinkSet::plan`]). A driver that will not open refuses the
//!    reload here: nothing has changed.
//! 2. The links of removed and changed devices are stopped and their
//!    threads joined ([`LinkSet::stop`]). From here none of them writes:
//!    their devices hold what they last took, and their inputs keep their
//!    last values in the table.
//! 3. The scan thread, between two ticks and with the table locked, does
//!    the rest as one step: the new table takes over every signal whose
//!    name, type, direction and binding are unchanged, value and command
//!    and all ([`SignalTable::carry_from`]); the modules are rebound to it
//!    ([`Runtime::rebind`]; refused, nothing changed); the new calendar is
//!    evaluated into it; it replaces the old table; the recorder starts
//!    over on it. No tick ever sees half of it.
//! 4. Refused or not taken: the stopped links run again, as they were
//!    ([`LinkSet::resume`]). Taken: every output point the new map no
//!    longer has on its device is written to its safe value through the
//!    stopped link's own driver, and the new links start
//!    ([`LinkSet::finish`]). Links the new map leaves exactly as they were
//!    never stop; they find their points in the new table by name.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};

use super::catalog::{Catalog, ModuleInfo};
use crate::calendar::Source;
use crate::layout::Layout;
use crate::link::{Finished, LinkSet};
use crate::manifest::ModuleDir;
use crate::record::{RecordTap, Recorder};
use crate::spec::Defaults;
use crate::store::{ChangeSet, Code};
use crate::wasm::Limits;
use crate::{Module, Runtime, SignalTable};

/// What a replay or an apply needs to know about the box that does not
/// change while it runs: the signal table's shape (the I/O map and the
/// calendar's signals; values ignored), the image sizes, the module
/// defaults, and the recorder's tap. A reload replaces the table and the
/// tap; the API keeps the current ones.
#[derive(Debug, Clone)]
pub struct Plant {
    pub table: SignalTable,
    pub i_len: usize,
    pub q_len: usize,
    pub defaults: Defaults,
    /// `None`: nothing is recorded, so no replay can run.
    pub record: Option<RecordTap>,
}

/// Every module of `cs`, compiled (ST) or loaded under the default
/// [`Limits`] (WASM) and checked against its manifest, in name order: what
/// the box would run.
///
/// # Errors
/// Every module that does not parse, compile, load or match its manifest.
pub fn compile_set(cs: &ChangeSet, d: &Defaults) -> Result<Vec<Module>, Vec<String>> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for (name, f) in &cs.modules {
        let dir = Path::new(name);
        let built = match &f.code {
            Code::Wasm(bytes) => ModuleDir::from_wasm(dir, &f.manifest, bytes.clone(), d)
                .and_then(|md| md.wasm_module(Limits::default())),
            Code::St(source) => ModuleDir::from_text(dir, &f.manifest, source.clone(), d).and_then(|md| {
                let sizes = ImageSizes {
                    i: md.spec.i.len,
                    q: md.spec.q.len,
                    m: md.spec.m,
                };
                let program = compile(&md.source, sizes).map_err(|diags| {
                    let lines: Vec<String> = diags.iter().map(|d| format!("{name}: {d}")).collect();
                    lines.join("; ")
                })?;
                md.module(program)
            }),
        };
        match built {
            Ok(m) => out.push(m),
            Err(e) => errs.push(e),
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    Pending,
    Taken,
    Withdrawn,
}

/// What the scan thread answers a job with.
#[derive(Debug)]
pub struct Swapped {
    pub catalog: Catalog,
    /// After a reload: the new table as it took over (for replays, which
    /// ignore its values).
    pub table: Option<SignalTable>,
    /// After a reload: the new recorder's tap, if the scan loop records.
    pub record: Option<RecordTap>,
}

/// One change for the scan thread.
struct Job {
    modules: Vec<Module>,
    change: ChangeSet,
    /// A reload: the new map and calendar.
    layout: Option<Layout>,
    claim: Arc<Mutex<Claim>>,
    reply: SyncSender<Result<Swapped, Vec<String>>>,
}

/// What a scan loop owns beside the runtime and the table that a reload
/// replaces: the calendar bound to the table, the recorder of it, and the
/// wall clock (seconds since the epoch) a new calendar is evaluated at.
#[derive(Debug, Default)]
pub struct Scene {
    pub calendar: Option<Source>,
    pub recorder: Option<Recorder>,
    pub wall: Option<i64>,
}

/// What [`Swapper::serve_scene`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Served {
    /// The module set changed.
    pub modules: bool,
    /// The table (map or calendar) was replaced.
    pub reloaded: bool,
}

/// The scan thread's side.
#[derive(Debug)]
pub struct Swapper {
    rx: Receiver<Job>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job").finish_non_exhaustive()
    }
}

/// The API's side.
#[derive(Debug)]
pub struct SwapHandle {
    tx: Mutex<Sender<Job>>,
    /// Longest the API waits for the scan thread to take a job.
    pub wait: Duration,
    /// The box's device links, which a reload changes; `None`: the box
    /// runs none, and a reload to a map with device or box signals is
    /// refused.
    links: Option<Arc<Mutex<LinkSet>>>,
}

/// A connected pair; `wait` as [`SwapHandle::wait`].
#[must_use]
pub fn channel(wait: Duration) -> (SwapHandle, Swapper) {
    let (tx, rx) = mpsc::channel();
    (
        SwapHandle {
            tx: Mutex::new(tx),
            wait,
            links: None,
        },
        Swapper { rx },
    )
}

/// The catalog of what `rt` runs, its modules' files from `change`.
#[must_use]
pub fn catalog_of(rt: &Runtime, table: &SignalTable, change: &ChangeSet) -> Catalog {
    let origin = format!("snapshot {}", change.id());
    let modules: Vec<ModuleInfo> = rt
        .names()
        .filter_map(|name| {
            let f = change.modules.get(name)?;
            let manifest = crate::manifest::Manifest::parse(&f.manifest).ok();
            ModuleInfo::of(rt, name, origin.clone(), f.code.shown(), manifest)
        })
        .collect();
    let map = change
        .io_map
        .as_deref()
        .and_then(|m| helmsman_io::text::parse(m).ok())
        .map(|p| p.map);
    Catalog {
        signals: Catalog::signals(table, map.as_ref(), &modules),
        modules,
    }
}

impl Swapper {
    /// Runs any job waiting, on the scan thread, between ticks, with the
    /// table locked. Returns whether the module set changed. A reload is
    /// refused here (this loop has no [`Scene`]); see
    /// [`Swapper::serve_scene`].
    pub fn serve(&self, rt: &mut Runtime, table: &mut SignalTable) -> bool {
        self.serve_scene(rt, table, None).modules
    }

    /// [`Swapper::serve`] for a scan loop that can take a reload: one that
    /// keeps its calendar and recorder in `scene`. A reload replaces
    /// `table` and `scene`'s calendar and recorder as one step (see the
    /// [module docs](self)); refused, it changes none of them.
    pub fn serve_scene(
        &self,
        rt: &mut Runtime,
        table: &mut SignalTable,
        mut scene: Option<&mut Scene>,
    ) -> Served {
        let mut served = Served::default();
        while let Ok(job) = self.rx.try_recv() {
            {
                let mut c = job.claim.lock().unwrap_or_else(PoisonError::into_inner);
                if *c != Claim::Pending {
                    continue;
                }
                *c = Claim::Taken;
            }
            let result = match (job.layout, scene.as_deref_mut()) {
                (None, _) => match rt.reconfigure(job.modules, table) {
                    Ok(()) => {
                        served.modules = true;
                        Ok(Swapped {
                            catalog: catalog_of(rt, table, &job.change),
                            table: None,
                            record: None,
                        })
                    }
                    Err(errs) => Err(errs.iter().map(ToString::to_string).collect()),
                },
                (Some(_), None) => Err(vec![
                    "this scan loop cannot reload the I/O map or the calendar".to_string(),
                ]),
                (Some(layout), Some(scene)) => reload(rt, table, scene, job.modules, layout).map(|record| {
                    served.modules = true;
                    served.reloaded = true;
                    Swapped {
                        catalog: catalog_of(rt, table, &job.change),
                        table: Some(table.clone()),
                        record,
                    }
                }),
            };
            let _ = job.reply.send(result);
        }
        served
    }
}

/// Step 3 of a reload (module docs), on the scan thread. The new
/// recorder's tap, if there is a recorder.
fn reload(
    rt: &mut Runtime,
    table: &mut SignalTable,
    scene: &mut Scene,
    modules: Vec<Module>,
    layout: Layout,
) -> Result<Option<RecordTap>, Vec<String>> {
    let Layout {
        table: mut next,
        calendar,
        map: _,
    } = layout;
    next.carry_from(table);
    rt.rebind(modules, &mut next)
        .map_err(|errs| errs.iter().map(ToString::to_string).collect::<Vec<_>>())?;
    if let Some(c) = &calendar {
        c.update(scene.wall, &mut next);
    }
    *table = next;
    scene.calendar = calendar;
    Ok(scene.recorder.as_mut().map(|r| {
        *r = r.rebuilt(table);
        r.tap()
    }))
}

/// Why the scan thread did not run a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapError {
    /// It refused the module set; nothing changed.
    Refused(Vec<String>),
    /// It did not take the job in time (or is gone); withdrawn, nothing
    /// changed.
    NotTaken,
    /// A reload's device link would not start (its driver would not
    /// open); nothing changed.
    Links(String),
    /// A reload to a map with device or box signals on a box that runs no
    /// device links; nothing changed.
    NoLinks,
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(errs) => write!(f, "the running box refused it: {}", errs.join("; ")),
            Self::NotTaken => f.write_str("the scan thread did not take it in time"),
            Self::Links(e) => write!(f, "a device link would not start: {e}"),
            Self::NoLinks => f.write_str(
                "this box runs no device links, so it cannot take a map with device or box signals",
            ),
        }
    }
}

/// A reload that landed.
#[derive(Debug)]
pub struct Reloaded {
    pub swapped: Swapped,
    /// What happened to the device links; empty when the box runs none.
    pub links: Finished,
}

impl SwapHandle {
    /// This handle, reloading `links` with the map (see the module docs).
    #[must_use]
    pub fn with_links(mut self, links: Arc<Mutex<LinkSet>>) -> Self {
        self.links = Some(links);
        self
    }

    /// Hands `modules` (compiled from `change`) to the scan thread and
    /// waits for it.
    ///
    /// # Errors
    /// [`SwapError`]; in every case the running box is as it was.
    pub fn run(&self, modules: Vec<Module>, change: ChangeSet) -> Result<Catalog, SwapError> {
        self.send(modules, change, None).map(|s| s.catalog)
    }

    /// Reloads the box onto `layout` (built from `change`'s map and
    /// calendar) with `modules` (compiled from `change`, checked against
    /// `layout`'s table): the whole sequence of the [module docs](self).
    /// One reload at a time.
    ///
    /// # Errors
    /// [`SwapError`]; in every case the box runs what it ran, links and
    /// all.
    pub fn reload(
        &self,
        modules: Vec<Module>,
        change: ChangeSet,
        layout: Layout,
    ) -> Result<Reloaded, SwapError> {
        let map = layout.map.clone().unwrap_or_default();
        let Some(links) = &self.links else {
            if !crate::signals::by_device(&map).is_empty() {
                return Err(SwapError::NoLinks);
            }
            return self.send(modules, change, Some(layout)).map(|swapped| Reloaded {
                swapped,
                links: Finished::default(),
            });
        };
        let mut links = links.lock().unwrap_or_else(PoisonError::into_inner);
        let plan = links.plan(&map).map_err(SwapError::Links)?;
        let stopped = links.stop(&plan);
        match self.send(modules, change, Some(layout)) {
            Ok(swapped) => Ok(Reloaded {
                swapped,
                links: links.finish(plan, stopped),
            }),
            Err(e) => {
                links.resume(stopped);
                Err(e)
            }
        }
    }

    fn send(
        &self,
        modules: Vec<Module>,
        change: ChangeSet,
        layout: Option<Layout>,
    ) -> Result<Swapped, SwapError> {
        let claim = Arc::new(Mutex::new(Claim::Pending));
        let (reply, answer) = mpsc::sync_channel(1);
        let job = Job {
            modules,
            change,
            layout,
            claim: Arc::clone(&claim),
            reply,
        };
        if self
            .tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(job)
            .is_err()
        {
            return Err(SwapError::NotTaken);
        }
        match answer.recv_timeout(self.wait) {
            Ok(r) => return r.map_err(SwapError::Refused),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(SwapError::NotTaken),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        {
            let mut c = claim.lock().unwrap_or_else(PoisonError::into_inner);
            if *c == Claim::Pending {
                *c = Claim::Withdrawn;
                return Err(SwapError::NotTaken);
            }
        }
        // Taken: the scan thread is applying it now and always answers.
        answer
            .recv()
            .map_err(|_| SwapError::NotTaken)
            .and_then(|r| r.map_err(SwapError::Refused))
    }
}
