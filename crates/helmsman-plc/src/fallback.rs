//! The offline chat: with the relay unreachable there is no model, so a
//! chat message is read by rules, and the only thing it can become is one
//! param edit, checked here against the running manifests' types, ranges
//! and units. The edit is not applied here or anywhere on this path: the
//! chat route posts it as a param proposal (`POST /api/proposals`), which
//! the approval policy then handles like any other.
//!
//! The grammar, one command per message, on one line:
//!
//! ```text
//! set REF to VALUE        set hopper threshold to 80
//! set REF = VALUE         set conveyor.speed = 2.5 m/s
//! REF = VALUE             fill_time = 5 s
//!
//! REF    PARAM | MODULE PARAM | MODULE.PARAM, each a run of words; a
//!        param's words are joined by `_` (`fill time` is `fill_time`)
//! VALUE  NUMBER [UNIT] | on | off | true | false | enable(d) | disable(d)
//! NUMBER [+-]DIGITS[.DIGITS]   (no exponent, hex, inf or NaN)
//! ```
//!
//! - **Exact names only.** Names match exactly, ASCII case aside. A ref
//!   that reads as more than one param (the same name in two modules, or
//!   `hopper threshold` when both `hopper.threshold` and a
//!   `hopper_threshold` exist) is refused with the candidates: the box
//!   asks, it never picks. Near misses (edit distance, prefixes) are only
//!   ever suggested.
//! - **Typed.** An `int` takes a whole number (`5.5` and `5.0` are
//!   refused), a `real` any number, a `bool` only the words above. The
//!   value must be within the manifest's range and differ from the
//!   current one.
//! - **Units are checked, never converted.** A unit may follow the number
//!   (`80 %`, `80%`, `5 s`, `5 seconds`, `2.5 bar`); it must be the
//!   param's own (`unit=` in the manifest, a few spellings of `%`, `s`,
//!   `ms`, `min`, `h` and `bar` accepted, otherwise case-insensitively
//!   equal). A number without a unit is in the param's unit. A unit on a
//!   param that declares none is refused.
//! - **Anything else is refused whole**, with the reason and the list of
//!   editable params: more than [`MAX_COMMAND`] bytes, more than one
//!   line, any character outside ASCII letters, digits, blanks and
//!   `_ . - + % = /`, or any trailing word. There is no partial parse.

use std::fmt::{self, Write as _};

use helmsman_io::{Ty, Value};

use crate::manifest::{Manifest, Param};

/// Longest command read, in bytes (after trimming blanks).
pub const MAX_COMMAND: usize = 200;
/// Most params listed in the help text.
pub const MAX_LISTED: usize = 30;
/// Most suggestions for an unknown name.
pub const MAX_SUGGESTIONS: usize = 3;
/// Longest number literal.
const MAX_NUMBER: usize = 40;
/// Most words in a ref.
const MAX_REF_WORDS: usize = 6;

/// One param a chat command may name.
#[derive(Debug, Clone, PartialEq)]
pub struct Editable {
    pub module: String,
    pub param: Param,
}

/// The params of `modules` (name, manifest), in order.
pub fn editable<'a>(modules: impl IntoIterator<Item = (&'a str, &'a Manifest)>) -> Vec<Editable> {
    modules
        .into_iter()
        .flat_map(|(module, m)| {
            m.params.iter().map(move |p| Editable {
                module: module.to_string(),
                param: p.clone(),
            })
        })
        .collect()
}

/// One param's new value: what a param proposal carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub module: String,
    pub param: String,
    pub value: Value,
}

/// Why a command was not turned into an edit.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// Too long, more than one line, or characters outside the grammar.
    Input(&'static str),
    /// The words do not fit the grammar.
    NotACommand,
    /// No param is called that; near names, if any.
    Unknown {
        name: String,
        suggestions: Vec<String>,
    },
    /// More than one param is called that (`MODULE PARAM` each).
    Ambiguous {
        name: String,
        candidates: Vec<String>,
    },
    WrongType {
        target: String,
        expected: Ty,
    },
    UnitMismatch {
        target: String,
        expected: Option<String>,
        given: String,
    },
    OutOfRange {
        target: String,
        min: Value,
        max: Value,
        unit: Option<String>,
    },
    Unchanged {
        target: String,
        value: Value,
    },
}

impl Refusal {
    /// A short machine word for the chat panel.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Input(_) => "input",
            Self::NotACommand => "not_a_command",
            Self::Unknown { .. } => "unknown_param",
            Self::Ambiguous { .. } => "ambiguous_param",
            Self::WrongType { .. } => "wrong_type",
            Self::UnitMismatch { .. } => "unit_mismatch",
            Self::OutOfRange { .. } => "out_of_range",
            Self::Unchanged { .. } => "unchanged",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(why) => f.write_str(why),
            Self::NotACommand => {
                f.write_str("offline, chat only takes one param change, like \"set conveyor speed to 2.5\"")
            }
            Self::Unknown { name, suggestions } => {
                write!(f, "no param called {name}")?;
                if suggestions.is_empty() {
                    Ok(())
                } else {
                    write!(f, "; did you mean {}?", suggestions.join(", or "))
                }
            }
            Self::Ambiguous { name, candidates } => write!(
                f,
                "{name} could be {}; say which, e.g. \"set {} to ...\"",
                candidates.join(" or "),
                candidates.first().map_or("", String::as_str)
            ),
            Self::WrongType { target, expected } => write!(
                f,
                "{target} is {}",
                match expected {
                    Ty::Bool => "on or off",
                    Ty::Int => "a whole number",
                    Ty::Real => "a number",
                }
            ),
            Self::UnitMismatch {
                target,
                expected: Some(u),
                given,
            } => write!(
                f,
                "{target} is in {u}, not {given} (no conversion: give it in {u})"
            ),
            Self::UnitMismatch {
                target,
                expected: None,
                given,
            } => write!(f, "{target} has no unit; give a plain number, not {given}"),
            Self::OutOfRange {
                target,
                min,
                max,
                unit,
            } => write!(
                f,
                "{target} must be within {}",
                range(*min, *max, unit.as_deref())
            ),
            Self::Unchanged { target, value } => {
                write!(f, "{target} is already {}", show(*value))
            }
        }
    }
}

/// What a command comes to.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Edit(Edit),
    Refused(Refusal),
}

/// A value as the help text and refusals show it.
fn show(v: Value) -> String {
    match v {
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        Value::Int(n) => n.to_string(),
        Value::Real(x) => x.to_string(),
    }
}

fn range(min: Value, max: Value, unit: Option<&str>) -> String {
    let mut s = format!("{}..={}", show(min), show(max));
    if let Some(u) = unit {
        s.push(' ');
        s.push_str(u);
    }
    s
}

/// The editable params, one per line, at most [`MAX_LISTED`].
#[must_use]
pub fn help(params: &[Editable]) -> String {
    if params.is_empty() {
        return "This box has no params to change.".into();
    }
    let mut s = String::from(
        "Offline: the agent cannot be reached, so chat takes one param change per message, \
         like \"set MODULE PARAM to VALUE\" or \"PARAM = VALUE\". Editable params:\n",
    );
    for e in params.iter().take(MAX_LISTED) {
        let p = &e.param;
        let what = match (p.ty(), p.range) {
            (Ty::Bool, _) | (_, None) => "bool on/off".to_string(),
            (Ty::Int, Some((lo, hi))) => format!("int {}", range(lo, hi, p.unit.as_deref())),
            (Ty::Real, Some((lo, hi))) => format!("real {}", range(lo, hi, p.unit.as_deref())),
        };
        let _ = writeln!(s, "  {} {}: {what} (now {})", e.module, p.name, show(p.value));
    }
    if params.len() > MAX_LISTED {
        let _ = writeln!(s, "  ... and {} more", params.len() - MAX_LISTED);
    }
    s
}

// ------------------------------------------------------------------ lexing

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '\t' | '_' | '.' | '-' | '+' | '%' | '=' | '/')
}

/// Splits on blanks, with `=` always a word of its own.
fn words(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for w in s.split_ascii_whitespace() {
        let mut rest = w;
        while let Some(i) = rest.find('=') {
            if i > 0 {
                out.push(&rest[..i]);
            }
            out.push("=");
            rest = &rest[i + 1..];
        }
        if !rest.is_empty() {
            out.push(rest);
        }
    }
    out
}

fn keyword(w: &str, k: &str) -> bool {
    w.eq_ignore_ascii_case(k)
}

/// A typed value before it meets its param.
#[derive(Debug, Clone, PartialEq)]
enum Given {
    Bool(bool),
    /// The literal as written (validated), and whether it has a fraction.
    Number {
        text: String,
        fraction: bool,
        unit: Option<String>,
    },
}

fn boolean(w: &str) -> Option<bool> {
    let w = w.to_ascii_lowercase();
    match w.as_str() {
        "on" | "true" | "enable" | "enabled" => Some(true),
        "off" | "false" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

fn unit_word(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 16
        && u.bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'%' || b == b'/')
}

/// `[+-]DIGITS[.DIGITS]` then, glued, an optional unit.
fn number(word: &str) -> Option<(String, bool, Option<String>)> {
    let bytes = word.as_bytes();
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .position(|c| !c.is_ascii_digit())
            .map_or(bytes.len(), |n| from + n)
    };
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut end = digits(sign);
    if end == sign {
        return None;
    }
    let fraction = bytes.get(end) == Some(&b'.');
    if fraction {
        let after = digits(end + 1);
        if after == end + 1 {
            return None;
        }
        end = after;
    }
    if end > MAX_NUMBER {
        return None;
    }
    let (w, i) = (word, end);
    let unit = &w[i..];
    if !unit.is_empty() && !unit_word(unit) {
        return None;
    }
    Some((
        w[..i].to_string(),
        fraction,
        (!unit.is_empty()).then(|| unit.to_string()),
    ))
}

/// The value at the end of a command: one word, or a number and a unit.
fn value(ws: &[&str]) -> Option<Given> {
    match ws {
        [w] => boolean(w)
            .map(Given::Bool)
            .or_else(|| number(w).map(|(text, fraction, unit)| Given::Number { text, fraction, unit })),
        [n, u] => {
            let (text, fraction, glued) = number(n)?;
            (glued.is_none() && unit_word(u)).then(|| Given::Number {
                text,
                fraction,
                unit: Some((*u).to_string()),
            })
        }
        _ => None,
    }
}

/// A ref: the module, if named, and the param's words.
#[derive(Debug)]
enum Ref {
    /// `MODULE.PARAM`: exactly that.
    Dotted(String, String),
    /// Words: any module's `w1_w2_..`, or module `w1`'s `w2_..`.
    Words(Vec<String>),
}

fn ident(w: &str) -> bool {
    let b = w.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn reference(ws: &[&str]) -> Option<Ref> {
    if ws.is_empty() || ws.len() > MAX_REF_WORDS {
        return None;
    }
    if ws
        .iter()
        .any(|w| ["set", "to", "="].iter().any(|k| keyword(w, k)))
    {
        return None;
    }
    if let [w] = ws
        && let Some((m, p)) = w.split_once('.')
    {
        return (ident(m) && ident(p)).then(|| Ref::Dotted(m.into(), p.into()));
    }
    ws.iter()
        .all(|w| ident(w))
        .then(|| Ref::Words(ws.iter().map(|w| (*w).to_string()).collect()))
}

/// The command's ref and value, if the words fit exactly one way.
fn grammar(ws: &[&str]) -> Option<(Ref, Given)> {
    let (body, seps): (&[&str], &[&str]) = match ws.first() {
        Some(w) if keyword(w, "set") => (&ws[1..], &["to", "="]),
        _ => (ws, &["="]),
    };
    let mut found = None;
    for k in [1, 2] {
        if body.len() < k + 2 {
            continue;
        }
        let (head, tail) = body.split_at(body.len() - k);
        let (sep, refw) = head.split_last()?;
        if !seps.iter().any(|s| keyword(sep, s)) {
            continue;
        }
        let (Some(r), Some(v)) = (reference(refw), value(tail)) else {
            continue;
        };
        if found.is_some() {
            return None;
        }
        found = Some((r, v));
    }
    found
}

// ---------------------------------------------------------------- matching

fn label(e: &Editable) -> String {
    format!("{} {}", e.module, e.param.name)
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Every param the ref names exactly, by index.
fn resolve(r: &Ref, params: &[Editable]) -> Vec<usize> {
    let hit = |i: &usize| {
        let e = &params[*i];
        match r {
            Ref::Dotted(m, p) => eq(&e.module, m) && eq(&e.param.name, p),
            Ref::Words(w) => {
                eq(&e.param.name, &w.join("_"))
                    || (w.len() > 1 && eq(&e.module, &w[0]) && eq(&e.param.name, &w[1..].join("_")))
            }
        }
    };
    (0..params.len()).filter(hit).collect()
}

fn ref_name(r: &Ref) -> String {
    match r {
        Ref::Dotted(m, p) => format!("{m}.{p}"),
        Ref::Words(w) => w.join(" "),
    }
}

/// Levenshtein distance over ASCII-lowercased bytes.
fn distance(a: &str, b: &str) -> usize {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, &x) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &y) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if x == y {
                prev
            } else {
                1 + prev.min(cur).min(row[j])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

/// Near names for a ref nothing matched: never acted on.
fn suggest(r: &Ref, params: &[Editable]) -> Vec<String> {
    let (module, wanted): (Option<&str>, Vec<String>) = match r {
        Ref::Dotted(m, p) => (Some(m), vec![p.clone()]),
        Ref::Words(w) => (
            None,
            std::iter::once(w.join("_"))
                .chain((w.len() > 1).then(|| w[1..].join("_")))
                .collect(),
        ),
    };
    let mut scored: Vec<(usize, String)> = params
        .iter()
        .filter(|e| module.is_none_or(|m| eq(&e.module, m) || distance(&e.module, m) <= 2))
        .filter_map(|e| {
            let name = &e.param.name;
            let best = wanted
                .iter()
                .filter_map(|q| {
                    let d = distance(q, name);
                    let near = d <= (q.len().max(name.len()) / 4).max(1);
                    let lq = q.to_ascii_lowercase();
                    let ln = name.to_ascii_lowercase();
                    let part = lq.len() >= 3 && (ln.starts_with(&lq) || lq.starts_with(&ln));
                    (near || part).then_some(d)
                })
                .min()?;
            Some((best, label(e)))
        })
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(MAX_SUGGESTIONS).map(|(_, s)| s).collect()
}

/// The canonical spelling of a unit, for comparison only.
fn canonical(u: &str) -> String {
    let l = u.to_ascii_lowercase();
    let c = match l.as_str() {
        "%" | "percent" | "pct" => "%",
        "s" | "sec" | "secs" | "second" | "seconds" => "s",
        "ms" | "msec" | "millisecond" | "milliseconds" => "ms",
        "min" | "mins" | "minute" | "minutes" => "min",
        "h" | "hr" | "hrs" | "hour" | "hours" => "h",
        "bar" | "bars" => "bar",
        _ => return l,
    };
    c.to_string()
}

/// The value `given` is for `p`, or why not.
fn typed(target: &str, p: &Param, given: &Given) -> Result<Value, Refusal> {
    let wrong = || Refusal::WrongType {
        target: target.to_string(),
        expected: p.ty(),
    };
    let (text, fraction, unit) = match (p.ty(), given) {
        (Ty::Bool, Given::Bool(b)) => return Ok(Value::Bool(*b)),
        (Ty::Bool, _) | (Ty::Int | Ty::Real, Given::Bool(_)) => return Err(wrong()),
        (_, Given::Number { text, fraction, unit }) => (text, *fraction, unit),
    };
    if p.ty() == Ty::Int && fraction {
        return Err(wrong());
    }
    if let Some(u) = unit {
        let ok = p
            .unit
            .as_deref()
            .is_some_and(|want| canonical(want) == canonical(u));
        if !ok {
            return Err(Refusal::UnitMismatch {
                target: target.to_string(),
                expected: p.unit.clone(),
                given: u.clone(),
            });
        }
    }
    let out_of_range = || match p.range {
        Some((min, max)) => Refusal::OutOfRange {
            target: target.to_string(),
            min,
            max,
            unit: p.unit.clone(),
        },
        None => wrong(),
    };
    let v = if p.ty() == Ty::Int {
        Value::Int(text.parse::<i64>().map_err(|_| out_of_range())?)
    } else {
        let x: f64 = text.parse().map_err(|_| wrong())?;
        if !x.is_finite() {
            return Err(out_of_range());
        }
        Value::Real(x)
    };
    if !p.admits(v) {
        return Err(out_of_range());
    }
    Ok(v)
}

/// Reads one chat command against the box's editable params.
#[must_use]
pub fn parse(command: &str, params: &[Editable]) -> Reply {
    match read(command, params) {
        Ok(e) => Reply::Edit(e),
        Err(r) => Reply::Refused(r),
    }
}

fn read(command: &str, params: &[Editable]) -> Result<Edit, Refusal> {
    let s = command.trim_matches(|c: char| c.is_ascii_whitespace());
    if s.len() > MAX_COMMAND {
        return Err(Refusal::Input("too long for a param change"));
    }
    if s.contains(['\n', '\r']) {
        return Err(Refusal::Input("one param change per message, on one line"));
    }
    if !s.chars().all(allowed) {
        return Err(Refusal::Input(
            "offline, chat only reads plain ASCII letters, digits, blanks and _ . - + % = /",
        ));
    }
    let ws = words(s);
    let (r, given) = grammar(&ws).ok_or(Refusal::NotACommand)?;
    let hits = resolve(&r, params);
    let at = match hits.as_slice() {
        [i] => *i,
        [] => {
            return Err(Refusal::Unknown {
                name: ref_name(&r),
                suggestions: suggest(&r, params),
            });
        }
        many => {
            let mut candidates: Vec<String> = many.iter().map(|&i| label(&params[i])).collect();
            candidates.sort();
            return Err(Refusal::Ambiguous {
                name: ref_name(&r),
                candidates,
            });
        }
    };
    let e = &params[at];
    let target = label(e);
    let v = typed(&target, &e.param, &given)?;
    if v == e.param.value {
        return Err(Refusal::Unchanged { target, value: v });
    }
    Ok(Edit {
        module: e.module.clone(),
        param: e.param.name.clone(),
        value: v,
    })
}
