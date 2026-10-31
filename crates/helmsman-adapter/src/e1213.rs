//! The application side of a simulated Moxa ioLogik E1213 (the layout is
//! [`DeviceConfig::moxa_e1213`](crate::DeviceConfig::moxa_e1213)): what the
//! device does with the output assembly and how it fills the input one.
//!
//! Byte layout, from the ioLogik E1200 Series User's Manual v15.10,
//! Appendix B "EtherNet/IP Default Address Mappings", pp. B-1–B-2
//! (`docs/devices/moxa-iologik-e1213.md` has the table):
//!
//! - input assembly 101 (51 bytes): bytes 0–1 `diStatus` DI-00..DI-11,
//!   bytes 2–49 `diCounterValue` (UDINT), byte 50 `doStatus` DO-00..DO-07;
//! - output assembly 102 (1 byte): byte 0 `doStatus` DO-00..DO-07.
//!
//! Inferred, not in the manual: bit n of the little-endian word is channel
//! n (CIP's packing of BOOL arrays), and counter n is the UDINT at
//! `2 + 4n`, little-endian.
//!
//! Simulator choices, not the device's documented behaviour: outputs are
//! on only while an exclusive owner is connected and in run (otherwise
//! the DO status reads 0, as if the device fell back to "off"), and every
//! DI counts its rising edges whatever its mode (the real counter counts
//! only in counter mode, which the manual says is off by default).

use crate::device::Device;

pub const INPUT: u16 = 101;
pub const OUTPUT: u16 = 102;
pub const INPUT_LEN: usize = 51;
/// DI channels: 8 DI and 4 DIO, which read as DI-08..DI-11 when jumpered
/// to DI (manual p.2-5; their numbering is inferred).
pub const DI_CHANNELS: u8 = 12;
/// DO channels: 4 DO and 4 DIO (DO-04..DO-07 when jumpered to DO, the
/// default; numbering inferred).
pub const DO_CHANNELS: u8 = 8;
/// Offset of DI-00's counter in the input assembly.
pub const COUNTER_OFFSET: usize = 2;
/// Offset of the DO status byte.
pub const DO_STATUS_OFFSET: usize = 50;

/// The E1213's I/O, with outputs wired back to inputs like a bench.
#[derive(Debug, Clone, Default)]
pub struct Application {
    /// `(do, di)`: DO-`do` drives DI-`di`.
    pub wires: Vec<(u8, u8)>,
    /// Inputs held on regardless of the wires (a switch on the panel).
    pub forced: u16,
    di: u16,
    counters: [u32; DI_CHANNELS as usize],
}

impl Application {
    /// The tabletop bench: DO-00..DO-03 wired to DI-00..DI-03.
    #[must_use]
    pub fn bench() -> Self {
        Self {
            wires: (0..4).map(|n| (n, n)).collect(),
            ..Self::default()
        }
    }

    /// Reads the outputs, updates the inputs. Call it often (every
    /// millisecond or so); it is cheap.
    pub fn step(&mut self, dev: &mut Device) {
        let outputs = if dev.scanner_running() {
            dev.assembly(OUTPUT).and_then(|a| a.first().copied()).unwrap_or(0)
        } else {
            0
        };
        let mut di = self.forced & ((1 << DI_CHANNELS) - 1);
        for &(o, i) in &self.wires {
            if o < DO_CHANNELS && i < DI_CHANNELS && outputs & (1 << o) != 0 {
                di |= 1 << i;
            }
        }
        let rising = di & !self.di;
        for (n, c) in self.counters.iter_mut().enumerate() {
            if rising & (1 << n) != 0 {
                *c = c.wrapping_add(1);
            }
        }
        self.di = di;
        let mut input = [0u8; INPUT_LEN];
        input[..2].copy_from_slice(&di.to_le_bytes());
        for (n, c) in self.counters.iter().enumerate() {
            let at = COUNTER_OFFSET + 4 * n;
            input[at..at + 4].copy_from_slice(&c.to_le_bytes());
        }
        input[DO_STATUS_OFFSET] = outputs;
        if dev.assembly(INPUT) != Some(&input[..]) {
            // Only fails if the device is not an E1213 layout.
            let _ = dev.write_assembly(INPUT, &input);
        }
    }
}
