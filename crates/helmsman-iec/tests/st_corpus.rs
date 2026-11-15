//! Real Structured Text: every `.st` file under `$HELMSMAN_ST_CORPUS` is run
//! through the parser and the compiler, and the test reports, per file,
//! whether it parses and whether it compiles, a total, and the refusals
//! ranked by cause. The ranking is the measured order of the dialect work
//! (INVESTIGATION.md §3, §4).
//!
//! A refusal is not a failure: these files are written for other tools
//! (CODESYS, Automation Studio, ruSTy, matiec) and use things this runtime
//! does not have. The test fails on a panic, and on a file in [`COMPILES`]
//! that stops compiling.
//!
//! The public API cannot say which of lexing and parsing refused a file
//! (`helmsman_iec::ast::parse` does both), nor separate the checker from
//! code generation (`helmsman_iec::compile` does both), so the stages are
//! "parses" (lex and parse) and "compiles" (check and compile).
//!
//! `scripts/st-corpus.sh` fetches the corpus (pinned by commit and
//! SHA-256, never vendored) and runs this test against it. Without the
//! variable the test has nothing to read and passes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use helmsman_iec::{ImageSizes, ast, compile};

const SIZES: ImageSizes = ImageSizes {
    i: 32,
    q: 32,
    m: 1024,
};

/// Files (path suffix) that must compile: they did when this list was
/// written, and a change that loses one is a regression.
const COMPILES: &[&str] = &[
    "iec-checker/test/st/good/empty-body.st",
    "iec-checker/test/st/good/time-literals.st",
    "iec-checker/test/st/plcopen-cp8.st",
    "iec-checker/test/st/plcopen-l17.st",
    "iec-checker/test/st/plcopen-l22.st",
    "iec-checker/test/st/plcopen-n10.st",
    "iec-checker/test/st/plcopen-n2.st",
    "iec-checker/test/st/plcopen-n3.st",
    "iec-checker/test/st/plcopen-n4.st",
    "iec-checker/test/st/plcopen-n6.st",
    "iec-checker/test/st/plcopen-n8.st",
    "rusty/tests/lit/single/build_info/build_info_in_comment_section.st",
    "rusty/tests/lit/single/build_info/version_flag.st",
    "rusty/tests/lit/single/debugging/overwritten_store_rows.st",
    "rusty/tests/lit/single/init/array_partial_init_warning_type_and_var.st",
];

/// Appended to a unit that has functions and function blocks but no
/// `PROGRAM`, so the checker looks at the library code too.
const STUB: &str = "\nPROGRAM st_corpus_stub\nEND_PROGRAM\n";

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Parses and compiles.
    Compiles,
    /// Parses and compiles once a stub PROGRAM is added (a library file).
    CompilesAsLibrary,
    /// Refused by the parser, with the first diagnostic.
    ParseRefused(String),
    /// Parses, refused by the checker, with the first diagnostic.
    CompileRefused(String),
    Panicked,
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|e| e == "st") {
            out.push(p);
        }
    }
}

fn corpus() -> Option<(PathBuf, Vec<PathBuf>)> {
    let root = PathBuf::from(std::env::var_os("HELMSMAN_ST_CORPUS")?);
    let mut found = Vec::new();
    files(&root, &mut found);
    assert!(!found.is_empty(), "no .st file under {}", root.display());
    Some((root, found))
}

fn rel(root: &Path, f: &Path) -> String {
    f.strip_prefix(root)
        .unwrap_or(f)
        .to_string_lossy()
        .replace('\\', "/")
}

fn run(src: &str) -> Outcome {
    if let Err(d) = ast::parse(src) {
        return Outcome::ParseRefused(d.msg);
    }
    match compile(src, SIZES) {
        Ok(_) => Outcome::Compiles,
        Err(d) if d.len() == 1 && d[0].msg == "no PROGRAM found" => {
            match compile(&format!("{src}{STUB}"), SIZES) {
                Ok(_) => Outcome::CompilesAsLibrary,
                Err(d) => Outcome::CompileRefused(d[0].msg.clone()),
            }
        }
        Err(d) => Outcome::CompileRefused(d[0].msg.clone()),
    }
}

/// The source names a file written to be refused: matiec's `bad_*.st`,
/// iec-checker's `bad/`. Those are listed apart and left out of the ranking.
fn expects_refusal(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.starts_with("bad_") || name.contains("/bad/")
}

/// Words that stay in a cause: the language's own, not the file's names.
const KEYWORDS: &[&str] = &[
    "PROGRAM",
    "FUNCTION",
    "FUNCTION_BLOCK",
    "TYPE",
    "END_TYPE",
    "END_IF",
    "END_FOR",
    "END_WHILE",
    "END_CASE",
    "END_PROGRAM",
    "END_FUNCTION",
    "END_FUNCTION_BLOCK",
    "CONFIGURATION",
    "RESOURCE",
    "TASK",
    "INTERFACE",
    "METHOD",
    "PROPERTY",
    "ACTION",
    "UNIT",
    "NAMESPACE",
    "STRUCT",
    "END_STRUCT",
    "UNION",
    "VAR",
    "VAR_INPUT",
    "VAR_OUTPUT",
    "VAR_IN_OUT",
    "VAR_TEMP",
    "VAR_GLOBAL",
    "VAR_EXTERNAL",
    "VAR_ACCESS",
    "VAR_CONFIG",
    "VAR_STAT",
    "END_VAR",
    "CONSTANT",
    "RETAIN",
    "PERSISTENT",
    "AT",
    "IF",
    "THEN",
    "ELSE",
    "ELSIF",
    "FOR",
    "TO",
    "BY",
    "DO",
    "WHILE",
    "REPEAT",
    "UNTIL",
    "CASE",
    "OF",
    "EXIT",
    "CONTINUE",
    "RETURN",
    "ARRAY",
    "POINTER",
    "REFERENCE",
    "REF_TO",
    "STRING",
    "WSTRING",
    "CHAR",
    "WCHAR",
    "TIME",
    "LTIME",
    "DATE",
    "LDATE",
    "DT",
    "LDT",
    "TOD",
    "LTOD",
    "DATE_AND_TIME",
    "TIME_OF_DAY",
    "BOOL",
    "BYTE",
    "WORD",
    "DWORD",
    "LWORD",
    "SINT",
    "INT",
    "DINT",
    "LINT",
    "USINT",
    "UINT",
    "UDINT",
    "ULINT",
    "REAL",
    "LREAL",
    "ANY",
];

/// A short, countable name for why a file was refused: the diagnostic with
/// its names, numbers and quoted text taken out, so "unknown type FOO" and
/// "unknown type BAR" rank together.
fn cause(stage: &str, msg: &str) -> String {
    let mut out = String::new();
    let mut quoted: Option<char> = None;
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let w = std::mem::take(word);
        let numeric = w.chars().all(|c| c.is_ascii_digit() || c == '.');
        // Our own message words are lower case; an upper case word stays
        // only when it is a language keyword or type, anything else is a
        // name from the file.
        let ours = w.chars().all(|c| c.is_ascii_lowercase());
        if numeric {
            out.push('N');
        } else if ours || KEYWORDS.contains(&w.as_str()) {
            out.push_str(&w);
        } else {
            out.push('X');
        }
    };
    let keep_quoted = msg.starts_with("unexpected character");
    for c in msg.chars() {
        if let Some(q) = quoted {
            if c == q {
                quoted = None;
            } else if keep_quoted {
                out.push(c);
            }
            continue;
        }
        if c == '`' || c == '\'' || c == '"' {
            flush(&mut word, &mut out);
            quoted = Some(c);
            continue;
        }
        if c.is_alphanumeric() || c == '_' || c == '.' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    format!("{stage}: {out}")
}

type Causes = BTreeMap<String, BTreeMap<String, usize>>;

/// The per-source totals and the refusals ranked by cause.
fn summary(by_source: &BTreeMap<String, [usize; 5]>, causes: &Causes) -> String {
    let mut report = String::new();
    writeln!(
        report,
        "\nper source (files, parse, compile, refused, refused as expected):"
    )
    .unwrap();
    let mut total = [0usize; 5];
    for (s, r) in by_source {
        writeln!(
            report,
            "  {s}: {} files, {} parse, {} compile, {} refused, {} refused as expected",
            r[0], r[1], r[2], r[3], r[4]
        )
        .unwrap();
        for (t, v) in total.iter_mut().zip(r) {
            *t += v;
        }
    }
    writeln!(
        report,
        "total: {} files, {} parse, {} compile, {} refused, {} refused as expected (named bad_* or bad/, left out of the ranking)",
        total[0], total[1], total[2], total[3], total[4]
    )
    .unwrap();

    let mut ranked: Vec<(&String, usize, String)> = causes
        .iter()
        .map(|(c, by)| {
            let list: Vec<String> = by.iter().map(|(s, n)| format!("{s} {n}")).collect();
            (c, by.values().sum(), list.join(", "))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    writeln!(report, "\nrefusals by cause (first diagnostic of each file):").unwrap();
    for (c, n, by) in ranked {
        writeln!(report, "  {n:>4} files  {c}  ({by})").unwrap();
    }
    report
}

#[test]
fn real_st_compiles_or_says_why_not() {
    let Some((root, found)) = corpus() else {
        eprintln!("HELMSMAN_ST_CORPUS unset: no corpus to read");
        return;
    };
    let mut report = String::new();
    let mut by_source: BTreeMap<String, [usize; 5]> = BTreeMap::new();
    let mut causes = Causes::new();
    let mut regressions = Vec::new();
    for f in &found {
        let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let src = String::from_utf8_lossy(&bytes);
        let name = rel(&root, f);
        let source = name.split('/').next().unwrap_or("").to_string();
        let outcome = std::panic::catch_unwind(|| run(&src)).unwrap_or(Outcome::Panicked);
        let expected = expects_refusal(&name);
        let row = by_source.entry(source.clone()).or_default();
        row[0] += 1;
        let line = match &outcome {
            Outcome::Compiles => {
                row[1] += 1;
                row[2] += 1;
                format!("parses  compiles  {name}")
            }
            Outcome::CompilesAsLibrary => {
                row[1] += 1;
                row[2] += 1;
                format!("parses  compiles  {name} (library: no PROGRAM)")
            }
            Outcome::ParseRefused(m) => {
                let c = cause("parse", m);
                if expected {
                    row[4] += 1;
                } else {
                    row[3] += 1;
                    *causes.entry(c).or_default().entry(source).or_default() += 1;
                }
                let tag = if expected {
                    "refused-as-expected"
                } else {
                    "refused"
                };
                format!("-       -       {name}: {tag}, {m}")
            }
            Outcome::CompileRefused(m) => {
                row[1] += 1;
                let c = cause("compile", m);
                if expected {
                    row[4] += 1;
                } else {
                    row[3] += 1;
                    *causes.entry(c).or_default().entry(source).or_default() += 1;
                }
                let tag = if expected {
                    "refused-as-expected"
                } else {
                    "refused"
                };
                format!("parses  -         {name}: {tag}, {m}")
            }
            Outcome::Panicked => {
                regressions.push(format!("PANIC {name}"));
                format!("PANIC   {name}")
            }
        };
        writeln!(report, "{line}").unwrap();
        let compiled = matches!(outcome, Outcome::Compiles | Outcome::CompilesAsLibrary);
        if !compiled && COMPILES.iter().any(|c| name.ends_with(c)) {
            regressions.push(format!("{name} no longer compiles"));
        }
    }

    report.push_str(&summary(&by_source, &causes));
    eprint!("{report}");
    assert!(regressions.is_empty(), "{}", regressions.join("\n"));
}
