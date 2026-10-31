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
//!   0-9 _ -` starting with a letter or digit, or one of [`IO_MAP`] and
//!   [`SETTINGS`] ([`file_name`]); two files that differ only by case are
//!   refused. Checked after percent-decoding, so `..`, `%2e%2e`, `a/b`,
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
pub mod zip;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use sha2::{Digest as _, Sha256};

/// The I/O map's file name in a project.
pub const IO_MAP: &str = "io.map";
/// The settings file's name in a project ([`settings`]).
pub const SETTINGS: &str = "project.settings";
/// Largest file, bytes.
pub const MAX_FILE: usize = 256 * 1024;
/// Most `.st` files in a project.
pub const MAX_FILES: usize = 64;
/// Largest project, all files together, bytes. Every module's source is
/// at most this (plus a header line per file), under the store's 1 MiB.
pub const MAX_TOTAL: usize = 900 * 1024;
/// Most projects on a box.
pub const MAX_PROJECTS: usize = 256;
/// Longest project name.
pub const NAME_LEN: usize = 48;
/// Longest `.st` file stem.
pub const STEM_LEN: usize = 64;
/// The file in the state directory naming the running project.
pub const RUNNING: &str = "running-project";

/// Whether `s` is a project name.
#[must_use]
pub fn project_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=NAME_LEN).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// What a file in a project is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    St,
    IoMap,
    Settings,
}

/// What `s` names in a project, if it names anything.
#[must_use]
pub fn file_name(s: &str) -> Option<FileKind> {
    match s {
        IO_MAP => return Some(FileKind::IoMap),
        SETTINGS => return Some(FileKind::Settings),
        _ => {}
    }
    let stem = s.strip_suffix(".st")?;
    let b = stem.as_bytes();
    ((1..=STEM_LEN).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-'))
    .then_some(FileKind::St)
}

/// Why a file's bytes are refused, if they are.
///
/// # Errors
/// Not UTF-8, too large, or a character the module docs refuse (named by
/// line).
pub fn check_text(bytes: &[u8]) -> Result<&str, String> {
    if bytes.len() > MAX_FILE {
        return Err(format!("larger than {MAX_FILE} bytes"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8".to_string())?;
    for (n, line) in text.split('\n').enumerate() {
        for c in line.chars() {
            let bidi =
                matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}');
            if (c.is_control() && c != '\t' && c != '\r') || bidi {
                return Err(format!(
                    "line {}: character U+{:04X} is not allowed (control or bidirectional formatting)",
                    n + 1,
                    u32::from(c)
                ));
            }
        }
    }
    Ok(text)
}

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

/// A project's files in memory, by name (sorted).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files(pub BTreeMap<String, String>);

impl Files {
    /// The `.st` files, by name.
    pub fn st(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .filter(|(n, _)| file_name(n) == Some(FileKind::St))
            .map(|(n, t)| (n.as_str(), t.as_str()))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// Bytes, all files together.
    #[must_use]
    pub fn total(&self) -> usize {
        self.0.values().map(String::len).sum()
    }

    /// Holds a whole set of files to the rules: names, contents, counts,
    /// sizes, and no two names that differ only by case.
    ///
    /// # Errors
    /// Every problem, one per file (or one for the set).
    pub fn check(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let mut folded: BTreeMap<String, &str> = BTreeMap::new();
        for (name, text) in &self.0 {
            if file_name(name).is_none() {
                errs.push(format!("{}: not a project file name", crate::api::clean(name)));
                continue;
            }
            if let Err(e) = check_text(text.as_bytes()) {
                errs.push(format!("{name}: {e}"));
            }
            if let Some(other) = folded.insert(name.to_ascii_lowercase(), name) {
                errs.push(format!("{name}: differs from {other} only by case"));
            }
        }
        let st = self.st().count();
        if st > MAX_FILES {
            errs.push(format!("{st} .st files; a project holds at most {MAX_FILES}"));
        }
        if self.total() > MAX_TOTAL {
            errs.push(format!(
                "{} bytes; a project is at most {MAX_TOTAL}",
                self.total()
            ));
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
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
            return Err(Refusal::new(
                400,
                format!(
                    "a project file is NAME.st (NAME 1 to {STEM_LEN} of A-Z a-z 0-9 _ -), {IO_MAP} or {SETTINGS}"
                ),
            ));
        }
        let text = check_text(text).map_err(|e| Refusal::new(422, format!("{file}: {e}")))?;
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

/// Files from an archive's entries: one common top-level folder stripped,
/// Mac metadata skipped, every name a project file name, no duplicates.
///
/// # Errors
/// A name that is not a project file (traversal, an absolute path, a
/// nested folder, anything else), a duplicate, bad contents.
pub fn from_zip(entries: Vec<zip::Entry>) -> Result<Files, String> {
    let keep: Vec<zip::Entry> = entries
        .into_iter()
        .filter(|e| {
            !(e.name.starts_with("__MACOSX/") || e.name == ".DS_Store" || e.name.ends_with("/.DS_Store"))
        })
        .collect();
    let prefix = keep
        .first()
        .and_then(|e| e.name.split_once('/'))
        .map(|(p, _)| format!("{p}/"))
        .filter(|p| p.len() > 1 && keep.iter().all(|e| e.name.starts_with(p.as_str())));
    let mut files = BTreeMap::new();
    for e in keep {
        let name = prefix
            .as_deref()
            .and_then(|p| e.name.strip_prefix(p))
            .unwrap_or(&e.name)
            .to_string();
        if file_name(&name).is_none() {
            return Err(format!(
                "{}: not a project file (NAME.st, {IO_MAP} or {SETTINGS} at the archive's top level)",
                crate::api::clean(&e.name)
            ));
        }
        let text = check_text(&e.data).map_err(|why| format!("{name}: {why}"))?;
        if files.insert(name.clone(), text.to_string()).is_some() {
            return Err(format!("{name}: twice in the archive"));
        }
    }
    let files = Files(files);
    files.check().map_err(|e| e.join("; "))?;
    Ok(files)
}
