//! Wired Ethernet on the POE2 (EMAC + LAN8720A over RMII), copied from
//! supervictor's `network/ethernet.rs` (2026-11-03) with the board module
//! beside it; the recipe is `ph-esp32-mac`'s hardware-verified embassy-net
//! example. Bring-up order matters (the PHY needs the 50 MHz reference clock
//! and power before it answers on MDIO):
//!
//! 1. APLL → 50 MHz → clock-out pad ([`crate::board::enable_rmii_clk_out`])
//! 2. PHY power on GPIO12, then a settle delay
//! 3. EMAC init (also sets the internal-clock selector, so the reference
//!    clock reaches the pin)
//! 4. PHY soft-reset + auto-negotiation over MDIO
//! 5. EMAC start + ISR bind, the embassy-net stack on top
//!
//! DHCP starts only after link-up, with promiscuous mode held until a lease
//! is in (MAC filtering during DHCP was unreliable on real hardware).

use embassy_net::{Config, ConfigV4, DhcpConfig, Runner, Stack, StaticConfigV4};
use embassy_time::{Duration, Timer};
use esp_hal::delay::Delay;
use esp_hal::gpio::Output;
use esp_println::println;
use ph_esp32_mac::esp_hal::{EmacBuilder, EmacExt, EmacPhyBundle, Priority};
use ph_esp32_mac::hal::MdioController;
use ph_esp32_mac::{Emac, EmacConfig, EmbassyEmac, Lan8720a, PhyInterface, RmiiClockMode};

use crate::board;

/// DMA frame ring sizes (supervictor's figures for the same chip).
pub const EMAC_RX_FRAMES: usize = 8;
pub const EMAC_TX_FRAMES: usize = 4;
pub const EMAC_FRAME_BUF_SIZE: usize = 1600;
/// How often the PHY is asked about the link.
pub const ETH_LINK_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// The LAN8720A latches its straps on power-on and needs this before MDIO.
pub const PHY_POWER_STABLE_DELAY_MS: u32 = 100;
/// Sockets the stack holds: DHCP, the Modbus TCP link, two HTTP
/// connections, the beacon, the UDP log, two spare. smoltcp panics on a
/// socket past this ("adding a socket to a full SocketSet": the first
/// commissioned boot, 2026-11-08, with a site set and four slots).
pub const SOCKETS: usize = 6;

/// EMAC instance shape used throughout this module.
pub type EthEmac = Emac<EMAC_RX_FRAMES, EMAC_TX_FRAMES, EMAC_FRAME_BUF_SIZE>;
/// embassy-net driver over the EMAC.
pub type EthDriver = EmbassyEmac<'static, EMAC_RX_FRAMES, EMAC_TX_FRAMES, EMAC_FRAME_BUF_SIZE>;

// EMAC hardware instance (in DRAM for DMA), shared ISR/link state, and the
// embassy-net stack resources.
ph_esp32_mac::embassy_net_statics!(
    EMAC,
    EMAC_STATE,
    NET_RESOURCES,
    EMAC_RX_FRAMES,
    EMAC_TX_FRAMES,
    EMAC_FRAME_BUF_SIZE,
    SOCKETS
);

// EMAC interrupt: wake the embassy-net driver on RX/TX/error events.
ph_esp32_mac::emac_isr!(EMAC_IRQ, Priority::Priority1, {
    EMAC_STATE.handle_interrupt();
});

/// Brings up the EMAC and the PHY and builds the embassy-net stack.
///
/// `phy_power` is the GPIO12 output, still low; `seed` feeds embassy-net's
/// TCP sequence randomness. Returns the stack, its runner (spawn
/// [`eth_net_task`] with it) and the EMAC pointer for the link and DHCP
/// tasks.
///
/// # Errors
/// The EMAC's or the PHY's init refusing (no PHY answering on MDIO means
/// the clock or the power is not there).
pub fn init(
    mut phy_power: Output<'static>,
    seed: u64,
) -> Result<(Stack<'static>, Runner<'static, EthDriver>, *mut EthEmac), ph_esp32_mac::Error> {
    let mut delay = Delay::new();

    // 1. 50 MHz reference clock plumbing (APLL + the board's clock-out pad).
    board::enable_rmii_clk_out(&mut delay);

    // 2. PHY power, then its settle window.
    phy_power.set_high();
    delay.delay_millis(PHY_POWER_STABLE_DELAY_MS);

    // 3. EMAC init. The efuse base MAC is factory-programmed on every ESP32.
    let mac = board::ethernet_mac(esp_hal::efuse::Efuse::mac_address());
    let config = EmacConfig::rmii_esp32_default()
        .with_phy_interface(PhyInterface::Rmii)
        .with_rmii_clock(RmiiClockMode::InternalOutput {
            gpio: board::RMII_CLK_OUT_GPIO,
        })
        .with_mac_address(mac);

    let emac_ptr = EMAC.init(Emac::new()) as *mut EthEmac;
    // SAFETY: emac_ptr points to static storage valid for the program lifetime.
    let emac = unsafe { &mut *emac_ptr };
    EmacBuilder::new(emac).with_config(config).init(&mut delay)?;

    // 4. PHY init (soft reset + auto-negotiation). The PHY id on the console
    //    makes a wiring or strap problem visible.
    {
        // SAFETY: same static EMAC; the builder borrow above has ended.
        let emac = unsafe { &mut *emac_ptr };
        let mut bundle = EmacPhyBundle::new(
            emac,
            Lan8720a::new(board::PHY_ADDR),
            MdioController::new(Delay::new()),
        );
        bundle.init_phy()?;
        let (_, phy, mut mdio) = bundle.into_parts();
        use ph_esp32_mac::PhyDriver;
        match phy.phy_id(&mut mdio) {
            // LAN8710A/LAN8720A family reads 0x0007C0Fx.
            Ok(id) => println!("ethernet: PHY at addr {}: id {:#010x}", board::PHY_ADDR, id),
            Err(e) => println!("ethernet: PHY id read failed: {:?}", e),
        }
    }

    // 5. Start the MAC and bind the interrupt.
    // SAFETY: same static EMAC; prior borrows have ended.
    let emac = unsafe { &mut *emac_ptr };
    emac.start()?;
    emac.bind_interrupt(EMAC_IRQ);

    // 6. The stack, unconfigured: eth_dhcp_task applies IPv4 once the link
    //    is up.
    let driver = ph_esp32_mac::embassy_net_driver!(emac_ptr, &EMAC_STATE);
    let (stack, runner) = ph_esp32_mac::embassy_net_stack!(driver, NET_RESOURCES, Config::default(), seed);

    println!(
        "ethernet: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    Ok((stack, runner, emac_ptr))
}

/// Drives the embassy-net stack (packet rx/tx).
#[embassy_executor::task]
pub async fn eth_net_task(mut runner: Runner<'static, EthDriver>) -> ! {
    println!("ethernet: net task running");
    runner.run().await
}

/// Polls the PHY for link changes; keeps the driver's link state and the
/// MAC's speed/duplex in step with auto-negotiation.
#[embassy_executor::task]
pub async fn eth_link_task(emac_ptr: *mut EthEmac) -> ! {
    println!("ethernet: link task running");
    let mut mdio = MdioController::new(Delay::new());
    let mut phy = Lan8720a::new(board::PHY_ADDR);
    let mut was_up = false;
    let mut polls: u32 = 0;

    loop {
        polls += 1;
        match EMAC_STATE.update_link_from_phy(&mut phy, &mut mdio) {
            Ok(Some(status)) => {
                if !was_up {
                    println!("ethernet: link up {:?} {:?}", status.speed, status.duplex);
                    // SAFETY: EMAC is static; this is the only task touching
                    // speed/duplex after init.
                    unsafe {
                        let emac = &mut *emac_ptr;
                        emac.set_speed(status.speed);
                        emac.set_duplex(status.duplex);
                    }
                    was_up = true;
                }
            }
            Ok(None) => {
                if was_up {
                    println!("ethernet: link down");
                    was_up = false;
                } else if polls.is_multiple_of(10) {
                    // A stall here with the cable in and the LED on is worth
                    // the PHY's own word (2026-11-08: a build stalled at
                    // "waiting for the link" after the socket set grew).
                    use ph_esp32_mac::PhyDriver;
                    let id = phy.phy_id(&mut mdio);
                    println!(
                        "ethernet: no link yet after {} polls; PHY id reads {:?}",
                        polls, id
                    );
                }
            }
            Err(e) => println!("ethernet: PHY poll error {:?}", e),
        }
        Timer::after(ETH_LINK_POLL_INTERVAL).await;
    }
}

/// How the node gets its address: a lease, or a fixed one (most plants
/// commission controllers on static networks with no DHCP server; Nick,
/// 2026-11-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressing {
    Dhcp,
    Static(StaticConfigV4),
}

/// Applies the address once the link is up: a static one at once, or
/// DHCP, re-armed if the lease is lost.
#[embassy_executor::task]
pub async fn eth_dhcp_task(stack: Stack<'static>, emac_ptr: *mut EthEmac, addressing: Addressing) -> ! {
    let mut promisc = false;
    loop {
        stack.wait_link_up().await;
        if let Addressing::Static(fixed) = &addressing {
            println!("ethernet: link up, static {}", fixed.address);
            stack.set_config_v4(ConfigV4::Static(fixed.clone()));
            stack.wait_link_down().await;
            println!("ethernet: link down");
            continue;
        }
        println!("ethernet: link up, starting DHCP");
        Timer::after(Duration::from_secs(1)).await;

        if !promisc {
            // SAFETY: EMAC is static; only this task toggles promiscuous mode.
            unsafe { (*emac_ptr).set_promiscuous(true) };
            promisc = true;
        }

        stack.set_config_v4(ConfigV4::Dhcp(DhcpConfig::default()));

        loop {
            if let Some(config) = stack.config_v4() {
                println!("ethernet: DHCP {}", config.address);
                if promisc {
                    // SAFETY: as above.
                    unsafe { (*emac_ptr).set_promiscuous(false) };
                    promisc = false;
                }
                break;
            }
            Timer::after(Duration::from_secs(1)).await;
        }

        stack.wait_config_down().await;
        println!("ethernet: DHCP lost, waiting for the link");
    }
}
