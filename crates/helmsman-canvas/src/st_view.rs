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
use helmsman_diagram::lower::Var;
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
    /// TRUE, running: green.
    pub const GREEN: Color32 = Color32::from_rgb(0x3C, 0xC8, 0x5C);
    /// FALSE, stopped: red.
    pub const RED: Color32 = Color32::from_rgb(0xFF, 0x6E, 0x64);
    /// A switch that is off: a dark red ground.
    pub const OFF_RED: Color32 = Color32::from_rgb(0x6A, 0x22, 0x1E);
}

use brand::{BLACK, GRAPHITE, PANEL, STEEL, WHITE, YELLOW};

/// How much of the diagram a pan must leave in view, points.
pub const KEEP: f32 = 120.0;
/// The smallest zoom a fit goes to: a block's label (11.5 units of type)
/// still reads at about 9 px. A diagram too big to fit at that zoom shows
/// its left part and you pan, instead of shrinking to an unreadable whole
/// (Nick, 2026-11-01).
pub const LEGIBLE: f32 = 0.75;
/// The band along the diagram's top the glyph buttons sit in, points: a
/// fit keeps the diagram below it.
pub const GLYPH_BAND: f32 = 52.0;

/// Pan and zoom: `screen = origin + offset + world * zoom`.
#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub offset: Vec2,
    pub zoom: f32,
    origin: Pos2,
    fitted: bool,
    /// The operator panned or zoomed since the last fit: a change of the
    /// view's size then keeps their framing (clamped) instead of refitting.
    moved: bool,
    /// The size of the area last drawn in.
    area: Vec2,
}

impl Default for Cam {
    fn default() -> Self {
        Self {
            offset: Vec2::new(30.0, 30.0),
            zoom: 1.0,
            origin: Pos2::ZERO,
            fitted: false,
            moved: false,
            area: Vec2::ZERO,
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
        self.moved = true;
        let (wx, wy) = self.to_world(screen);
        self.zoom = (self.zoom * factor).clamp(0.15, 3.0);
        self.offset = screen - self.origin - Vec2::new(wx, wy) * self.zoom;
    }

    /// Keeps the diagram in view: however far a pan or zoom went, at
    /// least [`KEEP`] points of it stay inside `area` on each side (Nick,
    /// 2026-11-01: no dragging it off into half a screen of nothing).
    fn clamp(&mut self, area: Rect, size: (f32, f32)) {
        let (w, h) = (size.0.max(1.0) * self.zoom, size.1.max(1.0) * self.zoom);
        let keep = KEEP.min(w / 2.0).min(h / 2.0);
        let left = self.origin.x + self.offset.x;
        let top = self.origin.y + self.offset.y;
        let x = left.clamp(area.left() + keep - w, area.right() - keep);
        let y = top.clamp(area.top() + keep - h, area.bottom() - keep);
        self.offset += Vec2::new(x - left, y - top);
    }

    fn fit(&mut self, area: Rect, size: (f32, f32)) {
        let (w, h) = (size.0.max(1.0) + 60.0, size.1.max(1.0) + 60.0);
        self.zoom = (area.width() / w).min(area.height() / h).clamp(LEGIBLE, 1.2);
        self.origin = area.min;
        self.offset = Vec2::splat(30.0 * self.zoom);
        self.fitted = true;
        self.moved = false;
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
    /// When each node (scope, id) was last seen to run, page time: a
    /// statement that ran for one scan stays lit for [`HOLD`].
    ran_at: std::collections::HashMap<(usize, u32), f64>,
    /// When each region (scope, id) was last seen taken, and how.
    taken_at: std::collections::HashMap<(usize, u32), (f64, Gate)>,
    /// The variable (upper-cased head name in this scope) whose reads and
    /// writes are lit instead of the "because" path: Cross references.
    xref: Option<String>,
    /// Instances folded in the tree (their prefixes).
    folded: Vec<String>,
    /// How far the operator moved each block from where the layout put it,
    /// per POU. The view's own; the program is untouched.
    moved: Vec<Vec<(NodeId, Vec2)>>,
    /// Counts changes to `moved` (see [`Self::edits`]).
    edits: u64,
    /// The blocks' one-character icons (the project's `icons.map` over
    /// the built-in ones).
    pub icons: crate::icons::Icons,
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

/// Offsets closer to zero than this are no move (float residue).
const STILL: f32 = 0.01;

impl Arrangement {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many blocks it moves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Read from a project's `diagram.layout` ([`helmsman_project::layout`]).
    ///
    /// # Errors
    /// What [`helmsman_project::layout::parse`] refuses.
    pub fn from_text(text: &str) -> Result<Self, Vec<String>> {
        Ok(Self(
            helmsman_project::layout::parse(text)?
                .into_iter()
                .map(|m| (m.pou, m.label, m.text, m.k, Vec2::new(m.dx, m.dy)))
                .collect(),
        ))
    }

    /// As a `diagram.layout` file, the moves in a stable order.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut moves: Vec<helmsman_project::layout::Move> = self
            .0
            .iter()
            .map(|(pou, label, text, k, v)| helmsman_project::layout::Move {
                pou: pou.clone(),
                label: label.clone(),
                text: text.clone(),
                k: *k,
                dx: v.x,
                dy: v.y,
            })
            .collect();
        moves.sort_by(|a, b| (&a.pou, &a.label, &a.text, a.k).cmp(&(&b.pou, &b.label, &b.text, b.k)));
        helmsman_project::layout::render(&moves)
    }

    /// The offset `self` gives each block, less what `base` gives it: the
    /// moves to make on top of `base` to get `self`. A block `base` moves
    /// and `self` does not is moved back.
    #[must_use]
    pub fn minus(&self, base: &Self) -> Self {
        let key = |e: &(String, String, String, usize, Vec2)| (e.0.clone(), e.1.clone(), e.2.clone(), e.3);
        let mut total: std::collections::BTreeMap<(String, String, String, usize), Vec2> =
            std::collections::BTreeMap::new();
        for e in &self.0 {
            *total.entry(key(e)).or_default() += e.4;
        }
        for e in &base.0 {
            *total.entry(key(e)).or_default() -= e.4;
        }
        Self(
            total
                .into_iter()
                .filter(|(_, v)| v.x.abs() >= STILL || v.y.abs() >= STILL)
                .map(|((p, l, t, k), v)| (p, l, t, k, v))
                .collect(),
        )
    }

    /// Only the moves of blocks `v` has.
    #[must_use]
    pub fn within(&self, v: &Viewer) -> Self {
        Self(
            self.0
                .iter()
                .filter(|(p, l, t, k, _)| v.block(p, l, t, *k).is_some())
                .cloned()
                .collect(),
        )
    }
}

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
            ran_at: std::collections::HashMap::new(),
            taken_at: std::collections::HashMap::new(),
            xref: None,
            folded: Vec::new(),
            moved,
            edits: 0,
            icons: crate::icons::Icons::default(),
        }
    }

    /// The variable of this scope the picked block is about: the head of
    /// a read, write or access path (`M` for `M.ON`, `A` for `A[I]`), or
    /// a block instance's name. Upper-cased, as the POU's `vars` are.
    #[must_use]
    pub fn picked_var(&self) -> Option<String> {
        let Some(Pick::Node(n)) = self.pick else {
            return None;
        };
        let node = self.diagram().nodes.get(n.0 as usize)?;
        let head = match &node.kind {
            NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => {
                path.split(['.', '[']).next()?
            }
            NodeKind::Fb { instance, .. } => instance.as_str(),
            _ => return None,
        };
        (!head.starts_with('%')).then(|| head.to_ascii_uppercase())
    }

    /// The source line where the picked block's variable is declared
    /// (Go to definition).
    #[must_use]
    pub fn definition_line(&self) -> Option<u32> {
        let head = self.picked_var()?;
        let pou = self.module.pous.get(self.scopes[self.scope].pou)?;
        pou.vars.get(&head).map(|v| v.line)
    }

    /// Lights every block that reads or writes the picked block's
    /// variable, in place of the "because" path, until the next click
    /// (Cross references). Whether there was a variable to show.
    pub fn cross_references(&mut self) -> bool {
        let Some(head) = self.picked_var() else {
            return false;
        };
        self.xref = Some(head);
        true
    }

    /// The reads and writes of `head` on this diagram, with the wires
    /// between them.
    fn references(&self, head: &str) -> Drivers {
        let d = self.diagram();
        let of = |n: &helmsman_diagram::Node| match &n.kind {
            NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => path
                .split(['.', '['])
                .next()
                .is_some_and(|h| h.eq_ignore_ascii_case(head)),
            NodeKind::Fb { instance, .. } => instance.eq_ignore_ascii_case(head),
            _ => false,
        };
        let nodes: Vec<bool> = d.nodes.iter().map(of).collect();
        let wires = d
            .wires
            .iter()
            .map(|w| nodes[w.from.0 as usize] && nodes[w.to.0 as usize])
            .collect();
        Drivers {
            nodes,
            wires,
            regions: vec![false; d.regions.len()],
        }
    }

    /// Fits the open level to its area on the next frame (View menu).
    pub fn refit(&mut self) {
        self.cams[self.scope].fitted = false;
    }

    /// Keeps what ran lit for [`HOLD`] after the scan it ran in. The
    /// diagram draws one scan per frame, so a statement that runs for a
    /// single scan (the body of `IF pulse.Q`, a one-shot) would brighten
    /// for one frame, only on the frames that happen to land on it: a
    /// flicker. Held, it reads as a blink of fixed length instead.
    pub fn hold(&mut self, live: &mut Live, now: f64) {
        let scope = self.scope;
        for (i, ran) in live.ran.iter_mut().enumerate() {
            let key = (scope, u32::try_from(i).unwrap_or(u32::MAX));
            if *ran {
                self.ran_at.insert(key, now);
            } else if self.ran_at.get(&key).is_some_and(|t| now - t < HOLD) {
                *ran = true;
            }
        }
        for (i, g) in live.gates.iter_mut().enumerate() {
            let key = (scope, u32::try_from(i).unwrap_or(u32::MAX));
            if matches!(g, Gate::Open(_)) {
                self.taken_at.insert(key, (now, *g));
            } else if let Some((t, held)) = self.taken_at.get(&key)
                && now - t < HOLD
            {
                *g = *held;
            }
        }
    }

    /// Counts every change to the arrangement (a drag step, an
    /// [`Self::arrange`], a reset): a new count means something moved.
    #[must_use]
    pub const fn edits(&self) -> u64 {
        self.edits
    }

    /// A block or region is being dragged.
    #[must_use]
    pub const fn dragging(&self) -> bool {
        matches!(self.grab, Some(Grab::Node(_) | Grab::Region(_)))
    }

    /// Block `k` of those labelled `label` with text `text` in POU `pou`.
    #[must_use]
    pub fn block(&self, pou: &str, label: &str, text: &str, k: usize) -> Option<(usize, NodeId)> {
        let p = self.module.pous.iter().position(|q| q.name == pou)?;
        self.module.pous[p]
            .diagram
            .nodes
            .iter()
            .filter(|m| m.label == label && m.text == text)
            .nth(k)
            .map(|m| (p, m.id))
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
        self.edits += 1;
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
        self.edits += 1;
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
        self.edits += 1;
        for (name, label, text, k, v) in &a.0 {
            if let Some((pou, n)) = self.block(name, label, text, *k) {
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
        // The view changed size (a pane opened, the window resized) and
        // the operator had not panned or zoomed: fit again, or the
        // diagram's right part slides under a panel.
        if cam.fitted && !cam.moved && cam.area != area.size() {
            cam.fitted = false;
        }
        cam.area = area.size();
        if !cam.fitted {
            // Fit under the glyph strip along the top, so no block opens
            // under it.
            let below = Rect::from_min_max(area.min + Vec2::new(0.0, GLYPH_BAND), area.max);
            cam.fit(below, size);
            cam.offset.y += GLYPH_BAND;
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
                cam.moved = true;
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
                _ => {
                    let cam = &mut self.cams[self.scope];
                    cam.offset += by;
                    cam.moved = true;
                }
            }
        }
        if response.drag_stopped() {
            self.grab = None;
        }
        let size = self.diagram().size;
        self.cams[self.scope].clamp(area, size);
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
            // A click ends a cross-reference view.
            self.xref = None;
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
        let mut live = self.live(scan);
        if let Some(l) = &mut live {
            let now = ui.input(|i| i.time);
            self.hold(l, now);
        }
        self.lit = self
            .xref
            .as_ref()
            .map(|h| self.references(h))
            .filter(|r| !r.is_empty())
            .or_else(|| self.path(scan));
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
                vars: self.module.program().map(|p| &p.vars),
                icons: &self.icons,
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
    /// The POU's variables, by upper-cased name: how a value prints
    /// (`T#500ms` for a TIME) and which are constants (no value shown).
    vars: Option<&'a std::collections::HashMap<String, Var>>,
    /// The blocks' icons.
    icons: &'a crate::icons::Icons,
}

/// The program variable a node's path starts at (`M.ON` is `M`'s).
fn var_of<'a>(vars: Option<&'a std::collections::HashMap<String, Var>>, path: &str) -> Option<&'a Var> {
    let head = path.split(['.', '[']).next().unwrap_or(path).to_ascii_uppercase();
    vars?.get(&head)
}

/// What a node's value is, as it should read: a whole TIME variable's
/// in a time literal's unit, anything else as the value prints. `None`:
/// the node is a literal or a constant, whose label says it all.
fn value_text(
    d: &Diagram,
    n: NodeId,
    v: Value,
    vars: Option<&std::collections::HashMap<String, Var>>,
) -> Option<String> {
    let node = d.node(n);
    let path = match &node.kind {
        NodeKind::Read { path } | NodeKind::Write { path } | NodeKind::Access { path } => path,
        NodeKind::Const { .. } => return None,
        _ => return Some(v.to_string()),
    };
    let var = var_of(vars, path);
    if var.is_some_and(|x| x.constant) {
        return None;
    }
    match v {
        Value::Int(ns) if !path.contains(['.', '[']) && var.is_some_and(|x| x.ty == "TIME") => {
            Some(helmsman_diagram::text::time(ns))
        }
        v => Some(v.to_string()),
    }
}

/// How strongly what is off the "because" path is drawn.
const DIMMED: f32 = 0.22;

/// A wire between statements longer than this (diagram units) is drawn
/// as a tunnel: a tagged stub at each end, no lead across the diagram.
pub const TUNNEL_AT: f32 = 260.0;

/// Whether the wire from `a` to `b`, `len` units long, is a tunnel.
#[must_use]
pub fn tunnel(a: &helmsman_diagram::Node, b: &helmsman_diagram::Node, len: f32) -> bool {
    a.stmt != b.stmt && len > TUNNEL_AT
}

/// What a tunnel's tags say: the variable the source writes or reads, or
/// a block's `instance.output`.
#[must_use]
pub fn source_name(a: &helmsman_diagram::Node, out: usize) -> String {
    match &a.kind {
        NodeKind::Write { path } | NodeKind::Read { path } | NodeKind::Access { path } => {
            path.to_ascii_lowercase()
        }
        NodeKind::Fb { instance, .. } => {
            let port = a.outs.get(out).map_or("", String::as_str);
            format!("{}.{}", instance.to_ascii_lowercase(), port.to_ascii_lowercase())
        }
        _ => a.label.to_ascii_lowercase(),
    }
}

/// How long a statement stays lit after the scan it ran in, seconds.
pub const HOLD: f64 = 0.3;

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
    let Marks {
        pick,
        lines,
        lit,
        vars,
        icons,
    } = marks;
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
            let (x, y) = out_port(n, port);
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
        let (ax, ay) = out_port(a, w.out);
        let (bx, by) = in_port(b, w.inp);
        let v = live.and_then(|l| l.wire(d, k));
        let (c, width) = wire_color(v);
        let (pa, pb) = (cam.to_screen(ax, ay), cam.to_screen(bx, by));
        let end = if w.negated {
            pb - Vec2::new(2.0 * BUBBLE * z, 0.0)
        } else {
            pb
        };
        if tunnel(a, b, (end - pa).length() / z) {
            // Too far to trace: a tagged stub at each end naming the signal.
            let name = source_name(a, w.out);
            let stub = 14.0 * z;
            let stroke = Stroke::new(width * z.max(0.5), c.gamma_multiply(f));
            painter.line_segment([pa, pa + Vec2::new(stub, 0.0)], stroke);
            painter.text(
                pa + Vec2::new(stub + 3.0 * z, 0.0),
                Align2::LEFT_CENTER,
                format!("{name} ▸"),
                font(9.5 * z),
                c.gamma_multiply(f),
            );
            painter.line_segment([end - Vec2::new(stub, 0.0), end], stroke);
            painter.text(
                end - Vec2::new(stub + 3.0 * z, 0.0),
                Align2::RIGHT_CENTER,
                format!("▸ {name}"),
                font(9.5 * z),
                c.gamma_multiply(f),
            );
        } else {
            bezier(painter, pa, end, z, c.gamma_multiply(f), width);
        }
        if w.negated {
            bubble(painter, pb, z, live.and_then(|l| l.input(d, k)), f);
        }
        if text
            && let Some(v) = v
            && !matches!(v, Value::Bool(_))
            && let Some(label) = value_text(d, w.from, v, vars)
        {
            painter.text(
                pa + Vec2::new(4.0, -7.0) * z,
                Align2::LEFT_CENTER,
                label,
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
        let h = Rect::from_min_size(r.min, Vec2::new(r.width(), HEADER * z));
        // A block that shows a value under its header (a variable) or
        // keeps its instance's ports apart (a function block) has a body
        // of its own; an operator, function or literal is one shade.
        let two_tone = n.kind.headed();
        if two_tone {
            painter.rect_filled(r, radius, PANEL.gamma_multiply(fade.max(0.7) * on_path));
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
        } else {
            painter.rect_filled(r, radius, head.gamma_multiply(fade));
        }
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
        // The block's one-character icon: at the header's left edge beside
        // the label, or alone and large enough to read when the label is
        // too small to draw (Nick, 2026-11-01).
        let icon = icons.of(n);
        if !text && let Some(i) = icon {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                i,
                font((14.0 * z).max(11.0)),
                ink.gamma_multiply(fade),
            );
        }
        if text {
            let band = if two_tone { h } else { r };
            // With an icon the label sits right of it, so the two never
            // overlap on a block sized to its label alone.
            let mut centre = band.center();
            if let Some(i) = icon {
                painter.text(
                    Pos2::new(band.left() + 3.0 * z, band.center().y),
                    Align2::LEFT_CENTER,
                    i,
                    font(11.5 * z),
                    ink.gamma_multiply(fade),
                );
                centre.x += 6.0 * z;
            }
            painter.text(
                centre,
                Align2::CENTER_CENTER,
                &n.label,
                font(11.5 * z),
                ink.gamma_multiply(fade),
            );
            for (k, name) in n.ins.iter().enumerate() {
                let (x, y) = in_port(n, k);
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
                let (x, y) = out_port(n, k);
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
            // A variable's value in its body (a constant's is its label).
            if matches!(n.kind, NodeKind::Read { .. } | NodeKind::Write { .. })
                && let Some(v) = live.and_then(|l| l.values[n.id.0 as usize].first().copied().flatten())
                && let Some(label) = value_text(d, n.id, v, vars)
            {
                let color = match v {
                    Value::Bool(true) => brand::GREEN,
                    Value::Bool(false) => brand::RED,
                    _ => WHITE,
                };
                painter.text(
                    Pos2::new(r.center().x, h.max.y + (r.max.y - h.max.y) / 2.0),
                    Align2::CENTER_CENTER,
                    label,
                    font(11.0 * z),
                    color.gamma_multiply(on_path),
                );
            }
        }
    }
}

/// A one-scan statement stays lit (the Blink lamp's flicker).
#[cfg(test)]
mod hold_tests {
    use helmsman_diagram::{Gate, Live, NodeKind};

    use super::{HOLD, Viewer};
    use crate::playground::Session;

    const SRC: &str = "PROGRAM blink
VAR_INPUT
  enable : BOOL := TRUE;
  period : TIME := T#500ms;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
pulse(IN := enable AND NOT pulse.Q, PT := period);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
END_PROGRAM
";

    #[test]
    fn a_statement_that_ran_one_scan_stays_lit_for_hold_then_dims() {
        let session = Session::new(SRC);
        assert!(session.diags.is_empty(), "{:?}", session.diag_lines());
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut v = Viewer::new(SRC, module);
        let d = v.diagram().clone();
        let lamp = d
            .nodes
            .iter()
            .position(|n| n.kind == NodeKind::Write { path: "LAMP".into() })
            .unwrap();
        // The program's one region: the IF the lamp's assignment sits in.
        assert_eq!(d.regions.len(), 1);
        let region = 0;
        let fresh = |ran: bool, gate: Gate| Live {
            values: d.nodes.iter().map(|_| Vec::new()).collect(),
            ran: d
                .nodes
                .iter()
                .enumerate()
                .map(|(i, _)| i != lamp || ran)
                .collect(),
            gates: d
                .regions
                .iter()
                .enumerate()
                .map(|(i, _)| if i == region { gate } else { Gate::Closed })
                .collect(),
        };
        // The scan it ran in: lit, the IF taken.
        let mut l = fresh(true, Gate::Open(1));
        v.hold(&mut l, 10.0);
        assert!(l.ran[lamp] && l.gates[region] == Gate::Open(1));
        // The next frames show scans it did not run in: still lit, within HOLD.
        let mut l = fresh(false, Gate::Closed);
        v.hold(&mut l, 10.0 + HOLD / 2.0);
        assert!(l.ran[lamp], "held");
        assert_eq!(l.gates[region], Gate::Open(1), "the IF stays taken");
        // After HOLD: as the scan says.
        let mut l = fresh(false, Gate::Closed);
        v.hold(&mut l, 10.0 + HOLD + 0.01);
        assert!(!l.ran[lamp], "dimmed again");
        assert_eq!(l.gates[region], Gate::Closed);
        // Never reached: nothing to hold.
        let mut l = fresh(false, Gate::NotReached);
        v.hold(&mut l, 20.0);
        assert!(!l.ran[lamp]);
        assert_eq!(l.gates[region], Gate::NotReached);
    }
}

/// What a block's value reads as (cards `period` value, node fills).
#[cfg(test)]
mod value_tests {
    use helmsman_diagram::NodeKind;
    use helmsman_iec::Value;

    use super::{Viewer, value_text};
    use crate::playground::Session;

    const SRC: &str = "PROGRAM clutter
VAR_INPUT
  period : TIME := T#500ms;
  n : INT := 7;
END_VAR
VAR CONSTANT
  k : INT := 3;
END_VAR
VAR
  pulse : TON;
  m : INT;
END_VAR
pulse(IN := TRUE, PT := period);
m := n + k + 2;
END_PROGRAM
";

    fn viewer() -> Viewer {
        let session = Session::new(SRC);
        assert!(session.diags.is_empty(), "{:?}", session.diag_lines());
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        Viewer::new(SRC, module)
    }

    fn node(v: &Viewer, k: &NodeKind) -> helmsman_diagram::NodeId {
        v.diagram().nodes.iter().find(|n| n.kind == *k).unwrap().id
    }

    #[test]
    fn a_time_reads_as_a_literal_a_constant_shows_nothing_and_an_int_is_itself() {
        let v = viewer();
        let d = v.diagram();
        let vars = v.module.program().map(|p| &p.vars);
        let read = |p: &str| node(&v, &NodeKind::Read { path: p.into() });
        assert_eq!(
            value_text(d, read("PERIOD"), Value::Int(500_000_000), vars).as_deref(),
            Some("T#500ms"),
            "a TIME in its unit, not nanoseconds"
        );
        assert_eq!(
            value_text(d, read("N"), Value::Int(7), vars).as_deref(),
            Some("7")
        );
        assert_eq!(
            value_text(d, read("K"), Value::Int(3), vars),
            None,
            "a VAR CONSTANT shows no value"
        );
        let lit = node(
            &v,
            &NodeKind::Const {
                value: Some(Value::Int(2)),
            },
        );
        assert_eq!(
            value_text(d, lit, Value::Int(2), vars),
            None,
            "a literal is its label"
        );
        let plus = node(&v, &NodeKind::Op { op: "+".into() });
        assert_eq!(value_text(d, plus, Value::Int(12), vars).as_deref(), Some("12"));
        assert_eq!(
            value_text(d, read("N"), Value::Int(7), None).as_deref(),
            Some("7"),
            "no declarations known: as the value prints"
        );
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

    /// Edit › Go to definition and Cross references (hotkey cards,
    /// 2026-11-01): the picked block's variable, its VAR line, and every
    /// read and write of it lit until the next click.
    #[test]
    fn a_picked_block_knows_its_declaration_and_lights_its_references() {
        let mut h = Harness::new();
        assert_eq!(h.sv.view.picked_var(), None, "nothing picked");
        assert_eq!(h.sv.view.definition_line(), None);
        assert!(!h.sv.view.cross_references());
        h.click(h.at(&write("MOTOR")));
        assert_eq!(h.sv.view.picked_var().as_deref(), Some("MOTOR"));
        assert_eq!(h.sv.view.definition_line(), Some(5), "motor AT %QX0.0 is line 5");
        assert!(h.sv.view.cross_references());
        h.frame(vec![]);
        let lit = h.lit();
        assert!(lit.contains(&"MOTOR".to_string()), "the read of motor: {lit:?}");
        assert!(
            lit.iter().any(|l| l.starts_with("MOTOR :=")),
            "its write: {lit:?}"
        );
        assert!(
            !lit.iter().any(|l| l.contains("OTHER")),
            "not another variable: {lit:?}"
        );
        // A click anywhere ends it; the because path is back.
        h.click(h.at(&write("LAMP")));
        h.frame(vec![]);
        assert_eq!(h.sv.view.picked_var().as_deref(), Some("LAMP"));
        assert_eq!(h.sv.view.definition_line(), Some(7));
        let lit = h.lit();
        assert!(!lit.iter().any(|l| l.starts_with("MOTOR :=")), "{lit:?}");
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

        // Empty space, far from every block (the view's bottom-right
        // corner: a fit leaves margins there): cleared, nothing dimmed.
        let empty = Pos2::new(1190.0, 790.0);
        assert_eq!(
            h.sv.view.diagram().node_at(
                h.sv.view.cams[h.sv.view.scope].to_world(empty).0,
                h.sv.view.cams[h.sv.view.scope].to_world(empty).1
            ),
            None,
            "the corner is empty"
        );
        h.click(empty);
        assert_eq!(h.sv.view.because, None);
        assert!(h.sv.view.highlighted().is_none());
        assert_eq!(h.lit(), Vec::<String>::new());
    }
}
