//! Style hints (Nick, 2026-11-11: a third, lighter level under errors and
//! warnings): things a program compiles and runs with that are most often
//! a slip. A variable declared and never used, one set and never read, an
//! assignment of a variable to itself.
//!
//! Read off [`crate::scan`], POU by POU, so a file gets its hints whether
//! or not the rest of the project compiles. A hint never stops a deploy
//! and never changes what the program does.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use crate::Pos;
use crate::scan::{Kind, Piece, Word, scan, word};

/// One hint: where it points (the variable's declaration, or the
/// statement) and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub pos: Pos,
    pub msg: String,
}

/// Which section a variable is declared in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// `VAR`, `VAR_TEMP`: the POU's own.
    Local,
    Input,
    Output,
    InOut,
    /// `VAR_GLOBAL`, `VAR_EXTERNAL`, `VAR_ACCESS`, `VAR_CONFIG`: used
    /// from elsewhere, so nothing is said of them.
    Shared,
}

fn section(upper: &str) -> Option<Section> {
    Some(match upper {
        "VAR" | "VAR_TEMP" => Section::Local,
        "VAR_INPUT" => Section::Input,
        "VAR_OUTPUT" => Section::Output,
        "VAR_IN_OUT" => Section::InOut,
        v if v.starts_with("VAR_") => Section::Shared,
        _ => return None,
    })
}

#[derive(Debug)]
struct Var {
    /// Upper-cased.
    name: String,
    /// As declared.
    shown: String,
    at: usize,
    section: Section,
    /// Placed at an address (`AT %QX0.0`): the machine reads or writes it.
    placed: bool,
    reads: u32,
    writes: u32,
}

/// A code piece with what the hints ask of it.
struct Code<'a> {
    piece: Piece,
    text: &'a str,
    upper: String,
}

impl Code<'_> {
    fn is(&self, op: &str) -> bool {
        self.piece.kind == Kind::Op && self.text == op
    }

    fn name(&self) -> bool {
        self.piece.kind == Kind::Word && word(&self.upper) == Word::Name
    }
}

/// The hints for `src`, in source order.
#[must_use]
pub fn hints(src: &str) -> Vec<Hint> {
    let code: Vec<Code<'_>> = scan(src)
        .into_iter()
        .filter(Piece::code)
        .map(|piece| {
            let text = piece.text(src);
            Code {
                piece,
                text,
                upper: text.to_ascii_uppercase(),
            }
        })
        .collect();
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut k = 0;
    while k < code.len() {
        let opens = matches!(code[k].upper.as_str(), "PROGRAM" | "FUNCTION_BLOCK" | "FUNCTION")
            && code[k].piece.kind == Kind::Word;
        if !opens {
            k += 1;
            continue;
        }
        let close = format!("END_{}", code[k].upper);
        let end = code[k..]
            .iter()
            .position(|c| c.piece.kind == Kind::Word && c.upper == close)
            .map_or(code.len(), |n| k + n);
        // A POU still being typed (no END_ yet) is not judged: half its
        // body is missing.
        if end < code.len() {
            pou(&code[k + 1..end], &mut found);
        }
        k = end + 1;
    }
    found.sort();
    found
        .into_iter()
        .map(|(at, msg)| Hint {
            pos: pos_of(src, at),
            msg,
        })
        .collect()
}

/// The 1-based line and column (in characters) of byte `at`.
fn pos_of(src: &str, at: usize) -> Pos {
    let before = &src[..at];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    Pos {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        col: u32::try_from(col).unwrap_or(u32::MAX),
    }
}

/// One POU's code, between its keyword and its `END_`.
fn pou(code: &[Code<'_>], found: &mut Vec<(usize, String)>) {
    let mut vars: Vec<Var> = Vec::new();
    let mut body: Vec<&Code<'_>> = Vec::new();
    let mut k = 0;
    while k < code.len() {
        let c = &code[k];
        let Some(sec) = section(&c.upper).filter(|_| c.piece.kind == Kind::Word) else {
            body.push(c);
            k += 1;
            continue;
        };
        let end = code[k..]
            .iter()
            .position(|c| c.piece.kind == Kind::Word && c.upper == "END_VAR")
            .map_or(code.len(), |n| k + n);
        declarations(&code[k + 1..end], sec, &mut vars, &mut body);
        k = end + 1;
    }
    uses(&body, &mut vars, found);
    for v in &vars {
        let msg = match (v.section, v.reads, v.writes) {
            (Section::Shared, _, _) => continue,
            (Section::Output, 0, 0) => format!("{}: this output is never set", v.shown),
            (_, 0, 0) if v.placed => continue,
            (_, 0, 0) => format!("{}: declared but never used", v.shown),
            (Section::Local, 0, _) if !v.placed => format!("{}: set but never read", v.shown),
            _ => continue,
        };
        found.push((v.at, msg));
    }
}

/// A VAR section's declarations: names, an optional `AT` address, a
/// colon, then a type and an optional initial value up to the `;`. What
/// follows the colon may name other variables (a bound, an initial
/// value): that goes to `body` as a read.
fn declarations<'a, 'b>(
    code: &'b [Code<'a>],
    section: Section,
    vars: &mut Vec<Var>,
    body: &mut Vec<&'b Code<'a>>,
) {
    for decl in code.split(|c| c.is(";")) {
        let colon = decl.iter().position(|c| c.is(":")).unwrap_or(decl.len());
        let placed = decl[..colon]
            .iter()
            .any(|c| c.upper == "AT" || c.piece.kind == Kind::Direct);
        for c in decl[..colon].iter().filter(|c| c.name()) {
            // The first declaration of a name is the one that counts.
            if vars.iter().all(|v| v.name != c.upper) {
                vars.push(Var {
                    name: c.upper.clone(),
                    shown: c.text.into(),
                    at: c.piece.start,
                    section,
                    placed,
                    reads: 0,
                    writes: 0,
                });
            }
        }
        // The type itself is not a variable; what follows it may be.
        body.extend(decl.iter().skip(colon + 2));
        // Keeps one declaration's reads from running into the next.
        if let Some(semi) = code.iter().find(|c| c.is(";")) {
            body.push(semi);
        }
    }
}

/// Counts each variable's reads and writes in the body, and finds the
/// assignments of a variable to itself.
fn uses(body: &[&Code<'_>], vars: &mut [Var], found: &mut Vec<(usize, String)>) {
    let mut depth = 0usize;
    for (k, c) in body.iter().enumerate() {
        match c.text {
            "(" | "[" if c.piece.kind == Kind::Op => depth += 1,
            ")" | "]" if c.piece.kind == Kind::Op => depth = depth.saturating_sub(1),
            _ => {}
        }
        if !c.name() {
            continue;
        }
        let prev = k.checked_sub(1).map(|p| body[p]);
        let next = body.get(k + 1);
        // `m.run`: a member of m, whatever else is called run.
        if prev.is_some_and(|p| p.is(".")) {
            continue;
        }
        // `m(run := x, q => y)`: the block's own parameter names.
        if depth > 0 && next.is_some_and(|n| n.is(":=") || n.is("=>")) {
            continue;
        }
        let Some(v) = vars.iter_mut().find(|v| v.name == c.upper) else {
            continue;
        };
        let assigned = depth == 0 && next.is_some_and(|n| n.is(":="));
        let counted = prev.is_some_and(|p| p.piece.kind == Kind::Word && p.upper == "FOR");
        let output_of_call = prev.is_some_and(|p| p.is("=>"));
        if assigned && !counted || output_of_call {
            v.writes += 1;
        } else {
            v.reads += 1;
        }
        // `x := x;` and nothing more.
        let own = body.get(k + 2).is_some_and(|r| r.name() && r.upper == c.upper)
            && body.get(k + 3).is_some_and(|e| e.is(";"))
            && prev.is_none_or(|p| p.is(";") || p.piece.kind == Kind::Word && !p.name());
        if assigned && own {
            found.push((c.piece.start, format!("{0} := {0} has no effect", c.text)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::hints;

    fn said(src: &str) -> Vec<(u32, u32, String)> {
        hints(src)
            .into_iter()
            .map(|h| (h.pos.line, h.pos.col, h.msg))
            .collect()
    }

    #[test]
    fn a_program_with_nothing_to_say_gets_no_hints() {
        assert_eq!(
            said(
                "PROGRAM blink
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
pulse(IN := NOT pulse.Q, PT := T#500ms);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
END_PROGRAM
"
            ),
            []
        );
        // Every .st file in the repository's examples compiles clean and
        // is written with care: the library says nothing of itself.
        assert_eq!(said(crate::LIBRARY), []);
    }

    #[test]
    fn unused_set_but_never_read_and_self_assignment() {
        let src = "PROGRAM p
VAR_INPUT
  go, spare : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
  horn : BOOL;
END_VAR
VAR
  count : INT;
  seen : BOOL;
  idle : TON;
  n : INT := 3;
END_VAR
lamp := go;
seen := go;
count := count;
n := n + 1;
END_PROGRAM
";
        assert_eq!(
            said(src),
            [
                (3, 7, "spare: declared but never used".into()),
                (7, 3, "horn: this output is never set".into()),
                (11, 3, "seen: set but never read".into()),
                (12, 3, "idle: declared but never used".into()),
                (17, 1, "count := count has no effect".into()),
            ]
        );
    }

    /// What looks like a slip and is not.
    #[test]
    fn what_is_used_some_other_way_is_left_alone() {
        let src = "FUNCTION_BLOCK motor
VAR_INPUT
  run : BOOL;
END_VAR
VAR_OUTPUT
  on : BOOL;
END_VAR
on := run;
END_FUNCTION_BLOCK

PROGRAM cell
VAR
  start AT %IX0.0 : BOOL;
  lamp AT %QX0.1 : BOOL;
  spare AT %QX0.2 : BOOL;
  size : INT := 4;
  grid : ARRAY[1..size] OF INT;
  m : motor;
  run : BOOL;
  i : INT;
  total : INT;
  done : BOOL;
END_VAR
VAR_GLOBAL
  shared : INT;
END_VAR
(* run here is the motor's own input; the program's run is read below *)
m(run := start, on => lamp);
FOR i := 1 TO 4 DO
  grid[i] := i;
  total := total + grid[i];
END_FOR;
done := m.on AND run;
IF done THEN
  total := 0;
END_IF;
END_PROGRAM
";
        // lamp is only ever written, but it is an output of the machine;
        // spare is placed and unused, which is wiring, not a slip; size
        // is read by a bound; i is the loop's; shared is someone else's.
        assert_eq!(said(src), []);
    }

    #[test]
    fn names_are_matched_whatever_their_case_and_only_within_their_pou() {
        let src = "FUNCTION_BLOCK a
VAR
  Temp : INT;
END_VAR
END_FUNCTION_BLOCK
FUNCTION_BLOCK b
VAR
  x : INT;
END_VAR
x := TEMP;
END_FUNCTION_BLOCK
";
        // b's use of TEMP is not a's Temp, and b never reads x.
        assert_eq!(
            said(src),
            [
                (3, 3, "Temp: declared but never used".into()),
                (8, 3, "x: set but never read".into()),
            ]
        );
        assert_eq!(
            said("program p\nvar X : int; end_var\nx := X + 1;\nend_program"),
            []
        );
    }

    /// Text still being typed, or that will never compile: no panic, and
    /// nothing said of a POU that has not been closed.
    #[test]
    fn broken_text_is_safe() {
        for src in [
            "",
            "PROGRAM",
            "PROGRAM p\nVAR x : INT;",
            "PROGRAM p\nVAR x : INT; END_VAR\nx := ",
            "VAR x END_VAR END_PROGRAM",
            "PROGRAM p VAR : ; ; , END_VAR := := ; END_PROGRAM",
            "PROGRAM p\nVAR é : INT; END_VAR\nEND_PROGRAM",
            "(* PROGRAM p VAR x : INT; END_VAR END_PROGRAM",
        ] {
            let _ = hints(src);
        }
        assert_eq!(said("PROGRAM p\nVAR x : INT; END_VAR\n"), []);
    }

    proptest::proptest! {
        /// Any text: never a panic, and every hint points inside it.
        #[test]
        fn any_text_gets_hints_safely(src in "\\PC{0,200}") {
            let lines = u32::try_from(src.matches('\n').count()).unwrap() + 1;
            for h in hints(&src) {
                proptest::prop_assert!(h.pos.line >= 1 && h.pos.line <= lines);
                proptest::prop_assert!(h.pos.col >= 1);
            }
        }
    }
}
