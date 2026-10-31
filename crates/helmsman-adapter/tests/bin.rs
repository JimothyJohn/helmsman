//! The `helmsman-adapter` binary as CI uses it: an `OpENer`-sample
//! stand-in that echoes the scanner's output back as input.

use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use helmsman_proto::scanner::{IoConfig, IoConnection};
use helmsman_proto::session::Session;

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts the binary on free loopback ports and reads back where it bound.
fn spawn(peer_io_port: u16) -> (Running, SocketAddrV4, SocketAddrV4) {
    spawn_with(peer_io_port, &[])
}

fn spawn_with(peer_io_port: u16, extra: &[&str]) -> (Running, SocketAddrV4, SocketAddrV4) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-adapter"))
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--peer-io-port", &peer_io_port.to_string()])
        .args(extra)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    // "helmsman-adapter: CIP on 127.0.0.1:N (I/O 127.0.0.1:M)"
    let addrs: Vec<SocketAddrV4> = line
        .split([' ', '(', ')'])
        .filter_map(|w| w.parse().ok())
        .collect();
    assert_eq!(addrs.len(), 2, "unexpected banner: {line:?}");
    (Running(child), addrs[0], addrs[1])
}

#[test]
fn output_comes_back_as_input() {
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_bin, encap, io) = spawn(scanner_sock.local_addr().unwrap().port());
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();
    let cfg = IoConfig::exclusive_owner(151, 150, 100, 32, 32, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();

    let pattern: Vec<u8> = (1..=32).collect();
    conn.set_output(&pattern).unwrap();
    let got = conn
        .wait_for_input_eq(&pattern, Duration::from_secs(3))
        .expect("output never came back as input");
    assert_eq!(got.data, pattern);
    assert_eq!(conn.packets_ignored(), 0);
    conn.close(&mut s).unwrap();
    s.close();
}

/// `--profile moxa-e1213`: the E1213's assemblies, DO-00..03 wired to
/// DI-00..03, DO status in byte 50; and its RPI floor.
#[test]
fn the_e1213_profile_loops_its_outputs_back() {
    let scanner_sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (_bin, encap, io) = spawn_with(0, &["--profile", "moxa-e1213"]);
    let mut s = Session::connect(encap, Duration::from_secs(2)).unwrap();

    let fast = IoConfig::exclusive_owner(100, 102, 101, 1, 51, Duration::from_millis(10));
    let refused = IoConnection::open(&mut s, &scanner_sock, io, fast).unwrap_err();
    assert!(refused.to_string().contains("0x0111"), "{refused}");

    let cfg = IoConfig::exclusive_owner(100, 102, 101, 1, 51, Duration::from_millis(100));
    let conn = IoConnection::open(&mut s, &scanner_sock, io, cfg).unwrap();
    conn.set_output(&[0b1000_0101]).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let got = conn.wait_for_input(Duration::from_secs(1)).expect("no input");
        assert_eq!(got.data.len(), 51);
        // DI-00 and DI-02 from the wires (DO-07 is not wired), DO status.
        if got.data[0] == 0b101 && got.data[50] == 0b1000_0101 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "input {:?}", got.data);
    }
    conn.close(&mut s).unwrap();
    s.close();
}

#[test]
fn bad_flags_exit_2_with_usage() {
    for args in [
        &["--encap-port", "x"][..],
        &["--bogus", "1"],
        &["--bind"],
        &["--profile", "acme"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_helmsman-adapter"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("usage:"),
            "{args:?}"
        );
    }
}
