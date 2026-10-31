//! The offline probe (card `relay-health`): `GET /v1/health` against a
//! one-shot server that answers with a chosen status. Only a 2xx means
//! the agent is up; a relay without the route (404), one whose upstream
//! is down (503), and every other answer mean it is not.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;

use helmsman_agent::relay::{self, Config, Url};

const TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// Answers one request with `status` and `body`; returns what the probe
/// made of it and the request line it sent.
fn probe_against(status: u16, body: &str) -> (Result<(), relay::Error>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = body.to_string();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut first = String::new();
        r.read_line(&mut first).unwrap();
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        let _ = write!(
            s,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        first
    });
    let cfg = Config::new(
        Url::parse(&format!("http://{addr}")).unwrap(),
        TOKEN.into(),
        "v3".into(),
    );
    let r = relay::probe(&cfg);
    (r, server.join().unwrap())
}

/// The relay's 503 body (relay protocol v1, `GET /v1/health`).
fn upstream_down(category: &str) -> String {
    format!(
        r#"{{"type":"error","error":{{"type":"upstream_unavailable","message":"the model API rejected the relay's key","retryable":true}},"request_id":"r","ok":false,"protocol":1,"upstream":"{category}"}}"#
    )
}

#[test]
fn only_a_2xx_health_answer_means_up() {
    let (r, line) = probe_against(200, r#"{"ok":true,"protocol":1,"upstream":"ok"}"#);
    assert_eq!(r, Ok(()));
    assert_eq!(line, "GET /v1/health HTTP/1.1\r\n");
    assert_eq!(probe_against(204, "").0, Ok(()));
}

#[test]
fn a_relay_without_the_health_route_is_not_up() {
    // Before relay-health any answer below 500 but 401/403 counted, so a
    // 404 from a relay without the route read as online.
    for status in [404, 400, 405, 429, 301, 418] {
        let (r, _) = probe_against(status, "");
        let e = r.expect_err(&status.to_string());
        assert_eq!(e.kind(), "relay_unhealthy", "{status}: {e}");
        assert!(e.to_string().contains(&status.to_string()), "{e}");
    }
}

#[test]
fn a_503_says_the_upstream_is_down_and_why() {
    let (r, _) = probe_against(503, &upstream_down("key_rejected"));
    let e = r.unwrap_err();
    assert_eq!(e.kind(), "upstream_unavailable", "{e}");
    assert!(e.retryable(), "{e}");
    assert!(e.to_string().contains("key_rejected"), "{e}");
    // A 5xx without the relay's body still means down.
    for status in [500, 502, 503] {
        let e = probe_against(status, "<html>bad gateway</html>").0.unwrap_err();
        assert_eq!(e.kind(), "relay_unavailable", "{status}: {e}");
        assert!(
            !e.to_string().contains("html"),
            "the body is not echoed unless it is the relay's: {e}"
        );
    }
}

#[test]
fn a_refused_token_is_not_up() {
    for status in [401, 403] {
        let e = probe_against(status, "").0.unwrap_err();
        assert_eq!(e.kind(), "relay_refused_token", "{status}: {e}");
        assert!(!e.retryable());
    }
}
