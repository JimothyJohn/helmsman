//! Snapshots: every applied change set as an immutable, content-addressed
//! record, in one SQLite file ([`DB_FILE`]) under the store directory.
//!
//! A [`ChangeSet`] is everything the box runs: each module's
//! `module.manifest` and `module.st` (params live in the manifest), the I/O
//! map (the signal bindings) and the shift calendar, as the exact bytes of
//! their files. Its [`SnapshotId`] is the SHA-256 of a canonical *tree*:
//!
//! ```text
//! helmsman-snapshot 1
//! module conveyor <sha256 of module.manifest> <sha256 of module.st>
//! module lamp     ...                          (sorted by name, one space)
//! io-map <sha256>                              (if present)
//! calendar <sha256>                            (if present)
//! ```
//!
//! so identical content is the identical id, in any store and with any
//! metadata, and a tree plus its blobs is all a bundle needs to carry.
//! Every read re-hashes the tree and each blob, so a tampered or corrupted
//! row is an [`Error::Corrupt`], never wrong content.
//!
//! History is an append-only **log**: one [`Entry`] per apply or rollback,
//! with who, why, who approved and when ([`Meta`]). Each entry's hash covers
//! its fields and the previous entry's hash, so editing history breaks the
//! chain ([`Store::list`] checks all of it, [`Store::head`] the head entry).
//! A rollback is a new entry pointing at old content; nothing is rewritten.
//! Committing what is already head is a no-op that returns head's id.
//!
//! Tables are guarded by triggers that refuse `UPDATE` and `DELETE`, the
//! file carries an application id and a schema version (`PRAGMA
//! user_version`, [`SCHEMA`]), and a store from a newer version is refused
//! rather than misread. Writers serialize on SQLite's write lock
//! (`BEGIN IMMEDIATE`), so concurrent commits from threads or processes
//! never lose an entry. A [`Store`] is one connection: open one per thread.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use helmsman_io::{Device, Signal, Value};

use crate::manifest::{self, MAX_FILE, Manifest, Param, hidden, read_capped};
use crate::spec::name_ok;

/// The store's file inside its directory.
pub const DB_FILE: &str = "snapshots.db";
/// The schema version this build reads and writes.
pub const SCHEMA: i64 = 1;
/// `PRAGMA application_id`: "HLMS".
const APP_ID: i64 = 0x484C_4D53;
/// Most modules in one change set.
pub const MAX_MODULES: usize = 1024;
/// Longest `author` or `approver`, in bytes.
pub const MAX_NAME: usize = 1024;
/// Longest `prompt`, in bytes.
pub const MAX_PROMPT: usize = 64 * 1024;

const TREE_HEADER: &str = "helmsman-snapshot 1";
const ENTRY_HEADER: &str = "helmsman-entry 1";

const SCHEMA_SQL: &str = "
CREATE TABLE blobs (hash BLOB PRIMARY KEY NOT NULL, data BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE snapshots (id BLOB PRIMARY KEY NOT NULL, tree TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE log (
    seq INTEGER PRIMARY KEY,
    snapshot BLOB NOT NULL REFERENCES snapshots(id),
    rollback_to BLOB REFERENCES snapshots(id),
    time_ms INTEGER NOT NULL,
    author TEXT NOT NULL,
    prompt TEXT NOT NULL,
    approver TEXT,
    hash BLOB NOT NULL
);
CREATE TRIGGER blobs_no_update BEFORE UPDATE ON blobs
    BEGIN SELECT RAISE(ABORT, 'snapshot store is immutable'); END;
CREATE TRIGGER blobs_no_delete BEFORE DELETE ON blobs
    BEGIN SELECT RAISE(ABORT, 'snapshot store is immutable'); END;
CREATE TRIGGER snapshots_no_update BEFORE UPDATE ON snapshots
    BEGIN SELECT RAISE(ABORT, 'snapshot store is immutable'); END;
CREATE TRIGGER snapshots_no_delete BEFORE DELETE ON snapshots
    BEGIN SELECT RAISE(ABORT, 'snapshot store is immutable'); END;
CREATE TRIGGER log_no_update BEFORE UPDATE ON log
    BEGIN SELECT RAISE(ABORT, 'snapshot store is immutable'); END;
CREATE TRIGGER log_no_delete BEFORE DELETE ON log
    BEGIN SELECT RAISE(ABORT, 'snapshot store is immutable'); END;
";

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(&Sha256::digest(data));
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// A snapshot's content address: the SHA-256 of its tree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnapshotId([u8; 32]);

impl SnapshotId {
    /// From 64 lowercase hex digits, as [`Display`](fmt::Display) writes it.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        unhex32(s).map(Self)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn from_slice(b: &[u8]) -> Option<Self> {
        b.try_into().ok().map(Self)
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SnapshotId({self})")
    }
}

/// One module's two files, byte for byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleFiles {
    /// `module.manifest`.
    pub manifest: String,
    /// `module.st`.
    pub source: String,
}

/// Everything one snapshot holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// By module name, which must be the manifest's `name`.
    pub modules: BTreeMap<String, ModuleFiles>,
    /// The I/O map file (`helmsman_io::text`): devices and signal bindings.
    pub io_map: Option<String>,
    /// The shift calendar file, stored as given.
    pub calendar: Option<String>,
}

impl ChangeSet {
    /// Reads module directories and optional map and calendar files from
    /// disk, each capped at [`MAX_FILE`].
    ///
    /// # Errors
    /// A file missing, too large or not UTF-8, a manifest that does not
    /// parse, or two directories with one module name.
    pub fn read(
        module_dirs: &[PathBuf],
        io_map: Option<&Path>,
        calendar: Option<&Path>,
    ) -> Result<Self, String> {
        let mut modules = BTreeMap::new();
        for dir in module_dirs {
            let path = dir.join(manifest::FILE);
            let text = read_capped(&path)?;
            let name = Manifest::parse(&text)
                .map_err(|_| format!("{} does not parse", path.display()))?
                .name;
            let source = read_capped(&dir.join(manifest::SOURCE))?;
            let files = ModuleFiles {
                manifest: text,
                source,
            };
            if modules.insert(name.clone(), files).is_some() {
                return Err(format!("two module directories are named {name}"));
            }
        }
        Ok(Self {
            modules,
            io_map: io_map.map(read_capped).transpose()?,
            calendar: calendar.map(read_capped).transpose()?,
        })
    }

    /// The canonical tree this change set hashes to (module docs).
    #[must_use]
    pub fn tree(&self) -> String {
        let h = |s: &str| hex(&sha256(s.as_bytes()));
        let mut t = format!("{TREE_HEADER}\n");
        for (name, f) in &self.modules {
            let _ = writeln!(t, "module {name} {} {}", h(&f.manifest), h(&f.source));
        }
        if let Some(m) = &self.io_map {
            let _ = writeln!(t, "io-map {}", h(m));
        }
        if let Some(c) = &self.calendar {
            let _ = writeln!(t, "calendar {}", h(c));
        }
        t
    }

    /// Its content address, with no store.
    #[must_use]
    pub fn id(&self) -> SnapshotId {
        SnapshotId(sha256(self.tree().as_bytes()))
    }

    /// Whether the store will take it: module names the runtime accepts,
    /// each manifest parses and names its module, the I/O map parses, no
    /// file over [`MAX_FILE`]. Programs are not compiled here; whoever
    /// applies a change set compiles it before committing.
    ///
    /// # Errors
    /// Every problem, at most one per module.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let big = |s: &str| s.len() as u64 > MAX_FILE;
        if self.modules.len() > MAX_MODULES {
            errs.push(format!("{} modules, more than {MAX_MODULES}", self.modules.len()));
        }
        for (name, f) in &self.modules {
            if !name_ok(name) {
                errs.push(format!(
                    "module {name:?}: not a module name (1..=64 of A-Z a-z 0-9 _ -)"
                ));
            } else if big(&f.manifest) || big(&f.source) {
                errs.push(format!("module {name}: a file is larger than {MAX_FILE} bytes"));
            } else {
                match Manifest::parse(&f.manifest) {
                    Err(e) => {
                        let lines: Vec<String> = e.iter().map(|(n, w)| format!("{n}: {w}")).collect();
                        errs.push(format!(
                            "module {name}: manifest does not parse: {}",
                            lines.join("; ")
                        ));
                    }
                    Ok(m) if m.name != *name => {
                        errs.push(format!("module {name}: its manifest is named {}", m.name));
                    }
                    Ok(_) => {}
                }
            }
        }
        if let Some(m) = &self.io_map {
            if big(m) {
                errs.push(format!("io map: larger than {MAX_FILE} bytes"));
            } else if let Err(e) = helmsman_io::text::parse(m) {
                let lines: Vec<String> = e.iter().map(|(n, w)| format!("{n}: {w}")).collect();
                errs.push(format!("io map does not parse: {}", lines.join("; ")));
            }
        }
        if self.calendar.as_deref().is_some_and(big) {
            errs.push(format!("calendar: larger than {MAX_FILE} bytes"));
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}

/// Who changed the box, why, who approved it and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    /// One line, not blank.
    pub author: String,
    /// The prompt behind the change; may span lines, may be empty.
    pub prompt: String,
    /// One line, not blank; `None` until approval policy requires one.
    pub approver: Option<String>,
    /// Milliseconds since the Unix epoch, from the caller's clock.
    pub time_ms: i64,
}

impl Meta {
    /// Free text as the manifest takes it: no control or invisible
    /// formatting characters (a prompt may also hold newlines and tabs).
    fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let mut name = |key: &str, v: &str| {
            if v.trim().is_empty() {
                errs.push(format!("{key} is blank"));
            } else if v.len() > MAX_NAME {
                errs.push(format!("{key} is longer than {MAX_NAME} bytes"));
            } else if v.chars().any(hidden) {
                errs.push(format!("{key} holds a control or invisible character"));
            }
        };
        name("author", &self.author);
        if let Some(a) = &self.approver {
            name("approver", a);
        }
        if self.prompt.len() > MAX_PROMPT {
            errs.push(format!("prompt is longer than {MAX_PROMPT} bytes"));
        } else if self.prompt.chars().any(|c| c != '\n' && c != '\t' && hidden(c)) {
            errs.push("prompt holds a control or invisible character".into());
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}

/// How an entry came to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A change set was applied.
    Apply,
    /// The box was rolled back to an earlier snapshot's content.
    Rollback { to: SnapshotId },
}

/// One line of history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 1, 2, ... in commit order.
    pub seq: u64,
    /// The snapshot the box runs from this entry on.
    pub id: SnapshotId,
    pub kind: Kind,
    pub meta: Meta,
}

impl Entry {
    fn hash(&self, prev: Option<&[u8; 32]>) -> [u8; 32] {
        let mut s = format!("{ENTRY_HEADER}\nseq {}\nprev ", self.seq);
        s.push_str(&prev.map_or_else(|| "-".into(), |p| hex(p)));
        let _ = write!(s, "\nsnapshot {}\nkind ", self.id);
        match self.kind {
            Kind::Apply => s.push_str("apply"),
            Kind::Rollback { to } => {
                let _ = write!(s, "rollback {to}");
            }
        }
        let _ = writeln!(s, "\ntime {}", self.meta.time_ms);
        let mut field = |key: &str, v: Option<&str>| match v {
            None => {
                let _ = writeln!(s, "{key} -");
            }
            Some(v) => {
                let _ = writeln!(s, "{key} {}\n{v}", v.len());
            }
        };
        field("author", Some(&self.meta.author));
        field("prompt", Some(&self.meta.prompt));
        field("approver", self.meta.approver.as_deref());
        sha256(s.as_bytes())
    }
}

/// What can go wrong with a store.
#[derive(Debug)]
pub enum Error {
    /// The directory or file could not be used.
    Io(String),
    /// `SQLite` refused.
    Sqlite(rusqlite::Error),
    /// The file is a database, but not a snapshot store.
    NotAStore(PathBuf),
    /// Written by a newer build; this one will not guess at it.
    NewerSchema { found: i64, supported: i64 },
    /// The change set or meta is not one the store takes; every reason.
    Invalid(Vec<String>),
    /// No snapshot with this id.
    NotFound(SnapshotId),
    /// Stored content does not match its hash, or history is broken.
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "snapshot store: {e}"),
            Self::Sqlite(e) => write!(f, "snapshot store: {e}"),
            Self::NotAStore(p) => write!(f, "{} is not a Helmsman snapshot store", p.display()),
            Self::NewerSchema { found, supported } => write!(
                f,
                "snapshot store has schema version {found}, newer than this build reads ({supported}); \
                 run a newer helmsman-plc"
            ),
            Self::Invalid(why) => write!(f, "refused: {}", why.join("; ")),
            Self::NotFound(id) => write!(f, "no snapshot {id}"),
            Self::Corrupt(why) => write!(f, "snapshot store is corrupt: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// A snapshot store: one connection to [`DB_FILE`] in a directory.
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

/// A raw log row.
struct Row {
    seq: i64,
    snapshot: Vec<u8>,
    rollback_to: Option<Vec<u8>>,
    time_ms: i64,
    author: String,
    prompt: String,
    approver: Option<String>,
    hash: Vec<u8>,
}

const ROW_COLS: &str = "seq, snapshot, rollback_to, time_ms, author, prompt, approver, hash";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        seq: r.get(0)?,
        snapshot: r.get(1)?,
        rollback_to: r.get(2)?,
        time_ms: r.get(3)?,
        author: r.get(4)?,
        prompt: r.get(5)?,
        approver: r.get(6)?,
        hash: r.get(7)?,
    })
}

/// The entry a row holds, once its hash checks against `prev`.
fn entry(r: Row, prev: Option<&[u8; 32]>) -> Result<(Entry, [u8; 32]), Error> {
    let bad = |what: &str| Error::Corrupt(format!("log entry {}: {what}", r.seq));
    let seq = u64::try_from(r.seq).map_err(|_| bad("bad seq"))?;
    let id = SnapshotId::from_slice(&r.snapshot).ok_or_else(|| bad("bad snapshot id"))?;
    let kind = match &r.rollback_to {
        None => Kind::Apply,
        Some(b) => Kind::Rollback {
            to: SnapshotId::from_slice(b).ok_or_else(|| bad("bad rollback id"))?,
        },
    };
    let stored: [u8; 32] = r.hash.as_slice().try_into().map_err(|_| bad("bad hash"))?;
    let e = Entry {
        seq,
        id,
        kind,
        meta: Meta {
            author: r.author,
            prompt: r.prompt,
            approver: r.approver,
            time_ms: r.time_ms,
        },
    };
    if e.hash(prev) != stored {
        return Err(bad("does not match its hash (history was edited)"));
    }
    Ok((e, stored))
}

/// The head entry and its hash, checked.
fn head_of(conn: &Connection) -> Result<Option<(Entry, [u8; 32])>, Error> {
    let last = conn
        .query_row(
            &format!("SELECT {ROW_COLS} FROM log ORDER BY seq DESC LIMIT 1"),
            [],
            row,
        )
        .optional()?;
    let Some(last) = last else { return Ok(None) };
    let prev: Option<[u8; 32]> = if last.seq == 1 {
        None
    } else {
        let h: Option<Vec<u8>> = conn
            .query_row("SELECT hash FROM log WHERE seq = ?1", [last.seq - 1], |r| {
                r.get(0)
            })
            .optional()?;
        let h = h.ok_or_else(|| Error::Corrupt(format!("log entry {} is missing", last.seq - 1)))?;
        Some(
            h.as_slice()
                .try_into()
                .map_err(|_| Error::Corrupt(format!("log entry {}: bad hash", last.seq - 1)))?,
        )
    };
    entry(last, prev.as_ref()).map(Some)
}

/// Reads a blob and checks it against its hash.
fn blob(conn: &Connection, hash: &str) -> Result<String, Error> {
    let key = unhex32(hash).ok_or_else(|| Error::Corrupt(format!("tree names a bad blob hash {hash:?}")))?;
    let data: Option<Vec<u8>> = conn
        .query_row("SELECT data FROM blobs WHERE hash = ?1", [key.as_slice()], |r| {
            r.get(0)
        })
        .optional()?;
    let data = data.ok_or_else(|| Error::Corrupt(format!("blob {hash} is missing")))?;
    if sha256(&data) != key {
        return Err(Error::Corrupt(format!("blob {hash} does not match its hash")));
    }
    String::from_utf8(data).map_err(|_| Error::Corrupt(format!("blob {hash} is not UTF-8")))
}

/// Reads a snapshot, checking the tree and every blob.
fn load(conn: &Connection, id: &SnapshotId) -> Result<ChangeSet, Error> {
    let tree: Option<String> = conn
        .query_row(
            "SELECT tree FROM snapshots WHERE id = ?1",
            [id.0.as_slice()],
            |r| r.get(0),
        )
        .optional()?;
    let tree = tree.ok_or(Error::NotFound(*id))?;
    if sha256(tree.as_bytes()) != id.0 {
        return Err(Error::Corrupt(format!(
            "snapshot {id}: tree does not match its id"
        )));
    }
    let bad = || Error::Corrupt(format!("snapshot {id}: tree does not parse"));
    let mut lines = tree.lines();
    if lines.next() != Some(TREE_HEADER) {
        return Err(bad());
    }
    let mut cs = ChangeSet::default();
    for line in lines {
        let w: Vec<&str> = line.split(' ').collect();
        match w.as_slice() {
            ["module", name, m, s] => {
                let files = ModuleFiles {
                    manifest: blob(conn, m)?,
                    source: blob(conn, s)?,
                };
                cs.modules.insert((*name).to_string(), files);
            }
            ["io-map", h] => cs.io_map = Some(blob(conn, h)?),
            ["calendar", h] => cs.calendar = Some(blob(conn, h)?),
            _ => return Err(bad()),
        }
    }
    if cs.tree() != tree {
        return Err(bad());
    }
    Ok(cs)
}

impl Store {
    /// Opens the store in `dir`, creating the directory and an empty store
    /// if there is none.
    ///
    /// # Errors
    /// The directory or file cannot be used, the file is not a store, or it
    /// was written by a newer schema ([`Error::NewerSchema`]).
    pub fn open(dir: &Path) -> Result<Self, Error> {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(format!("{}: {e}", dir.display())))?;
        let path = dir.join(DB_FILE);
        let mut conn = Connection::open(&path)?;
        conn.busy_timeout(Duration::from_secs(30))?;
        let app: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let ver: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if app == 0 && ver == 0 {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
            if app == 0 {
                let tables: i64 = tx.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
                if tables != 0 {
                    return Err(Error::NotAStore(path));
                }
                tx.execute_batch(SCHEMA_SQL)?;
                tx.pragma_update(None, "application_id", APP_ID)?;
                tx.pragma_update(None, "user_version", SCHEMA)?;
            }
            tx.commit()?;
        }
        let app: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let ver: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if app != APP_ID {
            return Err(Error::NotAStore(path));
        }
        if ver > SCHEMA {
            return Err(Error::NewerSchema {
                found: ver,
                supported: SCHEMA,
            });
        }
        if ver < SCHEMA {
            return Err(Error::NotAStore(path));
        }
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { conn, path })
    }

    /// The database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records `cs` as the new head and returns its id. If head already
    /// holds exactly this content, nothing is written and head's id is
    /// returned.
    ///
    /// # Errors
    /// [`Error::Invalid`] with every reason the change set or meta is
    /// refused; a storage error.
    pub fn commit(&mut self, cs: &ChangeSet, meta: &Meta) -> Result<SnapshotId, Error> {
        let mut errs = cs.validate().err().unwrap_or_default();
        errs.extend(meta.validate().err().unwrap_or_default());
        if !errs.is_empty() {
            return Err(Error::Invalid(errs));
        }
        let id = cs.id();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head = head_of(&tx)?;
        if head.as_ref().is_some_and(|(e, _)| e.id == id) {
            return Ok(id);
        }
        {
            let mut put = tx.prepare("INSERT OR IGNORE INTO blobs (hash, data) VALUES (?1, ?2)")?;
            let files = cs
                .modules
                .values()
                .flat_map(|f| [&f.manifest, &f.source])
                .chain(&cs.io_map)
                .chain(&cs.calendar);
            for data in files {
                put.execute(params![sha256(data.as_bytes()).as_slice(), data.as_bytes()])?;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO snapshots (id, tree) VALUES (?1, ?2)",
            params![id.0.as_slice(), cs.tree()],
        )?;
        append(&tx, head, id, Kind::Apply, meta)?;
        tx.commit()?;
        Ok(id)
    }

    /// Makes `to`'s content head again, as a new entry; history is kept.
    /// Rolling back to what head already is writes nothing.
    ///
    /// # Errors
    /// [`Error::NotFound`] for an unknown id, [`Error::Corrupt`] if its
    /// content no longer checks, [`Error::Invalid`] for bad meta.
    pub fn rollback(&mut self, to: &SnapshotId, meta: &Meta) -> Result<SnapshotId, Error> {
        meta.validate().map_err(Error::Invalid)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        load(&tx, to)?;
        let head = head_of(&tx)?;
        if head.as_ref().is_some_and(|(e, _)| e.id == *to) {
            return Ok(*to);
        }
        append(&tx, head, *to, Kind::Rollback { to: *to }, meta)?;
        tx.commit()?;
        Ok(*to)
    }

    /// A snapshot's content, every byte checked against its hash.
    ///
    /// # Errors
    /// [`Error::NotFound`], [`Error::Corrupt`], or a storage error.
    pub fn get(&self, id: &SnapshotId) -> Result<ChangeSet, Error> {
        load(&self.conn, id)
    }

    /// The latest entry, checked against its predecessor's hash; `None`
    /// for a new store.
    ///
    /// # Errors
    /// [`Error::Corrupt`] or a storage error.
    pub fn head(&self) -> Result<Option<Entry>, Error> {
        Ok(head_of(&self.conn)?.map(|(e, _)| e))
    }

    /// Every entry, newest first, with the whole hash chain checked.
    ///
    /// # Errors
    /// [`Error::Corrupt`] if any entry was edited or removed; a storage
    /// error.
    pub fn list(&self) -> Result<Vec<Entry>, Error> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {ROW_COLS} FROM log ORDER BY seq"))?;
        let rows = stmt.query_map([], row)?;
        let mut out = Vec::new();
        let mut prev: Option<[u8; 32]> = None;
        for (n, r) in (1..).zip(rows) {
            let r = r?;
            if r.seq != n {
                return Err(Error::Corrupt(format!("log entry {n} is missing")));
            }
            let (e, h) = entry(r, prev.as_ref())?;
            prev = Some(h);
            out.push(e);
        }
        out.reverse();
        Ok(out)
    }

    /// What changed from snapshot `a` to snapshot `b`.
    ///
    /// # Errors
    /// As [`Store::get`] for either.
    pub fn diff(&self, a: &SnapshotId, b: &SnapshotId) -> Result<Diff, Error> {
        Ok(diff(&self.get(a)?, &self.get(b)?))
    }
}

fn append(
    tx: &Connection,
    head: Option<(Entry, [u8; 32])>,
    id: SnapshotId,
    kind: Kind,
    meta: &Meta,
) -> Result<(), Error> {
    let (seq, prev) = head.map_or((1, None), |(e, h)| (e.seq + 1, Some(h)));
    let e = Entry {
        seq,
        id,
        kind,
        meta: meta.clone(),
    };
    let hash = e.hash(prev.as_ref());
    let rollback_to = match kind {
        Kind::Apply => None,
        Kind::Rollback { to } => Some(to.0.to_vec()),
    };
    tx.execute(
        &format!("INSERT INTO log ({ROW_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
        params![
            i64::try_from(seq).map_err(|_| Error::Corrupt("log is full".into()))?,
            id.0.as_slice(),
            rollback_to,
            meta.time_ms,
            meta.author,
            meta.prompt,
            meta.approver,
            hash.as_slice(),
        ],
    )?;
    Ok(())
}

// ---- diff ---------------------------------------------------------------------

/// One named thing on either side: `from` is `None` when it was added,
/// `to` when it was removed.
#[derive(Clone, Debug, PartialEq)]
pub struct Field<T> {
    pub name: String,
    pub from: Option<T>,
    pub to: Option<T>,
}

/// How one module differs.
#[derive(Clone, Debug, PartialEq)]
pub enum ModuleChange {
    Added(String),
    Removed(String),
    Changed {
        name: String,
        /// `module.st` changed.
        source: bool,
        /// `module.manifest` changed (params, schedule, windows, ...).
        manifest: bool,
        /// The params that changed, by name.
        params: Vec<Field<Param>>,
    },
}

/// What changed between two snapshots, each list sorted by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diff {
    pub modules: Vec<ModuleChange>,
    /// The I/O map file changed at all.
    pub io_map: bool,
    /// Devices added, removed or changed, by id.
    pub devices: Vec<Field<Device>>,
    /// Signals added, removed or rebound, with their safe value.
    pub bindings: Vec<Field<(Signal, Option<Value>)>>,
    /// The calendar file changed.
    pub calendar: bool,
}

impl Diff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
            && !self.io_map
            && self.devices.is_empty()
            && self.bindings.is_empty()
            && !self.calendar
    }
}

fn fields<T: PartialEq + Clone>(a: &BTreeMap<String, T>, b: &BTreeMap<String, T>) -> Vec<Field<T>> {
    let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    names
        .into_iter()
        .filter_map(|n| {
            let (from, to) = (a.get(n), b.get(n));
            (from != to).then(|| Field {
                name: n.clone(),
                from: from.cloned(),
                to: to.cloned(),
            })
        })
        .collect()
}

fn params(manifest: &str) -> BTreeMap<String, Param> {
    Manifest::parse(manifest)
        .map(|m| m.params.into_iter().map(|p| (p.name.clone(), p)).collect())
        .unwrap_or_default()
}

type Map = (
    BTreeMap<String, Device>,
    BTreeMap<String, (Signal, Option<Value>)>,
);

/// A map file as devices and signals by name; `None` if it does not parse
/// (a snapshot from an older map format), so the diff says only that the
/// file changed.
fn map(src: Option<&str>) -> Option<Map> {
    let Some(src) = src else {
        return Some(Map::default());
    };
    let f = helmsman_io::text::parse(src).ok()?;
    let devices = f.map.devices.into_iter().map(|d| (d.id.clone(), d)).collect();
    let signals = f
        .map
        .signals
        .into_iter()
        .map(|s| {
            let safe = f.safe.get(&s.name).copied();
            (s.name.clone(), (s, safe))
        })
        .collect();
    Some((devices, signals))
}

/// What changed from `a` to `b`: modules added, removed, or changed (with
/// the params that changed), devices and signal bindings, the calendar.
#[must_use]
pub fn diff(a: &ChangeSet, b: &ChangeSet) -> Diff {
    let names: BTreeSet<&String> = a.modules.keys().chain(b.modules.keys()).collect();
    let modules = names
        .into_iter()
        .filter_map(|n| match (a.modules.get(n), b.modules.get(n)) {
            (Some(_), None) => Some(ModuleChange::Removed(n.clone())),
            (None, Some(_)) => Some(ModuleChange::Added(n.clone())),
            (Some(x), Some(y)) if x != y => Some(ModuleChange::Changed {
                name: n.clone(),
                source: x.source != y.source,
                manifest: x.manifest != y.manifest,
                params: fields(&params(&x.manifest), &params(&y.manifest)),
            }),
            _ => None,
        })
        .collect();
    let io_map = a.io_map != b.io_map;
    let (devices, bindings) = match (map(a.io_map.as_deref()), map(b.io_map.as_deref())) {
        (Some((da, sa)), Some((db, sb))) if io_map => (fields(&da, &db), fields(&sa, &sb)),
        _ => (Vec::new(), Vec::new()),
    };
    Diff {
        modules,
        io_map,
        devices,
        bindings,
        calendar: a.calendar != b.calendar,
    }
}
