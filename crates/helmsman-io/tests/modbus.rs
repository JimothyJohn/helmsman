//! Modbus driver against the simulator and against hostile peers.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use helmsman_io::modbus::{self, Client, Format, ModbusDriver, Point, reply};
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Driver, Error, Ty, Value};
use proptest::prelude::*;

fn driver(s: &Sim) -> ModbusDriver {
    let params = BTreeMap::from([("timeout_ms".to_string(), "300".to_string())]);
    ModbusDriver::from_params(&s.endpoint(), &params).unwrap()
}

#[test]
fn addresses_parse_strictly() {
    for ok in [
        "coil:0",
        "di:65535",
        "hr:7",
        "ir:0:i16",
        "hr:65534:f32",
        "hr:1:u32sw",
    ] {
        Point::parse(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "",
        "coil",
        "coil:",
        "coil:-1",
        "coil:+1",
        "coil:65536",
        "coil:1:u16",
        "hr:1:f64",
        "hr:65535:f32",
        "hr:1:u16:x",
        "HR:1",
        "hr: 1",
        "x:1",
        "hr:0x10",
    ] {
        assert!(
            matches!(Point::parse(bad), Err(Error::Address(_))),
            "{bad:?} parsed"
        );
    }
    assert!(!Point::parse("ir:3").unwrap().info().writable);
    assert_eq!(Point::parse("hr:3:f32").unwrap().info().ty, Ty::Real);
}

#[test]
fn the_driver_reads_and_writes_every_table_and_format() {
    let mut st = State::new(32);
    st.discrete[3] = true;
    st.input[2] = 0xFFFF;
    let s = Sim::start(st).unwrap();
    let mut d = driver(&s);
    assert_eq!(
        d.read(&["di:3", "di:4", "ir:2", "ir:2:i16"]).unwrap(),
        [
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(65535),
            Value::Int(-1)
        ]
    );
    d.write("coil:9", Value::Bool(true)).unwrap();
    d.write("hr:0:i32", Value::Int(-2)).unwrap();
    d.write("hr:4:f32sw", Value::Real(-0.75)).unwrap();
    assert!(s.state().coils[9]);
    assert_eq!(s.state().holding[0..2], [0xFFFF, 0xFFFE], "high word first");
    let f = (-0.75f32).to_bits();
    assert_eq!(
        s.state().holding[4..6],
        [(f & 0xFFFF) as u16, (f >> 16) as u16],
        "sw: low word first"
    );
    assert_eq!(d.read(&["hr:4:f32sw"]).unwrap(), [Value::Real(-0.75)]);
}

#[test]
fn the_driver_refuses_bad_writes_without_touching_the_device() {
    let s = Sim::start(State::new(8)).unwrap();
    let mut d = driver(&s);
    assert!(matches!(
        d.write("di:0", Value::Bool(true)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(d.write("ir:0", Value::Int(1)), Err(Error::ReadOnly(_))));
    assert!(matches!(
        d.write("coil:0", Value::Int(1)),
        Err(Error::Type { .. })
    ));
    assert!(matches!(d.write("hr:0", Value::Int(70000)), Err(Error::Point(_))));
    assert!(matches!(
        d.write("hr:0:i16", Value::Int(-40000)),
        Err(Error::Point(_))
    ));
    assert!(matches!(
        d.write("hr:0", Value::Real(1.0)),
        Err(Error::Type { .. })
    ));
    assert_eq!(s.state().requests, 0);
}

#[test]
fn exceptions_come_back_as_exceptions() {
    let s = Sim::start(State::new(8)).unwrap();
    let mut d = driver(&s);
    assert!(matches!(
        d.read(&["hr:8"]),
        Err(Error::Exception(modbus::ILLEGAL_DATA_ADDRESS))
    ));
    assert!(matches!(
        d.read(&["hr:7:u32"]),
        Err(Error::Exception(modbus::ILLEGAL_DATA_ADDRESS))
    ));
    let mut c = Client::new(&s.endpoint(), 1, Duration::from_millis(300));
    assert!(matches!(c.transact(&[0x2B, 0x0E, 0x01, 0x00]), Ok(r) if r == [0xAB, modbus::ILLEGAL_FUNCTION]));
    assert!(
        matches!(c.read_registers(0x03, 0, 126), Err(Error::Protocol(_))),
        "client checks quantity first"
    );
}

#[test]
fn an_unreachable_device_errors_promptly() {
    // Bind then drop: a port that is closed.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut c = Client::new(&format!("127.0.0.1:{port}"), 1, Duration::from_millis(300));
    let t = Instant::now();
    assert!(matches!(c.read_bits(0x01, 0, 1), Err(Error::Io(_))));
    assert!(t.elapsed() < Duration::from_secs(2));
}

/// A server that answers every request with `answer(request)`.
fn hostile(answer: impl Fn(&[u8]) -> Vec<u8> + Send + 'static) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut conn) = conn else { return };
            let mut buf = [0u8; 300];
            while let Ok(n) = conn.read(&mut buf) {
                if n == 0 || conn.write_all(&answer(&buf[..n])).is_err() {
                    break;
                }
            }
        }
    });
    addr
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever bytes come back, the client returns an error or a valid
    /// value, within its timeout, and never panics.
    #[test]
    fn hostile_replies_never_panic_or_hang(junk in prop::collection::vec(any::<u8>(), 0..300), echo_header in any::<bool>()) {
        let addr = hostile(move |req| {
            let mut out = Vec::new();
            if echo_header && req.len() >= 7 {
                out.extend(&req[..4]);
                let n = u16::try_from(junk.len() + 1).unwrap_or(u16::MAX);
                out.extend(n.to_be_bytes());
                out.push(req[6]);
            }
            out.extend(&junk);
            out
        });
        let mut c = Client::new(&addr, 1, Duration::from_millis(100));
        let t = Instant::now();
        let _ = c.read_registers(0x03, 0, 4);
        let _ = c.read_bits(0x02, 0, 9);
        prop_assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn reply_decoders_never_panic(fc in 1u8..=6, qty in 1u16..=2000, pdu in prop::collection::vec(any::<u8>(), 0..260)) {
        let _ = reply::bits(fc, qty, &pdu);
        let _ = reply::registers(fc, qty.min(125), &pdu);
        let _ = reply::echo(&[fc, 0, 1, 0, 2], &pdu);
    }

    #[test]
    fn point_parse_never_panics(s in "\\PC{0,24}") {
        let _ = Point::parse(&s);
    }

    #[test]
    fn formats_round_trip(n in any::<i64>(), r in any::<f32>(), swapped in any::<bool>()) {
        for f in [Format::U16, Format::I16, Format::U32 { swapped }, Format::I32 { swapped }] {
            if let Ok(regs) = f.encode(Value::Int(n)) {
                prop_assert_eq!(f.decode(&regs), Value::Int(n));
            }
        }
        let f = Format::F32 { swapped };
        let back = f.decode(&f.encode(Value::Real(f64::from(r))).unwrap());
        match back {
            Value::Real(x) if r.is_nan() => prop_assert!(x.is_nan()),
            v => prop_assert_eq!(v, Value::Real(f64::from(r))),
        }
    }

    /// Random writes through the driver match a plain model of the device.
    #[test]
    fn driver_matches_a_model_of_the_device(ops in prop::collection::vec((0u8..4, 0u16..30, any::<i32>()), 1..40)) {
        let s = Sim::start(State::new(32)).unwrap();
        let mut d = driver(&s);
        let mut coils = vec![false; 32];
        let mut regs = vec![0u16; 32];
        for (op, addr, x) in ops {
            match op {
                0 => {
                    d.write(&format!("coil:{addr}"), Value::Bool(x % 2 == 0)).unwrap();
                    coils[usize::from(addr)] = x % 2 == 0;
                }
                1 => {
                    let v = u16::try_from(x.rem_euclid(65536)).unwrap();
                    d.write(&format!("hr:{addr}"), Value::Int(i64::from(v))).unwrap();
                    regs[usize::from(addr)] = v;
                }
                2 => {
                    d.write(&format!("hr:{addr}:i32"), Value::Int(i64::from(x))).unwrap();
                    let u = x.cast_unsigned();
                    regs[usize::from(addr)] = (u >> 16) as u16;
                    regs[usize::from(addr) + 1] = (u & 0xFFFF) as u16;
                }
                _ => {
                    let got = d.read(&[&format!("coil:{addr}"), &format!("hr:{addr}")]).unwrap();
                    prop_assert_eq!(got, vec![Value::Bool(coils[usize::from(addr)]), Value::Int(i64::from(regs[usize::from(addr)]))]);
                }
            }
        }
        prop_assert_eq!(&s.state().coils, &coils);
        prop_assert_eq!(&s.state().holding, &regs);
    }
}
