//! The live ST view: a module's Structured Text drawn as its diagram
//! (`helmsman-diagram`), read-only, with a traced scan on it every frame.
//!
//! [`Viewer`] draws a module from whatever scans it is given: the demo
//! here ([`StView`]), or a box over its API (`box_view`).
//!
//! The demo runs the module in-process the way the box does: a
//! `helmsman_plc::Runtime` scanning every 10 ms on the page's clock, a
//! `TraceTap` asked for one scan per frame. Its inputs are simulated (a
//! scripted shift, or switched by hand); the ST itself is never edited
//! here (`canvas-edit-scope`: changes go through the agent as ST).

use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
    epaint::CubicBezierShape,
};
use helmsman_diagram::layout::{HEADER, LABEL_H, in_port, out_port};
use helmsman_diagram::phrase::{Phrased, paths};
use helmsman_diagram::{
    Diagram, Gate, Live, Module, NodeId, NodeKind, RegionId, Scan, Words, annotate, explain, phrase,
};
use helmsman_iec::{ImageSizes, Value, compile};
use helmsman_plc::{Module as Slot, Runtime, Sample, Status, TraceTap, Window};

use crate::explain::{Ask, Phase, Steady};

/// Brand colours (docs/brand/index.html).
pub mod brand {
    use eframe::egui::Color32;
    pub const YELLOW: Color32 = Color32::from_rgb(0xFF, 0xC2, 0x0E);
    pub const BLACK: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
    pub const GRAPHITE: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
    pub const STEEL: Color32 = Color32::from_rgb(0x8C, 0x8C, 0x8C);
    pub const WHITE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
    /// Between black and graphite: node bodies.
    pub const PANEL: Color32 = Color32::from_rgb(0x24, 0x24, 0x24);
}

use brand::{BLACK, GRAPHITE, PANEL, STEEL, WHITE, YELLOW};

/// The demo module: a conveyor cell with a start/stop latch, a motor
/// block with a nested IF, a blinker, a part counter, a CASE and a stack
/// light.
pub const DEMO: &str = "FUNCTION_BLOCK motor
VAR_INPUT run, fault : BOOL; END_VAR
VAR_OUTPUT on : BOOL; END_VAR
IF run THEN
  IF NOT fault THEN
    on := TRUE;
  ELSE
    on := FALSE;
  END_IF;
ELSE
  on := FALSE;
END_IF;
END_FUNCTION_BLOCK

PROGRAM cell
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  jam AT %IX0.2 : BOOL;
  conveyor AT %QX0.0 : BOOL;
  green AT %QX0.1 : BOOL;
  red AT %QX0.2 : BOOL;
  running : BOOL;
  m : motor;
  blink : TON;
  flash : BOOL;
  parts : CTU;
  count : INT;
  lane : INT;
END_VAR
running := (start OR running) AND NOT stop;
m(run := running, fault := jam);
conveyor := m.on;
blink(IN := NOT blink.Q, PT := T#400ms);
IF blink.Q THEN flash := NOT flash; END_IF;
parts(CU := m.on AND flash, R := stop, PV := 1000);
count := parts.CV;
CASE count MOD 3 OF
  0: lane := 1;
  1: lane := 2;
ELSE
  lane := 3;
END_CASE;
IF jam THEN
  red := flash;
  green := FALSE;
ELSIF m.on THEN
  green := TRUE;
  red := FALSE;
ELSE
  green := FALSE;
  red := FALSE;
END_IF;
END_PROGRAM
";

const MODULE: &str = "cell";
const SCAN_MS: u64 = 10;
/// The scripted shift repeats every this many ms.
const SHIFT_MS: u64 = 12_000;
const INPUTS: [&str; 3] = ["start", "stop", "jam"];

/// Pan and zoom: `screen = origin + offset + world * zoom`.
#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub offset: Vec2,
    pub zoom: f32,
    origin: Pos2,
    fitted: bool,
}

impl Default for Cam {
    fn default() -> Self {
        Self {
            offset: Vec2::new(30.0, 30.0),
            zoom: 1.0,
            origin: Pos2::ZERO,
            fitted: false,
        }
    }
}

impl Cam {
    pub fn to_screen(self, x: f32, y: f32) -> Pos2 {
        self.origin + self.offset + Vec2::new(x, y) * self.zoom
    }

    pub fn to_world(self, s: Pos2) -> (f32, f32) {
        let v = (s - self.origin - self.offset) / self.zoom;
        (v.x, v.y)
    }

    fn rect(self, r: &helmsman_diagram::Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.x, r.y), self.to_screen(r.right(), r.bottom()))
    }

    fn zoom_at(&mut self, screen: Pos2, factor: f32) {
        let (wx, wy) = self.to_world(screen);
        self.zoom = (self.zoom * factor).clamp(0.15, 3.0);
        self.offset = screen - self.origin - Vec2::new(wx, wy) * self.zoom;
    }

    fn fit(&mut self, area: Rect, size: (f32, f32)) {
        let (w, h) = (size.0.max(1.0) + 60.0, size.1.max(1.0) + 60.0);
        self.zoom = (area.width() / w).min(area.height() / h).clamp(0.2, 1.2);
        self.origin = area.min;
        self.offset = Vec2::splat(30.0 * self.zoom);
        self.fitted = true;
    }
}

/// What the view shows: the program, or a user function block through
/// one instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub label: String,
    pub pou: usize,
    /// `""` for the program, `M.` for instance `M`.
    pub prefix: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Node(NodeId),
    Region(RegionId),
}

/// One module's diagram on screen, whatever the scans come from (the
/// in-page demo, or a box over its API): scopes, camera, selection, the
/// variable "on because" is shown for, and the panels drawing them.
pub struct Viewer {
    pub src: String,
    pub module: Module,
    pub scopes: Vec<Scope>,
    pub scope: usize,
    pub cams: Vec<Cam>,
    pub pick: Option<Pick>,
    /// The variable "on because" is shown for (program path).
    pub because: Option<String>,
    /// What the templates know beside the trace: the module's kinds, and
    /// the signal model's units when a box gave them.
    pub words: Words,
    /// How long each explained variable has shown its value.
    pub steady: Steady,
    /// The source lines the explanation is about (drawn highlighted).
    pub highlight: Vec<u32>,
}

/// The smallest thing a finger can hit, in points.
pub const TAP: f32 = 44.0;

/// What the explain panel offers beside the box's own explanation.
pub struct AskUi<'a> {
    /// Why "Ask why" cannot be used now; `None`: it can.
    pub blocked: Option<&'a str>,
    /// The ask in flight or answered.
    pub ask: Option<&'a Ask>,
    /// The API token, to type in (box only; memory only).
    pub token: Option<&'a mut String>,
}

impl AskUi<'_> {
    /// For the in-page demo: no box, no agent.
    #[must_use]
    pub const fn demo() -> Self {
        Self {
            blocked: Some("The demo runs in this page: asking the agent needs a box with a relay."),
            ask: None,
            token: None,
        }
    }
}

/// What the operator asked for in the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// "Ask why" for this program path.
    AskWhy(String),
}

impl Viewer {
    /// `module`, lowered from `src`: the program first, then each user
    /// function block instance.
    #[must_use]
    pub fn new(src: &str, module: Module) -> Self {
        let mut scopes = Vec::new();
        if let Some(p) = module
            .pous
            .iter()
            .position(|p| p.kind == helmsman_iec::ast::PouKind::Program)
        {
            scopes.push(Scope {
                label: format!("PROGRAM {}", module.pous[p].name),
                pou: p,
                prefix: String::new(),
            });
        }
        for (path, fb) in module.instances() {
            if let Some(p) = module.pous.iter().position(|q| q.name == fb) {
                scopes.push(Scope {
                    label: format!("{path} : {fb}"),
                    pou: p,
                    prefix: format!("{path}."),
                });
            }
        }
        if scopes.is_empty() {
            // No program (a library of blocks): show the first POU as is.
            scopes.push(Scope {
                label: module.pous.first().map_or_else(String::new, |p| p.name.clone()),
                pou: 0,
                prefix: String::new(),
            });
        }
        let cams = vec![Cam::default(); scopes.len()];
        Self {
            src: src.to_string(),
            module,
            scopes,
            scope: 0,
            cams,
            pick: None,
            because: None,
            words: Words::default(),
            steady: Steady::default(),
            highlight: Vec::new(),
        }
        .with_words()
    }

    fn with_words(mut self) -> Self {
        self.words = Words::of(&self.module);
        self
    }

    /// The box's own explanation of the variable "on because" is shown
    /// for, from `scan` (at `now_ms` on the module's clock): the facts, as
    /// sentences. Notes the values it saw, for "unchanged over the last".
    pub fn explanation(&mut self, scan: Option<&dyn Scan>, now_ms: Option<u64>) -> Option<Phrased> {
        let path = self.because.clone()?;
        let b = explain(&self.module, scan?, &path, 4);
        if let Some(now) = now_ms {
            let s = scan?;
            for p in paths(&b) {
                self.steady.observe(&p, s.get(&p), now);
            }
        }
        let mut w = self.words.clone();
        for p in paths(&b) {
            if let Some(ms) = self.steady.ms(&p) {
                w.steady_ms.insert(p, ms);
            }
        }
        let said = phrase(&b, &w);
        self.highlight = said.lines();
        Some(said)
    }

    /// Whether there is anything to draw.
    #[must_use]
    pub fn drawable(&self) -> bool {
        self.module.pous.get(self.scopes[self.scope].pou).is_some()
    }

    #[must_use]
    pub fn diagram(&self) -> &Diagram {
        &self.module.pous[self.scopes[self.scope].pou].diagram
    }

    #[must_use]
    pub fn live(&self, scan: Option<&dyn Scan>) -> Option<Live> {
        Some(annotate(self.diagram(), scan?, &self.scopes[self.scope].prefix))
    }

    /// The source line a statement starts on, numbered.
    #[must_use]
    pub fn source_line(&self, line: u32) -> String {
        let text = self
            .src
            .lines()
            .nth(line.saturating_sub(1) as usize)
            .unwrap_or("")
            .trim();
        format!("{line:>3} | {text}")
    }

    /// What the selection panel shows: the picked element's source line
    /// and its text.
    #[must_use]
    pub fn picked(&self) -> Option<(u32, String)> {
        let d = self.diagram();
        Some(match self.pick? {
            Pick::Node(n) => {
                let n = d.nodes.get(n.0 as usize)?;
                (n.pos.line, n.text.clone())
            }
            Pick::Region(r) => {
                let r = d.regions.get(r.0 as usize)?;
                (r.pos.line, r.label.clone())
            }
        })
    }

    /// Program paths the outputs list offers: located outputs and
    /// `VAR_OUTPUT`s of the program, in source order of first assignment.
    #[must_use]
    pub fn outputs(&self) -> Vec<String> {
        let Some(p) = self.module.program() else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for n in &p.diagram.nodes {
            if let NodeKind::Write { path } = &n.kind
                && p.vars
                    .get(path)
                    .is_some_and(|v| v.located || v.section == helmsman_iec::ast::Section::Output)
                && !out.contains(path)
            {
                out.push(path.clone());
            }
        }
        out
    }

    /// The scope picker (program, or a block through one instance).
    pub fn scopes_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("View").strong());
        let mut chosen = None;
        for (k, s) in self.scopes.iter().enumerate() {
            if ui.selectable_label(k == self.scope, &s.label).clicked() {
                chosen = Some(k);
            }
        }
        if let Some(k) = chosen
            && k != self.scope
        {
            self.scope = k;
            self.pick = None;
        }
    }

    /// The outputs with their values; a click asks why.
    pub fn outputs_ui(&mut self, ui: &mut egui::Ui, scan: Option<&dyn Scan>) {
        ui.label(RichText::new("Outputs").strong())
            .on_hover_text("click: why is it in this state?");
        for path in self.outputs() {
            let value = scan.and_then(|s| s.get(&path));
            let on = value == Some(Value::Bool(true));
            let text = RichText::new(format!("{} {path} = {}", if on { "●" } else { "○" }, show(value)))
                .monospace()
                .color(if on { YELLOW } else { WHITE });
            let row = egui::Button::selectable(self.because.as_deref() == Some(path.as_str()), text)
                .min_size(Vec2::new(ui.available_width(), TAP));
            if ui.add(row).clicked() {
                self.because = Some(path);
            }
        }
    }

    /// The explain panel: why the chosen variable is in its state (the
    /// box's own words, instantly), "Ask why" for the agent's, the facts
    /// for an engineer, and the selection's source line.
    pub fn right(
        &mut self,
        ui: &mut egui::Ui,
        scan: Option<&dyn Scan>,
        now_ms: Option<u64>,
        ask: AskUi<'_>,
    ) -> Option<Action> {
        let said = self.explanation(scan, now_ms);
        let mut action = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new("Why").strong().size(16.0));
            match (&self.because, &said) {
                (Some(_), Some(said)) => {
                    for s in &said.sentences {
                        ui.horizontal_wrapped(|ui| {
                            #[allow(clippy::cast_precision_loss)]
                            ui.add_space(12.0 * s.depth as f32);
                            let color = if s.depth == 0 {
                                WHITE
                            } else {
                                STEEL.gamma_multiply(1.4)
                            };
                            ui.label(RichText::new(&s.text).color(color));
                        });
                    }
                    for l in said.lines() {
                        ui.label(RichText::new(self.source_line(l)).monospace().color(YELLOW));
                    }
                }
                (Some(_), None) => {
                    ui.label(RichText::new("Waiting for a traced scan…").color(STEEL));
                }
                (None, _) => {
                    ui.label(
                        RichText::new("Tap an output or a block to see why it is in its state.").color(STEEL),
                    );
                }
            }
            if let Some(path) = self.because.clone() {
                ui.add_space(6.0);
                if let Some(a) = action_ui(ui, &path, ask) {
                    action = Some(a);
                }
            }
            if let (Some(path), Some(s)) = (&self.because, scan) {
                egui::CollapsingHeader::new(RichText::new("Facts").strong())
                    .id_salt("facts")
                    .default_open(false)
                    .show(ui, |ui| {
                        for line in explain(&self.module, s, path, 4).outline() {
                            ui.label(RichText::new(line).monospace().size(11.5));
                        }
                    });
            }
            ui.separator();
            ui.label(RichText::new("Selection").strong());
            match self.picked() {
                Some((line, text)) => {
                    ui.label(RichText::new(self.source_line(line)).monospace().color(YELLOW));
                    ui.label(RichText::new(text).monospace());
                    if let Some(Pick::Node(n)) = self.pick {
                        let live = self.live(scan);
                        let node = &self.diagram().nodes[n.0 as usize];
                        if let Some(live) = &live {
                            for (k, o) in node.outs.iter().enumerate() {
                                let v = live.values[n.0 as usize].get(k).copied().flatten();
                                let name = if o.is_empty() { "out" } else { o.as_str() };
                                ui.label(
                                    RichText::new(format!("{name} = {}", show(v)))
                                        .monospace()
                                        .color(STEEL),
                                );
                            }
                        }
                    }
                }
                None => {
                    ui.label(RichText::new("Tap a block or region to see its source line.").color(STEEL));
                }
            }
        });
        action
    }

    /// The variable a tap on node `n` asks about: a variable read or
    /// written, or a function block's first output.
    #[must_use]
    pub fn tapped(&self, n: NodeId) -> Option<String> {
        let prefix = &self.scopes[self.scope].prefix;
        let node = self.diagram().nodes.get(n.0 as usize)?;
        match &node.kind {
            NodeKind::Write { path } | NodeKind::Read { path } if !path.starts_with('%') => {
                Some(format!("{prefix}{path}"))
            }
            NodeKind::Fb { instance, .. } => {
                let out = node.outs.iter().find(|o| !o.is_empty())?;
                Some(format!("{prefix}{instance}.{out}"))
            }
            _ => None,
        }
    }

    /// The diagram, panned, zoomed and clickable, with `scan` on it.
    pub fn canvas(&mut self, ui: &mut egui::Ui, scan: Option<&dyn Scan>) {
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        let size = self.diagram().size;
        let cam = &mut self.cams[self.scope];
        if !cam.fitted {
            cam.fit(area, size);
        }
        cam.origin = area.min;
        let (hover, scroll, zoom) = ui
            .ctx()
            .input(|i| (i.pointer.hover_pos(), i.smooth_scroll_delta, i.zoom_delta()));
        if response.hovered() {
            if (zoom - 1.0).abs() > f32::EPSILON {
                if let Some(h) = hover {
                    cam.zoom_at(h, zoom);
                }
            } else if scroll != Vec2::ZERO {
                cam.offset += scroll;
            }
        }
        if response.dragged() {
            cam.offset += response.drag_delta();
        }
        if response.double_clicked() {
            cam.fitted = false;
        }
        let cam = *cam;
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = cam.to_world(p);
            let d = self.diagram();
            // A finger is wider than a block at low zoom: the nearest
            // block within half a tap target counts.
            let slop = TAP / 2.0 / cam.zoom;
            self.pick = node_near(d, x, y, slop)
                .map(Pick::Node)
                .or_else(|| d.region_at(x, y).map(Pick::Region));
            if let Some(Pick::Node(n)) = self.pick
                && let Some(path) = self.tapped(n)
            {
                self.because = Some(path);
            }
        }
        let live = self.live(scan);
        draw(
            &painter,
            area,
            cam,
            self.diagram(),
            live.as_ref(),
            self.pick,
            &self.highlight,
        );
    }
}

/// The demo: the module running in-page on a `helmsman_plc::Runtime`.
pub struct StView {
    pub view: Viewer,
    rt: Runtime,
    tap: TraceTap,
    pub sample: Option<Sample>,
    /// Runtime clock, ms.
    pub now_ms: u64,
    /// Page time the clock last caught up to.
    last: Option<f64>,
    pub running: bool,
    /// Inputs follow the scripted shift; off: switched by hand.
    pub auto: bool,
    pub inputs: [bool; 3],
    pub error: Option<String>,
}

impl StView {
    /// The demo module, compiled and running.
    pub fn demo() -> Self {
        Self::new(DEMO).unwrap_or_else(|e| {
            // DEMO is a constant that compiles (tested); keep the view up anyway.
            let mut v = Self::new("PROGRAM cell\nVAR x : BOOL; END_VAR\nx := NOT x;\nEND_PROGRAM")
                .unwrap_or_else(|_| unreachable!());
            v.error = Some(e);
            v
        })
    }

    /// `src` as module `cell` with one byte of `%I` and `%Q`.
    ///
    /// # Errors
    /// Compile, lowering or runtime configuration errors, as text.
    pub fn new(src: &str) -> Result<Self, String> {
        let sizes = ImageSizes { i: 1, q: 1, m: 0 };
        let program = compile(src, sizes).map_err(|d| {
            d.iter()
                .map(|d| format!("{}: {}", d.pos, d.msg))
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        let module = helmsman_diagram::lower(src, &program.trace_map()).map_err(|e| e.to_string())?;
        let rt = Runtime::new(
            1,
            1,
            vec![Slot {
                name: MODULE.into(),
                program,
                period: Duration::from_millis(SCAN_MS),
                i: Window::new(0, 1),
                q: Window::new(0, 1),
                params: Vec::new(),
            }],
        )
        .map_err(|e| format!("{e:?}"))?;
        let tap = rt.tap(MODULE).ok_or("no tap")?;
        let mut view = Viewer::new(src, module);
        view.because = Some("CONVEYOR".into());
        Ok(Self {
            view,
            rt,
            tap,
            sample: None,
            now_ms: 0,
            last: None,
            running: true,
            auto: true,
            inputs: [false; 3],
            error: None,
        })
    }

    /// The scripted shift: start at 0.2 s, a jam from 6 to 7.5 s, stop at 10 s.
    #[must_use]
    pub const fn scripted(ms: u64) -> [bool; 3] {
        let t = ms % SHIFT_MS;
        [
            t >= 200 && t < 500,
            t >= 10_000 && t < 10_300,
            t >= 6_000 && t < 7_500,
        ]
    }

    /// Runs the module up to page time `t` (seconds), tracing the last scan.
    pub fn advance(&mut self, t: f64) {
        let last = *self.last.get_or_insert(t);
        self.last = Some(t);
        if !self.running {
            return;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let due = (((t - last).max(0.0) * 1000.0) as u64 / SCAN_MS).min(200);
        // At least one scan per frame keeps a live trace on the page.
        self.scans(due.max(1));
    }

    /// Runs `n` scans; the last one is traced.
    pub fn scans(&mut self, n: u64) {
        for k in 0..n {
            if self.auto {
                self.inputs = Self::scripted(self.now_ms);
            }
            let byte = self
                .inputs
                .iter()
                .enumerate()
                .fold(0u8, |b, (i, &on)| b | (u8::from(on) << i));
            if k + 1 == n {
                self.tap.request();
            }
            self.rt.tick(Duration::from_millis(self.now_ms), &[byte]);
            self.now_ms += SCAN_MS;
        }
        if let Some(s) = self.tap.take() {
            self.sample = Some(s);
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn live(&self) -> Option<Live> {
        self.view
            .live(self.sample.as_ref().map(|s| &s.trace as &dyn Scan))
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let t = ui.ctx().input(|i| i.time);
        self.advance(t);
        egui::Panel::left("st-left")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| self.left(ui));
        let scan = self.sample.as_ref().map(|s| &s.trace as &dyn Scan);
        egui::Panel::right("st-right")
            .resizable(true)
            .default_size(340.0)
            .show(ui, |ui| {
                let _ = self.view.right(ui, scan, Some(self.now_ms), AskUi::demo());
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.view.canvas(ui, scan));
        if self.running {
            ui.ctx().request_repaint();
        }
    }

    fn left(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("Module cell").color(YELLOW));
        ui.label(RichText::new("demo, running in this page").color(STEEL));
        let status = match self.rt.status(MODULE) {
            Some(Status::Running) => "running".to_string(),
            Some(Status::Held) => "held".to_string(),
            Some(Status::Faulted(f)) => format!("faulted: {f:?}"),
            None => "?".into(),
        };
        let scans = self.rt.stats(MODULE).map_or(0, |s| s.scans);
        ui.label(
            RichText::new(format!(
                "{status} · {scans} scans · t={:.1}s",
                self.now_ms as f64 / 1000.0
            ))
            .monospace()
            .color(STEEL),
        );
        ui.horizontal(|ui| {
            let label = if self.running { "Pause" } else { "Run" };
            if ui.button(label).clicked() {
                self.running = !self.running;
            }
            if ui.add_enabled(!self.running, egui::Button::new("Step")).clicked() {
                self.scans(1);
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(255, 110, 100), e);
        }
        ui.separator();
        self.view.scopes_ui(ui);
        ui.separator();
        ui.label(RichText::new("Demo inputs").strong());
        ui.checkbox(&mut self.auto, "scripted shift");
        ui.add_enabled_ui(!self.auto, |ui| {
            for (k, name) in INPUTS.iter().enumerate() {
                ui.checkbox(&mut self.inputs[k], format!("{name}  %IX0.{k}"));
            }
        });
        ui.separator();
        let scan = self.sample.as_ref().map(|s| &s.trace as &dyn Scan);
        self.view.outputs_ui(ui, scan);
    }
}

pub fn show(v: Option<Value>) -> String {
    v.map_or_else(|| "–".into(), |v| v.to_string())
}

fn font(size: f32) -> FontId {
    FontId::monospace(size)
}

fn wire_color(v: Option<Value>) -> (Color32, f32) {
    match v {
        Some(Value::Bool(true)) => (YELLOW, 2.5),
        Some(Value::Bool(false)) => (STEEL, 1.5),
        Some(_) => (WHITE, 1.5),
        None => (GRAPHITE.gamma_multiply(1.6), 1.2),
    }
}

fn bezier(painter: &egui::Painter, a: Pos2, b: Pos2, z: f32, color: Color32, width: f32) {
    let dx = ((b.x - a.x).abs() * 0.5).max(30.0 * z);
    painter.add(CubicBezierShape::from_points_stroke(
        [a, a + Vec2::new(dx, 0.0), b - Vec2::new(dx, 0.0), b],
        false,
        Color32::TRANSPARENT,
        Stroke::new(width * z.max(0.5), color),
    ));
}

/// The node at `(x, y)`, or the nearest one within `slop` of it.
#[must_use]
pub fn node_near(d: &Diagram, x: f32, y: f32, slop: f32) -> Option<NodeId> {
    d.node_at(x, y).or_else(|| {
        d.nodes
            .iter()
            .map(|n| {
                let r = &n.rect;
                let dx = (r.x - x).max(x - r.right()).max(0.0);
                let dy = (r.y - y).max(y - r.bottom()).max(0.0);
                (n.id, dx.hypot(dy))
            })
            .filter(|(_, dist)| *dist <= slop)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    })
}

/// "Ask why" and its answer, under the box's own explanation.
fn action_ui(ui: &mut egui::Ui, path: &str, ask: AskUi<'_>) -> Option<Action> {
    let mut action = None;
    let answered = ask.ask.filter(|a| a.path.eq_ignore_ascii_case(path));
    let running = answered.is_some_and(Ask::running);
    let enabled = ask.blocked.is_none() && !running;
    let button = egui::Button::new(RichText::new("Ask why").strong().color(BLACK))
        .fill(if enabled { YELLOW } else { GRAPHITE })
        .min_size(Vec2::new(TAP * 3.0, TAP));
    let r = ui.add_enabled(enabled, button);
    if r.clicked() {
        action = Some(Action::AskWhy(path.to_string()));
    }
    if let Some(why) = ask.blocked {
        ui.label(RichText::new(why).color(STEEL));
    }
    if let Some(token) = ask.token {
        ui.horizontal(|ui| {
            ui.label(RichText::new("API token").color(STEEL));
            ui.add(
                egui::TextEdit::singleline(token)
                    .password(true)
                    .desired_width(160.0)
                    .min_size(Vec2::new(0.0, TAP * 0.75)),
            );
        });
    }
    if let Some(a) = answered {
        ui.add_space(4.0);
        if let Some(scan) = a.scan {
            ui.label(RichText::new(format!("The agent's answer (scan {scan}):")).color(STEEL));
        }
        if !a.text.is_empty() {
            ui.label(RichText::new(&a.text).color(WHITE));
        }
        for n in &a.notes {
            ui.label(RichText::new(n).small().color(STEEL));
        }
        match &a.phase {
            Phase::Waiting | Phase::Streaming => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("asking…").color(STEEL));
                });
            }
            Phase::Done => {}
            Phase::Failed(why) => {
                ui.colored_label(Color32::from_rgb(255, 110, 100), why);
            }
        }
    }
    action
}

#[allow(clippy::too_many_arguments)]
fn draw(
    painter: &egui::Painter,
    area: Rect,
    cam: Cam,
    d: &Diagram,
    live: Option<&Live>,
    pick: Option<Pick>,
    lines: &[u32],
) {
    let z = cam.zoom;
    painter.rect_filled(area, CornerRadius::ZERO, BLACK);
    let text = z > 0.35;
    // Regions, outermost first.
    for r in &d.regions {
        let gate = live.map(|l| l.gates[r.id.0 as usize]);
        let rect = cam.rect(&r.rect);
        let (stroke, fill, state) = match gate {
            Some(Gate::Open(n)) => (YELLOW, YELLOW.gamma_multiply(0.06), format!("taken ×{n}")),
            Some(Gate::Closed) => (STEEL.gamma_multiply(0.7), Color32::TRANSPARENT, "skipped".into()),
            Some(Gate::NotReached) => (GRAPHITE, Color32::TRANSPARENT, "not reached".into()),
            None => (STEEL, Color32::TRANSPARENT, String::new()),
        };
        painter.rect_filled(rect, CornerRadius::same(6), fill);
        let width = if pick == Some(Pick::Region(r.id)) {
            2.5
        } else {
            1.2
        };
        painter.rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(width * z, stroke),
            StrokeKind::Inside,
        );
        if text {
            let at = rect.min + Vec2::new(8.0, LABEL_H / 2.0) * z;
            painter.text(at, Align2::LEFT_CENTER, &r.label, font(12.0 * z), stroke);
            painter.text(
                Pos2::new(rect.max.x - 8.0 * z, at.y),
                Align2::RIGHT_CENTER,
                state,
                font(10.0 * z),
                stroke,
            );
        }
        // The gate's wire into the region's label.
        if let Some((g, port)) = r.gate {
            let n = d.node(g);
            let (x, y) = out_port(&n.rect, port);
            let v = live.and_then(|l| l.values[g.0 as usize].get(port).copied().flatten());
            let (c, w) = wire_color(v);
            bezier(
                painter,
                cam.to_screen(x, y),
                cam.to_screen(r.rect.x, r.rect.y + LABEL_H / 2.0),
                z,
                c,
                w,
            );
        }
    }
    // Wires.
    for (k, w) in d.wires.iter().enumerate() {
        let (a, b) = (d.node(w.from), d.node(w.to));
        let (ax, ay) = out_port(&a.rect, w.out);
        let (bx, by) = in_port(&b.rect, w.inp);
        let v = live.and_then(|l| l.wire(d, k));
        let (c, width) = wire_color(v);
        let (pa, pb) = (cam.to_screen(ax, ay), cam.to_screen(bx, by));
        bezier(painter, pa, pb, z, c, width);
        if text
            && let Some(v) = v
            && !matches!(v, Value::Bool(_))
        {
            painter.text(
                pa + Vec2::new(4.0, -7.0) * z,
                Align2::LEFT_CENTER,
                v.to_string(),
                font(10.0 * z),
                WHITE,
            );
        }
    }
    // Nodes.
    for n in &d.nodes {
        let r = cam.rect(&n.rect);
        if !area.intersects(r) {
            continue;
        }
        let ran = live.is_none_or(|l| l.ran[n.id.0 as usize]);
        let (head, ink) = match &n.kind {
            NodeKind::Fb { .. } | NodeKind::For { .. } => (YELLOW, BLACK),
            NodeKind::Write { .. } => (WHITE, BLACK),
            NodeKind::Read { .. } | NodeKind::Const { .. } => (GRAPHITE, WHITE),
            NodeKind::Op { .. } | NodeKind::Func { .. } | NodeKind::Access { .. } => {
                (Color32::from_rgb(0x55, 0x55, 0x55), WHITE)
            }
            NodeKind::Jump => (STEEL, BLACK),
        };
        let fade: f32 = if ran { 1.0 } else { 0.4 };
        let radius = CornerRadius::same(4);
        painter.rect_filled(r, radius, PANEL.gamma_multiply(fade.max(0.7)));
        let h = Rect::from_min_size(r.min, Vec2::new(r.width(), HEADER * z));
        painter.rect_filled(
            h,
            CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            },
            head.gamma_multiply(fade),
        );
        if lines.contains(&n.pos.line) {
            // The source line the explanation is about.
            painter.rect_stroke(
                r.expand(4.0 * z),
                CornerRadius::same(6),
                Stroke::new(2.0 * z, YELLOW.gamma_multiply(0.6)),
                StrokeKind::Outside,
            );
        }
        let outline = if pick == Some(Pick::Node(n.id)) {
            Stroke::new(2.5 * z, YELLOW)
        } else {
            Stroke::new(1.0 * z, Color32::from_black_alpha(200))
        };
        painter.rect_stroke(r, radius, outline, StrokeKind::Outside);
        if text {
            painter.text(
                h.center(),
                Align2::CENTER_CENTER,
                &n.label,
                font(11.5 * z),
                ink.gamma_multiply(fade),
            );
            for (k, name) in n.ins.iter().enumerate() {
                let (x, y) = in_port(&n.rect, k);
                let p = cam.to_screen(x, y);
                painter.circle_filled(p, 3.0 * z, STEEL);
                if !name.is_empty() {
                    painter.text(
                        p + Vec2::new(6.0 * z, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        font(10.0 * z),
                        STEEL,
                    );
                }
            }
            for (k, name) in n.outs.iter().enumerate() {
                let (x, y) = out_port(&n.rect, k);
                let p = cam.to_screen(x, y);
                let v = live.and_then(|l| l.values[n.id.0 as usize].get(k).copied().flatten());
                painter.circle_filled(p, 3.5 * z, wire_color(v).0);
                if !name.is_empty() {
                    painter.text(
                        p - Vec2::new(6.0 * z, 0.0),
                        Align2::RIGHT_CENTER,
                        name,
                        font(10.0 * z),
                        STEEL,
                    );
                }
            }
            // A variable's value in its body.
            if matches!(n.kind, NodeKind::Read { .. } | NodeKind::Write { .. })
                && let Some(v) = live.and_then(|l| l.values[n.id.0 as usize].first().copied().flatten())
            {
                let on = v == Value::Bool(true);
                painter.text(
                    Pos2::new(r.center().x, h.max.y + (r.max.y - h.max.y) / 2.0),
                    Align2::CENTER_CENTER,
                    v.to_string(),
                    font(11.0 * z),
                    if on { YELLOW } else { WHITE },
                );
            }
        }
    }
}
