//! Golden vectors from Rockwell's own manual: "Logix 5000 Controllers Data
//! Access", publication 1756-PM020I-EN-P (September 2025). Every byte string
//! here is copied from a worked example in it, cited by page, so these tests
//! hold the encoder and decoders to the controller vendor's documentation
//! rather than to another client library.

use helmsman_proto::cip;
use helmsman_proto::logix::wire::{decode_multiple, decode_read_data};
use helmsman_proto::logix::{AtomicType, DataType, ReadRequest, Symbol, TagPath, Value};

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn path(s: &str) -> TagPath {
    TagPath::parse(s).unwrap()
}

/// p. 18: Read Tag, one element of `TotalCount` (ANSI extended symbolic
/// segment, 6-word path).
#[test]
fn read_tag_request_matches_pm020_p18() {
    let want = hex("4C 06 91 0A 54 6F 74 61 6C 43 6F 75 6E 74 01 00");
    assert_eq!(ReadRequest::read_tag(&path("TotalCount"), 1).as_bytes(), want);
}

/// p. 18: the reply, a DINT of 534.
#[test]
fn read_tag_reply_matches_pm020_p18() {
    let resp = cip::Response::decode(&hex("CC 00 00 00 C4 00 16 02 00 00")).unwrap();
    assert_eq!(resp.service, 0x4C | 0x80);
    let data = decode_read_data(&resp.data).unwrap();
    assert_eq!(data.ty, DataType::Atomic(AtomicType::Dint));
    assert_eq!(data.value().unwrap(), Value::Dint(534));
}

/// p. 20 and p. 21: Read Tag Fragmented, 1750 SINTs of `TotalCount`, from
/// byte offset 0 and then 490 (the offset is in bytes whatever the type).
#[test]
fn read_tag_fragmented_requests_match_pm020_p20_p21() {
    let at = |offset: &str| {
        hex(&format!(
            "52 06 91 0A 54 6F 74 61 6C 43 6F 75 6E 74 D6 06 {offset}"
        ))
    };
    assert_eq!(
        ReadRequest::read_tag_fragmented(&path("TotalCount"), 1750, 0).as_bytes(),
        at("00 00 00 00")
    );
    assert_eq!(
        ReadRequest::read_tag_fragmented(&path("TotalCount"), 1750, 490).as_bytes(),
        at("EA 01 00 00")
    );
}

/// p. 38: a Multiple Service Packet to the Message Router (class 2,
/// instance 1) carrying two Read Tags; offsets count from the start of the
/// request data (the service count).
#[test]
fn multiple_service_request_matches_pm020_p38() {
    let want = hex("0A 02 20 02 24 01 02 00 06 00 12 00 \
         4C 04 91 05 70 61 72 74 73 00 01 00 \
         4C 07 91 0B 43 6F 6E 74 72 6F 6C 57 6F 72 64 00 01 00");
    let msp = ReadRequest::multiple(&[
        ReadRequest::read_tag(&path("parts"), 1),
        ReadRequest::read_tag(&path("ControlWord"), 1),
    ])
    .unwrap();
    assert_eq!(msp.as_bytes(), want);
}

/// p. 39: the reply, two DINTs (42 and 476) behind offsets counted from the
/// start of the reply data.
#[test]
fn multiple_service_reply_matches_pm020_p39() {
    let resp = cip::Response::decode(&hex("8A 00 00 00 02 00 06 00 10 00 \
         CC 00 00 00 C4 00 2A 00 00 00 \
         CC 00 00 00 C4 00 DC 01 00 00"))
    .unwrap();
    let parts = decode_multiple(&resp, 2).unwrap();
    let values: Vec<Value> = parts
        .iter()
        .map(|p| decode_read_data(&p.data).unwrap().value().unwrap())
        .collect();
    assert_eq!(values, [Value::Dint(42), Value::Dint(476)]);
}

/// p. 48: `Get_Instance_Attribute_List` on the Symbol object continuing from
/// instance 0x5D35 addresses it as `20 6B 25 00 35 5D` (class 0x6B, 16-bit
/// instance segment). This crate also asks for attribute 8 (dimensions),
/// which PM020 does not document, so only the service and path are checked.
#[test]
fn symbol_browse_path_matches_pm020_p48() {
    let req = ReadRequest::symbol_list(None, 0x5D35).unwrap();
    assert_eq!(&req.as_bytes()[..8], hex("55 03 20 6B 25 00 35 5D"));
}

/// p. 47, p. 49 and p. 50: the Symbol Type word. Bit 15 is a structure (its
/// bits 0-11 the template instance), bit 12 a system tag, bits 13-14 the
/// number of dimensions, and an atomic tag's low byte its type code.
#[test]
fn symbol_type_words_from_pm020_p47_to_p50() {
    let sym = |symbol_type: u16, name: &str| Symbol {
        instance: 1,
        name: name.into(),
        symbol_type,
        dims: [0; 3],
    };
    let cip_read = sym(0x8FFF, "CipReadData10"); // p. 47 "FF 8F"
    assert!(cip_read.is_struct() && !cip_read.is_system());
    assert_eq!(cip_read.template(), Some(0x0FFF));
    let counts = sym(0xCF82, "counts"); // p. 47 "82 CF"
    assert!(counts.is_struct());
    assert_eq!(counts.dim_count(), 2);
    let parts_dest = sym(0x00C3, "Parts_Dest"); // p. 47 "C3 00": INT
    assert_eq!(parts_dest.atomic(), Some(AtomicType::Int));
    assert_eq!(parts_dest.dim_count(), 0);
    let program = sym(0x1068, "Program:MainProgram"); // p. 49 "68 10"
    assert!(program.is_system() && program.is_program());
    let machine = sym(0x82E9, "MachineSummary"); // p. 51: STRUCT_B, template 0x2E9
    assert_eq!(machine.template(), Some(0x02E9));
}
