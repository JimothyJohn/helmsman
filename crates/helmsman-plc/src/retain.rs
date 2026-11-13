//! `RETAIN` variables kept across a restart of the box: a reboot, a power
//! cut, a killed process. A deploy carries every variable already
//! ([`crate::Runtime::swap`]); this keeps the ones declared `VAR RETAIN`
//! when the process itself goes away.
//!
//! # On disk
//!
//! One file per ST module, `<state-dir>/retain/<module>.ret`:
//!
//! ```text
//! "HRT1"   len:u32 LE   crc:u32 LE (CRC-32/IEEE of the payload)   payload[len]
//! ```
//!
//! where the payload is [`helmsman_iec::State::encode`]: each RETAIN
//! variable's name, type and values. A write goes to a temporary file in
//! the same folder, is synced, renamed over the old file, and the folder
//! is synced, so a power cut leaves the old file or the new one, never
//! half of either.
//!
//! # When it writes
//!
//! The scan thread never touches the disk. At most every [`EVERY`] it
//! hands the modules' RETAIN values to a writer thread through a
//! latest-wins slot ([`Writer::offer`], which only tries the slot's lock:
//! a busy writer means the next offer, not a wait). The writer writes the
//! modules whose values changed since its last write, so a box whose
//! RETAIN values sit still does not write at all. A power cut loses at
//! most the last [`EVERY`] of changes; a planned stop (SIGTERM, SIGINT)
//! makes a last offer ([`Writer::offer_now`]) and waits for it
//! ([`Writer::flush`]) first. A module a deploy removes, or one
//! that no longer declares RETAIN, has its file removed.
//!
//! # At start
//!
//! [`restore`] reads each ST module's file and restores it by name and
//! type ([`helmsman_iec::Program::restore_retain`]), the rules a deploy
//! carries state by: a variable whose type changed starts at its initial
//! value and is named, as is a saved one the program no longer declares
//! RETAIN. A file that is damaged (bad magic, short, failed CRC, does not
//! decode) or bigger than [`MAX_FILE`] is reported, moved aside to
//! `<module>.ret.damaged`, and the module starts from its initial values.
//! The checker caps RETAIN at [`helmsman_iec::MAX_RETAIN`] values per
//! module, and a state that would still encode past [`MAX_FILE`] is
//! refused and reported on stderr (again whenever the reason changes): nothing is quietly
//! kept in part.
//!
//! A cold restart from the API ([`crate::Runtime::restart`]) resets RETAIN
//! variables too, as IEC 61131-3 has it; the writer then saves those.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread;
use std::time::Duration;

use helmsman_iec::State;

use crate::runtime::{Module, Runtime};
use crate::sink::queue::crc32;
use crate::spec::name_ok;

/// The folder under the state directory.
pub const DIR: &str = "retain";
/// A module's file: `<module>.ret`.
pub const EXT: &str = "ret";
/// The file's first four bytes.
pub const MAGIC: &[u8; 4] = b"HRT1";
/// Magic, length and CRC.
const HEAD: usize = 12;
/// Largest file read or written. The checker's cap keeps any program well
/// under it (17 bytes a value at most, plus the type's names).
pub const MAX_FILE: usize = 8 << 20;
/// How often, at most, the scan thread hands RETAIN values to the writer.
pub const EVERY: Duration = Duration::from_secs(1);

/// `<dir>/<module>.ret`, for a module name that is safe as a file name.
#[must_use]
pub fn path(dir: &Path, module: &str) -> Option<PathBuf> {
    name_ok(module).then(|| dir.join(format!("{module}.{EXT}")))
}

/// The file's bytes for `state`.
///
/// # Errors
/// A state that cannot be encoded, or one past [`MAX_FILE`].
pub fn encode(state: &State) -> Result<Vec<u8>, String> {
    let payload = state.encode().map_err(|e| e.to_string())?;
    if payload.len() + HEAD > MAX_FILE {
        return Err(format!(
            "{} bytes, more than the {MAX_FILE}-byte limit",
            payload.len() + HEAD
        ));
    }
    let len = u32::try_from(payload.len()).map_err(|_| "too long".to_string())?;
    let mut out = Vec::with_capacity(HEAD + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend(len.to_le_bytes());
    out.extend(crc32(&payload).to_le_bytes());
    out.extend(payload);
    Ok(out)
}

/// Reads a file's bytes back into a state.
///
/// # Errors
/// What is wrong with them.
pub fn decode(bytes: &[u8]) -> Result<State, String> {
    if bytes.len() > MAX_FILE {
        return Err(format!(
            "{} bytes, more than the {MAX_FILE}-byte limit",
            bytes.len()
        ));
    }
    if bytes.len() < HEAD || &bytes[..4] != MAGIC {
        return Err("not a RETAIN file (bad magic)".into());
    }
    let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let crc = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let payload = &bytes[HEAD..];
    if usize::try_from(len).ok() != Some(payload.len()) {
        return Err(format!("truncated: {} of {len} payload bytes", payload.len()));
    }
    if crc32(payload) != crc {
        return Err("its checksum does not match".into());
    }
    State::decode(payload).map_err(|e| format!("does not decode: {e}"))
}

/// The saved state of `module` in `dir`: `Ok(None)` when there is none.
///
/// # Errors
/// A file that cannot be read, is too big or is damaged.
pub fn read(dir: &Path, module: &str) -> Result<Option<State>, String> {
    let Some(p) = path(dir, module) else {
        return Ok(None);
    };
    let f = match File::open(&p) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    f.take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    decode(&bytes).map(Some)
}

static TMP: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` as `module`'s file: a temporary in the same folder,
/// synced, renamed over the old one, then the folder synced.
///
/// # Errors
/// The first I/O error; the old file is then as it was.
pub fn write(dir: &Path, module: &str, bytes: &[u8]) -> io::Result<()> {
    let Some(dest) = path(dir, module) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a module name"));
    };
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{module}.{EXT}.{}-{}",
        std::process::id(),
        TMP.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, &dest)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    File::open(dir)?.sync_all()
}

/// Removes `module`'s file, if there is one.
fn remove(dir: &Path, module: &str) -> io::Result<()> {
    let Some(p) = path(dir, module) else {
        return Ok(());
    };
    match fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Restores every ST module's RETAIN variables from `dir`, before the
/// runtime starts; returns what happened, a line per module that has
/// RETAIN variables or a saved file, for the log.
pub fn restore(dir: &Path, modules: &mut [Module]) -> Vec<String> {
    let mut lines = Vec::new();
    for m in modules {
        let name = m.name.clone();
        let Some(p) = m.program.st_mut() else {
            continue;
        };
        let saved = match read(dir, &name) {
            Ok(s) => s,
            Err(why) => {
                let file = path(dir, &name).unwrap_or_default();
                let aside = file.with_extension(format!("{EXT}.damaged"));
                let moved = fs::rename(&file, &aside).map_or_else(
                    |e| format!("it could not be moved aside: {e}"),
                    |()| format!("kept as {}", aside.display()),
                );
                lines.push(format!(
                    "module {name}: saved RETAIN values ignored, {} is {why} ({moved}); starting from initial values",
                    file.display()
                ));
                continue;
            }
        };
        let Some(saved) = saved else {
            if p.has_retain() {
                lines.push(format!(
                    "module {name}: no saved RETAIN values; starting from initial values"
                ));
            }
            continue;
        };
        let carry = p.restore_retain(&saved);
        let was: Vec<&str> = saved.names().collect();
        if !carry.kept.is_empty() {
            lines.push(format!(
                "module {name}: RETAIN restored: {}",
                carry.kept.join(", ")
            ));
        }
        let (changed, new): (Vec<&String>, Vec<&String>) =
            carry.reset.iter().partition(|n| was.contains(&n.as_str()));
        if !changed.is_empty() {
            lines.push(format!(
                "module {name}: RETAIN {} starts at its initial value: its type changed since it was saved",
                join(&changed)
            ));
        }
        if !new.is_empty() {
            lines.push(format!(
                "module {name}: RETAIN {} is new and starts at its initial value",
                join(&new)
            ));
        }
        if !carry.dropped.is_empty() {
            lines.push(format!(
                "module {name}: saved RETAIN {} dropped: no longer declared RETAIN",
                carry.dropped.join(", ")
            ));
        }
    }
    lines
}

fn join(names: &[&String]) -> String {
    names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

/// What the scan thread hands over: every module, with its RETAIN values
/// (`None`: it has none, or is not ST).
type Offer = Vec<(String, Option<State>)>;

#[derive(Default)]
struct Inner {
    pending: Option<Offer>,
    /// Offers made, and the newest the writer has finished.
    offered: u64,
    written: u64,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<Inner>,
    /// Signalled on an offer.
    work: Condvar,
    /// Signalled when an offer is written.
    done: Condvar,
}

/// The writer thread and the scan thread's side of its slot.
#[derive(Debug)]
pub struct Writer {
    shared: Arc<Shared>,
    last: Option<Duration>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

impl Writer {
    /// Starts the writer thread for files in `dir` (made on the first
    /// write). Problems go to stderr, once each until they clear.
    #[must_use]
    pub fn start(dir: PathBuf) -> Self {
        Self::start_with(dir, |line| eprintln!("helmsman-plc: {line}"))
    }

    /// [`Writer::start`], reporting through `report`.
    #[must_use]
    pub fn start_with(dir: PathBuf, report: impl Fn(&str) + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        let s = Arc::clone(&shared);
        thread::Builder::new()
            .name("retain".into())
            .spawn(move || write_loop(&dir, &s, &report))
            .expect("spawn the RETAIN writer");
        Self { shared, last: None }
    }

    /// Hands the runtime's RETAIN values to the writer if [`EVERY`] has
    /// passed since the last offer, without waiting: when the writer holds
    /// the slot, the next call tries again.
    pub fn offer(&mut self, now: Duration, rt: &Runtime) {
        if self.last.is_some_and(|l| now.saturating_sub(l) < EVERY) {
            return;
        }
        let inner = match self.shared.inner.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        hand_over(&self.shared, inner, rt);
        self.last = Some(now);
    }

    /// [`Writer::offer`] without the [`EVERY`] bound, waiting for the slot
    /// if the writer holds it: the last offer before a planned stop, which
    /// [`Writer::flush`] then waits out.
    pub fn offer_now(&mut self, now: Duration, rt: &Runtime) {
        let inner = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        hand_over(&self.shared, inner, rt);
        self.last = Some(now);
    }

    /// Waits until everything offered so far is written (or `timeout`
    /// passes); whether it was.
    pub fn flush(&self, timeout: Duration) -> bool {
        let inner = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let want = inner.offered;
        let (inner, _) = self
            .shared
            .done
            .wait_timeout_while(inner, timeout, |i| i.written < want)
            .unwrap_or_else(PoisonError::into_inner);
        inner.written >= want
    }
}

/// Puts the runtime's RETAIN values in the writer's slot and wakes it.
fn hand_over(shared: &Shared, mut inner: MutexGuard<'_, Inner>, rt: &Runtime) {
    let offer: Offer = rt
        .names()
        .map(|n| {
            let state = rt
                .program(n)
                .filter(|p| p.has_retain())
                .map(helmsman_iec::Program::retain_state);
            (n.to_string(), state)
        })
        .collect();
    inner.pending = Some(offer);
    inner.offered += 1;
    drop(inner);
    shared.work.notify_one();
}

/// The writer thread: forever, takes the newest offer and writes what
/// changed.
fn write_loop(dir: &Path, shared: &Shared, report: &dyn Fn(&str)) {
    // What each module's file holds, as this thread last left it (`None`:
    // no file); a module not in it has not been seen yet.
    let mut on_disk: HashMap<String, Option<Vec<u8>>> = HashMap::new();
    // Modules whose last write failed, to say so once.
    let mut failing: HashMap<String, String> = HashMap::new();
    loop {
        let (offer, n) = {
            let mut inner = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            while inner.pending.is_none() {
                inner = shared.work.wait(inner).unwrap_or_else(PoisonError::into_inner);
            }
            (inner.pending.take().unwrap_or_default(), inner.offered)
        };
        let names: Vec<&str> = offer.iter().map(|(n, _)| n.as_str()).collect();
        let gone: Vec<String> = on_disk
            .keys()
            .filter(|k| !names.contains(&k.as_str()))
            .cloned()
            .collect();
        for name in gone {
            if remove(dir, &name).is_ok() {
                on_disk.remove(&name);
            }
        }
        for (name, state) in &offer {
            let result = match state {
                None => {
                    if on_disk.get(name).is_some_and(Option::is_none) {
                        continue;
                    }
                    remove(dir, name)
                        .map(|()| None)
                        .map_err(|e| format!("its old RETAIN file could not be removed: {e}"))
                }
                Some(s) => match encode(s) {
                    Ok(bytes) if on_disk.get(name).and_then(Option::as_ref) == Some(&bytes) => continue,
                    Ok(bytes) => write(dir, name, &bytes)
                        .map(|()| Some(bytes))
                        .map_err(|e| format!("RETAIN values not saved: {e}")),
                    Err(e) => Err(format!("RETAIN values refused, not saved: {e}")),
                },
            };
            match result {
                Ok(file) => {
                    if failing.remove(name).is_some() {
                        report(&format!("module {name}: RETAIN values saved again"));
                    }
                    on_disk.insert(name.clone(), file);
                }
                Err(why) => {
                    if failing.get(name) != Some(&why) {
                        report(&format!("module {name}: {why}"));
                        failing.insert(name.clone(), why);
                    }
                }
            }
        }
        let mut inner = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.written = inner.written.max(n);
        drop(inner);
        shared.done.notify_all();
    }
}
