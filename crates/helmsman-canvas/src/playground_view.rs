//! The Editor on the simulation in the page (docs/playground/, or any page
//! no box serves): an ST editor that checks as you type (errors marked
//! after a pause; what runs is unchanged until Deploy, as on a box), the
//! program running in the page with Run / Stop / Step, a virtual I/O
//! panel built from the program's ports, a trend, the live diagram, and
//! Claude with the visitor's own key. No box and no server: the state is
//! [`crate::playground::Session`] and [`crate::claude::Chat`].

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, RichText, Sense, Stroke, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::{Ty, Value};

use crate::arrange::Layers;
use crate::claude::{Chat, Model, Phase, Proxy, Request, Segment, segments};
use crate::editor::{self, Body, Chrome, Tab};
use crate::playground::{
    MAX_PERIOD, MAX_SOURCE, MIN_PERIOD, Point, Session, TREND_MS, TREND_SIGNALS, Widget,
};
use crate::projects::{self, Act, Diag, FilesUi, Listing, Workspace};
use crate::sim_projects::{SimProjects, Storage};
use crate::st_view::brand::{BLACK, GRAPHITE, STEEL, WHITE, YELLOW};
use crate::st_view::{Pick, TAP, Viewer};

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Quiet time after the last keystroke before the editor checks, s.
const DEBOUNCE: f64 = 0.35;
/// Trend line colours, in signal order.
const TREND: [Color32; TREND_SIGNALS] = [
    YELLOW,
    Color32::from_rgb(0x6E, 0xC6, 0xFF),
    Color32::from_rgb(0x9B, 0xE5, 0x8C),
    Color32::from_rgb(0xFF, 0x8A, 0x65),
    Color32::from_rgb(0xD5, 0x9C, 0xFF),
    WHITE,
];

/// Where Claude requests go: the browser, or (natively) a list tests read.
#[derive(Default)]
pub struct Link {
    #[cfg(target_arch = "wasm32")]
    web: crate::web::ClaudeLink,
    /// Requests "sent" (native builds have no network here).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<(u64, Request)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub inbox: Vec<crate::claude::Incoming>,
    /// Answers stopped.
    #[cfg(not(target_arch = "wasm32"))]
    pub aborted: usize,
}

impl Link {
    fn send(&mut self, seq: u64, req: Request, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.send(seq, req, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.sent.push((seq, req));
        }
    }

    fn abort(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.web.abort();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.aborted += 1;
        }
    }

    fn check_proxy(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.check_proxy(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.inbox.push(crate::claude::Incoming::Proxy(false));
        }
    }

    fn drain(&mut self) -> Vec<crate::claude::Incoming> {
        #[cfg(target_arch = "wasm32")]
        return self.web.drain();
        #[cfg(not(target_arch = "wasm32"))]
        std::mem::take(&mut self.inbox)
    }
}

pub struct PlaygroundView {
    pub session: Session,
    /// The diagram of the program running, rebuilt when it changes.
    pub viewer: Option<Viewer>,
    /// Its shared default layout (the project's) and this browser's
    /// tweaks (`arrange.rs`).
    pub layers: Layers,
    /// The viewer was just built: lay it out from the layers.
    fresh: bool,
    viewer_of: String,
    /// When the editor last changed and is not checked yet.
    pending: Option<f64>,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
    pub chat: Chat,
    pub link: Link,
    proxy_asked: bool,
    /// A one-line note under the top bar (loaded an example, …).
    pub notice: Option<String>,
    /// Why the last "Ask" could not be sent.
    pub ask_error: Option<String>,
    /// The projects, kept in the browser's storage.
    pub proj: SimProjects,
    pub files_ui: FilesUi,
    /// Downloads made (natively nothing saves them: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub downloads: Vec<(String, Vec<u8>)>,
}

impl PlaygroundView {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let projects: Box<dyn Storage> = Box::new(crate::web::LocalProjects);
        #[cfg(not(target_arch = "wasm32"))]
        let projects: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_storage(projects)
    }

    /// The simulation with its projects kept in `projects`; it opens the
    /// project open last (else the first example), running.
    #[must_use]
    pub fn with_storage(projects: Box<dyn Storage>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let tweaks: Box<dyn Storage> = Box::new(crate::web::LocalArrangements);
        #[cfg(not(target_arch = "wasm32"))]
        let tweaks: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_stores(projects, tweaks)
    }

    /// [`Self::with_storage`], with the diagram's personal layouts kept in
    /// `tweaks`.
    #[must_use]
    pub fn with_stores(projects: Box<dyn Storage>, tweaks: Box<dyn Storage>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::web::LocalKeyStore);
        #[cfg(not(target_arch = "wasm32"))]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::claude::NoStore);
        let proj = SimProjects::load(projects);
        let mut v = Self {
            session: Session::new(&proj.build_src()),
            viewer: None,
            layers: Layers::load(tweaks),
            fresh: false,
            viewer_of: String::new(),
            pending: None,
            last_pick: None,
            chat: Chat::new(store),
            link: Link::default(),
            proxy_asked: false,
            notice: None,
            ask_error: None,
            proj,
            files_ui: FilesUi::default(),
            #[cfg(not(target_arch = "wasm32"))]
            downloads: Vec::new(),
        };
        v.session.set_running(true);
        v.proj.running = Some(v.proj.open.clone());
        v.sync_viewer();
        v
    }

    /// Loads the open project's build into the session, compiled and
    /// stopped.
    fn reload(&mut self) {
        let src = self.proj.build_src();
        self.session.load(&src);
        self.pending = None;
        self.sync_viewer();
    }

    /// Opens project `name`, compiled and stopped (Deploy runs it).
    pub fn open_project(&mut self, name: &str) {
        self.proj.open_project(name);
        self.reload();
        self.notice = None;
    }

    /// The editor's text changed: the session's source follows at once
    /// and is checked after the pause. What runs waits for Deploy.
    fn edited(&mut self, t: f64) {
        self.session.src = self.proj.build_src();
        self.pending = Some(t);
        self.notice = None;
    }

    /// The compile errors, each with its file and line.
    #[must_use]
    pub fn diags(&self) -> Vec<Diag> {
        self.session
            .diags
            .iter()
            .map(|d| {
                let (file, line) = self
                    .proj
                    .locate(d.pos.line)
                    .unwrap_or_else(|| (self.proj.current.clone(), d.pos.line));
                Diag {
                    file,
                    line,
                    col: d.pos.col,
                    message: d.msg.clone(),
                }
            })
            .collect()
    }

    /// Opens `file` at `line` in the ST editor.
    fn goto(&mut self, file: &str, line: u32, chrome: &mut Chrome) {
        self.proj.open_file(file);
        chrome.goto(line);
    }

    /// Carries out what the Files sidebar asked.
    fn act(&mut self, a: Act) {
        match a {
            Act::OpenProject(p) => self.open_project(&p),
            Act::OpenFile(f) => self.proj.open_file(&f),
            Act::NewProject(p) => {
                self.proj.create(&p, None);
                self.reload();
            }
            Act::CopyProject { from, to } | Act::FromStarter { from, to } => {
                self.proj.create(&to, Some(&from));
                self.reload();
            }
            Act::RenameProject { from, to } => self.proj.rename(&from, &to),
            Act::DeleteProject(p) => {
                let open = self.proj.open == p;
                self.proj.delete(&p);
                if open {
                    self.reload();
                }
            }
            Act::NewFile(f) => self.proj.new_file(&f),
            Act::RenameFile { from, to } => {
                self.proj.rename_file(&from, &to);
                self.edited(f64::NEG_INFINITY);
            }
            Act::DeleteFile(f) => {
                self.proj.delete_file(&f);
                self.edited(f64::NEG_INFINITY);
            }
        }
    }

    /// Rebuilds the diagram when the program running changed.
    fn sync_viewer(&mut self) {
        let Some(b) = &self.session.build else {
            self.viewer = None;
            return;
        };
        if b.src == self.viewer_of && self.viewer.is_some() {
            return;
        }
        self.viewer_of.clone_from(&b.src);
        let old = self.viewer.as_ref().and_then(|v| v.because.clone());
        // Blocks the user moved stay moved across edits: kept as tweaks,
        // and the new diagram is laid out from the layers.
        if let Some(v) = &self.viewer {
            self.layers.track(v);
        }
        self.fresh = true;
        self.viewer = b.module.clone().map(|m| {
            let mut v = Viewer::new(&b.src, m);
            // Opens with no path lit; the user's pick survives an edit
            // while its variable does.
            v.because = old.filter(|o| v.knows(o));
            v
        });
    }

    /// Loads `src` into the editor (the open file, or the project's first
    /// program file when a map or settings file is open), compiled and
    /// stopped.
    pub fn load(&mut self, src: &str, what: &str) {
        if crate::sim_projects::is_st(&self.proj.current) {
            if let Some(t) = self.proj.current_text_mut() {
                src.clone_into(t);
            }
        } else if let Some(f) = self
            .proj
            .files
            .keys()
            .find(|f| crate::sim_projects::is_st(f))
            .cloned()
        {
            self.proj.files.insert(f.clone(), src.to_string());
            self.proj.current = f;
        } else {
            self.proj.files.insert("main.st".into(), src.to_string());
            self.proj.current = "main.st".into();
        }
        let ok = self.session.load(&self.proj.build_src());
        self.pending = None;
        self.sync_viewer();
        self.notice = Some(if ok {
            format!("Loaded {what}. Press Run to start it.")
        } else {
            format!("Loaded {what}; it does not compile yet (see the errors under the editor).")
        });
    }

    /// Checks the editor if it has been quiet long enough (or `now`):
    /// errors are marked, and the program running (and its diagram) stay
    /// as deployed.
    fn check_due(&mut self, t: f64, now: bool) {
        if let Some(at) = self.pending
            && (now || t - at >= DEBOUNCE)
        {
            self.pending = None;
            let src = self.session.src.clone();
            self.session.check(&src);
        }
    }

    /// Picks output `name`, as a tap on it does.
    pub fn pick(&mut self, name: &str) {
        if let Some(v) = &mut self.viewer {
            v.because = Some(name.to_string());
        }
    }

    /// What the header names: the open project.
    #[must_use]
    pub fn title(&self) -> String {
        self.proj.open.clone()
    }

    /// The PROGRAM running, in lower case (the Monitor's name for it);
    /// the project's name while nothing compiled.
    #[must_use]
    pub fn program_name(&self) -> String {
        self.viewer
            .as_ref()
            .and_then(|v| v.module.program())
            .map_or_else(|| self.title(), |p| p.name.to_ascii_lowercase())
    }

    /// Sends the typed question about the editor's program.
    pub fn ask(&mut self, ctx: &egui::Context) {
        let diags = self.session.diag_lines();
        match self.chat.ask(&self.session.src, &diags) {
            Ok((seq, req)) => {
                self.ask_error = None;
                self.link.send(seq, req, ctx);
            }
            Err(e) => self.ask_error = Some(e),
        }
    }

    pub fn stop_answer(&mut self) {
        self.chat.stop();
        self.link.abort();
    }

    /// Loads the program in Claude's answer (never runs it).
    pub fn use_answer(&mut self) -> bool {
        let Some(code) = self.chat.answer.as_ref().and_then(crate::claude::Answer::program) else {
            return false;
        };
        self.load(&code, "Claude's program");
        true
    }

    /// One frame of the Editor.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.tick(&ui.ctx().clone());
        editor::show(ui, chrome, self);
    }

    /// Runs the program (and Claude's answer) on with nothing drawn: the
    /// Monitor watches the same session.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.proxy_asked {
            self.proxy_asked = true;
            self.link.check_proxy(ctx);
        }
        for m in self.link.drain() {
            self.chat.receive(m);
        }
        let t = ctx.input(|i| i.time);
        self.check_due(t, false);
        self.arrange();
        self.session.advance(t);
        if self.session.running || self.pending.is_some() || self.chat.busy() {
            ctx.request_repaint();
        }
    }
}

impl PlaygroundView {
    /// Lays the diagram out from its layers, or keeps a finished move.
    pub fn arrange(&mut self) {
        let fresh = std::mem::take(&mut self.fresh);
        if let Some(v) = &mut self.viewer {
            let key = format!("sim:{}", self.proj.open);
            self.layers.show(v, &key, self.proj.layout(), fresh);
        }
    }

    /// Saves the diagram as it stands as the open project's default.
    pub fn save_layout(&mut self) {
        let Some(v) = &self.viewer else { return };
        let text = v.arrangement().to_text();
        if self.proj.save_layout(&text) {
            self.layers.adopt(v, &text);
        }
    }
}

impl Body for PlaygroundView {
    fn toolbar(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        {
            let can = self.proj.can_save_layout();
            if crate::arrange::buttons(ui, chrome, &mut self.layers, self.viewer.as_mut(), can) {
                self.save_layout();
            }
            ui.separator();
            let s = &mut self.session;
            let (label, fill) = if s.running {
                ("■ Stop", GRAPHITE)
            } else {
                ("▶ Run", YELLOW)
            };
            let ink = if s.running { WHITE } else { BLACK };
            let run = egui::Button::new(RichText::new(label).strong().color(ink))
                .fill(fill)
                .min_size(Vec2::new(80.0, 28.0));
            if ui.add_enabled(s.build.is_some(), run).clicked() {
                let on = !s.running;
                s.set_running(on);
            }
            if ui
                .add_enabled(!s.running && s.build.is_some(), egui::Button::new("Step"))
                .on_hover_text("one scan")
                .clicked()
            {
                s.step();
            }
            if ui
                .add_enabled(s.build.is_some(), egui::Button::new("Restart"))
                .on_hover_text("cold restart: initial values, clock to zero")
                .clicked()
            {
                s.restart();
            }
            ui.label(RichText::new("scan").color(STEEL));
            let mut p = s.period_ms;
            if ui
                .add(
                    egui::DragValue::new(&mut p)
                        .range(MIN_PERIOD..=MAX_PERIOD)
                        .suffix(" ms"),
                )
                .changed()
            {
                s.set_period(p);
            }
            ui.separator();
            let state = if s.fault.is_some() {
                "faulted"
            } else if s.running {
                "running"
            } else {
                "stopped"
            };
            #[allow(clippy::cast_precision_loss)]
            let secs = s.now_ms as f64 / 1000.0;
            ui.label(
                RichText::new(format!("{state} · {} scans · t={secs:.1}s", s.scans))
                    .monospace()
                    .color(if s.fault.is_some() { RED } else { STEEL }),
            );
        }
        if let Some(f) = &self.session.fault {
            ui.colored_label(RED, format!("Fault: {f}. Run or Step restarts the program."));
        }
        if let Some(n) = &self.notice {
            ui.label(RichText::new(n).color(STEEL));
        }
        if let Some((n, error)) = self.proj.notice.clone() {
            let r = ui.label(RichText::new(n).color(if error { RED } else { STEEL }));
            chrome.hit("notice", r.rect);
            if error && self.proj.pending.is_some() {
                let r = ui.button("Replace");
                chrome.hit("replace", r.rect);
                if r.clicked() {
                    self.proj.replace();
                    self.reload();
                }
            }
        }
        if let Some(why) = &self.proj.storage_problem {
            let r = ui.label(RichText::new(format!("⚠ {why}")).color(STEEL));
            chrome.hit("storage", r.rect);
        }
    }

    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = ui.ctx().input(|i| i.time);
        let stale = self.session.stale();
        let errors = self.session.diags.len();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Structured Text").strong());
            if self.pending.is_some() {
                ui.label(RichText::new("…").color(STEEL));
            } else if errors > 0 {
                ui.label(RichText::new(format!("✖ {errors} error(s)")).color(RED));
            } else {
                ui.label(RichText::new("✔ compiles").color(STEEL));
            }
        });
        // An edit is only checked: what runs is the last deploy, as on a box.
        if stale && self.pending.is_none() && self.session.build.is_some() {
            let note = if errors > 0 {
                "The program running is unchanged. Fix the errors, then Deploy."
            } else {
                "The program running is unchanged. Deploy runs this version."
            };
            ui.label(RichText::new(note).small().color(STEEL));
        }
        let all = self.diags();
        let current = self.proj.current.clone();
        let diags: Vec<(u32, u32)> = all
            .iter()
            .filter(|d| d.file == current)
            .map(|d| (d.line, d.col))
            .collect();
        let avail = ui.available_height();
        let list_h = if self.session.diags.is_empty() { 0.0 } else { 110.0 };
        let size = Vec2::new(ui.available_width(), (avail - list_h).max(120.0));
        let Some(text) = self.proj.current_text_mut() else {
            ui.label(RichText::new("No file open: make one in Files.").color(STEEL));
            return;
        };
        let r = ui
            .allocate_ui(size, |ui| {
                editor::st_editor(
                    ui,
                    egui::Id::unique("pg-source"),
                    text,
                    &diags,
                    MAX_SOURCE,
                    chrome,
                )
            })
            .inner;
        if r.changed() {
            self.edited(t);
        }
        if !self.session.diags.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("pg-diags")
                .max_height(list_h)
                .show(ui, |ui| {
                    if let Some(d) = projects::diags_ui(ui, &all, chrome) {
                        self.goto(&d.file, d.line, chrome);
                    }
                });
        }
    }

    /// The projects (the visitor's, then the examples), the open one's
    /// files, and under them the program's outline: every block instance
    /// it calls, nested; a click shows that one's diagram.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let rows = self.proj.rows();
        let files: Vec<(String, bool)> = self
            .proj
            .files
            .keys()
            .map(|f| (f.clone(), self.proj.file_dirty(f)))
            .collect();
        let open = self.proj.open.clone();
        let current = self.proj.current.clone();
        let listing = Listing {
            projects: &rows,
            open: Some(&open),
            files,
            current: Some(&current),
            write: Ok(()),
            dirty: self.proj.dirty(),
            starters: false,
        };
        if let Some(a) = self.files_ui.show(ui, &listing, chrome) {
            self.act(a);
        }
        ui.separator();
        ui.label(RichText::new("Program").strong().color(STEEL));
        match &mut self.viewer {
            Some(v) => {
                ui.indent("pg-tree", |ui| v.tree_ui(ui));
            }
            None => {
                ui.label(RichText::new("Nothing compiled yet.").color(STEEL));
            }
        }
    }

    fn tabs(&self) -> &'static [Tab] {
        &[Tab::Io, Tab::Trend, Tab::Claude]
    }

    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab) {
        match tab {
            Tab::Io => self.io(ui),
            Tab::Trend => self.trend_tab(ui),
            Tab::Claude => self.claude(ui),
        }
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        if let Some(v) = &mut self.viewer {
            v.crumbs_ui(ui, &[]);
            let area = ui.available_rect_before_wrap();
            v.canvas(ui, scan);
            let mut jump = None;
            if v.pick != self.last_pick {
                self.last_pick = v.pick;
                if let Some((line, _)) = v.picked() {
                    jump = Some(line);
                }
            }
            let reads = v.reads_on_screen();
            self.map_inputs(ui, area, &reads);
            if let Some(line) = jump {
                match self.proj.locate(line) {
                    Some((file, l)) => self.goto(&file, l, chrome),
                    None => chrome.goto(line),
                }
            }
            return;
        }
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, BLACK);
        let why = self
            .session
            .build
            .as_ref()
            .and_then(|b| b.module_error.clone())
            .unwrap_or_else(|| "Write a PROGRAM that compiles to see its diagram.".into());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            why,
            FontId::proportional(14.0),
            STEEL,
        );
    }
}

impl PlaygroundView {
    fn io(&mut self, ui: &mut egui::Ui) {
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(STEEL));
            return;
        }
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        let outputs: Vec<Point> = self.session.outputs().cloned().collect();
        let on_map = self.on_map();
        let off_map: Vec<&Point> = inputs
            .iter()
            .filter(|p| !on_map.iter().any(|r| r.eq_ignore_ascii_case(&p.name)))
            .collect();
        egui::ScrollArea::vertical().id_salt("pg-io").show(ui, |ui| {
            ui.label(RichText::new("Inputs").strong());
            if inputs.is_empty() {
                ui.label(
                    RichText::new("No VAR_INPUT or %IX bit: add one to get a control.")
                        .small()
                        .color(STEEL),
                );
            } else if off_map.len() < inputs.len() {
                ui.label(
                    RichText::new("Switch, press or drag the inputs on their blocks in the diagram.")
                        .small()
                        .color(STEEL),
                );
            }
            for p in off_map {
                if let Some(v) = input_ui(ui, p) {
                    self.session.set_input(&p.name, v);
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Outputs").strong())
                .on_hover_text("click: pick it");
            if outputs.is_empty() {
                ui.label(
                    RichText::new("No VAR_OUTPUT or %QX bit: add one to get a lamp.")
                        .small()
                        .color(STEEL),
                );
            }
            for p in &outputs {
                if output_ui(ui, p) {
                    self.pick(&p.name);
                }
            }
        });
    }

    /// The signals to trend (outputs, then inputs) and the chart.
    fn trend_tab(&mut self, ui: &mut egui::Ui) {
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(STEEL));
            return;
        }
        ui.label(RichText::new(format!("Pin up to {TREND_SIGNALS} signals")).color(STEEL));
        let points: Vec<String> = self
            .session
            .outputs()
            .chain(self.session.inputs())
            .map(|p| p.name.clone())
            .collect();
        ui.horizontal_wrapped(|ui| {
            for name in &points {
                let on = self.session.trend.signals.contains(name);
                if ui.selectable_label(on, name.to_ascii_lowercase()).clicked() {
                    self.session.trend.toggle(name);
                }
            }
        });
        ui.add_space(6.0);
        self.trend(ui);
    }

    fn claude(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().id_salt("pg-claude").show(ui, |ui| {
            match self.chat.proxy {
                Proxy::Yes => {
                    ui.label(
                        RichText::new("Using the local proxy: the API key stays on this machine.")
                            .color(STEEL),
                    );
                }
                Proxy::Checking => {
                    ui.label(RichText::new("Checking for a local proxy…").color(STEEL));
                }
                Proxy::No => self.key_ui(ui),
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Model").color(STEEL));
                egui::ComboBox::from_id_salt("pg-model")
                    .selected_text(self.chat.model.label())
                    .show_ui(ui, |ui| {
                        for m in Model::ALL {
                            ui.selectable_value(&mut self.chat.model, m, m.label());
                        }
                    });
            });
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::multiline(&mut self.chat.question)
                    .id_salt("pg-question")
                    .hint_text("e.g. Add a stop button that resets the counter")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                let busy = self.chat.busy();
                let ask = egui::Button::new(RichText::new("Ask").strong().color(BLACK))
                    .fill(YELLOW)
                    .min_size(Vec2::new(TAP * 1.5, TAP * 0.75));
                if ui
                    .add_enabled(!busy, ask)
                    .on_hover_text("sends the question with the editor's program and its errors")
                    .clicked()
                {
                    self.ask(&ctx);
                }
                if busy && ui.button("Stop").clicked() {
                    self.stop_answer();
                }
                if !self.chat.history.is_empty() && !busy && ui.button("New conversation").clicked() {
                    self.chat.clear();
                }
            });
            if let Some(e) = &self.ask_error {
                ui.colored_label(RED, e);
            }
            ui.separator();
            self.answer_ui(ui);
        });
    }

    fn key_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Anthropic API key").strong());
        let r = ui.add(
            egui::TextEdit::singleline(self.chat.key_mut())
                .id_salt("pg-key")
                .password(true)
                .hint_text("sk-ant-…")
                .desired_width(f32::INFINITY),
        );
        if r.changed() {
            self.chat.key_changed();
        }
        if self.chat.has_key() && !self.chat.key_looks_right() {
            ui.label(
                RichText::new("That does not look like an Anthropic key (sk-ant-…).")
                    .small()
                    .color(RED),
            );
        }
        let mut remember = self.chat.remember;
        if ui
            .checkbox(&mut remember, "Remember on this device")
            .on_hover_text("stores the key in this browser's localStorage")
            .changed()
        {
            self.chat.set_remember(remember);
        }
        if remember {
            ui.label(
                RichText::new(
                    "Stored in this browser's localStorage, readable by any script on this site. \
                     Untick, or Forget, on a shared computer.",
                )
                .small()
                .color(RED),
            );
        }
        ui.horizontal(|ui| {
            if self.chat.has_key() && ui.button("Forget key").clicked() {
                self.chat.forget_key();
            }
            ui.label(
                RichText::new(
                    "Held in this page only; sent only to api.anthropic.com, billed to your account.",
                )
                .small()
                .color(STEEL),
            );
        });
        ui.add_space(6.0);
    }

    fn answer_ui(&mut self, ui: &mut egui::Ui) {
        let Some(a) = &self.chat.answer else {
            ui.label(
                RichText::new(
                    "Ask for a program (\"a conveyor that stops on a jam\"), a fix for the \
                     errors, or what a line does. Claude sees the editor's program.",
                )
                .color(STEEL),
            );
            return;
        };
        if let Some(m) = &a.model
            && m != self.chat.model.id()
        {
            ui.label(RichText::new(format!("answered by {m}")).small().color(STEEL));
        }
        markdown(ui, &a.text);
        if a.cut {
            ui.label(RichText::new("(answer cut: too long)").small().color(STEEL));
        }
        match &a.phase {
            Phase::Waiting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("thinking…").color(STEEL));
                });
            }
            Phase::Streaming => {
                ui.spinner();
            }
            Phase::Failed(why) => {
                ui.colored_label(RED, why);
            }
            Phase::Done => {}
        }
        if a.stop_reason.as_deref() == Some("max_tokens") {
            ui.label(
                RichText::new("(the answer hit its length limit)")
                    .small()
                    .color(STEEL),
            );
        }
        let has_code = a.phase == Phase::Done && a.program().is_some();
        if has_code {
            let b = egui::Button::new(RichText::new("Use this code").strong().color(BLACK))
                .fill(YELLOW)
                .min_size(Vec2::new(TAP * 3.0, TAP * 0.8));
            if ui
                .add(b)
                .on_hover_text("replaces the editor's program; it does not run until you press Run")
                .clicked()
            {
                self.use_answer();
            }
        }
    }

    fn trend(&self, ui: &mut egui::Ui) {
        let trend = &self.session.trend;
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, BLACK);
        if trend.signals.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Pick signals to trend above",
                FontId::proportional(13.0),
                STEEL,
            );
            return;
        }
        let n = trend.signals.len();
        #[allow(clippy::cast_precision_loss)]
        let lane = rect.height() / n as f32;
        let end = trend.samples.back().map_or(0, |s| s.t_ms);
        let start = end.saturating_sub(TREND_MS);
        #[allow(clippy::cast_precision_loss)]
        let x_of =
            |t: u64| rect.left() + 90.0 + (rect.width() - 96.0) * ((t - start) as f32 / TREND_MS as f32);
        for (k, name) in trend.signals.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let top = rect.top() + lane * k as f32;
            let color = TREND[k % TREND.len()];
            let (lo, hi) = trend
                .samples
                .iter()
                .filter_map(|s| s.values.get(k).copied().filter(|v| v.is_finite()))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let last = trend.samples.back().and_then(|s| s.values.get(k).copied());
            painter.text(
                Pos2::new(rect.left() + 6.0, top + lane / 2.0),
                egui::Align2::LEFT_CENTER,
                format!(
                    "{}\n{}",
                    name.to_ascii_lowercase(),
                    last.filter(|v| v.is_finite())
                        .map_or("–".into(), |v| format!("{v}"))
                ),
                FontId::monospace(10.5),
                color,
            );
            painter.line_segment(
                [Pos2::new(rect.left(), top), Pos2::new(rect.right(), top)],
                Stroke::new(1.0, GRAPHITE),
            );
            let span = if hi > lo { hi - lo } else { 1.0 };
            let y_of = |v: f64| {
                #[allow(clippy::cast_possible_truncation)]
                let f = ((v - lo) / span) as f32;
                top + lane - 4.0 - (lane - 8.0) * f.clamp(0.0, 1.0)
            };
            let mut pts: Vec<Pos2> = Vec::new();
            for s in &trend.samples {
                let Some(v) = s.values.get(k).copied().filter(|v| v.is_finite()) else {
                    continue;
                };
                let p = Pos2::new(x_of(s.t_ms), y_of(v));
                // Steps, not slopes: a PLC value holds until the next scan.
                if let Some(prev) = pts.last().copied() {
                    pts.push(Pos2::new(p.x, prev.y));
                }
                pts.push(p);
            }
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
            }
        }
    }
}

impl PlaygroundView {
    /// Each input's control, drawn on its blocks in the diagram: a click
    /// flips a switch, a push button is on while held, a number drags
    /// sideways (or takes a typed value on a click).
    fn map_inputs(&mut self, ui: &mut egui::Ui, area: egui::Rect, reads: &[(String, egui::Rect)]) {
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        ui.set_clip_rect(area);
        // A push button read in two places is held if either is.
        let mut held: Vec<(String, bool)> = Vec::new();
        let mut set = Vec::new();
        for (k, (path, rect)) in reads.iter().enumerate() {
            let Some(p) = inputs.iter().find(|p| p.name.eq_ignore_ascii_case(path)) else {
                continue;
            };
            if !area.intersects(*rect) {
                continue;
            }
            match map_input_ui(ui, *rect, p, k) {
                MapInput::Held(on) => match held.iter_mut().find(|(n, _)| *n == p.name) {
                    Some((_, h)) => *h |= on,
                    None => held.push((p.name.clone(), on)),
                },
                MapInput::Set(v) => set.push((p.name.clone(), v)),
                MapInput::Nothing => {}
            }
        }
        for (name, on) in held {
            self.session.set_input(&name, Value::Bool(on));
        }
        for (name, v) in set {
            self.session.set_input(&name, v);
        }
    }

    /// The inputs the diagram shows a control for.
    pub(crate) fn on_map(&self) -> Vec<String> {
        let Some(v) = &self.viewer else { return Vec::new() };
        v.reads_on_screen().into_iter().map(|(path, _)| path).collect()
    }
}

/// What an input's control on the diagram did this frame.
enum MapInput {
    Nothing,
    /// A push button: whether it is held.
    Held(bool),
    Set(Value),
}

/// One input's control filling its block at `rect`.
fn map_input_ui(ui: &mut egui::Ui, rect: egui::Rect, p: &Point, k: usize) -> MapInput {
    let label = p.name.to_ascii_lowercase();
    let size = (rect.height() * 0.4).clamp(7.0, 18.0);
    let builder = egui::UiBuilder::new().max_rect(rect).id_salt(("pg-map", k));
    ui.scope_builder(builder, |ui| match p.widget {
        Some(Widget::Push) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(RichText::new(&label).strong().size(size).color(if on {
                BLACK
            } else {
                WHITE
            }))
            .fill(if on { YELLOW } else { GRAPHITE })
            .sense(Sense::click_and_drag());
            let r = ui
                .add_sized(rect.size(), b)
                .on_hover_text("push button: on while held");
            MapInput::Held(r.is_pointer_button_down_on())
        }
        Some(Widget::Toggle) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(format!("{label} {}", if on { "ON" } else { "OFF" }))
                    .strong()
                    .size(size)
                    .color(if on { BLACK } else { WHITE }),
            )
            .fill(if on { YELLOW } else { GRAPHITE });
            if ui
                .add_sized(rect.size(), b)
                .on_hover_text("switch: click to flip")
                .clicked()
            {
                MapInput::Set(Value::Bool(!on))
            } else {
                MapInput::Nothing
            }
        }
        Some(Widget::Int { min, max }) => {
            let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
            let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
            let mut v = match p.value {
                Value::Int(n) => i64::try_from(n).unwrap_or(0),
                _ => 0,
            };
            let before = v;
            let d = egui::DragValue::new(&mut v)
                .range(lo..=hi)
                .prefix(format!("{label} = "));
            ui.add_sized(rect.size(), d).on_hover_text(DRAG_HINT);
            if v == before {
                MapInput::Nothing
            } else {
                MapInput::Set(Value::Int(i128::from(v)))
            }
        }
        Some(Widget::Real) => {
            let mut v = match p.value {
                Value::Real(r) => r,
                _ => 0.0,
            };
            let before = v;
            let d = egui::DragValue::new(&mut v)
                .speed(0.1)
                .prefix(format!("{label} = "));
            ui.add_sized(rect.size(), d).on_hover_text(DRAG_HINT);
            if v.to_bits() != before.to_bits() && v.is_finite() {
                MapInput::Set(Value::Real(v))
            } else {
                MapInput::Nothing
            }
        }
        Some(Widget::Time) => {
            let mut ms = match p.value {
                Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                _ => 0,
            };
            let before = ms;
            let d = egui::DragValue::new(&mut ms)
                .range(0..=86_400_000)
                .prefix(format!("{label} = "))
                .suffix(" ms");
            ui.add_sized(rect.size(), d).on_hover_text(DRAG_HINT);
            if ms == before {
                MapInput::Nothing
            } else {
                MapInput::Set(Value::Int(i128::from(ms) * 1_000_000))
            }
        }
        None => MapInput::Nothing,
    })
    .inner
}

const DRAG_HINT: &str = "drag sideways to change, or click to type";

impl Workspace for PlaygroundView {
    fn dirty(&self) -> bool {
        self.proj.dirty()
    }

    fn writable(&self) -> Result<(), String> {
        Ok(())
    }

    fn save(&mut self) {
        self.proj.save();
    }

    /// Compiles the open project and runs it in the page: the one place
    /// an edit reaches the program running. One that does not compile is
    /// refused, its errors listed, and the program running is unchanged.
    fn deploy(&mut self) {
        let src = self.proj.build_src();
        let ok = self.session.set_source(&src);
        self.pending = None;
        self.sync_viewer();
        if ok {
            self.session.set_running(true);
            self.proj.running = Some(self.proj.open.clone());
            let program = self
                .viewer
                .as_ref()
                .and_then(|v| v.module.program())
                .map_or_else(String::new, |p| format!(": {}", p.name.to_ascii_lowercase()));
            self.notice = Some(format!("{} is running in this page{program}.", self.proj.open));
        } else {
            self.notice = Some(format!(
                "{} does not compile: {} problem(s), listed under the editor; click one to go there.",
                self.proj.open,
                self.session.diags.len()
            ));
        }
    }

    fn busy(&self) -> bool {
        false
    }

    fn upload(&mut self, file: &str, bytes: Vec<u8>) {
        let before = (self.proj.open.clone(), self.proj.build_src());
        self.proj.upload(file, &bytes);
        if before.0 != self.proj.open {
            self.reload();
        } else if before.1 != self.proj.build_src() {
            self.session.src = self.proj.build_src();
            self.pending = Some(f64::NEG_INFINITY);
        }
    }

    fn download(&mut self) {
        match self.proj.zip() {
            Ok((name, bytes)) => {
                #[cfg(target_arch = "wasm32")]
                crate::web::download_bytes(&name, &bytes, "application/zip");
                #[cfg(not(target_arch = "wasm32"))]
                self.downloads.push((name, bytes));
            }
            Err(e) => self.proj.notice = Some((format!("Cannot download: {e}"), true)),
        }
    }
}

impl Default for PlaygroundView {
    fn default() -> Self {
        Self::new()
    }
}

/// One input's control; the value it sets, if it changed (a push button
/// reports every frame).
fn input_ui(ui: &mut egui::Ui, p: &Point) -> Option<Value> {
    let label = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let mut out = None;
    ui.horizontal(|ui| match p.widget {
        Some(Widget::Push) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(&label)
                    .strong()
                    .color(if on { BLACK } else { WHITE }),
            )
            .fill(if on { YELLOW } else { GRAPHITE })
            .sense(Sense::click_and_drag())
            .min_size(Vec2::new(TAP * 3.0, TAP * 0.8));
            let r = ui.add(b).on_hover_text("push button: on while held");
            out = Some(Value::Bool(r.is_pointer_button_down_on()));
        }
        Some(Widget::Toggle) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(format!("{label}: {}", if on { "ON" } else { "OFF" }))
                    .strong()
                    .color(if on { BLACK } else { WHITE }),
            )
            .fill(if on { YELLOW } else { GRAPHITE })
            .min_size(Vec2::new(TAP * 3.0, TAP * 0.8));
            if ui.add(b).on_hover_text("switch: click to flip").clicked() {
                out = Some(Value::Bool(!on));
            }
        }
        Some(Widget::Int { min, max }) => {
            ui.label(&label);
            let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
            let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
            let (slo, shi) = (lo.max(-1000), hi.min(1000));
            let mut v = match p.value {
                Value::Int(n) => i64::try_from(n).unwrap_or(0),
                _ => 0,
            };
            let before = v;
            ui.add(egui::Slider::new(&mut v, slo..=shi).show_value(false));
            ui.add(egui::DragValue::new(&mut v).range(lo..=hi));
            if v != before {
                out = Some(Value::Int(i128::from(v)));
            }
        }
        Some(Widget::Real) => {
            ui.label(&label);
            let mut v = match p.value {
                Value::Real(r) => r,
                _ => 0.0,
            };
            let before = v;
            ui.add(egui::Slider::new(&mut v, -100.0..=100.0).show_value(false));
            ui.add(egui::DragValue::new(&mut v).speed(0.1));
            if v.to_bits() != before.to_bits() && v.is_finite() {
                out = Some(Value::Real(v));
            }
        }
        Some(Widget::Time) => {
            ui.label(&label);
            let mut ms = match p.value {
                Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                _ => 0,
            };
            let before = ms;
            ui.add(egui::Slider::new(&mut ms, 0..=10_000).show_value(false));
            ui.add(egui::DragValue::new(&mut ms).range(0..=86_400_000).suffix(" ms"));
            if ms != before {
                out = Some(Value::Int(i128::from(ms) * 1_000_000));
            }
        }
        None => {}
    });
    out
}

/// One output: a lamp (BOOL) or a readout. Returns whether it was clicked.
fn output_ui(ui: &mut egui::Ui, p: &Point) -> bool {
    let on = p.value == Value::Bool(true);
    let name = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let text = if p.ty == Ty::Bool {
        format!("{} {name}", if on { "●" } else { "○" })
    } else {
        format!("{name} = {}", show(p.ty, p.value))
    };
    let b =
        egui::Button::new(
            RichText::new(text)
                .monospace()
                .size(15.0)
                .color(if on { YELLOW } else { WHITE }),
        )
        .min_size(Vec2::new(ui.available_width(), TAP * 0.8));
    ui.add(b).on_hover_text("click: pick it").clicked()
}

/// A value for a readout (TIME in ms).
#[must_use]
pub fn show(ty: Ty, v: Value) -> String {
    match (ty, v) {
        (Ty::Time, Value::Int(ns)) => format!("{} ms", ns / 1_000_000),
        _ => v.to_string(),
    }
}

/// Answer text: paragraphs as labels, fenced code in monospace blocks.
fn markdown(ui: &mut egui::Ui, text: &str) {
    for s in segments(text) {
        match s {
            Segment::Prose(p) => {
                let t = p.trim();
                if !t.is_empty() {
                    ui.label(RichText::new(t).color(WHITE));
                }
            }
            Segment::Code { body, .. } => {
                egui::Frame::new()
                    .fill(BLACK)
                    .stroke(Stroke::new(1.0, GRAPHITE))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(body).monospace().size(12.0))
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
            }
        }
    }
}
