//! Interop against a real adapter: the `OpENer` POSIX sample, built from a
//! pinned commit in `docker/opener/`. Set `TCPETHERNET_OPENER_ADDR` to the
//! host that publishes ports 44818 tcp/udp (`make opener-up` → 127.0.0.1).
//! Without the variable the suite reports itself skipped and passes, so
//! `cargo test` stays usable on a machine without Docker; CI sets it.
//!
//! Expected identity values are `OpENer`'s `CMake` defaults
//! (`source/CMakeLists.txt`: vendor 1, device type 12, product code 65001,
//! `"OpENer PC"`) and `ports/POSIX/main.c` (`SetDeviceSerialNumber(123456789)`).
//! Expected error codes are what `OpENer` returns on each path:
//! `cipmessagerouter.c` `NotifyMessageRouter` (unknown class →
//! path destination unknown), `cipcommon.c` (unknown attribute → 0x14,
//! unknown service → 0x08), `encap.c` (bad session → 0x64).

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use tcpethernet::cip::{self, GeneralStatus, class, service};
use tcpethernet::codec::Reader;
use tcpethernet::discover::sweep;
use tcpethernet::encap::{self, Frame, Status};
use tcpethernet::explicit;
use tcpethernet::identity::IdentityItem;
use tcpethernet::session::{Error, Session};

const VENDOR_ID: u16 = 1;
const DEVICE_TYPE: u16 = 12;
const PRODUCT_CODE: u16 = 65001;
const SERIAL: u32 = 123_456_789;
const NAME: &[u8] = b"OpENer PC";
const TIMEOUT: Duration = Duration::from_secs(3);

fn target() -> Option<Ipv4Addr> {
    if let Ok(s) = std::env::var("TCPETHERNET_OPENER_ADDR") {
        Some(
            s.parse()
                .expect("TCPETHERNET_OPENER_ADDR must be an IPv4 address"),
        )
    } else {
        eprintln!("TCPETHERNET_OPENER_ADDR not set; interop test skipped");
        None
    }
}

fn discover_one(ip: Ipv4Addr) -> IdentityItem {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let result = sweep(&sock, SocketAddrV4::new(ip, encap::PORT), TIMEOUT).unwrap();
    assert_eq!(result.rejected, vec![], "{result:?}");
    assert_eq!(result.found.len(), 1, "{result:?}");
    result.found.into_iter().next().unwrap().identity
}

#[test]
fn list_identity_over_udp_unicast() {
    let Some(ip) = target() else { return };
    let id = discover_one(ip);
    assert_eq!(id.protocol_version, encap::PROTOCOL_VERSION);
    assert_eq!(id.socket_address.family, 2);
    assert_eq!(id.socket_address.port, encap::PORT);
    assert_eq!(id.vendor_id, VENDOR_ID);
    assert_eq!(id.device_type, DEVICE_TYPE);
    assert_eq!(id.product_code, PRODUCT_CODE);
    assert_eq!(id.serial_number, SERIAL);
    assert_eq!(id.product_name, NAME);
}

#[test]
fn identity_attributes_match_list_identity() {
    let Some(ip) = target() else { return };
    let id = discover_one(ip);
    let mut s = Session::connect((ip, encap::PORT), TIMEOUT).unwrap();
    assert_ne!(s.session_handle(), 0);

    let get = |s: &mut Session, a: u16| s.get_attribute_single(class::IDENTITY, 1, a).unwrap();
    assert_eq!(get(&mut s, 1), id.vendor_id.to_le_bytes());
    assert_eq!(get(&mut s, 2), id.device_type.to_le_bytes());
    assert_eq!(get(&mut s, 3), id.product_code.to_le_bytes());
    assert_eq!(get(&mut s, 4), [id.revision_major, id.revision_minor]);
    assert_eq!(get(&mut s, 6), id.serial_number.to_le_bytes());
    let name = get(&mut s, 7);
    assert_eq!(name[0] as usize, NAME.len());
    assert_eq!(&name[1..], NAME);

    // Get_Attributes_All returns the same fields packed in attribute order.
    let all = s
        .request(&cip::Request::get_attributes_all(class::IDENTITY, 1))
        .unwrap();
    let mut r = Reader::new(&all);
    assert_eq!(r.u16_le().unwrap(), VENDOR_ID);
    assert_eq!(r.u16_le().unwrap(), DEVICE_TYPE);
    assert_eq!(r.u16_le().unwrap(), PRODUCT_CODE);
    assert_eq!(r.u8().unwrap(), id.revision_major);
    assert_eq!(r.u8().unwrap(), id.revision_minor);
    let _status = r.u16_le().unwrap();
    assert_eq!(r.u32_le().unwrap(), SERIAL);
    let n = usize::from(r.u8().unwrap());
    assert_eq!(r.bytes(n).unwrap(), NAME);

    s.close();
}

#[test]
fn error_paths_return_the_codes_opener_documents() {
    let Some(ip) = target() else { return };
    let mut s = Session::connect((ip, encap::PORT), TIMEOUT).unwrap();

    let err = s.get_attribute_single(class::IDENTITY, 1, 99).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Cip {
                status: GeneralStatus::ATTRIBUTE_NOT_SUPPORTED,
                ..
            }
        ),
        "{err}"
    );

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

    let err = s
        .request(&cip::Request::get_attribute_single(0x7F, 1, 1))
        .unwrap_err();
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

    // The session still works after three error replies.
    assert_eq!(
        s.get_attribute_single(class::IDENTITY, 1, 1).unwrap(),
        VENDOR_ID.to_le_bytes()
    );
    s.close();
}

#[test]
fn unregistered_session_handle_is_refused_at_the_encapsulation_layer() {
    use std::io::{Read, Write};
    let Some(ip) = target() else { return };
    let mut stream = std::net::TcpStream::connect_timeout(&(ip, encap::PORT).into(), TIMEOUT).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let req = cip::Request::get_attribute_single(class::IDENTITY, 1, 1);
    stream
        .write_all(&explicit::send_rr_data(0xDEAD_BEEF, [9; 8], 0, &req))
        .unwrap();
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).unwrap();
    let (frame, _) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(frame.header.command, encap::Command::SendRrData as u16);
    assert_eq!(
        frame.header.status,
        Status::InvalidSessionHandle as u32,
        "{frame:?}"
    );
    assert_eq!(frame.header.sender_context, [9; 8]);
}

#[test]
fn service_bit_round_trips_through_a_real_target() {
    let Some(ip) = target() else { return };
    let mut s = Session::connect((ip, encap::PORT), TIMEOUT).unwrap();
    let resp = s
        .request_raw(&cip::Request::get_attribute_single(class::IDENTITY, 1, 1))
        .unwrap();
    assert_eq!(resp.request_service(), service::GET_ATTRIBUTE_SINGLE);
    assert!(resp.status.is_success(), "{resp:?}");
    assert_eq!(resp.additional_status, Vec::<u16>::new());
    s.close();
}
