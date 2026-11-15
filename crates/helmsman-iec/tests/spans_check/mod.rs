//! Shared by `tests/spans.rs` and helmsman-diagram's `tests/props.rs`
//! (`#[path]`): checks that every statement and expression of a parsed
//! program knows its exact source range.
//!
//! The contract: the text from a node's start to its `end` is non-empty,
//! begins and ends on a token (no whitespace or comment at either edge)
//! and parses, in the place the node came from, to the same tree as the
//! node, positions aside.

#![allow(dead_code)]

use helmsman_iec::ast::{self, Arg, Expr, ExprKind, Init, Pos, Stmt, StmtKind};

/// Byte offset of a 1-based line and column (the lexer counts bytes).
pub fn offset(src: &str, p: Pos) -> usize {
    let mut line = 1;
    let mut at = 0;
    for (i, b) in src.bytes().enumerate() {
        if line == p.line {
            at = i;
            break;
        }
        if b == b'\n' {
            line += 1;
            at = i + 1;
        }
    }
    assert_eq!(line, p.line, "line {} is past the end of the source", p.line);
    at + (p.col as usize - 1)
}

/// `{:?}` of a tree with every `Pos { .. }` replaced by `_`: two trees that
/// differ only in where they sit in the text print alike.
pub fn shape(tree: &impl std::fmt::Debug) -> String {
    let s = format!("{tree:?}");
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("Pos { line: ") {
        out.push_str(&rest[..i]);
        let j = rest[i..].find('}').expect("a Pos closes") + i + 1;
        out.push('_');
        rest = &rest[j..];
    }
    out.push_str(rest);
    out
}

fn wrap_stmt(text: &str) -> String {
    format!("PROGRAM p\n{text}\nEND_PROGRAM\n")
}

fn wrap_expr(text: &str) -> String {
    format!("PROGRAM p\nx := {text};\nEND_PROGRAM\n")
}

fn reparse_stmt(text: &str) -> Result<Stmt, String> {
    let unit = ast::parse(&wrap_stmt(text)).map_err(|e| format!("{e:?}"))?;
    let body = &unit.pous[0].body;
    if body.len() != 1 {
        return Err(format!("{} statements, not one", body.len()));
    }
    Ok(body[0].clone())
}

fn reparse_expr(text: &str) -> Result<Expr, String> {
    let unit = ast::parse(&wrap_expr(text)).map_err(|e| format!("{e:?}"))?;
    match unit.pous[0].body.as_slice() {
        [
            Stmt {
                kind: StmtKind::Assign(_, rhs),
                ..
            },
        ] => Ok(rhs.clone()),
        other => Err(format!("not one assignment: {other:?}")),
    }
}

fn slice(src: &str, from: Pos, to: Pos) -> Result<&str, String> {
    let (a, b) = (offset(src, from), offset(src, to));
    if a >= b {
        return Err(format!("empty or backwards range {from}..{to}"));
    }
    let text = src
        .get(a..b)
        .ok_or_else(|| format!("{from}..{to} splits a character"))?;
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        return Err(format!("{from}..{to} has whitespace at an edge: {text:?}"));
    }
    if text.ends_with("*)") && !text.contains("(*") || text.starts_with("(*") && !text.ends_with(')') {
        return Err(format!("{from}..{to} edge is a comment: {text:?}"));
    }
    Ok(text)
}

struct Walk<'a> {
    src: &'a str,
    seen: usize,
}

impl Walk<'_> {
    fn stmt(&mut self, s: &Stmt) -> Result<(), String> {
        self.seen += 1;
        let text = slice(self.src, s.pos, s.end)?;
        let back = reparse_stmt(text).map_err(|e| format!("statement {}..{} {text:?}: {e}", s.pos, s.end))?;
        if shape(&back) != shape(s) {
            return Err(format!(
                "statement {}..{} {text:?} parses to a different tree:\n  {}\n  {}",
                s.pos,
                s.end,
                shape(&back),
                shape(s)
            ));
        }
        match &s.kind {
            StmtKind::Assign(a, b) => {
                self.expr(a)?;
                self.expr(b)?;
            }
            StmtKind::Call(e) | StmtKind::While(e, _) => {
                self.expr(e)?;
                if let StmtKind::While(_, body) = &s.kind {
                    self.block(body)?;
                }
            }
            StmtKind::If(arms, els) => {
                for (c, b) in arms {
                    self.expr(c)?;
                    self.block(b)?;
                }
                self.block(els)?;
            }
            StmtKind::Case(e, arms, els) => {
                self.expr(e)?;
                for (_, b) in arms {
                    self.block(b)?;
                }
                self.block(els)?;
            }
            StmtKind::For {
                from, to, by, body, ..
            } => {
                self.expr(from)?;
                self.expr(to)?;
                if let Some(by) = by {
                    self.expr(by)?;
                }
                self.block(body)?;
            }
            StmtKind::Repeat(body, e) => {
                self.block(body)?;
                self.expr(e)?;
            }
            StmtKind::Exit | StmtKind::Continue | StmtKind::Return | StmtKind::Empty => {}
        }
        Ok(())
    }

    fn block(&mut self, b: &[Stmt]) -> Result<(), String> {
        b.iter().try_for_each(|s| self.stmt(s))
    }

    fn arg(&mut self, a: &Arg) -> Result<(), String> {
        self.expr(&a.value)
    }

    fn expr(&mut self, e: &Expr) -> Result<(), String> {
        self.seen += 1;
        let text = slice(self.src, e.start, e.end)?;
        let back =
            reparse_expr(text).map_err(|m| format!("expression {}..{} {text:?}: {m}", e.start, e.end))?;
        if shape(&back) != shape(e) {
            return Err(format!(
                "expression {}..{} {text:?} parses to a different tree:\n  {}\n  {}",
                e.start,
                e.end,
                shape(&back),
                shape(e)
            ));
        }
        match &e.kind {
            ExprKind::Member(a, _) | ExprKind::Bit(a, _) | ExprKind::Unary(_, a) => self.expr(a),
            ExprKind::Index(a, idx) => {
                self.expr(a)?;
                idx.iter().try_for_each(|i| self.expr(i))
            }
            ExprKind::Binary(_, a, b) => {
                self.expr(a)?;
                self.expr(b)
            }
            ExprKind::Call(f, args) => {
                self.expr(f)?;
                args.iter().try_for_each(|a| self.arg(a))
            }
            _ => Ok(()),
        }
    }

    fn init(&mut self, i: &Init) -> Result<(), String> {
        match i {
            Init::Expr(e) => self.expr(e),
            Init::Array(items, _) => items.iter().try_for_each(|(_, i)| self.init(i)),
            Init::Struct(fields, _) => fields.iter().try_for_each(|(_, _, i)| self.init(i)),
        }
    }
}

/// Checks every statement and expression in the bodies of `src`'s POUs, and
/// every expression in their variable declarations' initialisers (`VAR x :
/// INT := 1 + 2;`, array and structure initialisers included). Returns how
/// many nodes it checked.
///
/// # Errors
/// The first node whose range does not read back, or `src` not parsing.
pub fn check(src: &str) -> Result<usize, String> {
    let unit = ast::parse(src).map_err(|e| format!("{}: {}", e.pos, e.msg))?;
    let mut w = Walk { src, seen: 0 };
    for pou in &unit.pous {
        for v in &pou.vars {
            if let Some(init) = &v.init {
                w.init(init)?;
            }
        }
        w.block(&pou.body)?;
    }
    Ok(w.seen)
}

/// The text of the `n`th top-level statement of the first POU.
pub fn stmt_text(src: &str, n: usize) -> String {
    let unit = ast::parse(src).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let s = &unit.pous[0].body[n];
    src[offset(src, s.pos)..offset(src, s.end)].to_string()
}
