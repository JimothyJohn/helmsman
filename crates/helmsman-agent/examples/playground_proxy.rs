//! The ST playground on localhost with Claude behind a same-origin proxy,
//! so the page never holds an API key (`scripts/playground-serve.sh`).
//!
//! ```text
//! ANTHROPIC_API_KEY=... cargo run -p helmsman-agent --example playground-proxy -- DIR [PORT]
//! ```
//!
//! Serves the built playground from `DIR` on `127.0.0.1:PORT` (default
//! 8791) and answers two routes beside it:
//!
//! - `GET /proxy/health`: `helmsman-playground-proxy ok`, which tells the
//!   page to hide its key field;
//! - `POST /proxy/v1/messages`: the page's Messages API request, sent on
//!   to `https://api.anthropic.com/v1/messages` with `x-api-key` from this
//!   process's environment, its streamed answer copied back as it comes.
//!
//! Loopback only. A request must name this server in `Host` (a DNS
//! rebinding page cannot), carry `x-helmsman-playground: 1` (a cross-site
//! form cannot; a cross-site script would need a preflight this server
//! never grants), and come from this origin if it says where it came
//! from. Only `anthropic-version` and `anthropic-beta` are passed on from
//! the page; the key is added here and never logged. The log is one line
//! per request: method, path, status.
//!
//! Reuses `helmsman_agent::http` (the box's relay client: rustls, Mozilla
//! roots) for the upstream leg, so no new crate.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use helmsman_agent::http::{self, Timeouts, Url};

const HELLO: &str = "helmsman-playground-proxy ok";
const MARK: &str = "x-helmsman-playground";
const UPSTREAM: &str = "https://api.anthropic.com";
const MAX_HEAD: usize = 16 * 1024;
/// A playground request is a program and a short conversation.
const MAX_BODY: usize = 1024 * 1024;
const DEFAULT_VERSION: &str = "2026-10-21";

/// What the server needs; `upstream` is the API (tests point it at a
/// local fake).
struct Config {
    dir: PathBuf,
    port: u16,
    key: String,
    upstream: Url,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: playground-proxy DIR [PORT]   (ANTHROPIC_API_KEY in the environment)");
        std::process::exit(2);
    };
    let port: u16 = args.next().map_or(Ok(8791), |p| p.parse()).unwrap_or_else(|_| {
        eprintln!("bad port");
        std::process::exit(2);
    });
    let key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    if key.trim().is_empty() {
        eprintln!("ANTHROPIC_API_KEY is not set: the proxy adds it to every request");
        std::process::exit(2);
    }
    let dir = PathBuf::from(dir);
    if !dir.join("index.html").is_file() {
        eprintln!("{} has no index.html: build the playground first", dir.display());
        std::process::exit(2);
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| {
        eprintln!("cannot listen on 127.0.0.1:{port}: {e}");
        std::process::exit(1);
    });
    let port = listener.local_addr().map_or(port, |a| a.port());
    eprintln!("playground on http://127.0.0.1:{port}/ (Claude through the proxy; Ctrl-C stops)");
    serve(
        &listener,
        &Arc::new(Config {
            dir,
            port,
            key: key.trim().to_string(),
            upstream: Url::parse(UPSTREAM).expect("constant URL"),
        }),
    );
}

fn serve(listener: &TcpListener, cfg: &Arc<Config>) {
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let cfg = Arc::clone(cfg);
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Some((method, path, status)) = handle(stream, &cfg) {
                eprintln!("{method} {path} {status}");
            }
        });
    }
}

/// A request head.
#[derive(Debug, Default)]
struct Head {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

fn read_head(r: &mut impl BufRead) -> Option<Head> {
    let mut head = Head::default();
    let mut used = 0;
    let mut first = true;
    loop {
        let mut line = String::new();
        let n = r.by_ref().take(MAX_HEAD as u64).read_line(&mut line).ok()?;
        used += n;
        if n == 0 || used > MAX_HEAD {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if first {
            first = false;
            let mut parts = line.split(' ');
            head.method = parts.next()?.to_string();
            head.path = parts.next()?.to_string();
            continue;
        }
        if line.is_empty() {
            return Some(head);
        }
        let (n, v) = line.split_once(':')?;
        head.headers
            .push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
}

fn reply(w: &mut impl Write, status: u16, ctype: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Bad Gateway",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = w.write_all(head.as_bytes());
    let _ = w.write_all(body);
    let _ = w.flush();
}

/// One connection; what to log.
fn handle(stream: TcpStream, cfg: &Config) -> Option<(String, String, u16)> {
    let mut w = stream.try_clone().ok()?;
    let mut r = BufReader::new(stream);
    let head = read_head(&mut r)?;
    let path = head.path.split(['?', '#']).next().unwrap_or("").to_string();
    let status = route(&head, &path, &mut r, &mut w, cfg);
    Some((head.method, path, status))
}

fn route(head: &Head, path: &str, r: &mut impl BufRead, w: &mut impl Write, cfg: &Config) -> u16 {
    let ours = [
        format!("127.0.0.1:{}", cfg.port),
        format!("localhost:{}", cfg.port),
    ];
    if !head
        .header("host")
        .is_some_and(|h| ours.iter().any(|o| o.eq_ignore_ascii_case(h)))
    {
        reply(w, 403, "text/plain", b"wrong Host\n");
        return 403;
    }
    match (head.method.as_str(), path) {
        ("GET", "/proxy/health") => {
            reply(w, 200, "text/plain", HELLO.as_bytes());
            200
        }
        ("POST", "/proxy/v1/messages") => forward(head, r, w, cfg),
        (_, p) if p.starts_with("/proxy/") => {
            reply(w, 405, "text/plain", b"POST /proxy/v1/messages only\n");
            405
        }
        ("GET" | "HEAD", p) => file(p, w, cfg),
        _ => {
            reply(w, 405, "text/plain", b"GET only\n");
            405
        }
    }
}

/// The file `path` names under `dir`, or `None` for anything that could
/// leave it (`..`, a drive or root, a backslash, an encoded byte).
fn resolve(dir: &Path, path: &str) -> Option<PathBuf> {
    let rel = path.strip_prefix('/')?;
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.contains(['\\', '%', '\0']) {
        return None;
    }
    let p = Path::new(rel);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(dir.join(p))
}

fn file(path: &str, w: &mut impl Write, cfg: &Config) -> u16 {
    let body = resolve(&cfg.dir, path)
        .filter(|p| p.is_file())
        .and_then(|p| std::fs::read(&p).ok().map(|b| (p, b)));
    let Some((p, body)) = body else {
        reply(w, 404, "text/plain", b"not found\n");
        return 404;
    };
    let ctype = match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" => "application/json",
        _ => "application/octet-stream",
    };
    reply(w, 200, ctype, &body);
    200
}

/// `POST /proxy/v1/messages`: checked, sent on with the key, streamed back.
fn forward(head: &Head, r: &mut impl BufRead, w: &mut impl Write, cfg: &Config) -> u16 {
    if head.header(MARK) != Some("1") {
        reply(w, 403, "text/plain", b"not from the playground page\n");
        return 403;
    }
    if let Some(origin) = head.header("origin") {
        let ours = [
            format!("http://127.0.0.1:{}", cfg.port),
            format!("http://localhost:{}", cfg.port),
        ];
        if !ours.iter().any(|o| o.eq_ignore_ascii_case(origin)) {
            reply(w, 403, "text/plain", b"cross-origin\n");
            return 403;
        }
    }
    let Some(len) = head
        .header("content-length")
        .and_then(|n| n.parse::<usize>().ok())
    else {
        reply(w, 400, "text/plain", b"Content-Length required\n");
        return 400;
    };
    if len > MAX_BODY {
        reply(w, 413, "text/plain", b"too large\n");
        return 413;
    }
    let mut body = vec![0; len];
    if r.read_exact(&mut body).is_err() {
        return 400;
    }
    // Only the API's own headers pass, and only in their own alphabet.
    let clean = |v: &str| {
        (!v.is_empty()
            && v.len() <= 200
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-,_".contains(&b)))
        .then(|| v.to_string())
    };
    let version = head
        .header("anthropic-version")
        .and_then(clean)
        .unwrap_or_else(|| DEFAULT_VERSION.into());
    let beta = head.header("anthropic-beta").and_then(clean);
    let mut headers: Vec<(&str, &str)> =
        vec![("x-api-key", cfg.key.as_str()), ("anthropic-version", &version)];
    if let Some(b) = &beta {
        headers.push(("anthropic-beta", b));
    }
    let path = format!("{}/v1/messages", cfg.upstream.prefix);
    let req = http::Request {
        method: "POST",
        path: &path,
        headers: &headers,
        json: Some(&body),
    };
    let t = Timeouts {
        connect: Duration::from_secs(10),
        idle: Duration::from_secs(120),
        total: Duration::from_secs(600),
    };
    let mut resp = match http::send(&cfg.upstream, &req, t) {
        Ok(r) => r,
        Err(e) => {
            // http::Error never carries a header value.
            let msg = serde_json::json!({
                "type": "error",
                "error": {"type": "proxy_error", "message": format!("the proxy could not reach the API: {e}")},
            });
            reply(w, 502, "application/json", msg.to_string().as_bytes());
            return 502;
        }
    };
    let ctype = resp
        .header("content-type")
        .unwrap_or("application/json")
        .to_string();
    let head = format!(
        "HTTP/1.1 {} Upstream\r\nContent-Type: {ctype}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        resp.status
    );
    if w.write_all(head.as_bytes()).is_err() {
        return resp.status;
    }
    // Close-delimited: copy each piece as it arrives, so the page streams.
    let mut buf = [0u8; 8192];
    loop {
        match resp.body.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if w.write_all(&buf[..n]).and_then(|()| w.flush()).is_err() {
                    break;
                }
            }
        }
    }
    resp.status
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake upstream: answers one request with `answer` and hands back
    /// what it was sent.
    fn fake_api(answer: &'static str) -> (Url, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}", l.local_addr().unwrap())).unwrap();
        let h = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            let head = read_head(&mut r).unwrap();
            let len: usize = head.header("content-length").unwrap().parse().unwrap();
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            w.write_all(answer.as_bytes()).unwrap();
            let mut seen: Vec<String> = head.headers.iter().map(|(n, v)| format!("{n}: {v}")).collect();
            seen.sort();
            format!(
                "{} {}\n{}\n\n{}",
                head.method,
                head.path,
                seen.join("\n"),
                String::from_utf8(body).unwrap()
            )
        });
        (url, h)
    }

    struct Server {
        port: u16,
        _dir: TempDir,
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn server(upstream: Url) -> Server {
        let dir = std::env::temp_dir().join(format!(
            "playground-proxy-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html>page").unwrap();
        std::fs::write(dir.join("app.wasm"), b"\0asm").unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let cfg = Arc::new(Config {
            dir: dir.clone(),
            port,
            key: "test-key-not-real".into(),
            upstream,
        });
        std::thread::spawn(move || serve(&l, &cfg));
        Server {
            port,
            _dir: TempDir(dir),
        }
    }

    /// Sends raw `req` to the server; the whole response.
    fn send(port: u16, req: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    fn get(port: u16, path: &str) -> String {
        send(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
    }

    fn post(port: u16, extra: &str, body: &str) -> String {
        send(
            port,
            &format!(
                "POST /proxy/v1/messages HTTP/1.1\r\nHost: localhost:{port}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    fn unused() -> Url {
        Url::parse("http://127.0.0.1:9").unwrap()
    }

    #[test]
    fn serves_the_page_and_says_hello() {
        let s = server(unused());
        let page = get(s.port, "/");
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("text/html") && page.ends_with("page"));
        assert!(get(s.port, "/app.wasm?v=1").contains("application/wasm"));
        let hello = get(s.port, "/proxy/health");
        assert!(hello.ends_with(HELLO), "{hello}");
    }

    #[test]
    fn refuses_paths_out_of_the_directory() {
        let s = server(unused());
        for p in [
            "/../Cargo.toml",
            "/sub/../../x",
            "/%2e%2e/x",
            "/..\\x",
            "//etc/passwd",
            "/sub",
            "/nope",
        ] {
            let r = get(s.port, p);
            assert!(r.starts_with("HTTP/1.1 404"), "{p}: {r}");
        }
        let d = Path::new("/srv");
        assert_eq!(resolve(d, "/"), Some(PathBuf::from("/srv/index.html")));
        assert_eq!(resolve(d, "/a/b.js"), Some(PathBuf::from("/srv/a/b.js")));
        assert_eq!(resolve(d, "/a/../b"), None);
        assert_eq!(resolve(d, "/./b"), None);
    }

    #[test]
    fn refuses_other_hosts_cross_site_posts_and_bodies_too_large() {
        let s = server(unused());
        let rebound = send(s.port, "GET / HTTP/1.1\r\nHost: evil.example:80\r\n\r\n");
        assert!(rebound.starts_with("HTTP/1.1 403"), "DNS rebinding: {rebound}");
        let unmarked = post(s.port, "", "{}");
        assert!(unmarked.starts_with("HTTP/1.1 403"), "no mark: {unmarked}");
        let cross = post(
            s.port,
            &format!("{MARK}: 1\r\nOrigin: https://evil.example\r\n"),
            "{}",
        );
        assert!(cross.starts_with("HTTP/1.1 403"), "{cross}");
        let big = send(
            s.port,
            &format!(
                "POST /proxy/v1/messages HTTP/1.1\r\nHost: localhost:{}\r\n{MARK}: 1\r\nContent-Length: {}\r\n\r\n",
                s.port,
                MAX_BODY + 1
            ),
        );
        assert!(big.starts_with("HTTP/1.1 413"), "{big}");
        let wrong = send(
            s.port,
            &format!(
                "GET /proxy/v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
                s.port
            ),
        );
        assert!(wrong.starts_with("HTTP/1.1 405"), "{wrong}");
    }

    #[test]
    fn forwards_with_the_key_and_streams_the_answer_back() {
        let (url, seen) = fake_api(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n\
             11\r\nevent: ping\ndata:\r\n3\r\n {}\r\n0\r\n\r\n",
        );
        let s = server(url);
        let origin = format!("http://localhost:{}", s.port);
        let r = post(
            s.port,
            &format!(
                "{MARK}: 1\r\nOrigin: {origin}\r\nanthropic-version: 2026-10-21\r\nanthropic-beta: server-side-fallback-2026-07-01\r\nx-api-key: from-the-page\r\nCookie: secret=1\r\n"
            ),
            r#"{"model":"claude-sonnet-5-5"}"#,
        );
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(r.contains("text/event-stream"));
        assert!(r.ends_with("event: ping\ndata: {}"), "de-chunked, whole: {r:?}");
        let seen = seen.join().unwrap();
        assert!(seen.starts_with("POST /v1/messages\n"), "{seen}");
        assert!(seen.contains("x-api-key: test-key-not-real"), "{seen}");
        assert!(!seen.contains("from-the-page"), "the page's own key is dropped");
        assert!(!seen.contains("secret"), "cookies are not forwarded");
        assert!(!seen.contains(MARK));
        assert!(seen.contains("anthropic-version: 2026-10-21"));
        assert!(seen.contains("anthropic-beta: server-side-fallback-2026-07-01"));
        assert!(seen.ends_with(r#"{"model":"claude-sonnet-5-5"}"#));
    }

    #[test]
    fn passes_api_errors_through_and_reports_an_unreachable_api() {
        let (url, seen) = fake_api(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 16\r\n\r\n{\"type\":\"error\"}",
        );
        let s = server(url);
        let r = post(
            s.port,
            &format!("{MARK}: 1\r\nanthropic-version: bad version\r\n"),
            "{}",
        );
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        assert!(
            seen.join().unwrap().contains("anthropic-version: 2026-10-21"),
            "a bad value is replaced"
        );
        let down = server(unused());
        let r = post(down.port, &format!("{MARK}: 1\r\n"), "{}");
        assert!(r.starts_with("HTTP/1.1 502"), "{r}");
        assert!(r.contains("proxy_error"));
        assert!(!r.contains("test-key-not-real"));
    }
}
