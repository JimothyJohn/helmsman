//! The whole PLC in-process on loopback: adapter + scan engine + web API,
//! driven by this repo's scanner and a raw HTTP client.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use proptest::prelude::*;
use tcpethernet::scanner::{IoConfig, IoConnection};
use tcpethernet::session::Session;
use tcpethernet_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use tcpethernet_plc::engine::{Engine, EngineConfig};
use tcpethernet_plc::lang::{self, Scan, Sizes};
use tcpethernet_plc::web::Web;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Stack {
    adapter: Arc<Adapter>,
    engine: Arc<Engine>,
    web: Web,
    scanner_sock: UdpSocket,
}

fn stack() -> Stack {
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let adapter = Arc::new(
        Adapter::start(
            Device::new(DeviceConfig::opener_sample()),
            &ServerConfig {
                bind: Ipv4Addr::LOCALHOST,
                encap_port: 0,
                io_port: 0,
                peer_io_port: scanner_sock.local_addr().unwrap().port(),
                ..ServerConfig::default()
            },
        )
        .unwrap(),
    );
    let engine = Arc::new(
        Engine::start(
            adapter.device(),
            EngineConfig {
                input_assembly: 150,
                output_assembly: 100,
                memory: 16,
                period: Duration::from_millis(2),
                program_file: None,
            },
        )
        .unwrap(),
    );
    let a = Arc::clone(&adapter);
    let web = Web::start(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Arc::clone(&engine),
        TOKEN.into(),
        Arc::new(move || a.sessions()),
    )
    .unwrap();
    Stack {
        adapter,
        engine,
        web,
        scanner_sock,
    }
}

fn http(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let code = out[9..12].parse().unwrap();
    let body = out
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (code, body)
}

#[test]
fn scanner_output_goes_through_the_program_and_back() {
    let st = stack();
    let mut s = Session::connect(st.adapter.encap_addr(), Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(5));
    let conn = IoConnection::open(&mut s, &st.scanner_sock, st.adapter.io_addr(), cfg).unwrap();

    // Default program echoes I to Q.
    let pattern: Vec<u8> = (1..=32).collect();
    conn.set_output(&pattern).unwrap();
    assert!(
        conn.wait_for_input_eq(&pattern, Duration::from_secs(3)).is_some(),
        "echo program did not echo"
    );

    // Swap the program over HTTP: Q[0] = I[0] * 2, Q[1] = RUN, rest zero.
    let (code, body) = http(
        st.web.addr(),
        "PUT",
        "/api/program",
        Some(TOKEN),
        "Q[0] = I[0] * 2\nQ[1] = RUN\nQ[2..32] = 0\n",
    );
    assert_eq!(code, 200, "{body}");
    let mut want = vec![0u8; 32];
    want[0] = 2;
    want[1] = 1;
    let got = conn.wait_for_input_eq(&want, Duration::from_secs(3));
    assert_eq!(
        got.map(|i| i.data),
        Some(want),
        "new program not running or RUN not seen"
    );

    // Idle header -> RUN drops to 0.
    conn.set_run(false);
    let mut idle = vec![0u8; 32];
    idle[0] = 2;
    assert!(
        conn.wait_for_input_eq(&idle, Duration::from_secs(3)).is_some(),
        "RUN did not follow the idle header"
    );

    conn.close(&mut s).unwrap();
    s.close();
    assert!(st.engine.state().stats.scans > 10);
}

#[test]
fn web_api_contract() {
    let st = stack();
    let a = st.web.addr();
    assert_eq!(http(a, "GET", "/", None, "").0, 200);
    assert_eq!(http(a, "GET", "/app.js", None, "").0, 200);
    assert_eq!(http(a, "GET", "/api/state", None, "").0, 401);
    assert_eq!(
        http(
            a,
            "GET",
            "/api/state",
            Some("wrong-token-wrong-token-wrong-tok"),
            ""
        )
        .0,
        401
    );
    assert_eq!(http(a, "GET", "/nope", None, "").0, 404);

    let (code, body) = http(a, "GET", "/api/state", Some(TOKEN), "");
    assert_eq!(code, 200);
    for key in [
        r#""identity":"#,
        r#""name":"OpENer PC""#,
        r#""i":["#,
        r#""connections":[]"#,
        r#""scan":"#,
    ] {
        assert!(body.contains(key), "{key} missing from {body}");
    }

    let (code, body) = http(
        a,
        "PUT",
        "/api/program",
        Some(TOKEN),
        "Q[0] = 1\nI[0] = 2\nQ[99] = 1\n",
    );
    assert_eq!(code, 422);
    assert!(
        body.contains(r#""line":2"#) && body.contains(r#""line":3"#),
        "{body}"
    );
    // The rejected program did not replace the running one.
    let (_, src) = http(a, "GET", "/api/program", Some(TOKEN), "");
    assert!(src.contains("Q[0..32] = I[0..32]"), "{src}");

    assert_eq!(http(a, "PUT", "/api/memory/3", Some(TOKEN), "200").0, 200);
    assert_eq!(st.engine.state().memory[3], 200);
    assert_eq!(http(a, "PUT", "/api/memory/99", Some(TOKEN), "1").0, 422);
    assert_eq!(http(a, "PUT", "/api/memory/3", Some(TOKEN), "300").0, 400);
    assert_eq!(http(a, "DELETE", "/api/state", Some(TOKEN), "").0, 405);
}

#[test]
fn hostile_http_is_bounded() {
    let st = stack();
    let a = st.web.addr();
    // Oversized body declared: refused from the headers, before any body is read.
    let mut s = TcpStream::connect(a).unwrap();
    write!(
        s,
        "PUT /api/program HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 70000\r\n\r\n"
    )
    .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");
    // Garbage request line.
    let mut s = TcpStream::connect(a).unwrap();
    s.write_all(b"\x00\x01\x02\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
    // Chunked encoding is refused rather than mis-parsed.
    let mut s = TcpStream::connect(a).unwrap();
    s.write_all(b"PUT /api/program HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
        .unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 501"), "{out}");
    // Still serving.
    assert_eq!(http(a, "GET", "/api/state", Some(TOKEN), "").0, 200);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    /// No source text crashes the compiler, and anything that compiles
    /// scans without panicking.
    #[test]
    fn any_source_compiles_or_errors_and_scans_safely(src in "[IQMWRUNSCA0-9xb\\[\\]\\.\\-+*/%&|^~!<>=?:;() \n#_]{0,200}") {
        let sizes = Sizes { i: 8, q: 8, m: 8 };
        if let Ok(p) = lang::compile(&src, sizes) {
            let (i, mut q, mut m) = ([1u8; 8], [0u8; 8], [0u8; 8]);
            let _ = p.scan(&mut Scan { i: &i, q: &mut q, m: &mut m, run: true, scan: 1, ms: 1 });
        }
    }

    #[test]
    fn arbitrary_utf8_never_panics_the_compiler(src in "\\PC{0,300}") {
        let _ = lang::compile(&src, Sizes { i: 4, q: 4, m: 4 });
    }
}
