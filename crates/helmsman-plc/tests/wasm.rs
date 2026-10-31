//! The WASM module tier (`wasm`): modules compiled to WebAssembly run in the
//! scan beside ST modules, bound to signals by name through the
//! `helmsman/v1` imports, under fuel and memory limits. Every test module
//! is hand-written WAT, assembled here, so what each one does is on the
//! page.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use helmsman_iec::{FaultKind, ImageSizes, PortDir, Ty as StTy, Value as St, compile};
use helmsman_io::{Binding, Dir, IoMap, Signal, Ty, Value};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::spec::Defaults;
use helmsman_plc::wasm::{Limits, WasmProgram};
use helmsman_plc::{ConfigError, Logic, Module, Runtime, SignalTable, Status, Window};
use proptest::prelude::*;

const D: Defaults = Defaults {
    period: Duration::from_millis(10),
    i: Window::new(0, 0),
    q: Window::new(0, 0),
    m: 0,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn wasm(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap_or_else(|e| panic!("WAT does not assemble: {e}"))
}

fn load(wat: &str) -> WasmProgram {
    WasmProgram::load(&wasm(wat), Limits::default()).unwrap_or_else(|e| panic!("does not load: {e:?}"))
}

fn load_err(wat: &str) -> String {
    WasmProgram::load(&wasm(wat), Limits::default())
        .map(|_| ())
        .expect_err("should be refused")
        .join("\n")
}

fn module(name: &str, program: impl Into<Logic>) -> Module {
    Module {
        name: name.into(),
        program: program.into(),
        period: ms(10),
        i: Window::new(0, 0),
        q: Window::new(0, 0),
        params: Vec::new(),
    }
}

fn st(name: &str, src: &str) -> Module {
    module(
        name,
        compile(src, ImageSizes { i: 0, q: 0, m: 0 }).unwrap_or_else(|d| panic!("{name}: {d:?}")),
    )
}

fn local(name: &str, ty: Ty, dir: Dir, channel: &str) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir,
        binding: Binding::Local {
            channel: channel.into(),
        },
        unit: None,
        poll: None,
    }
}

/// A screen tile output: an `out` signal of any type no device takes.
fn tile(name: &str, ty: Ty) -> Signal {
    Signal {
        name: name.into(),
        ty,
        dir: Dir::Out,
        binding: Binding::Screen { tile: "t".into() },
        unit: None,
        poll: None,
    }
}

/// Inputs `start` (a channel), `level`, `count_in` (box inputs); outputs
/// `alarm`, `lamp`, `beacon` (channels), `count`, `now` (screen tiles).
fn table() -> SignalTable {
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![
            local("start", Ty::Bool, Dir::In, "di:0"),
            tile("count", Ty::Int),
            local("alarm", Ty::Bool, Dir::Out, "do:1"),
            local("lamp", Ty::Bool, Dir::Out, "do:2"),
            local("beacon", Ty::Bool, Dir::Out, "do:3"),
            tile("now", Ty::Int),
        ],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    t.add_box_input("level", Ty::Real).unwrap();
    t.add_box_input("count_in", Ty::Int).unwrap();
    t
}

fn set(t: &mut SignalTable, name: &str, v: Value) {
    let i = t.index(name).unwrap();
    t.set_input(i, Some(v));
}

fn cmd(t: &SignalTable, name: &str) -> Option<Value> {
    t.command(t.index(name).unwrap())
}

fn safe(t: &SignalTable, name: &str) -> Option<Value> {
    t.safe(t.index(name).unwrap())
}

/// Counts scans while `start` is on; the count is an exported global.
const COUNTER: &str = r#"(module
  (import "helmsman/v1/in" "start" (func $start (result i32)))
  (import "helmsman/v1/out" "count" (func $count (param i64)))
  (global $n (export "n") (mut i64) (i64.const 0))
  (func (export "scan")
    (if (call $start)
      (then (global.set $n (i64.add (global.get $n) (i64.const 1)))))
    (call $count (global.get $n))))"#;

/// `alarm` when `level` is over the `limit` param.
const THRESHOLD: &str = r#"(module
  (import "helmsman/v1/in" "level" (func $level (result f64)))
  (import "helmsman/v1/in" "limit" (func $limit (result f64)))
  (import "helmsman/v1/out" "alarm" (func $alarm (param i32)))
  (func (export "scan")
    (call $alarm (f64.gt (call $level) (call $limit)))))"#;

/// Never returns.
const SPIN: &str = r#"(module
  (import "helmsman/v1/out" "lamp" (func $lamp (param i32)))
  (func (export "scan")
    (call $lamp (i32.const 1))
    (loop $l (br $l))))"#;

/// Reads past its one page of memory.
const OOB: &str = r#"(module
  (import "helmsman/v1/out" "lamp" (func $lamp (param i32)))
  (memory 1)
  (func (export "scan")
    (call $lamp (i32.and (i32.load (i32.const 70000)) (i32.const 1)))))"#;

const MANIFEST_COUNTER: &str = "name counter\nauthor test\nscan_ms 10\nreads start\ndrives count\n";
const MANIFEST_THRESHOLD: &str =
    "name threshold\nauthor test\nscan_ms 10\nreads level\ndrives alarm\nparam limit real 50 min=0 max=100\n";

fn dir_module(manifest: &str, wat: &str) -> Result<Module, String> {
    ModuleDir::from_wasm(Path::new("m"), manifest, wasm(wat), &D)?.wasm_module(Limits::default())
}

// ---- the interface ----

#[test]
fn ports_come_from_the_imports_with_their_st_types() {
    let p = load(THRESHOLD);
    let ports: Vec<_> = p.ports().into_iter().map(|p| (p.name, p.dir, p.ty)).collect();
    assert_eq!(
        ports,
        vec![
            ("LEVEL".to_string(), PortDir::Input, StTy::LReal),
            ("LIMIT".to_string(), PortDir::Input, StTy::LReal),
            ("ALARM".to_string(), PortDir::Output, StTy::Bool),
        ]
    );
}

#[test]
fn an_import_outside_the_interface_is_refused() {
    let wasi = load_err(
        r#"(module
          (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
          (func (export "scan")))"#,
    );
    assert!(wasi.contains("not part of the helmsman/v1 interface"), "{wasi}");
    let clock = load_err(
        r#"(module
          (import "helmsman/v1" "wall_clock" (func (result i64)))
          (func (export "scan")))"#,
    );
    assert!(clock.contains("not part of the helmsman/v1 interface"), "{clock}");
    let memory = load_err(r#"(module (import "helmsman/v1" "memory" (memory 1)) (func (export "scan")))"#);
    assert!(memory.contains("only functions"), "{memory}");
    let global =
        load_err(r#"(module (import "helmsman/v1/in" "start" (global i32)) (func (export "scan")))"#);
    assert!(global.contains("only functions"), "{global}");
}

#[test]
fn ports_of_the_wrong_shape_are_refused() {
    for (wat, want) in [
        (
            r#"(module (import "helmsman/v1/in" "a" (func (param i32))) (func (export "scan")))"#,
            "must be () -> i32 / i64 / f64",
        ),
        (
            r#"(module (import "helmsman/v1/out" "a" (func (result i32))) (func (export "scan")))"#,
            "must be (i32 / i64 / f64) -> ()",
        ),
        (
            r#"(module (import "helmsman/v1/in" "a" (func (result f32))) (func (export "scan")))"#,
            "i32 (BOOL), i64 (LINT) or f64 (LREAL)",
        ),
        (
            r#"(module (import "helmsman/v1/in" "a b" (func (result i32))) (func (export "scan")))"#,
            "identifier",
        ),
        (
            r#"(module (import "helmsman/v1/in" "a" (func (result i32)))
                       (import "helmsman/v1/out" "A" (func (param i32))) (func (export "scan")))"#,
            "imported twice",
        ),
        (
            r#"(module (import "helmsman/v1" "now_ms" (func (result i32))) (func (export "scan")))"#,
            "must be () -> i64",
        ),
        (r#"(module (func (export "run")))"#, "no export \"scan\""),
        (
            r#"(module (func (export "scan") (param i32)))"#,
            "no export \"scan\"",
        ),
    ] {
        let err = load_err(wat);
        assert!(err.contains(want), "{wat}: {err}");
    }
}

#[test]
fn not_webassembly_and_start_functions_are_refused() {
    let err = WasmProgram::load(b"\0asm garbage", Limits::default())
        .map(|_| ())
        .unwrap_err();
    assert!(err[0].contains("not a valid WASM module"), "{err:?}");
    let err = WasmProgram::load(&[], Limits::default()).map(|_| ()).unwrap_err();
    assert!(err[0].contains("not a valid WASM module"), "{err:?}");
    let err = load_err(r#"(module (func $s) (start $s) (func (export "scan")))"#);
    assert!(err.contains("not a valid WASM module"), "{err}");
}

#[test]
fn memory_past_the_cap_is_refused_and_cannot_be_grown() {
    // 100 pages = 6.25 MiB, past the 4 MiB default.
    let err = load_err(r#"(module (memory 100) (func (export "scan")))"#);
    assert!(err.contains("does not instantiate"), "{err}");
    let small = Limits {
        memory: 64 * 1024,
        ..Limits::default()
    };
    // Growing past the cap fails as memory.grow does: -1, not a crash.
    let mut p = WasmProgram::load(
        &wasm(
            r#"(module
              (import "helmsman/v1/out" "now" (func $out (param i64)))
              (memory 1)
              (func (export "scan") (call $out (i64.extend_i32_s (memory.grow (i32.const 1))))))"#,
        ),
        small,
    )
    .unwrap();
    p.scan_at(ms(0)).unwrap();
    assert_eq!(p.get("now"), Some(St::Int(-1)));
    assert_eq!(p.limits(), small);
}

// ---- the manifest ----

#[test]
fn a_module_directory_with_module_wasm_loads_against_its_manifest() {
    let m = dir_module(MANIFEST_THRESHOLD, THRESHOLD).unwrap();
    assert_eq!(m.name, "threshold");
    assert_eq!(m.program.kind(), "wasm");
    assert_eq!(m.params, vec![("limit".to_string(), Value::Real(50.0))]);
    assert_eq!((m.i, m.q), (Window::new(0, 0), Window::new(0, 0)));
}

#[test]
fn a_manifest_that_lies_about_the_wasm_is_refused() {
    // Says it drives lamp; the module drives count and reads start.
    let lie = "name counter\nauthor test\nscan_ms 10\nreads\ndrives lamp\n";
    let err = dir_module(lie, COUNTER).unwrap_err();
    assert!(err.contains("does not match module.wasm"), "{err}");
    assert!(err.contains("VAR_INPUT START is not in the manifest"), "{err}");
    assert!(err.contains("VAR_OUTPUT COUNT is not in the manifest"), "{err}");
    assert!(
        err.contains("drives lamp: the program has no VAR_OUTPUT"),
        "{err}"
    );
    // A param the port cannot hold: an int range on an f64 port.
    let bad =
        "name threshold\nauthor test\nscan_ms 10\nreads level\ndrives alarm\nparam limit int 5 min=0 max=9\n";
    let err = dir_module(bad, THRESHOLD).unwrap_err();
    assert!(err.contains("param limit"), "{err}");
}

#[test]
fn a_wasm_module_has_no_windows() {
    let m = format!("{MANIFEST_COUNTER}q 0+1\n");
    let err = dir_module(&m, COUNTER).unwrap_err();
    assert!(err.contains("no i, q or m windows"), "{err}");
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("wasm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn module_dir_read_picks_module_wasm_and_refuses_both_files() {
    let dir = tmp("read");
    std::fs::write(dir.join("module.manifest"), MANIFEST_COUNTER).unwrap();
    std::fs::write(dir.join("module.wasm"), wasm(COUNTER)).unwrap();
    let md = ModuleDir::read(&dir, &D).unwrap();
    assert!(md.spec.path.ends_with("module.wasm"));
    assert_eq!(md.wasm.as_deref(), Some(wasm(COUNTER).as_slice()));
    assert_eq!(md.wasm_module(Limits::default()).unwrap().name, "counter");
    std::fs::write(dir.join("module.st"), "PROGRAM p END_PROGRAM").unwrap();
    let err = ModuleDir::read(&dir, &D).unwrap_err();
    assert!(err.contains("both module.st and module.wasm"), "{err}");
    // An ST directory is not a WASM module.
    std::fs::remove_file(dir.join("module.wasm")).unwrap();
    let md = ModuleDir::read(&dir, &D).unwrap();
    assert!(
        md.wasm_module(Limits::default())
            .unwrap_err()
            .contains("not a WASM module")
    );
}

// ---- in the scan ----

#[test]
fn counter_and_threshold_run_on_signals() {
    let mut t = table();
    let counter = dir_module(MANIFEST_COUNTER, COUNTER).unwrap();
    let threshold = dir_module(MANIFEST_THRESHOLD, THRESHOLD).unwrap();
    let mut rt = Runtime::with_signals(0, 0, vec![counter, threshold], &t).unwrap();
    set(&mut t, "start", Value::Bool(true));
    set(&mut t, "level", Value::Real(10.0));
    for k in 0..3 {
        assert_eq!(rt.tick_with(ms(10 * k), &[], &mut t), Vec::new());
    }
    assert_eq!(cmd(&t, "count"), Some(Value::Int(3)));
    assert_eq!(cmd(&t, "alarm"), Some(Value::Bool(false)));
    set(&mut t, "start", Value::Bool(false));
    set(&mut t, "level", Value::Real(50.5));
    rt.tick_with(ms(30), &[], &mut t);
    assert_eq!(cmd(&t, "count"), Some(Value::Int(3)));
    assert_eq!(cmd(&t, "alarm"), Some(Value::Bool(true)));
    let counter = rt.module("counter").unwrap();
    assert_eq!(counter.program.get("n"), Some(St::Int(3)));
    assert_eq!(rt.program("counter").map(|_| ()), None, "no ST program to read");
}

#[test]
fn a_wasm_module_with_a_missing_input_is_held() {
    let mut t = table();
    let mut rt =
        Runtime::with_signals(0, 0, vec![dir_module(MANIFEST_COUNTER, COUNTER).unwrap()], &t).unwrap();
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(rt.status("counter"), Some(Status::Held));
    assert_eq!(rt.stats("counter").unwrap().scans, 0);
}

#[test]
fn the_scan_time_is_the_runtimes() {
    let wat = r#"(module
      (import "helmsman/v1" "now_ms" (func $now (result i64)))
      (import "helmsman/v1/out" "now" (func $out (param i64)))
      (func (export "scan") (call $out (call $now))))"#;
    let mut t = table();
    let mut rt = Runtime::with_signals(0, 0, vec![module("clock", load(wat))], &t).unwrap();
    rt.tick_with(ms(1234), &[], &mut t);
    assert_eq!(cmd(&t, "now"), Some(Value::Int(1234)));
}

/// One faulting WASM module next to an ST module and a healthy WASM one.
fn fault_beside_others(bad: Module) -> (FaultKind, SignalTable, Runtime) {
    let mut t = table();
    let lamp_st = st(
        "beacon",
        "PROGRAM p VAR_INPUT start : BOOL; END_VAR VAR_OUTPUT beacon : BOOL; END_VAR beacon := start; END_PROGRAM",
    );
    let counter = dir_module(MANIFEST_COUNTER, COUNTER).unwrap();
    let name = bad.name.clone();
    let mut rt = Runtime::with_signals(0, 0, vec![lamp_st, bad, counter], &t).unwrap();
    set(&mut t, "start", Value::Bool(true));
    let faults = rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0].0, name);
    // The others keep their schedule.
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(cmd(&t, "beacon"), Some(Value::Bool(true)));
    assert_eq!(cmd(&t, "count"), Some(Value::Int(2)));
    assert!(matches!(rt.status(&name), Some(Status::Faulted(_))));
    assert_eq!(
        rt.stats(&name).unwrap().scans,
        1,
        "a faulted module is not scanned again"
    );
    (faults[0].1.kind, t, rt)
}

#[test]
fn fuel_exhaustion_is_a_watchdog_fault_and_outputs_go_safe() {
    let (kind, t, _) = fault_beside_others(module("spin", load(SPIN)));
    assert_eq!(kind, FaultKind::Watchdog);
    // It set lamp before spinning; a faulted scan publishes nothing.
    assert_eq!(cmd(&t, "lamp"), safe(&t, "lamp"));
}

#[test]
fn out_of_bounds_memory_is_a_fault() {
    let (kind, t, _) = fault_beside_others(module("oob", load(OOB)));
    assert_eq!(kind, FaultKind::IndexOutOfRange);
    assert_eq!(cmd(&t, "lamp"), safe(&t, "lamp"));
}

#[test]
fn traps_map_to_faults() {
    let lamp = |body: &str| {
        format!(
            r#"(module (import "helmsman/v1/out" "lamp" (func $lamp (param i32)))
                 (global $z (mut i32) (i32.const 0))
                 (func $deep (call $deep))
                 (func (export "scan") {body}))"#
        )
    };
    for (body, want) in [
        (
            "(call $lamp (i32.div_s (i32.const 1) (global.get $z)))",
            FaultKind::DivisionByZero,
        ),
        ("unreachable", FaultKind::Trap),
        ("(call $deep)", FaultKind::Trap),
        ("(call $lamp (i32.const 2))", FaultKind::OutputRange),
        ("(call $lamp (i32.const -1))", FaultKind::OutputRange),
    ] {
        let (kind, ..) = fault_beside_others(module("bad", load(&lamp(body))));
        assert_eq!(kind, want, "{body}");
    }
}

#[test]
fn a_non_finite_real_output_is_an_output_range_fault() {
    let wat = r#"(module
      (import "helmsman/v1/out" "level" (func $o (param f64)))
      (func (export "scan") (call $o (f64.div (f64.const 1) (f64.const 0)))))"#;
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![tile("level", Ty::Real)],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    let mut rt = Runtime::with_signals(0, 0, vec![module("inf", load(wat))], &t).unwrap();
    let faults = rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(faults[0].1.kind, FaultKind::OutputRange);
}

#[test]
fn fuel_is_per_scan_not_cumulative() {
    // About 3 000 instructions a scan, under a 10 000 limit: never runs out.
    let wat = r#"(module
      (import "helmsman/v1/out" "count" (func $o (param i64)))
      (func (export "scan") (local $i i64)
        (loop $l
          (local.set $i (i64.add (local.get $i) (i64.const 1)))
          (br_if $l (i64.lt_s (local.get $i) (i64.const 1000))))
        (call $o (local.get $i))))"#;
    let limits = Limits {
        fuel: 10_000,
        ..Limits::default()
    };
    let mut p = WasmProgram::load(&wasm(wat), limits).unwrap();
    for k in 0..50 {
        p.scan_at(ms(k)).unwrap();
    }
    let tight = Limits { fuel: 500, ..limits };
    let mut p = WasmProgram::load(&wasm(wat), tight).unwrap();
    assert_eq!(p.scan_at(ms(0)).unwrap_err().kind, FaultKind::Watchdog);
}

#[test]
fn a_wasm_port_binds_like_an_st_port() {
    let t = table();
    // Drives an input signal; reads a signal that does not exist.
    let wat = r#"(module
      (import "helmsman/v1/in" "nosuch" (func (result i32)))
      (import "helmsman/v1/out" "start" (func (param i32)))
      (func (export "scan")))"#;
    let err = Runtime::with_signals(0, 0, vec![module("w", load(wat))], &t).unwrap_err();
    assert_eq!(err.len(), 2, "{err:?}");
    assert!(
        err.iter().all(|e| matches!(e, ConfigError::Port { .. })),
        "{err:?}"
    );
    // An f64 port on an int signal is the wrong type.
    let wat = r#"(module (import "helmsman/v1/in" "count_in" (func (result f64))) (func (export "scan")))"#;
    let err = Runtime::with_signals(0, 0, vec![module("w", load(wat))], &t).unwrap_err();
    assert!(matches!(err[0], ConfigError::Port { .. }), "{err:?}");
    // Two writers, one ST and one WASM.
    let st_count = st(
        "st",
        "PROGRAM p VAR_OUTPUT count : LINT; END_VAR count := 1; END_PROGRAM",
    );
    let wat = r#"(module (import "helmsman/v1/out" "count" (func (param i64))) (func (export "scan")))"#;
    let err = Runtime::with_signals(0, 0, vec![st_count, module("w", load(wat))], &t).unwrap_err();
    assert!(matches!(err[0], ConfigError::TwoWriters { .. }), "{err:?}");
}

#[test]
fn st_and_wasm_read_each_others_outputs_through_module_signals() {
    let map = IoMap {
        devices: Vec::new(),
        signals: vec![
            local("start", Ty::Bool, Dir::In, "di:0"),
            Signal {
                name: "count".into(),
                ty: Ty::Int,
                dir: Dir::In,
                binding: Binding::Module {
                    module: "counter".into(),
                },
                unit: None,
                poll: None,
            },
            local("lamp", Ty::Bool, Dir::Out, "do:0"),
        ],
    };
    let mut t = SignalTable::new(&map, &BTreeMap::new()).unwrap();
    let counter = dir_module(MANIFEST_COUNTER, COUNTER).unwrap();
    let reader = st(
        "reader",
        "PROGRAM p VAR_INPUT count : LINT; END_VAR VAR_OUTPUT lamp : BOOL; END_VAR lamp := count >= 2; END_PROGRAM",
    );
    let mut rt = Runtime::with_signals(0, 0, vec![counter, reader], &t).unwrap();
    set(&mut t, "start", Value::Bool(true));
    rt.tick_with(ms(0), &[], &mut t);
    assert_eq!(cmd(&t, "lamp"), Some(Value::Bool(false)));
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(cmd(&t, "lamp"), Some(Value::Bool(true)));
}

// ---- hot swap ----

#[test]
fn swapping_a_wasm_module_carries_its_globals_and_outputs() {
    let mut t = table();
    let mut rt =
        Runtime::with_signals(0, 0, vec![dir_module(MANIFEST_COUNTER, COUNTER).unwrap()], &t).unwrap();
    set(&mut t, "start", Value::Bool(true));
    for k in 0..5 {
        rt.tick_with(ms(10 * k), &[], &mut t);
    }
    assert_eq!(cmd(&t, "count"), Some(Value::Int(5)));
    // v2 counts by ten; same global and output, so both carry.
    let v2 = COUNTER.replace("(i64.const 1)", "(i64.const 10)");
    let carry = rt
        .swap(dir_module(MANIFEST_COUNTER, &v2).unwrap(), &mut t)
        .unwrap();
    assert_eq!(carry.kept, vec!["COUNT".to_string(), "n".to_string()]);
    assert_eq!(
        cmd(&t, "count"),
        Some(Value::Int(5)),
        "no glitch before the first scan"
    );
    rt.tick_with(ms(50), &[], &mut t);
    assert_eq!(cmd(&t, "count"), Some(Value::Int(15)));
    // v3 retypes the global: it starts again.
    let v3 = COUNTER
        .replace(
            "(global $n (export \"n\") (mut i64) (i64.const 0))",
            "(global $n (export \"n\") (mut i32) (i32.const 0))",
        )
        .replace(
            "(global.set $n (i64.add (global.get $n) (i64.const 1)))",
            "(global.set $n (i32.add (global.get $n) (i32.const 1)))",
        )
        .replace(
            "(call $count (global.get $n))",
            "(call $count (i64.extend_i32_s (global.get $n)))",
        );
    let carry = rt
        .swap(dir_module(MANIFEST_COUNTER, &v3).unwrap(), &mut t)
        .unwrap();
    assert_eq!(carry.reset, vec!["n".to_string()]);
    rt.tick_with(ms(60), &[], &mut t);
    assert_eq!(cmd(&t, "count"), Some(Value::Int(1)));
}

#[test]
fn a_faulted_wasm_module_restarts_on_swap_and_st_can_replace_wasm() {
    let mut t = table();
    let mut rt = Runtime::with_signals(0, 0, vec![module("lampy", load(SPIN))], &t).unwrap();
    assert_eq!(rt.tick_with(ms(0), &[], &mut t)[0].1.kind, FaultKind::Watchdog);
    let fixed = r#"(module
      (import "helmsman/v1/out" "lamp" (func $lamp (param i32)))
      (func (export "scan") (call $lamp (i32.const 1))))"#;
    rt.swap(module("lampy", load(fixed)), &mut t).unwrap();
    assert_eq!(rt.status("lampy"), Some(Status::Running));
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(cmd(&t, "lamp"), Some(Value::Bool(true)));
    // The same module, now in ST: nothing carries across kinds, it runs.
    let as_st = st(
        "lampy",
        "PROGRAM p VAR_OUTPUT lamp : BOOL; END_VAR lamp := FALSE; END_PROGRAM",
    );
    let carry = rt.swap(as_st, &mut t).unwrap();
    assert_eq!(carry.kept, Vec::<String>::new(), "{carry:?}");
    rt.tick_with(ms(20), &[], &mut t);
    assert_eq!(cmd(&t, "lamp"), Some(Value::Bool(false)));
}

#[test]
fn a_refused_wasm_swap_changes_nothing() {
    let mut t = table();
    let mut rt =
        Runtime::with_signals(0, 0, vec![dir_module(MANIFEST_COUNTER, COUNTER).unwrap()], &t).unwrap();
    set(&mut t, "start", Value::Bool(true));
    rt.tick_with(ms(0), &[], &mut t);
    let bad = r#"(module (import "helmsman/v1/out" "start" (func (param i32))) (func (export "scan")))"#;
    assert!(rt.swap(module("counter", load(bad)), &mut t).is_err());
    rt.tick_with(ms(10), &[], &mut t);
    assert_eq!(cmd(&t, "count"), Some(Value::Int(2)));
}

#[test]
fn reconfigure_and_replay_reset_treat_wasm_like_st() {
    let mut t = table();
    let mut rt =
        Runtime::with_signals(0, 0, vec![dir_module(MANIFEST_COUNTER, COUNTER).unwrap()], &t).unwrap();
    set(&mut t, "start", Value::Bool(true));
    rt.tick_with(ms(0), &[], &mut t);
    rt.tick_with(ms(10), &[], &mut t);
    let threshold = dir_module(MANIFEST_THRESHOLD, THRESHOLD).unwrap();
    set(&mut t, "level", Value::Real(99.0));
    rt.reconfigure(
        vec![dir_module(MANIFEST_COUNTER, COUNTER).unwrap(), threshold],
        &mut t,
    )
    .unwrap();
    rt.tick_with(ms(20), &[], &mut t);
    assert_eq!(
        cmd(&t, "count"),
        Some(Value::Int(3)),
        "the kept module carried its count"
    );
    assert_eq!(cmd(&t, "alarm"), Some(Value::Bool(true)));
    // A clone keeps the state; a reset is a cold restart.
    let mut copy = rt.module("counter").unwrap().program.clone();
    assert_eq!(copy.get("n"), Some(St::Int(3)));
    copy.reset();
    assert_eq!(copy.get("n"), Some(St::Int(0)));
}

// ---- determinism ----

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Two instances fed the same inputs and times give the same outputs,
    /// scan for scan.
    #[test]
    fn same_inputs_same_outputs(scans in prop::collection::vec((any::<bool>(), -1e6f64..1e6, 0u64..100_000), 1..40)) {
        let wat = r#"(module
          (import "helmsman/v1/in" "start" (func $start (result i32)))
          (import "helmsman/v1/in" "level" (func $level (result f64)))
          (import "helmsman/v1" "now_ms" (func $now (result i64)))
          (import "helmsman/v1/out" "count" (func $count (param i64)))
          (import "helmsman/v1/out" "alarm" (func $alarm (param i32)))
          (global $acc (mut f64) (f64.const 0))
          (func (export "scan")
            (global.set $acc (f64.add (f64.mul (global.get $acc) (f64.const 0.5)) (call $level)))
            (call $alarm (i32.and (call $start) (f64.gt (global.get $acc) (f64.const 0))))
            (call $count (i64.xor (call $now) (i64.trunc_sat_f64_s (global.get $acc))))))"#;
        let (mut a, mut b) = (load(wat), load(wat));
        for (start, level, now) in scans {
            for p in [&mut a, &mut b] {
                p.set_input("start", St::Bool(start)).unwrap();
                p.set_input("level", St::Real(level)).unwrap();
                p.scan_at(ms(now)).unwrap();
            }
            prop_assert_eq!(a.get("count"), b.get("count"));
            prop_assert_eq!(a.get("alarm"), b.get("alarm"));
        }
    }

    /// Whatever value is offered, an input takes it exactly when ST would
    /// take it on a port of the same type.
    #[test]
    fn inputs_take_what_st_takes(n in any::<i64>(), x in any::<f64>(), wide in any::<i128>()) {
        let wat = r#"(module
          (import "helmsman/v1/in" "b" (func (result i32)))
          (import "helmsman/v1/in" "i" (func (result i64)))
          (import "helmsman/v1/in" "r" (func (result f64)))
          (func (export "scan")))"#;
        let mut w = load(wat);
        let mut s = compile(
            "PROGRAM p VAR_INPUT b : BOOL; i : LINT; r : LREAL; END_VAR END_PROGRAM",
            ImageSizes { i: 0, q: 0, m: 0 },
        ).unwrap();
        for (port, v) in [
            ("b", St::Bool(n % 2 == 0)),
            ("b", St::Int(n.into())),
            ("i", St::Int(n.into())),
            ("i", St::Int(wide)),
            ("i", St::Real(x)),
            ("r", St::Real(x)),
            ("r", St::Bool(true)),
        ] {
            prop_assert_eq!(w.set_input(port, v).is_ok(), s.set_input(port, v).is_ok(), "{} {:?}", port, v);
        }
        prop_assert!(w.set_input("nosuch", St::Bool(true)).is_err());
    }
}
