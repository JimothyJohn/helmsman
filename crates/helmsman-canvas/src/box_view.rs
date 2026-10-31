//! The Editor on a real box: the page was served by the box's API
//! (`--ui`), so it talks to the same origin. [`BoxState`] decides what to
//! ask for; in the browser `web::Driver` asks (fetch, `EventSource`) and
//! hands back what came, so this whole view also runs headless in tests,
//! fed messages instead. The ST is read-only here: changes reach a box as
//! proposals.

use eframe::egui::{self, Color32, RichText};
use helmsman_diagram::Scan;

use crate::editor::{self, Body, Chrome, Tab};
use crate::remote::{BoxState, Cmd, Link, Msg};
use crate::st_view::{Pick, Viewer, brand, show};

const RED: Color32 = Color32::from_rgb(255, 110, 100);

pub struct BoxView {
    pub state: BoxState,
    pub view: Option<Viewer>,
    /// The lowering the view was built from.
    generation: u64,
    /// Set when the operator, with the box out of reach, asks for the
    /// simulation in this page instead.
    pub wants_sim: bool,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
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
            wants_sim: false,
            last_pick: None,
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
                // Opens with no path lit; the operator's pick survives a
                // new version while its variable does.
                if let Some(old) = self.view.take() {
                    if old.src == v.src {
                        v = old;
                    } else {
                        v.arrange(&old.arrangement());
                        v.because = old.because.filter(|b| v.knows(b));
                    }
                }
                self.last_pick = v.pick;
                self.view = Some(v);
            }
            None => self.view = None,
            Some(_) => {}
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
    fn inbox(&mut self) -> Vec<Msg> {
        self.driver.drain()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn inbox(&mut self) -> Vec<Msg> {
        Vec::new()
    }

    /// One frame of the Editor.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let msgs = self.inbox();
        self.step(&ctx, msgs, now);
        editor::show(ui, chrome, self);
    }

    /// What the header names: the module open.
    #[must_use]
    pub fn title(&self) -> String {
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
                RichText::new(format!("scan {} · t={:.1}s", s.scan, s.now_ms as f64 / 1000.0))
                    .monospace()
                    .color(brand::STEEL),
            );
            if let Some(f) = &s.fault {
                ui.colored_label(RED, format!("fault: {f}"));
            }
        }
        let moved = self.view.as_ref().is_some_and(Viewer::rearranged);
        if ui
            .add_enabled(moved, egui::Button::new("Reset layout"))
            .on_hover_text("drag blocks, or an IF/CASE/loop by its label, to rearrange the diagram")
            .clicked()
            && let Some(v) = &mut self.view
        {
            v.reset_arrangement();
        }
    }

    /// The box's modules, each a program; the open one with every block
    /// instance it calls, nested; a click opens any of them.
    fn files(&mut self, ui: &mut egui::Ui) {
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

    fn tabs(&self) -> &'static [Tab] {
        // Trend and Claude come to the box with projects (docs/UI.md, step 2).
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

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        match &mut self.view {
            Some(v) if v.drawable() => {
                let outer = vec![
                    "This box".to_string(),
                    self.state.selected.clone().unwrap_or_default(),
                ];
                if v.crumbs_ui(ui, &outer).is_some() {
                    v.open(0);
                }
                v.canvas(ui, scan);
                if v.pick != self.last_pick {
                    self.last_pick = v.pick;
                    if let Some((line, _)) = v.picked() {
                        chrome.goto(line);
                    }
                }
            }
            _ => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(self.waiting()).color(brand::STEEL)));
            }
        }
    }
}
