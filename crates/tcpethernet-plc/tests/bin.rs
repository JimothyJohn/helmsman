//! The `tcpethernet-plc` binary as a scanner sees it: a Structured Text
//! program, or a ladder project, behind a real CIP adapter on loopback.

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::time::{Duration, Instant};

use tcpethernet::cip::class;
use tcpethernet::scanner::{Input, IoConfig, IoConnection};
use tcpethernet::session::Session;

struct Running {
    child: Child,
    stderr: BufReader<ChildStderr>,
}

impl Running {
    /// Kills the process and returns what it wrote to stderr after the banner.
    fn finish(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut rest = String::new();
        let _ = self.stderr.read_to_string(&mut rest);
        rest
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn program_file(name: &str, src: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.st"));
    std::fs::write(&path, src).unwrap();
    path
}

fn ladder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tcpethernet-iec/tests/plcopen/motor.xml")
}

fn plc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tcpethernet-plc"))
}

/// Starts the PLC on free loopback ports; returns it and its CIP and I/O addresses.
fn spawn(program: &Path, peer_io_port: u16) -> (Running, SocketAddrV4, SocketAddrV4) {
    spawn_with(&[program.as_os_str()], peer_io_port)
}

/// [`spawn`] with any program and `--module` arguments.
fn spawn_with(args: &[&std::ffi::OsStr], peer_io_port: u16) -> (Running, SocketAddrV4, SocketAddrV4) {
    let mut child = plc()
        .args(args)
        .args([
            "--bind",
            "127.0.0.1",
            "--encap-port",
            "0",
            "--io-port",
            "0",
            "--scan-ms",
            "2",
        ])
        .args(["--peer-io-port", &peer_io_port.to_string()])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let addrs: Vec<SocketAddrV4> = line
        .split([' ', '(', ')', ','])
        .filter_map(|w| w.parse().ok())
        .collect();
    assert_eq!(addrs.len(), 2, "unexpected banner: {line:?}");
    (Running { child, stderr }, addrs[0], addrs[1])
}

fn wait_for(conn: &IoConnection, what: &str, mut ok: impl FnMut(&Input) -> bool) -> Input {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(input) = conn.latest_input()
            && ok(&input)
        {
            return input;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {what}; last input {:?}",
        conn.latest_input()
    );
}

#[test]
fn program_drives_class1_io() {
    let prog = program_file(
        "drives_io",
        "PROGRAM io_test
         VAR
           a AT %ID0 : UDINT;  r AT %QD0 : UDINT;
           n : UINT;           count AT %QW3 : UINT;
         END_VAR
           r := a + 1;
           %QX4.0 := NOT %IX4.0;
           n := n + 1;
           count := n;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn(&prog, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 7).unwrap()[1..],
        *b"tcpethernet PLC"
    );
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();

    let mut out = [0u8; 32];
    out[..4].copy_from_slice(&41u32.to_le_bytes());
    conn.set_output(&out).unwrap();
    let first = wait_for(&conn, "r = a + 1 and the inverted bit", |i| {
        i.data[..4] == 42u32.to_le_bytes() && i.data[4] & 1 == 1
    });
    let count = |i: &Input| u16::from_le_bytes([i.data[6], i.data[7]]);
    let c0 = count(&first);
    wait_for(&conn, "the scan counter to advance", |i| count(i) > c0);

    out[4] = 1;
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the bit to follow its input", |i| i.data[4] & 1 == 0);
    assert_eq!(conn.packets_ignored(), 0);
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn a_fault_zeroes_outputs_and_keeps_the_adapter_up() {
    let prog = program_file(
        "faults",
        "PROGRAM faulty
         VAR trigger AT %IX0.0 : BOOL; d AT %IB2 : USINT; q AT %QB1 : USINT; END_VAR
           %QB0 := 16#FF;
           IF trigger THEN
             q := 10 / d;
           END_IF;
         END_PROGRAM",
    );
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (plc, encap, io) = spawn(&prog, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0; 32]).unwrap();
    wait_for(&conn, "the program to run", |i| i.data[0] == 0xFF);

    let mut out = [0u8; 32];
    out[0] = 1; // trigger, with d = 0
    conn.set_output(&out).unwrap();
    wait_for(&conn, "outputs zeroed after the fault", |i| {
        i.data.iter().all(|&b| b == 0)
    });

    // Still on the network: explicit messaging answers.
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 1).unwrap(),
        0xFFFFu16.to_le_bytes()
    );
    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    assert!(
        log.contains("STOP, outputs zeroed") && log.contains("faults.st: line 5: integer division by zero"),
        "{log}"
    );
}

#[test]
fn modules_share_the_image_and_a_fault_stops_only_its_module() {
    // `counter` owns output byte 0 and counts; `faulty` owns bytes 1..3 and
    // divides by its input byte 2 when bit 0.0 is set. Both read all inputs.
    let counter = program_file(
        "mm_counter",
        "PROGRAM counter
         VAR n : USINT; o AT %QB0 : USINT; END_VAR
           n := n + 1;
           o := n;
         END_PROGRAM",
    );
    let faulty = program_file(
        "mm_faulty",
        "PROGRAM faulty
         VAR trigger AT %IX0.0 : BOOL; d AT %IB2 : USINT; q AT %QB1 : USINT; END_VAR
           %QB0 := 16#FF;
           IF trigger THEN
             q := 10 / d;
           END_IF;
         END_PROGRAM",
    );
    let counter_spec = format!("{},scan=2,q=0+1", counter.display());
    let faulty_spec = format!("{},name=bad,scan=5,q=1+2,m=16", faulty.display());
    let args = ["--module", &counter_spec, "--module", &faulty_spec].map(std::ffi::OsStr::new);
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (plc, encap, io) = spawn_with(&args, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0; 32]).unwrap();
    let first = wait_for(&conn, "both modules to run", |i| {
        i.data[1] == 0xFF && i.data[0] > 0
    });
    assert!(
        first.data[3..].iter().all(|&b| b == 0),
        "unowned bytes: {:?}",
        first.data
    );

    let mut out = [0u8; 32];
    out[0] = 1; // trigger, with d = 0
    conn.set_output(&out).unwrap();
    let after = wait_for(&conn, "bad's outputs zeroed", |i| i.data[1..3] == [0, 0]);
    let c0 = after.data[0];
    wait_for(&conn, "counter to keep counting", |i| i.data[0] != c0);

    conn.close(&mut s).unwrap();
    s.close();
    let log = plc.finish();
    assert!(
        log.contains("STOP, outputs zeroed")
            && log.contains("mm_faulty.st: line 5: integer division by zero (module bad)"),
        "{log}"
    );
    assert!(!log.contains("module mm_counter"), "only bad stopped: {log}");
}

#[test]
fn modules_fighting_for_an_output_are_refused_before_the_adapter_starts() {
    let prog = program_file(
        "fight",
        "PROGRAM p VAR o AT %QB0 : BYTE; END_VAR o := 1; END_PROGRAM",
    );
    // Two bare programs both default to the whole output image.
    let out = plc()
        .arg(&prog)
        .arg(&prog)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("module \"fight\" defined twice"), "{err}");
    assert!(err.contains("fight and fight drive the same %Q bytes"), "{err}");
    assert!(
        !err.contains("CIP on"),
        "refused before going on the network: {err}"
    );

    let a = format!("{},name=a,q=0+4", prog.display());
    let b = format!("{},name=b,q=3+4", prog.display());
    let out = plc()
        .args([
            "--module",
            &a,
            "--module",
            &b,
            "--encap-port",
            "0",
            "--io-port",
            "0",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("a and b drive the same %Q bytes"), "{err}");

    for (spec, why) in [
        (
            format!("{},q=30+4", prog.display()),
            "%Q window reaches past the image",
        ),
        (format!("{},q=0+1,bogus=1", prog.display()), "unknown key"),
        (format!("{},name=a\nb", prog.display()), "--module name"),
        (format!("{},scan=0", prog.display()), "--module scan"),
        (",q=0+1".into(), "needs a path first"),
    ] {
        let out = plc()
            .args(["--module", &spec, "--encap-port", "0", "--io-port", "0"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{spec}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(why), "{spec}: {err}");
    }
}

#[test]
fn compile_errors_exit_1_with_file_line_col() {
    let prog = program_file(
        "bad",
        "PROGRAM bad\nVAR x : SINT; END_VAR\n  x := 300;\n  y := 1;\nEND_PROGRAM",
    );
    let out = plc()
        .arg(&prog)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bad.st:3:8: 300 does not fit SINT"), "{err}");
    assert!(err.contains("bad.st:4:3: undeclared variable Y"), "{err}");
}

#[test]
fn bad_arguments_exit_2_with_usage() {
    for args in [
        &[][..],
        &["--module"],
        &["a.st", "--scan-ms", "0"],
        &["a.st", "--serial", "xyz"],
        &["a.st", "--memory", "999999"],
        &["a.st", "--bogus", "1"],
    ] {
        let out = plc().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("usage:"),
            "{args:?}"
        );
    }
    let out = plc().arg("/nonexistent/x.st").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn shipped_echo_program_echoes() {
    let prog = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("programs/echo.st");
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn(&prog, scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    let pattern: Vec<u8> = (1..=32).collect();
    conn.set_output(&pattern).unwrap();
    conn.wait_for_input_eq(&pattern, Duration::from_secs(3))
        .expect("echo.st did not echo all 32 bytes");
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn ladder_project_drives_class1_io() {
    // motor.xml: Start = %IX0.0, Stop = %IX0.1, Lamp = %QX0.0 (sealed in).
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_plc, encap, io) = spawn(&ladder(), scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    let mut out = [0u8; 32];
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the lamp off", |i| i.data[0] & 1 == 0);
    out[0] = 0b01; // Start
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the lamp on", |i| i.data[0] & 1 == 1);
    out[0] = 0;
    conn.set_output(&out).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    wait_for(&conn, "the lamp sealed in", |i| i.data[0] & 1 == 1);
    out[0] = 0b10; // Stop
    conn.set_output(&out).unwrap();
    wait_for(&conn, "the lamp off after Stop", |i| i.data[0] & 1 == 0);
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn emit_st_prints_the_lowered_project() {
    let out = plc().arg(ladder()).arg("--emit-st").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let st = String::from_utf8_lossy(&out.stdout);
    assert!(st.starts_with("PROGRAM motor\n"), "{st}");
    assert!(st.contains("delay(IN := Run, PT := T#2s);"), "{st}");

    let st_prog = program_file("not_xml", "PROGRAM p END_PROGRAM");
    let out = plc().arg(&st_prog).arg("--emit-st").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn project_errors_exit_1_with_xml_positions() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bad.xml");
    let xml = std::fs::read_to_string(ladder())
        .unwrap()
        .replace("<variable>Stop</variable>", "<variable>Stopp</variable>");
    std::fs::write(&path, &xml).unwrap();
    let line = xml
        .lines()
        .position(|l| l.contains("<contact localId=\"4\""))
        .unwrap()
        + 1;
    let out = plc()
        .arg(&path)
        .args(["--encap-port", "0", "--io-port", "0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("bad.xml:{line}:13: undeclared variable STOPP")),
        "{err}"
    );
}
