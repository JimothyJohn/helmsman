//! `helmsman-adapter`: this repo's adapter as a stand-alone interop
//! target. By default the same identity, assemblies and behaviour as
//! `OpENer`'s POSIX sample application: whatever the scanner writes to
//! assembly 150 comes back in assembly 100, so the interop suite runs
//! unchanged against either. `--profile moxa-e1213` is a simulated Moxa
//! ioLogik E1213 instead (`helmsman_adapter::e1213`), DO-00..03 wired to
//! DI-00..03.

use std::net::Ipv4Addr;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use helmsman_adapter::device::lock_ref;
use helmsman_adapter::{Adapter, Device, DeviceConfig, ServerConfig};

const USAGE: &str = "\
usage: helmsman-adapter [options]
  --bind IP            CIP interface address (default 0.0.0.0)
  --encap-port N       TCP/UDP encapsulation port (default 44818, 0 = any)
  --io-port N          local UDP I/O port (default 2222, 0 = any)
  --peer-io-port N     scanner's UDP I/O port, where input is sent (default 2222;
                       0 = the port its output packets come from)
  --profile NAME       opener (default) or moxa-e1213
OpENer sample profile: 150 = from the scanner (32 B), 100 = to the scanner
(32 B, a copy of 150), 151 = configuration (10 B), 152/153 = heartbeats.
moxa-e1213: 102 = DO byte from the scanner, 101 = 51 B of DI bits, counters
and DO status, 100 = configuration (none), 111 = input-only heartbeat.";

/// `OpENer`'s sample application copies the output assembly into the input
/// assembly after each O->T packet; polling at 1 ms is close enough for any
/// RPI the sample accepts.
const ECHO_FROM: u16 = 150;
const ECHO_TO: u16 = 100;
const ECHO_PERIOD: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Opener,
    MoxaE1213,
}

fn parse(args: &[String]) -> Result<(ServerConfig, Profile), String> {
    let mut cfg = ServerConfig::default();
    let mut profile = Profile::Opener;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "-h" || flag == "--help" {
            return Err(String::new());
        }
        let v = args.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
        let port = || v.parse::<u16>().map_err(|_| format!("{flag}: not a port: {v:?}"));
        match flag {
            "--bind" => {
                cfg.bind = v
                    .parse::<Ipv4Addr>()
                    .map_err(|_| format!("--bind: not an IPv4 address: {v:?}"))?;
            }
            "--encap-port" => cfg.encap_port = port()?,
            "--io-port" => cfg.io_port = port()?,
            "--peer-io-port" => cfg.peer_io_port = port()?,
            "--profile" => {
                profile = match v.as_str() {
                    "opener" => Profile::Opener,
                    "moxa-e1213" => Profile::MoxaE1213,
                    other => return Err(format!("--profile: opener or moxa-e1213, not {other:?}")),
                };
            }
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    Ok((cfg, profile))
}

fn run(cfg: &ServerConfig, profile: Profile) -> Result<(), String> {
    let device = match profile {
        Profile::Opener => DeviceConfig::opener_sample(),
        Profile::MoxaE1213 => DeviceConfig::moxa_e1213(),
    };
    let adapter = Adapter::start(Device::new(device), cfg)
        .map_err(|e| format!("CIP sockets: {e}"))?;
    eprintln!(
        "helmsman-adapter: CIP on {} (I/O {})",
        adapter.encap_addr(),
        adapter.io_addr()
    );
    let dev = adapter.device();
    let mut e1213 = helmsman_adapter::e1213::Application::bench();
    loop {
        if profile == Profile::MoxaE1213 {
            e1213.step(&mut lock_ref(&dev));
            thread::sleep(ECHO_PERIOD);
            continue;
        }
        {
            let mut d = lock_ref(&dev);
            if let Some(data) = d.assembly(ECHO_FROM).map(<[u8]>::to_vec)
                && d.assembly(ECHO_TO) != Some(&data[..])
            {
                d.write_assembly(ECHO_TO, &data)?;
            }
        }
        thread::sleep(ECHO_PERIOD);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cfg, profile) = match parse(&args) {
        Ok(c) => c,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&cfg, profile) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
