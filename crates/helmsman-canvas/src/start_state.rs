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

use helmsman_iec::{Ty, Value};

use crate::playground::{Place, Point};

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
        list.push_str(&format!(
            "{} | {} | {} | now {}\n",
            p.name,
            p.ty,
            accepts(p.ty),
            plain(p.ty, p.value)
        ));
    }
    format!(
        "<{DESCRIPTION_TAG}>\n{}\n</{DESCRIPTION_TAG}>\n<{INPUTS_TAG}>\n{}</{INPUTS_TAG}>\n",
        crate::claude::neutralize(&words),
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
