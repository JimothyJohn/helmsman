//! The transport-free half of an originator's Class 1 I/O connection: what
//! to ask for ([`IoConfig`]), the O->T packets to send ([`Producer`]), the
//! T->O packets to accept ([`Consumer`]), and the two small decisions a
//! `Forward_Open` reply forces (where O->T goes, what closes it later).
//! `no_std`: no socket, no thread, no clock. [`crate::scanner`] runs it on
//! `std` sockets and threads; the edge node (crates/helmsman-node,
//! docs/EDGE.md) runs the same code on embassy-net, so a node and a box
//! put the same bytes on the wire.
//!
//! Addressing follows what `OpENer` does for point-to-point
//! (`cipioconnection.c` `OpenProducingPointToPointConnection` /
//! `OpenConsumingPointToPointConnection`): the target sends T->O to the
//! IP it saw the TCP session come from, UDP port 2222, and only accepts
//! O->T packets from that same IP. Timeout semantics: RPI × 2^(multiplier
//! + 2) (`cipconnectionobject.c`), applied to the T->O direction.

use alloc::vec::Vec;
use core::fmt;
use core::net::{Ipv4Addr, SocketAddrV4};
use core::time::Duration;

use crate::codec::{Reader, Writer};
use crate::cpf;
use crate::explicit;
use crate::identity::SocketAddress;
use crate::io::{
    Class1Body, ConnectionPath, ElectronicKey, ForwardClose, ForwardOpen, IoPacket, NetworkParams, Priority,
    Transport, class1_size,
};

/// What the originator asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoConfig {
    /// Configuration assembly instance.
    pub configuration: u16,
    /// O->T connection point (the target's output/consumed assembly, or its
    /// heartbeat instance for `Input Only` / `Listen Only`).
    pub consumed: u16,
    /// T->O connection point (the target's input/produced assembly).
    pub produced: u16,
    /// Application bytes per O->T packet; 0 makes it a heartbeat.
    pub output_len: u16,
    /// Application bytes per T->O packet.
    pub input_len: u16,
    pub rpi: Duration,
    /// Inactivity timeout = RPI × 2^(multiplier + 2).
    pub timeout_multiplier: u8,
    /// Prepend the 32-bit run/idle header to O->T data. `OpENer` builds
    /// with this on (`OPENER_CONSUMED_DATA_HAS_RUN_IDLE_HEADER` default
    /// TRUE); Logix controllers send it too.
    pub o_to_t_run_idle: bool,
    /// Expect a run/idle header on T->O data. `OpENer` default is off.
    pub t_to_o_run_idle: bool,
    pub priority: Priority,
    pub originator_vendor_id: u16,
    pub originator_serial: u32,
    pub key: Option<ElectronicKey>,
    pub config_data: Vec<u8>,
}

impl IoConfig {
    /// `Exclusive Owner`: bidirectional, this originator owns the output.
    #[must_use]
    pub fn exclusive_owner(
        configuration: u16,
        consumed: u16,
        produced: u16,
        output_len: u16,
        input_len: u16,
        rpi: Duration,
    ) -> Self {
        Self {
            configuration,
            consumed,
            produced,
            output_len,
            input_len,
            rpi,
            timeout_multiplier: 1,
            o_to_t_run_idle: true,
            t_to_o_run_idle: false,
            priority: Priority::Scheduled,
            originator_vendor_id: 0xFFFF,
            originator_serial: 0,
            key: None,
            config_data: Vec::new(),
        }
    }

    /// `Input Only`: heartbeat O->T to `heartbeat` (PUB00213R0 glossary p.24),
    /// data T->O from `produced`.
    #[must_use]
    pub fn input_only(
        configuration: u16,
        heartbeat: u16,
        produced: u16,
        input_len: u16,
        rpi: Duration,
    ) -> Self {
        Self::exclusive_owner(configuration, heartbeat, produced, 0, input_len, rpi)
    }

    fn rpi_us(&self) -> u32 {
        u32::try_from(self.rpi.as_micros()).unwrap_or(u32::MAX)
    }

    /// The `Forward_Open` this configuration asks with: `serial` tells this
    /// connection from the originator's others (targets key on serial,
    /// vendor and originator serial), `t_to_o_id` is the connection ID the
    /// target should put on its T->O packets.
    #[must_use]
    pub fn forward_open(&self, serial: u16, t_to_o_id: u32) -> ForwardOpen {
        let mut path = ConnectionPath::assembly(self.configuration, Some(self.consumed), Some(self.produced));
        path.key = self.key;
        path.config_data.clone_from(&self.config_data);
        ForwardOpen {
            priority_time_tick: 0x0A,
            timeout_ticks: 0xF0,
            o_to_t_connection_id: 0,
            t_to_o_connection_id: t_to_o_id,
            connection_serial: serial,
            originator_vendor_id: self.originator_vendor_id,
            originator_serial: self.originator_serial,
            timeout_multiplier: self.timeout_multiplier,
            o_to_t_rpi_us: self.rpi_us(),
            o_to_t: NetworkParams::point_to_point(
                class1_size(self.output_len, self.o_to_t_run_idle),
                self.priority,
            ),
            t_to_o_rpi_us: self.rpi_us(),
            t_to_o: NetworkParams::point_to_point(
                class1_size(self.input_len, self.t_to_o_run_idle),
                self.priority,
            ),
            transport: Transport::CLASS1_CYCLIC,
            path,
        }
    }

    /// Inactivity timeout this configuration implies.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.rpi * (1u32 << (u32::from(self.timeout_multiplier) + 2))
    }

    /// Whether O->T packets carry the run/idle header: configured on, and
    /// not a heartbeat.
    #[must_use]
    pub const fn produces_header(&self) -> bool {
        self.o_to_t_run_idle && self.output_len > 0
    }

    /// Checks that `output` is one O->T packet's worth of application data.
    ///
    /// # Errors
    /// [`OutputLen`] with both sizes when it is not.
    pub fn check_output(&self, output: &[u8]) -> Result<(), OutputLen> {
        if output.len() == usize::from(self.output_len) {
            Ok(())
        } else {
            Err(OutputLen {
                given: output.len(),
                carried: self.output_len,
            })
        }
    }
}

/// An O->T buffer of the wrong size for its connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLen {
    pub given: usize,
    pub carried: u16,
}

impl fmt::Display for OutputLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "output is {} bytes, connection carries {}",
            self.given, self.carried
        )
    }
}

impl core::error::Error for OutputLen {}

/// Builds the O->T datagrams of one connection, one per RPI: the
/// encapsulation sequence and the Class 1 sequence count advance on every
/// packet, the data is whatever [`Self::set_output`] last gave, and the
/// run/idle header (when the connection carries one) follows
/// [`Self::set_run`]. The caller owns the clock and the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Producer {
    connection_id: u32,
    header: bool,
    run: bool,
    output_len: u16,
    output: Vec<u8>,
    encapsulation_sequence: u32,
    sequence_count: u16,
    sent: u64,
}

impl Producer {
    /// A producer for the O->T connection ID the target granted, starting
    /// with `output` as the data of the very first packet (so a reconnect
    /// re-sends what the outputs held instead of a zero glitch). Run until
    /// told otherwise.
    ///
    /// # Errors
    /// [`OutputLen`] if `output` is not `config.output_len` bytes.
    pub fn new(config: &IoConfig, o_to_t_connection_id: u32, output: &[u8]) -> Result<Self, OutputLen> {
        config.check_output(output)?;
        Ok(Self {
            connection_id: o_to_t_connection_id,
            header: config.produces_header(),
            run: true,
            output_len: config.output_len,
            output: output.to_vec(),
            encapsulation_sequence: 0,
            sequence_count: 0,
            sent: 0,
        })
    }

    /// Replace the application data. Carried from the next packet on.
    ///
    /// # Errors
    /// [`OutputLen`] if `data` is not the connection's output size; the
    /// data held so far is kept.
    pub fn set_output(&mut self, data: &[u8]) -> Result<(), OutputLen> {
        if data.len() != usize::from(self.output_len) {
            return Err(OutputLen {
                given: data.len(),
                carried: self.output_len,
            });
        }
        self.output.clear();
        self.output.extend_from_slice(data);
        Ok(())
    }

    /// Set the run/idle header bit (ignored for heartbeats).
    pub const fn set_run(&mut self, run: bool) {
        self.run = run;
    }

    #[must_use]
    pub const fn run(&self) -> bool {
        self.run
    }

    #[must_use]
    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// Packets built so far.
    #[must_use]
    pub const fn sent(&self) -> u64 {
        self.sent
    }

    /// The next datagram to send, counters advanced.
    #[must_use]
    pub fn next_packet(&mut self) -> Vec<u8> {
        self.encapsulation_sequence = self.encapsulation_sequence.wrapping_add(1);
        self.sequence_count = self.sequence_count.wrapping_add(1);
        self.sent += 1;
        let run_idle = self.header.then_some(if self.run {
            Class1Body::RUN
        } else {
            Class1Body::IDLE
        });
        let body = Class1Body {
            sequence_count: self.sequence_count,
            run_idle,
            data: &self.output,
        }
        .encode();
        IoPacket {
            connection_id: self.connection_id,
            encapsulation_sequence: Some(self.encapsulation_sequence),
            body: &body,
        }
        .encode()
    }
}

/// One T->O packet's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub data: Vec<u8>,
    pub sequence_count: u16,
    pub run_idle: Option<u32>,
}

/// Sorts the datagrams that arrive on the I/O port: those for this
/// connection's T->O ID that decode are [`Received`], the rest are
/// counted as ignored (a nonzero count is worth a look, not an error:
/// another connection's packets on a shared socket land here too).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consumer {
    connection_id: u32,
    header: bool,
    latest: Option<Received>,
    received: u64,
    ignored: u64,
}

impl Consumer {
    /// A consumer for the T->O connection ID in use.
    #[must_use]
    pub const fn new(config: &IoConfig, t_to_o_connection_id: u32) -> Self {
        Self {
            connection_id: t_to_o_connection_id,
            header: config.t_to_o_run_idle,
            latest: None,
            received: 0,
            ignored: 0,
        }
    }

    /// Offers one datagram. `Some` if it was this connection's and parsed
    /// (and it is now [`Self::latest`]); `None` if it was ignored.
    pub fn accept(&mut self, datagram: &[u8]) -> Option<&Received> {
        let parsed = IoPacket::decode(datagram)
            .ok()
            .filter(|p| p.connection_id == self.connection_id)
            .and_then(|p| {
                Class1Body::decode(p.body, self.header).ok().map(|b| Received {
                    data: b.data.to_vec(),
                    sequence_count: b.sequence_count,
                    run_idle: b.run_idle,
                })
            });
        if let Some(r) = parsed {
            self.received += 1;
            self.latest = Some(r);
            self.latest.as_ref()
        } else {
            self.ignored += 1;
            None
        }
    }

    #[must_use]
    pub const fn latest(&self) -> Option<&Received> {
        self.latest.as_ref()
    }

    #[must_use]
    pub const fn received(&self) -> u64 {
        self.received
    }

    #[must_use]
    pub const fn ignored(&self) -> u64 {
        self.ignored
    }
}

/// Where the target consumes O->T: `target` unless its `Forward_Open`
/// reply carried an O->T socket-address item (CPF 0x8000) naming a port,
/// and an address if that is not 0.0.0.0. CIP Volume 2 lets a target
/// answer with one; the ioLogik E1213 (firmware 3.1) names UDP 5222 and
/// ignores O->T data sent to 2222. `OpENer` answers with none.
#[must_use]
pub fn o_to_t_target(items: &[explicit::OwnedItem], target: SocketAddrV4) -> SocketAddrV4 {
    let Some((_, data)) = items.iter().find(|(t, _)| *t == cpf::item::SOCKADDR_O_TO_T) else {
        return target;
    };
    let Ok(addr) = SocketAddress::decode(&mut Reader::new(data)) else {
        return target;
    };
    if addr.family != SocketAddress::AF_INET || addr.port == 0 {
        return target;
    }
    let ip = if addr.addr.is_unspecified() {
        *target.ip()
    } else {
        addr.addr
    };
    SocketAddrV4::new(ip, addr.port)
}

/// A T->O socket-address item body: `AF_INET`, `port`, address 0 (the
/// target sends to the IP the session came from; `OpENer` reads only the
/// port, `cipioconnection.c` `OpenProducingPointToPointConnection`). Sent
/// with the `Forward_Open` when the originator listens somewhere other
/// than 2222.
#[must_use]
pub fn t_to_o_sockaddr(port: u16) -> Vec<u8> {
    let mut w = Writer::with_capacity(SocketAddress::LEN);
    SocketAddress {
        family: SocketAddress::AF_INET,
        port,
        addr: Ipv4Addr::UNSPECIFIED,
    }
    .encode(&mut w);
    w.into_inner()
}

/// The `Forward_Close` that closes what `open` opened: same serial,
/// vendor, originator serial and path.
#[must_use]
pub fn close_for(open: &ForwardOpen) -> ForwardClose {
    ForwardClose {
        priority_time_tick: open.priority_time_tick,
        timeout_ticks: open.timeout_ticks,
        connection_serial: open.connection_serial,
        originator_vendor_id: open.originator_vendor_id,
        originator_serial: open.originator_serial,
        path: open.path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> IoConfig {
        IoConfig::exclusive_owner(0x66, 0x65, 0x64, 2, 4, Duration::from_millis(10))
    }

    #[test]
    fn producer_counts_up_and_carries_the_latest_output() {
        let mut p = Producer::new(&config(), 0x1234_5678, &[0xAA, 0xBB]).unwrap();
        let first = p.next_packet();
        let a = IoPacket::decode(&first).unwrap();
        assert_eq!(a.connection_id, 0x1234_5678);
        assert_eq!(a.encapsulation_sequence, Some(1));
        let body = Class1Body::decode(a.body, true).unwrap();
        assert_eq!(body.sequence_count, 1);
        assert_eq!(body.run_idle, Some(Class1Body::RUN));
        assert_eq!(body.data, &[0xAA, 0xBB]);

        p.set_output(&[1, 2]).unwrap();
        p.set_run(false);
        let second = p.next_packet();
        let b = IoPacket::decode(&second).unwrap();
        assert_eq!(b.encapsulation_sequence, Some(2));
        let body = Class1Body::decode(b.body, true).unwrap();
        assert_eq!(body.sequence_count, 2);
        assert_eq!(body.run_idle, Some(Class1Body::IDLE));
        assert_eq!(body.data, &[1, 2]);
        assert_eq!(p.sent(), 2);
    }

    #[test]
    fn producer_refuses_the_wrong_output_size_and_keeps_the_old() {
        let c = config();
        assert_eq!(
            Producer::new(&c, 1, &[0; 3]).unwrap_err(),
            OutputLen { given: 3, carried: 2 }
        );
        let mut p = Producer::new(&c, 1, &[7, 8]).unwrap();
        assert!(p.set_output(&[1]).is_err());
        assert_eq!(p.output(), &[7, 8]);
    }

    #[test]
    fn heartbeat_has_no_header() {
        let c = IoConfig::input_only(0x66, 0x65, 0x64, 4, Duration::from_millis(10));
        assert!(!c.produces_header());
        let mut p = Producer::new(&c, 9, &[]).unwrap();
        let pkt = p.next_packet();
        let a = IoPacket::decode(&pkt).unwrap();
        let body = Class1Body::decode(a.body, false).unwrap();
        assert_eq!(body.run_idle, None);
        assert_eq!(body.data, &[] as &[u8]);
    }

    #[test]
    fn consumer_takes_its_own_id_and_ignores_the_rest() {
        let c = config();
        let mut consumer = Consumer::new(&c, 0xC0FF_EE00);
        let mine = IoPacket {
            connection_id: 0xC0FF_EE00,
            encapsulation_sequence: Some(5),
            body: &Class1Body {
                sequence_count: 42,
                run_idle: None,
                data: &[1, 2, 3, 4],
            }
            .encode(),
        }
        .encode();
        let theirs = IoPacket {
            connection_id: 0xC0FF_EE01,
            encapsulation_sequence: Some(5),
            body: &[0, 0, 1, 2, 3, 4],
        }
        .encode();
        assert!(consumer.accept(&theirs).is_none());
        assert!(consumer.accept(b"not a packet").is_none());
        let got = consumer.accept(&mine).unwrap().clone();
        assert_eq!(got.sequence_count, 42);
        assert_eq!(got.data, vec![1, 2, 3, 4]);
        assert_eq!(consumer.received(), 1);
        assert_eq!(consumer.ignored(), 2);
        assert_eq!(consumer.latest(), Some(&got));
    }

    #[test]
    fn timeout_is_rpi_times_two_to_the_multiplier_plus_two() {
        let mut c = config();
        c.timeout_multiplier = 1;
        assert_eq!(c.timeout(), Duration::from_millis(80));
        c.timeout_multiplier = 0;
        assert_eq!(c.timeout(), Duration::from_millis(40));
    }

    #[test]
    fn o_to_t_target_follows_a_named_port_and_keeps_the_ip_when_unspecified() {
        let target = SocketAddrV4::new(Ipv4Addr::new(192, 168, 3, 60), 2222);
        assert_eq!(o_to_t_target(&[], target), target);
        let named = (cpf::item::SOCKADDR_O_TO_T, t_to_o_sockaddr(5222));
        assert_eq!(
            o_to_t_target(&[named], target),
            SocketAddrV4::new(Ipv4Addr::new(192, 168, 3, 60), 5222)
        );
        let zero_port = (cpf::item::SOCKADDR_O_TO_T, t_to_o_sockaddr(0));
        assert_eq!(o_to_t_target(&[zero_port], target), target);
    }

    #[test]
    fn close_for_mirrors_the_open() {
        let open = config().forward_open(0x1234, 0xABCD_0001);
        let close = close_for(&open);
        assert_eq!(close.connection_serial, 0x1234);
        assert_eq!(close.originator_vendor_id, open.originator_vendor_id);
        assert_eq!(close.path, open.path);
    }
}
