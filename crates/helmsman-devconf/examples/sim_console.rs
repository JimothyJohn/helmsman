//! The simulated E1200 console as a process, for the canvas smoke
//! (`scripts/canvas-smoke-devconf.py`): prints `http=PORT modbus=PORT` on
//! stdout and serves until its stdin closes. Loopback only.

use std::io::Read;

use helmsman_devconf::moxa_e1200::sim::{Options, Settings, Sim};

fn main() {
    let mut settings = Settings::default();
    settings.safe_mode[1] = true;
    let sim = Sim::start_with(Options {
        settings,
        ..Options::default()
    });
    let http = sim.addr().rsplit(':').next().unwrap_or("0").to_string();
    println!("http={http} modbus={}", sim.modbus_port());
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
}
