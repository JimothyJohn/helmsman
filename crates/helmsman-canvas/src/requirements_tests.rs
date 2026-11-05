//! The Requirements tab in the simulation: a requirement typed becomes a
//! test file behind its row, written by the page's Agent route when it can
//! be reached (a scripted Messages API answer here, compiled and run for
//! real by `helmsman_iec::spec`) and as "no test yet" when it cannot.

use eframe::egui;
use serde_json::json;

use crate::claude::Incoming;
use crate::playground_view::PlaygroundView;
use crate::requirements::{Phase, State};
use crate::sim_projects_tests::Shared;

const KEY: &str = "sk-ant-api03-TESTKEY-not-a-real-key-0123456789";

/// A page with a latch program of the visitor's own open.
fn page() -> PlaygroundView {
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    v.proj.create("mine", Some("latch"));
    v.open_project("mine");
    assert!(!v.proj.is_example());
    // The check for a local proxy settles (none here).
    v.tick(&egui::Context::default());
    v
}

/// A Messages API stream whose one text block is `text`.
fn answer(text: &str) -> String {
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

#[test]
fn with_no_agent_a_requirement_is_added_with_no_test_yet() {
    let ctx = egui::Context::default();
    let mut v = page();
    v.add_requirement("The motor runs once start is pressed", &ctx);
    let a = v.adding.as_ref().unwrap();
    let Phase::Untested { file, why } = &a.phase else {
        panic!("{:?}", a.phase)
    };
    assert_eq!(file, "test_the_motor_runs_once_start_is_pressed.st");
    assert!(why.contains("API key"), "{why}");
    let text = &v.proj.files[file];
    assert!(
        text.starts_with("(* The motor runs once start is pressed *)\n"),
        "{text}"
    );
    let row = v.tests.rows.iter().find(|r| &r.file == file).expect("its row");
    assert_eq!(row.state, State::Untested);
    assert_eq!(row.requirement, "The motor runs once start is pressed");
    assert_eq!(v.tests.refusal(), None, "no test yet never refuses a deploy");
    assert_eq!(v.link.sent.len(), 0, "nothing went anywhere");
}

#[test]
fn the_agents_test_is_written_behind_the_requirement_and_run() {
    let ctx = egui::Context::default();
    let mut v = page();
    v.chat.key_mut().push_str(KEY);
    v.add_requirement("The motor runs once start is pressed", &ctx);
    let (seq, req) = v.link.sent.last().cloned().expect("one request");
    assert!(req.body.contains("<box-data>"), "the program goes as data");
    assert!(req.body.contains("PROGRAM latch"), "{}", req.body);
    assert!(req.body.contains("test_the_motor_runs_once_start_is_pressed"));
    assert!(
        req.body.contains("You write requirement tests"),
        "the test writer's prompt"
    );
    assert!(v.adding.as_ref().unwrap().busy());

    let test = "PROGRAM test_the_motor_runs_once_start_is_pressed
VAR_INPUT motor : BOOL; END_VAR
VAR_OUTPUT start : BOOL; stop : BOOL; done : BOOL; END_VAR
VAR n : INT; motor_on : ASSERT; END_VAR
start := TRUE;
n := n + 1;
IF n > 3 THEN
  motor_on(COND := motor);
  done := TRUE;
END_IF;
END_PROGRAM";
    let text = format!("```st\n{test}\n```\nIt checks the motor after start.");
    v.link.inbox.push(Incoming::Chunk {
        seq,
        text: answer(&text),
    });
    v.link.inbox.push(Incoming::End { seq });
    v.tick(&ctx);
    let a = v.adding.as_ref().unwrap();
    assert_eq!(
        a.phase,
        Phase::Written {
            file: "test_the_motor_runs_once_start_is_pressed.st".into(),
            state: State::Passed,
            why: String::new()
        }
    );
    let f = &v.proj.files["test_the_motor_runs_once_start_is_pressed.st"];
    assert!(
        f.starts_with("(* The motor runs once start is pressed *)\nPROGRAM test_"),
        "{f}"
    );
    assert_eq!(v.chat.answer, None, "the Agent tab's conversation never saw it");
}

#[test]
fn an_answer_with_the_wrong_program_is_added_with_no_test_yet() {
    let ctx = egui::Context::default();
    let mut v = page();
    v.chat.key_mut().push_str(KEY);
    v.add_requirement("The motor stops on stop", &ctx);
    let (seq, _) = v.link.sent.last().cloned().unwrap();
    let text = "```st\nPROGRAM something_else\nEND_PROGRAM\n```";
    v.link.inbox.push(Incoming::Chunk {
        seq,
        text: answer(text),
    });
    v.link.inbox.push(Incoming::End { seq });
    v.tick(&ctx);
    assert!(
        matches!(&v.adding.as_ref().unwrap().phase, Phase::Untested { why, .. } if why.contains("no test")),
        "{:?}",
        v.adding.as_ref().unwrap().phase
    );
    let f = &v.proj.files["test_the_motor_stops_on_stop.st"];
    assert!(
        !f.contains("something_else"),
        "the wrong program is never written"
    );
}

#[test]
fn an_example_takes_no_requirement_until_it_is_your_own() {
    let ctx = egui::Context::default();
    let mut v = PlaygroundView::with_storage(Box::new(Shared::default()));
    assert!(v.proj.is_example());
    v.add_requirement("The lamp blinks", &ctx);
    assert!(v.adding.is_none());
    assert!(!v.proj.files.keys().any(|f| f.starts_with("test_")));
}

#[test]
fn requirements_are_held_to_one_line_of_at_most_500_characters() {
    use crate::requirements::{name_for, refusal};
    assert!(refusal("  ").is_some());
    assert!(refusal("a\nb").is_some());
    assert!(refusal(&"é".repeat(501)).is_some());
    assert_eq!(refusal(&"é".repeat(500)), None);
    let taken = vec!["test_lamp_on.st".to_string()];
    assert_eq!(name_for("Lamp on!", &taken), "lamp_on_2");
    assert_eq!(name_for("Lamp off", &taken), "lamp_off");
}
