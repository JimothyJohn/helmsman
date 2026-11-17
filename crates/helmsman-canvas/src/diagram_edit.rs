//! Editing the program on its diagram, in the simulation: a right click on
//! a block or a pin opens a menu of what can change there, a double click
//! on a constant edits it in place. A choice becomes an
//! [`helmsman_diagram::edit::Edit`]; the engine turns it into an exact
//! splice of the source or refuses in plain words, and the host
//! (`playground_view`) writes it into the file and deploys it.
//!
//! What is offered is what the engine accepts: each entry is dry-run
//! against the diagram's source when the menu opens, so a menu never
//! lists a change that would be refused. A change that still is (a name
//! that is taken, a variable of the wrong type) is refused with the
//! engine's words, shown in the popup, and the source is untouched.
//!
//! The gestures are on only while [`Gate`] says so: the diagram must show
//! the code in the editor (a single program file, deployed as it stands).
//! On a box they are off ([`BOX_REASON`]) until the card
//! `diagram-edit-box`.

use eframe::egui::{self, Pos2, Rect, RichText, Vec2};
use helmsman_diagram::edit::{self, Bubble, Edit, Literal, Pin, Source};
use helmsman_diagram::layout::{PORT_ROW, in_port};
use helmsman_diagram::{NodeId, NodeKind, Span};
use helmsman_iec::ast::{self, BinOp};

use crate::editor::Chrome;
use crate::st_view::brand::RED;
use crate::st_view::{Cam, TAP, Viewer, node_near};
use crate::theme::{BODY, theme};
use crate::tips::Tip;

/// Why a box's diagram does not edit.
pub const BOX_REASON: &str = "Editing from the diagram works in the simulation for now. \
     On a box, change the code in the editor and Deploy.";

/// The longest a pin's pointer target is, in screen points.
const PIN_REACH: f32 = 16.0;
/// The tallest the variable list is, in points.
const LIST_H: f32 = 264.0;
/// The widest a popup is, in points.
const POPUP_W: f32 = 380.0;

/// What the host remembers between a diagram edit and what follows it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Follow {
    /// The next rebuilt viewer keeps the one it replaces: its cameras,
    /// its pick and the places of the blocks an edit renamed.
    pub keep_view: bool,
    /// What is deployed is what a diagram edit last wrote, or what Undo
    /// or Redo of it restored: Undo and Redo then deploy again.
    pub deployed: bool,
}

/// Whether the diagram's gestures change the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    On,
    /// Off, and why, in words for the person at the diagram.
    Off(String),
}

/// What a gesture was made on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Input `input` of block `node`.
    Pin {
        node: NodeId,
        input: usize,
    },
    Block(NodeId),
}

/// A gesture the host has not taken yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gesture {
    pub at: Pos2,
    pub hit: Hit,
    /// A double click on a constant: its value in place.
    pub in_place: bool,
}

impl Viewer {
    /// The pin or block under `p`: a pin within reach of an input port,
    /// else the nearest block.
    pub(crate) fn hit_at(&self, cam: Cam, p: Pos2) -> Option<Hit> {
        let (x, y) = cam.to_world(p);
        // On the diagram as drawn; a fold (several blocks as one) has
        // nothing to change until it is opened, but for a comparison
        // folded with its literal.
        let d = self.shown();
        let reach = (PIN_REACH / cam.zoom).max(PORT_ROW / 2.0);
        let pin = d
            .nodes
            .iter()
            .flat_map(|n| (0..n.ins.len()).map(move |k| (n, k)))
            .map(|(n, k)| {
                let (px, py) = in_port(n, k);
                (n.id, k, (px - x).hypot(py - y))
            })
            .filter(|(.., dist)| *dist <= reach)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((node, input, _)) = pin {
            return self
                .base_pin(node, input)
                .map(|(node, input)| Hit::Pin { node, input });
        }
        node_near(d, x, y, TAP / 2.0 / cam.zoom)
            .and_then(|n| self.base_block(n))
            .map(Hit::Block)
    }

    /// What io.map can do with the program port `node` reads (an input's
    /// Read block) or writes (an output's Write block), on the program's
    /// own diagram.
    pub(crate) fn port_block(&self, node: NodeId) -> Option<&crate::assign::PortMenu> {
        use crate::device_form::Dir;
        if !self.scopes[self.scope].prefix.is_empty() {
            return None;
        }
        let (path, dir) = match &self.diagram().node(node).kind {
            NodeKind::Read { path } => (path, Dir::In),
            NodeKind::Write { path } => (path, Dir::Out),
            _ => return None,
        };
        self.assign
            .iter()
            .find(|m| m.dir == dir && m.port.eq_ignore_ascii_case(path.trim()))
    }

    /// A right-click on an on-diagram control (an input's switch or force
    /// button sits over its Read block, so the diagram never sees the
    /// click) is a gesture on that block all the same.
    pub(crate) fn secondary_on_controls(&mut self, ui: &egui::Ui, controls: &[(String, Rect)]) {
        let Some(p) = ui.input(|i| {
            i.pointer
                .button_clicked(egui::PointerButton::Secondary)
                .then(|| i.pointer.interact_pos())
                .flatten()
        }) else {
            return;
        };
        if !controls.iter().any(|(_, r)| r.contains(p)) {
            return;
        }
        let cam = self.cams[self.scope];
        if let Some(hit) = self.hit_at(cam, p) {
            let (Hit::Block(n) | Hit::Pin { node: n, .. }) = hit;
            self.pick = Some(crate::st_view::Pick::Node(n));
            self.gesture_at(p, hit, false);
        }
    }

    /// The constant block under `p`.
    pub(crate) fn const_at(&self, cam: Cam, p: Pos2) -> Option<NodeId> {
        let (x, y) = cam.to_world(p);
        let n = self.shown().node_at(x, y)?;
        self.base_const(n)
    }

    /// Takes a gesture made at `at`: for the host when gestures are on,
    /// else a note of why they are not.
    pub(crate) fn gesture_at(&mut self, at: Pos2, hit: Hit, in_place: bool) {
        // A program port's block has its io.map items, and any block
        // "Chat about this" while the diagram's lines are the host's,
        // whatever the gate says: the menu then shows the code's items
        // off, with the reason.
        let port = !in_place && matches!(hit, Hit::Block(n) if self.port_block(n).is_some());
        match &self.edit_gate {
            Gate::Off(_) if port || (self.chat_ok && !in_place) => {
                self.off_note = None;
                self.gesture = Some(Gesture { at, hit, in_place });
            }
            Gate::On => {
                self.off_note = None;
                self.gesture = Some(Gesture { at, hit, in_place });
            }
            Gate::Off(why) => {
                self.gesture = None;
                self.off_note = Some((at, why.clone()));
            }
        }
    }

    /// What hovering a constant says: how to change it, or why that is off.
    pub(crate) fn edit_hover(&self, cam: Cam, p: Pos2) -> Option<String> {
        self.const_at(cam, p)?;
        Some(match &self.edit_gate {
            Gate::On => {
                "Double-click changes this value. Right-click shows what else can change here.".into()
            }
            Gate::Off(why) => why.clone(),
        })
    }

    /// The note of why a gesture did nothing, until the next click or Escape.
    pub(crate) fn off_note_ui(&mut self, ui: &egui::Ui, area: Rect) {
        let Some((at, why)) = self.off_note.clone() else {
            return;
        };
        let w = (area.width() - 24.0).clamp(120.0, POPUP_W);
        let r = egui::Area::new(egui::Id::unique("diagram-off-note"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .constrain_to(area)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(w);
                    ui.label(RichText::new(why).size(BODY));
                });
            });
        if crate::theme::dismissed(ui.ctx(), "diagram-off-note", &[r.response.rect]) {
            self.off_note = None;
        }
    }
}

/// The kind of value a constant holds, read from its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    Int,
    Real,
    Time,
}

impl ValueKind {
    /// What the literal `text` is.
    #[must_use]
    pub fn of(text: &str) -> Self {
        let t = text.trim().to_ascii_uppercase();
        if t == "TRUE" || t == "FALSE" {
            Self::Bool
        } else if t.starts_with("T#") || t.starts_with("TIME#") {
            Self::Time
        } else if t.contains('.') || t.contains('E') && !t.starts_with("16#") {
            Self::Real
        } else {
            Self::Int
        }
    }

    /// What to write, when `text` is not that.
    fn hint(self) -> &'static str {
        match self {
            Self::Bool => "Write TRUE or FALSE.",
            Self::Int => "Write a whole number, such as 40.",
            Self::Real => "Write a number, such as 2.5.",
            Self::Time => "Write a time, such as T#5s, T#1m30s or T#250ms.",
        }
    }
}

/// The value `text` says, as a constant of `kind`.
///
/// # Errors
/// What to write instead, in words.
pub fn parse_value(text: &str, kind: ValueKind) -> Result<Literal, String> {
    let t = text.trim();
    let bad = || format!("{t:?} is not a value of that kind. {}", kind.hint());
    if t.is_empty() {
        return Err(format!("No value was written. {}", kind.hint()));
    }
    match kind {
        ValueKind::Bool => match t.to_ascii_uppercase().as_str() {
            "TRUE" => Ok(Literal::Bool(true)),
            "FALSE" => Ok(Literal::Bool(false)),
            _ => Err(bad()),
        },
        ValueKind::Int => t
            .replace('_', "")
            .parse::<i128>()
            .map(Literal::Int)
            .map_err(|_| bad()),
        ValueKind::Real => t
            .replace('_', "")
            .parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .map(Literal::Real)
            .ok_or_else(bad),
        ValueKind::Time => parse_time(t).map(Literal::Time).ok_or_else(bad),
    }
}

/// `T#1h2m3s4ms`, in nanoseconds.
fn parse_time(t: &str) -> Option<i128> {
    let upper = t.to_ascii_uppercase();
    let rest = upper.strip_prefix("TIME#").or_else(|| upper.strip_prefix("T#"))?;
    let (neg, rest) = rest.strip_prefix('-').map_or((false, rest), |r| (true, r));
    let mut total: i128 = 0;
    let mut chars = rest.chars().filter(|c| *c != '_').peekable();
    let mut any = false;
    while chars.peek().is_some() {
        let mut num = String::new();
        while let Some(c) = chars.peek().copied().filter(|c| c.is_ascii_digit() || *c == '.') {
            num.push(c);
            chars.next();
        }
        let mut unit = String::new();
        while let Some(c) = chars.peek().copied().filter(char::is_ascii_alphabetic) {
            unit.push(c);
            chars.next();
        }
        let per: f64 = match unit.as_str() {
            "D" => 86_400e9,
            "H" => 3_600e9,
            "M" => 60e9,
            "S" => 1e9,
            "MS" => 1e6,
            "US" => 1e3,
            "NS" => 1.0,
            _ => return None,
        };
        let n: f64 = num.parse().ok()?;
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let ns = (n * per).round() as i128;
        total = total.checked_add(ns)?;
        any = true;
    }
    any.then_some(if neg { -total } else { total })
}

/// What a right click on something can do, each already dry-run.
#[derive(Default, Debug)]
struct Entries {
    /// The operators the block can become.
    operators: Vec<(BinOp, Edit)>,
    /// The inputs whose bubble can be toggled (the port's name).
    negate: Vec<(String, Edit)>,
    /// The pin a new source would go to.
    connect: Option<Pin>,
    /// The pin whose constant can change, and what the constant says now.
    value: Option<(Pin, String)>,
    /// The variable a rename is about.
    rename: Option<String>,
    delete: Option<Edit>,
    /// The port this block is, with what io.map offers on it.
    assign: Option<crate::assign::PortMenu>,
    /// The block "Chat about this" is about ([`crate::chat_about`]): the
    /// block hit, or the block of the pin hit.
    chat: Option<NodeId>,
    /// Why the code's own items are off, when they are and the menu is
    /// only here for `assign`.
    off: Option<String>,
}

impl Entries {
    fn is_empty(&self) -> bool {
        self.assign.is_none()
            && self.chat.is_none()
            && self.operators.is_empty()
            && self.negate.is_empty()
            && self.connect.is_none()
            && self.value.is_none()
            && self.rename.is_none()
            && self.delete.is_none()
    }
}

fn applies(v: &Viewer, e: &Edit) -> bool {
    edit::apply(&v.src, &v.module, e.clone()).is_ok()
}

/// The first name of a path (`T1.Q` and `A[2]` are about `T1` and `A`).
fn head(path: &str) -> &str {
    path.split(['.', '[']).next().unwrap_or(path).trim()
}

/// The source range of the statement block `node` was lowered from.
#[must_use]
pub fn statement_span(v: &Viewer, node: NodeId) -> Option<Span> {
    let pou = &v.module.pous[v.scopes[v.scope].pou];
    let d = &pou.diagram;
    let n = d.nodes.get(node.0 as usize)?;
    let stmt = d
        .stmts
        .iter()
        .find(|s| s.id == n.stmt)
        .or_else(|| d.stmts.iter().find(|s| s.roots.contains(&node)))?;
    let unit = ast::parse(&v.src).ok()?;
    let p = unit
        .pous
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&pou.name))?;
    let found = find_stmt(&p.body, stmt.pos)?;
    Some(Span {
        start: offset_in(&v.src, found.pos),
        end: offset_in(&v.src, found.end),
    })
}

fn find_stmt(body: &[ast::Stmt], at: ast::Pos) -> Option<&ast::Stmt> {
    for s in body {
        if s.pos == at {
            return Some(s);
        }
        let inner = match &s.kind {
            ast::StmtKind::If(conds, other) => conds
                .iter()
                .find_map(|(_, b)| find_stmt(b, at))
                .or_else(|| find_stmt(other, at)),
            ast::StmtKind::Case(_, arms, other) => arms
                .iter()
                .find_map(|(_, b)| find_stmt(b, at))
                .or_else(|| find_stmt(other, at)),
            ast::StmtKind::For { body, .. }
            | ast::StmtKind::While(_, body)
            | ast::StmtKind::Repeat(body, _) => find_stmt(body, at),
            _ => None,
        };
        if inner.is_some() {
            return inner;
        }
    }
    None
}

/// Byte offset of a 1-based line and column.
fn offset_in(s: &str, p: ast::Pos) -> usize {
    let mut at = 0;
    for _ in 1..p.line {
        match s.as_bytes()[at..].iter().position(|&b| b == b'\n') {
            Some(i) => at += i + 1,
            None => return s.len(),
        }
    }
    (at + (p.col as usize).saturating_sub(1)).min(s.len())
}

const COMPARISONS: [&str; 6] = ["=", "<>", "<", "<=", ">", ">="];

const ALL_OPS: [BinOp; 15] = [
    BinOp::Or,
    BinOp::Xor,
    BinOp::And,
    BinOp::Eq,
    BinOp::Ne,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Mod,
    BinOp::Pow,
];

fn entries(v: &Viewer, hit: Hit) -> Entries {
    let pou = v.module.pous[v.scopes[v.scope].pou].name.clone();
    let d = v.diagram();
    let mut out = Entries::default();
    let pin_of = |node: NodeId, input: usize| Pin {
        pou: pou.clone(),
        node,
        input,
    };
    let negation = |node: NodeId, input: usize| Edit::ToggleNegation(Bubble::Pin(pin_of(node, input)));
    let delete_of = |node: NodeId| {
        statement_span(v, node).map(|span| Edit::DeleteStmt {
            pou: pou.clone(),
            span,
        })
    };
    match hit {
        Hit::Pin { node, input } => {
            let e = negation(node, input);
            if applies(v, &e) {
                out.negate
                    .push((d.node(node).ins.get(input).cloned().unwrap_or_default(), e));
            }
            if let Some(w) = d.wires.iter().find(|w| w.to == node && w.inp == input) {
                let from = d.node(w.from);
                match &from.kind {
                    NodeKind::Const { .. } => {
                        out.value = Some((pin_of(node, input), from.text.clone()));
                        out.connect = Some(pin_of(node, input));
                    }
                    NodeKind::Read { .. } | NodeKind::Write { .. } => out.connect = Some(pin_of(node, input)),
                    _ => {}
                }
            }
            out.delete = delete_of(node).filter(|e| applies(v, e));
        }
        Hit::Block(node) => {
            let n = d.node(node);
            if let NodeKind::Op { op } = &n.kind {
                for to in ALL_OPS {
                    if to.symbol() == op {
                        continue;
                    }
                    let e = Edit::SwapOp {
                        pou: pou.clone(),
                        node,
                        to,
                    };
                    if applies(v, &e) {
                        out.operators.push((to, e));
                    }
                }
            }
            // A comparison against one literal is drawn as one block with
            // it (`> 10`): its menu changes the value too, or puts a
            // variable in its place.
            if matches!(&n.kind, NodeKind::Op { op } if COMPARISONS.contains(&op.as_str())) {
                let mut literals = d
                    .inputs(node)
                    .filter(|w| matches!(d.node(w.from).kind, NodeKind::Const { .. }));
                if let (Some(w), None) = (literals.next(), literals.next()) {
                    out.value = Some((pin_of(node, w.inp), d.node(w.from).text.clone()));
                    out.connect = Some(pin_of(node, w.inp));
                }
            }
            for (k, name) in n.ins.iter().enumerate() {
                let e = negation(node, k);
                if applies(v, &e) {
                    out.negate.push((name.clone(), e));
                }
            }
            match &n.kind {
                NodeKind::Const { .. } => {
                    if let Some(w) = d.wires.iter().find(|w| w.from == node) {
                        out.value = Some((pin_of(w.to, w.inp), n.text.clone()));
                        out.connect = Some(pin_of(w.to, w.inp));
                    }
                }
                NodeKind::Read { path } | NodeKind::Write { path } => {
                    let name = head(path).to_ascii_uppercase();
                    if v.module.pou(&pou).is_some_and(|p| p.vars.contains_key(&name)) {
                        out.rename = Some(name);
                    }
                }
                NodeKind::Fb { instance, .. } => {
                    let name = instance.to_ascii_uppercase();
                    if v.module.pou(&pou).is_some_and(|p| p.vars.contains_key(&name)) {
                        out.rename = Some(name);
                    }
                }
                _ => {}
            }
            out.delete = delete_of(node).filter(|e| applies(v, e));
            out.assign = v.port_block(node).cloned();
        }
    }
    let (Hit::Block(node) | Hit::Pin { node, .. }) = hit;
    out.chat = v.chat_ok.then_some(node);
    // Edits are off (a box): only what io.map takes and the chat stay.
    if let Gate::Off(why) = &v.edit_gate {
        out = Entries {
            assign: out.assign.take(),
            chat: out.chat.take(),
            off: Some(why.clone()),
            ..Entries::default()
        };
    }
    out
}

/// The variables a pin can be connected to, in declaration order: name
/// and type.
fn variables(v: &Viewer) -> Vec<(String, String)> {
    let Some(p) = v.module.pou(&v.module.pous[v.scopes[v.scope].pou].name) else {
        return Vec::new();
    };
    let mut vars: Vec<(&String, &helmsman_diagram::lower::Var)> = p
        .vars
        .iter()
        .filter(|(_, x)| x.fb.is_none() && !x.array)
        .collect();
    vars.sort_by(|a, b| (a.1.line, a.0).cmp(&(b.1.line, b.0)));
    vars.into_iter().map(|(n, x)| (n.clone(), x.ty.clone())).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    Menu,
    Operators,
    Negate,
    Connect(String),
    Value(String),
    Rename(String),
}

struct Open {
    at: Pos2,
    stage: Stage,
    entries: Entries,
    kind: ValueKind,
    error: Option<String>,
    focus: bool,
}

/// The menu and the small popups the gestures open.
#[derive(Default)]
pub struct EditUi {
    open: Option<Open>,
    /// A port's assignment asked for in the menu, for the host to take.
    assign: Option<(String, crate::assign_ui::PortAsk)>,
    /// "Chat about this" chosen in the menu, for the host to take.
    chat: Option<NodeId>,
    /// The Agent tab holds as many chips as it takes: the menu item is
    /// off and says why. The host sets it before [`Self::show`].
    pub chat_full: bool,
}

impl std::fmt::Debug for EditUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditUi")
            .field("open", &self.stage_name())
            .finish()
    }
}

impl EditUi {
    /// Opens the menu (or, for a double click on a constant, its value
    /// field) for a gesture of `v`'s diagram.
    pub fn open(&mut self, v: &Viewer, g: Gesture) {
        let entries = entries(v, g.hit);
        let kind = entries
            .value
            .as_ref()
            .map_or(ValueKind::Int, |(_, text)| ValueKind::of(text));
        let stage = match (&entries.value, g.in_place) {
            (Some((_, text)), true) => Stage::Value(text.clone()),
            _ => Stage::Menu,
        };
        self.open = Some(Open {
            at: g.at,
            stage,
            entries,
            kind,
            error: None,
            focus: true,
        });
    }

    /// Which popup is open, for the tests.
    #[must_use]
    pub fn stage_name(&self) -> &'static str {
        match self.open.as_ref().map(|o| &o.stage) {
            None => "closed",
            Some(Stage::Menu) => "menu",
            Some(Stage::Operators) => "operators",
            Some(Stage::Negate) => "negate",
            Some(Stage::Connect(_)) => "connect",
            Some(Stage::Value(_)) => "value",
            Some(Stage::Rename(_)) => "rename",
        }
    }

    /// The words the popup shows for a refused edit.
    #[cfg(test)]
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.open.as_ref()?.error.as_deref()
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// The block the menu asked to chat about, once; the menu is closed
    /// then.
    pub fn take_chat(&mut self) -> Option<NodeId> {
        self.chat.take()
    }

    /// The port the menu asked to assign (or put back on simulated I/O),
    /// once; the menu is closed then.
    pub fn take_assign(&mut self) -> Option<(String, crate::assign_ui::PortAsk)> {
        self.assign.take()
    }

    /// The host could not make the edit: say why and keep the popup.
    pub fn fail(&mut self, why: String) {
        if let Some(o) = &mut self.open {
            o.error = Some(why);
        }
    }

    /// Draws the open popup inside `area`; the edit asked for, if one was.
    pub fn show(&mut self, ctx: &egui::Context, chrome: &mut Chrome, area: Rect, v: &Viewer) -> Option<Edit> {
        let mut o = self.open.take()?;
        let width = (area.width() - 24.0).clamp(120.0, POPUP_W);
        let pou = v.module.pous[v.scopes[v.scope].pou].name.clone();
        let mut asked: Option<Edit> = None;
        let mut close = false;
        let mut assign: Option<(String, crate::assign_ui::PortAsk)> = None;
        let mut chat: Option<NodeId> = None;
        let chat_full = self.chat_full;
        let vars = if matches!(o.stage, Stage::Connect(_)) {
            variables(v)
        } else {
            Vec::new()
        };
        let r = egui::Area::new(egui::Id::unique("diagram-edit-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(o.at)
            .constrain_to(area)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(width);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let stage = o.stage.clone();
                    match stage {
                        Stage::Menu => {
                            let e = &o.entries;
                            let mut next: Option<Stage> = None;
                            if let Some(why) = &e.off {
                                ui.label(RichText::new(why).size(BODY));
                            }
                            if let Some(m) = &e.assign {
                                if row_on(
                                    ui,
                                    chrome,
                                    "diagram menu assign",
                                    "Assign to device I/O…",
                                    m.assign.is_ok(),
                                    m.assign.as_ref().err().map_or(
                                        "Pick a free channel of a device, of the same direction and type, added to the project or not yet. Its line in io.map is named for this variable.",
                                        String::as_str,
                                    ),
                                ) {
                                    assign = Some((m.port.clone(), crate::assign_ui::PortAsk::Assign));
                                }
                                if let Some(back) = &m.back
                                    && row_on(
                                        ui,
                                        chrome,
                                        "diagram menu back to simulated",
                                        "Back to simulated I/O",
                                        back.is_ok(),
                                        back.as_ref().err().map_or(
                                            "Puts this back on a simulated input or output. The device channel goes back under + Add I/O, as Remove leaves it.",
                                            String::as_str,
                                        ),
                                    )
                                {
                                    assign = Some((m.port.clone(), crate::assign_ui::PortAsk::Back));
                                }
                            }
                            if e.is_empty() {
                                ui.label(RichText::new("Nothing here can be changed from the diagram.").size(BODY));
                            }
                            if let Some(n) = e.chat
                                && row_on(
                                    ui,
                                    chrome,
                                    "diagram menu chat",
                                    "Chat about this",
                                    !chat_full,
                                    if chat_full {
                                        crate::chat_about::FULL
                                    } else {
                                        "Puts this block and its statement in the Agent tab, with what drives it and what it drives. Nothing is sent until you type what to ask and send it."
                                    },
                                )
                            {
                                chat = Some(n);
                            }
                            if !e.operators.is_empty()
                                && row(
                                    ui,
                                    chrome,
                                    "diagram menu operator",
                                    "Change operator ▸",
                                    "Swap this block's operator for another of its kind.",
                                )
                            {
                                next = Some(Stage::Operators);
                            }
                            match e.negate.as_slice() {
                                [] => {}
                                [(name, edit)] => {
                                    let text = if name.is_empty() {
                                        "Negate input".to_string()
                                    } else {
                                        format!("Negate input {name}")
                                    };
                                    if row(
                                        ui,
                                        chrome,
                                        "diagram menu negate",
                                        &text,
                                        "Put a NOT in front of this input, or take it off.",
                                    ) {
                                        asked = Some(edit.clone());
                                    }
                                }
                                _ => {
                                    if row(
                                        ui,
                                        chrome,
                                        "diagram menu negate",
                                        "Negate input ▸",
                                        "Choose the input to put a NOT in front of, or to take it off.",
                                    ) {
                                        next = Some(Stage::Negate);
                                    }
                                }
                            }
                            if e.connect.is_some()
                                && row(
                                    ui,
                                    chrome,
                                    "diagram menu connect",
                                    "Connect to…",
                                    "Wire this input from another variable of this program.",
                                )
                            {
                                next = Some(Stage::Connect(String::new()));
                            }
                            if let Some((_, text)) = &e.value
                                && row(
                                    ui,
                                    chrome,
                                    "diagram menu value",
                                    "Edit value…",
                                    "Change the constant that feeds this input.",
                                )
                            {
                                next = Some(Stage::Value(text.clone()));
                            }
                            if let Some(name) = &e.rename
                                && row(
                                    ui,
                                    chrome,
                                    "diagram menu rename",
                                    "Rename…",
                                    "Give this variable another name, in every place this program uses it.",
                                )
                            {
                                next = Some(Stage::Rename(name.clone()));
                            }
                            if let Some(edit) = &e.delete
                                && row(
                                    ui,
                                    chrome,
                                    "diagram menu delete",
                                    "Delete statement",
                                    "Remove the whole statement this block belongs to from the code.",
                                )
                            {
                                asked = Some(edit.clone());
                            }
                            if let Some(s) = next {
                                o.stage = s;
                            }
                        }
Stage::Operators => {
                            if back(ui, chrome) {
                                o.stage = Stage::Menu;
                            }
                            for (op, edit) in &o.entries.operators {
                                if row(
                                    ui,
                                    chrome,
                                    &format!("diagram op {}", op.symbol()),
                                    op.symbol(),
                                    "Make this block this operator.",
                                ) {
                                    asked = Some(edit.clone());
                                }
                            }
                        }
                        Stage::Negate => {
                            if back(ui, chrome) {
                                o.stage = Stage::Menu;
                            }
                            for (name, edit) in &o.entries.negate {
                                if row(
                                    ui,
                                    chrome,
                                    &format!("diagram negate {name}"),
                                    name,
                                    "Put a NOT in front of this input, or take it off.",
                                ) {
                                    asked = Some(edit.clone());
                                }
                            }
                        }
                        Stage::Connect(mut query) => {
                            if back(ui, chrome) {
                                o.stage = Stage::Menu;
                                return;
                            }
                            let field = ui.add(
                                egui::TextEdit::singleline(&mut query)
                                    .hint_text("Search variables")
                                    .desired_width(ui.available_width()),
                            )
                            .tip("Type part of a variable's name. A path such as T1.Q can be typed whole.");
                            chrome.hit("diagram connect search", field.rect);
                            if std::mem::take(&mut o.focus) {
                                field.request_focus();
                            }
                            let q = query.trim().to_ascii_uppercase();
                            let shown: Vec<&(String, String)> =
                                vars.iter().filter(|(n, _)| q.is_empty() || n.contains(&q)).collect();
                            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let pin = o.entries.connect.clone();
                            egui::ScrollArea::vertical()
                                .id_salt("diagram-connect-list")
                                .max_height(LIST_H.min(area.height() * 0.6))
                                .show(ui, |ui| {
                                    for (name, ty) in &shown {
                                        if row(
                                            ui,
                                            chrome,
                                            &format!("diagram connect {name}"),
                                            &format!("{name}   {ty}"),
                                            "Wire this input from this variable.",
                                        ) && let Some(pin) = &pin
                                        {
                                            asked = Some(Edit::Rewire {
                                                pin: pin.clone(),
                                                to: Source::Var(name.clone()),
                                            });
                                        }
                                    }
                                });
                            let typed = query.trim().to_string();
                            if !typed.is_empty() && !shown.iter().any(|(n, _)| n.eq_ignore_ascii_case(&typed)) {
                                let go = row(
                                    ui,
                                    chrome,
                                    "diagram connect typed",
                                    &format!("Use {typed}"),
                                    "Wire this input from the path as typed.",
                                );
                                if (go || enter && shown.is_empty())
                                    && let Some(pin) = &pin
                                {
                                    asked = Some(Edit::Rewire {
                                        pin: pin.clone(),
                                        to: Source::Var(typed),
                                    });
                                }
                            } else if enter
                                && let (Some((name, _)), Some(pin)) = (shown.first(), &pin)
                            {
                                asked = Some(Edit::Rewire {
                                    pin: pin.clone(),
                                    to: Source::Var(name.clone()),
                                });
                            }
                            o.stage = Stage::Connect(query);
                        }
                        Stage::Value(mut text) => {
                            ui.label(RichText::new("Value").size(BODY));
                            let field = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()))
                                .tip("The constant's new value. Enter sets it, Escape cancels.");
                            chrome.hit("diagram value field", field.rect);
                            if std::mem::take(&mut o.focus) {
                                select_all(ui.ctx(), field.id, text.chars().count());
                                field.request_focus();
                            }
                            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let mut set = enter;
                            ui.horizontal_wrapped(|ui| {
                                set |= row_w(
                                    ui,
                                    chrome,
                                    "diagram value ok",
                                    "Set",
                                    "Write this value into the code.",
                                );
                                if row_w(
                                    ui,
                                    chrome,
                                    "diagram value cancel",
                                    "Cancel",
                                    "Leave the value as it is.",
                                ) {
                                    close = true;
                                }
                            });
                            if set {
                                match (parse_value(&text, o.kind), &o.entries.value) {
                                    (Ok(value), Some((pin, _))) => {
                                        asked = Some(Edit::SetConst {
                                            pin: pin.clone(),
                                            value,
                                        });
                                    }
                                    (Err(why), _) => o.error = Some(why),
                                    (Ok(_), None) => {}
                                }
                            }
                            o.stage = Stage::Value(text);
                        }
                        Stage::Rename(mut text) => {
                            ui.label(RichText::new("New name").size(BODY));
                            let field = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()))
                                .tip("The variable's new name, in every place this program uses it. Enter renames, Escape cancels.");
                            chrome.hit("diagram rename field", field.rect);
                            if std::mem::take(&mut o.focus) {
                                select_all(ui.ctx(), field.id, text.chars().count());
                                field.request_focus();
                            }
                            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let mut set = enter;
                            ui.horizontal_wrapped(|ui| {
                                set |= row_w(
                                    ui,
                                    chrome,
                                    "diagram rename ok",
                                    "Rename",
                                    "Rename the variable in the code.",
                                );
                                if row_w(
                                    ui,
                                    chrome,
                                    "diagram rename cancel",
                                    "Cancel",
                                    "Leave the name as it is.",
                                ) {
                                    close = true;
                                }
                            });
                            if set && let Some(from) = &o.entries.rename {
                                asked = Some(Edit::Rename {
                                    pou: pou.clone(),
                                    from: from.clone(),
                                    to: text.trim().to_string(),
                                });
                            }
                            o.stage = Stage::Rename(text);
                        }
                    }
                    if let Some(e) = &o.error {
                        let red = theme(ui).ink(RED);
                        ui.label(RichText::new(e).size(BODY).color(red));
                    }
                });
            });
        chrome.hit("diagram popup", r.response.rect);
        if assign.is_some() {
            self.assign = assign;
            return None;
        }
        if chat.is_some() {
            self.chat = chat;
            return None;
        }
        if close || crate::theme::dismissed(ctx, "diagram-edit-popup", &[r.response.rect]) {
            return None;
        }
        // A new try clears the last refusal; the host sets it again.
        if asked.is_some() {
            o.error = None;
        }
        self.open = Some(o);
        asked
    }
}

/// Selects the `len` characters of the text field `id`, so typing
/// replaces what it holds.
fn select_all(ctx: &egui::Context, id: egui::Id, len: usize) {
    use egui::text::{CCursor, CCursorRange};
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(len))));
    state.store(ctx, id);
}

/// A full-width row of a menu, big enough for a finger.
fn row(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, tip: &str) -> bool {
    let w = ui.available_width();
    let r = ui
        .add(egui::Button::new(RichText::new(text).size(BODY)).min_size(Vec2::new(w, TAP)))
        .tip(tip);
    chrome.hit(key, r.rect);
    r.clicked()
}

/// `row`, off when `on` is false.
fn row_on(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, on: bool, tip: &str) -> bool {
    let w = ui.available_width();
    let r = ui
        .add_enabled(
            on,
            egui::Button::new(RichText::new(text).size(BODY)).min_size(Vec2::new(w, TAP)),
        )
        .tip(tip);
    chrome.hit(key, r.rect);
    r.clicked()
}

/// A button of a row of buttons.
fn row_w(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, tip: &str) -> bool {
    let r = ui
        .add(egui::Button::new(RichText::new(text).size(BODY)).min_size(Vec2::new(TAP * 1.5, TAP)))
        .tip(tip);
    chrome.hit(key, r.rect);
    r.clicked()
}

fn back(ui: &mut egui::Ui, chrome: &mut Chrome) -> bool {
    row(ui, chrome, "diagram menu back", "◂ Back", "Back to the menu.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_as_the_kind_of_the_constant() {
        assert_eq!(parse_value("40", ValueKind::Int), Ok(Literal::Int(40)));
        assert_eq!(parse_value(" -3 ", ValueKind::Int), Ok(Literal::Int(-3)));
        assert_eq!(parse_value("2.5", ValueKind::Real), Ok(Literal::Real(2.5)));
        assert_eq!(parse_value("true", ValueKind::Bool), Ok(Literal::Bool(true)));
        assert_eq!(
            parse_value("T#1m30s", ValueKind::Time),
            Ok(Literal::Time(90_000_000_000))
        );
        assert_eq!(
            parse_value("t#250ms", ValueKind::Time),
            Ok(Literal::Time(250_000_000))
        );
        assert_eq!(
            parse_value("T#1h2m3s4ms", ValueKind::Time),
            Ok(Literal::Time(3_723_004_000_000))
        );
    }

    #[test]
    fn a_value_that_is_not_of_the_kind_says_what_to_write() {
        for (text, kind) in [
            ("", ValueKind::Int),
            ("abc", ValueKind::Int),
            ("2.5", ValueKind::Int),
            ("1e999", ValueKind::Real),
            ("nan", ValueKind::Real),
            ("inf", ValueKind::Real),
            ("1", ValueKind::Bool),
            ("5s", ValueKind::Time),
            ("T#", ValueKind::Time),
            ("T#5", ValueKind::Time),
            ("T#5x", ValueKind::Time),
            ("T#-", ValueKind::Time),
            ("99999999999999999999999999999999999999999999", ValueKind::Int),
        ] {
            let why = parse_value(text, kind).unwrap_err();
            assert!(why.contains("Write"), "{text:?} as {kind:?}: {why}");
        }
    }

    #[test]
    fn a_constant_is_known_by_its_text() {
        assert_eq!(ValueKind::of("TRUE"), ValueKind::Bool);
        assert_eq!(ValueKind::of("T#5s"), ValueKind::Time);
        assert_eq!(ValueKind::of("2.5"), ValueKind::Real);
        assert_eq!(ValueKind::of("40"), ValueKind::Int);
    }
}

/// The diagram of a box has no gestures: the right click and the double
/// click on a constant say so, in words, and make no popup.
#[cfg(test)]
mod box_tests {
    use eframe::egui::{self, Event, PointerButton, Pos2, RawInput, Rect, Vec2};
    use helmsman_diagram::NodeKind;

    use super::{BOX_REASON, Gate};
    use crate::playground::Session;
    use crate::st_view::Viewer;
    use helmsman_diagram::Scan;

    const SRC: &str = "PROGRAM p\nVAR\n  a AT %IX0.0 : BOOL;\n  q AT %QX0.0 : BOOL;\n  n : INT;\nEND_VAR\nq := a AND n > 3;\nEND_PROGRAM\n";

    fn frames(ctx: &egui::Context, v: &mut Viewer, s: &Session, events: Vec<Event>, t: f64) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let scan = s.trace.as_ref().map(|t| t as &dyn Scan);
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::no_frame().show(ui, |ui| v.canvas(ui, scan));
        });
        out.textures_delta.clear();
    }

    fn click(
        ctx: &egui::Context,
        v: &mut Viewer,
        s: &Session,
        at: Pos2,
        button: PointerButton,
        times: usize,
    ) {
        let b = |pressed| Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut t = 1.0;
        frames(ctx, v, s, vec![Event::PointerMoved(at)], t);
        for _ in 0..times {
            t += 0.02;
            frames(ctx, v, s, vec![b(true)], t);
            t += 0.02;
            frames(ctx, v, s, vec![b(false)], t);
        }
        frames(ctx, v, s, vec![], t + 0.02);
    }

    fn setup() -> (egui::Context, Session, Viewer) {
        let session = Session::new(SRC);
        let module = session.build.as_ref().unwrap().module.clone().unwrap();
        let mut v = Viewer::new(SRC, module);
        let ctx = egui::Context::default();
        frames(&ctx, &mut v, &session, vec![], 0.0);
        frames(&ctx, &mut v, &session, vec![], 0.1);
        (ctx, session, v)
    }

    fn centre(v: &Viewer, f: impl Fn(&NodeKind) -> bool) -> Pos2 {
        let n = v.diagram().nodes.iter().find(|n| f(&n.kind)).unwrap();
        v.block_on_screen(n.id).unwrap().center()
    }

    #[test]
    fn a_viewer_starts_with_its_gestures_off_and_says_why() {
        let (ctx, s, mut v) = setup();
        assert_eq!(v.edit_gate, Gate::Off(BOX_REASON.into()));
        let at = centre(&v, |k| matches!(k, NodeKind::Op { op } if op == "AND"));
        click(&ctx, &mut v, &s, at, PointerButton::Secondary, 1);
        assert!(v.gesture.is_none(), "no menu is asked for");
        assert_eq!(v.off_note.as_ref().map(|(_, t)| t.as_str()), Some(BOX_REASON));
        assert!(v.pick.is_some(), "the click still picks the block");
    }

    /// A host that says the diagram's lines are its own (`chat_ok`) gets
    /// the menu while the edits are off: it holds "Chat about this" and
    /// says why the rest is not there.
    #[test]
    fn a_host_that_owns_the_lines_gets_the_menu_with_the_edits_off() {
        let (ctx, s, mut v) = setup();
        v.chat_ok = true;
        let at = centre(&v, |k| matches!(k, NodeKind::Op { op } if op == "AND"));
        click(&ctx, &mut v, &s, at, PointerButton::Secondary, 1);
        assert!(v.gesture.is_some(), "the menu is asked for");
        assert!(v.off_note.is_none());
    }

    #[test]
    fn a_double_click_on_a_constant_does_not_open_a_value_field_when_off() {
        let (ctx, s, mut v) = setup();
        let at = centre(&v, |k| matches!(k, NodeKind::Const { .. }));
        click(&ctx, &mut v, &s, at, PointerButton::Primary, 2);
        assert!(v.gesture.is_none());
        assert_eq!(v.off_note.as_ref().map(|(_, t)| t.as_str()), Some(BOX_REASON));
    }

    #[test]
    fn with_the_gate_on_the_same_gestures_are_handed_to_the_host() {
        let (ctx, s, mut v) = setup();
        v.edit_gate = Gate::On;
        let at = centre(&v, |k| matches!(k, NodeKind::Op { op } if op == "AND"));
        click(&ctx, &mut v, &s, at, PointerButton::Secondary, 1);
        assert!(v.gesture.is_some());
        assert!(v.off_note.is_none());
    }
}
