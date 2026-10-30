//! A blocking TCP session to one target: register, send unconnected
//! explicit requests, unregister. One request in flight at a time, which is
//! what the encapsulation layer assumes for unconnected messaging.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::cip::{self, GeneralStatus};
use crate::codec::DecodeError;
use crate::encap::{Frame, HEADER_LEN, Header, Status};
use crate::explicit;

/// Anything that can go wrong talking to a target.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Decode(e) => write!(f, "decode: {e}"),
            Self::Encapsulation(s) => match Status::from_u32(*s) {
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
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<DecodeError> for Error {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

/// A registered encapsulation session.
#[derive(Debug)]
pub struct Session {
    stream: TcpStream,
    peer: SocketAddr,
    handle: u32,
    next_context: u64,
}

impl Session {
    /// Connect with `timeout` for the TCP handshake and every subsequent
    /// read and write, then register.
    ///
    /// # Errors
    /// Connection, timeout, or a refused registration.
    pub fn connect<A: ToSocketAddrs>(addr: A, timeout: Duration) -> Result<Self, Error> {
        let mut last = None;
        for peer in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&peer, timeout) {
                Ok(stream) => return Self::register(stream, peer, timeout),
                Err(e) => last = Some(e),
            }
        }
        Err(Error::Io(last.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "address resolved to nothing")
        })))
    }

    fn register(stream: TcpStream, peer: SocketAddr, timeout: Duration) -> Result<Self, Error> {
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        let mut s = Self {
            stream,
            peer,
            handle: 0,
            next_context: 1,
        };
        let ctx = s.context();
        s.stream.write_all(&explicit::register_session(ctx))?;
        let bytes = s.read_frame()?;
        let (frame, _) = Frame::decode(&bytes)?;
        if frame.header.sender_context != ctx {
            return Err(Error::ContextMismatch);
        }
        if frame.header.status != 0 {
            return Err(Error::Encapsulation(frame.header.status));
        }
        s.handle = explicit::decode_register_session(&frame)?.session_handle;
        Ok(s)
    }

    #[must_use]
    pub const fn session_handle(&self) -> u32 {
        self.handle
    }

    #[must_use]
    pub const fn peer(&self) -> SocketAddr {
        self.peer
    }

    fn context(&mut self) -> [u8; 8] {
        let c = self.next_context;
        self.next_context = self.next_context.wrapping_add(1);
        c.to_le_bytes()
    }

    /// Read exactly one encapsulation message.
    fn read_frame(&mut self) -> Result<Vec<u8>, Error> {
        let mut head = [0u8; HEADER_LEN];
        self.stream.read_exact(&mut head)?;
        let header = Header::decode(&mut crate::codec::Reader::new(&head))?;
        let mut out = Vec::with_capacity(HEADER_LEN + usize::from(header.length));
        out.extend_from_slice(&head);
        out.resize(HEADER_LEN + usize::from(header.length), 0);
        self.stream.read_exact(&mut out[HEADER_LEN..])?;
        Ok(out)
    }

    /// Send one unconnected request and return the raw Message Router
    /// response, including error responses. Use [`Self::request`] when a
    /// CIP error should be an `Err`.
    ///
    /// # Errors
    /// Transport, decode, encapsulation, or context mismatch.
    #[cfg(feature = "unrestricted")]
    pub fn request_raw(&mut self, request: &cip::Request) -> Result<cip::Response, Error> {
        let mut mr = crate::codec::Writer::default();
        request.encode(&mut mr);
        self.exchange(&mr.into_inner())
    }

    /// Send one encoded Message Router request and read its reply. Crate
    /// private: outside the `unrestricted` feature only read services reach
    /// it (through [`Self::get_attribute_single`] and
    /// [`crate::logix::TagReader`]).
    pub(crate) fn exchange(&mut self, mr: &[u8]) -> Result<cip::Response, Error> {
        let ctx = self.context();
        self.stream
            .write_all(&explicit::send_rr_data_bytes(self.handle, ctx, 0, mr))?;
        let bytes = self.read_frame()?;
        let (frame, _) = Frame::decode(&bytes)?;
        if frame.header.sender_context != ctx {
            return Err(Error::ContextMismatch);
        }
        if frame.header.status != 0 {
            return Err(Error::Encapsulation(frame.header.status));
        }
        Ok(explicit::decode_send_rr_data(&frame)?)
    }

    /// Send one unconnected request; a non-zero general status is an error.
    ///
    /// # Errors
    /// As [`Self::request_raw`], plus `Error::Cip` for a CIP error status.
    #[cfg(feature = "unrestricted")]
    pub fn request(&mut self, request: &cip::Request) -> Result<Vec<u8>, Error> {
        self.request_raw(request).and_then(Self::data_or_error)
    }

    fn data_or_error(resp: cip::Response) -> Result<Vec<u8>, Error> {
        if resp.status.is_success() {
            Ok(resp.data)
        } else {
            Err(Error::Cip {
                status: resp.status,
                additional: resp.additional_status,
            })
        }
    }

    /// `Get_Attribute_Single` on `class.instance.attribute`; raw attribute bytes.
    ///
    /// # Errors
    /// As [`Self::request`].
    pub fn get_attribute_single(
        &mut self,
        class: u16,
        instance: u16,
        attribute: u16,
    ) -> Result<Vec<u8>, Error> {
        let mut mr = crate::codec::Writer::default();
        cip::Request::get_attribute_single(class, instance, attribute).encode(&mut mr);
        self.exchange(&mr.into_inner()).and_then(Self::data_or_error)
    }

    /// Unregister and drop the connection. Errors on the final write are
    /// ignored: the target closes the socket without replying.
    pub fn close(mut self) {
        let ctx = self.context();
        let _ = self
            .stream
            .write_all(&explicit::unregister_session(self.handle, ctx));
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
