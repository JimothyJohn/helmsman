//! The Agent's answer checked before the engineer sees it (Nick,
//! 2026-11-05, card `agent-scenarios`): when it gives code, it also writes
//! scenarios, requirement tests in the `helmsman_iec::spec` format, each in
//! a fenced block marked ```` ```st scenario ````. The page takes the code
//! into a copy of the project through the scope rules ([`crate::scope::apply`]),
//! adds the scenarios, compiles it, and runs the scenarios and the
//! project's own requirement tests ([`check`]). An answer that fails goes
//! back to the Agent with what failed, up to [`RETRIES`] times ([`next`]).
//! What the tests still say after that is a warning (Nick, 2026-11-15:
//! "just do automated testing and don't let random requirements get in the
//! way of the flow"): the code is written and the checks that did not pass
//! are named ([`Check::warnings`]). Only an answer that cannot be taken
//! at all is refused ([`Check::blocked`]): cut off, outside its scope,
//! not the whole file, or code that does not compile. Every scenario that
//! passed is kept as a requirement test, its first comment the requirement
//! in plain words; one that failed is not kept.
//!
//! **The whole file comes back** (Nick, 2026-11-05): code for a file, some
//! lines or a block is the whole open file with the edits made in place,
//! never a fragment ([`full_copy`]): no elision markers, every POU of the
//! file there with its header and its `END_` line, and for lines or a
//! block the landed file equal to the code (a fragment the scope rules
//! would splice in is refused). The selection still bounds what may
//! change: [`crate::scope::apply`] refuses a change outside it.
//!
//! **A test the instruction contradicts is replaced** (Nick, 2026-11-05):
//! a scenario marked ```` ```st scenario replaces test_NAME ```` takes
//! the place of that requirement test (kept scenarios are tests too, so
//! the next instruction that changes their behaviour could never pass
//! otherwise). It must pass like any scenario, every other test still
//! must, and the status line names it, old wording and new
//! ([`Check::replaced`]). A test is replaced only when it is red against
//! the new code: one that still passes is not contradicted, so it stays
//! and the replacement is refused.
//!
//! **A scenario must tell the change from no change** (red before, green
//! after): when the answer changes what the file does, at least one of
//! its scenarios must fail on the file as it was. Scenarios that pass on
//! the old file prove nothing about the change and go back to the Agent.
//! A change that keeps the behaviour (a rename, a tidy) says so in an
//! `Unchanged behaviour: <why>` line after its `Why:` line; the rule is
//! skipped then, every test and scenario must still pass, and the status
//! line calls it the Agent's claim.
//!
//! Plain data and text, so it is tested natively and runs in the page.

use std::collections::BTreeSet;

use helmsman_iec::spec::{self, Settings};

use crate::claude::{MAX_PROGRAM, Segment, extract_program, fences, segments};
use crate::scope::{Files, Scope, apply, pous};

/// How many times a failing answer goes back to the Agent.
pub const RETRIES: u8 = 2;
/// Most scenarios one answer may carry.
pub const MAX_SCENARIOS: usize = 8;

/// Whether a fence's info string marks a scenario (```` ```st scenario ````).
#[must_use]
pub fn is_scenario(info: &str) -> bool {
    info.split_whitespace()
        .any(|w| w.eq_ignore_ascii_case("scenario"))
}

/// Whether a fence is the list of requirements with no scenario
/// (```` ```requirements ````, [`crate::documents::untested`]): words, not
/// code.
#[must_use]
pub fn is_requirements(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("requirements"))
}

/// The test file a scenario fence replaces (```` ```st scenario
/// replaces test_lamp ````): `test_lamp.st`.
#[must_use]
pub fn replaces(info: &str) -> Option<String> {
    let mut w = info.split_whitespace();
    w.find(|w| w.eq_ignore_ascii_case("replaces"))?;
    let name = w.next()?.trim_end_matches(".st").to_ascii_lowercase();
    let bare = name.strip_prefix(spec::TEST_PREFIX)?;
    spec::name_ok(bare).then(|| format!("{name}.st"))
}

/// The answer without its scenario blocks and its list of requirements:
/// what the scope rules read as the code.
#[must_use]
pub fn code_part(answer: &str) -> String {
    let mut out = Vec::new();
    for s in segments(answer) {
        match s {
            Segment::Prose(p) => out.push(p),
            Segment::Code { info, .. } if is_scenario(&info) || is_requirements(&info) => {}
            Segment::Code { info, body, closed } => {
                out.push(if closed {
                    format!("```{info}\n{body}\n```")
                } else {
                    format!("```{info}\n{body}")
                });
            }
        }
    }
    out.join("\n")
}

/// Whether an answer offers code (a closed, non-empty block that is not
/// a scenario or a list of requirements).
#[must_use]
pub fn has_code(answer: &str) -> bool {
    fences(answer)
        .iter()
        .any(|f| !is_scenario(&f.info) && !is_requirements(&f.info) && !f.body.trim().is_empty())
}

/// One scenario of an answer, as the test file it becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    /// `test_NAME.st`, free in the project.
    pub file: String,
    /// What it checks, in plain words (its first comment).
    pub requirement: String,
    /// The file: the requirement as its first comment ([`spec::header`]),
    /// then the test's PROGRAM.
    pub text: String,
    /// It takes the place of this existing test (`file` is that name).
    pub replaces: bool,
}

fn first_word(line: &str) -> &str {
    let t = line.trim_start();
    let end = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(t.len());
    &t[..end]
}

/// The answer's scenarios, named free among `taken` (the project's file
/// names), and what is wrong with any that cannot be one.
#[must_use]
pub fn scenarios(answer: &str, taken: &[String]) -> (Vec<Scenario>, Vec<String>) {
    let mut out: Vec<Scenario> = Vec::new();
    let mut problems = Vec::new();
    let mut taken: Vec<String> = taken.to_vec();
    let mut names = BTreeSet::new();
    let blocks: Vec<_> = fences(answer)
        .into_iter()
        .filter(|f| is_scenario(&f.info))
        .collect();
    if blocks.len() > MAX_SCENARIOS {
        problems.push(format!(
            "The answer has {} scenarios; the page takes at most {MAX_SCENARIOS}.",
            blocks.len()
        ));
        return (out, problems);
    }
    for (k, f) in blocks.iter().enumerate() {
        let k = k + 1;
        let body = f.body.replace("\r\n", "\n").replace('\r', "\n");
        let body = body.trim_matches('\n');
        if body.trim().is_empty() {
            problems.push(format!("Scenario {k} is empty."));
            continue;
        }
        if body.len() > MAX_PROGRAM || body.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
            problems.push(format!("Scenario {k} is too large or holds control characters."));
            continue;
        }
        let lines: Vec<&str> = body.lines().collect();
        let Some(at) = lines
            .iter()
            .position(|l| first_word(l).eq_ignore_ascii_case("PROGRAM"))
        else {
            problems.push(format!("Scenario {k} has no PROGRAM test_<name>."));
            continue;
        };
        let Some(requirement) = spec::stated(&lines[..at].join("\n")) else {
            problems.push(format!(
                "Scenario {k} does not say what it checks: start it with a comment in plain words, before PROGRAM."
            ));
            continue;
        };
        if let Some(why) = crate::requirements::refusal(&requirement) {
            problems.push(format!("Scenario {k}'s requirement: {why}"));
            continue;
        }
        let head = lines[at].trim_start();
        let named = first_word(&head["PROGRAM".len()..]).to_ascii_lowercase();
        let Some(name) = named
            .strip_prefix(spec::TEST_PREFIX)
            .filter(|n| spec::name_ok(n))
            .map(str::to_string)
        else {
            problems.push(format!(
                "Scenario {k}'s PROGRAM is named {named:?}: it must be test_ and then a name of a-z, 0-9 and _."
            ));
            continue;
        };
        let replaced = replaces(&f.info);
        let key = replaced.clone().unwrap_or_else(|| format!("test_{name}.st"));
        if !names.insert(key) {
            problems.push(format!("Two scenarios are both named test_{name}."));
            continue;
        }
        let file = if let Some(old) = &replaced {
            if !taken.contains(old) {
                problems.push(format!(
                    "Scenario {k} replaces {}, which the project does not have: only a test listed in <requirement_tests> can be replaced; give the scenario a name of its own.",
                    old.trim_end_matches(".st")
                ));
                continue;
            }
            old.clone()
        } else {
            let free = crate::requirements::name_for(&name, &taken);
            let file = format!("test_{free}.st");
            taken.push(file.clone());
            file
        };
        let text = format!("{}{}\n", spec::header(&requirement), lines[at..].join("\n"));
        out.push(Scenario {
            file,
            requirement,
            text,
            replaces: replaced.is_some(),
        });
    }
    (out, problems)
}

/// The comments of `text`: `(* ... *)` and `// ...`, their inner text.
fn comments(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let block = rest.find("(*");
        let line = rest.find("//");
        match (block, line) {
            (Some(b), l) if l.is_none_or(|l| b < l) => {
                let inner = &rest[b + 2..];
                let end = inner.find("*)").unwrap_or(inner.len());
                out.push(&inner[..end]);
                rest = &inner[(end + 2).min(inner.len())..];
            }
            (_, Some(l)) => {
                let inner = &rest[l + 2..];
                let end = inner.find('\n').unwrap_or(inner.len());
                out.push(&inner[..end]);
                rest = &inner[end..];
            }
            _ => return out,
        }
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Words that, in a comment the file did not have, stand for code left out.
const ELIDED: [&str; 15] = [
    "same as before",
    "remains the same",
    "remain the same",
    "rest of the code",
    "rest of the file",
    "rest of the program",
    "rest of the logic",
    "existing code",
    "code omitted",
    "omitted for",
    "not shown",
    "for brevity",
    "and so on",
    "left out here",
    "etc.",
];
/// "unchanged" with one of these is code left out too.
const UNCHANGED_WITH: [&str; 11] = [
    "rest",
    "code",
    "lines",
    "above",
    "below",
    "remaining",
    "existing",
    "logic",
    "declarations",
    "other",
    "as is",
];

/// What in `code` marks a part left out that `old` did not already have:
/// `...` or `…`, or a new comment such as `(* rest unchanged *)`.
#[must_use]
pub fn elision(old: &str, code: &str) -> Option<String> {
    for e in ["...", "…"] {
        if code.matches(e).count() > old.matches(e).count() {
            return Some(e.to_string());
        }
    }
    let had: BTreeSet<String> = comments(old).into_iter().map(one_line).collect();
    comments(code).into_iter().find_map(|c| {
        let n = one_line(c);
        if had.contains(&n) {
            return None;
        }
        let left_out = ELIDED.iter().any(|p| n.contains(p))
            || (n.contains("unchanged") && UNCHANGED_WITH.iter().any(|w| n.contains(w)));
        left_out.then(|| format!("(*{c}*)"))
    })
}

const HEADS: [&str; 3] = ["PROGRAM", "FUNCTION_BLOCK", "FUNCTION"];
const ENDS: [&str; 3] = ["END_PROGRAM", "END_FUNCTION_BLOCK", "END_FUNCTION"];

/// `text`'s lines as compared: no trailing blanks, no blank lines at
/// either end.
fn trimmed(text: &str) -> Vec<&str> {
    let v: Vec<&str> = text.lines().map(str::trim_end).collect();
    let a = v.iter().position(|l| !l.is_empty()).unwrap_or(v.len());
    let b = v.iter().rposition(|l| !l.is_empty()).map_or(a, |k| k + 1);
    v[a..b].to_vec()
}

/// Why `code` (the answer's code block) is not the whole file of `scope`
/// with the edits made in it, if it is not. `old` is the file as it is,
/// `landed` the file with the answer taken in through the scope rules
/// (when they took it). A new project's program is checked for markers
/// and balance alone.
#[must_use]
pub fn full_copy(scope: &Scope, old: &str, code: &str, landed: Option<&str>) -> Option<String> {
    let file = scope.file().unwrap_or("the program");
    let whole = format!("Send the whole of {file} back, every line, with the edits made in it.");
    if let Some(m) = elision(old, code) {
        return Some(format!(
            "The code leaves part of {file} out: it says {m} where code should be. {whole}"
        ));
    }
    if *scope != Scope::NewProject {
        let have: BTreeSet<String> = pous(code).into_iter().map(|(n, _)| n).collect();
        let missing: Vec<String> = pous(old)
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| !have.contains(n))
            .collect();
        if !missing.is_empty() {
            return Some(format!(
                "The code is not the whole of {file}: it leaves out {}. {whole}",
                missing.join(", ")
            ));
        }
    }
    let count = |words: &[&str]| {
        code.lines()
            .filter(|l| words.iter().any(|w| first_word(l).eq_ignore_ascii_case(w)))
            .count()
    };
    let (opened, closed) = (count(&HEADS), count(&ENDS));
    if opened != closed {
        return Some(format!(
            "The code opens {opened} POU(s) and closes {closed}: a header or an END_ line is missing. {whole}"
        ));
    }
    if scope.lines().is_some()
        && let Some(landed) = landed
        && trimmed(landed) != trimmed(code)
    {
        let (from, to) = scope.lines().unwrap_or_default();
        return Some(format!(
            "The code is only part of {file} (lines {from}-{to}). {whole} Change only lines {from}-{to}."
        ));
    }
    None
}

/// One scenario as it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    pub file: String,
    pub requirement: String,
    pub passed: bool,
    /// Why not (empty when it passed).
    pub why: String,
}

/// An answer as the page checked it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    /// It gives code (an explanation alone needs no check).
    pub code: bool,
    /// Why the answer cannot be taken: the scope rules, the full copy, the
    /// compile. Nothing is written while one stands.
    pub failures: Vec<String>,
    /// What the tests say against it besides its own scenarios failing: a
    /// scenario that cannot be one, a requirement test red with the code,
    /// a replacement of a test that still passes, scenarios that do not
    /// tell the change from no change. Sent back while retries last, then
    /// a warning.
    pub tests: Vec<String>,
    pub scenarios: Vec<Ran>,
    /// The scenario files (name, text) kept as tests: the ones that
    /// passed.
    pub add: Vec<(String, String)>,
    /// How many of the project's own requirement tests ran green with it.
    pub tests_green: usize,
    /// Requirement tests a scenario replaces, old wording and new.
    pub replaced: Vec<Replaced>,
    /// The answer claims the change keeps the behaviour (an `Unchanged
    /// behaviour:` line), so no scenario has to fail on the old file.
    pub unchanged: bool,
}

/// Whether the answer's prose carries an `Unchanged behaviour: <why>` line
/// with a reason.
#[must_use]
pub fn claims_unchanged(answer: &str) -> bool {
    const KEY: &str = "unchanged behaviour:";
    segments(answer).iter().any(|s| match s {
        Segment::Prose(p) => p.lines().any(|l| {
            let l = l.trim_start_matches(['*', '_', '#', ' ']).to_ascii_lowercase();
            l.strip_prefix(KEY).is_some_and(|r| !r.trim().is_empty())
        }),
        Segment::Code { .. } => false,
    })
}

/// A requirement test a scenario took the place of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replaced {
    pub file: String,
    /// The requirement the test stated.
    pub was: String,
    /// The requirement the scenario states.
    pub now: String,
}

impl Check {
    /// Everything passed (scenarios, if any, all green).
    #[must_use]
    pub fn green(&self) -> bool {
        self.failures.is_empty() && self.tests.is_empty() && self.scenarios.iter().all(|s| s.passed)
    }

    /// Code that cannot be written whatever its tests say.
    #[must_use]
    pub fn blocked(&self) -> bool {
        self.code && !self.failures.is_empty()
    }

    /// What the tests say against code that can be written, one line
    /// each: empty for a green answer and for a blocked one.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        if self.blocked() {
            return Vec::new();
        }
        let mut out = self.tests.clone();
        out.extend(self.failed_scenarios());
        out
    }

    fn failed_scenarios(&self) -> impl Iterator<Item = String> + '_ {
        self.scenarios
            .iter()
            .filter(|s| !s.passed)
            .map(|s| format!("Scenario {} ({}) failed: {}", s.requirement, s.file, s.why))
    }

    /// Code with nothing wrong and no scenario to prove it.
    #[must_use]
    pub fn unchecked(&self) -> bool {
        self.code && self.green() && self.scenarios.is_empty()
    }

    /// Code that failed.
    #[must_use]
    pub fn red(&self) -> bool {
        self.code && !self.green()
    }

    /// Every failure, one line each.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = self.failures.clone();
        out.extend(self.tests.iter().cloned());
        out.extend(self.failed_scenarios());
        out
    }
}

/// The answer's code block: the whole PROGRAM for a new project, else its
/// one non-empty block (none when there are several: the scope rules say
/// why).
fn the_code(scope: &Scope, answer: &str) -> Option<String> {
    let code = code_part(answer);
    if *scope == Scope::NewProject {
        return extract_program(&code);
    }
    let blocks: Vec<_> = fences(&code)
        .into_iter()
        .filter(|f| !f.body.trim().is_empty())
        .collect();
    match blocks.as_slice() {
        [one] => Some(one.body.replace("\r\n", "\n")),
        _ => None,
    }
}

/// Checks `answer` against the project `files` in `scope`: the scope
/// rules, the full copy, the scenarios, the compile, and the scenarios and
/// requirement tests run with `settings`. Deterministic.
#[must_use]
pub fn check(scope: &Scope, files: &Files, answer: &str, settings: Settings) -> Check {
    // A block that never closes means the answer was cut off in it; half
    // a file or half a scenario is not checked as if it were whole.
    if segments(answer)
        .iter()
        .any(|s| matches!(s, Segment::Code { closed: false, .. }))
    {
        return Check {
            code: true,
            failures: vec![
                "The answer ends inside a code block that was never closed: it was cut off. Send it again in full, every block closed."
                    .into(),
            ],
            ..Check::default()
        };
    }
    if !has_code(answer) {
        return Check::default();
    }
    let mut c = Check {
        code: true,
        ..Check::default()
    };
    let landed = match apply(scope, files, answer) {
        Ok(f) => Some(f),
        Err(r) => {
            c.failures.push(r.why);
            None
        }
    };
    let old = scope.file().and_then(|f| files.get(f)).map_or("", String::as_str);
    if let Some(code) = the_code(scope, answer) {
        let landed_file = scope.file().map_or(Some(helmsman_project::MAIN), Some);
        let now = landed
            .as_ref()
            .zip(landed_file)
            .and_then(|(l, f)| l.get(f))
            .map(String::as_str);
        if let Some(why) = full_copy(scope, old, &code, now) {
            c.failures.insert(0, why);
        }
    }
    let Some(landed) = landed else {
        return c;
    };
    let taken: Vec<String> = landed.keys().cloned().collect();
    let (scen, problems) = scenarios(answer, &taken);
    c.tests.extend(problems);
    let (scen, replaced) = contradicted(&landed, scen, settings, &mut c.tests);
    let mut all = landed;
    for s in &scen {
        all.insert(s.file.clone(), s.text.clone());
    }
    let plan = spec::plan(
        all.iter()
            .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
            .map(|(n, t)| (n.as_str(), t.as_str())),
    );
    if let Err(d) = helmsman_iec::compile(&plan.main, settings.sizes) {
        let first = d.first().map_or_else(String::new, ToString::to_string);
        c.failures.push(format!("The code does not compile: {first}"));
        return c;
    }
    let (held, advice) = lint_findings(files, &plan.main);
    c.failures.extend(held);
    c.tests.extend(advice);
    for v in helmsman_sim::iec::run_all(&plan, settings, plant_of(&all)) {
        if let Some(s) = scen.iter().find(|s| s.file == v.file) {
            c.scenarios.push(Ran {
                file: s.file.clone(),
                requirement: s.requirement.clone(),
                passed: v.passed(),
                why: if v.untested() {
                    "it checks nothing: it holds no ASSERT".into()
                } else {
                    v.why()
                },
            });
        } else if v.red() {
            let name = v.file.trim_end_matches(".st");
            c.tests.push(format!(
                "Requirement test {name} is red with this code: {}. If the instruction changes what it checks, send its replacement in a block marked ```st scenario replaces {name}; else keep it passing.",
                v.why()
            ));
        } else if v.passed() {
            c.tests_green += 1;
        }
    }
    if c.tests.is_empty() && c.scenarios.iter().all(|r| r.passed) {
        c.unchanged = claims_unchanged(answer);
        if !c.unchanged {
            c.tests.extend(indistinct(scope, files, &all, &scen, settings));
        }
    }
    // Only a scenario that passed is kept, and only a kept one replaces.
    let passed = |file: &str| c.scenarios.iter().any(|r| r.file == file && r.passed);
    c.replaced = replaced.into_iter().filter(|r| passed(&r.file)).collect();
    c.add = scen
        .into_iter()
        .filter(|s| passed(&s.file))
        .map(|s| (s.file, s.text))
        .collect();
    c
}

/// The lint findings (`helmsman_iec::lint`) the answer adds: those of the
/// project with it that the project as it was did not have, as `(held,
/// advice)`. A blocking one that no pragma overrides holds the answer like
/// a compile error (it could not be deployed); the others go back to the
/// Agent and are a warning after the retries. A finding the project
/// already had is not the answer's to fix.
fn lint_findings(old: &Files, main: &str) -> (Vec<String>, Vec<String>) {
    let before = spec::plan(
        old.iter()
            .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
            .map(|(n, t)| (n.as_str(), t.as_str())),
    );
    let mut had: Vec<(&'static str, String)> = helmsman_iec::lint::lints(&before.main)
        .into_iter()
        .map(|l| (l.id, l.msg))
        .collect();
    let (mut held, mut advice) = (Vec::new(), Vec::new());
    for l in helmsman_iec::lint::lints(main) {
        if l.overridden.is_some() {
            continue;
        }
        if let Some(i) = had.iter().position(|(id, m)| *id == l.id && *m == l.msg) {
            had.swap_remove(i);
            continue;
        }
        let (file, line) = lint_place(main, l.pos.line);
        let said = format!("{} at {file}:{line}: {}", l.id, l.msg);
        if l.blocking {
            held.push(format!(
                "Lint {said} This blocks the code from being used or deployed until it is fixed."
            ));
        } else {
            advice.push(format!("Lint {said}"));
        }
    }
    (held, advice)
}

/// The file and line of `line` in a build's source, which holds each file
/// under a `(* file NAME *)` line when there are several.
fn lint_place(main: &str, line: u32) -> (String, u32) {
    let (mut file, mut start) = ("the code".to_string(), 0u32);
    for (i, l) in main.lines().enumerate().take(line as usize) {
        if let Some(n) = l.strip_prefix("(* file ").and_then(|r| r.strip_suffix(" *)")) {
            file = n.to_string();
            start = u32::try_from(i + 1).unwrap_or(0);
        }
    }
    (file, line - start)
}

/// The code of `text` with comments and white space gone: what it does.
fn bare(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("(*") {
            rest = r.find("*)").map_or("", |k| &r[k + 2..]);
        } else if let Some(r) = rest.strip_prefix("//") {
            rest = r.find('\n').map_or("", |k| &r[k..]);
        } else {
            let mut cs = rest.chars();
            let c = cs.next().unwrap_or(' ');
            if !c.is_whitespace() {
                out.push(c.to_ascii_lowercase());
            }
            rest = cs.as_str();
        }
    }
    out
}

/// The scenarios that replace a requirement test, held against the new
/// build: a replacement stands only when the test it replaces is red with
/// the new code (an untested placeholder has nothing to contradict). One
/// whose test still passes, or that the project does not have, is dropped
/// and the old test stays, with the reason in `failures`. Returns the
/// scenarios left and what was replaced.
/// The project's `plant.sim`, empty without one: what its tests run
/// against.
fn plant_of(files: &Files) -> &str {
    files.get(helmsman_project::PLANT).map_or("", String::as_str)
}

fn contradicted(
    landed: &Files,
    scen: Vec<Scenario>,
    settings: Settings,
    failures: &mut Vec<String>,
) -> (Vec<Scenario>, Vec<Replaced>) {
    if !scen.iter().any(|s| s.replaces) {
        return (scen, Vec::new());
    }
    let now = spec::plan(
        landed
            .iter()
            .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
            .map(|(n, t)| (n.as_str(), t.as_str())),
    );
    let mut kept = Vec::new();
    let mut replaced = Vec::new();
    for s in scen {
        let old = s
            .replaces
            .then(|| now.tests.iter().find(|t| t.file == s.file))
            .flatten();
        let Some(old) = old else {
            kept.push(s);
            continue;
        };
        let v = helmsman_sim::iec::run(&now.main, old, settings, plant_of(landed));
        if v.red() || v.untested() {
            replaced.push(Replaced {
                file: s.file.clone(),
                was: old.requirement.clone(),
                now: s.requirement.clone(),
            });
            kept.push(s);
        } else {
            let name = s.file.trim_end_matches(".st");
            failures.push(format!(
                "Scenario {} replaces {name} ({}), but {name} still passes with this code, so the instruction does not contradict it and it stays. Keep {name} passing and give this scenario a name of its own.",
                s.file, old.requirement
            ));
        }
    }
    (kept, replaced)
}

/// Why the scenarios do not tell the change from no change, if they do
/// not: the answer changes what the file does, and every scenario passes
/// on the project as it was. `all` is the project with the answer, whose
/// scenarios `scen` are.
fn indistinct(
    scope: &Scope,
    files: &Files,
    all: &Files,
    scen: &[Scenario],
    settings: Settings,
) -> Option<String> {
    let file = scope.file()?;
    if scen.is_empty() || bare(files.get(file)?) == bare(all.get(file)?) {
        return None;
    }
    let mut before = files.clone();
    for s in scen {
        before.insert(s.file.clone(), s.text.clone());
    }
    let plan = spec::plan(
        before
            .iter()
            .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
            .map(|(n, t)| (n.as_str(), t.as_str())),
    );
    let ran: Vec<_> = helmsman_sim::iec::run_all(&plan, settings, plant_of(&before))
        .into_iter()
        .filter(|v| scen.iter().any(|s| s.file == v.file))
        .collect();
    if ran.iter().any(spec::Verdict::red) {
        return None;
    }
    let names: Vec<String> = ran
        .iter()
        .map(|v| v.file.trim_end_matches(".st").to_string())
        .collect();
    Some(format!(
        "The scenario does not distinguish the change: {} pass on {file} as it was before, so they would pass without the change. Send a scenario that fails on the old file and passes on the new one, or, if the instruction keeps what the file does (a rename, a tidy, moving code into a FUNCTION_BLOCK), add the line `Unchanged behaviour: ` and one sentence why right after the Why line.",
        names.join(", ")
    ))
}

/// How a retry asks for the code again, from an agent that sends whole
/// files; one that sends the lines that change is asked in its own form
/// (`crate::edits::retry_words`).
pub const FIXED_FILE: &str = "the whole file with the fix made in it, in one ```st block,";
/// [`FIXED_FILE`], when the code itself was not what failed.
pub const SAME_FILE: &str = "the same whole file in one ```st block,";

/// What the page does with a checked answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Show it as it is (green, red after the retries, or no code).
    Show,
    /// Send this back to the Agent and show what it answers instead.
    Retry(String),
}

/// After `retries` retries (and `asked` whether one of them asked for
/// scenarios already): a red answer goes back while retries are left;
/// code with no scenario is asked for scenarios once.
#[must_use]
pub fn next(check: &Check, retries: u8, asked: bool) -> Next {
    if !check.code || retries >= RETRIES {
        return Next::Show;
    }
    if check.red() {
        let mut m = String::from("The page checked your answer before showing it, and it did not pass:\n");
        for p in check.problems() {
            m.push_str("- ");
            m.push_str(&p);
            m.push('\n');
        }
        if check.scenarios.iter().any(|r| !r.passed) {
            m.push_str(
                "A failed scenario lists, in brackets, the values it held at the failing scan: the inputs it drove then and the outputs it read. If those inputs are not the case the check is about, the scenario's timing is wrong, so fix the scenario; if they are, fix the code.\n",
            );
        }
        m.push_str("Answer again in full: ");
        m.push_str(FIXED_FILE);
        m.push_str(" and the ```st scenario blocks that check it.");
        return Next::Retry(m);
    }
    if check.unchecked() && !asked {
        return Next::Retry(format!(
            "Your answer gives code but no scenario to check it. Answer again in full: {SAME_FILE} and one or more ```st scenario blocks that prove it does what was asked."
        ));
    }
    Next::Show
}

/// The I/O list a deploy to a Skipper binds the program's ports to: the
/// project's `io.map` when it has a point (the deploy sends it first and
/// the Skipper restarts on it), else `node`, the one the Skipper runs.
#[must_use]
pub fn io_list<'a>(files: &'a Files, node: &'a str) -> &'a str {
    match files.get(helmsman_project::IO_MAP) {
        Some(m)
            if m.lines()
                .any(|l| matches!(l.split_whitespace().next(), Some("in" | "out"))) =>
        {
            m
        }
        _ => node,
    }
}
