//! A program's retained state, carried from one compiled program to
//! another (a hot swap) by variable name and type.
//!
//! What is state: every top-level program variable that keeps its value
//! between scans: `VAR`, `VAR_INPUT` and `VAR_OUTPUT`, not `VAR_TEMP`, not
//! `CONSTANT`, not located (`AT %...`, which live in the image). A function
//! block instance, array or struct is one variable: it carries whole or not
//! at all.
//!
//! A variable carries only when the new program declares one of the same
//! name whose type has the same shape: the same elementary type; the same
//! enumeration (name and values); arrays of the same bounds over the same
//! element shape; structs and function blocks of the same name with the
//! same members, in order, of the same shapes (and, for function blocks,
//! the same `CONSTANT` flags). On top of that every copied slot is checked
//! against the new program's slot type, so a value of the wrong kind or out
//! of range never lands in memory. Constants inside a carried function
//! block keep the new program's values.
//!
//! A restart of the box keeps less: only the variables declared in a
//! `VAR RETAIN` block ([`Program::retain_state`]), which the host saves as
//! bytes ([`State::encode`]) and reads back ([`State::decode`]) to restore
//! with [`Program::restore_retain`], by the same name and type rules. The
//! checker keeps them under [`crate::MAX_RETAIN`] values per program.
//!
//! # Encoding
//!
//! Little-endian throughout; a name is `len:u16` and its UTF-8 bytes.
//!
//! ```text
//! state  = version:u8 (1)  count:u32  var*
//! var    = name  shape  count:u32  value*
//! shape  = 0 ty:u8
//!        | 1 name count:u32 (name value:i128)*                     enumeration
//!        | 2 shape count:u32 (lo:i128 hi:i128)*                    array
//!        | 3 name count:u32 (name shape)*                          struct
//!        | 4 name count:u32 (name constant:u8 section:u8 shape)*   function block
//! value  = 0 bool:u8 | 1 int:i128 | 2 real:f64 bits
//! ```
//!
//! Decoding checks every length against what is left, so hostile bytes
//! are an error, never a panic or a huge allocation, and shapes nest at
//! most [`MAX_DEPTH`] deep.

#[cfg(not(feature = "std"))]
use crate::prelude::*;
#[cfg(feature = "std")]
use std::time::Instant;

use crate::Program;
use crate::check::{AggKind, Type};
use crate::parser::Section;
use crate::types::{Ty, Value, round_real};

/// A program's retained state, taken by [`Program::state`] and applied
/// with [`Program::restore`], or its RETAIN variables alone
/// ([`Program::retain_state`]), which can be saved as bytes. Opaque: it
/// comes from a program or from [`State::decode`].
#[derive(Debug, Clone)]
pub struct State {
    vars: Vec<Var>,
    #[cfg(feature = "std")]
    started: Option<Instant>,
    /// The program had scanned: its image is live, so a program restored
    /// from it does not write located initial values again.
    warm: bool,
}

#[derive(Debug, Clone)]
struct Var {
    name: String,
    shape: Shape,
    values: Vec<Value>,
}

/// A type, structurally, with every name that makes it what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    Elem(Ty),
    Enum {
        name: String,
        values: Vec<(String, i128)>,
    },
    Array {
        elem: Box<Shape>,
        dims: Vec<(i128, i128)>,
    },
    Struct {
        name: String,
        fields: Vec<(String, Shape)>,
    },
    Fb {
        name: String,
        /// Name, `CONSTANT`, section, shape.
        vars: Vec<(String, bool, Section, Shape)>,
    },
}

/// What one slot may hold.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Elem(Ty),
    Enum(usize),
}

/// What [`Program::restore`] did, by variable name (upper-cased), in the
/// new program's declaration order (`dropped`: the old program's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Carry {
    /// Carried over from the old program.
    pub kept: Vec<String>,
    /// In the new program but starting at its initial value: a new name, or
    /// a name whose type changed.
    pub reset: Vec<String>,
    /// In the old program only; their values are gone.
    pub dropped: Vec<String>,
}

impl State {
    /// The retained variables' names (upper-cased), in declaration order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.vars.iter().map(|v| v.name.as_str())
    }

    /// The slot values of the variable `name` (upper-cased), in layout
    /// order: one for an elementary variable, every element's or member's
    /// otherwise.
    #[must_use]
    pub fn values(&self, name: &str) -> Option<&[Value]> {
        self.vars
            .iter()
            .find(|v| v.name == name)
            .map(|v| v.values.as_slice())
    }

    /// No variables.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// The variables and their values as bytes (the module docs' format),
    /// for [`State::decode`]. Names, types and values only: neither the
    /// wall clock nor the warm flag travels.
    ///
    /// # Errors
    /// A type nested deeper than [`MAX_DEPTH`].
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut out = vec![VERSION];
        put_len(&mut out, self.vars.len());
        for v in &self.vars {
            put_name(&mut out, &v.name);
            put_shape(&mut out, &v.shape, 0)?;
            put_len(&mut out, v.values.len());
            for x in &v.values {
                match *x {
                    Value::Bool(b) => out.extend([0, u8::from(b)]),
                    Value::Int(n) => {
                        out.push(1);
                        out.extend(n.to_le_bytes());
                    }
                    Value::Real(r) => {
                        out.push(2);
                        out.extend(r.to_bits().to_le_bytes());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Reads what [`State::encode`] wrote. A state read back is cold: a
    /// program restored from it still writes its located initial values.
    ///
    /// # Errors
    /// Bytes that are not an encoded state, with the offset where reading
    /// stopped.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader { bytes, at: 0 };
        if r.u8()? != VERSION {
            return Err(r.fail("an unknown version"));
        }
        let n = r.count(2 + 1 + 4)?;
        let mut vars: Vec<Var> = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.name()?;
            let shape = r.shape(0)?;
            let len = r.count(2)?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(match r.u8()? {
                    0 => match r.u8()? {
                        0 => Value::Bool(false),
                        1 => Value::Bool(true),
                        _ => return Err(r.fail("a BOOL that is neither 0 nor 1")),
                    },
                    1 => Value::Int(r.i128()?),
                    2 => Value::Real(f64::from_bits(u64::from_le_bytes(r.array()?))),
                    _ => return Err(r.fail("an unknown value tag")),
                });
            }
            if vars.iter().any(|v| v.name == name) {
                return Err(r.fail("a variable named twice"));
            }
            vars.push(Var { name, shape, values });
        }
        if r.at != bytes.len() {
            return Err(r.fail("bytes after the last variable"));
        }
        Ok(Self {
            vars,
            #[cfg(feature = "std")]
            started: None,
            warm: false,
        })
    }
}

/// The encoding's version byte.
const VERSION: u8 = 1;

/// How deep [`State::encode`] and [`State::decode`] let types nest (an
/// array of structs is two deep).
pub const MAX_DEPTH: usize = 64;

/// Why bytes are not an encoded [`State`], or a state cannot be encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// The byte offset where reading (or writing) stopped.
    pub at: usize,
    pub what: &'static str,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

/// Every [`Ty`], by its code in the encoding (never reorder).
const TYS: [Ty; 16] = [
    Ty::Bool,
    Ty::SInt,
    Ty::Int,
    Ty::DInt,
    Ty::LInt,
    Ty::USInt,
    Ty::UInt,
    Ty::UDInt,
    Ty::ULInt,
    Ty::Byte,
    Ty::Word,
    Ty::DWord,
    Ty::LWord,
    Ty::Real,
    Ty::LReal,
    Ty::Time,
];

/// Every [`Section`], by its code in the encoding (never reorder).
const SECTIONS: [Section; 5] = [
    Section::Var,
    Section::Temp,
    Section::Input,
    Section::Output,
    Section::InOut,
];

fn code_of<T: PartialEq>(table: &[T], x: &T) -> u8 {
    let k = table
        .iter()
        .position(|t| t == x)
        .expect("every variant has a code");
    u8::try_from(k).expect("the tables are short")
}

fn put_len(out: &mut Vec<u8>, n: usize) {
    out.extend(u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
}

fn put_name(out: &mut Vec<u8>, s: &str) {
    // Identifiers are far shorter than 64 KiB; the parser caps a source.
    let n = u16::try_from(s.len()).unwrap_or(u16::MAX);
    out.extend(n.to_le_bytes());
    out.extend(&s.as_bytes()[..usize::from(n)]);
}

fn put_shape(out: &mut Vec<u8>, shape: &Shape, depth: usize) -> Result<(), DecodeError> {
    if depth >= MAX_DEPTH {
        return Err(DecodeError {
            at: out.len(),
            what: "a type nested too deep to keep",
        });
    }
    match shape {
        Shape::Elem(t) => out.extend([0, code_of(&TYS, t)]),
        Shape::Enum { name, values } => {
            out.push(1);
            put_name(out, name);
            put_len(out, values.len());
            for (n, v) in values {
                put_name(out, n);
                out.extend(v.to_le_bytes());
            }
        }
        Shape::Array { elem, dims } => {
            out.push(2);
            put_shape(out, elem, depth + 1)?;
            put_len(out, dims.len());
            for (lo, hi) in dims {
                out.extend(lo.to_le_bytes());
                out.extend(hi.to_le_bytes());
            }
        }
        Shape::Struct { name, fields } => {
            out.push(3);
            put_name(out, name);
            put_len(out, fields.len());
            for (n, f) in fields {
                put_name(out, n);
                put_shape(out, f, depth + 1)?;
            }
        }
        Shape::Fb { name, vars } => {
            out.push(4);
            put_name(out, name);
            put_len(out, vars.len());
            for (n, constant, section, f) in vars {
                put_name(out, n);
                out.extend([u8::from(*constant), code_of(&SECTIONS, section)]);
                put_shape(out, f, depth + 1)?;
            }
        }
    }
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    const fn fail(&self, what: &'static str) -> DecodeError {
        DecodeError { at: self.at, what }
    }

    fn take(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| self.fail("the bytes end early"))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    fn i128(&mut self) -> Result<i128, DecodeError> {
        Ok(i128::from_le_bytes(self.array()?))
    }

    /// A count of items of at least `min` bytes each, checked against what
    /// is left, so a lying count cannot allocate.
    fn count(&mut self, min: usize) -> Result<usize, DecodeError> {
        let n = usize::try_from(u32::from_le_bytes(self.array()?)).unwrap_or(usize::MAX);
        if n.saturating_mul(min) > self.bytes.len() - self.at {
            return Err(self.fail("a count longer than the bytes left"));
        }
        Ok(n)
    }

    fn name(&mut self) -> Result<String, DecodeError> {
        let n = usize::from(u16::from_le_bytes(self.array()?));
        let at = self.at;
        let b = self.take(n)?;
        core::str::from_utf8(b)
            .map(ToString::to_string)
            .map_err(|_| DecodeError {
                at,
                what: "a name that is not UTF-8",
            })
    }

    fn shape(&mut self, depth: usize) -> Result<Shape, DecodeError> {
        if depth >= MAX_DEPTH {
            return Err(self.fail("a type nested too deep"));
        }
        Ok(match self.u8()? {
            0 => {
                let code = usize::from(self.u8()?);
                Shape::Elem(
                    *TYS.get(code)
                        .ok_or_else(|| self.fail("an unknown elementary type"))?,
                )
            }
            1 => {
                let name = self.name()?;
                let n = self.count(2 + 16)?;
                let mut values = Vec::with_capacity(n);
                for _ in 0..n {
                    values.push((self.name()?, self.i128()?));
                }
                Shape::Enum { name, values }
            }
            2 => {
                let elem = Box::new(self.shape(depth + 1)?);
                let n = self.count(32)?;
                let mut dims = Vec::with_capacity(n);
                for _ in 0..n {
                    dims.push((self.i128()?, self.i128()?));
                }
                Shape::Array { elem, dims }
            }
            3 => {
                let name = self.name()?;
                let n = self.count(2 + 2)?;
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    fields.push((self.name()?, self.shape(depth + 1)?));
                }
                Shape::Struct { name, fields }
            }
            4 => {
                let name = self.name()?;
                let n = self.count(2 + 2 + 2)?;
                let mut vars = Vec::with_capacity(n);
                for _ in 0..n {
                    let vname = self.name()?;
                    let constant = match self.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err(self.fail("a CONSTANT flag that is neither 0 nor 1")),
                    };
                    let code = usize::from(self.u8()?);
                    let section = *SECTIONS
                        .get(code)
                        .ok_or_else(|| self.fail("an unknown section"))?;
                    vars.push((vname, constant, section, self.shape(depth + 1)?));
                }
                Shape::Fb { name, vars }
            }
            _ => return Err(self.fail("an unknown type tag")),
        })
    }
}

impl Program {
    /// Retained top-level variables: `(name, type, absolute slot)`.
    fn retained(&self) -> impl Iterator<Item = (&str, Type, usize)> {
        let prog = &self.code.pous[self.code.program];
        prog.vars
            .iter()
            .filter(|v| v.loc.is_none() && !v.constant && v.section != Section::Temp)
            .map(move |v| (v.name.as_str(), v.ty, prog.base + v.off))
    }

    /// The `RETAIN` variables among [`Program::retained`]'s (the checker
    /// refuses RETAIN on the others).
    fn retain_vars(&self) -> impl Iterator<Item = (&str, Type, usize)> {
        self.retained().filter(|(name, _, _)| {
            self.code.pous[self.code.program]
                .vars
                .iter()
                .any(|v| v.retain && v.name == *name)
        })
    }

    /// Whether the program declares any `RETAIN` variable.
    #[must_use]
    pub fn has_retain(&self) -> bool {
        self.retain_vars().next().is_some()
    }

    /// The `RETAIN` variables alone, to keep across a restart of the box
    /// ([`State::encode`]). Cold and with no clock: restoring it brings
    /// back the variables, not the image or the timers' time base.
    #[must_use]
    pub fn retain_state(&self) -> State {
        let vars = self
            .retain_vars()
            .map(|(name, ty, slot)| Var {
                name: name.to_string(),
                shape: self.shape(ty),
                values: self.mem[slot..slot + self.size_of(ty)].to_vec(),
            })
            .collect();
        State {
            vars,
            #[cfg(feature = "std")]
            started: None,
            warm: false,
        }
    }

    /// [`Program::restore`] onto the `RETAIN` variables alone: what a box
    /// does at start with the state it saved. In the [`Carry`], `reset`
    /// names the RETAIN variables that start at their initial values (new,
    /// or of a changed type when `state` has the name too) and `dropped`
    /// the saved ones this program does not declare RETAIN.
    pub fn restore_retain(&mut self, state: &State) -> Carry {
        let mine: Vec<(String, Type, usize)> = self
            .retain_vars()
            .map(|(n, ty, slot)| (n.to_string(), ty, slot))
            .collect();
        self.restore_onto(&mine, state)
    }

    fn size_of(&self, ty: Type) -> usize {
        match ty {
            Type::Agg(a) => self.code.aggs[a].size,
            _ => 1,
        }
    }

    fn shape(&self, ty: Type) -> Shape {
        match ty {
            Type::Elem(t) => Shape::Elem(t),
            Type::Enum(e) => {
                let info = &self.code.enums[e];
                Shape::Enum {
                    name: info.name.clone(),
                    values: info.values.clone(),
                }
            }
            Type::Agg(a) => {
                let info = &self.code.aggs[a];
                match &info.kind {
                    AggKind::Array { elem, dims } => Shape::Array {
                        elem: Box::new(self.shape(*elem)),
                        dims: dims.clone(),
                    },
                    AggKind::Struct { fields } => Shape::Struct {
                        name: info.name.clone(),
                        fields: fields
                            .iter()
                            .map(|f| (f.name.clone(), self.shape(f.ty)))
                            .collect(),
                    },
                    AggKind::Fb(p) => {
                        let pou = &self.code.pous[*p];
                        Shape::Fb {
                            name: pou.name.clone(),
                            vars: pou
                                .vars
                                .iter()
                                .map(|v| (v.name.clone(), v.constant, v.section, self.shape(v.ty)))
                                .collect(),
                        }
                    }
                }
            }
        }
    }

    /// The type of every slot of `ty` laid out at `off` within `out`, and
    /// the `(offset, len)` ranges that are function block constants.
    fn layout(&self, ty: Type, off: usize, out: &mut [Option<Slot>], fixed: &mut Vec<(usize, usize)>) {
        match ty {
            Type::Elem(t) => out[off] = Some(Slot::Elem(t)),
            Type::Enum(e) => out[off] = Some(Slot::Enum(e)),
            Type::Agg(a) => match &self.code.aggs[a].kind {
                AggKind::Array { elem, .. } => {
                    let esize = self.size_of(*elem);
                    let n = self.code.aggs[a].size.checked_div(esize).unwrap_or(0);
                    for k in 0..n {
                        self.layout(*elem, off + k * esize, out, fixed);
                    }
                }
                AggKind::Struct { fields } => {
                    for f in fields {
                        self.layout(f.ty, off + f.off, out, fixed);
                    }
                }
                AggKind::Fb(p) => {
                    for v in &self.code.pous[*p].vars {
                        self.layout(v.ty, off + v.off, out, fixed);
                        if v.constant {
                            fixed.push((off + v.off, self.size_of(v.ty)));
                        }
                    }
                }
            },
        }
    }

    fn fits(&self, slot: Slot, v: Value) -> bool {
        match (slot, v) {
            (Slot::Elem(Ty::Bool), Value::Bool(_)) => true,
            (Slot::Elem(t), Value::Real(x)) if t.is_real() => {
                x.is_finite() && round_real(t, x).to_bits() == x.to_bits()
            }
            (Slot::Elem(t), Value::Int(n)) if t != Ty::Bool && !t.is_real() => {
                let (lo, hi) = t.range();
                lo <= n && n <= hi
            }
            (Slot::Enum(e), Value::Int(n)) => self.code.enums[e].values.iter().any(|(_, x)| *x == n),
            _ => false,
        }
    }

    /// The program's retained state (see [`State`]), to carry into another
    /// program with [`Program::restore`].
    #[must_use]
    pub fn state(&self) -> State {
        let vars = self
            .retained()
            .map(|(name, ty, slot)| Var {
                name: name.to_string(),
                shape: self.shape(ty),
                values: self.mem[slot..slot + self.size_of(ty)].to_vec(),
            })
            .collect();
        State {
            vars,
            #[cfg(feature = "std")]
            started: self.started,
            warm: !self.cold,
        }
    }

    /// Copies `state` into this program: each retained variable of this
    /// program takes the value of the variable of the same name in `state`
    /// when their types have the same shape and every value fits; the rest
    /// keep their current values (a freshly compiled program's are its
    /// initial ones). The wall clock [`Program::scan`] uses carries too,
    /// and so does a warm start: located initial values are not written
    /// again over an image the old program has been scanning.
    pub fn restore(&mut self, state: &State) -> Carry {
        let mine: Vec<(String, Type, usize)> = self
            .retained()
            .map(|(n, ty, slot)| (n.to_string(), ty, slot))
            .collect();
        self.restore_onto(&mine, state)
    }

    /// Copies `state` into the variables `mine` (see [`Program::restore`]).
    fn restore_onto(&mut self, mine: &[(String, Type, usize)], state: &State) -> Carry {
        let mut carry = Carry::default();
        for (name, ty, slot) in mine {
            let copied = state
                .vars
                .iter()
                .find(|v| &v.name == name)
                .is_some_and(|old| self.copy_in(old, *ty, *slot));
            if copied {
                carry.kept.push(name.clone());
            } else {
                carry.reset.push(name.clone());
            }
        }
        carry.dropped = state
            .vars
            .iter()
            .filter(|v| !mine.iter().any(|(n, _, _)| n == &v.name))
            .map(|v| v.name.clone())
            .collect();
        #[cfg(feature = "std")]
        if state.started.is_some() {
            self.started = state.started;
        }
        if state.warm {
            self.cold = false;
        }
        carry
    }

    /// Copies `old` into the variable of type `ty` at `slot` if it fits
    /// exactly; false, with nothing written, otherwise.
    fn copy_in(&mut self, old: &Var, ty: Type, slot: usize) -> bool {
        let size = self.size_of(ty);
        if old.values.len() != size || old.shape != self.shape(ty) {
            return false;
        }
        let mut slots = vec![None; size];
        let mut fixed = Vec::new();
        self.layout(ty, 0, &mut slots, &mut fixed);
        let ok = slots
            .iter()
            .zip(&old.values)
            .all(|(s, v)| s.is_some_and(|s| self.fits(s, *v)));
        if !ok {
            return false;
        }
        let dst = &mut self.mem[slot..slot + size];
        dst.copy_from_slice(&old.values);
        for (off, len) in fixed {
            dst[off..off + len].copy_from_slice(&self.code.mem_init[slot + off..slot + off + len]);
        }
        true
    }
}
