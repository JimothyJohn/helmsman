//! The Agent's answer checked before the engineer sees it (Nick,
//! 2026-11-05, card `agent-scenarios`): when it gives code, it also writes
//! scenarios, requirement tests in the `helmsman_iec::spec` format, each in
//! a fenced block marked ```` ```st scenario ````. The page takes the code
//! into a copy of the project through the scope rules ([`crate::scope::apply`]),
//! adds the scenarios, compiles it, and runs the scenarios and the
//! project's own requirement tests ([`check`]). An answer that fails goes
//! back to the Agent with what failed, up to [`RETRIES`] times ([`next`]);
//! one still failing is shown red and "Use this code" refuses it. On "Use
//! this code" every scenario of a green answer is kept as a requirement
//! test, its first comment the requirement in plain words.
//!
//! **The whole file comes back** (Nick, 2026-11-05): code for a file, some
//! lines or a block is the whole open file with the edits made in place,
//! never a fragment ([`full_copy`]): no elision markers, every POU of the
//! file there with its header and its `END_` line, and for lines or a
//! block the landed file equal to the code (a fragment the scope rules
//! would splice in is refused). The selection still bounds what may
//! change: [`crate::scope::apply`] refuses a change outside it.
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

/// The answer without its scenario blocks: what the scope rules read as
/// the code.
#[must_use]
pub fn code_part(answer: &str) -> String {
    let mut out = Vec::new();
    for s in segments(answer) {
        match s {
            Segment::Prose(p) => out.push(p),
            Segment::Code { info, .. } if is_scenario(&info) => {}
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
/// a scenario).
#[must_use]
pub fn has_code(answer: &str) -> bool {
    fences(answer)
        .iter()
        .any(|f| !is_scenario(&f.info) && !f.body.trim().is_empty())
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
        if !names.insert(name.clone()) {
            problems.push(format!("Two scenarios are both named test_{name}."));
            continue;
        }
        let free = crate::requirements::name_for(&name, &taken);
        let file = format!("test_{free}.st");
        taken.push(file.clone());
        let text = format!("{}{}\n", spec::header(&requirement), lines[at..].join("\n"));
        out.push(Scenario {
            file,
            requirement,
            text,
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
    /// What failed besides the scenarios: the scope rules, the full copy,
    /// a scenario that cannot be one, the compile, a requirement test.
    pub failures: Vec<String>,
    pub scenarios: Vec<Ran>,
    /// The scenario files (name, text) "Use this code" adds.
    pub add: Vec<(String, String)>,
    /// How many of the project's own requirement tests ran green with it.
    pub tests_green: usize,
}

impl Check {
    /// Everything passed (scenarios, if any, all green).
    #[must_use]
    pub fn green(&self) -> bool {
        self.failures.is_empty() && self.scenarios.iter().all(|s| s.passed)
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
        out.extend(
            self.scenarios
                .iter()
                .filter(|s| !s.passed)
                .map(|s| format!("Scenario {} ({}) failed: {}", s.requirement, s.file, s.why)),
        );
        out
    }

    /// Why "Use this code" refuses it, if it does.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        self.red().then(|| {
            format!(
                "The answer did not pass the page's checks: {}",
                self.problems().join(" ")
            )
        })
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
    c.failures.extend(problems);
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
    for v in spec::run_all(&plan, settings) {
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
            c.failures.push(format!(
                "Requirement test {} is red with this code: {}",
                v.file.trim_end_matches(".st"),
                v.why()
            ));
        } else if v.passed() {
            c.tests_green += 1;
        }
    }
    c.add = scen.into_iter().map(|s| (s.file, s.text)).collect();
    c
}

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
        m.push_str(
            "Answer again in full: the whole file with the fix made in it, in one ```st block, and the ```st scenario blocks that check it.",
        );
        return Next::Retry(m);
    }
    if check.unchecked() && !asked {
        return Next::Retry(
            "Your answer gives code but no scenario to check it. Answer again in full: the same whole file in one ```st block, and one or more ```st scenario blocks that prove it does what was asked."
                .into(),
        );
    }
    Next::Show
}
