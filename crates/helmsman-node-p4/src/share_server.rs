//! The Skipper's own Modbus TCP server (docs/EDGE.md, "Sharing values"):
//! [`CONNECTIONS`] at a time on port 502, started only when the map has a
//! `share` line, so a node that shares nothing opens no port. Each
//! connection reads whole frames, hands them to helmsman-nodecore's
//! sans-IO `modbus_server::serve` and writes the reply; the bytes of
//! Modbus and the rules of what may be read and written are there and
//! host-tested, this is the socket.
//!
//! The scan loop never waits on any of this. The server and the scan loop
//! meet at the share bank in [`crate::state::Live`], each holding it for
//! the length of a copy: a request is answered from a copy taken under the
//! lock, and the writes it approved are committed under the lock after.
//! A slow, silent or hostile peer costs a connection slot and nothing
//! else: a frame must complete within [`FRAME_TIMEOUT`], a quiet
//! connection is closed after [`IDLE_TIMEOUT`], a peer that does not
//! read its replies is dropped after [`WRITE_TIMEOUT`], and a stream that
//! is not Modbus TCP is closed at its first header.

use alloc::vec;
use alloc::vec::Vec;

use embassy_futures::yield_now;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::Write;
use esp_println::println;
use helmsman_nodecore::modbus_server::{self, Head, MAX_FRAME};

use crate::state::{self, Shared};

/// Connections served at once: a PLC polling and a Skipper beside it. A
/// third is refused by the stack (nothing is listening for it).
pub const CONNECTIONS: usize = 2;
/// A frame that has begun must be whole within this.
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(3);
/// A connection that sends nothing for this long is closed. A master
/// polling more slowly than this reconnects, which Modbus TCP allows.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A reply the peer does not take within this ends the connection.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// What the connections need: the shared state and the shares (a copy of
/// the map's, which the scan loop and the server both index by position).
pub struct Ctx {
    pub shared: &'static Shared,
    pub shares: &'static [helmsman_nodecore::iomap::Share],
}

/// Serves connections on port 502 for the life of the node;
/// [`CONNECTIONS`] run, and none is spawned for a map without shares.
#[embassy_executor::task(pool_size = CONNECTIONS)]
pub async fn share_task(stack: Stack<'static>, ctx: &'static Ctx) -> ! {
    // A frame is at most 260 bytes; the buffers hold one with room.
    let mut rx = vec![0u8; 512];
    let mut tx = vec![0u8; 512];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    // Keep-alive probes end a connection whose peer vanished.
    socket.set_keep_alive(Some(Duration::from_secs(15)));
    socket.set_timeout(Some(Duration::from_secs(30)));
    loop {
        if socket.accept(modbus_server::PORT).await.is_err() {
            Timer::after(Duration::from_millis(100)).await;
            socket.abort();
            continue;
        }
        serve(&mut socket, ctx).await;
        socket.close();
        let _ = with_timeout(Duration::from_millis(500), socket.flush()).await;
        socket.abort();
    }
}

async fn serve(socket: &mut TcpSocket<'_>, ctx: &Ctx) {
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_FRAME);
    let mut chunk = [0u8; 128];
    loop {
        // Every whole frame in the buffer, in order.
        loop {
            match modbus_server::head(&buf) {
                Head::Frame(n) => {
                    let answered = answer(socket, ctx, &buf[..n]).await;
                    buf.drain(..n);
                    if !answered {
                        return;
                    }
                    // A peer that never stops sending must not hold the
                    // executor from the scan loop.
                    yield_now().await;
                }
                Head::Need(_) => break,
                Head::Refuse(why) => {
                    println!("modbus server: closing a connection ({why})");
                    return;
                }
            }
        }
        let wait = if buf.is_empty() {
            IDLE_TIMEOUT
        } else {
            FRAME_TIMEOUT
        };
        match with_timeout(wait, socket.read(&mut chunk)).await {
            Ok(Ok(0) | Err(_)) | Err(_) => return,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Answers one frame; false if the connection should end.
async fn answer(socket: &mut TcpSocket<'_>, ctx: &Ctx, request: &[u8]) -> bool {
    // A copy of the bank under the lock, the answer outside it.
    let bank = state::with(ctx.shared, |live| live.bank.clone());
    let Some(reply) = modbus_server::serve(ctx.shares, &bank, request) else {
        return false;
    };
    if !reply.writes.is_empty() {
        state::with(ctx.shared, |live| live.bank.apply(ctx.shares, &reply.writes));
    }
    matches!(
        with_timeout(WRITE_TIMEOUT, socket.write_all(&reply.frame)).await,
        Ok(Ok(()))
    )
}
