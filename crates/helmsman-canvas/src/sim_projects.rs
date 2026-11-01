//! The simulation's projects (docs/UI.md "Projects": "in the browser
//! simulation, projects persist in the browser's storage, with the same
//! Upload and Download"). Held to the box's rules and zipped the box's way
//! ([`helmsman_project`]), so a project downloaded here uploads to a box
//! and back.
//!
//! - **Storage** is whatever the page has ([`Storage`]): `localStorage` in
//!   a browser (every access wrapped: private windows, blocked site data
//!   and full quotas throw), memory natively. When it fails the page
//!   still works, says so once, and keeps the projects in memory.
//! - **The built-in examples** are read-only projects: they open, run and
//!   edit like any other, and Save makes a copy of one under a free name.
//! - **The build** is the box's: one `.st` file is compiled as it is;
//!   several are concatenated in name order, each under a `(* file NAME
//!   *)` line, so a line of the build is a line of one file
//!   ([`crate::projects::file_line`]).

use std::collections::BTreeMap;

use helmsman_project::starters::{Kind, STARTERS, starter};
use helmsman_project::{FileKind, Files, file_name, project_name};
use serde_json::{Value as Json, json};

use crate::playground::EXAMPLES;
use crate::projects::{Group, ProjectRow, file_line, slug};

/// Whether `f` is a program file (`NAME.st`).
#[must_use]
pub fn is_st(f: &str) -> bool {
    file_name(f) == Some(FileKind::St)
}

/// Where the projects are kept between visits.
pub trait Storage {
    /// What was stored (`None`: nothing yet).
    ///
    /// # Errors
    /// The storage cannot be read (blocked, private, broken).
    fn load(&self) -> Result<Option<String>, String>;
    /// Stores `text` in place of what was there.
    ///
    /// # Errors
    /// The storage cannot be written (blocked, full).
    fn save(&mut self, text: &str) -> Result<(), String>;
}

/// Storage in memory: gone with the page (native builds, tests).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Debug, Default)]
pub struct MemStorage(pub Option<String>);

impl Storage for MemStorage {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.clone())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        self.0 = Some(text.to_string());
        Ok(())
    }
}

/// The format the projects are stored in.
const FORMAT: &str = "helmsman-projects 1";

/// A starter project made from a built-in example: named after its
/// PROGRAM, one file.
#[must_use]
pub fn example_project(src: &str) -> (String, String) {
    let program = src
        .lines()
        .find_map(|l| l.trim().strip_prefix("PROGRAM "))
        .map_or_else(
            || "main".into(),
            |n| n.split_whitespace().next().unwrap_or("main").to_ascii_lowercase(),
        );
    let name = slug(&program);
    let file = if file_name(&format!("{program}.st")).is_some() {
        format!("{program}.st")
    } else {
        "main.st".into()
    };
    (name, file)
}

fn examples() -> BTreeMap<String, Files> {
    EXAMPLES
        .iter()
        .map(|e| {
            let (name, file) = example_project(e.src);
            (name, Files(BTreeMap::from([(file, e.src.to_string())])))
        })
        .chain(STARTERS.iter().map(|s| (s.name.to_string(), s.project())))
        .collect()
}

/// The list a built-in project is under.
fn group_of(name: &str) -> Group {
    match starter(name).map(|s| s.kind) {
        Some(Kind::Machine) => Group::Machines,
        Some(Kind::Template) => Group::Templates,
        None => Group::FirstSteps,
    }
}

/// What a new project starts with.
pub const STARTER: &str = "PROGRAM main
VAR_INPUT
  run : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
lamp := run;
END_PROGRAM
";

/// Everything the simulation knows about its projects.
pub struct SimProjects {
    store: Box<dyn Storage>,
    /// The visitor's projects, as last saved.
    pub saved: BTreeMap<String, Files>,
    pub examples: BTreeMap<String, Files>,
    /// The open project's name.
    pub open: String,
    /// Its files as edited here.
    pub files: BTreeMap<String, String>,
    /// The file in the ST editor.
    pub current: String,
    /// The project deployed in this page.
    pub running: Option<String>,
    /// Why the browser's storage does not keep projects, if it does not.
    pub storage_problem: Option<String>,
    /// A one-line note (`true`: an error).
    pub notice: Option<(String, bool)>,
    /// An upload whose name is taken, waiting for Replace.
    pub pending: Option<(String, Files)>,
}

impl std::fmt::Debug for SimProjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimProjects")
            .field("open", &self.open)
            .field("current", &self.current)
            .field("saved", &self.saved.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl SimProjects {
    /// The projects `store` holds, and the examples; opens the one open
    /// last, else the first example. A storage that fails is noted, not
    /// fatal.
    pub fn load(store: Box<dyn Storage>) -> Self {
        let examples = examples();
        let first = example_project(EXAMPLES[0].src).0;
        let mut s = Self {
            store,
            saved: BTreeMap::new(),
            examples,
            open: String::new(),
            files: BTreeMap::new(),
            current: String::new(),
            running: None,
            storage_problem: None,
            notice: None,
            pending: None,
        };
        let mut open = None;
        match s.store.load() {
            Ok(Some(text)) => match parse(&text) {
                Some((saved, last)) => {
                    s.saved = saved;
                    open = last;
                }
                None => {
                    s.storage_problem =
                        Some("what this browser stored is not readable; starting afresh".into());
                }
            },
            Ok(None) => {}
            Err(e) => {
                s.storage_problem = Some(format!(
                    "this browser does not keep projects ({e}): they last until the page closes"
                ));
            }
        }
        let open = open.filter(|o| s.exists(o)).unwrap_or(first);
        s.open_project(&open);
        s
    }

    fn exists(&self, name: &str) -> bool {
        self.saved.contains_key(name) || self.examples.contains_key(name)
    }

    /// The open project is a built-in example.
    #[must_use]
    pub fn is_example(&self) -> bool {
        !self.saved.contains_key(&self.open) && self.examples.contains_key(&self.open)
    }

    /// The projects for the sidebar: the visitor's, then the examples.
    #[must_use]
    pub fn rows(&self) -> Vec<ProjectRow> {
        let mine = self.saved.keys().map(|n| ProjectRow {
            running: self.running.as_deref() == Some(n.as_str()),
            name: n.clone(),
            group: Group::Mine,
        });
        let ex = self
            .examples
            .keys()
            .filter(|n| !self.saved.contains_key(*n))
            .map(|n| ProjectRow {
                running: self.running.as_deref() == Some(n.as_str()),
                name: n.clone(),
                group: group_of(n),
            });
        let mut rows: Vec<ProjectRow> = mine.chain(ex).collect();
        rows.sort_by_key(|r| r.group);
        rows
    }

    /// What the open project holds as stored (the example's own text for
    /// an example).
    fn base(&self) -> Option<&Files> {
        self.saved
            .get(&self.open)
            .or_else(|| self.examples.get(&self.open))
    }

    /// Whether file `f` has edits not saved.
    #[must_use]
    pub fn file_dirty(&self, f: &str) -> bool {
        self.base().and_then(|b| b.get(f)) != self.files.get(f).map(String::as_str)
    }

    /// The open project has edits not saved.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.base().is_none_or(|b| b.0 != self.files)
    }

    /// Opens `name` (its stored files; unsaved edits elsewhere are
    /// dropped), showing its `main.st` if it has one, else its first
    /// program file.
    pub fn open_project(&mut self, name: &str) {
        let Some(files) = self.saved.get(name).or_else(|| self.examples.get(name)).cloned() else {
            return;
        };
        name.clone_into(&mut self.open);
        self.files = files.0;
        self.current = self
            .files
            .keys()
            .find(|f| *f == "main.st")
            .or_else(|| self.files.keys().find(|f| file_name(f) == Some(FileKind::St)))
            .or_else(|| self.files.keys().next())
            .cloned()
            .unwrap_or_default();
        self.pending = None;
        if self.is_example() {
            if let Some(e) = EXAMPLES.iter().find(|e| example_project(e.src).0 == name) {
                self.note(
                    format!("{}: a built-in example. Save keeps your own copy.", e.name),
                    false,
                );
            } else if let Some(s) = starter(name) {
                self.note(
                    format!("{}: {}. Save keeps your own copy.", s.title, s.about),
                    false,
                );
            }
        }
        self.persist();
    }

    /// Shows `file` in the ST editor.
    pub fn open_file(&mut self, file: &str) {
        if self.files.contains_key(file) {
            file.clone_into(&mut self.current);
        }
    }

    /// The text of the file in the ST editor.
    pub fn current_text_mut(&mut self) -> Option<&mut String> {
        self.files.get_mut(&self.current)
    }

    /// The open project's build: its `.st` files, one as it is, several
    /// each under a `(* file NAME *)` line.
    #[must_use]
    pub fn build_src(&self) -> String {
        let st: Vec<(&String, &String)> = self
            .files
            .iter()
            .filter(|(n, _)| file_name(n) == Some(FileKind::St))
            .collect();
        if let [(_, text)] = st.as_slice() {
            return (*text).clone();
        }
        let mut out = String::new();
        for (name, text) in st {
            out.push_str("(* file ");
            out.push_str(name);
            out.push_str(" *)\n");
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }

    /// The file and line a line of the build is.
    #[must_use]
    pub fn locate(&self, line: u32) -> Option<(String, u32)> {
        let st: Vec<&String> = self
            .files
            .keys()
            .filter(|n| file_name(n) == Some(FileKind::St))
            .collect();
        match st.as_slice() {
            [one] => Some(((*one).clone(), line)),
            _ => file_line(&self.build_src(), line),
        }
    }

    fn note(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some((text.into(), error));
    }

    /// Writes the visitor's projects and which is open; a failure is
    /// noted once, and the projects stay in memory.
    fn persist(&mut self) {
        let text = render(&self.saved, &self.open);
        if let Err(e) = self.store.save(&text)
            && self.storage_problem.is_none()
        {
            self.storage_problem = Some(format!(
                "this browser does not keep projects ({e}): they last until the page closes"
            ));
        }
    }

    /// A free name for a copy of `base` (neither a project here nor an
    /// example): `base`, else `base-2`, `-3`, ...
    fn free(&self, base: &str) -> String {
        let base: String = base.chars().take(44).collect();
        let taken = |n: &str| self.saved.contains_key(n) || self.examples.contains_key(n);
        if !taken(&base) {
            return base;
        }
        // At most 256 + 64 names are taken, so one of these is free.
        (2..=400)
            .map(|k| format!("{base}-{k}"))
            .find(|n| !taken(n))
            .unwrap_or(base)
    }

    /// Saves the open project; an example is saved as a copy, which opens.
    pub fn save(&mut self) {
        let files = Files(self.files.clone());
        if let Err(e) = files.check() {
            self.note(format!("Not saved: {}", e.join("; ")), true);
            return;
        }
        if self.is_example() {
            let name = self.free(&self.open);
            let from = std::mem::replace(&mut self.open, name.clone());
            self.saved.insert(name.clone(), files);
            self.note(
                format!("{from} is a built-in example: saved your copy as {name}."),
                false,
            );
        } else {
            self.saved.insert(self.open.clone(), files);
            self.note(format!("Saved {}.", self.open), false);
        }
        self.persist();
    }

    /// The open project's shared diagram layout (`""`: none), as edited.
    #[must_use]
    pub fn layout(&self) -> &str {
        self.files
            .get(helmsman_project::LAYOUT)
            .map_or("", String::as_str)
    }

    /// Why the open project cannot keep a default layout, if it cannot.
    ///
    /// # Errors
    /// It is a built-in example.
    pub fn can_save_layout(&self) -> Result<(), String> {
        if self.is_example() {
            return Err(
                "A built-in example is read-only: Save your own copy first, then save its layout.".into(),
            );
        }
        Ok(())
    }

    /// Saves `text` as the open project's shared diagram layout, that one
    /// file alone (other edits stay unsaved). Whether it was saved.
    pub fn save_layout(&mut self, text: &str) -> bool {
        if let Err(e) = self.can_save_layout() {
            self.note(e, true);
            return false;
        }
        if let Err(e) = helmsman_project::check_file(helmsman_project::LAYOUT, text.as_bytes()) {
            self.note(format!("The layout was not saved: {e}"), true);
            return false;
        }
        let mut stored = self.base().cloned().unwrap_or_default();
        stored.0.insert(helmsman_project::LAYOUT.into(), text.to_string());
        if let Err(e) = stored.check() {
            self.note(format!("The layout was not saved: {}", e.join("; ")), true);
            return false;
        }
        self.files
            .insert(helmsman_project::LAYOUT.into(), text.to_string());
        self.saved.insert(self.open.clone(), stored);
        self.note(
            format!("Saved this arrangement as {}'s default layout.", self.open),
            false,
        );
        self.persist();
        true
    }

    /// A new project with a starter program, opened.
    pub fn create(&mut self, name: &str, from: Option<&str>) {
        if !project_name(name) {
            self.note(
                format!("{name:?} is not a project name: 1 to 48 of a-z 0-9 _ -"),
                true,
            );
            return;
        }
        if self.exists(name) {
            self.note(format!("{name} already exists."), true);
            return;
        }
        let files = match from {
            Some(f) if f == self.open => Files(self.files.clone()),
            Some(f) => match self.saved.get(f).or_else(|| self.examples.get(f)) {
                Some(files) => files.clone(),
                None => return,
            },
            None => Files(BTreeMap::from([("main.st".to_string(), STARTER.to_string())])),
        };
        self.saved.insert(name.to_string(), files);
        self.open_project(name);
        self.note(format!("Made {name}."), false);
    }

    pub fn rename(&mut self, from: &str, to: &str) {
        if !project_name(to) || self.exists(to) {
            self.note(format!("{to:?} is taken or not a project name."), true);
            return;
        }
        let Some(files) = self.saved.remove(from) else {
            return;
        };
        self.saved.insert(to.to_string(), files);
        if self.open == from {
            to.clone_into(&mut self.open);
        }
        if self.running.as_deref() == Some(from) {
            self.running = Some(to.to_string());
        }
        self.note(format!("Renamed {from} to {to}."), false);
        self.persist();
    }

    pub fn delete(&mut self, name: &str) {
        if self.saved.remove(name).is_none() {
            return;
        }
        if self.running.as_deref() == Some(name) {
            self.running = None;
        }
        self.note(format!("Deleted {name}."), false);
        if self.open == name {
            let first = example_project(EXAMPLES[0].src).0;
            self.open_project(&first);
        } else {
            self.persist();
        }
    }

    pub fn new_file(&mut self, file: &str) {
        if file_name(file).is_none() {
            self.note(
                format!(
                    "{file:?} is not a project file name: {}",
                    helmsman_project::FILE_NAMES
                ),
                true,
            );
            return;
        }
        if self.files.keys().any(|f| f.eq_ignore_ascii_case(file)) {
            self.note(format!("{file} already exists."), true);
            return;
        }
        self.files.insert(file.to_string(), String::new());
        file.clone_into(&mut self.current);
    }

    pub fn rename_file(&mut self, from: &str, to: &str) {
        if file_name(to).is_none() || self.files.contains_key(to) {
            self.note(format!("{to:?} is taken or not a project file name."), true);
            return;
        }
        let Some(text) = self.files.remove(from) else {
            return;
        };
        self.files.insert(to.to_string(), text);
        if self.current == from {
            to.clone_into(&mut self.current);
        }
    }

    pub fn delete_file(&mut self, file: &str) {
        if self.files.remove(file).is_some() && self.current == file {
            self.current = self.files.keys().next().cloned().unwrap_or_default();
        }
    }

    /// The open project as a `.zip` (as edited here), and its file name.
    ///
    /// # Errors
    /// The files break a rule (too many, too large).
    pub fn zip(&self) -> Result<(String, Vec<u8>), String> {
        let files = Files(self.files.clone());
        files.check().map_err(|e| e.join("; "))?;
        Ok((format!("{}.zip", self.open), files.to_zip()?))
    }

    /// A file the visitor picked: a `.zip` becomes the project its name
    /// says (a taken name waits for Replace); `.st`, `PLCopen` `.xml` and
    /// Logix `.L5X` (ladder in the converted subset) go into the open
    /// project; anything else is refused, as the box does.
    pub fn upload(&mut self, file: &str, bytes: &[u8]) {
        let ext = file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("zip") => match helmsman_project::read_zip(bytes) {
                Ok(files) => {
                    let mut name = slug(file);
                    if !self.saved.contains_key(&name) && self.examples.contains_key(&name) {
                        name = self.free(&name);
                    }
                    if self.saved.contains_key(&name) {
                        self.pending = Some((name.clone(), files));
                        self.note(
                            format!("A project named {name} is already here. Replace it?"),
                            true,
                        );
                    } else {
                        self.install(&name, files);
                    }
                }
                Err(e) => self.note(format!("Upload refused: the archive is refused: {e}"), true),
            },
            Some("st" | "xml" | "l5x") => self.import(file, bytes),
            _ => self.note(
                format!(
                    "{file} was refused: upload takes .zip, .st, .xml (PLCopen TC6) or .L5X (Logix ladder)"
                ),
                true,
            ),
        }
    }

    /// Installs the upload waiting for Replace.
    pub fn replace(&mut self) {
        if let Some((name, files)) = self.pending.take() {
            self.install(&name, files);
        }
    }

    fn install(&mut self, name: &str, files: Files) {
        self.saved.insert(name.to_string(), files);
        self.open_project(name);
        self.note(format!("Uploaded {name}."), false);
    }

    fn import(&mut self, file: &str, bytes: &[u8]) {
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        let target = format!("{stem}.st");
        if file_name(&target) != Some(FileKind::St) {
            self.note(
                format!("{file}: not a project file name (NAME.st, NAME 1 to 64 of A-Z a-z 0-9 _ -)"),
                true,
            );
            return;
        }
        if ext.eq_ignore_ascii_case("l5x") {
            match l5x(file, bytes) {
                Ok(st) => {
                    self.files.insert(target.clone(), st);
                    self.current.clone_from(&target);
                    self.note(format!("Imported {file} into {} as {target}.", self.open), false);
                }
                Err(e) => self.note(e, true),
            }
            return;
        }
        let text = match helmsman_project::check_text(bytes) {
            Ok(t) => t.to_string(),
            Err(e) => {
                self.note(format!("{file} was refused: {e}"), true);
                return;
            }
        };
        let text = if ext.eq_ignore_ascii_case("xml") {
            match helmsman_iec::plcopen::lower(&text) {
                Ok(l) => l.st().to_string(),
                Err(diags) => {
                    let lines: Vec<String> = diags.iter().take(5).map(ToString::to_string).collect();
                    self.note(
                        format!("{file} is not a PLCopen project this reads: {}", lines.join("; ")),
                        true,
                    );
                    return;
                }
            }
        } else {
            text
        };
        self.files.insert(target.clone(), text);
        self.current.clone_from(&target);
        self.note(format!("Imported {file} into {} as {target}.", self.open), false);
    }
}

/// An L5X export as the box imports it (helmsman-plc's `api/projects.rs`):
/// ladder in the converted subset becomes Structured Text
/// ([`helmsman_iec::ladder::convert`]); anything else refuses the whole
/// file, every problem with its rung (the first 20, then how many more).
fn l5x(file: &str, bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{file}: not UTF-8"))?;
    let converted = helmsman_iec::ladder::convert(text).map_err(|r| {
        let mut lines: Vec<String> = r.problems.iter().take(20).map(ToString::to_string).collect();
        if r.total > lines.len() {
            lines.push(format!("and {} more", r.total - lines.len()));
        }
        format!("{file} is not converted: {}", lines.join("; "))
    })?;
    helmsman_project::check_text(converted.st.as_bytes()).map_err(|e| format!("{file} was refused: {e}"))?;
    Ok(converted.st)
}

fn render(saved: &BTreeMap<String, Files>, open: &str) -> String {
    json!({
        "format": FORMAT,
        "open": open,
        "projects": saved.iter().map(|(n, f)| (n.clone(), json!(f.0))).collect::<serde_json::Map<_, _>>(),
    })
    .to_string()
}

/// What [`render`] wrote, each project held to the rules (one that breaks
/// them is dropped, not trusted).
fn parse(text: &str) -> Option<(BTreeMap<String, Files>, Option<String>)> {
    let j: Json = serde_json::from_str(text).ok()?;
    if j["format"] != FORMAT {
        return None;
    }
    let mut out = BTreeMap::new();
    for (name, files) in j["projects"].as_object()? {
        let Some(files) = files.as_object() else { continue };
        let files = Files(
            files
                .iter()
                .filter_map(|(f, t)| Some((f.clone(), t.as_str()?.to_string())))
                .collect(),
        );
        if project_name(name) && files.check().is_ok() {
            out.insert(name.clone(), files);
        }
    }
    Some((out, j["open"].as_str().map(str::to_string)))
}
