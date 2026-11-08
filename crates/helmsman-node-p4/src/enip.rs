//! EtherNet/IP from the node: an originator's explicit session over an
//! embassy-net TCP socket (`RegisterSession`, `SendRRData`), and a Class 1
//! I/O connection opened through it, whose packets helmsman-proto's
//! `class1::{Producer, Consumer}` build and sort. The bytes are the box's
//! (helmsman-proto's `explicit`, `io` and `class1`); only the sockets and
//! the clock are this crate's.
//!
//! One request in flight at a time, which is what the encapsulation layer
//! assumes for unconnected messaging. Any error drops the TCP connection so
//! the next request starts clean, as the Modbus client does; the one socket
//! is reused across reconnects.
//!
//! Addressing follows `OpENer` for point-to-point: the target sends T->O to
//! the IP it saw the session come from, UDP 2222, and takes O->T from that
//! same IP, at 2222 unless its `Forward_Open` reply names another port (the
//! ioLogik E1213 names 5222; `class1::o_to_t_target`).

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::net::SocketAddrV4;

use embassy_net::tcp::TcpSocket;
use embassy_net::{IpAddress, IpEndpoint, Ipv4Address, Stack};
use embassy_time::{Duration, with_timeout};
use embedded_io_async::{Read, Write};
use helmsman_proto::cip::GeneralStatus;
use helmsman_proto::class1::{self, Consumer, IoConfig, OutputLen, Producer};
use helmsman_proto::codec::{DecodeError, Reader, Writer};
use helmsman_proto::encap::{self, Frame, HEADER_LEN, Header};
use helmsman_proto::explicit::{self, OwnedItem};
use helmsman_proto::io::{
    ForwardCloseReply, ForwardOpen, ForwardOpenReply, Refused, close_result, open_result,
};

/// Anything that can go wrong talking to a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The link: connect, send or receive failed, or took longer than the
    /// timeout.
    Link(&'static str),
    /// The bytes on the wire did not decode.
    Decode(DecodeError),
    /// The target answered with an encapsulation-layer error status.
    Encapsulation(u32),
    /// The reply's sender context did not match the request's.
    ContextMismatch,
    /// The target answered, and the answer is a CIP error.
    Cip {
        status: GeneralStatus,
        additional: Vec<u16>,
    },
    /// The Connection Manager refused the open or the close.
    Refused(Refused),
    /// The O->T data is not the connection's size.
    Output(OutputLen),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link(what) => write!(f, "link: {what}"),
            Self::Decode(e) => write!(f, "decode: {e}"),
            Self::Encapsulation(s) => match encap::Status::from_u32(*s) {
                Some(known) => write!(f, "encapsulation status {known:?}"),
                None => write!(f, "encapsulation status 0x{s:08X}"),
            },
            Self::ContextMismatch => write!(f, "reply sender context does not match request"),
            Self::Cip { status, additional } => {
                write!(f, "cip status {status}")?;
                if !additional.is_empty() {
                    write!(f, " additional {additional:04X?}")?;
                }
                Ok(())
            }
            Self::Refused(r) => write!(f, "refused: {r}"),
            Self::Output(e) => write!(f, "{e}"),
        }
    }
}

impl From<DecodeError> for Error {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

impl From<OutputLen> for Error {
    fn from(e: OutputLen) -> Self {
        Self::Output(e)
    }
}

/// An explicit session to one target on TCP 44818.
pub struct Session<'a> {
    endpoint: IpEndpoint,
    timeout: Duration,
    socket: TcpSocket<'a>,
    connected: bool,
    handle: u32,
    next_context: u64,
    /// Connections opened since start: a reconnect count for the record.
    pub connects: u32,
}

impl<'a> Session<'a> {
    /// `rx` and `tx` are the socket's buffers for the session's whole life
    /// (a `Forward_Open` exchange is under 200 bytes each way; 512 is
    /// plenty).
    pub fn new(
        stack: Stack<'a>,
        target: Ipv4Address,
        timeout: Duration,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Self {
        let mut socket = TcpSocket::new(stack, rx, tx);
        socket.set_timeout(Some(timeout));
        Self {
            endpoint: IpEndpoint::new(IpAddress::Ipv4(target), encap::PORT),
            timeout,
            socket,
            connected: false,
            handle: 0,
            next_context: 1,
            connects: 0,
        }
    }

    /// The target's IP.
    #[must_use]
    pub fn target(&self) -> Ipv4Address {
        match self.endpoint.addr {
            IpAddress::Ipv4(ip) => ip,
        }
    }

    #[must_use]
    pub const fn is_registered(&self) -> bool {
        self.connected && self.handle != 0
    }

    fn context(&mut self) -> [u8; 8] {
        let c = self.next_context;
        self.next_context = self.next_context.wrapping_add(1);
        c.to_le_bytes()
    }

    /// Drops the TCP connection; the next request registers again.
    pub fn drop_link(&mut self) {
        self.socket.abort();
        self.connected = false;
        self.handle = 0;
    }

    async fn connect(&mut self) -> Result<(), Error> {
        if self.connected {
            return Ok(());
        }
        match with_timeout(self.timeout, self.socket.connect(self.endpoint)).await {
            Ok(Ok(())) => {
                self.connected = true;
                self.connects += 1;
                Ok(())
            }
            Ok(Err(_)) => {
                self.socket.abort();
                Err(Error::Link("connect refused"))
            }
            Err(_) => {
                self.socket.abort();
                Err(Error::Link("connect timed out"))
            }
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        with_timeout(self.timeout, self.socket.write_all(bytes))
            .await
            .map_err(|_| Error::Link("send timed out"))?
            .map_err(|_| Error::Link("send failed"))?;
        with_timeout(self.timeout, self.socket.flush())
            .await
            .map_err(|_| Error::Link("flush timed out"))?
            .map_err(|_| Error::Link("flush failed"))
    }

    /// Reads exactly one encapsulation message.
    async fn read_frame(&mut self) -> Result<Vec<u8>, Error> {
        let mut head = [0u8; HEADER_LEN];
        with_timeout(self.timeout, self.socket.read_exact(&mut head))
            .await
            .map_err(|_| Error::Link("reply timed out"))?
            .map_err(|_| Error::Link("reply cut short"))?;
        let header = Header::decode(&mut Reader::new(&head))?;
        let mut out = vec![0u8; HEADER_LEN + usize::from(header.length)];
        out[..HEADER_LEN].copy_from_slice(&head);
        if header.length > 0 {
            with_timeout(self.timeout, self.socket.read_exact(&mut out[HEADER_LEN..]))
                .await
                .map_err(|_| Error::Link("reply body timed out"))?
                .map_err(|_| Error::Link("reply body cut short"))?;
        }
        Ok(out)
    }

    /// Connects and registers if not already; idempotent.
    ///
    /// # Errors
    /// The link, or a refused registration; the link is dropped on any.
    pub async fn register(&mut self) -> Result<(), Error> {
        let r = self.register_inner().await;
        if r.is_err() {
            self.drop_link();
        }
        r
    }

    async fn register_inner(&mut self) -> Result<(), Error> {
        self.connect().await?;
        if self.handle != 0 {
            return Ok(());
        }
        let ctx = self.context();
        self.write(&explicit::register_session(ctx)).await?;
        let bytes = self.read_frame().await?;
        let (frame, _) = Frame::decode(&bytes)?;
        if frame.header.sender_context != ctx {
            return Err(Error::ContextMismatch);
        }
        if frame.header.status != 0 {
            return Err(Error::Encapsulation(frame.header.status));
        }
        self.handle = explicit::decode_register_session(&frame)?.session_handle;
        Ok(())
    }

    /// Sends one encoded Message Router request as `SendRRData` (with
    /// `extra` CPF items after the data item) and returns the raw response
    /// and the reply's own extra items (a target's socket-address items on
    /// a `Forward_Open` reply). Registers first if needed.
    ///
    /// # Errors
    /// The link, the decode, an encapsulation status or a context
    /// mismatch; the link is dropped on any. A CIP error status in the
    /// response is *not* an error here: `open_result` and `close_result`
    /// read it.
    pub async fn exchange(
        &mut self,
        mr: &[u8],
        extra: &[helmsman_proto::cpf::Item<'_>],
    ) -> Result<(helmsman_proto::cip::Response, Vec<OwnedItem>), Error> {
        let r = self.exchange_inner(mr, extra).await;
        if r.is_err() {
            self.drop_link();
        }
        r
    }

    async fn exchange_inner(
        &mut self,
        mr: &[u8],
        extra: &[helmsman_proto::cpf::Item<'_>],
    ) -> Result<(helmsman_proto::cip::Response, Vec<OwnedItem>), Error> {
        self.register().await?;
        let ctx = self.context();
        let request = explicit::send_rr_data_with_items(self.handle, ctx, 0, mr, extra);
        self.write(&request).await?;
        let bytes = self.read_frame().await?;
        let (frame, _) = Frame::decode(&bytes)?;
        if frame.header.sender_context != ctx {
            return Err(Error::ContextMismatch);
        }
        if frame.header.status != 0 {
            return Err(Error::Encapsulation(frame.header.status));
        }
        Ok(explicit::decode_send_rr_data_with_items(&frame)?)
    }

    /// `UnregisterSession`, best effort, then drops the link.
    pub async fn unregister(&mut self) {
        if self.is_registered() {
            let ctx = self.context();
            let _ = self.write(&explicit::unregister_session(self.handle, ctx)).await;
        }
        self.drop_link();
    }
}

/// An open Class 1 connection: what to send ([`Self::producer`]), what to
/// take ([`Self::consumer`]), and where O->T goes ([`Self::target`]).
pub struct IoConnection {
    config: IoConfig,
    open: ForwardOpen,
    reply: ForwardOpenReply,
    target: IpEndpoint,
    pub producer: Producer,
    pub consumer: Consumer,
}

impl IoConnection {
    /// Sends `Forward_Open` over `session` for `config`, with `output` as
    /// the first O->T data (so a reopen re-sends what the outputs held).
    /// `serial` tells this connection from the node's others; the T->O
    /// connection ID is `serial << 16 | vendor`, as the box's scanner does
    /// on a socket of its own.
    ///
    /// # Errors
    /// `Refused` with the Connection Manager's extended status (0x0106
    /// ownership conflict when another originator owns the target's
    /// outputs), or a session error.
    pub async fn open(
        session: &mut Session<'_>,
        config: IoConfig,
        serial: u16,
        output: &[u8],
    ) -> Result<Self, Error> {
        config.check_output(output)?;
        let t_to_o_id = u32::from(serial) << 16 | u32::from(config.originator_vendor_id);
        let open = config.forward_open(serial, t_to_o_id);
        let mut w = Writer::default();
        open.to_request().encode(&mut w);
        let (resp, items) = session.exchange(&w.into_inner(), &[]).await?;
        let reply = open_result(&resp)?.map_err(Error::Refused)?;
        let default = SocketAddrV4::new(session.target(), encap::IO_PORT);
        let to = class1::o_to_t_target(&items, default);
        let target = IpEndpoint::new(IpAddress::Ipv4(*to.ip()), to.port());
        let producer = Producer::new(&config, reply.o_to_t_connection_id, output)?;
        let consumer = Consumer::new(&config, reply.t_to_o_connection_id);
        Ok(Self {
            config,
            open,
            reply,
            target,
            producer,
            consumer,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &IoConfig {
        &self.config
    }

    #[must_use]
    pub const fn reply(&self) -> &ForwardOpenReply {
        &self.reply
    }

    /// Where O->T packets go.
    #[must_use]
    pub const fn target(&self) -> IpEndpoint {
        self.target
    }

    /// Sends `Forward_Close` over `session`.
    ///
    /// # Errors
    /// A refused close (the target no longer knows the connection, which
    /// after a timeout is expected) or a session error.
    pub async fn close(&self, session: &mut Session<'_>) -> Result<ForwardCloseReply, Error> {
        let mut w = Writer::default();
        class1::close_for(&self.open).to_request().encode(&mut w);
        let (resp, _) = session.exchange(&w.into_inner(), &[]).await?;
        close_result(&resp)?.map_err(Error::Refused)
    }
}
