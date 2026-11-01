//! Projects in the Editor, the same on a box (`box_projects`) and in the
//! simulation (`sim_projects`), as docs/UI.md lays them out: the "Files"
//! sidebar (the projects, which one runs, the open project's files, new,
//! copy, rename and delete), and what the header asks of whichever is
//! open ([`Workspace`]: the project's name and dirty marker, Save, Deploy,
//! Upload, Download).
//!
//! Nothing here talks to a box or to storage: the sidebar answers with an
//! [`Act`], and the view carries it out.

use std::collections::HashSet;

use eframe::egui::{self, Color32, RichText, Vec2};
use helmsman_project::starters::{Kind, STARTERS, starter};

use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::st_view::brand::{STEEL, WHITE, YELLOW};

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// A compile error in a project: which file, where, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl Diag {
    /// `main.st:3:5  expected ;`
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}:{}:{}  {}", self.file, self.line, self.col, self.message)
    }
}

/// A project as the sidebar lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub name: String,
    /// It is what runs (on a box: the last deployed and still current;
    /// in the simulation: deployed in this page).
    pub running: bool,
    /// The list it is under.
    pub group: Group,
}

impl ProjectRow {
    /// A built-in project: read-only, copied to edit.
    #[must_use]
    pub fn example(&self) -> bool {
        self.group != Group::Mine
    }
}

/// The sidebar's lists of projects, in the order they show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// The person's own.
    Mine,
    /// The small built-in examples, always in view.
    FirstSteps,
    /// Worked machines, folded until asked for.
    Machines,
    /// Starting points, folded until asked for.
    Templates,
}

/// What the sidebar asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    OpenProject(String),
    OpenFile(String),
    NewProject(String),
    CopyProject {
        from: String,
        to: String,
    },
    /// A new project from a built-in machine or template.
    FromStarter {
        from: String,
        to: String,
    },
    RenameProject {
        from: String,
        to: String,
    },
    DeleteProject(String),
    NewFile(String),
    RenameFile {
        from: String,
        to: String,
    },
    DeleteFile(String),
    /// A file's text to the sidebar's clipboard (right-click › Copy).
    CopyFile(String),
    /// A copy of a file beside it, under a free name (right-click ›
    /// Duplicate).
    DuplicateFile(String),
    /// The clipboard's file into the open project under a free name
    /// (right-click › Paste); works across projects.
    PasteFile {
        name: String,
        text: String,
    },
    /// The Agent's answer as a new project (a PROGRAM) or a new file in
    /// the open project, named after what it declares; `save`: the open
    /// project's unsaved edits are saved first, else dropped.
    UseAnswer {
        name: String,
        text: String,
        save: bool,
    },
}

/// What the sidebar shows.
pub struct Listing<'a> {
    pub projects: &'a [ProjectRow],
    pub open: Option<&'a str>,
    /// The open project's files, each with whether it has unsaved edits.
    pub files: Vec<(String, bool)>,
    pub current: Option<&'a str>,
    /// Whether this page may write, or why not.
    pub write: Result<(), String>,
    /// The open project has unsaved edits (opening another asks first).
    pub dirty: bool,
    /// Offer the built-in machines and templates to start a project from
    /// (a box; the simulation lists them as projects).
    pub starters: bool,
}

/// Which name the sidebar's text field is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    NewProject,
    CopyProject(String),
    FromStarter(String),
    RenameProject(String),
    NewFile,
    RenameFile(String),
}

/// The sidebar's own state: a name being typed, a delete being confirmed.
#[derive(Debug, Default)]
pub struct FilesUi {
    pub naming: Option<Naming>,
    pub input: String,
    pub confirm: Option<Act>,
    /// The long lists opened by hand.
    pub unfolded: HashSet<Group>,
    /// The list of machines and templates to start from is open.
    pub starters_open: bool,
    /// The file last copied (name, text): the app's own, not the
    /// system's, so a paste works in any open project here.
    pub clipboard: Option<(String, String)>,
}

/// A file name not taken, from `want`: `want` itself, else `stem-2.ext`,
/// `stem-3.ext`, ... (`taken` says which names are).
#[must_use]
pub fn free_file_name(want: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(want) {
        return want.to_string();
    }
    let (stem, ext) = want.rsplit_once('.').unwrap_or((want, "st"));
    (2..=400)
        .map(|k| format!("{stem}-{k}.{ext}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| want.to_string())
}

/// A right-click menu's item: whether it was clicked (the menu closes).
fn menu_item(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, enabled: bool) -> bool {
    let b = egui::Button::new(text).min_size(Vec2::new(ui.available_width().max(110.0), TAP * 0.6));
    let r = ui.add_enabled(enabled, b);
    chrome.hit(key, r.rect);
    if r.clicked() {
        ui.close();
        return true;
    }
    false
}

/// A button that is disabled, with why on hover, unless writes are allowed.
fn write_button(ui: &mut egui::Ui, write: &Result<(), String>, text: &str, hint: &str) -> egui::Response {
    let b = egui::Button::new(text).min_size(Vec2::new(0.0, TAP * 0.7));
    match write {
        Ok(()) => ui.add(b).on_hover_text(hint),
        Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
    }
}

impl FilesUi {
    /// The projects, then the open project's files. Returns what was asked.
    pub fn show(&mut self, ui: &mut egui::Ui, l: &Listing<'_>, chrome: &mut Chrome) -> Option<Act> {
        let mut act = None;
        let mut shown = Group::Mine;
        for p in l.projects {
            if p.group != shown {
                shown = p.group;
                self.group_header(ui, l, shown, chrome);
            }
            if !self.group_open(l, p.group) {
                continue;
            }
            let open = l.open == Some(p.name.as_str());
            let mut text = format!("{} {}", if open { "▾" } else { "▸" }, p.name);
            if p.running {
                text.push_str("  ● running");
            }
            let color = if p.running { YELLOW } else { WHITE };
            let r = ui.selectable_label(open, RichText::new(text).monospace().color(color));
            chrome.hit(format!("project {}", p.name), r.rect);
            let r = match starter(&p.name).filter(|_| p.example()) {
                Some(s) => r.on_hover_text(format!("{}: {}", s.title, s.about)),
                None => r,
            };
            if r.clicked() && !open {
                let a = Act::OpenProject(p.name.clone());
                if l.dirty {
                    self.naming = None;
                    self.confirm = Some(a);
                } else {
                    act = Some(a);
                }
            }
        }
        if l.projects.is_empty() {
            ui.label(RichText::new("No projects yet.").color(STEEL));
        }
        ui.horizontal_wrapped(|ui| {
            let r = write_button(ui, &l.write, "New", "a new, empty project");
            chrome.hit("new project", r.rect);
            if r.clicked() {
                self.ask(Naming::NewProject, "");
            }
            if let Some(open) = l.open {
                let is_example = l.projects.iter().any(|p| p.name == open && p.example());
                let r = write_button(ui, &l.write, "Copy", "a copy of the open project");
                chrome.hit("copy project", r.rect);
                if r.clicked() {
                    self.ask(Naming::CopyProject(open.into()), &format!("{open}-copy"));
                }
                if !is_example {
                    let r = write_button(ui, &l.write, "Rename", "rename the open project");
                    chrome.hit("rename project", r.rect);
                    if r.clicked() {
                        self.ask(Naming::RenameProject(open.into()), open);
                    }
                    let r = write_button(ui, &l.write, "Delete", "delete the open project");
                    chrome.hit("delete project", r.rect);
                    if r.clicked() {
                        self.confirm = Some(Act::DeleteProject(open.into()));
                    }
                }
            }
        });
        if l.starters {
            self.starters_ui(ui, &l.write, chrome);
        }
        if let Some(open) = l.open {
            ui.separator();
            let is_example = l.projects.iter().any(|p| p.name == open && p.example());
            let can_write = l.write.is_ok() && !is_example;
            let head = ui.label(RichText::new(format!("{open}: files")).strong().color(STEEL));
            // Right-click the heading: paste into this project.
            head.context_menu(|ui| {
                let paste = self.clipboard.is_some() && can_write;
                if menu_item(ui, chrome, "menu paste", "Paste", paste)
                    && let Some((name, text)) = self.clipboard.clone()
                {
                    act = Some(Act::PasteFile { name, text });
                }
            });
            for (f, dirty) in &l.files {
                let here = l.current == Some(f.as_str());
                let text = format!("{f}{}", if *dirty { " ●" } else { "" });
                let r = ui.selectable_label(here, RichText::new(text).monospace());
                chrome.hit(format!("file {f}"), r.rect);
                if r.clicked() && !here {
                    act = Some(Act::OpenFile(f.clone()));
                }
                // Right-click a file: copy, paste, duplicate, rename, delete
                // (Nick, 2026-11-01: easy copy and paste of files).
                r.context_menu(|ui| {
                    if menu_item(ui, chrome, "menu copy", "Copy", true) {
                        act = Some(Act::CopyFile(f.clone()));
                    }
                    let paste = self.clipboard.is_some() && can_write;
                    if menu_item(ui, chrome, "menu paste", "Paste", paste)
                        && let Some((name, text)) = self.clipboard.clone()
                    {
                        act = Some(Act::PasteFile { name, text });
                    }
                    if menu_item(ui, chrome, "menu duplicate", "Duplicate", can_write) {
                        act = Some(Act::DuplicateFile(f.clone()));
                    }
                    if menu_item(ui, chrome, "menu rename", "Rename…", can_write) {
                        self.ask(Naming::RenameFile(f.clone()), f);
                    }
                    if menu_item(ui, chrome, "menu delete", "Delete…", can_write) {
                        self.naming = None;
                        self.confirm = Some(Act::DeleteFile(f.clone()));
                    }
                });
            }
            if !is_example {
                ui.horizontal_wrapped(|ui| {
                    let r = write_button(ui, &l.write, "New file", "a new .st file in this project");
                    chrome.hit("new file", r.rect);
                    if r.clicked() {
                        self.ask(Naming::NewFile, ".st");
                    }
                    if let Some(cur) = l.current {
                        let r = write_button(ui, &l.write, "Rename", "rename the open file");
                        chrome.hit("rename file", r.rect);
                        if r.clicked() {
                            self.ask(Naming::RenameFile(cur.into()), cur);
                        }
                        let r = write_button(ui, &l.write, "Delete", "delete the open file");
                        chrome.hit("delete file", r.rect);
                        if r.clicked() {
                            self.confirm = Some(Act::DeleteFile(cur.into()));
                        }
                    }
                });
            }
        }
        if let Some(a) = self.naming_ui(ui, chrome) {
            act = Some(a);
        }
        if let Some(a) = self.confirm_ui(ui, chrome) {
            act = Some(a);
        }
        if l.write.is_err() {
            self.naming = None;
            if !matches!(self.confirm, Some(Act::OpenProject(_))) {
                self.confirm = None;
            }
        }
        act
    }

    /// Whether `g`'s rows show: the short lists always; a long one once
    /// unfolded, or while the open project is in it.
    fn group_open(&self, l: &Listing<'_>, g: Group) -> bool {
        match g {
            Group::Mine | Group::FirstSteps => true,
            Group::Machines | Group::Templates => {
                self.unfolded.contains(&g)
                    || l.projects
                        .iter()
                        .any(|p| p.group == g && l.open == Some(p.name.as_str()))
            }
        }
    }

    /// A list's heading; a long list's folds on a click.
    fn group_header(&mut self, ui: &mut egui::Ui, l: &Listing<'_>, g: Group, chrome: &mut Chrome) {
        let title = match g {
            Group::Mine => return,
            Group::FirstSteps => {
                ui.add_space(4.0);
                ui.label(RichText::new("Examples").strong().color(STEEL));
                return;
            }
            Group::Machines => "Machines",
            Group::Templates => "Templates",
        };
        let open = self.group_open(l, g);
        let n = l.projects.iter().filter(|p| p.group == g).count();
        let text = format!("{} {title} ({n})", if open { "▾" } else { "▸" });
        ui.add_space(4.0);
        let r = ui.selectable_label(false, RichText::new(text).strong().color(STEEL));
        chrome.hit(format!("group {title}"), r.rect);
        if r.clicked() && !self.unfolded.remove(&g) {
            self.unfolded.insert(g);
        }
    }

    /// "Start from a machine or template": the same list the simulation
    /// has; a click asks for the new project's name.
    fn starters_ui(&mut self, ui: &mut egui::Ui, write: &Result<(), String>, chrome: &mut Chrome) {
        let arrow = if self.starters_open { "▾" } else { "▸" };
        let r = ui.selectable_label(
            false,
            RichText::new(format!("{arrow} Start from a machine or template")).color(STEEL),
        );
        chrome.hit("starters", r.rect);
        if r.clicked() {
            self.starters_open = !self.starters_open;
        }
        if !self.starters_open {
            return;
        }
        for (kind, title) in [(Kind::Machine, "Machines"), (Kind::Template, "Templates")] {
            ui.label(RichText::new(title).strong().color(STEEL));
            for s in STARTERS.iter().filter(|s| s.kind == kind) {
                let r = write_button(ui, write, s.title, s.about);
                chrome.hit(format!("starter {}", s.name), r.rect);
                if r.clicked() {
                    self.ask(Naming::FromStarter(s.name.into()), s.name);
                }
            }
        }
    }

    fn ask(&mut self, n: Naming, start: &str) {
        self.naming = Some(n);
        self.input = start.into();
        self.confirm = None;
    }

    fn naming_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) -> Option<Act> {
        let n = self.naming.clone()?;
        let what = match &n {
            Naming::NewProject => "New project".to_string(),
            Naming::CopyProject(p) => format!("Copy {p} as"),
            Naming::FromStarter(p) => format!("New project from {p}, named"),
            Naming::RenameProject(p) => format!("Rename {p} to"),
            Naming::NewFile => "New file".to_string(),
            Naming::RenameFile(f) => format!("Rename {f} to"),
        };
        let hint = match &n {
            Naming::NewFile | Naming::RenameFile(_) => "NAME.st",
            _ => "a-z 0-9 _ -",
        };
        let mut out = None;
        ui.separator();
        ui.label(RichText::new(what).strong());
        let t = ui.add(
            egui::TextEdit::singleline(&mut self.input)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );
        chrome.hit("name", t.rect);
        let enter = t.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            let ok = ui.button("OK");
            chrome.hit("name ok", ok.rect);
            let no = ui.button("Cancel");
            chrome.hit("name cancel", no.rect);
            if ok.clicked() || enter {
                let name = self.input.trim().to_string();
                if !name.is_empty() {
                    out = Some(match n.clone() {
                        Naming::NewProject => Act::NewProject(name),
                        Naming::CopyProject(from) => Act::CopyProject { from, to: name },
                        Naming::FromStarter(from) => Act::FromStarter { from, to: name },
                        Naming::RenameProject(from) => Act::RenameProject { from, to: name },
                        Naming::NewFile => Act::NewFile(name),
                        Naming::RenameFile(from) => Act::RenameFile { from, to: name },
                    });
                }
                self.naming = None;
            } else if no.clicked() {
                self.naming = None;
            }
        });
        out
    }

    fn confirm_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) -> Option<Act> {
        let a = self.confirm.clone()?;
        let (what, verb, stay) = match &a {
            Act::DeleteProject(p) => (format!("Delete project {p} and all its files?"), "Delete", "Keep"),
            Act::DeleteFile(f) => (format!("Delete {f}?"), "Delete", "Keep"),
            Act::OpenProject(p) => (
                format!("The edits here are not saved and are dropped if you open {p}."),
                "Open, drop them",
                "Stay",
            ),
            Act::UseAnswer { name, .. } => (
                format!("The edits here are not saved. Save them before opening {name}, or drop them?"),
                "Save, then open",
                "Stay",
            ),
            _ => return None,
        };
        let mut out = None;
        ui.separator();
        ui.colored_label(RED, what);
        ui.horizontal(|ui| {
            let yes = ui.button(verb);
            chrome.hit("confirm yes", yes.rect);
            if let Act::UseAnswer { name, text, .. } = &a {
                let drop = ui.button("Drop them, then open");
                chrome.hit("confirm drop", drop.rect);
                if drop.clicked() {
                    out = Some(Act::UseAnswer {
                        name: name.clone(),
                        text: text.clone(),
                        save: false,
                    });
                    self.confirm = None;
                }
            }
            let no = ui.button(stay);
            chrome.hit("confirm no", no.rect);
            if yes.clicked() {
                out = Some(a.clone());
                self.confirm = None;
            } else if no.clicked() {
                self.confirm = None;
            }
        });
        out
    }
}

/// The compile errors, each a link to its file and line. Returns the one
/// clicked.
pub fn diags_ui(ui: &mut egui::Ui, diags: &[Diag], chrome: &mut Chrome) -> Option<Diag> {
    let mut out = None;
    for (k, d) in diags.iter().enumerate() {
        let r = ui
            .add(
                egui::Label::new(RichText::new(d.label()).monospace().color(RED)).sense(egui::Sense::click()),
            )
            .on_hover_text("open the file at this line");
        chrome.hit(format!("diag {k}"), r.rect);
        if r.clicked() {
            out = Some(d.clone());
        }
    }
    out
}

/// What the header asks of the open project, wherever it lives.
pub trait Workspace {
    /// Whether it has edits not saved.
    fn dirty(&self) -> bool;
    /// Whether this page may write (save, deploy, upload), or why not.
    fn writable(&self) -> Result<(), String>;
    fn save(&mut self);
    fn deploy(&mut self);
    /// A save or deploy is on its way.
    fn busy(&self) -> bool;
    /// A file the user picked: a `.zip` becomes a project, an `.st` or
    /// `.xml` (and, refused on a box with its reason, `.l5x`) goes into
    /// the open project.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    fn upload(&mut self, file: &str, bytes: Vec<u8>);
    /// The open project as a `.zip`.
    fn download(&mut self);
    /// The View menu's layout items (`arrange::menu_items`), acting on
    /// this view's diagram.
    fn layout_menu(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome);
    /// Fits the open diagram to its area again.
    fn zoom_to_fit(&mut self);
    /// Undoes (or redoes) the last edit in the ST editor; whether it did.
    fn undo(&mut self, ctx: &egui::Context, redo: bool) -> bool;
    /// Sends the ST editor to the declaration of the picked block's
    /// variable (Go to definition); whether there was one.
    fn go_to_definition(&mut self, chrome: &mut Chrome) -> bool;
    /// Lights every read and write of the picked block's variable on the
    /// diagram (Cross references); whether there was one.
    fn cross_references(&mut self) -> bool;
}

/// The file and line a line of a project build is from: the build is every
/// file under a `(* file NAME *)` line (the box's `project::build`, and the
/// simulation's). `None` before the first such line.
#[must_use]
pub fn file_line(src: &str, line: u32) -> Option<(String, u32)> {
    let mut at = None;
    for (k, l) in src.lines().enumerate().take(line as usize) {
        if let Some(name) = l
            .trim()
            .strip_prefix("(* file ")
            .and_then(|r| r.strip_suffix(" *)"))
        {
            at = Some((name.to_string(), u32::try_from(k + 1).unwrap_or(u32::MAX)));
        }
    }
    let (file, header) = at?;
    (line > header).then(|| (file, line - header))
}

/// A project name from a file the user picked (`Line 3 (copy).zip` →
/// `line-3-copy`): lower case, `a-z 0-9 _ -`, at most 48.
#[must_use]
pub fn slug(file: &str) -> String {
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let mut out = String::new();
    for c in stem.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut out = out.trim_end_matches('-').to_string();
    out.truncate(48);
    if out.is_empty() { "project".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_line_maps_back_to_its_file() {
        let src = "(* file a.st *)\nPROGRAM a\nEND_PROGRAM\n(* file b.st *)\nFUNCTION_BLOCK b\n";
        assert_eq!(file_line(src, 1), None, "the header itself");
        assert_eq!(file_line(src, 2), Some(("a.st".into(), 1)));
        assert_eq!(file_line(src, 3), Some(("a.st".into(), 2)));
        assert_eq!(file_line(src, 5), Some(("b.st".into(), 1)));
        assert_eq!(file_line("PROGRAM x\n", 1), None, "not a build");
    }

    #[test]
    fn a_picked_file_names_a_project() {
        assert_eq!(slug("Line 3 (copy).zip"), "line-3-copy");
        assert_eq!(slug("conveyor_2.zip"), "conveyor_2");
        assert_eq!(slug("....zip"), "project");
        assert_eq!(slug("ÜBER.zip"), "ber");
        assert!(slug(&"x".repeat(100)).len() <= 48);
    }
}
