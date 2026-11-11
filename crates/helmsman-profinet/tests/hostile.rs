//! Whatever arrives, nothing panics: every decoder on arbitrary bytes,
//! and both state machines on the packets of a real startup with bytes
//! changed, cut short or handed over out of order.

mod common;

use common::{Bench, CONTROLLER_IP, DEVICE_IP, From};
use helmsman_profinet::controller::{Out, State};
use helmsman_profinet::{alarm, block, dcp, eth, rpc, rt};
use proptest::prelude::*;

/// The packets of one clean startup and a second of cyclic data.
fn recording() -> Vec<common::Packet> {
    let mut b = Bench::new();
    assert!(b.run_until(2000, |b| b.c.state() == State::Running));
    let mut out = Vec::new();
    b.d.raise_alarm(1, 2, 1, &[1, 2], &mut out);
    b.device_sends(out);
    b.run_ms(100);
    b.log
}

/// A byte that depends on its position, to see data arrive in order.
fn pattern(i: usize, step: usize) -> u8 {
    (i * step + 1).to_le_bytes()[0]
}

fn bytes_of(out: &Out) -> &[u8] {
    match out {
        Out::Frame(f) => f,
        Out::Udp { data, .. } => data,
    }
}

proptest! {
    #[test]
    fn decoders_take_any_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..300), len in 0u16..1500) {
        let _ = eth::parse(&bytes);
        let _ = dcp::parse(&bytes);
        let _ = dcp::parse_request(&bytes);
        let _ = dcp::parse_identify_response(&bytes);
        let _ = dcp::parse_set_response(&bytes);
        let _ = rpc::Header::decode(&bytes);
        for order in [helmsman_profinet::codec::Order::Big, helmsman_profinet::codec::Order::Little] {
            let _ = rpc::parse_request_body(order, &bytes);
            let _ = rpc::parse_response_body(order, &bytes);
        }
        let _ = block::ConnectReq::decode(&bytes);
        let _ = block::ConnectRes::decode(&bytes);
        let _ = block::Control::decode(&bytes);
        let _ = block::WriteReq::decode(&bytes);
        let _ = block::WriteRes::decode(&bytes);
        let _ = rt::parse(&bytes, len);
        let _ = alarm::Rta::parse(&bytes);
        let _ = alarm::Notification::parse(&bytes);
        let _ = alarm::Ack::parse(&bytes);
    }

    /// A startup on a wire that loses packets: neither side panics, and
    /// once the wire is whole again the pair reaches cyclic data.
    #[test]
    fn a_lossy_startup_never_panics_and_recovers(every in 2usize..7, offset in 0usize..7) {
        let mut b = Bench::new();
        let mut n = offset;
        b.lose = Box::new(move |_| {
            n += 1;
            n % every == 0
        });
        b.run_ms(3000);
        let _ = (b.c.state(), b.c.error(), b.d.connected());
        b.lose = Box::new(|_| false);
        prop_assert!(
            b.run_until(40_000, |b| b.c.state() == State::Running && b.c.inputs().is_some()),
            "{:?} {:?}", b.c.state(), b.c.error()
        );
    }

    /// Recorded packets, damaged, into a fresh controller and a fresh
    /// device in any order.
    #[test]
    fn damaged_packets_in_any_order_never_panic(
        picks in proptest::collection::vec((any::<prop::sample::Index>(), 0usize..1500, any::<u8>(), any::<bool>()), 1..60),
    ) {
        let log = recording();
        let mut b = Bench::new();
        // Let the pair get somewhere first, so later states are reached too.
        b.run_ms(40);
        let mut now = b.now;
        for (pick, at, value, cut) in picks {
            let p = &log[pick.index(log.len())];
            let mut bytes = bytes_of(&p.out).to_vec();
            if !bytes.is_empty() {
                let at = at % bytes.len();
                bytes[at] = value;
                if cut {
                    bytes.truncate(at);
                }
            }
            now += 500;
            let mut out = Vec::new();
            match (&p.out, p.from) {
                (Out::Frame(_), From::Device) => b.c.on_frame(now, &bytes, &mut out),
                (Out::Frame(_), From::Controller) => b.d.on_frame(now, &bytes, &mut out),
                (Out::Udp { .. }, From::Device) => b.c.on_udp(now, DEVICE_IP, p.source_port, &bytes, &mut out),
                (Out::Udp { .. }, From::Controller) => b.d.on_udp(now, CONTROLLER_IP, p.source_port, &bytes, &mut out),
            }
            let mut out = Vec::new();
            b.c.poll(now, &mut out);
            b.d.poll(now, &mut out);
        }
    }
}

// Arbitrary module shapes: whatever layout comes out, the startup
// completes, every record arrives and data moves both ways.
proptest! {
    #[test]
    fn any_configuration_that_builds_starts_up(
        shapes in proptest::collection::vec((0u16..20, 0u16..20, 0usize..3), 1..12),
        ratio in 0u32..6,
    ) {
        use helmsman_profinet::controller::DeviceConfig;
        use helmsman_profinet::rt::{Module, Record};
        let modules: Vec<Module> = shapes
            .iter()
            .enumerate()
            .map(|(i, (input, output, records))| {
                let slot = u16::try_from(i).unwrap();
                let mut s = common::sub(1, 0x100 + u32::from(slot), *input, *output);
                s.records = (0..*records)
                    .map(|r| Record { index: 0x10 + u16::try_from(r).unwrap(), data: vec![slot.to_le_bytes()[0]; r + 1] })
                    .collect();
                Module { api: 0, slot, ident: 0x10 + u32::from(slot), submodules: vec![s] }
            })
            .collect();
        let mut cfg = DeviceConfig::new(common::NAME, modules.clone());
        cfg.reduction_ratio = 1 << ratio;
        let mut b = Bench::with(cfg, modules.clone(), DEVICE_IP);
        prop_assert!(b.run_until(3000, |b| b.c.state() == State::Running && b.c.inputs().is_some()), "{:?}", b.c.error());
        // Every record arrived, in order.
        let want: usize = shapes.iter().map(|s| s.2).sum();
        prop_assert_eq!(b.d.records.len(), want);
        // Data moves both ways through whatever layout came out.
        let n_in = b.d.inputs_mut().len();
        for (i, byte) in b.d.inputs_mut().iter_mut().enumerate() {
            *byte = pattern(i, 7);
        }
        for (i, byte) in b.c.outputs_mut().iter_mut().enumerate() {
            *byte = pattern(i, 5);
        }
        b.run_ms(80);
        let inputs = b.c.inputs().unwrap().to_vec();
        prop_assert_eq!(inputs.len(), n_in);
        prop_assert!(inputs.iter().enumerate().all(|(i, v)| *v == pattern(i, 7)));
        prop_assert!(b.d.outputs().iter().enumerate().all(|(i, v)| *v == pattern(i, 5)));
    }
}
