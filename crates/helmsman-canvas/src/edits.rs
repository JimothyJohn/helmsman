//! An answer given as the lines that change (TODO.md
//! `agent-edits-not-files`), turned back into the whole file the checks
//! read.
//!
//! The Agent rewrote the whole file for every instruction, so on a long
//! program most of the wait was the file coming back unchanged. An agent
//! whose prompt asks for it (the relay's `edits` agent) sends each place
//! that changes as a block marked ```` ```st edit ````: the lines as they
//! are, a line of equals signs, the lines as they are to be. [`expand`]
//! puts the blocks into the scope's file and hands on the answer in the
//! form every later step already takes (`crate::verify`, `crate::scope`):
//! the whole file in one block, the scenarios as they came. An answer
//! without such a block passes through untouched, so an agent that still
//! sends whole files is checked as before.
//!
//! A block applies only where its lines stand in the file exactly once.
//! Nothing is guessed: lines that are not there, or are there twice, are
//! refused in words the Agent can act on, and nothing of the answer is
//! taken.

use crate::claude::{Segment, segments};
use crate::scope::{Files, Scope};
use crate::verify::is_scenario;

/// Most edit blocks one answer may carry.
pub const MAX_EDITS: usize = 64;

/// Whether a fence's info string marks an edit (```` ```st edit ````).
#[must_use]
pub fn is_edit(info: &str) -> bool {
    !is_scenario(info) && info.split_whitespace().any(|w| w.eq_ignore_ascii_case("edit"))
}

/// One place that changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The lines as they stand in the file.
    pub was: Vec<String>,
    /// The lines that take their place.
    pub now: Vec<String>,
}

/// Whether `line` divides a block: seven or more `=` and nothing else.
fn divider(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 7 && t.bytes().all(|b| b == b'=')
}

/// The lines of `text`, blank ones at either end dropped.
fn lines(text: &[&str]) -> Vec<String> {
    let first = text
        .iter()
        .position(|l| !l.trim().is_empty())
        .unwrap_or(text.len());
    let last = text
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(first, |k| k + 1);
    text[first..last].iter().map(|l| (*l).to_string()).collect()
}

/// A block's body as an [`Edit`].
///
/// # Errors
/// It has no divider, more than one, or no lines above it.
pub fn parse(body: &str) -> Result<Edit, String> {
    let all: Vec<&str> = body.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut at = all.iter().enumerate().filter(|(_, l)| divider(l)).map(|(k, _)| k);
    let (Some(k), None) = (at.next(), at.next()) else {
        return Err(
            "it needs exactly one line of seven equals signs (=======) between the lines as they are and the lines as they are to be"
                .into(),
        );
    };
    let was = lines(&all[..k]);
    if was.is_empty() {
        return Err(
            "it has no lines above the =======: to add lines, give the line they come after and repeat it below with the new lines under it"
                .into(),
        );
    }
    Ok(Edit {
        was,
        now: lines(&all[k + 1..]),
    })
}

/// Where `was` stands in `file`, comparing lines by `same`: the index of
/// its first line for each place.
fn places(file: &[String], was: &[String], same: impl Fn(&str, &str) -> bool) -> Vec<usize> {
    if was.len() > file.len() {
        return Vec::new();
    }
    (0..=file.len() - was.len())
        .filter(|&k| was.iter().zip(&file[k..]).all(|(w, f)| same(w, f)))
        .collect()
}

/// `line` in a few words, for a message.
fn quoted(line: &str) -> String {
    let t = line.trim();
    if t.chars().count() <= 60 {
        format!("`{t}`")
    } else {
        format!("`{}…`", t.chars().take(60).collect::<String>())
    }
}

/// `edits` put into `old`, one after the other. A block's lines are
/// looked for as they stand (white space at a line's end aside); when
/// they are nowhere, once more with the indentation aside, since a model
/// often gets a line right and its indentation wrong. Either way they
/// must be there exactly once.
///
/// # Errors
/// Which block failed and why, for the Agent.
pub fn apply(old: &str, edits: &[Edit]) -> Result<String, String> {
    let mut file: Vec<String> = old.replace("\r\n", "\n").lines().map(str::to_string).collect();
    let n = edits.len();
    for (k, e) in edits.iter().enumerate() {
        let which = if n == 1 {
            "The edit block".to_string()
        } else {
            format!("Edit block {} of {n}", k + 1)
        };
        let exact = places(&file, &e.was, |a, b| a.trim_end() == b.trim_end());
        let found = if exact.is_empty() {
            places(&file, &e.was, |a, b| a.trim() == b.trim())
        } else {
            exact
        };
        match found.as_slice() {
            [at] => {
                file.splice(*at..*at + e.was.len(), e.now.iter().cloned());
            }
            [] => {
                return Err(format!(
                    "{which}: its lines above the ======= (starting {}) are not in the file as it stands. Copy them from the file exactly, whole lines; nothing of this answer was taken, so the file is still the one you were sent.",
                    quoted(&e.was[0])
                ));
            }
            many => {
                return Err(format!(
                    "{which}: its lines above the ======= (starting {}) stand in the file {} times. Take a line before or after them along so they stand there once.",
                    quoted(&e.was[0]),
                    many.len()
                ));
            }
        }
    }
    let mut out = file.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// `answer` with its edit blocks put into the scope's file: the whole
/// file in one ```` ```st ```` block where the first edit block stood,
/// everything else as it came. An answer without an edit block, or one
/// cut off inside a block (the checks say so), comes back as it is.
///
/// # Errors
/// Why the edits cannot be taken, for the Agent: a block that does not
/// apply ([`apply`], [`parse`]), edits beside a whole file, edits where
/// there is no file yet.
pub fn expand(scope: &Scope, files: &Files, answer: &str) -> Result<String, String> {
    let parts = segments(answer);
    let edit = |s: &Segment| matches!(s, Segment::Code { info, .. } if is_edit(info));
    if !parts.iter().any(edit)
        || parts
            .iter()
            .any(|s| matches!(s, Segment::Code { closed: false, .. }))
    {
        return Ok(answer.to_string());
    }
    let whole = parts.iter().any(|s| {
        matches!(s, Segment::Code { info, body, .. } if !is_edit(info) && !is_scenario(info) && !body.trim().is_empty())
    });
    if whole {
        return Err(
            "The answer has ```st edit blocks and a ```st block with code: send one form or the other, never both.".into(),
        );
    }
    let Some(old) = scope.file().and_then(|f| files.get(f)) else {
        return Err(
            "There is no file to edit yet: send the whole program in one block marked ```st, not ```st edit blocks.".into(),
        );
    };
    let mut edits = Vec::new();
    for s in &parts {
        if let Segment::Code { info, body, .. } = s
            && is_edit(info)
        {
            let k = edits.len() + 1;
            edits.push(parse(body).map_err(|why| format!("Edit block {k}: {why}."))?);
        }
    }
    if edits.len() > MAX_EDITS {
        return Err(format!(
            "The answer has {} edit blocks; with more than {MAX_EDITS}, send the whole file in one block marked ```st.",
            edits.len()
        ));
    }
    let file = apply(old, &edits)?;
    let mut out: Vec<String> = Vec::new();
    let mut placed = false;
    for s in parts {
        match s {
            Segment::Prose(p) => out.push(p),
            Segment::Code { info, .. } if is_edit(&info) => {
                if !placed {
                    out.push(format!("```st\n{}```", file));
                    placed = true;
                }
            }
            Segment::Code { info, body, .. } => out.push(format!("```{info}\n{body}\n```")),
        }
    }
    Ok(out.join("\n"))
}
