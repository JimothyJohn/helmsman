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
//! ```
//!
//! - `helmsman-layout 1` first (an empty file is an empty arrangement).
//! - One `move` line per block: its offset from where the automatic
//!   layout puts it, in diagram units. Fields are tab-separated; a tab,
//!   line break or backslash inside one is written `\t`, `\n`, `\r`,
//!   `\\`.
//! - Blank lines and `#` comments are skipped.
//! - Bounded: at most [`MAX_MOVES`] moves, offsets finite and within
//!   [`MAX_OFFSET`], names and texts at most [`MAX_TEXT`] bytes. A file
//!   past any bound is refused whole, never half-read.

use std::fmt::Write as _;

/// The first line.
pub const HEADER: &str = "helmsman-layout 1";
/// Most moves in one file.
pub const MAX_MOVES: usize = 4096;
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

/// The moves `src` holds.
///
/// # Errors
/// Every problem, one line each, with its line number: the header
/// missing or another version, a line that is not a `move`, a bad field,
/// too many moves.
pub fn parse(src: &str) -> Result<Vec<Move>, Vec<String>> {
    let mut lines = src
        .lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.trim_end_matches('\r')));
    let mut moves = Vec::new();
    let mut errs = Vec::new();
    let first = lines
        .by_ref()
        .find(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    match first {
        None => return Ok(moves),
        Some((_, l)) if l.trim() == HEADER => {}
        Some((n, l)) => {
            return Err(vec![format!(
                "line {n}: expected `{HEADER}` first, got {}",
                crate::clean(l)
            )]);
        }
    }
    for (n, line) in lines {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if moves.len() == MAX_MOVES {
            errs.push(format!("line {n}: more than {MAX_MOVES} moves"));
            break;
        }
        let f: Vec<&str> = line.split('\t').collect();
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
            Ok(m) => moves.push(m),
            Err(e) => errs.push(format!("line {n}: {e}")),
        }
    }
    if errs.is_empty() { Ok(moves) } else { Err(errs) }
}

/// `moves` as a file [`parse`] reads back the same.
#[must_use]
pub fn render(moves: &[Move]) -> String {
    let mut out = String::from(HEADER);
    out.push('\n');
    for m in moves {
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
    out
}
