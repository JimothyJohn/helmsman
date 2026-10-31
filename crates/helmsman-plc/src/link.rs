//! Device links: one per device in the I/O map, plus one for the box's own
//! channels. Each [`Link::cycle`] writes the output commands that changed
//! since the device last took them, then reads every input.
//!
//! A *link* error (I/O, a reply that isn't valid; [`Error::is_link`]) marks
//! the link down: its inputs go missing (so the modules reading them are
//! held and their outputs go safe) and it forgets what the device holds, so
//! every output is written again once the device answers. A *point* error
//! (a value the point can't hold, a device exception, an unknown tag)
//! stays on that point: it is recorded on the signal
//! ([`SignalTable::point_error`]), a refused output is retried next cycle,
//! a refused input goes missing alone, and the link stays up.
//!
//! An input may have its own poll period (`poll_ms=` on its map line);
//! the link then cycles at the fastest of its device's and its inputs'
//! periods and [`Link::cycle_at`] reads only the inputs that are due.
//! Outputs are written on every cycle in which they changed.
//!
//! A link knows its points by signal name, device and address, and finds
//! their table indices again whenever the table's layout changes
//! ([`SignalTable::epoch`]): a point the table no longer binds to exactly
//! this device and address is neither written nor read. So a link that a
//! reload keeps runs on across it untouched, and no link can ever write a
//! command to a point the running map does not bind it to.
//!
//! [`spawn`] runs a link on its own thread at its poll period; tests call
//! [`Link::cycle`] / [`Link::cycle_at`] directly. [`LinkSet`] owns the
//! running links and changes them for a new map (see its docs).

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_io::{Device, Dir, Driver, Error, IoMap, Value};

use crate::drivers::{self, Drivers};
use crate::signals::{Polled, SignalTable, by_device};

/// Poll period when a device sets no `poll_ms`.
pub const DEFAULT_POLL: Duration = Duration::from_millis(50);

/// The link name of the box's own channels.
pub const BOX: &str = helmsman_io::text::BOX;

struct Out {
    name: String,
    addr: String,
    /// Its index in the table the link last looked at; `None`: the table
    /// does not bind it here.
    signal: Option<usize>,
    /// What the device holds, as far as the link knows.
    written: Option<Value>,
}

struct In {
    name: String,
    addr: String,
    signal: Option<usize>,
    poll: Duration,
    /// When it is next due, on the [`Link::cycle_at`] clock.
    next: Duration,
}

/// One device's inputs and outputs, and the driver that reaches it.
pub struct Link {
    /// The device id, or `box` for the box's own channels.
    pub name: String,
    /// How often the link cycles: the fastest of the device's poll period
    /// and its inputs' own.
    pub poll: Duration,
    driver: Box<dyn Driver + Send>,
    inputs: Vec<In>,
    outputs: Vec<Out>,
    up: bool,
    /// The table layout the indices above are for.
    epoch: Option<u64>,
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

    /// One poll: changed outputs out, every input in, whatever its period.
    ///
    /// # Errors
    /// The first driver error; the link is down until a cycle succeeds.
    pub fn cycle(&mut self, table: &Mutex<SignalTable>) -> Result<(), Error> {
        self.run(table, None)
    }

    /// One poll at `now` (time since the link started): changed outputs
    /// out, then the inputs whose period has come round. An input read at
    /// `now` is next due one period later; a late cycle skips the periods
    /// it missed rather than bursting.
    ///
    /// # Errors
    /// As [`Link::cycle`]; after an error every input is due again.
    pub fn cycle_at(&mut self, table: &Mutex<SignalTable>, now: Duration) -> Result<(), Error> {
        self.run(table, Some(now))
    }

    /// Looks every point up again if `t` is another layout than the one
    /// the link last saw: by name, and only where `t` binds that name to
    /// this device, this address and this direction.
    fn sync(&mut self, t: &SignalTable) {
        if self.epoch == Some(t.epoch()) {
            return;
        }
        self.epoch = Some(t.epoch());
        let dev = self.name.as_str();
        let find = |name: &str, addr: &str, dir: Dir| {
            t.index(name)
                .filter(|&i| t.point(i) == Some((dev, addr)) && t.signal(i).map(|s| s.0) == Some(dir))
        };
        for o in &mut self.outputs {
            o.signal = find(&o.name, &o.addr, Dir::Out);
        }
        for i in &mut self.inputs {
            i.signal = find(&i.name, &i.addr, Dir::In);
        }
    }

    fn run(&mut self, table: &Mutex<SignalTable>, now: Option<Duration>) -> Result<(), Error> {
        let r = self.exchange(table, now);
        if r.is_err() {
            self.up = false;
            for o in &mut self.outputs {
                o.written = None;
            }
            let mut t = lock(table);
            self.sync(&t);
            for i in &mut self.inputs {
                if let Some(s) = i.signal {
                    t.set_input(s, None);
                }
                i.next = Duration::ZERO;
            }
        } else {
            self.up = true;
        }
        r
    }

    /// Records a point error (or clears it) on output or input `name`,
    /// wherever the table now has it.
    fn note(
        &mut self,
        table: &Mutex<SignalTable>,
        output: Option<usize>,
        input: Option<usize>,
        e: Option<String>,
    ) {
        let mut t = lock(table);
        self.sync(&t);
        let s = match (output, input) {
            (Some(k), _) => self.outputs[k].signal,
            (_, Some(k)) => self.inputs[k].signal,
            _ => None,
        };
        if let Some(s) = s {
            t.set_point_error(s, e);
        }
    }

    fn exchange(&mut self, table: &Mutex<SignalTable>, now: Option<Duration>) -> Result<(), Error> {
        let due: Vec<(usize, Value)> = {
            let t = lock(table);
            self.sync(&t);
            self.outputs
                .iter()
                .enumerate()
                .filter_map(|(k, o)| {
                    let cmd = t.command(o.signal?)?;
                    (o.written != Some(cmd)).then_some((k, cmd))
                })
                .collect()
        };
        for (k, cmd) in due {
            match self.driver.write(&self.outputs[k].addr, cmd) {
                Ok(()) => {
                    self.outputs[k].written = Some(cmd);
                    self.note(table, Some(k), None, None);
                }
                Err(e) if !e.is_link() => {
                    // Left unwritten, so it is tried again next cycle.
                    self.note(table, Some(k), None, Some(e.to_string()));
                }
                Err(e) => return Err(e),
            }
        }
        let due: Vec<usize> = (0..self.inputs.len())
            .filter(|&k| now.is_none_or(|now| self.inputs[k].next <= now))
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        let addrs: Vec<&str> = due.iter().map(|&k| self.inputs[k].addr.as_str()).collect();
        let values: Vec<Result<Value, String>> = match self.driver.read(&addrs) {
            Ok(vs) => vs.into_iter().map(Ok).collect(),
            Err(e) if !e.is_link() => {
                // A read is all or nothing, so find the point(s) that
                // refused by reading each due input on its own.
                let mut each = Vec::with_capacity(addrs.len());
                for a in &addrs {
                    match self.driver.read(&[a]) {
                        Ok(mut v) if v.len() == 1 => each.push(Ok(v.remove(0))),
                        Ok(_) => return Err(Error::Protocol(format!("{a}: read returned the wrong count"))),
                        Err(e) if !e.is_link() => each.push(Err(e.to_string())),
                        Err(e) => return Err(e),
                    }
                }
                each
            }
            Err(e) => return Err(e),
        };
        let mut t = lock(table);
        self.sync(&t);
        for (&k, v) in due.iter().zip(values) {
            let i = &mut self.inputs[k];
            if let Some(s) = i.signal {
                match v {
                    Ok(v) => {
                        t.set_input(s, Some(v));
                        t.set_point_error(s, None);
                    }
                    Err(e) => {
                        t.set_input(s, None);
                        t.set_point_error(s, Some(e));
                    }
                }
            }
            if let Some(now) = now {
                i.next = now.saturating_add(i.poll);
            }
        }
        Ok(())
    }

    /// Writes `value` to output point `addr` directly, whatever the table
    /// says: how a link stopped for good sends its outputs to safe.
    fn force(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        self.driver.write(addr, value)
    }
}

/// The poll period a device's `poll_ms` param asks for.
fn device_poll(d: &Device) -> Result<Duration, String> {
    match d.params.get("poll_ms").map(|p| p.parse::<u64>()) {
        None => Ok(DEFAULT_POLL),
        Some(Ok(ms @ 1..=60_000)) => Ok(Duration::from_millis(ms)),
        Some(_) => Err(format!("{}: poll_ms must be 1..=60000", d.id)),
    }
}

/// One link over `sigs` with `driver`; indices are found on first use.
fn build(
    name: String,
    poll: Duration,
    driver: Box<dyn Driver + Send>,
    map: &IoMap,
    sigs: Vec<Polled>,
) -> Link {
    let mut link = Link {
        name,
        poll,
        driver,
        inputs: Vec::new(),
        outputs: Vec::new(),
        up: false,
        epoch: None,
    };
    for Polled {
        signal,
        addr,
        dir,
        poll: own,
    } in sigs
    {
        let name = map.signals[signal].name.clone();
        match dir {
            Dir::In => {
                let p = own.unwrap_or(poll);
                link.poll = link.poll.min(p);
                link.inputs.push(In {
                    name,
                    addr,
                    signal: None,
                    poll: p,
                    next: Duration::ZERO,
                });
            }
            Dir::Out => link.outputs.push(Out {
                name,
                addr,
                signal: None,
                written: None,
            }),
        }
    }
    link
}

/// Builds a link per device that has signals, plus `box` for local
/// channels, with drivers from `open` (`helmsman_io::open` in the
/// runtime) and `channels` for the box. Each finds its signals in the
/// table by name, device and address (a table built from `map` by
/// [`SignalTable::new`]).
///
/// # Errors
/// A device whose driver will not open, or whose `poll_ms` is not
/// 1..=60000, with the device id. (A signal's own poll period is checked
/// by [`IoMap::validate`].)
pub fn links(
    map: &IoMap,
    mut open: impl FnMut(&helmsman_io::Device) -> Result<Box<dyn Driver + Send>, Error>,
    channels: Box<dyn Driver + Send>,
) -> Result<Vec<Link>, String> {
    let mut channels = Some(channels);
    let mut out = Vec::new();
    for (dev, sigs) in by_device(map) {
        let (name, poll, driver) = match dev {
            None => (
                BOX.to_string(),
                DEFAULT_POLL,
                channels.take().expect("one local group"),
            ),
            Some(id) => {
                let d = map
                    .devices
                    .iter()
                    .find(|d| d.id == id)
                    .ok_or_else(|| format!("{id}: no such device"))?;
                let poll = device_poll(d)?;
                (id, poll, open(d).map_err(|e| format!("{}: {e}", d.id))?)
            }
        };
        out.push(build(name, poll, driver, map, sigs));
    }
    Ok(out)
}

/// Waits until `until` or until `stop` is set (and the thread unparked).
fn nap(until: Instant, stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        match until.checked_duration_since(Instant::now()) {
            Some(d) if !d.is_zero() => thread::park_timeout(d),
            _ => return,
        }
    }
}

/// Runs `link` on its own thread until `stop` is set, calling `report` on
/// every change between up and down (with the error when it went down).
/// The thread hands the link back when it ends: set `stop`, unpark the
/// thread (so a slow poll does not have to run out) and join.
pub fn spawn(
    mut link: Link,
    table: Arc<Mutex<SignalTable>>,
    stop: Arc<AtomicBool>,
    report: impl Fn(&str, Option<&Error>) + Send + 'static,
) -> JoinHandle<Link> {
    thread::spawn(move || {
        let mut was_up = None;
        let start = Instant::now();
        let mut next = start;
        while !stop.load(Ordering::Acquire) {
            let r = link.cycle_at(&table, start.elapsed());
            if was_up != Some(r.is_ok()) {
                report(&link.name, r.as_ref().err());
                was_up = Some(r.is_ok());
            }
            next += link.poll;
            let now = Instant::now();
            if next < now {
                next = now;
            }
            nap(next, &stop);
        }
        link
    })
}

/// Opens a device's driver: `helmsman_io::open` on a box.
pub type Opener = Arc<dyn Fn(&Device) -> Result<Box<dyn Driver + Send>, Error> + Send + Sync>;
/// A new driver for the box's own channels.
pub type Channels = Arc<dyn Fn() -> Box<dyn Driver + Send> + Send + Sync>;
/// Where links say they went up (`None`) or down (the error).
pub type Report = Arc<dyn Fn(&str, Option<&Error>) + Send + Sync>;

/// What a link is, to tell whether a new map changes it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    name: String,
    /// The device's line; `None` for the box's own channels.
    device: Option<Device>,
    /// Every point: direction, signal name, type, address, own poll.
    points: Vec<String>,
    /// Output points: signal name and address.
    outputs: Vec<(String, String)>,
    /// For a `wasm:` device, SHA-256 of the driver binary it runs on: a
    /// newly approved version of the driver is another link.
    guest: Option<[u8; 32]>,
}

fn groups(map: &IoMap, drivers: &Drivers) -> Vec<(Group, Vec<Polled>)> {
    by_device(map)
        .into_iter()
        .map(|(dev, sigs)| {
            let device = dev
                .as_ref()
                .and_then(|id| map.devices.iter().find(|d| &d.id == id).cloned());
            let mut points: Vec<String> = sigs
                .iter()
                .map(|p| {
                    let s = &map.signals[p.signal];
                    let mut line = format!("{:?} {} {:?} {}", p.dir, s.name, s.ty, p.addr);
                    if let Some(poll) = p.poll {
                        let _ = write!(line, " {}", poll.as_nanos());
                    }
                    line
                })
                .collect();
            points.sort();
            let outputs = sigs
                .iter()
                .filter(|p| p.dir == Dir::Out)
                .map(|p| (map.signals[p.signal].name.clone(), p.addr.clone()))
                .collect();
            let guest = device.as_ref().and_then(|d| drivers::digest(drivers, d));
            let group = Group {
                name: dev.unwrap_or_else(|| BOX.to_string()),
                device,
                points,
                outputs,
                guest,
            };
            (group, sigs)
        })
        .collect()
}

/// Whether two device lines reach the same device: the same id, driver,
/// endpoint and driver params, but for how often and how patiently the
/// link polls it (`poll_ms`, `timeout_ms`). The box's own channels are
/// one place.
fn same_place(a: Option<&Device>, b: Option<&Device>) -> bool {
    const PACE: [&str; 2] = ["poll_ms", "timeout_ms"];
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let params = |d: &Device| {
                d.params
                    .iter()
                    .filter(|(k, _)| !PACE.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            };
            a.id == b.id && a.driver == b.driver && a.endpoint == b.endpoint && params(a) == params(b)
        }
        _ => false,
    }
}

struct Running {
    group: Group,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Link>,
}

/// What [`LinkSet::plan`] found a new map needs: links kept as they run,
/// links to stop, and new links built (drivers opened) but not started.
pub struct Plan {
    stop: Vec<String>,
    start: Vec<(Group, Link)>,
}

impl Plan {
    /// Whether the new map changes no link at all.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.stop.is_empty() && self.start.is_empty()
    }

    /// The links it would stop (removed or changed devices).
    #[must_use]
    pub fn stops(&self) -> &[String] {
        &self.stop
    }

    /// The links it would start (added or changed devices).
    #[must_use]
    pub fn starts(&self) -> Vec<&str> {
        self.start.iter().map(|(g, _)| g.name.as_str()).collect()
    }
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("stop", &self.stop)
            .field("start", &self.starts())
            .finish()
    }
}

/// Links [`LinkSet::stop`] stopped and joined, each with the safe value
/// of each of its outputs as the table had them when it stopped.
pub struct Stopped {
    links: Vec<(Group, Link, Safes)>,
}

/// Each output point's address and safe value.
type Safes = Vec<(String, Option<Value>)>;

impl std::fmt::Debug for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.links.iter().map(|(g, _, _)| g.name.as_str()).collect();
        f.debug_struct("Stopped").field("links", &names).finish()
    }
}

/// What [`LinkSet::finish`] did besides starting links.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Finished {
    /// Links that ended for good (their device left the map).
    pub removed: Vec<String>,
    /// Links that started for the first time.
    pub added: Vec<String>,
    /// Outputs driven to their safe value on a device as they left the map
    /// (or were bound elsewhere): `device addr`.
    pub safed: Vec<String>,
    /// Anything that could not be done: an output that could not be sent
    /// to safe because its device did not answer, in words.
    pub notes: Vec<String>,
}

/// The running device links of a box, changed as one when the I/O map is.
///
/// A reload goes [`LinkSet::plan`] (nothing running is touched: links the
/// new map leaves exactly as they are are kept, the rest are built with
/// their drivers opened, so a driver that will not open refuses the
/// reload before anything stops), then [`LinkSet::stop`] (links of removed
/// or changed devices are stopped and their threads joined, so none of
/// them can write again), then the table swap, then either
/// [`LinkSet::resume`] (the swap was refused: the same links, state and
/// all, run again) or [`LinkSet::finish`]. `finish` sends every output
/// point the new map no longer has on its device to its safe value,
/// through the stopped link's own driver, then starts the new links.
///
/// A device that does not answer then cannot be sent to safe: the box can
/// only reach a device through its link, and a down link is the case
/// where it cannot. That is reported ([`Finished::notes`]); the device's
/// own communication watchdog is what makes it safe then.
///
/// A link changed only in its signals (same device line) carries its
/// driver (connection, and for `box` the channels themselves) into its
/// new link. Kept links never stop: they find their points again in the
/// new table by name ([`Link`] docs).
pub struct LinkSet {
    table: Arc<Mutex<SignalTable>>,
    open: Opener,
    channels: Channels,
    report: Report,
    running: Vec<Running>,
}

impl std::fmt::Debug for LinkSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkSet")
            .field("links", &self.names())
            .finish_non_exhaustive()
    }
}

impl LinkSet {
    /// No links yet; [`LinkSet::start`] them for a map.
    #[must_use]
    pub fn new(table: Arc<Mutex<SignalTable>>, open: Opener, channels: Channels, report: Report) -> Self {
        Self {
            table,
            open,
            channels,
            report,
            running: Vec::new(),
        }
    }

    /// Starts every link of `map` (on a table built from it).
    ///
    /// # Errors
    /// As [`LinkSet::plan`]; nothing is started.
    pub fn start(&mut self, map: &IoMap) -> Result<Finished, String> {
        let plan = self.plan(map)?;
        let stopped = self.stop(&plan);
        Ok(self.finish(plan, stopped))
    }

    /// The running links' names, in start order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.running.iter().map(|r| r.group.name.clone()).collect()
    }

    /// What `map` needs; see the [type docs](Self). Touches nothing that
    /// runs.
    ///
    /// # Errors
    /// A device whose driver will not open or whose `poll_ms` is out of
    /// range, with its id.
    pub fn plan(&self, map: &IoMap) -> Result<Plan, String> {
        self.plan_with(map, &Drivers::new())
    }

    /// [`LinkSet::plan`] for a map that may name the approved WASM
    /// `drivers` (`wasm:NAME`): such a device's link runs on its guest
    /// ([`drivers::open`]), not the box's opener, and a link whose
    /// driver's bytes changed is replaced like one whose device line did.
    ///
    /// # Errors
    /// As [`LinkSet::plan`], plus a device naming a driver not in
    /// `drivers`.
    pub fn plan_with(&self, map: &IoMap, drivers: &Drivers) -> Result<Plan, String> {
        let mut keep = Vec::new();
        let mut start = Vec::new();
        for (group, sigs) in groups(map, drivers) {
            if self.running.iter().any(|r| r.group == group) {
                keep.push(group.name);
                continue;
            }
            let (poll, driver) = match &group.device {
                None => (DEFAULT_POLL, (self.channels)()),
                Some(d) => (
                    device_poll(d)?,
                    drivers::open(drivers, d)
                        .unwrap_or_else(|| (self.open)(d))
                        .map_err(|e| format!("{}: {e}", d.id))?,
                ),
            };
            let link = build(group.name.clone(), poll, driver, map, sigs);
            start.push((group, link));
        }
        let stop = self
            .running
            .iter()
            .map(|r| r.group.name.clone())
            .filter(|n| !keep.contains(n))
            .collect();
        Ok(Plan { stop, start })
    }

    /// Stops and joins the links `plan` stops. When this returns none of
    /// them is running.
    pub fn stop(&mut self, plan: &Plan) -> Stopped {
        let (going, staying): (Vec<Running>, Vec<Running>) = std::mem::take(&mut self.running)
            .into_iter()
            .partition(|r| plan.stop.contains(&r.group.name));
        self.running = staying;
        for r in &going {
            r.stop.store(true, Ordering::Release);
            r.handle.thread().unpark();
        }
        let mut links = Vec::new();
        for r in going {
            // A link thread that panicked has no link to hand back; its
            // device is then reached by nobody, as if it were down.
            if let Ok(link) = r.handle.join() {
                links.push((r.group, link));
            }
        }
        let t = lock(&self.table);
        let links = links
            .into_iter()
            .map(|(group, mut link)| {
                link.sync(&t);
                let safes = link
                    .outputs
                    .iter()
                    .map(|o| (o.addr.clone(), o.signal.and_then(|s| t.safe(s))))
                    .collect();
                (group, link, safes)
            })
            .collect();
        Stopped { links }
    }

    fn run(&mut self, group: Group, link: Link) {
        let stop = Arc::new(AtomicBool::new(false));
        let report = Arc::clone(&self.report);
        let handle = spawn(link, Arc::clone(&self.table), Arc::clone(&stop), move |n, e| {
            report(n, e);
        });
        self.running.push(Running { group, stop, handle });
    }

    /// The reload was refused: runs the stopped links again as they were.
    pub fn resume(&mut self, stopped: Stopped) {
        for (group, link, _) in stopped.links {
            self.run(group, link);
        }
    }

    /// The table is the new map's: sends what left each stopped link's
    /// device to safe, then starts `plan`'s new links. See the
    /// [type docs](Self).
    pub fn finish(&mut self, plan: Plan, stopped: Stopped) -> Finished {
        let mut done = Finished::default();
        let mut start = plan.start;
        let was: Vec<String> = stopped.links.iter().map(|(g, _, _)| g.name.clone()).collect();
        for (group, mut link, safes) in stopped.links {
            let next = start
                .iter_mut()
                .find(|(g, _)| g.name == group.name && same_place(g.device.as_ref(), group.device.as_ref()));
            let mut down = false;
            for (addr, safe) in safes {
                // The same device still has an output there: its new link
                // writes it at once, with whatever the new map says (for a
                // signal the reload kept, the command it had: no glitch).
                if next
                    .as_ref()
                    .is_some_and(|(g, _)| g.outputs.iter().any(|(_, a)| *a == addr))
                {
                    continue;
                }
                let Some(safe) = safe else { continue };
                if down {
                    done.notes.push(format!(
                        "{} {addr}: not sent to safe; the device did not answer",
                        group.name
                    ));
                    continue;
                }
                match link.force(&addr, safe) {
                    Ok(()) => done.safed.push(format!("{} {addr}", group.name)),
                    Err(e) => {
                        down = e.is_link();
                        done.notes
                            .push(format!("{} {addr}: not sent to safe: {e}", group.name));
                    }
                }
            }
            match next {
                // Same device line on the same driver: the connection (for
                // `box`, the channels themselves) carries over. A new
                // version of a WASM driver starts on its own fresh guest.
                Some((g, fresh)) if g.device == group.device && g.guest == group.guest => {
                    fresh.driver = link.driver;
                }
                Some(_) => {}
                None => {
                    if !start.iter().any(|(g, _)| g.name == group.name) {
                        done.removed.push(group.name);
                    }
                }
            }
        }
        for (group, link) in start {
            if !was.contains(&group.name) {
                done.added.push(group.name.clone());
            }
            self.run(group, link);
        }
        done
    }

    /// Stops and joins every link.
    pub fn shutdown(&mut self) {
        let plan = Plan {
            stop: self.names(),
            start: Vec::new(),
        };
        drop(self.stop(&plan));
    }
}

impl Drop for LinkSet {
    fn drop(&mut self) {
        self.shutdown();
    }
}
