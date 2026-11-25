//! What a node does with a map (helmsman-nodecore's `iomap` and `ioplan`)
//! against what the box's own drivers do with the same addresses: the box
//! is the second implementation, so a register order or a format the node
//! gets wrong shows here, on the host, before a board is involved. The
//! Modbus half runs the node's requests against the simulated device; the
//! EtherNet/IP half holds the node's assembly codec to the box's `Point`.

#![allow(clippy::format_push_string)] // tests build map text a line at a time

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_io::enip::Point as EnipPoint;
use helmsman_io::modbus::{Client, ModbusDriver};
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Driver, Value};
use helmsman_nodecore::api::SignalValue;
use helmsman_nodecore::iomap::{self, Dir, Fmt, IMAGE_BYTES, IoMap, Point};
use helmsman_nodecore::ioplan::{Plan, ReadKind, WriteKind, set_bit, value_at};
use proptest::prelude::*;

const READ_DISCRETE_INPUTS: u8 = 0x02;
const READ_HOLDING_REGISTERS: u8 = 0x03;
const READ_INPUT_REGISTERS: u8 = 0x04;

/// `(direction, type, name, address)`: the points of the test's device.
const POINTS: &[(&str, &str, &str, &str)] = &[
    ("in", "bool", "di0", "di:0"),
    ("in", "bool", "di3", "di:3"),
    ("in", "bool", "di31", "di:31"),
    ("in", "int", "w", "hr:10:i16"),
    ("in", "int", "u", "hr:11:u32"),
    ("in", "int", "usw", "hr:13:u32sw"),
    ("in", "int", "i", "hr:15:i32"),
    ("in", "int", "isw", "hr:17:i32sw"),
    ("in", "real", "f", "hr:19:f32"),
    ("in", "real", "fsw", "hr:21:f32sw"),
    ("in", "int", "plain", "hr:23"),
    ("in", "int", "ir", "ir:2:i16"),
    ("in", "real", "irf", "ir:4:f32"),
    ("out", "bool", "c0", "coil:0"),
    ("out", "bool", "c2", "coil:2"),
    ("out", "int", "ow", "hr:30:u16"),
    ("out", "int", "od", "hr:31:i32sw"),
    ("out", "int", "ou", "hr:33:u32"),
    ("out", "real", "of", "hr:35:f32"),
    ("out", "real", "ofsw", "hr:37:f32sw"),
];

fn map_text(endpoint: &str) -> String {
    let mut text = format!("device dev modbus-tcp {endpoint} unit=1\n");
    for (dir, ty, name, addr) in POINTS {
        text += &format!("{dir} {name} {ty} dev {addr}\n");
    }
    text
}

fn point<'a>(map: &'a IoMap, name: &str) -> &'a Point {
    map.points.iter().find(|p| p.name == name).unwrap()
}

/// Both sides' values equal, a NaN equal to a NaN.
fn same(box_value: Value, node: Option<SignalValue>) -> bool {
    match (box_value, node) {
        (Value::Bool(a), Some(SignalValue::Bool(b))) => a == b,
        (Value::Int(a), Some(SignalValue::Int(b))) => a == b,
        (Value::Real(a), Some(SignalValue::Real(b))) => {
            a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
        }
        _ => false,
    }
}

/// The image bytes of a value in a point's slot.
fn put(image: &mut [u8], p: &Point, v: Value) {
    let at = p.at;
    let bytes = match v {
        Value::Bool(on) => return set_bit(image, at, on),
        Value::Int(n) => n.to_le_bytes().to_vec(),
        #[allow(clippy::cast_possible_truncation)]
        Value::Real(x) => (x as f32).to_le_bytes().to_vec(),
    };
    let n = usize::from(at.bytes);
    image[usize::from(at.byte)..usize::from(at.byte) + n].copy_from_slice(&bytes[..n]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The node's reads ask the device for what the map names and read the
    /// answers as the box's driver reads the same addresses; the node's
    /// writes leave in the device what the box's driver reads back.
    #[test]
    fn a_node_moves_modbus_values_as_the_boxs_driver_does(
        holding in prop::collection::vec(any::<u16>(), 64),
        input in prop::collection::vec(any::<u16>(), 16),
        discrete in prop::collection::vec(any::<bool>(), 32),
        out_w in any::<u16>(), out_d in any::<i32>(), out_u in any::<u32>(),
        out_f in any::<f32>().prop_filter("finite", |f| f.is_finite()),
        out_fsw in any::<f32>().prop_filter("finite", |f| f.is_finite()),
        c0 in any::<bool>(), c2 in any::<bool>(),
    ) {
        let sim = Sim::start(State {
            coils: vec![true; 32],
            discrete: discrete.clone(),
            holding: holding.clone(),
            input: input.clone(),
            ..State::default()
        }).unwrap();
        let text = map_text(&sim.endpoint());
        let map = iomap::parse(&text).unwrap();
        let Plan::Modbus(plan) = map.plan(0) else { panic!("modbus") };
        let mut client = Client::new(&sim.endpoint(), plan.unit, Duration::from_millis(500));

        // Reads, as the node's round makes them.
        let mut image = [0u8; IMAGE_BYTES];
        for read in &plan.reads {
            match read.kind {
                ReadKind::Discrete => {
                    let bits = client.read_bits(READ_DISCRETE_INPUTS, read.start, read.qty).unwrap();
                    read.store_bits(&bits, &mut image);
                }
                ReadKind::Input => {
                    let regs = client.read_registers(READ_INPUT_REGISTERS, read.start, read.qty).unwrap();
                    read.store_regs(&regs, &mut image);
                }
                ReadKind::Holding => {
                    let regs = client.read_registers(READ_HOLDING_REGISTERS, read.start, read.qty).unwrap();
                    read.store_regs(&regs, &mut image);
                }
            }
        }
        let mut driver = ModbusDriver::from_params(&sim.endpoint(), &BTreeMap::new()).unwrap();
        for (dir, _, name, addr) in POINTS.iter().filter(|p| p.0 == "in") {
            let _ = dir;
            let boxs = driver.read(&[addr]).unwrap()[0];
            let nodes = value_at(&image, point(&map, name));
            prop_assert!(same(boxs, nodes), "{name} {addr}: box {boxs:?}, node {nodes:?}");
        }

        // Writes: the image holds what a program set; the node's requests
        // put it in the device.
        let mut outputs = [0u8; IMAGE_BYTES];
        put(&mut outputs, point(&map, "c0"), Value::Bool(c0));
        put(&mut outputs, point(&map, "c2"), Value::Bool(c2));
        put(&mut outputs, point(&map, "ow"), Value::Int(i64::from(out_w)));
        put(&mut outputs, point(&map, "od"), Value::Int(i64::from(out_d)));
        put(&mut outputs, point(&map, "ou"), Value::Int(i64::from(out_u)));
        put(&mut outputs, point(&map, "of"), Value::Real(f64::from(out_f)));
        put(&mut outputs, point(&map, "ofsw"), Value::Real(f64::from(out_fsw)));
        for write in &plan.writes {
            match write.kind {
                WriteKind::Coils => {
                    let req = helmsman_modbus::pdu::write_coils(write.start, &write.coils(&outputs)).unwrap();
                    let reply = client.transact(&req).unwrap();
                    prop_assert_eq!(reply[0], 0x0F, "an exception, not an echo: {:?}", reply);
                }
                WriteKind::Holding => {
                    client.write_registers(write.start, &write.registers(&outputs)).unwrap();
                }
            }
        }
        let expect = [
            ("coil:0", Value::Bool(c0)),
            ("coil:1", Value::Bool(false)),
            ("coil:2", Value::Bool(c2)),
            ("coil:3", Value::Bool(true)),
            ("hr:30:u16", Value::Int(i64::from(out_w))),
            ("hr:31:i32sw", Value::Int(i64::from(out_d))),
            ("hr:33:u32", Value::Int(i64::from(out_u))),
            ("hr:35:f32", Value::Real(f64::from(out_f))),
            ("hr:37:f32sw", Value::Real(f64::from(out_fsw))),
        ];
        for (addr, want) in expect {
            let got = driver.read(&[addr]).unwrap()[0];
            prop_assert!(same(want, Some(match got {
                Value::Bool(b) => SignalValue::Bool(b),
                Value::Int(n) => SignalValue::Int(n),
                Value::Real(x) => SignalValue::Real(x),
            })), "{addr}: wanted {want:?}, the device holds {got:?}");
        }
        // Registers nobody named were not written: the gap between the points
        // keeps what the device had (holding 29 and 39 are outside every span).
        for at in [29usize, 39] {
            prop_assert_eq!(sim.state().holding[at], holding[at]);
        }
    }

    /// The node's assembly codec and the box's `enip-io` points agree on
    /// every format, both ways.
    #[test]
    fn a_node_reads_and_writes_an_assembly_as_the_boxs_enip_points_do(
        data in prop::collection::vec(any::<u8>(), 16),
        which in 0usize..7,
        offset in 0usize..12,
        out_n in any::<i32>(),
        out_f in any::<f32>().prop_filter("finite", |f| f.is_finite()),
    ) {
        let name = ["u8", "i8", "u16", "i16", "u32", "i32", "f32"][which];
        let fmt = [Fmt::U8, Fmt::I8, Fmt::U16, Fmt::I16, Fmt::U32, Fmt::I32, Fmt::F32][which];
        prop_assume!(offset + usize::from(fmt.width()) <= 16);
        let ty = if name == "f32" { "real" } else { "int" };
        let map = iomap::parse(&format!(
            "device e enip-io 10.0.0.5 config=1 o2t=2:16 t2o=3:16\nin i {ty} e in:{offset}:{name}\nout o {ty} e out:{offset}:{name}\n"
        )).unwrap();
        let Plan::Enip(plan) = map.plan(0) else { panic!("enip") };

        // Inputs.
        let mut image = [0u8; IMAGE_BYTES];
        plan.store_inputs(&data, &mut image);
        let boxs = EnipPoint::parse(&format!("in:{offset}:{name}")).unwrap().read_from(&data).unwrap();
        prop_assert!(same(boxs, value_at(&image, point(&map, "i"))), "{name}@{offset}: {boxs:?}");

        // Outputs: a value in the format's range.
        let value = match name {
            "f32" => Value::Real(f64::from(out_f)),
            "u8" => Value::Int(i64::from(out_n.unsigned_abs() % 256)),
            "i8" => Value::Int(i64::from(out_n % 128)),
            "u16" => Value::Int(i64::from(out_n.unsigned_abs() % 65536)),
            "i16" => Value::Int(i64::from(out_n % 32768)),
            "u32" => Value::Int(i64::from(out_n.unsigned_abs())),
            _ => Value::Int(i64::from(out_n)),
        };
        let mut outputs = [0u8; IMAGE_BYTES];
        put(&mut outputs, point(&map, "o"), value);
        // put() writes the low bytes of an i64 into the image slot; an 8-bit
        // value takes a 2-byte slot, sign-extended as the image holds it.
        if let (Fmt::I8, Value::Int(n)) = (fmt, value) {
            let at = point(&map, "o").at;
            outputs[usize::from(at.byte)..usize::from(at.byte) + 2].copy_from_slice(&i16::try_from(n).unwrap().to_le_bytes());
        }
        let mut node_bytes = [0u8; 16];
        plan.fill_outputs(&outputs, &mut node_bytes);
        let mut box_bytes = [0u8; 16];
        EnipPoint::parse(&format!("out:{offset}:{name}")).unwrap().write_into(&mut box_bytes, value).unwrap();
        prop_assert_eq!(node_bytes, box_bytes, "{} @{}: {:?}", name, offset, value);
    }
}

#[test]
fn the_nodes_bool_bits_of_an_assembly_are_the_boxs() {
    let map = iomap::parse(
        "device e enip-io 10.0.0.5 config=1 o2t=2:4 t2o=3:4\nin a bool e di:1\nin b bool e in:2.7\nout c bool e do:10\nout d bool e out:3.0\n",
    )
    .unwrap();
    let Plan::Enip(plan) = map.plan(0) else {
        panic!("enip")
    };
    let data = [0b0000_0010, 0, 0b1000_0000, 0];
    let mut image = [0u8; IMAGE_BYTES];
    plan.store_inputs(&data, &mut image);
    for (name, addr) in [("a", "di:1"), ("b", "in:2.7")] {
        let boxs = EnipPoint::parse(addr).unwrap().read_from(&data).unwrap();
        assert!(same(boxs, value_at(&image, point(&map, name))), "{addr}");
    }
    let mut outputs = [0u8; IMAGE_BYTES];
    put(&mut outputs, point(&map, "c"), Value::Bool(true));
    put(&mut outputs, point(&map, "d"), Value::Bool(true));
    let mut bytes = [0u8; 4];
    plan.fill_outputs(&outputs, &mut bytes);
    let mut want = [0u8; 4];
    for (addr, v) in [("do:10", true), ("out:3.0", true)] {
        EnipPoint::parse(addr)
            .unwrap()
            .write_into(&mut want, Value::Bool(v))
            .unwrap();
    }
    assert_eq!(bytes, want);
    let _ = Dir::In;
}

// ---- the Skipper's own Modbus TCP server, asked by the box's client codec ----

/// One request through the box's encoder, the node's server and the box's
/// reply checks: `Ok(reply PDU)` or the exception the box's codec reads.
fn ask_skipper(
    bank: &mut helmsman_nodecore::modbus_server::ShareBank,
    m: &IoMap,
    pdu: &[u8],
) -> Result<Vec<u8>, helmsman_modbus::Error> {
    use helmsman_modbus::{MBAP_LEN, mbap};
    let request = mbap::frame(0x1234, 1, pdu).unwrap();
    let reply = bank.handle(&m.shares, &request).expect("a frame");
    let head: [u8; MBAP_LEN] = reply[..MBAP_LEN].try_into().unwrap();
    let body = mbap::check_head(&head, 0x1234, 1).expect("the box accepts the header");
    assert_eq!(reply.len(), MBAP_LEN + body);
    let reply_pdu = reply[MBAP_LEN..].to_vec();
    helmsman_modbus::reply::body(pdu[0], &reply_pdu)?;
    Ok(reply_pdu)
}

#[test]
fn a_skipper_answers_the_boxs_modbus_client() {
    use helmsman_modbus::{Error, pdu, reply};
    use helmsman_nodecore::ioplan::put_slot;
    use helmsman_nodecore::modbus_server::ShareBank;

    let m = iomap::parse(
        "share speed int hr:0:i32\nshare run_cmd bool coil:0\nshare at_home bool di:0\nshare level real ir:4:f32sw\nshare target real hr:8:f32\n",
    )
    .unwrap();
    let mut bank = ShareBank {
        writable: (1 << 1) | (1 << 4),
        ..ShareBank::default()
    };
    for (i, v) in [
        (0, SignalValue::Int(-123_456)),
        (2, SignalValue::Bool(true)),
        (3, SignalValue::Real(-2.5)),
    ] {
        put_slot(&mut bank.image, m.shares[i].fmt, m.shares[i].at, v);
    }
    // The box reads a 32-bit register pair the way it reads any Modbus device.
    let r = ask_skipper(&mut bank, &m, &pdu::read(0x03, 0, 2).unwrap()).unwrap();
    let regs = reply::registers(0x03, 2, &r).unwrap();
    assert_eq!(
        ((u32::from(regs[0]) << 16) | u32::from(regs[1])).cast_signed(),
        -123_456
    );
    let r = ask_skipper(&mut bank, &m, &pdu::read(0x04, 4, 2).unwrap()).unwrap();
    let regs = reply::registers(0x04, 2, &r).unwrap();
    let bits = (u32::from(regs[1]) << 16) | u32::from(regs[0]); // f32sw: low word first
    assert!((f32::from_bits(bits) + 2.5).abs() < f32::EPSILON);
    let r = ask_skipper(&mut bank, &m, &pdu::read(0x02, 0, 1).unwrap()).unwrap();
    assert_eq!(reply::bits(0x02, 1, &r).unwrap(), [true]);
    // Writing a coil the program takes as input, and reading it back.
    let w = pdu::write_single(0x05, 0, helmsman_modbus::COIL_ON);
    let r = ask_skipper(&mut bank, &m, &w).unwrap();
    reply::echo(&w, &r).unwrap();
    let r = ask_skipper(&mut bank, &m, &pdu::read(0x01, 0, 1).unwrap()).unwrap();
    assert_eq!(reply::bits(0x01, 1, &r).unwrap(), [true]);
    // A register pair written the way the box writes one.
    let w = pdu::write_registers(8, &[0x3FC0, 0x0000]).unwrap();
    let r = ask_skipper(&mut bank, &m, &w).unwrap();
    reply::echo(&w, &r).unwrap();
    assert_eq!(
        helmsman_nodecore::ioplan::value_slot(&bank.image, m.shares[4].fmt, m.shares[4].at),
        Some(SignalValue::Real(1.5))
    );
    // What is not writable answers an exception the box's codec reads as one.
    let w = pdu::write_registers(0, &[0, 1]).unwrap();
    assert_eq!(ask_skipper(&mut bank, &m, &w), Err(Error::Exception(0x02)));
    // And a read the box makes past the shared range.
    assert_eq!(
        ask_skipper(&mut bank, &m, &pdu::read(0x03, 100, 1).unwrap()),
        Err(Error::Exception(0x02))
    );
}
