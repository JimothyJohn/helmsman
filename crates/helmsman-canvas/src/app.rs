//! The app: the always-on header over one screen, as docs/UI.md lays it
//! out. The screen is the Editor or the Monitor (or Proposals, from the ⋯
//! menu); what it watches is the [`Source`], a setting rather than a mode:
//! the box that served the page, or the simulation running in it. A phone
//! (a narrow window) opens on the Monitor, as do `?monitor` and the old
//! `?andon`.

use eframe::egui::{self, Rect, RichText, Vec2};

use crate::box_view::{BoxView, is_engineer};
use crate::editor::{Body, CHROME_KEY, Chrome, menu_item};
use crate::monitor::{Monitor, NARROW, Wish};
use crate::playground_view::PlaygroundView;
use crate::projects::Workspace;
use crate::review_view::ReviewView;
use crate::st_view::TAP;
use crate::st_view::brand::{BLACK, GRAPHITE, STEEL, WHITE, YELLOW};

/// The header's height, points: one row, as tall as the two it replaced.
pub const HEADER_H: f32 = 64.0;

/// Seconds between re-reads of the proposal list for the ⋯ badge.
const PROPOSALS_EVERY: f64 = 30.0;

/// What Upload's file picker offers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const UPLOAD_ACCEPT: &str = ".zip,.st,.xml,.l5x";

/// What the program runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The box that served the page, over its API.
    Box,
    /// The program running in this page against virtual I/O.
    Sim,
}

/// Which screen fills the window under the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Editor,
    /// Status tiles, phone to wall TV; watch only.
    Monitor,
    /// The box's proposals: diffs, sim replay, approve with a PIN.
    Proposals,
}

fn has(query: &str, key: &str) -> bool {
    query
        .trim_start_matches('?')
        .split('&')
        .any(|kv| kv.split('=').next() == Some(key))
}

/// Whether a page with this query string (`?a&b=c`) and canvas `data-mode`
/// runs the simulation rather than watching the box that served it:
/// docs/playground/'s page says `data-mode="playground"`, and the old
/// links' `?playground` and `?demo` still lead there.
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
#[must_use]
pub fn wants_sim(query: &str, mode: Option<&str>) -> bool {
    mode == Some("playground") || has(query, "playground") || has(query, "demo")
}

/// Whether a page with this query string opens on the Monitor: `?monitor`
/// (a wall TV's link), and the old floor screen's `?andon`. (`?site` is
/// gone.)
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
#[must_use]
pub fn wants_monitor(query: &str) -> bool {
    has(query, "monitor") || has(query, "andon")
}

// Independent UI flags (menu open, served by a box, first frame routed,
// …), not a state machine in disguise.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub screen: Screen,
    pub source: Source,
    /// The page came from a box, so it can watch one.
    pub served_by_box: bool,
    /// The Editor's layout, kept per browser.
    pub chrome: Chrome,
    /// The ⋯ menu is open.
    pub menu: bool,
    /// The sign-in panel under the header is open.
    pub login: bool,
    /// The box's Editor, built the first time it is shown.
    pub boxv: Option<BoxView>,
    /// The simulation's, likewise.
    pub sim: Option<PlaygroundView>,
    /// The box's proposals, read from the start for the ⋯ badge.
    pub review: Option<ReviewView>,
    /// Which level the Monitor shows.
    pub monitor: Monitor,
    /// The first frame chose the screen (a phone opens the Monitor).
    routed: bool,
    /// When the proposal list was last asked for (page time, s).
    listed: Option<f64>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        // A page the box serves watches that box; docs/playground/ (and
        // the old `?playground` and `?demo` links) run the simulation, as
        // does the native window.
        #[cfg(target_arch = "wasm32")]
        let mut app = if crate::web::wants_sim() {
            Self::sim()
        } else {
            Self::boxed()
        };
        #[cfg(not(target_arch = "wasm32"))]
        let mut app = Self::sim();
        if let Some(saved) = cc.storage.and_then(|s| s.get_string(CHROME_KEY)) {
            app.chrome = Chrome::restored(&saved);
        }
        #[cfg(target_arch = "wasm32")]
        if crate::web::wants_monitor() {
            app.open_monitor();
        }
        app
    }

    /// The simulation, on a page no box serves.
    #[must_use]
    pub fn sim() -> Self {
        // Writing ST is what a visitor came for: the editor starts open.
        let mut chrome = Chrome::default();
        chrome.st = true;
        Self {
            screen: Screen::Editor,
            source: Source::Sim,
            served_by_box: false,
            chrome,
            menu: false,
            login: false,
            boxv: None,
            sim: None,
            review: None,
            monitor: Monitor::default(),
            routed: false,
            listed: None,
        }
    }

    /// Opens on the Monitor, whatever the window's width.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub fn open_monitor(&mut self) {
        self.screen = Screen::Monitor;
        self.routed = true;
    }

    /// The box that served the page.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    #[must_use]
    pub fn boxed() -> Self {
        Self {
            source: Source::Box,
            served_by_box: true,
            chrome: Chrome::default(),
            ..Self::sim()
        }
    }

    /// Proposals waiting on the box (`None`: not watching a box).
    #[must_use]
    pub fn waiting(&self) -> Option<usize> {
        if self.source != Source::Box {
            return None;
        }
        Some(self.review.as_ref().map_or(0, |r| r.state.waiting()))
    }

    /// The open project's Save, Deploy, Upload and Download, wherever it
    /// lives (`None`: nothing to edit here).
    pub fn workspace(&mut self) -> Option<&mut dyn Workspace> {
        match self.source {
            Source::Box => self
                .boxv
                .as_mut()
                .filter(|b| b.has_projects())
                .map(|b| b as &mut dyn Workspace),
            Source::Sim => self.sim.as_mut().map(|s| s as &mut dyn Workspace),
        }
    }

    /// Who the page is signed in as on the box.
    #[must_use]
    pub fn session(&self) -> Option<crate::review::Session> {
        self.review.as_ref().and_then(|r| r.state.session.clone())
    }

    /// Whether the Monitor's status tiles carry the scan period and
    /// count: this browser chose it, and an engineer is signed in.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn scan_detail(&self) -> bool {
        self.chrome.scan && is_engineer(self.session().as_ref()).is_ok()
    }

    /// Files the browser's picker read since the last frame, uploaded to
    /// whatever is open.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn uploaded(&mut self, files: Vec<(String, Vec<u8>)>) {
        for (name, bytes) in files {
            if let Some(w) = self.workspace() {
                w.upload(&name, bytes);
            }
        }
    }

    /// What the header names: the program or module open.
    #[must_use]
    pub fn title(&self) -> String {
        match self.source {
            Source::Box => self
                .boxv
                .as_ref()
                .map_or_else(|| "This box".into(), BoxView::title),
            Source::Sim => self
                .sim
                .as_ref()
                .map_or_else(|| "Simulation".into(), PlaygroundView::title),
        }
    }

    /// One frame of the whole window. Split from [`eframe::App::ui`] so tests
    /// can drive it without a window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if self.source == Source::Sim && self.screen == Screen::Proposals {
            self.screen = Screen::Editor;
        }
        // A phone opens on the Monitor; the header switches either way.
        if !self.routed {
            self.routed = true;
            if ctx.content_rect().width() < NARROW {
                self.screen = Screen::Monitor;
            }
        }
        if let Some(b) = &mut self.boxv {
            b.state.set_monitor(self.screen == Screen::Monitor);
        }
        // Proposals are read from the start, so the badge knows.
        if self.source == Source::Box {
            let r = self.review.get_or_insert_with(ReviewView::new);
            if self.listed.is_none_or(|t| now - t >= PROPOSALS_EVERY) {
                if self.listed.is_some() {
                    r.state.relist();
                }
                self.listed = Some(now);
            }
            if self.screen != Screen::Proposals {
                r.tick(&ctx, now);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(PROPOSALS_EVERY));
        }
        #[cfg(target_arch = "wasm32")]
        self.uploaded(crate::web::take_picked());
        let session = self.session();
        let engineer = is_engineer(session.as_ref()).is_ok();
        if let Some(b) = &mut self.boxv {
            b.session = session;
        }
        let header = self.header(ui);
        self.chrome.hits.clear();
        match (self.screen, self.source) {
            (Screen::Proposals, _) => {
                if let Some(r) = &mut self.review {
                    r.show(ui);
                }
            }
            (Screen::Editor, Source::Box) => {
                let session = self.review.as_ref().and_then(|r| r.state.session.clone());
                let b = self.boxv.get_or_insert_with(BoxView::new);
                b.session = session;
                b.show(ui, &mut self.chrome);
                if std::mem::take(&mut b.wants_sim) {
                    self.source = Source::Sim;
                }
                // A project call found the session gone: so is the header's.
                if std::mem::take(&mut b.projects.ended)
                    && let Some(r) = &mut self.review
                {
                    r.state.session_ended();
                }
            }
            (Screen::Editor, Source::Sim) => {
                self.sim
                    .get_or_insert_with(PlaygroundView::new)
                    .show(ui, &mut self.chrome);
            }
            (Screen::Monitor, Source::Box) => {
                let b = self.boxv.get_or_insert_with(BoxView::new);
                b.state.set_monitor(true);
                b.tick(&ctx);
                self.monitor.show(ui, &b.state, &mut self.chrome, engineer);
            }
            (Screen::Monitor, Source::Sim) => {
                let s = self.sim.get_or_insert_with(PlaygroundView::new);
                s.tick(&ctx);
                self.monitor.show(ui, &*s, &mut self.chrome, engineer);
            }
        }
        if let Some(Wish::Diagram { program, prefix }) = self.monitor.wish.take() {
            self.open_diagram(&program, &prefix);
        }
        self.chrome.hits.extend(header);
    }

    /// The Editor's diagram of `program` (empty: the one open), at the
    /// instance `prefix` (empty: the program).
    pub fn open_diagram(&mut self, program: &str, prefix: &str) {
        self.screen = Screen::Editor;
        match self.source {
            Source::Box => {
                let b = self.boxv.get_or_insert_with(BoxView::new);
                b.state.set_monitor(false);
                if !program.is_empty() {
                    b.state.select(program);
                }
                b.open_at = Some(prefix.to_string());
            }
            Source::Sim => {
                if let Some(v) = self.sim.as_mut().and_then(|s| s.viewer.as_mut())
                    && let Some(k) = v.scopes.iter().position(|s| s.prefix == prefix)
                {
                    v.open(k);
                }
            }
        }
    }

    /// The header, one row as tall as two: the mark and the File / Edit /
    /// View menus on the left, the open view's actions after them, and on
    /// the right what is open (its name, Save, Deploy), the screens, the
    /// ⋯ menu and Sign in. Returns what it drew where.
    fn header(&mut self, ui: &mut egui::Ui) -> Vec<(String, Rect)> {
        let mut hits = Vec::new();
        let waiting = self.waiting();
        let title = self.title();
        let engineer = is_engineer(self.session().as_ref()).is_ok();
        let panel = egui::Panel::top("header").show(ui, |ui| {
            let size = Vec2::new(ui.available_width(), HEADER_H);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let mark = ui.label(RichText::new("HELMSMAN").strong().color(YELLOW));
                hits.push(("mark".to_string(), mark.rect));
                self.menus(ui, engineer, waiting, &mut hits);
                ui.separator();
                self.actions(ui, &mut hits);
                // The right-hand group, laid out from the window's edge
                // leftwards (the order below is right to left).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = match waiting {
                        Some(n) if n > 0 => format!("⋯ {n}"),
                        _ => "⋯".to_string(),
                    };
                    let text = RichText::new(label).strong();
                    let b = if waiting.is_some_and(|n| n > 0) {
                        egui::Button::new(text.color(BLACK)).fill(YELLOW)
                    } else {
                        egui::Button::selectable(self.menu, text)
                    };
                    let m = ui
                        .add(b.min_size(Vec2::new(TAP, 0.0)))
                        .on_hover_text("more: proposals, where the program runs");
                    hits.push(("menu".to_string(), m.rect));
                    if m.clicked() {
                        self.menu = !self.menu;
                    }
                    // ⋯ keeps the corner whether or not Sign in is shown.
                    self.login_button(ui, &mut hits);
                    let mon = ui.add(
                        egui::Button::selectable(
                            self.screen == Screen::Monitor,
                            RichText::new("Monitor").strong(),
                        )
                        .min_size(Vec2::new(TAP * 1.5, 0.0)),
                    );
                    hits.push(("monitor".to_string(), mon.rect));
                    if mon.clicked() {
                        self.screen = Screen::Monitor;
                        self.menu = false;
                    }
                    let e = ui.add(
                        egui::Button::selectable(
                            self.screen == Screen::Editor,
                            RichText::new("Editor").strong(),
                        )
                        .min_size(Vec2::new(TAP * 1.5, 0.0)),
                    );
                    hits.push(("editor".to_string(), e.rect));
                    if e.clicked() {
                        self.screen = Screen::Editor;
                        self.menu = false;
                    }
                    ui.separator();
                    self.project_controls(ui, &mut hits);
                    if self.source == Source::Sim {
                        ui.label(RichText::new("simulation").color(STEEL));
                    }
                    let t = ui.label(RichText::new(&title).strong().monospace().color(WHITE));
                    hits.push(("title".to_string(), t.rect));
                    m.rect
                })
                .inner
            })
            .inner
        });
        hits.push(("header".to_string(), panel.response.rect));
        let menu_at = panel.inner;
        if self.menu {
            self.menu_ui(ui.ctx(), menu_at, waiting, &mut hits);
        }
        if self.login {
            self.login_ui(ui.ctx(), menu_at, &mut hits);
        }
        hits
    }

    /// File, Edit and View: what exists today, nothing greyed for later.
    fn menus(
        &mut self,
        ui: &mut egui::Ui,
        engineer: bool,
        waiting: Option<usize>,
        hits: &mut Vec<(String, Rect)>,
    ) {
        let _ = waiting;
        let ctx = ui.ctx().clone();
        let mut chrome = std::mem::take(&mut self.chrome);
        let before = chrome.hits.len();
        // Plain menu buttons in the row: a MenuBar would take the whole width.
        ui.scope(|ui| {
            let f = ui.menu_button("File", |ui| {
                let (dirty, write) = self
                    .workspace()
                    .map_or((false, Err(String::new())), |w| (w.dirty(), w.writable()));
                let can = write.is_ok();
                ui.add_enabled_ui(can && dirty, |ui| {
                    if menu_item(ui, &mut chrome, "file save", "Save", false)
                        && let Some(w) = self.workspace()
                    {
                        w.save();
                    }
                });
                ui.add_enabled_ui(can, |ui| {
                    if menu_item(ui, &mut chrome, "file deploy", "Deploy", false)
                        && let Some(w) = self.workspace()
                    {
                        w.deploy();
                    }
                });
                if let Err(why) = &write
                    && !why.is_empty()
                {
                    ui.label(RichText::new(why).small().color(STEEL));
                }
                ui.separator();
                ui.add_enabled_ui(can, |ui| {
                    if menu_item(ui, &mut chrome, "file upload", "Upload…", false) {
                        #[cfg(target_arch = "wasm32")]
                        crate::web::pick_file(UPLOAD_ACCEPT, &ctx);
                    }
                });
                ui.add_enabled_ui(self.workspace().is_some(), |ui| {
                    if menu_item(ui, &mut chrome, "file download", "Download .zip", false)
                        && let Some(w) = self.workspace()
                    {
                        w.download();
                    }
                });
            });
            hits.push(("bar file".to_string(), f.response.rect));
            let e = ui.menu_button("Edit", |ui| {
                if menu_item(ui, &mut chrome, "edit undo", "Undo", false)
                    && let Some(w) = self.workspace()
                {
                    w.undo(&ctx, false);
                }
                if menu_item(ui, &mut chrome, "edit redo", "Redo", false)
                    && let Some(w) = self.workspace()
                {
                    w.undo(&ctx, true);
                }
            });
            hits.push(("bar edit".to_string(), e.response.rect));
            let v = ui.menu_button("View", |ui| {
                if menu_item(
                    ui,
                    &mut chrome,
                    "view editor",
                    "Editor",
                    self.screen == Screen::Editor,
                ) {
                    self.screen = Screen::Editor;
                }
                if menu_item(
                    ui,
                    &mut chrome,
                    "view monitor",
                    "Monitor",
                    self.screen == Screen::Monitor,
                ) {
                    self.screen = Screen::Monitor;
                }
                ui.separator();
                let tabs: &[crate::editor::Tab] = match self.source {
                    Source::Box => self.boxv.as_ref().map_or(&[], |b| b.tabs()),
                    Source::Sim => self.sim.as_ref().map_or(&[], |s| s.tabs()),
                };
                crate::editor::view_menu(ui, &mut chrome, tabs);
                ui.separator();
                if menu_item(ui, &mut chrome, "view fit", "Zoom to fit", false)
                    && let Some(w) = self.workspace()
                {
                    w.zoom_to_fit();
                }
                if let Some(w) = self.workspace() {
                    w.layout_menu(ui, &mut chrome);
                }
                if engineer {
                    ui.separator();
                    let on = chrome.scan;
                    if menu_item(ui, &mut chrome, "view scan", "Scan detail", on) {
                        chrome.scan = !on;
                    }
                }
            });
            hits.push(("bar view".to_string(), v.response.rect));
        });
        hits.extend(chrome.hits.drain(before..));
        self.chrome = chrome;
    }

    /// The open view's actions in the header: the simulation's run
    /// controls, scan period and status; a box's status and notices.
    fn actions(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        if self.screen != Screen::Editor {
            return;
        }
        let mut chrome = std::mem::take(&mut self.chrome);
        let before = chrome.hits.len();
        ui.horizontal(|ui| match self.source {
            Source::Box => {
                if let Some(b) = &mut self.boxv {
                    Body::toolbar(b, ui, &mut chrome);
                }
            }
            Source::Sim => {
                if let Some(s) = &mut self.sim {
                    Body::toolbar(s, ui, &mut chrome);
                }
            }
        });
        hits.extend(chrome.hits.drain(before..));
        self.chrome = chrome;
    }

    /// The open project's name with its dirty marker, Save and Deploy;
    /// disabled, with why on hover, when this page may not write. Not on
    /// a phone's width.
    fn project_controls(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        // A phone is the Monitor's: no room, and nothing to edit there.
        if ui.ctx().content_rect().width() < NARROW {
            return;
        }
        let Some(w) = self.workspace() else { return };
        let dirty = w.dirty();
        let write = w.writable();
        let busy = w.busy();
        let button = |ui: &mut egui::Ui, text: &str, hint: &str, on: bool| {
            let b = egui::Button::new(RichText::new(text).strong()).min_size(Vec2::new(TAP * 1.5, 0.0));
            match &write {
                Ok(()) => ui.add_enabled(on && !busy, b).on_hover_text(hint),
                Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
            }
        };
        // Laid out right to left: Deploy, Save, then the dirty mark.
        let deploy = button(
            ui,
            "Deploy",
            "save, compile and run this project (through the box's approval policy)",
            true,
        );
        hits.push(("deploy".to_string(), deploy.rect));
        let save = button(
            ui,
            "Save",
            "write the edited files (a draft: what runs does not change)",
            dirty,
        );
        hits.push(("save".to_string(), save.rect));
        ui.separator();
        if dirty {
            let d = ui
                .label(RichText::new("●").strong().color(YELLOW))
                .on_hover_text("edited, not saved");
            hits.push(("dirty".to_string(), d.rect));
        }
        let (save, deploy) = (save.clicked(), deploy.clicked());
        if let Some(w) = self.workspace() {
            if save {
                w.save();
            }
            if deploy {
                w.deploy();
            }
        }
    }

    /// Sign in, or who is signed in: on a box only.
    fn login_button(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        if self.source != Source::Box {
            return;
        }
        let text = self
            .review
            .as_ref()
            .and_then(|r| r.state.signed_in())
            .map_or_else(|| "Sign in".to_string(), |who| format!("● {who}"));
        let b = ui
            .add(egui::Button::selectable(self.login, RichText::new(text).strong()))
            .on_hover_text("sign in to edit, deploy, force and approve; or sign out");
        hits.push(("sign in".to_string(), b.rect));
        if b.clicked() {
            self.login = !self.login;
            self.menu = false;
        }
    }

    /// The sign-in panel: the box's own (a user and password, or a role
    /// and its PIN), or who is signed in with Sign out.
    fn login_ui(&mut self, ctx: &egui::Context, at: Rect, hits: &mut Vec<(String, Rect)>) {
        let Some(r) = &mut self.review else { return };
        let was = r.state.session.is_some();
        r.rects.clear();
        egui::Area::new(egui::Id::unique("header-login"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(at.right_bottom() + Vec2::new(0.0, 4.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(GRAPHITE).show(ui, |ui| {
                    ui.set_max_width(300.0);
                    r.login_ui(ui);
                });
            });
        hits.extend(r.rects.iter().map(|(k, rect)| (format!("login {k}"), *rect)));
        // Signed in or out: the panel has done its job.
        let now = r.state.session.is_some();
        if was != now {
            self.login = false;
        }
    }

    fn menu_ui(
        &mut self,
        ctx: &egui::Context,
        at: Rect,
        waiting: Option<usize>,
        hits: &mut Vec<(String, Rect)>,
    ) {
        let mut close = false;
        let area = egui::Area::new(egui::Id::unique("header-menu"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(at.right_bottom() + Vec2::new(0.0, 4.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(220.0);
                    let mut item = |ui: &mut egui::Ui, key: &str, text: String, on: bool| {
                        let r = ui.add(
                            egui::Button::selectable(on, text)
                                .min_size(Vec2::new(ui.available_width(), TAP * 0.8)),
                        );
                        hits.push((key.to_string(), r.rect));
                        r.clicked()
                    };
                    if item(ui, "menu editor", "Editor".into(), self.screen == Screen::Editor) {
                        self.screen = Screen::Editor;
                        close = true;
                    }
                    ui.separator();
                    if let Some(n) = waiting {
                        let text = if n > 0 {
                            format!("Proposals ({n} waiting)")
                        } else {
                            "Proposals".into()
                        };
                        if item(ui, "menu proposals", text, self.screen == Screen::Proposals) {
                            self.screen = Screen::Proposals;
                            close = true;
                        }
                    }
                    if self.served_by_box {
                        ui.separator();
                        let (key, text, to) = match self.source {
                            Source::Box => ("menu sim", "Simulate in this browser", Source::Sim),
                            Source::Sim => ("menu box", "Back to this box", Source::Box),
                        };
                        if item(ui, key, text.into(), false) {
                            self.source = to;
                            self.screen = Screen::Editor;
                            close = true;
                        }
                    }
                    // The box's HTTPS certificate, to compare with the
                    // browser's and the box's startup line (`box-https`).
                    let policy = self.review.as_ref().and_then(|r| r.state.policy.as_ref());
                    if let Some(fp) = policy.and_then(|p| p["transport"]["fingerprint"].as_str()) {
                        ui.separator();
                        let (a, b) = fp.split_at(if fp.is_char_boundary(48) { 48 } else { fp.len() });
                        let l = ui
                            .label(
                                RichText::new(format!("HTTPS certificate SHA-256\n{a}\n{b}"))
                                    .small()
                                    .monospace()
                                    .color(STEEL),
                            )
                            .on_hover_text(
                                "the browser shows this page's certificate fingerprint too: they must match",
                            );
                        hits.push(("tls fingerprint".to_string(), l.rect));
                    }
                });
            });
        // A click outside closes it, except on ⋯ itself (which toggles it).
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let on_button = pointer.is_some_and(|p| at.contains(p));
        if close || (area.response.clicked_elsewhere() && !on_button) {
            self.menu = false;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(CHROME_KEY, self.chrome.saved());
    }
}
