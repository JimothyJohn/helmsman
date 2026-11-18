//! The relay protocol v2 client (`docs/relay-protocol-v2.md`) over real
//! sockets: a loopback relay answers each request from a script with what
//! the protocol document shows (the model's own events are the recorded
//! stream in `tests/fixtures/relay/text-basic.sse`), and records what the
//! box sent. No real relay has answered this client in these tests; the
//! shapes are the protocol document's.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helmsman_agent::http::{Timeouts, Url};
use helmsman_agent::relay::{self, Config, Error};
use helmsman_agent::session::{self, Event, Input, MAX_CALLS};
use serde_json::{Value as Json, json};

const TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const SID: &str = "s00112233445566778899aabbccddeeff";
const DOC: &str = "d0011223344556677";

/// What the relay answers: status, content type, extra headers, body.
struct Answer {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

fn json_answer(status: u16, j: &Json) -> Answer {
    Answer {
        status,
        content_type: "application/json",
        body: j.to_string().into_bytes(),
    }
}

fn stream(body: &str) -> Answer {
    Answer {
        status: 200,
        content_type: "text/event-stream",
        body: body.as_bytes().to_vec(),
    }
}

fn error(status: u16, kind: &str, retryable: bool) -> Answer {
    json_answer(
        status,
        &json!({"type":"error","error":{"type":kind,"message":format!("{kind} for the test"),"retryable":retryable},"request_id":"r"}),
    )
}

/// What the relay saw.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    headers: String,
    body: Json,
}

/// Answers one request per answer, in order, then stops listening.
fn relay_of(answers: Vec<Answer>) -> (Config, JoinHandle<Vec<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut seen = Vec::new();
        for a in answers {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut headers = String::new();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                headers.push_str(&h);
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let head = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                a.status,
                a.content_type,
                a.body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&a.body);
            seen.push(Seen {
                line,
                headers,
                body: serde_json::from_slice(&body).unwrap_or(Json::Null),
            });
        }
        seen
    });
    (config(&format!("http://{addr}"), "helmsman/v8"), server)
}

fn config(url: &str, version: &str) -> Config {
    let mut cfg = Config::new(Url::parse(url).unwrap(), TOKEN.into(), version.into());
    cfg.timeouts = Timeouts {
        connect: Duration::from_secs(2),
        idle: Duration::from_secs(5),
        total: Duration::from_secs(20),
    };
    cfg
}

/// A port nothing listens on: the client must refuse before connecting,
/// or the error would be `Unreachable`.
fn nowhere() -> Config {
    // Port 1: never bound or reused by another test's server.
    config("http://127.0.0.1:1", "helmsman/v8")
}

fn ev(name: &str, data: &Json) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// The protocol document's order: session, a call, the model's recorded
/// stream, usage, then the yield.
fn step_stream(tail: &str) -> String {
    let model = std::fs::read_to_string(format!(
        "{}/tests/fixtures/relay/text-basic.sse",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut s = ev(
        "harness_session",
        &json!({"type":"harness_session","session_id":SID,"seq":1}),
    );
    s += &ev(
        "harness_call",
        &json!({"type":"harness_call","n":1,"purpose":"turn"}),
    );
    s += &model;
    s += &ev(
        "relay_usage",
        &json!({"type":"relay_usage","request_id":"r","agent_version":"helmsman/v8","model":"m","input_tokens":25,"output_tokens":15,"purpose":"turn"}),
    );
    s + tail
}

fn yielded(reason: &str, tool_uses: &Json) -> String {
    ev(
        "harness_yield",
        &json!({"type":"harness_yield","reason":reason,"seq":3,"tool_uses":tool_uses,"context_tokens":100,"model_calls":1}),
    )
}

fn run_step(a: Answer, input: &Input) -> (Result<session::StepEnd, Error>, Vec<String>, Vec<Seen>) {
    let (cfg, server) = relay_of(vec![a]);
    let mut got = Vec::new();
    let r = session::step(&cfg, SID, input, &mut |e| match e {
        Event::Text(t) => got.push(format!("text:{t}")),
        Event::RelayTool { id, name, is_error } => got.push(format!("tool:{id}:{name}:{is_error}")),
    });
    (r, got, server.join().unwrap())
}

fn user() -> Input {
    Input::User(vec![json!({"type":"text","text":"hello"})])
}

// -------------------------------------------------------------- lookup

#[test]
fn the_relay_says_which_protocol_a_profile_speaks() {
    for (body, want) in [
        (json!({"version":"helmsman/v8","protocol":2}), 2),
        (json!({"version":"helmsman/v8","protocol":1}), 1),
        // A relay from before v2 says nothing: that is protocol 1.
        (json!({"version":"helmsman/v8"}), 1),
    ] {
        let (cfg, server) = relay_of(vec![json_answer(200, &body)]);
        let p = relay::lookup(&cfg).unwrap();
        assert_eq!((p.protocol, p.id.as_str()), (want, "helmsman/v8"), "{body}");
        let seen = server.join().unwrap();
        assert_eq!(seen[0].line, "GET /v1/agents/helmsman%2Fv8 HTTP/1.1\r\n");
        assert!(
            seen[0]
                .headers
                .contains(&format!("Authorization: Bearer {TOKEN}\r\n"))
        );
    }
}

#[test]
fn a_bare_name_the_relay_does_not_know_is_asked_again_in_the_boxs_namespace() {
    let (mut cfg, server) = relay_of(vec![
        error(404, "unknown_agent_version", false),
        json_answer(200, &json!({"version":"helmsman/v8","protocol":2})),
    ]);
    cfg.version = "v8".into();
    let p = relay::lookup(&cfg).unwrap();
    assert_eq!((p.protocol, p.id.as_str()), (2, "helmsman/v8"));
    let seen = server.join().unwrap();
    assert_eq!(seen[0].line, "GET /v1/agents/v8 HTTP/1.1\r\n");
    assert_eq!(seen[1].line, "GET /v1/agents/helmsman%2Fv8 HTTP/1.1\r\n");
}

#[test]
fn an_agent_the_relay_does_not_serve_stays_unknown_and_a_strange_protocol_is_refused() {
    let (mut cfg, server) = relay_of(vec![
        error(404, "unknown_agent_version", false),
        error(404, "unknown_agent_version", false),
    ]);
    cfg.version = "v99".into();
    assert!(
        matches!(relay::lookup(&cfg), Err(Error::Status { status: 404, ref kind, .. }) if kind == "unknown_agent_version")
    );
    server.join().unwrap();
    for bad in [
        json!({"protocol":3}),
        json!({"protocol":"2"}),
        json!({"protocol":-1}),
    ] {
        let (cfg, server) = relay_of(vec![json_answer(200, &bad)]);
        assert!(matches!(relay::lookup(&cfg), Err(Error::Malformed(_))), "{bad}");
        server.join().unwrap();
    }
}

// ----------------------------------------------------------- the session

#[test]
fn a_session_is_opened_for_the_agent_and_its_id_checked() {
    let (cfg, server) = relay_of(vec![json_answer(
        201,
        &json!({"session_id":SID,"agent":"helmsman/v8","protocol":2,"harness":{"regime":"eocc"},"expires_ms":1}),
    )]);
    assert_eq!(session::open(&cfg).unwrap(), SID);
    let seen = server.join().unwrap();
    assert_eq!(seen[0].line, "POST /v2/sessions HTTP/1.1\r\n");
    assert_eq!(seen[0].body, json!({"agent":"helmsman/v8"}));

    // An id that is not `s` and 32 hex digits never becomes a path.
    for id in [
        "",
        "s1",
        "../../v1/health",
        &format!("{SID}/steps"),
        &SID.to_uppercase(),
        "s00112233445566778899aabbccddeeff\n",
    ] {
        let (cfg, server) = relay_of(vec![json_answer(201, &json!({"session_id":id}))]);
        assert!(matches!(session::open(&cfg), Err(Error::Malformed(_))), "{id:?}");
        server.join().unwrap();
    }

    // A v1 profile on /v2 is the relay's 400.
    let (cfg, server) = relay_of(vec![error(400, "invalid_request", false)]);
    assert!(matches!(
        session::open(&cfg),
        Err(Error::Status { status: 400, .. })
    ));
    server.join().unwrap();
}

#[test]
fn a_step_sends_only_the_new_input_and_streams_the_text() {
    let tail = ev(
        "harness_tool",
        &json!({"type":"harness_tool","tool_use_id":"toolu_1","name":"doc_grep","is_error":false,"ran":true}),
    ) + &ev(
        "harness_compaction",
        &json!({"type":"harness_compaction","through":4,"context_tokens":41000,"recap":"..."}),
    ) + &ev(
        "harness_future",
        &json!({"type":"harness_future","anything":[1,2]}),
    ) + &yielded(
        "tool_use",
        &json!([{"id":"toolu_2","name":"read_io_map","input":{}},{"id":"toolu_3","name":"list_signals","input":{"prefix":"a"}}]),
    );
    let (r, got, seen) = run_step(stream(&step_stream(&tail)), &user());
    let end = r.unwrap();
    assert_eq!(got, ["text:Hello", "text:!", "tool:toolu_1:doc_grep:false"]);
    assert_eq!(end.output_tokens, 15);
    assert_eq!(end.yielded.reason, "tool_use");
    let calls: Vec<_> = end
        .yielded
        .calls
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();
    assert_eq!(calls, [("toolu_2", "read_io_map"), ("toolu_3", "list_signals")]);
    assert_eq!(end.yielded.calls[1].input, Ok(json!({"prefix":"a"})));
    assert_eq!(
        seen[0].line,
        format!("POST /v2/sessions/{SID}/steps HTTP/1.1\r\n")
    );
    assert_eq!(
        seen[0].body,
        json!({"input":{"type":"user","content":[{"type":"text","text":"hello"}]}})
    );
    assert!(seen[0].headers.contains("Accept: text/event-stream\r\n"));

    // The other two inputs, byte for byte as the protocol shows them.
    let results = Input::ToolResults(vec![
        json!({"tool_use_id":"toolu_2","content":"x","is_error":false}),
    ]);
    let (r, _, seen) = run_step(stream(&step_stream(&yielded("end_turn", &json!([])))), &results);
    assert_eq!(r.unwrap().yielded.reason, "end_turn");
    assert_eq!(
        seen[0].body,
        json!({"input":{"type":"tool_results","results":[{"tool_use_id":"toolu_2","content":"x","is_error":false}]}})
    );
    let (_, _, seen) = run_step(
        stream(&step_stream(&yielded("end_turn", &json!([])))),
        &Input::Continue,
    );
    assert_eq!(seen[0].body, json!({"input":{"type":"continue"}}));
}

#[test]
fn every_finishing_reason_and_continue_are_yields_and_nothing_else_is() {
    for reason in ["end_turn", "max_tokens", "refusal", "stop_sequence", "continue"] {
        // Calls named on a yield that does not ask for tools are not taken.
        let (r, _, _) = run_step(
            stream(&step_stream(&yielded(
                reason,
                &json!([{"id":"t","name":"read_io_map","input":{}}]),
            ))),
            &user(),
        );
        let end = r.unwrap();
        assert_eq!(end.yielded.reason, reason);
        assert!(end.yielded.calls.is_empty(), "{reason}");
    }
    for reason in ["", "pause_turn", "TOOL_USE", "tool_use "] {
        let (r, _, _) = run_step(stream(&step_stream(&yielded(reason, &json!([])))), &user());
        assert!(matches!(r, Err(Error::Malformed(_))), "{reason:?}: {r:?}");
    }
}

#[test]
fn documents_the_relay_stored_come_back_with_the_step_and_are_bounded() {
    let doc = json!({"id":DOC,"name":"E1212 manual.pdf","kind":"pdf","bytes":10});
    let mut tail = String::new();
    for _ in 0..40 {
        tail += &ev("harness_document", &json!({"type":"harness_document","doc":doc}));
    }
    tail += &ev(
        "harness_document",
        &json!({"type":"harness_document","doc":"not an object"}),
    );
    tail += &yielded("end_turn", &json!([]));
    let (r, _, _) = run_step(stream(&step_stream(&tail)), &user());
    let end = r.unwrap();
    assert_eq!(end.documents.len(), session::MAX_DOCS);
    assert_eq!(end.documents[0], doc);
}

#[test]
fn a_yield_is_bounded_and_its_tool_calls_are_checked() {
    let many: Vec<Json> = (0..=MAX_CALLS)
        .map(|k| json!({"id":format!("t{k}"),"name":"read_io_map","input":{}}))
        .collect();
    let long = "x".repeat(300);
    for calls in [
        json!(many),
        json!([]),
        json!([{"name":"read_io_map","input":{}}]),
        json!([{"id":"t","input":{}}]),
        json!([{"id":"","name":"read_io_map","input":{}}]),
        json!([{"id":long,"name":"read_io_map","input":{}}]),
        json!("read_io_map"),
    ] {
        let (r, _, _) = run_step(stream(&step_stream(&yielded("tool_use", &calls))), &user());
        assert!(matches!(r, Err(Error::Malformed(_))), "{calls}: {r:?}");
    }
    // An unknown tool is still a named call: the loop answers it unrun.
    // An input that is not an object is kept as the text received.
    let (r, _, _) = run_step(
        stream(&step_stream(&yielded(
            "tool_use",
            &json!([{"id":"t","name":"rm_rf","input":"{\"x\":"},{"id":"u","name":"read_io_map","input":[1]}]),
        ))),
        &user(),
    );
    let calls = r.unwrap().yielded.calls;
    assert_eq!(calls[0].name, "rm_rf");
    assert!(calls[0].input.is_err() && calls[1].input.is_err());
}

#[test]
fn a_stream_that_breaks_gives_an_error_and_no_yield() {
    // Truncated: no yield.
    let (r, _, _) = run_step(stream(&step_stream("")), &user());
    assert_eq!(r.unwrap_err(), Error::CutOff);
    // Cut in the middle of an event.
    let (r, _, _) = run_step(
        stream("event: harness_yield\ndata: {\"type\":\"harness_yie"),
        &user(),
    );
    assert_eq!(r.unwrap_err(), Error::CutOff);
    // relay_error instead of a yield.
    let tail = ev(
        "relay_error",
        &json!({"type":"error","error":{"type":"upstream_unavailable","message":"overloaded","retryable":true},"request_id":"r"}),
    );
    let (r, _, _) = run_step(stream(&step_stream(&tail)), &user());
    let Err(Error::Stream(e)) = r else { panic!("{r:?}") };
    assert_eq!((e.kind.as_str(), e.retryable), ("upstream_unavailable", true));
    // Data that is not JSON, and an event that ends a v1 turn's message
    // before it started.
    let (r, _, _) = run_step(stream("event: harness_call\ndata: nope\n\n"), &user());
    assert!(matches!(r, Err(Error::Malformed(_))));
    let (r, _, _) = run_step(
        stream(&ev(
            "content_block_delta",
            &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}),
        )),
        &user(),
    );
    assert!(matches!(r, Err(Error::Malformed(_))));
    // Not an event stream at all.
    let (r, _, _) = run_step(json_answer(200, &json!({"ok":true})), &user());
    assert!(matches!(r, Err(Error::Malformed(_))));
}

#[test]
fn the_relays_refusals_before_the_stream_keep_their_kind_and_retry_flag() {
    for (status, kind, retryable) in [
        (404, "not_found", false),
        (409, "session_busy", true),
        (409, "conflict", false),
        (400, "invalid_request", false),
        (503, "upstream_unavailable", true),
    ] {
        let (r, _, _) = run_step(error(status, kind, retryable), &user());
        let Err(e) = r else { panic!("{kind}") };
        assert_eq!(e.retryable(), retryable, "{kind}");
        assert!(
            matches!(&e, Error::Status { status: s, kind: k, .. } if *s == status && k == kind),
            "{e:?}"
        );
    }
}

#[test]
fn the_phase_names_the_waiting_calls() {
    let (cfg, server) = relay_of(vec![json_answer(
        200,
        &json!({"id":SID,"phase":{"state":"awaiting_tools","tool_uses":[{"id":"t1","name":"read_io_map","input":{}},{"id":"t2","name":"doc_grep","input":{"doc":DOC}}]}}),
    )]);
    let p = session::phase(&cfg, SID).unwrap();
    assert_eq!(p.state, "awaiting_tools");
    assert_eq!(p.calls.len(), 2);
    assert_eq!(
        server.join().unwrap()[0].line,
        format!("GET /v2/sessions/{SID} HTTP/1.1\r\n")
    );
}

#[test]
fn an_id_with_path_characters_never_reaches_a_url() {
    let cfg = nowhere();
    for id in [
        "../../v1/agents",
        "s00112233445566778899aabbccddeeff/../../x",
        "s00112233445566778899aabbccddeeff?x=1",
        "s0011223344556677889",
        "",
        "s%2e%2e",
    ] {
        assert!(
            matches!(session::phase(&cfg, id), Err(Error::Malformed(_))),
            "{id:?}"
        );
        assert!(
            matches!(
                session::step(&cfg, id, &Input::Continue, &mut |_| {}),
                Err(Error::Malformed(_))
            ),
            "{id:?}"
        );
    }
    for id in [
        "../x",
        "d00112233445566/../../v1",
        "d0011223344556",
        "d00112233445566778",
        "D00112233445566",
        "d0011223344556g",
        "d00112233445566?raw",
        "d00112233445566%2f",
        "",
    ] {
        assert!(!session::doc_id_ok(id), "{id:?}");
        assert!(
            matches!(session::raw_document(&cfg, id), Err(Error::Malformed(_))),
            "{id:?}"
        );
    }
    assert!(session::doc_id_ok(DOC) && session::session_id_ok(SID));
}

#[test]
fn an_unreachable_relay_is_unreachable_for_every_v2_call() {
    let cfg = nowhere();
    assert!(matches!(session::open(&cfg), Err(Error::Unreachable(_))));
    assert!(matches!(session::phase(&cfg, SID), Err(Error::Unreachable(_))));
    assert!(matches!(
        session::step(&cfg, SID, &user(), &mut |_| {}),
        Err(Error::Unreachable(_))
    ));
    assert!(matches!(
        session::raw_document(&cfg, DOC),
        Err(Error::Unreachable(_))
    ));
    assert!(matches!(relay::lookup(&cfg), Err(Error::Unreachable(_))));
}
