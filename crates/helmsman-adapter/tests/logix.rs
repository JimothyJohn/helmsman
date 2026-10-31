//! `TagReader` end to end against the Logix fixture behind the real
//! adapter server, over loopback TCP: reads, members, arrays, batches with
//! partial failures, fragmented transfers, browse, routing, and proof that
//! nothing but read services ever reached the controller.

use std::net::Ipv4Addr;
use std::time::Duration;

use helmsman_adapter::logix::{Controller, EXT_BEYOND_END, TagDef};
use helmsman_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use helmsman_proto::cip::GeneralStatus;
use helmsman_proto::logix::{AtomicType, DataType, Error, TagPath, TagReader, Value};

const TIMEOUT: Duration = Duration::from_secs(5);

fn cfg() -> ServerConfig {
    ServerConfig {
        bind: Ipv4Addr::LOCALHOST,
        encap_port: 0,
        io_port: 0,
        ..ServerConfig::default()
    }
}

fn plant() -> Controller {
    let mut c = Controller::new();
    c.add("Temp", TagDef::atomic(AtomicType::Real))
        .add("Count", TagDef::atomic(AtomicType::Dint))
        .add("Run", TagDef::atomic(AtomicType::Bool))
        .add("Small", TagDef::atomic(AtomicType::Sint))
        .add("Word", TagDef::atomic(AtomicType::Int))
        .add("Big", TagDef::atomic(AtomicType::Lint))
        .add("Ubig", TagDef::atomic(AtomicType::Ulint))
        .add("Precise", TagDef::atomic(AtomicType::Lreal))
        .add("Status", TagDef::atomic(AtomicType::Dint))
        .add("Matrix", TagDef::array(AtomicType::Int, &[3, 4]))
        .add("Log", TagDef::array(AtomicType::Dint, &[1000]))
        .add(
            "Recipe",
            TagDef::structure(
                0x0ABC,
                vec![
                    ("Speed", TagDef::atomic(AtomicType::Real)),
                    ("Steps", TagDef::array(AtomicType::Dint, &[4])),
                ],
            )
            .with_dims(&[5]),
        )
        .add(
            "Local:1:I",
            TagDef::structure(0x0123, vec![("Data", TagDef::atomic(AtomicType::Dint))]),
        )
        .add_program_tag("Main", "Temp", TagDef::atomic(AtomicType::Real))
        .add_program_tag("Main", "Level", TagDef::atomic(AtomicType::Udint));
    c.set("Temp", &21.5f32.to_le_bytes()).unwrap();
    c.set("Count", &(-7i32).to_le_bytes()).unwrap();
    c.set("Run", &[1]).unwrap();
    c.set("Small", &[0x80]).unwrap();
    c.set("Word", &1234i16.to_le_bytes()).unwrap();
    c.set("Big", &i64::MIN.to_le_bytes()).unwrap();
    c.set("Ubig", &u64::MAX.to_le_bytes()).unwrap();
    c.set("Precise", &std::f64::consts::PI.to_le_bytes()).unwrap();
    c.set("Status", &0b10_0000i32.to_le_bytes()).unwrap();
    c.set("Matrix[2,3]", &99i16.to_le_bytes()).unwrap();
    let log: Vec<u8> = (0..1000i32).flat_map(i32::to_le_bytes).collect();
    c.set("Log", &log).unwrap();
    c.set("Recipe[3].Speed", &2.5f32.to_le_bytes()).unwrap();
    c.set("Recipe[3].Steps[2]", &42i32.to_le_bytes()).unwrap();
    c.set("Local:1:I.Data", &0x55i32.to_le_bytes()).unwrap();
    c.set("Program:Main.Temp", &80.0f32.to_le_bytes()).unwrap();
    c.set("Program:Main.Level", &4_000_000_000u32.to_le_bytes())
        .unwrap();
    c
}

fn start(c: Controller) -> Adapter {
    Adapter::start(Device::new(DeviceConfig::opener_sample()).with_logix(c), &cfg()).unwrap()
}

fn reader(a: &Adapter) -> TagReader {
    TagReader::connect(a.encap_addr(), TIMEOUT).unwrap()
}

fn p(s: &str) -> TagPath {
    TagPath::parse(s).unwrap()
}

fn one(r: &mut TagReader, s: &str) -> Value {
    r.read_tag(&p(s), 1)
        .unwrap_or_else(|e| panic!("{s}: {e}"))
        .value()
        .unwrap()
}

/// Every service the controller saw was a read.
fn assert_only_reads(a: &Adapter) {
    let dev = a.device();
    let d = dev.lock().unwrap();
    let seen = d.logix().unwrap().services_seen();
    for s in seen.keys() {
        assert!(
            [0x4C, 0x52, 0x55, 0x0A].contains(s),
            "controller saw service 0x{s:02X}: {seen:02X?}"
        );
    }
}

fn cip_status(e: &Error) -> (u8, Vec<u16>) {
    match e {
        Error::Cip { status, additional } => (status.0, additional.clone()),
        other => panic!("expected a CIP error, got {other}"),
    }
}

#[test]
fn reads_every_atomic_type() {
    let a = start(plant());
    let mut r = reader(&a);
    assert_eq!(one(&mut r, "Temp"), Value::Real(21.5));
    assert_eq!(
        one(&mut r, "count"),
        Value::Dint(-7),
        "names are case-insensitive"
    );
    assert_eq!(one(&mut r, "Run"), Value::Bool(true));
    assert_eq!(one(&mut r, "Small"), Value::Sint(-128));
    assert_eq!(one(&mut r, "Word"), Value::Int(1234));
    assert_eq!(one(&mut r, "Big"), Value::Lint(i64::MIN));
    assert_eq!(one(&mut r, "Ubig"), Value::Ulint(u64::MAX));
    assert_eq!(one(&mut r, "Precise"), Value::Lreal(std::f64::consts::PI));
    assert_eq!(one(&mut r, "Program:Main.Temp"), Value::Real(80.0));
    assert_eq!(one(&mut r, "Program:Main.Level"), Value::Udint(4_000_000_000));
    // The bit suffix is picked out by the caller; the wire read is the DINT.
    let status = r.read_tag(&p("Status.5"), 1).unwrap().value().unwrap();
    assert_eq!(status.bit(5), Some(true));
    r.close();
    assert_only_reads(&a);
}

#[test]
fn reads_array_elements_and_members() {
    let a = start(plant());
    let mut r = reader(&a);
    assert_eq!(one(&mut r, "Matrix[2,3]"), Value::Int(99));
    assert_eq!(one(&mut r, "Log[999]"), Value::Dint(999));
    assert_eq!(one(&mut r, "Recipe[3].Speed"), Value::Real(2.5));
    assert_eq!(one(&mut r, "Recipe[3].Steps[2]"), Value::Dint(42));
    assert_eq!(one(&mut r, "Local:1:I.Data"), Value::Dint(0x55));
    // A slice: elements from an index on.
    let d = r.read_tag(&p("Log[10]"), 3).unwrap();
    assert_eq!(
        d.values().unwrap(),
        [Value::Dint(10), Value::Dint(11), Value::Dint(12)]
    );
    // A whole structure element comes back as a structure, raw.
    let s = r.read_tag(&p("Recipe[3]"), 1).unwrap();
    assert_eq!(s.ty, DataType::Struct { handle: 0x0ABC });
    assert_eq!(s.bytes.len(), 4 + 16);
    assert_eq!(&s.bytes[..4], 2.5f32.to_le_bytes());
    r.close();
    assert_only_reads(&a);
}

#[test]
fn unknown_tags_and_bad_paths_get_logix_statuses() {
    let a = start(plant());
    let mut r = reader(&a);
    let cases = [
        ("Nope", 1, (0x05, vec![])),
        ("Program:Other.Temp", 1, (0x05, vec![])),
        ("Recipe[0].Nope", 1, (0x05, vec![])),
        ("Temp.Member", 1, (0x05, vec![])),
        ("Temp[0]", 1, (0x04, vec![])),
        ("Matrix[1]", 1, (0x04, vec![])),
        ("Recipe.Speed", 1, (0x04, vec![])),
        ("Log[1000]", 1, (0xFF, vec![EXT_BEYOND_END])),
        ("Log[998]", 3, (0xFF, vec![EXT_BEYOND_END])),
        ("Temp", 2, (0xFF, vec![EXT_BEYOND_END])),
        ("Temp", 0, (0xFF, vec![EXT_BEYOND_END])),
    ];
    for (tag, n, want) in cases {
        let e = r.read_tag(&p(tag), n).expect_err(tag);
        assert_eq!(cip_status(&e), want, "{tag} x{n}");
    }
    // The session survives every refusal.
    assert_eq!(one(&mut r, "Temp"), Value::Real(21.5));
    r.close();
}

#[test]
fn batches_report_each_tag_on_its_own() {
    let a = start(plant());
    let mut r = reader(&a);
    let items: Vec<(TagPath, u16)> = ["Temp", "Nope", "Count", "Log[5000]", "Program:Main.Temp"]
        .iter()
        .map(|s| (p(s), 1))
        .collect();
    let out = r.read_tags(&items).unwrap();
    assert_eq!(out.len(), 5);
    assert_eq!(out[0].as_ref().unwrap().value().unwrap(), Value::Real(21.5));
    assert_eq!(cip_status(out[1].as_ref().unwrap_err()).0, 0x05);
    assert_eq!(out[2].as_ref().unwrap().value().unwrap(), Value::Dint(-7));
    assert_eq!(cip_status(out[3].as_ref().unwrap_err()).0, 0xFF);
    assert_eq!(out[4].as_ref().unwrap().value().unwrap(), Value::Real(80.0));
    let seen = a
        .device()
        .lock()
        .unwrap()
        .logix()
        .unwrap()
        .services_seen()
        .clone();
    assert_eq!(seen.get(&0x0A), Some(&1), "one packet for five tags: {seen:02X?}");
    r.close();
    assert_only_reads(&a);
}

#[test]
fn many_tags_split_across_packets_in_order() {
    let mut c = Controller::new();
    for i in 0..200 {
        c.add(
            &format!("Tag_with_a_long_name_{i:03}"),
            TagDef::atomic(AtomicType::Dint),
        );
        c.set(&format!("Tag_with_a_long_name_{i:03}"), &i32::to_le_bytes(i * 3))
            .unwrap();
    }
    let a = start(c);
    let mut r = reader(&a);
    let items: Vec<(TagPath, u16)> = (0..200)
        .map(|i| (p(&format!("Tag_with_a_long_name_{i:03}")), 1))
        .collect();
    let out = r.read_tags(&items).unwrap();
    for (i, v) in (0i32..).zip(&out) {
        assert_eq!(v.as_ref().unwrap().value().unwrap(), Value::Dint(i * 3));
    }
    let packets = a.device().lock().unwrap().logix().unwrap().services_seen()[&0x0A];
    assert!(packets > 1, "200 tags cannot fit one packet");
    r.close();
}

#[test]
fn large_arrays_come_back_fragmented() {
    let a = start(plant());
    let mut r = reader(&a);
    let want: Vec<Value> = (0..1000).map(Value::Dint).collect();
    // Read Tag answers 0x06 and the reader finishes with Read Tag Fragmented.
    let d = r.read_tag(&p("Log"), 1000).unwrap();
    assert_eq!(d.values().unwrap(), want);
    let d = r.read_tag_fragmented(&p("Log"), 1000).unwrap();
    assert_eq!(d.values().unwrap(), want);
    let d = r.read_tag(&p("Log[100]"), 900).unwrap();
    assert_eq!(d.values().unwrap(), want[100..]);
    let seen = a
        .device()
        .lock()
        .unwrap()
        .logix()
        .unwrap()
        .services_seen()
        .clone();
    assert!(seen[&0x52] >= 8, "4000 bytes in <500-byte replies: {seen:02X?}");
    r.close();
    assert_only_reads(&a);
}

#[test]
fn a_full_batch_finishes_big_reads_fragmented() {
    let a = start(plant());
    let mut r = reader(&a);
    let items = vec![
        (p("Log"), 1000),
        (p("Temp"), 1),
        (p("Log[500]"), 500),
        (p("Count"), 1),
    ];
    let out = r.read_tags(&items).unwrap();
    let log: Vec<Value> = (0..1000).map(Value::Dint).collect();
    assert_eq!(out[0].as_ref().unwrap().values().unwrap(), log);
    assert_eq!(out[1].as_ref().unwrap().value().unwrap(), Value::Real(21.5));
    assert_eq!(out[2].as_ref().unwrap().values().unwrap(), log[500..]);
    assert_eq!(out[3].as_ref().unwrap().value().unwrap(), Value::Dint(-7));
    r.close();
}

#[test]
fn browse_lists_controller_and_program_tags() {
    let mut c = plant();
    for i in 0..60 {
        c.add(&format!("Filler_{i}"), TagDef::atomic(AtomicType::Int));
    }
    let a = start(c);
    let mut r = reader(&a);
    let syms = r.list_tags(None).unwrap();
    let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"Temp") && names.contains(&"Filler_59"),
        "{names:?}"
    );
    assert!(names.contains(&"Program:Main"));
    assert_eq!(syms.len(), 13 + 60 + 1);
    let mut sorted = syms.iter().map(|s| s.instance).collect::<Vec<_>>();
    sorted.dedup();
    assert_eq!(sorted.len(), syms.len(), "no symbol twice");
    let log = syms.iter().find(|s| s.name == "Log").unwrap();
    assert_eq!(log.atomic(), Some(AtomicType::Dint));
    assert_eq!(log.shape(), [1000]);
    let recipe = syms.iter().find(|s| s.name == "Recipe").unwrap();
    assert!(recipe.is_struct());
    assert_eq!(recipe.shape(), [5]);
    let main = r.list_tags(Some("Program:Main")).unwrap();
    let names: Vec<&str> = main.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Temp", "Level"]);
    let e = r.list_tags(Some("Program:Nope")).unwrap_err();
    assert_eq!(cip_status(&e).0, 0x05);
    let seen = a.device().lock().unwrap().logix().unwrap().services_seen()[&0x55];
    assert!(seen > 2, "74 symbols take more than one reply");
    r.close();
    assert_only_reads(&a);
}

#[test]
fn routed_requests_reach_the_slot_and_wrong_slots_are_refused() {
    let mut chassis = plant();
    chassis.slot = 2;
    let a = start(chassis);
    let mut r = reader(&a).with_slot(2);
    assert_eq!(one(&mut r, "Temp"), Value::Real(21.5));
    let log = r.read_tag(&p("Log"), 1000).unwrap();
    assert_eq!(log.values().unwrap().len(), 1000);
    let out = r.read_tags(&[(p("Temp"), 1), (p("Count"), 1)]).unwrap();
    assert!(out.iter().all(Result::is_ok), "{out:?}");
    assert!(r.list_tags(None).unwrap().len() > 10);
    // Routed Identity read is answered by the device behind the fixture.
    let vendor = r.get_attribute_single(1, 1, 1).unwrap();
    assert_eq!(vendor.len(), 2);
    r.close();
    let mut wrong = reader(&a).with_slot(3);
    let refused = wrong.read_tag(&p("Temp"), 1).unwrap_err();
    assert_eq!(
        cip_status(&refused),
        (GeneralStatus::CONNECTION_FAILURE.0, vec![0x0312])
    );
    wrong.close();
    assert_only_reads(&a);
}

#[test]
fn values_change_between_reads() {
    let a = start(plant());
    let mut r = reader(&a);
    assert_eq!(one(&mut r, "Count"), Value::Dint(-7));
    a.device()
        .lock()
        .unwrap()
        .logix_mut()
        .unwrap()
        .set("Count", &8i32.to_le_bytes())
        .unwrap();
    assert_eq!(one(&mut r, "Count"), Value::Dint(8));
    r.close();
}

mod fixture_input {
    use helmsman_adapter::logix::Controller;
    use helmsman_proto::cip::Response;
    use proptest::prelude::*;

    use super::plant;

    #[test]
    fn writes_are_not_served() {
        let mut c = plant();
        // Write Tag "Temp" REAL x1 = 0.0, and Write Tag Fragmented.
        let write = [
            0x4D, 3, 0x91, 4, b'T', b'e', b'm', b'p', 0xCA, 0, 1, 0, 0, 0, 0, 0,
        ];
        let r = c.handle(&write).unwrap();
        assert_eq!(r.status.0, 0x08);
        let mut frag = write;
        frag[0] = 0x53;
        assert_eq!(c.handle(&frag).unwrap().status.0, 0x08);
        assert_eq!(c.services_seen().get(&0x4D), Some(&1));
    }

    #[test]
    fn non_logix_requests_are_handed_back() {
        let mut c = Controller::new();
        // Get_Attribute_Single Identity 1.1.
        assert!(c.handle(&[0x0E, 3, 0x20, 0x01, 0x24, 0x01, 0x30, 0x01]).is_none());
        // Unconnected_Send around it is the device's, too.
        let us = [
            0x52, 2, 0x20, 0x06, 0x24, 0x01, 0x0A, 0x05, 8, 0, 0x0E, 3, 0x20, 0x01, 0x24, 0x01, 0x30, 0x01,
            1, 0, 1, 0,
        ];
        assert!(c.handle(&us).is_none());
    }

    #[test]
    fn hostile_requests_get_errors() {
        let mut c = plant();
        let cases: &[&[u8]] = &[
            &[0x4C, 3, 0x91, 4, b'T', b'e', b'm', b'p'], // no element count
            &[0x52, 3, 0x91, 4, b'T', b'e', b'm', b'p', 1, 0], // fragmented, no offset
            &[0x4C, 3, 0x91, 4, b'T', b'e', b'm', b'p', 1, 0, 9], // trailing byte
            &[0x4C, 2, 0x91, 9, b'T', b'e'],             // name runs past the path
            &[0x4C, 2, 0x91, 2, 0, 0, 1, 0],             // NUL name
            &[0x4C, 3, 0x28, 1, 0x91, 1, b'T', 0, 1, 0], // index before a name
            &[0x0A, 2, 0x20, 0x02, 0x24, 0x01, 2, 0, 0xFF, 0x7F, 0, 0], // offsets past the end
            &[0x0A, 2, 0x20, 0x02, 0x24, 0x01],          // empty packet
            &[0x55, 2, 0x20, 0x6B, 0x24, 0x00, 1, 0, 99, 0], // unknown attribute
            &[0x55, 2, 0x20, 0x6B, 0x24, 0x00, 5, 0, 1, 0], // attribute list cut short
            &[0x4C, 9, 0x91, 4, b'T', b'e', b'm', b'p'], // path size lie
        ];
        for req in cases {
            let r = c.handle(req).unwrap_or_else(|| panic!("{req:02X?} not handled"));
            assert!(!r.status.is_success(), "{req:02X?} gave {r:?}");
        }
    }

    #[test]
    fn nested_packets_are_refused_inside() {
        let mut c = plant();
        let inner = [0x4C, 3, 0x91, 4, b'T', b'e', b'm', b'p', 1, 0];
        let mut msp = vec![0x0A, 2, 0x20, 0x02, 0x24, 0x01, 1, 0, 4, 0];
        msp.extend_from_slice(&inner);
        let mut outer = vec![0x0A, 2, 0x20, 0x02, 0x24, 0x01, 1, 0, 4, 0];
        outer.extend_from_slice(&msp);
        let r = c.handle(&outer).unwrap();
        assert_eq!(r.status.0, 0x1E);
        let parts = helmsman_proto::logix::wire::split_multiple(&r.data).unwrap();
        assert_eq!(Response::decode(parts[0]).unwrap().status.0, 0x08);
    }

    proptest! {
        #[test]
        fn arbitrary_requests_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
            let mut c = plant();
            let _ = c.handle(&bytes);
        }

        #[test]
        fn arbitrary_symbolic_reads_never_panic(
            name in "[A-Za-z_:]{1,12}",
            idx in prop::collection::vec(any::<u32>(), 0..4),
            n in any::<u16>(),
            off in any::<u32>(),
            frag in any::<bool>(),
        ) {
            let mut c = plant();
            let mut path = vec![0x91, u8::try_from(name.len()).unwrap()];
            path.extend_from_slice(name.as_bytes());
            if name.len() % 2 == 1 {
                path.push(0);
            }
            for i in idx {
                path.extend_from_slice(&[0x2A, 0]);
                path.extend_from_slice(&i.to_le_bytes());
            }
            let mut req = vec![if frag { 0x52 } else { 0x4C }, u8::try_from(path.len() / 2).unwrap()];
            req.extend_from_slice(&path);
            req.extend_from_slice(&n.to_le_bytes());
            if frag {
                req.extend_from_slice(&off.to_le_bytes());
            }
            let r = c.handle(&req).unwrap();
            prop_assert!(r.data.len() + 4 <= c.max_reply);
        }
    }
}
