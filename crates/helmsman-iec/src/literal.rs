//! A literal in ST source as text to replace: where the one that starts
//! at a line and column ends, and whether a text is one literal. For
//! changing a set value (a constant, a timer's preset) where it stands
//! without touching anything else of the line. A STRING literal is left
//! out: its text holds what elsewhere ends a token.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use core::ops::Range;

use crate::lexer::{Tok, lex_spans};

const fn value(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Int { .. } | Tok::Real { .. } | Tok::Bool(_) | Tok::Time(_) | Tok::Calendar(..)
    )
}

/// The bytes of the literal that starts at `line`, `col` (both from 1,
/// the column in bytes) of `src`: a number, `TRUE`/`FALSE`, a time, a
/// date. `None` when the source does not lex or something else starts
/// there.
#[must_use]
pub fn literal_at(src: &str, line: u32, col: u32) -> Option<Range<usize>> {
    let (toks, ends) = lex_spans(src, false).ok()?;
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let off = |line: u32, col: u32| {
        let l = starts.get((line as usize).checked_sub(1)?)?;
        Some(l + (col as usize).checked_sub(1)?)
    };
    let k = toks.iter().position(|(_, p)| p.line == line && p.col == col)?;
    if !value(&toks[k].0) {
        return None;
    }
    let (a, b) = (off(line, col)?, off(ends[k].line, ends[k].col)?);
    (a < b && src.is_char_boundary(a) && src.is_char_boundary(b)).then_some(a..b)
}

/// Whether `text` is one literal and nothing else, a minus sign in front
/// of a number aside.
#[must_use]
pub fn is_literal(text: &str) -> bool {
    let Ok((toks, _)) = lex_spans(text, false) else {
        return false;
    };
    match toks.as_slice() {
        [(t, _), (Tok::Eof, _)] => value(t),
        [(Tok::Minus, _), (t, _), (Tok::Eof, _)] => matches!(t, Tok::Int { .. } | Tok::Real { .. }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_literal, literal_at};

    const SRC: &str = "PROGRAM p\nVAR\n  t : TON;\n  n : INT;\nEND_VAR\nt(IN := TRUE, PT := T#5s); (* 5 s *)\nIF n >= 10 THEN n := n-3; END_IF;\nEND_PROGRAM\n";

    fn at(line: u32, col: u32) -> Option<&'static str> {
        literal_at(SRC, line, col).map(|r| &SRC[r])
    }

    #[test]
    fn a_literal_is_found_whole_where_it_starts_and_nothing_else_is() {
        assert_eq!(at(6, 9), Some("TRUE"));
        assert_eq!(at(6, 21), Some("T#5s"));
        assert_eq!(at(7, 9), Some("10"));
        assert_eq!(at(7, 24), Some("3"), "the minus before it is an operator");
        // A name, an operator, the middle of a literal, a comment, off the page.
        for (l, c) in [(6, 1), (7, 6), (6, 22), (6, 30), (99, 1), (0, 0), (6, 0)] {
            assert_eq!(at(l, c), None, "{l}:{c}");
        }
        assert_eq!(literal_at("x := 'a;b';", 1, 6), None, "a STRING is left out");
        assert_eq!(
            literal_at("x := 'never closed", 1, 6),
            None,
            "source that does not lex"
        );
    }

    #[test]
    fn one_literal_and_nothing_else_is_a_literal() {
        for ok in [
            "5",
            "-5",
            "2.5",
            "-0.1",
            "T#5s",
            "TIME#1h30m",
            "TRUE",
            "false",
            "INT#-5",
            "16#FF",
            "D#2024-02-29",
            " 7 ",
        ] {
            assert!(is_literal(ok), "{ok}");
        }
        for no in [
            "",
            "x",
            "5 + 1",
            "5; y := 1",
            "5 (* c *) 6",
            "-TRUE",
            "- -5",
            "'s'",
            "T#",
            "5;",
            "1 //\n; z := 2",
        ] {
            assert!(!is_literal(no), "{no}");
        }
    }
}
