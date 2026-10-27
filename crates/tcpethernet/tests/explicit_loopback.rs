//! End-to-end explicit messaging against a loopback TCP target that speaks
//! the encapsulation layer the way `OpENer` does: register, answer
//! `Get_Attribute_Single` on an Identity object, reject bad sessions and
//! unknown attributes with the right status codes.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use tcpethernet::cip::{self, GeneralStatus, class, service};
use tcpethernet::codec::{Reader, Writer};
use tcpethernet::encap::{Command, Frame, HEADER_LEN, Header, Status};
use tcpethernet::explicit;
use tcpethernet::session::{Error, Session};

const VENDOR_ID: u16 = 1;
const PRODUCT_CODE: u16 = 65001;
const SERIAL: u32 = 0xCAFE_F00D;
const NAME: &[u8] = b"loopback target";
const SESSION: u32 = 0x0000_0042;

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut head = [0u8; HEADER_LEN];
    stream.read_exact(&mut head).ok()?;
    let header = Header::decode(&mut Reader::new(&head)).ok()?;
    let mut out = head.to_vec();
    out.resize(HEADER_LEN + usize::from(header.length), 0);
    stream.read_exact(&mut out[HEADER_LEN..]).ok()?;
    Some(out)
}

fn identity_attribute(attr: u16) -> Option<Vec<u8>> {
    let mut w = Writer::default();
    match attr {
        1 => w.u16_le(VENDOR_ID),
        2 => w.u16_le(0x0C),
        3 => w.u16_le(PRODUCT_CODE),
        4 => w.u8(2).u8(3),
        5 => w.u16_le(0x0030),
        6 => w.u32_le(SERIAL),
        7 => w.u8(u8::try_from(NAME.len()).unwrap()).bytes(NAME),
        _ => return None,
    };
    Some(w.into_inner())
}

/// Serve one client then exit.
fn spawn_target() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut registered = false;
        while let Some(bytes) = read_frame(&mut stream) {
            let (frame, _) = Frame::decode(&bytes).expect("client sent a malformed frame");
            let ctx = frame.header.sender_context;
            let reply = match Command::from_u16(frame.header.command) {
                Some(Command::RegisterSession) => {
                    registered = true;
                    let mut header = Header::request(Command::RegisterSession, 4, ctx);
                    header.session_handle = SESSION;
                    let mut out = header.encode().to_vec();
                    out.extend_from_slice(&[1, 0, 0, 0]);
                    out
                }
                Some(Command::UnregisterSession) => return,
                Some(Command::SendRrData) => {
                    if !registered || frame.header.session_handle != SESSION {
                        let mut header = Header::request(Command::SendRrData, 0, ctx);
                        header.session_handle = frame.header.session_handle;
                        header.status = Status::InvalidSessionHandle as u32;
                        header.encode().to_vec()
                    } else {
                        let req = explicit::decode_send_rr_data_request(&frame).expect("bad SendRRData");
                        let resp = match (req.service, req.path.class, req.path.instance) {
                            (service::GET_ATTRIBUTE_SINGLE, class::IDENTITY, 1) => {
                                match req.path.attribute.and_then(identity_attribute) {
                                    Some(data) => cip::Response::ok(req.service, data),
                                    None => cip::Response::error(
                                        req.service,
                                        GeneralStatus::ATTRIBUTE_NOT_SUPPORTED,
                                    ),
                                }
                            }
                            (_, class::IDENTITY, _) => {
                                cip::Response::error(req.service, GeneralStatus::SERVICE_NOT_SUPPORTED)
                            }
                            _ => cip::Response::error(req.service, GeneralStatus::PATH_DESTINATION_UNKNOWN),
                        };
                        explicit::encode_send_rr_data_reply(SESSION, ctx, &resp)
                    }
                }
                _ => {
                    let mut header = Header::request(Command::Nop, 0, ctx);
                    header.command = frame.header.command;
                    header.status = Status::InvalidCommand as u32;
                    header.encode().to_vec()
                }
            };
            stream.write_all(&reply).unwrap();
        }
    });
    (port, handle)
}

#[test]
fn reads_identity_attributes_over_a_registered_session() {
    let (port, target) = spawn_target();
    let mut s = Session::connect((Ipv4Addr::LOCALHOST, port), Duration::from_secs(2)).unwrap();
    assert_eq!(s.session_handle(), SESSION);

    let vendor = s.get_attribute_single(class::IDENTITY, 1, 1).unwrap();
    assert_eq!(vendor, VENDOR_ID.to_le_bytes());
    let serial = s.get_attribute_single(class::IDENTITY, 1, 6).unwrap();
    assert_eq!(serial, SERIAL.to_le_bytes());
    let name = s.get_attribute_single(class::IDENTITY, 1, 7).unwrap();
    assert_eq!(&name[1..], NAME);

    s.close();
    target.join().unwrap();
}

#[test]
fn cip_errors_are_typed() {
    let (port, target) = spawn_target();
    let mut s = Session::connect((Ipv4Addr::LOCALHOST, port), Duration::from_secs(2)).unwrap();

    let err = s.get_attribute_single(class::IDENTITY, 1, 99).unwrap_err();
    match &err {
        Error::Cip { status, additional } => {
            assert_eq!(*status, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED, "{err}");
            assert_eq!(*additional, Vec::<u16>::new());
        }
        other => panic!("expected Cip error, got {other}"),
    }
    assert_eq!(err.to_string(), "cip status 0x14 attribute not supported");

    let err = s
        .request(&cip::Request::new(0x33, cip::Path::new(class::IDENTITY, 1)))
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Cip {
                status: GeneralStatus::SERVICE_NOT_SUPPORTED,
                ..
            }
        ),
        "{err}"
    );

    let err = s.get_attribute_single(0x7F, 1, 1).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Cip {
                status: GeneralStatus::PATH_DESTINATION_UNKNOWN,
                ..
            }
        ),
        "{err}"
    );

    // request_raw hands back the error response instead of failing.
    let raw = s
        .request_raw(&cip::Request::get_attribute_single(class::IDENTITY, 1, 99))
        .unwrap();
    assert_eq!(raw.status, GeneralStatus::ATTRIBUTE_NOT_SUPPORTED);
    assert_eq!(raw.request_service(), service::GET_ATTRIBUTE_SINGLE);

    s.close();
    target.join().unwrap();
}

#[test]
fn unregistered_session_is_an_encapsulation_error() {
    // Talk to the target with a raw socket and a bogus handle; the reply
    // must be status 0x64 with no CPF, and our decoder must say so.
    let (port, target) = spawn_target();
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let req = cip::Request::get_attribute_single(class::IDENTITY, 1, 1);
    stream
        .write_all(&explicit::send_rr_data(0xDEAD, [5; 8], 0, &req))
        .unwrap();
    let bytes = read_frame(&mut stream).unwrap();
    let (frame, _) = Frame::decode(&bytes).unwrap();
    assert_eq!(frame.header.status, Status::InvalidSessionHandle as u32);
    assert_eq!(frame.header.sender_context, [5; 8]);
    assert_eq!(frame.payload, []);
    assert!(explicit::decode_send_rr_data(&frame).is_err());
    stream
        .write_all(&explicit::unregister_session(0xDEAD, [6; 8]))
        .unwrap();
    drop(stream);
    target.join().unwrap();
}

#[test]
fn connect_to_a_closed_port_fails_fast() {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let err = Session::connect((Ipv4Addr::LOCALHOST, port), Duration::from_millis(500)).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err}");
}
