//! The `tcpethernet-plc` binary as a scanner sees it: a Structured Text
//! program behind a real CIP adapter on loopback.

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
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

fn plc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tcpethernet-plc"))
}

/// Starts the PLC on free loopback ports; returns it and its CIP and I/O addresses.
fn spawn(program: &PathBuf, peer_io_port: u16) -> (Running, SocketAddrV4, SocketAddrV4) {
    let mut child = plc()
        .arg(program)
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
        &["a.st", "b.st"],
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
