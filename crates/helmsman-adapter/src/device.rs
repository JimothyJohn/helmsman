//! Object model and protocol state of one adapter.
//!
//! Objects: Identity (0x01), Message Router (0x02), Assembly (0x04),
//! Connection Manager (0x06). Anything else answers "path destination
//! unknown" (0x05), unknown services 0x08, unknown attributes 0x14, as
//! `OpENer`'s `cipmessagerouter.c` / `cipcommon.c` do.
//!
//! `Forward_Open` checks run in this order, each with `OpENer`'s extended
//! status (`cipconnectionmanager.h`): duplicate (0x0100), transport
//! class/trigger (0x0103), connection types (0x0123/0x0124), electronic
//! key (0x0114/0x0115/0x0116), path class and configuration instance
//! (0x0315), connection point (0x012F), RPI (0x0111), O->T size (0x0127 +
//! wanted size), T->O size (0x0128 + wanted size), ownership (0x0106),
//! listen-only without an owner (0x0119), capacity (0x0113), config data
//! size (0x0126). Sizes follow `cipioconnection.c`: data + 2-byte sequence
//! count + 4-byte run/idle header when that direction carries one; a
//! heartbeat is 2.
//!
//! Inactivity timeout = O->T RPI × 2^(multiplier + 2), with an initial
//! grace (`OpENer` uses 10 s) until the first O->T packet arrives
//! (`cipconnectionobject.c`).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use helmsman_proto::cip::{self, GeneralStatus, Request, Response, class, service};
use helmsman_proto::codec::{DecodeError, Reader, Writer};
use helmsman_proto::endpoint::IdAllocator;
use helmsman_proto::io::{
    self, Class1Body, ConnectionType, ElectronicKey, ForwardClose, ForwardOpen, ForwardOpenReply, IoPacket,
    ProductionTrigger, class1_size, extended,
};

/// Identity object attributes 1–7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// ODVA-assigned for real products. 0xFFFF here means "unassigned".
    pub vendor_id: u16,
    pub device_type: u16,
    pub product_code: u16,
    pub revision: (u8, u8),
    pub status: u16,
    pub serial_number: u32,
    pub product_name: String,
}

/// What kind of I/O connection a connection point accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    /// Bidirectional; one owner of the consumed (output) assembly at a time.
    ExclusiveOwner,
    /// Heartbeat O->T, data T->O. Any number may share the produced assembly.
    InputOnly,
    /// Like input-only, but only while some non-listen connection produces it.
    ListenOnly,
}

/// One `(configuration, consumed, produced)` triple a scanner may open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionPoint {
    pub kind: PointKind,
    pub configuration: u16,
    /// O->T: the assembly this device consumes (a zero-length assembly makes it a heartbeat).
    pub consumed: u16,
    /// T->O: the assembly this device produces.
    pub produced: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceConfig {
    pub identity: Identity,
    /// Assembly instance → size in bytes.
    pub assemblies: Vec<(u16, usize)>,
    pub points: Vec<ConnectionPoint>,
    /// O->T data carries a 32-bit run/idle header (`OpENer` default: yes).
    pub consumed_run_idle: bool,
    /// T->O data carries one (`OpENer` default: no).
    pub produced_run_idle: bool,
    pub max_connections: usize,
    pub min_rpi: Duration,
    /// Slowest RPI accepted (an EDS's RPI param max; `Duration::MAX` for none).
    pub max_rpi: Duration,
    /// Grace period before the first O->T packet must arrive.
    pub initial_watchdog: Duration,
    /// First O->T connection ID handed out; later ones count up.
    pub connection_id_base: u32,
}

impl DeviceConfig {
    /// The layout of `OpENer`'s POSIX sample application
    /// (`sampleapplication.c`): input 100 (32 B), output 150 (32 B), config
    /// 151 (10 B), heartbeats 152 (input-only) and 153 (listen-only),
    /// explicit 154 (32 B). Identity per `OpENer`'s `CMake` defaults.
    #[must_use]
    pub fn opener_sample() -> Self {
        Self {
            identity: Identity {
                vendor_id: 1,
                device_type: 12,
                product_code: 65001,
                revision: (2, 3),
                status: 0,
                serial_number: 123_456_789,
                product_name: "OpENer PC".into(),
            },
            assemblies: vec![(100, 32), (150, 32), (151, 10), (152, 0), (153, 0), (154, 32)],
            points: vec![
                ConnectionPoint {
                    kind: PointKind::ExclusiveOwner,
                    configuration: 151,
                    consumed: 150,
                    produced: 100,
                },
                ConnectionPoint {
                    kind: PointKind::InputOnly,
                    configuration: 151,
                    consumed: 152,
                    produced: 100,
                },
                ConnectionPoint {
                    kind: PointKind::ListenOnly,
                    configuration: 151,
                    consumed: 153,
                    produced: 100,
                },
            ],
            consumed_run_idle: true,
            produced_run_idle: false,
            max_connections: 16,
            min_rpi: Duration::from_millis(1),
            max_rpi: Duration::MAX,
            initial_watchdog: Duration::from_secs(10),
            connection_id_base: 0x0100_0000,
        }
    }

    /// A simulated Moxa ioLogik E1213, from its EDS (v2.0, SHA-256
    /// `3895fbca…d470d`; `docs/devices/moxa-iologik-e1213.md` cites each
    /// line): identity 991 / 12 / 515 rev 4.1 "E1213" (l.14–21); input
    /// assembly 101, 51 bytes, and output assembly 102, 1 byte (l.460–473);
    /// configuration instance 100 and no configuration data; an exclusive
    /// owner 100/102/101 with a run/idle header O->T and none T->O, and an
    /// input-only connection with heartbeat point 111 (l.484–573); RPI
    /// 100 ms ..= 3 s (Param3, l.433–445). No listen-only connection: the
    /// EDS declares none. At most 10 connections, "9 for read-only, and 1
    /// for read/write" (ioLogik E1200 User's Manual v15.10, p.3-27).
    ///
    /// The data's meaning (DI bits, counters, DO status) is the
    /// application's business; `helmsman-io`'s E1213 simulator fills it.
    #[must_use]
    pub fn moxa_e1213() -> Self {
        Self {
            identity: Identity {
                vendor_id: 991,
                device_type: 12,
                product_code: 515,
                revision: (4, 1),
                status: 0,
                serial_number: 0x00E1_2130,
                product_name: "E1213".into(),
            },
            assemblies: vec![(100, 0), (101, 51), (102, 1), (111, 0)],
            points: vec![
                ConnectionPoint {
                    kind: PointKind::ExclusiveOwner,
                    configuration: 100,
                    consumed: 102,
                    produced: 101,
                },
                ConnectionPoint {
                    kind: PointKind::InputOnly,
                    configuration: 100,
                    consumed: 111,
                    produced: 101,
                },
            ],
            consumed_run_idle: true,
            produced_run_idle: false,
            max_connections: 10,
            min_rpi: Duration::from_millis(100),
            max_rpi: Duration::from_secs(3),
            initial_watchdog: Duration::from_secs(10),
            connection_id_base: 0x0E12_0000,
        }
    }

    /// A target laid out as an EDS describes it: its identity, its
    /// assemblies (plus zero-length ones for configuration and heartbeat
    /// instances the EDS names only in a path), a connection point per
    /// `ConnectionN`, run/idle headers per the exclusive owner's real-time
    /// formats, and the RPI range all connections share.
    ///
    /// Up to 16 connections and serial number 0: the EDS says neither.
    ///
    /// # Errors
    /// The EDS doesn't resolve, has no Class 1 connection, declares a
    /// connection only as redundant owner (not simulated), needs
    /// configuration data, or uses a real-time format other than
    /// modeless, heartbeat and run/idle.
    pub fn from_eds(eds: &helmsman_proto::eds::Eds) -> Result<Self, String> {
        use helmsman_proto::eds::{AppType, RealTimeFormat};
        let e = |x: helmsman_proto::eds::Error| x.to_string();
        let id = eds.identity().map_err(e)?;
        let mut assemblies: Vec<(u16, usize)> = eds
            .assemblies()
            .map_err(e)?
            .iter()
            .map(|a| (a.instance, usize::from(a.size.unwrap_or(0))))
            .collect();
        let connections = eds.connections().map_err(e)?;
        let mut points = Vec::new();
        let (mut min_rpi, mut max_rpi) = (Duration::ZERO, Duration::MAX);
        let (mut consumed_run_idle, mut produced_run_idle) = (true, false);
        for c in connections.iter().filter(|c| c.class1) {
            let name = format!("Connection{} ({:?})", c.number, c.name);
            if c.config_size > 0 {
                return Err(format!("{name}: configuration data is not simulated"));
            }
            for f in [c.o_to_t.format, c.t_to_o.format] {
                if let RealTimeFormat::Other(v) = f {
                    return Err(format!("{name}: real-time format {v} is not simulated"));
                }
            }
            let kind = if c.is(AppType::ExclusiveOwner) {
                consumed_run_idle = c.o_to_t.format == RealTimeFormat::RunIdle;
                produced_run_idle = c.t_to_o.format == RealTimeFormat::RunIdle;
                PointKind::ExclusiveOwner
            } else if c.is(AppType::InputOnly) {
                PointKind::InputOnly
            } else if c.is(AppType::ListenOnly) {
                PointKind::ListenOnly
            } else {
                return Err(format!(
                    "{name}: only exclusive owner, input only and listen only are simulated"
                ));
            };
            for r in [c.o_to_t.rpi, c.t_to_o.rpi] {
                min_rpi = min_rpi.max(r.min.unwrap_or(Duration::ZERO));
                max_rpi = max_rpi.min(r.max.unwrap_or(Duration::MAX));
            }
            for inst in [c.configuration, c.o_to_t_point, c.t_to_o_point] {
                if !assemblies.iter().any(|(i, _)| *i == inst) {
                    assemblies.push((inst, 0));
                }
            }
            points.push(ConnectionPoint {
                kind,
                configuration: c.configuration,
                consumed: c.o_to_t_point,
                produced: c.t_to_o_point,
            });
        }
        if points.is_empty() {
            return Err("the EDS declares no Class 1 connection".into());
        }
        if min_rpi > max_rpi {
            return Err(format!(
                "no RPI fits every connection ({min_rpi:?} > {max_rpi:?})"
            ));
        }
        assemblies.sort_unstable();
        Ok(Self {
            identity: Identity {
                vendor_id: id.vendor_id,
                device_type: id.device_type,
                product_code: id.product_code,
                revision: (id.major_revision, id.minor_revision),
                status: 0,
                serial_number: 0,
                product_name: id.product_name.unwrap_or_default().chars().take(32).collect(),
            },
            assemblies,
            points,
            consumed_run_idle,
            produced_run_idle,
            max_connections: 16,
            min_rpi,
            max_rpi,
            initial_watchdog: Duration::from_secs(10),
            connection_id_base: 0x0100_0000,
        })
    }

    fn assembly_len(&self, instance: u16) -> Option<usize> {
        self.assemblies
            .iter()
            .find(|(i, _)| *i == instance)
            .map(|(_, l)| *l)
    }
}

/// One established I/O connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub point: ConnectionPoint,
    pub originator: Ipv4Addr,
    pub connection_serial: u16,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    pub o_to_t_id: u32,
    pub t_to_o_id: u32,
    pub o_to_t_rpi: Duration,
    pub t_to_o_rpi: Duration,
    pub timeout: Duration,
    pub opened: Instant,
    /// Deadline for the next O->T packet.
    deadline: Instant,
    next_tx: Instant,
    produce_seq: u16,
    encap_seq: u32,
    last_rx_encap_seq: Option<u32>,
    pub packets_in: u64,
    pub packets_out: u64,
}

impl Connection {
    fn key(&self) -> (u16, u16, u32) {
        (
            self.connection_serial,
            self.originator_vendor_id,
            self.originator_serial,
        )
    }
}

/// Why an inbound I/O datagram was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    Malformed(DecodeError),
    UnknownConnection(u32),
    WrongSender,
    Stale,
    WrongSize { got: usize, want: usize },
}

/// Counters for a status page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub requests: u64,
    pub forward_opens: u64,
    pub refusals: u64,
    pub timeouts: u64,
    pub io_dropped: u64,
}

#[derive(Debug)]
pub struct Device {
    cfg: DeviceConfig,
    assemblies: BTreeMap<u16, Vec<u8>>,
    connections: Vec<Connection>,
    next_id: u32,
    /// Last run/idle header from an exclusive owner; `None` until one arrives.
    run_idle: Option<u32>,
    /// Logix tag services (test fixture), answered before the objects above.
    logix: Option<crate::logix::Controller>,
    pub stats: Stats,
    /// Simulator knob: misbehave on purpose (see [`Misbehave`]).
    misbehave: Option<Misbehave>,
    /// Where O->T connection IDs come from when the I/O socket is shared
    /// (`helmsman_proto::endpoint`); `None`: `connection_id_base` upward.
    ids: Option<IdAllocator>,
}

/// One T->O datagram due, from [`Device::due`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Production {
    /// Where it goes: the IP the connection was opened from.
    pub originator: Ipv4Addr,
    /// The connection's O->T ID (unique on this device; the T->O ID in the
    /// datagram is the originator's and two originators may pick the same).
    pub o_to_t_id: u32,
    pub datagram: Vec<u8>,
}

/// Ways a simulated target misbehaves on purpose, for testing scanners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Misbehave {
    /// Produce nothing (a pulled cable, a hung device). Connections stay
    /// open until their O->T traffic stops.
    Silent,
    /// Produce T->O data one byte short of the assembly.
    ShortInput,
    /// Produce T->O datagrams whose body is junk of the wrong length.
    Garbage,
}

/// `SEQ_GT32` from `OpENer`: `a` is newer than `b` with wraparound.
const fn seq_newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000_0000
}

fn refuse(fo_key: (u16, u16, u32), service: u8, status: GeneralStatus, ext: &[u16]) -> Response {
    let mut r = Response::error(service, status);
    r.additional_status = ext.to_vec();
    let mut w = Writer::with_capacity(10);
    w.u16_le(fo_key.0).u16_le(fo_key.1).u32_le(fo_key.2).u8(0).u8(0);
    r.data = w.into_inner();
    r
}

fn cm_refuse(fo_key: (u16, u16, u32), service: u8, ext: &[u16]) -> Response {
    refuse(fo_key, service, GeneralStatus::CONNECTION_FAILURE, ext)
}

impl Device {
    #[must_use]
    pub fn new(cfg: DeviceConfig) -> Self {
        let assemblies = cfg.assemblies.iter().map(|(i, l)| (*i, vec![0u8; *l])).collect();
        let next_id = cfg.connection_id_base;
        Self {
            cfg,
            assemblies,
            connections: Vec::new(),
            next_id,
            run_idle: None,
            logix: None,
            misbehave: None,
            stats: Stats::default(),
            ids: None,
        }
    }

    /// Also answer Logix tag services from `controller` (a test fixture:
    /// Read Tag, Multiple Service Packet, Read Tag Fragmented, Symbol
    /// object browse, routed or not). Everything else is answered as before.
    #[must_use]
    pub fn with_logix(mut self, controller: crate::logix::Controller) -> Self {
        self.logix = Some(controller);
        self
    }

    /// Take O->T connection IDs from a shared I/O endpoint, so they never
    /// collide with the IDs the box chose as an originator.
    pub fn set_id_allocator(&mut self, ids: IdAllocator) {
        self.ids = Some(ids);
    }

    #[must_use]
    pub const fn logix(&self) -> Option<&crate::logix::Controller> {
        self.logix.as_ref()
    }

    pub const fn logix_mut(&mut self) -> Option<&mut crate::logix::Controller> {
        self.logix.as_mut()
    }

    #[must_use]
    pub const fn config(&self) -> &DeviceConfig {
        &self.cfg
    }

    #[must_use]
    pub fn identity(&self) -> &Identity {
        &self.cfg.identity
    }

    #[must_use]
    pub fn connections(&self) -> &[Connection] {
        &self.connections
    }

    /// Run bit of the most recent exclusive-owner O->T packet. False while
    /// no owner is connected, so a program can tell "scanner in run".
    #[must_use]
    pub fn scanner_running(&self) -> bool {
        self.run_idle.is_some_and(|h| h & 1 == 1)
            && self
                .connections
                .iter()
                .any(|c| c.point.kind == PointKind::ExclusiveOwner)
    }

    #[must_use]
    pub fn assembly(&self, instance: u16) -> Option<&[u8]> {
        self.assemblies.get(&instance).map(Vec::as_slice)
    }

    /// Overwrite an assembly from the application side (e.g. the PLC writing
    /// its produced data). Length must match.
    ///
    /// # Errors
    /// The unknown instance or the length it wants.
    pub fn write_assembly(&mut self, instance: u16, data: &[u8]) -> Result<(), String> {
        let a = self
            .assemblies
            .get_mut(&instance)
            .ok_or_else(|| format!("no assembly {instance}"))?;
        if a.len() != data.len() {
            return Err(format!(
                "assembly {instance} is {} bytes, got {}",
                a.len(),
                data.len()
            ));
        }
        a.copy_from_slice(data);
        Ok(())
    }

    /// Answer one explicit request. `peer` is the originator's IP (TCP peer).
    pub fn handle(&mut self, req: &Request, peer: Ipv4Addr, now: Instant) -> Response {
        self.stats.requests += 1;
        match req.path.class {
            class::IDENTITY => self.identity_service(req),
            class::MESSAGE_ROUTER => Self::message_router_service(req),
            class::ASSEMBLY => self.assembly_service(req),
            class::CONNECTION_MANAGER => self.connection_manager_service(req, peer, now),
            _ => Response::error(req.service, GeneralStatus::PATH_DESTINATION_UNKNOWN),
        }
    }

    fn identity_attr(&self, attr: u16) -> Option<Vec<u8>> {
        let id = &self.cfg.identity;
        let mut w = Writer::with_capacity(36);
        match attr {
            1 => w.u16_le(id.vendor_id),
            2 => w.u16_le(id.device_type),
            3 => w.u16_le(id.product_code),
            4 => w.u8(id.revision.0).u8(id.revision.1),
            5 => w.u16_le(self.identity_status()),
            6 => w.u32_le(id.serial_number),
            7 => {
                let name = id.product_name.as_bytes();
                let n = name.len().min(32);
                w.u8(u8::try_from(n).unwrap_or(32)).bytes(&name[..n])
            }
            _ => return None,
        };
        Some(w.into_inner())
    }

    /// Identity status word: bit 0 "owned" when an exclusive owner exists.
    fn identity_status(&self) -> u16 {
        let owned = self
            .connections
            .iter()
            .any(|c| c.point.kind == PointKind::ExclusiveOwner);
        self.cfg.identity.status | u16::from(owned)
    }

    fn identity_service(&self, req: &Request) -> Response {
        if req.path.instance == 0 {
            // Class attributes: 1 revision, 2 max instance.
            return match (req.service, req.path.attribute) {
                (service::GET_ATTRIBUTE_SINGLE, Some(1 | 2)) => Response::ok(req.service, vec![1, 0]),
                (service::GET_ATTRIBUTE_SINGLE, _) => {
                    Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED)
                }
                _ => Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED),
            };
        }
        if req.path.instance != 1 {
            return Response::error(req.service, GeneralStatus::PATH_DESTINATION_UNKNOWN);
        }
        match req.service {
            service::GET_ATTRIBUTE_SINGLE => match req.path.attribute.and_then(|a| self.identity_attr(a)) {
                Some(d) => Response::ok(req.service, d),
                None => Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED),
            },
            service::GET_ATTRIBUTE_ALL => {
                let mut d = Vec::with_capacity(48);
                for a in 1..=7 {
                    d.extend(self.identity_attr(a).unwrap_or_default());
                }
                Response::ok(req.service, d)
            }
            _ => Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED),
        }
    }

    fn message_router_service(req: &Request) -> Response {
        match req.service {
            service::GET_ATTRIBUTE_SINGLE => {
                Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED)
            }
            _ => Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED),
        }
    }

    fn is_consumed_by_owner(&self, instance: u16) -> bool {
        self.connections
            .iter()
            .any(|c| c.point.kind == PointKind::ExclusiveOwner && c.point.consumed == instance)
    }

    fn assembly_service(&mut self, req: &Request) -> Response {
        let Some(len) = self.cfg.assembly_len(req.path.instance) else {
            return Response::error(req.service, GeneralStatus::PATH_DESTINATION_UNKNOWN);
        };
        let attr = req.path.attribute;
        match (req.service, attr) {
            (service::GET_ATTRIBUTE_SINGLE, Some(3)) => {
                Response::ok(req.service, self.assemblies[&req.path.instance].clone())
            }
            (service::GET_ATTRIBUTE_SINGLE, Some(4)) => Response::ok(
                req.service,
                u16::try_from(len).unwrap_or(u16::MAX).to_le_bytes().to_vec(),
            ),
            (service::GET_ATTRIBUTE_SINGLE, _) => {
                Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED)
            }
            (service::SET_ATTRIBUTE_SINGLE, Some(3)) => {
                let inst = req.path.instance;
                let writable = self
                    .cfg
                    .points
                    .iter()
                    .any(|p| (p.consumed == inst || p.configuration == inst) && len > 0);
                if !writable {
                    return Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SETTABLE);
                }
                if self.is_consumed_by_owner(inst) {
                    return Response::error(req.service, GeneralStatus::DEVICE_STATE_CONFLICT);
                }
                match req.data.len().cmp(&len) {
                    std::cmp::Ordering::Less => Response::error(req.service, GeneralStatus::NOT_ENOUGH_DATA),
                    std::cmp::Ordering::Greater => Response::error(req.service, GeneralStatus::TOO_MUCH_DATA),
                    std::cmp::Ordering::Equal => {
                        if let Some(a) = self.assemblies.get_mut(&inst) {
                            a.copy_from_slice(&req.data);
                        }
                        Response::ok(req.service, Vec::new())
                    }
                }
            }
            (service::SET_ATTRIBUTE_SINGLE, _) => {
                Response::error(req.service, GeneralStatus::ATTRIBUTE_NOT_SETTABLE)
            }
            _ => Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED),
        }
    }

    fn connection_manager_service(&mut self, req: &Request, peer: Ipv4Addr, now: Instant) -> Response {
        if req.path.instance != 1 {
            return Response::error(req.service, GeneralStatus::PATH_DESTINATION_UNKNOWN);
        }
        match req.service {
            io::service::FORWARD_OPEN => self.forward_open(req, peer, now),
            io::service::FORWARD_CLOSE => self.forward_close(req, peer),
            io::service::UNCONNECTED_SEND => self.unconnected_send(req, peer, now),
            _ => Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED),
        }
    }

    /// `Unconnected_Send` with a route path that ends here (Logix-style
    /// originators wrap every request in one). The embedded request is
    /// processed locally and its reply returned as the reply.
    fn unconnected_send(&mut self, req: &Request, peer: Ipv4Addr, now: Instant) -> Response {
        let parse = || -> Result<Request, DecodeError> {
            let mut r = Reader::new(&req.data);
            let _tick = r.u8()?;
            let _ticks = r.u8()?;
            let n = usize::from(r.u16_le()?);
            let embedded = r.bytes(n)?;
            Request::decode(embedded)
        };
        match parse() {
            Ok(inner)
                if inner.path.class == class::CONNECTION_MANAGER
                    && inner.service == io::service::UNCONNECTED_SEND =>
            {
                Response::error(req.service, GeneralStatus::PATH_SEGMENT_ERROR)
            }
            Ok(inner) => self.handle(&inner, peer, now),
            Err(DecodeError::Truncated { .. }) => {
                Response::error(req.service, GeneralStatus::NOT_ENOUGH_DATA)
            }
            Err(DecodeError::Invalid(_)) => Response::error(req.service, GeneralStatus::PATH_SEGMENT_ERROR),
        }
    }

    fn check_key(&self, k: &ElectronicKey) -> Result<(), u16> {
        let id = &self.cfg.identity;
        if (k.vendor_id != 0 && k.vendor_id != id.vendor_id)
            || (k.product_code != 0 && k.product_code != id.product_code)
        {
            return Err(extended::VENDOR_OR_PRODUCT_CODE_MISMATCH);
        }
        if k.device_type != 0 && k.device_type != id.device_type {
            return Err(extended::DEVICE_TYPE_MISMATCH);
        }
        let (maj, min) = id.revision;
        let ok = if k.compatibility {
            (k.major_revision == 0 || k.major_revision == maj)
                && (k.minor_revision == 0 || k.minor_revision <= min)
        } else {
            (k.major_revision == 0 || k.major_revision == maj)
                && (k.minor_revision == 0 || k.minor_revision == min)
        };
        if ok {
            Ok(())
        } else {
            Err(extended::REVISION_MISMATCH)
        }
    }

    fn expected_size(&self, instance: u16, run_idle: bool) -> Option<u16> {
        let len = u16::try_from(self.cfg.assembly_len(instance)?).ok()?;
        Some(class1_size(len, run_idle))
    }

    #[allow(clippy::too_many_lines)]
    fn forward_open(&mut self, req: &Request, peer: Ipv4Addr, now: Instant) -> Response {
        self.stats.forward_opens += 1;
        let fo = match ForwardOpen::decode(&req.data) {
            Ok(fo) => fo,
            Err(DecodeError::Truncated { .. }) => {
                self.stats.refusals += 1;
                return Response::error(req.service, GeneralStatus::NOT_ENOUGH_DATA);
            }
            Err(DecodeError::Invalid(_)) => {
                self.stats.refusals += 1;
                let mut r = Response::error(req.service, GeneralStatus::CONNECTION_FAILURE);
                r.additional_status = vec![extended::INVALID_SEGMENT_TYPE_IN_PATH];
                return r;
            }
        };
        let key = (
            fo.connection_serial,
            fo.originator_vendor_id,
            fo.originator_serial,
        );
        let svc = req.service;
        let refused = |dev: &mut Self, ext: &[u16]| {
            dev.stats.refusals += 1;
            cm_refuse(key, svc, ext)
        };

        if self.connections.iter().any(|c| c.key() == key) {
            return refused(self, &[extended::CONNECTION_IN_USE]);
        }
        if fo.transport.class != 1 || fo.transport.server || fo.transport.trigger != ProductionTrigger::Cyclic
        {
            return refused(self, &[extended::TRANSPORT_TRIGGER_NOT_SUPPORTED]);
        }
        if fo.o_to_t.kind != ConnectionType::PointToPoint {
            return refused(self, &[extended::INVALID_O_TO_T_TYPE]);
        }
        if fo.t_to_o.kind != ConnectionType::PointToPoint {
            return refused(self, &[extended::INVALID_T_TO_O_TYPE]);
        }
        if let Some(k) = &fo.path.key
            && let Err(code) = self.check_key(k)
        {
            return refused(self, &[code]);
        }
        let cfg_inst = match fo.path.configuration {
            Some(c) if fo.path.class == class::ASSEMBLY && self.cfg.assembly_len(c).is_some() => c,
            _ => return refused(self, &[extended::INVALID_SEGMENT_TYPE_IN_PATH]),
        };
        let (Some(consumed), Some(produced)) = (fo.path.consumed, fo.path.produced) else {
            return refused(self, &[extended::INCONSISTENT_PATH_COMBO]);
        };
        let Some(point) = self
            .cfg
            .points
            .iter()
            .copied()
            .find(|p| p.configuration == cfg_inst && p.consumed == consumed && p.produced == produced)
        else {
            return refused(self, &[extended::INCONSISTENT_PATH_COMBO]);
        };
        let min_us = u32::try_from(self.cfg.min_rpi.as_micros()).unwrap_or(u32::MAX);
        let max_us = u32::try_from(self.cfg.max_rpi.as_micros()).unwrap_or(u32::MAX);
        if fo.o_to_t_rpi_us < min_us
            || fo.t_to_o_rpi_us < min_us
            || fo.o_to_t_rpi_us > max_us
            || fo.t_to_o_rpi_us > max_us
        {
            return refused(self, &[extended::RPI_NOT_SUPPORTED]);
        }
        let want_o = self
            .expected_size(consumed, self.cfg.consumed_run_idle)
            .unwrap_or(2);
        if fo.o_to_t.size != want_o {
            return refused(self, &[extended::INVALID_O_TO_T_SIZE, want_o]);
        }
        let want_t = self
            .expected_size(produced, self.cfg.produced_run_idle)
            .unwrap_or(2);
        if fo.t_to_o.size != want_t {
            return refused(self, &[extended::INVALID_T_TO_O_SIZE, want_t]);
        }
        match point.kind {
            PointKind::ExclusiveOwner if self.is_consumed_by_owner(consumed) => {
                return refused(self, &[extended::OWNERSHIP_CONFLICT]);
            }
            PointKind::ListenOnly
                if !self
                    .connections
                    .iter()
                    .any(|c| c.point.produced == produced && c.point.kind != PointKind::ListenOnly) =>
            {
                return refused(self, &[extended::NON_LISTEN_ONLY_NOT_OPENED]);
            }
            _ => {}
        }
        if self.connections.len() >= self.cfg.max_connections {
            return refused(self, &[extended::NO_MORE_CONNECTIONS]);
        }
        if !fo.path.config_data.is_empty() {
            let cfg_len = self.cfg.assembly_len(cfg_inst).unwrap_or(0);
            let padded = cfg_len.div_ceil(2) * 2;
            if fo.path.config_data.len() != padded {
                let words = u16::try_from(padded / 2).unwrap_or(u16::MAX);
                return refused(self, &[extended::INVALID_CONFIGURATION_SIZE, words]);
            }
            if let Some(a) = self.assemblies.get_mut(&cfg_inst) {
                a.copy_from_slice(&fo.path.config_data[..cfg_len]);
            }
        }

        let o_to_t_id = if let Some(id) = self.ids.as_ref().and_then(IdAllocator::allocate) {
            id
        } else {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            id
        };
        let o_to_t_rpi = Duration::from_micros(u64::from(fo.o_to_t_rpi_us));
        let t_to_o_rpi = Duration::from_micros(u64::from(fo.t_to_o_rpi_us));
        let timeout = o_to_t_rpi * (1u32 << (u32::from(fo.timeout_multiplier.min(7)) + 2));
        self.connections.push(Connection {
            point,
            originator: peer,
            connection_serial: fo.connection_serial,
            originator_vendor_id: fo.originator_vendor_id,
            originator_serial: fo.originator_serial,
            o_to_t_id,
            t_to_o_id: fo.t_to_o_connection_id,
            o_to_t_rpi,
            t_to_o_rpi,
            timeout,
            opened: now,
            deadline: now + timeout.max(self.cfg.initial_watchdog),
            next_tx: now,
            produce_seq: 0,
            encap_seq: 0,
            last_rx_encap_seq: None,
            packets_in: 0,
            packets_out: 0,
        });
        let reply = ForwardOpenReply {
            o_to_t_connection_id: o_to_t_id,
            t_to_o_connection_id: fo.t_to_o_connection_id,
            connection_serial: fo.connection_serial,
            originator_vendor_id: fo.originator_vendor_id,
            originator_serial: fo.originator_serial,
            o_to_t_api_us: fo.o_to_t_rpi_us,
            t_to_o_api_us: fo.t_to_o_rpi_us,
            application_reply: Vec::new(),
        };
        let mut w = Writer::with_capacity(26);
        reply.encode(&mut w);
        Response::ok(req.service, w.into_inner())
    }

    fn forward_close(&mut self, req: &Request, peer: Ipv4Addr) -> Response {
        let Ok(fc) = ForwardClose::decode(&req.data) else {
            return Response::error(req.service, GeneralStatus::NOT_ENOUGH_DATA);
        };
        let key = (
            fc.connection_serial,
            fc.originator_vendor_id,
            fc.originator_serial,
        );
        let Some(pos) = self.connections.iter().position(|c| c.key() == key) else {
            return cm_refuse(key, req.service, &[extended::CONNECTION_NOT_FOUND]);
        };
        if self.connections[pos].originator != peer {
            return refuse(key, req.service, GeneralStatus::PRIVILEGE_VIOLATION, &[]);
        }
        self.connections.remove(pos);
        self.drop_orphan_listeners();
        let mut w = Writer::with_capacity(10);
        io::ForwardCloseReply {
            connection_serial: fc.connection_serial,
            originator_vendor_id: fc.originator_vendor_id,
            originator_serial: fc.originator_serial,
            application_reply: Vec::new(),
        }
        .encode(&mut w);
        Response::ok(req.service, w.into_inner())
    }

    fn drop_orphan_listeners(&mut self) {
        let live: Vec<u16> = self
            .connections
            .iter()
            .filter(|c| c.point.kind != PointKind::ListenOnly)
            .map(|c| c.point.produced)
            .collect();
        self.connections
            .retain(|c| c.point.kind != PointKind::ListenOnly || live.contains(&c.point.produced));
        if !self
            .connections
            .iter()
            .any(|c| c.point.kind == PointKind::ExclusiveOwner)
        {
            self.run_idle = None;
        }
    }

    /// Feed one O->T datagram from `from`.
    ///
    /// # Errors
    /// Why it was dropped; dropping is normal (stale, foreign) and counted.
    pub fn on_io_packet(&mut self, datagram: &[u8], from: Ipv4Addr, now: Instant) -> Result<(), Dropped> {
        let result = self.consume(datagram, from, now);
        if result.is_err() {
            self.stats.io_dropped += 1;
        }
        result
    }

    fn consume(&mut self, datagram: &[u8], from: Ipv4Addr, now: Instant) -> Result<(), Dropped> {
        let pkt = IoPacket::decode(datagram).map_err(Dropped::Malformed)?;
        let idx = self
            .connections
            .iter()
            .position(|c| c.o_to_t_id == pkt.connection_id)
            .ok_or(Dropped::UnknownConnection(pkt.connection_id))?;
        let header = self.cfg.consumed_run_idle;
        let c = &mut self.connections[idx];
        if c.originator != from {
            return Err(Dropped::WrongSender);
        }
        if let (Some(seq), Some(last)) = (pkt.encapsulation_sequence, c.last_rx_encap_seq)
            && !seq_newer(seq, last)
        {
            return Err(Dropped::Stale);
        }
        c.last_rx_encap_seq = pkt.encapsulation_sequence.or(c.last_rx_encap_seq);
        c.deadline = now + c.timeout;
        c.packets_in += 1;
        let point = c.point;
        let body = Class1Body::decode(pkt.body, header).map_err(Dropped::Malformed)?;
        if point.kind != PointKind::ExclusiveOwner {
            return Ok(());
        }
        let want = self.cfg.assembly_len(point.consumed).unwrap_or(0);
        if body.data.len() != want {
            return Err(Dropped::WrongSize {
                got: body.data.len(),
                want,
            });
        }
        if let Some(h) = body.run_idle {
            self.run_idle = Some(h);
        }
        if let Some(a) = self.assemblies.get_mut(&point.consumed) {
            a.copy_from_slice(body.data);
        }
        Ok(())
    }

    /// Misbehave from now on (`None`: behave). A simulator knob.
    pub const fn set_misbehave(&mut self, m: Option<Misbehave>) {
        self.misbehave = m;
    }

    /// Forget every connection at once, as a power cycle does. A simulator
    /// knob: the scanners find out when their T->O traffic stops.
    pub fn drop_connections(&mut self) {
        self.connections.clear();
        self.run_idle = None;
    }

    /// T->O datagrams due at `now`, addressed to each originator's IP.
    pub fn due_productions(&mut self, now: Instant) -> Vec<(Ipv4Addr, Vec<u8>)> {
        self.due(now)
            .into_iter()
            .map(|p| (p.originator, p.datagram))
            .collect()
    }

    /// [`Self::due_productions`] with the connection each one is for.
    pub fn due(&mut self, now: Instant) -> Vec<Production> {
        let mut out = Vec::new();
        let header = self.cfg.produced_run_idle;
        for c in &mut self.connections {
            if now < c.next_tx {
                continue;
            }
            c.next_tx = if now.duration_since(c.next_tx) > c.t_to_o_rpi {
                now + c.t_to_o_rpi
            } else {
                c.next_tx + c.t_to_o_rpi
            };
            c.produce_seq = c.produce_seq.wrapping_add(1);
            c.encap_seq = c.encap_seq.wrapping_add(1);
            let mut data = self
                .assemblies
                .get(&c.point.produced)
                .cloned()
                .unwrap_or_default();
            match self.misbehave {
                Some(Misbehave::Silent) => continue,
                Some(Misbehave::ShortInput) => {
                    data.pop();
                }
                Some(Misbehave::Garbage) => data = vec![0xA5; data.len() * 2 + 3],
                None => {}
            }
            let body = Class1Body {
                sequence_count: c.produce_seq,
                run_idle: (header && !data.is_empty()).then_some(1),
                data: &data,
            }
            .encode();
            let pkt = IoPacket {
                connection_id: c.t_to_o_id,
                encapsulation_sequence: Some(c.encap_seq),
                body: &body,
            }
            .encode();
            c.packets_out += 1;
            out.push(Production {
                originator: c.originator,
                o_to_t_id: c.o_to_t_id,
                datagram: pkt,
            });
        }
        out
    }

    /// Close connections whose O->T traffic stopped. Returns how many closed.
    pub fn expire(&mut self, now: Instant) -> usize {
        let before = self.connections.len();
        self.connections.retain(|c| now < c.deadline);
        let timed_out = before - self.connections.len();
        if timed_out > 0 {
            self.stats.timeouts += timed_out as u64;
            self.drop_orphan_listeners();
        }
        before - self.connections.len()
    }

    /// Earliest instant something needs doing (next production or deadline).
    #[must_use]
    pub fn next_event(&self) -> Option<Instant> {
        self.connections
            .iter()
            .flat_map(|c| [c.next_tx, c.deadline])
            .min()
    }
}

/// Lock a shared device, recovering from a poisoned lock (the state is
/// plain data and each call leaves it consistent).
pub fn lock_ref(m: &std::sync::Mutex<Device>) -> std::sync::MutexGuard<'_, Device> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Map a raw `SendRRData` payload's request to a response, for callers that
/// only have bytes (the server). Malformed requests get a CIP error, never a
/// dropped session.
pub fn handle_bytes(dev: &mut Device, mr: &[u8], peer: Ipv4Addr, now: Instant) -> Response {
    if let Some(resp) = dev.logix.as_mut().and_then(|l| l.handle(mr)) {
        dev.stats.requests += 1;
        return resp;
    }
    match Request::decode(mr) {
        Ok(req) => dev.handle(&req, peer, now),
        Err(e) => {
            let service = mr.first().copied().unwrap_or(0) & !cip::service::REPLY_BIT;
            let status = match e {
                DecodeError::Truncated { .. } => GeneralStatus::NOT_ENOUGH_DATA,
                DecodeError::Invalid(_) => GeneralStatus::PATH_SEGMENT_ERROR,
            };
            Response::error(service, status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helmsman_proto::io::{ConnectionPath, NetworkParams, Priority, Refused, Transport, open_result};

    const PEER: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 9);

    fn dev() -> Device {
        Device::new(DeviceConfig::opener_sample())
    }

    fn open_req(serial: u16, consumed: u16, out_size: u16, in_size: u16) -> Request {
        ForwardOpen {
            priority_time_tick: 0x0A,
            timeout_ticks: 0xF0,
            o_to_t_connection_id: 0,
            t_to_o_connection_id: 0xAB00 + u32::from(serial),
            connection_serial: serial,
            originator_vendor_id: 0xFFFF,
            originator_serial: 7,
            timeout_multiplier: 0,
            o_to_t_rpi_us: 10_000,
            o_to_t: NetworkParams::point_to_point(out_size, Priority::Scheduled),
            t_to_o_rpi_us: 10_000,
            t_to_o: NetworkParams::point_to_point(in_size, Priority::Scheduled),
            transport: Transport::CLASS1_CYCLIC,
            path: ConnectionPath::assembly(151, Some(consumed), Some(100)),
        }
        .to_request()
    }

    fn refused(r: &Response) -> Refused {
        open_result(r).unwrap().unwrap_err()
    }

    #[test]
    fn identity_attributes_and_errors() {
        let mut d = dev();
        let now = Instant::now();
        let r = d.handle(&Request::get_attribute_single(1, 1, 7), PEER, now);
        assert_eq!(r.data, b"\x09OpENer PC");
        let r = d.handle(&Request::get_attribute_single(1, 1, 99), PEER, now);
        assert_eq!(r.status, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED);
        let r = d.handle(&Request::new(0x33, cip::Path::new(1, 1)), PEER, now);
        assert_eq!(r.status, GeneralStatus::SERVICE_NOT_SUPPORTED);
        let r = d.handle(&Request::get_attribute_single(0x7F, 1, 1), PEER, now);
        assert_eq!(r.status, GeneralStatus::PATH_DESTINATION_UNKNOWN);
        let all = d.handle(&Request::get_attributes_all(1, 1), PEER, now);
        assert_eq!(&all.data[..6], &[1, 0, 12, 0, 0xE9, 0xFD]);
    }

    #[test]
    fn open_echo_close() {
        let mut d = dev();
        let t0 = Instant::now();
        let r = d.handle(&open_req(1, 150, 38, 34), PEER, t0);
        let reply = open_result(&r).unwrap().unwrap();
        assert_eq!(reply.o_to_t_connection_id, 0x0100_0000);
        assert_eq!(d.identity_status() & 1, 1);

        // O->T data with run header lands in assembly 150.
        let body = Class1Body {
            sequence_count: 1,
            run_idle: Some(1),
            data: &[7u8; 32],
        }
        .encode();
        let pkt = IoPacket {
            connection_id: reply.o_to_t_connection_id,
            encapsulation_sequence: Some(1),
            body: &body,
        }
        .encode();
        d.on_io_packet(&pkt, PEER, t0).unwrap();
        assert_eq!(d.assembly(150).unwrap(), &[7u8; 32]);
        assert!(d.scanner_running());
        assert_eq!(d.on_io_packet(&pkt, PEER, t0), Err(Dropped::Stale));
        assert_eq!(
            d.on_io_packet(&pkt, Ipv4Addr::new(1, 2, 3, 4), t0),
            Err(Dropped::WrongSender)
        );

        // T->O production of assembly 100, no header, to the originator.
        d.write_assembly(100, &[3u8; 32]).unwrap();
        let out = d.due_productions(t0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, PEER);
        let p = IoPacket::decode(&out[0].1).unwrap();
        assert_eq!(p.connection_id, 0xAB01);
        assert_eq!(Class1Body::decode(p.body, false).unwrap().data, &[3u8; 32]);
        assert!(
            d.due_productions(t0).is_empty(),
            "not due again until the RPI passes"
        );
        assert_eq!(d.due_productions(t0 + Duration::from_millis(10)).len(), 1);

        let close = ForwardClose {
            priority_time_tick: 0x0A,
            timeout_ticks: 0xF0,
            connection_serial: 1,
            originator_vendor_id: 0xFFFF,
            originator_serial: 7,
            path: ConnectionPath::assembly(151, Some(150), Some(100)),
        };
        let r = d.handle(&close.to_request(), Ipv4Addr::new(9, 9, 9, 9), t0);
        assert_eq!(r.status, GeneralStatus::PRIVILEGE_VIOLATION);
        let r = d.handle(&close.to_request(), PEER, t0);
        assert!(r.status.is_success(), "{r:?}");
        assert_eq!(d.connections(), &[]);
        assert!(!d.scanner_running());
        let r = d.handle(&close.to_request(), PEER, t0);
        assert_eq!(refused(&r).extended_code(), Some(extended::CONNECTION_NOT_FOUND));
    }

    #[test]
    fn refusals_match_opener_codes() {
        let mut d = dev();
        let now = Instant::now();
        let r = d.handle(&open_req(1, 150, 34, 34), PEER, now);
        assert_eq!(refused(&r).extended, vec![0x0127, 38]);
        let r = d.handle(&open_req(1, 150, 38, 38), PEER, now);
        assert_eq!(refused(&r).extended, vec![0x0128, 34]);
        let r = d.handle(&open_req(1, 99, 38, 34), PEER, now);
        assert_eq!(
            refused(&r).extended_code(),
            Some(extended::INCONSISTENT_PATH_COMBO)
        );
        let r = d.handle(&open_req(2, 153, 2, 34), PEER, now);
        assert_eq!(
            refused(&r).extended_code(),
            Some(extended::NON_LISTEN_ONLY_NOT_OPENED)
        );

        assert!(d.handle(&open_req(1, 150, 38, 34), PEER, now).status.is_success());
        let r = d.handle(&open_req(1, 150, 38, 34), PEER, now);
        assert_eq!(refused(&r).extended_code(), Some(extended::CONNECTION_IN_USE));
        let r = d.handle(&open_req(2, 150, 38, 34), PEER, now);
        assert_eq!(refused(&r).extended_code(), Some(extended::OWNERSHIP_CONFLICT));
        assert!(d.handle(&open_req(3, 152, 2, 34), PEER, now).status.is_success());
        assert!(d.handle(&open_req(4, 153, 2, 34), PEER, now).status.is_success());
        assert_eq!(d.connections().len(), 3);
    }

    #[test]
    fn electronic_key_rules() {
        let mut d = dev();
        let now = Instant::now();
        let mut req = open_req(1, 150, 38, 34);
        let mut fo = ForwardOpen::decode(&req.data).unwrap();
        for (key, want) in [
            ((2, 12, 65001, 2, 3, false), Some(0x0114)),
            ((1, 13, 65001, 2, 3, false), Some(0x0115)),
            ((1, 12, 65001, 3, 0, true), Some(0x0116)),
            ((1, 12, 65001, 2, 4, true), Some(0x0116)),
            ((1, 12, 65001, 2, 1, true), None),
            ((0, 0, 0, 0, 0, false), None),
        ] {
            fo.connection_serial += 1;
            fo.path.key = Some(ElectronicKey {
                vendor_id: key.0,
                device_type: key.1,
                product_code: key.2,
                major_revision: key.3,
                minor_revision: key.4,
                compatibility: key.5,
            });
            req = fo.to_request();
            let r = d.handle(&req, PEER, now);
            if let Some(code) = want {
                assert_eq!(refused(&r).extended_code(), Some(code), "{key:?}");
            } else {
                assert!(r.status.is_success(), "{key:?} {r:?}");
                d.connections.clear();
            }
        }
    }

    #[test]
    fn timeout_uses_initial_grace_then_rpi_multiple_and_drops_listeners() {
        let mut d = dev();
        let t0 = Instant::now();
        let reply = open_result(&d.handle(&open_req(1, 150, 38, 34), PEER, t0))
            .unwrap()
            .unwrap();
        assert!(d.handle(&open_req(2, 153, 2, 34), PEER, t0).status.is_success());
        assert_eq!(d.expire(t0 + Duration::from_secs(9)), 0, "10 s initial grace");
        let body = Class1Body {
            sequence_count: 1,
            run_idle: Some(1),
            data: &[0u8; 32],
        }
        .encode();
        let pkt = IoPacket {
            connection_id: reply.o_to_t_connection_id,
            encapsulation_sequence: Some(1),
            body: &body,
        }
        .encode();
        let t1 = t0 + Duration::from_secs(9);
        d.on_io_packet(&pkt, PEER, t1).unwrap();
        // timeout = 10 ms × 2^(0+2) = 40 ms after the last packet.
        assert_eq!(d.expire(t1 + Duration::from_millis(39)), 0);
        assert!(d.expire(t1 + Duration::from_millis(41)) >= 1);
        assert_eq!(d.connections(), &[], "listen-only goes with its owner");
        assert_eq!(
            d.stats.timeouts, 1,
            "the listener closed with its owner, it did not time out"
        );
    }

    #[test]
    fn assembly_get_set_rules() {
        let mut d = dev();
        let now = Instant::now();
        let set = |inst: u16, data: Vec<u8>| {
            Request::new(service::SET_ATTRIBUTE_SINGLE, cip::Path::attribute(4, inst, 3)).with_data(data)
        };
        assert!(d.handle(&set(150, vec![5; 32]), PEER, now).status.is_success());
        assert_eq!(d.assembly(150).unwrap(), &[5u8; 32]);
        assert_eq!(
            d.handle(&set(100, vec![5; 32]), PEER, now).status,
            GeneralStatus::ATTRIBUTE_NOT_SETTABLE
        );
        assert_eq!(
            d.handle(&set(150, vec![5; 31]), PEER, now).status,
            GeneralStatus::NOT_ENOUGH_DATA
        );
        assert!(d.handle(&open_req(1, 150, 38, 34), PEER, now).status.is_success());
        assert_eq!(
            d.handle(&set(150, vec![5; 32]), PEER, now).status,
            GeneralStatus::DEVICE_STATE_CONFLICT
        );
        let size = d.handle(&Request::get_attribute_single(4, 151, 4), PEER, now);
        assert_eq!(size.data, vec![10, 0]);
    }

    #[test]
    fn unconnected_send_unwraps_to_a_local_request() {
        let mut d = dev();
        let now = Instant::now();
        let mut inner = Writer::default();
        Request::get_attribute_single(1, 1, 1).encode(&mut inner);
        let inner = inner.into_inner();
        let mut w = Writer::default();
        w.u8(0x0A)
            .u8(0xF0)
            .u16_le(u16::try_from(inner.len()).unwrap())
            .bytes(&inner);
        if inner.len() % 2 == 1 {
            w.u8(0);
        }
        w.u8(1).u8(0).u8(0x01).u8(0x00);
        let req = Request::new(io::service::UNCONNECTED_SEND, cip::Path::new(6, 1)).with_data(w.into_inner());
        let r = d.handle(&req, PEER, now);
        assert_eq!(r.data, vec![1, 0]);
        assert_eq!(r.request_service(), service::GET_ATTRIBUTE_SINGLE);
    }

    #[test]
    fn seq_newer_wraps() {
        assert!(seq_newer(1, 0));
        assert!(seq_newer(0, u32::MAX));
        assert!(!seq_newer(5, 5));
        assert!(!seq_newer(0, 1));
    }
}
