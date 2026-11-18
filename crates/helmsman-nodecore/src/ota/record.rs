//! The trial record: whether the running image is still on trial, and how
//! many times it has booted without confirming.
//!
//! It lives in its own two-sector flash partition (`bootrec`), written the
//! way the settings are (settings.rs): a 32-byte checksummed frame with a
//! sequence number, alternating between the two sectors, so a power cut in
//! the middle of a write leaves the previous record intact and the next
//! boot reads whichever valid one is newer. Anything that does not check
//! out reads as "no trial": reverting a slot on the strength of garbage
//! would be worse than not reverting.
//!
//! ```text
//!  0 magic "HMBR"   4 version (1)   6 kind (0 idle, 1 on trial)
//!  7 target slot    8 seq            12 previous slot
//! 13 boots         14 reserved      28 checksum (FNV-1a over 0..28)
//! ```

use super::install::{SECTOR_U32, Slot};

/// Bytes in a frame.
pub const LEN: usize = 32;
/// `"HMBR"` little-endian.
pub const MAGIC: u32 = u32::from_le_bytes(*b"HMBR");
const VERSION: u16 = 1;
/// Flash the record needs: one sector per copy.
pub const SPAN: u32 = 2 * SECTOR_U32;

/// Which of the two sectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copy {
    A,
    B,
}

impl Copy {
    #[must_use]
    pub const fn offset(self) -> u32 {
        match self {
            Self::A => 0,
            Self::B => SECTOR_U32,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

/// What the record says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trial {
    /// Nothing is on trial.
    None,
    /// `target` was activated over `previous` and has booted `boots` times
    /// without confirming.
    On { target: u8, previous: u8, boots: u8 },
}

/// A valid record and where it sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub copy: Copy,
    pub seq: u32,
    pub trial: Trial,
}

fn checksum(frame: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in frame {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Frames a record.
#[must_use]
pub fn encode(trial: Trial, seq: u32) -> [u8; LEN] {
    let mut f = [0xFFu8; LEN];
    f[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    f[4..6].copy_from_slice(&VERSION.to_le_bytes());
    f[8..12].copy_from_slice(&seq.to_le_bytes());
    f[14..28].fill(0);
    match trial {
        Trial::None => {
            f[6] = 0;
            f[7] = 0;
            f[12] = 0;
            f[13] = 0;
        }
        Trial::On {
            target,
            previous,
            boots,
        } => {
            f[6] = 1;
            f[7] = target;
            f[12] = previous;
            f[13] = boots;
        }
    }
    let sum = checksum(&f[..28]);
    f[28..32].copy_from_slice(&sum.to_le_bytes());
    f
}

/// Reads a frame; `None` unless every part of it checks out. A target or
/// previous slot that is not 0 or 1, or the same slot twice, is refused.
#[must_use]
pub fn decode(frame: &[u8]) -> Option<(u32, Trial)> {
    if frame.len() < LEN {
        return None;
    }
    let u32_at = |o: usize| u32::from_le_bytes([frame[o], frame[o + 1], frame[o + 2], frame[o + 3]]);
    if u32_at(0) != MAGIC || u32_at(28) != checksum(&frame[..28]) {
        return None;
    }
    if u16::from_le_bytes([frame[4], frame[5]]) != VERSION {
        return None;
    }
    let seq = u32_at(8);
    match frame[6] {
        0 => Some((seq, Trial::None)),
        1 => {
            let (target, previous, boots) = (frame[7], frame[12], frame[13]);
            if target > 1 || previous > 1 || target == previous {
                return None;
            }
            Some((
                seq,
                Trial::On {
                    target,
                    previous,
                    boots,
                },
            ))
        }
        _ => None,
    }
}

/// Which valid record is newer; a sequence number counts as newer by
/// wrapping distance, so the choice survives 2^32 writes.
#[must_use]
pub fn choose(a: &[u8], b: &[u8]) -> Option<Found> {
    let a = decode(a).map(|(seq, trial)| Found {
        copy: Copy::A,
        seq,
        trial,
    });
    let b = decode(b).map(|(seq, trial)| Found {
        copy: Copy::B,
        seq,
        trial,
    });
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.seq.wrapping_sub(a.seq).cast_signed() > 0 {
            b
        } else {
            a
        }),
        (a, b) => a.or(b),
    }
}

/// Where the next record goes: the copy not holding the current one, with
/// the next sequence number.
#[must_use]
pub fn next(current: Option<&Found>) -> (Copy, u32) {
    match current {
        Some(f) => (f.copy.other(), f.seq.wrapping_add(1)),
        None => (Copy::A, 1),
    }
}

/// Reads the record from a two-sector region.
///
/// # Errors
/// The region failing to read.
pub fn load<S: Slot>(region: &mut S) -> Result<Option<Found>, S::Error> {
    let mut a = [0u8; LEN];
    let mut b = [0u8; LEN];
    region.read(Copy::A.offset(), &mut a)?;
    region.read(Copy::B.offset(), &mut b)?;
    Ok(choose(&a, &b))
}

/// Why a record did not get written.
#[derive(Debug, PartialEq, Eq)]
pub enum StoreError<E> {
    /// The region failed.
    Region(E),
    /// It was written and reads back differently.
    Readback,
}

/// Writes `trial` as the next record, and reads it back.
///
/// # Errors
/// The region failing, or a read-back that differs.
pub fn store<S: Slot>(
    region: &mut S,
    current: Option<&Found>,
    trial: Trial,
) -> Result<Found, StoreError<S::Error>> {
    let (copy, seq) = next(current);
    let frame = encode(trial, seq);
    region.write(copy.offset(), &frame).map_err(StoreError::Region)?;
    let mut check = [0u8; LEN];
    region
        .read(copy.offset(), &mut check)
        .map_err(StoreError::Region)?;
    if check != frame {
        return Err(StoreError::Readback);
    }
    Ok(Found { copy, seq, trial })
}
