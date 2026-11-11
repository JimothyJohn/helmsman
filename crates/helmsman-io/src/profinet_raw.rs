//! The wire under a `profinet-io` controller on a real interface: raw
//! Ethernet frames of `EtherType` 0x8892 (`helmsman-l2`) and the box's
//! one UDP socket at the PROFINET context-manager port.
//!
//! **One port for every device.** A device answers the controller's
//! calls at whatever port they came from, but sends its own call
//! ("application ready") to port 34964 on the controller, so every link
//! of a box shares one socket bound there ([`RpcPort`]). A datagram is
//! handed to the link that last sent to its source address; one from an
//! address no link has written to is dropped.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use helmsman_l2::RawSocket;
use helmsman_profinet::dcp::Ip;
use helmsman_profinet::eth::{ETHERTYPE_PROFINET, Mac};
use helmsman_profinet::rpc;

use super::{Event, Link};

/// How long one wait on the interface lasts before the UDP socket is
/// looked at again: the most a datagram waits for a link that is idle.
const SLICE: Duration = Duration::from_millis(2);
/// Datagrams kept per device while its link is busy; the oldest go first.
const QUEUE: usize = 16;

/// A device address's owner (a link's id) and the datagrams waiting for
/// it, each with the port it came from.
type Peer = (u64, VecDeque<(u16, Vec<u8>)>);

/// The box's socket at port 34964, shared by its `profinet-io` links.
pub struct RpcPort {
    socket: UdpSocket,
    /// Per device address: which link owns it, and what waits for it.
    peers: Mutex<BTreeMap<Ip, Peer>>,
}

static PORT: OnceLock<Mutex<Weak<RpcPort>>> = OnceLock::new();

impl RpcPort {
    /// The process's port, bound on first use and closed when the last
    /// link lets go of it.
    ///
    /// # Errors
    /// The port is taken (another controller on this host).
    pub fn shared() -> io::Result<Arc<Self>> {
        let mut slot = PORT
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(port) = slot.upgrade() {
            return Ok(port);
        }
        let port = Arc::new(Self::bind(rpc::PORT)?);
        *slot = Arc::downgrade(&port);
        Ok(port)
    }

    /// A port of its own at `port` (0 for any free one): what `shared`
    /// binds once per process, and what a test binds beside other tests.
    ///
    /// # Errors
    /// The port is taken.
    pub fn bind(port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("UDP port {port} (the PROFINET context manager): {e}"),
            )
        })?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            peers: Mutex::new(BTreeMap::new()),
        })
    }

    /// The port it is bound to.
    ///
    /// # Errors
    /// The socket no longer has an address.
    pub fn local_port(&self) -> io::Result<u16> {
        self.socket.local_addr().map(|a| a.port())
    }

    /// Sends to a device as link `owner`, which from now on is handed
    /// what that device sends.
    ///
    /// # Errors
    /// The socket refused the datagram.
    pub fn send(&self, owner: u64, ip: Ip, port: u16, data: &[u8]) -> io::Result<()> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(ip)
            .or_insert_with(|| (owner, VecDeque::new()))
            .0 = owner;
        self.socket
            .send_to(data, SocketAddrV4::new(Ipv4Addr::from(ip), port))
            .map(|_| ())
    }

    /// Sorts what has arrived into the devices' queues and takes the
    /// next datagram for `owner`.
    ///
    /// # Errors
    /// The socket failed.
    pub fn take(&self, owner: u64) -> io::Result<Option<Event>> {
        let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
        let mut buf = [0u8; 1600];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, SocketAddr::V4(from))) => {
                    if let Some((_, queue)) = peers.get_mut(&from.ip().octets()) {
                        if queue.len() == QUEUE {
                            queue.pop_front();
                        }
                        queue.push_back((from.port(), buf[..n].to_vec()));
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // A device that is not listening yet answers a datagram
                // with ICMP, which some systems report on the next read.
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {}
                Err(e) => return Err(e),
            }
        }
        for (ip, (who, queue)) in peers.iter_mut() {
            if *who == owner
                && let Some((port, data)) = queue.pop_front()
            {
                return Ok(Some(Event::Udp { ip: *ip, port, data }));
            }
        }
        Ok(None)
    }

    /// Drops every device `owner` talked to, and what waits for it.
    pub fn forget(&self, owner: u64) {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|_, (who, _)| *who != owner);
    }
}

/// One controller's link on a real interface.
pub struct RawLink {
    raw: RawSocket,
    port: Arc<RpcPort>,
    id: u64,
}

impl RawLink {
    /// Opens `interface` for PROFINET frames and joins the box's port.
    ///
    /// # Errors
    /// No such interface, no permission for raw Ethernet (the text says
    /// what to grant), or UDP port 34964 is taken.
    pub fn open(interface: &str) -> io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let raw = RawSocket::open(interface, ETHERTYPE_PROFINET)
            .map_err(|e| io::Error::new(e.kind(), format!("{interface}: {e}")))?;
        Ok(Self {
            raw,
            port: RpcPort::shared()?,
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        })
    }
}

impl Link for RawLink {
    fn mac(&self) -> Mac {
        self.raw.mac()
    }

    fn send_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        self.raw.send(frame)
    }

    fn send_udp(&mut self, ip: Ip, port: u16, data: &[u8]) -> io::Result<()> {
        self.port.send(self.id, ip, port, data)
    }

    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(e) = self.port.take(self.id)? {
                return Ok(Some(e));
            }
            if let Some(frame) = self.raw.recv(SLICE)? {
                return Ok(Some(Event::Frame(frame)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}

impl Drop for RawLink {
    fn drop(&mut self) {
        self.port.forget(self.id);
    }
}
