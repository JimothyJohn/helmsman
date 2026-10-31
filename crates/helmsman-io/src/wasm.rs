//! Drivers compiled to WebAssembly: the [`Driver`] contract carried across
//! a sandbox, so a driver the agent writes can reach its own device and
//! nothing else. Run by the interpreter the module tier uses ([wasmi]),
//! under fuel and memory limits; behind the `wasm` feature.
//!
//! **The interface, `helmsman-driver/v1`.** A guest may import only these
//! functions, from module `helmsman-driver/v1`:
//!
//! | import | type | what |
//! | --- | --- | --- |
//! | `send` | `(ptr, len: i32) -> i32` | write `len` bytes to the device; 0, or -1 if the link failed |
//! | `recv` | `(ptr, len: i32) -> i32` | read exactly `len` bytes from the device; 0, or -1 (time-out, closed, error) |
//! | `close` | `() -> ()` | drop the connection; the next `send` opens a new one |
//! | `point` | `(ty, writable: i32) -> ()` | `parse`'s answer |
//! | `value` | `(ty: i32, bits: i64) -> ()` | `read`'s answer, once per address, in order |
//! | `fail` | `(kind, code, ptr, len: i32) -> ()` | the call's error, with a message of up to 256 bytes |
//!
//! and must export:
//!
//! | export | type | what |
//! | --- | --- | --- |
//! | `memory` | memory | its linear memory |
//! | `buffer` | `(len: i32) -> i32` | where the host may put a call's `len` input bytes |
//! | `parse` | `(ptr, len: i32) -> ()` | parse the address at `ptr` without touching the device |
//! | `read` | `(ptr, len: i32) -> ()` | read the addresses at `ptr`, each a `u16` little-endian length and its bytes |
//! | `write` | `(ptr, len, ty: i32, bits: i64) -> ()` | write the value to the address at `ptr` |
//! | `configure` | `(ptr, len: i32) -> ()` | optional: the device's params as sorted `key=value` lines, once per instance |
//!
//! A type is 0 `BOOL` (bits 0 or 1), 1 `INT` (bits are the `i64`) or 2
//! `REAL` (bits are the `f64`'s). A `fail` kind is one of [`kind`] and
//! becomes the matching [`Error`]: `Address`, `Type` (code: the type the
//! point wants), `Protocol`, `Exception` (code: the device's exception),
//! `ReadOnly`, `Point`, or `Io` (the host's own record of what failed on
//! the socket). Anything else a guest imports or lacks, a `start`
//! function, a second memory or memory64 refuses it at load ([`check`]
//! lists every problem).
//!
//! **One capability.** The socket is a TCP stream to the device's own
//! endpoint, from the map: the guest never names a host or port, so it has
//! no way to ask for another one, and nothing in the interface reaches a
//! file, a clock, randomness or another device. The connection opens on
//! the first `send` and is dropped after any link error or trap. `parse`
//! and `configure` get no socket at all (`send` and `recv` fail), so
//! parsing never touches the device. Time-outs are the host's: each
//! socket operation waits at most the device's `timeout_ms` (1000 by
//! default), and a whole call at most that times one more than its
//! addresses.
//!
//! **Limits** ([`Limits`]): fuel per call (wasmi's, about one unit an
//! instruction; [`DEFAULT_FUEL`]), linear memory capped at
//! [`Limits::memory`] (a guest declaring more is refused, `memory.grow`
//! past it traps), one table of at most 10 000 elements, 64 KiB per
//! `send`/`recv` and per call's input, addresses of up to 255 bytes.
//!
//! **Faults.** A trap (out of fuel, the memory cap, an out-of-bounds
//! pointer handed to an import, `unreachable`, a contract breach such as
//! more values than addresses) fails that call with [`Error::Driver`],
//! a point error, so the link reading it marks that point bad and keeps
//! the device's other points; the instance and its connection are thrown
//! away and the next call starts a fresh one.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use wasmi::{
    Caller, CompilationMode, Config, Engine, Extern, ExternType, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TrapCode, TypedFunc, ValType,
};

use crate::driver::{Driver, Error, PointInfo};
use crate::model::{Device, Ty, Value};

/// The interface's module name and version.
pub const ABI: &str = "helmsman-driver/v1";
/// Fuel per call by default.
pub const DEFAULT_FUEL: u64 = 1_000_000;
/// Linear memory cap by default, in bytes (16 pages).
pub const DEFAULT_MEMORY: usize = 1 << 20;
/// Longest address, in bytes.
pub const MAX_ADDR: usize = 255;
/// Most bytes one call hands the guest, and one `send` or `recv` moves.
pub const MAX_IO: usize = 64 << 10;
/// Longest `fail` message kept, in bytes.
pub const MAX_MESSAGE: usize = 256;
const MAX_TABLE: usize = 10_000;

/// Value type codes.
pub mod ty {
    pub const BOOL: i32 = 0;
    pub const INT: i32 = 1;
    pub const REAL: i32 = 2;
}

/// `fail` kinds.
pub mod kind {
    pub const ADDRESS: i32 = 1;
    pub const TYPE: i32 = 2;
    pub const PROTOCOL: i32 = 3;
    pub const EXCEPTION: i32 = 4;
    pub const READ_ONLY: i32 = 5;
    pub const POINT: i32 = 6;
    pub const IO: i32 = 7;
}

/// Fuel and memory for one driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Fuel per call (`parse`, `read`, `write`).
    pub fuel: u64,
    /// Largest linear memory, in bytes.
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: DEFAULT_FUEL,
            memory: DEFAULT_MEMORY,
        }
    }
}

const I32: ValType = ValType::I32;
const I64: ValType = ValType::I64;

/// What a guest may import: name, params, results.
const IMPORTS: &[(&str, &[ValType], &[ValType])] = &[
    ("send", &[I32, I32], &[I32]),
    ("recv", &[I32, I32], &[I32]),
    ("close", &[], &[]),
    ("point", &[I32, I32], &[]),
    ("value", &[I32, I64], &[]),
    ("fail", &[I32, I32, I32, I32], &[]),
];

/// What a guest exports: name, params, results, required.
const EXPORTS: &[(&str, &[ValType], &[ValType], bool)] = &[
    ("buffer", &[I32], &[I32], true),
    ("parse", &[I32, I32], &[], true),
    ("read", &[I32, I32], &[], true),
    ("write", &[I32, I32, I32, I64], &[], true),
    ("configure", &[I32, I32], &[], false),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Configure,
    Parse,
    Read,
    Write,
}

impl Phase {
    /// Whether the guest may use its socket.
    const fn io(self) -> bool {
        matches!(self, Self::Read | Self::Write)
    }
}

/// A `fail` call, as the guest made it.
struct Failure {
    kind: i32,
    code: i32,
    message: String,
}

/// The store's data: the one socket and what the call has answered.
struct Host {
    endpoint: String,
    timeout: Duration,
    stream: Option<TcpStream>,
    phase: Phase,
    deadline: Instant,
    /// The last socket error in this call.
    io_error: Option<io::Error>,
    point: Option<PointInfo>,
    values: Vec<Value>,
    /// How many values `read` must answer.
    want: usize,
    failure: Option<Failure>,
    limits: StoreLimits,
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the call ran out of time")
}

impl Host {
    /// How long the next socket operation may wait.
    fn wait(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(timed_out())
        } else {
            Ok(left.min(self.timeout))
        }
    }

    fn refuse_io(&mut self) -> bool {
        if self.phase.io() {
            return false;
        }
        self.io_error = Some(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "no socket outside read and write",
        ));
        true
    }

    fn connect(&mut self) -> io::Result<&mut TcpStream> {
        if self.stream.is_none() {
            let mut last = None;
            for addr in self.endpoint.to_socket_addrs()? {
                match TcpStream::connect_timeout(&addr, self.wait()?) {
                    Ok(s) => {
                        s.set_nodelay(true)?;
                        self.stream = Some(s);
                        break;
                    }
                    Err(e) => last = Some(e),
                }
            }
            if self.stream.is_none() {
                return Err(last.unwrap_or_else(|| io::Error::other("no address")));
            }
        }
        Ok(self.stream.as_mut().expect("set above"))
    }

    fn try_send(&mut self, bytes: &[u8]) -> io::Result<()> {
        let wait = self.wait()?;
        let s = self.connect()?;
        s.set_write_timeout(Some(wait))?;
        s.write_all(bytes)
    }

    fn try_recv(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let wait = self.wait()?;
            let s = self
                .stream
                .as_mut()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "recv before send"))?;
            s.set_read_timeout(Some(wait))?;
            match s.read(&mut buf[got..]) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(buf)
    }

    /// Records a socket error and drops the connection.
    fn broke(&mut self, e: io::Error) {
        self.stream = None;
        self.io_error = Some(e);
    }
}

type Trap = wasmi::Error;

fn trap(why: impl Into<String>) -> Trap {
    Trap::new(why)
}

fn memory(c: &Caller<'_, Host>) -> Result<Memory, Trap> {
    c.get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| trap("the guest exports no memory"))
}

/// `ptr`, `len` as a range of the guest's memory of at most `max` bytes.
fn region(mem: Memory, c: &Caller<'_, Host>, ptr: i32, len: i32, max: usize) -> Result<usize, Trap> {
    let (Ok(start), Ok(n)) = (usize::try_from(ptr), usize::try_from(len)) else {
        return Err(trap("a negative pointer or length"));
    };
    if n > max {
        return Err(trap(format!("{n} bytes at once; at most {max}")));
    }
    if start.checked_add(n).is_none_or(|end| end > mem.data_size(c)) {
        return Err(trap("a pointer past the guest's memory"));
    }
    Ok(start)
}

fn bytes(c: &Caller<'_, Host>, ptr: i32, len: i32, max: usize) -> Result<Vec<u8>, Trap> {
    let mem = memory(c)?;
    let start = region(mem, c, ptr, len, max)?;
    let mut buf = vec![0u8; usize::try_from(len).unwrap_or(0)];
    mem.read(c, start, &mut buf)
        .map_err(|_| trap("a pointer past the guest's memory"))?;
    Ok(buf)
}

/// A message from the guest: valid UTF-8 or replaced, control
/// characters as spaces.
fn message(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn ty_of(code: i32) -> Option<Ty> {
    match code {
        ty::BOOL => Some(Ty::Bool),
        ty::INT => Some(Ty::Int),
        ty::REAL => Some(Ty::Real),
        _ => None,
    }
}

const fn encode(v: Value) -> (i32, i64) {
    match v {
        Value::Bool(b) => (ty::BOOL, b as i64),
        Value::Int(n) => (ty::INT, n),
        Value::Real(x) => (ty::REAL, x.to_bits().cast_signed()),
    }
}

fn linker(engine: &Engine) -> Result<Linker<Host>, Trap> {
    let mut l = Linker::<Host>::new(engine);
    l.func_wrap(
        ABI,
        "send",
        |mut c: Caller<'_, Host>, ptr: i32, len: i32| -> Result<i32, Trap> {
            let out = bytes(&c, ptr, len, MAX_IO)?;
            let h = c.data_mut();
            if h.refuse_io() {
                return Ok(-1);
            }
            Ok(match h.try_send(&out) {
                Ok(()) => 0,
                Err(e) => {
                    h.broke(e);
                    -1
                }
            })
        },
    )?;
    l.func_wrap(
        ABI,
        "recv",
        |mut c: Caller<'_, Host>, ptr: i32, len: i32| -> Result<i32, Trap> {
            // Checked before reading, so no byte is taken off the wire for
            // a buffer that can't hold it.
            let mem = memory(&c)?;
            let start = region(mem, &c, ptr, len, MAX_IO)?;
            let h = c.data_mut();
            if h.refuse_io() {
                return Ok(-1);
            }
            match h.try_recv(usize::try_from(len).unwrap_or(0)) {
                Ok(got) => {
                    mem.write(&mut c, start, &got)
                        .map_err(|_| trap("a pointer past the guest's memory"))?;
                    Ok(0)
                }
                Err(e) => {
                    h.broke(e);
                    Ok(-1)
                }
            }
        },
    )?;
    l.func_wrap(ABI, "close", |mut c: Caller<'_, Host>| {
        c.data_mut().stream = None;
    })?;
    answers(&mut l)?;
    Ok(l)
}

/// The imports a call answers through: `point`, `value`, `fail`.
fn answers(l: &mut Linker<Host>) -> Result<(), Trap> {
    l.func_wrap(
        ABI,
        "point",
        |mut c: Caller<'_, Host>, t: i32, writable: i32| -> Result<(), Trap> {
            let h = c.data_mut();
            if h.phase != Phase::Parse {
                return Err(trap("point outside parse"));
            }
            if h.point.is_some() {
                return Err(trap("parse answered twice"));
            }
            let ty = ty_of(t).ok_or_else(|| trap(format!("point type {t}")))?;
            let writable = match writable {
                0 => false,
                1 => true,
                w => return Err(trap(format!("point writable {w}: 0 or 1"))),
            };
            h.point = Some(PointInfo { ty, writable });
            Ok(())
        },
    )?;
    l.func_wrap(
        ABI,
        "value",
        |mut c: Caller<'_, Host>, t: i32, bits: i64| -> Result<(), Trap> {
            let h = c.data_mut();
            if h.phase != Phase::Read {
                return Err(trap("value outside read"));
            }
            if h.values.len() >= h.want {
                return Err(trap("more values than addresses"));
            }
            let v = match (t, bits) {
                (ty::BOOL, 0 | 1) => Value::Bool(bits == 1),
                (ty::BOOL, _) => return Err(trap(format!("BOOL value {bits}: 0 or 1"))),
                (ty::INT, n) => Value::Int(n),
                (ty::REAL, b) => Value::Real(f64::from_bits(b.cast_unsigned())),
                _ => return Err(trap(format!("value type {t}"))),
            };
            h.values.push(v);
            Ok(())
        },
    )?;
    l.func_wrap(
        ABI,
        "fail",
        |mut c: Caller<'_, Host>, kind: i32, code: i32, ptr: i32, len: i32| -> Result<(), Trap> {
            if c.data().failure.is_some() {
                return Ok(());
            }
            // A longer message is cut, not refused.
            let len = len.min(i32::try_from(MAX_MESSAGE).expect("small"));
            let raw = bytes(&c, ptr, len, MAX_MESSAGE)?;
            c.data_mut().failure = Some(Failure {
                kind,
                code,
                message: message(&raw),
            });
            Ok(())
        },
    )?;
    Ok(())
}

fn engine() -> Engine {
    let mut config = Config::default();
    config
        .compilation_mode(CompilationMode::Eager)
        .consume_fuel(true)
        .allow_start_fn(false)
        .wasm_multi_memory(false);
    Engine::new(&config)
}

fn func_ok(ty: &ExternType, params: &[ValType], results: &[ValType]) -> bool {
    matches!(ty, ExternType::Func(f) if f.params() == params && f.results() == results)
}

fn sig(params: &[ValType], results: &[ValType]) -> String {
    let list = |v: &[ValType]| {
        v.iter()
            .map(|t| format!("{t:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("({}) -> ({})", list(params), list(results))
}

fn compile(wasm: &[u8]) -> Result<Module, Vec<String>> {
    let module = Module::new(&engine(), wasm).map_err(|e| vec![format!("not a valid WASM module: {e}")])?;
    let mut errs = Vec::new();
    for import in module.imports() {
        let (m, name) = (import.module(), import.name());
        let what = format!("import {m:?} {name:?}");
        let allowed = IMPORTS.iter().find(|(n, ..)| m == ABI && *n == name);
        match allowed {
            Some((_, p, r)) if func_ok(import.ty(), p, r) => {}
            Some((_, p, r)) => errs.push(format!("{what}: must be a function {}", sig(p, r))),
            None => errs.push(format!(
                "{what}: not part of {ABI}; a driver's one capability is its device's socket (no other hosts, files, clocks or WASI)"
            )),
        }
    }
    let exports: Vec<_> = module
        .exports()
        .map(|e| (e.name().to_string(), e.ty().clone()))
        .collect();
    for (name, p, r, required) in EXPORTS {
        match exports.iter().find(|(n, _)| n == name) {
            Some((_, ty)) if func_ok(ty, p, r) => {}
            Some(_) => errs.push(format!("export {name:?} must be a function {}", sig(p, r))),
            None if *required => errs.push(format!("no export {name:?} {}", sig(p, r))),
            None => {}
        }
    }
    if !exports
        .iter()
        .any(|(n, t)| n == "memory" && matches!(t, ExternType::Memory(_)))
    {
        errs.push("no exported memory \"memory\"".into());
    }
    if errs.is_empty() { Ok(module) } else { Err(errs) }
}

/// Checks `wasm` against `helmsman-driver/v1` without running it.
///
/// # Errors
/// Every problem found (not WebAssembly, an import outside the interface
/// or of the wrong type, a missing or mistyped export, a `start`
/// function).
pub fn check(wasm: &[u8]) -> Result<(), Vec<String>> {
    compile(wasm).map(|_| ())
}

/// Whether `wasm` would run as a driver: [`check`], then one instance
/// started under `limits` (its declared memory within the cap, its data
/// and element segments in bounds), with no device and without calling
/// `configure`, whose params are the device's. What a box asks of a
/// driver before it is proposed; nothing runs that could reach a socket.
///
/// # Errors
/// Every problem [`check`] finds, or why the instance does not start.
pub fn sandbox(wasm: &[u8], limits: Limits) -> Result<(), Vec<String>> {
    let module = compile(wasm)?;
    let d = WasmDriver {
        module,
        endpoint: String::new(),
        params: BTreeMap::new(),
        timeout: Duration::from_millis(1),
        limits,
        guest: RefCell::new(None),
    };
    d.instance().map(drop).map_err(|e| vec![e.to_string()])
}

/// One instance of the guest and its exports.
struct Guest {
    store: Store<Host>,
    memory: Memory,
    buffer: TypedFunc<i32, i32>,
    parse: TypedFunc<(i32, i32), ()>,
    read: TypedFunc<(i32, i32), ()>,
    write: TypedFunc<(i32, i32, i32, i64), ()>,
    configure: Option<TypedFunc<(i32, i32), ()>>,
}

/// What a call answered.
struct Outcome {
    point: Option<PointInfo>,
    values: Vec<Value>,
}

/// A call's input.
struct Call<'a> {
    phase: Phase,
    input: &'a [u8],
    /// Addresses in the call: values a read must answer, and how long the
    /// call may take.
    points: usize,
    /// A write's value.
    value: Option<Value>,
    /// What an `Address` or `ReadOnly` without a message names.
    subject: &'a str,
}

fn trapped(e: &Trap, limits: Limits) -> String {
    match e.as_trap_code() {
        Some(TrapCode::OutOfFuel) => format!("out of fuel ({} per call)", limits.fuel),
        // StoreLimits' trap_on_grow_failure: a memory.grow past the cap
        // (or a table's past its).
        Some(TrapCode::GrowthOperationLimited) => {
            format!(
                "over its memory cap ({} bytes) or table cap ({MAX_TABLE} elements)",
                limits.memory
            )
        }
        _ => format!("trapped: {e}"),
    }
}

impl Guest {
    /// Runs one call. The outer error is the guest's fault (a trap, a
    /// contract breach); the instance is then not to be trusted.
    fn invoke(
        &mut self,
        call: &Call<'_>,
        limits: Limits,
        timeout: Duration,
    ) -> Result<Result<Outcome, Error>, String> {
        if call.phase == Phase::Configure && self.configure.is_none() {
            return Ok(Ok(Outcome {
                point: None,
                values: Vec::new(),
            }));
        }
        let h = self.store.data_mut();
        h.phase = call.phase;
        h.want = if call.phase == Phase::Read { call.points } else { 0 };
        h.values.clear();
        h.point = None;
        h.failure = None;
        h.io_error = None;
        let n = u32::try_from(call.points).unwrap_or(u32::MAX).saturating_add(1);
        h.deadline = Instant::now() + timeout.saturating_mul(n);
        self.store.set_fuel(limits.fuel).expect("the engine meters fuel");
        let len = i32::try_from(call.input.len()).map_err(|_| "input too long".to_string())?;
        let ptr = self
            .buffer
            .call(&mut self.store, len)
            .map_err(|e| trapped(&e, limits))?;
        let start = usize::try_from(ptr)
            .ok()
            .filter(|&p| {
                p != 0
                    && p.checked_add(call.input.len())
                        .is_some_and(|end| end <= self.memory.data_size(&self.store))
            })
            .ok_or_else(|| format!("buffer({len}) gave {ptr}, not a region of its memory"))?;
        self.memory
            .write(&mut self.store, start, call.input)
            .map_err(|e| e.to_string())?;
        let (vt, bits) = call.value.map_or((0, 0), encode);
        let ran = match call.phase {
            Phase::Configure => self
                .configure
                .expect("checked above")
                .call(&mut self.store, (ptr, len)),
            Phase::Parse => self.parse.call(&mut self.store, (ptr, len)),
            Phase::Read => self.read.call(&mut self.store, (ptr, len)),
            Phase::Write => self.write.call(&mut self.store, (ptr, len, vt, bits)),
        };
        ran.map_err(|e| trapped(&e, limits))?;
        let h = self.store.data_mut();
        if let Some(f) = h.failure.take() {
            let e = failure(f, h.io_error.take(), call)?;
            if e.is_link() {
                h.stream = None;
            }
            return Ok(Err(e));
        }
        match call.phase {
            Phase::Parse if h.point.is_none() => Err("parse answered neither point nor fail".into()),
            Phase::Read if h.values.len() != call.points => Err(format!(
                "read answered {} values for {} addresses",
                h.values.len(),
                call.points
            )),
            _ => Ok(Ok(Outcome {
                point: h.point.take(),
                values: std::mem::take(&mut h.values),
            })),
        }
    }
}

/// The [`Error`] a `fail` call means, or why it means nothing.
fn failure(f: Failure, io: Option<io::Error>, call: &Call<'_>) -> Result<Error, String> {
    let msg = if f.message.is_empty() {
        call.subject.to_string()
    } else {
        f.message
    };
    Ok(match f.kind {
        kind::ADDRESS => Error::Address(msg),
        kind::TYPE => match (ty_of(f.code), call.value) {
            (Some(want), Some(v)) => Error::Type { want, got: v.ty() },
            _ => {
                return Err(format!(
                    "a type error needs a write and a type, got code {}",
                    f.code
                ));
            }
        },
        kind::PROTOCOL => Error::Protocol(msg),
        kind::EXCEPTION => {
            Error::Exception(u8::try_from(f.code).map_err(|_| format!("exception code {}", f.code))?)
        }
        kind::READ_ONLY => Error::ReadOnly(msg),
        kind::POINT => Error::Point(msg),
        kind::IO => Error::Io(io.unwrap_or_else(|| io::Error::other(msg))),
        k => return Err(format!("fail kind {k} is not in {ABI}")),
    })
}

/// A driver running as a WASM guest, for one device.
pub struct WasmDriver {
    module: Module,
    endpoint: String,
    params: BTreeMap<String, String>,
    timeout: Duration,
    limits: Limits,
    /// `None` after a trap, until the next call instantiates again.
    /// A `RefCell` because [`Driver::parse`] takes `&self`.
    guest: RefCell<Option<Guest>>,
}

impl fmt::Debug for WasmDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmDriver")
            .field("endpoint", &self.endpoint)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl WasmDriver {
    /// Checks `wasm`, instantiates it under `limits` and hands it the
    /// device's params (`configure`). The device's `endpoint` is the only
    /// place its socket reaches; `timeout_ms` (default 1000) is the
    /// host's.
    ///
    /// # Errors
    /// [`Error::Driver`] with every problem [`check`] finds, or an
    /// instance that does not start (memory over the cap, a trap);
    /// `Address("timeout_ms")` if that param doesn't parse; whatever the
    /// guest's `configure` fails with.
    pub fn load(wasm: &[u8], device: &Device, limits: Limits) -> Result<Self, Error> {
        let module = compile(wasm).map_err(|errs| Error::Driver(errs.join("; ")))?;
        let ms = device
            .params
            .get("timeout_ms")
            .map_or(Ok(1000), |t| t.parse::<u64>())
            .map_err(|_| Error::Address("timeout_ms".into()))?;
        let d = Self {
            module,
            endpoint: device.endpoint.clone(),
            params: device.params.clone(),
            timeout: Duration::from_millis(ms.max(1)),
            limits,
            guest: RefCell::new(None),
        };
        let g = d.instantiate()?;
        *d.guest.borrow_mut() = Some(g);
        Ok(d)
    }

    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// A fresh instance with its device's params handed to `configure`.
    fn instantiate(&self) -> Result<Guest, Error> {
        let mut g = self.instance()?;
        let mut params = String::new();
        for (k, v) in &self.params {
            let _ = writeln!(params, "{k}={v}");
        }
        if params.len() > MAX_IO {
            return Err(Error::Driver("device params over 64 KiB".into()));
        }
        let call = Call {
            phase: Phase::Configure,
            input: params.as_bytes(),
            points: 0,
            value: None,
            subject: "",
        };
        g.invoke(&call, self.limits, self.timeout)
            .map_err(Error::Driver)??;
        Ok(g)
    }

    /// A fresh instance, not yet configured.
    fn instance(&self) -> Result<Guest, Error> {
        let engine = self.module.engine();
        let host = Host {
            endpoint: self.endpoint.clone(),
            timeout: self.timeout,
            stream: None,
            phase: Phase::Configure,
            deadline: Instant::now(),
            io_error: None,
            point: None,
            values: Vec::new(),
            want: 0,
            failure: None,
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory)
                .memories(1)
                .tables(1)
                .table_elements(MAX_TABLE)
                .instances(1)
                .trap_on_grow_failure(true)
                .build(),
        };
        let mut store = Store::new(engine, host);
        store.limiter(|h| &mut h.limits);
        store.set_fuel(self.limits.fuel).expect("the engine meters fuel");
        let bad = |e: Trap| Error::Driver(format!("does not instantiate: {e}"));
        let instance = linker(engine)
            .and_then(|l| l.instantiate_and_start(&mut store, &self.module))
            .map_err(bad)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| Error::Driver("no memory".into()))?;
        let buffer = instance.get_typed_func(&store, "buffer").map_err(bad)?;
        let parse = instance.get_typed_func(&store, "parse").map_err(bad)?;
        let read = instance.get_typed_func(&store, "read").map_err(bad)?;
        let write = instance.get_typed_func(&store, "write").map_err(bad)?;
        let configure = instance.get_typed_func(&store, "configure").ok();
        Ok(Guest {
            store,
            memory,
            buffer,
            parse,
            read,
            write,
            configure,
        })
    }

    fn call(&self, call: &Call<'_>) -> Result<Outcome, Error> {
        let mut slot = self.guest.borrow_mut();
        if slot.is_none() {
            *slot = Some(self.instantiate()?);
        }
        let g = slot.as_mut().expect("set above");
        match g.invoke(call, self.limits, self.timeout) {
            Ok(r) => r,
            Err(why) => {
                // Its memory and its connection are both suspect now.
                *slot = None;
                Err(Error::Driver(why))
            }
        }
    }
}

fn addr_ok(addr: &str) -> Result<(), Error> {
    if addr.len() > MAX_ADDR {
        Err(Error::Address(addr.chars().take(32).collect::<String>() + "..."))
    } else {
        Ok(())
    }
}

impl Driver for WasmDriver {
    fn parse(&self, addr: &str) -> Result<PointInfo, Error> {
        addr_ok(addr)?;
        let o = self.call(&Call {
            phase: Phase::Parse,
            input: addr.as_bytes(),
            points: 0,
            value: None,
            subject: addr,
        })?;
        Ok(o.point.expect("checked by invoke"))
    }

    fn read(&mut self, addrs: &[&str]) -> Result<Vec<Value>, Error> {
        if addrs.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = Vec::new();
        for a in addrs {
            addr_ok(a)?;
            input.extend(u16::try_from(a.len()).expect("at most 255").to_le_bytes());
            input.extend(a.as_bytes());
        }
        if input.len() > MAX_IO {
            return Err(Error::Point(format!(
                "{} addresses are too many for one read",
                addrs.len()
            )));
        }
        let subject = addrs.join(",");
        self.call(&Call {
            phase: Phase::Read,
            input: &input,
            points: addrs.len(),
            value: None,
            subject: &subject,
        })
        .map(|o| o.values)
    }

    fn write(&mut self, addr: &str, value: Value) -> Result<(), Error> {
        addr_ok(addr)?;
        self.call(&Call {
            phase: Phase::Write,
            input: addr.as_bytes(),
            points: 1,
            value: Some(value),
            subject: addr,
        })
        .map(|_| ())
    }
}
