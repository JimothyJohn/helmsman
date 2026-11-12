//! What a link reports while it is down (2026-11-07): the try box lost the
//! E1213, and every reconnect then failed for another reason ("No route to
//! host", macOS Local Network privacy), but the Devices tab and the log kept
//! the first error ("no T->O data") because a link reported only a change
//! between up and down. A down link now reports each new reason once; the
//! same reason again stays quiet.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helmsman_io::driver::{Driver, Error, PointInfo};
use helmsman_io::{Ty, Value};
use helmsman_plc::layout::Layout;
use helmsman_plc::link::LinkSet;

/// Fails every read with the reasons in order, then answers.
struct Flaky {
    reads: Arc<AtomicUsize>,
    reasons: Vec<&'static str>,
}

impl Driver for Flaky {
    fn parse(&self, _addr: &str) -> Result<PointInfo, Error> {
        Ok(PointInfo {
            ty: Ty::Bool,
            writable: false,
        })
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst);
        match self.reasons.get(n) {
            Some(r) => Err(Error::Io(std::io::Error::other(*r))),
            None => Ok(vec![Value::Bool(true); addrs.len()]),
        }
    }

    fn write(&mut self, _addr: &str, _value: Value) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
fn a_down_link_reports_each_new_reason_once() {
    let text = "device dev modbus-tcp 127.0.0.1:1 poll_ms=5\nin x bool dev di:0\n";
    let layout = Layout::build(Some(text), None).unwrap();
    let table = Arc::new(Mutex::new(layout.table.clone()));
    let reads = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&reads);
    let said: Arc<Mutex<Vec<String>>> = Arc::default();
    let s = Arc::clone(&said);
    let mut set = LinkSet::new(
        table,
        Arc::new(move |_d: &helmsman_io::Device| {
            Ok(Box::new(Flaky {
                reads: Arc::clone(&r),
                reasons: vec![
                    "no T->O data",
                    "no route to host",
                    "no route to host",
                    "no route to host",
                ],
            }) as Box<dyn Driver + Send>)
        }),
        Arc::new(|| Box::new(helmsman_io::local::Channels::new())),
        Arc::new(move |r: &helmsman_plc::link::LinkReport| {
            if r.name == "dev" {
                let line = r.error.clone().unwrap_or_else(|| "up".to_string());
                s.lock().unwrap_or_else(PoisonError::into_inner).push(line);
            }
        }),
    );
    set.start(layout.map.as_ref().unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let lines = loop {
        let lines = said.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if lines.last().is_some_and(|l| l == "up") || Instant::now() > deadline {
            break lines;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    set.shutdown();
    assert_eq!(lines, ["io: no T->O data", "io: no route to host", "up"]);
}
