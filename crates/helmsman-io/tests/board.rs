//! The box's own channels: the address syntax, the board driver over
//! stand-in GPIO and I2C (the kernel side is Linux-only and runs on the Pi;
//! see scripts/pi-gpio-check.sh), and the in-memory backend.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use helmsman_io::board::{Board, Channel, Gpio, I2c, I2cFormat, I2cPoint};
use helmsman_io::local::Channels;
use helmsman_io::{Binding, Dir, Driver, Error, IoMap, MapError, Signal, Ty, Value, text};
use proptest::prelude::*;

#[test]
fn channels_parse_strictly() {
    assert_eq!(Channel::parse("do:0").unwrap(), Channel::Out(0));
    assert_eq!(Channel::parse("di:65535").unwrap(), Channel::In(65535));
    let i2c = |bus, addr, reg, format| {
        Channel::I2c(I2cPoint {
            bus,
            addr,
            reg,
            format,
        })
    };
    for (s, want) in [
        ("i2c:1:0x48:0", i2c(1, 0x48, 0, I2cFormat::U8)),
        ("i2c:1:72:0x00:u8", i2c(1, 0x48, 0, I2cFormat::U8)),
        ("i2c:0:0x08:255:i8", i2c(0, 0x08, 255, I2cFormat::I8)),
        ("i2c:255:0x77:0xFF:u16", i2c(255, 0x77, 0xFF, I2cFormat::U16)),
        ("i2c:1:0x4a:0x0f:i16", i2c(1, 0x4A, 0x0F, I2cFormat::I16)),
        ("i2c:3:0x40:2:u16le", i2c(3, 0x40, 2, I2cFormat::U16Le)),
        ("i2c:3:0x40:2:i16le", i2c(3, 0x40, 2, I2cFormat::I16Le)),
    ] {
        assert_eq!(Channel::parse(s).unwrap(), want, "{s}");
    }
    for bad in [
        "",
        "do",
        "do:",
        "do:x",
        "do:-1",
        "do:+1",
        "do: 1",
        "do:65536",
        "do:0:1",
        "DO:0",
        "ao:0",
        "coil:0",
        "i2c",
        "i2c:1",
        "i2c:1:0x48",
        "i2c:256:0x48:0",
        "i2c:-1:0x48:0",
        "i2c:1:0x07:0",
        "i2c:1:0x78:0",
        "i2c:1:7:0",
        "i2c:1:0x:0",
        "i2c:1:0x048:0",
        "i2c:1:0X48:0",
        "i2c:1:0x48:256",
        "i2c:1:0x48:0x100",
        "i2c:1:0x48:0:f32",
        "i2c:1:0x48:0:u16:x",
        "i2c:1:0x48:0:",
        "i2c::0x48:0",
        "i2c:1:+0x48:0",
    ] {
        assert!(
            matches!(Channel::parse(bad), Err(Error::Address(_))),
            "{bad:?} parsed"
        );
    }
}

#[test]
fn i2c_formats_decode_both_byte_orders_and_signs() {
    assert_eq!(I2cFormat::U8.decode(&[0xFF]), 255);
    assert_eq!(I2cFormat::I8.decode(&[0xFF]), -1);
    assert_eq!(I2cFormat::U16.decode(&[0x12, 0x34]), 0x1234);
    assert_eq!(I2cFormat::U16Le.decode(&[0x12, 0x34]), 0x3412);
    assert_eq!(I2cFormat::I16.decode(&[0xFF, 0x38]), -200);
    assert_eq!(I2cFormat::I16Le.decode(&[0x38, 0xFF]), -200);
    assert_eq!(I2cFormat::U16.bytes(), 2);
    assert_eq!(I2cFormat::I8.bytes(), 1);
}

#[derive(Clone, Default)]
struct FakeGpio {
    levels: Arc<Mutex<BTreeMap<u32, bool>>>,
    /// Lines the "kernel" refuses (in use elsewhere, or not on the chip).
    busy: Vec<u32>,
}

impl Gpio for FakeGpio {
    fn get(&mut self, line: u32) -> Result<bool, Error> {
        if self.busy.contains(&line) {
            return Err(Error::Point(format!("line {line}: busy")));
        }
        Ok(self.levels.lock().unwrap().get(&line).copied().unwrap_or(false))
    }

    fn set(&mut self, line: u32, on: bool) -> Result<(), Error> {
        if self.busy.contains(&line) {
            return Err(Error::Point(format!("line {line}: busy")));
        }
        self.levels.lock().unwrap().insert(line, on);
        Ok(())
    }
}

/// `(bus, addr, reg, bytes)` of each transfer.
type Reads = Arc<Mutex<Vec<(u8, u8, u8, usize)>>>;

#[derive(Default)]
struct FakeI2c {
    regs: BTreeMap<(u8, u8, u8), Vec<u8>>,
    /// `(bus, addr, reg, bytes)` of every transfer, shared with the test.
    reads: Reads,
}

impl I2c for FakeI2c {
    fn read_register(&mut self, bus: u8, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), Error> {
        self.reads.lock().unwrap().push((bus, addr, reg, buf.len()));
        let v = self
            .regs
            .get(&(bus, addr, reg))
            .ok_or_else(|| Error::Point(format!("i2c-{bus} {addr:#04x}: no answer")))?;
        buf.copy_from_slice(&v[..buf.len()]);
        Ok(())
    }
}

fn board() -> (Board<FakeGpio, FakeI2c>, FakeGpio) {
    let gpio = FakeGpio {
        busy: vec![99],
        ..FakeGpio::default()
    };
    let mut i2c = FakeI2c::default();
    i2c.regs.insert((1, 0x48, 0), vec![0x19, 0x00]);
    i2c.regs.insert((1, 0x48, 1), vec![0xF0]);
    (Board::new(gpio.clone(), i2c), gpio)
}

#[test]
fn the_board_reads_lines_and_registers_and_drives_outputs() {
    let (mut b, gpio) = board();
    gpio.levels.lock().unwrap().insert(4, true);
    assert_eq!(
        b.read(&["di:4", "di:5", "i2c:1:0x48:0:i16", "i2c:1:0x48:1:i8"])
            .unwrap(),
        [
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(0x1900),
            Value::Int(-16)
        ]
    );
    b.write("do:17", Value::Bool(true)).unwrap();
    assert_eq!(gpio.levels.lock().unwrap().get(&17), Some(&true));
    assert_eq!(
        b.read(&["do:17"]).unwrap(),
        [Value::Bool(true)],
        "an output reads back"
    );
    b.write("do:17", Value::Bool(false)).unwrap();
    assert_eq!(gpio.levels.lock().unwrap().get(&17), Some(&false));
}

#[test]
fn the_board_refuses_what_it_should_as_point_errors() {
    let (mut b, _) = board();
    assert!(matches!(
        b.write("di:3", Value::Bool(true)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(
        b.write("i2c:1:0x48:0", Value::Int(1)),
        Err(Error::ReadOnly(_))
    ));
    assert!(matches!(
        b.write("do:3", Value::Int(1)),
        Err(Error::Type {
            want: Ty::Bool,
            got: Ty::Int
        })
    ));
    // One pin, one direction.
    b.write("do:6", Value::Bool(true)).unwrap();
    let e = b.read(&["di:6"]).unwrap_err();
    assert!(matches!(e, Error::Point(_)) && !e.is_link(), "{e}");
    b.read(&["di:7"]).unwrap();
    let e = b.write("do:7", Value::Bool(true)).unwrap_err();
    assert!(matches!(e, Error::Point(_)), "{e}");
    // A busy line and a sensor that doesn't answer are that point's problem.
    for a in ["di:99", "i2c:2:0x48:0", "i2c:1:0x49:0"] {
        let e = b.read(&[a]).unwrap_err();
        assert!(!e.is_link(), "{a}: {e}");
    }
    assert!(!b.write("do:99", Value::Bool(true)).unwrap_err().is_link());
    // All or nothing: a bad address anywhere fails the read before any I/O.
    assert!(matches!(b.read(&["di:4", "di:x"]), Err(Error::Address(_))));
}

#[test]
fn the_board_reads_only_the_bytes_the_format_needs() {
    let gpio = FakeGpio::default();
    let mut i2c = FakeI2c::default();
    i2c.regs.insert((1, 0x10, 5), vec![1, 2]);
    let reads = Arc::clone(&i2c.reads);
    let mut b = Board::new(gpio, i2c);
    assert_eq!(
        b.read(&["i2c:1:0x10:5:u8", "i2c:1:0x10:5:u16le"]).unwrap(),
        [Value::Int(1), Value::Int(0x0201)]
    );
    assert_eq!(*reads.lock().unwrap(), [(1, 0x10, 5, 1), (1, 0x10, 5, 2)]);
}

#[test]
fn in_memory_channels_serve_i2c_registers() {
    let panel = Channels::new();
    let mut d = panel.clone();
    assert_eq!(
        d.read(&["i2c:1:0x48:0:i16"]).unwrap(),
        [Value::Int(0)],
        "zeros until set"
    );
    panel.set_i2c(1, 0x48, 0, &[0xFF, 0x38]);
    assert_eq!(d.read(&["i2c:1:0x48:0:i16"]).unwrap(), [Value::Int(-200)]);
    assert_eq!(d.read(&["i2c:1:0x48:0:u8"]).unwrap(), [Value::Int(255)]);
    assert_eq!(d.parse("i2c:1:0x48:0").unwrap().ty, Ty::Int);
    assert!(matches!(
        d.write("i2c:1:0x48:0", Value::Int(1)),
        Err(Error::ReadOnly(_))
    ));
}

#[test]
fn the_map_takes_i2c_inputs_on_the_box_and_refuses_them_as_outputs() {
    let file = text::parse(
        "in temp int box i2c:1:0x48:0:i16 unit=raw poll_ms=500\nin start bool box di:4\nout lamp bool box do:17\n",
    )
    .unwrap();
    assert!(file.map.validate().is_ok(), "{:?}", file.map.validate());
    let sig = |dir, ty, channel: &str| Signal {
        name: "s".into(),
        ty,
        dir,
        binding: Binding::Local {
            channel: channel.into(),
        },
        unit: None,
        poll: None,
    };
    let bad = |s: Signal| {
        IoMap {
            devices: vec![],
            signals: vec![s],
        }
        .validate()
        .unwrap_err()
    };
    assert!(matches!(
        bad(sig(Dir::Out, Ty::Int, "i2c:1:0x48:0"))[..],
        [MapError::NotWritable(_)]
    ));
    assert!(matches!(
        bad(sig(Dir::In, Ty::Bool, "i2c:1:0x48:0"))[..],
        [MapError::TypeMismatch { .. }]
    ));
    assert!(matches!(
        bad(sig(Dir::In, Ty::Int, "i2c:1:0x80:0"))[..],
        [MapError::BadPoint { .. }]
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn channel_parsing_never_panics(s in "\\PC{0,40}") {
        let _ = Channel::parse(&s);
    }

    #[test]
    fn channel_parsing_never_panics_on_near_misses(
        kind in prop::sample::select(vec!["do", "di", "i2c", "i2c:1", "i2c:1:0x48", "i2c:1:0x48:0"]),
        rest in "[0-9a-fxX:+ -]{0,20}",
    ) {
        let s = format!("{kind}:{rest}");
        // Whatever parses is a well-formed channel.
        if let Ok(Channel::I2c(p)) = Channel::parse(&s) {
            prop_assert!((0x08..=0x77).contains(&p.addr));
        }
    }

    /// Parsing is total over well-formed i2c points.
    #[test]
    fn well_formed_i2c_points_parse(bus in 0u8..=255, addr in 0x08u8..=0x77, reg in any::<u8>(), hex in any::<bool>()) {
        let s = if hex { format!("i2c:{bus}:{addr:#04x}:{reg:#04x}:i16") } else { format!("i2c:{bus}:{addr}:{reg}") };
        let c = Channel::parse(&s).unwrap();
        let Channel::I2c(p) = c else { panic!("{s}") };
        prop_assert_eq!((p.bus, p.addr, p.reg), (bus, addr, reg));
    }
}
