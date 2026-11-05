//! Projects in the Editor, the same on a box (`box_projects`) and in the
//! simulation (`sim_projects`), as docs/UI.md lays them out: the "Files"
//! sidebar (the projects, which one runs, the open project's files, new,
//! copy, rename and delete), and what the header asks of whichever is
//! open ([`Workspace`]: the project's name and dirty marker, Save, Deploy,
//! Upload, Download).
//!
//! Nothing here talks to a box or to storage: the sidebar answers with an
//! [`Act`], and the view carries it out.

use eframe::egui::{self, Color32, Key, Modifiers, RichText, Vec2};

use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

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
    /// The built-in examples, always in view: a click asks for a name and
    /// makes the person's own copy.
    FirstSteps,
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
    /// Show revision n of the open project (the History row).
    ShowRevision(u32),
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
    /// The open project is the one running: its `main.st` is marked.
    pub running: bool,
    /// The open project's revisions, oldest first (the History row).
    pub history: &'a [crate::history_ui::RevisionRow],
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

/// The row last clicked in the sidebar: what the Delete key is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Project(String),
    File(String),
}

/// The sidebar's own state: a name being typed, a delete being confirmed.
#[derive(Debug, Default)]
pub struct FilesUi {
    pub naming: Option<Naming>,
    pub input: String,
    /// A question with the page's answer pending, asked in the middle of
    /// the screen ([`Self::confirm_ui`]).
    pub confirm: Option<Act>,
    /// The project or file last clicked; the Delete key deletes it (or
    /// the open project when nothing was).
    pub picked: Option<Picked>,
    /// The earlier iterations (`vN.st`) are unfolded.
    pub earlier_open: bool,
    /// The History row is unfolded.
    pub history_open: bool,
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
    // A fixed floor, not `available_width()`: see editor::menu_item_keyed.
    let b = egui::Button::new(text).min_size(Vec2::new(110.0, TAP * 0.6));
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
        let t = theme(ui);
        let mut act = None;
        let mut shown = Group::Mine;
        for p in l.projects {
            if p.group != shown {
                shown = p.group;
                group_header(ui, shown);
            }
            let open = l.open == Some(p.name.as_str());
            let mut text = format!("{} {}", if open { "▾" } else { "▸" }, p.name);
            if p.running {
                text.push_str("  ● running");
            }
            let color = if p.running { t.accent } else { t.fg };
            let r = ui.selectable_label(open, RichText::new(text).monospace().color(color));
            chrome.hit(format!("project {}", p.name), r.rect);
            if p.example() {
                let title = example_title(&p.name).unwrap_or(&p.name);
                let r = r.tip(format!(
                    "{title}: click to make your own copy of this example, under a name you choose."
                ));
                if r.clicked() {
                    let name = free_name(l, &p.name);
                    self.ask(Naming::FromStarter(p.name.clone()), &name);
                }
                continue;
            }
            let r = r.tip(if open {
                "Your program, open. Right-click to copy, rename or delete it."
            } else {
                "Your program: click to open it. Right-click to copy, rename or delete it."
            });
            if r.clicked() {
                self.picked = Some(Picked::Project(p.name.clone()));
            }
            if r.clicked() && !open {
                let a = Act::OpenProject(p.name.clone());
                if l.dirty {
                    self.naming = None;
                    self.confirm = Some(a);
                } else {
                    act = Some(a);
                }
            }
            // Right-click a project: copy it, and for one of your own
            // rename or delete it (Nick, 2026-11-01). Rename and Delete
            // live only here and on the Delete key, not as buttons
            // (Nick, 2026-11-02).
            let can_write = l.write.is_ok();
            let mine = !p.example();
            r.context_menu(|ui| {
                if menu_item(ui, chrome, "menu copy project", "Copy project…", can_write) {
                    self.copy_project(&p.name);
                }
                if menu_item(
                    ui,
                    chrome,
                    "menu rename project",
                    "Rename project…",
                    can_write && mine,
                ) {
                    self.ask(Naming::RenameProject(p.name.clone()), &p.name);
                }
                if menu_item(
                    ui,
                    chrome,
                    "menu delete project",
                    "Delete project…",
                    can_write && mine,
                ) {
                    self.naming = None;
                    self.confirm = Some(Act::DeleteProject(p.name.clone()));
                }
            });
        }
        if l.projects.is_empty() {
            ui.label(RichText::new("No projects yet.").color(t.muted));
        }
        if l.starters {
            self.starters_ui(ui, l, chrome);
        }
        if let Some(open) = l.open {
            ui.separator();
            let is_example = l.projects.iter().any(|p| p.name == open && p.example());
            let can_write = l.write.is_ok() && !is_example;
            let head = ui.label(RichText::new(format!("{open}: files")).strong().color(t.muted));
            // Right-click the heading: paste into this project.
            head.context_menu(|ui| {
                let paste = self.clipboard.is_some() && can_write;
                if menu_item(ui, chrome, "menu paste", "Paste", paste)
                    && let Some((name, text)) = self.clipboard.clone()
                {
                    act = Some(Act::PasteFile { name, text });
                }
            });
            // The program's files, then the earlier iterations folded under
            // one row, newest first (Nick, 2026-11-01).
            let mut earlier: Vec<&(String, bool)> = l
                .files
                .iter()
                .filter(|(f, _)| helmsman_project::iteration_number(f).is_some())
                .collect();
            earlier.sort_by_key(|(f, _)| std::cmp::Reverse(helmsman_project::iteration_number(f)));
            // Requirement tests live behind their rows in the Requirements
            // tab (Nick, 2026-11-05), not here.
            let mut rows: Vec<&(String, bool)> = l
                .files
                .iter()
                .filter(|(f, _)| {
                    helmsman_project::iteration_number(f).is_none() && !helmsman_iec::spec::is_test(f)
                })
                .collect();
            if !earlier.is_empty() {
                let arrow = if self.earlier_open { "▾" } else { "▸" };
                let r = ui.selectable_label(
                    false,
                    RichText::new(format!("{arrow} earlier ({})", earlier.len()))
                        .monospace()
                        .color(t.muted),
                );
                chrome.hit("earlier", r.rect);
                if r.clicked() {
                    self.earlier_open = !self.earlier_open;
                }
                if self.earlier_open {
                    rows.extend(earlier);
                }
            }
            for (f, dirty) in rows {
                let here = l.current == Some(f.as_str());
                let runs = l.running && f == helmsman_project::MAIN && !l.files.is_empty();
                let text = format!(
                    "{f}{}{}",
                    if *dirty { " ●" } else { "" },
                    if runs { "  · runs" } else { "" }
                );
                let r = ui.selectable_label(here, RichText::new(text).monospace());
                chrome.hit(format!("file {f}"), r.rect);
                if r.clicked() {
                    self.picked = Some(Picked::File(f.clone()));
                    if !here {
                        act = Some(Act::OpenFile(f.clone()));
                    }
                }
                // A double click opens the file in the Code tab (Nick,
                // 2026-11-02).
                if r.double_clicked() {
                    chrome.show_code();
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
                let r = write_button(ui, &l.write, "New file", "a new .st file in this project");
                chrome.hit("new file", r.rect);
                if r.clicked() {
                    self.ask(Naming::NewFile, ".st");
                }
            }
        }
        if l.open.is_some()
            && let Some(n) = crate::history_ui::rows_ui(ui, chrome, l.history, &mut self.history_open)
        {
            act = Some(Act::ShowRevision(n));
        }
        if let Some(a) = self.delete_key(ui, l) {
            self.naming = None;
            self.confirm = Some(a);
        }
        if let Some(a) = self.naming_ui(ui, chrome) {
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

    /// The Delete key (Backspace on a Mac keyboard) with the pane up and
    /// no text field focused: what to delete, asked first. The file or
    /// project last clicked; with nothing clicked, the open project. A
    /// built-in example is never offered (Nick, 2026-11-02).
    fn delete_key(&mut self, ui: &egui::Ui, l: &Listing<'_>) -> Option<Act> {
        if l.write.is_err() || self.naming.is_some() || self.confirm.is_some() {
            return None;
        }
        if ui.memory(|m| m.focused().is_some()) {
            return None;
        }
        let pressed = ui.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)
        });
        if !pressed {
            return None;
        }
        let mine = |name: &str| l.projects.iter().any(|p| p.name == name && !p.example());
        let open_mine = l.open.filter(|o| mine(o));
        match &self.picked {
            Some(Picked::File(f)) if open_mine.is_some() && l.files.iter().any(|(n, _)| n == f) => {
                Some(Act::DeleteFile(f.clone()))
            }
            Some(Picked::Project(p)) if mine(p) => Some(Act::DeleteProject(p.clone())),
            _ => open_mine.map(|o| Act::DeleteProject(o.into())),
        }
    }

    /// The examples on a box (the simulation lists them with the
    /// projects): a click asks for the new program's name.
    fn starters_ui(&mut self, ui: &mut egui::Ui, l: &Listing<'_>, chrome: &mut Chrome) {
        let t = theme(ui);
        ui.add_space(4.0);
        ui.label(RichText::new("Examples").strong().color(t.muted))
            .tip("Short programs that teach one thing each. Click one to make it a program of your own on this box.");
        for e in &crate::playground::EXAMPLES {
            let (name, _) = crate::sim_projects::example_project(e.src);
            let r = write_button(
                ui,
                &l.write,
                e.name,
                "makes your own copy of this example, under a name you choose",
            );
            chrome.hit(format!("starter {name}"), r.rect);
            if r.clicked() {
                let free = free_name(l, &name);
                self.ask(Naming::FromStarter(name), &free);
            }
        }
    }

    fn ask(&mut self, n: Naming, start: &str) {
        self.naming = Some(n);
        self.input = start.into();
        self.confirm = None;
    }

    /// Asks for a new, empty project's name (File › New project, or a
    /// right-click on the pane's blank space).
    pub fn new_project(&mut self) {
        self.ask(Naming::NewProject, "");
    }

    /// Asks for the name of a copy of `project` (File › Copy project, or
    /// a right-click on the project).
    pub fn copy_project(&mut self, project: &str) {
        self.ask(Naming::CopyProject(project.into()), &format!("{project}-copy"));
    }

    /// The pane's blank space under everything else: a right-click there
    /// offers a new project, and a paste when the clipboard has a file
    /// (`can_paste`). Returns what was asked.
    pub fn blank_menu(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        write: &Result<(), String>,
        can_paste: bool,
    ) -> Option<Act> {
        // What is left of the view (the pane scrolls: its own height is
        // unbounded), at least a tap.
        let left = (ui.clip_rect().bottom() - ui.cursor().top()).max(TAP);
        let size = Vec2::new(ui.available_width(), left);
        let r = ui.allocate_response(size, egui::Sense::click());
        chrome.hit("files blank", r.rect);
        let mut act = None;
        r.context_menu(|ui| {
            if menu_item(ui, chrome, "menu new project", "New project…", write.is_ok()) {
                self.new_project();
            }
            let paste = can_paste && write.is_ok() && self.clipboard.is_some();
            if menu_item(ui, chrome, "menu paste", "Paste", paste)
                && let Some((name, text)) = self.clipboard.clone()
            {
                act = Some(Act::PasteFile { name, text });
            }
        });
        act
    }

    fn naming_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) -> Option<Act> {
        let n = self.naming.clone()?;
        let what = match &n {
            Naming::NewProject => "New project".to_string(),
            Naming::CopyProject(p) => format!("Copy {p} as"),
            Naming::FromStarter(p) => format!(
                "Name your program (from the {} example)",
                example_title(p).unwrap_or(p)
            ),
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

    /// The pending question ([`Self::confirm`]) as a box in the middle of
    /// the screen over a dimmed page, whichever tab is up: its buttons,
    /// Escape or a click outside answer it (Nick, 2026-11-02: every
    /// confirmation a popup in the centre). The view draws it each frame
    /// after the Editor; returns what was agreed to.
    pub fn confirm_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<Act> {
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
        let m = egui::Modal::new(egui::Id::unique("files-confirm")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(RichText::new(what).strong().size(16.0));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let yes = ui.add(egui::Button::new(RichText::new(verb).color(RED)).min_size(tall));
                chrome.hit("confirm yes", yes.rect);
                if let Act::UseAnswer { name, text, .. } = &a {
                    let drop = ui.add(egui::Button::new("Drop them, then open").min_size(tall));
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
                let no = ui.add(egui::Button::new(stay).min_size(tall));
                chrome.hit("confirm no", no.rect);
                if yes.clicked() {
                    out = Some(a.clone());
                    self.confirm = None;
                } else if no.clicked() {
                    self.confirm = None;
                }
            });
        });
        chrome.hit("confirm", m.response.rect);
        if m.should_close() {
            self.confirm = None;
        }
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
    /// File › New project: asks for the name, on the Files tab.
    fn new_project(&mut self, chrome: &mut Chrome);
    /// File › Copy project: asks for the copy's name, on the Files tab.
    fn copy_project(&mut self, chrome: &mut Chrome);
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

/// An example's title from its project name.
fn example_title(name: &str) -> Option<&'static str> {
    crate::playground::EXAMPLES
        .iter()
        .find(|e| crate::sim_projects::example_project(e.src).0 == name)
        .map(|e| e.name)
}

/// `base`, or `base-2`, `base-3`, ... whichever no project has yet.
fn free_name(l: &Listing<'_>, base: &str) -> String {
    let taken = |n: &str| l.projects.iter().any(|p| p.name == n);
    if !taken(base) {
        return base.to_string();
    }
    // One more name tried than there are projects: one of them is free.
    let last = l.projects.len() + 2;
    (2..=last)
        .map(|k| format!("{base}-{k}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| format!("{base}-{last}"))
}

/// A list's heading.
fn group_header(ui: &mut egui::Ui, g: Group) {
    let t = theme(ui);
    if g == Group::FirstSteps {
        ui.add_space(4.0);
        ui.label(RichText::new("Examples").strong().color(t.muted)).tip(
            "Short programs that teach one thing each. Click one to make your own copy under a name you choose.",
        );
    }
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
