//! A start state in words (Nick, 2026-11-16: "the user should be able to
//! rapidly configure the state of the machine for various scenarios
//! manually and with the agent before starting").
//!
//! The engineer describes how the machine stands ("part seated, guard
//! open, e-stop healthy, speed 40"); a small fast model (the relay's
//! `start` agent) answers with input values; the page shows them as a
//! proposal and sets nothing until the engineer accepts. This module is
//! the words and the checks, with no UI: [`message`] is what goes out,
//! [`parse`] is what comes back.
//!
//! The answer is lines of `NAME = value`, nothing else (one fenced block
//! around them is allowed). It is refused whole when anything in it
//! is not an input of the program, not a value that input can hold
//! (the rules of `Session::try_input`, so a proposal can always be set),
//! a name twice, or not a line of that form: a half-read answer would
//! set a machine to a state nobody asked for.

use std::fmt::Write as _;

use helmsman_iec::{Ty, Value};

use crate::playground::{Place, Point};
use crate::scope::Files;

/// The relay's agent that proposes a start state.
pub const AGENT: &str = "start";
/// Most characters of a description that go out.
pub const MAX_DESCRIPTION: usize = 1000;
/// Most lines an answer may have.
pub const MAX_LINES: usize = 64;
/// Longest answer read, in bytes.
pub const MAX_ANSWER: usize = 8 * 1024;

/// The envelope tags of the message (also in [`crate::scope::ENVELOPES`],
/// so a closing tag inside either is defanged).
const DESCRIPTION_TAG: &str = "start_description";
const INPUTS_TAG: &str = "program_inputs";
const OUTPUTS_TAG: &str = "program_outputs";

/// What the page asks beside the words (the relay's `start-v1` prompt
/// knows only `NAME = value` lines, and a served profile never changes:
/// a `start-v2` that asks for names itself would make this reliable).
const NAMES_RULE: &str = "If the description calls an input or output by another name than the \
program's, you may end the answer with at most two lines `Offer: rename <old> <new>`, <old> a name \
listed above and <new> letters, digits and underscores. Everything else stays `NAME = value` lines.\n";

/// What one input accepts, as the message says it and [`parse`] holds it.
fn accepts(ty: Ty) -> String {
    match ty {
        Ty::Bool => "TRUE or FALSE".into(),
        Ty::Time => "whole milliseconds".into(),
        t if t.is_real() => "a finite number".into(),
        t => {
            let (lo, hi) = t.range();
            format!("a whole number from {lo} to {hi}")
        }
    }
}

/// An input's value as the message and the answer write it: TIME in
/// milliseconds, the rest as the panel shows it.
#[must_use]
pub fn plain(ty: Ty, v: Value) -> String {
    match (ty, v) {
        (Ty::Time, Value::Int(ns)) => (ns / 1_000_000).to_string(),
        _ => v.to_string(),
    }
}

/// The message that asks for a start state: the engineer's words and the
/// program's inputs, each in a tag. The words are data (the page says so
/// to the model's prompt), and a closing tag in either is defanged.
#[must_use]
pub fn message(description: &str, inputs: &[Point]) -> String {
    let words: String = description.trim().chars().take(MAX_DESCRIPTION).collect();
    let mut list = String::new();
    for p in inputs {
        let _ = writeln!(
            list,
            "{} | {} | {} | now {}",
            p.name,
            p.ty,
            accepts(p.ty),
            plain(p.ty, p.value)
        );
    }
    format!(
        "<{DESCRIPTION_TAG}>\n{}\n</{DESCRIPTION_TAG}>\n<{INPUTS_TAG}>\n{}</{INPUTS_TAG}>\n",
        crate::claude::neutralize(&words),
        crate::claude::neutralize(&list),
    )
}

/// [`message`] for the I/O tab: also the program's outputs (so a name
/// can be proposed for them) and the request for `Offer: rename` lines.
#[must_use]
pub fn message_for(description: &str, inputs: &[Point], outputs: &[Point]) -> String {
    let mut list = String::new();
    for p in outputs {
        let _ = writeln!(list, "{} | {}", p.name, p.ty);
    }
    format!(
        "{}<{OUTPUTS_TAG}>\n{}</{OUTPUTS_TAG}>\n{NAMES_RULE}",
        message(description, inputs),
        crate::claude::neutralize(&list),
    )
}

/// What `text` says, flattened to one short line for a reason.
fn quoted(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 40 {
        let cut: String = line.chars().take(40).collect();
        format!("\"{cut}...\"")
    } else {
        format!("\"{line}\"")
    }
}

/// `text` as a value of `p`'s type, by the rules of `Session::try_input`.
#[allow(clippy::cast_possible_truncation)]
fn value(p: &Point, text: &str) -> Result<Value, String> {
    let bad = |what: &str| format!("{}: {} is {what}.", p.name, quoted(text));
    if matches!(p.place, Place::Bit(..)) && p.ty != Ty::Bool {
        return Err(format!("{} cannot be set to a value.", p.name));
    }
    match p.ty {
        Ty::Bool => match text.to_ascii_uppercase().as_str() {
            "TRUE" => Ok(Value::Bool(true)),
            "FALSE" => Ok(Value::Bool(false)),
            _ => Err(bad("not TRUE or FALSE")),
        },
        Ty::Time => {
            let ms: i128 = text
                .parse()
                .map_err(|_| bad("not a whole number of milliseconds"))?;
            let ns = ms
                .checked_mul(1_000_000)
                .filter(|n| (p.ty.range().0..=p.ty.range().1).contains(n))
                .ok_or_else(|| bad("out of range for a time"))?;
            Ok(Value::Int(ns))
        }
        t if t.is_real() => {
            let x: f64 = text.parse().map_err(|_| bad("not a number"))?;
            if !x.is_finite() {
                return Err(bad("not a finite number"));
            }
            let held = if t == Ty::Real { f64::from(x as f32) } else { x };
            if held.is_finite() {
                Ok(Value::Real(held))
            } else {
                Err(bad("out of range"))
            }
        }
        t => {
            let n: i128 = text.parse().map_err(|_| bad("not a whole number"))?;
            let (lo, hi) = t.range();
            if (lo..=hi).contains(&n) {
                Ok(Value::Int(n))
            } else {
                Err(format!("{}: {n} is outside {lo} to {hi}.", p.name))
            }
        }
    }
}

/// The lines inside the one fenced block an answer may be in, or the
/// answer's own lines.
fn body(answer: &str) -> Result<Vec<&str>, String> {
    let lines: Vec<&str> = answer.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let fenced = lines.first().is_some_and(|l| l.starts_with("```"));
    if !fenced {
        return Ok(lines);
    }
    match lines.as_slice() {
        [_, inner @ .., last] if *last == "```" && inner.iter().all(|l| !l.contains("```")) => {
            Ok(inner.to_vec())
        }
        _ => Err("The answer has text outside its list of inputs.".into()),
    }
}

/// The input values the `start` agent answered with, by the program's
/// spelling of each name, in the order given. `Ok(vec![])` when it
/// answered with nothing (an empty answer or `NONE`): nothing to set.
///
/// # Errors
/// A plain reason, when the answer is not lines of `name = value` that
/// the program's inputs can all take: prose, a name that is not an
/// input, a name twice (in any case), a value of the wrong kind or out of
/// range, more than [`MAX_LINES`] lines or more than [`MAX_ANSWER`] bytes.
/// Nothing of a refused answer is returned.
pub fn parse(answer: &str, inputs: &[Point]) -> Result<Vec<(String, Value)>, String> {
    if answer.len() > MAX_ANSWER {
        return Err("The answer is too long to be a list of inputs.".into());
    }
    // A bare CR is a line end here too.
    let answer = answer.replace("\r\n", "\n").replace('\r', "\n");
    let lines = body(&answer)?;
    if lines.len() == 1 && lines[0].eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    if lines.len() > MAX_LINES {
        return Err(format!("The answer has more than {MAX_LINES} lines."));
    }
    let mut out: Vec<(String, Value)> = Vec::new();
    for line in lines {
        let Some((name, text)) = line.split_once('=') else {
            return Err(format!(
                "The answer is not a list of \"name = value\" lines: {}.",
                quoted(line)
            ));
        };
        let (name, text) = (name.trim(), text.trim());
        if name.is_empty() || text.is_empty() {
            return Err(format!(
                "A line has no {}: {}.",
                if name.is_empty() { "name" } else { "value" },
                quoted(line)
            ));
        }
        let Some(p) = inputs.iter().find(|p| p.name.eq_ignore_ascii_case(name)) else {
            return Err(format!("{} is not an input of this program.", quoted(name)));
        };
        if out.iter().any(|(n, _)| *n == p.name) {
            return Err(format!("{} is given twice.", p.name));
        }
        out.push((p.name.clone(), value(p, text)?));
    }
    Ok(out)
}

/// One input a proposal would move.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub name: String,
    pub ty: Ty,
    pub old: Value,
    pub new: Value,
}

/// What the Agent proposed, and the program text it was made for.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub program: String,
    /// Only the inputs whose value differs from what they have.
    pub changes: Vec<Change>,
    /// Renames the answer proposed that the page can carry out, each
    /// done on a click ([`crate::offers`]).
    pub offers: Vec<crate::offers::Offer>,
}

/// Where a request for a start state stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    /// A request is out.
    Working,
    Proposed(Proposal),
}

/// The field, its request and its proposal (the I/O tab's).
#[derive(Debug, Default)]
pub struct Card {
    /// The engineer's words.
    pub draft: String,
    pub phase: Phase,
    /// One quiet line under the field: a refusal, what was set, why not.
    pub line: Option<String>,
}

impl Card {
    /// The request went out.
    pub fn asked(&mut self) {
        self.phase = Phase::Working;
        self.line = None;
    }

    /// The request is stopped or dropped; nothing proposed, nothing said.
    pub fn stopped(&mut self) {
        self.phase = Phase::Idle;
        self.line = None;
    }

    /// The Agent's answer (or why there was none) for the program text
    /// `program`, whose inputs and outputs are as they are now, in the
    /// project `files`. The answer may also end in `Offer: rename`
    /// lines, kept when the name is an input or output and the project
    /// can carry the rename out. An answer that is refused proposes
    /// neither values nor names.
    pub fn answered(
        &mut self,
        answer: Result<&str, &str>,
        program: &str,
        inputs: &[Point],
        outputs: &[Point],
        files: &Files,
    ) {
        self.phase = Phase::Idle;
        let text = match answer {
            Ok(text) => text,
            Err(why) => {
                self.line = Some(format!("No answer: {}.", quoted_plain(why, 80)));
                return;
            }
        };
        let (values, whats) = crate::offers::split(text);
        let is_point = |n: &str| {
            inputs
                .iter()
                .chain(outputs)
                .any(|p| p.name.eq_ignore_ascii_case(n))
        };
        let whats: Vec<crate::offers::What> = whats
            .into_iter()
            .filter(|w| matches!(w, crate::offers::What::Rename { old, .. } if is_point(old)))
            .collect();
        match parse(&values, inputs) {
            Err(why) => {
                self.line = Some(format!(
                    "The answer cannot be used, so nothing is proposed. {why}"
                ));
            }
            Ok(set) => {
                let offers = crate::offers::kept(whats, files);
                let said_none = set.is_empty();
                let changes: Vec<Change> = set
                    .into_iter()
                    .filter_map(|(name, new)| {
                        let p = inputs.iter().find(|p| p.name == name)?;
                        (p.value != new).then_some(Change {
                            name,
                            ty: p.ty,
                            old: p.value,
                            new,
                        })
                    })
                    .collect();
                if changes.is_empty() && offers.is_empty() {
                    self.line = Some(if said_none {
                        "Nothing in that description names an input of this program.".into()
                    } else {
                        "The inputs already have the values it describes.".into()
                    });
                } else {
                    self.line = None;
                    self.phase = Phase::Proposed(Proposal {
                        program: program.to_owned(),
                        changes,
                        offers,
                    });
                }
            }
        }
    }

    /// Rename `k` of the proposal was carried out: `program` is the text
    /// after it. The proposal follows the new name (a value proposed for
    /// the old one is now for the new one) and the offer goes.
    pub fn renamed(&mut self, k: usize, program: &str) {
        let Phase::Proposed(p) = &mut self.phase else {
            return;
        };
        let Some(crate::offers::What::Rename { old, new }) = p.offers.get(k).map(|o| o.what.clone()) else {
            return;
        };
        p.offers.remove(k);
        p.offers.retain(
            |o| !matches!(&o.what, crate::offers::What::Rename { old: o2, .. } if o2.eq_ignore_ascii_case(&old)),
        );
        for c in &mut p.changes {
            if c.name.eq_ignore_ascii_case(&old) {
                c.name.clone_from(&new);
            }
        }
        program.clone_into(&mut p.program);
        if p.changes.is_empty() && p.offers.is_empty() {
            self.phase = Phase::Idle;
        }
        self.line = Some(format!("Renamed {old} to {new}."));
    }

    /// The proposal's input names spelled as the program now spells them
    /// (`names`), after a rename.
    pub fn respell(&mut self, names: &[String]) {
        let Phase::Proposed(p) = &mut self.phase else {
            return;
        };
        for c in &mut p.changes {
            if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(&c.name)) {
                c.name.clone_from(n);
            }
        }
    }

    /// The proposal, once it may be set: `program` is the text it was made
    /// for and nothing is running. Otherwise why not, in a line.
    ///
    /// # Errors
    /// There is no proposal, the program changed, or it is running.
    pub fn accept(&self, running: bool, program: &str) -> Result<&Proposal, String> {
        let Phase::Proposed(p) = &self.phase else {
            return Err("There is nothing proposed.".into());
        };
        if p.program != program {
            return Err("The program changed after this was proposed, so nothing was set.".into());
        }
        if running {
            return Err("The program is running, so nothing was set. Stop it first.".into());
        }
        Ok(p)
    }
}

/// `text` as one line of at most `n` characters, for a reason.
fn quoted_plain(text: &str, n: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = line.trim_end_matches('.');
    if line.chars().count() > n {
        format!("{}...", line.chars().take(n).collect::<String>())
    } else {
        line.to_owned()
    }
}
