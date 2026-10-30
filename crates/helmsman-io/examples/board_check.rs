//! Checks the board backend on real hardware (a Pi): opens a GPIO chip,
//! reads the channels given, and proves a loop (an output wired to an
//! input) if one is given. `scripts/pi-gpio-check.sh` builds and runs it.
//!
//! ```text
//! board_check --chip /dev/gpiochip0 --read di:4 --read i2c:1:0x48:0:i16 \
//!     [--loop do:17=di:27]
//! ```
//!
//! Prints one PASS/FAIL line per check; exits 1 if any failed, 2 on bad
//! arguments or a build without the Linux backend.

use std::process::ExitCode;

const USAGE: &str = "usage: board_check --chip PATH [--read CHANNEL]... [--loop do:N=di:M]...";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut chip, mut reads, mut loops) = (None, Vec::new(), Vec::new());
    for pair in args.chunks(2) {
        match pair {
            [f, v] if f == "--chip" => chip = Some(v.clone()),
            [f, v] if f == "--read" => reads.push(v.clone()),
            [f, v] if f == "--loop" => {
                let Some((o, i)) = v.split_once('=') else {
                    eprintln!("--loop wants do:N=di:M, got {v}");
                    return ExitCode::from(2);
                };
                loops.push((o.to_string(), i.to_string()));
            }
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(chip) = chip else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    run(&chip, &reads, &loops)
}

#[cfg(all(target_os = "linux", feature = "gpio"))]
fn run(chip: &str, reads: &[String], loops: &[(String, String)]) -> ExitCode {
    use std::time::Duration;

    use helmsman_io::{Driver, Value};

    let mut board = match helmsman_io::board::linux::open(std::path::Path::new(chip)) {
        Ok(b) => b,
        Err(e) => {
            println!("FAIL  open {chip}: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("PASS  open {chip}");
    let mut failed = false;
    for r in reads {
        match board.read(&[r.as_str()]) {
            Ok(v) => println!("PASS  read {r} = {:?}", v[0]),
            Err(e) => {
                failed = true;
                println!("FAIL  read {r}: {e}");
            }
        }
    }
    for (o, i) in loops {
        let mut r = Ok(());
        for on in [true, false, true, false] {
            r = board
                .write(o, Value::Bool(on))
                .and_then(|()| {
                    std::thread::sleep(Duration::from_millis(50));
                    board.read(&[i.as_str()])
                })
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    if v[0] == Value::Bool(on) {
                        Ok(())
                    } else {
                        Err(format!("wrote {on} to {o}, {i} reads {:?}", v[0]))
                    }
                });
            if r.is_err() {
                break;
            }
        }
        let _ = board.write(o, Value::Bool(false));
        match r {
            Ok(()) => println!("PASS  loop {o} -> {i}"),
            Err(e) => {
                failed = true;
                println!("FAIL  loop {o} -> {i}: {e}");
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(not(all(target_os = "linux", feature = "gpio")))]
fn run(chip: &str, _: &[String], _: &[(String, String)]) -> ExitCode {
    eprintln!("{chip}: the board backend needs Linux and helmsman-io's `gpio` feature");
    ExitCode::from(2)
}
