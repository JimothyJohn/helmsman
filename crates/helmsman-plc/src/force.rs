//! Forced inputs: an engineer pins an input to a value from the editor
//! (docs/UI.md, "Inputs on the diagram"). Each scan, after the I/O read
//! and before any module runs, the scan thread writes every force over
//! what was read: an input signal's value in the [`SignalTable`], or bits
//! and bytes of the input image (`%IX`, `%IB`, `%IW`, `%ID`, addresses in
//! the box's input assembly, little-endian as the image is).
//!
//! The API owns the list ([`Forces`]); the scan thread never waits on it.
//! Each change publishes a new immutable set and bumps a generation; the
//! scan side ([`Reader`]) picks the new set up with a `try_lock` when the
//! generation moves, and keeps applying the one it has otherwise. So a
//! force lands within a scan or two of being made, and a release likewise.
//!
//! What a force may name is checked when it is made (the API): an input
//! signal (`in`, not another module's output) and a value of its type,
//! or an address inside the input image and a value that fits it. A
//! signal that a reload removes or retypes is skipped from then on (the
//! table refuses a value of the wrong type), never applied to something
//! else.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use helmsman_io::Value;

use crate::SignalTable;

/// Most forces at once on a box.
pub const MAX_FORCES: usize = 64;

/// A place in the input image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address {
    /// `%IXbyte.bit`
    Bit(usize, u8),
    /// `%IBn`
    Byte(usize),
    /// `%IWn`: two bytes from byte `n`.
    Word(usize),
    /// `%IDn`: four bytes from byte `n`.
    Dword(usize),
}

impl Address {
    /// Parses `%IX3.2`, `%IB4`, `%IW4`, `%ID4` (case-insensitive), held to an
    /// input image of `len` bytes.
    ///
    /// # Errors
    /// Not an input address of that form, or past the image.
    pub fn parse(s: &str, len: usize) -> Result<Self, String> {
        let up = s.to_ascii_uppercase();
        let rest = up
            .strip_prefix("%I")
            .ok_or_else(|| "an input address starts %I (%IX0.0, %IB0, %IW0, %ID0)".to_string())?;
        let num = |t: &str| -> Result<usize, String> {
            if t.is_empty() || t.len() > 5 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("not an address: {s:?}"));
            }
            t.parse().map_err(|_| format!("not an address: {s:?}"))
        };
        let (a, width) = match rest.as_bytes().first() {
            Some(b'X') => {
                let (byte, bit) = rest[1..]
                    .split_once('.')
                    .ok_or_else(|| format!("a bit address is %IXbyte.bit, got {s:?}"))?;
                let bit = num(bit)?;
                if bit > 7 {
                    return Err(format!("bit {bit} of a byte: 0 to 7"));
                }
                (Self::Bit(num(byte)?, u8::try_from(bit).unwrap_or(0)), 1)
            }
            Some(b'B') => (Self::Byte(num(&rest[1..])?), 1),
            Some(b'W') => (Self::Word(num(&rest[1..])?), 2),
            Some(b'D') => (Self::Dword(num(&rest[1..])?), 4),
            _ => return Err(format!("not an address: {s:?}")),
        };
        if a.start() + width > len {
            return Err(format!("{s} is past the {len}-byte input image"));
        }
        Ok(a)
    }

    const fn start(self) -> usize {
        match self {
            Self::Bit(b, _) | Self::Byte(b) | Self::Word(b) | Self::Dword(b) => b,
        }
    }

    /// Whether `v` is a value this address holds.
    #[must_use]
    pub fn admits(self, v: Value) -> bool {
        match (self, v) {
            (Self::Bit(..), Value::Bool(_)) => true,
            (Self::Byte(_), Value::Int(n)) => (0..=0xFF).contains(&n),
            (Self::Word(_), Value::Int(n)) => (0..=0xFFFF).contains(&n),
            (Self::Dword(_), Value::Int(n)) => (0..=0xFFFF_FFFF).contains(&n),
            _ => false,
        }
    }

    fn write(self, image: &mut [u8], v: Value) {
        let put = |image: &mut [u8], at: usize, bytes: &[u8]| {
            if let Some(dst) = image.get_mut(at..at + bytes.len()) {
                dst.copy_from_slice(bytes);
            }
        };
        match (self, v) {
            (Self::Bit(b, bit), Value::Bool(on)) => {
                if let Some(byte) = image.get_mut(b) {
                    if on {
                        *byte |= 1 << bit;
                    } else {
                        *byte &= !(1 << bit);
                    }
                }
            }
            (Self::Byte(b), Value::Int(n)) => put(image, b, &n.to_le_bytes()[..1]),
            (Self::Word(b), Value::Int(n)) => put(image, b, &n.to_le_bytes()[..2]),
            (Self::Dword(b), Value::Int(n)) => put(image, b, &n.to_le_bytes()[..4]),
            _ => {}
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bit(b, bit) => write!(f, "%IX{b}.{bit}"),
            Self::Byte(b) => write!(f, "%IB{b}"),
            Self::Word(b) => write!(f, "%IW{b}"),
            Self::Dword(b) => write!(f, "%ID{b}"),
        }
    }
}

/// What a force pins.
#[derive(Debug, Clone, PartialEq)]
pub enum Point {
    Signal(String),
    Address(Address),
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signal(s) => write!(f, "signal {s}"),
            Self::Address(a) => write!(f, "{a}"),
        }
    }
}

/// One force.
#[derive(Debug, Clone, PartialEq)]
pub struct Force {
    pub id: u64,
    pub point: Point,
    pub value: Value,
    /// Who forced it, as the audit log names them.
    pub by: String,
    /// The SHA-256 of the session that made it: it ends with that session.
    pub session: [u8; 32],
    /// Wall-clock seconds when it was made.
    pub at: u64,
}

#[derive(Default)]
struct Inner {
    list: Vec<Force>,
    next: u64,
}

/// The box's forces: made and released by the API, applied by the scan.
#[derive(Default)]
pub struct Forces {
    inner: Mutex<Inner>,
    /// What the scan applies; replaced whole on each change.
    published: Mutex<Arc<Vec<Force>>>,
    generation: AtomicU64,
}

impl fmt::Debug for Forces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Forces")
            .field("forces", &self.list().len())
            .finish()
    }
}

impl Forces {
    fn publish(&self, list: &[Force]) {
        *self.published.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(list.to_vec());
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Every force, oldest first.
    #[must_use]
    pub fn list(&self) -> Vec<Force> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .list
            .clone()
    }

    /// Adds a force (one per point: a new force on a forced point replaces
    /// it). The force, with its id, and the one it replaced.
    ///
    /// # Errors
    /// [`MAX_FORCES`] forces already.
    pub fn add(&self, mut f: Force) -> Result<(Force, Option<Force>), String> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let replaced = inner
            .list
            .iter()
            .position(|g| g.point == f.point)
            .map(|k| inner.list.remove(k));
        if replaced.is_none() && inner.list.len() >= MAX_FORCES {
            return Err(format!("at most {MAX_FORCES} forces at once"));
        }
        inner.next += 1;
        f.id = inner.next;
        inner.list.push(f.clone());
        self.publish(&inner.list);
        Ok((f, replaced))
    }

    /// Releases the forces `keep` says no to; the released ones.
    pub fn release_where(&self, mut release: impl FnMut(&Force) -> bool) -> Vec<Force> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let (gone, kept): (Vec<Force>, Vec<Force>) = inner.list.drain(..).partition(|f| release(f));
        inner.list = kept;
        if !gone.is_empty() {
            self.publish(&inner.list);
        }
        gone
    }

    /// Releases one force by id.
    pub fn release(&self, id: u64) -> Option<Force> {
        self.release_where(|f| f.id == id).pop()
    }

    /// Releases every force.
    pub fn release_all(&self) -> Vec<Force> {
        self.release_where(|_| true)
    }

    /// The session hashes that hold forces.
    #[must_use]
    pub fn sessions(&self) -> Vec<[u8; 32]> {
        let mut v: Vec<[u8; 32]> = self.list().iter().map(|f| f.session).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Whether `signal` is forced (any case).
    #[must_use]
    pub fn is_forced(&self, signal: &str) -> bool {
        self.list()
            .iter()
            .any(|f| matches!(&f.point, Point::Signal(s) if s.eq_ignore_ascii_case(signal)))
    }
}

/// The scan thread's side: the set it applies, refreshed without waiting.
#[derive(Debug, Default)]
pub struct Reader {
    seen: u64,
    set: Arc<Vec<Force>>,
}

impl Reader {
    /// Writes every force over what was read: signals into `table`,
    /// addresses into `inputs` (the input image). Picks up a newer set
    /// first if it can without waiting.
    pub fn apply(&mut self, forces: &Forces, inputs: &mut [u8], table: &mut SignalTable) {
        let g = forces.generation.load(Ordering::Acquire);
        if g != self.seen
            && let Ok(p) = forces.published.try_lock()
        {
            self.set = Arc::clone(&p);
            self.seen = g;
        }
        apply(&self.set, inputs, table);
    }
}

/// Writes `set` over `inputs` and `table`.
pub fn apply(set: &[Force], inputs: &mut [u8], table: &mut SignalTable) {
    for f in set {
        match &f.point {
            Point::Signal(name) => {
                if let Some(i) = table.index(name) {
                    table.set_input(i, Some(f.value));
                }
            }
            Point::Address(a) => a.write(inputs, f.value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_and_stay_in_the_image() {
        assert_eq!(Address::parse("%IX3.2", 32), Ok(Address::Bit(3, 2)));
        assert_eq!(Address::parse("%iw30", 32), Ok(Address::Word(30)));
        for bad in [
            "%IX3.8",
            "%IW31",
            "%ID29",
            "%QX0.0",
            "%IX1",
            "%IB",
            "%IB-1",
            "IX0.0",
            "%IB 1",
            "%IB999999",
        ] {
            assert!(Address::parse(bad, 32).is_err(), "{bad}");
        }
    }

    #[test]
    fn forces_write_bits_and_words_little_endian() {
        let set = vec![
            Force {
                id: 1,
                point: Point::Address(Address::Bit(0, 3)),
                value: Value::Bool(true),
                by: String::new(),
                session: [0; 32],
                at: 0,
            },
            Force {
                id: 2,
                point: Point::Address(Address::Word(2)),
                value: Value::Int(0x1234),
                by: String::new(),
                session: [0; 32],
                at: 0,
            },
        ];
        let mut img = vec![0u8; 4];
        apply(&set, &mut img, &mut SignalTable::default());
        assert_eq!(img, [0b1000, 0, 0x34, 0x12]);
    }
}
