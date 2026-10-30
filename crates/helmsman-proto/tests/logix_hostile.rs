//! `TagReader` against a controller that lies: replies for the wrong
//! service, unknown type codes, length lies, fragments that change type or
//! never finish, browses that loop, packets whose counts are wrong. Each
//! must end in an error, never a panic, a hang, or a wrong value.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use helmsman_proto::cip::{self, GeneralStatus};
use helmsman_proto::codec::{Reader, Writer};
use helmsman_proto::encap::{Command, Frame, HEADER_LEN, Header};
use helmsman_proto::logix::wire::{self, encode_read_data, join_multiple};
use helmsman_proto::logix::{AtomicType, DataType, Error, Symbol, TagPath, TagReader, Value};
use proptest::prelude::*;

const SESSION: u32 = 7;

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = [0u8; HEADER_LEN];
    stream.read_exact(&mut head).ok()?;
    let header = Header::decode(&mut Reader::new(&head)).ok()?;
    let mut out = head.to_vec();
    out.resize(HEADER_LEN + usize::from(header.length), 0);
    stream.read_exact(&mut out[HEADER_LEN..]).ok()?;
    Some(out)
}

/// The Message Router request inside a `SendRRData` request frame.
fn mr_of(frame: &Frame<'_>) -> Vec<u8> {
    let mut r = Reader::new(frame.payload);
    r.u32_le().unwrap();
    r.u16_le().unwrap();
    let items = helmsman_proto::cpf::decode(r.rest()).unwrap();
    items[1].data.to_vec()
}

fn reply_frame(ctx: [u8; 8], mr: &[u8]) -> Vec<u8> {
    // Build a SendRRData reply around raw MR reply bytes.
    let items = [
        helmsman_proto::cpf::Item {
            type_id: helmsman_proto::cpf::item::NULL_ADDRESS,
            data: &[],
        },
        helmsman_proto::cpf::Item {
            type_id: helmsman_proto::cpf::item::UNCONNECTED_DATA,
            data: mr,
        },
    ];
    let mut w = Writer::default();
    w.u32_le(0).u16_le(0);
    helmsman_proto::cpf::encode(&items, &mut w);
    Frame::encode(Command::SendRrData, SESSION, ctx, &w.into_inner())
}

/// Serve one client; `answer` maps each MR request to raw MR reply bytes.
fn serve<F>(mut answer: F) -> u16
where
    F: FnMut(&[u8]) -> Vec<u8> + Send + 'static,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        while let Some(bytes) = read_frame(&mut stream) {
            let (frame, _) = Frame::decode(&bytes).unwrap();
            let ctx = frame.header.sender_context;
            let out = match Command::from_u16(frame.header.command) {
                Some(Command::RegisterSession) => {
                    let mut h = Header::request(Command::RegisterSession, 4, ctx);
                    h.session_handle = SESSION;
                    let mut out = h.encode().to_vec();
                    out.extend_from_slice(&[1, 0, 0, 0]);
                    out
                }
                Some(Command::SendRrData) => reply_frame(ctx, &answer(&mr_of(&frame))),
                _ => return,
            };
            if stream.write_all(&out).is_err() {
                return;
            }
        }
    });
    port
}

fn connect(port: u16) -> TagReader {
    TagReader::connect((Ipv4Addr::LOCALHOST, port), Duration::from_secs(5)).unwrap()
}

fn mr(service: u8, status: u8, data: &[u8]) -> Vec<u8> {
    let mut w = Writer::default();
    cip::Response {
        service,
        status: GeneralStatus(status),
        additional_status: vec![],
        data: data.to_vec(),
    }
    .encode(&mut w);
    w.into_inner()
}

fn dint(v: i32) -> Vec<u8> {
    encode_read_data(DataType::Atomic(AtomicType::Dint), &v.to_le_bytes())
}

fn p(s: &str) -> TagPath {
    TagPath::parse(s).unwrap()
}

fn read_one(answer: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static) -> Result<Value, Error> {
    let mut r = connect(serve(answer));
    let out = r.read_tag(&p("X"), 1).map(|d| d.value().unwrap());
    r.close();
    out
}

#[test]
fn honest_reply_reads() {
    assert_eq!(read_one(|_| mr(0xCC, 0, &dint(5))).unwrap(), Value::Dint(5));
}

#[test]
fn reply_for_a_different_service_is_refused() {
    // A Write Tag reply, a Read Tag Fragmented reply, a Get_Attribute_Single reply.
    for svc in [0xCD, 0xD2, 0x8E] {
        let e = read_one(move |_| mr(svc, 0, &dint(5))).unwrap_err();
        assert!(matches!(e, Error::Decode(_)), "0x{svc:02X}: {e}");
    }
}

#[test]
fn request_echoed_back_is_not_a_reply() {
    let e = read_one(<[u8]>::to_vec).unwrap_err();
    assert!(matches!(e, Error::Session(_)), "{e}");
}

#[test]
fn unknown_or_cut_type_codes_are_refused() {
    for data in [
        vec![],
        vec![0xC4],
        vec![0xFF, 0x00, 1, 2, 3, 4],
        vec![0xA0, 0x02, 0x01],
    ] {
        let d = data.clone();
        let e = read_one(move |_| mr(0xCC, 0, &d)).unwrap_err();
        assert!(matches!(e, Error::Decode(_)), "{data:02X?}: {e}");
    }
}

#[test]
fn length_lies_are_refused() {
    let long = encode_read_data(DataType::Atomic(AtomicType::Dint), &[0; 8]);
    let short = encode_read_data(DataType::Atomic(AtomicType::Dint), &[0; 2]);
    for data in [long, short] {
        let e = read_one(move |_| mr(0xCC, 0, &data)).unwrap_err();
        assert!(matches!(e, Error::Protocol(_)), "{e}");
    }
}

#[test]
fn type_change_between_fragments_is_refused() {
    let mut n = 0;
    let port = serve(move |_| {
        n += 1;
        if n == 1 {
            mr(
                0xCC,
                0x06,
                &encode_read_data(DataType::Atomic(AtomicType::Dint), &[0; 4]),
            )
        } else {
            mr(
                0xD2,
                0x00,
                &encode_read_data(DataType::Atomic(AtomicType::Real), &[0; 4]),
            )
        }
    });
    let mut r = connect(port);
    let e = r.read_tag(&p("X"), 2).unwrap_err();
    assert!(
        matches!(e, Error::Protocol("data type changed between fragments")),
        "{e}"
    );
}

#[test]
fn fragments_that_never_progress_end() {
    let mut n = 0;
    let port = serve(move |_| {
        n += 1;
        let svc = if n == 1 { 0xCC } else { 0xD2 };
        mr(
            svc,
            0x06,
            &encode_read_data(DataType::Atomic(AtomicType::Dint), &[]),
        )
    });
    let mut r = connect(port);
    let e = r.read_tag(&p("X"), 10).unwrap_err();
    assert!(
        matches!(e, Error::Protocol("partial transfer made no progress")),
        "{e}"
    );
}

#[test]
fn fragments_that_never_finish_end_at_the_size_limit() {
    // A structure has no length the reader can check, so the cap is the limit.
    let port = serve(|req| {
        let svc = req[0] | 0x80;
        mr(
            svc,
            0x06,
            &encode_read_data(DataType::Struct { handle: 1 }, &[0; 400]),
        )
    });
    let mut r = connect(port);
    let e = r.read_tag(&p("X"), 1).unwrap_err();
    assert!(matches!(e, Error::Protocol(_)), "{e}");
    // An atomic array stops as soon as it holds more than was asked.
    let port = serve(|req| {
        let svc = req[0] | 0x80;
        mr(
            svc,
            0x06,
            &encode_read_data(DataType::Atomic(AtomicType::Dint), &[0; 400]),
        )
    });
    let mut r = connect(port);
    let e = r.read_tag(&p("X"), 150).unwrap_err();
    assert!(
        matches!(
            e,
            Error::Protocol("reply holds more data than the elements requested")
        ),
        "{e}"
    );
}

#[test]
fn fragmented_offsets_follow_what_arrived() {
    // The reader must ask for exactly the next byte, every time.
    let data: Vec<u8> = (0..400u32).flat_map(u32::to_le_bytes).collect();
    let port = serve(move |req| {
        let mut r = Reader::new(req);
        let svc = r.u8().unwrap();
        let words = r.u8().unwrap();
        r.bytes(usize::from(words) * 2).unwrap();
        let _n = r.u16_le().unwrap();
        let off = if svc == 0x52 {
            r.u32_le().unwrap() as usize
        } else {
            0
        };
        let end = (off + 300).min(data.len());
        let status = if end < data.len() { 0x06 } else { 0 };
        mr(
            svc | 0x80,
            status,
            &encode_read_data(DataType::Atomic(AtomicType::Udint), &data[off..end]),
        )
    });
    let mut r = connect(port);
    let d = r.read_tag(&p("X"), 400).unwrap();
    assert_eq!(
        d.values().unwrap(),
        (0..400).map(Value::Udint).collect::<Vec<_>>()
    );
}

fn symbols(list: &[(u32, &str)]) -> Vec<u8> {
    let mut w = Writer::default();
    for (i, n) in list {
        Symbol {
            instance: *i,
            name: (*n).into(),
            symbol_type: 0xC4,
            dims: [0; 3],
        }
        .encode(&mut w);
    }
    w.into_inner()
}

#[test]
fn browse_that_does_not_move_forward_ends() {
    let port = serve(|_| mr(0xD5, 0x06, &symbols(&[(5, "A")])));
    let e = connect(port).list_tags(None).unwrap_err();
    assert!(
        matches!(e, Error::Protocol("symbol instances do not move forward")),
        "{e}"
    );
    let port = serve(|_| mr(0xD5, 0x06, &[]));
    let e = connect(port).list_tags(None).unwrap_err();
    assert!(
        matches!(e, Error::Protocol("partial symbol reply with no symbols")),
        "{e}"
    );
    let port = serve(|_| mr(0xD5, 0x06, &symbols(&[(u32::MAX, "A")])));
    let e = connect(port).list_tags(None).unwrap_err();
    assert!(matches!(e, Error::Protocol("symbol instance overflow")), "{e}");
    let port = serve(|_| mr(0xD5, 0x00, &symbols(&[(9, "A"), (3, "B")])));
    let e = connect(port).list_tags(None).unwrap_err();
    assert!(matches!(e, Error::Protocol(_)), "out of order: {e}");
}

#[test]
fn browse_continues_from_the_last_instance_seen() {
    let mut starts = Vec::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let port = serve(move |req| {
        // Path: [0x20 0x6B] then an 8/16/32-bit instance segment.
        let start = match req[4] {
            0x24 => u32::from(req[5]),
            0x25 => u32::from(u16::from_le_bytes([req[6], req[7]])),
            _ => u32::from_le_bytes([req[6], req[7], req[8], req[9]]),
        };
        starts.push(start);
        tx.send(starts.clone()).unwrap();
        match start {
            0 => mr(0xD5, 0x06, &symbols(&[(0x10, "A"), (0x300, "B")])),
            0x301 => mr(0xD5, 0x06, &symbols(&[(0x2_0000, "C")])),
            _ => mr(0xD5, 0x00, &symbols(&[(0x2_0005, "D")])),
        }
    });
    let syms = connect(port).list_tags(None).unwrap();
    let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["A", "B", "C", "D"]);
    assert_eq!(rx.try_iter().last().unwrap(), [0, 0x301, 0x2_0001]);
}

#[test]
fn multiple_service_packet_lies_fail_the_batch() {
    let items = [(p("A"), 1), (p("B"), 1)];
    // Count lie: one reply for two requests.
    let port = serve(|_| mr(0x8A, 0, &join_multiple(&[mr(0xCC, 0, &dint(1))])));
    assert!(connect(port).read_tags(&items).is_err());
    // Garbage body.
    let port = serve(|_| mr(0x8A, 0, &[2, 0, 0xFF, 0xFF, 0, 0]));
    assert!(connect(port).read_tags(&items).is_err());
    // The packet itself refused.
    let port = serve(|_| mr(0x8A, 0x08, &[]));
    assert!(connect(port).read_tags(&items).is_err());
}

#[test]
fn multiple_service_packet_bad_part_fails_only_that_tag() {
    let port = serve(|_| {
        mr(
            0x8A,
            0x1E,
            &join_multiple(&[mr(0xCD, 0, &dint(1)), mr(0xCC, 0, &dint(2)), mr(0xCC, 0x05, &[])]),
        )
    });
    let out = connect(port)
        .read_tags(&[(p("A"), 1), (p("B"), 1), (p("C"), 1)])
        .unwrap();
    assert!(
        matches!(out[0], Err(Error::Decode(_))),
        "wrong service: {:?}",
        out[0]
    );
    assert_eq!(out[1].as_ref().unwrap().value().unwrap(), Value::Dint(2));
    assert!(
        matches!(&out[2], Err(Error::Cip { status, .. }) if *status == GeneralStatus::PATH_DESTINATION_UNKNOWN),
        "{:?}",
        out[2]
    );
}

#[test]
fn routing_failures_surface_as_cip_errors() {
    let port = serve(|_| {
        let mut w = Writer::default();
        cip::Response {
            service: 0xD2,
            status: GeneralStatus::CONNECTION_FAILURE,
            additional_status: vec![0x0312],
            data: vec![0],
        }
        .encode(&mut w);
        w.into_inner()
    });
    let mut r = connect(port).with_slot(4);
    let e = r.read_tag(&p("X"), 1).unwrap_err();
    assert!(
        matches!(&e, Error::Cip { status, additional } if *status == GeneralStatus::CONNECTION_FAILURE && additional == &[0x0312]),
        "{e}"
    );
}

#[test]
fn reader_sends_only_reads_routed_or_not() {
    let (tx, rx) = std::sync::mpsc::channel();
    let port = serve(move |req| {
        tx.send(req.to_vec()).unwrap();
        mr(req[0] | 0x80, 0x05, &[])
    });
    let mut r = connect(port).with_slot(0);
    let _ = r.read_tag(&p("A"), 1);
    let _ = r.read_tags(&[(p("A"), 1), (p("B"), 1)]);
    let _ = r.list_tags(None);
    let _ = r.get_attribute_single(1, 1, 1);
    r.close();
    let sent: Vec<Vec<u8>> = rx.try_iter().collect();
    assert_eq!(sent.len(), 4);
    for req in sent {
        // Unconnected_Send to the Connection Manager, carrying a read.
        assert_eq!(&req[..6], [0x52, 2, 0x20, 0x06, 0x24, 0x01]);
        assert!(wire::READ_SERVICES.contains(&req[10]), "{req:02X?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever comes back, read_tag returns (Ok or Err) and does not panic.
    #[test]
    fn arbitrary_replies_never_panic(reply in prop::collection::vec(any::<u8>(), 0..48), status in prop::sample::select(vec![0u8, 6, 0x1E, 0xFF])) {
        let port = serve(move |req| {
            let mut out = vec![req[0] | 0x80, 0, status, 0];
            out.extend_from_slice(&reply);
            out
        });
        let mut r = connect(port);
        let _ = r.read_tag(&p("X"), 3);
        let _ = r.read_tags(&[(p("A"), 1), (p("B"), 2)]);
        let _ = r.list_tags(None);
    }
}
