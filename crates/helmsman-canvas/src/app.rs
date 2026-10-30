//! Application state and everything around the canvas: panels, navigation
//! into blocks, undo, the simulation clock, save and load.

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, RichText};
use helmsman_graph::{BlockId, CmpOp, Kind, MathOp, Node, NodeId, Project, Scope, Sim, Ty};

use crate::andon::AndonView;
use crate::box_view::BoxView;
use crate::canvas::{Interaction, View};
use crate::example;
use crate::look;
use crate::review_view::ReviewView;
use crate::st_view::{StView, brand};

const STORAGE_KEY: &str = "helmsman-canvas/project";
const UNDO_DEPTH: usize = 200;

/// One level of "inside a block": the definition shown, and the instance it
/// was entered through (none when opened from the library, so no live values).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub block: BlockId,
    pub instance: Option<NodeId>,
}

pub struct App {
    pub project: Project,
    pub sim: Sim,
    pub running: bool,
    pub scan_ms: f64,
    pub speed: f64,
    acc_ms: f64,
    pub nav: Vec<Frame>,
    pub views: HashMap<Scope, View>,
    pub selected: BTreeSet<NodeId>,
    pub ix: Interaction,
    undo: Vec<Project>,
    redo: Vec<Project>,
    /// Tag and time of the last coalescable edit (typing into a field).
    last_edit: Option<(String, f64)>,
    pub clipboard: Option<(Project, Scope, BTreeSet<NodeId>)>,
    pub toast: Option<(String, f64, bool)>,
    io_window: Option<String>,
    show_help: bool,
    pub mode: Mode,
    /// Where the live ST view's scans come from.
    pub source: Source,
    /// The demo's live ST view, built the first time it is opened.
    pub st: Option<StView>,
    /// The box's, likewise.
    pub boxv: Option<BoxView>,
    /// The proposal review screen, built the first time it is opened.
    pub review: Option<ReviewView>,
    /// The floor screen, likewise.
    pub andon: Option<AndonView>,
}

/// Where the live ST view's scans come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Source {
    /// A module running in this page.
    #[default]
    Demo,
    /// The box that served the page, over its API.
    Box,
}

/// Which view fills the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// The node editor (lab tool).
    #[default]
    Lab,
    /// A module's ST as a live, read-only diagram.
    Live,
    /// The box's proposals: diffs, sim replay, approve with a PIN.
    Review,
    /// The floor screen: the box's andon tiles, full screen (`?andon`).
    Andon,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let project = cc
            .storage
            .and_then(|s| s.get_string(STORAGE_KEY))
            .and_then(|s| Project::from_json(&s).ok())
            .unwrap_or_else(example::project);
        // The operator's view first; the editor is a lab tool (TODO.md
        // canvas-default-mode).
        // In a browser the page came from a box (`helmsman-plc --ui`):
        // watch it, unless the page asks for the demo (`?demo`).
        #[cfg(target_arch = "wasm32")]
        let source = if crate::web::wants_demo() {
            Source::Demo
        } else {
            Source::Box
        };
        #[cfg(not(target_arch = "wasm32"))]
        let source = Source::Demo;
        #[cfg(target_arch = "wasm32")]
        let mode = if crate::web::wants("andon") {
            Mode::Andon
        } else {
            Mode::Live
        };
        #[cfg(not(target_arch = "wasm32"))]
        let mode = Mode::Live;
        Self {
            mode,
            source,
            ..Self::with_project(project)
        }
    }

    pub fn with_project(project: Project) -> Self {
        Self {
            project,
            sim: Sim::default(),
            running: true,
            scan_ms: 10.0,
            speed: 1.0,
            acc_ms: 0.0,
            nav: Vec::new(),
            views: HashMap::new(),
            selected: BTreeSet::new(),
            ix: Interaction::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            clipboard: None,
            toast: None,
            io_window: None,
            show_help: false,
            mode: Mode::Lab,
            source: Source::Demo,
            st: None,
            boxv: None,
            review: None,
            andon: None,
        }
    }

    pub fn scope(&self) -> Scope {
        self.nav.last().map_or(Scope::Root, |f| Scope::Block(f.block))
    }

    /// Instance path to the graph on screen, when it was entered through
    /// instances all the way down (so its live values exist).
    pub fn live_path(&self) -> Option<Vec<NodeId>> {
        self.nav.iter().map(|f| f.instance).collect()
    }

    pub fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>, error: bool) {
        self.toast = Some((msg.into(), ctx.input(|i| i.time), error));
    }

    /// Records `before` as an undo step if the project changed since.
    pub fn commit(&mut self, before: Project) {
        if before != self.project {
            self.undo.push(before);
            if self.undo.len() > UNDO_DEPTH {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.last_edit = None;
        }
    }

    /// Like [`Self::commit`], but repeated edits with the same tag within a
    /// second collapse into one undo step (typing a name, dragging a slider).
    pub fn commit_coalesced(&mut self, before: Project, tag: &str, now: f64) {
        if before == self.project {
            return;
        }
        let same = self
            .last_edit
            .as_ref()
            .is_some_and(|(t, at)| t == tag && now - at < 1.0);
        if !same {
            self.commit(before);
        }
        self.last_edit = Some((tag.into(), now));
    }

    /// Folds the step just committed from `before` into one with whatever
    /// changed since (add a node, then wire it: one undo).
    pub fn undo_merge(&mut self, before: Project) {
        if self.undo.last() == Some(&before) {
            self.undo.pop();
        }
        self.commit(before);
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        self.last_edit = None;
        self.fix_nav();
        let g = self.project.graph(self.scope()).expect("fix_nav keeps it valid");
        self.selected.retain(|id| g.nodes.contains_key(id));
    }

    /// Drops navigation levels that no longer exist (after undo, delete, load).
    pub fn fix_nav(&mut self) {
        let mut scope = Scope::Root;
        let mut keep = 0;
        for f in &self.nav {
            let Some(g) = self.project.graph(scope) else { break };
            if !self.project.blocks.contains_key(&f.block) {
                break;
            }
            if let Some(i) = f.instance
                && g.nodes.get(&i).map(|n| &n.kind) != Some(&Kind::Block(f.block))
            {
                break;
            }
            keep += 1;
            scope = Scope::Block(f.block);
        }
        self.nav.truncate(keep);
    }

    pub fn enter(&mut self, instance: NodeId) {
        let Some(Kind::Block(b)) = self
            .project
            .graph(self.scope())
            .and_then(|g| g.nodes.get(&instance))
            .map(|n| &n.kind)
        else {
            return;
        };
        self.nav.push(Frame {
            block: *b,
            instance: Some(instance),
        });
        self.selected.clear();
        self.ix = Interaction::default();
    }

    pub fn open_definition(&mut self, block: BlockId) {
        self.nav = vec![Frame {
            block,
            instance: None,
        }];
        self.selected.clear();
        self.ix = Interaction::default();
    }

    pub fn go_up(&mut self, levels: usize) {
        let keep = self.nav.len().saturating_sub(levels);
        self.nav.truncate(keep);
        self.selected.clear();
        self.ix = Interaction::default();
    }

    pub fn add_node(&mut self, ctx: &egui::Context, kind: Kind, pos: [f32; 2]) -> Option<NodeId> {
        let before = self.project.clone();
        match self.project.add_node(self.scope(), Node::new(kind, pos)) {
            Ok(id) => {
                self.commit(before);
                self.selected = BTreeSet::from([id]);
                Some(id)
            }
            Err(e) => {
                self.toast(ctx, e.to_string(), true);
                None
            }
        }
    }

    pub fn delete_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let before = self.project.clone();
        let sel = std::mem::take(&mut self.selected);
        self.project.remove_nodes(self.scope(), &sel);
        self.commit(before);
    }

    pub fn duplicate_selected(&mut self) {
        let before = self.project.clone();
        if let Ok(ids) = self.project.duplicate(self.scope(), &self.selected, [30.0, 30.0]) {
            self.selected = ids.into_iter().collect();
        }
        self.commit(before);
    }

    pub fn group_selected(&mut self, ctx: &egui::Context) {
        let before = self.project.clone();
        let name = format!("FB{}", self.project.blocks.len() + 1);
        match self.project.collapse(self.scope(), &self.selected, &name) {
            Ok((_, inst)) => {
                self.commit(before);
                self.selected = BTreeSet::from([inst]);
                self.toast(
                    ctx,
                    format!("grouped into {name}; double-click it to go inside"),
                    false,
                );
            }
            Err(e) => self.toast(ctx, e.to_string(), true),
        }
    }

    pub fn copy_selected(&mut self) {
        if !self.selected.is_empty() {
            self.clipboard = Some((self.project.clone(), self.scope(), self.selected.clone()));
        }
    }

    /// Pastes the copied nodes (and the wires among them) into the graph on
    /// screen, offset so they don't sit on top of the originals.
    pub fn paste(&mut self, ctx: &egui::Context) {
        let Some((src, scope, ids)) = self.clipboard.clone() else {
            return;
        };
        let Some(g) = src.graph(scope) else { return };
        let before = self.project.clone();
        let mut map = std::collections::BTreeMap::new();
        for id in &ids {
            let Some(n) = g.nodes.get(id) else { continue };
            let mut n = n.clone();
            n.pos = [n.pos[0] + 40.0, n.pos[1] + 40.0];
            // Blocks copied from another project version may be gone.
            match self.project.add_node(self.scope(), n) {
                Ok(new) => {
                    map.insert(*id, new);
                }
                Err(e) => self.toast(ctx, format!("skipped a node: {e}"), true),
            }
        }
        for w in &g.wires {
            if let (Some(&from), Some(&to)) = (map.get(&w.from), map.get(&w.to)) {
                let _ = self
                    .project
                    .connect(self.scope(), helmsman_graph::Wire { from, to, ..*w });
            }
        }
        self.selected = map.into_values().collect();
        self.commit(before);
        // Offset the clipboard so a second paste lands further along.
        if let Some((p, s, _)) = &mut self.clipboard
            && let Some(g) = p.graph_mut(*s)
        {
            for id in &ids {
                if let Some(n) = g.nodes.get_mut(id) {
                    n.pos = [n.pos[0] + 40.0, n.pos[1] + 40.0];
                }
            }
        }
    }

    /// Advances the simulation by real time scaled by `speed`, in whole scans.
    fn tick(&mut self, ctx: &egui::Context) {
        if !self.running {
            return;
        }
        let dt = f64::from(ctx.input(|i| i.stable_dt)).min(0.25) * 1000.0 * self.speed;
        self.acc_ms += dt;
        let mut budget = 2000;
        while self.acc_ms >= self.scan_ms && budget > 0 {
            self.sim.scan(&self.project, self.scan_ms);
            self.acc_ms -= self.scan_ms;
            budget -= 1;
        }
        if budget == 0 {
            self.acc_ms = 0.0;
        }
        ctx.request_repaint();
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (events, m) = ctx.input(|i| (i.events.clone(), i.modifiers));
        for e in events {
            match e {
                egui::Event::Copy => self.copy_selected(),
                egui::Event::Cut => {
                    self.copy_selected();
                    self.delete_selected();
                }
                egui::Event::Paste(_) => self.paste(ctx),
                egui::Event::Key {
                    key, pressed: true, ..
                } => match key {
                    egui::Key::Delete | egui::Key::Backspace => self.delete_selected(),
                    egui::Key::Z if m.command && m.shift => self.redo(),
                    egui::Key::Z if m.command => self.undo(),
                    egui::Key::Y if m.command => self.redo(),
                    egui::Key::D if m.command => self.duplicate_selected(),
                    egui::Key::G if m.command => self.group_selected(ctx),
                    egui::Key::A if m.command => {
                        if let Some(g) = self.project.graph(self.scope()) {
                            self.selected = g.nodes.keys().copied().collect();
                        }
                    }
                    egui::Key::Escape => {
                        if self.ix.busy() {
                            self.ix = Interaction::default();
                        } else if !self.nav.is_empty() {
                            self.go_up(1);
                        }
                    }
                    egui::Key::F => self.fit_requested(),
                    egui::Key::Space if m.shift => self.running = !self.running,
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn fit_requested(&mut self) {
        self.views.remove(&self.scope());
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("Live ST view").color(brand::YELLOW))
                .on_hover_text("a module's Structured Text as a live diagram")
                .clicked()
            {
                self.mode = Mode::Live;
            }
            ui.separator();
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    let before = self.project.clone();
                    self.project = Project::default();
                    self.after_load(before);
                    ui.close();
                }
                if ui.button("Load example").clicked() {
                    let before = self.project.clone();
                    self.project = example::project();
                    self.after_load(before);
                    ui.close();
                }
                if ui.button("Export / import JSON").clicked() {
                    self.io_window = Some(self.project.to_json());
                    ui.close();
                }
            });
            ui.add_enabled_ui(!self.undo.is_empty(), |ui| {
                if ui.button("Undo").on_hover_text("Cmd/Ctrl+Z").clicked() {
                    self.undo();
                }
            });
            ui.add_enabled_ui(!self.redo.is_empty(), |ui| {
                if ui.button("Redo").on_hover_text("Cmd/Ctrl+Shift+Z").clicked() {
                    self.redo();
                }
            });
            ui.separator();

            // Breadcrumbs.
            if ui.selectable_label(self.nav.is_empty(), "Root").clicked() {
                self.go_up(self.nav.len());
            }
            let mut up = None;
            for (i, f) in self.nav.iter().enumerate() {
                ui.label(RichText::new("›").color(look::DIM));
                let name = self.project.blocks.get(&f.block).map_or("?", |d| d.name.as_str());
                let text = if f.instance.is_some() {
                    name.to_string()
                } else {
                    format!("{name} (definition)")
                };
                if ui.selectable_label(i + 1 == self.nav.len(), text).clicked() {
                    up = Some(self.nav.len() - i - 1);
                }
            }
            if let Some(n) = up {
                self.go_up(n);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("?").clicked() {
                    self.show_help = !self.show_help;
                }
                ui.label(
                    RichText::new(format!(
                        "t={:.1}s  scans {}",
                        self.sim.time_ms / 1000.0,
                        self.sim.scans
                    ))
                    .monospace()
                    .color(look::DIM),
                );
                ui.add(
                    egui::DragValue::new(&mut self.speed)
                        .range(0.1..=20.0)
                        .speed(0.05)
                        .suffix("x"),
                )
                .on_hover_text("simulation speed");
                ui.add(
                    egui::DragValue::new(&mut self.scan_ms)
                        .range(1.0..=1000.0)
                        .speed(1.0)
                        .suffix(" ms"),
                )
                .on_hover_text("scan period");
                if ui
                    .button("Reset")
                    .on_hover_text("clear timers, counters, latches")
                    .clicked()
                {
                    self.sim.reset();
                }
                if ui.add_enabled(!self.running, egui::Button::new("Step")).clicked() {
                    self.sim.scan(&self.project, self.scan_ms);
                }
                let label = if self.running { "Pause" } else { "Run" };
                if ui.button(label).on_hover_text("Shift+Space").clicked() {
                    self.running = !self.running;
                }
                let dot = if self.running { look::ON } else { look::DIM };
                ui.label(RichText::new("●").color(dot));
            });
        });
    }

    fn after_load(&mut self, before: Project) {
        self.commit(before);
        self.sim.reset();
        self.nav.clear();
        self.views.clear();
        self.selected.clear();
    }

    fn palette(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let at = self.drop_point();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Palette");
            ui.label(
                RichText::new("click to add · double-click canvas to search")
                    .small()
                    .color(look::DIM),
            );
            let mut current = "";
            for (cat, kind) in Kind::palette() {
                if cat != current {
                    ui.add_space(4.0);
                    ui.label(RichText::new(cat).strong().color(look::DIM));
                    current = cat;
                }
                let title = kind.title(&self.project);
                let button = egui::Button::new(RichText::new(title).color(look::TEXT))
                    .fill(look::accent(&kind).gamma_multiply(0.55));
                if ui.add_sized([ui.available_width(), 20.0], button).clicked() {
                    self.add_node(&ctx, kind, at);
                }
            }
            if self.scope() != Scope::Root {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Pins (this block's interface)")
                        .strong()
                        .color(look::DIM),
                );
                for (label, kind) in pin_kinds() {
                    let button = egui::Button::new(label).fill(look::accent(&kind).gamma_multiply(0.55));
                    if ui.add_sized([ui.available_width(), 20.0], button).clicked() {
                        self.add_node(&ctx, kind, at);
                    }
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("Blocks");
                if ui.small_button("+ new").clicked() {
                    let before = self.project.clone();
                    let name = format!("FB{}", self.project.blocks.len() + 1);
                    let b = self.project.new_block(&name);
                    self.commit(before);
                    self.open_definition(b);
                }
            });
            ui.label(
                RichText::new("select nodes + Cmd/Ctrl+G to make one")
                    .small()
                    .color(look::DIM),
            );
            let ids: Vec<BlockId> = self.project.blocks.keys().copied().collect();
            for b in ids {
                let uses = self.project.uses(b);
                let name = self.project.blocks[&b].name.clone();
                ui.horizontal(|ui| {
                    let add = egui::Button::new(RichText::new(&name).color(look::TEXT))
                        .fill(look::accent(&Kind::Block(b)).gamma_multiply(0.55));
                    if ui.add(add).on_hover_text("add an instance here").clicked() {
                        self.add_node(&ctx, Kind::Block(b), at);
                    }
                    ui.label(RichText::new(format!("×{uses}")).small().color(look::DIM));
                    if ui
                        .small_button("edit")
                        .on_hover_text("open the definition")
                        .clicked()
                    {
                        self.open_definition(b);
                    }
                    if uses == 0 && ui.small_button("del").clicked() {
                        let before = self.project.clone();
                        if self.project.delete_block(b).is_ok() {
                            self.commit(before);
                            self.fix_nav();
                        }
                    }
                });
            }
        });
    }

    /// Where palette clicks put a node: the middle of the current view.
    fn drop_point(&self) -> [f32; 2] {
        let v = self.views.get(&self.scope()).copied().unwrap_or_default();
        let p = v.to_world(v.last_center);
        [p.x - 80.0, p.y - 30.0]
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        ui.heading("Inspector");
        let scope = self.scope();
        if self.selected.len() > 1 {
            ui.label(format!("{} nodes selected", self.selected.len()));
            if ui.button("Group into block (Cmd/Ctrl+G)").clicked() {
                self.group_selected(&ctx);
            }
            if ui.button("Duplicate (Cmd/Ctrl+D)").clicked() {
                self.duplicate_selected();
            }
            if ui.button("Delete").clicked() {
                self.delete_selected();
            }
            return;
        }
        let Some(&id) = self.selected.iter().next() else {
            ui.label(RichText::new("select a node").color(look::DIM));
            ui.add_space(8.0);
            self.graph_stats(ui);
            return;
        };
        let before = self.project.clone();
        let blocks: Vec<(BlockId, String)> = self
            .project
            .blocks
            .iter()
            .map(|(b, d)| (*b, d.name.clone()))
            .collect();
        let Some(node) = self.project.graph_mut(scope).and_then(|g| g.nodes.get_mut(&id)) else {
            return;
        };
        let mut tag = format!("{id:?}");
        let mut action = None;
        ui.label(RichText::new(format!("node #{}", id.0)).small().color(look::DIM));
        if !matches!(
            node.kind,
            Kind::PinIn { .. } | Kind::PinOut { .. } | Kind::Note { .. }
        ) {
            ui.horizontal(|ui| {
                ui.label("Label");
                ui.text_edit_singleline(&mut node.label);
            });
        }
        egui::Grid::new("params")
            .num_columns(2)
            .show(ui, |ui| match &mut node.kind {
                Kind::Toggle { on } => {
                    ui.label("On");
                    ui.checkbox(on, "");
                    ui.end_row();
                }
                Kind::Const { value } => {
                    ui.label("Value");
                    ui.add(egui::DragValue::new(value).speed(0.1));
                    ui.end_row();
                }
                Kind::Clock { period_ms } => {
                    ui.label("Period");
                    ui.add(
                        egui::DragValue::new(period_ms)
                            .range(1.0..=600_000.0)
                            .suffix(" ms"),
                    );
                    ui.end_row();
                }
                Kind::And { inputs } | Kind::Or { inputs } => {
                    ui.label("Inputs");
                    ui.add(egui::DragValue::new(inputs).range(2..=8));
                    ui.end_row();
                }
                Kind::Ton { preset_ms }
                | Kind::Tof { preset_ms }
                | Kind::Tp { preset_ms }
                | Kind::Tonr { preset_ms } => {
                    ui.label("Preset");
                    ui.add(
                        egui::DragValue::new(preset_ms)
                            .range(0.0..=3_600_000.0)
                            .speed(10.0)
                            .suffix(" ms"),
                    );
                    ui.end_row();
                }
                Kind::Ctu { preset } => {
                    ui.label("Preset");
                    ui.add(egui::DragValue::new(preset).range(0.0..=1e9).speed(1.0));
                    ui.end_row();
                }
                Kind::Math(op) => {
                    ui.label("Op");
                    egui::ComboBox::from_id_salt("math")
                        .selected_text(op.symbol())
                        .show_ui(ui, |ui| {
                            for o in MathOp::ALL {
                                ui.selectable_value(op, o, o.symbol());
                            }
                        });
                    ui.end_row();
                }
                Kind::Compare(op) => {
                    ui.label("Op");
                    egui::ComboBox::from_id_salt("cmp")
                        .selected_text(op.symbol())
                        .show_ui(ui, |ui| {
                            for o in CmpOp::ALL {
                                ui.selectable_value(op, o, o.symbol());
                            }
                        });
                    ui.end_row();
                }
                Kind::PinIn { name, ty } | Kind::PinOut { name, ty } => {
                    ui.label("Name");
                    ui.text_edit_singleline(name);
                    ui.end_row();
                    ui.label("Type");
                    ui.horizontal(|ui| {
                        ui.selectable_value(ty, Ty::Bool, "BOOL");
                        ui.selectable_value(ty, Ty::Num, "NUM");
                    });
                    ui.end_row();
                }
                Kind::Note { text } => {
                    ui.label("Text");
                    ui.text_edit_multiline(text);
                    ui.end_row();
                }
                Kind::Block(b) => {
                    let b = *b;
                    if let Some((_, name)) = blocks.iter().find(|(x, _)| *x == b) {
                        let mut name = name.clone();
                        ui.label("Block");
                        if ui.text_edit_singleline(&mut name).changed() {
                            action = Some(Action::Rename(b, name));
                            tag = format!("rename {b:?}");
                        }
                        ui.end_row();
                    }
                    ui.label("");
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() {
                            action = Some(Action::Enter);
                        }
                        if ui
                            .button("Make unique")
                            .on_hover_text("detach this instance into its own copy")
                            .clicked()
                        {
                            action = Some(Action::Unique);
                        }
                        if ui
                            .button("Explode")
                            .on_hover_text("replace with its contents")
                            .clicked()
                        {
                            action = Some(Action::Explode);
                        }
                    });
                    ui.end_row();
                }
                _ => {
                    ui.label(RichText::new("no parameters").color(look::DIM));
                    ui.end_row();
                }
            });
        if let Some(Action::Rename(b, name)) = &action
            && let Some(d) = self.project.blocks.get_mut(b)
        {
            d.name.clone_from(name);
        }
        // A pin's type may have changed: drop wires that no longer fit.
        self.project.prune();
        self.commit_coalesced(before, &tag, now);
        match action {
            Some(Action::Enter) => self.enter(id),
            Some(Action::Unique) => {
                let before = self.project.clone();
                if self.project.make_unique(scope, id).is_ok() {
                    self.commit(before);
                }
            }
            Some(Action::Explode) => {
                let before = self.project.clone();
                match self.project.explode(scope, id) {
                    Ok(ids) => {
                        self.commit(before);
                        self.selected = ids.into_iter().collect();
                    }
                    Err(e) => self.toast(&ctx, e.to_string(), true),
                }
            }
            _ => {}
        }
        ui.add_space(8.0);
        if ui.button("Delete node").clicked() {
            self.delete_selected();
        }
    }

    fn graph_stats(&self, ui: &mut egui::Ui) {
        let Some(g) = self.project.graph(self.scope()) else {
            return;
        };
        let depth = self
            .project
            .blocks
            .keys()
            .map(|b| self.depth(*b))
            .max()
            .unwrap_or(0);
        ui.label(
            RichText::new(format!(
                "{} nodes, {} wires here\n{} block definitions, nesting depth {}",
                g.nodes.len(),
                g.wires.len(),
                self.project.blocks.len(),
                depth
            ))
            .small()
            .color(look::DIM),
        );
    }

    /// How many levels of blocks an instance of `b` contains, itself included.
    fn depth(&self, b: BlockId) -> usize {
        fn go(p: &Project, b: BlockId, budget: usize) -> usize {
            if budget == 0 {
                return 0;
            }
            let inner = p.blocks.get(&b).map_or(0, |d| {
                d.graph
                    .nodes
                    .values()
                    .filter_map(|n| {
                        if let Kind::Block(c) = n.kind {
                            Some(go(p, c, budget - 1))
                        } else {
                            None
                        }
                    })
                    .max()
                    .unwrap_or(0)
            });
            1 + inner
        }
        go(&self.project, b, 64)
    }

    fn io_window(&mut self, ctx: &egui::Context) {
        let Some(mut text) = self.io_window.take() else {
            return;
        };
        let mut open = true;
        let mut load = false;
        egui::Window::new("Project JSON")
            .open(&mut open)
            .default_size([520.0, 420.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(text.clone());
                    }
                    if ui
                        .button("Load this")
                        .on_hover_text("replace the project with the text below")
                        .clicked()
                    {
                        load = true;
                    }
                    if ui.button("Refresh").clicked() {
                        text = self.project.to_json();
                    }
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .code_editor()
                            .desired_width(f32::INFINITY),
                    );
                });
            });
        if load {
            match Project::from_json(&text) {
                Ok(p) => {
                    let before = std::mem::replace(&mut self.project, p);
                    self.after_load(before);
                    self.toast(ctx, "loaded", false);
                }
                Err(e) => self.toast(ctx, format!("not loaded: {e}"), true),
            }
        }
        if open {
            self.io_window = Some(text);
        }
    }

    fn help(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        egui::Window::new("Controls")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("keys")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (k, v) in [
                            ("drag background", "box select (Shift adds)"),
                            ("right/middle drag, Space+drag", "pan"),
                            ("scroll", "pan"),
                            ("pinch, Ctrl+scroll", "zoom"),
                            ("F", "fit view"),
                            ("drag from a port", "wire; drop on nothing to search"),
                            ("drag from a wired input", "pick the wire up"),
                            ("double-click background", "search and add"),
                            ("double-click a block", "go inside"),
                            ("Esc", "cancel, or go up a level"),
                            ("click switch / hold button", "operate it while running"),
                            ("Cmd/Ctrl+G", "group selection into a block"),
                            ("Cmd/Ctrl+D", "duplicate"),
                            ("Cmd/Ctrl+C / V / X", "copy, paste, cut"),
                            ("Cmd/Ctrl+Z, Shift+Z", "undo, redo"),
                            ("Delete / Backspace", "delete selection"),
                            ("Shift+Space", "run / pause"),
                        ] {
                            ui.label(RichText::new(k).monospace());
                            ui.label(v);
                            ui.end_row();
                        }
                    });
            });
        self.show_help = open;
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((msg, at, error)) = self.toast.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now - at > 3.0 {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -20.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let color = if error { look::ERR } else { look::TEXT };
                    ui.label(RichText::new(msg).color(color));
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

enum Action {
    Rename(BlockId, String),
    Enter,
    Unique,
    Explode,
}

pub fn pin_kinds() -> [(&'static str, Kind); 4] {
    [
        (
            "Pin in · BOOL",
            Kind::PinIn {
                name: "IN".into(),
                ty: Ty::Bool,
            },
        ),
        (
            "Pin in · NUM",
            Kind::PinIn {
                name: "N".into(),
                ty: Ty::Num,
            },
        ),
        (
            "Pin out · BOOL",
            Kind::PinOut {
                name: "Q".into(),
                ty: Ty::Bool,
            },
        ),
        (
            "Pin out · NUM",
            Kind::PinOut {
                name: "OUT".into(),
                ty: Ty::Num,
            },
        ),
    ]
}

impl App {
    /// One frame of the whole window. Split from [`eframe::App::ui`] so tests
    /// can drive it without a window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.mode == Mode::Andon {
            self.andon.get_or_insert_with(AndonView::new).show(ui);
            return;
        }
        if matches!(self.mode, Mode::Live | Mode::Review) {
            self.show_live(ui);
            return;
        }
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("palette")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| self.palette(ui));
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(240.0)
            .show(ui, |ui| self.inspector(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
        self.tick(&ctx);
        self.io_window(&ctx);
        self.help(&ctx);
        self.toast_ui(&ctx);
    }
}

impl App {
    fn show_live(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("st-top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("Lab editor")
                    .on_hover_text("back to the node editor")
                    .clicked()
                {
                    self.mode = Mode::Lab;
                }
                ui.separator();
                ui.label(RichText::new("HELMSMAN").strong().color(brand::YELLOW));
                ui.separator();
                for (s, label, tip) in [
                    (Source::Box, "Box", "the box that served this page, live"),
                    (
                        Source::Demo,
                        "Demo",
                        "a module running in this page, no box needed",
                    ),
                ] {
                    if ui
                        .selectable_label(self.mode == Mode::Live && self.source == s, label)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.source = s;
                        self.mode = Mode::Live;
                    }
                }
                if ui
                    .selectable_label(self.mode == Mode::Review, "Proposals")
                    .on_hover_text("the agent's drafts on this box: review, replay, approve")
                    .clicked()
                {
                    self.mode = Mode::Review;
                }
                ui.separator();
                ui.label(
                    RichText::new("live ST view · read-only: changes go through the agent as ST")
                        .color(brand::STEEL),
                );
            });
        });
        match self.mode {
            Mode::Lab => return,
            Mode::Review => {
                self.review.get_or_insert_with(ReviewView::new).show(ui);
                return;
            }
            Mode::Live => {}
        }
        match self.source {
            Source::Demo => self.st.get_or_insert_with(StView::demo).show(ui),
            Source::Box => {
                let b = self.boxv.get_or_insert_with(BoxView::new);
                b.show(ui);
                if std::mem::take(&mut b.wants_demo) {
                    self.source = Source::Demo;
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(STORAGE_KEY, self.project.to_json());
    }
}
