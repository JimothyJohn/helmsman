//! Asks every PROFINET station on an interface to identify itself (DCP)
//! and lists who answers: name, MAC and IP address, vendor and device
//! id. The first thing to run with a new device on the bench.
//!
//! ```sh
//! cargo run -p helmsman-io --features profinet-raw --example profinet_scan -- en0
//! ```
//!
//! Needs raw Ethernet: on macOS access to `/dev/bpf*`, on Linux
//! `CAP_NET_RAW` (the error says which is missing).

use std::time::{Duration, Instant};

use helmsman_l2::RawSocket;
use helmsman_profinet::{dcp, eth};

fn main() {
    let interface = std::env::args().nth(1).unwrap_or_else(|| "en0".into());
    let mut raw = match RawSocket::open(&interface, eth::ETHERTYPE_PROFINET) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{interface}: {e}");
            std::process::exit(1);
        }
    };
    let xid = std::process::id() | 1;
    // A response delay factor of 100: stations spread their answers over
    // up to a second, so a full network does not answer all at once.
    let request = dcp::identify_request(xid, None, 100);
    let frame = eth::encode(
        dcp::MULTICAST_IDENTIFY,
        raw.mac(),
        None,
        eth::FRAME_ID_DCP_IDENTIFY_REQ,
        &request,
    );
    if let Err(e) = raw.send(&frame) {
        eprintln!("{interface}: sending the Identify: {e}");
        std::process::exit(1);
    }
    println!("{interface}: asked every station to identify itself; listening 3 s");
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut found = 0;
    while Instant::now() < deadline {
        let Ok(Some(bytes)) = raw.recv(Duration::from_millis(100)) else {
            continue;
        };
        let Ok(f) = eth::parse(&bytes) else { continue };
        if f.frame_id != eth::FRAME_ID_DCP_IDENTIFY_RES {
            continue;
        }
        let Ok((got, s)) = dcp::parse_identify_response(f.payload) else {
            println!("  an Identify response from {:02x?} that does not parse", f.src);
            continue;
        };
        if got != xid {
            continue;
        }
        found += 1;
        let mac: Vec<String> = f.src.iter().map(|b| format!("{b:02x}")).collect();
        let name = if s.name.is_empty() { "(no name)" } else { &s.name };
        println!(
            "  {name:<24} {}  ip {:<15} vendor {:#06x} device {:#06x}  {}",
            mac.join(":"),
            std::net::Ipv4Addr::from(s.ip).to_string(),
            s.vendor_id,
            s.device_id,
            s.type_of_station
        );
    }
    println!("{found} station(s)");
}
