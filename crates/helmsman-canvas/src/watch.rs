//! The Agent watches and speaks first (TODO.md `agent-watches`; Nick,
//! 2026-11-14: "It should be watching every time it breaks to point out
//! the stupid thing you did to show how it should be done better").
//!
//! Whatever the page already sees go wrong (the program does not compile,
//! it faulted, it is held by a lost input, a requirement test is red) is
//! said unasked, in one line under the top bar, with the better way and
//! one button: **Fix it** hands the Agent an instruction that carries the
//! failure, and the change goes through the same checks as any other.
//!
//! What is said comes from rules here, not from the model: watching costs
//! no model turn, and only a click spends one (the cost question the card
//! left open, decided 2026-11-14).
//!
//! Patterns (TODO.md `agent-patterns`; Nick: "It should be seeing your
//! patterns and fixing them"): the same kind of slip in several places is
//! said once, with one Fix it for all of them, and a kind that keeps
//! coming up in a session is counted and said to be a habit. The count
//! lives in memory only; nothing about the engineer is stored.
//!
//! [`noticed`] picks the one thing to say, most urgent first. [`Watching`]
//! decides when: not while the engineer is still typing ([`QUIET`]), not
//! again once they said "Not now" to that same thing.

use crate::projects::{Diag, Severity};
use crate::requirements::{Results, State};

/// Seconds a problem stands before it is said: a line half typed is not
/// a mistake yet.
pub const QUIET: f64 = 1.5;

/// One thing that went wrong, said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noticed {
    /// What it is, to tell it from the next one.
    pub key: String,
    /// What went wrong, one line.
    pub what: String,
    /// The better way, when a rule knows one.
    pub better: Option<String>,
    /// What Fix it asks the Agent.
    pub ask: String,
    /// The kind of slip, when a rule knows it: what [`Watching`] counts
    /// to see a habit.
    pub rule: Option<&'static str>,
}

/// What the page sees of the open project.
pub struct Seen<'a> {
    /// The compile errors and warnings, in source order.
    pub diags: &'a [Diag],
    /// The scan's fault, if the program stopped on one.
    pub fault: Option<&'a str>,
    /// The inputs whose device is lost while the program has no `NAME_ok`
    /// for them: it is held, outputs at zero.
    pub held_by: &'a [String],
    pub tests: &'a Results,
}

/// The one thing to say about `seen`, most urgent first: an error that
/// stops the build, a fault, a held program, a red requirement.
#[must_use]
pub fn noticed(seen: &Seen<'_>) -> Option<Noticed> {
    let errors: Vec<&Diag> = seen
        .diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if let Some(d) = errors.first() {
        let rule = rule_of(&d.message);
        // The same slip in several places is one thing to say, and one
        // fix (TODO.md `agent-patterns`).
        let same: Vec<&Diag> = match rule {
            Some((key, _)) => errors
                .iter()
                .copied()
                .filter(|e| rule_of(&e.message).is_some_and(|(k, _)| k == key))
                .collect(),
            None => vec![d],
        };
        let better = rule.map(|(_, text)| text.to_string());
        let rule = rule.map(|(key, _)| key);
        if same.len() > 1 {
            use std::fmt::Write as _;
            let mut ask = format!(
                "Fix these {} errors of the same kind. Change only what the fixes need.\n",
                same.len()
            );
            let mut key = String::from("errors");
            for e in &same {
                let _ = writeln!(ask, "- {}, line {}: {}", e.file, e.line, e.message);
                let _ = write!(key, " {}:{} {}", e.file, e.line, e.message);
            }
            let lines: Vec<String> = same
                .iter()
                .map(|e| {
                    if e.file == d.file {
                        e.line.to_string()
                    } else {
                        format!("{} {}", e.file, e.line)
                    }
                })
                .collect();
            return Some(Noticed {
                key,
                what: format!(
                    "The same slip {} times in {}, lines {}. The first: {}.",
                    same.len(),
                    d.file,
                    list(&lines),
                    d.message.trim_end_matches('.')
                ),
                better,
                ask,
                rule,
            });
        }
        let at = format!("{} line {}", d.file, d.line);
        return Some(Noticed {
            key: format!("error {at} {}", d.message),
            what: format!("{at} does not compile: {}.", d.message.trim_end_matches('.')),
            better,
            ask: format!(
                "Fix this error in {}, line {}: {}. Change only what the fix needs.",
                d.file, d.line, d.message
            ),
            rule,
        });
    }
    if let Some(f) = seen.fault {
        return Some(Noticed {
            key: format!("fault {f}"),
            what: format!("The program stopped on a fault: {}.", f.trim_end_matches('.')),
            better: better_for_fault(f),
            ask: format!(
                "The program stops at run time with this fault: {f}. Change it so that cannot happen, and add a scenario that drives the inputs that caused it."
            ),
            rule: None,
        });
    }
    if let Some(first) = seen.held_by.first() {
        let names = seen.held_by.join(", ");
        return Some(Noticed {
            key: format!("held {names}"),
            what: format!(
                "The program is held with its outputs at zero: {names} {} lost, and the program does not say what to do then.",
                if seen.held_by.len() == 1 { "is" } else { "are" }
            ),
            better: Some(format!(
                "Declare a BOOL input {first}_ok. It reads FALSE while {first} is lost, the program keeps running, and it decides what is safe."
            )),
            ask: format!(
                "The device behind {names} can be lost. For each of these inputs declare a BOOL input NAME_ok, make the outputs safe while it is FALSE, and add a scenario that loses the input (NAME_absent) and checks the outputs."
            ),
            rule: None,
        });
    }
    let red = seen.tests.rows.iter().find(|r| r.state == State::Failed)?;
    Some(Noticed {
        key: format!("red {} {}", red.file, red.why),
        what: format!("A requirement is not met: {}", red.requirement),
        better: Some(format!("Its test says: {}.", red.why.trim_end_matches('.'))),
        ask: format!(
            "The requirement test {} is red: {}. Its requirement: {} Change the program so it passes. Do not change the test.",
            red.name(),
            red.why,
            red.requirement
        ),
        rule: None,
    })
}

/// `a`, `a and b`, `a, b and c`.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// The kind of slip a compile error is and the better way, from the
/// checker's own words: `(kind, the better way)`.
fn rule_of(message: &str) -> Option<(&'static str, &'static str)> {
    let m = message.to_ascii_lowercase();
    Some(if m.starts_with("undeclared variable") {
        (
            "undeclared",
            "Declare it in a VAR block with its type before using it, or correct the name if it is a slip.",
        )
    } else if m.starts_with("unknown function") || m.starts_with("unknown type") {
        (
            "unknown",
            "Use a name the standard library or this project defines; a function block needs an instance declared in VAR.",
        )
    } else if m.contains("convert explicitly") || m.contains("does not convert implicitly") {
        (
            "convert",
            "Structured Text never converts types silently. Write the conversion the message names, so the rounding is yours to see.",
        )
    } else if m.contains("is a function block type") || m.contains("call an instance") {
        (
            "instance",
            "A function block keeps state: declare an instance in VAR and call the instance as a statement.",
        )
    } else if m.starts_with("expected") && m.contains(":=") {
        ("assign", "Assignment is := in Structured Text; = compares.")
    } else if m.starts_with("expected") && m.contains(';') {
        ("semicolon", "Every statement ends with a semicolon.")
    } else if m.contains("is already defined") {
        (
            "redefined",
            "Each name is declared once: rename one of them, or remove the second declaration.",
        )
    } else if m.contains("has no input or output") {
        (
            "port",
            "Name the block's own inputs and outputs in the call; its internal variables are not visible.",
        )
    } else {
        return None;
    })
}

/// The better way for a run-time fault.
fn better_for_fault(fault: &str) -> Option<String> {
    let m = fault.to_ascii_lowercase();
    let rule = if m.contains("divi") && m.contains("zero") {
        "Check the divisor before dividing, and decide what the result is when it is zero."
    } else if m.contains("index") || m.contains("bounds") || m.contains("outside") {
        "Check the index against the array's bounds before using it."
    } else if m.contains("overflow") || m.contains("range") {
        "Limit the value before the operation (LIMIT), or use a wider type."
    } else {
        return None;
    };
    Some(rule.to_string())
}

/// When to say what was noticed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Watching {
    /// The problem standing now, and since when (page time, s).
    standing: Option<(String, f64)>,
    /// The problem the engineer said "Not now" to.
    dismissed: Option<String>,
    /// How often each kind of slip has been said this session (kept in
    /// memory only: it goes with the page).
    habits: std::collections::BTreeMap<&'static str, u32>,
    /// The problem last said, and how often its kind had come up by then.
    said: Option<(String, u32)>,
}

/// A kind of slip said this many times in a session is called a habit.
pub const HABIT: u32 = 3;

impl Watching {
    /// What to say at page time `t`, given what is `noticed` now and
    /// whether the page is `busy` (an edit not yet checked, the Agent at
    /// work): nothing until the same problem has stood [`QUIET`] seconds,
    /// and nothing for a problem that was dismissed, until it changes.
    pub fn say(&mut self, noticed: Option<Noticed>, busy: bool, t: f64) -> Option<Noticed> {
        let Some(n) = noticed else {
            self.standing = None;
            self.dismissed = None;
            // Fixed: the same slip made again is a new one to count.
            self.said = None;
            return None;
        };
        if self.dismissed.as_ref().is_some_and(|d| *d != n.key) {
            self.dismissed = None;
        }
        let since = match &self.standing {
            Some((key, since)) if *key == n.key => *since,
            _ => {
                self.standing = Some((n.key.clone(), t));
                t
            }
        };
        if busy {
            // The clock starts again once the page is at rest.
            self.standing = Some((n.key.clone(), t));
            return None;
        }
        if t - since < QUIET || self.dismissed.as_ref() == Some(&n.key) {
            return None;
        }
        // Newly said: its kind is counted once. A kind that keeps coming
        // up is said to be a habit.
        let times = match &self.said {
            Some((key, times)) if *key == n.key => *times,
            _ => {
                let times = n.rule.map_or(0, |rule| {
                    let c = self.habits.entry(rule).or_insert(0);
                    *c += 1;
                    *c
                });
                self.said = Some((n.key.clone(), times));
                times
            }
        };
        let mut n = n;
        if times >= HABIT {
            let note = format!("That is {times} times this session.");
            n.better = Some(match n.better.take() {
                Some(b) => format!("{b} {note}"),
                None => note,
            });
        }
        Some(n)
    }

    /// "Not now": `key` is not said again until something else is wrong.
    pub fn dismiss(&mut self, key: &str) {
        self.dismissed = Some(key.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::requirements::Row;

    fn error(message: &str) -> Diag {
        Diag {
            file: "main.st".into(),
            line: 7,
            col: 1,
            message: message.into(),
            severity: Severity::Error,
            id: None,
            blocking: false,
        }
    }

    fn red() -> Results {
        Results {
            rows: vec![Row {
                file: "test_lamp.st".into(),
                requirement: "The lamp is on within 2 s of start.".into(),
                state: State::Failed,
                why: "lamp on in time failed at 2.00 s".into(),
                scans: 200,
                elapsed_ms: 2000,
            }],
            ran: true,
        }
    }

    fn seen<'a>(
        diags: &'a [Diag],
        fault: Option<&'a str>,
        held: &'a [String],
        tests: &'a Results,
    ) -> Seen<'a> {
        Seen {
            diags,
            fault,
            held_by: held,
            tests,
        }
    }

    #[test]
    fn nothing_wrong_says_nothing() {
        let ok = Results::default();
        assert_eq!(noticed(&seen(&[], None, &[], &ok)), None);
    }

    #[test]
    fn an_error_is_said_with_where_the_better_way_and_a_fix_that_carries_it() {
        let ok = Results::default();
        let d = [error("undeclared variable lmap; did you mean lamp?")];
        let n = noticed(&seen(&d, None, &[], &ok)).unwrap();
        assert_eq!(
            n.what,
            "main.st line 7 does not compile: undeclared variable lmap; did you mean lamp?."
        );
        assert!(
            n.better
                .as_deref()
                .unwrap()
                .starts_with("Declare it in a VAR block")
        );
        assert!(n.ask.contains("main.st, line 7") && n.ask.contains("undeclared variable lmap"));
        // A warning alone is not said.
        let mut w = error("the result of ABS is unused");
        w.severity = Severity::Warning;
        assert_eq!(noticed(&seen(&[w], None, &[], &ok)), None);
    }

    #[test]
    fn the_most_urgent_thing_is_the_one_said() {
        let tests = red();
        let d = [error("expected ;, found END_IF")];
        let held = ["temp_c".to_string()];
        let all = noticed(&seen(&d, Some("division by zero"), &held, &tests)).unwrap();
        assert!(all.key.starts_with("error "), "{all:?}");
        assert_eq!(
            all.better.as_deref(),
            Some("Every statement ends with a semicolon.")
        );
        let fault = noticed(&seen(&[], Some("division by zero"), &held, &tests)).unwrap();
        assert!(
            fault
                .what
                .starts_with("The program stopped on a fault: division by zero")
        );
        assert!(fault.better.as_deref().unwrap().starts_with("Check the divisor"));
        let held_n = noticed(&seen(&[], None, &held, &tests)).unwrap();
        assert!(held_n.what.contains("temp_c is lost"), "{}", held_n.what);
        assert!(held_n.better.as_deref().unwrap().contains("temp_c_ok"));
        let red_n = noticed(&seen(&[], None, &[], &tests)).unwrap();
        assert_eq!(
            red_n.what,
            "A requirement is not met: The lamp is on within 2 s of start."
        );
        assert!(red_n.ask.contains("test_lamp") && red_n.ask.contains("Do not change the test"));
    }

    #[test]
    fn it_waits_for_the_typing_to_stop_and_takes_not_now_for_an_answer() {
        let ok = Results::default();
        let d = [error("undeclared variable x")];
        let n = || noticed(&seen(&d, None, &[], &ok));
        let mut w = Watching::default();
        assert_eq!(w.say(n(), false, 10.0), None, "just appeared");
        assert_eq!(w.say(n(), false, 10.0 + QUIET / 2.0), None);
        assert!(w.say(n(), false, 10.0 + QUIET).is_some());
        // Still typing (or the Agent at work): the wait starts again.
        assert_eq!(w.say(n(), true, 20.0), None);
        assert_eq!(w.say(n(), false, 20.0 + QUIET / 2.0), None);
        let said = w.say(n(), false, 20.0 + QUIET).unwrap();
        w.dismiss(&said.key);
        assert_eq!(w.say(n(), false, 99.0), None, "not now");
        // Something else goes wrong: that is said.
        let other = [error("undeclared variable y")];
        let m = || noticed(&seen(&other, None, &[], &ok));
        assert_eq!(w.say(m(), false, 100.0), None);
        assert!(w.say(m(), false, 100.0 + QUIET).is_some());
        // Fixed: nothing, and the first one would be said again.
        assert_eq!(w.say(None, false, 110.0), None);
        assert_eq!(w.say(n(), false, 111.0), None);
        assert!(w.say(n(), false, 111.0 + QUIET).is_some());
    }

    #[test]
    fn the_same_slip_in_several_places_is_said_once_with_one_fix_for_all() {
        let ok = Results::default();
        let mut d = vec![
            error("undeclared variable lmap; did you mean lamp?"),
            error("expected ;, found END_IF"),
            error("undeclared variable strat"),
            error("undeclared variable hron"),
        ];
        d[1].line = 9;
        d[2].line = 12;
        d[3].line = 15;
        let n = noticed(&seen(&d, None, &[], &ok)).unwrap();
        assert_eq!(
            n.what,
            "The same slip 3 times in main.st, lines 7, 12 and 15. The first: undeclared variable lmap; did you mean lamp?."
        );
        assert_eq!(n.rule, Some("undeclared"));
        assert!(
            n.better
                .as_deref()
                .unwrap()
                .starts_with("Declare it in a VAR block")
        );
        assert!(
            n.ask.starts_with("Fix these 3 errors of the same kind."),
            "{}",
            n.ask
        );
        for want in [
            "line 7: undeclared variable lmap",
            "line 12: undeclared variable strat",
            "line 15: undeclared variable hron",
        ] {
            assert!(n.ask.contains(want), "{want}: {}", n.ask);
        }
        assert!(
            !n.ask.contains("expected ;"),
            "the other kind is the next thing to say"
        );
        // An error no rule knows stands alone, however many there are.
        let odd = [error("field x appears twice"), error("field y appears twice")];
        let n = noticed(&seen(&odd, None, &[], &ok)).unwrap();
        assert!(
            n.what.starts_with("main.st line 7 does not compile"),
            "{}",
            n.what
        );
        assert_eq!(n.rule, None);
    }

    #[test]
    fn a_kind_of_slip_that_keeps_coming_up_is_called_a_habit() {
        let ok = Results::default();
        let mut w = Watching::default();
        let mut t = 0.0;
        let mut say = |w: &mut Watching, message: &str| {
            let d = [error(message)];
            let n = noticed(&seen(&d, None, &[], &ok));
            t += 10.0;
            assert_eq!(w.say(n.clone(), false, t), None);
            let said = w.say(n.clone(), false, t + QUIET).expect("said");
            // The same thing again next frame: counted once.
            assert_eq!(w.say(n, false, t + QUIET + 0.1), Some(said.clone()));
            // Fixed.
            assert_eq!(w.say(None, false, t + QUIET + 1.0), None);
            said
        };
        let first = say(&mut w, "undeclared variable a");
        assert!(!first.better.unwrap().contains("this session"));
        let second = say(&mut w, "undeclared variable b");
        assert!(!second.better.unwrap().contains("this session"));
        // Another kind in between does not count towards it.
        let other = say(&mut w, "expected ;, found END_IF");
        assert_eq!(
            other.better.as_deref(),
            Some("Every statement ends with a semicolon.")
        );
        let third = say(&mut w, "undeclared variable a");
        assert!(
            third
                .better
                .as_deref()
                .unwrap()
                .ends_with("That is 3 times this session."),
            "{:?}",
            third.better
        );
        // A problem no rule knows is never a habit.
        for _ in 0..4 {
            assert_eq!(say(&mut w, "field x appears twice").better, None);
        }
    }

    // ---- on the page ----

    use crate::editor::{Chrome, Pane};
    use crate::instruct_tests::{LAMP, mine};
    use eframe::egui;

    fn body_of_last(v: &crate::playground_view::PlaygroundView) -> String {
        v.link.sent.last().expect("a request").1.body.clone()
    }

    #[test]
    fn a_program_that_does_not_compile_is_noticed_and_fix_it_sends_it_to_the_agent() {
        let mut v = mine();
        v.notice_problems(1.0);
        v.notice_problems(1.0 + QUIET);
        assert_eq!(v.noticed, None, "nothing wrong with the lamp");
        v.proj
            .files
            .insert("main.st".into(), LAMP.replace("lamp := start;", "lmap := start;"));
        v.proj.save();
        v.open_project("mine");
        v.notice_problems(10.0);
        assert_eq!(v.noticed, None, "not the moment it appears");
        v.notice_problems(10.0 + QUIET);
        let n = v.noticed.clone().expect("noticed");
        assert!(
            n.what.contains("does not compile") && n.what.contains("LMAP"),
            "{}",
            n.what
        );
        let mut chrome = Chrome::default();
        chrome.pane = Pane::Code;
        let sent = v.link.sent.len();
        v.fix_noticed(&mut chrome, &egui::Context::default());
        assert_eq!(v.link.sent.len(), sent + 1, "one request, on the click");
        assert!(
            body_of_last(&v).contains("undeclared variable LMAP"),
            "{}",
            body_of_last(&v)
        );
        assert_eq!(chrome.pane, Pane::Claude, "the Agent tab shows it working");
        // While the Agent is at it, nothing more is said.
        v.notice_problems(50.0);
        v.notice_problems(50.0 + QUIET);
        assert_eq!(v.noticed, None);
    }

    #[test]
    fn a_red_requirement_is_noticed_in_its_own_words() {
        let mut v = mine();
        v.proj.files.insert(
            "main.st".into(),
            LAMP.replace("lamp := start;", "lamp := NOT start;"),
        );
        v.proj.save();
        v.open_project("mine");
        v.run_tests();
        v.notice_problems(10.0);
        v.notice_problems(10.0 + QUIET);
        let n = v.noticed.clone().expect("noticed");
        assert_eq!(n.what, "A requirement is not met: The lamp comes on with start.");
        assert!(n.ask.contains("test_lamp"), "{}", n.ask);
    }
}
