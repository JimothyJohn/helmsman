#![allow(clippy::format_push_string)] // tests build map text a line at a time

use super::*;
use crate::iomap::tests::THREE;
use crate::iomap::{IMAGE_BYTES, parse};
use alloc::format;
use alloc::string::String;
use proptest::prelude::*;

fn modbus(m: &IoMap, k: usize) -> ModbusPlan {
    match m.plan(k) {
        Plan::Modbus(p) => p,
        Plan::Enip(_) => panic!("device {k} is not modbus"),
    }
}

fn enip(m: &IoMap, k: usize) -> EnipPlan {
    match m.plan(k) {
        Plan::Enip(p) => p,
        Plan::Modbus(_) => panic!("device {k} is not enip"),
    }
}

fn point<'a>(m: &'a IoMap, name: &str) -> &'a Point {
    m.points.iter().find(|p| p.name == name).unwrap()
}

/// Puts `bytes` at a point's image slot.
fn put(image: &mut [u8], m: &IoMap, name: &str, bytes: &[u8]) {
    let at = point(m, name).at;
    image[usize::from(at.byte)..usize::from(at.byte) + bytes.len()].copy_from_slice(bytes);
}

#[test]
fn each_device_of_a_map_gets_its_own_plan() {
    let m = parse(THREE).unwrap();
    let rio = modbus(&m, 0);
    assert_eq!(rio.unit, 1);
    assert_eq!(rio.reads.len(), 1);
    assert_eq!(
        (rio.reads[0].kind, rio.reads[0].start, rio.reads[0].qty),
        (ReadKind::Discrete, 0, 1)
    );
    assert_eq!(
        (rio.writes[0].kind, rio.writes[0].start, rio.writes[0].qty),
        (WriteKind::Coils, 0, 1)
    );
    let press = modbus(&m, 2);
    assert_eq!(press.reads.len(), 1);
    assert_eq!(
        (press.reads[0].kind, press.reads[0].start, press.reads[0].qty),
        (ReadKind::Holding, 5, 2)
    );
    assert_eq!(press.writes.len(), 0);
    let drive = enip(&m, 1);
    assert_eq!((drive.link.t_to_o_len, drive.link.o_to_t_len), (16, 36));
    assert_eq!((drive.inputs.len(), drive.outputs.len()), (2, 2));
}

#[test]
fn reads_join_near_registers_and_writes_only_adjacent_ones() {
    let t = "device a modbus-tcp 10.0.0.1\n\
        in r0 int a hr:0\nin r1 int a hr:5:u32\nin r2 int a hr:30\nin r3 int a hr:124\nin r4 int a hr:125\n\
        out w0 int a hr:10\nout w1 int a hr:11:u32\nout w2 int a hr:14\nout c3 bool a coil:3\nout c1 bool a coil:1\n";
    let p = modbus(&parse(t).unwrap(), 0);
    let spans: Vec<(u16, u16)> = p.reads.iter().map(|r| (r.start, r.qty)).collect();
    // 0..7 (r0, r1), 30 alone, and 124..126 (r3, r4).
    assert_eq!(spans, [(0, 7), (30, 1), (124, 2)]);
    let w: Vec<(WriteKind, u16, u16)> = p.writes.iter().map(|w| (w.kind, w.start, w.qty)).collect();
    // Coils from 0 to the highest named; registers 10 to 12 together and 14
    // apart: 13 is nobody's and is not written.
    assert_eq!(
        w,
        [
            (WriteKind::Coils, 0, 4),
            (WriteKind::Holding, 10, 3),
            (WriteKind::Holding, 14, 1)
        ]
    );
    // No request asks for more than the protocol allows.
    let mut t = String::from("device a modbus-tcp 10.0.0.1\n");
    for i in 0..32 {
        t += &format!("in r{i} int a hr:{}\n", i * 8);
    }
    let p = modbus(&parse(&t).unwrap(), 0);
    assert!(p.reads.len() >= 2);
    for r in &p.reads {
        assert!(r.qty <= MAX_READ_REGS, "{}", r.qty);
    }
}

#[test]
fn modbus_registers_land_in_the_image_and_come_back_out() {
    let t = "device a modbus-tcp 10.0.0.1\n\
        in a int a hr:0:i16\nin b int a hr:1:u32\nin c int a hr:3:i32sw\nin d real a hr:5:f32\nin e real a hr:7:f32sw\nin f bool a di:2\n\
        out oa int a hr:20:i16\nout ob int a hr:21:u32sw\nout oc real a hr:23:f32\nout od bool a coil:0\n";
    let m = parse(t).unwrap();
    let plan = modbus(&m, 0);
    let mut image = [0u8; IMAGE_BYTES];
    let regs = [
        0xFFFE, 0x0001, 0x0002, 0x0001, 0xFFFF, 0x3FC0, 0x0000, 0x0000, 0x3FC0,
    ];
    let reads: Vec<&Read> = plan
        .reads
        .iter()
        .filter(|r| r.kind == ReadKind::Holding)
        .collect();
    assert_eq!(reads.len(), 1);
    reads[0].store_regs(&regs, &mut image);
    plan.reads
        .iter()
        .find(|r| r.kind == ReadKind::Discrete)
        .unwrap()
        .store_bits(&[false, false, true], &mut image);
    let val = |n: &str| value_at(&image, point(&m, n));
    assert_eq!(val("a"), Some(SignalValue::Int(-2)));
    assert_eq!(val("b"), Some(SignalValue::Int(0x0001_0002)));
    // i32sw sends the low word first: registers 0x0001, 0xFFFF are 0xFFFF0001.
    assert_eq!(
        val("c"),
        Some(SignalValue::Int(i64::from(0xFFFF_0001u32.cast_signed())))
    );
    assert_eq!(val("d"), Some(SignalValue::Real(1.5)));
    assert_eq!(val("e"), Some(SignalValue::Real(1.5)));
    assert_eq!(val("f"), Some(SignalValue::Bool(true)));

    // Outputs: the image gives the registers, high word first unless `sw`.
    let mut out = [0u8; IMAGE_BYTES];
    put(&mut out, &m, "oa", &(-5i16).to_le_bytes());
    put(&mut out, &m, "ob", &0x1234_5678u32.to_le_bytes());
    put(&mut out, &m, "oc", &1.5f32.to_le_bytes());
    set_bit(&mut out, point(&m, "od").at, true);
    let writes: Vec<&Write> = plan
        .writes
        .iter()
        .filter(|w| w.kind == WriteKind::Holding)
        .collect();
    assert_eq!(writes.len(), 1, "20, 21-22 and 23-24 are adjacent");
    assert_eq!(writes[0].start, 20);
    assert_eq!(
        writes[0].registers(&out),
        [0xFFFB, 0x5678, 0x1234, 0x3FC0, 0x0000]
    );
    let coils = plan.writes.iter().find(|w| w.kind == WriteKind::Coils).unwrap();
    assert_eq!(coils.coils(&out), [true]);
}

#[test]
fn a_short_reply_or_a_short_image_changes_nothing_and_does_not_panic() {
    let m = parse("device a modbus-tcp 10.0.0.1\nin a int a hr:0:u32\nin b bool a di:5\n").unwrap();
    let plan = modbus(&m, 0);
    let discrete = plan.reads.iter().find(|r| r.kind == ReadKind::Discrete).unwrap();
    let holding = plan.reads.iter().find(|r| r.kind == ReadKind::Holding).unwrap();
    let mut image = [0u8; IMAGE_BYTES];
    holding.store_regs(&[1], &mut image);
    discrete.store_bits(&[true], &mut image);
    assert_eq!(image, [0u8; IMAGE_BYTES]);
    let mut tiny = [0u8; 3];
    holding.store_regs(&[1, 2], &mut tiny);
    discrete.store_regs(&[1, 2], &mut tiny);
    discrete.store_bits(&[true; 6], &mut tiny);
    assert_eq!(value_at(&tiny, point(&m, "a")), None);
    assert_eq!(value_at(&[], point(&m, "b")), Some(SignalValue::Bool(false)));
}

#[test]
fn an_assembly_moves_bits_and_numbers_both_ways() {
    let m = parse(
        "device e enip-io 10.0.0.5 config=1 o2t=2:12 t2o=3:16\n\
        in b0 bool e di:0\nin b9 bool e in:1.1\nin u int e in:4:u8\nin s int e in:5:i8\nin w int e in:6:i16\nin d int e in:8:u32\nin f real e in:12:f32\n\
        out o0 bool e do:1\nout ou int e out:2:u8\nout os int e out:3:i8\nout ow int e out:4:u16\nout of real e out:8:f32\n",
    )
    .unwrap();
    let plan = enip(&m, 0);
    let mut data = [0u8; 16];
    data[0] = 0b1;
    data[1] = 0b10;
    data[4] = 200;
    data[5] = 0xFE;
    data[6..8].copy_from_slice(&(-300i16).to_le_bytes());
    data[8..12].copy_from_slice(&4_000_000_000u32.to_le_bytes());
    data[12..16].copy_from_slice(&(-2.25f32).to_le_bytes());
    let mut image = [0u8; IMAGE_BYTES];
    plan.store_inputs(&data, &mut image);
    let val = |image: &[u8], n: &str| value_at(image, point(&m, n));
    assert_eq!(val(&image, "b0"), Some(SignalValue::Bool(true)));
    assert_eq!(val(&image, "b9"), Some(SignalValue::Bool(true)));
    assert_eq!(val(&image, "u"), Some(SignalValue::Int(200)));
    assert_eq!(
        val(&image, "s"),
        Some(SignalValue::Int(-2)),
        "i8 is sign-extended"
    );
    assert_eq!(val(&image, "w"), Some(SignalValue::Int(-300)));
    assert_eq!(val(&image, "d"), Some(SignalValue::Int(4_000_000_000)));
    assert_eq!(val(&image, "f"), Some(SignalValue::Real(-2.25)));
    // A bit that went low goes low.
    data[0] = 0;
    plan.store_inputs(&data, &mut image);
    assert_eq!(val(&image, "b0"), Some(SignalValue::Bool(false)));
    // A short packet keeps what the points past it had.
    plan.store_inputs(&data[..5], &mut image);
    assert_eq!(val(&image, "d"), Some(SignalValue::Int(4_000_000_000)));

    // Outputs: what the map names, 8-bit values saturating, zero elsewhere.
    let mut out = [0u8; IMAGE_BYTES];
    set_bit(&mut out, point(&m, "o0").at, true);
    put(&mut out, &m, "ou", &300u16.to_le_bytes());
    put(&mut out, &m, "os", &(-200i16).to_le_bytes());
    put(&mut out, &m, "ow", &513u16.to_le_bytes());
    put(&mut out, &m, "of", &0.5f32.to_le_bytes());
    let mut bytes = [0xAAu8; 12];
    plan.fill_outputs(&out, &mut bytes);
    assert_eq!(bytes[0], 0b10, "do:1 is bit 1 of byte 0");
    assert_eq!(bytes[1], 0);
    assert_eq!(bytes[2], 255, "300 saturates");
    assert_eq!(bytes[3].cast_signed(), -128, "-200 saturates");
    assert_eq!(&bytes[4..6], 513u16.to_le_bytes());
    assert_eq!(&bytes[6..8], [0, 0]);
    assert_eq!(&bytes[8..12], 0.5f32.to_le_bytes());
}

#[test]
fn nothing_named_in_an_assembly_is_sent_as_zero() {
    let m = parse("device e enip-io 10.0.0.5 config=1 o2t=2:4 t2o=3:4\nout o bool e do:9\n").unwrap();
    let plan = enip(&m, 0);
    let mut out = [0u8; IMAGE_BYTES];
    let mut bytes = [0xFFu8; 4];
    plan.fill_outputs(&out, &mut bytes);
    assert_eq!(bytes, [0; 4]);
    set_bit(&mut out, point(&m, "o").at, true);
    plan.fill_outputs(&out, &mut bytes);
    assert_eq!(bytes, [0, 0b10, 0, 0], "do:9 is byte 1 bit 1");
    // An assembly shorter than the map's own points never panics.
    plan.fill_outputs(&out, &mut []);
}

proptest! {
    /// A value that goes out through a Modbus format and comes back in
    /// through the same one is the same bytes, in either word order.
    #[test]
    fn a_modbus_value_survives_the_round_trip_through_every_format(x in any::<u32>(), which in 0usize..8) {
        let (f, ty, n) = [
            ("u16", "int", 2), ("i16", "int", 2), ("u32", "int", 4), ("u32sw", "int", 4),
            ("i32", "int", 4), ("i32sw", "int", 4), ("f32", "real", 4), ("f32sw", "real", 4),
        ][which];
        let text = format!("device a modbus-tcp 10.0.0.1\nin i {ty} a hr:0:{f}\nout o {ty} a hr:0:{f}\n");
        let m = parse(&text).unwrap();
        let plan = modbus(&m, 0);
        let mut image = [0u8; IMAGE_BYTES];
        let v = x.to_le_bytes();
        put(&mut image, &m, "o", &v[..n]);
        let regs = plan.writes[0].registers(&image);
        prop_assert_eq!(regs.len(), n / 2);
        let mut back = [0u8; IMAGE_BYTES];
        plan.reads[0].store_regs(&regs, &mut back);
        let at = point(&m, "i").at;
        prop_assert_eq!(&back[usize::from(at.byte)..usize::from(at.byte) + n], &v[..n]);
    }

    /// An EtherNet/IP number goes out and comes back the same through
    /// every format, and 8-bit outputs saturate instead of wrapping.
    #[test]
    fn an_assembly_value_survives_the_round_trip_and_eight_bits_saturate(v in any::<i32>(), which in 0usize..7) {
        let f = ["u8", "i8", "u16", "i16", "u32", "i32", "f32"][which];
        let ty = if f == "f32" { "real" } else { "int" };
        let text = format!("device e enip-io 10.0.0.5 config=1 o2t=2:8 t2o=3:8\nin i {ty} e in:0:{f}\nout o {ty} e out:0:{f}\n");
        let m = parse(&text).unwrap();
        let plan = enip(&m, 0);
        let at = point(&m, "o").at;
        let n = usize::from(at.bytes);
        let le = v.to_le_bytes();
        let mut image = [0u8; IMAGE_BYTES];
        put(&mut image, &m, "o", &le[..n]);
        let mut bytes = [0u8; 8];
        plan.fill_outputs(&image, &mut bytes);
        let mut back = [0u8; IMAGE_BYTES];
        plan.store_inputs(&bytes, &mut back);
        let at_i = point(&m, "i").at;
        let got = &back[usize::from(at_i.byte)..usize::from(at_i.byte) + n];
        match f {
            "u8" => prop_assert_eq!(got, &u16::from_le_bytes([le[0], le[1]]).min(255).to_le_bytes()[..]),
            "i8" => prop_assert_eq!(got, &i16::from_le_bytes([le[0], le[1]]).clamp(-128, 127).to_le_bytes()[..]),
            _ => prop_assert_eq!(got, &le[..n]),
        }
    }
}

#[test]
fn two_devices_never_write_one_anothers_bool_windows() {
    let m = parse(
        "device a modbus-tcp 10.0.0.1\ndevice e enip-io 10.0.0.5 config=1 o2t=2:4 t2o=3:8\n\
        in x bool a di:31\nin y bool e di:0\nin z bool e di:31\n",
    )
    .unwrap();
    let mut image = [0u8; IMAGE_BYTES];
    modbus(&m, 0).reads[0].store_bits(&[true; 32], &mut image);
    assert_eq!(
        &image[..8],
        [0, 0, 0, 0x80, 0, 0, 0, 0],
        "only bit 31 of device 0's window"
    );
    enip(&m, 1).store_inputs(&[0xFF; 8], &mut image);
    assert_eq!(&image[..4], [0, 0, 0, 0x80]);
    assert_eq!(image[4], 1, "device 1's window starts at byte 4");
    assert_eq!(image[7], 0x80);
}
