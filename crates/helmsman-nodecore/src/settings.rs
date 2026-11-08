//! The node's settings, and the record they sit in on flash.
//!
//! One fixed [`LEN`]-byte frame, little-endian, checksummed, written to
//! one of two slots a sector apart (A at offset 0, B at [`SLOT_STRIDE`]).
//! A write goes to the slot *not* holding the current record with a higher
//! sequence number, so the previous record survives a power loss mid-write
//! and the next boot picks whichever valid slot is newer. The same shape
//! supervictor's edge firmware uses for its record (`settings/record.rs`
//! there, 2026-11); this one holds a node's fields.
//!
//! ```text
//!   0  magic "HMND"     4  version (1)    6  net mode    7  reserved
//!   8  seq             12  ip 4   16  mask 4   20  gateway 4
//!  24  log sink ip 4   28  log sink port 2   30  api port 2
//!  32  name  len + 32  65  site  len + 32  98  token  len + 64
//! 163  reserved      252  checksum (FNV-1a over 0..252)
//! ```
//!
//! Pure functions over byte slices; the firmware does the flash I/O.

use alloc::string::String;
use core::fmt;

/// Bytes in a record.
pub const LEN: usize = 256;
/// `"HMND"` little-endian: marks a record this firmware wrote.
pub const MAGIC: u32 = u32::from_le_bytes(*b"HMND");
/// Layout version this firmware reads and writes.
pub const VERSION: u16 = 1;
/// Distance between the two slots: one flash sector, so rewriting one
/// never erases the other.
pub const SLOT_STRIDE: u32 = 4096;
/// Flash the two slots need.
pub const SPAN: u32 = 2 * SLOT_STRIDE;

pub const NAME_MAX: usize = 32;
pub const SITE_MAX: usize = 32;
pub const TOKEN_MAX: usize = 64;
/// The API port a fresh node listens on, the box's default.
pub const DEFAULT_API_PORT: u16 = 8080;

const OFF_VERSION: usize = 4;
const OFF_MODE: usize = 6;
const OFF_SEQ: usize = 8;
const OFF_IP: usize = 12;
const OFF_MASK: usize = 16;
const OFF_GATEWAY: usize = 20;
const OFF_SINK_IP: usize = 24;
const OFF_SINK_PORT: usize = 28;
const OFF_API_PORT: usize = 30;
const OFF_NAME: usize = 32;
const OFF_SITE: usize = OFF_NAME + 1 + NAME_MAX;
const OFF_TOKEN: usize = OFF_SITE + 1 + SITE_MAX;
const OFF_CHECKSUM: usize = LEN - 4;
const _: () = assert!(OFF_TOKEN + 1 + TOKEN_MAX <= OFF_CHECKSUM);

/// How the node gets its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Net {
    Dhcp,
    Static {
        ip: [u8; 4],
        /// Prefix length, 0 to 32.
        prefix: u8,
        gateway: [u8; 4],
    },
}

/// What the node is told about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub net: Net,
    /// The node's name on the floor (the beacon's `name`).
    pub name: String,
    /// The site it announces itself in; empty means no beacon.
    pub site: String,
    /// What `Authorization: Bearer` must carry to change the node; empty
    /// means every write is refused.
    pub token: String,
    /// Where report lines go over UDP, if anywhere.
    pub log_sink: Option<([u8; 4], u16)>,
    pub api_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            net: Net::Dhcp,
            name: String::new(),
            site: String::new(),
            token: String::new(),
            log_sink: None,
            api_port: DEFAULT_API_PORT,
        }
    }
}

/// Why a settings value was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
    /// Longer than the record holds.
    TooLong(&'static str, usize),
    /// A character outside the field's alphabet.
    BadChar(&'static str),
    /// A prefix length past 32.
    Prefix(u8),
    /// A port of 0.
    Port(&'static str),
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong(what, max) => write!(f, "{what}: longer than {max} bytes"),
            Self::BadChar(what) => write!(f, "{what}: letters, digits, '-', '_' and '.' only"),
            Self::Prefix(p) => write!(f, "prefix length {p} is past 32"),
            Self::Port(what) => write!(f, "{what}: port 0"),
        }
    }
}

impl core::error::Error for Invalid {}

fn plain(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl Settings {
    /// Checks every field against what the record holds and what the
    /// beacon's alphabet allows (`crates/helmsman-plc/src/site.rs`:
    /// letters, digits, `-`, `_`, `.`).
    ///
    /// # Errors
    /// The first field that does not fit.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.name.len() > NAME_MAX {
            return Err(Invalid::TooLong("name", NAME_MAX));
        }
        if !plain(&self.name) {
            return Err(Invalid::BadChar("name"));
        }
        if self.site.len() > SITE_MAX {
            return Err(Invalid::TooLong("site", SITE_MAX));
        }
        if !plain(&self.site) {
            return Err(Invalid::BadChar("site"));
        }
        if self.token.len() > TOKEN_MAX {
            return Err(Invalid::TooLong("token", TOKEN_MAX));
        }
        if !self.token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Invalid::BadChar("token"));
        }
        if let Net::Static { prefix, .. } = self.net
            && prefix > 32
        {
            return Err(Invalid::Prefix(prefix));
        }
        if self.api_port == 0 {
            return Err(Invalid::Port("api port"));
        }
        if matches!(self.log_sink, Some((_, 0))) {
            return Err(Invalid::Port("log sink"));
        }
        Ok(())
    }

    /// The netmask of a static address.
    #[must_use]
    pub fn mask(prefix: u8) -> [u8; 4] {
        let bits = u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0);
        if prefix == 0 { [0; 4] } else { bits.to_be_bytes() }
    }
}

/// Which of the two slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    /// Byte offset of the slot inside the partition.
    #[must_use]
    pub const fn offset(self) -> u32 {
        match self {
            Self::A => 0,
            Self::B => SLOT_STRIDE,
        }
    }

    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

/// A valid record read from flash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub slot: Slot,
    pub seq: u32,
    pub settings: Settings,
}

/// FNV-1a over `bytes`.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    })
}

fn put_str(frame: &mut [u8], off: usize, s: &str, max: usize) {
    let n = s.len().min(max);
    frame[off] = u8::try_from(n).unwrap_or(u8::MAX);
    frame[off + 1..off + 1 + n].copy_from_slice(&s.as_bytes()[..n]);
}

fn get_str(frame: &[u8], off: usize, max: usize) -> Option<String> {
    let n = usize::from(frame[off]);
    if n > max {
        return None;
    }
    core::str::from_utf8(&frame[off + 1..off + 1 + n])
        .ok()
        .map(String::from)
}

fn four(frame: &[u8], off: usize) -> [u8; 4] {
    [frame[off], frame[off + 1], frame[off + 2], frame[off + 3]]
}

/// Writes `settings` as record `seq` into `frame`.
///
/// # Panics
/// If `frame` is shorter than [`LEN`].
pub fn encode(settings: &Settings, seq: u32, frame: &mut [u8]) {
    assert!(frame.len() >= LEN, "a settings frame is {LEN} bytes");
    let frame = &mut frame[..LEN];
    frame.fill(0);
    frame[..4].copy_from_slice(&MAGIC.to_le_bytes());
    frame[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());
    frame[OFF_SEQ..OFF_SEQ + 4].copy_from_slice(&seq.to_le_bytes());
    match settings.net {
        Net::Dhcp => frame[OFF_MODE] = 0,
        Net::Static { ip, prefix, gateway } => {
            frame[OFF_MODE] = 1;
            frame[OFF_IP..OFF_IP + 4].copy_from_slice(&ip);
            frame[OFF_MASK] = prefix;
            frame[OFF_GATEWAY..OFF_GATEWAY + 4].copy_from_slice(&gateway);
        }
    }
    if let Some((ip, port)) = settings.log_sink {
        frame[OFF_SINK_IP..OFF_SINK_IP + 4].copy_from_slice(&ip);
        frame[OFF_SINK_PORT..OFF_SINK_PORT + 2].copy_from_slice(&port.to_le_bytes());
    }
    frame[OFF_API_PORT..OFF_API_PORT + 2].copy_from_slice(&settings.api_port.to_le_bytes());
    put_str(frame, OFF_NAME, &settings.name, NAME_MAX);
    put_str(frame, OFF_SITE, &settings.site, SITE_MAX);
    put_str(frame, OFF_TOKEN, &settings.token, TOKEN_MAX);
    let sum = checksum(&frame[..OFF_CHECKSUM]);
    frame[OFF_CHECKSUM..].copy_from_slice(&sum.to_le_bytes());
}

/// Reads a record: `None` for anything but a frame this firmware wrote,
/// whole, with its checksum.
#[must_use]
pub fn decode(frame: &[u8]) -> Option<(u32, Settings)> {
    if frame.len() < LEN {
        return None;
    }
    let frame = &frame[..LEN];
    if u32::from_le_bytes(four(frame, 0)) != MAGIC {
        return None;
    }
    if u16::from_le_bytes([frame[OFF_VERSION], frame[OFF_VERSION + 1]]) != VERSION {
        return None;
    }
    let sum = u32::from_le_bytes(four(frame, OFF_CHECKSUM));
    if checksum(&frame[..OFF_CHECKSUM]) != sum {
        return None;
    }
    let seq = u32::from_le_bytes(four(frame, OFF_SEQ));
    let net = match frame[OFF_MODE] {
        0 => Net::Dhcp,
        1 => Net::Static {
            ip: four(frame, OFF_IP),
            prefix: frame[OFF_MASK],
            gateway: four(frame, OFF_GATEWAY),
        },
        _ => return None,
    };
    let sink_port = u16::from_le_bytes([frame[OFF_SINK_PORT], frame[OFF_SINK_PORT + 1]]);
    let log_sink = (sink_port != 0).then(|| (four(frame, OFF_SINK_IP), sink_port));
    let api_port = u16::from_le_bytes([frame[OFF_API_PORT], frame[OFF_API_PORT + 1]]);
    let settings = Settings {
        net,
        name: get_str(frame, OFF_NAME, NAME_MAX)?,
        site: get_str(frame, OFF_SITE, SITE_MAX)?,
        token: get_str(frame, OFF_TOKEN, TOKEN_MAX)?,
        log_sink,
        api_port,
    };
    settings.validate().ok()?;
    Some((seq, settings))
}

/// The newer of the two slots' valid records, if either holds one.
#[must_use]
pub fn choose(a: &[u8], b: &[u8]) -> Option<Found> {
    let a = decode(a).map(|(seq, settings)| Found {
        slot: Slot::A,
        seq,
        settings,
    });
    let b = decode(b).map(|(seq, settings)| Found {
        slot: Slot::B,
        seq,
        settings,
    });
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.seq > a.seq { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Where the next record goes and with which sequence number.
#[must_use]
pub fn next(current: Option<&Found>) -> (Slot, u32) {
    match current {
        Some(f) => (f.slot.other(), f.seq.wrapping_add(1).max(1)),
        None => (Slot::A, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use proptest::prelude::*;

    fn sample() -> Settings {
        Settings {
            net: Net::Static {
                ip: [10, 0, 0, 50],
                prefix: 24,
                gateway: [10, 0, 0, 1],
            },
            name: "press-4".into(),
            site: "line-a".into(),
            token: "s3cret-token.0123456789".into(),
            log_sink: Some(([10, 0, 0, 241], 9514)),
            api_port: 8080,
        }
    }

    #[test]
    fn round_trips() {
        let mut f = [0u8; LEN];
        encode(&sample(), 7, &mut f);
        assert_eq!(decode(&f), Some((7, sample())));
        let mut g = [0u8; LEN];
        encode(&Settings::default(), 1, &mut g);
        assert_eq!(decode(&g), Some((1, Settings::default())));
    }

    #[test]
    fn every_single_byte_corruption_is_refused() {
        let mut f = [0u8; LEN];
        encode(&sample(), 3, &mut f);
        for i in 0..LEN {
            let mut c = f;
            c[i] ^= 0x5A;
            assert!(decode(&c).is_none(), "byte {i} flipped and the record still read");
        }
    }

    #[test]
    fn blank_flash_and_short_frames_are_nothing() {
        assert!(decode(&[0xFF; LEN]).is_none());
        assert!(decode(&[0; LEN]).is_none());
        assert!(decode(&[0; LEN - 1]).is_none());
        assert!(choose(&[0xFF; LEN], &[0xFF; LEN]).is_none());
    }

    #[test]
    fn the_newer_slot_wins_and_the_next_write_goes_to_the_other() {
        let mut a = [0u8; LEN];
        let mut b = [0u8; LEN];
        encode(&sample(), 4, &mut a);
        let mut newer = sample();
        newer.name = "press-5".into();
        encode(&newer, 5, &mut b);
        let found = choose(&a, &b).unwrap();
        assert_eq!(found.slot, Slot::B);
        assert_eq!(found.settings.name, "press-5");
        assert_eq!(next(Some(&found)), (Slot::A, 6));
        assert_eq!(next(None), (Slot::A, 1));
        // A torn write of slot A keeps B.
        a[100] ^= 1;
        assert_eq!(choose(&a, &b).unwrap().slot, Slot::B);
    }

    #[test]
    fn validation_catches_what_the_record_cannot_hold() {
        let mut s = sample();
        s.name = "x".repeat(NAME_MAX + 1);
        assert_eq!(s.validate(), Err(Invalid::TooLong("name", NAME_MAX)));
        s.name = "has space".into();
        assert_eq!(s.validate(), Err(Invalid::BadChar("name")));
        s.name = "ok".into();
        s.net = Net::Static {
            ip: [1, 2, 3, 4],
            prefix: 33,
            gateway: [0; 4],
        };
        assert_eq!(s.validate(), Err(Invalid::Prefix(33)));
        s.net = Net::Dhcp;
        s.api_port = 0;
        assert_eq!(s.validate(), Err(Invalid::Port("api port")));
        s.api_port = 8080;
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn masks() {
        assert_eq!(Settings::mask(24), [255, 255, 255, 0]);
        assert_eq!(Settings::mask(32), [255; 4]);
        assert_eq!(Settings::mask(0), [0; 4]);
        assert_eq!(Settings::mask(20), [255, 255, 240, 0]);
    }

    proptest! {
        #[test]
        fn any_frame_decodes_without_panicking(frame in proptest::collection::vec(any::<u8>(), 0..LEN + 8)) {
            let _ = decode(&frame);
            let _ = choose(&frame, &frame);
        }

        #[test]
        fn plain_fields_round_trip(
            name in "[A-Za-z0-9._-]{0,32}",
            site in "[A-Za-z0-9._-]{0,32}",
            token in "[!-~]{0,64}",
            seq in any::<u32>(),
        ) {
            let s = Settings { name, site, token, ..Settings::default() };
            let mut f = vec![0u8; LEN];
            encode(&s, seq, &mut f);
            prop_assert_eq!(decode(&f), Some((seq, s)));
        }
    }
}
