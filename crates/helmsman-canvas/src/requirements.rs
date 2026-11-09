//! The project's requirement tests as results (Nick, 2026-11-05): one row
//! per `test_NAME.st` file (`helmsman_iec::spec`), green, red with why, or
//! grey (its requirement written, no test yet). The simulation runs them
//! whenever the project's build changes and before Deploy and "Use this
//! code"; a box runs them when a project opens, after each save and
//! before a deploy (`POST /api/projects/{p}/tests`). A red test refuses
//! Deploy and "Use this code", naming the test; a grey one does not.
//!
//! The Requirements tab that listed these rows and took a requirement in
//! plain words for the agent to write its test is archived
//! (`crates/helmsman-canvas/archive/requirements/`, Nick, 2026-11-08): the
//! scenarios the Agent writes with each change are the tests now.

use helmsman_iec::spec::Verdict;
use serde_json::Value as Json;

/// Longest requirement, in characters (the box's limit).
pub const MAX_REQUIREMENT: usize = 500;

/// Where a test stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Passed,
    Failed,
    /// Its requirement is written, its test not yet.
    Untested,
}

impl State {
    fn parse(s: &str) -> Self {
        match s {
            "passed" => Self::Passed,
            "untested" => Self::Untested,
            _ => Self::Failed,
        }
    }
}

/// One test's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub file: String,
    pub requirement: String,
    pub state: State,
    /// Why it is red (empty otherwise).
    pub why: String,
    pub scans: u32,
    pub elapsed_ms: u64,
}

impl Row {
    fn from_verdict(v: &Verdict) -> Self {
        Self {
            file: v.file.clone(),
            requirement: v.requirement.clone(),
            state: State::parse(v.state()),
            why: if v.red() { v.why() } else { String::new() },
            scans: v.scans,
            elapsed_ms: u64::try_from(v.elapsed.as_millis()).unwrap_or(u64::MAX),
        }
    }

    fn from_json(j: &Json) -> Option<Self> {
        let state = match j["state"].as_str() {
            Some(s) => State::parse(s),
            // A box from before the untested state.
            None if j["passed"].as_bool() == Some(true) => State::Passed,
            None => State::Failed,
        };
        Some(Self {
            file: j["file"].as_str()?.to_string(),
            requirement: j["requirement"].as_str().unwrap_or("").to_string(),
            state,
            why: if state == State::Failed {
                j["why"].as_str().unwrap_or("").to_string()
            } else {
                String::new()
            },
            scans: u32::try_from(j["scans"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX),
            elapsed_ms: j["elapsed_ms"].as_u64().unwrap_or(0),
        })
    }

    /// `test_lamp` for `test_lamp.st`.
    #[must_use]
    pub fn name(&self) -> &str {
        self.file.trim_end_matches(".st")
    }

    /// The tests read a row by it; the gate reads `state`.
    #[cfg(test)]
    #[must_use]
    pub fn passed(&self) -> bool {
        self.state == State::Passed
    }
}

/// The tests' last results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Results {
    pub rows: Vec<Row>,
    /// Whether they have run at all (a project with no tests has run,
    /// with no rows).
    pub ran: bool,
}

impl Results {
    #[must_use]
    pub fn from_verdicts(v: &[Verdict]) -> Self {
        Self {
            rows: v.iter().map(Row::from_verdict).collect(),
            ran: true,
        }
    }

    /// `POST /api/projects/{p}/tests`'s body.
    #[must_use]
    pub fn from_json(j: &Json) -> Self {
        Self {
            rows: j["tests"]
                .as_array()
                .map(|a| a.iter().filter_map(Row::from_json).collect())
                .unwrap_or_default(),
            ran: true,
        }
    }

    /// The red rows, one line each: `test_lamp: lamp on in time failed at 2.00 s`.
    #[must_use]
    pub fn red(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| r.state == State::Failed)
            .map(|r| format!("{}: {}", r.name(), r.why))
            .collect()
    }

    /// Why a deploy or a "Use this code" is refused, in one line; `None`
    /// when no test is red (grey ones never refuse).
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        let red = self.red();
        match red.len() {
            0 => None,
            1 => Some(format!("Refused: requirement test {} is red.", red[0])),
            n => Some(format!(
                "Refused: {n} requirement tests are red: {}.",
                red.join("; ")
            )),
        }
    }
}

/// Why `text` cannot be a requirement, if it cannot.
#[must_use]
pub fn refusal(text: &str) -> Option<String> {
    let t = text.trim();
    let n = t.chars().count();
    if n == 0 {
        Some("Type a requirement first.".into())
    } else if n > MAX_REQUIREMENT {
        Some(format!(
            "A requirement is at most {MAX_REQUIREMENT} characters ({n} now)."
        ))
    } else if t.chars().any(char::is_control) {
        Some("A requirement is one line.".into())
    } else {
        None
    }
}

/// A free test name for `text`: its words as a slug, unique among `taken`
/// (file names).
#[must_use]
pub fn name_for(text: &str, taken: &[String]) -> String {
    let base = helmsman_iec::spec::slug(text, 45);
    let free = |n: &str| {
        !taken
            .iter()
            .any(|f| f.eq_ignore_ascii_case(&format!("test_{n}.st")))
    };
    if free(&base) {
        return base;
    }
    (2..100)
        .map(|k| format!("{base}_{k}"))
        .find(|n| free(n))
        .unwrap_or_else(|| format!("{base}_{}", taken.len() + 100))
}
