//! Wired Ethernet on the P4: esp-hal 1.2's EMAC driver (RMII, external
//! reference clock from the IP101GRI) under embassy-net, the shape of
//! esp-hal's own `embassy_ethernet` example. Bring-up order: hold the PHY in
//! reset 100 ms, release it and give it 300 ms, build the driver (which
//! initialises the PHY over MDIO: reset, auto-negotiation advertisement),
//! switch to async (DMA interrupts), hand it to embassy-net with DHCP.
//!
//! The link poller: `GenericPhy` reads the PHY's status register on every
//! poll of the network driver, which is every packet. [`LinkPhy`] wraps it
//! so the register is read every 500 ms and the cached state is answered in
//! between, as the example does.

use core::future::Future;
use core::pin::Pin;
use core::task::Context;

use embassy_net::{Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_hal::Async;
use esp_hal::ethernet::clock::ExternalRefClock;
use esp_hal::ethernet::mac::{Duplex, LinkState, Speed};
use esp_hal::ethernet::phy::generic::GenericPhy;
use esp_hal::ethernet::phy::{MdioBus, Phy, PhyError};
use esp_hal::ethernet::{Error, Ethernet, EthernetDmaStorage, RmiiPinBundle};
use esp_hal::gpio::{Level, Output, OutputConfig};
use static_cell::{ConstStaticCell, StaticCell};

use crate::board::{self, EthPins};

/// DMA descriptors: frames the EMAC can hold each way before the stack
/// takes them (esp-hal's example uses 10 and 10).
pub const RX_FRAMES: usize = 10;
pub const TX_FRAMES: usize = 10;
/// Sockets the stack holds: DHCP, the TCP link (Modbus or the explicit
/// session), the UDP I/O socket, the UDP log, one spare.
pub const SOCKETS: usize = 5;
/// How often the PHY is asked about the link.
pub const LINK_POLL: Duration = Duration::from_millis(500);

/// The driver type the stack runs on.
pub type EthDriver = Ethernet<'static, Async, LinkPhy>;

static STORAGE: ConstStaticCell<EthernetDmaStorage<RX_FRAMES, TX_FRAMES>> =
    ConstStaticCell::new(EthernetDmaStorage::new());
static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

/// `GenericPhy` at the board's strapped address, polled on a timer.
pub struct LinkPhy {
    phy: GenericPhy,
    timer: Timer,
    cached: LinkState,
}

impl LinkPhy {
    #[must_use]
    pub fn new(addr: u8) -> Self {
        Self {
            phy: GenericPhy::new(addr),
            timer: Timer::after_ticks(0),
            cached: LinkState {
                up: false,
                speed: Speed::_100M,
                duplex: Duplex::Full,
            },
        }
    }
}

impl Phy for LinkPhy {
    fn address(&self) -> u8 {
        self.phy.address()
    }

    fn init<M: MdioBus>(&mut self, mdio: &mut M) -> Result<(), PhyError> {
        self.phy.init(mdio)
    }

    fn poll_link<M: MdioBus>(&mut self, mdio: &mut M, cx: Option<&mut Context<'_>>) -> LinkState {
        if let Some(cx) = cx {
            if Pin::new(&mut self.timer).poll(cx).is_pending() {
                return self.cached;
            }
            self.timer = Timer::after(LINK_POLL);
            // Arm the wake for the next read.
            let _ = Pin::new(&mut self.timer).poll(cx);
        }
        self.cached = self.phy.poll_link(mdio, None);
        self.cached
    }
}

/// Brings the PHY and the EMAC up and builds the embassy-net stack, DHCP
/// configured. `seed` feeds the stack's TCP sequence randomness. Returns
/// the stack and its runner (spawn [`net_task`] with it).
///
/// # Errors
/// The driver's init refusing: no PHY answering at the strapped address
/// means the reset line, the reference clock or the MDIO pins.
pub async fn init(pins: EthPins, seed: u64) -> Result<(Stack<'static>, Runner<'static, EthDriver>), Error> {
    let mut reset = Output::new(pins.phy_reset, Level::Low, OutputConfig::default());
    Timer::after(Duration::from_millis(100)).await;
    reset.set_high();
    Timer::after(Duration::from_millis(300)).await;
    // The handle goes out of scope below; the pad stays as driven: released.

    let eth = Ethernet::new(
        pins.eth,
        STORAGE.take(),
        board::mac_address(),
        LinkPhy::new(board::PHY_ADDR),
        RmiiPinBundle {
            clock: ExternalRefClock::new(pins.ref_clk),
            rxd0: pins.rxd0,
            rxd1: pins.rxd1,
            rx_dv: pins.rx_dv,
            txd0: pins.txd0,
            txd1: pins.txd1,
            tx_en: pins.tx_en,
            mdc: pins.mdc,
            mdio: pins.mdio,
        },
    )?
    .into_async();

    let config = embassy_net::Config::dhcpv4(embassy_net::DhcpConfig::default());
    Ok(embassy_net::new(
        eth,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    ))
}

/// Drives the embassy-net stack (packet rx/tx and the PHY poll).
#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, EthDriver>) -> ! {
    runner.run().await
}
