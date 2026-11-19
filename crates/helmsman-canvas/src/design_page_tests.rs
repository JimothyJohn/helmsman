//! The design step on the simulation's page (`crate::design_step`,
//! `PlaygroundView`): the request the relay is sent, the design kept with
//! the project as `design.md`, Build handing the first batch of the build
//! order to the steps run with the design's briefs, a closed or spent
//! design entry leaving the chat alone. Scripted streams, no network, no
//! sleeps (`design_link.sent`, `design_link.inbox`, `link.sent`).

use eframe::egui;
use serde_json::{Value as Json, json};

use crate::claude::Incoming;
use crate::design;
use crate::design_step::DESIGN_FILE;
use crate::instruct::{Kind, Mode, PlanState};
use crate::instruct_tests::{LAMP, horn_follows, latches, mine, reply, st};
use crate::playground_view::PlaygroundView;

const LINE_1: &str = include_str!("fixtures/design/bottling-line-1.md");
const LINE_2: &str = include_str!("fixtures/design/bottling-line-2.md");
const LAMP_DESIGN: &str = include_str!("fixtures/design/lamp.md");
const OPEN: &str = r#"{"open": true, "agents": ["design"], "design": {"open": true}}"#;

fn ctx() -> egui::Context {
    egui::Context::default()
}

fn ev(name: &str, data: &Json) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// One relay turn with `text`, finished (`relay_usage` and all).
fn relay_turn(text: &str, stop: &str) -> String {
    [
        ev("message_start", &json!({"type":"message_start","message":{"model":"m","id":"m","type":"message","role":"assistant","content":[],"stop_reason":null}})),
        ev("content_block_start", &json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),
        ev("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Sorting out the stations."}})),
        ev("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"c2ln"}})),
        ev("content_block_stop", &json!({"type":"content_block_stop","index":0})),
        ev("content_block_start", &json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}})),
        ev("content_block_delta", &json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":text}})),
        ev("content_block_stop", &json!({"type":"content_block_stop","index":1})),
        ev("message_delta", &json!({"type":"message_delta","delta":{"stop_reason":stop}})),
        ev("message_stop", &json!({"type":"message_stop"})),
        ev("relay_usage", &json!({"type":"relay_usage"})),
    ]
    .concat()
}

/// The design entry answers the request in flight.
fn design_reply(v: &mut PlaygroundView, text: &str, stop: &str) {
    let (seq, _) = v.design_link.sent.last().cloned().expect("a design request");
    v.design_link.inbox.push(Incoming::Chunk {
        seq,
        text: relay_turn(text, stop),
    });
    v.design_link.inbox.push(Incoming::End { seq });
    v.tick(&ctx());
}

/// A page with the design entry served at a made-up address.
fn served() -> PlaygroundView {
    let mut v = mine();
    v.design.set_base("https://design.example");
    v.design_link.inbox.push(Incoming::DesignStatus(OPEN.into()));
    v.tick(&ctx());
    assert_eq!(
        v.design_link.design_checks,
        ["https://design.example"],
        "asked once"
    );
    assert!(v.design.visible());
    v
}

fn asked(v: &mut PlaygroundView, description: &str) {
    v.design.description = description.into();
    v.send_design(&ctx());
}

fn body_of(v: &PlaygroundView, k: usize) -> Json {
    serde_json::from_str(&v.design_link.sent[k].1.body).unwrap()
}

/// The text of the last chat request's last user message.
fn last_user(v: &PlaygroundView) -> String {
    let (_, req) = v.link.sent.last().unwrap();
    let body: Json = serde_json::from_str(&req.body).unwrap();
    body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

fn horn() -> String {
    LAMP.replace("  lamp : BOOL;\n", "  lamp : BOOL;\n  horn : BOOL;\n")
        .replace("lamp := start;\n", "lamp := start;\nhorn := start;\n")
}

fn horn_latched() -> String {
    horn().replace("lamp := start;\n", "lamp := start OR lamp;\n")
}

fn answer(code: &str, scenario: &str) -> String {
    format!("Why: step done.\n\n```st\n{code}```\n\n{scenario}")
}

#[test]
fn the_request_carries_the_machine_and_the_project_as_data_and_nothing_else() {
    let mut v = served();
    asked(&mut v, "A lamp that goes out by itself after ten minutes.");
    assert_eq!(v.design_link.sent.len(), 1, "{:?}", v.design.error);
    let (_, req) = &v.design_link.sent[0];
    assert_eq!(req.url, "https://design.example/demo/v1/messages");
    assert!(
        req.headers().iter().all(|(n, _)| *n == "content-type"),
        "no key and no other header goes to the design entry: {:?}",
        req.headers()
    );
    let b = body_of(&v, 0);
    assert_eq!(b.as_object().unwrap().len(), 2);
    assert_eq!(b["agent"], "design");
    let m = b["messages"][0]["content"].as_str().unwrap();
    assert!(
        m.starts_with("A lamp that goes out by itself after ten minutes.\n\n<dialect>"),
        "{m}"
    );
    assert!(
        m.contains("file: main.st\nPROGRAM lamp"),
        "the program goes as project data: {m}"
    );
    assert!(m.contains("<requirement_tests>\nfile: test_lamp.st"), "{m}");
    assert!(!v.design.open, "the window closes once it is sent");
    // While it runs nothing else starts.
    asked(&mut v, "Another.");
    assert_eq!(v.design_link.sent.len(), 1);
    assert_eq!(v.design.error.as_deref(), Some("A design is being written."));
}

#[test]
fn a_design_streams_in_is_kept_as_design_md_and_the_project_has_it_after() {
    let mut v = served();
    asked(&mut v, "A lamp.");
    design_reply(&mut v, LAMP_DESIGN, "end_turn");
    let read = v.design.read.as_ref().expect("on show");
    assert_eq!(read.problems, Vec::<String>::new());
    assert!(
        !v.proj.files.contains_key(DESIGN_FILE),
        "nothing is kept before Keep"
    );
    let revs = v.proj.revisions().len();
    v.keep_design();
    assert_eq!(v.proj.files[DESIGN_FILE], LAMP_DESIGN, "word for word");
    assert!(v.design.read.is_none() && !v.design.window);
    assert_eq!(v.proj.revisions().len(), revs + 1, "one revision");
    assert!(
        v.proj
            .revisions()
            .last()
            .unwrap()
            .what
            .starts_with("Kept the design: The lamp lights"),
        "{}",
        v.proj.revisions().last().unwrap().what
    );
    assert!(v.proj.revisions().last().unwrap().files.contains_key(DESIGN_FILE));
    assert_eq!(v.proj.files["main.st"], LAMP, "the program is as it was");
    assert!(
        v.notice
            .as_deref()
            .unwrap()
            .contains("Kept the design as design.md in mine")
    );
    let k = v.kept_design_for_test().unwrap();
    assert_eq!((k.total, k.built, k.next), (2, 0, Some((0, 2))));
}

#[test]
fn a_design_asked_for_with_an_example_open_is_kept_in_a_project_of_its_own() {
    let mut v = PlaygroundView::with_storage(Box::new(crate::sim_projects::MemStorage::default()));
    v.design.set_base("https://design.example");
    v.design_link.inbox.push(Incoming::DesignStatus(OPEN.into()));
    v.tick(&ctx());
    assert!(v.proj.is_example());
    asked(&mut v, "A bottling line: conveyor, rinser and filler.");
    design_reply(&mut v, LAMP_DESIGN, "end_turn");
    v.keep_design();
    assert!(!v.proj.is_example(), "an example is read-only: {}", v.proj.open);
    assert_eq!(v.proj.open, "bottling-line-conveyor-rinser");
    assert!(v.proj.files.contains_key(DESIGN_FILE));
    assert!(
        v.proj.files.contains_key("main.st"),
        "a starter program to build into"
    );
}

#[test]
fn a_design_the_project_cannot_hold_is_not_kept_and_says_why() {
    let mut v = served();
    asked(&mut v, "A lamp.");
    // Streamed in pieces, as a model does: one event line has a bound.
    for _ in 0..5 {
        design_reply(&mut v, &"x".repeat(60 * 1024), "max_tokens");
    }
    assert!(
        v.design.read.is_none(),
        "five cut turns are too large for the relay"
    );
    v.design.read = Some(crate::design_step::Read::new(
        "x".repeat(helmsman_project::MAX_FILE + 10),
        1,
        "A lamp.".into(),
    ));
    let before = v.proj.revisions().len();
    v.keep_design();
    assert!(!v.proj.files.contains_key(DESIGN_FILE));
    assert_eq!(v.proj.revisions().len(), before);
    assert!(v.design.notice.as_deref().unwrap().contains("cannot be kept"));
    assert!(v.design.read.is_some(), "still there to read");
}

#[test]
fn build_hands_the_first_batch_of_a_big_design_to_the_steps_run_with_the_designs_briefs() {
    let mut v = mine();
    let text = design::join([LINE_1, LINE_2]);
    v.proj.files.insert(DESIGN_FILE.into(), text.clone());
    let d = design::parse(&text);
    assert_eq!(d.steps.len(), 19);
    let k = v.kept_design_for_test().unwrap();
    assert_eq!((k.parts, k.total, k.built, k.next), (11, 19, 0, Some((0, 6))));
    assert_eq!(k.problems, Vec::<String>::new());
    let sent = v.link.sent.len();
    v.build_design(&ctx());
    assert_eq!(v.link.sent.len(), sent + 1, "step 1 goes at once");
    let Mode::Steps(p) = &v.instruct.run.as_ref().unwrap().mode else {
        panic!("a run in steps")
    };
    assert_eq!(p.steps.len(), 6, "MAX_STEPS, not the 19 of the design");
    assert_eq!(p.of, Some((0, 19)));
    assert_eq!(p.briefs.len(), 6);
    for (i, b) in p.briefs.iter().enumerate() {
        assert_eq!(Some(b.as_str()), d.step_brief(i).as_deref());
    }
    let asked = last_user(&v);
    assert!(
        asked.starts_with(&d.step_brief(0).unwrap()),
        "the brief is the step's instruction, as it is: {asked}"
    );
    assert!(
        !asked.contains("Step 1 of 6 of a larger change"),
        "not the plan's own words"
    );
    assert_eq!(
        v.instruct
            .run
            .as_ref()
            .unwrap()
            .instruction
            .chars()
            .take(26)
            .collect::<String>(),
        "Design steps 1 to 6 of 19:"
    );
    // The next batch starts after the ones built.
    v.stop_answer();
    assert!(!v.instruct.busy());
    assert_eq!(
        v.kept_design_for_test().unwrap().built,
        0,
        "a stopped batch built nothing"
    );
    v.design_built_for_test(6);
    v.build_design(&ctx());
    assert!(last_user(&v).starts_with(&d.step_brief(6).unwrap()));
    let Mode::Steps(p) = &v.instruct.run.as_ref().unwrap().mode else {
        panic!("steps")
    };
    assert_eq!(p.of, Some((6, 19)));
    v.stop_answer();
    v.design_built_for_test(18);
    v.build_design(&ctx());
    let Mode::Steps(p) = &v.instruct.run.as_ref().unwrap().mode else {
        panic!("steps")
    };
    assert_eq!(
        (p.steps.len(), p.of),
        (1, Some((18, 19))),
        "the last batch is the one step left"
    );
    v.stop_answer();
    v.design_built_for_test(19);
    v.build_design(&ctx());
    assert!(!v.instruct.busy(), "nothing left to build");
    assert_eq!(v.kept_design_for_test().unwrap().next, None);
}

#[test]
fn a_small_design_builds_step_by_step_and_is_written_once_when_every_step_has_passed() {
    let mut v = mine();
    v.proj.files.insert(DESIGN_FILE.into(), LAMP_DESIGN.into());
    v.proj.save_as("kept the design", None);
    let revs = v.proj.revisions().len();
    v.build_design(&ctx());
    assert!(
        last_user(&v).contains("Build step 1 of 2 of a designed system"),
        "{}",
        last_user(&v)
    );
    assert!(
        last_user(&v).contains("Requirements this step must pass"),
        "{}",
        last_user(&v)
    );
    reply(&mut v, &answer(&horn(), &horn_follows()));
    assert_eq!(v.proj.files["main.st"], LAMP, "the file waits for the last step");
    let step2 = last_user(&v);
    assert!(
        step2.contains("Build step 2 of 2 of a designed system"),
        "{step2}"
    );
    assert!(
        step2.contains("Steps already built, keep them working"),
        "{step2}"
    );
    reply(&mut v, &answer(&horn_latched(), &latches()));
    let o = v.instruct.outcome.as_ref().unwrap();
    assert_eq!(o.kind, Kind::Applied, "{o:?}");
    assert_eq!(v.proj.files["main.st"], horn_latched());
    assert_eq!(v.proj.revisions().len(), revs + 1);
    assert!(
        v.proj
            .revisions()
            .last()
            .unwrap()
            .what
            .contains("Design steps 1 to 2 of 2"),
        "{}",
        v.proj.revisions().last().unwrap().what
    );
    let k = v.kept_design_for_test().unwrap();
    assert_eq!(
        (k.built, k.next),
        (2, None),
        "both steps are counted once they are written"
    );
}

#[test]
fn a_design_step_that_runs_out_of_room_pauses_and_is_never_split() {
    let mut v = mine();
    v.proj.files.insert(DESIGN_FILE.into(), LAMP_DESIGN.into());
    v.build_design(&ctx());
    let sent = v.link.sent.len();
    crate::steps_tests::cut_off(&mut v);
    assert_eq!(
        v.link.sent.len(),
        sent,
        "no request to split it: the brief names parts and requirements"
    );
    let Mode::Steps(p) = &v.instruct.run.as_ref().unwrap().mode else {
        panic!("steps")
    };
    assert!(matches!(p.state, PlanState::Paused(_)), "{:?}", p.state);
    // Try again asks for the same step from the design.
    v.run_steps(&ctx());
    assert!(last_user(&v).contains("Build step 1 of 2"));
    v.stop_answer();
    assert_eq!(v.kept_design_for_test().unwrap().built, 0);
    assert!(!v.proj.files.contains_key("test_horn.st"));
}

#[test]
fn a_design_that_cannot_be_built_from_offers_no_build() {
    let mut v = mine();
    v.proj.files.insert(
        DESIGN_FILE.into(),
        LAMP_DESIGN.replace("Requirements 3, 7, 8, 9.", "Requirements 3, 7, 8, 9, 77."),
    );
    let k = v.kept_design_for_test().unwrap();
    assert_eq!(k.next, None);
    assert!(k.problems[0].contains("requirement 77"), "{:?}", k.problems);
    let sent = v.link.sent.len();
    v.build_design(&ctx());
    assert_eq!(v.link.sent.len(), sent, "nothing is sent");
    assert!(!v.instruct.busy());
}

#[test]
fn building_waits_for_the_chat_and_for_an_instruction_in_hand() {
    let mut v = mine();
    v.proj.files.insert(DESIGN_FILE.into(), LAMP_DESIGN.into());
    v.instruct.draft = "make the lamp blink".into();
    v.send_instruction(&ctx());
    assert!(v.instruct.busy());
    let sent = v.link.sent.len();
    v.build_design(&ctx());
    assert_eq!(v.link.sent.len(), sent, "one run at a time");
    assert!(v.kept_design_for_test().unwrap().build_blocked.is_some());
}

#[test]
fn a_design_entry_the_relay_does_not_serve_shows_nothing_and_the_chat_works() {
    for status in [
        r#"{"open": true, "thinking": ["off"], "agents": ["ask", "check", "edits"]}"#,
        r#"{"open": true, "agents": ["ask"], "design": {"open": true}}"#,
        "",
    ] {
        let mut v = mine();
        v.design.set_base("https://design.example");
        v.design_link.inbox.push(Incoming::DesignStatus(status.into()));
        v.tick(&ctx());
        assert!(!v.design.visible(), "{status}");
        let bodies = crate::instruct_tests::instruct(
            &mut v,
            "make the lamp latch",
            &[&st(
                &crate::instruct_tests::LAMP.replace("lamp := start;\n", "lamp := start OR lamp;\n")
            )],
        );
        assert_ne!(bodies.len(), 0);
        assert!(v.design_link.sent.is_empty(), "no design request was made");
    }
}

#[test]
fn a_closed_entry_and_a_spent_allowance_say_so_without_touching_the_chat() {
    let mut v = served();
    v.design_link.inbox.push(Incoming::DesignStatus(
        r#"{"open": false, "reopens_by_itself": true, "design": {"open": false, "reopens_by_itself": true}}"#
            .into(),
    ));
    v.tick(&ctx());
    assert!(v.design.visible());
    assert!(v.design.closed_line(0.0).unwrap().contains("closed for now"));
    asked(&mut v, "A lamp.");
    assert!(v.design_link.sent.is_empty(), "a closed entry is not asked");
    assert!(v.design.error.as_deref().unwrap().contains("closed for now"));
    // The chat's own state is untouched and it answers.
    assert!(v.chat.route().is_ok());
    let _ = crate::instruct_tests::instruct(
        &mut v,
        "make the lamp latch",
        &[&st(&LAMP.replace("lamp := start;\n", "lamp := start OR lamp;\n"))],
    );
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Applied));
    // An allowance spent at the relay: said with the wait; the chat goes on.
    let mut v = served();
    asked(&mut v, "A lamp.");
    let (seq, _) = v.design_link.sent[0].clone();
    v.design_link.inbox.push(Incoming::Status {
        seq,
        status: 429,
        body: json!({"type":"error","error":{"type":"design_allowance_used","message":"x","retryable":true,"retry_after_s":2400}}).to_string(),
    });
    v.tick(&ctx());
    assert!(!v.design.busy());
    assert!(
        v.design.notice.as_deref().unwrap().contains("about 40 minutes"),
        "{:?}",
        v.design.notice
    );
    assert!(v.chat.route().is_ok());
    let _ = crate::instruct_tests::instruct(
        &mut v,
        "make the lamp latch",
        &[&st(&LAMP.replace("lamp := start;\n", "lamp := start OR lamp;\n"))],
    );
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Applied));
}

#[test]
fn stop_mid_turn_leaves_no_draft_and_the_page_goes_on() {
    let mut v = served();
    asked(&mut v, "A lamp.");
    let (seq, _) = v.design_link.sent[0].clone();
    v.design_link.inbox.push(Incoming::Chunk {
        seq,
        text: relay_turn(LAMP_DESIGN, "end_turn")[..800].to_string(),
    });
    v.tick(&ctx());
    assert!(v.design.busy());
    v.stop_design();
    assert_eq!(v.design_link.aborted, 1, "the request is aborted");
    v.design_link.inbox.push(Incoming::Chunk {
        seq,
        text: relay_turn(LAMP_DESIGN, "end_turn")[800..].to_string(),
    });
    v.design_link.inbox.push(Incoming::End { seq });
    v.tick(&ctx());
    assert!(v.design.read.is_none() && !v.design.busy() && !v.design.window);
    assert!(!v.proj.files.contains_key(DESIGN_FILE));
}

#[test]
fn a_big_design_goes_on_by_itself_and_every_turn_is_asked_through_the_same_link() {
    let mut v = served();
    asked(&mut v, "A bottling line.");
    design_reply(&mut v, LINE_1, "max_tokens");
    assert_eq!(
        v.design_link.sent.len(),
        2,
        "the second turn is asked for at once"
    );
    let b = body_of(&v, 1);
    assert_eq!(b["messages"].as_array().unwrap().len(), 3);
    assert_eq!(b["messages"][1]["content"][0]["signature"], "c2ln");
    assert_eq!(b["messages"][2]["content"], "Continue");
    design_reply(&mut v, LINE_2, "end_turn");
    let read = v.design.read.as_ref().unwrap();
    assert_eq!((read.turns, read.design.steps.len()), (2, 19));
}

/// "Chat about this" (`crate::chat_about`) goes with a design step's ask
/// as it does with an instruction (the owner, 2026-11-18): what was
/// collected is in every step of the batch, is spent with it, and with
/// nothing collected the ask carries no envelope.
#[test]
fn what_was_collected_with_chat_about_this_goes_with_the_design_steps_asks() {
    use crate::chat_about::{Chip, Kind as ChipKind};
    let build = |v: &mut PlaygroundView| {
        v.proj.files.insert(DESIGN_FILE.into(), LAMP_DESIGN.into());
        v.build_design(&ctx());
        last_user(v)
    };
    let mut plain = mine();
    let none = build(&mut plain);
    assert!(!none.contains("<chat_about>"), "{none}");

    let mut v = mine();
    v.instruct.add_chip(Chip::new(
        "lamp",
        ChipKind::Output,
        "main.st",
        7,
        "lamp := start;",
        vec!["start".into()],
        vec![],
    ));
    let asked = build(&mut v);
    assert_eq!(asked.matches("<chat_about>").count(), 1, "{asked}");
    assert!(asked.contains("output `lamp` at main.st:7"), "{asked}");
    assert!(v.instruct.chips.is_empty(), "they went with it");
    assert_eq!(
        asked.replace(&v.instruct.about, ""),
        none,
        "nothing else in the ask changes"
    );
}
