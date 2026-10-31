//! One Class 1 I/O endpoint per host: a single UDP socket (2222 by
//! default) shared by everything on the box that consumes I/O datagrams,
//! each datagram routed by the connection ID in its address item.
//!
//! A box is a CIP target (its adapter consumes O->T from the scanners that
//! own it) and an originator (`enip-io` consumes T->O from the devices it
//! masters). Both kinds of datagram arrive at the same UDP port, and the
//! connection ID in each one was chosen by *its consumer*, which is always
//! this box:
//!
//! - O->T, point-to-point: the target chooses the ID ("if we have a point to
//!   point connection for the O to T direction the target shall choose the
//!   connection ID", `OpENer` `cipconnectionobject.c`
//!   `ConnectionObjectGeneralConfiguration`) and finds the connection of an
//!   incoming datagram by it (`cipconnectionmanager.c` `GetConnectedObject`,
//!   which compares the consumed connection ID).
//! - T->O, point-to-point: the originator's ID from the `Forward_Open` is the
//!   target's produced ID (`ConnectionObjectInitializeFromMessage` reads
//!   the `T_to_O` connection ID into `cip_produced_connection_id`), which the target
//!   puts in every T->O datagram (`cipioconnection.c` `SendConnectedData`).
//!   `EIPScanner`'s originator dispatches T->O by the reply's T->O ID
//!   (`src/ConnectionManager.cpp`, `_connectionMap`).
//!
//! So the box picks every ID it will ever be asked to route; [`Router`]
//! hands them out ([`Router::allocate`]) so the adapter's and the
//! originator's never collide, and refuses a second owner for an ID.
//!
//! Every route is bound to the peer IP the datagrams must come from (the
//! scanner that opened the adapter's connection, the device `enip-io`
//! opened), as `OpENer`'s `HandleReceivedConnectedData` checks
//! `originator_address` against the sender. A datagram with an unknown ID,
//! from the wrong IP, or that does not parse is dropped and counted; it
//! never reaches any owner. UDP source addresses can be forged on the LAN;
//! this is the same check every EtherNet/IP stack makes, not more.
//!
//! [`Router`] is the sans-IO core; [`IoEndpoint`] puts it on a socket with
//! one receive thread and a bounded queue per owner. A full queue drops its
//! *oldest* datagram: cyclic I/O only ever wants the newest one, and one
//! owner's flood never touches another owner's queue.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::io::IoPacket;

/// Who consumes a connection's datagrams: one per [`Owner`] handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OwnerId(pub u32);

/// Where one connection ID's datagrams go, and who may send them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub owner: OwnerId,
    pub peer: Ipv4Addr,
}

/// What [`Router::route`] decided for one datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Deliver {
        owner: OwnerId,
        connection_id: u32,
    },
    /// Not an address item plus a connected data item.
    Malformed,
    /// No connection has this ID.
    Unknown(u32),
    /// The ID is someone's, but the datagram came from another IP.
    WrongPeer {
        connection_id: u32,
        owner: OwnerId,
    },
}

/// The ID is already routed to another owner (or to this one, elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Taken {
    pub connection_id: u32,
    pub owner: OwnerId,
}

impl std::fmt::Display for Taken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "connection id 0x{:08X} already routed to owner {}",
            self.connection_id, self.owner.0
        )
    }
}

impl std::error::Error for Taken {}

/// The routing table: connection ID -> owner and peer. Sans-IO.
#[derive(Debug, Clone)]
pub struct Router {
    routes: HashMap<u32, Route>,
    next_id: u32,
}

impl Router {
    /// IDs from [`Self::allocate`] start at `seed` (`OpENer`'s
    /// `GetConnectionId` puts a per-boot incarnation in the upper half the
    /// same way, so a reboot doesn't reuse the IDs of a minute ago).
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            routes: HashMap::new(),
            next_id: seed,
        }
    }

    /// A connection ID no route uses, never 0, never the same twice
    /// until the counter wraps.
    pub fn allocate(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            if id != 0 && !self.routes.contains_key(&id) {
                return id;
            }
        }
    }

    /// Route `connection_id` to `owner`, from `peer` only.
    ///
    /// # Errors
    /// [`Taken`] if any owner already has the ID.
    pub fn register(&mut self, connection_id: u32, owner: OwnerId, peer: Ipv4Addr) -> Result<(), Taken> {
        if let Some(r) = self.routes.get(&connection_id) {
            return Err(Taken {
                connection_id,
                owner: r.owner,
            });
        }
        self.routes.insert(connection_id, Route { owner, peer });
        Ok(())
    }

    /// Forget `connection_id` if `owner` has it; another owner's route
    /// stays. Returns whether a route went.
    pub fn unregister(&mut self, connection_id: u32, owner: OwnerId) -> bool {
        if self.routes.get(&connection_id).is_some_and(|r| r.owner == owner) {
            self.routes.remove(&connection_id);
            true
        } else {
            false
        }
    }

    /// Forget every route `owner` has.
    pub fn unregister_all(&mut self, owner: OwnerId) {
        self.routes.retain(|_, r| r.owner != owner);
    }

    /// Make `owner`'s routes exactly `want` (ID, peer): add the missing,
    /// drop the gone, re-bind a changed peer. IDs another owner holds are
    /// skipped and returned.
    pub fn sync(&mut self, owner: OwnerId, want: &[(u32, Ipv4Addr)]) -> Vec<Taken> {
        self.routes
            .retain(|id, r| r.owner != owner || want.iter().any(|(w, _)| w == id));
        let mut taken = Vec::new();
        for &(id, peer) in want {
            match self.routes.get_mut(&id) {
                Some(r) if r.owner == owner => r.peer = peer,
                Some(r) => taken.push(Taken {
                    connection_id: id,
                    owner: r.owner,
                }),
                None => {
                    self.routes.insert(id, Route { owner, peer });
                }
            }
        }
        taken
    }

    #[must_use]
    pub fn get(&self, connection_id: u32) -> Option<Route> {
        self.routes.get(&connection_id).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Decide where one datagram from `from` goes.
    #[must_use]
    pub fn route(&self, datagram: &[u8], from: Ipv4Addr) -> Verdict {
        let Ok(pkt) = IoPacket::decode(datagram) else {
            return Verdict::Malformed;
        };
        let connection_id = pkt.connection_id;
        match self.routes.get(&connection_id) {
            None => Verdict::Unknown(connection_id),
            Some(r) if r.peer != from => Verdict::WrongPeer {
                connection_id,
                owner: r.owner,
            },
            Some(r) => Verdict::Deliver {
                owner: r.owner,
                connection_id,
            },
        }
    }
}

/// One datagram handed to its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    pub from: SocketAddrV4,
    pub connection_id: u32,
    pub bytes: Vec<u8>,
}

/// Counters for everything the endpoint received.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub received: u64,
    pub delivered: u64,
    pub malformed: u64,
    pub unknown: u64,
    pub wrong_peer: u64,
    /// Datagrams pushed out of a full owner queue (the oldest go first).
    pub overflow: u64,
}

#[derive(Default)]
struct Counters {
    received: AtomicU64,
    delivered: AtomicU64,
    malformed: AtomicU64,
    unknown: AtomicU64,
    wrong_peer: AtomicU64,
    overflow: AtomicU64,
}

struct Queue {
    items: Mutex<VecDeque<Datagram>>,
    ready: Condvar,
    overflow: AtomicU64,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, VecDeque<Datagram>> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Datagrams each owner may have waiting before the oldest are dropped:
/// a few cycles of every connection one owner is likely to hold.
pub const QUEUE_LEN: usize = 256;

struct Inner {
    socket: UdpSocket,
    local: SocketAddrV4,
    router: Arc<Mutex<Router>>,
    queues: Mutex<HashMap<OwnerId, Arc<Queue>>>,
    next_owner: AtomicU32,
    counters: Counters,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let handle = self
            .thread
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(h) = handle
            && h.thread().id() != thread::current().id()
        {
            let _ = h.join();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The box's one I/O socket. Cheap to clone; the socket closes when the
/// last clone and the last [`Owner`] are gone.
#[derive(Clone)]
pub struct IoEndpoint {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for IoEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IoEndpoint")
            .field("local", &self.inner.local)
            .finish_non_exhaustive()
    }
}

fn seed() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| {
            d.subsec_nanos() ^ u32::try_from(d.as_secs() & 0xFFFF_FFFF).unwrap_or(0)
        });
    (nanos & 0xFFFF) << 16 | 1
}

impl IoEndpoint {
    /// Bind the I/O socket at `addr` (port 2222 for a real box; 0 picks one)
    /// and start routing.
    ///
    /// # Errors
    /// Bind failure, typically another process on the port.
    pub fn bind(addr: SocketAddrV4) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        Self::from_socket(socket)
    }

    /// Route datagrams from an already-bound socket.
    ///
    /// # Errors
    /// An IPv6 socket, or a socket that can't be cloned for the thread.
    pub fn from_socket(socket: UdpSocket) -> io::Result<Self> {
        let SocketAddr::V4(local) = socket.local_addr()? else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "IPv6 not supported"));
        };
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let rx = socket.try_clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        let inner = Arc::new(Inner {
            socket,
            local,
            router: Arc::new(Mutex::new(Router::new(seed()))),
            queues: Mutex::new(HashMap::new()),
            next_owner: AtomicU32::new(1),
            counters: Counters::default(),
            stop: Arc::clone(&stop),
            thread: Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        let handle = thread::Builder::new()
            .name("cip-io-endpoint".into())
            .spawn(move || receive_loop(&rx, &weak, &stop))?;
        *lock(&inner.thread) = Some(handle);
        Ok(Self { inner })
    }

    #[must_use]
    pub fn local_addr(&self) -> SocketAddrV4 {
        self.inner.local
    }

    /// A new owner with an empty queue and no routes.
    #[must_use]
    pub fn owner(&self) -> Owner {
        let id = OwnerId(self.inner.next_owner.fetch_add(1, Ordering::Relaxed));
        let queue = Arc::new(Queue {
            items: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            overflow: AtomicU64::new(0),
        });
        lock(&self.inner.queues).insert(id, Arc::clone(&queue));
        Owner {
            ep: self.clone(),
            id,
            queue,
        }
    }

    /// Hands out connection IDs no route uses, without keeping the socket
    /// alive (for a target state machine that picks its own O->T IDs).
    #[must_use]
    pub fn allocator(&self) -> IdAllocator {
        IdAllocator(Arc::downgrade(&self.inner.router))
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        let c = &self.inner.counters;
        Stats {
            received: c.received.load(Ordering::Relaxed),
            delivered: c.delivered.load(Ordering::Relaxed),
            malformed: c.malformed.load(Ordering::Relaxed),
            unknown: c.unknown.load(Ordering::Relaxed),
            wrong_peer: c.wrong_peer.load(Ordering::Relaxed),
            overflow: c.overflow.load(Ordering::Relaxed),
        }
    }

    /// The route for `connection_id`, if any.
    #[must_use]
    pub fn route_of(&self, connection_id: u32) -> Option<Route> {
        lock(&self.inner.router).get(connection_id)
    }
}

/// See [`IoEndpoint::allocator`].
#[derive(Clone)]
pub struct IdAllocator(Weak<Mutex<Router>>);

impl std::fmt::Debug for IdAllocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdAllocator")
    }
}

impl IdAllocator {
    /// A fresh ID, or `None` once the endpoint is gone.
    #[must_use]
    pub fn allocate(&self) -> Option<u32> {
        self.0.upgrade().map(|r| lock(&r).allocate())
    }
}

fn receive_loop(socket: &UdpSocket, weak: &Weak<Inner>, stop: &AtomicBool) {
    let mut buf = vec![0u8; 65535];
    while !stop.load(Ordering::Relaxed) {
        let got = socket.recv_from(&mut buf);
        let Some(inner) = weak.upgrade() else { return };
        let Ok((n, from)) = got else { continue };
        let c = &inner.counters;
        c.received.fetch_add(1, Ordering::Relaxed);
        let SocketAddr::V4(from) = from else {
            c.malformed.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let verdict = lock(&inner.router).route(&buf[..n], *from.ip());
        match verdict {
            Verdict::Deliver { owner, connection_id } => {
                let Some(q) = lock(&inner.queues).get(&owner).cloned() else {
                    c.unknown.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let mut items = q.lock();
                if items.len() >= QUEUE_LEN {
                    items.pop_front();
                    q.overflow.fetch_add(1, Ordering::Relaxed);
                    c.overflow.fetch_add(1, Ordering::Relaxed);
                }
                items.push_back(Datagram {
                    from,
                    connection_id,
                    bytes: buf[..n].to_vec(),
                });
                drop(items);
                q.ready.notify_one();
                c.delivered.fetch_add(1, Ordering::Relaxed);
            }
            Verdict::Malformed => {
                c.malformed.fetch_add(1, Ordering::Relaxed);
            }
            Verdict::Unknown(_) => {
                c.unknown.fetch_add(1, Ordering::Relaxed);
            }
            Verdict::WrongPeer { .. } => {
                c.wrong_peer.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// One consumer of the endpoint: the adapter, or one originator
/// connection. Its routes go when it is dropped.
pub struct Owner {
    ep: IoEndpoint,
    id: OwnerId,
    queue: Arc<Queue>,
}

impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owner")
            .field("id", &self.id)
            .field("local", &self.ep.inner.local)
            .finish_non_exhaustive()
    }
}

impl Owner {
    #[must_use]
    pub const fn id(&self) -> OwnerId {
        self.id
    }

    #[must_use]
    pub const fn endpoint(&self) -> &IoEndpoint {
        &self.ep
    }

    #[must_use]
    pub fn local_addr(&self) -> SocketAddrV4 {
        self.ep.inner.local
    }

    /// A connection ID nobody on this endpoint uses (not yet registered).
    #[must_use]
    pub fn allocate(&self) -> u32 {
        lock(&self.ep.inner.router).allocate()
    }

    /// Route `connection_id`'s datagrams from `peer` to this owner.
    ///
    /// # Errors
    /// `AddrInUse` if the ID is already routed.
    pub fn register(&self, connection_id: u32, peer: Ipv4Addr) -> io::Result<()> {
        lock(&self.ep.inner.router)
            .register(connection_id, self.id, peer)
            .map_err(|t| io::Error::new(io::ErrorKind::AddrInUse, t.to_string()))
    }

    /// Stop routing `connection_id` here (if this owner has it).
    #[must_use = "false: this owner did not have the ID"]
    pub fn unregister(&self, connection_id: u32) -> bool {
        lock(&self.ep.inner.router).unregister(connection_id, self.id)
    }

    /// Make this owner's routes exactly `want`; see [`Router::sync`].
    #[must_use = "IDs another owner holds stay unrouted here"]
    pub fn sync(&self, want: &[(u32, Ipv4Addr)]) -> Vec<Taken> {
        lock(&self.ep.inner.router).sync(self.id, want)
    }

    /// The next datagram for this owner, waiting up to `timeout`.
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Datagram> {
        let items = self.queue.lock();
        let (mut items, _) = self
            .queue
            .ready
            .wait_timeout_while(items, timeout, |q| q.is_empty())
            .unwrap_or_else(PoisonError::into_inner);
        items.pop_front()
    }

    /// Send from the endpoint's socket (so the peer sees the shared port).
    ///
    /// # Errors
    /// The socket's.
    pub fn send_to(&self, bytes: &[u8], to: SocketAddrV4) -> io::Result<usize> {
        self.ep.inner.socket.send_to(bytes, to)
    }

    /// Datagrams dropped from this owner's full queue.
    #[must_use]
    pub fn overflow(&self) -> u64 {
        self.queue.overflow.load(Ordering::Relaxed)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        lock(&self.ep.inner.router).unregister_all(self.id);
        lock(&self.ep.inner.queues).remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(id: u32) -> Vec<u8> {
        IoPacket {
            connection_id: id,
            encapsulation_sequence: Some(1),
            body: &[1, 0, 0xAA],
        }
        .encode()
    }

    const A: OwnerId = OwnerId(1);
    const B: OwnerId = OwnerId(2);
    const SCANNER: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 5);
    const DEVICE: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 60);

    #[test]
    fn routes_by_id_and_peer() {
        let mut r = Router::new(1);
        r.register(7, A, SCANNER).unwrap();
        r.register(8, B, DEVICE).unwrap();
        assert_eq!(
            r.route(&pkt(7), SCANNER),
            Verdict::Deliver {
                owner: A,
                connection_id: 7
            }
        );
        assert_eq!(
            r.route(&pkt(8), DEVICE),
            Verdict::Deliver {
                owner: B,
                connection_id: 8
            }
        );
        // B's ID from A's peer: refused, not delivered to anyone.
        assert_eq!(
            r.route(&pkt(8), SCANNER),
            Verdict::WrongPeer {
                connection_id: 8,
                owner: B
            }
        );
        assert_eq!(r.route(&pkt(9), DEVICE), Verdict::Unknown(9));
        assert_eq!(r.route(&[1, 2, 3], DEVICE), Verdict::Malformed);
    }

    #[test]
    fn an_id_has_one_owner_and_only_it_can_drop_it() {
        let mut r = Router::new(1);
        r.register(7, A, SCANNER).unwrap();
        assert_eq!(
            r.register(7, B, DEVICE),
            Err(Taken {
                connection_id: 7,
                owner: A
            })
        );
        assert!(!r.unregister(7, B));
        assert_eq!(r.get(7).map(|x| x.owner), Some(A));
        assert!(r.unregister(7, A));
        assert!(r.is_empty());
    }

    #[test]
    fn allocate_skips_taken_ids_and_zero() {
        let mut r = Router::new(u32::MAX);
        r.register(1, A, SCANNER).unwrap();
        assert_eq!(r.allocate(), u32::MAX);
        // wraps past 0 and the taken 1
        assert_eq!(r.allocate(), 2);
    }

    #[test]
    fn sync_replaces_only_its_own_routes() {
        let mut r = Router::new(1);
        r.register(1, A, SCANNER).unwrap();
        r.register(2, B, DEVICE).unwrap();
        let taken = r.sync(A, &[(3, SCANNER), (2, SCANNER)]);
        assert_eq!(
            taken,
            vec![Taken {
                connection_id: 2,
                owner: B
            }]
        );
        assert_eq!(r.get(1), None);
        assert_eq!(r.get(2).map(|x| x.owner), Some(B));
        assert_eq!(r.get(3).map(|x| x.owner), Some(A));
        r.unregister_all(A);
        assert_eq!(r.len(), 1);
    }
}
