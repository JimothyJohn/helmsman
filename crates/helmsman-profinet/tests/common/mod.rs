//! A controller and a simulated device on one wire, with a clock the test
//! steps: every frame and datagram either side sends is handed to the
//! other, and kept for the test (and the pcap example) to look at.

#![allow(dead_code)]

use helmsman_profinet::controller::{Controller, DeviceConfig, Local, Out};
use helmsman_profinet::dcp::Ip;
use helmsman_profinet::device::{self, Device};
use helmsman_profinet::eth::Mac;
use helmsman_profinet::rpc;
use helmsman_profinet::rt::{Module, Record, Submodule};

pub const CONTROLLER_MAC: Mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
pub const DEVICE_MAC: Mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
pub const CONTROLLER_IP: Ip = [192, 168, 3, 10];
pub const DEVICE_IP: Ip = [192, 168, 3, 70];
/// The port the device answers RPCs from: not the well-known one, as
/// p-net's responses are not (`pf_cmrpc.c`, the session's socket), so the
/// controller has to follow it.
pub const DEVICE_REPLY_PORT: u16 = 49200;
/// The port the device calls "application ready" from (p-net
/// `PF_RPC_CCONTROL_EPHEMERAL_PORT`).
pub const DEVICE_CALL_PORT: u16 = 0xC001;
pub const NAME: &str = "press-5";
pub const VENDOR: u16 = 0x0145;
pub const DEVICE: u16 = 0x0A01;

/// Who sent a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum From {
    Controller,
    Device,
}

/// One packet on the wire.
#[derive(Debug, Clone)]
pub struct Packet {
    pub at: u64,
    pub from: From,
    pub out: Out,
    /// For a datagram: the port it was sent from.
    pub source_port: u16,
}

pub fn sub(subslot: u16, ident: u32, input: u16, output: u16) -> Submodule {
    Submodule {
        subslot,
        ident,
        input,
        output,
        records: Vec::new(),
    }
}

/// A device access point without data, four bytes of inputs, and a
/// module with two bytes in and three out that takes one record.
pub fn modules() -> Vec<Module> {
    let mut io = sub(1, 0x131, 2, 3);
    io.records.push(Record {
        index: 0x0064,
        data: vec![1, 2, 3, 4, 5],
    });
    vec![
        Module {
            api: 0,
            slot: 0,
            ident: 0x01,
            submodules: vec![sub(1, 0x01, 0, 0)],
        },
        Module {
            api: 0,
            slot: 1,
            ident: 0x30,
            submodules: vec![sub(1, 0x130, 4, 0)],
        },
        Module {
            api: 0,
            slot: 2,
            ident: 0x31,
            submodules: vec![io],
        },
    ]
}

pub fn local() -> Local {
    Local {
        mac: CONTROLLER_MAC,
        station_name: "helmsman".into(),
        vendor_id: 0xFFFF,
        device_id: 0x0001,
    }
}

pub struct Bench {
    pub c: Controller,
    pub d: Device,
    pub now: u64,
    pub log: Vec<Packet>,
    /// Packets to lose: called with each packet, true drops it.
    pub lose: Box<dyn FnMut(&Packet) -> bool>,
}

impl Bench {
    /// The usual pair: the device plugged exactly as the controller
    /// expects it, with an IP address.
    pub fn new() -> Self {
        Self::with(DeviceConfig::new(NAME, modules()), modules(), DEVICE_IP)
    }

    pub fn with(cfg: DeviceConfig, plugged: Vec<Module>, device_ip: Ip) -> Self {
        let c = Controller::new(local(), cfg, 0x1234_5678_9ABC_DEF0).expect("a valid configuration");
        let d = Device::new(
            DEVICE_MAC,
            device::station(NAME, VENDOR, DEVICE, device_ip),
            plugged,
            7,
        );
        Self {
            c,
            d,
            now: 1_000_000,
            log: Vec::new(),
            lose: Box::new(|_| false),
        }
    }

    fn deliver(&mut self, from: From, outs: Vec<Out>) {
        let mut queue: Vec<(From, Out)> = outs.into_iter().map(|o| (from, o)).collect();
        while !queue.is_empty() {
            let mut next = Vec::new();
            for (from, out) in queue {
                // The device answers from one port and calls from another.
                let source_port = match (&out, from) {
                    (Out::Udp { data, .. }, From::Device) => match rpc::Header::decode(data) {
                        Ok((h, _)) if h.packet_type == rpc::PT_REQUEST => DEVICE_CALL_PORT,
                        _ => DEVICE_REPLY_PORT,
                    },
                    _ => rpc::PORT,
                };
                let packet = Packet {
                    at: self.now,
                    from,
                    out,
                    source_port,
                };
                let lost = (self.lose)(&packet);
                self.log.push(packet.clone());
                if lost {
                    continue;
                }
                let mut replies = Vec::new();
                match (from, &packet.out) {
                    (From::Controller, Out::Frame(f)) => self.d.on_frame(self.now, f, &mut replies),
                    (From::Controller, Out::Udp { data, .. }) => {
                        self.d
                            .on_udp(self.now, CONTROLLER_IP, source_port, data, &mut replies);
                    }
                    (From::Device, Out::Frame(f)) => self.c.on_frame(self.now, f, &mut replies),
                    (From::Device, Out::Udp { data, .. }) => {
                        let ip = self.d.station().ip;
                        self.c.on_udp(self.now, ip, source_port, data, &mut replies);
                    }
                }
                let other = if from == From::Controller {
                    From::Device
                } else {
                    From::Controller
                };
                next.extend(replies.into_iter().map(|o| (other, o)));
            }
            queue = next;
        }
    }

    /// Hands the device's own sends (an alarm it raised) to the wire.
    pub fn device_sends(&mut self, outs: Vec<Out>) {
        self.deliver(From::Device, outs);
    }

    /// Runs both sides for `ms` milliseconds, a millisecond at a time.
    pub fn run_ms(&mut self, ms: u64) {
        for _ in 0..ms {
            self.now += 1000;
            let mut out = Vec::new();
            self.c.poll(self.now, &mut out);
            self.deliver(From::Controller, out);
            let mut out = Vec::new();
            self.d.poll(self.now, &mut out);
            self.deliver(From::Device, out);
        }
    }

    /// Runs until `done` holds, at most `ms` milliseconds; whether it
    /// came to hold.
    pub fn run_until(&mut self, ms: u64, mut done: impl FnMut(&Self) -> bool) -> bool {
        for _ in 0..ms {
            if done(self) {
                return true;
            }
            self.run_ms(1);
        }
        done(self)
    }
}
