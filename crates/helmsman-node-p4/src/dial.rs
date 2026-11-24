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
use embassy_net::tcp::TcpSocket;
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

/// A port no service of the node listens on (discard), for [`give_up`].
const FORGET_PORT: u16 = 9;

/// Ends a dial nobody answered: the socket forgets where it was going, and
/// the next turn is [`SPACING`] from now.
///
/// `abort` alone is not enough. smoltcp keeps an aborted socket's endpoint
/// until its RST has gone out (`socket/tcp.rs`, `State::Closed` in
/// `dispatch`), and towards an address nothing answers at the RST never
/// goes: the socket asks for that neighbour again every second, for good,
/// and each ask silences every other lookup for a second. On the bench
/// (2026-11-24, a three-device map with one address absent) the node kept
/// scanning and answered nothing: no ping, no API, and a second device on
/// the same wire never connected. Only `listen` clears the endpoint
/// (`reset`), so the socket listens for the length of one poll and is
/// aborted again, which leaves it closed with nothing to send.
///
/// The turn is pushed back from here, not from the dial's start: a dial
/// that takes its whole timeout would otherwise run into the next one, and
/// a few absent devices would hold the stack's one request between them.
pub fn give_up(socket: &mut TcpSocket<'_>) {
    socket.abort();
    let _ = embassy_futures::poll_once(socket.accept(FORGET_PORT));
    socket.abort();
    let now = Instant::now().as_millis();
    NEXT_MS.lock(|next| next.set(next.get().max(now + SPACING.as_millis())));
}
