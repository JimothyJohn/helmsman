//! The P4 node's first EtherNet/IP firmware (docs/EDGE.md Phase 3): the
//! node as the originator of one Class 1 connection to a Moxa ioLogik E1213,
//! a 10 ms scan of an embedded ST program against the inputs it receives,
//! its outputs produced at the RPI. Late scans, cycle times, packets each
//! way, reopens and the last error are counted and reported every 10 s over
//! serial and as UDP lines to a log sink on the Studio
//! (scripts/node-log-sink.py), since on power over Ethernet there is no USB.
//!
//! The connection is `Exclusive Owner` (configuration 100, O->T 102, one
//! byte of DO bits under a run/idle header, T->O 101, 51 bytes, modeless)
//! unless `ENIP_INPUT_ONLY=1` at build time makes it `Input Only`
//! (heartbeat to 111, the same 51 bytes back): the E1213 grants one
//! exclusive owner, and today that is the try box on the Studio, which
//! refuses this node's open with 0x0106 (ownership conflict) until it lets
//! go. Input Only runs beside it. Layout: docs/devices/moxa-iologik-e1213.md.
//!
//! Power rule, until the power-over-Ethernet module's isolation is known:
//! flash over USB with Ethernet unplugged, unplug USB, then plug the
//! Ethernet cable. Never both.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration as CoreDuration;

use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Ticker, Timer, with_timeout};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use helmsman_iec::{Image, ImageSizes, compile};
use helmsman_node_p4::enip::{self, IoConnection, Session};
use helmsman_node_p4::report::{parse_endpoint_port, parse_ipv4, send_line};
use helmsman_node_p4::{board, ethernet};
use helmsman_proto::class1::IoConfig;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

/// Internal heap. The P4 has 768 KB of SRAM; the program, the sockets and
/// the frames need a fraction of this, and the stack takes what is left.
const HEAP_SIZE: usize = 256 * 1024;

/// The remote block: the bench E1213 unless `E1213_ADDR` says otherwise at
/// build time.
const E1213_ADDR: &str = match option_env!("E1213_ADDR") {
    Some(a) => a,
    None => "192.168.3.60",
};
/// Where the report lines go: the Studio's log sink (`LOG_SINK=host:port`).
const LOG_SINK: &str = match option_env!("LOG_SINK") {
    Some(a) => a,
    None => "192.168.3.1:9514",
};
const INPUT_ONLY: bool = option_env!("ENIP_INPUT_ONLY").is_some();

const SCAN: Duration = Duration::from_millis(10);
/// The E1213's RPI range is 100 ms to 3 s (its EDS, Param3).
const RPI: Duration = Duration::from_millis(100);
/// An explicit exchange that takes longer than this is a link error.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(1000);
/// After a lost connection, how long before the next open.
const REOPEN_BACKOFF: Duration = Duration::from_secs(2);
const REPORT_EVERY: Duration = Duration::from_secs(10);
const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

// E1213 assemblies (docs/devices/moxa-iologik-e1213.md).
const CONFIG_ASSEMBLY: u16 = 100;
const OUTPUT_ASSEMBLY: u16 = 102;
const INPUT_ASSEMBLY: u16 = 101;
const HEARTBEAT_ASSEMBLY: u16 = 111;
const OUTPUT_LEN: u16 = 1;
const INPUT_LEN: u16 = 51;
/// Byte 50 of the input assembly: the DO status.
const DO_STATUS_BYTE: usize = 50;

/// The program: a one-of-three chase on DO-00..02 every 500 ms, the inputs
/// read as DI-00..02 (wired back from the outputs on the bench). `%IX2.x`
/// is the DO status the block reports, so `seen` can also be checked
/// without the loop-back wiring.
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

// The sockets' buffers, in statics rather than on the task's stack.
static LOG_RX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static LOG_TX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static LOG_RX: StaticCell<[u8; 256]> = StaticCell::new();
static LOG_TX: StaticCell<[u8; 512]> = StaticCell::new();
static IO_RX_META: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static IO_TX_META: StaticCell<[PacketMetadata; 4]> = StaticCell::new();
static IO_RX: StaticCell<[u8; 1024]> = StaticCell::new();
static IO_TX: StaticCell<[u8; 512]> = StaticCell::new();
static TCP_RX: StaticCell<[u8; 1024]> = StaticCell::new();
static TCP_TX: StaticCell<[u8; 512]> = StaticCell::new();

/// What the scan loop asks of the link task. The explicit session (TCP
/// connect, `RegisterSession`, `Forward_Open`, `Forward_Close`) can take
/// its whole timeout against a block that is not there, and the scan must
/// not wait for it: on the bench (2026-11-10) each boot's first two opens
/// timed out inside the scan loop and 199 scans were late before the
/// first packet.
enum LinkRequest {
    Open {
        config: IoConfig,
        serial: u16,
        output: Vec<u8>,
    },
    /// Best effort, after a loss.
    Close(Box<IoConnection>),
}

static LINK_REQUESTS: Channel<CriticalSectionRawMutex, LinkRequest, 2> = Channel::new();
static LINK_OPENED: Channel<CriticalSectionRawMutex, Result<IoConnection, enip::Error>, 1> = Channel::new();
/// The session's TCP connects since start, for the report line.
static CONNECTS: AtomicU32 = AtomicU32::new(0);

/// Owns the explicit session and does what the scan loop asks, one
/// request at a time.
#[embassy_executor::task]
async fn link_task(mut session: Session<'static>) -> ! {
    loop {
        match LINK_REQUESTS.receive().await {
            LinkRequest::Open {
                config,
                serial,
                output,
            } => {
                let opened = IoConnection::open(&mut session, config, serial, &output).await;
                CONNECTS.store(session.connects, Ordering::Relaxed);
                LINK_OPENED.send(opened).await;
            }
            LinkRequest::Close(connection) => {
                if let Err(e) = connection.close(&mut session).await {
                    println!("close after loss: {e}");
                }
            }
        }
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    println!();
    println!(
        "helmsman-node-p4 enip ({} {}) on the {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        board::BOARD
    );
    println!(
        "block {E1213_ADDR} ({}); log sink {LOG_SINK}; scan {} ms, RPI {} ms",
        if INPUT_ONLY {
            "input only"
        } else {
            "exclusive owner"
        },
        SCAN.as_millis(),
        RPI.as_millis()
    );

    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let mac = board::mac_address();
    println!(
        "MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

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
        Err(e) => halt(&alloc::format!("ethernet init failed: {e:?}")).await,
    };
    match ethernet::net_task(runner) {
        Ok(task) => spawner.spawn(task),
        Err(_) => halt("the network task would not spawn").await,
    }

    println!("waiting for the link and a lease...");
    stack.wait_config_up().await;
    // A fixed address is "up" before the PHY has negotiated; an open sent
    // into a link that is still down only times out.
    stack.wait_link_up().await;
    if let Some(cfg) = stack.config_v4() {
        println!("lease {}", cfg.address);
    }

    let Some(block) = parse_ipv4(E1213_ADDR) else {
        halt(&alloc::format!(
            "E1213_ADDR {E1213_ADDR:?} is not an IPv4 address"
        ))
        .await
    };
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

    // The I/O socket on 2222: where the target sends T->O.
    let mut io = UdpSocket::new(
        stack,
        IO_RX_META.init([PacketMetadata::EMPTY; 4]),
        IO_RX.init([0; 1024]),
        IO_TX_META.init([PacketMetadata::EMPTY; 4]),
        IO_TX.init([0; 512]),
    );
    if io.bind(helmsman_proto::encap::IO_PORT).is_err() {
        halt("io: udp 2222 bind failed").await;
    }

    let session = Session::new(
        stack,
        block,
        REQUEST_TIMEOUT,
        TCP_RX.init([0; 1024]),
        TCP_TX.init([0; 512]),
    );
    match link_task(session) {
        Ok(task) => spawner.spawn(task),
        Err(_) => halt("the link task would not spawn").await,
    }

    let mut program = match compile(PROGRAM, SIZES) {
        Ok(p) => p,
        Err(diags) => {
            for d in &diags {
                println!("compile error {}: {}", d.pos, d.msg);
            }
            halt("the program does not compile").await
        }
    };
    let mut image = Image::new(SIZES);

    let mut stats = Stats::default();
    let start = Instant::now();
    let mut last_report = start;
    let mut ticker = Ticker::every(SCAN);
    let mut deadline = start + SCAN;
    let mut next_produce = start;
    let mut serial: u16 = 0;
    let mut connection: Option<IoConnection> = None;
    // An open is with the link task; its answer is taken when it is there.
    let mut opening = false;
    let mut last_input: Option<Instant> = None;
    let mut reopen_at = start;
    let mut buf = [0u8; 1024];

    send_line(
        &mut log,
        sink,
        &alloc::format!(
            "start: block {E1213_ADDR} {}, scan {} ms, RPI {} ms",
            if INPUT_ONLY {
                "input only"
            } else {
                "exclusive owner"
            },
            SCAN.as_millis(),
            RPI.as_millis()
        ),
    )
    .await;

    loop {
        ticker.next().await;
        let t0 = Instant::now();
        if t0 > deadline + SCAN {
            stats.late += 1;
        }
        deadline += SCAN;

        // The connection: open, or reopen after a loss.
        if connection.is_none() && !opening && t0 >= reopen_at {
            serial = serial.wrapping_add(1).max(1);
            let output = if INPUT_ONLY {
                Vec::new()
            } else {
                alloc::vec![image.q[0]]
            };
            let request = LinkRequest::Open {
                config: io_config(mac),
                serial,
                output,
            };
            opening = LINK_REQUESTS.try_send(request).is_ok();
        }
        if opening && let Ok(opened) = LINK_OPENED.try_receive() {
            opening = false;
            match opened {
                Ok(c) => {
                    println!(
                        "open: O->T id {:#010x} to {}, T->O id {:#010x}, timeout {} ms",
                        c.reply().o_to_t_connection_id,
                        c.target(),
                        c.reply().t_to_o_connection_id,
                        c.config().timeout().as_millis()
                    );
                    stats.opens += 1;
                    last_input = Some(Instant::now());
                    next_produce = Instant::now();
                    connection = Some(c);
                }
                Err(e) => {
                    println!("open failed: {e}");
                    stats.errors += 1;
                    stats.last_error = Some(e);
                    reopen_at = Instant::now() + REOPEN_BACKOFF;
                }
            }
        }

        // T->O: take what arrived since the last scan, without blocking the
        // scan (a zero wait drains the socket).
        if let Some(c) = connection.as_mut() {
            while let Ok(Ok((n, _from))) =
                with_timeout(Duration::from_micros(200), io.recv_from(&mut buf)).await
            {
                match c.consumer.accept(&buf[..n]) {
                    Some(r) => {
                        if r.data.len() >= INPUT_LEN as usize {
                            image.i[0] = r.data[0];
                            image.i[1] = r.data[1];
                            image.i[2] = r.data[DO_STATUS_BYTE];
                        }
                        stats.packets_in += 1;
                        last_input = Some(Instant::now());
                    }
                    None => stats.ignored += 1,
                }
            }
        }

        // The scan, with the node's own clock.
        let now = CoreDuration::from_micros(start.elapsed().as_micros());
        if let Err(fault) = program.scan_at(&mut image, now) {
            send_line(&mut log, sink, &alloc::format!("fault: {fault}")).await;
            halt(&alloc::format!("fault: {fault}")).await;
        }
        if image.q[1] & 1 == 1 {
            stats.matched += 1;
        } else {
            stats.mismatched += 1;
        }

        // O->T at the RPI, with the scan's outputs.
        let mut lost = false;
        if let Some(c) = connection.as_mut() {
            if !INPUT_ONLY {
                let _ = c.producer.set_output(&[image.q[0]]);
            }
            if t0 >= next_produce {
                next_produce += RPI;
                let pkt = c.producer.next_packet();
                match io.send_to(&pkt, c.target()).await {
                    Ok(()) => stats.packets_out += 1,
                    Err(_) => stats.send_errors += 1,
                }
            }
            // Inactivity: the target's T->O stopped for longer than the
            // connection's timeout; close what we can and reopen later.
            let timeout = c.config().timeout();
            let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            if last_input.is_some_and(|t| t.elapsed() > Duration::from_millis(timeout_ms)) {
                println!("connection lost: no T->O for {timeout_ms} ms");
                stats.losses += 1;
                lost = true;
            }
        }
        if lost {
            // The close goes to the link task; if its queue is full the
            // target's own timeout ends the connection.
            if let Some(c) = connection.take() {
                let _ = LINK_REQUESTS.try_send(LinkRequest::Close(Box::new(c)));
            }
            last_input = None;
            reopen_at = Instant::now() + REOPEN_BACKOFF;
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
            let line = stats.line(
                start.elapsed(),
                CONNECTS.load(Ordering::Relaxed),
                connection.as_ref(),
            );
            println!("{line}");
            send_line(&mut log, sink, &line).await;
            stats.cycle_max = Duration::from_ticks(0);
        }
    }
}

/// The E1213 connection this build asks for.
fn io_config(mac: [u8; 6]) -> IoConfig {
    let rpi = CoreDuration::from_millis(RPI.as_millis());
    let mut c = if INPUT_ONLY {
        IoConfig::input_only(
            CONFIG_ASSEMBLY,
            HEARTBEAT_ASSEMBLY,
            INPUT_ASSEMBLY,
            INPUT_LEN,
            rpi,
        )
    } else {
        IoConfig::exclusive_owner(
            CONFIG_ASSEMBLY,
            OUTPUT_ASSEMBLY,
            INPUT_ASSEMBLY,
            OUTPUT_LEN,
            INPUT_LEN,
            rpi,
        )
    };
    // The E1213's T->O is modeless (no run/idle header); its O->T carries
    // one, which `exclusive_owner` already asks for.
    c.t_to_o_run_idle = false;
    c.originator_serial = board::originator_serial(mac);
    c
}

/// Prints the reason and stops; the watchdog story is the block's own
/// comms-loss state, which this node's silence triggers.
async fn halt(reason: &str) -> ! {
    println!("FATAL: {reason}");
    loop {
        Timer::after(Duration::from_secs(60)).await;
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
    opens: u64,
    losses: u64,
    packets_out: u64,
    packets_in: u64,
    ignored: u64,
    send_errors: u64,
    errors: u64,
    matched: u64,
    mismatched: u64,
    cycle_max: Duration,
    cycle_sum: u64,
    last_error: Option<enip::Error>,
}

impl Stats {
    fn line(&self, up: Duration, connects: u32, connection: Option<&IoConnection>) -> alloc::string::String {
        let mean = self.cycle_sum.checked_div(self.scans).unwrap_or(0);
        #[allow(clippy::cast_precision_loss)]
        let late_pct = if self.scans == 0 {
            0.0
        } else {
            self.late as f64 * 100.0 / self.scans as f64
        };
        alloc::format!(
            "up {}s scans {} late {} ({late_pct:.3}%) over {} cycle mean {} us max {} us {} opens {} losses {} out {} in {} ignored {} send errors {} errors {} sessions {} match {} mismatch {} heap used {} B peak {} B{}",
            up.as_secs(),
            self.scans,
            self.late,
            self.over,
            mean,
            self.cycle_max.as_micros(),
            if connection.is_some() {
                "connected"
            } else {
                "no connection"
            },
            self.opens,
            self.losses,
            self.packets_out,
            self.packets_in,
            self.ignored,
            self.send_errors,
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
