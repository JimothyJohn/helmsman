//! DCP, the discovery and basic configuration protocol: finding a station
//! by its name on the wire (Identify), and giving one its IP address or
//! its name (Set). Layer 2 only, so it works before a device has an
//! address.
//!
//! What is here is the payload after the `FrameID` ([`crate::eth`]): a
//! 10-byte header (service, type, transaction id, response delay, data
//! length) and blocks of option, suboption, length, value, each padded to
//! an even length. Layouts and numbers as p-net's `pf_dcp.c` / `pf_dcp.h`
//! and Wireshark's `packet-pn-dcp.c` have them (docs/SOURCES.md).

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::{DecodeError, Reader, Writer};
use crate::eth::Mac;

/// Where an Identify request goes (p-net `pf_dcp.c`, Wireshark
/// `packet-pn-dcp.c`): the PROFINET identify multicast group.
pub const MULTICAST_IDENTIFY: Mac = [0x01, 0x0E, 0xCF, 0x00, 0x00, 0x00];

pub const SERVICE_GET: u8 = 0x03;
pub const SERVICE_SET: u8 = 0x04;
pub const SERVICE_IDENTIFY: u8 = 0x05;

pub const TYPE_REQUEST: u8 = 0x00;
pub const TYPE_SUCCESS: u8 = 0x01;
pub const TYPE_NOT_SUPPORTED: u8 = 0x05;

pub const OPT_IP: u8 = 0x01;
pub const OPT_DEVICE: u8 = 0x02;
pub const OPT_CONTROL: u8 = 0x05;
pub const OPT_ALL: u8 = 0xFF;

pub const SUB_IP_PARAMETER: u8 = 0x02;
/// "Type of station", the vendor's free text.
pub const SUB_DEVICE_TYPE: u8 = 0x01;
pub const SUB_DEVICE_NAME: u8 = 0x02;
pub const SUB_DEVICE_ID: u8 = 0x03;
pub const SUB_DEVICE_ROLE: u8 = 0x04;
pub const SUB_DEVICE_OPTIONS: u8 = 0x05;
pub const SUB_DEVICE_INSTANCE: u8 = 0x07;
pub const SUB_CONTROL_RESPONSE: u8 = 0x04;
pub const SUB_ALL: u8 = 0xFF;

/// Device role bits: an IO device, an IO controller.
pub const ROLE_DEVICE: u8 = 0x01;
pub const ROLE_CONTROLLER: u8 = 0x02;

/// A Set block's qualifier: keep the value across a power cycle, or only
/// until then.
pub const QUALIFIER_TEMPORARY: u16 = 0x0000;
pub const QUALIFIER_PERMANENT: u16 = 0x0001;

/// Block errors of a Set response; 0 is success.
pub const ERROR_NONE: u8 = 0x00;
pub const ERROR_OPTION_UNSUPPORTED: u8 = 0x01;
pub const ERROR_SUBOPTION_UNSUPPORTED: u8 = 0x02;
pub const ERROR_SUBOPTION_NOT_SET: u8 = 0x03;
pub const ERROR_RESOURCE: u8 = 0x04;
pub const ERROR_SET_NOT_POSSIBLE: u8 = 0x05;
pub const ERROR_IN_OPERATION: u8 = 0x06;

const HEADER_LEN: usize = 10;
/// The longest station name.
pub const MAX_NAME: usize = 240;

/// An IPv4 address, in network order.
pub type Ip = [u8; 4];

/// Whether `name` is a station name this crate will put on the wire:
/// 1..=240 characters, labels of 1..=63 of `a-z 0-9 -` separated by dots,
/// no label starting or ending with a hyphen. Deliberately the narrow
/// DNS-label form, so a name accepted here is one every device takes; a
/// name read from a device is not held to it.
#[must_use]
pub fn valid_station_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The 10-byte DCP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub service: u8,
    pub service_type: u8,
    /// The transaction id a response echoes.
    pub xid: u32,
    /// In a multicast Identify request: a responder waits up to
    /// (factor - 1) x 10 ms, spread by its MAC address (p-net
    /// `pf_dcp_calculate_response_delay`). 1 is "answer at once".
    pub response_delay: u16,
}

/// One block: option, suboption and its value (in a response the value
/// starts with a 16-bit block info, in a Set request with a qualifier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block<'a> {
    pub option: u8,
    pub sub: u8,
    pub value: &'a [u8],
}

/// Splits a DCP payload into its header and blocks. Bytes after the
/// header's data length (the frame's padding) are ignored.
///
/// # Errors
/// Truncated, or a block running past the data length.
pub fn parse(payload: &[u8]) -> Result<(Header, Vec<Block<'_>>), DecodeError> {
    let mut r = Reader::new(payload);
    let header = Header {
        service: r.u8()?,
        service_type: r.u8()?,
        xid: r.u32()?,
        response_delay: r.u16()?,
    };
    let data_len = usize::from(r.u16()?);
    let mut data = r.sub(data_len)?;
    let mut blocks = Vec::new();
    while !data.is_empty() {
        let option = data.u8()?;
        let sub = data.u8()?;
        let len = usize::from(data.u16()?);
        let value = data.bytes(len)?;
        if len % 2 == 1 && !data.is_empty() {
            data.u8()?;
        }
        blocks.push(Block { option, sub, value });
    }
    Ok((header, blocks))
}

struct Pdu {
    w: Writer,
}

impl Pdu {
    fn new(service: u8, service_type: u8, xid: u32, response_delay: u16) -> Self {
        let mut w = Writer::with_capacity(64);
        w.u8(service).u8(service_type).u32(xid).u16(response_delay).u16(0);
        Self { w }
    }

    /// A block whose value is `head` then `tail`, padded to even.
    fn block(&mut self, option: u8, sub: u8, head: &[u8], tail: &[u8]) {
        let len = head.len() + tail.len();
        // Every value written here is far below 64 KiB: names are capped
        // at MAX_NAME by the callers, the rest are fixed.
        let len16 = u16::try_from(len).unwrap_or(u16::MAX);
        self.w.u8(option).u8(sub).u16(len16).bytes(head).bytes(tail);
        if len % 2 == 1 {
            self.w.u8(0);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let data_len = u16::try_from(self.w.len() - HEADER_LEN).unwrap_or(u16::MAX);
        self.w.set_u16(8, data_len);
        self.w.into_inner()
    }
}

/// An Identify request: for the station called `name`, or for every
/// station when `None`. Sent to [`MULTICAST_IDENTIFY`] with `FrameID`
/// [`crate::eth::FRAME_ID_DCP_IDENTIFY_REQ`].
#[must_use]
pub fn identify_request(xid: u32, name: Option<&str>, response_delay: u16) -> Vec<u8> {
    let mut p = Pdu::new(SERVICE_IDENTIFY, TYPE_REQUEST, xid, response_delay);
    match name {
        Some(n) => p.block(OPT_DEVICE, SUB_DEVICE_NAME, n.as_bytes(), &[]),
        None => p.block(OPT_ALL, SUB_ALL, &[], &[]),
    }
    p.finish()
}

/// What a station says about itself in an Identify response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Station {
    /// Its station name; empty when it has none yet.
    pub name: String,
    pub vendor_id: u16,
    pub device_id: u16,
    /// [`ROLE_DEVICE`], [`ROLE_CONTROLLER`] bits.
    pub role: u8,
    /// The vendor's "type of station" text.
    pub type_of_station: String,
    pub ip: Ip,
    pub mask: Ip,
    pub gateway: Ip,
    /// The device instance, when the station reports one.
    pub instance: Option<u16>,
}

/// An Identify response for `station`, as a device sends it (unicast to
/// the asker, `FrameID` [`crate::eth::FRAME_ID_DCP_IDENTIFY_RES`]).
#[must_use]
pub fn identify_response(xid: u32, station: &Station) -> Vec<u8> {
    let mut p = Pdu::new(SERVICE_IDENTIFY, TYPE_SUCCESS, xid, 0);
    let info = [0, 0];
    let mut ip = Vec::with_capacity(12);
    ip.extend_from_slice(&station.ip);
    ip.extend_from_slice(&station.mask);
    ip.extend_from_slice(&station.gateway);
    // Block info of the IP block: bit 0 says an address is set.
    let ip_info = [0, u8::from(station.ip != [0; 4])];
    p.block(OPT_IP, SUB_IP_PARAMETER, &ip_info, &ip);
    let ids = [station.vendor_id.to_be_bytes(), station.device_id.to_be_bytes()].concat();
    p.block(OPT_DEVICE, SUB_DEVICE_ID, &info, &ids);
    p.block(
        OPT_DEVICE,
        SUB_DEVICE_TYPE,
        &info,
        station.type_of_station.as_bytes(),
    );
    p.block(OPT_DEVICE, SUB_DEVICE_NAME, &info, station.name.as_bytes());
    p.block(OPT_DEVICE, SUB_DEVICE_ROLE, &info, &[station.role, 0]);
    if let Some(instance) = station.instance {
        p.block(OPT_DEVICE, SUB_DEVICE_INSTANCE, &info, &instance.to_be_bytes());
    }
    p.finish()
}

fn text(bytes: &[u8]) -> Result<String, DecodeError> {
    core::str::from_utf8(bytes)
        .map(String::from)
        .map_err(|_| DecodeError::Invalid("DCP text is not UTF-8"))
}

/// Reads an Identify response: its transaction id and the station.
/// Blocks this crate has no use for are skipped.
///
/// # Errors
/// Not an Identify success, or a block of the wrong size.
pub fn parse_identify_response(payload: &[u8]) -> Result<(u32, Station), DecodeError> {
    let (header, blocks) = parse(payload)?;
    if header.service != SERVICE_IDENTIFY || header.service_type != TYPE_SUCCESS {
        return Err(DecodeError::Invalid("not an Identify response"));
    }
    let mut station = Station::default();
    for b in blocks {
        // Every response block starts with its 16-bit block info.
        let mut r = Reader::new(b.value);
        r.u16()?;
        match (b.option, b.sub) {
            (OPT_IP, SUB_IP_PARAMETER) => {
                station.ip = r.array()?;
                station.mask = r.array()?;
                station.gateway = r.array()?;
            }
            (OPT_DEVICE, SUB_DEVICE_ID) => {
                station.vendor_id = r.u16()?;
                station.device_id = r.u16()?;
            }
            (OPT_DEVICE, SUB_DEVICE_TYPE) => station.type_of_station = text(r.rest())?,
            (OPT_DEVICE, SUB_DEVICE_NAME) => station.name = text(r.rest())?,
            (OPT_DEVICE, SUB_DEVICE_ROLE) => station.role = r.u8()?,
            (OPT_DEVICE, SUB_DEVICE_INSTANCE) => station.instance = Some(r.u16()?),
            _ => {}
        }
    }
    Ok((header.xid, station))
}

/// What a Set changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Set {
    Ip { ip: Ip, mask: Ip, gateway: Ip },
    Name(String),
}

impl Set {
    const fn option(&self) -> (u8, u8) {
        match self {
            Self::Ip { .. } => (OPT_IP, SUB_IP_PARAMETER),
            Self::Name(_) => (OPT_DEVICE, SUB_DEVICE_NAME),
        }
    }
}

/// A Set request, sent unicast to the station with `FrameID`
/// [`crate::eth::FRAME_ID_DCP_GET_SET`].
///
/// # Errors
/// A name longer than [`MAX_NAME`].
pub fn set_request(xid: u32, set: &Set, permanent: bool) -> Result<Vec<u8>, DecodeError> {
    let mut p = Pdu::new(SERVICE_SET, TYPE_REQUEST, xid, 0);
    let qualifier = if permanent {
        QUALIFIER_PERMANENT
    } else {
        QUALIFIER_TEMPORARY
    }
    .to_be_bytes();
    let (option, sub) = set.option();
    match set {
        Set::Ip { ip, mask, gateway } => {
            p.block(option, sub, &qualifier, &[*ip, *mask, *gateway].concat());
        }
        Set::Name(name) => {
            if name.len() > MAX_NAME {
                return Err(DecodeError::Invalid("station name longer than 240"));
            }
            p.block(option, sub, &qualifier, name.as_bytes());
        }
    }
    Ok(p.finish())
}

/// A Set response: the option the request set and its block error
/// ([`ERROR_NONE`] for success).
#[must_use]
pub fn set_response(xid: u32, option: u8, sub: u8, error: u8) -> Vec<u8> {
    let mut p = Pdu::new(SERVICE_SET, TYPE_SUCCESS, xid, 0);
    p.block(OPT_CONTROL, SUB_CONTROL_RESPONSE, &[option, sub, error], &[]);
    p.finish()
}

/// Reads a Set response: its transaction id and the first block error
/// that is not [`ERROR_NONE`], else [`ERROR_NONE`].
///
/// # Errors
/// Not a Set response, or one without a response block.
pub fn parse_set_response(payload: &[u8]) -> Result<(u32, u8), DecodeError> {
    let (header, blocks) = parse(payload)?;
    if header.service != SERVICE_SET || header.service_type == TYPE_REQUEST {
        return Err(DecodeError::Invalid("not a Set response"));
    }
    if header.service_type != TYPE_SUCCESS {
        return Ok((header.xid, ERROR_OPTION_UNSUPPORTED));
    }
    let mut seen = false;
    for b in blocks {
        if (b.option, b.sub) == (OPT_CONTROL, SUB_CONTROL_RESPONSE) {
            let mut r = Reader::new(b.value);
            r.u16()?;
            let error = r.u8()?;
            seen = true;
            if error != ERROR_NONE {
                return Ok((header.xid, error));
            }
        }
    }
    if seen {
        Ok((header.xid, ERROR_NONE))
    } else {
        Err(DecodeError::Invalid("Set response without a response block"))
    }
}

/// A request as a device sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Identify. `name` is the station name asked for; `None` asks every
    /// station.
    Identify {
        xid: u32,
        name: Option<String>,
        response_delay: u16,
    },
    /// Identify with a filter this crate does not evaluate (an alias, a
    /// device id): a device that cannot tell whether it matches stays
    /// silent.
    IdentifyOther {
        xid: u32,
    },
    Set {
        xid: u32,
        set: Set,
        permanent: bool,
    },
    /// A Get, or a Set of something else: answered "not supported".
    Unsupported {
        service: u8,
        xid: u32,
    },
}

/// Reads a DCP request.
///
/// # Errors
/// Truncated or malformed, or not a request at all.
pub fn parse_request(payload: &[u8]) -> Result<Request, DecodeError> {
    let (header, blocks) = parse(payload)?;
    if header.service_type != TYPE_REQUEST {
        return Err(DecodeError::Invalid("not a DCP request"));
    }
    let xid = header.xid;
    match header.service {
        SERVICE_IDENTIFY => {
            let mut name = None;
            for b in &blocks {
                match (b.option, b.sub) {
                    (OPT_ALL, SUB_ALL) => {}
                    (OPT_DEVICE, SUB_DEVICE_NAME) => name = Some(text(b.value)?),
                    _ => return Ok(Request::IdentifyOther { xid }),
                }
            }
            Ok(Request::Identify {
                xid,
                name,
                response_delay: header.response_delay,
            })
        }
        SERVICE_SET => {
            for b in &blocks {
                let mut r = Reader::new(b.value);
                match (b.option, b.sub) {
                    (OPT_IP, SUB_IP_PARAMETER) => {
                        let permanent = r.u16()? & 1 == 1;
                        let set = Set::Ip {
                            ip: r.array()?,
                            mask: r.array()?,
                            gateway: r.array()?,
                        };
                        return Ok(Request::Set { xid, set, permanent });
                    }
                    (OPT_DEVICE, SUB_DEVICE_NAME) => {
                        let permanent = r.u16()? & 1 == 1;
                        let set = Set::Name(text(r.rest())?);
                        return Ok(Request::Set { xid, set, permanent });
                    }
                    _ => {}
                }
            }
            Ok(Request::Unsupported {
                service: header.service,
                xid,
            })
        }
        service => Ok(Request::Unsupported { service, xid }),
    }
}

/// The option and suboption a [`Set`] names, for the response.
#[must_use]
pub const fn set_option(set: &Set) -> (u8, u8) {
    set.option()
}

/// A "not supported" response to a request of `service`.
#[must_use]
pub fn unsupported_response(service: u8, xid: u32) -> Vec<u8> {
    Pdu::new(service, TYPE_NOT_SUPPORTED, xid, 0).finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station() -> Station {
        Station {
            name: "press-5.cell-2".into(),
            vendor_id: 0x0145,
            device_id: 0x0A01,
            role: ROLE_DEVICE,
            type_of_station: "sim".into(),
            ip: [192, 168, 3, 70],
            mask: [255, 255, 255, 0],
            gateway: [192, 168, 3, 1],
            instance: Some(1),
        }
    }

    #[test]
    fn an_identify_request_by_name_is_the_bytes_on_the_wire() {
        // Header: Identify (5), request (0), xid, delay 1, length 8;
        // block: device (2), name (2), length 3, "abc", one pad byte.
        let bytes = identify_request(0x0102_0304, Some("abc"), 1);
        assert_eq!(
            bytes,
            [5, 0, 1, 2, 3, 4, 0, 1, 0, 8, 2, 2, 0, 3, b'a', b'b', b'c', 0]
        );
        assert_eq!(
            parse_request(&bytes).unwrap(),
            Request::Identify {
                xid: 0x0102_0304,
                name: Some("abc".into()),
                response_delay: 1
            }
        );
    }

    #[test]
    fn an_identify_all_request_names_no_station() {
        let bytes = identify_request(7, None, 0x80);
        assert_eq!(bytes, [5, 0, 0, 0, 0, 7, 0, 0x80, 0, 4, 0xFF, 0xFF, 0, 0]);
        assert_eq!(
            parse_request(&bytes).unwrap(),
            Request::Identify {
                xid: 7,
                name: None,
                response_delay: 0x80
            }
        );
    }

    #[test]
    fn an_identify_response_reads_back() {
        let s = station();
        let bytes = identify_response(9, &s);
        assert_eq!(parse_identify_response(&bytes).unwrap(), (9, s));
        // With an Ethernet frame's padding after it too.
        let mut padded = bytes;
        padded.extend_from_slice(&[0; 9]);
        assert_eq!(parse_identify_response(&padded).unwrap().1, station());
    }

    #[test]
    fn a_set_reads_back_and_its_response_carries_the_error() {
        let set = Set::Ip {
            ip: [10, 0, 0, 2],
            mask: [255, 0, 0, 0],
            gateway: [0; 4],
        };
        let bytes = set_request(3, &set, false).unwrap();
        // Block: IP (1), parameter (2), length 14, qualifier 0, 12 bytes.
        assert_eq!(&bytes[10..16], &[1, 2, 0, 14, 0, 0]);
        assert_eq!(
            parse_request(&bytes).unwrap(),
            Request::Set {
                xid: 3,
                set: set.clone(),
                permanent: false
            }
        );
        let name = Set::Name("a-b".into());
        assert_eq!(
            parse_request(&set_request(4, &name, true).unwrap()).unwrap(),
            Request::Set {
                xid: 4,
                set: name,
                permanent: true
            }
        );
        let ok = set_response(3, OPT_IP, SUB_IP_PARAMETER, ERROR_NONE);
        // Control (5), response (4), length 3, option, suboption, error, pad.
        assert_eq!(&ok[10..], &[5, 4, 0, 3, 1, 2, 0, 0]);
        assert_eq!(parse_set_response(&ok).unwrap(), (3, ERROR_NONE));
        let refused = set_response(3, OPT_IP, SUB_IP_PARAMETER, ERROR_IN_OPERATION);
        assert_eq!(parse_set_response(&refused).unwrap(), (3, ERROR_IN_OPERATION));
    }

    #[test]
    fn station_names_follow_the_label_rules() {
        for good in ["a", "press-5", "press-5.cell-2", "0a", &"a".repeat(63)] {
            assert!(valid_station_name(good), "{good}");
        }
        for bad in [
            "",
            "Press",
            "a_b",
            "-a",
            "a-",
            "a..b",
            ".a",
            &"a".repeat(64),
            "a b",
        ] {
            assert!(!valid_station_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_block_past_the_data_length_is_refused() {
        let mut bytes = identify_request(1, Some("abcd"), 1);
        bytes[13] = 200;
        assert!(matches!(parse(&bytes), Err(DecodeError::Truncated { .. })));
        assert!(matches!(parse(&bytes[..5]), Err(DecodeError::Truncated { .. })));
    }
}
