//! Requirement tests (TODO.md `spec-tests`, interview 2026-11-02): a
//! project's `test_NAME.st` files are PROGRAMs that script a program's
//! inputs, step the simulation and check its outputs with the `ASSERT`
//! standard block. They are kept with the project and written in the
//! same language as the program, by the agent or the engineer; the box
//! and the page run them before a deploy or a "Use this code" and refuse
//! a red one, naming the requirement in the engineer's words.
//!
//! How one runs ([`run`]): the test and the program under test are two
//! compiled [`crate::Program`]s scanned in lockstep at the project's period on
//! one clock. Each scan the test runs first; its `VAR_OUTPUT`s are copied
//! into the program's `VAR_INPUT`s of the same name (the test drives the
//! inputs), the program runs, and its `VAR_OUTPUT`s are copied into the
//! test's `VAR_INPUT`s of the same name (the test reads the outputs). The
//! test ends when its `DONE` output goes TRUE (`TEST_DONE` when it
//! declares one, for a program with a `done` of its own); it fails when any of its
//! `ASSERT` instances has `FAILED`, when either program faults, when it
//! never sets `DONE` within the limit, or when it holds no `ASSERT` at all
//! (a test that checks nothing is not green). It also fails when it is done
//! but a declared `ASSERT` was never called (`CHECKED` stayed FALSE), or when
//! it shares no port with the program under test (reads no output, drives no
//! input), since such a test can pass whatever the program does. Deterministic: no wall
//! clock, no I/O.
//!
//! **No test yet** (Nick, 2026-11-05: the Requirements tab is a list of
//! requirements, each with the test the agent writes behind it). A test
//! file that states its requirement (a first comment) but declares no
//! `ASSERT`, or whose PROGRAM has an empty body, is *untested*
//! ([`Outcome::Untested`]): the requirement is written down and nothing
//! checks it yet. It is neither green nor red: [`red`] leaves it out, so
//! it never refuses a deploy or a "Use this code", and [`Verdict::state`]
//! says `untested`. A file with neither a comment nor an empty body and
//! no `ASSERT` is still red ([`Outcome::NoAssert`]). [`header`] writes the
//! requirement line a new test file starts with.
//!
//! What a test says in words: its first comment is the requirement (the
//! tab shows it); each `ASSERT` instance's name, and the comment on its
//! declaration line if there is one, name the check that failed.
//!
//! ```text
//! (* The lamp is on within 2 s of start_pb. *)
//! PROGRAM test_lamp
//! VAR_INPUT lamp : BOOL; END_VAR            (* the program's output *)
//! VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR   (* its input; the end *)
//! VAR t : TON; lamp_on_in_time : ASSERT; (* on within 2 s *) END_VAR
//! start_pb := TRUE;
//! t(IN := TRUE, PT := T#2s);
//! IF t.Q THEN
//!   lamp_on_in_time(COND := lamp);
//!   done := TRUE;
//! END_IF;
//! END_PROGRAM
//! ```
//!
//! Test files are not part of the deployed build (the box's project build
//! and the page's simulation both skip [`is_test`] files), so an old test
//! never collides with a program.

use core::fmt::Write as _;
use core::time::Duration;

use crate::ast::{self, PouKind, Section, TypeRef};
#[cfg(not(feature = "std"))]
use crate::prelude::*;
use crate::{Diagnostic, Image, ImageSizes, Port, PortDir, Value, compile};

/// The file name prefix that makes a `.st` file a test: `test_NAME.st`.
pub const TEST_PREFIX: &str = "test_";

/// The output a test sets TRUE when it is done.
pub const DONE: &str = "DONE";

/// The same, for a test of a program that has a `done` of its own (the
/// test reads that one as a `VAR_INPUT done`): a test that declares
/// `TEST_DONE` ends on it instead of [`DONE`].
pub const TEST_DONE: &str = "TEST_DONE";

/// The standard block a test checks with.
pub const ASSERT: &str = "ASSERT";

/// Whether `file` names a requirement test (`test_NAME.st`, case-insensitive).
#[must_use]
pub fn is_test(file: &str) -> bool {
    file.len() >= TEST_PREFIX.len() + 3
        && file.is_char_boundary(TEST_PREFIX.len())
        && file.is_char_boundary(file.len() - 3)
        && file[..TEST_PREFIX.len()].eq_ignore_ascii_case(TEST_PREFIX)
        && file[file.len() - 3..].eq_ignore_ascii_case(".st")
}

/// How tests run: the scan period (the project's), how much simulated
/// time a test may take before it is "never done", and the image sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub period: Duration,
    pub limit: Duration,
    pub sizes: ImageSizes,
}

impl Settings {
    /// 10 ms scans, two minutes of simulated time, the sizes given.
    #[must_use]
    pub const fn new(sizes: ImageSizes) -> Self {
        Self {
            period: Duration::from_millis(10),
            limit: Duration::from_secs(120),
            sizes,
        }
    }

    #[must_use]
    pub const fn period(mut self, period: Duration) -> Self {
        self.period = period;
        self
    }

    #[must_use]
    pub const fn limit(mut self, limit: Duration) -> Self {
        self.limit = limit;
        self
    }
}

/// One test file, ready to run: its source with the project's shared
/// blocks, and the requirement it states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSrc {
    pub file: String,
    pub src: String,
    /// The test's first comment, one line; the file's name when it has none.
    pub requirement: String,
    /// Whether the file states its requirement (has a first comment).
    pub stated: bool,
}

/// A project's `.st` files split for testing: the build (every non-test
/// file, as the page compiles it) and each test with the blocks it may use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub main: String,
    pub tests: Vec<TestSrc>,
}

/// Collapses a comment's text to one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `(* ... *)` comment in `src`, one line, if any.
fn first_comment(src: &str) -> Option<String> {
    let start = src.find("(*")?;
    let end = src[start + 2..].find("*)")? + start + 2;
    let text = one_line(&src[start + 2..end]);
    (!text.is_empty()).then_some(text)
}

/// `lamp_on_in_time` as words: `lamp on in time`.
fn words(name: &str) -> String {
    name.to_ascii_lowercase().replace('_', " ")
}

/// Whether a parsed unit holds a PROGRAM; `None` when it does not parse.
fn has_program(text: &str) -> Option<bool> {
    ast::parse(text)
        .ok()
        .map(|u| u.pous.iter().any(|p| matches!(p.kind, PouKind::Program)))
}

fn push_file(out: &mut String, name: &str, text: &str) {
    let _ = writeln!(out, "(* file {name} *)");
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// Splits `files` (name, text; the project's `.st` files in name order)
/// into the build and its tests. A test's source is its file followed by
/// every non-test file that parses and holds no PROGRAM (the project's
/// shared blocks), each under a `(* file NAME *)` line.
#[must_use]
pub fn plan<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Plan {
    let files: Vec<(&str, &str)> = files.into_iter().collect();
    let built: Vec<(&str, &str)> = files.iter().copied().filter(|(n, _)| !is_test(n)).collect();
    let mut main = String::new();
    if let [(_, text)] = built.as_slice() {
        main.push_str(text);
    } else {
        for (name, text) in &built {
            push_file(&mut main, name, text);
        }
    }
    let shared: Vec<(&str, &str)> = built
        .iter()
        .copied()
        .filter(|(_, t)| has_program(t) == Some(false))
        .collect();
    let tests = files
        .iter()
        .filter(|(n, _)| is_test(n))
        .map(|(name, text)| {
            let mut src = String::new();
            push_file(&mut src, name, text);
            for (n, t) in &shared {
                push_file(&mut src, n, t);
            }
            let stated = first_comment(text);
            TestSrc {
                file: (*name).to_string(),
                src,
                stated: stated.is_some(),
                requirement: stated
                    .unwrap_or_else(|| words(name.trim_end_matches(".st").trim_start_matches(TEST_PREFIX))),
            }
        })
        .collect();
    Plan { main, tests }
}

/// How a test came out.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// `DONE` went TRUE with no `ASSERT` failed.
    Passed,
    /// An `ASSERT` failed: which (its name as words, and its line's comment
    /// if any), when, and what the test held of the program's ports then
    /// (`name=value`, the inputs it drove and the outputs it had read, one
    /// scan old; empty when it shares none).
    Failed {
        check: String,
        at: Duration,
        seen: String,
    },
    /// `DONE` never went TRUE within the limit.
    NeverDone,
    /// The test holds no `ASSERT`: it checks nothing.
    NoAssert,
    /// `DONE` went TRUE but a declared `ASSERT` was never called (its name
    /// as words): that check never ran.
    NeverChecked { check: String },
    /// The test neither reads an output nor drives an input of the program
    /// under test, so whatever it asserts says nothing about the program.
    ChecksNothing,
    /// The test declares a port of the program in a section that does not
    /// connect it: a program input anywhere but the test's `VAR_OUTPUT`, a
    /// program output anywhere but its `VAR_INPUT`. `fix` says where it goes.
    Misdeclared {
        port: String,
        section: String,
        fix: String,
    },
    /// The test assigns one input of the program more than once at the top
    /// level of its body: a script written as if statements waited, where a
    /// scan runs them all at once and the last value wins.
    DrivenTwice { port: String, times: usize },
    /// The test reads the program's own `done` and declares no `TEST_DONE`
    /// output, so it would end the moment the program's `done` came on,
    /// whatever it had checked by then.
    EndsOnProgramDone,
    /// No test yet: the file states its requirement but declares no
    /// `ASSERT` (or its PROGRAM's body is empty). Neither green nor red.
    Untested,
    /// The test does not compile.
    TestDoesNotCompile(Vec<Diagnostic>),
    /// The program under test does not compile.
    ProgramDoesNotCompile(Vec<Diagnostic>),
    /// A port both share has different types (test's, program's).
    PortTypes {
        port: String,
        test: String,
        program: String,
    },
    /// A scan faulted (whose, and the fault).
    Faulted { test: bool, fault: String },
}

/// One test's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub file: String,
    pub requirement: String,
    pub outcome: Outcome,
    /// Scans run.
    pub scans: u32,
    /// Simulated time at the end.
    pub elapsed: Duration,
}

impl Verdict {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.outcome == Outcome::Passed
    }

    /// Whether nothing checks the requirement yet ([`Outcome::Untested`]).
    #[must_use]
    pub fn untested(&self) -> bool {
        self.outcome == Outcome::Untested
    }

    /// Whether it refuses a deploy: neither green nor untested.
    #[must_use]
    pub fn red(&self) -> bool {
        !self.passed() && !self.untested()
    }

    /// `passed`, `failed` or `untested`.
    #[must_use]
    pub fn state(&self) -> &'static str {
        match self.outcome {
            Outcome::Passed => "passed",
            Outcome::Untested => "untested",
            _ => "failed",
        }
    }

    /// Why it is red, in one line (empty when green; what is missing when
    /// untested).
    #[must_use]
    pub fn why(&self) -> String {
        match &self.outcome {
            Outcome::Passed => String::new(),
            Outcome::Untested => "no test yet: nothing checks this requirement".into(),
            Outcome::Failed { check, at, seen } if at.is_zero() => format!(
                "{check} failed on the first scan (0.00 s), before the program had run once: what a test reads is one scan old, so check from the second scan on{seen}"
            ),
            Outcome::Failed { check, at, seen } => format!("{check} failed at {}{seen}", secs(*at)),
            Outcome::NeverDone => format!(
                "never set done within {}: set done := TRUE at the last check",
                secs(self.elapsed)
            ),
            Outcome::NoAssert => "has no ASSERT: it checks nothing".into(),
            Outcome::NeverChecked { check } => format!("{check} never checked: call every ASSERT you declare"),
            Outcome::ChecksNothing => {
                "reads no output and drives no input of the program, so it checks nothing: declare the program's inputs in the test's VAR_OUTPUT and its outputs in the test's VAR_INPUT".into()
            }
            Outcome::Misdeclared { port, section, fix } => format!("declares {port} in {section}: {fix}"),
            Outcome::EndsOnProgramDone => "reads the program's own output done, so it would end the moment that came on: declare test_done : BOOL in the test's VAR_OUTPUT and set it TRUE at the last check".into(),
            Outcome::DrivenTwice { port, times } => format!(
                "assigns {port} {times} times in one scan: a test runs once per scan like the program, nothing waits between its statements and the last value wins, so drive one state per scan (count scans with n := n + 1 and switch on n)"
            ),
            Outcome::TestDoesNotCompile(d) => format!("the test does not compile: {}", first(d)),
            Outcome::ProgramDoesNotCompile(d) => format!("the program does not compile: {}", first(d)),
            Outcome::PortTypes { port, test, program } => {
                format!("{port} is {test} in the test and {program} in the program")
            }
            Outcome::Faulted { test: true, fault } => format!("the test faulted: {fault}"),
            Outcome::Faulted { test: false, fault } => format!("the program faulted: {fault}"),
        }
    }
}

fn first(d: &[Diagnostic]) -> String {
    d.first().map_or_else(String::new, ToString::to_string)
}

fn secs(d: Duration) -> String {
    format!("{:.2} s", d.as_secs_f64())
}

/// The `ASSERT` instances a test program declares at its top level: the
/// variable's path and the words for it (its name, plus the comment on
/// its declaration line if there is one).
fn asserts(src: &str) -> Vec<(String, String)> {
    let Ok(unit) = ast::parse(src) else {
        return Vec::new();
    };
    let Some(prog) = unit.pous.iter().find(|p| matches!(p.kind, PouKind::Program)) else {
        return Vec::new();
    };
    let lines: Vec<&str> = src.lines().collect();
    prog.vars
        .iter()
        .filter(|v| {
            v.section == Section::Var
                && matches!(&v.ty, TypeRef::Named(n, _) if n.eq_ignore_ascii_case(ASSERT))
        })
        .map(|v| {
            let line = usize::try_from(v.pos.line).unwrap_or(0).saturating_sub(1);
            let line = lines.get(line).copied().unwrap_or("");
            let mut text = words(&v.name);
            if let Some(c) = first_comment(line) {
                let _ = write!(text, " ({c})");
            }
            (v.name.clone(), text)
        })
        .collect()
}

/// A port of the program declared in the test's wrong section
/// ([`Outcome::Misdeclared`]), the first one.
fn misdeclared(src: &str, ports: &[Port]) -> Option<Outcome> {
    let unit = ast::parse(src).ok()?;
    let prog = unit.pous.iter().find(|p| matches!(p.kind, PouKind::Program))?;
    for v in &prog.vars {
        let Some(p) = ports.iter().find(|p| p.name.eq_ignore_ascii_case(&v.name)) else {
            continue;
        };
        let right = match p.dir {
            PortDir::Input => Section::Output,
            PortDir::Output => Section::Input,
        };
        if v.section == right {
            continue;
        }
        let section = match v.section {
            Section::Var => "VAR",
            Section::Temp => "VAR_TEMP",
            Section::Input => "VAR_INPUT",
            Section::Output => "VAR_OUTPUT",
            Section::InOut => "VAR_IN_OUT",
        };
        let fix = match p.dir {
            PortDir::Input => format!(
                "the program's input {} is driven by declaring it in the test's VAR_OUTPUT",
                words(&p.name)
            ),
            PortDir::Output if p.name == DONE => {
                "the program's output done is read by declaring it in the test's VAR_INPUT, and the test then ends on an output test_done of its own".to_string()
            }
            PortDir::Output => format!(
                "the program's output {} is read by declaring it in the test's VAR_INPUT",
                words(&p.name)
            ),
        };
        return Some(Outcome::Misdeclared {
            port: words(&v.name),
            section: section.into(),
            fix,
        });
    }
    None
}

/// An input of the program the test assigns more than once at the top
/// level of its body ([`Outcome::DrivenTwice`]), the first one.
fn driven_twice(src: &str, drives: &[String]) -> Option<Outcome> {
    let unit = ast::parse(src).ok()?;
    let prog = unit.pous.iter().find(|p| matches!(p.kind, PouKind::Program))?;
    for name in drives {
        let times = prog
            .body
            .iter()
            .filter(|s| {
                matches!(&s.kind, ast::StmtKind::Assign(target, _)
                    if matches!(&target.kind, ast::ExprKind::Var(n) if n.eq_ignore_ascii_case(name)))
            })
            .count();
        if times > 1 {
            return Some(Outcome::DrivenTwice {
                port: words(name),
                times,
            });
        }
    }
    None
}

/// `name=value` for each of `names` as the test holds it: ` (a=TRUE, b=0)`,
/// empty when there are none.
fn seen(t: &crate::Program, names: &[&String]) -> String {
    let mut out = String::new();
    for name in names {
        if let Some(v) = t.get(name) {
            out.push_str(if out.is_empty() { " (" } else { ", " });
            let _ = write!(out, "{}={v}", words(name));
        }
    }
    if !out.is_empty() {
        out.push(')');
    }
    out
}

/// Whether the first PROGRAM in `src` has an empty body.
fn empty_body(src: &str) -> bool {
    ast::parse(src).is_ok_and(|u| {
        u.pous
            .iter()
            .find(|p| matches!(p.kind, PouKind::Program))
            .is_some_and(|p| p.body.is_empty())
    })
}

/// Runs one test against `main` (the program under test's source).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn run(main: &str, test: &TestSrc, settings: Settings) -> Verdict {
    let verdict = |outcome, scans, elapsed| Verdict {
        file: test.file.clone(),
        requirement: test.requirement.clone(),
        outcome,
        scans,
        elapsed,
    };
    let zero = Duration::ZERO;
    let checks = asserts(&test.src);
    let mut t = match compile(&test.src, settings.sizes) {
        Ok(p) => p,
        Err(d) => return verdict(Outcome::TestDoesNotCompile(d), 0, zero),
    };
    let mut m = match compile(main, settings.sizes) {
        Ok(p) => p,
        Err(d) => return verdict(Outcome::ProgramDoesNotCompile(d), 0, zero),
    };
    if checks.is_empty() {
        let none = if test.stated || empty_body(&test.src) {
            Outcome::Untested
        } else {
            Outcome::NoAssert
        };
        return verdict(none, 0, zero);
    }
    let program_ports = m.ports();
    if let Some(o) = misdeclared(&test.src, &program_ports) {
        return verdict(o, 0, zero);
    }
    // The test drives these inputs of the program; it reads those outputs.
    let mut drives = Vec::new();
    let mut reads = Vec::new();
    for tp in t.ports() {
        for mp in &program_ports {
            if tp.name != mp.name {
                continue;
            }
            if tp.ty != mp.ty {
                return verdict(
                    Outcome::PortTypes {
                        port: words(&tp.name),
                        test: format!("{:?}", tp.ty).to_ascii_uppercase(),
                        program: format!("{:?}", mp.ty).to_ascii_uppercase(),
                    },
                    0,
                    zero,
                );
            }
            match (tp.dir, mp.dir) {
                (PortDir::Output, PortDir::Input) => drives.push(tp.name.clone()),
                (PortDir::Input, PortDir::Output) => reads.push(tp.name.clone()),
                _ => {}
            }
        }
    }
    if drives.is_empty() && reads.is_empty() {
        return verdict(Outcome::ChecksNothing, 0, zero);
    }
    if let Some(o) = driven_twice(&test.src, &drives) {
        return verdict(o, 0, zero);
    }
    let shared: Vec<&String> = drives.iter().chain(reads.iter()).collect();
    let end = if t
        .ports()
        .iter()
        .any(|p| p.name == TEST_DONE && p.dir == PortDir::Output)
    {
        TEST_DONE
    } else if reads.iter().any(|r| r == DONE) {
        return verdict(Outcome::EndsOnProgramDone, 0, zero);
    } else {
        DONE
    };
    let period = settings.period.max(Duration::from_millis(1));
    let max = u32::try_from(settings.limit.as_nanos() / period.as_nanos().max(1)).unwrap_or(u32::MAX);
    let mut ti = Image::new(settings.sizes);
    let mut mi = Image::new(settings.sizes);
    let mut scans = 0u32;
    let mut now = zero;
    while scans <= max {
        now = period * scans;
        if let Err(f) = t.scan_at(&mut ti, now) {
            let fault = f.to_string();
            return verdict(Outcome::Faulted { test: true, fault }, scans, now);
        }
        for name in &drives {
            if let Some(v) = t.get(name) {
                let _ = m.set_input(name, v);
            }
        }
        if let Err(f) = m.scan_at(&mut mi, now) {
            let fault = f.to_string();
            return verdict(Outcome::Faulted { test: false, fault }, scans, now);
        }
        for name in &reads {
            if let Some(v) = m.get(name) {
                let _ = t.set_input(name, v);
            }
        }
        scans += 1;
        for (path, check) in &checks {
            if t.get(&format!("{path}.FAILED")) == Some(Value::Bool(true)) {
                let at = match t.get(&format!("{path}.FAILED_AT")) {
                    Some(Value::Int(ns)) => Duration::from_nanos(u64::try_from(ns).unwrap_or(0)),
                    _ => now,
                };
                let check = check.clone();
                let seen = seen(&t, &shared);
                return verdict(Outcome::Failed { check, at, seen }, scans, now);
            }
        }
        if t.get(end) == Some(Value::Bool(true)) {
            if let Some((_, check)) = checks
                .iter()
                .find(|(path, _)| t.get(&format!("{path}.CHECKED")) != Some(Value::Bool(true)))
            {
                let check = check.clone();
                return verdict(Outcome::NeverChecked { check }, scans, now);
            }
            return verdict(Outcome::Passed, scans, now);
        }
    }
    verdict(Outcome::NeverDone, scans, now)
}

/// Every test of the plan, in file order.
#[must_use]
pub fn run_all(plan: &Plan, settings: Settings) -> Vec<Verdict> {
    plan.tests.iter().map(|t| run(&plan.main, t, settings)).collect()
}

/// The red verdicts' one-line reasons, for a refusal: `test_lamp: lamp on
/// in time failed at 2.00 s`. An untested one is not red.
#[must_use]
pub fn red(verdicts: &[Verdict]) -> Vec<String> {
    verdicts
        .iter()
        .filter(|v| v.red())
        .map(|v| format!("{}: {}", v.file.trim_end_matches(".st"), v.why()))
        .collect()
}

/// Longest test name ([`name_ok`]): `test_NAME.st` then fits a project's
/// 64-character file stem with room to spare.
pub const NAME_MAX: usize = 48;

/// Whether `name` is a requirement test's name (`test_NAME.st` is its
/// file): snake case, 1 to [`NAME_MAX`] of `a-z 0-9 _`, starting with a
/// letter.
#[must_use]
pub fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=NAME_MAX).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// A test name made from a requirement's words: lower case `a-z 0-9`,
/// every run of anything else one `_`, trimmed, at most `max` (and
/// [`NAME_MAX`]) cut at a word where it can be, starting with a letter
/// (`r_` in front of a digit). `requirement` when nothing survives.
/// Always [`name_ok`].
#[must_use]
pub fn slug(text: &str, max: usize) -> String {
    let max = max.clamp(1, NAME_MAX);
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    if out.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        out.insert_str(0, "r_");
    }
    if out.len() > max {
        let cut = out[..=max].rfind('_').filter(|&k| k > 0).unwrap_or(max);
        out.truncate(cut);
    }
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() {
        "requirement".chars().take(max).collect()
    } else {
        out
    }
}

/// `text` safe inside one `(* ... *)` comment: a space goes between any
/// `(` and `*` or `*` and `)` side by side, so it can neither close the
/// comment nor open another; whitespace runs (newlines included) become
/// one space.
#[must_use]
pub fn neutralise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in one_line(text).chars() {
        let last = out.chars().last();
        if (last == Some('*') && c == ')') || (last == Some('(') && c == '*') {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The first line of a requirement test: the requirement as a comment,
/// [`neutralise`]d, so the test's first comment reads it back.
#[must_use]
pub fn header(requirement: &str) -> String {
    format!("(* {} *)\n", neutralise(requirement))
}

/// The requirement a test file states, as [`plan`] reads it (its first
/// comment, one line), if it states one.
#[must_use]
pub fn stated(text: &str) -> Option<String> {
    first_comment(text)
}

/// A test file with only its requirement, nothing that checks it: what
/// runs as [`Outcome::Untested`].
#[must_use]
pub fn placeholder(name: &str, requirement: &str) -> String {
    format!(
        "{}PROGRAM {TEST_PREFIX}{name}\nEND_PROGRAM\n",
        header(requirement)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };

    const LAMP: &str = "PROGRAM lamp
VAR_INPUT start_pb : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR t : TON; END_VAR
t(IN := start_pb, PT := T#1s);
lamp := t.Q;
END_PROGRAM
";

    const TEST_OK: &str = "(* The lamp is on within 2 s of start_pb. *)
PROGRAM test_lamp
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR t : TON; lamp_on_in_time : ASSERT; (* on within 2 s *) END_VAR
start_pb := TRUE;
t(IN := TRUE, PT := T#2s);
IF t.Q THEN
  lamp_on_in_time(COND := lamp);
  done := TRUE;
END_IF;
END_PROGRAM
";

    fn settings() -> Settings {
        Settings::new(SIZES)
    }

    fn one(main: &str, test: &str) -> Verdict {
        let plan = plan([("main.st", main), ("test_x.st", test)]);
        assert_eq!(plan.tests.len(), 1);
        run(&plan.main, &plan.tests[0], settings())
    }

    /// A program with a `done` output of its own (the Counter example):
    /// the test reads it as an input and ends on `test_done` instead.
    #[test]
    fn a_program_with_its_own_done_is_tested_with_test_done() {
        const COUNTER: &str = "PROGRAM counter
VAR_INPUT pulse : BOOL; END_VAR
VAR_OUTPUT done : BOOL; END_VAR
VAR c : CTU; END_VAR
c(CU := pulse, R := FALSE, PV := 3);
done := c.Q;
END_PROGRAM
";
        let test = "(* done comes on at the third pulse. *)
PROGRAM test_x
VAR_INPUT done : BOOL; END_VAR
VAR_OUTPUT pulse : BOOL; test_done : BOOL; END_VAR
VAR n : INT; on_at_3 : ASSERT; END_VAR
n := n + 1;
pulse := (n MOD 2) = 1;
IF n > 8 THEN on_at_3(COND := done); test_done := TRUE; END_IF;
END_PROGRAM
";
        assert_eq!(one(COUNTER, test).outcome, Outcome::Passed);
        // It checks what it says: two pulses only, and it is red.
        let short = test
            .replace("n > 8", "n > 4")
            .replace("(n MOD 2) = 1", "(n MOD 2) = 1 AND n < 4");
        assert!(one(COUNTER, &short).red());
        // A test that declares no test_done still ends on done.
        assert_eq!(one(LAMP, TEST_OK).outcome, Outcome::Passed);
    }

    #[test]
    fn test_file_names() {
        assert!(is_test("test_lamp.st"));
        assert!(is_test("TEST_Lamp.ST"));
        assert!(is_test("test_.st"));
        assert!(!is_test("test.st"));
        assert!(!is_test("tests_lamp.st"));
        assert!(!is_test("main.st"));
        assert!(!is_test("test_lamp.xml"));
        assert!(!is_test(""));
        assert!(!is_test("te"));
        assert!(!is_test("tést_x.st"));
        assert!(!is_test("test_é"));
    }

    #[test]
    fn a_passing_test_is_green_and_timed() {
        let v = one(LAMP, TEST_OK);
        assert_eq!(v.outcome, Outcome::Passed, "{}", v.why());
        assert_eq!(v.requirement, "The lamp is on within 2 s of start_pb.");
        assert_eq!(v.file, "test_x.st");
        assert!(v.passed() && v.why().is_empty());
        // Done one scan after the 2 s timer: 10 ms scans.
        assert!((200..=202).contains(&v.scans), "{}", v.scans);
    }

    #[test]
    fn a_failing_test_names_the_check_in_words_and_when() {
        let broken = LAMP.replace("lamp := t.Q;", "lamp := FALSE;");
        let v = one(&broken, TEST_OK);
        match &v.outcome {
            Outcome::Failed { check, at, seen } => {
                assert_eq!(check, "lamp on in time (on within 2 s)");
                assert_eq!(seen, " (start pb=TRUE, lamp=FALSE)");
                assert!(
                    (Duration::from_millis(1990)..=Duration::from_millis(2010)).contains(at),
                    "{at:?}"
                );
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(
            v.why(),
            "lamp on in time (on within 2 s) failed at 2.00 s (start pb=TRUE, lamp=FALSE)"
        );
        assert_eq!(
            red(std::slice::from_ref(&v)),
            vec!["test_x: lamp on in time (on within 2 s) failed at 2.00 s (start pb=TRUE, lamp=FALSE)"]
        );
    }

    #[test]
    fn a_slow_program_fails_the_timing_requirement() {
        let slow = LAMP.replace("T#1s", "T#3s");
        let v = one(&slow, TEST_OK);
        assert!(matches!(v.outcome, Outcome::Failed { .. }), "{}", v.why());
    }

    #[test]
    fn a_test_that_never_finishes_is_red() {
        let never = TEST_OK.replace("done := TRUE;", "");
        let plan = plan([("main.st", LAMP), ("test_x.st", never.as_str())]);
        let v = run(
            &plan.main,
            &plan.tests[0],
            settings().limit(Duration::from_secs(5)),
        );
        assert_eq!(v.outcome, Outcome::NeverDone);
        assert_eq!(
            v.why(),
            "never set done within 5.00 s: set done := TRUE at the last check"
        );
        assert_eq!(v.scans, 501);
    }

    /// A scenario that passes while checking nothing: an ASSERT declared
    /// and never called, or a test that touches none of the program's ports.
    #[test]
    fn a_test_that_checks_nothing_is_red() {
        let never_called = "(* The lamp is on. *)
PROGRAM test_x
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR lamp_ok : ASSERT; END_VAR
start_pb := TRUE;
done := TRUE;
END_PROGRAM
";
        let v = one(LAMP, never_called);
        assert_eq!(
            v.outcome,
            Outcome::NeverChecked {
                check: "lamp ok".into()
            }
        );
        assert_eq!(v.why(), "lamp ok never checked: call every ASSERT you declare");
        assert!(v.red());
        // Called, but only on a branch that never runs: still never checked.
        let branch = never_called.replace(
            "done := TRUE;",
            "IF FALSE THEN lamp_ok(COND := lamp); END_IF;\ndone := TRUE;",
        );
        assert!(matches!(one(LAMP, &branch).outcome, Outcome::NeverChecked { .. }));
        // Called with a constant, touching no port of the program.
        let no_ports = "(* The lamp is on. *)
PROGRAM test_x
VAR_OUTPUT done : BOOL; END_VAR
VAR lamp_ok : ASSERT; END_VAR
lamp_ok(COND := TRUE);
done := TRUE;
END_PROGRAM
";
        let v = one(LAMP, no_ports);
        assert_eq!(v.outcome, Outcome::ChecksNothing);
        assert!(v.why().contains("checks nothing"), "{}", v.why());
        // Called and sharing a port: green.
        let called = never_called.replace("done := TRUE;", "lamp_ok(COND := TRUE);\ndone := TRUE;");
        assert_eq!(one(LAMP, &called).outcome, Outcome::Passed);
        // Two asserts, one never called: the one that did not run is named.
        let two = TEST_OK.replace(
            "lamp_on_in_time : ASSERT;",
            "lamp_on_in_time : ASSERT; spare_check : ASSERT;",
        );
        assert_eq!(
            one(LAMP, &two).outcome,
            Outcome::NeverChecked {
                check: "spare check (on within 2 s)".into()
            }
        );
        assert_eq!(one(LAMP, TEST_OK).outcome, Outcome::Passed);
    }

    /// A program port in the wrong section of the test connects nothing:
    /// the verdict names the port, the section and where it goes, before
    /// anything runs (the model-written scenarios of 2026-11-12 read an
    /// output from a plain VAR and failed "at 0.00 s" with no clue why).
    #[test]
    fn a_program_port_in_the_wrong_section_is_named_with_its_fix() {
        let output_in_var = "(* The lamp is on. *)
PROGRAM test_x
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR lamp : BOOL; n : INT; lamp_ok : ASSERT; END_VAR
start_pb := TRUE;
n := n + 1;
IF n > 150 THEN lamp_ok(COND := lamp); done := TRUE; END_IF;
END_PROGRAM
";
        let v = one(LAMP, output_in_var);
        assert_eq!(
            v.why(),
            "declares lamp in VAR: the program's output lamp is read by declaring it in the test's VAR_INPUT"
        );
        assert!(v.red());
        let input_in_var = output_in_var
            .replace(
                "VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR",
                "VAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT done : BOOL; END_VAR",
            )
            .replace("VAR lamp : BOOL; n : INT;", "VAR start_pb : BOOL; n : INT;");
        assert_eq!(
            one(LAMP, &input_in_var).why(),
            "declares start pb in VAR: the program's input start pb is driven by declaring it in the test's VAR_OUTPUT"
        );
        // A program with its own done: the test's done output is the
        // program's, misread; the fix names test_done.
        let counter = "PROGRAM counter
VAR_INPUT pulse : BOOL; END_VAR
VAR_OUTPUT done : BOOL; END_VAR
VAR c : CTU; END_VAR
c(CU := pulse, R := FALSE, PV := 3);
done := c.Q;
END_PROGRAM
";
        let done_out = "(* done comes on. *)
PROGRAM test_x
VAR_OUTPUT pulse : BOOL; done : BOOL; END_VAR
VAR n : INT; on : ASSERT; END_VAR
n := n + 1;
pulse := (n MOD 2) = 1;
IF n > 8 THEN on(COND := TRUE); done := TRUE; END_IF;
END_PROGRAM
";
        let v = one(counter, done_out);
        assert!(matches!(v.outcome, Outcome::Misdeclared { .. }), "{v:?}");
        assert!(v.why().contains("test_done"), "{}", v.why());
        // It reads the program's done but ends on a flag of another name:
        // it would stop when the program's done came on (2026-11-12, the
        // model's `done_flag`).
        let other_flag = "(* done comes on at the third pulse. *)
PROGRAM test_x
VAR_INPUT done : BOOL; END_VAR
VAR_OUTPUT pulse : BOOL; done_flag : BOOL; END_VAR
VAR n : INT; on_at_3 : ASSERT; END_VAR
n := n + 1;
pulse := (n MOD 2) = 1;
IF n = 20 THEN on_at_3(COND := done); done_flag := TRUE; END_IF;
END_PROGRAM
";
        let v = one(counter, other_flag);
        assert_eq!(v.outcome, Outcome::EndsOnProgramDone);
        assert!(v.why().contains("declare test_done : BOOL"), "{}", v.why());
        assert_eq!(
            one(counter, &other_flag.replace("done_flag", "test_done")).outcome,
            Outcome::Passed
        );
    }

    /// A script that sets an input, checks, sets it again in one scan:
    /// named as such, with the scan-counter way out.
    #[test]
    fn an_input_assigned_twice_in_one_scan_is_named() {
        let script = "(* The lamp follows start. *)
PROGRAM test_x
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR off : ASSERT; on : ASSERT; END_VAR
start_pb := FALSE;
off(COND := NOT lamp);
start_pb := TRUE;
on(COND := lamp);
done := TRUE;
END_PROGRAM
";
        let v = one(LAMP, script);
        assert_eq!(
            v.outcome,
            Outcome::DrivenTwice {
                port: "start pb".into(),
                times: 2
            }
        );
        assert!(
            v.why().starts_with("assigns start pb 2 times in one scan"),
            "{}",
            v.why()
        );
        // One assignment per branch is one state per scan: not a script.
        let branches = "(* The lamp follows start. *)
PROGRAM test_x
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR n : INT; on : ASSERT; END_VAR
n := n + 1;
IF n < 5 THEN start_pb := FALSE; ELSE start_pb := TRUE; END_IF;
IF n > 150 THEN on(COND := lamp); done := TRUE; END_IF;
END_PROGRAM
";
        assert_eq!(one(LAMP, branches).outcome, Outcome::Passed);
    }

    /// A check on the first scan sees nothing of the program yet: the
    /// verdict says so and shows what the test held.
    #[test]
    fn a_failure_on_the_first_scan_says_the_program_had_not_run() {
        let eager = "(* The lamp is off at first. *)
PROGRAM test_x
VAR_INPUT lamp : BOOL; END_VAR
VAR_OUTPUT start_pb : BOOL; done : BOOL; END_VAR
VAR n : INT; on_at_once : ASSERT; END_VAR
n := n + 1;
start_pb := TRUE;
on_at_once(COND := lamp);
done := n > 2;
END_PROGRAM
";
        let v = one(LAMP, eager);
        assert_eq!(
            v.why(),
            "on at once failed on the first scan (0.00 s), before the program had run once: what a test reads is one scan old, so check from the second scan on (start pb=TRUE, lamp=FALSE)"
        );
    }

    #[test]
    fn a_test_without_assert_is_red() {
        let none = "PROGRAM test_x VAR_OUTPUT done : BOOL; END_VAR done := TRUE; END_PROGRAM";
        let v = one(LAMP, none);
        assert_eq!(v.outcome, Outcome::NoAssert);
        assert_eq!(v.requirement, "x", "no comment: the file's name as words");
    }

    #[test]
    fn a_stated_requirement_with_nothing_checking_it_is_untested_not_red() {
        // The placeholder the box writes: the requirement, an empty PROGRAM.
        let v = one(LAMP, &placeholder("x", "The lamp is on within 2 s."));
        assert_eq!(v.outcome, Outcome::Untested, "{}", v.why());
        assert_eq!((v.state(), v.red(), v.passed()), ("untested", false, false));
        assert_eq!(v.requirement, "The lamp is on within 2 s.");
        assert_eq!(v.why(), "no test yet: nothing checks this requirement");
        assert!(red(std::slice::from_ref(&v)).is_empty(), "never refuses a deploy");
        // A body but no ASSERT, with its requirement stated: untested too.
        let v = one(
            LAMP,
            "(* lamp on *)\nPROGRAM test_x VAR_OUTPUT done : BOOL; END_VAR done := TRUE; END_PROGRAM",
        );
        assert_eq!(v.state(), "untested");
        // An empty body with no comment: untested (nothing written yet).
        assert_eq!(one(LAMP, "PROGRAM test_x END_PROGRAM").state(), "untested");
        // Still red: a stated test that does not compile, or that fails.
        let v = one(LAMP, "(* lamp on *)\nPROGRAM test_x x := ; END_PROGRAM");
        assert_eq!(v.state(), "failed");
        assert!(v.red());
        let broken = LAMP.replace("lamp := t.Q;", "lamp := FALSE;");
        assert_eq!(one(&broken, TEST_OK).state(), "failed");
        assert_eq!(one(LAMP, TEST_OK).state(), "passed");
        // A mix: only the failing one is red.
        let plan = plan([
            ("main.st", broken.as_str()),
            ("test_a.st", TEST_OK),
            ("test_b.st", placeholder("b", "b").as_str()),
        ]);
        let v = run_all(&plan, settings());
        assert_eq!(red(&v).len(), 1, "{:?}", red(&v));
        assert!(red(&v)[0].starts_with("test_a: "));
    }

    #[test]
    fn names_and_slugs() {
        for ok in ["a", "lamp_on", "x1", &"a".repeat(NAME_MAX)] {
            assert!(name_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "_a",
            "1a",
            "Lamp",
            "a-b",
            "../x",
            "a/b",
            "a.st",
            "a b",
            "é",
            &"a".repeat(NAME_MAX + 1),
        ] {
            assert!(!name_ok(bad), "{bad}");
        }
        assert_eq!(
            slug("The lamp is on within 2 s of start_pb.", 48),
            "the_lamp_is_on_within_2_s_of_start_pb"
        );
        assert_eq!(slug("  ** ", 48), "requirement");
        assert_eq!(slug("2 lamps", 48), "r_2_lamps");
        assert_eq!(slug("alpha beta gamma", 12), "alpha_beta");
        assert_eq!(slug("../../etc/passwd", 48), "etc_passwd");
    }

    #[test]
    fn the_header_cannot_close_its_comment() {
        let h = header("x *) PROGRAM evil END_PROGRAM (* y");
        assert_eq!(h, "(* x * ) PROGRAM evil END_PROGRAM ( * y *)\n");
        assert_eq!(
            stated(&h).as_deref(),
            Some("x * ) PROGRAM evil END_PROGRAM ( * y")
        );
        let file = placeholder("x", "a *)(*) b\nc");
        let v = one(LAMP, &file);
        assert_eq!(v.state(), "untested", "{}", v.why());
        assert_eq!(v.requirement, "a * )( * ) b c");
    }

    proptest::proptest! {
        #[test]
        fn any_slug_is_a_name(text in "\\PC{0,200}", max in 0usize..80) {
            let s = slug(&text, max);
            proptest::prop_assert!(name_ok(&s), "{:?} -> {:?}", text, s);
            proptest::prop_assert!(s.len() <= max.clamp(1, NAME_MAX));
        }

        #[test]
        fn any_requirement_reads_back_and_never_escapes_its_comment(text in "\\PC{0,200}") {
            let n = neutralise(&text);
            proptest::prop_assert!(!n.contains("*)") && !n.contains("(*"), "{:?}", n);
            let file = placeholder("x", &text);
            // The comment ends where the header line ends, nowhere before.
            let first = file.lines().next().unwrap();
            proptest::prop_assert_eq!(first.find("*)"), Some(first.len() - 2));
            if n.is_empty() {
                proptest::prop_assert_eq!(stated(&file), None);
            } else {
                proptest::prop_assert_eq!(stated(&file), Some(n.clone()));
            }
            let plan = plan([("main.st", LAMP), ("test_x.st", file.as_str())]);
            let v = run(&plan.main, &plan.tests[0], settings());
            proptest::prop_assert_eq!(v.state(), "untested", "{}", v.why());
        }
    }

    #[test]
    fn compile_errors_on_either_side_are_red_and_say_which() {
        let v = one(
            LAMP,
            "PROGRAM test_x VAR a : ASSERT; END_VAR a(COND := ; END_PROGRAM",
        );
        assert!(matches!(v.outcome, Outcome::TestDoesNotCompile(_)), "{}", v.why());
        assert!(v.why().starts_with("the test does not compile: "));
        let v = one(
            "PROGRAM lamp VAR_OUTPUT lamp : BOOL; END_VAR lamp := ; END_PROGRAM",
            TEST_OK,
        );
        assert!(matches!(v.outcome, Outcome::ProgramDoesNotCompile(_)));
    }

    #[test]
    fn a_shared_port_of_different_types_is_red() {
        let int_lamp = LAMP
            .replace("VAR_OUTPUT lamp : BOOL;", "VAR_OUTPUT lamp : INT;")
            .replace("lamp := t.Q;", "lamp := 1;");
        let v = one(&int_lamp, TEST_OK);
        assert_eq!(
            v.outcome,
            Outcome::PortTypes {
                port: "lamp".into(),
                test: "BOOL".into(),
                program: "INT".into()
            }
        );
    }

    #[test]
    fn a_fault_in_the_program_is_red_and_says_so() {
        let div = "PROGRAM lamp
VAR_INPUT start_pb : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : INT := 0; z : INT; END_VAR
IF start_pb THEN z := 10 / n; END_IF;
lamp := TRUE;
END_PROGRAM
";
        let v = one(div, TEST_OK);
        assert!(
            matches!(v.outcome, Outcome::Faulted { test: false, .. }),
            "{}",
            v.why()
        );
        assert!(v.why().starts_with("the program faulted: "));
        let div_test = TEST_OK.replace(
            "start_pb := TRUE;",
            "start_pb := TRUE; IF TRUE THEN start_pb := 1 / 0 = 0; END_IF;",
        );
        let v = one(LAMP, &div_test);
        assert!(
            matches!(
                v.outcome,
                Outcome::Faulted { test: true, .. } | Outcome::TestDoesNotCompile(_)
            ),
            "{}",
            v.why()
        );
    }

    #[test]
    fn the_plan_builds_main_without_tests_and_tests_with_shared_blocks() {
        let block = "FUNCTION_BLOCK dbl VAR_INPUT x : INT; END_VAR VAR_OUTPUT y : INT; END_VAR y := 2 * x; END_FUNCTION_BLOCK\n";
        let main = "PROGRAM p VAR_INPUT a : INT; END_VAR VAR_OUTPUT b : INT; END_VAR VAR d : dbl; END_VAR d(x := a); b := d.y; END_PROGRAM\n";
        let test = "(* b is twice a *)\nPROGRAM test_p
VAR_INPUT b : INT; END_VAR
VAR_OUTPUT a : INT; done : BOOL; END_VAR
VAR n : INT; twice : ASSERT; d : dbl; END_VAR
a := 21;
n := n + 1;
IF n > 2 THEN d(x := a); twice(COND := b = d.y); done := TRUE; END_IF;
END_PROGRAM
";
        let plan = plan([("blocks.st", block), ("main.st", main), ("test_p.st", test)]);
        assert!(!plan.main.contains("PROGRAM test_p"));
        assert!(plan.main.contains("(* file blocks.st *)") && plan.main.contains("(* file main.st *)"));
        assert_eq!(plan.tests.len(), 1);
        assert!(
            plan.tests[0].src.contains("FUNCTION_BLOCK dbl"),
            "the shared block travels with the test"
        );
        assert!(
            !plan.tests[0].src.contains("PROGRAM p "),
            "the program under test does not"
        );
        let v = run_all(&plan, settings());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].outcome, Outcome::Passed, "{}", v[0].why());
        // One non-test file: the main is that file, verbatim.
        let single = super::plan([("main.st", main)]);
        assert_eq!(single.main, main);
        assert_eq!(single.tests, []);
    }

    #[test]
    fn garbage_never_panics() {
        let long = "x".repeat(1 << 16);
        for text in [
            "",
            "(*",
            "(* *)",
            "PROGRAM",
            "\u{0}\u{1}",
            long.as_str(),
            "(* a *) (* b *)",
            "PROGRAM test_x END_PROGRAM",
            "PROGRAM test_x VAR a : ASSERT; END_VAR END_PROGRAM",
        ] {
            let plan = plan([("main.st", text), ("test_x.st", text), ("test_y.st", LAMP)]);
            let _ = run_all(&plan, settings().limit(Duration::from_millis(50)));
        }
        let p = plan(std::iter::empty());
        assert!(p.tests.is_empty() && p.main.is_empty());
    }
}
