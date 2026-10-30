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

use std::time::Instant;

use crate::Program;
use crate::check::{AggKind, Type};
use crate::parser::Section;
use crate::types::{Ty, Value, round_real};

/// A program's retained state, taken by [`Program::state`] and applied
/// with [`Program::restore`]. Opaque: it can only come from a program.
#[derive(Debug, Clone)]
pub struct State {
    vars: Vec<Var>,
    started: Option<Instant>,
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
            (Slot::Elem(t), Value::Real(x)) if t.is_real() => x.is_finite() && round_real(t, x).to_bits() == x.to_bits(),
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
            started: self.started,
        }
    }

    /// Copies `state` into this program: each retained variable of this
    /// program takes the value of the variable of the same name in `state`
    /// when their types have the same shape and every value fits; the rest
    /// keep their current values (a freshly compiled program's are its
    /// initial ones). The wall clock [`Program::scan`] uses carries too.
    pub fn restore(&mut self, state: &State) -> Carry {
        let mut carry = Carry::default();
        let mine: Vec<(String, Type, usize)> = self
            .retained()
            .map(|(n, ty, slot)| (n.to_string(), ty, slot))
            .collect();
        for (name, ty, slot) in &mine {
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
        if state.started.is_some() {
            self.started = state.started;
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
