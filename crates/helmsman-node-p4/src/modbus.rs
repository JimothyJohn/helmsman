//! Modbus TCP from the node: the helmsman-modbus codec over an embassy-net
//! TCP socket, the async twin of helmsman-io's `Transport`. One request at
//! a time, one connection to one unit; any error drops the connection so the
//! next request starts clean (a late reply is never taken for the answer to
//! a newer request), the same rule as the box's client. The one socket is
//! reused across reconnects (embassy-net allows `connect` again after
//! `abort`), so the buffers are lent once and never juggled.

use alloc::vec;
use alloc::vec::Vec;

use embassy_net::tcp::TcpSocket;
use embassy_net::{IpEndpoint, Stack};
use embassy_time::{Duration, Instant, with_timeout};
use embedded_io_async::{Read, Write};
use helmsman_modbus::{MBAP_LEN, READ_DISCRETE_INPUTS, mbap, pdu, reply};

/// What a request can fail with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The codec refused the request or the reply.
    Codec(helmsman_modbus::Error),
    /// The link: connect, send or receive failed, or took longer than the
    /// timeout.
    Link(&'static str),
}

impl From<helmsman_modbus::Error> for Error {
    fn from(e: helmsman_modbus::Error) -> Self {
        Self::Codec(e)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Codec(e) => write!(f, "{e}"),
            Self::Link(what) => write!(f, "link: {what}"),
        }
    }
}

/// A client of one unit at one endpoint.
pub struct Client<'a> {
    endpoint: IpEndpoint,
    unit: u8,
    timeout: Duration,
    socket: TcpSocket<'a>,
    connected: bool,
    tid: u16,
    /// Connections opened since start: a reconnect count for the record.
    pub connects: u32,
    /// After a failed connect, no new attempt before this: a dead or
    /// absent unit would otherwise be dialled every scan, and each dial
    /// spends the stack's one ARP request per second (smoltcp's neighbour
    /// cache is silent for 1 s after any request, for every address), so
    /// nothing else on the node could resolve a neighbour; the 2026-11-08
    /// run lost its report sink that way.
    retry_at: Option<Instant>,
}

/// How long a failed connect keeps the link quiet.
pub const CONNECT_BACKOFF: Duration = Duration::from_secs(2);

impl<'a> Client<'a> {
    /// `rx` and `tx` are the socket's buffers for the client's whole life
    /// (a Modbus frame is at most 260 bytes; 512 each is plenty).
    pub fn new(
        stack: Stack<'a>,
        endpoint: IpEndpoint,
        unit: u8,
        timeout: Duration,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Self {
        let mut socket = TcpSocket::new(stack, rx, tx);
        socket.set_timeout(Some(timeout));
        Self {
            endpoint,
            unit,
            timeout,
            socket,
            connected: false,
            tid: 0,
            connects: 0,
            retry_at: None,
        }
    }

    async fn connect(&mut self) -> Result<(), Error> {
        if self.connected {
            return Ok(());
        }
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            return Err(Error::Link("backing off after a failed connect"));
        }
        match with_timeout(self.timeout, self.socket.connect(self.endpoint)).await {
            Ok(Ok(())) => {
                self.connected = true;
                self.connects += 1;
                self.retry_at = None;
                Ok(())
            }
            Ok(Err(_)) => {
                self.socket.abort();
                self.retry_at = Some(Instant::now() + CONNECT_BACKOFF);
                Err(Error::Link("connect refused"))
            }
            Err(_) => {
                self.socket.abort();
                self.retry_at = Some(Instant::now() + CONNECT_BACKOFF);
                Err(Error::Link("connect timed out"))
            }
        }
    }

    fn drop_link(&mut self) {
        self.socket.abort();
        self.connected = false;
    }

    /// Sends one request PDU and returns the reply PDU.
    ///
    /// # Errors
    /// The link or the codec; the connection is dropped on any error.
    pub async fn request(&mut self, req: &[u8]) -> Result<Vec<u8>, Error> {
        let r = self.exchange(req).await;
        if r.is_err() {
            self.drop_link();
        }
        r
    }

    async fn exchange(&mut self, req: &[u8]) -> Result<Vec<u8>, Error> {
        self.connect().await?;
        self.tid = self.tid.wrapping_add(1);
        let (tid, unit) = (self.tid, self.unit);
        let frame = mbap::frame(tid, unit, req)?;
        with_timeout(self.timeout, self.socket.write_all(&frame))
            .await
            .map_err(|_| Error::Link("send timed out"))?
            .map_err(|_| Error::Link("send failed"))?;
        with_timeout(self.timeout, self.socket.flush())
            .await
            .map_err(|_| Error::Link("flush timed out"))?
            .map_err(|_| Error::Link("flush failed"))?;
        let mut head = [0u8; MBAP_LEN];
        with_timeout(self.timeout, self.socket.read_exact(&mut head))
            .await
            .map_err(|_| Error::Link("reply timed out"))?
            .map_err(|_| Error::Link("reply cut short"))?;
        let len = mbap::check_head(&head, tid, unit)?;
        let mut body = vec![0u8; len];
        with_timeout(self.timeout, self.socket.read_exact(&mut body))
            .await
            .map_err(|_| Error::Link("reply body timed out"))?
            .map_err(|_| Error::Link("reply body cut short"))?;
        Ok(body)
    }

    /// FC 0x02: `qty` discrete inputs from `start`.
    ///
    /// # Errors
    /// See [`Self::request`].
    pub async fn read_discrete_inputs(&mut self, start: u16, qty: u16) -> Result<Vec<bool>, Error> {
        let req = pdu::read(READ_DISCRETE_INPUTS, start, qty)?;
        let rep = self.request(&req).await?;
        Ok(reply::bits(READ_DISCRETE_INPUTS, qty, &rep)?)
    }

    /// FC 0x0F: the coils from `start`, one round trip.
    ///
    /// # Errors
    /// See [`Self::request`].
    pub async fn write_coils(&mut self, start: u16, values: &[bool]) -> Result<(), Error> {
        let req = pdu::write_coils(start, values)?;
        let rep = self.request(&req).await?;
        Ok(reply::echo(&req, &rep)?)
    }
}
