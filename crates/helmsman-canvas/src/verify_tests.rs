//! The Agent's answer checked before it is written (card
//! `agent-scenarios`): its scenarios read and run, the whole-file rule
//! held against fragments, and the retry decision. The tab's flow with
//! scripted answers is `instruct_tests.rs`.

use std::fmt::Write as _;

use crate::scope::{Files, Scope};
use crate::verify::{Check, Next, RETRIES, check, code_part, elision, full_copy, has_code, next, scenarios};

pub(crate) const LAMP: &str = "PROGRAM lamp\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := start;\nEND_PROGRAM\n";
/// The project's own test: green while the lamp follows start.
pub(crate) const FOLLOWS: &str = "(* The lamp comes on with start. *)\nPROGRAM test_lamp\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; lamp_follows : ASSERT; END_VAR\nstart := TRUE;\nn := n + 1;\nIF n > 3 THEN lamp_follows(COND := lamp); done := TRUE; END_IF;\nEND_PROGRAM\n";

/// A scenario: the lamp stays off without start (`holds` FALSE makes
/// it expect the lamp on, so it fails against `LAMP`).
pub(crate) fn off(name: &str, holds: bool) -> String {
    let cond = if holds { "NOT lamp" } else { "lamp" };
    format!(
        "(* The lamp stays off while start is not pressed. *)\nPROGRAM test_{name}\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; lamp_off : ASSERT; END_VAR\nstart := FALSE;\nn := n + 1;\nIF n > 3 THEN lamp_off(COND := {cond}); done := TRUE; END_IF;\nEND_PROGRAM"
    )
}

/// An answer: a few words, the code, then the scenarios.
pub(crate) fn answer_with(code: &str, scen: &[String]) -> String {
    let mut a = format!("It now says why.\n\n```st\n{}\n```\n", code.trim_end());
    for s in scen {
        let _ = write!(a, "\n```st scenario\n{s}\n```\n");
    }
    a
}

pub(crate) fn commented() -> String {
    LAMP.replace("lamp := start;", "lamp := start; (* follows start *)")
}

fn project() -> Files {
    Files::from([
        ("main.st".to_string(), LAMP.to_string()),
        ("test_lamp.st".to_string(), FOLLOWS.to_string()),
    ])
}

fn settings() -> helmsman_iec::spec::Settings {
    helmsman_iec::spec::Settings::new(crate::playground::SIZES)
}

fn file() -> Scope {
    Scope::File("main.st".into())
}

// ---- scenarios read from an answer ----

#[test]
fn scenarios_are_read_from_their_own_blocks_and_named_free_in_the_project() {
    let a = answer_with(&commented(), &[off("lamp", true), off("lamp_off", true)]);
    let taken = vec!["main.st".to_string(), "test_lamp.st".to_string()];
    let (s, problems) = scenarios(&a, &taken);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.len(), 2);
    assert_eq!(s[0].file, "test_lamp_2.st", "test_lamp.st is the project's own");
    assert_eq!(s[1].file, "test_lamp_off.st");
    assert_eq!(s[0].requirement, "The lamp stays off while start is not pressed.");
    assert!(
        s[0].text
            .starts_with("(* The lamp stays off while start is not pressed. *)\nPROGRAM test_lamp\n"),
        "{}",
        s[0].text
    );
    assert_eq!(
        helmsman_iec::spec::stated(&s[0].text).as_deref(),
        Some("The lamp stays off while start is not pressed.")
    );
    // The code is what the scope rules read; scenarios are not code.
    let code = code_part(&a);
    assert!(!code.contains("test_lamp"), "{code}");
    assert!(code.contains("(* follows start *)"));
    assert!(has_code(&a));
    let only = answer_with("", &[off("x", true)]).replace("```st\n\n```\n", "");
    assert!(!has_code(&only), "{only}");
}

#[test]
fn a_block_that_cannot_be_a_scenario_says_why() {
    let cases = [
        (
            "PROGRAM test_x\nEND_PROGRAM".to_string(),
            "does not say what it checks",
        ),
        (
            "(* words *)\nPROGRAM lamp_check\nEND_PROGRAM".to_string(),
            "must be test_",
        ),
        (
            "(* words *)\nPROGRAM test_9lives\nEND_PROGRAM".to_string(),
            "must be test_",
        ),
        ("(* words *)\nVAR x : INT; END_VAR".to_string(), "no PROGRAM"),
        ("   ".to_string(), "empty"),
        (
            "(* a\u{7}b *)\nPROGRAM test_x\nEND_PROGRAM".to_string(),
            "control characters",
        ),
        (
            format!("(* {} *)\nPROGRAM test_x\nEND_PROGRAM", "w".repeat(501)),
            "at most 500",
        ),
    ];
    for (body, why) in cases {
        let a = format!("```st scenario\n{body}\n```\n");
        let (s, p) = scenarios(&a, &[]);
        assert!(s.is_empty(), "{body}");
        assert!(p.len() == 1 && p[0].contains(why), "{body}: {p:?}");
    }
    let twice = answer_with(LAMP, &[off("a", true), off("a", true)]);
    let (s, p) = scenarios(&twice, &[]);
    assert_eq!(s.len(), 1);
    assert!(p[0].contains("both named test_a"), "{p:?}");
    let many: Vec<String> = (0..9).map(|k| off(&format!("s{k}"), true)).collect();
    let (s, p) = scenarios(&answer_with(LAMP, &many), &[]);
    assert!(s.is_empty() && p[0].contains("at most 8"), "{p:?}");
}

// ---- the whole file, never a fragment ----

#[test]
fn elision_markers_are_refused_unless_the_file_had_them() {
    let body = "lamp := start;";
    for marker in [
        "...",
        "…",
        "(* rest unchanged *)",
        "(* ... *)",
        "(* Rest of the code unchanged *)",
        "(* the other declarations, unchanged *)",
        "// same as before",
        "(* existing code here *)",
        "(* lines above unchanged *)",
        "(* remains the same *)",
        "(* code omitted for brevity *)",
        "(* and so on *)",
        "(*   REST   OF THE   PROGRAM *)",
    ] {
        let code = LAMP.replace(body, &format!("{body}\n{marker}"));
        assert!(elision(LAMP, &code).is_some(), "{marker}");
        let why = full_copy(&file(), LAMP, &code, None).expect(marker);
        assert!(why.contains("leaves part of main.st out"), "{why}");
    }
    // Comments of the engineer's own, kept, and plain words that say no
    // code is missing, are fine.
    let had = LAMP.replace(body, "lamp := start; (* rest unchanged ... *)");
    assert_eq!(elision(&had, &had), None);
    let fine = LAMP.replace(
        body,
        "lamp := start; (* the output holds unchanged while paused *)",
    );
    assert_eq!(elision(LAMP, &fine), None);
    assert_eq!(full_copy(&file(), LAMP, &fine, None), None);
}

#[test]
fn every_pou_of_the_file_comes_back_with_its_header_and_end() {
    let two = format!("{LAMP}FUNCTION_BLOCK Motor\nVAR_INPUT on : BOOL; END_VAR\nEND_FUNCTION_BLOCK\n");
    let why = full_copy(&file(), &two, LAMP, None).expect("Motor left out");
    assert!(why.contains("leaves out MOTOR"), "{why}");
    let no_end = two.replace("END_FUNCTION_BLOCK\n", "");
    let why = full_copy(&file(), &two, &no_end, None).expect("no END_");
    assert!(why.contains("opens 2 POU(s) and closes 1"), "{why}");
    let no_head = LAMP.replace("PROGRAM lamp\n", "");
    let why = full_copy(&file(), LAMP, &no_head, None).expect("no header");
    assert!(why.contains("leaves out LAMP"), "{why}");
    assert_eq!(
        full_copy(
            &file(),
            &two,
            &two.replace("on : BOOL", "on : BOOL; x : INT"),
            None
        ),
        None
    );
}

#[test]
fn a_selection_takes_the_whole_file_changed_only_inside_it() {
    let lines = Scope::Lines {
        file: "main.st".into(),
        from: 4,
        to: 4,
    };
    let files = project();
    // A fragment would be spliced in by the scope rules: refused here.
    let frag = "```st\nlamp := NOT NOT start;\n```\n";
    let c = check(&lines, &files, frag, settings());
    assert!(c.red(), "{c:?}");
    assert!(
        c.failures[0].contains("not the whole of main.st"),
        "{:?}",
        c.failures
    );
    // The whole file with the change in line 4: green (no scenario).
    let whole = LAMP.replace("lamp := start;", "lamp := NOT NOT start;");
    let c = check(&lines, &files, &answer_with(&whole, &[]), settings());
    assert!(c.green() && c.unchecked(), "{c:?}");
    // The whole file with a change outside line 4: refused by the scope.
    let outside = whole.replace("PROGRAM lamp", "PROGRAM lamp (* moved *)");
    let c = check(&lines, &files, &answer_with(&outside, &[]), settings());
    assert!(c.red(), "{c:?}");
    assert!(
        c.failures.iter().any(|f| f.contains("outside line 4 of main.st")),
        "{:?}",
        c.failures
    );
    // The section alone with its header and end, the body of a longer
    // file left out: a fragment too.
    let block = Scope::Block {
        file: "main.st".into(),
        name: "lamp".into(),
        from: 4,
        to: 4,
    };
    let c = check(
        &block,
        &files,
        "```st\nPROGRAM lamp\nlamp := start;\nEND_PROGRAM\n```",
        settings(),
    );
    assert!(c.red(), "{c:?}");
}

// ---- the check ----

#[test]
fn the_check_runs_the_scenarios_and_the_projects_tests_on_a_copy() {
    let files = project();
    let green = answer_with(&commented(), &[off("lamp_off", true)]);
    let c = check(&file(), &files, &green, settings());
    assert!(c.green() && !c.unchecked(), "{c:?}");
    assert_eq!(c.scenarios.len(), 1);
    assert!(c.scenarios[0].passed);
    assert_eq!(c.tests_green, 1, "test_lamp ran too");
    assert_eq!(c.add.len(), 1);
    assert_eq!(c.add[0].0, "test_lamp_off.st");
    // A red scenario, named in its words.
    let red = answer_with(&commented(), &[off("lamp_off", false)]);
    let c = check(&file(), &files, &red, settings());
    assert!(c.red());
    let p = c.problems().join("\n");
    assert!(
        p.contains("Scenario The lamp stays off while start is not pressed. (test_lamp_off.st) failed: lamp off failed"),
        "{p}"
    );
    // A scenario with no ASSERT checks nothing: red.
    let idle =
        "(* Nothing. *)\nPROGRAM test_idle\nVAR_OUTPUT done : BOOL; END_VAR\ndone := TRUE;\nEND_PROGRAM"
            .to_string();
    let c = check(&file(), &files, &answer_with(&commented(), &[idle]), settings());
    assert!(c.problems().join(" ").contains("checks nothing"), "{c:?}");
    // The project's own test goes red with the code: named.
    let invert = LAMP.replace("lamp := start;", "lamp := NOT start;");
    let c = check(&file(), &files, &answer_with(&invert, &[]), settings());
    assert!(c.failures.iter().any(|f| f.contains("test_lamp is red")), "{c:?}");
    // Code that does not compile, with or without tests.
    let broken = LAMP.replace("lamp := start;", "lamp := strat;");
    let c = check(
        &file(),
        &Files::from([("main.st".into(), LAMP.into())]),
        &answer_with(&broken, &[]),
        settings(),
    );
    assert!(c.failures.iter().any(|f| f.contains("does not compile")), "{c:?}");
    // No code: nothing to check.
    let c = check(&file(), &files, "The lamp follows start.", settings());
    assert_eq!(c, Check::default());
    assert!(!c.red() && !c.unchecked());
    // A scenario never touches the project's files.
    assert_eq!(files, project());
}

#[test]
fn a_new_projects_program_is_checked_with_its_scenarios() {
    let examples = Files::from([("main.st".into(), "PROGRAM blink\nEND_PROGRAM\n".into())]);
    let a = answer_with(LAMP, &[off("lamp_off", true)]);
    let c = check(&Scope::NewProject, &examples, &a, settings());
    assert!(c.green() && c.scenarios.len() == 1, "{c:?}");
    // The scenario is not taken as the new program.
    let only = format!("```st scenario\n{}\n```", off("lamp_off", true));
    assert!(!has_code(&only));
}

// ---- the retry decision ----

#[test]
fn a_red_answer_goes_back_twice_and_code_without_a_scenario_once() {
    let red = Check {
        code: true,
        failures: vec!["The code does not compile: 4:9: unknown variable STRAT".into()],
        ..Check::default()
    };
    for k in 0..RETRIES {
        let Next::Retry(m) = next(&red, k, false) else {
            panic!("retry {k}")
        };
        assert!(m.contains("unknown variable STRAT"), "{m}");
        assert!(m.contains("Answer again in full"), "{m}");
    }
    assert_eq!(next(&red, RETRIES, false), Next::Show, "then it is shown red");
    let bare = Check {
        code: true,
        ..Check::default()
    };
    assert!(matches!(next(&bare, 0, false), Next::Retry(m) if m.contains("no scenario")));
    assert_eq!(next(&bare, 1, true), Next::Show, "asked once already");
    assert_eq!(next(&Check::default(), 0, false), Next::Show, "no code, no check");
    let green = Check {
        code: true,
        scenarios: vec![crate::verify::Ran {
            file: "test_x.st".into(),
            requirement: "x".into(),
            passed: true,
            why: String::new(),
        }],
        ..Check::default()
    };
    assert_eq!(next(&green, 0, false), Next::Show);
}

/// The project's test, with the change that contradicts it: the lamp is
/// now off while start is on.
const INVERTED: &str = "PROGRAM lamp\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := NOT start;\nEND_PROGRAM\n";

fn inverted_test() -> String {
    "(* The lamp is off while start is on. *)\nPROGRAM test_lamp\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; lamp_off : ASSERT; END_VAR\nstart := TRUE;\nn := n + 1;\nIF n > 3 THEN lamp_off(COND := NOT lamp); done := TRUE; END_IF;\nEND_PROGRAM".into()
}

fn replacing(code: &str, block: &str, scen: &str) -> String {
    format!(
        "Why: the lamp is now off while start is on.\n\n```st\n{}\n```\n\n```{block}\n{scen}\n```\n",
        code.trim_end()
    )
}

#[test]
fn a_scenario_marked_replaces_names_the_test_it_takes_the_place_of() {
    use crate::verify::replaces;
    assert_eq!(
        replaces("st scenario replaces test_lamp").as_deref(),
        Some("test_lamp.st")
    );
    assert_eq!(
        replaces("st scenario replaces test_lamp.st").as_deref(),
        Some("test_lamp.st")
    );
    assert_eq!(
        replaces("st scenario REPLACES TEST_Lamp").as_deref(),
        Some("test_lamp.st")
    );
    assert_eq!(replaces("st scenario"), None);
    assert_eq!(replaces("st scenario replaces"), None);
    // Only a test file name: never main.st or a path.
    assert_eq!(replaces("st scenario replaces main"), None);
    assert_eq!(replaces("st scenario replaces test_../main"), None);
}

/// Nick, 2026-11-05: kept scenarios are tests, so an instruction that
/// changes their behaviour could never pass. The answer replaces the test
/// it contradicts; it must pass, and the status line names it.
#[test]
fn a_test_the_instruction_contradicts_is_replaced_and_said_so() {
    // Without the replacement: red, and the failure says how to replace it.
    let plain = answer_with(INVERTED, &[inverted_test().replace("test_lamp", "test_inverted")]);
    let c = check(&file(), &project(), &plain, settings());
    assert!(c.red());
    let p = c.problems().join("\n");
    assert!(p.contains("Requirement test test_lamp is red"), "{p}");
    assert!(p.contains("```st scenario replaces test_lamp"), "{p}");

    // With it: green, the old test's file takes the new text.
    let a = replacing(INVERTED, "st scenario replaces test_lamp", &inverted_test());
    let c = check(&file(), &project(), &a, settings());
    assert!(c.green(), "{:?}", c.problems());
    assert_eq!(
        c.replaced,
        vec![crate::verify::Replaced {
            file: "test_lamp.st".into(),
            was: "The lamp comes on with start.".into(),
            now: "The lamp is off while start is on.".into(),
        }]
    );
    assert_eq!(c.add.len(), 1);
    assert_eq!(c.add[0].0, "test_lamp.st");
    assert!(c.add[0].1.contains("NOT lamp"));
    let s = crate::instruct::status("main.st", &c);
    assert!(
        s.contains(
            "changed requirement test_lamp: The lamp is off while start is on (was: The lamp comes on with start)"
        ),
        "{s}"
    );

    // The replacement must itself pass: one that checks the old behaviour
    // against the new code is red.
    let wrong = replacing(INVERTED, "st scenario replaces test_lamp", FOLLOWS.trim_end());
    assert!(check(&file(), &project(), &wrong, settings()).red());

    // A test the project does not have cannot be replaced.
    let none = replacing(INVERTED, "st scenario replaces test_nope", &inverted_test());
    let c = check(&file(), &project(), &none, settings());
    assert!(
        c.problems()
            .iter()
            .any(|p| p.contains("replaces test_nope, which the project does not have")),
        "{:?}",
        c.problems()
    );
}

/// The lamp program with a second output, which leaves `FOLLOWS` green.
const LAMP_AND_FLAG: &str = "PROGRAM lamp\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; flag : BOOL; END_VAR\nlamp := start;\nflag := NOT start;\nEND_PROGRAM\n";

/// A scenario that reads `flag` and expects it on while start is off:
/// false on `LAMP` (no such output), true on `LAMP_AND_FLAG`.
fn flag_scenario(name: &str) -> String {
    format!(
        "(* The flag is on while start is not pressed. *)\nPROGRAM test_{name}\nVAR_INPUT flag : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; flag_on : ASSERT; END_VAR\nstart := FALSE;\nn := n + 1;\nIF n > 3 THEN flag_on(COND := flag); done := TRUE; END_IF;\nEND_PROGRAM"
    )
}

/// A test is replaced only when the new code turns it red. A trivial
/// replacement of a test that still passes is refused and the old test
/// stays (review of 2026-11-08, hole 2).
#[test]
fn a_test_that_still_passes_cannot_be_replaced() {
    let trivial = "(* Nothing to check. *)\nPROGRAM test_lamp\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT done : BOOL; END_VAR\nVAR ok : ASSERT; END_VAR\nok(COND := TRUE);\nlamp := lamp;\ndone := TRUE;\nEND_PROGRAM";
    let a = replacing(LAMP_AND_FLAG, "st scenario replaces test_lamp", trivial);
    let c = check(&file(), &project(), &a, settings());
    assert!(c.red(), "{:?}", c.problems());
    let p = c.problems().join("\n");
    assert!(p.contains("replaces test_lamp"), "{p}");
    assert!(p.contains("still passes with this code"), "{p}");
    assert!(c.replaced.is_empty() && c.add.is_empty(), "the old test stays");
    // The retry sends the reason back.
    let Next::Retry(m) = next(&c, 0, false) else {
        panic!("a retry")
    };
    assert!(m.contains("still passes with this code"), "{m}");
    // A scenario with a name of its own that tells the change apart is green.
    let a = answer_with(LAMP_AND_FLAG, &[flag_scenario("flag")]);
    let c = check(&file(), &project(), &a, settings());
    assert!(c.green(), "{:?}", c.problems());
    assert_eq!(c.tests_green, 1, "test_lamp ran and passed");
}

/// Red before, green after: scenarios that pass on the old file say
/// nothing about a change that alters what the file does.
#[test]
fn a_scenario_that_passes_on_the_old_file_does_not_distinguish_the_change() {
    // Both pass on LAMP too: refused, and the retry text says so.
    let a = answer_with(LAMP_AND_FLAG, &[off("lamp_off", true)]);
    let c = check(&file(), &project(), &a, settings());
    assert!(c.red(), "{:?}", c.problems());
    let p = c.problems().join("\n");
    assert!(p.contains("does not distinguish the change"), "{p}");
    assert!(p.contains("lamp_off"), "{p}");
    assert!(matches!(next(&c, 0, false), Next::Retry(m) if m.contains("does not distinguish")));
    // One that fails on the old file and passes on the new one: green.
    let a = answer_with(LAMP_AND_FLAG, &[off("lamp_off", true), flag_scenario("flag")]);
    let c = check(&file(), &project(), &a, settings());
    assert!(c.green(), "{:?}", c.problems());
    // A change in comments alone changes nothing to tell apart.
    let a = answer_with(&commented(), &[off("lamp_off", true)]);
    assert!(check(&file(), &project(), &a, settings()).green());
}
