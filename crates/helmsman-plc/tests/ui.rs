//! The canvas served by the box (`--ui DIR`), end to end: the real binary
//! with a running program, its API on loopback, and a UI directory laid
//! out the way `make canvas-web` builds it. What it serves, with which
//! types and cache headers, and that nothing outside the directory (or
//! inside it but not a UI file) can be reached however the path is
//! spelled.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use proptest::prelude::*;

const JS: &str = "helmsman-canvas-0123456789abcdef.js";
const WASM: &str = "helmsman-canvas-0123456789abcdef_bg.wasm";
const SECRET: &str = "SECRET-7f3a-do-not-serve";

const INDEX: &str = "<!doctype html><html><head>\
<script type=\"module\">import init from '/helmsman-canvas-0123456789abcdef.js';await init();</script>\
<link rel=\"modulepreload\" href=\"/helmsman-canvas-0123456789abcdef.js\"></head>\
<body><canvas id=\"canvas\"></canvas></body></html>";

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "ui-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Bytes that look like a wasm module and are bigger than one send chunk,
/// with no run of repeats, so a dropped or reordered chunk shows.
fn wasm_bytes() -> Vec<u8> {
    let mut b = b"\0asm\x01\0\0\0".to_vec();
    let mut x: u32 = 0x1234_5678;
    while b.len() < 300_000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        b.push(x.to_le_bytes()[0]);
    }
    b
}

/// `root/ui` as trunk lays it out, plus bait: a secret beside it, a
/// symlink and a dotfile into it, a file type that is not served, and a
/// directory named like a script.
fn ui_dir() -> PathBuf {
    let root = tmp("dir");
    let ui = root.join("ui");
    std::fs::create_dir_all(ui.join("sub.js")).unwrap();
    std::fs::create_dir_all(ui.join("assets")).unwrap();
    std::fs::write(ui.join("index.html"), INDEX).unwrap();
    std::fs::write(ui.join(JS), "export default async function init() {}\n").unwrap();
    std::fs::write(ui.join(WASM), wasm_bytes()).unwrap();
    std::fs::write(
        ui.join("assets/logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    std::fs::write(root.join("secret.txt"), SECRET).unwrap();
    std::fs::write(root.join("secret.js"), SECRET).unwrap();
    std::fs::write(ui.join(".env"), SECRET).unwrap();
    std::fs::write(ui.join("notes.md"), SECRET).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("secret.txt"), ui.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("secret.js"), ui.join("link.js")).unwrap();
        std::os::unix::fs::symlink(&root, ui.join("up")).unwrap();
    }
    ui
}

struct Plc {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Plc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn plc() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    c.arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st"))
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .arg("--state-dir")
        .arg(tmp("state"));
    c
}

/// The PLC with its API on a free loopback port, serving `ui` if given.
fn spawn(ui: Option<&Path>) -> Plc {
    let mut c = plc();
    c.args(["--api", "127.0.0.1:0"]);
    if let Some(ui) = ui {
        c.arg("--ui").arg(ui);
    }
    let mut child = c.stderr(Stdio::piped()).spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut addr = None;
    let mut canvas = ui.is_none();
    while addr.is_none() || !canvas {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "PLC exited");
        if let Some(rest) = line.strip_prefix("helmsman-plc: API on http://") {
            addr = rest.split('/').next().and_then(|a| a.parse().ok());
        }
        canvas |= line.starts_with("helmsman-plc: canvas on http://");
    }
    // Keep draining stderr so the PLC never blocks on a full pipe.
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = stderr.read_to_end(&mut sink);
    });
    Plc {
        child,
        addr: addr.unwrap(),
    }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn send(addr: SocketAddr, raw: &[u8]) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(raw).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let end = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no head in {:?}", String::from_utf8_lossy(&out)));
    let head = String::from_utf8_lossy(&out[..end]).into_owned();
    let status = head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    Resp {
        status,
        head,
        body: out[end + 4..].to_vec(),
    }
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    send(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: box\r\n\r\n").as_bytes(),
    )
}

#[test]
fn the_box_serves_the_canvas_beside_the_api() {
    let ui = ui_dir();
    let p = spawn(Some(&ui));

    for path in ["/", "/index.html", "/?demo"] {
        let r = get(p.addr, path);
        assert_eq!(r.status, 200, "{path}: {r:?}");
        assert_eq!(r.header("Content-Type"), Some("text/html; charset=utf-8"));
        assert_eq!(
            r.header("Cache-Control"),
            Some("no-cache"),
            "the page revalidates"
        );
        assert_eq!(r.text(), INDEX);
        assert_eq!(r.header("Content-Length"), Some(INDEX.len().to_string().as_str()));
        assert_eq!(r.header("X-Content-Type-Options"), Some("nosniff"));
        assert_eq!(r.header("X-Frame-Options"), Some("DENY"));
        assert_eq!(
            r.header("Access-Control-Allow-Origin"),
            None,
            "same origin, no CORS"
        );
        let csp = r.header("Content-Security-Policy").unwrap();
        assert!(
            csp.starts_with("default-src 'none'; script-src 'self' 'wasm-unsafe-eval' 'sha256-"),
            "{csp}"
        );
        assert!(
            csp.contains("connect-src 'self'") && csp.contains("frame-ancestors 'none'"),
            "{csp}"
        );
    }

    let r = get(p.addr, &format!("/{WASM}"));
    assert_eq!(r.status, 200, "{:?}", r.head);
    assert_eq!(r.header("Content-Type"), Some("application/wasm"));
    assert_eq!(
        r.header("Cache-Control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(
        r.body == wasm_bytes(),
        "the wasm arrives whole, {} bytes",
        r.body.len()
    );
    assert_eq!(r.header("Content-Security-Policy"), None);

    let r = get(p.addr, &format!("/{JS}"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("text/javascript; charset=utf-8"));
    let r = get(p.addr, "/assets/logo.svg");
    assert_eq!((r.status, r.header("Content-Type")), (200, Some("image/svg+xml")));
    assert_eq!(r.header("Cache-Control"), Some("no-cache"));

    // The API is untouched by it.
    let r = get(p.addr, "/api/modules");
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("Cache-Control"), Some("no-store"));
    assert!(r.text().contains("\"name\":\"echo\""), "{}", r.text());
    assert_eq!(get(p.addr, "/api/nope").status, 404);
}

#[test]
fn missing_files_and_directories_are_not_found() {
    let ui = ui_dir();
    let p = spawn(Some(&ui));
    for path in [
        "/nope.js",
        "/nope.html",
        "/assets",
        "/assets/",
        "/sub.js",
        "/assets/nope.svg",
        "/index.html/",
        "/index.html/x.js",
        "/favicon.ico",
        "/INDEX.HTML",
    ] {
        let r = get(p.addr, path);
        assert_eq!(r.status, 404, "{path}: {r:?}");
        assert_eq!(r.header("Content-Type"), Some("application/json"), "{path}");
    }
}

/// Every way of spelling a way out (or a way to a bait file) the test
/// could think of; none is served.
const ESCAPES: &[&str] = &[
    "/../secret.txt",
    "/..%2fsecret.txt",
    "/%2e%2e/secret.txt",
    "/%2E%2E/secret.txt",
    "/%2e%2e%2fsecret.txt",
    "/.%2e/secret.txt",
    "/%2e./secret.txt",
    "/..%5csecret.txt",
    "/..\\secret.txt",
    "/assets/../../secret.txt",
    "/assets/%2e%2e/%2e%2e/secret.txt",
    "/./index.html",
    "/assets/./logo.svg",
    "//secret.txt",
    "///etc/passwd",
    "/%2fetc%2fpasswd",
    "/%2Fetc/passwd",
    "/etc/passwd",
    "/secret.txt%00.js",
    "/index.html%00",
    "/%c0%ae%c0%ae/secret.txt",
    "/%c0%af",
    "/%e0%80%ae%e0%80%ae/secret.txt",
    "/%ef%bc%8e%ef%bc%8e/secret.txt",
    "/%ef%bc%8f..%ef%bc%8fsecret.txt",
    "/%u002e%u002e/secret.txt",
    "/%252e%252e/secret.txt",
    "/.env",
    "/%2eenv",
    "/notes.md",
    "/link.txt",
    "/link.js",
    "/up/secret.js",
    "/up/ui/index.html",
    "/index.html::$DATA",
    "/index.html;.js",
    "/INDEX.html",
    "/~/secret.txt",
    "/assets/logo.svg/..",
];

#[test]
fn nothing_outside_the_ui_directory_is_reachable() {
    let ui = ui_dir();
    let p = spawn(Some(&ui));
    for path in ESCAPES {
        let r = get(p.addr, path);
        assert!(matches!(r.status, 400 | 404), "{path}: {r:?}");
        assert!(!r.text().contains(SECRET), "{path} leaked: {r:?}");
        assert!(
            !r.text().contains(&ui.display().to_string()),
            "{path} names the directory"
        );
    }
}

#[test]
fn huge_paths_are_refused() {
    let ui = ui_dir();
    let p = spawn(Some(&ui));
    let long_segment = format!("/{}.js", "a".repeat(200));
    let deep = format!("{}/index.html", "/a".repeat(8));
    let near_cap = format!("/{}", "b/".repeat(1000));
    let over_cap = format!("/{}.js", "c".repeat(3000));
    for path in [&long_segment, &deep, &near_cap] {
        assert_eq!(get(p.addr, path).status, 404, "{} bytes", path.len());
    }
    assert_eq!(
        get(p.addr, &over_cap).status,
        400,
        "the parser's 2048-byte target cap"
    );
    let head = format!("GET /{} HTTP/1.1\r\n\r\n", "d".repeat(20_000));
    assert!(matches!(send(p.addr, head.as_bytes()).status, 400 | 431));
}

#[test]
fn only_get_is_served_and_changes_still_need_the_token() {
    let ui = ui_dir();
    let p = spawn(Some(&ui));
    for method in ["POST", "PUT", "DELETE", "HEAD", "OPTIONS", "PATCH"] {
        let r = send(
            p.addr,
            format!("{method} /index.html HTTP/1.1\r\nContent-Length: 0\r\n\r\n").as_bytes(),
        );
        assert_eq!(r.status, 401, "{method}: {r:?}");
        let before = std::fs::read_to_string(ui.join("index.html")).unwrap();
        assert_eq!(before, INDEX, "{method} changed nothing");
    }
    let r = get(p.addr, "/?token=abc");
    assert_eq!(r.status, 400, "a credential in the URL is refused here too");
}

#[test]
fn without_ui_the_root_is_not_found_and_says_why() {
    let p = spawn(None);
    let r = get(p.addr, "/");
    assert_eq!(r.status, 404);
    assert!(r.text().contains("--ui"), "{}", r.text());
}

#[test]
fn a_bad_ui_directory_stops_the_plc_with_a_message() {
    let empty = tmp("empty");
    let file = empty.join("file.txt");
    std::fs::write(&file, "x").unwrap();
    for (dir, why) in [
        (empty.join("missing"), "--ui"),
        (empty.clone(), "index.html"),
        (file, "--ui"),
    ] {
        let out = plc()
            .args(["--api", "127.0.0.1:0", "--ui"])
            .arg(&dir)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", dir.display());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(why), "{err}");
    }
    let out = plc().args(["--api", "off", "--ui"]).arg(&empty).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "--ui needs the API");
    let out = plc().args(["--ui", "a", "--ui", "b"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "one --ui");
}

/// A path segment made of the characters traversal is built from.
fn hostile_segment() -> impl Strategy<Value = String> {
    let atom = prop_oneof![
        Just(".".to_string()),
        Just("..".to_string()),
        Just("%2e".to_string()),
        Just("%2E".to_string()),
        Just("%2f".to_string()),
        Just("%5c".to_string()),
        Just("%00".to_string()),
        Just("%25".to_string()),
        Just("%c0%ae".to_string()),
        Just("%ef%bc%8e".to_string()),
        Just("\\".to_string()),
        Just("secret".to_string()),
        Just(".txt".to_string()),
        Just(".js".to_string()),
        Just("up".to_string()),
        Just("ui".to_string()),
        Just("link".to_string()),
        Just(".env".to_string()),
        "[a-z]{1,3}".prop_map(|s| s),
    ];
    prop::collection::vec(atom, 1..5).prop_map(|v| v.concat())
}

#[test]
fn fuzzed_paths_never_leave_the_directory() {
    use proptest::test_runner::{Config, TestRunner};
    let ui = ui_dir();
    let p = spawn(Some(&ui));
    let known = [INDEX.as_bytes().to_vec(), wasm_bytes()];
    let mut runner = TestRunner::new(Config {
        cases: 300,
        ..Config::default()
    });
    runner
        .run(&prop::collection::vec(hostile_segment(), 1..6), |segments| {
            let path = format!("/{}", segments.join("/"));
            let r = get(p.addr, &path);
            prop_assert!(!r.text().contains(SECRET), "{} leaked: {:?}", path, r);
            if r.status == 200 {
                // What is served is a UI file, byte for byte.
                prop_assert!(
                    known.contains(&r.body) || r.body.starts_with(b"export default"),
                    "{}",
                    path
                );
            } else {
                prop_assert!(matches!(r.status, 400 | 404), "{}: {:?}", path, r);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_slow_reader_of_a_big_file_does_not_hold_up_the_api() {
    let ui = ui_dir();
    let p = spawn(Some(&ui));
    // Ask for the wasm and never read it.
    let mut stalled = Vec::new();
    for _ in 0..4 {
        let mut s = TcpStream::connect(p.addr).unwrap();
        write!(s, "GET /{WASM} HTTP/1.1\r\n\r\n").unwrap();
        stalled.push(s);
    }
    let t = Instant::now();
    let r = get(p.addr, "/api/modules");
    assert_eq!(r.status, 200);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    drop(stalled);
}
