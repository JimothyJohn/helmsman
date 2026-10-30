//! Broadcast `ListIdentity` on the LAN and print who answers.
//!
//! `cargo run --example discover [target[:port]] [window_ms]`
//! Default target is the limited broadcast 255.255.255.255:44818.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use helmsman_proto::discover::sweep;
use helmsman_proto::encap::PORT;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let target: SocketAddrV4 = args
        .next()
        .map_or(SocketAddrV4::new(Ipv4Addr::BROADCAST, PORT), |s| {
            if s.contains(':') {
                s.parse().expect("target must be ip:port")
            } else {
                SocketAddrV4::new(s.parse().expect("target must be an IPv4 address"), PORT)
            }
        });
    let window = Duration::from_millis(args.next().map_or(2000, |s| s.parse().expect("window_ms")));

    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    let result = sweep(&sock, target, window)?;

    for f in &result.found {
        let id = &f.identity;
        println!(
            "{from}\tvendor={vendor} type=0x{dt:04X} code={code} rev={maj}.{min} status=0x{status:04X} serial=0x{serial:08X} state={state}\t{name}",
            from = f.from,
            vendor = id.vendor_id,
            dt = id.device_type,
            code = id.product_code,
            maj = id.revision_major,
            min = id.revision_minor,
            status = id.status,
            serial = id.serial_number,
            state = id.state,
            name = id.product_name_lossy(),
        );
    }
    for (from, why) in &result.rejected {
        eprintln!("rejected {from}: {why:?}");
    }
    eprintln!("{} found, {} rejected", result.found.len(), result.rejected.len());
    Ok(())
}
