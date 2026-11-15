//! Transmission rates and the times that are counted in bit times (IOL
//! §5.3.3 p.58-61, §7.3.2.2 p.80-82, §A.3 p.247-249). Times are in
//! microseconds on whatever clock the caller keeps.

/// IOL Table 9 p.59: the three transmission rates. A Device has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rate {
    /// 4,8 kbit/s.
    Com1,
    /// 38,4 kbit/s.
    Com2,
    /// 230,4 kbit/s.
    Com3,
}

impl Rate {
    #[must_use]
    pub const fn baud(self) -> u32 {
        match self {
            Self::Com1 => 4_800,
            Self::Com2 => 38_400,
            Self::Com3 => 230_400,
        }
    }

    /// `bits` bit times (IOL equation (A.2) p.247), rounded up.
    #[must_use]
    pub const fn micros(self, bits: usize) -> u64 {
        (bits as u64 * 1_000_000).div_ceil(self.baud() as u64)
    }

    /// The rate a Master tries after this one found nobody (IOL §7.3.2.2
    /// p.81: "in descending order until a response is obtained").
    #[must_use]
    pub const fn lower(self) -> Option<Self> {
        match self {
            Self::Com3 => Some(Self::Com2),
            Self::Com2 => Some(Self::Com1),
            Self::Com1 => None,
        }
    }
}

/// IOL Table 10 p.60: the longest wake-up current pulse, `T_WU`.
pub const T_WU_US: u64 = 85;
/// IOL Table 10 p.60: a Device is ready to receive `T_REN` after the
/// pulse at the latest.
pub const T_REN_US: u64 = 500;
/// IOL Table 42 p.82: `T_DMT`, the delay before a test message, 27 to 37
/// bit times of the rate about to be used. The middle of the window.
pub const T_DMT_BITS: usize = 32;
/// IOL Table 42 p.82: `T_DWU`, 30 to 50 ms between wake-up requests of
/// one retry sequence.
pub const T_DWU_US: u64 = 40_000;
/// IOL Table 42 p.82 (`n_WU` = 2 retries) and §7.3.2.2 p.81: "up to
/// `n_WU` + 1 successive wake-up requests".
pub const WAKE_UPS: u8 = 3;
/// IOL Table 42 p.82: `T_SD`, 0,5 to 1 s between two retry sequences.
pub const T_SD_US: u64 = 750_000;
/// IOL Table 42 p.82: `T_DSIO`, after which a woken Device nobody talked
/// to is back in SIO mode (60 to 300 ms; the longest).
pub const T_DSIO_US: u64 = 300_000;
/// IOL Table 46 p.95 and Table 102: a message is sent again twice.
pub const MAX_RETRY: u8 = 2;

/// The longest an M-sequence of `master` octets out and `device` octets
/// back takes, in bit times (IOL equation (A.6) p.248 with the largest
/// `t_A` of 10, `t_1` of 1 and `t_2` of 3 bit times, §A.3.3 to §A.3.5).
#[must_use]
pub const fn m_sequence_bits(master: usize, device: usize) -> usize {
    (master + device) * 11 + 10 + master.saturating_sub(1) + device.saturating_sub(1) * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IOL Table 9: 208,33 µs, 26,04 µs and 4,34 µs.
    #[test]
    fn bit_times_follow_table_9() {
        assert_eq!(Rate::Com1.micros(100), 20_834);
        assert_eq!(Rate::Com2.micros(100), 2_605);
        assert_eq!(Rate::Com3.micros(100), 435);
    }

    /// A `TYPE_0` read: 2 octets out, 2 back.
    #[test]
    fn the_test_message_takes_at_most_58_bit_times() {
        assert_eq!(m_sequence_bits(2, 2), 58);
        assert_eq!(m_sequence_bits(3, 1), 56);
    }
}
