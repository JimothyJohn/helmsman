//! A project's history on the box (Nick 2026-11-01, `project-history`,
//! interview 2026-11-02: revisions, not git): every save, upload, import,
//! deploy and restore writes a numbered revision, the project's files as
//! they were after the change, with who did it, when, what, and the Agent
//! page that produced it when there was one. Listable, readable,
//! restorable (a restore is a new revision); the newest [`MAX_REVISIONS`]
//! are kept, the oldest go.
//!
//! ```text
//! <state-dir>/projects-history/
//!   line3/                   the project's name
//!     000001/                revision 1
//!       revision.json        {n, at_ms, by, what, page}
//!       main.st, io.map, …   the files as they were
//!       page.md              the Agent page, when the change came from one
//!     000002/
//! ```
//!
//! Beside the projects folder, not inside a project: the project's own
//! reads, zips and listings never see it, and a project renamed or deleted
//! takes its history along. Every revision is written whole into a
//! temporary folder and renamed into place, so a reader never sees half
//! of one. Deploy snapshots in the store are untouched: they are what
//! runs; this is what was written.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use helmsman_project::{Files, MAX_FILE, agent, check_text, file_name, project_name};
use serde_json::{Value as Json, json};

use super::{Refusal, is_dir, read_file, tmp_name, write_atomic};

/// Most revisions kept per project.
pub const MAX_REVISIONS: usize = 200;
/// Longest `what` kept, characters.
pub const MAX_WHAT: usize = 200;
/// The revision's record in its folder.
const META: &str = "revision.json";
/// The Agent page in a revision's folder.
const PAGE: &str = "page.md";

type R<T> = Result<T, Refusal>;

/// One revision, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// 1, 2, 3, ... per project.
    pub n: u32,
    /// Unix time, milliseconds.
    pub at_ms: u64,
    /// Who: the session's label (`alice (engineer)`), or `box`.
    pub by: String,
    /// What happened, one line: `saved main.st`, `deployed`, ...
    pub what: String,
    /// The revision carries an Agent page.
    pub page: bool,
    /// The files' names.
    pub files: Vec<String>,
}

impl Revision {
    #[must_use]
    pub fn json(&self) -> Json {
        json!({
            "n": self.n, "at_ms": self.at_ms, "by": self.by, "what": self.what,
            "page": self.page, "files": self.files,
        })
    }
}

/// One revision, read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub revision: Revision,
    pub files: Files,
    pub page: Option<String>,
}

/// The history of every project under one folder.
#[derive(Debug)]
pub struct History {
    root: PathBuf,
}

/// Now, as Unix milliseconds.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// `what` fit for the record: one line, control characters out, cut.
fn clean_what(what: &str) -> String {
    let one: String = what
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one.trim().chars().take(MAX_WHAT).collect()
}

fn folder_name(n: u32) -> String {
    format!("{n:06}")
}

fn parse_n(name: &str) -> Option<u32> {
    (name.len() == 6 && name.bytes().all(|b| b.is_ascii_digit()))
        .then(|| name.parse().ok())
        .flatten()
}

impl History {
    /// The histories under `root` (made if missing).
    ///
    /// # Errors
    /// `root` cannot be made, or is not a directory.
    pub fn open(root: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root)?;
        if !is_dir(&root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the history folder is not a directory",
            ));
        }
        Ok(Self { root })
    }

    fn dir(&self, name: &str) -> R<PathBuf> {
        if !project_name(name) {
            return Err(Refusal::new(404, "no such project"));
        }
        Ok(self.root.join(name))
    }

    /// The revision folders of `name`, by number, ascending.
    fn folders(&self, name: &str) -> R<Vec<(u32, PathBuf)>> {
        let dir = self.dir(name)?;
        if !is_dir(&dir) {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for e in fs::read_dir(&dir).map_err(|e| Refusal::io(&e))? {
            let e = e.map_err(|e| Refusal::io(&e))?;
            let Some(n) = e.file_name().to_str().and_then(parse_n) else {
                continue;
            };
            if is_dir(&e.path()) {
                out.push((n, e.path()));
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    fn meta(path: &Path, n: u32) -> Option<Revision> {
        let bytes = read_file(&path.join(META), MAX_FILE).ok()?;
        let j: Json = serde_json::from_slice(&bytes).ok()?;
        let mut files: Vec<String> = fs::read_dir(path)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|f| file_name(f).is_some())
            .collect();
        files.sort();
        Some(Revision {
            n,
            at_ms: j["at_ms"].as_u64().unwrap_or(0),
            by: j["by"].as_str().unwrap_or("?").to_string(),
            what: j["what"].as_str().unwrap_or("").to_string(),
            page: path.join(PAGE).is_file(),
            files,
        })
    }

    /// Every revision of `name`, oldest first (none for a project without
    /// any, or none at all).
    ///
    /// # Errors
    /// A bad project name (404), or the folder cannot be read.
    pub fn list(&self, name: &str) -> R<Vec<Revision>> {
        Ok(self
            .folders(name)?
            .into_iter()
            .filter_map(|(n, p)| Self::meta(&p, n))
            .collect())
    }

    /// Revision `n` of `name`, whole.
    ///
    /// # Errors
    /// No such project or revision (404); a file in it that breaks the
    /// rules (500).
    pub fn read(&self, name: &str, n: u32) -> R<Read> {
        let dir = self.dir(name)?.join(folder_name(n));
        let Some(revision) = is_dir(&dir).then(|| Self::meta(&dir, n)).flatten() else {
            return Err(Refusal::new(404, "no such revision"));
        };
        let mut files = Files::default();
        for f in &revision.files {
            let bytes = read_file(&dir.join(f), MAX_FILE).map_err(|e| Refusal::io(&e))?;
            let text =
                check_text(&bytes).map_err(|e| Refusal::new(500, format!("{f} in revision {n}: {e}")))?;
            files.0.insert(f.clone(), text.to_string());
        }
        let page = revision
            .page
            .then(|| read_file(&dir.join(PAGE), MAX_FILE).ok())
            .flatten()
            .and_then(|b| String::from_utf8(b).ok());
        Ok(Read {
            revision,
            files,
            page,
        })
    }

    /// Writes the next revision of `name`: `files` as they are now, `by`
    /// and `what` as given, `page` (sanitized) when the change came from
    /// an Agent page. Past [`MAX_REVISIONS`] the oldest go. Returns its
    /// number.
    ///
    /// # Errors
    /// A bad project name (404), or the folder cannot be written.
    pub fn record(&self, name: &str, files: &Files, by: &str, what: &str, page: Option<&str>) -> R<u32> {
        let dir = self.dir(name)?;
        fs::create_dir_all(&dir).map_err(|e| Refusal::io(&e))?;
        let folders = self.folders(name)?;
        let n = folders.last().map_or(1, |(k, _)| k + 1);
        let tmp = dir.join(tmp_name("rev"));
        let made = (|| {
            fs::create_dir(&tmp)?;
            for (f, text) in &files.0 {
                write_atomic(&tmp, f, text.as_bytes())?;
            }
            if let Some(p) = page {
                let p = agent::sanitize(p);
                let p: String = p.chars().take(MAX_FILE / 4).collect();
                write_atomic(&tmp, PAGE, p.as_bytes())?;
            }
            let meta = json!({
                "n": n,
                "at_ms": now_ms(),
                "by": clean_what(by),
                "what": clean_what(what),
                "page": page.is_some(),
            });
            write_atomic(&tmp, META, meta.to_string().as_bytes())
        })();
        if let Err(e) = made {
            let _ = fs::remove_dir_all(&tmp);
            return Err(Refusal::io(&e));
        }
        fs::rename(&tmp, dir.join(folder_name(n))).map_err(|e| {
            let _ = fs::remove_dir_all(&tmp);
            Refusal::io(&e)
        })?;
        // The oldest go once there are more than kept.
        let excess = (folders.len() + 1).saturating_sub(MAX_REVISIONS);
        for (_, old) in folders.iter().take(excess) {
            let _ = fs::remove_dir_all(old);
        }
        Ok(n)
    }

    /// The history follows a renamed project.
    ///
    /// # Errors
    /// The folder cannot be moved.
    pub fn rename(&self, from: &str, to: &str) -> R<()> {
        let src = self.dir(from)?;
        if !is_dir(&src) {
            return Ok(());
        }
        let dest = self.dir(to)?;
        if fs::symlink_metadata(&dest).is_ok() {
            // A history under the new name already (a project deleted and
            // another named like it): the old one is kept behind a stamp.
            let _ = fs::rename(&dest, self.root.join(tmp_name(&format!("old-{to}"))));
        }
        fs::rename(&src, &dest).map_err(|e| Refusal::io(&e))
    }

    /// A deleted project's history goes with it.
    ///
    /// # Errors
    /// The folder cannot be removed.
    pub fn delete(&self, name: &str) -> R<()> {
        let dir = self.dir(name)?;
        if !is_dir(&dir) {
            return Ok(());
        }
        let gone = self.root.join(tmp_name("gone"));
        fs::rename(&dir, &gone).map_err(|e| Refusal::io(&e))?;
        fs::remove_dir_all(&gone).map_err(|e| Refusal::io(&e))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "helmsman-history-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn files(main: &str) -> Files {
        Files(BTreeMap::from([
            ("main.st".to_string(), main.to_string()),
            ("io.map".to_string(), "in x bool box di:0\n".to_string()),
        ]))
    }

    #[test]
    fn revisions_are_numbered_listed_read_renamed_and_deleted() {
        let h = History::open(tmp("basic")).unwrap();
        assert_eq!(h.list("line3").unwrap(), vec![]);
        assert_eq!(
            h.record(
                "line3",
                &files("PROGRAM a END_PROGRAM"),
                "alice (engineer)",
                "saved main.st",
                None
            )
            .unwrap(),
            1
        );
        let page = "Make it blink\n\n<!-- page -->\u{202E}evil";
        assert_eq!(
            h.record(
                "line3",
                &files("PROGRAM b END_PROGRAM"),
                "alice (engineer)",
                "took the Agent's answer\ninto main.st",
                Some(page)
            )
            .unwrap(),
            2
        );
        let list = h.list("line3").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].n, 1);
        assert!(!list[0].page && list[1].page);
        assert_eq!(list[1].what, "took the Agent's answer into main.st", "one line");
        assert_eq!(list[1].files, ["io.map", "main.st"]);
        assert!(list[1].at_ms >= list[0].at_ms);
        let r = h.read("line3", 1).unwrap();
        assert_eq!(r.files.get("main.st"), Some("PROGRAM a END_PROGRAM"));
        assert_eq!(r.page, None);
        let r = h.read("line3", 2).unwrap();
        assert_eq!(
            r.page.as_deref(),
            Some("Make it blink\n\n<!-- page -->evil"),
            "sanitized"
        );
        assert_eq!(h.read("line3", 3).unwrap_err().status, 404);
        assert_eq!(h.read("nope", 1).unwrap_err().status, 404);
        assert_eq!(h.read("../x", 1).unwrap_err().status, 404);
        h.rename("line3", "line4").unwrap();
        assert_eq!(h.list("line3").unwrap(), vec![]);
        assert_eq!(h.list("line4").unwrap().len(), 2);
        assert_eq!(
            h.record("line4", &files("c"), "box", "deployed", None).unwrap(),
            3,
            "numbers go on"
        );
        h.delete("line4").unwrap();
        assert_eq!(h.list("line4").unwrap(), vec![]);
        h.delete("line4").unwrap();
        h.rename("line4", "line5").unwrap();
    }

    #[test]
    fn the_oldest_go_past_the_cap_and_junk_folders_are_skipped() {
        let h = History::open(tmp("cap")).unwrap();
        for k in 0..MAX_REVISIONS + 3 {
            h.record(
                "p",
                &files(&format!("PROGRAM p{k} END_PROGRAM")),
                "box",
                "saved",
                None,
            )
            .unwrap();
        }
        let list = h.list("p").unwrap();
        assert_eq!(list.len(), MAX_REVISIONS);
        assert_eq!(list[0].n, 4);
        assert_eq!(list.last().unwrap().n as usize, MAX_REVISIONS + 3);
        fs::create_dir_all(h.root.join("p").join("junk")).unwrap();
        fs::write(h.root.join("p").join("000002"), b"a file, not a folder").unwrap();
        fs::create_dir_all(h.root.join("p").join("000003")).unwrap();
        assert_eq!(h.list("p").unwrap().len(), MAX_REVISIONS, "no meta, no revision");
        assert_eq!(h.read("p", 3).unwrap_err().status, 404);
    }
}
