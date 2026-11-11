//! The P4 node as a PROFINET IO controller for one device: it finds the
//! station by name, connects with the modules the build names, exchanges
//! cyclic frames and reports what it sees. Outputs stay zero: this image
//! proves the connection, it drives nothing.
//!
//! Build settings (environment, at build time), the same text as a box's
//! `profinet-io` device line:
//!
//! - `PN_STATION` (default `mdrive`): the device's station name.
//! - `PN_SLOTS` (default the Novanta `MDrive`'s, docs/devices/novanta-mdrive.md):
//!   `SLOT:MODULE/SUBSLOT:SUBMODULE[:iN][:oN],...`.
//! - `PN_IP=a.b.c.d/nn`: the address to give the station when its own
//!   differs; left out, the station must have one.
//! - `PN_CYCLE_MS` (default 8), `LOG_SINK=host:port`, `NODE_STATIC_IP`.
//!
//! Every 10 s, and on every change of state, a line goes to serial and
//! to the log sink (scripts/node-log-sink.py): the startup's state or why
//! it failed, how many startups there were, the first input bytes.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_time::{Duration, Instant, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use helmsman_node_p4::report::{parse_cidr, parse_endpoint_port, send_line};
use helmsman_node_p4::{board, ethernet, profinet};
use helmsman_profinet::controller::{Address, Controller, DeviceConfig, Local, State};
use helmsman_profinet::{rpc, text};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const HEAP_SIZE: usize = 256 * 1024;

const STATION: &str = match option_env!("PN_STATION") {
    Some(s) => s,
    None => "mdrive",
};
const SLOTS: &str = match option_env!("PN_SLOTS") {
    Some(s) => s,
    None => "0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128",
};
const CYCLE_MS: &str = match option_env!("PN_CYCLE_MS") {
    Some(s) => s,
    None => "8",
};
const LOG_SINK: &str = match option_env!("LOG_SINK") {
    Some(a) => a,
    None => "192.168.3.1:9514",
};
const REPORT_EVERY: Duration = Duration::from_secs(10);
/// Input bytes shown in a report line.
const SHOWN: usize = 16;

static LOG_RX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static LOG_TX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static LOG_RX: StaticCell<[u8; 256]> = StaticCell::new();
static LOG_TX: StaticCell<[u8; 512]> = StaticCell::new();
static RPC_RX_META: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static RPC_TX_META: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static RPC_RX: StaticCell<[u8; 3200]> = StaticCell::new();
static RPC_TX: StaticCell<[u8; 3200]> = StaticCell::new();

async fn halt(reason: &str) -> ! {
    println!("FATAL: {reason}");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

/// The device as the build describes it.
fn device() -> Result<DeviceConfig, String> {
    let mut cfg = DeviceConfig::new(STATION, text::slots(SLOTS)?);
    cfg.reduction_ratio =
        text::number16(CYCLE_MS).ok_or_else(|| format!("PN_CYCLE_MS={CYCLE_MS}: not a number"))?;
    if let Some(ip) = option_env!("PN_IP") {
        let cidr = parse_cidr(ip).ok_or_else(|| format!("PN_IP={ip}: want a.b.c.d/nn"))?;
        cfg.address = Some(Address {
            ip: cidr.address().octets(),
            mask: cidr.netmask().octets(),
            gateway: [0; 4],
        });
    }
    Ok(cfg)
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Identifying => "looking for the station",
        State::Addressing => "giving it its address",
        State::Connecting => "connecting",
        State::Parameterising => "writing parameters",
        State::WaitingReady => "waiting for application ready",
        State::Running => "running",
        State::Backoff => "waiting to try again",
    }
}

fn line(controller: &Controller) -> String {
    let mut s = format!(
        "profinet {STATION}: {} (start {})",
        state_name(controller.state()),
        controller.starts()
    );
    if let Some(e) = controller.error() {
        s.push_str(": ");
        s.push_str(e);
    }
    if let Some(inputs) = controller.inputs() {
        s.push_str("; in");
        for b in inputs.iter().take(SHOWN) {
            let _ = write!(s, " {b:02x}");
        }
    }
    s
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    println!();
    println!(
        "helmsman-node-p4 profinet ({} {}) on the {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        board::BOARD
    );
    println!("station {STATION}; slots {SLOTS}; a frame every {CYCLE_MS} ms; log sink {LOG_SINK}");

    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let mac = board::mac_address();
    let config = match device() {
        Ok(c) => c,
        Err(e) => halt(&e).await,
    };
    let local = Local {
        mac,
        station_name: "helmsman-node".into(),
        vendor_id: 0xFFFF,
        device_id: 0x0002,
    };
    let mut controller = match Controller::new(local, config, seed) {
        Ok(c) => c,
        Err(e) => halt(&e).await,
    };

    let pins = board::EthPins {
        eth: peripherals.ETH,
        ref_clk: peripherals.GPIO50,
        rxd0: peripherals.GPIO29,
        rxd1: peripherals.GPIO30,
        rx_dv: peripherals.GPIO28,
        txd0: peripherals.GPIO34,
        txd1: peripherals.GPIO35,
        tx_en: peripherals.GPIO49,
        mdc: peripherals.GPIO31,
        mdio: peripherals.GPIO52,
        phy_reset: peripherals.GPIO51,
    };
    let addressing = match helmsman_node_p4::report::build_addressing(
        option_env!("NODE_STATIC_IP"),
        option_env!("NODE_GATEWAY"),
    ) {
        Ok(a) => a,
        Err(e) => halt(e).await,
    };
    let (stack, runner) = match ethernet::init(pins, addressing, seed).await {
        Ok(parts) => parts,
        Err(e) => halt(&format!("ethernet init failed: {e:?}")).await,
    };
    match ethernet::net_task(runner) {
        Ok(task) => spawner.spawn(task),
        Err(_) => halt("the network task would not spawn").await,
    }
    println!("waiting for the link and a lease...");
    stack.wait_config_up().await;
    stack.wait_link_up().await;
    if let Some(cfg) = stack.config_v4() {
        println!("lease {}", cfg.address);
    }
    let sink = parse_endpoint_port(LOG_SINK);

    let mut log = UdpSocket::new(
        stack,
        LOG_RX_META.init([PacketMetadata::EMPTY; 2]),
        LOG_RX.init([0; 256]),
        LOG_TX_META.init([PacketMetadata::EMPTY; 2]),
        LOG_TX.init([0; 512]),
    );
    if log.bind(0).is_err() {
        println!("log: udp bind failed; reports go to serial only");
    }
    // The context-manager port: the device answers calls here and sends
    // its own ("application ready") to it.
    let mut udp = UdpSocket::new(
        stack,
        RPC_RX_META.init([PacketMetadata::EMPTY; 4]),
        RPC_RX.init([0; 3200]),
        RPC_TX_META.init([PacketMetadata::EMPTY; 4]),
        RPC_TX.init([0; 3200]),
    );
    if udp.bind(rpc::PORT).is_err() {
        halt("udp 34964 bind failed").await;
    }
    profinet::listen();

    let mut buf = [0u8; 1600];
    let mut out = Vec::new();
    let mut last = String::new();
    let mut last_report = Instant::now();
    loop {
        profinet::step(&mut controller, &mut udp, &mut buf, &mut out).await;
        // A change of state or of error is reported at once; the input
        // bytes only with the periodic line.
        let headline = format!("{:?} {:?}", controller.state(), controller.error());
        let due = last_report.elapsed() >= REPORT_EVERY;
        if headline != last || due {
            let text = line(&controller);
            println!("{text}");
            send_line(&mut log, sink, &text).await;
            last = headline;
            last_report = Instant::now();
        }
    }
}
