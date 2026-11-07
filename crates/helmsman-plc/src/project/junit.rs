//! `JUnit` XML, the report every CI system reads (GitHub Actions through a
//! reporter action, GitLab, Jenkins, Azure DevOps), written by hand: one
//! `<testsuites>` holding one `<testsuite>`, each case passed, failed
//! (`<failure>`), an error (`<error>`: it could not run) or skipped
//! (`<skipped>`). What `helmsman-plc test --junit` writes ([`super::check`]).
//!
//! Text and attributes are escaped (`& < > " '`), and a character XML 1.0
//! cannot carry (a control character other than tab, LF and CR) becomes
//! U+FFFD, so any name or message makes a well-formed file. The cases come
//! out in the order given; only the `time` attributes vary between runs.

use std::fmt::Write as _;
use std::time::Duration;

/// How one case came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    /// It ran and was wrong: a one-line message, and the detail.
    Failed {
        message: String,
        detail: String,
    },
    /// It could not run.
    Error {
        message: String,
        detail: String,
    },
    /// Nothing checked it.
    Skipped {
        message: String,
    },
}

/// One test case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub classname: String,
    pub name: String,
    /// The file it is in, when it is one.
    pub file: Option<String>,
    pub outcome: Outcome,
    /// What it printed (`<system-out>`), if anything.
    pub out: String,
}

/// `text` safe in XML text and in a quoted attribute.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if allowed(c) => out.push(c),
            _ => out.push('\u{FFFD}'),
        }
    }
    out
}

/// Whether XML 1.0 can carry `c` (its `Char` production, tab, LF and CR
/// aside).
fn allowed(c: char) -> bool {
    matches!(c, '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn secs(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64())
}

/// The report: suite `name` of `cases`, which took `time`.
#[must_use]
pub fn report(name: &str, cases: &[Case], time: Duration) -> String {
    let count = |f: fn(&Outcome) -> bool| cases.iter().filter(|c| f(&c.outcome)).count();
    let failures = count(|o| matches!(o, Outcome::Failed { .. }));
    let errors = count(|o| matches!(o, Outcome::Error { .. }));
    let skipped = count(|o| matches!(o, Outcome::Skipped { .. }));
    let totals = format!(
        "tests=\"{}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{}\"",
        cases.len(),
        secs(time)
    );
    let name = escape(name);
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(x, "<testsuites name=\"{name}\" {totals}>");
    let _ = writeln!(x, "  <testsuite name=\"{name}\" {totals}>");
    for c in cases {
        let _ = write!(
            x,
            "    <testcase classname=\"{}\" name=\"{}\"",
            escape(&c.classname),
            escape(&c.name)
        );
        if let Some(f) = &c.file {
            let _ = write!(x, " file=\"{}\"", escape(f));
        }
        if c.outcome == Outcome::Passed && c.out.is_empty() {
            x.push_str("/>\n");
            continue;
        }
        x.push_str(">\n");
        match &c.outcome {
            Outcome::Passed => {}
            Outcome::Failed { message, detail } => {
                let _ = writeln!(
                    x,
                    "      <failure message=\"{}\" type=\"failed\">{}</failure>",
                    escape(message),
                    escape(detail)
                );
            }
            Outcome::Error { message, detail } => {
                let _ = writeln!(
                    x,
                    "      <error message=\"{}\" type=\"error\">{}</error>",
                    escape(message),
                    escape(detail)
                );
            }
            Outcome::Skipped { message } => {
                let _ = writeln!(x, "      <skipped message=\"{}\"/>", escape(message));
            }
        }
        if !c.out.is_empty() {
            let _ = writeln!(x, "      <system-out>{}</system-out>", escape(&c.out));
        }
        x.push_str("    </testcase>\n");
    }
    x.push_str("  </testsuite>\n</testsuites>\n");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, outcome: Outcome) -> Case {
        Case {
            classname: "p".into(),
            name: name.into(),
            file: None,
            outcome,
            out: String::new(),
        }
    }

    #[test]
    fn escapes_markup_quotes_and_unencodable_characters() {
        assert_eq!(
            escape(r#"a<b>&"c"'d'"#),
            "a&lt;b&gt;&amp;&quot;c&quot;&apos;d&apos;"
        );
        assert_eq!(escape("Pumpe läuft – 2 µs ✓ 🚰"), "Pumpe läuft – 2 µs ✓ 🚰");
        assert_eq!(escape("tab\tline\ncr\r"), "tab\tline\ncr\r");
        assert_eq!(
            escape("nul\0bell\u{7}esc\u{1b}"),
            "nul\u{FFFD}bell\u{FFFD}esc\u{FFFD}"
        );
        assert_eq!(escape("\u{FFFE}\u{FFFF}"), "\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn counts_each_outcome_and_keeps_the_order() {
        let cases = [
            case("b", Outcome::Passed),
            case(
                "a",
                Outcome::Failed {
                    message: "m".into(),
                    detail: "d".into(),
                },
            ),
            case(
                "c",
                Outcome::Error {
                    message: "e".into(),
                    detail: String::new(),
                },
            ),
            case("d", Outcome::Skipped { message: "s".into() }),
        ];
        let x = report("p", &cases, Duration::from_millis(1234));
        assert!(x.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(x.contains(
            "<testsuite name=\"p\" tests=\"4\" failures=\"1\" errors=\"1\" skipped=\"1\" time=\"1.234\">"
        ));
        let at = |n: &str| x.find(&format!("name=\"{n}\"")).unwrap();
        assert!(at("b") < at("a") && at("a") < at("c") && at("c") < at("d"));
        assert!(x.contains("<testcase classname=\"p\" name=\"b\"/>"));
        assert!(x.contains("<failure message=\"m\" type=\"failed\">d</failure>"));
        assert!(x.contains("<skipped message=\"s\"/>"));
        assert!(x.ends_with("</testsuites>\n"));
    }

    #[test]
    fn a_hostile_name_stays_inside_its_attribute() {
        let x = report(
            "\"/><evil a='",
            &[case(
                "x\" injected=\"1",
                Outcome::Skipped {
                    message: "</skipped>".into(),
                },
            )],
            Duration::ZERO,
        );
        assert!(!x.contains("<evil"));
        assert!(!x.contains("injected=\"1\""));
        assert!(x.contains("message=\"&lt;/skipped&gt;\""));
    }
}
