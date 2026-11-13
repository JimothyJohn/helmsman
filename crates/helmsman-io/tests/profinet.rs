//! `profinet-io`: the device line and its points, and the driver against
//! the simulated device (helmsman-profinet's, on a link in the process).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use helmsman_io::driver::{self, Driver, Error};
use helmsman_io::profinet::{Config, Link, Opener, Point, ProfinetDriver, SimLink};
use helmsman_io::text;
use helmsman_io::{Ty, Value};
use helmsman_profinet::device::{self, Device};

/// The Novanta `MDrive`'s shape (docs/devices/novanta-mdrive.md): an access
/// point without data, 128 bytes in, 128 bytes out.
const SLOTS: &str = "0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128";

fn params(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut p: BTreeMap<String, String> = [("slots", SLOTS), ("if", "en0")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    for (k, v) in extra {
        p.insert((*k).to_string(), (*v).to_string());
    }
    p
}

fn config(extra: &[(&str, &str)]) -> Result<Config, String> {
    Config::parse("mdrive", &params(extra)).map_err(|e| e.to_string())
}

/// A driver wired to a simulated device built as the line describes, and
/// the handle to that device.
fn pair(extra: &[(&str, &str)]) -> (ProfinetDriver, Arc<Mutex<Device>>) {
    let cfg = config(extra).unwrap();
    let station = device::station("mdrive", 0x0261, 0x0001, [192, 168, 3, 70]);
    let dev = Arc::new(Mutex::new(Device::new(
        [2, 0, 0, 0, 0, 9],
        station,
        cfg.device.modules.clone(),
        3,
    )));
    let handle = Arc::clone(&dev);
    let opener: Opener = Arc::new(move |_| Ok(Box::new(SimLink::new(Arc::clone(&dev))) as Box<dyn Link>));
    (
        ProfinetDriver::new("mdrive", &params(extra), opener).unwrap(),
        handle,
    )
}

/// Polls `f` until it is `Some`, for at most five seconds.
fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn up(d: &mut ProfinetDriver) {
    wait("cyclic data", || d.read(&["in:0:u8"]).ok());
}

#[test]
fn the_device_line_reads_into_modules_timing_and_address() {
    let c = config(&[
        ("ip", "192.168.3.70/24"),
        ("gateway", "192.168.3.1"),
        ("cycle_ms", "16"),
        ("hold", "5"),
        ("vendor", "0x0261"),
        ("device", "1"),
        ("startup", "advanced"),
        ("vlan", "off"),
        ("frame_id", "0xC010"),
        ("records", "2.1.0x64:0102ff;1.1.16:00"),
        ("poll_ms", "10"),
    ])
    .unwrap();
    assert_eq!(c.interface.as_deref(), Some("en0"));
    let d = &c.device;
    assert_eq!(d.station_name, "mdrive");
    assert_eq!(d.modules.len(), 3);
    assert_eq!((d.modules[1].slot, d.modules[1].ident), (1, 0x30));
    let sub = &d.modules[1].submodules[0];
    assert_eq!((sub.subslot, sub.ident, sub.input, sub.output), (1, 1, 128, 0));
    assert_eq!(d.modules[2].submodules[0].output, 128);
    assert_eq!(d.modules[2].submodules[0].records[0].data, [1, 2, 0xFF]);
    assert_eq!(d.modules[1].submodules[0].records[0].index, 16);
    let a = d.address.unwrap();
    assert_eq!(
        (a.ip, a.mask, a.gateway),
        ([192, 168, 3, 70], [255, 255, 255, 0], [192, 168, 3, 1])
    );
    assert_eq!((d.reduction_ratio, d.data_hold_factor), (16, 5));
    assert_eq!((d.vendor_id, d.device_id), (Some(0x0261), Some(1)));
    assert!(d.advanced_startup);
    assert!(!d.vlan);
    assert_eq!(d.input_frame_id, 0xC010);
}

#[test]
fn every_malformed_param_is_refused_in_words() {
    let err = |extra: &[(&str, &str)]| config(extra).unwrap_err();
    assert!(
        Config::parse("mdrive", &BTreeMap::new())
            .unwrap_err()
            .to_string()
            .contains("slots= is missing")
    );
    assert!(err(&[("slots", "1")]).contains("want SLOT:MODULE/SUBSLOT:SUBMODULE"));
    assert!(err(&[("slots", "1:0x30")]).contains("at least one /SUBSLOT:SUBMODULE"));
    assert!(err(&[("slots", "1:0x30/1:1:i0")]).contains("1..=1440"));
    assert!(err(&[("slots", "1:0x30/1:1:i4:i4")]).contains("once each"));
    assert!(err(&[("slots", "1:0x30/1:1,1:0x31/1:1")]).contains("slot is listed twice"));
    assert!(err(&[("slots", "1:0x30/1:1/1:2")]).contains("subslot is listed twice"));
    assert!(err(&[("slots", "1:0x30/1:1:i1000,2:0x30/1:1:i1000")]).contains("1440"));
    assert!(err(&[("ip", "192.168.3.70")]).contains("A.B.C.D/BITS"));
    assert!(err(&[("ip", "192.168.3.70/33")]).contains("A.B.C.D/BITS"));
    assert!(err(&[("gateway", "192.168.3.1")]).contains("gateway= without ip="));
    assert!(err(&[("cycle_ms", "10")]).contains("1, 2, 4 .. 512"));
    assert!(err(&[("hold", "0")]).contains("1 or more"));
    assert!(err(&[("hold", "300")]).contains("hold time"));
    assert!(err(&[("vendor", "0x10000")]).contains("0xFFFF"));
    assert!(err(&[("startup", "fast")]).contains("legacy or advanced"));
    assert!(err(&[("vlan", "yes")]).contains("on or off"));
    assert!(err(&[("frame_id", "0x8000")]).contains("0xC000..=0xF7FF"));
    assert!(err(&[("records", "9.1.1:00")]).contains("no such slot and subslot"));
    assert!(err(&[("records", "1.1.1:0")]).contains("whole bytes of hex"));
    assert!(err(&[("if", "en 0")]).contains("interface name"));
    assert!(err(&[("unit", "1")]).contains("unit: not a profinet-io param"));
    let name = Config::parse("M Drive", &params(&[])).unwrap_err().to_string();
    assert!(name.contains("is not a station name"), "{name}");
}

#[test]
fn points_parse_with_types_and_are_checked_against_the_images() {
    let c = config(&[]).unwrap();
    let info = c.check("in:127.7").unwrap();
    assert_eq!((info.ty, info.writable), (Ty::Bool, false));
    let info = c.check("out:124:f32").unwrap();
    assert_eq!((info.ty, info.writable), (Ty::Real, true));
    assert_eq!(c.check("in:0:u16le").unwrap().ty, Ty::Int);
    let past = c.check("in:127:u16").unwrap_err().to_string();
    assert!(
        past.contains("128 input bytes, this point ends at byte 129"),
        "{past}"
    );
    assert!(c.check("out:128.0").is_err());
    for bad in [
        "in:0",
        "in:0.8",
        "di:0",
        "in:0:u64",
        "in:0:u8le",
        "out:x:u8",
        "in::u8",
        "",
    ] {
        assert!(Point::parse(bad).is_err(), "{bad}");
    }
    // The same checks through the map: a point past the image is a map
    // error before anything runs, and two outputs on one bit collide.
    let map = "
device axis profinet-io mdrive if=en0 slots=0:0x1/1:0x1,1:0x30/1:0x1:i128,2:0x31/1:0x1:o128
in  status int  axis in:0:u16
out speed  int  axis out:4:i32
out enable bool axis out:0.0
";
    let f = text::parse(map).unwrap();
    assert_eq!(f.map.validate(), Ok(()));
    let clash = format!("{map}out again bool axis out:7.7\n");
    let errs = text::parse(&clash).unwrap().map.validate().unwrap_err();
    assert_eq!(errs.len(), 1, "{errs:?}");
    let past = format!("{map}in far int axis in:200:u8\n");
    assert!(text::parse(&past).unwrap().map.validate().is_err());
}

#[test]
fn until_data_flows_a_read_says_what_the_startup_waits_for() {
    let (mut d, dev) = pair(&[]);
    dev.lock().unwrap().faults.never_ready = true;
    let e = wait("the startup to reach the ready wait", || {
        d.read(&["in:0:u8"])
            .err()
            .filter(|e| e.to_string().contains("waiting for the device to report ready"))
    });
    assert!(e.is_link(), "{e}");
    assert!(d.write("out:0:u8", Value::Int(1)).unwrap_err().is_link());
}

#[test]
fn inputs_read_and_outputs_write_in_every_format() {
    let (mut d, dev) = pair(&[]);
    up(&mut d);
    {
        let mut dev = dev.lock().unwrap();
        let image = dev.inputs_mut();
        image[..8].copy_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x80, 0x00, 0xFF, 0x85]);
        image[8..12].copy_from_slice(&1.5f32.to_be_bytes());
    }
    let read = |d: &mut ProfinetDriver, addr: &str, want: Value| {
        wait(addr, || d.read(&[addr]).ok().filter(|v| v[0] == want));
    };
    // Big-endian unless the format says otherwise.
    read(&mut d, "in:0:u16", Value::Int(0x1234));
    read(&mut d, "in:0:u16le", Value::Int(0x3412));
    read(&mut d, "in:0:u32", Value::Int(0x1234_5678));
    read(&mut d, "in:0:u32le", Value::Int(0x7856_3412));
    read(&mut d, "in:4:i16", Value::Int(-32768));
    read(&mut d, "in:6:i8", Value::Int(-1));
    read(&mut d, "in:7:u8", Value::Int(0x85));
    read(&mut d, "in:7.0", Value::Bool(true));
    read(&mut d, "in:7.1", Value::Bool(false));
    read(&mut d, "in:7.7", Value::Bool(true));
    read(&mut d, "in:8:f32", Value::Real(1.5));
    // One call, several points, in order.
    assert_eq!(
        d.read(&["in:7.7", "in:0:u16", "in:8:f32"]).unwrap(),
        [Value::Bool(true), Value::Int(0x1234), Value::Real(1.5)]
    );

    d.write("out:0:u16", Value::Int(0xBEEF)).unwrap();
    d.write("out:2:i32le", Value::Int(-2)).unwrap();
    d.write("out:6.3", Value::Bool(true)).unwrap();
    d.write("out:8:f32", Value::Real(-0.25)).unwrap();
    let mut want = vec![0xBE, 0xEF, 0xFE, 0xFF, 0xFF, 0xFF, 0x08, 0x00];
    want.extend_from_slice(&(-0.25f32).to_be_bytes());
    wait("the outputs at the device", || {
        (dev.lock().unwrap().outputs()[..12] == want[..]).then_some(())
    });
    // An output reads back what the box commands.
    assert_eq!(
        d.read(&["out:0:u16", "out:6.3"]).unwrap(),
        [Value::Int(0xBEEF), Value::Bool(true)]
    );
    d.write("out:6.3", Value::Bool(false)).unwrap();
    wait("the bit cleared", || {
        (dev.lock().unwrap().outputs()[6] == 0).then_some(())
    });
}

#[test]
fn values_a_format_cannot_hold_and_inputs_are_refused_as_points() {
    let (mut d, _dev) = pair(&[]);
    up(&mut d);
    let e = d.write("out:0:u8", Value::Int(256)).unwrap_err();
    assert!(matches!(e, Error::Point(_)) && !e.is_link(), "{e}");
    assert!(matches!(
        d.write("out:0:i16", Value::Int(40000)),
        Err(Error::Point(_))
    ));
    assert!(matches!(
        d.write("out:0:f32", Value::Real(f64::NAN)),
        Err(Error::Point(_))
    ));
    assert!(matches!(
        d.write("out:0:u8", Value::Bool(true)),
        Err(Error::Type { .. })
    ));
    assert!(matches!(
        d.write("out:0.0", Value::Int(1)),
        Err(Error::Type { .. })
    ));
    assert!(matches!(
        d.write("in:0:u8", Value::Int(1)),
        Err(Error::ReadOnly(_))
    ));
    // The driver is still up after each refusal.
    assert!(d.read(&["in:0:u8"]).is_ok());
}

#[test]
fn a_device_that_goes_quiet_is_a_link_error_and_comes_back_with_its_outputs() {
    let (mut d, dev) = pair(&[]);
    up(&mut d);
    d.write("out:0:u8", Value::Int(7)).unwrap();
    wait("the output", || {
        (dev.lock().unwrap().outputs()[0] == 7).then_some(())
    });
    dev.lock().unwrap().faults.silent = true;
    let e = wait("the link to go down", || d.read(&["in:0:u8"]).err());
    assert!(e.is_link(), "{e}");
    assert!(
        e.to_string().contains("\"mdrive\" stopped sending cyclic data"),
        "{e}"
    );
    // The device, no longer driven, lets its outputs go.
    wait("the device's outputs to drop", || {
        (dev.lock().unwrap().outputs()[0] == 0).then_some(())
    });
    dev.lock().unwrap().faults.silent = false;
    up(&mut d);
    // What the box last commanded is driven again without a new write.
    wait("the output again", || {
        (dev.lock().unwrap().outputs()[0] == 7).then_some(())
    });
    assert_eq!(d.address().as_deref(), Some("192.168.3.70"));
}

#[test]
fn a_device_built_differently_never_serves_data_and_says_which_slot() {
    let cfg = config(&[]).unwrap();
    let mut plugged = cfg.device.modules.clone();
    plugged[2].ident = 0x99;
    let dev = Arc::new(Mutex::new(Device::new(
        [2, 0, 0, 0, 0, 9],
        device::station("mdrive", 0x0261, 1, [192, 168, 3, 70]),
        plugged,
        3,
    )));
    let opener: Opener = Arc::new(move |_| Ok(Box::new(SimLink::new(Arc::clone(&dev))) as Box<dyn Link>));
    let mut d = ProfinetDriver::new("mdrive", &params(&[]), opener).unwrap();
    wait("the refusal", || {
        d.read(&["in:0:u8"]).err().filter(|e| {
            e.to_string()
                .contains("is not built as configured: slot 2 holds module 0x99")
        })
    });
}

#[test]
fn dropping_the_driver_releases_the_device() {
    let (mut d, dev) = pair(&[]);
    up(&mut d);
    assert!(dev.lock().unwrap().connected());
    drop(d);
    assert!(!dev.lock().unwrap().connected());
}

#[test]
fn a_line_without_an_interface_or_a_build_without_raw_ethernet_is_a_link_error() {
    let mut p = params(&[]);
    p.remove("if");
    let dev = helmsman_io::Device {
        id: "axis".into(),
        driver: "profinet-io".into(),
        endpoint: "mdrive".into(),
        params: p,
    };
    let mut d = driver::open(&dev).unwrap();
    let e = d.read(&["in:0:u8"]).unwrap_err();
    assert!(e.is_link() && e.to_string().contains("names no interface"), "{e}");
}

#[test]
fn the_simulated_interface_loops_outputs_back_through_the_boxs_own_open() {
    let dev = helmsman_io::Device {
        id: "axis".into(),
        driver: "profinet-io".into(),
        endpoint: "mdrive".into(),
        params: params(&[("if", "sim")]),
    };
    let mut d = driver::open(&dev).unwrap();
    wait("cyclic data", || d.read(&["in:0:u8"]).ok());
    d.write("out:3:u16", Value::Int(0x0102)).unwrap();
    d.write("out:5.4", Value::Bool(true)).unwrap();
    wait("the loop", || {
        d.read(&["in:3:u16", "in:5.4"])
            .ok()
            .filter(|v| v == &[Value::Int(0x0102), Value::Bool(true)])
    });
}

#[test]
fn the_device_maps_in_the_docs_load() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/devices/");
    let map = std::fs::read_to_string(format!("{root}novanta-mdrive-profinet.map")).unwrap();
    let f = text::parse(&map).unwrap();
    assert_eq!(f.map.validate(), Ok(()));
    assert_eq!(f.map.devices[0].driver, "profinet-io");
    // The device line in the UR's page, as written there.
    let page = std::fs::read_to_string(format!("{root}universal-robots.md")).unwrap();
    let line = page
        .lines()
        .find(|l| l.starts_with("device ur profinet-io"))
        .expect("the page has a device line");
    let f = text::parse(&format!(
        "{line}\nin state int ur in:0:u32\nout reg int ur out:24:u32\n"
    ))
    .unwrap();
    assert_eq!(f.map.validate(), Ok(()));
}

mod port {
    //! The box's UDP port for PROFINET's RPCs, shared by its links: each
    //! datagram goes to the link that last wrote to its sender.

    use std::net::UdpSocket;
    use std::time::{Duration, Instant};

    use helmsman_io::profinet::Event;
    use helmsman_io::profinet::raw::RpcPort;

    fn take(port: &RpcPort, owner: u64) -> Option<Event> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(e) = port.take(owner).unwrap() {
                return Some(e);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_datagram_goes_to_the_link_that_last_wrote_to_its_sender() {
        let port = RpcPort::bind(0).unwrap();
        let at = port.local_port().unwrap();
        let device = UdpSocket::bind("127.0.0.1:0").unwrap();
        device.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let device_port = device.local_addr().unwrap().port();
        let lo = [127, 0, 0, 1];

        port.send(1, lo, device_port, b"connect").unwrap();
        let mut buf = [0u8; 64];
        let (n, from) = device.recv_from(&mut buf).unwrap();
        assert_eq!((&buf[..n], from.port()), (&b"connect"[..], at));

        device.send_to(b"answer", ("127.0.0.1", at)).unwrap();
        assert_eq!(
            take(&port, 1),
            Some(Event::Udp {
                ip: lo,
                port: device_port,
                data: b"answer".to_vec()
            })
        );
        // Link 2 is handed nothing of link 1's device.
        device.send_to(b"again", ("127.0.0.1", at)).unwrap();
        assert_eq!(port.take(2).unwrap(), None);
        assert!(matches!(take(&port, 1), Some(Event::Udp { data, .. }) if data == b"again"));

        // Once link 2 writes to the device, it is link 2's.
        port.send(2, lo, device_port, b"mine now").unwrap();
        device.recv_from(&mut buf).unwrap();
        device.send_to(b"for two", ("127.0.0.1", at)).unwrap();
        assert!(matches!(take(&port, 2), Some(Event::Udp { data, .. }) if data == b"for two"));
        assert_eq!(port.take(1).unwrap(), None);

        // A link that goes away takes its devices with it.
        port.forget(2);
        device.send_to(b"late", ("127.0.0.1", at)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(port.take(2).unwrap(), None);
    }
}
