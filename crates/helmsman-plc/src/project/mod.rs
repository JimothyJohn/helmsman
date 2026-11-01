//! Projects on the box: folders of Structured Text an engineer edits in
//! the browser, of which one runs (docs/UI.md, "Projects").
//!
//! ```text
//! <state-dir>/projects/
//!   line3/                 a project: its name is the folder's
//!     main.st              free-form ST: any PROGRAMs, FUNCTION_BLOCKs,
//!     blocks.st            FUNCTIONs and TYPEs, compiled together
//!     io.map               optional: the I/O map it runs with (helmsman_io::text)
//!     project.settings     optional: per-PROGRAM schedule, windows, params
//! <state-dir>/running-project   which project was last deployed, and as what
//! ```
//!
//! - **Names.** A project is 1..=48 of `a-z 0-9 _ -`, starting with a
//!   letter or digit ([`project_name`]): one path segment, lower case so
//!   two projects never differ by case alone (a case-folding file system
//!   would merge them). A file is `NAME.st` with `NAME` 1..=64 of `A-Z a-z
//!   0-9 _ -` starting with a letter or digit, or one of [`IO_MAP`],
//!   [`SETTINGS`] and [`LAYOUT`] (the diagram's shared arrangement, which
//!   the build ignores; [`file_name`]); two files that differ only by
//!   case are refused. Checked after percent-decoding, so `..`, `%2e%2e`, `a/b`,
//!   `a\b`, `C:x` and NUL never name anything.
//! - **Contents.** UTF-8 without NUL, other control characters (tab, CR
//!   and LF aside) or bidirectional overrides (no "Trojan Source" in a
//!   program an engineer reviews); at most [`MAX_FILE`] bytes a file,
//!   [`MAX_FILES`] `.st` files and [`MAX_TOTAL`] bytes a project.
//! - **Writes are atomic** (a temporary file renamed over the old one, in
//!   the same folder) and serialized for the whole store, so a reader (a
//!   deploy, a download) sees each file as it was before or after a save,
//!   never half of one. Nothing below the projects folder is followed if
//!   it is a symbolic link.
//! - **Drafts, not what runs.** Saving changes the folder only. What runs
//!   changes on deploy ([`build`], then the approval path), which records
//!   the project's name and snapshot in `running-project`; the box boots
//!   the store's head (`--projects`), which is that snapshot until a
//!   later apply or rollback moves it.

pub mod build;
pub mod settings;

pub use helmsman_project::{
    FILE_NAMES, FileKind, Files, IO_MAP, LAYOUT, MAX_FILE, MAX_FILES, MAX_TOTAL, NAME_LEN, SETTINGS,
    STEM_LEN, check_file, check_text, file_name, from_zip, layout, project_name, zip,
};

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use sha2::{Digest as _, Sha256};

/// Most projects on a box.
pub const MAX_PROJECTS: usize = 256;
/// The file in the state directory naming the running project.
pub const RUNNING: &str = "running-project";

/// SHA-256 of `bytes`, hex: a file's version, for `if_match` and listings.
#[must_use]
pub fn version(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in d {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Why a store call failed: the status the API answers and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub why: String,
}

impl Refusal {
    fn new(status: u16, why: impl Into<String>) -> Self {
        Self {
            status,
            why: why.into(),
        }
    }

    fn io(e: &io::Error) -> Self {
        // The path is the box's own; the kind is enough for the caller.
        Self::new(500, format!("the project store failed: {}", e.kind()))
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.why)
    }
}

type R<T> = Result<T, Refusal>;

/// One file in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub name: String,
    pub bytes: usize,
    pub version: String,
}

/// Which project was last deployed, and as which snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub name: String,
    pub snapshot: String,
}

/// The projects folder of one box.
#[derive(Debug)]
pub struct Projects {
    root: PathBuf,
    running: PathBuf,
    /// Held by every write, and by reads that must see a whole project
    /// at one moment (deploy, download).
    lock: Mutex<()>,
}

static TMP: AtomicU64 = AtomicU64::new(0);

/// A name for a temporary file or folder beside the real ones: dot-led,
/// so never a project or file name.
fn tmp_name(what: &str) -> String {
    format!(
        ".{what}-{}-{}",
        std::process::id(),
        TMP.fetch_add(1, Ordering::Relaxed)
    )
}

/// Whether `path` is a real directory (not a link to one).
fn is_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(tmp_name("save"));
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

/// Reads a regular file of at most `cap` bytes, refusing links.
fn read_file(path: &Path, cap: usize) -> io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a regular file"));
    }
    let mut buf = Vec::new();
    fs::File::open(path)?.take(cap as u64 + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

impl Projects {
    /// The projects under `root` (created if missing); the running
    /// project is recorded in `running`.
    ///
    /// # Errors
    /// `root` cannot be made, or is a link.
    pub fn open(root: PathBuf, running: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        if !is_dir(&root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the projects folder is not a directory",
            ));
        }
        Ok(Self {
            root,
            running,
            lock: Mutex::new(()),
        })
    }

    /// `<state>/projects`, recording the running project in
    /// `<state>/running-project`.
    ///
    /// # Errors
    /// As [`Projects::open`].
    pub fn in_state_dir(state: &Path) -> io::Result<Self> {
        Self::open(state.join("projects"), state.join(RUNNING))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn dir(&self, name: &str) -> R<PathBuf> {
        if !project_name(name) {
            return Err(Refusal::new(404, "no such project"));
        }
        let dir = self.root.join(name);
        if !is_dir(&dir) {
            return Err(Refusal::new(404, "no such project"));
        }
        Ok(dir)
    }

    /// Every project's name, sorted.
    ///
    /// # Errors
    /// The folder cannot be read.
    pub fn list(&self) -> R<Vec<String>> {
        let mut names = Vec::new();
        for e in fs::read_dir(&self.root).map_err(|e| Refusal::io(&e))? {
            let e = e.map_err(|e| Refusal::io(&e))?;
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if project_name(&name) && e.file_type().is_ok_and(|t| t.is_dir()) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// A project's files with their sizes and versions, by name.
    ///
    /// # Errors
    /// No such project.
    pub fn files(&self, name: &str) -> R<Vec<FileInfo>> {
        let files = self.read(name)?;
        Ok(files
            .0
            .iter()
            .map(|(n, t)| FileInfo {
                name: n.clone(),
                bytes: t.len(),
                version: version(t.as_bytes()),
            })
            .collect())
    }

    /// Every file of a project, at one moment.
    ///
    /// # Errors
    /// No such project, or a file that breaks the rules (one put there by
    /// hand, not through the store).
    pub fn read(&self, name: &str) -> R<Files> {
        let _g = self.guard();
        self.read_locked(name)
    }

    fn read_locked(&self, name: &str) -> R<Files> {
        let dir = self.dir(name)?;
        let mut files = BTreeMap::new();
        for e in fs::read_dir(&dir).map_err(|e| Refusal::io(&e))? {
            let e = e.map_err(|e| Refusal::io(&e))?;
            let Some(file) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if file_name(&file).is_none() {
                continue; // temporaries, and anything not ours
            }
            let bytes = read_file(&e.path(), MAX_FILE).map_err(|e| Refusal::io(&e))?;
            let text = check_text(&bytes).map_err(|e| Refusal::new(500, format!("{file} on disk: {e}")))?;
            files.insert(file, text.to_string());
        }
        Ok(Files(files))
    }

    /// One file's text.
    ///
    /// # Errors
    /// No such project or file.
    pub fn file(&self, name: &str, file: &str) -> R<String> {
        let files = self.read(name)?;
        if file_name(file).is_none() {
            return Err(Refusal::new(404, "no such file"));
        }
        files
            .0
            .get(file)
            .cloned()
            .ok_or_else(|| Refusal::new(404, "no such file"))
    }

    /// Makes an empty project.
    ///
    /// # Errors
    /// A bad name (400), one that exists (409), or too many projects (507).
    pub fn create(&self, name: &str) -> R<()> {
        self.install(name, &Files::default(), false)
    }

    /// Writes `files` as project `name`, all at once: a new project, or
    /// (with `replace`) in place of the one there.
    ///
    /// # Errors
    /// A bad name (400), files that break the rules (422), an existing
    /// project without `replace` (409), too many projects (507).
    pub fn install(&self, name: &str, files: &Files, replace: bool) -> R<()> {
        if !project_name(name) {
            return Err(Refusal::new(
                400,
                format!("a project name is 1 to {NAME_LEN} of a-z 0-9 _ -, starting with a letter or digit"),
            ));
        }
        files.check().map_err(|e| Refusal::new(422, e.join("; ")))?;
        let _g = self.guard();
        let dest = self.root.join(name);
        let exists = fs::symlink_metadata(&dest).is_ok();
        if exists && !replace {
            return Err(Refusal::new(409, "a project of that name exists"));
        }
        if exists && !is_dir(&dest) {
            return Err(Refusal::new(
                409,
                "that name is taken by something that is not a project",
            ));
        }
        if !exists && self.list()?.len() >= MAX_PROJECTS {
            return Err(Refusal::new(
                507,
                format!("the box holds at most {MAX_PROJECTS} projects"),
            ));
        }
        let tmp = self.root.join(tmp_name("new"));
        let made = (|| {
            fs::create_dir(&tmp)?;
            for (f, text) in &files.0 {
                write_atomic(&tmp, f, text.as_bytes())?;
            }
            Ok::<_, io::Error>(())
        })();
        if let Err(e) = made {
            let _ = fs::remove_dir_all(&tmp);
            return Err(Refusal::io(&e));
        }
        if exists {
            let old = self.root.join(tmp_name("old"));
            fs::rename(&dest, &old).map_err(|e| {
                let _ = fs::remove_dir_all(&tmp);
                Refusal::io(&e)
            })?;
            if let Err(e) = fs::rename(&tmp, &dest) {
                let _ = fs::rename(&old, &dest);
                let _ = fs::remove_dir_all(&tmp);
                return Err(Refusal::io(&e));
            }
            let _ = fs::remove_dir_all(&old);
        } else if let Err(e) = fs::rename(&tmp, &dest) {
            let _ = fs::remove_dir_all(&tmp);
            return Err(Refusal::io(&e));
        }
        Ok(())
    }

    /// Copies project `from` to a new project `to`.
    ///
    /// # Errors
    /// No such project (404), or as [`Projects::install`].
    pub fn copy(&self, from: &str, to: &str) -> R<()> {
        let files = self.read(from)?;
        self.install(to, &files, false)
    }

    /// Renames a project. The running project's record follows it.
    ///
    /// # Errors
    /// No such project (404), a bad new name (400), one that exists (409).
    pub fn rename(&self, from: &str, to: &str) -> R<()> {
        if !project_name(to) {
            return Err(Refusal::new(400, "not a project name"));
        }
        let _g = self.guard();
        let src = self.dir(from)?;
        let dest = self.root.join(to);
        if fs::symlink_metadata(&dest).is_ok() {
            return Err(Refusal::new(409, "a project of that name exists"));
        }
        fs::rename(&src, &dest).map_err(|e| Refusal::io(&e))?;
        if let Some(r) = self.running()
            && r.name == from
        {
            self.record(&Running {
                name: to.to_string(),
                snapshot: r.snapshot,
            })
            .map_err(|e| Refusal::io(&e))?;
        }
        Ok(())
    }

    /// Deletes a project; never the running one (409).
    ///
    /// # Errors
    /// No such project (404), or it is the one running (409).
    pub fn delete(&self, name: &str) -> R<()> {
        let _g = self.guard();
        let dir = self.dir(name)?;
        if self.running().is_some_and(|r| r.name == name) {
            return Err(Refusal::new(
                409,
                "this project is the one running; deploy another before deleting it",
            ));
        }
        let gone = self.root.join(tmp_name("gone"));
        fs::rename(&dir, &gone).map_err(|e| Refusal::io(&e))?;
        fs::remove_dir_all(&gone).map_err(|e| Refusal::io(&e))
    }

    /// Saves one file (a draft). With `if_match`, only if the file is still
    /// that version (`None` inside: only if it does not exist yet).
    ///
    /// # Errors
    /// No such project (404), a bad name (400), bad contents (422), a
    /// version mismatch (409), a project grown past the caps (413).
    pub fn save(&self, name: &str, file: &str, text: &[u8], if_match: Option<Option<&str>>) -> R<String> {
        if file_name(file).is_none() {
            return Err(Refusal::new(400, format!("a project file is {FILE_NAMES}")));
        }
        let text = check_file(file, text).map_err(|e| Refusal::new(422, format!("{file}: {e}")))?;
        let _g = self.guard();
        let mut files = self.read_locked(name)?;
        let current = files.0.get(file).map(|t| version(t.as_bytes()));
        if let Some(want) = if_match
            && want != current.as_deref()
        {
            return Err(Refusal::new(
                409,
                "the file changed since it was read (or exists already); reload it",
            ));
        }
        if current.is_none()
            && let Some(other) = files.0.keys().find(|k| k.eq_ignore_ascii_case(file))
        {
            return Err(Refusal::new(
                409,
                format!("{other} exists; names differ only by case"),
            ));
        }
        files.0.insert(file.to_string(), text.to_string());
        if files.st().count() > MAX_FILES || files.total() > MAX_TOTAL {
            return Err(Refusal::new(
                413,
                format!("a project holds at most {MAX_FILES} .st files and {MAX_TOTAL} bytes"),
            ));
        }
        write_atomic(&self.dir(name)?, file, text.as_bytes()).map_err(|e| Refusal::io(&e))?;
        Ok(version(text.as_bytes()))
    }

    /// Deletes one file.
    ///
    /// # Errors
    /// No such project or file (404).
    pub fn remove(&self, name: &str, file: &str) -> R<()> {
        let _g = self.guard();
        let dir = self.dir(name)?;
        if file_name(file).is_none() {
            return Err(Refusal::new(404, "no such file"));
        }
        let path = dir.join(file);
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            return Err(Refusal::new(404, "no such file"));
        }
        fs::remove_file(&path).map_err(|e| Refusal::io(&e))
    }

    /// The project as a ZIP (its files at the archive's root, sorted).
    ///
    /// # Errors
    /// No such project.
    pub fn zip(&self, name: &str) -> R<Vec<u8>> {
        let files = self.read(name)?;
        let list: Vec<(String, Vec<u8>)> = files.0.into_iter().map(|(n, t)| (n, t.into_bytes())).collect();
        zip::write(&list).map_err(|e| Refusal::new(500, e))
    }

    /// Which project was last deployed, if the record is readable.
    #[must_use]
    pub fn running(&self) -> Option<Running> {
        let bytes = read_file(&self.running, 1024).ok()?;
        let text = std::str::from_utf8(&bytes).ok()?;
        let (name, snapshot) = text.trim_end().split_once(' ')?;
        (project_name(name) && snapshot.len() == 64 && snapshot.bytes().all(|b| b.is_ascii_hexdigit())).then(
            || Running {
                name: name.to_string(),
                snapshot: snapshot.to_string(),
            },
        )
    }

    /// Records `r` as the running project.
    ///
    /// # Errors
    /// The record cannot be written.
    pub fn record(&self, r: &Running) -> io::Result<()> {
        let dir = self.running.parent().unwrap_or(Path::new("."));
        let name = self
            .running
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(RUNNING);
        write_atomic(dir, name, format!("{} {}\n", r.name, r.snapshot).as_bytes())
    }
}

/// What [`seed`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seeded {
    /// The project was made and deployed as this snapshot.
    Deployed { snapshot: String },
    /// Not a first install (a project exists, or something was deployed):
    /// nothing was touched.
    Left(String),
}

/// A first install's project: when the box has no project and nothing
/// deployed, makes project `name` from `files` and commits its build as
/// the store's first snapshot (author `project NAME`, approver `first
/// install`), recorded as the running project, so the box boots it. On
/// any later install it touches nothing, so it never clobbers a project
/// or brings back one an engineer deleted. There is no approval to ask
/// for: nothing runs yet, and only someone with root on the box runs it
/// (`helmsman-plc project seed`, the service's `ExecStartPre`).
///
/// # Errors
/// The folders or store cannot be used, a file breaks the rules, or the
/// project does not build (every diagnostic, by file and line); a failed
/// build leaves the project made, for an engineer to fix.
pub fn seed(
    state: &Path,
    name: &str,
    files: &[(String, String)],
    shape: &build::Shape,
    time_ms: i64,
) -> Result<Seeded, String> {
    let projects =
        Projects::in_state_dir(state).map_err(|e| format!("projects in {}: {e}", state.display()))?;
    let store_dir = state.join("store");
    let mut store =
        crate::store::Store::open(&store_dir).map_err(|e| format!("store {}: {e}", store_dir.display()))?;
    let existing = projects.list().map_err(|r| r.why)?;
    if !existing.is_empty() {
        return Ok(Seeded::Left(format!(
            "projects already here ({}); nothing seeded",
            existing.join(", ")
        )));
    }
    if let Some(h) = store.head().map_err(|e| e.to_string())? {
        return Ok(Seeded::Left(format!(
            "snapshot {} already deployed; nothing seeded",
            h.id
        )));
    }
    projects.create(name).map_err(|r| r.why)?;
    for (file, text) in files {
        projects
            .save(name, file, text.as_bytes(), Some(None))
            .map_err(|r| format!("{file}: {}", r.why))?;
    }
    let read = projects.read(name).map_err(|r| r.why)?;
    let built = build::build(name, &read, None, shape).map_err(|diags| {
        let lines: Vec<String> = diags.iter().map(ToString::to_string).collect();
        format!("project {name} does not build:\n  {}", lines.join("\n  "))
    })?;
    let meta = crate::store::Meta {
        author: format!("project {name}"),
        prompt: "seeded on first install".into(),
        approver: Some("first install".into()),
        time_ms,
    };
    let id = store.commit(&built.change, &meta).map_err(|e| e.to_string())?;
    projects
        .record(&Running {
            name: name.to_string(),
            snapshot: id.to_string(),
        })
        .map_err(|e| format!("the running-project record: {e}"))?;
    Ok(Seeded::Deployed {
        snapshot: id.to_string(),
    })
}
