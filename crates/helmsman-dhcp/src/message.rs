//! The BOOTP/DHCP message (RFC 2131 section 2, Table 1).
//!
//! ```text
//! op htype hlen hops | xid (4) | secs (2) flags (2) | ciaddr yiaddr siaddr giaddr (4 each)
//! chaddr (16) | sname (64) | file (128) | options: cookie 99.130.83.99, then code/len/data
//! ```
//!
//! The fixed part is 236 octets. A message without the magic cookie is a
//! plain BOOTP message without vendor extensions. Options are read from
//! the options field and, when option 52 (overload) says so, from `file`
//! and then `sname`; a code that appears more than once is one option
//! whose pieces are joined in that order (RFC 3396).

use alloc::vec::Vec;
use core::fmt;
use core::net::Ipv4Addr;

/// Length of the fixed part, up to and without the options.
pub const FIXED_LEN: usize = 236;
/// The magic cookie that opens the options field (RFC 2131 section 3).
pub const COOKIE: [u8; 4] = [99, 130, 83, 99];
/// The smallest message a BOOTP client must be sent (RFC 1542 2.1, from
/// RFC 951): replies are padded with zeros to this length.
pub const MIN_LEN: usize = 300;

/// `op`: a client's request.
pub const BOOTREQUEST: u8 = 1;
/// `op`: a server's reply.
pub const BOOTREPLY: u8 = 2;
/// `htype` for Ethernet (RFC 1700 "Hardware Type"), with `hlen` 6.
pub const HTYPE_ETHERNET: u8 = 1;
/// The BROADCAST flag: the most significant bit of `flags` (RFC 1542 2.2).
pub const FLAG_BROADCAST: u16 = 0x8000;

/// Option codes this crate reads or writes (RFC 2132).
pub mod opt {
    pub const PAD: u8 = 0;
    pub const SUBNET_MASK: u8 = 1;
    pub const ROUTER: u8 = 3;
    pub const REQUESTED_IP: u8 = 50;
    pub const LEASE_TIME: u8 = 51;
    pub const OVERLOAD: u8 = 52;
    pub const MESSAGE_TYPE: u8 = 53;
    pub const SERVER_ID: u8 = 54;
    pub const RENEWAL_TIME: u8 = 58;
    pub const REBINDING_TIME: u8 = 59;
    pub const END: u8 = 255;
}

/// Option 53, the DHCP message type (RFC 2132 9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Discover,
    Offer,
    Request,
    Decline,
    Ack,
    Nak,
    Release,
    Inform,
}

impl Kind {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Discover => 1,
            Self::Offer => 2,
            Self::Request => 3,
            Self::Decline => 4,
            Self::Ack => 5,
            Self::Nak => 6,
            Self::Release => 7,
            Self::Inform => 8,
        }
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Discover,
            2 => Self::Offer,
            3 => Self::Request,
            4 => Self::Decline,
            5 => Self::Ack,
            6 => Self::Nak,
            7 => Self::Release,
            8 => Self::Inform,
            _ => return None,
        })
    }
}

/// Why a datagram is not a BOOTP/DHCP message. Decoding never panics,
/// whatever the bytes (tests/hostile.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The datagram ended inside the fixed part or inside an option.
    Truncated {
        /// Bytes the field needed.
        needed: usize,
        /// Bytes that were left.
        available: usize,
    },
    /// A field carried a value this crate does not accept.
    Invalid(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, available } => {
                write!(f, "truncated: needed {needed} bytes, {available} available")
            }
            Self::Invalid(what) => write!(f, "invalid: {what}"),
        }
    }
}

impl core::error::Error for DecodeError {}

/// An Ethernet address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mac(pub [u8; 6]);

impl fmt::Display for Mac {
    /// `00-90-e8-7e-af-be`, the form `helmsman_io::resolve`'s `mac=` takes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl core::str::FromStr for Mac {
    type Err = DecodeError;

    /// Six hex bytes with `-` or `:` between them.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut out = [0u8; 6];
        let mut parts = s.split(['-', ':']);
        for byte in &mut out {
            let part = parts.next().ok_or(DecodeError::Invalid("a MAC has six bytes"))?;
            // from_str_radix alone takes a leading '+'.
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(DecodeError::Invalid("a MAC byte is two hex digits"));
            }
            *byte = u8::from_str_radix(part, 16)
                .map_err(|_| DecodeError::Invalid("a MAC byte is two hex digits"))?;
        }
        if parts.next().is_some() {
            return Err(DecodeError::Invalid("a MAC has six bytes"));
        }
        Ok(Self(out))
    }
}

/// One BOOTP/DHCP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub hops: u8,
    pub xid: u32,
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: Ipv4Addr,
    pub yiaddr: Ipv4Addr,
    pub siaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    pub chaddr: [u8; 16],
    pub sname: [u8; 64],
    pub file: [u8; 128],
    /// Whether the options field opened with the magic cookie. A BOOTP
    /// client without vendor extensions sends none; a reply to it then
    /// carries none either.
    pub cookie: bool,
    /// The options in the order they first appeared, each code once, its
    /// pieces joined (RFC 3396). Pad, end and overload are not kept.
    pub options: Vec<(u8, Vec<u8>)>,
}

impl Message {
    /// An empty message: every field zero, no options.
    #[must_use]
    pub fn empty(op: u8) -> Self {
        Self {
            op,
            htype: HTYPE_ETHERNET,
            hlen: 6,
            hops: 0,
            xid: 0,
            secs: 0,
            flags: 0,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: [0; 16],
            sname: [0; 64],
            file: [0; 128],
            cookie: true,
            options: Vec::new(),
        }
    }

    /// Decode a datagram.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the fixed part is short or an option runs past
    /// the end of its field.
    pub fn parse(buf: &[u8]) -> Result<Self, DecodeError> {
        if buf.len() < FIXED_LEN {
            return Err(DecodeError::Truncated {
                needed: FIXED_LEN,
                available: buf.len(),
            });
        }
        let ip = |at: usize| Ipv4Addr::new(buf[at], buf[at + 1], buf[at + 2], buf[at + 3]);
        let mut m = Self::empty(buf[0]);
        m.htype = buf[1];
        m.hlen = buf[2];
        m.hops = buf[3];
        m.xid = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        m.secs = u16::from_be_bytes([buf[8], buf[9]]);
        m.flags = u16::from_be_bytes([buf[10], buf[11]]);
        m.ciaddr = ip(12);
        m.yiaddr = ip(16);
        m.siaddr = ip(20);
        m.giaddr = ip(24);
        m.chaddr.copy_from_slice(&buf[28..44]);
        m.sname.copy_from_slice(&buf[44..108]);
        m.file.copy_from_slice(&buf[108..236]);
        let rest = &buf[FIXED_LEN..];
        m.cookie = rest.len() >= 4 && rest[..4] == COOKIE;
        if !m.cookie {
            return Ok(m);
        }
        let mut overload = 0u8;
        read_options(&rest[4..], &mut m.options, &mut overload)?;
        // Overload is read from the options field only; `file` and
        // `sname` cannot overload themselves.
        let mut ignored = 0u8;
        if overload & 1 != 0 {
            let file = m.file;
            read_options(&file, &mut m.options, &mut ignored)?;
        }
        if overload & 2 != 0 {
            let sname = m.sname;
            read_options(&sname, &mut m.options, &mut ignored)?;
        }
        Ok(m)
    }

    /// The bytes of this message: the fixed part, the cookie and options
    /// (when [`Message::cookie`]), the end option, zeros to [`MIN_LEN`].
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MIN_LEN + 64);
        out.extend_from_slice(&[self.op, self.htype, self.hlen, self.hops]);
        out.extend_from_slice(&self.xid.to_be_bytes());
        out.extend_from_slice(&self.secs.to_be_bytes());
        out.extend_from_slice(&self.flags.to_be_bytes());
        for a in [self.ciaddr, self.yiaddr, self.siaddr, self.giaddr] {
            out.extend_from_slice(&a.octets());
        }
        out.extend_from_slice(&self.chaddr);
        out.extend_from_slice(&self.sname);
        out.extend_from_slice(&self.file);
        if self.cookie {
            out.extend_from_slice(&COOKIE);
            for (code, data) in &self.options {
                // A value longer than 255 goes out in pieces (RFC 3396).
                // An empty one still needs its code and a zero length.
                let mut pieces = data.chunks(255).peekable();
                if pieces.peek().is_none() {
                    out.extend_from_slice(&[*code, 0]);
                }
                for piece in pieces {
                    out.push(*code);
                    out.push(u8::try_from(piece.len()).unwrap_or(u8::MAX));
                    out.extend_from_slice(piece);
                }
            }
            out.push(opt::END);
        }
        if out.len() < MIN_LEN {
            out.resize(MIN_LEN, 0);
        }
        out
    }

    /// The client's Ethernet address, when `htype`/`hlen` say Ethernet.
    #[must_use]
    pub fn mac(&self) -> Option<Mac> {
        if self.htype != HTYPE_ETHERNET || self.hlen != 6 {
            return None;
        }
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&self.chaddr[..6]);
        Some(Mac(mac))
    }

    /// The value of option `code`, if present.
    #[must_use]
    pub fn option(&self, code: u8) -> Option<&[u8]> {
        self.options
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, d)| d.as_slice())
    }

    /// Option 53, when present and known. `None` is a BOOTP message.
    #[must_use]
    pub fn kind(&self) -> Option<Kind> {
        match self.option(opt::MESSAGE_TYPE) {
            Some([code]) => Kind::from_code(*code),
            _ => None,
        }
    }

    /// Whether option 53 is present at all (a known code or not).
    #[must_use]
    pub fn is_dhcp(&self) -> bool {
        self.option(opt::MESSAGE_TYPE).is_some()
    }

    /// An option holding exactly one address (50, 54, 1, ...).
    #[must_use]
    pub fn option_ip(&self, code: u8) -> Option<Ipv4Addr> {
        match self.option(code) {
            Some(&[a, b, c, d]) => Some(Ipv4Addr::new(a, b, c, d)),
            _ => None,
        }
    }

    /// Set option `code` to `data`, replacing an earlier value.
    pub fn set_option(&mut self, code: u8, data: &[u8]) {
        if let Some(slot) = self.options.iter_mut().find(|(c, _)| *c == code) {
            slot.1 = data.to_vec();
        } else {
            self.options.push((code, data.to_vec()));
        }
    }
}

/// Read code/len/data triples from `field` into `into`, joining repeated
/// codes. Stops at the end option or the end of the field.
fn read_options(field: &[u8], into: &mut Vec<(u8, Vec<u8>)>, overload: &mut u8) -> Result<(), DecodeError> {
    let mut i = 0;
    while i < field.len() {
        let code = field[i];
        i += 1;
        match code {
            opt::PAD => {}
            opt::END => return Ok(()),
            _ => {
                let Some(&len) = field.get(i) else {
                    return Err(DecodeError::Truncated {
                        needed: 1,
                        available: 0,
                    });
                };
                i += 1;
                let len = usize::from(len);
                let Some(data) = field.get(i..i + len) else {
                    return Err(DecodeError::Truncated {
                        needed: len,
                        available: field.len() - i,
                    });
                };
                i += len;
                if code == opt::OVERLOAD {
                    if let [v] = data {
                        *overload = *v & 3;
                    }
                    continue;
                }
                match into.iter_mut().find(|(c, _)| *c == code) {
                    Some(slot) => slot.1.extend_from_slice(data),
                    None => into.push((code, data.to_vec())),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_text_round_trips_and_refuses_junk() {
        let mac: Mac = "00:90:E8:7e-af-be".parse().unwrap();
        assert_eq!(mac.0, [0x00, 0x90, 0xe8, 0x7e, 0xaf, 0xbe]);
        assert_eq!(mac.to_string(), "00-90-e8-7e-af-be");
        for bad in [
            "",
            "00-90-e8-7e-af",
            "00-90-e8-7e-af-be-01",
            "0-90-e8-7e-af-be",
            "zz-90-e8-7e-af-be",
            "+1-90-e8-7e-af-be",
        ] {
            assert!(bad.parse::<Mac>().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_reply_is_at_least_300_octets_and_parses_back() {
        let mut m = Message::empty(BOOTREPLY);
        m.xid = 0xdead_beef;
        m.yiaddr = Ipv4Addr::new(192, 168, 3, 100);
        m.set_option(opt::MESSAGE_TYPE, &[Kind::Offer.code()]);
        m.set_option(opt::SERVER_ID, &[192, 168, 3, 1]);
        let bytes = m.encode();
        assert_eq!(bytes.len(), MIN_LEN);
        assert_eq!(Message::parse(&bytes).unwrap(), m);
    }

    #[test]
    fn a_long_option_goes_out_in_pieces_and_comes_back_whole() {
        let mut m = Message::empty(BOOTREQUEST);
        let long: Vec<u8> = (0..600u16).map(|i| i.to_le_bytes()[0]).collect();
        m.set_option(77, &long);
        m.set_option(12, &[]);
        let back = Message::parse(&m.encode()).unwrap();
        assert_eq!(back.option(77), Some(long.as_slice()));
        assert_eq!(back.option(12), Some(&[][..]));
    }

    #[test]
    fn overload_reads_file_then_sname() {
        let mut bytes = Message::empty(BOOTREQUEST).encode();
        bytes.truncate(FIXED_LEN);
        bytes.extend_from_slice(&COOKIE);
        bytes.extend_from_slice(&[opt::OVERLOAD, 1, 3, 77, 1, 0xaa, opt::END]);
        // file: option 53 and a piece of 77; sname: the rest of 77.
        bytes[108..115].copy_from_slice(&[opt::MESSAGE_TYPE, 1, 1, 77, 1, 0xbb, opt::END]);
        bytes[44..48].copy_from_slice(&[77, 1, 0xcc, opt::END]);
        let m = Message::parse(&bytes).unwrap();
        assert_eq!(m.kind(), Some(Kind::Discover));
        assert_eq!(m.option(77), Some(&[0xaa, 0xbb, 0xcc][..]));
    }

    #[test]
    fn bootp_without_a_cookie_has_no_options_and_none_go_back() {
        let mut m = Message::empty(BOOTREQUEST);
        m.cookie = false;
        let bytes = m.encode();
        assert_eq!(bytes.len(), MIN_LEN);
        assert!(bytes[FIXED_LEN..].iter().all(|b| *b == 0));
        let back = Message::parse(&bytes).unwrap();
        assert!(!back.cookie);
        assert!(!back.is_dhcp());
    }

    #[test]
    fn short_and_overrunning_input_is_an_error() {
        assert!(matches!(
            Message::parse(&[0; 235]),
            Err(DecodeError::Truncated { .. })
        ));
        let mut bytes = Message::empty(BOOTREQUEST).encode();
        bytes.truncate(FIXED_LEN);
        bytes.extend_from_slice(&COOKIE);
        bytes.extend_from_slice(&[opt::MESSAGE_TYPE, 9, 1]);
        assert!(matches!(
            Message::parse(&bytes),
            Err(DecodeError::Truncated { .. })
        ));
        bytes.truncate(FIXED_LEN + 4);
        bytes.push(opt::MESSAGE_TYPE);
        assert!(matches!(
            Message::parse(&bytes),
            Err(DecodeError::Truncated { .. })
        ));
    }
}
