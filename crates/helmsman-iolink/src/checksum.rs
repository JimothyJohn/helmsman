//! The message checksum (IOL §A.1.6 p.240-241): every octet of a message
//! combined by exclusive or onto a seed, the check octet taking part with
//! its checksum bits at zero, and the result folded from 8 bits to 6.

/// IOL §A.1.6 p.240: "A seed value of 0x52 is used".
pub const SEED: u8 = 0x52;

/// The six checksum bits of a check/type or check/status octet (IOL
/// Figures A.2 and A.3); the two above them are the octet's own flags.
pub const MASK: u8 = 0x3F;

/// Equations (A.1), IOL p.241: bit 5 is the XOR of the odd bits, bit 4 of
/// the even ones, bits 3 to 0 of each neighbouring pair from the top.
#[must_use]
pub const fn compress(sum: u8) -> u8 {
    let odd = (sum >> 7) ^ (sum >> 5) ^ (sum >> 3) ^ (sum >> 1);
    let even = (sum >> 6) ^ (sum >> 4) ^ (sum >> 2) ^ sum;
    let pairs = sum ^ (sum >> 1);
    ((odd & 1) << 5)
        | ((even & 1) << 4)
        | (((pairs >> 6) & 1) << 3)
        | (((pairs >> 4) & 1) << 2)
        | (((pairs >> 2) & 1) << 1)
        | (pairs & 1)
}

/// The checksum of `message`, whose check octet is at `check`. That
/// octet's checksum bits are read as zero, so the same call makes the
/// checksum of a message being built and checks one that arrived.
#[must_use]
pub fn of(message: &[u8], check: usize) -> u8 {
    let mut sum = SEED;
    for (at, octet) in message.iter().enumerate() {
        sum ^= if at == check { octet & !MASK } else { *octet };
    }
    compress(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Worked by hand from equations (A.1), not by this code.
    #[test]
    fn compress_follows_the_equations() {
        assert_eq!(compress(0x00), 0x00);
        assert_eq!(compress(0xF0), 0x00);
        assert_eq!(compress(0x45), 0x1B);
        assert_eq!(compress(0xC5), 0x33);
        assert_eq!(compress(0xE8), 0x36);
        // One input bit at a time: bit 7 feeds D5 and D3, bit 0 D4 and D0.
        assert_eq!(compress(0x80), 0b10_1000);
        assert_eq!(compress(0x40), 0b01_1000);
        assert_eq!(compress(0x20), 0b10_0100);
        assert_eq!(compress(0x10), 0b01_0100);
        assert_eq!(compress(0x08), 0b10_0010);
        assert_eq!(compress(0x04), 0b01_0010);
        assert_eq!(compress(0x02), 0b10_0001);
        assert_eq!(compress(0x01), 0b01_0001);
    }

    #[test]
    fn the_check_octets_own_checksum_bits_do_not_count() {
        assert_eq!(of(&[0xA2, 0x00], 1), of(&[0xA2, 0x3F], 1));
        assert_ne!(of(&[0xA2, 0x00], 1), of(&[0xA2, 0x40], 1));
    }
}
