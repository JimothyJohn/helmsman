use super::*;
use crate::api::SignalValue;
use crate::iomap::{self, IoMap};
use crate::ioplan::{put_slot, value_slot};
use proptest::prelude::*;

const MAP: &str = "\
share speed    int  hr:0
share run_cmd  bool coil:0
share at_home  bool di:0
share level    real ir:4:f32
share pos      int  hr:10:i32
share target   real hr:20:f32sw
share armed    bool coil:5
share count    int  ir:0
";

/// Index of each share in `MAP`.
const SPEED: usize = 0;
const RUN_CMD: usize = 1;
const AT_HOME: usize = 2;
const LEVEL: usize = 3;
const POS: usize = 4;
const TARGET: usize = 5;
const ARMED: usize = 6;

fn map() -> IoMap {
    iomap::parse(MAP).unwrap()
}

/// A bank where `run_cmd`, `pos` and `target` are VAR_INPUTs (writable)
/// and the rest are the program's own.
fn bank(m: &IoMap) -> ShareBank {
    let mut b = ShareBank {
        writable: (1 << RUN_CMD) | (1 << POS) | (1 << TARGET),
        ..ShareBank::default()
    };
    let set = |b: &mut ShareBank, i: usize, v: SignalValue| {
        let s = &m.shares[i];
        put_slot(&mut b.image, s.fmt, s.at, v);
    };
    set(&mut b, SPEED, SignalValue::Int(1500));
    set(&mut b, AT_HOME, SignalValue::Bool(true));
    set(&mut b, LEVEL, SignalValue::Real(2.5));
    set(&mut b, POS, SignalValue::Int(-70_000));
    set(&mut b, 7, SignalValue::Int(65_535));
    b
}

fn frame(tid: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
    mbap::frame(tid, unit, pdu).unwrap()
}

/// Sends a PDU and returns the reply PDU.
fn ask(b: &mut ShareBank, m: &IoMap, pdu: &[u8]) -> Vec<u8> {
    let reply = b.handle(&m.shares, &frame(7, 1, pdu)).expect("a frame");
    assert_eq!(&reply[..2], &7u16.to_be_bytes(), "transaction echoed");
    assert_eq!(&reply[2..4], &[0, 0], "protocol id");
    assert_eq!(
        usize::from(u16::from_be_bytes([reply[4], reply[5]])),
        reply.len() - 6
    );
    assert_eq!(reply[6], 1, "unit echoed");
    reply[7..].to_vec()
}

fn read(fc: u8, start: u16, qty: u16) -> Vec<u8> {
    let mut p = vec![fc];
    p.extend(start.to_be_bytes());
    p.extend(qty.to_be_bytes());
    p
}

fn single(fc: u8, addr: u16, value: u16) -> Vec<u8> {
    read(fc, addr, value)
}

fn regs(start: u16, values: &[u16]) -> Vec<u8> {
    let mut p = vec![WRITE_MULTIPLE_REGISTERS];
    p.extend(start.to_be_bytes());
    p.extend(u16::try_from(values.len()).unwrap().to_be_bytes());
    p.push(u8::try_from(values.len() * 2).unwrap());
    for v in values {
        p.extend(v.to_be_bytes());
    }
    p
}

fn coils(start: u16, values: &[bool]) -> Vec<u8> {
    let mut p = vec![WRITE_MULTIPLE_COILS];
    p.extend(start.to_be_bytes());
    p.extend(u16::try_from(values.len()).unwrap().to_be_bytes());
    let n = values.len().div_ceil(8);
    p.push(u8::try_from(n).unwrap());
    let mut bytes = vec![0u8; n];
    for (i, v) in values.iter().enumerate() {
        if *v {
            bytes[i / 8] |= 1 << (i % 8);
        }
    }
    p.extend(bytes);
    p
}

fn exception(fc: u8, code: u8) -> Vec<u8> {
    vec![fc | 0x80, code]
}

fn value(b: &ShareBank, m: &IoMap, i: usize) -> Option<SignalValue> {
    let s = &m.shares[i];
    value_slot(&b.image, s.fmt, s.at)
}

#[test]
fn every_read_function_answers_what_the_program_holds() {
    let m = map();
    let mut b = bank(&m);
    assert_eq!(
        ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 0, 1)),
        [3, 2, 0x05, 0xDC]
    );
    // A 32-bit int is two registers, the high word first: -70000 = 0xFFFEEE90.
    assert_eq!(
        ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 10, 2)),
        [3, 4, 0xFF, 0xFE, 0xEE, 0x90]
    );
    // A read may start in the middle of a 32-bit value.
    assert_eq!(
        ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 11, 1)),
        [3, 2, 0xEE, 0x90]
    );
    // 2.5f32 = 0x40200000.
    assert_eq!(
        ask(&mut b, &m, &read(READ_INPUT_REGISTERS, 4, 2)),
        [4, 4, 0x40, 0x20, 0x00, 0x00]
    );
    assert_eq!(
        ask(&mut b, &m, &read(READ_INPUT_REGISTERS, 0, 1)),
        [4, 2, 0xFF, 0xFF]
    );
    assert_eq!(ask(&mut b, &m, &read(READ_DISCRETE_INPUTS, 0, 1)), [2, 1, 1]);
    assert_eq!(ask(&mut b, &m, &read(READ_COILS, 0, 1)), [1, 1, 0]);
    // Coils 0 and 5 are shared, 1 to 4 are not: a range over the gap is refused whole.
    assert_eq!(
        ask(&mut b, &m, &read(READ_COILS, 0, 6)),
        exception(READ_COILS, ILLEGAL_DATA_ADDRESS)
    );
}

#[test]
fn the_sw_formats_swap_the_words_on_the_wire() {
    let m = map();
    let mut b = bank(&m);
    // target is f32sw at hr:20: 1.0f32 = 0x3F800000, the low word first.
    assert_eq!(
        ask(&mut b, &m, &regs(20, &[0x0000, 0x3F80])),
        [WRITE_MULTIPLE_REGISTERS, 0, 20, 0, 2]
    );
    assert_eq!(value(&b, &m, TARGET), Some(SignalValue::Real(1.0)));
    assert_eq!(
        ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 20, 2)),
        [3, 4, 0x00, 0x00, 0x3F, 0x80]
    );
}

#[test]
fn a_coil_write_goes_to_a_writable_share_only() {
    let m = map();
    let mut b = bank(&m);
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_COIL, 0, COIL_ON)),
        single(WRITE_SINGLE_COIL, 0, COIL_ON),
        "the reply echoes the request"
    );
    assert_eq!(value(&b, &m, RUN_CMD), Some(SignalValue::Bool(true)));
    assert_eq!(
        ask(&mut b, &m, &read(READ_COILS, 0, 1)),
        [1, 1, 1],
        "and a read sees it"
    );
    ask(&mut b, &m, &single(WRITE_SINGLE_COIL, 0, COIL_OFF));
    assert_eq!(value(&b, &m, RUN_CMD), Some(SignalValue::Bool(false)));
    // Shared, in the coil table, but the program's own output.
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_COIL, 5, COIL_ON)),
        exception(WRITE_SINGLE_COIL, ILLEGAL_DATA_ADDRESS)
    );
    assert_eq!(value(&b, &m, ARMED), Some(SignalValue::Bool(false)));
    // Not shared at all.
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_COIL, 9, COIL_ON)),
        exception(WRITE_SINGLE_COIL, ILLEGAL_DATA_ADDRESS)
    );
    // A coil value is 0xFF00 or 0x0000.
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_COIL, 0, 1)),
        exception(WRITE_SINGLE_COIL, ILLEGAL_DATA_VALUE)
    );
}

#[test]
fn a_multiple_write_is_all_or_nothing() {
    let m = map();
    let mut b = bank(&m);
    // Coils 0 (writable) and 1 (not shared).
    assert_eq!(
        ask(&mut b, &m, &coils(0, &[true, true])),
        exception(WRITE_MULTIPLE_COILS, ILLEGAL_DATA_ADDRESS)
    );
    assert_eq!(
        value(&b, &m, RUN_CMD),
        Some(SignalValue::Bool(false)),
        "nothing was written"
    );
    assert_eq!(
        ask(&mut b, &m, &coils(0, &[true])),
        [WRITE_MULTIPLE_COILS, 0, 0, 0, 1]
    );
    assert_eq!(value(&b, &m, RUN_CMD), Some(SignalValue::Bool(true)));
    // 32-bit pos (hr:10) with target (hr:20) in one request is not a range any share spans.
    let before = b.clone();
    assert_eq!(
        ask(&mut b, &m, &regs(10, &[1, 2, 3])),
        exception(WRITE_MULTIPLE_REGISTERS, ILLEGAL_DATA_ADDRESS)
    );
    assert_eq!(b, before);
}

#[test]
fn registers_write_whole_values_and_only_where_the_program_takes_input() {
    let m = map();
    let mut b = bank(&m);
    // pos is i32 at hr:10: both registers in one FC 16.
    assert_eq!(
        ask(&mut b, &m, &regs(10, &[0x0001, 0x0000])),
        [WRITE_MULTIPLE_REGISTERS, 0, 10, 0, 2]
    );
    assert_eq!(value(&b, &m, POS), Some(SignalValue::Int(65_536)));
    // Half of it is not a value: FC 6 cannot write a 32-bit share, FC 16 may not stop short.
    let before = b.clone();
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_REGISTER, 10, 5)),
        exception(WRITE_SINGLE_REGISTER, ILLEGAL_DATA_VALUE)
    );
    assert_eq!(
        ask(&mut b, &m, &regs(11, &[5])),
        exception(WRITE_MULTIPLE_REGISTERS, ILLEGAL_DATA_VALUE)
    );
    assert_eq!(b, before);
    // speed is shared in hr but the program's output.
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_REGISTER, 0, 9)),
        exception(WRITE_SINGLE_REGISTER, ILLEGAL_DATA_ADDRESS)
    );
    assert_eq!(value(&b, &m, SPEED), Some(SignalValue::Int(1500)));
    // Input registers have no write function; the nearest hr address is unshared.
    assert_eq!(
        ask(&mut b, &m, &single(WRITE_SINGLE_REGISTER, 4, 9)),
        exception(WRITE_SINGLE_REGISTER, ILLEGAL_DATA_ADDRESS)
    );
}

#[test]
fn a_single_register_write_to_a_16_bit_share_lands() {
    let m = iomap::parse("share setpoint int hr:3:i16\nshare big int hr:4:u16\n").unwrap();
    let mut b = ShareBank {
        writable: 0b11,
        ..ShareBank::default()
    };
    ask(&mut b, &m, &single(WRITE_SINGLE_REGISTER, 3, 0xFFFE));
    ask(&mut b, &m, &single(WRITE_SINGLE_REGISTER, 4, 0xFFFE));
    assert_eq!(value(&b, &m, 0), Some(SignalValue::Int(-2)), "i16 reads signed");
    assert_eq!(value(&b, &m, 1), Some(SignalValue::Int(65_534)));
}

#[test]
fn a_real_written_as_nan_or_infinity_is_refused() {
    let m = map();
    let mut b = bank(&m);
    // target is f32sw: low word first. NaN = 0x7FC00000, +inf = 0x7F800000.
    for (lo, hi) in [(0x0000u16, 0x7FC0u16), (0x0000, 0x7F80), (0x0000, 0xFF80)] {
        assert_eq!(
            ask(&mut b, &m, &regs(20, &[lo, hi])),
            exception(WRITE_MULTIPLE_REGISTERS, ILLEGAL_DATA_VALUE)
        );
    }
    assert_eq!(value(&b, &m, TARGET), Some(SignalValue::Real(0.0)));
}

#[test]
fn quantities_and_lengths_outside_the_spec_answer_exception_3() {
    let m = map();
    let mut b = bank(&m);
    for (fc, qty) in [
        (READ_COILS, 0),
        (READ_COILS, MAX_READ_BITS + 1),
        (READ_DISCRETE_INPUTS, 0),
        (READ_HOLDING_REGISTERS, 0),
        (READ_HOLDING_REGISTERS, MAX_READ_REGS + 1),
        (READ_INPUT_REGISTERS, 0xFFFF),
    ] {
        assert_eq!(
            ask(&mut b, &m, &read(fc, 0, qty)),
            exception(fc, ILLEGAL_DATA_VALUE),
            "fc {fc} qty {qty}"
        );
    }
    // A read past the end of the table is an address, not a quantity.
    assert_eq!(
        ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 0xFFFF, 2)),
        exception(READ_HOLDING_REGISTERS, ILLEGAL_DATA_ADDRESS)
    );
    // A PDU too short or too long for its function.
    for pdu in [
        vec![READ_COILS],
        vec![READ_COILS, 0, 0, 0],
        vec![READ_COILS, 0, 0, 0, 1, 0],
        vec![WRITE_SINGLE_COIL, 0, 0, 0xFF],
        vec![WRITE_SINGLE_REGISTER, 0, 0, 0, 1, 2],
        vec![WRITE_MULTIPLE_COILS, 0, 0, 0, 1],
        vec![WRITE_MULTIPLE_REGISTERS, 0, 0, 0, 1, 2, 0],
    ] {
        assert_eq!(
            ask(&mut b, &m, &pdu),
            exception(pdu[0], ILLEGAL_DATA_VALUE),
            "{pdu:?}"
        );
    }
    // Byte count that disagrees with the quantity.
    let mut bad = coils(0, &[true]);
    bad[5] = 2;
    bad.push(0);
    assert_eq!(
        ask(&mut b, &m, &bad),
        exception(WRITE_MULTIPLE_COILS, ILLEGAL_DATA_VALUE)
    );
    let mut bad = regs(10, &[1, 2]);
    bad[5] = 2;
    assert_eq!(
        ask(&mut b, &m, &bad),
        exception(WRITE_MULTIPLE_REGISTERS, ILLEGAL_DATA_VALUE)
    );
    // Writes of more than the spec allows.
    let mut huge = vec![WRITE_MULTIPLE_REGISTERS, 0, 0];
    huge.extend((MAX_WRITE_REGS + 1).to_be_bytes());
    huge.push(0);
    assert_eq!(
        ask(&mut b, &m, &huge),
        exception(WRITE_MULTIPLE_REGISTERS, ILLEGAL_DATA_VALUE)
    );
}

#[test]
fn another_function_is_illegal_and_any_unit_is_answered() {
    let m = map();
    let mut b = bank(&m);
    for fc in [0u8, 7, 8, 0x11, 0x16, 0x17, 0x2B, 0x7F, 0x80, 0xFF] {
        assert_eq!(
            ask(&mut b, &m, &[fc, 0, 0, 0, 1]),
            exception(fc, ILLEGAL_FUNCTION),
            "fc {fc}"
        );
    }
    for unit in [0u8, 1, 17, 255] {
        let r = b
            .handle(&m.shares, &frame(0xBEEF, unit, &read(READ_COILS, 0, 1)))
            .unwrap();
        assert_eq!(
            (r[6], &r[..2], &r[7..]),
            (unit, &[0xBE, 0xEF][..], &[1, 1, 0][..])
        );
    }
}

#[test]
fn a_node_with_no_shares_at_an_address_says_so() {
    let m = iomap::parse("device rio modbus-tcp 10.0.0.1\n").unwrap();
    let mut b = ShareBank::default();
    assert_eq!(
        ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 0, 1)),
        exception(READ_HOLDING_REGISTERS, ILLEGAL_DATA_ADDRESS)
    );
}

#[test]
fn the_header_is_judged_before_the_body() {
    assert_eq!(head(&[]), Head::Need(MBAP_LEN));
    assert_eq!(head(&[0; 6]), Head::Need(MBAP_LEN));
    let f = frame(1, 1, &read(READ_COILS, 0, 1));
    assert_eq!(f.len(), 12);
    assert_eq!(head(&f[..7]), Head::Need(12));
    assert_eq!(head(&f[..11]), Head::Need(12));
    assert_eq!(head(&f), Head::Frame(12));
    let mut two = f.clone();
    two.extend(&f);
    assert_eq!(head(&two), Head::Frame(12), "the first of two");
    let mut other = f.clone();
    other[3] = 1;
    assert!(matches!(head(&other), Head::Refuse(_)), "protocol id");
    for len in [0u16, 1, 255, 0xFFFF] {
        let mut h = f.clone();
        h[4..6].copy_from_slice(&len.to_be_bytes());
        assert!(matches!(head(&h), Head::Refuse(_)), "length {len}");
    }
    // The largest frame is taken, and never asks for more than MAX_FRAME.
    let mut big = f;
    big[4..6].copy_from_slice(&254u16.to_be_bytes());
    assert_eq!(head(&big[..7]), Head::Need(MAX_FRAME));
}

#[test]
fn serve_refuses_what_head_would() {
    let m = map();
    let b = bank(&m);
    let f = frame(1, 1, &read(READ_COILS, 0, 1));
    assert!(serve(&m.shares, &b, &f).is_some());
    assert!(serve(&m.shares, &b, &f[..11]).is_none(), "cut short");
    let mut long = f.clone();
    long.push(0);
    assert!(serve(&m.shares, &b, &long).is_none(), "trailing byte");
    let mut proto = f;
    proto[2] = 9;
    assert!(serve(&m.shares, &b, &proto).is_none());
}

#[test]
fn the_largest_reads_fit_a_frame() {
    // 32 shares of 4 bytes each is the most a map holds; read the whole table.
    let mut text = String::new();
    for i in 0..32 {
        text += &format!("share v{i} int hr:{i}\n");
    }
    let m = iomap::parse(&text).unwrap();
    let mut b = ShareBank::default();
    let r = ask(&mut b, &m, &read(READ_HOLDING_REGISTERS, 0, 32));
    assert_eq!(r.len(), 2 + 64);
}

proptest! {
    /// Whatever arrives, the framing and the server never panic, a frame
    /// they accept is answered with a well-formed frame carrying the
    /// request's transaction and unit, and nothing but a writable share's
    /// bytes ever changes.
    #[test]
    fn hostile_bytes_never_panic_and_never_reach_the_program_side(
        bytes in proptest::collection::vec(any::<u8>(), 0..300),
    ) {
        let m = map();
        let b = bank(&m);
        let _ = head(&bytes);
        let _ = serve(&m.shares, &b, &bytes);
    }

    #[test]
    fn a_framed_request_gets_a_framed_reply(
        tid in any::<u16>(),
        unit in any::<u8>(),
        pdu in proptest::collection::vec(any::<u8>(), 1..=MAX_PDU),
        seed in proptest::collection::vec(any::<u8>(), SHARE_BYTES),
    ) {
        let m = map();
        let mut b = bank(&m);
        b.image.copy_from_slice(&seed);
        let before = b.clone();
        let req = frame(tid, unit, &pdu);
        prop_assert_eq!(head(&req), Head::Frame(req.len()));
        let reply = serve(&m.shares, &b, &req).expect("a frame is answered");
        let f = &reply.frame;
        prop_assert_eq!(&f[..2], &tid.to_be_bytes());
        prop_assert_eq!(&f[2..4], &[0u8, 0]);
        prop_assert_eq!(usize::from(u16::from_be_bytes([f[4], f[5]])), f.len() - 6);
        prop_assert_eq!(f[6], unit);
        prop_assert!(f.len() <= MAX_FRAME);
        prop_assert!(f[7] == pdu[0] || f[7] == pdu[0] | 0x80, "function echoed, or its exception");
        if f[7] & 0x80 != 0 {
            prop_assert!(reply.writes.is_empty(), "an exception writes nothing");
            prop_assert_eq!(f.len(), 9);
        }
        let mut after = b.clone();
        after.apply(&m.shares, &reply.writes);
        // Only shares the program takes as input may have changed.
        for (k, s) in m.shares.iter().enumerate() {
            if !b.may_write(k) {
                let (from, n) = (usize::from(s.at.byte), usize::from(s.at.bytes));
                if s.at.bit.is_some() {
                    prop_assert_eq!(bit_of(&after.image, s.at), bit_of(&before.image, s.at), "{}", s.name);
                } else {
                    prop_assert_eq!(&after.image[from..from + n], &before.image[from..from + n], "{}", s.name);
                }
            }
        }
        prop_assert_eq!(after.writable, before.writable);
    }

    /// Cutting a stream of frames at any byte and feeding it in two parts
    /// finds the same frames: the framing does not depend on how TCP
    /// delivered the bytes.
    #[test]
    fn framing_is_independent_of_segmentation(
        pdus in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 1..40), 1..5),
        cut in 0usize..400,
    ) {
        let stream: Vec<u8> = pdus.iter().flat_map(|p| frame(1, 1, p)).collect();
        let cut = cut.min(stream.len());
        let whole = split(&stream);
        let mut buf = stream[..cut].to_vec();
        let mut found = split_prefix(&mut buf);
        buf.extend(&stream[cut..]);
        found.extend(split(&buf));
        prop_assert_eq!(found, whole);
    }
}

/// All frames in a buffer that ends on a frame boundary.
fn split(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut at = 0;
    let mut out = Vec::new();
    while let Head::Frame(n) = head(&buf[at..]) {
        out.push(buf[at..at + n].to_vec());
        at += n;
    }
    out
}

/// The whole frames at the start of `buf`, removed from it.
fn split_prefix(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Head::Frame(n) = head(buf) {
        out.push(buf.drain(..n).collect());
    }
    out
}
