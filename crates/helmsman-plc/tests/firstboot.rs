//! deploy/helmsman-firstboot.sh against the box that reads what it writes:
//! whatever pairing files sit on the boot partition, the `PLC_RELAY_ARGS`
//! line it leaves must be one helmsman-plc accepts, or the PLC service
//! stops at its next start. Run in a temp root (`HELMSMAN_FIRSTBOOT_ROOT`),
//! the line split on whitespace the way the unit's `$PLC_RELAY_ARGS` is.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use proptest::prelude::*;

const TOKEN: &str = "hmr1_00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const URL: &str = "https://7kyephbdjczrsu4vpih3fuo6l40vpzii.lambda-url.us-east-1.on.aws";
const CRED: &str = "/run/credentials/helmsman-plc.service/relay-token";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helmsman-firstboot.sh")
}

/// A fresh root with an installed box's env file and an empty pairing folder.
fn root() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "firstboot-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("boot/firmware/helmsman")).unwrap();
    fs::create_dir_all(dir.join("etc/helmsman-plc")).unwrap();
    fs::write(
        dir.join("etc/helmsman-plc/env"),
        "PLC_SOURCE=--projects\nPLC_API=127.0.0.1:8080\n",
    )
    .unwrap();
    dir
}

fn put(root: &Path, name: &str, text: &str) {
    fs::write(root.join("boot/firmware/helmsman").join(name), text).unwrap();
}

struct Boot {
    stderr: String,
    /// The `PLC_RELAY_ARGS` value, if the line is there.
    line: Option<String>,
}

fn boot(root: &Path) -> Boot {
    let out = Command::new("bash")
        .arg(script())
        .env("HELMSMAN_FIRSTBOOT_ROOT", root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "firstboot failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let env = fs::read_to_string(root.join("etc/helmsman-plc/env")).unwrap();
    assert!(
        env.contains("PLC_API=127.0.0.1:8080\n"),
        "the rest of env was lost: {env}"
    );
    let lines: Vec<_> = env
        .lines()
        .filter_map(|l| l.strip_prefix("PLC_RELAY_ARGS="))
        .collect();
    assert!(lines.len() <= 1, "two PLC_RELAY_ARGS lines: {env}");
    Boot {
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        line: lines.first().map(|l| (*l).to_string()),
    }
}

/// The box parses the line with the unit's other arguments and gets as far
/// as loading its program (here missing on purpose: exit 1, not the usage
/// error's exit 2), so nothing is started.
fn box_accepts(line: Option<&str>) {
    let args: Vec<&str> = line.map(|l| l.split_whitespace().collect()).unwrap_or_default();
    let state =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("firstboot-state-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(["--opcua", "off", "--api", "127.0.0.1:18080", "--state-dir"])
        .arg(&state)
        .args(&args)
        .arg("/nonexistent/firstboot-check.st")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "the box refused {args:?}: {stderr}");
    assert!(stderr.contains("firstboot-check.st"), "{args:?}: {stderr}");
}

fn words(line: Option<&str>) -> Vec<String> {
    line.unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn a_full_pairing_names_the_box_in_its_site_and_the_box_starts() {
    let r = root();
    put(&r, "relay-url", &format!("{URL}\n"));
    put(&r, "relay-token", &format!("{TOKEN}\r\n"));
    put(&r, "site", "line-a\n");
    put(&r, "box-name", "Press 4\n");
    let b = boot(&r);
    assert_eq!(
        words(b.line.as_deref()),
        [
            "--relay",
            URL,
            "--relay-token",
            CRED,
            "--site",
            "line-a",
            "--box-name",
            "Press-4"
        ]
    );
    box_accepts(b.line.as_deref());
    // The secret left the FAT partition and sits root-only in /etc.
    assert!(!r.join("boot/firmware/helmsman/relay-token").exists());
    let token = r.join("etc/helmsman-plc/relay-token");
    assert_eq!(fs::read_to_string(&token).unwrap(), format!("{TOKEN}\n"));
    assert_eq!(fs::metadata(&token).unwrap().permissions().mode() & 0o777, 0o600);
}

/// Regression, 2026-11-04: the bench Pi crash-looped on `--box-name`
/// without `--site` ("--box-name names this box in its site; it needs
/// --site NAME"). A name without a site is dropped, with a word why.
#[test]
fn a_box_name_without_a_site_is_dropped_and_the_box_starts() {
    let r = root();
    put(&r, "relay-url", URL);
    put(&r, "relay-token", TOKEN);
    put(&r, "box-name", "bench-pi");
    let b = boot(&r);
    assert_eq!(words(b.line.as_deref()), ["--relay", URL, "--relay-token", CRED]);
    assert!(b.stderr.contains("box-name needs a site"), "{}", b.stderr);
    box_accepts(b.line.as_deref());
}

/// Regression: on the boot after pairing the token is gone from the boot
/// partition but relay-url is not; the rebuilt line dropped --relay-token
/// (env already had it), and the box refuses --relay alone.
#[test]
fn every_later_boot_writes_the_same_line() {
    let r = root();
    put(&r, "relay-url", URL);
    put(&r, "relay-token", TOKEN);
    put(&r, "site", "line-a");
    let first = boot(&r).line;
    assert!(!r.join("boot/firmware/helmsman/relay-token").exists());
    for _ in 0..2 {
        let again = boot(&r).line;
        assert_eq!(again, first);
        box_accepts(again.as_deref());
    }
}

#[test]
fn half_a_relay_pair_sets_no_relay() {
    let r = root();
    put(&r, "relay-url", URL);
    let b = boot(&r);
    assert!(
        !words(b.line.as_deref()).contains(&"--relay".to_string()),
        "{:?}",
        b.line
    );
    assert!(b.stderr.contains("without a token"), "{}", b.stderr);
    box_accepts(b.line.as_deref());

    let r = root();
    put(&r, "relay-token", TOKEN);
    let b = boot(&r);
    assert!(
        !words(b.line.as_deref()).iter().any(|w| w.starts_with("--relay")),
        "{:?}",
        b.line
    );
    assert!(b.stderr.contains("without relay-url"), "{}", b.stderr);
    box_accepts(b.line.as_deref());
}

#[test]
fn hostile_values_are_refused_and_never_stop_the_box() {
    for url in [
        "https://",
        "https://:443",
        "https://a:0",
        "https://a:99999",
        "https://a b",
        "https://u@a",
        "https://a/x?y",
        "https://a/\"$(reboot)\"",
        "http://a",
        "https://a:44x",
    ] {
        let r = root();
        put(&r, "relay-url", url);
        put(&r, "relay-token", TOKEN);
        let b = boot(&r);
        assert!(
            !words(b.line.as_deref()).contains(&"--relay".to_string()),
            "{url}: {:?}",
            b.line
        );
        box_accepts(b.line.as_deref());
    }
    for token in [
        "hmr1_",
        "hmr1_abc; reboot",
        "hmr1_ABC",
        "sk-ant-xyz",
        "hmr1_ab cd",
    ] {
        let r = root();
        put(&r, "relay-url", URL);
        put(&r, "relay-token", token);
        let b = boot(&r);
        assert!(
            !r.join("etc/helmsman-plc/relay-token").exists(),
            "{token} was installed"
        );
        box_accepts(b.line.as_deref());
    }
}

fn pairing_text() -> impl Strategy<Value = String> {
    // Printable ASCII with the shell's and systemd's specials, tabs, CR,
    // a newline (only the first line counts) and some non-ASCII.
    "[ -~\t\r\n\u{e9}\u{4e2d}]{0,70}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    /// Whatever sits in the four files, the box starts with the line.
    #[test]
    fn any_pairing_files_leave_a_line_the_box_accepts(
        url in prop_oneof![pairing_text(), pairing_text().prop_map(|s| format!("https://{s}"))],
        token in prop_oneof![Just(TOKEN.to_string()), pairing_text()],
        site in proptest::option::of(pairing_text()),
        name in proptest::option::of(pairing_text()),
    ) {
        let r = root();
        put(&r, "relay-url", &url);
        put(&r, "relay-token", &token);
        if let Some(site) = &site { put(&r, "site", site); }
        if let Some(name) = &name { put(&r, "box-name", name); }
        let first = boot(&r).line;
        box_accepts(first.as_deref());
        let again = boot(&r).line;
        prop_assert_eq!(&again, &first);
        box_accepts(again.as_deref());
    }
}
