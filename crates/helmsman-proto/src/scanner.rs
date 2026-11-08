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

use crate::class1::{self, Consumer, Producer};
use crate::cpf;
use crate::encap;
use crate::endpoint::{IoEndpoint, Owner};
use crate::explicit;
use crate::io::{
    ForwardClose, ForwardCloseReply, ForwardOpen, ForwardOpenReply, Refused, close_result, open_result,
};
use crate::session::{self, Session};

pub use crate::class1::{IoConfig, OutputLen, Received};

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
    producer: Mutex<Producer>,
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
    /// Holds this connection's route on the shared endpoint
    /// ([`Self::open_on`]); `None` on a socket of its own.
    _route: Option<Arc<Owner>>,
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
    /// (the target's UDP I/O address, normally its IP at port 2222, unless
    /// the reply's O->T socket-address item names another; [`Self::target`])
    /// and consuming on `socket`.
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
        config.check_output(output).map_err(invalid_input)?;
        let rx = socket.try_clone()?;
        rx.set_read_timeout(Some(config.rpi.max(Duration::from_millis(10))))?;
        let tx = socket.try_clone()?;
        let serial = next_serial();
        let t_to_o_id = u32::from(serial) << 16 | u32::from(config.originator_vendor_id);
        let open = config.forward_open(serial, t_to_o_id);
        let (resp, items) = send(session, &open.to_request(), &[])?;
        let reply = open_result(&resp)
            .map_err(session::Error::Decode)?
            .map_err(Error::Refused)?;
        let target = class1::o_to_t_target(&items, target);
        let mut buf = vec![0u8; 65535];
        let recv: Recv = Box::new(move |_wait| {
            let (n, from) = rx.recv_from(&mut buf).ok()?;
            Some((buf[..n].to_vec(), from))
        });
        let send_to: Send = Box::new(move |bytes, to| tx.send_to(bytes, to).map(|_| ()));
        Self::start(config, open, reply, target, output, send_to, recv, None)
    }

    /// As [`Self::open_with_output`], on the box's shared I/O endpoint
    /// instead of a socket of its own: the T->O connection ID comes from
    /// the endpoint (so it never collides with the box adapter's), is
    /// routed to this connection from `target`'s IP only, and O->T leaves
    /// from the endpoint's port. If the endpoint is not on 2222 the
    /// `Forward_Open` carries a T->O socket-address item naming its port
    /// (`OpENer` `cipioconnection.c` `OpenProducingPointToPointConnection`
    /// sends T->O there; without one it uses 2222).
    ///
    /// # Errors
    /// As [`Self::open_with_output`]; `Io(AddrInUse)` if the device
    /// replies with a T->O connection ID the endpoint already routes.
    pub fn open_on(
        session: &mut Session,
        endpoint: &IoEndpoint,
        target: SocketAddrV4,
        config: IoConfig,
        output: &[u8],
    ) -> Result<Self, Error> {
        config.check_output(output).map_err(invalid_input)?;
        let owner = Arc::new(endpoint.owner());
        let serial = next_serial();
        let t_to_o_id = owner.allocate();
        owner.register(t_to_o_id, *target.ip())?;
        let open = config.forward_open(serial, t_to_o_id);
        let port = endpoint.local_addr().port();
        let sockaddr = class1::t_to_o_sockaddr(port);
        let item = [cpf::Item {
            type_id: cpf::item::SOCKADDR_T_TO_O,
            data: &sockaddr,
        }];
        let extra: &[cpf::Item<'_>] = if port == encap::IO_PORT { &[] } else { &item };
        let (resp, items) = send(session, &open.to_request(), extra)?;
        let reply = open_result(&resp)
            .map_err(session::Error::Decode)?
            .map_err(Error::Refused)?;
        let target = class1::o_to_t_target(&items, target);
        if reply.t_to_o_connection_id != t_to_o_id {
            // The device chose the T->O ID itself; route that one instead.
            let _ = owner.unregister(t_to_o_id);
            if let Err(e) = owner.register(reply.t_to_o_connection_id, *target.ip()) {
                let _ = Closer(class1::close_for(&open)).send(session);
                return Err(Error::Io(e));
            }
        }
        let rx = Arc::clone(&owner);
        let recv: Recv = Box::new(move |wait| {
            rx.recv_timeout(wait)
                .map(|d| (d.bytes, std::net::SocketAddr::V4(d.from)))
        });
        let tx = Arc::clone(&owner);
        let send_to: Send = Box::new(move |bytes, to| tx.send_to(bytes, to).map(|_| ()));
        Self::start(config, open, reply, target, output, send_to, recv, Some(owner))
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        config: IoConfig,
        open: ForwardOpen,
        reply: ForwardOpenReply,
        target: SocketAddrV4,
        output: &[u8],
        send_to: Send,
        recv: Recv,
        owner: Option<Arc<Owner>>,
    ) -> Result<Self, Error> {
        let producer = Producer::new(&config, reply.o_to_t_connection_id, output).map_err(invalid_input)?;
        let shared = Arc::new(Shared {
            producer: Mutex::new(producer),
            input: Mutex::new(None),
            packets_in: AtomicU64::new(0),
            packets_out: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
        });
        let stop = Arc::new(AtomicBool::new(false));

        let producer = spawn_producer(
            send_to,
            target,
            config.rpi,
            Arc::clone(&shared),
            Arc::clone(&stop),
        )?;
        let consumer = spawn_consumer(
            recv,
            Consumer::new(&config, reply.t_to_o_connection_id),
            config.rpi,
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
            _route: owner,
        })
    }

    #[must_use]
    pub const fn reply(&self) -> &ForwardOpenReply {
        &self.reply
    }

    /// Where O->T data goes: the target given at open, or what its
    /// `Forward_Open` reply's O->T socket-address item named instead.
    #[must_use]
    pub const fn target(&self) -> SocketAddrV4 {
        self.target
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
        let mut g = self
            .shared
            .producer
            .lock()
            .map_err(|_| io::Error::other("producer poisoned"))?;
        g.set_output(data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))
    }

    /// Set the run/idle header bit for O->T data (ignored for heartbeats).
    pub fn set_run(&self, run: bool) {
        if let Ok(mut g) = self.shared.producer.lock() {
            g.set_run(run);
        }
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
        Closer(class1::close_for(&self.open))
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
        let (resp, _) = send(session, &self.0.to_request(), &[])?;
        close_result(&resp)
            .map_err(session::Error::Decode)?
            .map_err(Error::Refused)
    }
}

fn invalid_input(e: OutputLen) -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))
}

/// Sends one datagram to an address.
type Send = Box<dyn FnMut(&[u8], SocketAddrV4) -> io::Result<()> + std::marker::Send>;
/// Waits up to the given time for the next datagram.
type Recv = Box<dyn FnMut(Duration) -> Option<(Vec<u8>, std::net::SocketAddr)> + std::marker::Send>;

/// The only requests this module puts on the wire: `Forward_Open` and
/// `Forward_Close`, built above with an Assembly-class path.
fn send(
    session: &mut Session,
    request: &crate::cip::Request,
    extra: &[cpf::Item<'_>],
) -> Result<(crate::cip::Response, Vec<explicit::OwnedItem>), session::Error> {
    let mut mr = crate::codec::Writer::default();
    request.encode(&mut mr);
    session.exchange_with_items(&mr.into_inner(), extra)
}

impl Drop for IoConnection {
    fn drop(&mut self) {
        self.stop_threads();
    }
}

fn spawn_producer(
    mut send_to: Send,
    target: SocketAddrV4,
    rpi: Duration,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("helmsman-producer".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some(pkt) = shared.producer.lock().ok().map(|mut g| g.next_packet()) else {
                    break;
                };
                if send_to(&pkt, target).is_ok() {
                    shared.packets_out.fetch_add(1, Ordering::Relaxed);
                }
                thread::sleep(rpi);
            }
        })
}

fn spawn_consumer(
    mut recv: Recv,
    mut consumer: Consumer,
    rpi: Duration,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    let wait = rpi.max(Duration::from_millis(10));
    thread::Builder::new()
        .name("helmsman-consumer".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let Some((bytes, _from)) = recv(wait) else {
                    continue;
                };
                match consumer.accept(&bytes) {
                    Some(r) => {
                        let input = Input {
                            data: r.data.clone(),
                            sequence_count: r.sequence_count,
                            run_idle: r.run_idle,
                            received: Instant::now(),
                        };
                        if let Ok(mut g) = shared.input.lock() {
                            *g = Some(input);
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
