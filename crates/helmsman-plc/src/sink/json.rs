//! The event body. Written by hand: one flat object of strings, numbers,
//! booleans and null.

use std::fmt::Write as _;

use helmsman_io::Value;

/// `s` as a JSON string, quotes included.
#[must_use]
pub fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A signal value as JSON. Reals are finite (the signal table refuses
/// anything else); one that is not anyway becomes `null`. Rust's `Debug`
/// for `f64` is the shortest round-tripping form (`0.1`, `1.0`, `1e300`),
/// all of which are JSON numbers.
#[must_use]
pub fn value(v: Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Real(x) if x.is_finite() => format!("{x:?}"),
        Value::Real(_) => "null".into(),
    }
}
