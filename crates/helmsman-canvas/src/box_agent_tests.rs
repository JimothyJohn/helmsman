//! The Agent tab against the real box: `POST /api/chat` on a box API
//! ([`crate::box_tests::Bench`] with a chat route) whose relay is a
//! scripted one on a real socket replaying helmsman-plc's relay fixtures.
//! The bytes the box streams back are fed to [`BoxAgent`] in pieces of
//! every size, the way `fetch` hands them over.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helmsman_agent::http::{Timeouts, Url};
use helmsman_agent::{Retry, relay};
use helmsman_plc::api::chat;
use proptest::prelude::*;
use serde_json::{Value as Json, json};

use crate::box_agent::{
    BoxAgent, End, Incoming, Item, MAX_MESSAGE, MAX_TEXT, MAX_TURNS, STATUS_RETRY_S, Status, cut,
};
use crate::box_tests::{Bench, DEMO};

const RELAY_TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const API_TOKEN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../helmsman-plc/tests/fixtures/agent/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Answers `GET /v1/health` with 200 and each other request with the
/// next scripted event stream (400 when the script is out).
struct Relay {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    fn start(script: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut script = script.into_iter();
                while !stop.load(Ordering::Acquire) {
                    let Ok((mut s, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    s.set_nonblocking(false).unwrap();
                    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let Some(head) = read_request(&mut s) else {
                        continue;
                    };
                    if head.starts_with("GET /v1/health ") {
                        let _ = s.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                        );
                        continue;
                    }
                    if let Some(body) = script.next() {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-request-id: r\r\nConnection: close\r\n\r\n{body}"
                        );
                    } else {
                        let b = json!({"type":"error","error":{"type":"invalid_request","message":"script exhausted","retryable":false}}).to_string();
                        let _ = write!(
                            s,
                            "HTTP/1.1 400 X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
                            b.len()
                        );
                    }
                }
            })
        };
        Self {
            addr,
            stop,
            thread: Some(thread),
        }
    }

    fn setup(&self) -> chat::Setup {
        let url = Url::parse(&format!("http://{}", self.addr)).unwrap();
        let mut rc = relay::Config::new(url, RELAY_TOKEN.into(), "v1".into());
        rc.timeouts = Timeouts {
            connect: Duration::from_secs(2),
            idle: Duration::from_secs(5),
            total: Duration::from_secs(20),
        };
        let mut s = chat::Setup::new(rc);
        s.probe_every = Duration::from_millis(50);
        s.retry = Retry {
            attempts: 0,
            first: Duration::from_millis(10),
            cap: Duration::from_millis(40),
        };
        s
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The request head; its body (by Content-Length) read and dropped.
fn read_request(s: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if s.read(&mut byte).ok()? == 0 || buf.len() > 64 << 10 {
            return None;
        }
        buf.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let len = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).ok()?;
    Some(head)
}

/// Carries `req` out on the box with the API token (the page uses its
/// session cookie; the route takes either): the status and the raw body.
fn post(addr: SocketAddr, url: &str, body: &str, token: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    write!(
        s,
        "POST {url} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, rest) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    // Chunked when it streams: undo the framing.
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        unchunk(rest)
    } else {
        rest.to_string()
    };
    (status, body)
}

fn unchunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        s = &rest[n + 2..];
    }
    out
}

fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    // The box token stands for the signed-in page's cookie (card read-lock).
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAuthorization: Bearer {}\r\n\r\n",
        crate::box_tests::TOKEN
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.to_string())
}

/// Sends `message` to the box (continuing the conversation, if any) and
/// feeds the answer back in `piece`-byte chunks (whole characters), as
/// `web.rs` would.
fn turn(a: &mut BoxAgent, b: &Bench, message: &str, piece: usize, token: Option<&str>) {
    let req = a.send(message, false).expect("the message is sendable");
    assert_eq!(req.url, "/api/chat");
    let (status, body) = post(b.addr, &req.url, &req.body, token);
    let seq = req.seq;
    if status != 200 {
        a.apply(Incoming::Status { seq, status, body });
        return;
    }
    let chars: Vec<char> = body.chars().collect();
    for c in chars.chunks(piece.max(1)) {
        a.apply(Incoming::Chunk {
            seq,
            text: c.iter().collect(),
        });
    }
    a.apply(Incoming::End { seq });
}

fn status(a: &mut BoxAgent, b: &Bench) {
    let (s, body) = get(b.addr, "/api/chat/status");
    a.status_answer(s, &body, 0.0);
}

/// A status read the box refuses is asked again on a cadence, not on every
/// frame (the dot stays grey, the tab keeps drawing, and each answer
/// repaints).
#[test]
fn a_failed_status_read_is_asked_again_on_the_cadence_not_every_frame() {
    let mut a = BoxAgent::new();
    assert!(a.status_due(10.0), "asked once");
    assert!(!a.status_due(10.0), "not twice while on its way");
    a.status_answer(500, "", 10.1);
    for frame in 0..600 {
        assert!(!a.status_due(10.1 + f64::from(frame) / 200.0), "frame {frame}");
    }
    assert!(a.status_due(10.1 + STATUS_RETRY_S), "after the cadence");
    a.status_answer(200, r#"{"state":"online"}"#, 16.0);
    assert!(!a.status_due(100.0), "known: never asked again");
}

#[test]
fn a_reply_cut_to_a_size_never_splits_a_character() {
    let mut s = "aé€😀".repeat(10);
    for max in 0..s.len() + 3 {
        let mut t = s.clone();
        cut(&mut t, max);
        assert!(t.len() <= max && s.starts_with(&t), "{max}");
        assert!(
            max - t.len() < 4 || t.len() == s.len(),
            "cut no more than a character: {max}"
        );
    }
    cut(&mut s, 1 << 20);
    assert_eq!(s.chars().count(), 40);
}

#[test]
fn a_text_turn_from_the_box_reads_as_one_answer() {
    let relay = Relay::start(vec![fixture("text-only.sse")]);
    let b = Bench::with_chat(&[("cell", DEMO)], Some(relay.setup()));
    let mut a = BoxAgent::new();
    status(&mut a, &b);
    // Not asked yet, or already probed by the box's watcher.
    assert!(
        matches!(a.status, Status::Unknown | Status::Online),
        "{:?}",
        a.status
    );
    turn(&mut a, &b, "hello", 7, Some(API_TOKEN));
    let t = &a.turns[0];
    assert_eq!(t.user, "hello");
    assert_eq!(t.text, "Hello! I can read the box's signals and modules.");
    assert_eq!(
        t.end,
        Some(End::Done {
            stop: "end_turn".into(),
            why: None
        })
    );
    assert!(a.session.is_some(), "the box named the conversation");
    assert!(!a.busy());
    status(&mut a, &b);
    assert_eq!(a.status, Status::Online);
}

#[test]
fn a_tool_round_shows_the_tool_and_its_result_then_the_answer() {
    let relay = Relay::start(vec![fixture("list-signals.sse"), fixture("text-only.sse")]);
    let b = Bench::with_chat(&[("cell", DEMO)], Some(relay.setup()));
    let mut a = BoxAgent::new();
    turn(&mut a, &b, "what signals are there?", 1, Some(API_TOKEN));
    let t = &a.turns[0];
    assert!(t.text.starts_with("Let me look at the signals."), "{:?}", t.text);
    assert!(
        t.text
            .ends_with("Hello! I can read the box's signals and modules."),
        "{:?}",
        t.text
    );
    let tools: Vec<_> = t
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Tool { name, result, .. } => Some((name.as_str(), result.as_ref().map(|r| r.0))),
            _ => None,
        })
        .collect();
    assert_eq!(tools, [("list_signals", Some(true))], "{:?}", t.items);
    assert!(
        matches!(&t.end, Some(End::Done { stop, .. }) if stop == "end_turn"),
        "{:?}",
        t.end
    );
}

#[test]
fn a_retry_continues_the_conversation_and_a_new_instruction_starts_one() {
    let relay = Relay::start(vec![fixture("text-only.sse"), fixture("text-only.sse")]);
    let b = Bench::with_chat(&[("cell", DEMO)], Some(relay.setup()));
    let mut a = BoxAgent::new();
    turn(&mut a, &b, "one", 64, Some(API_TOKEN));
    let first = a.session.clone().unwrap();
    let req = a.send("two", false).unwrap();
    let body: Json = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body, json!({ "message": "two", "session": first }));
    a.stop();
    // Fresh forgets the session; the box starts another.
    let req = a.send("three", true).unwrap();
    let body: Json = serde_json::from_str(&req.body).unwrap();
    assert_eq!(body, json!({ "message": "three", "session": null }));
    a.stop();
    turn(&mut a, &b, "four", 64, Some(API_TOKEN));
    assert!(a.session.is_some());
}

#[test]
fn signed_out_is_a_401_that_says_to_sign_in_and_runs_nothing() {
    let relay = Relay::start(vec![]);
    let b = Bench::with_chat(&[("cell", DEMO)], Some(relay.setup()));
    let mut a = BoxAgent::new();
    for token in [None, Some("f".repeat(64).as_str())] {
        turn(&mut a, &b, "hello", 64, token);
        let end = a.turns.last().unwrap().end.clone();
        assert!(
            matches!(&end, Some(End::Failed(w)) if w.starts_with("Sign in")),
            "{end:?}"
        );
        assert!(a.session.is_none());
    }
}

#[test]
fn a_box_without_a_relay_says_so_and_the_tab_stops_sending() {
    let b = Bench::with_chat(&[("cell", DEMO)], None);
    let mut a = BoxAgent::new();
    status(&mut a, &b);
    assert!(matches!(a.status, Status::Unconfigured(_)), "{:?}", a.status);
    assert!(a.send("hello", true).is_err());
    let why = a.refusal("hello").unwrap();
    assert!(why.contains("no relay"), "{why}");
    // Even when the page had not asked first: the 503 teaches it.
    let mut a = BoxAgent::new();
    turn(&mut a, &b, "hello", 64, Some(API_TOKEN));
    assert!(matches!(a.status, Status::Unconfigured(_)), "{:?}", a.status);
    assert!(matches!(&a.turns[0].end, Some(End::Failed(w)) if w.contains("no relay")));
}

#[test]
fn a_relay_that_goes_away_ends_the_turn_in_words_and_marks_the_agent_offline() {
    // A relay address nothing listens on: bound, then released.
    let dead = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let url = Url::parse(&format!("http://{dead}")).unwrap();
    let mut rc = relay::Config::new(url, RELAY_TOKEN.into(), "v1".into());
    rc.timeouts = Timeouts {
        connect: Duration::from_millis(500),
        idle: Duration::from_secs(2),
        total: Duration::from_secs(5),
    };
    let mut setup = chat::Setup::new(rc);
    setup.retry = Retry {
        attempts: 0,
        first: Duration::from_millis(10),
        cap: Duration::from_millis(40),
    };
    let b = Bench::with_chat(&[("cell", DEMO)], Some(setup));
    let mut a = BoxAgent::new();
    // The relay is gone when the message arrives, or the box's own probe
    // found that first and reads it by rule: the turn ends either way.
    turn(&mut a, &b, "hello", 5, Some(API_TOKEN));
    assert!(
        matches!(&a.turns[0].end, Some(End::Failed(_) | End::Done { .. })),
        "{:?}",
        a.turns[0].end
    );
    assert!(matches!(a.status, Status::Offline(_)), "{:?}", a.status);
    // Offline, the next message is read by rule: an answer, no model.
    turn(&mut a, &b, "what can you do?", 5, Some(API_TOKEN));
    let t = &a.turns[1];
    assert!(
        matches!(&t.end, Some(End::Done { stop, .. }) if stop == "offline"),
        "{:?}",
        t.end
    );
    assert!(
        t.items
            .iter()
            .any(|i| matches!(i, Item::Fallback { proposed: None, .. })),
        "{:?}",
        t.items
    );
    assert_ne!(t.text, "");
}

// ------------------------------------------------- the stream, by hand

fn ev(name: &str, data: &Json) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

fn started() -> (BoxAgent, u64) {
    let mut a = BoxAgent::new();
    a.status = Status::Online;
    let seq = a.send("go", true).unwrap().seq;
    (a, seq)
}

#[test]
fn proposals_are_listed_with_their_ids_for_the_review() {
    let (mut a, seq) = started();
    let body = [
        ev("session", &json!({"id": "s1"})),
        ev("text", &json!({"delta": "Raising the speed."})),
        ev(
            "tool_call",
            &json!({"id": "p1", "name": "propose_change", "input": {}}),
        ),
        ev(
            "tool_result",
            &json!({"id": "p1", "name": "propose_change", "ok": true, "summary": "stored as proposal 3"}),
        ),
        ev("proposal", &json!({"ids": [3], "summary": "speed to 2"})),
        ev(
            "done",
            &json!({"stop": "end_turn", "why": null, "steps": 2, "output_tokens": 9, "proposals": [3]}),
        ),
    ]
    .concat();
    a.apply(Incoming::Chunk { seq, text: body });
    let t = &a.turns[0];
    assert_eq!(t.proposals(), [3]);
    assert!(t.items.contains(&Item::Proposal {
        ids: vec![3],
        summary: "speed to 2".into()
    }));
    assert!(!a.busy());
}

/// Regression, 2026-11-05: a turn the relay dropped ends with `error`
/// and then `status` (offline); the tab stopped reading at `error`, so
/// the offline line was lost whenever both came in one piece.
#[test]
fn the_status_after_a_turns_error_still_lands() {
    let error = ev(
        "error",
        &json!({"kind": "relay_unreachable", "message": "the relay did not answer", "retryable": true}),
    );
    let status = ev(
        "status",
        &json!({"state": "offline", "message": "relay unreachable"}),
    );
    for pieces in [vec![error.clone() + &status], vec![error.clone(), status.clone()]] {
        let (mut a, seq) = started();
        for text in pieces {
            a.apply(Incoming::Chunk { seq, text });
        }
        assert!(matches!(&a.turns[0].end, Some(End::Failed(w)) if w == "the relay did not answer"));
        assert_eq!(a.status, Status::Offline("relay unreachable".into()));
        assert!(!a.busy(), "the turn is over");
        a.apply(Incoming::End { seq });
        assert!(!a.busy());
    }
}

#[test]
fn a_stream_cut_short_ends_the_turn_and_late_pieces_are_ignored() {
    let (mut a, seq) = started();
    a.apply(Incoming::Chunk {
        seq,
        text: ev("text", &json!({"delta": "Half"})),
    });
    a.apply(Incoming::End { seq });
    assert!(matches!(&a.turns[0].end, Some(End::Failed(w)) if w.contains("closed")));
    a.apply(Incoming::Chunk {
        seq,
        text: ev("text", &json!({"delta": " more"})),
    });
    assert_eq!(a.turns[0].text, "Half");
}

#[test]
fn stop_drops_whatever_the_stopped_turn_sends_later() {
    let (mut a, seq) = started();
    a.stop();
    assert_eq!(a.turns[0].end, Some(End::Stopped));
    a.apply(Incoming::Chunk {
        seq,
        text: ev("text", &json!({"delta": "late"})) + &ev("done", &json!({"stop": "end_turn"})),
    });
    assert_eq!(a.turns[0].text, "");
    assert_eq!(a.turns[0].end, Some(End::Stopped));
}

#[test]
fn messages_are_held_to_what_the_box_takes() {
    let mut a = BoxAgent::new();
    a.status = Status::Online;
    assert!(a.send("   ", true).is_err());
    let long = "é".repeat(MAX_MESSAGE + 1);
    let why = a.send(&long, true).unwrap_err();
    assert!(why.contains(&format!("at most {MAX_MESSAGE}")), "{why}");
    assert!(a.send(&"é".repeat(MAX_MESSAGE), true).is_ok());
    // One turn at a time.
    assert!(a.send("again", true).is_err());
}

#[test]
fn a_runaway_answer_and_a_long_conversation_stay_bounded() {
    let (mut a, seq) = started();
    let delta = "x".repeat(64 << 10);
    for _ in 0..8 {
        a.apply(Incoming::Chunk {
            seq,
            text: ev("text", &json!({ "delta": delta })),
        });
    }
    assert!(a.turns[0].text.len() <= MAX_TEXT + 64);
    assert!(a.turns[0].text.ends_with("cut]"));
    let mut a = BoxAgent::new();
    a.status = Status::Online;
    for k in 0..MAX_TURNS + 5 {
        a.send(&format!("m{k}"), true).unwrap();
        a.stop();
    }
    assert_eq!(a.turns.len(), MAX_TURNS);
    assert_eq!(a.turns[0].user, "m5");
}

#[test]
fn the_boxs_words_reach_the_screen_without_control_characters() {
    let (mut a, seq) = started();
    a.apply(Incoming::Chunk {
        seq,
        text: ev(
            "error",
            &json!({"kind": "x", "message": "bad\u{1b}[31m relay\u{7}", "retryable": false}),
        ),
    });
    assert_eq!(a.turns[0].end, Some(End::Failed("bad[31m relay".into())));
    let (mut a, seq) = started();
    a.apply(Incoming::Status {
        seq,
        status: 409,
        body: json!({"error": "this conversation is in use\u{0}"}).to_string(),
    });
    assert_eq!(
        a.turns[0].end,
        Some(End::Failed("this conversation is in use".into()))
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    /// Whatever the stream holds, in whatever pieces, the agent never
    /// panics, stays bounded, and ends the turn at the stream's end.
    #[test]
    fn any_stream_in_any_pieces_ends_the_turn(
        raw in "(event: (text|tool_call|tool_result|proposal|done|error|status|fallback|session|junk)\ndata: [ -~]{0,40}\n\n|[ -~\n\r]{0,30}){0,30}",
        cuts in proptest::collection::vec(1usize..50, 1..20),
    ) {
        let (mut a, seq) = started();
        let chars: Vec<char> = raw.chars().collect();
        let mut at = 0;
        for c in cuts.iter().cycle() {
            if at >= chars.len() { break; }
            let end = (at + c).min(chars.len());
            a.apply(Incoming::Chunk { seq, text: chars[at..end].iter().collect() });
            at = end;
        }
        a.apply(Incoming::End { seq });
        prop_assert!(!a.busy());
        prop_assert!(a.turns[0].end.is_some());
        prop_assert!(a.turns[0].items.len() <= crate::box_agent::MAX_ITEMS);
    }
}

/// Nick, 2026-11-08: the Agent tab shows the state as a dot, green online,
/// red offline or without a relay, grey until heard.
#[test]
fn the_status_dot_is_green_online_red_offline_grey_unknown() {
    use crate::st_view::brand::{GREEN, RED, STEEL};
    assert_eq!(Status::Online.dot(), GREEN);
    assert_eq!(Status::Offline("down".into()).dot(), RED);
    assert_eq!(Status::Unconfigured(String::new()).dot(), RED);
    assert_eq!(Status::Unread.dot(), STEEL);
    assert_eq!(Status::Unknown.dot(), STEEL);
}
