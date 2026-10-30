//! `tcpethernet-plc`: runs IEC 61131-3 modules behind the CIP adapter:
//! Structured Text, or `PLCopen` XML projects (LD, FBD, ST) exported from an
//! editor such as Beremiz or the `OpenPLC` Editor. Assembly 150 (what the
//! scanner sends) is the input image, assembly 100 (what it receives) the
//! output image; each module scans at its own rate against its own windows
//! of them (see [`tcpethernet_plc::runtime`]). A module that faults has its
//! outputs zeroed and stops; the rest keep running, and the adapter stays on
//! the network so the PLC can still be found.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use tcpethernet_adapter::device::lock_ref;
use tcpethernet_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use tcpethernet_iec::{ImageSizes, compile, compile_plcopen, plcopen};
use tcpethernet_plc::spec::{Defaults, Spec};
use tcpethernet_plc::{Module, Runtime, Window};

const USAGE: &str = "\
usage: tcpethernet-plc [PROGRAM.st|PROJECT.xml ...] [--module SPEC ...] [options]
  --module SPEC        PATH[,name=N][,scan=MS][,i=START+LEN][,q=START+LEN][,m=BYTES]
                       one module; unset keys take the defaults below. A bare
                       PROGRAM argument is a module with every default.
  --emit-st            print the Structured Text a .xml project lowers to, and exit
  --scan-ms N          default scan period (10, 1..=10000)
  --memory N           default %M area in bytes (1024)
  --serial HEX         identity serial number (default 1)
  --bind IP            CIP interface address (default 0.0.0.0)
  --encap-port N       TCP/UDP encapsulation port (default 44818, 0 = any)
  --io-port N          local UDP I/O port (default 2222, 0 = any)
  --peer-io-port N     scanner's UDP I/O port (default 2222)
Process image: %I = assembly 150 (32 B from the scanner), %Q = assembly
100 (32 B to the scanner); a module's windows default to all 32 B of each,
and no two modules may share an output byte. Connection points as OpENer's
sample: config 151, input-only 152, listen-only 153.";

/// Assembly the scanner writes (O->T) and the program reads as `%I`.
const INPUTS: u16 = 150;
/// Assembly the program writes as `%Q` and the scanner reads (T->O).
const OUTPUTS: u16 = 100;

struct Opts {
    programs: Vec<PathBuf>,
    modules: Vec<String>,
    emit_st: bool,
    scan: Duration,
    memory: usize,
    serial: u32,
    server: ServerConfig,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        programs: Vec::new(),
        modules: Vec::new(),
        emit_st: false,
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
        if flag == "--emit-st" {
            o.emit_st = true;
            i += 1;
            continue;
        }
        if !flag.starts_with("--") {
            o.programs.push(PathBuf::from(flag));
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
            "--module" => o.modules.push(v.clone()),
            "--encap-port" => o.server.encap_port = port()?,
            "--io-port" => o.server.io_port = port()?,
            "--peer-io-port" => o.server.peer_io_port = port()?,
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    if o.programs.is_empty() && o.modules.is_empty() {
        return Err("missing PROGRAM.st, PROJECT.xml or --module".into());
    }
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

fn is_xml(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml"))
}

fn specs(o: &Opts, cfg: &DeviceConfig) -> Result<Vec<Spec>, String> {
    let d = Defaults {
        period: o.scan,
        i: Window::new(0, assembly_size(cfg, INPUTS)),
        q: Window::new(0, assembly_size(cfg, OUTPUTS)),
        m: o.memory,
    };
    let mut specs: Vec<Spec> = o.programs.iter().map(|p| Spec::from_path(p, &d)).collect();
    for m in &o.modules {
        specs.push(Spec::parse(m, &d)?);
    }
    Ok(specs)
}

fn load(spec: &Spec) -> Result<Module, String> {
    let file = spec.path.display();
    let src = fs::read_to_string(&spec.path).map_err(|e| format!("{file}: {e}"))?;
    let sizes = ImageSizes {
        i: spec.i.len,
        q: spec.q.len,
        m: spec.m,
    };
    let compiled = if is_xml(&spec.path) {
        compile_plcopen(&src, sizes)
    } else {
        compile(&src, sizes)
    };
    let program = compiled.map_err(|diags| {
        let lines: Vec<String> = diags.iter().map(|d| format!("\n  {file}:{d}")).collect();
        format!("{file} does not compile:{}", lines.concat())
    })?;
    Ok(Module {
        name: spec.name.clone(),
        program,
        period: spec.period,
        i: spec.i,
        q: spec.q,
    })
}

fn emit_st(o: &Opts) -> Result<(), String> {
    let [path] = o.programs.as_slice() else {
        return Err("--emit-st takes exactly one .xml project".into());
    };
    if !is_xml(path) || !o.modules.is_empty() {
        return Err("--emit-st takes exactly one .xml project".into());
    }
    let src = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lowered = plcopen::lower(&src).map_err(|diags| {
        let file = path.display();
        let lines: Vec<String> = diags.iter().map(|d| format!("\n  {file}:{d}")).collect();
        format!("{file} does not lower:{}", lines.concat())
    })?;
    print!("{}", lowered.st());
    Ok(())
}

fn run(o: &Opts) -> Result<(), String> {
    if o.emit_st {
        return emit_st(o);
    }
    let cfg = device_config(o.serial);
    let specs = specs(o, &cfg)?;
    let modules = specs.iter().map(load).collect::<Result<Vec<_>, _>>()?;
    let (i_len, q_len) = (assembly_size(&cfg, INPUTS), assembly_size(&cfg, OUTPUTS));
    let mut rt = Runtime::new(i_len, q_len, modules).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {e}")).collect();
        format!("modules do not fit together:{}", lines.concat())
    })?;
    let adapter = Adapter::start(Device::new(cfg), &o.server).map_err(|e| format!("CIP sockets: {e}"))?;
    let listed: Vec<String> = specs
        .iter()
        .map(|s| {
            format!(
                "{} from {} every {} ms, %I {}+{}, %Q {}+{}",
                s.name,
                s.path.display(),
                s.period.as_millis(),
                s.i.start,
                s.i.len,
                s.q.start,
                s.q.len
            )
        })
        .collect();
    eprintln!(
        "tcpethernet-plc: CIP on {} (I/O {}), {} module(s): {}",
        adapter.encap_addr(),
        adapter.io_addr(),
        specs.len(),
        listed.join("; ")
    );
    let dev = adapter.device();
    let mut inputs = vec![0; i_len];
    let start = Instant::now();
    loop {
        {
            let d = lock_ref(&dev);
            if let Some(src) = d.assembly(INPUTS) {
                inputs.clear();
                inputs.extend_from_slice(src);
            }
        }
        for (name, fault) in rt.tick(start.elapsed(), &inputs) {
            let path = specs.iter().find(|s| s.name == name).map(|s| s.path.display());
            eprintln!(
                "tcpethernet-plc: STOP, outputs zeroed: {}: {fault} (module {name})",
                path.map_or_else(String::new, |p| p.to_string())
            );
        }
        lock_ref(&dev).write_assembly(OUTPUTS, rt.outputs())?;
        let Some(next) = rt.next_deadline() else {
            eprintln!("tcpethernet-plc: every module has stopped; the adapter stays up");
            loop {
                thread::park();
            }
        };
        // A late tick skips missed periods (the runtime counts them), so
        // sleeping to the next deadline never bursts.
        if let Some(wait) = (start + next).checked_duration_since(Instant::now()) {
            thread::sleep(wait);
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
