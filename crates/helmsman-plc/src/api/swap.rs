//! How an approved change reaches the running box. The scan thread owns
//! the [`Runtime`] and the API never touches it, so an apply is a job
//! handed to the scan thread: a [`Swapper`] there takes it between ticks
//! (as the swap contract wants), runs [`Runtime::reconfigure`], rebuilds
//! the [`Catalog`] the API serves, and answers. A refused job changes
//! nothing. The API side ([`SwapHandle::run`]) waits a bounded time; a job
//! the scan thread has not yet taken when that runs out is withdrawn, so
//! it can never land after the API has answered.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};

use super::catalog::{Catalog, ModuleInfo};
use crate::manifest::ModuleDir;
use crate::record::RecordTap;
use crate::spec::Defaults;
use crate::store::ChangeSet;
use crate::{Module, Runtime, SignalTable};

/// What a replay or an apply needs to know about the box that does not
/// change while it runs: the signal table's shape (the I/O map and the
/// calendar's signals; values ignored), the image sizes, the module
/// defaults, and the recorder's tap.
#[derive(Debug, Clone)]
pub struct Plant {
    pub table: SignalTable,
    pub i_len: usize,
    pub q_len: usize,
    pub defaults: Defaults,
    /// `None`: nothing is recorded, so no replay can run.
    pub record: Option<RecordTap>,
}

/// Every module of `cs`, compiled and checked against its manifest, in
/// name order: what the box would run.
///
/// # Errors
/// Every module that does not parse, compile or match its manifest.
pub fn compile_set(cs: &ChangeSet, d: &Defaults) -> Result<Vec<Module>, Vec<String>> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for (name, f) in &cs.modules {
        let built = ModuleDir::from_text(Path::new(name), &f.manifest, f.source.clone(), d).and_then(|md| {
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
        });
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

/// One change for the scan thread.
struct Job {
    modules: Vec<Module>,
    change: ChangeSet,
    claim: Arc<Mutex<Claim>>,
    reply: SyncSender<Result<Catalog, Vec<String>>>,
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
}

/// A connected pair; `wait` as [`SwapHandle::wait`].
#[must_use]
pub fn channel(wait: Duration) -> (SwapHandle, Swapper) {
    let (tx, rx) = mpsc::channel();
    (
        SwapHandle {
            tx: Mutex::new(tx),
            wait,
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
            ModuleInfo::of(rt, name, origin.clone(), f.source.clone(), manifest)
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
    /// table locked. Returns whether the module set changed.
    pub fn serve(&self, rt: &mut Runtime, table: &mut SignalTable) -> bool {
        let mut changed = false;
        while let Ok(job) = self.rx.try_recv() {
            {
                let mut c = job.claim.lock().unwrap_or_else(PoisonError::into_inner);
                if *c != Claim::Pending {
                    continue;
                }
                *c = Claim::Taken;
            }
            let result = match rt.reconfigure(job.modules, table) {
                Ok(()) => {
                    changed = true;
                    Ok(catalog_of(rt, table, &job.change))
                }
                Err(errs) => Err(errs.iter().map(ToString::to_string).collect()),
            };
            let _ = job.reply.send(result);
        }
        changed
    }
}

/// Why the scan thread did not run a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapError {
    /// It refused the module set; nothing changed.
    Refused(Vec<String>),
    /// It did not take the job in time (or is gone); withdrawn, nothing
    /// changed.
    NotTaken,
}

impl SwapHandle {
    /// Hands `modules` (compiled from `change`) to the scan thread and
    /// waits for it.
    ///
    /// # Errors
    /// [`SwapError`]; in every case the running box is as it was.
    pub fn run(&self, modules: Vec<Module>, change: ChangeSet) -> Result<Catalog, SwapError> {
        let claim = Arc::new(Mutex::new(Claim::Pending));
        let (reply, answer) = mpsc::sync_channel(1);
        let job = Job {
            modules,
            change,
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
