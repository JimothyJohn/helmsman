//! The box's DHCP server on its device port (`--dhcp IFACE`, off by
//! default; TODO.md `box-dhcp-server`): a block that ships as a DHCP
//! client (Turck TBEN at rotary 600, Murrelektronik IMPACT67 E at 000)
//! gets an address with nobody touching it.
//!
//! A box setting, not a project's (Nick, 2026-11-13): which port faces the
//! devices is a fact about the box, and one project deployed to two boxes
//! must not start a server on whatever network the second one is plugged
//! into.
//!
//! **It serves only where nobody else does.** A second DHCP server on a
//! network someone else runs is an outage, so:
//!
//! 1. Both sockets are tied to the one interface named
//!    ([`helmsman_l2::bind_to_interface`]): nothing is heard from, or sent
//!    to, any other port of the box.
//! 2. Before it answers anyone it sends a DISCOVER of its own
//!    ([`helmsman_dhcp::guard::probe`]) and listens for [`PROBE_FOR`]; any
//!    other server's reply and it never starts ([`Status::Refused`]).
//! 3. While it serves it keeps listening on the client port; another
//!    server's reply there stops it the same way.
//!
//! A refused or stopped server stays off until the box is restarted: the
//! box says so in its log and at `GET /api/dhcp`. Relayed requests are
//! dropped by the server itself.
//!
//! Leases are kept in `<state-dir>/dhcp/leases` ([`helmsman_dhcp::leases`]),
//! written atomically after each change, so a block keeps its address
//! across box restarts.

use std::fs;
use std::io::{self, Write as _};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use helmsman_dhcp::server::{Binding, Event, State as BindingState};
use helmsman_dhcp::{CLIENT_PORT, Config, Dest, Mac, Message, SERVER_PORT, Server, guard, leases};

/// How long the box listens for another server before it serves.
pub const PROBE_FOR: Duration = Duration::from_secs(4);
/// The Ethernet address the probe is sent as: locally administered (the
/// second bit of the first byte), so no vendor's device has it, and the
/// server ignores requests from it.
pub const PROBE_MAC: Mac = Mac([0x02, 0x68, 0x6d, 0x64, 0x68, 0x63]);
/// The default lease: a day. A device renews at half of it.
pub const LEASE_S: u32 = 86_400;
/// The lease file, in `<state-dir>/dhcp/`.
pub const FILE: &str = "leases";
/// How long one wait for a datagram lasts before the loop looks again.
const TICK: Duration = Duration::from_millis(50);
/// A datagram longer than this is not a DHCP message anyone sends.
const MAX_DATAGRAM: usize = 1500;

type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// Where the server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Listening for another server before serving.
    Probing,
    /// Answering clients.
    Serving,
    /// Another server answered: this one does not serve until the box is
    /// restarted.
    Refused {
        /// That server's identifier (or the address its reply came from).
        server: Ipv4Addr,
        /// What it offered or gave.
        offered: Ipv4Addr,
    },
    /// The sockets failed.
    Failed(String),
}

/// What the box serves and where it keeps its leases.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The interface the devices are on, as the OS names it.
    pub interface: String,
    /// The server's configuration: its address on that interface, the
    /// mask, the pool, the lease length.
    pub config: Config,
    /// `<state-dir>/dhcp`.
    pub dir: PathBuf,
}

/// The two sockets and where things are sent: on a box, UDP 67 and 68 on
/// the device interface and the limited broadcast; a test hands in
/// loopback sockets and addresses.
#[derive(Debug)]
pub struct Wire {
    /// Bound to the server port: requests arrive here, replies leave here.
    pub server: UdpSocket,
    /// Bound to the client port: the probe leaves here, and every other
    /// server's replies arrive here.
    pub watch: UdpSocket,
    /// Where the probe goes (255.255.255.255:67).
    pub probe_to: SocketAddr,
    /// Where a broadcast reply goes (255.255.255.255:68).
    pub broadcast_to: SocketAddr,
    /// The client port a unicast reply goes to (68).
    pub client_port: u16,
}

impl Wire {
    /// UDP 67 and 68 on every address, tied to `interface`, broadcasts
    /// allowed.
    ///
    /// # Errors
    /// A bind fails: the port is taken (a DHCP client or server already
    /// runs on this host), no permission for a port under 1024
    /// (`CAP_NET_BIND_SERVICE` on Linux), or no such interface.
    pub fn open(interface: &str) -> io::Result<Self> {
        let open = |port: u16| -> io::Result<UdpSocket> {
            let s = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))
                .map_err(|e| io::Error::new(e.kind(), format!("UDP {port}: {e}")))?;
            helmsman_l2::bind_to_interface(&s, interface)
                .map_err(|e| io::Error::new(e.kind(), format!("UDP {port} on {interface}: {e}")))?;
            s.set_broadcast(true)?;
            Ok(s)
        };
        let broadcast = Ipv4Addr::BROADCAST;
        Ok(Self {
            server: open(SERVER_PORT)?,
            watch: open(CLIENT_PORT)?,
            probe_to: SocketAddr::from((broadcast, SERVER_PORT)),
            broadcast_to: SocketAddr::from((broadcast, CLIENT_PORT)),
            client_port: CLIENT_PORT,
        })
    }
}

struct Shared {
    interface: String,
    config: Config,
    status: Mutex<Status>,
    bindings: Mutex<Vec<(Mac, Binding)>>,
}

/// What the API reads: the status and the bindings.
#[derive(Clone)]
pub struct Handle(Arc<Shared>);

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("interface", &self.0.interface)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Handle {
    #[must_use]
    pub fn status(&self) -> Status {
        self.0
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn interface(&self) -> &str {
        &self.0.interface
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.0.config
    }

    /// The bindings, offers included, sorted by Ethernet address.
    #[must_use]
    pub fn bindings(&self) -> Vec<(Mac, Binding)> {
        self.0
            .bindings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `GET /api/dhcp`.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        let c = self.config();
        let (status, refused_by, error) = match self.status() {
            Status::Probing => ("probing", None, None),
            Status::Serving => ("serving", None, None),
            Status::Refused { server, offered } => (
                "refused",
                Some(serde_json::json!({ "server": server.to_string(), "offered": offered.to_string() })),
                None,
            ),
            Status::Failed(e) => ("failed", None, Some(e)),
        };
        let leases: Vec<serde_json::Value> = self
            .bindings()
            .into_iter()
            .map(|(mac, b)| {
                let state = match b.state {
                    BindingState::Offered => "offered",
                    BindingState::Bound => "bound",
                    BindingState::Released => "released",
                };
                let until = (b.until != helmsman_dhcp::server::FOREVER).then_some(b.until);
                serde_json::json!({ "mac": mac.to_string(), "ip": b.ip.to_string(), "state": state, "until": until })
            })
            .collect();
        serde_json::json!({
            "interface": self.interface(),
            "server": c.server.to_string(),
            "netmask": c.netmask.to_string(),
            "pool": [c.pool.0.to_string(), c.pool.1.to_string()],
            "lease_s": c.lease_s,
            "status": status,
            "refused_by": refused_by,
            "error": error,
            "leases": leases,
        })
    }
}

/// The running server; dropping it stops it.
pub struct Dhcp {
    handle: Handle,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Dhcp {
    /// Read the lease file, then probe for [`PROBE_FOR`]-like `probe_for`
    /// and serve, on a thread of its own.
    ///
    /// # Errors
    /// The configuration is refused ([`Config::check`]), the lease file is
    /// unreadable (not absent: a file that does not parse is an error,
    /// not an empty table, so no device loses its address to a typo), or
    /// the thread cannot start.
    pub fn start(settings: Settings, wire: Wire, probe_for: Duration, log: Log) -> io::Result<Self> {
        let mut server = Server::new(settings.config.clone())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        let path = settings.dir.join(FILE);
        match fs::read_to_string(&path) {
            Ok(text) => {
                let read = leases::parse(&text).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))
                })?;
                let skipped = server.restore(read);
                if skipped > 0 {
                    log(&format!(
                        "dhcp: {skipped} lease(s) in {} do not fit this configuration and were left out",
                        path.display()
                    ));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
        }
        wire.server.set_read_timeout(Some(TICK))?;
        wire.watch.set_read_timeout(Some(Duration::from_millis(1)))?;
        let shared = Arc::new(Shared {
            interface: settings.interface.clone(),
            config: settings.config.clone(),
            status: Mutex::new(Status::Probing),
            bindings: Mutex::new(server.bindings().iter().map(|(m, b)| (*m, *b)).collect()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            thread::Builder::new()
                .name("dhcp".into())
                .spawn(move || run(server, &wire, &settings, probe_for, &shared, &stop, &log))?
        };
        Ok(Self {
            handle: Handle(shared),
            stop,
            thread: Some(thread),
        })
    }

    #[must_use]
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

impl Drop for Dhcp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn set_status(shared: &Shared, s: Status) {
    *shared.status.lock().unwrap_or_else(PoisonError::into_inner) = s;
}

/// A reply heard on the client port, if it came from another server.
fn heard_foreign(wire: &Wire, own: Ipv4Addr, buf: &mut [u8]) -> Option<guard::Foreign> {
    let (n, from) = wire.watch.recv_from(buf).ok()?;
    let IpAddr::V4(src) = from.ip() else { return None };
    let m = Message::parse(&buf[..n]).ok()?;
    guard::foreign(&m, src, Some(own))
}

fn refuse(shared: &Shared, f: guard::Foreign, log: &Log, when: &str) {
    log(&format!(
        "dhcp: another DHCP server answers on {} ({} offered {}) {when}: not serving until the box restarts",
        shared.interface, f.server, f.yiaddr
    ));
    set_status(
        shared,
        Status::Refused {
            server: f.server,
            offered: f.yiaddr,
        },
    );
}

fn run(
    mut server: Server,
    wire: &Wire,
    settings: &Settings,
    probe_for: Duration,
    shared: &Shared,
    stop: &AtomicBool,
    log: &Log,
) {
    let own = settings.config.server;
    let mut buf = vec![0u8; MAX_DATAGRAM + 1];

    // 1. Probe: two DISCOVERs, half the wait apart.
    let xid = u32::try_from(now_s() & 0xffff_ffff).unwrap_or(0) ^ 0x6864_6370;
    let probe = guard::probe(xid, PROBE_MAC).encode();
    let t0 = Instant::now();
    let (mut sends, mut next_send) = (0u8, Duration::ZERO);
    while t0.elapsed() < probe_for {
        if stop.load(Ordering::Acquire) {
            return;
        }
        if sends < 2 && t0.elapsed() >= next_send {
            if let Err(e) = wire.watch.send_to(&probe, wire.probe_to) {
                log(&format!(
                    "dhcp: cannot send the probe on {}: {e}",
                    shared.interface
                ));
                set_status(
                    shared,
                    Status::Failed(format!("the probe could not be sent: {e}")),
                );
                return;
            }
            sends += 1;
            next_send = probe_for / 2;
        }
        if let Some(f) = heard_foreign(wire, own, &mut buf) {
            refuse(shared, f, log, "before this box served");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    // Whatever queued on the server port meanwhile (our own probe among
    // it) was sent before anyone could have heard an offer from us.
    let _ = wire.server.set_nonblocking(true);
    while wire.server.recv_from(&mut buf).is_ok() {}
    let _ = wire.server.set_nonblocking(false);
    let _ = wire.server.set_read_timeout(Some(TICK));
    set_status(shared, Status::Serving);
    log(&format!(
        "dhcp: serving {}..{} on {} as {own}",
        settings.config.pool.0, settings.config.pool.1, shared.interface
    ));

    // 2. Serve, watching for another server all along.
    while !stop.load(Ordering::Acquire) {
        if let Some(f) = heard_foreign(wire, own, &mut buf) {
            refuse(shared, f, log, "while this box served");
            return;
        }
        let Ok((n, _from)) = wire.server.recv_from(&mut buf) else {
            continue;
        };
        if n > MAX_DATAGRAM {
            continue;
        }
        let Ok(req) = Message::parse(&buf[..n]) else {
            continue;
        };
        if req.mac() == Some(PROBE_MAC) {
            continue;
        }
        let reply = server.handle(now_s(), &req);
        let events = server.take_events();
        let changed = events
            .iter()
            .any(|e| matches!(e, Event::Leased(..) | Event::Released(..) | Event::Declined(..)));
        for e in &events {
            match e {
                Event::Leased(mac, ip) => log(&format!("dhcp: {mac} has {ip}")),
                Event::Declined(mac, ip) => {
                    log(&format!(
                        "dhcp: {mac} declined {ip} (something else answers for it); held out of the pool"
                    ));
                }
                Event::PoolFull(mac) => log(&format!("dhcp: no address left for {mac}")),
                Event::Offered(..) | Event::Refused(..) | Event::Released(..) => {}
            }
        }
        *shared.bindings.lock().unwrap_or_else(PoisonError::into_inner) =
            server.bindings().iter().map(|(m, b)| (*m, *b)).collect();
        if changed && let Err(e) = save(&settings.dir, &server) {
            log(&format!(
                "dhcp: cannot write {}: {e}",
                settings.dir.join(FILE).display()
            ));
        }
        if let Some(r) = reply {
            let to = match r.dest {
                // Sending to an address with no ARP entry needs a raw
                // frame; a broadcast reaches the client just as well.
                Dest::Broadcast | Dest::Unicast { mac: Some(_), .. } => wire.broadcast_to,
                Dest::Unicast { ip, mac: None } => SocketAddr::from((ip, wire.client_port)),
            };
            if let Err(e) = wire.server.send_to(&r.message.encode(), to) {
                log(&format!("dhcp: cannot answer on {}: {e}", shared.interface));
            }
        }
    }
}

/// Write the lease file atomically.
fn save(dir: &Path, server: &Server) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let text = leases::write(server.bindings());
    let tmp = dir.join(format!(".{FILE}.tmp"));
    let written = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, dir.join(FILE))) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// The pool `--dhcp` uses when `--dhcp-pool` is not given: .100 to .199
/// of the interface's /24. Any other mask needs the pool named.
#[must_use]
pub fn default_pool(server: Ipv4Addr, netmask: Ipv4Addr) -> Option<(Ipv4Addr, Ipv4Addr)> {
    if netmask != Ipv4Addr::new(255, 255, 255, 0) {
        return None;
    }
    let [a, b, c, _] = server.octets();
    Some((Ipv4Addr::new(a, b, c, 100), Ipv4Addr::new(a, b, c, 199)))
}

/// `--dhcp-pool FIRST-LAST`.
///
/// # Errors
/// Not two IPv4 addresses joined by `-`.
pub fn parse_pool(s: &str) -> Result<(Ipv4Addr, Ipv4Addr), String> {
    let (a, b) = s
        .split_once('-')
        .ok_or_else(|| format!("--dhcp-pool {s}: FIRST-LAST"))?;
    let ip = |t: &str| {
        t.trim()
            .parse::<Ipv4Addr>()
            .map_err(|_| format!("--dhcp-pool {s}: {t} is not an IPv4 address"))
    };
    Ok((ip(a)?, ip(b)?))
}
