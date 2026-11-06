//! What the Agent may change (Nick 2026-11-01, card `agent-scoped-edits`;
//! interview 2026-11-02): the open file by default, the lines selected
//! in the Code tab or the block picked in the diagram when there is one.
//! The page says which rule applied, sends the scope to the model in an
//! `<edit_scope>` envelope, and on "Use this code" [`apply`] takes the
//! answer into the scope alone: an answer that would change anything
//! else (another file, lines outside the selection, outside the block)
//! is refused here, by the page, with what it tried to touch. Whatever
//! [`apply`] returns differs from the project only inside the scope,
//! byte for byte.
//!
//! Plain data and text, so it is tested natively and runs in the page.

use std::collections::BTreeMap;
use std::fmt;

use crate::claude::{MAX_PROGRAM, fences};

/// A project's files by name.
pub type Files = BTreeMap<String, String>;

/// The tags the page wraps the model's context in; an answer that
/// carries one is refused.
pub const ENVELOPES: [&str; 4] = ["edit_scope", "current_program", "compile_errors", "io_devices"];

/// What one answer may change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Nothing of the engineer's is open (a built-in example is
    /// read-only): a whole PROGRAM becomes a project of its own.
    NewProject,
    /// The open file, all of it.
    File(String),
    /// Lines `from..=to` of `file` (counted from 1), as selected.
    Lines { file: String, from: u32, to: u32 },
    /// The statement the block picked in the diagram was lowered from:
    /// lines `from..=to` of `file`.
    Block {
        file: String,
        name: String,
        from: u32,
        to: u32,
    },
}

/// What the page knows about the engineer's focus this frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    /// The open project may be written (not a built-in example).
    pub writable: bool,
    /// The ST file open in the Code tab.
    pub open: Option<String>,
    /// Lines selected in the Code tab: file, first and last line.
    pub selection: Option<(String, u32, u32)>,
    /// The block picked in the diagram: file, name, first and last line
    /// of its statement.
    pub block: Option<(String, String, u32, u32)>,
}

/// The scope [`Focus`] gives: a selection in the open file, else a block
/// picked in it, else the open file; nothing writable, a new project.
#[must_use]
pub fn detect(f: &Focus) -> Scope {
    let Some(open) = f.open.clone().filter(|_| f.writable) else {
        return Scope::NewProject;
    };
    if let Some((file, a, b)) = &f.selection
        && *file == open
        && *a >= 1
    {
        return Scope::Lines {
            file: open,
            from: *a.min(b),
            to: *a.max(b),
        };
    }
    if let Some((file, name, a, b)) = &f.block
        && *file == open
        && *a >= 1
    {
        return Scope::Block {
            file: open,
            name: name.clone(),
            from: *a.min(b),
            to: *a.max(b),
        };
    }
    Scope::File(open)
}

impl Scope {
    /// The file the scope is in.
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        match self {
            Self::NewProject => None,
            Self::File(f) | Self::Lines { file: f, .. } | Self::Block { file: f, .. } => Some(f),
        }
    }

    /// Its line range, for a selection or a block.
    #[must_use]
    pub const fn lines(&self) -> Option<(u32, u32)> {
        match self {
            Self::Lines { from, to, .. } | Self::Block { from, to, .. } => Some((*from, *to)),
            _ => None,
        }
    }

    /// The Agent tab's line: "Editing: lines 12–30 of main.st".
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::NewProject => "Editing: a new project".into(),
            Self::File(f) => format!("Editing: {f}"),
            Self::Lines { file, from, to } if from == to => format!("Editing: line {from} of {file}"),
            Self::Lines { file, from, to } => format!("Editing: lines {from}–{to} of {file}"),
            Self::Block { name, .. } => format!("Editing: block {name}"),
        }
    }

    /// Which rule gave it, in the engineer's words.
    #[must_use]
    pub const fn rule(&self) -> &'static str {
        match self {
            Self::NewProject => "the examples are read-only, so the program becomes a project of its own",
            Self::File(_) => "the file open in the Code tab",
            Self::Lines { .. } => "the lines selected in the Code tab",
            Self::Block { .. } => "the block picked in the diagram",
        }
    }

    /// What "outside" means in a refusal: "outside lines 12–30 of main.st".
    fn bounds(&self) -> String {
        match self {
            Self::NewProject => "outside a new project".into(),
            Self::File(f) => format!("outside {f}"),
            Self::Lines { file, from, to } => format!("outside {}", span(file, *from, *to)),
            Self::Block { file, name, from, to } => {
                format!("outside block {name} ({})", span(file, *from, *to))
            }
        }
    }

    /// The `<edit_scope>` envelope's content for project `project`: the
    /// scope, its rule, and the whole file as it is now, after the lines
    /// of a selection or a block as `section:` (the claude module
    /// neutralizes it). [`from_asked`] reads it back from a kept page.
    #[must_use]
    pub fn envelope(&self, project: &str, files: &Files) -> String {
        let mut m = format!("{} ({})\nproject: {project}\n", self.label(), self.rule());
        match self {
            Self::NewProject => {
                m.push_str("new project: answer with one complete PROGRAM");
            }
            Self::File(f) => {
                m.push_str(
                    &[
                        "file: ",
                        f,
                        "\nanswer with the whole of ",
                        f,
                        ", every line of it, with the edits made in it\ntext:\n",
                    ]
                    .concat(),
                );
                m.push_str(files.get(f).map_or("", String::as_str));
            }
            Self::Lines { file, from, to } | Self::Block { file, from, to, .. } => {
                m.push_str(
                    &[
                        "file: ",
                        file,
                        "\nlines: ",
                        &from.to_string(),
                        "-",
                        &to.to_string(),
                        "\n",
                    ]
                    .concat(),
                );
                if let Self::Block { name, .. } = self {
                    m.push_str(&["block: ", name, "\n"].concat());
                }
                // The whole file goes, the section beside it (Nick,
                // 2026-11-05: "send and receive the whole file so nothing
                // is missed"); only the section may change.
                m.push_str(
                    &[
                        "answer with the whole of ",
                        file,
                        ", every line of it, changed only in lines ",
                        &from.to_string(),
                        "-",
                        &to.to_string(),
                        "\nsection:\n",
                    ]
                    .concat(),
                );
                let text = files.get(file).map_or("", String::as_str);
                for l in text
                    .split_inclusive('\n')
                    .skip(from.saturating_sub(1) as usize)
                    .take(to.saturating_sub(*from) as usize + 1)
                {
                    m.push_str(l);
                }
                if !m.ends_with('\n') {
                    m.push('\n');
                }
                m.push_str("text:\n");
                m.push_str(text);
            }
        }
        m
    }
}

/// The project and scope a kept page's question went with: read from the
/// last `<edit_scope>` of its user message ([`Scope::envelope`]); `None`
/// for a page asked before scopes, or one whose envelope does not parse.
#[must_use]
pub fn from_asked(asked: &str) -> Option<(String, Scope)> {
    let at = asked.rfind("<edit_scope>\n")?;
    let body = &asked[at + "<edit_scope>\n".len()..];
    let field = |key: &str| {
        body.lines()
            .take_while(|l| *l != "text:" && *l != "section:")
            .find_map(|l| l.strip_prefix(key))
            .map(str::trim)
    };
    let project = field("project: ")?.to_string();
    let Some(file) = field("file: ") else {
        return field("new project:").map(|_| (project, Scope::NewProject));
    };
    let file = file.to_string();
    let Some(lines) = field("lines: ") else {
        return Some((project, Scope::File(file)));
    };
    let (a, b) = lines.split_once('-')?;
    let (from, to): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    if from == 0 || to < from {
        return None;
    }
    let scope = match field("block: ") {
        Some(name) => Scope::Block {
            file,
            name: name.to_string(),
            from,
            to,
        },
        None => Scope::Lines { file, from, to },
    };
    Some((project, scope))
}

fn span(file: &str, from: u32, to: u32) -> String {
    if from == to {
        format!("line {from} of {file}")
    } else {
        format!("lines {from}–{to} of {file}")
    }
}

/// Why an answer was not taken, and what it tried to touch (files, or
/// `file:lines`), for the notice and the Messages tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub why: String,
    pub touched: Vec<String>,
}

impl Refusal {
    fn new(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            touched: Vec::new(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.why)
    }
}

/// The project with `answer` taken into `scope`, or why not. Exactly one
/// closed code block is the change: for a file, the whole new file; for
/// lines or a block, the new text of those lines (a whole file is read
/// as one too, and refused if it changes anything outside them). For
/// [`Scope::NewProject`], the files of the new project (`main.st`).
///
/// # Errors
/// No code, more than one block, the page's envelope markers, a file or
/// a POU of another file, lines outside the scope, no change at all, or
/// code too large or holding control characters.
pub fn apply(scope: &Scope, files: &Files, answer: &str) -> Result<Files, Refusal> {
    let lower = answer.to_ascii_lowercase();
    if let Some(tag) = ENVELOPES
        .iter()
        .find(|t| lower.contains(&format!("<{t}>")) || lower.contains(&format!("</{t}>")))
    {
        return Err(Refusal::new(format!(
            "The answer carries the page's own <{tag}> markers, so none of it was taken."
        )));
    }
    // Scenario blocks are checks, not the change (`crate::verify`).
    let answer = &crate::verify::code_part(answer);
    if *scope == Scope::NewProject {
        let code = crate::claude::extract_program(answer)
            .ok_or_else(|| Refusal::new("The answer has no whole PROGRAM to start a project from."))?;
        return Ok(Files::from([(helmsman_project::MAIN.to_string(), code)]));
    }
    let Some(file) = scope.file() else {
        return Err(Refusal::new("Nothing to edit."));
    };
    let Some(old) = files.get(file) else {
        return Err(Refusal::new(format!("{file} is no longer in the project.")));
    };
    let blocks: Vec<_> = fences(answer)
        .into_iter()
        .filter(|f| !f.body.trim().is_empty())
        .collect();
    let block = match blocks.as_slice() {
        [] => return Err(Refusal::new("The answer has no code to use.")),
        [one] => one,
        many => {
            return Err(Refusal::new(format!(
                "The answer has {} code blocks; a change {} comes as one.",
                many.len(),
                scope.label().replacen("Editing: ", "to ", 1)
            )));
        }
    };
    let code = clean(&block.body)?;
    // A block named for another file, or marked as one.
    let named = block
        .info
        .split_whitespace()
        .skip(1)
        .map(|w| w.trim_start_matches("file=").trim_matches(['"', '\'']))
        .find(|w| w.contains('.'));
    let marked = code.lines().filter_map(file_marker);
    let mut others: Vec<String> = named
        .into_iter()
        .chain(marked)
        .filter(|n| !n.eq_ignore_ascii_case(file))
        .map(str::to_string)
        .collect();
    // POUs another file declares.
    for (pou, _) in pous(&code) {
        for (name, text) in files {
            if name != file
                && helmsman_project::file_name(name) == Some(helmsman_project::FileKind::St)
                && pous(text).iter().any(|(p, _)| *p == pou)
                && !others.contains(name)
            {
                others.push(name.clone());
            }
        }
    }
    if !others.is_empty() {
        let what = if others.len() > 1 {
            format!("the whole project ({})", others.join(", "))
        } else {
            others[0].clone()
        };
        return Err(Refusal {
            why: format!("The answer changes {what}, {}.", scope.bounds()),
            touched: others,
        });
    }
    let new = match scope.lines() {
        None => code,
        Some((from, to)) => region(scope, file, old, &code, from, to)?,
    };
    if new == *old {
        return Err(Refusal::new(format!("The answer leaves {file} as it is.")));
    }
    let mut out = files.clone();
    out.insert(file.to_string(), new);
    Ok(out)
}

/// `code` into lines `from..=to` of `old`. An answer that brings text
/// from outside the lines (the file's start or end, a POU's header or
/// footer that is not in them) is read as the whole file, and taken only
/// if what it changes lies within the lines.
fn region(scope: &Scope, file: &str, old: &str, code: &str, from: u32, to: u32) -> Result<String, Refusal> {
    if code == old {
        return Ok(old.to_string());
    }
    let f: Vec<&str> = old.split_inclusive('\n').collect();
    let a: Vec<&str> = code.split_inclusive('\n').collect();
    let (from, to) = (from as usize, (to as usize).min(f.len()));
    if from == 0 || from > f.len() {
        return Err(Refusal::new(format!("{file} no longer has line {from}.")));
    }
    let same = |x: &str, y: &str| x.trim_end() == y.trim_end();
    let p = f.iter().zip(&a).take_while(|(x, y)| same(x, y)).count();
    let s = f
        .iter()
        .rev()
        .zip(a.iter().rev())
        .take_while(|(x, y)| same(x, y))
        .count()
        .min(f.len().min(a.len()) - p);
    let outside: Vec<&str> = f[..from - 1].iter().chain(&f[to..]).copied().collect();
    let edges = |l: &&str| is_pou_edge(l) && outside.iter().any(|o| same(o, l));
    let whole =
        (from > 1 && p >= 1 && !f[0].trim().is_empty()) || (to < f.len() && s >= 1) || a.iter().any(edges);
    if !whole {
        let mut out: String = f[..from - 1].concat();
        out.push_str(code);
        out.push_str(&f[to..].concat());
        return Ok(out);
    }
    // Outside what it changes, the old bytes stay (a line equal but for
    // trailing blanks is the old one).
    let kept = || {
        let mut out = f[..p].concat();
        out.push_str(&a[p..a.len() - s].concat());
        out.push_str(&f[f.len() - s..].concat());
        out
    };
    // The old lines it changes: p+1 ..= len-s (none when it only inserts).
    let (lo, hi) = (p + 1, f.len() - s);
    if lo > hi {
        // A pure insertion before old line `lo` (none at all when as
        // long): inside when between the scope's first line and the line
        // after its last.
        if a.len() == f.len() || (lo >= from && lo <= to + 1) {
            return Ok(kept());
        }
        return Err(Refusal {
            why: format!(
                "The answer adds lines before line {lo} of {file}, {}.",
                scope.bounds()
            ),
            touched: vec![format!("{file}:{lo}")],
        });
    }
    if lo >= from && hi <= to {
        return Ok(kept());
    }
    let at = span(file, lo as u32, hi as u32);
    Err(Refusal {
        why: format!("The answer changes {at}, {}.", scope.bounds()),
        touched: vec![if lo == hi {
            format!("{file}:{lo}")
        } else {
            format!("{file}:{lo}-{hi}")
        }],
    })
}

/// A code block as text to land: `\n` line ends, no blank lines around
/// it, one final newline; refused over [`MAX_PROGRAM`] or with control
/// characters other than tab.
fn clean(body: &str) -> Result<String, Refusal> {
    let body = body.replace("\r\n", "\n").replace('\r', "\n");
    if body.len() > MAX_PROGRAM {
        return Err(Refusal::new(format!(
            "The answer's code is over {} KiB, more than the page takes.",
            MAX_PROGRAM / 1024
        )));
    }
    if body.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(Refusal::new("The answer's code holds control characters."));
    }
    Ok(format!("{}\n", body.trim_matches('\n')))
}

/// `(* file NAME *)`, the build's marker between files.
fn file_marker(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("(* file ")
        .and_then(|r| r.strip_suffix(" *)"))
        .map(str::trim)
}

const HEADS: [&str; 3] = ["PROGRAM", "FUNCTION_BLOCK", "FUNCTION"];

/// A line that opens or closes a POU.
fn is_pou_edge(line: &&str) -> bool {
    let w = first_word(line).to_ascii_uppercase();
    HEADS.contains(&w.as_str()) || matches!(w.as_str(), "END_PROGRAM" | "END_FUNCTION_BLOCK" | "END_FUNCTION")
}

fn first_word(line: &str) -> &str {
    let t = line.trim_start();
    let end = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(t.len());
    &t[..end]
}

/// The POUs `text` declares, upper-cased, with their lines.
#[must_use]
pub fn pous(text: &str) -> Vec<(String, u32)> {
    text.lines()
        .enumerate()
        .filter_map(|(k, l)| {
            let w = first_word(l);
            if !HEADS.iter().any(|h| h.eq_ignore_ascii_case(w)) {
                return None;
            }
            let name = first_word(&l.trim_start()[w.len()..]);
            (!name.is_empty()).then(|| {
                (
                    name.to_ascii_uppercase(),
                    u32::try_from(k + 1).unwrap_or(u32::MAX),
                )
            })
        })
        .collect()
}

/// The line a statement starting at `line`:`col` of `text` ends on: its
/// `;` outside comments, strings and the IF, CASE and loop bodies it
/// opens. The start line when none is found.
#[must_use]
pub fn statement_end(text: &str, line: u32, col: u32) -> u32 {
    let mut at = 0;
    for l in text.split_inclusive('\n').take(line.saturating_sub(1) as usize) {
        at += l.len();
    }
    let skip: usize = text[at.min(text.len())..]
        .chars()
        .take(col.saturating_sub(1) as usize)
        .map(char::len_utf8)
        .sum();
    let rest = &text[(at + skip).min(text.len())..];
    let b = rest.as_bytes();
    let (mut k, mut depth, mut lines) = (0, 0i32, line);
    while k < b.len() {
        match b[k] {
            b'\n' => lines += 1,
            b'(' if b.get(k + 1) == Some(&b'*') => {
                let end = rest[k + 2..].find("*)").map_or(b.len(), |e| k + 2 + e + 2);
                lines += u32::try_from(rest[k..end].matches('\n').count()).unwrap_or(0);
                k = end;
                continue;
            }
            b'/' if b.get(k + 1) == Some(&b'/') => {
                k = rest[k..].find('\n').map_or(b.len(), |e| k + e);
                continue;
            }
            q @ (b'\'' | b'"') => {
                k = rest[k + 1..].find(q as char).map_or(b.len(), |e| k + 1 + e + 1);
                continue;
            }
            b';' if depth <= 0 => return lines,
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let end = rest[k..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .map_or(b.len(), |e| k + e);
                match rest[k..end].to_ascii_uppercase().as_str() {
                    "IF" | "CASE" | "FOR" | "WHILE" | "REPEAT" => depth += 1,
                    "END_IF" | "END_CASE" | "END_FOR" | "END_WHILE" | "END_REPEAT" => depth -= 1,
                    _ => {}
                }
                k = end;
                continue;
            }
            _ => {}
        }
        k += 1;
    }
    line
}

/// The lines a selection of characters `a..b` (either order) of `text`
/// covers; `None` when nothing is selected. A selection that ends at the
/// start of a line leaves that line out.
#[must_use]
pub fn selected_lines(text: &str, a: usize, b: usize) -> Option<(u32, u32)> {
    let (a, b) = (a.min(b), a.max(b));
    if a == b {
        return None;
    }
    let line_of = |c: usize| {
        let n = text.chars().take(c).filter(|&x| x == '\n').count();
        u32::try_from(n + 1).unwrap_or(u32::MAX)
    };
    let first = line_of(a);
    let ends_at_line_start = b > 0 && text.chars().nth(b - 1) == Some('\n');
    let last = if ends_at_line_start {
        line_of(b) - 1
    } else {
        line_of(b)
    };
    Some((first, last.max(first)))
}
