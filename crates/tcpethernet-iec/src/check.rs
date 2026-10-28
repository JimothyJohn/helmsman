//! Type checking: syntax tree -> typed code ([`Ex`], [`St`]) with every
//! implicit conversion made explicit, every name resolved to a slot or a
//! process-image location, and every literal range-checked.

use std::collections::HashMap;

use crate::interp;
use crate::lexer::Pos;
use crate::parser::{self, BinOp, CaseLabel, Expr, ExprKind, Stmt, StmtKind, UnOp, VarKind};
use crate::types::{Ty, Value, round_real};

/// A compile error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub pos: Pos,
    pub msg: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.pos, self.msg)
    }
}

/// Process-image areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// `%I`: inputs, from the scanner. Read-only to the program.
    I,
    /// `%Q`: outputs, to the scanner.
    Q,
    /// `%M`: memory.
    M,
}

/// Byte sizes of the process image areas a program is compiled against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSizes {
    pub i: usize,
    pub q: usize,
    pub m: usize,
}

/// A located address: `%IX3.2` is `{ I, byte 3, bit 2, 1 bit }`; `%QW2` is
/// `{ Q, byte 4, 16 bits }` (word *n* starts at byte 2*n*, little-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loc {
    pub area: Area,
    pub byte: usize,
    pub bit: u8,
    /// 1, 8, 16, 32 or 64.
    pub bits: u32,
}

impl Loc {
    /// The bit-string type of a bare address (`%QW2` is a WORD).
    const fn natural_ty(self) -> Ty {
        match self.bits {
            1 => Ty::Bool,
            8 => Ty::Byte,
            16 => Ty::Word,
            32 => Ty::DWord,
            _ => Ty::LWord,
        }
    }
}

/// Parses `IX0.1`, `I0.1`, `QB3`, `QW2`, `MD4`, `IL1` (the text after `%`).
pub fn parse_address(s: &str, sizes: ImageSizes) -> Result<Loc, String> {
    let b = s.as_bytes();
    let (area, limit) = match b.first() {
        Some(b'I') => (Area::I, sizes.i),
        Some(b'Q') => (Area::Q, sizes.q),
        Some(b'M') => (Area::M, sizes.m),
        _ => return Err(format!("%{s}: address must start with I, Q or M")),
    };
    let (bits, rest) = match b.get(1) {
        Some(b'X') => (1, &s[2..]),
        Some(b'B') => (8, &s[2..]),
        Some(b'W') => (16, &s[2..]),
        Some(b'D') => (32, &s[2..]),
        Some(b'L') => (64, &s[2..]),
        Some(c) if c.is_ascii_digit() => (1, &s[1..]),
        _ => return Err(format!("%{s}: expected a size X, B, W, D or L")),
    };
    let num = |t: &str| -> Result<usize, String> {
        if t.is_empty() || !t.bytes().all(|c| c.is_ascii_digit()) {
            return Err(format!("%{s}: malformed address"));
        }
        t.parse::<usize>().map_err(|_| format!("%{s}: address too large"))
    };
    let (byte, bit) = if bits == 1 {
        let Some((by, bi)) = rest.split_once('.') else {
            return Err(format!("%{s}: a bit address is byte.bit, e.g. %IX0.3"));
        };
        let bit = num(bi)?;
        if bit > 7 {
            return Err(format!("%{s}: bit must be 0..7"));
        }
        (num(by)?, u8::try_from(bit).unwrap_or(0))
    } else {
        if rest.contains('.') {
            return Err(format!("%{s}: only X addresses take a .bit"));
        }
        let n = num(rest)?;
        let width = (bits / 8) as usize;
        (
            n.checked_mul(width)
                .ok_or_else(|| format!("%{s}: address too large"))?,
            0,
        )
    };
    let need = byte.saturating_add((bits as usize).div_ceil(8));
    if need > limit {
        return Err(format!("%{s}: outside the {limit}-byte {area:?} area"));
    }
    Ok(Loc {
        area,
        byte,
        bit,
        bits,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Slot(usize),
    Image(Loc),
}

/// Typed expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Ex {
    Const(Value),
    Load(Place, Ty),
    /// Operand and result type are the same.
    Un(UnOp, Box<Ex>, Ty),
    /// Both operands have type `.3`; comparisons yield BOOL.
    Bin(BinOp, Box<Ex>, Box<Ex>, Ty),
    Conv(Box<Ex>, Ty, Ty),
}

#[derive(Debug, Clone, PartialEq)]
pub struct St {
    pub kind: StKind,
    pub line: u32,
}

/// Inclusive label ranges and the statements they select.
pub type CaseArm = (Vec<(i128, i128)>, Vec<St>);

#[derive(Debug, Clone, PartialEq)]
pub enum StKind {
    Store(Place, Ty, Ex),
    If(Vec<(Ex, Vec<St>)>, Vec<St>),
    Case(Ex, Vec<CaseArm>, Vec<St>),
    For {
        slot: usize,
        ty: Ty,
        from: Ex,
        to: Ex,
        by: Ex,
        body: Vec<St>,
    },
    While(Ex, Vec<St>),
    Repeat(Vec<St>, Ex),
    Exit,
    Continue,
    Return,
}

#[derive(Debug, Clone, Copy)]
pub enum SymKind {
    Slot(usize),
    Image(Loc),
}

#[derive(Debug, Clone, Copy)]
pub struct Sym {
    pub kind: SymKind,
    pub ty: Ty,
    pub constant: bool,
}

/// Output of the checker.
#[derive(Debug, Clone)]
pub struct Checked {
    pub syms: HashMap<String, Sym>,
    /// Initial value of every slot.
    pub init: Vec<Value>,
    /// Slots reset to their initial value at the start of every scan.
    pub temps: Vec<usize>,
    pub body: Vec<St>,
}

/// Type of a synthesized expression: known, or a literal still free to take
/// the type its context needs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum T {
    Known(Ty),
    IntLit(i128),
    RealLit(f64),
}

struct Ck {
    sizes: ImageSizes,
    syms: HashMap<String, Sym>,
    init: Vec<Value>,
    diags: Vec<Diagnostic>,
    loops: usize,
}

type O<T> = Option<T>;

impl Ck {
    fn err(&mut self, pos: Pos, msg: impl Into<String>) {
        self.diags.push(Diagnostic { pos, msg: msg.into() });
    }

    // ---- declarations ----

    fn decl(&mut self, d: &parser::VarDecl, temps: &mut Vec<usize>) {
        if self.syms.contains_key(&d.name) {
            self.err(d.pos, format!("{} is declared twice", d.name));
            return;
        }
        let Some(ty) = Ty::from_name(&d.ty) else {
            self.err(d.ty_pos, format!("unknown type {}", d.ty));
            return;
        };
        if let Some((addr, apos)) = &d.at {
            let bad = |m: &str| Some(format!("{}: {m}", d.name));
            let problem = if d.kind == VarKind::Temp {
                bad("VAR_TEMP cannot be located")
            } else if d.constant {
                bad("a CONSTANT cannot be located")
            } else if d.init.is_some() {
                bad("a located variable takes no initial value")
            } else {
                None
            };
            if let Some(m) = problem {
                self.err(d.pos, m);
                return;
            }
            match parse_address(addr, self.sizes) {
                Ok(loc)
                    if (loc.bits == 1) != (ty == Ty::Bool) || (loc.bits != 1 && loc.bits != ty.bits()) =>
                {
                    self.err(
                        *apos,
                        format!("%{addr} is {} bits wide, {ty} is {}", loc.bits, ty.bits()),
                    );
                }
                Ok(loc) => {
                    self.syms.insert(
                        d.name.clone(),
                        Sym {
                            kind: SymKind::Image(loc),
                            ty,
                            constant: false,
                        },
                    );
                }
                Err(m) => self.err(*apos, m),
            }
            return;
        }
        let value = match &d.init {
            None if d.constant => {
                self.err(d.pos, format!("CONSTANT {} needs an initial value", d.name));
                return;
            }
            None => Value::zero(ty),
            Some(e) => match self.const_value(e, ty) {
                Some(v) => v,
                None => return,
            },
        };
        let slot = self.init.len();
        self.init.push(value);
        if d.kind == VarKind::Temp {
            temps.push(slot);
        }
        self.syms.insert(
            d.name.clone(),
            Sym {
                kind: SymKind::Slot(slot),
                ty,
                constant: d.constant,
            },
        );
    }

    /// Evaluates an initialiser; it may refer only to earlier CONSTANTs.
    fn const_value(&mut self, e: &Expr, ty: Ty) -> O<Value> {
        let ex = self.coerced(e, ty)?;
        if !self.is_const(&ex) {
            self.err(e.pos, "an initial value may use only literals and CONSTANTs");
            return None;
        }
        match interp::eval_const(&ex, &self.init) {
            Ok(v) => Some(v),
            Err(m) => {
                self.err(e.pos, m);
                None
            }
        }
    }

    fn is_const(&self, ex: &Ex) -> bool {
        match ex {
            Ex::Const(_) => true,
            Ex::Load(Place::Slot(s), _) => self
                .syms
                .values()
                .any(|y| y.constant && matches!(y.kind, SymKind::Slot(k) if k == *s)),
            Ex::Load(Place::Image(_), _) => false,
            Ex::Un(_, a, _) | Ex::Conv(a, _, _) => self.is_const(a),
            Ex::Bin(_, a, b, _) => self.is_const(a) && self.is_const(b),
        }
    }

    // ---- expressions ----

    /// Synthesizes the type of `e`; `want` is the type the context expects,
    /// used only to type literals.
    fn synth(&mut self, e: &Expr, want: Option<Ty>) -> O<(Ex, T)> {
        match &e.kind {
            ExprKind::Int { value, prefix: None } => Some((Ex::Const(Value::Int(*value)), T::IntLit(*value))),
            ExprKind::Real { value, prefix: None } => {
                Some((Ex::Const(Value::Real(*value)), T::RealLit(*value)))
            }
            ExprKind::Int {
                value,
                prefix: Some(p),
            } => {
                let ty = self.prefix_ty(p, e.pos)?;
                let ex = self.lit_as(T::IntLit(*value), ty, e.pos)?;
                Some((ex, T::Known(ty)))
            }
            ExprKind::Real {
                value,
                prefix: Some(p),
            } => {
                let ty = self.prefix_ty(p, e.pos)?;
                let ex = self.lit_as(T::RealLit(*value), ty, e.pos)?;
                Some((ex, T::Known(ty)))
            }
            ExprKind::Bool(b) => Some((Ex::Const(Value::Bool(*b)), T::Known(Ty::Bool))),
            ExprKind::Var(name) => {
                let Some(sym) = self.syms.get(name).copied() else {
                    self.err(e.pos, format!("undeclared variable {name}"));
                    return None;
                };
                let place = match sym.kind {
                    SymKind::Slot(s) => Place::Slot(s),
                    SymKind::Image(l) => Place::Image(l),
                };
                Some((Ex::Load(place, sym.ty), T::Known(sym.ty)))
            }
            ExprKind::Direct(addr) => match parse_address(addr, self.sizes) {
                Ok(loc) => Some((
                    Ex::Load(Place::Image(loc), loc.natural_ty()),
                    T::Known(loc.natural_ty()),
                )),
                Err(m) => {
                    self.err(e.pos, m);
                    None
                }
            },
            ExprKind::Unary(op, a) => self.unary(*op, a, want, e.pos),
            ExprKind::Binary(BinOp::Pow, a, b) => self.power(a, b, want, e.pos),
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, want, e.pos),
            ExprKind::Call(name, args) => self.call(name, args, e.pos),
        }
    }

    fn prefix_ty(&mut self, p: &str, pos: Pos) -> O<Ty> {
        let ty = Ty::from_name(p);
        if ty.is_none() {
            self.err(pos, format!("unknown type {p} in typed literal"));
        }
        ty
    }

    /// A literal as type `ty`, range-checked.
    fn lit_as(&mut self, t: T, ty: Ty, pos: Pos) -> O<Ex> {
        #[allow(clippy::cast_precision_loss)]
        let v = match (t, ty) {
            (T::IntLit(v), t) if t.is_integral() && t != Ty::Bool => {
                let (lo, hi) = t.range();
                if v < lo || v > hi {
                    self.err(pos, format!("{v} does not fit {t} ({lo}..{hi})"));
                    return None;
                }
                Value::Int(v)
            }
            (T::IntLit(v), t) if t.is_real() => Value::Real(round_real(t, v as f64)),
            (T::RealLit(v), t) if t.is_real() => {
                let r = round_real(t, v);
                if !r.is_finite() {
                    self.err(pos, format!("{v} is out of range for {t}"));
                    return None;
                }
                Value::Real(r)
            }
            (T::IntLit(v), t) => {
                self.err(pos, format!("integer literal {v} cannot be a {t}"));
                return None;
            }
            (T::RealLit(v), t) => {
                self.err(
                    pos,
                    format!("real literal {v:?} cannot be a {t}; use a conversion such as LREAL_TO_{t}"),
                );
                return None;
            }
            (T::Known(_), _) => unreachable!("lit_as on a typed expression"),
        };
        Some(Ex::Const(v))
    }

    /// `ex` (of type `t`) as type `to`: identity, an implicit widening, or
    /// a literal taking on `to`.
    fn coerce(&mut self, ex: Ex, t: T, to: Ty, pos: Pos) -> O<Ex> {
        match t {
            T::Known(from) if from == to => Some(ex),
            T::Known(from) if from.widens_to(to) => Some(Ex::Conv(Box::new(ex), from, to)),
            T::Known(from) => {
                self.err(
                    pos,
                    format!("expected {to}, found {from}; convert explicitly with {from}_TO_{to}"),
                );
                None
            }
            lit => self.lit_as(lit, to, pos),
        }
    }

    fn coerced(&mut self, e: &Expr, to: Ty) -> O<Ex> {
        let (ex, t) = self.synth(e, Some(to))?;
        self.coerce(ex, t, to, e.pos)
    }

    /// Default type for a pair of untyped literals.
    fn default_lit(a: T, b: T, want: Option<Ty>) -> Ty {
        let real = matches!(a, T::RealLit(_)) || matches!(b, T::RealLit(_));
        match want {
            Some(w) if real && w.is_real() => w,
            Some(w) if !real && (w.is_num() || w.is_bitstring()) => w,
            _ if real => Ty::LReal,
            _ => {
                let (lo, hi) = Ty::DInt.range();
                let fits = |t: T| !matches!(t, T::IntLit(v) if v < lo || v > hi);
                if fits(a) && fits(b) { Ty::DInt } else { Ty::LInt }
            }
        }
    }

    /// The type both operands are brought to.
    fn operand_ty(&mut self, a: T, b: T, want: Option<Ty>, op: BinOp, pos: Pos) -> O<Ty> {
        let lit_ok = |lit: T, k: Ty| match lit {
            T::IntLit(_) => k.is_integral() && k != Ty::Bool || k.is_real(),
            T::RealLit(_) => k.is_real(),
            T::Known(_) => false,
        };
        match (a, b) {
            (T::Known(x), T::Known(y)) => {
                let common = Ty::common(x, y);
                if common.is_none() {
                    self.err(
                        pos,
                        format!(
                            "{} on {x} and {y}: no implicit conversion between them; convert one explicitly",
                            op.symbol()
                        ),
                    );
                }
                common
            }
            (T::Known(k), lit) | (lit, T::Known(k)) => {
                if lit_ok(lit, k) {
                    Some(k)
                } else if matches!(lit, T::RealLit(_)) && k.is_int() {
                    let r = [Ty::Real, Ty::LReal].into_iter().find(|&r| k.widens_to(r));
                    if r.is_none() {
                        self.err(
                            pos,
                            format!("{k} does not convert implicitly to a real type; use {k}_TO_LREAL"),
                        );
                    }
                    r
                } else {
                    // Let `coerce` report the literal that does not fit.
                    Some(k)
                }
            }
            (a, b) => Some(Self::default_lit(a, b, want)),
        }
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, want: Option<Ty>, pos: Pos) -> O<(Ex, T)> {
        let cmp = matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );
        let inner_want = if cmp { None } else { want };
        let ra = self.synth(a, inner_want);
        let rb = self.synth(b, inner_want);
        let ((xa, ta), (xb, tb)) = (ra?, rb?);
        let ty = self.operand_ty(ta, tb, inner_want, op, pos)?;
        let ok = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => ty.is_num(),
            BinOp::Mod => ty.is_int(),
            BinOp::And | BinOp::Or | BinOp::Xor => ty.is_bit(),
            _ => true,
        };
        if !ok {
            let need = match op {
                BinOp::Mod => "an integer type",
                BinOp::And | BinOp::Or | BinOp::Xor => "BOOL or a bit string (BYTE..LWORD)",
                _ => "a numeric type",
            };
            self.err(pos, format!("{} needs {need}, found {ty}", op.symbol()));
            return None;
        }
        let xa = self.coerce(xa, ta, ty, a.pos);
        let xb = self.coerce(xb, tb, ty, b.pos);
        let ex = Ex::Bin(op, Box::new(xa?), Box::new(xb?), ty);
        Some((ex, T::Known(if cmp { Ty::Bool } else { ty })))
    }

    fn power(&mut self, a: &Expr, b: &Expr, want: Option<Ty>, pos: Pos) -> O<(Ex, T)> {
        let base = self.synth(a, want);
        let exp = self.synth(b, None);
        let ((xa, ta), (xb, tb)) = (base?, exp?);
        let ty = match ta {
            T::Known(t) if t.is_real() => t,
            T::Known(t) => {
                self.err(
                    pos,
                    format!("** needs a REAL or LREAL base, found {t}; convert with {t}_TO_LREAL"),
                );
                return None;
            }
            _ => want.filter(|w| w.is_real()).unwrap_or(Ty::LReal),
        };
        let xa = self.coerce(xa, ta, ty, a.pos)?;
        let xb = match tb {
            T::Known(t) if t.is_num() => {
                if t == ty {
                    xb
                } else {
                    Ex::Conv(Box::new(xb), t, ty)
                }
            }
            T::Known(t) => {
                self.err(b.pos, format!("** needs a numeric exponent, found {t}"));
                return None;
            }
            lit => self.lit_as(lit, ty, b.pos)?,
        };
        Some((Ex::Bin(BinOp::Pow, Box::new(xa), Box::new(xb), ty), T::Known(ty)))
    }

    fn unary(&mut self, op: UnOp, a: &Expr, want: Option<Ty>, pos: Pos) -> O<(Ex, T)> {
        let (x, t) = self.synth(a, want)?;
        match (op, t) {
            (UnOp::Neg, T::IntLit(v)) => Some((Ex::Const(Value::Int(-v)), T::IntLit(-v))),
            (UnOp::Neg, T::RealLit(v)) => Some((Ex::Const(Value::Real(-v)), T::RealLit(-v))),
            (UnOp::Neg, T::Known(ty)) if ty.is_signed_int() || ty.is_real() => {
                Some((Ex::Un(op, Box::new(x), ty), T::Known(ty)))
            }
            (UnOp::Neg, T::Known(ty)) => {
                self.err(
                    pos,
                    format!("unary - needs a signed integer or real type, found {ty}"),
                );
                None
            }
            (UnOp::Not, T::Known(ty)) if ty.is_bit() => Some((Ex::Un(op, Box::new(x), ty), T::Known(ty))),
            (UnOp::Not, T::IntLit(_)) if want.is_some_and(Ty::is_bitstring) => {
                let ty = want.unwrap_or(Ty::Byte);
                let x = self.lit_as(t, ty, a.pos)?;
                Some((Ex::Un(op, Box::new(x), ty), T::Known(ty)))
            }
            (UnOp::Not, other) => {
                let found = match other {
                    T::Known(ty) => ty.to_string(),
                    _ => "an untyped literal (write e.g. BYTE#16#0F)".into(),
                };
                self.err(pos, format!("NOT needs BOOL or a bit string, found {found}"));
                None
            }
        }
    }

    /// `A_TO_B(x)` and `TO_B(x)`.
    fn call(&mut self, name: &str, args: &[Expr], pos: Pos) -> O<(Ex, T)> {
        let (from, to) = match name.split_once("_TO_") {
            Some((f, t)) => (Ty::from_name(f), Ty::from_name(t)),
            None => (None, name.strip_prefix("TO_").and_then(Ty::from_name)),
        };
        let typed_from = name.contains("_TO_") && !name.starts_with("TO_");
        let Some(to) = to.filter(|_| !typed_from || from.is_some()) else {
            self.err(pos, format!("unknown function {name}"));
            return None;
        };
        if args.len() != 1 {
            self.err(pos, format!("{name} takes 1 argument, got {}", args.len()));
            return None;
        }
        let arg = &args[0];
        let (x, from) = if let Some(f) = from {
            (self.coerced(arg, f)?, f)
        } else {
            let (x, t) = self.synth(arg, None)?;
            let f = match t {
                T::Known(f) => f,
                lit => Self::default_lit(lit, lit, None),
            };
            (self.coerce(x, t, f, arg.pos)?, f)
        };
        let ex = if from == to {
            x
        } else {
            Ex::Conv(Box::new(x), from, to)
        };
        Some((ex, T::Known(to)))
    }

    // ---- statements ----

    fn block(&mut self, b: &[Stmt]) -> Vec<St> {
        b.iter().filter_map(|s| self.stmt(s)).collect()
    }

    fn cond(&mut self, e: &Expr) -> O<Ex> {
        self.coerced(e, Ty::Bool)
    }

    fn target(&mut self, e: &Expr) -> O<(Place, Ty)> {
        let (place, ty) = match &e.kind {
            ExprKind::Var(name) => {
                let Some(sym) = self.syms.get(name).copied() else {
                    self.err(e.pos, format!("undeclared variable {name}"));
                    return None;
                };
                if sym.constant {
                    self.err(e.pos, format!("{name} is a CONSTANT"));
                    return None;
                }
                match sym.kind {
                    SymKind::Slot(s) => (Place::Slot(s), sym.ty),
                    SymKind::Image(l) => (Place::Image(l), sym.ty),
                }
            }
            ExprKind::Direct(addr) => match parse_address(addr, self.sizes) {
                Ok(l) => (Place::Image(l), l.natural_ty()),
                Err(m) => {
                    self.err(e.pos, m);
                    return None;
                }
            },
            _ => unreachable!("the parser only produces variable and address targets"),
        };
        if let Place::Image(Loc { area: Area::I, .. }) = place {
            self.err(e.pos, "inputs (%I) are read-only");
            return None;
        }
        Some((place, ty))
    }

    fn stmt(&mut self, s: &Stmt) -> O<St> {
        let kind = match &s.kind {
            StmtKind::Empty => return None,
            StmtKind::Assign(t, v) => {
                let target = self.target(t);
                let (place, ty) = target?;
                StKind::Store(place, ty, self.coerced(v, ty)?)
            }
            StmtKind::If(arms, other) => {
                let mut out = Vec::new();
                for (c, b) in arms {
                    let c = self.cond(c);
                    let b = self.block(b);
                    out.push((c?, b));
                }
                StKind::If(out, self.block(other))
            }
            StmtKind::Case(sel, arms, other) => self.case(sel, arms, other)?,
            StmtKind::For {
                var,
                from,
                to,
                by,
                body,
            } => {
                let Some(sym) = self.syms.get(var).copied() else {
                    self.err(s.pos, format!("undeclared variable {var}"));
                    return None;
                };
                let SymKind::Slot(slot) = sym.kind else {
                    self.err(s.pos, format!("FOR variable {var} cannot be located"));
                    return None;
                };
                if !sym.ty.is_int() || sym.constant {
                    self.err(
                        s.pos,
                        format!(
                            "FOR variable {var} must be a non-CONSTANT integer, found {}",
                            sym.ty
                        ),
                    );
                    return None;
                }
                let from = self.coerced(from, sym.ty);
                let to = self.coerced(to, sym.ty);
                let by = match by {
                    Some(b) => self.coerced(b, sym.ty),
                    None => Some(Ex::Const(Value::Int(1))),
                };
                self.loops += 1;
                let body = self.block(body);
                self.loops -= 1;
                StKind::For {
                    slot,
                    ty: sym.ty,
                    from: from?,
                    to: to?,
                    by: by?,
                    body,
                }
            }
            StmtKind::While(c, b) => {
                let c = self.cond(c);
                self.loops += 1;
                let b = self.block(b);
                self.loops -= 1;
                StKind::While(c?, b)
            }
            StmtKind::Repeat(b, c) => {
                self.loops += 1;
                let b = self.block(b);
                self.loops -= 1;
                StKind::Repeat(b, self.cond(c)?)
            }
            StmtKind::Exit | StmtKind::Continue if self.loops == 0 => {
                self.err(s.pos, "EXIT and CONTINUE are only allowed inside a loop");
                return None;
            }
            StmtKind::Exit => StKind::Exit,
            StmtKind::Continue => StKind::Continue,
            StmtKind::Return => StKind::Return,
        };
        Some(St {
            kind,
            line: s.pos.line,
        })
    }

    fn case(&mut self, sel: &Expr, arms: &[(Vec<CaseLabel>, Vec<Stmt>)], other: &[Stmt]) -> O<StKind> {
        let (x, t) = self.synth(sel, None)?;
        let ty = match t {
            T::Known(ty) if ty.is_int() => ty,
            T::Known(ty) => {
                self.err(sel.pos, format!("CASE selector must be an integer, found {ty}"));
                return None;
            }
            lit => Self::default_lit(lit, lit, None),
        };
        let x = self.coerce(x, t, ty, sel.pos)?;
        let (lo, hi) = ty.range();
        let mut seen: Vec<(i128, i128)> = Vec::new();
        let mut out = Vec::new();
        for (labels, body) in arms {
            let mut ranges = Vec::new();
            for l in labels {
                if l.lo < lo || l.hi > hi {
                    self.err(l.pos, format!("CASE label outside {ty} ({lo}..{hi})"));
                } else if seen.iter().any(|&(a, b)| l.lo <= b && a <= l.hi) {
                    self.err(l.pos, "CASE label overlaps an earlier one");
                } else {
                    seen.push((l.lo, l.hi));
                    ranges.push((l.lo, l.hi));
                }
            }
            out.push((ranges, self.block(body)));
        }
        Some(StKind::Case(x, out, self.block(other)))
    }
}

/// Type-checks a parsed program against the given image sizes.
pub fn check(p: &parser::Program, sizes: ImageSizes) -> Result<Checked, Vec<Diagnostic>> {
    let mut ck = Ck {
        sizes,
        syms: HashMap::new(),
        init: Vec::new(),
        diags: Vec::new(),
        loops: 0,
    };
    let mut temps = Vec::new();
    for d in &p.vars {
        ck.decl(d, &mut temps);
    }
    let body = ck.block(&p.body);
    if ck.diags.is_empty() {
        Ok(Checked {
            syms: ck.syms,
            init: ck.init,
            temps,
            body,
        })
    } else {
        ck.diags.sort_by_key(|d| d.pos);
        Err(ck.diags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: ImageSizes = ImageSizes { i: 32, q: 32, m: 64 };

    #[test]
    fn addresses() {
        let a = |s| parse_address(s, SIZES);
        assert_eq!(
            a("IX3.2"),
            Ok(Loc {
                area: Area::I,
                byte: 3,
                bit: 2,
                bits: 1
            })
        );
        assert_eq!(
            a("I3.2"),
            Ok(Loc {
                area: Area::I,
                byte: 3,
                bit: 2,
                bits: 1
            })
        );
        assert_eq!(
            a("QW2"),
            Ok(Loc {
                area: Area::Q,
                byte: 4,
                bit: 0,
                bits: 16
            })
        );
        assert_eq!(
            a("MD15"),
            Ok(Loc {
                area: Area::M,
                byte: 60,
                bit: 0,
                bits: 32
            })
        );
        assert_eq!(
            a("QL3"),
            Ok(Loc {
                area: Area::Q,
                byte: 24,
                bit: 0,
                bits: 64
            })
        );
        for bad in [
            "IX0.8",
            "IX0",
            "QW2.1",
            "QB32",
            "QD8",
            "ML8",
            "Z0",
            "IW",
            "I",
            "IX.1",
            "IX0.",
            "IB99999999999999999999",
        ] {
            assert!(a(bad).is_err(), "{bad}");
        }
        assert!(a("QB31").is_ok());
        assert!(a("QD7").is_ok());
    }
}
