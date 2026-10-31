//! The live ST view of a real box: the page was served by the box's API
//! (`--ui`), so it talks to the same origin. [`BoxState`] decides what to
//! ask for; in the browser `web::Driver` asks (fetch, `EventSource`) and
//! hands back what came, so this whole view also runs headless in tests,
//! fed messages instead.

use eframe::egui::{self, Color32, RichText};
use helmsman_diagram::Scan;

use crate::explain::{self, Ask, Side, SideCmd};
use crate::remote::{BoxState, Cmd, Link, Msg};
use crate::st_view::{Action, AskUi, Viewer, brand, show};

const RED: Color32 = Color32::from_rgb(255, 110, 100);

pub struct BoxView {
    pub state: BoxState,
    pub view: Option<Viewer>,
    /// The lowering the view was built from.
    generation: u64,
    /// Set when the operator asks for the offline demo.
    pub wants_demo: bool,
    /// Set when the operator goes up to the site.
    pub wants_site: bool,
    /// Whether the box has an agent (`GET /api`); `None`: not known yet.
    pub agent: Option<bool>,
    /// `GET /api` is out.
    index_out: bool,
    /// The last "Ask why" (sent under the browser's session cookie, which
    /// this page never sees).
    pub ask: Option<Ask>,
    /// Explain-panel requests not carried out (tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub side_sent: Vec<SideCmd>,
    #[cfg(target_arch = "wasm32")]
    driver: crate::web::Driver,
    /// Commands not carried out (no transport here: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<Cmd>,
}

impl Default for BoxView {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BoxState::new(),
            view: None,
            generation: 0,
            wants_demo: false,
            wants_site: false,
            agent: None,
            index_out: false,
            ask: None,
            #[cfg(not(target_arch = "wasm32"))]
            side_sent: Vec::new(),
            #[cfg(target_arch = "wasm32")]
            driver: crate::web::Driver::default(),
            #[cfg(not(target_arch = "wasm32"))]
            sent: Vec::new(),
        }
    }

    /// Takes in `msgs` at page time `now`, then carries out what is due.
    pub fn step(&mut self, ctx: &egui::Context, msgs: Vec<Msg>, now: f64) {
        for m in msgs {
            self.state.apply(m, now);
        }
        let cmds = self.state.poll(now);
        self.run(ctx, cmds);
        match &self.state.lowered {
            Some(l) if l.generation != self.generation => {
                self.generation = l.generation;
                let mut v = Viewer::new(&l.src, l.module.clone());
                // The same source again (refetched): keep what was being
                // looked at, as arranged. A new version keeps the blocks
                // the operator moved that are still there.
                if let Some(old) = self.view.take() {
                    if old.src == v.src {
                        v = old;
                    } else {
                        v.arrange(&old.arrangement());
                    }
                }
                if v.because.is_none() {
                    v.because = v.outputs().into_iter().next();
                }
                self.view = Some(v);
            }
            None => self.view = None,
            Some(_) => {}
        }
        if let Some(v) = &mut self.view {
            for sig in &self.state.signals {
                let key = sig.name.to_ascii_uppercase();
                if let Some(u) = &sig.unit {
                    v.words.units.insert(key.clone(), u.clone());
                }
                if sig.dir == "in" {
                    v.words.field.insert(key);
                }
            }
        }
        if self.agent.is_none() && !self.index_out && matches!(self.state.link(), Link::Connected) {
            self.index_out = true;
            self.side(ctx, SideCmd::Index);
        }
        if let Some(at) = self.state.next_retry() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((at - now).clamp(0.05, 60.0)));
        }
    }

    /// Takes in what the transport saw for the explain panel.
    pub fn side_in(&mut self, msgs: Vec<Side>) {
        for m in msgs {
            match m {
                Side::Index(r) => {
                    self.index_out = false;
                    if let Ok(body) = r {
                        self.agent = explain::agent(&body).or(Some(false));
                    }
                }
                Side::Ask(r) => {
                    if let Some(a) = &mut self.ask {
                        a.apply(r);
                    }
                }
            }
        }
    }

    /// Why "Ask why" cannot be used now.
    #[must_use]
    pub fn blocked(&self) -> Option<&'static str> {
        explain::blocked(matches!(self.state.link(), Link::Connected), self.agent)
    }

    /// "Ask why" for `path` of the selected module, if it can be asked.
    pub fn ask_why(&mut self, ctx: &egui::Context, path: &str) {
        let Some(module) = self.state.selected.clone() else {
            return;
        };
        if self.blocked().is_some() || self.ask.as_ref().is_some_and(Ask::running) {
            return;
        }
        self.ask = Some(Ask::new(&module, path));
        let cmd = SideCmd::Ask {
            body: explain::body(&module, path),
        };
        self.side(ctx, cmd);
    }

    #[cfg(target_arch = "wasm32")]
    fn side(&mut self, ctx: &egui::Context, cmd: SideCmd) {
        self.driver.side(cmd, ctx);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn side(&mut self, _ctx: &egui::Context, cmd: SideCmd) {
        self.side_sent.push(cmd);
    }

    #[cfg(target_arch = "wasm32")]
    fn side_inbox(&mut self) -> Vec<Side> {
        self.driver.drain_side()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn side_inbox(&mut self) -> Vec<Side> {
        Vec::new()
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
    fn inbox(&mut self) -> Vec<Msg> {
        self.driver.drain()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn inbox(&mut self) -> Vec<Msg> {
        Vec::new()
    }

    /// One frame.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let msgs = self.inbox();
        self.step(&ctx, msgs, now);
        let side = self.side_inbox();
        self.side_in(side);
        egui::Panel::left("box-left")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.left(ui, now))
            });
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        let now_ms = self.state.scan.as_ref().map(|s| s.now_ms);
        let blocked = self.blocked();
        let mut action = None;
        match &mut self.view {
            Some(v) if v.drawable() => {
                let ask = AskUi {
                    blocked,
                    ask: self.ask.as_ref(),
                };
                egui::Panel::right("box-right")
                    .resizable(true)
                    .default_size(360.0)
                    .show(ui, |ui| action = v.right(ui, scan, now_ms, ask));
                let outer = vec![
                    "Site".to_string(),
                    "This box".to_string(),
                    self.state.selected.clone().unwrap_or_default(),
                ];
                let mut up = None;
                egui::CentralPanel::no_frame().show(ui, |ui| {
                    up = v.crumbs_ui(ui, &outer);
                    v.canvas(ui, scan);
                });
                match up {
                    Some(0) => self.wants_site = true,
                    Some(_) => v.open(0),
                    None => {}
                }
            }
            _ => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new(self.waiting()).color(brand::STEEL)));
                });
            }
        }
        if let Some(Action::AskWhy(path)) = action {
            self.ask_why(&ctx, &path);
        }
        if self.ask.as_ref().is_some_and(Ask::running) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
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

    fn left(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.heading(RichText::new("This box").color(brand::YELLOW));
        let (text, color) = self.status(now);
        ui.label(RichText::new(text).monospace().color(color));
        if matches!(self.state.link(), Link::Unreachable { .. })
            && ui
                .button("Open the demo")
                .on_hover_text("a module running in this page, no box needed")
                .clicked()
        {
            self.wants_demo = true;
        }
        ui.separator();
        // The control system as a tree, IDE style: the site above this
        // box, its modules, and the open module's program with every
        // block instance it calls, nested; a click opens any of them.
        if ui
            .selectable_label(false, RichText::new("▴ Site: every box").color(brand::STEEL))
            .on_hover_text("up a level: the boxes of this site")
            .clicked()
        {
            self.wants_site = true;
        }
        ui.label(RichText::new("▾ This box").strong());
        let mut chosen = None;
        let modules = self.state.modules.clone();
        for m in &modules {
            let selected = self.state.selected.as_deref() == Some(m.name.as_str());
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let arrow = if selected { "▾" } else { "▸" };
                let text = RichText::new(format!("{arrow} {}  {} · {} scans", m.name, m.state, m.scans))
                    .monospace()
                    .color(if m.state == "faulted" { RED } else { brand::WHITE });
                let r = ui.selectable_label(selected, text);
                let r = match &m.fault {
                    Some(f) => r.on_hover_text(f),
                    None => r,
                };
                if r.clicked() {
                    chosen = Some(m.name.clone());
                }
            });
            if selected && let Some(v) = &mut self.view {
                ui.indent(("box-tree", &m.name), |ui| v.tree_ui(ui));
            }
        }
        if let Some(name) = chosen {
            self.state.select(&name);
        }
        if let Some(s) = &self.state.scan {
            ui.label(
                RichText::new(format!("scan {} · t={:.1}s", s.scan, s.now_ms as f64 / 1000.0))
                    .monospace()
                    .color(brand::STEEL),
            );
            if let Some(f) = &s.fault {
                ui.colored_label(RED, format!("fault: {f}"));
            }
        }
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        if let Some(v) = &mut self.view {
            ui.separator();
            v.outputs_ui(ui, scan);
        }
        ui.separator();
        egui::CollapsingHeader::new(RichText::new("Signals").strong())
            .default_open(true)
            .show(ui, |ui| {
                for s in &self.state.signals {
                    let unit = s.unit.as_deref().map_or(String::new(), |u| format!(" {u}"));
                    let text = format!("{} {} = {}{unit}", s.dir, s.name, show(s.value));
                    let color = match s.quality.as_str() {
                        "missing" => RED,
                        _ if s.value == Some(helmsman_iec::Value::Bool(true)) => brand::YELLOW,
                        _ => brand::WHITE,
                    };
                    let l = ui.label(RichText::new(text).monospace().color(color));
                    if let Some(e) = &s.point_error {
                        l.on_hover_text(e);
                    }
                }
            });
    }
}
