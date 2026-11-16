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
use helmsman_diagram::fold::{Fold, FoldKind, Origin, View, structure};
use helmsman_diagram::layout::{HEADER, LABEL_H, in_port, out_port};
use helmsman_diagram::lower::Var;
use helmsman_diagram::{
    Diagram, Drivers, Gate, Live, Module, NodeId, NodeKind, RegionId, Scan, annotate, drivers,
};
use helmsman_iec::Value;
use helmsman_iec::trace::StmtId;

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

use brand::{BLACK, STEEL, YELLOW};

use crate::theme::{Theme, theme};
use crate::tips::Tip;

/// How much of the diagram a pan must leave in view, points.
pub const KEEP: f32 = 120.0;
/// The smallest zoom a fit goes to: a block's label (11.5 units of type)
/// still reads at about 9 px. A diagram too big to fit at that zoom shows
/// its left part and you pan, instead of shrinking to an unreadable whole
/// (Nick, 2026-11-01).
pub const LEGIBLE: f32 = 0.75;
/// The largest zoom a fit goes to: a port's name (10 units of type) is
/// then the 16 pt the rest of the app holds to ([`crate::theme::MIN_TEXT`])
/// and a block's label 18. At the 1.2 it was, a small program sat in a
/// sixth of the canvas with 12 pt port names (2026-11-10).
pub const FIT_MAX: f32 = 1.6;
/// Points of wheel or two-finger scroll that zoom the diagram by e
/// (egui's own figure for a Ctrl scroll).
pub const SCROLL_ZOOM: f32 = 200.0;
/// How fast a held arrow key (or W A S D) moves the view, points a second.
pub const KEY_PAN: f32 = 900.0;
/// The band along the diagram's top the glyph buttons sit in, points: a
/// fit keeps the diagram below it.
pub const GLYPH_BAND: f32 = 52.0;
/// The largest zoom a Q or E jump goes to ([`crate::jump`]): one function
/// block with its arguments fills a part of the view and keeps its
/// neighbours around it, instead of a timer as large as the window.
pub const FOCUS_MAX: f32 = 2.0;
/// The room a jump leaves around what it frames, diagram units.
pub const FOCUS_PAD: f32 = 40.0;
/// How long a jump takes to ease the view over, seconds: long enough to
/// see which way it went.
pub const GLIDE: f64 = 0.18;

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
        self.zoom = (area.width() / w).min(area.height() / h).clamp(LEGIBLE, FIT_MAX);
        self.origin = area.min;
        self.offset = Vec2::splat(30.0 * self.zoom);
        self.fitted = true;
        self.moved = false;
    }

    /// Frames `r` in `area` with [`FOCUS_PAD`] around it, as close as
    /// [`FOCUS_MAX`] lets it: in the middle, or from its top left corner
    /// along a side that does not fit at the smallest zoom (an IF larger
    /// than the view shows its label and condition).
    fn frame(&mut self, area: Rect, r: &helmsman_diagram::Rect) {
        let pad = 2.0 * FOCUS_PAD;
        let zoom = (area.width() / (r.w + pad))
            .min(area.height() / (r.h + pad))
            .clamp(LEGIBLE, FOCUS_MAX);
        let along = |lo: f32, hi: f32, at: f32, len: f32| {
            let start = if (len + pad) * zoom > hi - lo {
                lo + FOCUS_PAD * zoom
            } else {
                f32::midpoint(lo, hi) - len * zoom / 2.0
            };
            start - at * zoom
        };
        self.zoom = zoom;
        self.offset = Vec2::new(
            along(area.left(), area.right(), r.x, r.w),
            along(area.top(), area.bottom(), r.y, r.h),
        ) - self.origin.to_vec2();
        self.moved = true;
    }

    /// Whether the view is where `to` (an offset and a zoom) put it.
    fn rests_at(self, to: (Vec2, f32)) -> bool {
        (self.offset - to.0).length() < 0.5 && (self.zoom - to.1).abs() < 1e-3
    }
}

/// A Q or E jump under way: the view of scope `scope` eases from one
/// offset and zoom to another, from page time `t0` for [`GLIDE`].
#[derive(Clone, Copy, Debug)]
struct Glide {
    scope: usize,
    from: (Vec2, f32),
    to: (Vec2, f32),
    t0: f64,
}

impl Glide {
    /// The offset and zoom at page time `now`, and whether that is the
    /// end. The zoom changes by equal factors and the diagram point in
    /// the middle of the view (`half` its size) moves in a straight line.
    fn at(&self, now: f64, half: Vec2) -> ((Vec2, f32), bool) {
        let s = ((now - self.t0) / GLIDE).clamp(0.0, 1.0);
        if s >= 1.0 {
            return (self.to, true);
        }
        #[allow(clippy::cast_possible_truncation)]
        let s = s as f32;
        let e = s * s * (3.0 - 2.0 * s);
        let zoom = self.from.1 * (self.to.1 / self.from.1).powf(e);
        let world = |(offset, zoom): (Vec2, f32)| (half - offset) / zoom;
        let (a, b) = (world(self.from), world(self.to));
        ((half - (a + (b - a) * e) * zoom, zoom), false)
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
    /// Breakpoints to mark, as the host keeps them (the simulation's).
    pub breaks: Vec<StmtId>,
    /// The statement a paused scan stopped before.
    pub paused: Option<StmtId>,
    /// The program as the diagram showed it when the Agent's last change
    /// landed ([`crate::agent_marks`]): what that change added is outlined
    /// green and what it removed drawn as red ghosts, against it. The host
    /// sets it each frame and clears it at the engineer's next edit.
    pub baseline: Option<std::rc::Rc<Module>>,
    /// The diff of `baseline` to `module`, kept while neither changes:
    /// (the baseline's address, the source it was made against, the diff).
    change: Option<(usize, String, helmsman_diagram::diff::ModuleDiff)>,
    /// Whether the diagram's gestures change the program (the host sets
    /// it each frame): off unless it says why not ([`crate::diagram_edit`]).
    pub edit_gate: crate::diagram_edit::Gate,
    /// The right click or double click the host has not taken yet.
    pub gesture: Option<crate::diagram_edit::Gesture>,
    /// Why a gesture did nothing, and where it was made: shown until the
    /// next click or Escape.
    pub off_note: Option<(Pos2, String)>,
    /// The folds each POU's diagram opens with
    /// ([`helmsman_diagram::fold::auto`]): sets of blocks drawn as one.
    folds: Vec<Vec<Fold>>,
    /// The folds the reader opened, as (POU, fold key): drawn as their
    /// blocks, outlined, until folded back.
    opened: std::collections::BTreeSet<(String, String)>,
    /// The folds that start as their blocks (a section, an arm: see
    /// [`on_request`]) and the reader folded, as (POU, fold key).
    collapsed: std::collections::BTreeSet<(String, String)>,
    /// How far the operator moved each folded block, as (POU number, fold
    /// key, by).
    fold_moved: Vec<(usize, String, Vec2)>,
    /// What is drawn for each POU while any of its folds is closed: its
    /// diagram with them applied. `None`: the diagram itself.
    views: Vec<Option<View>>,
    /// The Q or E jump the view is easing through.
    glide: Option<Glide>,
    /// Where the last jump went: the scope, the stop it picked and the
    /// offset and zoom it left. While the view rests there the next jump
    /// steps on from that stop ([`crate::jump::next`]).
    jumped: Option<(usize, Pick, (Vec2, f32))>,
}

/// Whether folds of `kind` start as their blocks and fold when the reader
/// asks: the author's own structure. The standard circuits start folded.
const fn on_request(kind: FoldKind) -> bool {
    matches!(kind, FoldKind::Arm | FoldKind::Section | FoldKind::Output)
}

/// Height of the tab over an opened fold's blocks, in diagram units.
const FOLD_TAB: f32 = 16.0;

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

/// A block as an [`Arrangement`] names it: POU, label, source text, which
/// of the same.
pub type BlockKey = (String, String, String, usize);

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

    /// `self` brought up to date by `newer`, the moves now read off `v`:
    /// those of blocks `v` has are `newer`'s alone (a move undone is
    /// gone), while those of blocks `v` lacks stay, for an edit that
    /// brings the block back. When too many stay for a layout file, the
    /// oldest go first.
    #[must_use]
    pub fn updated(&self, newer: &Self, v: &Viewer) -> Self {
        let mut out: Vec<_> = self
            .0
            .iter()
            .filter(|(p, l, t, k, _)| v.block(p, l, t, *k).is_none())
            .cloned()
            .collect();
        let room = helmsman_project::layout::MAX_MOVES.saturating_sub(newer.len());
        out.drain(..out.len().saturating_sub(room));
        out.extend(newer.0.iter().cloned());
        Self(out)
    }

    /// The moves of the blocks `pairs` name as old, now under the new name
    /// (a diagram edit changes a block's text, which names it), less what
    /// `shared` already moves the new name by.
    #[must_use]
    pub fn carried(&self, pairs: &[(BlockKey, BlockKey)], shared: &Self) -> Self {
        let mut out = Vec::new();
        for (old, new) in pairs {
            let Some(e) = self
                .0
                .iter()
                .find(|e| (&e.0, &e.1, &e.2, e.3) == (&old.0, &old.1, &old.2, old.3))
            else {
                continue;
            };
            let mut by = e.4;
            for s in shared
                .0
                .iter()
                .filter(|s| (&s.0, &s.1, &s.2, s.3) == (&new.0, &new.1, &new.2, new.3))
            {
                by -= s.4;
            }
            out.push((new.0.clone(), new.1.clone(), new.2.clone(), new.3, by));
        }
        Self(out)
    }

    /// `self` with `more` added: a move of `more` replaces one of `self`
    /// for the same block.
    #[must_use]
    pub fn merged(&self, more: &Self) -> Self {
        let mut out: Vec<_> = self
            .0
            .iter()
            .filter(|e| {
                !more
                    .0
                    .iter()
                    .any(|m| (&m.0, &m.1, &m.2, m.3) == (&e.0, &e.1, &e.2, e.3))
            })
            .cloned()
            .collect();
        out.extend(more.0.iter().cloned());
        Self(out)
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
        // The standard circuits, then what the author's own structure
        // offers: sections named by a comment, then arms.
        let folds = module
            .pous
            .iter()
            .map(|p| {
                let d = &p.diagram;
                // A section that is the whole body names nothing.
                let sections = structure::sections(d, src)
                    .into_iter()
                    .filter(|f| f.stmts.len() < d.top.len());
                let by_request = sections.chain(structure::arms(d));
                helmsman_diagram::fold::auto(d)
                    .into_iter()
                    .chain(by_request.map(Fold::with_blocks))
                    .collect()
            })
            .collect();
        let views = vec![None; module.pous.len()];
        let mut v = Self {
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
            breaks: Vec::new(),
            paused: None,
            baseline: None,
            change: None,
            edit_gate: crate::diagram_edit::Gate::Off(crate::diagram_edit::BOX_REASON.into()),
            gesture: None,
            off_note: None,
            folds,
            opened: std::collections::BTreeSet::new(),
            collapsed: std::collections::BTreeSet::new(),
            fold_moved: Vec::new(),
            views,
            glide: None,
            jumped: None,
        };
        v.reshow();
        v
    }

    /// Rebuilds what is drawn for every POU: its diagram with the folds
    /// still closed applied, each block where the operator left it.
    fn reshow(&mut self) {
        for pou in 0..self.module.pous.len() {
            // The folds the reader made go first (a fold that shares a
            // block with an earlier one is left out): a folded section
            // is one block whatever is inside it.
            let asked = self.folds[pou].iter().filter(|f| on_request(f.kind));
            let standard = self.folds[pou].iter().filter(|f| !on_request(f.kind));
            let closed: Vec<Fold> = asked
                .chain(standard)
                .filter(|f| self.is_closed(pou, f))
                .cloned()
                .collect();
            if closed.is_empty() {
                self.views[pou] = None;
                continue;
            }
            let mut view = helmsman_diagram::fold::view(&self.module.pous[pou].diagram, &closed);
            if view.folds.is_empty() {
                self.views[pou] = None;
                continue;
            }
            for k in 0..view.diagram.nodes.len() {
                let by = match view.origin[k] {
                    Origin::Node(n) => self.moved[pou].iter().find(|(m, _)| *m == n).map(|(_, v)| *v),
                    Origin::Fold(f) => self
                        .fold_moved
                        .iter()
                        .find(|(p, key, _)| *p == pou && *key == view.folds[f].key)
                        .map(|(_, _, v)| *v),
                };
                if let Some(by) = by {
                    let r = &mut view.diagram.nodes[k].rect;
                    r.x += by.x;
                    r.y += by.y;
                }
            }
            helmsman_diagram::layout::refit(&mut view.diagram);
            self.views[pou] = Some(view);
        }
    }

    /// Whether fold `f` of POU number `pou` is drawn as one block now.
    fn is_closed(&self, pou: usize, f: &Fold) -> bool {
        let entry = (self.module.pous[pou].name.clone(), f.key.clone());
        if on_request(f.kind) {
            self.collapsed.contains(&entry)
        } else {
            !self.opened.contains(&entry)
        }
    }

    /// The folded view of the POU shown, while one of its folds is closed.
    fn view(&self) -> Option<&View> {
        self.views.get(self.scopes[self.scope].pou)?.as_ref()
    }

    /// The diagram as drawn: [`Self::diagram`] with its closed folds each
    /// one block. Its block and region numbers are its own; picks, edits
    /// and everything else the viewer keeps are in the diagram's.
    #[must_use]
    pub fn shown(&self) -> &Diagram {
        self.view().map_or_else(|| self.diagram(), |v| &v.diagram)
    }

    /// The block of the diagram behind block `n` as drawn; `None` for a
    /// fold.
    fn to_base(&self, n: NodeId) -> Option<NodeId> {
        self.view().map_or(Some(n), |v| v.original(n))
    }

    /// The comparison and the literal of the folded comparison drawn as
    /// block `n` ([`helmsman_diagram::fold::chips`]).
    fn chip(&self, n: NodeId) -> Option<(NodeId, NodeId)> {
        let f = self.fold_shown(n).filter(|f| f.kind == FoldKind::Chip)?;
        let d = self.diagram();
        let of = |want: fn(&NodeKind) -> bool| f.nodes.iter().copied().find(|&m| want(&d.node(m).kind));
        Some((
            of(|k| matches!(k, NodeKind::Op { .. }))?,
            of(|k| matches!(k, NodeKind::Const { .. }))?,
        ))
    }

    /// The block of the diagram a gesture on block `n` as drawn is made
    /// on ([`crate::diagram_edit`]): the block itself, or the comparison
    /// of a folded comparison (its operator is what a menu changes). Any
    /// other fold has nothing to change until it is opened.
    pub(crate) fn base_block(&self, n: NodeId) -> Option<NodeId> {
        self.to_base(n).or_else(|| self.chip(n).map(|(op, _)| op))
    }

    /// The literal a double click on block `n` as drawn changes: a
    /// constant block, or the one folded into a comparison.
    pub(crate) fn base_const(&self, n: NodeId) -> Option<NodeId> {
        match self.to_base(n) {
            Some(o) => matches!(self.diagram().node(o).kind, NodeKind::Const { .. }).then_some(o),
            None => self.chip(n).map(|(_, konst)| konst),
        }
    }

    /// The input pin of the diagram behind input `input` of block `n` as
    /// drawn: the same pin, or for a fold the one pin of its blocks that
    /// wire enters by (none when several read it).
    pub(crate) fn base_pin(&self, n: NodeId, input: usize) -> Option<(NodeId, usize)> {
        let Some(v) = self.view() else {
            return Some((n, input));
        };
        match *v.origin.get(n.0 as usize)? {
            Origin::Node(o) => Some((o, input)),
            Origin::Fold(f) => {
                let helmsman_diagram::fold::Input::Wire { from, negated } = v.ins[f].get(input)? else {
                    return None;
                };
                let mut into = self.diagram().wires.iter().filter(|w| {
                    (w.from, w.out, w.negated) == (from.0, from.1, *negated)
                        && v.folds[f].nodes.contains(&w.to)
                });
                let w = into.next()?;
                into.next().is_none().then_some((w.to, w.inp))
            }
        }
    }

    /// The block drawn for block `n` of the diagram: itself, or the fold
    /// it is in.
    fn to_shown(&self, n: NodeId) -> Option<NodeId> {
        self.view().map_or(Some(n), |v| v.shown(n))
    }

    /// Where block `n` of the diagram is drawn on screen: its own box,
    /// or the box of the fold it is in.
    #[cfg(test)]
    pub(crate) fn block_on_screen(&self, n: NodeId) -> Option<Rect> {
        let s = self.to_shown(n)?;
        Some(self.cams[self.scope].rect(&self.shown().node(s).rect))
    }

    /// Opens every fold that hides block `n` of the diagram shown, as a
    /// reader would by double clicks (a comparison folded with its
    /// literal is never opened: it is edited as it is).
    #[cfg(test)]
    pub(crate) fn reveal(&mut self, n: NodeId) {
        let pou = self.scopes[self.scope].pou;
        let keys: Vec<String> = self.folds[pou]
            .iter()
            .filter(|f| f.kind != FoldKind::Chip && f.nodes.contains(&n) && self.is_closed(pou, f))
            .map(|f| f.key.clone())
            .collect();
        for key in keys {
            self.set_fold(&key, true);
        }
    }

    /// Where input `k` of block `n` of the diagram is drawn on screen:
    /// its own pin, or the pin of the fold it is in that carries the same
    /// wire. `None` for a pin inside a fold.
    #[cfg(test)]
    pub(crate) fn pin_on_screen(&self, n: NodeId, k: usize) -> Option<Pos2> {
        let s = self.to_shown(n)?;
        let shown = self.shown().node(s);
        let pin = match self.view().and_then(|v| Some((v, v.fold_at(s)?))) {
            None => k,
            Some((v, _)) => {
                let w = self.diagram().wires.iter().find(|w| w.to == n && w.inp == k)?;
                let Origin::Fold(f) = v.origin[s.0 as usize] else {
                    return None;
                };
                v.ins[f].iter().position(|i| {
                    *i == helmsman_diagram::fold::Input::Wire {
                        from: (w.from, w.out),
                        negated: w.negated,
                    }
                })?
            }
        };
        let (x, y) = in_port(shown, pin);
        Some(self.cams[self.scope].to_screen(x, y))
    }

    /// The fold drawn as block `n`, if it is one.
    fn fold_shown(&self, n: NodeId) -> Option<&Fold> {
        self.view()?.fold_at(n)
    }

    /// The block of the diagram a click on block `n` as drawn picks: the
    /// block itself, or for a fold the block its first output comes from.
    fn pick_of(&self, n: NodeId) -> Option<NodeId> {
        let Some(v) = self.view() else { return Some(n) };
        match *v.origin.get(n.0 as usize)? {
            Origin::Node(o) => Some(o),
            Origin::Fold(f) => v.outs[f]
                .first()
                .map(|p| p.0)
                .or_else(|| v.folds[f].nodes.last().copied()),
        }
    }

    /// The closed fold under screen point `p`: its key.
    fn fold_at(&self, cam: Cam, p: Pos2) -> Option<String> {
        let (x, y) = cam.to_world(p);
        let n = self.shown().node_at(x, y)?;
        // A folded comparison hides nothing: its title is its text, and
        // a double click changes its value.
        self.fold_shown(n)
            .filter(|f| f.kind != FoldKind::Chip)
            .map(|f| f.key.clone())
    }

    /// Draws fold `key` of the POU shown as its blocks (`open`) or as
    /// one block again.
    pub fn set_fold(&mut self, key: &str, open: bool) {
        let pou = self.scopes[self.scope].pou;
        let Some(kind) = self.folds[pou].iter().find(|f| f.key == key).map(|f| f.kind) else {
            return;
        };
        let entry = (self.module.pous[pou].name.clone(), key.to_string());
        // What starts as its blocks is kept while folded; what starts
        // folded, while opened.
        let (set, hold) = if on_request(kind) {
            (&mut self.collapsed, !open)
        } else {
            (&mut self.opened, open)
        };
        if hold {
            set.insert(entry);
        } else {
            set.remove(&entry);
        }
        self.pick = None;
        self.reshow();
    }

    /// Each opened fold of the POU shown with the box around its blocks
    /// and the tab over it that folds it back, on screen.
    fn opened_tabs(&self, cam: Cam) -> Vec<(String, String, Rect, Rect)> {
        let pou = self.scopes[self.scope].pou;
        let d = self.shown();
        let mut out = Vec::new();
        for f in &self.folds[pou] {
            // An arm has its region's box and label already; a folded
            // comparison is never opened.
            if self.is_closed(pou, f) || matches!(f.kind, FoldKind::Arm | FoldKind::Chip) {
                continue;
            }
            // Around whatever is drawn for its blocks (a fold inside it
            // that is closed is one of them).
            let boxes = f.regions(self.diagram()).into_iter().filter_map(|r| {
                let r = self.view().map_or(Some(r), |v| v.region_shown(r))?;
                Some(cam.rect(&d.region(r).rect))
            });
            let rects: Vec<Rect> = f
                .nodes
                .iter()
                .filter_map(|&n| self.to_shown(n))
                .map(|s| cam.rect(&d.node(s).rect))
                .chain(boxes)
                .collect();
            let Some(around) = rects.into_iter().reduce(Rect::union) else {
                continue;
            };
            let around = around.expand(6.0 * cam.zoom);
            let w = (f.title.chars().count() as f32 * 6.5 + 14.0) * cam.zoom;
            let tab = Rect::from_min_size(
                Pos2::new(around.min.x, around.min.y - FOLD_TAB * cam.zoom),
                Vec2::new(w, FOLD_TAB * cam.zoom),
            );
            out.push((f.key.clone(), f.title.clone(), around, tab));
        }
        out
    }

    /// The arm whose label strip is under screen point `p`, if it folds:
    /// its fold's key.
    fn arm_at(&self, cam: Cam, p: Pos2) -> Option<String> {
        let (x, y) = cam.to_world(p);
        let shown = self.shown();
        let r = shown
            .regions
            .iter()
            .filter(|r| r.rect.contains(x, y) && y <= r.rect.y + LABEL_H)
            .min_by(|a, b| (a.rect.w * a.rect.h).total_cmp(&(b.rect.w * b.rect.h)))?;
        let base = self.view().map_or(r.id, |v| v.region_origin[r.id.0 as usize]);
        let body = &self.diagram().region(base).body;
        self.folds[self.scopes[self.scope].pou]
            .iter()
            .find(|f| f.kind == FoldKind::Arm && f.stmts == *body)
            .map(|f| f.key.clone())
    }

    /// Moves block `n` as drawn by `by`: a block of the diagram, or a fold.
    fn move_shown(&mut self, n: NodeId, by: Vec2) {
        let pou = self.scopes[self.scope].pou;
        let Some(key) = self.fold_shown(n).map(|f| f.key.clone()) else {
            if let Some(o) = self.to_base(n) {
                self.move_node(o, by);
            }
            return;
        };
        self.edits += 1;
        match self
            .fold_moved
            .iter_mut()
            .find(|(p, k, _)| *p == pou && *k == key)
        {
            Some((_, _, total)) => *total += by,
            None => self.fold_moved.push((pou, key, by)),
        }
        self.reshow();
    }

    /// Moves everything in region `r` as drawn by `by`.
    fn move_shown_region(&mut self, r: RegionId, by: Vec2) {
        if self.view().is_none() {
            self.move_region(r, by);
            return;
        }
        let members = helmsman_diagram::layout::inside(self.shown(), r);
        for n in members {
            self.move_shown(n, by);
        }
    }

    /// What the Agent's last change added on the diagram shown (block
    /// ids of the new diagram) and what it removed (block ids of the old
    /// one, with the old diagram): empty with no baseline.
    pub(crate) fn agent_change(&mut self) -> (Vec<NodeId>, Vec<NodeId>) {
        let Some(base) = self.baseline.clone() else {
            self.change = None;
            return (Vec::new(), Vec::new());
        };
        let key = std::rc::Rc::as_ptr(&base) as usize;
        if self
            .change
            .as_ref()
            .is_none_or(|(k, src, _)| *k != key || *src != self.src)
        {
            let d = helmsman_diagram::diff::modules(&base, &self.module);
            self.change = Some((key, self.src.clone(), d));
        }
        let Some((_, _, d)) = &self.change else {
            return (Vec::new(), Vec::new());
        };
        let pou = &self.module.pous[self.scopes[self.scope].pou].name;
        (
            d.new_nodes(pou, helmsman_diagram::diff::Change::Added),
            d.removed_nodes(pou),
        )
    }

    /// The picked block if it reads a variable: its path and its index
    /// among the diagram's reads (the order [`Self::reads_on_screen`]
    /// lists them in, which keys the controls drawn over them).
    #[must_use]
    pub fn picked_read(&self) -> Option<(usize, String)> {
        let Some(Pick::Node(n)) = self.pick else {
            return None;
        };
        let reads = self
            .diagram()
            .nodes
            .iter()
            .filter(|m| matches!(m.kind, NodeKind::Read { .. }));
        reads.enumerate().find_map(|(k, m)| match &m.kind {
            NodeKind::Read { path } if m.id == n => Some((k, path.clone())),
            _ => None,
        })
    }

    /// The statement the picked block or region was lowered from: what a
    /// breakpoint (Shift F9) goes on.
    #[must_use]
    pub fn picked_stmt(&self) -> Option<StmtId> {
        let d = self.diagram();
        match self.pick? {
            Pick::Node(n) => d.nodes.get(n.0 as usize).map(|n| n.stmt),
            Pick::Region(r) => d.regions.get(r.0 as usize).map(|r| r.stmt),
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

    /// A scan that ran since the last frame and is not the one drawn:
    /// what ran in it stays lit for [`HOLD`] all the same, so a statement
    /// lights every time it runs, whichever scan of a frame that was in.
    pub fn saw(&mut self, scan: &dyn Scan, now: f64) {
        if let Some(mut l) = self.live(Some(scan)) {
            self.hold(&mut l, now);
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

    /// How an [`Arrangement`] names block `n` of POU number `pou`.
    #[must_use]
    pub fn key_of(&self, pou: usize, n: NodeId) -> Option<BlockKey> {
        let p = self.module.pous.get(pou)?;
        let node = p.diagram.nodes.get(n.0 as usize)?;
        let k = p.diagram.nodes[..n.0 as usize]
            .iter()
            .filter(|m| m.label == node.label && m.text == node.text)
            .count();
        Some((p.name.clone(), node.label.clone(), node.text.clone(), k))
    }

    /// The same blocks under their new names across an edit. A POU whose
    /// blocks are the same in number and kind (a constant, an operator, a
    /// bubble or a rename changes only text) pairs them by place; for any
    /// other the blocks that differ pair up as
    /// [`helmsman_diagram::diff::modules`] pairs them.
    #[must_use]
    pub fn edited_blocks(&self, new: &Self) -> Vec<(BlockKey, BlockKey, NodeId, NodeId)> {
        use helmsman_diagram::diff::{Change, modules};
        let pou_of = |v: &Self, name: &str| v.module.pous.iter().position(|p| p.name == name);
        let mut out = Vec::new();
        let mut by_diff = false;
        for (pn, p) in new.module.pous.iter().enumerate() {
            let Some(po) = pou_of(self, &p.name) else {
                continue;
            };
            let (a, b) = (&self.module.pous[po].diagram.nodes, &p.diagram.nodes);
            let same_shape = a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(x, y)| std::mem::discriminant(&x.kind) == std::mem::discriminant(&y.kind));
            if !same_shape {
                by_diff = true;
                continue;
            }
            for (x, y) in a.iter().zip(b) {
                if (x.label.as_str(), x.text.as_str()) == (y.label.as_str(), y.text.as_str()) {
                    continue;
                }
                if let (Some(ko), Some(kn)) = (self.key_of(po, x.id), new.key_of(pn, y.id)) {
                    out.push((ko, kn, x.id, y.id));
                }
            }
        }
        if by_diff {
            for c in &modules(&self.module, &new.module).nodes {
                if c.change != Change::Changed {
                    continue;
                }
                let (Some((o, _)), Some((n, _))) = (c.old.as_ref(), c.new.as_ref()) else {
                    continue;
                };
                let (Some(po), Some(pn)) = (pou_of(self, &c.pou), pou_of(new, &c.pou)) else {
                    continue;
                };
                if self.module.pous[po].diagram.nodes.len() == new.module.pous[pn].diagram.nodes.len()
                    && std::mem::discriminant(&self.module.pous[po].diagram.nodes[o.0 as usize].kind)
                        == std::mem::discriminant(&new.module.pous[pn].diagram.nodes[n.0 as usize].kind)
                    && out.iter().any(|r| r.2 == *o)
                {
                    continue;
                }
                if let (Some(ko), Some(kn)) = (self.key_of(po, *o), new.key_of(pn, *n)) {
                    out.push((ko, kn, *o, *n));
                }
            }
        }
        out
    }

    /// Keeps what `old` showed across an edit: the scope, every camera
    /// and the picked block (by `renamed`, from [`Self::edited_blocks`],
    /// when its text changed).
    pub fn carry_view(&mut self, old: &Self, renamed: &[(BlockKey, BlockKey, NodeId, NodeId)]) {
        if old.scopes == self.scopes {
            self.cams.clone_from(&old.cams);
            self.scope = old.scope;
        }
        // A fold the reader opened stays open while the program still
        // has it under the same key.
        self.opened.clone_from(&old.opened);
        self.collapsed.clone_from(&old.collapsed);
        self.reshow();
        let Some(pick) = old.pick else { return };
        let pou = self.scopes.get(self.scope).map_or(0, |s| s.pou);
        self.pick = match pick {
            Pick::Node(n) => {
                let key = old.key_of(old.scopes[old.scope].pou, n);
                renamed
                    .iter()
                    .find(|r| r.2 == n)
                    .map(|r| r.3)
                    .or_else(|| {
                        let (p, label, text, k) = key?;
                        self.block(&p, &label, &text, k).map(|(_, id)| id)
                    })
                    .map(Pick::Node)
            }
            Pick::Region(r) => self
                .module
                .pous
                .get(pou)
                .and_then(|p| p.diagram.regions.get(r.0 as usize))
                .map(|_| Pick::Region(r)),
        };
    }

    /// Moves block `n` of the shown POU by `by` (diagram units).
    pub fn move_node(&mut self, n: NodeId, by: Vec2) {
        let pou = self.scopes[self.scope].pou;
        self.shift(pou, &[n], by);
        helmsman_diagram::layout::refit(&mut self.module.pous[pou].diagram);
        self.reshow();
    }

    /// Moves everything in region `r` of the shown POU by `by`.
    pub fn move_region(&mut self, r: RegionId, by: Vec2) {
        let pou = self.scopes[self.scope].pou;
        let members = helmsman_diagram::layout::inside(&self.module.pous[pou].diagram, r);
        self.shift(pou, &members, by);
        helmsman_diagram::layout::refit(&mut self.module.pous[pou].diagram);
        self.reshow();
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
            || self.fold_moved.iter().any(|(_, _, v)| *v != Vec2::ZERO)
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
        self.fold_moved.clear();
        self.reshow();
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
        self.reshow();
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
                    if ui
                        .small_button(if row.folded { "▸" } else { "▾" })
                        .tip(if row.folded {
                            "Shows the instances it calls."
                        } else {
                            "Hides the instances it calls."
                        })
                        .clicked()
                    {
                        fold = Some(row.scope);
                    }
                } else {
                    ui.add_space(18.0);
                }
                let text = RichText::new(&s.label);
                if ui
                    .selectable_label(row.scope == self.scope, text)
                    .tip("Shows this one's diagram.")
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
        let t = theme(ui);
        ui.horizontal_wrapped(|ui| {
            let up = self.parent();
            if ui
                .add_enabled(up.is_some(), egui::Button::new("↑"))
                .tip(if up.is_some() {
                    "Up one level (Alt+↑)."
                } else {
                    "Up one level (Alt+↑): this is the program, the top."
                })
                .clicked()
            {
                go = up;
            }
            for (i, o) in outer.iter().enumerate() {
                if ui
                    .link(RichText::new(o).color(t.muted))
                    .tip(format!("Goes to {o}."))
                    .clicked()
                {
                    hit = Some(i);
                }
                ui.label(RichText::new("›").color(t.muted));
            }
            let trail = self.trail();
            for (j, &k) in trail.iter().enumerate() {
                if j > 0 {
                    ui.label(RichText::new("›").color(t.muted));
                }
                let last = j + 1 == trail.len();
                let text = RichText::new(&self.scopes[k].label).monospace();
                let text = if last { text.strong().color(t.accent) } else { text };
                let tip = if last {
                    "The diagram shown now."
                } else {
                    "Shows this level's diagram."
                };
                if ui.link(text).tip(tip).clicked() {
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
            .tip("Click an output to light the path that sets it in the diagram.");
        let t = theme(ui);
        for path in self.outputs() {
            let value = scan.and_then(|s| s.get(&path));
            let on = value == Some(Value::Bool(true));
            let color = if on {
                crate::lamps::colour(&path).map_or(t.accent, |c| t.ink(c))
            } else {
                t.fg
            };
            let text = RichText::new(format!("{} {path} = {}", if on { "●" } else { "○" }, show(value)))
                .monospace()
                .color(color);
            let glyph = crate::glyph_lib::for_point(&path, false, matches!(value, Some(Value::Bool(_))));
            let clicked = ui
                .horizontal(|ui| {
                    crate::glyph_lib::show(
                        ui,
                        glyph,
                        crate::glyphs::ROW,
                        color,
                        crate::glyph_lib::tip(glyph),
                    );
                    let row = egui::Button::selectable(self.because.as_deref() == Some(path.as_str()), text)
                        .min_size(Vec2::new(ui.available_width(), TAP));
                    ui.add(row)
                        .tip("Click to light why it has this value in the diagram.")
                        .clicked()
                })
                .inner;
            if clicked {
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
        let shown = self.shown();
        self.diagram()
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Read { path } => {
                    // A read inside a closed fold has no block to put a
                    // control under.
                    let r = self.to_shown(n.id).filter(|&s| self.to_base(s).is_some()).map_or(
                        Rect::NOTHING,
                        |s| {
                            let mut r = cam.rect(&shown.node(s).rect);
                            r.min.y += HEADER * cam.zoom;
                            r
                        },
                    );
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
        let n = self.shown().node_at(x, y)?;
        let NodeKind::Fb { instance, .. } = &self.shown().node(n).kind else {
            return None;
        };
        self.instance_scope(instance)
    }

    /// The scope of block instance `instance` called on the diagram shown,
    /// if it has a diagram of its own (a user function block's).
    fn instance_scope(&self, instance: &str) -> Option<usize> {
        let want = format!("{}{instance}.", self.scopes[self.scope].prefix);
        self.scopes
            .iter()
            .position(|s| s.prefix.eq_ignore_ascii_case(&want))
    }

    /// The blocks on the diagram shown that open into a diagram of their
    /// own (a user function block's instances): drawn as a stack with a
    /// chevron, so it shows that a double click goes deeper (Nick,
    /// 2026-11-11).
    #[must_use]
    pub fn openable(&self) -> Vec<NodeId> {
        self.shown()
            .nodes
            .iter()
            .filter(|n| match &n.kind {
                NodeKind::Fb { instance, .. } => self.instance_scope(instance).is_some(),
                _ => false,
            })
            .map(|n| n.id)
            .collect()
    }

    /// Opens the picked block instance's diagram (Alt ]); whether the
    /// pick was an instance with a diagram of its own.
    pub fn step_into(&mut self) -> bool {
        let Some(Pick::Node(n)) = self.pick else {
            return false;
        };
        let Some(NodeKind::Fb { instance, .. }) = self.diagram().nodes.get(n.0 as usize).map(|n| &n.kind)
        else {
            return false;
        };
        let Some(k) = self.instance_scope(&instance.clone()) else {
            return false;
        };
        self.open(k);
        true
    }

    /// Up to the caller's diagram (Alt [), the instance just left picked
    /// there so Alt ] goes back in; whether there was a level up.
    pub fn step_out(&mut self) -> bool {
        let Some(up) = self.parent() else {
            return false;
        };
        let left = self.scopes[self.scope].prefix.clone();
        self.open(up);
        let pre = self.scopes[up].prefix.clone();
        self.pick = self
            .diagram()
            .nodes
            .iter()
            .find(|n| {
                matches!(&n.kind, NodeKind::Fb { instance, .. }
                    if format!("{pre}{instance}.").eq_ignore_ascii_case(&left))
            })
            .map(|n| Pick::Node(n.id));
        true
    }

    /// What a drag starting at screen point `p` takes hold of.
    fn grab_at(&self, cam: Cam, p: Pos2) -> Grab {
        let (x, y) = cam.to_world(p);
        let d = self.shown();
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

    /// The stops of the diagram shown ([`crate::jump::stops`]), each
    /// framed where it is drawn: a stop inside a fold is the fold's block
    /// (an IF drawn as a selector is still a stop).
    pub(crate) fn stops(&self) -> Vec<crate::jump::Stop> {
        let base = crate::jump::stops(self.diagram());
        let Some(v) = self.view() else { return base };
        let d = self.diagram();
        let drawn: Vec<crate::jump::Stop> = crate::jump::stops(&v.diagram)
            .into_iter()
            .filter_map(|s| {
                let pick = match s.pick {
                    Pick::Node(n) => Pick::Node(self.pick_of(n)?),
                    Pick::Region(r) => Pick::Region(v.region_origin[r.0 as usize]),
                };
                Some(crate::jump::Stop { pick, rect: s.rect })
            })
            .collect();
        let around = |blocks: &mut dyn Iterator<Item = NodeId>| {
            blocks
                .filter_map(|n| v.shown(n))
                .map(|n| v.diagram.node(n).rect)
                .reduce(|a, b| {
                    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
                    helmsman_diagram::Rect {
                        x,
                        y,
                        w: a.right().max(b.right()) - x,
                        h: a.bottom().max(b.bottom()) - y,
                    }
                })
        };
        base.into_iter()
            .map(|s| {
                if let Some(at) = drawn.iter().find(|x| x.pick == s.pick) {
                    return crate::jump::Stop {
                        pick: s.pick,
                        rect: at.rect,
                    };
                }
                // Folded away: whatever is drawn for its blocks.
                let rect = match s.pick {
                    Pick::Node(n) => around(&mut std::iter::once(n)),
                    Pick::Region(r) => {
                        let stmt = d.region(r).stmt;
                        let inside = |n: &&helmsman_diagram::Node| {
                            n.stmt == stmt || d.chain(n.region).any(|q| q.stmt == stmt)
                        };
                        around(&mut d.nodes.iter().filter(inside).map(|n| n.id))
                    }
                };
                crate::jump::Stop {
                    pick: s.pick,
                    rect: rect.unwrap_or(s.rect),
                }
            })
            .collect()
    }

    /// Q or E ([`crate::jump`]): picks the next (`forward`) or previous
    /// stop on the diagram shown and starts the view easing over to frame
    /// it in `area`, below the glyph strip. Nothing with no stops.
    fn jump(&mut self, forward: bool, area: Rect, now: f64) {
        let stops = self.stops();
        let cam = self.cams[self.scope];
        let below = Rect::from_min_max(area.min + Vec2::new(0.0, GLYPH_BAND), area.max);
        // A jump still easing over counts as having arrived, so two quick
        // presses go two stops on.
        let at = self
            .jumped
            .filter(|&(scope, _, to)| scope == self.scope && (self.glide.is_some() || cam.rests_at(to)))
            .and_then(|(_, pick, _)| stops.iter().position(|s| s.pick == pick));
        let Some(k) = crate::jump::next(&stops, at, cam.to_world(below.center()), forward) else {
            return;
        };
        let mut to = cam;
        to.frame(below, &stops[k].rect);
        to.clamp(area, self.shown().size);
        let to = (to.offset, to.zoom);
        self.cams[self.scope].moved = true;
        self.glide = Some(Glide {
            scope: self.scope,
            from: (cam.offset, cam.zoom),
            to,
            t0: now,
        });
        self.jumped = Some((self.scope, stops[k].pick, to));
        self.pick = Some(stops[k].pick);
    }

    /// The diagram, panned, zoomed and clickable, with `scan` on it.
    pub fn canvas(&mut self, ui: &mut egui::Ui, scan: Option<&dyn Scan>) {
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        let t = theme(ui);
        let size = self.shown().size;
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
        // A jump under way moves the view first; anything the operator
        // does to it this frame ends the jump where it is.
        let now = ui.input(|i| i.time);
        self.glide = self.glide.filter(|g| g.scope == self.scope);
        if let Some(g) = self.glide {
            let (to, done) = g.at(now, area.size() / 2.0);
            (cam.offset, cam.zoom) = to;
            if done {
                self.glide = None;
            } else {
                ui.ctx().request_repaint();
            }
        }
        let glided = (cam.offset, cam.zoom);
        // The wheel (or a two-finger scroll) zooms about the pointer, as
        // a pinch does; a drag on empty space, the arrow keys or W A S D
        // move the view (Nick, 2026-11-14: scroll moved it up and down).
        // Q and E jump to the previous and the next stop.
        let (hover, scroll, zoom, keys, jump, dt) = ui.ctx().input(|i| {
            use egui::Key;
            let held = |a: Key, b: Key| f32::from(u8::from(i.key_down(a) || i.key_down(b)));
            // The key's own modifiers: a chord with E or Q is not a jump.
            let jump = i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key: key @ (Key::E | Key::Q),
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() => Some(*key == Key::E),
                _ => None,
            });
            let keys = if i.modifiers.any() {
                Vec2::ZERO
            } else {
                Vec2::new(
                    held(Key::ArrowRight, Key::D) - held(Key::ArrowLeft, Key::A),
                    held(Key::ArrowDown, Key::S) - held(Key::ArrowUp, Key::W),
                )
            };
            (
                i.pointer.hover_pos(),
                i.smooth_scroll_delta,
                i.zoom_delta(),
                keys,
                jump,
                i.stable_dt.min(0.1),
            )
        });
        // The keys are the diagram's only while nothing is typed in.
        let keyboard = response.hovered() && ui.ctx().memory(|m| m.focused().is_none());
        if response.hovered() {
            let zoom = if (zoom - 1.0).abs() > f32::EPSILON {
                zoom
            } else {
                (scroll.y / SCROLL_ZOOM).exp()
            };
            if (zoom - 1.0).abs() > f32::EPSILON
                && let Some(h) = hover
            {
                cam.zoom_at(h, zoom);
            }
            if keys != Vec2::ZERO && keyboard {
                // Right shows what is further right: the diagram goes left.
                cam.offset -= keys * KEY_PAN * dt;
                cam.moved = true;
                ui.ctx().request_repaint();
            }
        }
        let double = response.double_clicked();
        if (cam.offset, cam.zoom) != glided || double || response.dragged() {
            self.glide = None;
        }
        let cam = *cam;
        // A double click on a block instance opens it; anywhere else it
        // fits the diagram to the view.
        if double
            && let Some(p) = response.interact_pointer_pos()
            && let Some(n) = self.const_at(cam, p)
        {
            self.gesture_at(p, crate::diagram_edit::Hit::Block(n), true);
        } else if double
            && let Some(p) = response.interact_pointer_pos()
            && let Some(key) = self.fold_at(cam, p)
        {
            // A fold opens into its blocks...
            self.set_fold(&key, true);
        } else if double
            && let Some(p) = response.interact_pointer_pos()
            && let Some((key, ..)) = self
                .opened_tabs(cam)
                .into_iter()
                .find(|(.., tab)| tab.contains(p))
        {
            // ...and its tab folds them back.
            self.set_fold(&key, false);
        } else if double
            && let Some(p) = response.interact_pointer_pos()
            && let Some(key) = self.arm_at(cam, p)
        {
            // An arm folds from its label.
            self.set_fold(&key, false);
        } else if double {
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
                Some(Grab::Node(n)) => self.move_shown(n, by / cam.zoom),
                Some(Grab::Region(r)) => self.move_shown_region(r, by / cam.zoom),
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
        let size = self.shown().size;
        self.cams[self.scope].clamp(area, size);
        if let Some(forward) = jump.filter(|_| keyboard) {
            self.jump(forward, area, now);
        }
        let cam = self.cams[self.scope];
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let (x, y) = cam.to_world(p);
            let d = self.shown();
            // A finger is wider than a block at low zoom: the nearest
            // block within half a tap target counts.
            let slop = TAP / 2.0 / cam.zoom;
            let region = |r: RegionId| self.view().map_or(r, |v| v.region_origin[r.0 as usize]);
            self.pick = match node_near(d, x, y, slop) {
                Some(n) => self.pick_of(n).map(Pick::Node),
                None => d.region_at(x, y).map(|r| Pick::Region(region(r))),
            };
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
        if response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some(hit) = self.hit_at(cam, p)
        {
            let (crate::diagram_edit::Hit::Block(n) | crate::diagram_edit::Hit::Pin { node: n, .. }) = hit;
            self.pick = Some(Pick::Node(n));
            self.gesture_at(p, hit, false);
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
        let (added, removed) = self.agent_change();
        // On the diagram as drawn: a fold stands for its blocks.
        let seen = match (self.view(), &live) {
            (Some(v), Some(l)) => Some(v.live(l)),
            (_, l) => l.clone(),
        };
        let lit = match (self.view(), &self.lit) {
            (Some(v), Some(on)) => Some(v.drivers(on)),
            (_, on) => on.clone(),
        };
        let mut added: Vec<NodeId> = added.into_iter().filter_map(|n| self.to_shown(n)).collect();
        added.dedup();
        let pick = match self.pick {
            Some(Pick::Node(n)) => self.to_shown(n).map(Pick::Node),
            Some(Pick::Region(r)) => self
                .view()
                .map_or(Some(r), |v| v.region_shown(r))
                .map(Pick::Region),
            None => None,
        };
        let mut opens = self.openable();
        opens.extend(
            self.shown()
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Fold { kind, .. } if kind != FoldKind::Chip))
                .map(|n| n.id),
        );
        let tabs = self.opened_tabs(cam);
        if response.hovered()
            && let Some(p) = hover
        {
            if self.fold_at(cam, p).is_some() {
                response
                    .clone()
                    .on_hover_text_at_pointer("Several blocks drawn as one. Double-click shows them.");
            } else if tabs.iter().any(|(.., tab)| tab.contains(p)) {
                response
                    .clone()
                    .on_hover_text_at_pointer("Double-click draws these blocks as one.");
            } else if self.arm_at(cam, p).is_some() {
                response
                    .clone()
                    .on_hover_text_at_pointer("Double-click draws what this arm does as one block.");
            }
        }
        if response.hovered()
            && let Some(p) = hover
            && self.instance_at(cam, p).is_some()
        {
            response.clone().on_hover_text_at_pointer(
                "This block has a diagram of its own. Double-click opens it (Alt ] with it picked).",
            );
        }
        if response.hovered()
            && let Some(p) = hover
            && let Some(text) = self.edit_hover(cam, p)
        {
            response.clone().on_hover_text_at_pointer(text);
        }
        let pou = &self.module.pous[self.scopes[self.scope].pou].name;
        let old = self
            .baseline
            .as_ref()
            .and_then(|b| b.pou(pou))
            .map(|p| &p.diagram);
        draw(
            &painter,
            &t,
            area,
            cam,
            self.shown(),
            seen.as_ref(),
            Marks {
                pick,
                lines: &self.highlight,
                lit: lit.as_ref(),
                vars: self.module.program().map(|p| &p.vars),
                icons: &self.icons,
                breaks: &self.breaks,
                paused: self.paused,
                added: &added,
                opens: &opens,
                ghosts: old.map(|o| (o, removed.as_slice())),
            },
        );
        for (_, title, around, tab) in &tabs {
            let z = cam.zoom;
            painter.rect_stroke(
                *around,
                CornerRadius::same(6),
                Stroke::new(1.0 * z.max(0.6), t.edge),
                StrokeKind::Outside,
            );
            painter.rect_filled(*tab, CornerRadius::same(3), t.shade);
            painter.text(
                tab.left_center() + Vec2::new(7.0 * z, 0.0),
                Align2::LEFT_CENTER,
                title,
                font(10.0 * z),
                t.fg,
            );
        }
        self.off_note_ui(ui, area);
        // The program's BOOL outputs as lamps in the corner (a traffic
        // light for a red, a yellow and a green), on the program's own
        // diagram only: a block instance's outputs are not the machine's.
        if let Some(scan) = scan
            && self.scopes[self.scope].prefix.is_empty()
        {
            let outs = self.outputs();
            let fixture = crate::lamps::fixture(outs.iter().map(|p| (p.as_str(), scan.get(p))));
            if let Some(f) = &fixture {
                crate::lamps::paint(&painter, area, f);
            }
        }
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

/// `text` on one row at most `width` wide, ending in an ellipsis where it
/// does not fit.
fn fitted(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, width.max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    painter.layout_job(job)
}

pub fn show(v: Option<Value>) -> String {
    v.map_or_else(|| "–".into(), |v| v.to_string())
}

fn font(size: f32) -> FontId {
    FontId::monospace(size)
}

/// A wire's colour and width: TRUE in the brand yellow on either page,
/// FALSE muted, a value in the page's ink, no scan faint.
fn wire_color(t: &Theme, v: Option<Value>) -> (Color32, f32) {
    match v {
        Some(Value::Bool(true)) => (YELLOW, 2.5),
        Some(Value::Bool(false)) => (t.muted, 1.5),
        Some(_) => (t.fg, 1.5),
        None => (t.idle, 1.2),
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

/// The Agent's added blocks and wires (the brand green), and its
/// removed ones (the brand red, ghosted).
const ADDED: Color32 = brand::GREEN;

/// Blocks the Agent's change removed, drawn where they were in the old
/// diagram: a faint red fill, a red outline and their label, and the old
/// wires between two removed blocks, so what went is seen in place.
fn draw_ghosts(painter: &egui::Painter, cam: Cam, old: &Diagram, removed: &[NodeId]) {
    let z = cam.zoom;
    let red = brand::RED;
    for w in &old.wires {
        if removed.contains(&w.from) && removed.contains(&w.to) {
            let (a, b) = (old.node(w.from), old.node(w.to));
            let (ax, ay) = out_port(a, w.out);
            let (bx, by) = in_port(b, w.inp);
            bezier(
                painter,
                cam.to_screen(ax, ay),
                cam.to_screen(bx, by),
                z,
                red.gamma_multiply(0.45),
                1.2,
            );
        }
    }
    for id in removed {
        let Some(n) = old.nodes.get(id.0 as usize) else {
            continue;
        };
        let r = cam.rect(&n.rect);
        painter.rect_filled(r, CornerRadius::same(4), red.gamma_multiply(0.12));
        painter.rect_stroke(
            r,
            CornerRadius::same(4),
            Stroke::new(1.5 * z.max(0.6), red.gamma_multiply(0.7)),
            StrokeKind::Outside,
        );
        if z > 0.35 {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                &n.label,
                font(11.5 * z),
                red.gamma_multiply(0.75),
            );
        }
    }
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
    /// Statements with a breakpoint: a red dot on the blocks they end in
    /// (an IF's, CASE's or loop's on its first region's label).
    breaks: &'a [StmtId],
    /// The statement a paused scan stopped before: outlined.
    paused: Option<StmtId>,
    /// Blocks the Agent's last change added: outlined green.
    added: &'a [NodeId],
    /// Blocks that open into a diagram of their own: drawn as a stack.
    opens: &'a [NodeId],
    /// Blocks it removed, in the old diagram: drawn as red ghosts where
    /// they were.
    ghosts: Option<(&'a Diagram, &'a [NodeId])>,
}

/// Where a statement is marked on the diagram: the blocks it ends in, or
/// for an IF, CASE or loop its first region.
fn marked(d: &Diagram, id: StmtId) -> (Vec<NodeId>, Option<RegionId>) {
    let Some(s) = d.stmts.iter().find(|s| s.id == id) else {
        return (Vec::new(), None);
    };
    match s.arms.first() {
        Some(&r) => (Vec::new(), Some(r)),
        None => (s.roots.clone(), None),
    }
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
fn bubble(painter: &egui::Painter, t: &Theme, p: Pos2, z: f32, seen: Option<Value>, fade: f32) {
    let (c, _) = wire_color(t, seen);
    painter.circle(
        p - Vec2::new(BUBBLE * z, 0.0),
        BUBBLE * z,
        t.bg,
        Stroke::new(1.5 * z.max(0.5), c.gamma_multiply(fade)),
    );
}

/// A breakpoint's mark: a red dot with a dark rim, centred at `at`.
fn break_dot(painter: &egui::Painter, at: Pos2, z: f32) {
    let r = (6.0 * z).max(4.0);
    painter.circle(at, r, brand::RED, Stroke::new((1.5 * z).max(1.0), BLACK));
}

/// Where a paused scan stands: a thick warning-coloured frame.
fn paused_outline(painter: &egui::Painter, r: Rect, z: f32) {
    painter.rect_stroke(
        r.expand(5.0 * z),
        CornerRadius::same(7),
        Stroke::new((3.0 * z).max(2.0), brand::WARN),
        StrokeKind::Outside,
    );
}

fn draw(
    painter: &egui::Painter,
    t: &Theme,
    area: Rect,
    cam: Cam,
    d: &Diagram,
    live: Option<&Live>,
    marks: Marks<'_>,
) {
    let Marks {
        pick,
        lines,
        lit,
        vars,
        icons,
        breaks,
        paused,
        added,
        opens,
        ghosts,
    } = marks;
    let mut dot_nodes = std::collections::HashSet::new();
    let mut dot_regions = std::collections::HashSet::new();
    for &b in breaks {
        let (ns, r) = marked(d, b);
        dot_nodes.extend(ns);
        dot_regions.extend(r);
    }
    let (here_nodes, here_region) = paused.map_or_else(|| (Vec::new(), None), |p| marked(d, p));
    let z = cam.zoom;
    let strength = |on: bool| if lit.is_none() || on { 1.0 } else { DIMMED };
    // Pins entered through a bubble: no pin dot over it.
    let bubbled: std::collections::HashSet<(NodeId, usize)> = d
        .wires
        .iter()
        .filter(|w| w.negated)
        .map(|w| (w.to, w.inp))
        .collect();
    painter.rect_filled(area, CornerRadius::ZERO, t.bg);
    let text = z > 0.35;
    // Regions, outermost first.
    for r in &d.regions {
        let gate = live.map(|l| l.gates[r.id.0 as usize]);
        let f = strength(lit.is_some_and(|l| l.region(r.id)));
        let rect = cam.rect(&r.rect);
        let (stroke, fill, state) = match gate {
            Some(Gate::Open(n)) => (YELLOW, YELLOW.gamma_multiply(0.06), format!("taken ×{n}")),
            Some(Gate::Closed) => (
                t.muted.gamma_multiply(0.7),
                Color32::TRANSPARENT,
                "skipped".into(),
            ),
            Some(Gate::NotReached) => (t.rule, Color32::TRANSPARENT, "not reached".into()),
            None => (t.muted, Color32::TRANSPARENT, String::new()),
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
        if here_region == Some(r.id) {
            paused_outline(painter, rect, z);
        }
        if dot_regions.contains(&r.id) {
            break_dot(painter, rect.min + Vec2::new(-2.0, LABEL_H / 2.0) * z, z);
        }
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
            let (c, w) = wire_color(t, v);
            let end = cam.to_screen(r.rect.x, r.rect.y + LABEL_H / 2.0);
            let wire_end = if r.negated {
                end - Vec2::new(2.0 * BUBBLE * z, 0.0)
            } else {
                end
            };
            bezier(painter, cam.to_screen(x, y), wire_end, z, c.gamma_multiply(f), w);
            if r.negated {
                bubble(painter, t, end, z, live.and_then(|l| l.gate_value(d, r.id)), f);
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
        let (c, width) = wire_color(t, v);
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
            bubble(painter, t, pb, z, live.and_then(|l| l.input(d, k)), f);
        }
        if added.contains(&w.from) || added.contains(&w.to) {
            bezier(painter, pa, end, z, ADDED.gamma_multiply(0.85), 1.5);
        }
        if text
            && let Some(v) = v
            && !matches!(v, Value::Bool(_))
            && let Some(label) = value_text(d, w.from, v, vars)
        {
            // At most the wire's own length (and a block's width) wide.
            let room = (end.x - pa.x - 8.0 * z).clamp(48.0 * z, 120.0 * z);
            let g = fitted(painter, &label, font(10.0 * z), t.fg.gamma_multiply(f), room);
            let at = pa + Vec2::new(4.0, -7.0) * z;
            painter.galley(at - Vec2::new(0.0, g.size().y / 2.0), g, t.fg);
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
        // A block's head is yellow with black ink (the brand pair), an
        // assignment's the page inverted, a variable's or an operator's a
        // grey of the page with its ink.
        let (head, ink) = match &n.kind {
            NodeKind::Fb { .. } | NodeKind::For { .. } | NodeKind::Fold { .. } => (YELLOW, BLACK),
            NodeKind::Write { .. } => (t.fg, t.bg),
            NodeKind::Read { .. } | NodeKind::Const { .. } => (t.rule, t.fg),
            NodeKind::Op { .. } | NodeKind::Func { .. } | NodeKind::Access { .. } => (t.shade, t.fg),
            NodeKind::Jump => (STEEL, BLACK),
        };
        let fade: f32 = if ran { 1.0 } else { 0.4 } * on_path;
        let radius = CornerRadius::same(4);
        let h = Rect::from_min_size(r.min, Vec2::new(r.width(), HEADER * z));
        // A block that shows a value under its header (a variable) or
        // keeps its instance's ports apart (a function block) has a body
        // of its own; an operator, function or literal is one shade.
        let two_tone = n.kind.headed();
        let deeper = opens.contains(&n.id);
        if deeper {
            // Two cards behind it, each a step down and right: there is
            // more under this block.
            for k in [2.0, 1.0] {
                let back = r.translate(Vec2::splat(3.5 * k * z));
                painter.rect_filled(back, radius, t.bg);
                painter.rect_filled(back, radius, head.gamma_multiply(0.45 * fade));
                painter.rect_stroke(back, radius, Stroke::new(1.0 * z, t.edge), StrokeKind::Outside);
            }
        }
        if two_tone {
            painter.rect_filled(r, radius, t.panel.gamma_multiply(fade.max(0.7) * on_path));
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
            Stroke::new(1.0 * z, t.edge)
        };
        painter.rect_stroke(r, radius, outline, StrokeKind::Outside);
        if deeper {
            // A double chevron at the header's right end: go in.
            let c = Pos2::new(r.right() - 9.0 * z, h.center().y);
            let (dx, dy) = (3.0 * z, 4.0 * z);
            let stroke = Stroke::new(1.6 * z.max(0.6), ink.gamma_multiply(fade));
            for off in [-2.5 * z, 2.5 * z] {
                painter.add(egui::Shape::line(
                    vec![
                        Pos2::new(c.x + off - dx / 2.0, c.y - dy),
                        Pos2::new(c.x + off + dx / 2.0, c.y),
                        Pos2::new(c.x + off - dx / 2.0, c.y + dy),
                    ],
                    stroke,
                ));
            }
        }
        if added.contains(&n.id) {
            painter.rect_stroke(
                r.expand(3.0 * z),
                CornerRadius::same(6),
                Stroke::new(2.5 * z.max(0.6), ADDED),
                StrokeKind::Outside,
            );
        }
        if here_nodes.contains(&n.id) {
            paused_outline(painter, r, z);
        }
        if dot_nodes.contains(&n.id) {
            break_dot(painter, r.min + Vec2::new(-2.0, HEADER / 2.0) * z, z);
        }
        // The block's one-character icon: at the header's left edge beside
        // the label, or alone and large enough to read when the label is
        // too small to draw (Nick, 2026-11-01).
        // A standard block takes its picture from the glyph library
        // instead of its character (`Icons::glyph`).
        let glyph = icons.glyph(n);
        let icon = if glyph.is_some() { None } else { icons.of(n) };
        if !text {
            if let Some(g) = glyph {
                let side = (18.0 * z).max(14.0);
                crate::glyph_lib::paint(
                    painter,
                    Rect::from_center_size(r.center(), Vec2::splat(side)),
                    g,
                    ink.gamma_multiply(fade),
                );
            } else if let Some(i) = icon {
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    i,
                    font((14.0 * z).max(11.0)),
                    ink.gamma_multiply(fade),
                );
            }
        }
        if text {
            let band = if two_tone { h } else { r };
            // With an icon the label sits right of it, so the two never
            // overlap on a block sized to its label alone.
            let mut centre = band.center();
            if let Some(g) = glyph {
                let side = 11.0 * z;
                crate::glyph_lib::paint(
                    painter,
                    Rect::from_min_size(
                        Pos2::new(band.left() + 3.0 * z, band.center().y - side / 2.0),
                        Vec2::splat(side),
                    ),
                    g,
                    ink.gamma_multiply(fade),
                );
                centre.x += 7.0 * z;
            } else if let Some(i) = icon {
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
                    painter.circle_filled(p, 3.0 * z, t.muted.gamma_multiply(on_path));
                }
                if !name.is_empty() {
                    painter.text(
                        p + Vec2::new(6.0 * z, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        font(10.0 * z),
                        t.muted.gamma_multiply(on_path),
                    );
                }
            }
            for (k, name) in n.outs.iter().enumerate() {
                let (x, y) = out_port(n, k);
                let p = cam.to_screen(x, y);
                let v = live.and_then(|l| l.values[n.id.0 as usize].get(k).copied().flatten());
                painter.circle_filled(p, 3.5 * z, wire_color(t, v).0.gamma_multiply(on_path));
                if !name.is_empty() {
                    painter.text(
                        p - Vec2::new(6.0 * z, 0.0),
                        Align2::RIGHT_CENTER,
                        name,
                        font(10.0 * z),
                        t.muted.gamma_multiply(on_path),
                    );
                }
            }
            // A variable's value in its body (a constant's is its label).
            if matches!(n.kind, NodeKind::Read { .. } | NodeKind::Write { .. })
                && let Some(v) = live.and_then(|l| l.values[n.id.0 as usize].first().copied().flatten())
                && let Some(label) = value_text(d, n.id, v, vars)
            {
                let color = match v {
                    Value::Bool(true) => t.ink(brand::GREEN),
                    Value::Bool(false) => t.ink(brand::RED),
                    _ => t.fg,
                };
                // Elided to the block, never spilling out of it.
                let g = fitted(
                    painter,
                    &label,
                    font(11.0 * z),
                    color.gamma_multiply(on_path),
                    r.width() - 6.0 * z,
                );
                let mid = Pos2::new(r.center().x, h.max.y + (r.max.y - h.max.y) / 2.0);
                painter.galley(mid - g.size() / 2.0, g, color);
            }
        }
    }
    // Last, so what the Agent removed shows over whatever took its place.
    if let Some((old, removed)) = ghosts {
        draw_ghosts(painter, cam, old, removed);
    }
}

/// A one-scan statement stays lit (the Blink lamp's flicker).
#[cfg(test)]
#[path = "st_view_fold_tests.rs"]
mod fold_tests;

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

    /// Nick, 2026-11-11: a block with a diagram of its own is marked as
    /// one, so it shows that a double click goes deeper. A library block
    /// (a TON, a CTU) has none and is not.
    #[test]
    fn only_blocks_with_a_diagram_of_their_own_are_marked_as_opening() {
        let src = crate::box_tests::DEMO;
        let session = Session::new(src);
        assert!(session.diags.is_empty(), "{:?}", session.diag_lines());
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut v = Viewer::new(src, module);
        let d = v.diagram().clone();
        let fbs: Vec<(String, bool)> = d
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                // `openable` numbers blocks as they are drawn.
                NodeKind::Fb { instance, .. } => Some((
                    instance.to_ascii_uppercase(),
                    v.to_shown(n.id).is_some_and(|s| v.openable().contains(&s)),
                )),
                _ => None,
            })
            .collect();
        assert!(
            fbs.contains(&("M".into(), true)),
            "the motor block opens: {fbs:?}"
        );
        assert!(
            fbs.contains(&("BLINK".into(), false)),
            "a timer does not: {fbs:?}"
        );
        assert!(fbs.contains(&("PARTS".into(), false)), "nor a counter: {fbs:?}");
        // Every block marked opens, and inside the motor nothing does.
        for n in v.openable() {
            v.pick = v.to_base(n).map(super::Pick::Node);
            assert!(v.step_into(), "a marked block opens");
            assert!(v.openable().is_empty(), "the motor calls no block of its own");
            assert!(v.step_out());
        }
    }

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

    /// The lamp's assignment runs one scan in fifty. A frame of two scans
    /// draws the second, so half the time the flip is in the scan that is
    /// not drawn: it lit on some flips and not on others (Nick,
    /// 2026-11-11).
    #[test]
    fn a_one_scan_statement_lights_every_time_it_runs_whichever_scan_of_the_frame() {
        let mut session = Session::new(SRC);
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut v = Viewer::new(SRC, module);
        let d = v.diagram().clone();
        let lamp = d
            .nodes
            .iter()
            .position(|n| n.kind == NodeKind::Write { path: "LAMP".into() })
            .unwrap();
        session.set_running(true);
        session.advance(0.0);
        let (mut flips, mut lit, mut was_on, mut was_lit) = (0, 0, false, false);
        let mut both = [false; 2];
        // 20 ms frames of two 10 ms scans, 6 s: the lamp flips every 510 ms,
        // so in the first scan of a frame and in the second by turns.
        for frame in 1..=300 {
            let now = f64::from(frame) * 0.02;
            session.advance(now);
            for t in session.earlier() {
                v.saw(t, now);
            }
            let trace = session.trace.as_ref().unwrap();
            let on = session.value("lamp") == Some(helmsman_iec::Value::Bool(true));
            if on != was_on {
                flips += 1;
                both[usize::from(trace.get("pulse.Q") == Some(helmsman_iec::Value::Bool(true)))] = true;
            }
            was_on = on;
            let mut l = v.live(Some(trace as &dyn helmsman_diagram::Scan)).unwrap();
            v.hold(&mut l, now);
            lit += usize::from(l.ran[lamp] && !was_lit);
            was_lit = l.ran[lamp];
        }
        assert!(flips >= 10, "{flips} flips");
        assert_eq!(
            both, [true; 2],
            "flips in the drawn scan and in the one before it"
        );
        assert_eq!(lit, flips, "the assignment lit once per flip");
    }
}

/// What a block's value reads as (cards `period` value, node fills).
#[cfg(test)]
mod value_tests {
    use helmsman_diagram::NodeKind;
    use helmsman_iec::Value;

    use eframe::egui;

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

    /// A REAL is an f32 widened; the diagram, the I/O tab and the banners
    /// show it as the f32 (23.7, not 23.700000762939453).
    #[test]
    fn a_real_reads_short_and_a_long_value_is_elided_to_its_block() {
        let v = viewer();
        let d = v.diagram();
        let vars = v.module.program().map(|p| &p.vars);
        let plus = node(&v, &NodeKind::Op { op: "+".into() });
        let real = Value::Real(f64::from(23.7f32));
        assert_eq!(value_text(d, plus, real, vars).as_deref(), Some("23.7"));
        assert_eq!(super::show(Some(real)), "23.7");

        let ctx = egui::Context::default();
        let mut width = 0.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let g = super::fitted(
                ui.painter(),
                "0.30000000000000004000",
                egui::FontId::monospace(11.0),
                egui::Color32::WHITE,
                40.0,
            );
            width = g.size().x;
        });
        out.textures_delta.clear();
        assert!(width <= 40.5, "{width} pt wide in a 40 pt block");
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
            // Where it is drawn, in its header (an input's control sits
            // under it).
            let r = v.block_on_screen(n.id).unwrap();
            Pos2::new(
                r.center().x,
                r.min.y + (r.height() / 2.0).min(8.0 * v.cams[v.scope].zoom),
            )
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
