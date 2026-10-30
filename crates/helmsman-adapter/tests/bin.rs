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
    let mut child = Command::new(env!("CARGO_BIN_EXE_helmsman-adapter"))
        .args(["--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"])
        .args(["--peer-io-port", &peer_io_port.to_string()])
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

#[test]
fn bad_flags_exit_2_with_usage() {
    for args in [&["--encap-port", "x"][..], &["--bogus", "1"], &["--bind"]] {
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
