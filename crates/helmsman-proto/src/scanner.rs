//! Originator-side Class 1 I/O: open a connection with `Forward_Open`,
//! produce O->T packets at the RPI from a thread, consume T->O packets on
//! another, close with `Forward_Close`.
//!
//! Addressing follows what `OpENer` does for point-to-point
//! (`cipioconnection.c` `OpenProducingPointToPointConnection` /
//! `OpenConsumingPointToPointConnection`): the target sends T->O to the
//! IP it saw the TCP session come from, UDP port 2222, and only accepts
//! O->T packets from that same IP. So the originator binds UDP 2222 on
//! the interface it opened the session from; see [`bind_io_socket`].
//! Timeout semantics: RPI × 2^(multiplier + 2)
//! (`cipconnectionobject.c`), applied here to the T->O direction.

use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::encap;
use crate::io::{
    Class1Body, ConnectionPath, ElectronicKey, ForwardClose, ForwardCloseReply, ForwardOpen,
    ForwardOpenReply, IoPacket, NetworkParams, Priority, Refused, Transport, class1_size, close_result,
    open_result,
};
use crate::session::{self, Session};

/// What the originator asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoConfig {
    /// Configuration assembly instance.
    pub configuration: u16,
    /// O->T connection point (the target's output/consumed assembly, or its
    /// heartbeat instance for `Input Only` / `Listen Only`).
    pub consumed: u16,
    /// T->O connection point (the target's input/produced assembly).
    pub produced: u16,
    /// Application bytes per O->T packet; 0 makes it a heartbeat.
    pub output_len: u16,
    /// Application bytes per T->O packet.
    pub input_len: u16,
    pub rpi: Duration,
    /// Inactivity timeout = RPI × 2^(multiplier + 2).
    pub timeout_multiplier: u8,
    /// Prepend the 32-bit run/idle header to O->T data. `OpENer` builds
    /// with this on (`OPENER_CONSUMED_DATA_HAS_RUN_IDLE_HEADER` default
    /// TRUE); Logix controllers send it too.
    pub o_to_t_run_idle: bool,
    /// Expect a run/idle header on T->O data. `OpENer` default is off.
    pub t_to_o_run_idle: bool,
    pub priority: Priority,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    pub key: Option<ElectronicKey>,
    pub config_data: Vec<u8>,
}

impl IoConfig {
    /// `Exclusive Owner`: bidirectional, this originator owns the output.
    #[must_use]
    pub fn exclusive_owner(
        configuration: u16,
        consumed: u16,
        produced: u16,
        output_len: u16,
        input_len: u16,
        rpi: Duration,
    ) -> Self {
        Self {
            configuration,
            consumed,
            produced,
            output_len,
            input_len,
            rpi,
            timeout_multiplier: 1,
            o_to_t_run_idle: true,
            t_to_o_run_idle: false,
            priority: Priority::Scheduled,
            originator_vendor_id: 0xFFFF,
            originator_serial: 0,
            key: None,
            config_data: Vec::new(),
        }
    }

    /// `Input Only`: heartbeat O->T to `heartbeat` (PUB00213R0 glossary p.24),
    /// data T->O from `produced`.
    #[must_use]
    pub fn input_only(
        configuration: u16,
        heartbeat: u16,
        produced: u16,
        input_len: u16,
        rpi: Duration,
    ) -> Self {
        Self::exclusive_owner(configuration, heartbeat, produced, 0, input_len, rpi)
    }

    fn rpi_us(&self) -> u32 {
        u32::try_from(self.rpi.as_micros()).unwrap_or(u32::MAX)
    }

    fn forward_open(&self, serial: u16, t_to_o_id: u32) -> ForwardOpen {
        let mut path = ConnectionPath::assembly(self.configuration, Some(self.consumed), Some(self.produced));
        path.key = self.key;
        path.config_data.clone_from(&self.config_data);
        ForwardOpen {
            priority_time_tick: 0x0A,
            timeout_ticks: 0xF0,
            o_to_t_connection_id: 0,
            t_to_o_connection_id: t_to_o_id,
            connection_serial: serial,
            originator_vendor_id: self.originator_vendor_id,
            originator_serial: self.originator_serial,
            timeout_multiplier: self.timeout_multiplier,
            o_to_t_rpi_us: self.rpi_us(),
            o_to_t: NetworkParams::point_to_point(
                class1_size(self.output_len, self.o_to_t_run_idle),
                self.priority,
            ),
            t_to_o_rpi_us: self.rpi_us(),
            t_to_o: NetworkParams::point_to_point(
                class1_size(self.input_len, self.t_to_o_run_idle),
                self.priority,
            ),
            transport: Transport::CLASS1_CYCLIC,
            path,
        }
    }

    /// Inactivity timeout this configuration implies.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.rpi * (1u32 << (u32::from(self.timeout_multiplier) + 2))
    }
}

/// Why an I/O connection could not be opened or closed.
#[derive(Debug)]
pub enum Error {
    Session(session::Error),
    Refused(Refused),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session(e) => write!(f, "session: {e}"),
            Self::Refused(r) => write!(f, "forward open {r}"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<session::Error> for Error {
    fn from(e: session::Error) -> Self {
        Self::Session(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The latest T->O packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub data: Vec<u8>,
    pub sequence_count: u16,
    pub run_idle: Option<u32>,
    pub received: Instant,
}

struct Shared {
    output: Mutex<Vec<u8>>,
    run: AtomicBool,
    input: Mutex<Option<Input>>,
    packets_in: AtomicU64,
    packets_out: AtomicU64,
    /// Datagrams that decoded but were not for this connection or did not
    /// parse; a nonzero count is worth a look, not an error.
    ignored: AtomicU64,
}

/// Bind the I/O socket on `local` at port 2222, where point-to-point
/// targets send T->O data.
///
/// # Errors
/// Bind failure, typically because something else holds 2222.
pub fn bind_io_socket(local: Ipv4Addr) -> io::Result<UdpSocket> {
    UdpSocket::bind(SocketAddrV4::new(local, encap::IO_PORT))
}

/// An established Class 1 connection with its producer and consumer threads.
pub struct IoConnection {
    config: IoConfig,
    open: ForwardOpen,
    reply: ForwardOpenReply,
    target: SocketAddrV4,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    producer: Option<JoinHandle<()>>,
    consumer: Option<JoinHandle<()>>,
}

impl fmt::Debug for IoConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoConnection")
            .field("target", &self.target)
            .field("o_to_t_id", &self.reply.o_to_t_connection_id)
            .field("t_to_o_id", &self.reply.t_to_o_connection_id)
            .finish_non_exhaustive()
    }
}

impl IoConnection {
    /// Send `Forward_Open` over `session`, then start producing to `target`
    /// (the target's UDP I/O address, normally its IP at port 2222) and
    /// consuming on `socket`.
    ///
    /// # Errors
    /// `Refused` with the Connection Manager's extended status, or a
    /// session/transport error.
    pub fn open(
        session: &mut Session,
        socket: &UdpSocket,
        target: SocketAddrV4,
        config: IoConfig,
    ) -> Result<Self, Error> {
        let zeros = vec![0; usize::from(config.output_len)];
        Self::open_with_output(session, socket, target, config, &zeros)
    }

    /// As [`Self::open`], with `output` as the O->T data from the very first
    /// packet, so a reconnect re-sends what the outputs held instead of a
    /// zero glitch.
    ///
    /// # Errors
    /// As [`Self::open`]; `Io(InvalidInput)` if `output` is not
    /// `config.output_len` bytes.
    pub fn open_with_output(
        session: &mut Session,
        socket: &UdpSocket,
        target: SocketAddrV4,
        config: IoConfig,
        output: &[u8],
    ) -> Result<Self, Error> {
        if output.len() != usize::from(config.output_len) {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "initial output is {} bytes, connection carries {}",
                    output.len(),
                    config.output_len
                ),
            )));
        }
        let serial = next_serial();
        let t_to_o_id = u32::from(serial) << 16 | u32::from(config.originator_vendor_id);
        let open = config.forward_open(serial, t_to_o_id);
        let resp = send(session, &open.to_request())?;
        let reply = open_result(&resp)
            .map_err(session::Error::Decode)?
            .map_err(Error::Refused)?;

        let shared = Arc::new(Shared {
            output: Mutex::new(output.to_vec()),
            run: AtomicBool::new(true),
            input: Mutex::new(None),
            packets_in: AtomicU64::new(0),
            packets_out: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));

        let producer = spawn_producer(
            socket.try_clone()?,
            target,
            reply.o_to_t_connection_id,
            config.rpi,
            config.o_to_t_run_idle && config.output_len > 0,
            Arc::clone(&shared),
            Arc::clone(&stop),
        )?;
        let consumer = spawn_consumer(
            socket.try_clone()?,
            reply.t_to_o_connection_id,
            config.rpi,
            config.t_to_o_run_idle,
            Arc::clone(&shared),
            Arc::clone(&stop),
        )?;

        Ok(Self {
            config,
            open,
            reply,
            target,
            shared,
            stop,
            producer: Some(producer),
            consumer: Some(consumer),
        })
    }

    #[must_use]
    pub const fn reply(&self) -> &ForwardOpenReply {
        &self.reply
    }

    #[must_use]
    pub const fn config(&self) -> &IoConfig {
        &self.config
    }

    /// Replace the O->T application data. Sent on the next production.
    ///
    /// # Errors
    /// `InvalidInput` if `data.len()` differs from the configured output size.
    pub fn set_output(&self, data: &[u8]) -> io::Result<()> {
        if data.len() != usize::from(self.config.output_len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "output is {} bytes, connection carries {}",
                    data.len(),
                    self.config.output_len
                ),
            ));
        }
        if let Ok(mut g) = self.shared.output.lock() {
            g.clear();
            g.extend_from_slice(data);
        }
        Ok(())
    }

    /// Set the run/idle header bit for O->T data (ignored for heartbeats).
    pub fn set_run(&self, run: bool) {
        self.shared.run.store(run, Ordering::Relaxed);
    }

    #[must_use]
    pub fn latest_input(&self) -> Option<Input> {
        self.shared.input.lock().ok().and_then(|g| g.clone())
    }

    /// Block until a T->O packet newer than the ones counted so far arrives,
    /// or `timeout` passes.
    #[must_use]
    pub fn wait_for_input(&self, timeout: Duration) -> Option<Input> {
        let start = Instant::now();
        let before = self.shared.packets_in.load(Ordering::Relaxed);
        while start.elapsed() < timeout {
            if self.shared.packets_in.load(Ordering::Relaxed) > before {
                return self.latest_input();
            }
            thread::sleep(Duration::from_millis(1));
        }
        None
    }

    /// Block until the T->O data equals `expected`, or `timeout` passes.
    /// Returns the last input seen either way.
    #[must_use]
    pub fn wait_for_input_eq(&self, expected: &[u8], timeout: Duration) -> Option<Input> {
        let start = Instant::now();
        let mut last = None;
        while start.elapsed() < timeout {
            if let Some(i) = self.wait_for_input(timeout.saturating_sub(start.elapsed())) {
                if i.data == expected {
                    return Some(i);
                }
                last = Some(i);
            }
        }
        last
    }

    #[must_use]
    pub fn packets_received(&self) -> u64 {
        self.shared.packets_in.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn packets_sent(&self) -> u64 {
        self.shared.packets_out.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn packets_ignored(&self) -> u64 {
        self.shared.ignored.load(Ordering::Relaxed)
    }

    /// True while T->O data has arrived within the inactivity timeout.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.latest_input()
            .is_some_and(|i| i.received.elapsed() < self.config.timeout())
    }

    fn stop_threads(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.producer.take() {
            let _ = h.join();
        }
        if let Some(h) = self.consumer.take() {
            let _ = h.join();
        }
    }

    /// Stop producing, then send `Forward_Close` over `session`.
    ///
    /// # Errors
    /// A refused close (unknown connection) or session error.
    pub fn close(mut self, session: &mut Session) -> Result<ForwardCloseReply, Error> {
        self.stop_threads();
        self.closer().send(session)
    }

    /// What closes this connection later, over any session from this
    /// host: drop the connection (its threads stop), keep the closer, and
    /// send it once a session is up again. A target that lost the
    /// connection meanwhile answers "not found", which is fine.
    #[must_use]
    pub fn closer(&self) -> Closer {
        Closer(ForwardClose {
            priority_time_tick: self.open.priority_time_tick,
            timeout_ticks: self.open.timeout_ticks,
            connection_serial: self.open.connection_serial,
            originator_vendor_id: self.open.originator_vendor_id,
            originator_serial: self.open.originator_serial,
            path: self.open.path.clone(),
        })
    }
}

/// A `Forward_Close` for one connection this originator opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closer(ForwardClose);

impl Closer {
    /// Sends the `Forward_Close`.
    ///
    /// # Errors
    /// A refused close (unknown connection) or session error.
    pub fn send(&self, session: &mut Session) -> Result<ForwardCloseReply, Error> {
        let resp = send(session, &self.0.to_request())?;
        close_result(&resp)
            .map_err(session::Error::Decode)?
            .map_err(Error::Refused)
    }
}

/// The only requests this module puts on the wire: `Forward_Open` and
/// `Forward_Close`, built above with an Assembly-class path.
fn send(
    session: &mut Session,
    request: &crate::cip::Request,
) -> Result<crate::cip::Response, session::Error> {
    let mut mr = crate::codec::Writer::default();
    request.encode(&mut mr);
    session.exchange(&mr.into_inner())
}

impl Drop for IoConnection {
    fn drop(&mut self) {
        self.stop_threads();
    }
}

fn spawn_producer(
    socket: UdpSocket,
    target: SocketAddrV4,
    id: u32,
    rpi: Duration,
    header: bool,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("helmsman-producer".into())
        .spawn(move || {
            let mut encap_seq: u32 = 0;
            let mut seq: u16 = 0;
            while !stop.load(Ordering::Relaxed) {
                encap_seq = encap_seq.wrapping_add(1);
                seq = seq.wrapping_add(1);
                let data = shared.output.lock().map(|g| g.clone()).unwrap_or_default();
                let run_idle = header.then(|| {
                    if shared.run.load(Ordering::Relaxed) {
                        Class1Body::RUN
                    } else {
                        Class1Body::IDLE
                    }
                });
                let body = Class1Body {
                    sequence_count: seq,
                    run_idle,
                    data: &data,
                }
                .encode();
                let pkt = IoPacket {
                    connection_id: id,
                    encapsulation_sequence: Some(encap_seq),
                    body: &body,
                }
                .encode();
                if socket.send_to(&pkt, target).is_ok() {
                    shared.packets_out.fetch_add(1, Ordering::Relaxed);
                }
                thread::sleep(rpi);
            }
        })
}

fn spawn_consumer(
    socket: UdpSocket,
    id: u32,
    rpi: Duration,
    header: bool,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    socket.set_read_timeout(Some(rpi.max(Duration::from_millis(10))))?;
    thread::Builder::new()
        .name("helmsman-consumer".into())
        .spawn(move || {
            let mut buf = vec![0u8; 65535];
            while !stop.load(Ordering::Relaxed) {
                let Ok((n, _from)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                let parsed = IoPacket::decode(&buf[..n])
                    .ok()
                    .filter(|p| p.connection_id == id)
                    .and_then(|p| {
                        Class1Body::decode(p.body, header)
                            .ok()
                            .map(|b| (b.sequence_count, b.run_idle, b.data.to_vec()))
                    });
                match parsed {
                    Some((sequence_count, run_idle, data)) => {
                        if let Ok(mut g) = shared.input.lock() {
                            *g = Some(Input {
                                data,
                                sequence_count,
                                run_idle,
                                received: Instant::now(),
                            });
                        }
                        shared.packets_in.fetch_add(1, Ordering::Relaxed);
                    }
                    None => {
                        shared.ignored.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        })
}

/// Process-wide connection serial numbers so two connections from one
/// originator never collide (targets key on serial + vendor + originator serial).
fn next_serial() -> u16 {
    use std::sync::atomic::AtomicU16;
    static NEXT: AtomicU16 = AtomicU16::new(1);
    let s = NEXT.fetch_add(1, Ordering::Relaxed);
    if s == 0 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    } else {
        s
    }
}
