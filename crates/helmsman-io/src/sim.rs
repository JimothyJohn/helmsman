//! A Modbus device simulator (TCP here; RTU over a pipe in
//! [`crate::rtu::sim`], sharing [`State`]) for tests and the bench harness: four tables,
//! cross-wires (a coil driving a discrete input, as a wire from an output
//! terminal to an input terminal would), and injectable faults. It answers
//! the function codes [`crate::modbus`] uses and refuses the rest with the
//! spec's exceptions. [`Sim::gateway`] is a TCP-to-serial gateway instead:
//! several units, each with its own tables and faults, one request at a
//! time, a cap on connections, and units that can be muted.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::modbus::{
    COIL_OFF, COIL_ON, ENCAPSULATED_INTERFACE, ILLEGAL_DATA_ADDRESS, ILLEGAL_DATA_VALUE, ILLEGAL_FUNCTION,
    MAX_PDU, MAX_READ_BITS, MAX_READ_REGS, MAX_WRITE_REGS, MBAP_LEN, MEI_READ_DEVICE_ID,
    OBJECT_MAJOR_MINOR_REVISION, OBJECT_PRODUCT_CODE, OBJECT_VENDOR_NAME, READ_COILS, READ_DISCRETE_INPUTS,
    READ_HOLDING_REGISTERS, READ_INPUT_REGISTERS, SERVER_DEVICE_FAILURE, WRITE_MULTIPLE_REGISTERS,
    WRITE_SINGLE_COIL, WRITE_SINGLE_REGISTER,
};

/// A misbehaviour for the next request(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Answer late.
    Delay(Duration),
    /// Close the connection without answering.
    Drop,
    /// Answer with bytes that are not a Modbus reply.
    Garbage,
    /// Answer with the wrong transaction id.
    WrongTransaction,
    /// Answer with exception 04 (server device failure).
    DeviceFailure,
    /// Answer with this exception code (a gateway's 0x0A and 0x0B, App §7
    /// p.47).
    Exception(u8),
    /// RTU: answer with one bit of the frame flipped, so its CRC fails.
    BadCrc,
    /// RTU: answer with the frame cut short by two bytes.
    Truncate,
    /// RTU: answer as another unit (a valid frame, CRC and all).
    WrongUnit,
    /// RTU: line noise just before an otherwise good answer.
    Noise,
    /// RTU: a good answer, one byte at a time.
    Split,
    /// EtherNet/IP: the target stops producing T->O data (a pulled cable,
    /// a hung device) until cleared.
    Silent,
    /// EtherNet/IP: T->O data one byte shorter than the connection's size.
    ShortInput,
    /// EtherNet/IP: the target forgets every connection (a power cycle).
    Reboot,
}

/// A simulator that can be told to misbehave: what the conformance
/// harness's fault checks drive. The TCP simulator ignores the RTU-only
/// faults (it answers normally), so each simulator lists the faults that
/// mean something on its transport.
pub trait Faults {
    /// Applies `fault` to the next request.
    fn inject(&self, fault: Fault);
    fn clear_faults(&self);
    /// The faults worth checking on this transport, named.
    fn fault_checks(&self) -> Vec<(&'static str, Fault)>;
    /// How long after a fault is injected the driver may still answer
    /// reads before it must notice. Zero for request/reply transports
    /// (the next read is the one that meets the fault); a cyclic
    /// transport (EtherNet/IP Class 1) notices only when its connection
    /// times out.
    fn fault_window(&self) -> Duration {
        Duration::ZERO
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub coils: Vec<bool>,
    pub discrete: Vec<bool>,
    pub holding: Vec<u16>,
    pub input: Vec<u16>,
    /// `(coil, discrete input)`: writing the coil sets the input.
    pub wires: Vec<(u16, u16)>,
    /// Applied to the next requests, one per request, front first.
    pub faults: Vec<Fault>,
    /// Requests answered, faults included.
    pub requests: u64,
    /// What Read Device Identification (FC 43/14) answers; `None`: the
    /// function is refused with Illegal Function, as a device without it
    /// does.
    pub identification: Option<Identification>,
}

/// The simulator's Read Device Identification objects (App §6.21
/// pp.43-46): stream access (codes 01-03) from the object asked for,
/// restarting at the first object when that id is unknown, at most
/// `per_reply` objects a reply with More Follows and Next Object Id for
/// the rest; individual access (code 04) for one object, Illegal Data
/// Address when there is none; any other code Illegal Data Value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identification {
    /// `(object id, value)`, ascending by id.
    pub objects: Vec<(u8, Vec<u8>)>,
    /// Objects a reply carries at most (at least 1); fewer when the PDU
    /// would pass 253 bytes.
    pub per_reply: usize,
}

impl Identification {
    /// The basic three objects in one reply.
    #[must_use]
    pub fn basic(vendor: &str, product: &str, revision: &str) -> Self {
        Self {
            objects: vec![
                (OBJECT_VENDOR_NAME, vendor.as_bytes().to_vec()),
                (OBJECT_PRODUCT_CODE, product.as_bytes().to_vec()),
                (OBJECT_MAJOR_MINOR_REVISION, revision.as_bytes().to_vec()),
            ],
            per_reply: 3,
        }
    }

    fn answer(&self, pdu: &[u8]) -> Vec<u8> {
        let ex = |code: u8| vec![ENCAPSULATED_INTERFACE | 0x80, code];
        let &[_, mei, code, object] = pdu else {
            return ex(ILLEGAL_DATA_VALUE);
        };
        if mei != MEI_READ_DEVICE_ID {
            return ex(ILLEGAL_FUNCTION);
        }
        let mut out = vec![ENCAPSULATED_INTERFACE, MEI_READ_DEVICE_ID, code, 0x81];
        let push = |out: &mut Vec<u8>, (id, v): &(u8, Vec<u8>)| {
            out.push(*id);
            out.push(u8::try_from(v.len()).unwrap_or(u8::MAX));
            out.extend(v.iter().take(usize::from(u8::MAX)));
        };
        match code {
            0x01..=0x03 => {
                let start = self.objects.iter().position(|(id, _)| *id == object).unwrap_or(0);
                let mut body = Vec::new();
                let mut n = 0usize;
                let mut next = None;
                for o in &self.objects[start..] {
                    if n == self.per_reply.max(1) || out.len() + 3 + body.len() + 2 + o.1.len() > MAX_PDU {
                        next = Some(o.0);
                        break;
                    }
                    push(&mut body, o);
                    n += 1;
                }
                out.push(if next.is_some() { 0xFF } else { 0x00 });
                out.push(next.unwrap_or(0));
                out.push(u8::try_from(n).unwrap_or(u8::MAX));
                out.extend(body);
                out
            }
            0x04 => {
                let Some(o) = self.objects.iter().find(|(id, _)| *id == object) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                out.extend([0x00, 0x00, 0x01]);
                push(&mut out, o);
                out
            }
            _ => ex(ILLEGAL_DATA_VALUE),
        }
    }
}

impl State {
    #[must_use]
    pub fn new(size: u16) -> Self {
        let n = usize::from(size);
        Self {
            coils: vec![false; n],
            discrete: vec![false; n],
            holding: vec![0; n],
            input: vec![0; n],
            ..Self::default()
        }
    }

    /// Counts a request and takes the fault for it, if any.
    pub(crate) fn next_fault(&mut self) -> Option<Fault> {
        self.requests += 1;
        if self.faults.is_empty() {
            None
        } else {
            Some(self.faults.remove(0))
        }
    }

    fn apply_wires(&mut self) {
        for &(c, d) in &self.wires {
            if let (Some(&v), Some(slot)) = (
                self.coils.get(usize::from(c)),
                self.discrete.get_mut(usize::from(d)),
            ) {
                *slot = v;
            }
        }
    }

    /// FC 43: Read Device Identification when the simulator has
    /// [`Identification`], else Illegal Function as a device without it.
    fn identify(&self, pdu: &[u8]) -> Vec<u8> {
        match &self.identification {
            Some(id) => id.answer(pdu),
            None => vec![ENCAPSULATED_INTERFACE | 0x80, ILLEGAL_FUNCTION],
        }
    }

    /// Answers one request PDU (App §6), or an exception (App §7).
    #[must_use]
    pub fn handle(&mut self, pdu: &[u8]) -> Vec<u8> {
        let Some(&fc) = pdu.first() else {
            return vec![0x80, ILLEGAL_FUNCTION];
        };
        let ex = |code: u8| vec![fc | 0x80, code];
        let word = |i: usize| pdu.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let range = |len: usize, start: u16, qty: u16| {
            let end = usize::from(start) + usize::from(qty);
            (end <= len).then_some(usize::from(start)..end)
        };
        match fc {
            READ_COILS | READ_DISCRETE_INPUTS => {
                let (Some(start), Some(qty), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if qty == 0 || qty > MAX_READ_BITS {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let table = if fc == READ_COILS {
                    &self.coils
                } else {
                    &self.discrete
                };
                let Some(r) = range(table.len(), start, qty) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                let mut bytes = vec![0u8; usize::from(qty).div_ceil(8)];
                for (i, b) in table[r].iter().enumerate() {
                    bytes[i / 8] |= u8::from(*b) << (i % 8);
                }
                let mut out = vec![fc, u8::try_from(bytes.len()).expect("at most 250")];
                out.extend(bytes);
                out
            }
            READ_HOLDING_REGISTERS | READ_INPUT_REGISTERS => {
                let (Some(start), Some(qty), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if qty == 0 || qty > MAX_READ_REGS {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let table = if fc == READ_HOLDING_REGISTERS {
                    &self.holding
                } else {
                    &self.input
                };
                let Some(r) = range(table.len(), start, qty) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                let mut out = vec![fc, u8::try_from(qty * 2).expect("at most 250")];
                for v in &table[r] {
                    out.extend(v.to_be_bytes());
                }
                out
            }
            WRITE_SINGLE_COIL => {
                let (Some(addr), Some(v), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if v != COIL_ON && v != COIL_OFF {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let Some(slot) = self.coils.get_mut(usize::from(addr)) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                *slot = v == COIL_ON;
                self.apply_wires();
                pdu.to_vec()
            }
            WRITE_SINGLE_REGISTER => {
                let (Some(addr), Some(v), 5) = (word(1), word(3), pdu.len()) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                let Some(slot) = self.holding.get_mut(usize::from(addr)) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                *slot = v;
                pdu.to_vec()
            }
            WRITE_MULTIPLE_REGISTERS => {
                let (Some(start), Some(qty), Some(&n)) = (word(1), word(3), pdu.get(5)) else {
                    return ex(ILLEGAL_DATA_VALUE);
                };
                if qty == 0
                    || qty > MAX_WRITE_REGS
                    || usize::from(n) != usize::from(qty) * 2
                    || pdu.len() != 6 + usize::from(n)
                {
                    return ex(ILLEGAL_DATA_VALUE);
                }
                let Some(r) = range(self.holding.len(), start, qty) else {
                    return ex(ILLEGAL_DATA_ADDRESS);
                };
                for (slot, c) in self.holding[r].iter_mut().zip(pdu[6..].as_chunks::<2>().0) {
                    *slot = u16::from_be_bytes(*c);
                }
                pdu[..5].to_vec()
            }
            ENCAPSULATED_INTERFACE => self.identify(pdu),
            _ => ex(ILLEGAL_FUNCTION),
        }
    }
}

/// What a server does with one connection more than it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    /// Takes the new one and closes the oldest open one: what the guide
    /// prescribes ("the oldest unused connection is closed", TCP §4.2.1.2
    /// p.12).
    CloseOldest,
    /// Closes the new one at once, unanswered: what a gateway that
    /// refuses past its limit does.
    Refuse,
}

/// A Modbus TCP gateway to a serial line: several units behind one
/// address (the unit id routes the request, TCP §3.1.3 pp.5-6). It
/// answers one request at a time across all its connections, as the line
/// behind it does, and never answers a unit it does not have.
#[derive(Clone, Debug, Default)]
pub struct Gateway {
    /// The units behind it, each with its own tables and faults.
    pub units: Vec<(u8, State)>,
    /// Most connections open at once, and what one more meets.
    pub max_connections: Option<(usize, Cap)>,
}

enum Units {
    /// The plain simulator: one device, whatever unit id is asked.
    Any(Arc<Mutex<State>>),
    /// A gateway's units.
    Each(BTreeMap<u8, Arc<Mutex<State>>>),
}

struct Shared {
    units: Units,
    max_connections: Option<(usize, Cap)>,
    /// Units that never answer (a unit unplugged from the line).
    muted: Mutex<BTreeSet<u8>>,
    /// A gateway's serial line: one request at a time.
    line: Mutex<()>,
    accepted: AtomicUsize,
    refused: AtomicUsize,
    /// Open connections, oldest first, by accept number.
    open: Mutex<Vec<(usize, TcpStream)>>,
}

/// A running simulator on 127.0.0.1. Stops when dropped.
pub struct Sim {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Sim {
    /// Starts serving `state` on an ephemeral loopback port, for any unit
    /// id, on as many connections as are opened.
    ///
    /// # Errors
    /// The listener could not be bound.
    pub fn start(state: State) -> std::io::Result<Self> {
        Self::listen(Units::Any(Arc::new(Mutex::new(state))), None)
    }

    /// Starts a [`Gateway`] on an ephemeral loopback port.
    ///
    /// # Errors
    /// The listener could not be bound.
    pub fn gateway(g: Gateway) -> std::io::Result<Self> {
        let units = g
            .units
            .into_iter()
            .map(|(u, s)| (u, Arc::new(Mutex::new(s))))
            .collect();
        Self::listen(Units::Each(units), g.max_connections)
    }

    fn listen(units: Units, max_connections: Option<(usize, Cap)>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            units,
            max_connections,
            muted: Mutex::new(BTreeSet::new()),
            line: Mutex::new(()),
            accepted: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            open: Mutex::new(Vec::new()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, sp) = (Arc::clone(&shared), Arc::clone(&stop));
        let accept = thread::spawn(move || {
            while !sp.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let Some(id) = admit(&sh, &conn) else {
                            let _ = conn.shutdown(Shutdown::Both);
                            continue;
                        };
                        let (sh, sp) = (Arc::clone(&sh), Arc::clone(&sp));
                        thread::spawn(move || {
                            serve(conn, &sh, &sp);
                            lock(&sh.open).retain(|(i, _)| *i != id);
                        });
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(2)),
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            addr,
            shared,
            stop,
            accept: Some(accept),
        })
    }

    /// The simulator's tables, to set up or inspect.
    ///
    /// # Panics
    /// A gateway: its units each have their own ([`Self::unit`]).
    pub fn state(&self) -> MutexGuard<'_, State> {
        match &self.shared.units {
            Units::Any(s) => lock(s),
            Units::Each(_) => panic!("a gateway's tables are per unit: Sim::unit"),
        }
    }

    /// Unit `unit`'s tables (the plain simulator's one set for any unit).
    ///
    /// # Panics
    /// A gateway without that unit.
    pub fn unit(&self, unit: u8) -> MutexGuard<'_, State> {
        match &self.shared.units {
            Units::Any(s) => lock(s),
            Units::Each(m) => lock(m.get(&unit).expect("a unit the gateway has")),
        }
    }

    /// Unit `unit` stops answering (`true`), as one unplugged from the
    /// line, or answers again.
    pub fn mute(&self, unit: u8, muted: bool) {
        let mut m = lock(&self.shared.muted);
        if muted {
            m.insert(unit);
        } else {
            m.remove(&unit);
        }
    }

    /// Connections taken so far (refused ones not counted).
    #[must_use]
    pub fn accepted(&self) -> usize {
        self.shared.accepted.load(Ordering::Relaxed)
    }

    /// Connections refused for the cap ([`Cap::Refuse`]).
    #[must_use]
    pub fn refused(&self) -> usize {
        self.shared.refused.load(Ordering::Relaxed)
    }

    /// Connections open now.
    #[must_use]
    pub fn open_connections(&self) -> usize {
        lock(&self.shared.open).len()
    }

    /// Closes every open connection, as a gateway that restarts does.
    pub fn drop_connections(&self) {
        for (_, c) in lock(&self.shared.open).drain(..) {
            let _ = c.shutdown(Shutdown::Both);
        }
    }

    /// Applies `fault` to the next request (on a gateway, to each unit's
    /// next request).
    pub fn inject(&self, fault: Fault) {
        match &self.shared.units {
            Units::Any(s) => lock(s).faults.push(fault),
            Units::Each(m) => m.values().for_each(|s| lock(s).faults.push(fault)),
        }
    }

    pub fn clear_faults(&self) {
        match &self.shared.units {
            Units::Any(s) => lock(s).faults.clear(),
            Units::Each(m) => m.values().for_each(|s| lock(s).faults.clear()),
        }
    }

    #[must_use]
    pub fn endpoint(&self) -> String {
        self.addr.to_string()
    }
}

/// Takes a connection under the cap: its accept number, or `None` when
/// it is refused.
///
/// Counts first, then acts: whatever a peer sees (a refusal, its old
/// connection closed) happens after the counters say so.
fn admit(sh: &Shared, conn: &TcpStream) -> Option<usize> {
    let mut open = lock(&sh.open);
    if let Some((max, Cap::Refuse)) = sh.max_connections
        && open.len() >= max
    {
        sh.refused.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    let id = sh.accepted.fetch_add(1, Ordering::Relaxed);
    if let Some((max, Cap::CloseOldest)) = sh.max_connections {
        while !open.is_empty() && open.len() >= max {
            let (_, old) = open.remove(0);
            let _ = old.shutdown(Shutdown::Both);
        }
    }
    if let Ok(c) = conn.try_clone() {
        open.push((id, c));
    }
    Some(id)
}

/// The TCP fault checks.
pub(crate) fn tcp_fault_checks() -> Vec<(&'static str, Fault)> {
    vec![
        ("drop", Fault::Drop),
        ("garbage", Fault::Garbage),
        ("wrong transaction", Fault::WrongTransaction),
        ("device failure", Fault::DeviceFailure),
        ("late reply", Fault::Delay(Duration::from_secs(3))),
    ]
}

impl Faults for Sim {
    fn inject(&self, fault: Fault) {
        Self::inject(self, fault);
    }

    fn clear_faults(&self) {
        Self::clear_faults(self);
    }

    fn fault_checks(&self) -> Vec<(&'static str, Fault)> {
        tcp_fault_checks()
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

/// Reads exactly `buf.len()` bytes, giving up when the simulator stops.
fn read_full(conn: &mut TcpStream, buf: &mut [u8], stop: &AtomicBool) -> bool {
    let mut got = 0;
    while got < buf.len() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        match conn.read(&mut buf[got..]) {
            Ok(0) => return false,
            Ok(n) => got += n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return false,
        }
    }
    true
}

fn serve(mut conn: TcpStream, sh: &Shared, stop: &AtomicBool) {
    let _ = conn.set_read_timeout(Some(Duration::from_millis(20)));
    let _ = conn.set_nodelay(true);
    loop {
        let mut head = [0u8; MBAP_LEN];
        if !read_full(&mut conn, &mut head, stop) {
            return;
        }
        let len = usize::from(u16::from_be_bytes([head[4], head[5]]));
        // TCP §3.1.3 p.5: protocol id 0; length covers unit id + PDU.
        if head[2..4] != [0, 0] || !(2..=MAX_PDU + 1).contains(&len) {
            return;
        }
        let mut pdu = vec![0u8; len - 1];
        if !read_full(&mut conn, &mut pdu, stop) {
            return;
        }
        let unit = head[6];
        let state = match &sh.units {
            Units::Any(s) => Some(s),
            Units::Each(m) => m.get(&unit),
        };
        // A unit the gateway lacks, or one muted, never answers.
        let Some(state) = state.filter(|_| !lock(&sh.muted).contains(&unit)) else {
            continue;
        };
        // A gateway's line carries one request at a time.
        let _line = matches!(sh.units, Units::Each(_)).then(|| lock(&sh.line));
        let (fault, mut reply) = {
            let mut s = lock(state);
            let fault = s.next_fault();
            let reply = match fault {
                Some(Fault::DeviceFailure) => vec![pdu[0] | 0x80, SERVER_DEVICE_FAILURE],
                Some(Fault::Exception(code)) => vec![pdu[0] | 0x80, code],
                _ => s.handle(&pdu),
            };
            (fault, reply)
        };
        let mut tid = [head[0], head[1]];
        match fault {
            Some(Fault::Drop) => return,
            Some(Fault::Delay(d)) => thread::sleep(d),
            Some(Fault::Garbage) => reply = vec![0xDE, 0xAD],
            Some(Fault::WrongTransaction) => tid = u16::from_be_bytes(tid).wrapping_add(1).to_be_bytes(),
            Some(
                Fault::DeviceFailure
                | Fault::Exception(_)
                | Fault::BadCrc
                | Fault::Truncate
                | Fault::WrongUnit
                | Fault::Noise
                | Fault::Split
                | Fault::Silent
                | Fault::ShortInput
                | Fault::Reboot,
            )
            | None => {}
        }
        let mut frame = Vec::with_capacity(MBAP_LEN + reply.len());
        frame.extend(tid);
        frame.extend([0, 0]);
        frame.extend(u16::try_from(reply.len() + 1).expect("pdu fits").to_be_bytes());
        frame.push(unit);
        frame.extend(reply);
        if fault == Some(Fault::Garbage) {
            // Garbage in the header too: protocol id 0xBEEF.
            frame[2..4].copy_from_slice(&[0xBE, 0xEF]);
        }
        if conn.write_all(&frame).is_err() {
            return;
        }
    }
}
