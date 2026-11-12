//! One layout for Structured Text (Nick, 2026-11-11: formatted on save
//! and deploy, so every project reads the same): two spaces a level,
//! keywords and elementary types in capitals, a statement a line, one
//! space round operators, no run of blank lines.
//!
//! [`format`] works on the pieces [`crate::scan`] cuts, so it lays out
//! text that does not compile as well as text that does, keeps every
//! comment as written, and changes nothing but spacing, line breaks and
//! the case of keywords. It checks that itself: if the formatted text's
//! code or comments differ from the source's, or the compiler's lexer
//! reads other tokens from it, the source comes back untouched.

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use crate::scan::{BlockKind, Kind, Piece, Word, scan, word};

/// Spaces a level.
const INDENT: usize = 2;
/// Levels a statement's later lines stand in from its first.
const CONTINUED: usize = 2;

/// A piece as the formatter writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tok {
    kind: Kind,
    /// As written, but keywords, elementary types and TRUE/FALSE in
    /// capitals.
    text: String,
    /// A word's text upper-cased; empty for anything else.
    upper: String,
    class: Option<Word>,
}

impl Tok {
    fn new(src: &str, p: &Piece) -> Self {
        let raw = p.text(src);
        if p.kind != Kind::Word {
            return Self {
                kind: p.kind,
                text: written(p.kind, raw).into(),
                upper: String::new(),
                class: None,
            };
        }
        let upper = raw.to_ascii_uppercase();
        let class = word(&upper);
        let text = if class == Word::Name {
            raw.into()
        } else {
            upper.clone()
        };
        Self {
            kind: p.kind,
            text,
            upper,
            class: Some(class),
        }
    }

    fn is(&self, op: &str) -> bool {
        self.kind == Kind::Op && self.text == op
    }

    fn keyword(&self) -> bool {
        self.class == Some(Word::Keyword)
    }

    fn code(&self) -> bool {
        self.kind != Kind::Comment
    }

    /// An operator a statement cannot end on: what follows it on the next
    /// line is the rest of the statement.
    fn binds_on(&self) -> bool {
        match self.kind {
            Kind::Op => !matches!(self.text.as_str(), ";" | ")" | "]" | ":"),
            Kind::Word => matches!(self.upper.as_str(), "AND" | "OR" | "XOR" | "MOD" | "NOT"),
            _ => false,
        }
    }

    /// An operator a statement cannot start with.
    fn binds_back(&self) -> bool {
        match self.kind {
            Kind::Op => !matches!(self.text.as_str(), "(" | "[" | "-" | "+" | ";"),
            Kind::Word => matches!(self.upper.as_str(), "AND" | "OR" | "XOR" | "MOD"),
            _ => false,
        }
    }
}

/// The source as the lines it will be written on: the writer's own line
/// breaks, and a break after each statement, section header and THEN, DO,
/// ELSE, REPEAT or CASE … OF that has code after it on its line. An empty
/// line is a blank one.
fn lines(src: &str) -> Vec<Vec<Tok>> {
    let mut out: Vec<Vec<Tok>> = Vec::new();
    let mut cur: Vec<Tok> = Vec::new();
    let mut depth = 0usize;
    // The next code starts a line of its own.
    let mut split = false;
    // Past a VAR keyword, while only its qualifiers have followed.
    let mut var_header = false;
    let mut case_header = false;
    for p in scan(src) {
        match p.kind {
            Kind::Space => continue,
            Kind::Newline => {
                out.push(core::mem::take(&mut cur));
                (split, var_header) = (false, false);
                continue;
            }
            _ => {}
        }
        let t = Tok::new(src, &p);
        if t.code() {
            let qualifier = matches!(
                t.upper.as_str(),
                "CONSTANT" | "RETAIN" | "NON_RETAIN" | "PERSISTENT"
            );
            if var_header && !qualifier {
                (split, var_header) = (true, false);
            }
            let closes = BlockKind::closed_by(&t.upper).is_some()
                || matches!(t.upper.as_str(), "ELSE" | "ELSIF" | "UNTIL");
            let has_code = cur.iter().any(Tok::code);
            if has_code && (split || (closes && depth == 0)) {
                out.push(core::mem::take(&mut cur));
            }
            split = false;
            match (t.kind, t.text.as_str()) {
                (Kind::Op, "(" | "[") => depth += 1,
                (Kind::Op, ")" | "]") => depth = depth.saturating_sub(1),
                (Kind::Op, ";") if depth == 0 => split = true,
                (Kind::Word, _) => match t.upper.as_str() {
                    "THEN" | "DO" | "REPEAT" | "ELSE" | "END_VAR" => split = true,
                    "CASE" => case_header = true,
                    "OF" if case_header => {
                        case_header = false;
                        split = true;
                    }
                    v if v == "VAR" || v.starts_with("VAR_") => var_header = true,
                    _ => {}
                },
                _ => {}
            }
        }
        cur.push(t);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A block open where a line is written.
#[derive(Debug, Clone, Copy)]
struct Open {
    kind: BlockKind,
    /// A CASE past its first label: its statements stand a level in from
    /// the labels.
    labelled: bool,
}

fn width(stack: &[Open]) -> usize {
    stack
        .iter()
        .map(|o| usize::from(o.kind != BlockKind::Pou) + usize::from(o.labelled))
        .sum()
}

/// Whether a line of a CASE is a label: a colon that is not in brackets
/// before any `:=` or `;`.
fn is_label(line: &[Tok]) -> bool {
    let mut depth = 0usize;
    for t in line.iter().filter(|t| t.code()) {
        match (t.kind, t.text.as_str()) {
            (Kind::Op, "(" | "[") => depth += 1,
            (Kind::Op, ")" | "]") => depth = depth.saturating_sub(1),
            (Kind::Op, ":") if depth == 0 => return true,
            (Kind::Op, ":=" | ";") => return false,
            _ => {}
        }
    }
    false
}

/// Whether `cur` stands a space after `prev` on a line. `decl`: a colon
/// here is a declaration's (spaced both sides), not a CASE label's.
fn spaced(prev: &Tok, before: Option<&Tok>, cur: &Tok, decl: bool) -> bool {
    if prev.kind == Kind::Comment || cur.kind == Kind::Comment {
        return true;
    }
    if cur.kind == Kind::Op && matches!(cur.text.as_str(), ";" | "," | ")" | "]" | "." | "..") {
        return false;
    }
    if prev.kind == Kind::Op && matches!(prev.text.as_str(), "(" | "[" | "." | "..") {
        return false;
    }
    // A sign, not a subtraction: it stands against what it signs.
    if prev.kind == Kind::Op && matches!(prev.text.as_str(), "-" | "+") && unary(before) {
        return false;
    }
    if cur.is("(") || cur.is("[") {
        // A call's or an index's bracket stands against its name.
        let operand = match prev.kind {
            Kind::Word => !prev.keyword() || (cur.is("[") && prev.upper == "ARRAY"),
            Kind::Op => matches!(prev.text.as_str(), ")" | "]"),
            _ => true,
        };
        return !operand;
    }
    if cur.is(":") {
        return decl;
    }
    true
}

/// Whether an operator that follows `before` has nothing to its left to
/// work on: it is a sign.
fn unary(before: Option<&Tok>) -> bool {
    match before {
        None => true,
        Some(t) => match t.kind {
            Kind::Op => !matches!(t.text.as_str(), ")" | "]"),
            Kind::Word => t.keyword(),
            _ => false,
        },
    }
}

fn layout(src: &str) -> String {
    let mut out = String::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut depth = 0usize;
    // The last code of the last line with any.
    let mut last: Option<Tok> = None;
    let mut blank = false;
    // The level of the line the statement being written started on.
    let mut statement = 0usize;
    for line in lines(src) {
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        let first = line.iter().find(|t| t.code());
        let continued = first.is_some_and(|f| {
            // A line that starts with a sign goes on from one that did
            // not end its statement.
            let signed =
                (f.is("+") || f.is("-")) && last.as_ref().is_some_and(|l| !l.is(";") && !l.keyword());
            depth > 0 || signed || f.binds_back() || last.as_ref().is_some_and(Tok::binds_on)
        });
        let mut level = width(&stack);
        // The closer a line starts with is handled here, not again below.
        let mut skip_first = false;
        if let Some(f) = first.filter(|_| !continued) {
            let top = stack.last().copied();
            if let Some(kind) = BlockKind::closed_by(&f.upper) {
                if let Some(k) = stack.iter().rposition(|o| o.kind == kind) {
                    stack.truncate(k);
                }
                level = width(&stack);
                skip_first = true;
            } else if matches!(f.upper.as_str(), "ELSE" | "ELSIF")
                && top.is_some_and(|o| o.kind == BlockKind::If)
                || f.upper == "UNTIL" && top.is_some_and(|o| o.kind == BlockKind::Repeat)
            {
                level = level.saturating_sub(1);
            } else if top.is_some_and(|o| o.kind == BlockKind::Case)
                && (f.upper == "ELSE" || (is_label(&line) && BlockKind::opened_by(&f.upper).is_none()))
            {
                level -= usize::from(top.is_some_and(|o| o.labelled));
                if let Some(o) = stack.last_mut() {
                    o.labelled = true;
                }
            }
        }
        if continued {
            level = statement + CONTINUED;
        } else {
            statement = level;
        }
        let decl = stack
            .last()
            .is_some_and(|o| matches!(o.kind, BlockKind::Var | BlockKind::Type | BlockKind::Struct))
            || first.is_some_and(|f| f.upper == "FUNCTION" || f.upper == "TYPE");
        if blank {
            out.push('\n');
            blank = false;
        }
        out.extend(core::iter::repeat_n(' ', level * INDENT));
        let mut code_seen = false;
        for (k, t) in line.iter().enumerate() {
            if k > 0 {
                let before = if k > 1 { Some(&line[k - 2]) } else { last.as_ref() };
                // A sign at the start of a continued line still has the
                // line before it to its left.
                let before = before.filter(|_| k > 1 || continued);
                if spaced(&line[k - 1], before, t, decl) {
                    out.push(' ');
                }
            }
            out.push_str(&t.text);
            if !t.code() {
                continue;
            }
            let skip = skip_first && !code_seen;
            code_seen = true;
            match (t.kind, t.text.as_str()) {
                (Kind::Op, "(" | "[") => depth += 1,
                (Kind::Op, ")" | "]") => depth = depth.saturating_sub(1),
                (Kind::Word, _) if !skip => {
                    if let Some(kind) = BlockKind::opened_by(&t.upper) {
                        stack.push(Open {
                            kind,
                            labelled: false,
                        });
                    } else if let Some(kind) = BlockKind::closed_by(&t.upper)
                        && let Some(k) = stack.iter().rposition(|o| o.kind == kind)
                    {
                        stack.truncate(k);
                    }
                }
                _ => {}
            }
        }
        if let Some(t) = line.iter().rev().find(|t| t.code()) {
            last = Some(t.clone());
        }
        out.push('\n');
    }
    out
}

/// What the formatter must leave as it found it: the code, token by
/// token (a word by its capitals, so `end_if` is `END_IF`), and every
/// comment to the letter.
fn substance(src: &str) -> Vec<String> {
    scan(src)
        .iter()
        .filter(|p| !matches!(p.kind, Kind::Space | Kind::Newline))
        .map(|p| match p.kind {
            Kind::Word => p.text(src).to_ascii_uppercase(),
            kind => written(kind, p.text(src)).into(),
        })
        .collect()
}

/// A piece as it is kept: as written, but a line comment without the
/// spaces after its last word (they would end the line in spaces).
fn written(kind: Kind, text: &str) -> &str {
    if kind == Kind::Comment && text.starts_with("//") {
        text.trim_end()
    } else {
        text
    }
}

/// `src` in the one layout (the module's docs). What comes back differs
/// from `src` only in spacing, line breaks and the capitals of keywords,
/// elementary types and TRUE/FALSE, and formatting it again changes
/// nothing. Text with nothing in it comes back empty.
#[must_use]
pub fn format(src: &str) -> String {
    let out = layout(src);
    // The compiler's own reading is the last word: the same tokens, or
    // the same refusal.
    let read = |text: &str| {
        crate::lexer::lex(text, true)
            .map(|t| t.into_iter().map(|(t, _)| t).collect::<Vec<_>>())
            .map_err(|e| e.msg)
    };
    if substance(&out) == substance(src) && read(&out) == read(src) {
        out
    } else {
        src.into()
    }
}

#[cfg(test)]
mod tests {
    use super::format;

    #[track_caller]
    fn same(src: &str) {
        assert_eq!(format(src), src, "already in the layout");
    }

    #[test]
    fn the_examples_layout_is_the_layout() {
        same(
            "PROGRAM blink
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
(* The timer restarts itself every 500 ms; each time, the lamp flips. *)
pulse(IN := NOT pulse.Q, PT := T#500ms);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
END_PROGRAM
",
        );
    }

    #[test]
    fn a_program_on_few_lines_is_laid_out() {
        let src = "function_block motor
var_input run,fault:bool; end_var
VAR_OUTPUT on : BOOL; END_VAR
if run then if not fault then on:=true; else on:=false; end_if; else on := FALSE; end_if;
END_FUNCTION_BLOCK


program cell
var x AT %IX0.0:BOOL;n:int:=-1;a:ARRAY [1..4] of INT; end_var
n:=n+1 ;   x := (n>3)and not(n=-2);
a[ n ] := abs( -n )*2;
end_program";
        assert_eq!(
            format(src),
            "FUNCTION_BLOCK motor
VAR_INPUT
  run, fault : BOOL;
END_VAR
VAR_OUTPUT
  on : BOOL;
END_VAR
IF run THEN
  IF NOT fault THEN
    on := TRUE;
  ELSE
    on := FALSE;
  END_IF;
ELSE
  on := FALSE;
END_IF;
END_FUNCTION_BLOCK

PROGRAM cell
VAR
  x AT %IX0.0 : BOOL;
  n : INT := -1;
  a : ARRAY[1..4] OF INT;
END_VAR
n := n + 1;
x := (n > 3) AND NOT (n = -2);
a[n] := abs(-n) * 2;
END_PROGRAM
"
        );
    }

    #[test]
    fn loops_and_a_case_are_indented() {
        same(
            "PROGRAM p
VAR
  i, n : INT;
END_VAR
FOR i := 1 TO 10 BY 2 DO
  n := n + i;
END_FOR;
WHILE n > 0 DO
  n := n - 1;
END_WHILE;
REPEAT
  n := n + 1;
UNTIL n >= 3
END_REPEAT;
CASE n OF
  1: n := 2;
  2, 3:
    n := 4;
    IF n = 4 THEN
      n := 5;
    END_IF;
  5..9: n := 0;
  ELSE
    n := -1;
END_CASE;
END_PROGRAM
",
        );
    }

    #[test]
    fn comments_are_kept_to_the_letter_and_stay_where_they_are() {
        same(
            "PROGRAM p
// before the section
VAR
  x : INT; (* counts   parts *)
END_VAR
(* a comment
     of two lines, its own spacing kept *)
x := x + 1; // one   more
/* c */ x := 0;
{ a pragma }
END_PROGRAM
",
        );
        // A line comment ends its line: nothing is ever pulled up after it.
        assert_eq!(format("x := 1; // a  \ny := 2;"), "x := 1; // a\ny := 2;\n");
        assert_eq!(format("x := 1; (* a *) y := 2;"), "x := 1; (* a *)\ny := 2;\n");
    }

    #[test]
    fn a_statement_over_several_lines_keeps_its_breaks() {
        same(
            "PROGRAM p
IF a AND
    b OR
    c THEN
  m(run := a,
      fault := b);
  x := a
      + b;
END_IF;
END_PROGRAM
",
        );
    }

    #[test]
    fn blank_lines_are_one_at_most_and_none_at_the_ends() {
        assert_eq!(
            format("\n\n  x := 1;\n\n\n\ny := 2;\n\n\n"),
            "x := 1;\n\ny := 2;\n"
        );
        assert_eq!(format(""), "");
        assert_eq!(format("  \n\t\n"), "");
        assert_eq!(format("x:=1;\r\ny:=2;\r\n"), "x := 1;\ny := 2;\n");
    }

    #[test]
    fn signs_ranges_members_and_typed_literals() {
        assert_eq!(
            format("x:=-1;y := a - -b; z:=a-b;t:=T#-5s+TIME#1h30m;r:=1.5e-3**2;"),
            "x := -1;\ny := a - -b;\nz := a - b;\nt := T#-5s + TIME#1h30m;\nr := 1.5e-3 ** 2;\n"
        );
        assert_eq!(format("a . b [ 1 , 2 ] . c := m . q ;"), "a.b[1, 2].c := m.q;\n");
        assert_eq!(
            format("m( x:=1 , y=>z ) ;IF(a<>b)THEN RETURN;END_IF;"),
            "m(x := 1, y => z);\nIF (a <> b) THEN\n  RETURN;\nEND_IF;\n"
        );
        assert_eq!(
            format("TYPE Color:(Red,Green);END_TYPE"),
            "TYPE Color : (Red, Green);\nEND_TYPE\n"
        );
        assert_eq!(
            format("FUNCTION f:INT\nf:=1;\nEND_FUNCTION"),
            "FUNCTION f : INT\nf := 1;\nEND_FUNCTION\n"
        );
    }

    /// Broken text is laid out as far as it goes and never loses a thing.
    #[test]
    fn text_that_does_not_compile_is_still_laid_out_safely() {
        for src in [
            "IF a THEN",
            "END_IF; END_IF; x := 1;",
            "x := (1 + ;\ny := 2;",
            "(* never closed\nx := 1;",
            "x := 'a' @ é;",
            "CASE OF OF 1: END_CASE",
            "VAR VAR x END_VAR END_VAR",
            ")))) ]]] x",
        ] {
            let once = format(src);
            assert_eq!(format(&once), once, "{src:?}");
            assert_eq!(super::substance(&once), super::substance(src), "{src:?}");
        }
    }

    /// The tokens the compiler reads, without their places.
    fn tokens(src: &str) -> Result<Vec<crate::lexer::Tok>, String> {
        crate::lexer::lex(src, true)
            .map(|t| t.into_iter().map(|(t, _)| t).collect())
            .map_err(|e| e.msg)
    }

    /// What every formatted text must hold to: formatting it again
    /// changes nothing, the compiler reads the same tokens from it (or
    /// refuses both for the same reason), no line ends in a space, no two
    /// blank lines follow each other, and it ends in one line end.
    #[track_caller]
    fn holds(src: &str) {
        let once = format(src);
        assert_eq!(format(&once), once, "formatted twice: {src:?}");
        assert_eq!(super::substance(&once), super::substance(src), "{src:?}");
        assert_eq!(tokens(&once), tokens(src), "{src:?}");
        // A block comment's own lines are the writer's (blank ones and
        // trailing spaces too); every other line is ours.
        if once != src && !once.contains("(*") && !once.contains("/*") {
            assert!(!once.contains("\n\n\n"), "{once:?}");
            assert!(
                once.is_empty() || once.ends_with('\n') && !once.ends_with("\n\n"),
                "{once:?}"
            );
            assert!(!once.starts_with('\n'), "{once:?}");
            assert!(once.lines().all(|l| l == l.trim_end()), "{once:?}");
        }
    }

    /// Every Structured Text file in the repository: the library, the
    /// examples, the fixtures, the benches.
    #[test]
    fn every_st_file_in_the_repository_formats_to_the_same_tokens() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let path = e.path();
                let name = e.file_name();
                let name = name.to_string_lossy();
                if path.is_dir() {
                    if !name.starts_with('.') && name != "target" && name != "node_modules" {
                        walk(&path, out);
                    }
                } else if name.ends_with(".st") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() >= 20, "found {} .st files", files.len());
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            holds(&src);
            // Real programs are laid out, not handed back by the check.
            assert_eq!(format(&src), super::layout(&src), "{}", f.display());
            // A file that compiles still compiles, to the same warnings.
            let sizes = crate::ImageSizes { i: 64, q: 64, m: 256 };
            let before = crate::compile_warned(&src, sizes);
            let after = crate::compile_warned(&format(&src), sizes);
            assert_eq!(before.0.is_ok(), after.0.is_ok(), "{}", f.display());
            let texts = |d: &[crate::Diagnostic]| d.iter().map(|d| d.msg.clone()).collect::<Vec<_>>();
            assert_eq!(texts(&before.1), texts(&after.1), "{}", f.display());
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(2000))]

        /// Any run of the things a program is made of, in any order and
        /// with any spacing.
        #[test]
        fn any_soup_of_tokens_formats_safely(parts in proptest::collection::vec(
            proptest::sample::select(vec![
                "IF", "if", "THEN", "ELSE", "ELSIF", "END_IF", "CASE", "OF", "END_CASE", "FOR", "TO",
                "DO", "END_FOR", "WHILE", "END_WHILE", "REPEAT", "UNTIL", "END_REPEAT", "VAR",
                "VAR_INPUT", "CONSTANT", "END_VAR", "PROGRAM", "END_PROGRAM", "FUNCTION",
                "FUNCTION_BLOCK", "END_FUNCTION_BLOCK", "TYPE", "STRUCT", "END_STRUCT", "END_TYPE",
                "ARRAY", "AT", "AND", "OR", "NOT", "MOD", "RETURN", "x", "y", "Lamp", "m", "TON",
                "INT", "bool", "TRUE", "false", "1", "2.5", "1e-3", "16#FF", "T#500ms", "TIME#-5s",
                "%IX0.1", ":=", "=>", ":", ";", ",", ".", "..", "(", ")", "[", "]", "+", "-", "*",
                "**", "/", "&", "=", "<>", "<", "<=", ">", ">=", "(* c *)", "(* a\nb *)", "// c\n",
                "/* c */", "{p}", "\n", "\n\n", " ", "  ", "\t", "\r\n", "'", "é", "(*",
            ]),
            0..60,
        ), gaps in proptest::collection::vec(proptest::bool::ANY, 60)) {
            let mut src = String::new();
            for (k, p) in parts.iter().enumerate() {
                src.push_str(p);
                if gaps[k] {
                    src.push(' ');
                }
            }
            holds(&src);
        }

        /// Any text at all: it never panics and never loses a thing.
        #[test]
        fn any_text_formats_safely(src in "\\PC{0,200}") {
            let once = format(&src);
            proptest::prop_assert_eq!(format(&once), once.clone());
            proptest::prop_assert_eq!(super::substance(&once), super::substance(&src));
        }
    }
}
