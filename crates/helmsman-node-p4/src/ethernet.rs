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

use embassy_net::{Runner, Stack, StackResources, StaticConfigV4};
use embassy_net_driver::{Capabilities, Checksum, Driver, HardwareAddress};
use embassy_time::{Duration, Timer};
use esp_hal::Async;
use esp_hal::ethernet::clock::ExternalRefClock;
use esp_hal::ethernet::mac::{Duplex, LinkState, Speed};
use esp_hal::ethernet::phy::generic::GenericPhy;
use esp_hal::ethernet::phy::{MdioBus, Phy, PhyError};
use esp_hal::ethernet::{Error, Ethernet, EthernetDmaStorage, RmiiPinBundle};
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::peripherals::EMAC_DMA;
use static_cell::{ConstStaticCell, StaticCell};

use crate::board::{self, EthPins};

/// DMA descriptors: frames the EMAC can hold each way before the stack
/// takes them (esp-hal's example uses 10 and 10).
pub const RX_FRAMES: usize = 10;
pub const TX_FRAMES: usize = 10;
/// Sockets the stack holds: DHCP, the TCP links (Modbus, the explicit
/// session), the UDP I/O socket, two HTTP connections, the beacon, the
/// UDP log, spare. smoltcp panics on a socket past this.
pub const SOCKETS: usize = 10;
/// How often the PHY is asked about the link.
pub const LINK_POLL: Duration = Duration::from_millis(500);

/// The driver type the stack runs on.
pub type EthDriver = CutThroughTx;

/// esp-hal's driver with transmit store-and-forward switched off and the
/// checksums of what the node sends left to the stack.
///
/// esp-hal 1.2.2 starts the P4's MAC with transmit store-and-forward on,
/// because its descriptors ask the MAC to insert the IP, TCP, UDP and ICMP
/// checksums and the MAC can only do that with the whole frame in its
/// transmit FIFO. The P4's FIFO does not hold a whole frame: on the bench
/// (Waveshare ESP32-P4-ETH, chip revision v3.2, 2026-11-10) every frame up
/// to 262 bytes went out and none from 272 bytes up, so ping and Modbus
/// worked and no HTTP reply ever left the node. Upstream `main` has since
/// made the same change (store-and-forward only with transmit checksum
/// offload, a per-chip property). Until a release carries it,
/// [`cut_through_tx`] clears the bit after the driver starts, and this
/// wrapper tells smoltcp to compute transmit checksums itself; without
/// store-and-forward the MAC's insertion is bypassed. Receive checksums
/// stay with the hardware, as esp-hal set them up.
pub struct CutThroughTx(Ethernet<'static, Async, LinkPhy>);

impl Driver for CutThroughTx {
    type RxToken<'a>
        = l2::TapRx<<Ethernet<'static, Async, LinkPhy> as Driver>::RxToken<'a>>
    where
        Self: 'a;
    type TxToken<'a>
        = <Ethernet<'static, Async, LinkPhy> as Driver>::TxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.send_raw(cx);
        Driver::receive(&mut self.0, cx).map(|(rx, tx)| (l2::TapRx(rx), tx))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        self.send_raw(cx);
        Driver::transmit(&mut self.0, cx)
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> embassy_net_driver::LinkState {
        Driver::link_state(&mut self.0, cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut caps = Driver::capabilities(&self.0);
        caps.checksum.ipv4 = Checksum::Tx;
        caps.checksum.tcp = Checksum::Tx;
        caps.checksum.udp = Checksum::Tx;
        caps.checksum.icmpv4 = Checksum::Tx;
        caps
    }

    fn hardware_address(&self) -> HardwareAddress {
        Driver::hardware_address(&self.0)
    }
}

impl CutThroughTx {
    /// Sends the raw frames [`l2::send`] queued, as far as the MAC has
    /// room, and leaves the stack's waker where `l2::send` finds it. The
    /// stack polls the driver whenever it is woken, so a queued frame
    /// goes out on the next poll, ahead of whatever the stack sends.
    fn send_raw(&mut self, cx: &mut Context<'_>) {
        l2::WAKER.register(cx.waker());
        while let Ok(frame) = l2::TX.try_receive() {
            // No room in the MAC: the frame is dropped. What goes this
            // way is cyclic, and the next one replaces it.
            let Some(tx) = Driver::transmit(&mut self.0, cx) else {
                break;
            };
            embassy_net_driver::TxToken::consume(tx, frame.len(), |buf| buf.copy_from_slice(&frame));
        }
    }
}

/// Raw Ethernet frames beside the IP stack: what a protocol below IP
/// (PROFINET's cyclic data, alarms and DCP, `EtherType` 0x8892) sends and
/// receives on the same MAC the stack runs on.
///
/// Received frames of the `EtherType` given to [`listen`] (plain, or
/// behind one 802.1Q tag) are copied into a queue as the stack takes
/// them off the MAC; the stack sees them too and drops them as a type it
/// does not know. Frames to send are queued and go out on the stack's
/// next poll of the driver, which [`send`] asks for.
pub mod l2 {
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicU16, Ordering};

    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::channel::Channel;
    use embassy_sync::waitqueue::AtomicWaker;

    /// Frames each way that may wait: at an 8 ms cycle, a burst of DCP or
    /// alarm frames beside the cyclic one.
    const QUEUE: usize = 8;

    pub(super) static TX: Channel<CriticalSectionRawMutex, Vec<u8>, QUEUE> = Channel::new();
    static RX: Channel<CriticalSectionRawMutex, Vec<u8>, QUEUE> = Channel::new();
    pub(super) static WAKER: AtomicWaker = AtomicWaker::new();
    /// The `EtherType` listened for; 0 is none.
    static ETHERTYPE: AtomicU16 = AtomicU16::new(0);

    /// Starts copying received frames of `ethertype` into the queue
    /// [`recv`] reads.
    pub fn listen(ethertype: u16) {
        ETHERTYPE.store(ethertype, Ordering::Relaxed);
    }

    /// Queues one complete frame (destination, source, type, payload).
    /// `false` when the queue is full and the frame was dropped.
    pub fn send(frame: Vec<u8>) -> bool {
        let queued = TX.try_send(frame).is_ok();
        WAKER.wake();
        queued
    }

    /// The next received frame of the listened `EtherType`.
    pub async fn recv() -> Vec<u8> {
        RX.receive().await
    }

    /// Whether `frame` carries `ethertype`, plain or behind one tag.
    fn wanted(frame: &[u8], ethertype: u16) -> bool {
        let word = |at: usize| frame.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        match word(12) {
            Some(t) if t == ethertype => true,
            Some(0x8100) => word(16) == Some(ethertype),
            _ => false,
        }
    }

    /// A receive token that copies a wanted frame aside before the stack
    /// has it.
    pub struct TapRx<T>(pub(super) T);

    impl<T: embassy_net_driver::RxToken> embassy_net_driver::RxToken for TapRx<T> {
        fn consume<R, F>(self, f: F) -> R
        where
            F: FnOnce(&mut [u8]) -> R,
        {
            self.0.consume(|buf| {
                let ethertype = ETHERTYPE.load(Ordering::Relaxed);
                if ethertype != 0 && wanted(buf, ethertype) {
                    // A full queue drops the frame: its reader is behind,
                    // and cyclic data is only worth its newest.
                    let _ = RX.try_send(buf.to_vec());
                }
                f(buf)
            })
        }
    }
}

/// Switches the MAC's transmit side from store-and-forward to cut-through
/// with a 64-byte threshold (see [`CutThroughTx`]). The transmit engine is
/// stopped around the change, which the mode bits require; nothing has
/// been queued yet when this runs.
fn cut_through_tx() {
    let mode = EMAC_DMA::regs().dmaoperation_mode();
    mode.modify(|_, w| w.start_stop_transmission_command().clear_bit());
    mode.modify(|_, w| unsafe { w.tx_str_fwd().clear_bit().tx_thresh_ctrl().bits(0) });
    mode.modify(|_, w| w.start_stop_transmission_command().set_bit());
}

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

/// How the node gets its address: a lease, or a fixed one (most plants
/// commission controllers on static networks with no DHCP server; Nick,
/// 2026-11-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addressing {
    Dhcp,
    Static(StaticConfigV4),
}

/// Brings the PHY and the EMAC up and builds the embassy-net stack with
/// `addressing`. `seed` feeds the stack's TCP sequence randomness. Returns
/// the stack and its runner (spawn [`net_task`] with it).
///
/// # Errors
/// The driver's init refusing: no PHY answering at the strapped address
/// means the reset line, the reference clock or the MDIO pins.
pub async fn init(
    pins: EthPins,
    addressing: Addressing,
    seed: u64,
) -> Result<(Stack<'static>, Runner<'static, EthDriver>), Error> {
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
    cut_through_tx();
    let eth = CutThroughTx(eth);

    let config = match addressing {
        Addressing::Dhcp => embassy_net::Config::dhcpv4(embassy_net::DhcpConfig::default()),
        Addressing::Static(fixed) => embassy_net::Config::ipv4_static(fixed),
    };
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
