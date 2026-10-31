//! The live ST view: a module's Structured Text drawn as its diagram
//! (`helmsman-diagram`), read-only, with a traced scan on it every frame.
//!
//! [`Viewer`] draws a module from whatever scans it is given: the
//! simulation in the page (`playground_view`), or a box over its API
//! (`box_view`). The ST itself is never edited from the diagram
//! (`canvas-edit-scope`).

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
    epaint::CubicBezierShape,
};
use helmsman_diagram::layout::{HEADER, LABEL_H, in_port, out_port};
use helmsman_diagram::{
    Diagram, Drivers, Gate, Live, Module, NodeId, NodeKind, RegionId, Scan, annotate, drivers,
};
use helmsman_iec::Value;

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
    /// The warning colour: a forced input, the force banner.
    pub const WARN: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x1E);
}

use brand::{BLACK, GRAPHITE, PANEL, STEEL, WHITE, YELLOW};

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
    /// The variable last tapped (an output, or a block's first output),
    /// as a program path.
    pub because: Option<String>,
    /// Source lines drawn highlighted.
    pub highlight: Vec<u32>,
    /// The "because" path the last frame drew (see [`Self::highlighted`]).
    lit: Option<Drivers>,
    /// What the drag in progress holds.
    grab: Option<Grab>,
    /// Instances folded in the tree (their prefixes).
    folded: Vec<String>,
    /// How far the operator moved each block from where the layout put it,
    /// per POU. The view's own; the program is untouched.
    moved: Vec<Vec<(NodeId, Vec2)>>,
}

/// One row of the program tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub scope: usize,
    /// 0 for the program, 1 for what it calls, and so on.
    pub depth: usize,
    pub has_kids: bool,
    pub folded: bool,
}

/// What a drag on the diagram holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    Node(NodeId),
    Region(RegionId),
    Pan,
}

/// Where the operator moved blocks, by what they are rather than their
/// ids, so it survives an edit that renumbers them: (POU, the block's
/// label and source text, which of the same, offset).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arrangement(Vec<(String, String, String, usize, Vec2)>);

/// The smallest thing a finger can hit, in points.
pub const TAP: f32 = 44.0;

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
        let moved = vec![Vec::new(); module.pous.len()];
        Self {
            src: src.to_string(),
            module,
            scopes,
            scope: 0,
            cams,
            pick: None,
            because: None,
            highlight: Vec::new(),
            lit: None,
            grab: None,
            folded: Vec::new(),
            moved,
        }
    }

    /// Moves block `n` of the shown POU by `by` (diagram units).
    pub fn move_node(&mut self, n: NodeId, by: Vec2) {
        let pou = self.scopes[self.scope].pou;
        self.shift(pou, &[n], by);
        helmsman_diagram::layout::refit(&mut self.module.pous[pou].diagram);
    }

    /// Moves everything in region `r` of the shown POU by `by`.
    pub fn move_region(&mut self, r: RegionId, by: Vec2) {
        let pou = self.scopes[self.scope].pou;
        let members = helmsman_diagram::layout::inside(&self.module.pous[pou].diagram, r);
        self.shift(pou, &members, by);
        helmsman_diagram::layout::refit(&mut self.module.pous[pou].diagram);
    }

    fn shift(&mut self, pou: usize, nodes: &[NodeId], by: Vec2) {
        let d = &mut self.module.pous[pou].diagram;
        for &n in nodes {
            let Some(node) = d.nodes.get_mut(n.0 as usize) else {
                continue;
            };
            node.rect.x += by.x;
            node.rect.y += by.y;
            match self.moved[pou].iter_mut().find(|(m, _)| *m == n) {
                Some((_, total)) => *total += by,
                None => self.moved[pou].push((n, by)),
            }
        }
    }

    /// Whether the operator moved anything.
    #[must_use]
    pub fn rearranged(&self) -> bool {
        self.moved.iter().any(|m| m.iter().any(|(_, v)| *v != Vec2::ZERO))
    }

    /// Every block back where the layout put it.
    pub fn reset_arrangement(&mut self) {
        for pou in 0..self.moved.len() {
            for (n, v) in std::mem::take(&mut self.moved[pou]) {
                if let Some(node) = self.module.pous[pou].diagram.nodes.get_mut(n.0 as usize) {
                    node.rect.x -= v.x;
                    node.rect.y -= v.y;
                }
            }
            helmsman_diagram::layout::refit(&mut self.module.pous[pou].diagram);
        }
    }

    /// Where the operator moved blocks, to carry onto the next version of
    /// the program.
    #[must_use]
    pub fn arrangement(&self) -> Arrangement {
        let mut out = Vec::new();
        for (pou, moved) in self.module.pous.iter().zip(&self.moved) {
            for &(n, v) in moved {
                let Some(node) = pou.diagram.nodes.get(n.0 as usize) else {
                    continue;
                };
                let k = pou.diagram.nodes[..n.0 as usize]
                    .iter()
                    .filter(|m| m.label == node.label && m.text == node.text)
                    .count();
                out.push((pou.name.clone(), node.label.clone(), node.text.clone(), k, v));
            }
        }
        Arrangement(out)
    }

    /// Moves the blocks `a` names, wherever they still are; the rest stay
    /// where the layout puts them.
    pub fn arrange(&mut self, a: &Arrangement) {
        for (name, label, text, k, v) in &a.0 {
            let Some(pou) = self.module.pous.iter().position(|p| &p.name == name) else {
                continue;
            };
            let n = self.module.pous[pou]
                .diagram
                .nodes
                .iter()
                .filter(|m| &m.label == label && &m.text == text)
                .nth(*k)
                .map(|m| m.id);
            if let Some(n) = n {
                self.shift(pou, &[n], *v);
            }
        }
        for p in &mut self.module.pous {
            helmsman_diagram::layout::refit(&mut p.diagram);
        }
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

    /// The picked element's source line and its text (a block click
    /// sends the ST editor to the line).
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

    /// Whether `path` (a program path; `M.ON` for an instance's) names a
    /// variable of the program: a pick that survives a new version.
    #[must_use]
    pub fn knows(&self, path: &str) -> bool {
        let head = path.split('.').next().unwrap_or(path).to_ascii_uppercase();
        self.module.program().is_some_and(|p| p.vars.contains_key(&head))
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
    /// Shows scope `k`: the program or a block instance.
    pub fn open(&mut self, k: usize) {
        if k < self.scopes.len() && k != self.scope {
            self.scope = k;
            self.pick = None;
        }
    }

    /// The scope holding the shown one (an instance's caller), if any.
    #[must_use]
    pub fn parent(&self) -> Option<usize> {
        parent_of(&self.scopes, self.scope)
    }

    /// The chain from the program down to the shown scope.
    #[must_use]
    pub fn trail(&self) -> Vec<usize> {
        let mut out = vec![self.scope];
        // Each step drops a path segment, so this ends.
        while let Some(p) = parent_of(&self.scopes, out[out.len() - 1]) {
            out.push(p);
        }
        out.reverse();
        out
    }

    /// The program and its block instances as a tree, nested by call; a
    /// click shows one. `▸`/`▾` folds an instance's own instances.
    pub fn tree_ui(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        let mut fold = None;
        for row in self.tree_rows() {
            let s = &self.scopes[row.scope];
            ui.horizontal(|ui| {
                #[allow(clippy::cast_precision_loss)]
                ui.add_space(row.depth as f32 * 14.0);
                if row.has_kids {
                    if ui.small_button(if row.folded { "▸" } else { "▾" }).clicked() {
                        fold = Some(row.scope);
                    }
                } else {
                    ui.add_space(18.0);
                }
                let text = RichText::new(&s.label).monospace();
                if ui
                    .selectable_label(row.scope == self.scope, text)
                    .on_hover_text("show this one's diagram")
                    .clicked()
                {
                    chosen = Some(row.scope);
                }
            });
        }
        if let Some(k) = fold {
            self.toggle_fold(k);
        }
        if let Some(k) = chosen {
            self.open(k);
        }
    }

    /// The tree's rows, in order, skipping what folded instances hold.
    #[must_use]
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let mut out = Vec::new();
        let mut hidden: Option<&str> = None;
        for (k, s) in self.scopes.iter().enumerate() {
            if let Some(h) = hidden {
                if s.prefix.starts_with(h) {
                    continue;
                }
                hidden = None;
            }
            // The program's prefix is empty, so every instance is under it;
            // it is the root and never folds.
            let has_kids = k > 0
                && self
                    .scopes
                    .iter()
                    .any(|q| q.prefix.len() > s.prefix.len() && q.prefix.starts_with(s.prefix.as_str()));
            let folded = has_kids && self.folded.contains(&s.prefix);
            if folded {
                hidden = Some(&s.prefix);
            }
            out.push(TreeRow {
                scope: k,
                depth: s.prefix.matches('.').count(),
                has_kids,
                folded,
            });
        }
        out
    }

    /// Folds scope `k`'s own instances away in the tree, or shows them.
    pub fn toggle_fold(&mut self, k: usize) {
        let Some(p) = self.scopes.get(k).map(|s| s.prefix.clone()) else {
            return;
        };
        match self.folded.iter().position(|f| *f == p) {
            Some(i) => {
                self.folded.remove(i);
            }
            None => self.folded.push(p),
        }
    }

    /// "site › box › module" (from the host, `outer`), then the program
    /// and each instance down to the one shown, every part a link, and ↑
    /// for one level up. Returns the outer part clicked.
    pub fn crumbs_ui(&mut self, ui: &mut egui::Ui, outer: &[String]) -> Option<usize> {
        let mut hit = None;
        let mut go = None;
        ui.horizontal_wrapped(|ui| {
            let up = self.parent();
            if ui
                .add_enabled(up.is_some(), egui::Button::new("↑"))
                .on_hover_text("up one level (Alt+↑)")
                .clicked()
            {
                go = up;
            }
            for (i, o) in outer.iter().enumerate() {
                if ui.link(RichText::new(o).color(brand::STEEL)).clicked() {
                    hit = Some(i);
                }
                ui.label(RichText::new("›").color(brand::STEEL));
            }
            let trail = self.trail();
            for (j, &k) in trail.iter().enumerate() {
                if j > 0 {
                    ui.label(RichText::new("›").color(brand::STEEL));
                }
                let last = j + 1 == trail.len();
                let text = RichText::new(&self.scopes[k].label).monospace();
                let text = if last {
                    text.strong().color(brand::YELLOW)
                } else {
                    text
                };
                if ui.link(text).clicked() {
                    go = Some(k);
                }
            }
        });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp)) {
            go = go.or_else(|| self.parent());
        }
        if let Some(k) = go {
            self.open(k);
        }
        hit
    }

    /// The outputs with their values; a click picks one.
    pub fn outputs_ui(&mut self, ui: &mut egui::Ui, scan: Option<&dyn Scan>) {
        ui.label(RichText::new("Outputs").strong())
            .on_hover_text("click: pick it");
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

    /// Each variable the program reads, with where its block sits on
    /// screen after the last [`Self::canvas`], under its header (the
    /// playground puts the inputs' controls there; the header stays free
    /// to drag the block by). Empty while a block instance is shown.
    #[must_use]
    pub fn reads_on_screen(&self) -> Vec<(String, Rect)> {
        if !self.scopes[self.scope].prefix.is_empty() {
            return Vec::new();
        }
        let cam = self.cams[self.scope];
        self.diagram()
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Read { path } => {
                    let mut r = cam.rect(&n.rect);
                    r.min.y += HEADER * cam.zoom;
                    Some((path.clone(), r))
                }
                _ => None,
            })
            .collect()
    }

    /// The scope of the block instance drawn at screen point `p`, if it
    /// has a diagram of its own.
    fn instance_at(&self, cam: Cam, p: Pos2) -> Option<usize> {
        let (x, y) = cam.to_world(p);
        let n = self.diagram().node_at(x, y)?;
        let NodeKind::Fb { instance, .. } = &self.diagram().node(n).kind else {
            return None;
        };
        let want = format!("{}{instance}.", self.scopes[self.scope].prefix);
        self.scopes
            .iter()
            .position(|s| s.prefix.eq_ignore_ascii_case(&want))
    }

    /// What a drag starting at screen point `p` takes hold of.
    fn grab_at(&self, cam: Cam, p: Pos2) -> Grab {
        let (x, y) = cam.to_world(p);
        let d = self.diagram();
        if let Some(n) = d.node_at(x, y) {
            return Grab::Node(n);
        }
        // The innermost region whose label strip is under the pointer.
        d.regions
            .iter()
            .filter(|r| r.rect.contains(x, y) && y <= r.rect.y + LABEL_H)
            .min_by(|a, b| (a.rect.w * a.rect.h).total_cmp(&(b.rect.w * b.rect.h)))
            .map_or(Grab::Pan, |r| Grab::Region(r.id))
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
        let double = response.double_clicked();
        let cam = *cam;
        // A double click on a block instance opens it; anywhere else it
        // fits the diagram to the view.
        if double {
            let into = response
                .interact_pointer_pos()
                .and_then(|p| self.instance_at(cam, p));
            match into {
                Some(k) => self.open(k),
                None => self.cams[self.scope].fitted = false,
            }
        }
        // A drag that starts on a block moves it, on a region's label moves
        // the region with all it holds, anywhere else pans.
        if response.drag_started() {
            let at = ui.ctx().input(|i| i.pointer.press_origin());
            self.grab = Some(at.map_or(Grab::Pan, |p| self.grab_at(cam, p)));
        }
        if response.dragged() {
            let by = response.drag_delta();
            match self.grab {
                Some(Grab::Node(n)) => self.move_node(n, by / cam.zoom),
                Some(Grab::Region(r)) => self.move_region(r, by / cam.zoom),
                _ => self.cams[self.scope].offset += by,
            }
        }
        if response.drag_stopped() {
            self.grab = None;
        }
        let cam = self.cams[self.scope];
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
            match self.pick {
                // An output, an assignment or a variable's block: its path.
                Some(Pick::Node(n)) => {
                    if let Some(path) = self.tapped(n) {
                        self.because = Some(path);
                    }
                }
                // Empty space (a region's inside counts): no path.
                _ => self.because = None,
            }
        }
        let live = self.live(scan);
        self.lit = self.path(scan);
        draw(
            &painter,
            area,
            cam,
            self.diagram(),
            live.as_ref(),
            Marks {
                pick: self.pick,
                lines: &self.highlight,
                lit: self.lit.as_ref(),
            },
        );
    }

    /// What drives the "because" variable's value in `scan`, on the
    /// diagram shown: `None` with no variable or scan, or when nothing on
    /// this diagram drives it (it lives in another scope).
    #[must_use]
    pub fn path(&self, scan: Option<&dyn Scan>) -> Option<Drivers> {
        let path = self.because.as_deref()?;
        let s = &self.scopes[self.scope];
        let on = drivers(&self.module, scan?, s.pou, &s.prefix, path);
        (!on.is_empty()).then_some(on)
    }

    /// The path the last [`Self::canvas`] drew at full strength (the rest
    /// dimmed); `None`: nothing dimmed.
    #[cfg(test)]
    #[must_use]
    pub const fn highlighted(&self) -> Option<&Drivers> {
        self.lit.as_ref()
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

/// The scope that calls scope `k` (`m.t.` → `m.`, `m.` → the program).
fn parent_of(scopes: &[Scope], k: usize) -> Option<usize> {
    let prefix = scopes.get(k)?.prefix.strip_suffix('.')?;
    let up = prefix
        .rsplit_once('.')
        .map_or(String::new(), |(p, _)| format!("{p}."));
    scopes.iter().position(|s| s.prefix == up)
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

/// What [`draw`] marks on the diagram beside the scan.
#[derive(Clone, Copy)]
struct Marks<'a> {
    pick: Option<Pick>,
    /// Source lines the explanation cites.
    lines: &'a [u32],
    /// The "because" path: drawn at full strength, the rest dimmed.
    lit: Option<&'a Drivers>,
}

/// How strongly what is off the "because" path is drawn.
const DIMMED: f32 = 0.22;

/// The FBD negation bubble's radius, in diagram units.
const BUBBLE: f32 = 4.0;

/// The negation bubble on a pin at `p` (screen), entered from the left:
/// outlined in the colour of the value its consumer sees.
fn bubble(painter: &egui::Painter, p: Pos2, z: f32, seen: Option<Value>, fade: f32) {
    let (c, _) = wire_color(seen);
    painter.circle(
        p - Vec2::new(BUBBLE * z, 0.0),
        BUBBLE * z,
        BLACK,
        Stroke::new(1.5 * z.max(0.5), c.gamma_multiply(fade)),
    );
}

fn draw(painter: &egui::Painter, area: Rect, cam: Cam, d: &Diagram, live: Option<&Live>, marks: Marks<'_>) {
    let Marks { pick, lines, lit } = marks;
    let z = cam.zoom;
    let strength = |on: bool| if lit.is_none() || on { 1.0 } else { DIMMED };
    // Pins entered through a bubble: no pin dot over it.
    let bubbled: std::collections::HashSet<(NodeId, usize)> = d
        .wires
        .iter()
        .filter(|w| w.negated)
        .map(|w| (w.to, w.inp))
        .collect();
    painter.rect_filled(area, CornerRadius::ZERO, BLACK);
    let text = z > 0.35;
    // Regions, outermost first.
    for r in &d.regions {
        let gate = live.map(|l| l.gates[r.id.0 as usize]);
        let f = strength(lit.is_some_and(|l| l.region(r.id)));
        let rect = cam.rect(&r.rect);
        let (stroke, fill, state) = match gate {
            Some(Gate::Open(n)) => (YELLOW, YELLOW.gamma_multiply(0.06), format!("taken ×{n}")),
            Some(Gate::Closed) => (STEEL.gamma_multiply(0.7), Color32::TRANSPARENT, "skipped".into()),
            Some(Gate::NotReached) => (GRAPHITE, Color32::TRANSPARENT, "not reached".into()),
            None => (STEEL, Color32::TRANSPARENT, String::new()),
        };
        let (stroke, fill) = (stroke.gamma_multiply(f), fill.gamma_multiply(f));
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
        // The gate's wire into the region's label, through its bubble.
        if let Some((g, port)) = r.gate {
            let n = d.node(g);
            let (x, y) = out_port(&n.rect, port);
            let v = live.and_then(|l| l.values[g.0 as usize].get(port).copied().flatten());
            let (c, w) = wire_color(v);
            let end = cam.to_screen(r.rect.x, r.rect.y + LABEL_H / 2.0);
            let wire_end = if r.negated {
                end - Vec2::new(2.0 * BUBBLE * z, 0.0)
            } else {
                end
            };
            bezier(painter, cam.to_screen(x, y), wire_end, z, c.gamma_multiply(f), w);
            if r.negated {
                bubble(painter, end, z, live.and_then(|l| l.gate_value(d, r.id)), f);
            }
        }
    }
    // Wires, the path's last so they sit on top.
    let mut order: Vec<usize> = (0..d.wires.len()).collect();
    order.sort_by_key(|&k| lit.is_some_and(|l| l.wire(k)));
    for k in order {
        let w = &d.wires[k];
        let f = strength(lit.is_some_and(|l| l.wire(k)));
        let (a, b) = (d.node(w.from), d.node(w.to));
        let (ax, ay) = out_port(&a.rect, w.out);
        let (bx, by) = in_port(&b.rect, w.inp);
        let v = live.and_then(|l| l.wire(d, k));
        let (c, width) = wire_color(v);
        let (pa, pb) = (cam.to_screen(ax, ay), cam.to_screen(bx, by));
        if w.negated {
            bezier(
                painter,
                pa,
                pb - Vec2::new(2.0 * BUBBLE * z, 0.0),
                z,
                c.gamma_multiply(f),
                width,
            );
            bubble(painter, pb, z, live.and_then(|l| l.input(d, k)), f);
        } else {
            bezier(painter, pa, pb, z, c.gamma_multiply(f), width);
        }
        if text
            && let Some(v) = v
            && !matches!(v, Value::Bool(_))
        {
            painter.text(
                pa + Vec2::new(4.0, -7.0) * z,
                Align2::LEFT_CENTER,
                v.to_string(),
                font(10.0 * z),
                WHITE.gamma_multiply(f),
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
        let on_path = strength(lit.is_some_and(|l| l.node(n.id)));
        let (head, ink) = match &n.kind {
            NodeKind::Fb { .. } | NodeKind::For { .. } => (YELLOW, BLACK),
            NodeKind::Write { .. } => (WHITE, BLACK),
            NodeKind::Read { .. } | NodeKind::Const { .. } => (GRAPHITE, WHITE),
            NodeKind::Op { .. } | NodeKind::Func { .. } | NodeKind::Access { .. } => {
                (Color32::from_rgb(0x55, 0x55, 0x55), WHITE)
            }
            NodeKind::Jump => (STEEL, BLACK),
        };
        let fade: f32 = if ran { 1.0 } else { 0.4 } * on_path;
        let radius = CornerRadius::same(4);
        painter.rect_filled(r, radius, PANEL.gamma_multiply(fade.max(0.7) * on_path));
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
                Stroke::new(2.0 * z, YELLOW.gamma_multiply(0.6 * on_path)),
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
                if !bubbled.contains(&(n.id, k)) {
                    painter.circle_filled(p, 3.0 * z, STEEL.gamma_multiply(on_path));
                }
                if !name.is_empty() {
                    painter.text(
                        p + Vec2::new(6.0 * z, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        font(10.0 * z),
                        STEEL.gamma_multiply(on_path),
                    );
                }
            }
            for (k, name) in n.outs.iter().enumerate() {
                let (x, y) = out_port(&n.rect, k);
                let p = cam.to_screen(x, y);
                let v = live.and_then(|l| l.values[n.id.0 as usize].get(k).copied().flatten());
                painter.circle_filled(p, 3.5 * z, wire_color(v).0.gamma_multiply(on_path));
                if !name.is_empty() {
                    painter.text(
                        p - Vec2::new(6.0 * z, 0.0),
                        Align2::RIGHT_CENTER,
                        name,
                        font(10.0 * z),
                        STEEL.gamma_multiply(on_path),
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
                    (if on { YELLOW } else { WHITE }).gamma_multiply(on_path),
                );
            }
        }
    }
}

/// The "because" path, driven headlessly through egui: taps on the
/// diagram, asserting on what the canvas drew at full strength.
#[cfg(test)]
mod path_tests {
    use eframe::egui::{self, Event, PointerButton, Pos2, RawInput, Rect, Vec2};
    use helmsman_diagram::NodeKind;
    use helmsman_iec::Value;

    use super::{Scan, Viewer};
    use crate::playground::Session;

    /// The playground's latch, with two unrelated statements.
    const LATCH: &str = "PROGRAM latch
VAR
  start AT %IX0.0 : BOOL;
  stop AT %IX0.1 : BOOL;
  motor AT %QX0.0 : BOOL;
  other AT %IX0.2 : BOOL;
  lamp AT %QX0.1 : BOOL;
  count : INT;
END_VAR
motor := (start OR motor) AND NOT stop;
lamp := other;
IF other THEN count := count + 1; END_IF;
END_PROGRAM
";

    /// The simulation's session (stopped, stepped by hand) and its view.
    struct Sim {
        session: Session,
        view: Viewer,
    }

    struct Harness {
        ctx: egui::Context,
        sv: Sim,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let mut session = Session::new(LATCH);
            assert!(session.set_input("start", Value::Bool(true)));
            for _ in 0..3 {
                session.step();
            }
            let module = session.build.as_ref().unwrap().module.clone().unwrap();
            let mut view = Viewer::new(LATCH, module);
            // Something not on this diagram, as a box's first output can be.
            view.because = Some("CONVEYOR".into());
            let mut h = Self {
                ctx: egui::Context::default(),
                sv: Sim { session, view },
                time: 0.0,
            };
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let Sim { view, session } = &mut self.sv;
            let scan = session.trace.as_ref().map(|t| t as &dyn Scan);
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::no_frame().show(ui, |ui| view.canvas(ui, scan));
            });
            out.textures_delta.clear();
        }

        fn click(&mut self, at: Pos2) {
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.frame(vec![Event::PointerMoved(at)]);
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
            // Far enough apart not to make a double click.
            self.time += 1.0;
            self.frame(vec![]);
        }

        /// The screen centre of the first node of kind `k`.
        fn at(&self, k: &NodeKind) -> Pos2 {
            let v = &self.sv.view;
            let n = v.diagram().nodes.iter().find(|n| n.kind == *k).unwrap();
            v.cams[v.scope].to_screen(n.rect.x + n.rect.w / 2.0, n.rect.y + n.rect.h / 2.0)
        }

        /// The nodes drawn at full strength, by label.
        fn lit(&self) -> Vec<String> {
            let v = &self.sv.view;
            let Some(on) = v.highlighted() else {
                return Vec::new();
            };
            v.diagram()
                .nodes
                .iter()
                .filter(|n| on.node(n.id))
                .map(|n| n.label.clone())
                .collect()
        }
    }

    fn write(path: &str) -> NodeKind {
        NodeKind::Write { path: path.into() }
    }

    #[test]
    fn tapping_an_output_dims_all_but_its_path_and_empty_space_clears_it() {
        let mut h = Harness::new();
        assert_eq!(
            h.sv.view.because.as_deref(),
            Some("CONVEYOR"),
            "a variable elsewhere"
        );
        assert!(
            h.sv.view.highlighted().is_none(),
            "not on this diagram: nothing dimmed"
        );

        h.click(h.at(&write("MOTOR")));
        assert_eq!(h.sv.view.because.as_deref(), Some("MOTOR"));
        assert_eq!(
            h.lit(),
            ["START", "MOTOR", "OR", "STOP", "AND", "MOTOR :="],
            "start, stop and the seal-in"
        );
        let v = &h.sv.view;
        let on = v.highlighted().unwrap();
        let d = v.diagram();
        for n in &d.nodes {
            let unrelated = n.pos.line > 10;
            assert_eq!(on.node(n.id), !unrelated, "{} on line {}", n.label, n.pos.line);
        }
        assert!(!on.region(d.regions[0].id), "the IF is dimmed");
        let negated = d.wires.iter().position(|w| w.negated).unwrap();
        assert!(on.wire(negated), "the bubbled STOP wire is on the path");

        // A read's block: that variable's path.
        h.click(h.at(&NodeKind::Read { path: "OTHER".into() }));
        assert_eq!(h.sv.view.because.as_deref(), Some("OTHER"));
        assert_eq!(h.lit(), ["OTHER", "OTHER"]);

        // Empty space, far from every block: cleared, nothing dimmed.
        let v = &h.sv.view;
        let (w, hgt) = v.diagram().size;
        let empty = v.cams[v.scope].to_screen(w + 60.0, hgt + 60.0);
        h.click(empty);
        assert_eq!(h.sv.view.because, None);
        assert!(h.sv.view.highlighted().is_none());
        assert_eq!(h.lit(), Vec::<String>::new());
    }
}
