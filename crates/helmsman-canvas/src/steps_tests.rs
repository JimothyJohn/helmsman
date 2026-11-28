//! A change too big for one answer, in steps (`crate::steps`, Nick
//! 2026-11-14): split up front for a big ask and after an answer that ran
//! out of room; a plan waits for approval; the steps run on a working
//! copy and the file is written once, when all of them pass; a step that
//! fails pauses the plan. Scripted Agent streams, no network, no sleeps.

use eframe::egui;
use serde_json::json;

use crate::claude::Incoming;
use crate::instruct::{Kind, Mode, PlanState, Way};
use crate::instruct_tests::{LAMP, horn_follows, latches, mine, reply};
use crate::playground_view::PlaygroundView;
use crate::steps::{MAX_STEPS, big_ask, parse, plan_request, step_request};

/// The lamp with a horn that follows start.
fn horn() -> String {
    LAMP.replace("  lamp : BOOL;\n", "  lamp : BOOL;\n  horn : BOOL;\n")
        .replace("lamp := start;\n", "lamp := start;\nhorn := start;\n")
}

/// [`horn`], and the lamp latches.
fn horn_latched() -> String {
    horn().replace("lamp := start;\n", "lamp := start OR lamp;\n")
}

fn answer(code: &str, scenario: &str) -> String {
    format!("Why: step done.\n\n```st\n{code}```\n\n{scenario}")
}

pub(crate) const PLAN: &str = "1. Add a horn that sounds with start.\n2. Latch the lamp until power off.\n";

/// The Agent's answer in flight runs out of room halfway through the code.
pub(crate) fn cut_off(v: &mut PlaygroundView) {
    let (seq, _) = v.link.sent.last().cloned().expect("a request");
    let ev = |name: &str, data: serde_json::Value| format!("event: {name}\ndata: {data}\n\n");
    let text = [
        ev("message_start", json!({"type":"message_start","message":{"model":"m","id":"m","type":"message","role":"assistant","content":[],"stop_reason":null}})),
        ev("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        ev("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Why: x.\n\n```st\nPROGRAM lamp\n"}})),
        ev("content_block_stop", json!({"type":"content_block_stop","index":0})),
        ev("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}})),
        ev("message_stop", json!({"type":"message_stop"})),
    ]
    .concat();
    v.link.inbox.push(Incoming::Chunk { seq, text });
    v.link.inbox.push(Incoming::End { seq });
    v.tick(&egui::Context::default());
}

/// The text of the last request's last user message.
fn last_user(v: &PlaygroundView) -> String {
    let (_, req) = v.link.sent.last().unwrap();
    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
    body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Sent as the page sends it, a big ask the way the notice's first
/// button does: in steps.
fn send(v: &mut PlaygroundView, instruction: &str) {
    v.instruct.draft = instruction.into();
    let way = if big_ask(instruction, Some(9)) {
        Way::Steps
    } else {
        Way::Ask
    };
    v.send_instruction_as(&egui::Context::default(), way);
}

fn press_send(v: &mut PlaygroundView, instruction: &str) {
    v.instruct.draft = instruction.into();
    v.send_instruction(&egui::Context::default());
}

const BIG: &str = "Latch the lamp.\n- Start sets it\n- Stop resets it\n- A power cut resets it\n";

fn plan_state(v: &PlaygroundView) -> Option<PlanState> {
    match &v.instruct.run.as_ref()?.mode {
        Mode::Steps(p) => Some(p.state.clone()),
        _ => None,
    }
}

// ---- the pure parts ----

#[test]
fn a_plan_is_its_numbered_lines() {
    assert_eq!(
        parse("Here is the plan:\n1. Add a horn.\n2) Latch the lamp.\n\n**3.** Blink it.\nThat's all."),
        ["Add a horn.", "Latch the lamp.", "Blink it."]
    );
    assert_eq!(parse("- 1. Add a horn.\n- 2. Latch."), ["Add a horn.", "Latch."]);
    assert_eq!(parse("No plan here."), Vec::<String>::new());
    assert!(
        parse("1. A step.\n```st\nPROGRAM p\nEND_PROGRAM\n```").is_empty(),
        "code is no plan"
    );
    assert!(
        parse("100. Not a step.\n1.").is_empty(),
        "three digits, or nothing after the number"
    );
    let many = (1..=20).fold(String::new(), |mut m, k| {
        use std::fmt::Write as _;
        let _ = writeln!(m, "{k}. step {k}");
        m
    });
    assert_eq!(parse(&many).len(), MAX_STEPS);
    assert!(parse(&format!("1. {}", "x".repeat(5000)))[0].chars().count() <= 300);
}

#[test]
fn a_big_ask_is_long_in_parts_or_on_a_long_file() {
    assert!(!big_ask("Latch the lamp until stop.", Some(9)));
    assert!(
        !big_ask("Make a traffic light.", None),
        "a new project alone is not big"
    );
    assert!(big_ask(&"word ".repeat(70), Some(9)), "long");
    assert!(
        big_ask(
            "Add a conveyor.\n- It starts on start\n- It stops on stop or a jam\n- A lamp shows a jam\n",
            Some(9)
        ),
        "four parts"
    );
    assert!(big_ask("Rename the lamp.", Some(200)), "a long file");
}

#[test]
fn the_requests_say_what_is_asked() {
    let p = plan_request("make it blink", true);
    assert!(
        p.contains("ran out of room") && p.contains("Do not write any code"),
        "{p}"
    );
    assert!(p.ends_with("The instruction: make it blink"), "{p}");
    assert!(plan_request("x", false).contains("planned first"));
    let steps = parse(PLAN);
    let s = step_request("horn and latch", &steps, 1);
    assert!(
        s.starts_with("Step 2 of 2 of a larger change. The whole change: horn and latch"),
        "{s}"
    );
    assert!(
        s.contains("1. Add a horn that sounds with start. (done, already in the file)"),
        "{s}"
    );
    assert!(
        s.contains("2. Latch the lamp until power off. (this answer)"),
        "{s}"
    );
    assert!(s.ends_with("Do step 2 only: Latch the lamp until power off. Keep what the earlier steps did working, and leave the later steps for later answers."), "{s}");
}

// ---- the flow ----

/// The answer that ran out of room becomes a plan; its first step goes
/// at once, with no approval (Nick, 2026-11-14: "just go"); each step
/// builds on the last on a copy; the file is
/// written once, one revision, with every step's scenario kept.
#[test]
fn an_answer_that_ran_out_of_room_goes_in_steps_written_once_at_the_end() {
    let mut v = mine();
    let revs = v.proj.revisions().len();
    send(&mut v, "add a horn and latch the lamp");
    cut_off(&mut v);
    assert!(v.instruct.busy(), "no failure: {:?}", v.instruct.outcome);
    assert!(last_user(&v).contains("ran out of room before the code was finished"));
    assert!(last_user(&v).contains("The instruction: add a horn and latch the lamp"));

    let sent = v.link.sent.len();
    reply(&mut v, PLAN);
    assert_eq!(plan_state(&v), Some(PlanState::Running));
    assert_eq!(v.link.sent.len(), sent + 1, "step 1 goes without a click");
    assert!(last_user(&v).contains("Step 1 of 2"), "{}", last_user(&v));
    reply(&mut v, &answer(&horn(), &horn_follows()));
    assert_eq!(v.proj.files["main.st"], LAMP, "the file waits for the last step");
    assert_eq!(v.proj.revisions().len(), revs);
    let step2 = last_user(&v);
    assert!(step2.contains("Step 2 of 2"), "{step2}");
    assert!(
        step2.contains("horn := start;"),
        "the step builds on the first: {step2}"
    );
    assert!(
        step2.contains("test_horn"),
        "with its scenario as a requirement test"
    );

    reply(&mut v, &answer(&horn_latched(), &latches()));
    assert!(!v.instruct.busy());
    assert_eq!(
        v.proj.files["main.st"], LAMP,
        "held: the last step waits for Apply"
    );
    assert_eq!(v.proj.revisions().len(), revs);
    crate::instruct_tests::apply(&mut v);
    let o = v.instruct.outcome.as_ref().unwrap();
    assert_eq!(o.kind, Kind::Applied, "{o:?}");
    assert!(
        o.status
            .starts_with("Updated main.st in 2 steps: 2 scenarios passed"),
        "{}",
        o.status
    );
    assert_eq!(v.proj.files["main.st"], horn_latched());
    assert!(
        v.proj.files.contains_key("test_horn.st") && v.proj.files.contains_key("test_lamp_latches.st"),
        "{:?}",
        v.proj.files.keys().collect::<Vec<_>>()
    );
    assert_eq!(v.proj.revisions().len(), revs + 1, "one version");
    let page = v.proj.revisions().last().unwrap().page.clone().unwrap();
    assert!(
        page.starts_with("Instruction: add a horn and latch the lamp\nFile: main.st\n"),
        "{page}"
    );
    assert!(
        page.contains("Done in steps, each checked:\n1. Add a horn that sounds with start. Why: step done."),
        "{page}"
    );
}

/// A step that still fails after its retries pauses the plan with what
/// failed; trying again asks for the same step; Stop drops the plan and
/// the file is as it was.
#[test]
fn a_step_that_fails_pauses_the_plan_for_the_engineer() {
    let mut v = mine();
    send(&mut v, "add a horn and latch the lamp");
    cut_off(&mut v);
    reply(&mut v, PLAN);
    reply(&mut v, &answer(&horn(), &horn_follows()));
    // Step 2 never latches: red on every try.
    let red = answer(&horn(), &latches());
    for _ in 0..=crate::verify::RETRIES {
        assert_eq!(plan_state(&v), Some(PlanState::Running));
        reply(&mut v, &red);
    }
    let Some(PlanState::Paused(problems)) = plan_state(&v) else {
        panic!("{:?}", plan_state(&v))
    };
    assert_ne!(problems, Vec::<String>::new());
    let sent = v.link.sent.len();
    v.run_steps(&egui::Context::default());
    assert_eq!(v.link.sent.len(), sent + 1);
    assert!(last_user(&v).contains("Step 2 of 2"), "the same step again");
    v.stop_answer();
    assert!(!v.instruct.busy());
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Failed));
    assert_eq!(v.proj.files["main.st"], LAMP, "nothing written");
    assert!(
        !v.proj.files.contains_key("test_horn.st"),
        "not even a step that passed"
    );
}

/// A step that runs out of room is split into smaller steps by the Agent
/// with nobody asked, and the plan carries on from the first of them
/// (Nick, 2026-11-14, a screenshot of it: a pause whose only way on was
/// "Try this step again", which ran out of room the same way).
#[test]
fn a_step_that_runs_out_of_room_is_split_and_the_plan_carries_on() {
    let mut v = mine();
    send(&mut v, "add a horn and latch the lamp");
    cut_off(&mut v);
    reply(&mut v, PLAN);
    reply(&mut v, &answer(&horn(), &horn_follows()));
    let sent = v.link.sent.len();
    cut_off(&mut v);
    assert!(
        !matches!(plan_state(&v), Some(PlanState::Paused(_))),
        "no pause: {:?}",
        plan_state(&v)
    );
    assert_eq!(v.link.sent.len(), sent + 1, "the split is asked for at once");
    let asked = last_user(&v);
    assert!(asked.contains("Step 2 ran out of room"), "{asked}");
    assert!(asked.contains("Do not write any code"), "{asked}");
    assert!(
        asked.contains("Latch the lamp until power off"),
        "the step to split: {asked}"
    );

    reply(
        &mut v,
        "1. Latch the lamp on start.\n2. Drop the latch at power off.\n",
    );
    assert_eq!(plan_state(&v), Some(PlanState::Running));
    let Mode::Steps(p) = &v.instruct.run.as_ref().unwrap().mode else {
        panic!("a plan")
    };
    assert_eq!(
        p.steps,
        [
            "Add a horn that sounds with start.",
            "Latch the lamp on start.",
            "Drop the latch at power off."
        ]
    );
    assert_eq!(p.done, 1, "the step that passed is kept");
    assert!(last_user(&v).contains("Step 2 of 3"), "{}", last_user(&v));
}

/// A step the Agent cannot split (its split is one step, or runs out of
/// room too) pauses with what to do next in plain words; trying again
/// asks for the step once more.
#[test]
fn a_step_that_cannot_be_split_pauses_with_a_way_on() {
    let mut v = mine();
    send(&mut v, "add a horn and latch the lamp");
    cut_off(&mut v);
    reply(&mut v, PLAN);
    cut_off(&mut v);
    reply(&mut v, "1. Add a horn that sounds with start.");
    let Some(PlanState::Paused(problems)) = plan_state(&v) else {
        panic!("{:?}", plan_state(&v))
    };
    assert_eq!(
        problems,
        [
            "Step 1 is too big for one answer and could not be split. Try it again, or stop and ask for this part by itself in a few words."
        ]
    );
    let sent = v.link.sent.len();
    v.run_steps(&egui::Context::default());
    assert_eq!(v.link.sent.len(), sent + 1);
    assert!(last_user(&v).contains("Step 1 of 2"), "{}", last_user(&v));
    // Out of room again: one more split is tried, and that one out of
    // room pauses rather than looping.
    cut_off(&mut v);
    assert!(last_user(&v).contains("Step 1 ran out of room"));
    cut_off(&mut v);
    assert!(matches!(plan_state(&v), Some(PlanState::Paused(_))));
}

/// A big ask is planned before any code is asked for; a plan of one step
/// is just the instruction, sent at once with no approval.
#[test]
fn a_big_ask_is_planned_first_and_one_step_needs_no_approval() {
    let mut v = mine();
    let big = "Latch the lamp.\n- Start sets it\n- Stop resets it\n- A power cut resets it\n";
    send(&mut v, big);
    assert!(last_user(&v).contains("This instruction is big, so it is planned first"));
    reply(&mut v, "1. Latch the lamp.");
    assert!(matches!(v.instruct.run.as_ref().unwrap().mode, Mode::Direct));
    let asked = last_user(&v);
    assert!(asked.starts_with(big.trim()), "the instruction itself: {asked}");
    reply(
        &mut v,
        &answer(
            &LAMP.replace("lamp := start;", "lamp := start OR lamp;"),
            &latches(),
        ),
    );
    assert_eq!(v.proj.files["main.st"], LAMP, "held, not written");
    crate::instruct_tests::apply(&mut v);
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Applied));
}

/// A big ask sent as typed goes nowhere: the draft stays, the notice has
/// the time in one go, and nothing is sent until the engineer picks.
#[test]
fn a_big_ask_is_held_with_its_estimate_until_the_engineer_picks() {
    let mut v = mine();
    let before = v.link.sent.len();
    press_send(&mut v, BIG);
    assert_eq!(v.link.sent.len(), before, "nothing is sent");
    assert!(v.instruct.run.is_none(), "no run");
    assert_eq!(v.instruct.draft, BIG, "what was typed is kept");
    let (least, most) = v.big.as_ref().expect("the notice").1;
    assert_eq!((least, most), crate::instruct::estimate_secs(Some(9)));
    assert_eq!(
        crate::instruct::big_notice(least, most),
        format!(
            "This is a big change (about {least} to {most} s in one go). Splitting it into checked steps is faster to get right."
        )
    );
    // Editing the draft takes the notice off; sending again re-offers.
    v.instruct.draft.push_str("Also a horn.");
    assert!(v.big.as_ref().is_some_and(|b| b.0 != v.instruct.draft));
}

#[test]
fn splitting_a_held_big_ask_sends_the_plan_request() {
    let mut v = mine();
    press_send(&mut v, BIG);
    v.send_instruction_as(&egui::Context::default(), Way::Steps);
    assert!(v.big.is_none());
    assert!(last_user(&v).contains("This instruction is big, so it is planned first"));
    assert!(v.instruct.run.is_some());
}

/// Send pressed again on the unchanged draft takes the way on offer.
#[test]
fn sending_the_held_ask_again_takes_the_steps() {
    let mut v = mine();
    press_send(&mut v, BIG);
    v.send_instruction(&egui::Context::default());
    assert!(last_user(&v).contains("This instruction is big, so it is planned first"));
}

#[test]
fn one_go_sends_the_instruction_as_typed() {
    let mut v = mine();
    press_send(&mut v, BIG);
    v.send_instruction_as(&egui::Context::default(), Way::OneGo);
    assert!(v.big.is_none());
    let asked = last_user(&v);
    assert!(!asked.contains("planned first"), "{asked}");
    assert!(asked.starts_with(BIG.trim()), "{asked}");
    assert!(matches!(v.instruct.run.as_ref().unwrap().mode, Mode::Direct));
}

#[test]
fn a_small_ask_goes_at_once_with_no_notice() {
    let mut v = mine();
    let before = v.link.sent.len();
    press_send(&mut v, "Latch the lamp until stop.");
    assert!(v.big.is_none());
    assert_eq!(v.link.sent.len(), before + 1, "sent at once");
    assert!(!last_user(&v).contains("planned first"));
}

/// A plan request answered with code after all is checked as the answer.
#[test]
fn a_plan_answered_with_code_is_taken_as_the_answer() {
    let mut v = mine();
    send(&mut v, "add a horn and latch the lamp");
    cut_off(&mut v);
    reply(&mut v, &answer(&horn(), &horn_follows()));
    assert_eq!(v.proj.files["main.st"], LAMP, "held, not written");
    crate::instruct_tests::apply(&mut v);
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Applied));
    assert_eq!(v.proj.files["main.st"], horn());
}

/// Out of room again while planning, or a plan of one step after a
/// cut-off: the instruction ends and says to ask for one part.
#[test]
fn no_split_after_a_cut_off_ends_the_instruction() {
    for second in [None, Some("1. Do it all.")] {
        let mut v = mine();
        send(&mut v, "add a horn and latch the lamp");
        cut_off(&mut v);
        match second {
            None => cut_off(&mut v),
            Some(plan) => reply(&mut v, plan),
        }
        assert!(!v.instruct.busy(), "{second:?}");
        let o = v.instruct.outcome.as_ref().unwrap();
        assert_eq!(o.kind, Kind::Failed, "{second:?}");
        assert_eq!(v.proj.files["main.st"], LAMP);
    }
}
