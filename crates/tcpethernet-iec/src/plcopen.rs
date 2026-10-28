//! `PLCopen` TC6 XML (v2.01) import. A project's data types and POUs become
//! Structured Text, which then compiles like any other source; positions in
//! that text map back to the XML.
//!
//! ST bodies are copied. LD and FBD networks are lowered to statements in
//! the order Beremiz's generator uses: elements with an `executionOrderId`
//! first, then output variables and coils top to bottom (left to right on a
//! row), then the remaining blocks. A function block runs when first needed;
//! a function is evaluated where its output is used. Contacts and coils
//! become boolean expressions and assignments, edge modifiers become
//! `R_TRIG`/`F_TRIG` instances, an EN input guards the block's call (or, for
//! a function, the assignments that use its result), and an `inOutVariable`
//! is read before it is written, so feedback through a variable works.
//!
//! Not yet: SFC and IL bodies, actions, transitions, jumps and labels,
//! `VAR_IN_OUT` block parameters, anonymous inline types, and feedback that
//! does not pass through a variable. `<instances>` (configurations, tasks)
//! is ignored: the runtime runs the project's one PROGRAM.

#![allow(clippy::many_single_char_names)]

use std::collections::{HashMap, HashSet};

use crate::check::Diagnostic;
use crate::lexer::Pos;
use crate::parser::{ExprKind, parse_expr};
use crate::xml::{self, Element};

/// The generated Structured Text and where each of its lines came from.
#[derive(Debug, Clone)]
pub struct Lowered {
    st: String,
    origins: Vec<Origin>,
}

#[derive(Debug, Clone, Copy)]
enum Origin {
    /// Copied from an ST body: XML `line`, columns shifted by `shift`.
    Verbatim { line: u32, shift: u32 },
    /// Generated for the element at this position.
    Element(Pos),
}

impl Lowered {
    /// The Structured Text the project lowered to.
    #[must_use]
    pub fn st(&self) -> &str {
        &self.st
    }

    /// The XML position that a position in [`Lowered::st`] came from.
    #[must_use]
    pub fn origin(&self, p: Pos) -> Pos {
        let k = usize::try_from(p.line.saturating_sub(1)).unwrap_or(usize::MAX);
        match self.origins.get(k).or(self.origins.last()) {
            Some(Origin::Verbatim { line, shift }) => Pos {
                line: *line,
                col: p.col + shift,
            },
            Some(Origin::Element(e)) => *e,
            None => Pos { line: 1, col: 1 },
        }
    }
}

/// Lowers a `PLCopen` TC6 project to Structured Text.
///
/// # Errors
/// Every problem found, by XML position.
pub fn lower(src: &str) -> Result<Lowered, Vec<Diagnostic>> {
    let root = xml::parse(src).map_err(|e| vec![diag(e.pos, e.msg)])?;
    if root.name != "project" {
        return Err(vec![diag(
            root.pos,
            format!("expected a PLCopen <project>, found <{}>", root.name),
        )]);
    }
    let mut cx = Cx::default();
    let types = root.child("types");
    if let Some(dts) = types.and_then(|t| t.child("dataTypes")) {
        for dt in dts.children_named("dataType") {
            cx.data_type(dt);
        }
    }
    let pous: Vec<&Element> = types
        .and_then(|t| t.child("pous"))
        .map(|p| p.children_named("pou").collect())
        .unwrap_or_default();
    let user: HashSet<String> = pous
        .iter()
        .filter_map(|p| p.attr("name"))
        .map(str::to_ascii_uppercase)
        .collect();
    for pou in pous {
        cx.pou(pou, &user);
    }
    if cx.diags.is_empty() {
        Ok(Lowered {
            st: cx.st,
            origins: cx.origins,
        })
    } else {
        cx.diags.sort_by_key(|d| d.pos);
        Err(cx.diags)
    }
}

fn diag(pos: Pos, msg: impl Into<String>) -> Diagnostic {
    Diagnostic { pos, msg: msg.into() }
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.contains("__")
}

/// Problem already reported.
struct Failed;

type L<T> = Result<T, Failed>;

const ELEMENTARY: &[&str] = &[
    "BOOL", "BYTE", "WORD", "DWORD", "LWORD", "SINT", "INT", "DINT", "LINT", "USINT", "UINT", "UDINT",
    "ULINT", "REAL", "LREAL", "TIME", "DATE", "DT", "TOD",
];

#[derive(Default)]
struct Cx {
    st: String,
    origins: Vec<Origin>,
    diags: Vec<Diagnostic>,
}

impl Cx {
    fn err<T>(&mut self, pos: Pos, msg: impl Into<String>) -> L<T> {
        self.diags.push(diag(pos, msg));
        Err(Failed)
    }

    fn line(&mut self, at: Pos, s: &str) {
        self.st.push_str(s);
        self.st.push('\n');
        self.origins.push(Origin::Element(at));
    }

    fn verbatim(&mut self, text: &str, at: Pos) {
        for (k, l) in text.split('\n').enumerate() {
            self.st.push_str(l.trim_end_matches('\r'));
            self.st.push('\n');
            let k = u32::try_from(k).unwrap_or(u32::MAX);
            self.origins.push(Origin::Verbatim {
                line: at.line.saturating_add(k),
                shift: if k == 0 { at.col - 1 } else { 0 },
            });
        }
    }

    fn name(&mut self, e: &Element, attr: &str) -> L<String> {
        match e.attr(attr) {
            Some(n) if is_ident(n) => Ok(n.to_string()),
            Some(n) => self.err(e.pos, format!("<{}> {attr} {n:?} is not a valid name", e.name)),
            None => self.err(e.pos, format!("<{}> has no {attr}", e.name)),
        }
    }

    fn flag(&mut self, e: &Element, attr: &str) -> L<bool> {
        match e.attr(attr) {
            None | Some("false" | "0") => Ok(false),
            Some("true" | "1") => Ok(true),
            Some(v) => self.err(e.pos, format!("{attr}={v:?}: expected true or false")),
        }
    }

    /// The single element inside a wrapper such as `<type>`.
    fn only<'a>(&mut self, wrapper: &'a Element) -> L<&'a Element> {
        let mut it = wrapper.elements().filter(|e| e.name != "addData");
        match (it.next(), it.next()) {
            (Some(e), None) => Ok(e),
            _ => self.err(
                wrapper.pos,
                format!("<{}> must hold exactly one element", wrapper.name),
            ),
        }
    }

    /// A type inside a `<type>` or `<baseType>` wrapper.
    fn ty(&mut self, wrapper: &Element) -> L<String> {
        let e = self.only(wrapper)?;
        let n = e.name.as_str();
        match n {
            _ if ELEMENTARY.contains(&n) => Ok(n.to_string()),
            "string" | "wstring" => {
                let base = n.to_ascii_uppercase();
                match e.attr("length").map(str::parse::<u32>) {
                    None => Ok(base),
                    Some(Ok(len)) => Ok(format!("{base}[{len}]")),
                    Some(Err(_)) => self.err(e.pos, "string length is not a number"),
                }
            }
            "derived" => self.name(e, "name"),
            "array" => {
                let mut dims = Vec::new();
                for d in e.children_named("dimension") {
                    let lo = d.attr("lower").and_then(|v| v.parse::<i64>().ok());
                    let hi = d.attr("upper").and_then(|v| v.parse::<i64>().ok());
                    match (lo, hi) {
                        (Some(lo), Some(hi)) => dims.push(format!("{lo}..{hi}")),
                        _ => return self.err(d.pos, "array bounds must be integers"),
                    }
                }
                if dims.is_empty() {
                    return self.err(e.pos, "array without a dimension");
                }
                let Some(bt) = e.child("baseType") else {
                    return self.err(e.pos, "array without a baseType");
                };
                let elem = self.ty(bt)?;
                Ok(format!("ARRAY [{}] OF {elem}", dims.join(", ")))
            }
            "enum" | "struct" => self.err(
                e.pos,
                format!("anonymous {n} types are not supported; declare it under <dataTypes>"),
            ),
            "subrangeSigned" | "subrangeUnsigned" | "pointer" => {
                self.err(e.pos, format!("{n} types are not supported yet"))
            }
            _ => self.err(e.pos, format!("unknown type <{n}>")),
        }
    }

    /// Checks `text` is exactly one expression; returns it parenthesised
    /// unless it is a single operand.
    fn expr(&mut self, text: &str, at: Pos) -> L<String> {
        let t = text.trim();
        match parse_expr(t) {
            Ok(e) if atomic(&e.kind) => Ok(t.to_string()),
            Ok(_) => Ok(format!("({t})")),
            Err(e) => self.err(at, format!("{t:?}: {}", e.msg)),
        }
    }

    /// Checks `text` names a variable (or part of one).
    fn lvalue(&mut self, text: &str, at: Pos) -> L<String> {
        let t = text.trim();
        match parse_expr(t) {
            Ok(e)
                if matches!(
                    e.kind,
                    ExprKind::Var(_)
                        | ExprKind::Member(..)
                        | ExprKind::Index(..)
                        | ExprKind::Bit(..)
                        | ExprKind::Direct(_)
                ) =>
            {
                Ok(t.to_string())
            }
            Ok(_) => self.err(at, format!("{t:?} is not a variable")),
            Err(e) => self.err(at, format!("{t:?}: {}", e.msg)),
        }
    }

    /// The text of a child element (`<expression>`, `<variable>`).
    fn child_text(&mut self, e: &Element, child: &str) -> L<(String, Pos)> {
        match e.child(child) {
            Some(c) => {
                let (t, p) = c.text();
                Ok((t, p.unwrap_or(c.pos)))
            }
            None => self.err(e.pos, format!("<{}> has no <{child}>", e.name)),
        }
    }

    fn init(&mut self, e: &Element) -> L<String> {
        match e.name.as_str() {
            "initialValue" | "value" => {
                let v = self.only(e)?;
                self.init(v)
            }
            "simpleValue" => match e.attr("value") {
                Some(v) => self.expr(v, e.pos),
                None => self.err(e.pos, "simpleValue without a value"),
            },
            "arrayValue" => {
                let mut items = Vec::new();
                for v in e.children_named("value") {
                    let x = self.init(v)?;
                    match v.attr("repetitionValue").map(str::parse::<u32>) {
                        None => items.push(x),
                        Some(Ok(n)) => items.push(format!("{n}({x})")),
                        Some(Err(_)) => return self.err(v.pos, "repetitionValue is not a number"),
                    }
                }
                Ok(format!("[{}]", items.join(", ")))
            }
            "structValue" => {
                let mut items = Vec::new();
                for v in e.children_named("value") {
                    let m = self.name(v, "member")?;
                    let x = self.init(v)?;
                    items.push(format!("{m} := {x}"));
                }
                Ok(format!("({})", items.join(", ")))
            }
            n => self.err(e.pos, format!("unknown initial value <{n}>")),
        }
    }

    /// `name [AT addr] : type [:= init];`
    fn var_decl(&mut self, v: &Element) -> L<String> {
        let name = self.name(v, "name")?;
        let mut s = name;
        if let Some(a) = v.attr("address") {
            match parse_expr(a.trim()) {
                Ok(e) if matches!(e.kind, ExprKind::Direct(_)) => s = format!("{s} AT {}", a.trim()),
                _ => return self.err(v.pos, format!("address {a:?} is not a direct address")),
            }
        }
        let Some(t) = v.child("type") else {
            return self.err(v.pos, format!("variable {s} has no type"));
        };
        let t = self.ty(t)?;
        s = format!("{s} : {t}");
        if let Some(iv) = v.child("initialValue") {
            s = format!("{s} := {}", self.init(iv)?);
        }
        Ok(s + ";")
    }

    fn data_type(&mut self, dt: &Element) {
        let _ = self.data_type_inner(dt);
    }

    fn data_type_inner(&mut self, dt: &Element) -> L<()> {
        let name = self.name(dt, "name")?;
        let Some(bt) = dt.child("baseType") else {
            return self.err(dt.pos, format!("data type {name} has no baseType"));
        };
        let inner = self.only(bt)?;
        let init = match dt.child("initialValue") {
            Some(iv) => Some(self.init(iv)?),
            None => None,
        };
        match inner.name.as_str() {
            "struct" => {
                if init.is_some() {
                    return self.err(dt.pos, "initial values of a STRUCT go on its members");
                }
                let mut fields = Vec::new();
                for v in inner.children_named("variable") {
                    if let Ok(d) = self.var_decl(v) {
                        fields.push((v.pos, d));
                    }
                }
                self.line(dt.pos, &format!("TYPE {name} : STRUCT"));
                for (p, f) in fields {
                    self.line(p, &format!("  {f}"));
                }
                self.line(dt.pos, "END_STRUCT; END_TYPE");
            }
            "enum" => {
                if init.is_some() {
                    return self.err(
                        dt.pos,
                        "initial values of enumeration types are not supported yet",
                    );
                }
                let mut values = Vec::new();
                for v in inner
                    .child("values")
                    .into_iter()
                    .flat_map(|vs| vs.children_named("value"))
                {
                    let n = self.name(v, "name")?;
                    match v.attr("value").map(str::parse::<i64>) {
                        None => values.push(n),
                        Some(Ok(k)) => values.push(format!("{n} := {k}")),
                        Some(Err(_)) => return self.err(v.pos, "enumeration value is not an integer"),
                    }
                }
                if values.is_empty() {
                    return self.err(inner.pos, format!("enumeration {name} has no values"));
                }
                self.line(
                    dt.pos,
                    &format!("TYPE {name} : ({}); END_TYPE", values.join(", ")),
                );
            }
            _ => {
                let t = self.ty(bt)?;
                let init = init.map(|i| format!(" := {i}")).unwrap_or_default();
                self.line(dt.pos, &format!("TYPE {name} : {t}{init}; END_TYPE"));
            }
        }
        Ok(())
    }

    fn pou(&mut self, pou: &Element, user: &HashSet<String>) {
        let _ = self.pou_inner(pou, user);
    }

    #[allow(clippy::too_many_lines)]
    fn pou_inner(&mut self, pou: &Element, user: &HashSet<String>) -> L<()> {
        let name = self.name(pou, "name")?;
        let iface = pou.child("interface");
        let (head, end) = match pou.attr("pouType") {
            Some("program") => (format!("PROGRAM {name}"), "END_PROGRAM"),
            Some("functionBlock") => (format!("FUNCTION_BLOCK {name}"), "END_FUNCTION_BLOCK"),
            Some("function") => {
                let Some(rt) = iface.and_then(|i| i.child("returnType")) else {
                    return self.err(pou.pos, format!("function {name} has no returnType"));
                };
                (format!("FUNCTION {name} : {}", self.ty(rt)?), "END_FUNCTION")
            }
            other => {
                return self.err(
                    pou.pos,
                    format!("pouType {other:?} is not program, function or functionBlock"),
                );
            }
        };
        for extra in ["actions", "transitions"] {
            if let Some(a) = pou.child(extra)
                && a.elements().next().is_some()
            {
                return self.err(a.pos, format!("{extra} are not supported yet"));
            }
        }

        // Interface: (position, line) pairs, and the declared names.
        let mut decls: Vec<(Pos, String)> = Vec::new();
        let mut taken: HashSet<String> = HashSet::new();
        let mut ok = true;
        for list in iface.into_iter().flat_map(Element::elements) {
            let kw = match list.name.as_str() {
                "localVars" => "VAR",
                "tempVars" => "VAR_TEMP",
                "inputVars" => "VAR_INPUT",
                "outputVars" => "VAR_OUTPUT",
                "inOutVars" => "VAR_IN_OUT",
                "externalVars" => "VAR_EXTERNAL",
                "globalVars" => "VAR_GLOBAL",
                "returnType" | "addData" | "documentation" => continue,
                other => {
                    let _ = self.err::<()>(list.pos, format!("<{other}> is not supported"));
                    ok = false;
                    continue;
                }
            };
            let qual = if self.flag(list, "constant")? {
                " CONSTANT"
            } else if self.flag(list, "retain")? {
                " RETAIN"
            } else if self.flag(list, "nonretain")? {
                " NON_RETAIN"
            } else {
                ""
            };
            decls.push((list.pos, format!("{kw}{qual}")));
            for v in list.children_named("variable") {
                match self.var_decl(v) {
                    Ok(d) => {
                        if let Some(n) = v.attr("name") {
                            taken.insert(n.to_ascii_uppercase());
                        }
                        decls.push((v.pos, format!("  {d}")));
                    }
                    Err(Failed) => ok = false,
                }
            }
            decls.push((list.pos, "END_VAR".into()));
        }

        let mut bodies = pou.children_named("body");
        let body = bodies.next();
        if let Some(b) = bodies.next() {
            return self.err(b.pos, format!("{name} has more than one body"));
        }
        let lowered = match body {
            None => Body::None,
            Some(b) => {
                let lang = self.only_body(b)?;
                match lang.name.as_str() {
                    "ST" => Body::St(texts(lang)),
                    "LD" | "FBD" => {
                        let mut g = Graph::new(taken, user);
                        g.lower(lang);
                        if !g.diags.is_empty() {
                            self.diags.append(&mut g.diags);
                            return Err(Failed);
                        }
                        Body::Graph(Box::new(g))
                    }
                    "IL" => return self.err(lang.pos, "IL bodies are not supported"),
                    "SFC" => return self.err(lang.pos, "SFC bodies are not supported yet"),
                    other => return self.err(lang.pos, format!("unknown body language <{other}>")),
                }
            }
        };
        if !ok {
            return Err(Failed);
        }
        self.line(pou.pos, &head);
        for (p, d) in decls {
            self.line(p, &d);
        }
        match lowered {
            Body::None => {}
            Body::St(pieces) => {
                for (t, p) in pieces {
                    self.verbatim(t, p);
                }
            }
            Body::Graph(g) => {
                if !g.temps.is_empty() {
                    let at = body.map_or(pou.pos, |b| b.pos);
                    self.line(at, "VAR");
                    for (n, t) in &g.temps {
                        self.line(at, &format!("  {n} : {t};"));
                    }
                    self.line(at, "END_VAR");
                }
                for (p, s) in &g.lines {
                    self.line(*p, s);
                }
            }
        }
        self.line(pou.pos, end);
        Ok(())
    }

    fn only_body<'a>(&mut self, b: &'a Element) -> L<&'a Element> {
        let mut it = b
            .elements()
            .filter(|e| !matches!(e.name.as_str(), "addData" | "documentation"));
        match (it.next(), it.next()) {
            (Some(e), None) => Ok(e),
            _ => self.err(b.pos, "<body> must hold exactly one language"),
        }
    }
}

/// A POU body, ready to emit.
enum Body<'a> {
    None,
    St(Vec<(&'a str, Pos)>),
    Graph(Box<Graph>),
}

/// The non-blank character data pieces below `e`.
fn texts(e: &Element) -> Vec<(&str, Pos)> {
    let mut out = Vec::new();
    let mut stack = vec![e.children.iter()];
    while let Some(it) = stack.last_mut() {
        match it.next() {
            None => {
                stack.pop();
            }
            Some(xml::Node::Elem(c)) => stack.push(c.children.iter()),
            Some(xml::Node::Text(t, p)) if !t.trim().is_empty() => out.push((t.as_str(), *p)),
            Some(xml::Node::Text(..)) => {}
        }
    }
    out
}

const fn atomic(k: &ExprKind) -> bool {
    matches!(
        k,
        ExprKind::Int { .. }
            | ExprKind::Real { .. }
            | ExprKind::Bool(_)
            | ExprKind::Time(_)
            | ExprKind::EnumLit(..)
            | ExprKind::Var(_)
            | ExprKind::Direct(_)
            | ExprKind::Member(..)
            | ExprKind::Index(..)
            | ExprKind::Bit(..)
    )
}

/// A value on a wire: an expression, valid only where every guard (an EN
/// that fed a function on the way) is TRUE.
#[derive(Debug, Clone)]
struct Val {
    expr: String,
    guards: Vec<String>,
}

impl Val {
    fn plain(expr: impl Into<String>) -> Self {
        Self {
            expr: expr.into(),
            guards: Vec::new(),
        }
    }

    fn add_guards(&mut self, gs: &[String]) {
        for g in gs {
            if !self.guards.contains(g) {
                self.guards.push(g.clone());
            }
        }
    }
}

#[derive(Debug, Clone)]
enum Done {
    /// A function block call: outputs are `instance.OUTPUT`.
    Fb { inst: String, en: Option<String> },
    /// A function: its value, valid under `guards`.
    Func {
        out: String,
        val: Val,
        en: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    None,
    Rising,
    Falling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Storage {
    None,
    Set,
    Reset,
}

/// Lowers one LD or FBD network.
struct Graph {
    lines: Vec<(Pos, String)>,
    temps: Vec<(String, &'static str)>,
    diags: Vec<Diagnostic>,
    taken: HashSet<String>,
    user: HashSet<String>,
    memo: HashMap<(u64, String), Val>,
    blocks: HashMap<u64, Done>,
    busy: HashSet<u64>,
    failed: HashSet<u64>,
    counter: u32,
}

/// Elements an LD or FBD network may hold.
const OBJECTS: &[&str] = &[
    "leftPowerRail",
    "rightPowerRail",
    "contact",
    "coil",
    "block",
    "inVariable",
    "outVariable",
    "inOutVariable",
    "connector",
    "continuation",
    "comment",
    "return",
];

impl Graph {
    fn new(taken: HashSet<String>, user: &HashSet<String>) -> Self {
        Self {
            lines: Vec::new(),
            temps: Vec::new(),
            diags: Vec::new(),
            taken,
            user: user.clone(),
            memo: HashMap::new(),
            blocks: HashMap::new(),
            busy: HashSet::new(),
            failed: HashSet::new(),
            counter: 0,
        }
    }

    fn err<T>(&mut self, pos: Pos, msg: impl Into<String>) -> L<T> {
        self.diags.push(diag(pos, msg));
        Err(Failed)
    }

    /// Reuses [`Cx`]'s checks, collecting their diagnostics here.
    fn cx<T>(&mut self, f: impl FnOnce(&mut Cx) -> L<T>) -> L<T> {
        let mut cx = Cx::default();
        let r = f(&mut cx);
        self.diags.append(&mut cx.diags);
        r
    }

    fn fresh(&mut self, base: &str, ty: &'static str) -> String {
        loop {
            self.counter += 1;
            let n = format!("_{base}{}", self.counter);
            if self.taken.insert(n.to_ascii_uppercase()) {
                self.temps.push((n.clone(), ty));
                return n;
            }
        }
    }

    fn emit(&mut self, at: Pos, guards: &[String], stmt: String) {
        if guards.is_empty() {
            self.lines.push((at, stmt));
        } else {
            self.lines
                .push((at, format!("IF {} THEN {stmt} END_IF;", guards.join(" AND "))));
        }
    }

    fn edge(&mut self, e: &Element, attr: &str) -> L<Edge> {
        match e.attr(attr) {
            None | Some("none") => Ok(Edge::None),
            Some("rising") => Ok(Edge::Rising),
            Some("falling") => Ok(Edge::Falling),
            Some(v) => self.err(e.pos, format!("{attr}={v:?}: expected none, rising or falling")),
        }
    }

    fn storage(&mut self, e: &Element, attr: &str) -> L<Storage> {
        match e.attr(attr) {
            None | Some("none") => Ok(Storage::None),
            Some("set") => Ok(Storage::Set),
            Some("reset") => Ok(Storage::Reset),
            Some(v) => self.err(e.pos, format!("{attr}={v:?}: expected none, set or reset")),
        }
    }

    /// Applies a negation or an edge detector to a boolean value.
    fn modify(&mut self, e: &Element, v: Val, negated: &str, edge: &str) -> L<Val> {
        let neg = self.cx(|c| c.flag(e, negated))?;
        let edge = self.edge(e, edge)?;
        match (neg, edge) {
            (false, Edge::None) => Ok(v),
            (true, Edge::None) => Ok(Val {
                expr: format!("(NOT {})", v.expr),
                ..v
            }),
            (false, edge) => {
                let (base, fb) = if edge == Edge::Rising {
                    ("rtrig", "R_TRIG")
                } else {
                    ("ftrig", "F_TRIG")
                };
                let t = self.fresh(base, fb);
                self.emit(e.pos, &v.guards, format!("{t}(CLK := {});", v.expr));
                Ok(Val {
                    expr: format!("{t}.Q"),
                    ..v
                })
            }
            (true, _) => self.err(e.pos, "an element cannot be both negated and edge-triggered"),
        }
    }

    fn lower(&mut self, net: &Element) {
        let mut nodes: HashMap<u64, &Element> = HashMap::new();
        let mut connectors: HashMap<String, &Element> = HashMap::new();
        for e in net.elements() {
            if !OBJECTS.contains(&e.name.as_str()) {
                let what = match e.name.as_str() {
                    "jump" | "label" => "jumps and labels are not supported yet".to_string(),
                    n => format!("<{n}> is not supported in LD or FBD"),
                };
                let _ = self.err::<()>(e.pos, what);
                continue;
            }
            let Some(id) = e.attr("localId").and_then(|v| v.parse::<u64>().ok()) else {
                let _ = self.err::<()>(e.pos, format!("<{}> has no numeric localId", e.name));
                continue;
            };
            if nodes.insert(id, e).is_some() {
                let _ = self.err::<()>(e.pos, format!("localId {id} is used twice"));
            }
            if e.name == "connector" {
                match e.attr("name") {
                    Some(n) if connectors.insert(n.to_ascii_uppercase(), e).is_none() => {}
                    Some(n) => {
                        let _ = self.err::<()>(e.pos, format!("connector {n:?} is defined twice"));
                    }
                    None => {
                        let _ = self.err::<()>(e.pos, "connector without a name");
                    }
                }
            }
        }
        if !self.diags.is_empty() {
            return;
        }
        let mut readers: HashMap<u64, Vec<u64>> = HashMap::new();
        for (&id, e) in &nodes {
            let mut rs = Vec::new();
            refs(e, &mut rs);
            for r in rs {
                readers.entry(r).or_default().push(id);
            }
        }
        let net = Net {
            nodes,
            connectors,
            readers,
        };

        let order = |e: &Element| -> u64 {
            e.attr("executionOrderId")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        };
        let xy = |e: &Element| -> (f64, f64) {
            let p = e.child("position");
            let c = |a| {
                p.and_then(|p| p.attr(a))
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            };
            (c("y"), c("x"))
        };
        let by_xy = |a: &&Element, b: &&Element| {
            let (a, b) = (xy(a), xy(b));
            a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
        };
        let mut ordered: Vec<&Element> = Vec::new();
        let mut writes: Vec<&Element> = Vec::new();
        let mut blocks: Vec<&Element> = Vec::new();
        for e in net.nodes.values().copied() {
            let root = matches!(
                e.name.as_str(),
                "outVariable" | "inOutVariable" | "coil" | "block" | "return"
            );
            if !root {
                continue;
            }
            if order(e) > 0 {
                ordered.push(e);
            } else if e.name == "block" {
                blocks.push(e);
            } else {
                writes.push(e);
            }
        }
        ordered.sort_by(|a, b| order(a).cmp(&order(b)).then_with(|| by_xy(a, b)));
        writes.sort_by(by_xy);
        blocks.sort_by(by_xy);
        for e in ordered.into_iter().chain(writes).chain(blocks) {
            let _ = self.root(&net, e);
        }
    }

    fn root(&mut self, net: &Net, e: &Element) -> L<()> {
        let id = local_id(e);
        match e.name.as_str() {
            "block" => self.block(net, e, id).map(|_| ()),
            "return" => {
                let Some(v) = self.input(net, e.child("connectionPointIn"))? else {
                    return Ok(());
                };
                let stmt = if v.expr == "TRUE" {
                    "RETURN;".to_string()
                } else {
                    format!("IF {} THEN RETURN; END_IF;", v.expr)
                };
                self.emit(e.pos, &v.guards, stmt);
                Ok(())
            }
            kind => {
                let (target_tag, neg, edge, storage) = match kind {
                    "coil" => ("variable", "negated", "edge", "storage"),
                    "outVariable" => ("expression", "negated", "edge", "storage"),
                    _ => ("expression", "negatedIn", "edgeIn", "storageIn"),
                };
                let Some(v) = self.input(net, e.child("connectionPointIn"))? else {
                    return Ok(());
                };
                let (t, at) = self.cx(|c| c.child_text(e, target_tag))?;
                let target = self.cx(|c| c.lvalue(&t, at))?;
                let storage = self.storage(e, storage)?;
                if storage != Storage::None
                    && (self.cx(|c| c.flag(e, neg))? || self.edge(e, edge)? != Edge::None)
                {
                    return self.err(
                        e.pos,
                        "a set or reset coil cannot also be negated or edge-triggered",
                    );
                }
                let v = self.modify(e, v, neg, edge)?;
                let stmt = match storage {
                    Storage::None => format!("{target} := {};", v.expr),
                    Storage::Set => format!("IF {} THEN {target} := TRUE; END_IF;", v.expr),
                    Storage::Reset => format!("IF {} THEN {target} := FALSE; END_IF;", v.expr),
                };
                self.emit(e.pos, &v.guards, stmt);
                Ok(())
            }
        }
    }

    /// What arrives at a connection point: nothing, one source, or (in LD)
    /// several sources OR-ed together.
    fn input(&mut self, net: &Net, cpi: Option<&Element>) -> L<Option<Val>> {
        let Some(cpi) = cpi else { return Ok(None) };
        let mut vals = Vec::new();
        for c in cpi.children_named("connection") {
            let Some(r) = c.attr("refLocalId").and_then(|v| v.parse::<u64>().ok()) else {
                return self.err(c.pos, "connection without a numeric refLocalId");
            };
            let port = c.attr("formalParameter").filter(|p| !p.is_empty());
            vals.push(self.value(net, r, port, c.pos)?);
        }
        match vals.len() {
            0 => Ok(None),
            1 => Ok(vals.pop()),
            _ => {
                let mut out = Val::plain(format!(
                    "({})",
                    vals.iter()
                        .map(|v| v.expr.as_str())
                        .collect::<Vec<_>>()
                        .join(" OR ")
                ));
                for v in &vals {
                    out.add_guards(&v.guards);
                }
                Ok(Some(out))
            }
        }
    }

    fn required(&mut self, net: &Net, e: &Element, what: &str) -> L<Val> {
        match self.input(net, e.child("connectionPointIn"))? {
            Some(v) => Ok(v),
            None => self.err(e.pos, format!("{what} is not connected")),
        }
    }

    /// The value leaving element `id` at output `port`.
    fn value(&mut self, net: &Net, id: u64, port: Option<&str>, at: Pos) -> L<Val> {
        let Some(&e) = net.nodes.get(&id) else {
            return self.err(at, format!("connection to localId {id}, which does not exist"));
        };
        if self.failed.contains(&id) {
            return Err(Failed);
        }
        if e.name == "block" {
            return self.block_out(net, e, id, port, at);
        }
        let key = (id, String::new());
        if let Some(v) = self.memo.get(&key) {
            return Ok(v.clone());
        }
        if !self.busy.insert(id) {
            return self.err(
                e.pos,
                format!(
                    "feedback loop through <{}> {id}; route it through a variable",
                    e.name
                ),
            );
        }
        let r = self.compute(net, e);
        self.busy.remove(&id);
        if let Ok(v) = &r {
            self.memo.insert(key, v.clone());
        } else {
            self.failed.insert(id);
        }
        r
    }

    fn compute(&mut self, net: &Net, e: &Element) -> L<Val> {
        match e.name.as_str() {
            "leftPowerRail" => Ok(Val::plain("TRUE")),
            "inVariable" | "inOutVariable" => {
                let (t, at) = self.cx(|c| c.child_text(e, "expression"))?;
                let x = self.cx(|c| c.expr(&t, at))?;
                let (neg, edge) = if e.name == "inVariable" {
                    ("negated", "edge")
                } else {
                    ("negatedOut", "edgeOut")
                };
                self.modify(e, Val::plain(x), neg, edge)
            }
            "contact" => {
                let power = self.required(net, e, "contact")?;
                let (t, at) = self.cx(|c| c.child_text(e, "variable"))?;
                let x = self.cx(|c| c.expr(&t, at))?;
                let m = self.modify(e, Val::plain(x), "negated", "edge")?;
                let mut v = if power.expr == "TRUE" {
                    m
                } else {
                    Val::plain(format!("({} AND {})", power.expr, m.expr))
                };
                v.add_guards(&power.guards);
                // Read more than once (a branch, or a coil and what follows
                // it): capture the power now, before a coil changes a
                // variable it was computed from.
                if net.uses(local_id(e), 0) > 1 {
                    let t = self.fresh("pwr", "BOOL");
                    self.lines.push((e.pos, format!("{t} := {};", v.expr)));
                    v.expr = t;
                }
                Ok(v)
            }
            // Power flows on through a coil.
            "coil" => self.required(net, e, "coil"),
            "continuation" => {
                let name = e.attr("name").unwrap_or_default();
                let Some(&c) = net.connectors.get(&name.to_ascii_uppercase()) else {
                    return self.err(e.pos, format!("continuation {name:?} has no connector"));
                };
                self.required(net, c, &format!("connector {name:?}"))
            }
            n => self.err(e.pos, format!("<{n}> has no output to connect to")),
        }
    }

    fn block_out(&mut self, net: &Net, e: &Element, id: u64, port: Option<&str>, at: Pos) -> L<Val> {
        let done = self.block(net, e, id)?;
        let Some(port) = port else {
            return self.err(at, format!("connection to block {id} does not name an output"));
        };
        let en = match &done {
            Done::Fb { en, .. } | Done::Func { en, .. } => en.clone(),
        };
        if port.eq_ignore_ascii_case("ENO") {
            return Ok(Val::plain(en.unwrap_or_else(|| "TRUE".into())));
        }
        let Some(out) = e
            .child("outputVariables")
            .into_iter()
            .flat_map(|o| o.children_named("variable"))
            .find(|v| {
                v.attr("formalParameter")
                    .is_some_and(|f| f.eq_ignore_ascii_case(port))
            })
        else {
            return self.err(at, format!("block {id} has no output {port}"));
        };
        let v = match done {
            Done::Fb { inst, .. } => {
                if !is_ident(port) {
                    return self.err(at, format!("{port:?} is not a valid output name"));
                }
                Val::plain(format!("{inst}.{port}"))
            }
            Done::Func { out: name, val, .. } => {
                if !name.eq_ignore_ascii_case(port) {
                    return self.err(at, format!("function block {id} has one output, {name}"));
                }
                val
            }
        };
        let key = (id, port.to_ascii_uppercase());
        if let Some(v) = self.memo.get(&key) {
            return Ok(v.clone());
        }
        let v = self.modify(out, v, "negated", "edge")?;
        self.memo.insert(key, v.clone());
        Ok(v)
    }

    /// Emits a block's call (once) and records how to read its outputs.
    fn block(&mut self, net: &Net, e: &Element, id: u64) -> L<Done> {
        if let Some(d) = self.blocks.get(&id) {
            return Ok(d.clone());
        }
        if self.failed.contains(&id) {
            return Err(Failed);
        }
        if !self.busy.insert(id) {
            return self.err(
                e.pos,
                format!("feedback loop through block {id}; route it through a variable"),
            );
        }
        let r = self.block_inner(net, e);
        self.busy.remove(&id);
        if let Ok(d) = &r {
            self.blocks.insert(id, d.clone());
        } else {
            self.failed.insert(id);
        }
        r
    }

    fn block_inner(&mut self, net: &Net, e: &Element) -> L<Done> {
        let ty = self.cx(|c| c.name(e, "typeName"))?;
        let inst = match e.attr("instanceName").filter(|n| !n.is_empty()) {
            Some(_) => Some(self.cx(|c| c.name(e, "instanceName"))?),
            None => None,
        };
        if let Some(io) = e.child("inOutVariables")
            && io.elements().next().is_some()
        {
            return self.err(io.pos, "VAR_IN_OUT block parameters are not supported yet");
        }
        let mut en = None;
        let mut guards: Vec<String> = Vec::new();
        let mut ins: Vec<(String, String, Pos)> = Vec::new();
        for v in e
            .child("inputVariables")
            .into_iter()
            .flat_map(|i| i.children_named("variable"))
        {
            let formal = self.cx(|c| c.name(v, "formalParameter"))?;
            let Some(x) = self.input(net, v.child("connectionPointIn"))? else {
                continue;
            };
            let x = self.modify(v, x, "negated", "edge")?;
            if formal.eq_ignore_ascii_case("EN") {
                let t = self.fresh("en", "BOOL");
                self.emit(v.pos, &x.guards, format!("{t} := {};", x.expr));
                en = Some(t);
                continue;
            }
            for g in x.guards {
                if !guards.contains(&g) {
                    guards.push(g);
                }
            }
            ins.push((formal, x.expr, v.pos));
        }
        if let Some(t) = &en {
            guards.push(t.clone());
        }
        if let Some(inst) = inst {
            let args: Vec<String> = ins.iter().map(|(f, x, _)| format!("{f} := {x}")).collect();
            self.emit(e.pos, &guards, format!("{inst}({});", args.join(", ")));
            return Ok(Done::Fb { inst, en });
        }
        let outs: Vec<&Element> = e
            .child("outputVariables")
            .into_iter()
            .flat_map(|o| o.children_named("variable"))
            .filter(|v| {
                !v.attr("formalParameter")
                    .is_some_and(|f| f.eq_ignore_ascii_case("ENO"))
            })
            .collect();
        let [out] = outs.as_slice() else {
            return self.err(e.pos, format!("function {ty} must have exactly one output"));
        };
        let out = self.cx(|c| c.name(out, "formalParameter"))?;
        let expr = self.call(&ty, ins, e.pos)?;
        Ok(Done::Func {
            out,
            val: Val { expr, guards },
            en,
        })
    }

    /// A function block's value as an expression.
    fn call(&mut self, ty: &str, mut ins: Vec<(String, String, Pos)>, at: Pos) -> L<String> {
        let up = ty.to_ascii_uppercase();
        let numbered = |f: &str, prefix: &str| -> Option<u32> {
            f.to_ascii_uppercase()
                .strip_prefix(prefix)
                .and_then(|n| n.parse().ok())
        };
        let infix = |ins: &[(String, String, Pos)], op: &str| {
            let xs: Vec<&str> = ins.iter().map(|(_, x, _)| x.as_str()).collect();
            format!("({})", xs.join(&format!(" {op} ")))
        };
        let op = match up.as_str() {
            "ADD" => Some(("+", 2, usize::MAX)),
            "MUL" => Some(("*", 2, usize::MAX)),
            "AND" => Some(("AND", 2, usize::MAX)),
            "OR" => Some(("OR", 2, usize::MAX)),
            "XOR" => Some(("XOR", 2, usize::MAX)),
            "SUB" => Some(("-", 2, 2)),
            "DIV" => Some(("/", 2, 2)),
            "MOD" => Some(("MOD", 2, 2)),
            "EXPT" => Some(("**", 2, 2)),
            "NE" => Some(("<>", 2, 2)),
            "GT" => Some((">", 2, usize::MAX)),
            "GE" => Some((">=", 2, usize::MAX)),
            "EQ" => Some(("=", 2, usize::MAX)),
            "LE" => Some(("<=", 2, usize::MAX)),
            "LT" => Some(("<", 2, usize::MAX)),
            "NOT" | "MOVE" => Some(("", 1, 1)),
            _ => None,
        };
        if let Some((op, min, max)) = op {
            if ins.len() < min || ins.len() > max {
                return self.err(
                    at,
                    format!("{up} needs {min}..{max} connected inputs, has {}", ins.len()),
                );
            }
            if min == 1 {
                let x = &ins[0].1;
                return Ok(if up == "NOT" {
                    format!("(NOT {x})")
                } else {
                    x.clone()
                });
            }
            if ins.iter().all(|(f, _, _)| numbered(f, "IN").is_some()) {
                ins.sort_by_key(|(f, _, _)| numbered(f, "IN"));
            }
            if matches!(up.as_str(), "GT" | "GE" | "EQ" | "LE" | "LT") && ins.len() > 2 {
                let pairs: Vec<String> = ins.windows(2).map(|w| infix(w, op)).collect();
                return Ok(format!("({})", pairs.join(" AND ")));
            }
            return Ok(infix(&ins, op));
        }
        if self.user.contains(&up) {
            let args: Vec<String> = ins.iter().map(|(f, x, _)| format!("{f} := {x}")).collect();
            return Ok(format!("{ty}({})", args.join(", ")));
        }
        // Standard functions take positional arguments in their formal order.
        let formal: Option<&[&str]> = match up.as_str() {
            "SEL" => Some(&["G", "IN0", "IN1"]),
            "LIMIT" => Some(&["MN", "IN", "MX"]),
            "SHL" | "SHR" | "ROL" | "ROR" => Some(&["IN", "N"]),
            _ => None,
        };
        if let Some(formal) = formal {
            let mut sorted = Vec::new();
            for f in formal {
                match ins.iter().position(|(n, _, _)| n.eq_ignore_ascii_case(f)) {
                    Some(k) => sorted.push(ins.remove(k)),
                    None => return self.err(at, format!("input {f} of {up} is not connected")),
                }
            }
            if let Some((f, _, p)) = ins.first() {
                return self.err(*p, format!("{up} has no input {f}"));
            }
            ins = sorted;
        } else if up == "MUX" {
            let Some(k) = ins.iter().position(|(n, _, _)| n.eq_ignore_ascii_case("K")) else {
                return self.err(at, "input K of MUX is not connected");
            };
            let k = ins.remove(k);
            ins.sort_by_key(|(f, _, _)| numbered(f, "IN"));
            ins.insert(0, k);
        } else if ins.iter().all(|(f, _, _)| numbered(f, "IN").is_some()) {
            ins.sort_by_key(|(f, _, _)| numbered(f, "IN"));
        }
        let args: Vec<&str> = ins.iter().map(|(_, x, _)| x.as_str()).collect();
        Ok(format!("{ty}({})", args.join(", ")))
    }
}

struct Net<'a> {
    nodes: HashMap<u64, &'a Element>,
    connectors: HashMap<String, &'a Element>,
    /// For each localId, the elements whose inputs connect to it.
    readers: HashMap<u64, Vec<u64>>,
}

impl Net<'_> {
    /// How many times the value leaving `id` is read. Power passing through
    /// a coil is read again by whatever the coil feeds.
    fn uses(&self, id: u64, depth: usize) -> usize {
        if depth > self.nodes.len() {
            return 2;
        }
        self.readers.get(&id).map_or(0, |rs| {
            rs.iter()
                .map(|r| match self.nodes.get(r) {
                    Some(e) if e.name == "coil" => 1 + self.uses(*r, depth + 1),
                    _ => 1,
                })
                .sum()
        })
    }
}

/// Every `refLocalId` below `e`.
fn refs(e: &Element, out: &mut Vec<u64>) {
    for c in e.elements() {
        if c.name == "connection"
            && let Some(r) = c.attr("refLocalId").and_then(|v| v.parse().ok())
        {
            out.push(r);
        }
        refs(c, out);
    }
}

fn local_id(e: &Element) -> u64 {
    e.attr("localId").and_then(|v| v.parse().ok()).unwrap_or(0)
}
