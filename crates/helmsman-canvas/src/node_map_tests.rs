//! The Skipper's map from the Devices tab (card node-comms-check, part 2),
//! against the real Skipper API: `helmsman-nodecore`'s router answers every
//! call the canvas makes, and its replies are read by the canvas's own
//! code. Nothing here is a canned reply except the hostile ones, which no
//! Skipper would send.

use helmsman_nodecore::api::NodeView;
use helmsman_nodecore::check::{Check, Verdict as Said};
use helmsman_nodecore::http::{Read, parse};
use helmsman_nodecore::router::{Effect, Outcome, check_started, handle, io_map_saved};
use helmsman_nodecore::settings::Settings;
use serde_json::json;

use crate::box_projects::{PCall, PReq};
use crate::node_map::{DeviceCheck, NodeMapUi, NodeOp, Report, Stage, Verdict};

pub(crate) const TOKEN: &str = "bench-token-2026";
const MAP: &str =
    "device rio modbus-tcp 192.168.3.60\ndevice bench modbus-tcp 192.168.3.10:1502\nin estop bool rio di:0\n";

/// A Skipper: the real router over a node's view, and what the firmware
/// does with the router's effects (the P4's check task, its save).
pub(crate) struct Skipper {
    pub(crate) view: NodeView,
    settings: Settings,
}

impl Skipper {
    pub(crate) fn new() -> Self {
        let mut view = crate::node_api_tests::view();
        view.io_map = "device rio modbus-tcp 192.168.3.60\n".into();
        view.check = Some(Check::default());
        view.uptime_ms = 100_000;
        Self {
            view,
            settings: Settings {
                token: TOKEN.into(),
                ..Settings::default()
            },
        }
    }

    /// A board that does not check (the ESP32's).
    fn plain() -> Self {
        let mut s = Self::new();
        s.view.check = None;
        s
    }

    /// One call, as the browser sends it: the token as the canvas's
    /// transport carries it, `token` being what the page holds.
    pub(crate) fn call(&mut self, r: &PReq, token: Option<&str>) -> (u16, String) {
        let auth = token.map_or_else(String::new, |t| {
            let (name, value) = crate::node_token::header(t);
            format!("{name}: {value}\r\n")
        });
        let body = r.body.clone().unwrap_or_default();
        let mut raw = format!(
            "{} {} HTTP/1.1\r\nHost: skipper\r\n{auth}Content-Type: {}\r\nContent-Length: {}\r\n\r\n",
            r.method,
            r.url,
            r.ctype,
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(&body);
        let Ok(Read::Done(req, _)) = parse(&raw) else {
            panic!("the canvas sent a request the Skipper cannot read: {r:?}");
        };
        let reply = match handle(&req, &self.view, &self.settings) {
            Outcome::Reply(r) => r,
            Outcome::Effect(Effect::CheckIoMap { text, .. }) => {
                let map = helmsman_nodecore::iomap::parse(&text).expect("the router parsed it");
                self.view.check = Some(Check::begin(&map));
                check_started(false)
            }
            Outcome::Effect(Effect::SaveIoMap { text, .. }) => {
                self.view.io_map = text;
                // The check does not outlive the restart.
                self.view.check = Some(Check::default());
                io_map_saved(false, true)
            }
            // What both firmwares do: the wall time less the uptime, in RAM.
            Outcome::Effect(Effect::SetTime { unix_ms }) => {
                self.view.clock_offset_ms = Some(unix_ms - self.view.uptime_ms);
                helmsman_nodecore::router::time_set(unix_ms)
            }
            other => panic!("not a map call: {other:?}"),
        };
        (reply.status, String::from_utf8(reply.body).unwrap())
    }

    /// What device `k` said to the Skipper's check.
    pub(crate) fn says(&mut self, k: usize, v: Said) {
        self.view.check.as_mut().unwrap().set(k, v);
    }

    pub(crate) fn finish(&mut self) {
        let now = self.view.uptime_ms;
        self.view.check.as_mut().unwrap().finish(now);
    }

    /// Whether the Skipper would save `text` this instant.
    fn would_save(&mut self, text: &str) -> bool {
        let r = PReq::json(
            PCall::NodeMap(NodeOp::Save),
            "PUT",
            "/api/io_map".into(),
            &json!({ "text": text }),
        );
        let before = self.view.clone();
        let (status, _) = self.call(&r, Some(TOKEN));
        self.view = before;
        status == 200
    }
}

/// Carries every request the window made to the Skipper and its answer
/// back, as the browser would, until nothing more is asked.
fn pump(ui: &mut NodeMapUi, sk: &mut Skipper, token: Option<&str>) {
    for _ in 0..8 {
        let reqs = ui.take_requests();
        if reqs.is_empty() {
            return;
        }
        for r in reqs {
            let PCall::NodeMap(op) = r.call else {
                panic!("not a map call: {r:?}");
            };
            let (status, body) = sk.call(&r, token);
            ui.answer(op, status, &body);
        }
    }
}

/// A window on a Skipper that runs `view.io_map`, signed in, open and past
/// its first read.
fn open(sk: &mut Skipper, signed_in: bool) -> NodeMapUi {
    let mut ui = NodeMapUi::default();
    ui.open = true;
    ui.set_context(true, signed_in, Some(&sk.view.io_map));
    ui.tick(0.0);
    pump(&mut ui, sk, signed_in.then_some(TOKEN));
    ui
}

/// Polls until the Skipper's check is read, with a virtual clock.
fn settle(ui: &mut NodeMapUi, sk: &mut Skipper, mut now: f64) -> f64 {
    for _ in 0..200 {
        ui.tick(now);
        pump(ui, sk, Some(TOKEN));
        if !ui.waiting() {
            return now;
        }
        now += 1.0;
    }
    panic!("the page was still waiting after 200 s of virtual time");
}

fn type_into(ui: &mut NodeMapUi, text: &str) {
    ui.edit(text.to_string());
}

// ---- reading the check ----

#[test]
fn a_checks_every_state_is_read_from_the_skipper_itself() {
    let mut sk = Skipper::new();
    // None run yet.
    let r = Report::parse(&serde_json::from_str(&sk.call(&get(), None).1).unwrap()).unwrap();
    assert_eq!(
        (r.stage, r.passed, r.may_save, r.age_ms),
        (Stage::None, false, false, None)
    );
    assert!(r.devices.is_empty() && r.text.is_empty());
    // Running: both devices waiting.
    let mut ui = open(&mut sk, true);
    type_into(&mut ui, MAP);
    ui.check();
    pump(&mut ui, &mut sk, Some(TOKEN));
    ui.tick(1.0);
    pump(&mut ui, &mut sk, Some(TOKEN));
    let r = ui.report().expect("read after the start");
    assert_eq!(r.stage, Stage::Running);
    assert_eq!(r.text, MAP);
    assert_eq!(
        r.devices
            .iter()
            .map(|d| (&*d.name, &d.verdict))
            .collect::<Vec<_>>(),
        [("rio", &Verdict::Waiting), ("bench", &Verdict::Waiting)]
    );
    assert_eq!(r.devices[0].address, "192.168.3.60:502");
    // One answered, one did not.
    sk.says(0, Said::Answered { ms: 4 });
    sk.says(1, Said::Failed("connect timed out after 3 s".into()));
    sk.finish();
    sk.view.uptime_ms += 2500;
    settle(&mut ui, &mut sk, 1.0);
    let r = ui.report().unwrap();
    assert_eq!(
        (r.stage, r.passed, r.may_save, r.age_ms),
        (Stage::Done, false, false, Some(2500))
    );
    assert_eq!(r.devices[0].verdict, Verdict::Answered(Some(4)));
    assert_eq!(
        r.devices[1].verdict,
        Verdict::Failed("connect timed out after 3 s".into())
    );
    assert_eq!(
        r.devices[1].verdict.words(),
        "did not answer: connect timed out after 3 s"
    );
    assert_eq!(r.devices[0].verdict.words(), "answered in 4 ms");
}

fn get() -> PReq {
    PReq::get(PCall::NodeMap(NodeOp::Poll), "/api/io_map/check".into())
}

#[test]
fn hostile_and_broken_check_replies_read_as_nothing_or_clean_text() {
    let long = "x".repeat(100_000);
    let j = json!({
        "state": "done", "passed": false, "may_save": false, "age_ms": 5, "text": MAP,
        "devices": [
            {"name": "ri\u{0}o\u{1b}[31m\n", "address": "<b>1.2.3.4:502</b>", "state": "failed",
             "ms": null, "error": format!("a\r\nb\u{7}<script>alert(1)</script>{long}")},
            {"name": long, "address": long, "state": "answered", "ms": 99_999_999_999u64},
            {"name": 7, "state": "sideways"},
            "not an object",
            {"state": "answered", "ms": -3},
        ],
    });
    let r = Report::parse(&j).expect("a check with odd devices is still a check");
    assert_eq!(r.devices.len(), 4, "a device that is no object is no device");
    let a = &r.devices[0];
    assert_eq!(a.name, "rio[31m");
    assert!(!a.name.chars().any(char::is_control));
    assert_eq!(a.address, "<b>1.2.3.4:502</b>", "shown as text, never as markup");
    let Verdict::Failed(why) = &a.verdict else {
        panic!()
    };
    assert!(!why.chars().any(char::is_control));
    assert!(why.starts_with("ab<script>"), "{why}");
    assert_eq!(why.chars().count(), 240, "bounded");
    assert_eq!(r.devices[1].name.chars().count(), 48);
    assert_eq!(r.devices[1].address.chars().count(), 64);
    assert_eq!(
        r.devices[1].verdict,
        Verdict::Answered(None),
        "a time that does not fit is no time"
    );
    assert!(
        matches!(r.devices[2].verdict, Verdict::Failed(_)),
        "an unknown state is no answer"
    );
    assert_eq!(r.devices[2].name, "?");
    assert_eq!(r.devices[3].verdict, Verdict::Answered(None));

    // An empty list, and the fields missing or of the wrong type.
    let empty = Report::parse(&json!({"state": "none", "text": "", "devices": []})).unwrap();
    assert!(empty.devices.is_empty(), "an empty list is no devices");
    let bare = Report::parse(&json!({"state": "done", "text": MAP})).unwrap();
    assert!(bare.devices.is_empty() && !bare.passed && !bare.may_save && bare.age_ms.is_none());
    let typed = Report::parse(
        &json!({"state": "done", "text": MAP, "passed": "true", "may_save": 1, "devices": "x"}),
    )
    .unwrap();
    assert!(!typed.passed && !typed.may_save, "only a real true counts");
    for bad in [
        json!(null),
        json!([]),
        json!("done"),
        json!({}),
        json!({"state": 3, "text": MAP}),
        json!({"state": "finished", "text": MAP}),
        json!({"state": "done"}),
        json!({"state": "done", "text": 5}),
        json!({"state": "done", "text": long}),
    ] {
        assert!(Report::parse(&bad).is_none(), "{bad}");
    }
}

// ---- the flow ----

/// The Skipper's rule and the window's agree at every step, and a window
/// that is wrong is refused by the Skipper, not trusted.
#[test]
fn download_is_refused_before_a_pass_allowed_after_and_withdrawn_by_one_character() {
    let mut sk = Skipper::new();
    let mut ui = open(&mut sk, true);
    assert_eq!(
        ui.text(),
        "device rio modbus-tcp 192.168.3.60\n",
        "starts from the Skipper's own text"
    );
    assert!(!ui.can_download(), "nothing checked yet");
    ui.download();
    assert!(
        ui.take_requests().is_empty(),
        "no download is sent without a pass"
    );

    type_into(&mut ui, MAP);
    assert!(ui.edited());
    assert!(
        !ui.can_download() && !sk.would_save(MAP),
        "a changed map is not checked"
    );

    ui.check();
    pump(&mut ui, &mut sk, Some(TOKEN));
    assert!(ui.waiting(), "the check runs on the Skipper");
    assert!(!ui.can_download() && !sk.would_save(MAP), "not while it runs");
    ui.check();
    assert!(
        ui.take_requests().is_empty(),
        "one check at a time from this page"
    );

    sk.says(0, Said::Answered { ms: 3 });
    sk.says(1, Said::Failed("connection refused".into()));
    sk.finish();
    settle(&mut ui, &mut sk, 1.0);
    assert!(
        !ui.can_download() && !sk.would_save(MAP),
        "a silent device holds the map back"
    );

    // Again, and this time both answer.
    ui.check();
    pump(&mut ui, &mut sk, Some(TOKEN));
    sk.says(0, Said::Answered { ms: 3 });
    sk.says(1, Said::Answered { ms: 11 });
    sk.finish();
    settle(&mut ui, &mut sk, 10.0);
    assert!(ui.report().unwrap().may_save);
    assert!(ui.can_download() && sk.would_save(MAP), "both agree after a pass");

    // One character more, one fewer, a space at the end: the pass is of
    // the other text.
    for edited in [
        format!("{MAP}\n"),
        format!("{MAP}#"),
        MAP[..MAP.len() - 1].to_string(),
        MAP.replace("3.60", "3.61"),
    ] {
        type_into(&mut ui, &edited);
        assert!(!ui.can_download(), "{edited:?}");
        assert!(!sk.would_save(&edited), "the Skipper says the same of {edited:?}");
    }
    type_into(&mut ui, MAP);
    assert!(
        ui.can_download(),
        "back to the checked text, the pass applies again"
    );

    // Download: the Skipper saves and restarts.
    ui.download();
    pump(&mut ui, &mut sk, Some(TOKEN));
    assert_eq!(sk.view.io_map, MAP, "the Skipper has the map");
    assert!(!ui.can_download(), "once is enough");
    let note = ui.note_text().unwrap();
    assert!(note.contains("restarting"), "{note}");
    // The page reads the new map once the Skipper is back, and says so.
    ui.set_context(true, true, Some(&sk.view.io_map));
    assert!(!ui.edited(), "the Skipper runs the window's text now");
    assert!(ui.note_text().unwrap().contains("runs this map"));
}

#[test]
fn a_download_the_skipper_refuses_shows_its_reason_and_its_devices() {
    let mut sk = Skipper::new();
    let mut ui = open(&mut sk, true);
    type_into(&mut ui, MAP);
    ui.check();
    pump(&mut ui, &mut sk, Some(TOKEN));
    sk.says(0, Said::Answered { ms: 3 });
    sk.says(1, Said::Answered { ms: 3 });
    sk.finish();
    settle(&mut ui, &mut sk, 1.0);
    assert!(ui.can_download());
    // Meanwhile somebody else's check of the same text found a device
    // gone: the Skipper's record is now a failure the window has not read.
    sk.view.check = Some(Check::begin(&helmsman_nodecore::iomap::parse(MAP).unwrap()));
    sk.says(0, Said::Answered { ms: 3 });
    sk.says(1, Said::Failed("no route to host".into()));
    sk.finish();
    ui.download();
    pump(&mut ui, &mut sk, Some(TOKEN));
    assert_ne!(sk.view.io_map, MAP, "refused: nothing was saved");
    let note = ui.note_text().unwrap();
    assert!(
        note.contains("bench did not answer this node"),
        "the Skipper's own reason: {note}"
    );
    let r = ui.report().unwrap();
    assert_eq!(
        r.devices[1].verdict,
        Verdict::Failed("no route to host".into()),
        "the list comes from the reply"
    );
    assert!(!ui.can_download(), "and the button goes");

    // A pass that is too old: the Skipper's clock says so, and so does the window.
    ui.check();
    pump(&mut ui, &mut sk, Some(TOKEN));
    sk.says(0, Said::Answered { ms: 3 });
    sk.says(1, Said::Answered { ms: 3 });
    sk.finish();
    settle(&mut ui, &mut sk, 1.0);
    assert!(ui.can_download());
    sk.view.uptime_ms += 11 * 60 * 1000;
    ui.download();
    pump(&mut ui, &mut sk, Some(TOKEN));
    assert!(
        ui.note_text().unwrap().contains("check it again"),
        "{:?}",
        ui.note_text()
    );
}

#[test]
fn a_map_the_skipper_cannot_parse_shows_its_line_next_to_the_editor() {
    let mut sk = Skipper::new();
    let mut ui = open(&mut sk, true);
    type_into(
        &mut ui,
        "device rio modbus-tcp 192.168.3.60\nthis is not a map line\n",
    );
    ui.check();
    pump(&mut ui, &mut sk, Some(TOKEN));
    let reason = ui.reason_text().expect("the 422 is shown");
    assert!(reason.starts_with("line 2:"), "{reason}");
    assert!(!ui.waiting(), "nothing runs");
    assert!(!ui.can_download());
    // Typing again takes the old complaint away: it was about the old text.
    type_into(&mut ui, MAP);
    assert!(ui.reason_text().is_none());
    // A 422 to a download nobody asked for is not read.
    ui.answer(NodeOp::Save, 422, "{\"error\":\"line 1: x\"}");
    assert!(ui.reason_text().is_none());
}

#[test]
fn a_skipper_without_the_check_route_is_told_so_and_offers_neither_button() {
    let mut sk = Skipper::plain();
    let mut ui = open(&mut sk, true);
    assert!(ui.no_check(), "the first read of the check answered 404");
    assert!(ui.can_check().unwrap_err().contains("without asking its devices"));
    ui.check();
    assert!(ui.take_requests().is_empty(), "no check is sent");
    assert!(!ui.can_download());
    // Even a window that was handed a pass does not offer Download.
    type_into(&mut ui, MAP);
    assert!(!ui.can_download());
    // The route itself is a 404 on the real router too.
    let post = PReq::json(
        PCall::NodeMap(NodeOp::Check),
        "POST",
        "/api/io_map/check".into(),
        &json!({"text": MAP}),
    );
    assert_eq!(sk.call(&post, Some(TOKEN)).0, 404);

    // A check asked for before the page knew: the same answer, learned late.
    let mut late = NodeMapUi::default();
    late.set_context(true, true, Some("device rio modbus-tcp 192.168.3.60\n"));
    late.open = true;
    late.edit(MAP.into());
    late.check();
    pump(&mut late, &mut sk, Some(TOKEN));
    assert!(late.no_check() && late.report().is_none());
}

#[test]
fn without_a_token_the_buttons_say_what_is_needed_and_nothing_is_sent() {
    let mut sk = Skipper::new();
    let mut ui = open(&mut sk, false);
    type_into(&mut ui, MAP);
    let why = ui.can_check().unwrap_err();
    assert!(why.contains("token") && why.contains("sign-in"), "{why}");
    ui.check();
    ui.download();
    assert!(ui.take_requests().is_empty(), "no call that would answer 401");
    // The Skipper does refuse it without the token, which is why.
    let post = PReq::json(
        PCall::NodeMap(NodeOp::Check),
        "POST",
        "/api/io_map/check".into(),
        &json!({"text": MAP}),
    );
    assert_eq!(sk.call(&post, None).0, 401);
    assert_eq!(sk.call(&post, Some("wrong")).0, 401);
    // The read of the last check needs none, so the list can still be shown.
    assert!(!ui.no_check());
    // A report that says may_save is not enough without the token.
    let mut other = Skipper::new();
    let mut signed = open(&mut other, true);
    type_into(&mut signed, MAP);
    signed.check();
    pump(&mut signed, &mut other, Some(TOKEN));
    other.says(0, Said::Answered { ms: 1 });
    other.says(1, Said::Answered { ms: 1 });
    other.finish();
    settle(&mut signed, &mut other, 1.0);
    assert!(signed.can_download());
    signed.set_context(true, false, Some(&other.view.io_map));
    assert!(!signed.can_download(), "the token went: no Download");

    // A token the Skipper refuses (changed since it was typed) is said, not
    // thrown away silently.
    let mut ui = open(&mut sk, true);
    type_into(&mut ui, MAP);
    ui.check();
    for r in ui.take_requests() {
        let (status, body) = sk.call(&r, Some("old-token"));
        assert_eq!(status, 401);
        ui.answer(NodeOp::Check, status, &body);
    }
    assert!(
        ui.note_text().unwrap().contains("did not take the token"),
        "{:?}",
        ui.note_text()
    );
    assert!(!ui.waiting());
}

#[test]
fn a_check_already_running_is_watched_not_started_again() {
    let mut sk = Skipper::new();
    let mut a = open(&mut sk, true);
    type_into(&mut a, MAP);
    a.check();
    pump(&mut a, &mut sk, Some(TOKEN));
    // Another page asks while it runs.
    let mut b = open(&mut sk, true);
    type_into(&mut b, "device rio modbus-tcp 192.168.3.60\n");
    b.check();
    pump(&mut b, &mut sk, Some(TOKEN));
    assert!(b.note_text().unwrap().contains("already running"));
    assert!(b.waiting(), "it watches the one that runs");
    sk.says(0, Said::Answered { ms: 2 });
    sk.says(1, Said::Answered { ms: 2 });
    sk.finish();
    settle(&mut b, &mut sk, 1.0);
    assert!(!b.waiting());
    assert!(!b.can_download(), "its result is of the other text, not b's");
}

// ---- time ----

#[test]
fn the_check_is_read_once_a_second_one_at_a_time_and_given_up_after_two_minutes() {
    let mut ui = NodeMapUi::default();
    ui.set_context(true, true, Some("a\n"));
    ui.edit(MAP.into());
    ui.check();
    ui.take_requests();
    ui.answer(NodeOp::Check, 202, "{\"checking\":true}");
    ui.tick(100.0);
    assert_eq!(ui.take_requests().len(), 1, "the first read at once");
    ui.tick(101.5);
    assert!(ui.take_requests().is_empty(), "the last read has not come back");
    ui.answer(
        NodeOp::Poll,
        200,
        &json!({"state":"running","text":MAP,"devices":[]}).to_string(),
    );
    ui.tick(100.5);
    assert!(ui.take_requests().is_empty(), "not twice in a second");
    ui.tick(101.0);
    assert_eq!(ui.take_requests().len(), 1);
    // A read that fails is tried again, until the deadline.
    ui.answer(NodeOp::Poll, 0, "no route");
    ui.tick(102.0);
    assert_eq!(ui.take_requests().len(), 1);
    // The result of an earlier text, read while this one's starts, is not shown.
    ui.answer(
        NodeOp::Poll,
        200,
        &json!({"state":"done","text":"old\n","devices":[]}).to_string(),
    );
    assert!(ui.waiting() && ui.report().is_some_and(|r| r.text == MAP));
    ui.tick(100.0 + 121.0);
    assert!(!ui.waiting(), "given up");
    assert!(ui.note_text().unwrap().contains("did not finish"));
    ui.tick(100.0 + 123.0);
    assert!(ui.take_requests().is_empty(), "and no longer asked");
}

#[test]
fn an_unreadable_check_stops_the_waiting_and_is_said() {
    let mut ui = NodeMapUi::default();
    ui.set_context(true, true, Some("a\n"));
    ui.edit(MAP.into());
    ui.check();
    ui.take_requests();
    ui.answer(NodeOp::Check, 202, "{}");
    ui.tick(0.0);
    ui.answer(NodeOp::Poll, 200, "<html>captive portal</html>");
    assert!(!ui.waiting());
    assert!(ui.note_text().unwrap().contains("could not be read"));
    assert!(!ui.can_download());
    // A reply for a call that is not outstanding changes nothing.
    ui.answer(NodeOp::Check, 202, "{}");
    ui.answer(NodeOp::Save, 200, "{}");
    assert!(!ui.waiting());
}

// ---- how the page knows a Skipper ----

#[test]
fn a_skippers_map_reply_marks_the_page_and_a_boxs_does_not() {
    use crate::remote::{BoxState, Fetch, Msg};
    let sk = Skipper::new();
    let mut s = BoxState::new();
    s.apply(
        Msg::Got(
            Fetch::IoMap,
            Ok(helmsman_nodecore::api::io_map(&sk.view).to_string()),
        ),
        1.0,
    );
    assert!(s.io_map_node);
    s.apply(
        Msg::Got(
            Fetch::IoMap,
            Ok(json!({"snapshot": "0f3a", "text": "x\n"}).to_string()),
        ),
        2.0,
    );
    assert!(!s.io_map_node, "a box answers with a snapshot id of its own");
    s.apply(
        Msg::Got(
            Fetch::IoMap,
            Ok(json!({"snapshot": "node", "text": "x\n"}).to_string()),
        ),
        3.0,
    );
    assert!(s.io_map_node);
    s.apply(Msg::Got(Fetch::IoMap, Err("HTTP 404".into())), 4.0);
    assert!(!s.io_map_node && s.io_map.is_none());
    s.apply(
        Msg::Got(
            Fetch::IoMap,
            Ok(json!({"snapshot": "node", "text": "x\n"}).to_string()),
        ),
        5.0,
    );
    // A read that failed leaves what was known.
    s.apply(Msg::Got(Fetch::IoMap, Err("HTTP 500".into())), 6.0);
    assert!(s.io_map_node);
    // A reply that is not a map at all does not make a Skipper.
    let mut other = BoxState::new();
    other.apply(Msg::Got(Fetch::IoMap, Ok("{\"snapshot\":\"node\"}".into())), 1.0);
    assert!(!other.io_map_node && other.io_map.is_none());
}

#[test]
fn the_window_follows_the_skipper_when_nothing_is_edited() {
    let mut ui = NodeMapUi::default();
    ui.set_context(true, true, Some("a\n"));
    assert_eq!(ui.text(), "a\n");
    ui.set_context(true, true, Some("b\n"));
    assert_eq!(ui.text(), "b\n");
    ui.edit("c\n".into());
    ui.set_context(true, true, Some("d\n"));
    assert_eq!(ui.text(), "c\n", "an edit is not overwritten by a new read");
    ui.discard();
    assert_eq!(ui.text(), "d\n");
    // Typing the Skipper's own text back is no edit.
    ui.edit("x\n".into());
    ui.edit("d\n".into());
    assert!(!ui.edited());
    // Leaving the Skipper closes the window.
    ui.open = true;
    ui.set_context(false, true, Some("d\n"));
    assert!(!ui.open);
}

/// Keeps `DeviceCheck` in the public surface the tests above read.
#[test]
fn a_device_row_is_its_name_its_address_and_its_words() {
    let d = DeviceCheck {
        name: "rio".into(),
        address: "192.168.3.60:502".into(),
        verdict: Verdict::Failed(String::new()),
    };
    assert_eq!(d.verdict.words(), "did not answer");
}
