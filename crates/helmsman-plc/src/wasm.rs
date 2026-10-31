//! WASM modules: logic compiled to WebAssembly, run in the scan beside ST
//! modules by an interpreter ([wasmi]) under fuel and memory limits.
//!
//! **The host interface, `helmsman/v1`.** A module may import only these,
//! and must export one function:
//!
//! | import | type | what |
//! | --- | --- | --- |
//! | `helmsman/v1/in` `NAME` | `() -> i32 / i64 / f64` | read input `NAME` (a signal it `reads`, or a `param`) |
//! | `helmsman/v1/out` `NAME` | `(i32 / i64 / f64) -> ()` | set output `NAME` (a signal it `drives`) |
//! | `helmsman/v1` `now_ms` | `() -> i64` | the scan's time, in ms on the runtime's clock |
//!
//! | export | type | what |
//! | --- | --- | --- |
//! | `scan` | `() -> ()` | one scan |
//!
//! Each `in` / `out` import is a **port**, as a `VAR_INPUT` / `VAR_OUTPUT`
//! is in ST: its name is the port's (an identifier, case-insensitive, one
//! direction only), its value type its ST type (`i32` a `BOOL` as 0 or 1,
//! `i64` an `LINT`, `f64` an `LREAL`). The manifest is checked against
//! these ports exactly as against an ST program's, and the runtime binds
//! them to signals by name, so a module can name no signal its manifest
//! does not declare. Anything else it imports (WASI, another clock, a
//! memory, a table, a global) refuses the module at load: there is no
//! network, file system or clock in this interface, and none can be
//! reached.
//!
//! **Limits.** Each scan gets [`Limits::fuel`] (wasmi's fuel, about one
//! unit per instruction); running out is a [`FaultKind::Watchdog`] fault,
//! as an ST module's instruction budget is. Linear memory is capped at
//! [`Limits::memory`] bytes (a module declaring more is refused at load,
//! `memory.grow` past it fails); one memory and one table at most, no
//! `start` function, no memory64 or multi-memory. A trap is a fault: out
//! of bounds memory or table access is `IndexOutOfRange`, integer division
//! by zero `DivisionByZero`, anything else (`unreachable`, overflow, a
//! stack overflow) [`FaultKind::Trap`]; a `BOOL` output set to anything
//! but 0 or 1 is `OutputRange`. Faults stop the module and send its
//! outputs safe, as for ST.
//!
//! **Determinism.** The interpreter has no clock, no randomness and no
//! threads, and NaNs are canonicalised (wasmi's `deterministic`), so the
//! same inputs and `now_ms` give the same outputs, scan after scan.
//!
//! **State.** An output keeps its last value until set again, as a
//! `VAR_OUTPUT` does. On a hot swap the module's exported mutable globals
//! and its outputs carry by name and type; linear memory starts again from
//! its data segments (a counter that must survive a swap lives in an
//! exported global).

use std::fmt;
use std::time::Duration;

use helmsman_iec::{Carry, Fault, FaultKind, Port, PortDir, SetError, Ty, Value};
use wasmi::{
    Caller, CompilationMode, Config, Engine, Error, ExternType, Instance, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder, TrapCode, TypedFunc, Val, ValType,
};

/// The interface version's module name; also the prefix of the port modules.
pub const ABI: &str = "helmsman/v1";
/// Import module of input ports.
pub const INPUTS: &str = "helmsman/v1/in";
/// Import module of output ports.
pub const OUTPUTS: &str = "helmsman/v1/out";
/// The scan time import, in [`ABI`].
pub const NOW: &str = "now_ms";
/// The export run once per scan.
pub const SCAN: &str = "scan";
/// Fuel per scan by default: the same order as ST's instruction budget.
pub const DEFAULT_FUEL: u64 = 1_000_000;
/// Linear memory cap by default, in bytes (64 pages: room for a Rust
/// module's default 1 MiB stack and its data).
pub const DEFAULT_MEMORY: usize = 4 << 20;
/// Most table elements a module may have.
const MAX_TABLE: usize = 10_000;

/// Fuel and memory for one module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Fuel per scan.
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

/// A port's or global's value as the host holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cell {
    Bool(bool),
    Int(i64),
    Real(f64),
}

impl Cell {
    const fn zero(ty: ValType) -> Self {
        match ty {
            ValType::I32 => Self::Bool(false),
            ValType::F64 => Self::Real(0.0),
            _ => Self::Int(0),
        }
    }

    const fn st(self) -> Value {
        match self {
            Self::Bool(b) => Value::Bool(b),
            Self::Int(n) => Value::Int(n as i128),
            Self::Real(x) => Value::Real(x),
        }
    }
}

/// The store's data: what the imports read and write.
struct Host {
    inputs: Vec<Cell>,
    outputs: Vec<Cell>,
    now_ms: i64,
    /// Set by an output import refusing its value; the trap it raises is
    /// then an `OutputRange` fault.
    bad_output: bool,
    limits: StoreLimits,
}

/// A port: its ST view, and where its value is in [`Host`].
#[derive(Debug, Clone)]
struct Slot {
    port: Port,
    /// The import's own spelling: the linker matches names exactly.
    raw: String,
    val: ValType,
    /// Index into `Host::inputs` or `Host::outputs`, by direction.
    at: usize,
}

/// An exported global's value, for carrying state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Raw {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
}

impl Raw {
    fn of(v: &Val) -> Option<Self> {
        match v {
            Val::I32(n) => Some(Self::I32(*n)),
            Val::I64(n) => Some(Self::I64(*n)),
            Val::F32(x) => Some(Self::F32(x.to_bits())),
            Val::F64(x) => Some(Self::F64(x.to_bits())),
            _ => None,
        }
    }

    fn val(self) -> Val {
        match self {
            Self::I32(n) => Val::I32(n),
            Self::I64(n) => Val::I64(n),
            Self::F32(b) => Val::F32(f32::from_bits(b).into()),
            Self::F64(b) => Val::F64(f64::from_bits(b).into()),
        }
    }

    fn st(self) -> Value {
        match self {
            Self::I32(n) => Value::Int(n.into()),
            Self::I64(n) => Value::Int(n.into()),
            Self::F32(b) => Value::Real(f32::from_bits(b).into()),
            Self::F64(b) => Value::Real(f64::from_bits(b)),
        }
    }

    const fn same_type(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::I32(_), Self::I32(_))
                | (Self::I64(_), Self::I64(_))
                | (Self::F32(_), Self::F32(_))
                | (Self::F64(_), Self::F64(_))
        )
    }
}

/// A loaded WASM module, instantiated and ready to scan.
pub struct WasmProgram {
    module: Module,
    slots: Vec<Slot>,
    /// Exported mutable globals, by name: the state that carries.
    globals: Vec<String>,
    limits: Limits,
    store: Store<Host>,
    instance: Instance,
    scan: TypedFunc<(), ()>,
}

impl fmt::Debug for WasmProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmProgram")
            .field("ports", &self.slots.iter().map(|s| &s.port).collect::<Vec<_>>())
            .field("globals", &self.globals)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Clone for WasmProgram {
    /// A new instance of the same module with this one's inputs, outputs and
    /// exported globals; its linear memory starts from the data segments.
    fn clone(&self) -> Self {
        let mut copy = Self::instantiate(
            &self.module,
            self.slots.clone(),
            self.globals.clone(),
            self.limits,
        )
        .expect("instantiated once with the same module and limits");
        copy.restore(self);
        copy.store.data_mut().inputs.clone_from(&self.store.data().inputs);
        copy
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The ST type a port of this value type has.
const fn st_ty(v: ValType) -> Option<Ty> {
    match v {
        ValType::I32 => Some(Ty::Bool),
        ValType::I64 => Some(Ty::LInt),
        ValType::F64 => Some(Ty::LReal),
        _ => None,
    }
}

fn engine() -> Engine {
    let mut config = Config::default();
    // Eager: translation happens at load, never in a scan, so the first
    // scan costs what every other does and a bad body is a load error.
    config
        .compilation_mode(CompilationMode::Eager)
        .consume_fuel(true)
        .allow_start_fn(false)
        .wasm_multi_memory(false);
    Engine::new(&config)
}

impl WasmProgram {
    /// Validates `wasm` against the `helmsman/v1` interface and instantiates
    /// it under `limits`.
    ///
    /// # Errors
    /// Every problem found: not WebAssembly, an import outside the
    /// interface or of the wrong type, a port name that is no identifier or
    /// is used twice, no `scan` export of type `() -> ()`, or an
    /// instantiation that fails (a memory larger than `limits.memory`, a
    /// data segment out of bounds).
    pub fn load(wasm: &[u8], limits: Limits) -> Result<Self, Vec<String>> {
        let module =
            Module::new(&engine(), wasm).map_err(|e| vec![format!("not a valid WASM module: {e}")])?;
        let mut errs = Vec::new();
        let mut slots: Vec<Slot> = Vec::new();
        let (mut ins, mut outs) = (0, 0);
        for import in module.imports() {
            let (m, name) = (import.module(), import.name());
            let what = format!("import {m:?} {name:?}");
            let ExternType::Func(func) = import.ty() else {
                errs.push(format!("{what}: only functions of {ABI} may be imported"));
                continue;
            };
            let (params, results) = (func.params(), func.results());
            match m {
                ABI if name == NOW => {
                    if !(params.is_empty() && results == [ValType::I64]) {
                        errs.push(format!("{what}: must be () -> i64"));
                    }
                }
                INPUTS | OUTPUTS => {
                    let dir = if m == INPUTS {
                        PortDir::Input
                    } else {
                        PortDir::Output
                    };
                    let val = match (dir, params, results) {
                        (PortDir::Input, [], [v]) | (PortDir::Output, [v], []) => *v,
                        _ => {
                            let want = if dir == PortDir::Input {
                                "() -> i32 / i64 / f64"
                            } else {
                                "(i32 / i64 / f64) -> ()"
                            };
                            errs.push(format!("{what}: must be {want}"));
                            continue;
                        }
                    };
                    let Some(ty) = st_ty(val) else {
                        errs.push(format!("{what}: a port is i32 (BOOL), i64 (LINT) or f64 (LREAL)"));
                        continue;
                    };
                    if !is_ident(name) {
                        errs.push(format!("{what}: a port name is an identifier"));
                        continue;
                    }
                    let upper = name.to_ascii_uppercase();
                    if slots.iter().any(|s| s.port.name == upper) {
                        errs.push(format!("{what}: port {name} is imported twice"));
                        continue;
                    }
                    let at = if dir == PortDir::Input {
                        &mut ins
                    } else {
                        &mut outs
                    };
                    slots.push(Slot {
                        port: Port { name: upper, dir, ty },
                        raw: name.to_string(),
                        val,
                        at: *at,
                    });
                    *at += 1;
                }
                _ => errs.push(format!(
                    "{what}: not part of the {ABI} interface (no WASI, clocks, network or files)"
                )),
            }
        }
        let mut globals = Vec::new();
        let mut scan = false;
        for export in module.exports() {
            match export.ty() {
                ExternType::Func(f) if export.name() == SCAN => {
                    scan = f.params().is_empty() && f.results().is_empty();
                }
                ExternType::Global(g) if g.mutability().is_mut() => globals.push(export.name().to_string()),
                _ => {}
            }
        }
        if !scan {
            errs.push(format!("no export {SCAN:?} of type () -> ()"));
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        Self::instantiate(&module, slots, globals, limits).map_err(|e| vec![e])
    }

    fn instantiate(
        module: &Module,
        slots: Vec<Slot>,
        globals: Vec<String>,
        limits: Limits,
    ) -> Result<Self, String> {
        let count = |d: PortDir| slots.iter().filter(|s| s.port.dir == d).count();
        let host = Host {
            inputs: slots
                .iter()
                .filter(|s| s.port.dir == PortDir::Input)
                .map(|s| Cell::zero(s.val))
                .collect(),
            outputs: slots
                .iter()
                .filter(|s| s.port.dir == PortDir::Output)
                .map(|s| Cell::zero(s.val))
                .collect(),
            now_ms: 0,
            bad_output: false,
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory)
                .memories(1)
                .tables(1)
                .table_elements(MAX_TABLE)
                .instances(1)
                .build(),
        };
        debug_assert_eq!(host.inputs.len(), count(PortDir::Input));
        let mut store = Store::new(module.engine(), host);
        store.limiter(|h| &mut h.limits);
        store.set_fuel(limits.fuel).expect("the engine meters fuel");
        let mut linker = Linker::<Host>::new(module.engine());
        linker
            .func_wrap(ABI, NOW, |c: Caller<'_, Host>| -> i64 { c.data().now_ms })
            .map_err(|e| e.to_string())?;
        for s in &slots {
            let (raw, k) = (s.raw.as_str(), s.at);
            let defined = match (s.port.dir, s.val) {
                (PortDir::Input, ValType::I32) => {
                    linker.func_wrap(INPUTS, raw, move |c: Caller<'_, Host>| -> i32 {
                        i32::from(c.data().inputs[k] == Cell::Bool(true))
                    })
                }
                (PortDir::Input, ValType::I64) => {
                    linker.func_wrap(INPUTS, raw, move |c: Caller<'_, Host>| -> i64 {
                        match c.data().inputs[k] {
                            Cell::Int(n) => n,
                            _ => 0,
                        }
                    })
                }
                (PortDir::Input, _) => linker.func_wrap(INPUTS, raw, move |c: Caller<'_, Host>| -> f64 {
                    match c.data().inputs[k] {
                        Cell::Real(x) => x,
                        _ => 0.0,
                    }
                }),
                (PortDir::Output, ValType::I32) => linker.func_wrap(
                    OUTPUTS,
                    raw,
                    move |mut c: Caller<'_, Host>, v: i32| -> Result<(), Error> {
                        let b = match v {
                            0 => false,
                            1 => true,
                            _ => {
                                c.data_mut().bad_output = true;
                                return Err(Error::new("a BOOL output takes 0 or 1"));
                            }
                        };
                        c.data_mut().outputs[k] = Cell::Bool(b);
                        Ok(())
                    },
                ),
                (PortDir::Output, ValType::I64) => {
                    linker.func_wrap(OUTPUTS, raw, move |mut c: Caller<'_, Host>, v: i64| {
                        c.data_mut().outputs[k] = Cell::Int(v);
                    })
                }
                (PortDir::Output, _) => {
                    linker.func_wrap(OUTPUTS, raw, move |mut c: Caller<'_, Host>, v: f64| {
                        c.data_mut().outputs[k] = Cell::Real(v);
                    })
                }
            };
            defined.map_err(|e| e.to_string())?;
        }
        let instance = linker
            .instantiate_and_start(&mut store, module)
            .map_err(|e| format!("does not instantiate: {e}"))?;
        let scan = instance
            .get_typed_func::<(), ()>(&store, SCAN)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            module: module.clone(),
            slots,
            globals,
            limits,
            store,
            instance,
            scan,
        })
    }

    /// The module's ports, in import order, names upper-cased.
    #[must_use]
    pub fn ports(&self) -> Vec<Port> {
        self.slots.iter().map(|s| s.port.clone()).collect()
    }

    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    fn slot(&self, name: &str, dir: Option<PortDir>) -> Option<&Slot> {
        self.slots
            .iter()
            .find(|s| s.port.name.eq_ignore_ascii_case(name) && dir.is_none_or(|d| s.port.dir == d))
    }

    /// The cell `v` would be on input `name`, as [`helmsman_iec::Program::set_input`]
    /// checks it: the same kind, in range, finite.
    fn cell(&self, name: &str, v: Value) -> Result<(usize, Cell), SetError> {
        let s = self
            .slot(name, Some(PortDir::Input))
            .ok_or(SetError::NoSuchInput)?;
        let ty = s.port.ty;
        let cell = match (s.val, v) {
            (ValType::I32, Value::Bool(b)) => Cell::Bool(b),
            (ValType::I64, Value::Int(n)) => Cell::Int(i64::try_from(n).map_err(|_| SetError::Range(ty))?),
            (ValType::F64, Value::Real(x)) if x.is_finite() => Cell::Real(x),
            (ValType::F64, Value::Real(_)) => return Err(SetError::Range(ty)),
            _ => return Err(SetError::Kind(ty)),
        };
        Ok((s.at, cell))
    }

    /// Whether [`WasmProgram::set_input`] would take `v` on `name`.
    #[must_use]
    pub fn accepts(&self, name: &str, v: Value) -> bool {
        self.cell(name, v).is_ok()
    }

    /// Sets input port `name` (case-insensitive) before a scan.
    ///
    /// # Errors
    /// No such input, or a value of the wrong kind or out of range; the
    /// input keeps its previous value.
    pub fn set_input(&mut self, name: &str, v: Value) -> Result<(), SetError> {
        let (k, cell) = self.cell(name, v)?;
        self.store.data_mut().inputs[k] = cell;
        Ok(())
    }

    /// A port's value (case-insensitive), or an exported global's (exact
    /// name), as an ST value.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(s) = self.slot(name, None) {
            let h = self.store.data();
            let cells = if s.port.dir == PortDir::Input {
                &h.inputs
            } else {
                &h.outputs
            };
            return Some(cells[s.at].st());
        }
        let g = self.instance.get_global(&self.store, name)?;
        Raw::of(&g.get(&self.store)).map(Raw::st)
    }

    /// Runs `scan` once with `now` as its time, under the fuel limit.
    ///
    /// # Errors
    /// The fault the trap maps to (module docs).
    pub fn scan_at(&mut self, now: Duration) -> Result<(), Fault> {
        let h = self.store.data_mut();
        h.now_ms = i64::try_from(now.as_millis()).unwrap_or(i64::MAX);
        h.bad_output = false;
        self.store
            .set_fuel(self.limits.fuel)
            .expect("the engine meters fuel");
        self.scan
            .call(&mut self.store, ())
            .map_err(|e| fault(&e, self.store.data().bad_output))
    }

    /// A cold restart: a fresh instance of the same module.
    pub fn reset(&mut self) {
        *self = Self::instantiate(
            &self.module,
            self.slots.clone(),
            self.globals.clone(),
            self.limits,
        )
        .expect("instantiated once with the same module and limits");
    }

    fn global_values(&self) -> Vec<(String, Raw)> {
        self.globals
            .iter()
            .filter_map(|n| {
                let g = self.instance.get_global(&self.store, n)?;
                Raw::of(&g.get(&self.store)).map(|r| (n.clone(), r))
            })
            .collect()
    }

    /// Names of the state that carries: outputs, then exported globals.
    pub(crate) fn state_names(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter(|s| s.port.dir == PortDir::Output)
            .map(|s| s.port.name.clone())
            .chain(self.globals.iter().cloned())
            .collect()
    }

    /// Carries `old`'s outputs and exported mutable globals into this one by
    /// name and type; everything else starts at its initial value.
    pub fn restore(&mut self, old: &Self) -> Carry {
        let mut carry = Carry {
            kept: Vec::new(),
            reset: Vec::new(),
            dropped: Vec::new(),
        };
        for s in self.slots.iter().filter(|s| s.port.dir == PortDir::Output) {
            match old.slot(&s.port.name, Some(PortDir::Output)) {
                Some(o) if o.val == s.val => {
                    self.store.data_mut().outputs[s.at] = old.store.data().outputs[o.at];
                    carry.kept.push(s.port.name.clone());
                }
                _ => carry.reset.push(s.port.name.clone()),
            }
        }
        let before = old.global_values();
        for (name, now) in self.global_values() {
            let kept = before.iter().find(|(n, _)| *n == name).is_some_and(|(_, v)| {
                v.same_type(now)
                    && self
                        .instance
                        .get_global(&self.store, &name)
                        .is_some_and(|g| g.set(&mut self.store, v.val()).is_ok())
            });
            if kept {
                carry.kept.push(name);
            } else {
                carry.reset.push(name);
            }
        }
        carry.dropped = old
            .state_names()
            .into_iter()
            .filter(|n| !carry.kept.contains(n) && !carry.reset.contains(n))
            .collect();
        carry
    }
}

/// The fault a failed scan is.
fn fault(e: &Error, bad_output: bool) -> Fault {
    let kind = if bad_output {
        FaultKind::OutputRange
    } else {
        match e.as_trap_code() {
            Some(TrapCode::OutOfFuel) => FaultKind::Watchdog,
            Some(TrapCode::IntegerDivisionByZero) => FaultKind::DivisionByZero,
            Some(TrapCode::MemoryOutOfBounds | TrapCode::TableOutOfBounds) => FaultKind::IndexOutOfRange,
            _ => FaultKind::Trap,
        }
    };
    Fault { kind, line: 0 }
}
