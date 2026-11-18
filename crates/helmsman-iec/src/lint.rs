//! `PLCopen` guideline lints: what the checker says about code that compiles
//! and still holds a classic bug (`st-lints`, `INVESTIGATION.md` §1).
//!
//! A pass over the syntax tree of a parsed unit, apart from the checker, so
//! a source that does not compile still lists what it can. Each finding has
//! a stable id (`PLCOPEN-CP<n>` and `PLCOPEN-L<n>` where the `PLCopen` Coding
//! Guidelines v1.0 number the rule, `HELMSMAN-L<n>` otherwise), a position
//! and a `blocking` flag: only [`FB_IN_CONDITIONAL`] and
//! [`OUTPUT_WRITTEN_TWICE`] block, and a pragma on the finding's line
//! overrides one with a reason:
//!
//! ```text
//! tmr(IN := go, PT := T#2s); {lint: off HELMSMAN-L1 "the branch is the enable"}
//! ```
//!
//! What each lint flags, and what it leaves alone, is on its constant below
//! and in docs/help/lints.md. Recursion (CP13) is not here: the checker
//! already refuses it as an error. The rule numbers and pages are in
//! docs/SOURCES.md.
//!
//! Deterministic, never panics on a parsed unit, and allocation only (the
//! crate stays `no_std + alloc`).

#[cfg(not(feature = "std"))]
use crate::prelude::*;
use alloc::collections::{BTreeMap, BTreeSet};

use crate::lexer::Pos;
use crate::parser::{
    Arg, BinOp, Block, Expr, ExprKind, Field, Init, Pou, PouKind, Section, Stmt, StmtKind, TypeDecl, TypeDef,
    TypeRef, UnOp, Unit, VarDecl,
};

/// A function block instance called inside an IF, ELSIF, ELSE or CASE arm
/// that a scan can skip (HELMSMAN-L1, blocking).
///
/// Flagged: a call statement of a function block instance (also one inside
/// a structure, array or another block) lexically inside a branch, at any
/// depth, when some enclosing IF or CASE has a path that does not call that
/// same instance. Not flagged: a call in an IF ... ELSE (or CASE with an
/// ELSE) whose every branch calls the instance; a call in a loop body that
/// no IF or CASE encloses; a call after an early RETURN (not "inside" a
/// branch). A timer called in a CASE state machine arm is flagged by
/// design: the timer stops with the state, which is the bug; override it
/// with a reason when that is the intent.
pub const FB_IN_CONDITIONAL: &str = "HELMSMAN-L1";
/// An output written in more than one place of one scan (PLCOPEN-CP12,
/// blocking).
///
/// An output is a PROGRAM's `VAR_OUTPUT`, a variable located at `%Q`, or a
/// direct `%Q` address. A write is an assignment to it or to a member,
/// element or bit of it, or a call argument that binds it (`=>`, or a
/// `VAR_IN_OUT` argument). Two writes are two places when one scan can run
/// both: in sequence, or one inside a branch and one outside it. The arms
/// of one IF/ELSIF/ELSE or CASE are one place per path, and so are the
/// writes either side of a RETURN that leaves the branch. A write in a
/// loop body is one place. Different members or different literal array
/// elements are different outputs; `o[i]` and `o[j]` are taken as the same.
/// Function block outputs and function results are not outputs here.
pub const OUTPUT_WRITTEN_TWICE: &str = "PLCOPEN-CP12";
/// A local variable that is read and was never given a value (PLCOPEN-CP3,
/// advisory).
///
/// Flagged: a `VAR` or `VAR_TEMP` of an elementary or enumerated type with
/// no initial value, not located, not RETAIN, not a constant, that a
/// `VAR_TEMP` or function local reads before anything assigns it (textual
/// order), or that a `VAR` reads and nothing ever writes. A `VAR` that is
/// read before its first assignment but is written later is the counter
/// and toggle pattern and is not flagged. Not flagged: inputs, in-outs,
/// outputs, globals, structures, arrays, function blocks, a type whose
/// declaration gives a default.
pub const READ_BEFORE_WRITE: &str = "PLCOPEN-CP3";
/// `=` or `<>` on REAL or LREAL (PLCOPEN-CP8, advisory).
///
/// The type of each side is worked out through variables, members,
/// elements, literals, arithmetic and conversions (`INT_TO_REAL(i) = r` is
/// flagged, `REAL_TO_INT(r) = 3` is not). A comparison with a literal zero
/// is not flagged (the guideline's exception).
pub const REAL_EQUALITY: &str = "PLCOPEN-CP8";
/// A FOR variable or its bounds changed in the body (PLCOPEN-L12,
/// advisory).
///
/// Flagged: a write in the body (assignment, `=>` or in-out argument, a
/// nested FOR on the same variable) to the control variable or to anything
/// that appears in the start, end or step expression, including through a
/// member or element or a second name located at the same address.
pub const FOR_CHANGED: &str = "PLCOPEN-L12";
/// A declared variable no code mentions (PLCOPEN-CP24, advisory).
///
/// Flagged: a `VAR`, `VAR_TEMP` or `VAR CONSTANT` of a POU that its body
/// and the other declarations never mention, and a `VAR_GLOBAL` that no
/// POU mentions. Not flagged: inputs, outputs, in-outs, externals and
/// located variables (the I/O map and the signals use those).
pub const UNUSED_VARIABLE: &str = "PLCOPEN-CP24";
/// Code that can never run (PLCOPEN-CP2, advisory).
///
/// Flagged: the first statement after a RETURN, EXIT or CONTINUE, or after
/// an IF or CASE whose every branch ends that way; the body of a branch
/// whose condition is the literal FALSE, and the branches after one whose
/// condition is the literal TRUE. Not flagged: a POU nothing calls.
pub const UNREACHABLE: &str = "PLCOPEN-CP2";
/// A `{lint: off ...}` pragma that overrides nothing (HELMSMAN-L2,
/// advisory): no reason, a reason of more than [`MAX_REASON`] characters or
/// with a control character, an unknown id, a lint that cannot be
/// overridden, or no such finding on its line.
pub const PRAGMA_UNUSED: &str = "HELMSMAN-L2";

/// Every lint id, whether it blocks, and what it says in a few words.
pub const IDS: &[(&str, bool, &str)] = &[
    (
        FB_IN_CONDITIONAL,
        true,
        "function block called inside a condition",
    ),
    (
        OUTPUT_WRITTEN_TWICE,
        true,
        "output written in more than one place",
    ),
    (READ_BEFORE_WRITE, false, "variable read before it has a value"),
    (REAL_EQUALITY, false, "= or <> on REAL"),
    (FOR_CHANGED, false, "FOR variable or bound changed in the loop"),
    (UNUSED_VARIABLE, false, "variable never used"),
    (UNREACHABLE, false, "code that can never run"),
    (PRAGMA_UNUSED, false, "lint pragma that overrides nothing"),
];

/// Longest reason a pragma may carry, in characters (the audit trail keeps
/// one bounded line per override).
pub const MAX_REASON: usize = 200;

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Lint {
    pub pos: Pos,
    pub id: &'static str,
    /// Holds "Use this code" and Deploy, unless overridden.
    pub blocking: bool,
    pub msg: String,
    /// The reason of the pragma that overrides this finding: it is then
    /// advisory (`blocking` is false) and still listed.
    pub overridden: Option<String>,
}

impl Lint {
    /// `ID: message`, with the override when there is one.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.overridden {
            Some(r) => format!("{}: {} (overridden: {r})", self.id, self.msg),
            None => format!("{}: {}", self.id, self.msg),
        }
    }
}

/// Whether `id` is a lint id.
#[must_use]
pub fn known(id: &str) -> bool {
    IDS.iter().any(|(i, _, _)| i.eq_ignore_ascii_case(id))
}

/// The findings for `src`, sorted by position then id: none when it does not
/// parse (the compiler says why).
#[must_use]
pub fn lints(src: &str) -> Vec<Lint> {
    match crate::parser::parse(src, false) {
        Ok(unit) => lint_unit(&unit, src),
        Err(_) => Vec::new(),
    }
}

/// [`lints`] for a unit already parsed from `src`.
#[must_use]
pub fn lint_unit(unit: &Unit, src: &str) -> Vec<Lint> {
    let cx = Cx::new(unit, src);
    let mut out = Vec::new();
    for pou in &unit.pous {
        let sc = Scope::new(&cx, pou);
        l1_fb_in_conditional(&sc, &mut out);
        cp12_outputs(&sc, &mut out);
        cp3_read_before_write(&cx, &sc, &mut out);
        cp8_real_equality(&sc, &mut out);
        l12_for_changed(&sc, &mut out);
        cp24_unused_locals(&cx, &sc, &mut out);
        cp2_unreachable(&sc, &mut out);
    }
    cp24_unused_globals(&cx, &mut out);
    apply_pragmas(src, &mut out);
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------- context

#[derive(Clone)]
enum Ty<'a> {
    Elem(String),
    Fb(&'a Pou),
    Struct(&'a [Field]),
    Array(&'a TypeRef),
    Enum,
    Unknown,
}

impl Ty<'_> {
    fn is_real(&self) -> bool {
        matches!(self, Ty::Elem(n) if n == "REAL" || n == "LREAL")
    }
}

/// Standard functions whose result is the type of their arguments.
const KEEP_FUNCTIONS: &[&str] = &["ABS", "MIN", "MAX", "LIMIT", "SEL", "MUX", "MOVE"];
/// Standard functions that give a REAL.
const REAL_FUNCTIONS: &[&str] = &[
    "SQRT", "LN", "LOG", "EXP", "EXPT", "SIN", "COS", "TAN", "ASIN", "ACOS", "ATAN",
];

struct Cx<'a> {
    types: BTreeMap<&'a str, &'a TypeDecl>,
    fbs: BTreeMap<&'a str, &'a Pou>,
    funcs: BTreeMap<&'a str, &'a Pou>,
    globals: BTreeMap<&'a str, &'a VarDecl>,
    unit: &'a Unit,
    lines: Vec<&'a str>,
}

impl<'a> Cx<'a> {
    fn new(unit: &'a Unit, src: &'a str) -> Self {
        let lib: &'a Unit = crate::library();
        let mut cx = Cx {
            types: BTreeMap::new(),
            fbs: BTreeMap::new(),
            funcs: BTreeMap::new(),
            globals: BTreeMap::new(),
            unit,
            lines: src.lines().collect(),
        };
        for u in [lib, unit] {
            for t in &u.types {
                cx.types.insert(t.name.as_str(), t);
            }
            for p in &u.pous {
                match p.kind {
                    PouKind::FunctionBlock => {
                        cx.fbs.insert(p.name.as_str(), p);
                    }
                    PouKind::Function(_) => {
                        cx.funcs.insert(p.name.as_str(), p);
                    }
                    PouKind::Program => {}
                }
            }
        }
        for g in &unit.globals {
            cx.globals.insert(g.name.as_str(), g);
        }
        cx
    }

    fn resolve(&self, tr: &'a TypeRef, depth: u8) -> Ty<'a> {
        if depth > 16 {
            return Ty::Unknown;
        }
        match tr {
            TypeRef::Array { elem, .. } => Ty::Array(elem),
            TypeRef::Named(n, _) => {
                if let Some(p) = self.fbs.get(n.as_str()) {
                    return Ty::Fb(p);
                }
                match self.types.get(n.as_str()).map(|t| &t.def) {
                    Some(TypeDef::Alias(t, _)) => self.resolve(t, depth + 1),
                    Some(TypeDef::Struct(f)) => Ty::Struct(f),
                    Some(TypeDef::Enum(_)) => Ty::Enum,
                    None => Ty::Elem(n.clone()),
                }
            }
        }
    }

    /// Whether the type's own declaration gives a default value.
    fn type_has_default(&self, tr: &TypeRef, depth: u8) -> bool {
        if depth > 16 {
            return false;
        }
        match tr {
            TypeRef::Array { .. } => true,
            TypeRef::Named(n, _) => match self.types.get(n.as_str()).map(|t| &t.def) {
                Some(TypeDef::Alias(t, init)) => init.is_some() || self.type_has_default(t, depth + 1),
                Some(TypeDef::Struct(_)) => true,
                _ => false,
            },
        }
    }

    /// The source spelling of `name` declared at `pos`, else `name`.
    fn spell(&self, pos: Pos, name: &str) -> String {
        let line = usize::try_from(pos.line).unwrap_or(0).saturating_sub(1);
        let col = usize::try_from(pos.col).unwrap_or(0).saturating_sub(1);
        self.lines
            .get(line)
            .and_then(|l| l.get(col..col + name.len()))
            .filter(|s| s.eq_ignore_ascii_case(name))
            .map_or_else(|| name.to_string(), ToString::to_string)
    }
}

struct Scope<'a> {
    cx: &'a Cx<'a>,
    pou: &'a Pou,
    vars: BTreeMap<&'a str, &'a VarDecl>,
}

impl<'a> Scope<'a> {
    fn new(cx: &'a Cx<'a>, pou: &'a Pou) -> Self {
        let mut vars = BTreeMap::new();
        for v in &pou.vars {
            vars.entry(v.name.as_str()).or_insert(v);
        }
        Scope { cx, pou, vars }
    }

    fn lookup(&self, name: &str) -> Option<&'a VarDecl> {
        match self.vars.get(name) {
            Some(v) if v.section != Section::External => Some(v),
            _ => self.cx.globals.get(name).copied(),
        }
    }

    fn nice(&self, name: &str) -> String {
        self.lookup(name)
            .map_or_else(|| name.to_string(), |v| self.cx.spell(v.pos, name))
    }

    fn ty_of_var(&self, name: &str) -> Ty<'a> {
        self.lookup(name)
            .map_or(Ty::Unknown, |v| self.cx.resolve(&v.ty, 0))
    }

    fn callee_pou(&self, callee: &'a Expr) -> Option<&'a Pou> {
        if let ExprKind::Var(n) = &callee.kind
            && self.lookup(n).is_none()
        {
            return self.cx.funcs.get(n.as_str()).copied();
        }
        match self.type_of(callee) {
            Ty::Fb(p) => Some(p),
            _ => None,
        }
    }

    fn type_of(&self, e: &'a Expr) -> Ty<'a> {
        match &e.kind {
            ExprKind::Real { .. } => Ty::Elem("REAL".into()),
            ExprKind::Int { .. } => Ty::Elem("INT".into()),
            ExprKind::Bool(_) | ExprKind::Unary(UnOp::Not, _) => Ty::Elem("BOOL".into()),
            ExprKind::Time(_) => Ty::Elem("TIME".into()),
            ExprKind::EnumLit(..) => Ty::Enum,
            ExprKind::Var(n) => self.ty_of_var(n),
            ExprKind::Direct(_) | ExprKind::Bit(..) => Ty::Unknown,
            ExprKind::Member(b, f) => match self.type_of(b) {
                Ty::Struct(fields) => fields
                    .iter()
                    .find(|x| x.name == *f)
                    .map_or(Ty::Unknown, |x| self.cx.resolve(&x.ty, 0)),
                Ty::Fb(p) => p
                    .vars
                    .iter()
                    .find(|v| v.name == *f)
                    .map_or(Ty::Unknown, |v| self.cx.resolve(&v.ty, 0)),
                _ => Ty::Unknown,
            },
            ExprKind::Index(b, _) => match self.type_of(b) {
                Ty::Array(el) => self.cx.resolve(el, 0),
                _ => Ty::Unknown,
            },
            ExprKind::Unary(UnOp::Neg, x) => self.type_of(x),
            ExprKind::Binary(op, l, r) => {
                if op.is_comparison() || matches!(op, BinOp::And | BinOp::Or | BinOp::Xor) {
                    Ty::Elem("BOOL".into())
                } else if self.type_of(l).is_real() || self.type_of(r).is_real() {
                    Ty::Elem("REAL".into())
                } else {
                    self.type_of(l)
                }
            }
            ExprKind::Call(c, args) => self.call_type(c, args),
        }
    }

    fn call_type(&self, callee: &'a Expr, args: &'a [Arg]) -> Ty<'a> {
        let ExprKind::Var(name) = &callee.kind else {
            return Ty::Unknown;
        };
        if let Some(p) = self.cx.funcs.get(name.as_str())
            && let PouKind::Function(rt) = &p.kind
        {
            return self.cx.resolve(rt, 0);
        }
        if let Some((_, to)) = name.split_once("_TO_") {
            return Ty::Elem(to.to_string());
        }
        let real_arg = || args.iter().any(|a| self.type_of(&a.value).is_real());
        if REAL_FUNCTIONS.contains(&name.as_str()) || (KEEP_FUNCTIONS.contains(&name.as_str()) && real_arg())
        {
            Ty::Elem("REAL".into())
        } else {
            Ty::Unknown
        }
    }

    /// The canonical root of a name: the address a located variable sits
    /// at (so two names of one address are one root), else the name.
    fn root_key(&self, name: &str) -> String {
        match self.lookup(name).and_then(|v| v.at.as_ref()) {
            Some((a, _)) => format!("%{}", a.to_ascii_uppercase()),
            None => name.to_string(),
        }
    }

    /// The root key and the path (members, elements, bits) of an lvalue.
    #[allow(clippy::many_single_char_names)]
    fn target(&self, e: &Expr) -> Option<(String, Vec<Seg>)> {
        match &e.kind {
            ExprKind::Var(n) => Some((self.root_key(n), Vec::new())),
            ExprKind::Direct(d) => Some((format!("%{}", d.to_ascii_uppercase()), Vec::new())),
            ExprKind::Member(b, f) => {
                let (k, mut s) = self.target(b)?;
                s.push(Seg::Field(f.clone()));
                Some((k, s))
            }
            ExprKind::Index(b, ix) => {
                let (k, mut s) = self.target(b)?;
                let lit: Option<Vec<i128>> = ix
                    .iter()
                    .map(|i| match i.kind {
                        ExprKind::Int { value, .. } => Some(value),
                        _ => None,
                    })
                    .collect();
                s.push(Seg::Idx(lit));
                Some((k, s))
            }
            ExprKind::Bit(b, n) => {
                let (k, mut s) = self.target(b)?;
                s.push(Seg::Bit(*n));
                Some((k, s))
            }
            _ => None,
        }
    }

    /// Whether `a` is an argument the call writes: an output (`=>`) or a
    /// `VAR_IN_OUT` of the callee.
    fn writes_arg(&self, callee: &'a Expr, a: &Arg) -> bool {
        a.output
            || a.name.as_ref().is_some_and(|n| {
                self.callee_pou(callee)
                    .is_some_and(|p| p.vars.iter().any(|v| v.name == *n && v.section == Section::InOut))
            })
    }

    /// Everything a statement itself writes (not its nested blocks).
    fn writes(&self, s: &'a Stmt) -> Vec<(String, Vec<Seg>, Pos)> {
        let mut out = Vec::new();
        match &s.kind {
            StmtKind::Assign(l, _) => {
                if let Some((k, sg)) = self.target(l) {
                    out.push((k, sg, s.pos));
                }
            }
            StmtKind::For { var, .. } => out.push((self.root_key(var), Vec::new(), s.pos)),
            _ => {}
        }
        for e in stmt_exprs(s) {
            walk_expr(e, &mut |x| {
                if let ExprKind::Call(c, args) = &x.kind {
                    for a in args {
                        if self.writes_arg(c, a)
                            && let Some((k, sg)) = self.target(&a.value)
                        {
                            out.push((k, sg, a.pos));
                        }
                    }
                }
            });
        }
        out
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Seg {
    Field(String),
    Idx(Option<Vec<i128>>),
    Bit(u8),
}

fn overlap(a: &[Seg], b: &[Seg]) -> bool {
    a.iter().zip(b).all(|(x, y)| match (x, y) {
        (Seg::Idx(Some(p)), Seg::Idx(Some(q))) => p == q,
        (Seg::Field(p), Seg::Field(q)) => p == q,
        (Seg::Bit(p), Seg::Bit(q)) => p == q,
        _ => true,
    })
}

// ---------------------------------------------------------------- walking

fn walk_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Member(b, _) | ExprKind::Bit(b, _) | ExprKind::Unary(_, b) => walk_expr(b, f),
        ExprKind::Index(b, ix) => {
            walk_expr(b, f);
            for i in ix {
                walk_expr(i, f);
            }
        }
        ExprKind::Binary(_, l, r) => {
            walk_expr(l, f);
            walk_expr(r, f);
        }
        ExprKind::Call(c, args) => {
            walk_expr(c, f);
            for a in args {
                walk_expr(&a.value, f);
            }
        }
        _ => {}
    }
}

/// The expressions a statement holds directly, in evaluation order.
fn stmt_exprs(s: &Stmt) -> Vec<&Expr> {
    match &s.kind {
        StmtKind::Assign(l, r) => vec![r, l],
        StmtKind::Call(e) | StmtKind::While(e, _) | StmtKind::Repeat(_, e) | StmtKind::Case(e, ..) => vec![e],
        StmtKind::If(br, _) => br.iter().map(|(c, _)| c).collect(),
        StmtKind::For { from, to, by, .. } => {
            let mut v = vec![from, to];
            v.extend(by.iter());
            v
        }
        _ => Vec::new(),
    }
}

/// The blocks nested in a statement, in branch order (an IF's or a CASE's
/// ELSE last).
fn stmt_blocks(s: &Stmt) -> Vec<&Block> {
    match &s.kind {
        StmtKind::If(br, els) => br.iter().map(|(_, b)| b).chain(core::iter::once(els)).collect(),
        StmtKind::Case(_, arms, els) => arms.iter().map(|(_, b)| b).chain(core::iter::once(els)).collect(),
        StmtKind::For { body, .. } | StmtKind::While(_, body) | StmtKind::Repeat(body, _) => vec![body],
        _ => Vec::new(),
    }
}

fn walk_stmts<'a>(b: &'a [Stmt], f: &mut dyn FnMut(&'a Stmt)) {
    for s in b {
        f(s);
        for inner in stmt_blocks(s) {
            walk_stmts(inner, f);
        }
    }
}

fn is_branching(s: &Stmt) -> bool {
    matches!(s.kind, StmtKind::If(..) | StmtKind::Case(..))
}

fn push(out: &mut Vec<Lint>, pos: Pos, id: &'static str, msg: String) {
    let blocking = IDS.iter().any(|(i, b, _)| *i == id && *b);
    out.push(Lint {
        pos,
        id,
        blocking,
        msg,
        overridden: None,
    });
}

// ------------------------------------------------------------- HELMSMAN-L1

/// The instances a statement calls on every path through it.
fn definite(sc: &Scope<'_>, s: &Stmt) -> BTreeSet<String> {
    let all = |blocks: Vec<&Block>| -> BTreeSet<String> {
        let mut it = blocks.into_iter().map(|b| definite_block(sc, b));
        let first = it.next().unwrap_or_default();
        it.fold(first, |a, b| a.intersection(&b).cloned().collect())
    };
    match &s.kind {
        StmtKind::Call(e) => fb_call_key(sc, e).into_iter().collect(),
        StmtKind::If(br, els) if !els.is_empty() => all(br.iter().map(|(_, b)| b).chain([els]).collect()),
        StmtKind::Case(_, arms, els) if !els.is_empty() => {
            all(arms.iter().map(|(_, b)| b).chain([els]).collect())
        }
        StmtKind::Repeat(body, _) => definite_block(sc, body),
        _ => BTreeSet::new(),
    }
}

fn exits_early(s: &Stmt) -> bool {
    let mut hit = false;
    walk_stmts(core::slice::from_ref(s), &mut |x| {
        hit |= matches!(x.kind, StmtKind::Return | StmtKind::Exit | StmtKind::Continue);
    });
    hit
}

fn definite_block(sc: &Scope<'_>, b: &Block) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for s in b {
        out.extend(definite(sc, s));
        if exits_early(s) {
            break;
        }
    }
    out
}

/// A rendering of an instance expression that tells two instances apart.
fn render(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Var(n) => n.clone(),
        ExprKind::Member(b, f) => format!("{}.{f}", render(b)),
        ExprKind::Index(b, ix) => {
            let i: Vec<String> = ix
                .iter()
                .map(|i| match i.kind {
                    ExprKind::Int { value, .. } => value.to_string(),
                    _ => "?".to_string(),
                })
                .collect();
            format!("{}[{}]", render(b), i.join(","))
        }
        _ => "?".to_string(),
    }
}

fn fb_call_key(sc: &Scope<'_>, e: &Expr) -> Option<String> {
    let ExprKind::Call(callee, _) = &e.kind else {
        return None;
    };
    matches!(sc.type_of(callee), Ty::Fb(_)).then(|| render(callee))
}

fn l1_walk(
    sc: &Scope<'_>,
    b: &Block,
    enclosing: &mut Vec<(&'static str, BTreeSet<String>)>,
    out: &mut Vec<Lint>,
) {
    for s in b {
        if let StmtKind::Call(e) = &s.kind
            && let ExprKind::Call(callee, _) = &e.kind
            && let Some(key) = fb_call_key(sc, e)
            && let Some((kind, _)) = enclosing.iter().find(|(_, d)| !d.contains(&key))
        {
            let ty = match sc.type_of(callee) {
                Ty::Fb(p) => p.name.clone(),
                _ => String::new(),
            };
            let name = match &callee.kind {
                ExprKind::Var(n) => sc.nice(n),
                _ => key.clone(),
            };
            push(
                out,
                s.pos,
                FB_IN_CONDITIONAL,
                format!(
                    "{name} ({ty}) is called inside {kind}, so on a scan where that branch is not taken it does not run: a timer stops counting, an edge is missed, a counter misses its input. Call it on every scan, outside the condition, and put the condition on its input (IN := ...). If the branch really is the enable, say why with a pragma on this line: {{lint: off {FB_IN_CONDITIONAL} \"reason\"}}"
                ),
            );
        }
        let branching = is_branching(s);
        if branching {
            let kind = if matches!(s.kind, StmtKind::If(..)) {
                "an IF"
            } else {
                "a CASE"
            };
            enclosing.push((kind, definite(sc, s)));
        }
        for inner in stmt_blocks(s) {
            l1_walk(sc, inner, enclosing, out);
        }
        if branching {
            enclosing.pop();
        }
    }
}

fn l1_fb_in_conditional(sc: &Scope<'_>, out: &mut Vec<Lint>) {
    l1_walk(sc, &sc.pou.body, &mut Vec::new(), out);
}

// ------------------------------------------------------------ PLCOPEN-CP12

struct Ev {
    key: String,
    segs: Vec<Seg>,
    pos: Pos,
    ctx: Vec<(u32, u32)>,
}

fn collect_events(
    sc: &Scope<'_>,
    b: &Block,
    ctx: &mut Vec<(u32, u32)>,
    next: &mut u32,
    returning: &mut BTreeSet<(u32, u32)>,
    out: &mut Vec<Ev>,
) {
    for s in b {
        for (key, segs, pos) in sc.writes(s) {
            out.push(Ev {
                key,
                segs,
                pos,
                ctx: ctx.clone(),
            });
        }
        let blocks = stmt_blocks(s);
        if is_branching(s) {
            let id = *next;
            *next += 1;
            for (i, inner) in blocks.into_iter().enumerate() {
                let br = (id, u32::try_from(i).unwrap_or(u32::MAX));
                if inner.iter().any(|x| matches!(x.kind, StmtKind::Return)) {
                    returning.insert(br);
                }
                ctx.push(br);
                collect_events(sc, inner, ctx, next, returning, out);
                ctx.pop();
            }
        } else {
            for inner in blocks {
                collect_events(sc, inner, ctx, next, returning, out);
            }
        }
    }
}

fn is_output(sc: &Scope<'_>, key: &str) -> bool {
    if let Some(addr) = key.strip_prefix('%') {
        return addr.starts_with('Q');
    }
    sc.vars.get(key).is_some_and(|v| v.section == Section::Output) && sc.pou.kind == PouKind::Program
}

fn cp12_outputs(sc: &Scope<'_>, out: &mut Vec<Lint>) {
    let mut evs = Vec::new();
    let mut returning = BTreeSet::new();
    collect_events(
        sc,
        &sc.pou.body,
        &mut Vec::new(),
        &mut 0,
        &mut returning,
        &mut evs,
    );
    let exclusive = |a: &Ev, b: &Ev| -> bool {
        a.ctx
            .iter()
            .any(|x| b.ctx.iter().any(|y| x.0 == y.0 && x.1 != y.1))
            || a.ctx.iter().any(|x| returning.contains(x) && !b.ctx.contains(x))
    };
    let mut found: BTreeMap<String, Vec<Pos>> = BTreeMap::new();
    for (j, b) in evs.iter().enumerate() {
        for a in &evs[..j] {
            if a.key == b.key && is_output(sc, &a.key) && overlap(&a.segs, &b.segs) && !exclusive(a, b) {
                let e = found.entry(a.key.clone()).or_default();
                for p in [a.pos, b.pos] {
                    if !e.contains(&p) {
                        e.push(p);
                    }
                }
            }
        }
    }
    for (key, mut places) in found {
        places.sort();
        let name = if key.starts_with('%') {
            key.clone()
        } else {
            sc.nice(&key)
        };
        let lines: Vec<String> = places.iter().map(|p| p.line.to_string()).collect();
        let at = places.get(1).copied().unwrap_or_default();
        push(
            out,
            at,
            OUTPUT_WRITTEN_TWICE,
            format!(
                "output {name} is written in {} places (line {}): one scan can run more than one of them, so the last wins and the earlier ones only flicker. Work its value out in one place and write it once. If the writes are meant, say why with a pragma on this line: {{lint: off {OUTPUT_WRITTEN_TWICE} \"reason\"}}",
                places.len(),
                lines.join(", ")
            ),
        );
    }
}

// ------------------------------------------------------------- PLCOPEN-CP3

#[derive(PartialEq)]
enum Acc {
    Read,
    Write,
}

type Accesses = Vec<(String, Acc, Pos)>;

fn reads_of(e: &Expr, out: &mut Accesses) {
    walk_expr(e, &mut |x| {
        if let ExprKind::Var(n) = &x.kind {
            out.push((n.clone(), Acc::Read, x.pos));
        }
    });
}

fn root_name(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Var(n) => Some(n.clone()),
        ExprKind::Member(b, _) | ExprKind::Index(b, _) | ExprKind::Bit(b, _) => root_name(b),
        _ => None,
    }
}

fn accesses<'a>(sc: &Scope<'a>, b: &'a Block, out: &mut Accesses) {
    for s in b {
        match &s.kind {
            StmtKind::Assign(l, r) => {
                reads_of(r, out);
                // The element index of the target is read; the target is written.
                walk_expr(l, &mut |x| {
                    if let ExprKind::Index(_, ix) = &x.kind {
                        for i in ix {
                            reads_of(i, out);
                        }
                    }
                });
                if let Some(n) = root_name(l) {
                    out.push((n, Acc::Write, l.pos));
                }
            }
            StmtKind::Call(e) => {
                if let ExprKind::Call(c, args) = &e.kind {
                    for a in args.iter().filter(|a| !a.output) {
                        reads_of(&a.value, out);
                    }
                    if let Some(n) = root_name(c) {
                        out.push((n, Acc::Write, c.pos));
                    }
                    for a in args.iter().filter(|a| sc.writes_arg(c, a)) {
                        if let Some(n) = root_name(&a.value) {
                            out.push((n, Acc::Write, a.pos));
                        }
                    }
                } else {
                    reads_of(e, out);
                }
            }
            StmtKind::If(br, els) => {
                for (c, body) in br {
                    reads_of(c, out);
                    accesses(sc, body, out);
                }
                accesses(sc, els, out);
            }
            StmtKind::Case(sel, arms, els) => {
                reads_of(sel, out);
                for (_, body) in arms {
                    accesses(sc, body, out);
                }
                accesses(sc, els, out);
            }
            StmtKind::For {
                var,
                from,
                to,
                by,
                body,
            } => {
                reads_of(from, out);
                reads_of(to, out);
                if let Some(by) = by {
                    reads_of(by, out);
                }
                out.push((var.clone(), Acc::Write, s.pos));
                accesses(sc, body, out);
            }
            StmtKind::While(c, body) => {
                reads_of(c, out);
                accesses(sc, body, out);
            }
            StmtKind::Repeat(body, c) => {
                accesses(sc, body, out);
                reads_of(c, out);
            }
            _ => {}
        }
    }
}

fn cp3_read_before_write(cx: &Cx<'_>, sc: &Scope<'_>, out: &mut Vec<Lint>) {
    let mut acc = Vec::new();
    accesses(sc, &sc.pou.body, &mut acc);
    let is_function = matches!(sc.pou.kind, PouKind::Function(_));
    for v in &sc.pou.vars {
        let eligible = matches!(v.section, Section::Var | Section::Temp)
            && v.init.is_none()
            && v.at.is_none()
            && !v.retain
            && !v.constant
            && matches!(cx.resolve(&v.ty, 0), Ty::Elem(_) | Ty::Enum)
            && !cx.type_has_default(&v.ty, 0);
        if !eligible || sc.vars.get(v.name.as_str()).is_none_or(|d| d.pos != v.pos) {
            continue;
        }
        let mine: Vec<&(String, Acc, Pos)> = acc.iter().filter(|(n, _, _)| *n == v.name).collect();
        let Some((_, _, rpos)) = mine.iter().find(|(_, a, _)| *a == Acc::Read) else {
            continue;
        };
        let never_written = !mine.iter().any(|(_, a, _)| *a == Acc::Write);
        let read_first = mine.first().is_some_and(|(_, a, _)| *a == Acc::Read);
        let name = cx.spell(v.pos, &v.name);
        if never_written {
            push(
                out,
                *rpos,
                READ_BEFORE_WRITE,
                format!(
                    "{name} is read but nothing ever assigns it, and it has no initial value, so it is always its type's zero. Give it a value where it is declared, or assign it."
                ),
            );
        } else if read_first && (v.section == Section::Temp || is_function) {
            push(
                out,
                *rpos,
                READ_BEFORE_WRITE,
                format!(
                    "{name} is read before this scan assigns it, and it has no initial value, so it starts every scan at its type's zero. Assign it first, or give it a value where it is declared."
                ),
            );
        }
    }
}

// ------------------------------------------------------------- PLCOPEN-CP8

fn is_zero(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Real { value, .. } => *value == 0.0,
        ExprKind::Int { value, .. } => *value == 0,
        ExprKind::Unary(UnOp::Neg, x) => is_zero(x),
        _ => false,
    }
}

fn cp8_real_equality(sc: &Scope<'_>, out: &mut Vec<Lint>) {
    walk_stmts(&sc.pou.body, &mut |s| {
        for e in stmt_exprs(s) {
            walk_expr(e, &mut |x| {
                if let ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), l, r) = &x.kind
                    && (sc.type_of(l).is_real() || sc.type_of(r).is_real())
                    && !is_zero(l)
                    && !is_zero(r)
                {
                    push(
                        out,
                        x.pos,
                        REAL_EQUALITY,
                        format!(
                            "{} on a REAL almost never behaves as written: a sum or a sensor reading is rarely exactly the number. Compare with <, <=, > or >=, or against a band (ABS(a - b) < 0.01).",
                            op.symbol()
                        ),
                    );
                }
            });
        }
    });
}

// -------------------------------------------------------------- PLCOPEN-L12

/// The storage an expression reads: each maximal variable, member, element
/// or address path in it, and what the element indexes read in turn.
fn lvalue_paths(sc: &Scope<'_>, e: &Expr, out: &mut Vec<(String, Vec<Seg>)>) {
    match &e.kind {
        ExprKind::Var(_) | ExprKind::Direct(_) => out.extend(sc.target(e)),
        ExprKind::Member(b, _) | ExprKind::Bit(b, _) => match sc.target(e) {
            Some(t) => {
                out.push(t);
                index_reads(sc, b, out);
            }
            None => lvalue_paths(sc, b, out),
        },
        ExprKind::Index(b, ix) => {
            out.extend(sc.target(e));
            index_reads(sc, b, out);
            for i in ix {
                lvalue_paths(sc, i, out);
            }
        }
        ExprKind::Unary(_, x) => lvalue_paths(sc, x, out),
        ExprKind::Binary(_, l, r) => {
            lvalue_paths(sc, l, out);
            lvalue_paths(sc, r, out);
        }
        ExprKind::Call(_, args) => {
            for a in args {
                lvalue_paths(sc, &a.value, out);
            }
        }
        _ => {}
    }
}

/// The indexes inside a path's base (`a[k].x` reads `k`).
fn index_reads(sc: &Scope<'_>, e: &Expr, out: &mut Vec<(String, Vec<Seg>)>) {
    match &e.kind {
        ExprKind::Member(b, _) | ExprKind::Bit(b, _) => index_reads(sc, b, out),
        ExprKind::Index(b, ix) => {
            index_reads(sc, b, out);
            for i in ix {
                lvalue_paths(sc, i, out);
            }
        }
        _ => {}
    }
}

fn l12_for_changed(sc: &Scope<'_>, out: &mut Vec<Lint>) {
    walk_stmts(&sc.pou.body, &mut |s| {
        let StmtKind::For {
            var,
            from,
            to,
            by,
            body,
        } = &s.kind
        else {
            return;
        };
        let mut watched: Vec<(String, Vec<Seg>)> = vec![(sc.root_key(var), Vec::new())];
        for e in [from, to].into_iter().chain(by.iter()) {
            lvalue_paths(sc, e, &mut watched);
        }
        let mut hit: Option<(Pos, bool)> = None;
        walk_stmts(body, &mut |w| {
            for (k, sg, pos) in sc.writes(w) {
                if hit.is_none()
                    && let Some(i) = watched.iter().position(|(wk, ws)| *wk == k && overlap(ws, &sg))
                {
                    hit = Some((pos, i == 0));
                }
            }
        });
        if let Some((pos, is_var)) = hit {
            let name = sc.nice(var);
            let what = if is_var { "its control variable" } else { "a bound" };
            push(
                out,
                pos,
                FOR_CHANGED,
                format!(
                    "the body of the FOR on {name} changes {what}: the number of passes then no longer follows from the loop's header, and the loop can run too long or never end. Leave the variable and its bounds alone in the body; to leave early use EXIT, or use WHILE."
                ),
            );
        }
    });
}

// ------------------------------------------------------------ PLCOPEN-CP24

fn mentions_init(i: &Init, f: &mut dyn FnMut(&str)) {
    match i {
        Init::Expr(e) => walk_expr(e, &mut |x| {
            if let ExprKind::Var(n) = &x.kind {
                f(n);
            }
        }),
        Init::Array(items, _) => items.iter().for_each(|(_, i)| mentions_init(i, f)),
        Init::Struct(items, _) => items.iter().for_each(|(_, _, i)| mentions_init(i, f)),
    }
}

fn mentioned(pou: &Pou) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    walk_stmts(&pou.body, &mut |s| {
        if let StmtKind::For { var, .. } = &s.kind {
            names.insert(var.clone());
        }
        for e in stmt_exprs(s) {
            walk_expr(e, &mut |x| {
                if let ExprKind::Var(n) = &x.kind {
                    names.insert(n.clone());
                }
            });
        }
    });
    for v in &pou.vars {
        if let Some(i) = &v.init {
            mentions_init(i, &mut |n| {
                names.insert(n.to_string());
            });
        }
    }
    names
}

fn cp24_unused_locals(cx: &Cx<'_>, sc: &Scope<'_>, out: &mut Vec<Lint>) {
    let names = mentioned(sc.pou);
    for v in &sc.pou.vars {
        if matches!(v.section, Section::Var | Section::Temp) && v.at.is_none() && !names.contains(&v.name) {
            let name = cx.spell(v.pos, &v.name);
            push(
                out,
                v.pos,
                UNUSED_VARIABLE,
                format!("{name} is declared and never used. Remove it, or use it."),
            );
        }
    }
}

fn cp24_unused_globals(cx: &Cx<'_>, out: &mut Vec<Lint>) {
    let mut names = BTreeSet::new();
    for p in &cx.unit.pous {
        names.extend(mentioned(p));
    }
    for g in &cx.unit.globals {
        if let Some(i) = &g.init {
            mentions_init(i, &mut |n| {
                names.insert(n.to_string());
            });
        }
    }
    for g in &cx.unit.globals {
        if g.at.is_none() && !names.contains(&g.name) {
            let name = cx.spell(g.pos, &g.name);
            push(
                out,
                g.pos,
                UNUSED_VARIABLE,
                format!("global {name} is declared and no POU uses it. Remove it, or use it."),
            );
        }
    }
}

// -------------------------------------------------------------- PLCOPEN-CP2

fn terminates(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return | StmtKind::Exit | StmtKind::Continue => true,
        StmtKind::If(br, els) => {
            !els.is_empty() && br.iter().all(|(_, b)| block_terminates(b)) && block_terminates(els)
        }
        StmtKind::Case(_, arms, els) => {
            !els.is_empty() && arms.iter().all(|(_, b)| block_terminates(b)) && block_terminates(els)
        }
        _ => false,
    }
}

fn block_terminates(b: &Block) -> bool {
    b.iter().any(terminates)
}

fn first_real(b: &[Stmt]) -> Option<&Stmt> {
    b.iter().find(|s| !matches!(s.kind, StmtKind::Empty))
}

fn lit_bool(e: &Expr) -> Option<bool> {
    match e.kind {
        ExprKind::Bool(b) => Some(b),
        _ => None,
    }
}

fn cp2_unreachable(sc: &Scope<'_>, out: &mut Vec<Lint>) {
    let check_block = |b: &Block, out: &mut Vec<Lint>| {
        if let Some(k) = b.iter().position(terminates)
            && let Some(next) = first_real(&b[k + 1..])
        {
            push(
                out,
                next.pos,
                UNREACHABLE,
                "this code can never run: the statement above always leaves the block (RETURN, EXIT, CONTINUE, or an IF/CASE that does on every branch)".to_string(),
            );
        }
    };
    check_block(&sc.pou.body, out);
    walk_stmts(&sc.pou.body, &mut |s| {
        for b in stmt_blocks(s) {
            check_block(b, out);
        }
        match &s.kind {
            StmtKind::If(br, els) => {
                let mut dead = false;
                for (c, body) in br {
                    let why = if dead {
                        Some("an earlier condition is TRUE")
                    } else if lit_bool(c) == Some(false) {
                        Some("its condition is FALSE")
                    } else {
                        None
                    };
                    if let (Some(why), Some(first)) = (why, first_real(body)) {
                        push(
                            out,
                            first.pos,
                            UNREACHABLE,
                            format!("this code can never run: {why}"),
                        );
                    }
                    dead |= lit_bool(c) == Some(true);
                }
                if dead && let Some(first) = first_real(els) {
                    push(
                        out,
                        first.pos,
                        UNREACHABLE,
                        "this code can never run: an earlier condition is TRUE".to_string(),
                    );
                }
            }
            StmtKind::While(c, body) if lit_bool(c) == Some(false) => {
                if let Some(first) = first_real(body) {
                    push(
                        out,
                        first.pos,
                        UNREACHABLE,
                        "this code can never run: its condition is FALSE".to_string(),
                    );
                }
            }
            _ => {}
        }
    });
}

// ----------------------------------------------------------------- pragmas

struct Pragma {
    pos: Pos,
    text: String,
}

/// Every `{ ... }` pragma outside comments and strings.
fn pragmas(src: &str) -> Vec<Pragma> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let step = |i: &mut usize, line: &mut u32, col: &mut u32| {
        if b[*i] == b'\n' {
            *line += 1;
            *col = 1;
        } else if b[*i] & 0xC0 != 0x80 {
            *col += 1;
        }
        *i += 1;
    };
    while i < b.len() {
        match (b[i], b.get(i + 1).copied()) {
            (b'/', Some(b'/')) => {
                while i < b.len() && b[i] != b'\n' {
                    step(&mut i, &mut line, &mut col);
                }
            }
            (b'(' | b'/', Some(b'*')) => {
                let close = if b[i] == b'(' { b')' } else { b'/' };
                step(&mut i, &mut line, &mut col);
                step(&mut i, &mut line, &mut col);
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&close)) {
                    step(&mut i, &mut line, &mut col);
                }
                for _ in 0..2 {
                    if i < b.len() {
                        step(&mut i, &mut line, &mut col);
                    }
                }
            }
            (q @ (b'\'' | b'"'), _) => {
                step(&mut i, &mut line, &mut col);
                while i < b.len() && b[i] != q && b[i] != b'\n' {
                    if b[i] == b'$' && i + 1 < b.len() {
                        step(&mut i, &mut line, &mut col);
                    }
                    step(&mut i, &mut line, &mut col);
                }
                if i < b.len() {
                    step(&mut i, &mut line, &mut col);
                }
            }
            (b'{', _) => {
                let pos = Pos { line, col };
                step(&mut i, &mut line, &mut col);
                let start = i;
                while i < b.len() && b[i] != b'}' {
                    step(&mut i, &mut line, &mut col);
                }
                if i < b.len() {
                    out.push(Pragma {
                        pos,
                        text: String::from_utf8_lossy(&b[start..i]).into_owned(),
                    });
                    step(&mut i, &mut line, &mut col);
                }
            }
            _ => step(&mut i, &mut line, &mut col),
        }
    }
    out
}

enum Parsed {
    NotOurs,
    Bad(String),
    Off { id: String, reason: String },
}

fn parse_pragma(text: &str) -> Parsed {
    let t = text.trim();
    let Some(rest) = t
        .get(..4)
        .filter(|h| h.eq_ignore_ascii_case("lint"))
        .map(|_| &t[4..])
    else {
        return Parsed::NotOurs;
    };
    if !(rest.is_empty() || rest.starts_with(':') || rest.starts_with(char::is_whitespace)) {
        return Parsed::NotOurs;
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
    let (word, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if !word.eq_ignore_ascii_case("off") {
        return Parsed::Bad("write it {lint: off ID \"reason\"}".into());
    }
    let rest = rest.trim_start();
    let (id, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if id.is_empty() {
        return Parsed::Bad("it names no lint id; write {lint: off ID \"reason\"}".into());
    }
    let rest = rest.trim();
    let Some(reason) = rest
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .filter(|_| rest.len() >= 2)
    else {
        return Parsed::Bad(format!(
            "{id} needs a reason in double quotes: {{lint: off {id} \"why\"}}"
        ));
    };
    Parsed::Off {
        id: id.to_ascii_uppercase(),
        reason: reason.trim().to_string(),
    }
}

fn apply_pragmas(src: &str, found: &mut Vec<Lint>) {
    let mut extra = Vec::new();
    for p in pragmas(src) {
        let (id, reason) = match parse_pragma(&p.text) {
            Parsed::NotOurs => continue,
            Parsed::Bad(why) => {
                push(
                    &mut extra,
                    p.pos,
                    PRAGMA_UNUSED,
                    format!("this lint pragma overrides nothing: {why}"),
                );
                continue;
            }
            Parsed::Off { id, reason } => (id, reason),
        };
        let bad = if !known(&id) {
            Some(format!("{id} is not a lint id (ids: {})", id_list()))
        } else if id == PRAGMA_UNUSED {
            Some(format!("{PRAGMA_UNUSED} cannot be overridden"))
        } else if reason.is_empty() {
            Some(format!("{id} needs a reason: {{lint: off {id} \"why\"}}"))
        } else if reason.chars().count() > MAX_REASON {
            Some(format!("the reason is longer than {MAX_REASON} characters"))
        } else if reason.chars().any(char::is_control) {
            Some("the reason must be one line of plain text".to_string())
        } else {
            None
        };
        if let Some(why) = bad {
            push(
                &mut extra,
                p.pos,
                PRAGMA_UNUSED,
                format!("this lint pragma overrides nothing: {why}"),
            );
            continue;
        }
        let mut used = false;
        for l in found
            .iter_mut()
            .filter(|l| l.id.eq_ignore_ascii_case(&id) && l.pos.line == p.pos.line)
        {
            if l.overridden.is_none() {
                l.overridden = Some(reason.clone());
                l.blocking = false;
            }
            used = true;
        }
        if !used {
            push(
                &mut extra,
                p.pos,
                PRAGMA_UNUSED,
                format!(
                    "this lint pragma overrides nothing: there is no {id} finding on this line (put the pragma on the line the finding names)"
                ),
            );
        }
    }
    found.extend(extra);
}

fn id_list() -> String {
    IDS.iter()
        .filter(|(i, _, _)| *i != PRAGMA_UNUSED)
        .map(|(i, _, _)| *i)
        .collect::<Vec<_>>()
        .join(", ")
}
