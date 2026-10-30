//! Unconnected explicit messaging over TCP: `RegisterSession`,
//! `UnregisterSession`, and `SendRRData` carrying a Message Router request
//! in an unconnected data item.
//!
//! Layouts from `OpENer` `encap.c`: `HandleReceivedRegisterSessionCommand`
//! (payload: protocol version u16, options u16; reply carries the handle in
//! the header and echoes version 1, options 0),
//! `HandleReceivedSendRequestResponseDataCommand` (payload: interface
//! handle u32, timeout u16, then CPF), and `cpf.c` `NotifyCommonPacketFormat`
//! (the CPF must be a null address item followed by an unconnected data
//! item). Sequence overview in PUB00213R0 Appendix C, Figure 13.

use crate::cip;
use crate::codec::{DecodeError, Reader, Writer};
use crate::cpf::{self, Item};
use crate::encap::{Command, Frame, PROTOCOL_VERSION, Status};

/// `RegisterSession` request bytes. Session handle is zero until the reply.
#[must_use]
pub fn register_session(sender_context: [u8; 8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(4);
    w.u16_le(PROTOCOL_VERSION).u16_le(0);
    Frame::encode(Command::RegisterSession, 0, sender_context, &w.into_inner())
}

/// What a `RegisterSession` reply told us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registered {
    pub session_handle: u32,
    pub protocol_version: u16,
}

/// Interpret a `RegisterSession` reply.
///
/// # Errors
/// `Invalid` if the frame is not a successful `RegisterSession` reply;
/// `Truncated` if the payload is short.
pub fn decode_register_session(frame: &Frame<'_>) -> Result<Registered, DecodeError> {
    if frame.header.command != Command::RegisterSession as u16 {
        return Err(DecodeError::Invalid("not a RegisterSession reply"));
    }
    if frame.header.status != Status::Success as u32 {
        return Err(DecodeError::Invalid("RegisterSession refused"));
    }
    let mut r = Reader::new(frame.payload);
    let protocol_version = r.u16_le()?;
    let _options = r.u16_le()?;
    if frame.header.session_handle == 0 {
        return Err(DecodeError::Invalid(
            "RegisterSession reply carries a zero session handle",
        ));
    }
    Ok(Registered {
        session_handle: frame.header.session_handle,
        protocol_version,
    })
}

/// `UnregisterSession` request bytes. The target closes the TCP connection
/// without replying.
#[must_use]
pub fn unregister_session(session_handle: u32, sender_context: [u8; 8]) -> Vec<u8> {
    Frame::encode(Command::UnregisterSession, session_handle, sender_context, &[])
}

/// `SendRRData` request carrying one Message Router request unconnected.
/// `timeout` is the encapsulation timeout in seconds; targets ignore it for
/// unconnected messages but 0 is the conventional value.
#[must_use]
pub fn send_rr_data(
    session_handle: u32,
    sender_context: [u8; 8],
    timeout: u16,
    request: &cip::Request,
) -> Vec<u8> {
    let mut mr = Writer::default();
    request.encode(&mut mr);
    let mr = mr.into_inner();
    let items = [
        Item {
            type_id: cpf::item::NULL_ADDRESS,
            data: &[],
        },
        Item {
            type_id: cpf::item::UNCONNECTED_DATA,
            data: &mr,
        },
    ];
    let mut w = Writer::default();
    w.u32_le(0).u16_le(timeout);
    cpf::encode(&items, &mut w);
    Frame::encode(
        Command::SendRrData,
        session_handle,
        sender_context,
        &w.into_inner(),
    )
}

/// Pull the Message Router response out of a `SendRRData` reply.
///
/// # Errors
/// `Invalid` if the frame is the wrong command, reports an encapsulation
/// error, or carries no unconnected data item; `Truncated` on short data.
pub fn decode_send_rr_data(frame: &Frame<'_>) -> Result<cip::Response, DecodeError> {
    if frame.header.command != Command::SendRrData as u16 {
        return Err(DecodeError::Invalid("not a SendRRData reply"));
    }
    if frame.header.status != Status::Success as u32 {
        return Err(DecodeError::Invalid(
            "SendRRData reply reports an encapsulation error",
        ));
    }
    let mut r = Reader::new(frame.payload);
    let _interface_handle = r.u32_le()?;
    let _timeout = r.u16_le()?;
    let items = cpf::decode(r.rest())?;
    let data = cpf::find(&items, cpf::item::UNCONNECTED_DATA).ok_or(DecodeError::Invalid(
        "SendRRData reply has no unconnected data item",
    ))?;
    cip::Response::decode(data.data)
}

/// Split a `SendRRData` *request* into its Message Router request. Used by
/// the test target and by anything that wants to answer explicit messages.
///
/// # Errors
/// As [`decode_send_rr_data`], for the request direction.
pub fn decode_send_rr_data_request(frame: &Frame<'_>) -> Result<cip::Request, DecodeError> {
    if frame.header.command != Command::SendRrData as u16 {
        return Err(DecodeError::Invalid("not a SendRRData request"));
    }
    let mut r = Reader::new(frame.payload);
    let _interface_handle = r.u32_le()?;
    let _timeout = r.u16_le()?;
    let items = cpf::decode(r.rest())?;
    match items.as_slice() {
        [addr, data, ..]
            if addr.type_id == cpf::item::NULL_ADDRESS && data.type_id == cpf::item::UNCONNECTED_DATA =>
        {
            cip::Request::decode(data.data)
        }
        _ => Err(DecodeError::Invalid(
            "SendRRData request is not null address + unconnected data",
        )),
    }
}

/// Build a `SendRRData` reply frame around a Message Router response.
#[must_use]
pub fn encode_send_rr_data_reply(
    session_handle: u32,
    sender_context: [u8; 8],
    response: &cip::Response,
) -> Vec<u8> {
    let mut mr = Writer::default();
    response.encode(&mut mr);
    let mr = mr.into_inner();
    let items = [
        Item {
            type_id: cpf::item::NULL_ADDRESS,
            data: &[],
        },
        Item {
            type_id: cpf::item::UNCONNECTED_DATA,
            data: &mr,
        },
    ];
    let mut w = Writer::default();
    w.u32_le(0).u16_le(0);
    cpf::encode(&items, &mut w);
    Frame::encode(
        Command::SendRrData,
        session_handle,
        sender_context,
        &w.into_inner(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_session_layout() {
        let bytes = register_session([9; 8]);
        assert_eq!(bytes.len(), 28);
        assert_eq!(&bytes[..4], &[0x65, 0x00, 0x04, 0x00]);
        assert_eq!(&bytes[24..], &[0x01, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn send_rr_data_layout() {
        let req = cip::Request::get_attribute_single(1, 1, 1);
        let bytes = send_rr_data(0x1234, [0; 8], 0, &req);
        let (frame, rest) = Frame::decode(&bytes).unwrap();
        assert_eq!(rest, []);
        assert_eq!(frame.header.session_handle, 0x1234);
        // interface handle (4) + timeout (2) + count (2) + null item (4) + data item header (4) + MR request (8)
        assert_eq!(frame.payload.len(), 4 + 2 + 2 + 4 + 4 + 8);
        assert_eq!(
            &frame.payload[6..],
            &[
                2, 0, 0, 0, 0, 0, 0xB2, 0, 8, 0, 0x0E, 3, 0x20, 1, 0x24, 1, 0x30, 1
            ]
        );
        assert_eq!(decode_send_rr_data_request(&frame).unwrap(), req);
    }

    #[test]
    fn reply_round_trip() {
        let resp = cip::Response::ok(cip::service::GET_ATTRIBUTE_SINGLE, vec![0xAB, 0xCD]);
        let bytes = encode_send_rr_data_reply(7, [1; 8], &resp);
        let (frame, _) = Frame::decode(&bytes).unwrap();
        assert_eq!(decode_send_rr_data(&frame).unwrap(), resp);
    }

    #[test]
    fn encapsulation_error_surfaces() {
        let mut bytes = encode_send_rr_data_reply(7, [1; 8], &cip::Response::ok(0x0E, vec![]));
        bytes[8] = Status::InvalidSessionHandle as u8;
        let (frame, _) = Frame::decode(&bytes).unwrap();
        assert!(matches!(
            decode_send_rr_data(&frame),
            Err(DecodeError::Invalid(_))
        ));
    }
}
