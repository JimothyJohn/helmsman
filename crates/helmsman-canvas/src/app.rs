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
use crate::monitor::{Monitor, NARROW, Transport, Wish};
use crate::playground_view::PlaygroundView;
use crate::projects::Workspace;
use crate::review_view::ReviewView;
use crate::st_view::TAP;
use crate::st_view::brand::{BLACK, GRAPHITE, YELLOW};
use crate::stamp;
use crate::theme::{self, theme};
use crate::tips::Tip;

/// The header's height, points: one row, as tall as the two it replaced.
pub const HEADER_H: f32 = 48.0;

/// The product site, and its help page (Help menu, the ? glyph).
pub const SITE_URL: &str = "https://helmsman.advin.io/";
pub const HELP_URL: &str = "https://helmsman.advin.io/help/";

/// Seconds between re-reads of the proposal list for the ⋯ badge.
const PROPOSALS_EVERY: f64 = 30.0;

/// What Upload's file picker offers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const UPLOAD_ACCEPT: &str = ".zip,.st,.xml,.l5x,.eds";
/// What the Devices tab's "Import device…" picker offers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const DEVICE_ACCEPT: &str = ".eds";

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

/// Whether a page with this query string starts the guided tour:
/// `?tour` (the help pages link `/playground/?tour`).
#[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
#[must_use]
pub fn wants_tour(query: &str) -> bool {
    has(query, "tour")
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
    /// Who was signed in last frame, so the sign-in panel closes the
    /// frame a sign-in or sign-out lands (the answer comes frames after
    /// the panel's button; Nick, 2026-11-07).
    login_seen: Option<String>,
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
    /// The guided tour, while it runs (`?tour`, Help › Take the tour).
    pub tour: Option<crate::tour::Tour>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        // Dark by default; the light mode this browser chose, from the
        // first frame.
        theme::apply(&cc.egui_ctx, app.chrome.light);
        #[cfg(target_arch = "wasm32")]
        if crate::web::wants_monitor() {
            app.open_monitor();
        }
        #[cfg(target_arch = "wasm32")]
        if app.source == Source::Sim && crate::web::wants_tour() {
            app.tour = Some(crate::tour::Tour::new());
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
            login_seen: None,
            boxv: None,
            sim: None,
            review: None,
            monitor: Monitor::default(),
            routed: false,
            about: false,
            fell_back: false,
            listed: None,
            tour: None,
        }
    }

    /// The desk (`crate::desk`) and the App: the screen and the sign-in
    /// name come back once from before a reload, and are handed to the box
    /// view to keep every frame. A password or PIN never is.
    fn desk(&mut self) {
        let Some(b) = &mut self.boxv else { return };
        if let Some(d) = b.restored.take() {
            if self.screen == Screen::Editor && d.screen.as_deref() == Some("monitor") {
                self.screen = Screen::Monitor;
            }
            if !d.user.is_empty() {
                let r = self.review.get_or_insert_with(ReviewView::new);
                if r.user_input.is_empty() {
                    r.user_input = d.user;
                }
            }
        }
        b.desk_screen = match self.screen {
            Screen::Editor => Some("editor".into()),
            Screen::Monitor => Some("monitor".into()),
            Screen::Proposals => b.desk_screen.take(),
        };
        if let Some(r) = &self.review {
            b.desk_user.clone_from(&r.user_input);
            b.decided = r
                .state
                .list
                .iter()
                .filter(|p| p.status != "draft")
                .map(|p| (p.id, p.status.clone()))
                .collect();
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
        theme::apply(&ctx, self.chrome.light);
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
                    r.state.badge_relist();
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
        self.desk();
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
        let mut header = self.header(ui);
        header.extend(self.build_banner(ui));
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
                // The Agent tab's "Review #n": the Proposals screen, on it.
                if let Some(id) = b.wants_review.take() {
                    let r = self.review.get_or_insert_with(ReviewView::new);
                    r.state.open(id);
                    self.screen = Screen::Proposals;
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
                // Card monitor-box-run-stop: an engineer's Stop, Run and
                // Restart; anyone else's Monitor stays watch-only.
                let targets = self.monitor.targets(&b.state.modules);
                let t = engineer.then(|| Transport {
                    built: !targets.is_empty(),
                    running: b
                        .state
                        .modules
                        .iter()
                        .any(|m| targets.contains(&m.name) && m.state != "stopped"),
                    on_box: true,
                });
                self.monitor.show(ui, &b.state, &mut self.chrome, engineer, t);
            }
            (Screen::Monitor, Source::Sim) => {
                let s = self.sim.get_or_insert_with(PlaygroundView::new);
                s.tick(&ctx);
                let t = Transport {
                    built: s.session.build.is_some(),
                    running: s.session.running,
                    on_box: false,
                };
                self.monitor.show(ui, &*s, &mut self.chrome, engineer, Some(t));
            }
        }
        match self.monitor.wish.take() {
            Some(Wish::Diagram { program, prefix }) => self.open_diagram(&program, &prefix),
            Some(Wish::Act(a)) if self.source == Source::Box => {
                let targets = self
                    .boxv
                    .as_ref()
                    .map(|b| self.monitor.targets(&b.state.modules))
                    .unwrap_or_default();
                self.box_control(a, targets);
            }
            Some(Wish::Act(a)) => self.act(a, &ctx),
            None => {}
        }
        self.glyphs(ui);
        self.chrome.hits.extend(header);
        // Last, so a focused text editor has had its own Ctrl+Z first.
        for a in hotkeys::pressed(&ctx) {
            self.act(a, &ctx);
        }
        self.tour_ui(&ctx);
        theme::pointer_cursor(&ctx);
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
                // On the Editor and the Monitor alike (Nick, 2026-11-01).
                if self.source != Source::Sim || self.screen == Screen::Proposals {
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
            Action::Force => {
                if let Some(w) = self.workspace() {
                    w.force_picked(ctx);
                }
            }
            Action::Find => {
                self.chrome.show_code();
                crate::find::Find::open(&mut self.chrome.find);
            }
            Action::FindNext | Action::FindPrevious => {
                self.chrome.show_code();
                crate::find::Find::step(&mut self.chrome.find, if a == Action::FindNext { 1 } else { -1 });
            }
            Action::StepInto | Action::StepOut => {
                if let Some(w) = self.workspace() {
                    if a == Action::StepInto {
                        w.step_into();
                    } else {
                        w.step_out();
                    }
                }
            }
            Action::ReleaseAll => {
                if let Some(w) = self.workspace() {
                    w.release_all();
                }
            }
            Action::Breakpoint => {
                if self.source == Source::Sim
                    && let Some(w) = self.workspace()
                {
                    w.toggle_breakpoint();
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
            Action::Devices => self.chrome.toggle_devices(),
            Action::St => self.chrome.toggle_code(),
            Action::Panel => self.chrome.right = !self.chrome.right,
            Action::Trend => self.chrome.trend = !self.chrome.trend,
        }
    }

    /// The Editor's diagram of `program` (empty: the one open), at the
    /// instance `prefix` (empty: the program).
    /// A box's transport, from the Monitor or the Editor (card
    /// monitor-box-run-stop): Run/Stop stops every running one of
    /// `targets`, or runs them all when none runs; Restart restarts each.
    /// The box checks the session and audits.
    fn box_control(&mut self, a: Action, targets: Vec<String>) {
        let Some(b) = self.boxv.as_mut() else {
            return;
        };
        let running: Vec<String> = b
            .state
            .modules
            .iter()
            .filter(|m| targets.contains(&m.name) && m.state != "stopped")
            .map(|m| m.name.clone())
            .collect();
        let (op, names) = match a {
            Action::RunStop if running.is_empty() => ("run", targets),
            Action::RunStop => ("stop", running),
            Action::Restart => ("restart", targets),
            _ => return,
        };
        for n in names {
            b.projects.control(&n, op);
        }
    }

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
            let t = theme(ui);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // The wordmark is the way to the site.
                let mark = ui
                    .add(egui::Button::new(RichText::new("HELMSMAN").strong().color(t.accent)).frame(false))
                    .tip(format!("Opens {SITE_URL} in a new tab."));
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
                        let r = ui
                            .add(b)
                            .tip("Proposals waiting for an approver. Click to review them.");
                        hits.push(("proposals waiting".to_string(), r.rect));
                        if r.clicked() {
                            self.screen = Screen::Proposals;
                        }
                    }
                    ui.separator();
                    self.dirty_mark(ui, &mut hits);
                    if self.source == Source::Sim {
                        ui.label(RichText::new("simulation").color(t.muted))
                            .tip("The program runs in this page. Nothing reaches a controller.");
                    }
                    let r = ui
                        .label(RichText::new(&title).strong().monospace().color(t.fg))
                        .tip("The open project.");
                    hits.push(("title".to_string(), r.rect));
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

    /// Card build-match: a strip under the header, on every screen, when
    /// the box answers as a different build from this page (a page left
    /// open over an update, or a `--ui` built from another release).
    /// Nothing to compare ([`stamp::differs`]) shows nothing.
    fn build_banner(&self, ui: &mut egui::Ui) -> Vec<(String, Rect)> {
        let mut hits = Vec::new();
        let theirs = match (&self.source, &self.boxv) {
            (Source::Box, Some(b)) => b.state.build.as_deref(),
            _ => None,
        };
        let Some(theirs) = theirs.filter(|t| stamp::differs(stamp::CANVAS, Some(t))) else {
            return hits;
        };
        let frame = egui::Frame::new()
            .fill(YELLOW)
            .inner_margin(egui::Margin::symmetric(8, 4));
        let panel = egui::Panel::top("build-match").frame(frame).show(ui, |ui| {
            // Wrapped, so a narrow window grows the strip's height and
            // never a panel's width.
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(
                        "This editor is from a different release than the controller. \
                         Reload the page to get the controller's own.",
                    )
                    .color(BLACK),
                )
                .tip(format!(
                    "This page is build {}; the controller is build {theirs}. A page left open \
                     over an update keeps the old editor until it reloads.",
                    stamp::CANVAS
                ));
                let r = ui
                    .add(egui::Button::new(RichText::new("Reload").strong()))
                    .tip("Reloads this page from the controller.");
                hits.push(("reload".to_string(), r.rect));
                if r.clicked() {
                    #[cfg(target_arch = "wasm32")]
                    crate::web::reload();
                }
            });
        });
        hits.push(("build banner".to_string(), panel.response.rect));
        hits
    }

    /// Editor | Monitor as one two-way toggle: the active side filled.
    fn screen_toggle(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        let half = Vec2::new(TAP * 1.6, TAP * 0.62);
        let t = theme(ui);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(half.x * 2.0, half.y), egui::Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.rule);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            egui::Stroke::new(1.0, t.muted.gamma_multiply(0.6)),
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
            let resp = ui.interact(r, id, egui::Sense::click()).tip(match screen {
                Screen::Editor => "Editor: the program, its live diagram and its files.",
                _ => "Monitor: the machine's state at a glance, for any screen from a phone to a wall TV.",
            });
            let on = self.screen == screen;
            if on {
                ui.painter()
                    .rect_filled(r.shrink(2.0), CornerRadius::same(5), YELLOW);
            }
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(crate::theme::BODY),
                if on { BLACK } else { t.fg },
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
    /// step and restart in the simulation (on a box, an engineer's stop or
    /// run and restart of what it runs, card monitor-box-run-stop; no key
    /// does it there, so a stray Ctrl+Enter never stops a machine), then
    /// save, upload, download and help. Drawn after the screen, so they
    /// sit on top.
    fn glyphs(&mut self, ui: &mut egui::Ui) {
        if self.screen != Screen::Editor || self.chrome.diagram == Rect::NOTHING {
            return;
        }
        let at = self.chrome.diagram.right_top() + Vec2::new(-8.0, 8.0);
        let mut hits = Vec::new();
        let mut act: Vec<Action> = Vec::new();
        let (dirty, write, busy) = self.workspace().map_or_else(
            || (false, Err("No project is open.".to_string()), false),
            |w| (w.dirty(), w.writable(), w.busy()),
        );
        let can_write = write.is_ok() && !busy;
        // A disabled glyph says why on hover.
        let blocked = |what: &str| match &write {
            Err(why) if !why.is_empty() => format!("{what}: {why}"),
            Err(_) => format!("{what}: no project is open."),
            Ok(()) if busy => format!("{what}: wait for the save or deploy under way."),
            Ok(()) => what.to_string(),
        };
        let save_tip = if can_write && !dirty {
            "Save (Ctrl+S): nothing changed since the last save.".to_string()
        } else {
            blocked("Save (Ctrl+S)")
        };
        let upload_tip = blocked("Upload a .zip, .st, .xml or .l5x file into the project");
        let has_ws = self.workspace().is_some();
        let sim = (self.source == Source::Sim).then(|| {
            self.sim
                .as_ref()
                .map(|s| (s.session.build.is_some(), s.session.running))
                .unwrap_or_default()
        });
        let engineer = is_engineer(self.session().as_ref()).is_ok();
        let on_box = self
            .boxv
            .as_ref()
            .filter(|_| self.source == Source::Box && engineer)
            .map(|b| {
                let m = &b.state.modules;
                (!m.is_empty(), m.iter().any(|m| m.state != "stopped"))
            });

        egui::Area::new(egui::Id::unique("glyphs"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme(ui).veil)
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
                                let (icon, tip) = if !built {
                                    (
                                        glyphs::Icon::Run,
                                        "Run (Ctrl+Enter): the program does not compile yet.",
                                    )
                                } else if running {
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
                                    if running {
                                        "Step one scan (F10): stop the program first."
                                    } else {
                                        "Step one scan (F10)"
                                    },
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
                            if let Some((any, running)) = on_box {
                                let (icon, tip) = if running {
                                    (
                                        glyphs::Icon::Stop,
                                        "Stop on the box: the program stops scanning and its outputs go to their safe values until you run it again (an engineer's; audited).",
                                    )
                                } else {
                                    (
                                        glyphs::Icon::Run,
                                        "Run on the box: the program scans again from where it stopped (an engineer's; audited).",
                                    )
                                };
                                g(ui, "run", icon, any, false, tip, Action::RunStop);
                                g(
                                    ui,
                                    "restart",
                                    glyphs::Icon::Restart,
                                    any,
                                    false,
                                    "Restart on the box: every variable back to its initial value, then run (an engineer's; audited).",
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
                                &save_tip,
                                Action::Save,
                            );
                            g(
                                ui,
                                "upload",
                                glyphs::Icon::Upload,
                                can_write,
                                false,
                                &upload_tip,
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
            // On a box the run and restart glyphs are the box's (the
            // simulation's are not drawn there): every module it runs.
            if on_box.is_some() && matches!(a, Action::RunStop | Action::Restart) {
                let all = self
                    .boxv
                    .as_ref()
                    .map(|b| b.state.modules.iter().map(|m| m.name.clone()).collect())
                    .unwrap_or_default();
                self.box_control(a, all);
            } else {
                self.act(a, &ctx);
            }
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
                let t = theme(ui);
                ui.label(RichText::new("HELMSMAN").strong().size(22.0).color(t.accent));
                ui.label(RichText::new("FACTORY AUTOMATION").strong().color(t.muted));
                ui.add_space(8.0);
                ui.label(format!(
                    "Version {} (build {})",
                    env!("CARGO_PKG_VERSION"),
                    stamp::CANVAS
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
                                .color(t.muted),
                        )
                        .on_hover_text(
                            "the browser shows this page's certificate fingerprint too: they must match",
                        );
                    hits.push(("tls fingerprint".to_string(), l.rect));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.link(SITE_URL).tip("Opens the site in a new tab.").clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(SITE_URL));
                    }
                });
                ui.add_space(6.0);
                let c = ui.button("Close").tip("Closes this window.");
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
                // Why the writes are off goes in each item's tooltip (it
                // was a note under Deploy).
                let or_why = |tip: &str| match &write {
                    Err(why) if !why.is_empty() => why.clone(),
                    Err(_) => "No project is open.".to_string(),
                    Ok(()) => tip.to_string(),
                };
                // New project and Copy project moved here from the Files
                // tab's buttons (Nick, 2026-11-01).
                ui.add_enabled_ui(can, |ui| {
                    if menu_item(
                        ui,
                        &mut chrome,
                        "file new",
                        "New project…",
                        false,
                        &or_why("Starts an empty project."),
                    ) && let Some(w) = self.workspace()
                    {
                        w.new_project(&mut chrome);
                    }
                    if menu_item(
                        ui,
                        &mut chrome,
                        "file copy",
                        "Copy project…",
                        false,
                        &or_why("Copies the open project under a new name."),
                    ) && let Some(w) = self.workspace()
                    {
                        w.copy_project(&mut chrome);
                    }
                });
                ui.separator();
                let save_tip = if can && !dirty {
                    "Nothing changed since the last save.".to_string()
                } else {
                    or_why("Keeps the edits in the project. Nothing runs until Deploy.")
                };
                ui.add_enabled_ui(can && dirty, |ui| {
                    if menu_item_keyed(
                        ui,
                        &mut chrome,
                        "file save",
                        "Save",
                        false,
                        &k(Action::Save),
                        &save_tip,
                    ) && let Some(w) = self.workspace()
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
                        &or_why("Saves, checks the program and its requirement tests, then runs it."),
                    ) && let Some(w) = self.workspace()
                    {
                        w.deploy();
                    }
                });
                ui.separator();
                ui.add_enabled_ui(can, |ui| {
                    if menu_item(
                        ui,
                        &mut chrome,
                        "file upload",
                        "Upload…",
                        false,
                        &or_why("Adds a .zip, .st, .xml or .l5x file to the project."),
                    ) {
                        #[cfg(target_arch = "wasm32")]
                        crate::web::pick_file(UPLOAD_ACCEPT, &ctx);
                    }
                });
                let has_ws = self.workspace().is_some();
                ui.add_enabled_ui(has_ws, |ui| {
                    if menu_item(
                        ui,
                        &mut chrome,
                        "file download",
                        "Download .zip",
                        false,
                        if has_ws {
                            "Downloads the open project as a .zip file."
                        } else {
                            "No project is open."
                        },
                    ) && let Some(w) = self.workspace()
                    {
                        w.download();
                    }
                });
            });
            hits.push(("bar file".to_string(), f.response.rect));
            let e = ui.menu_button("Edit", |ui| {
                let k = |a| hotkeys::label(&ctx, a);
                if menu_item_keyed(
                    ui,
                    &mut chrome,
                    "edit undo",
                    "Undo",
                    false,
                    &k(Action::Undo),
                    "Takes back the last edit to the open file.",
                ) && let Some(w) = self.workspace()
                {
                    w.undo(&ctx, false);
                }
                if menu_item_keyed(
                    ui,
                    &mut chrome,
                    "edit redo",
                    "Redo",
                    false,
                    &k(Action::Redo),
                    "Puts back the edit Undo took back.",
                ) && let Some(w) = self.workspace()
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
                    "Opens the code where the picked block's variable is declared.",
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
                    "Lights every block that reads or writes the picked block's variable.",
                ) && let Some(w) = self.workspace()
                {
                    w.cross_references();
                }
                if menu_item_keyed(
                    ui,
                    &mut chrome,
                    "edit force",
                    "Force input",
                    false,
                    &k(Action::Force),
                    "Forces the input the picked block reads: a switch flips, a number's field opens. \
                     On a box only an engineer forces, and every force is recorded.",
                ) && let Some(w) = self.workspace()
                {
                    w.force_picked(&ctx);
                }
                if self.source == Source::Box
                    && menu_item_keyed(
                        ui,
                        &mut chrome,
                        "edit release",
                        "Release all forces",
                        false,
                        &k(Action::ReleaseAll),
                        "Releases every force on the box: each input reads its module again.",
                    )
                    && let Some(w) = self.workspace()
                {
                    w.release_all();
                }
                if self.source == Source::Sim
                    && menu_item_keyed(
                        ui,
                        &mut chrome,
                        "edit breakpoint",
                        "Breakpoint",
                        false,
                        &k(Action::Breakpoint),
                        "Sets or clears a breakpoint on the picked block: the simulation's scan stops just before it.",
                    )
                    && let Some(w) = self.workspace()
                {
                    w.toggle_breakpoint();
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
                    "The program, its live diagram and its files.",
                ) {
                    self.screen = Screen::Editor;
                }
                if menu_item(
                    ui,
                    &mut chrome,
                    "view monitor",
                    "Monitor",
                    self.screen == Screen::Monitor,
                    "The machine's state at a glance, for any screen from a phone to a wall TV.",
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
                        "Changes waiting for review and approval before they reach the box.",
                    ) {
                        self.screen = Screen::Proposals;
                    }
                }
                if self.served_by_box {
                    let (key, text, to, tip) = match self.source {
                        Source::Box => (
                            "view sim",
                            "Simulate in this browser",
                            Source::Sim,
                            "Runs the program in this page. The box keeps running its own.",
                        ),
                        Source::Sim => (
                            "view box",
                            "Back to this box",
                            Source::Box,
                            "Leaves the simulation and shows the box again.",
                        ),
                    };
                    if menu_item(ui, &mut chrome, key, text, false, tip) {
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
                if menu_item(
                    ui,
                    &mut chrome,
                    "view fit",
                    "Zoom to fit",
                    false,
                    "Fits the whole diagram in view.",
                ) && let Some(w) = self.workspace()
                {
                    w.zoom_to_fit();
                }
                if let Some(w) = self.workspace() {
                    w.layout_menu(ui, &mut chrome);
                }
                ui.separator();
                if engineer {
                    let on = chrome.scan;
                    if menu_item(
                        ui,
                        &mut chrome,
                        "view scan",
                        "Scan detail",
                        on,
                        "Shows each program's scan period and counts on the Monitor.",
                    ) {
                        chrome.scan = !on;
                    }
                }
                // Dark by default; the choice is kept per browser with the
                // layout, and `show` applies it to egui each frame.
                let light = chrome.light;
                if menu_item(
                    ui,
                    &mut chrome,
                    "view light",
                    "Light mode",
                    light,
                    "Light colours for a bright room. Kept in this browser.",
                ) {
                    chrome.light = !light;
                }
            });
            hits.push(("bar view".to_string(), v.response.rect));
            let h = ui.menu_button("Help", |ui| {
                if menu_item(
                    ui,
                    &mut chrome,
                    "help docs",
                    "Help",
                    false,
                    "Opens the help pages in a new tab.",
                ) {
                    ctx.open_url(egui::OpenUrl::new_tab(HELP_URL));
                }
                if menu_item(
                    ui,
                    &mut chrome,
                    "help keys",
                    "Keyboard shortcuts",
                    false,
                    "Opens the list of keys in a new tab.",
                ) {
                    ctx.open_url(egui::OpenUrl::new_tab(format!("{HELP_URL}#keyboard")));
                }
                if self.source == Source::Sim
                    && menu_item(
                        ui,
                        &mut chrome,
                        "help tour",
                        "Take the tour",
                        self.tour.is_some(),
                        "A guided tour of the playground: each step points at a control and moves on when you use it.",
                    )
                {
                    self.tour = Some(crate::tour::Tour::new());
                    self.screen = Screen::Editor;
                }
                ui.separator();
                if menu_item(
                    ui,
                    &mut chrome,
                    "help about",
                    "About Helmsman…",
                    self.about,
                    "The version, the build and the licence.",
                ) {
                    self.about = true;
                }
            });
            hits.push(("bar help".to_string(), h.response.rect));
        });
        hits.extend(chrome.hits.drain(before..));
        self.chrome = chrome;
    }

    /// The guided tour over the window, while it runs: reads the
    /// simulation's state, moves the tour on, and draws its card.
    fn tour_ui(&mut self, ctx: &egui::Context) {
        let Some(tour) = &mut self.tour else { return };
        let seen = match (self.screen, self.source, &self.sim) {
            (Screen::Editor, Source::Sim, Some(s)) => s.tour_seen(&self.chrome),
            _ => crate::tour::Seen::default(),
        };
        tour.observe(&seen);
        let deploy = hotkeys::label(ctx, Action::Deploy);
        let effect = crate::tour::ui(ctx, tour, &seen, &mut self.chrome, &deploy);
        if tour.ended {
            self.tour = None;
        }
        match effect {
            Some(crate::tour::Effect::CloseNaming) => {
                if let Some(s) = &mut self.sim {
                    s.files_ui.naming = None;
                }
            }
            Some(crate::tour::Effect::Help) => ctx.open_url(egui::OpenUrl::new_tab(HELP_URL)),
            None => {}
        }
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
                .tip("Edited, not saved. Save keeps it (Ctrl+S).");
            hits.push(("dirty".to_string(), d.rect));
        }
    }

    /// Sign in, or who is signed in: on a box only. Signed in, the role's
    /// glyph leads the name: a hard hat for an operator, a wrench and a
    /// screwdriver for an engineer or an integrator (Nick, 2026-11-07).
    fn login_button(&mut self, ui: &mut egui::Ui, hits: &mut Vec<(String, Rect)>) {
        if self.source != Source::Box {
            return;
        }
        let session = self.review.as_ref().and_then(|r| r.state.session.clone());
        let who = session.as_ref().map(crate::review::Session::label);
        if who != self.login_seen {
            self.login = false;
            self.login_seen.clone_from(&who);
        }
        let icon = session.as_ref().map(|s| match s.role.as_str() {
            "operator" => (glyphs::Icon::HardHat, YELLOW, "an operator"),
            _ => (glyphs::Icon::Tools, theme(ui).fg, "an engineer"),
        });
        let text = who.clone().unwrap_or_else(|| "Sign in".to_string());
        let tip = match (&who, icon) {
            (Some(w), Some((_, _, role))) => {
                format!("Signed in as {w}, {role}. Click to sign out or change who is signed in.")
            }
            _ => "Sign in to edit, deploy, force inputs and approve proposals.".to_string(),
        };
        let mut clicked = false;
        if let Some((icon, color, _)) = icon {
            let side = ui.spacing().interact_size.y;
            let (at, r) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::click());
            glyphs::paint(ui.painter(), at.shrink(side * 0.12), icon, color);
            let key = if icon == glyphs::Icon::HardHat {
                "operator"
            } else {
                "engineer"
            };
            hits.push((format!("role {key}"), at));
            clicked |= r.tip(&tip).clicked();
        }
        let b = ui
            .add(egui::Button::selectable(self.login, RichText::new(text).strong()))
            .tip(&tip);
        hits.push(("sign in".to_string(), b.rect));
        if clicked || b.clicked() {
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
                // Lifted off the dark header on graphite; a white card on
                // the light page.
                let t = theme(ui);
                let fill = if t.dark { GRAPHITE } else { t.panel };
                egui::Frame::popup(ui.style()).fill(fill).show(ui, |ui| {
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

    /// The layout string above is all that is kept per browser. egui's
    /// own memory (panel widths among it) is not: a width a pane once
    /// grew to would come back on every load for good, long after the
    /// row that pushed it was fixed (Nick, 2026-11-02: the side panes
    /// still too wide in a browser that had seen the old builds).
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// What shows where nothing is painted (the frameless centre behind
    /// the diagram's crumbs): the page, in either theme, rather than
    /// eframe's near-black.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        theme::Theme::for_mode(visuals.dark_mode)
            .bg
            .to_normalized_gamma_f32()
    }
}
