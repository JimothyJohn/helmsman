//! Structured Text to diagram.
//!
//! Each statement becomes a small data-flow graph: reads and literals on the
//! left, operators, functions and function block calls in the middle, the
//! assignment on the right. A read of a variable the POU already wrote on
//! the same path (same region or an enclosing one) is wired from that write,
//! so data flow crosses statements; after an IF, CASE or loop, whatever it
//! wrote is read as the variable again (which arm wrote it is a runtime
//! question, answered by the trace). An IF or CASE becomes one region per
//! arm, gated by its condition (or the selector); a loop becomes one region
//! gated by its head or condition.

use std::collections::{HashMap, HashSet};
use std::fmt;

use helmsman_iec::Value;
use helmsman_iec::ast::{self, Expr, ExprKind, Pos, PouKind, Section, StmtKind, TypeRef, UnOp};
use helmsman_iec::trace::{StmtId, TraceMap};

use crate::model::{Diagram, Node, NodeId, NodeKind, Rect, Region, RegionId, RegionKind, Stmt, Wire};
use crate::text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowerError {
    Syntax(helmsman_iec::Diagnostic),
    /// A statement the trace map does not list: the map is not from this
    /// source.
    Unmapped {
        pou: String,
        line: u32,
        col: u32,
    },
}

impl fmt::Display for LowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(d) => write!(f, "{}: {}", d.pos, d.msg),
            Self::Unmapped { pou, line, col } => {
                write!(
                    f,
                    "{pou} {line}:{col}: statement not in the trace map (map from another source?)"
                )
            }
        }
    }
}

impl std::error::Error for LowerError {}

/// A variable a POU declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub section: Section,
    /// Located (`AT %IX0.0`).
    pub located: bool,
    /// The function block type of an instance (or of an array of
    /// instances), upper-cased.
    pub fb: Option<String>,
    /// Declared as an ARRAY.
    pub array: bool,
    /// The (element) type as declared, upper-cased: `BOOL`, `TIME`, `TON`.
    pub ty: String,
}

/// One POU of the module.
#[derive(Debug, Clone, PartialEq)]
pub struct Pou {
    pub name: String,
    pub kind: PouKind,
    pub vars: HashMap<String, Var>,
    pub diagram: Diagram,
}

/// Every user POU of a module's source, lowered and laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// In source order.
    pub pous: Vec<Pou>,
}

impl Module {
    /// The POU named `name` (case-insensitive).
    #[must_use]
    pub fn pou(&self, name: &str) -> Option<&Pou> {
        let name = name.to_ascii_uppercase();
        self.pous.iter().find(|p| p.name == name)
    }

    /// The `PROGRAM`.
    #[must_use]
    pub fn program(&self) -> Option<&Pou> {
        self.pous.iter().find(|p| p.kind == PouKind::Program)
    }

    /// Every user function block instance reachable from the program, as
    /// `(path, FB name)`: `M`, `M.INNER`. Arrays of instances are not
    /// listed. Depth first, names sorted at each level.
    #[must_use]
    pub fn instances(&self) -> Vec<(String, String)> {
        fn go(m: &Module, pou: &Pou, prefix: &str, depth: usize, out: &mut Vec<(String, String)>) {
            if depth > 8 {
                return;
            }
            let mut names: Vec<(&String, &Var)> = pou.vars.iter().collect();
            names.sort_by_key(|(n, _)| n.as_str());
            for (name, v) in names {
                if let Some(fb) = &v.fb
                    && !v.array
                    && let Some(inner) = m.pou(fb)
                {
                    let path = format!("{prefix}{name}");
                    out.push((path.clone(), inner.name.clone()));
                    go(m, inner, &format!("{path}."), depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(p) = self.program() {
            go(self, p, "", 0, &mut out);
        }
        out
    }
}

/// Lowers every user POU of `src` (one compiled into `map`'s program) and
/// lays each out.
///
/// # Errors
/// A syntax error, or a statement `map` does not list.
pub fn lower(src: &str, map: &TraceMap) -> Result<Module, LowerError> {
    lower_by(src, |pou| {
        map.in_pou(pou)
            .filter_map(|id| map.stmt(id).map(|s| ((s.line, s.col), id)))
            .collect()
    })
}

/// One row of a statement table, as the box API's trace lists it: row
/// `k` is `StmtId(k)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StmtPos {
    /// The POU whose body holds it, upper-cased.
    pub pou: String,
    /// From the built-in library, not the user's source.
    pub system: bool,
    pub line: u32,
    pub col: u32,
}

/// [`lower()`] against a statement table instead of a compiled program's
/// map: what a viewer has that only sees the box's source and traces.
///
/// # Errors
/// A syntax error, or a statement `table` does not list.
pub fn lower_table(src: &str, table: &[StmtPos]) -> Result<Module, LowerError> {
    lower_by(src, |pou| {
        let pou = pou.to_ascii_uppercase();
        table
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.system && s.pou == pou)
            .filter_map(|(k, s)| Some(((s.line, s.col), StmtId(u32::try_from(k).ok()?))))
            .collect()
    })
}

fn lower_by(src: &str, ids_of: impl Fn(&str) -> HashMap<(u32, u32), StmtId>) -> Result<Module, LowerError> {
    let unit = ast::parse(src).map_err(LowerError::Syntax)?;
    let mut fbs: HashSet<String> = ast::library()
        .pous
        .iter()
        .filter(|p| p.kind == PouKind::FunctionBlock)
        .map(|p| p.name.clone())
        .collect();
    fbs.extend(
        unit.pous
            .iter()
            .filter(|p| p.kind == PouKind::FunctionBlock)
            .map(|p| p.name.clone()),
    );
    let mut pous = Vec::new();
    for p in &unit.pous {
        let vars = vars(p, &fbs);
        let ids = ids_of(&p.name);
        let mut l = Lower {
            d: Diagram {
                pou: p.name.clone(),
                ..Diagram::default()
            },
            pou: &p.name,
            ids,
            vars: &vars,
            region: None,
            stmt: StmtId(0),
            env: HashMap::new(),
            written: Vec::new(),
            reads: HashMap::new(),
        };
        l.d.top = l.block(&p.body)?;
        let mut d = l.d;
        crate::layout::layout(&mut d);
        pous.push(Pou {
            name: p.name.clone(),
            kind: p.kind.clone(),
            vars,
            diagram: d,
        });
    }
    Ok(Module { pous })
}

fn vars(p: &ast::Pou, fbs: &HashSet<String>) -> HashMap<String, Var> {
    p.vars
        .iter()
        .map(|v| {
            let mut ty = &v.ty;
            let array = matches!(ty, TypeRef::Array { .. });
            while let TypeRef::Array { elem, .. } = ty {
                ty = elem;
            }
            let fb = match ty {
                TypeRef::Named(n, _) if fbs.contains(n) => Some(n.clone()),
                _ => None,
            };
            let named = match ty {
                TypeRef::Named(n, _) => n.to_ascii_uppercase(),
                TypeRef::Array { .. } => String::new(),
            };
            (
                v.name.clone(),
                Var {
                    section: v.section,
                    located: v.at.is_some(),
                    fb,
                    array,
                    ty: named,
                },
            )
        })
        .collect()
}

/// What last wrote a path, as far as lowering can tell.
#[derive(Debug, Clone, Copy)]
enum Src {
    Val(NodeId, usize),
    /// A function block instance called: its outputs are ports of this node.
    Fb(NodeId),
}

type Port = (NodeId, usize);
type R<T> = Result<T, LowerError>;

struct Lower<'a> {
    d: Diagram,
    pou: &'a str,
    ids: HashMap<(u32, u32), StmtId>,
    vars: &'a HashMap<String, Var>,
    region: Option<RegionId>,
    stmt: StmtId,
    env: HashMap<String, Src>,
    /// Every path written, in order, so a compound statement can forget
    /// what its arms wrote.
    written: Vec<String>,
    /// This statement's reads, so a variable read twice is one node.
    reads: HashMap<String, NodeId>,
}

/// A place in memory as text: `P.X`, `A[1,2]`, `%QX0.0`; plus the index
/// expressions that are not literals (then the text shows them as written
/// and the path cannot be looked up).
struct Place<'e> {
    path: String,
    dynamic: Vec<&'e Expr>,
}

fn literal_index(e: &Expr) -> Option<i128> {
    match &e.kind {
        ExprKind::Int { value, .. } => Some(*value),
        ExprKind::Unary(UnOp::Neg, a) => literal_index(a).map(|v| -v),
        _ => None,
    }
}

fn place(e: &Expr) -> Option<Place<'_>> {
    Some(match &e.kind {
        ExprKind::Var(n) => Place {
            path: n.clone(),
            dynamic: Vec::new(),
        },
        ExprKind::Direct(d) => Place {
            path: format!("%{d}"),
            dynamic: Vec::new(),
        },
        ExprKind::Member(b, m) => {
            let mut p = place(b)?;
            p.path = format!("{}.{m}", p.path);
            p
        }
        ExprKind::Bit(b, n) => {
            let mut p = place(b)?;
            p.path = format!("{}.{n}", p.path);
            p
        }
        ExprKind::Index(b, ix) => {
            let mut p = place(b)?;
            let parts: Vec<String> = ix
                .iter()
                .map(|i| match literal_index(i) {
                    Some(v) => v.to_string(),
                    None => {
                        p.dynamic.push(i);
                        text::expr(i)
                    }
                })
                .collect();
            p.path = format!("{}[{}]", p.path, parts.join(","));
            p
        }
        _ => return None,
    })
}

/// The variable a path starts with: `P` of `P.X[2]`.
pub(crate) fn root_of(path: &str) -> &str {
    path.split(['.', '[']).next().unwrap_or(path)
}

/// Whether `key` is `path` or something inside it.
fn within(key: &str, path: &str) -> bool {
    key.strip_prefix(path)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('['))
}

impl Lower<'_> {
    fn id_of(&self, pos: Pos) -> R<StmtId> {
        self.ids
            .get(&(pos.line, pos.col))
            .copied()
            .ok_or_else(|| LowerError::Unmapped {
                pou: self.pou.to_string(),
                line: pos.line,
                col: pos.col,
            })
    }

    fn node(
        &mut self,
        kind: NodeKind,
        label: String,
        text: String,
        ins: Vec<String>,
        outs: Vec<String>,
        pos: Pos,
    ) -> NodeId {
        #[allow(clippy::cast_possible_truncation)]
        let id = NodeId(self.d.nodes.len() as u32);
        self.d.nodes.push(Node {
            id,
            kind,
            label,
            text,
            ins,
            outs,
            stmt: self.stmt,
            pos,
            region: self.region,
            rect: Rect::default(),
        });
        id
    }

    fn wire(&mut self, from: Port, to: NodeId, inp: usize) {
        self.d.wires.push(Wire {
            from: from.0,
            out: from.1,
            to,
            inp,
        });
    }

    fn block(&mut self, b: &[ast::Stmt]) -> R<Vec<usize>> {
        let mut out = Vec::new();
        for s in b {
            if let Some(i) = self.stmt(s)? {
                out.push(i);
            }
        }
        Ok(out)
    }

    /// Forgets what was written since `mark` (an arm or loop body ran or
    /// not, the diagram cannot say which).
    fn forget_since(&mut self, mark: usize, saved: &HashMap<String, Src>) {
        let written: Vec<String> = self.written[mark..].to_vec();
        self.env.clone_from(saved);
        for w in &written {
            self.env.retain(|k, _| !within(k, w));
        }
    }

    fn region(
        &mut self,
        kind: RegionKind,
        stmt: StmtId,
        arm: usize,
        label: String,
        gate: Option<Port>,
        pos: Pos,
    ) -> RegionId {
        #[allow(clippy::cast_possible_truncation)]
        let id = RegionId(self.d.regions.len() as u32);
        self.d.regions.push(Region {
            id,
            kind,
            stmt,
            arm,
            parent: self.region,
            label,
            gate,
            pos,
            body: Vec::new(),
            rect: Rect::default(),
        });
        id
    }

    /// Lowers `b` inside region `r`, starting from `env`.
    fn inside(&mut self, r: RegionId, b: &[ast::Stmt], env: &HashMap<String, Src>) -> R<()> {
        let outer = self.region.replace(r);
        let outer_stmt = self.stmt;
        self.env.clone_from(env);
        let body = self.block(b)?;
        self.region = outer;
        self.stmt = outer_stmt;
        self.d.regions[r.0 as usize].body = body;
        Ok(())
    }

    fn stmt(&mut self, s: &ast::Stmt) -> R<Option<usize>> {
        if matches!(s.kind, StmtKind::Empty) {
            return Ok(None);
        }
        let id = self.id_of(s.pos)?;
        self.stmt = id;
        self.reads.clear();
        let ix = self.d.stmts.len();
        self.d.stmts.push(Stmt {
            id,
            pos: s.pos,
            region: self.region,
            roots: Vec::new(),
            arms: Vec::new(),
        });
        let mut roots = Vec::new();
        let mut arms = Vec::new();
        match &s.kind {
            StmtKind::Empty => {}
            StmtKind::Assign(target, value) => {
                let v = self.expr(value)?;
                let label = format!("{} := {}", text::expr(target), text::expr(value));
                roots.push(self.write(target, v, label, s.pos)?);
            }
            StmtKind::Call(e) => {
                let (n, writes) = self.call(e)?;
                roots.push(n);
                roots.extend(writes);
            }
            StmtKind::If(conds, other) => {
                let saved = self.env.clone();
                let mark = self.written.len();
                for (k, (c, body)) in conds.iter().enumerate() {
                    self.stmt = id;
                    self.reads.clear();
                    self.env.clone_from(&saved);
                    let g = self.expr(c)?;
                    roots.push(g.0);
                    let (kind, kw) = if k == 0 {
                        (RegionKind::If, "IF")
                    } else {
                        (RegionKind::Elsif, "ELSIF")
                    };
                    let r = self.region(kind, id, k, format!("{kw} {}", text::expr(c)), Some(g), c.pos);
                    arms.push(r);
                    self.inside(r, body, &saved)?;
                }
                if !other.is_empty() {
                    let r = self.region(RegionKind::Else, id, conds.len(), "ELSE".into(), None, s.pos);
                    arms.push(r);
                    self.inside(r, other, &saved)?;
                }
                self.forget_since(mark, &saved);
            }
            StmtKind::Case(sel, cases, other) => {
                let g = self.expr(sel)?;
                roots.push(g.0);
                let saved = self.env.clone();
                let mark = self.written.len();
                for (k, (ls, body)) in cases.iter().enumerate() {
                    let pos = ls.first().map_or(s.pos, |l| l.pos);
                    let r = self.region(
                        RegionKind::Case,
                        id,
                        k,
                        format!("CASE {}", text::labels(ls)),
                        Some(g),
                        pos,
                    );
                    arms.push(r);
                    self.inside(r, body, &saved)?;
                }
                if !other.is_empty() {
                    let r = self.region(
                        RegionKind::CaseElse,
                        id,
                        cases.len(),
                        "ELSE".into(),
                        Some(g),
                        s.pos,
                    );
                    arms.push(r);
                    self.inside(r, other, &saved)?;
                }
                self.forget_since(mark, &saved);
            }
            StmtKind::For {
                var,
                from,
                to,
                by,
                body,
            } => {
                let mut ins = vec![("FROM", from), ("TO", to)];
                if let Some(b) = by {
                    ins.push(("BY", b));
                }
                let mut srcs = Vec::new();
                for (_, e) in &ins {
                    srcs.push(self.expr(e)?);
                }
                let mut label = format!("FOR {var} := {} TO {}", text::expr(from), text::expr(to));
                if let Some(b) = by {
                    label = format!("{label} BY {}", text::expr(b));
                }
                let head = self.node(
                    NodeKind::For { var: var.clone() },
                    format!("FOR {var}"),
                    label.clone(),
                    ins.iter().map(|(n, _)| (*n).to_string()).collect(),
                    vec![var.clone()],
                    s.pos,
                );
                for (k, src) in srcs.into_iter().enumerate() {
                    self.wire(src, head, k);
                }
                roots.push(head);
                let saved = self.env.clone();
                let mark = self.written.len();
                let r = self.region(RegionKind::For, id, 0, label, Some((head, 0)), s.pos);
                arms.push(r);
                let mut inner = saved.clone();
                inner.retain(|k, _| !within(k, var));
                inner.insert(var.clone(), Src::Val(head, 0));
                self.written.push(var.clone());
                self.inside(r, body, &inner)?;
                self.forget_since(mark, &saved);
            }
            StmtKind::While(c, body) => {
                let g = self.expr(c)?;
                roots.push(g.0);
                let saved = self.env.clone();
                let mark = self.written.len();
                let r = self.region(
                    RegionKind::While,
                    id,
                    0,
                    format!("WHILE {}", text::expr(c)),
                    Some(g),
                    s.pos,
                );
                arms.push(r);
                self.inside(r, body, &saved)?;
                self.forget_since(mark, &saved);
            }
            StmtKind::Repeat(body, c) => {
                let g = self.expr(c)?;
                roots.push(g.0);
                let saved = self.env.clone();
                let mark = self.written.len();
                let r = self.region(
                    RegionKind::Repeat,
                    id,
                    0,
                    format!("REPEAT … UNTIL {}", text::expr(c)),
                    Some(g),
                    s.pos,
                );
                arms.push(r);
                self.inside(r, body, &saved)?;
                self.forget_since(mark, &saved);
            }
            StmtKind::Exit | StmtKind::Continue | StmtKind::Return => {
                let kw = match s.kind {
                    StmtKind::Exit => "EXIT",
                    StmtKind::Continue => "CONTINUE",
                    _ => "RETURN",
                };
                roots.push(self.node(
                    NodeKind::Jump,
                    kw.into(),
                    kw.into(),
                    Vec::new(),
                    Vec::new(),
                    s.pos,
                ));
            }
        }
        let st = &mut self.d.stmts[ix];
        st.roots = roots;
        st.arms = arms;
        Ok(Some(ix))
    }

    fn write(&mut self, target: &Expr, v: Port, label: String, pos: Pos) -> R<NodeId> {
        let Some(p) = place(target) else {
            // The checker only accepts places; keep going with the text.
            let n = self.node(
                NodeKind::Write {
                    path: text::expr(target),
                },
                format!("{} :=", text::expr(target)),
                label,
                vec![String::new()],
                vec![String::new()],
                pos,
            );
            self.wire(v, n, 0);
            return Ok(n);
        };
        let mut ins = vec![String::new()];
        ins.extend(p.dynamic.iter().map(|e| text::expr(e)));
        let mut srcs = Vec::new();
        for e in &p.dynamic {
            srcs.push(self.expr(e)?);
        }
        let n = self.node(
            NodeKind::Write { path: p.path.clone() },
            format!("{} :=", p.path),
            label,
            ins,
            vec![String::new()],
            pos,
        );
        self.wire(v, n, 0);
        for (k, s) in srcs.into_iter().enumerate() {
            self.wire(s, n, k + 1);
        }
        self.assigned(&p, Src::Val(n, 0));
        Ok(n)
    }

    /// `p` now holds what `src` produced.
    fn assigned(&mut self, p: &Place<'_>, src: Src) {
        let key = if p.dynamic.is_empty() {
            p.path.clone()
        } else {
            // Some element changed; which one is a runtime question.
            root_of(&p.path).to_string()
        };
        self.env.retain(|k, _| !within(k, &key));
        if p.dynamic.is_empty() {
            self.env.insert(key.clone(), src);
        }
        self.written.push(key);
    }

    fn konst(&mut self, e: &Expr, value: Option<Value>) -> Port {
        let t = text::expr(e);
        (
            self.node(
                NodeKind::Const { value },
                t.clone(),
                t,
                Vec::new(),
                vec![String::new()],
                e.pos,
            ),
            0,
        )
    }

    fn expr(&mut self, e: &Expr) -> R<Port> {
        Ok(match &e.kind {
            ExprKind::Int { value, .. } => self.konst(e, Some(Value::Int(*value))),
            ExprKind::Real { value, .. } => self.konst(e, Some(Value::Real(*value))),
            ExprKind::Bool(b) => self.konst(e, Some(Value::Bool(*b))),
            ExprKind::Time(ns) => self.konst(e, Some(Value::Int(*ns))),
            ExprKind::EnumLit(..) => self.konst(e, None),
            ExprKind::Var(n) if !self.vars.contains_key(n) => {
                // An enumeration value (or a constant the library knows).
                self.konst(e, None)
            }
            ExprKind::Var(_)
            | ExprKind::Direct(_)
            | ExprKind::Member(..)
            | ExprKind::Bit(..)
            | ExprKind::Index(..) => {
                match place(e) {
                    Some(p) if p.dynamic.is_empty() => self.read(&p.path, e.pos),
                    Some(p) => {
                        let mut srcs = Vec::new();
                        for i in &p.dynamic {
                            srcs.push(self.expr(i)?);
                        }
                        let n = self.node(
                            NodeKind::Access { path: p.path.clone() },
                            p.path.clone(),
                            text::expr(e),
                            p.dynamic.iter().map(|e| text::expr(e)).collect(),
                            vec![String::new()],
                            e.pos,
                        );
                        for (k, s) in srcs.into_iter().enumerate() {
                            self.wire(s, n, k);
                        }
                        (n, 0)
                    }
                    None => {
                        // A member of a call result, say: show it as a block over its base.
                        let ExprKind::Member(b, m) = &e.kind else {
                            return Ok(self.konst(e, None));
                        };
                        let s = self.expr(b)?;
                        let n = self.node(
                            NodeKind::Op { op: format!(".{m}") },
                            format!(".{m}"),
                            text::expr(e),
                            vec![String::new()],
                            vec![String::new()],
                            e.pos,
                        );
                        self.wire(s, n, 0);
                        (n, 0)
                    }
                }
            }
            ExprKind::Unary(op, a) => {
                let s = self.expr(a)?;
                let sym = match op {
                    UnOp::Neg => "-",
                    UnOp::Not => "NOT",
                };
                let n = self.node(
                    NodeKind::Op { op: sym.into() },
                    sym.into(),
                    text::expr(e),
                    vec![String::new()],
                    vec![String::new()],
                    e.pos,
                );
                self.wire(s, n, 0);
                (n, 0)
            }
            ExprKind::Binary(op, a, b) => {
                let sa = self.expr(a)?;
                let sb = self.expr(b)?;
                let sym = op.symbol();
                let n = self.node(
                    NodeKind::Op { op: sym.into() },
                    sym.into(),
                    text::expr(e),
                    vec![String::new(), String::new()],
                    vec![String::new()],
                    e.pos,
                );
                self.wire(sa, n, 0);
                self.wire(sb, n, 1);
                (n, 0)
            }
            ExprKind::Call(..) => {
                let (n, _) = self.call(e)?;
                (n, 0)
            }
        })
    }

    /// Reads `path`: from what last wrote it, an output of the instance
    /// last called, or a variable node.
    fn read(&mut self, path: &str, pos: Pos) -> Port {
        if let Some(Src::Val(n, p)) = self.env.get(path) {
            return (*n, *p);
        }
        // `T.Q`, `M.INNER.Q`: the longest prefix that is a called instance.
        let mut cut = path.len();
        while let Some(i) = path[..cut].rfind('.') {
            if let Some(Src::Fb(n)) = self.env.get(&path[..i]) {
                let port = &path[i + 1..];
                let node = &mut self.d.nodes[n.0 as usize];
                let k = node.outs.iter().position(|o| o == port).unwrap_or_else(|| {
                    node.outs.push(port.to_string());
                    node.outs.len() - 1
                });
                return (*n, k);
            }
            cut = i;
        }
        if let Some(&n) = self.reads.get(path) {
            return (n, 0);
        }
        let n = self.node(
            NodeKind::Read {
                path: path.to_string(),
            },
            path.to_string(),
            path.to_string(),
            Vec::new(),
            vec![String::new()],
            pos,
        );
        self.reads.insert(path.to_string(), n);
        (n, 0)
    }

    /// A call: a function block instance (by its declaration) or a
    /// function. Returns the node and the writes its `=>` outputs made.
    fn call(&mut self, e: &Expr) -> R<(NodeId, Vec<NodeId>)> {
        let ExprKind::Call(f, args) = &e.kind else {
            return Ok((self.expr(e)?.0, Vec::new()));
        };
        let callee = text::expr(f);
        let instance = place(f).filter(|p| self.vars.contains_key(root_of(&p.path)));
        let mut ins = Vec::new();
        let mut srcs = Vec::new();
        for (k, a) in args.iter().filter(|a| !a.output).enumerate() {
            srcs.push(self.expr(&a.value)?);
            ins.push(a.name.clone().unwrap_or_else(|| format!("IN{}", k + 1)));
        }
        let outputs: Vec<&ast::Arg> = args.iter().filter(|a| a.output).collect();
        let n = match &instance {
            Some(p) => {
                let fb = self
                    .vars
                    .get(root_of(&p.path))
                    .and_then(|v| v.fb.clone())
                    .unwrap_or_default();
                let label = if fb.is_empty() {
                    p.path.clone()
                } else {
                    format!("{} : {fb}", p.path)
                };
                self.node(
                    NodeKind::Fb {
                        instance: p.path.clone(),
                        fb,
                    },
                    label,
                    text::expr(e),
                    ins,
                    Vec::new(),
                    e.pos,
                )
            }
            None => self.node(
                NodeKind::Func { name: callee.clone() },
                callee,
                text::expr(e),
                ins,
                vec![String::new()],
                e.pos,
            ),
        };
        for (k, s) in srcs.into_iter().enumerate() {
            self.wire(s, n, k);
        }
        if let Some(p) = &instance {
            self.assigned(p, Src::Fb(n));
        }
        let mut writes = Vec::new();
        for a in outputs {
            let port = a.name.clone().unwrap_or_default();
            let node = &mut self.d.nodes[n.0 as usize];
            node.outs.push(port.clone());
            let k = node.outs.len() - 1;
            let label = format!("{port} => {}", text::expr(&a.value));
            writes.push(self.write(&a.value, (n, k), label, a.pos)?);
        }
        Ok((n, writes))
    }
}
