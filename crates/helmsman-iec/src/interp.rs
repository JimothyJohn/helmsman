//! Executes checked code against the slot array and a process image.
//!
//! Semantics the standard leaves to the implementation, chosen here, pinned
//! by tests and cross-checked against ruSTy (tests/oracle.rs):
//! - binary integer arithmetic is carried out at least 32 bits wide: SINT
//!   and INT as DINT, USINT and UINT as UDINT (unsigned stays unsigned, so
//!   `/` and `MOD` of a wrapped UINT difference are unsigned), wrapping at
//!   that width; a value is truncated to its variable's type when stored,
//!   so `d := i * 1000` with INT `i` and DINT `d` gives 100000. Unary minus
//!   works at its operand's width. Generic standard functions take their
//!   arguments as computed. Bit strings are not promoted. All of this
//!   matches ruSTy on 6000 random programs (tests/oracle.rs);
//! - integer `/` and `MOD` by zero fault; REAL division follows IEEE 754;
//! - REAL -> integer rounds half away from zero and saturates at the
//!   target's range (NaN gives 0); integer narrowing keeps the low bits;
//!   TRUNC truncates toward zero, also saturating;
//! - TIME <-> number conversions count milliseconds;
//! - `AND`/`OR` evaluate both operands (no short-circuit);
//! - FOR evaluates its bounds once and stops, rather than wrapping, when the
//!   next value would leave the control variable's range;
//! - an array index or MUX selector out of range faults;
//! - each scan has an instruction budget; exhausting it is a watchdog fault.

#![allow(clippy::many_single_char_names)]

use std::fmt;

use crate::check::{Area, Base, Builtin, Call, Ex, Loc, Move, Place, PouCode, PouType, St, StKind};
use crate::parser::{BinOp, UnOp};
use crate::trace::Recorder;
use crate::types::{Ty, Value, round_real};

/// The process image a program reads and writes, one buffer per area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub i: Vec<u8>,
    pub q: Vec<u8>,
    pub m: Vec<u8>,
}

impl Image {
    fn area(&self, a: Area) -> &[u8] {
        match a {
            Area::I => &self.i,
            Area::Q => &self.q,
            Area::M => &self.m,
        }
    }

    fn area_mut(&mut self, a: Area) -> &mut [u8] {
        match a {
            Area::I => &mut self.i,
            Area::Q => &mut self.q,
            Area::M => &mut self.m,
        }
    }

    /// Raw little-endian bits at `loc`. The checker guaranteed the bounds;
    /// a shorter image reads as zero.
    pub(crate) fn read(&self, loc: Loc) -> u64 {
        let buf = self.area(loc.area);
        if loc.bits == 1 {
            return buf.get(loc.byte).map_or(0, |b| u64::from((b >> loc.bit) & 1));
        }
        let n = (loc.bits / 8) as usize;
        let mut raw = [0u8; 8];
        if let Some(src) = buf.get(loc.byte..loc.byte + n) {
            raw[..n].copy_from_slice(src);
        }
        u64::from_le_bytes(raw)
    }

    fn write(&mut self, loc: Loc, raw: u64) {
        let buf = self.area_mut(loc.area);
        if loc.bits == 1 {
            if let Some(b) = buf.get_mut(loc.byte) {
                let m = 1u8 << loc.bit;
                *b = if raw & 1 == 1 { *b | m } else { *b & !m };
            }
            return;
        }
        let n = (loc.bits / 8) as usize;
        if let Some(dst) = buf.get_mut(loc.byte..loc.byte + n) {
            dst.copy_from_slice(&raw.to_le_bytes()[..n]);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Integer `/` or `MOD` by zero (or TIME divided by zero).
    DivisionByZero,
    /// An array index or MUX selector outside its range.
    IndexOutOfRange,
    /// The scan used up its instruction budget (an endless loop, usually).
    Watchdog,
    /// Raised by a host, not the interpreter: a `VAR_OUTPUT` holds a value
    /// its signal cannot carry (a non-finite REAL, an integer past 64 bits).
    /// `line` is 0.
    OutputRange,
    /// Raised by a host, not the interpreter: a WASM module trapped
    /// (`unreachable`, an integer overflow or bad conversion, a bad
    /// indirect call, a stack overflow, or an import it called wrongly).
    /// `line` is 0.
    Trap,
    /// A built-in library block was given a parameter it cannot work with
    /// (library.st raises it). `line` is the caller's: the statement that
    /// called the block.
    BadParam(BadParam),
}

/// Which parameter of which built-in library block was unusable
/// ([`FaultKind::BadParam`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadParam {
    /// `BLINK_PATTERN` in a blinking MODE (2, 3 or 4) with a PERIOD of 0,
    /// below 0, or too short to divide into the pattern's steps.
    BlinkPeriod,
}

impl BadParam {
    /// What the fault says, naming the block and the parameter.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::BlinkPeriod => {
                "BLINK_PATTERN: PERIOD is 0, negative or too short to blink (MODE 2, 3 or 4 needs a PERIOD above 0)"
            }
        }
    }
}

/// A runtime error. The program is in an undefined state afterwards; the
/// caller should stop it and put outputs in a safe state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub kind: FaultKind,
    /// Source line of the statement that faulted.
    pub line: u32,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            FaultKind::DivisionByZero => "integer division by zero",
            FaultKind::IndexOutOfRange => "array index or MUX selector out of range",
            FaultKind::Watchdog => "watchdog: scan exceeded its instruction budget",
            FaultKind::OutputRange => "an output's value does not fit its signal",
            FaultKind::Trap => "the WASM module trapped",
            FaultKind::BadParam(p) => p.text(),
        };
        write!(f, "line {}: {what}", self.line)
    }
}

enum Flow {
    Next,
    Exit,
    Continue,
    Return,
}

const NS_PER_MS: i128 = 1_000_000;

/// REAL -> integer: round half away from zero, saturate, NaN -> 0.
#[allow(clippy::cast_possible_truncation)]
fn real_to_int(r: f64, to: Ty, round: bool) -> i128 {
    let (lo, hi) = to.range();
    // `as` saturates at the i128 range and maps NaN to 0.
    let v = (if round { r.round() } else { r.trunc() }) as i128;
    v.clamp(lo, hi)
}

/// Writes located variables' initial values (a cold start).
pub fn init_located(image: &mut Image, inits: &[(Loc, Ty, Value)]) {
    for &(loc, ty, v) in inits {
        image.write(loc, to_raw(ty, v));
    }
}

/// Value -> raw image bits for a store of type `ty`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_raw(ty: Ty, v: Value) -> u64 {
    match v {
        Value::Bool(b) => u64::from(b),
        Value::Int(i) => i as u64,
        Value::Real(r) if ty == Ty::Real => u64::from((r as f32).to_bits()),
        Value::Real(r) => r.to_bits(),
    }
}

/// Raw image bits -> value of type `ty`.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn from_raw(ty: Ty, raw: u64) -> Value {
    match ty {
        Ty::Bool => Value::Bool(raw & 1 == 1),
        Ty::Real => Value::Real(f64::from(f32::from_bits(raw as u32))),
        Ty::LReal => Value::Real(f64::from_bits(raw)),
        t => Value::Int(t.wrap(i128::from(raw))),
    }
}

/// Explicit or implicit conversion between elementary types.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn convert(v: Value, from: Ty, to: Ty) -> Value {
    if from == Ty::Time && to != Ty::Time {
        // TIME is counted in milliseconds when it becomes a number.
        let ns = int(v);
        return match to {
            Ty::Bool => Value::Bool(ns != 0),
            t if t.is_real() => Value::Real(round_real(t, ns as f64 / 1e6)),
            t => Value::Int(t.wrap(ns / NS_PER_MS)),
        };
    }
    if to == Ty::Time && from != Ty::Time {
        return Value::Int(Ty::Time.wrap(match v {
            Value::Bool(b) => i128::from(b) * NS_PER_MS,
            Value::Int(i) => i.wrapping_mul(NS_PER_MS),
            Value::Real(r) => (r * 1e6).round() as i128,
        }));
    }
    match (v, to) {
        (Value::Bool(b), Ty::Bool) => Value::Bool(b),
        (Value::Bool(b), t) if t.is_real() => Value::Real(if b { 1.0 } else { 0.0 }),
        (Value::Bool(b), _) => Value::Int(i128::from(b)),
        (Value::Int(i), Ty::Bool) => Value::Bool(i != 0),
        (Value::Int(i), t) if t.is_real() => Value::Real(round_real(t, i as f64)),
        (Value::Int(i), t) => Value::Int(t.wrap(i)),
        (Value::Real(r), Ty::Bool) => Value::Bool(r != 0.0),
        (Value::Real(r), t) if t.is_real() => Value::Real(round_real(t, r)),
        (Value::Real(r), t) => Value::Int(real_to_int(r, t, true)),
    }
}

fn int(v: Value) -> i128 {
    match v {
        Value::Int(i) => i,
        Value::Bool(b) => i128::from(b),
        Value::Real(_) => unreachable!("checked code never mixes REAL into integer ops"),
    }
}

fn real(v: Value) -> f64 {
    match v {
        Value::Real(r) => r,
        _ => unreachable!("checked code never mixes integers into REAL ops"),
    }
}

fn boolean(v: Value) -> bool {
    match v {
        Value::Bool(b) => b,
        _ => unreachable!("checked code only branches on BOOL"),
    }
}

/// Orders two values of the same type (NaN compares unordered).
fn compare(a: Value, b: Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(&y)),
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(&y)),
        (Value::Real(x), Value::Real(y)) => x.partial_cmp(&y),
        _ => unreachable!("checked code compares equal types"),
    }
}

fn binop(op: BinOp, a: Value, b: Value, ty: Ty) -> Result<Value, FaultKind> {
    use std::cmp::Ordering::{Equal, Greater, Less};
    let ord = || compare(a, b);
    let v = match op {
        BinOp::Eq => Value::Bool(ord() == Some(Equal)),
        BinOp::Ne => Value::Bool(ord() != Some(Equal)),
        BinOp::Lt => Value::Bool(ord() == Some(Less)),
        BinOp::Le => Value::Bool(matches!(ord(), Some(Less | Equal))),
        BinOp::Gt => Value::Bool(ord() == Some(Greater)),
        BinOp::Ge => Value::Bool(matches!(ord(), Some(Greater | Equal))),
        BinOp::And | BinOp::Or | BinOp::Xor if ty == Ty::Bool => {
            let (x, y) = (boolean(a), boolean(b));
            Value::Bool(match op {
                BinOp::And => x & y,
                BinOp::Or => x | y,
                _ => x ^ y,
            })
        }
        BinOp::And => Value::Int(int(a) & int(b)),
        BinOp::Or => Value::Int(int(a) | int(b)),
        BinOp::Xor => Value::Int(int(a) ^ int(b)),
        _ if ty.is_real() => {
            let (x, y) = (real(a), real(b));
            Value::Real(round_real(
                ty,
                match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    BinOp::Div => x / y,
                    BinOp::Pow => x.powf(y),
                    _ => unreachable!("MOD on REAL is rejected by the checker"),
                },
            ))
        }
        _ => {
            let (x, y) = (int(a), int(b));
            let out = match op {
                BinOp::Add => x.wrapping_add(y),
                BinOp::Sub => x.wrapping_sub(y),
                BinOp::Mul => x.wrapping_mul(y),
                BinOp::Div | BinOp::Mod if y == 0 => return Err(FaultKind::DivisionByZero),
                // Operands are at most 64 bits wide: no i128 overflow.
                BinOp::Div => x / y,
                BinOp::Mod => x % y,
                _ => unreachable!("** on integers is rejected by the checker"),
            };
            Value::Int(ty.arith().wrap(out))
        }
    };
    Ok(v)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn builtin(b: Builtin, args: &[Value], ty: Ty, now: i128) -> Result<Value, FaultKind> {
    let r = |f: fn(f64) -> f64| Value::Real(round_real(ty, f(real(args[0]))));
    let pick = |want: std::cmp::Ordering| {
        args[1..].iter().fold(
            args[0],
            |acc, &x| if compare(x, acc) == Some(want) { x } else { acc },
        )
    };
    Ok(match b {
        Builtin::Now => Value::Int(now),
        Builtin::Fault(p) => return Err(FaultKind::BadParam(p)),
        Builtin::Abs => match args[0] {
            Value::Real(x) => Value::Real(x.abs()),
            v => Value::Int(ty.arith().wrap(int(v).abs())),
        },
        Builtin::Sqrt => r(f64::sqrt),
        Builtin::Ln => r(f64::ln),
        Builtin::Log => r(f64::log10),
        Builtin::Exp => r(f64::exp),
        Builtin::Sin => r(f64::sin),
        Builtin::Cos => r(f64::cos),
        Builtin::Tan => r(f64::tan),
        Builtin::Asin => r(f64::asin),
        Builtin::Acos => r(f64::acos),
        Builtin::Atan => r(f64::atan),
        Builtin::Trunc => Value::Int(real_to_int(real(args[0]), Ty::DInt, false)),
        Builtin::Min => pick(std::cmp::Ordering::Less),
        Builtin::Max => pick(std::cmp::Ordering::Greater),
        Builtin::Limit => {
            let (mn, x, mx) = (args[0], args[1], args[2]);
            let low = if compare(x, mx) == Some(std::cmp::Ordering::Greater) {
                mx
            } else {
                x
            };
            if compare(low, mn) == Some(std::cmp::Ordering::Less) {
                mn
            } else {
                low
            }
        }
        Builtin::Sel => {
            if boolean(args[0]) {
                args[2]
            } else {
                args[1]
            }
        }
        Builtin::Mux => {
            let k = int(args[0]);
            let n = i128::try_from(args.len() - 1).unwrap_or(i128::MAX);
            if k < 0 || k >= n {
                return Err(FaultKind::IndexOutOfRange);
            }
            args[1 + usize::try_from(k).unwrap_or(0)]
        }
        Builtin::Shl | Builtin::Shr | Builtin::Rol | Builtin::Ror => {
            let bits = i128::from(ty.bits());
            let x = int(args[0]);
            let n = int(args[1]);
            let out = match b {
                Builtin::Shl | Builtin::Shr if n >= bits || n < 0 => 0,
                Builtin::Shl => x << n,
                Builtin::Shr => x >> n,
                _ => {
                    let n = n.rem_euclid(bits);
                    let n = if b == Builtin::Ror { (bits - n) % bits } else { n };
                    (x << n) | (x >> ((bits - n) % bits))
                }
            };
            Value::Int(ty.wrap(out))
        }
    })
}

/// Where a place lives after indices are applied.
#[derive(Debug, Clone, Copy)]
enum Target {
    Mem(usize),
    Image(Loc),
}

/// Interpreter state for one scan.
pub struct Machine<'a> {
    pub mem: &'a mut [Value],
    pub image: &'a mut Image,
    pub pous: &'a [PouCode],
    /// POUs `0..library` are the built-in library's: a fault inside one is
    /// reported at the line that called it, not a line of library.st.
    pub library: usize,
    /// The running POU's frame base.
    pub base: usize,
    pub budget: u64,
    /// Nanoseconds since the program started.
    pub now: i128,
    /// Records the scan when tracing; `None` for a plain scan, which then
    /// pays one predictable branch per statement.
    pub tap: Option<&'a mut Recorder>,
}

type Res<T> = Result<T, Fault>;

impl Machine<'_> {
    #[inline]
    fn enter(&mut self, tr: u32) -> u32 {
        match &mut self.tap {
            Some(r) => r.enter(tr),
            None => 0,
        }
    }

    #[inline]
    fn leave(&mut self, outer: u32) {
        if let Some(r) = &mut self.tap {
            r.cur = outer;
        }
    }

    #[inline]
    fn arm(&mut self, tr: u32, k: usize) {
        if let Some(r) = &mut self.tap {
            r.arm(tr, k);
        }
    }

    #[inline]
    fn pass(&mut self, tr: u32) {
        if let Some(r) = &mut self.tap {
            r.pass(tr);
        }
    }

    fn at(line: u32) -> impl Fn(FaultKind) -> Fault {
        move |kind| Fault { kind, line }
    }

    fn target(&mut self, p: &Place, line: u32) -> Res<Target> {
        let mut i = match p.base {
            Base::Image(loc) => return Ok(Target::Image(loc)),
            Base::Frame => self.base + p.off,
        };
        for d in &p.dims {
            let v = int(self.eval(&d.idx, line)?);
            if v < d.lo || v >= d.lo + d.len {
                return Err(Fault {
                    kind: FaultKind::IndexOutOfRange,
                    line,
                });
            }
            i += usize::try_from(v - d.lo).unwrap_or(0) * d.stride;
        }
        Ok(Target::Mem(i))
    }

    fn load(&mut self, p: &Place, ty: Ty, line: u32) -> Res<Value> {
        let t = self.target(p, line)?;
        if let Some((bit, cty)) = p.bit {
            let whole = match t {
                Target::Mem(i) => int(self.mem[i]),
                Target::Image(loc) => int(from_raw(cty, self.image.read(loc))),
            };
            return Ok(Value::Bool((whole >> bit) & 1 == 1));
        }
        Ok(match t {
            Target::Mem(i) => self.mem[i],
            Target::Image(loc) => from_raw(ty, self.image.read(loc)),
        })
    }

    fn store(&mut self, p: &Place, ty: Ty, v: Value, line: u32) -> Res<()> {
        let t = self.target(p, line)?;
        let v = if let Some((bit, cty)) = p.bit {
            let whole = match t {
                Target::Mem(i) => int(self.mem[i]),
                Target::Image(loc) => int(from_raw(cty, self.image.read(loc))),
            };
            let m = 1i128 << bit;
            let set = if boolean(v) { whole | m } else { whole & !m };
            Value::Int(cty.wrap(set))
        } else {
            v
        };
        let raw_ty = p.bit.map_or(ty, |(_, cty)| cty);
        // Promoted arithmetic may leave a value wider than its variable.
        let v = match v {
            Value::Int(x) if raw_ty.is_integral() || raw_ty == Ty::Time => Value::Int(raw_ty.wrap(x)),
            v => v,
        };
        match t {
            Target::Mem(i) => self.mem[i] = v,
            Target::Image(loc) => self.image.write(loc, to_raw(raw_ty, v)),
        }
        Ok(())
    }

    fn mem_index(&mut self, p: &Place, line: u32) -> Res<usize> {
        match self.target(p, line)? {
            Target::Mem(i) => Ok(i),
            Target::Image(_) => unreachable!("structured values are never located"),
        }
    }

    fn eval(&mut self, e: &Ex, line: u32) -> Res<Value> {
        let f = Self::at(line);
        Ok(match e {
            Ex::Const(v) => *v,
            Ex::Load(p, ty) => self.load(p, *ty, line)?,
            Ex::Conv(a, from, to) => convert(self.eval(a, line)?, *from, *to),
            Ex::Un(UnOp::Neg, a, ty) => match self.eval(a, line)? {
                Value::Real(r) => Value::Real(-r),
                // `.2` is the negation's width, chosen by the checker.
                v => Value::Int(ty.wrap(-int(v))),
            },
            Ex::Un(UnOp::Not, a, ty) => match self.eval(a, line)? {
                Value::Bool(b) => Value::Bool(!b),
                v => Value::Int(ty.wrap(!int(v))),
            },
            Ex::Bin(op, a, b, ty) => {
                let x = self.eval(a, line)?;
                let y = self.eval(b, line)?;
                binop(*op, x, y, *ty).map_err(&f)?
            }
            Ex::Scale(op, t, k) => {
                let ns = int(self.eval(t, line)?);
                let k = real(self.eval(k, line)?);
                #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
                let out = match op {
                    BinOp::Div if k == 0.0 => return Err(f(FaultKind::DivisionByZero)),
                    BinOp::Div => (ns as f64 / k).round() as i128,
                    _ => (ns as f64 * k).round() as i128,
                };
                Value::Int(Ty::Time.wrap(out))
            }
            Ex::Builtin(b, args, ty) => {
                // Generic standard functions see their arguments as computed
                // (possibly widened), like ruSTy's generic resolution.
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a, line)?);
                }
                builtin(*b, &vals, *ty, self.now).map_err(&f)?
            }
            Ex::Call(c) => self.call(c, line)?.unwrap_or(Value::Bool(false)),
        })
    }

    /// Runs a function or function block; returns a function's result.
    fn call(&mut self, c: &Call, line: u32) -> Res<Option<Value>> {
        let pous = self.pous;
        let pou = &pous[c.pou];
        // Arguments are evaluated in the caller's frame, before the callee's
        // frame is touched (so `f(f(1))` works).
        let mut staged: Vec<(usize, Vec<Value>)> = Vec::with_capacity(c.moves.len());
        for m in &c.moves {
            match m {
                Move::Value { off, ex, ty } => {
                    let v = match self.eval(ex, line)? {
                        Value::Int(x) => Value::Int(ty.wrap(x)),
                        v => v,
                    };
                    staged.push((*off, vec![v]));
                }
                Move::Copy { off, src, len } => {
                    let i = self.mem_index(src, line)?;
                    staged.push((*off, self.mem[i..i + len].to_vec()));
                }
            }
        }
        let callee = match &c.inst {
            Some(p) => self.mem_index(p, line)?,
            None => pou.base,
        };
        if pou.kind == PouType::Function {
            self.mem[callee..callee + pou.size].copy_from_slice(&pou.init);
        } else {
            for &(off, len) in &pou.temps {
                self.mem[callee + off..callee + off + len].copy_from_slice(&pou.init[off..off + len]);
            }
        }
        for (off, vals) in staged {
            self.mem[callee + off..callee + off + vals.len()].copy_from_slice(&vals);
        }
        let saved = self.base;
        self.base = callee;
        let r = self.block(&pou.body);
        self.base = saved;
        r.map_err(|f| {
            if c.pou < self.library {
                Fault { line, ..f }
            } else {
                f
            }
        })?;
        for o in &c.outs {
            if let Some((from, to)) = o.conv {
                let v = convert(self.mem[callee + o.off], from, to);
                self.store(&o.dst, to, v, line)?;
            } else {
                let j = self.mem_index(&o.dst, line)?;
                self.mem.copy_within(callee + o.off..callee + o.off + o.len, j);
            }
        }
        Ok((pou.kind == PouType::Function).then(|| self.mem[callee]))
    }

    fn tick(&mut self, line: u32) -> Res<()> {
        if self.budget == 0 {
            return Err(Fault {
                kind: FaultKind::Watchdog,
                line,
            });
        }
        self.budget -= 1;
        Ok(())
    }

    /// Runs a statement list.
    fn block(&mut self, stmts: &[St]) -> Res<Flow> {
        for s in stmts {
            match self.stmt(s)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    /// Runs a loop body; `Some(flow)` means leave the loop with it.
    fn body(&mut self, b: &[St]) -> Res<Option<Flow>> {
        Ok(match self.block(b)? {
            Flow::Next | Flow::Continue => None,
            Flow::Exit => Some(Flow::Next),
            Flow::Return => Some(Flow::Return),
        })
    }

    fn stmt(&mut self, s: &St) -> Res<Flow> {
        let outer = self.enter(s.tr);
        let flow = self.exec(s)?;
        self.leave(outer);
        Ok(flow)
    }

    #[inline]
    fn exec(&mut self, s: &St) -> Res<Flow> {
        let line = s.line;
        self.tick(line)?;
        match &s.kind {
            StKind::Store(place, ty, e) => {
                let v = self.eval(e, line)?;
                self.store(place, *ty, v, line)?;
            }
            StKind::Copy(dst, src, len) => {
                let i = self.mem_index(src, line)?;
                let j = self.mem_index(dst, line)?;
                self.mem.copy_within(i..i + len, j);
            }
            StKind::Call(c) => {
                self.call(c, line)?;
            }
            StKind::If(arms, other) => {
                for (k, (c, b)) in arms.iter().enumerate() {
                    if boolean(self.eval(c, line)?) {
                        self.arm(s.tr, k);
                        return self.block(b);
                    }
                }
                self.arm(s.tr, arms.len());
                return self.block(other);
            }
            StKind::Case(sel, arms, other) => {
                let v = int(self.eval(sel, line)?);
                for (k, (ranges, b)) in arms.iter().enumerate() {
                    if ranges.iter().any(|&(lo, hi)| lo <= v && v <= hi) {
                        self.arm(s.tr, k);
                        return self.block(b);
                    }
                }
                self.arm(s.tr, arms.len());
                return self.block(other);
            }
            StKind::For {
                var,
                ty,
                from,
                to,
                by,
                body,
            } => {
                let start = int(self.eval(from, line)?);
                let end = int(self.eval(to, line)?);
                let step = int(self.eval(by, line)?);
                let (lo, hi) = ty.range();
                let slot = self.mem_index(var, line)?;
                let mut i = start;
                self.mem[slot] = Value::Int(i);
                while (step >= 0 && i <= end) || (step < 0 && i >= end) {
                    self.tick(line)?;
                    self.pass(s.tr);
                    if let Some(flow) = self.body(body)? {
                        return Ok(flow);
                    }
                    // The body may assign the control variable.
                    let next = int(self.mem[slot]) + step;
                    self.mem[slot] = Value::Int(ty.wrap(next));
                    if next < lo || next > hi {
                        break;
                    }
                    i = next;
                }
            }
            StKind::While(c, b) => {
                while boolean(self.eval(c, line)?) {
                    self.tick(line)?;
                    self.pass(s.tr);
                    if let Some(flow) = self.body(b)? {
                        return Ok(flow);
                    }
                }
            }
            StKind::Repeat(b, c) => loop {
                self.tick(line)?;
                self.pass(s.tr);
                if let Some(flow) = self.body(b)? {
                    return Ok(flow);
                }
                if boolean(self.eval(c, line)?) {
                    break;
                }
            },
            StKind::Exit => return Ok(Flow::Exit),
            StKind::Continue => return Ok(Flow::Continue),
            StKind::Return => return Ok(Flow::Return),
        }
        Ok(Flow::Next)
    }

    /// Runs the program POU once.
    pub fn run(&mut self, program: &PouCode) -> Res<()> {
        for &(off, len) in &program.temps {
            self.mem[program.base + off..program.base + off + len]
                .copy_from_slice(&program.init[off..off + len]);
        }
        self.base = program.base;
        self.block(&program.body).map(|_| ())
    }
}

/// Evaluates a constant initialiser (literals and folded CONSTANTs only).
pub fn eval_const(e: &Ex) -> Result<Value, String> {
    let mut mem = [];
    let mut image = Image {
        i: Vec::new(),
        q: Vec::new(),
        m: Vec::new(),
    };
    let mut m = Machine {
        mem: &mut mem,
        image: &mut image,
        pous: &[],
        library: 0,
        base: 0,
        budget: 0,
        now: 0,
        tap: None,
    };
    m.eval(e, 0).map_err(|f| match f.kind {
        FaultKind::DivisionByZero => "integer division by zero in an initial value".to_string(),
        k => format!("{k:?} in an initial value"),
    })
}
