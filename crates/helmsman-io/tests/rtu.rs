//! Modbus RTU: framing, CRC, timing, the master against the RTU simulator
//! over an in-memory line, and the conformance harness on top.

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use helmsman_io::harness::{self, Bench, Outcome};
use helmsman_io::modbus::{self, ModbusDriver, Transport};
use helmsman_io::rtu::pipe::{self, End};
use helmsman_io::rtu::sim::RtuSim;
use helmsman_io::rtu::{
    self, BROADCAST, Line, MAX_ADU, Params, Parity, Receiver, RtuClient, Timing, crc16, decode, encode,
    reply_len,
};
use helmsman_io::sim::{Fault, Faults, State};
use helmsman_io::{Device, Driver, Error, IoMap, Value};
use proptest::prelude::*;

const BAUD: u32 = 115_200;

fn params(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
    kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
}

fn p(kv: &[(&str, &str)]) -> Params {
    let mut all = params(&[("baud", "115200"), ("timeout_ms", "200")]);
    all.extend(params(kv));
    Params::parse(&all).unwrap()
}

fn state() -> State {
    let mut s = State::new(64);
    s.wires = (0..8).map(|n| (n, n)).collect();
    s.input[0] = 4242;
    s.holding[20] = 0x3FC0; // 1.5f32, high word first
    s
}

fn rig(unit: u8) -> (RtuSim, ModbusDriver<RtuClient<End>>) {
    let (sim, line) = RtuSim::start(state(), unit, BAUD);
    let unit = unit.to_string();
    (sim, ModbusDriver::rtu(line, p(&[("unit", &unit)])))
}

fn read1(d: &mut dyn Driver, a: &str) -> Result<Value, Error> {
    d.read(&[a]).map(|v| v[0])
}

// ---- CRC and frames ----------------------------------------------------

#[test]
fn crc_matches_the_spec_example_and_a_known_frame() {
    // Serial §6.2.2 p.41: the register ends at 0001 0010 0100 0001 for the
    // frame 02 07, and "the CRC 16 of the frame is then: 4112": the bytes
    // in the order sent, low byte first.
    assert_eq!(crc16(&[0x02, 0x07]), 0x1241);
    assert_eq!(crc16(&[0x02, 0x07]).to_le_bytes(), [0x41, 0x12]);
    // Read one holding register at 0 from unit 1: CRC bytes 84 0A on the wire.
    assert_eq!(
        encode(1, &[0x03, 0x00, 0x00, 0x00, 0x01]).unwrap(),
        [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A]
    );
    assert_eq!(crc16(&[]), 0xFFFF, "the preset");
}

#[test]
fn every_single_bit_error_in_a_frame_is_caught() {
    let frame = encode(17, &[0x10, 0x00, 0x01, 0x00, 0x02, 0x04, 0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    for bit in 0..frame.len() * 8 {
        let mut f = frame.clone();
        f[bit / 8] ^= 1 << (bit % 8);
        assert!(decode(&f).is_err(), "bit {bit} flipped went unnoticed");
    }
}

#[test]
fn frames_out_of_size_are_refused() {
    assert!(encode(1, &[]).is_err());
    assert!(encode(1, &[0x03; 254]).is_err());
    assert_eq!(encode(1, &[0x03; 253]).unwrap().len(), MAX_ADU);
    for n in [0, 1, 2, 3, MAX_ADU + 1] {
        assert!(decode(&vec![0; n]).is_err(), "{n} bytes");
    }
}

#[test]
fn reply_lengths_follow_the_function() {
    assert_eq!(reply_len(&[1]), None);
    assert_eq!(reply_len(&[1, 0x03]), None, "needs the byte count");
    assert_eq!(reply_len(&[1, 0x03, 4]), Some(9));
    assert_eq!(reply_len(&[1, 0x83]), Some(5));
    assert_eq!(reply_len(&[1, 0x06]), Some(8));
    assert_eq!(reply_len(&[1, 0x10]), Some(8));
    assert_eq!(reply_len(&[1, 0x2B]), None, "a function this master never sends");
}

#[test]
fn timing_follows_the_baud_rate() {
    // 3.5 and 1.5 eleven-bit characters up to 19200 baud (Serial §2.5.1.1 p.13).
    let t = Timing::for_baud(9600);
    assert_eq!(t.t35, Duration::from_nanos(11_000_000_000 / 9600 * 7 / 2));
    assert_eq!(t.t15, Duration::from_nanos(11_000_000_000 / 9600 * 3 / 2));
    assert!(Timing::for_baud(19_200).t35 > Duration::from_micros(2000));
    // Fixed above 19200.
    for baud in [19_201, 38_400, 115_200, 4_000_000] {
        let t = Timing::for_baud(baud);
        assert_eq!(t.t15, Duration::from_micros(750), "{baud}");
        assert_eq!(t.t35, Duration::from_micros(1750), "{baud}");
    }
}

#[test]
fn params_parse_strictly() {
    let d = Params::parse(&params(&[])).unwrap();
    assert_eq!(
        (d.unit, d.settings.baud, d.settings.parity, d.settings.stop_bits),
        (1, 19_200, Parity::Even, 1)
    );
    assert_eq!(d.timeout, Duration::from_secs(1));
    assert!(!d.strict);
    let n = Params::parse(&params(&[("parity", "none")])).unwrap();
    assert_eq!(
        n.settings.stop_bits, 2,
        "no parity takes two stop bits (Serial §2.5.1 p.12)"
    );
    let o = Params::parse(&params(&[
        ("parity", "odd"),
        ("stop", "2"),
        ("unit", "247"),
        ("timing", "strict"),
    ]))
    .unwrap();
    assert_eq!(
        (o.settings.parity, o.settings.stop_bits, o.unit, o.strict),
        (Parity::Odd, 2, 247, true)
    );
    assert_eq!(Params::parse(&params(&[("unit", "0")])).unwrap().unit, BROADCAST);
    assert_eq!(
        Params::parse(&params(&[("poll_ms", "50")])).unwrap(),
        d,
        "the link's param"
    );
    for (k, v) in [
        ("unit", "248"),
        ("unit", "255"),
        ("unit", "-1"),
        ("unit", "+1"),
        ("unit", ""),
        ("unit", "0x01"),
        ("baud", "0"),
        ("baud", "99999999999999"),
        ("baud", "9600.0"),
        ("parity", "EVEN"),
        ("parity", "mark"),
        ("stop", "0"),
        ("stop", "1.5"),
        ("timeout_ms", "0"),
        ("timeout_ms", "60001"),
        ("turnaround_ms", "10001"),
        ("timing", "loose"),
    ] {
        assert!(
            matches!(Params::parse(&params(&[(k, v)])), Err(Error::Address(_))),
            "{k}={v} parsed"
        );
    }
}

#[test]
fn rtu_points_are_modbus_points() {
    let map = IoMap {
        devices: vec![Device {
            id: "vfd".into(),
            driver: rtu::DRIVER.into(),
            endpoint: "/dev/ttyUSB0".into(),
            params: params(&[("unit", "3")]),
        }],
        signals: vec![],
    };
    assert!(map.validate().is_ok());
    assert!(helmsman_io::driver::parse_point(rtu::DRIVER, "hr:10:f32").is_ok());
    assert!(matches!(
        helmsman_io::driver::parse_point(rtu::DRIVER, "hr:x"),
        Err(Error::Address(_))
    ));
}

#[test]
fn open_parses_params_and_opens_the_port_lazily() {
    let dev = |kv: &[(&str, &str)]| Device {
        id: "d".into(),
        driver: rtu::DRIVER.into(),
        endpoint: "/nonexistent/helmsman-tty".into(),
        params: params(kv),
    };
    assert!(matches!(
        helmsman_io::open(&dev(&[("unit", "300")])),
        Err(Error::Address(_))
    ));
    let opened = helmsman_io::open(&dev(&[("timeout_ms", "50")]));
    if cfg!(all(unix, feature = "serial")) {
        let mut d = opened.expect("opens lazily");
        let e = read1(d.as_mut(), "hr:0").unwrap_err();
        assert!(e.is_link(), "a missing port is a link error: {e}");
    } else {
        assert!(
            matches!(opened, Err(Error::Io(_))),
            "no serial support in this build"
        );
    }
}

// ---- The receiver, without a clock --------------------------------------

fn reply_frame() -> Vec<u8> {
    encode(5, &[0x03, 0x04, 0x12, 0x34, 0xAB, 0xCD]).unwrap()
}

fn rx() -> Receiver {
    Receiver::new(5, 0x03, None)
}

#[test]
fn a_reply_split_at_every_byte_boundary_reassembles() {
    let f = reply_frame();
    let want = f[1..f.len() - 2].to_vec();
    let t0 = Instant::now();
    for i in 0..=f.len() {
        for j in i..=f.len() {
            let mut r = rx();
            let mut got = None;
            for (k, part) in [&f[..i], &f[i..j], &f[j..]].into_iter().enumerate() {
                let out = r.push(part, t0 + Duration::from_millis(k as u64));
                if out.is_some() {
                    assert!(got.is_none());
                    got = out;
                }
            }
            assert_eq!(got.as_deref(), Some(&want[..]), "split at {i}, {j}");
        }
    }
    // One byte at a time.
    let mut r = rx();
    let got: Vec<_> = f.iter().filter_map(|b| r.push(&[*b], t0)).collect();
    assert_eq!(got, [want]);
}

#[test]
fn garbage_other_units_and_bad_frames_before_the_reply_are_skipped() {
    let f = reply_frame();
    let other = encode(6, &[0x03, 0x04, 0x12, 0x34, 0xAB, 0xCD]).unwrap();
    let mut bad = f.clone();
    bad[4] ^= 0x40;
    let t0 = Instant::now();
    for junk in [
        vec![0xFF, 0x00, 0x05],
        vec![0x05, 0x03],
        vec![0x05, 0x83, 0x02],
        other.clone(),
        bad.clone(),
        [other, bad, vec![5, 5, 3]].concat(),
    ] {
        let mut r = rx();
        let stream = [junk.clone(), f.clone()].concat();
        assert_eq!(
            r.push(&stream, t0).as_deref(),
            Some(&f[1..f.len() - 2]),
            "after {junk:02x?}"
        );
    }
}

#[test]
fn only_wrong_units_mean_a_timeout_and_bad_frames_a_protocol_error() {
    let t0 = Instant::now();
    let mut r = rx();
    assert!(r.push(&encode(6, &[0x03, 0x02, 0, 1]).unwrap(), t0).is_none());
    assert!(matches!(r.give_up(Duration::from_millis(1)), Error::Io(_)));

    let mut r = rx();
    let mut bad = reply_frame();
    bad[3] ^= 1;
    assert!(r.push(&bad, t0).is_none());
    let e = r.give_up(Duration::from_millis(1));
    assert!(matches!(e, Error::Protocol(_)) && e.is_link(), "{e}");

    let mut r = rx();
    let f = reply_frame();
    assert!(r.push(&f[..f.len() - 2], t0).is_none());
    let e = r.give_up(Duration::from_millis(1));
    assert!(e.to_string().contains("incomplete"), "{e}");
}

#[test]
fn an_exception_reply_is_a_frame() {
    let f = encode(5, &[0x83, 0x02]).unwrap();
    assert_eq!(rx().push(&f, Instant::now()).as_deref(), Some(&[0x83, 0x02][..]));
}

#[test]
fn strict_timing_discards_a_frame_with_a_gap_over_t15() {
    let t15 = Timing::for_baud(9600).t15;
    let f = reply_frame();
    let t0 = Instant::now();
    let mut r = Receiver::new(5, 0x03, Some(t15));
    assert!(r.push(&f[..4], t0).is_none());
    assert!(
        r.push(&f[4..], t0 + t15 + Duration::from_micros(1)).is_none(),
        "the gap broke the frame"
    );
    // Within t1.5 it is one frame.
    let mut r = Receiver::new(5, 0x03, Some(t15));
    assert!(r.push(&f[..4], t0).is_none());
    assert!(r.push(&f[4..], t0 + t15).is_some());
    // And a whole frame after the gap still counts.
    let mut r = Receiver::new(5, 0x03, Some(t15));
    assert!(r.push(&[0x05, 0x03], t0).is_none());
    assert!(r.push(&f, t0 + Duration::from_millis(50)).is_some());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn encode_decode_round_trips(unit in any::<u8>(), pdu in prop::collection::vec(any::<u8>(), 1..=253)) {
        let f = encode(unit, &pdu).unwrap();
        prop_assert!(f.len() <= MAX_ADU);
        let (u, back) = decode(&f).unwrap();
        prop_assert_eq!(u, unit);
        prop_assert_eq!(back, &pdu[..]);
    }

    #[test]
    fn a_flipped_bit_never_decodes(unit in any::<u8>(), pdu in prop::collection::vec(any::<u8>(), 1..=253), bit in any::<prop::sample::Index>()) {
        let mut f = encode(unit, &pdu).unwrap();
        let b = bit.index(f.len() * 8);
        f[b / 8] ^= 1 << (b % 8);
        prop_assert!(decode(&f).is_err());
    }

    #[test]
    fn decode_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = decode(&bytes);
        let _ = reply_len(&bytes);
    }

    /// Arbitrary bytes, in arbitrary chunks and gaps, never panic the
    /// receiver, and anything it returns is a CRC-checked frame's PDU.
    #[test]
    fn the_receiver_survives_anything(
        chunks in prop::collection::vec((prop::collection::vec(any::<u8>(), 0..64), 0u64..3000), 0..20),
        strict in any::<bool>(),
        fc in prop::sample::select(vec![0x01u8, 0x03, 0x05, 0x06, 0x10]),
    ) {
        let mut r = Receiver::new(1, fc, strict.then_some(Duration::from_micros(750)));
        let mut at = Instant::now();
        for (c, gap) in chunks {
            at += Duration::from_micros(gap);
            if let Some(pdu) = r.push(&c, at) {
                prop_assert!(pdu[0] == fc || pdu[0] == fc | 0x80);
            }
        }
        let _ = r.give_up(Duration::from_millis(1));
    }

    /// A good reply anywhere in a stream of noise is found.
    #[test]
    fn a_reply_after_noise_is_found(noise in prop::collection::vec(any::<u8>(), 0..100)) {
        let f = reply_frame();
        let mut r = rx();
        let got = r.push(&[noise, f.clone()].concat(), Instant::now());
        // Noise can, with odds of 2^-16, hold a valid frame of its own;
        // what comes out must still be a reply to this request.
        let pdu = got.expect("the reply is in the stream");
        prop_assert_eq!(pdu[0], 0x03);
    }
}

// ---- The master against the simulator -----------------------------------

#[test]
fn the_driver_reads_and_writes_every_table() {
    let (sim, mut d) = rig(7);
    sim.state().discrete[3] = true;
    sim.state().input[2] = 0xFFFF;
    assert_eq!(read1(&mut d, "di:3").unwrap(), Value::Bool(true));
    assert_eq!(read1(&mut d, "ir:2:i16").unwrap(), Value::Int(-1));
    assert_eq!(read1(&mut d, "hr:20:f32").unwrap(), Value::Real(1.5));
    d.write("coil:2", Value::Bool(true)).unwrap();
    assert_eq!(
        read1(&mut d, "di:2").unwrap(),
        Value::Bool(true),
        "through the cross-wire"
    );
    d.write("hr:5:i32", Value::Int(-123_456)).unwrap();
    assert_eq!(read1(&mut d, "hr:5:i32").unwrap(), Value::Int(-123_456));
    d.write("hr:9", Value::Int(7)).unwrap();
    assert_eq!(sim.state().holding[9], 7);
    assert_eq!(
        d.read(&["coil:2", "hr:9", "ir:0"]).unwrap(),
        [Value::Bool(true), Value::Int(7), Value::Int(4242)]
    );
}

#[test]
fn exceptions_are_point_errors_and_the_link_stays_up() {
    let (_sim, mut d) = rig(1);
    let e = read1(&mut d, "hr:9999").unwrap_err();
    assert!(matches!(e, Error::Exception(modbus::ILLEGAL_DATA_ADDRESS)), "{e}");
    assert!(!e.is_link());
    assert_eq!(read1(&mut d, "ir:0").unwrap(), Value::Int(4242));
}

fn fault_then_recover(fault: Fault) -> Error {
    let (sim, mut d) = rig(9);
    sim.inject(fault);
    let started = Instant::now();
    let e = read1(&mut d, "ir:0").unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{fault:?} took {:?}",
        started.elapsed()
    );
    assert_eq!(
        read1(&mut d, "ir:0").unwrap(),
        Value::Int(4242),
        "usable after {fault:?}"
    );
    e
}

#[test]
fn line_faults_are_link_errors_and_the_master_recovers() {
    for fault in [
        Fault::Drop,
        Fault::Garbage,
        Fault::BadCrc,
        Fault::Truncate,
        Fault::WrongUnit,
    ] {
        let e = fault_then_recover(fault);
        assert!(e.is_link(), "{fault:?}: {e}");
    }
    assert!(
        matches!(fault_then_recover(Fault::Drop), Error::Io(_)),
        "silence is a time-out"
    );
    assert!(matches!(fault_then_recover(Fault::BadCrc), Error::Protocol(_)));
    assert!(matches!(fault_then_recover(Fault::Truncate), Error::Protocol(_)));
    assert!(
        matches!(fault_then_recover(Fault::WrongUnit), Error::Io(_)),
        "another unit's reply is ignored"
    );
    let e = fault_then_recover(Fault::DeviceFailure);
    assert!(
        matches!(e, Error::Exception(modbus::SERVER_DEVICE_FAILURE)) && !e.is_link(),
        "{e}"
    );
}

#[test]
fn noise_before_a_reply_and_a_reply_in_pieces_still_read() {
    let (sim, mut d) = rig(4);
    sim.inject(Fault::Noise);
    sim.inject(Fault::Split);
    assert_eq!(read1(&mut d, "ir:0").unwrap(), Value::Int(4242));
    assert_eq!(read1(&mut d, "ir:0").unwrap(), Value::Int(4242));
}

#[test]
fn a_late_reply_is_never_taken_for_the_next_answer() {
    // The reply to hr:0 comes 100 ms after the 200 ms time-out; the next
    // read, of hr:1 (same function, same length), must get hr:1.
    let (sim, mut d) = rig(2);
    sim.state().holding[0] = 111;
    sim.state().holding[1] = 222;
    sim.inject(Fault::Delay(Duration::from_millis(300)));
    assert!(read1(&mut d, "hr:0").unwrap_err().is_link());
    assert_eq!(read1(&mut d, "hr:1").unwrap(), Value::Int(222));
    assert_eq!(read1(&mut d, "hr:0").unwrap(), Value::Int(111));
}

#[test]
fn a_frame_for_another_unit_gets_no_answer() {
    let (sim, line) = RtuSim::start(state(), 3, BAUD);
    let mut d = ModbusDriver::rtu(line, p(&[("unit", "4"), ("timeout_ms", "50")]));
    let e = read1(&mut d, "ir:0").unwrap_err();
    assert!(matches!(e, Error::Io(_)), "{e}");
    assert_eq!(sim.state().requests, 0, "unit 3 ignored a request for unit 4");
}

#[test]
fn a_broadcast_write_reaches_the_unit_and_is_not_waited_on() {
    let (sim, line) = RtuSim::start(state(), 12, BAUD);
    // A long time-out: a broadcast that waited for a reply would show.
    let mut d = ModbusDriver::rtu(
        line,
        p(&[("unit", "0"), ("timeout_ms", "5000"), ("turnaround_ms", "50")]),
    );
    let started = Instant::now();
    d.write("hr:3", Value::Int(77)).unwrap();
    d.write("coil:1", Value::Bool(true)).unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(1), "took {took:?}");
    assert!(
        took >= Duration::from_millis(50),
        "the turnaround delay sits between broadcasts: {took:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while !sim.state().coils[1] && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(sim.state().holding[3], 77);
    assert!(sim.state().coils[1]);
    assert!(sim.state().discrete[1], "cross-wired");
}

#[test]
fn a_broadcast_read_is_refused_before_anything_is_sent() {
    let (a, mut b) = pipe::pair();
    let mut c = RtuClient::new(a, p(&[("unit", "0")]));
    let e = c.request(&[0x03, 0, 0, 0, 1]).unwrap_err();
    assert!(matches!(e, Error::Point(_)) && !e.is_link(), "{e}");
    let mut buf = [0u8; 8];
    assert_eq!(
        b.recv(&mut buf, Duration::from_millis(20)).unwrap(),
        0,
        "nothing on the line"
    );
}

#[test]
fn a_dead_line_is_a_link_error() {
    let (a, b) = pipe::pair();
    drop(b);
    let mut d = ModbusDriver::rtu(a, p(&[]));
    assert!(read1(&mut d, "ir:0").unwrap_err().is_link());
    assert!(d.write("coil:0", Value::Bool(true)).unwrap_err().is_link());
}

#[test]
fn the_conformance_harness_passes_against_the_rtu_simulator() {
    let (sim, mut d) = rig(1);
    let bench = Bench {
        points: vec!["di:0".into(), "ir:0".into(), "coil:3".into(), "hr:4:i32".into()],
        expect: vec![
            ("ir:0".into(), Value::Int(4242)),
            ("hr:20:f32".into(), Value::Real(1.5)),
        ],
        loops: vec![("coil:0".into(), "di:0".into()), ("coil:5".into(), "di:5".into())],
        registers: vec![
            "hr:0".into(),
            "hr:1:i16".into(),
            "hr:2:u32sw".into(),
            "hr:10:f32".into(),
        ],
        absent: Some("hr:9999".into()),
        settle: Duration::from_millis(200),
    };
    let checks = harness::run(&mut d, &bench, Some(&sim));
    assert!(
        checks.iter().all(|c| c.outcome == Outcome::Pass),
        "{}",
        harness::render(&checks)
    );
    for name in [
        "fault bad crc",
        "fault truncated frame",
        "fault wrong unit",
        "fault late reply",
    ] {
        assert!(checks.iter().any(|c| c.name == name), "{name} ran");
    }
}

/// A unit that answers every request with whatever bytes it is given.
fn hostile(junk: Vec<u8>) -> End {
    let (master, mut slave) = pipe::pair();
    thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok(n) = slave.recv(&mut buf, Duration::from_secs(5)) {
            if n > 0 && slave.send(&junk).is_err() {
                return;
            }
        }
    });
    master
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever comes back, the master returns a value or an error within
    /// its time-out (plus the drain), and never panics.
    #[test]
    fn hostile_replies_never_panic_or_hang(junk in prop::collection::vec(any::<u8>(), 0..300), echo in any::<bool>()) {
        let junk = if echo { [encode(1, &[0x03, 0x02]).unwrap(), junk].concat() } else { junk };
        let mut d = ModbusDriver::rtu(hostile(junk), p(&[("timeout_ms", "30")]));
        for a in ["hr:0", "hr:0:f32", "coil:1"] {
            let started = Instant::now();
            let _ = read1(&mut d, a);
            prop_assert!(started.elapsed() < Duration::from_millis(500));
        }
        let _ = d.write("hr:1", Value::Int(5));
    }
}
