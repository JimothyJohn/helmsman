//! Installing a bundle from a marketplace on the review screen, against
//! the real box: a signed index and bundle served on loopback, the box's
//! install route with its review, and a scripted relay standing in for the
//! Agent. Every request is the [`Request`](crate::review::Request) the
//! screen made, carried out over a socket as the browser's `fetch` would.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use helmsman_agent::http::{Timeouts, Url};
use helmsman_agent::relay;
use helmsman_plc::Window;
use helmsman_plc::api::chat;
use helmsman_plc::bundle::index::{self, Index};
use helmsman_plc::bundle::{Bundle, BundleManifest, SigningKey, TrustStore, capabilities};
use helmsman_plc::spec::Defaults;
use helmsman_plc::store::{ChangeSet, ModuleFiles};
use serde_json::{Value as Json, json};

use crate::install_review::State;
use crate::review::{Review, Session};
use crate::review_tests::{Bench, ENGINEER, Jar, Market, OPERATOR, settle, settle_in};

const RELAY_TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

const HORNER_MANIFEST: &str = "\
name     horner
author   Acme Automation
scan_ms  2
reads    start
drives   horn
";

const HORNER_SOURCE: &str = "
PROGRAM horner
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT horn : BOOL; END_VAR
  horn := start;
END_PROGRAM";

fn publisher() -> SigningKey {
    SigningKey::from_seed(&[7; 32]).unwrap()
}

fn trust() -> TrustStore {
    let mut t = TrustStore::default();
    t.add("acme", publisher().public()).unwrap();
    t
}

/// The files of a static marketplace: the signed index and its one bundle.
fn site() -> Vec<(String, Vec<u8>)> {
    let cs = ChangeSet {
        modules: BTreeMap::from([(
            "horner".to_string(),
            ModuleFiles::st(HORNER_MANIFEST, HORNER_SOURCE),
        )]),
        io_map: None,
        calendar: None,
    };
    let d = Defaults {
        period: Duration::from_millis(2),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        m: 16,
    };
    let manifest = BundleManifest {
        name: "horn-kit".into(),
        version: "1.0.0".into(),
        author: "Acme Automation".into(),
        description: Some("Sounds the horn while start is held.".into()),
        caps: capabilities(&cs, &d).unwrap(),
    };
    let mut b = Bundle::new(manifest, cs);
    b.sign(&publisher());
    let files = vec![("horn-kit-1.0.0.hbundle".to_string(), b.encode())];
    let idx = Index::build("acme", &files).unwrap().encode();
    let sig = index::sign(idx.as_bytes(), &publisher());
    let mut out = files;
    out.push(("index.json".into(), idx.into_bytes()));
    out.push(("index.json.sig".into(), sig.into_bytes()));
    out
}

/// The files on loopback, by name, until the test ends.
fn serve(files: Vec<(String, Vec<u8>)>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let files = Arc::new(files);
    thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { break };
            let files = Arc::clone(&files);
            thread::spawn(move || {
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                let _ = r.read_line(&mut line);
                let name = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string();
                let reply = match files.iter().find(|(n, _)| *n == name) {
                    Some((_, body)) => {
                        let mut v = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        v.extend_from_slice(body);
                        v
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                    }
                };
                let _ = s.write_all(&reply);
            });
        }
    });
    addr
}

/// A relay on loopback that answers each review turn with the next text
/// of its script, as one finished model turn.
struct Relay {
    addr: SocketAddr,
    asked: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn event(out: &mut String, data: &Json) {
    let _ = write!(out, "event: {}\ndata: {data}\n\n", data["type"].as_str().unwrap());
}

fn turn(text: &str) -> String {
    let mut o = String::new();
    event(
        &mut o,
        &json!({"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[],"model":"m","stop_reason":null,"usage":{"input_tokens":1,"output_tokens":1}}}),
    );
    event(
        &mut o,
        &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    );
    event(
        &mut o,
        &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
    );
    event(&mut o, &json!({"type":"content_block_stop","index":0}));
    event(
        &mut o,
        &json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":9}}),
    );
    event(&mut o, &json!({"type":"message_stop"}));
    event(
        &mut o,
        &json!({"type":"relay_usage","request_id":"r","agent_version":"helmsman/review-v1","model":"m","input_tokens":1,"output_tokens":9,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}),
    );
    o
}

impl Relay {
    fn start(script: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let asked = Arc::new(Mutex::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (asked, stop) = (Arc::clone(&asked), Arc::clone(&stop));
            thread::spawn(move || {
                let mut script = script.into_iter();
                while !stop.load(Ordering::Acquire) {
                    let Ok((mut s, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    s.set_nonblocking(false).unwrap();
                    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    let mut first = String::new();
                    let _ = r.read_line(&mut first);
                    let mut len = 0usize;
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0; len];
                    let _ = r.read_exact(&mut body);
                    if first.starts_with("GET ") {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                        );
                        continue;
                    }
                    *asked.lock().unwrap_or_else(PoisonError::into_inner) += 1;
                    if let Some(t) = script.next() {
                        let _ = write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{}",
                            turn(&t)
                        );
                    }
                }
            })
        };
        Self {
            addr,
            asked,
            stop,
            thread: Some(thread),
        }
    }

    fn asked(&self) -> usize {
        *self.asked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn setup(&self) -> chat::Setup {
        let url = Url::parse(&format!("http://{}", self.addr)).unwrap();
        let mut rc = relay::Config::new(url, RELAY_TOKEN.into(), "v1".into());
        rc.timeouts = Timeouts {
            connect: Duration::from_secs(2),
            idle: Duration::from_secs(5),
            total: Duration::from_secs(20),
        };
        chat::Setup::new(rc)
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

fn verdict(refuse: bool, reason: &str) -> String {
    json!({
        "summary": "Sounds the horn while start is held.",
        "reads": ["start: the push button"], "writes": ["horn: on while start is held"],
        "devices": [], "access": [], "at_start": [], "undeclared": [],
        "risk": if refuse { "high" } else { "medium" }, "refuse": refuse, "reason": reason,
    })
    .to_string()
}

/// A bench box that installs from the site, and a screen signed in as
/// `role` with the form filled in.
fn screen(b: &Bench, jar: &Jar, role: &str, pin: &str, index: &str) -> Review {
    let mut r = Review::new();
    r.login_pin(role, pin.into());
    settle_in(b, jar, &mut r, 0.0);
    assert!(r.session.is_some(), "signed in: {:?}", r.login_problem);
    r.install.index = index.into();
    r.install.name = "horn-kit".into();
    r
}

fn index_url() -> String {
    format!("http://{}/market/index.json", serve(site()))
}

#[test]
fn a_bundle_the_agent_does_not_refuse_becomes_a_draft_with_its_review_on_it() {
    let relay = Relay::start(vec![verdict(false, "Check the horn's wiring first.")]);
    let b = Bench::with_market(Market {
        trust: trust(),
        chat: Some(relay.setup()),
    });
    let mut r = screen(&b, &b.jar, "engineer", ENGINEER, &index_url());
    r.install_bundle();
    assert!(r.install.busy, "the install is on its way");
    settle(&b, &mut r, 0.0);
    assert!(!r.install.busy);
    assert_eq!(relay.asked(), 1);
    let id = r.install.installed.expect("a draft was made");
    assert!(r.install.stopped.is_none() && r.install.error.is_none());
    // The draft is open, and the screen has its review to show above the
    // approval.
    let o = r.open.as_ref().expect("the new draft is opened");
    assert_eq!(o.id, id);
    assert_eq!(o.json["kind"], "bundle");
    let review = o.bundle_review().expect("a bundle draft carries its review");
    assert_eq!(review.state, State::Reviewed);
    assert_eq!(review.risk.as_deref(), Some("medium"));
    assert!(review.summary.contains("horn"), "{}", review.summary);
    assert_eq!(
        review.lists[1],
        ("Drives", vec!["horn: on while start is held".to_string()])
    );
    assert_eq!(review.reason, "Check the horn's wiring first.");
    // The approval is the box's own: output-affecting, an engineer, a replay.
    assert_eq!(o.tier(), Some("output-affecting"));
    assert_eq!(o.needs(), Some(("engineer".to_string(), true)));
    // The list names it as a bundle.
    assert!(
        r.list
            .iter()
            .any(|row| row.id == id && row.title == "bundle horn-kit 1.0.0"),
        "{:?}",
        r.list
    );
}

#[test]
fn a_refusal_shows_its_reason_as_text_and_has_no_way_on() {
    let reason = "<script>alert(1)</script> **it drives horn with nothing to turn it off**";
    let relay = Relay::start(vec![verdict(true, reason), verdict(true, reason)]);
    let b = Bench::with_market(Market {
        trust: trust(),
        chat: Some(relay.setup()),
    });
    let mut r = screen(&b, &b.jar, "engineer", ENGINEER, &index_url());
    r.install_bundle();
    settle(&b, &mut r, 0.0);
    let s = r.install.stopped.clone().expect("the install stopped");
    assert_eq!(s.review.state, State::Refused);
    assert_eq!(s.review.reason, reason, "the Agent's words, as text");
    assert_eq!(s.bundle, "horn-kit 1.0.0");
    assert!(s.error.contains("advises against"), "{}", s.error);
    assert!(!s.can_go_on());
    assert!(r.install.installed.is_none() && r.open.is_none());
    // An engineer cannot go on past a refusal from here, and the box would
    // not take it: nothing is sent.
    r.install_without_review();
    assert_eq!(r.poll().len(), 0);
    assert_eq!(relay.asked(), 1);
    // The box has no draft for it.
    r.relist();
    settle(&b, &mut r, 0.0);
    assert_eq!(r.list.len(), 0, "{:?}", r.list);
}

#[test]
fn without_a_review_an_engineer_may_go_on_and_the_draft_says_so() {
    // No relay on this box: the Agent cannot be asked.
    let b = Bench::with_market(Market {
        trust: trust(),
        chat: None,
    });
    let url = index_url();
    let mut r = screen(&b, &b.jar, "engineer", ENGINEER, &url);
    r.install_bundle();
    settle(&b, &mut r, 0.0);
    let s = r.install.stopped.clone().expect("stopped at not reviewed");
    assert_eq!(s.review.state, State::NotReviewed);
    assert!(s.review.why.contains("not set up"), "{}", s.review.why);
    assert!(s.can_go_on());
    assert!(r.install.installed.is_none());
    assert!(r.engineer());
    // An operator is signed in elsewhere: the way on is not theirs.
    let jar = Jar::default();
    let mut op = screen(&b, &jar, "operator", OPERATOR, &url);
    assert!(!op.engineer());
    op.install_bundle();
    settle_in(&b, &jar, &mut op, 0.0);
    assert!(op.install.stopped.is_some());
    op.install_without_review();
    settle_in(&b, &jar, &mut op, 0.0);
    let e = op.install.error.clone().expect("the box refused the operator");
    assert!(e.contains("engineer"), "{e}");
    assert!(op.install.installed.is_none());
    // The engineer goes on: a draft, marked as installed without a review.
    r.install_without_review();
    settle(&b, &mut r, 0.0);
    assert!(
        r.install.stopped.is_none() && r.install.error.is_none(),
        "{:?}",
        r.install
    );
    let o = r.open.as_ref().expect("the draft is open");
    let review = o.bundle_review().unwrap();
    assert_eq!(review.state, State::NotReviewed);
    assert!(
        review.went_on_by.as_deref().unwrap_or("").contains("engineer"),
        "{review:?}"
    );
    assert!(
        review.headline().contains("went on without a review"),
        "{}",
        review.headline()
    );
    assert_eq!(o.tier(), Some("output-affecting"));
    // The audit trail names it.
    let log = b.audit();
    assert!(log.contains("installed without a review"), "{log}");
}

#[test]
fn nothing_is_sent_without_a_session_a_name_or_while_one_is_on_its_way() {
    let b = Bench::with_market(Market {
        trust: trust(),
        chat: None,
    });
    let mut r = Review::new();
    // Its first round of reads is not what is under test.
    let _ = r.poll();
    r.install.index = "http://example.invalid/index.json".into();
    r.install.name = "horn-kit".into();
    r.install_bundle();
    assert_eq!(r.poll().len(), 0, "not signed in");
    assert!(r.install.error.as_deref().unwrap_or("").contains("Sign in"));
    r.session = Some(Session {
        user: None,
        role: "engineer".into(),
    });
    r.install.name = "   ".into();
    r.install_bundle();
    assert_eq!(r.poll().len(), 0);
    assert!(r.install.error.as_deref().unwrap_or("").contains("Type"));
    r.install.name = "horn-kit".into();
    r.install_bundle();
    r.install_bundle();
    assert_eq!(r.poll().len(), 1, "one install at a time");
    drop(b);
}

#[test]
fn a_marketplace_the_box_cannot_use_is_a_plain_error_and_not_a_stop() {
    let b = Bench::with_market(Market {
        trust: trust(),
        chat: None,
    });
    let mut r = screen(&b, &b.jar, "engineer", ENGINEER, "http://127.0.0.1:1/index.json");
    r.install_bundle();
    settle(&b, &mut r, 0.0);
    assert!(r.install.stopped.is_none());
    assert!(r.install.error.is_some());
    assert!(!r.install.busy);
    r.install.dismiss();
    assert!(!r.install.has_result());
}
