//! The P4 node's Modbus TCP firmware (docs/EDGE.md Phase 2 on the P4): a
//! 10 ms scan mastering one remote block over Modbus TCP. Each scan
//! reads the E1213's inputs (FC 2, one round trip), runs an embedded ST
//! program against them and writes its outputs back (FC 15, one round
//! trip). Late scans, cycle times, link errors and reconnects are counted
//! and reported every 10 s over serial and as UDP lines to a log sink on
//! the Studio (scripts/node-log-sink.py), since on power over Ethernet there is no USB.
//!
//! Power rule, until the power-over-Ethernet module's isolation is known:
//! flash over USB with Ethernet unplugged, unplug USB, then plug the
//! Ethernet cable. Never both.
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
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use helmsman_iec::{Image, ImageSizes, compile};
use helmsman_node_p4::report::{parse_endpoint, parse_endpoint_port, send_line};
use helmsman_node_p4::{board, ethernet, modbus};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// Internal heap. The P4 has 768 KB of SRAM; the stack takes what is left.
const HEAP_SIZE: usize = 256 * 1024;

/// The remote block: the bench E1213 unless `E1213_ADDR` says otherwise at
/// build time (`E1213_ADDR=10.0.0.60 make node-build net`).
const E1213_ADDR: &str = match option_env!("E1213_ADDR") {
    Some(a) => a,
    None => "192.168.3.60",
};
const E1213_PORT: u16 = 502;
const UNIT: u8 = 1;
/// Where the report lines go: the Studio's log sink (`LOG_SINK=host:port`).
const LOG_SINK: &str = match option_env!("LOG_SINK") {
    Some(a) => a,
    None => "192.168.3.1:9514",
};

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
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    println!();
    println!(
        "helmsman-node-p4 modbus ({} {}) on the {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        board::BOARD
    );
    println!(
        "block {E1213_ADDR}:{E1213_PORT} unit {UNIT}; log sink {LOG_SINK}; scan {} ms",
        SCAN.as_millis()
    );

    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());

    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

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
        Err(e) => {
            println!("FATAL: ethernet init failed: {e:?}");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    match ethernet::net_task(runner) {
        Ok(task) => spawner.spawn(task),
        Err(_) => halt("the network task would not spawn").await,
    }

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
    send_line(
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
            send_line(&mut udp, sink, &alloc::format!("fault: {fault}")).await;
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
            send_line(&mut udp, sink, &line).await;
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

/// Prints the reason and stops.
async fn halt(reason: &str) -> ! {
    println!("FATAL: {reason}");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
