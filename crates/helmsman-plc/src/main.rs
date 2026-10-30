//! `helmsman-plc`: runs IEC 61131-3 modules behind the CIP adapter:
//! Structured Text, or `PLCopen` XML projects (LD, FBD, ST) exported from an
//! editor such as Beremiz or the `OpenPLC` Editor. Assembly 150 (what the
//! scanner sends) is the input image, assembly 100 (what it receives) the
//! output image; each module scans at its own rate against its own windows
//! of them (see [`helmsman_plc::runtime`]). A module that faults has its
//! outputs zeroed and stops; the rest keep running, and the adapter stays on
//! the network so the PLC can still be found.

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use helmsman_adapter::device::lock_ref;
use helmsman_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use helmsman_iec::{ImageSizes, compile, compile_plcopen, plcopen};
use helmsman_io::local::Channels;
use helmsman_plc::calendar::{self, Source};
use helmsman_plc::link::{Link, links, spawn};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::spec::{Defaults, Spec};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};

const USAGE: &str = "\
usage: helmsman-plc [PROGRAM.st|PROJECT.xml ...] [--module SPEC ...] [--module-dir DIR ...] [options]
  --module SPEC        PATH[,name=N][,scan=MS][,i=START+LEN][,q=START+LEN][,m=BYTES]
                       one module; unset keys take the defaults below. A bare
                       PROGRAM argument is a module with every default.
  --module-dir DIR     one module as a directory: module.st and module.manifest
                       (see helmsman_plc::manifest), checked against each other
  --io-map FILE        devices and named signals (see helmsman_io::text); a
                       module's VAR_INPUT/VAR_OUTPUT bind to signals by name
  --calendar FILE      shifts, breaks, planned downtime (see
                       helmsman_plc::calendar::text): adds the signals in_shift,
                       on_break, planned_down, working, shift_id, shift_minute,
                       shift_left, and drives the outputs its `drive` lines name
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
    module_dirs: Vec<PathBuf>,
    io_map: Option<PathBuf>,
    calendar: Option<PathBuf>,
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
        module_dirs: Vec::new(),
        io_map: None,
        calendar: None,
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
            "--module-dir" => o.module_dirs.push(PathBuf::from(v)),
            "--io-map" => {
                if o.io_map.replace(PathBuf::from(v)).is_some() {
                    return Err("one --io-map only".into());
                }
            }
            "--calendar" => {
                if o.calendar.replace(PathBuf::from(v)).is_some() {
                    return Err("one --calendar only".into());
                }
            }
            "--encap-port" => o.server.encap_port = port()?,
            "--io-port" => o.server.io_port = port()?,
            "--peer-io-port" => o.server.peer_io_port = port()?,
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    if o.programs.is_empty() && o.modules.is_empty() && o.module_dirs.is_empty() {
        return Err("missing PROGRAM.st, PROJECT.xml, --module or --module-dir".into());
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
    cfg.identity.product_name = "Helmsman PLC".into();
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

/// Each module's spec, and its directory when it came from one.
fn specs(o: &Opts, cfg: &DeviceConfig) -> Result<Vec<(Spec, Option<ModuleDir>)>, String> {
    let d = Defaults {
        period: o.scan,
        i: Window::new(0, assembly_size(cfg, INPUTS)),
        q: Window::new(0, assembly_size(cfg, OUTPUTS)),
        m: o.memory,
    };
    let mut specs: Vec<(Spec, Option<ModuleDir>)> = o
        .programs
        .iter()
        .map(|p| (Spec::from_path(p, &d), None))
        .collect();
    for m in &o.modules {
        specs.push((Spec::parse(m, &d)?, None));
    }
    for dir in &o.module_dirs {
        let md = ModuleDir::read(dir, &d)?;
        specs.push((md.spec.clone(), Some(md)));
    }
    Ok(specs)
}

fn load((spec, dir): &(Spec, Option<ModuleDir>)) -> Result<Module, String> {
    let file = spec.path.display();
    let src = match dir {
        Some(md) => md.source.clone(),
        None => fs::read_to_string(&spec.path).map_err(|e| format!("{file}: {e}"))?,
    };
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
    if let Some(md) = dir {
        return md.module(program);
    }
    Ok(Module {
        name: spec.name.clone(),
        program,
        period: spec.period,
        i: spec.i,
        q: spec.q,
        params: Vec::new(),
    })
}

fn emit_st(o: &Opts) -> Result<(), String> {
    let [path] = o.programs.as_slice() else {
        return Err("--emit-st takes exactly one .xml project".into());
    };
    if !is_xml(path) || !o.modules.is_empty() || !o.module_dirs.is_empty() {
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

/// The I/O map's signal table and its links, not yet started, and whether
/// any signal is on the box's own channels.
fn load_io(path: &Path) -> Result<(SignalTable, Vec<Link>, bool), String> {
    let file = path.display();
    let src = fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
    let parsed = helmsman_io::text::parse(&src).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|(n, e)| format!("\n  {file}:{n}: {e}")).collect();
        format!("{file} does not parse:{}", lines.concat())
    })?;
    let table = SignalTable::new(&parsed.map, &parsed.safe).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} is not a valid map:{}", lines.concat())
    })?;
    let uses_box = parsed
        .map
        .signals
        .iter()
        .any(|s| matches!(s.binding, helmsman_io::Binding::Local { .. }));
    let links = links(&parsed.map, helmsman_io::open, Box::new(Channels::new()))?;
    Ok((table, links, uses_box))
}

/// The calendar file, bound to `table`.
fn load_calendar(path: &Path, table: &mut SignalTable) -> Result<Source, String> {
    let file = path.display();
    let src = fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
    let cal = calendar::load(&src).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} is not a valid calendar:{}", lines.concat())
    })?;
    Source::attach(cal, table).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} does not fit the I/O map:{}", lines.concat())
    })
}

/// [`load_io`] (or no signals), with the calendar's added.
fn load_signals(o: &Opts) -> Result<(SignalTable, Vec<Link>, bool, Option<Source>), String> {
    let (mut table, links, uses_box) = match &o.io_map {
        Some(path) => load_io(path)?,
        None => (SignalTable::default(), Vec::new(), false),
    };
    let cal = o
        .calendar
        .as_deref()
        .map(|p| load_calendar(p, &mut table))
        .transpose()?;
    Ok((table, links, uses_box, cal))
}

/// Wall-clock seconds since the epoch, or `None` before it.
fn wall() -> Option<i64> {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

/// How long the loop may sleep with a calendar to keep current.
const CALENDAR_TICK: Duration = Duration::from_secs(1);

fn report_calendar(c: &Source) {
    let k = c.calendar();
    eprintln!(
        "helmsman-plc: calendar: {} shift pattern(s), {} drive(s); clock {}",
        k.shifts.len(),
        k.drives.len(),
        if c.state(wall()).is_some() {
            "trusted"
        } else {
            "NOT SET: calendar signals missing, driven outputs safe"
        }
    );
}

/// Sleeps to the runtime's `next` deadline (from `start`). A late tick
/// skips missed periods (the runtime counts them), so this never bursts.
/// With a calendar it wakes at least every [`CALENDAR_TICK`] so the
/// calendar's signals and outputs stay current, even once every module has
/// stopped; without one, a runtime with nothing left to run parks for good.
fn sleep_to(start: Instant, next: Option<Duration>, calendar: bool) {
    let next = match next {
        Some(next) => next,
        None if !calendar => {
            eprintln!("helmsman-plc: every module has stopped; the adapter stays up");
            loop {
                thread::park();
            }
        }
        None => start.elapsed() + CALENDAR_TICK,
    };
    let wait = (start + next).checked_duration_since(Instant::now());
    if let Some(wait) = wait.map(|w| if calendar { w.min(CALENDAR_TICK) } else { w }) {
        thread::sleep(wait);
    }
}

/// Runs each link on its own thread for the life of the process.
fn start_io(links: Vec<Link>, table: &Arc<Mutex<SignalTable>>) {
    let stop = Arc::new(AtomicBool::new(false));
    for link in links {
        spawn(
            link,
            Arc::clone(table),
            Arc::clone(&stop),
            |name, err| match err {
                None => eprintln!("helmsman-plc: device {name} up"),
                Some(e) => eprintln!("helmsman-plc: device {name} down: {e}"),
            },
        );
    }
}

fn run(o: &Opts) -> Result<(), String> {
    if o.emit_st {
        return emit_st(o);
    }
    let cfg = device_config(o.serial);
    let loaded = specs(o, &cfg)?;
    let modules = loaded.iter().map(load).collect::<Result<Vec<_>, _>>()?;
    let specs: Vec<Spec> = loaded.into_iter().map(|(s, _)| s).collect();
    let (i_len, q_len) = (assembly_size(&cfg, INPUTS), assembly_size(&cfg, OUTPUTS));
    let (table, io_links, uses_box, cal) = load_signals(o)?;
    let table = Arc::new(Mutex::new(table));
    let bound = Runtime::with_signals(
        i_len,
        q_len,
        modules,
        &table.lock().unwrap_or_else(PoisonError::into_inner),
    );
    let mut rt = bound.map_err(|errs| {
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
        "helmsman-plc: CIP on {} (I/O {}), {} module(s): {}",
        adapter.encap_addr(),
        adapter.io_addr(),
        specs.len(),
        listed.join("; ")
    );
    if uses_box {
        eprintln!("helmsman-plc: box channels (do:N, di:N) are in memory only; no GPIO yet");
    }
    if let Some(c) = &cal {
        report_calendar(c);
    }
    start_io(io_links, &table);
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
        let held: Vec<bool> = specs
            .iter()
            .map(|s| rt.status(&s.name) == Some(Status::Held))
            .collect();
        let faults = {
            let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(c) = &cal {
                c.update(wall(), &mut t);
            }
            rt.tick_with(start.elapsed(), &inputs, &mut t)
        };
        for (s, was) in specs.iter().zip(held) {
            match (was, rt.status(&s.name)) {
                (false, Some(Status::Held)) => eprintln!(
                    "helmsman-plc: HOLD, outputs safe: a signal module {} reads has no value",
                    s.name
                ),
                (true, Some(Status::Running)) => {
                    eprintln!("helmsman-plc: module {} running again", s.name);
                }
                _ => {}
            }
        }
        for (name, fault) in faults {
            let path = specs.iter().find(|s| s.name == name).map(|s| s.path.display());
            eprintln!(
                "helmsman-plc: STOP, outputs zeroed: {}: {fault} (module {name})",
                path.map_or_else(String::new, |p| p.to_string())
            );
        }
        lock_ref(&dev).write_assembly(OUTPUTS, rt.outputs())?;
        sleep_to(start, rt.next_deadline(), cal.is_some());
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
