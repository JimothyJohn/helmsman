//! A simulated PROFINET IO device on a real interface: helmsman-profinet's
//! test device given this machine's MAC and IP address, so a controller
//! elsewhere on the wire (an edge node, another box) has something to
//! find, connect to and exchange cyclic data with when no real device is
//! on the bench.
//!
//! ```sh
//! cargo run -p helmsman-io --features profinet-raw --example profinet_device -- \
//!     en0 bench-sim 0:0x1/1:0x1,1:0x30/1:0x1:i8,2:0x31/1:0x1:o8
//! ```
//!
//! Its first input byte counts up ten times a second; every line it
//! prints says what arrived and what the controller drives. It answers
//! RPCs at UDP port 34964, so no box with `profinet-io` devices can run
//! on the same machine meanwhile.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use helmsman_l2::RawSocket;
use helmsman_profinet::controller::Out;
use helmsman_profinet::device::{self, Device};
use helmsman_profinet::{eth, rpc, text};

fn fail(why: &str) -> ! {
    eprintln!("{why}");
    std::process::exit(1);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(interface), Some(station), Some(slots)) = (args.next(), args.next(), args.next()) else {
        fail("usage: profinet_device INTERFACE STATION SLOTS");
    };
    let modules = text::slots(&slots).unwrap_or_else(|e| fail(&e));
    let addresses = helmsman_l2::addresses(&interface).unwrap_or_else(|e| fail(&e.to_string()));
    let Some(ip) = addresses.ipv4 else {
        fail(&format!("{interface} has no IPv4 address"));
    };
    let mut raw = RawSocket::open(&interface, eth::ETHERTYPE_PROFINET)
        .unwrap_or_else(|e| fail(&format!("{interface}: {e}")));
    let udp = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, rpc::PORT))
        .unwrap_or_else(|e| fail(&format!("UDP port {}: {e}", rpc::PORT)));
    udp.set_nonblocking(true).unwrap_or_else(|e| fail(&e.to_string()));
    let mut dev = Device::new(
        raw.mac(),
        device::station(&station, 0xFFFF, 0x0003, ip),
        modules,
        u64::from(std::process::id()),
    );
    println!(
        "{station} on {interface}: {} input bytes, {} output bytes, ip {}",
        dev.inputs_mut().len(),
        dev.outputs().len(),
        Ipv4Addr::from(ip)
    );

    let start = Instant::now();
    let now = || u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    let (mut frames_in, mut frames_out, mut calls, mut udp_errors) = (0u64, 0u64, 0u64, 0u64);
    let mut last = String::new();
    let mut report = Instant::now();
    let mut buf = [0u8; 1600];
    let mut out = Vec::new();
    loop {
        match raw.recv(Duration::from_millis(1)) {
            Ok(Some(frame)) => {
                frames_in += 1;
                dev.on_frame(now(), &frame, &mut out);
            }
            Ok(None) => {}
            Err(e) => fail(&format!("{interface}: {e}")),
        }
        while let Ok((n, SocketAddr::V4(from))) = udp.recv_from(&mut buf) {
            calls += 1;
            dev.on_udp(now(), from.ip().octets(), from.port(), &buf[..n], &mut out);
        }
        if let Some(first) = dev.inputs_mut().first_mut() {
            *first = (start.elapsed().as_millis() / 100).to_le_bytes()[0];
        }
        dev.poll(now(), &mut out);
        for o in out.drain(..) {
            match o {
                Out::Frame(f) => {
                    frames_out += 1;
                    if let Err(e) = raw.send(&f) {
                        fail(&format!("{interface}: {e}"));
                    }
                }
                Out::Udp { ip, port, data } => {
                    if let Err(e) = udp.send_to(&data, SocketAddrV4::new(Ipv4Addr::from(ip), port)) {
                        udp_errors += 1;
                        if udp_errors == 1 {
                            println!("UDP to {}:{port}: {e}", Ipv4Addr::from(ip));
                        }
                    }
                }
            }
        }
        let line = format!(
            "{}; outputs {:02x?}",
            if dev.connected() {
                "connected"
            } else {
                "no controller"
            },
            &dev.outputs()[..dev.outputs().len().min(8)]
        );
        if line != last || report.elapsed() >= Duration::from_secs(2) {
            println!(
                "{line}; frames in {frames_in} out {frames_out}, calls {calls}, connects {}",
                dev.connects
            );
            last = line;
            report = Instant::now();
        }
    }
}
