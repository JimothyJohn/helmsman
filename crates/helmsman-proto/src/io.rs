//! Connection Manager services for Class 0/1 I/O: `Forward_Open`,
//! `Forward_Close`, and the UDP packet framing once a connection is up.
//!
//! Field order and bit layouts from `OpENer`:
//! - `cipconnectionobject.c` `ConnectionObjectInitializeFromMessage`
//!   (request body order), `ConnectionObjectGetConnectionType/Priority/
//!   ConnectionSize` (network connection parameter bits), transport
//!   class/trigger masks 0x80 / 0x70 / 0x0F.
//! - `cipconnectionmanager.c` `AssembleForwardOpenResponse` /
//!   `AssembleForwardCloseResponse` / `ForwardClose` (reply and close
//!   layouts), `ParseConnectionPath` (electronic key, class, configuration
//!   instance, consumed then produced connection points, data segment).
//! - `cipepath.c` `GetElectronicKeyFormat4FromMessage` (key segment).
//! - `cipioconnection.c` `SendConnectedData` / `HandleReceivedIoConnectionData`
//!   (sequenced address item + connected data item, 16-bit sequence count
//!   for class 1, optional 32-bit run/idle header, connection size =
//!   data + 2 + 4-if-run-idle).
//! - `ciptypes.h` service codes 0x54 / 0x4E / 0x5B / 0x52.
//!
//! PUB00213R0 p.11 and glossary pp.23–26 describe the same in prose:
//! `Forward_Open` carries "transport class, production trigger, timing
//! information, electronic key and connection IDs"; Class 1 data is
//! "pre-pended with a 16-bit sequence count"; UDP port 2222.

use std::fmt;

use crate::cip::{self, GeneralStatus, Path};
use crate::codec::{DecodeError, Reader, Writer};
use crate::cpf::{self, Item};

/// Connection Manager service codes (`OpENer` `ciptypes.h`).
pub mod service {
    pub const FORWARD_OPEN: u8 = 0x54;
    pub const LARGE_FORWARD_OPEN: u8 = 0x5B;
    pub const FORWARD_CLOSE: u8 = 0x4E;
    pub const UNCONNECTED_SEND: u8 = 0x52;
}

/// Connection Manager instance every `Forward_Open` is addressed to.
#[must_use]
pub const fn connection_manager_path() -> Path {
    Path::new(cip::class::CONNECTION_MANAGER, 1)
}

/// Extended status words the Connection Manager returns with general
/// status 0x01 (`OpENer` `cipconnectionmanager.h`). Only the ones an
/// originator acts on are named.
pub mod extended {
    pub const CONNECTION_IN_USE: u16 = 0x0100;
    pub const TRANSPORT_TRIGGER_NOT_SUPPORTED: u16 = 0x0103;
    pub const OWNERSHIP_CONFLICT: u16 = 0x0106;
    pub const CONNECTION_NOT_FOUND: u16 = 0x0107;
    pub const RPI_NOT_SUPPORTED: u16 = 0x0111;
    pub const RPI_NOT_ACCEPTABLE: u16 = 0x0112;
    pub const NO_MORE_CONNECTIONS: u16 = 0x0113;
    pub const VENDOR_OR_PRODUCT_CODE_MISMATCH: u16 = 0x0114;
    pub const DEVICE_TYPE_MISMATCH: u16 = 0x0115;
    pub const REVISION_MISMATCH: u16 = 0x0116;
    pub const NON_LISTEN_ONLY_NOT_OPENED: u16 = 0x0119;
    pub const INVALID_O_TO_T_TYPE: u16 = 0x0123;
    pub const INVALID_T_TO_O_TYPE: u16 = 0x0124;
    pub const INVALID_CONFIGURATION_SIZE: u16 = 0x0126;
    pub const INVALID_O_TO_T_SIZE: u16 = 0x0127;
    pub const INVALID_T_TO_O_SIZE: u16 = 0x0128;
    pub const INVALID_CONSUMING_PATH: u16 = 0x012A;
    pub const INVALID_PRODUCING_PATH: u16 = 0x012B;
    pub const INCONSISTENT_PATH_COMBO: u16 = 0x012F;
    pub const CONNECTION_TIMED_OUT: u16 = 0x0203;
    pub const INVALID_SEGMENT_TYPE_IN_PATH: u16 = 0x0315;
}

/// Connection type bits 13–14 of the network connection parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ConnectionType {
    Null = 0,
    Multicast = 1,
    PointToPoint = 2,
}

/// Priority bits 10–11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Priority {
    Low = 0,
    High = 1,
    Scheduled = 2,
    Urgent = 3,
}

/// One direction's network connection parameters (16-bit form).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkParams {
    pub redundant_owner: bool,
    pub kind: ConnectionType,
    pub priority: Priority,
    pub variable_size: bool,
    /// Bytes per packet including the 16-bit sequence count and any
    /// run/idle header. Max 511 in the 16-bit form.
    pub size: u16,
}

impl NetworkParams {
    /// Fixed-size point-to-point parameters at the given priority.
    #[must_use]
    pub const fn point_to_point(size: u16, priority: Priority) -> Self {
        Self {
            redundant_owner: false,
            kind: ConnectionType::PointToPoint,
            priority,
            variable_size: false,
            size,
        }
    }

    /// Null type: this direction carries nothing (configuration-only or
    /// a null `Forward_Open`).
    #[must_use]
    pub const fn null() -> Self {
        Self {
            redundant_owner: false,
            kind: ConnectionType::Null,
            priority: Priority::Low,
            variable_size: false,
            size: 0,
        }
    }

    #[must_use]
    pub fn encode(&self) -> u16 {
        (u16::from(self.redundant_owner) << 15)
            | ((self.kind as u16) << 13)
            | ((self.priority as u16) << 10)
            | (u16::from(self.variable_size) << 9)
            | (self.size & 0x01FF)
    }

    /// # Errors
    /// `Invalid` for connection type 3, which the protocol reserves.
    pub fn decode(v: u16) -> Result<Self, DecodeError> {
        let kind = match (v >> 13) & 3 {
            0 => ConnectionType::Null,
            1 => ConnectionType::Multicast,
            2 => ConnectionType::PointToPoint,
            _ => return Err(DecodeError::Invalid("reserved connection type")),
        };
        let priority = match (v >> 10) & 3 {
            0 => Priority::Low,
            1 => Priority::High,
            2 => Priority::Scheduled,
            _ => Priority::Urgent,
        };
        Ok(Self {
            redundant_owner: v & 0x8000 != 0,
            kind,
            priority,
            variable_size: v & 0x0200 != 0,
            size: v & 0x01FF,
        })
    }
}

/// Production trigger bits 4–6 of the transport class/trigger byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProductionTrigger {
    Cyclic = 0,
    ChangeOfState = 1,
    Application = 2,
}

/// Transport class/trigger byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transport {
    /// Bit 7: server (target) behaviour. Originators send 0.
    pub server: bool,
    pub trigger: ProductionTrigger,
    /// Bits 0–3. Class 1 for I/O with sequence counts, 0 without, 3 for
    /// connected explicit messaging.
    pub class: u8,
}

impl Transport {
    pub const CLASS1_CYCLIC: Self = Self {
        server: false,
        trigger: ProductionTrigger::Cyclic,
        class: 1,
    };

    #[must_use]
    pub const fn encode(self) -> u8 {
        ((self.server as u8) << 7) | ((self.trigger as u8) << 4) | (self.class & 0x0F)
    }

    /// # Errors
    /// `Invalid` for an unknown production trigger.
    pub const fn decode(v: u8) -> Result<Self, DecodeError> {
        let trigger = match (v >> 4) & 7 {
            0 => ProductionTrigger::Cyclic,
            1 => ProductionTrigger::ChangeOfState,
            2 => ProductionTrigger::Application,
            _ => return Err(DecodeError::Invalid("unknown production trigger")),
        };
        Ok(Self {
            server: v & 0x80 != 0,
            trigger,
            class: v & 0x0F,
        })
    }
}

/// Electronic key segment, format 4 (`OpENer` `cipepath.c`): segment
/// 0x34, format 0x04, vendor, device type, product code, major revision
/// with the compatibility flag in bit 7, minor revision. 10 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElectronicKey {
    pub vendor_id: u16,
    pub device_type: u16,
    pub product_code: u16,
    pub major_revision: u8,
    pub minor_revision: u8,
    /// Accept a device that can emulate the requested key.
    pub compatibility: bool,
}

impl ElectronicKey {
    pub const LEN: usize = 10;

    pub fn encode(&self, w: &mut Writer) {
        w.u8(0x34)
            .u8(0x04)
            .u16_le(self.vendor_id)
            .u16_le(self.device_type)
            .u16_le(self.product_code)
            .u8((self.major_revision & 0x7F) | (u8::from(self.compatibility) << 7))
            .u8(self.minor_revision);
    }

    /// Decode after the 0x34 segment byte has been consumed.
    ///
    /// # Errors
    /// `Invalid` on a format other than 4; `Truncated` on short input.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        if r.u8()? != 0x04 {
            return Err(DecodeError::Invalid("electronic key format is not 4"));
        }
        let vendor_id = r.u16_le()?;
        let device_type = r.u16_le()?;
        let product_code = r.u16_le()?;
        let major = r.u8()?;
        let minor_revision = r.u8()?;
        Ok(Self {
            vendor_id,
            device_type,
            product_code,
            major_revision: major & 0x7F,
            minor_revision,
            compatibility: major & 0x80 != 0,
        })
    }
}

/// The connection path of a `Forward_Open` / `Forward_Close`: optional key,
/// application class (Assembly, 4, for I/O), optional configuration
/// instance, consumed (O->T) and produced (T->O) connection points, and
/// optional configuration data in a simple data segment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectionPath {
    pub key: Option<ElectronicKey>,
    pub class: u16,
    pub configuration: Option<u16>,
    /// O->T connection point: the assembly the target consumes.
    pub consumed: Option<u16>,
    /// T->O connection point: the assembly the target produces.
    pub produced: Option<u16>,
    /// Configuration data, padded to an even length on the wire.
    pub config_data: Vec<u8>,
}

const SEG_LOGICAL: u8 = 0x20;
const LOGICAL_CLASS: u8 = 0x00;
const LOGICAL_INSTANCE: u8 = 0x04;
const LOGICAL_CONNECTION_POINT: u8 = 0x0C;
const LOGICAL_SPECIAL: u8 = 0x14;
const SEG_DATA_SIMPLE: u8 = 0x80;

impl ConnectionPath {
    /// Assembly-class path with configuration, output, and input instances.
    #[must_use]
    pub fn assembly(configuration: u16, consumed: Option<u16>, produced: Option<u16>) -> Self {
        Self {
            key: None,
            class: cip::class::ASSEMBLY,
            configuration: Some(configuration),
            consumed,
            produced,
            config_data: Vec::new(),
        }
    }

    fn logical(w: &mut Writer, logical_type: u8, value: u16) {
        if let Ok(v) = u8::try_from(value) {
            w.u8(SEG_LOGICAL | logical_type).u8(v);
        } else {
            w.u8(SEG_LOGICAL | logical_type | 0x01).u8(0).u16_le(value);
        }
    }

    /// Path bytes, always an even count.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(24 + self.config_data.len());
        if let Some(k) = &self.key {
            k.encode(&mut w);
        }
        Self::logical(&mut w, LOGICAL_CLASS, self.class);
        if let Some(c) = self.configuration {
            Self::logical(&mut w, LOGICAL_INSTANCE, c);
        }
        if let Some(c) = self.consumed {
            Self::logical(&mut w, LOGICAL_CONNECTION_POINT, c);
        }
        if let Some(p) = self.produced {
            Self::logical(&mut w, LOGICAL_CONNECTION_POINT, p);
        }
        if !self.config_data.is_empty() {
            let words = self.config_data.len().div_ceil(2);
            w.u8(SEG_DATA_SIMPLE)
                .u8(u8::try_from(words).expect("config data exceeds 255 words"))
                .bytes(&self.config_data);
            if self.config_data.len() % 2 == 1 {
                w.u8(0);
            }
        }
        w.into_inner()
    }

    #[must_use]
    pub fn size_words(&self) -> u8 {
        u8::try_from(self.encode().len() / 2).expect("connection path exceeds 255 words")
    }

    /// Decode `words * 2` bytes. Tolerant about which of instance /
    /// connection-point segment types carry the points, like `OpENer`.
    /// The first instance-type segment after the class is the configuration
    /// instance; connection points fill consumed then produced.
    ///
    /// # Errors
    /// `Truncated` on short input; `Invalid` on unknown segments.
    pub fn decode(r: &mut Reader<'_>, words: u8) -> Result<Self, DecodeError> {
        let mut seg = r.sub(usize::from(words) * 2)?;
        let mut out = Self::default();
        let mut class = None;
        let mut points: Vec<u16> = Vec::new();
        while !seg.is_empty() {
            let head = seg.u8()?;
            match head & 0xE0 {
                SEG_LOGICAL => {
                    let logical_type = head & 0x1C;
                    if logical_type == LOGICAL_SPECIAL {
                        if head & 0x03 != 0 {
                            return Err(DecodeError::Invalid("unsupported special segment"));
                        }
                        out.key = Some(ElectronicKey::decode(&mut seg)?);
                        continue;
                    }
                    let value = match head & 0x03 {
                        0 => u16::from(seg.u8()?),
                        1 => {
                            let _pad = seg.u8()?;
                            seg.u16_le()?
                        }
                        _ => return Err(DecodeError::Invalid("32-bit logical segment in connection path")),
                    };
                    match logical_type {
                        LOGICAL_CLASS => class = Some(value),
                        LOGICAL_INSTANCE if out.configuration.is_none() && points.is_empty() => {
                            out.configuration = Some(value);
                        }
                        LOGICAL_INSTANCE | LOGICAL_CONNECTION_POINT => points.push(value),
                        _ => {
                            return Err(DecodeError::Invalid(
                                "unsupported logical segment in connection path",
                            ));
                        }
                    }
                }
                SEG_DATA_SIMPLE if head == SEG_DATA_SIMPLE => {
                    let words = usize::from(seg.u8()?);
                    out.config_data = seg.bytes(words * 2)?.to_vec();
                }
                _ => return Err(DecodeError::Invalid("unsupported segment in connection path")),
            }
        }
        out.class = class.ok_or(DecodeError::Invalid("connection path has no class"))?;
        if points.len() > 2 {
            return Err(DecodeError::Invalid("more than two connection points"));
        }
        let mut it = points.into_iter();
        out.consumed = it.next();
        out.produced = it.next();
        Ok(out)
    }
}

/// A `Forward_Open` request body (16-bit network parameter form).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardOpen {
    /// Tick time exponent (bits 0–3); targets ignore it for I/O.
    pub priority_time_tick: u8,
    pub timeout_ticks: u8,
    /// Zero for point-to-point: the target picks it (`OpENer`
    /// `cipconnectionobject.c`, "the target shall choose the connection ID").
    pub o_to_t_connection_id: u32,
    /// Chosen by the originator for point-to-point T->O.
    pub t_to_o_connection_id: u32,
    pub connection_serial: u16,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    /// Inactivity timeout = RPI × 2^(multiplier + 2)
    /// (`OpENer` `ConnectionObjectCalculateRegularInactivityWatchdogTimerValue`).
    pub timeout_multiplier: u8,
    pub o_to_t_rpi_us: u32,
    pub o_to_t: NetworkParams,
    pub t_to_o_rpi_us: u32,
    pub t_to_o: NetworkParams,
    pub transport: Transport,
    pub path: ConnectionPath,
}

impl ForwardOpen {
    pub fn encode(&self, w: &mut Writer) {
        w.u8(self.priority_time_tick)
            .u8(self.timeout_ticks)
            .u32_le(self.o_to_t_connection_id)
            .u32_le(self.t_to_o_connection_id)
            .u16_le(self.connection_serial)
            .u16_le(self.originator_vendor_id)
            .u32_le(self.originator_serial)
            .u8(self.timeout_multiplier)
            .bytes(&[0, 0, 0])
            .u32_le(self.o_to_t_rpi_us)
            .u16_le(self.o_to_t.encode())
            .u32_le(self.t_to_o_rpi_us)
            .u16_le(self.t_to_o.encode())
            .u8(self.transport.encode())
            .u8(self.path.size_words())
            .bytes(&self.path.encode());
    }

    /// The Message Router request addressed to the Connection Manager.
    #[must_use]
    pub fn to_request(&self) -> cip::Request {
        let mut w = Writer::with_capacity(64);
        self.encode(&mut w);
        cip::Request::new(service::FORWARD_OPEN, connection_manager_path()).with_data(w.into_inner())
    }

    /// # Errors
    /// `Truncated` / `Invalid` on malformed bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let priority_time_tick = r.u8()?;
        let timeout_ticks = r.u8()?;
        let o_to_t_connection_id = r.u32_le()?;
        let t_to_o_connection_id = r.u32_le()?;
        let connection_serial = r.u16_le()?;
        let originator_vendor_id = r.u16_le()?;
        let originator_serial = r.u32_le()?;
        let timeout_multiplier = r.u8()?;
        let _reserved = r.bytes(3)?;
        let o_to_t_rpi_us = r.u32_le()?;
        let o_to_t = NetworkParams::decode(r.u16_le()?)?;
        let t_to_o_rpi_us = r.u32_le()?;
        let t_to_o = NetworkParams::decode(r.u16_le()?)?;
        let transport = Transport::decode(r.u8()?)?;
        let words = r.u8()?;
        let path = ConnectionPath::decode(&mut r, words)?;
        if !r.is_empty() {
            return Err(DecodeError::Invalid("bytes after Forward_Open connection path"));
        }
        Ok(Self {
            priority_time_tick,
            timeout_ticks,
            o_to_t_connection_id,
            t_to_o_connection_id,
            connection_serial,
            originator_vendor_id,
            originator_serial,
            timeout_multiplier,
            o_to_t_rpi_us,
            o_to_t,
            t_to_o_rpi_us,
            t_to_o,
            transport,
            path,
        })
    }
}

/// A successful `Forward_Open` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardOpenReply {
    pub o_to_t_connection_id: u32,
    pub t_to_o_connection_id: u32,
    pub connection_serial: u16,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    /// Actual packet intervals granted, microseconds.
    pub o_to_t_api_us: u32,
    pub t_to_o_api_us: u32,
    pub application_reply: Vec<u8>,
}

impl ForwardOpenReply {
    pub fn encode(&self, w: &mut Writer) {
        w.u32_le(self.o_to_t_connection_id)
            .u32_le(self.t_to_o_connection_id)
            .u16_le(self.connection_serial)
            .u16_le(self.originator_vendor_id)
            .u32_le(self.originator_serial)
            .u32_le(self.o_to_t_api_us)
            .u32_le(self.t_to_o_api_us)
            .u8(u8::try_from(self.application_reply.len() / 2).expect("application reply exceeds 255 words"))
            .u8(0)
            .bytes(&self.application_reply);
    }

    /// # Errors
    /// `Truncated` on short input. A short application reply is tolerated.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let o_to_t_connection_id = r.u32_le()?;
        let t_to_o_connection_id = r.u32_le()?;
        let connection_serial = r.u16_le()?;
        let originator_vendor_id = r.u16_le()?;
        let originator_serial = r.u32_le()?;
        let o_to_t_api_us = r.u32_le()?;
        let t_to_o_api_us = r.u32_le()?;
        let words = usize::from(r.u8()?);
        let _reserved = r.u8()?;
        let want = words * 2;
        let application_reply = r.bytes(want.min(r.remaining()))?.to_vec();
        Ok(Self {
            o_to_t_connection_id,
            t_to_o_connection_id,
            connection_serial,
            originator_vendor_id,
            originator_serial,
            o_to_t_api_us,
            t_to_o_api_us,
            application_reply,
        })
    }
}

/// The body of an unsuccessful `Forward_Open` or `Forward_Close` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: GeneralStatus,
    /// Extended status words; the first is the Connection Manager code.
    pub extended: Vec<u16>,
    pub connection_serial: u16,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    pub remaining_path_size: u8,
}

impl Refused {
    /// # Errors
    /// `Truncated` on short input.
    pub fn decode(resp: &cip::Response) -> Result<Self, DecodeError> {
        let mut r = Reader::new(&resp.data);
        let connection_serial = r.u16_le()?;
        let originator_vendor_id = r.u16_le()?;
        let originator_serial = r.u32_le()?;
        let remaining_path_size = r.u8()?;
        let _reserved = r.u8()?;
        Ok(Self {
            status: resp.status,
            extended: resp.additional_status.clone(),
            connection_serial,
            originator_vendor_id,
            originator_serial,
            remaining_path_size,
        })
    }

    #[must_use]
    pub fn extended_code(&self) -> Option<u16> {
        self.extended.first().copied()
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused: {}", self.status)?;
        if let Some(code) = self.extended_code() {
            write!(f, " extended 0x{code:04X}")?;
        }
        if self.extended.len() > 1 {
            write!(f, " {:04X?}", &self.extended[1..])?;
        }
        Ok(())
    }
}

/// Interpret a Connection Manager response as success data or a refusal.
///
/// # Errors
/// `Truncated` if either shape is short.
pub fn open_result(resp: &cip::Response) -> Result<Result<ForwardOpenReply, Refused>, DecodeError> {
    if resp.status.is_success() {
        Ok(Ok(ForwardOpenReply::decode(&resp.data)?))
    } else {
        Ok(Err(Refused::decode(resp)?))
    }
}

/// A `Forward_Close` request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardClose {
    pub priority_time_tick: u8,
    pub timeout_ticks: u8,
    pub connection_serial: u16,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    pub path: ConnectionPath,
}

impl ForwardClose {
    pub fn encode(&self, w: &mut Writer) {
        w.u8(self.priority_time_tick)
            .u8(self.timeout_ticks)
            .u16_le(self.connection_serial)
            .u16_le(self.originator_vendor_id)
            .u32_le(self.originator_serial)
            .u8(self.path.size_words())
            .u8(0)
            .bytes(&self.path.encode());
    }

    #[must_use]
    pub fn to_request(&self) -> cip::Request {
        let mut w = Writer::with_capacity(32);
        self.encode(&mut w);
        cip::Request::new(service::FORWARD_CLOSE, connection_manager_path()).with_data(w.into_inner())
    }

    /// # Errors
    /// `Truncated` / `Invalid` on malformed bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let priority_time_tick = r.u8()?;
        let timeout_ticks = r.u8()?;
        let connection_serial = r.u16_le()?;
        let originator_vendor_id = r.u16_le()?;
        let originator_serial = r.u32_le()?;
        let words = r.u8()?;
        let _reserved = r.u8()?;
        let path = ConnectionPath::decode(&mut r, words)?;
        Ok(Self {
            priority_time_tick,
            timeout_ticks,
            connection_serial,
            originator_vendor_id,
            originator_serial,
            path,
        })
    }
}

/// A successful `Forward_Close` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardCloseReply {
    pub connection_serial: u16,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    pub application_reply: Vec<u8>,
}

impl ForwardCloseReply {
    pub fn encode(&self, w: &mut Writer) {
        w.u16_le(self.connection_serial)
            .u16_le(self.originator_vendor_id)
            .u32_le(self.originator_serial)
            .u8(u8::try_from(self.application_reply.len() / 2).expect("application reply exceeds 255 words"))
            .u8(0)
            .bytes(&self.application_reply);
    }

    /// # Errors
    /// `Truncated` on short input.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let connection_serial = r.u16_le()?;
        let originator_vendor_id = r.u16_le()?;
        let originator_serial = r.u32_le()?;
        let words = usize::from(r.u8()?);
        let _reserved = r.u8()?;
        let want = words * 2;
        let application_reply = r.bytes(want.min(r.remaining()))?.to_vec();
        Ok(Self {
            connection_serial,
            originator_vendor_id,
            originator_serial,
            application_reply,
        })
    }
}

/// Interpret a `Forward_Close` response.
///
/// # Errors
/// `Truncated` if either shape is short.
pub fn close_result(resp: &cip::Response) -> Result<Result<ForwardCloseReply, Refused>, DecodeError> {
    if resp.status.is_success() {
        Ok(Ok(ForwardCloseReply::decode(&resp.data)?))
    } else {
        Ok(Err(Refused::decode(resp)?))
    }
}

/// One UDP I/O datagram: a sequenced address item (connection ID plus a
/// 32-bit encapsulation sequence that the receiver uses to drop stale
/// packets) and a connected data item whose body is class-dependent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoPacket<'a> {
    pub connection_id: u32,
    /// `None` when the address item is the plain connection address (class 0
    /// without sequencing).
    pub encapsulation_sequence: Option<u32>,
    /// Class 1: 16-bit sequence count, optional run/idle header, data.
    pub body: &'a [u8],
}

impl<'a> IoPacket<'a> {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut addr = Writer::with_capacity(8);
        addr.u32_le(self.connection_id);
        if let Some(s) = self.encapsulation_sequence {
            addr.u32_le(s);
        }
        let addr = addr.into_inner();
        let items = [
            Item {
                type_id: if self.encapsulation_sequence.is_some() {
                    cpf::item::SEQUENCED_ADDRESS
                } else {
                    cpf::item::CONNECTION_ADDRESS
                },
                data: &addr,
            },
            Item {
                type_id: cpf::item::CONNECTED_DATA,
                data: self.body,
            },
        ];
        let mut w = Writer::with_capacity(16 + self.body.len());
        cpf::encode(&items, &mut w);
        w.into_inner()
    }

    /// # Errors
    /// `Truncated` / `Invalid` for anything that isn't address + data.
    pub fn decode(datagram: &'a [u8]) -> Result<Self, DecodeError> {
        let items = cpf::decode(datagram)?;
        let [addr, data] = items.as_slice() else {
            return Err(DecodeError::Invalid(
                "I/O datagram must carry exactly two CPF items",
            ));
        };
        if data.type_id != cpf::item::CONNECTED_DATA {
            return Err(DecodeError::Invalid("second I/O item is not connected data"));
        }
        let mut r = Reader::new(addr.data);
        let (connection_id, encapsulation_sequence) = match addr.type_id {
            cpf::item::SEQUENCED_ADDRESS => (r.u32_le()?, Some(r.u32_le()?)),
            cpf::item::CONNECTION_ADDRESS => (r.u32_le()?, None),
            _ => return Err(DecodeError::Invalid("first I/O item is not an address")),
        };
        if !r.is_empty() {
            return Err(DecodeError::Invalid("address item has trailing bytes"));
        }
        Ok(Self {
            connection_id,
            encapsulation_sequence,
            body: data.data,
        })
    }
}

/// Class 1 connected-data body: sequence count, optional 32-bit run/idle
/// header, then application data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class1Body<'a> {
    pub sequence_count: u16,
    pub run_idle: Option<u32>,
    pub data: &'a [u8],
}

impl<'a> Class1Body<'a> {
    pub const RUN: u32 = 1;
    pub const IDLE: u32 = 0;

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(6 + self.data.len());
        w.u16_le(self.sequence_count);
        if let Some(h) = self.run_idle {
            w.u32_le(h);
        }
        w.bytes(self.data);
        w.into_inner()
    }

    /// `has_header` is known from the connection, not the bytes: a
    /// heartbeat body is just the sequence count either way.
    ///
    /// # Errors
    /// `Truncated` if the body is shorter than its headers.
    pub fn decode(body: &'a [u8], has_header: bool) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        let sequence_count = r.u16_le()?;
        let run_idle = if has_header && !r.is_empty() {
            Some(r.u32_le()?)
        } else {
            None
        };
        Ok(Self {
            sequence_count,
            run_idle,
            data: r.rest(),
        })
    }
}

/// Bytes a Class 1 direction occupies on the wire for `data_len` bytes of
/// application data (`OpENer` `cipioconnection.c`: +2 sequence count, +4
/// run/idle header unless the direction is a heartbeat).
#[must_use]
pub const fn class1_size(data_len: u16, run_idle_header: bool) -> u16 {
    if data_len == 0 {
        2
    } else if run_idle_header {
        data_len + 6
    } else {
        data_len + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_open() -> ForwardOpen {
        ForwardOpen {
            priority_time_tick: 0x0A,
            timeout_ticks: 0xF0,
            o_to_t_connection_id: 0,
            t_to_o_connection_id: 0x1234_5678,
            connection_serial: 0x0101,
            originator_vendor_id: 0xFFFF,
            originator_serial: 0xDEAD_BEEF,
            timeout_multiplier: 1,
            o_to_t_rpi_us: 100_000,
            o_to_t: NetworkParams::point_to_point(class1_size(32, true), Priority::Scheduled),
            t_to_o_rpi_us: 100_000,
            t_to_o: NetworkParams::point_to_point(class1_size(32, false), Priority::Scheduled),
            transport: Transport::CLASS1_CYCLIC,
            path: ConnectionPath::assembly(151, Some(150), Some(100)),
        }
    }

    #[test]
    fn network_params_bits() {
        let p = NetworkParams::point_to_point(38, Priority::Scheduled);
        assert_eq!(p.encode(), 0x4800 | 0x0026);
        assert_eq!(NetworkParams::decode(p.encode()).unwrap(), p);
        assert!(NetworkParams::decode(0x6000).is_err());
        assert_eq!(NetworkParams::null().encode(), 0);
    }

    #[test]
    fn transport_byte() {
        assert_eq!(Transport::CLASS1_CYCLIC.encode(), 0x01);
        let t = Transport {
            server: true,
            trigger: ProductionTrigger::ChangeOfState,
            class: 3,
        };
        assert_eq!(t.encode(), 0x93);
        assert_eq!(Transport::decode(0x93).unwrap(), t);
    }

    #[test]
    fn sizes_follow_opener_rules() {
        assert_eq!(class1_size(32, true), 38);
        assert_eq!(class1_size(32, false), 34);
        assert_eq!(class1_size(0, true), 2);
    }

    #[test]
    fn assembly_path_matches_the_eipscanner_shape() {
        // EIPScanner ImplicitMessagingExample: {0x20,0x04, 0x24,151, 0x2C,150, 0x2C,100}
        let p = ConnectionPath::assembly(151, Some(150), Some(100));
        assert_eq!(p.encode(), vec![0x20, 0x04, 0x24, 151, 0x2C, 150, 0x2C, 100]);
        assert_eq!(p.size_words(), 4);
        let back = ConnectionPath::decode(&mut Reader::new(&p.encode()), 4).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn key_and_config_data_round_trip() {
        let mut p = ConnectionPath::assembly(151, Some(150), Some(100));
        p.key = Some(ElectronicKey {
            vendor_id: 1,
            device_type: 12,
            product_code: 65001,
            major_revision: 2,
            minor_revision: 3,
            compatibility: true,
        });
        p.config_data = vec![1, 2, 3];
        let bytes = p.encode();
        assert_eq!(&bytes[..2], &[0x34, 0x04]);
        assert_eq!(bytes[8], 0x82);
        assert_eq!(bytes.len() % 2, 0);
        let back = ConnectionPath::decode(&mut Reader::new(&bytes), p.size_words()).unwrap();
        assert_eq!(back.key, p.key);
        assert_eq!(back.config_data, vec![1, 2, 3, 0]);
    }

    #[test]
    fn forward_open_round_trip_and_header_length() {
        let fo = sample_open();
        let req = fo.to_request();
        assert_eq!(req.service, 0x54);
        assert_eq!(req.path, Path::new(6, 1));
        // `OpENer` g_kForwardOpenHeaderLength = 36 up to and including the path size.
        assert_eq!(req.data.len(), 36 + 8);
        assert_eq!(ForwardOpen::decode(&req.data).unwrap(), fo);
    }

    #[test]
    fn reply_and_refusal_round_trip() {
        let reply = ForwardOpenReply {
            o_to_t_connection_id: 7,
            t_to_o_connection_id: 0x1234_5678,
            connection_serial: 0x0101,
            originator_vendor_id: 0xFFFF,
            originator_serial: 0xDEAD_BEEF,
            o_to_t_api_us: 100_000,
            t_to_o_api_us: 100_000,
            application_reply: vec![],
        };
        let mut w = Writer::default();
        reply.encode(&mut w);
        let bytes = w.into_inner();
        assert_eq!(bytes.len(), 26);
        let resp = cip::Response::ok(service::FORWARD_OPEN, bytes);
        assert_eq!(open_result(&resp).unwrap().unwrap(), reply);

        let mut refused = cip::Response::error(service::FORWARD_OPEN, GeneralStatus::CONNECTION_FAILURE);
        refused.additional_status = vec![extended::INVALID_O_TO_T_SIZE, 38];
        refused.data = vec![0x01, 0x01, 0xFF, 0xFF, 0xEF, 0xBE, 0xAD, 0xDE, 0, 0];
        let r = open_result(&refused).unwrap().unwrap_err();
        assert_eq!(r.extended_code(), Some(0x0127));
        assert_eq!(r.extended[1], 38);
        assert_eq!(r.connection_serial, 0x0101);
        assert_eq!(
            r.to_string(),
            "refused: 0x01 connection failure extended 0x0127 [0026]"
        );
    }

    #[test]
    fn forward_close_round_trip() {
        let fc = ForwardClose {
            priority_time_tick: 0x0A,
            timeout_ticks: 0xF0,
            connection_serial: 1,
            originator_vendor_id: 2,
            originator_serial: 3,
            path: ConnectionPath::assembly(151, Some(150), Some(100)),
        };
        let req = fc.to_request();
        assert_eq!(req.service, 0x4E);
        assert_eq!(req.data[10], 4);
        assert_eq!(ForwardClose::decode(&req.data).unwrap(), fc);
    }

    #[test]
    fn io_packet_round_trip() {
        let body = Class1Body {
            sequence_count: 9,
            run_idle: Some(Class1Body::RUN),
            data: &[1, 2, 3, 4],
        }
        .encode();
        assert_eq!(body, vec![9, 0, 1, 0, 0, 0, 1, 2, 3, 4]);
        let pkt = IoPacket {
            connection_id: 0xAABB_CCDD,
            encapsulation_sequence: Some(42),
            body: &body,
        };
        let bytes = pkt.encode();
        assert_eq!(&bytes[..4], &[2, 0, 0x02, 0x80]);
        let back = IoPacket::decode(&bytes).unwrap();
        assert_eq!(back, pkt);
        let b = Class1Body::decode(back.body, true).unwrap();
        assert_eq!(b.sequence_count, 9);
        assert_eq!(b.run_idle, Some(1));
        assert_eq!(b.data, &[1, 2, 3, 4]);
    }

    #[test]
    fn heartbeat_body_has_no_header_even_when_configured() {
        let b = Class1Body::decode(&[5, 0], true).unwrap();
        assert_eq!(b.sequence_count, 5);
        assert_eq!(b.run_idle, None);
        assert_eq!(b.data, &[]);
    }
}
