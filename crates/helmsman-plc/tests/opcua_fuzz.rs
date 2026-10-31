//! The OPC UA server's decoder and connection state machine against
//! hostile bytes: random input, valid exchanges with bytes flipped, and
//! well-framed requests with random bodies. None may panic; a broken
//! message ends in `ERR` or a `ServiceFault`, never a crash. Time and the
//! wall clock are fixed, so every case replays.

use std::sync::Arc;
use std::time::{Duration, Instant};

use helmsman_io::{Dir, Ty, Value};
use helmsman_plc::opcua::codec::{NodeId, Reader, Writer};
use helmsman_plc::opcua::feed::{SigShape, Snapshot};
use helmsman_plc::opcua::{Conn, Ctx, Feed, Limits, Shape};
use proptest::prelude::*;

const NONE: &str = "http://opcfoundation.org/UA/SecurityPolicy#None";

fn ctx() -> Arc<Ctx> {
    let feed = Arc::new(Feed::default());
    let shape = Arc::new(Shape {
        signals: vec![SigShape {
            name: "level".into(),
            ty: Ty::Real,
            dir: Dir::In,
            unit: Some("mm".into()),
            source: None,
            device: true,
        }],
        modules: Vec::new(),
    });
    feed.offer(Arc::new(Snapshot {
        seq: 1,
        shape,
        values: vec![Some(Value::Real(1.5))],
        errors: vec![false],
        changed: vec![Some(1_800_000_000_000)],
        modules: Vec::new(),
    }));
    Arc::new(Ctx::new(
        feed,
        "opc.tcp://127.0.0.1:4840/".into(),
        Limits::default(),
        Arc::new(|| 1_800_000_000_000),
    ))
}

fn frame(head: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = head.to_vec();
    v.extend_from_slice(&u32::try_from(body.len() + 8).unwrap().to_le_bytes());
    v.extend_from_slice(body);
    v
}

fn hello() -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(0)
        .u32(65_536)
        .u32(65_536)
        .u32(0)
        .u32(0)
        .string(Some("opc.tcp://127.0.0.1:4840/"));
    frame(*b"HELF", &w.buf)
}

fn header(w: &mut Writer, auth: &NodeId, handle: u32) {
    w.node_id(auth)
        .i64(0)
        .u32(handle)
        .u32(0)
        .string(None)
        .u32(0)
        .extension_object(&NodeId::ns0(0), None);
}

fn open() -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(0).string(Some(NONE)).bytes(None).bytes(None).u32(1).u32(1);
    w.node_id(&NodeId::ns0(446));
    header(&mut w, &NodeId::ns0(0), 1);
    w.u32(0).u32(0).u32(1).bytes(Some(&[])).u32(60_000);
    frame(*b"OPNF", &w.buf)
}

/// A `MSG` on channel `channel` (token 1).
fn msg(channel: u32, seq: u32, request_id: u32, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(channel).u32(1).u32(seq).u32(request_id).raw(body);
    frame(*b"MSGF", &w.buf)
}

fn create_session() -> Vec<u8> {
    let mut w = Writer::new();
    w.node_id(&NodeId::ns0(461));
    header(&mut w, &NodeId::ns0(0), 2);
    w.string(Some("urn:t"))
        .string(None)
        .localized_text(Some("t"))
        .u32(1)
        .string(None)
        .string(None)
        .null_array()
        .string(None)
        .string(Some("opc.tcp://127.0.0.1:4840/"))
        .string(Some("s"))
        .bytes(Some(&[0; 32]))
        .bytes(None)
        .f64(60_000.0)
        .u32(0);
    w.buf
}

fn activate(auth: &NodeId) -> Vec<u8> {
    let mut w = Writer::new();
    w.node_id(&NodeId::ns0(467));
    header(&mut w, auth, 3);
    let mut token = Writer::new();
    token.string(Some("anonymous"));
    w.string(None)
        .bytes(None)
        .null_array()
        .null_array()
        .extension_object(&NodeId::ns0(321), Some(&token.buf))
        .string(None)
        .bytes(None);
    w.buf
}

fn read_level(auth: &NodeId) -> Vec<u8> {
    let mut w = Writer::new();
    w.node_id(&NodeId::ns0(631));
    header(&mut w, auth, 4);
    w.f64(0.0).u32(2).len(1);
    w.node_id(&NodeId::box_("signals/level"))
        .u32(13)
        .string(None)
        .u16(0)
        .string(None);
    w.buf
}

/// The `MSG` responses in `out`: (request id, response type id, body
/// after the type).
fn responses(out: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut v = Vec::new();
    let mut at = 0;
    while at + 8 <= out.len() {
        let size = u32::from_le_bytes(out[at + 4..at + 8].try_into().unwrap()) as usize;
        let m = &out[at..at + size];
        if &m[..3] == b"MSG" {
            let mut r = Reader::new(&m[8..]);
            r.take(12).unwrap();
            let id = r.u32().unwrap();
            let ty = r.node_id().unwrap().ns0_id().unwrap();
            v.push((id, ty, r.take(r.remaining()).unwrap().to_vec()));
        }
        at += size;
    }
    v
}

fn channel_id(out: &[u8]) -> u32 {
    let at = out.windows(4).position(|w| w == b"OPNF").expect("an OPN reply");
    u32::from_le_bytes(out[at + 8..at + 12].try_into().unwrap())
}

/// The bytes of a whole valid exchange: hello, open, create, activate,
/// read; and what the server answered.
fn exchange() -> (Vec<Vec<u8>>, Vec<u8>) {
    let ctx = ctx();
    let now = Instant::now();
    let mut c = Conn::new(ctx, now);
    let mut sent = vec![hello(), open()];
    c.feed(&sent[0], now);
    c.feed(&sent[1], now);
    let out = c.take_output();
    let ch = channel_id(&out);
    sent.push(msg(ch, 2, 2, &create_session()));
    c.feed(&sent[2], now);
    let out = c.take_output();
    let (_, ty, body) = &responses(&out)[0];
    assert_eq!(*ty, 464, "CreateSessionResponse");
    let mut r = Reader::new(body);
    r.i64().unwrap();
    r.u32().unwrap();
    assert_eq!(r.u32().unwrap(), 0, "CreateSession failed");
    r.u8().unwrap();
    r.count().unwrap();
    r.extension_object().unwrap();
    r.node_id().unwrap();
    let auth = r.node_id().unwrap();
    sent.push(msg(ch, 3, 3, &activate(&auth)));
    sent.push(msg(ch, 4, 4, &read_level(&auth)));
    c.feed(&sent[3], now);
    c.feed(&sent[4], now);
    (sent, c.take_output())
}

#[test]
fn a_valid_exchange_reads_a_value() {
    let (_, out) = exchange();
    let r = responses(&out);
    assert_eq!(r.iter().map(|x| x.1).collect::<Vec<_>>(), [470, 634], "{r:?}");
    let mut body = Reader::new(&r[1].2);
    body.i64().unwrap();
    body.u32().unwrap();
    assert_eq!(body.u32().unwrap(), 0);
    body.u8().unwrap();
    body.count().unwrap();
    body.extension_object().unwrap();
    assert_eq!(body.count().unwrap(), Some(1));
    // DataValue: value + timestamps, a Double 1.5.
    let mask = body.u8().unwrap();
    assert_eq!(mask & 0x03, 0x01, "value, no status: {mask:#x}");
    assert_eq!(body.u8().unwrap(), 11);
    assert!((body.f64().unwrap() - 1.5).abs() < f64::EPSILON);
}

#[test]
fn a_request_before_the_session_is_activated_is_refused() {
    let ctx = ctx();
    let now = Instant::now();
    let mut c = Conn::new(ctx, now);
    c.feed(&hello(), now);
    c.feed(&open(), now);
    let ch = channel_id(&c.take_output());
    // A read with a made-up token: no such session.
    c.feed(&msg(ch, 2, 2, &read_level(&NodeId::Bytes(0, vec![1; 32]))), now);
    let r = responses(&c.take_output());
    assert_eq!(r[0].1, 397, "ServiceFault: {r:?}");
    let mut b = Reader::new(&r[0].2);
    b.i64().unwrap();
    b.u32().unwrap();
    assert_eq!(b.u32().unwrap(), 0x8025_0000, "BadSessionIdInvalid");
    assert!(!c.is_closed());
}

#[test]
fn security_policies_other_than_none_are_refused() {
    let now = Instant::now();
    let mut c = Conn::new(ctx(), now);
    c.feed(&hello(), now);
    let mut w = Writer::new();
    w.u32(0)
        .string(Some("http://opcfoundation.org/UA/SecurityPolicy#Basic256Sha256"))
        .bytes(Some(&[1, 2, 3]))
        .bytes(None)
        .u32(1)
        .u32(1);
    c.feed(&frame(*b"OPNF", &w.buf), now);
    let out = c.take_output();
    assert!(out.windows(4).any(|w| w == b"ERRF"), "{out:?}");
    assert!(c.is_closed());
}

#[test]
fn an_idle_connection_is_dropped() {
    let now = Instant::now();
    let mut c = Conn::new(ctx(), now);
    c.tick(now + Duration::from_secs(6));
    assert!(c.is_closed());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn random_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let now = Instant::now();
        let mut c = Conn::new(ctx(), now);
        c.feed(&data, now);
        c.tick(now + Duration::from_secs(1));
        let out = c.take_output();
        prop_assert!(out.is_empty() || out.starts_with(b"ACKF") || out.starts_with(b"ERRF"));
    }

    #[test]
    fn random_bytes_on_an_open_channel_never_panic(
        data in proptest::collection::vec(any::<u8>(), 0..2048),
        split in 0usize..64,
    ) {
        let now = Instant::now();
        let mut c = Conn::new(ctx(), now);
        c.feed(&hello(), now);
        c.feed(&open(), now);
        let _ = c.take_output();
        let at = split.min(data.len());
        c.feed(&data[..at], now);
        c.feed(&data[at..], now);
        c.tick(now + Duration::from_millis(200));
    }

    #[test]
    fn random_request_bodies_get_an_answer_not_a_panic(
        body in proptest::collection::vec(any::<u8>(), 0..1024),
        ty in prop::sample::select(vec![428u32, 422, 461, 467, 473, 527, 533, 554, 560, 631, 673, 751, 763, 769, 781, 787, 793, 799, 826, 832, 847, 9999]),
    ) {
        let (sent, _) = exchange();
        let now = Instant::now();
        let mut c = Conn::new(ctx(), now);
        for m in &sent[..2] {
            c.feed(m, now);
        }
        let ch = channel_id(&c.take_output());
        let mut w = Writer::new();
        w.node_id(&NodeId::ns0(ty)).raw(&body);
        c.feed(&msg(ch, 2, 7, &w.buf), now);
        c.tick(now + Duration::from_secs(1));
        let out = c.take_output();
        prop_assert!(c.is_closed() || responses(&out).iter().any(|r| r.0 == 7));
    }

    #[test]
    fn a_flipped_byte_in_a_valid_exchange_never_panics(
        which in 0usize..5,
        at in any::<prop::sample::Index>(),
        bits in 1u8..=255,
    ) {
        let (mut sent, _) = exchange();
        let m = &mut sent[which];
        let i = at.index(m.len());
        m[i] ^= bits;
        let now = Instant::now();
        let mut c = Conn::new(ctx(), now);
        for m in &sent {
            c.feed(m, now);
        }
        c.tick(now + Duration::from_millis(500));
    }

    #[test]
    fn the_decoder_never_panics(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Reader::new(&data).skip_variant(0);
        let _ = Reader::new(&data).skip_data_value(0);
        let _ = Reader::new(&data).skip_diagnostic_info(0);
        let _ = Reader::new(&data).node_id();
        let _ = Reader::new(&data).expanded_node_id();
        let _ = Reader::new(&data).extension_object();
        let _ = Reader::new(&data).localized_text();
        let mut r = Reader::new(&data);
        let _ = r.vec(Reader::string);
    }
}
