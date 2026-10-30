//! A small seeded generator for retry jitter: deterministic in tests,
//! seeded from the clock in the binary. Not for anything secret.

/// `SplitMix64` (Steele, Lea, Flood 2014): one 64-bit word of state, every
/// seed is valid.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `lo..=hi` (modulo bias is irrelevant for jitter); `lo` if the
    /// range is empty.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        match (hi - lo).checked_add(1) {
            Some(n) => lo + self.next_u64() % n,
            None => self.next_u64(),
        }
    }
}
