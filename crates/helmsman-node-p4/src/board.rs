//! Waveshare ESP32-P4-ETH board support: the EMAC's RMII pins, the PHY and
//! what the console calls the board.
//!
//! The pin map is the one esp-hal's `embassy_ethernet` example documents for
//! the ESP32-P4-Function-EV board and `esp-p4-eth` (tested on this Waveshare
//! board, 2026) records as `BoardConfig::WAVESHARE_P4_ETH`; both agree. The
//! P4's RMII data pins go through the IO mux, so only these numbers work:
//! RXD0 GPIO29, RXD1 GPIO30, `CRS_DV` GPIO28, TXD0 GPIO34, TXD1 GPIO35,
//! `TX_EN` GPIO49; MDC GPIO31 and MDIO GPIO52; the 50 MHz reference clock comes
//! *from* the IP101GRI (its 25 MHz crystal, REFCLKO) into GPIO50; the PHY's
//! reset is GPIO51, active low, and its MDIO address is strapped to 1.
//!
//! Power: power over Ethernet through the board's plug-in module, or
//! USB-C. Whether that module is isolated is not documented anywhere we
//! found; until it is measured, the POE2 rule applies: never USB and the
//! Ethernet cable at the same time.

use esp_hal::peripherals::{
    ETH, GPIO28, GPIO29, GPIO30, GPIO31, GPIO34, GPIO35, GPIO49, GPIO50, GPIO51, GPIO52,
};

/// The board this image is for, as the console names it.
pub const BOARD: &str = "Waveshare ESP32-P4-ETH";
/// The PHY's Clause 22 address (the Waveshare strap).
pub const PHY_ADDR: u8 = 1;

/// The peripherals the Ethernet bring-up takes, moved out of
/// `esp_hal::Peripherals` by the binary.
pub struct EthPins {
    pub eth: ETH<'static>,
    /// `REF_CLK` in, from the PHY.
    pub ref_clk: GPIO50<'static>,
    pub rxd0: GPIO29<'static>,
    pub rxd1: GPIO30<'static>,
    pub rx_dv: GPIO28<'static>,
    pub txd0: GPIO34<'static>,
    pub txd1: GPIO35<'static>,
    pub tx_en: GPIO49<'static>,
    pub mdc: GPIO31<'static>,
    pub mdio: GPIO52<'static>,
    /// PHY reset, active low.
    pub phy_reset: GPIO51<'static>,
}

/// The node's MAC address: the factory one from efuse, which is what the
/// switch and the DHCP server will remember this board by.
#[must_use]
pub fn mac_address() -> [u8; 6] {
    // ESP-IDF derives the Ethernet MAC from the base one (base + 3); the
    // base itself is unique per chip and good enough for one interface.
    let mac = esp_hal::efuse::base_mac_address();
    let mut out = [0u8; 6];
    out.copy_from_slice(mac.as_bytes());
    out
}

/// A serial for the originator side of CIP connections (the target keys a
/// connection on serial, vendor and originator serial): the low four bytes
/// of the MAC, which differ between two boards on one bench.
#[must_use]
pub fn originator_serial(mac: [u8; 6]) -> u32 {
    u32::from_le_bytes([mac[5], mac[4], mac[3], mac[2]])
}
