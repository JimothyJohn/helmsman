//! A PROFINET IO controller on the node: helmsman-profinet's state
//! machine (the one the box's `profinet-io` driver runs), fed from the
//! Ethernet MAC's raw frames ([`crate::ethernet::l2`]) and a UDP socket
//! at the context-manager port, on embassy's clock.
//!
//! [`step`] is one turn: send what the controller has to send, then wait
//! for a frame, a datagram or the controller's next deadline, whichever
//! comes first, and hand it over. A scan loop calls it between scans, or
//! a task of its own loops on it.

use alloc::vec::Vec;

use embassy_futures::select::{Either3, select3};
use embassy_net::udp::UdpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address};
use embassy_time::{Duration, Instant, Timer};
use helmsman_profinet::controller::{Controller, Out};
use helmsman_profinet::eth::ETHERTYPE_PROFINET;

use crate::ethernet::l2;

/// The longest one [`step`] waits when the controller has nothing due:
/// the caller gets to look at its state at least this often.
pub const LONGEST_WAIT: Duration = Duration::from_millis(5);

/// Starts taking PROFINET frames off the MAC. Call once, before the
/// first [`step`].
pub fn listen() {
    l2::listen(ETHERTYPE_PROFINET);
}

/// Microseconds on embassy's clock: the controller's `now`.
#[must_use]
pub fn now() -> u64 {
    Instant::now().as_micros()
}

async fn send(udp: &mut UdpSocket<'_>, out: &mut Vec<Out>) {
    for o in out.drain(..) {
        match o {
            Out::Frame(frame) => {
                // A full queue drops it; the controller's retries and the
                // next cycle cover the loss.
                let _ = l2::send(frame);
            }
            Out::Udp { ip, port, data } => {
                let to = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(ip)), port);
                let _ = udp.send_to(&data, to).await;
            }
        }
    }
}

/// One turn of the controller. `udp` is bound to
/// [`helmsman_profinet::rpc::PORT`]; `buf` holds one datagram (1500
/// bytes is enough).
pub async fn step(controller: &mut Controller, udp: &mut UdpSocket<'_>, buf: &mut [u8], out: &mut Vec<Out>) {
    controller.poll(now(), out);
    send(udp, out).await;
    let wait =
        Duration::from_micros(controller.next_wake(now()).saturating_sub(now()).max(100)).min(LONGEST_WAIT);
    match select3(l2::recv(), udp.recv_from(buf), Timer::after(wait)).await {
        Either3::First(frame) => controller.on_frame(now(), &frame, out),
        Either3::Second(Ok((n, from))) => {
            let IpAddress::Ipv4(ip) = from.endpoint.addr;
            controller.on_udp(now(), ip.octets(), from.endpoint.port, &buf[..n], out);
        }
        Either3::Second(Err(_)) | Either3::Third(()) => {}
    }
    send(udp, out).await;
}
