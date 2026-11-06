//! Renaming a variable in ST source: every use of the identifier, by the
//! lexer's tokens, so a comment, a pragma or another name that merely
//! contains it is left alone. A member after a dot (`pulse.Q`) is a
//! field of something else and is never renamed. Identifiers compare
//! case-insensitively, as in ST; the new name is written as given.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use crate::lexer::{Tok, lex};

/// `src` with every use of `old` written `new`, and how many there were.
///
/// # Errors
/// When `src` does not lex (the editor shows why), or `new` is not a
/// plain identifier.
pub fn rename_ident(src: &str, old: &str, new: &str) -> Result<(String, usize), String> {
    if !ident(new) {
        return Err(format!(
            "{new:?} is not a name: a letter, then letters, digits or _"
        ));
    }
    let want = old.to_ascii_uppercase();
    let toks = lex(src, false).map_err(|e| format!("{e:?}"))?;
    // Byte offset of each line's start (the lexer's columns count bytes).
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let mut at = Vec::new();
    let mut prev_dot = false;
    for (t, p) in &toks {
        if let Tok::Ident(w) = t
            && !prev_dot
            && *w == want
            && let Some(line) = starts.get(p.line as usize - 1)
        {
            let off = line + p.col as usize - 1;
            if src
                .get(off..off + old.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(old))
            {
                at.push(off);
            }
        }
        prev_dot = *t == Tok::Dot;
    }
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for &off in &at {
        out.push_str(&src[last..off]);
        out.push_str(new);
        last = off + old.len();
    }
    out.push_str(&src[last..]);
    Ok((out, at.len()))
}

fn ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic())
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
        && !s.contains("__")
        && s.len() <= 64
}

#[cfg(test)]
mod tests {
    use super::rename_ident;

    const SRC: &str = "PROGRAM latch\nVAR_INPUT\n  start : BOOL; (* start button *)\n  stop : BOOL;\nEND_VAR\nVAR_OUTPUT\n  motor : BOOL;\nEND_VAR\nVAR\n  t : TON;\nEND_VAR\n// start is in this comment too\nmotor := (START OR motor) AND NOT stop; { start }\nt(IN := start);\nmotor := t.start;\nEND_PROGRAM\n";

    #[test]
    fn renames_every_use_and_nothing_else() {
        let (out, n) = rename_ident(SRC, "start", "go_button").unwrap();
        assert_eq!(n, 3, "{out}");
        assert!(out.contains("  go_button : BOOL; (* start button *)"), "{out}");
        assert!(out.contains("// start is in this comment too"), "{out}");
        assert!(
            out.contains("motor := (go_button OR motor) AND NOT stop; { start }"),
            "{out}"
        );
        assert!(out.contains("t(IN := go_button);"), "{out}");
        assert!(out.contains("motor := t.start;"), "a member after a dot: {out}");
        // The result still builds.
        assert!(crate::ast::parse(&out).is_ok(), "{out}");
    }

    #[test]
    fn a_name_inside_another_is_left_alone() {
        let src = "PROGRAM p\nVAR_INPUT\n  a : BOOL;\n  ab : BOOL;\nEND_VAR\nVAR_OUTPUT\n  q : BOOL;\nEND_VAR\nq := a AND ab;\nEND_PROGRAM\n";
        let (out, n) = rename_ident(src, "a", "x").unwrap();
        assert_eq!(n, 2);
        assert!(out.contains("q := x AND ab;"), "{out}");
    }

    #[test]
    fn refuses_a_bad_name_and_unlexable_source() {
        assert!(rename_ident(SRC, "start", "1go").is_err());
        assert!(rename_ident(SRC, "start", "go button").is_err());
        assert!(rename_ident(SRC, "start", "").is_err());
        assert!(rename_ident("PROGRAM p\nx := 'str';\nEND_PROGRAM\n", "x", "y").is_err());
    }

    /// Whatever the source, a rename never panics and changes only the
    /// renamed spans: undoing it gives the source back.
    #[test]
    fn rename_then_back_is_the_identity() {
        let srcs = [
            SRC,
            "",
            "PROGRAM p END_PROGRAM",
            "x\n\n\r\nx(*x*)x",
            "PROGRAM p\nVAR_INPUT x : BOOL; END_VAR\nEND_PROGRAM",
        ];
        for s in srcs {
            if let Ok((out, _)) = rename_ident(s, "x", "zz_unique") {
                let (back, _) = rename_ident(&out, "zz_unique", "x").unwrap();
                assert_eq!(back, s, "{s:?}");
            }
        }
    }
}
