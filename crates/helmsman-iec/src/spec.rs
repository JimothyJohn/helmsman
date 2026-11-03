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
//! test ends when its `DONE` output goes TRUE; it fails when any of its
//! `ASSERT` instances has `FAILED`, when either program faults, when it
//! never sets `DONE` within the limit, or when it holds no `ASSERT` at all
//! (a test that checks nothing is not green). Deterministic: no wall
//! clock, no I/O.
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

#[cfg(not(feature = "std"))]
use crate::prelude::*;

use crate::ast::{self, PouKind, Section, TypeRef};
use crate::{Diagnostic, Image, ImageSizes, PortDir, Value, compile};

/// The file name prefix that makes a `.st` file a test: `test_NAME.st`.
pub const TEST_PREFIX: &str = "test_";

/// The output a test sets TRUE when it is done.
pub const DONE: &str = "DONE";

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
            TestSrc {
                file: (*name).to_string(),
                src,
                requirement: first_comment(text)
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
    /// if any), when.
    Failed { check: String, at: Duration },
    /// `DONE` never went TRUE within the limit.
    NeverDone,
    /// The test holds no `ASSERT`: it checks nothing.
    NoAssert,
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

    /// Why it is red, in one line (empty when green).
    #[must_use]
    pub fn why(&self) -> String {
        match &self.outcome {
            Outcome::Passed => String::new(),
            Outcome::Failed { check, at } => format!("{check} failed at {}", secs(*at)),
            Outcome::NeverDone => format!("never set DONE within {}", secs(self.elapsed)),
            Outcome::NoAssert => "has no ASSERT: it checks nothing".into(),
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
        return verdict(Outcome::NoAssert, 0, zero);
    }
    // The test drives these inputs of the program; it reads those outputs.
    let mut drives = Vec::new();
    let mut reads = Vec::new();
    for tp in t.ports() {
        for mp in m.ports() {
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
                return verdict(Outcome::Failed { check, at }, scans, now);
            }
        }
        if t.get(DONE) == Some(Value::Bool(true)) {
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
/// in time failed at 2.00 s`.
#[must_use]
pub fn red(verdicts: &[Verdict]) -> Vec<String> {
    verdicts
        .iter()
        .filter(|v| !v.passed())
        .map(|v| format!("{}: {}", v.file.trim_end_matches(".st"), v.why()))
        .collect()
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
            Outcome::Failed { check, at } => {
                assert_eq!(check, "lamp on in time (on within 2 s)");
                assert!(
                    (Duration::from_millis(1990)..=Duration::from_millis(2010)).contains(at),
                    "{at:?}"
                );
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(v.why(), "lamp on in time (on within 2 s) failed at 2.00 s");
        assert_eq!(
            red(std::slice::from_ref(&v)),
            vec!["test_x: lamp on in time (on within 2 s) failed at 2.00 s"]
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
        assert_eq!(v.why(), "never set DONE within 5.00 s");
        assert_eq!(v.scans, 501);
    }

    #[test]
    fn a_test_without_assert_is_red() {
        let none = "PROGRAM test_x VAR_OUTPUT done : BOOL; END_VAR done := TRUE; END_PROGRAM";
        let v = one(LAMP, none);
        assert_eq!(v.outcome, Outcome::NoAssert);
        assert_eq!(v.requirement, "x", "no comment: the file's name as words");
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
