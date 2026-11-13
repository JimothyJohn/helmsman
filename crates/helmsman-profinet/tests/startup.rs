//! The controller against the simulated device: the whole startup, the
//! cyclic exchange, and what happens when either side misbehaves.

mod common;

use common::{Bench, DEVICE_IP, DEVICE_REPLY_PORT, From, NAME, modules, sub};
use helmsman_profinet::controller::{Address, Controller, DeviceConfig, Out, State};
use helmsman_profinet::rt::Module;
use helmsman_profinet::{block, eth, rpc, rt};

fn running(b: &Bench) -> bool {
    b.c.state() == State::Running && b.c.inputs().is_some()
}

#[test]
fn a_startup_reaches_cyclic_data_both_ways() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running), "{:?} {:?}", b.c.state(), b.c.error());
    assert_eq!(b.c.error(), None);
    assert_eq!(b.c.starts(), 1);
    assert_eq!(b.d.connects, 1);
    // Inputs: the device's image arrives in the controller's.
    b.d.inputs_mut().copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    b.c.outputs_mut().copy_from_slice(&[7, 8, 9]);
    b.run_ms(20);
    assert_eq!(b.c.inputs(), Some(&[1u8, 2, 3, 4, 5, 6][..]));
    assert_eq!(b.d.outputs(), &[7, 8, 9]);
    assert!((0..3).all(|i| b.c.input_good(i)));
    let station = b.c.station().unwrap();
    assert_eq!((station.vendor_id, station.ip), (common::VENDOR, DEVICE_IP));
}

#[test]
fn the_startup_is_identify_connect_write_prmend_ready_in_that_order() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    let mut steps = Vec::new();
    for p in &b.log {
        match &p.out {
            Out::Frame(f) => {
                let id = eth::parse(f).unwrap().frame_id;
                if id == eth::FRAME_ID_DCP_IDENTIFY_REQ || id == eth::FRAME_ID_DCP_IDENTIFY_RES {
                    steps.push(format!("{:?} dcp {id:#06x}", p.from));
                }
            }
            Out::Udp { data, .. } => {
                let (h, _) = rpc::Header::decode(data).unwrap();
                steps.push(format!("{:?} rpc type {} op {}", p.from, h.packet_type, h.opnum));
            }
        }
    }
    assert_eq!(
        steps,
        [
            "Controller dcp 0xfefe",
            "Device dcp 0xfeff",
            "Controller rpc type 0 op 0",
            "Device rpc type 2 op 0",
            "Controller rpc type 0 op 3",
            "Device rpc type 2 op 3",
            "Controller rpc type 0 op 4",
            "Device rpc type 2 op 4",
            "Device rpc type 0 op 4",
            "Controller rpc type 2 op 4",
        ]
    );
    // The record reached the device before PrmEnd did.
    assert_eq!(b.d.records, [(2, 1, 0x0064, vec![1, 2, 3, 4, 5])]);
}

#[test]
fn requests_after_the_connect_follow_the_port_the_device_answered_from() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    let ports: Vec<u16> = b
        .log
        .iter()
        .filter(|p| p.from == From::Controller)
        .filter_map(|p| match &p.out {
            Out::Udp { port, data, .. } => {
                (rpc::Header::decode(data).unwrap().0.packet_type == rpc::PT_REQUEST).then_some(*port)
            }
            Out::Frame(_) => None,
        })
        .collect();
    assert_eq!(ports, [rpc::PORT, DEVICE_REPLY_PORT, DEVICE_REPLY_PORT]);
}

#[test]
fn every_request_of_an_ar_shares_an_activity_and_counts_its_sequence() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    let headers: Vec<rpc::Header> = b
        .log
        .iter()
        .filter(|p| p.from == From::Controller)
        .filter_map(|p| match &p.out {
            Out::Udp { data, .. } => Some(rpc::Header::decode(data).unwrap().0),
            Out::Frame(_) => None,
        })
        .filter(|h| h.packet_type == rpc::PT_REQUEST)
        .collect();
    assert_eq!(headers.iter().map(|h| h.sequence).collect::<Vec<_>>(), [0, 1, 2]);
    assert!(headers.iter().all(|h| h.activity == headers[0].activity));
    assert!(headers.iter().all(|h| h.interface == rpc::IF_DEVICE));
    // The object UUID names the device: instance 1, its device and vendor id.
    assert_eq!(
        headers[0].object,
        rpc::object_uuid(1, common::DEVICE, common::VENDOR)
    );
    // The first request knows no boot time; later ones echo the device's.
    assert_eq!(
        headers.iter().map(|h| h.server_boot).collect::<Vec<_>>(),
        [0, 1, 1]
    );
}

#[test]
fn cyclic_frames_are_exact_length_tagged_and_their_counters_only_rise() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    b.run_ms(3000);
    for from in [From::Controller, From::Device] {
        let frames: Vec<_> = b
            .log
            .iter()
            .filter(|p| p.from == from)
            .filter_map(|p| match &p.out {
                Out::Frame(f) => Some((p.at, f.clone())),
                Out::Udp { .. } => None,
            })
            .filter(|(_, f)| (0xC000..=0xF7FF).contains(&eth::parse(f).unwrap().frame_id))
            .collect();
        assert!(frames.len() > 300, "{from:?}: {}", frames.len());
        let mut last: Option<(u64, u16)> = None;
        for (i, (at, f)) in frames.iter().enumerate() {
            // 6 + 6 + 4 (tag) + 2 + 2 (frame id) + 40 (data) + 4.
            assert_eq!(f.len(), 64);
            let parsed = eth::parse(f).unwrap();
            assert_eq!(parsed.vlan, Some(eth::Vlan::RT));
            let c = rt::parse(parsed.payload, 40).unwrap();
            assert_eq!(c.data_status, rt::STATUS_OK);
            if let Some((t, cycle)) = last {
                assert!(rt::newer(cycle, c.cycle), "{cycle} then {}", c.cycle);
                // Every 8 ms, once the first frame (sent as soon as the
                // Connect is answered) has set the grid.
                if i > 1 {
                    assert_eq!(at - t, 8000);
                }
            }
            last = Some((*at, c.cycle));
        }
    }
}

#[test]
fn a_station_without_an_address_is_given_the_configured_one() {
    let mut cfg = DeviceConfig::new(NAME, modules());
    cfg.address = Some(Address {
        ip: DEVICE_IP,
        mask: [255, 255, 255, 0],
        gateway: [0; 4],
    });
    let mut b = Bench::with(cfg, modules(), [0; 4]);
    assert!(b.run_until(2000, running), "{:?} {:?}", b.c.state(), b.c.error());
    assert_eq!(b.d.station().ip, DEVICE_IP);
    assert_eq!(b.c.station().unwrap().ip, DEVICE_IP);
}

#[test]
fn a_station_without_an_address_and_none_configured_is_an_error_in_words() {
    let mut b = Bench::with(DeviceConfig::new(NAME, modules()), modules(), [0; 4]);
    b.run_ms(500);
    assert_eq!(b.c.state(), State::Backoff);
    assert_eq!(
        b.c.error(),
        Some("\"press-5\" has no IP address and the device line gives it none")
    );
    assert!(!b.d.connected());
}

#[test]
fn a_station_of_another_make_is_refused_before_any_connect() {
    let mut cfg = DeviceConfig::new(NAME, modules());
    cfg.vendor_id = Some(0x0261);
    cfg.device_id = Some(0x0001);
    let mut b = Bench::with(cfg, modules(), DEVICE_IP);
    b.run_ms(500);
    assert_eq!(
        b.c.error(),
        Some("\"press-5\" is vendor 0x0145 device 0x0a01, not the 0x0261 / 0x0001 expected")
    );
    assert_eq!(b.d.connects, 0);
}

#[test]
fn nobody_by_that_name_keeps_asking_and_says_so() {
    let mut b = Bench::with(DeviceConfig::new("someone-else", modules()), modules(), DEVICE_IP);
    b.run_ms(3500);
    assert_eq!(b.c.state(), State::Identifying);
    assert_eq!(b.c.error(), Some("no station named \"someone-else\" answers"));
    let asks = b.log.iter().filter(|p| p.from == From::Controller).count();
    assert_eq!(asks, 4);
    assert!(b.log.iter().all(|p| p.from == From::Controller));
}

#[test]
fn a_device_built_differently_is_named_by_slot_and_released() {
    // The device has another module in slot 2 and nothing in slot 1.
    let plugged = vec![
        modules().remove(0),
        Module {
            api: 0,
            slot: 2,
            ident: 0x99,
            submodules: vec![sub(1, 0x131, 2, 3)],
        },
    ];
    let mut b = Bench::with(DeviceConfig::new(NAME, modules()), plugged, DEVICE_IP);
    b.run_ms(500);
    assert_eq!(
        b.c.error(),
        Some(
            "\"press-5\" is not built as configured: slot 1 is empty; slot 1 subslot 1 is empty; slot 2 holds module 0x99"
        )
    );
    assert_eq!(b.c.state(), State::Backoff);
    // The controller let go of the AR it had just opened.
    assert!(!b.d.connected());
    assert_eq!(b.c.inputs(), None);
}

#[test]
fn a_refused_record_stops_the_startup_and_names_the_record() {
    let mut b = Bench::new();
    b.d.faults.refuse_record = Some(0x0064);
    b.run_ms(500);
    assert_eq!(
        b.c.error(),
        Some("\"press-5\" refused a record write: PNIO status df 80 b0 00")
    );
    assert!(!b.d.connected());
    // Once the device takes it, the next attempt succeeds by itself.
    b.d.faults.refuse_record = None;
    assert!(b.run_until(5000, running));
    assert_eq!(b.c.error(), None);
    assert_eq!(b.c.starts(), 2);
}

#[test]
fn a_device_that_never_reports_ready_times_out() {
    let mut b = Bench::new();
    b.d.faults.never_ready = true;
    b.run_ms(2000);
    assert_eq!(b.c.state(), State::WaitingReady);
    assert_eq!(b.c.inputs(), None);
    b.run_ms(30_000);
    assert_eq!(b.c.error(), Some("\"press-5\" never reported application ready"));
}

#[test]
fn a_device_deaf_to_rpc_is_asked_three_times_then_given_up_on() {
    let mut b = Bench::new();
    b.d.faults.deaf = true;
    b.run_ms(7000);
    assert_eq!(b.c.error(), Some("\"press-5\" did not answer the Connect"));
    let connects = b
        .log
        .iter()
        .filter(|p| matches!(&p.out, Out::Udp { .. }) && p.at < 7_500_000)
        .count();
    assert_eq!(connects, 3);
}

#[test]
fn a_lost_connect_is_sent_again_with_the_same_sequence() {
    let mut b = Bench::new();
    let mut lost = false;
    b.lose = Box::new(move |p| {
        let first = !lost && p.from == From::Controller && matches!(p.out, Out::Udp { .. });
        lost |= first;
        first
    });
    assert!(b.run_until(5000, running), "{:?}", b.c.error());
    let seqs: Vec<u32> = b
        .log
        .iter()
        .filter(|p| p.from == From::Controller)
        .filter_map(|p| match &p.out {
            Out::Udp { data, .. } => Some(rpc::Header::decode(data).unwrap().0),
            Out::Frame(_) => None,
        })
        .filter(|h| h.opnum == rpc::OP_CONNECT)
        .map(|h| h.sequence)
        .collect();
    assert_eq!(seqs, [0, 0]);
    assert_eq!(b.d.connects, 1);
}

#[test]
fn a_device_that_goes_quiet_is_dropped_within_the_hold_time_and_taken_back() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    b.c.outputs_mut().copy_from_slice(&[1, 1, 1]);
    b.run_ms(20);
    assert_eq!(b.d.outputs(), &[1, 1, 1]);
    b.d.faults.silent = true;
    let quiet_at = b.now;
    assert!(b.run_until(200, |b| b.c.inputs().is_none()));
    // Three cycles of 8 ms after the last frame, which came up to one
    // cycle before the device went quiet, and the test's 1 ms step.
    let took = b.now - quiet_at;
    assert!((16_000..=26_000).contains(&took), "{took}");
    assert_eq!(b.c.state(), State::Backoff);
    assert!(
        b.c.error()
            .unwrap()
            .starts_with("\"press-5\" stopped sending cyclic data")
    );
    // The controller stopped driving it, so the device let its outputs go.
    b.run_ms(100);
    assert_eq!(b.d.outputs(), &[0, 0, 0]);
    b.d.faults.silent = false;
    assert!(b.run_until(5000, running));
    assert_eq!(b.c.error(), None);
    b.run_ms(20);
    // The outputs the owner set are driven again after the restart.
    assert_eq!(b.d.outputs(), &[1, 1, 1]);
}

#[test]
fn an_alarm_is_acknowledged_twice_over_and_handed_to_the_owner() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    for round in 1..=3u32 {
        let mut out = Vec::new();
        assert!(b.d.raise_alarm(1, 2, 1, &[0x80, 0x00, 0xAA], &mut out));
        b.device_sends(out);
        b.run_ms(5);
        assert_eq!(b.d.alarms_acked, round);
        let alarms = b.c.take_alarms();
        assert_eq!(alarms.len(), 1);
        assert_eq!(
            (alarms[0].alarm_type, alarms[0].slot, alarms[0].module_ident),
            (1, 2, 0x31)
        );
        assert_eq!(alarms[0].data, [0x80, 0x00, 0xAA]);
    }
    assert!(running(&b));
}

#[test]
fn a_repeated_alarm_is_acknowledged_again_but_reported_once() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    let mut out = Vec::new();
    b.d.raise_alarm(1, 2, 1, &[], &mut out);
    b.device_sends(out.clone());
    // The same PDU again, as a device whose acknowledgement was lost sends.
    b.device_sends(out);
    b.run_ms(5);
    assert_eq!(b.c.take_alarms().len(), 1);
}

#[test]
fn release_tells_the_device_and_the_controller_stops_sending() {
    let mut b = Bench::new();
    assert!(b.run_until(2000, running));
    let mut out = Vec::new();
    b.c.release(&mut out);
    assert_eq!(out.len(), 1);
    let Out::Udp { data, .. } = &out[0] else { panic!() };
    let (h, body) = rpc::Header::decode(data).unwrap();
    assert_eq!(h.opnum, rpc::OP_RELEASE);
    let (_, blocks) = rpc::parse_request_body(h.order, body).unwrap();
    let c = block::Control::decode(blocks).unwrap();
    assert_eq!(
        (c.block_type, c.command),
        (block::RELEASE_REQ, block::COMMAND_RELEASE)
    );
    assert_eq!(b.c.inputs(), None);
}

#[test]
fn configurations_that_cannot_go_on_the_wire_are_refused_when_built() {
    let build = |f: &dyn Fn(&mut DeviceConfig)| {
        let mut cfg = DeviceConfig::new(NAME, modules());
        f(&mut cfg);
        Controller::new(common::local(), cfg, 1).map(|_| ()).unwrap_err()
    };
    assert_eq!(
        build(&|c| c.station_name = "Press 5".into()),
        "\"Press 5\" is not a station name"
    );
    assert!(build(&|c| c.modules.clear()).contains("at least one module"));
    assert!(build(&|c| c.reduction_ratio = 3).contains("reduction ratio 3"));
    assert!(build(&|c| c.send_clock_factor = 33).contains("send clock factor 33"));
    assert!(build(&|c| c.data_hold_factor = 0).contains("data hold factor 0"));
    assert!(build(&|c| c.input_frame_id = 0x8000).contains("0x8000"));
    assert!(build(&|c| c.modules[1].submodules[0].input = 1500).contains("1440"));
    // 200 submodules: the Connect no longer fits one datagram.
    let many = build(&|c| {
        c.modules[1].submodules = (1..=200).map(|i| sub(i, 1, 1, 0)).collect();
    });
    assert!(many.contains("more than one datagram"), "{many}");
}

/// The shapes of the two devices due on the bench, as their GSDML files
/// give them (docs/devices/): each starts up and moves data end to end.
#[test]
fn the_bench_devices_shapes_start_up_and_fit_one_connect() {
    use helmsman_profinet::text::slots;
    // Novanta MDrive; a Universal Robot with every module it has and its
    // interface and port submodules (subslots 0x8000 and 0x8001).
    let shapes = [
        "0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128",
        "0:0x1/1:0x1/0x8000:0x8000/0x8001:0x8001,1:0x30/1:0x1:i32,2:0x31/1:0x1:i68,\
         3:0x32/1:0x1:i104,4:0x33/1:0x1:i76,5:0x34/1:0x1:i8,6:0x35/1:0x1:i96,7:0x36/1:0x1:i96,\
         8:0x40/1:0x1:o24,9:0x41/1:0x1:o100,10:0x42/1:0x1:o100,11:0x3B000200/1:0x3B000201,\
         12:0x43/1:0x1:i256:o256,13:0x37/1:0x1:i88,14:0x44/1:0x1:o40",
    ];
    for shape in shapes {
        let modules = slots(shape).unwrap();
        let mut b = Bench::with(DeviceConfig::new(NAME, modules.clone()), modules, DEVICE_IP);
        assert!(
            b.run_until(3000, running),
            "{shape}: {:?} {:?}",
            b.c.state(),
            b.c.error()
        );
        let (n_in, n_out) = (b.c.layout().input_image_len, b.c.layout().output_image_len);
        let pattern = |n: usize, step: usize| {
            (0..n)
                .map(|i| (i * step + 1).to_le_bytes()[0])
                .collect::<Vec<u8>>()
        };
        b.d.inputs_mut().copy_from_slice(&pattern(n_in, 3));
        b.c.outputs_mut().copy_from_slice(&pattern(n_out, 7));
        b.run_ms(40);
        assert_eq!(b.c.inputs().unwrap(), &pattern(n_in, 3)[..], "{shape}");
        assert_eq!(b.d.outputs(), &pattern(n_out, 7)[..], "{shape}");
    }
}
