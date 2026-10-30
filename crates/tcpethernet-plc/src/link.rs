//! Device links: one per device in the I/O map, plus one for the box's own
//! channels. Each [`Link::cycle`] writes the output commands that changed
//! since the device last took them, then reads every input. Any failure
//! marks the link down: its inputs go missing (so the modules reading them
//! are held and their outputs go safe) and it forgets what the device holds,
//! so every output is written again once the device answers.
//!
//! [`spawn`] runs a link on its own thread at its poll period; tests call
//! [`Link::cycle`] directly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tcpethernet_io::{Dir, Driver, Error, IoMap, Value};

use crate::signals::{SignalTable, by_device};

/// Poll period when a device sets no `poll_ms`.
pub const DEFAULT_POLL: Duration = Duration::from_millis(50);

struct Out {
    signal: usize,
    addr: String,
    /// What the device holds, as far as the link knows.
    written: Option<Value>,
}

/// One device's inputs and outputs, and the driver that reaches it.
pub struct Link {
    /// The device id, or `box` for the box's own channels.
    pub name: String,
    pub poll: Duration,
    driver: Box<dyn Driver + Send>,
    inputs: Vec<(usize, String)>,
    outputs: Vec<Out>,
    up: bool,
}

fn lock(t: &Mutex<SignalTable>) -> MutexGuard<'_, SignalTable> {
    t.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Link {
    /// Whether the last cycle succeeded.
    #[must_use]
    pub const fn is_up(&self) -> bool {
        self.up
    }

    /// One poll: changed outputs out, every input in.
    ///
    /// # Errors
    /// The first driver error; the link is down until a cycle succeeds.
    pub fn cycle(&mut self, table: &Mutex<SignalTable>) -> Result<(), Error> {
        let r = self.exchange(table);
        if r.is_err() {
            self.up = false;
            for o in &mut self.outputs {
                o.written = None;
            }
            let mut t = lock(table);
            for (i, _) in &self.inputs {
                t.set_input(*i, None);
            }
        } else {
            self.up = true;
        }
        r
    }

    fn exchange(&mut self, table: &Mutex<SignalTable>) -> Result<(), Error> {
        let due: Vec<(usize, Value)> = {
            let t = lock(table);
            self.outputs
                .iter()
                .enumerate()
                .filter_map(|(k, o)| {
                    let cmd = t.command(o.signal)?;
                    (o.written != Some(cmd)).then_some((k, cmd))
                })
                .collect()
        };
        for (k, cmd) in due {
            let o = &mut self.outputs[k];
            self.driver.write(&o.addr, cmd)?;
            o.written = Some(cmd);
        }
        if self.inputs.is_empty() {
            return Ok(());
        }
        let addrs: Vec<&str> = self.inputs.iter().map(|(_, a)| a.as_str()).collect();
        let values = self.driver.read(&addrs)?;
        let mut t = lock(table);
        for ((i, _), v) in self.inputs.iter().zip(values) {
            t.set_input(*i, Some(v));
        }
        Ok(())
    }
}

/// Builds a link per device that has signals, plus `box` for local
/// channels, with drivers from `open` (`tcpethernet_io::open` in the
/// runtime) and `channels` for the box. Signal indices are map order, the
/// same as [`SignalTable::new`]'s.
///
/// # Errors
/// A device whose driver will not open, or whose `poll_ms` is not
/// 1..=60000, with the device id.
pub fn links(
    map: &IoMap,
    mut open: impl FnMut(&tcpethernet_io::Device) -> Result<Box<dyn Driver + Send>, Error>,
    channels: Box<dyn Driver + Send>,
) -> Result<Vec<Link>, String> {
    let mut channels = Some(channels);
    let mut out = Vec::new();
    for (dev, sigs) in by_device(map) {
        let (name, poll, driver) = match dev {
            None => (
                "box".to_string(),
                DEFAULT_POLL,
                channels.take().expect("one local group"),
            ),
            Some(id) => {
                let d = map
                    .devices
                    .iter()
                    .find(|d| d.id == id)
                    .ok_or_else(|| format!("{id}: no such device"))?;
                let poll = match d.params.get("poll_ms").map(|p| p.parse::<u64>()) {
                    None => DEFAULT_POLL,
                    Some(Ok(ms @ 1..=60_000)) => Duration::from_millis(ms),
                    Some(_) => return Err(format!("{id}: poll_ms must be 1..=60000")),
                };
                (id, poll, open(d).map_err(|e| format!("{}: {e}", d.id))?)
            }
        };
        let mut link = Link {
            name,
            poll,
            driver,
            inputs: Vec::new(),
            outputs: Vec::new(),
            up: false,
        };
        for (signal, addr, dir) in sigs {
            match dir {
                Dir::In => link.inputs.push((signal, addr)),
                Dir::Out => link.outputs.push(Out {
                    signal,
                    addr,
                    written: None,
                }),
            }
        }
        out.push(link);
    }
    Ok(out)
}

/// Runs `link` on its own thread until `stop` is set, calling `report` on
/// every change between up and down (with the error when it went down).
pub fn spawn(
    mut link: Link,
    table: Arc<Mutex<SignalTable>>,
    stop: Arc<AtomicBool>,
    report: impl Fn(&str, Option<&Error>) + Send + 'static,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut was_up = None;
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let r = link.cycle(&table);
            if was_up != Some(r.is_ok()) {
                report(&link.name, r.as_ref().err());
                was_up = Some(r.is_ok());
            }
            next += link.poll;
            let now = Instant::now();
            match next.checked_duration_since(now) {
                Some(wait) => thread::sleep(wait),
                None => next = now,
            }
        }
    })
}
