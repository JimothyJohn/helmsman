//! The `iolink` lines of a node's I/O map: what parses and what is
//! refused, and the whole way from a Device's IODD to values by name,
//! on a port run against the simulated Device.

use helmsman_iolink::device::Device;
use helmsman_iolink::iodd;
use helmsman_iolink::master::{Fault, Identity, State, Step};
use helmsman_iolink::page::Page1;
use helmsman_iolink::timing::Rate;
use helmsman_nodecore::iolink::{Port, PortMap, Problem, Ty, Value, parse};
use helmsman_nodecore::iomap;
use proptest::prelude::*;

const MAP: &str = "\
# the press: a Modbus block and a distance sensor
device rio modbus-tcp 192.168.3.60 unit=1
in  estop bool rio di:0
device prox iolink port=1 vendor=310 device=666 in_bits=32 out_bits=8 cycle_ms=10
in  prox_distance int  prox pd:16:i16   # mm
in  prox_quality  int  prox pd:8:u8
in  prox_near     bool prox pd:0
out prox_teach    bool prox pd:0
out prox_gain     int  prox pd:4:u4
";

#[test]
fn the_iolink_lines_of_a_map_parse_to_ports_and_points() {
    let maps = parse(MAP, 2).unwrap();
    assert_eq!(maps.len(), 1);
    let map = &maps[0];
    assert_eq!((map.device.as_str(), map.port, map.cycle_us), ("prox", 1, 10_000));
    assert_eq!(
        map.expect,
        Some(Identity {
            vendor: 310,
            device: 666
        })
    );
    assert_eq!((map.in_bits, map.out_bits), (Some(32), Some(8)));
    let points = |list: &[helmsman_nodecore::iolink::Point]| -> Vec<(String, Ty, u16, u8, bool)> {
        list.iter()
            .map(|p| (p.name.clone(), p.ty, p.bit_offset, p.bit_length, p.signed))
            .collect()
    };
    assert_eq!(
        points(&map.inputs),
        [
            ("prox_distance".to_string(), Ty::Int, 16, 16, true),
            ("prox_quality".to_string(), Ty::Int, 8, 8, false),
            ("prox_near".to_string(), Ty::Bool, 0, 1, false),
        ]
    );
    assert_eq!(
        points(&map.outputs),
        [
            ("prox_teach".to_string(), Ty::Bool, 0, 1, false),
            ("prox_gain".to_string(), Ty::Int, 4, 4, false),
        ]
    );
    // A map with no IO-Link device has no ports, on any node.
    assert_eq!(
        parse("device rio modbus-tcp 192.168.3.60\nin a bool rio di:0\n", 0),
        Ok(vec![])
    );
}

/// No node has a Master port yet: the node's own parser still refuses
/// the line, and this one does on a node with no ports.
#[test]
fn a_node_without_a_port_takes_no_iolink_line() {
    let e = iomap::parse(MAP).unwrap_err();
    assert_eq!(e.line, 4, "{e}");
    let e = parse(MAP, 0).unwrap_err();
    assert_eq!((e.line, e.why.as_str()), (4, "this node has no IO-Link port"));
}

#[test]
fn what_a_port_cannot_run_is_refused_with_its_line() {
    let cases: &[(&str, &str, usize, &str)] = &[
        ("port=1", "port=3", 4, "port=3: 1 to 2"),
        ("port=1", "port=0", 4, "port=0: 1 to 2"),
        ("port=1 ", "", 4, "which port?"),
        ("vendor=310 ", "", 4, "vendor= and device= go together"),
        ("vendor=310", "vendor=0", 4, "vendor=0: 1 to 65535"),
        ("device=666", "device=16777216", 4, "1 to 16777215"),
        ("in_bits=32", "in_bits=257", 4, "in_bits=257: 1 to 256"),
        ("cycle_ms=10", "cycle_ms=500", 4, "cycle_ms=500: 1 to 132"),
        ("cycle_ms=10", "rate=COM3", 4, "an iolink device takes port"),
        ("cycle_ms=10", "fast", 4, "expected KEY=VALUE"),
        ("device prox iolink", "device 9prox iolink", 4, "device name"),
        ("pd:16:i16", "pd:17:i16", 5, "ends at bit 33 of 32 bits"),
        ("pd:16:i16", "pd:16:i65", 5, "u2 to u63 or i2 to i64"),
        ("pd:8:u8", "pd:8:u64", 6, "u2 to u63 or i2 to i64"),
        ("pd:8:u8", "pd:8:u1", 6, "u2 to u63 or i2 to i64"),
        ("pd:8:u8", "pd:8", 6, "an IO-Link int is pd:OFFSET:uN"),
        ("pd:8:u8", "pd:8:f32", 6, "an IO-Link int is"),
        ("pd:8:u8", "hr:8:u16", 6, "an IO-Link int is"),
        ("pd:8:u8", "pd:x:u8", 6, "an IO-Link int is"),
        ("pd:8:u8", "pd:8:u8:be", 6, "an IO-Link int is"),
        (
            "in  prox_near     bool prox pd:0",
            "in  prox_near     bool prox pd:0:u1",
            7,
            "an IO-Link bool is pd:OFFSET",
        ),
        (
            "in  prox_near     bool prox pd:0",
            "in  prox_near     bool prox pd:300",
            7,
            "Process Data ends at bit 255",
        ),
        (
            "in  prox_near     bool",
            "in  prox_near     real",
            7,
            "an IO-Link point is bool or int",
        ),
        ("in  prox_near ", "in  prox-near ", 7, "a name is letters"),
        ("in  prox_near ", "in  PROX_Quality ", 7, "named twice"),
        ("pd:4:u4", "pd:0:u4", 9, "shares bits with prox_teach"),
        ("pd:4:u4", "pd:5:u4", 9, "ends at bit 9 of 8 bits"),
        (
            "out prox_gain     int  prox pd:4:u4",
            "device prox2 iolink port=1",
            9,
            "port 1 already has prox",
        ),
        (
            "out prox_gain     int  prox pd:4:u4",
            "device PROX iolink port=2",
            9,
            "a second device of that name",
        ),
    ];
    for (from, to, line, why) in cases {
        assert!(MAP.contains(from), "the test's own map lacks {from:?}");
        let e = parse(&MAP.replacen(from, to, 1), 2).unwrap_err();
        assert!(
            e.why.contains(why),
            "{from:?} -> {to:?}: expected {why:?}, got {e}"
        );
        assert_eq!(e.line, *line, "{from:?} -> {to:?}: {e}");
    }
    let long = format!("{MAP}{}", "#".repeat(2048));
    assert_eq!(parse(&long, 2).unwrap_err().line, 0);
    let points: Vec<String> = (0..33).map(|i| format!("in p{i} bool d pd:{i}")).collect();
    let many = format!("device d iolink port=1\n{}\n", points.join("\n"));
    assert!(parse(&many, 1).unwrap_err().why.contains("at most 32 points"));
}

/// The IODD of the sensor the map above is for, written for this test.
const IODD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<IODevice xmlns="http://www.io-link.com/IODD/2010/10" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <ProfileBody>
    <DeviceIdentity vendorId="310" vendorName="Bench" deviceId="666"><DeviceName textId="T_Dev"/></DeviceIdentity>
    <DeviceFunction>
      <ProcessDataCollection><ProcessData id="P">
        <ProcessDataIn id="PI" bitLength="32">
          <Datatype xsi:type="RecordT" bitLength="32">
            <RecordItem subindex="1" bitOffset="16"><SimpleDatatype xsi:type="IntegerT" bitLength="16"/><Name textId="T_Dist"/></RecordItem>
            <RecordItem subindex="2" bitOffset="8"><SimpleDatatype xsi:type="UIntegerT" bitLength="8"/><Name textId="T_Qual"/></RecordItem>
            <RecordItem subindex="3" bitOffset="0"><SimpleDatatype xsi:type="BooleanT"/><Name textId="T_Near"/></RecordItem>
            <RecordItem subindex="4" bitOffset="1"><SimpleDatatype xsi:type="BooleanT"/><Name textId="T_Near"/></RecordItem>
          </Datatype>
          <Name textId="T_In"/>
        </ProcessDataIn>
        <ProcessDataOut id="PO" bitLength="8">
          <Datatype xsi:type="RecordT" bitLength="8">
            <RecordItem subindex="1" bitOffset="0"><SimpleDatatype xsi:type="BooleanT"/><Name textId="T_Teach"/></RecordItem>
            <RecordItem subindex="2" bitOffset="4"><SimpleDatatype xsi:type="UIntegerT" bitLength="4"/><Name textId="T_Gain"/></RecordItem>
          </Datatype>
          <Name textId="T_Out"/>
        </ProcessDataOut>
      </ProcessData></ProcessDataCollection>
    </DeviceFunction>
  </ProfileBody>
  <ExternalTextCollection><PrimaryLanguage xml:lang="en">
    <Text id="T_Dev" value="Distance sensor # 4"/>
    <Text id="T_Dist" value="Distance"/>
    <Text id="T_Qual" value="Signal quality (%)"/>
    <Text id="T_Near" value="Near"/>
    <Text id="T_Teach" value="Teach"/>
    <Text id="T_Gain" value="Gain"/>
  </PrimaryLanguage></ExternalTextCollection>
</IODevice>"#;

#[test]
fn an_iodd_becomes_map_lines_a_node_takes() {
    let lines = iodd::parse(IODD).unwrap().map_lines(0, "prox", 1).unwrap();
    assert_eq!(
        lines,
        "\
# Distance sensor  4 (Bench), from its IODD
device prox iolink port=1 vendor=310 device=666 in_bits=32 out_bits=8
in  prox_distance int  prox pd:16:i16  # Distance
in  prox_signal_quality int  prox pd:8:u8  # Signal quality (%)
in  prox_near bool prox pd:0  # Near
in  prox_near_2 bool prox pd:1  # Near
out prox_teach bool prox pd:0  # Teach
out prox_gain int  prox pd:4:u4  # Gain
"
    );
    let maps = parse(&lines, 1).unwrap();
    assert_eq!((maps[0].inputs.len(), maps[0].outputs.len()), (4, 2));
    assert_eq!(iodd::parse(IODD).unwrap().map_lines(1, "prox", 1), None);
}

/// 4 octets in, 1 out: PREOPERATE with `TYPE_1_2`, OPERATE code 4.
fn sensor_page() -> Page1 {
    Page1([
        0, 0, 0x17, 0x19, 0x11, 0x83, 0x08, 0x01, 0x36, 0x00, 0x02, 0x9A, 0, 0, 0, 0,
    ])
}

/// Runs the port against the Device for `micros` of simulated time.
fn run(port: &mut Port, device: &mut Device, now: &mut u64, micros: u64) {
    drive(port, Some(device), now, micros);
}

/// The same, with nothing plugged in when `device` is `None`.
fn drive(port: &mut Port, mut device: Option<&mut Device>, now: &mut u64, micros: u64) {
    let end = *now + micros;
    while *now < end {
        let mut sent = [0; 66];
        let (rate, len, want, timeout_us) = match port.poll(*now) {
            Step::WakeUp => {
                if let Some(device) = device.as_deref_mut() {
                    device.wake_up(*now);
                }
                *now += 80;
                continue;
            }
            Step::Idle { until } => {
                *now = until;
                continue;
            }
            Step::Exchange {
                rate,
                send,
                reply,
                timeout_us,
            } => {
                sent[..send.len()].copy_from_slice(send);
                (rate, send.len(), reply, timeout_us)
            }
        };
        let mut answer = [0; 80];
        let got = device
            .as_deref_mut()
            .and_then(|device| device.exchange(*now, rate, &sent[..len], &mut answer));
        match got {
            Some(n) if n == want => {
                *now += rate.micros((len + n) * 11 + 5);
                port.reply(*now, &answer[..n]);
            }
            _ => {
                *now += timeout_us;
                port.reply(*now, &[]);
            }
        }
    }
}

fn port_from_iodd() -> Port {
    let lines = iodd::parse(IODD).unwrap().map_lines(0, "prox", 1).unwrap();
    Port::new(parse(&lines, 1).unwrap().remove(0))
}

#[test]
fn values_travel_by_name_from_the_iodd_to_the_device_and_back() {
    let mut port = port_from_iodd();
    let mut device = Device::new(Rate::Com2, sensor_page()).unwrap();
    let mut now = 0;
    // -200 mm, quality 90, the second switch on.
    device.set_inputs(&[0xFF, 0x38, 90, 0x02]);
    assert_eq!(port.input(0), None);
    assert!(
        !port.set_outputs(Some(&[Value::Bool(true), Value::Int(9)])),
        "before a Device is accepted"
    );
    run(&mut port, &mut device, &mut now, 1_000_000);
    assert!(port.running(), "{:?} {:?}", port.master().state(), port.problem());
    let inputs: Vec<_> = (0..5).map(|i| port.input(i)).collect();
    assert_eq!(
        inputs,
        [
            Some(Value::Int(-200)),
            Some(Value::Int(90)),
            Some(Value::Bool(false)),
            Some(Value::Bool(true)),
            None
        ]
    );
    assert!(port.set_outputs(Some(&[Value::Bool(true), Value::Int(9)])));
    run(&mut port, &mut device, &mut now, 50_000);
    assert_eq!(device.outputs(), Some(&[0x91][..]));
    // A number that does not fit its 4 bits, a wrong type, a missing
    // value: nothing of it is sent and the Device is told so.
    for wrong in [
        &[Value::Bool(true), Value::Int(16)][..],
        &[Value::Bool(true), Value::Int(-1)],
        &[Value::Int(1), Value::Int(1)],
        &[Value::Bool(true)],
    ] {
        assert!(!port.set_outputs(Some(wrong)), "{wrong:?}");
        run(&mut port, &mut device, &mut now, 50_000);
        assert_eq!(device.outputs(), None, "{wrong:?}");
    }
    assert!(port.set_outputs(Some(&[Value::Bool(false), Value::Int(15)])));
    run(&mut port, &mut device, &mut now, 50_000);
    assert_eq!(device.outputs(), Some(&[0xF0][..]));
    // The program stops.
    assert!(port.set_outputs(None));
    run(&mut port, &mut device, &mut now, 50_000);
    assert_eq!(device.outputs(), None);
    // The sensor is unplugged: no values, and no problem to report but
    // its absence.
    drive(&mut port, None, &mut now, 100_000);
    assert_eq!(port.problem(), None);
    assert_eq!(port.input(0), None);
    assert!(!port.running());
    assert_eq!(port.master().state(), State::Searching);
}

/// A Device with other Process Data lengths than the map was written
/// for would put every point on other bits: nothing is read or written.
#[test]
fn a_device_of_another_length_gives_no_values() {
    for (pd_in, pd_out, problem) in [
        (0x10, 0x08, Problem::InBits { map: 32, device: 16 }),
        (0x83, 0x10, Problem::OutBits { map: 8, device: 16 }),
    ] {
        let mut page = sensor_page();
        // OPERATE code 0 where both lengths are 16 bits or fewer (IOL
        // Table A.10), code 4 from 3 octets.
        page.0[3] = if pd_in == 0x83 { 0x19 } else { 0x11 };
        page.0[5] = pd_in;
        page.0[6] = pd_out;
        let mut port = port_from_iodd();
        let mut device = Device::new(Rate::Com3, page).unwrap();
        let mut now = 0;
        device.set_inputs(&[1, 2, 3, 4]);
        run(&mut port, &mut device, &mut now, 1_000_000);
        assert_eq!(port.master().state(), State::Operate);
        assert_eq!(port.problem(), Some(problem));
        assert!(!port.running());
        assert_eq!(port.input(0), None);
        assert!(!port.set_outputs(Some(&[Value::Bool(true), Value::Int(1)])));
        run(&mut port, &mut device, &mut now, 50_000);
        assert_eq!(device.outputs(), None);
    }
    // No lengths in the map: a point past the Device's data is the
    // problem, by its index.
    let mut port = Port::new(
        parse(
            "device d iolink port=1\nin a bool d pd:0\nin b int d pd:24:u8\n",
            1,
        )
        .unwrap()
        .remove(0),
    );
    let mut page = sensor_page();
    page.0[3] = 0x11;
    page.0[5] = 0x10;
    let mut device = Device::new(Rate::Com3, page).unwrap();
    let mut now = 0;
    run(&mut port, &mut device, &mut now, 1_000_000);
    assert_eq!(
        port.problem(),
        Some(Problem::Point {
            input: true,
            index: 1
        })
    );
    assert_eq!(port.input(0), None);
}

#[test]
fn another_device_than_the_map_names_is_refused() {
    let mut port = port_from_iodd();
    let mut page = sensor_page();
    page.0[11] = 0x9B;
    let mut device = Device::new(Rate::Com2, page).unwrap();
    let mut now = 0;
    run(&mut port, &mut device, &mut now, 1_000_000);
    assert_eq!(
        port.problem(),
        Some(Problem::Refused(Fault::Identity(Identity {
            vendor: 310,
            device: 667
        })))
    );
    assert_eq!(port.input(0), None);
}

fn stored(map: &PortMap) -> usize {
    map.inputs.len() + map.outputs.len()
}

proptest! {
    /// Any text is answered, and what parses keeps to the limits.
    #[test]
    fn any_map_text_is_answered(text in "(device|in|out|[a-z0-9_ :=#\n]){0,60}", ports in 0u8..9) {
        if let Ok(maps) = parse(&text, ports) {
            prop_assert!(maps.len() <= usize::from(ports));
            for map in &maps {
                prop_assert!((1..=ports).contains(&map.port));
                prop_assert!(stored(map) <= 64);
            }
        }
    }

    /// Any value handed to a running port is sent whole or not at all,
    /// and what the Device gets is what was handed over.
    #[test]
    fn outputs_are_sent_whole_or_not_at_all(teach in any::<bool>(), gain in any::<i64>()) {
        let mut port = port_from_iodd();
        let mut device = Device::new(Rate::Com3, sensor_page()).unwrap();
        let mut now = 0;
        run(&mut port, &mut device, &mut now, 300_000);
        prop_assert!(port.running());
        let took = port.set_outputs(Some(&[Value::Bool(teach), Value::Int(gain)]));
        run(&mut port, &mut device, &mut now, 30_000);
        if (0..16).contains(&gain) {
            prop_assert!(took);
            let want = [u8::from(teach) | (u8::try_from(gain).unwrap() << 4)];
            prop_assert_eq!(device.outputs(), Some(&want[..]));
        } else {
            prop_assert!(!took);
            prop_assert_eq!(device.outputs(), None);
        }
    }
}

/// Real IODDs (docs/SOURCES.md): the map lines written from every
/// `*IODD1.1.xml` under `HELMSMAN_IODD_DIR` are lines a node takes.
///
/// `HELMSMAN_IODD_DIR=.../Examples cargo test -p helmsman-nodecore --test iolink -- --ignored --nocapture`
#[test]
#[ignore = "needs HELMSMAN_IODD_DIR, a directory of IODD files"]
fn map_lines_from_real_iodds_parse() {
    let dir = std::env::var("HELMSMAN_IODD_DIR").expect("HELMSMAN_IODD_DIR");
    let mut read = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with("IODD1.1.xml") {
            continue;
        }
        let description = iodd::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for which in 0..description.process_data.len() {
            let lines = description.map_lines(which, "dev", 1).unwrap();
            let maps = parse(&lines, 1).unwrap_or_else(|e| panic!("{}: {e}\n{lines}", path.display()));
            let layout = &description.process_data[which];
            let fields = |item: &Option<iodd::Item>| item.as_ref().map_or(0, |i| i.fields.len());
            let unmapped = lines.matches("# not mapped").count();
            assert_eq!(
                stored(&maps[0]) + unmapped,
                fields(&layout.input) + fields(&layout.output),
                "{}:\n{lines}",
                path.display()
            );
            read += 1;
        }
    }
    println!("{read} layouts written and parsed");
    assert!(read > 0);
}
