//! The I/O tasks: one a device of the node's map (up to 16), each on its
//! own so the scan loop never waits on the network (the modbus binary ran
//! both in one loop and scanned at 30 ms for a 10 ms period, 2026-11-08),
//! and one more that carries the cyclic packets of every EtherNet/IP
//! device on the node's one I/O socket.
//!
//! A Modbus TCP device (`device_task`): each round reads what the map names
//! from the device into the shared image, writes the shared outputs to it,
//! and records the link's state. The device's own comms-loss safe state
//! covers the node going silent; a link that is down leaves the last
//! inputs in the image and the outputs where the program put them, as the
//! one-block node always did.
//!
//! An EtherNet/IP device (`device_task`, then `enip_io_task`): the device's
//! task owns the explicit session (TCP connect, `RegisterSession`,
//! `Forward_Open` and, after a loss, `Forward_Close`), which can take its
//! whole timeout against a device that is not there (on the bench, 2026-11-10,
//! two opens timed out inside the scan loop and 199 scans were late). Once the
//! connection is open it hands its producer and consumer to [`Links`], and
//! `enip_io_task` sends each device's output assembly every RPI from the
//! shared image, hands each T->O packet to the device it came from, and
//! flags a connection that has heard nothing for its timeout. The device's
//! task sees the flag, closes, waits and opens again with the outputs the
//! image holds.
//!
//! The tasks share only the image and the link states (`state::Live`) and
//! the connections (`Links`), each held for a copy and released; no task
//! waits on another.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use core::time::Duration as CoreDuration;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_println::println;
use helmsman_modbus::{READ_HOLDING_REGISTERS, READ_INPUT_REGISTERS};
use helmsman_nodecore::iomap::{IoMap, MAX_ASSEMBLY, MAX_DEVICES};
use helmsman_nodecore::ioplan::{EnipPlan, ModbusPlan, Plan, ReadKind, WriteKind};
use helmsman_proto::class1::{Consumer, IoConfig, Producer};
use static_cell::{ConstStaticCell, StaticCell};

use crate::enip::{Closer, IoConnection, Session};
use crate::modbus;
use crate::state::{self, Shared};

/// A Modbus request that takes longer than this is a link error.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
/// An EtherNet/IP explicit exchange that takes longer than this is a link
/// error.
pub const ENIP_REQUEST_TIMEOUT: Duration = Duration::from_millis(1000);
/// Shortest Modbus round: a device answers in about 15 ms, so this never
/// binds on the bench; it keeps a silent device from being hammered.
pub const MIN_ROUND: Duration = Duration::from_millis(5);
/// After a lost or refused connection, how long before the next open.
pub const REOPEN_BACKOFF: Duration = Duration::from_secs(2);
/// How often a connected device's task looks for a loss flag.
const WATCH: Duration = Duration::from_millis(50);
/// How often the cyclic task wakes when no packet arrives: the finest
/// RPI it keeps is [`helmsman_nodecore::iomap::MIN_RPI_MS`], so 5 ms.
const TICK: Duration = Duration::from_millis(5);
/// The socket buffers of one device's link: a Modbus frame is at most 260
/// bytes and a `Forward_Open` exchange under 200 (a longer message is sent
/// in pieces).
pub const BUF: usize = 320;

const _: () = assert!(MAX_DEVICES == 16, "the task pool below is MAX_DEVICES");

static RX: ConstStaticCell<[[u8; BUF]; MAX_DEVICES]> = ConstStaticCell::new([[0; BUF]; MAX_DEVICES]);
static TX: ConstStaticCell<[[u8; BUF]; MAX_DEVICES]> = ConstStaticCell::new([[0; BUF]; MAX_DEVICES]);
static IO_RX_META: StaticCell<[PacketMetadata; 16]> = StaticCell::new();
static IO_TX_META: StaticCell<[PacketMetadata; 16]> = StaticCell::new();
static IO_RX: ConstStaticCell<[u8; 4096]> = ConstStaticCell::new([0; 4096]);
static IO_TX: ConstStaticCell<[u8; 2048]> = ConstStaticCell::new([0; 2048]);

/// An open Class 1 connection as the cyclic task holds it.
struct Cyclic {
    producer: Producer,
    consumer: Consumer,
    /// Where O->T goes.
    target: IpEndpoint,
    /// The device's address: the T->O packets that count come from it.
    host: IpAddress,
    rpi: Duration,
    timeout: Duration,
    next_produce: Instant,
    last_input: Instant,
}

struct Slots {
    open: [Option<Cyclic>; MAX_DEVICES],
    /// Why a connection was dropped, until its device's task has seen it.
    lost: [Option<String>; MAX_DEVICES],
}

/// The open EtherNet/IP connections, one slot a device.
pub struct Links {
    slots: Mutex<CriticalSectionRawMutex, RefCell<Slots>>,
}

impl Links {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Mutex::new(RefCell::new(Slots {
                open: [const { None }; MAX_DEVICES],
                lost: [const { None }; MAX_DEVICES],
            })),
        }
    }

    fn install(&self, k: usize, c: Cyclic) {
        self.slots.lock(|cell| {
            let mut s = cell.borrow_mut();
            s.lost[k] = None;
            s.open[k] = Some(c);
        });
    }

    /// Why the cyclic task dropped device `k`'s connection, once.
    fn take_lost(&self, k: usize) -> Option<String> {
        self.slots.lock(|cell| cell.borrow_mut().lost[k].take())
    }
}

impl Default for Links {
    fn default() -> Self {
        Self::new()
    }
}

/// What every I/O task needs; built once at boot from the map.
pub struct Ctx {
    pub shared: &'static Shared,
    pub map: &'static IoMap,
    /// One plan a device of the map.
    pub plans: &'static [Plan],
    pub links: &'static Links,
    pub mac: [u8; 6],
}

/// Spawns a task for each device of the map, and the cyclic task if any
/// device is EtherNet/IP.
///
/// # Errors
/// Which task would not spawn.
pub fn spawn_all(spawner: Spawner, stack: Stack<'static>, ctx: &'static Ctx) -> Result<(), &'static str> {
    let (rx, tx) = (RX.take(), TX.take());
    for (k, (rx, tx)) in rx
        .iter_mut()
        .zip(tx.iter_mut())
        .enumerate()
        .take(ctx.map.devices.len())
    {
        let task = device_task(k, stack, ctx, rx, tx).map_err(|_| "a device's I/O task would not spawn")?;
        spawner.spawn(task);
    }
    if ctx.plans.iter().any(|p| matches!(p, Plan::Enip(_))) {
        let task = enip_io_task(stack, ctx).map_err(|_| "the EtherNet/IP I/O task would not spawn")?;
        spawner.spawn(task);
    }
    Ok(())
}

/// Drives device `k` for the life of the node.
#[embassy_executor::task(pool_size = 16)]
async fn device_task(
    k: usize,
    stack: Stack<'static>,
    ctx: &'static Ctx,
    rx: &'static mut [u8],
    tx: &'static mut [u8],
) -> ! {
    let device = &ctx.map.devices[k];
    let host = Ipv4Address::from_octets(device.host);
    match &ctx.plans[k] {
        Plan::Modbus(plan) => {
            let endpoint = IpEndpoint::new(IpAddress::Ipv4(host), device.port);
            modbus_loop(k, stack, ctx, plan, endpoint, rx, tx).await
        }
        Plan::Enip(plan) => enip_loop(k, stack, ctx, plan, host, rx, tx).await,
    }
}

fn set_up(shared: &Shared, k: usize, reconnects: u32) {
    state::with(shared, |live| {
        let d = &mut live.devices[k];
        d.up = Some(true);
        d.error = None;
        d.reconnects = reconnects;
    });
}

fn set_down(shared: &Shared, k: usize, error: String, reconnects: u32) {
    state::with(shared, |live| {
        let d = &mut live.devices[k];
        d.up = Some(false);
        d.error = Some(error);
        d.reconnects = reconnects;
    });
}

/// What a read of the device answered.
enum Got {
    Bits(Vec<bool>),
    Regs(Vec<u16>),
}

async fn modbus_loop(
    k: usize,
    stack: Stack<'static>,
    ctx: &'static Ctx,
    plan: &'static ModbusPlan,
    endpoint: IpEndpoint,
    rx: &'static mut [u8],
    tx: &'static mut [u8],
) -> ! {
    let shared = ctx.shared;
    let mut client = modbus::Client::new(stack, endpoint, plan.unit, REQUEST_TIMEOUT, rx, tx);
    loop {
        if plan.reads.is_empty() && plan.writes.is_empty() {
            // A device the map names no point of: nothing to say about its link.
            Timer::after(Duration::from_secs(1)).await;
            continue;
        }
        // No dialling into a link that is down: a fixed address is
        // configured before the PHY has negotiated, and at boot the two
        // attempts made by then cost a back-off each (the P4, 2026-11-10:
        // the block came up 3.1 or 5.5 s after start, by luck).
        stack.wait_link_up().await;
        let round = Instant::now();
        let mut failed: Option<modbus::Error> = None;
        for read in &plan.reads {
            let got = match read.kind {
                ReadKind::Discrete => client
                    .read_discrete_inputs(read.start, read.qty)
                    .await
                    .map(Got::Bits),
                ReadKind::Input => client
                    .read_registers(READ_INPUT_REGISTERS, read.start, read.qty)
                    .await
                    .map(Got::Regs),
                ReadKind::Holding => client
                    .read_registers(READ_HOLDING_REGISTERS, read.start, read.qty)
                    .await
                    .map(Got::Regs),
            };
            match got {
                Ok(Got::Bits(bits)) => state::with(shared, |live| read.store_bits(&bits, &mut live.inputs)),
                Ok(Got::Regs(regs)) => state::with(shared, |live| read.store_regs(&regs, &mut live.inputs)),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        let outputs = state::with(shared, |live| live.outputs);
        let mut write_failed: Option<modbus::Error> = None;
        for write in &plan.writes {
            let done = match write.kind {
                WriteKind::Coils => client.write_coils(write.start, &write.coils(&outputs)).await,
                WriteKind::Holding => {
                    client
                        .write_registers(write.start, &write.registers(&outputs))
                        .await
                }
            };
            if let Err(e) = done {
                write_failed = Some(e);
                break;
            }
        }
        let reconnects = client.connects.saturating_sub(1);
        match failed.or(write_failed) {
            // Backing off is the link being down, said once.
            Some(e) => set_down(shared, k, e.to_string(), reconnects),
            None => set_up(shared, k, reconnects),
        }
        let took = round.elapsed();
        if took < MIN_ROUND {
            Timer::after(MIN_ROUND - took).await;
        }
    }
}

/// The connection an `enip-io` device's line asks for.
fn io_config(plan: &EnipPlan, mac: [u8; 6]) -> IoConfig {
    let link = &plan.link;
    let rpi = CoreDuration::from_millis(u64::from(link.rpi_ms));
    let mut config = if link.input_only {
        IoConfig::input_only(link.configuration, link.o_to_t, link.t_to_o, link.t_to_o_len, rpi)
    } else {
        let mut c = IoConfig::exclusive_owner(
            link.configuration,
            link.o_to_t,
            link.t_to_o,
            link.o_to_t_len,
            link.t_to_o_len,
            rpi,
        );
        c.o_to_t_run_idle = link.o_to_t_run_idle;
        c
    };
    // The E1213's and the MDrive's T->O are modeless: no run/idle header.
    config.t_to_o_run_idle = false;
    config.timeout_multiplier = link.multiplier;
    config.config_data.clone_from(&link.config_data);
    config.originator_serial = crate::board::originator_serial(mac);
    config
}

async fn enip_loop(
    k: usize,
    stack: Stack<'static>,
    ctx: &'static Ctx,
    plan: &'static EnipPlan,
    host: Ipv4Address,
    rx: &'static mut [u8],
    tx: &'static mut [u8],
) -> ! {
    let shared = ctx.shared;
    let mut session = Session::new(stack, host, ENIP_REQUEST_TIMEOUT, rx, tx);
    let mut closer: Option<Closer> = None;
    let mut opens: u32 = 0;
    let mut attempts: u16 = 0;
    let mut reopen_at = Instant::now();
    loop {
        // A fixed address is "up" before the PHY has negotiated; an open
        // sent into a link that is still down only times out.
        stack.wait_link_up().await;
        if closer.is_some() {
            Timer::after(WATCH).await;
            if let Some(why) = ctx.links.take_lost(k) {
                println!("device {}: {why}", ctx.map.devices[k].id);
                set_down(shared, k, why, opens.saturating_sub(1));
                // Best effort: the device's own timeout ends it otherwise.
                if let Some(c) = closer.take() {
                    let _ = with_timeout(Duration::from_secs(3), c.close(&mut session)).await;
                }
                reopen_at = Instant::now() + REOPEN_BACKOFF;
            }
            continue;
        }
        if Instant::now() < reopen_at {
            Timer::after(WATCH).await;
            continue;
        }
        // A connection serial of its own, so no two devices' (or one
        // device's two) connections are taken for each other.
        attempts = attempts.wrapping_add(1);
        let serial = (u16::try_from(k + 1).unwrap_or(1) << 8) | (attempts & 0xFF);
        let config = io_config(plan, ctx.mac);
        let (rpi, timeout) = (
            Duration::from_millis(u64::from(plan.link.rpi_ms)),
            Duration::from_micros(u64::try_from(config.timeout().as_micros()).unwrap_or(u64::MAX)),
        );
        let mut output = alloc::vec![0u8; usize::from(plan.link.o_to_t_len)];
        let image = state::with(shared, |live| live.outputs);
        plan.fill_outputs(&image, &mut output);
        match IoConnection::open(&mut session, config, serial, &output).await {
            Ok(connection) => {
                let (producer, consumer, target, close) = connection.split();
                let now = Instant::now();
                ctx.links.install(
                    k,
                    Cyclic {
                        producer,
                        consumer,
                        target,
                        host: IpAddress::Ipv4(host),
                        rpi,
                        timeout,
                        next_produce: now,
                        last_input: now,
                    },
                );
                closer = Some(close);
                opens += 1;
                set_up(shared, k, opens.saturating_sub(1));
            }
            Err(e) => {
                set_down(shared, k, e.to_string(), opens.saturating_sub(1));
                reopen_at = Instant::now() + REOPEN_BACKOFF;
            }
        }
    }
}

/// The node's one I/O socket (UDP 2222): every EtherNet/IP device's T->O
/// packets come to it and its O->T packets go from it.
#[embassy_executor::task]
async fn enip_io_task(stack: Stack<'static>, ctx: &'static Ctx) -> ! {
    let mut socket = UdpSocket::new(
        stack,
        IO_RX_META.init([PacketMetadata::EMPTY; 16]),
        IO_RX.take(),
        IO_TX_META.init([PacketMetadata::EMPTY; 16]),
        IO_TX.take(),
    );
    while socket.bind(helmsman_proto::encap::IO_PORT).is_err() {
        println!(
            "io: udp {} bind failed; trying again",
            helmsman_proto::encap::IO_PORT
        );
        Timer::after(Duration::from_secs(5)).await;
    }
    let mut buf = [0u8; 512];
    loop {
        match select(socket.recv_from(&mut buf), Timer::after(TICK)).await {
            Either::First(Ok((n, meta))) => take_packet(ctx, meta.endpoint.addr, &buf[..n]),
            Either::First(Err(_)) | Either::Second(()) => {}
        }
        let due = produce(ctx);
        for (to, packet) in due {
            let _ = socket.send_to(&packet, to).await;
        }
    }
}

/// Gives a T->O datagram to the connection of the device it came from;
/// what that connection takes goes into the shared image.
fn take_packet(ctx: &Ctx, from: IpAddress, datagram: &[u8]) {
    let taken: Option<(usize, Vec<u8>)> = ctx.links.slots.lock(|cell| {
        let mut slots = cell.borrow_mut();
        for (k, slot) in slots.open.iter_mut().enumerate() {
            let Some(c) = slot.as_mut().filter(|c| c.host == from) else {
                continue;
            };
            let Plan::Enip(plan) = &ctx.plans[k] else { continue };
            let got = c
                .consumer
                .accept(datagram)
                .filter(|r| r.data.len() >= usize::from(plan.link.t_to_o_len))
                .map(|r| r.data.clone());
            if got.is_some() {
                c.last_input = Instant::now();
            }
            return got.map(|data| (k, data));
        }
        None
    });
    if let Some((k, data)) = taken
        && let Plan::Enip(plan) = &ctx.plans[k]
    {
        state::with(ctx.shared, |live| plan.store_inputs(&data, &mut live.inputs));
    }
}

/// The O->T packets that are due now (each device's output assembly from
/// the image), and the connections that have heard nothing for their
/// timeout dropped and flagged for their tasks.
fn produce(ctx: &Ctx) -> Vec<(IpEndpoint, Vec<u8>)> {
    let now = Instant::now();
    let any_due = ctx
        .links
        .slots
        .lock(|cell| cell.borrow().open.iter().flatten().any(|c| now >= c.next_produce));
    let outputs = any_due.then(|| state::with(ctx.shared, |live| live.outputs));
    ctx.links.slots.lock(|cell| {
        let mut slots = cell.borrow_mut();
        let mut packets = Vec::new();
        let mut dropped: Vec<(usize, u64)> = Vec::new();
        for (k, slot) in slots.open.iter_mut().enumerate() {
            let Some(c) = slot else { continue };
            let Plan::Enip(plan) = &ctx.plans[k] else { continue };
            if let (Some(image), true) = (&outputs, now >= c.next_produce) {
                let mut bytes = [0u8; MAX_ASSEMBLY as usize];
                let len = usize::from(plan.link.o_to_t_len).min(bytes.len());
                plan.fill_outputs(image, &mut bytes[..len]);
                let _ = c.producer.set_output(&bytes[..len]);
                packets.push((c.target, c.producer.next_packet()));
                c.next_produce += c.rpi;
                if c.next_produce < now {
                    c.next_produce = now + c.rpi;
                }
            }
            if now.saturating_duration_since(c.last_input) > c.timeout {
                dropped.push((k, c.timeout.as_millis()));
            }
        }
        for (k, ms) in dropped {
            slots.open[k] = None;
            slots.lost[k] = Some(alloc::format!("no T->O data for {ms} ms"));
        }
        packets
    })
}
