//! Runs the real binary against loopback responders on ephemeral ports
//! (`--port`): a UDP `ListIdentity` responder and a TCP target answering
//! Identity attributes. Bytes go through the kernel; nothing is mocked.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, UdpSocket};
use std::process::Command;
use std::thread;
use std::time::Duration;

use helmsman_proto::cip::{self, GeneralStatus};
use helmsman_proto::codec::{Reader, Writer};
use helmsman_proto::encap::{Command as Enc, Frame, HEADER_LEN, Header};
use helmsman_proto::explicit;
use helmsman_proto::identity::{IdentityItem, SocketAddress, encode_list_identity};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_helmsman"))
}

fn closed_port() -> u16 {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn help_and_bad_args() {
    let out = bin().arg("help").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("helmsman discover"));
    let out = bin().args(["get", "nope", "1.1.1"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an IPv4 address"));
}

#[test]
fn get_against_a_closed_port_fails_cleanly() {
    let port = closed_port().to_string();
    let out = bin()
        .args(["--port", &port, "get", "127.0.0.1", "1.1.1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).starts_with("error: io:"),
        "{out:?}"
    );
}

#[test]
fn discover_prints_a_loopback_responder() {
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = sock.local_addr().unwrap().port();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let handle = thread::spawn(move || {
        let mut buf = [0u8; 1500];
        let (n, from) = sock.recv_from(&mut buf).expect("CLI never sent ListIdentity");
        let (frame, _) = Frame::decode(&buf[..n]).unwrap();
        let id = IdentityItem {
            protocol_version: 1,
            socket_address: SocketAddress {
                family: 2,
                port,
                addr: Ipv4Addr::LOCALHOST,
            },
            vendor_id: 1,
            device_type: 12,
            product_code: 65001,
            revision_major: 2,
            revision_minor: 3,
            status: 0,
            serial_number: 0x0102_0304,
            product_name: b"cli test".to_vec(),
            state: 3,
        };
        let reply = Frame::encode(
            Enc::ListIdentity,
            0,
            frame.header.sender_context,
            &encode_list_identity(&[id]),
        );
        sock.send_to(&reply, from).unwrap();
    });
    let out = bin()
        .args([
            "--port",
            &port.to_string(),
            "discover",
            "127.0.0.1",
            "--window",
            "500",
        ])
        .output()
        .unwrap();
    handle.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim_end(),
        "127.0.0.1\tvendor=1 type=0x000C product=65001 rev=2.3 serial=0x01020304\tcli test",
        "{out:?}"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("1 device(s)"));
}

fn spawn_identity_target() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let t = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        loop {
            let mut head = [0u8; HEADER_LEN];
            if s.read_exact(&mut head).is_err() {
                return;
            }
            let h = Header::decode(&mut Reader::new(&head)).unwrap();
            let mut all = head.to_vec();
            all.resize(HEADER_LEN + usize::from(h.length), 0);
            s.read_exact(&mut all[HEADER_LEN..]).unwrap();
            let (frame, _) = Frame::decode(&all).unwrap();
            let ctx = frame.header.sender_context;
            let reply = match Enc::from_u16(frame.header.command) {
                Some(Enc::RegisterSession) => {
                    let mut hh = Header::request(Enc::RegisterSession, 4, ctx);
                    hh.session_handle = 1;
                    let mut out = hh.encode().to_vec();
                    out.extend_from_slice(&[1, 0, 0, 0]);
                    out
                }
                Some(Enc::SendRrData) => {
                    let req = explicit::decode_send_rr_data_request(&frame).unwrap();
                    let mut w = Writer::default();
                    let resp = match req.path.attribute {
                        Some(1) => cip::Response::ok(req.service, 1u16.to_le_bytes().to_vec()),
                        Some(4) => cip::Response::ok(req.service, vec![2, 3]),
                        Some(6) => cip::Response::ok(req.service, 0xCAFE_F00Du32.to_le_bytes().to_vec()),
                        Some(7) => {
                            w.u8(3).bytes(b"abc");
                            cip::Response::ok(req.service, w.into_inner())
                        }
                        _ => cip::Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED),
                    };
                    explicit::encode_send_rr_data_reply(1, ctx, &resp)
                }
                _ => return,
            };
            s.write_all(&reply).unwrap();
        }
    });
    (port, t)
}

#[test]
fn identity_prints_each_attribute_and_errors_inline() {
    let (port, t) = spawn_identity_target();
    let out = bin()
        .args(["--port", &port.to_string(), "identity", "127.0.0.1"])
        .output()
        .unwrap();
    t.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "1\tvendor id\t1");
    assert!(
        lines[1].starts_with("2\tdevice type\terror: cip status 0x14"),
        "{stdout}"
    );
    assert_eq!(lines[3], "4\trevision\t2.3");
    assert_eq!(lines[5], "6\tserial number\t0xCAFEF00D");
    assert_eq!(lines[6], "7\tproduct name\tabc");
}

#[test]
fn get_prints_hex_and_a_cip_error_is_exit_1() {
    let (port, t) = spawn_identity_target();
    let out = bin()
        .args(["--port", &port.to_string(), "get", "127.0.0.1", "0x01.1.6"])
        .output()
        .unwrap();
    t.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "0d f0 fe ca");

    let (port, t) = spawn_identity_target();
    let out = bin()
        .args(["--port", &port.to_string(), "get", "127.0.0.1", "1.1.99"])
        .output()
        .unwrap();
    t.join().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("0x14 attribute not supported"),
        "{out:?}"
    );
}
