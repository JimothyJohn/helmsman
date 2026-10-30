//! Driver conformance against a device: the bench harness.
//!
//! ```text
//! # against the built-in simulator (coil:N wired to di:N for N in 0..8):
//! cargo run -p helmsman-io --example conformance -- --sim
//!
//! # against a Modbus TCP device on the bench, with coil 0 wired to input 0:
//! cargo run -p helmsman-io --example conformance -- \
//!     --endpoint 192.168.3.50:502 --unit 1 \
//!     --point di:0 --point ir:0 --expect ir:0=4242 --loop coil:0=di:0 \
//!     --reg hr:10:f32 --absent hr:9999
//! ```
//!
//! Prints one PASS/FAIL/SKIP line per check; exits 1 if any check failed.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Duration;

use helmsman_io::Value;
use helmsman_io::harness::{self, Bench};
use helmsman_io::modbus::ModbusDriver;
use helmsman_io::sim::{Sim, State};

const USAGE: &str = "usage: conformance (--sim | --endpoint HOST:PORT) [--unit N] [--timeout-ms N]
       [--point ADDR]... [--expect ADDR=VALUE]... [--loop OUT=IN]... [--reg ADDR]...
       [--absent ADDR] [--settle-ms N]
VALUE is true/false, an integer, or a number with a decimal point.";

#[allow(clippy::too_many_lines)] // flag parsing, top to bottom
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut bench = Bench {
        settle: Duration::from_millis(500),
        ..Bench::default()
    };
    let mut params = BTreeMap::new();
    let mut endpoint = None;
    let mut sim = false;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "--sim" {
            sim = true;
            i += 1;
            continue;
        }
        if flag == "-h" || flag == "--help" {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        let Some(v) = args.get(i + 1).cloned() else {
            eprintln!("{flag} needs a value\n{USAGE}");
            return ExitCode::from(2);
        };
        match flag {
            "--endpoint" => endpoint = Some(v),
            "--unit" => drop(params.insert("unit".to_string(), v)),
            "--timeout-ms" => drop(params.insert("timeout_ms".to_string(), v)),
            "--point" => bench.points.push(v),
            "--reg" => bench.registers.push(v),
            "--absent" => bench.absent = Some(v),
            "--settle-ms" => {
                if let Ok(ms) = v.parse() {
                    bench.settle = Duration::from_millis(ms);
                } else {
                    eprintln!("--settle-ms: not a number: {v}");
                    return ExitCode::from(2);
                }
            }
            "--expect" => {
                if let Some((a, x)) = v.split_once('=').and_then(|(a, x)| Some((a, parse_value(x)?))) {
                    bench.expect.push((a.into(), x));
                } else {
                    eprintln!("--expect wants ADDR=VALUE, got {v}");
                    return ExitCode::from(2);
                }
            }
            "--loop" => {
                if let Some((o, i)) = v.split_once('=') {
                    bench.loops.push((o.into(), i.into()));
                } else {
                    eprintln!("--loop wants OUT=IN, got {v}");
                    return ExitCode::from(2);
                }
            }
            other => {
                eprintln!("unknown flag {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
        i += 2;
    }

    let running = if sim {
        let mut state = State::new(64);
        state.wires = (0..8).map(|n| (n, n)).collect();
        state.input[0] = 4242;
        // 1.5f32 = 0x3FC0_0000, high word first, at hr:20..=21.
        state.holding[20] = 0x3FC0;
        match Sim::start(state) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("simulator: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    if let Some(s) = &running {
        endpoint = Some(s.endpoint());
        if bench.points.is_empty() && bench.loops.is_empty() && bench.registers.is_empty() {
            bench.points = vec!["di:0".into(), "ir:0".into(), "coil:3".into(), "hr:4:i32".into()];
            bench.loops = vec![("coil:0".into(), "di:0".into()), ("coil:5".into(), "di:5".into())];
            bench.registers = vec![
                "hr:0".into(),
                "hr:1:i16".into(),
                "hr:2:u32sw".into(),
                "hr:10:f32".into(),
            ];
            bench.absent = Some("hr:9999".into());
            bench.expect = vec![
                ("ir:0".into(), Value::Int(4242)),
                ("hr:20:f32".into(), Value::Real(1.5)),
            ];
        }
        params
            .entry("timeout_ms".to_string())
            .or_insert_with(|| "300".to_string());
    }
    let Some(endpoint) = endpoint else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut driver = match ModbusDriver::from_params(&endpoint, &params) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    println!("modbus-tcp conformance against {endpoint}");
    let checks = harness::run(&mut driver, &bench, running.as_ref());
    print!("{}", harness::render(&checks));
    if harness::ok(&checks) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_value(s: &str) -> Option<Value> {
    match s {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ if s.contains('.') => s.parse().ok().map(Value::Real),
        _ => s.parse().ok().map(Value::Int),
    }
}
