//! Olimex ESP32-POE2 board support, copied from supervictor's `board/poe_iso.rs`
//! and `board/poe2.rs` (2026-11-03; HARDWARE.md §4.2 there) rather than
//! depending on that crate: the pin map, the APLL that makes the PHY's
//! 50 MHz RMII reference clock, and the clock's route out of GPIO0.
//!
//! The POE2's ESP32-WROVER-E keeps GPIO16/17 for its PSRAM, so Olimex takes
//! the RMII clock out of **GPIO0** (their `LAN8720-POE2` Arduino example:
//! `ETH_CLOCK_GPIO0_OUT`). GPIO0 has no EMAC clock-out IO_MUX function, so
//! ESP-IDF routes the raw APLL through the chip's generic clock-output
//! channel 1 (`esp_clock_output_start(CLKOUT_SIG_APLL, 0)` in
//! `esp_eth_mac_esp.c`): IO_MUX GPIO0 function `CLK_OUT1`, `PIN_CTRL[3:0]`
//! = `CLKOUT_SIG_APLL`. GPIO0 is also the boot strap; the PHY is unpowered
//! (GPIO12 low) until bring-up, so the pad floats high at reset and the
//! clock only starts once we run, as Olimex ships it.
//!
//! No galvanic isolation on this board: never USB and Ethernet together.

/// PHY address on the SMI bus (LAN8720A PHYAD0 strapped low).
pub const PHY_ADDR: u8 = 0;
/// Gates 3.3 V to the PHY; driven high before any MDIO access.
pub const PHY_POWER_GPIO: u8 = 12;
/// RMII reference clock output: APLL via clock-output channel 1 on GPIO0.
pub const RMII_CLK_OUT_GPIO: u8 = 0;
/// RMII reference clock frequency the PHY needs.
pub const RMII_CLK_HZ: u32 = 50_000_000;
/// ESP32 crystal frequency in MHz (40 on every production board; the bench
/// unit's `espflash board-info` agrees).
pub const XTAL_FREQ_MHZ: u32 = 40;

/// IO_MUX function 1 on GPIO0 = `CLK_OUT1` (soc/io_mux_reg.h).
const FUNC_GPIO0_CLK_OUT1: u8 = 1;
/// `PIN_CTRL` clock-output selector value for the APLL (soc/clk_tree_defs.h).
const CLKOUT_SIG_APLL: u8 = 6;

const _: () = assert!(CLKOUT_SIG_APLL < 16, "PIN_CTRL.CLK_OUT1 is a 4-bit field");
const _: () = assert!(FUNC_GPIO0_CLK_OUT1 < 8, "IO_MUX MCU_SEL is a 3-bit field");

/// APLL tuning coefficients for a target output frequency:
/// `apll_freq = xtal * (4 + sdm2 + sdm1/256 + sdm0/65536) / ((o_div + 2) * 2)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApllCoeffs {
    pub o_div: u8,
    pub sdm0: u8,
    pub sdm1: u8,
    pub sdm2: u8,
    pub real_freq_hz: u32,
}

/// ESP-IDF's `rtc_clk_apll_coeff_calc` (the numerator must land in
/// 350–500 MHz), as supervictor's `board/poe_iso.rs` replicates it, with
/// its host tests. `None` when `freq_hz` is out of the APLL's reach.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn apll_coeffs(freq_hz: u32, xtal_freq_mhz: u32) -> Option<ApllCoeffs> {
    const MULTIPLIER_MIN_HZ: f64 = 350_000_000.0;
    const MULTIPLIER_MAX_HZ: f64 = 500_000_000.0;
    let xtal_hz = f64::from(xtal_freq_mhz) * 1_000_000.0;
    let freq = f64::from(freq_hz);

    let mut o_div = (MULTIPLIER_MIN_HZ / (freq * 2.0) + 1.0) as i32 - 2;
    if o_div > 31 {
        return None; // frequency too small
    }
    if o_div < 0 {
        o_div = (MULTIPLIER_MAX_HZ / (freq * 2.0)) as i32 - 2;
        if o_div < 0 {
            return None; // frequency too big
        }
    }

    // Integer division first, exactly like the C code.
    let denom = (o_div as u64 + 2) * 2;
    let mut sdm2 = ((denom * u64::from(freq_hz)) / (u64::from(xtal_freq_mhz) * 1_000_000)) as i32 - 4;
    let numerator = (denom as f64 * freq) / xtal_hz - 4.0 - f64::from(sdm2);

    let mut sdm0 = 0i32;
    let mut sdm1 = 0i32;
    if numerator > 1.0 - (1.0 / 65536.0) / 2.0 {
        sdm2 += 1;
    } else if numerator > (1.0 / 65536.0) / 2.0 {
        let scaled = (numerator * 65536.0 + 0.5) as i32;
        sdm1 = scaled / 256;
        sdm0 = scaled % 256;
    }

    let real_freq_hz = (xtal_hz
        * (4.0 + f64::from(sdm2) + f64::from(sdm1) / 256.0 + f64::from(sdm0) / 65536.0)
        / (denom as f64)) as u32;

    Some(ApllCoeffs {
        o_div: o_div as u8,
        sdm0: sdm0 as u8,
        sdm1: sdm1 as u8,
        sdm2: sdm2 as u8,
        real_freq_hz,
    })
}

/// The Ethernet MAC from the efuse base MAC: Espressif's universal-MAC
/// scheme gives the interfaces base+0 (WiFi STA), +1 (AP), +2 (BT), +3
/// (Ethernet) on the last octet.
#[must_use]
pub fn ethernet_mac(base: [u8; 6]) -> [u8; 6] {
    let mut mac = base;
    mac[5] = mac[5].wrapping_add(3);
    mac
}

// ROM regi2c accessors for the APLL's internal-I2C register file. The
// symbols are PROVIDEd by esp-rom-sys's esp32.rom.ld; signatures match
// ESP-IDF's esp_rom_regi2c.h.
unsafe extern "C" {
    fn rom_i2c_writeReg(block: u8, host_id: u8, reg_add: u8, data: u8);
    fn rom_i2c_writeReg_Mask(block: u8, host_id: u8, reg_add: u8, msb: u8, lsb: u8, data: u8);
    fn rom_i2c_readReg_Mask(block: u8, host_id: u8, reg_add: u8, msb: u8, lsb: u8) -> u8;
}

// Register map from ESP-IDF soc/regi2c_apll.h and hal/esp32/clk_tree_ll.h.
const I2C_APLL: u8 = 0x6D;
const I2C_APLL_HOSTID: u8 = 3;
const I2C_APLL_IR_CAL_DELAY: u8 = 0;
const I2C_APLL_OR_CAL_END: u8 = 3; // bit 7
const I2C_APLL_OR_OUTPUT_DIV: u8 = 4; // bits 4:0
const I2C_APLL_SDM_STOP: u8 = 5;
const I2C_APLL_DSDM2: u8 = 7; // bits 5:0
const I2C_APLL_DSDM1: u8 = 8;
const I2C_APLL_DSDM0: u8 = 9;
const APLL_SDM_STOP_VAL_1: u8 = 0x09;
const APLL_SDM_STOP_VAL_2_REV0: u8 = 0x69;
const APLL_SDM_STOP_VAL_2_REV1: u8 = 0x49;
const APLL_CAL_DELAY_1: u8 = 0x0f;
const APLL_CAL_DELAY_2: u8 = 0x3f;
const APLL_CAL_DELAY_3: u8 = 0x1f;

/// Power up the APLL at 50 MHz and route it out of GPIO0. Call before EMAC
/// init; the EMAC's internal-clock selector (set by ph-esp32-mac in
/// `Emac::init`) is what finally lets the MAC run from the same APLL.
pub fn enable_rmii_clk_out(delay: &mut esp_hal::delay::Delay) {
    enable_apll_50mhz(delay);
    route_apll_clk_out_gpio0();
}

/// Mirrors ESP-IDF `rtc_clk_apll_enable(true)` and `rtc_clk_apll_coeff_set`.
fn enable_apll_50mhz(delay: &mut esp_hal::delay::Delay) {
    let ApllCoeffs {
        o_div,
        mut sdm0,
        mut sdm1,
        sdm2,
        real_freq_hz: _,
    } = apll_coeffs(RMII_CLK_HZ, XTAL_FREQ_MHZ).expect("50 MHz is always reachable by the APLL");

    // Power up the APLL analog block.
    // SAFETY: the RTC_CNTL register block at its fixed address; a read-modify-
    // write of two bits nothing else touches during bring-up.
    let rtc_cntl = unsafe { &*esp32::RTC_CNTL::ptr() };
    rtc_cntl
        .ana_conf()
        .modify(|_, w| w.plla_force_pd().clear_bit().plla_force_pu().set_bit());

    // Revision-0 silicon cannot use the fractional SDM stages.
    let is_rev0 = esp_hal::efuse::Efuse::chip_revision() < 100;
    let sdm_stop_val_2 = if is_rev0 {
        sdm0 = 0;
        sdm1 = 0;
        APLL_SDM_STOP_VAL_2_REV0
    } else {
        APLL_SDM_STOP_VAL_2_REV1
    };

    // SAFETY: ROM routines with the register arguments ESP-IDF passes them.
    unsafe {
        rom_i2c_writeReg_Mask(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_DSDM2, 5, 0, sdm2);
        rom_i2c_writeReg_Mask(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_DSDM0, 7, 0, sdm0);
        rom_i2c_writeReg_Mask(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_DSDM1, 7, 0, sdm1);
        rom_i2c_writeReg(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_SDM_STOP, APLL_SDM_STOP_VAL_1);
        rom_i2c_writeReg(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_SDM_STOP, sdm_stop_val_2);
        rom_i2c_writeReg_Mask(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_OR_OUTPUT_DIV, 4, 0, o_div);

        // Calibration pulse train, then wait for OR_CAL_END.
        rom_i2c_writeReg(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_IR_CAL_DELAY, APLL_CAL_DELAY_1);
        rom_i2c_writeReg(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_IR_CAL_DELAY, APLL_CAL_DELAY_2);
        rom_i2c_writeReg(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_IR_CAL_DELAY, APLL_CAL_DELAY_3);
        while rom_i2c_readReg_Mask(I2C_APLL, I2C_APLL_HOSTID, I2C_APLL_OR_CAL_END, 7, 7) == 0 {
            delay.delay_micros(1);
        }
    }
}

/// GPIO0 as an output pad driven by clock-output channel 1, channel 1 fed by
/// the APLL. Mirrors ESP-IDF `esp_clock_output_start(CLKOUT_SIG_APLL, 0)`.
fn route_apll_clk_out_gpio0() {
    // SAFETY: the GPIO and IO_MUX register blocks at their fixed addresses;
    // the writes below touch pad 0 and clock-output channel 1 only.
    let gpio = unsafe { &*esp32::GPIO::ptr() };
    let io_mux = unsafe { &*esp32::IO_MUX::ptr() };
    // Output enable for pad 0 (GPIO_ENABLE_W1TS bit 0).
    gpio.enable_w1ts()
        .write(|w| unsafe { w.bits(1 << RMII_CLK_OUT_GPIO) });
    // Pad function: CLK_OUT1, input buffer off.
    io_mux
        .gpio0()
        .modify(|_, w| unsafe { w.mcu_sel().bits(FUNC_GPIO0_CLK_OUT1).fun_ie().clear_bit() });
    // Clock-output channel 1 source: APLL.
    io_mux
        .pin_ctrl()
        .modify(|_, w| unsafe { w.clk1().bits(CLKOUT_SIG_APLL) });
}
