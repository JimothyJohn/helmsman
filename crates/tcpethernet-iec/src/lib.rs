//! IEC 61131-3 Structured Text: compile a program, then run it one scan at
//! a time against a process image.
//!
//! ```
//! use std::time::Duration;
//! use tcpethernet_iec::{Image, ImageSizes, Value, compile};
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
//! (`AT %IX0.1`) and direct addresses; partial bit access (`w.3`); the full
//! ST statement set; conversions; the standard functions (ABS, SQRT, LN,
//! LOG, EXP, trigonometry, EXPT, MIN, MAX, LIMIT, SEL, MUX, SHL, SHR, ROL,
//! ROR, TRUNC, MOVE) and function blocks (TON, TOF, TP, CTU, CTD, CTUD,
//! `R_TRIG`, `F_TRIG`, SR, RS). Not yet: STRING, `VAR_IN_OUT`, `VAR_GLOBAL`,
//! function outputs, pointers and references.

mod check;
mod interp;
mod lexer;
mod parser;
mod types;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub use check::{Area, Diagnostic, ImageSizes};
pub use interp::{Fault, FaultKind, Image};
pub use lexer::Pos;
pub use types::{Ty, Value};

use check::{AggKind, Checked, Type};
use interp::Machine;

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
    let code = check::check(library(), &unit, sizes)?;
    Ok(Program {
        sizes,
        mem: code.mem_init.clone(),
        code,
        budget: DEFAULT_BUDGET,
        started: None,
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
    /// read it). `now` should not go backwards.
    ///
    /// On a fault the variables are left as the fault found them;
    /// [`Program::reset`] restores the initial values.
    ///
    /// # Errors
    /// A [`Fault`]: division by zero, an index out of range, or the watchdog.
    pub fn scan_at(&mut self, image: &mut Image, now: Duration) -> Result<(), Fault> {
        let program = &self.code.pous[self.code.program];
        let mut m = Machine {
            mem: &mut self.mem,
            image,
            pous: &self.code.pous,
            base: 0,
            budget: self.budget,
            now: i128::try_from(now.as_nanos())
                .unwrap_or(i128::MAX)
                .min(i128::from(i64::MAX)),
        };
        m.run(program)
    }

    /// Restores every variable to its initial value (a cold restart).
    pub fn reset(&mut self) {
        self.mem.clone_from(&self.code.mem_init);
        self.started = None;
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

    /// Declared elementary type of a variable path, if it has one.
    #[must_use]
    pub fn ty(&self, path: &str) -> Option<Ty> {
        match self.resolve(path)?.1 {
            Type::Elem(t) => Some(t),
            _ => None,
        }
    }
}

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
