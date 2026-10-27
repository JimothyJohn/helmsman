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

    // `Get_Attributes_All` returns the same fields packed in attribute order.
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

mod class1 {
    //! Class 1 I/O against `OpENer`'s sample application: output assembly
    //! 150 (32 bytes, consumed with a run/idle header), input assembly 100
    //! (32 bytes, produced without one), configuration 151, heartbeat
    //! instance 152 for `Input Only` (`sampleapplication.c`). The sample
    //! copies the output assembly into the input assembly whenever output
    //! data arrives, so what we send comes straight back.
    //!
    //! The data-plane tests need T->O datagrams to reach this host on UDP
    //! 2222, which Docker on Linux delivers and Docker Desktop on macOS
    //! does not. They run when `TCPETHERNET_OPENER_IO` is set (CI sets it);
    //! the refusal test needs only the TCP side.

    use std::net::{Ipv4Addr, SocketAddrV4};
    use std::time::Duration;

    use tcpethernet::encap;
    use tcpethernet::io;
    use tcpethernet::scanner::{Error, IoConfig, IoConnection, bind_io_socket};
    use tcpethernet::session::Session;

    const CFG: u16 = 151;
    const OUT: u16 = 150;
    const IN: u16 = 100;
    const HEARTBEAT: u16 = 152;
    const LEN: u16 = 32;
    const RPI: Duration = Duration::from_millis(50);

    /// `OpENer`'s sample app has one exclusive-owner connection point on
    /// assembly 150. Two tests opening it at once make the second fail
    /// with 0x0106 (ownership conflict) instead of what it is testing, so
    /// every test here that sends a `Forward_Open` holds this lock.
    static OPENER_CONNECTIONS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serialize() -> std::sync::MutexGuard<'static, ()> {
        OPENER_CONNECTIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn io_target() -> Option<Ipv4Addr> {
        let ip = super::target()?;
        if std::env::var_os("TCPETHERNET_OPENER_IO").is_none() {
            eprintln!("TCPETHERNET_OPENER_IO not set; data-plane interop test skipped");
            return None;
        }
        Some(ip)
    }

    #[test]
    fn wrong_o_to_t_size_is_refused_with_0x0127_and_the_size_opener_wants() {
        let Some(ip) = super::target() else { return };
        let _one_at_a_time = serialize();
        let mut s = Session::connect((ip, encap::PORT), super::TIMEOUT).unwrap();
        let sock = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let mut cfg = IoConfig::exclusive_owner(CFG, OUT, IN, LEN, LEN, RPI);
        cfg.o_to_t_run_idle = false; // claims 34 bytes; OpENer wants 38
        let err = IoConnection::open(&mut s, &sock, SocketAddrV4::new(ip, encap::IO_PORT), cfg).unwrap_err();
        match err {
            Error::Refused(r) => {
                assert_eq!(r.extended_code(), Some(io::extended::INVALID_O_TO_T_SIZE), "{r}");
                assert_eq!(r.extended, vec![0x0127, 38], "{r}");
            }
            other => panic!("expected Refused, got {other}"),
        }
        s.close();
    }

    #[test]
    fn exclusive_owner_and_input_only_against_opener() {
        let Some(ip) = io_target() else { return };
        let _one_at_a_time = serialize();
        let mut s = Session::connect((ip, encap::PORT), super::TIMEOUT).unwrap();
        let sock = bind_io_socket(Ipv4Addr::UNSPECIFIED).expect("UDP 2222 must be free for the originator");
        let target = SocketAddrV4::new(ip, encap::IO_PORT);

        let cfg = IoConfig::exclusive_owner(CFG, OUT, IN, LEN, LEN, RPI);
        let conn = IoConnection::open(&mut s, &sock, target, cfg).unwrap();
        assert_eq!(conn.reply().o_to_t_api_us, 50_000);
        assert_ne!(conn.reply().o_to_t_connection_id, 0);

        let pattern: Vec<u8> = (0..LEN).map(|i| u8::try_from(i * 3 % 251).unwrap()).collect();
        conn.set_output(&pattern).unwrap();
        let input = conn
            .wait_for_input_eq(&pattern, Duration::from_secs(3))
            .expect("no T->O data from OpENer within 3s");
        assert_eq!(input.data, pattern, "OpENer did not echo the output assembly");
        assert_eq!(input.run_idle, None);
        assert!(conn.is_alive());
        assert_eq!(conn.packets_ignored(), 0);
        let close = conn.close(&mut s).unwrap();
        assert_eq!(close.originator_vendor_id, 0xFFFF);

        let cfg = IoConfig::input_only(CFG, HEARTBEAT, IN, LEN, RPI);
        let conn = IoConnection::open(&mut s, &sock, target, cfg).unwrap();
        let input = conn
            .wait_for_input(Duration::from_secs(3))
            .expect("no T->O on input-only");
        assert_eq!(input.data.len(), usize::from(LEN));
        // The input assembly still holds what the exclusive-owner connection wrote.
        assert_eq!(input.data, pattern);
        conn.close(&mut s).unwrap();
        s.close();
    }
}
