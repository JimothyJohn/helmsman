//! The Editor on a real box: the page was served by the box's API
//! (`--ui`), so it talks to the same origin. [`BoxState`] decides what to
//! watch and [`BoxProjects`] what to edit; in the browser `web::Driver`
//! and `web::ProjHttp` ask (fetch, `EventSource`) and hand back what came,
//! so this whole view also runs headless in tests, fed messages instead.
//!
//! On a box with projects (`--projects`) the Files sidebar lists them and
//! the ST editor edits the open project's files, for an engineer; anyone
//! else reads. The diagram is always what runs, live, and an engineer
//! forces an input by clicking its block. On a box without projects the
//! sidebar lists its modules and the ST is read-only, as before.
//!
//! The diagram's arrangement is the running project's shared default with
//! this browser's tweaks on top (`arrange.rs`); an engineer saves it as
//! the default.

use eframe::egui::{self, Color32, Rect, RichText, Sense, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::Value;
use serde_json::json;

use crate::arrange::{self, Layers};
use crate::box_projects::{Answer, BoxProjects, Deploy, PCall, PReq};
use crate::box_trend::BoxTrend;
use crate::editor::{self, Body, Chrome, Tab};
use crate::projects::{self, Act, FilesUi, Listing, Workspace};
use crate::remote::{BoxState, Cmd, Link, Msg, SignalRow};
use crate::review::Session;
use crate::sim_projects::Storage;
use crate::st_view::brand::{BLACK, GRAPHITE, WARN};
use crate::st_view::{Pick, Viewer, brand, show};

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Largest file the editor takes (the box's `project::MAX_FILE`).
const MAX_FILE: usize = 256 * 1024;

/// What [`BoxView::watch`] compares against.
#[derive(Default)]
struct BoxSeen {
    link: Option<u8>,
    deploy: Deploy,
    fault: Option<String>,
    notice: Option<String>,
}

pub struct BoxView {
    pub state: BoxState,
    /// The Messages tab's log.
    pub log: crate::messages::Log,
    /// What the log last saw, to report changes once.
    seen: BoxSeen,
    /// The box's projects, when it runs with them.
    pub projects: BoxProjects,
    pub files_ui: FilesUi,
    /// Who the page is signed in as: the header's session, set by the app
    /// before every frame.
    pub session: Option<Session>,
    pub view: Option<Viewer>,
    /// The Trend tab: pinned signals from the box's history.
    pub trend: BoxTrend,
    /// The diagram's shared default and this browser's tweaks.
    pub layers: Layers,
    /// The lowering the view was built from.
    generation: u64,
    /// Set when the operator, with the box out of reach, asks for the
    /// simulation in this page instead.
    pub wants_sim: bool,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
    /// The scope (an instance prefix, `""` for the program) to open once
    /// the diagram is there: the Monitor's "Open diagram".
    pub open_at: Option<String>,
    #[cfg(target_arch = "wasm32")]
    driver: crate::web::Driver,
    #[cfg(target_arch = "wasm32")]
    http: crate::web::ProjHttp,
    /// Commands not carried out (no transport here: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<Cmd>,
    /// Project and force requests not carried out (tests carry them out).
    #[cfg(not(target_arch = "wasm32"))]
    pub psent: Vec<PReq>,
}

impl Default for BoxView {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether `s` may edit, save, deploy and force: an engineer or above.
pub fn is_engineer(s: Option<&Session>) -> Result<(), String> {
    match s {
        None => Err("Sign in as an engineer (top right) to edit, save, deploy or force.".into()),
        Some(s) if matches!(s.role.as_str(), "engineer" | "integrator") => Ok(()),
        Some(s) => Err(format!(
            "Signed in as {}: only an engineer edits, saves, deploys or forces.",
            s.label()
        )),
    }
}

impl BoxView {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let tweaks: Box<dyn Storage> = Box::new(crate::web::LocalArrangements);
        #[cfg(target_arch = "wasm32")]
        let pins: Box<dyn Storage> = Box::new(crate::web::LocalSlot("helmsman-canvas/trend-pins"));
        #[cfg(not(target_arch = "wasm32"))]
        let tweaks: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        #[cfg(not(target_arch = "wasm32"))]
        let pins: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_stores(tweaks, pins)
    }

    /// The Editor, the Trend's pins kept in `pins` while signed out.
    #[cfg(test)]
    #[must_use]
    pub fn with_pins(pins: Box<dyn Storage>) -> Self {
        Self::with_stores(Box::new(crate::sim_projects::MemStorage::default()), pins)
    }

    /// The Editor, the diagram's personal layouts kept in `tweaks` and
    /// the Trend's pins (signed out) in `pins`.
    #[must_use]
    pub fn with_stores(tweaks: Box<dyn Storage>, pins: Box<dyn Storage>) -> Self {
        Self {
            trend: BoxTrend::new(pins),
            log: crate::messages::Log::default(),
            seen: BoxSeen::default(),
            state: BoxState::new(),
            projects: BoxProjects::new(),
            files_ui: FilesUi::default(),
            session: None,
            view: None,
            layers: Layers::load(tweaks),
            generation: 0,
            wants_sim: false,
            last_pick: None,
            open_at: None,
            #[cfg(target_arch = "wasm32")]
            driver: crate::web::Driver::default(),
            #[cfg(target_arch = "wasm32")]
            http: crate::web::ProjHttp::default(),
            #[cfg(not(target_arch = "wasm32"))]
            sent: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            psent: Vec::new(),
        }
    }

    /// Takes in `msgs` at page time `now`, then carries out what is due.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn step(&mut self, ctx: &egui::Context, msgs: Vec<Msg>, now: f64) {
        self.step_all(ctx, msgs, Vec::new(), now);
    }

    /// [`Self::step`], with the project and force answers too.
    pub fn step_all(&mut self, ctx: &egui::Context, msgs: Vec<Msg>, answers: Vec<(PCall, Answer)>, now: f64) {
        for m in msgs {
            self.state.apply(m, now);
        }
        for (call, a) in answers {
            match call {
                PCall::TrendPins | PCall::SavePins | PCall::Trend => self.trend.answer(&call, &a, now),
                call => self.projects.answer(call, &a),
            }
        }
        if std::mem::take(&mut self.projects.deployed) {
            self.state.relist();
        }
        if let Some(f) = self.projects.forces.take() {
            for s in &mut self.state.signals {
                s.forced = f.iter().any(|f| f.signal.as_deref() == Some(s.name.as_str()));
            }
            self.state.forces = f;
        }
        let cmds = self.state.poll(now);
        self.run(ctx, cmds);
        self.trend.step(self.session.as_ref(), &self.state, now);
        let mut reqs = self.projects.poll();
        reqs.extend(self.trend.poll());
        self.send(ctx, reqs);
        self.downloads();
        let mut fresh = false;
        match &self.state.lowered {
            Some(l) if l.generation != self.generation => {
                self.generation = l.generation;
                let mut v = Viewer::new(&l.src, l.module.clone());
                // The same source again (refetched): keep what was being
                // looked at, as arranged. A new version is laid out from
                // the layers (the moves so far kept as tweaks first).
                // Opens with no path lit; the operator's pick survives a
                // new version while its variable does.
                if let Some(old) = self.view.take() {
                    self.layers.track(&old);
                    if old.src == v.src {
                        v = old;
                    } else {
                        v.because = old.because.filter(|b| v.knows(b));
                        fresh = true;
                    }
                } else {
                    fresh = true;
                }
                self.last_pick = v.pick;
                self.view = Some(v);
            }
            None => self.view = None,
            Some(_) => {}
        }
        let key = self.layout_key();
        if let Some(v) = &mut self.view {
            if let Some(text) = self.projects.layout_saved.take() {
                self.layers.adopt(v, &text);
            }
            self.layers.show(v, &key, self.projects.shared_layout(), fresh);
        }
        if let (Some(v), Some(p)) = (&mut self.view, &self.open_at) {
            if let Some(k) = v.scopes.iter().position(|s| s.prefix == *p) {
                v.open(k);
            }
            self.open_at = None;
        }
        if let Some(at) = self.state.next_retry() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((at - now).clamp(0.05, 60.0)));
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn run(&mut self, ctx: &egui::Context, cmds: Vec<Cmd>) {
        for c in cmds {
            self.driver.run(c, ctx);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn run(&mut self, _ctx: &egui::Context, cmds: Vec<Cmd>) {
        self.sent.extend(cmds);
    }

    #[cfg(target_arch = "wasm32")]
    fn send(&mut self, ctx: &egui::Context, reqs: Vec<PReq>) {
        for r in reqs {
            self.http.send(r, ctx);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send(&mut self, _ctx: &egui::Context, reqs: Vec<PReq>) {
        self.psent.extend(reqs);
    }

    #[cfg(target_arch = "wasm32")]
    fn downloads(&mut self) {
        for url in std::mem::take(&mut self.projects.downloads) {
            let name = format!(
                "{}.zip",
                self.projects.open.as_ref().map_or("project", |o| o.name.as_str())
            );
            crate::web::download_url(&url, &name);
        }
    }

    /// Natively the links stay in `projects.downloads` for tests.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    const fn downloads(&mut self) {}

    #[cfg(target_arch = "wasm32")]
    fn inbox(&mut self) -> (Vec<Msg>, Vec<(PCall, Answer)>) {
        (self.driver.drain(), self.http.drain())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn inbox(&mut self) -> (Vec<Msg>, Vec<(PCall, Answer)>) {
        (Vec::new(), Vec::new())
    }

    /// Takes in what arrived and carries out what is due, with nothing
    /// drawn (the Monitor draws from the same state).
    pub fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let (msgs, answers) = self.inbox();
        self.step_all(ctx, msgs, answers, now);
        self.watch(now);
    }

    /// Reports to the log what changed: the link, a deploy's outcome, a
    /// fault, a notice.
    fn watch(&mut self, now: f64) {
        let link = match self.state.link() {
            Link::Connecting => 0,
            Link::Connected => 1,
            Link::Reconnecting { .. } => 2,
            Link::Unreachable { .. } => 3,
        };
        if Some(link) != self.seen.link {
            match link {
                1 => self.log.info(now, "Connected to the box."),
                2 => self.log.warning(now, "Lost the box: reconnecting."),
                3 => self.log.error(now, "The box is unreachable."),
                _ => {}
            }
            self.seen.link = Some(link);
        }
        if self.projects.deploy != self.seen.deploy {
            match &self.projects.deploy {
                Deploy::Done(what) => self.log.info(now, format!("Deployed: {what}")),
                Deploy::Failed(why) => self.log.error(now, format!("Deploy failed: {why}")),
                _ => {}
            }
            self.seen.deploy = self.projects.deploy.clone();
        }
        let fault = self.state.scan.as_ref().and_then(|s| s.fault.clone());
        if fault != self.seen.fault {
            if let Some(f) = &fault {
                self.log.error(now, format!("Fault: {f}"));
            }
            self.seen.fault = fault;
        }
        let notice = self.projects.notice.as_ref().map(|(n, _)| n.clone());
        if notice != self.seen.notice {
            if let Some((n, error)) = &self.projects.notice {
                if *error {
                    self.log.error(now, n.clone());
                } else {
                    self.log.info(now, n.clone());
                }
            }
            self.seen.notice = notice;
        }
    }

    /// One frame of the Editor.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.tick(&ui.ctx().clone());
        editor::show(ui, chrome, self);
    }

    /// Whose personal layout the diagram shows: the running project's, or
    /// on a box without one the modules'.
    #[must_use]
    pub fn layout_key(&self) -> String {
        match &self.projects.running {
            Some(p) => format!("box:{p}"),
            None => "box-modules".into(),
        }
    }

    /// Whether this page may save the arrangement as the shared default.
    fn can_save_layout(&self) -> Result<(), String> {
        is_engineer(self.session.as_ref())?;
        if !self.has_projects() {
            return Err("This box runs without projects (helmsman-plc --projects): there is no project to keep a default layout in.".into());
        }
        if self.projects.running.is_none() {
            return Err("Deploy a project first: its default layout is kept with it.".into());
        }
        if self.projects.layout.is_none() {
            return Err("Reading the project's default layout…".into());
        }
        Ok(())
    }

    /// Saves the diagram as it stands as the running project's default.
    pub fn save_layout(&mut self) {
        if self.can_save_layout().is_err() {
            return;
        }
        if let Some(v) = &self.view {
            self.projects.save_layout(v.arrangement().to_text());
        }
    }

    /// The box runs with projects.
    #[must_use]
    pub fn has_projects(&self) -> bool {
        self.projects.available == Some(true)
    }

    /// What the header names: the open project, else the module open.
    #[must_use]
    pub fn title(&self) -> String {
        if let Some(o) = self.projects.open.as_ref().filter(|_| self.has_projects()) {
            return o.name.clone();
        }
        self.state.selected.clone().unwrap_or_else(|| "This box".into())
    }

    /// What the empty canvas says.
    #[must_use]
    pub fn waiting(&self) -> String {
        let s = &self.state;
        if let Some(p) = &s.problem {
            return p.clone();
        }
        match (&s.selected, &s.scan) {
            _ if !matches!(s.link(), Link::Connected) && s.modules.is_empty() => {
                "Looking for the box…".into()
            }
            (None, _) if s.modules.is_empty() && self.has_projects() => {
                "Nothing runs yet: open a project and Deploy it.".into()
            }
            (None, _) if s.modules.is_empty() => "No modules on this box.".into(),
            (None, _) => "Pick a module.".into(),
            (Some(m), None) => format!("Waiting for a traced scan of {m}…"),
            (Some(m), Some(_)) => format!("Loading {m}'s source…"),
        }
    }

    /// The connection status line.
    #[must_use]
    pub fn status(&self, now: f64) -> (String, Color32) {
        let retry = |at: f64| (at - now).max(0.0).ceil();
        match self.state.link() {
            Link::Connecting => ("● connecting…".into(), brand::STEEL),
            Link::Connected => ("● connected".into(), brand::YELLOW),
            Link::Reconnecting { at, .. } => {
                (format!("● reconnecting (retry in {}s)", retry(at)), brand::WHITE)
            }
            Link::Unreachable { at, .. } => (format!("● box unreachable (retry in {}s)", retry(at)), RED),
        }
    }

    /// The input signal a diagram read of `path` is, if the box forces it.
    fn input(&self, path: &str) -> Option<&SignalRow> {
        self.state
            .signals
            .iter()
            .find(|s| s.dir == "in" && s.name.eq_ignore_ascii_case(path))
    }

    /// Whether `name` is forced, as the box last said.
    #[must_use]
    pub fn forced(&self, name: &str) -> bool {
        self.state
            .forces
            .iter()
            .any(|f| f.signal.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(name)))
            || self
                .state
                .signals
                .iter()
                .any(|s| s.forced && s.name.eq_ignore_ascii_case(name))
    }

    /// Forces input `name` to `v` (an engineer's click on its block).
    pub fn force(&mut self, name: &str, v: Value) {
        if is_engineer(self.session.as_ref()).is_err() {
            return;
        }
        let value = match v {
            Value::Bool(b) => json!(b),
            Value::Int(n) => i64::try_from(n).map_or_else(|_| json!(n.to_string()), |n| json!(n)),
            Value::Real(r) => json!(r),
        };
        self.projects.force(name, &value);
    }

    /// Opens `file` of the open project at `line`.
    fn goto(&mut self, file: &str, line: u32, chrome: &mut Chrome) {
        self.projects.open_file(file);
        if self.projects.current().is_some() {
            chrome.st = true;
            chrome.goto(line);
        }
    }

    /// The forced inputs, each with release, and release all.
    fn banner(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        if self.state.forces.is_empty() {
            return;
        }
        let can = is_engineer(self.session.as_ref());
        let mut release = None;
        egui::Frame::new()
            .fill(WARN)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let r = ui.label(RichText::new("FORCED").strong().color(BLACK));
                    chrome.hit("forces", r.rect);
                    for f in &self.state.forces {
                        ui.label(
                            RichText::new(format!("{} = {} ({})", f.point(), show(f.value), f.by))
                                .monospace()
                                .color(BLACK),
                        );
                        let b = egui::Button::new("release");
                        let r = match &can {
                            Ok(()) => ui.add(b),
                            Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
                        };
                        chrome.hit(format!("release {}", f.id), r.rect);
                        if r.clicked() {
                            release = Some(Some(f.id));
                        }
                    }
                    let b = egui::Button::new(RichText::new("Release all").strong());
                    let r = match &can {
                        Ok(()) => ui.add(b),
                        Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
                    };
                    chrome.hit("release all", r.rect);
                    if r.clicked() {
                        release = Some(None);
                    }
                });
            });
        if let Some(id) = release {
            self.projects.release(id);
        }
    }

    /// Over each input's block: FORCED in the warning colour when forced,
    /// and for an engineer a control that forces it.
    fn input_controls(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        reads: &[(String, Rect)],
        chrome: &mut Chrome,
    ) {
        let engineer = is_engineer(self.session.as_ref()).is_ok();
        let mut wants = Vec::new();
        for (k, (path, rect)) in reads.iter().enumerate() {
            if !area.intersects(*rect) {
                continue;
            }
            let Some(sig) = self.input(path).cloned() else {
                continue;
            };
            let forced = self.forced(&sig.name);
            if engineer {
                let builder = egui::UiBuilder::new().max_rect(*rect).id_salt(("box-force", k));
                let r = ui
                    .scope_builder(builder, |ui| force_ui(ui, *rect, &sig, forced))
                    .inner;
                chrome.hit(format!("force {}", sig.name), *rect);
                if let Some(v) = r {
                    wants.push((sig.name.clone(), v));
                }
            }
            if forced {
                let tag = Rect::from_min_size(
                    egui::pos2(rect.max.x - 58.0, rect.min.y - 16.0),
                    Vec2::new(58.0, 15.0),
                );
                ui.painter().rect_filled(tag, 2.0, WARN);
                ui.painter().text(
                    tag.center(),
                    egui::Align2::CENTER_CENTER,
                    "FORCED",
                    egui::FontId::monospace(11.0),
                    BLACK,
                );
                chrome.hit(format!("forced {}", sig.name), tag);
            }
        }
        for (name, v) in wants {
            self.force(&name, v);
        }
    }
}

/// An input's force control over its block: a BOOL flips, a number drags.
/// The value to force it to, if one was asked for.
fn force_ui(ui: &mut egui::Ui, rect: Rect, s: &SignalRow, forced: bool) -> Option<Value> {
    let id = egui::Id::unique(("box-force-number", &s.name));
    let label = s.name.to_ascii_lowercase();
    let size = (rect.height() * 0.35).clamp(7.0, 16.0);
    let tip = "force this input on the box (an engineer only; ends on logout or deploy)";
    match s.value {
        Some(Value::Bool(on)) => {
            let fill = if forced { WARN } else { GRAPHITE };
            let ink = if forced { BLACK } else { brand::WHITE };
            let b = egui::Button::new(
                RichText::new(format!("{label} {}", if on { "TRUE" } else { "FALSE" }))
                    .strong()
                    .size(size)
                    .color(ink),
            )
            .fill(fill)
            .sense(Sense::click());
            ui.add_sized(rect.size(), b)
                .on_hover_text(tip)
                .clicked()
                .then_some(Value::Bool(!on))
        }
        Some(Value::Int(n)) => {
            let v = i64::try_from(n).unwrap_or(0);
            let c = crate::commit::number(ui, id, &v.to_string(), "", rect.size(), tip);
            crate::commit::int(&c, i64::MIN, i64::MAX)
                .filter(|x| *x != v)
                .map(|x| Value::Int(i128::from(x)))
        }
        Some(Value::Real(r)) => {
            let c = crate::commit::number(ui, id, &r.to_string(), "", rect.size(), tip);
            crate::commit::real(&c)
                .filter(|x| x.to_bits() != r.to_bits())
                .map(Value::Real)
        }
        _ => None,
    }
}

impl Workspace for BoxView {
    fn dirty(&self) -> bool {
        self.projects.dirty()
    }

    fn layout_menu(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let can = self.can_save_layout();
        if arrange::menu_items(ui, chrome, &mut self.layers, self.view.as_mut(), can) {
            self.save_layout();
        }
    }

    fn zoom_to_fit(&mut self) {
        if let Some(v) = &mut self.view {
            v.refit();
        }
    }

    fn go_to_definition(&mut self, chrome: &mut Chrome) -> bool {
        let Some(v) = &self.view else { return false };
        let Some(line) = v.definition_line() else {
            return false;
        };
        // As a block click: the file and line of the project build when the
        // project open is the one running; the build's line without projects.
        match projects::file_line(&v.src, line) {
            Some((file, l))
                if self.has_projects()
                    && self.projects.open.as_ref().map(|o| &o.name) == self.projects.running.as_ref() =>
            {
                self.goto(&file, l, chrome);
            }
            None if !self.has_projects() => {
                chrome.st = true;
                chrome.goto(line);
            }
            _ => return false,
        }
        true
    }

    fn cross_references(&mut self) -> bool {
        self.view.as_mut().is_some_and(Viewer::cross_references)
    }

    fn undo(&mut self, ctx: &egui::Context, redo: bool) -> bool {
        if self.writable().is_err() {
            return false;
        }
        let Some((name, file)) = self
            .projects
            .open
            .as_ref()
            .and_then(|o| o.current.clone().map(|f| (o.name.clone(), f)))
        else {
            return false;
        };
        let id = egui::Id::unique(("box-st", &name, &file));
        let Some(buf) = self.projects.current_mut() else {
            return false;
        };
        editor::undo(ctx, id, &mut buf.text, redo)
    }

    fn writable(&self) -> Result<(), String> {
        is_engineer(self.session.as_ref())?;
        match self.projects.available {
            Some(true) if self.projects.open.is_some() => Ok(()),
            Some(true) => Err("Open a project first.".into()),
            _ => Err("This box runs without projects (helmsman-plc --projects): nothing to edit.".into()),
        }
    }

    fn save(&mut self) {
        if self.writable().is_ok() {
            self.projects.save();
        }
    }

    fn deploy(&mut self) {
        if self.writable().is_ok() {
            self.projects.deploy();
        }
    }

    fn busy(&self) -> bool {
        self.projects.deploy.busy()
            || self
                .projects
                .open
                .as_ref()
                .is_some_and(|o| o.bufs.values().any(|b| b.saving.is_some()))
    }

    fn upload(&mut self, file: &str, bytes: Vec<u8>) {
        if is_engineer(self.session.as_ref()).is_ok() && self.has_projects() {
            self.projects.upload(file, bytes, false);
        }
    }

    fn download(&mut self) {
        self.projects.download();
    }
}

impl Body for BoxView {
    fn toolbar(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let now = ui.ctx().input(|i| i.time);
        let (text, color) = self.status(now);
        ui.label(RichText::new(text).monospace().color(color));
        if matches!(self.state.link(), Link::Unreachable { .. }) {
            let r = ui
                .button("Simulate in this browser")
                .on_hover_text("run the program in this page instead, no box needed");
            chrome.hit("simulate", r.rect);
            if r.clicked() {
                self.wants_sim = true;
            }
        }
        if let Some(s) = &self.state.scan {
            ui.separator();
            ui.label(
                RichText::new(format!("t={:.1}s", s.now_ms as f64 / 1000.0))
                    .monospace()
                    .color(brand::STEEL),
            )
            .on_hover_text(format!("scan {}", s.scan));
        }
    }

    fn has_notices(&self) -> bool {
        self.state.scan.as_ref().is_some_and(|s| s.fault.is_some())
            || self.projects.deploy != Deploy::Idle
            || self.projects.notice.is_some()
    }

    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        if let Some(f) = self.state.scan.as_ref().and_then(|s| s.fault.as_ref()) {
            ui.colored_label(RED, format!("fault: {f}"));
        }
        match &self.projects.deploy {
            Deploy::Idle => {}
            Deploy::Saving => {
                ui.label(RichText::new("saving…").color(brand::STEEL));
            }
            Deploy::Checking => {
                ui.label(RichText::new("checking…").color(brand::STEEL));
            }
            Deploy::Replaying => {
                ui.label(RichText::new("replaying in the sim…").color(brand::STEEL));
            }
            Deploy::Deploying => {
                ui.label(RichText::new("deploying…").color(brand::STEEL));
            }
            Deploy::Done(what) => {
                let r = ui.label(RichText::new(format!("✔ {what}")).color(brand::YELLOW));
                chrome.hit("deployed", r.rect);
            }
            Deploy::Failed(why) => {
                let r = ui.label(RichText::new(format!("✖ {why}")).color(RED));
                chrome.hit("deploy failed", r.rect);
            }
        }
        if let Some((text, error)) = self.projects.notice.clone() {
            ui.separator();
            let r = ui.label(RichText::new(text).color(if error { RED } else { brand::STEEL }));
            chrome.hit("notice", r.rect);
            if self.projects.pending.is_some() && error {
                let r = ui.button("Replace");
                chrome.hit("replace", r.rect);
                if r.clicked() {
                    self.projects.replace();
                }
            }
            if let Some(f) = self.projects.conflict.clone() {
                let r = ui.button(format!("Reload {f}"));
                chrome.hit("reload", r.rect);
                if r.clicked() {
                    self.projects.reload(&f);
                    self.projects.notice = None;
                }
            }
        }
    }

    /// The box's projects and the open one's files; under them the
    /// modules that run, each a program, the open one with every block
    /// instance it calls, nested; a click opens any of them.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        if self.has_projects() {
            let write = self.writable().or_else(|e| {
                // Opening is a read; only the writes need an engineer.
                if self.projects.open.is_none() && is_engineer(self.session.as_ref()).is_ok() {
                    Ok(())
                } else {
                    Err(e)
                }
            });
            let open = self.projects.open.clone();
            let files: Vec<(String, bool)> = open
                .as_ref()
                .map(|o| o.bufs.iter().map(|(n, b)| (n.clone(), b.dirty())).collect())
                .unwrap_or_default();
            let listing = Listing {
                projects: &self.projects.projects,
                open: open.as_ref().map(|o| o.name.as_str()),
                files,
                current: open.as_ref().and_then(|o| o.current.as_deref()),
                write,
                dirty: self.projects.dirty(),
                starters: true,
            };
            let act = self.files_ui.show(ui, &listing, chrome);
            match act {
                Some(Act::OpenProject(p)) => self.projects.open_project(&p),
                Some(Act::OpenFile(f)) => self.projects.open_file(&f),
                Some(Act::NewProject(p)) => self.projects.create(&p, None),
                Some(Act::CopyProject { from, to }) => self.projects.create(&to, Some(&from)),
                Some(Act::FromStarter { from, to }) => self.projects.create_from(&to, &from),
                Some(Act::RenameProject { from, to }) => self.projects.rename(&from, &to),
                Some(Act::DeleteProject(p)) => self.projects.delete(&p),
                Some(Act::NewFile(f)) => self.projects.new_file(&f),
                Some(Act::RenameFile { from, to }) => self.projects.rename_file(&from, &to),
                Some(Act::DeleteFile(f)) => self.projects.delete_file(&f),
                // The Agent's answer lands in the simulation's projects.
                Some(Act::UseAnswer { .. }) | None => {}
            }
            ui.separator();
            ui.label(RichText::new("Running").strong().color(brand::STEEL));
        }
        let mut chosen = None;
        let modules = self.state.modules.clone();
        if modules.is_empty() {
            ui.label(RichText::new(self.waiting()).color(brand::STEEL));
        }
        for m in &modules {
            let selected = self.state.selected.as_deref() == Some(m.name.as_str());
            let arrow = if selected { "▾" } else { "▸" };
            let text = RichText::new(format!("{arrow} {}  {} · {} scans", m.name, m.state, m.scans))
                .monospace()
                .color(if m.state == "faulted" { RED } else { brand::WHITE });
            let r = ui.selectable_label(selected, text);
            chrome.hit(format!("module {}", m.name), r.rect);
            let r = match &m.fault {
                Some(f) => r.on_hover_text(f),
                None => r,
            };
            if r.clicked() {
                chosen = Some(m.name.clone());
            }
            if selected && let Some(v) = &mut self.view {
                ui.indent(("box-tree", &m.name), |ui| v.tree_ui(ui));
            }
        }
        if let Some(name) = chosen {
            self.state.select(&name);
        }
    }

    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        if self.has_projects() {
            let write = self.writable();
            let Some(o) = self.projects.open.clone() else {
                ui.label(RichText::new("Open a project in Files.").color(brand::STEEL));
                return;
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Structured Text").strong());
                match &write {
                    Ok(()) => {}
                    Err(why) => {
                        let r = ui.label(RichText::new(format!("read-only: {why}")).color(brand::STEEL));
                        chrome.hit("read-only", r.rect);
                    }
                }
            });
            let Some(file) = o.current.clone() else {
                ui.label(RichText::new("No files in this project yet.").color(brand::STEEL));
                return;
            };
            let diags: Vec<(u32, u32)> = self
                .projects
                .diags
                .iter()
                .filter(|d| d.file == file)
                .map(|d| (d.line, d.col))
                .collect();
            let list_h = if self.projects.diags.is_empty() {
                0.0
            } else {
                110.0
            };
            let size = Vec2::new(ui.available_width(), (ui.available_height() - list_h).max(120.0));
            let id = egui::Id::unique(("box-st", &o.name, &file));
            let Some(buf) = self.projects.current_mut() else {
                return;
            };
            if !buf.loaded {
                ui.spinner();
                return;
            }
            ui.allocate_ui(size, |ui| {
                if write.is_ok() {
                    editor::st_editor(ui, id, &mut buf.text, &diags, MAX_FILE, chrome);
                } else {
                    let mut text: &str = &buf.text;
                    editor::st_editor(ui, id, &mut text, &diags, usize::MAX, chrome);
                }
            });
            if !self.projects.diags.is_empty() {
                ui.separator();
                let diags = self.projects.diags.clone();
                let mut go = None;
                egui::ScrollArea::vertical()
                    .id_salt("box-diags")
                    .max_height(list_h)
                    .show(ui, |ui| go = projects::diags_ui(ui, &diags, chrome));
                if let Some(d) = go {
                    self.goto(&d.file, d.line, chrome);
                }
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Structured Text").strong());
            ui.label(RichText::new("read-only: changes reach the box as proposals").color(brand::STEEL));
        });
        match &self.view {
            Some(v) => {
                let mut src: &str = &v.src;
                editor::st_editor(ui, egui::Id::unique("box-st"), &mut src, &[], usize::MAX, chrome);
            }
            None => {
                ui.label(RichText::new(self.waiting()).color(brand::STEEL));
            }
        }
    }

    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.trend.ui(ui, chrome, &self.state, self.session.as_ref());
    }

    fn messages(&mut self) -> &mut crate::messages::Log {
        &mut self.log
    }

    fn tabs(&self) -> &'static [Tab] {
        // Claude comes to the box later (docs/UI.md).
        &[Tab::Io]
    }

    fn tab(&mut self, ui: &mut egui::Ui, _tab: Tab) {
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        egui::ScrollArea::vertical().id_salt("box-io").show(ui, |ui| {
            if let Some(v) = &mut self.view {
                v.outputs_ui(ui, scan);
                ui.separator();
            }
            ui.label(RichText::new("Signals").strong());
            // Two columns (Nick, 2026-11-01: denser).
            egui::Grid::new("box-io-signals").num_columns(2).show(ui, |ui| {
                for pair in self.state.signals.chunks(2) {
                    for s in pair {
                        let unit = s.unit.as_deref().map_or(String::new(), |u| format!(" {u}"));
                        let forced = if s.forced { "  FORCED" } else { "" };
                        let text = format!("{} {} = {}{unit}{forced}", s.dir, s.name, show(s.value));
                        let color = match s.quality.as_str() {
                            _ if s.forced => WARN,
                            "missing" => RED,
                            _ if s.value == Some(helmsman_iec::Value::Bool(true)) => brand::YELLOW,
                            _ => brand::WHITE,
                        };
                        let l = ui.label(RichText::new(text).monospace().color(color));
                        if let Some(e) = &s.point_error {
                            l.on_hover_text(e);
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.banner(ui, chrome);
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        let mut jump = None;
        let mut reads = Vec::new();
        let mut area = Rect::NOTHING;
        match &mut self.view {
            Some(v) if v.drawable() => {
                let outer = vec![
                    "This box".to_string(),
                    self.state.selected.clone().unwrap_or_default(),
                ];
                if v.crumbs_ui(ui, &outer).is_some() {
                    v.open(0);
                }
                area = ui.available_rect_before_wrap();
                v.canvas(ui, scan);
                if v.pick != self.last_pick {
                    self.last_pick = v.pick;
                    if let Some((line, _)) = v.picked() {
                        jump = Some((projects::file_line(&v.src, line), line));
                    }
                }
                reads = v.reads_on_screen();
            }
            _ => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(self.waiting()).color(brand::STEEL)));
            }
        }
        if let Some((at, line)) = jump {
            match at {
                // A project build: the file and line, when the project
                // open is the one running.
                Some((file, l))
                    if self.has_projects()
                        && self.projects.open.as_ref().map(|o| &o.name) == self.projects.running.as_ref() =>
                {
                    self.goto(&file, l, chrome);
                }
                None if !self.has_projects() => chrome.goto(line),
                _ => {}
            }
        }
        if !reads.is_empty() {
            self.input_controls(ui, area, &reads, chrome);
        }
    }
}
