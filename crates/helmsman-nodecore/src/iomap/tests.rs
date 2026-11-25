#![allow(clippy::format_push_string)] // tests build map text a line at a time

use super::*;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use proptest::prelude::*;

/// The map a one-block node took before the multi-device work: its
/// addresses are the contract the layout keeps.
const BENCH: &str = "# the press\ndevice rio modbus-tcp 192.168.3.60 unit=1 timeout_ms=300\nin  estop  bool rio di:0\nin  run_pb bool rio di:3   # wired to the green button\nout motor  bool rio coil:0\nout lamp   bool rio coil:2\n";

/// Three devices of two kinds: the doc's example.
pub(crate) const THREE: &str = "\
device rio   modbus-tcp 192.168.3.60 unit=1
device drive enip-io    192.168.33.1 config=1 o2t=112:36 t2o=100:16 rpi_ms=20
device press modbus-tcp 192.168.3.71:502
in  estop  bool rio   di:0
out motor  bool rio   coil:0
in  pos    int  drive in:4:i32
out accel  int  drive out:4:u32
in  level  real press hr:5:f32
in  ready  bool drive in:0.1
out run    bool drive out:0.0
";

fn refused(text: &str) -> Bad {
    parse(text).unwrap_err()
}

fn at(m: &IoMap, name: &str) -> String {
    let p = m.points.iter().find(|p| p.name == name).expect(name);
    p.at.st(p.dir)
}

fn device_lines(n: usize) -> String {
    let mut text = String::new();
    for i in 0..n {
        text += &format!("device d{i} modbus-tcp 10.0.{}.{} unit=1\n", i / 200, i % 200 + 1);
    }
    text
}

#[test]
fn a_bench_map_parses_to_its_devices_and_points() {
    let m = parse(BENCH).unwrap();
    assert_eq!(m.devices.len(), 1);
    let d = &m.devices[0];
    assert_eq!(d.id, "rio");
    assert_eq!((d.host, d.port), ([192, 168, 3, 60], 502));
    assert_eq!(d.link, Link::Modbus { unit: 1 });
    assert_eq!(m.text, BENCH, "kept as written");
    let p = parse("device b modbus-tcp 10.0.0.9:1502 unit=7\n").unwrap();
    assert_eq!(
        (p.devices[0].host, p.devices[0].port, p.devices[0].link.clone()),
        ([10, 0, 0, 9], 1502, Link::Modbus { unit: 7 })
    );
    assert_eq!(p.points.len(), 0);
}

/// Every map a one-block node took keeps the addresses it had: `di:N` is
/// `%IX(N/8).(N%8)`, `coil:N` the same in `%Q`, whatever else is named.
#[test]
fn a_one_device_bool_map_keeps_the_addresses_it_always_had() {
    let m = parse(BENCH).unwrap();
    assert_eq!(at(&m, "estop"), "%IX0.0");
    assert_eq!(at(&m, "run_pb"), "%IX0.3");
    assert_eq!(at(&m, "motor"), "%QX0.0");
    assert_eq!(at(&m, "lamp"), "%QX0.2");
    // The signals list kept its order: inputs by index, then outputs.
    let names = |m: &IoMap| {
        m.layout()
            .iter()
            .map(|(n, _, _)| (*n).to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&m), ["estop", "run_pb", "motor", "lamp"]);

    let f = IoMap::factory([192, 168, 3, 60], "rio");
    for i in 0..8 {
        assert_eq!(at(&f, &format!("di{i}")), format!("%IX0.{i}"));
    }
    for i in 0..4 {
        assert_eq!(at(&f, &format!("do{i}")), format!("%QX0.{i}"));
    }
    assert_eq!(parse(&f.text).unwrap(), f, "its own text parses back");

    // The old limit: all sixteen of each, in any order of the lines.
    let mut text = String::from("device rio modbus-tcp 192.168.3.60\n");
    for i in (0..16).rev() {
        text += &format!("in  i{i} bool rio di:{i}\nout o{i} bool rio coil:{i}\n");
    }
    let m = parse(&text).unwrap();
    for i in 0..16 {
        assert_eq!(at(&m, &format!("i{i}")), format!("%IX{}.{}", i / 8, i % 8));
        assert_eq!(at(&m, &format!("o{i}")), format!("%QX{}.{}", i / 8, i % 8));
    }
    // The fixtures the router, the program slot and the docs use.
    for (text, name, addr) in [
        (
            "device rio modbus-tcp 192.168.3.60\nin estop bool rio di:0\n",
            "estop",
            "%IX0.0",
        ),
        (
            "device rio modbus-tcp 10.0.0.9\nin x bool rio di:1\n",
            "x",
            "%IX0.1",
        ),
        (
            "device rio modbus-tcp 192.168.3.60\nin estop bool rio di:0\nout motor bool rio coil:0\n",
            "motor",
            "%QX0.0",
        ),
    ] {
        assert_eq!(at(&parse(text).unwrap(), name), addr, "{text}");
    }
}

#[test]
fn three_devices_of_two_kinds_take_their_windows_and_slots() {
    let m = parse(THREE).unwrap();
    assert_eq!(m.devices.len(), 3);
    assert_eq!(m.devices[1].port, ENIP_PORT);
    let Link::Enip(l) = &m.devices[1].link else {
        panic!("enip")
    };
    assert_eq!(
        (l.configuration, l.o_to_t, l.o_to_t_len, l.t_to_o, l.t_to_o_len),
        (1, 112, 36, 100, 16)
    );
    assert_eq!(
        (l.rpi_ms, l.multiplier, l.o_to_t_run_idle, l.input_only),
        (20, 1, true, false)
    );
    // Bools: device k's window starts at byte 4k.
    assert_eq!(at(&m, "estop"), "%IX0.0");
    assert_eq!(at(&m, "motor"), "%QX0.0");
    assert_eq!(at(&m, "ready"), "%IX4.1");
    assert_eq!(at(&m, "run"), "%QX4.0");
    // Numerics: in map order from byte 64, aligned to their size.
    assert_eq!(at(&m, "pos"), "%ID16", "byte 64");
    assert_eq!(at(&m, "level"), "%ID17", "byte 68");
    assert_eq!(at(&m, "accel"), "%QD16");
    // The listing is the map's, inputs then outputs.
    let l = m.layout();
    assert_eq!(l.len(), 7);
    assert_eq!(l[0].2, "%IX0.0");
}

#[test]
fn numeric_slots_follow_their_sizes_in_the_order_written() {
    let text = "device a modbus-tcp 10.0.0.1\n\
        in b bool a di:0\n\
        in w1 int a hr:0:i16\n\
        in d1 int a hr:1:u32\n\
        in w2 int a hr:3\n\
        in r1 real a ir:0:f32sw\n";
    let m = parse(text).unwrap();
    assert_eq!(at(&m, "w1"), "%IW32", "byte 64");
    assert_eq!(at(&m, "d1"), "%ID17", "byte 68: aligned up past byte 66");
    assert_eq!(at(&m, "w2"), "%IW36", "byte 72");
    assert_eq!(at(&m, "r1"), "%ID19", "byte 76");
    let p = m.points.iter().find(|p| p.name == "r1").unwrap();
    assert_eq!((p.ty, p.fmt), (Ty::Real, Some(Fmt::F32)));
    assert_eq!(
        p.addr,
        Addr::Modbus {
            table: Table::Input,
            addr: 0,
            swapped: true
        }
    );
}

#[test]
fn an_enip_device_takes_the_profile_or_an_explicit_connection() {
    let m = parse(
        "device rio enip-io 192.168.3.60 profile=moxa-e1213 rpi_ms=200\nin a bool rio di:11\nout b bool rio do:7\nin c int rio in:50:u8\n",
    )
    .unwrap();
    let Link::Enip(l) = &m.devices[0].link else {
        panic!()
    };
    assert_eq!(
        (l.configuration, l.o_to_t, l.t_to_o, l.o_to_t_len, l.t_to_o_len),
        (100, 102, 101, 1, 51)
    );
    assert_eq!((l.rpi_ms, l.channels), (200, Some((12, 8))));
    // di:11 is bit 11 of the assembly: byte 1 bit 3.
    assert_eq!(at(&m, "a"), "%IX1.3");
    assert_eq!(at(&m, "c"), "%IW32");
    let io = parse(
        "device rio enip-io 192.168.3.60 profile=moxa-e1213 connection=input-only\nin a bool rio di:0\n",
    )
    .unwrap();
    let Link::Enip(l) = &io.devices[0].link else {
        panic!()
    };
    assert_eq!(
        (l.input_only, l.o_to_t, l.o_to_t_len, l.o_to_t_run_idle),
        (true, 111, 0, false)
    );
    let c = parse(
        "device m enip-io 10.0.0.5 config=109 o2t=112:36 t2o=100:16 run_idle=off multiplier=3 config_data=00ff00\n",
    )
    .unwrap();
    let Link::Enip(l) = &c.devices[0].link else {
        panic!()
    };
    assert_eq!(
        (l.o_to_t_run_idle, l.multiplier, l.config_data.clone()),
        (false, 3, vec![0, 255, 0])
    );
}

#[test]
fn sixteen_devices_are_taken_and_a_seventeenth_is_refused_at_its_line() {
    let sixteen = device_lines(MAX_DEVICES);
    let m = parse(&sixteen).unwrap();
    assert_eq!(m.devices.len(), MAX_DEVICES);
    // Each device's last bool window byte is inside the area.
    let mut text = sixteen.clone();
    for k in 0..MAX_DEVICES {
        text += &format!("in i{k} bool d{k} di:31\nout o{k} bool d{k} coil:31\n");
    }
    let m = parse(&text).unwrap();
    assert_eq!(at(&m, "i15"), "%IX63.7");
    assert_eq!(at(&m, "o0"), "%QX3.7");
    let e = refused(&device_lines(MAX_DEVICES + 1));
    assert_eq!(e.line, MAX_DEVICES + 1);
    assert!(e.why.contains("at most 16"), "{e}");
}

#[test]
fn points_are_counted_per_device_and_in_all() {
    let one = |n: usize| {
        let mut t = String::from("device a modbus-tcp 10.0.0.1\n");
        for i in 0..n {
            t += &format!("in p{i} int a hr:{i}\n");
        }
        t
    };
    assert!(parse(&one(MAX_POINTS_PER_DEVICE)).is_ok());
    let e = refused(&one(MAX_POINTS_PER_DEVICE + 1));
    assert_eq!(e.line, MAX_POINTS_PER_DEVICE + 2);
    assert!(e.why.contains("at most 32"), "{e}");

    // 96 points over three devices (bools, so the numeric area is not the limit).
    let mut t = device_lines(4);
    for k in 0..3 {
        for i in 0..MAX_POINTS_PER_DEVICE {
            t += &format!("in b{k}_{i} bool d{k} di:{i}\n");
        }
    }
    assert!(parse(&t).is_ok());
    let at_limit = t.lines().count();
    t += "in extra bool d3 di:0\n";
    let e = refused(&t);
    assert_eq!(e.line, at_limit + 1);
    assert!(e.why.contains("at most 96"), "{e}");
}

#[test]
fn the_numeric_area_runs_out_with_the_line_that_does_it() {
    // 32 double words fill 128 bytes; the 33rd, on a second device, does not fit.
    let mut t = String::from("device a modbus-tcp 10.0.0.1\ndevice b modbus-tcp 10.0.0.2\n");
    for i in 0..32 {
        t += &format!("out p{i} int a hr:{}:u32\n", i * 2);
    }
    assert!(parse(&t).is_ok());
    let lines = t.lines().count();
    // An input is another area: it still fits.
    assert!(parse(&format!("{t}in q int b hr:0:u32\n")).is_ok());
    let e = refused(&format!("{t}out q int b hr:0:u32\n"));
    assert_eq!(e.line, lines + 1);
    assert!(e.why.contains("used up"), "{e}");
}

#[test]
fn the_text_limit_is_exact() {
    let base = "device rio modbus-tcp 192.168.3.60\n";
    let pad = |n: usize| format!("{base}{}", "#".repeat(n - base.len()));
    assert!(parse(&pad(MAX_TEXT)).is_ok());
    let e = refused(&pad(MAX_TEXT + 1));
    assert_eq!(e.line, 0);
    assert!(e.why.contains("4096"), "{e}");
    // The slot holds the largest map and a program of what is left.
    const { assert!(MAX_TEXT + 52 < crate::program::SLOT_LEN) };
}

#[test]
fn a_map_at_its_point_limit_fits_a_slot_and_long_names_meet_the_text_limit_first() {
    // 16 devices, 96 points, ordinary names: well inside the text limit.
    let mut t = String::new();
    for k in 0..MAX_DEVICES {
        t += &format!("device dev{k:02} modbus-tcp 192.168.100.{} unit=255\n", k + 1);
    }
    for i in 0..MAX_POINTS {
        t += &format!(
            "in point{i:03} bool dev{:02} di:{}\n",
            i % MAX_DEVICES,
            i / MAX_DEVICES
        );
    }
    assert!(parse(&t).is_ok());
    assert!(
        t.len() + 52 + 1024 < crate::program::SLOT_LEN,
        "{} bytes",
        t.len()
    );
    // The same with 32-character names and a comment each is over 4096.
    let mut long = String::new();
    for k in 0..MAX_DEVICES {
        long += &format!(
            "device dev_{k:02}_mmmmmmmmmmmmmmmmmmmmmm modbus-tcp 192.168.100.{} unit=255\n",
            k + 1
        );
    }
    for i in 0..MAX_POINTS {
        long += &format!(
            "in point_{i:03}_mmmmmmmmmmmmmmmmmmmm bool dev_{:02}_mmmmmmmmmmmmmmmmmmmmmm di:{} # wired\n",
            i % MAX_DEVICES,
            i / MAX_DEVICES
        );
    }
    let e = refused(&long);
    assert_eq!(e.line, 0);
    assert!(e.why.contains("bytes"), "{e}");
}

#[test]
fn what_a_node_cannot_run_is_refused_with_its_line() {
    let why = |text: &str| refused(text).why;
    assert_eq!(refused("in a bool rio di:0\n").line, 1);
    assert!(why("").contains("no device"));
    assert!(why("# only a comment\n\n").contains("no device"));
    assert!(why("device rio profinet 192.168.3.60\n").contains("modbus-tcp and enip-io"));
    assert!(why("device rio iolink port=1 vendor=1\n").contains("modbus-tcp and enip-io"));
    assert!(why("device rio modbus-tcp box\n").contains("IPv4"));
    assert!(why("device rio modbus-tcp 192.168.3.60:0\n").contains("IPv4"));
    assert!(why("device rio modbus-tcp 1.2.3\n").contains("IPv4"));
    assert!(why("device rio modbus-tcp 1.2.3.4.5\n").contains("IPv4"));
    assert!(why("device rio modbus-tcp 300.2.3.4\n").contains("IPv4"));
    assert!(why("device rio modbus-tcp mac=00-90-e8-7e-af-be\n").contains("IPv4"));
    assert!(why("device rio modbus-tcp 192.168.3.60 unit=0\n").contains("unit=0"));
    assert!(why("device rio modbus-tcp 192.168.3.60 unit=x\n").contains("unit=x"));
    assert!(why("device rio modbus-tcp 192.168.3.60 unit=256\n").contains("unit=256"));
    assert!(why("device rio modbus-tcp 192.168.3.60 loud\n").contains("KEY=VALUE"));
    assert!(why("device rio modbus-tcp 192.168.3.60 unit=1 unit=2\n").contains("given twice"));
    assert!(why("device 9rio modbus-tcp 192.168.3.60\n").contains("letters"));
    assert!(why("device box modbus-tcp 192.168.3.60\n").contains("box's own"));
    assert!(why("device screen modbus-tcp 192.168.3.60\n").contains("box's own"));
    assert!(why("device rio\n").contains("want device"));
    assert!(why("device\n").contains("want device"));
    assert!(why("screen x\n").contains("device, in, out and share"));
    assert!(
        why("\u{0}device\n").contains("device, in, out and share")
            || why("\u{0}device\n").contains("in and out")
    );
}

#[test]
fn duplicate_names_and_unknown_devices_are_refused() {
    let d = "device rio modbus-tcp 192.168.3.60\ndevice two modbus-tcp 192.168.3.61\n";
    let e = refused("device a modbus-tcp 1.1.1.1\ndevice A modbus-tcp 1.1.1.2\n");
    assert_eq!((e.line, e.why.contains("named twice")), (2, true));
    let e = refused(&format!("{d}in x bool rio di:0\nout X bool two coil:0\n"));
    assert_eq!((e.line, e.why.contains("named twice")), (4, true), "{e}");
    let e = refused(&format!("{d}in x bool other di:0\n"));
    assert_eq!(e.line, 3);
    assert!(e.why.contains("not declared above"), "{e}");
    // A point before the device line it names.
    let e = refused("in x bool rio di:0\ndevice rio modbus-tcp 192.168.3.60\n");
    assert_eq!(e.line, 1);
    assert!(e.why.contains("comes before"), "{e}");
    // The box's own channels are not a node's.
    for dev in ["box", "module", "screen"] {
        let e = refused(&format!("{d}in x bool {dev} do:0\n"));
        assert!(e.why.contains("box's own"), "{e}");
    }
}

#[test]
fn a_modbus_point_is_checked_against_its_table_direction_and_type() {
    let d = "device rio modbus-tcp 192.168.3.60\n";
    let why = |p: &str| refused(&format!("{d}{p}\n")).why;
    assert!(why("in x bool rio coil:0").contains("reads di, ir and hr"));
    assert!(why("out x bool rio di:0").contains("writes coil and hr"));
    assert!(why("out x int rio ir:0").contains("writes coil and hr"));
    assert!(why("in x bool rio di:32").contains("di:0 to di:31"));
    assert!(why("out x bool rio coil:32").contains("coil:0 to coil:31"));
    assert!(why("in x bool rio di:q").contains("number"));
    assert!(why("in x bool rio di:-1").contains("number"));
    assert!(why("in x bool rio di:70000").contains("number"));
    assert!(why("in x bool rio di:0:u16").contains("no format"));
    assert!(why("in x int rio hr:5:i64").contains("a format is one of"));
    assert!(why("in x int rio hr:5:u8").contains("a format is one of"));
    assert!(why("in x int rio hr:5:i16:x").contains("too many"));
    assert!(why("in x int rio hr:65535:u32").contains("last register"));
    assert!(why("in x int rio hr").contains("number"));
    assert!(why("in x int rio nope:1").contains("a modbus-tcp point is"));
    assert!(why("in x int rio hr:5:f32").contains("it is real, and the signal is int"));
    assert!(why("in x real rio hr:5").contains("it is int, and the signal is real"));
    assert!(why("in x bool rio hr:5").contains("it is int, and the signal is bool"));
    assert!(why("in x int rio di:5").contains("it is bool"));
    assert!(why("in x float rio hr:5").contains("bool, int or real"));
    assert!(why("in 9x bool rio di:0").contains("letters"));
    assert!(why("in x bool rio").contains("want in|out"));
    // What the box takes at its edges is taken: the last register for 16 bits.
    assert!(
        parse(&format!(
            "{d}in x int rio hr:65535:i16\nin y bool rio di:31\nout z bool rio coil:31\n"
        ))
        .is_ok()
    );
}

#[test]
fn overlapping_addresses_are_refused() {
    let d = "device rio modbus-tcp 192.168.3.60\ndevice e enip-io 10.0.0.5 config=1 o2t=2:8 t2o=3:8\n";
    let why = |p: &str| refused(&format!("{d}{p}\n")).why;
    assert!(why("in a bool rio di:0\nin b bool rio di:0").contains("already named"));
    assert!(why("in a int rio hr:5:u32\nin b int rio hr:6").contains("overlaps a"));
    assert!(why("in a int rio hr:6\nin b int rio hr:5:u32").contains("overlaps a"));
    assert!(why("out a int rio hr:5:u32\nout b int rio hr:6:i16").contains("overlaps a"));
    assert!(why("out a bool rio coil:1\nout b bool rio coil:1").contains("already named"));
    assert!(why("in a int e in:0:u16\nin b int e in:1:u8").contains("overlaps a"));
    assert!(why("in a bool e in:0.1\nin b int e in:0:u8").contains("overlaps a"));
    assert!(why("in a bool e di:1\nin b bool e in:0.1").contains("already named"));
    assert!(why("out a bool e do:9\nout b bool e out:1.1").contains("already named"));
    // Neighbours, other directions, other tables and other devices are fine.
    assert!(
        parse(&format!(
            "{d}in a int rio hr:5:u32\nin b int rio hr:7\nout c int rio hr:5:u32\nin e int rio ir:5:u32\nin f bool rio di:5\nout g bool rio coil:5\nin h bool e di:0\nin i bool e di:1\nin j int e in:2:u16\nout k bool e do:0\nout l int e out:1:u8\n"
        ))
        .is_ok()
    );
}

#[test]
fn an_enip_device_is_refused_what_a_node_cannot_run() {
    let why = |d: &str| refused(&format!("{d}\n")).why;
    assert!(why("device e enip-io 10.0.0.5 eds=/etc/e.eds").contains("no files"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 key=exact").contains("electronic key"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 io_port=2223").contains("UDP 2222"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 sweep=1.2.3.255:44818").contains("identity"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 loud=1").contains("not an enip-io param"));
    assert!(why("device e enip-io 10.0.0.5").contains("exactly one of"));
    assert!(
        why("device e enip-io 10.0.0.5 profile=moxa-e1213 config=1 o2t=1:1 t2o=2:2")
            .contains("exactly one of")
    );
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:1").contains("explicit needs"));
    assert!(why("device e enip-io 10.0.0.5 profile=other").contains("known profiles"));
    assert!(
        why("device e enip-io 10.0.0.5 profile=moxa-e1213 connection=listen-only")
            .contains("exclusive-owner")
    );
    assert!(why("device e enip-io 10.0.0.5:2222 profile=moxa-e1213").contains("44818"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:129 t2o=2:2").contains("at most 128"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:129").contains("at most 128"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:x t2o=2:2").contains("POINT:BYTES"));
    assert!(
        why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 connection=input-only").contains("heartbeat")
    );
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 run_idle=maybe").contains("on or off"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 run_idle=off").contains("only with explicit"));
    assert!(
        why("device e enip-io 10.0.0.5 profile=moxa-e1213 config_data=00").contains("only with explicit")
    );
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 config_data=0").contains("hex digits"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 config_data=zz").contains("hex digits"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 rpi_ms=50").contains("100 to 3000"));
    assert!(why("device e enip-io 10.0.0.5 profile=moxa-e1213 rpi_ms=4000").contains("100 to 3000"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 rpi_ms=5").contains("10 to"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 rpi_ms=x").contains("rpi_ms"));
    assert!(why("device e enip-io 10.0.0.5 config=1 o2t=1:2 t2o=2:2 multiplier=8").contains("0 to 7"));
    let two = refused(
        "device a enip-io 10.0.0.5 profile=moxa-e1213\ndevice b enip-io 10.0.0.5 profile=moxa-e1213\n",
    );
    assert_eq!(two.line, 2);
    assert!(two.why.contains("another enip-io device"), "{two}");
    // The same address over Modbus and EtherNet/IP is two connections, and is taken.
    assert!(parse("device a enip-io 10.0.0.5 profile=moxa-e1213\ndevice b modbus-tcp 10.0.0.5\n").is_ok());
}

#[test]
fn an_enip_point_is_checked_against_its_assembly() {
    let d = "device e enip-io 10.0.0.5 config=1 o2t=2:8 t2o=3:16\n";
    let why = |p: &str| refused(&format!("{d}{p}\n")).why;
    assert!(why("in x bool e do:0").contains("O->T"));
    assert!(why("out x bool e di:0").contains("T->O"));
    assert!(why("in x bool e in:0.8").contains("BIT"));
    assert!(why("in x bool e in:0").contains("an enip-io point is"));
    assert!(why("in x int e in:0:i64").contains("a format is one of"));
    assert!(why("in x int e in:15:u16").contains("T->O assembly is 16 bytes and this ends at byte 17"));
    assert!(why("out x int e out:5:u32").contains("O->T assembly is 8 bytes"));
    assert!(why("in x int e in:0:f32").contains("it is real"));
    assert!(why("in x bool e in:4.0").contains("bits 0 to 31"));
    assert!(why("in x bool e di:32").contains("bits 0 to 31"));
    assert!(why("in x bool e in:2.0\nin y bool e in:4.0").contains("bits 0 to 31"));
    assert!(why("in x int e in:99999:u8").contains("OFFSET"));
    assert!(why("in x bool e xx:1").contains("an enip-io point is"));
    // A byte past the bool window is read as a number instead.
    assert!(parse(&format!("{d}in x int e in:15:u8\nin y bool e in:3.7\n")).is_ok());
    // Input-only has no outputs, and the profile's channels are its own.
    let io = "device e enip-io 10.0.0.5 profile=moxa-e1213 connection=input-only\n";
    assert!(
        refused(&format!("{io}out x bool e do:0\n"))
            .why
            .contains("no outputs")
    );
    let p = "device e enip-io 10.0.0.5 profile=moxa-e1213\n";
    assert!(
        refused(&format!("{p}in x bool e di:12\n"))
            .why
            .contains("di:0 to di:11")
    );
    assert!(
        refused(&format!("{p}out x bool e do:8\n"))
            .why
            .contains("do:0 to do:7")
    );
    assert!(parse(&format!("{p}in x bool e di:11\nout y bool e do:7\n")).is_ok());
}

#[test]
fn garbage_and_truncated_lines_are_refused_not_panicked_on() {
    for text in [
        "device",
        "device rio modbus-tcp",
        "in",
        "out x",
        "device rio modbus-tcp 1.1.1.1\nin x bool rio",
        "device rio modbus-tcp 1.1.1.1\nin x bool rio di:",
        "device rio modbus-tcp 1.1.1.1\nin x bool rio :",
        "device rio modbus-tcp 1.1.1.1\nin x int rio hr:1:",
        "device rio enip-io 1.1.1.1 config=1 o2t=: t2o=:",
        "device rio enip-io 1.1.1.1 config= o2t=1:1 t2o=1:1",
        "device rio enip-io 1.1.1.1 =x",
        "device e enip-io 1.1.1.1 profile=moxa-e1213\nin x bool e in:.",
        "device e enip-io 1.1.1.1 profile=moxa-e1213\nin x int e in::u8",
        "\u{feff}device rio modbus-tcp 1.1.1.1",
        "device rio modbus-tcp 1.1.1.1\r\nin x bool rio di:0\r\n",
        "device rio modbus-tcp 1.1.1.1 \u{0}\n",
        "device rio modbus-tcp 1.1.1.1\n\u{202e}in x bool rio di:0\n",
    ] {
        // Either answer is fine; what is not is a panic, and a refusal
        // without a line is only for a map as a whole.
        if let Err(e) = parse(text) {
            assert!(e.line > 0 || e.why.contains("no device"), "{text:?}: {e}");
        }
    }
    // CRLF text is a map like any other.
    assert!(parse("device rio modbus-tcp 1.1.1.1\r\nin x bool rio di:0\r\n").is_ok());
}

#[test]
fn a_refusal_reads_with_its_line() {
    assert_eq!(
        refused("").to_string(),
        "no device line: a node drives at least one device (or holds sim points)"
    );
    assert_eq!(
        refused("device rio profinet 1.1.1.1\n")
            .to_string()
            .split(':')
            .next(),
        Some("line 1")
    );
}

/// The lines a generated map is made of: a point of one of four devices
/// (two Modbus, two EtherNet/IP) in a form that is sometimes valid.
fn point_line(i: usize, dev: usize, form: usize, n: u16) -> String {
    let n = n % 40;
    let (d, body) = match (dev, form % 6) {
        (0 | 2, 0) => (dev, format!("in bool di:{n}")),
        (0 | 2, 1) => (dev, format!("out bool coil:{n}")),
        (0 | 2, 2) => (dev, format!("in int hr:{n}:i16")),
        (0 | 2, 3) => (dev, format!("in real ir:{n}:f32sw")),
        (0 | 2, 4) => (dev, format!("out int hr:{n}:u32")),
        (0 | 2, _) => (dev, format!("out real hr:{n}:f32")),
        (_, 0) => (dev, format!("in bool di:{n}")),
        (_, 1) => (dev, format!("out bool do:{}", n % 8)),
        (_, 2) => (dev, format!("in int in:{}:i16", n % 14)),
        (_, 3) => (dev, format!("in real in:{}:f32", n % 12)),
        (_, 4) => (dev, "out int out:0:u8".to_string()),
        (_, _) => (dev, format!("out bool out:0.{}", n % 8)),
    };
    let (dir, rest) = body.split_once(' ').unwrap();
    let (ty, addr) = rest.split_once(' ').unwrap();
    format!("{dir} p{i} {ty} d{d} {addr}\n")
}

const GEN_DEVICES: &str = "device d0 modbus-tcp 10.0.0.1\n\
    device d1 enip-io 10.0.0.2 config=1 o2t=2:8 t2o=3:16\n\
    device d2 modbus-tcp 10.0.0.3\n\
    device d3 enip-io 10.0.0.4 profile=moxa-e1213\n";

fn occupied(m: &IoMap, dir: Dir) -> Vec<(u16, u8)> {
    let mut used = Vec::new();
    for p in m.points.iter().filter(|p| p.dir == dir) {
        match p.at.bit {
            Some(b) => used.push((p.at.byte, 1 << b)),
            None => {
                for k in 0..u16::from(p.at.bytes) {
                    used.push((p.at.byte + k, 0xFF));
                }
            }
        }
    }
    used
}

proptest! {
    /// Whatever map a node accepts: no two points of a direction overlap
    /// in the image, every point lies in its area, numerics are aligned to
    /// their size, and the layout is a function of the text alone (a
    /// reparse, and the same lines with comments and blanks between,
    /// give the same places).
    #[test]
    fn an_accepted_maps_points_never_overlap_in_the_image(
        lines in prop::collection::vec((0usize..4, 0usize..6, 0u16..40), 0..40)
    ) {
        let mut text = String::from(GEN_DEVICES);
        for (i, (dev, form, n)) in lines.iter().enumerate() {
            text += &point_line(i, *dev, *form, *n);
        }
        let Ok(m) = parse(&text) else { return Ok(()); };
        for dir in [Dir::In, Dir::Out] {
            let used = occupied(&m, dir);
            for (a, (byte_a, mask_a)) in used.iter().enumerate() {
                prop_assert!(usize::from(*byte_a) < IMAGE_BYTES);
                for (byte_b, mask_b) in used.iter().skip(a + 1) {
                    prop_assert!(byte_a != byte_b || mask_a & mask_b == 0, "overlap in {text}");
                }
            }
        }
        for p in &m.points {
            if p.at.bit.is_some() {
                prop_assert!(usize::from(p.at.byte) < BOOL_BYTES);
            } else {
                prop_assert!(usize::from(p.at.byte) >= NUM_BASE);
                prop_assert_eq!(usize::from(p.at.byte) % usize::from(p.at.bytes), 0);
            }
        }
        prop_assert_eq!(parse(&text).unwrap().points, m.points.clone());
        let mut spaced = String::new();
        for l in text.lines() {
            spaced += &format!("  # a note\n\n{}   # trailing\n", l.replace(' ', "  "));
        }
        let again = parse(&spaced).unwrap();
        prop_assert_eq!(&again.points, &m.points);
        prop_assert_eq!(&again.devices, &m.devices);
    }

    /// Text a program of any shape throws at the parser comes back as a
    /// refusal or a map, never a panic.
    #[test]
    fn arbitrary_text_never_panics(text in "\\PC{0,300}") {
        let _ = parse(&text);
    }

    #[test]
    fn arbitrary_lines_after_a_device_never_panic(
        words in prop::collection::vec("[a-z0-9:.=_-]{0,12}", 0..8)
    ) {
        let line = words.join(" ");
        for head in ["in", "out", "device"] {
            let _ = parse(&format!("device rio modbus-tcp 10.0.0.1\ndevice e enip-io 10.0.0.2 profile=moxa-e1213\n{head} {line}\n"));
        }
    }
}

#[test]
fn the_generator_makes_maps_a_node_takes() {
    // Not vacuous: a fixed draw of the generator parses.
    let mut text = String::from(GEN_DEVICES);
    for (i, (dev, form, n)) in [(0, 0, 3), (0, 1, 3), (1, 2, 4), (2, 5, 7), (3, 4, 2), (3, 0, 9)]
        .iter()
        .enumerate()
    {
        text += &point_line(i, *dev, *form, *n);
    }
    let m = parse(&text).unwrap();
    assert_eq!(m.points.len(), 6);
}

/// The one-block boards (the ESP32 nodes) run a map through
/// `one_block`: the factory map and any one-device di/coil map pass with
/// their names in image order, and everything a node-p4 map adds is
/// refused with a reason.
#[test]
fn a_one_block_board_takes_one_modbus_block_of_bits_and_nothing_else() {
    let f = IoMap::factory([192, 168, 3, 60], "rio").one_block().unwrap();
    assert_eq!(
        (f.device.as_str(), f.host, f.port, f.unit),
        ("rio", [192, 168, 3, 60], 502, 1)
    );
    assert_eq!((f.input_count(), f.coil_count()), (8, 4));
    assert_eq!(f.inputs[7].as_deref(), Some("di7"));
    assert_eq!(f.outputs[0].as_deref(), Some("do0"));

    // A hole in the numbering stays a hole; the count runs to the highest.
    let holes =
        parse("device b modbus-tcp 10.0.0.9:1502 unit=7\nin jam bool b di:3\nout horn bool b coil:15\n")
            .unwrap()
            .one_block()
            .unwrap();
    assert_eq!((holes.port, holes.unit), (1502, 7));
    assert_eq!(holes.inputs, vec![None, None, None, Some("jam".to_string())]);
    assert_eq!(holes.coil_count(), 16);
    assert_eq!(holes.outputs[15].as_deref(), Some("horn"));

    let no_points = parse("device b modbus-tcp 10.0.0.9\n")
        .unwrap()
        .one_block()
        .unwrap();
    assert_eq!((no_points.input_count(), no_points.coil_count()), (0, 0));

    let refused = |text: &str| parse(text).unwrap().one_block().unwrap_err().to_string();
    assert!(refused(THREE).contains("3 devices"), "{}", refused(THREE));
    assert!(
        refused("device e enip-io 10.0.0.2 profile=moxa-e1213\n").contains("not a modbus-tcp device"),
        "an EtherNet/IP device"
    );
    for (text, point) in [
        ("device b modbus-tcp 10.0.0.9\nin level int b hr:5\n", "level"),
        ("device b modbus-tcp 10.0.0.9\nin far bool b di:16\n", "far"),
        ("device b modbus-tcp 10.0.0.9\nout sp int b hr:1\n", "sp"),
        ("device b modbus-tcp 10.0.0.9\nin raw int b ir:0\n", "raw"),
    ] {
        let why = refused(text);
        assert!(why.contains(&format!("point {point}")), "{text:?}: {why}");
        assert!(why.contains("one Modbus TCP block"), "{why}");
    }
    let long = format!("device b modbus-tcp 10.0.0.9\n{}", "# pad\n".repeat(400));
    assert!(long.len() > ONE_BLOCK_TEXT && long.len() <= MAX_TEXT);
    assert!(refused(&long).contains("2048"), "{}", refused(&long));
}

// Sim points: placeholders on no device (the module header's "Sim points").

fn addresses(m: &IoMap) -> Vec<(String, String)> {
    m.layout()
        .into_iter()
        .map(|(n, d, a)| (n.to_string(), format!("{} {a}", d.word())))
        .collect()
}

/// A map's device points with their addresses, in map order, no sims.
fn device_addresses(m: &IoMap) -> Vec<(String, String)> {
    m.points
        .iter()
        .filter(|p| !p.is_sim())
        .map(|p| (p.name.clone(), p.at.st(p.dir)))
        .collect()
}

#[test]
fn a_sim_line_parses_to_a_point_on_no_device() {
    let m = parse("in setpoint real sim\nout lamp bool sim unit=3\nin n int sim\n").unwrap();
    assert!(
        m.devices.is_empty(),
        "a map with sim points and no device is taken"
    );
    assert_eq!(m.points.len(), 3);
    for p in &m.points {
        assert!(p.is_sim());
        assert_eq!((p.device, p.addr), (SIM_DEVICE, Addr::Sim));
    }
    let by = |n: &str| m.points.iter().find(|p| p.name == n).unwrap();
    assert_eq!(
        (by("setpoint").ty, by("setpoint").fmt),
        (Ty::Real, Some(Fmt::F32))
    );
    assert_eq!((by("n").ty, by("n").fmt), (Ty::Int, Some(Fmt::I32)));
    assert_eq!(
        (by("lamp").ty, by("lamp").fmt, by("lamp").dir),
        (Ty::Bool, None, Dir::Out)
    );
    // Placed from the top of each area down, in the order written.
    assert_eq!(at(&m, "setpoint"), "%ID47");
    assert_eq!(at(&m, "n"), "%ID46");
    assert_eq!(at(&m, "lamp"), "%QX60.0");
    // Options other than unit are read and ignored, as for any point.
    assert!(parse("in a bool sim poll_ms=5 safe=1\n").is_ok());
}

#[test]
fn sim_points_never_move_a_device_point() {
    let base = "device rio modbus-tcp 192.168.3.60\ndevice drive enip-io 192.168.33.1 config=1 o2t=112:36 t2o=100:16\n";
    let dev = "in estop bool rio di:3\nin pos int drive in:4:i32\nout motor bool rio coil:1\nin level real rio hr:5:f32\nout accel int drive out:4:u32\nin ready bool drive in:0.1\n";
    let plain = parse(&format!("{base}{dev}")).unwrap();
    let want = device_addresses(&plain);
    assert_eq!(
        want,
        [
            ("estop", "%IX0.3"),
            ("pos", "%ID16"),
            ("motor", "%QX0.1"),
            ("level", "%ID17"),
            ("accel", "%QD16"),
            ("ready", "%IX4.1"),
        ]
        .map(|(n, a)| (n.to_string(), a.to_string()))
    );
    let sims = [
        "in s_b bool sim\n",
        "in s_i int sim\n",
        "in s_r real sim\n",
        "out o_b bool sim\n",
        "out o_i int sim\n",
        "out o_r real sim\n",
    ];
    let all: String = sims.concat();
    let device_lines: Vec<&str> = dev.lines().collect();
    // Before everything, after everything, and between every pair of device lines.
    let mut texts = vec![
        format!("{all}{base}{dev}"),
        format!("{base}{all}{dev}"),
        format!("{base}{dev}{all}"),
    ];
    for cut in 0..=device_lines.len() {
        let (a, b) = device_lines.split_at(cut);
        texts.push(format!("{base}{}\n{all}{}\n", a.join("\n"), b.join("\n")));
    }
    for text in &texts {
        let m = parse(text).unwrap();
        assert_eq!(device_addresses(&m), want, "{text}");
        assert_eq!(m.devices.len(), 2);
    }
    // And each sim is somewhere no device point is.
    let m = parse(&format!("{base}{dev}{all}")).unwrap();
    let mut seen = Vec::new();
    for p in &m.points {
        let key = (p.dir.index(), p.at.byte, p.at.bit, p.at.bytes);
        if let Some(bit) = p.at.bit {
            assert!(
                !seen.contains(&(key.0, key.1, Some(bit))),
                "{} shares a bit",
                p.name
            );
            seen.push((key.0, key.1, Some(bit)));
        } else {
            for b in key.1..key.1 + u16::from(key.3) {
                assert!(!seen.contains(&(key.0, b, None)), "{} shares a byte", p.name);
                seen.push((key.0, b, None));
            }
        }
    }
    assert_eq!(at(&m, "s_b"), "%IX60.0");
    assert_eq!(at(&m, "o_b"), "%QX60.0");
    assert_eq!(at(&m, "s_i"), "%ID47");
    assert_eq!(at(&m, "s_r"), "%ID46");
    assert_eq!(at(&m, "o_i"), "%QD47");
    assert_eq!(at(&m, "o_r"), "%QD46");
}

#[test]
fn sim_points_fill_the_windows_no_device_has_and_stop_where_a_device_is() {
    // 33 sim bools: the second window of them is the next one down.
    let mut text = String::new();
    for i in 0..33 {
        text += &format!("in b{i} bool sim\n");
    }
    let m = parse(&text).unwrap();
    assert_eq!(at(&m, "b0"), "%IX60.0");
    assert_eq!(at(&m, "b31"), "%IX63.7");
    assert_eq!(at(&m, "b32"), "%IX56.0");
    // With 15 devices one window is left: its 32 bits are taken and a 33rd is refused.
    let mut text = device_lines(15);
    for i in 0..32 {
        text += &format!("in b{i} bool sim\n");
    }
    assert_eq!(at(&parse(&text).unwrap(), "b31"), "%IX63.7");
    text += "in b32 bool sim\n";
    let e = refused(&text);
    assert!(e.why.contains("no bool window left"), "{e}");
    assert_eq!(e.line, 48);
    // The same 16 devices and one sim bool.
    let e = refused(&format!("{}in b bool sim\n", device_lines(16)));
    assert!(e.why.contains("no bool window left"), "{e}");
    // Outputs have windows of their own.
    let m = parse(&format!("{}in a bool sim\nout a_q bool sim\n", device_lines(15))).unwrap();
    assert_eq!((at(&m, "a"), at(&m, "a_q")), ("%IX60.0".into(), "%QX60.0".into()));
}

#[test]
fn sim_numbers_stop_where_the_device_numbers_end() {
    // 128 bytes of numeric area, 4 a sim: 32 of them and no device number.
    let mut text = String::new();
    for i in 0..32 {
        text += &format!("in n{i} int sim\n");
    }
    let m = parse(&text).unwrap();
    assert_eq!(at(&m, "n0"), "%ID47");
    assert_eq!(at(&m, "n31"), "%ID16");
    let e = refused(&format!("{text}in n32 int sim\n"));
    assert!(e.why.contains("used up") && e.line == 33, "{e}");
    // A device point takes its slot first, wherever its line is, and the sims make room.
    let dev = "device rio modbus-tcp 1.1.1.1\nin x int rio hr:0\nin y real rio hr:1:f32\n";
    let mut sims = String::new();
    for i in 0..30 {
        sims += &format!("in n{i} int sim\n");
    }
    let m = parse(&format!("{sims}{dev}")).unwrap();
    assert_eq!((at(&m, "x"), at(&m, "y")), ("%IW32".into(), "%ID17".into()));
    assert_eq!(at(&m, "n29"), "%ID18");
    let e = refused(&format!("{sims}in n30 int sim\nin n31 int sim\n{dev}"));
    assert!(e.why.contains("used up"), "{e}");
    // Only the direction that is full is refused.
    assert!(parse(&format!("{text}out q int sim\n")).is_ok());
}

#[test]
fn a_sim_line_that_is_not_one_is_refused_with_its_line() {
    for (text, line, word) in [
        ("device sim modbus-tcp 1.1.1.1\n", 1, "reserved"),
        ("device SIM modbus-tcp 1.1.1.1\n", 1, "reserved"),
        (
            "device rio modbus-tcp 1.1.1.1\ndevice sim enip-io 1.1.1.2 profile=moxa-e1213\n",
            2,
            "reserved",
        ),
        ("in a bool sim di:0\n", 1, "no address"),
        ("in a bool sim coil:0\n", 1, "no address"),
        ("out a int sim hr:4:i32\n", 1, "no address"),
        ("in a real sim 5\n", 1, "no address"),
        ("in a bool sim unit=1 di:0\n", 1, "no address"),
        ("in a bool sim unit=1 unit=2\n", 1, "given twice"),
        ("in a bool sim =3\n", 1, "KEY=VALUE"),
        ("in a bytes sim\n", 1, "bool, int or real"),
        ("in 1a bool sim\n", 1, "a name is"),
        ("in a bool sim\nin A int sim\n", 2, "named twice"),
        (
            "device rio modbus-tcp 1.1.1.1\nin a bool rio di:0\nin A bool sim\n",
            3,
            "named twice",
        ),
        (
            "device rio modbus-tcp 1.1.1.1\nin a bool sim\nin A bool rio di:0\n",
            3,
            "named twice",
        ),
        ("in a bool\n", 1, "want in|out"),
        ("in a bool rio\n", 1, "want in|out"),
        ("device rio modbus-tcp 1.1.1.1\nin a bool rio\n", 2, "want in|out"),
    ] {
        let e = refused(text);
        assert_eq!(e.line, line, "{text:?}: {e}");
        assert!(e.why.contains(word), "{text:?}: {e}");
    }
    // An empty map, or one of nothing, stays refused.
    assert!(refused("").why.contains("no device"));
    assert!(refused("# in a bool sim\n").why.contains("no device"));
    // 96 points in all, sim or not.
    let mut text = String::new();
    for i in 0..MAX_POINTS {
        text += &format!("in p{i} bool sim\n");
    }
    text += "in one_more bool sim\n";
    assert!(refused(&text).why.contains("at most 96"), "the 97th point");
}

#[test]
fn a_sim_point_is_listed_with_its_address_and_a_one_block_board_refuses_it() {
    let m = parse("device rio modbus-tcp 1.1.1.1\nin a bool rio di:0\nin go bool sim\nout lamp bool sim\nout q bool rio coil:0\n").unwrap();
    assert_eq!(
        addresses(&m),
        [
            ("a", "in %IX0.0"),
            ("go", "in %IX60.0"),
            ("q", "out %QX0.0"),
            ("lamp", "out %QX60.0"),
        ]
        .map(|(n, a)| (n.to_string(), a.to_string()))
    );
    assert!(m.one_block().unwrap_err().why.contains("point go"));
    assert!(parse("in go bool sim\n").unwrap().one_block().is_err());
    // Re-parsing the text gives the same map: the text is what is kept.
    assert_eq!(parse(&m.text).unwrap(), m);
}

// ---- share lines (a Skipper's own Modbus TCP tables) ----

const SHARED: &str = "\
# shared with other controllers
share speed   int  hr:0        # they read
share run_cmd bool coil:0      # they write
share at_home bool di:0
share level   real ir:4:f32
share pos     int  hr:10:i32sw
";

fn share<'a>(m: &'a IoMap, name: &str) -> &'a Share {
    m.shares.iter().find(|s| s.name == name).expect(name)
}

#[test]
fn share_lines_parse_to_tables_addresses_and_places() {
    let m = parse(SHARED).unwrap();
    assert_eq!(m.shares.len(), 5);
    assert!(
        m.devices.is_empty() && m.points.is_empty(),
        "a map of shares alone is a map"
    );
    let speed = share(&m, "speed");
    assert_eq!(
        (speed.ty, speed.table, speed.addr, speed.fmt),
        (Ty::Int, Table::Holding, 0, Some(Fmt::U16))
    );
    assert!(speed.writable_table());
    let at_home = share(&m, "at_home");
    assert_eq!(
        (at_home.ty, at_home.table, at_home.fmt),
        (Ty::Bool, Table::Discrete, None)
    );
    assert!(!at_home.writable_table());
    let level = share(&m, "level");
    assert_eq!(
        (level.table, level.fmt, level.words()),
        (Table::Input, Some(Fmt::F32), 2)
    );
    let pos = share(&m, "pos");
    assert_eq!((pos.fmt, pos.swapped, pos.words()), (Some(Fmt::I32), true, 2));
    let tokens: Vec<String> = m.shares.iter().map(Share::token).collect();
    assert_eq!(tokens, ["hr:0", "coil:0", "di:0", "ir:4:f32", "hr:10:i32sw"]);
    // The bank: bools from bit 0, numbers from byte 4 in slots of their size.
    assert_eq!(
        (share(&m, "run_cmd").at.byte, share(&m, "run_cmd").at.bit),
        (0, Some(0))
    );
    assert_eq!((at_home.at.byte, at_home.at.bit), (0, Some(1)));
    assert_eq!((speed.at.byte, speed.at.bytes), (4, 2));
    assert_eq!((level.at.byte, level.at.bytes), (8, 4), "aligned to its size");
    assert_eq!((pos.at.byte, pos.at.bytes), (12, 4));
    assert_eq!(m.text, SHARED);
}

#[test]
fn shares_sit_beside_devices_and_points_and_move_none() {
    let plain = parse(THREE).unwrap();
    let with = parse(&format!("{THREE}share speed int hr:0\nshare go bool coil:0\n")).unwrap();
    assert_eq!(with.points, plain.points, "no point moved");
    assert_eq!(with.shares.len(), 2);
    let first = parse(&format!("share speed int hr:0\n{THREE}")).unwrap();
    assert_eq!(first.points, plain.points, "before or after, the same");
    // A share of an out point's name is fine: the program's output goes both ways.
    parse("device rio modbus-tcp 10.0.0.1\nout motor bool rio coil:0\nshare motor bool di:0\n").unwrap();
}

#[test]
fn a_share_must_be_well_formed() {
    let long = format!("share {} int hr:0\n", "n".repeat(33));
    let cases: &[(&str, &str)] = &[
        ("share\n", "want share NAME"),
        ("share a int\n", "want share NAME"),
        ("share a int hr:0 extra\n", "want share NAME"),
        ("share a float hr:0\n", "type \"float\""),
        ("share 1a int hr:0\n", "a name is letters"),
        ("share a-b int hr:0\n", "a name is letters"),
        (&long, "at most 32"),
        ("share a int xx:0\n", "TABLE is di, coil, ir or hr"),
        ("share a int hr\n", "ADDR"),
        ("share a int hr:\n", "ADDR"),
        ("share a int hr:x\n", "ADDR"),
        ("share a int hr:65536\n", "ADDR"),
        ("share a int hr:-1\n", "ADDR"),
        ("share a int hr:0:u16:more\n", "too many fields"),
        ("share a int hr:0:u64\n", "a format is one of"),
        ("share a int hr:0:f32\n", "it is real"),
        ("share a real hr:0\n", "it is int"),
        ("share a real hr:0:u16\n", "it is int"),
        ("share a real hr:0:i32\n", "it is int"),
        ("share a int hr:65535:u32\n", "does not fit"),
        ("share a real ir:65535:f32sw\n", "does not fit"),
    ];
    for (text, why) in cases {
        let e = refused(text);
        assert_eq!(e.line, 1, "{text:?}: {e}");
        assert!(e.why.contains(why), "{text:?}: {e}");
    }
}

#[test]
fn a_table_and_a_type_must_agree() {
    for text in [
        "share a bool hr:0\n",
        "share a bool ir:0\n",
        "share a int di:0\n",
        "share a int coil:0\n",
        "share a real di:0:f32\n",
        "share a real coil:0\n",
    ] {
        let e = refused(&format!("# c\n{text}"));
        assert_eq!(e.line, 2, "{text:?}");
        assert!(e.why.contains("holds a"), "{text:?}: {e}");
    }
    assert!(
        refused("share a bool coil:0:u16\n")
            .why
            .contains("takes no format")
    );
    assert!(refused("share a bool di:0:f32\n").why.contains("takes no format"));
}

#[test]
fn shares_of_one_table_may_not_overlap() {
    let e = refused("share a int hr:0:u32\nshare b int hr:1\n");
    assert_eq!(e.line, 2);
    assert!(e.why.contains("overlaps a at hr:0:u32"), "{e}");
    let e = refused("share a int hr:5\nshare b int hr:4:u32\n");
    assert!(e.why.contains("overlaps a"), "{e}");
    let e = refused("share a bool coil:3\nshare b bool coil:3\n");
    assert!(e.why.contains("already shared"), "{e}");
    let e = refused("share a real ir:8:f32\n\nshare b int ir:9\n");
    assert_eq!(e.line, 3);
    // Next to each other is fine, and so is the same address in another table.
    parse(
        "share a int hr:0:u32\nshare b int hr:2\nshare c int ir:0\nshare d bool coil:0\nshare e bool di:0\n",
    )
    .unwrap();
    parse("share a int hr:0:u32\nshare b int hr:65535\n").unwrap();
}

#[test]
fn a_name_is_shared_once_and_has_one_source() {
    assert!(
        refused("share a int hr:0\nshare a int hr:1\n")
            .why
            .contains("shared twice")
    );
    let e = refused("share Speed int hr:0\nshare speed int hr:1\n");
    assert_eq!(e.line, 2);
    assert!(
        e.why.contains("shared twice"),
        "ST names are case-insensitive: {e}"
    );
    // A share and an `in` point of one name, in either order.
    let after = refused("device rio modbus-tcp 10.0.0.1\nin go bool rio di:0\nshare GO bool coil:0\n");
    assert_eq!(after.line, 3);
    assert!(after.why.contains("an in point has that name"), "{after}");
    let before = refused("share go bool coil:0\ndevice rio modbus-tcp 10.0.0.1\nin Go bool rio di:0\n");
    assert_eq!(before.line, 3);
    assert!(before.why.contains("a share of that name"), "{before}");
    // A sim in is an input too.
    assert!(
        refused("in go bool sim\nshare go bool coil:0\n")
            .why
            .contains("an in point")
    );
    assert!(
        refused("share go bool coil:0\nin go bool sim\n")
            .why
            .contains("a share of that name")
    );
}

#[test]
fn a_map_holds_at_most_32_shares() {
    let lines = |n: usize| (0..n).fold(String::new(), |t, i| t + &format!("share v{i} int hr:{i}\n"));
    let m = parse(&lines(MAX_SHARES)).unwrap();
    assert_eq!(m.shares.len(), MAX_SHARES);
    let top = m
        .shares
        .iter()
        .map(|s| usize::from(s.at.byte) + usize::from(s.at.bytes))
        .max()
        .unwrap();
    assert!(top <= SHARE_BYTES, "the bank holds them all");
    let e = refused(&lines(MAX_SHARES + 1));
    assert_eq!(e.line, MAX_SHARES + 1);
    assert!(e.why.contains("at most 32"), "{e}");
    // 32 bools take 4 bytes, 32 reals 128: neither spills.
    let reals = (0..MAX_SHARES).fold(String::new(), |t, i| {
        t + &format!("share r{i} real ir:{}:f32\n", i * 2)
    });
    assert!(
        parse(&reals)
            .unwrap()
            .shares
            .iter()
            .all(|s| usize::from(s.at.byte) + 4 <= SHARE_BYTES)
    );
    let bools = (0..MAX_SHARES).fold(String::new(), |t, i| t + &format!("share b{i} bool coil:{i}\n"));
    assert!(
        parse(&bools)
            .unwrap()
            .shares
            .iter()
            .all(|s| usize::from(s.at.byte) < SHARE_BOOL_BYTES)
    );
}

#[test]
fn a_one_block_board_cannot_run_a_share() {
    let text = format!("{BENCH}share x bool di:0\n");
    let m = parse(&text).unwrap();
    assert!(m.one_block().unwrap_err().why.contains("share"));
    assert!(parse(BENCH).unwrap().one_block().is_ok());
}

proptest! {
    /// Any share lines that parse place every share inside the bank, no
    /// two on the same bit or bytes.
    #[test]
    fn shares_that_parse_are_placed_apart(
        lines in proptest::collection::vec((any::<bool>(), 0u16..40, 0usize..4), 0..40),
    ) {
        let mut text = String::new();
        for (i, (is_bool, addr, f)) in lines.iter().enumerate() {
            if *is_bool {
                let table = if i % 2 == 0 { "coil" } else { "di" };
                text += &format!("share s{i} bool {table}:{addr}\n");
            } else {
                let fmt = ["u16", "i16", "u32", "f32"][*f];
                let ty = if fmt == "f32" { "real" } else { "int" };
                text += &format!("share s{i} {ty} hr:{addr}:{fmt}\n");
            }
        }
        if let Ok(m) = parse(&text) {
            let mut used = vec![false; SHARE_BYTES * 8];
            for s in &m.shares {
                let bits: Vec<usize> = match s.at.bit {
                    Some(b) => vec![usize::from(s.at.byte) * 8 + usize::from(b)],
                    None => (usize::from(s.at.byte) * 8..(usize::from(s.at.byte) + usize::from(s.at.bytes)) * 8).collect(),
                };
                for b in bits {
                    prop_assert!(b < used.len(), "inside the bank");
                    prop_assert!(!used[b], "{} overlaps another share's place", s.name);
                    used[b] = true;
                }
            }
        }
    }
}
