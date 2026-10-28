//! Executes checked code against variable slots and a process image.
//!
//! Semantics the standard leaves to the implementation, chosen here and
//! pinned by tests (the differential oracle is the arbiter):
//! - integer arithmetic wraps at the type's width;
//! - integer `/` and `MOD` by zero fault; REAL division follows IEEE 754;
//! - REAL -> integer rounds half away from zero, then wraps; integer
//!   narrowing keeps the low bits;
//! - `AND`/`OR` evaluate both operands (no short-circuit);
//! - FOR evaluates its bounds once and stops, rather than wrapping, when the
//!   next value would leave the control variable's range;
//! - each scan has an instruction budget; exhausting it is a watchdog fault.

use std::fmt;

use crate::check::{Area, Ex, Loc, Place, St, StKind};
use crate::parser::{BinOp, UnOp};
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
    fn read(&self, loc: Loc) -> u64 {
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
    /// Integer `/` or `MOD` by zero.
    DivisionByZero,
    /// The scan used up its instruction budget (an endless loop, usually).
    Watchdog,
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
            FaultKind::Watchdog => "watchdog: scan exceeded its instruction budget",
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
fn from_raw(ty: Ty, raw: u64) -> Value {
    match ty {
        Ty::Bool => Value::Bool(raw & 1 == 1),
        Ty::Real => Value::Real(f64::from(f32::from_bits(raw as u32))),
        Ty::LReal => Value::Real(f64::from_bits(raw)),
        t => Value::Int(t.wrap(i128::from(raw))),
    }
}

/// Explicit or implicit conversion between elementary types.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub fn convert(v: Value, to: Ty) -> Value {
    match (v, to) {
        (Value::Bool(b), Ty::Bool) => Value::Bool(b),
        (Value::Bool(b), t) if t.is_real() => Value::Real(if b { 1.0 } else { 0.0 }),
        (Value::Bool(b), _) => Value::Int(i128::from(b)),
        (Value::Int(i), Ty::Bool) => Value::Bool(i != 0),
        (Value::Int(i), t) if t.is_real() => Value::Real(round_real(t, i as f64)),
        (Value::Int(i), t) => Value::Int(t.wrap(i)),
        (Value::Real(r), Ty::Bool) => Value::Bool(r != 0.0),
        (Value::Real(r), t) if t.is_real() => Value::Real(round_real(t, r)),
        // `as` saturates and maps NaN to 0; then wrap into the target.
        (Value::Real(r), t) => Value::Int(t.wrap(r.round() as i128)),
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
            Value::Int(ty.wrap(out))
        }
    };
    Ok(v)
}

/// Interpreter state for one scan.
pub struct Machine<'a> {
    pub slots: &'a mut [Value],
    pub image: &'a mut Image,
    pub budget: u64,
}

impl Machine<'_> {
    fn eval(&self, expr: &Ex) -> Result<Value, FaultKind> {
        Ok(match expr {
            Ex::Const(v) => *v,
            Ex::Load(Place::Slot(s), _) => self.slots[*s],
            Ex::Load(Place::Image(loc), ty) => from_raw(*ty, self.image.read(*loc)),
            Ex::Conv(a, _, to) => convert(self.eval(a)?, *to),
            Ex::Un(UnOp::Neg, a, ty) => match self.eval(a)? {
                Value::Real(r) => Value::Real(-r),
                v => Value::Int(ty.wrap(-int(v))),
            },
            Ex::Un(UnOp::Not, a, ty) => match self.eval(a)? {
                Value::Bool(b) => Value::Bool(!b),
                v => Value::Int(ty.wrap(!int(v))),
            },
            Ex::Bin(op, a, b, ty) => {
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                binop(*op, x, y, *ty)?
            }
        })
    }

    fn store(&mut self, place: Place, ty: Ty, v: Value) {
        match place {
            Place::Slot(s) => self.slots[s] = v,
            Place::Image(loc) => self.image.write(loc, to_raw(ty, v)),
        }
    }

    fn tick(&mut self, line: u32) -> Result<(), Fault> {
        if self.budget == 0 {
            return Err(Fault {
                kind: FaultKind::Watchdog,
                line,
            });
        }
        self.budget -= 1;
        Ok(())
    }

    fn at(line: u32) -> impl Fn(FaultKind) -> Fault {
        move |kind| Fault { kind, line }
    }

    /// Runs a statement list.
    fn block(&mut self, stmts: &[St]) -> Result<Flow, Fault> {
        for s in stmts {
            match self.stmt(s)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    /// Runs a loop body; `Some(flow)` means leave the loop with it.
    fn body(&mut self, b: &[St]) -> Result<Option<Flow>, Fault> {
        Ok(match self.block(b)? {
            Flow::Next | Flow::Continue => None,
            Flow::Exit => Some(Flow::Next),
            Flow::Return => Some(Flow::Return),
        })
    }

    fn stmt(&mut self, s: &St) -> Result<Flow, Fault> {
        let line = s.line;
        self.tick(line)?;
        let f = Self::at(line);
        match &s.kind {
            StKind::Store(place, ty, e) => {
                let v = self.eval(e).map_err(&f)?;
                self.store(*place, *ty, v);
            }
            StKind::If(arms, other) => {
                for (c, b) in arms {
                    if boolean(self.eval(c).map_err(&f)?) {
                        return self.block(b);
                    }
                }
                return self.block(other);
            }
            StKind::Case(sel, arms, other) => {
                let v = int(self.eval(sel).map_err(&f)?);
                for (ranges, b) in arms {
                    if ranges.iter().any(|&(lo, hi)| lo <= v && v <= hi) {
                        return self.block(b);
                    }
                }
                return self.block(other);
            }
            StKind::For {
                slot,
                ty,
                from,
                to,
                by,
                body,
            } => {
                let start = int(self.eval(from).map_err(&f)?);
                let end = int(self.eval(to).map_err(&f)?);
                let step = int(self.eval(by).map_err(&f)?);
                let (lo, hi) = ty.range();
                let mut i = start;
                self.slots[*slot] = Value::Int(i);
                while (step >= 0 && i <= end) || (step < 0 && i >= end) {
                    self.tick(line)?;
                    if let Some(flow) = self.body(body)? {
                        return Ok(flow);
                    }
                    // The body may assign the control variable.
                    let next = int(self.slots[*slot]) + step;
                    self.slots[*slot] = Value::Int(ty.wrap(next));
                    if next < lo || next > hi {
                        break;
                    }
                    i = next;
                }
            }
            StKind::While(c, b) => {
                while boolean(self.eval(c).map_err(&f)?) {
                    self.tick(line)?;
                    if let Some(flow) = self.body(b)? {
                        return Ok(flow);
                    }
                }
            }
            StKind::Repeat(b, c) => loop {
                self.tick(line)?;
                if let Some(flow) = self.body(b)? {
                    return Ok(flow);
                }
                if boolean(self.eval(c).map_err(&f)?) {
                    break;
                }
            },
            StKind::Exit => return Ok(Flow::Exit),
            StKind::Continue => return Ok(Flow::Continue),
            StKind::Return => return Ok(Flow::Return),
        }
        Ok(Flow::Next)
    }

    /// Runs a program body once.
    pub fn run(&mut self, body: &[St]) -> Result<(), Fault> {
        self.block(body).map(|_| ())
    }
}

/// Evaluates a constant initialiser against the slots initialised so far.
pub fn eval_const(e: &Ex, slots: &[Value]) -> Result<Value, String> {
    let mut slots = slots.to_vec();
    let mut image = Image {
        i: Vec::new(),
        q: Vec::new(),
        m: Vec::new(),
    };
    let m = Machine {
        slots: &mut slots,
        image: &mut image,
        budget: 0,
    };
    m.eval(e).map_err(|k| match k {
        FaultKind::DivisionByZero => "integer division by zero in an initial value".to_string(),
        FaultKind::Watchdog => unreachable!("expressions do not tick"),
    })
}
