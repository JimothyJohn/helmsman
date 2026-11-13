//! RETAIN variables across a restart of the box: the files the writer
//! keeps, what a start restores from them and says about it, damaged and
//! oversize files, and the real binary killed mid-run and started again.

use std::io::{BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, State, Value, compile};
use helmsman_plc::retain::{self, MAX_FILE, Writer};
use helmsman_plc::{Module, Runtime, Window};

/// A fresh, empty directory of its own per call (parallel copies of the
/// suite included).
fn tmp(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "retain-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn module(name: &str, src: &str) -> Module {
    let sizes = ImageSizes { i: 0, q: 0, m: 0 };
    Module {
        name: name.into(),
        program: compile(src, sizes)
            .unwrap_or_else(|d| panic!("{name}: {d:?}"))
            .into(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

fn int(s: &State, name: &str) -> i128 {
    match s.values(name) {
        Some([Value::Int(n)]) => *n,
        v => panic!("{name}: {v:?}"),
    }
}

/// Saves `src` as module `name` after `scans` scans, through the writer.
fn saved_after(dir: &Path, name: &str, src: &str, scans: u64) {
    let mut rt = Runtime::new(0, 0, vec![module(name, src)]).unwrap();
    for k in 0..scans {
        rt.tick(Duration::from_millis(k * 10), &[]);
    }
    let mut w = Writer::start_with(dir.to_path_buf(), |l| panic!("{l}"));
    w.offer(Duration::ZERO, &rt);
    assert!(w.flush(Duration::from_secs(10)));
}

const COUNTER: &str = "PROGRAM c
VAR RETAIN n : UDINT; level : INT; END_VAR
VAR NON_RETAIN fresh : BOOL := TRUE; END_VAR
VAR plain : UDINT; END_VAR
  n := n + 1; plain := plain + 1; level := 3; fresh := FALSE;
END_PROGRAM";

/// A start restores RETAIN values by name and type and says which; the
/// `NON_RETAIN` and plain variables start at their initial values.
#[test]
fn a_start_restores_retain_values_and_resets_the_rest() {
    let dir = tmp("restore");
    saved_after(&dir, "c", COUNTER, 7);
    let mut modules = vec![module("c", COUNTER)];
    let lines = retain::restore(&dir, &mut modules);
    assert_eq!(lines, ["module c: RETAIN restored: N, LEVEL"]);
    let p = modules[0].program.st().unwrap();
    assert_eq!(p.get("n"), Some(Value::Int(7)));
    assert_eq!(p.get("level"), Some(Value::Int(3)));
    assert_eq!(p.get("fresh"), Some(Value::Bool(true)));
    assert_eq!(p.get("plain"), Some(Value::Int(0)));
}

/// A type change between restarts resets that variable with a message
/// naming it; a saved variable no longer RETAIN is dropped and named; a
/// new one is named; nothing crashes.
#[test]
fn a_type_change_between_restarts_is_listed_and_the_rest_restores() {
    let dir = tmp("types");
    saved_after(&dir, "c", COUNTER, 4);
    let changed = "PROGRAM c
        VAR RETAIN n : UDINT; level : REAL; extra : INT; END_VAR
        n := n + 1; END_PROGRAM";
    let mut modules = vec![module("c", changed)];
    let lines = retain::restore(&dir, &mut modules);
    assert_eq!(
        lines,
        [
            "module c: RETAIN restored: N",
            "module c: RETAIN LEVEL starts at its initial value: its type changed since it was saved",
            "module c: RETAIN EXTRA is new and starts at its initial value",
        ]
    );
    let p = modules[0].program.st().unwrap();
    assert_eq!(p.get("n"), Some(Value::Int(4)));
    assert_eq!(p.get("level"), Some(Value::Real(0.0)));

    let gone = "PROGRAM c VAR RETAIN n : UDINT; END_VAR VAR level : INT; END_VAR END_PROGRAM";
    let mut modules = vec![module("c", gone)];
    let lines = retain::restore(&dir, &mut modules);
    assert_eq!(
        lines[1],
        "module c: saved RETAIN LEVEL dropped: no longer declared RETAIN"
    );
    let p = modules[0].program.st().unwrap();
    assert_eq!(
        p.get("level"),
        Some(Value::Int(0)),
        "a plain VAR never takes a saved value"
    );
}

/// A damaged file (bad magic, truncated, a flipped bit, too big) is
/// reported, moved aside, and the module starts from its initial values.
#[test]
fn a_damaged_or_oversize_file_is_reported_moved_aside_and_ignored() {
    let dir = tmp("damaged");
    saved_after(&dir, "c", COUNTER, 5);
    let file = dir.join("c.ret");
    let good = std::fs::read(&file).unwrap();
    let mut flipped = good.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x10;
    let mut big = good.clone();
    big.resize(MAX_FILE + 1, 0);
    for (bytes, why) in [
        (b"nonsense".to_vec(), "not a RETAIN file (bad magic)"),
        (good[..good.len() - 3].to_vec(), "truncated"),
        (flipped, "its checksum does not match"),
        (big, "more than the 8388608-byte limit"),
        (Vec::new(), "not a RETAIN file"),
    ] {
        std::fs::write(&file, &bytes).unwrap();
        let mut modules = vec![module("c", COUNTER)];
        let lines = retain::restore(&dir, &mut modules);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("module c: saved RETAIN values ignored")
                && lines[0].contains(why)
                && lines[0].ends_with("starting from initial values"),
            "{why}: {}",
            lines[0]
        );
        assert_eq!(modules[0].program.st().unwrap().get("n"), Some(Value::Int(0)));
        assert!(!file.exists(), "moved aside");
        assert_eq!(std::fs::read(dir.join("c.ret.damaged")).unwrap(), bytes);
    }
}

/// The payload decodes, but not as a state: still damaged, never a panic.
#[test]
fn a_well_framed_file_with_a_bad_payload_is_refused() {
    let mut bytes = b"HRT1".to_vec();
    let payload = [9u8, 0, 0, 0, 0];
    bytes.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend(helmsman_plc::sink::queue::crc32(&payload).to_le_bytes());
    bytes.extend(payload);
    let e = retain::decode(&bytes).unwrap_err();
    assert!(e.starts_with("does not decode: an unknown version"), "{e}");
}

/// The writer writes a module's file when its values change, not when
/// they sit still; it removes the file of a module that no longer has
/// RETAIN variables or is gone; and it offers at most every
/// [`retain::EVERY`].
#[test]
fn the_writer_writes_on_change_at_a_bounded_rate_and_removes_stale_files() {
    let dir = tmp("writer");
    let still = "PROGRAM s VAR RETAIN k : INT := 4; END_VAR END_PROGRAM";
    let mut rt = Runtime::new(0, 0, vec![module("c", COUNTER), module("s", still)]).unwrap();
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&said);
    let mut w = Writer::start_with(dir.clone(), move |l| log.lock().unwrap().push(l.to_string()));
    rt.tick(Duration::ZERO, &[]);
    w.offer(Duration::ZERO, &rt);
    assert!(w.flush(Duration::from_secs(10)));
    let read = |m| retain::read(&dir, m).unwrap().unwrap();
    assert_eq!(int(&read("c"), "N"), 1);
    assert_eq!(int(&read("s"), "K"), 4);

    // Within EVERY of the last offer: nothing is handed over.
    rt.tick(Duration::from_millis(10), &[]);
    w.offer(Duration::from_millis(500), &rt);
    assert!(w.flush(Duration::from_secs(10)));
    assert_eq!(int(&read("c"), "N"), 1);

    // After it: the changed module is written, the still one is not (its
    // file, removed behind the writer's back, stays gone).
    std::fs::remove_file(dir.join("s.ret")).unwrap();
    w.offer(retain::EVERY, &rt);
    assert!(w.flush(Duration::from_secs(10)));
    assert_eq!(int(&read("c"), "N"), 2);
    assert!(
        !dir.join("s.ret").exists(),
        "unchanged values are not written again"
    );

    // A deploy that drops RETAIN from `c` and removes `s`.
    let mut table = helmsman_plc::SignalTable::default();
    rt.reconfigure(
        vec![module("c", "PROGRAM c VAR n : UDINT; END_VAR END_PROGRAM")],
        &mut table,
    )
    .unwrap();
    std::fs::write(dir.join("s.ret"), b"x").unwrap();
    w.offer(retain::EVERY * 2, &rt);
    assert!(w.flush(Duration::from_secs(10)));
    assert!(!dir.join("c.ret").exists(), "no RETAIN, no file");
    assert!(!dir.join("s.ret").exists(), "a removed module's file goes");
    assert!(said.lock().unwrap().is_empty(), "{:?}", said.lock().unwrap());
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert!(leftovers.is_empty(), "no temporary files left: {leftovers:?}");
}

/// A write that fails (the folder is a file) is reported once, not every
/// second, and the box carries on.
#[test]
fn a_failing_write_is_reported_once() {
    let dir = tmp("failing").join("retain");
    std::fs::write(&dir, b"in the way").unwrap();
    let mut rt = Runtime::new(0, 0, vec![module("c", COUNTER)]).unwrap();
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&said);
    let mut w = Writer::start_with(dir, move |l| log.lock().unwrap().push(l.to_string()));
    for k in 0..3 {
        rt.tick(Duration::from_millis(k * 10), &[]);
        w.offer(retain::EVERY * u32::try_from(k).unwrap(), &rt);
        assert!(w.flush(Duration::from_secs(10)));
    }
    let said = said.lock().unwrap();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with("module c: RETAIN values not saved:"),
        "{}",
        said[0]
    );
}

// ---- the real binary ----

const EXIT_WITH_STDIN: &str = "HELMSMAN_EXIT_WITH_STDIN";

/// Counts every scan into a RETAIN counter; at each start notes the count
/// it started from, and counts the starts its `NON_RETAIN` and plain flags
/// saw (each is TRUE only on a fresh start).
const BOX_COUNTER: &str = "FUNCTION_BLOCK pulse VAR RETAIN n : INT; END_VAR n := n + 1; END_FUNCTION_BLOCK
PROGRAM counter
VAR RETAIN
  count : UDINT;
  at_boot : UDINT;
  fresh_starts : UDINT;
  plain_starts : UDINT;
END_VAR
VAR NON_RETAIN fresh : BOOL := TRUE; END_VAR
VAR plain : BOOL := TRUE; beat : pulse; END_VAR
  beat();
  IF fresh THEN fresh_starts := fresh_starts + 1; at_boot := count; fresh := FALSE; END_IF;
  IF plain THEN plain_starts := plain_starts + 1; plain := FALSE; END_IF;
  count := count + 1;
END_PROGRAM";

struct Running {
    child: Child,
    stderr: BufReader<ChildStderr>,
}

impl Running {
    /// SIGKILL: no clean shutdown, no last write.
    fn kill(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut rest = String::new();
        let _ = self.stderr.read_to_string(&mut rest);
        rest
    }
}

impl Running {
    /// SIGTERM, then waits (with a deadline) for the box to exit on its
    /// own: its exit status and what it printed.
    fn terminate(mut self) -> (std::process::ExitStatus, String) {
        let pid = self.child.id().to_string();
        let sent = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
        assert!(sent.success(), "kill -TERM {pid}");
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "the box did not exit on SIGTERM");
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut rest = String::new();
        let _ = self.stderr.read_to_string(&mut rest);
        (status, rest)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(program: &Path, state: &Path) -> Running {
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-plc"))
        .args(["--opcua", "off", "--api", "127.0.0.1:0"])
        .arg(program)
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--scan-ms", "2", "--state-dir"])
        .arg(state)
        .current_dir(tmp("cwd"))
        .env_remove("STATE_DIRECTORY")
        .env(EXIT_WITH_STDIN, "1")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = BufReader::new(child.stderr.take().unwrap());
    Running { child, stderr }
}

/// Waits (with a deadline, never a fixed sleep) until the module's saved
/// state satisfies `ok`, and returns it.
fn wait_saved(retain_dir: &Path, what: &str, ok: impl Fn(&State) -> bool) -> State {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(Some(s)) = retain::read(retain_dir, "counter")
            && ok(&s)
        {
            return s;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The box is killed outright mid-run and started again on the same state
/// directory: the counter resumes from exactly the value it last saved,
/// and the `NON_RETAIN` and plain variables start fresh. Then a restart with
/// the counter's type changed says so, starts it from zero and runs.
#[test]
fn a_killed_box_resumes_its_retain_counter_and_says_what_changed() {
    let dir = tmp("box");
    let program = dir.join("counter.st");
    std::fs::write(&program, BOX_COUNTER).unwrap();
    let state = dir.join("state");
    let retain_dir = state.join(retain::DIR);

    let first = start(&program, &state);
    wait_saved(&retain_dir, "the first run to count and save", |s| {
        int(s, "COUNT") >= 50
    });
    let log = first.kill();
    assert!(
        log.contains("module counter: no saved RETAIN values; starting from initial values"),
        "{log}"
    );
    assert!(
        log.contains("module counter: warning: 1:33: N: RETAIN inside a function block is not kept"),
        "a RETAIN the box does not keep is shown, and the box runs: {log}"
    );
    let saved = retain::read(&retain_dir, "counter").unwrap().unwrap();
    let kept = int(&saved, "COUNT");
    assert_eq!(int(&saved, "FRESH_STARTS"), 1);

    let second = start(&program, &state);
    let after = wait_saved(&retain_dir, "the second run to save", |s| {
        int(s, "FRESH_STARTS") == 2
    });
    assert_eq!(int(&after, "AT_BOOT"), kept, "resumed from what was saved");
    assert!(int(&after, "COUNT") > kept);
    assert_eq!(int(&after, "PLAIN_STARTS"), 2, "a plain VAR starts fresh");
    let log = second.kill();
    assert!(
        log.contains("module counter: RETAIN restored: COUNT, AT_BOOT, FRESH_STARTS, PLAIN_STARTS"),
        "{log}"
    );

    std::fs::write(&program, BOX_COUNTER.replace("count : UDINT", "count : UINT")).unwrap();
    let third = start(&program, &state);
    let after = wait_saved(&retain_dir, "the third run to save", |s| {
        int(s, "FRESH_STARTS") == 3
    });
    assert_eq!(int(&after, "AT_BOOT"), 0, "COUNT changed type: it starts at zero");
    let log = third.kill();
    assert!(
        log.contains(
            "module counter: RETAIN COUNT starts at its initial value: its type changed since it was saved"
        ),
        "{log}"
    );
}

/// A planned stop (SIGTERM, which launchctl and systemd send first) writes
/// the RETAIN values the writer had not yet reached, says so and exits 0:
/// the saved count is past the periodic write the test saw, and a restart
/// resumes from exactly it. (The SIGKILL test above is the negative path:
/// that one keeps only what the periodic write got to.)
#[test]
fn a_terminated_box_writes_its_retain_values_before_exiting() {
    let dir = tmp("term");
    let program = dir.join("counter.st");
    std::fs::write(&program, BOX_COUNTER).unwrap();
    let state = dir.join("state");
    let retain_dir = state.join(retain::DIR);

    let first = start(&program, &state);
    let before = wait_saved(&retain_dir, "the first periodic write", |s| int(s, "COUNT") >= 1);
    let (status, log) = first.terminate();
    assert!(status.success(), "a planned stop exits 0, got {status:?}: {log}");
    assert!(
        log.contains("helmsman-plc: SIGTERM: RETAIN values written, exiting"),
        "{log}"
    );
    let saved = retain::read(&retain_dir, "counter").unwrap().unwrap();
    let kept = int(&saved, "COUNT");
    assert!(
        kept > int(&before, "COUNT"),
        "the last scans were written at the stop, not left to the next periodic write: {kept} vs {}",
        int(&before, "COUNT")
    );

    let second = start(&program, &state);
    let after = wait_saved(&retain_dir, "the second run to save", |s| {
        int(s, "FRESH_STARTS") == 2
    });
    assert_eq!(int(&after, "AT_BOOT"), kept, "resumed from what the stop wrote");
    drop(second);
}
