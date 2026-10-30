//! Puts a [`Device`] on the network with std threads:
//!
//! - TCP listener on the encapsulation port: one thread per client,
//!   `RegisterSession` / `UnregisterSession` / `SendRRData` / `ListIdentity`
//!   / `NOP`. A client idle for `session_idle` is dropped. Commands this
//!   adapter does not implement get encapsulation status 0x0001; a
//!   `SendRRData` on an unregistered handle gets 0x0064 with no payload,
//!   as in `OpENer`'s `encap.c`.
//! - UDP listener on the encapsulation port for `ListIdentity` discovery.
//! - UDP I/O socket: consumes O->T, produces T->O to `originator:peer_io_port`
//!   (2222 by default, where `OpENer` sends it), and expires connections.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use helmsman_proto::codec::Reader;
use helmsman_proto::encap::{self, Command, Frame, HEADER_LEN, Header, Status};
use helmsman_proto::explicit;
use helmsman_proto::identity::{IdentityItem, SocketAddress, encode_list_identity};

use crate::device::{self, Device};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: Ipv4Addr,
    /// TCP and UDP port for encapsulation (44818). 0 picks a free port.
    pub encap_port: u16,
    /// Local UDP port for I/O (2222). 0 picks a free port.
    pub io_port: u16,
    /// Port T->O data is sent to on the originator (2222).
    pub peer_io_port: u16,
    pub max_sessions: usize,
    pub session_idle: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: Ipv4Addr::UNSPECIFIED,
            encap_port: encap::PORT,
            io_port: encap::IO_PORT,
            peer_io_port: encap::IO_PORT,
            max_sessions: 32,
            session_idle: Duration::from_secs(120),
        }
    }
}

/// A running adapter. Dropping it stops every thread.
pub struct Adapter {
    device: Arc<Mutex<Device>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    encap_addr: SocketAddrV4,
    io_addr: SocketAddrV4,
    sessions: Arc<AtomicUsize>,
}

impl std::fmt::Debug for Adapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adapter")
            .field("encap_addr", &self.encap_addr)
            .field("io_addr", &self.io_addr)
            .finish_non_exhaustive()
    }
}

fn v4(a: SocketAddr) -> io::Result<SocketAddrV4> {
    match a {
        SocketAddr::V4(a) => Ok(a),
        SocketAddr::V6(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, "IPv6 not supported")),
    }
}

impl Adapter {
    /// Bind every socket, then start serving.
    ///
    /// # Errors
    /// Any bind failure (port in use, permission).
    pub fn start(device: Device, cfg: &ServerConfig) -> io::Result<Self> {
        let tcp = TcpListener::bind((cfg.bind, cfg.encap_port))?;
        let encap_addr = v4(tcp.local_addr()?)?;
        let udp_encap = UdpSocket::bind((cfg.bind, encap_addr.port()))?;
        let io_sock = UdpSocket::bind((cfg.bind, cfg.io_port))?;
        let io_addr = v4(io_sock.local_addr()?)?;

        let device = Arc::new(Mutex::new(device));
        let stop = Arc::new(AtomicBool::new(false));
        let sessions = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();

        tcp.set_nonblocking(true)?;
        {
            let (device, stop, sessions, cfg) = (
                Arc::clone(&device),
                Arc::clone(&stop),
                Arc::clone(&sessions),
                cfg.clone(),
            );
            let next_handle = Arc::new(AtomicU32::new(1));
            threads.push(thread::Builder::new().name("eip-tcp".into()).spawn(move || {
                accept_loop(&tcp, &device, &stop, &sessions, &next_handle, &cfg);
            })?);
        }
        {
            let (device, stop) = (Arc::clone(&device), Arc::clone(&stop));
            let port = encap_addr.port();
            udp_encap.set_read_timeout(Some(Duration::from_millis(50)))?;
            threads.push(thread::Builder::new().name("eip-udp".into()).spawn(move || {
                udp_encap_loop(&udp_encap, &device, &stop, port);
            })?);
        }
        {
            let (device, stop) = (Arc::clone(&device), Arc::clone(&stop));
            let peer_port = cfg.peer_io_port;
            io_sock.set_read_timeout(Some(Duration::from_millis(1)))?;
            threads.push(thread::Builder::new().name("eip-io".into()).spawn(move || {
                io_loop(&io_sock, &device, &stop, peer_port);
            })?);
        }
        Ok(Self {
            device,
            stop,
            threads,
            encap_addr,
            io_addr,
            sessions,
        })
    }

    #[must_use]
    pub fn device(&self) -> Arc<Mutex<Device>> {
        Arc::clone(&self.device)
    }

    #[must_use]
    pub const fn encap_addr(&self) -> SocketAddrV4 {
        self.encap_addr
    }

    #[must_use]
    pub const fn io_addr(&self) -> SocketAddrV4 {
        self.io_addr
    }

    #[must_use]
    pub fn sessions(&self) -> usize {
        self.sessions.load(Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(
    tcp: &TcpListener,
    device: &Arc<Mutex<Device>>,
    stop: &Arc<AtomicBool>,
    sessions: &Arc<AtomicUsize>,
    next_handle: &Arc<AtomicU32>,
    cfg: &ServerConfig,
) {
    let mut clients: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match tcp.accept() {
            Ok((stream, peer)) => {
                let IpAddr::V4(peer_ip) = peer.ip() else { continue };
                if sessions.load(Ordering::Relaxed) >= cfg.max_sessions {
                    drop(stream);
                    continue;
                }
                let (device, stop, sessions, next_handle) = (
                    Arc::clone(device),
                    Arc::clone(stop),
                    Arc::clone(sessions),
                    Arc::clone(next_handle),
                );
                let idle = cfg.session_idle;
                sessions.fetch_add(1, Ordering::Relaxed);
                let spawned = thread::Builder::new().name("eip-client".into()).spawn(move || {
                    let _ = serve_client(stream, peer_ip, &device, &stop, &next_handle, idle);
                    sessions.fetch_sub(1, Ordering::Relaxed);
                });
                if let Ok(h) = spawned {
                    clients.push(h);
                }
                clients.retain(|h| !h.is_finished());
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
    for c in clients {
        let _ = c.join();
    }
}

/// Read one frame, polling `stop` between short read timeouts. `Ok(None)`
/// on orderly close or idle timeout.
fn read_frame(stream: &mut TcpStream, stop: &AtomicBool, idle: Duration) -> io::Result<Option<Vec<u8>>> {
    let mut buf = vec![0u8; HEADER_LEN];
    let mut got = 0;
    let mut need = HEADER_LEN;
    let started = Instant::now();
    while got < need {
        if stop.load(Ordering::Relaxed) || started.elapsed() > idle {
            return Ok(None);
        }
        match stream.read(&mut buf[got..need]) {
            Ok(0) => return Ok(None),
            Ok(n) => {
                got += n;
                if got == HEADER_LEN && need == HEADER_LEN {
                    let h = Header::decode(&mut Reader::new(&buf))
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                    need = HEADER_LEN + usize::from(h.length);
                    buf.resize(need, 0);
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some(buf))
}

fn local_v4(stream: &TcpStream) -> Ipv4Addr {
    match stream.local_addr().map(|a| a.ip()) {
        Ok(IpAddr::V4(ip)) => ip,
        _ => Ipv4Addr::UNSPECIFIED,
    }
}

fn identity_reply(dev: &Device, ip: Ipv4Addr, port: u16, ctx: [u8; 8]) -> Vec<u8> {
    let id = dev.identity();
    let item = IdentityItem {
        protocol_version: encap::PROTOCOL_VERSION,
        socket_address: SocketAddress {
            family: SocketAddress::AF_INET,
            port,
            addr: ip,
        },
        vendor_id: id.vendor_id,
        device_type: id.device_type,
        product_code: id.product_code,
        revision_major: id.revision.0,
        revision_minor: id.revision.1,
        status: 0,
        serial_number: id.serial_number,
        product_name: id.product_name.as_bytes().iter().copied().take(32).collect(),
        state: 3,
    };
    Frame::encode(Command::ListIdentity, 0, ctx, &encode_list_identity(&[item]))
}

fn status_only(h: &Header, status: Status, session: u32) -> Vec<u8> {
    let mut out = *h;
    out.length = 0;
    out.status = status as u32;
    out.session_handle = session;
    out.encode().to_vec()
}

fn serve_client(
    mut stream: TcpStream,
    peer: Ipv4Addr,
    device: &Mutex<Device>,
    stop: &AtomicBool,
    next_handle: &AtomicU32,
    idle: Duration,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_nodelay(true)?;
    let local_ip = local_v4(&stream);
    let local_port = stream.local_addr()?.port();
    let mut handle: Option<u32> = None;
    while let Some(bytes) = read_frame(&mut stream, stop, idle)? {
        let Ok((frame, _)) = Frame::decode(&bytes) else {
            return Ok(());
        };
        let h = frame.header;
        let reply = match Command::from_u16(h.command) {
            Some(Command::Nop) => None,
            Some(Command::ListIdentity) => Some(identity_reply(
                &device::lock_ref(device),
                local_ip,
                local_port,
                h.sender_context,
            )),
            Some(Command::RegisterSession) => {
                let mut r = Reader::new(frame.payload);
                let version = r.u16_le().unwrap_or(0);
                let options = r.u16_le().unwrap_or(1);
                if version != encap::PROTOCOL_VERSION || options != 0 {
                    Some(status_only(&h, Status::UnsupportedProtocol, 0))
                } else if handle.is_some() {
                    Some(status_only(&h, Status::InvalidCommand, handle.unwrap_or(0)))
                } else {
                    let new = next_handle.fetch_add(1, Ordering::Relaxed).max(1);
                    handle = Some(new);
                    let mut out = h;
                    out.session_handle = new;
                    out.status = 0;
                    out.length = 4;
                    let mut bytes = out.encode().to_vec();
                    bytes.extend_from_slice(&[1, 0, 0, 0]);
                    Some(bytes)
                }
            }
            Some(Command::UnregisterSession) => return Ok(()),
            Some(Command::SendRrData) => {
                if handle == Some(h.session_handle) {
                    let mr = {
                        let mut r = Reader::new(frame.payload);
                        let _iface = r.u32_le();
                        let _timeout = r.u16_le();
                        helmsman_proto::cpf::decode(r.rest())
                            .ok()
                            .and_then(|items| match items.as_slice() {
                                [a, d, ..]
                                    if a.type_id == helmsman_proto::cpf::item::NULL_ADDRESS
                                        && d.type_id == helmsman_proto::cpf::item::UNCONNECTED_DATA =>
                                {
                                    Some(d.data.to_vec())
                                }
                                _ => None,
                            })
                    };
                    match mr {
                        Some(mr) => {
                            let resp = device::handle_bytes(
                                &mut device::lock_ref(device),
                                &mr,
                                peer,
                                Instant::now(),
                            );
                            Some(explicit::encode_send_rr_data_reply(
                                h.session_handle,
                                h.sender_context,
                                &resp,
                            ))
                        }
                        None => Some(status_only(&h, Status::IncorrectData, h.session_handle)),
                    }
                } else {
                    Some(status_only(&h, Status::InvalidSessionHandle, h.session_handle))
                }
            }
            _ => Some(status_only(&h, Status::InvalidCommand, h.session_handle)),
        };
        if let Some(r) = reply {
            stream.write_all(&r)?;
        }
    }
    Ok(())
}

/// The local IPv4 a reply to `to` would leave from (connected-UDP trick).
fn route_source(to: Ipv4Addr) -> Ipv4Addr {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| {
            s.connect((to, 9))?;
            s.local_addr()
        })
        .ok()
        .and_then(|a| match a.ip() {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        })
        .unwrap_or(Ipv4Addr::UNSPECIFIED)
}

fn udp_encap_loop(sock: &UdpSocket, device: &Mutex<Device>, stop: &AtomicBool, port: u16) {
    let mut buf = [0u8; 1500];
    while !stop.load(Ordering::Relaxed) {
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let Ok((frame, _)) = Frame::decode(&buf[..n]) else {
            continue;
        };
        if frame.header.command != Command::ListIdentity as u16 {
            continue;
        }
        let IpAddr::V4(from_ip) = from.ip() else { continue };
        let ip = route_source(from_ip);
        let reply = identity_reply(&device::lock_ref(device), ip, port, frame.header.sender_context);
        let _ = sock.send_to(&reply, from);
    }
}

fn io_loop(sock: &UdpSocket, device: &Mutex<Device>, stop: &AtomicBool, peer_port: u16) {
    let mut buf = [0u8; 1500];
    while !stop.load(Ordering::Relaxed) {
        if let Ok((n, SocketAddr::V4(from))) = sock.recv_from(&mut buf) {
            let _ = device::lock_ref(device).on_io_packet(&buf[..n], *from.ip(), Instant::now());
        }
        let now = Instant::now();
        let out = {
            let mut d = device::lock_ref(device);
            d.expire(now);
            d.due_productions(now)
        };
        for (ip, pkt) in out {
            let _ = sock.send_to(&pkt, SocketAddrV4::new(ip, peer_port));
        }
    }
}
