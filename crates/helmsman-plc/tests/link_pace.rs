//! Timing by itself (PROTOCOLS.md step 4): a device by its profile with no
//! `poll_ms` polls at the fastest period of the modules that read or
//! drive its signals, inside the profile's limits; a typed `poll_ms`
//! wins; and a link measures its own update period on the `cycle_at`
//! clock and says healthy, slow or lost. Scripted clocks, no sleeps
//! before an assertion.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::driver::{Driver, Error, PointInfo};
use helmsman_io::local::Channels;
use helmsman_io::{Ty, Value};
use helmsman_plc::drivers::Drivers;
use helmsman_plc::layout::Layout;
use helmsman_plc::link::{self, DEFAULT_POLL, Health, LinkReport, LinkSet, Pace, SAMPLES, links};
use helmsman_plc::{Module, Window};

/// A profile with limits on its first transport and none on its second.
const PROFILE: &str = "helmsman-device 1
vendor Test
model  Block
kind   remote-io
transport modbus-tcp cycle_ms=20..500 unit=1
transport modbus-rtu unit=1
in  DI-00 bool modbus-tcp=di:0 modbus-rtu=di:0
in  DI-01 bool modbus-tcp=di:1 modbus-rtu=di:1
out DO-00 bool modbus-tcp=coil:0 modbus-rtu=coil:0
";

fn drivers() -> Drivers {
    BTreeMap::from([("block.device".to_string(), PROFILE.as_bytes().to_vec())])
}

/// Answers every read at once, or fails every read after `fail_after`.
struct Stub {
    reads: Arc<AtomicUsize>,
    fail_after: Option<usize>,
}

impl Driver for Stub {
    fn parse(&self, _: &str) -> Result<PointInfo, Error> {
        Ok(PointInfo {
            ty: Ty::Bool,
            writable: true,
        })
    }
    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_after.is_some_and(|k| n >= k) {
            return Err(Error::Io(std::io::Error::other("gone")));
        }
        Ok(vec![Value::Bool(true); addrs.len()])
    }
    fn write(&mut self, _: &str, _: Value) -> Result<(), Error> {
        Ok(())
    }
}

fn module(name: &str, src: &str, period_ms: u64) -> Module {
    let program = compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap();
    Module {
        name: name.into(),
        program: program.into(),
        period: Duration::from_millis(period_ms),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

#[test]
fn the_pace_is_each_signals_fastest_module_by_name() {
    let fast = module(
        "fast",
        "PROGRAM fast VAR_INPUT part : BOOL; END_VAR VAR_OUTPUT clamp : BOOL; END_VAR clamp := part; END_PROGRAM",
        5,
    );
    let slow = module(
        "slow",
        "PROGRAM slow VAR_INPUT Part : BOOL; other : BOOL; END_VAR END_PROGRAM",
        200,
    );
    let pace = link::pace([&fast, &slow]);
    assert_eq!(
        pace["PART"],
        Duration::from_millis(5),
        "the faster reader, case-blind"
    );
    assert_eq!(pace["CLAMP"], Duration::from_millis(5));
    assert_eq!(pace["OTHER"], Duration::from_millis(200));
    assert_eq!(pace.len(), 3, "{pace:?}");
}

type Reports = Arc<Mutex<Vec<LinkReport>>>;

/// Starts `text`'s links on stub drivers, paced by `pace`: the reports as
/// they come.
fn start(text: &str, pace: &Pace, fail_after: Option<usize>) -> (LinkSet, Reports) {
    let layout = Layout::build_with(Some(text), None, &drivers()).unwrap();
    let table = Arc::new(Mutex::new(layout.table));
    let reports: Reports = Arc::default();
    let r = Arc::clone(&reports);
    let reads = Arc::new(AtomicUsize::new(0));
    let mut set = LinkSet::new(
        table,
        Arc::new(move |_d: &helmsman_io::Device| {
            Ok(Box::new(Stub {
                reads: Arc::clone(&reads),
                fail_after,
            }) as Box<dyn Driver + Send>)
        }),
        Arc::new(|| Box::new(Channels::new())),
        Arc::new(move |l: &LinkReport| {
            r.lock().unwrap_or_else(PoisonError::into_inner).push(l.clone());
        }),
    );
    let plan = set
        .plan_paced(layout.map.as_ref().unwrap(), &drivers(), pace)
        .unwrap();
    let stopped = set.stop(&plan);
    set.finish(plan, stopped);
    (set, reports)
}

/// The first report of link `name`, within 5 s.
fn first(reports: &Reports, name: &str) -> LinkReport {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|r| r.name == name)
            .cloned();
        match got {
            Some(r) => return r,
            None if Instant::now() > deadline => panic!("{name}: no report within 5 s"),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

#[test]
fn a_profile_device_polls_at_its_modules_pace_inside_the_profiles_limits() {
    let text = "device blk device:block 127.0.0.1\nin part bool blk ch:DI-00\nout clamp bool blk ch:DO-00\n";
    // Two modules touch its signals: the faster one paces the link, but
    // not faster than the transport allows.
    let pace: Pace = [("PART", 5u64), ("CLAMP", 100)]
        .into_iter()
        .map(|(n, ms)| (n.to_string(), Duration::from_millis(ms)))
        .collect();
    let (mut set, reports) = start(text, &pace, None);
    let r = first(&reports, "blk");
    assert_eq!(
        r.poll,
        Duration::from_millis(20),
        "5 ms clamped to the profile's 20..500"
    );
    assert_eq!(r.error, None);
    set.shutdown();

    // Only a slow module reads it: its period, under the ceiling.
    let pace: Pace = [("PART".to_string(), Duration::from_millis(250))]
        .into_iter()
        .collect();
    let (mut set, reports) = start(text, &pace, None);
    assert_eq!(first(&reports, "blk").poll, Duration::from_millis(250));
    set.shutdown();

    // Nothing reads it (a signal no module names): the default.
    let (mut set, reports) = start(text, &Pace::new(), None);
    assert_eq!(first(&reports, "blk").poll, DEFAULT_POLL);
    set.shutdown();

    // A typed poll_ms wins, limits or not.
    let typed = "device blk device:block 127.0.0.1 poll_ms=7\nin part bool blk ch:DI-00\n";
    let (mut set, reports) = start(typed, &pace, None);
    assert_eq!(first(&reports, "blk").poll, Duration::from_millis(7));
    set.shutdown();

    // A device that is not by its profile keeps the default: its pace is
    // its line's business, as ever.
    let plain = "device dev modbus-tcp 127.0.0.1:1\nin part bool dev di:0\n";
    let (mut set, reports) = start(plain, &pace, None);
    assert_eq!(first(&reports, "dev").poll, DEFAULT_POLL);
    set.shutdown();
}

#[test]
fn a_changed_pace_replaces_the_link_and_the_same_pace_keeps_it() {
    let text = "device blk device:block 127.0.0.1\nin part bool blk ch:DI-00\n";
    let pace = |ms: u64| -> Pace {
        [("PART".to_string(), Duration::from_millis(ms))]
            .into_iter()
            .collect()
    };
    let (mut set, _reports) = start(text, &pace(100), None);
    let map = Layout::build_with(Some(text), None, &drivers())
        .unwrap()
        .map
        .unwrap();
    assert!(set.plan_paced(&map, &drivers(), &pace(100)).unwrap().is_noop());
    let plan = set.plan_paced(&map, &drivers(), &pace(300)).unwrap();
    assert_eq!(plan.stops(), ["blk"]);
    assert_eq!(plan.starts(), ["blk"]);
    set.shutdown();
}

/// One link over a stub driver, driven by hand on the `cycle_at` clock.
fn one_link(fail_after: Option<usize>) -> (helmsman_plc::link::Link, Mutex<helmsman_plc::SignalTable>) {
    let text = "device dev modbus-tcp 127.0.0.1:1 poll_ms=50\nin x bool dev di:0\n";
    let layout = Layout::build(Some(text), None).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let mut links = links(
        layout.map.as_ref().unwrap(),
        |_d| {
            Ok(Box::new(Stub {
                reads: Arc::clone(&reads),
                fail_after,
            }) as Box<dyn Driver + Send>)
        },
        Box::new(Channels::new()),
    )
    .unwrap();
    let link = links.remove(links.iter().position(|l| l.name == "dev").unwrap());
    (link, Mutex::new(layout.table))
}

#[test]
fn a_link_measures_its_update_period_and_says_healthy_slow_or_lost() {
    let (mut link, table) = one_link(None);
    let ms = Duration::from_millis;
    assert_eq!(link.health(), Health::Lost, "nothing has answered yet");
    // On time: healthy, and the number says so once it has its samples.
    let mut now = ms(0);
    for k in 0..=SAMPLES {
        if k > 0 {
            now += ms(50);
        }
        link.cycle_at(&table, now).unwrap();
        if k < SAMPLES {
            assert_eq!(link.measured(), None, "cycle {k}: not enough samples yet");
        }
    }
    assert_eq!(link.measured(), Some(ms(50)));
    assert_eq!(link.health(), Health::Healthy);
    // One late cycle among eight is not slow: the period is a mean.
    now += ms(101);
    link.cycle_at(&table, now).unwrap();
    assert_eq!(link.health(), Health::Healthy, "one late cycle among eight");
    // Twice the period asked is still not slow; more than twice is.
    for _ in 0..SAMPLES {
        now += ms(100);
        link.cycle_at(&table, now).unwrap();
    }
    assert_eq!(link.measured(), Some(ms(100)));
    assert_eq!(link.health(), Health::Healthy, "exactly twice");
    for _ in 0..SAMPLES {
        now += ms(150);
        link.cycle_at(&table, now).unwrap();
    }
    assert_eq!(link.measured(), Some(ms(150)));
    assert_eq!(link.health(), Health::Slow);
    // Back on time: healthy again once the slow samples have rolled out.
    for _ in 0..SAMPLES {
        now += ms(50);
        link.cycle_at(&table, now).unwrap();
    }
    assert_eq!(link.health(), Health::Healthy);
    let r = link.report(None);
    assert_eq!(
        (r.health, r.measured, r.poll),
        (Health::Healthy, Some(ms(50)), ms(50))
    );
}

#[test]
fn a_lost_link_forgets_its_samples_and_measures_afresh_when_it_is_back() {
    let (mut link, table) = one_link(Some(SAMPLES + 1));
    let ms = Duration::from_millis;
    let mut now = ms(0);
    for _ in 0..=SAMPLES {
        link.cycle_at(&table, now).unwrap();
        now += ms(50);
    }
    assert_eq!(link.health(), Health::Healthy);
    let e = link.cycle_at(&table, now).unwrap_err();
    assert!(e.is_link(), "{e}");
    assert_eq!(link.health(), Health::Lost);
    assert_eq!(link.measured(), None, "a down link has no period");
    let r = link.report(Some(e.to_string()));
    assert_eq!(
        (r.health, r.measured, r.error.as_deref()),
        (Health::Lost, None, Some("io: gone"))
    );
}

/// Reads take longer than twice the poll: what a slow device looks like
/// on a real thread.
struct Sluggish;

impl Driver for Sluggish {
    fn parse(&self, _: &str) -> Result<PointInfo, Error> {
        Ok(PointInfo {
            ty: Ty::Bool,
            writable: false,
        })
    }
    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        std::thread::sleep(Duration::from_millis(30));
        Ok(vec![Value::Bool(true); addrs.len()])
    }
    fn write(&mut self, _: &str, _: Value) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
fn a_running_link_reports_slow_once_and_healthy_once() {
    let text = "device dev modbus-tcp 127.0.0.1:1 poll_ms=5\nin x bool dev di:0\n";
    let layout = Layout::build(Some(text), None).unwrap();
    let table = Arc::new(Mutex::new(layout.table));
    let reports: Reports = Arc::default();
    let r = Arc::clone(&reports);
    let mut set = LinkSet::new(
        table,
        Arc::new(|_d: &helmsman_io::Device| Ok(Box::new(Sluggish) as Box<dyn Driver + Send>)),
        Arc::new(|| Box::new(Channels::new())),
        Arc::new(move |l: &LinkReport| {
            r.lock().unwrap_or_else(PoisonError::into_inner).push(l.clone());
        }),
    );
    set.start(layout.map.as_ref().unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let seen = loop {
        let seen: Vec<(Option<String>, Health)> = reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|l| l.name == "dev")
            .map(|l| (l.error.clone(), l.health))
            .collect();
        if seen.len() >= 2 || Instant::now() > deadline {
            break seen;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    set.shutdown();
    assert_eq!(
        seen,
        [(None, Health::Healthy), (None, Health::Slow)],
        "up first (no samples yet), then slow once its samples are in, and nothing more"
    );
}
