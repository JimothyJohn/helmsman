//! The app: the always-on header over one screen, as docs/UI.md lays it
//! out. The screen is the Editor or the Monitor (or Proposals, from the ⋯
//! menu); what it watches is the [`Source`], a setting rather than a mode:
//! the box that served the page, or the simulation running in it. A phone
//! (a narrow window) opens on the Monitor, as do `?monitor` and the old
//! `?andon`.

use eframe::egui::{self, CornerRadius, Rect, RichText, Vec2};

use crate::box_view::{BoxView, is_engineer};
use crate::editor::{Body, CHROME_KEY, Chrome, menu_item, menu_item_keyed};
use crate::glyphs;
use crate::hotkeys::{self, Action};
use crate::monitor::{Monitor, NARROW, Wish};
use crate::playground_view::PlaygroundView;
use crate::projects::Workspace;
use crate::review_view::ReviewView;
use crate::st_view::TAP;
use crate::st_view::brand::{BLACK, GRAPHITE, STEEL, WHITE, YELLOW};

/// The header's height, points: one row, as tall as the two it replaced.
pub const HEADER_H: f32 = 48.0;

/// The product site, and its help page (Help menu, the ? glyph).
pub const SITE_URL: &str = "https://helmsman.advin.io/";
pub const HELP_URL: &str = "https://helmsman.advin.io/help/";

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
    /// Help › About is open.
    pub about: bool,
    /// The page fell back to the simulation by itself (no box answered);
    /// once, so ⋯ › Back to this box is not undone.
    fell_back: bool,
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
        chrome.show_code();
        Self {
            screen: Screen::Editor,
            source: Source::Sim,
            served_by_box: false,
            chrome,
            login: false,
            boxv: None,
            sim: None,
            review: None,
            monitor: Monitor::default(),
            routed: false,
            about: false,
            fell_back: false,
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
        // No box ever answered this page: it is the simulation then, by
        // itself (Nick, 2026-11-01). A box that answered and went away
        // stays the box: its last diagram is worth more than a demo.
        if self.source == Source::Box
            && !self.fell_back
            && let Some(b) = &self.boxv
            && matches!(b.state.link(), crate::remote::Link::Unreachable { .. })
            && !b.state.ever_connected()
        {
            self.fell_back = true;
            self.source = Source::Sim;
            if self.screen == Screen::Proposals {
                self.screen = Screen::Editor;
            }
            let s = self.sim.get_or_insert_with(PlaygroundView::new);
            s.notice = Some(
                "No box answered at this address: simulating in this browser (⋯ › Back to this box).".into(),
            );
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
        self.glyphs(ui);
        self.chrome.hits.extend(header);
        // Last, so a focused text editor has had its own Ctrl+Z first.
        for a in hotkeys::pressed(&ctx) {
            self.act(a, &ctx);
        }
    }

    /// Does what a shortcut (or its menu item) asks; nothing where it
    /// does not apply (Run in a box's page, Sign in in the simulation).
    pub fn act(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::Save => {
                if let Some(w) = self.workspace()
                    && w.dirty()
                    && w.writable().is_ok()
                    && !w.busy()
                {
                    w.save();
                }
            }
            Action::Deploy => {
                if let Some(w) = self.workspace()
                    && w.writable().is_ok()
                    && !w.busy()
                {
                    w.deploy();
                }
            }
            Action::RunStop | Action::Stop | Action::Step => {
                if self.source != Source::Sim || self.screen != Screen::Editor {
                    return;
                }
                let Some(s) = self.sim.as_mut() else { return };
                if s.session.build.is_none() {
                    return;
                }
                match a {
                    Action::RunStop => {
                        let on = !s.session.running;
                        s.session.set_running(on);
                    }
                    Action::Stop => s.session.set_running(false),
                    _ => {
                        if !s.session.running {
                            s.session.step();
                        }
                    }
                }
            }
            Action::SignIn => {
                if self.source == Source::Box {
                    self.login = true;
                }
            }
            Action::SignOut => {
                if self.source == Source::Box
                    && let Some(r) = &mut self.review
                {
                    r.state.logout();
                    self.login = false;
                }
            }
            Action::Undo | Action::Redo => {
                if let Some(w) = self.workspace() {
                    w.undo(ctx, a == Action::Redo);
                }
            }
            Action::GoToDefinition => {
                let mut chrome = std::mem::take(&mut self.chrome);
                if let Some(w) = self.workspace() {
                    w.go_to_definition(&mut chrome);
                }
                self.chrome = chrome;
            }
            Action::CrossReferences => {
                if let Some(w) = self.workspace() {
                    w.cross_references();
                }
            }
            Action::Restart => {
                if self.source == Source::Sim
                    && let Some(s) = self.sim.as_mut()
                    && s.session.build.is_some()
                {
                    s.session.restart();
                }
            }
            Action::Upload => {
                if self.workspace().is_some_and(|w| w.writable().is_ok()) {
                    #[cfg(target_arch = "wasm32")]
                    crate::web::pick_file(UPLOAD_ACCEPT, ctx);
                }
            }
            Action::Download => {
                if let Some(w) = self.workspace() {
                    w.download();
                }
            }
            Action::Files => self.chrome.toggle_files(),
            Action::St => self.chrome.toggle_code(),
            Action::Panel => self.chrome.right = !self.chrome.right,
            Action::Trend => self.chrome.trend = !self.chrome.trend,
        }
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
                // The wordmark is the way to the site.
                let mark = ui
                    .add(egui::Button::new(RichText::new("HELMSMAN").strong().color(YELLOW)).frame(false))
                    .on_hover_text(SITE_URL);
                hits.push(("mark".to_string(), mark.rect));
                if mark.clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(SITE_URL));
                }
                self.menus(ui, engineer, waiting, &mut hits);
                ui.separator();
                self.actions(ui, &mut hits);
                // The right-hand group, laid out from the window's edge
                // leftwards (the order below is right to left).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ? keeps the very corner (Nick, 2026-11-01: in place
                    // of the ⋯, whose items are the View menu's and About's).
                    let m = glyphs::button(ui, glyphs::Icon::Help, true, false, "Help (opens the site)");
                    hits.push(("help".to_string(), m.rect));
                    if m.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(HELP_URL));
                    }
                    self.login_button(ui, &mut hits);
                    self.screen_toggle(ui, &mut hits);
                    if let Some(n) = waiting.filter(|n| *n > 0) {
                        let b =
                            egui::Button::new(RichText::new(format!("{n} waiting")).strong().color(BLACK))
                                .fill(YELLOW);
                        let r = ui.add(b).on_hover_text("proposals waiting for an approver");
                        hits.push(("proposals waiting".to_string(), r.rect));
                        if r.clicked() {
                            self.screen = Screen::Proposals;
                        }
                    }
                    ui.separator();
                    self.dirty_mark(ui, &mut hits);
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
        if self.login {
            self.login_ui(ui.ctx(), menu_at, &mut hits);
        }
        if self.about {
            self.about_ui(ui.ctx(), &mut hits);
        }
        hits
    }

    /// Editor | Monitor as one two-way toggle: the active side filled.
    fn screen_toggle(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        let half = Vec2::new(TAP * 1.6, TAP * 0.62);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(half.x * 2.0, half.y), egui::Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), GRAPHITE);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            egui::Stroke::new(1.0, STEEL.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        let sides = [
            (Screen::Editor, "Editor", "editor"),
            (Screen::Monitor, "Monitor", "monitor"),
        ];
        for (k, (screen, label, key)) in sides.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let r = Rect::from_min_size(rect.min + Vec2::new(half.x * k as f32, 0.0), half);
            let id = ui.scope_id().with(key);
            let resp = ui.interact(r, id, egui::Sense::click());
            let on = self.screen == screen;
            if on {
                ui.painter()
                    .rect_filled(r.shrink(2.0), CornerRadius::same(5), YELLOW);
            }
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(14.0),
                if on { BLACK } else { WHITE },
            );
            hits.push((key.to_string(), r));
            // A click on either side flips (Nick, 2026-11-01), so from the
            // active side too; from Proposals either side goes to the Editor.
            if resp.clicked() {
                self.screen = match self.screen {
                    Screen::Editor => Screen::Monitor,
                    Screen::Monitor | Screen::Proposals => Screen::Editor,
                };
                let _ = screen;
            }
        }
    }

    /// The glyph buttons over the diagram's top-right corner: run/stop,
    /// step and restart in the simulation, then save, upload, download
    /// and help. Drawn after the screen, so they sit on top.
    fn glyphs(&mut self, ui: &mut egui::Ui) {
        if self.screen != Screen::Editor || self.chrome.diagram == Rect::NOTHING {
            return;
        }
        let at = self.chrome.diagram.right_top() + Vec2::new(-8.0, 8.0);
        let mut hits = Vec::new();
        let mut act: Vec<Action> = Vec::new();
        let (dirty, can_write) = self
            .workspace()
            .map_or((false, false), |w| (w.dirty(), w.writable().is_ok() && !w.busy()));
        let has_ws = self.workspace().is_some();
        let sim = (self.source == Source::Sim).then(|| {
            self.sim
                .as_ref()
                .map(|s| (s.session.build.is_some(), s.session.running))
                .unwrap_or_default()
        });
        egui::Area::new(egui::Id::unique("glyphs"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(BLACK.gamma_multiply(0.75))
                    .corner_radius(CornerRadius::same(7))
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let mut g =
                                |ui: &mut egui::Ui, key: &str, icon: glyphs::Icon, enabled, on, tip, a| {
                                    let r = glyphs::button(ui, icon, enabled, on, tip);
                                    hits.push((key.to_string(), r.rect));
                                    if r.clicked() {
                                        act.push(a);
                                    }
                                };
                            if let Some((built, running)) = sim {
                                let (icon, tip) = if running {
                                    (glyphs::Icon::Stop, "Stop (Ctrl+Enter, Shift+F8)")
                                } else {
                                    (glyphs::Icon::Run, "Run (Ctrl+Enter)")
                                };
                                g(ui, "run", icon, built, false, tip, Action::RunStop);
                                g(
                                    ui,
                                    "step",
                                    glyphs::Icon::Step,
                                    built && !running,
                                    false,
                                    "Step one scan (F10)",
                                    Action::Step,
                                );
                                g(
                                    ui,
                                    "restart",
                                    glyphs::Icon::Restart,
                                    built,
                                    false,
                                    "Restart: initial values, clock to zero",
                                    Action::Restart,
                                );
                                ui.separator();
                            }
                            g(
                                ui,
                                "save",
                                glyphs::Icon::Save,
                                can_write && dirty,
                                false,
                                "Save (Ctrl+S)",
                                Action::Save,
                            );
                            g(
                                ui,
                                "upload",
                                glyphs::Icon::Upload,
                                can_write,
                                false,
                                "Upload a .zip, .st, .xml or .l5x",
                                Action::Upload,
                            );
                            g(
                                ui,
                                "download",
                                glyphs::Icon::Download,
                                has_ws,
                                false,
                                "Download the project as .zip",
                                Action::Download,
                            );
                        });
                    });
            });
        let ctx = ui.ctx().clone();
        for a in act {
            self.act(a, &ctx);
        }
        self.chrome.hits.extend(hits);
    }

    /// Help › About: the product, its version and build, the licence, the site.
    fn about_ui(&mut self, ctx: &egui::Context, hits: &mut Vec<(String, Rect)>) {
        let mut open = self.about;
        egui::Window::new("About Helmsman")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(RichText::new("HELMSMAN").strong().size(22.0).color(YELLOW));
                ui.label(RichText::new("FACTORY AUTOMATION").strong().color(STEEL));
                ui.add_space(8.0);
                ui.label(format!(
                    "Version {} (build {})",
                    env!("CARGO_PKG_VERSION"),
                    env!("HELMSMAN_GIT_SHA")
                ));
                ui.label("Early release. Licensed under the Apache License, Version 2.0.");
                // The box's HTTPS certificate, to compare with the
                // browser's and the box's startup line (`box-https`).
                let policy = self.review.as_ref().and_then(|r| r.state.policy.as_ref());
                if let Some(fp) = policy.and_then(|p| p["transport"]["fingerprint"].as_str()) {
                    ui.add_space(6.0);
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
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.link(SITE_URL).clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(SITE_URL));
                    }
                });
                ui.add_space(6.0);
                let c = ui.button("Close");
                hits.push(("about close".to_string(), c.rect));
                if c.clicked() {
                    self.about = false;
                }
            });
        if !open {
            self.about = false;
        }
    }

    /// File, Edit and View: what exists today, nothing greyed for later.
    fn menus(
        &mut self,
        ui: &mut egui::Ui,
        engineer: bool,
        waiting: Option<usize>,
        hits: &mut Vec<(String, Rect)>,
    ) {
        let ctx = ui.ctx().clone();
        let mut chrome = std::mem::take(&mut self.chrome);
        let before = chrome.hits.len();
        // Plain menu buttons in the row, in a menu bar's frameless style
        // (a MenuBar itself would take the whole width).
        ui.scope(|ui| {
            egui::containers::menu::menu_style(ui.style_mut());
            let f = ui.menu_button("File", |ui| {
                let (dirty, write) = self
                    .workspace()
                    .map_or((false, Err(String::new())), |w| (w.dirty(), w.writable()));
                let can = write.is_ok();
                let k = |a| hotkeys::label(&ctx, a);
                ui.add_enabled_ui(can && dirty, |ui| {
                    if menu_item_keyed(ui, &mut chrome, "file save", "Save", false, &k(Action::Save))
                        && let Some(w) = self.workspace()
                    {
                        w.save();
                    }
                });
                ui.add_enabled_ui(can, |ui| {
                    if menu_item_keyed(
                        ui,
                        &mut chrome,
                        "file deploy",
                        "Deploy",
                        false,
                        &k(Action::Deploy),
                    ) && let Some(w) = self.workspace()
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
                let k = |a| hotkeys::label(&ctx, a);
                if menu_item_keyed(ui, &mut chrome, "edit undo", "Undo", false, &k(Action::Undo))
                    && let Some(w) = self.workspace()
                {
                    w.undo(&ctx, false);
                }
                if menu_item_keyed(ui, &mut chrome, "edit redo", "Redo", false, &k(Action::Redo))
                    && let Some(w) = self.workspace()
                {
                    w.undo(&ctx, true);
                }
                ui.separator();
                if menu_item_keyed(
                    ui,
                    &mut chrome,
                    "edit definition",
                    "Go to definition",
                    false,
                    &k(Action::GoToDefinition),
                ) && let Some(w) = self.workspace()
                {
                    w.go_to_definition(&mut chrome);
                }
                if menu_item_keyed(
                    ui,
                    &mut chrome,
                    "edit xref",
                    "Cross references",
                    false,
                    &k(Action::CrossReferences),
                ) && let Some(w) = self.workspace()
                {
                    w.cross_references();
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
                if let Some(n) = waiting {
                    let text = if n > 0 {
                        format!("Proposals ({n} waiting)")
                    } else {
                        "Proposals".into()
                    };
                    if menu_item(
                        ui,
                        &mut chrome,
                        "view proposals",
                        text,
                        self.screen == Screen::Proposals,
                    ) {
                        self.screen = Screen::Proposals;
                    }
                }
                if self.served_by_box {
                    let (key, text, to) = match self.source {
                        Source::Box => ("view sim", "Simulate in this browser", Source::Sim),
                        Source::Sim => ("view box", "Back to this box", Source::Box),
                    };
                    if menu_item(ui, &mut chrome, key, text, false) {
                        self.source = to;
                        self.screen = Screen::Editor;
                    }
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
            let h = ui.menu_button("Help", |ui| {
                if menu_item(ui, &mut chrome, "help docs", "Help", false) {
                    ctx.open_url(egui::OpenUrl::new_tab(HELP_URL));
                }
                if menu_item(ui, &mut chrome, "help keys", "Keyboard shortcuts", false) {
                    ctx.open_url(egui::OpenUrl::new_tab(format!("{HELP_URL}#keyboard")));
                }
                ui.separator();
                if menu_item(ui, &mut chrome, "help about", "About Helmsman…", self.about) {
                    self.about = true;
                }
            });
            hits.push(("bar help".to_string(), h.response.rect));
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

    /// The edited-not-saved mark (Save and Deploy are the File menu's and
    /// the glyphs'). Not on a phone's width.
    fn dirty_mark(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        if ui.ctx().content_rect().width() < NARROW {
            return;
        }
        if self.workspace().is_some_and(|w| w.dirty()) {
            let d = ui
                .label(RichText::new("●").strong().color(YELLOW))
                .on_hover_text("edited, not saved");
            hits.push(("dirty".to_string(), d.rect));
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(CHROME_KEY, self.chrome.saved());
    }
}
