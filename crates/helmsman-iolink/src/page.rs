//! Direct Parameter page 1 (IOL §B.1 p.258-263): the sixteen octets every
//! Device has, read and written one at a time on the page channel. They
//! say how to talk to the Device (cycle time, M-sequence types, Process
//! Data lengths) and which Device it is (vendor and device id).

use crate::mseq::{Error, Layout};

/// IOL Table B.1 p.259: the addresses of page 1.
pub const MASTER_COMMAND: u8 = 0x00;
pub const MASTER_CYCLE_TIME: u8 = 0x01;
pub const MIN_CYCLE_TIME: u8 = 0x02;
pub const M_SEQUENCE_CAPABILITY: u8 = 0x03;
pub const REVISION_ID: u8 = 0x04;
pub const PROCESS_DATA_IN: u8 = 0x05;
pub const PROCESS_DATA_OUT: u8 = 0x06;
pub const VENDOR_ID: u8 = 0x07;
pub const DEVICE_ID: u8 = 0x09;
pub const FUNCTION_ID: u8 = 0x0C;
pub const SYSTEM_COMMAND: u8 = 0x0F;
/// Page 1 is addresses 0x00 to 0x0F (IOL §B.1.1 p.258).
pub const PAGE_1_LEN: usize = 16;

/// IOL Table B.2 p.259-260: the values written to [`MASTER_COMMAND`].
pub mod command {
    /// Back to SIO mode.
    pub const FALLBACK: u8 = 0x5A;
    /// The Master's revision is above 1.0.
    pub const MASTER_IDENT: u8 = 0x95;
    /// Check the page for changed entries.
    pub const DEVICE_IDENT: u8 = 0x96;
    /// OPERATE or PREOPERATE to STARTUP.
    pub const DEVICE_STARTUP: u8 = 0x97;
    /// The output Process Data is valid.
    pub const PROCESS_DATA_OUTPUT_OPERATE: u8 = 0x98;
    /// To OPERATE; the output Process Data is invalid or absent.
    pub const DEVICE_OPERATE: u8 = 0x99;
    /// STARTUP to PREOPERATE.
    pub const DEVICE_PREOPERATE: u8 = 0x9A;
}

/// The protocol revision this crate speaks (IOL Table B.1: "shall be set
/// to 0x11", §B.1.5 p.261-262: major digit in the high nibble).
pub const REVISION_1_1: u8 = 0x11;

/// A `MasterCycleTime` or `MinCycleTime` octet as a time in units of
/// 100 µs (IOL §B.1.3 and Table B.3 p.260-261). `None` for the reserved
/// time base. 0 is a Device that states no minimum.
#[must_use]
pub const fn cycle_time(octet: u8) -> Option<u16> {
    let multiplier = (octet & 0x3F) as u16;
    match octet >> 6 {
        0 => Some(multiplier),
        1 => Some(64 + multiplier * 4),
        2 => Some(320 + multiplier * 16),
        _ => None,
    }
}

/// The octet for the shortest cycle time of IOL Table B.3 that is at
/// least `tenths` (units of 100 µs), held to the table's range of 0,4 ms
/// to 132,8 ms.
#[must_use]
pub const fn cycle_time_octet(tenths: u16) -> u8 {
    // Both quotients are at most 63 in their arms, so the casts keep
    // every bit.
    #[allow(clippy::cast_possible_truncation)]
    match tenths {
        0..=4 => 4,
        5..=63 => tenths as u8,
        64..=316 => 0x40 | (tenths - 64).div_ceil(4) as u8,
        317..=1328 => 0x80 | tenths.saturating_sub(320).div_ceil(16) as u8,
        _ => 0x80 | 63,
    }
}

/// The M-sequence capability octet (IOL §B.1.4 p.261).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability(pub u8);

impl Capability {
    /// Bit 0: the Device has the ISDU channel.
    #[must_use]
    pub const fn isdu(self) -> bool {
        self.0 & 1 != 0
    }

    /// Bits 1 to 3: the OPERATE M-sequence code (IOL Table A.10).
    #[must_use]
    pub const fn operate(self) -> u8 {
        (self.0 >> 1) & 7
    }

    /// Bits 4 to 5: the PREOPERATE M-sequence code (IOL Table A.8).
    #[must_use]
    pub const fn preoperate(self) -> u8 {
        (self.0 >> 4) & 3
    }
}

/// A `ProcessDataIn` or `ProcessDataOut` octet (IOL §B.1.6, §B.1.7 and
/// Table B.6 p.262-263).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessData {
    /// The length in bits: 0 to 16 as the Device states it, or eight
    /// times its octets from 3 octets up.
    pub bits: u16,
    /// Bit 6 of `ProcessDataIn`: the Device has a switching signal in SIO
    /// mode. Reserved in `ProcessDataOut`.
    pub sio: bool,
}

impl ProcessData {
    /// # Errors
    /// [`Error::ProcessData`] for the reserved lengths of IOL Table B.6.
    /// The reserved bit 5 is not looked at.
    pub const fn decode(octet: u8) -> Result<Self, Error> {
        let length = (octet & 0x1F) as u16;
        let bits = match (octet & 0x80 != 0, length) {
            (false, 0..=16) => length,
            (true, 2..=31) => (length + 1) * 8,
            _ => return Err(Error::ProcessData { octet }),
        };
        Ok(Self {
            bits,
            sio: octet & 0x40 != 0,
        })
    }

    /// Octets on the wire: a length in bits rounded up (1 to 8 bits
    /// travel as one octet, 9 to 16 as two; IOL Table A.10).
    #[must_use]
    pub const fn octets(self) -> u8 {
        // At most 32: `decode` gives no more than 256 bits.
        #[allow(clippy::cast_possible_truncation)]
        {
            self.bits.div_ceil(8) as u8
        }
    }
}

/// Page 1 as read from a Device, octet for octet (IOL Table B.1 p.259).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page1(pub [u8; PAGE_1_LEN]);

impl Page1 {
    /// The shortest cycle the Device supports, in units of 100 µs; `None`
    /// for the reserved time base.
    #[must_use]
    pub const fn min_cycle_time(&self) -> Option<u16> {
        cycle_time(self.0[MIN_CYCLE_TIME as usize])
    }

    #[must_use]
    pub const fn capability(&self) -> Capability {
        Capability(self.0[M_SEQUENCE_CAPABILITY as usize])
    }

    /// The protocol revision, major digit in the high nibble.
    #[must_use]
    pub const fn revision(&self) -> u8 {
        self.0[REVISION_ID as usize]
    }

    /// # Errors
    /// [`Error::ProcessData`] for a reserved length.
    pub const fn process_data_in(&self) -> Result<ProcessData, Error> {
        ProcessData::decode(self.0[PROCESS_DATA_IN as usize])
    }

    /// # Errors
    /// [`Error::ProcessData`] for a reserved length.
    pub const fn process_data_out(&self) -> Result<ProcessData, Error> {
        ProcessData::decode(self.0[PROCESS_DATA_OUT as usize])
    }

    /// Two octets, most significant first.
    #[must_use]
    pub const fn vendor_id(&self) -> u16 {
        u16::from_be_bytes([self.0[7], self.0[8]])
    }

    /// Three octets, most significant first.
    #[must_use]
    pub const fn device_id(&self) -> u32 {
        u32::from_be_bytes([0, self.0[9], self.0[10], self.0[11]])
    }

    /// The M-sequence type the Device uses in PREOPERATE.
    ///
    /// # Errors
    /// As [`Layout::preoperate`].
    pub const fn preoperate(&self) -> Result<Layout, Error> {
        Layout::preoperate(self.capability().preoperate())
    }

    /// The M-sequence type the Device uses in OPERATE.
    ///
    /// # Errors
    /// As [`ProcessData::decode`] and [`Layout::operate`].
    pub fn operate(&self) -> Result<Layout, Error> {
        let pd_in = self.process_data_in()?.octets();
        let pd_out = self.process_data_out()?.octets();
        Layout::operate(self.capability().operate(), pd_in, pd_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IOL Table B.3: the ends of each time base.
    #[test]
    fn cycle_times_follow_table_b3() {
        assert_eq!(cycle_time(0x00), Some(0));
        assert_eq!(cycle_time(0x04), Some(4));
        assert_eq!(cycle_time(0x3F), Some(63));
        assert_eq!(cycle_time(0x40), Some(64));
        assert_eq!(cycle_time(0x7F), Some(316));
        assert_eq!(cycle_time(0x80), Some(320));
        assert_eq!(cycle_time(0xBF), Some(1328));
        assert_eq!(cycle_time(0xC0), None);
        assert_eq!(cycle_time_octet(0), 0x04);
        assert_eq!(cycle_time_octet(23), 23);
        assert_eq!(cycle_time_octet(64), 0x40);
        assert_eq!(cycle_time_octet(65), 0x41);
        assert_eq!(cycle_time_octet(317), 0x80);
        assert_eq!(cycle_time_octet(321), 0x81);
        assert_eq!(cycle_time_octet(u16::MAX), 0xBF);
    }

    /// IOL Table B.6.
    #[test]
    fn process_data_lengths_follow_table_b6() {
        let bits = |octet| ProcessData::decode(octet).map(|pd| (pd.bits, pd.octets()));
        assert_eq!(bits(0x00), Ok((0, 0)));
        assert_eq!(bits(0x01), Ok((1, 1)));
        assert_eq!(bits(0x08), Ok((8, 1)));
        assert_eq!(bits(0x09), Ok((9, 2)));
        assert_eq!(bits(0x10), Ok((16, 2)));
        assert_eq!(bits(0x11), Err(Error::ProcessData { octet: 0x11 }));
        assert_eq!(bits(0x80), Err(Error::ProcessData { octet: 0x80 }));
        assert_eq!(bits(0x81), Err(Error::ProcessData { octet: 0x81 }));
        assert_eq!(bits(0x82), Ok((24, 3)));
        assert_eq!(bits(0x9F), Ok((256, 32)));
        assert!(ProcessData::decode(0x48).unwrap().sio);
        assert!(!ProcessData::decode(0x08).unwrap().sio);
    }

    /// A made-up Device: 2,3 ms minimum cycle, ISDU, OPERATE code 0 and
    /// PREOPERATE code 1, revision 1.1, 16 bits in with SIO, 8 bits out.
    #[test]
    fn a_page_reads_as_its_fields() {
        let page = Page1([
            0x00, 0x00, 0x17, 0x11, 0x11, 0x50, 0x08, 0x01, 0x36, 0x00, 0x02, 0x9A, 0, 0, 0, 0,
        ]);
        assert_eq!(page.min_cycle_time(), Some(23));
        assert!(page.capability().isdu());
        assert_eq!(page.capability().operate(), 0);
        assert_eq!(page.capability().preoperate(), 1);
        assert_eq!(page.revision(), REVISION_1_1);
        assert_eq!(page.vendor_id(), 0x0136);
        assert_eq!(page.device_id(), 0x00_029A);
        assert_eq!(page.preoperate(), Layout::type1(2).ok_or(Error::Interleaved));
        assert_eq!(page.operate(), Layout::type2(1, 1, 2).ok_or(Error::Interleaved));
    }
}
