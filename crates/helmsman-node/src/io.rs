//! The I/O task: the Modbus TCP link to the one remote block, run on its
//! own so the scan loop never waits on the network (the modbus binary ran
//! both in one loop and scanned at 30 ms for a 10 ms period, 2026-11-08).
//! Each round: read the block's discrete inputs into the shared image,
//! write the shared outputs to its coils, record the link's state. The
//! block's own comms-loss safe state covers the node going silent.

use alloc::string::ToString;

use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

use crate::modbus;
use crate::state::{self, Shared};

/// The E1213's digital inputs read each round (DI-00..07 in byte 0).
pub const INPUTS: u16 = 8;
/// Coils written each round (DO-00..03).
pub const COILS: u16 = 4;
/// A request that takes longer than this is a link error.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
/// Shortest round: the block answers in about 15 ms, so this never binds
/// on the bench; it keeps a silent block from being hammered.
pub const MIN_ROUND: Duration = Duration::from_millis(5);

static RX: StaticCell<[u8; 512]> = StaticCell::new();
static TX: StaticCell<[u8; 512]> = StaticCell::new();

/// Drives the block for the life of the node.
#[embassy_executor::task]
pub async fn io_task(stack: Stack<'static>, block: IpEndpoint, unit: u8, shared: &'static Shared) -> ! {
    let mut client = modbus::Client::new(
        stack,
        block,
        unit,
        REQUEST_TIMEOUT,
        RX.init([0; 512]),
        TX.init([0; 512]),
    );
    loop {
        let round = embassy_time::Instant::now();
        let read = client.read_discrete_inputs(0, INPUTS).await;
        let outputs = state::with(shared, |live| {
            if let Ok(bits) = &read {
                let mut b = 0u8;
                for (i, on) in bits.iter().take(8).enumerate() {
                    if *on {
                        b |= 1 << i;
                    }
                }
                live.inputs[0] = b;
            }
            live.outputs
        });
        let q = outputs[0];
        let coils = [q & 1 == 1, q & 2 == 2, q & 4 == 4, q & 8 == 8];
        let write = client.write_coils(0, &coils[..usize::from(COILS)]).await;
        state::with(shared, |live| {
            let connects = client.connects;
            live.device.reconnects = connects.saturating_sub(1);
            match (&read, &write) {
                (Ok(_), Ok(())) => {
                    live.device.up = Some(true);
                    live.device.error = None;
                }
                (Err(e), _) | (_, Err(e)) => {
                    // Backing off is the link being down, said once.
                    live.device.up = Some(false);
                    live.device.error = Some(e.to_string());
                }
            }
        });
        let took = round.elapsed();
        if took < MIN_ROUND {
            Timer::after(MIN_ROUND - took).await;
        }
    }
}
