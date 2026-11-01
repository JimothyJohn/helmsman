//! `project.settings`: how a project's PROGRAMs run, one declaration per
//! line, hand-parsed like a module manifest (whose lines these become):
//!
//! ```text
//! # project.settings
//! author   Nick <nick@example.com>
//! program  conveyor scan_ms=50 q=0+2
//! program  andon    scan_ms=200
//! param    conveyor speed real 1.5 min=0 max=10 unit=m/s
//! ```
//!
//! - `author` (once, free text as in a manifest): who the project's
//!   modules say wrote them. Default: `project NAME`.
//! - `program NAME [scan_ms=N] [i=START+LEN] [q=START+LEN] [m=BYTES]`:
//!   one PROGRAM's schedule and windows (a manifest's `scan_ms`, `i`, `q`,
//!   `m`). A PROGRAM without a line runs every `--scan-ms`; with no `i` /
//!   `q` it has no windows, except the only PROGRAM of a project, which
//!   gets the whole image as a bare program does.
//! - `param PROGRAM NAME TYPE VALUE ...`: a manifest `param` line for that
//!   PROGRAM (`VAR_INPUT NAME` bound to a value instead of a signal).
//!
//! Program names are matched case-insensitively, as ST is. Whatever the
//! manifest checks (ranges, types, windows) is checked when the project
//! builds, and reported on the line that said it.

use std::collections::BTreeMap;

/// One `program` line and the `param` lines for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Program {
    /// The `program` line, if there is one.
    pub line: Option<usize>,
    pub scan_ms: Option<u64>,
    pub i: Option<String>,
    pub q: Option<String>,
    pub m: Option<String>,
    /// `(line, the manifest param line's tail)`.
    pub params: Vec<(usize, String)>,
}

/// A parsed settings file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub author: Option<String>,
    /// By program name, upper case (as the parser names POUs).
    pub programs: BTreeMap<String, Program>,
}

fn window(v: &str) -> bool {
    v.split_once('+').is_some_and(|(a, b)| {
        !a.is_empty()
            && !b.is_empty()
            && a.len() <= 5
            && b.len() <= 5
            && a.bytes().all(|c| c.is_ascii_digit())
            && b.bytes().all(|c| c.is_ascii_digit())
    })
}

fn ident(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=64).contains(&b.len())
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn program_line(rest: &str, p: &mut Program) -> Result<(), String> {
    for kv in rest.split_whitespace() {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("expected KEY=VALUE, got {kv:?}"))?;
        match k {
            "scan_ms" => match v.parse::<u64>() {
                Ok(n @ 1..=10_000) if v.bytes().all(|c| c.is_ascii_digit()) => p.scan_ms = Some(n),
                _ => return Err(format!("scan_ms: expected 1..=10000, got {v:?}")),
            },
            "i" | "q" if window(v) => {
                let slot = if k == "i" { &mut p.i } else { &mut p.q };
                *slot = Some(v.to_string());
            }
            "i" | "q" => return Err(format!("{k}: expected START+LEN, got {v:?}")),
            "m" if !v.is_empty() && v.len() <= 5 && v.bytes().all(|c| c.is_ascii_digit()) => {
                p.m = Some(v.to_string());
            }
            "m" => return Err(format!("m: expected a byte count, got {v:?}")),
            other => return Err(format!("unknown key {other:?} (scan_ms, i, q, m)")),
        }
    }
    Ok(())
}

/// Parses a settings file.
///
/// # Errors
/// Every problem, as `(line from 1, why)`.
pub fn parse(src: &str) -> Result<Settings, Vec<(usize, String)>> {
    let mut s = Settings::default();
    let mut errs = Vec::new();
    for (k, raw) in src.lines().enumerate() {
        let n = k + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match key {
            "author" => {
                if s.author.is_some() {
                    errs.push((n, "author given twice".into()));
                } else if rest.is_empty() {
                    errs.push((n, "author needs a value".into()));
                } else {
                    s.author = Some(rest.to_string());
                }
            }
            "program" => {
                let (name, kvs) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if !ident(name) {
                    errs.push((n, format!("expected a PROGRAM name, got {name:?}")));
                    continue;
                }
                let p = s.programs.entry(name.to_ascii_uppercase()).or_default();
                if p.line.is_some() {
                    errs.push((n, format!("program {name} given twice")));
                    continue;
                }
                p.line = Some(n);
                if let Err(e) = program_line(kvs, p) {
                    errs.push((n, e));
                }
            }
            "param" => {
                let (name, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if !ident(name) || tail.trim().is_empty() {
                    errs.push((n, "expected param PROGRAM NAME TYPE VALUE ...".into()));
                    continue;
                }
                if !tail.is_ascii() {
                    errs.push((n, "a param line is printable ASCII".into()));
                    continue;
                }
                s.programs
                    .entry(name.to_ascii_uppercase())
                    .or_default()
                    .params
                    .push((n, tail.trim().to_string()));
            }
            other => errs.push((
                n,
                format!(
                    "unknown key {:?} (author, program, param)",
                    crate::api::clean(other)
                ),
            )),
        }
    }
    if errs.is_empty() { Ok(s) } else { Err(errs) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_parse_and_mistakes_name_their_line() {
        let s = parse(
            "# c\nauthor Nick\nprogram Conveyor scan_ms=50 q=0+2\nparam conveyor speed real 1 min=0 max=2\n",
        )
        .unwrap();
        let p = &s.programs["CONVEYOR"];
        assert_eq!(
            (p.line, p.scan_ms, p.q.as_deref()),
            (Some(3), Some(50), Some("0+2"))
        );
        assert_eq!(p.params, vec![(4, "speed real 1 min=0 max=2".to_string())]);
        let e = parse("program a scan_ms=0\nprogram a\nbogus\n").unwrap_err();
        assert_eq!(e.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
