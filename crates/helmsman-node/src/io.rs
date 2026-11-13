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

/// Most discrete inputs or coils a round moves: the image bytes the map
/// may name (`helmsman_nodecore::iomap::MAX_POINTS`).
pub const MAX_POINTS: u16 = 16;
/// A request that takes longer than this is a link error.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(200);
/// Shortest round: the block answers in about 15 ms, so this never binds
/// on the bench; it keeps a silent block from being hammered.
pub const MIN_ROUND: Duration = Duration::from_millis(5);

static RX: StaticCell<[u8; 512]> = StaticCell::new();
static TX: StaticCell<[u8; 512]> = StaticCell::new();

/// Drives the block for the life of the node: `inputs` discrete inputs
/// read from 0 and `coils` coils written from 0 each round (the map's
/// counts, card node-io-map), both capped at [`MAX_POINTS`]; a count of
/// zero skips that half.
#[embassy_executor::task]
pub async fn io_task(
    stack: Stack<'static>,
    block: IpEndpoint,
    unit: u8,
    inputs: u16,
    coils: u16,
    shared: &'static Shared,
) -> ! {
    let mut client = modbus::Client::new(
        stack,
        block,
        unit,
        REQUEST_TIMEOUT,
        RX.init([0; 512]),
        TX.init([0; 512]),
    );
    let inputs = inputs.min(MAX_POINTS);
    let coils = coils.min(MAX_POINTS);
    loop {
        // No dialling into a link that is down: a fixed address is
        // configured before the PHY has negotiated, and at boot the two
        // attempts made by then cost a back-off each (the P4, 2026-11-10:
        // the block came up 3.1 or 5.5 s after start, by luck).
        stack.wait_link_up().await;
        let round = embassy_time::Instant::now();
        let read = if inputs == 0 {
            Ok(alloc::vec::Vec::new())
        } else {
            client.read_discrete_inputs(0, inputs).await
        };
        let outputs = state::with(shared, |live| {
            if let Ok(bits) = &read {
                let mut bytes = [0u8; 2];
                for (i, on) in bits.iter().take(usize::from(MAX_POINTS)).enumerate() {
                    if *on {
                        bytes[i / 8] |= 1 << (i % 8);
                    }
                }
                live.inputs[0] = bytes[0];
                live.inputs[1] = bytes[1];
            }
            live.outputs
        });
        let mut coil_bits = [false; 16];
        for (i, c) in coil_bits.iter_mut().enumerate() {
            *c = outputs[i / 8] & (1 << (i % 8)) != 0;
        }
        let write = if coils == 0 {
            Ok(())
        } else {
            client.write_coils(0, &coil_bits[..usize::from(coils)]).await
        };
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
