//! WASM drivers as files (`wasm-driver-home`: a file path on the box).
//!
//! ```text
//! <state-dir>/drivers/          the box's drivers ([`DriverDir`], the
//!   modbus.wasm                 engineer's `/api/drivers`)
//! <state-dir>/projects/line/
//!   drivers/                    a project's own drivers, carried in its .zip
//!     modbus.wasm
//! ```
//!
//! A map line `device ID wasm:NAME ...` runs on `drivers/NAME.wasm`. Where
//! it is looked for is [`Sources`]: the project's folder first (the project
//! being deployed, else the one recorded as running), then the box's, so a
//! project that carries a driver runs the bytes it was written and tested
//! with wherever it goes, and the box's folder supplies the rest. The first
//! file found is the one: a bad project file never falls back to the box's.
//!
//! - **Names** are slugs ([`helmsman_project::driver_name`]: 1..=48 of `a-z
//!   0-9 _ -`, starting with a letter or digit), so `wasm:NAME` is one path
//!   segment and two drivers never differ by case alone; a map line naming
//!   anything else (`wasm:../x`, `wasm:Modbus`) never reaches the file
//!   system.
//! - **Reads** take regular files only, at most
//!   [`helmsman_project::MAX_DRIVER`] bytes: a symbolic link (the file or
//!   the folder) is refused, never followed, and on Unix the file opened
//!   must be the one looked at (same device and inode).
//! - **Checked on load**: each file must pass [`crate::drivers::check`]
//!   (`helmsman-driver/v1` and nothing else imported, an instance starts
//!   under the default limits), and the map's points must parse on it
//!   ([`crate::drivers::Points`]). A bad file is a map error naming the
//!   file, and nothing loads.
//! - **Writes** ([`DriverDir::put`], the API's upload) are checked the same
//!   way first, then written atomically (a temporary file renamed over the
//!   old one), so a load reads a file whole, before or after.
//! - **When a change takes effect**: drivers are read when a map loads (at
//!   boot, and when an apply, rollback or deploy loads the map), never while
//!   one runs. A link's identity includes its driver's SHA-256
//!   (`link::LinkSet::plan_with`), so a changed file under an unchanged map
//!   line replaces that device's link at the next load, between scans.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use helmsman_io::IoMap;
use helmsman_io::driver::wasm_name;
pub use helmsman_project::{DRIVER_EXT, MAX_DRIVER, driver_name};

use crate::drivers::{Drivers, MAX_DRIVERS};
use crate::layout::Layout;

/// The box's driver folder in its state directory, and a project's in
/// its folder.
pub const DIR: &str = helmsman_project::DRIVERS;

/// SHA-256 of `bytes`, hex.
#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    crate::project::version(bytes)
}

/// One driver file, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    pub bytes: usize,
    pub sha256: String,
    /// Why it would not load (empty: it would).
    pub problems: Vec<String>,
}

impl Info {
    fn of(name: &str, bytes: &[u8]) -> Self {
        Self {
            name: name.to_string(),
            bytes: bytes.len(),
            sha256: sha256(bytes),
            problems: check(bytes).err().unwrap_or_default(),
        }
    }
}

/// Why a call on the box's driver folder failed: the status the API
/// answers, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub why: String,
    /// Every problem a refused binary has.
    pub problems: Vec<String>,
}

impl Refusal {
    fn new(status: u16, why: impl Into<String>) -> Self {
        Self {
            status,
            why: why.into(),
            problems: Vec::new(),
        }
    }

    fn io(e: &io::Error) -> Self {
        // The path is the box's own; the kind is enough for the caller.
        Self::new(500, format!("the driver folder failed: {}", e.kind()))
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.why)
    }
}

/// Whether `bytes` would run as a driver, every problem prefixed with the
/// interface.
fn check(bytes: &[u8]) -> Result<(), Vec<String>> {
    crate::drivers::check(bytes).map_err(|errs| {
        errs.iter()
            .map(|e| format!("{}: {e}", helmsman_io::wasm::ABI))
            .collect()
    })
}

/// The file name of driver `name`.
fn file(name: &str) -> String {
    format!("{name}{DRIVER_EXT}")
}

/// Whether `dir` is a folder: `Ok(false)` if there is nothing there, an
/// error if it is a link or anything but a folder.
fn folder(dir: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_dir() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a folder (links are not followed)",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Reads the regular file at `path`, at most [`MAX_DRIVER`] bytes:
/// `Ok(None)` if there is none. A link is refused, never followed.
fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    // An upload renames a new file over the old between the look and the
    // open now and then: look again. A link swapped in fails every time.
    let mut tries = 0;
    loop {
        match read_once(path) {
            Err(Swapped) if tries < 8 => tries += 1,
            Err(Swapped) => return Err("replaced while it was read (links are not followed)".into()),
            Ok(r) => return r,
        }
    }
}

/// The file at a path changed between looking at it and opening it.
struct Swapped;

/// One try of [`read`]: the outer error is a swap (try again), the inner
/// one an answer.
fn read_once(path: &Path) -> Result<Result<Option<Vec<u8>>, String>, Swapped> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Ok(None)),
        Err(e) => return Ok(Err(format!("cannot be read ({})", e.kind()))),
    };
    if !meta.file_type().is_file() {
        return Ok(Err("not a regular file (links are not followed)".into()));
    }
    let too_big = || format!("larger than {MAX_DRIVER} bytes");
    if meta.len() > MAX_DRIVER as u64 {
        return Ok(Err(too_big()));
    }
    let f = match fs::File::open(path) {
        Ok(f) => f,
        // Renamed away between the look and the open.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Swapped),
        Err(e) => return Ok(Err(format!("cannot be read ({})", e.kind()))),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let opened = match f.metadata() {
            Ok(m) => m,
            Err(e) => return Ok(Err(format!("cannot be read ({})", e.kind()))),
        };
        if (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
            return Err(Swapped);
        }
    }
    let mut buf = Vec::new();
    if let Err(e) = f.take(MAX_DRIVER as u64 + 1).read_to_end(&mut buf) {
        return Ok(Err(format!("cannot be read ({})", e.kind())));
    }
    if buf.len() > MAX_DRIVER {
        return Ok(Err(too_big()));
    }
    Ok(Ok(Some(buf)))
}

/// Every driver file in `dir` (a box's driver folder or a project's
/// `drivers/`), by name: nothing if there is no folder. Other entries
/// (temporaries, files not named `NAME.wasm`) are left out.
///
/// # Errors
/// The folder is a link or cannot be read, or a driver file is not a
/// regular file, is too large or cannot be read (one line naming it).
pub fn read_folder(dir: &Path) -> Result<Drivers, String> {
    let mut out = Drivers::new();
    for (name, got) in entries(dir)? {
        let bytes = got.map_err(|why| format!("{DIR}/{}: {why}", file(&name)))?;
        out.insert(name, bytes);
    }
    Ok(out)
}

/// Each `NAME.wasm` in `dir`, by name, read or why not; nothing if there
/// is no folder.
fn entries(dir: &Path) -> Result<BTreeMap<String, Result<Vec<u8>, String>>, String> {
    let mut out = BTreeMap::new();
    if !folder(dir).map_err(|e| format!("{DIR}/: {e}"))? {
        return Ok(out);
    }
    let list = fs::read_dir(dir).map_err(|e| format!("{DIR}/: cannot be read ({})", e.kind()))?;
    for e in list {
        let e = e.map_err(|e| format!("{DIR}/: cannot be read ({})", e.kind()))?;
        let Some(name) = e
            .file_name()
            .to_str()
            .and_then(|f| f.strip_suffix(DRIVER_EXT))
            .filter(|n| driver_name(n))
            .map(str::to_string)
        else {
            continue;
        };
        match read(&e.path()) {
            Ok(Some(b)) => {
                out.insert(name, Ok(b));
            }
            Ok(None) => {}
            Err(why) => {
                out.insert(name, Err(why));
            }
        }
    }
    Ok(out)
}

static TMP: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` as `dir/name` through a temporary file renamed over it.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(
        ".put-{}-{}",
        std::process::id(),
        TMP.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, dir.join(name))) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// The box's driver folder (`<state-dir>/drivers/`), made on the first
/// upload.
#[derive(Debug)]
pub struct DriverDir {
    root: PathBuf,
    /// Held by every write.
    lock: Mutex<()>,
}

impl DriverDir {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self {
            root,
            lock: Mutex::new(()),
        }
    }

    /// `<state>/drivers`.
    #[must_use]
    pub fn in_state_dir(state: &Path) -> Self {
        Self::new(state.join(DIR))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every driver file, by name, with what [`crate::drivers::check`]
    /// says of it.
    ///
    /// # Errors
    /// The folder or a file in it cannot be used (500).
    pub fn list(&self) -> Result<Vec<Info>, Refusal> {
        let all = entries(&self.root).map_err(|e| Refusal::new(500, e))?;
        Ok(all
            .iter()
            .map(|(n, got)| match got {
                Ok(b) => Info::of(n, b),
                Err(why) => Info {
                    name: n.clone(),
                    bytes: 0,
                    sha256: String::new(),
                    problems: vec![why.clone()],
                },
            })
            .collect())
    }

    /// One driver's bytes; `None` if there is no such file.
    ///
    /// # Errors
    /// A bad name (404), or the file cannot be used (500).
    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>, Refusal> {
        if !driver_name(name) {
            return Err(Refusal::new(404, "no such driver"));
        }
        folder(&self.root).map_err(|e| Refusal::new(500, format!("{DIR}/: {e}")))?;
        read(&self.root.join(file(name))).map_err(|e| Refusal::new(500, format!("{DIR}/{}: {e}", file(name))))
    }

    /// Writes driver `name` (new, or in place of the one there), once it
    /// passes the check a load holds it to; whether it replaced one.
    ///
    /// # Errors
    /// A bad name (400), too large (413), not a driver this box can run
    /// (422, every problem), too many drivers (507), a write failure (500).
    pub fn put(&self, name: &str, bytes: &[u8]) -> Result<(Info, bool), Refusal> {
        if !driver_name(name) {
            return Err(Refusal::new(
                400,
                format!(
                    "a driver name is 1 to {} of a-z 0-9 _ -, starting with a letter or digit",
                    helmsman_project::NAME_LEN
                ),
            ));
        }
        if bytes.len() > MAX_DRIVER {
            return Err(Refusal::new(
                413,
                format!("a driver is at most {MAX_DRIVER} bytes"),
            ));
        }
        if let Err(problems) = check(bytes) {
            return Err(Refusal {
                problems,
                ..Refusal::new(422, "not a driver this box can run")
            });
        }
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        if !folder(&self.root).map_err(|e| Refusal::new(500, format!("{DIR}/: {e}")))? {
            fs::create_dir_all(&self.root).map_err(|e| Refusal::io(&e))?;
            folder(&self.root).map_err(|e| Refusal::new(500, format!("{DIR}/: {e}")))?;
        }
        let path = self.root.join(file(name));
        let replaced = match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => true,
            Ok(_) => {
                return Err(Refusal::new(
                    409,
                    format!("{DIR}/{} is not a regular file; remove it by hand", file(name)),
                ));
            }
            Err(_) => false,
        };
        if !replaced {
            let n = entries(&self.root).map_err(|e| Refusal::new(500, e))?.len();
            if n >= MAX_DRIVERS {
                return Err(Refusal::new(
                    507,
                    format!("the box holds at most {MAX_DRIVERS} drivers"),
                ));
            }
        }
        write_atomic(&self.root, &file(name), bytes).map_err(|e| Refusal::io(&e))?;
        Ok((Info::of(name, bytes), replaced))
    }

    /// Deletes driver `name`'s file.
    ///
    /// # Errors
    /// No such driver (404), a removal failure (500).
    pub fn delete(&self, name: &str) -> Result<(), Refusal> {
        if !driver_name(name) {
            return Err(Refusal::new(404, "no such driver"));
        }
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        if !folder(&self.root).unwrap_or(false) {
            return Err(Refusal::new(404, "no such driver"));
        }
        let path = self.root.join(file(name));
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            return Err(Refusal::new(404, "no such driver"));
        }
        fs::remove_file(&path).map_err(|e| Refusal::io(&e))
    }
}

/// Where a driver file was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// In this project's `drivers/`.
    Project(String),
    /// In the box's driver folder.
    Box,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(p) => write!(f, "project {p}"),
            Self::Box => write!(f, "the box"),
        }
    }
}

/// A driver file's bytes once checked, or every line on why not.
type Loaded = Result<Vec<u8>, Vec<String>>;

/// Where a load looks for `wasm:NAME`, in order: the project's
/// `drivers/`, then the box's driver folder.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// A project's name and its folder.
    pub project: Option<(String, PathBuf)>,
    /// The box's driver folder.
    pub boxed: Option<PathBuf>,
}

/// The drivers a map names, as found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// By name: each file's bytes, checked.
    pub drivers: Drivers,
    /// Where each came from.
    pub origin: BTreeMap<String, Origin>,
    /// One line per device whose driver file is there but will not load.
    pub problems: Vec<String>,
}

impl Sources {
    /// The box's folder alone.
    #[must_use]
    pub fn boxed(dir: &Path) -> Self {
        Self {
            project: None,
            boxed: Some(dir.to_path_buf()),
        }
    }

    /// Where to look, first first: each source and its `drivers/` folder.
    fn order(&self) -> Vec<(Origin, PathBuf)> {
        let mut v = Vec::new();
        if let Some((name, dir)) = &self.project {
            v.push((Origin::Project(name.clone()), dir.join(DIR)));
        }
        if let Some(dir) = &self.boxed {
            v.push((Origin::Box, dir.clone()));
        }
        v
    }

    /// Driver `name`: the first source with a file for it, and its bytes
    /// or why it will not load; `None` when no source has one.
    fn one(&self, name: &str) -> Option<(Origin, Loaded)> {
        for (origin, dir) in self.order() {
            let shown = format!("{DIR}/{} in {origin}", file(name));
            let got = match folder(&dir) {
                Ok(false) => continue,
                Ok(true) => read(&dir.join(file(name))),
                Err(e) => Err(format!("its {DIR}/ folder: {e}")),
            };
            return Some(match got {
                Ok(None) => continue,
                Ok(Some(bytes)) => (
                    origin,
                    check(&bytes)
                        .map(|()| bytes)
                        .map_err(|errs| errs.iter().map(|e| format!("{shown}: {e}")).collect()),
                ),
                Err(why) => (origin, Err(vec![format!("{shown}: {why}")])),
            });
        }
        None
    }

    /// The drivers `map` names (`wasm:NAME`), each from the first source
    /// that has a file for it, checked. A name that is not a slug, or a
    /// file that will not load, is a problem naming the device and the
    /// file; a name no source has is left to the map's own check
    /// ([`crate::drivers::unknown`]).
    #[must_use]
    pub fn find(&self, map: &IoMap) -> Found {
        let mut found = Found::default();
        let mut seen: BTreeMap<&str, Option<Result<(), Vec<String>>>> = BTreeMap::new();
        for d in &map.devices {
            let Some(name) = wasm_name(&d.driver) else {
                continue;
            };
            if !driver_name(name) {
                found.problems.push(format!(
                    "device {}: {}: not a driver name (1 to {} of a-z 0-9 _ -, starting with a letter or digit)",
                    d.id,
                    helmsman_project::clean(&d.driver),
                    helmsman_project::NAME_LEN
                ));
                continue;
            }
            let state = seen.entry(name).or_insert_with(|| {
                self.one(name).map(|(origin, got)| {
                    got.map(|bytes| {
                        found.drivers.insert(name.to_string(), bytes);
                        found.origin.insert(name.to_string(), origin);
                    })
                })
            });
            if let Some(Err(lines)) = state {
                found.problems.extend(
                    lines
                        .iter()
                        .map(|l| format!("device {}: {}: {l}", d.id, d.driver)),
                );
            }
        }
        found
    }

    /// [`Sources::find`] for a map's text; nothing for no map or one that
    /// does not parse (its load says why).
    #[must_use]
    pub fn find_text(&self, io_map: Option<&str>) -> Found {
        io_map
            .and_then(|t| helmsman_io::text::parse(t).ok())
            .map_or_else(Found::default, |f| self.find(&f.map))
    }
}

/// The map and calendar loaded with the drivers `found` for that map: its
/// problems if it has any (a bad driver file), else
/// [`Layout::build_with`] on its drivers.
///
/// # Errors
/// Every problem, each line naming the file it is in.
pub fn layout(io_map: Option<&str>, calendar: Option<&str>, found: &Found) -> Result<Layout, Vec<String>> {
    if !found.problems.is_empty() {
        return Err(found.problems.iter().map(|p| format!("io.map: {p}")).collect());
    }
    Layout::build_with(io_map, calendar, &found.drivers)
}
