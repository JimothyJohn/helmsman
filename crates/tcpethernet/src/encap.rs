//! Encapsulation layer: the 24-byte header that fronts every TCP and UDP
//! message on port 44818, and the command codes it carries.
//!
//! Layout and field order follow `OpENer` `encap.c` (`CreateEncapsulationStructure`,
//! `GenerateEncapsulationHeader`): command u16, length u16, session handle u32,
//! status u32, sender context 8 bytes, options u32, all little-endian. Port
//! numbers from PUB00213R0 glossary p.23 ("Encapsulation Protocol uses TCP/UDP
//! Port 44818").

use crate::codec::{DecodeError, Reader, Writer};

/// TCP and UDP port for encapsulation commands and explicit messaging.
pub const PORT: u16 = 44818;
/// UDP port for Class 0/1 I/O (PUB00213R0 glossary p.24, "UDP/IP frames on port 2222").
pub const IO_PORT: u16 = 2222;
/// Size of the fixed header.
pub const HEADER_LEN: usize = 24;
/// Encapsulation protocol version this crate speaks (`OpENer` `kSupportedProtocolVersion`).
pub const PROTOCOL_VERSION: u16 = 1;

/// Encapsulation command codes (`OpENer` `encap.c`, `EncapsulationCommands`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Command {
    /// TCP only.
    Nop = 0x0000,
    /// TCP or UDP.
    ListServices = 0x0004,
    /// TCP or UDP. The discovery command.
    ListIdentity = 0x0063,
    /// TCP or UDP, optional.
    ListInterfaces = 0x0064,
    /// TCP only.
    RegisterSession = 0x0065,
    /// TCP only.
    UnregisterSession = 0x0066,
    /// TCP only. Unconnected explicit messaging.
    SendRrData = 0x006F,
    /// TCP only. Connected explicit messaging.
    SendUnitData = 0x0070,
}

impl Command {
    /// Map a wire value to a known command.
    #[must_use]
    pub const fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            0x0000 => Self::Nop,
            0x0004 => Self::ListServices,
            0x0063 => Self::ListIdentity,
            0x0064 => Self::ListInterfaces,
            0x0065 => Self::RegisterSession,
            0x0066 => Self::UnregisterSession,
            0x006F => Self::SendRrData,
            0x0070 => Self::SendUnitData,
            _ => return None,
        })
    }
}

/// Encapsulation status codes (`OpENer` `encap.h`, `EncapsulationProtocolErrorCode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Status {
    Success = 0x0000,
    InvalidCommand = 0x0001,
    InsufficientMemory = 0x0002,
    IncorrectData = 0x0003,
    InvalidSessionHandle = 0x0064,
    InvalidLength = 0x0065,
    UnsupportedProtocol = 0x0069,
}

impl Status {
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        Some(match v {
            0x0000 => Self::Success,
            0x0001 => Self::InvalidCommand,
            0x0002 => Self::InsufficientMemory,
            0x0003 => Self::IncorrectData,
            0x0064 => Self::InvalidSessionHandle,
            0x0065 => Self::InvalidLength,
            0x0069 => Self::UnsupportedProtocol,
            _ => return None,
        })
    }
}

/// The fixed 24-byte header. `command` and `status` are kept as raw integers
/// so unknown values survive a round trip; use [`Command::from_u16`] and
/// [`Status::from_u32`] to interpret them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub command: u16,
    /// Length of the command-specific data that follows the header.
    pub length: u16,
    pub session_handle: u32,
    pub status: u32,
    /// Echoed back unchanged by the target; the originator uses it to match replies.
    pub sender_context: [u8; 8],
    /// Shall be zero. A nonzero value is rejected on decode.
    pub options: u32,
}

impl Header {
    /// A request header with zero session, status, and options.
    #[must_use]
    pub const fn request(command: Command, length: u16, sender_context: [u8; 8]) -> Self {
        Self {
            command: command as u16,
            length,
            session_handle: 0,
            status: 0,
            sender_context,
            options: 0,
        }
    }

    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut w = Writer::with_capacity(HEADER_LEN);
        w.u16_le(self.command)
            .u16_le(self.length)
            .u32_le(self.session_handle)
            .u32_le(self.status)
            .bytes(&self.sender_context)
            .u32_le(self.options);
        let v = w.into_inner();
        let mut out = [0u8; HEADER_LEN];
        out.copy_from_slice(&v);
        out
    }

    /// Decode a header from the front of `r`.
    ///
    /// # Errors
    /// `Truncated` if fewer than 24 bytes remain; `Invalid` if `options` is nonzero.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let command = r.u16_le()?;
        let length = r.u16_le()?;
        let session_handle = r.u32_le()?;
        let status = r.u32_le()?;
        let mut sender_context = [0u8; 8];
        sender_context.copy_from_slice(r.bytes(8)?);
        let options = r.u32_le()?;
        if options != 0 {
            return Err(DecodeError::Invalid("encapsulation options must be zero"));
        }
        Ok(Self {
            command,
            length,
            session_handle,
            status,
            sender_context,
            options,
        })
    }
}

/// One complete encapsulation message split into header and payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub header: Header,
    /// Exactly `header.length` bytes.
    pub payload: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Split one message off the front of `buf`. Returns the frame and the
    /// bytes after it, so a TCP stream carrying back-to-back messages can be
    /// walked without copying.
    ///
    /// # Errors
    /// `Truncated` if the header or the declared payload is incomplete;
    /// `Invalid` if the header is malformed.
    pub fn decode(buf: &'a [u8]) -> Result<(Self, &'a [u8]), DecodeError> {
        let mut r = Reader::new(buf);
        let header = Header::decode(&mut r)?;
        let payload = r.bytes(usize::from(header.length))?;
        Ok((Self { header, payload }, r.rest()))
    }

    /// Build a message. `payload.len()` must fit in a `u16`.
    ///
    /// # Panics
    /// If the payload is longer than `u16::MAX`.
    #[must_use]
    pub fn encode(command: Command, session_handle: u32, sender_context: [u8; 8], payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len()).expect("encapsulation payload exceeds u16");
        let header = Header {
            command: command as u16,
            length,
            session_handle,
            status: 0,
            sender_context,
            options: 0,
        };
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(&header.encode());
        out.extend_from_slice(payload);
        out
    }
}

/// The 24-byte `ListIdentity` request. It has no payload (`OpENer`
/// `EncapsulateListIdentityResponseMessage` echoes only the header fields).
#[must_use]
pub fn list_identity_request(sender_context: [u8; 8]) -> [u8; HEADER_LEN] {
    Header::request(Command::ListIdentity, 0, sender_context).encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let h = Header {
            command: 0x63,
            length: 5,
            session_handle: 0xDEAD_BEEF,
            status: 0,
            sender_context: *b"ctx-1234",
            options: 0,
        };
        let bytes = h.encode();
        assert_eq!(bytes.len(), HEADER_LEN);
        assert_eq!(&bytes[..2], &[0x63, 0x00]);
        let back = Header::decode(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn nonzero_options_rejected() {
        let mut bytes = list_identity_request([0; 8]);
        bytes[20] = 1;
        assert_eq!(
            Header::decode(&mut Reader::new(&bytes)),
            Err(DecodeError::Invalid("encapsulation options must be zero"))
        );
    }

    #[test]
    fn frame_splits_stream() {
        let a = Frame::encode(Command::Nop, 0, [1; 8], b"abc");
        let b = Frame::encode(Command::ListIdentity, 7, [2; 8], b"");
        let mut stream = a.clone();
        stream.extend_from_slice(&b);
        let (fa, rest) = Frame::decode(&stream).unwrap();
        assert_eq!(fa.payload, b"abc");
        assert_eq!(fa.header.command, Command::Nop as u16);
        let (fb, rest) = Frame::decode(rest).unwrap();
        assert_eq!(fb.header.session_handle, 7);
        assert!(fb.payload.is_empty());
        assert!(rest.is_empty());
    }

    #[test]
    fn frame_declared_length_beyond_buffer_is_truncated() {
        let mut msg = Frame::encode(Command::Nop, 0, [0; 8], b"ab");
        msg[2] = 0xFF;
        msg[3] = 0xFF;
        assert!(matches!(Frame::decode(&msg), Err(DecodeError::Truncated { .. })));
    }
}
