//! The structure the author wrote: the statements of an arm, and the runs
//! of statements under a comment.
//!
//! Both rules fold whole statements ([`Fold::over_stmts`]), so each is
//! offered on its own and never overlaps itself. Applied together, a fold
//! of one rule that overlaps one of the other is skipped by [`super::view`].
//!
//! ## Arms ([`arms`])
//!
//! One fold per IF, ELSIF, ELSE, CASE or loop arm that holds at least two
//! statements, or one statement that has arms of its own. Its title is the
//! arm's own label (`IF START AND NOT STOP`, `ELSE`, `CASE 1, 3..5`). Its
//! key is `arm:{n}:{k}`: `n` counts the statements of the POU's top level
//! (the IF, CASE or loop is the `n`th) and `k` is the arm's index in it,
//! so it holds while the rest of the program is edited.
//!
//! Only the **outermost** arms are offered: an arm inside another arm is
//! not, however many statements it has. Folds cannot overlap, and the
//! arm that holds it is the one a reader closes first. Once that one is
//! open, the canvas can ask for the arms of its view's diagram.
//!
//! ## Sections ([`sections`])
//!
//! A comment that stands alone on its line (nothing else on the line
//! before or after it) and has a statement of the POU's top level as the
//! next thing in the source names a section. The section runs from that
//! statement to the last top-level statement before the next section
//! comment, or the end of the body. Statements before the first one belong
//! to no section.
//!
//! - **Two comments in a row** above one statement: the last one names it.
//! - **A blank line** between the comment and the statement changes
//!   nothing: whitespace is skipped, as the lexer does.
//! - **A comment on a statement's line** (`x := 1; (* note *)`), one inside
//!   an arm, one with no statement after it, and an empty one (`(**)`) name
//!   nothing.
//! - **Title**: the text without the markers (`(*` `*)`, `/*` `*/`, `//`),
//!   trimmed, first line only, at most [`TITLE`] characters (longer is cut
//!   and ends in `…`).
//! - **Key**: `section:{pou}:{title}`, and `section:{pou}:{title}#2`, `#3`
//!   for a title used again in the POU.
//!
//! A section of one statement is offered only if that statement has arms.
//!
//! ## Statements ([`statements`])
//!
//! One fold per statement of the POU, at any depth (an IF inside an arm is
//! one too): the line the middle zoom draws ([`super::zoom`]). Its title is
//! the statement's own text with the comments taken out and the white space
//! run together, at most [`LINE`] characters (longer is cut and ends in
//! `…`). Its key is `stmt:{path}`: the statement's index at the top of the
//! POU (`stmt:2`), then for one nested in an arm `/{arm}.{index}` for each
//! level (`stmt:2/1.0` is the first statement of the second arm of the
//! third one). It holds while other statements are edited, as an arm's
//! does. The fold keeps no variable as a block: its inputs and outputs are
//! named pins, which makes it a line. Nothing is offered for a source that
//! does not parse, or a POU the source lacks.

use std::collections::HashMap;

use helmsman_iec::ast::{self, Stmt, StmtKind};

use super::{Fold, FoldKind};
use crate::edit::{offset_in, skip_trivia};
use crate::model::Diagram;

/// The longest section title, in characters.
pub const TITLE: usize = 40;

/// Whether a run of statements is worth a block of its own.
fn worth(d: &Diagram, run: &[usize]) -> bool {
    match run {
        [] => false,
        [one] => d.stmts.get(*one).is_some_and(|s| !s.arms.is_empty()),
        _ => true,
    }
}

/// One fold per outermost arm of `d` that holds two statements, or one
/// that has arms. See the module docs.
#[must_use]
pub fn arms(d: &Diagram) -> Vec<Fold> {
    let mut out = Vec::new();
    for (n, &s) in d.top.iter().enumerate() {
        let Some(stmt) = d.stmts.get(s) else { continue };
        for (k, &r) in stmt.arms.iter().enumerate() {
            let region = d.region(r);
            if region.parent.is_some() || !worth(d, &region.body) {
                continue;
            }
            let key = format!("arm:{n}:{k}");
            let fold = Fold::over_stmts(d, key, FoldKind::Arm, &region.label, &region.body);
            if !fold.nodes.is_empty() {
                out.push(fold);
            }
        }
    }
    out
}

/// A comment standing alone on its line(s).
struct Note {
    /// The title (see the module docs); never empty.
    title: String,
    /// Where the first thing after it that is not trivia starts.
    next: usize,
}

/// Every standalone comment of `src`, in order. Comments do not nest and
/// the language has no string literals, so one pass over the bytes finds
/// them as the lexer would.
fn notes(src: &str) -> Vec<Note> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let close = match (b[i], b.get(i + 1)) {
            (b'(', Some(b'*')) => Some(b')'),
            (b'/', Some(b'*')) => Some(b'/'),
            (b'/', Some(b'/')) => None,
            (b'{', _) => {
                // A pragma: what is in it is not a comment.
                i = b[i..]
                    .iter()
                    .position(|&c| c == b'}')
                    .map_or(b.len(), |p| i + p + 1);
                continue;
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let start = i;
        let (text_end, end) = if let Some(c) = close {
            let mut j = start + 2;
            while j < b.len() && !(b[j] == b'*' && b.get(j + 1) == Some(&c)) {
                j += 1;
            }
            (j.min(b.len()), (j + 2).min(b.len()))
        } else {
            let e = b[start..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(b.len(), |p| start + p);
            (e, e)
        };
        i = end.max(start + 1);
        let blank = |range: &[u8]| range.iter().all(u8::is_ascii_whitespace);
        let line_start = b[..start].iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1);
        let line_end = b[end..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |p| end + p);
        if !blank(&b[line_start..start]) || !blank(&b[end..line_end]) {
            continue;
        }
        // The markers are ASCII, so these are char boundaries.
        let text = src.get(start + 2..text_end).unwrap_or("");
        let first = text.trim().lines().next().unwrap_or("").trim();
        if first.is_empty() {
            continue;
        }
        let title = if first.chars().count() > TITLE {
            let cut: String = first.chars().take(TITLE - 1).collect();
            format!("{}…", cut.trim_end())
        } else {
            first.to_string()
        };
        out.push(Note {
            title,
            next: skip_trivia(b, end),
        });
    }
    out
}

/// One fold per section of the top level of `d`, a POU of the module whose
/// source is `src` (positions in `d` are positions in `src`). See the
/// module docs.
#[must_use]
pub fn sections(d: &Diagram, src: &str) -> Vec<Fold> {
    let notes = notes(src);
    // The title of the section each top-level statement starts, if any.
    let mut starts: Vec<(usize, String)> = Vec::new();
    for (at, &s) in d.top.iter().enumerate() {
        let Some(stmt) = d.stmts.get(s) else { continue };
        let offset = offset_in(src, stmt.pos);
        if let Some(note) = notes.iter().rev().find(|n| n.next == offset) {
            starts.push((at, note.title.clone()));
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for (i, (at, title)) in starts.iter().enumerate() {
        let to = starts.get(i + 1).map_or(d.top.len(), |(next, _)| *next);
        let run = &d.top[*at..to];
        let before = seen.iter().filter(|t| **t == title).count();
        seen.push(title);
        if !worth(d, run) {
            continue;
        }
        let key = if before == 0 {
            format!("section:{}:{title}", d.pou)
        } else {
            format!("section:{}:{title}#{}", d.pou, before + 1)
        };
        let fold = Fold::over_stmts(d, key, FoldKind::Section, title, run);
        if !fold.nodes.is_empty() {
            out.push(fold);
        }
    }
    out
}

/// The longest statement title, in characters.
pub const LINE: usize = 48;

/// Where each statement of `body` and of every block nested in it starts
/// and ends in `src`, by byte offset.
fn spans(src: &str, body: &[Stmt], out: &mut HashMap<usize, usize>) {
    for s in body {
        out.insert(offset_in(src, s.pos), offset_in(src, s.end));
        match &s.kind {
            StmtKind::If(conds, other) => {
                for (_, b) in conds {
                    spans(src, b, out);
                }
                spans(src, other, out);
            }
            StmtKind::Case(_, arms, other) => {
                for (_, b) in arms {
                    spans(src, b, out);
                }
                spans(src, other, out);
            }
            StmtKind::For { body, .. } | StmtKind::While(_, body) | StmtKind::Repeat(body, _) => {
                spans(src, body, out);
            }
            _ => {}
        }
    }
}

/// `text` with its comments taken out and its white space run together,
/// cut at [`LINE`] characters.
fn one_line(text: &str) -> String {
    let b = text.as_bytes();
    let mut line = String::new();
    let mut i = 0;
    while i < b.len() {
        let past = skip_trivia(b, i);
        if past > i {
            if !line.is_empty() && !line.ends_with(' ') {
                line.push(' ');
            }
            i = past;
        } else if let Some(c) = text.get(i..).and_then(|t| t.chars().next()) {
            line.push(c);
            i += c.len_utf8();
        } else {
            i += 1;
        }
    }
    let line = line.trim_end();
    if line.chars().count() > LINE {
        let cut: String = line.chars().take(LINE - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

/// Every statement of `d` with its path (see the module docs).
fn paths(d: &Diagram, run: &[usize], prefix: &str, out: &mut Vec<(usize, String)>) {
    for (j, &s) in run.iter().enumerate() {
        let path = if prefix.is_empty() {
            j.to_string()
        } else {
            format!("{prefix}.{j}")
        };
        let Some(stmt) = d.stmts.get(s) else { continue };
        out.push((s, path.clone()));
        for (k, &r) in stmt.arms.iter().enumerate() {
            paths(d, &d.region(r).body, &format!("{path}/{k}"), out);
        }
    }
}

/// One fold per statement of `d`, a POU of the module whose source is
/// `src` (positions in `d` are positions in `src`). See the module docs.
#[must_use]
pub fn statements(d: &Diagram, src: &str) -> Vec<Fold> {
    let Ok(unit) = ast::parse(src) else {
        return Vec::new();
    };
    let Some(pou) = unit.pous.iter().find(|p| p.name.eq_ignore_ascii_case(&d.pou)) else {
        return Vec::new();
    };
    let mut ends = HashMap::new();
    spans(src, &pou.body, &mut ends);
    let mut all = Vec::new();
    paths(d, &d.top, "", &mut all);
    let mut out = Vec::new();
    for (s, path) in all {
        let start = offset_in(src, d.stmts[s].pos);
        let Some(text) = ends.get(&start).and_then(|&end| src.get(start..end)) else {
            continue;
        };
        let fold = Fold::over_stmts(
            d,
            format!("stmt:{path}"),
            FoldKind::Statement,
            one_line(text),
            &[s],
        );
        if !fold.nodes.is_empty() {
            out.push(fold);
        }
    }
    out
}
