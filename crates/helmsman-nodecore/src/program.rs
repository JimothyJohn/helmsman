//! The deployed program as it sits on flash: its ST source and name in one
//! of two [`SLOT_LEN`]-byte slots, a sector apart, with the same two-slot,
//! sequence-numbered, checksummed scheme as [`crate::settings`], so a
//! power loss during a deploy keeps the program that ran before. The
//! node compiles the source again at boot: what runs is always what the
//! engineer deployed, byte for byte.
//!
//! ```text
//!   0  magic "HMPG"   4  version (1)   6  reserved   8  seq
//!  12  name len + 32  45  source len (u16)  47  reserved  48  checksum
//!  52  source bytes
//! ```
//!
//! The nvs partition espflash lays out by default is 24 KiB: settings in
//! its first two sectors ([`crate::settings::SPAN`]), the program in the
//! next four ([`BASE`] on), 8 KiB a slot. A chase-sized program is under
//! 1 KiB; the WROOM's heap, not the slot, bounds a real one.

use alloc::string::String;

use crate::settings::{NAME_MAX, Slot, checksum};

/// Bytes in a slot: two flash sectors.
pub const SLOT_LEN: usize = 8192;
/// Where the program slots start inside the partition: past the settings.
pub const BASE: u32 = crate::settings::SPAN;
/// Distance between the two program slots.
pub const SLOT_STRIDE: u32 = 8192;
const _: () = assert!(SLOT_STRIDE as usize == SLOT_LEN);
/// Flash the settings and the program need together.
pub const SPAN: u32 = BASE + 2 * SLOT_STRIDE;
/// `"HMPG"` little-endian.
pub const MAGIC: u32 = u32::from_le_bytes(*b"HMPG");
pub const VERSION: u16 = 1;
const HEADER: usize = 52;
/// Most source bytes a slot holds.
pub const MAX_SOURCE: usize = SLOT_LEN - HEADER;

const OFF_VERSION: usize = 4;
const OFF_SEQ: usize = 8;
const OFF_NAME: usize = 12;
const OFF_LEN: usize = OFF_NAME + 1 + NAME_MAX;
const OFF_CHECKSUM: usize = 48;
const _: () = assert!(OFF_LEN + 2 <= OFF_CHECKSUM);
const _: () = assert!(OFF_CHECKSUM + 4 == HEADER);

/// Byte offset of a program slot inside the partition.
#[must_use]
pub const fn offset(slot: Slot) -> u32 {
    match slot {
        Slot::A => BASE,
        Slot::B => BASE + SLOT_STRIDE,
    }
}

/// A program read from flash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub slot: Slot,
    pub seq: u32,
    pub name: String,
    pub source: String,
}

/// Why a program cannot be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooBig {
    Source(usize),
    Name(usize),
}

impl core::fmt::Display for TooBig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Source(n) => write!(f, "program is {n} bytes; a slot holds {MAX_SOURCE}"),
            Self::Name(n) => write!(f, "name is {n} bytes; at most {NAME_MAX}"),
        }
    }
}

/// Writes `name` and `source` as record `seq` into `frame`; the bytes past
/// the source are left as they are (a slot is erased to 0xFF).
///
/// # Errors
/// A source or name longer than the slot holds.
///
/// # Panics
/// If `frame` is shorter than [`SLOT_LEN`].
pub fn encode(name: &str, source: &str, seq: u32, frame: &mut [u8]) -> Result<usize, TooBig> {
    assert!(frame.len() >= SLOT_LEN, "a program slot is {SLOT_LEN} bytes");
    if source.len() > MAX_SOURCE {
        return Err(TooBig::Source(source.len()));
    }
    if name.len() > NAME_MAX {
        return Err(TooBig::Name(name.len()));
    }
    let total = HEADER + source.len();
    let frame = &mut frame[..total];
    frame[..HEADER].fill(0);
    frame[..4].copy_from_slice(&MAGIC.to_le_bytes());
    frame[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&VERSION.to_le_bytes());
    frame[OFF_SEQ..OFF_SEQ + 4].copy_from_slice(&seq.to_le_bytes());
    frame[OFF_NAME] = u8::try_from(name.len()).unwrap_or(u8::MAX);
    frame[OFF_NAME + 1..OFF_NAME + 1 + name.len()].copy_from_slice(name.as_bytes());
    let len = u16::try_from(source.len()).unwrap_or(u16::MAX);
    frame[OFF_LEN..OFF_LEN + 2].copy_from_slice(&len.to_le_bytes());
    frame[HEADER..].copy_from_slice(source.as_bytes());
    // The checksum covers the header (less itself) and the source.
    let sum = checksum(&frame[..OFF_CHECKSUM]) ^ checksum(&frame[HEADER..]);
    frame[OFF_CHECKSUM..HEADER].copy_from_slice(&sum.to_le_bytes());
    Ok(total)
}

/// Reads a slot: `None` for anything but a frame this firmware wrote,
/// whole, with its checksum.
#[must_use]
pub fn decode(frame: &[u8]) -> Option<(u32, String, String)> {
    if frame.len() < HEADER {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([frame[o], frame[o + 1], frame[o + 2], frame[o + 3]]);
    if u32_at(0) != MAGIC || u16::from_le_bytes([frame[OFF_VERSION], frame[OFF_VERSION + 1]]) != VERSION {
        return None;
    }
    let len = usize::from(u16::from_le_bytes([frame[OFF_LEN], frame[OFF_LEN + 1]]));
    if len > MAX_SOURCE || frame.len() < HEADER + len {
        return None;
    }
    let sum = checksum(&frame[..OFF_CHECKSUM]) ^ checksum(&frame[HEADER..HEADER + len]);
    if sum != u32_at(OFF_CHECKSUM) {
        return None;
    }
    let name_len = usize::from(frame[OFF_NAME]);
    if name_len > NAME_MAX {
        return None;
    }
    let name = core::str::from_utf8(&frame[OFF_NAME + 1..OFF_NAME + 1 + name_len]).ok()?;
    let source = core::str::from_utf8(&frame[HEADER..HEADER + len]).ok()?;
    Some((u32_at(OFF_SEQ), String::from(name), String::from(source)))
}

/// The newer of the two slots' valid programs, if either holds one.
#[must_use]
pub fn choose(a: &[u8], b: &[u8]) -> Option<Stored> {
    let read = |slot: Slot, bytes: &[u8]| {
        decode(bytes).map(|(seq, name, source)| Stored {
            slot,
            seq,
            name,
            source,
        })
    };
    match (read(Slot::A, a), read(Slot::B, b)) {
        (Some(a), Some(b)) => Some(if b.seq > a.seq { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Where the next program goes and with which sequence number.
#[must_use]
pub fn next(current: Option<&Stored>) -> (Slot, u32) {
    match current {
        Some(s) => (s.slot.other(), s.seq.wrapping_add(1).max(1)),
        None => (Slot::A, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use proptest::prelude::*;

    const SRC: &str = "PROGRAM chase\nVAR x : BOOL; END_VAR\nx := NOT x;\nEND_PROGRAM\n";

    #[test]
    fn round_trips_in_an_erased_slot() {
        let mut slot = vec![0xFFu8; SLOT_LEN];
        let n = encode("chase", SRC, 3, &mut slot).unwrap();
        assert_eq!(n, HEADER + SRC.len());
        assert_eq!(decode(&slot), Some((3, "chase".into(), SRC.into())));
        assert!(
            slot[n..].iter().all(|b| *b == 0xFF),
            "bytes past the source are untouched"
        );
    }

    #[test]
    fn corruption_anywhere_in_the_record_is_refused() {
        let mut slot = vec![0xFFu8; SLOT_LEN];
        let n = encode("chase", SRC, 1, &mut slot).unwrap();
        for i in 0..n {
            let mut c = slot.clone();
            c[i] ^= 0x01;
            assert!(
                decode(&c).is_none(),
                "byte {i} flipped and the program still read"
            );
        }
        // A flip past the record does not matter.
        let mut c = slot.clone();
        c[n + 10] ^= 0x01;
        assert!(decode(&c).is_some());
    }

    #[test]
    fn bounds() {
        let mut slot = vec![0xFFu8; SLOT_LEN];
        let big = "x".repeat(MAX_SOURCE + 1);
        assert_eq!(
            encode("p", &big, 1, &mut slot),
            Err(TooBig::Source(MAX_SOURCE + 1))
        );
        let exact = "y".repeat(MAX_SOURCE);
        assert_eq!(encode("p", &exact, 1, &mut slot), Ok(SLOT_LEN));
        assert_eq!(decode(&slot).map(|(_, _, s)| s.len()), Some(MAX_SOURCE));
        assert_eq!(
            encode(&"n".repeat(NAME_MAX + 1), SRC, 1, &mut slot),
            Err(TooBig::Name(NAME_MAX + 1))
        );
        assert!(decode(&[0xFF; SLOT_LEN]).is_none());
        assert!(decode(&[0; 10]).is_none());
    }

    #[test]
    fn the_newer_slot_wins() {
        let mut a = vec![0xFFu8; SLOT_LEN];
        let mut b = vec![0xFFu8; SLOT_LEN];
        encode("one", SRC, 4, &mut a).unwrap();
        encode("two", SRC, 5, &mut b).unwrap();
        let s = choose(&a, &b).unwrap();
        assert_eq!((s.slot, s.name.as_str()), (Slot::B, "two"));
        assert_eq!(next(Some(&s)), (Slot::A, 6));
        assert_eq!(next(None), (Slot::A, 1));
        assert_eq!(offset(Slot::A), 8192);
        assert_eq!(offset(Slot::B), 16384);
        assert_eq!(SPAN, 24576, "the default nvs partition is exactly this");
    }

    proptest! {
        #[test]
        fn any_slot_decodes_without_panicking(frame in proptest::collection::vec(any::<u8>(), 0..HEADER + 64)) {
            let _ = decode(&frame);
        }

        #[test]
        fn sources_round_trip(name in "[A-Za-z0-9_-]{1,32}", src in "[ -~\\n]{0,400}", seq in any::<u32>()) {
            let mut slot = vec![0xFFu8; SLOT_LEN];
            encode(&name, &src, seq, &mut slot).unwrap();
            prop_assert_eq!(decode(&slot), Some((seq, name, src)));
        }
    }
}
