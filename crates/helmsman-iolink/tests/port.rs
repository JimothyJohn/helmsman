//! A Master port against the simulated Device on a simulated line: the
//! startup at each rate, the times the specification sets, a lost and a
//! returning Device, refused Devices, and a line that drops, cuts, flips
//! and invents octets.

use helmsman_iolink::device::{Device, Mode};
use helmsman_iolink::master::{Config, Fault, Identity, Master, State, Step};
use helmsman_iolink::mseq::Error;
use helmsman_iolink::page::{Page1, cycle_time};
use helmsman_iolink::timing::Rate;
use proptest::prelude::*;

const VENDOR: u16 = 0x0136;
const DEVICE: u32 = 0x00_029A;

/// A page with revision 1.1 and the ids above.
fn page(min_cycle: u8, capability: u8, pd_in: u8, pd_out: u8) -> Page1 {
    Page1([
        0, 0, min_cycle, capability, 0x11, pd_in, pd_out, 0x01, 0x36, 0x00, 0x02, 0x9A, 0, 0, 0, 0,
    ])
}

/// 16 bits in, none out, `TYPE_2_2`; PREOPERATE with `TYPE_1_2`; 2,3 ms.
fn sensor() -> Page1 {
    page(0x17, 0x11, 0x10, 0x00)
}

/// 8 bits each way, `TYPE_2_5`; no minimum cycle time stated.
fn valve() -> Page1 {
    page(0x00, 0x11, 0x08, 0x08)
}

/// 4 octets in and 3 out, OPERATE code 4 (`TYPE_2_V`), PREOPERATE with 8
/// octets of On-request Data.
fn wide() -> Page1 {
    page(0x00, 0x29, 0x83, 0x82)
}

/// What the line does to one reply.
#[derive(Debug, Clone)]
enum Hit {
    Pass,
    Drop,
    Cut(usize),
    Flip(usize, u8),
    Invent(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Seen {
    WakeUp { at: u64 },
    Sent { at: u64, rate: Rate, mc: u8, len: usize },
}

struct Bench {
    master: Master,
    device: Device,
    plugged: bool,
    now: u64,
    seen: Vec<Seen>,
    hits: Vec<Hit>,
    /// The reply to every message with this control octet is replaced.
    rewrite: Option<(u8, Vec<u8>)>,
}

impl Bench {
    fn new(rate: Rate, page: Page1, config: Config) -> Self {
        Self {
            master: Master::new(config),
            device: Device::new(rate, page).unwrap(),
            plugged: true,
            now: 0,
            seen: Vec::new(),
            hits: Vec::new(),
            rewrite: None,
        }
    }

    /// One poll of the port, played out on the line.
    fn step(&mut self) {
        let mut sent = [0; 66];
        let (rate, len, want, timeout_us) = match self.master.poll(self.now) {
            Step::WakeUp => {
                self.seen.push(Seen::WakeUp { at: self.now });
                if self.plugged {
                    self.device.wake_up(self.now);
                }
                self.now += 80;
                return;
            }
            Step::Idle { until } => {
                assert!(until > self.now, "idle until {until} at {}", self.now);
                self.now = until;
                return;
            }
            Step::Exchange {
                rate,
                send,
                reply,
                timeout_us,
            } => {
                sent[..send.len()].copy_from_slice(send);
                (rate, send.len(), reply, timeout_us)
            }
        };
        self.seen.push(Seen::Sent {
            at: self.now,
            rate,
            mc: sent[0],
            len,
        });
        let mut answer = [0; 80];
        let got = if self.plugged {
            self.device.exchange(self.now, rate, &sent[..len], &mut answer)
        } else {
            None
        };
        let mut octets = got.map(|n| answer[..n].to_vec()).unwrap_or_default();
        match self.hits.pop().unwrap_or(Hit::Pass) {
            Hit::Pass => {}
            Hit::Drop => octets.clear(),
            Hit::Cut(n) => octets.truncate(n),
            Hit::Flip(at, bit) => {
                if !octets.is_empty() {
                    let at = at % octets.len();
                    octets[at] ^= 1 << bit;
                }
            }
            Hit::Invent(invented) => octets = invented,
        }
        if let Some((mc, reply)) = &self.rewrite
            && *mc == sent[0]
        {
            octets.clone_from(reply);
        }
        // A receiver stops at the octets it was told to expect, or at the
        // timeout with what it has.
        octets.truncate(want);
        if octets.len() == want {
            let took = rate.micros((len + want) * 11 + 5);
            assert!(
                took <= timeout_us,
                "a prompt reply took {took} of {timeout_us} us"
            );
            self.now += took;
        } else {
            self.now += timeout_us;
        }
        self.master.reply(self.now, &octets);
    }

    fn run_for(&mut self, micros: u64) {
        let end = self.now + micros;
        while self.now < end {
            self.step();
        }
    }

    /// Steps until the port is in OPERATE with inputs, for at most
    /// `micros`.
    fn operate_within(&mut self, micros: u64) -> bool {
        let end = self.now + micros;
        while self.now < end {
            if self.master.state() == State::Operate && self.master.inputs().is_some() {
                return true;
            }
            self.step();
        }
        false
    }

    fn sent(&self) -> Vec<(u64, Rate, u8)> {
        self.seen
            .iter()
            .filter_map(|seen| match *seen {
                Seen::Sent { at, rate, mc, .. } => Some((at, rate, mc)),
                Seen::WakeUp { .. } => None,
            })
            .collect()
    }

    fn wake_ups(&self) -> Vec<u64> {
        self.seen
            .iter()
            .filter_map(|seen| match *seen {
                Seen::WakeUp { at } => Some(at),
                Seen::Sent { .. } => None,
            })
            .collect()
    }
}

#[test]
fn a_sensor_comes_up_at_each_rate_and_its_inputs_arrive() {
    for rate in [Rate::Com1, Rate::Com2, Rate::Com3] {
        let mut bench = Bench::new(rate, sensor(), Config::default());
        bench.device.set_inputs(&[0x12, 0x34]);
        assert!(
            bench.operate_within(2_000_000),
            "{rate:?}: {:?}",
            bench.master.state()
        );
        assert_eq!(bench.master.rate(), Some(rate));
        assert_eq!(bench.master.inputs(), Some(&[0x12, 0x34][..]));
        assert_eq!(bench.master.fault(), None);
        assert_eq!(bench.device.mode(), Mode::Operate);
        let page = bench.master.page().unwrap();
        assert_eq!((page.vendor_id(), page.device_id()), (VENDOR, DEVICE));
        // New inputs are there a cycle later.
        bench.device.set_inputs(&[0xAB, 0xCD]);
        bench.run_for(2 * bench.master.cycle_us());
        assert_eq!(bench.master.inputs(), Some(&[0xAB, 0xCD][..]));
        assert_eq!((bench.master.losses(), bench.master.failures()), (0, 0));
    }
}

#[test]
fn outputs_reach_the_device_once_declared_valid() {
    let mut bench = Bench::new(Rate::Com3, valve(), Config::default());
    assert!(bench.operate_within(1_000_000));
    assert_eq!(bench.device.outputs(), None);
    assert_eq!(
        bench.master.set_outputs(Some(&[1, 2])),
        Err(Error::Length { want: 1, got: 2 })
    );
    bench.master.set_outputs(Some(&[0x5A])).unwrap();
    bench.run_for(3 * bench.master.cycle_us());
    assert_eq!(bench.device.outputs(), Some(&[0x5A][..]));
    bench.master.set_outputs(Some(&[0xA5])).unwrap();
    bench.run_for(2 * bench.master.cycle_us());
    assert_eq!(bench.device.outputs(), Some(&[0xA5][..]));
    // Invalid again: the Device is told and stops using them.
    bench.master.set_outputs(None).unwrap();
    bench.run_for(3 * bench.master.cycle_us());
    assert_eq!(bench.device.outputs(), None);
}

#[test]
fn a_wide_device_runs_type_2_v_both_ways() {
    let mut bench = Bench::new(Rate::Com1, wide(), Config::default());
    bench.device.set_inputs(&[1, 2, 3, 4]);
    assert!(bench.operate_within(3_000_000), "{:?}", bench.master.fault());
    assert_eq!(bench.master.inputs(), Some(&[1, 2, 3, 4][..]));
    bench.master.set_outputs(Some(&[7, 8, 9])).unwrap();
    bench.run_for(3 * bench.master.cycle_us());
    assert_eq!(bench.device.outputs(), Some(&[7, 8, 9][..]));
}

/// IOL Table 46 T1: the first message is MC 0xA2 at COM3, then COM2,
/// then COM1, each at least `T_DMT` (27 bit times of its own rate) after
/// the one before timed out (§7.3.2.2, Table 42).
#[test]
fn the_rates_are_probed_in_descending_order() {
    let mut bench = Bench::new(Rate::Com1, sensor(), Config::default());
    assert!(bench.operate_within(2_000_000));
    let sent = bench.sent();
    let rates: Vec<Rate> = sent.iter().take(3).map(|s| s.1).collect();
    assert_eq!(rates, [Rate::Com3, Rate::Com2, Rate::Com1]);
    assert!(sent.iter().take(3).all(|s| s.2 == 0xA2));
    let wake = bench.wake_ups()[0];
    // T_WU + T_REN + T_DMT after the pulse began, at the earliest.
    assert!(
        sent[0].0 - wake >= 75 + 500 + Rate::Com3.micros(27),
        "{}",
        sent[0].0 - wake
    );
    for (before, after) in sent.iter().zip(&sent[1..]).take(2) {
        let silence = after.0 - before.0 - before.1.micros(58);
        assert!(silence >= after.1.micros(27), "{silence} us before {:?}", after.1);
    }
    // After the answer nothing is sent at another rate.
    assert!(sent[3..].iter().all(|s| s.1 == Rate::Com1));
}

/// IOL §7.3.2.2 and Table 42: three wake-ups 30 to 50 ms apart (from the
/// end of one attempt), then at least 0,5 s of silence, for as long as
/// nobody answers.
#[test]
fn an_empty_port_retries_as_table_42_allows() {
    let mut bench = Bench::new(Rate::Com2, sensor(), Config::default());
    bench.plugged = false;
    bench.run_for(5_000_000);
    assert_eq!(bench.master.state(), State::Searching);
    assert_eq!(bench.master.inputs(), None);
    let wakes = bench.wake_ups();
    assert!(wakes.len() >= 9, "{} wake-ups in 5 s", wakes.len());
    // Each gap runs from the moment the COM1 test message timed out.
    let given_up: Vec<u64> = bench
        .sent()
        .iter()
        .filter(|s| s.1 == Rate::Com1)
        .map(|s| s.0 + Rate::Com1.micros(58))
        .collect();
    for (index, (end, wake)) in given_up.iter().zip(&wakes[1..]).enumerate() {
        let gap = wake - end;
        if index % 3 == 2 {
            assert!((500_000..=1_000_000).contains(&gap), "sequence gap {gap}");
        } else {
            assert!((30_000..=50_000).contains(&gap), "retry gap {gap}");
        }
    }
    // Three test messages for every wake-up, and nothing else.
    assert_eq!(bench.sent().len(), wakes.len() * 3);
    // The Device shows up: found on the next sequence.
    bench.plugged = true;
    assert!(bench.operate_within(2_000_000));
}

/// IOL Table A.7 and A.8: at least 100 bit times between the starts of
/// two acyclic requests. IOL §7.3.3.3: the cycle is the `MasterCycleTime`
/// the Device was given, -1 % to +10 %.
#[test]
fn recovery_and_cycle_times_are_kept() {
    for (rate, page) in [
        (Rate::Com2, sensor()),
        (Rate::Com3, valve()),
        (Rate::Com1, wide()),
        // A Device far slower than its line: 4,0 ms at COM3.
        (Rate::Com3, page(0x28, 0x11, 0x10, 0x00)),
    ] {
        let mut bench = Bench::new(rate, page, Config::default());
        assert!(bench.operate_within(3_000_000));
        let startup = bench.sent().len();
        bench.run_for(20 * bench.master.cycle_us());
        let sent: Vec<u64> = bench.sent().iter().filter(|s| s.1 == rate).map(|s| s.0).collect();
        let first_cyclic = startup - (bench.sent().len() - sent.len());
        for pair in sent[..first_cyclic].windows(2) {
            assert!(
                pair[1] - pair[0] >= rate.micros(100),
                "{rate:?}: {} us apart",
                pair[1] - pair[0]
            );
        }
        let told = u64::from(cycle_time(bench.device.master_cycle_time()).unwrap()) * 100;
        assert!(told > 0, "{rate:?}: no MasterCycleTime written");
        let min = u64::from(Page1(page.0).min_cycle_time().unwrap()) * 100;
        assert!(told >= min, "{rate:?}: {told} us under the Device's {min}");
        for pair in sent[first_cyclic..].windows(2) {
            let period = pair[1] - pair[0];
            assert!(
                period * 100 >= told * 99 && period * 100 <= told * 110,
                "{rate:?}: a {period} us cycle, the Device was told {told}"
            );
        }
    }
}

#[test]
fn a_slower_cycle_can_be_asked_for() {
    let config = Config {
        cycle_us: 10_000,
        expect: None,
    };
    let mut bench = Bench::new(Rate::Com3, valve(), config);
    assert!(bench.operate_within(1_000_000));
    assert_eq!(bench.master.cycle_us(), 10_000);
    assert_eq!(cycle_time(bench.device.master_cycle_time()), Some(100));
}

/// IOL Table 46 and §A.4.1.1: a message is repeated twice; one bad reply
/// loses nothing.
#[test]
fn one_damaged_reply_is_retried_not_lost() {
    let mut bench = Bench::new(Rate::Com2, sensor(), Config::default());
    bench.device.set_inputs(&[0x11, 0x22]);
    assert!(bench.operate_within(2_000_000));
    for hit in [Hit::Drop, Hit::Flip(1, 3), Hit::Cut(1)] {
        bench.hits = vec![hit.clone(), hit.clone()];
        bench.run_for(4 * bench.master.cycle_us());
        assert_eq!(bench.master.state(), State::Operate, "{hit:?}");
        assert_eq!(bench.master.inputs(), Some(&[0x11, 0x22][..]), "{hit:?}");
    }
    assert_eq!(bench.master.losses(), 0);
    assert_eq!(bench.master.failures(), 6);
}

#[test]
fn a_pulled_device_is_lost_after_three_cycles_and_found_again() {
    let mut bench = Bench::new(Rate::Com2, sensor(), Config::default());
    bench.device.set_inputs(&[0x11, 0x22]);
    assert!(bench.operate_within(2_000_000));
    bench.plugged = false;
    bench.device.power_cycle();
    bench.run_for(2 * bench.master.cycle_us());
    assert_eq!(bench.master.state(), State::Operate, "lost before its retries");
    bench.run_for(2 * bench.master.cycle_us());
    assert_eq!(bench.master.state(), State::Searching);
    assert_eq!(bench.master.inputs(), None);
    assert_eq!(bench.master.losses(), 1);
    bench.plugged = true;
    assert!(bench.operate_within(2_000_000));
    assert_eq!(bench.master.inputs(), Some(&[0x11, 0x22][..]));
}

/// The Master starts over while the Device is still in OPERATE (a
/// Master restart): the test message is of another M-sequence type, the
/// Device goes back to STARTUP (IOL Table 45 T11) and answers the next
/// attempt.
#[test]
fn a_device_left_in_operate_is_taken_over() {
    let mut bench = Bench::new(Rate::Com3, valve(), Config::default());
    assert!(bench.operate_within(1_000_000));
    bench.master = Master::new(Config::default());
    assert!(bench.operate_within(2_000_000), "{:?}", bench.device.mode());
    assert_eq!(bench.device.mode(), Mode::Operate);
}

#[test]
fn inputs_flagged_invalid_are_not_handed_out() {
    let mut bench = Bench::new(Rate::Com2, sensor(), Config::default());
    assert!(bench.operate_within(2_000_000));
    bench.device.set_pd_valid(false);
    bench.device.set_event(true);
    bench.run_for(2 * bench.master.cycle_us());
    assert_eq!(bench.master.state(), State::Operate);
    assert_eq!(bench.master.inputs(), None);
    assert!(bench.master.event());
    bench.device.set_pd_valid(true);
    bench.run_for(2 * bench.master.cycle_us());
    assert!(bench.master.inputs().is_some());
}

#[test]
fn a_device_the_port_cannot_run_is_refused_and_says_why() {
    let other = Config {
        cycle_us: 0,
        expect: Some(Identity {
            vendor: VENDOR,
            device: 7,
        }),
    };
    let mut legacy = sensor();
    legacy.0[4] = 0x10;
    // 3 octets in with OPERATE code 0: the interleaved mode. The
    // simulated Device cannot be built with it either, so it gets a page
    // it can run and the Master is shown the other one.
    let cases = [
        (
            sensor(),
            other,
            Fault::Identity(Identity {
                vendor: VENDOR,
                device: DEVICE,
            }),
        ),
        (legacy, Config::default(), Fault::Revision(0x10)),
    ];
    for (page, config, fault) in cases {
        let mut bench = Bench::new(Rate::Com2, page, config);
        bench.run_for(3_000_000);
        assert_eq!(bench.master.fault(), Some(fault));
        assert_ne!(bench.master.state(), State::Operate);
        assert_eq!(bench.master.inputs(), None);
        assert_eq!(bench.master.page(), None);
        assert_ne!(bench.device.mode(), Mode::Operate);
    }
    // The right Device is plugged in: the fault clears.
    let mut bench = Bench::new(Rate::Com2, sensor(), other);
    bench.run_for(1_000_000);
    bench.master = Master::new(Config {
        expect: Some(Identity {
            vendor: VENDOR,
            device: DEVICE,
        }),
        ..other
    });
    assert!(bench.operate_within(2_000_000));
    assert_eq!(bench.master.fault(), None);
}

#[test]
fn a_page_that_needs_the_interleaved_mode_is_a_layout_fault() {
    assert_eq!(
        Device::new(Rate::Com2, page(0, 0x01, 0x82, 0)).err(),
        Some(Error::Interleaved)
    );
    // Shown to the Master by a line that rewrites the answer to the read
    // of `ProcessDataIn` (MC 0xA5) into 0x82: 0x52 ^ 0x82 = 0xD0, which
    // folds to 0x24.
    let mut bench = Bench::new(Rate::Com2, page(0, 0x01, 0x10, 0), Config::default());
    bench.rewrite = Some((0xA5, vec![0x82, 0x24]));
    bench.run_for(1_000_000);
    assert_ne!(bench.master.state(), State::Operate);
    assert_eq!(bench.master.fault(), Some(Fault::Layout(Error::Interleaved)));
}

fn hits() -> impl Strategy<Value = Vec<Hit>> {
    let hit = prop_oneof![
        3 => Just(Hit::Pass),
        2 => Just(Hit::Drop),
        1 => (0usize..4).prop_map(Hit::Cut),
        2 => (any::<usize>(), 0u8..8).prop_map(|(at, bit)| Hit::Flip(at, bit)),
        2 => prop::collection::vec(any::<u8>(), 0..70).prop_map(Hit::Invent),
    ];
    prop::collection::vec(hit, 0..120)
}

fn devices() -> impl Strategy<Value = (Rate, Page1)> {
    (
        prop::sample::select(vec![Rate::Com1, Rate::Com2, Rate::Com3]),
        prop::sample::select(vec![sensor(), valve(), wide(), page(0x17, 0x01, 0, 0)]),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the line does to the replies, the port never panics,
    /// never hands out inputs outside OPERATE, and is exchanging the
    /// Device's real inputs again once the line is clean.
    #[test]
    fn a_noisy_line_is_survived((rate, page) in devices(), hits in hits(), unplug in any::<bool>()) {
        let mut bench = Bench::new(rate, page, Config::default());
        bench.device.set_inputs(&[0xC3, 0x3C, 0x55, 0xAA]);
        bench.hits = hits;
        let mut steps = 0;
        while !bench.hits.is_empty() && steps < 5_000 {
            bench.step();
            if bench.master.state() != State::Operate {
                prop_assert_eq!(bench.master.inputs(), None);
            }
            steps += 1;
        }
        bench.hits.clear();
        if unplug {
            bench.plugged = false;
            bench.device.power_cycle();
            bench.run_for(1_500_000);
            prop_assert_eq!(bench.master.inputs(), None);
            bench.plugged = true;
        }
        prop_assert!(
            bench.operate_within(6_000_000),
            "not back: {:?}, device {:?}, fault {:?}",
            bench.master.state(), bench.device.mode(), bench.master.fault()
        );
        bench.run_for(3 * bench.master.cycle_us());
        let len = bench.master.inputs().map_or(0, <[u8]>::len);
        prop_assert_eq!(bench.master.inputs(), Some(&[0xC3, 0x3C, 0x55, 0xAA][..len]));
        prop_assert_eq!(bench.master.fault(), None);
    }

    /// The simulated Device answers or stays silent for any octets, any
    /// rate and any order of wake-ups, and never writes past a reply.
    #[test]
    fn the_device_takes_any_octets(
        (rate, page) in devices(),
        messages in prop::collection::vec(
            (prop::collection::vec(any::<u8>(), 0..70), 0u8..4, 0u64..400_000),
            0..60,
        ),
    ) {
        let mut device = Device::new(rate, page).unwrap();
        let mut now = 0;
        let mut out = [0; 80];
        for (message, what, wait) in messages {
            now += wait;
            let rate = match what {
                0 => { device.wake_up(now); continue; }
                1 => Rate::Com1,
                2 => Rate::Com2,
                _ => Rate::Com3,
            };
            if let Some(len) = device.exchange(now, rate, &message, &mut out) {
                prop_assert!((1..=65).contains(&len));
            }
        }
    }
}
