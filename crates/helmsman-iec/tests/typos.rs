//! The common Structured Text typos, each with the one error it must give:
//! at the line and column where the fix goes, in words that name the fix.
//! A regression in error quality fails here.

use helmsman_iec::{ImageSizes, compile};

const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

/// One typo: the program, then the one error it gives (line, column and
/// a part of the message that says what to write).
struct Typo {
    name: &'static str,
    src: &'static str,
    line: u32,
    col: u32,
    msg: &'static str,
}

const fn t(name: &'static str, src: &'static str, line: u32, col: u32, msg: &'static str) -> Typo {
    Typo {
        name,
        src,
        line,
        col,
        msg,
    }
}

const CORPUS: &[Typo] = &[
    // matiec's twenty errors from one ELSE IF, reported at the END_PROGRAM.
    t(
        "ELSE IF for ELSIF",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nELSE IF a > 2 THEN\n  a := 2;\nEND_IF;\nEND_PROGRAM",
        5,
        1,
        "write ELSIF",
    ),
    t(
        "ELSEIF for ELSIF",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nELSEIF a > 2 THEN\n  a := 2;\nEND_IF;\nEND_PROGRAM",
        5,
        1,
        "did you mean ELSIF?",
    ),
    t(
        "= for :=",
        "PROGRAM t\nVAR a : INT; END_VAR\na = 1;\nEND_PROGRAM",
        3,
        3,
        "write := to assign",
    ),
    t(
        ":= in a condition",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF a := 1 THEN\n  b := TRUE;\nEND_IF;\nEND_PROGRAM",
        3,
        6,
        "write = to compare",
    ),
    t(
        "== for =",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF a == 1 THEN\n  b := TRUE;\nEND_IF;\nEND_PROGRAM",
        3,
        6,
        "write = to compare, not ==",
    ),
    t(
        "!= for <>",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF a != 1 THEN\n  b := TRUE;\nEND_IF;\nEND_PROGRAM",
        3,
        6,
        "write <> for not equal",
    ),
    t(
        "&& for AND",
        "PROGRAM t\nVAR a : BOOL; b : BOOL; END_VAR\na := a && b;\nEND_PROGRAM",
        3,
        8,
        "write AND",
    ),
    // The wrong-line complaint: the ; goes at the end of line 3, not on 4.
    t(
        "missing ; before the next statement",
        "PROGRAM t\nVAR a : INT; b : INT; END_VAR\na := 1\nb := 2;\nEND_PROGRAM",
        3,
        7,
        "missing ; at the end of this statement",
    ),
    t(
        "missing ; before END_PROGRAM",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 1\nEND_PROGRAM",
        3,
        7,
        "missing ; at the end of this statement",
    ),
    t(
        "END_IF without ;",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nEND_IF\na := 2;\nEND_PROGRAM",
        5,
        7,
        "missing ; after END_IF",
    ),
    t(
        "missing ; in a declaration",
        "PROGRAM t\nVAR\n  a : INT\n  b : INT;\nEND_VAR\nEND_PROGRAM",
        3,
        10,
        "missing ; at the end of this declaration",
    ),
    t(
        "missing END_IF",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nEND_PROGRAM",
        3,
        1,
        "add END_IF;",
    ),
    t(
        "missing END_FOR",
        "PROGRAM t\nVAR i : INT; END_VAR\nFOR i := 1 TO 3 DO\n  i := i;\nEND_PROGRAM",
        3,
        1,
        "add END_FOR;",
    ),
    t(
        "missing END_CASE",
        "PROGRAM t\nVAR a : INT; END_VAR\nCASE a OF\n1: a := 2;\nEND_PROGRAM",
        3,
        1,
        "add END_CASE;",
    ),
    t(
        "missing END_IF inside a FOR",
        "PROGRAM t\nVAR i : INT; b : BOOL; END_VAR\nFOR i := 1 TO 3 DO\n  IF b THEN\n    i := i;\nEND_FOR;\nEND_PROGRAM",
        4,
        3,
        "add END_IF;",
    ),
    t(
        "missing END_PROGRAM",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 1;\n",
        1,
        1,
        "add END_PROGRAM",
    ),
    t(
        "missing END_WHILE",
        "PROGRAM t\nVAR a : INT; END_VAR\nWHILE a < 3 DO\n  a := a + 1;\nEND_PROGRAM",
        3,
        1,
        "add END_WHILE;",
    ),
    t(
        "missing UNTIL",
        "PROGRAM t\nVAR a : INT; END_VAR\nREPEAT\n  a := a + 1;\nEND_REPEAT;\nEND_PROGRAM",
        5,
        1,
        "missing UNTIL <condition> before END_REPEAT",
    ),
    t(
        "missing END_REPEAT",
        "PROGRAM t\nVAR a : INT; END_VAR\nREPEAT\n  a := a + 1;\nEND_PROGRAM",
        3,
        1,
        "add UNTIL <condition> END_REPEAT;",
    ),
    t(
        "missing END_FUNCTION_BLOCK",
        "FUNCTION_BLOCK fb\nVAR_INPUT a : INT; END_VAR\nPROGRAM t\nEND_PROGRAM",
        1,
        1,
        "add END_FUNCTION_BLOCK",
    ),
    t(
        "ELSIF after ELSE",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nELSE\n  a := 2;\nELSIF a > 2 THEN\n  a := 3;\nEND_IF;\nEND_PROGRAM",
        7,
        1,
        "move this branch before the ELSE",
    ),
    t(
        "missing OF",
        "PROGRAM t\nVAR a : INT; END_VAR\nCASE a\n1: a := 2;\nEND_CASE;\nEND_PROGRAM",
        3,
        7,
        "missing OF",
    ),
    t(
        "an extra END_IF",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 1;\nEND_IF;\nEND_PROGRAM",
        4,
        1,
        "END_IF without a matching IF",
    ),
    t(
        "missing THEN",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b\n  a := 1;\nEND_IF;\nEND_PROGRAM",
        3,
        5,
        "missing THEN",
    ),
    t(
        "missing DO after FOR",
        "PROGRAM t\nVAR i : INT; a : INT; END_VAR\nFOR i := 1 TO 10\n  a := i;\nEND_FOR;\nEND_PROGRAM",
        3,
        17,
        "missing DO",
    ),
    t(
        "missing DO after WHILE",
        "PROGRAM t\nVAR a : INT; END_VAR\nWHILE a < 10\n  a := a + 1;\nEND_WHILE;\nEND_PROGRAM",
        3,
        13,
        "missing DO",
    ),
    t(
        "unclosed (",
        "PROGRAM t\nVAR a : INT; END_VAR\na := (1 + 2;\nEND_PROGRAM",
        3,
        12,
        "missing ): the ( at 3:6",
    ),
    t(
        "an extra )",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 1 + 2);\nEND_PROGRAM",
        3,
        11,
        "unmatched ): remove it",
    ),
    t(
        "missing END_VAR",
        "PROGRAM t\nVAR\n  a : INT;\na := 1;\nEND_PROGRAM",
        4,
        1,
        "add END_VAR before this line",
    ),
    t(
        "missing END_VAR before IF",
        "PROGRAM t\nVAR\n  a : INT;\nIF a > 1 THEN a := 1; END_IF;\nEND_PROGRAM",
        4,
        1,
        "add END_VAR before this line",
    ),
    t(
        "END_VAR misspelled",
        "PROGRAM t\nVAR\n  a : INT;\nEND_VRA\na := 1;\nEND_PROGRAM",
        4,
        1,
        "did you mean END_VAR?",
    ),
    t(
        "missing : in a declaration",
        "PROGRAM t\nVAR a INT; END_VAR\na := 1;\nEND_PROGRAM",
        2,
        7,
        "write A : INT",
    ),
    t(
        "END_IF misspelled",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nEND_IFF;\nEND_PROGRAM",
        5,
        1,
        "did you mean END_IF?",
    ),
    t(
        "WHILE misspelled",
        "PROGRAM t\nVAR a : INT; END_VAR\nWHLIE a < 10 DO\n  a := a + 1;\nEND_WHILE;\nEND_PROGRAM",
        3,
        1,
        "did you mean WHILE?",
    ),
    t(
        "PROGRAM misspelled",
        "PROGAM t\nVAR a : INT; END_VAR\nEND_PROGRAM",
        1,
        1,
        "did you mean PROGRAM?",
    ),
    t(
        "TON misspelled",
        "PROGRAM t\nVAR t1 : TOM; END_VAR\nEND_PROGRAM",
        2,
        10,
        "unknown type TOM (did you mean TOF or TON?)",
    ),
    t(
        "a function misspelled",
        "PROGRAM t\nVAR a : INT; END_VAR\na := ASB(a);\nEND_PROGRAM",
        3,
        6,
        "unknown function ASB (did you mean ABS?)",
    ),
    t(
        "a variable misspelled",
        "PROGRAM t\nVAR count : INT; END_VAR\ncount := cuont + 1;\nEND_PROGRAM",
        3,
        10,
        "undeclared variable CUONT (did you mean COUNT?)",
    ),
    t(
        "an undeclared variable",
        "PROGRAM t\nVAR a : INT; END_VAR\na := cnt + 1;\nEND_PROGRAM",
        3,
        6,
        "undeclared variable CNT (declare it in a VAR block)",
    ),
    t(
        "INT into BOOL",
        "PROGRAM t\nVAR b : BOOL; a : INT; END_VAR\nb := a;\nEND_PROGRAM",
        3,
        6,
        "convert explicitly with INT_TO_BOOL",
    ),
    t(
        "a REAL into an INT",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 1.5;\nEND_PROGRAM",
        3,
        6,
        "use a conversion such as LREAL_TO_INT",
    ),
    t(
        "a comment never closed",
        "PROGRAM t\nVAR a : INT; END_VAR\n(* note\na := 1;\nEND_PROGRAM",
        3,
        1,
        "end it with *)",
    ),
    t(
        "# for a comment",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 1; # note\nEND_PROGRAM",
        3,
        9,
        "comments are written (* like this *)",
    ),
    // There are no string literals yet, so a missing quote is reported as
    // that, at the quote.
    t(
        "a string missing its quote",
        "PROGRAM t\nVAR a : INT; END_VAR\na := 'hello;\nEND_PROGRAM",
        3,
        6,
        "STRING literals are not supported yet",
    ),
];

#[test]
fn each_typo_gives_one_error_at_the_fix_in_plain_words() {
    let mut bad = Vec::new();
    for c in CORPUS {
        let got: Vec<String> = match compile(c.src, SIZES) {
            Ok(_) => vec!["<compiled>".into()],
            Err(d) => d.iter().map(ToString::to_string).collect(),
        };
        let want = format!("{}:{}: ", c.line, c.col);
        let ok = got.len() == 1 && got[0].starts_with(&want) && got[0].contains(c.msg);
        if !ok {
            bad.push(format!("{}: want {want}...{}..., got {got:?}", c.name, c.msg));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} typos:\n{}",
        bad.len(),
        CORPUS.len(),
        bad.join("\n")
    );
}

/// The look-alikes of the typos above that are correct ST, which the hints
/// must leave alone.
#[test]
fn the_correct_look_alikes_compile() {
    for src in [
        // ELSE IF with its own END_IF is a nested IF.
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF b THEN\n  a := 1;\nELSE IF a > 2 THEN\n  a := 2;\nEND_IF;\nEND_IF;\nEND_PROGRAM",
        // Keywords and names are case-insensitive.
        "program t\nvar t1 : ton; x : bool; end_var\nt1(IN := x, PT := T#1s);\nend_program",
        "PROGRAM t\nVAR a : INT; b : BOOL; END_VAR\nIF a = 1 THEN b := TRUE; END_IF;\nEND_PROGRAM",
        "PROGRAM t\nVAR a : BOOL; b : BOOL; END_VAR\na := a & b;\nEND_PROGRAM",
        // A statement may span lines.
        "PROGRAM t\nVAR a : INT; END_VAR\na :=\n  1\n  + 2;\nIF a\n  > 1\nTHEN a := 0; END_IF;\nEND_PROGRAM",
    ] {
        if let Err(d) = compile(src, SIZES) {
            panic!("{src}\n{d:#?}");
        }
    }
}
