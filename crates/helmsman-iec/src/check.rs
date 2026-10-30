//! Type checking and layout: syntax tree -> typed code.
//!
//! Memory model: every variable is one or more *slots* in a single array.
//! The program's frame starts at slot 0, each function has a fixed frame
//! after it (IEC forbids recursion, so static frames are safe), and a
//! function block instance is a contiguous run of slots inside whoever
//! declares it. Code addresses its own variables relative to the current
//! frame base, so one compiled FB body serves every instance.

// Operands are `a`/`b`, their typed forms `x`/`t`: the idiom of type
// checkers, and clearer here than longer names.
#![allow(clippy::many_single_char_names)]

use std::collections::{HashMap, HashSet};

use crate::interp;
use crate::lexer::Pos;
use crate::parser::{
    self, Arg, BinOp, CaseLabel, Expr, ExprKind, Init, LabelValue, PouKind, Section, Stmt, StmtKind, TypeDef,
    TypeRef, UnOp,
};
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
    if rest.matches('.').count() > usize::from(bits == 1) {
        return Err(format!(
            "%{s}: multi-level addresses (as Beremiz extensions assign) are not supported; \
             this runtime takes %{area}X<byte>.<bit> or %{area}{size}<n>",
            area = char::from(b[0]),
            size = if bits == 1 { 'W' } else { char::from(b[1]) },
        ));
    }
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

// ---- types ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    Elem(Ty),
    Enum(usize),
    /// Array, struct or function block; index into [`Checked::aggs`].
    Agg(usize),
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Type,
    pub off: usize,
}

#[derive(Debug, Clone)]
pub enum AggKind {
    Array { elem: Type, dims: Vec<(i128, i128)> },
    Struct { fields: Vec<FieldInfo> },
    Fb(usize),
}

#[derive(Debug, Clone)]
pub struct AggInfo {
    pub name: String,
    pub kind: AggKind,
    pub size: usize,
    pub init: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub name: String,
    pub values: Vec<(String, i128)>,
}

#[derive(Debug, Clone)]
pub struct VarInfo {
    pub name: String,
    pub ty: Type,
    pub off: usize,
    pub section: Section,
    pub constant: bool,
    pub loc: Option<Loc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PouType {
    Program,
    Function,
    FunctionBlock,
}

/// A program, function or function block after checking.
#[derive(Debug, Clone)]
pub struct PouCode {
    pub name: String,
    pub kind: PouType,
    pub vars: Vec<VarInfo>,
    pub size: usize,
    /// Initial frame contents.
    pub init: Vec<Value>,
    /// `(offset, len)` ranges reset from `init` on every entry.
    pub temps: Vec<(usize, usize)>,
    pub body: Vec<St>,
    /// Absolute frame base (programs and functions; FBs live in instances).
    pub base: usize,
    /// Function result type.
    pub result: Option<Ty>,
}

// ---- typed code ----

/// One array dimension of a runtime index: slot += (idx - lo) * stride,
/// faulting unless lo <= idx < lo + len.
#[derive(Debug, Clone, PartialEq)]
pub struct Dim {
    pub idx: Ex,
    pub lo: i128,
    pub len: i128,
    pub stride: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// Relative to the running POU's frame.
    Frame,
    /// The process image.
    Image(Loc),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub base: Base,
    pub off: usize,
    pub dims: Vec<Dim>,
    /// Partial access to one bit of an integral scalar of the given type.
    pub bit: Option<(u8, Ty)>,
}

impl Place {
    const fn frame(off: usize) -> Self {
        Self {
            base: Base::Frame,
            off,
            dims: Vec::new(),
            bit: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Abs,
    Sqrt,
    Ln,
    Log,
    Exp,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Min,
    Max,
    Limit,
    Sel,
    Mux,
    Shl,
    Shr,
    Rol,
    Ror,
    Trunc,
    /// Time since the program started (the built-in library's clock).
    Now,
}

/// Typed expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Ex {
    Const(Value),
    /// Scalar load; enumerations load as DINT.
    Load(Box<Place>, Ty),
    Un(UnOp, Box<Ex>, Ty),
    /// Both operands have type `.3`; comparisons yield BOOL.
    Bin(BinOp, Box<Ex>, Box<Ex>, Ty),
    /// TIME `*` or `/` an LREAL factor.
    Scale(BinOp, Box<Ex>, Box<Ex>),
    Conv(Box<Ex>, Ty, Ty),
    Call(Box<Call>),
    /// Operands per [`Builtin`]; `.2` is the operand type.
    Builtin(Builtin, Vec<Ex>, Ty),
}

/// A value moved into a callee's frame before the call.
#[derive(Debug, Clone, PartialEq)]
pub enum Move {
    /// `ty` is the slot's runtime type; integers are truncated to it.
    Value {
        off: usize,
        ex: Ex,
        ty: Ty,
    },
    Copy {
        off: usize,
        src: Place,
        len: usize,
    },
}

/// A callee output copied out after the call (`Q => x`).
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub off: usize,
    pub len: usize,
    pub dst: Place,
    /// Scalar conversion from the output's type to the destination's;
    /// `None` for a structured value, which is copied slot for slot.
    pub conv: Option<(Ty, Ty)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub pou: usize,
    /// The instance, for a function block.
    pub inst: Option<Place>,
    pub moves: Vec<Move>,
    pub outs: Vec<Out>,
}

/// Inclusive label ranges and the statements they select.
pub type CaseArm = (Vec<(i128, i128)>, Vec<St>);

#[derive(Debug, Clone, PartialEq)]
pub struct St {
    pub kind: StKind,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StKind {
    Store(Place, Ty, Ex),
    Copy(Place, Place, usize),
    Call(Call),
    If(Vec<(Ex, Vec<St>)>, Vec<St>),
    Case(Ex, Vec<CaseArm>, Vec<St>),
    For {
        var: Place,
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

/// Output of the checker.
#[derive(Debug, Clone)]
pub struct Checked {
    pub pous: Vec<PouCode>,
    pub aggs: Vec<AggInfo>,
    pub program: usize,
    /// Initial contents of the whole slot array.
    pub mem_init: Vec<Value>,
    /// Located variables' initial values, written to the image at a cold
    /// start.
    pub loc_init: Vec<(Loc, Ty, Value)>,
}

/// Type of a synthesized expression: known, or a literal still free to take
/// the type its context needs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum T {
    Known(Type),
    IntLit(i128),
    RealLit(f64),
}

/// Write access through a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    ReadWrite,
    ReadOnly(&'static str),
}

type O<T> = Option<T>;

const BUILTINS: &[(&str, Builtin)] = &[
    ("ABS", Builtin::Abs),
    ("SQRT", Builtin::Sqrt),
    ("LN", Builtin::Ln),
    ("LOG", Builtin::Log),
    ("EXP", Builtin::Exp),
    ("SIN", Builtin::Sin),
    ("COS", Builtin::Cos),
    ("TAN", Builtin::Tan),
    ("ASIN", Builtin::Asin),
    ("ACOS", Builtin::Acos),
    ("ATAN", Builtin::Atan),
    ("MIN", Builtin::Min),
    ("MAX", Builtin::Max),
    ("LIMIT", Builtin::Limit),
    ("SEL", Builtin::Sel),
    ("MUX", Builtin::Mux),
    ("SHL", Builtin::Shl),
    ("SHR", Builtin::Shr),
    ("ROL", Builtin::Rol),
    ("ROR", Builtin::Ror),
    ("TRUNC", Builtin::Trunc),
    ("__NOW", Builtin::Now),
];

/// Named types before resolution.
#[derive(Debug, Clone, Copy)]
enum Named {
    Type(Type),
    /// An alias, resolved on first use.
    Alias(usize),
}

struct PouSrc<'a> {
    pou: &'a parser::Pou,
    system: bool,
}

struct Ck<'a> {
    sizes: ImageSizes,
    srcs: Vec<PouSrc<'a>>,
    pous: Vec<PouCode>,
    pou_index: HashMap<String, usize>,
    aggs: Vec<AggInfo>,
    /// Struct declarations by agg id.
    struct_src: HashMap<usize, &'a [parser::Field]>,
    enums: Vec<EnumInfo>,
    named: HashMap<String, Named>,
    aliases: Vec<(&'a parser::TypeDecl, Option<Type>, bool)>,
    /// 0 = not laid out, 1 = in progress, 2 = done (by agg id).
    agg_state: Vec<u8>,
    /// Same for POU frames.
    pou_state: Vec<u8>,
    /// Calls made by each POU (functions and FB types), for the recursion check.
    calls: Vec<HashSet<usize>>,
    /// Variables whose declaration failed, by POU: already reported, so
    /// their uses are not reported again.
    dropped: HashMap<usize, HashSet<String>>,
    /// Initial values of located variables: (where, type, value).
    loc_init: Vec<(Loc, Ty, Value)>,
    diags: Vec<Diagnostic>,
}

/// Per-body context.
struct Scope {
    pou: usize,
    vars: HashMap<String, usize>,
    loops: usize,
}

impl<'a> Ck<'a> {
    fn err(&mut self, pos: Pos, msg: impl Into<String>) {
        self.diags.push(Diagnostic { pos, msg: msg.into() });
    }

    /// Whether `name` in POU `pou` failed to declare (and was reported).
    fn was_dropped(&self, pou: usize, name: &str) -> bool {
        self.dropped.get(&pou).is_some_and(|d| d.contains(name))
    }

    fn type_name(&self, t: Type) -> String {
        match t {
            Type::Elem(ty) => ty.to_string(),
            Type::Enum(e) => self.enums[e].name.clone(),
            Type::Agg(a) => self.aggs[a].name.clone(),
        }
    }

    fn size_of(&self, t: Type) -> usize {
        match t {
            Type::Agg(a) => self.aggs[a].size,
            _ => 1,
        }
    }

    fn init_of(&self, t: Type) -> Vec<Value> {
        match t {
            Type::Elem(ty) => vec![Value::zero(ty)],
            Type::Enum(e) => vec![Value::Int(self.enums[e].values[0].1)],
            Type::Agg(a) => self.aggs[a].init.clone(),
        }
    }

    // ---- declarations ----

    fn declare(&mut self, name: &str, pos: Pos, what: Named) {
        if Ty::from_name(name).is_some() || self.named.contains_key(name) || self.pou_index.contains_key(name)
        {
            self.err(pos, format!("{name} is already defined"));
        } else {
            self.named.insert(name.to_string(), what);
        }
    }

    fn register_types(&mut self, decls: &'a [parser::TypeDecl]) {
        for d in decls {
            match &d.def {
                TypeDef::Struct(fields) => {
                    let id = self.aggs.len();
                    self.aggs.push(AggInfo {
                        name: d.name.clone(),
                        kind: AggKind::Struct { fields: Vec::new() },
                        size: 0,
                        init: Vec::new(),
                    });
                    self.agg_state.push(0);
                    self.struct_src.insert(id, fields);
                    self.declare(&d.name, d.pos, Named::Type(Type::Agg(id)));
                }
                TypeDef::Enum(values) => {
                    let mut out: Vec<(String, i128)> = Vec::new();
                    let mut next = 0;
                    for (v, vpos, explicit) in values {
                        let val = explicit.unwrap_or(next);
                        if out.iter().any(|(n, _)| n == v) {
                            self.err(*vpos, format!("{v} appears twice in {}", d.name));
                        } else if out.iter().any(|(_, x)| *x == val) {
                            self.err(
                                *vpos,
                                format!("{v} = {val} duplicates another value of {}", d.name),
                            );
                        }
                        out.push((v.clone(), val));
                        next = val + 1;
                    }
                    let id = self.enums.len();
                    self.enums.push(EnumInfo {
                        name: d.name.clone(),
                        values: out,
                    });
                    self.declare(&d.name, d.pos, Named::Type(Type::Enum(id)));
                }
                TypeDef::Alias(..) => {
                    let id = self.aliases.len();
                    self.aliases.push((d, None, false));
                    self.declare(&d.name, d.pos, Named::Alias(id));
                }
            }
        }
    }

    fn resolve(&mut self, t: &TypeRef) -> O<Type> {
        match t {
            TypeRef::Named(n, pos) => {
                if let Some(ty) = Ty::from_name(n) {
                    return Some(Type::Elem(ty));
                }
                match self.named.get(n).copied() {
                    Some(Named::Type(t)) => Some(t),
                    Some(Named::Alias(id)) => self.resolve_alias(id, *pos),
                    None => {
                        let hint = match n.as_str() {
                            "STRING" | "WSTRING" => " (STRING is not supported yet)".to_string(),
                            "DATE" | "DT" | "TOD" | "DATE_AND_TIME" | "TIME_OF_DAY" | "LDATE" | "LDT"
                            | "LTOD" | "LTIME" => format!(" ({n} is not supported yet)"),
                            _ => String::new(),
                        };
                        self.err(*pos, format!("unknown type {n}{hint}"));
                        None
                    }
                }
            }
            TypeRef::Array { dims, elem, pos } => {
                let elem = self.resolve(elem)?;
                let count: i128 = dims.iter().map(|(lo, hi)| hi - lo + 1).product();
                if count > 1_000_000 {
                    self.err(*pos, "arrays are limited to 1,000,000 elements");
                    return None;
                }
                let name = format!(
                    "ARRAY[{}] OF {}",
                    dims.iter()
                        .map(|(l, h)| format!("{l}..{h}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    self.type_name(elem)
                );
                let id = self.aggs.len();
                self.aggs.push(AggInfo {
                    name,
                    kind: AggKind::Array {
                        elem,
                        dims: dims.clone(),
                    },
                    size: 0,
                    init: Vec::new(),
                });
                self.agg_state.push(0);
                Some(Type::Agg(id))
            }
        }
    }

    fn resolve_alias(&mut self, id: usize, pos: Pos) -> O<Type> {
        let (decl, done, busy) = self.aliases[id];
        if let Some(t) = done {
            return Some(t);
        }
        if busy {
            self.err(pos, format!("type {} refers to itself", decl.name));
            return None;
        }
        self.aliases[id].2 = true;
        let TypeDef::Alias(target, _) = &decl.def else {
            unreachable!("aliases only")
        };
        let t = self.resolve(target);
        self.aliases[id] = (decl, t, false);
        t
    }

    /// The default slots for a declaration of type `t` (resolved to `ty`):
    /// an alias's own initial value if it has one (`TYPE Count : DINT := 42`),
    /// else the type's.
    fn default_of(&mut self, t: &TypeRef, ty: Type) -> O<Vec<Value>> {
        if let TypeRef::Named(n, _) = t
            && let Some(Named::Alias(id)) = self.named.get(n).copied()
        {
            let decl = self.aliases[id].0;
            let TypeDef::Alias(target, init) = &decl.def else {
                unreachable!("aliases only")
            };
            // An alias without its own value inherits its target's.
            return match init {
                Some(i) => self.init_value(i, ty, None),
                None => self.default_of(target, ty),
            };
        }
        Some(self.init_of(ty))
    }

    /// Computes size and initial value of an aggregate (and its parts).
    fn layout_agg(&mut self, id: usize, pos: Pos) -> bool {
        match self.agg_state[id] {
            2 => return true,
            1 => {
                let n = self.aggs[id].name.clone();
                self.err(pos, format!("{n} contains itself"));
                return false;
            }
            _ => {}
        }
        self.agg_state[id] = 1;
        let ok = match self.aggs[id].kind.clone() {
            AggKind::Array { elem, dims } => {
                if self.layout_type(elem, pos) {
                    let count: usize = dims
                        .iter()
                        .map(|(lo, hi)| usize::try_from(hi - lo + 1).unwrap_or(0))
                        .product();
                    let one = self.init_of(elem);
                    let init: Vec<Value> = one.iter().copied().cycle().take(one.len() * count).collect();
                    self.aggs[id].size = self.size_of(elem) * count;
                    self.aggs[id].init = init;
                    true
                } else {
                    false
                }
            }
            AggKind::Struct { .. } => {
                let src = self.struct_src[&id];
                let mut fields = Vec::new();
                let mut init = Vec::new();
                let mut ok = true;
                for f in src {
                    if fields.iter().any(|x: &FieldInfo| x.name == f.name) {
                        self.err(f.pos, format!("field {} appears twice", f.name));
                        ok = false;
                        continue;
                    }
                    let Some(ty) = self.resolve(&f.ty) else {
                        ok = false;
                        continue;
                    };
                    if matches!(ty, Type::Agg(a) if matches!(self.aggs[a].kind, AggKind::Fb(_))) {
                        self.err(f.pos, "a STRUCT cannot hold a function block instance");
                        ok = false;
                        continue;
                    }
                    if !self.layout_type(ty, f.pos) {
                        ok = false;
                        continue;
                    }
                    let init_v = match &f.init {
                        Some(i) => self.init_value(i, ty, None),
                        None => self.default_of(&f.ty, ty),
                    };
                    let Some(v) = init_v else {
                        ok = false;
                        continue;
                    };
                    fields.push(FieldInfo {
                        name: f.name.clone(),
                        ty,
                        off: init.len(),
                    });
                    init.extend(v);
                }
                self.aggs[id].size = init.len();
                self.aggs[id].init = init;
                self.aggs[id].kind = AggKind::Struct { fields };
                ok
            }
            AggKind::Fb(p) => {
                let ok = self.layout_pou(p);
                self.aggs[id].size = self.pous[p].size;
                self.aggs[id].init.clone_from(&self.pous[p].init);
                ok
            }
        };
        self.agg_state[id] = 2;
        ok
    }

    fn layout_type(&mut self, t: Type, pos: Pos) -> bool {
        match t {
            Type::Agg(a) => self.layout_agg(a, pos),
            _ => true,
        }
    }

    /// Lays out a POU's frame: variable offsets and initial values.
    #[allow(clippy::too_many_lines)]
    fn layout_pou(&mut self, p: usize) -> bool {
        match self.pou_state[p] {
            2 => return true,
            1 => {
                let (n, pos) = (self.pous[p].name.clone(), self.srcs[p].pou.pos);
                self.err(pos, format!("function block {n} contains an instance of itself"));
                return false;
            }
            _ => {}
        }
        self.pou_state[p] = 1;
        let src = self.srcs[p].pou;
        let kind = self.pous[p].kind;
        let mut vars: Vec<VarInfo> = Vec::new();
        let mut init: Vec<Value> = Vec::new();
        let mut temps = Vec::new();
        let mut ok = true;
        // A function's result is slot 0, named after the function.
        if let PouKind::Function(rt) = &src.kind {
            match self.resolve(rt) {
                Some(Type::Elem(ty)) => {
                    self.pous[p].result = Some(ty);
                    vars.push(VarInfo {
                        name: src.name.clone(),
                        ty: Type::Elem(ty),
                        off: 0,
                        section: Section::Output,
                        constant: false,
                        loc: None,
                    });
                    init.push(Value::zero(ty));
                }
                Some(Type::Enum(e)) => {
                    self.pous[p].result = Some(Ty::DInt);
                    vars.push(VarInfo {
                        name: src.name.clone(),
                        ty: Type::Enum(e),
                        off: 0,
                        section: Section::Output,
                        constant: false,
                        loc: None,
                    });
                    init.push(Value::Int(self.enums[e].values[0].1));
                }
                Some(t) => {
                    let n = self.type_name(t);
                    self.err(
                        rt.pos(),
                        format!("a FUNCTION must return an elementary type or enumeration, not {n}"),
                    );
                    ok = false;
                }
                None => ok = false,
            }
        }
        for d in &src.vars {
            let allowed = matches!(
                (kind, d.section),
                (_, Section::Var | Section::Temp)
                    | (
                        PouType::FunctionBlock | PouType::Program,
                        Section::Input | Section::Output
                    )
                    | (PouType::Function, Section::Input)
            );
            if !allowed {
                let what = match d.section {
                    Section::Input => "VAR_INPUT",
                    Section::Output => "VAR_OUTPUT",
                    Section::InOut => "VAR_IN_OUT",
                    _ => "this section",
                };
                let pk = match kind {
                    PouType::Program => "a PROGRAM",
                    PouType::Function => "a FUNCTION",
                    PouType::FunctionBlock => "a FUNCTION_BLOCK",
                };
                self.err(d.pos, format!("{what} is not supported in {pk} yet"));
                ok = false;
                continue;
            }
            if vars.iter().any(|v| v.name == d.name) {
                self.err(d.pos, format!("{} is declared twice", d.name));
                ok = false;
                continue;
            }
            let Some(ty) = self.resolve(&d.ty) else {
                ok = false;
                continue;
            };
            if let Type::Agg(a) = ty
                && let AggKind::Fb(fb) = self.aggs[a].kind
                && fb == p
            {
                self.err(
                    d.pos,
                    format!(
                        "function block {} contains an instance of itself",
                        self.pous[p].name
                    ),
                );
                ok = false;
                continue;
            }
            if !self.layout_type(ty, d.pos) {
                ok = false;
                continue;
            }
            // A program's inputs and outputs are its ports: the host binds
            // them to signals by name, one scalar each.
            if kind == PouType::Program && matches!(d.section, Section::Input | Section::Output) {
                let problem = if !matches!(ty, Type::Elem(_)) {
                    Some("a PROGRAM input or output must be an elementary type")
                } else if d.at.is_some() {
                    Some("a PROGRAM input or output is bound by name, not located")
                } else if d.constant {
                    Some("a PROGRAM input or output cannot be CONSTANT")
                } else {
                    None
                };
                if let Some(m) = problem {
                    self.err(d.pos, format!("{}: {m}", d.name));
                    ok = false;
                    continue;
                }
            }
            if let Some((addr, apos)) = &d.at {
                let problem = if kind != PouType::Program {
                    Some("AT is only allowed in a PROGRAM")
                } else if d.section == Section::Temp {
                    Some("VAR_TEMP cannot be located")
                } else if d.constant {
                    Some("a CONSTANT cannot be located")
                } else if d.init.is_some() && addr.starts_with('I') {
                    Some("an input (%I) cannot take an initial value")
                } else {
                    None
                };
                if let Some(m) = problem {
                    self.err(d.pos, format!("{}: {m}", d.name));
                    ok = false;
                    continue;
                }
                let Type::Elem(ety) = ty else {
                    self.err(
                        *apos,
                        format!("{}: only elementary variables can be located", d.name),
                    );
                    ok = false;
                    continue;
                };
                match parse_address(addr, self.sizes) {
                    Ok(loc)
                        if (loc.bits == 1) != (ety == Ty::Bool)
                            || (loc.bits != 1 && loc.bits != ety.bits()) =>
                    {
                        self.err(
                            *apos,
                            format!("%{addr} is {} bits wide, {ety} is {}", loc.bits, ety.bits()),
                        );
                        ok = false;
                    }
                    Ok(loc) => {
                        // Written to the image once, before the first scan.
                        if let Some(i) = &d.init {
                            let scope = Scope {
                                pou: p,
                                vars: vars
                                    .iter()
                                    .enumerate()
                                    .map(|(k, v)| (v.name.clone(), k))
                                    .collect(),
                                loops: 0,
                            };
                            self.pous[p].vars.clone_from(&vars);
                            self.pous[p].init.clone_from(&init);
                            match self.init_value(i, ty, Some(&scope)).as_deref() {
                                Some(&[v]) => self.loc_init.push((loc, ety, v)),
                                _ => ok = false,
                            }
                        }
                        vars.push(VarInfo {
                            name: d.name.clone(),
                            ty,
                            off: 0,
                            section: d.section,
                            constant: false,
                            loc: Some(loc),
                        });
                    }
                    Err(m) => {
                        self.err(*apos, m);
                        ok = false;
                    }
                }
                continue;
            }
            if d.constant && d.init.is_none() {
                self.err(d.pos, format!("CONSTANT {} needs an initial value", d.name));
                ok = false;
                continue;
            }
            let off = init.len();
            let mut v = self.default_of(&d.ty, ty).unwrap_or_else(|| {
                ok = false;
                self.init_of(ty)
            });
            if let Some(i) = &d.init {
                // Earlier variables are in scope; only CONSTANTs are usable.
                let scope = Scope {
                    pou: p,
                    vars: vars
                        .iter()
                        .enumerate()
                        .map(|(k, v)| (v.name.clone(), k))
                        .collect(),
                    loops: 0,
                };
                self.pous[p].vars.clone_from(&vars);
                self.pous[p].init.clone_from(&init);
                match self.init_value(i, ty, Some(&scope)) {
                    Some(x) => v = x,
                    None => ok = false,
                }
            }
            let len = v.len();
            init.extend(v);
            if d.section == Section::Temp {
                temps.push((off, len));
            }
            vars.push(VarInfo {
                name: d.name.clone(),
                ty,
                off,
                section: d.section,
                constant: d.constant,
                loc: None,
            });
        }
        let dropped: HashSet<String> = src
            .vars
            .iter()
            .filter(|d| !vars.iter().any(|v| v.name == d.name))
            .map(|d| d.name.clone())
            .collect();
        if !dropped.is_empty() {
            self.dropped.insert(p, dropped);
        }
        let pc = &mut self.pous[p];
        pc.size = init.len();
        pc.vars = vars;
        pc.init = init;
        pc.temps = temps;
        self.pou_state[p] = 2;
        ok
    }

    /// Initial slots for a value of type `ty`. `scope` gives access to the
    /// enclosing POU's constants (initialised so far in its `init`).
    fn init_value(&mut self, i: &Init, ty: Type, scope: Option<&Scope>) -> O<Vec<Value>> {
        match (i, ty) {
            (Init::Expr(e), Type::Elem(_) | Type::Enum(_)) => {
                let empty = Scope {
                    pou: usize::MAX,
                    vars: HashMap::new(),
                    loops: 0,
                };
                let sc = scope.unwrap_or(&empty);
                let ex = self.coerced(e, ty, sc)?;
                if !Self::is_const(&ex) {
                    self.err(e.pos, "an initial value may use only literals and CONSTANTs");
                    return None;
                }
                match interp::eval_const(&ex) {
                    Ok(v) => Some(vec![v]),
                    Err(m) => {
                        self.err(e.pos, m);
                        None
                    }
                }
            }
            (Init::Array(items, pos), Type::Agg(a)) if matches!(self.aggs[a].kind, AggKind::Array { .. }) => {
                let AggKind::Array { elem, dims } = self.aggs[a].kind.clone() else {
                    unreachable!()
                };
                let count: usize = dims
                    .iter()
                    .map(|(lo, hi)| usize::try_from(hi - lo + 1).unwrap_or(0))
                    .product();
                let mut out = Vec::new();
                let mut n = 0;
                for (rep, item) in items {
                    let v = self.init_value(item, elem, scope)?;
                    for _ in 0..*rep {
                        out.extend(v.iter().copied());
                    }
                    n += rep;
                }
                if n > count {
                    self.err(*pos, format!("{n} initial values for {count} elements"));
                    return None;
                }
                // Elements not listed keep their type's initial value.
                let mut full = self.aggs[a].init.clone();
                full[..out.len()].copy_from_slice(&out);
                Some(full)
            }
            (Init::Struct(fields, _), Type::Agg(a))
                if matches!(self.aggs[a].kind, AggKind::Struct { .. }) =>
            {
                let AggKind::Struct { fields: info } = self.aggs[a].kind.clone() else {
                    unreachable!()
                };
                let mut full = self.aggs[a].init.clone();
                for (name, fpos, fi) in fields {
                    let Some(f) = info.iter().find(|f| &f.name == name) else {
                        let n = self.aggs[a].name.clone();
                        self.err(*fpos, format!("{n} has no field {name}"));
                        return None;
                    };
                    let v = self.init_value(fi, f.ty, scope)?;
                    full[f.off..f.off + v.len()].copy_from_slice(&v);
                }
                Some(full)
            }
            (other, t) => {
                let n = self.type_name(t);
                self.err(other.pos(), format!("this initial value does not fit {n}"));
                None
            }
        }
    }

    /// Whether an integer expression's value is carried wider than its
    /// type (a binary arithmetic result, or a negation of one).
    fn widened(ex: &Ex) -> bool {
        match ex {
            Ex::Bin(BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod, _, _, t) => {
                t.arith() != *t
            }
            Ex::Un(UnOp::Neg, inner, _) => Self::widened(inner),
            _ => false,
        }
    }

    fn is_const(ex: &Ex) -> bool {
        match ex {
            Ex::Const(_) => true,
            Ex::Un(_, a, _) | Ex::Conv(a, _, _) => Self::is_const(a),
            Ex::Bin(_, a, b, _) | Ex::Scale(_, a, b) => Self::is_const(a) && Self::is_const(b),
            // CONSTANTs fold to `Const`, so any load reads a variable.
            Ex::Load(..) | Ex::Call(_) | Ex::Builtin(..) => false,
        }
    }

    // ---- places ----

    /// Resolves a variable path to a place.
    #[allow(clippy::too_many_lines)]
    fn place(&mut self, e: &Expr, sc: &Scope) -> O<(Place, Type, Access)> {
        match &e.kind {
            ExprKind::Var(name) => {
                let Some(&k) = sc.vars.get(name) else {
                    if self.was_dropped(sc.pou, name) {
                        return None;
                    }
                    let hint = if self.pou_index.contains_key(name) {
                        " (a function is called with parentheses)"
                    } else {
                        ""
                    };
                    self.err(e.pos, format!("undeclared variable {name}{hint}"));
                    return None;
                };
                let v = &self.pous[sc.pou].vars[k];
                let acc = if v.constant {
                    Access::ReadOnly("is a CONSTANT")
                } else {
                    Access::ReadWrite
                };
                Some(match v.loc {
                    Some(loc) => {
                        let acc = if loc.area == Area::I {
                            Access::ReadOnly("cannot be written: inputs (%I) are read-only")
                        } else {
                            acc
                        };
                        (
                            Place {
                                base: Base::Image(loc),
                                off: 0,
                                dims: Vec::new(),
                                bit: None,
                            },
                            v.ty,
                            acc,
                        )
                    }
                    None => (Place::frame(v.off), v.ty, acc),
                })
            }
            ExprKind::Direct(addr) => match parse_address(addr, self.sizes) {
                Ok(loc) => {
                    let acc = if loc.area == Area::I {
                        Access::ReadOnly("cannot be written: inputs (%I) are read-only")
                    } else {
                        Access::ReadWrite
                    };
                    Some((
                        Place {
                            base: Base::Image(loc),
                            off: 0,
                            dims: Vec::new(),
                            bit: None,
                        },
                        Type::Elem(loc.natural_ty()),
                        acc,
                    ))
                }
                Err(m) => {
                    self.err(e.pos, m);
                    None
                }
            },
            ExprKind::Member(base, m) => {
                let (mut p, t, acc) = self.place(base, sc)?;
                let Type::Agg(a) = t else {
                    let n = self.type_name(t);
                    self.err(e.pos, format!("{n} has no members"));
                    return None;
                };
                if p.bit.is_some() {
                    self.err(e.pos, "a bit has no members");
                    return None;
                }
                match &self.aggs[a].kind {
                    AggKind::Struct { fields } => {
                        let Some(f) = fields.iter().find(|f| &f.name == m) else {
                            let n = self.aggs[a].name.clone();
                            self.err(e.pos, format!("{n} has no field {m}"));
                            return None;
                        };
                        p.off += f.off;
                        Some((p, f.ty, acc))
                    }
                    AggKind::Fb(fb) => {
                        let fb = *fb;
                        let Some(v) = self.pous[fb].vars.iter().find(|v| &v.name == m) else {
                            let n = self.pous[fb].name.clone();
                            self.err(e.pos, format!("{n} has no input or output {m}"));
                            return None;
                        };
                        let (off, ty, section) = (v.off, v.ty, v.section);
                        let acc = match section {
                            Section::Input => acc,
                            Section::Output => Access::ReadOnly("is an output of the function block"),
                            _ => {
                                let n = self.pous[fb].name.clone();
                                self.err(
                                    e.pos,
                                    format!("{m} is internal to {n}; only inputs and outputs are visible"),
                                );
                                return None;
                            }
                        };
                        p.off += off;
                        Some((p, ty, acc))
                    }
                    AggKind::Array { .. } => {
                        self.err(e.pos, format!("an array has no member {m}; index it with [ ]"));
                        None
                    }
                }
            }
            ExprKind::Index(base, idx) => {
                let (mut p, t, acc) = self.place(base, sc)?;
                let arr = match t {
                    Type::Agg(a) => match &self.aggs[a].kind {
                        AggKind::Array { elem, dims } => Some((*elem, dims.clone())),
                        _ => None,
                    },
                    _ => None,
                };
                let Some((elem, dims)) = arr else {
                    let n = self.type_name(t);
                    self.err(e.pos, format!("{n} is not an array"));
                    return None;
                };
                if idx.len() != dims.len() {
                    self.err(
                        e.pos,
                        format!("this array takes {} index(es), got {}", dims.len(), idx.len()),
                    );
                    return None;
                }
                let esize = self.size_of(elem);
                let mut ok = true;
                for (k, ie) in idx.iter().enumerate() {
                    let stride = esize
                        * dims[k + 1..]
                            .iter()
                            .map(|(lo, hi)| usize::try_from(hi - lo + 1).unwrap_or(0))
                            .product::<usize>();
                    let (lo, hi) = dims[k];
                    let Some((x, it)) = self.synth(ie, None, sc) else {
                        ok = false;
                        continue;
                    };
                    let x = match it {
                        T::IntLit(v) => Ex::Const(Value::Int(v)),
                        T::Known(Type::Elem(ty)) if ty.is_int() => x,
                        T::Known(other) => {
                            let n = self.type_name(other);
                            self.err(ie.pos, format!("an array index must be an integer, found {n}"));
                            ok = false;
                            continue;
                        }
                        T::RealLit(_) => {
                            self.err(ie.pos, "an array index must be an integer");
                            ok = false;
                            continue;
                        }
                    };
                    if let Ex::Const(Value::Int(v)) = x {
                        if v < lo || v > hi {
                            self.err(ie.pos, format!("index {v} is outside {lo}..{hi}"));
                            ok = false;
                            continue;
                        }
                        p.off += usize::try_from(v - lo).unwrap_or(0) * stride;
                    } else {
                        if matches!(p.base, Base::Image(_)) {
                            self.err(ie.pos, "located variables cannot be indexed");
                            ok = false;
                            continue;
                        }
                        p.dims.push(Dim {
                            idx: x,
                            lo,
                            len: hi - lo + 1,
                            stride,
                        });
                    }
                }
                ok.then_some((p, elem, acc))
            }
            ExprKind::Bit(base, n) => {
                let (mut p, t, acc) = self.place(base, sc)?;
                let ok_ty = match t {
                    Type::Elem(ty) => (ty.is_int() || ty.is_bitstring()).then_some(ty),
                    _ => None,
                };
                let Some(ty) = ok_ty else {
                    let tn = self.type_name(t);
                    self.err(
                        e.pos,
                        format!("bit access needs an integer or bit string, found {tn}"),
                    );
                    return None;
                };
                if u32::from(*n) >= ty.bits() || p.bit.is_some() {
                    self.err(e.pos, format!("{ty} has bits 0..{}", ty.bits() - 1));
                    return None;
                }
                p.bit = Some((*n, ty));
                Some((p, Type::Elem(Ty::Bool), acc))
            }
            _ => {
                self.err(e.pos, "expected a variable");
                None
            }
        }
    }

    /// Runtime scalar type of a scalar `Type`.
    const fn rt(t: Type) -> Option<Ty> {
        match t {
            Type::Elem(ty) => Some(ty),
            Type::Enum(_) => Some(Ty::DInt),
            Type::Agg(_) => None,
        }
    }

    // ---- expressions ----

    fn enum_value(&mut self, name: &str, pos: Pos) -> O<(usize, i128)> {
        let hits: Vec<(usize, i128)> = self
            .enums
            .iter()
            .enumerate()
            .filter_map(|(k, e)| e.values.iter().find(|(n, _)| n == name).map(|(_, v)| (k, *v)))
            .collect();
        match hits.as_slice() {
            [one] => Some(*one),
            [] => None,
            _ => {
                self.err(
                    pos,
                    format!("{name} is a value of several enumerations; write Type#{name}"),
                );
                None
            }
        }
    }

    /// Synthesizes the type of `e`; `want` is the type the context expects,
    /// used only to type literals.
    fn synth(&mut self, e: &Expr, want: Option<Type>, sc: &Scope) -> O<(Ex, T)> {
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
                Some((ex, T::Known(Type::Elem(ty))))
            }
            ExprKind::Real {
                value,
                prefix: Some(p),
            } => {
                let ty = self.prefix_ty(p, e.pos)?;
                let ex = self.lit_as(T::RealLit(*value), ty, e.pos)?;
                Some((ex, T::Known(Type::Elem(ty))))
            }
            ExprKind::Bool(b) => Some((Ex::Const(Value::Bool(*b)), T::Known(Type::Elem(Ty::Bool)))),
            ExprKind::Time(ns) => Some((Ex::Const(Value::Int(*ns)), T::Known(Type::Elem(Ty::Time)))),
            ExprKind::EnumLit(tn, v) => {
                let Some(Named::Type(Type::Enum(id))) = self.named.get(tn).copied() else {
                    self.err(e.pos, format!("{tn} is not an enumeration type"));
                    return None;
                };
                let Some((_, val)) = self.enums[id].values.iter().find(|(n, _)| n == v) else {
                    self.err(e.pos, format!("{tn} has no value {v}"));
                    return None;
                };
                Some((Ex::Const(Value::Int(*val)), T::Known(Type::Enum(id))))
            }
            ExprKind::Var(name) if !sc.vars.contains_key(name) => {
                if let Some((id, val)) = self.enum_value(name, e.pos) {
                    return Some((Ex::Const(Value::Int(val)), T::Known(Type::Enum(id))));
                }
                self.place(e, sc)
                    .map(|_| unreachable!("place() reports the undeclared name"))
            }
            ExprKind::Var(_)
            | ExprKind::Direct(_)
            | ExprKind::Member(..)
            | ExprKind::Index(..)
            | ExprKind::Bit(..) => {
                let (p, t, _) = self.place(e, sc)?;
                let Some(rt) = Self::rt(t) else {
                    let n = self.type_name(t);
                    self.err(
                        e.pos,
                        format!("{n} is a structured value: assign or pass it whole, or select a part"),
                    );
                    return None;
                };
                // A constant scalar folds to its value (keeps initialisers constant).
                if p.base == Base::Frame
                    && p.dims.is_empty()
                    && p.bit.is_none()
                    && let ExprKind::Var(n) = &e.kind
                    && let Some(&k) = sc.vars.get(n)
                    && self.pous[sc.pou].vars[k].constant
                    && let Some(v) = self.pous[sc.pou].init.get(p.off)
                {
                    return Some((Ex::Const(*v), T::Known(t)));
                }
                Some((Ex::Load(Box::new(p), rt), T::Known(t)))
            }
            ExprKind::Unary(op, a) => self.unary(*op, a, want, e.pos, sc),
            ExprKind::Binary(BinOp::Pow, a, b) => self.power(a, b, want, e.pos, sc),
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, want, e.pos, sc),
            ExprKind::Call(callee, args) => self.call_expr(callee, args, want, e.pos, sc),
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
            // boolean_literal: [BOOL#] (1 | 0) | TRUE | FALSE.
            (T::IntLit(v @ (0 | 1)), Ty::Bool) => Value::Bool(v == 1),
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

    /// `ex` (of type `t`) as type `to`: identity, an implicit widening, or a
    /// literal taking on `to`.
    fn coerce(&mut self, ex: Ex, t: T, to: Type, pos: Pos) -> O<Ex> {
        match (t, to) {
            (T::Known(from), to) if from == to => Some(ex),
            (T::Known(Type::Elem(f)), Type::Elem(t)) if f.widens_to(t) => Some(Ex::Conv(Box::new(ex), f, t)),
            (T::Known(Type::Elem(f)), Type::Elem(t)) => {
                self.err(
                    pos,
                    format!("expected {t}, found {f}; convert explicitly with {f}_TO_{t}"),
                );
                None
            }
            (T::Known(from), to) => {
                let (a, b) = (self.type_name(to), self.type_name(from));
                self.err(pos, format!("expected {a}, found {b}"));
                None
            }
            (lit, Type::Elem(t)) => self.lit_as(lit, t, pos),
            (_, to) => {
                let n = self.type_name(to);
                self.err(pos, format!("a number cannot be a {n}"));
                None
            }
        }
    }

    fn coerced(&mut self, e: &Expr, to: Type, sc: &Scope) -> O<Ex> {
        let (ex, t) = self.synth(e, Some(to), sc)?;
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

    /// The elementary type two operands are brought to; `what` names the
    /// operation in errors.
    fn operand_ty(&mut self, a: T, b: T, want: Option<Ty>, what: &str, pos: Pos) -> O<Ty> {
        let lit_ok = |lit: T, k: Ty| match lit {
            T::IntLit(_) => (k.is_integral() && k != Ty::Bool) || k.is_real(),
            T::RealLit(_) => k.is_real(),
            T::Known(_) => false,
        };
        match (a, b) {
            (T::Known(Type::Elem(x)), T::Known(Type::Elem(y))) => {
                let common = Ty::common(x, y);
                if common.is_none() {
                    self.err(pos, format!("{what} on {x} and {y}: no implicit conversion between them; convert one explicitly"));
                }
                common
            }
            (T::Known(Type::Elem(k)), lit) | (lit, T::Known(Type::Elem(k)))
                if !matches!(lit, T::Known(_)) =>
            {
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
            (T::Known(x), T::Known(y)) => {
                let (a, b) = (self.type_name(x), self.type_name(y));
                self.err(pos, format!("{what} cannot take {a} and {b}"));
                None
            }
            (T::Known(x), _) | (_, T::Known(x)) => {
                let n = self.type_name(x);
                self.err(pos, format!("{what} cannot take {n} and a number"));
                None
            }
            (a, b) => Some(Self::default_lit(a, b, want)),
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        a: &Expr,
        b: &Expr,
        want: Option<Type>,
        pos: Pos,
        sc: &Scope,
    ) -> O<(Ex, T)> {
        let cmp = op.is_comparison();
        let inner_want = if cmp { None } else { want };
        let ra = self.synth(a, inner_want, sc);
        let rb = self.synth(b, inner_want, sc);
        let ((xa, ta), (xb, tb)) = (ra?, rb?);
        // Enumerations: = and <> only, same type.
        if let (T::Known(Type::Enum(x)), T::Known(Type::Enum(y))) = (ta, tb) {
            if x != y || !matches!(op, BinOp::Eq | BinOp::Ne) {
                let (n1, n2) = (self.enums[x].name.clone(), self.enums[y].name.clone());
                self.err(pos, format!("{} cannot compare {n1} and {n2}", op.symbol()));
                return None;
            }
            return Some((
                Ex::Bin(op, Box::new(xa), Box::new(xb), Ty::DInt),
                T::Known(Type::Elem(Ty::Bool)),
            ));
        }
        // TIME scaled by a number.
        let is_time = |t: T| t == T::Known(Type::Elem(Ty::Time));
        let is_num = |t: T| {
            matches!(t, T::IntLit(_) | T::RealLit(_)) || matches!(t, T::Known(Type::Elem(n)) if n.is_num())
        };
        if matches!(op, BinOp::Mul | BinOp::Div)
            && ((is_time(ta) && is_num(tb)) || (op == BinOp::Mul && is_num(ta) && is_time(tb)))
        {
            let ((tx, _), (nx, nt, npos)) = if is_time(ta) {
                ((xa, ta), (xb, tb, b.pos))
            } else {
                ((xb, tb), (xa, ta, a.pos))
            };
            let factor = match nt {
                T::Known(Type::Elem(Ty::LReal)) => nx,
                T::Known(Type::Elem(n)) => Ex::Conv(Box::new(nx), n, Ty::LReal),
                lit => self.lit_as(lit, Ty::LReal, npos)?,
            };
            return Some((
                Ex::Scale(op, Box::new(tx), Box::new(factor)),
                T::Known(Type::Elem(Ty::Time)),
            ));
        }
        let want_ty = inner_want.and_then(Self::rt);
        let ty = self.operand_ty(ta, tb, want_ty, op.symbol(), pos)?;
        let ok = match op {
            BinOp::Add | BinOp::Sub => ty.is_num() || ty == Ty::Time,
            BinOp::Mul | BinOp::Div => ty.is_num(),
            BinOp::Mod => ty.is_int(),
            BinOp::And | BinOp::Or | BinOp::Xor => ty.is_bit(),
            _ => true,
        };
        if !ok {
            let need = match op {
                BinOp::Mod => "an integer type",
                BinOp::And | BinOp::Or | BinOp::Xor => "BOOL or a bit string (BYTE..LWORD)",
                BinOp::Mul | BinOp::Div if ty == Ty::Time => "a TIME and a number",
                _ => "a numeric type",
            };
            self.err(pos, format!("{} needs {need}, found {ty}", op.symbol()));
            return None;
        }
        let xa = self.coerce(xa, ta, Type::Elem(ty), a.pos);
        let xb = self.coerce(xb, tb, Type::Elem(ty), b.pos);
        let ex = Ex::Bin(op, Box::new(xa?), Box::new(xb?), ty);
        Some((ex, T::Known(Type::Elem(if cmp { Ty::Bool } else { ty }))))
    }

    fn power(&mut self, a: &Expr, b: &Expr, want: Option<Type>, pos: Pos, sc: &Scope) -> O<(Ex, T)> {
        let base = self.synth(a, want, sc);
        let exp = self.synth(b, None, sc);
        let ((xa, ta), (xb, tb)) = (base?, exp?);
        let ty = match ta {
            T::Known(Type::Elem(t)) if t.is_real() => t,
            T::Known(t) => {
                let n = self.type_name(t);
                self.err(
                    pos,
                    format!("** needs a REAL or LREAL base, found {n}; convert with {n}_TO_LREAL"),
                );
                return None;
            }
            _ => want
                .and_then(Self::rt)
                .filter(|w| w.is_real())
                .unwrap_or(Ty::LReal),
        };
        let xa = self.coerce(xa, ta, Type::Elem(ty), a.pos)?;
        let xb = match tb {
            T::Known(Type::Elem(t)) if t.is_num() => {
                if t == ty {
                    xb
                } else {
                    Ex::Conv(Box::new(xb), t, ty)
                }
            }
            T::Known(t) => {
                let n = self.type_name(t);
                self.err(b.pos, format!("** needs a numeric exponent, found {n}"));
                return None;
            }
            lit => self.lit_as(lit, ty, b.pos)?,
        };
        Some((
            Ex::Bin(BinOp::Pow, Box::new(xa), Box::new(xb), ty),
            T::Known(Type::Elem(ty)),
        ))
    }

    fn unary(&mut self, op: UnOp, a: &Expr, want: Option<Type>, pos: Pos, sc: &Scope) -> O<(Ex, T)> {
        let (x, t) = self.synth(a, want, sc)?;
        match (op, t) {
            (UnOp::Neg, T::IntLit(v)) => Some((Ex::Const(Value::Int(-v)), T::IntLit(-v))),
            (UnOp::Neg, T::RealLit(v)) => Some((Ex::Const(Value::Real(-v)), T::RealLit(-v))),
            (UnOp::Neg, T::Known(Type::Elem(ty))) if ty.is_signed_int() || ty.is_real() || ty == Ty::Time => {
                // Negation works at its operand's width: a variable's own,
                // or the widened width of an arithmetic result (as ruSTy).
                let width = if Self::widened(&x) { ty.arith() } else { ty };
                Some((Ex::Un(op, Box::new(x), width), t))
            }
            (UnOp::Not, T::Known(Type::Elem(ty))) if ty.is_bit() => Some((Ex::Un(op, Box::new(x), ty), t)),
            (UnOp::Not, T::IntLit(_)) if want.and_then(Self::rt).is_some_and(Ty::is_bitstring) => {
                let ty = want.and_then(Self::rt).unwrap_or(Ty::Byte);
                let x = self.lit_as(t, ty, a.pos)?;
                Some((Ex::Un(op, Box::new(x), ty), T::Known(Type::Elem(ty))))
            }
            (UnOp::Neg, other) => {
                let found = match other {
                    T::Known(k) => self.type_name(k),
                    _ => unreachable!("literals handled above"),
                };
                self.err(
                    pos,
                    format!("unary - needs a signed integer, real or TIME, found {found}"),
                );
                None
            }
            (UnOp::Not, other) => {
                let found = match other {
                    T::Known(k) => self.type_name(k),
                    _ => "an untyped literal (write e.g. BYTE#16#0F)".into(),
                };
                self.err(pos, format!("NOT needs BOOL or a bit string, found {found}"));
                None
            }
        }
    }

    // ---- calls ----

    fn call_expr(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        want: Option<Type>,
        pos: Pos,
        sc: &Scope,
    ) -> O<(Ex, T)> {
        let ExprKind::Var(name) = &callee.kind else {
            self.err(
                pos,
                "a function block call is a statement: call the instance, then read its outputs",
            );
            return None;
        };
        if sc.vars.contains_key(name) {
            self.err(
                pos,
                format!("{name} is a variable; a function block call is a statement"),
            );
            return None;
        }
        if let Some(&p) = self.pou_index.get(name) {
            if self.pous[p].kind != PouType::Function {
                self.err(
                    pos,
                    format!("{name} is not a function; call an instance of it as a statement"),
                );
                return None;
            }
            let call = self.fn_call(p, args, sc)?;
            let rt = self.pous[p].vars[0].ty;
            return Some((Ex::Call(Box::new(call)), T::Known(rt)));
        }
        self.builtin(name, args, want, pos, sc)
    }

    fn positional(&mut self, name: &str, args: &[Arg], pos: Pos) -> bool {
        if let Some(a) = args.iter().find(|a| a.name.is_some()) {
            self.err(a.pos, format!("{name} takes positional arguments only"));
            return false;
        }
        if args.is_empty() && name != "__NOW" {
            self.err(pos, format!("{name} needs arguments"));
            return false;
        }
        true
    }

    /// Standard functions and conversions.
    #[allow(clippy::too_many_lines)]
    fn builtin(&mut self, name: &str, args: &[Arg], want: Option<Type>, pos: Pos, sc: &Scope) -> O<(Ex, T)> {
        if let Some(conv) = self.conversion(name, args, pos, sc) {
            return conv;
        }
        if name == "EXPT" {
            if !self.positional(name, args, pos) {
                return None;
            }
            if args.len() != 2 {
                self.err(pos, "EXPT takes 2 arguments");
                return None;
            }
            return self.power(&args[0].value, &args[1].value, want, pos, sc);
        }
        if name == "MOVE" {
            if !self.positional(name, args, pos) || args.len() != 1 {
                self.err(pos, "MOVE takes 1 argument");
                return None;
            }
            return self.synth(&args[0].value, want, sc);
        }
        let Some(&(_, b)) = BUILTINS.iter().find(|(n, _)| *n == name) else {
            if !self.was_dropped(sc.pou, name) {
                self.err(pos, format!("unknown function {name}"));
            }
            return None;
        };
        if !self.positional(name, args, pos) {
            return None;
        }
        let arity = |n: usize, ok: bool, me: &mut Self| {
            if !ok {
                me.err(pos, format!("{name} takes {n} argument(s), got {}", args.len()));
            }
            ok
        };
        let want_ty = want.and_then(Self::rt);
        match b {
            Builtin::Now => {
                if !arity(0, args.is_empty(), self) {
                    return None;
                }
                Some((
                    Ex::Builtin(b, Vec::new(), Ty::Time),
                    T::Known(Type::Elem(Ty::Time)),
                ))
            }
            Builtin::Abs => {
                if !arity(1, args.len() == 1, self) {
                    return None;
                }
                let (x, ty) = self.unify(name, &args[..1], want_ty, pos, sc)?;
                if !ty.is_num() && ty != Ty::Time {
                    self.err(pos, format!("ABS needs a number, found {ty}"));
                    return None;
                }
                Some((Ex::Builtin(b, x, ty), T::Known(Type::Elem(ty))))
            }
            Builtin::Sqrt
            | Builtin::Ln
            | Builtin::Log
            | Builtin::Exp
            | Builtin::Sin
            | Builtin::Cos
            | Builtin::Tan
            | Builtin::Asin
            | Builtin::Acos
            | Builtin::Atan => {
                if !arity(1, args.len() == 1, self) {
                    return None;
                }
                let (x, t) = self.synth(&args[0].value, want, sc)?;
                let ty = match t {
                    T::Known(Type::Elem(ty)) if ty.is_real() => ty,
                    T::Known(k) => {
                        let n = self.type_name(k);
                        self.err(
                            pos,
                            format!("{name} needs REAL or LREAL, found {n}; convert with {n}_TO_LREAL"),
                        );
                        return None;
                    }
                    _ => want_ty.filter(|w| w.is_real()).unwrap_or(Ty::LReal),
                };
                let x = self.coerce(x, t, Type::Elem(ty), args[0].value.pos)?;
                Some((Ex::Builtin(b, vec![x], ty), T::Known(Type::Elem(ty))))
            }
            Builtin::Trunc => {
                if !arity(1, args.len() == 1, self) {
                    return None;
                }
                let (x, t) = self.synth(&args[0].value, None, sc)?;
                let x = match t {
                    T::Known(Type::Elem(ty)) if ty.is_real() => x,
                    T::RealLit(_) | T::IntLit(_) => self.lit_as(t, Ty::LReal, pos)?,
                    T::Known(k) => {
                        let n = self.type_name(k);
                        self.err(pos, format!("TRUNC needs REAL or LREAL, found {n}"));
                        return None;
                    }
                };
                Some((Ex::Builtin(b, vec![x], Ty::DInt), T::Known(Type::Elem(Ty::DInt))))
            }
            Builtin::Min | Builtin::Max | Builtin::Limit => {
                let ok = if b == Builtin::Limit {
                    args.len() == 3
                } else {
                    args.len() >= 2
                };
                if !arity(if b == Builtin::Limit { 3 } else { 2 }, ok, self) {
                    return None;
                }
                let (x, ty) = self.unify(name, args, want_ty, pos, sc)?;
                Some((Ex::Builtin(b, x, ty), T::Known(Type::Elem(ty))))
            }
            Builtin::Sel | Builtin::Mux => {
                let ok = if b == Builtin::Sel {
                    args.len() == 3
                } else {
                    args.len() >= 2
                };
                if !arity(if b == Builtin::Sel { 3 } else { 2 }, ok, self) {
                    return None;
                }
                let g = if b == Builtin::Sel {
                    self.coerced(&args[0].value, Type::Elem(Ty::Bool), sc)
                } else {
                    let (x, t) = self.synth(&args[0].value, None, sc)?;
                    match t {
                        T::Known(Type::Elem(ty)) if ty.is_int() => Some(x),
                        T::IntLit(v) => Some(Ex::Const(Value::Int(v))),
                        _ => {
                            self.err(args[0].pos, "MUX selector must be an integer");
                            None
                        }
                    }
                };
                let (mut x, ty) = self.unify(name, &args[1..], want_ty, pos, sc)?;
                x.insert(0, g?);
                Some((Ex::Builtin(b, x, ty), T::Known(Type::Elem(ty))))
            }
            Builtin::Shl | Builtin::Shr | Builtin::Rol | Builtin::Ror => {
                if !arity(2, args.len() == 2, self) {
                    return None;
                }
                let (x, t) = self.synth(&args[0].value, want, sc)?;
                let ty = match t {
                    T::Known(Type::Elem(ty)) if ty.is_bitstring() => ty,
                    T::IntLit(_) if want_ty.is_some_and(Ty::is_bitstring) => want_ty.unwrap_or(Ty::Byte),
                    _ => {
                        self.err(args[0].pos, format!("{name} needs a bit string (BYTE..LWORD)"));
                        return None;
                    }
                };
                let x = self.coerce(x, t, Type::Elem(ty), args[0].pos)?;
                let (n, nt) = self.synth(&args[1].value, None, sc)?;
                let n = match nt {
                    T::Known(Type::Elem(k)) if k.is_int() => n,
                    T::IntLit(v) if v >= 0 => Ex::Const(Value::Int(v)),
                    _ => {
                        self.err(
                            args[1].pos,
                            format!("{name} needs a non-negative integer shift count"),
                        );
                        return None;
                    }
                };
                Some((Ex::Builtin(b, vec![x, n], ty), T::Known(Type::Elem(ty))))
            }
        }
    }

    /// Brings every argument to one elementary type (as for a binary operator).
    fn unify(
        &mut self,
        name: &str,
        args: &[Arg],
        want: Option<Ty>,
        pos: Pos,
        sc: &Scope,
    ) -> O<(Vec<Ex>, Ty)> {
        let mut parts = Vec::new();
        for a in args {
            parts.push(self.synth(&a.value, want.map(Type::Elem), sc));
        }
        let parts: Vec<(Ex, T)> = parts.into_iter().collect::<Option<_>>()?;
        let mut acc = parts[0].1;
        for p in &parts[1..] {
            let ty = self.operand_ty(acc, p.1, want, name, pos)?;
            acc = T::Known(Type::Elem(ty));
        }
        let ty = match acc {
            T::Known(Type::Elem(t)) => t,
            T::Known(k) => {
                let n = self.type_name(k);
                self.err(pos, format!("{name} cannot take {n}"));
                return None;
            }
            lit => Self::default_lit(lit, lit, want),
        };
        let mut out = Vec::new();
        for ((x, t), a) in parts.into_iter().zip(args) {
            out.push(self.coerce(x, t, Type::Elem(ty), a.value.pos)?);
        }
        Some((out, ty))
    }

    /// `A_TO_B(x)` and `TO_B(x)`; `None` if `name` is not a conversion.
    fn conversion(&mut self, name: &str, args: &[Arg], pos: Pos, sc: &Scope) -> O<O<(Ex, T)>> {
        let (from, to) = match name.split_once("_TO_") {
            Some((f, t)) if !name.starts_with("TO_") => (Some(Ty::from_name(f)?), Ty::from_name(t)?),
            _ => (None, Ty::from_name(name.strip_prefix("TO_")?)?),
        };
        if !self.positional(name, args, pos) {
            return Some(None);
        }
        if args.len() != 1 {
            self.err(pos, format!("{name} takes 1 argument, got {}", args.len()));
            return Some(None);
        }
        let arg = &args[0].value;
        let r = if let Some(f) = from {
            self.coerced(arg, Type::Elem(f), sc).map(|x| (x, f))
        } else {
            self.synth(arg, None, sc).and_then(|(x, t)| {
                let f = match t {
                    T::Known(Type::Elem(f)) => f,
                    T::Known(Type::Enum(_)) => Ty::DInt,
                    T::Known(k) => {
                        let n = self.type_name(k);
                        self.err(arg.pos, format!("{name} cannot convert {n}"));
                        return None;
                    }
                    lit => Self::default_lit(lit, lit, None),
                };
                let x = if matches!(t, T::Known(Type::Enum(_))) {
                    x
                } else {
                    self.coerce(x, t, Type::Elem(f), arg.pos)?
                };
                Some((x, f))
            })
        };
        Some(r.map(|(x, f)| {
            (
                if f == to { x } else { Ex::Conv(Box::new(x), f, to) },
                T::Known(Type::Elem(to)),
            )
        }))
    }

    /// A move of `value` into a callee slot of type `ty` at `off`.
    fn arg_move(&mut self, off: usize, ty: Type, value: &Expr, sc: &Scope) -> O<Move> {
        if let Type::Agg(_) = ty {
            let (src, st, _) = self.place(value, sc)?;
            if st != ty {
                let (a, b) = (self.type_name(ty), self.type_name(st));
                self.err(value.pos, format!("expected {a}, found {b}"));
                return None;
            }
            return Some(Move::Copy {
                off,
                src,
                len: self.size_of(ty),
            });
        }
        Some(Move::Value {
            off,
            ex: self.coerced(value, ty, sc)?,
            ty: Self::rt(ty)?,
        })
    }

    /// Matches call arguments to the callee's inputs (and, for FBs, outputs).
    fn bind_args(&mut self, p: usize, args: &[Arg], sc: &Scope) -> O<(Vec<Move>, Vec<Out>)> {
        let callee = self.pous[p].name.clone();
        let inputs: Vec<(String, Type, usize)> = self.pous[p]
            .vars
            .iter()
            .filter(|v| v.section == Section::Input)
            .map(|v| (v.name.clone(), v.ty, v.off))
            .collect();
        let mut moves = Vec::new();
        let mut outs = Vec::new();
        let mut seen = HashSet::new();
        let mut ok = true;
        let mut named_seen = false;
        for (k, a) in args.iter().enumerate() {
            let Some(n) = &a.name else {
                if named_seen {
                    self.err(a.pos, "positional arguments must come before named ones");
                    ok = false;
                    continue;
                }
                let Some((iname, ty, off)) = inputs.get(k).cloned() else {
                    self.err(a.pos, format!("{callee} takes {} input(s)", inputs.len()));
                    ok = false;
                    continue;
                };
                seen.insert(iname);
                match self.arg_move(off, ty, &a.value, sc) {
                    Some(m) => moves.push(m),
                    None => ok = false,
                }
                continue;
            };
            named_seen = true;
            if !seen.insert(n.clone()) {
                self.err(a.pos, format!("{n} is given twice"));
                ok = false;
                continue;
            }
            let Some(v) = self.pous[p].vars.iter().find(|v| &v.name == n).cloned() else {
                self.err(a.pos, format!("{callee} has no input or output {n}"));
                ok = false;
                continue;
            };
            match (a.output, v.section) {
                (false, Section::Input) => match self.arg_move(v.off, v.ty, &a.value, sc) {
                    Some(m) => moves.push(m),
                    None => ok = false,
                },
                (true, Section::Output) if self.pous[p].kind == PouType::FunctionBlock => {
                    match self.out_binding(&v, &a.value, sc) {
                        Some(o) => outs.push(o),
                        None => ok = false,
                    }
                }
                (false, _) => {
                    self.err(
                        a.pos,
                        format!("{n} is not an input of {callee}; outputs are bound with =>"),
                    );
                    ok = false;
                }
                (true, _) => {
                    self.err(a.pos, format!("{n} is not an output of {callee}"));
                    ok = false;
                }
            }
        }
        ok.then_some((moves, outs))
    }

    fn out_binding(&mut self, v: &VarInfo, dst: &Expr, sc: &Scope) -> O<Out> {
        let (place, dt, acc) = self.place(dst, sc)?;
        if let Access::ReadOnly(why) = acc {
            self.err(dst.pos, format!("the target {why}"));
            return None;
        }
        let conv = match (v.ty, dt) {
            // Scalars always carry their type; structured values are copied.
            (a, b) if a == b => Self::rt(a).map(|t| (t, t)),
            (Type::Elem(a), Type::Elem(b)) if a.widens_to(b) => Some((a, b)),
            (a, b) => {
                let (x, y) = (self.type_name(a), self.type_name(b));
                self.err(dst.pos, format!("output {} is {x}, the target is {y}", v.name));
                return None;
            }
        };
        Some(Out {
            off: v.off,
            len: self.size_of(v.ty),
            dst: place,
            conv,
        })
    }

    fn fn_call(&mut self, p: usize, args: &[Arg], sc: &Scope) -> O<Call> {
        self.calls[sc.pou].insert(p);
        let (moves, outs) = self.bind_args(p, args, sc)?;
        Some(Call {
            pou: p,
            inst: None,
            moves,
            outs,
        })
    }

    // ---- statements ----

    fn block(&mut self, b: &[Stmt], sc: &mut Scope) -> Vec<St> {
        b.iter().filter_map(|s| self.stmt(s, sc)).collect()
    }

    fn cond(&mut self, e: &Expr, sc: &Scope) -> O<Ex> {
        self.coerced(e, Type::Elem(Ty::Bool), sc)
    }

    fn assign(&mut self, t: &Expr, v: &Expr, sc: &Scope) -> O<StKind> {
        let target = self.place(t, sc);
        let (place, ty, acc) = target?;
        if let Access::ReadOnly(why) = acc {
            let what = match &t.kind {
                ExprKind::Var(n) => n.clone(),
                ExprKind::Direct(d) => format!("%{d}"),
                _ => "the target".into(),
            };
            self.err(t.pos, format!("{what} {why}"));
            return None;
        }
        if let Type::Agg(_) = ty {
            let (src, st, _) = self.place(v, sc)?;
            if st != ty {
                let (a, b) = (self.type_name(ty), self.type_name(st));
                self.err(v.pos, format!("expected {a}, found {b}"));
                return None;
            }
            return Some(StKind::Copy(place, src, self.size_of(ty)));
        }
        let rt = Self::rt(ty)?;
        Some(StKind::Store(place, rt, self.coerced(v, ty, sc)?))
    }

    fn call_stmt(&mut self, e: &Expr, sc: &Scope) -> O<StKind> {
        let ExprKind::Call(callee, args) = &e.kind else {
            unreachable!("parser only builds call statements from calls")
        };
        if let ExprKind::Var(name) = &callee.kind
            && !sc.vars.contains_key(name)
        {
            if let Some(&p) = self.pou_index.get(name)
                && self.pous[p].kind == PouType::Function
            {
                return self.fn_call(p, args, sc).map(StKind::Call);
            }
            if self.pou_index.contains_key(name) {
                self.err(
                    e.pos,
                    format!("{name} is a function block type; declare an instance and call that"),
                );
                return None;
            }
            // A standard function as a statement: evaluate and discard.
            self.call_expr(callee, args, None, e.pos, sc)?;
            self.err(e.pos, format!("the result of {name} is unused"));
            return None;
        }
        let (inst, ty, _) = self.place(callee, sc)?;
        let fb = match ty {
            Type::Agg(a) => match self.aggs[a].kind {
                AggKind::Fb(fb) => Some(fb),
                _ => None,
            },
            _ => None,
        };
        let Some(fb) = fb else {
            let n = self.type_name(ty);
            self.err(e.pos, format!("{n} is not a function block instance"));
            return None;
        };
        self.calls[sc.pou].insert(fb);
        let (moves, outs) = self.bind_args(fb, args, sc)?;
        Some(StKind::Call(Call {
            pou: fb,
            inst: Some(inst),
            moves,
            outs,
        }))
    }

    fn stmt(&mut self, s: &Stmt, sc: &mut Scope) -> O<St> {
        let kind = match &s.kind {
            StmtKind::Empty => return None,
            StmtKind::Assign(t, v) => self.assign(t, v, sc)?,
            StmtKind::Call(c) => self.call_stmt(c, sc)?,
            StmtKind::If(arms, other) => {
                let mut out = Vec::new();
                let mut ok = true;
                for (c, b) in arms {
                    let c = self.cond(c, sc);
                    let b = self.block(b, sc);
                    match c {
                        Some(c) => out.push((c, b)),
                        None => ok = false,
                    }
                }
                let other = self.block(other, sc);
                if !ok {
                    return None;
                }
                StKind::If(out, other)
            }
            StmtKind::Case(sel, arms, other) => self.case(sel, arms, other, sc)?,
            StmtKind::For {
                var,
                from,
                to,
                by,
                body,
            } => {
                let Some(&k) = sc.vars.get(var) else {
                    if !self.was_dropped(sc.pou, var) {
                        self.err(s.pos, format!("undeclared variable {var}"));
                    }
                    return None;
                };
                let v = self.pous[sc.pou].vars[k].clone();
                let ty = match v.ty {
                    Type::Elem(t) if t.is_int() && !v.constant && v.loc.is_none() => t,
                    _ => {
                        let n = self.type_name(v.ty);
                        self.err(
                            s.pos,
                            format!(
                                "FOR variable {var} must be a non-CONSTANT, non-located integer, found {n}"
                            ),
                        );
                        return None;
                    }
                };
                let from = self.coerced(from, v.ty, sc);
                let to = self.coerced(to, v.ty, sc);
                let by = match by {
                    Some(b) => self.coerced(b, v.ty, sc),
                    None => Some(Ex::Const(Value::Int(1))),
                };
                sc.loops += 1;
                let body = self.block(body, sc);
                sc.loops -= 1;
                StKind::For {
                    var: Place::frame(v.off),
                    ty,
                    from: from?,
                    to: to?,
                    by: by?,
                    body,
                }
            }
            StmtKind::While(c, b) => {
                let c = self.cond(c, sc);
                sc.loops += 1;
                let b = self.block(b, sc);
                sc.loops -= 1;
                StKind::While(c?, b)
            }
            StmtKind::Repeat(b, c) => {
                sc.loops += 1;
                let b = self.block(b, sc);
                sc.loops -= 1;
                StKind::Repeat(b, self.cond(c, sc)?)
            }
            StmtKind::Exit | StmtKind::Continue if sc.loops == 0 => {
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

    fn label(&mut self, l: &LabelValue, sel: Type, pos: Pos, sc: &Scope) -> O<i128> {
        match (l, sel) {
            (LabelValue::Int(v), Type::Elem(_)) => Some(*v),
            (LabelValue::Name(n), Type::Enum(e)) => {
                let hit = self.enums[e].values.iter().find(|(v, _)| v == n).map(|(_, x)| *x);
                if hit.is_none() {
                    let en = self.enums[e].name.clone();
                    self.err(pos, format!("{en} has no value {n}"));
                }
                hit
            }
            (LabelValue::Name(n), Type::Elem(_)) => {
                // A CONSTANT integer may label a CASE arm.
                let k = sc.vars.get(n).copied();
                let c = k
                    .map(|k| self.pous[sc.pou].vars[k].clone())
                    .filter(|v| v.constant);
                if let Some(Value::Int(x)) = c.and_then(|v| self.pous[sc.pou].init.get(v.off).copied()) {
                    Some(x)
                } else {
                    self.err(pos, format!("CASE label {n} is not an integer CONSTANT"));
                    None
                }
            }
            (LabelValue::Int(_), _) => {
                self.err(pos, "an enumeration CASE takes value names as labels");
                None
            }
            (LabelValue::Name(_), _) => unreachable!("selector is elementary or enum"),
        }
    }

    fn case(&mut self, sel: &Expr, arms: &[parser::CaseArm], other: &[Stmt], sc: &mut Scope) -> O<StKind> {
        let (x, t) = self.synth(sel, None, sc)?;
        let ty = match t {
            T::Known(Type::Elem(ty)) if ty.is_int() => Type::Elem(ty),
            T::Known(Type::Enum(e)) => Type::Enum(e),
            T::Known(k) => {
                let n = self.type_name(k);
                self.err(
                    sel.pos,
                    format!("CASE selector must be an integer or enumeration, found {n}"),
                );
                return None;
            }
            lit => Type::Elem(Self::default_lit(lit, lit, None)),
        };
        let x = self.coerce(x, t, ty, sel.pos)?;
        let (lo, hi) = match ty {
            Type::Elem(e) => e.range(),
            _ => (i128::MIN, i128::MAX),
        };
        let mut seen: Vec<(i128, i128)> = Vec::new();
        let mut out = Vec::new();
        for (labels, body) in arms {
            let mut ranges = Vec::new();
            for CaseLabel { lo: l, hi: h, pos } in labels {
                let (Some(a), Some(b)) = (self.label(l, ty, *pos, sc), self.label(h, ty, *pos, sc)) else {
                    continue;
                };
                if b < a {
                    self.err(*pos, format!("empty CASE range {a}..{b}"));
                } else if a < lo || b > hi {
                    let n = self.type_name(ty);
                    self.err(*pos, format!("CASE label outside {n} ({lo}..{hi})"));
                } else if seen.iter().any(|&(x, y)| a <= y && x <= b) {
                    self.err(*pos, "CASE label overlaps an earlier one");
                } else {
                    seen.push((a, b));
                    ranges.push((a, b));
                }
            }
            out.push((ranges, self.block(body, sc)));
        }
        Some(StKind::Case(x, out, self.block(other, sc)))
    }

    fn check_body(&mut self, p: usize) {
        let src = self.srcs[p].pou;
        let vars = self.pous[p]
            .vars
            .iter()
            .enumerate()
            .map(|(k, v)| (v.name.clone(), k))
            .collect();
        let mut sc = Scope {
            pou: p,
            vars,
            loops: 0,
        };
        let body = self.block(&src.body, &mut sc);
        self.pous[p].body = body;
    }

    /// Reports a cycle among function and FB calls.
    fn check_recursion(&mut self) {
        let n = self.pous.len();
        let mut state = vec![0u8; n];
        let mut stack = Vec::new();
        for start in 0..n {
            if state[start] == 0 {
                self.dfs(start, &mut state, &mut stack);
            }
        }
    }

    fn dfs(&mut self, p: usize, state: &mut [u8], stack: &mut Vec<usize>) {
        state[p] = 1;
        stack.push(p);
        let mut next: Vec<usize> = self.calls[p].iter().copied().collect();
        next.sort_unstable();
        for q in next {
            if state[q] == 1 {
                let k = stack.iter().position(|&s| s == q).unwrap_or(0);
                let path: Vec<String> = stack[k..]
                    .iter()
                    .chain([&q])
                    .map(|&s| self.pous[s].name.clone())
                    .collect();
                let pos = self.srcs[q].pou.pos;
                self.err(pos, format!("recursion is not allowed: {}", path.join(" -> ")));
            } else if state[q] == 0 {
                self.dfs(q, state, stack);
            }
        }
        stack.pop();
        state[p] = 2;
    }
}

/// Type-checks the built-in library and a user unit together. The user
/// unit must hold exactly one PROGRAM.
#[allow(clippy::too_many_lines)]
pub fn check(
    library: &parser::Unit,
    user: &parser::Unit,
    sizes: ImageSizes,
) -> Result<Checked, Vec<Diagnostic>> {
    let mut ck = Ck {
        sizes,
        srcs: Vec::new(),
        pous: Vec::new(),
        pou_index: HashMap::new(),
        aggs: Vec::new(),
        struct_src: HashMap::new(),
        enums: Vec::new(),
        named: HashMap::new(),
        aliases: Vec::new(),
        agg_state: Vec::new(),
        pou_state: Vec::new(),
        calls: Vec::new(),
        dropped: HashMap::new(),
        loc_init: Vec::new(),
        diags: Vec::new(),
    };
    ck.register_types(&library.types);
    ck.register_types(&user.types);
    for (unit, system) in [(library, true), (user, false)] {
        for pou in &unit.pous {
            let kind = match pou.kind {
                PouKind::Program => PouType::Program,
                PouKind::Function(_) => PouType::Function,
                PouKind::FunctionBlock => PouType::FunctionBlock,
            };
            if Ty::from_name(&pou.name).is_some()
                || ck.named.contains_key(&pou.name)
                || ck.pou_index.contains_key(&pou.name)
                || BUILTINS.iter().any(|(n, _)| *n == pou.name)
            {
                let std = if ck.pou_index.get(&pou.name).is_some_and(|&p| ck.srcs[p].system) {
                    " (a standard block)"
                } else {
                    ""
                };
                ck.err(pou.pos, format!("{} is already defined{std}", pou.name));
                continue;
            }
            let id = ck.pous.len();
            ck.pou_index.insert(pou.name.clone(), id);
            ck.srcs.push(PouSrc { pou, system });
            ck.pous.push(PouCode {
                name: pou.name.clone(),
                kind,
                vars: Vec::new(),
                size: 0,
                init: Vec::new(),
                temps: Vec::new(),
                body: Vec::new(),
                base: 0,
                result: None,
            });
            ck.pou_state.push(0);
            ck.calls.push(HashSet::new());
            if kind == PouType::FunctionBlock {
                let agg = ck.aggs.len();
                ck.aggs.push(AggInfo {
                    name: pou.name.clone(),
                    kind: AggKind::Fb(id),
                    size: 0,
                    init: Vec::new(),
                });
                ck.agg_state.push(0);
                ck.named.insert(pou.name.clone(), Named::Type(Type::Agg(agg)));
            }
        }
    }
    let programs: Vec<usize> = (0..ck.pous.len())
        .filter(|&p| ck.pous[p].kind == PouType::Program && !ck.srcs[p].system)
        .collect();
    let program = match programs.as_slice() {
        [one] => Some(*one),
        [] => {
            ck.err(Pos { line: 1, col: 1 }, "no PROGRAM found");
            None
        }
        [_, second, ..] => {
            let pos = ck.srcs[*second].pou.pos;
            ck.err(pos, "only one PROGRAM is supported");
            None
        }
    };
    // Layout everything, including types nothing uses (to report their errors).
    for a in 0..ck.aggs.len() {
        ck.layout_agg(a, Pos::default());
    }
    for p in 0..ck.pous.len() {
        ck.layout_pou(p);
    }
    // Frames: the program at 0, then each function.
    let mut mem_init = Vec::new();
    if let Some(p) = program {
        mem_init.extend(ck.pous[p].init.iter().copied());
    }
    for p in 0..ck.pous.len() {
        if ck.pous[p].kind == PouType::Function {
            ck.pous[p].base = mem_init.len();
            mem_init.extend(ck.pous[p].init.iter().copied());
        }
    }
    for p in 0..ck.pous.len() {
        ck.check_body(p);
    }
    ck.check_recursion();
    match program {
        Some(program) if ck.diags.is_empty() => Ok(Checked {
            pous: ck.pous,
            aggs: ck.aggs,
            program,
            mem_init,
            loc_init: ck.loc_init,
        }),
        _ => {
            ck.diags.sort_by_key(|d| d.pos);
            ck.diags.dedup();
            Err(ck.diags)
        }
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
