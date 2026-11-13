//! A device by its profile (`device:moxa-e1213`, `ch:` points) against the
//! simulated E1213 on both of its protocols, each simulator standing in
//! for one face of the same unit: EtherNet/IP when its one connection is
//! free, Modbus TCP when another controller owns it, with the same
//! channels either way, and back to the first transport after a link
//! loss.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use helmsman_io::enip::EnipDriver;
use helmsman_io::enip_sim::E1213Sim;
use helmsman_io::profile::Profile;
use helmsman_io::profile_driver::{Open, ProfileDriver};
use helmsman_io::sim::{Sim, State};
use helmsman_io::{Device, Driver, Error, Ty, Value};

const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");

fn port(endpoint: &str) -> String {
    endpoint.rsplit_once(':').unwrap().1.to_string()
}

/// A Modbus TCP face of the unit: DO-00..03 wired to DI-00..03 as on the
/// bench.
fn modbus() -> Sim {
    let mut s = State::new(64);
    s.wires = (0..4).map(|n| (n, n)).collect();
    Sim::start(s).unwrap()
}

/// The device line reaching `enip` and `mb` as one unit on 127.0.0.1.
fn line(enip: &E1213Sim, mb: &Sim) -> Device {
    let mut params: BTreeMap<String, String> = enip
        .params()
        .into_iter()
        .filter(|(k, _)| k != "profile" && k != "rpi_ms")
        .map(|(k, v)| (format!("enip-io.{k}"), v))
        .collect();
    params.insert("enip-io.port".into(), port(&enip.endpoint()));
    params.insert("modbus-tcp.port".into(), port(&mb.endpoint()));
    params.insert("modbus-tcp.timeout_ms".into(), "500".into());
    params.insert("poll_ms".into(), "20".into());
    Device {
        id: "rio".into(),
        driver: "device:moxa-e1213".into(),
        endpoint: "127.0.0.1".into(),
        params,
    }
}

fn opener() -> Open {
    Box::new(helmsman_io::open)
}

fn profiled(enip: &E1213Sim, mb: &Sim) -> ProfileDriver {
    ProfileDriver::new(Profile::parse(E1213).unwrap(), &line(enip, mb), opener()).unwrap()
}

/// Reads until `ch` is `want`, or panics after two seconds.
fn wait(d: &mut dyn Driver, ch: &str, want: Value) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match d.read(&[ch]) {
            Ok(v) if v[0] == want => return,
            other if Instant::now() > deadline => panic!("{ch}: {other:?}, want {want:?}"),
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[test]
fn the_first_transport_is_used_when_it_comes_up() {
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let mut d = profiled(&enip, &mb);
    assert_eq!(d.transport(), None, "nothing has worked yet");
    enip.force_di(5, true);
    wait(&mut d, "ch:DI-05", Value::Bool(true));
    assert_eq!(d.transport().as_deref(), Some("enip-io"));
    d.write("ch:do-01", Value::Bool(true)).unwrap();
    wait(&mut d, "ch:DI-01", Value::Bool(true));
    assert_eq!(mb.state().requests, 0, "Modbus was never asked");
}

#[test]
fn a_unit_another_controller_owns_is_read_over_modbus() {
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let mut owner = EnipDriver::from_params(&enip.endpoint(), &enip.params()).unwrap();
    owner.read(&["di:0"]).unwrap();

    let mut d = profiled(&enip, &mb);
    mb.state().discrete[4] = true;
    wait(&mut d, "ch:DI-04", Value::Bool(true));
    assert_eq!(d.transport().as_deref(), Some("modbus-tcp"));
    d.write("ch:DO-02", Value::Bool(true)).unwrap();
    assert!(mb.state().coils[2], "the write went to coil 2");
    wait(&mut d, "ch:DO-02-status", Value::Bool(true));

    // A link loss sends it back to the first transport, which the owner
    // has let go of meanwhile.
    drop(owner);
    mb.mute(1, true);
    let e = d.read(&["ch:DI-04"]).unwrap_err();
    assert!(e.is_link(), "{e}");
    assert_eq!(d.transport(), None);
    enip.force_di(4, true);
    wait(&mut d, "ch:DI-04", Value::Bool(true));
    assert_eq!(d.transport().as_deref(), Some("enip-io"));
}

#[test]
fn nothing_answering_is_a_link_error_after_every_transport_was_tried() {
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let mut owner = EnipDriver::from_params(&enip.endpoint(), &enip.params()).unwrap();
    owner.read(&["di:0"]).unwrap();
    mb.mute(1, true);
    let mut d = profiled(&enip, &mb);
    let e = d.read(&["ch:DI-00"]).unwrap_err();
    assert!(e.is_link(), "{e}");
    assert!(mb.accepted() > 0, "Modbus was tried after EtherNet/IP");
    assert_eq!(d.transport(), None);
}

#[test]
fn channels_parse_from_the_profile() {
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let d = profiled(&enip, &mb);
    let i = d.parse("ch:DI-04-count").unwrap();
    assert_eq!((i.ty, i.writable), (Ty::Int, false));
    let o = d.parse("ch:do-00").unwrap();
    assert_eq!((o.ty, o.writable), (Ty::Bool, true));
    for bad in ["ch:DI-99", "di:4", "ch:", "DI-04"] {
        assert!(matches!(d.parse(bad), Err(Error::Address(_))), "{bad}");
    }
}

#[test]
fn errors_before_any_traffic() {
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let mut d = profiled(&enip, &mb);
    assert!(matches!(
        d.write("ch:DI-00", Value::Bool(true)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(d.read(&["ch:nope"]), Err(Error::Address(_))));
    assert_eq!(mb.state().requests, 0);
}

#[test]
fn the_line_takes_poll_ms_and_per_transport_params_only() {
    let profile = Profile::parse(E1213).unwrap();
    let dev = |params: &[(&str, &str)], endpoint: &str| Device {
        id: "rio".into(),
        driver: "device:moxa-e1213".into(),
        endpoint: endpoint.into(),
        params: params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    };
    let ok = ProfileDriver::new(
        profile.clone(),
        &dev(
            &[
                ("poll_ms", "50"),
                ("modbus-tcp.unit", "3"),
                ("modbus-tcp.port", "1502"),
            ],
            "10.0.0.5:9",
        ),
        opener(),
    )
    .unwrap();
    let mb = &ok.lines()[1];
    assert_eq!(mb.driver, "modbus-tcp");
    assert_eq!(mb.endpoint, "10.0.0.5:1502");
    assert_eq!(mb.params.get("unit").map(String::as_str), Some("3"));
    assert_eq!(ok.lines()[0].endpoint, "10.0.0.5:9");
    assert_eq!(
        ok.lines()[0].params.get("profile").map(String::as_str),
        Some("moxa-e1213")
    );
    assert!(!ok.lines()[0].params.contains_key("poll_ms"));

    for (params, endpoint) in [
        (&[("unit", "3")][..], "10.0.0.5"),
        (&[("profinet-io.x", "1")][..], "10.0.0.5"),
        (&[("modbus-tcp.port", "0")][..], "10.0.0.5"),
        (&[("modbus-tcp.port", "502")][..], "mac=00-90-e8-7e-af-be"),
        (&[("modbus-tcp.", "1")][..], "10.0.0.5"),
    ] {
        let e = ProfileDriver::new(profile.clone(), &dev(params, endpoint), opener()).unwrap_err();
        assert!(matches!(e, Error::Address(_)), "{params:?}: {e}");
    }
}

#[test]
fn the_pace_sets_a_cyclic_transports_rate_inside_the_profiles_limits() {
    let (enip, mb) = (E1213Sim::start().unwrap(), modbus());
    let mut d = profiled(&enip, &mb);
    assert_eq!(
        d.limits(),
        Some((Duration::from_millis(100), Duration::from_secs(3))),
        "the first transport's"
    );
    let rpi = |d: &ProfileDriver| d.lines()[0].params.get("rpi_ms").cloned();
    assert_eq!(
        rpi(&d).as_deref(),
        Some("100"),
        "the profile's own, before any pace"
    );
    d.pace(Duration::from_millis(10));
    assert_eq!(
        rpi(&d).as_deref(),
        Some("100"),
        "a 10 ms program is held to the floor"
    );
    d.pace(Duration::from_millis(250));
    assert_eq!(rpi(&d).as_deref(), Some("250"));
    d.pace(Duration::from_secs(9));
    assert_eq!(rpi(&d).as_deref(), Some("3000"), "held to the ceiling");
    assert!(
        !d.lines()[1].params.contains_key("rpi_ms") && !d.lines()[1].params.contains_key("cycle_ms"),
        "Modbus is polled, not paced: {:?}",
        d.lines()[1].params
    );
    // And the paced connection opens: the unit takes the RPI asked.
    d.pace(Duration::from_millis(200));
    enip.force_di(3, true);
    wait(&mut d, "ch:DI-03", Value::Bool(true));
    assert_eq!(d.transport().as_deref(), Some("enip-io"));
}
