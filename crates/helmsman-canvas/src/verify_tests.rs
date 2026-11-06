//! The Agent's answer checked before it is shown (card `agent-scenarios`):
//! its scenarios read and run, the whole-file rule held against fragments,
//! the retry decision, and the Agent tab's flow with scripted answers (no
//! network, no sleeps: `link.sent` and `link.inbox`).

use std::fmt::Write as _;

use crate::scope::{Files, Scope};
use crate::verify::{Check, Next, RETRIES, check, code_part, elision, full_copy, has_code, next, scenarios};

const LAMP: &str = "PROGRAM lamp\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\nlamp := start;\nEND_PROGRAM\n";
/// The project's own test: green while the lamp follows start.
const FOLLOWS: &str = "(* The lamp comes on with start. *)\nPROGRAM test_lamp\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; lamp_follows : ASSERT; END_VAR\nstart := TRUE;\nn := n + 1;\nIF n > 3 THEN lamp_follows(COND := lamp); done := TRUE; END_IF;\nEND_PROGRAM\n";

/// A scenario: the lamp stays off without start (`holds` FALSE makes
/// it expect the lamp on, so it fails against `LAMP`).
fn off(name: &str, holds: bool) -> String {
    let cond = if holds { "NOT lamp" } else { "lamp" };
    format!(
        "(* The lamp stays off while start is not pressed. *)\nPROGRAM test_{name}\nVAR_INPUT lamp : BOOL; END_VAR\nVAR_OUTPUT start : BOOL; done : BOOL; END_VAR\nVAR n : INT; lamp_off : ASSERT; END_VAR\nstart := FALSE;\nn := n + 1;\nIF n > 3 THEN lamp_off(COND := {cond}); done := TRUE; END_IF;\nEND_PROGRAM"
    )
}

/// An answer: a few words, the code, then the scenarios.
fn answer_with(code: &str, scen: &[String]) -> String {
    let mut a = format!("It now says why.\n\n```st\n{}\n```\n", code.trim_end());
    for s in scen {
        let _ = write!(a, "\n```st scenario\n{s}\n```\n");
    }
    a
}

fn commented() -> String {
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
    assert_eq!(c.refusal(), None);
    // A red scenario, named in its words.
    let red = answer_with(&commented(), &[off("lamp_off", false)]);
    let c = check(&file(), &files, &red, settings());
    assert!(c.red());
    let p = c.problems().join("\n");
    assert!(
        p.contains("Scenario The lamp stays off while start is not pressed. (test_lamp_off.st) failed: lamp off failed"),
        "{p}"
    );
    assert!(c.refusal().is_some_and(|r| r.contains("did not pass")));
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

// ---- the Agent tab ----

mod in_the_page {
    use eframe::egui;
    use serde_json::json;

    use super::{FOLLOWS, LAMP, answer_with, commented, off};
    use crate::claude::{Incoming, Phase};
    use crate::playground_view::PlaygroundView;

    /// Project `mine`: `LAMP` and its green test, on the local proxy.
    fn mine() -> PlaygroundView {
        let mut v = PlaygroundView::with_storage(Box::new(crate::sim_projects::MemStorage::default()));
        v.proj.create("mine", None);
        v.proj.files.insert("main.st".into(), LAMP.into());
        v.proj.add_file("test_lamp.st", FOLLOWS.into());
        v.proj.save();
        v.open_project("mine");
        v.proj.open_file("main.st");
        // The first tick settles the proxy check (none natively); then the
        // proxy is there.
        v.tick(&egui::Context::default());
        v.chat.receive(Incoming::Proxy(true));
        v
    }

    /// A Messages API stream whose one text block is `text`.
    fn stream(text: &str) -> String {
        let ev = |name: &str, data: serde_json::Value| format!("event: {name}\ndata: {data}\n\n");
        [
            ev("message_start", json!({"type":"message_start","message":{"model":"claude-sonnet-5-5","id":"m","type":"message","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":1}}})),
            ev("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            ev("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})),
            ev("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ev("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":40}})),
            ev("message_stop", json!({"type":"message_stop"})),
        ]
        .concat()
    }

    /// The Agent answers the request in flight with `text`.
    fn reply(v: &mut PlaygroundView, text: &str) {
        let (seq, _) = v.link.sent.last().cloned().expect("a request");
        v.link.inbox.push(Incoming::Chunk {
            seq,
            text: stream(text),
        });
        v.link.inbox.push(Incoming::End { seq });
        v.tick(&egui::Context::default());
    }

    fn ask(v: &mut PlaygroundView, q: &str) {
        v.chat.question = q.into();
        v.ask(&egui::Context::default());
        assert_eq!(v.ask_error, None);
    }

    /// The last request's messages.
    fn messages(v: &PlaygroundView) -> Vec<serde_json::Value> {
        let (_, req) = v.link.sent.last().unwrap();
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        body["messages"].as_array().unwrap().clone()
    }

    #[test]
    fn a_green_answer_shows_its_scenarios_and_use_keeps_them_as_tests() {
        let mut v = mine();
        ask(&mut v, "say why the lamp is on");
        reply(&mut v, &answer_with(&commented(), &[off("lamp_off", true)]));
        assert_eq!(v.link.sent.len(), 1, "green the first time: no retry");
        assert!(v.chain.is_none());
        let (c, tries) = v.live_check.clone().expect("checked");
        assert!(c.green() && c.scenarios.len() == 1 && tries == 0, "{c:?}");
        assert_eq!(v.chat.history.len(), 1);
        assert_eq!(v.chat.history[0].question, "say why the lamp is on");
        assert!(v.use_answer());
        assert_eq!(v.use_error, None, "{:?}", v.notice);
        assert_eq!(v.proj.files["main.st"], commented());
        let t = &v.proj.files["test_lamp_off.st"];
        assert!(
            t.starts_with("(* The lamp stays off while start is not pressed. *)\nPROGRAM test_lamp_off"),
            "{t}"
        );
        assert!(
            v.notice
                .as_deref()
                .is_some_and(|n| n.contains("scenario is kept")),
            "{:?}",
            v.notice
        );
        let row = v
            .tests
            .rows
            .iter()
            .find(|r| r.file == "test_lamp_off.st")
            .expect("its row");
        assert!(row.passed(), "{row:?}");
        assert_eq!(row.requirement, "The lamp stays off while start is not pressed.");
    }

    #[test]
    fn a_red_answer_goes_back_with_what_failed_and_the_fix_is_shown() {
        let mut v = mine();
        ask(&mut v, "say why the lamp is on");
        let first = answer_with(&commented(), &[off("lamp_off", false)]);
        reply(&mut v, &first);
        assert_eq!(v.link.sent.len(), 2, "sent back on its own");
        let c = v.chain.as_ref().expect("retrying");
        assert_eq!(c.retries, 1);
        assert!(
            c.problems.iter().any(|p| p.contains("lamp off failed")),
            "{:?}",
            c.problems
        );
        assert!(v.chat.busy());
        assert!(v.chat.history.is_empty(), "the failed answer is no page");
        let m = messages(&v);
        assert_eq!(m.len(), 3, "the question, the failed answer, what failed");
        assert!(
            m[0]["content"]
                .as_str()
                .unwrap()
                .starts_with("say why the lamp is on")
        );
        assert_eq!(m[1]["content"], first.as_str());
        let back = m[2]["content"].as_str().unwrap();
        assert!(
            back.starts_with("The page checked your answer before showing it"),
            "{back}"
        );
        assert!(
            back.contains("test_lamp_off.st) failed: lamp off failed"),
            "{back}"
        );
        assert!(back.contains("<edit_scope>"), "the same envelopes: {back}");
        // The fix passes: shown, as the page of the first question.
        reply(&mut v, &answer_with(&commented(), &[off("lamp_off", true)]));
        assert_eq!(v.link.sent.len(), 2);
        assert!(v.chain.is_none());
        let (c, tries) = v.live_check.clone().unwrap();
        assert!(c.green() && tries == 1, "{c:?}");
        assert_eq!(v.chat.history.len(), 1);
        assert_eq!(v.chat.history[0].question, "say why the lamp is on");
        assert!(v.chat.history[0].asked.starts_with("say why the lamp is on"));
        assert!(v.use_answer());
        assert_eq!(v.use_error, None, "{:?}", v.notice);
    }

    #[test]
    fn still_red_after_two_retries_it_is_shown_red_and_refused() {
        let mut v = mine();
        let before = v.proj.files.clone();
        ask(&mut v, "say why the lamp is on");
        let red = answer_with(&commented(), &[off("lamp_off", false)]);
        for k in 0..3 {
            assert_eq!(v.link.sent.len(), k + 1);
            reply(&mut v, &red);
        }
        assert_eq!(v.link.sent.len(), 3, "two retries, no more");
        assert_eq!(messages(&v).len(), 5, "both failed answers went back");
        assert!(v.chain.is_none());
        let (c, tries) = v.live_check.clone().unwrap();
        assert!(c.red() && tries == 2, "{c:?}");
        assert_eq!(v.chat.history.len(), 1);
        assert!(v.use_answer());
        let why = v.use_error.clone().expect("refused");
        assert!(
            why.contains("did not pass") && why.contains("lamp off failed"),
            "{why}"
        );
        assert_eq!(v.proj.files["main.st"], before["main.st"]);
        assert!(!v.proj.files.contains_key("test_lamp_off.st"), "no scenario kept");
    }

    #[test]
    fn a_fragment_of_the_selection_goes_back_and_the_whole_file_is_taken() {
        let mut v = mine();
        v.selection = Some(("main.st".into(), 4, 4));
        ask(&mut v, "comment line 4");
        let asked = messages(&v)[0]["content"].as_str().unwrap().to_string();
        assert!(
            asked.contains(&format!("section:\nlamp := start;\ntext:\n{LAMP}")),
            "{asked}"
        );
        reply(
            &mut v,
            &answer_with("lamp := start; (* follows start *)", &[off("lamp_off", true)]),
        );
        assert_eq!(v.link.sent.len(), 2, "a fragment goes back");
        let back = messages(&v).last().unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(back.contains("not the whole of main.st"), "{back}");
        reply(&mut v, &answer_with(&commented(), &[off("lamp_off", true)]));
        let (c, _) = v.live_check.clone().unwrap();
        assert!(c.green(), "{c:?}");
        v.selection = None;
        assert!(v.use_answer());
        assert_eq!(v.use_error, None, "{:?}", v.notice);
        assert_eq!(v.proj.files["main.st"], commented());
    }

    #[test]
    fn an_answer_with_no_code_needs_no_scenario() {
        let mut v = mine();
        ask(&mut v, "what does the lamp do?");
        reply(&mut v, "It follows start.");
        assert_eq!(v.link.sent.len(), 1);
        assert!(v.chain.is_none());
        assert_eq!(v.live_check.as_ref().map(|(c, _)| c.code), Some(false));
        assert!(!v.use_answer());
    }

    #[test]
    fn code_with_no_scenario_is_asked_for_one_once_then_shown_unchecked() {
        let mut v = mine();
        ask(&mut v, "comment it");
        reply(&mut v, &answer_with(&commented(), &[]));
        assert_eq!(v.link.sent.len(), 2);
        let back = messages(&v).last().unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(back.contains("no scenario"), "{back}");
        reply(&mut v, &answer_with(&commented(), &[]));
        assert_eq!(v.link.sent.len(), 2, "asked once");
        let (c, _) = v.live_check.clone().unwrap();
        assert!(c.unchecked(), "{c:?}");
        assert!(v.use_answer());
        assert_eq!(v.use_error, None, "not refused: {:?}", v.notice);
        assert_eq!(v.proj.files["main.st"], commented());
    }

    #[test]
    fn a_retry_stopped_brings_the_last_answer_back_as_the_page() {
        let mut v = mine();
        ask(&mut v, "comment it");
        let red = answer_with(&commented(), &[off("lamp_off", false)]);
        reply(&mut v, &red);
        assert!(v.chat.busy());
        v.stop_answer();
        v.tick(&egui::Context::default());
        assert!(v.chain.is_none());
        assert_eq!(v.chat.history.len(), 1);
        assert_eq!(v.chat.history[0].question, "comment it");
        assert_eq!(v.chat.history[0].answer, red);
        assert_eq!(v.chat.page, Some(0), "on view as a page");
        assert!(matches!(
            v.chat.answer.as_ref().map(|a| &a.phase),
            Some(Phase::Failed(_))
        ));
        let (c, _) = v.live_check.clone().unwrap();
        assert!(c.red());
        // An earlier page with code is checked too: Use this code refuses it.
        let page = v.chat.history[0].clone();
        v.use_page(&page);
        assert!(v.use_error.as_deref().is_some_and(|e| e.contains("did not pass")));
    }
}
