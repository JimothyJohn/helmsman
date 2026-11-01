//! Webhook sinks: the contract in `helmsman_plc::sink`. A real HTTP server
//! on loopback (std `TcpListener`) plays every kind of receiver: one that
//! fails then recovers, hangs, redirects elsewhere, answers forever, or
//! rejects.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_io::{Ty, Value};
use helmsman_plc::SignalTable;
use helmsman_plc::sink::config::{self, parse};
use helmsman_plc::sink::http::{self, Outcome, Request, SendError};
use helmsman_plc::sink::queue::Queue;
use helmsman_plc::sink::url::{Url, UrlError, refused};
use helmsman_plc::sink::worker::{self, Backoff, SinkConfig, Stats};
use helmsman_plc::sink::{Emitter, Event, Log, Secret, Sinks, Trigger};

// ---------------------------------------------------------------- server

/// One request as the server read it.
#[derive(Debug, Clone)]
struct Req {
    head: String,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }

    fn request_line(&self) -> &str {
        self.head.lines().next().unwrap_or_default()
    }
}

enum Reply {
    Status(u16),
    /// Write these bytes, then close.
    Raw(Vec<u8>),
    /// Write `head`, then `filler` over and over until the client goes.
    Endless {
        head: Vec<u8>,
        filler: Vec<u8>,
    },
    /// Say nothing for this long, then close.
    Hang(Duration),
}

struct Server {
    addr: SocketAddr,
    reqs: Arc<Mutex<Vec<Req>>>,
    conns: Arc<AtomicUsize>,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn reqs(&self) -> Vec<Req> {
        self.reqs.lock().unwrap().clone()
    }

    fn conns(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }
}

/// Reads one request: head, then `Content-Length` bytes of body.
fn read_req(s: &mut TcpStream) -> Option<Req> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut b = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = s.read(&mut b).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&b[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut body = buf[end + 4..].to_vec();
    let len: usize = Req {
        head: head.clone(),
        body: String::new(),
    }
    .header("content-length")
    .and_then(|v| v.parse().ok())
    .unwrap_or(0);
    while body.len() < len {
        let n = s.read(&mut b).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&b[..n]);
    }
    Some(Req {
        head,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// A server that answers the n-th request (from 0) with `reply(n, req)`.
fn serve(reply: impl Fn(usize, &Req) -> Reply + Send + Sync + 'static) -> Server {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let reqs = Arc::new(Mutex::new(Vec::new()));
    let conns = Arc::new(AtomicUsize::new(0));
    let (r2, c2) = (Arc::clone(&reqs), Arc::clone(&conns));
    let reply = Arc::new(reply);
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            c2.fetch_add(1, Ordering::SeqCst);
            let (reqs, reply) = (Arc::clone(&r2), Arc::clone(&reply));
            thread::spawn(move || {
                let Some(req) = read_req(&mut s) else { return };
                let n = {
                    let mut g = reqs.lock().unwrap();
                    g.push(req.clone());
                    g.len() - 1
                };
                match reply(n, &req) {
                    Reply::Status(code) => {
                        let _ = write!(
                            s,
                            "HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                    }
                    Reply::Raw(b) => {
                        let _ = s.write_all(&b);
                    }
                    Reply::Endless { head, filler } => {
                        let _ = s.write_all(&head);
                        let t = Instant::now();
                        while t.elapsed() < Duration::from_secs(10) && s.write_all(&filler).is_ok() {}
                    }
                    Reply::Hang(d) => thread::sleep(d),
                }
            });
        }
    });
    Server { addr, reqs, conns }
}

/// A loopback port with nothing listening.
fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn wait_until(what: &str, ok: impl Fn() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// A fresh directory of its own: the pid and a counter keep parallel
/// copies of this suite apart.
fn tmpdir(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "sink-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<String>>>);

impl Logs {
    fn log(&self) -> Log {
        let l = Arc::clone(&self.0);
        Arc::new(move |m: &str| l.lock().unwrap().push(m.to_string()))
    }

    fn all(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}

fn no_dns(_: &str, _: u16) -> io::Result<Vec<SocketAddr>> {
    panic!("no lookup expected")
}

fn cfg(name: &str, url: &str) -> SinkConfig {
    SinkConfig {
        name: name.into(),
        url: Url::parse(url).unwrap(),
        headers: vec![],
        timeout: Duration::from_secs(2),
        queue_bytes: 1 << 20,
        backoff: Backoff {
            min: Duration::from_millis(2),
            max: Duration::from_millis(10),
        },
        allow_local: true,
    }
}

fn ev(signal: &str, v: Value) -> Event {
    Event {
        signal: signal.into(),
        value: v,
        trigger: Trigger::Change,
        ts_ms: Some(1_793_404_800_000),
    }
}

fn post(url: &Url, timeout: Duration, allow_local: bool) -> Outcome {
    let req = Request {
        url,
        headers: &[],
        key: "k-1-0",
        body: b"{}",
    };
    http::post(&req, timeout, allow_local, &http::std_resolve)
}

// ---------------------------------------------------------------- URLs

#[test]
fn urls_parse_scheme_host_port_and_path() {
    let u = Url::parse("HTTPS://MES.Example.com/hooks/line3?x=1").unwrap();
    assert_eq!(u.host, "mes.example.com");
    assert_eq!(u.port, 443);
    assert_eq!(u.path(), "/hooks/line3?x=1");
    assert_eq!(u.origin(), "https://mes.example.com:443");
    assert_eq!(u.host_header(), "mes.example.com");
    let u = Url::parse("http://10.0.0.5:8080").unwrap();
    assert_eq!(
        (u.port, u.path(), u.host_header().as_str()),
        (8080, "/", "10.0.0.5:8080")
    );
    let u = Url::parse("http://[fd00::5]:81?q").unwrap();
    assert_eq!((u.host.as_str(), u.port, u.path()), ("fd00::5", 81, "/?q"));
    assert_eq!(u.origin(), "http://[fd00::5]:81");
}

#[test]
fn urls_refuse_other_schemes_credentials_fragments_and_junk() {
    let cases: &[(&str, UrlError)] = &[
        ("ftp://h/x", UrlError::Scheme),
        ("file:///etc/passwd", UrlError::Scheme),
        ("gopher://h:70/_x", UrlError::Scheme),
        ("javascript:alert(1)", UrlError::Scheme),
        ("//h/x", UrlError::Scheme),
        ("h/x", UrlError::Scheme),
        ("http://user:pw@h/x", UrlError::UserInfo),
        ("http://h@169.254.169.254/", UrlError::UserInfo),
        ("http://h/x#frag", UrlError::Fragment),
        ("http://h /x", UrlError::Chars),
        ("http://h/x\r\nX-Evil: 1", UrlError::Chars),
        ("http://h/\u{e9}", UrlError::Chars),
        ("", UrlError::Scheme),
        ("http://", UrlError::Host),
        ("http://:80/", UrlError::Host),
        ("http://-h/", UrlError::Host),
        ("http://h..x/", UrlError::Host),
        ("http://h_x/", UrlError::Host),
        ("http://[::1/", UrlError::Host),
        ("http://[nope]/", UrlError::Host),
        ("http://[::1]x/", UrlError::Host),
        ("http://h:0/", UrlError::Port),
        ("http://h:65536/", UrlError::Port),
        ("http://h:+80/", UrlError::Port),
        ("http://h:/", UrlError::Port),
    ];
    for (u, want) in cases {
        assert_eq!(Url::parse(u).as_ref().err(), Some(want), "{u:?}");
    }
}

#[test]
fn a_url_never_prints_its_path() {
    let u = Url::parse("https://hooks.example.com/services/T0K3N/S3CR3T?sig=abc").unwrap();
    let shown = format!("{u:?} {}", u.origin());
    assert!(
        !shown.contains("T0K3N") && !shown.contains("S3CR3T") && !shown.contains("sig"),
        "{shown}"
    );
    let e = Url::parse("ftp://h/S3CR3T").unwrap_err();
    assert!(!e.to_string().contains("S3CR3T"));
}

#[test]
fn metadata_link_local_and_odd_addresses_are_refused() {
    let bad: &[IpAddr] = &[
        Ipv4Addr::new(169, 254, 169, 254).into(),
        Ipv4Addr::new(169, 254, 0, 1).into(),
        Ipv4Addr::new(100, 100, 100, 200).into(),
        Ipv4Addr::UNSPECIFIED.into(),
        Ipv4Addr::new(0, 1, 2, 3).into(),
        Ipv4Addr::BROADCAST.into(),
        Ipv4Addr::new(224, 0, 0, 1).into(),
        Ipv4Addr::new(240, 0, 0, 1).into(),
        "fe80::1".parse::<Ipv6Addr>().unwrap().into(),
        "febf::1".parse::<Ipv6Addr>().unwrap().into(),
        "fec0::1".parse::<Ipv6Addr>().unwrap().into(),
        "fd00:ec2::254".parse::<Ipv6Addr>().unwrap().into(),
        "::".parse::<Ipv6Addr>().unwrap().into(),
        "ff02::1".parse::<Ipv6Addr>().unwrap().into(),
        "::ffff:169.254.169.254".parse::<Ipv6Addr>().unwrap().into(),
        "::169.254.169.254".parse::<Ipv6Addr>().unwrap().into(),
        "64:ff9b::a9fe:a9fe".parse::<Ipv6Addr>().unwrap().into(),
    ];
    for ip in bad {
        assert!(refused(*ip, false).is_some(), "{ip} should be refused");
        assert!(
            refused(*ip, true).is_some(),
            "{ip} should be refused even with allow_local"
        );
    }
    let local: &[IpAddr] = &[
        Ipv4Addr::LOCALHOST.into(),
        Ipv4Addr::new(127, 9, 9, 9).into(),
        Ipv6Addr::LOCALHOST.into(),
        "::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap().into(),
    ];
    for ip in local {
        assert!(refused(*ip, false).is_some(), "{ip} is loopback");
        assert_eq!(refused(*ip, true), None, "{ip} allowed with allow_local");
    }
    let ok: &[IpAddr] = &[
        Ipv4Addr::new(192, 168, 3, 10).into(),
        Ipv4Addr::new(10, 0, 0, 5).into(),
        Ipv4Addr::new(172, 16, 0, 1).into(),
        Ipv4Addr::new(8, 8, 8, 8).into(),
        "fd12::5".parse::<Ipv6Addr>().unwrap().into(),
        "2606:4700::1111".parse::<Ipv6Addr>().unwrap().into(),
    ];
    for ip in ok {
        assert_eq!(refused(*ip, false), None, "{ip} is a plant or internet address");
    }
}

#[test]
fn a_name_resolving_to_any_refused_address_is_refused() {
    let u = Url::parse("http://mes.plant:8080/x").unwrap();
    let rebind = |_: &str, port: u16| {
        Ok(vec![
            SocketAddr::new(Ipv4Addr::new(10, 0, 0, 5).into(), port),
            SocketAddr::new(Ipv4Addr::new(169, 254, 169, 254).into(), port),
        ])
    };
    match http::addresses(&u, true, &rebind) {
        Err(SendError::Refused { addr, .. }) => assert_eq!(addr, IpAddr::from([169, 254, 169, 254])),
        other => panic!("{other:?}"),
    }
    let fine = |_: &str, port: u16| Ok(vec![SocketAddr::new(Ipv4Addr::new(10, 0, 0, 5).into(), port)]);
    assert_eq!(http::addresses(&u, false, &fine).unwrap().len(), 1);
    let none = |_: &str, _: u16| Ok(vec![]);
    assert_eq!(http::addresses(&u, false, &none), Err(SendError::NoAddress));
    let fail = |_: &str, _: u16| Err(io::Error::from(io::ErrorKind::NotFound));
    assert!(matches!(
        http::addresses(&u, false, &fail),
        Err(SendError::Resolve(_))
    ));
}

#[test]
fn localhost_names_are_refused_before_any_lookup() {
    for u in [
        "http://localhost:9/",
        "http://LOCALHOST/",
        "http://api.localhost/",
    ] {
        let u = Url::parse(u).unwrap();
        assert_eq!(http::addresses(&u, false, &no_dns), Err(SendError::Localhost));
    }
}

// ---------------------------------------------------------------- HTTP

#[test]
fn a_post_carries_the_key_headers_and_body() {
    let srv = serve(|_, _| Reply::Status(204));
    let url = Url::parse(&srv.url("/hooks/line3?x=1")).unwrap();
    let headers = vec![
        ("Authorization".to_string(), Secret::new("Bearer abc".into())),
        ("X-Line".to_string(), Secret::new("3".into())),
    ];
    let req = Request {
        url: &url,
        headers: &headers,
        key: "mes-1-7",
        body: br#"{"a":1}"#,
    };
    assert_eq!(
        http::post(&req, Duration::from_secs(2), true, &no_dns),
        Outcome::Delivered(204)
    );
    let r = &srv.reqs()[0];
    assert_eq!(r.request_line(), "POST /hooks/line3?x=1 HTTP/1.1");
    assert_eq!(r.header("host").unwrap(), srv.addr.to_string());
    assert_eq!(r.header("idempotency-key").as_deref(), Some("mes-1-7"));
    assert_eq!(r.header("authorization").as_deref(), Some("Bearer abc"));
    assert_eq!(r.header("x-line").as_deref(), Some("3"));
    assert_eq!(r.header("content-type").as_deref(), Some("application/json"));
    assert_eq!(r.header("connection").as_deref(), Some("close"));
    assert_eq!(r.body, r#"{"a":1}"#);
}

#[test]
fn loopback_is_refused_without_allow_local_and_nothing_connects() {
    let srv = serve(|_, _| Reply::Status(200));
    let url = Url::parse(&srv.url("/")).unwrap();
    assert!(matches!(
        post(&url, Duration::from_secs(1), false),
        Outcome::Retry(SendError::Refused { .. })
    ));
    assert_eq!(srv.conns(), 0);
}

#[test]
fn a_receiver_that_never_answers_times_out_on_time() {
    let srv = serve(|_, _| Reply::Hang(Duration::from_secs(10)));
    let url = Url::parse(&srv.url("/")).unwrap();
    let t = Instant::now();
    assert_eq!(
        post(&url, Duration::from_millis(300), true),
        Outcome::Retry(SendError::Timeout)
    );
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

#[test]
fn a_redirect_to_another_host_is_never_followed() {
    let other = serve(|_, _| Reply::Status(200));
    for code in [301, 302, 303, 307, 308] {
        let loc = other.url("/stolen");
        let srv = serve(move |_, _| {
            Reply::Raw(
                format!("HTTP/1.1 {code} Moved\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n").into_bytes(),
            )
        });
        let url = Url::parse(&srv.url("/")).unwrap();
        assert_eq!(
            post(&url, Duration::from_secs(2), true),
            Outcome::Retry(SendError::Redirect(code))
        );
    }
    assert_eq!(other.conns(), 0, "the redirect target was contacted");
}

#[test]
fn a_huge_body_is_never_read() {
    let srv = serve(|_, _| Reply::Endless {
        head: b"HTTP/1.1 200 OK\r\nContent-Length: 999999999999\r\n\r\n".to_vec(),
        filler: vec![b'x'; 64 * 1024],
    });
    let url = Url::parse(&srv.url("/")).unwrap();
    let t = Instant::now();
    assert_eq!(post(&url, Duration::from_secs(5), true), Outcome::Delivered(200));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn an_endless_response_head_is_cut_at_the_cap() {
    let srv = serve(|_, _| Reply::Endless {
        head: b"HTTP/1.1 200 OK\r\nX-Pad: ".to_vec(),
        filler: vec![b'a'; 4096],
    });
    let url = Url::parse(&srv.url("/")).unwrap();
    let t = Instant::now();
    assert!(matches!(
        post(&url, Duration::from_secs(5), true),
        Outcome::Retry(SendError::Protocol(_))
    ));
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn statuses_and_garbage_are_classified() {
    let cases: &[(&[u8], Outcome)] = &[
        (b"HTTP/1.1 200 OK\r\n\r\n", Outcome::Delivered(200)),
        (b"HTTP/1.0 202 Accepted\r\n\r\n", Outcome::Delivered(202)),
        (b"HTTP/1.1 400 Bad\r\n\r\n", Outcome::Rejected(400)),
        (b"HTTP/1.1 413 Big\r\n\r\n", Outcome::Rejected(413)),
        (b"HTTP/1.1 422 No\r\n\r\n", Outcome::Rejected(422)),
        (
            b"HTTP/1.1 401 Who\r\n\r\n",
            Outcome::Retry(SendError::Status(401)),
        ),
        (
            b"HTTP/1.1 404 Gone\r\n\r\n",
            Outcome::Retry(SendError::Status(404)),
        ),
        (
            b"HTTP/1.1 429 Slow\r\n\r\n",
            Outcome::Retry(SendError::Status(429)),
        ),
        (
            b"HTTP/1.1 503 Busy\r\n\r\n",
            Outcome::Retry(SendError::Status(503)),
        ),
        // A head cut off before its blank line still has a status.
        (b"HTTP/1.1 200 OK\r\n", Outcome::Delivered(200)),
        (
            b"HELLO\r\n\r\n",
            Outcome::Retry(SendError::Protocol("no HTTP status line")),
        ),
        (
            b"HTTP/1.1 2000 OK\r\n\r\n",
            Outcome::Retry(SendError::Protocol("no HTTP status line")),
        ),
        (
            b"HTTP/1.1 099 Low\r\n\r\n",
            Outcome::Retry(SendError::Protocol("no HTTP status line")),
        ),
        (
            b"HTTP/2 200\r\n\r\n",
            Outcome::Retry(SendError::Protocol("no HTTP status line")),
        ),
        (
            b"",
            Outcome::Retry(SendError::Protocol("connection closed without a response")),
        ),
    ];
    for (raw, want) in cases {
        let bytes = raw.to_vec();
        let srv = serve(move |_, _| Reply::Raw(bytes.clone()));
        let url = Url::parse(&srv.url("/")).unwrap();
        assert_eq!(&post(&url, Duration::from_secs(2), true), want, "{raw:?}");
    }
}

#[test]
fn a_closed_port_is_a_connect_error() {
    let url = Url::parse(&format!("http://127.0.0.1:{}/", closed_port())).unwrap();
    assert!(matches!(
        post(&url, Duration::from_secs(2), true),
        Outcome::Retry(SendError::Connect(_))
    ));
}

#[cfg(feature = "https")]
#[test]
fn https_to_a_plain_http_server_fails_as_tls() {
    // Answers plain HTTP the moment a client connects, whatever it sent.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            thread::sleep(Duration::from_millis(200));
        }
    });
    let url = Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap();
    match post(&url, Duration::from_secs(2), true) {
        Outcome::Retry(SendError::Tls(_)) => {}
        other => panic!("{other:?}"),
    }
}

#[cfg(not(feature = "https"))]
#[test]
fn https_without_the_feature_is_refused_at_load() {
    let f = parse("sink s https://h/x\n").unwrap();
    let errs = f.resolve(&|_| None, &|_| Ok(String::new())).unwrap_err();
    assert!(errs[0].contains("https feature"), "{errs:?}");
}

// ---------------------------------------------------------------- the worker

#[test]
fn backoff_doubles_within_jitter_and_caps() {
    let b = Backoff {
        min: Duration::from_millis(100),
        max: Duration::from_millis(1000),
    };
    let mut rng = helmsman_plc::sink::rng::Rng::new(7);
    for (attempt, cap) in [
        (0, 100),
        (1, 200),
        (2, 400),
        (3, 800),
        (4, 1000),
        (30, 1000),
        (200, 1000),
    ] {
        for _ in 0..50 {
            let d = b.delay(attempt, &mut rng).as_millis();
            assert!(
                (cap / 2..=cap).contains(&d),
                "attempt {attempt}: {d} not in {}..={cap}",
                cap / 2
            );
        }
    }
    // Seeded: the same seed gives the same waits.
    let run = |seed| {
        let mut r = helmsman_plc::sink::rng::Rng::new(seed);
        (0..10).map(|a| b.delay(a, &mut r)).collect::<Vec<_>>()
    };
    assert_eq!(run(42), run(42));
    assert_ne!(run(42), run(43));
}

#[test]
fn fails_three_times_then_delivers_with_one_key() {
    let srv = serve(|n, _| Reply::Status(if n < 3 { 503 } else { 200 }));
    let dir = tmpdir("retry");
    let logs = Logs::default();
    let h = worker::spawn(cfg("mes", &srv.url("/hook")), &dir, 1, logs.log()).unwrap();
    assert!(h.offer(ev("parts", Value::Int(42))));
    // The worker counts the delivery, then says "up": wait for both.
    wait_until("delivery", || {
        Stats::get(&h.stats().delivered) == 1 && logs.all().contains("up;")
    });
    let reqs = srv.reqs();
    assert_eq!(reqs.len(), 4);
    let keys: Vec<_> = reqs
        .iter()
        .map(|r| r.header("idempotency-key").unwrap())
        .collect();
    assert!(keys.iter().all(|k| k == "mes-1-0"), "{keys:?}");
    assert_eq!(
        reqs[3].body,
        r#"{"key":"mes-1-0","boot":1,"seq":0,"signal":"parts","value":42,"trigger":"change","ts_ms":1793404800000}"#
    );
    assert_eq!(Stats::get(&h.stats().retries), 3);
    assert_eq!(Stats::get(&h.stats().queued), 0);
    let log = logs.all();
    assert!(log.contains("down: status 503") && log.contains("up;"), "{log}");
    h.stop();
}

#[test]
fn events_are_delivered_in_order() {
    let srv = serve(|n, _| Reply::Status(if n % 2 == 0 { 500 } else { 200 }));
    let dir = tmpdir("order");
    let h = worker::spawn(cfg("s", &srv.url("/")), &dir, 2, Logs::default().log()).unwrap();
    for i in 0..10 {
        assert!(h.offer(ev("n", Value::Int(i))));
    }
    wait_until("ten deliveries", || Stats::get(&h.stats().delivered) == 10);
    let delivered: Vec<String> = srv
        .reqs()
        .iter()
        .skip(1)
        .step_by(2)
        .map(|r| r.header("idempotency-key").unwrap())
        .collect();
    let want: Vec<String> = (0..10).map(|i| format!("s-1-{i}")).collect();
    assert_eq!(delivered, want);
    h.stop();
}

#[test]
fn the_queue_survives_a_restart_and_keys_never_repeat() {
    let dir = tmpdir("restart");
    let down = cfg("mes", &format!("http://127.0.0.1:{}/", closed_port()));
    let h = worker::spawn(down, &dir, 3, Logs::default().log()).unwrap();
    for i in 0..3 {
        assert!(h.offer(ev("parts", Value::Int(i))));
    }
    wait_until("three queued", || Stats::get(&h.stats().queued) == 3);
    h.stop();

    // The box comes back; so does the receiver.
    let srv = serve(|_, _| Reply::Status(200));
    let logs = Logs::default();
    let h = worker::spawn(cfg("mes", &srv.url("/")), &dir, 4, logs.log()).unwrap();
    wait_until("replay", || Stats::get(&h.stats().delivered) == 3);
    assert!(h.offer(ev("parts", Value::Int(3))));
    wait_until("the new one", || Stats::get(&h.stats().delivered) == 4);
    let keys: Vec<_> = srv
        .reqs()
        .iter()
        .map(|r| r.header("idempotency-key").unwrap())
        .collect();
    assert_eq!(keys, ["mes-1-0", "mes-1-1", "mes-1-2", "mes-2-0"]);
    assert!(logs.all().contains("boot 2, 3 event(s) queued"), "{}", logs.all());
    h.stop();
}

#[test]
fn a_full_queue_drops_the_oldest_and_counts() {
    let srv = serve(|_, _| Reply::Status(503));
    let dir = tmpdir("full");
    let mut c = cfg("s", &srv.url("/"));
    c.queue_bytes = 4096;
    c.backoff = Backoff {
        min: Duration::from_secs(60),
        max: Duration::from_secs(60),
    };
    let logs = Logs::default();
    let h = worker::spawn(c, &dir, 5, logs.log()).unwrap();
    for i in 0..100 {
        assert!(h.offer(ev("n", Value::Int(i))));
    }
    let s = h.stats();
    wait_until("all accounted", || {
        Stats::get(&s.queued) + Stats::get(&s.dropped) == 100
    });
    let (queued, dropped) = (Stats::get(&s.queued), Stats::get(&s.dropped));
    assert!(dropped > 0 && queued > 0, "queued {queued} dropped {dropped}");
    assert!(logs.all().contains("queue full"), "{}", logs.all());
    h.stop();
    let (q, _) = Queue::open(&worker::paths(&dir, "s").0, 4096).unwrap();
    assert!(q.bytes() <= 4096);
    let keys: Vec<_> = q.iter().map(|e| e.key.clone()).collect();
    assert_eq!(
        keys.last().map(String::as_str),
        Some("s-1-99"),
        "the newest is kept"
    );
    let first: u64 = keys[0].rsplit('-').next().unwrap().parse().unwrap();
    assert_eq!(first, dropped, "exactly the oldest went: {keys:?}");
}

#[test]
fn a_rejected_event_is_dropped_and_the_next_one_goes() {
    let srv = serve(|n, _| Reply::Status(if n == 0 { 422 } else { 200 }));
    let dir = tmpdir("reject");
    let logs = Logs::default();
    let h = worker::spawn(cfg("s", &srv.url("/")), &dir, 6, logs.log()).unwrap();
    assert!(h.offer(ev("a", Value::Bool(true))));
    assert!(h.offer(ev("a", Value::Bool(false))));
    wait_until("one delivered", || Stats::get(&h.stats().delivered) == 1);
    assert_eq!(Stats::get(&h.stats().rejected), 1);
    assert_eq!(srv.reqs().len(), 2);
    assert!(
        logs.all().contains("event s-1-0 rejected with status 422"),
        "{}",
        logs.all()
    );
    h.stop();
}

#[test]
fn offering_never_blocks_the_scan() {
    // The receiver hangs, so the sink thread is stuck in an attempt.
    let srv = serve(|_, _| Reply::Hang(Duration::from_secs(10)));
    let dir = tmpdir("noblock");
    let mut c = cfg("s", &srv.url("/"));
    c.timeout = Duration::from_secs(3);
    let h = worker::spawn(c, &dir, 7, Logs::default().log()).unwrap();
    assert!(h.offer(ev("n", Value::Int(0))));
    wait_until("the attempt", || srv.conns() == 1);
    let t = Instant::now();
    let mut refused = 0;
    for i in 0..5000 {
        if !h.offer(ev("n", Value::Int(i))) {
            refused += 1;
        }
    }
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    assert!(refused > 0);
    assert_eq!(Stats::get(&h.stats().dropped_channel), refused);
}

const SECRET: &str = "s3cr3t-T0KEN-value";

#[test]
fn secrets_never_reach_a_log_or_an_error() {
    let dir = tmpdir("secret");
    let key_file = dir.join("token");
    std::fs::write(&key_file, format!("Bearer {SECRET}\n")).unwrap();
    let hang = serve(|_, _| Reply::Hang(Duration::from_secs(5)));
    let other = serve(|_, _| Reply::Status(200));
    let to = other.url(&format!("/{SECRET}"));
    let redirect =
        serve(move |_, _| Reply::Raw(format!("HTTP/1.1 302 X\r\nLocation: {to}\r\n\r\n").into_bytes()));
    let deny = serve(|_, _| Reply::Raw(format!("HTTP/1.1 401 {SECRET}\r\n\r\n{SECRET}").into_bytes()));
    let receivers = [
        format!("http://127.0.0.1:{}/hooks/{SECRET}", closed_port()),
        hang.url(&format!("/hooks/{SECRET}")),
        redirect.url(&format!("/hooks/{SECRET}")),
        deny.url(&format!("/hooks/{SECRET}?k={SECRET}")),
    ];
    let logs = Logs::default();
    for (i, url) in receivers.iter().enumerate() {
        let src = format!(
            "sink s{i} {url} timeout_ms=200 retry_min_ms=5 retry_max_ms=20 {}\n\
             header s{i} Authorization file:{}\n\
             header s{i} X-Api-Key env:API_KEY\n",
            "allow_local=true",
            key_file.display()
        );
        let file = parse(&src).unwrap();
        assert!(!format!("{file:?}").contains(SECRET));
        let env = |k: &str| (k == "API_KEY").then(|| SECRET.to_string());
        let mut sinks = file.resolve(&env, &config::read_header_file).unwrap();
        let c = sinks.remove(0);
        assert!(!format!("{c:?}").contains(SECRET), "{c:?}");
        let h = worker::spawn(c, &dir, 8, logs.log()).unwrap();
        assert!(h.offer(ev("n", Value::Int(1))));
        wait_until("two attempts", || Stats::get(&h.stats().retries) >= 2);
        h.stop();
    }
    let all = logs.all();
    assert!(all.contains("down:"), "{all}");
    assert!(!all.contains(SECRET), "a secret was logged:\n{all}");
    assert!(!all.contains("hooks"), "a URL path was logged:\n{all}");
    assert_eq!(other.conns(), 0);

    // Config errors name the problem, never the value.
    let bad = [
        format!("sink s http://h/{SECRET} {SECRET}=1\n"),
        format!("sink s ftp://h/{SECRET}\n"),
        format!("sink s http://u:{SECRET}@h/\n"),
        format!("sink s http://h/\nheader s Authorization text:{SECRET}\n"),
        format!("sink s http://h/\nheader s X-Token text:{SECRET}\n"),
        format!("sink s http://h/\nheader s Cookie text:{SECRET}\n"),
        format!("sink s http://h/\nheader s X-Line text:{SECRET} more\n"),
        format!("{SECRET} s http://h/\n"),
        format!("sink s http://h/\nheader s Host text:{SECRET}\n"),
    ];
    for src in &bad {
        let errs = parse(src).unwrap_err();
        let shown = format!("{errs:?}");
        assert!(!shown.contains(SECRET), "{src:?} -> {shown}");
    }
    // A refused address at load names the address, not the path.
    let f = parse(&format!("sink s http://169.254.169.254/{SECRET}\n")).unwrap();
    let errs = f.resolve(&|_| None, &config::read_header_file).unwrap_err();
    assert!(
        errs[0].contains("169.254.169.254 refused") && !errs[0].contains(SECRET),
        "{errs:?}"
    );
    // A loaded value that a header cannot carry is refused, unshown.
    let f = parse("sink s http://h/\nheader s Authorization env:T\n").unwrap();
    let env = |_: &str| Some(format!("{SECRET}\r\nX-Evil: 1"));
    let errs = f.resolve(&env, &config::read_header_file).unwrap_err();
    assert!(!format!("{errs:?}").contains(SECRET), "{errs:?}");
}

// ---------------------------------------------------------------- config

#[test]
fn a_sinks_file_parses_with_defaults() {
    let f = parse(
        "# plant\n\
         sink mes https://mes.example.com/hooks/line3 timeout_ms=2500 queue_bytes=65536\n\
         header mes Authorization file:/etc/helmsman-plc/mes.token\n\
         header mes X-Line text:3\n\
         sink lab http://10.0.0.5:8080/ retry_min_ms=50 retry_max_ms=500 allow_local=false\n\
         emit mes parts change   # counts\n\
         emit mes jam rise\n\
         emit lab level every_ms=60000\n",
    )
    .unwrap();
    assert_eq!(f.sinks.len(), 2);
    let mes = &f.sinks[0];
    assert_eq!(mes.timeout, Duration::from_millis(2500));
    assert_eq!(mes.queue_bytes, 65536);
    assert_eq!(mes.backoff.min, Duration::from_secs(1));
    assert_eq!(mes.backoff.max, Duration::from_secs(300));
    assert!(!mes.allow_local);
    assert_eq!(mes.headers.len(), 2);
    let lab = &f.sinks[1];
    assert_eq!(lab.timeout, Duration::from_secs(5));
    assert_eq!(lab.queue_bytes, 1 << 20);
    assert_eq!(lab.backoff.max, Duration::from_millis(500));
    let triggers: Vec<_> = f.emits.iter().map(|e| e.trigger).collect();
    assert_eq!(
        triggers,
        [
            Trigger::Change,
            Trigger::Rise,
            Trigger::Every(Duration::from_secs(60))
        ]
    );
}

#[test]
fn bad_sinks_files_name_every_bad_line() {
    let src = "\
sink 9bad http://h/
sink s http://h/ timeout_ms=0
sink s http://h/ queue_bytes=10
sink s http://h/ retry_min_ms=500 retry_max_ms=100
sink s http://h/ allow_local=yes
sink s http://h/ timeout_ms=5 timeout_ms=6
sink t http://h/
sink t http://h/
header nope X-A text:1
header t Bad\\Name text:1
header t Content-Length text:1
header t X-A text:1
header t x-a text:2
header t X-B bare
header t X-C env:
emit t sig sometimes
emit t sig every_ms=10
emit t 1sig change
emit t sig
bogus line here
";
    let errs = parse(src).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        lines,
        [1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20],
        "{errs:#?}"
    );
}

#[test]
fn header_values_load_from_env_and_files() {
    let dir = tmpdir("headers");
    let p = dir.join("tok");
    std::fs::write(&p, "Bearer abc \n\n").unwrap();
    let big = dir.join("big");
    std::fs::write(&big, "x".repeat(9000)).unwrap();
    let src = format!(
        "sink s http://h/\nheader s Authorization file:{}\nheader s X-Key env:K\n",
        p.display()
    );
    let f = parse(&src).unwrap();
    let env = |k: &str| (k == "K").then(|| "v1".to_string());
    let c = f.resolve(&env, &config::read_header_file).unwrap();
    let got: Vec<(&str, &str)> = c[0]
        .headers
        .iter()
        .map(|(n, v)| (n.as_str(), v.expose()))
        .collect();
    assert_eq!(got, [("Authorization", "Bearer abc"), ("X-Key", "v1")]);
    // Missing env, missing file, an oversized file.
    let src = format!(
        "sink s http://h/\nheader s A-Key env:NOPE\nheader s B-Key file:{}\nheader s C-Key file:{}\n",
        dir.join("missing").display(),
        big.display()
    );
    let errs = parse(&src)
        .unwrap()
        .resolve(&|_| None, &config::read_header_file)
        .unwrap_err();
    assert_eq!(errs.len(), 3, "{errs:?}");
}

// ---------------------------------------------------------------- emits

fn table() -> SignalTable {
    let mut t = SignalTable::default();
    t.add_box_input("jam", Ty::Bool).unwrap();
    t.add_box_input("parts", Ty::Int).unwrap();
    t.add_box_input("level", Ty::Real).unwrap();
    t
}

fn emitter(lines: &str, t: &SignalTable) -> Emitter {
    let f = parse(&format!("sink s http://h/\n{lines}")).unwrap();
    Emitter::bind(&f.emits, &["s"], t).unwrap()
}

fn set(t: &mut SignalTable, name: &str, v: Option<Value>) {
    let i = t.index(name).unwrap();
    t.set_input(i, v);
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn rise_and_fall_fire_on_edges_across_missing_values() {
    let mut t = table();
    let mut e = emitter("emit s jam rise\n", &t);
    let mut fired = |t: &SignalTable| {
        e.observe(t, ms(0))
            .into_iter()
            .map(|f| f.value)
            .collect::<Vec<_>>()
    };
    assert_eq!(fired(&t).len(), 0, "nothing has a value");
    set(&mut t, "jam", Some(Value::Bool(true)));
    assert_eq!(fired(&t).len(), 0, "the first value is a baseline");
    set(&mut t, "jam", Some(Value::Bool(false)));
    assert_eq!(fired(&t).len(), 0, "a fall is not a rise");
    set(&mut t, "jam", Some(Value::Bool(true)));
    assert_eq!(fired(&t), [Value::Bool(true)]);
    assert_eq!(fired(&t).len(), 0, "held high is not another rise");
    set(&mut t, "jam", Some(Value::Bool(false)));
    fired(&t);
    set(&mut t, "jam", None);
    assert_eq!(fired(&t).len(), 0, "missing never fires");
    set(&mut t, "jam", Some(Value::Bool(true)));
    assert_eq!(fired(&t), [Value::Bool(true)], "false, missing, true is a rise");
    set(&mut t, "jam", None);
    fired(&t);
    set(&mut t, "jam", Some(Value::Bool(true)));
    assert_eq!(fired(&t).len(), 0, "true, missing, true is not");

    let mut e = emitter("emit s jam fall\n", &t);
    set(&mut t, "jam", Some(Value::Bool(true)));
    assert_eq!(e.observe(&t, ms(0)).len(), 0);
    set(&mut t, "jam", Some(Value::Bool(false)));
    assert_eq!(e.observe(&t, ms(0)).len(), 1);
}

#[test]
fn change_fires_on_any_new_value_after_the_baseline() {
    let mut t = table();
    let mut e = emitter("emit s parts change\nemit s level change\n", &t);
    set(&mut t, "parts", Some(Value::Int(5)));
    set(&mut t, "level", Some(Value::Real(0.0)));
    assert_eq!(e.observe(&t, ms(0)).len(), 0);
    set(&mut t, "parts", Some(Value::Int(6)));
    let f = e.observe(&t, ms(0));
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].signal.as_str(), f[0].value, f[0].trigger),
        ("parts", Value::Int(6), Trigger::Change)
    );
    assert_eq!(e.observe(&t, ms(0)).len(), 0);
    set(&mut t, "level", Some(Value::Real(-0.0)));
    assert_eq!(
        e.observe(&t, ms(0)).len(),
        1,
        "a real fires on any change of bits"
    );
    set(&mut t, "parts", None);
    assert_eq!(e.observe(&t, ms(0)).len(), 0);
    set(&mut t, "parts", Some(Value::Int(6)));
    assert_eq!(
        e.observe(&t, ms(0)).len(),
        0,
        "back to the same value is no change"
    );
}

#[test]
fn every_fires_on_its_period_and_skips_missed_ones() {
    let mut t = table();
    let mut e = emitter("emit s level every_ms=100\n", &t);
    assert_eq!(e.observe(&t, ms(0)).len(), 0, "no value, no event");
    set(&mut t, "level", Some(Value::Real(1.5)));
    let at = |e: &mut Emitter, t: &SignalTable, n| e.observe(t, ms(n)).len();
    assert_eq!(at(&mut e, &t, 10), 1, "the first value fires");
    assert_eq!(at(&mut e, &t, 50), 0);
    assert_eq!(at(&mut e, &t, 109), 0);
    assert_eq!(at(&mut e, &t, 110), 1);
    assert_eq!(at(&mut e, &t, 150), 0);
    // A long stall: one event, not a burst, then back on the grid.
    assert_eq!(at(&mut e, &t, 555), 1);
    assert_eq!(at(&mut e, &t, 556), 0);
    assert_eq!(at(&mut e, &t, 609), 0);
    assert_eq!(at(&mut e, &t, 610), 1);
    set(&mut t, "level", None);
    assert_eq!(at(&mut e, &t, 710), 0, "missing skips the period");
    set(&mut t, "level", Some(Value::Real(2.0)));
    assert_eq!(at(&mut e, &t, 715), 1);
}

#[test]
fn emits_that_do_not_fit_the_table_are_refused() {
    let t = table();
    let f = parse(
        "sink s http://h/\n\
         emit s parts rise\n\
         emit s level fall\n\
         emit s nope change\n\
         emit other jam rise\n\
         emit s jam rise\n\
         emit s JAM change\n",
    )
    .unwrap();
    let errs = Emitter::bind(&f.emits, &["s"], &t).unwrap_err();
    assert_eq!(errs.len(), 5, "{errs:#?}");
    assert!(
        errs[0].starts_with("line 2:") && errs[4].starts_with("line 7:"),
        "{errs:#?}"
    );
}

#[test]
fn sinks_publish_what_fires_end_to_end() {
    let srv = serve(|_, _| Reply::Status(200));
    let dir = tmpdir("e2e");
    let mut t = table();
    let src = format!(
        "sink mes {} allow_local=true\nheader mes X-Line text:3\nemit mes jam rise\nemit mes parts change\n",
        srv.url("/in")
    );
    let logs = Logs::default();
    let env = |_: &str| None;
    let mut s = Sinks::start(&src, &dir, &t, &env, &config::read_header_file, 9, &logs.log()).unwrap();
    set(&mut t, "jam", Some(Value::Bool(false)));
    set(&mut t, "parts", Some(Value::Int(0)));
    s.observe(&t, ms(0), Some(1));
    set(&mut t, "jam", Some(Value::Bool(true)));
    set(&mut t, "parts", Some(Value::Int(1)));
    s.observe(&t, ms(10), None);
    wait_until("two deliveries", || srv.reqs().len() == 2);
    let bodies: Vec<String> = srv.reqs().iter().map(|r| r.body.clone()).collect();
    assert_eq!(
        bodies,
        [
            r#"{"key":"mes-1-0","boot":1,"seq":0,"signal":"jam","value":true,"trigger":"rise","ts_ms":null}"#,
            r#"{"key":"mes-1-1","boot":1,"seq":1,"signal":"parts","value":1,"trigger":"change","ts_ms":null}"#,
        ]
    );
    assert_eq!(srv.reqs()[0].header("x-line").as_deref(), Some("3"));
    let stats = s.stats();
    assert_eq!(stats[0].0, "mes");

    // A file that does not fit is refused as a whole, with every reason.
    let bad = "sink a http://h/\nemit a nope change\nemit b jam rise\n";
    let errs = Sinks::start(bad, &dir, &t, &env, &config::read_header_file, 9, &logs.log())
        .err()
        .unwrap();
    assert_eq!(errs.len(), 2, "{errs:?}");
}

#[test]
fn wall_clock_before_2024_is_untrusted() {
    use helmsman_plc::sink::wall_ms;
    use std::time::{SystemTime, UNIX_EPOCH};
    assert_eq!(wall_ms(UNIX_EPOCH + Duration::from_secs(1_000)), None);
    assert_eq!(wall_ms(UNIX_EPOCH + Duration::from_secs(1_704_067_199)), None);
    assert_eq!(
        wall_ms(UNIX_EPOCH + Duration::from_secs(1_704_067_200)),
        Some(1_704_067_200_000)
    );
    assert!(wall_ms(SystemTime::now()).is_some());
}
