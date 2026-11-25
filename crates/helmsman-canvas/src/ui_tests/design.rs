//! The design step on the screen (`crate::design_ui`): the entry in the
//! Agent tab and its cards at the panel's default width (the panel must
//! not grow to any row: CLAUDE.md "A panel is only ever laid out at the
//! width something forces"), the description window, the window the design
//! is read in at a phone's width, Keep, Build, Stop, and an entry the relay
//! does not serve. Scripted streams; no network, no sleeps.

use eframe::egui::{Pos2, Rect, Vec2};
use serde_json::{Value as Json, json};

use super::{App, Harness};
use crate::claude::Incoming;
use crate::design;
use crate::design_step::DESIGN_FILE;

const LINE_1: &str = include_str!("../fixtures/design/bottling-line-1.md");
const LINE_2: &str = include_str!("../fixtures/design/bottling-line-2.md");
const LAMP_DESIGN: &str = include_str!("../fixtures/design/lamp.md");
const OPEN: &str = r#"{"open": true, "agents": ["design"], "design": {"open": true}}"#;
const CLOSED: &str =
    r#"{"open": false, "reopens_by_itself": true, "design": {"open": false, "reopens_by_itself": true}}"#;

fn ev(name: &str, data: &Json) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// The head of a relay turn: thinking with `summary`, then the text.
fn turn_head(summary: &str) -> String {
    [
        ev("content_block_start", &json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),
        ev("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":summary}})),
        ev("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"c2ln"}})),
        ev("content_block_stop", &json!({"type":"content_block_stop","index":0})),
        ev("content_block_start", &json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}})),
    ]
    .concat()
}

fn turn_tail(text: &str, stop: &str) -> String {
    [
        ev(
            "content_block_delta",
            &json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":text}}),
        ),
        ev(
            "content_block_stop",
            &json!({"type":"content_block_stop","index":1}),
        ),
        ev(
            "message_delta",
            &json!({"type":"message_delta","delta":{"stop_reason":stop}}),
        ),
        ev("message_stop", &json!({"type":"message_stop"})),
        ev("relay_usage", &json!({"type":"relay_usage"})),
    ]
    .concat()
}

fn page() -> Harness {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    h
}

fn served(h: &mut Harness, status: &str) {
    let sim = h.app.sim.as_mut().unwrap();
    sim.design.set_base("https://design.example");
    sim.design_link.inbox.push(Incoming::DesignStatus(status.into()));
    h.wait(0.3);
}

fn sim(h: &mut Harness) -> &mut crate::playground_view::PlaygroundView {
    h.app.sim.as_mut().unwrap()
}

/// The description window's Send, as a click on it does.
fn send(h: &mut Harness) {
    let ctx = h.ctx.clone();
    sim(h).send_design(&ctx);
}

/// The panel is as wide as it was and `keys` are in it.
fn in_panel(h: &Harness, left: Rect, keys: &[&str]) {
    let grew = h.rect("left").right() - left.right();
    assert!((0.0..=8.0).contains(&grew), "the left pane grew {grew} pt");
    for k in keys {
        let r = h.rect(k);
        assert!(
            r.right() <= h.rect("left").right() + 0.5,
            "{k} {r:?} sticks out of the pane"
        );
    }
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
}

fn inside_page(h: &Harness, keys: &[&str]) {
    let screen = Rect::from_min_size(Pos2::ZERO, h.size).expand(0.5);
    for k in keys {
        let r = h.rect(k);
        assert!(
            screen.contains_rect(r),
            "{k} {r:?} is outside the page {screen:?}"
        );
    }
}

fn answer_turn(h: &mut Harness, text: &str, stop: &str) {
    let (seq, _) = sim(h).design_link.sent.last().cloned().unwrap();
    let body = format!("{}{}", turn_head("Sorting the stations."), turn_tail(text, stop));
    sim(h).design_link.inbox.push(Incoming::Chunk { seq, text: body });
    sim(h).design_link.inbox.push(Incoming::End { seq });
    h.wait(0.2);
}

#[test]
fn no_entry_without_a_served_design_entry_and_the_chat_tab_is_as_before() {
    let mut h = page();
    assert!(!h.has("design open"), "no address built in: no entry");
    served(&mut h, r#"{"open": true, "agents": ["ask", "edits"]}"#);
    assert!(!h.has("design open"), "the relay does not list the agent");
    assert!(
        h.has("agent send") || h.has("agent instruction"),
        "the Agent tab still draws"
    );
}

#[test]
fn the_entry_at_the_default_width_leaves_the_pane_its_width() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    assert!(h.has("design open"));
    in_panel(&h, left, &["design open"]);
    h.press("design open");
    h.wait(0.3);
    for k in [
        "design form",
        "design text",
        "design count",
        "design send",
        "design close",
    ] {
        assert!(h.has(k), "{k}");
    }
    assert!(
        h.drew("Describe the machine, or add a document, first"),
        "nothing typed: Send says why"
    );
    in_panel(&h, left, &["design open"]);
    inside_page(&h, &["design form"]);
    h.press("design close");
    h.wait(0.2);
    assert!(!h.has("design form"));
}

#[test]
fn a_closed_entry_says_so_beside_the_button_and_does_not_open_the_window() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, CLOSED);
    assert!(h.has("design open") && h.has("design closed"));
    assert!(h.drew("closed for now"), "said in words");
    in_panel(&h, left, &["design open", "design closed"]);
    h.press("design open");
    h.wait(0.2);
    assert!(!h.has("design form"), "a closed entry does not open");
    // The chat's own box is untouched.
    assert!(h.has("agent instruction"));
}

#[test]
fn a_design_under_way_shows_the_time_the_thinking_and_stop_and_stop_leaves_nothing() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    sim(&mut h).design.description = "A bottling line with a rinser, a filler and a crowner.".into();
    h.press("design open");
    h.wait(0.2);
    h.press("design send");
    h.wait(0.2);
    assert_eq!(sim(&mut h).design_link.sent.len(), 1);
    assert!(!h.has("design form"), "the window closes when it is sent");
    // A long summary with an unbreakable run: it must wrap, not widen the pane.
    let summary = format!("First the stations. {}", "Z".repeat(300));
    let (seq, _) = sim(&mut h).design_link.sent.last().cloned().unwrap();
    sim(&mut h).design_link.inbox.push(Incoming::Chunk {
        seq,
        text: turn_head(&summary),
    });
    h.wait(1.2);
    for k in ["design progress", "design stop", "design thinking"] {
        assert!(h.has(k), "{k}");
    }
    assert!(h.drew("Designing, turn 1:"), "the elapsed time is there");
    in_panel(&h, left, &["design progress", "design stop", "design thinking"]);
    assert!(
        h.rect("design thinking").height() > 40.0,
        "the summary wraps over rows: {:?}",
        h.rect("design thinking")
    );
    h.press("design stop");
    h.wait(0.2);
    assert!(!h.has("design progress"));
    assert!(sim(&mut h).design.read.is_none());
    assert_eq!(sim(&mut h).design_link.aborted, 1);
    assert!(!sim(&mut h).proj.files.contains_key(DESIGN_FILE));
    in_panel(&h, left, &["design open"]);
}

#[test]
fn a_cut_stream_says_so_and_carry_on_asks_again() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    sim(&mut h).design.description = "A lamp.".into();
    send(&mut h);
    let (seq, _) = sim(&mut h).design_link.sent.last().cloned().unwrap();
    // No relay_usage at the end.
    let text = format!(
        "{}{}",
        turn_head("Thinking."),
        ev(
            "content_block_delta",
            &json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Design: a"}})
        )
    );
    sim(&mut h).design_link.inbox.push(Incoming::Chunk { seq, text });
    sim(&mut h).design_link.inbox.push(Incoming::End { seq });
    h.wait(0.3);
    assert!(h.has("design cut") && h.has("design carry on"));
    assert!(h.drew("stopped before the design was finished"));
    in_panel(&h, left, &["design cut", "design carry on"]);
    h.press("design carry on");
    h.wait(0.2);
    assert_eq!(sim(&mut h).design_link.sent.len(), 2);
    let sent = &sim(&mut h).design_link.sent;
    assert_eq!(sent[1].1.body, sent[0].1.body, "the same turn again");
}

#[test]
fn a_design_is_read_kept_and_built_a_batch_at_a_time_without_the_pane_growing() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    // The chat route: the local proxy, so Build can ask the Agent.
    sim(&mut h).chat.receive(Incoming::Proxy(true));
    sim(&mut h).design.description = "A bottling line.".into();
    send(&mut h);
    answer_turn(&mut h, LINE_1, "max_tokens");
    assert_eq!(sim(&mut h).design_link.sent.len(), 2);
    answer_turn(&mut h, LINE_2, "end_turn");
    // The design opens for reading at a phone's width.
    h.size = Vec2::new(360.0, 700.0);
    h.wait(0.3);
    assert!(
        h.has("design read window") && h.has("design body"),
        "{:?}",
        sim(&mut h).design.window
    );
    inside_page(
        &h,
        &[
            "design read window",
            "design body",
            "design window keep",
            "design section 1",
        ],
    );
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    assert!(
        h.drew("## Machine") || h.drew("What the line is"),
        "the first section is shown"
    );
    h.press("design section 3");
    h.wait(0.2);
    assert!(h.drew("The one PROGRAM"), "the Parts section is shown");
    assert_eq!(sim(&mut h).design.read.as_ref().map(|r| r.section), Some(2));
    h.press("design window close");
    h.wait(0.2);
    h.size = Vec2::new(1400.0, 1180.0);
    h.wait(0.3);
    // The card, at the default width.
    assert!(
        h.has("design card") && h.has("design summary"),
        "{:?}",
        sim(&mut h).design.read.is_some()
    );
    assert!(h.drew("Design ready: 11 parts, 81 requirements, 19 build steps in 2 turns."));
    assert!(!h.has("design problem 1"), "a design with none shows none");
    in_panel(
        &h,
        left,
        &["design card", "design summary", "design keep", "design read"],
    );
    h.press("design keep");
    h.wait(0.3);
    assert!(sim(&mut h).proj.files.contains_key(DESIGN_FILE));
    assert!(
        h.has("design kept") && h.has("design build"),
        "the kept design offers Build"
    );
    assert!(h.drew("Built 0 of 19"));
    assert!(h.drew("Build steps 1 to 6 of 19"));
    in_panel(&h, left, &["design kept", "design build"]);
    let sent = sim(&mut h).link.sent.len();
    h.press("design build");
    h.wait(0.3);
    assert_eq!(sim(&mut h).link.sent.len(), sent + 1);
    assert!(h.has("agent plan steps"));
    assert!(
        h.drew("Building steps 1 to 6 of 19 of the design"),
        "said in the UI"
    );
    assert!(h.drew("1. "), "the steps are listed");
    in_panel(&h, left, &["agent plan steps"]);
    let d = design::parse(&design::join([LINE_1, LINE_2]));
    let (_, req) = sim(&mut h).link.sent.last().cloned().unwrap();
    let body: Json = serde_json::from_str(&req.body).unwrap();
    assert!(
        body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap()
            .starts_with(&d.step_brief(0).unwrap())
    );
}

#[test]
fn problems_are_shown_before_build_and_build_is_not_offered() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    sim(&mut h).design.description = "A lamp.".into();
    send(&mut h);
    let bad = LAMP_DESIGN.replace("Requirements 3, 7, 8, 9.", "Requirements 3, 7, 8, 9, 55.");
    answer_turn(&mut h, &bad, "end_turn");
    sim(&mut h).design.window = false;
    h.wait(0.3);
    assert!(
        h.has("design problem 1"),
        "{:?}",
        sim(&mut h).design.read.as_ref().map(|r| &r.problems)
    );
    assert!(h.drew("requirement 55"));
    in_panel(&h, left, &["design card", "design problem 1"]);
    h.press("design keep");
    h.wait(0.3);
    assert!(h.has("design kept problem 1"), "still said once kept");
    assert!(
        !h.has("design build"),
        "Build is not offered for a design with a problem"
    );
    in_panel(&h, left, &["design kept", "design kept problem 1"]);
}

#[test]
fn a_spent_allowance_shows_the_wait_and_the_chat_box_stays() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    sim(&mut h).design.description = "A lamp.".into();
    send(&mut h);
    let (seq, _) = sim(&mut h).design_link.sent.last().cloned().unwrap();
    sim(&mut h).design_link.inbox.push(Incoming::Status {
        seq,
        status: 429,
        body: json!({"type":"error","error":{"type":"design_allowance_used","message":"x","retryable":true,"retry_after_s":1500}}).to_string(),
    });
    h.wait(0.3);
    assert!(h.has("design notice") && h.has("design closed"));
    assert!(
        h.drew("about 25 minutes") || h.drew("allowance"),
        "the wait is said"
    );
    in_panel(&h, left, &["design notice", "design closed", "design open"]);
    assert!(
        h.has("agent instruction") && h.has("agent send"),
        "the chat box is untouched"
    );
}

// ---- on a box ----

/// A box's page with an engineer signed in and a project open.
fn boxed() -> Harness {
    let mut h = Harness::new(App::boxed());
    h.app.review.as_mut().unwrap().state.session = Some(crate::review::Session {
        user: Some("alice".into()),
        role: "engineer".into(),
    });
    let b = h.app.boxv.as_mut().unwrap();
    b.projects.available = Some(true);
    // The entry is shelved on the page for now; these test it.
    b.design.shelved = false;
    h.wait(0.3);
    h
}

fn open_project(h: &mut Harness, design: Option<&str>) {
    let mut bufs = std::collections::BTreeMap::new();
    let buf = |text: &str| crate::box_projects::Buf {
        text: text.into(),
        base: text.into(),
        version: Some("v".into()),
        saving: None,
        loaded: true,
    };
    bufs.insert("main.st".to_string(), buf("PROGRAM p\nEND_PROGRAM\n"));
    if let Some(d) = design {
        bufs.insert(DESIGN_FILE.to_string(), buf(d));
    }
    let b = h.app.boxv.as_mut().unwrap();
    b.projects.open = Some(crate::box_projects::OpenProject {
        name: "line".into(),
        bufs,
        current: Some("main.st".into()),
        runs: None,
    });
    h.wait(0.3);
}

fn box_turn(text: &str, more: bool) -> String {
    let e = |n: &str, d: Json| format!("event: {n}\ndata: {d}\n\n");
    [
        e("session", json!({"id": "d1"})),
        e(
            "thinking",
            json!({"delta": format!("Stations first. {}", "Q".repeat(300))}),
        ),
        e("text", json!({"delta": text})),
        e(
            "done",
            json!({"stop": if more { "max_tokens" } else { "end_turn" }, "more": more}),
        ),
    ]
    .concat()
}

#[test]
fn on_a_box_the_entry_the_progress_and_the_cards_leave_the_panel_its_width() {
    let mut h = boxed();
    h.press("tab claude");
    h.wait(0.3);
    let left = h.rect("left");
    assert!(h.has("design open"), "a box always offers the entry");
    in_panel(&h, left, &["design open"]);
    // No project open: off, with the reason on its tip.
    let why = h.app.boxv.as_ref().unwrap().design_blocked().unwrap();
    assert!(why.contains("Open a project first"), "{why}");
    h.press("design open");
    h.wait(0.2);
    assert!(!h.has("design form"));
    // With a project open it opens and sends.
    open_project(&mut h, None);
    h.press("design open");
    h.wait(0.3);
    assert!(h.has("design form") && h.has("design send"));
    h.app.boxv.as_mut().unwrap().design.description = "A bottling line.".into();
    let ctx = h.ctx.clone();
    h.app.boxv.as_mut().unwrap().send_design(&ctx);
    h.wait(0.2);
    let seq = h.app.boxv.as_ref().unwrap().design_sent[0].seq;
    let first = box_turn(LINE_1, true);
    let head = first[..first.find("event: text").unwrap()].to_string();
    let b = h.app.boxv.as_mut().unwrap();
    b.design_feed(crate::design_step::Feed::Chunk { seq, text: head }, &ctx);
    h.wait(1.2);
    for k in ["design progress", "design thinking", "design stop"] {
        assert!(h.has(k), "{k}");
    }
    in_panel(&h, left, &["design progress", "design thinking", "design stop"]);
    h.press("design stop");
    h.wait(0.2);
    assert!(!h.app.boxv.as_ref().unwrap().design.busy());
    // A design on show (with a problem), then the kept one with Build.
    let bad = LAMP_DESIGN.replace("Requirements 3, 7, 8, 9.", "Requirements 3, 7, 8, 9, 55.");
    let b = h.app.boxv.as_mut().unwrap();
    b.design.read = Some(crate::design_step::Read::new(bad, 1, "A lamp.".into()));
    h.wait(0.3);
    assert!(h.has("design card") && h.has("design problem 1"));
    in_panel(&h, left, &["design card", "design problem 1", "design keep"]);
    h.app.boxv.as_mut().unwrap().design.discard();
    open_project(&mut h, Some(LAMP_DESIGN));
    assert!(h.has("design kept") && h.has("design build"));
    assert!(h.drew("Build steps 1 to 2 of 2"));
    in_panel(&h, left, &["design kept", "design build"]);
}

#[test]
fn no_section_of_the_design_shows_a_raw_marker_and_a_wide_table_stays_in_the_window() {
    let mut h = page();
    served(&mut h, OPEN);
    sim(&mut h).design.description = "A bottling line.".into();
    send(&mut h);
    answer_turn(&mut h, LINE_1, "max_tokens");
    answer_turn(&mut h, LINE_2, "end_turn");
    h.size = Vec2::new(360.0, 700.0);
    h.wait(0.3);
    for k in 1..=7 {
        h.press(&format!("design section {k}"));
        h.wait(0.3);
        assert!(!h.drew("**"), "section {k}: a bold mark is showing");
        assert!(!h.drew("|---"), "section {k}: a table separator is showing");
        inside_page(&h, &["design read window", "design body"]);
    }
}
// ---- documents in the description window ----

#[test]
fn the_window_takes_documents_and_there_is_no_second_entry_for_them() {
    let mut h = page();
    let left = h.rect("left");
    served(&mut h, OPEN);
    assert!(h.has("design open"));
    assert!(
        !h.has("spec open") && !h.has("documents open"),
        "one entry: Design a machine"
    );
    h.press("design open");
    h.wait(0.3);
    for k in [
        "design form",
        "design text",
        "documents text",
        "documents add file",
    ] {
        assert!(h.has(k), "{k}");
    }
    assert!(!h.has("documents total"), "no meter before there is a document");
    in_panel(&h, left, &["design open"]);
    // Files are listed with a size and a remove control, and the total is
    // against the cap.
    sim(&mut h)
        .design
        .docs
        .add_file("plant.md", &vec![b'a'; 3000])
        .unwrap();
    sim(&mut h).design.docs.add_file("scope.txt", b"Scope.").unwrap();
    h.wait(0.3);
    assert!(h.has("documents doc 1") && h.has("documents doc 2") && h.has("documents remove 2"));
    assert!(h.drew("plant.md") && h.drew("2.9 KiB"), "name and size");
    assert!(h.drew("of 120.0 KiB used"), "the total against the cap");
    // Documents alone are enough to ask: no complaint about the description.
    assert!(!h.drew("Describe the machine, or add a document, first"));
    h.press("documents remove 1");
    h.wait(0.2);
    assert_eq!(sim(&mut h).design.docs.docs.len(), 1);
    assert_eq!(sim(&mut h).design.docs.docs[0].name, "scope.txt");
    // Over the cap the window says so and asking sends nothing.
    sim(&mut h).design.docs.pasted = "a".repeat(crate::documents::MAX_TEXT + 10);
    h.wait(0.3);
    assert!(h.drew("one design request takes at most 120 KiB"));
    let before = sim(&mut h).design_link.sent.len();
    h.press("design send");
    h.wait(0.2);
    assert_eq!(sim(&mut h).design_link.sent.len(), before, "nothing was sent");
    in_panel(&h, left, &["design open"]);
}

#[test]
fn add_file_asks_the_picker_and_says_so_natively() {
    let mut h = page();
    served(&mut h, OPEN);
    sim(&mut h).design.open = true;
    h.wait(0.3);
    h.press("documents add file");
    h.wait(0.2);
    assert!(h.drew("Files are added in the browser page."));
}

#[test]
fn the_window_with_documents_stays_inside_a_narrow_page_and_draws_no_small_text() {
    for (w, hgt) in [(1400.0, 860.0), (760.0, 640.0), (360.0, 640.0), (1400.0, 500.0)] {
        let mut h = page();
        h.size = Vec2::new(w, hgt);
        served(&mut h, OPEN);
        let d = &mut sim(&mut h).design;
        d.open = true;
        d.description = "line\n".repeat(50);
        d.docs.pasted = "line\n".repeat(200);
        d.docs.add_file(&format!("{}.md", "n".repeat(100)), b"x").unwrap();
        h.wait(0.4);
        inside_page(
            &h,
            &[
                "design form",
                "design text",
                "documents text",
                "documents add file",
                "design send",
                "design close",
            ],
        );
        assert!(h.rect("design form").width() <= w, "{w}");
        assert!(h.small_text().is_empty(), "{w}: {:?}", h.small_text());
        let r = h.rect("documents doc 1");
        assert!(r.left() >= 0.0 && r.right() <= w + 0.5, "{w}: {r:?}");
    }
}

#[test]
fn on_a_box_the_window_takes_documents_with_the_boxs_caps() {
    let mut h = boxed();
    h.press("tab claude");
    h.wait(0.3);
    open_project(&mut h, None);
    let left = h.rect("left");
    assert!(!h.has("spec open"), "one entry on a box too: Design a machine");
    h.press("design open");
    h.wait(0.3);
    for k in ["design form", "documents text", "documents add file"] {
        assert!(h.has(k), "{k}");
    }
    assert!(h.app.boxv.as_ref().unwrap().design.docs.on_box);
    let d = &mut h.app.boxv.as_mut().unwrap().design;
    d.docs.pasted = "line\n".repeat(200);
    d.docs.add_file("plant.md", b"# Plant").unwrap();
    h.wait(0.3);
    assert!(h.drew("characters"), "the box's total is in characters");
    in_panel(&h, left, &["design open"]);
    assert!(h.small_text().is_empty(), "{:?}", h.small_text());
    h.press("documents add file");
    h.wait(0.2);
    assert!(h.drew("Files are added in the browser page."));
    for (w, hgt) in [(760.0, 640.0), (360.0, 640.0)] {
        h.size = Vec2::new(w, hgt);
        h.wait(0.4);
        inside_page(
            &h,
            &[
                "design form",
                "documents text",
                "documents add file",
                "design close",
            ],
        );
        assert!(h.small_text().is_empty(), "{w}: {:?}", h.small_text());
    }
}
