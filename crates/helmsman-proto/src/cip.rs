//! CIP application layer: logical paths, service codes, general status
//! codes, and the Message Router request/response envelopes that every
//! explicit message is wrapped in.
//!
//! References: `OpENer` `cipepath.h` (segment constants), `cipcommon.c`
//! `DecodePaddedEPath` (padded logical segment layout), `ciptypes.h`
//! (`CIPServiceCode`, `CipMessageRouterResponse`), `ciperror.h` (general
//! status codes), `cipmessagerouter.c` `CreateMessageRouterRequestStructure`
//! (request layout: service, path size in words, path, data). PUB00213R0
//! p.9 gives the same addressing model in prose: class, instance, attribute,
//! service code.

use std::fmt;

use crate::codec::{DecodeError, Reader, Writer};

/// Common service codes (`OpENer` `ciptypes.h`, `CIPServiceCode`).
pub mod service {
    pub const GET_ATTRIBUTE_ALL: u8 = 0x01;
    pub const SET_ATTRIBUTE_ALL: u8 = 0x02;
    pub const GET_ATTRIBUTE_LIST: u8 = 0x03;
    pub const SET_ATTRIBUTE_LIST: u8 = 0x04;
    pub const RESET: u8 = 0x05;
    pub const START: u8 = 0x06;
    pub const STOP: u8 = 0x07;
    pub const CREATE: u8 = 0x08;
    pub const DELETE: u8 = 0x09;
    pub const MULTIPLE_SERVICE_PACKET: u8 = 0x0A;
    pub const APPLY_ATTRIBUTES: u8 = 0x0D;
    pub const GET_ATTRIBUTE_SINGLE: u8 = 0x0E;
    pub const SET_ATTRIBUTE_SINGLE: u8 = 0x10;
    pub const FIND_NEXT_OBJECT_INSTANCE: u8 = 0x11;
    pub const RESTORE: u8 = 0x15;
    pub const SAVE: u8 = 0x16;
    pub const NO_OPERATION: u8 = 0x17;
    pub const GET_MEMBER: u8 = 0x18;
    pub const SET_MEMBER: u8 = 0x19;
    pub const INSERT_MEMBER: u8 = 0x1A;
    pub const REMOVE_MEMBER: u8 = 0x1B;
    pub const GROUP_SYNC: u8 = 0x1C;
    /// Set on the service byte of every reply.
    pub const REPLY_BIT: u8 = 0x80;
}

/// Well-known class codes.
pub mod class {
    pub const IDENTITY: u16 = 0x01;
    pub const MESSAGE_ROUTER: u16 = 0x02;
    pub const ASSEMBLY: u16 = 0x04;
    pub const CONNECTION_MANAGER: u16 = 0x06;
    pub const TCP_IP_INTERFACE: u16 = 0xF5;
    pub const ETHERNET_LINK: u16 = 0xF6;
}

/// General status codes (`OpENer` `ciperror.h`). Kept as a plain `u8`
/// newtype so unknown and vendor codes survive; `describe` names the ones
/// the reference lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GeneralStatus(pub u8);

impl GeneralStatus {
    pub const SUCCESS: Self = Self(0x00);
    pub const CONNECTION_FAILURE: Self = Self(0x01);
    pub const RESOURCE_UNAVAILABLE: Self = Self(0x02);
    pub const PATH_SEGMENT_ERROR: Self = Self(0x04);
    pub const PATH_DESTINATION_UNKNOWN: Self = Self(0x05);
    pub const PARTIAL_TRANSFER: Self = Self(0x06);
    pub const CONNECTION_LOST: Self = Self(0x07);
    pub const SERVICE_NOT_SUPPORTED: Self = Self(0x08);
    pub const INVALID_ATTRIBUTE_VALUE: Self = Self(0x09);
    pub const ATTRIBUTE_LIST_ERROR: Self = Self(0x0A);
    pub const ALREADY_IN_REQUESTED_MODE: Self = Self(0x0B);
    pub const OBJECT_STATE_CONFLICT: Self = Self(0x0C);
    pub const OBJECT_ALREADY_EXISTS: Self = Self(0x0D);
    pub const ATTRIBUTE_NOT_SETTABLE: Self = Self(0x0E);
    pub const PRIVILEGE_VIOLATION: Self = Self(0x0F);
    pub const DEVICE_STATE_CONFLICT: Self = Self(0x10);
    pub const REPLY_DATA_TOO_LARGE: Self = Self(0x11);
    pub const FRAGMENTATION_OF_PRIMITIVE: Self = Self(0x12);
    pub const NOT_ENOUGH_DATA: Self = Self(0x13);
    pub const ATTRIBUTE_NOT_SUPPORTED: Self = Self(0x14);
    pub const TOO_MUCH_DATA: Self = Self(0x15);
    pub const OBJECT_DOES_NOT_EXIST: Self = Self(0x16);
    pub const NO_STORED_ATTRIBUTE_DATA: Self = Self(0x18);
    pub const STORE_OPERATION_FAILURE: Self = Self(0x19);
    pub const ROUTING_REQUEST_TOO_LARGE: Self = Self(0x1A);
    pub const ROUTING_RESPONSE_TOO_LARGE: Self = Self(0x1B);
    pub const EMBEDDED_SERVICE_ERROR: Self = Self(0x1E);
    pub const VENDOR_SPECIFIC: Self = Self(0x1F);
    pub const INVALID_PARAMETER: Self = Self(0x20);
    pub const INVALID_REPLY_RECEIVED: Self = Self(0x22);
    pub const KEY_FAILURE_IN_PATH: Self = Self(0x25);
    pub const PATH_SIZE_INVALID: Self = Self(0x26);
    pub const ATTRIBUTE_NOT_GETTABLE: Self = Self(0x2C);
    pub const SERVICE_NOT_SUPPORTED_FOR_PATH: Self = Self(0x2E);

    #[must_use]
    pub const fn is_success(self) -> bool {
        self.0 == 0
    }

    /// Reference text for the code, or `None` for codes outside the table.
    #[must_use]
    pub const fn describe(self) -> Option<&'static str> {
        Some(match self.0 {
            0x00 => "success",
            0x01 => "connection failure",
            0x02 => "resource unavailable",
            0x04 => "path segment error",
            0x05 => "path destination unknown",
            0x06 => "partial transfer",
            0x07 => "connection lost",
            0x08 => "service not supported",
            0x09 => "invalid attribute value",
            0x0A => "attribute list error",
            0x0B => "already in requested mode",
            0x0C => "object state conflict",
            0x0D => "object already exists",
            0x0E => "attribute not settable",
            0x0F => "privilege violation",
            0x10 => "device state conflict",
            0x11 => "reply data too large",
            0x12 => "fragmentation of a primitive value",
            0x13 => "not enough data",
            0x14 => "attribute not supported",
            0x15 => "too much data",
            0x16 => "object does not exist",
            0x18 => "no stored attribute data",
            0x19 => "store operation failure",
            0x1A => "routing failure, request packet too large",
            0x1B => "routing failure, response packet too large",
            0x1E => "embedded service error",
            0x1F => "vendor specific error",
            0x20 => "invalid parameter",
            0x22 => "invalid reply received",
            0x25 => "key failure in path",
            0x26 => "path size invalid",
            0x2C => "attribute not gettable",
            0x2E => "service not supported for specified path",
            _ => return None,
        })
    }
}

impl fmt::Display for GeneralStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.describe() {
            Some(text) => write!(f, "0x{:02X} {text}", self.0),
            None => write!(f, "0x{:02X}", self.0),
        }
    }
}

/// A logical application path: class, instance, and optionally attribute.
/// Encoded as padded logical segments, 8-bit where the value fits and
/// 16-bit padded otherwise, which is the subset `OpENer` `DecodePaddedEPath`
/// accepts. Instance 0 addresses the class itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Path {
    pub class: u16,
    pub instance: u16,
    pub attribute: Option<u16>,
}

const SEGMENT_LOGICAL: u8 = 0x20;
const LOGICAL_CLASS: u8 = 0x00;
const LOGICAL_INSTANCE: u8 = 0x04;
const LOGICAL_ATTRIBUTE: u8 = 0x10;
const FORMAT_8BIT: u8 = 0x00;
const FORMAT_16BIT: u8 = 0x01;
const SEGMENT_RESERVED: u8 = 0xE0;

impl Path {
    #[must_use]
    pub const fn new(class: u16, instance: u16) -> Self {
        Self {
            class,
            instance,
            attribute: None,
        }
    }

    #[must_use]
    pub const fn attribute(class: u16, instance: u16, attribute: u16) -> Self {
        Self {
            class,
            instance,
            attribute: Some(attribute),
        }
    }

    fn segment(w: &mut Writer, logical_type: u8, value: u16) {
        if let Ok(v) = u8::try_from(value) {
            w.u8(SEGMENT_LOGICAL | logical_type | FORMAT_8BIT).u8(v);
        } else {
            w.u8(SEGMENT_LOGICAL | logical_type | FORMAT_16BIT)
                .u8(0)
                .u16_le(value);
        }
    }

    /// Path bytes without the size prefix. Always an even number of bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(12);
        Self::segment(&mut w, LOGICAL_CLASS, self.class);
        Self::segment(&mut w, LOGICAL_INSTANCE, self.instance);
        if let Some(a) = self.attribute {
            Self::segment(&mut w, LOGICAL_ATTRIBUTE, a);
        }
        w.into_inner()
    }

    /// Size of the encoded path in 16-bit words, as the request carries it.
    #[must_use]
    pub fn size_words(&self) -> u8 {
        u8::try_from(self.encode().len() / 2).expect("path longer than 255 words")
    }

    /// Decode `words * 2` bytes of padded logical segments from `r`.
    ///
    /// # Errors
    /// `Truncated` if the bytes are short; `Invalid` on a reserved or
    /// unsupported segment, or a class segment that is missing.
    pub fn decode(r: &mut Reader<'_>, words: u8) -> Result<Self, DecodeError> {
        let mut seg = r.sub(usize::from(words) * 2)?;
        let mut class = None;
        let mut instance = 0u16;
        let mut attribute = None;
        while !seg.is_empty() {
            let head = seg.u8()?;
            if head & SEGMENT_RESERVED == SEGMENT_RESERVED {
                return Err(DecodeError::Invalid("reserved path segment type"));
            }
            if head & SEGMENT_RESERVED != SEGMENT_LOGICAL {
                return Err(DecodeError::Invalid("only logical path segments are supported"));
            }
            let logical_type = head & 0x1C;
            let value = match head & 0x03 {
                FORMAT_8BIT => u16::from(seg.u8()?),
                FORMAT_16BIT => {
                    let _pad = seg.u8()?;
                    seg.u16_le()?
                }
                _ => return Err(DecodeError::Invalid("32-bit logical segments are not supported")),
            };
            match logical_type {
                LOGICAL_CLASS => class = Some(value),
                LOGICAL_INSTANCE => instance = value,
                LOGICAL_ATTRIBUTE => attribute = Some(value),
                _ => return Err(DecodeError::Invalid("unsupported logical segment type")),
            }
        }
        let class = class.ok_or(DecodeError::Invalid("path has no class segment"))?;
        Ok(Self {
            class,
            instance,
            attribute,
        })
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:02X}.{}", self.class, self.instance)?;
        if let Some(a) = self.attribute {
            write!(f, ".{a}")?;
        }
        Ok(())
    }
}

/// A Message Router request: service, path, service-specific data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub service: u8,
    pub path: Path,
    pub data: Vec<u8>,
}

impl Request {
    #[must_use]
    pub fn new(service: u8, path: Path) -> Self {
        Self {
            service,
            path,
            data: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.data = data;
        self
    }

    #[must_use]
    pub fn get_attribute_single(class: u16, instance: u16, attribute: u16) -> Self {
        Self::new(
            service::GET_ATTRIBUTE_SINGLE,
            Path::attribute(class, instance, attribute),
        )
    }

    #[must_use]
    pub fn get_attributes_all(class: u16, instance: u16) -> Self {
        Self::new(service::GET_ATTRIBUTE_ALL, Path::new(class, instance))
    }

    pub fn encode(&self, w: &mut Writer) {
        w.u8(self.service)
            .u8(self.path.size_words())
            .bytes(&self.path.encode())
            .bytes(&self.data);
    }

    /// # Errors
    /// `Truncated` on a short buffer; `Invalid` on a bad path.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let service = r.u8()?;
        if service & service::REPLY_BIT != 0 {
            return Err(DecodeError::Invalid("request service byte has the reply bit set"));
        }
        let words = r.u8()?;
        let path = Path::decode(&mut r, words)?;
        Ok(Self {
            service,
            path,
            data: r.rest().to_vec(),
        })
    }
}

/// A Message Router response (`OpENer` `CipMessageRouterResponse`): reply
/// service, reserved byte, general status, additional status words, data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The request's service code with [`service::REPLY_BIT`] set.
    pub service: u8,
    pub status: GeneralStatus,
    pub additional_status: Vec<u16>,
    pub data: Vec<u8>,
}

impl Response {
    #[must_use]
    pub fn ok(request_service: u8, data: Vec<u8>) -> Self {
        Self {
            service: request_service | service::REPLY_BIT,
            status: GeneralStatus::SUCCESS,
            additional_status: Vec::new(),
            data,
        }
    }

    #[must_use]
    pub fn error(request_service: u8, status: GeneralStatus) -> Self {
        Self {
            service: request_service | service::REPLY_BIT,
            status,
            additional_status: Vec::new(),
            data: Vec::new(),
        }
    }

    /// The request service this answers.
    #[must_use]
    pub const fn request_service(&self) -> u8 {
        self.service & !service::REPLY_BIT
    }

    /// # Panics
    /// If there are more than 255 additional status words.
    pub fn encode(&self, w: &mut Writer) {
        w.u8(self.service)
            .u8(0)
            .u8(self.status.0)
            .u8(u8::try_from(self.additional_status.len()).expect("additional status exceeds 255 words"));
        for word in &self.additional_status {
            w.u16_le(*word);
        }
        w.bytes(&self.data);
    }

    /// # Errors
    /// `Truncated` on a short buffer; `Invalid` if the reply bit is missing.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let service = r.u8()?;
        if service & service::REPLY_BIT == 0 {
            return Err(DecodeError::Invalid("response service byte lacks the reply bit"));
        }
        let _reserved = r.u8()?;
        let status = GeneralStatus(r.u8()?);
        let n = usize::from(r.u8()?);
        let mut additional_status = Vec::with_capacity(n);
        for _ in 0..n {
            additional_status.push(r.u16_le()?);
        }
        Ok(Self {
            service,
            status,
            additional_status,
            data: r.rest().to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_attribute_path_is_8bit_segments() {
        let p = Path::attribute(class::IDENTITY, 1, 7);
        assert_eq!(p.encode(), vec![0x20, 0x01, 0x24, 0x01, 0x30, 0x07]);
        assert_eq!(p.size_words(), 3);
        assert_eq!(p.to_string(), "0x01.1.7");
    }

    #[test]
    fn wide_values_use_padded_16bit_segments() {
        // 0xF5 still fits a byte; 300 and 0x1234 do not.
        let p = Path::attribute(class::TCP_IP_INTERFACE, 300, 0x1234);
        assert_eq!(
            p.encode(),
            vec![0x20, 0xF5, 0x25, 0x00, 0x2C, 0x01, 0x31, 0x00, 0x34, 0x12]
        );
        assert_eq!(p.size_words(), 5);
        let back = Path::decode(&mut Reader::new(&p.encode()), p.size_words()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn request_round_trip() {
        let req = Request::get_attribute_single(1, 1, 7);
        let mut w = Writer::default();
        req.encode(&mut w);
        let bytes = w.into_inner();
        assert_eq!(&bytes[..2], &[0x0E, 0x03]);
        assert_eq!(Request::decode(&bytes).unwrap(), req);
    }

    #[test]
    fn response_round_trip_with_additional_status() {
        let resp = Response {
            service: 0x8E,
            status: GeneralStatus::KEY_FAILURE_IN_PATH,
            additional_status: vec![0x0114],
            data: vec![1, 2, 3],
        };
        let mut w = Writer::default();
        resp.encode(&mut w);
        let bytes = w.into_inner();
        assert_eq!(&bytes[..6], &[0x8E, 0x00, 0x25, 0x01, 0x14, 0x01]);
        let back = Response::decode(&bytes).unwrap();
        assert_eq!(back, resp);
        assert_eq!(back.request_service(), 0x0E);
        assert_eq!(back.status.to_string(), "0x25 key failure in path");
    }

    #[test]
    fn reserved_segment_rejected() {
        assert_eq!(
            Path::decode(&mut Reader::new(&[0xE0, 0x01]), 1),
            Err(DecodeError::Invalid("reserved path segment type"))
        );
    }
}
