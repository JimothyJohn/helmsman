//! The edge spike's Phase 2 firmware (docs/EDGE.md, G4 and G5): a 10 ms scan
//! on the ESP32-POE2 mastering one remote block over Modbus TCP. Each scan
//! reads the E1213's inputs (FC 2, one round trip), runs an embedded ST
//! program against them and writes its outputs back (FC 15, one round
//! trip). Late scans, cycle times, link errors and reconnects are counted
//! and reported every 10 s over serial and as UDP lines to a log sink on
//! the Studio (scripts/node-log-sink.py), since on PoE there is no USB.
//!
//! Run order, because the board has no galvanic isolation: flash over USB
//! with Ethernet unplugged, unplug USB, then plug PoE. Never both.
//!
//! G5 (the block goes safe when the node's cable is pulled) is the block's
//! own comms-loss setting, not this firmware's doing: set it on the E1213
//! first (its Modbus watchdog and DO safe mode), then yank and watch.

#![no_std]
#![no_main]

extern crate alloc;

use core::time::Duration as CoreDuration;

use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use helmsman_iec::{Image, ImageSizes, compile};
use helmsman_node::{ethernet, modbus};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!(
    env!("CARGO_PKG_VERSION"),
    env!("CARGO_PKG_NAME"),
    esp_bootloader_esp_idf::BUILD_TIME,
    esp_bootloader_esp_idf::BUILD_DATE,
    esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
    esp_bootloader_esp_idf::MMU_PAGE_SIZE,
    0,
    u16::MAX,
    esp_bootloader_esp_idf::SECURE_VERSION
);

/// Internal heap in the main DRAM segment, which the stack shares: at
/// 160 KiB the stack was 1.7 KiB and the node panicked on its stack guard
/// right after the PHY came up (2026-11-03). 112 KiB leaves about 50 KiB;
/// the chase program and the sockets need far less heap than that.
/// `scripts/node-build.sh` refuses an image with less than 32 KiB.
const HEAP_SIZE: usize = 112 * 1024;
const DRAM2_HEAP_SIZE: usize = 96 * 1024;

/// The remote block: the bench E1213 unless `E1213_ADDR` says otherwise at
/// build time (`E1213_ADDR=10.0.0.60 make node-build net`).
const E1213_ADDR: &str = match option_env!("E1213_ADDR") {
    Some(a) => a,
    None => "10.0.0.57",
};
const E1213_PORT: u16 = 502;
const UNIT: u8 = 1;
/// Where the report lines go: the Studio's log sink (`LOG_SINK=host:port`).
const LOG_SINK: &str = match option_env!("LOG_SINK") {
    Some(a) => a,
    None => "10.0.0.41:9514",
};
/// A fixed address instead of DHCP: `NODE_STATIC_IP=192.168.3.55/24` (and
/// `NODE_GATEWAY=192.168.3.1` if the sink or the block is off the subnet)
/// at build time. Unset: DHCP. The settings record (docs/EDGE.md Phase 4)
/// replaces this.
const STATIC_IP: Option<&str> = option_env!("NODE_STATIC_IP");
const GATEWAY: Option<&str> = option_env!("NODE_GATEWAY");

const SCAN: Duration = Duration::from_millis(10);
/// A Modbus request that takes longer than this is a link error.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
const REPORT_EVERY: Duration = Duration::from_secs(10);
const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

/// The program: a one-of-three chase on DO-00..02 every 500 ms, the inputs
/// read as DI-00..02 (wired back from the outputs on the bench).
const PROGRAM: &str = "
PROGRAM chase
VAR
  tick : TON;
  step : INT;
  di0 AT %IX0.0 : BOOL;
  di1 AT %IX0.1 : BOOL;
  di2 AT %IX0.2 : BOOL;
  do0 AT %QX0.0 : BOOL;
  do1 AT %QX0.1 : BOOL;
  do2 AT %QX0.2 : BOOL;
  seen AT %QX1.0 : BOOL;
END_VAR
  tick(IN := NOT tick.Q, PT := T#500ms);
  IF tick.Q THEN
    step := step + 1;
    IF step > 2 THEN step := 0; END_IF;
  END_IF;
  do0 := step = 0;
  do1 := step = 1;
  do2 := step = 2;
  seen := (di0 = do0) AND (di1 = do1) AND (di2 = do2);
END_PROGRAM
";

// The sockets' buffers, in statics (DRAM) rather than on the task's stack.
static UDP_RX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static UDP_TX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static UDP_RX: StaticCell<[u8; 256]> = StaticCell::new();
static UDP_TX: StaticCell<[u8; 512]> = StaticCell::new();
static TCP_RX: StaticCell<[u8; 512]> = StaticCell::new();
static TCP_TX: StaticCell<[u8; 512]> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: DRAM2_HEAP_SIZE);
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    println!();
    println!(
        "helmsman-node modbus ({} {}) on the {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        helmsman_node::board::BOARD
    );
    println!(
        "block {E1213_ADDR}:{E1213_PORT} unit {UNIT}; log sink {LOG_SINK}; scan {} ms",
        SCAN.as_millis()
    );

    let _trng = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());

    esp_rtos::start(timg0.timer0);

    let phy_power = Output::new(peripherals.GPIO12, Level::Low, OutputConfig::default());
    let (stack, runner, emac_ptr) = match ethernet::init(phy_power, seed) {
        Ok(parts) => parts,
        Err(e) => {
            println!("FATAL: ethernet init failed: {e:?}");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    spawner.spawn(ethernet::eth_net_task(runner)).ok();
    spawner.spawn(ethernet::eth_link_task(emac_ptr)).ok();
    // The DNS list is heapless's Vec, which this crate does not name.
    #[allow(clippy::default_trait_access)]
    let addressing = match STATIC_IP {
        None => ethernet::Addressing::Dhcp,
        Some(cidr) => match parse_cidr(cidr) {
            Some(address) => ethernet::Addressing::Static(StaticConfigV4 {
                address,
                gateway: GATEWAY.and_then(parse_ipv4),
                dns_servers: Default::default(),
            }),
            None => {
                println!("FATAL: NODE_STATIC_IP {cidr:?} is not a.b.c.d/nn");
                loop {
                    Timer::after(Duration::from_secs(60)).await;
                }
            }
        },
    };
    spawner
        .spawn(ethernet::eth_dhcp_task(stack, emac_ptr, addressing))
        .ok();

    println!("waiting for the link and a lease...");
    stack.wait_config_up().await;

    let Some(block) = parse_endpoint(E1213_ADDR, E1213_PORT) else {
        println!("FATAL: E1213_ADDR {E1213_ADDR:?} is not an IPv4 address");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    };
    let sink = parse_endpoint_port(LOG_SINK);

    // The UDP log socket: small, one datagram at a time.
    let udp_rx_meta = UDP_RX_META.init([PacketMetadata::EMPTY; 2]);
    let udp_tx_meta = UDP_TX_META.init([PacketMetadata::EMPTY; 2]);
    let udp_rx = UDP_RX.init([0; 256]);
    let udp_tx = UDP_TX.init([0; 512]);
    let mut udp = UdpSocket::new(stack, udp_rx_meta, udp_rx, udp_tx_meta, udp_tx);
    if udp.bind(0).is_err() {
        println!("udp: bind failed; reports go to serial only");
    }

    let rx = TCP_RX.init([0; 512]);
    let tx = TCP_TX.init([0; 512]);
    let mut client = modbus::Client::new(stack, block, UNIT, REQUEST_TIMEOUT, rx, tx);

    let mut program = match compile(PROGRAM, SIZES) {
        Ok(p) => p,
        Err(diags) => {
            for d in diags {
                println!("compile error {}: {}", d.pos, d.msg);
            }
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    let mut image = Image::new(SIZES);

    let mut stats = Stats::default();
    let start = Instant::now();
    let mut last_report = start;
    let mut ticker = Ticker::every(SCAN);
    let mut deadline = start + SCAN;
    report(
        &mut udp,
        sink,
        &alloc::format!(
            "start: block {E1213_ADDR}:{E1213_PORT}, scan {} ms",
            SCAN.as_millis()
        ),
    )
    .await;

    loop {
        ticker.next().await;
        let t0 = Instant::now();
        // A scan that starts after its deadline is late (the previous one
        // overran, or the executor was busy).
        if t0 > deadline + SCAN {
            stats.late += 1;
        }
        deadline += SCAN;

        // Inputs: DI-00..07 in one round trip.
        match client.read_discrete_inputs(0, 8).await {
            Ok(bits) => {
                let mut b = 0u8;
                for (i, on) in bits.iter().enumerate() {
                    if *on {
                        b |= 1 << i;
                    }
                }
                image.i[0] = b;
                stats.ok_reads += 1;
            }
            Err(e) => {
                stats.errors += 1;
                stats.last_error = Some(e);
            }
        }

        // The scan, with the node's own clock.
        let now = CoreDuration::from_micros(start.elapsed().as_micros());
        if let Err(fault) = program.scan_at(&mut image, now) {
            println!("fault: {fault}");
            report(&mut udp, sink, &alloc::format!("fault: {fault}")).await;
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
        if image.q[1] & 1 == 1 {
            stats.matched += 1;
        } else {
            stats.mismatched += 1;
        }

        // Outputs: DO-00..03 in one round trip (DO-03 stays off).
        let q = image.q[0];
        let coils = [q & 1 == 1, q & 2 == 2, q & 4 == 4, false];
        match client.write_coils(0, &coils).await {
            Ok(()) => stats.ok_writes += 1,
            Err(e) => {
                stats.errors += 1;
                stats.last_error = Some(e);
            }
        }

        let cycle = t0.elapsed();
        stats.scans += 1;
        stats.cycle_max = stats.cycle_max.max(cycle);
        stats.cycle_sum += cycle.as_micros();
        if cycle > SCAN {
            stats.over += 1;
        }

        if last_report.elapsed() >= REPORT_EVERY {
            last_report = Instant::now();
            let line = stats.line(start.elapsed(), client.connects);
            println!("{line}");
            report(&mut udp, sink, &line).await;
            stats.cycle_max = Duration::from_ticks(0);
        }
    }
}

/// What the run has done so far.
#[derive(Default)]
struct Stats {
    scans: u64,
    /// Scans that started more than one period after their deadline.
    late: u64,
    /// Scans whose own work took longer than the period.
    over: u64,
    ok_reads: u64,
    ok_writes: u64,
    errors: u64,
    /// Scans where the inputs matched the outputs (after the block's
    /// 100 ms filter the steady state) or did not.
    matched: u64,
    mismatched: u64,
    cycle_max: Duration,
    cycle_sum: u64,
    last_error: Option<modbus::Error>,
}

impl Stats {
    fn line(&self, up: Duration, connects: u32) -> alloc::string::String {
        let mean = self.cycle_sum.checked_div(self.scans).unwrap_or(0);
        #[allow(clippy::cast_precision_loss)]
        let late_pct = if self.scans == 0 {
            0.0
        } else {
            self.late as f64 * 100.0 / self.scans as f64
        };
        alloc::format!(
            "up {}s scans {} late {} ({late_pct:.3}%) over {} cycle mean {} us max {} us reads {} writes {} errors {} connects {} match {} mismatch {} heap used {} B peak {} B{}",
            up.as_secs(),
            self.scans,
            self.late,
            self.over,
            mean,
            self.cycle_max.as_micros(),
            self.ok_reads,
            self.ok_writes,
            self.errors,
            connects,
            self.matched,
            self.mismatched,
            esp_alloc::HEAP.used(),
            esp_alloc::HEAP.stats().max_usage,
            match &self.last_error {
                Some(e) => alloc::format!(" last error: {e}"),
                None => alloc::string::String::new(),
            }
        )
    }
}

/// One line to the sink, best effort.
async fn report(udp: &mut UdpSocket<'_>, sink: Option<IpEndpoint>, line: &str) {
    if let Some(to) = sink {
        let _ = udp.send_to(line.as_bytes(), to).await;
    }
}

/// `a.b.c.d`.
fn parse_ipv4(addr: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in addr.trim().split('.') {
        if n == 4 {
            return None;
        }
        octets[n] = part.parse().ok()?;
        n += 1;
    }
    (n == 4).then(|| Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
}

/// `a.b.c.d/nn`.
fn parse_cidr(s: &str) -> Option<Ipv4Cidr> {
    let (ip, prefix) = s.split_once('/')?;
    let prefix: u8 = prefix.parse().ok().filter(|p| *p <= 32)?;
    Some(Ipv4Cidr::new(parse_ipv4(ip)?, prefix))
}

/// `a.b.c.d` with `port`.
fn parse_endpoint(addr: &str, port: u16) -> Option<IpEndpoint> {
    parse_ipv4(addr).map(|ip| IpEndpoint::new(IpAddress::Ipv4(ip), port))
}

/// `a.b.c.d:port`.
fn parse_endpoint_port(s: &str) -> Option<IpEndpoint> {
    let (host, port) = s.rsplit_once(':')?;
    parse_endpoint(host, port.parse().ok()?)
}
