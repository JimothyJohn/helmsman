//! IEC 61131-3 Structured Text: compile a program, then run it one scan at
//! a time against a process image.
//!
//! ```
//! use std::time::Duration;
//! use helmsman_iec::{Image, ImageSizes, Value, compile};
//!
//! let src = "
//! PROGRAM blink
//! VAR pulse : TON; on : BOOL; END_VAR
//!   pulse(IN := NOT pulse.Q, PT := T#500ms);
//!   IF pulse.Q THEN on := NOT on; END_IF;
//!   %QX0.0 := on;
//! END_PROGRAM";
//! let sizes = ImageSizes { i: 32, q: 32, m: 64 };
//! let mut p = compile(src, sizes).unwrap();
//! let mut img = Image::new(sizes);
//! // 10 ms scans: the timer fires at 500 ms, restarts a scan later, fires at 1020 ms.
//! for ms in (0..=1100).step_by(10) {
//!     p.scan_at(&mut img, Duration::from_millis(ms)).unwrap();
//! }
//! assert_eq!(p.get("on"), Some(Value::Bool(false)), "toggled twice");
//! assert_eq!(img.q[0] & 1, 0);
//! ```
//!
//! Covered: PROGRAM, FUNCTION and `FUNCTION_BLOCK` units (`VAR`, `VAR_TEMP`,
//! `VAR_INPUT`, `VAR_OUTPUT`, `VAR CONSTANT`); TYPE declarations (STRUCT,
//! enumerations, aliases) and ARRAYs; the elementary types BOOL, SINT..LINT,
//! USINT..ULINT, BYTE..LWORD, REAL, LREAL and TIME; located variables
//! (`AT %IX0.1`, with an initial value written at a cold start) and direct
//! addresses; partial bit access (`w.3`); the full ST statement set;
//! conversions; the standard functions (ABS, SQRT, LN, LOG, EXP,
//! trigonometry, EXPT, MIN, MAX, LIMIT, SEL, MUX, SHL, SHR, ROL, ROR, TRUNC,
//! MOVE) and function blocks (TON, TOF, TP, CTU, CTD, CTUD, `R_TRIG`,
//! `F_TRIG`, SR, RS, and `BLINK_PATTERN`, a stack light's patterns that
//! IEC 61131-3 does not have; see library.st: a blinking MODE with a
//! PERIOD of 0 or below faults the module, [`BadParam::BlinkPeriod`]). Pragmas (`{...}`) are skipped. Not yet: STRING, DATE,
//! DT and TOD, `VAR_IN_OUT`, `VAR_GLOBAL`, function outputs, pointers and
//! references, multi-level addresses (`%QW0.1.2`). Nesting is capped at
//! `MAX_NESTING` levels so hostile input cannot overflow the stack.
//!
//! Ladder (LD) and function block diagrams (FBD) drawn in an editor such
//! as Beremiz or the `OpenPLC` Editor come in as `PLCopen` TC6 XML through
//! [`compile_plcopen`].

/// The syntax tree of a compilation unit, read-only, for views built from
/// the source (the ST diagram, `helmsman-diagram`). Statement positions are
/// the ones [`trace::StmtInfo`] carries, so a view finds a statement's
/// [`trace::StmtId`] by POU, line and column.
pub mod ast {
    pub use crate::lexer::Pos;
    pub use crate::parser::{
        Arg, BinOp, Block, CaseArm, CaseLabel, Expr, ExprKind, Field, Init, LabelValue, Pou, PouKind,
        Section, Stmt, StmtKind, TypeDecl, TypeDef, TypeRef, UnOp, Unit, VarDecl,
    };

    /// Parses a unit: syntax only. [`crate::compile`] parses and checks.
    ///
    /// # Errors
    /// The first syntax error.
    pub fn parse(src: &str) -> Result<Unit, crate::Diagnostic> {
        crate::parser::parse(src, false).map_err(|e| crate::Diagnostic {
            pos: e.pos,
            msg: e.msg,
        })
    }

    /// The standard function blocks ([`crate::LIBRARY`]), parsed.
    #[must_use]
    pub fn library() -> &'static Unit {
        crate::library()
    }
}

mod check;
mod interp;
pub mod l5x;
mod lexer;
mod parser;
pub mod plcopen;
mod state;
pub mod trace;
mod types;
mod xml;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub use check::{Area, Diagnostic, ImageSizes};
pub use interp::{BadParam, Fault, FaultKind, Image};
pub use lexer::Pos;
pub use state::{Carry, State};
pub use types::{Ty, Value};

use check::{AggKind, Checked, Type};
use interp::Machine;
use parser::Section;
use trace::{Recorder, Trace, TraceMap};

/// Instructions one scan may execute before it faults (see [`FaultKind::Watchdog`]).
pub const DEFAULT_BUDGET: u64 = 1_000_000;

/// Source of the standard function blocks.
pub const LIBRARY: &str = include_str!("library.st");

fn library() -> &'static parser::Unit {
    static LIB: OnceLock<parser::Unit> = OnceLock::new();
    LIB.get_or_init(|| {
        parser::parse(LIBRARY, true).unwrap_or_else(|e| panic!("library.st {}: {}", e.pos, e.msg))
    })
}

impl Image {
    /// A zeroed image of the given sizes.
    #[must_use]
    pub fn new(sizes: ImageSizes) -> Self {
        Self {
            i: vec![0; sizes.i],
            q: vec![0; sizes.q],
            m: vec![0; sizes.m],
        }
    }
}

/// A compiled program and its variables.
#[derive(Debug, Clone)]
pub struct Program {
    sizes: ImageSizes,
    code: Checked,
    mem: Vec<Value>,
    budget: u64,
    started: Option<Instant>,
    /// Located initial values are still to be written (a cold start).
    cold: bool,
    /// Trace counters per scan (see [`trace`]).
    counters: u32,
    /// Built on the first traced scan; shared by its clones.
    map: OnceLock<Arc<TraceMap>>,
}

/// Compiles a unit holding one `PROGRAM` (plus any FUNCTIONs,
/// `FUNCTION_BLOCK`s and TYPEs it uses) against an image of `sizes`.
///
/// # Errors
/// Every syntax or type error found, sorted by position.
pub fn compile(src: &str, sizes: ImageSizes) -> Result<Program, Vec<Diagnostic>> {
    let unit = parser::parse(src, false).map_err(|e| {
        vec![Diagnostic {
            pos: e.pos,
            msg: e.msg,
        }]
    })?;
    let mut code = check::check(library(), &unit, sizes)?;
    let counters = trace::number(&mut code);
    Ok(Program {
        sizes,
        mem: code.mem_init.clone(),
        code,
        budget: DEFAULT_BUDGET,
        started: None,
        cold: true,
        counters,
        map: OnceLock::new(),
    })
}

/// Compiles a `PLCopen` TC6 XML project (see [`plcopen`]) holding one
/// `PROGRAM`, against an image of `sizes`.
///
/// # Errors
/// Every problem found, positioned in the XML.
pub fn compile_plcopen(xml: &str, sizes: ImageSizes) -> Result<Program, Vec<Diagnostic>> {
    let lowered = plcopen::lower(xml)?;
    compile(lowered.st(), sizes).map_err(|diags| {
        diags
            .into_iter()
            .map(|d| Diagnostic {
                pos: lowered.origin(d.pos),
                msg: d.msg,
            })
            .collect()
    })
}

impl Program {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.code.pous[self.code.program].name
    }

    #[must_use]
    pub const fn sizes(&self) -> ImageSizes {
        self.sizes
    }

    /// Changes the per-scan instruction budget.
    pub const fn set_budget(&mut self, budget: u64) {
        self.budget = budget;
    }

    /// Runs one scan; the clock is wall time since the first scan.
    ///
    /// # Errors
    /// A [`Fault`]; see [`Program::scan_at`].
    pub fn scan(&mut self, image: &mut Image) -> Result<(), Fault> {
        let now = self.started.get_or_insert_with(Instant::now).elapsed();
        self.scan_at(image, now)
    }

    /// Runs one scan with the clock at `now` since program start (timers
    /// read it). `now` should not go backwards. The first scan after
    /// compiling or [`Program::reset`] first writes located variables'
    /// initial values (`q AT %QX0.3 : BOOL := TRUE`) into `image`.
    ///
    /// On a fault the variables are left as the fault found them;
    /// [`Program::reset`] restores the initial values.
    ///
    /// # Errors
    /// A [`Fault`]: division by zero, an index out of range, the watchdog,
    /// or a library block refusing a parameter ([`FaultKind::BadParam`]).
    pub fn scan_at(&mut self, image: &mut Image, now: Duration) -> Result<(), Fault> {
        self.run(image, now, None)
    }

    /// [`Program::scan_at`], recording what the scan did. The result and
    /// every variable are exactly what the untraced scan would leave; the
    /// trace is the scan up to its end or its fault.
    pub fn scan_traced(&mut self, image: &mut Image, now: Duration) -> (Result<(), Fault>, Trace) {
        let mut rec = Recorder::new(self.counters);
        let r = self.run(image, now, Some(&mut rec));
        let prog = &self.code.pous[self.code.program];
        let frame = self.mem[prog.base..prog.base + prog.size].to_vec();
        let trace = Trace::new(self.trace_map(), now, rec, frame, image.clone(), r.err());
        (r, trace)
    }

    /// The statement table and variable list every trace of this program
    /// shares; built on first use.
    #[must_use]
    pub fn trace_map(&self) -> Arc<TraceMap> {
        self.map
            .get_or_init(|| Arc::new(TraceMap::build(&self.code)))
            .clone()
    }

    fn run(&mut self, image: &mut Image, now: Duration, tap: Option<&mut Recorder>) -> Result<(), Fault> {
        if self.cold {
            interp::init_located(image, &self.code.loc_init);
            self.cold = false;
        }
        let program = &self.code.pous[self.code.program];
        let mut m = Machine {
            mem: &mut self.mem,
            image,
            pous: &self.code.pous,
            library: self.code.library,
            base: 0,
            budget: self.budget,
            now: i128::try_from(now.as_nanos())
                .unwrap_or(i128::MAX)
                .min(i128::from(i64::MAX)),
            tap,
        };
        m.run(program)
    }

    /// Restores every variable to its initial value (a cold restart); the
    /// next scan rewrites located initial values into the image.
    pub fn reset(&mut self) {
        self.mem.clone_from(&self.code.mem_init);
        self.started = None;
        self.cold = true;
    }

    /// Resolves a variable path such as `t.ET`, `grid[1,2]` or `p.x` to a
    /// slot and type. Function block internals are visible here (for
    /// inspection), unlike in programs.
    fn resolve(&self, path: &str) -> Option<(usize, Type)> {
        let path = path.to_ascii_uppercase();
        let mut parts = path.split('.');
        let first = parts.next()?;
        let (name, mut idx) = split_index(first)?;
        let prog = &self.code.pous[self.code.program];
        let v = prog.vars.iter().find(|v| v.name == name && v.loc.is_none())?;
        let (mut slot, mut ty) = (prog.base + v.off, v.ty);
        let mut rest = parts.peekable();
        loop {
            if let Some(ix) = idx.take() {
                let Type::Agg(a) = ty else { return None };
                let AggKind::Array { elem, dims } = &self.code.aggs[a].kind else {
                    return None;
                };
                if ix.len() != dims.len() {
                    return None;
                }
                let esize = match elem {
                    Type::Agg(e) => self.code.aggs[*e].size,
                    _ => 1,
                };
                let mut stride = esize;
                for (k, &(lo, hi)) in dims.iter().enumerate().rev() {
                    let i = ix[k];
                    if i < lo || i > hi {
                        return None;
                    }
                    slot += usize::try_from(i - lo).ok()? * stride;
                    stride *= usize::try_from(hi - lo + 1).ok()?;
                }
                ty = *elem;
            }
            let Some(seg) = rest.next() else {
                return Some((slot, ty));
            };
            let (m, ix) = split_index(seg)?;
            idx = ix;
            let Type::Agg(a) = ty else { return None };
            match &self.code.aggs[a].kind {
                AggKind::Struct { fields } => {
                    let f = fields.iter().find(|f| f.name == m)?;
                    slot += f.off;
                    ty = f.ty;
                }
                AggKind::Fb(fb) => {
                    let f = self.code.pous[*fb].vars.iter().find(|v| v.name == m)?;
                    slot += f.off;
                    ty = f.ty;
                }
                AggKind::Array { .. } => return None,
            }
        }
    }

    /// Current value of a scalar variable, by path (`count`, `t.ET`,
    /// `grid[1,2]`, `p.x`; case-insensitive). Enumerations read as their
    /// integer value. Located variables live in the image: `None`.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<Value> {
        let (slot, ty) = self.resolve(path)?;
        matches!(ty, Type::Elem(_) | Type::Enum(_)).then(|| self.mem[slot])
    }

    /// The program's ports: its `VAR_INPUT`s and `VAR_OUTPUT`s, in
    /// declaration order, names upper-cased. A host binds them to signals.
    #[must_use]
    pub fn ports(&self) -> Vec<Port> {
        let prog = &self.code.pous[self.code.program];
        prog.vars
            .iter()
            .filter_map(|v| {
                let dir = match v.section {
                    Section::Input => PortDir::Input,
                    Section::Output => PortDir::Output,
                    _ => return None,
                };
                let Type::Elem(ty) = v.ty else { return None };
                Some(Port {
                    name: v.name.clone(),
                    dir,
                    ty,
                })
            })
            .collect()
    }

    /// Sets a `VAR_INPUT` (case-insensitive) before a scan. The value must
    /// already be the input's kind and fit its range: nothing is wrapped,
    /// truncated or converted, and REALs must be finite.
    ///
    /// # Errors
    /// No such input, or a value of the wrong kind or out of range; the
    /// input keeps its previous value.
    pub fn set_input(&mut self, name: &str, value: Value) -> Result<(), SetError> {
        let name = name.to_ascii_uppercase();
        let prog = &self.code.pous[self.code.program];
        let Some(v) = prog
            .vars
            .iter()
            .find(|v| v.name == name && v.section == Section::Input)
        else {
            return Err(SetError::NoSuchInput);
        };
        let (slot, Type::Elem(ty)) = (prog.base + v.off, v.ty) else {
            return Err(SetError::NoSuchInput);
        };
        let stored = match (ty, value) {
            (Ty::Bool, Value::Bool(_)) => value,
            (t, Value::Real(x)) if t.is_real() => {
                if !x.is_finite() {
                    return Err(SetError::Range(ty));
                }
                let r = types::round_real(t, x);
                if !r.is_finite() {
                    return Err(SetError::Range(ty));
                }
                Value::Real(r)
            }
            (t, Value::Int(n)) if t != Ty::Bool && !t.is_real() => {
                let (lo, hi) = t.range();
                if n < lo || n > hi {
                    return Err(SetError::Range(ty));
                }
                value
            }
            _ => return Err(SetError::Kind(ty)),
        };
        self.mem[slot] = stored;
        Ok(())
    }

    /// Declared elementary type of a variable path, if it has one.
    #[must_use]
    pub fn ty(&self, path: &str) -> Option<Ty> {
        match self.resolve(path)?.1 {
            Type::Elem(t) => Some(t),
            _ => None,
        }
    }
}

/// Which way a program port faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDir {
    /// `VAR_INPUT`: the host writes it before each scan.
    Input,
    /// `VAR_OUTPUT`: the host reads it after each scan.
    Output,
}

/// A program's `VAR_INPUT` or `VAR_OUTPUT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    /// Upper-cased, as ST names are case-insensitive.
    pub name: String,
    pub dir: PortDir,
    pub ty: Ty,
}

/// Why [`Program::set_input`] refused a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetError {
    /// The program has no `VAR_INPUT` of that name.
    NoSuchInput,
    /// A value of the wrong kind (BOOL, integer or real) for this type.
    Kind(Ty),
    /// Out of this type's range, or a non-finite real.
    Range(Ty),
}

impl std::fmt::Display for SetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchInput => f.write_str("no such VAR_INPUT"),
            Self::Kind(t) => write!(f, "wrong kind of value for {t}"),
            Self::Range(t) => write!(f, "value out of range for {t}"),
        }
    }
}

impl std::error::Error for SetError {}

/// `name[1,2]` -> ("NAME", Some([1, 2])); `name` -> ("NAME", None).
fn split_index(seg: &str) -> Option<(&str, Option<Vec<i128>>)> {
    match seg.split_once('[') {
        None => Some((seg, None)),
        Some((name, rest)) => {
            let inner = rest.strip_suffix(']')?;
            let ix = inner
                .split(',')
                .map(|s| s.trim().parse().ok())
                .collect::<Option<Vec<i128>>>()?;
            Some((name, Some(ix)))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn library_compiles() {
        let sizes = super::ImageSizes { i: 0, q: 0, m: 0 };
        super::compile("PROGRAM p END_PROGRAM", sizes).unwrap();
    }
}
