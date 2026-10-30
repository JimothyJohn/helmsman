//! The I/O map file: the documented example parses, every malformed line is
//! reported with its number, and any map round-trips through render/parse.

use proptest::prelude::*;
use tcpethernet_io::text::{MapFile, parse, render};
use tcpethernet_io::{Binding, Device, Dir, IoMap, Signal, Ty, Value};

const EXAMPLE: &str = "
# remote I/O block on the bench
device rio modbus-tcp 192.168.3.50:502 unit=1 timeout_ms=200 poll_ms=50
in  start  bool rio di:1
in  level  int  rio hr:5:i16   # trailing comment
out motor  bool rio coil:0
\tout lamp bool box do:0  safe=true
out speed  real rio hr:8:f32 safe=-1.5
";

#[test]
fn the_documented_example_parses_and_validates() {
    let f = parse(EXAMPLE).unwrap();
    assert_eq!(f.map.devices.len(), 1);
    let d = &f.map.devices[0];
    assert_eq!(
        (d.id.as_str(), d.driver.as_str(), d.endpoint.as_str()),
        ("rio", "modbus-tcp", "192.168.3.50:502")
    );
    assert_eq!(d.params.get("poll_ms").map(String::as_str), Some("50"));
    let names: Vec<&str> = f.map.signals.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["start", "level", "motor", "lamp", "speed"]);
    assert_eq!(
        f.map.signals[3].binding,
        Binding::Local {
            channel: "do:0".into()
        }
    );
    assert_eq!(
        f.map.signals[1].binding,
        Binding::Device {
            device: "rio".into(),
            point: "hr:5:i16".into()
        }
    );
    assert_eq!(f.safe.get("lamp"), Some(&Value::Bool(true)));
    assert_eq!(f.safe.get("speed"), Some(&Value::Real(-1.5)));
    f.map.validate().unwrap();
}

#[test]
fn every_bad_line_is_reported_with_its_number() {
    let src = "\
device box modbus-tcp h:1
device 9x modbus-tcp h:1
device ok modbus-tcp h:1 unit
device ok2 modbus-tcp h:1 unit=1 unit=2
device short modbus-tcp
in a bool
in a word rio di:0
in a bool rio di:0 safe=true
out a bool rio coil:0 safe=1
out a int rio hr:0 safe=1.5
out a real rio hr:0:f32 safe=NaN
out a real rio hr:0:f32 safe=inf
out a bool rio coil:0 safe=true extra
signal a bool rio di:0
in a\u{7f} bool rio di:0
in a bool rio di:0\u{0}
in caf\u{e9} bool rio di:0
device =x modbus-tcp h:1 =v
";
    let errs = parse(src).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, (1..=18).collect::<Vec<_>>(), "{errs:#?}");
}

#[test]
fn blank_lines_and_comments_are_nothing() {
    assert_eq!(parse("").unwrap(), MapFile::default());
    assert_eq!(parse("\n  \n# x\n\t# y\n").unwrap(), MapFile::default());
}

fn token() -> impl Strategy<Value = String> {
    "[!-\"$-~]{1,12}" // printable ASCII but '#'
}

fn ident() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,8}".prop_filter("box is reserved", |s| s != "box")
}

fn file() -> impl Strategy<Value = MapFile> {
    let device = (
        ident(),
        token(),
        token(),
        prop::collection::btree_map("[a-z_]{1,6}", token(), 0..3),
    )
        .prop_map(|(id, driver, endpoint, params)| Device {
            id,
            driver,
            endpoint,
            params,
        });
    let signal = (
        ident(),
        0..3u8,
        any::<bool>(),
        prop::option::of(ident()),
        token(),
        any::<i64>(),
        -1e9f64..1e9,
        any::<bool>(),
    );
    (
        prop::collection::vec(device, 0..3),
        prop::collection::vec(signal, 0..6),
    )
        .prop_map(|(devices, sigs)| {
            let mut f = MapFile::default();
            f.map.devices = devices;
            for (k, (name, t, out, dev, point, n, x, with_safe)) in sigs.into_iter().enumerate() {
                let name = format!("{name}{k}");
                let ty = [Ty::Bool, Ty::Int, Ty::Real][usize::from(t)];
                let dir = if out { Dir::Out } else { Dir::In };
                let binding = match dev {
                    Some(device) => Binding::Device { device, point },
                    None => Binding::Local { channel: point },
                };
                if out && with_safe {
                    let v = match ty {
                        Ty::Bool => Value::Bool(n % 2 == 0),
                        Ty::Int => Value::Int(n),
                        Ty::Real => Value::Real(x),
                    };
                    f.safe.insert(name.clone(), v);
                }
                f.map.signals.push(Signal {
                    name,
                    ty,
                    dir,
                    binding,
                });
            }
            f
        })
}

proptest! {
    #[test]
    fn render_then_parse_is_identity(f in file()) {
        let text = render(&f);
        let back = parse(&text).map_err(|e| TestCaseError::fail(format!("{e:?}\n{text}")))?;
        prop_assert_eq!(&back.map, &f.map, "{}", text);
        prop_assert_eq!(&back.safe, &f.safe, "{}", text);
    }

    #[test]
    fn arbitrary_text_never_panics(src in "(?s).{0,300}") {
        let _ = parse(&src);
    }
}

#[test]
fn a_parsed_map_is_still_validated_separately() {
    let f = parse("in a bool nowhere di:0\nout a bool box di:0\n").unwrap();
    let errs = f.map.validate().unwrap_err();
    assert_eq!(errs.len(), 3, "{errs:?}");
    let _: &IoMap = &f.map;
}
