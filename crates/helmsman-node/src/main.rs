//! The edge spike's Phase 1 firmware (docs/EDGE.md, G2 and G3): Helmsman's
//! IEC runtime on an Olimex ESP32-POE2, no network. Two programs are
//! embedded and compiled at boot, scanned 10 000 times each, and the
//! per-scan time (min, mean, max, p99) and the heap (after compile, after
//! the scans, the peak) go out over USB serial. The round runs twice: on
//! internal SRAM alone, then with the WROVER's PSRAM added as a second
//! heap region (esp-alloc fills internal RAM first and overflows into
//! PSRAM), so G3 has both figures.
//!
//! Ethernet unplugged throughout; USB power only (the board has no
//! galvanic isolation). Flashing is `scripts/node-flash.sh`, Nick's step.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::time::Duration;

use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::time::Instant;
use esp_println::println;
use helmsman_iec::{Image, ImageSizes, compile};

// The bootloader's application descriptor (espflash's esp-idf bootloader
// refuses an image without one). Version, name, build stamps, the IDF
// version the layout is compatible with, the MMU page size, two reserved
// fields, the anti-rollback secure version: the order the macro wants.
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

/// The image the box gives a bare program: 32 B of inputs and outputs, 64 B
/// of memory. Same as the host bench and the default deploy.
const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

/// Scans per program per round: enough for a p99 that means something and
/// for the heap to show any growth over time.
const SCANS: usize = 10_000;

/// The host bench's workload, about 4 000 statements a scan.
const BENCH: &str = include_str!("../../helmsman-iec/examples/trace_bench.st");

/// A realistic cell: the batch-mixer starter's controls with the blocks it
/// uses, as the project builds them (the plant model left out: on a real
/// machine the sensors replace it).
const CELL: &str = concat!(
    include_str!("../../helmsman-project/starters/batch-mixer/main.st"),
    "\n",
    include_str!("../../helmsman-project/starters/blocks/analog_scale.st"),
    "\n",
    include_str!("../../helmsman-project/starters/blocks/latched_alarm.st"),
    "\n",
    include_str!("../../helmsman-project/starters/blocks/motor_starter.st"),
    "\n",
    include_str!("../../helmsman-project/starters/blocks/solenoid_valve.st"),
    "\n",
    include_str!("../../helmsman-project/starters/blocks/stack_light.st"),
    "\n",
    include_str!("../../helmsman-project/starters/blocks/step_sequencer.st"),
);

/// Internal heap: the main DRAM segment, then the otherwise unused dram2
/// segment (the figures supervictor settled on for the same chip).
const HEAP_SIZE: usize = 160 * 1024;
const DRAM2_HEAP_SIZE: usize = 96 * 1024;

#[esp_hal::main]
fn main() -> ! {
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: DRAM2_HEAP_SIZE);
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    println!();
    println!(
        "helmsman-node scan bench ({} {})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "heap: {} B internal in two regions; esp-alloc heap-stats on (a little per-alloc cost)",
        esp_alloc::HEAP.free()
    );
    println!("--- round 1: internal SRAM ---");
    run("bench-4k", BENCH);
    run("cell", CELL);

    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);
    println!(
        "--- round 2: internal SRAM + PSRAM ({} B heap in all) ---",
        esp_alloc::HEAP.stats().size
    );
    run("bench-4k", BENCH);
    run("cell", CELL);
    println!("--- done ---");

    loop {
        // Nothing more to do; the serial monitor has the numbers.
        esp_hal::delay::Delay::new().delay_millis(1000);
    }
}

/// Compiles `src`, scans it [`SCANS`] times at a 10 ms clock, and prints
/// one line of numbers for the record in docs/EDGE.md.
fn run(name: &str, src: &str) {
    let used_before = esp_alloc::HEAP.used();
    let t0 = Instant::now();
    let mut program = match compile(src, SIZES) {
        Ok(p) => p,
        Err(diags) => {
            for d in diags {
                println!("{name}: compile error {}: {}", d.pos, d.msg);
            }
            return;
        }
    };
    let compile_us = t0.elapsed().as_micros();
    let used_after_compile = esp_alloc::HEAP.used();

    let mut image = Image::new(SIZES);
    let mut samples: Vec<u32> = Vec::with_capacity(SCANS);
    let mut now = Duration::ZERO;
    for n in 0..SCANS {
        let t = Instant::now();
        if let Err(fault) = program.scan_at(&mut image, now) {
            println!("{name}: fault at scan {n}: {fault}");
            return;
        }
        samples.push(t.elapsed().as_micros() as u32);
        now += Duration::from_millis(10);
    }
    let used_after_scans = esp_alloc::HEAP.used();
    let peak = esp_alloc::HEAP.stats().max_usage;

    samples.sort_unstable();
    let min = samples[0];
    let max = samples[SCANS - 1];
    let p99 = samples[SCANS * 99 / 100];
    let mean = samples.iter().map(|&s| u64::from(s)).sum::<u64>() / SCANS as u64;
    println!(
        "{name}: compile {compile_us} us, heap +{} B for the program; {SCANS} scans: min {min} mean {mean} max {max} p99 {p99} us; heap used {used_after_scans} B after the scans, peak {peak} B",
        used_after_compile.saturating_sub(used_before)
    );
}
