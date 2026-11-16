//! A project's diagram arrangement ([`crate::LAYOUT`], `diagram.layout`):
//! where an engineer moved blocks in the live diagram, saved as the
//! project's shared default (card `arrangement-persist`). It changes only
//! the drawing, never the program, and the build ignores it, so saving
//! one never changes what a deploy compiles.
//!
//! Blocks are named by what they are, not by their ids, so an edit that
//! renumbers them keeps the arrangement: the POU, the block's label and
//! source text, and which of the blocks with that label and text it is
//! (`k`, counted in diagram order).
//!
//! ```text
//! helmsman-layout 1
//! move<TAB>POU<TAB>K<TAB>DX<TAB>DY<TAB>LABEL<TAB>TEXT
//!
//! helmsman-layout 2
//! move<TAB>POU<TAB>K<TAB>DX<TAB>DY<TAB>LABEL<TAB>TEXT
//! open<TAB>POU<TAB>KEY
//! collapse<TAB>POU<TAB>KEY
//! foldmove<TAB>POU<TAB>KEY<TAB>DX<TAB>DY
//! ```
//!
//! - `helmsman-layout 1` or `helmsman-layout 2` first (an empty file is
//!   an empty arrangement). Version 2 is version 1 plus the fold lines
//!   below. A version 1 file with a fold line is refused, as a reader
//!   built before version 2 refuses a version 2 file whole.
//! - One `move` line per block: its offset from where the automatic
//!   layout puts it, in diagram units. Fields are tab-separated; a tab,
//!   line break or backslash inside one is written `\t`, `\n`, `\r`,
//!   `\\`.
//! - Fold state (version 2), by the fold's key (`Fold::key` in
//!   helmsman-diagram, the name a fold keeps across edits of the
//!   source): `open` for a fold that starts as one block and the reader
//!   opened, `collapse` for one that starts as its blocks (a section, an
//!   arm) and the reader folded, `foldmove` for how far a folded block
//!   was dragged from where the layout puts it. Each (POU, key) at most
//!   once per line kind. A key the program no longer has is no error
//!   here; the canvas drops it on load.
//! - [`render`] and [`render_all`] write version 1 whenever there is no
//!   fold state, so a project that never folds anything stays readable
//!   by pages and boxes built before version 2.
//! - Blank lines and `#` comments are skipped.
//! - Bounded: at most [`MAX_MOVES`] moves and [`MAX_FOLDS`] fold lines,
//!   offsets finite and within [`MAX_OFFSET`], names, keys and texts at
//!   most [`MAX_TEXT`] bytes. A file past any bound is refused whole,
//!   never half-read.

use std::fmt::Write as _;

/// The first line of a file without fold state, and what the writer
/// writes then.
pub const HEADER: &str = "helmsman-layout 1";
/// The first line of a file with fold state.
pub const HEADER_FOLDS: &str = "helmsman-layout 2";
/// Most moves in one file.
pub const MAX_MOVES: usize = 4096;
/// Most fold lines (`open`, `collapse`, `foldmove`) in one file.
pub const MAX_FOLDS: usize = 4096;
/// Largest offset either way, diagram units.
pub const MAX_OFFSET: f32 = 1.0e6;
/// Longest POU name, label or block text, bytes.
pub const MAX_TEXT: usize = 4096;

/// One block moved: which block, and by how much.
#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    pub pou: String,
    pub label: String,
    pub text: String,
    /// Which of the POU's blocks with this label and text, in order.
    pub k: usize,
    pub dx: f32,
    pub dy: f32,
}

/// A fold named by its POU and key, for `open` and `collapse`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FoldRef {
    pub pou: String,
    pub key: String,
}

/// A folded block dragged by (`dx`, `dy`) from where the layout puts it.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldMove {
    pub pou: String,
    pub key: String,
    pub dx: f32,
    pub dy: f32,
}

/// Everything a `diagram.layout` holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub moves: Vec<Move>,
    /// Folds that start as one block and the reader opened.
    pub opened: Vec<FoldRef>,
    /// Folds that start as their blocks and the reader folded.
    pub collapsed: Vec<FoldRef>,
    pub fold_moves: Vec<FoldMove>,
}

impl Layout {
    /// Whether there is fold state, which is what makes a file version 2.
    #[must_use]
    pub fn has_folds(&self) -> bool {
        !(self.opened.is_empty() && self.collapsed.is_empty() && self.fold_moves.is_empty())
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match it.next() {
            Some('\\') => '\\',
            Some('t') => '\t',
            Some('n') => '\n',
            Some('r') => '\r',
            Some(o) => return Err(format!("unknown escape \\{}", o.escape_debug())),
            None => return Err("a lone \\ at the end of a field".into()),
        });
    }
    Ok(out)
}

fn offset(s: &str) -> Result<f32, String> {
    let v: f32 = s
        .parse()
        .map_err(|_| format!("{} is not a number", crate::clean(s)))?;
    if v.is_finite() && v.abs() <= MAX_OFFSET {
        Ok(v)
    } else {
        Err(format!("offset {} is past ±{MAX_OFFSET}", crate::clean(s)))
    }
}

fn field(s: &str, what: &str) -> Result<String, String> {
    let v = unescape(s)?;
    if v.len() > MAX_TEXT {
        return Err(format!("{what} longer than {MAX_TEXT} bytes"));
    }
    Ok(v)
}

/// The moves `src` holds (fold state is read and checked, then left out:
/// see [`parse_all`]).
///
/// # Errors
/// What [`parse_all`] refuses.
pub fn parse(src: &str) -> Result<Vec<Move>, Vec<String>> {
    parse_all(src).map(|l| l.moves)
}

fn pou_key(pou: &str, key: &str) -> Result<(String, String), String> {
    let pou = field(pou, "the POU name")?;
    if pou.is_empty() {
        return Err("no POU name".into());
    }
    let key = field(key, "the fold key")?;
    if key.is_empty() {
        return Err("no fold key".into());
    }
    Ok((pou, key))
}

/// One fold line of a version 2 file, added to `out`.
fn fold_line(out: &mut Layout, f: &[&str]) -> Result<(), String> {
    match f {
        [word @ ("open" | "collapse"), pou, key] => {
            let (pou, key) = pou_key(pou, key)?;
            let list = if *word == "open" {
                &mut out.opened
            } else {
                &mut out.collapsed
            };
            let r = FoldRef { pou, key };
            if list.contains(&r) {
                return Err(format!("{} is already listed under {word}", crate::clean(&r.key)));
            }
            list.push(r);
            Ok(())
        }
        ["foldmove", pou, key, dx, dy] => {
            let (pou, key) = pou_key(pou, key)?;
            if out.fold_moves.iter().any(|m| m.pou == pou && m.key == key) {
                return Err(format!("{} is already moved", crate::clean(&key)));
            }
            let (dx, dy) = (offset(dx)?, offset(dy)?);
            out.fold_moves.push(FoldMove { pou, key, dx, dy });
            Ok(())
        }
        ["foldmove", ..] => Err("expected foldmove<TAB>POU<TAB>KEY<TAB>DX<TAB>DY".into()),
        _ => Err("expected open or collapse<TAB>POU<TAB>KEY".into()),
    }
}

/// Everything `src` holds.
///
/// # Errors
/// Every problem, one line each, with its line number: the header
/// missing or another version, a line that is not one this version has
/// (a fold line under version 1 included), a bad or repeated field, too
/// many moves or fold lines.
pub fn parse_all(src: &str) -> Result<Layout, Vec<String>> {
    let mut lines = src
        .lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.trim_end_matches('\r')));
    let mut out = Layout::default();
    let mut errs = Vec::new();
    let first = lines
        .by_ref()
        .find(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let v2 = match first {
        None => return Ok(out),
        Some((_, l)) if l.trim() == HEADER => false,
        Some((_, l)) if l.trim() == HEADER_FOLDS => true,
        Some((n, l)) => {
            return Err(vec![format!(
                "line {n}: expected `{HEADER}` or `{HEADER_FOLDS}` first, got {}",
                crate::clean(l)
            )]);
        }
    };
    let mut folds = 0usize;
    for (n, line) in lines {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if matches!(f[0], "open" | "collapse" | "foldmove") {
            if !v2 {
                errs.push(format!(
                    "line {n}: `{}` needs `{HEADER_FOLDS}`, this file is `{HEADER}`",
                    f[0]
                ));
                continue;
            }
            if folds == MAX_FOLDS {
                errs.push(format!("line {n}: more than {MAX_FOLDS} fold lines"));
                break;
            }
            folds += 1;
            if let Err(e) = fold_line(&mut out, &f) {
                errs.push(format!("line {n}: {e}"));
            }
            continue;
        }
        if v2 && f[0] != "move" {
            errs.push(format!(
                "line {n}: expected move, open, collapse or foldmove, got {}",
                crate::clean(line)
            ));
            continue;
        }
        if out.moves.len() == MAX_MOVES {
            errs.push(format!("line {n}: more than {MAX_MOVES} moves"));
            break;
        }
        let parsed = (|| {
            let ["move", pou, k, dx, dy, label, text] = f.as_slice() else {
                return Err("expected move<TAB>POU<TAB>K<TAB>DX<TAB>DY<TAB>LABEL<TAB>TEXT".to_string());
            };
            let pou = field(pou, "the POU name")?;
            if pou.is_empty() {
                return Err("no POU name".into());
            }
            Ok(Move {
                pou,
                k: k.parse()
                    .ok()
                    .filter(|k| *k < MAX_MOVES * 16)
                    .ok_or_else(|| format!("{} is not a block number", crate::clean(k)))?,
                dx: offset(dx)?,
                dy: offset(dy)?,
                label: field(label, "the label")?,
                text: field(text, "the text")?,
            })
        })();
        match parsed {
            Ok(m) => out.moves.push(m),
            Err(e) => errs.push(format!("line {n}: {e}")),
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

/// `moves` as a file [`parse`] reads back the same (version 1).
#[must_use]
pub fn render(moves: &[Move]) -> String {
    render_all(&Layout {
        moves: moves.to_vec(),
        ..Layout::default()
    })
}

/// `layout` as a file [`parse_all`] reads back the same: version 1 when
/// it has no fold state (byte for byte what [`render`] always wrote),
/// version 2 when it has.
#[must_use]
pub fn render_all(layout: &Layout) -> String {
    let mut out = String::from(if layout.has_folds() { HEADER_FOLDS } else { HEADER });
    out.push('\n');
    for m in &layout.moves {
        let _ = writeln!(
            out,
            "move\t{}\t{}\t{}\t{}\t{}\t{}",
            escape(&m.pou),
            m.k,
            m.dx,
            m.dy,
            escape(&m.label),
            escape(&m.text)
        );
    }
    for (word, list) in [("open", &layout.opened), ("collapse", &layout.collapsed)] {
        for r in list {
            let _ = writeln!(out, "{word}\t{}\t{}", escape(&r.pou), escape(&r.key));
        }
    }
    for m in &layout.fold_moves {
        let _ = writeln!(
            out,
            "foldmove\t{}\t{}\t{}\t{}",
            escape(&m.pou),
            escape(&m.key),
            m.dx,
            m.dy
        );
    }
    out
}
