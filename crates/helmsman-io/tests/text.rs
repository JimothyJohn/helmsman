//! The I/O map file: the documented example parses, every malformed line is
//! reported with its number, and any map round-trips through render/parse.

use helmsman_io::text::{MapFile, parse, render};
use helmsman_io::{Binding, Device, Dir, IoMap, Signal, Ty, Value};
use proptest::prelude::*;
use std::time::Duration;

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
    "[a-z_][a-z0-9_]{0,8}".prop_filter("box and module are reserved", |s| s != "box" && s != "module")
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
        (0..4u8, ident()),
        token(),
        any::<i64>(),
        -1e9f64..1e9,
        any::<bool>(),
        prop::option::of("[!-\"$-~]{1,16}"),
        prop::option::of(1u64..=60_000),
    );
    (
        prop::collection::vec(device, 0..3),
        prop::collection::vec(signal, 0..6),
    )
        .prop_map(|(devices, sigs)| {
            let mut f = MapFile::default();
            f.map.devices = devices;
            for (k, (name, t, out, (kind, dev), point, n, x, with_safe, unit, poll)) in
                sigs.into_iter().enumerate()
            {
                let name = format!("{name}{k}");
                let ty = [Ty::Bool, Ty::Int, Ty::Real][usize::from(t)];
                // A module's output is only ever an `in` signal.
                // A screen tile only ever an `out` one.
                let kind = if (out && kind == 2) || (!out && kind == 3) {
                    0
                } else {
                    kind
                };
                let dir = if out { Dir::Out } else { Dir::In };
                let binding = match kind {
                    0 => Binding::Device { device: dev, point },
                    1 => Binding::Local { channel: point },
                    2 => Binding::Module { module: point },
                    _ => Binding::Screen { tile: point },
                };
                let poll = poll.filter(|_| !out && kind < 2).map(Duration::from_millis);
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
                    unit,
                    poll,
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

#[test]
fn units_poll_rates_and_module_outputs_parse_in_any_order() {
    let src = "
device rio modbus-tcp 10.0.0.2:502 poll_ms=50
in  level real rio ir:0:f32 poll_ms=500 unit=mm
in  slow  bool box di:3 unit=on/off poll_ms=60000
out lamp  bool box do:0 unit=% safe=true
out speed real rio hr:8:f32 safe=1.5 unit=m/s
in  rate  real module conveyor-2 unit=m/s
";
    let f = parse(src).unwrap();
    f.map.validate().unwrap();
    let s = &f.map.signals;
    assert_eq!(
        (s[0].unit.as_deref(), s[0].poll),
        (Some("mm"), Some(Duration::from_millis(500)))
    );
    assert_eq!(s[1].poll, Some(Duration::from_secs(60)));
    assert_eq!((s[2].unit.as_deref(), s[2].poll), (Some("%"), None));
    assert_eq!(f.safe.get("speed"), Some(&Value::Real(1.5)));
    assert_eq!(
        s[4].binding,
        Binding::Module {
            module: "conveyor-2".into()
        }
    );
    assert_eq!(parse(&render(&f)).unwrap(), f);
}

#[test]
fn a_map_written_before_units_reads_the_same() {
    let f = parse(EXAMPLE).unwrap();
    for s in &f.map.signals {
        assert_eq!((s.unit.as_deref(), s.poll), (None, None), "{}", s.name);
    }
    let text = render(&f);
    for line in text.lines().filter(|l| !l.starts_with("device")) {
        assert!(!line.contains("unit=") && !line.contains("poll_ms="), "{text}");
    }
}

#[test]
fn bad_units_poll_rates_and_module_lines_are_reported_with_their_numbers() {
    let src = "\
in a bool box di:0 unit=
in a bool box di:0 unit=abcdefghijklmnopq
in a bool box di:0 unit=a unit=b
in a bool box di:0 unit=a\u{7}
in a bool box di:0 unit=\u{1b}[31m
in a bool box di:0 unit=\u{b0}C
in a bool box di:0 poll_ms=0
in a bool box di:0 poll_ms=60001
in a bool box di:0 poll_ms=+5
in a bool box di:0 poll_ms=99999999999999999999999
in a bool box di:0 poll_ms=5 poll_ms=6
out a bool box do:0 poll_ms=5
in a real module m poll_ms=5
in a bool box di:0 safe=true
out a bool box do:0 safe=true safe=false
in a bool box di:0 colour=red
in a bool box di:0 unit
device module modbus-tcp h:1
";
    let errs = parse(src).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, (1..=18).collect::<Vec<_>>(), "{errs:#?}");
}

#[test]
fn screen_tiles_are_outputs_the_map_declares() {
    let f = parse("out line1_state int screen line1 safe=0\nout line1_parts int screen line1\n").unwrap();
    f.map.validate().unwrap();
    assert_eq!(f.map.signals[0].binding, Binding::Screen { tile: "line1".into() });
    assert_eq!(f.safe.get("line1_state"), Some(&Value::Int(0)));
    assert_eq!(parse(&render(&f)).unwrap(), f, "two signals may feed one tile");

    let bad = parse(
        "in a bool screen t1\nout b bool screen 1t\nout c bool screen t2 poll_ms=5\ndevice screen modbus-tcp h:1\n",
    );
    let lines: Vec<usize> = bad.as_ref().unwrap_err().iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, [3, 4], "{bad:?}");
    let f = parse("in a bool screen t1\nout b bool screen 1t\nout c real screen t2\n").unwrap();
    let errs = f.map.validate().unwrap_err();
    assert_eq!(errs.len(), 2, "an in screen signal and a bad tile id: {errs:?}");
}
