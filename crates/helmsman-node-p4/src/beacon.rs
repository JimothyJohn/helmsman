//! The site beacon: the box's six lines (`crates/helmsman-plc/src/site.rs`)
//! every 5 s to 239.255.72.77:41910 with a hop limit of 1, so a canvas on
//! the LAN lists the node beside the boxes. Off when the settings name no
//! site. A node only sends: it never reads the group.

use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::{Duration, Timer};
use helmsman_nodecore::api;
use helmsman_nodecore::settings::Settings;
use static_cell::StaticCell;

use crate::state::{self, Shared};

pub const GROUP: Ipv4Address = Ipv4Address::new(239, 255, 72, 77);
pub const PORT: u16 = 41910;
pub const EVERY: Duration = Duration::from_secs(5);

static RX_META: StaticCell<[PacketMetadata; 1]> = StaticCell::new();
static TX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static RX: StaticCell<[u8; 64]> = StaticCell::new();
static TX: StaticCell<[u8; 512]> = StaticCell::new();

/// Sends the beacon for the life of the node.
#[embassy_executor::task]
pub async fn beacon_task(
    stack: Stack<'static>,
    settings: &'static Settings,
    shared: &'static Shared,
    mac: [u8; 6],
    board: &'static str,
    heap_size: usize,
    scan_ms: f64,
) -> ! {
    let mut udp = UdpSocket::new(
        stack,
        RX_META.init([PacketMetadata::EMPTY; 1]),
        RX.init([0; 64]),
        TX_META.init([PacketMetadata::EMPTY; 2]),
        TX.init([0; 512]),
    );
    udp.set_hop_limit(Some(1));
    let _ = udp.bind(0);
    let to = IpEndpoint::new(IpAddress::Ipv4(GROUP), PORT);
    loop {
        let view = state::view(shared, mac, board, heap_size, scan_ms);
        let text = api::beacon(&view, settings);
        let _ = udp.send_to(text.as_bytes(), to).await;
        Timer::after(EVERY).await;
    }
}
