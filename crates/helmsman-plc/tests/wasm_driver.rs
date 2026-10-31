//! A WASM driver behind a link: a guest that runs out of fuel or past its
//! memory cap on one point marks that point bad; the device's other points,
//! the link and the runtime carry on.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::local::Channels;
use helmsman_io::wasm::{Limits, WasmDriver};
use helmsman_io::{Driver, Value, text};
use helmsman_plc::link::links;
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};

/// Answers 7 for every address, but spins on one ending in `2` and grows
/// its memory without end on one ending in `3`.
const GUEST: &str = r#"(module
  (import "helmsman-driver/v1" "point" (func $point (param i32 i32)))
  (import "helmsman-driver/v1" "value" (func $value (param i32 i64)))
  (memory (export "memory") 1)
  (func (export "buffer") (param i32) (result i32) (i32.const 1024))
  (func (export "parse") (param i32 i32) (call $point (i32.const 1) (i32.const 0)))
  (func (export "write") (param i32 i32 i32 i64))
  (func (export "read") (param $p i32) (param $n i32)
    (local $end i32) (local $len i32) (local $last i32)
    (local.set $end (i32.add (local.get $p) (local.get $n)))
    (block $done
      (loop $next
        (br_if $done (i32.ge_u (local.get $p) (local.get $end)))
        (local.set $len (i32.load16_u (local.get $p)))
        (local.set $last (i32.load8_u (i32.add (local.get $p) (i32.add (local.get $len) (i32.const 1)))))
        (if (i32.eq (local.get $last) (i32.const 50)) (then (loop $spin (br $spin))))
        (if (i32.eq (local.get $last) (i32.const 51))
          (then (loop $bomb (drop (memory.grow (i32.const 1))) (br $bomb))))
        (call $value (i32.const 1) (i64.const 7))
        (local.set $p (i32.add (local.get $p) (i32.add (local.get $len) (i32.const 2))))
        (br $next)))))"#;

#[test]
fn a_trapping_guest_marks_its_point_bad_and_nothing_else() {
    // The guest never touches the network; the endpoint is only the map's.
    let src = "device rio modbus-tcp 127.0.0.1:9 timeout_ms=300
in  ok    int  rio hr:1
in  spin  int  rio hr:2
in  bomb  int  rio hr:3
out lamp  bool box do:0
";
    let file = text::parse(src).unwrap();
    let table = Mutex::new(SignalTable::new(&file.map, &BTreeMap::new()).unwrap());
    let wasm = wat::parse_str(GUEST).unwrap();
    let mut links = links(
        &file.map,
        |d| WasmDriver::load(&wasm, d, Limits::default()).map(|w| Box::new(w) as Box<dyn Driver + Send>),
        Box::new(Channels::new()),
    )
    .unwrap();
    let watch = Module {
        name: "watch".into(),
        program: compile(
            "PROGRAM watch
               VAR_INPUT ok : DINT; END_VAR
               VAR_OUTPUT lamp : BOOL; END_VAR
               lamp := ok = 7;
             END_PROGRAM",
            ImageSizes { i: 0, q: 0, m: 0 },
        )
        .unwrap()
        .into(),
        period: Duration::from_millis(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    };
    let mut rt = Runtime::with_signals(0, 0, vec![watch], &table.lock().unwrap()).unwrap();
    for now in [0, 10] {
        for l in &mut links {
            l.cycle(&table).unwrap();
        }
        rt.tick_with(Duration::from_millis(now), &[], &mut table.lock().unwrap());
    }

    let rio = links.iter().find(|l| l.name == "rio").unwrap();
    assert!(rio.is_up(), "a guest's trap is not a dead device");
    let t = table.lock().unwrap();
    let at = |n: &str| t.index(n).unwrap();
    assert_eq!(
        t.value(at("ok")),
        Some(Value::Int(7)),
        "the device's other point reads"
    );
    assert_eq!(t.point_error(at("ok")), None);
    assert_eq!(t.value(at("spin")), None);
    let spin = t.point_error(at("spin")).expect("recorded on spin");
    assert!(spin.contains("out of fuel"), "{spin}");
    assert_eq!(t.value(at("bomb")), None);
    let bomb = t.point_error(at("bomb")).expect("recorded on bomb");
    assert!(bomb.contains("memory cap"), "{bomb}");
    assert_eq!(
        rt.status("watch"),
        Some(Status::Running),
        "the runtime carries on"
    );
    assert_eq!(t.command(at("lamp")), Some(Value::Bool(true)));
}
