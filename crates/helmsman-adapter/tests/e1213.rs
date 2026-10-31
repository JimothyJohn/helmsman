//! The E1213 profile and targets built from an EDS: the profile is what
//! the EDS says (fixture: our restatement of the vendor's facts), the
//! device refuses what the EDS doesn't allow, and the simulator knobs
//! misbehave as documented. Sans-IO: every call takes `now`.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use helmsman_adapter::e1213::{self, Application};
use helmsman_adapter::{Device, DeviceConfig, Misbehave, PointKind};
use helmsman_proto::cip::Request;
use helmsman_proto::eds::Eds;
use helmsman_proto::io::{
    Class1Body, ConnectionPath, ForwardOpen, IoPacket, NetworkParams, Priority, Transport, class1_size,
    open_result,
};

const FACTS: &str = include_str!("../../helmsman-proto/tests/fixtures/e1213-facts.eds");
const PEER: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 9);

#[test]
fn the_profile_is_what_the_eds_says() {
    let from_eds = DeviceConfig::from_eds(&Eds::parse(FACTS).unwrap()).unwrap();
    let mut profile = DeviceConfig::moxa_e1213();
    // Neither is in the EDS: the serial is per device, the connection
    // limit is from the manual.
    profile.identity.serial_number = 0;
    profile.max_connections = 16;
    profile.connection_id_base = from_eds.connection_id_base;
    assert_eq!(from_eds, profile);
}

#[test]
fn eds_targets_refuse_what_they_cannot_simulate() {
    for (from, to, what) in [
        ("0x04010002,", "0x08010002,", "only exclusive owner"),
        ("0x44640405,", "0x44640505,", "real-time format 5"),
        ("0x04010002,", "0x04010001,", "no Class 1"),
    ] {
        let text = FACTS.replacen(from, to, 1);
        let text = if what == "no Class 1" {
            text.replacen("0x02010002,", "0x02010001,", 1)
        } else {
            text
        };
        let e = DeviceConfig::from_eds(&Eds::parse(&text).unwrap()).unwrap_err();
        assert!(e.contains(what), "{from} -> {to}: {e}");
    }
}

fn open(
    dev: &mut Device,
    serial: u16,
    path: ConnectionPath,
    out: u16,
    rpi_ms: u32,
    now: Instant,
) -> Result<u32, u16> {
    let fo = ForwardOpen {
        priority_time_tick: 0x0A,
        timeout_ticks: 0xF0,
        o_to_t_connection_id: 0,
        t_to_o_connection_id: 0xAB00 + u32::from(serial),
        connection_serial: serial,
        originator_vendor_id: 0xFFFF,
        originator_serial: 7,
        timeout_multiplier: 1,
        o_to_t_rpi_us: rpi_ms * 1000,
        o_to_t: NetworkParams::point_to_point(out, Priority::Scheduled),
        t_to_o_rpi_us: rpi_ms * 1000,
        t_to_o: NetworkParams::point_to_point(class1_size(51, false), Priority::Scheduled),
        transport: Transport::CLASS1_CYCLIC,
        path,
    };
    let req: Request = fo.to_request();
    match open_result(&dev.handle(&req, PEER, now)).unwrap() {
        Ok(r) => Ok(r.o_to_t_connection_id),
        Err(r) => Err(r.extended_code().unwrap_or(0)),
    }
}

fn owner_path() -> ConnectionPath {
    ConnectionPath::assembly(100, Some(102), Some(101))
}

#[test]
fn the_e1213_takes_its_two_connections_and_nothing_else() {
    let now = Instant::now();
    let mut dev = Device::new(DeviceConfig::moxa_e1213());
    let owner_size = class1_size(1, true);
    assert_eq!(
        open(&mut dev, 1, owner_path(), owner_size, 99, now),
        Err(0x0111),
        "RPI under 100 ms"
    );
    assert_eq!(
        open(&mut dev, 2, owner_path(), owner_size, 3001, now),
        Err(0x0111),
        "RPI over 3 s"
    );
    assert_eq!(
        open(&mut dev, 3, owner_path(), class1_size(1, false), 100, now),
        Err(0x0127),
        "no run/idle header"
    );
    let listen = ConnectionPath::assembly(100, Some(112), Some(101));
    assert_eq!(
        open(&mut dev, 4, listen, 2, 100, now),
        Err(0x012F),
        "no listen-only point"
    );
    open(&mut dev, 5, owner_path(), owner_size, 100, now).unwrap();
    assert_eq!(
        open(&mut dev, 6, owner_path(), owner_size, 100, now),
        Err(0x0106),
        "one owner"
    );
    let input_only = ConnectionPath::assembly(100, Some(111), Some(101));
    open(&mut dev, 7, input_only.clone(), 2, 3000, now).unwrap();
    let kinds: Vec<_> = dev.connections().iter().map(|c| c.point.kind).collect();
    assert_eq!(kinds, [PointKind::ExclusiveOwner, PointKind::InputOnly]);
    // The manual's limit: 9 read-only + 1 read/write.
    for serial in 8..16 {
        open(&mut dev, serial, input_only.clone(), 2, 100, now).unwrap();
    }
    assert_eq!(open(&mut dev, 16, input_only, 2, 100, now), Err(0x0113));
}

fn feed(dev: &mut Device, id: u32, seq: u32, out: u8, now: Instant) {
    let body = Class1Body {
        sequence_count: u16::try_from(seq).unwrap(),
        run_idle: Some(1),
        data: &[out],
    }
    .encode();
    let pkt = IoPacket {
        connection_id: id,
        encapsulation_sequence: Some(seq),
        body: &body,
    }
    .encode();
    dev.on_io_packet(&pkt, PEER, now).unwrap();
}

#[test]
fn the_application_fills_the_input_assembly() {
    let now = Instant::now();
    let mut dev = Device::new(DeviceConfig::moxa_e1213());
    let mut app = Application::bench();
    app.forced = 1 << 11;
    app.step(&mut dev);
    assert_eq!(
        dev.assembly(e1213::INPUT).unwrap()[..2],
        [0, 0b1000],
        "DI-11 forced on"
    );

    let id = open(&mut dev, 1, owner_path(), class1_size(1, true), 100, now).unwrap();
    for (seq, out) in [(1, 0b0000_0011), (2, 0), (3, 0b0000_0001)] {
        feed(&mut dev, id, seq, out, now);
        app.step(&mut dev);
    }
    let input = dev.assembly(e1213::INPUT).unwrap();
    assert_eq!(input[0], 0b01, "DO-00 -> DI-00 on, DO-01 -> DI-01 off");
    assert_eq!(input[e1213::DO_STATUS_OFFSET], 0b01);
    let counter = |n: usize| {
        let at = e1213::COUNTER_OFFSET + 4 * n;
        u32::from_le_bytes(input[at..at + 4].try_into().unwrap())
    };
    assert_eq!((counter(0), counter(1), counter(11)), (2, 1, 1), "rising edges");

    // The owner goes away: outputs off, so the wired inputs drop.
    dev.drop_connections();
    app.step(&mut dev);
    assert_eq!(dev.assembly(e1213::INPUT).unwrap()[0], 0);
    assert_eq!(dev.assembly(e1213::INPUT).unwrap()[e1213::DO_STATUS_OFFSET], 0);
}

#[test]
fn misbehaving_targets_produce_what_they_say() {
    let now = Instant::now();
    let mut dev = Device::new(DeviceConfig::moxa_e1213());
    open(&mut dev, 1, owner_path(), class1_size(1, true), 100, now).unwrap();
    let produced = |dev: &mut Device, at: Instant| -> Vec<usize> {
        dev.due_productions(at)
            .iter()
            .map(|(_, p)| IoPacket::decode(p).unwrap().body.len())
            .collect()
    };
    let rpi = Duration::from_millis(100);
    assert_eq!(produced(&mut dev, now), [2 + 51]);
    dev.set_misbehave(Some(Misbehave::ShortInput));
    assert_eq!(produced(&mut dev, now + rpi), [2 + 50]);
    dev.set_misbehave(Some(Misbehave::Garbage));
    assert_eq!(produced(&mut dev, now + rpi * 2), [2 + 51 * 2 + 3]);
    dev.set_misbehave(Some(Misbehave::Silent));
    assert_eq!(produced(&mut dev, now + rpi * 3), Vec::<usize>::new());
    dev.set_misbehave(None);
    assert_eq!(produced(&mut dev, now + rpi * 4), [2 + 51]);
    dev.drop_connections();
    assert_eq!(produced(&mut dev, now + rpi * 5), Vec::<usize>::new());
    assert_eq!(dev.connections().len(), 0);
}
