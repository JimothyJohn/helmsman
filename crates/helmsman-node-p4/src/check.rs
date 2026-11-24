//! The comms check, asked from the node (`helmsman_nodecore::check` has
//! the record and the rule): one task that waits for a map to check,
//! then asks each of its devices in turn on a socket of its own and
//! writes what each said into the shared state as it goes, where
//! `GET /api/io_map/check` and the page read it.
//!
//! What is asked: a Modbus TCP device is connected to and, when the map
//! names something to read from it, asked for the first of those reads;
//! an EtherNet/IP device is connected to and a session is registered and
//! dropped. Nothing is written and no I/O connection is opened, so the
//! check runs beside the node's own I/O without moving an output, and a
//! device this node already owns answers it like any other.
//!
//! A device on another subnet is not dialled at all (it would only time
//! out): it is failed with the reason and the address it needs. After the
//! first device that does not answer, each further dial waits its turn
//! (`dial::wait_turn`), so a map of absent devices cannot hold the
//! stack's one neighbour request a second for the length of the check.

use alloc::string::ToString;

use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant};
use esp_println::println;
use helmsman_modbus::{READ_HOLDING_REGISTERS, READ_INPUT_REGISTERS};
use helmsman_nodecore::check::{Verdict, off_subnet};
use helmsman_nodecore::iomap::{self, IoMap, Link};
use helmsman_nodecore::ioplan::{Plan, ReadKind};
use helmsman_nodecore::settings::Settings;

use crate::enip::Session;
use crate::state::{self, Shared};
use crate::{dial, io, modbus};

/// Raised by the server once the check's record is in the shared state.
pub static START: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// A connect or a request that takes longer than this is no answer.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// Runs the checks for the life of the node, one at a time.
#[embassy_executor::task]
pub async fn check_task(stack: Stack<'static>, shared: &'static Shared, settings: &'static Settings) -> ! {
    let mut rx = [0u8; io::BUF];
    let mut tx = [0u8; io::BUF];
    loop {
        START.wait().await;
        let text = state::with(shared, |live| live.check.text.clone());
        // The server parsed it before it started the check; a text that
        // does not parse here leaves every device unasked, which fails.
        if let Ok(map) = iomap::parse(&text) {
            println!("check: asking {} device(s)", map.devices.len());
            let mut silent = false;
            for k in 0..map.devices.len() {
                let verdict = ask(stack, settings, &map, k, &mut rx, &mut tx, silent).await;
                match &verdict {
                    Verdict::Answered { ms } => println!("check: {} answered in {ms} ms", map.devices[k].id),
                    Verdict::Failed(why) => {
                        silent = true;
                        println!("check: {} did not answer: {why}", map.devices[k].id);
                    }
                    Verdict::Waiting => {}
                }
                state::with(shared, |live| live.check.set(k, verdict));
            }
        }
        state::with(shared, |live| live.check.finish(Instant::now().as_millis()));
    }
}

/// Asks device `k` of `map`; `wait` when a device before it was silent.
async fn ask(
    stack: Stack<'static>,
    settings: &Settings,
    map: &IoMap,
    k: usize,
    rx: &mut [u8],
    tx: &mut [u8],
    wait: bool,
) -> Verdict {
    let device = &map.devices[k];
    if let Some(why) = off_subnet(&settings.net, device.host) {
        return Verdict::Failed(why);
    }
    if !stack.is_link_up() {
        return Verdict::Failed("this node's own network link is down".to_string());
    }
    if wait {
        dial::wait_turn().await;
    }
    let host = Ipv4Address::from_octets(device.host);
    let started = Instant::now();
    let said = match (&device.link, map.plan(k)) {
        (Link::Modbus { unit }, Plan::Modbus(plan)) => {
            let endpoint = IpEndpoint::new(IpAddress::Ipv4(host), device.port);
            let mut client = modbus::Client::new(stack, endpoint, *unit, TIMEOUT, rx, tx);
            let said = match plan.reads.first() {
                Some(read) => match read.kind {
                    ReadKind::Discrete => client.read_discrete_inputs(read.start, read.qty).await.map(drop),
                    ReadKind::Input => client
                        .read_registers(READ_INPUT_REGISTERS, read.start, read.qty)
                        .await
                        .map(drop),
                    ReadKind::Holding => client
                        .read_registers(READ_HOLDING_REGISTERS, read.start, read.qty)
                        .await
                        .map(drop),
                },
                // Nothing the map reads from it: the connection is the answer.
                None => client.dial().await,
            };
            client.hang_up().await;
            said.map_err(|e| e.to_string())
        }
        (Link::Enip(_), _) => {
            let mut session = Session::new(stack, host, TIMEOUT, rx, tx);
            let said = session.register().await;
            session.hang_up().await;
            said.map_err(|e| e.to_string())
        }
        (Link::Modbus { .. }, Plan::Enip(_)) => Err("the map's plan and its device disagree".to_string()),
    };
    match said {
        Ok(()) => Verdict::Answered {
            ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
        },
        Err(why) => Verdict::Failed(why),
    }
}
