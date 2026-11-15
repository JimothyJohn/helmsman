//! Values inside Process Data, by bit offset and bit length as a
//! Device's IODD gives them (IOL Annex F p.298-307).
//!
//! The octets are sent most significant first (Table F.3 NOTE 2 p.299),
//! and a bit offset counts from the other end: "the bit offset starts
//! with the last octet within the sequence; this octet starts with
//! offset 0 for the least significant bit" (Table F.19 rule 3 p.305).

/// The octet and the bit in it that hold bit `at` of the sequence.
const fn place(octets: usize, at: usize) -> Option<(usize, u32)> {
    let back = at / 8;
    if back >= octets {
        return None;
    }
    // `at % 8` is at most 7.
    #[allow(clippy::cast_possible_truncation)]
    Some((octets - 1 - back, (at % 8) as u32))
}

/// The unsigned value of the `bit_length` bits (1 to 64) from
/// `bit_offset` up. `None` when they are not all inside `data`.
#[must_use]
pub fn get(data: &[u8], bit_offset: usize, bit_length: usize) -> Option<u64> {
    if !(1..=64).contains(&bit_length) {
        return None;
    }
    let mut value = 0;
    for bit in (0..bit_length).rev() {
        let (octet, shift) = place(data.len(), bit_offset.checked_add(bit)?)?;
        value = (value << 1) | u64::from((data[octet] >> shift) & 1);
    }
    Some(value)
}

/// The same bits as a signed number, the top one being the sign (IOL
/// §F.2.4 p.299: two's complement, extended to the container).
#[must_use]
pub fn get_signed(data: &[u8], bit_offset: usize, bit_length: usize) -> Option<i64> {
    let value = get(data, bit_offset, bit_length)?;
    // 1 to 63, since `get` took a length of 1 to 64.
    #[allow(clippy::cast_possible_truncation)]
    let unused = (64 - bit_length) as u32;
    // The bits moved to the top and back down, the sign following them.
    #[allow(clippy::cast_possible_wrap)]
    Some(((value << unused) as i64) >> unused)
}

/// Writes the low `bit_length` bits (1 to 64) of `value` from
/// `bit_offset` up and leaves every other bit as it was. False, with
/// nothing written, when the bits are not all inside `data` or `value`
/// has bits above `bit_length`.
pub fn put(data: &mut [u8], bit_offset: usize, bit_length: usize, value: u64) -> bool {
    if !(1..=64).contains(&bit_length)
        || (bit_length < 64 && value >> bit_length != 0)
        || bit_offset
            .checked_add(bit_length - 1)
            .and_then(|top| place(data.len(), top))
            .is_none()
    {
        return false;
    }
    for bit in 0..bit_length {
        if let Some((octet, shift)) = place(data.len(), bit_offset + bit) {
            if (value >> bit) & 1 == 1 {
                data[octet] |= 1 << shift;
            } else {
                data[octet] &= !(1 << shift);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IOL Table F.21 and Figure F.8 p.306: "Level" 0x32F1 in 14 bits at
    /// offset 2, "Min" FALSE at 1, "Max" TRUE at 0, drawn as the octets
    /// 0xCB 0xC5.
    #[test]
    fn record_example_2_reads_as_printed() {
        let data = [0xCB, 0xC5];
        assert_eq!(get(&data, 2, 14), Some(0x32F1));
        assert_eq!(get(&data, 1, 1), Some(0));
        assert_eq!(get(&data, 0, 1), Some(1));
        let mut built = [0; 2];
        assert!(put(&mut built, 2, 14, 0x32F1));
        assert!(put(&mut built, 0, 1, 1));
        assert_eq!(built, data);
    }

    /// IOL Table F.20 p.306: "Status" 0x2345 at offset 88, "Text" HELLO
    /// in 7 octets at 32, "Value" 0x56122234 at 0.
    #[test]
    fn record_example_1_reads_as_printed() {
        let data = [
            0x23, 0x45, b'H', b'E', b'L', b'L', b'O', 0, 0, 0x56, 0x12, 0x22, 0x34,
        ];
        assert_eq!(get(&data, 88, 16), Some(0x2345));
        assert_eq!(get(&data, 0, 32), Some(0x5612_2234));
        assert_eq!(get(&data, 32 + 48, 8), Some(u64::from(b'H')));
        assert_eq!(get(&data, 32, 56), Some(0x48_454C_4C4F_0000));
    }

    /// IOL Table F.18 and Figure F.7 p.305: five 3-bit `IntegerT` at
    /// offsets 12, 9, 6, 3 and 0 holding 0x2, 0x6, 0x4, 0x7 and 0x5.
    #[test]
    fn the_array_example_reads_as_printed() {
        let data = [0x2D, 0x3D];
        let unsigned: Vec<_> = [12, 9, 6, 3, 0].iter().map(|at| get(&data, *at, 3)).collect();
        assert_eq!(unsigned, [Some(2), Some(6), Some(4), Some(7), Some(5)]);
        let signed: Vec<_> = [12, 9, 6, 3, 0]
            .iter()
            .map(|at| get_signed(&data, *at, 3))
            .collect();
        assert_eq!(signed, [Some(2), Some(-2), Some(-4), Some(-1), Some(-3)]);
    }

    #[test]
    fn bits_outside_the_data_are_refused() {
        let mut data = [0xFF; 2];
        assert_eq!(get(&data, 9, 8), None);
        assert_eq!(get(&data, 16, 1), None);
        assert_eq!(get(&data, 0, 0), None);
        assert_eq!(get(&data, 0, 65), None);
        assert_eq!(get(&data, usize::MAX, 2), None);
        assert_eq!(get(&[], 0, 1), None);
        assert!(!put(&mut data, 9, 8, 0));
        assert!(!put(&mut data, 0, 4, 0x10));
        assert!(!put(&mut data, usize::MAX, 2, 0));
        assert_eq!(data, [0xFF; 2]);
        assert_eq!(get_signed(&[0xFF; 8], 0, 64), Some(-1));
        assert_eq!(get_signed(&[0x80], 7, 1), Some(-1));
    }
}
