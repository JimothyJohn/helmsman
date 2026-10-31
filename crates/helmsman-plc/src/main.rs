//! `helmsman-plc`: runs IEC 61131-3 modules behind the CIP adapter:
//! Structured Text, or `PLCopen` XML projects (LD, FBD, ST) exported from an
//! editor such as Beremiz or the `OpenPLC` Editor. Assembly 150 (what the
//! scanner sends) is the input image, assembly 100 (what it receives) the
//! output image; each module scans at its own rate against its own windows
//! of them (see [`helmsman_plc::runtime`]). A module that faults has its
//! outputs zeroed and stops; the rest keep running, and the adapter stays on
//! the network so the PLC can still be found.

use std::fs;
use std::io::BufRead as _;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use helmsman_adapter::IoEndpoint;
use helmsman_adapter::device::lock_ref;
use helmsman_adapter::{Adapter, Device, DeviceConfig, ServerConfig};
use helmsman_agent::relay;
use helmsman_iec::{ImageSizes, compile, compile_plcopen, plcopen};
use helmsman_io::Driver;
use helmsman_io::local::Channels;
use helmsman_plc::api::swap::Scene;
use helmsman_plc::api::{
    self, Api, Approval, Catalog, Devices, Live, MemAudit, MemProposals, ModuleInfo, Plant, Publisher,
    Swapper,
};
use helmsman_plc::bundle::TrustStore;
use helmsman_plc::calendar::{self, Source};
use helmsman_plc::force::{self, Forces};
use helmsman_plc::link::{LinkSet, Plan};
use helmsman_plc::manifest::ModuleDir;
use helmsman_plc::opcua;
use helmsman_plc::policy::account::{self, Accounts, Logins};
use helmsman_plc::policy::pin::{self, Gate, Pins};
use helmsman_plc::policy::{Policy, Role};
use helmsman_plc::record::{RecordConfig, Recorder};
use helmsman_plc::sink::{self, Sinks};
use helmsman_plc::site::{self, Site};
use helmsman_plc::spec::{Defaults, Spec};
use helmsman_plc::store::{ChangeSet, Code, Kind, Store};
use helmsman_plc::{Module, Runtime, SignalTable, Status, Window};

const USAGE: &str = "\
usage: helmsman-plc [PROGRAM.st|PROJECT.xml ...] [--module SPEC ...] [--module-dir DIR ...] [options]
       helmsman-plc bundle COMMAND ...   (signed bundles: `helmsman-plc bundle help`)
       helmsman-plc account add USER ROLE | passwd USER | disable USER | enable USER | list
                    [--state-dir DIR]   (per-person logins; passwords on stdin. Once
                    any account exists, approvers log in and role PINs are refused)
  --module SPEC        PATH[,name=N][,scan=MS][,i=START+LEN][,q=START+LEN][,m=BYTES]
                       one module; unset keys take the defaults below. A bare
                       PROGRAM argument is a module with every default.
  --module-dir DIR     one module as a directory: module.st and module.manifest
                       (see helmsman_plc::manifest), checked against each other
  --io-map FILE        devices and named signals (see helmsman_io::text); a
                       module's VAR_INPUT/VAR_OUTPUT bind to signals by name
  --gpio-chip PATH     the box's own channels on this board (Linux): di:N and
                       do:N are lines N of PATH (/dev/gpiochip4 on a Pi 5
                       before kernel 6.6.45, /dev/gpiochip0 after), and
                       i2c:BUS:... reads /dev/i2c-BUS. Default: in memory
  --calendar FILE      shifts, breaks, planned downtime (see
                       helmsman_plc::calendar::text): adds the signals in_shift,
                       on_break, planned_down, working, shift_id, shift_minute,
                       shift_left, and drives the outputs its `drive` lines name
  --sinks FILE         webhook sinks and the signals they publish (see
                       helmsman_plc::sink); needs --sink-dir
  --sink-dir DIR       where each sink keeps its on-disk queue (state, not config)
  --store DIR          run the head snapshot of the store in DIR (see
                       helmsman_plc::store): its modules, I/O map and calendar,
                       instead of the files above
  --projects           the browser editor's projects, in the state directory's
                       projects/ (see helmsman_plc::project): deploying one
                       commits it to the store (--store DIR, default the
                       state directory's store/) and hot-swaps it; the box
                       boots the store's head, or nothing until a first deploy.
                       Takes no PROGRAM, --module, --module-dir, --io-map or
                       --calendar
  --emit-st           print the Structured Text a .xml project lowers to, and exit
  --api ADDR           box API (HTTP + JSON, see helmsman_plc::api) on ADDR
                       (default 127.0.0.1:8080: loopback only; 0.0.0.0:8080
                       for the LAN), or `off`. Reads are open; anything
                       else needs the token from the state directory.
  --opcua ADDR         read-only OPC UA server (see helmsman_plc::opcua) on
                       ADDR (default 127.0.0.1:4840: loopback only), or `off`.
                       Anonymous, SecurityPolicy None: nothing is signed or
                       encrypted, so bind it to the plant LAN only.
  --relay URL          the relay the chat panel's agent talks to (POST
                       /api/chat), e.g. https://relay.example.com; without
                       it the agent is offline and /api/chat answers 503
  --relay-token FILE   the relay's per-box token (hmr1_...); needs --relay.
                       The box never holds an Anthropic key.
  --relay-agent V      the agent version to ask the relay for (default v3)
  --relay-explain-agent V
                       the agent version POST /api/explain asks for (an
                       explain profile; default explain-v1). The box runs
                       only read-only tools for it either way.
  --screen FILE        the floor screen's layout (see helmsman_plc::screen):
                       tiles showing the I/O map's `screen` outputs, served
                       at /api/screen and drawn by the canvas at /?andon
  --site NAME          announce this box on the LAN as part of site NAME
                       (a UDP multicast beacon to 239.255.72.77:41910, TTL
                       1, every 5 s) and list the boxes of that site it
                       hears at /api/site, which other boxes' canvases may
                       read cross-origin; the canvas gets a Site view. Off
                       by default; needs a fixed --api port. A box only
                       lists what it hears, it never acts on it
  --box-name NAME      this box's name in its site (default: the host
                       name, else box-<id>)
  --ui DIR             serve the canvas (`make canvas-web` builds it) from DIR
                       at / on the API's address: a browser on the LAN is
                       the HMI (read-only, like the API's reads)
  --state-dir DIR      where the API token, the role PINs and the trusted
                       bundle publishers (trusted-publishers) live (default
                       $STATE_DIRECTORY, as systemd's StateDirectory= sets
                       it, else ./helmsman-state)
  --policy FILE        approval policy (see helmsman_plc::policy); stricter
                       than the built-in floor or refused. Default: the floor
  --set-pin ROLE       read a PIN for operator, engineer or integrator from
                       stdin (one line), store its hash in the state
                       directory, and exit. Applying a change through the API
                       takes the token and a PIN of a role the policy allows
  --scan-ms N          default scan period (10, 1..=10000)
  --memory N           default %M area in bytes (1024)
  --serial HEX         identity serial number (default 1)
  --bind IP            CIP interface address (default 0.0.0.0)
  --encap-port N       TCP/UDP encapsulation port (default 44818, 0 = any)
  --io-port N          the box's Class 1 I/O port, shared by its adapter and
                       its enip-io links (default 2222, 0 = any)
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
    gpio_chip: Option<PathBuf>,
    calendar: Option<PathBuf>,
    sinks: Option<PathBuf>,
    sink_dir: Option<PathBuf>,
    store: Option<PathBuf>,
    projects: bool,
    emit_st: bool,
    api: Option<SocketAddr>,
    opcua: Option<SocketAddr>,
    state_dir: Option<PathBuf>,
    relay: Option<String>,
    relay_token: Option<PathBuf>,
    relay_agent: String,
    relay_explain_agent: Option<String>,
    ui: Option<PathBuf>,
    screen: Option<PathBuf>,
    site: Option<String>,
    box_name: Option<String>,
    policy: Option<PathBuf>,
    set_pin: Option<Role>,
    scan: Duration,
    memory: usize,
    serial: u32,
    server: ServerConfig,
}

/// Sets a flag that may be given once.
fn once(slot: &mut Option<PathBuf>, flag: &str, v: &str) -> Result<(), String> {
    if slot.replace(PathBuf::from(v)).is_some() {
        return Err(format!("one {flag} only"));
    }
    Ok(())
}

/// `--relay`, `--relay-token`, `--relay-agent`, `--relay-explain-agent`.
fn relay_flag(o: &mut Opts, flag: &str, v: &str) -> Result<(), String> {
    match flag {
        "--relay" => {
            relay::Url::parse(v).map_err(|e| format!("--relay: {e}"))?;
            o.relay = Some(v.to_string());
        }
        "--relay-token" => once(&mut o.relay_token, flag, v)?,
        _ if !relay::version_ok(v) => return Err(format!("{flag}: not an agent version: {v:?}")),
        "--relay-explain-agent" => o.relay_explain_agent = Some(v.to_string()),
        _ => v.clone_into(&mut o.relay_agent),
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        programs: Vec::new(),
        modules: Vec::new(),
        module_dirs: Vec::new(),
        io_map: None,
        gpio_chip: None,
        calendar: None,
        sinks: None,
        sink_dir: None,
        store: None,
        projects: false,
        emit_st: false,
        api: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 8080))),
        opcua: Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 4840))),
        state_dir: None,
        relay: None,
        relay_token: None,
        relay_agent: "v3".into(),
        relay_explain_agent: None,
        ui: None,
        screen: None,
        site: None,
        box_name: None,
        policy: None,
        set_pin: None,
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
        if flag == "--projects" {
            o.projects = true;
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
            "--api" if v == "off" => o.api = None,
            "--opcua" if v == "off" => o.opcua = None,
            "--opcua" => {
                o.opcua = Some(
                    v.parse::<SocketAddr>()
                        .map_err(|_| format!("--opcua: expected IP:PORT or off, got {v:?}"))?,
                );
            }
            "--api" => {
                o.api = Some(
                    v.parse::<SocketAddr>()
                        .map_err(|_| format!("--api: expected IP:PORT or off, got {v:?}"))?,
                );
            }
            "--state-dir" => o.state_dir = Some(PathBuf::from(v)),
            "--relay" | "--relay-token" | "--relay-agent" | "--relay-explain-agent" => {
                relay_flag(&mut o, flag, v)?;
            }
            "--policy" => once(&mut o.policy, flag, v)?,
            "--set-pin" => {
                o.set_pin = Some(Role::parse(v).ok_or_else(|| {
                    format!("--set-pin: expected operator, engineer or integrator, got {v:?}")
                })?);
            }
            "--module" => o.modules.push(v.clone()),
            "--module-dir" => o.module_dirs.push(PathBuf::from(v)),
            "--io-map" => once(&mut o.io_map, flag, v)?,
            "--gpio-chip" => once(&mut o.gpio_chip, flag, v)?,
            "--calendar" => once(&mut o.calendar, flag, v)?,
            "--sinks" => once(&mut o.sinks, flag, v)?,
            "--sink-dir" => once(&mut o.sink_dir, flag, v)?,
            "--store" => once(&mut o.store, flag, v)?,
            "--ui" => once(&mut o.ui, flag, v)?,
            "--screen" => once(&mut o.screen, flag, v)?,
            "--site" if site::valid_token(v) => o.site = Some(v.clone()),
            "--site" => return Err(format!("--site: 1 to 32 of A-Z a-z 0-9 . _ -, got {v:?}")),
            "--box-name" if site::valid_name(v) => o.box_name = Some(v.clone()),
            "--box-name" => {
                return Err(format!(
                    "--box-name: 1 to 48 of A-Z a-z 0-9 space . _ -, no space at either end, got {v:?}"
                ));
            }
            "--encap-port" => o.server.encap_port = port()?,
            "--io-port" => o.server.io_port = port()?,
            "--peer-io-port" => o.server.peer_io_port = port()?,
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    if o.relay.is_some() != o.relay_token.is_some() {
        return Err("--relay and --relay-token go together".into());
    }
    if o.ui.is_some() && o.api.is_none() {
        return Err("--ui is served by the API; it does not go with --api off".into());
    }
    if o.screen.is_some() && o.api.is_none() {
        return Err("--screen is served by the API; it does not go with --api off".into());
    }
    if o.site.is_some() && o.api.is_none_or(|a| a.port() == 0) {
        return Err("--site announces the API's port; it needs --api IP:PORT with a fixed port".into());
    }
    if o.box_name.is_some() && o.site.is_none() {
        return Err("--box-name names this box in its site; it needs --site NAME".into());
    }
    if o.sinks.is_some() != o.sink_dir.is_some() {
        return Err("--sinks and --sink-dir go together".into());
    }
    let from_files = !(o.programs.is_empty() && o.modules.is_empty() && o.module_dirs.is_empty());
    if o.projects {
        if from_files || o.io_map.is_some() || o.calendar.is_some() || o.emit_st {
            return Err(
                "--projects runs the deployed project (the store's head); it takes no PROGRAM, --module, \
                 --module-dir, --io-map, --calendar or --emit-st"
                    .into(),
            );
        }
        if o.store.is_none() {
            o.store = Some(state_dir(&o).join("store"));
        }
    }
    if o.store.is_some() && (from_files || o.io_map.is_some() || o.calendar.is_some() || o.emit_st) {
        return Err(
            "--store runs the store's head snapshot; it takes no PROGRAM, --module, --module-dir, \
             --io-map, --calendar or --emit-st"
                .into(),
        );
    }
    if o.set_pin.is_some() {
        return Ok(o);
    }
    if !from_files && o.store.is_none() {
        return Err("missing PROGRAM.st, PROJECT.xml, --module, --module-dir or --store".into());
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

/// What a module gets for whatever it does not set.
fn defaults(o: &Opts, i_len: usize, q_len: usize) -> Defaults {
    Defaults {
        period: o.scan,
        i: Window::new(0, i_len),
        q: Window::new(0, q_len),
        m: o.memory,
    }
}

/// A file's text, and where messages say it is.
type Text = (PathBuf, String);

/// What the box runs beyond bare programs: module directories, the I/O
/// map and the calendar, read from their files or from a snapshot.
struct Content {
    dirs: Vec<ModuleDir>,
    io_map: Option<Text>,
    calendar: Option<Text>,
    /// The approved WASM drivers the map may name (`wasm:NAME`): the
    /// snapshot's; none from files.
    drivers: helmsman_plc::drivers::Drivers,
    /// Which snapshot, when it came from a store.
    origin: Option<String>,
    /// All of it as a change set, for the API's proposals: the head
    /// snapshot, or the files read again. `None` when bare programs or
    /// `--module` specs are loaded too (no snapshot can hold those).
    running: Option<ChangeSet>,
}

fn read_text(path: &Path) -> Result<Text, String> {
    let src = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path.to_path_buf(), src))
}

/// The head snapshot of the store in `dir`; messages name its files as
/// `DIR/<id>/...`.
fn from_store(dir: &Path, d: &Defaults, projects: bool) -> Result<Content, String> {
    let store = Store::open(dir).map_err(|e| e.to_string())?;
    let head = store.head().map_err(|e| e.to_string())?;
    let head = match head {
        Some(h) => h,
        // The editor's box before its first deploy: nothing runs yet.
        None if projects => {
            return Ok(Content {
                dirs: Vec::new(),
                io_map: None,
                calendar: None,
                drivers: helmsman_plc::drivers::Drivers::new(),
                origin: Some(format!(
                    "store {}: no project deployed yet; nothing runs until one is",
                    dir.display()
                )),
                running: Some(ChangeSet::default()),
            });
        }
        None => return Err(format!("store {} has no snapshot yet", dir.display())),
    };
    let cs = store.get(&head.id).map_err(|e| e.to_string())?;
    let running = Some(cs.clone());
    let at = dir.join(head.id.to_string());
    let kind = match head.kind {
        Kind::Apply => "applied".to_string(),
        Kind::Rollback { to } => format!("rolled back to {to}"),
    };
    let dirs = cs
        .modules
        .into_iter()
        .map(|(name, f)| match f.code {
            Code::St(source) => ModuleDir::from_text(&at.join(name), &f.manifest, source, d),
            Code::Wasm(bytes) => ModuleDir::from_wasm(&at.join(name), &f.manifest, bytes, d),
        })
        .collect::<Result<_, _>>()?;
    Ok(Content {
        dirs,
        io_map: cs.io_map.map(|s| (at.join("io.map"), s)),
        calendar: cs.calendar.map(|s| (at.join("calendar"), s)),
        drivers: cs.drivers,
        origin: Some(format!(
            "store {}: snapshot {} (entry {}, {kind} by {})",
            dir.display(),
            head.id,
            head.seq,
            head.meta.author
        )),
        running,
    })
}

fn content(o: &Opts, d: &Defaults) -> Result<Content, String> {
    if let Some(dir) = &o.store {
        return from_store(dir, d, o.projects);
    }
    Ok(Content {
        dirs: o
            .module_dirs
            .iter()
            .map(|dir| ModuleDir::read(dir, d))
            .collect::<Result<_, _>>()?,
        io_map: o.io_map.as_deref().map(read_text).transpose()?,
        calendar: o.calendar.as_deref().map(read_text).transpose()?,
        drivers: helmsman_plc::drivers::Drivers::new(),
        origin: None,
        running: (o.programs.is_empty() && o.modules.is_empty())
            .then(|| ChangeSet::read(&o.module_dirs, o.io_map.as_deref(), o.calendar.as_deref()).ok())
            .flatten(),
    })
}

/// Each module's spec, and its directory when it came from one.
fn specs(o: &Opts, d: &Defaults, dirs: Vec<ModuleDir>) -> Result<Vec<(Spec, Option<ModuleDir>)>, String> {
    let mut specs: Vec<(Spec, Option<ModuleDir>)> =
        o.programs.iter().map(|p| (Spec::from_path(p, d), None)).collect();
    for m in &o.modules {
        specs.push((Spec::parse(m, d)?, None));
    }
    for md in dirs {
        specs.push((md.spec.clone(), Some(md)));
    }
    Ok(specs)
}

/// The module and its source text.
fn load((spec, dir): &(Spec, Option<ModuleDir>)) -> Result<(Module, String), String> {
    let file = spec.path.display();
    if let Some(md) = dir
        && let Some(bytes) = &md.wasm
    {
        #[cfg(feature = "wasm")]
        return Ok((
            md.wasm_module(helmsman_plc::wasm::Limits::default())?,
            helmsman_plc::store::wasm_note(bytes),
        ));
        #[cfg(not(feature = "wasm"))]
        return Err(format!(
            "{file}: a WASM module, and this build has no `wasm` feature ({} bytes)",
            bytes.len()
        ));
    }
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
        return Ok((md.module(program)?, src));
    }
    let module = Module {
        name: spec.name.clone(),
        program: program.into(),
        period: spec.period,
        i: spec.i,
        q: spec.q,
        params: Vec::new(),
    };
    Ok((module, src))
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

/// What the I/O map and calendar load into: the signal table, whether any
/// signal is on the box's own channels, the map itself, and the calendar.
struct Signals {
    table: SignalTable,
    uses_box: bool,
    map: Option<helmsman_io::IoMap>,
    calendar: Option<Source>,
}

/// The I/O map's signal table, whether any signal is on the box's own
/// channels, and the map.
fn load_io(
    (path, src): &Text,
    drivers: &helmsman_plc::drivers::Drivers,
) -> Result<(SignalTable, bool, helmsman_io::IoMap), String> {
    let file = path.display();
    let parsed = helmsman_io::text::parse(src).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|(n, e)| format!("\n  {file}:{n}: {e}")).collect();
        format!("{file} does not parse:{}", lines.concat())
    })?;
    let unknown = helmsman_plc::drivers::unknown(&parsed.map, drivers);
    let mut points = helmsman_plc::drivers::Points::new(drivers);
    let table = SignalTable::with_points(&parsed.map, &parsed.safe, &mut |d, a| points.parse(d, a));
    let table = match table {
        Ok(t) if unknown.is_empty() => t,
        r => {
            let mut lines: Vec<String> = unknown.iter().map(|e| format!("\n  {file}: {e}")).collect();
            lines.extend(r.err().into_iter().flatten().map(|e| format!("\n  {file}: {e}")));
            return Err(format!("{file} is not a valid map:{}", lines.concat()));
        }
    };
    let uses_box = parsed
        .map
        .signals
        .iter()
        .any(|s| matches!(s.binding, helmsman_io::Binding::Local { .. }));
    Ok((table, uses_box, parsed.map))
}

/// The floor screen's layout, checked against the I/O map; `None` without
/// `--screen`. A map with `screen` outputs needs one when the API runs
/// (without it, nothing would show them).
fn load_screen(o: &Opts, map: Option<&helmsman_io::IoMap>) -> Result<Option<api::Screen>, String> {
    let screens = map.is_some_and(|m| {
        m.signals
            .iter()
            .any(|s| matches!(s.binding, helmsman_io::Binding::Screen { .. }))
    });
    let Some(path) = &o.screen else {
        if screens && o.api.is_some() {
            return Err("the I/O map declares screen tiles: give their layout with --screen FILE".into());
        }
        return Ok(None);
    };
    let file = path.display();
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| {
            use std::io::Read as _;
            f.take(helmsman_plc::screen::MAX_FILE as u64 + 1)
                .read_to_end(&mut buf)
        })
        .map_err(|e| format!("{file}: {e}"))?;
    let source = String::from_utf8(buf).map_err(|_| format!("{file}: not UTF-8"))?;
    let layout = helmsman_plc::screen::parse(&source).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} is not a valid screen layout:{}", lines.concat())
    })?;
    layout
        .check(map.unwrap_or(&helmsman_io::IoMap::default()))
        .map_err(|errs| {
            let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
            format!("{file} does not fit the I/O map:{}", lines.concat())
        })?;
    Ok(Some(api::Screen { source, layout }))
}

/// The calendar file, bound to `table`.
fn load_calendar((path, src): &Text, table: &mut SignalTable) -> Result<Source, String> {
    let file = path.display();
    let cal = calendar::load(src).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} is not a valid calendar:{}", lines.concat())
    })?;
    Source::attach(cal, table).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} does not fit the I/O map:{}", lines.concat())
    })
}

/// One driver shared by every link that asks for it: the board's GPIO
/// lines stay requested once, across map reloads.
struct Shared(Arc<Mutex<Box<dyn Driver + Send>>>);

impl Driver for Shared {
    fn parse(&self, addr: &str) -> Result<helmsman_io::PointInfo, helmsman_io::Error> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).parse(addr)
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<helmsman_io::Value>, helmsman_io::Error> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).read(addrs)
    }

    fn write(&mut self, addr: &str, value: helmsman_io::Value) -> Result<(), helmsman_io::Error> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(addr, value)
    }
}

/// The box's own channels: the board's GPIO and I2C with `--gpio-chip`
/// (the chip is checked here, at start), in memory without.
fn box_channels(chip: Option<&Path>) -> Result<helmsman_plc::link::Channels, String> {
    let Some(p) = chip else {
        return Ok(Arc::new(|| Box::new(Channels::new())));
    };
    #[cfg(target_os = "linux")]
    {
        let board = helmsman_io::board::linux::open(p).map_err(|e| format!("--gpio-chip: {e}"))?;
        let shared: Arc<Mutex<Box<dyn Driver + Send>>> = Arc::new(Mutex::new(Box::new(board)));
        Ok(Arc::new(move || Box::new(Shared(Arc::clone(&shared)))))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = Shared;
        Err(format!(
            "--gpio-chip {}: the GPIO backend is Linux only",
            p.display()
        ))
    }
}

/// [`load_io`] (or no signals), with the calendar's added.
fn load_signals(c: &Content) -> Result<Signals, String> {
    let (mut table, uses_box, map) = match &c.io_map {
        Some(text) => {
            let (table, uses_box, map) = load_io(text, &c.drivers)?;
            (table, uses_box, Some(map))
        }
        None => (SignalTable::default(), false, None),
    };
    let calendar = c
        .calendar
        .as_ref()
        .map(|t| load_calendar(t, &mut table))
        .transpose()?;
    Ok(Signals {
        table,
        uses_box,
        map,
        calendar,
    })
}

/// Wall-clock seconds since the epoch, or `None` before it.
fn wall() -> Option<i64> {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

/// What the modules see, recorded for sim replay (the box API will serve
/// its tap). Built after the calendar attaches, so its signals are in.
fn recorder(table: &Mutex<SignalTable>) -> Recorder {
    Recorder::new(
        &table.lock().unwrap_or_else(PoisonError::into_inner),
        RecordConfig::default(),
        wall().and_then(|s| s.checked_mul(1000)),
    )
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
/// With a `cap` (a calendar to keep current, a view to publish for the
/// API) it wakes at least that often, even once every module has stopped;
/// without one, a runtime with nothing left to run parks for good.
/// `said` keeps the "every module has stopped" line to once.
fn sleep_to(start: Instant, next: Option<Duration>, cap: Option<Duration>, said: &mut bool) {
    let next = if let Some(next) = next {
        next
    } else {
        if !*said {
            eprintln!("helmsman-plc: every module has stopped; the adapter stays up");
            *said = true;
        }
        match cap {
            Some(cap) => start.elapsed() + cap,
            None => loop {
                thread::park();
            },
        }
    };
    let wait = (start + next).checked_duration_since(Instant::now());
    if let Some(wait) = wait.map(|w| cap.map_or(w, |c| w.min(c))) {
        thread::sleep(wait);
    }
}

/// The sinks file's sinks, started, if there is one.
fn start_sinks(o: &Opts, table: &SignalTable) -> Result<Option<Sinks>, String> {
    let (Some(path), Some(dir)) = (&o.sinks, &o.sink_dir) else {
        return Ok(None);
    };
    let file = path.display();
    let src = fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let seed = (u64::from(nanos) << 32) ^ u64::from(std::process::id());
    let log: sink::Log = Arc::new(|m: &str| eprintln!("helmsman-plc: {m}"));
    let env = |k: &str| std::env::var(k).ok();
    Sinks::start(
        &src,
        dir,
        table,
        &env,
        &sink::config::read_header_file,
        seed,
        &log,
    )
    .map(Some)
    .map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
        format!("{file} is not a valid sinks file:{}", lines.concat())
    })
}

/// The device links of `map`, built (drivers opened) but not started:
/// each will run on its own thread, reporting up/down to the journal and
/// to `devices`. A reload changes them ([`LinkSet`]).
fn plan_io(
    map: Option<&helmsman_io::IoMap>,
    table: &Arc<Mutex<SignalTable>>,
    devices: &Arc<Devices>,
    channels: helmsman_plc::link::Channels,
    io: &IoEndpoint,
    drivers: &helmsman_plc::drivers::Drivers,
) -> Result<(LinkSet, Plan), String> {
    let devices = Arc::clone(devices);
    let io = io.clone();
    let set = LinkSet::new(
        Arc::clone(table),
        Arc::new(move |d: &helmsman_io::Device| helmsman_io::open_on(d, Some(&io))),
        channels,
        Arc::new(move |name: &str, err: Option<&helmsman_io::Error>| {
            match err {
                None => eprintln!("helmsman-plc: device {name} up"),
                Some(e) => eprintln!("helmsman-plc: device {name} down: {e}"),
            }
            devices.set(name, err.map(ToString::to_string));
        }),
    );
    let plan = set.plan_with(&map.cloned().unwrap_or_default(), drivers)?;
    Ok((set, plan))
}

/// One module, for the startup banner.
fn describe(s: &Spec) -> String {
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
}

/// Where the token lives: `--state-dir`, systemd's `$STATE_DIRECTORY`,
/// or `./helmsman-state`.
fn state_dir(o: &Opts) -> PathBuf {
    o.state_dir.clone().unwrap_or_else(default_state_dir)
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("STATE_DIRECTORY")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("helmsman-state"), PathBuf::from)
}

/// Starts the API on `bind` over the loaded box; prints the token if it
/// was just made. The API opens its own read connection to the store.
/// Reads one PIN from stdin for `role`, hashes it and stores it in the
/// state directory beside the others.
fn set_pin(o: &Opts, role: Role) -> Result<(), String> {
    let dir = state_dir(o);
    let mut pins = Pins::load(&dir, pin::MIN_ITERATIONS)?;
    eprintln!("helmsman-plc: PIN for {role} (one line on stdin):");
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("stdin: {e}"))?;
    let salt = pin::salt().map_err(|e| format!("random salt: {e}"))?;
    pins.set(role, line.trim_end_matches(['\r', '\n']), pin::ITERATIONS, salt)?;
    pins.save(&dir)
        .map_err(|e| format!("{}: {e}", dir.join(pin::FILE).display()))?;
    eprintln!(
        "helmsman-plc: {role} PIN stored (hashed) in {}",
        dir.join(pin::FILE).display()
    );
    Ok(())
}

/// The policy file, or the floor.
fn load_policy(o: &Opts) -> Result<Policy, String> {
    let Some(path) = &o.policy else {
        return Ok(Policy::default());
    };
    let file = path.display();
    let src = fs::read_to_string(path).map_err(|e| format!("{file}: {e}"))?;
    Policy::parse(&src).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(|(n, e)| format!("\n  {file}:{n}: {e}")).collect();
        format!("{file} is refused:{}", lines.concat())
    })
}

/// The policy file and the PINs, read before anything starts so a bad one
/// stops the box before it is on the network.
fn load_access(o: &Opts) -> Result<(Policy, Pins), String> {
    let policy = load_policy(o)?;
    let pins = Pins::load(&state_dir(o), pin::MIN_ITERATIONS)?;
    Accounts::load(&state_dir(o), pin::MIN_ITERATIONS)?;
    // Checked now so a bad trust store stops the box before it is on the
    // network; the API reads it again when it starts.
    TrustStore::load(&state_dir(o))?;
    Ok((policy, pins))
}

// What the API serves is that many separate things; a struct for one caller adds nothing.
#[allow(clippy::too_many_arguments)]
fn start_api(
    o: &Opts,
    bind: SocketAddr,
    catalog: Catalog,
    running: Option<ChangeSet>,
    live: &Arc<Live>,
    devices: &Arc<Devices>,
    approval: Approval,
    screen: Option<api::Screen>,
    forces: &Arc<Forces>,
) -> Result<(Api, Option<Site>), String> {
    let ui =
        o.ui.as_deref()
            .map(|d| api::ui::Ui::open(d).map_err(|e| format!("--ui {}: {e}", d.display())))
            .transpose()?;
    let dir = state_dir(o);
    let token =
        api::token::load_or_create(&dir).map_err(|e| format!("API token in {}: {e}", dir.display()))?;
    let snapshots = o
        .store
        .as_deref()
        .map(|d| Store::open(d).map(Mutex::new).map_err(|e| e.to_string()))
        .transpose()?;
    if Accounts::load(&dir, pin::MIN_ITERATIONS).is_ok_and(|a| a.is_some()) {
        eprintln!(
            "helmsman-plc: accounts in {}: approvers log in (POST /api/login); role PINs are refused",
            dir.join(account::FILE).display()
        );
    } else if Role::ALL.iter().all(|r| !approval.gate.pins().has(*r)) {
        eprintln!(
            "helmsman-plc: no role PINs in {}: nothing can be applied until one is set (--set-pin ROLE)",
            dir.display()
        );
    }
    let cfg = api::Config {
        bind,
        token: token.token.clone(),
        limits: api::Limits::default(),
        stream_every: Duration::from_millis(250),
        memory: o.memory,
        log: Arc::new(|line: &str| eprintln!("helmsman-plc: {line}")),
    };
    let shared = api::Shared {
        catalog,
        live: Arc::clone(live),
        devices: Arc::clone(devices),
        audit: Box::new(MemAudit::new(100_000)),
        proposals: Box::new(MemProposals::new(1000)),
        running,
        snapshots,
        approval: Some(approval),
        chat: chat_setup(o)?,
    };
    let trust = TrustStore::load(&dir)?;
    let has_ui = ui.is_some();
    let has_screen = screen.is_some();
    let extras = api::Extras {
        ui,
        screen,
        trust: Some(trust),
    };
    let site = o
        .site
        .as_deref()
        .map(|name| start_site(o, name, bind.port(), &token.token))
        .transpose()?;
    let projects = o
        .projects
        .then(|| {
            helmsman_plc::project::Projects::in_state_dir(&dir)
                .map_err(|e| format!("--projects in {}: {e}", dir.display()))
        })
        .transpose()?;
    if let Some(p) = &projects {
        eprintln!(
            "helmsman-plc: projects in {}; deploying one needs an engineer login",
            p.root().display()
        );
    }
    let more = api::More {
        projects,
        forces: Some(Arc::clone(forces)),
    };
    let server = Api::start_box(cfg, shared, extras, site.as_ref().map(Site::handle), more)
        .map_err(|e| format!("--api {bind}: {e}"))?;
    eprintln!(
        "helmsman-plc: API on http://{}/api (reads open; changes need the token)",
        server.addr()
    );
    if has_ui {
        eprintln!("helmsman-plc: canvas on http://{}/", server.addr());
        if has_screen {
            eprintln!("helmsman-plc: floor screen on http://{}/?andon", server.addr());
        }
    }
    if token.new {
        eprintln!(
            "helmsman-plc: API token (new, shown once, kept in {}): {}",
            token.path.display(),
            token.token
        );
    } else {
        eprintln!("helmsman-plc: API token in {}", token.path.display());
    }
    Ok((server, site))
}

/// The host's name as a site box name, if it is one.
fn host_name() -> Option<String> {
    let n = fs::read_to_string("/etc/hostname").ok()?;
    let n = n.trim();
    site::valid_name(n).then(|| n.to_string())
}

/// Starts announcing this box in site `name` and hearing its peers.
fn start_site(o: &Opts, name: &str, api_port: u16, token: &str) -> Result<Site, String> {
    let id = site::box_id(token);
    let box_name = o
        .box_name
        .clone()
        .or_else(host_name)
        .unwrap_or_else(|| format!("box-{}", &id[..8]));
    let me = site::Beacon::new(name, &id, &box_name, api_port, env!("CARGO_PKG_VERSION"))?;
    let group = SocketAddr::from((site::GROUP, site::PORT));
    let socket = site::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, site::PORT)), group)
        .map_err(|e| format!("--site: cannot hear beacons on {group}: {e}"))?;
    let started = Site::start(
        socket,
        me,
        group,
        site::EVERY,
        Arc::new(|line: &str| eprintln!("helmsman-plc: {line}")),
    )
    .map_err(|e| format!("--site: {e}"))?;
    eprintln!("helmsman-plc: site {name}: announcing {box_name} (id {id}) on {group}; peers at /api/site");
    if o.api.is_some_and(|a| a.ip().is_loopback()) {
        eprintln!(
            "helmsman-plc: site {name}: the API is on loopback only, so other boxes' canvases \
             will list this one as unreachable (--api 0.0.0.0:PORT)"
        );
    }
    Ok(started)
}

/// The OPC UA server on `bind`, if any, and the scan side of its feed.
fn start_opcua(
    bind: Option<SocketAddr>,
) -> Result<(Option<opcua::Server>, Option<opcua::FeedPublisher>), String> {
    let Some(bind) = bind else {
        return Ok((None, None));
    };
    let feed = Arc::new(opcua::Feed::default());
    let server = opcua::Server::start(opcua::Config::new(bind), Arc::clone(&feed))
        .map_err(|e| format!("--opcua {bind}: {e} (`--opcua off` runs without it)"))?;
    eprintln!(
        "helmsman-plc: OPC UA on opc.tcp://{}/ (read-only, anonymous, SecurityPolicy None)",
        server.addr()
    );
    if !server.addr().ip().is_loopback() {
        eprintln!(
            "helmsman-plc: OPC UA is reachable off this box with no signing or encryption: plant LAN only"
        );
    }
    Ok((
        Some(server),
        Some(opcua::FeedPublisher::new(feed, opcua::PUBLISH_EVERY)),
    ))
}

/// The chat route's relay, from `--relay` and `--relay-token`.
fn chat_setup(o: &Opts) -> Result<Option<api::chat::Setup>, String> {
    let (Some(url), Some(path)) = (&o.relay, &o.relay_token) else {
        return Ok(None);
    };
    let url = relay::Url::parse(url).map_err(|e| format!("--relay: {e}"))?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("--relay-token {}: {}", path.display(), e.kind()))?;
    let token = text.trim();
    if !relay::token_ok(token) {
        return Err(format!(
            "--relay-token {}: not a relay box token (hmr1_ and 64 hex digits)",
            path.display()
        ));
    }
    eprintln!(
        "helmsman-plc: chat agent via relay {url} (agent {})",
        o.relay_agent
    );
    let mut setup = api::chat::Setup::new(relay::Config::new(url, token.to_string(), o.relay_agent.clone()));
    setup.explain_agent = Some(
        o.relay_explain_agent
            .clone()
            .unwrap_or_else(|| "explain-v1".into()),
    );
    Ok(Some(setup))
}

/// What the API serves that is fixed once the runtime is built.
fn catalog(
    rt: &Runtime,
    loaded: &[(Spec, Option<ModuleDir>)],
    sources: Vec<String>,
    table: &Mutex<SignalTable>,
    map: Option<&helmsman_io::IoMap>,
) -> Catalog {
    let modules: Vec<ModuleInfo> = loaded
        .iter()
        .zip(sources)
        .filter_map(|((spec, dir), source)| {
            let origin = dir
                .as_ref()
                .and_then(|_| spec.path.parent())
                .unwrap_or(&spec.path)
                .display()
                .to_string();
            ModuleInfo::of(
                rt,
                &spec.name,
                origin,
                source,
                dir.as_ref().map(|d| d.manifest.clone()),
            )
        })
        .collect();
    let signals = Catalog::signals(
        &table.lock().unwrap_or_else(PoisonError::into_inner),
        map,
        &modules,
    );
    Catalog { signals, modules }
}

/// The startup lines: the CIP banner (tests read its addresses from the
/// first line), where the content came from, what is simulated.
fn announce(
    adapter: &Adapter,
    specs: &[Spec],
    content: &Content,
    uses_box: bool,
    chip: Option<&Path>,
    cal: Option<&Source>,
) {
    let listed: Vec<String> = specs.iter().map(describe).collect();
    eprintln!(
        "helmsman-plc: CIP on {} (I/O {}), {} module(s): {}",
        adapter.encap_addr(),
        adapter.io_addr(),
        specs.len(),
        listed.join("; ")
    );
    if let Some(origin) = &content.origin {
        eprintln!("helmsman-plc: {origin}");
    }
    match (uses_box, chip) {
        (true, None) => eprintln!(
            "helmsman-plc: box channels (do:N, di:N, i2c:...) are in memory only; --gpio-chip puts them on the board"
        ),
        (true, Some(c)) => eprintln!(
            "helmsman-plc: box channels on {} (di:N, do:N) and /dev/i2c-N (i2c:...)",
            c.display()
        ),
        (false, _) => {}
    }
    if let Some(c) = cal {
        report_calendar(c);
    }
}

/// Journal lines for modules that were held or came back this tick
/// (`held`: which were held before it), and for those that faulted.
fn report_changes(
    specs: &[Spec],
    names: &[String],
    rt: &Runtime,
    held: &[bool],
    faults: Vec<(String, helmsman_iec::Fault)>,
) {
    for (name, was) in names.iter().zip(held) {
        match (was, rt.status(name)) {
            (false, Some(Status::Held)) => {
                eprintln!("helmsman-plc: HOLD, outputs safe: a signal module {name} reads has no value");
            }
            (true, Some(Status::Running)) => {
                eprintln!("helmsman-plc: module {name} running again");
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
}

/// Lets the box API browse each observed Logix controller in the map
/// (`POST /api/devices/{name}/browse`), read-only, over a session of its own.
fn browsers(map: Option<&helmsman_io::IoMap>, devices: &Devices) {
    for d in map.map_or(&[][..], |m| m.devices.as_slice()) {
        if d.driver != helmsman_io::logix::DRIVER {
            continue;
        }
        let Ok(driver) = helmsman_io::logix::LogixDriver::from_params(&d.endpoint, &d.params) else {
            continue;
        };
        let browse = move || {
            driver
                .browse()
                .map(|tags| {
                    tags.into_iter()
                        .map(|t| api::catalog::BrowsedTag {
                            name: t.name,
                            ty: t.ty,
                            dims: t.dims,
                        })
                        .collect()
                })
                .map_err(|e| e.to_string())
        };
        devices.set_browser(&d.id, Arc::new(browse));
    }
}

#[allow(clippy::too_many_lines)]
fn run(o: &Opts) -> Result<(), String> {
    if o.emit_st {
        return emit_st(o);
    }
    if let Some(role) = o.set_pin {
        return set_pin(o, role);
    }
    let access = o.api.map(|_| load_access(o)).transpose()?;
    let cfg = device_config(o.serial);
    let (i_len, q_len) = (assembly_size(&cfg, INPUTS), assembly_size(&cfg, OUTPUTS));
    let d = defaults(o, i_len, q_len);
    let mut content = content(o, &d)?;
    let loaded = specs(o, &d, std::mem::take(&mut content.dirs))?;
    let (modules, sources): (Vec<Module>, Vec<String>) = loaded
        .iter()
        .map(load)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();
    let Signals {
        table,
        uses_box,
        map,
        calendar: cal,
    } = load_signals(&content)?;
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
    let screen = load_screen(o, map.as_ref())?;
    let mut sinks = start_sinks(o, &table.lock().unwrap_or_else(PoisonError::into_inner))?;
    let catalog = catalog(&rt, &loaded, sources, &table, map.as_ref());
    let specs: Vec<Spec> = loaded.into_iter().map(|(s, _)| s).collect();
    let devices = Arc::new(Devices::new(Vec::new()));
    // One Class 1 I/O socket for the box: its adapter's connections and its
    // enip-io links' are routed by connection ID (helmsman_proto::endpoint).
    let io = IoEndpoint::bind(SocketAddrV4::new(o.server.bind, o.server.io_port))
        .map_err(|e| format!("CIP sockets: UDP {} (Class 1 I/O): {e}", o.server.io_port))?;
    let (link_set, io_plan) = plan_io(
        map.as_ref(),
        &table,
        &devices,
        box_channels(o.gpio_chip.as_deref())?,
        &io,
        &content.drivers,
    )?;
    for name in io_plan.starts() {
        devices.add(name);
    }
    browsers(map.as_ref(), &devices);
    let link_set = Arc::new(Mutex::new(link_set));
    let live = Arc::new(Live::default());
    let adapter =
        Adapter::start_on(Device::new(cfg), &o.server, &io).map_err(|e| format!("CIP sockets: {e}"))?;
    announce(
        &adapter,
        &specs,
        &content,
        uses_box,
        o.gpio_chip.as_deref(),
        cal.as_ref(),
    );
    let recorder = recorder(&table);
    let (swap, swapper) = api::swap::channel(Duration::from_secs(5));
    let swap = swap.with_links(Arc::clone(&link_set));
    let approval = access.map(|(policy, pins)| Approval {
        gate: Gate::new(pins, policy.lockout, Arc::new(Instant::now)).with_logins(Logins::new(
            state_dir(o),
            pin::MIN_ITERATIONS,
            policy.lockout,
            Arc::new(Instant::now),
        )),
        policy,
        plant: Plant {
            table: table.lock().unwrap_or_else(PoisonError::into_inner).clone(),
            i_len,
            q_len,
            defaults: d,
            record: Some(recorder.tap()),
        },
        swap,
    });
    let forces = Arc::new(Forces::default());
    let mut force_reader = force::Reader::default();
    let started = o
        .api
        .zip(approval)
        .map(|(bind, approval)| {
            start_api(
                o,
                bind,
                catalog,
                content.running.take(),
                &live,
                &devices,
                approval,
                screen,
                &forces,
            )
        })
        .transpose()?;
    // The site runs as long as the API does.
    let (server, _site) = started.map_or((None, None), |(api, site)| (Some(api), site));
    let swapper: Option<Swapper> = server.as_ref().map(|_| swapper);
    let mut publisher = server
        .as_ref()
        .map(|_| Publisher::new(Arc::clone(&live), api::PUBLISH_EVERY));
    let (_opcua, mut feed) = start_opcua(o.opcua)?;
    let every = publisher.as_ref().map(|p| p.every);
    let ua_every = feed.as_ref().map(|f| f.every);
    let cap = |cal: Option<&Source>| {
        [cal.map(|_| CALENDAR_TICK), every, ua_every]
            .into_iter()
            .flatten()
            .min()
    };
    let mut said = false;
    {
        let mut set = link_set.lock().unwrap_or_else(PoisonError::into_inner);
        let stopped = set.stop(&io_plan);
        set.finish(io_plan, stopped);
    }
    let mut scene = Scene {
        calendar: cal,
        recorder: Some(recorder),
        wall: None,
    };
    let dev = adapter.device();
    let mut inputs = vec![0; i_len];
    let mut names: Vec<String> = rt.names().map(str::to_string).collect();
    let start = Instant::now();
    loop {
        {
            let d = lock_ref(&dev);
            if let Some(src) = d.assembly(INPUTS) {
                inputs.clear();
                inputs.extend_from_slice(src);
            }
        }
        let (held, faults) = {
            let mut t = table.lock().unwrap_or_else(PoisonError::into_inner);
            scene.wall = wall();
            if let Some(sw) = &swapper {
                let did = sw.serve_scene(&mut rt, &mut t, Some(&mut scene));
                if (did.modules || did.reloaded)
                    && let Some(f) = &mut feed
                {
                    f.invalidate();
                }
                if did.modules {
                    names = rt.names().map(str::to_string).collect();
                    eprintln!(
                        "helmsman-plc: approved change applied; modules now: {}",
                        names.join(", ")
                    );
                }
                if did.reloaded {
                    eprintln!(
                        "helmsman-plc: I/O map and calendar reloaded: {} signal(s)",
                        t.len()
                    );
                    if let Some(c) = &scene.calendar {
                        report_calendar(c);
                    }
                }
            }
            let held: Vec<bool> = names.iter().map(|n| rt.status(n) == Some(Status::Held)).collect();
            if let Some(c) = &scene.calendar {
                c.update(scene.wall, &mut t);
            }
            // After the I/O read, before any module runs.
            force_reader.apply(&forces, &mut inputs, &mut t);
            let now = start.elapsed();
            if let Some(r) = &mut scene.recorder {
                r.observe(now, &t);
            }
            let faults = rt.tick_with(now, &inputs, &mut t);
            if let Some(p) = &mut publisher {
                p.publish(now, &rt, &t);
            }
            if let Some(f) = &mut feed {
                f.publish(now, sink::wall_ms(SystemTime::now()), &rt, &t);
            }
            if let Some(sk) = &mut sinks {
                sk.observe(&t, now, sink::wall_ms(SystemTime::now()));
            }
            (held, faults)
        };
        report_changes(&specs, &names, &rt, &held, faults);
        lock_ref(&dev).write_assembly(OUTPUTS, rt.outputs())?;
        sleep_to(start, rt.next_deadline(), cap(scene.calendar.as_ref()), &mut said);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "account") {
        let stdin = std::io::stdin();
        return match account::cli(&args[1..], &mut stdin.lock(), default_state_dir()) {
            Ok(out) => {
                print!("{out}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().is_some_and(|a| a == "bundle") {
        return match helmsman_plc::bundle::cli::run(&args[1..]) {
            Ok(out) => {
                print!("{out}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
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
