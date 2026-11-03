//! `fetch_document`, the one tool the relay runs (relay agent v6): the box
//! posts the model's input to the relay's tool route and stores what comes
//! back as a drop-in. A loopback relay answers each case the protocol
//! names; a recording box API stands in for `POST /api/dropins`. No mocked
//! client library: real sockets, the real HTTP path.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helmsman_agent::http::{Timeouts, Url};
use helmsman_agent::relay::Config;
use helmsman_agent::tools::{self, Caller, Drafts, FETCH_PDF_MAX, FETCH_TEXT_MAX};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

const TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// What the relay answers: status, headers (name, value) and body bytes.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn json_answer(status: u16, j: &Json) -> Answer {
    Answer {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: j.to_string().into_bytes(),
    }
}

/// A document answer with the protocol's headers, the hash computed
/// unless `sha` says otherwise.
fn document(kind: &str, name: &str, body: &[u8], sha: Option<&str>) -> Answer {
    let ct = match kind {
        "pdf" => "application/pdf",
        "eds" => "application/vnd.odva.eds",
        _ => "text/plain; charset=utf-8",
    };
    let sha = sha.map_or_else(|| hex(&Sha256::digest(body)), str::to_string);
    Answer {
        status: 200,
        headers: vec![
            ("Content-Type".into(), ct.into()),
            ("x-document-kind".into(), kind.into()),
            ("x-document-name".into(), name.into()),
            ("x-document-sha256".into(), sha),
            (
                "x-document-source".into(),
                "https://www.moxa.com/getmedia/abc/file".into(),
            ),
            ("x-document-wire-bytes".into(), body.len().to_string()),
        ],
        body: body.to_vec(),
    }
}

/// What the relay saw: the request line, the headers, the body as JSON.
#[derive(Debug, Clone)]
struct Seen {
    line: String,
    headers: String,
    body: Json,
}

/// Answers one request with `answer` and records it.
fn relay(answer: Answer) -> (Config, JoinHandle<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
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
        let mut head = format!(
            "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
            answer.status,
            answer.body.len()
        );
        for (n, v) in &answer.headers {
            head.push_str(n);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        // The client may hang up early (a body past its cap): not the test's failure.
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&answer.body);
        Seen {
            line,
            headers,
            body: serde_json::from_slice(&body).unwrap_or(Json::Null),
        }
    });
    let mut cfg = Config::new(
        Url::parse(&format!("http://{addr}")).unwrap(),
        TOKEN.into(),
        "v6".into(),
    );
    cfg.timeouts = Timeouts {
        connect: Duration::from_secs(2),
        idle: Duration::from_secs(5),
        total: Duration::from_secs(20),
    };
    (cfg, server)
}

/// A box API that records what the tool stores and answers 201 like
/// `POST /api/dropins`.
#[derive(Default)]
struct Box_ {
    stored: Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl Caller for Box_ {
    fn call(&self, method: &str, path: &str, _body: Option<&Json>) -> Result<(u16, Json), String> {
        Err(format!("unexpected {method} {path}"))
    }

    fn post_bytes(&self, path: &str, content_type: &str, bytes: &[u8]) -> Result<(u16, Json), String> {
        let mut st = self.stored.lock().unwrap();
        st.push((path.to_string(), content_type.to_string(), bytes.to_vec()));
        let id = st.len();
        let kind = path.split("kind=").nth(1).unwrap_or("prose").to_string();
        Ok((
            201,
            json!({ "id": id, "name": "from-box", "kind": kind, "bytes": bytes.len(), "chunks": 1 }),
        ))
    }
}

fn input() -> Json {
    json!({ "url": "https://www.moxa.com/getmedia/abc/e1213.eds", "vendor_id": 991, "product_code": 515, "query": "Moxa ioLogik E1213" })
}

fn run(answer: Answer, input: &Json) -> (tools::Outcome, Box_, Seen) {
    let (cfg, server) = relay(answer);
    let b = Box_::default();
    let out = tools::run_with(&b, Some(&cfg), &mut Drafts::default(), "fetch_document", input);
    let seen = server.join().unwrap();
    (out, b, seen)
}

#[test]
fn the_input_goes_to_the_relays_tool_route_with_the_box_token() {
    let (out, b, seen) = run(
        document(
            "eds",
            "e1213.eds",
            b"[File]\nDescText = \"EtherNet/IP Adapter\";\n",
            None,
        ),
        &input(),
    );
    assert!(out.ok, "{}", out.summary);
    assert_eq!(seen.line, "POST /v1/agents/v6/tools/fetch_document HTTP/1.1\r\n");
    assert!(
        seen.headers
            .contains(&format!("Authorization: Bearer {TOKEN}\r\n")),
        "{}",
        seen.headers
    );
    assert!(seen.headers.contains("Content-Type: application/json\r\n"));
    assert_eq!(seen.body, input(), "the input verbatim");
    let stored = b.stored.lock().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, "/api/dropins?name=e1213.eds&kind=prose");
    assert_eq!(stored[0].1, "text/plain; charset=utf-8");
    assert_eq!(out.data["type"], "document");
    assert_eq!(out.data["dropin_id"], 1);
    assert_eq!(out.data["kind"], "eds");
    assert_eq!(out.data["dropin_kind"], "prose");
    assert_eq!(out.data["name"], "from-box", "the name the box stored it under");
    assert_eq!(out.data["size"], stored[0].2.len());
    assert_eq!(out.data["sha256"], hex(&Sha256::digest(&stored[0].2)));
    assert_eq!(out.data["source"], "https://www.moxa.com/getmedia/abc/file");
    assert!(out.data["next"].as_str().unwrap().contains("read_dropin"));
}

#[test]
fn a_pdf_is_stored_as_a_pdf_and_read_when_attached() {
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.extend(std::iter::repeat_n(b'x', 70_000));
    let (out, b, _) = run(document("pdf", "E1200 manual.pdf", &pdf, None), &input());
    assert!(out.ok, "{}", out.summary);
    let stored = b.stored.lock().unwrap();
    assert_eq!(
        stored[0].0, "/api/dropins?name=E1200manual.pdf&kind=pdf",
        "the name made safe"
    );
    assert_eq!(stored[0].1, "application/pdf");
    assert_eq!(stored[0].2, pdf);
    assert_eq!(out.data["kind"], "pdf");
    assert_eq!(out.data["dropin_kind"], "pdf");
    assert!(out.data["next"].as_str().unwrap().contains("attach"));
}

#[test]
fn a_library_match_is_handed_to_the_model_and_nothing_is_stored() {
    let lib = json!({ "type": "library", "index_url": "https://raw.githubusercontent.com/x/index.json",
        "device": { "folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213" } });
    let (out, b, _) = run(
        json_answer(200, &lib),
        &json!({ "url": "", "vendor_id": 991, "product_code": 515, "query": "" }),
    );
    assert!(out.ok, "{}", out.summary);
    assert_eq!(out.data["type"], "library");
    assert_eq!(out.data["device"]["model"], "ioLogik E1213");
    assert_eq!(out.summary, "library: Moxa ioLogik E1213");
    assert!(b.stored.lock().unwrap().is_empty());
}

#[test]
fn no_match_and_the_relays_refusals_are_errors_in_words() {
    let (out, b, _) = run(
        json_answer(404, &json!({ "type": "no_match", "message": "nothing" })),
        &json!({ "url": "", "vendor_id": 0, "product_code": 0, "query": "a thing" }),
    );
    assert!(!out.ok);
    assert!(out.summary.contains("ask the user"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
    for (status, kind) in [
        (403, "fetch_refused"),
        (429, "rate_limited"),
        (502, "fetch_failed"),
    ] {
        let body =
            json!({ "type": "error", "error": { "type": kind, "message": "because", "retryable": false } });
        let (out, b, _) = run(json_answer(status, &body), &input());
        assert!(!out.ok);
        assert!(
            out.summary.contains(kind) && out.summary.contains("because"),
            "{}",
            out.summary
        );
        assert!(b.stored.lock().unwrap().is_empty());
    }
}

#[test]
fn a_document_whose_hash_does_not_match_is_not_stored() {
    let (out, b, _) = run(
        document("eds", "e.eds", b"[File]\n", Some(&"0".repeat(64))),
        &input(),
    );
    assert!(!out.ok);
    assert!(out.summary.contains("SHA-256"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
    // No hash at all is a mismatch too.
    let (out, b, _) = run(document("eds", "e.eds", b"[File]\n", Some("")), &input());
    assert!(!out.ok);
    assert!(b.stored.lock().unwrap().is_empty());
}

#[test]
fn a_document_past_the_relays_cap_is_refused_before_it_is_stored() {
    // The relay's caps (relay-protocol.md, "Relay-run tools").
    assert_eq!(FETCH_PDF_MAX, 12 * 1024 * 1024);
    assert_eq!(FETCH_TEXT_MAX, 2 * 1024 * 1024);
    let big = vec![b'a'; FETCH_TEXT_MAX + 1];
    let (out, b, _) = run(document("text", "big.txt", &big, None), &input());
    assert!(!out.ok);
    assert!(out.summary.contains("bytes"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.resize(FETCH_PDF_MAX + 1, b'x');
    let (out, b, _) = run(document("pdf", "big.pdf", &pdf, None), &input());
    assert!(!out.ok, "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
}

/// A box whose drop-in store is full-size only up to 2 MiB, answering
/// `413` past it like `POST /api/dropins`.
struct SmallBox;

impl Caller for SmallBox {
    fn call(&self, method: &str, path: &str, _body: Option<&Json>) -> Result<(u16, Json), String> {
        Err(format!("unexpected {method} {path}"))
    }

    fn post_bytes(&self, _path: &str, _ct: &str, bytes: &[u8]) -> Result<(u16, Json), String> {
        assert!(bytes.len() > 2 * 1024 * 1024, "only big documents reach this box");
        Ok((
            413,
            json!({ "error": "a PDF is at most 2 MiB (2097152 bytes), the relay's limit; send the pages that matter, or split it" }),
        ))
    }
}

#[test]
fn a_document_within_the_relays_cap_but_past_the_boxs_is_refused_in_words() {
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.resize(3 * 1024 * 1024, b'x');
    let (cfg, server) = relay(document("pdf", "big.pdf", &pdf, None));
    let out = tools::run_with(
        &SmallBox,
        Some(&cfg),
        &mut Drafts::default(),
        "fetch_document",
        &input(),
    );
    server.join().unwrap();
    assert!(!out.ok, "{}", out.summary);
    assert!(
        out.summary.contains("too large for the box") && out.summary.contains("2 MiB"),
        "{}",
        out.summary
    );
}

#[test]
fn a_fake_pdf_an_unknown_kind_and_an_empty_body_are_refused() {
    let (out, b, _) = run(document("pdf", "x.pdf", b"not a pdf at all", None), &input());
    assert!(!out.ok && out.summary.contains("not one"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
    let (out, b, _) = run(document("exe", "x.exe", b"MZ", None), &input());
    assert!(!out.ok && out.summary.contains("exe"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
    let (out, b, _) = run(document("eds", "x.eds", b"", None), &input());
    assert!(!out.ok && out.summary.contains("empty"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
}

#[test]
fn a_hostile_name_is_made_safe() {
    let (out, b, _) = run(
        document("text", "../../etc/passwd\r\nX-Evil: 1", b"hello", None),
        &input(),
    );
    assert!(out.ok, "{}", out.summary);
    let stored = b.stored.lock().unwrap();
    // The CR LF split the value into another header the box never sees as a name.
    assert_eq!(stored[0].0, "/api/dropins?name=etcpasswd&kind=prose");
}

#[test]
fn the_schema_is_the_relays_and_a_box_without_a_relay_says_so() {
    assert!(tools::validate("fetch_document", &input()).is_ok());
    for bad in [
        json!({ "url": "https://x", "vendor_id": 1, "product_code": 2 }),
        json!({ "url": "https://x", "vendor_id": 1, "product_code": 2, "query": "", "extra": 1 }),
        json!({ "url": 7, "vendor_id": 1, "product_code": 2, "query": "" }),
        json!({ "url": "https://x", "vendor_id": -1, "product_code": 2, "query": "" }),
    ] {
        assert!(tools::validate("fetch_document", &bad).is_err(), "{bad}");
    }
    let b = Box_::default();
    let out = tools::run(&b, &mut Drafts::default(), "fetch_document", &input());
    assert!(!out.ok);
    assert!(out.summary.contains("relay"), "{}", out.summary);
    // Local rules before any request: a non-https URL, an over-long field.
    let cfg = Config::new(
        Url::parse("http://127.0.0.1:1").unwrap(),
        TOKEN.into(),
        "v6".into(),
    );
    let out = tools::run_with(
        &b,
        Some(&cfg),
        &mut Drafts::default(),
        "fetch_document",
        &json!({ "url": "http://www.moxa.com/x", "vendor_id": 0, "product_code": 0, "query": "" }),
    );
    assert!(!out.ok && out.summary.contains("https"), "{}", out.summary);
    let out = tools::run_with(
        &b,
        Some(&cfg),
        &mut Drafts::default(),
        "fetch_document",
        &json!({ "url": "", "vendor_id": 0, "product_code": 0, "query": "q".repeat(2049) }),
    );
    assert!(!out.ok && out.summary.contains("2048"), "{}", out.summary);
    assert!(b.stored.lock().unwrap().is_empty());
    assert!(tools::NAMES.contains(&"fetch_document"));
    assert!(
        !tools::READ_ONLY.contains(&"fetch_document"),
        "it reaches the internet"
    );
}
