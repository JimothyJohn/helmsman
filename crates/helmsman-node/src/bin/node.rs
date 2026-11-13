//! The node (docs/EDGE.md Phase 4): settings on flash, a fixed scan of the
//! deployed program, the remote block on its own I/O task, the box API
//! subset on the settings' port for the canvas, the site beacon, and a
//! report line every 10 s over serial and UDP.
//!
//! First boot: nothing on flash, so the node writes the factory record and
//! answers at 192.168.127.250/24 with no program and no token. Its page
//! (`http://192.168.127.250:8080/`) sets the name, site, address and token;
//! a deploy is `PUT /api/modules/{name}` with the ST source and the token
//! as a bearer. Build-time: `E1213_ADDR` (the block), `NODE_SEED_CHASE=1`
//! (write the chase program to flash when none is there, for the bench).
//!
//! Power rule, until the board's PoE isolation is known: never USB and the
//! Ethernet cable together (the POE-ISO is isolated; the POE2 is not).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use core::cell::RefCell;
use core::time::Duration as CoreDuration;

use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, StaticConfigV4};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker, Timer};
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;
use esp_storage::FlashStorage;
use helmsman_iec::{Image, ImageSizes, Program, compile};
use helmsman_node::state::{self, IO_BYTES, Pending, PointNames, Shared};
use helmsman_node::{beacon, ethernet, flash, io, server};
use helmsman_nodecore::api::State;
use helmsman_nodecore::iomap::{self, IoMap};
use helmsman_nodecore::router::Control;
use helmsman_nodecore::settings::{Net, Settings};
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

/// The main stack is what the linker leaves above .bss, and the task pools
/// sit at the very top of .bss: at 112 KiB of heap the node's stack was
/// 42 KiB and compiling the program at boot overran it by a few hundred
/// bytes, which flipped the network tasks' state to "spawned" so the first
/// spawn failed Busy (2026-11-08, three flashes). 96 KiB leaves about
/// 58 KiB of stack; the heap peaked at 162 KiB of the 192 KiB that remain.
const HEAP_SIZE: usize = 96 * 1024;
const DRAM2_HEAP_SIZE: usize = 96 * 1024;

/// The remote block the factory map names (the stored map wins once one
/// is saved, card node-io-map).
const E1213_ADDR: &str = match option_env!("E1213_ADDR") {
    Some(a) => a,
    None => "192.168.3.60",
};
const E1213_PORT: u16 = 502;
const SEED_CHASE: bool = option_env!("NODE_SEED_CHASE").is_some();

const SCAN: Duration = Duration::from_millis(10);
const REPORT_EVERY: Duration = Duration::from_secs(10);
const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

/// The bench program: a one-of-three chase on DO-00..02 every 500 ms.
const CHASE: &str = "
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

static SHARED: StaticCell<Shared> = StaticCell::new();
static NAMES: StaticCell<PointNames> = StaticCell::new();
static SETTINGS: StaticCell<Settings> = StaticCell::new();
static STORE: StaticCell<server::StoreCell> = StaticCell::new();
static CTX: StaticCell<server::Ctx> = StaticCell::new();
static LOG_RX_META: StaticCell<[PacketMetadata; 1]> = StaticCell::new();
static LOG_TX_META: StaticCell<[PacketMetadata; 2]> = StaticCell::new();
static LOG_RX: StaticCell<[u8; 64]> = StaticCell::new();
static LOG_TX: StaticCell<[u8; 512]> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: DRAM2_HEAP_SIZE);
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    println!();
    println!(
        "helmsman-node node ({} {}) on the {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        helmsman_node::board::BOARD
    );

    let _trng = TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    esp_rtos::start(timg0.timer0);

    // Settings and the program, from flash.
    let mut store = match flash::Store::open(FlashStorage::new(peripherals.FLASH)) {
        Ok(s) => Some(s),
        Err(e) => {
            println!("flash: {e}; settings and programs will not persist");
            None
        }
    };
    let settings: &'static Settings = SETTINGS.init(match store.as_mut() {
        Some(s) => flash::boot_settings(s),
        None => Settings::factory(),
    });
    let mut program: Option<(String, String, Program)> = None;
    if let Some(stored) = store.as_ref().and_then(|s| s.program().cloned()) {
        match compile(&stored.source, SIZES) {
            Ok(p) => {
                println!(
                    "program: {} ({} bytes) from flash slot {:?}",
                    stored.name,
                    stored.source.len(),
                    stored.slot
                );
                program = Some((stored.name, stored.source, p));
            }
            Err(diags) => {
                println!("program: the stored program no longer compiles:");
                for d in &diags {
                    println!("  {}: {}", d.pos, d.msg);
                }
            }
        }
    } else if SEED_CHASE {
        if let Ok(p) = compile(CHASE, SIZES) {
            if let Some(s) = store.as_mut() {
                match s.write_program("chase", CHASE) {
                    Ok(()) => println!("program: chase seeded to flash"),
                    Err(e) => println!("program: chase not kept ({e})"),
                }
            }
            program = Some((String::from("chase"), String::from(CHASE), p));
        } else {
            println!("program: the built-in chase does not compile?!");
        }
    } else {
        println!("program: none; deploy one with PUT /api/modules/{{name}}");
    }
    // The I/O map (card node-io-map): the text kept beside the program,
    // else the factory map at the build-time block address.
    let factory_host = parse_endpoint(E1213_ADDR, E1213_PORT).map_or([192, 168, 3, 60], |e| match e.addr {
        IpAddress::Ipv4(a) => a.octets(),
    });
    let stored_map = store
        .as_ref()
        .and_then(|s| s.program().map(|p| p.map.clone()))
        .unwrap_or_default();
    let map = if stored_map.trim().is_empty() {
        println!("io map: none on flash; the factory map ({E1213_ADDR}, di0..7, do0..3)");
        IoMap::factory(factory_host, "rio")
    } else {
        match iomap::parse(&stored_map) {
            Ok(m) => {
                println!(
                    "io map: {} at {}.{}.{}.{}:{} unit {}, {} inputs, {} coils",
                    m.device,
                    m.host[0],
                    m.host[1],
                    m.host[2],
                    m.host[3],
                    m.port,
                    m.unit,
                    m.input_count(),
                    m.coil_count()
                );
                m
            }
            Err(e) => {
                println!("io map: the stored map no longer parses ({e}); the factory map");
                IoMap::factory(factory_host, "rio")
            }
        }
    };
    let names: &'static PointNames = NAMES.init(PointNames::of(&map));
    let block = IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::new(
            map.host[0],
            map.host[1],
            map.host[2],
            map.host[3],
        )),
        map.port,
    );
    let (unit, input_count, coil_count) = (map.unit, map.input_count(), map.coil_count());
    let store: &'static server::StoreCell = STORE.init(embassy_sync::mutex::Mutex::new(RefCell::new(store)));

    let shared: &'static Shared = SHARED.init(Mutex::new(RefCell::new(state::Live::default())));
    state::with(shared, |live| {
        live.device.name.clone_from(&map.device);
        live.device.address = alloc::format!("{block}");
        live.io_map.clone_from(&map.text);
        if let Some((name, source, p)) = &program {
            live.module.name.clone_from(name);
            live.module.source.clone_from(source);
            live.module.state = Some(State::Running);
            live.module.signals = state::signals_of(p, &[0; IO_BYTES], &[0; IO_BYTES], names);
        }
    });

    // The network, as the settings say.
    let addressing = match settings.net {
        Net::Dhcp => ethernet::Addressing::Dhcp,
        Net::Static { ip, prefix, gateway } =>
        {
            #[allow(clippy::default_trait_access)]
            ethernet::Addressing::Static(StaticConfigV4 {
                address: Ipv4Cidr::new(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]), prefix),
                gateway: (gateway != [0; 4])
                    .then(|| Ipv4Address::new(gateway[0], gateway[1], gateway[2], gateway[3])),
                dns_servers: Default::default(),
            })
        }
    };
    let phy_power = Output::new(peripherals.GPIO12, Level::Low, OutputConfig::default());
    let (stack, runner, emac_ptr) = match ethernet::init(phy_power, seed) {
        Ok(parts) => parts,
        Err(e) => halt(&alloc::format!("ethernet init failed: {e:?}")).await,
    };
    let mac = helmsman_node::board::ethernet_mac(esp_hal::efuse::Efuse::mac_address());
    // A spawn that fails must say so: an ignored one looks like a hang.
    spawner
        .spawn(ethernet::eth_net_task(runner))
        .expect("spawn net task");
    spawner
        .spawn(ethernet::eth_link_task(emac_ptr))
        .expect("spawn link task");
    spawner
        .spawn(ethernet::eth_dhcp_task(stack, emac_ptr, addressing))
        .expect("spawn address task");

    println!("waiting for the link...");
    let waited = Instant::now();
    loop {
        if embassy_time::with_timeout(Duration::from_secs(5), stack.wait_config_up())
            .await
            .is_ok()
        {
            break;
        }
        println!(
            "still waiting for the link after {} s (link {})",
            waited.elapsed().as_secs(),
            if stack.is_link_up() { "up" } else { "down" }
        );
    }
    if let Some(cfg) = stack.config_v4() {
        println!("address {}; API on port {}", cfg.address, settings.api_port);
    }

    // The tasks: the block (as the map says), the server (two
    // connections), the beacon.
    spawner
        .spawn(io::io_task(stack, block, unit, input_count, coil_count, shared))
        .ok();
    let ctx: &'static server::Ctx = CTX.init(server::Ctx {
        shared,
        settings,
        store,
        mac,
        board: helmsman_node::board::BOARD,
        heap_size: HEAP_SIZE + DRAM2_HEAP_SIZE,
        scan_ms: 10.0,
        sizes: SIZES,
    });
    spawner.spawn(server::http_task(stack, ctx)).ok();
    spawner.spawn(server::http_task(stack, ctx)).ok();
    if !settings.site.is_empty() {
        spawner
            .spawn(beacon::beacon_task(
                stack,
                settings,
                shared,
                mac,
                helmsman_node::board::BOARD,
                ctx.heap_size,
                10.0,
            ))
            .ok();
    }

    // The report line's UDP socket, if the settings name a sink.
    let sink = settings.log_sink.map(|(ip, port)| {
        IpEndpoint::new(
            IpAddress::Ipv4(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3])),
            port,
        )
    });
    let mut log = UdpSocket::new(
        stack,
        LOG_RX_META.init([PacketMetadata::EMPTY; 1]),
        LOG_RX.init([0; 64]),
        LOG_TX_META.init([PacketMetadata::EMPTY; 2]),
        LOG_TX.init([0; 512]),
    );
    let _ = log.bind(0);

    // The scan loop.
    let mut image = Image::new(SIZES);
    let mut running = program.map(|(_, _, p)| p);
    let mut stopped = false;
    let start = Instant::now();
    let mut ticker = Ticker::every(SCAN);
    let mut deadline = start + SCAN;
    let mut last_report = start;
    let mut scan_count: u64 = 0;
    loop {
        ticker.next().await;
        let t0 = Instant::now();
        let late_us = u32::try_from(t0.saturating_duration_since(deadline).as_micros()).unwrap_or(u32::MAX);
        let was_late = t0 > deadline + SCAN;
        deadline += SCAN;

        // What a request asked for.
        let pending = state::with(shared, |live| live.pending.take());
        match pending {
            Some(Pending::Deploy { name, source }) => {
                // Free the running program first: one compiled program and
                // one compile peak at a time is all this heap holds.
                let old_source = state::with(shared, |live| live.module.source.clone());
                running = None;
                let result = match compile(&source, SIZES) {
                    Ok(p) => {
                        running = Some(p);
                        stopped = false;
                        state::with(shared, |live| {
                            live.module.name.clone_from(&name);
                            live.module.source = source;
                            live.module.state = Some(State::Running);
                            live.module.scans = 0;
                            live.module.late = 0;
                            live.module.over = 0;
                            live.module.last_overrun = None;
                            live.module.exec = state::Ring::default();
                            live.module.lateness = state::Ring::default();
                        });
                        println!("deployed: {name}");
                        Ok(())
                    }
                    Err(diags) => {
                        let errors: alloc::vec::Vec<String> = diags
                            .iter()
                            .map(|d| alloc::format!("{}: {}", d.pos, d.msg))
                            .collect();
                        // Back to what ran, if it still compiles.
                        if !old_source.is_empty() {
                            match compile(&old_source, SIZES) {
                                Ok(p) => running = Some(p),
                                Err(_) => state::with(shared, |live| {
                                    live.module.state = Some(State::Faulted(String::from(
                                        "deploy refused and the previous program could not be restored",
                                    )));
                                    live.outputs = [0; IO_BYTES];
                                }),
                            }
                        }
                        println!("deploy refused: {} error(s)", errors.len());
                        Err(errors)
                    }
                };
                state::with(shared, |live| live.deployed = Some(result));
            }
            Some(Pending::Control(action)) => {
                let state = match (action, running.as_mut()) {
                    (Control::Stop, Some(_)) => {
                        stopped = true;
                        image.q.fill(0);
                        "stopped"
                    }
                    (Control::Run, Some(_)) => {
                        stopped = false;
                        "running"
                    }
                    (Control::Restart, Some(p)) => {
                        p.reset();
                        image.q.fill(0);
                        stopped = false;
                        "running"
                    }
                    (_, None) => "unknown",
                };
                state::with(shared, |live| {
                    live.controlled = Some(state);
                    live.module.state = match state {
                        "stopped" => Some(State::Stopped),
                        "running" => Some(State::Running),
                        _ => live.module.state.clone(),
                    };
                    if state == "stopped" {
                        live.outputs = [0; IO_BYTES];
                    }
                });
            }
            None => {}
        }

        if let Some(p) = running.as_mut().filter(|_| !stopped) {
            let inputs = state::with(shared, |live| live.inputs);
            image.i[..IO_BYTES].copy_from_slice(&inputs);
            let now = CoreDuration::from_micros(start.elapsed().as_micros());
            let result = p.scan_at(&mut image, now);
            let exec_us = u32::try_from(t0.elapsed().as_micros()).unwrap_or(u32::MAX);
            scan_count += 1;
            let outputs: [u8; IO_BYTES] = image.q[..IO_BYTES].try_into().unwrap_or([0; IO_BYTES]);
            let refresh = scan_count.is_multiple_of(10);
            let signals = refresh.then(|| state::signals_of(p, &inputs, &outputs, names));
            let faulted = result.as_ref().err().map(|f| alloc::format!("{f}"));
            state::with(shared, |live| {
                live.module.scans += 1;
                live.module.exec.push(exec_us);
                live.module.lateness.push(late_us);
                if was_late {
                    live.module.late += 1;
                    live.module.last_overrun = Some(t0);
                }
                if t0.elapsed() > SCAN {
                    live.module.over += 1;
                }
                match &faulted {
                    Some(text) => {
                        live.module.state = Some(State::Faulted(text.clone()));
                        live.outputs = [0; IO_BYTES];
                    }
                    None => live.outputs = outputs,
                }
                if let Some(s) = signals {
                    live.module.signals = s;
                    live.seq += 1;
                    live.at = Some(t0);
                }
            });
            if let Some(text) = faulted {
                println!("fault: {text}");
                stopped = true;
            }
        } else if scan_count.is_multiple_of(10) {
            scan_count += 1;
            state::with(shared, |live| {
                live.seq += 1;
                live.at = Some(t0);
            });
        } else {
            scan_count += 1;
        }

        if last_report.elapsed() >= REPORT_EVERY {
            last_report = Instant::now();
            let (link_up, ip) = match stack.config_v4() {
                Some(cfg) => (true, Some(cfg.address.address().octets())),
                None => (stack.is_link_up(), None),
            };
            let line = state::with(shared, |live| {
                live.link_up = link_up;
                live.ip = ip;
                alloc::format!(
                    "up {}s {} scans {} late {} over {} exec max {} us device {} heap used {} B peak {} B",
                    start.elapsed().as_secs(),
                    if live.module.state.is_some() {
                        live.module.name.as_str()
                    } else {
                        "no program"
                    },
                    live.module.scans,
                    live.module.late,
                    live.module.over,
                    live.module.exec.spread().map_or(0.0, |s| s.max),
                    match live.device.up {
                        Some(true) => "up",
                        Some(false) => "down",
                        None => "?",
                    },
                    esp_alloc::HEAP.used(),
                    esp_alloc::HEAP.stats().max_usage,
                )
            });
            println!("{line}");
            if let Some(to) = sink {
                let _ = log.send_to(line.as_bytes(), to).await;
            }
        }
    }
}

/// Prints the reason and stops.
async fn halt(reason: &str) -> ! {
    println!("FATAL: {reason}");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

/// `a.b.c.d` with `port`.
fn parse_endpoint(addr: &str, port: u16) -> Option<IpEndpoint> {
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in addr.split('.') {
        if n == 4 {
            return None;
        }
        octets[n] = part.parse().ok()?;
        n += 1;
    }
    (n == 4).then(|| {
        IpEndpoint::new(
            IpAddress::Ipv4(Ipv4Address::new(octets[0], octets[1], octets[2], octets[3])),
            port,
        )
    })
}
