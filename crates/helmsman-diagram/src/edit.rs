//! Editing the program from its diagram: a gesture on the diagram becomes
//! an exact splice of the Structured Text, which stays the source of truth.
//! The parse tree drops comments and upper-cases names, so nothing here
//! prints it back: an edit is a byte range of the source and the text that
//! replaces it, and every byte outside that range is untouched.
//!
//! [`apply`] takes the source, the [`Module`] it was lowered to and an
//! [`Edit`], and gives a [`Patch`] or a [`Refusal`] in plain words. It is
//! pure (no I/O, no clock) and depends only on helmsman-iec, so the canvas
//! calls it in the browser as well as natively.
//!
//! The first edits:
//!
//! - [`Edit::SetConst`]: the value of a literal (a number, `TRUE`, a TIME
//!   preset such as `PT := T#5s`).
//! - [`Edit::ToggleNegation`]: the bubble on an input pin or on an IF,
//!   ELSIF, WHILE or REPEAT gate, as a `NOT` put in front of the operand or
//!   taken off it.
//! - [`Edit::SwapOp`]: AND for OR, one comparison for another, `+ - * /`
//!   for each other.
//!
//! # Where the text is
//!
//! Lowering records the source range of every input pin
//! ([`crate::Diagram::pins`]: `(node, input)` to the operand's text, its
//! `NOT`s and parentheses inside), of every gate ([`crate::Diagram::gates`])
//! and of every binary operator block ([`crate::Diagram::ops`]). The ranges
//! come from the parser's `start` and `end` on each expression, converted
//! from line and column (bytes, 1-based) to byte offsets. A read wired from
//! an earlier write keeps the text it has in its own statement.
//!
//! # Brackets
//!
//! Only where precedence needs them (the levels of [`crate::text::expr`]).
//! A swapped operator puts brackets round an operand, or round itself in
//! its parent, when the new precedence would regroup the expression; a
//! `NOT` is put in front of a bare comparison as `NOT (a > b)`; taking the
//! `NOT` off `NOT (a > b)` takes the brackets too when nothing needs
//! them. Brackets the user wrote are otherwise left alone.
//!
//! # Checked before it is returned
//!
//! The patch is spliced into the source, the result compiled and lowered
//! again, and the two diagrams compared block by block (and with
//! [`crate::diff::modules`]): the edit must have changed exactly the block
//! it was addressed to (and the blocks of the same statement that contain
//! its text), flipped exactly the bubble asked for, and nothing else.
//! Otherwise it is refused, with the reason.

use std::collections::BTreeSet;
use std::fmt;

use helmsman_iec::ast::{self, BinOp, Expr, ExprKind, Pos, StmtKind, UnOp};
use helmsman_iec::trace::StmtId;
use helmsman_iec::{ImageSizes, Value, compile};

use crate::diff;
use crate::lower::{Module, Pou, lower};
use crate::model::{Diagram, NodeId, NodeKind, Rect, RegionId, Span, Wire};
use crate::text::{UNARY, level, prec};

/// An input pin: input `input` of block `node` in the POU `pou`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The POU's name, in any case.
    pub pou: String,
    pub node: NodeId,
    pub input: usize,
}

/// A literal value to write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Literal {
    Int(i128),
    Real(f64),
    Bool(bool),
    /// A TIME in nanoseconds, written `T#5s`.
    Time(i128),
}

/// A negation bubble: on an input pin, or on a region's gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bubble {
    Pin(Pin),
    /// The condition of an IF, ELSIF, WHILE or REPEAT region.
    Gate {
        pou: String,
        region: RegionId,
    },
}

/// What a gesture asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// The literal feeding `pin` becomes `value`.
    SetConst { pin: Pin, value: Literal },
    /// The bubble is added where there is none and removed where there is.
    ToggleNegation(Bubble),
    /// The binary operator block `node` becomes `to`.
    SwapOp { pou: String, node: NodeId, to: BinOp },
}

/// Replace the bytes `start..end` of the source with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Patch {
    /// `src` with the patch applied.
    ///
    /// # Panics
    /// If the range is not on character boundaries inside `src`, which
    /// [`apply`] never gives for the `src` it was asked about.
    #[must_use]
    pub fn apply_to(&self, src: &str) -> String {
        format!("{}{}{}", &src[..self.start], self.text, &src[self.end..])
    }

    /// The patch that undoes this one on the result: `src` is the source
    /// this patch was made for.
    ///
    /// # Panics
    /// As [`Patch::apply_to`].
    #[must_use]
    pub fn inverse(&self, src: &str) -> Self {
        Self {
            start: self.start,
            end: self.start + self.text.len(),
            text: src[self.start..self.end].to_string(),
        }
    }
}

/// Why an edit was not made, in words for the person at the diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NoSuchPou(String),
    NoSuchBlock,
    NoSuchInput,
    NoSuchRegion,
    /// The diagram was not lowered from this source text.
    StaleDiagram,
    /// The source as it stands does not compile (the reason).
    SourceDoesNotCompile(String),
    /// The input has no text of its own (a block output feeds it).
    NoText,
    /// The input is not a plain number, TRUE/FALSE or time (what it is).
    NotAValue(String),
    BadValue(String),
    /// The region has no condition (ELSE, CASE, FOR).
    NoCondition,
    /// A constant has no bubble.
    NoBubble,
    /// A NOT there would be a bitwise complement, drawn as a block.
    NotBoolean,
    NotAnOperator,
    NotSwappable(String),
    WrongFamily {
        from: String,
        to: String,
    },
    NothingToChange,
    /// The text at that place did not read back as expected.
    Unreadable,
    /// The result does not compile (the compiler's words).
    WouldNotCompile(String),
    /// The result would differ from the diagram by more than the edit.
    WouldChangeMore(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchPou(p) => write!(f, "there is no program or block called {p} in this source"),
            Self::NoSuchBlock => write!(f, "that block is not in the diagram"),
            Self::NoSuchInput => write!(f, "that block has no such input"),
            Self::NoSuchRegion => write!(f, "that region is not in the diagram"),
            Self::StaleDiagram => write!(
                f,
                "the diagram was drawn from different text than the source being edited; \
                 redraw it from the current source first"
            ),
            Self::SourceDoesNotCompile(m) => {
                write!(
                    f,
                    "the source does not compile ({m}), so it cannot be edited from the diagram"
                )
            }
            Self::NoText => write!(
                f,
                "that input has no text of its own in the source (a function block output feeds it)"
            ),
            Self::NotAValue(what) => write!(
                f,
                "that input is not a plain number, TRUE/FALSE or time: it is {what}"
            ),
            Self::BadValue(m) => write!(f, "that value cannot be written: {m}"),
            Self::NoCondition => write!(
                f,
                "that region has no condition to negate (only IF, ELSIF, WHILE and REPEAT have one)"
            ),
            Self::NoBubble => write!(f, "a constant has no negation bubble: change its value instead"),
            Self::NotBoolean => write!(
                f,
                "that input is not a boolean: NOT on a number is a bitwise complement, \
                 a block of its own, not a bubble"
            ),
            Self::NotAnOperator => write!(f, "that block is not a two-input operator"),
            Self::NotSwappable(op) => write!(
                f,
                "{op} cannot be swapped here: only AND and OR, the comparisons and + - * / can"
            ),
            Self::WrongFamily { from, to } => {
                write!(f, "{from} cannot become {to}: pick one of its own kind")
            }
            Self::NothingToChange => write!(f, "the source already says that"),
            Self::Unreadable => write!(f, "the source text at that place could not be read back"),
            Self::WouldNotCompile(m) => {
                write!(f, "the edit would leave a program that does not compile ({m})")
            }
            Self::WouldChangeMore(m) => write!(f, "the edit would change more than asked ({m})"),
        }
    }
}

impl std::error::Error for Refusal {}

/// Image areas big enough that no address in a program the box accepts is
/// out of range when it is compiled for checking.
const SIZES: ImageSizes = ImageSizes {
    i: 1 << 24,
    q: 1 << 24,
    m: 1 << 24,
};

/// Makes the [`Patch`] for `edit` of `src`, which `module` was lowered from.
///
/// # Errors
/// A [`Refusal`]: the edit does not apply to what it is addressed to, the
/// diagram is not of this source, or the checked result would not compile
/// or would differ from the diagram in more than the edit.
pub fn apply(src: &str, module: &Module, edit: Edit) -> Result<Patch, Refusal> {
    let fresh = lowered(src).map_err(Refusal::SourceDoesNotCompile)?;
    if plain(&fresh) != plain(module) {
        return Err(Refusal::StaleDiagram);
    }
    let plan = match &edit {
        Edit::SetConst { pin, value } => set_const(src, module, pin, *value)?,
        Edit::ToggleNegation(Bubble::Pin(pin)) => toggle_pin(src, module, pin)?,
        Edit::ToggleNegation(Bubble::Gate { pou, region }) => toggle_gate(src, module, pou, *region)?,
        Edit::SwapOp { pou, node, to } => swap_op(src, module, pou, *node, *to)?,
    };
    let old = slice(src, plan.span)?;
    let new = splice(old, plan.splices)?;
    let patch = trim(plan.span.start, old, &new);
    if patch.start == patch.end && patch.text.is_empty() {
        return Err(Refusal::NothingToChange);
    }
    verify(module, &patch.apply_to(src), &plan.expect)?;
    Ok(patch)
}

/// What the re-lowered diagram must show.
struct Expect {
    pou: String,
    target: Target,
}

enum Target {
    /// The literal block now holds this value.
    Const { node: NodeId, value: Literal },
    /// The bubble on input `pin` of `node` flipped.
    Pin { node: NodeId, pin: usize },
    /// The operator block is now `to`.
    Op { node: NodeId, to: BinOp },
    /// The region's bubble flipped.
    Gate { region: RegionId },
}

impl Target {
    const fn node(&self) -> Option<NodeId> {
        match self {
            Self::Const { node, .. } | Self::Pin { node, .. } | Self::Op { node, .. } => Some(*node),
            Self::Gate { .. } => None,
        }
    }
}

struct Plan {
    /// The operand or expression the splices are relative to.
    span: Span,
    splices: Vec<Splice>,
    expect: Expect,
}

/// Replace `del` bytes at `at` (relative to the plan's span) with `ins`.
struct Splice {
    at: usize,
    del: usize,
    ins: String,
}

fn splice(text: &str, mut ops: Vec<Splice>) -> Result<String, Refusal> {
    // Stable: insertions made in order at one place stay in order.
    ops.sort_by_key(|s| s.at);
    let mut out = String::with_capacity(text.len() + 8);
    let mut cur = 0;
    for s in &ops {
        if s.at < cur {
            return Err(Refusal::Unreadable);
        }
        out.push_str(text.get(cur..s.at).ok_or(Refusal::Unreadable)?);
        out.push_str(&s.ins);
        cur = s.at + s.del;
    }
    out.push_str(text.get(cur..).ok_or(Refusal::Unreadable)?);
    Ok(out)
}

/// The smallest patch turning `old` into `new`, from byte `at`.
fn trim(at: usize, old: &str, new: &str) -> Patch {
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut p = 0;
    while p < ob.len() && p < nb.len() && ob[p] == nb[p] {
        p += 1;
    }
    while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
        p -= 1;
    }
    let mut s = 0;
    while s < ob.len() - p && s < nb.len() - p && ob[ob.len() - 1 - s] == nb[nb.len() - 1 - s] {
        s += 1;
    }
    while !old.is_char_boundary(ob.len() - s) || !new.is_char_boundary(nb.len() - s) {
        s -= 1;
    }
    Patch {
        start: at + p,
        end: at + ob.len() - s,
        text: new[p..nb.len() - s].to_string(),
    }
}

fn slice(src: &str, s: Span) -> Result<&str, Refusal> {
    src.get(s.start..s.end).ok_or(Refusal::StaleDiagram)
}

/// Compiles (against roomy image areas) and lowers `src`.
fn lowered(src: &str) -> Result<Module, String> {
    let p = compile(src, SIZES).map_err(|ds| {
        ds.first().map_or_else(
            || "unknown error".to_string(),
            |d| format!("{}: {}", d.pos, d.msg),
        )
    })?;
    lower(src, &p.trace_map()).map_err(|e| e.to_string())
}

/// `m` without what differs between two lowerings of one text: geometry
/// (the canvas moves boxes) and statement ids (a viewer may number them
/// from the box's table).
fn plain(m: &Module) -> Module {
    let mut m = m.clone();
    for p in &mut m.pous {
        let d = &mut p.diagram;
        d.size = (0.0, 0.0);
        for n in &mut d.nodes {
            n.rect = Rect::default();
            n.stmt = StmtId(0);
        }
        for r in &mut d.regions {
            r.rect = Rect::default();
            r.stmt = StmtId(0);
        }
        for s in &mut d.stmts {
            s.id = StmtId(0);
        }
    }
    m
}

// ---- reading the text -------------------------------------------------

/// The byte offset of a 1-based line and column (the lexer counts bytes).
fn offset_in(s: &str, p: Pos) -> usize {
    let mut at = 0;
    for _ in 1..p.line {
        match s.as_bytes()[at..].iter().position(|&b| b == b'\n') {
            Some(i) => at += i + 1,
            None => return s.len(),
        }
    }
    (at + (p.col as usize).saturating_sub(1)).min(s.len())
}

/// Steps over whitespace, `(* *)` and `/* */` comments, `//` comments and
/// `{ }` pragmas, as the lexer does.
fn skip_trivia(b: &[u8], mut i: usize) -> usize {
    fn close(b: &[u8], from: usize, c: u8) -> usize {
        let mut i = from;
        while i + 1 < b.len() {
            if b[i] == b'*' && b[i + 1] == c {
                return i + 2;
            }
            i += 1;
        }
        b.len()
    }
    loop {
        match (b.get(i), b.get(i + 1)) {
            (Some(c), _) if c.is_ascii_whitespace() => i += 1,
            (Some(b'('), Some(b'*')) => i = close(b, i + 2, b')'),
            (Some(b'/'), Some(b'*')) => i = close(b, i + 2, b'/'),
            (Some(b'/'), Some(b'/')) => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            (Some(b'{'), _) => {
                while i < b.len() && b[i] != b'}' {
                    i += 1;
                }
                i = (i + 1).min(b.len());
            }
            _ => return i,
        }
    }
}

/// The first byte of the first token that is not an opening parenthesis.
fn first_token(b: &[u8], mut i: usize) -> usize {
    loop {
        i = skip_trivia(b, i);
        if b.get(i) == Some(&b'(') {
            i += 1;
        } else {
            return i;
        }
    }
}

const HEAD: &str = "PROGRAM p\nx := ";

/// An operand's text parsed on its own, in the place an assignment's right
/// side is (the contract of the parser's spans).
struct Reparsed {
    expr: Expr,
    wrapped: String,
}

impl Reparsed {
    fn of(text: &str) -> Result<Self, Refusal> {
        let wrapped = format!("{HEAD}{text};\nEND_PROGRAM\n");
        let unit = ast::parse(&wrapped).map_err(|_| Refusal::Unreadable)?;
        let body = unit.pous.first().map(|p| p.body.as_slice());
        match body {
            Some([s]) => match &s.kind {
                StmtKind::Assign(_, rhs) => Ok(Self {
                    expr: rhs.clone(),
                    wrapped,
                }),
                _ => Err(Refusal::Unreadable),
            },
            _ => Err(Refusal::Unreadable),
        }
    }

    /// A position in the wrapped text as a byte offset in the operand.
    fn at(&self, p: Pos) -> usize {
        offset_in(&self.wrapped, p).saturating_sub(HEAD.len())
    }
}

/// A binary expression written in parentheses: its text starts before its
/// left operand's does.
fn parenthesised(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Binary(_, l, _) if e.start != l.start)
}

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

fn binop(sym: &str) -> Option<BinOp> {
    ALL_OPS.into_iter().find(|b| b.symbol() == sym)
}

/// The operators `op` can become.
fn family(op: BinOp) -> &'static [BinOp] {
    match op {
        BinOp::And | BinOp::Or => &[BinOp::And, BinOp::Or],
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            &[BinOp::Eq, BinOp::Ne, BinOp::Lt, BinOp::Le, BinOp::Gt, BinOp::Ge]
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            &[BinOp::Add, BinOp::Sub, BinOp::Mul, BinOp::Div]
        }
        _ => &[],
    }
}

/// The lowest precedence an operand at input `input` of `node` may have
/// without brackets.
fn context(d: &Diagram, node: NodeId, input: usize) -> u8 {
    match &d.node(node).kind {
        NodeKind::Op { op } => match binop(op) {
            Some(b) => prec(b) + u8::from(input == 1),
            None if op == "NOT" || op == "-" => UNARY,
            None => 0,
        },
        _ => 0,
    }
}

fn pou_of<'m>(module: &'m Module, name: &str) -> Result<&'m Pou, Refusal> {
    module
        .pou(name)
        .ok_or_else(|| Refusal::NoSuchPou(name.to_string()))
}

fn check_node(d: &Diagram, node: NodeId) -> Result<(), Refusal> {
    if (node.0 as usize) < d.nodes.len() {
        Ok(())
    } else {
        Err(Refusal::NoSuchBlock)
    }
}

/// The pin's operand text and the wire feeding it.
fn pin_of<'m>(module: &'m Module, pin: &Pin) -> Result<(&'m Pou, Span, &'m Wire), Refusal> {
    let pou = pou_of(module, &pin.pou)?;
    let d = &pou.diagram;
    check_node(d, pin.node)?;
    if pin.input >= d.node(pin.node).ins.len() {
        return Err(Refusal::NoSuchInput);
    }
    let span = *d.pins.get(&(pin.node, pin.input)).ok_or(Refusal::NoText)?;
    let wire = d
        .wires
        .iter()
        .find(|w| w.to == pin.node && w.inp == pin.input)
        .ok_or(Refusal::NoText)?;
    Ok((pou, span, wire))
}

// ---- SetConst ----------------------------------------------------------

fn is_delimiter(c: u8) -> bool {
    c.is_ascii_whitespace() || matches!(c, b'(' | b')' | b'/' | b'{' | b',' | b';' | b'[' | b']')
}

/// `new` in the letter case of the literal it replaces: all lowercase if
/// that was.
fn restyle(old: &str, new: String) -> String {
    let upper = old.bytes().any(|c| c.is_ascii_uppercase());
    let lower = old.bytes().any(|c| c.is_ascii_lowercase());
    if lower && !upper {
        new.to_ascii_lowercase()
    } else {
        new
    }
}

fn render(value: Literal, prefix: Option<&str>) -> Result<String, Refusal> {
    Ok(match value {
        Literal::Int(v) => match prefix {
            Some(p) => format!("{p}#{v}"),
            None => v.to_string(),
        },
        Literal::Real(v) if v.is_finite() => match prefix {
            Some(p) => format!("{p}#{v:?}"),
            None => format!("{v:?}"),
        },
        Literal::Real(_) => return Err(Refusal::BadValue("a real number must be finite".into())),
        Literal::Bool(b) => (if b { "TRUE" } else { "FALSE" }).to_string(),
        Literal::Time(ns) => crate::text::time(ns),
    })
}

fn describe(e: &Expr) -> &'static str {
    match &e.kind {
        ExprKind::Var(_) | ExprKind::Direct(_) | ExprKind::Member(..) | ExprKind::Bit(..) => "a variable",
        ExprKind::Index(..) => "an array element",
        ExprKind::Call(..) => "a call",
        ExprKind::EnumLit(..) => "an enumeration value",
        ExprKind::Unary(..) | ExprKind::Binary(..) => "an expression",
        _ => "a value",
    }
}

fn set_const(src: &str, module: &Module, pin: &Pin, value: Literal) -> Result<Plan, Refusal> {
    let (pou, span, wire) = pin_of(module, pin)?;
    let d = &pou.diagram;
    let text = slice(src, span)?;
    let r = Reparsed::of(text)?;
    let (kind, prefix) = match &r.expr.kind {
        ExprKind::Int { prefix, .. } => (0, prefix.as_deref()),
        ExprKind::Real { prefix, .. } => (1, prefix.as_deref()),
        ExprKind::Bool(_) => (2, None),
        ExprKind::Time(_) => (3, None),
        _ => return Err(Refusal::NotAValue(describe(&r.expr).into())),
    };
    if !matches!(d.node(wire.from).kind, NodeKind::Const { value: Some(_) }) {
        return Err(Refusal::NotAValue("a folded constant".into()));
    }
    let same_kind = matches!((kind, value), (0, Literal::Int(_)) | (1, Literal::Real(_)));
    let b = text.as_bytes();
    let t0 = first_token(b, 0);
    let mut i = t0;
    if matches!(b.get(i), Some(b'-' | b'+')) {
        i = skip_trivia(b, i + 1);
    }
    let mut t1 = i;
    while t1 < b.len() && !is_delimiter(b[t1]) {
        t1 += 1;
    }
    let old = text.get(t0..t1).ok_or(Refusal::Unreadable)?;
    if !matches!(
        Reparsed::of(old)?.expr.kind,
        ExprKind::Int { .. } | ExprKind::Real { .. } | ExprKind::Bool(_) | ExprKind::Time(_)
    ) {
        return Err(Refusal::Unreadable);
    }
    let mut new = restyle(old, render(value, if same_kind { prefix } else { None })?);
    // `a -5` with the 5 made negative must not become `a --3`.
    if new.starts_with('-') && t0 == 0 && span.start > 0 && src.as_bytes()[span.start - 1] == b'-' {
        new.insert(0, ' ');
    }
    Ok(Plan {
        span,
        splices: vec![Splice {
            at: t0,
            del: t1 - t0,
            ins: new,
        }],
        expect: Expect {
            pou: pou.name.clone(),
            target: Target::Const {
                node: wire.from,
                value,
            },
        },
    })
}

// ---- ToggleNegation ---------------------------------------------------

/// The splices that flip the `NOT` on an operand whose context takes
/// operands of at least precedence `ctx` without brackets.
fn toggle_splices(text: &str, ctx: u8) -> Result<Vec<Splice>, Refusal> {
    let r = Reparsed::of(text)?;
    let b = text.as_bytes();
    let ExprKind::Unary(UnOp::Not, a) = &r.expr.kind else {
        // Add: `NOT x`, `NOT (a > b)`; brackets only round a bare binary.
        if matches!(r.expr.kind, ExprKind::Binary(..)) && !parenthesised(&r.expr) {
            return Ok(vec![
                Splice {
                    at: 0,
                    del: 0,
                    ins: "NOT (".into(),
                },
                Splice {
                    at: text.len(),
                    del: 0,
                    ins: ")".into(),
                },
            ]);
        }
        return Ok(vec![Splice {
            at: 0,
            del: 0,
            ins: "NOT ".into(),
        }]);
    };
    // Remove: the NOT and the blanks after it.
    let n0 = first_token(b, 0);
    if !text
        .get(n0..n0 + 3)
        .is_some_and(|w| w.eq_ignore_ascii_case("NOT"))
    {
        return Err(Refusal::Unreadable);
    }
    let mut n1 = n0 + 3;
    while b.get(n1).is_some_and(u8::is_ascii_whitespace) {
        n1 += 1;
    }
    let mut out = vec![Splice {
        at: n0,
        del: n1 - n0,
        ins: String::new(),
    }];
    // `NOT (a > b)` in a place that takes `a > b`: the brackets go too.
    let ctx = if r.at(r.expr.start) == n0 { ctx } else { 0 };
    if parenthesised(a) && level(a) >= ctx {
        let (s, e) = (r.at(a.start), r.at(a.end));
        if s >= n1 && e > s && b.get(s) == Some(&b'(') && b.get(e - 1) == Some(&b')') {
            out.push(Splice {
                at: s,
                del: 1,
                ins: String::new(),
            });
            out.push(Splice {
                at: e - 1,
                del: 1,
                ins: String::new(),
            });
        }
    }
    Ok(out)
}

/// The block feeding a bubble must be able to have one.
fn bubble_source(d: &Diagram, from: NodeId) -> Result<(), Refusal> {
    match &d.node(from).kind {
        NodeKind::Const { .. } => Err(Refusal::NoBubble),
        NodeKind::Op { op } if op == "NOT" => Err(Refusal::NotBoolean),
        _ => Ok(()),
    }
}

fn toggle_pin(src: &str, module: &Module, pin: &Pin) -> Result<Plan, Refusal> {
    let (pou, span, wire) = pin_of(module, pin)?;
    let d = &pou.diagram;
    bubble_source(d, wire.from)?;
    let text = slice(src, span)?;
    Ok(Plan {
        span,
        splices: toggle_splices(text, context(d, pin.node, pin.input))?,
        expect: Expect {
            pou: pou.name.clone(),
            target: Target::Pin {
                node: pin.node,
                pin: pin.input,
            },
        },
    })
}

fn toggle_gate(src: &str, module: &Module, pou: &str, region: RegionId) -> Result<Plan, Refusal> {
    let pou = pou_of(module, pou)?;
    let d = &pou.diagram;
    let reg = d.regions.get(region.0 as usize).ok_or(Refusal::NoSuchRegion)?;
    let span = *d.gates.get(&region).ok_or(Refusal::NoCondition)?;
    let (from, _) = reg.gate.ok_or(Refusal::NoCondition)?;
    bubble_source(d, from)?;
    let text = slice(src, span)?;
    Ok(Plan {
        span,
        splices: toggle_splices(text, 0)?,
        expect: Expect {
            pou: pou.name.clone(),
            target: Target::Gate { region },
        },
    })
}

// ---- SwapOp ------------------------------------------------------------

fn swap_op(src: &str, module: &Module, pou: &str, node: NodeId, to: BinOp) -> Result<Plan, Refusal> {
    let pou = pou_of(module, pou)?;
    let d = &pou.diagram;
    check_node(d, node)?;
    let n = d.node(node);
    let NodeKind::Op { op } = &n.kind else {
        return Err(Refusal::NotAnOperator);
    };
    let from = binop(op).ok_or(Refusal::NotAnOperator)?;
    let kin = family(from);
    if kin.is_empty() {
        return Err(Refusal::NotSwappable(op.clone()));
    }
    if from == to {
        return Err(Refusal::NothingToChange);
    }
    if !kin.contains(&to) {
        return Err(Refusal::WrongFamily {
            from: from.symbol().into(),
            to: to.symbol().into(),
        });
    }
    let whole = *d.ops.get(&node).ok_or(Refusal::NotAnOperator)?;
    let text = slice(src, whole)?;
    let r = Reparsed::of(text)?;
    let ExprKind::Binary(bop, l, rhs) = &r.expr.kind else {
        return Err(Refusal::Unreadable);
    };
    if *bop != from {
        return Err(Refusal::StaleDiagram);
    }
    // The operator token.
    let at = offset_in(src, n.pos)
        .checked_sub(whole.start)
        .filter(|&a| a < text.len())
        .ok_or(Refusal::StaleDiagram)?;
    let tok_len = if text.as_bytes()[at] == b'&' {
        1
    } else {
        from.symbol().len()
    };
    let old_tok = text.get(at..at + tok_len).ok_or(Refusal::StaleDiagram)?;
    if old_tok != "&" && !old_tok.eq_ignore_ascii_case(from.symbol()) {
        return Err(Refusal::StaleDiagram);
    }
    let mut new_tok = to.symbol().to_string();
    if old_tok
        .bytes()
        .all(|c| !c.is_ascii_alphabetic() || c.is_ascii_lowercase())
        && old_tok.bytes().any(|c| c.is_ascii_alphabetic())
    {
        new_tok = new_tok.to_ascii_lowercase();
    }

    let p = prec(to);
    let mut open = Vec::new();
    let mut kids = Vec::new();
    let mut close = Vec::new();
    // The expression in its context: does it still group as it did?
    let ctx = d.wires.iter().find(|w| w.from == node).map_or(0, |w| {
        let direct = d.pins.get(&(w.to, w.inp)) == Some(&whole);
        if direct { context(d, w.to, w.inp) } else { UNARY }
    });
    if p < ctx && !parenthesised(&r.expr) {
        open.push(Splice {
            at: 0,
            del: 0,
            ins: "(".into(),
        });
        close.push(Splice {
            at: text.len(),
            del: 0,
            ins: ")".into(),
        });
    }
    // Its operands: left needs level `p`, right `p + 1`.
    for (child, min) in [(&**l, p), (&**rhs, p + 1)] {
        if level(child) < min && !parenthesised(child) {
            kids.push(Splice {
                at: r.at(child.start),
                del: 0,
                ins: "(".into(),
            });
            kids.push(Splice {
                at: r.at(child.end),
                del: 0,
                ins: ")".into(),
            });
        }
    }
    let mut splices = open;
    splices.extend(kids);
    splices.push(Splice {
        at,
        del: tok_len,
        ins: new_tok,
    });
    splices.extend(close);
    Ok(Plan {
        span: whole,
        splices,
        expect: Expect {
            pou: pou.name.clone(),
            target: Target::Op { node, to },
        },
    })
}

// ---- the check ---------------------------------------------------------

/// `t` and the blocks of its statement that contain its text: those it
/// feeds, transitively, without leaving the statement.
fn affected(d: &Diagram, t: NodeId) -> BTreeSet<NodeId> {
    let stmt = d.node(t).stmt;
    let mut set = BTreeSet::from([t]);
    let mut todo = vec![t];
    while let Some(x) = todo.pop() {
        for w in d.wires.iter().filter(|w| w.from == x) {
            if d.node(w.to).stmt == stmt && set.insert(w.to) {
                todo.push(w.to);
            }
        }
    }
    set
}

fn value_of(l: Literal) -> Value {
    match l {
        Literal::Int(v) | Literal::Time(v) => Value::Int(v),
        Literal::Real(v) => Value::Real(v),
        Literal::Bool(b) => Value::Bool(b),
    }
}

/// Splices, compiles and re-lowers; the new diagram must differ from the
/// old in exactly what `ex` says.
fn verify(old: &Module, new_src: &str, ex: &Expect) -> Result<(), Refusal> {
    let new = lowered(new_src).map_err(Refusal::WouldNotCompile)?;
    let more = |what: &str| Refusal::WouldChangeMore(what.to_string());
    let bubble = matches!(ex.target, Target::Pin { .. } | Target::Gate { .. });
    // Another number of blocks: a NOT that is not a bubble, say.
    let shape = || {
        if bubble {
            Refusal::NotBoolean
        } else {
            more("the number of blocks or wires")
        }
    };
    if old.pous.len() != new.pous.len() {
        return Err(more("the programs and blocks of the source"));
    }
    for (a, b) in old.pous.iter().zip(&new.pous) {
        if a.name != b.name {
            return Err(more("the programs and blocks of the source"));
        }
        if a.name != ex.pou {
            let same = diff::signatures(&a.name, &a.diagram) == diff::signatures(&b.name, &b.diagram)
                && a.diagram
                    .regions
                    .iter()
                    .map(|r| &r.label)
                    .eq(b.diagram.regions.iter().map(|r| &r.label));
            if !same {
                return Err(more(&format!("the diagram of {}", a.name)));
            }
        }
    }
    let (Some(po), Some(pn)) = (old.pou(&ex.pou), new.pou(&ex.pou)) else {
        return Err(more("the programs and blocks of the source"));
    };
    let (od, nd) = (&po.diagram, &pn.diagram);
    if od.nodes.len() != nd.nodes.len()
        || od.wires.len() != nd.wires.len()
        || od.regions.len() != nd.regions.len()
    {
        return Err(shape());
    }
    let hit = ex.target.node().map_or_else(BTreeSet::new, |t| affected(od, t));

    let mut flips = 0;
    for (a, b) in od.wires.iter().zip(&nd.wires) {
        if (a.from, a.out, a.to, a.inp) != (b.from, b.out, b.to, b.inp) {
            return Err(more("how the blocks are wired"));
        }
        if a.negated != b.negated {
            match ex.target {
                Target::Pin { node, pin } if a.to == node && a.inp == pin => flips += 1,
                _ => return Err(more("a negation bubble elsewhere")),
            }
        }
    }
    if matches!(ex.target, Target::Pin { .. }) && flips != 1 {
        return Err(Refusal::NotBoolean);
    }
    for (a, b) in od.regions.iter().zip(&nd.regions) {
        if (a.kind, a.arm, a.parent, a.gate) != (b.kind, b.arm, b.parent, b.gate) {
            return Err(more("the regions"));
        }
        let target = matches!(ex.target, Target::Gate { region } if region == a.id);
        let on_path = a.gate.is_some_and(|(g, _)| hit.contains(&g));
        if target {
            if a.negated == b.negated || a.label == b.label {
                return Err(Refusal::NotBoolean);
            }
        } else if a.negated != b.negated {
            return Err(more("a negation bubble elsewhere"));
        } else if a.label != b.label && !on_path {
            return Err(more(&format!("the label of {}", a.label)));
        }
    }
    let (so, sn) = (diff::signatures(&po.name, od), diff::signatures(&pn.name, nd));
    for (i, (a, b)) in od.nodes.iter().zip(&nd.nodes).enumerate() {
        if a.ins != b.ins || a.outs != b.outs || a.region != b.region {
            return Err(shape());
        }
        let is_target = ex.target.node() == Some(a.id);
        let kind_ok = match (&ex.target, is_target) {
            (Target::Op { to, .. }, true) => {
                b.kind
                    == NodeKind::Op {
                        op: to.symbol().into(),
                    }
            }
            (Target::Const { value, .. }, true) => {
                b.kind
                    == NodeKind::Const {
                        value: Some(value_of(*value)),
                    }
            }
            _ => a.kind == b.kind,
        };
        if !kind_ok {
            return Err(if is_target {
                more("the block did not read back as asked")
            } else {
                shape()
            });
        }
        if so[i] != sn[i] && !hit.contains(&a.id) {
            return Err(more(&format!("the block {}", a.label)));
        }
    }
    if let Some(t) = ex.target.node()
        && so[t.0 as usize] == sn[t.0 as usize]
    {
        return Err(Refusal::NothingToChange);
    }
    // The review screen's own view: no blocks or regions appear or go,
    // except the one a swapped operator or a changed literal replaces.
    let df = diff::modules(old, &new);
    let counts = (df.counts().0, df.counts().1);
    let expect = if matches!(ex.target, Target::Op { .. } | Target::Const { .. }) {
        (1, 1)
    } else {
        (0, 0)
    };
    if !df.pous_added.is_empty()
        || !df.pous_removed.is_empty()
        || counts != expect
        || df.regions.iter().any(|r| r.change != diff::Change::Changed)
    {
        return Err(more("the diagram as the review screen would show it"));
    }
    Ok(())
}
