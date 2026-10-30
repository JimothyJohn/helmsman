//! The `logix-tag` driver: addresses, map validation (read-only), and reads
//! through the `Driver` contract against the Logix fixture on loopback.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, TcpListener};

use helmsman_adapter::logix::{Controller, TagDef};
use helmsman_adapter::{Adapter, Device as Target, DeviceConfig, ServerConfig};
use helmsman_io::logix::{DRIVER, LogixDriver, Point};
use helmsman_io::{Binding, Device, Dir, Driver, Error, IoMap, MapError, Signal, Ty, Value, driver, text};
use helmsman_proto::logix::AtomicType;
use proptest::prelude::*;

#[test]
fn points_parse_with_their_type() {
    let cases = [
        ("Program:Main.Temp@real", Ty::Real),
        ("Precise@LREAL", Ty::Real),
        ("Count@dint", Ty::Int),
        ("Big@lint", Ty::Int),
        ("Ubig@ulint", Ty::Int),
        ("Run@bool", Ty::Bool),
        ("Status.5@bool", Ty::Bool),
        ("Local:1:I.Data@dint", Ty::Int),
        ("Recipe[3].Steps[2]@dint", Ty::Int),
    ];
    for (addr, ty) in cases {
        let info = driver::parse_point(DRIVER, addr).unwrap_or_else(|e| panic!("{addr}: {e}"));
        assert_eq!(info.ty, ty, "{addr}");
        assert!(!info.writable, "{addr} must be read-only");
    }
}

#[test]
fn bad_points_are_refused() {
    for addr in [
        "",
        "Temp",
        "Temp@",
        "@real",
        "Temp@float",
        "Temp@real@real",
        "Status.5@dint",
        "Status.64@bool",
        "Te mp@real",
        "Temp[1,2,3,4]@int",
        "../etc@dint",
        "Temp\n@real",
    ] {
        assert!(
            matches!(driver::parse_point(DRIVER, addr), Err(Error::Address(_))),
            "{addr:?}"
        );
    }
}

fn map(dir: Dir, ty: Ty, point: &str) -> IoMap {
    IoMap {
        devices: vec![Device {
            id: "plc".into(),
            driver: DRIVER.into(),
            endpoint: "127.0.0.1:44818".into(),
            params: BTreeMap::new(),
        }],
        signals: vec![Signal {
            name: "sig".into(),
            ty,
            dir,
            binding: Binding::Device {
                device: "plc".into(),
                point: point.into(),
            },
        }],
    }
}

#[test]
fn map_refuses_outputs_and_wrong_types() {
    assert_eq!(map(Dir::In, Ty::Real, "Temp@real").validate(), Ok(()));
    assert_eq!(
        map(Dir::Out, Ty::Real, "Temp@real").validate(),
        Err(vec![MapError::NotWritable("sig".into())])
    );
    assert_eq!(
        map(Dir::In, Ty::Int, "Temp@real").validate(),
        Err(vec![MapError::TypeMismatch {
            signal: "sig".into(),
            signal_ty: Ty::Int,
            point_ty: Ty::Real,
        }])
    );
}

#[test]
fn map_file_round_trips_a_logix_device() {
    let src = "device plc logix-tag 192.168.3.20:44818 slot=0 timeout_ms=500\n\
               in temp real plc Program:Main.Temp@real\n\
               in fault bool plc Status.5@bool\n";
    let file = text::parse(src).unwrap();
    assert_eq!(file.map.validate(), Ok(()));
    assert_eq!(text::parse(&text::render(&file)).unwrap(), file);
    let out = "device plc logix-tag 10.0.0.1\nout lamp bool plc Run@bool\n";
    assert!(text::parse(out).unwrap().map.validate().is_err());
}

fn plant() -> Controller {
    let mut c = Controller::new();
    c.add("Temp", TagDef::atomic(AtomicType::Real))
        .add("Count", TagDef::atomic(AtomicType::Dint))
        .add("Run", TagDef::atomic(AtomicType::Bool))
        .add("Status", TagDef::atomic(AtomicType::Dint))
        .add("Big", TagDef::atomic(AtomicType::Lint))
        .add("Ubig", TagDef::atomic(AtomicType::Ulint))
        .add("Log", TagDef::array(AtomicType::Uint, &[10]))
        .add_program_tag("Main", "Temp", TagDef::atomic(AtomicType::Lreal));
    c.set("Temp", &21.5f32.to_le_bytes()).unwrap();
    c.set("Count", &(-7i32).to_le_bytes()).unwrap();
    c.set("Run", &[1]).unwrap();
    c.set("Status", &(1i32 << 31).to_le_bytes()).unwrap();
    c.set("Big", &i64::MIN.to_le_bytes()).unwrap();
    c.set("Ubig", &u64::MAX.to_le_bytes()).unwrap();
    c.set("Log[9]", &65535u16.to_le_bytes()).unwrap();
    c.set("Program:Main.Temp", &80.25f64.to_le_bytes()).unwrap();
    c
}

fn start(c: Controller) -> Adapter {
    let cfg = ServerConfig {
        bind: Ipv4Addr::LOCALHOST,
        encap_port: 0,
        io_port: 0,
        ..ServerConfig::default()
    };
    Adapter::start(Target::new(DeviceConfig::opener_sample()).with_logix(c), &cfg).unwrap()
}

fn open(a: &Adapter, params: &[(&str, &str)]) -> Box<dyn Driver + Send> {
    driver::open(&Device {
        id: "plc".into(),
        driver: DRIVER.into(),
        endpoint: a.encap_addr().to_string(),
        params: params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    })
    .unwrap()
}

#[test]
fn reads_signals_through_the_driver_contract() {
    let a = start(plant());
    let mut d = open(&a, &[("timeout_ms", "2000")]);
    let got = d
        .read(&[
            "Temp@real",
            "Count@dint",
            "Run@bool",
            "Status.31@bool",
            "Status.0@bool",
            "Big@lint",
            "Log[9]@uint",
            "Program:Main.Temp@lreal",
        ])
        .unwrap();
    assert_eq!(
        got,
        [
            Value::Real(21.5),
            Value::Int(-7),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(i64::MIN),
            Value::Int(65535),
            Value::Real(80.25),
        ]
    );
    assert_eq!(d.read(&[]).unwrap(), []);
}

#[test]
fn a_tag_of_another_type_is_an_error_not_a_reinterpretation() {
    let a = start(plant());
    let mut d = open(&a, &[]);
    for addr in [
        "Temp@dint",
        "Count@real",
        "Count@udint",
        "Temp.3@bool",
        "Run@sint",
    ] {
        assert!(matches!(d.read(&[addr]), Err(Error::Protocol(_))), "{addr}");
    }
    // A ULINT above i64::MAX cannot become a signal int.
    assert!(matches!(d.read(&["Ubig@ulint"]), Err(Error::Protocol(_))));
    // The connection survived all of that.
    assert_eq!(d.read(&["Count@dint"]).unwrap(), [Value::Int(-7)]);
}

#[test]
fn one_unknown_tag_fails_the_read_and_the_next_read_works() {
    let a = start(plant());
    let mut d = open(&a, &[]);
    assert!(matches!(
        d.read(&["Temp@real", "Nope@dint"]),
        Err(Error::Protocol(_))
    ));
    assert_eq!(d.read(&["Temp@real"]).unwrap(), [Value::Real(21.5)]);
}

#[test]
fn writes_are_refused_without_touching_the_controller() {
    let a = start(plant());
    let mut d = open(&a, &[]);
    assert!(matches!(
        d.write("Temp@real", Value::Real(1.0)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(
        d.write("Run@bool", Value::Bool(false)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(
        d.write("bad", Value::Bool(false)),
        Err(Error::Address(_))
    ));
    d.read(&["Temp@real"]).unwrap();
    let dev = a.device();
    let target = dev.lock().unwrap();
    let seen = target.logix().unwrap().services_seen();
    assert!(seen.keys().all(|s| [0x4C, 0x0A].contains(s)), "{seen:02X?}");
}

#[test]
fn slot_param_routes_through_the_backplane() {
    let mut c = plant();
    c.slot = 1;
    let a = start(c);
    assert_eq!(
        open(&a, &[("slot", "1")]).read(&["Count@dint"]).unwrap(),
        [Value::Int(-7)]
    );
    assert!(open(&a, &[("slot", "4")]).read(&["Count@dint"]).is_err());
    let bad = Device {
        id: "plc".into(),
        driver: DRIVER.into(),
        endpoint: "127.0.0.1".into(),
        params: [("slot".to_string(), "300".to_string())].into(),
    };
    assert!(driver::open(&bad).is_err());
}

#[test]
fn unreachable_controller_is_an_io_error() {
    // A port that was just bound and released: nothing listens there.
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut d = LogixDriver::from_params(
        &format!("127.0.0.1:{port}"),
        &[("timeout_ms".to_string(), "200".to_string())].into(),
    )
    .unwrap();
    assert!(matches!(d.read(&["Temp@real"]), Err(Error::Io(_))));
}

proptest! {
    #[test]
    fn point_parse_never_panics(s in "\\PC{0,40}") {
        if let Ok(p) = Point::parse(&s) {
            prop_assert!(!p.info().writable);
        }
    }
}
