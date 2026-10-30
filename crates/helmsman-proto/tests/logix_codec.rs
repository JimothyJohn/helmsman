//! Logix tag-read codecs: paths, types, request bytes, reply decoders.
//! Decoders parse bytes from a device the box does not control, so every
//! one is fuzzed for "error, never panic".

use helmsman_proto::cip::{self, GeneralStatus};
use helmsman_proto::codec::{DecodeError, Reader, Writer};
use helmsman_proto::io::service::UNCONNECTED_SEND;
use helmsman_proto::logix::path::{Segment, decode_segments};
use helmsman_proto::logix::wire::{
    self, READ_SERVICES, check_service, decode_multiple, decode_read_data, decode_symbols, join_multiple,
    split_multiple,
};
use helmsman_proto::logix::{AtomicType, DataType, PathError, ReadRequest, Symbol, TagData, TagPath, Value};
use proptest::prelude::*;

// ---- paths ----

#[test]
fn valid_paths_round_trip_through_display() {
    for s in [
        "Temp",
        "_x",
        "Program:Main.Temp",
        "Local:1:I.Data",
        "Arr[1,2,3]",
        "Recipe[3].Steps[0].Time",
        "Big[70000]",
        "Status.5",
        "Status.63",
        "A[0]",
    ] {
        let p = TagPath::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(p.to_string(), s);
    }
}

#[test]
fn bad_paths_are_rejected_not_mangled() {
    type Check = fn(&PathError) -> bool;
    let cases: &[(&str, Check)] = &[
        ("", |e| *e == PathError::Empty),
        (".", |e| matches!(e, PathError::BadName(_))),
        ("A..B", |e| matches!(e, PathError::BadName(_))),
        ("1abc", |e| matches!(e, PathError::BadName(_))),
        ("A b", |e| matches!(e, PathError::BadName(_))),
        ("T\u{e9}mp", |e| matches!(e, PathError::BadName(_))),
        ("A\0", |e| matches!(e, PathError::BadName(_))),
        ("A\nB", |e| matches!(e, PathError::BadName(_))),
        ("A-B", |e| matches!(e, PathError::BadName(_))),
        ("A[", |e| matches!(e, PathError::BadIndex(_))),
        ("A[]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[1,2,3,4]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[-1]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[+1]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[ 1]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[4294967296]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[1][2]", |e| matches!(e, PathError::BadIndex(_))),
        ("A[1]x", |e| matches!(e, PathError::BadIndex(_))),
        ("A.64", |e| matches!(e, PathError::BadBit(_))),
        ("A.256", |e| matches!(e, PathError::BadBit(_))),
        ("A.5.B", |e| matches!(e, PathError::BadBit(_))),
        ("A.5.6", |e| matches!(e, PathError::BadBit(_))),
    ];
    for (s, ok) in cases {
        let r = TagPath::parse(s);
        assert!(r.as_ref().is_err_and(ok), "{s:?} gave {r:?}");
    }
    let long = "A".repeat(256);
    assert!(matches!(TagPath::parse(&long), Err(PathError::BadName(_))));
    // 255 is the segment length limit; a few of them overflow the path size.
    let max = "A".repeat(255);
    assert!(TagPath::parse(&max).is_ok());
    let too_many = [max.as_str(); 3].join(".");
    assert_eq!(TagPath::parse(&too_many), Err(PathError::TooLong));
}

#[test]
fn symbolic_segments_pad_odd_names() {
    assert_eq!(
        TagPath::parse("Temp").unwrap().encode(),
        [0x91, 4, b'T', b'e', b'm', b'p']
    );
    assert_eq!(
        TagPath::parse("Abc").unwrap().encode(),
        [0x91, 3, b'A', b'b', b'c', 0]
    );
    let p = TagPath::parse("Program:Main.X").unwrap();
    let mut want = vec![0x91, 12];
    want.extend_from_slice(b"Program:Main");
    want.extend_from_slice(&[0x91, 1, b'X', 0]);
    assert_eq!(p.encode(), want);
    assert_eq!(usize::from(p.size_words()) * 2, want.len());
}

#[test]
fn indexes_use_the_narrowest_member_segment() {
    let p = TagPath::parse("A[5,300,70000]").unwrap();
    assert_eq!(
        p.encode(),
        [
            0x91, 1, b'A', 0, // name
            0x28, 5, // 8-bit
            0x29, 0, 0x2C, 0x01, // 16-bit, padded
            0x2A, 0, 0x70, 0x11, 0x01, 0x00, // 32-bit, padded
        ]
    );
}

#[test]
fn bit_suffix_is_not_on_the_wire() {
    let p = TagPath::parse("Status.5").unwrap();
    assert_eq!(p.bit(), Some(5));
    assert_eq!(p.encode(), TagPath::parse("Status").unwrap().encode());
}

fn name() -> impl Strategy<Value = String> {
    "[A-Za-z_][A-Za-z0-9_:]{0,20}"
}

fn tag_path() -> impl Strategy<Value = TagPath> {
    (
        prop::collection::vec((name(), prop::collection::vec(any::<u32>(), 0..=3)), 1..5),
        prop::option::of(0u8..=63),
    )
        .prop_map(|(els, bit)| {
            let elements = els
                .into_iter()
                .map(|(name, index)| helmsman_proto::logix::Element { name, index })
                .collect();
            TagPath::from_elements(elements, bit).unwrap()
        })
}

proptest! {
    #[test]
    fn display_parse_round_trip(p in tag_path()) {
        prop_assert_eq!(TagPath::parse(&p.to_string()).unwrap(), p);
    }

    #[test]
    fn encode_decode_round_trip(p in tag_path()) {
        let bytes = p.encode();
        prop_assert_eq!(bytes.len() % 2, 0);
        let segs = decode_segments(&mut Reader::new(&bytes), p.size_words()).unwrap();
        prop_assert_eq!(TagPath::from_segments(&segs).unwrap(), p.without_bit());
    }

    #[test]
    fn parse_never_panics(s in "\\PC{0,80}") {
        let _ = TagPath::parse(&s);
    }

    #[test]
    fn parse_never_panics_on_path_alphabet(s in "[A-Za-z0-9_:.\\[\\],-]{0,60}") {
        if let Ok(p) = TagPath::parse(&s) {
            // Whatever parses re-renders to something that parses the same.
            prop_assert_eq!(TagPath::parse(&p.to_string()).unwrap(), p);
        }
    }

    #[test]
    fn decode_segments_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..64), words in any::<u8>()) {
        let _ = decode_segments(&mut Reader::new(&bytes), words);
    }
}

#[test]
fn decode_segments_rejects_hostile_paths() {
    let bad: &[&[u8]] = &[
        &[0x91, 0],             // empty symbol
        &[0x91, 1, b'A', 1],    // non-zero pad
        &[0x91, 2, 0x00, b'A'], // NUL in a name
        &[0x91, 2, 0xC3, 0xA9], // non-ASCII
        &[0x29, 1, 0x00, 0x01], // non-zero pad in a 16-bit member
        &[0x2B, 0],             // reserved logical format
        &[0x40, 0],             // not a segment type this target reads
    ];
    for b in bad {
        let words = u8::try_from(b.len() / 2).unwrap();
        assert!(
            matches!(
                decode_segments(&mut Reader::new(b), words),
                Err(DecodeError::Invalid(_))
            ),
            "{b:02X?}"
        );
    }
    // A length that runs past the path is truncation.
    assert!(matches!(
        decode_segments(&mut Reader::new(&[0x91, 9, b'A', 0]), 2),
        Err(DecodeError::Truncated { .. })
    ));
    // Members before any symbol are not a tag path.
    let segs = decode_segments(&mut Reader::new(&[0x28, 1]), 1).unwrap();
    assert_eq!(segs, [Segment::Member(1)]);
    assert_eq!(TagPath::from_segments(&segs), None);
}

// ---- types ----

#[test]
fn every_atomic_code_round_trips() {
    for t in AtomicType::ALL {
        assert_eq!(AtomicType::from_code(t.code()), Some(t));
        assert_eq!(AtomicType::from_name(&t.name().to_lowercase()), Some(t));
        let mut w = Writer::default();
        DataType::Atomic(t).encode(&mut w);
        let bytes = w.into_inner();
        assert_eq!(bytes, [t.code(), 0]);
        assert_eq!(
            DataType::decode(&mut Reader::new(&bytes)),
            Ok(DataType::Atomic(t))
        );
    }
}

#[test]
fn unknown_and_wide_type_codes_are_rejected() {
    for code in [
        0x0000u16, 0x00C0, 0x00CC, 0x00D0, 0x00D5, 0x01C4, 0xC400, 0x02A1, 0xFFFF,
    ] {
        assert_eq!(
            DataType::decode(&mut Reader::new(&code.to_le_bytes())),
            Err(DecodeError::Invalid("unknown data type code")),
            "{code:04X}"
        );
    }
    assert!(matches!(
        DataType::decode(&mut Reader::new(&[0xA0, 0x02, 0x01])),
        Err(DecodeError::Truncated { .. })
    ));
    assert_eq!(
        DataType::decode(&mut Reader::new(&[0xA0, 0x02, 0xCE, 0x0F])),
        Ok(DataType::Struct { handle: 0x0FCE })
    );
}

#[test]
fn values_decode_little_endian() {
    let d = TagData {
        ty: DataType::Atomic(AtomicType::Dint),
        bytes: [1i32, -2, i32::MAX]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
    };
    assert_eq!(
        d.values().unwrap(),
        [Value::Dint(1), Value::Dint(-2), Value::Dint(i32::MAX)]
    );
    let r = TagData {
        ty: DataType::Atomic(AtomicType::Real),
        bytes: 21.5f32.to_le_bytes().to_vec(),
    };
    assert_eq!(r.value().unwrap(), Value::Real(21.5));
    let b = TagData {
        ty: DataType::Atomic(AtomicType::Bool),
        bytes: vec![0xFF],
    };
    assert_eq!(b.value().unwrap(), Value::Bool(true));
    let u = TagData {
        ty: DataType::Atomic(AtomicType::Ulint),
        bytes: u64::MAX.to_le_bytes().to_vec(),
    };
    assert_eq!(u.value().unwrap(), Value::Ulint(u64::MAX));
    assert_eq!(u.value().unwrap().as_integer(), Some(i128::from(u64::MAX)));
}

#[test]
fn ragged_and_struct_data_do_not_decode_as_values() {
    let ragged = TagData {
        ty: DataType::Atomic(AtomicType::Dint),
        bytes: vec![0; 6],
    };
    assert!(ragged.values().is_err());
    let empty = TagData {
        ty: DataType::Atomic(AtomicType::Lreal),
        bytes: vec![],
    };
    assert!(matches!(empty.value(), Err(DecodeError::Truncated { .. })));
    let s = TagData {
        ty: DataType::Struct { handle: 1 },
        bytes: vec![0; 8],
    };
    assert!(s.values().is_err());
    assert!(s.value().is_err());
    assert!(AtomicType::Dint.decode(&[0; 5]).is_err());
}

#[test]
fn bits_follow_twos_complement_at_the_type_width() {
    assert_eq!(Value::Sint(-1).bit(7), Some(true));
    assert_eq!(Value::Sint(-1).bit(8), None);
    assert_eq!(Value::Dint(1 << 5).bit(5), Some(true));
    assert_eq!(Value::Dint(1 << 5).bit(4), Some(false));
    assert_eq!(Value::Lint(i64::MIN).bit(63), Some(true));
    assert_eq!(Value::Real(1.0).bit(0), None);
    assert_eq!(Value::Bool(true).bit(0), None);
}

proptest! {
    #[test]
    fn decode_read_data_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        if let Ok(d) = decode_read_data(&bytes) {
            let _ = d.values();
            let _ = d.value();
        }
    }

    #[test]
    fn atomic_values_round_trip(t in prop::sample::select(AtomicType::ALL.to_vec()), raw in prop::collection::vec(any::<u8>(), 8)) {
        let bytes = &raw[..t.size()];
        let data = wire::encode_read_data(DataType::Atomic(t), bytes);
        let back = decode_read_data(&data).unwrap();
        prop_assert_eq!(back.ty, DataType::Atomic(t));
        prop_assert_eq!(&back.bytes[..], bytes);
        prop_assert!(back.value().is_ok());
    }
}

// ---- requests ----

#[test]
fn read_tag_request_bytes() {
    let r = ReadRequest::read_tag(&TagPath::parse("Temp").unwrap(), 1);
    assert_eq!(r.as_bytes(), [0x4C, 3, 0x91, 4, b'T', b'e', b'm', b'p', 1, 0]);
    let r = ReadRequest::read_tag(&TagPath::parse("A[2]").unwrap(), 10);
    assert_eq!(r.as_bytes(), [0x4C, 3, 0x91, 1, b'A', 0, 0x28, 2, 10, 0]);
    let r = ReadRequest::read_tag_fragmented(&TagPath::parse("A").unwrap(), 400, 0x0102_0304);
    assert_eq!(r.as_bytes(), [0x52, 2, 0x91, 1, b'A', 0, 0x90, 0x01, 4, 3, 2, 1]);
}

#[test]
fn symbol_list_request_bytes() {
    let r = ReadRequest::symbol_list(None, 0).unwrap();
    assert_eq!(
        r.as_bytes(),
        [0x55, 2, 0x20, 0x6B, 0x24, 0, 3, 0, 1, 0, 2, 0, 8, 0]
    );
    let r = ReadRequest::symbol_list(None, 0x1234).unwrap();
    assert_eq!(&r.as_bytes()[..8], [0x55, 3, 0x20, 0x6B, 0x25, 0, 0x34, 0x12]);
    let r = ReadRequest::symbol_list(None, 0x0001_0000).unwrap();
    assert_eq!(&r.as_bytes()[..10], [0x55, 4, 0x20, 0x6B, 0x26, 0, 0, 0, 1, 0]);
    let r = ReadRequest::symbol_list(Some("Program:Main"), 7).unwrap();
    let mut want = vec![0x55, 9, 0x91, 12];
    want.extend_from_slice(b"Program:Main");
    want.extend_from_slice(&[0x20, 0x6B, 0x24, 7]);
    assert_eq!(&r.as_bytes()[..want.len()], &want[..]);
    for bad in ["", "Program:Main.X", "P[1]", "P.3", "P q"] {
        assert!(ReadRequest::symbol_list(Some(bad), 0).is_err(), "{bad:?}");
    }
}

#[test]
fn multiple_service_packet_layout() {
    let a = ReadRequest::read_tag(&TagPath::parse("A").unwrap(), 1);
    let b = ReadRequest::read_tag(&TagPath::parse("Bb").unwrap(), 2);
    let m = ReadRequest::multiple(&[a.clone(), b.clone()]).unwrap();
    let bytes = m.as_bytes();
    assert_eq!(&bytes[..6], [0x0A, 2, 0x20, 0x02, 0x24, 0x01]);
    let parts = split_multiple(&bytes[6..]).unwrap();
    assert_eq!(parts, [a.as_bytes(), b.as_bytes()]);
    assert_eq!(&bytes[6..12], [2, 0, 6, 0, 6 + 8, 0]);
    assert_eq!(m.multiple_count(), Some(2));
    assert!(ReadRequest::multiple(&[]).is_err());
    assert!(
        ReadRequest::multiple(std::slice::from_ref(&m)).is_err(),
        "nested packet"
    );
    assert!(
        ReadRequest::multiple(&[a.clone().routed(0).unwrap()]).is_err(),
        "routed inside"
    );
}

#[test]
fn unconnected_send_wraps_with_pad_and_route() {
    let inner = ReadRequest::read_tag(&TagPath::parse("Abc").unwrap(), 1);
    assert_eq!(inner.as_bytes().len(), 10);
    let r = inner.clone().routed(2).unwrap();
    assert_eq!(r.service(), UNCONNECTED_SEND);
    assert_eq!(r.answers(), 0x4C);
    let b = r.as_bytes();
    assert_eq!(&b[..6], [0x52, 2, 0x20, 0x06, 0x24, 0x01]);
    assert_eq!(&b[6..10], [0x0A, 0x05, 10, 0]);
    assert_eq!(&b[10..20], inner.as_bytes());
    assert_eq!(&b[20..], [1, 0, 1, 2]);
    let tag = ReadRequest::read_tag(&TagPath::parse("A").unwrap(), 1);
    let r = ReadRequest::multiple(&[tag]).unwrap().routed(0).unwrap();
    assert!(r.clone().routed(0).is_err(), "routed twice");
    assert_eq!(r.answers(), cip::service::MULTIPLE_SERVICE_PACKET);
}

/// Every way to build a request ends in a read service, including what is
/// inside packets and routes. There is no constructor that takes a code.
fn any_read_request() -> impl Strategy<Value = ReadRequest> {
    let leaf = prop_oneof![
        (tag_path(), any::<u16>()).prop_map(|(p, n)| ReadRequest::read_tag(&p, n)),
        (tag_path(), any::<u16>(), any::<u32>())
            .prop_map(|(p, n, o)| ReadRequest::read_tag_fragmented(&p, n, o)),
        any::<u32>().prop_map(|s| ReadRequest::symbol_list(None, s).unwrap()),
        (any::<u16>(), any::<u16>(), any::<u16>())
            .prop_map(|(c, i, a)| ReadRequest::get_attribute_single(c, i, a)),
    ];
    (
        prop::collection::vec(leaf, 1..4),
        any::<bool>(),
        prop::option::of(any::<u8>()),
    )
        .prop_map(|(reqs, msp, slot)| {
            let r = if msp && reqs.len() > 1 {
                ReadRequest::multiple(&reqs).unwrap()
            } else {
                reqs.into_iter().next().unwrap()
            };
            match slot {
                Some(s) => r.routed(s).unwrap(),
                None => r,
            }
        })
}

fn services_in(mr: &[u8], out: &mut Vec<u8>) {
    let req_service = mr[0];
    out.push(req_service);
    let words = usize::from(mr[1]);
    let data = &mr[2 + words * 2..];
    if req_service == cip::service::MULTIPLE_SERVICE_PACKET {
        for p in split_multiple(data).unwrap() {
            services_in(p, out);
        }
    } else if req_service == UNCONNECTED_SEND && mr[2..4] == [0x20, 0x06] {
        let n = usize::from(u16::from_le_bytes([data[2], data[3]]));
        services_in(&data[4..4 + n], out);
    }
}

proptest! {
    #[test]
    fn only_read_services_can_be_built(r in any_read_request()) {
        let mut seen = Vec::new();
        services_in(r.as_bytes(), &mut seen);
        for s in seen {
            prop_assert!(
                READ_SERVICES.contains(&s) || s == UNCONNECTED_SEND,
                "service 0x{:02X} in {:02X?}", s, r.as_bytes()
            );
            // Write Tag, Write Tag Fragmented, Read-Modify-Write, Set_Attribute_*.
            prop_assert!(![0x4D, 0x53, 0x4E, 0x10, 0x02, 0x04].contains(&s));
        }
    }
}

// ---- replies ----

#[test]
fn check_service_refuses_a_reply_for_another_request() {
    let r = cip::Response::ok(0x4D, vec![]);
    assert!(check_service(&r, 0x4C).is_err());
    assert!(check_service(&cip::Response::ok(0x4C, vec![]), 0x4C).is_ok());
    // A request echoed back without the reply bit is not a reply.
    let echoed = cip::Response {
        service: 0x4C,
        status: GeneralStatus::SUCCESS,
        additional_status: vec![],
        data: vec![],
    };
    assert!(check_service(&echoed, 0x4C).is_err());
}

fn msp_reply(status: GeneralStatus, parts: &[cip::Response]) -> cip::Response {
    let encoded: Vec<Vec<u8>> = parts
        .iter()
        .map(|p| {
            let mut w = Writer::default();
            p.encode(&mut w);
            w.into_inner()
        })
        .collect();
    cip::Response {
        service: 0x8A,
        status,
        additional_status: vec![],
        data: join_multiple(&encoded),
    }
}

#[test]
fn multiple_reply_with_partial_failure() {
    let ok = cip::Response::ok(
        0x4C,
        wire::encode_read_data(DataType::Atomic(AtomicType::Int), &[7, 0]),
    );
    let bad = cip::Response::error(0x4C, GeneralStatus::PATH_DESTINATION_UNKNOWN);
    let r = msp_reply(GeneralStatus::EMBEDDED_SERVICE_ERROR, &[ok.clone(), bad.clone()]);
    assert_eq!(decode_multiple(&r, 2).unwrap(), [ok.clone(), bad]);
    assert!(decode_multiple(&r, 3).is_err(), "count lie");
    let mut wrong = r.clone();
    wrong.service = 0x8E;
    assert!(decode_multiple(&wrong, 2).is_err(), "wrong service");
    let failed = cip::Response::error(0x0A, GeneralStatus::SERVICE_NOT_SUPPORTED);
    assert!(decode_multiple(&failed, 2).is_err(), "packet-level failure");
}

#[test]
fn split_multiple_rejects_lying_offsets() {
    let bad: &[&[u8]] = &[
        &[0, 0],                         // zero services
        &[2, 0, 6, 0],                   // table cut short
        &[1, 0, 2, 0, 0xAA],             // offset inside the table
        &[1, 0, 9, 0, 0xAA],             // offset past the end
        &[2, 0, 7, 0, 6, 0, 0xAA, 0xBB], // decreasing
        &[1, 0, 0xFF, 0xFF],             // huge
    ];
    for b in bad {
        assert!(split_multiple(b).is_err(), "{b:02X?}");
    }
}

proptest! {
    #[test]
    fn split_multiple_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..96)) {
        if let Ok(parts) = split_multiple(&bytes) {
            for p in parts {
                prop_assert!(p.len() <= bytes.len());
            }
        }
    }

    #[test]
    fn decode_multiple_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..96), n in 0usize..8) {
        let r = cip::Response { service: 0x8A, status: GeneralStatus::SUCCESS, additional_status: vec![], data: bytes };
        let _ = decode_multiple(&r, n);
    }

    #[test]
    fn join_split_round_trip(parts in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..20), 1..8)) {
        let body = join_multiple(&parts);
        let back: Vec<Vec<u8>> = split_multiple(&body).unwrap().into_iter().map(<[u8]>::to_vec).collect();
        prop_assert_eq!(back, parts);
    }
}

// ---- symbols ----

fn sym(instance: u32, name: &str, ty: DataType, dims: &[u32]) -> Symbol {
    let mut d = [0u32; 3];
    d[..dims.len()].copy_from_slice(dims);
    Symbol {
        instance,
        name: name.into(),
        symbol_type: Symbol::type_word(ty, dims.len()),
        dims: d,
    }
}

fn encode_symbols(s: &[Symbol]) -> Vec<u8> {
    let mut w = Writer::default();
    for x in s {
        x.encode(&mut w);
    }
    w.into_inner()
}

#[test]
fn symbol_type_word_bits() {
    let a = sym(1, "Temps", DataType::Atomic(AtomicType::Real), &[10, 2]);
    assert_eq!(a.symbol_type, 0x40CA);
    assert_eq!(a.dim_count(), 2);
    assert_eq!(a.shape(), [10, 2]);
    assert_eq!(a.atomic(), Some(AtomicType::Real));
    assert!(!a.is_struct());
    let s = sym(2, "Timer1", DataType::Struct { handle: 0x0F83 }, &[]);
    assert_eq!(s.symbol_type, 0x8F83);
    assert_eq!(s.template(), Some(0x0F83));
    assert_eq!(s.atomic(), None);
    let p = sym(3, "Program:Main", DataType::Atomic(AtomicType::Dint), &[]);
    assert!(p.is_program());
    let sys = Symbol {
        symbol_type: 0x1000 | 0xC4,
        ..p.clone()
    };
    assert!(sys.is_system());
    assert!(sym(4, "__hidden", DataType::Atomic(AtomicType::Dint), &[]).is_system());
}

#[test]
fn symbols_round_trip_and_truncation_is_an_error_at_every_cut() {
    let syms = vec![
        sym(0x10, "Temp", DataType::Atomic(AtomicType::Real), &[]),
        sym(0x2_0000, "Big", DataType::Atomic(AtomicType::Dint), &[1000]),
    ];
    let bytes = encode_symbols(&syms);
    assert_eq!(decode_symbols(&bytes).unwrap(), syms);
    for cut in 1..bytes.len() {
        let r = decode_symbols(&bytes[..cut]);
        // A cut exactly between two entries is a valid shorter list.
        if let Ok(list) = r {
            assert_eq!(list.len(), 1, "cut {cut}");
        }
    }
    assert!(decode_symbols(&bytes[..bytes.len() - 1]).is_err());
}

#[test]
fn hostile_symbol_names_are_rejected() {
    for name in [&b""[..], b"A\x00B", b"A B", b"\xC3\xA9", b"A\nB", b"\x1B[31m"] {
        let mut w = Writer::default();
        w.u32_le(1)
            .u16_le(u16::try_from(name.len()).unwrap())
            .bytes(name)
            .u16_le(0xC4)
            .u32_le(0)
            .u32_le(0)
            .u32_le(0);
        assert!(decode_symbols(&w.into_inner()).is_err(), "{name:02X?}");
    }
    // A length that claims more than is there.
    let mut w = Writer::default();
    w.u32_le(1).u16_le(0xFFFF).bytes(b"Temp");
    assert!(matches!(
        decode_symbols(&w.into_inner()),
        Err(DecodeError::Truncated { .. })
    ));
}

proptest! {
    #[test]
    fn decode_symbols_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..128)) {
        let _ = decode_symbols(&bytes);
    }
}
