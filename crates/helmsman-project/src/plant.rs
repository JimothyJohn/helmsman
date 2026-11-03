//! Plant-model state variables: the quantities a simulation keeps for the
//! machine it stands in for (the well level, the belt position, the tank
//! temperature), marked in the Structured Text with a `{state}` pragma so
//! the Devices tab can list them, say which sensor measures each on the
//! real machine and suggest one where none does (TODO.md
//! `sim-feedback-devices`).
//!
//! ```text
//! VAR
//!   level_m : REAL := 1.5;  {state unit=m}
//!   {state unit=degC measured_by=temp_in}
//!   temp_c : REAL := 20.0;
//! END_VAR
//! ```
//!
//! The pragma is IEC 61131-3's `{ }`: the compiler skips it (so does the
//! ruSTy oracle), which is why it is the hook and not a comment. It goes
//! on the declaration's line or on the line before it. Attributes:
//! `unit=U` (the engineering unit; a declaration comment is never read)
//! and `measured_by=NAME` (the `in` signal of the I/O map that stands for
//! this quantity on the real machine; without it the signal is found by
//! name, see [`measurement_names`]). This is a skim of the text, like
//! [`crate::iomap`]: no validation, and anything inside a comment or a
//! string literal is not a pragma.

/// One state variable of a plant model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateVar {
    pub name: String,
    /// The declared type, as written (`REAL`, `INT`, `BOOL`, ...).
    pub ty: String,
    pub unit: Option<String>,
    /// The `measured_by=` attribute, if given.
    pub measured_by: Option<String>,
    /// The POU (`PROGRAM`, `FUNCTION_BLOCK`, `FUNCTION`) the variable
    /// belongs to, as written; empty outside any.
    pub pou: String,
    /// 1-based line of the declaration.
    pub line: usize,
}

/// `text` with every comment (`(* *)`, `/* */`, `//`) and string literal
/// (`'...'`, `"..."`) blanked to spaces, line breaks kept, so a pragma in
/// prose or in a string is not a pragma.
fn blanked(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut k = 0;
    let blank = |out: &mut Vec<u8>, bytes: &[u8]| {
        out.extend(bytes.iter().map(|&c| if c == b'\n' { b'\n' } else { b' ' }));
    };
    while k < b.len() {
        let rest = &b[k..];
        let end = if rest.starts_with(b"(*") {
            Some(find(rest, 2, b"*)").map_or(rest.len(), |e| e + 2))
        } else if rest.starts_with(b"/*") {
            Some(find(rest, 2, b"*/").map_or(rest.len(), |e| e + 2))
        } else if rest.starts_with(b"//") {
            Some(rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len()))
        } else if rest[0] == b'\'' || rest[0] == b'"' {
            let q = rest[0];
            Some(find(rest, 1, &[q]).map_or(rest.len(), |e| e + 1))
        } else {
            None
        };
        match end {
            Some(e) => {
                blank(&mut out, &rest[..e]);
                k += e;
            }
            None => {
                out.push(rest[0]);
                k += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// The index of `pat` in `hay` from `from` on, or `None`.
fn find(hay: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    (from..hay.len().saturating_sub(pat.len() - 1)).find(|&i| hay[i..].starts_with(pat))
}

/// An identifier: letters, digits and `_`, not starting with a digit.
fn ident(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let n = s
        .char_indices()
        .take_while(|&(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
        .count();
    (n > 0).then(|| &s[..n])
}

/// `NAME : TYPE` at the start of a declaration line.
fn declaration(line: &str) -> Option<(String, String)> {
    let name = ident(line)?;
    let after = line.trim_start()[name.len()..].trim_start();
    let after = after.strip_prefix(':')?;
    if after.starts_with('=') {
        return None;
    }
    let ty = ident(after)?;
    if ty.eq_ignore_ascii_case("AT") {
        return None;
    }
    Some((name.to_string(), ty.to_ascii_uppercase()))
}

/// The `{state ...}` pragma on a (blanked) line, as its attributes.
fn pragma(line: &str) -> Option<Vec<(String, String)>> {
    let open = line.find('{')?;
    let close = line[open..].find('}')? + open;
    let body = line[open + 1..close].trim();
    let mut words = body.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("state") {
        return None;
    }
    Some(
        words
            .filter_map(|w| w.split_once('='))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect(),
    )
}

/// The POU a header line opens, if it is one.
fn pou_header(line: &str) -> Option<String> {
    let mut words = line.split_whitespace();
    let kw = words.next()?.to_ascii_uppercase();
    if matches!(kw.as_str(), "PROGRAM" | "FUNCTION_BLOCK" | "FUNCTION") {
        ident(words.next()?).map(str::to_string)
    } else {
        None
    }
}

/// Every `{state}`-marked declaration in one file's text, in order.
#[must_use]
pub fn skim(text: &str) -> Vec<StateVar> {
    let clean = blanked(text);
    let mut out = Vec::new();
    let mut pou = String::new();
    let mut pending: Option<Vec<(String, String)>> = None;
    for (k, line) in clean.lines().enumerate() {
        if let Some(p) = pou_header(line) {
            pou = p;
            pending = None;
            continue;
        }
        let t = line.trim_start();
        if t.to_ascii_uppercase().starts_with("END_") {
            if !t[4..].to_ascii_uppercase().starts_with("VAR") {
                pou.clear();
            }
            pending = None;
            continue;
        }
        let here = pragma(line);
        match declaration(line) {
            Some((name, ty)) => {
                if let Some(attrs) = here.or_else(|| pending.take()) {
                    let get = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
                    out.push(StateVar {
                        name,
                        ty,
                        unit: get("unit"),
                        measured_by: get("measured_by"),
                        pou: pou.clone(),
                        line: k + 1,
                    });
                }
            }
            // A pragma on a line of its own marks the next declaration,
            // blank lines between them or not.
            None => {
                if here.is_some() {
                    pending = here;
                }
            }
        }
    }
    out
}

/// The map `in` signals that would stand for `s` on the real machine, in
/// the order to try: the `measured_by=` attribute, else the name itself,
/// `NAME_in`, `NAME_raw`, and the name without its unit suffix
/// (`level_m` → `level`, `temp_c` → `temp`).
#[must_use]
pub fn measurement_names(s: &StateVar) -> Vec<String> {
    if let Some(m) = &s.measured_by {
        return vec![m.clone()];
    }
    let n = s.name.to_ascii_lowercase();
    let mut v = vec![n.clone(), format!("{n}_in"), format!("{n}_raw")];
    if let Some((stem, suffix)) = n.rsplit_once('_')
        && !stem.is_empty()
        && suffix.len() <= 4
    {
        v.push(stem.to_string());
    }
    v
}

/// The kind of sensor that measures a quantity like `s`, from the words
/// of its name and unit, and the type: a hint for the engineer and the
/// agent, never a part number.
#[must_use]
pub fn suggest(s: &StateVar) -> &'static str {
    let name = s.name.to_ascii_lowercase();
    let unit = s.unit.as_deref().unwrap_or("").to_ascii_lowercase();
    let words: Vec<&str> = name.split('_').collect();
    let has = |ws: &[&str]| ws.iter().any(|w| words.contains(w));
    let unit_is = |us: &[&str]| us.contains(&unit.as_str());
    if has(&["level", "lvl", "tank", "well", "fill"]) {
        "level transmitter"
    } else if has(&["press", "pressure"]) || unit_is(&["bar", "psi", "kpa", "pa", "mbar"]) {
        "pressure transducer"
    } else if has(&["temp", "temperature"]) || unit_is(&["degc", "°c", "c", "k", "degf", "°f"]) {
        "RTD or thermocouple"
    } else if has(&["flow"]) || unit_is(&["l/min", "lpm", "gpm", "m3/h", "m3h"]) {
        "flow meter"
    } else if has(&["speed", "rpm", "velocity", "freq"]) || unit_is(&["rpm", "hz", "m/s"]) {
        "encoder or tachometer"
    } else if has(&[
        "pos",
        "position",
        "travel",
        "stroke",
        "angle",
        "extended",
        "retracted",
    ]) || unit_is(&["mm", "m", "deg", "°"])
    {
        "encoder or proximity switch"
    } else if has(&["weight", "mass", "load"]) || unit_is(&["kg", "g", "lb", "t"]) {
        "load cell"
    } else if has(&["parts", "count", "cycles", "pieces"]) {
        "counter input (photo eye or proximity switch)"
    } else if s.ty == "BOOL" {
        "a switch (limit, proximity or float)"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "(* PLANT MODEL {state} in prose is not a pragma *)
FUNCTION_BLOCK plant_model
VAR_INPUT
  pump : BOOL;
END_VAR
VAR
  motor : TON;
  level_m : REAL := 1.5;  {state unit=m}
  {STATE unit=degC measured_by=temp_in}
  temp_c : REAL := 20.0;
  note : STRING := '{state} in a string is not one';
  storm : BOOL; // {state} in a line comment is not one
  parts : INT; { state }
END_VAR
level_m := level_m + 0.1;
END_FUNCTION_BLOCK
PROGRAM main
VAR
  plant : plant_model;
  x : INT; {state}
END_VAR
END_PROGRAM
";

    #[test]
    fn skims_marked_declarations_with_their_pou_unit_and_line() {
        let s = skim(MODEL);
        let names: Vec<(&str, &str, &str, usize)> = s
            .iter()
            .map(|v| (v.pou.as_str(), v.name.as_str(), v.ty.as_str(), v.line))
            .collect();
        assert_eq!(
            names,
            [
                ("plant_model", "level_m", "REAL", 8),
                ("plant_model", "temp_c", "REAL", 10),
                ("plant_model", "parts", "INT", 13),
                ("main", "x", "INT", 20),
            ]
        );
        assert_eq!(s[0].unit.as_deref(), Some("m"));
        assert_eq!(s[1].unit.as_deref(), Some("degC"));
        assert_eq!(s[1].measured_by.as_deref(), Some("temp_in"));
        assert_eq!(s[2].unit, None);
    }

    #[test]
    fn comments_strings_and_other_pragmas_are_not_states() {
        assert!(skim("VAR\n  a : INT; (* {state} *)\nEND_VAR\n").is_empty());
        assert!(skim("VAR\n  a : INT; {attribute 'x'}\nEND_VAR\n").is_empty());
        assert!(skim("VAR\n  s : STRING := '{state}';\nEND_VAR\n").is_empty());
        assert!(
            skim("VAR\n  a AT %IX0.0 : BOOL; {state}\nEND_VAR\n").is_empty(),
            "located, not a state"
        );
        // A pragma on its own line marks only the very next declaration.
        let s = skim("VAR\n  {state}\n\n  a : INT;\n  b : INT;\nEND_VAR\n");
        assert_eq!(s.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn garbage_never_panics() {
        for text in [
            "",
            "{",
            "{state",
            "(* unterminated",
            "'unterminated",
            "VAR a : {state}",
            "{state} {state} {state}",
            &"{state} a : INT;\n".repeat(5000),
            &"x".repeat(1 << 16),
            "\u{0}{state}\u{feff}a : INT;",
            "FUNCTION_BLOCK\nVAR\n  a : INT; {state}\nEND_VAR",
        ] {
            let _ = skim(text);
        }
    }

    fn var(name: &str, ty: &str, unit: Option<&str>) -> StateVar {
        StateVar {
            name: name.into(),
            ty: ty.into(),
            unit: unit.map(str::to_string),
            measured_by: None,
            pou: "plant_model".into(),
            line: 1,
        }
    }

    #[test]
    fn measurement_names_try_the_attribute_then_the_name_and_its_stem() {
        let mut v = var("level_m", "REAL", Some("m"));
        assert_eq!(
            measurement_names(&v),
            ["level_m", "level_m_in", "level_m_raw", "level"]
        );
        v.measured_by = Some("tx_level".into());
        assert_eq!(measurement_names(&v), ["tx_level"]);
        assert_eq!(
            measurement_names(&var("parts", "INT", None)),
            ["parts", "parts_in", "parts_raw"]
        );
        assert_eq!(
            measurement_names(&var("belt_position", "REAL", None)),
            ["belt_position", "belt_position_in", "belt_position_raw"],
            "a long suffix is a word, not a unit"
        );
    }

    #[test]
    fn suggestions_read_the_name_then_the_unit_then_the_type() {
        assert_eq!(suggest(&var("level_m", "REAL", Some("m"))), "level transmitter");
        assert_eq!(suggest(&var("bar", "REAL", Some("bar"))), "pressure transducer");
        assert_eq!(
            suggest(&var("supply_c", "REAL", Some("degC"))),
            "RTD or thermocouple"
        );
        assert_eq!(suggest(&var("temp_c", "REAL", None)), "RTD or thermocouple");
        assert_eq!(
            suggest(&var("dirt_pa", "REAL", Some("Pa"))),
            "pressure transducer"
        );
        assert_eq!(suggest(&var("belt_speed", "REAL", None)), "encoder or tachometer");
        assert_eq!(
            suggest(&var("arm_position", "REAL", Some("mm"))),
            "encoder or proximity switch"
        );
        assert_eq!(
            suggest(&var("parts", "INT", None)),
            "counter input (photo eye or proximity switch)"
        );
        assert_eq!(
            suggest(&var("door", "BOOL", None)),
            "a switch (limit, proximity or float)"
        );
        assert_eq!(suggest(&var("value", "REAL", None)), "unknown");
    }
}
