//! The device profile the agent proposed, on the Devices tab
//! (`crate::device_proposal`): shown, renamed in place, saved into the
//! open project or discarded, every refusal beside what caused it, and
//! all of it inside the panel's default width. The box's answers are
//! scripted here; the box itself answers in `box_projects_tests`.

use eframe::egui::{Event, Key, Modifiers};
use serde_json::{Value as Json, json};

use super::{App, Harness, all_text};
use crate::box_projects::{Buf, OpenProject, PCall, PReq};
use crate::box_tests::{Bench, watch};
use crate::review::{Answer, Session};

pub(super) const PROFILE: &str = "helmsman-device 1\nvendor Acme\nmodel Rio 8\nkind remote-io\n\
transport modbus-tcp unit=1\nin DI-00 bool modbus-tcp=di:0\nin TEMP-1 real unit=C modbus-tcp=ir:0:f32\n\
out DO-00 bool safe=false modbus-tcp=coil:0\n";

pub(super) fn proposal(id: u64, name: &str, summary: &str) -> String {
    json!({
        "id": id, "name": name, "summary": summary, "text": PROFILE, "at": 1, "by": "agent",
        "vendor": "Acme", "model": "Rio 8", "kind": "remote-io",
        "channels": [
            { "name": "DI-00", "dir": "in", "type": "bool", "unit": null, "safe": null,
              "points": { "modbus-tcp": "di:0" } },
            { "name": "TEMP-1", "dir": "in", "type": "real", "unit": "C", "safe": null,
              "points": { "modbus-tcp": "ir:0:f32" } },
            { "name": "DO-00", "dir": "out", "type": "bool", "unit": null, "safe": "false",
              "points": { "modbus-tcp": "coil:0" } },
        ],
    })
    .to_string()
}

pub(super) fn buf(text: &str) -> Buf {
    Buf {
        text: text.into(),
        base: text.into(),
        loaded: true,
        ..Buf::default()
    }
}

/// A box view signed in as `role`, a project open, the Devices tab shown;
/// the box's first read of the proposal is on its way (not answered).
pub(super) fn open(role: &str) -> Harness {
    let bench = Bench::new();
    let (state, _) = watch(&bench);
    let mut h = Harness::new(App::boxed());
    let v = h.app.boxv.as_mut().unwrap();
    v.state = state;
    v.projects.available = Some(true);
    h.app.review.as_mut().unwrap().state.session = Some(Session {
        user: Some("alice".into()),
        role: role.into(),
    });
    let v = h.app.boxv.as_mut().unwrap();
    let mut o = OpenProject {
        name: "demo".into(),
        ..OpenProject::default()
    };
    o.bufs.insert("io.map".into(), buf("in start bool box di:0\n"));
    v.projects.open = Some(o);
    h.frame(vec![]);
    h.press("pane devices");
    h.wait(0.3);
    h
}

pub(super) fn sent(h: &mut Harness) -> Vec<PReq> {
    h.frame(vec![]);
    std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent)
}

pub(super) fn answer(h: &mut Harness, call: PCall, status: u16, body: &str) {
    let ctx = h.ctx.clone();
    let a = Answer {
        status,
        retry_after: None,
        body: body.into(),
    };
    let t = h.time;
    h.app
        .boxv
        .as_mut()
        .unwrap()
        .step_all(&ctx, Vec::new(), vec![(call, a)], t);
    h.wait(0.2);
}

/// A box with a proposal `body` read by the page.
fn with(role: &str, body: &str) -> Harness {
    let mut h = open(role);
    let first = sent(&mut h);
    assert!(
        first
            .iter()
            .any(|r| r.call == PCall::Proposal && r.method == "GET"),
        "the tab reads the proposal: {first:?}"
    );
    answer(&mut h, PCall::Proposal, 200, body);
    h
}

fn type_into(h: &mut Harness, key: &str, text: &str) {
    h.press(key);
    for pressed in [true, false] {
        h.frame(vec![Event::Key {
            key: Key::A,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }]);
    }
    h.frame(vec![Event::Text(text.into())]);
    h.wait(0.1);
}

fn calls(reqs: &[PReq], call: &PCall) -> Vec<(&'static str, String, Json)> {
    reqs.iter()
        .filter(|r| &r.call == call)
        .map(|r| {
            (
                r.method,
                r.url.clone(),
                r.body
                    .as_ref()
                    .map_or(Json::Null, |b| serde_json::from_slice(b).unwrap()),
            )
        })
        .collect()
}

fn proposal_ui(h: &Harness) -> &crate::device_proposal::ProposalUi {
    &h.app.boxv.as_ref().unwrap().devices_ui.proposal
}

#[test]
fn nothing_pending_shows_nothing() {
    let mut h = open("engineer");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::Proposal,
        404,
        r#"{"error":"no device profile is waiting for you"}"#,
    );
    assert!(!h.has("proposal"));
    assert!(!all_text(&mut h).contains("Proposed device profile"));
}

#[test]
fn a_signed_out_page_never_reads_it() {
    let mut h = open("engineer");
    h.app.review.as_mut().unwrap().state.session = None;
    h.app.boxv.as_mut().unwrap().session = None;
    let _ = sent(&mut h);
    h.wait(4.0);
    let more = sent(&mut h);
    assert!(
        calls(&more, &PCall::Proposal).is_empty(),
        "nobody signed in: {more:?}"
    );
}

#[test]
fn the_proposal_shows_its_name_summary_and_channels() {
    let mut h = with(
        "engineer",
        &proposal(7, "acme-rio", "Read from the manual, table 4."),
    );
    for k in [
        "proposal",
        "proposal name",
        "proposal channel DI-00",
        "proposal channel TEMP-1",
        "proposal channel DO-00",
        "proposal save",
        "proposal discard",
    ] {
        assert!(h.has(k), "no {k}");
    }
    let t = all_text(&mut h);
    for want in [
        "Proposed device profile",
        "Acme Rio 8",
        "Read from the manual, table 4.",
        "3 channel(s)",
        "input, bool",
        "input, real in C",
        "output, bool, safe value false",
    ] {
        assert!(t.contains(want), "{want:?} not in:\n{t}");
    }
    assert!(!t.contains("modbus"), "no protocol on the face of it");
}

#[test]
fn renaming_a_channel_and_saving_sends_exactly_the_accept() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    type_into(&mut h, "proposal channel DI-00", "part_present");
    type_into(&mut h, "proposal name", "rio-line3");
    h.press("proposal save");
    let reqs = sent(&mut h);
    assert_eq!(
        calls(&reqs, &PCall::ProposalAccept),
        [(
            "POST",
            "/api/device-profile/proposal/accept".to_string(),
            json!({ "id": 7, "project": "demo", "name": "rio-line3", "channels": { "DI-00": "part_present" } })
        )]
    );
    assert!(h.has("proposal save"), "still there until the box answers");
    assert!(proposal_ui(&h).saving);
    // Saved with no change: only the id and the project go.
    let mut h = with("engineer", &proposal(8, "acme-rio", "s"));
    h.press("proposal save");
    let reqs = sent(&mut h);
    assert_eq!(
        calls(&reqs, &PCall::ProposalAccept)[0].2,
        json!({ "id": 8, "project": "demo" })
    );
}

#[test]
fn a_save_the_box_took_closes_the_proposal_and_reads_the_project_again() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    h.press("proposal save");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::ProposalAccept,
        201,
        r#"{"project":"demo","file":"acme-rio.device","version":"v1","revision":3}"#,
    );
    assert!(!h.has("proposal save"), "the proposal is gone");
    let reqs = sent(&mut h);
    assert!(
        reqs.iter().any(|r| r.call == PCall::Project("demo".into())),
        "the project is read again: {reqs:?}"
    );
    let t = all_text(&mut h);
    assert!(t.contains("Saved acme-rio.device in demo."), "{t}");
    assert!(t.contains("Reading it back from the project"), "{t}");
    // The file arrives as the project holds it: its channels are listed.
    h.app
        .boxv
        .as_mut()
        .unwrap()
        .projects
        .open
        .as_mut()
        .unwrap()
        .bufs
        .insert("acme-rio.device".into(), buf(PROFILE));
    h.wait(0.2);
    let t = all_text(&mut h);
    for want in [
        "3 channel(s), as the project holds them:",
        "in  DI-00  bool",
        "in  TEMP-1  real C",
        "out  DO-00  bool",
    ] {
        assert!(t.contains(want), "{want:?} not in:\n{t}");
    }
    h.press("proposal done");
    h.wait(0.1);
    assert!(!h.has("proposal done"));
}

#[test]
fn discard_sends_the_delete_and_the_proposal_goes() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    h.press("proposal discard");
    let reqs = sent(&mut h);
    assert_eq!(
        calls(&reqs, &PCall::ProposalDiscard),
        [("DELETE", "/api/device-profile/proposal".to_string(), Json::Null)]
    );
    answer(&mut h, PCall::ProposalDiscard, 200, r#"{"discarded":"acme-rio"}"#);
    assert!(!h.has("proposal"));
}

#[test]
fn a_problem_the_box_found_is_beside_the_channel_it_belongs_to() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    type_into(&mut h, "proposal channel TEMP-1", "DI_00");
    h.press("proposal save");
    let _ = sent(&mut h);
    // PROFILE's line 7 is `in TEMP-1`.
    answer(
        &mut h,
        PCall::ProposalAccept,
        422,
        r#"{"error":"the profile does not check after the rename","problems":[
            {"line":7,"error":"channel DI_00 gives the same signal as DI-00"},
            {"line":0,"error":"something about the whole file"}]}"#,
    );
    let t = all_text(&mut h);
    assert!(t.contains("channel DI_00 gives the same signal as DI-00"), "{t}");
    assert!(
        t.contains("The profile does not check: something about the whole file"),
        "{t}"
    );
    assert_eq!(
        proposal_ui(&h).refusals.channels.keys().collect::<Vec<_>>(),
        ["TEMP-1"],
        "only that channel carries it"
    );
    assert!(
        h.has("proposal save"),
        "the proposal stays, to fix and save again"
    );
    assert_eq!(
        proposal_ui(&h).edits["TEMP-1"],
        "DI_00",
        "what was typed is still typed"
    );
}

#[test]
fn a_bad_device_name_is_said_beside_the_name() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    type_into(&mut h, "proposal name", "Bad Name");
    h.press("proposal save");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::ProposalAccept,
        422,
        r#"{"error":"the name is a file name without .device: 1 to 48 of a-z 0-9 _ -","problems":[]}"#,
    );
    assert!(proposal_ui(&h).refusals.name.is_some());
    assert_eq!(proposal_ui(&h).refusals.general.len(), 0);
    let t = all_text(&mut h);
    assert!(t.contains("A name is 1 to 48 characters"), "{t}");
}

#[test]
fn a_newer_proposal_replaces_it_and_keeps_the_names_typed_where_the_channel_remains() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "old summary"));
    type_into(&mut h, "proposal channel DI-00", "part_present");
    type_into(&mut h, "proposal channel DO-00", "lamp");
    type_into(&mut h, "proposal name", "mine");
    h.press("proposal save");
    let _ = sent(&mut h);
    // The agent replaced it meanwhile: the box says 409, the page reads
    // again and finds another id, without DO-00.
    answer(
        &mut h,
        PCall::ProposalAccept,
        409,
        r#"{"error":"the proposal changed since you looked at it; read it again"}"#,
    );
    let reqs = sent(&mut h);
    assert_eq!(
        calls(&reqs, &PCall::Proposal).len(),
        1,
        "read again at once: {reqs:?}"
    );
    let newer = proposal(9, "acme-rio-2", "new summary").replace("DO-00", "DO-01");
    answer(&mut h, PCall::Proposal, 200, &newer);
    let t = all_text(&mut h);
    assert!(t.contains("The agent proposed a newer version"), "{t}");
    assert!(t.contains("new summary"), "{t}");
    let ui = proposal_ui(&h);
    assert_eq!(
        ui.edits["DI-00"], "part_present",
        "kept where the channel is there"
    );
    assert_eq!(ui.edits["DO-01"], "DO-01", "the new channel is as proposed");
    assert!(!ui.edits.contains_key("DO-00"));
    assert_eq!(ui.name, "mine", "the name typed is kept");
    // Saving again sends the new id with the kept renames.
    h.press("proposal save");
    let reqs = sent(&mut h);
    assert_eq!(
        calls(&reqs, &PCall::ProposalAccept)[0].2,
        json!({ "id": 9, "project": "demo", "name": "mine", "channels": { "DI-00": "part_present" } })
    );
}

#[test]
fn a_file_already_there_is_said_beside_the_name_and_nothing_else_changes() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    type_into(&mut h, "proposal channel DI-00", "part_present");
    h.press("proposal save");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::ProposalAccept,
        409,
        r#"{"error":"acme-rio.device already exists"}"#,
    );
    let _ = sent(&mut h);
    // The read finds the same proposal: the 409 was the name.
    answer(&mut h, PCall::Proposal, 200, &proposal(7, "acme-rio", "s"));
    let t = all_text(&mut h);
    assert!(
        t.contains("The project already has a file called acme-rio.device. Give the device another name"),
        "{t}"
    );
    assert!(!t.contains("newer version"));
    assert_eq!(
        proposal_ui(&h).edits["DI-00"],
        "part_present",
        "the typed name stays"
    );
}

#[test]
fn the_other_refusals_are_said_in_plain_words() {
    for (status, body, want) in [
        (
            403,
            r#"{"error":"device profile accept needs an engineer login; this session is operator alice"}"#,
            "Only an engineer can save a device profile. Sign in as one; nothing was saved.",
        ),
        (
            401,
            r#"{"error":"x"}"#,
            "The box no longer knows this page's session. Sign in again; nothing was saved.",
        ),
        (
            404,
            r#"{"error":"no such project line9"}"#,
            "Nothing was saved: no such project line9",
        ),
        (
            0,
            "connection refused",
            "The box did not answer; nothing was saved (connection refused).",
        ),
        (
            500,
            r#"{"error":"disk full"}"#,
            "Nothing was saved: the box refused it (500): disk full",
        ),
    ] {
        let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
        h.press("proposal save");
        let _ = sent(&mut h);
        answer(&mut h, PCall::ProposalAccept, status, body);
        let t = all_text(&mut h);
        assert!(t.contains(want), "{status}: {want:?} not in:\n{t}");
        assert!(h.has("proposal save"), "{status}: still there to try again");
    }
    // The proposal is gone: said, and nothing offered.
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    h.press("proposal save");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::ProposalAccept,
        404,
        r#"{"error":"no device profile is waiting for you"}"#,
    );
    assert!(!h.has("proposal save"));
    assert!(all_text(&mut h).contains("The proposal is gone"));
}

#[test]
fn a_session_that_may_not_save_is_not_offered_save() {
    let mut h = with("operator", &proposal(7, "acme-rio", "s"));
    assert!(h.has("proposal"), "it can read it");
    assert!(!h.has("proposal save") && !h.has("proposal discard"));
    let t = all_text(&mut h);
    assert!(t.contains("only an engineer"), "{t}");
}

#[test]
fn with_no_project_open_save_waits_for_one() {
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    h.app.boxv.as_mut().unwrap().projects.open = None;
    h.wait(0.2);
    let t = all_text(&mut h);
    assert!(
        t.contains("Open a project in Files to save the profile into it."),
        "{t}"
    );
    let at = h.rect("proposal save").center();
    h.click(at);
    assert!(
        calls(&sent(&mut h), &PCall::ProposalAccept).is_empty(),
        "nothing is sent without a project"
    );
}

/// 256 channels with long names, a summary of 8000 characters, bidi and
/// control characters: the panels stay as wide as they were.
#[test]
fn a_hostile_proposal_leaves_the_panels_their_width() {
    let mut h = open("engineer");
    let _ = sent(&mut h);
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.3);
    let left = h.rect("left");
    let right = h.rect("right");
    let channels: Vec<Json> = (0..256)
        .map(|k| {
            json!({ "name": format!("N{k:03}-{}", "x".repeat(24)), "dir": "out", "type": "real",
                    "unit": "\u{202E}kilopascals-per-square-furlong-and-more-and-more-and-more-and-more",
                    "safe": "\u{0007}1234567890123456789012345678901234567890123456789012345678901234567890",
                    "points": { "modbus-tcp": "ir:0:f32" } })
        })
        .collect();
    let summary = format!("{}\u{202E}\u{0000}evil {}", "wide ".repeat(1500), "w".repeat(600));
    let body = json!({
        "id": 3, "name": "n".repeat(48), "summary": summary, "text": "", "vendor": "V".repeat(200),
        "model": "\u{202E}M\u{200F}".repeat(40), "channels": channels,
    })
    .to_string();
    answer(&mut h, PCall::Proposal, 200, &body);
    h.wait(0.3);
    assert!(h.has("proposal save"));
    let t = all_text(&mut h);
    assert!(!t.contains('\u{202E}') && !t.contains('\u{0007}') && !t.contains('\u{200F}'));
    for (what, was, now) in [("left", left, h.rect("left")), ("right", right, h.rect("right"))] {
        let grew = now.right() - was.right();
        let moved = was.left() - now.left();
        assert!(
            (-8.0..=8.0).contains(&grew) && (-8.0..=8.0).contains(&moved),
            "the {what} panel changed by {grew}/{moved} pt"
        );
    }
    // Every control of the section is inside the left panel.
    let edge = h.rect("left").right();
    for k in [
        "proposal name",
        "proposal save",
        "proposal discard",
        "proposal channel N000-xxxxxxxxxxxxxxxxxxxxxxxx",
    ] {
        let r = h.rect(k);
        assert!(
            r.right() <= edge + 0.5,
            "{k} at {r:?} past the panel's edge {edge}"
        );
    }
    let small = h.small_text();
    assert!(small.is_empty(), "text under the smallest size: {small:?}");
}

#[test]
fn the_panel_keeps_its_default_width_with_a_proposal_and_a_refusal_showing() {
    let mut h = open("engineer");
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.3);
    let left = h.rect("left");
    let right = h.rect("right");
    answer(
        &mut h,
        PCall::Proposal,
        200,
        &proposal(7, "acme-rio", &"A sentence the agent wrote. ".repeat(40)),
    );
    h.press("proposal save");
    answer(
        &mut h,
        PCall::ProposalAccept,
        422,
        &json!({
            "error": "the profile does not check",
            "problems": [{"line": 6, "error": "x".repeat(300)}, {"line": 0, "error": "y".repeat(300)}]
        })
        .to_string(),
    );
    for k in ["pane files", "pane devices", "tab claude", "pane devices"] {
        h.press(k);
        h.wait(0.3);
        let l = h.rect("left").right() - left.right();
        let r = right.left() - h.rect("right").left();
        assert!((0.0..=8.0).contains(&l), "after {k} the left pane grew {l} pt");
        assert!((0.0..=8.0).contains(&r), "after {k} the right panel grew {r} pt");
    }
}

// ---- publishing the saved profile to the library (card `device-profile-publish`) ----

/// An engineer's page with `acme-rio.device` saved into `demo` and read
/// back from the project, the proposal gone.
fn saved(role: &str, file_text: &str) -> Harness {
    let mut h = with(role, &proposal(7, "acme-rio", "s"));
    h.press("proposal save");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::ProposalAccept,
        201,
        r#"{"project":"demo","file":"acme-rio.device","version":"v1","revision":3}"#,
    );
    let _ = sent(&mut h);
    h.app
        .boxv
        .as_mut()
        .unwrap()
        .projects
        .open
        .as_mut()
        .unwrap()
        .bufs
        .insert("acme-rio.device".into(), buf(file_text));
    h.wait(0.2);
    h
}

/// Asks to publish and confirms: the confirmation is drawn a frame after
/// the click.
fn send(h: &mut Harness) {
    h.press("proposal publish");
    h.wait(0.1);
    h.press("proposal publish send");
}

const LINK: &str = "https://github.com/JimothyJohn/helmsman-devices/pull/12";

fn sent_ok(number: u64, url: &str, created: bool) -> String {
    json!({ "number": number, "url": url, "created": created, "draft": true, "folder": "devices/acme/rio-8" })
        .to_string()
}

#[test]
fn publishing_is_offered_only_for_a_profile_saved_and_read_back() {
    // Not before the project hands the saved file back.
    let mut h = with("engineer", &proposal(7, "acme-rio", "s"));
    assert!(!h.has("proposal publish"), "no saved profile yet");
    h.press("proposal save");
    let _ = sent(&mut h);
    answer(
        &mut h,
        PCall::ProposalAccept,
        201,
        r#"{"project":"demo","file":"acme-rio.device","version":"v1","revision":3}"#,
    );
    assert!(
        !h.has("proposal publish"),
        "saved, but not read back from the project"
    );
    h.app
        .boxv
        .as_mut()
        .unwrap()
        .projects
        .open
        .as_mut()
        .unwrap()
        .bufs
        .insert("acme-rio.device".into(), buf(PROFILE));
    h.wait(0.2);
    assert!(h.has("proposal publish"), "saved and read back");
    // A saved file that does not read as a profile is not offered.
    let h = saved("engineer", "this is not a device profile\n");
    assert!(!h.has("proposal publish"));
    // Closing the note takes the button with it.
    let mut h = saved("engineer", PROFILE);
    h.press("proposal done");
    h.wait(0.1);
    assert!(!h.has("proposal publish"));
    // Its words: the button names the library, with the ellipsis of a step that asks first.
    let mut h = saved("engineer", PROFILE);
    assert!(all_text(&mut h).contains("Publish to the library…"));
}

#[test]
fn nothing_is_sent_until_the_person_confirms_and_cancel_sends_nothing() {
    let mut h = saved("engineer", PROFILE);
    h.press("proposal publish");
    let reqs = sent(&mut h);
    assert!(
        calls(&reqs, &PCall::ProposalPublish).is_empty(),
        "asking is not sending"
    );
    let t = all_text(&mut h);
    assert!(
        t.contains("Send acme-rio.device to the public device library?"),
        "the confirmation names the file: {t}"
    );
    assert!(t.contains("Only this profile file is sent."), "{t}");
    h.press("proposal publish cancel");
    let reqs = sent(&mut h);
    assert!(calls(&reqs, &PCall::ProposalPublish).is_empty());
    assert!(h.has("proposal publish"), "back to the button");
    // Confirm: exactly one request, the project and the file name only.
    send(&mut h);
    let reqs = sent(&mut h);
    assert_eq!(
        calls(&reqs, &PCall::ProposalPublish),
        [(
            "POST",
            "/api/device-profile/publish".to_string(),
            json!({ "project": "demo", "file": "acme-rio.device" })
        )],
        "the box reads the file itself: no text goes from the page"
    );
    assert!(
        !h.has("proposal publish send"),
        "no second send while one is on its way"
    );
    assert!(all_text(&mut h).contains("Sending…"));
    assert!(calls(&sent(&mut h), &PCall::ProposalPublish).is_empty());
}

#[test]
fn the_review_request_comes_back_as_a_number_and_a_library_link() {
    let mut h = saved("engineer", PROFILE);
    send(&mut h);
    let _ = sent(&mut h);
    answer(&mut h, PCall::ProposalPublish, 201, &sent_ok(12, LINK, true));
    let t = all_text(&mut h);
    assert!(
        t.contains("Sent. The library's maintainers have review request #12."),
        "{t}"
    );
    assert!(t.contains("Open request #12"), "{t}");
    assert!(h.has("proposal publish link"));
    assert!(!h.has("proposal publish"), "nothing to send again");
    // The same profile again: said as it is.
    let mut h = saved("engineer", PROFILE);
    send(&mut h);
    let _ = sent(&mut h);
    answer(&mut h, PCall::ProposalPublish, 200, &sent_ok(12, LINK, false));
    assert!(all_text(&mut h).contains("Already sent. Review request #12 is still open."));
    // A link is only ever a library review page: anything else is not drawn.
    for url in [
        "https://evil.example/o/r/pull/12",
        "http://github.com/o/r/pull/12",
        "https://github.com/o/r/pull/13",
        "https://github.com.evil.example/o/r/pull/12",
        "javascript:alert(1)",
        "https://github.com/o/r/pull/12 and more",
        "",
    ] {
        let mut h = saved("engineer", PROFILE);
        send(&mut h);
        let _ = sent(&mut h);
        answer(&mut h, PCall::ProposalPublish, 201, &sent_ok(12, url, true));
        assert!(!h.has("proposal publish link"), "{url}");
        assert!(all_text(&mut h).contains("review request #12"), "{url}");
    }
}

#[test]
fn every_refusal_says_why_nothing_was_sent_and_the_button_comes_back() {
    for (status, body, want) in [
        (
            422,
            r#"{"error":"the device library has no entry for Acme Rio 8: a profile is added to an entry that exists","kind":"invalid_request"}"#,
            "Not sent: the device library has no entry for Acme Rio 8",
        ),
        (
            409,
            r#"{"error":"the library already has a profile for this device"}"#,
            "Not sent: the library already has a profile",
        ),
        (
            429,
            r#"{"error":"at most 6 publishes an hour from one box"}"#,
            "Not sent: at most 6 publishes an hour",
        ),
        (
            503,
            r#"{"error":"publishing is not set up on this relay: it has no GitHub token"}"#,
            "Not sent: publishing is not set up",
        ),
        (
            502,
            r#"{"error":"the agent service could not be reached. Nothing was sent"}"#,
            "Not sent: the agent service could not be reached",
        ),
        (403, r#"{"error":"x"}"#, "Not sent: only an engineer can publish"),
        (
            401,
            r#"{"error":"x"}"#,
            "Not sent: the box no longer knows this page's session",
        ),
        (
            404,
            r#"{"error":"no projects on this box (--projects)"}"#,
            "Not sent: no projects on this box",
        ),
        (500, "not json", "Not sent: the box refused it (500)"),
        (
            0,
            "connection reset",
            "Not sent: the box did not answer (connection reset)",
        ),
        (
            201,
            r#"{"url":"x"}"#,
            "Not sent: the box's answer was not understood",
        ),
    ] {
        let mut h = saved("engineer", PROFILE);
        send(&mut h);
        let _ = sent(&mut h);
        answer(&mut h, PCall::ProposalPublish, status, body);
        let t = all_text(&mut h);
        assert!(t.contains(want), "{status}: {want:?} not in:\n{t}");
        assert!(h.has("proposal publish"), "{status}: the button is back");
        assert!(!h.has("proposal publish link"), "{status}");
    }
}

#[test]
fn words_from_the_box_are_printable_and_bounded() {
    let mut h = saved("engineer", PROFILE);
    send(&mut h);
    let _ = sent(&mut h);
    let evil = format!("a\u{202E}b\u{0007}c {}", "wide ".repeat(400));
    answer(
        &mut h,
        PCall::ProposalPublish,
        409,
        &json!({ "error": evil }).to_string(),
    );
    let t = all_text(&mut h);
    assert!(!t.contains('\u{202E}') && !t.contains('\u{0007}'));
    assert!(t.contains("Not sent: abc wide"), "{t}");
    assert!(t.len() < 6000);
    // An answer that arrives when nothing was sent changes nothing.
    let mut h = saved("engineer", PROFILE);
    answer(&mut h, PCall::ProposalPublish, 201, &sent_ok(12, LINK, true));
    assert!(h.has("proposal publish"), "an unasked answer is ignored");
    assert!(!all_text(&mut h).contains("review request"));
}

#[test]
fn a_session_that_changes_takes_the_offer_away_until_the_project_is_read_again() {
    let mut h = saved("engineer", PROFILE);
    assert!(h.has("proposal publish"));
    h.app.review.as_mut().unwrap().state.session = Some(Session {
        user: Some("bob".into()),
        role: "operator".into(),
    });
    h.wait(0.2);
    assert!(
        !h.has("proposal publish"),
        "the project closed with the session: nothing is read back to publish"
    );
    assert!(calls(&sent(&mut h), &PCall::ProposalPublish).is_empty());
}

#[test]
fn the_panel_keeps_its_default_width_while_publishing_every_state() {
    let mut h = saved("engineer", PROFILE);
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.wait(0.3);
    let left = h.rect("left");
    let right = h.rect("right");
    let check = |h: &mut Harness, what: &str| {
        for k in ["pane files", "pane devices", "tab claude", "pane devices"] {
            h.press(k);
            h.wait(0.3);
            let l = h.rect("left").right() - left.right();
            let r = right.left() - h.rect("right").left();
            assert!(
                (0.0..=8.0).contains(&l),
                "{what}: after {k} the left pane grew {l} pt"
            );
            assert!(
                (0.0..=8.0).contains(&r),
                "{what}: after {k} the right panel grew {r} pt"
            );
        }
        let small = h.small_text();
        assert!(
            small.is_empty(),
            "{what}: text under the smallest size: {small:?}"
        );
    };
    check(&mut h, "the button");
    h.press("proposal publish");
    check(&mut h, "the confirmation");
    h.press("proposal publish send");
    check(&mut h, "sending");
    let long = format!("{} {}", "a refusal in words".repeat(30), "w".repeat(250));
    answer(
        &mut h,
        PCall::ProposalPublish,
        409,
        &json!({ "error": long }).to_string(),
    );
    check(&mut h, "a long refusal");
    h.press("proposal publish");
    h.wait(0.2);
    h.press("proposal publish send");
    answer(
        &mut h,
        PCall::ProposalPublish,
        201,
        &sent_ok(
            12345678,
            "https://github.com/JimothyJohn/helmsman-devices/pull/12345678",
            true,
        ),
    );
    check(&mut h, "the review request");
    let edge = h.rect("left").right();
    assert!(h.rect("proposal publish link").right() <= edge + 0.5);
}
