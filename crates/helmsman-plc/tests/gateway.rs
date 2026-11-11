//! Devices behind one Modbus TCP gateway: each is its own link, with its
//! own scan and its own up/down reports, on one shared connection.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helmsman_io::local::Channels;
use helmsman_io::sim::{Cap, Fault, Gateway, Sim, State};
use helmsman_io::{Value, text};
use helmsman_plc::SignalTable;
use helmsman_plc::link::{links, spawn};

type Reports = Arc<Mutex<Vec<(String, Option<String>)>>>;

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The last thing `link` reported: `Ok(())` up, `Err(why)` down.
fn last(r: &Reports, link: &str) -> Option<Result<(), String>> {
    r.lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(n, _)| n == link)
        .map(|(_, e)| e.clone().map_or(Ok(()), Err))
}

#[test]
fn a_silent_unit_goes_down_alone_on_a_shared_gateway() {
    let units = (1..=8u8)
        .map(|u| {
            let mut s = State::new(4);
            s.holding[0] = u16::from(u) * 100;
            (u, s)
        })
        .collect();
    let sim = Sim::gateway(Gateway {
        units,
        max_connections: Some((1, Cap::Refuse)),
    })
    .unwrap();
    // Unit 3 gives up quickly; the others are patient, so waiting their
    // turn behind it never takes them down on a loaded machine.
    let mut src = String::new();
    for u in 1..=8 {
        let ms = if u == 3 { 150 } else { 3000 };
        writeln!(
            src,
            "device rtu{u} modbus-tcp {} unit={u} timeout_ms={ms} poll_ms=20\nin v{u} int rtu{u} hr:0",
            sim.endpoint()
        )
        .unwrap();
    }
    let file = text::parse(&src).unwrap();
    let table = Arc::new(Mutex::new(SignalTable::new(&file.map, &BTreeMap::new()).unwrap()));
    let reports: Reports = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = links(&file.map, helmsman_io::open, Box::new(Channels::new()))
        .unwrap()
        .into_iter()
        .map(|l| {
            let r = Arc::clone(&reports);
            spawn(l, Arc::clone(&table), Arc::clone(&stop), move |n, e, _| {
                r.lock()
                    .unwrap()
                    .push((n.to_string(), e.map(ToString::to_string)));
            })
        })
        .collect();
    let value = |name: &str| {
        let t = table.lock().unwrap();
        t.value(t.index(name).unwrap())
    };
    wait("every unit up and read", || {
        (1..=8).all(|u| {
            last(&reports, &format!("rtu{u}")) == Some(Ok(()))
                && value(&format!("v{u}")) == Some(Value::Int(i64::from(u) * 100))
        })
    });

    sim.mute(3, true);
    wait(
        "unit 3 down for its own reason",
        || matches!(last(&reports, "rtu3"), Some(Err(why)) if why.contains("unit 3: no reply")),
    );
    sim.unit(4).holding[0] = 401;
    wait("unit 4 still read", || value("v4") == Some(Value::Int(401)));
    for u in [1, 2, 4, 5, 6, 7, 8] {
        assert_eq!(
            last(&reports, &format!("rtu{u}")),
            Some(Ok(())),
            "rtu{u} stays up beside a silent unit"
        );
    }
    sim.mute(3, false);
    wait("unit 3 back", || last(&reports, "rtu3") == Some(Ok(())));
    assert_eq!(sim.accepted(), 1, "eight devices, one connection");
    assert_eq!(sim.refused(), 0);

    stop.store(true, Ordering::Release);
    for t in threads {
        t.thread().unpark();
        t.join().unwrap();
    }
}

#[test]
fn a_gateway_that_reports_a_dead_unit_takes_that_device_down_alone() {
    let units = [3u8, 4]
        .into_iter()
        .map(|u| {
            let mut s = State::new(4);
            s.holding[0] = u16::from(u) * 100;
            (u, s)
        })
        .collect();
    let sim = Sim::gateway(Gateway {
        units,
        max_connections: Some((1, Cap::Refuse)),
    })
    .unwrap();
    let mut src = String::new();
    for u in [3, 4] {
        writeln!(
            src,
            "device rtu{u} modbus-tcp {} unit={u} timeout_ms=3000 poll_ms=20\nin v{u} int rtu{u} hr:0",
            sim.endpoint()
        )
        .unwrap();
    }
    let file = text::parse(&src).unwrap();
    let table = Arc::new(Mutex::new(SignalTable::new(&file.map, &BTreeMap::new()).unwrap()));
    let reports: Reports = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = links(&file.map, helmsman_io::open, Box::new(Channels::new()))
        .unwrap()
        .into_iter()
        .map(|l| {
            let r = Arc::clone(&reports);
            spawn(l, Arc::clone(&table), Arc::clone(&stop), move |n, e, _| {
                r.lock()
                    .unwrap()
                    .push((n.to_string(), e.map(ToString::to_string)));
            })
        })
        .collect();
    wait("both units up", || {
        last(&reports, "rtu3") == Some(Ok(())) && last(&reports, "rtu4") == Some(Ok(()))
    });

    // Unit 3 answers 0x0B (target failed to respond) until cleared.
    sim.unit(3).faults = vec![Fault::Exception(0x0B); 50];
    wait(
        "unit 3 down with the gateway's reason",
        || matches!(last(&reports, "rtu3"), Some(Err(why)) if why.contains("did not respond")),
    );
    assert_eq!(last(&reports, "rtu4"), Some(Ok(())), "unit 4 stays up");
    sim.unit(3).faults.clear();
    wait("unit 3 back", || last(&reports, "rtu3") == Some(Ok(())));
    assert_eq!(sim.accepted(), 1, "the connection was kept");

    stop.store(true, Ordering::Release);
    for t in threads {
        t.thread().unpark();
        t.join().unwrap();
    }
}
