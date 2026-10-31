//! What a WASM module costs the scan beside the same logic in ST: one
//! runtime tick of a small module (two inputs, one output, a counter), and
//! a heavier one (a 1 000-pass loop), each as ST and as WASM; plus what
//! loading a module costs.
//!
//! `cargo run --release -p helmsman-plc --example wasm_bench`

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::wasm::{Limits, WasmProgram};
use helmsman_plc::{Logic, Module, Runtime, SignalTable, Window};

const TICKS: u64 = 200_000;

const ST_SMALL: &str = "PROGRAM p VAR_INPUT start : BOOL; level : LINT; END_VAR VAR_OUTPUT out : LINT; END_VAR \
                        VAR n : LINT; END_VAR IF start AND level > 3 THEN n := n + 1; END_IF; out := n; END_PROGRAM";

const WAT_SMALL: &str = r#"(module
  (import "helmsman/v1/in" "start" (func $start (result i32)))
  (import "helmsman/v1/in" "level" (func $level (result i64)))
  (import "helmsman/v1/out" "out" (func $out (param i64)))
  (global $n (export "n") (mut i64) (i64.const 0))
  (func (export "scan")
    (if (i32.and (call $start) (i64.gt_s (call $level) (i64.const 3)))
      (then (global.set $n (i64.add (global.get $n) (i64.const 1)))))
    (call $out (global.get $n))))"#;

const ST_LOOP: &str = "PROGRAM p VAR_INPUT start : BOOL; level : LINT; END_VAR VAR_OUTPUT out : LINT; END_VAR \
                       VAR i : LINT; acc : LINT; END_VAR acc := 0; FOR i := 1 TO 1000 DO acc := acc + i * level; \
                       END_FOR; out := acc; END_PROGRAM";

const WAT_LOOP: &str = r#"(module
  (import "helmsman/v1/in" "start" (func $start (result i32)))
  (import "helmsman/v1/in" "level" (func $level (result i64)))
  (import "helmsman/v1/out" "out" (func $out (param i64)))
  (func (export "scan") (local $i i64) (local $acc i64) (local $l i64)
    (local.set $l (call $level))
    (local.set $i (i64.const 1))
    (loop $top
      (local.set $acc (i64.add (local.get $acc) (i64.mul (local.get $i) (local.get $l))))
      (local.set $i (i64.add (local.get $i) (i64.const 1)))
      (br_if $top (i64.le_s (local.get $i) (i64.const 1000))))
    (call $out (local.get $acc))))"#;

fn table() -> SignalTable {
    let local = |name: &str, ty, dir, ch: &str| Signal {
        name: name.into(),
        ty,
        dir,
        binding: Binding::Local { channel: ch.into() },
        unit: None,
        poll: None,
    };
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![
            local("start", Ty::Bool, Dir::In, "di:0"),
            Signal {
                name: "out".into(),
                ty: Ty::Int,
                dir: Dir::Out,
                binding: Binding::Screen { tile: "t".into() },
                unit: None,
                poll: None,
            },
        ],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    let level = t.add_box_input("level", Ty::Int).unwrap();
    t.set_input(0, Some(Value::Bool(true)));
    t.set_input(level, Some(Value::Int(7)));
    t
}

/// Per-tick nanoseconds: mean and worst, over `ticks`.
fn bench(program: Logic, ticks: u64) -> (u64, u64) {
    let mut t = table();
    let m = Module {
        name: "p".into(),
        program,
        period: Duration::from_millis(1),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    };
    let mut rt = Runtime::with_signals(0, 0, vec![m], &t).unwrap();
    let (mut sum, mut worst) = (0u64, 0u64);
    for tick in 0..ticks {
        let s = Instant::now();
        black_box(rt.tick_with(Duration::from_millis(tick), &[], &mut t));
        let ns = u64::try_from(s.elapsed().as_nanos()).unwrap_or(u64::MAX);
        sum += ns;
        worst = worst.max(ns);
    }
    (sum / ticks.max(1), worst)
}

fn st(src: &str) -> Logic {
    compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap().into()
}

fn wasm(wat: &str) -> Logic {
    WasmProgram::load(&wat::parse_str(wat).unwrap(), Limits::default())
        .unwrap()
        .into()
}

fn main() {
    for (what, st_src, wat, ticks) in [
        ("small module", ST_SMALL, WAT_SMALL, TICKS),
        ("1000-pass loop", ST_LOOP, WAT_LOOP, TICKS / 10),
    ] {
        let (st_mean, st_worst) = bench(st(st_src), ticks);
        let (w_mean, w_worst) = bench(wasm(wat), ticks);
        println!("{what}: ST {st_mean} ns/tick (worst {st_worst}), WASM {w_mean} ns/tick (worst {w_worst})");
    }
    let bytes = wat::parse_str(WAT_LOOP).unwrap();
    let s = Instant::now();
    for _ in 0..1000 {
        black_box(WasmProgram::load(&bytes, Limits::default()).unwrap());
    }
    println!("load + instantiate: {} us", s.elapsed().as_micros() / 1000);
}
