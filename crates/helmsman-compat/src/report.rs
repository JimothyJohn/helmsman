//! One finding per check, printed one per line and kept as JSON for the
//! train's summary.

use serde_json::{Value as Json, json};
use std::fmt::Write as _;

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub ok: bool,
    /// What was checked, short and stable (`devices/moxa/iologik-e1213`).
    pub check: String,
    /// What was seen, in words.
    pub detail: String,
}

impl Finding {
    #[must_use]
    pub fn pass(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            check: check.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn fail(check: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            check: check.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn json(&self) -> Json {
        json!({ "ok": self.ok, "check": self.check, "detail": self.detail })
    }
}

/// The findings as lines, failures marked; whether all passed.
#[must_use]
pub fn render(findings: &[Finding]) -> (String, bool) {
    let mut out = String::new();
    for f in findings {
        let mark = if f.ok { "ok  " } else { "FAIL" };
        let _ = writeln!(out, "{mark} {}: {}", f.check, f.detail);
    }
    let failed = findings.iter().filter(|f| !f.ok).count();
    let _ = writeln!(out, "{} checks, {failed} failed", findings.len());
    (out, failed == 0 && !findings.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_checked_is_not_a_pass() {
        assert!(!render(&[]).1);
    }

    #[test]
    fn one_failure_fails_the_set() {
        let (text, ok) = render(&[Finding::pass("a", "fine"), Finding::fail("b", "broken")]);
        assert!(!ok);
        assert!(text.contains("FAIL b: broken"), "{text}");
        assert!(text.ends_with("2 checks, 1 failed\n"), "{text}");
    }
}
