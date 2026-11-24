//! One turn at a time for the devices that are not answering.
//!
//! smoltcp resolves a neighbour with one ARP request a second for the whole
//! stack (its cache is silent for a second after any request, for every
//! address; the 2026-11-08 run lost its report sink that way). With up to 16
//! devices each retrying every couple of seconds, a few absent ones would
//! keep every other lookup waiting: a device that came back, the report
//! sink, a neighbour entry that expired. A link that has failed more than
//! once calls [`wait_turn`] before it dials again, and the turns are
//! [`SPACING`] apart across the node, so at least every other second the
//! stack's one request is free for whoever needs it.

use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer};

/// Time between two turns, node-wide.
pub const SPACING: Duration = Duration::from_secs(2);

/// When the next turn may start, ms since boot.
static NEXT_MS: Mutex<CriticalSectionRawMutex, Cell<u64>> = Mutex::new(Cell::new(0));

/// Waits for this link's turn to dial, and takes it.
pub async fn wait_turn() {
    loop {
        let now = Instant::now().as_millis();
        let wait = NEXT_MS.lock(|next| {
            if now >= next.get() {
                next.set(now + SPACING.as_millis());
                None
            } else {
                Some(next.get() - now)
            }
        });
        match wait {
            None => return,
            Some(ms) => Timer::after(Duration::from_millis(ms)).await,
        }
    }
}
