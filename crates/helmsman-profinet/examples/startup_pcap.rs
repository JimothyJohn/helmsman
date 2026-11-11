//! Writes the conversation of one startup (controller against the
//! simulated device, with a record write and an alarm) as a pcap file, to
//! look at in Wireshark or to hand to another dissector:
//!
//! ```sh
//! cargo run -p helmsman-profinet --example startup_pcap -- target/profinet-startup.pcap
//! ```
//!
//! RPC datagrams are wrapped in Ethernet, IPv4 and UDP headers so the
//! file is ordinary Ethernet throughout.

#[path = "../tests/common/mod.rs"]
mod common;

use std::io::Write;

use common::{Bench, CONTROLLER_IP, CONTROLLER_MAC, DEVICE_IP, DEVICE_MAC, From};
use helmsman_profinet::controller::{Out, State};
use helmsman_profinet::rpc;

fn checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in header.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !u16::try_from(sum).expect("folded to 16 bits")
}

/// `data` as a UDP datagram in an IPv4 packet in an Ethernet frame.
fn udp_frame(from: From, source_port: u16, port: u16, data: &[u8]) -> Vec<u8> {
    let (src_mac, dst_mac, src_ip, dst_ip) = match from {
        From::Controller => (CONTROLLER_MAC, DEVICE_MAC, CONTROLLER_IP, DEVICE_IP),
        From::Device => (DEVICE_MAC, CONTROLLER_MAC, DEVICE_IP, CONTROLLER_IP),
    };
    let udp_len = u16::try_from(8 + data.len()).expect("a datagram");
    let mut ip = vec![0x45, 0];
    ip.extend_from_slice(&(20 + udp_len).to_be_bytes());
    ip.extend_from_slice(&[0, 0, 0x40, 0, 64, 17, 0, 0]);
    ip.extend_from_slice(&src_ip);
    ip.extend_from_slice(&dst_ip);
    let sum = checksum(&ip);
    ip[10..12].copy_from_slice(&sum.to_be_bytes());
    let mut f = Vec::new();
    f.extend_from_slice(&dst_mac);
    f.extend_from_slice(&src_mac);
    f.extend_from_slice(&[0x08, 0x00]);
    f.extend_from_slice(&ip);
    f.extend_from_slice(&source_port.to_be_bytes());
    f.extend_from_slice(&port.to_be_bytes());
    f.extend_from_slice(&udp_len.to_be_bytes());
    // No UDP checksum (0), which IPv4 allows.
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(data);
    f
}

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "profinet-startup.pcap".into());
    let mut b = Bench::new();
    assert!(b.run_until(2000, |b| b.c.state() == State::Running && b.c.inputs().is_some()));
    b.d.inputs_mut().copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    b.c.outputs_mut().copy_from_slice(&[7, 8, 9]);
    b.run_ms(50);
    let mut out = Vec::new();
    b.d.raise_alarm(1, 2, 1, &[0x80, 0x00, 0xAA, 0xBB], &mut out);
    b.device_sends(out);
    b.run_ms(50);
    let mut out = Vec::new();
    b.c.release(&mut out);
    b.log.push(common::Packet {
        at: b.now,
        from: From::Controller,
        out: out.remove(0),
        source_port: rpc::PORT,
    });

    let mut file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    // The classic pcap header: magic, version 2.4, snap length, Ethernet.
    file.write_all(&0xA1B2_C3D4u32.to_le_bytes())?;
    file.write_all(&[2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
    file.write_all(&65535u32.to_le_bytes())?;
    file.write_all(&1u32.to_le_bytes())?;
    for p in &b.log {
        let frame = match &p.out {
            Out::Frame(f) => f.clone(),
            Out::Udp { port, data, .. } => udp_frame(p.from, p.source_port, *port, data),
        };
        let len = u32::try_from(frame.len()).expect("a frame");
        file.write_all(&u32::try_from(p.at / 1_000_000).expect("seconds").to_le_bytes())?;
        file.write_all(
            &u32::try_from(p.at % 1_000_000)
                .expect("microseconds")
                .to_le_bytes(),
        )?;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(&len.to_le_bytes())?;
        file.write_all(&frame)?;
    }
    file.flush()?;
    println!("{} packets written to {path}", b.log.len());
    Ok(())
}
