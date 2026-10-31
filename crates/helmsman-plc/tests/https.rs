//! The box's own HTTPS (`box-https`), over real sockets: the certificate
//! made at first start and kept in the state directory, its fingerprint
//! printed at start and shown at `GET /api/policy`, a TLS client that pins
//! that fingerprint (and checks the handshake's signatures against it)
//! getting answers and the event stream, plain HTTP from this host still
//! served, and what a hostile client gets: nothing in clear, nothing held
//! past the deadline.

#![allow(clippy::many_single_char_names)] // box, response, json, stream, time

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_plc::api::catalog::Catalog;
use helmsman_plc::api::live::Live;
use helmsman_plc::api::{
    Api, Config, Devices, Extras, Limits, MemAudit, MemProposals, More, Shared, Tls, tls,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::Value as Json;

// ------------------------------------------------------------ the client

/// Trusts exactly the certificate whose SHA-256 is `pin`, as a person
/// does once they have compared the fingerprint, and still checks every
/// handshake signature against that certificate's key.
#[derive(Debug)]
struct Pinned {
    pin: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if intermediates.is_empty() && tls::fingerprint(end_entity) == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the pinned certificate".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// A TLS connection to `addr` trusting only `pin`, handshake done.
fn connect(addr: SocketAddr, pin: &str) -> Result<TlsStream, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            pin: pin.into(),
            provider,
        }))
        .with_no_client_auth();
    let conn = ClientConnection::new(Arc::new(cfg), ServerName::try_from("localhost").unwrap()).unwrap();
    let sock = TcpStream::connect(addr).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut s = StreamOwned::new(conn, sock);
    while s.conn.is_handshaking() {
        s.conn.complete_io(&mut s.sock).map_err(|e| e.to_string())?;
    }
    Ok(s)
}

/// `GET path` over `s`: the whole response, read to the server's close.
fn get_over(mut s: impl Read + Write, path: &str) -> String {
    write!(s, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    s.flush().unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn https_get(addr: SocketAddr, pin: &str, path: &str) -> String {
    get_over(connect(addr, pin).unwrap(), path)
}

fn http_get(addr: SocketAddr, path: &str) -> String {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    get_over(s, path)
}

fn body(response: &str) -> Json {
    let (_, b) = response.split_once("\r\n\r\n").unwrap_or(("", ""));
    serde_json::from_str(b).unwrap_or_else(|e| panic!("{e}: {response}"))
}

// ------------------------------------------------------- the binary, end to end

fn state_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "https-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

struct Plc {
    child: Child,
    _stderr: BufReader<ChildStderr>,
    api: SocketAddr,
    /// The startup's certificate line.
    cert: String,
}

impl Drop for Plc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn plc(state: &Path, extra: &[&str]) -> Command {
    let echo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    let mut c = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"));
    c.args(["--opcua", "off"])
        .arg(echo)
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--api", "127.0.0.1:0", "--state-dir"])
        .arg(state)
        .args(extra);
    c
}

/// Starts the box; returns once its API line is out.
fn start(state: &Path, extra: &[&str]) -> Plc {
    let mut child = plc(state, extra).stderr(Stdio::piped()).spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let (mut api, mut cert) = (None, String::new());
    while api.is_none() {
        let mut line = String::new();
        assert!(stderr.read_line(&mut line).unwrap() > 0, "PLC exited");
        if line.starts_with("helmsman-plc: TLS certificate") || line.contains("--https off") {
            cert = line.trim_end().to_string();
        }
        if let Some(rest) = line
            .strip_prefix("helmsman-plc: API on https://")
            .or_else(|| line.strip_prefix("helmsman-plc: API on http://"))
        {
            api = rest.split('/').next().and_then(|a| a.parse().ok());
            if line.contains("https://") {
                assert!(!cert.is_empty(), "the certificate line comes first: {line}");
            }
        }
    }
    Plc {
        child,
        _stderr: stderr,
        api: api.unwrap(),
        cert,
    }
}

/// The fingerprint a startup line names.
fn printed(line: &str) -> String {
    line.split_whitespace()
        .map(|w| w.trim_end_matches(';'))
        .find(|w| w.len() == 95 && w.matches(':').count() == 31)
        .unwrap_or_else(|| panic!("no fingerprint in {line:?}"))
        .to_string()
}

#[test]
fn the_box_serves_https_with_a_certificate_it_keeps_and_a_fingerprint_that_holds() {
    let state = state_dir("keep");
    let b = start(&state, &[]);
    assert!(b.cert.contains("(new,"), "{}", b.cert);
    let fp = printed(&b.cert);
    // The file: private, and the certificate in it is the one printed.
    let file = state.join(tls::FILE);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let kept = tls::load_or_create(&state, None).unwrap();
    assert!(!kept.new);
    assert_eq!(kept.fingerprint(), fp);

    // Over TLS, trusting exactly that certificate: answers, and the
    // fingerprint the page shows is the one printed.
    let r = https_get(b.api, &fp, "/api/policy");
    assert!(r.starts_with("HTTP/1.1 200 OK"), "{r}");
    let j = body(&r);
    assert_eq!(j["transport"]["https"], true, "{j}");
    assert_eq!(j["transport"]["fingerprint"], fp.as_str(), "{j}");
    let r = https_get(b.api, &fp, "/api/modules");
    assert!(r.contains("\"name\":\"echo\""), "{r}");

    // The event stream the canvas's EventSource opens, over TLS.
    let mut s = connect(b.api, &fp).unwrap();
    write!(
        s,
        "GET /api/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n"
    )
    .unwrap();
    s.flush().unwrap();
    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut chunk = [0u8; 4096];
    while !seen.contains("event: values") && Instant::now() < deadline {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&chunk[..n])),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("{e}: {seen}"),
        }
    }
    assert!(seen.starts_with("HTTP/1.1 200 OK"), "{seen}");
    assert!(seen.contains("text/event-stream"), "{seen}");
    assert!(seen.contains("event: values"), "{seen}");
    drop(s);

    // A client pinning anything else does not get through.
    let other = fp.replacen(&fp[..2], if &fp[..2] == "00" { "01" } else { "00" }, 1);
    assert!(connect(b.api, &other).is_err());
    // Plain HTTP from this host (scripts, an SSH tunnel) is served as ever.
    let r = http_get(b.api, "/api/modules");
    assert!(r.starts_with("HTTP/1.1 200 OK"), "{r}");
    drop(b);

    // Restarted on the same state directory: the same certificate.
    let b = start(&state, &[]);
    assert!(b.cert.contains("(kept,"), "{}", b.cert);
    assert_eq!(printed(&b.cert), fp);
    let r = https_get(b.api, &fp, "/api/policy");
    assert_eq!(body(&r)["transport"]["fingerprint"], fp.as_str());
}

#[test]
fn with_https_off_the_box_is_plain_http_and_makes_no_certificate() {
    let state = state_dir("off");
    let b = start(&state, &["--https", "off"]);
    assert!(b.cert.contains("--https off"), "{}", b.cert);
    assert!(!state.join(tls::FILE).exists());
    let r = http_get(b.api, "/api/policy");
    assert_eq!(body(&r)["transport"]["https"], false, "{r}");
    assert_eq!(body(&r)["transport"]["fingerprint"], Json::Null, "{r}");
    assert!(connect(b.api, &"00:".repeat(32)[..95]).is_err());
}

#[test]
fn a_certificate_file_that_does_not_hold_up_stops_the_box_and_is_left_alone() {
    for (why, text) in [
        ("garbage", "not a certificate\n".to_string()),
        ("empty", String::new()),
        ("key of another certificate", {
            let a = state_dir("a");
            let b = state_dir("b");
            tls::load_or_create(&a, None).unwrap();
            tls::load_or_create(&b, None).unwrap();
            let a = std::fs::read_to_string(a.join(tls::FILE)).unwrap();
            let b = std::fs::read_to_string(b.join(tls::FILE)).unwrap();
            let cert = &a[..a.find("-----BEGIN PRIVATE KEY-----").unwrap()];
            let key = &b[b.find("-----BEGIN PRIVATE KEY-----").unwrap()..];
            format!("{cert}{key}")
        }),
    ] {
        let state = state_dir("bad");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join(tls::FILE), &text).unwrap();
        let out = plc(&state, &[]).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{why}: {err}");
        assert!(
            err.contains(tls::FILE) && err.contains("delete it"),
            "{why}: {err}"
        );
        assert!(!err.contains("API on"), "{why}: refused before serving: {err}");
        assert_eq!(
            std::fs::read_to_string(state.join(tls::FILE)).unwrap(),
            text,
            "{why}"
        );
    }
}

#[test]
fn boxes_starting_at_once_settle_on_one_certificate() {
    let state = state_dir("race");
    let n = 8;
    let go = Arc::new(Barrier::new(n));
    let made: Vec<_> = (0..n)
        .map(|_| {
            let (state, go) = (state.clone(), Arc::clone(&go));
            thread::spawn(move || {
                go.wait();
                tls::load_or_create(&state, None).unwrap()
            })
        })
        .collect();
    let ids: Vec<_> = made.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(ids.iter().filter(|i| i.new).count(), 1, "{ids:?}");
    assert!(ids.iter().all(|i| i.cert == ids[0].cert), "{ids:?}");
    let left: Vec<_> = std::fs::read_dir(&state)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, [tls::FILE], "no staging files left behind");
}

// ------------------------------------------------- the server, adversarially

struct Served {
    api: Api,
    fp: String,
}

fn served(deadline: Duration) -> Served {
    let id = tls::load_or_create(&state_dir("lib"), Some("box-1")).unwrap();
    let cfg = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        token: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        limits: Limits {
            deadline,
            ..Limits::default()
        },
        stream_every: Duration::from_millis(20),
        memory: 16,
        log: Arc::new(|_: &str| {}),
    };
    let shared = Shared {
        catalog: Catalog::default(),
        live: Arc::new(Live::default()),
        devices: Arc::new(Devices::new(Vec::<String>::new())),
        audit: Box::new(MemAudit::new(100)),
        proposals: Box::new(MemProposals::new(100)),
        running: None,
        snapshots: None,
        approval: None,
        chat: None,
    };
    let tls = Some(Tls::new(&id).unwrap());
    let api = Api::start_secure(cfg, shared, Extras::default(), None, More::default(), tls).unwrap();
    Served {
        api,
        fp: id.fingerprint(),
    }
}

/// Reads until the server closes; `Some(bytes)` if it did by `within`.
fn closed_within(mut s: TcpStream, within: Duration) -> Option<Vec<u8>> {
    s.set_read_timeout(Some(within)).unwrap();
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => Some(out),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset) => Some(out),
        Err(_) => None,
    }
}

#[test]
fn a_client_that_says_nothing_or_stalls_its_handshake_is_dropped_at_the_deadline() {
    let s = served(Duration::from_millis(400));
    let addr = s.api.addr();
    // Silent: never says which protocol.
    let t = Instant::now();
    let out = closed_within(TcpStream::connect(addr).unwrap(), Duration::from_secs(3));
    assert_eq!(out.as_deref(), Some(&[][..]), "closed, nothing sent");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // The first byte of a handshake, then nothing.
    let mut c = TcpStream::connect(addr).unwrap();
    c.write_all(&[0x16, 0x03, 0x01]).unwrap();
    let t = Instant::now();
    let out = closed_within(c, Duration::from_secs(3)).expect("closed");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(
        !String::from_utf8_lossy(&out).contains("HTTP/1.1"),
        "nothing in clear"
    );
    // A handshake record of garbage: a TLS alert at most, never HTTP.
    let mut c = TcpStream::connect(addr).unwrap();
    let mut junk = vec![0x16, 0x03, 0x01, 0x00, 0x40];
    junk.extend([0xAB; 64]);
    c.write_all(&junk).unwrap();
    let out = closed_within(c, Duration::from_secs(3)).expect("closed");
    assert!(!String::from_utf8_lossy(&out).contains("HTTP/1.1"), "{out:?}");
    assert!(
        out.first().is_none_or(|b| *b == 0x15),
        "an alert record or nothing: {out:?}"
    );
    // After all that the box still answers.
    let r = https_get(addr, &s.fp, "/api/signals");
    assert!(r.starts_with("HTTP/1.1 200 OK"), "{r}");
}

#[test]
fn many_tls_clients_at_once_all_get_their_answer() {
    let s = served(Duration::from_secs(5));
    let (addr, fp) = (s.api.addr(), s.fp.clone());
    let n = 16;
    let go = Arc::new(Barrier::new(n));
    let all: Vec<_> = (0..n)
        .map(|k| {
            let (go, fp) = (Arc::clone(&go), fp.clone());
            thread::spawn(move || {
                go.wait();
                // Every other one plain, from this host: both kinds at once.
                if k % 2 == 0 {
                    https_get(addr, &fp, "/api/signals")
                } else {
                    http_get(addr, "/api/signals")
                }
            })
        })
        .collect();
    for (k, t) in all.into_iter().enumerate() {
        let r = t.join().unwrap();
        assert!(r.starts_with("HTTP/1.1 200 OK"), "{k}: {r}");
        assert_eq!(body(&r), serde_json::json!([]), "{k}: {r}");
    }
}
