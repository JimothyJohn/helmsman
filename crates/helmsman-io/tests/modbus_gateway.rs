//! Several units behind one Modbus TCP gateway: one connection for all of
//! them, requests matched by transaction id, each unit's errors its own.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use helmsman_io::modbus::{self, ModbusDriver};
use helmsman_io::sim::{Cap, Fault, Gateway, Sim, State};
use helmsman_io::{Driver, Error, Value};

/// A gateway with units `units`, each holding `unit * 100` in hr:0.
fn gateway(units: impl IntoIterator<Item = u8>, cap: Option<(usize, Cap)>) -> Sim {
    Sim::gateway(Gateway {
        units: units
            .into_iter()
            .map(|u| {
                let mut s = State::new(8);
                s.holding[0] = u16::from(u) * 100;
                (u, s)
            })
            .collect(),
        max_connections: cap,
    })
    .unwrap()
}

fn driver(s: &Sim, unit: u8, timeout_ms: u64) -> ModbusDriver {
    let p = BTreeMap::from([
        ("unit".to_string(), unit.to_string()),
        ("timeout_ms".to_string(), timeout_ms.to_string()),
    ]);
    ModbusDriver::from_params(&s.endpoint(), &p).unwrap()
}

fn hr0(d: &mut ModbusDriver) -> Result<Value, Error> {
    d.read(&["hr:0"]).map(|v| v[0])
}

fn wait(what: &str, deadline: Duration, mut ok: impl FnMut() -> bool) {
    let until = Instant::now() + deadline;
    while !ok() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::yield_now();
    }
}

fn timed_out(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut)
}

#[test]
fn eight_units_behind_a_one_connection_gateway_share_its_connection() {
    for cap in [Cap::Refuse, Cap::CloseOldest] {
        let s = gateway(1..=8, Some((1, cap)));
        let workers: Vec<_> = (1..=8u8)
            .map(|u| {
                let mut d = driver(&s, u, 2000);
                thread::spawn(move || {
                    for i in 0..20 {
                        assert_eq!(
                            hr0(&mut d).unwrap(),
                            Value::Int(i64::from(u) * 100),
                            "unit {u} read {i}"
                        );
                        d.write("hr:1", Value::Int(i64::from(u))).unwrap();
                    }
                    d
                })
            })
            .collect();
        let drivers: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        for u in 1..=8u8 {
            assert_eq!(s.unit(u).holding[1], u16::from(u), "{cap:?}: unit {u}'s write");
            assert_eq!(s.unit(u).requests, 40, "{cap:?}: unit {u}");
        }
        assert_eq!(s.accepted(), 1, "{cap:?}: one connection for eight units");
        assert_eq!(s.refused(), 0, "{cap:?}");
        assert_eq!(s.open_connections(), 1);
        drop(drivers);
        wait(
            "the last unit to let go closes it",
            Duration::from_secs(5),
            || s.open_connections() == 0,
        );
    }
}

#[test]
fn the_simulated_cap_refuses_or_closes_the_oldest() {
    let s = gateway([1], Some((1, Cap::Refuse)));
    let _first = TcpStream::connect(s.addr).unwrap();
    wait("the first is taken", Duration::from_secs(5), || s.accepted() == 1);
    let mut second = TcpStream::connect(s.addr).unwrap();
    second.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert!(
        matches!(second.read(&mut [0u8; 1]), Ok(0) | Err(_)),
        "the second is closed unanswered"
    );
    assert_eq!((s.accepted(), s.refused()), (1, 1));

    let s = gateway([1], Some((1, Cap::CloseOldest)));
    let mut first = TcpStream::connect(s.addr).unwrap();
    first.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    wait("the first is taken", Duration::from_secs(5), || s.accepted() == 1);
    let _second = TcpStream::connect(s.addr).unwrap();
    assert!(
        matches!(first.read(&mut [0u8; 1]), Ok(0) | Err(_)),
        "the oldest is closed"
    );
    assert_eq!((s.accepted(), s.refused()), (2, 0));
}

#[test]
fn a_silent_unit_stalls_the_others_by_at_most_its_own_timeout() {
    let s = gateway([3, 4], Some((1, Cap::Refuse)));
    let (mut dead, mut live) = (driver(&s, 3, 100), driver(&s, 4, 3000));
    assert_eq!(hr0(&mut dead).unwrap(), Value::Int(300));
    assert_eq!(hr0(&mut live).unwrap(), Value::Int(400));
    s.mute(3, true);
    let poller = thread::spawn(move || {
        // More timeouts than the connection remembers (MAX_ABANDONED).
        for _ in 0..modbus::MAX_ABANDONED + 4 {
            let t = Instant::now();
            let e = hr0(&mut dead).unwrap_err();
            assert!(timed_out(&e) && e.to_string().contains("unit 3"), "{e}");
            assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        }
        dead
    });
    let mut slowest = Duration::ZERO;
    while !poller.is_finished() {
        let t = Instant::now();
        assert_eq!(hr0(&mut live).unwrap(), Value::Int(400), "unit 4 stays up");
        slowest = slowest.max(t.elapsed());
    }
    let mut dead = poller.join().unwrap();
    // Unit 4 waits for unit 3's request (100 ms) at worst, never its own
    // 3 s timeout.
    assert!(slowest < Duration::from_millis(1500), "{slowest:?}");
    assert_eq!(s.accepted(), 1, "a silent unit does not cost a reconnect");
    s.mute(3, false);
    assert_eq!(
        hr0(&mut dead).unwrap(),
        Value::Int(300),
        "and is back once it answers"
    );
}

#[test]
fn a_late_reply_is_skipped_not_given_to_the_next_unit() {
    let s = gateway([3, 4], None);
    let (mut slow, mut next) = (driver(&s, 3, 100), driver(&s, 4, 3000));
    assert_eq!(hr0(&mut slow).unwrap(), Value::Int(300));
    assert_eq!(hr0(&mut next).unwrap(), Value::Int(400));
    s.unit(3).faults.push(Fault::Delay(Duration::from_millis(400)));
    assert!(timed_out(&hr0(&mut slow).unwrap_err()));
    // Unit 3's reply (300) comes first on the stream, then unit 4's.
    assert_eq!(hr0(&mut next).unwrap(), Value::Int(400));
    assert_eq!(hr0(&mut slow).unwrap(), Value::Int(300));
    assert_eq!(s.accepted(), 1);
}

#[test]
fn a_reply_with_a_transaction_never_sent_closes_the_connection() {
    let s = gateway([3, 4], None);
    let (mut a, mut b) = (driver(&s, 3, 1000), driver(&s, 4, 1000));
    assert_eq!(hr0(&mut a).unwrap(), Value::Int(300));
    s.unit(3).faults.push(Fault::WrongTransaction);
    assert!(matches!(hr0(&mut a), Err(Error::Protocol(_))));
    assert_eq!(hr0(&mut b).unwrap(), Value::Int(400), "a fresh connection");
    assert_eq!(hr0(&mut a).unwrap(), Value::Int(300));
    assert_eq!(s.accepted(), 2);
}

#[test]
fn one_units_exception_leaves_the_others_and_the_connection_alone() {
    let s = gateway([3, 4], None);
    let (mut a, mut b) = (driver(&s, 3, 1000), driver(&s, 4, 1000));
    s.unit(3).faults.push(Fault::DeviceFailure);
    assert!(matches!(
        hr0(&mut a),
        Err(Error::Exception(modbus::SERVER_DEVICE_FAILURE))
    ));
    assert!(matches!(
        a.read(&["hr:8"]),
        Err(Error::Exception(modbus::ILLEGAL_DATA_ADDRESS))
    ));
    assert_eq!(hr0(&mut b).unwrap(), Value::Int(400));
    assert_eq!(hr0(&mut a).unwrap(), Value::Int(300));
    assert_eq!(s.accepted(), 1);
}

#[test]
fn a_dropped_connection_is_opened_again_once_for_every_unit() {
    let s = gateway(1..=8, Some((1, Cap::Refuse)));
    let mut ds: Vec<_> = (1..=8u8).map(|u| driver(&s, u, 1000)).collect();
    for d in &mut ds {
        hr0(d).unwrap();
    }
    s.drop_connections();
    let mut errors = 0;
    for (u, d) in (1..=8u8).zip(&mut ds) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match hr0(d) {
                Ok(v) => {
                    assert_eq!(v, Value::Int(i64::from(u) * 100));
                    break;
                }
                Err(e) => {
                    assert!(e.is_link(), "{e}");
                    errors += 1;
                    assert!(Instant::now() < until, "unit {u} never came back: {e}");
                }
            }
        }
    }
    assert_eq!(errors, 1, "only the unit that met the closed socket saw it");
    assert_eq!(s.accepted(), 2, "one reconnect for eight units");
    assert_eq!(s.refused(), 0);
}

#[test]
fn a_gateway_that_answers_no_unit_is_reconnected() {
    // Every unit on the line timing out is the one sign a half-open
    // socket gives: the connection is closed and the next request
    // opens another.
    let s = gateway([3, 4], None);
    let (mut a, mut b) = (driver(&s, 3, 100), driver(&s, 4, 100));
    assert_eq!(hr0(&mut a).unwrap(), Value::Int(300));
    assert_eq!(hr0(&mut b).unwrap(), Value::Int(400));
    s.mute(3, true);
    s.mute(4, true);
    assert!(timed_out(&hr0(&mut a).unwrap_err()));
    assert!(timed_out(&hr0(&mut b).unwrap_err()));
    s.mute(3, false);
    s.mute(4, false);
    assert_eq!(hr0(&mut a).unwrap(), Value::Int(300));
    assert_eq!(hr0(&mut b).unwrap(), Value::Int(400));
    assert_eq!(s.accepted(), 2);
}

#[test]
fn different_gateways_get_their_own_connections() {
    let (g1, g2) = (gateway([1], None), gateway([1], None));
    let (mut a, mut b) = (driver(&g1, 1, 1000), driver(&g2, 1, 1000));
    hr0(&mut a).unwrap();
    hr0(&mut b).unwrap();
    assert_eq!((g1.accepted(), g2.accepted()), (1, 1));
}
