//! Pairing by code on the box: `helmsman-plc pair` against a scripted
//! relay on a real loopback socket, and `helmsman-firstboot` (the script
//! the image runs) driving that binary end to end in a temp root.
//!
//! The contract: a code is traded for a token once; the token lands in a
//! 0600 file or nowhere; neither the code nor the token is ever printed;
//! a refused code is dropped and not retried at every boot; a code that
//! could not be tried stays; a token file on the card wins over a code;
//! `http://` needs the card's say-so. The relay is the test's own, so the
//! run needs no network and no clock.

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const CODE: &str = "7K3QM-X9R2D";
const BOX_ID: &str = "plc-7";
const TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const OTHER_TOKEN: &str = "hmr1_ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_helmsman-plc")
}

fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "pair-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------- relay

/// What the scripted relay answers: status and JSON body.
type Reply = (u16, String);

fn token_reply(token: &str, box_id: &str) -> Reply {
    (200, format!(r#"{{"box_id":"{box_id}","token":"{token}"}}"#))
}

fn refusal() -> Reply {
    (
        403,
        r#"{"type":"error","error":{"type":"forbidden","message":"pairing refused: the code is wrong, used, expired or for another box","retryable":false},"request_id":"r"}"#
            .into(),
    )
}

/// One request the relay got.
#[derive(Debug, Clone)]
struct Seen {
    head: String,
    body: String,
}

struct Relay {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    script: Arc<Mutex<VecDeque<Reply>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    /// Answers the replies in order; anything after them is a 500.
    fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::from(replies)));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, sc, st) = (seen.clone(), script.clone(), stop.clone());
        let thread = thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut conn) = conn else { continue };
                let Some(request) = read_request(&mut conn) else {
                    continue;
                };
                s.lock().unwrap_or_else(PoisonError::into_inner).push(request);
                let (status, body) = sc
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .pop_front()
                    .unwrap_or((500, "{}".into()));
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            addr,
            seen,
            script,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn unanswered(&self) -> usize {
        self.script.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_request(conn: &mut TcpStream) -> Option<Seen> {
    conn.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(k) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break k + 4;
        }
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().ok())
        })
        .flatten()
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Some(Seen {
        head,
        body: String::from_utf8_lossy(&buf[head_end..]).into_owned(),
    })
}

/// A URL nobody listens on: bind, note the port, close.
fn dead_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", l.local_addr().unwrap())
}

// ----------------------------------------------------- `helmsman-plc pair`

struct Pair {
    dir: PathBuf,
    out: Output,
}

impl Pair {
    fn token_file(&self) -> PathBuf {
        self.dir.join("relay-token")
    }

    /// Everything the command printed.
    fn said(&self) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&self.out.stdout),
            String::from_utf8_lossy(&self.out.stderr)
        )
    }

    fn code(&self) -> i32 {
        self.out.status.code().unwrap()
    }
}

fn pair_with(url: &str, box_id: &str, code_text: &str, extra: &[&str]) -> Pair {
    let dir = tmp("cmd");
    fs::write(dir.join("code"), code_text).unwrap();
    let out = Command::new(bin())
        .arg("pair")
        .args(["--relay", url, "--box-id", box_id])
        .arg("--code-file")
        .arg(dir.join("code"))
        .arg("--out")
        .arg(dir.join("relay-token"))
        .args(extra)
        .current_dir(&dir)
        .output()
        .unwrap();
    Pair { dir, out }
}

fn pair(relay: &Relay) -> Pair {
    pair_with(&relay.url(), BOX_ID, &format!("{CODE}\n"), &["--wait", "0"])
}

/// The command printed neither secret, and left no half-written token.
fn assert_quiet(p: &Pair) {
    let said = p.said();
    assert!(
        !said.contains(CODE) && !said.contains("7K3QM"),
        "the code was printed: {said}"
    );
    assert!(
        !said.contains(TOKEN) && !said.contains("hmr1_"),
        "a token was printed: {said}"
    );
    let leftovers: Vec<_> = fs::read_dir(&p.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "left {leftovers:?}");
}

#[test]
fn a_code_is_traded_for_a_token_in_a_private_file() {
    let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
    let p = pair(&relay);
    assert_eq!(p.code(), 0, "{}", p.said());
    assert_eq!(fs::read_to_string(p.token_file()).unwrap(), format!("{TOKEN}\n"));
    // Windows has no mode bits: the file takes its directory's ACL.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(p.token_file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_quiet(&p);
    assert!(p.said().contains("paired as plc-7"), "{}", p.said());

    // What went over the wire: one POST, the code and the id, no bearer.
    let seen = relay.seen();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].head.starts_with("POST /v1/pair HTTP/1.1\r\n"),
        "{}",
        seen[0].head
    );
    assert!(!seen[0].head.to_ascii_lowercase().contains("authorization"));
    let body: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(body["code"], CODE);
    assert_eq!(body["box_id"], BOX_ID);
}

#[test]
fn a_relay_under_a_path_prefix_is_asked_there() {
    let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
    let p = pair_with(&format!("{}/relay/", relay.url()), BOX_ID, CODE, &["--wait", "0"]);
    assert_eq!(p.code(), 0, "{}", p.said());
    assert!(relay.seen()[0].head.starts_with("POST /relay/v1/pair "));
}

#[test]
fn a_refused_code_exits_3_and_leaves_no_token() {
    let relay = Relay::start(vec![refusal()]);
    let p = pair(&relay);
    assert_eq!(p.code(), 3, "{}", p.said());
    assert!(!p.token_file().exists());
    assert_quiet(&p);
    assert!(p.said().contains("new one"), "{}", p.said());
    assert_eq!(relay.seen().len(), 1, "a refusal is not retried");
}

#[test]
fn a_relay_without_pairing_a_failing_relay_and_a_silent_one_exit_1() {
    for (status, body) in [
        (404, r#"{"error":{"type":"not_found","message":"not found"}}"#),
        (500, "{}"),
        (502, "<html>"),
    ] {
        let relay = Relay::start(vec![(status, body.to_string())]);
        let p = pair(&relay);
        assert_eq!(p.code(), 1, "{status}: {}", p.said());
        assert!(!p.token_file().exists(), "{status}");
        assert_quiet(&p);
        assert_eq!(
            relay.seen().len(),
            1,
            "{status}: a failure after the code was sent is not retried"
        );
    }
    let p = pair_with(&dead_url(), BOX_ID, CODE, &["--wait", "0"]);
    assert_eq!(p.code(), 1, "{}", p.said());
    assert!(!p.token_file().exists());
    assert_quiet(&p);
}

#[test]
fn too_many_tries_is_waited_out_and_an_unreachable_relay_is_retried() {
    let slow = (
        429,
        r#"{"error":{"type":"rate_limited","message":"slow down","retryable":true}}"#.to_string(),
    );
    let relay = Relay::start(vec![slow, token_reply(TOKEN, BOX_ID)]);
    let p = pair_with(&relay.url(), BOX_ID, CODE, &["--wait", "30"]);
    assert_eq!(p.code(), 0, "{}", p.said());
    assert_eq!(relay.seen().len(), 2);
    assert_eq!(relay.unanswered(), 0);
    assert_quiet(&p);
}

#[test]
fn an_answer_that_is_not_this_boxs_token_is_not_kept() {
    for reply in [
        token_reply(TOKEN, "plc-8"),       // another box's
        token_reply("hmr1_short", BOX_ID), // not a token
        token_reply("not-a-token", BOX_ID),
        (200, "{}".into()),
        (200, "not json".into()),
        (200, r#"{"token":42,"box_id":"plc-7"}"#.into()),
    ] {
        let relay = Relay::start(vec![reply.clone()]);
        let p = pair(&relay);
        assert_eq!(p.code(), 1, "{reply:?}: {}", p.said());
        assert!(!p.token_file().exists(), "{reply:?}");
        assert_quiet(&p);
        assert!(
            p.said().contains("ask the operator for a new one")
                || p.said().contains("bad response")
                || p.said().contains("malformed"),
            "{}",
            p.said()
        );
    }
}

#[test]
fn a_bad_request_never_reaches_the_relay() {
    let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
    let url = relay.url();
    for (box_id, code_text, why) in [
        ("Plc-7", CODE, "upper case id"),
        ("-plc", CODE, "leading dash"),
        ("", CODE, "empty id"),
        ("a b", CODE, "blank in id"),
        (BOX_ID, "", "empty code file"),
        (BOX_ID, "short", "short code"),
        (BOX_ID, "7K3QM-X9R2D-EXTRA", "long code"),
        (BOX_ID, "7K3QM-X9R2!", "punctuation"),
        (BOX_ID, "7K3QM-X9R2U", "a U is no symbol"),
        (
            BOX_ID,
            "hmr1_00112233445566778899aabbccddeeff",
            "a token where the code goes",
        ),
    ] {
        let p = pair_with(&url, box_id, code_text, &["--wait", "0"]);
        assert_eq!(p.code(), 1, "{why}: {}", p.said());
        assert!(!p.token_file().exists(), "{why}");
        let said = p.said();
        assert!(
            !said.contains("hmr1_") && !said.contains("EXTRA") && !said.contains("X9R2"),
            "{why}: echoed: {said}"
        );
    }
    assert!(relay.seen().is_empty(), "{:?}", relay.seen());
}

#[test]
fn bad_arguments_are_refused_before_anything_is_read() {
    let dir = tmp("args");
    let run = |args: &[&str]| {
        Command::new(bin())
            .arg("pair")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    for args in [
        &[][..],
        &["--relay", "http://127.0.0.1:1"],
        &[
            "--relay",
            "ftp://x",
            "--box-id",
            "a",
            "--code-file",
            "c",
            "--out",
            "o",
        ],
        &[
            "--relay",
            "http://127.0.0.1:1",
            "--box-id",
            "a",
            "--code-file",
            "c",
            "--out",
            "o",
            "--wait",
            "601",
        ],
        &[
            "--relay",
            "http://127.0.0.1:1",
            "--box-id",
            "a",
            "--code-file",
            "c",
            "--out",
            "o",
            "--bogus",
            "x",
        ],
        &[
            "--relay",
            "http://127.0.0.1:1",
            "--relay",
            "http://127.0.0.1:2",
            "--box-id",
            "a",
            "--code-file",
            "c",
            "--out",
            "o",
        ],
        &[
            "--relay",
            "http://127.0.0.1:1",
            "--box-id",
            "a",
            "--code-file",
            "/nonexistent/code",
            "--out",
            "o",
        ],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(!dir.join("o").exists());
    }
}

#[test]
fn an_unwritable_destination_fails_without_spending_a_second_try() {
    let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
    let dir = tmp("nowhere");
    fs::write(dir.join("code"), CODE).unwrap();
    let out = Command::new(bin())
        .arg("pair")
        .args(["--relay", &relay.url(), "--box-id", BOX_ID, "--wait", "0"])
        .arg("--code-file")
        .arg(dir.join("code"))
        .arg("--out")
        .arg(dir.join("no-such-dir/relay-token"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!said.contains(TOKEN) && !said.contains(CODE), "{said}");
}

// ------------------------------------------------------ helmsman-firstboot

// deploy/helmsman-firstboot.sh is a bash script for the Linux image: its
// tests run on Unix hosts only.
#[cfg(unix)]
mod firstboot {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn script() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helmsman-firstboot.sh")
    }

    /// A fresh root: an installed box's env file, an empty (root-only) token
    /// file as the image ships it, and an empty pairing folder.
    fn root() -> PathBuf {
        let dir = tmp("root");
        fs::create_dir_all(dir.join("boot/firmware/helmsman")).unwrap();
        fs::create_dir_all(dir.join("etc/helmsman-plc")).unwrap();
        fs::write(
            dir.join("etc/helmsman-plc/env"),
            "PLC_SOURCE=--projects\nPLC_RELAY_ARGS=\n",
        )
        .unwrap();
        fs::write(dir.join("etc/helmsman-plc/relay-token"), "").unwrap();
        dir
    }

    fn put(root: &Path, name: &str, text: &str) {
        fs::write(root.join("boot/firmware/helmsman").join(name), text).unwrap();
    }

    fn on_card(root: &Path, name: &str) -> bool {
        root.join("boot/firmware/helmsman").join(name).exists()
    }

    struct Boot {
        said: String,
        ok: bool,
    }

    fn boot(root: &Path) -> Boot {
        let out = Command::new("bash")
            .arg(script())
            .env("HELMSMAN_FIRSTBOOT_ROOT", root)
            .env("HELMSMAN_PLC_BIN", bin())
            .output()
            .unwrap();
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !said.contains(CODE) && !said.contains("7K3QM"),
            "the code was printed: {said}"
        );
        assert!(
            !said.contains(TOKEN) && !said.contains(OTHER_TOKEN),
            "a token was printed: {said}"
        );
        Boot {
            said,
            ok: out.status.success(),
        }
    }

    fn installed_token(root: &Path) -> String {
        fs::read_to_string(root.join("etc/helmsman-plc/relay-token")).unwrap()
    }

    fn relay_args(root: &Path) -> String {
        fs::read_to_string(root.join("etc/helmsman-plc/env"))
            .unwrap()
            .lines()
            .find_map(|l| l.strip_prefix("PLC_RELAY_ARGS=").map(str::to_owned))
            .unwrap()
    }

    /// A card as the operator prepares it for pairing by code.
    fn card(root: &Path, url: &str) {
        put(root, "relay-url", &format!("{url}\n"));
        put(root, "relay-allow-http", "");
        put(root, "relay-box-id", &format!("{BOX_ID}\n"));
        put(root, "pairing-code", &format!("{CODE}\n"));
    }

    #[test]
    fn first_boot_pairs_by_code_and_sets_the_relay() {
        let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
        let root = root();
        card(&root, &relay.url());
        let b = boot(&root);
        assert!(b.ok, "{}", b.said);
        assert_eq!(installed_token(&root), format!("{TOKEN}\n"));
        let mode = fs::metadata(root.join("etc/helmsman-plc/relay-token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!on_card(&root, "pairing-code"), "the code leaves the card");
        assert_eq!(
            relay_args(&root),
            format!(
                "--relay {} --relay-token /run/credentials/helmsman-plc.service/relay-token",
                relay.url()
            )
        );
        let body: serde_json::Value = serde_json::from_str(&relay.seen()[0].body).unwrap();
        assert_eq!(
            (body["code"].as_str(), body["box_id"].as_str()),
            (Some(CODE), Some(BOX_ID))
        );

        // The next boot has nothing to trade and changes nothing.
        let again = boot(&root);
        assert!(again.ok, "{}", again.said);
        assert_eq!(relay.seen().len(), 1);
        assert_eq!(installed_token(&root), format!("{TOKEN}\n"));
    }

    #[test]
    fn a_refused_code_is_dropped_and_the_box_starts_without_a_relay() {
        let relay = Relay::start(vec![refusal()]);
        let root = root();
        card(&root, &relay.url());
        let b = boot(&root);
        assert!(b.ok, "a refusal must not stop the PLC service: {}", b.said);
        assert!(
            !on_card(&root, "pairing-code"),
            "a refused code is not tried at every boot"
        );
        assert_eq!(installed_token(&root), "");
        assert_eq!(relay_args(&root), "");
        assert!(b.said.contains("refused"), "{}", b.said);
        boot(&root);
        assert_eq!(relay.seen().len(), 1);
    }

    #[test]
    fn a_code_that_could_not_be_tried_stays_for_the_next_boot() {
        let root = root();
        card(&root, &dead_url());
        let b = boot(&root);
        assert!(b.ok, "{}", b.said);
        assert!(on_card(&root, "pairing-code"));
        assert_eq!(installed_token(&root), "");
        assert_eq!(relay_args(&root), "");
        // The relay comes up; the next start pairs.
        let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
        put(&root, "relay-url", &format!("{}\n", relay.url()));
        let b = boot(&root);
        assert!(b.ok, "{}", b.said);
        assert!(!on_card(&root, "pairing-code"));
        assert_eq!(installed_token(&root), format!("{TOKEN}\n"));
    }

    #[test]
    fn a_token_file_wins_over_a_code_and_the_relay_is_not_asked() {
        let relay = Relay::start(vec![token_reply(OTHER_TOKEN, BOX_ID)]);
        let root = root();
        card(&root, &relay.url());
        put(&root, "relay-token", &format!("{TOKEN}\n"));
        let b = boot(&root);
        assert!(b.ok, "{}", b.said);
        assert!(relay.seen().is_empty());
        assert_eq!(installed_token(&root), format!("{TOKEN}\n"));
        assert!(!on_card(&root, "pairing-code") && !on_card(&root, "relay-token"));
        assert!(relay_args(&root).starts_with("--relay "));
        // An installed token also keeps a later code out.
        put(&root, "pairing-code", &format!("{CODE}\n"));
        let b = boot(&root);
        assert!(b.ok, "{}", b.said);
        assert!(relay.seen().is_empty());
        assert!(!on_card(&root, "pairing-code"));
        assert_eq!(installed_token(&root), format!("{TOKEN}\n"));
    }

    #[test]
    fn a_code_is_not_tried_without_what_it_needs() {
        for (drop_file, why) in [
            ("relay-url", "no relay-url"),
            ("relay-box-id", "no relay-box-id"),
            ("relay-allow-http", "an http:// relay without the say-so"),
        ] {
            let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
            let root = root();
            card(&root, &relay.url());
            fs::remove_file(root.join("boot/firmware/helmsman").join(drop_file)).unwrap();
            let b = boot(&root);
            assert!(b.ok, "{why}: {}", b.said);
            assert!(relay.seen().is_empty(), "{why}: the relay was asked");
            assert!(
                on_card(&root, "pairing-code"),
                "{why}: the code is kept for when the card is fixed"
            );
            assert_eq!(installed_token(&root), "", "{why}");
            assert_eq!(relay_args(&root), "", "{why}");
        }
    }

    #[test]
    fn a_bad_box_id_on_the_card_is_not_sent() {
        for id in [
            "Plc-7",
            "-plc",
            "plc 7",
            "plc/7",
            "a".repeat(65).as_str(),
            "",
            "plc-7\nplc-8",
        ] {
            let relay = Relay::start(vec![token_reply(TOKEN, BOX_ID)]);
            let root = root();
            card(&root, &relay.url());
            put(&root, "relay-box-id", &format!("{id}\n"));
            let b = boot(&root);
            assert!(b.ok, "{id:?}: {}", b.said);
            if id == "plc-7\nplc-8" {
                // Only the first line counts, as for every pairing file.
                assert_eq!(relay.seen().len(), 1);
                continue;
            }
            assert!(relay.seen().is_empty(), "{id:?}: sent");
            assert!(on_card(&root, "pairing-code"), "{id:?}");
        }
    }
}
