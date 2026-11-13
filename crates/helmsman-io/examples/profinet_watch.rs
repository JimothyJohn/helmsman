//! Brings one PROFINET device up from a device line and shows what it
//! sends: the startup's state once a second, then the first bytes of the
//! input image as they change. Outputs stay zero. For the first run
//! against a new device, before any program drives it.
//!
//! ```sh
//! cargo run -p helmsman-io --features profinet-raw --example profinet_watch -- \
//!     mdrive if=en0 slots=0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128
//! ```
//!
//! The arguments are a `profinet-io` device line's endpoint and params
//! (crates/helmsman-io/src/profinet.rs). Ctrl-C ends it; the device is
//! released when the driver drops, which Ctrl-C does not wait for, so the
//! device then times the connection out by itself.

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_io::driver::Driver;
use helmsman_io::profinet::{self, ProfinetDriver};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(station) = args.next() else {
        eprintln!("usage: profinet_watch STATION if=NAME slots=... [param=value ...]");
        std::process::exit(64);
    };
    let mut params = BTreeMap::new();
    for a in args {
        let Some((k, v)) = a.split_once('=') else {
            eprintln!("{a}: want param=value");
            std::process::exit(64);
        };
        params.insert(k.to_string(), v.to_string());
    }
    let config = match profinet::Config::parse(&station, &params) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(64);
        }
    };
    let layout = helmsman_profinet::rt::Layout::new(&config.device.modules).expect("checked by the parse");
    println!(
        "{station}: {} input bytes, {} output bytes, a frame every {} ms",
        layout.input_image_len,
        layout.output_image_len,
        u32::from(config.device.reduction_ratio) * u32::from(config.device.send_clock_factor) / 32
    );
    let mut driver = ProfinetDriver::new(&station, &params, profinet::opener()).expect("parsed above");
    // Read the image a byte at a time, as far as 32 bytes.
    let shown = layout.input_image_len.min(32);
    let points: Vec<String> = (0..shown).map(|i| format!("in:{i}:u8")).collect();
    let addrs: Vec<&str> = points.iter().map(String::as_str).collect();
    if addrs.is_empty() {
        eprintln!("{station}: the modules have no input data to show");
    }
    let mut last = String::new();
    loop {
        let probe = if addrs.is_empty() {
            vec!["out:0:u8"]
        } else {
            addrs.clone()
        };
        let line = match driver.read(&probe) {
            Ok(values) => {
                let hex: Vec<String> = values
                    .iter()
                    .map(|v| match v {
                        helmsman_io::Value::Int(n) => format!("{n:02x}"),
                        other => format!("{other:?}"),
                    })
                    .collect();
                format!("running  in[0..{shown}] = {}", hex.join(" "))
            }
            Err(e) => format!("{e}"),
        };
        if line != last {
            println!("{line}");
            last = line;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
