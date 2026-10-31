//! The ST playground on screen (`?playground`, and the page at
//! `docs/playground/`): an editor that compiles as you type, the program
//! running in the page with Run / Stop / Step, a virtual I/O panel built
//! from the program's ports, a trend, the live diagram with "on
//! because", and an "Ask Claude" panel. No box and no server: the state
//! is [`crate::playground::Session`] and [`crate::claude::Chat`].

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, RichText, Sense, Stroke, TextFormat, Vec2, text::LayoutJob,
};
use helmsman_diagram::Scan;
use helmsman_iec::{Ty, Value};

use crate::claude::{Chat, Model, Phase, Proxy, Request, Segment, segments};
use crate::playground::{
    EXAMPLES, MAX_PERIOD, MAX_SOURCE, MIN_PERIOD, Point, Session, TREND_MS, TREND_SIGNALS, Widget,
};
use crate::st_view::brand::{BLACK, GRAPHITE, STEEL, WHITE, YELLOW};
use crate::st_view::{AskUi, TAP, Viewer};

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Quiet time after the last keystroke before the editor compiles, s.
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

/// The right panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Io,
    Why,
    Claude,
}

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
    viewer_of: String,
    /// When the editor last changed and is not compiled yet.
    pending: Option<f64>,
    pub tab: Tab,
    pub chat: Chat,
    pub link: Link,
    proxy_asked: bool,
    /// A one-line note under the top bar (loaded an example, …).
    pub notice: Option<String>,
    /// Why the last "Ask" could not be sent.
    pub ask_error: Option<String>,
}

impl PlaygroundView {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::web::LocalKeyStore);
        #[cfg(not(target_arch = "wasm32"))]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::claude::NoStore);
        let mut v = Self {
            session: Session::new(EXAMPLES[0].src),
            viewer: None,
            viewer_of: String::new(),
            pending: None,
            tab: Tab::Io,
            chat: Chat::new(store),
            link: Link::default(),
            proxy_asked: false,
            notice: None,
            ask_error: None,
        };
        v.session.set_running(true);
        v.sync_viewer();
        v
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
        self.viewer = b.module.clone().map(|m| {
            let mut v = Viewer::new(&b.src, m);
            let outs = v.outputs();
            v.because = old.filter(|o| outs.contains(o)).or_else(|| outs.first().cloned());
            v
        });
    }

    /// Loads `src` into the editor, compiled and stopped.
    pub fn load(&mut self, src: &str, what: &str) {
        let ok = self.session.load(src);
        self.pending = None;
        self.sync_viewer();
        self.notice = Some(if ok {
            format!("Loaded {what}. Press Run to start it.")
        } else {
            format!("Loaded {what}; it does not compile yet (see the errors under the editor).")
        });
    }

    /// Compiles the editor if it has been quiet long enough (or `now`).
    fn compile_due(&mut self, t: f64, now: bool) {
        if let Some(at) = self.pending
            && (now || t - at >= DEBOUNCE)
        {
            self.pending = None;
            let src = self.session.src.clone();
            self.session.set_source(&src);
            self.sync_viewer();
        }
    }

    /// Picks the variable "on because" explains and shows it.
    pub fn explain(&mut self, name: &str) {
        if let Some(v) = &mut self.viewer {
            v.because = Some(name.to_string());
        }
        self.tab = Tab::Why;
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

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.proxy_asked {
            self.proxy_asked = true;
            self.link.check_proxy(&ctx);
        }
        for m in self.link.drain() {
            self.chat.receive(m);
        }
        let t = ctx.input(|i| i.time);
        self.compile_due(t, false);
        self.session.advance(t);
        if self.session.running || self.pending.is_some() || self.chat.busy() {
            ctx.request_repaint();
        }
        egui::Panel::top("pg-top").show(ui, |ui| self.top(ui));
        egui::Panel::left("pg-editor")
            .resizable(true)
            .default_size(460.0)
            .show(ui, |ui| self.editor(ui, t));
        egui::Panel::right("pg-side")
            .resizable(true)
            .default_size(360.0)
            .show(ui, |ui| self.side(ui));
        egui::Panel::bottom("pg-trend")
            .resizable(true)
            .default_size(150.0)
            .show(ui, |ui| self.trend(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.diagram(ui));
    }

    fn top(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("HELMSMAN").strong().color(YELLOW));
            ui.label(RichText::new("ST playground").color(WHITE));
            ui.separator();
            let mut pick = None;
            egui::ComboBox::from_id_salt("pg-examples")
                .selected_text("Examples")
                .show_ui(ui, |ui| {
                    for (k, e) in EXAMPLES.iter().enumerate() {
                        if ui.selectable_label(false, e.name).clicked() {
                            pick = Some(k);
                        }
                    }
                });
            if let Some(k) = pick {
                self.load(EXAMPLES[k].src, &format!("the {} example", EXAMPLES[k].name));
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
        });
        if let Some(f) = &self.session.fault {
            ui.colored_label(RED, format!("Fault: {f}. Run or Step restarts the program."));
        }
        if let Some(n) = &self.notice {
            ui.label(RichText::new(n).color(STEEL));
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, t: f64) {
        let stale = self.session.stale();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Structured Text").strong());
            if self.session.diags.is_empty() && !stale {
                ui.label(RichText::new("✔ compiles").color(STEEL));
            } else if !self.session.diags.is_empty() {
                ui.label(RichText::new(format!("✖ {} error(s)", self.session.diags.len())).color(RED));
            } else {
                ui.label(RichText::new("…").color(STEEL));
            }
        });
        if stale && self.session.build.is_some() && !self.session.diags.is_empty() {
            ui.label(
                RichText::new("Still running the last version that compiled.")
                    .small()
                    .color(STEEL),
            );
        }
        let diags: Vec<(u32, u32)> = self
            .session
            .diags
            .iter()
            .map(|d| (d.pos.line, d.pos.col))
            .collect();
        let font = FontId::monospace(13.0);
        let lines = self.session.src.lines().count().max(1) + usize::from(self.session.src.ends_with('\n'));
        let avail = ui.available_height();
        let list_h = if self.session.diags.is_empty() { 0.0 } else { 110.0 };
        egui::ScrollArea::both()
            .id_salt("pg-editor-scroll")
            .max_height((avail - list_h).max(120.0))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    // The gutter: line numbers, an error's in red.
                    let mut gutter = LayoutJob::default();
                    for n in 1..=lines {
                        let bad = diags.iter().any(|(l, _)| *l as usize == n);
                        gutter.append(
                            &format!("{n:>3}\n"),
                            0.0,
                            TextFormat::simple(font.clone(), if bad { RED } else { STEEL }),
                        );
                    }
                    ui.label(gutter);
                    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                        let job = highlight(buf.as_str(), &diags, &font);
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let r = ui.add(
                        egui::TextEdit::multiline(&mut self.session.src)
                            .id(egui::Id::unique("pg-source"))
                            .code_editor()
                            .char_limit(MAX_SOURCE)
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .lock_focus(true)
                            .layouter(&mut layouter),
                    );
                    if r.changed() {
                        self.pending = Some(t);
                        self.notice = None;
                    }
                });
            });
        if !self.session.diags.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("pg-diags")
                .max_height(list_h)
                .show(ui, |ui| {
                    for d in &self.session.diags {
                        ui.label(
                            RichText::new(format!("{}:{}  {}", d.pos.line, d.pos.col, d.msg))
                                .monospace()
                                .color(RED),
                        );
                    }
                });
        }
    }

    fn side(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, label) in [(Tab::Io, "I/O"), (Tab::Why, "Why"), (Tab::Claude, "Ask Claude")] {
                let b = egui::Button::selectable(self.tab == tab, RichText::new(label).strong())
                    .min_size(Vec2::new(80.0, TAP * 0.75));
                if ui.add(b).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.separator();
        match self.tab {
            Tab::Io => self.io(ui),
            Tab::Why => self.why(ui),
            Tab::Claude => self.claude(ui),
        }
    }

    fn io(&mut self, ui: &mut egui::Ui) {
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(STEEL));
            return;
        }
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        let outputs: Vec<Point> = self.session.outputs().cloned().collect();
        egui::ScrollArea::vertical().id_salt("pg-io").show(ui, |ui| {
            ui.label(RichText::new("Inputs").strong());
            if inputs.is_empty() {
                ui.label(
                    RichText::new("No VAR_INPUT or %IX bit: add one to get a control.")
                        .small()
                        .color(STEEL),
                );
            }
            for p in &inputs {
                if let Some(v) = input_ui(ui, p) {
                    self.session.set_input(&p.name, v);
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Outputs").strong())
                .on_hover_text("click: why is it in this state?");
            if outputs.is_empty() {
                ui.label(
                    RichText::new("No VAR_OUTPUT or %QX bit: add one to get a lamp.")
                        .small()
                        .color(STEEL),
                );
            }
            for p in &outputs {
                if output_ui(ui, p) {
                    self.explain(&p.name);
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Trend").strong())
                .on_hover_text(format!("up to {TREND_SIGNALS} signals"));
            ui.horizontal_wrapped(|ui| {
                for p in outputs.iter().chain(&inputs) {
                    let on = self.session.trend.signals.contains(&p.name);
                    if ui.selectable_label(on, p.name.to_ascii_lowercase()).clicked() {
                        self.session.trend.toggle(&p.name);
                    }
                }
            });
        });
    }

    fn why(&mut self, ui: &mut egui::Ui) {
        let Some(v) = &mut self.viewer else {
            ui.label(RichText::new("No diagram for this program.").color(STEEL));
            return;
        };
        v.outputs_ui(ui, self.session.trace.as_ref().map(|t| t as &dyn Scan));
        ui.separator();
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        let ask = AskUi {
            blocked: Some("In the playground, ask Claude in the Ask Claude tab."),
            ask: None,
            token: None,
        };
        let _ = v.right(ui, scan, Some(self.session.now_ms), ask);
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
                "Pick signals to trend in the I/O tab",
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

    fn diagram(&mut self, ui: &mut egui::Ui) {
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        if let Some(v) = &mut self.viewer {
            v.canvas(ui, scan);
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

impl Default for PlaygroundView {
    fn default() -> Self {
        Self::new()
    }
}

/// The editor's text as a layout: a line with a diagnostic on a dark red
/// ground, its column to the end of the word underlined.
#[must_use]
pub fn highlight(text: &str, diags: &[(u32, u32)], font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), Color32::from_gray(230));
    let bad_ground = Color32::from_rgb(0x4A, 0x16, 0x14);
    for (k, line) in text.split_inclusive('\n').enumerate() {
        let n = u32::try_from(k + 1).unwrap_or(u32::MAX);
        let Some(&(_, col)) = diags.iter().find(|(l, _)| *l == n) else {
            job.append(line, 0.0, plain.clone());
            continue;
        };
        let ground = TextFormat {
            background: bad_ground,
            ..plain.clone()
        };
        // Columns count characters from 1.
        let start = line
            .char_indices()
            .nth(col.saturating_sub(1) as usize)
            .map_or(line.trim_end_matches('\n').len(), |(i, _)| i);
        let rest = &line[start..];
        let word = rest
            .char_indices()
            .find(|(i, c)| *i > 0 && !(c.is_alphanumeric() || *c == '_'))
            .map_or(rest.trim_end_matches('\n').len(), |(i, _)| i)
            .max(
                rest.chars()
                    .next()
                    .filter(|c| *c != '\n')
                    .map_or(0, char::len_utf8),
            );
        job.append(&line[..start], 0.0, ground.clone());
        job.append(
            &rest[..word],
            0.0,
            TextFormat {
                underline: Stroke::new(1.5, RED),
                color: WHITE,
                ..ground.clone()
            },
        );
        job.append(&rest[word..], 0.0, ground);
    }
    job.wrap.max_width = f32::INFINITY;
    job
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
    ui.add(b).on_hover_text("why is it in this state?").clicked()
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
