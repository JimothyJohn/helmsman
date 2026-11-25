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
    "[a-z_][a-z0-9_]{0,8}".prop_filter("box, module and sim are reserved", |s| {
        s != "box" && s != "module" && s != "sim"
    })
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
        (0..5u8, ident()),
        token(),
        any::<i64>(),
        -1e9f64..1e9,
        any::<bool>(),
        prop::option::of("[!-\"$-~]{1,16}"),
        prop::option::of(1u64..=60_000),
        prop::option::of("[a-z0-9][a-z0-9_-]{0,6}/[a-z0-9][a-z0-9_-]{0,6}"),
    );
    (
        prop::collection::vec(device, 0..3),
        prop::collection::vec(signal, 0..6),
    )
        .prop_map(|(devices, sigs)| {
            let mut f = MapFile::default();
            f.map.devices = devices;
            for (k, (name, t, out, (kind, dev), point, n, x, with_safe, unit, poll, wired)) in
                sigs.into_iter().enumerate()
            {
                let name = format!("{name}{k}");
                let ty = [Ty::Bool, Ty::Int, Ty::Real][usize::from(t)];
                // A module's output is only ever an `in` signal.
                // A screen tile only ever an `out` one.
                // A sim point has no safe value, poll or wired.
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
                    3 => Binding::Screen { tile: point },
                    _ => Binding::Sim,
                };
                let poll = poll.filter(|_| !out && kind < 2).map(Duration::from_millis);
                if out && with_safe && kind != 4 {
                    let v = match ty {
                        Ty::Bool => Value::Bool(n % 2 == 0),
                        Ty::Int => Value::Int(n),
                        Ty::Real => Value::Real(x),
                    };
                    f.safe.insert(name.clone(), v);
                }
                // Only a device's or the box's point has something wired to it.
                if let Some(w) = wired.filter(|_| kind < 2) {
                    f.wired.insert(name.clone(), w);
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
        prop_assert_eq!(&back.wired, &f.wired, "{}", text);
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

/// `wired=` names the sensor or actuator on a point by its folder in the
/// device library; the map keeps it and writes it back.
#[test]
fn what_is_wired_to_a_point_is_kept_by_signal() {
    let src = "\
device rio modbus-tcp 192.168.3.60:502
in  part_present bool rio di:3 wired=ifm/if6123
in  level int rio hr:5:i16 unit=mm wired=ifm/o5d100 poll_ms=500
out clamp bool box do:0 safe=false wired=festo/vuvg-l10
in  estop bool rio di:0
";
    let f = parse(src).unwrap();
    let wired: Vec<(&str, &str)> = f.wired.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        wired,
        [
            ("clamp", "festo/vuvg-l10"),
            ("level", "ifm/o5d100"),
            ("part_present", "ifm/if6123")
        ]
    );
    assert_eq!(f.map.signals.len(), 4);
    f.map.validate().unwrap();
    let text = render(&f);
    assert!(
        text.contains("in part_present bool rio di:3 wired=ifm/if6123\n"),
        "{text}"
    );
    assert!(
        !text
            .lines()
            .any(|l| l.starts_with("in estop") && l.contains("wired")),
        "{text}"
    );
    assert_eq!(parse(&text).unwrap(), f);
    // A map written before the option reads the same.
    assert!(parse(EXAMPLE).unwrap().wired.is_empty());
}

#[test]
fn a_wired_value_that_is_not_a_library_folder_is_reported_with_its_line() {
    let long = "a".repeat(65);
    let src = format!(
        "\
in a bool box di:0 wired=
in a bool box di:0 wired=ifm
in a bool box di:0 wired=ifm/
in a bool box di:0 wired=/if6123
in a bool box di:0 wired=ifm/if6123/x
in a bool box di:0 wired=IFM/IF6123
in a bool box di:0 wired=../etc
in a bool box di:0 wired=ifm/a..b
in a bool box di:0 wired=ifm/-x
in a bool box di:0 wired=ifm/{long}
in a bool box di:0 wired=ifm/a\u{7}
in a bool box di:0 wired=devices/ifm/if6123
in a bool box di:0 wired=ifm/a wired=ifm/b
in a real module m wired=ifm/a
out a int screen line1 wired=ifm/a
in a bool box di:0 wired
"
    );
    let errs = parse(&src).unwrap_err();
    let lines: Vec<usize> = errs.iter().map(|(n, _)| *n).collect();
    assert_eq!(lines, (1..=16).collect::<Vec<_>>(), "{errs:#?}");
    assert!(errs[1].1.contains("VENDOR/MODEL"), "{errs:#?}");
    assert!(
        errs[13].1.contains("only a signal on a device or box"),
        "{errs:#?}"
    );
    // The longest folder parts are taken.
    let ok = format!("in a bool box di:0 wired={0}/{0}\n", "a".repeat(64));
    assert!(parse(&ok).is_ok());
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

#[test]
fn sim_points_parse_print_and_validate_with_no_device() {
    let src = "in sw bool sim\nout total int sim unit=mm\nin rate real sim unit=m/s # a comment\n";
    let f = parse(src).unwrap();
    assert_eq!(f.map.devices.len(), 0);
    assert_eq!(f.map.signals.len(), 3);
    assert!(f.map.signals.iter().all(|s| s.binding == Binding::Sim));
    assert_eq!(f.map.signals[1].unit.as_deref(), Some("mm"));
    assert_eq!(f.map.signals[1].dir, Dir::Out);
    f.map.validate().unwrap();
    let text = render(&f);
    assert_eq!(
        text,
        "in sw bool sim\nout total int sim unit=mm\nin rate real sim unit=m/s\n"
    );
    assert_eq!(parse(&text).unwrap(), f, "print then parse keeps the meaning");
    // Beside a device, in any order.
    let mixed =
        parse("in a bool box di:0\nin b bool sim\ndevice rio modbus-tcp h:502\nout c bool rio coil:0\n")
            .unwrap();
    mixed.map.validate().unwrap();
    assert_eq!(parse(&render(&mixed)).unwrap(), mixed);
}

#[test]
fn a_sim_line_takes_no_address_and_no_option_but_a_unit() {
    let bad = [
        "in a bool sim 1",
        "in a bool sim di:0",
        "in a bool sim poll_ms=5",
        "out a bool sim safe=true",
        "in a bool sim wired=ifm/if6123",
        "in a bool sim unit=",
        "in a bool sim unit=mm unit=cm",
        "in a bool sim bogus=1",
        "in a nope sim",
        "device sim modbus-tcp h:502",
        "in a bool sim\nin b bool sim poll_ms=5\n",
    ];
    for src in bad {
        let errs = parse(src).expect_err(src);
        assert!(
            !errs.is_empty() && errs.iter().all(|(n, why)| *n >= 1 && !why.is_empty()),
            "{src}: {errs:?}"
        );
    }
    let errs = parse("in a bool sim\nin b bool sim poll_ms=5\n").unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].0, 2, "the line number");
    assert!(errs[0].1.contains("poll_ms"), "{errs:?}");
    let errs = parse("device sim modbus-tcp h:502").unwrap_err();
    assert!(errs[0].1.contains("sim"), "{errs:?}");
}

#[test]
fn sim_names_stay_unique_with_the_device_points() {
    let f = parse("in a bool sim\nin a bool box di:0\n").unwrap();
    assert!(f.map.validate().is_err());
}

// ---- share lines: a Skipper's own Modbus TCP tables (docs/EDGE.md) ----

const SHARES: &str = "\
device rio modbus-tcp 192.168.3.50
in  start bool rio di:1
# shared with other controllers
share speed   int  hr:0        # they read
share run_cmd bool coil:0      # they write
share at_home bool di:0
share level   real ir:4:f32
share pos     int  hr:10:i32sw
";

#[test]
fn a_box_accepts_the_share_lines_a_skipper_runs() {
    let f = parse(SHARES).unwrap();
    assert_eq!(f.map.signals.len(), 1, "a share is no signal");
    let shares: Vec<(&str, Ty, &str)> = f
        .shares
        .iter()
        .map(|s| (s.name.as_str(), s.ty, s.address.as_str()))
        .collect();
    assert_eq!(
        shares,
        [
            ("speed", Ty::Int, "hr:0"),
            ("run_cmd", Ty::Bool, "coil:0"),
            ("at_home", Ty::Bool, "di:0"),
            ("level", Ty::Real, "ir:4:f32"),
            ("pos", Ty::Int, "hr:10:i32sw"),
        ]
    );
    let again = parse(&render(&f)).unwrap();
    assert_eq!(again, f, "the lines survive a render");
    // The box's project check takes the map as it takes any other.
    f.map.validate().unwrap();
}

#[test]
fn a_malformed_share_line_is_reported_with_its_number() {
    for (text, why) in [
        ("share\n", "want share"),
        ("share a int\n", "want share"),
        ("share a int hr:0 extra\n", "want share"),
        ("share a float hr:0\n", "bool, int or real"),
        ("share 1a int hr:0\n", "identifier"),
        ("share a int xx:0\n", "goes in ir or hr"),
        ("share a bool hr:0\n", "goes in di or coil"),
        ("share a int di:0\n", "goes in ir or hr"),
        ("share a int hr:x\n", "ADDR"),
        ("share a int hr:65536\n", "ADDR"),
        ("share a int hr:0:f32\n", "int's format"),
        ("share a real hr:0:u16\n", "f32 or f32sw"),
        ("share a real hr:0\n", "f32 or f32sw"),
        ("share a bool coil:0:u16\n", "no format"),
        ("share a int hr:0:u16:more\n", "too many"),
    ] {
        let errs = parse(&format!("# first\n{text}")).unwrap_err();
        assert_eq!(errs.len(), 1, "{text:?}: {errs:?}");
        assert_eq!(errs[0].0, 2, "{text:?}");
        assert!(errs[0].1.contains(why), "{text:?}: {errs:?}");
    }
    let errs = parse("share a int hr:0\nshare A int hr:1\n").unwrap_err();
    assert_eq!(errs[0].0, 2);
    assert!(errs[0].1.contains("shared twice"));
}
