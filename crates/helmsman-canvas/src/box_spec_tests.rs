//! Spec mode on a box (`crate::box_spec`), against the real box API
//! (`box_projects_tests`' bench): a text spec and a PDF spec each become a
//! project on the box through one zip; the chat request carries the
//! drop-in ids; a taken name writes nothing; an operator cannot start it;
//! a project with a number input says which inputs need a device and the
//! box's refusal of Deploy is put in words; a BOOL-only project deploys
//! and runs. The Agent's answers are scripted (no model is called).

use eframe::egui;
use serde_json::{Value as Json, json};

use crate::box_agent::Incoming;
use crate::box_projects::{Deploy, PCall, PReq};
use crate::box_projects_tests::{ALICE, BOB, Bench, Jar, make, page, pump};
use crate::box_view::BoxView;
use crate::instruct::{BOX_NEW_RULES, BOX_RULES, Kind};
use crate::spec_mode::tests as sm;
use crate::spec_mode::{RULES, sha256_hex};

const SPEC_TEXT: &str = "Conveyor 4 starts on the start button and keeps running until the stop button.\nThe operator must be able to stop it at any time.";

/// The agent's turn for the last message: its words, then done.
fn answer(v: &mut BoxView, text: &str) {
    let seq = v.chat_sent.last().expect("a message").seq;
    let ev = |n: &str, d: Json| format!("event: {n}\ndata: {d}\n\n");
    let body = [
        ev("session", json!({"id": "s1"})),
        ev("text", json!({ "delta": text })),
        ev("done", json!({"stop": "end_turn", "why": null})),
    ]
    .concat();
    v.agent.apply(Incoming::Chunk { seq, text: body });
    v.agent.apply(Incoming::End { seq });
}

/// A frame, then the box's answers carried out until quiet.
fn run(b: &Bench, jar: &Jar, v: &mut BoxView, ctx: &egui::Context) {
    v.tick(ctx);
    for _ in 0..30 {
        pump(b, jar, v, ctx, 1.0);
        if v.psent.is_empty() {
            return;
        }
    }
    panic!("the box never went quiet");
}

fn body(v: &BoxView, k: usize) -> Json {
    serde_json::from_str(&v.chat_sent[k].body).unwrap()
}

fn good() -> String {
    sm::answer(&[&sm::motor_latches(), &sm::motor_stops()], sm::REQUIREMENTS)
}

/// A PDF as the box knows one: the magic, then `n` bytes in all.
fn pdf(n: usize) -> Vec<u8> {
    let mut v = b"%PDF-1.4\n".to_vec();
    v.resize(n, b' ');
    v
}

fn signed_in(b: &Bench, jar: &Jar) -> (BoxView, egui::Context) {
    let ctx = egui::Context::default();
    let mut v = page(b, jar, Some(("alice", ALICE)));
    v.tick(&ctx);
    (v, ctx)
}

fn check(b: &Bench, jar: &Jar, name: &str) -> (u16, Json) {
    let r = PReq::json(
        PCall::Check(name.into()),
        "POST",
        format!("/api/projects/{name}/check"),
        &json!({}),
    );
    let a = b.send(jar, &r);
    let j = serde_json::from_str(&a.body).unwrap_or(Json::Null);
    (a.status, j)
}

/// A program with a number input: the box has no channel a number could
/// come from, so `level` has no line in io.map.
const TANK: &str = "PROGRAM tank\nVAR_INPUT\n  level : INT; (* tank level in percent *)\n  stop_pb : BOOL; (* on while the stop button is pressed *)\nEND_VAR\nVAR_OUTPUT\n  pump : BOOL; (* runs the fill pump when on *)\nEND_VAR\npump := level < 20 AND NOT stop_pb;\nEND_PROGRAM\n";

fn tank_answer() -> String {
    let scenario = "(* The pump runs while the level is low. *)\nPROGRAM test_pump_runs_when_low\nVAR_INPUT pump : BOOL; END_VAR\nVAR_OUTPUT level : INT; done : BOOL; END_VAR\nVAR n : INT; runs : ASSERT; END_VAR\nn := n + 1;\nlevel := 5;\nIF n > 8 THEN runs(COND := pump); done := TRUE; END_IF;\nEND_PROGRAM";
    format!(
        "Why: the pump fills the tank while the level is low.\n\n```st\n{TANK}```\n\n```st scenario\n{scenario}\n```\n"
    )
}

fn project_names(b: &Bench) -> Vec<String> {
    b.json("/api/projects")["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_list_tells_a_page_with_no_project_how_the_box_checks_a_new_one() {
    let b = Bench::new();
    let jar = Jar::default();
    let (v, _) = signed_in(&b, &jar);
    assert!(project_names(&b).is_empty(), "the box has no project yet");
    let runs = v.projects.new_runs.expect("read from the list");
    assert_eq!((runs.sizes.i, runs.sizes.q), (32, 32), "{runs:?}");
    assert_eq!(runs.period, std::time::Duration::from_millis(2));
}

#[test]
fn a_text_spec_makes_a_project_on_the_box_from_one_zip() {
    let b = Bench::new();
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    // No project is open: a spec makes a new one all the same.
    assert!(v.projects.open.is_none());
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    assert_eq!(v.chat_sent.len(), 1, "{:?}", v.spec.spec.error);
    let m = body(&v, 0);
    assert_eq!(m["session"], Json::Null, "a new conversation");
    assert_eq!(m["dropins"], Json::Null, "no PDF: no drop-in");
    let message = m["message"].as_str().unwrap();
    assert!(message.starts_with(BOX_NEW_RULES), "{message}");
    assert!(
        !message.starts_with(BOX_RULES),
        "not the rules for editing a file"
    );
    assert!(message.contains(RULES), "{message}");
    assert!(
        message.contains(&format!("=== document: Pasted ===\n{SPEC_TEXT}\n")),
        "{message}"
    );
    assert!(message.contains("<edit_scope>"), "{message}");
    assert!(!message.contains("From a spec"), "the label is History's");
    // The draft is on review and the box has nothing yet.
    answer(&mut v, &good());
    run(&b, &jar, &mut v, &ctx);
    let r = v.spec.review.as_ref().expect("the draft is on review");
    assert_eq!(r.name, "conveyor");
    assert!(v.instruct.busy(), "the run waits for Accept or Discard");
    assert!(project_names(&b).is_empty(), "nothing is written before Accept");
    // Accept: one request, then the project is open.
    v.accept_spec();
    run(&b, &jar, &mut v, &ctx);
    assert_eq!(project_names(&b), ["conveyor"]);
    assert!(v.spec.review.is_none() && !v.instruct.busy());
    assert_eq!(
        v.projects.open.as_ref().map(|o| o.name.as_str()),
        Some("conveyor")
    );
    let file = |f: &str| {
        b.json(&format!("/api/projects/conveyor/files/{f}"))["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no {f} on the box"))
            .to_string()
    };
    assert_eq!(file("main.st"), sm::MOTOR);
    assert_eq!(file("spec.md"), format!("# Pasted\n\n{SPEC_TEXT}\n\n"));
    let map = file("io.map");
    assert!(map.contains("in  start_pb bool box di:0"), "{map}");
    assert!(map.contains("in  stop_pb bool box di:1"), "{map}");
    assert!(map.contains("out motor bool box do:0 safe=false"), "{map}");
    assert!(file("test_motor_latches.st").contains("The motor keeps running"));
    assert!(
        file("test_the_belt_stops_when_the_guard_door_opens.st").contains("PROGRAM test_"),
        "a requirement with no scenario is a placeholder test"
    );
    // The box's own check accepts what was made.
    let (status, j) = check(&b, &jar, "conveyor");
    assert_eq!(status, 200, "{j}");
    // One revision; the zip route carries no label, so it is the box's own.
    let revs = b.json("/api/projects/conveyor/history");
    let revs = revs["revisions"].as_array().unwrap();
    assert_eq!(revs.len(), 1, "{revs:?}");
    assert_eq!(revs[0]["what"], "uploaded a zip");
    let o = v.instruct.outcome.as_ref().unwrap();
    assert_eq!(o.kind, Kind::Applied, "{o:?}");
    assert_eq!(
        o.status,
        "Made conveyor from a spec: 2 inputs, 1 output, 2 requirement tests, 2 untested."
    );
    let note = v.projects.notice.clone().unwrap();
    assert!(note.0.contains("Deploy runs it") && !note.1, "{note:?}");
    // The window starts empty next time and still means a box.
    assert!(
        v.spec.spec.on_box && v.spec.spec.pasted.is_empty(),
        "{:?}",
        v.spec.spec
    );
}

#[test]
fn a_pdf_spec_keeps_the_pdf_as_a_dropin_then_asks_the_agent_with_its_id() {
    let b = Bench::new();
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    let bytes = pdf(5000);
    v.spec.spec.add_file("Plant manual.pdf", &bytes).unwrap();
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    assert!(
        v.chat_sent.is_empty(),
        "the agent waits for the box to keep the PDF"
    );
    assert!(v.spec.spec.pdfs[0].sending);
    run(&b, &jar, &mut v, &ctx);
    let list = b.json("/api/dropins");
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["kind"], "pdf");
    assert_eq!(list[0]["name"], "Plant_manual.pdf");
    let id = list[0]["id"].as_u64().unwrap();
    assert_eq!(v.chat_sent.len(), 1, "{:?}", v.spec.spec.error);
    let m = body(&v, 0);
    assert_eq!(m["dropins"], json!([id]), "the request carries the drop-in's id");
    let message = m["message"].as_str().unwrap();
    assert!(message.contains(&format!("drop-in {id}")), "{message}");
    assert!(message.contains("Plant manual.pdf"), "{message}");
    assert!(
        message.contains(&format!("=== document: Pasted ===\n{SPEC_TEXT}\n")),
        "the text still goes in the message"
    );
    // The box reads the request as it was sent.
    let ask: helmsman_plc::api::chat::Ask =
        serde_json::from_str(&v.chat_sent[0].body).expect("the box reads it");
    assert_eq!(ask.dropins, Some(vec![id]));
    answer(&mut v, &good());
    run(&b, &jar, &mut v, &ctx);
    assert!(v.spec.review.is_some());
    v.accept_spec();
    run(&b, &jar, &mut v, &ctx);
    let spec = b.json("/api/projects/conveyor/files/spec.md")["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        spec.contains("# Plant manual.pdf\n\nA PDF of 5000 bytes, SHA-256 "),
        "{spec}"
    );
    assert!(spec.contains(&sha256_hex(&bytes)), "{spec}");
    assert!(
        !spec.contains("%PDF"),
        "the PDF itself is not kept in the project"
    );
    let (status, j) = check(&b, &jar, "conveyor");
    assert_eq!(status, 200, "{j}");
    let o = v.instruct.outcome.as_ref().unwrap();
    assert_eq!(o.kind, Kind::Applied, "{o:?}");
}

#[test]
fn the_boxs_own_refusal_of_a_pdf_is_said_beside_it_and_the_agent_is_not_asked() {
    let b = Bench::new();
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    // The page checks first; these go past it, as a changed box would.
    for (k, (bytes, says)) in [
        (b"PK\x03\x04 a zip named .pdf".to_vec(), "not a PDF"),
        (vec![b'%'; 0], "empty"),
    ]
    .into_iter()
    .enumerate()
    {
        v.spec.spec.pasted = SPEC_TEXT.into();
        v.spec.spec.pdfs = vec![crate::spec_mode::Pdf {
            name: format!("bad{k}.pdf"),
            bytes,
            id: None,
            sending: false,
            refused: None,
        }];
        v.send_spec(&ctx);
        run(&b, &jar, &mut v, &ctx);
        let p = &v.spec.spec.pdfs[0];
        let why = p
            .refused
            .clone()
            .unwrap_or_else(|| panic!("{:?}", v.spec.spec.error));
        assert!(why.contains(says) || why.contains("could not read"), "{why}");
        assert!(
            why.contains(&format!("bad{k}.pdf")) || why.contains("PDF"),
            "{why}"
        );
        assert!(v.chat_sent.is_empty(), "nothing was sent to the agent");
        assert!(!v.spec.uploading && v.spec.review.is_none());
        // The window refuses Build until the document is taken out.
        assert!(
            v.spec.spec.refusal().unwrap().contains("did not keep"),
            "{:?}",
            v.spec.spec.refusal()
        );
        v.spec.spec.remove_pdf(0);
    }
    assert_eq!(b.json("/api/dropins"), json!([]), "the box kept neither");
    // Too big for the box (the box says so with 413).
    v.spec.spec.pdfs = vec![crate::spec_mode::Pdf {
        name: "huge.pdf".into(),
        bytes: pdf(crate::spec_mode::BOX_PDF + 1),
        id: None,
        sending: false,
        refused: None,
    }];
    v.send_spec(&ctx);
    run(&b, &jar, &mut v, &ctx);
    let why = v.spec.spec.pdfs[0].refused.clone().expect("refused");
    assert!(why.contains("huge.pdf") && why.contains("too large"), "{why}");
    assert!(v.chat_sent.is_empty(), "{:?}", v.chat_sent);
}

#[test]
fn a_taken_name_is_refused_beside_the_field_and_nothing_is_written() {
    let b = Bench::new();
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    make(
        &b,
        &jar,
        &mut v.projects,
        "conveyor",
        &[("main.st", "PROGRAM old\nEND_PROGRAM\n")],
    );
    run(&b, &jar, &mut v, &ctx);
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    answer(&mut v, &good());
    run(&b, &jar, &mut v, &ctx);
    let r = v.spec.review.as_ref().unwrap();
    assert_eq!(r.name, "conveyor-2", "the suggestion is a free name");
    // The page knows the name is taken: refused without a request.
    v.spec.review.as_mut().unwrap().name = "Conveyor".into();
    v.accept_spec();
    assert!(v.psent.is_empty(), "{:?}", v.psent);
    let e = v.spec.review.as_ref().unwrap().error.clone().unwrap();
    assert!(e.contains("already on the box"), "{e}");
    // A stale list: the box says 409 and writes nothing.
    v.projects.projects.clear();
    v.accept_spec();
    run(&b, &jar, &mut v, &ctx);
    let r = v.spec.review.as_ref().expect("the draft stays");
    assert!(
        r.error.as_deref().unwrap().contains("already on the box")
            && r.error.as_deref().unwrap().contains("Nothing was written"),
        "{:?}",
        r.error
    );
    assert!(v.instruct.busy(), "the run waits");
    assert_eq!(project_names(&b), ["conveyor"]);
    let old = b.json("/api/projects/conveyor");
    let names: Vec<&str> = old["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["main.st"], "the project that was there is untouched");
    // An invalid name never leaves the page.
    for bad in ["", "Bad Name!", "_lead", &"n".repeat(49)] {
        v.spec.review.as_mut().unwrap().name = bad.into();
        v.accept_spec();
        assert!(v.psent.is_empty(), "{bad:?}");
        assert!(
            v.spec
                .review
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("1 to 48")
        );
    }
    // A free name goes through.
    v.spec.review.as_mut().unwrap().name = "belt".into();
    v.accept_spec();
    run(&b, &jar, &mut v, &ctx);
    assert_eq!(project_names(&b), ["belt", "conveyor"]);
}

#[test]
fn an_operator_cannot_start_a_spec_run_and_nothing_goes_to_the_agent() {
    let b = Bench::new();
    let jar = Jar::default();
    let ctx = egui::Context::default();
    let mut v = page(&b, &jar, Some(("bob", BOB)));
    v.tick(&ctx);
    let off = v.spec_entry_blocked().expect("off for an operator");
    assert!(off.contains("only an engineer"), "{off}");
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    assert!(v.chat_sent.is_empty(), "{:?}", v.chat_sent);
    assert!(
        v.psent
            .iter()
            .all(|r| !matches!(r.call, PCall::SpecPdf(_) | PCall::SpecProject(_))),
        "{:?}",
        v.psent
    );
    assert!(v.spec.spec.error.as_deref().unwrap().contains("only an engineer"));
    // Signed out as well.
    let mut v = page(&b, &jar, None);
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    assert!(v.chat_sent.is_empty(), "{:?}", v.chat_sent);
    assert!(v.spec.spec.error.as_deref().unwrap().contains("Sign in"));
}

#[test]
fn stop_and_a_failed_run_leave_no_draft_and_discard_writes_nothing() {
    let b = Bench::new();
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    answer(&mut v, &good());
    run(&b, &jar, &mut v, &ctx);
    assert!(v.spec.review.is_some());
    v.discard_spec();
    assert!(v.spec.review.is_none() && !v.instruct.busy());
    let o = v.instruct.outcome.as_ref().unwrap();
    assert_eq!(o.status, "Discarded the draft from the spec. Nothing was made.");
    assert!(project_names(&b).is_empty(), "{:?}", project_names(&b));
    // A run the agent could not answer: no draft.
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    let seq = v.chat_sent.last().unwrap().seq;
    let body = "event: error\ndata: {\"error\":\"the relay is down\"}\n\n";
    v.agent.apply(Incoming::Chunk {
        seq,
        text: body.into(),
    });
    v.agent.apply(Incoming::End { seq });
    run(&b, &jar, &mut v, &ctx);
    assert!(
        v.spec.review.is_none() && !v.instruct.busy(),
        "{:?}",
        v.instruct.outcome
    );
    assert!(project_names(&b).is_empty(), "{:?}", project_names(&b));
    // An answer with no code: said, no draft.
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    answer(&mut v, "I cannot tell what the machine is.");
    run(&b, &jar, &mut v, &ctx);
    assert!(v.spec.review.is_none());
    assert_eq!(v.instruct.outcome.as_ref().map(|o| o.kind), Some(Kind::Said));
    // Stop with a request in flight.
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    assert!(v.instruct.busy());
    v.stop_run();
    assert!(!v.instruct.busy() && v.spec.review.is_none());
}

#[test]
fn a_number_input_is_listed_as_needing_a_device_and_deploy_says_what_to_do() {
    let b = Bench::new();
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    v.spec.spec.pasted = "The pump fills the tank while the level is below 20 percent.".into();
    v.send_spec(&ctx);
    answer(&mut v, &tank_answer());
    run(&b, &jar, &mut v, &ctx);
    let r = v.spec.review.as_ref().expect("a draft");
    assert_eq!(r.draft.needs_device(), ["level"]);
    v.accept_spec();
    run(&b, &jar, &mut v, &ctx);
    let map = b.json("/api/projects/tank/files/io.map")["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        !map.lines()
            .any(|l| l.trim_start().starts_with("in") && l.contains("level")),
        "{map}"
    );
    assert!(map.contains("in  stop_pb bool box di:0"), "{map}");
    assert!(map.contains("out pump bool box do:0 safe=false"), "{map}");
    let note = v.projects.notice.clone().unwrap();
    assert!(
        note.0.contains("level needs a device before Deploy") && note.0.contains("Assign to device I/O"),
        "{note:?}"
    );
    // What the box says for the input with no signal.
    let (status, j) = check(&b, &jar, "tank");
    assert_eq!(status, 200, "{j}");
    // Deploy is refused, and the words name the input and the way out.
    v.projects.deploy();
    run(&b, &jar, &mut v, &ctx);
    let Deploy::Failed(why) = v.projects.deploy.clone() else {
        panic!("{:?} {:?}", v.projects.deploy, v.projects.diags);
    };
    assert!(
        why.contains("port LEVEL: no signal of that name")
            && why.contains("Input level has no signal on this box")
            && why.contains("Assign to device I/O"),
        "{why}"
    );
}

#[test]
fn a_bool_only_spec_deploys_and_runs_on_the_box() {
    let b = Bench::build(true);
    let jar = Jar::default();
    let (mut v, ctx) = signed_in(&b, &jar);
    v.spec.spec.pasted = SPEC_TEXT.into();
    v.send_spec(&ctx);
    answer(&mut v, &good());
    run(&b, &jar, &mut v, &ctx);
    assert!(
        v.spec.review.as_ref().unwrap().draft.needs_device().is_empty(),
        "{:?}",
        v.spec.review.as_ref().unwrap().draft.needs_device()
    );
    v.accept_spec();
    run(&b, &jar, &mut v, &ctx);
    v.projects.deploy();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        run(&b, &jar, &mut v, &ctx);
        match &v.projects.deploy {
            Deploy::Done(_) => break,
            Deploy::Failed(why) => panic!("{why} {:?}", v.projects.diags),
            _ => {}
        }
        assert!(std::time::Instant::now() < deadline, "{:?}", v.projects.deploy);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let mods = b.json("/api/modules");
        if mods.to_string().contains("conveyor") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no module: {mods}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

// ---- the state alone ----

#[test]
fn a_pdf_answer_for_nothing_in_flight_changes_nothing_and_a_refusal_is_in_words() {
    use crate::box_spec::BoxSpec;
    let mut s = BoxSpec::new();
    assert_eq!(s.pdf_answer("ghost.pdf", 201, "{\"id\":1}"), None);
    s.spec.add_file("a.pdf", &pdf(100)).unwrap();
    s.spec.add_file("b.pdf", &pdf(100)).unwrap();
    let reqs = s.upload_requests();
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs.iter()
            .all(|r| r.url.starts_with("/api/dropins?name=") && r.url.ends_with("&kind=pdf"))
    );
    assert!(
        reqs.iter()
            .all(|r| r.ctype == "application/pdf" && r.method == "POST")
    );
    assert!(s.uploading);
    assert_eq!(
        s.pdf_answer("a.pdf", 201, "{\"id\":4}"),
        None,
        "b is still on its way"
    );
    // A 201 that names no id is not kept.
    assert_eq!(s.pdf_answer("b.pdf", 201, "{}"), Some(false));
    let why = s.spec.pdfs[1].refused.clone().unwrap();
    assert!(
        why.contains("did not say where") && why.contains("Nothing was sent to the agent"),
        "{why}"
    );
    assert_eq!(s.dropins(), [4]);
    assert!(!s.uploading);
    // Sent again after the person took it out and added it back.
    s.spec.remove_pdf(1);
    s.spec.add_file("b.pdf", &pdf(100)).unwrap();
    assert_eq!(s.upload_requests().len(), 1, "only the one not kept");
    assert_eq!(s.pdf_answer("b.pdf", 201, "{\"id\":5}"), Some(true));
    assert_eq!(s.dropins(), [4, 5]);
    // The box's refusals, each in words naming the file.
    for (status, body, says) in [
        (413, "{\"error\":\"a PDF is at most 2 MiB\"}", "too large"),
        (415, "{\"error\":\"this is not a PDF\"}", "does not take"),
        (401, "", "Sign in again"),
        (0, "connection reset", "did not answer"),
        (500, "{\"error\":\"disk full\"}", "disk full"),
    ] {
        let mut s = BoxSpec::new();
        s.spec.add_file("x.pdf", &pdf(100)).unwrap();
        s.upload_requests();
        assert_eq!(s.pdf_answer("x.pdf", status, body), Some(false), "{status}");
        let why = s.spec.pdfs[0].refused.clone().unwrap();
        assert!(
            why.contains(says) && why.contains("Nothing was sent to the agent"),
            "{status}: {why}"
        );
    }
}

#[test]
fn a_draft_waits_for_its_run_and_accept_refuses_twice_in_a_row() {
    use crate::box_spec::BoxSpec;
    let mut s = BoxSpec::new();
    assert!(s.spec.on_box && s.review.is_none() && !s.accepting);
    let apply = crate::instruct::Apply {
        project: String::new(),
        scope: crate::scope::Scope::NewProject,
        file: "main.st".into(),
        text: sm::MOTOR.into(),
        tests: Vec::new(),
        warnings: Vec::new(),
        record: String::new(),
        status: String::new(),
        why: None,
        instruction: "From a spec: Pasted".into(),
        evidence: Box::new(crate::instruct::Evidence {
            answer: String::new(),
            ran: Vec::new(),
        }),
    };
    // No documents held: no draft (the run was never started here).
    assert!(!s.drafted(apply.clone(), &[]));
    s.spec.pasted = "Belt.".into();
    s.begin_run();
    assert!(s.drafted(apply, &["conveyor".into()]));
    assert_eq!(s.review.as_ref().unwrap().name, "conveyor-2");
    assert!(s.accept(&["conveyor".into()]).is_some());
    assert!(s.accepting);
    assert!(
        s.accept(&[]).is_none(),
        "a second Accept while the first is on its way"
    );
    s.refused("conveyor-2", 409, "{\"error\":\"taken\"}");
    assert!(!s.accepting);
    assert!(
        s.review
            .as_ref()
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("Nothing was written")
    );
    assert!(s.drop_run());
    assert!(!s.drop_run());
}

#[test]
fn the_words_after_a_refused_deploy_name_every_input_with_no_signal_and_change_nothing_else() {
    use crate::box_projects::deploy_words;
    let one = "x was not deployed: modules do not fit together: tank: port LEVEL: no signal of that name"
        .to_string();
    let said = deploy_words(one.clone());
    assert!(said.starts_with(&one), "the box's own words stay first: {said}");
    assert!(said.contains("Input level has no signal on this box"), "{said}");
    let two = "tank: port LEVEL: no signal of that name; tank: port Temp_1: no signal of that name; tank: port LEVEL: no signal of that name";
    let said = deploy_words(two.into());
    assert!(
        said.contains("Input level, temp_1 have no signal on this box"),
        "each once, lower case: {said}"
    );
    // Other refusals are left exactly as the box said them, and so is a
    // sentence that only looks like one.
    for other in [
        "x was not deployed: a module does not build",
        "no signal of that name",
        ": no signal of that name",
        "the word port: no signal of that name",
        "",
    ] {
        assert_eq!(deploy_words(other.into()), other, "{other:?}");
    }
}
