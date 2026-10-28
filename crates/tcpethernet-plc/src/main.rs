//! `tcpethernet-plc`: CIP adapter + scan engine + browser programming page.

use std::fs;
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tcpethernet_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use tcpethernet_plc::engine::{Engine, EngineConfig};
use tcpethernet_plc::web::Web;

const USAGE: &str = "\
usage: tcpethernet-plc [options]
  --state-dir DIR      program, token and serial live here (default ./plc-state)
  --http ADDR:PORT     browser interface (default 0.0.0.0:8080)
  --bind IP            CIP interface address (default 0.0.0.0)
  --encap-port N       TCP/UDP encapsulation port (default 44818)
  --io-port N          local UDP I/O port (default 2222)
  --peer-io-port N     scanner's UDP I/O port, where input is sent (default 2222)
  --scan-ms N          scan period (default 10)
  --profile P          default | opener-sample (OpENer's identity, for interop tests)
  --name TEXT          product name (max 32 bytes)
Assemblies: 150 = from the scanner (I, 32 B), 100 = to the scanner (Q, 32 B),
151 = configuration (10 B), 152/153 = input-only/listen-only heartbeats.";

struct Opts {
    state_dir: PathBuf,
    http: SocketAddr,
    server: ServerConfig,
    scan: Duration,
    profile: String,
    name: Option<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        state_dir: PathBuf::from("plc-state"),
        http: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080)),
        server: ServerConfig::default(),
        scan: Duration::from_millis(10),
        profile: "default".into(),
        name: None,
    };
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "-h" || flag == "--help" {
            return Err(String::new());
        }
        let v = args.get(i + 1).ok_or_else(|| format!("{flag} needs a value"))?;
        let port = || v.parse::<u16>().map_err(|_| format!("{flag}: not a port: {v:?}"));
        match flag {
            "--state-dir" => o.state_dir = PathBuf::from(v),
            "--http" => {
                o.http = v
                    .parse()
                    .map_err(|_| format!("--http: expected ADDR:PORT, got {v:?}"))?;
            }
            "--bind" => {
                o.server.bind = v
                    .parse()
                    .map_err(|_| format!("--bind: not an IPv4 address: {v:?}"))?;
            }
            "--encap-port" => o.server.encap_port = port()?,
            "--io-port" => o.server.io_port = port()?,
            "--peer-io-port" => o.server.peer_io_port = port()?,
            "--scan-ms" => {
                let ms: u64 = v.parse().map_err(|_| format!("--scan-ms: not a number: {v:?}"))?;
                if !(1..=10_000).contains(&ms) {
                    return Err("--scan-ms must be 1..=10000".into());
                }
                o.scan = Duration::from_millis(ms);
            }
            "--profile" if v == "default" || v == "opener-sample" => o.profile.clone_from(v),
            "--profile" => return Err(format!("--profile: unknown {v:?}")),
            "--name" if v.len() <= 32 => o.name = Some(v.clone()),
            "--name" => return Err("--name: at most 32 bytes".into()),
            _ => return Err(format!("unknown option {flag}")),
        }
        i += 2;
    }
    Ok(o)
}

fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

fn write_private(path: &Path, data: &str) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    io::Write::write_all(&mut opts.open(path)?, data.as_bytes())
}

/// Token for the web API: read it, or create one and say so once.
fn token(dir: &Path) -> io::Result<String> {
    let path = dir.join("token");
    if let Ok(t) = fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            eprintln!("web token: in {}", path.display());
            return Ok(t);
        }
    }
    let t = random_bytes::<16>()?
        .iter()
        .fold(String::with_capacity(32), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });
    let _ = fs::remove_file(&path);
    write_private(&path, &t)?;
    eprintln!("web token (new, also in {}): {t}", path.display());
    Ok(t)
}

/// A per-install serial number so two PLCs on one network differ.
fn serial(dir: &Path) -> io::Result<u32> {
    let path = dir.join("serial");
    if let Some(s) = fs::read_to_string(&path)
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim(), 16).ok())
    {
        return Ok(s);
    }
    let s = u32::from_le_bytes(random_bytes::<4>()?);
    fs::write(&path, format!("{s:08x}\n"))?;
    Ok(s)
}

fn run(o: &Opts) -> Result<(), String> {
    fs::create_dir_all(&o.state_dir).map_err(|e| format!("{}: {e}", o.state_dir.display()))?;
    let mut cfg = DeviceConfig::opener_sample();
    if o.profile == "default" {
        cfg.identity.vendor_id = 0xFFFF;
        cfg.identity.device_type = 0x2B;
        cfg.identity.product_code = 1;
        cfg.identity.revision = (0, 1);
        cfg.identity.product_name = "tcpethernet PLC".into();
        cfg.identity.serial_number = serial(&o.state_dir).map_err(|e| format!("serial: {e}"))?;
    }
    if let Some(n) = &o.name {
        cfg.identity.product_name.clone_from(n);
    }
    let adapter = Adapter::start(Device::new(cfg), &o.server).map_err(|e| format!("CIP sockets: {e}"))?;
    let engine = Arc::new(Engine::start(
        adapter.device(),
        EngineConfig {
            input_assembly: 150,
            output_assembly: 100,
            memory: 64,
            period: o.scan,
            program_file: Some(o.state_dir.join("program.plc")),
        },
    )?);
    let token = token(&o.state_dir).map_err(|e| format!("token: {e}"))?;
    let adapter = Arc::new(adapter);
    let sessions = {
        let a = Arc::clone(&adapter);
        Arc::new(move || a.sessions()) as tcpethernet_plc::web::SessionCount
    };
    let web = Web::start(o.http, Arc::clone(&engine), token, sessions)
        .map_err(|e| format!("http {}: {e}", o.http))?;
    eprintln!(
        "tcpethernet-plc: CIP on {} (I/O {}), web on http://{}/, scan {} ms",
        adapter.encap_addr(),
        adapter.io_addr(),
        web.addr(),
        o.scan.as_millis()
    );
    loop {
        std::thread::park();
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
