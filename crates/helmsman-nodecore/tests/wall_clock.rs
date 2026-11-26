//! The Skipper's wall clock (Nick, 2026-11-26: "The clock on the Skipper
//! must sync to the browser/device so it's an actual timestamp, not a time
//! passed since it went alive"): `PUT /api/time` with the token sets it,
//! `GET /api/node` and each values event say it once it is set. Read
//! through the router a node runs and the bodies it serves.

use helmsman_nodecore::api::{self, NodeView};
use helmsman_nodecore::http::{self, Read, Request, Response};
use helmsman_nodecore::router::{Effect, Outcome, handle, time_set};
use helmsman_nodecore::settings::Settings;
use proptest::prelude::*;
use serde_json::{Value as Json, json};

/// 2020-01-01T00:00:00Z and 2100-01-01T00:00:00Z.
const MIN: u64 = 1_577_836_800_000;
const MAX: u64 = 4_102_444_800_000;
/// 2026-11-26T14:03:27Z.
const NOW: u64 = 1_795_701_807_000;

const NOT_OBJECT: &str = "want {\"unix_ms\": milliseconds since 1970}";
const NOT_WHOLE: &str = "unix_ms is a whole number of milliseconds since 1970";
const OUT_OF_RANGE: &str = "unix_ms is a time between 2020 and 2100";

fn view() -> NodeView {
    NodeView {
        module: None,
        signals: Vec::new(),
        devices: Vec::new(),
        seq: 3,
        at_ms: 95_000,
        uptime_ms: 100_000,
        clock_offset_ms: None,
        heap_used: 0,
        heap_peak: 0,
        heap_size: 0,
        heap_external: None,
        mac: [0x02, 0, 0, 0, 0, 1],
        ip: Some([192, 168, 33, 56]),
        link_up: true,
        version: "test",
        board: "test",
        io_map: String::new(),
        shares: Vec::new(),
        one_block: false,
        check: None,
        firmware: None,
    }
}

fn node() -> Settings {
    Settings {
        token: "tok".into(),
        ..Settings::default()
    }
}

fn req(text: &str) -> Request {
    match http::parse(text.as_bytes()) {
        Ok(Read::Done(r, _)) => r,
        other => panic!("{other:?}"),
    }
}

fn put_with(auth: &str, body: &str) -> Request {
    req(&format!(
        "PUT /api/time HTTP/1.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}

fn put(settings: &Settings, body: &str) -> Outcome {
    handle(
        &put_with("Authorization: Bearer tok\r\n", body),
        &view(),
        settings,
    )
}

fn reply(o: Outcome) -> Response {
    match o {
        Outcome::Reply(r) => r,
        other => panic!("not a reply: {other:?}"),
    }
}

fn body(r: &Response) -> Json {
    serde_json::from_slice(&r.body).unwrap_or_else(|e| panic!("{e}: {:?}", String::from_utf8_lossy(&r.body)))
}

/// A 400 with `why` as its error.
fn bad(o: Outcome, why: &str, what: &str) {
    let r = reply(o);
    assert_eq!(r.status, 400, "{what}: {:?}", String::from_utf8_lossy(&r.body));
    assert_eq!(body(&r)["error"], why, "{what}");
}

fn get(path: &str, v: &NodeView) -> Json {
    let r = reply(handle(&req(&format!("GET {path} HTTP/1.1\r\n\r\n")), v, &node()));
    assert_eq!(r.status, 200, "{path}");
    body(&r)
}

#[test]
fn a_time_in_range_with_the_token_is_set_and_said_back() {
    for ms in [MIN, NOW, MAX - 1] {
        let o = put(&node(), &format!(r#"{{"unix_ms": {ms}}}"#));
        assert_eq!(o, Outcome::Effect(Effect::SetTime { unix_ms: ms }), "{ms}");
    }
    let r = time_set(NOW);
    assert_eq!(r.status, 200);
    assert_eq!(body(&r), json!({ "unix_ms": NOW }));
    // Other keys beside it are not a reason to refuse.
    let o = put(
        &node(),
        &format!(r#"{{"unix_ms": {NOW}, "zone": "Europe/Berlin"}}"#),
    );
    assert_eq!(o, Outcome::Effect(Effect::SetTime { unix_ms: NOW }));
}

#[test]
fn no_token_set_is_403_and_a_wrong_or_missing_one_is_401() {
    let b = format!(r#"{{"unix_ms": {NOW}}}"#);
    let fresh = Settings::default();
    assert_eq!(fresh.token, "");
    for auth in ["", "Authorization: Bearer tok\r\n", "Authorization: Bearer \r\n"] {
        let r = reply(handle(&put_with(auth, &b), &view(), &fresh));
        assert_eq!(r.status, 403, "no token on the node, auth {auth:?}");
    }
    for auth in [
        "",
        "Authorization: Bearer tok2\r\n",
        "Authorization: Bearer to\r\n",
        "Authorization: Bearer TOK\r\n",
        "Authorization: Basic dG9rOnRvaw==\r\n",
        "Authorization: Bearer \r\n",
        "Authorization: tok\r\n",
    ] {
        let r = reply(handle(&put_with(auth, &b), &view(), &node()));
        assert_eq!(r.status, 401, "auth {auth:?}");
    }
}

#[test]
fn the_range_is_2020_to_2100_and_its_edges_are_exact() {
    bad(
        put(&node(), &format!(r#"{{"unix_ms": {}}}"#, MIN - 1)),
        OUT_OF_RANGE,
        "a ms before 2020",
    );
    bad(
        put(&node(), &format!(r#"{{"unix_ms": {MAX}}}"#)),
        OUT_OF_RANGE,
        "2100 itself",
    );
    bad(put(&node(), r#"{"unix_ms": 0}"#), OUT_OF_RANGE, "0");
    bad(put(&node(), r#"{"unix_ms": 1}"#), OUT_OF_RANGE, "1");
    // Seconds, not milliseconds: a common slip, refused.
    bad(
        put(&node(), r#"{"unix_ms": 1795701807}"#),
        OUT_OF_RANGE,
        "seconds",
    );
    bad(
        put(&node(), r#"{"unix_ms": 18446744073709551615}"#),
        OUT_OF_RANGE,
        "2^64 - 1",
    );
}

#[test]
fn every_body_that_is_not_a_whole_unix_ms_is_a_400_that_says_why() {
    for b in [
        "[]",
        "17957018070",
        r#""1795701807000""#,
        "null",
        "true",
        "",
        "{",
        "not json",
        "[{\"unix_ms\": 1795701807000}]",
    ] {
        bad(put(&node(), b), NOT_OBJECT, b);
    }
    for b in [
        "{}",
        r#"{"unix_ms": "1795701807000"}"#,
        r#"{"unix_ms": 1795701807000.5}"#,
        r#"{"unix_ms": 1795701807000.0}"#,
        r#"{"unix_ms": -1795701807000}"#,
        r#"{"unix_ms": -1}"#,
        r#"{"unix_ms": null}"#,
        r#"{"unix_ms": true}"#,
        r#"{"unix_ms": 1e20}"#,
        r#"{"unix_ms": 1.7957018070e12}"#,
        r#"{"unix_ms": 18446744073709551616}"#,
        r#"{"unix_ms": 99999999999999999999999999999}"#,
        r#"{"unix_ms": {"unix_ms": 1795701807000}}"#,
        r#"{"unix_ms": [1795701807000]}"#,
        r#"{"UNIX_MS": 1795701807000}"#,
        r#"{"time": {"unix_ms": 1795701807000}}"#,
    ] {
        bad(put(&node(), b), NOT_WHOLE, b);
    }
}

#[test]
fn the_node_says_its_wall_time_once_set_and_nothing_else_changes() {
    let unset = view();
    assert_eq!(unset.unix_ms_at(5), None);
    let n = get("/api/node", &unset);
    assert!(n.as_object().unwrap().contains_key("unix_ms"), "{n}");
    assert_eq!(n["unix_ms"], Json::Null, "{n}");
    let ev = api::values_event(&unset);
    assert!(ev.as_object().unwrap().contains_key("at_unix_ms"), "{ev}");
    assert_eq!(ev["at_unix_ms"], Json::Null, "{ev}");

    // Set at uptime 100 000 to NOW: the offset is NOW - 100 000.
    let mut set = view();
    set.clock_offset_ms = Some(NOW - set.uptime_ms);
    assert_eq!(set.unix_ms_at(set.uptime_ms), Some(NOW));
    assert_eq!(set.unix_ms_at(0), Some(NOW - 100_000));
    let n2 = get("/api/node", &set);
    assert_eq!(n2["unix_ms"], NOW, "uptime + offset: {n2}");
    let ev2 = api::values_event(&set);
    assert_eq!(ev2["at_unix_ms"], NOW - 5_000, "at_ms + offset: {ev2}");

    // Every other field is as it was.
    let strip = |mut j: Json, k: &str| {
        j.as_object_mut().unwrap().remove(k);
        j
    };
    assert_eq!(strip(n, "unix_ms"), strip(n2, "unix_ms"));
    assert_eq!(strip(ev, "at_unix_ms"), strip(ev2, "at_unix_ms"));
    // An offset near the top does not wrap.
    let mut top = view();
    top.clock_offset_ms = Some(u64::MAX - 10);
    assert!(top.unix_ms_at(1_000).unwrap() >= u64::MAX - 10);
}

proptest! {
    /// Any whole ms in range is set exactly; any outside is refused.
    #[test]
    fn whole_ms_in_range_set_exactly(ms in any::<u64>()) {
        let o = put(&node(), &format!(r#"{{"unix_ms": {ms}}}"#));
        if (MIN..MAX).contains(&ms) {
            prop_assert_eq!(o, Outcome::Effect(Effect::SetTime { unix_ms: ms }));
        } else {
            let r = reply(o);
            prop_assert_eq!(r.status, 400);
            prop_assert_eq!(body(&r)["error"].clone(), json!(OUT_OF_RANGE));
        }
    }

    /// Whatever the body, the route answers 400 or sets the clock: it
    /// never panics, and nothing else is ever its effect.
    #[test]
    fn any_body_is_a_400_or_a_set(b in "\\PC{0,64}") {
        match put(&node(), &b) {
            Outcome::Effect(Effect::SetTime { unix_ms }) => prop_assert!((MIN..MAX).contains(&unix_ms)),
            Outcome::Reply(r) => prop_assert_eq!(r.status, 400),
            other => prop_assert!(false, "{other:?}"),
        }
    }
}
