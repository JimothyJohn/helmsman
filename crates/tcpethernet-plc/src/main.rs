//! `tcpethernet-plc`: runs one IEC 61131-3 Structured Text program behind
//! the CIP adapter. Each scan copies assembly 150 (what the scanner sends)
//! into `%I`, runs the program, and copies `%Q` into assembly 100 (what the
//! scanner receives). A runtime fault zeroes the outputs and stops the
//! program; the adapter stays on the network so the PLC can still be found.

use std::fs;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use tcpethernet_adapter::device::lock_ref;
use tcpethernet_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use tcpethernet_iec::{Image, ImageSizes, compile};

const USAGE: &str = "\
usage: tcpethernet-plc PROGRAM.st [options]
  --scan-ms N          scan period (default 10, 1..=10000)
  --memory N           %M area in bytes (default 1024)
  --serial HEX         identity serial number (default 1)
  --bind IP            CIP interface address (default 0.0.0.0)
  --encap-port N       TCP/UDP encapsulation port (default 44818, 0 = any)
  --io-port N          local UDP I/O port (default 2222, 0 = any)
  --peer-io-port N     scanner's UDP I/O port (default 2222)
Process image: %I = assembly 150 (32 B from the scanner), %Q = assembly
100 (32 B to the scanner). Connection points as OpENer's sample: config 151,
input-only 152, listen-only 153.";

/// Assembly the scanner writes (O->T) and the program reads as `%I`.
const INPUTS: u16 = 150;
/// Assembly the program writes as `%Q` and the scanner reads (T->O).
const OUTPUTS: u16 = 100;

struct Opts {
    program: PathBuf,
    scan: Duration,
    memory: usize,
    serial: u32,
    server: ServerConfig,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut program = None;
    let mut o = Opts {
        program: PathBuf::new(),
        scan: Duration::from_millis(10),
        memory: 1024,
        serial: 1,
        server: ServerConfig::default(),
    };
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "-h" || flag == "--help" {
            return Err(String::new());
        }
        if !flag.starts_with("--") {
            if program.replace(PathBuf::from(flag)).is_some() {
                return Err("one program file only".into());
            }
            i += 1;
            continue;
        }
        let v = args.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
        let port = || v.parse::<u16>().map_err(|_| format!("{flag}: not a port: {v:?}"));
        match flag {
            "--scan-ms" => match v.parse::<u64>() {
                Ok(ms @ 1..=10_000) => o.scan = Duration::from_millis(ms),
                _ => return Err(format!("--scan-ms: expected 1..=10000, got {v:?}")),
            },
            "--memory" => match v.parse::<usize>() {
                Ok(n @ 0..=65_536) => o.memory = n,
                _ => return Err(format!("--memory: expected 0..=65536 bytes, got {v:?}")),
            },
            "--serial" => {
                o.serial = u32::from_str_radix(v.trim_start_matches("0x"), 16)
                    .map_err(|_| format!("--serial: expected hex, got {v:?}"))?;
            }
            "--bind" => {
                o.server.bind = v
                    .parse::<Ipv4Addr>()
                    .map_err(|_| format!("--bind: not an IPv4 address: {v:?}"))?;
            }
            "--encap-port" => o.server.encap_port = port()?,
            "--io-port" => o.server.io_port = port()?,
            "--peer-io-port" => o.server.peer_io_port = port()?,
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    o.program = program.ok_or("missing PROGRAM.st")?;
    Ok(o)
}

fn device_config(serial: u32) -> DeviceConfig {
    let mut cfg = DeviceConfig::opener_sample();
    cfg.identity.vendor_id = 0xFFFF;
    cfg.identity.device_type = 0x0E; // Programmable Logic Controller
    cfg.identity.product_code = 1;
    cfg.identity.revision = (0, 1);
    cfg.identity.serial_number = serial;
    cfg.identity.product_name = "tcpethernet PLC".into();
    cfg
}

fn assembly_size(cfg: &DeviceConfig, instance: u16) -> usize {
    cfg.assemblies
        .iter()
        .find(|(i, _)| *i == instance)
        .map_or(0, |(_, n)| *n)
}

fn run(o: &Opts) -> Result<(), String> {
    let src = fs::read_to_string(&o.program).map_err(|e| format!("{}: {e}", o.program.display()))?;
    let cfg = device_config(o.serial);
    let sizes = ImageSizes {
        i: assembly_size(&cfg, INPUTS),
        q: assembly_size(&cfg, OUTPUTS),
        m: o.memory,
    };
    let mut program = compile(&src, sizes).map_err(|diags| {
        let file = o.program.display();
        let lines: Vec<String> = diags.iter().map(|d| format!("\n  {file}:{d}")).collect();
        format!("{file} does not compile:{}", lines.concat())
    })?;
    let adapter = Adapter::start(Device::new(cfg), &o.server).map_err(|e| format!("CIP sockets: {e}"))?;
    eprintln!(
        "tcpethernet-plc: CIP on {} (I/O {}), program {} from {}, scan {} ms",
        adapter.encap_addr(),
        adapter.io_addr(),
        program.name(),
        o.program.display(),
        o.scan.as_millis()
    );
    let dev = adapter.device();
    let mut image = Image::new(sizes);
    let mut next = Instant::now();
    loop {
        {
            let d = lock_ref(&dev);
            if let Some(src) = d.assembly(INPUTS) {
                image.i.copy_from_slice(src);
            }
        }
        match program.scan(&mut image) {
            Ok(()) => lock_ref(&dev).write_assembly(OUTPUTS, &image.q)?,
            Err(fault) => {
                lock_ref(&dev).write_assembly(OUTPUTS, &vec![0; sizes.q])?;
                eprintln!(
                    "tcpethernet-plc: STOP, outputs zeroed: {}: {fault}",
                    o.program.display()
                );
                loop {
                    thread::park();
                }
            }
        }
        next += o.scan;
        let now = Instant::now();
        if next > now {
            thread::sleep(next - now);
        } else {
            // Overran: start the next period from now rather than bursting.
            next = now;
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse(&args) {
        Ok(o) => o,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
