//! Class 1 I/O end to end against a loopback target that speaks the
//! encapsulation layer, answers `Forward_Open`/`Forward_Close` through our
//! codec, and echoes O->T data back as T->O like `OpENer`'s sample app does
//! (`sampleapplication.c` copies the output assembly into the input one).
//! Real sockets, real threads, no mocks.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use helmsman_proto::cip::{self, GeneralStatus};
use helmsman_proto::codec::{Reader, Writer};
use helmsman_proto::encap::{Command, Frame, HEADER_LEN, Header};
use helmsman_proto::explicit;
use helmsman_proto::io::{self, Class1Body, ForwardClose, ForwardOpen, ForwardOpenReply, IoPacket};
use helmsman_proto::scanner::{Error, IoConfig, IoConnection};
use helmsman_proto::session::Session;

const SESSION: u32 = 0x77;
const DATA_LEN: u16 = 4;

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = [0u8; HEADER_LEN];
    stream.read_exact(&mut head).ok()?;
    let header = Header::decode(&mut Reader::new(&head)).ok()?;
    let mut out = head.to_vec();
    out.resize(HEADER_LEN + usize::from(header.length), 0);
    stream.read_exact(&mut out[HEADER_LEN..]).ok()?;
    Some(out)
}

struct Target {
    tcp_port: u16,
    udp_port: u16,
    /// Refuse every `Forward_Open` with this extended status (and the size it wants).
    refuse: Option<(u16, u16)>,
}

/// Serves one session. Returns (tcp port, udp port, join handle).
#[allow(clippy::too_many_lines)]
fn spawn_target(refuse: Option<(u16, u16)>) -> (Target, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let tcp_port = listener.local_addr().unwrap().port();
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let udp_port = udp.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let mut io_thread: Option<thread::JoinHandle<()>> = None;
        let mut open: Option<ForwardOpen> = None;
        while let Some(bytes) = read_frame(&mut stream) {
            let (frame, _) = Frame::decode(&bytes).expect("malformed frame from originator");
            let ctx = frame.header.sender_context;
            let reply = match Command::from_u16(frame.header.command) {
                Some(Command::RegisterSession) => {
                    let mut h = Header::request(Command::RegisterSession, 4, ctx);
                    h.session_handle = SESSION;
                    let mut out = h.encode().to_vec();
                    out.extend_from_slice(&[1, 0, 0, 0]);
                    out
                }
                Some(Command::UnregisterSession) => break,
                Some(Command::SendRrData) => {
                    let req = explicit::decode_send_rr_data_request(&frame).expect("bad SendRRData");
                    let resp = match req.service {
                        io::service::FORWARD_OPEN => {
                            let fo = ForwardOpen::decode(&req.data).expect("bad Forward_Open");
                            if let Some((code, size)) = refuse {
                                let mut r =
                                    cip::Response::error(req.service, GeneralStatus::CONNECTION_FAILURE);
                                r.additional_status = vec![code, size];
                                let mut w = Writer::default();
                                w.u16_le(fo.connection_serial)
                                    .u16_le(fo.originator_vendor_id)
                                    .u32_le(fo.originator_serial)
                                    .u8(0)
                                    .u8(0);
                                r.data = w.into_inner();
                                r
                            } else {
                                assert_eq!(fo.transport, io::Transport::CLASS1_CYCLIC);
                                assert_eq!(fo.path.class, cip::class::ASSEMBLY);
                                let reply = ForwardOpenReply {
                                    o_to_t_connection_id: 0x0001_0001,
                                    t_to_o_connection_id: fo.t_to_o_connection_id,
                                    connection_serial: fo.connection_serial,
                                    originator_vendor_id: fo.originator_vendor_id,
                                    originator_serial: fo.originator_serial,
                                    o_to_t_api_us: fo.o_to_t_rpi_us,
                                    t_to_o_api_us: fo.t_to_o_rpi_us,
                                    application_reply: vec![],
                                };
                                // I/O thread: consume O->T, echo as T->O to wherever it came from.
                                let udp = udp.try_clone().unwrap();
                                let stop = Arc::clone(&stop);
                                let has_header = fo.o_to_t.size > 2 + DATA_LEN;
                                udp.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
                                io_thread = Some(thread::spawn(move || {
                                    let mut buf = [0u8; 1500];
                                    let mut seq: u16 = 0;
                                    let mut encap: u32 = 0;
                                    while !stop.load(Ordering::Relaxed) {
                                        let Ok((n, from)) = udp.recv_from(&mut buf) else {
                                            continue;
                                        };
                                        let pkt = IoPacket::decode(&buf[..n]).expect("bad O->T packet");
                                        assert_eq!(pkt.connection_id, 0x0001_0001);
                                        let body = Class1Body::decode(pkt.body, has_header).unwrap();
                                        let echo = if body.data.is_empty() {
                                            vec![0xEE; 4]
                                        } else {
                                            body.data.to_vec()
                                        };
                                        seq = seq.wrapping_add(1);
                                        encap = encap.wrapping_add(1);
                                        let out = Class1Body {
                                            sequence_count: seq,
                                            run_idle: None,
                                            data: &echo,
                                        }
                                        .encode();
                                        let out = IoPacket {
                                            connection_id: fo.t_to_o_connection_id,
                                            encapsulation_sequence: Some(encap),
                                            body: &out,
                                        }
                                        .encode();
                                        udp.send_to(&out, from).unwrap();
                                    }
                                }));
                                open = Some(fo);
                                let mut w = Writer::default();
                                reply.encode(&mut w);
                                cip::Response::ok(req.service, w.into_inner())
                            }
                        }
                        io::service::FORWARD_CLOSE => {
                            let fc = ForwardClose::decode(&req.data).expect("bad Forward_Close");
                            let o = open.take().expect("close without open");
                            assert_eq!(fc.connection_serial, o.connection_serial);
                            assert_eq!(fc.path, o.path);
                            stop.store(true, Ordering::Relaxed);
                            if let Some(h) = io_thread.take() {
                                h.join().unwrap();
                            }
                            let mut w = Writer::default();
                            io::ForwardCloseReply {
                                connection_serial: fc.connection_serial,
                                originator_vendor_id: fc.originator_vendor_id,
                                originator_serial: fc.originator_serial,
                                application_reply: vec![],
                            }
                            .encode(&mut w);
                            cip::Response::ok(req.service, w.into_inner())
                        }
                        _ => cip::Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED),
                    };
                    explicit::encode_send_rr_data_reply(SESSION, ctx, &resp)
                }
                _ => {
                    let mut h = Header::request(Command::Nop, 0, ctx);
                    h.command = frame.header.command;
                    h.status = 1;
                    h.encode().to_vec()
                }
            };
            stream.write_all(&reply).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        if let Some(h) = io_thread.take() {
            h.join().unwrap();
        }
    });
    (
        Target {
            tcp_port,
            udp_port,
            refuse,
        },
        handle,
    )
}

#[test]
fn exclusive_owner_round_trip_echoes_output_into_input() {
    let (t, handle) = spawn_target(None);
    let mut s = Session::connect((Ipv4Addr::LOCALHOST, t.tcp_port), Duration::from_secs(2)).unwrap();
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target = SocketAddrV4::new(Ipv4Addr::LOCALHOST, t.udp_port);
    let cfg = IoConfig::exclusive_owner(151, 150, 100, DATA_LEN, DATA_LEN, Duration::from_millis(10));
    let conn = IoConnection::open(&mut s, &sock, target, cfg).unwrap();
    assert_eq!(conn.reply().o_to_t_connection_id, 0x0001_0001);

    let first = conn
        .wait_for_input(Duration::from_secs(2))
        .expect("no T->O within 2s");
    assert_eq!(first.data, vec![0, 0, 0, 0]);

    conn.set_output(&[9, 8, 7, 6]).unwrap();
    let echoed = conn
        .wait_for_input_eq(&[9, 8, 7, 6], Duration::from_secs(2))
        .expect("no echo");
    assert_eq!(echoed.data, vec![9, 8, 7, 6]);
    assert!(conn.is_alive());
    assert!(conn.packets_sent() >= 2);
    assert_eq!(conn.packets_ignored(), 0);
    assert!(conn.set_output(&[1]).is_err());

    let close = conn.close(&mut s).unwrap();
    assert_eq!(close.originator_vendor_id, 0xFFFF);
    s.close();
    handle.join().unwrap();
}

#[test]
fn input_only_sends_heartbeats_and_receives_data() {
    let (t, handle) = spawn_target(None);
    let mut s = Session::connect((Ipv4Addr::LOCALHOST, t.tcp_port), Duration::from_secs(2)).unwrap();
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target = SocketAddrV4::new(Ipv4Addr::LOCALHOST, t.udp_port);
    let cfg = IoConfig::input_only(151, 152, 100, DATA_LEN, Duration::from_millis(10));
    assert_eq!(cfg.timeout(), Duration::from_millis(80));
    let conn = IoConnection::open(&mut s, &sock, target, cfg).unwrap();
    let input = conn
        .wait_for_input(Duration::from_secs(2))
        .expect("no T->O within 2s");
    assert_eq!(input.data, vec![0xEE; 4]);
    conn.close(&mut s).unwrap();
    s.close();
    handle.join().unwrap();
}

#[test]
fn refusal_carries_the_extended_status_and_wanted_size() {
    let (t, handle) = spawn_target(Some((io::extended::INVALID_O_TO_T_SIZE, 38)));
    let mut s = Session::connect((Ipv4Addr::LOCALHOST, t.tcp_port), Duration::from_secs(2)).unwrap();
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target = SocketAddrV4::new(Ipv4Addr::LOCALHOST, t.udp_port);
    let cfg = IoConfig::exclusive_owner(151, 150, 100, DATA_LEN, DATA_LEN, Duration::from_millis(10));
    let err = IoConnection::open(&mut s, &sock, target, cfg).unwrap_err();
    match &err {
        Error::Refused(r) => {
            assert_eq!(r.extended_code(), Some(0x0127));
            assert_eq!(r.extended, vec![0x0127, 38]);
            assert_eq!(r.originator_vendor_id, 0xFFFF);
        }
        other => panic!("expected Refused, got {other}"),
    }
    assert_eq!(
        err.to_string(),
        "forward open refused: 0x01 connection failure extended 0x0127 [0026]"
    );
    assert!(t.refuse.is_some());
    s.close();
    handle.join().unwrap();
}
