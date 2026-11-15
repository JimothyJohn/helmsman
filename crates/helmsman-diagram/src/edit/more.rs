//! `Rewire`, `SetFbArg`, `DeleteStmt` and `Rename`: planning each as one splice of
//! the source, and the check each result goes through.

use helmsman_iec::ast::{
    self, Expr, ExprKind, Pou as AstPou, Section, Stmt, StmtKind, TypeDef, TypeRef, UnOp, Unit,
};
use helmsman_iec::{is_reserved_word, rename_ident};

use super::graph::{self, Hole, Opts, Tree};
use super::{
    Literal, Module, NodeId, NodeKind, Patch, Pin, Refusal, Reparsed, Source, Span, check_node, lowered,
    offset_in, pin_of, plain, pou_of, render, skip_trivia, slice, trim, value_of,
};
use crate::lower::Types;
use crate::model::{Diagram, Rect};

fn more(what: &str) -> Refusal {
    Refusal::WouldChangeMore(what.to_string())
}

/// The source parsed, and the declarations lowering sees.
struct Ctx {
    unit: Unit,
    types: Types,
}

impl Ctx {
    fn new(src: &str) -> Result<Self, Refusal> {
        let unit =
            ast::parse(src).map_err(|d| Refusal::SourceDoesNotCompile(format!("{}: {}", d.pos, d.msg)))?;
        let types = Types::of(&[ast::library(), &unit]);
        Ok(Self { unit, types })
    }

    fn pou(&self, name: &str) -> Result<&AstPou, Refusal> {
        let up = name.to_ascii_uppercase();
        self.unit
            .pous
            .iter()
            .find(|p| p.name == up)
            .ok_or_else(|| Refusal::NoSuchPou(name.to_string()))
    }
}

/// A type as far as an input cares: what can be wired to what.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Class {
    Bool,
    Number,
    Time,
    Text,
    Other(String),
}

struct Ty {
    class: Class,
    /// How the refusal names it.
    name: String,
}

fn class_of(types: &Types, t: &TypeRef) -> Ty {
    match types.resolve(t) {
        TypeRef::Named(n, _) => {
            let class = match n.as_str() {
                "BOOL" => Class::Bool,
                "SINT" | "INT" | "DINT" | "LINT" | "USINT" | "UINT" | "UDINT" | "ULINT" | "BYTE" | "WORD"
                | "DWORD" | "LWORD" | "REAL" | "LREAL" => Class::Number,
                "TIME" => Class::Time,
                "STRING" | "WSTRING" => Class::Text,
                other => Class::Other(other.to_string()),
            };
            Ty {
                class,
                name: n.clone(),
            }
        }
        TypeRef::Array { .. } => Ty {
            class: Class::Other("ARRAY".into()),
            name: "an array".into(),
        },
    }
}

fn literal_ty(l: Literal) -> Ty {
    let (class, name) = match l {
        Literal::Int(_) | Literal::Real(_) => (Class::Number, "a number"),
        Literal::Bool(_) => (Class::Bool, "TRUE or FALSE"),
        Literal::Time(_) => (Class::Time, "a time"),
    };
    Ty {
        class,
        name: name.into(),
    }
}

/// The type of an operand read from the source (`NOT`s looked through).
fn expr_ty(ctx: &Ctx, pou: &str, e: &Expr) -> Option<Ty> {
    match &e.kind {
        ExprKind::Unary(UnOp::Not, a) => expr_ty(ctx, pou, a),
        ExprKind::Int { .. } | ExprKind::Real { .. } => Some(literal_ty(Literal::Int(0))),
        ExprKind::Bool(_) => Some(literal_ty(Literal::Bool(true))),
        ExprKind::Time(_) => Some(literal_ty(Literal::Time(0))),
        _ => ctx.types.type_of(pou, e).map(|t| class_of(&ctx.types, &t)),
    }
}

fn check_types(expected: Option<&Ty>, found: Option<&Ty>) -> Result<(), Refusal> {
    match (expected, found) {
        (Some(a), Some(b)) if a.class != b.class => Err(Refusal::TypeMismatch {
            expected: a.name.clone(),
            found: b.name.clone(),
        }),
        _ => Ok(()),
    }
}

// ---- the sources --------------------------------------------------------

/// The root variable of a place expression.
fn root(e: &Expr) -> Option<&Expr> {
    match &e.kind {
        ExprKind::Var(_) | ExprKind::Direct(_) => Some(e),
        ExprKind::Member(b, _) | ExprKind::Index(b, _) | ExprKind::Bit(b, _) => root(b),
        _ => None,
    }
}

/// The declaration of variable `name` of the POU `pou`, and its name as
/// the source spells it.
fn spelled(ctx: &Ctx, src: &str, pou: &str, name: &str) -> Option<String> {
    let v = ctx
        .pou(pou)
        .ok()?
        .vars
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(name))?;
    let at = offset_in(src, v.pos);
    src.get(at..at + v.name.len())
        .filter(|s| s.eq_ignore_ascii_case(&v.name))
        .map_or_else(|| Some(v.name.clone()), |s| Some(s.to_string()))
}

/// A variable path as text, its first name as declared. The tree gives the
/// type of what it names.
fn variable(ctx: &Ctx, src: &str, pou: &str, path: &str) -> Result<(String, Option<Ty>), Refusal> {
    let bad = || Refusal::NotAVariable(path.to_string());
    if path.trim() != path || path.is_empty() {
        return Err(bad());
    }
    let r = Reparsed::of(path).map_err(|_| bad())?;
    if r.at(r.expr.start) != 0 || r.at(r.expr.end) != path.len() {
        return Err(bad());
    }
    let base = root(&r.expr).ok_or_else(bad)?;
    let text = match &base.kind {
        ExprKind::Var(n) => {
            if !path.get(..n.len()).is_some_and(|w| w.eq_ignore_ascii_case(n)) {
                return Err(bad());
            }
            let decl = spelled(ctx, src, pou, n).ok_or_else(|| Refusal::NoSuchVariable {
                name: n.clone(),
                pou: pou.to_string(),
            })?;
            format!("{decl}{}", &path[n.len()..])
        }
        _ => path.to_string(),
    };
    let ty = expr_ty(ctx, &pou.to_ascii_uppercase(), &r.expr);
    Ok((text, ty))
}

/// What the new text must read back as.
enum Want {
    Var(String),
    Const(Literal),
    Write(NodeId),
}

struct Resolved {
    text: String,
    found: Option<Ty>,
    want: Want,
    /// A bubble does not go with it.
    plain: bool,
}

fn resolve(
    ctx: &Ctx,
    src: &str,
    module: &Module,
    pou: &str,
    at_stmt: Option<usize>,
    to: &Source,
) -> Result<Resolved, Refusal> {
    match to {
        Source::Const(l) => Ok(Resolved {
            text: render(*l, None)?,
            found: Some(literal_ty(*l)),
            want: Want::Const(*l),
            plain: true,
        }),
        Source::Var(p) => {
            let (text, found) = variable(ctx, src, pou, p)?;
            Ok(Resolved {
                want: Want::Var(text.to_ascii_uppercase()),
                text,
                found,
                plain: false,
            })
        }
        Source::Write(n) => {
            let d = &pou_of(module, pou)?.diagram;
            check_node(d, *n)?;
            let NodeKind::Write { path } = &d.node(*n).kind else {
                return Err(Refusal::NotAWrite);
            };
            if at_stmt.is_some_and(|s| graph::stmt_ordinal(d, *n) >= s) {
                return Err(Refusal::NotWrittenYet(path.clone()));
            }
            let (text, found) = variable(ctx, src, pou, path)?;
            Ok(Resolved {
                text,
                found,
                want: Want::Write(*n),
                plain: false,
            })
        }
    }
}

/// What fed the pin must be a variable, a literal or an earlier write.
fn leaf(d: &Diagram, from: NodeId) -> Result<(), Refusal> {
    let n = d.node(from);
    match &n.kind {
        NodeKind::Read { .. }
        | NodeKind::Const { .. }
        | NodeKind::Write { .. }
        | NodeKind::Fb { .. }
        | NodeKind::For { .. } => Ok(()),
        NodeKind::Op { op } => Err(Refusal::NotALeaf(format!("the operator {op}"))),
        NodeKind::Func { name } => Err(Refusal::NotALeaf(format!("the call of {name}"))),
        NodeKind::Access { path } => Err(Refusal::NotALeaf(format!("the element {path}"))),
        NodeKind::Jump => Err(Refusal::NotALeaf("a jump".into())),
    }
}

// ---- Rewire ---------------------------------------------------------------

pub(super) fn rewire(src: &str, module: &Module, pin: &Pin, to: &Source) -> Result<Patch, Refusal> {
    let (pou, span, wire) = pin_of(module, pin)?;
    let d = &pou.diagram;
    leaf(d, wire.from)?;
    let ctx = Ctx::new(src)?;
    let stmt = graph::stmt_ordinal(d, pin.node);
    let new = resolve(&ctx, src, module, &pou.name, Some(stmt), to)?;
    let old = slice(src, span)?;
    // What the input takes: a bubble means BOOL, a block's port has its
    // declared type, anything else what it is wired from now.
    let node = d.node(pin.node);
    let expected = if wire.negated {
        Some(Ty {
            class: Class::Bool,
            name: "BOOL".into(),
        })
    } else {
        let port = match &node.kind {
            NodeKind::Fb { fb, .. } => ctx.types.declared(fb, &node.ins[pin.input]),
            _ => None,
        };
        match port {
            Some(t) => Some(class_of(&ctx.types, t)),
            None => expr_ty(&ctx, &pou.name, &Reparsed::of(old)?.expr),
        }
    };
    check_types(expected.as_ref(), new.found.as_ref())?;

    let keep = wire.negated && !new.plain;
    let mut text = if keep {
        format!("NOT {}", new.text)
    } else {
        new.text.clone()
    };
    if text.starts_with('-') && span.start > 0 && src.as_bytes()[span.start - 1] == b'-' {
        text.insert(0, ' ');
    }
    let patch = trim(span.start, old, &text);
    if patch.start == patch.end && patch.text.is_empty() {
        return Err(Refusal::NothingToChange);
    }
    verify_input(
        module,
        &patch.apply_to(src),
        &pou.name,
        pin.node,
        Hole::Replace(pin.input),
        &new.want,
        keep,
    )?;
    Ok(patch)
}

/// The POUs other than `except` are as they were.
fn others_same(old: &Module, new: &Module, except: &str) -> Result<(), Refusal> {
    if old.pous.len() != new.pous.len() {
        return Err(more("the programs and blocks of the source"));
    }
    for (a, b) in old.pous.iter().zip(&new.pous) {
        if a.name != b.name {
            return Err(more("the programs and blocks of the source"));
        }
        if a.name != except && !graph::same_pou(a, b) {
            return Err(more(&format!("the diagram of {}", a.name)));
        }
    }
    Ok(())
}

fn verify_input(
    old: &Module,
    new_src: &str,
    pou: &str,
    node: NodeId,
    hole: Hole,
    want: &Want,
    negated: bool,
) -> Result<(), Refusal> {
    let new = lowered(new_src).map_err(Refusal::WouldNotCompile)?;
    others_same(old, &new, pou)?;
    let (Some(po), Some(pn)) = (old.pou(pou), new.pou(pou)) else {
        return Err(more("the programs and blocks of the source"));
    };
    let at = graph::stmt_ordinal(&po.diagram, node);
    let id = |i: usize| Some(i);
    let od = graph::digest(
        &po.diagram,
        &Opts {
            remap: &id,
            target: Some((node, hole)),
            open_labels: Some(at),
        },
    );
    let nd = graph::digest(
        &pn.diagram,
        &Opts {
            remap: &id,
            target: None,
            open_labels: Some(at),
        },
    );
    let Ok(Some((neg, got))) = graph::compare(&od, &nd, false) else {
        return Err(more("the diagram around that input"));
    };
    let read_back = || more("the new source did not read back as asked");
    match (want, &got) {
        (Want::Var(path), Tree::Var(p, _)) if p.eq_ignore_ascii_case(path) => {}
        (Want::Const(l), Tree::Leaf(k))
            if *k
                == format!(
                    "{:?}",
                    NodeKind::Const {
                        value: Some(value_of(*l))
                    }
                ) => {}
        (Want::Write(n), Tree::Var(_, o)) if *o == graph::write_origin(&po.diagram, *n) => {}
        (Want::Write(n), _) => {
            let NodeKind::Write { path } = &po.diagram.node(*n).kind else {
                return Err(read_back());
            };
            return Err(Refusal::NotWrittenYet(path.clone()));
        }
        // `NOT x` that lowering kept as an operator: x is not a BOOL to it.
        (_, Tree::Node(..)) if negated => return Err(Refusal::NotBoolean),
        _ => return Err(read_back()),
    }
    if neg != negated {
        return Err(Refusal::NotBoolean);
    }
    Ok(())
}

// ---- SetFbArg -------------------------------------------------------------

fn find_call(body: &[Stmt], pos: ast::Pos) -> Option<&Expr> {
    for s in body {
        let hit = match &s.kind {
            StmtKind::Call(e) if e.pos == pos => Some(e),
            StmtKind::If(conds, other) => conds
                .iter()
                .find_map(|(_, b)| find_call(b, pos))
                .or_else(|| find_call(other, pos)),
            StmtKind::Case(_, arms, other) => arms
                .iter()
                .find_map(|(_, b)| find_call(b, pos))
                .or_else(|| find_call(other, pos)),
            StmtKind::For { body, .. } | StmtKind::While(_, body) | StmtKind::Repeat(body, _) => {
                find_call(body, pos)
            }
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

pub(super) fn set_fb_arg(
    src: &str,
    module: &Module,
    pou: &str,
    node: NodeId,
    arg: &str,
    to: &Source,
) -> Result<Patch, Refusal> {
    let p = pou_of(module, pou)?;
    let d = &p.diagram;
    check_node(d, node)?;
    let n = d.node(node);
    let NodeKind::Fb { fb, .. } = &n.kind else {
        return Err(Refusal::NotAFunctionBlock);
    };
    if let Some(input) = n.ins.iter().position(|x| x.eq_ignore_ascii_case(arg)) {
        let pin = Pin {
            pou: p.name.clone(),
            node,
            input,
        };
        return rewire(src, module, &pin, to);
    }
    let ctx = Ctx::new(src)?;
    if fb.is_empty() {
        return Err(Refusal::NotAFunctionBlock);
    }
    let not_input = || Refusal::NotAnInput {
        fb: fb.clone(),
        arg: arg.to_string(),
    };
    let user = ctx.unit.pous.iter().any(|q| q.name == *fb);
    let block = if user {
        ctx.pou(fb)?
    } else {
        ast::library()
            .pous
            .iter()
            .find(|q| q.name == *fb)
            .ok_or_else(not_input)?
    };
    let decl = block
        .vars
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(arg) && matches!(v.section, Section::Input | Section::InOut))
        .ok_or_else(not_input)?;
    let name = if user {
        let at = offset_in(src, decl.pos);
        src.get(at..at + decl.name.len())
            .unwrap_or(&decl.name)
            .to_string()
    } else {
        decl.name.clone()
    };
    let ast_pou = ctx.pou(&p.name)?;
    let call = find_call(&ast_pou.body, n.pos).ok_or(Refusal::Unreadable)?;
    let ExprKind::Call(callee, args) = &call.kind else {
        return Err(Refusal::Unreadable);
    };
    if args.iter().any(|a| a.name.is_none() && !a.output) {
        return Err(Refusal::UnnamedArguments);
    }
    let new = resolve(&ctx, src, module, &p.name, Some(graph::stmt_ordinal(d, node)), to)?;
    check_types(Some(&class_of(&ctx.types, &decl.ty)), new.found.as_ref())?;
    let b = src.as_bytes();
    let (at, text) = match args.iter().rfind(|a| !a.output).or(args.last()) {
        Some(last) => (
            offset_in(src, last.value.end),
            format!(", {name} := {}", new.text),
        ),
        None => {
            let open = skip_trivia(b, offset_in(src, callee.end));
            if b.get(open) != Some(&b'(') {
                return Err(Refusal::Unreadable);
            }
            (open + 1, format!("{name} := {}", new.text))
        }
    };
    let patch = Patch {
        start: at,
        end: at,
        text,
    };
    verify_input(
        module,
        &patch.apply_to(src),
        &p.name,
        node,
        Hole::Append,
        &new.want,
        false,
    )?;
    Ok(patch)
}

// ---- DeleteStmt -----------------------------------------------------------

fn stmt_spans(src: &str, body: &[Stmt], out: &mut Vec<(usize, usize)>) {
    for s in body {
        out.push((offset_in(src, s.pos), offset_in(src, s.end)));
        match &s.kind {
            StmtKind::If(conds, other) => {
                for (_, b) in conds {
                    stmt_spans(src, b, out);
                }
                stmt_spans(src, other, out);
            }
            StmtKind::Case(_, arms, other) => {
                for (_, b) in arms {
                    stmt_spans(src, b, out);
                }
                stmt_spans(src, other, out);
            }
            StmtKind::For { body, .. } | StmtKind::While(_, body) | StmtKind::Repeat(body, _) => {
                stmt_spans(src, body, out);
            }
            _ => {}
        }
    }
}

/// The end of a comment that starts at `i`, if one does and it ends on its
/// line (`//` runs to the end of it).
fn comment_end(src: &str, i: usize) -> Option<usize> {
    let rest = &src[i..];
    if rest.starts_with("//") {
        return Some(rest.find('\n').map_or(src.len(), |n| i + n));
    }
    let close = if rest.starts_with("(*") {
        "*)"
    } else if rest.starts_with("/*") {
        "*/"
    } else {
        return None;
    };
    let end = i + 2 + rest[2..].find(close)? + 2;
    (!src[i..end].contains('\n')).then_some(end)
}

/// The bytes that go with the statement at `s..e`.
///
/// A statement alone on its line (nothing but blanks before it, and after
/// it blanks and at most one comment that ends on the line) takes the whole
/// line with it, indentation, comment and line break. Otherwise only the
/// statement goes, with the blanks that kept it apart from its neighbour:
/// those after it where something follows on the line, before it where not.
/// A comment on a line that holds other things stays.
fn removal(src: &str, s: usize, e: usize) -> (usize, usize) {
    let b = src.as_bytes();
    let blank = |c: u8| c == b' ' || c == b'\t';
    let line = src[..s].rfind('\n').map_or(0, |i| i + 1);
    let alone_before = b[line..s].iter().all(|&c| blank(c));
    let mut a = e;
    while a < b.len() && blank(b[a]) {
        a += 1;
    }
    let mut after = a;
    if let Some(c) = comment_end(src, a) {
        after = c;
        while after < b.len() && blank(b[after]) {
            after += 1;
        }
    }
    let eol = |i: usize| i >= b.len() || b[i] == b'\n' || (b[i] == b'\r' && b.get(i + 1) == Some(&b'\n'));
    let line_end = |i: usize| {
        let mut j = i;
        while j < b.len() && b[j] != b'\n' {
            j += 1;
        }
        (j + 1).min(b.len())
    };
    if alone_before && eol(after) {
        return (line, line_end(after));
    }
    if eol(a) {
        // Something before it on the line, nothing after (a comment may
        // follow): take the blanks before it.
        let mut st = s;
        while st > line && blank(b[st - 1]) {
            st -= 1;
        }
        return (st, e);
    }
    (s, a)
}

pub(super) fn delete_stmt(src: &str, module: &Module, pou: &str, span: Span) -> Result<Patch, Refusal> {
    let p = pou_of(module, pou)?;
    let ctx = Ctx::new(src)?;
    let mut spans = Vec::new();
    stmt_spans(src, &ctx.pou(&p.name)?.body, &mut spans);
    if !spans.contains(&(span.start, span.end)) || span.start >= span.end {
        return Err(Refusal::NotAStatement);
    }
    let (from, to) = removal(src, span.start, span.end);
    let patch = Patch {
        start: from,
        end: to,
        text: String::new(),
    };
    let new = lowered(&patch.apply_to(src)).map_err(Refusal::WouldNotCompile)?;
    others_same(module, &new, &p.name)?;
    let Some(pn) = new.pou(&p.name) else {
        return Err(more("the programs and blocks of the source"));
    };
    // The statements of the diagram that lay inside the range go.
    let d = &p.diagram;
    let gone: Vec<bool> = d
        .stmts
        .iter()
        .map(|s| (span.start..span.end).contains(&offset_in(src, s.pos)))
        .collect();
    let remap = |i: usize| (!gone[i]).then(|| gone[..i].iter().filter(|g| !**g).count());
    let id = |i: usize| Some(i);
    let od = graph::digest(
        d,
        &Opts {
            remap: &remap,
            target: None,
            open_labels: None,
        },
    );
    let nd = graph::digest(
        &pn.diagram,
        &Opts {
            remap: &id,
            target: None,
            open_labels: None,
        },
    );
    if !matches!(graph::compare(&od, &nd, true), Ok(None)) {
        return Err(more("the statements that stay"));
    }
    Ok(patch)
}

// ---- Rename ---------------------------------------------------------------

/// `s` with every whole word equal (any case) to one of `names`, and not
/// after a dot (a member), replaced by a placeholder.
fn blank(s: &str, names: &[&str]) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(s.len());
    let mut prev = '\0';
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if word(c) && !word(prev) {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = it.peek() {
                if !word(d) {
                    break;
                }
                end = j + d.len_utf8();
                it.next();
            }
            let w = &s[i..end];
            if prev != '.' && names.iter().any(|n| n.eq_ignore_ascii_case(w)) {
                out.push('\u{1}');
            } else {
                out.push_str(w);
            }
            prev = s[end - 1..end].chars().next().unwrap_or('\0');
        } else {
            out.push(c);
            prev = c;
        }
    }
    out
}

fn blank_kind(k: &NodeKind, f: &dyn Fn(&str) -> String) -> NodeKind {
    match k {
        NodeKind::Read { path } => NodeKind::Read { path: f(path) },
        NodeKind::Op { op } => NodeKind::Op { op: f(op) },
        NodeKind::Func { name } => NodeKind::Func { name: f(name) },
        NodeKind::Fb { instance, fb } => NodeKind::Fb {
            instance: f(instance),
            fb: fb.clone(),
        },
        NodeKind::Access { path } => NodeKind::Access { path: f(path) },
        NodeKind::Write { path } => NodeKind::Write { path: f(path) },
        NodeKind::For { var } => NodeKind::For { var: f(var) },
        NodeKind::Const { .. } | NodeKind::Jump => k.clone(),
    }
}

/// The module with `names` blanked out everywhere a name is written, and
/// what moves with a longer or shorter name (columns, byte ranges) and
/// with the canvas (geometry) cleared.
fn without_names(m: &Module, names: &[&str]) -> Module {
    let f = |s: &str| blank(s, names);
    let mut m = plain(m);
    for p in &mut m.pous {
        p.vars = std::mem::take(&mut p.vars)
            .into_iter()
            .map(|(k, v)| (f(&k), v))
            .collect();
        let d = &mut p.diagram;
        d.pins.clear();
        d.gates.clear();
        d.ops.clear();
        d.size = (0.0, 0.0);
        for n in &mut d.nodes {
            n.kind = blank_kind(&n.kind, &f);
            n.label = f(&n.label);
            n.text = f(&n.text);
            n.ins = n.ins.iter().map(|s| f(s)).collect();
            n.outs = n.outs.iter().map(|s| f(s)).collect();
            n.pos.col = 0;
            n.rect = Rect::default();
        }
        for r in &mut d.regions {
            r.label = f(&r.label);
            r.pos.col = 0;
        }
        for s in &mut d.stmts {
            s.pos.col = 0;
        }
    }
    m
}

/// The byte range of the POU `p`: from its name to where the next POU or
/// type declaration starts (names are only renamed inside it).
fn pou_range(src: &str, unit: &Unit, p: &AstPou) -> (usize, usize) {
    let start = offset_in(src, p.pos);
    let end = unit
        .pous
        .iter()
        .map(|q| q.pos)
        .chain(unit.types.iter().map(|t| t.pos))
        .map(|pos| offset_in(src, pos))
        .filter(|&o| o > start)
        .min()
        .unwrap_or(src.len());
    (start, end)
}

/// The elementary types: names a variable could be given, but would hide.
const ELEMENTARY: &[&str] = &[
    "BOOL",
    "SINT",
    "INT",
    "DINT",
    "LINT",
    "USINT",
    "UINT",
    "UDINT",
    "ULINT",
    "BYTE",
    "WORD",
    "DWORD",
    "LWORD",
    "REAL",
    "LREAL",
    "TIME",
    "STRING",
    "WSTRING",
    "DATE",
    "TIME_OF_DAY",
    "TOD",
    "DATE_AND_TIME",
    "DT",
];

fn taken(ctx: &Ctx, pou: &AstPou, to: &str, own: &str) -> bool {
    let is = |n: &str| n.eq_ignore_ascii_case(to);
    ELEMENTARY.iter().any(|n| is(n))
        || pou
            .vars
            .iter()
            .any(|v| is(&v.name) && !v.name.eq_ignore_ascii_case(own))
        || ctx.unit.pous.iter().any(|q| is(&q.name))
        || ast::library().pous.iter().any(|q| is(&q.name))
        || ctx
            .unit
            .types
            .iter()
            .any(|t| is(&t.name) || matches!(&t.def, TypeDef::Enum(vs) if vs.iter().any(|(n, _, _)| is(n))))
        || ast::library().types.iter().any(|t| is(&t.name))
}

pub(super) fn rename(src: &str, module: &Module, pou: &str, from: &str, to: &str) -> Result<Patch, Refusal> {
    let p = pou_of(module, pou)?;
    if is_reserved_word(to) {
        return Err(Refusal::KeywordAsName(to.to_string()));
    }
    if let Err(m) = rename_ident("", "x", to) {
        return Err(Refusal::NotAName(m));
    }
    let ctx = Ctx::new(src)?;
    let ast_pou = ctx.pou(&p.name)?;
    let decl = ast_pou
        .vars
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(from))
        .ok_or_else(|| Refusal::NoSuchVariable {
            name: from.to_string(),
            pou: p.name.clone(),
        })?;
    if taken(&ctx, ast_pou, to, &decl.name) {
        return Err(Refusal::NameTaken(to.to_string()));
    }
    let (a, b) = pou_range(src, &ctx.unit, ast_pou);
    let (renamed, _) = rename_ident(&src[a..b], &decl.name, to).map_err(Refusal::NotAName)?;
    let new_src = format!("{}{renamed}{}", &src[..a], &src[b..]);
    if new_src == src {
        return Err(Refusal::NothingToChange);
    }
    let patch = trim(0, src, &new_src);
    let new = lowered(&new_src).map_err(Refusal::WouldNotCompile)?;
    let names = [decl.name.as_str(), to];
    if without_names(module, &names) != without_names(&new, &names) {
        return Err(more("the diagram, beyond the name"));
    }
    Ok(patch)
}
