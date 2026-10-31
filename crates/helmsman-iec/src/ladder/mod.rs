//! Rockwell ladder (RLL) in an L5X export, converted to IEC 61131-3
//! Structured Text, for a defined subset; everything else is refused with
//! where it is. Either the whole export converts or nothing does.
//!
//! **What converts** (semantics from Rockwell's *Logix 5000 Controllers
//! General Instructions*, 1756-RM003Z-EN-P, cited by page in
//! docs/SOURCES.md and in `lib.st`):
//! - contacts and coils XIC, XIO, OTE, OTL, OTU; branches; ONS, OSR, OSF;
//! - TON, TOF, RTO on TIMER tags; CTU, CTD on COUNTER tags; RES. A TIMER or
//!   COUNTER stays a structure the rungs read and write (`T.DN`,
//!   `MOV(5000, T.PRE)`); the instructions are stateless blocks in `lib.st`
//!   that copy it in and out, following the manual's flow charts,
//!   overflow, wrap and fault cases included. Time is the scan's clock in
//!   whole milliseconds;
//! - EQU/EQ, NEQ/NE, LES/LT, LEQ/LE, GRT/GT, GEQ/GE, LIM/LIMIT (the old
//!   and the version 36 mnemonics, RM003 p.320ff.);
//! - ADD, SUB, MUL, DIV, MOD, CPT (`+ - * / MOD`, negation, parentheses),
//!   MOV/MOVE, CLR, COP between elements of one type, with Logix's data
//!   conversions: SINT and INT widen to DINT, REAL wins over integers, a
//!   REAL stored in an integer rounds to the nearest (ties to even) and
//!   keeps the low bits, a narrowing store keeps the low bits; integer DIV
//!   by zero gives Source A, MOD by zero 0;
//! - JSR to another ladder routine of the same program, without
//!   parameters, as the last instruction of its rung: the routine is
//!   inlined there under the rung condition;
//! - tags of controller and program scope of type BOOL, SINT, INT, DINT,
//!   REAL, TIMER, COUNTER and arrays of them (constant subscripts), bits of
//!   integers (`Word.5`), aliases of such tags; their decorated initial
//!   values (RM014 pp.43-46) and the pseudo-operands of timers and
//!   counters (RM003 p.856).
//!
//! **The shape**: one file, one `PROGRAM` per Logix task, named as the
//! task, running the task's programs in order, each from its main
//! routine. A program's `Input` and `Output` parameters of an elementary
//! type become `VAR_INPUT` and `VAR_OUTPUT` (the box's ports); every other
//! tag a `VAR`. Before the first scan the program runs Logix's prescan
//! (each instruction's Prescan row in RM003): OTE clears, one-shots arm,
//! timers reset. An export without tasks (one program's) is one PROGRAM
//! named as the controller.
//!
//! **Refused**, each with its program, routine and rung: any other
//! instruction or add-on instruction, FBD, SFC and ST routines (Logix
//! Structured Text calls instructions IEC ST does not have, so it is not
//! passed through), tags of other types (user-defined types, LINT,
//! STRING, CONTROL, module-defined I/O tags) and aliases to them,
//! indirect subscripts (`A[i]`), produced/consumed and `InOut` tags, JSR
//! with parameters, recursion, programs in no task or disabled, event
//! tasks, a controller tag shared by two tasks (the box's programs share
//! no memory), and names that are Structured Text keywords.
//!
//! **Not reproduced**: math status flags (S:V, S:Z, S:N, S:C) and minor
//! faults, which do not change a tag; the system's own `S:FS`; the
//! controller's microsecond clock (a timer reads the scan's time once).

pub mod rung;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::l5x::{self, Data, Project, RoutineKind, TagKind};
use rung::{Elem, Expr, Index, Instr, Op, Operand, Path, Seg};

/// Problems listed in a refusal; the rest are counted.
pub const MAX_PROBLEMS: usize = 200;
/// Largest Structured Text built while inlining (the project then holds
/// a file to its own, smaller limit).
pub const MAX_ST: usize = 4 << 20;
/// Deepest JSR nesting (RM003 p.627: "up to 25 nested subroutines").
pub const MAX_JSR_DEPTH: usize = 25;
/// Most elements one COP copies.
pub const MAX_COP: i64 = 4096;

/// A converted export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    /// The Structured Text: helper types and blocks, then the PROGRAMs.
    pub st: String,
    /// The PROGRAMs, in order (one per task).
    pub programs: Vec<String>,
    /// What the engineer should know (also at the top of `st`).
    pub notes: Vec<String>,
}

/// One reason the export does not convert, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// `program P, routine R, rung N`, or what else it is about; may be empty.
    pub at: String,
    pub msg: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.at.is_empty() {
            f.write_str(&self.msg)
        } else {
            write!(f, "{}: {}", self.at, self.msg)
        }
    }
}

/// Why the export was not converted: every problem found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The first [`MAX_PROBLEMS`].
    pub problems: Vec<Problem>,
    pub total: usize,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (k, p) in self.problems.iter().enumerate() {
            if k > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{p}")?;
        }
        if self.total > self.problems.len() {
            write!(f, "; and {} more", self.total - self.problems.len())?;
        }
        Ok(())
    }
}

/// The Logix numeric types converted, in rank order (RM003 p.851).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Num {
    SInt,
    Int,
    DInt,
    Real,
}

impl Num {
    const fn st(self) -> &'static str {
        match self {
            Self::SInt => "SINT",
            Self::Int => "INT",
            Self::DInt => "DINT",
            Self::Real => "REAL",
        }
    }

    const fn bits(self) -> u32 {
        match self {
            Self::SInt => 8,
            Self::Int => 16,
            Self::DInt | Self::Real => 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Bool,
    Num(Num),
    Timer,
    Counter,
}

impl Kind {
    fn from_logix(t: &str) -> Option<Self> {
        Some(match t.to_ascii_uppercase().as_str() {
            "BOOL" => Self::Bool,
            "SINT" => Self::Num(Num::SInt),
            "INT" => Self::Num(Num::Int),
            "DINT" => Self::Num(Num::DInt),
            "REAL" => Self::Num(Num::Real),
            "TIMER" => Self::Timer,
            "COUNTER" => Self::Counter,
            _ => return None,
        })
    }

    const fn st(self) -> &'static str {
        match self {
            Self::Bool => "BOOL",
            Self::Num(n) => n.st(),
            Self::Timer => "LOGIX_TIMER",
            Self::Counter => "LOGIX_COUNTER",
        }
    }

    const fn logix(self) -> &'static str {
        match self {
            Self::Bool => "BOOL",
            Self::Num(Num::SInt) => "SINT",
            Self::Num(Num::Int) => "INT",
            Self::Num(Num::DInt) => "DINT",
            Self::Num(Num::Real) => "REAL",
            Self::Timer => "TIMER",
            Self::Counter => "COUNTER",
        }
    }
}

const TIMER_MEMBERS: &[(&str, Kind)] = &[
    ("PRE", Kind::Num(Num::DInt)),
    ("ACC", Kind::Num(Num::DInt)),
    ("EN", Kind::Bool),
    ("TT", Kind::Bool),
    ("DN", Kind::Bool),
];
const COUNTER_MEMBERS: &[(&str, Kind)] = &[
    ("PRE", Kind::Num(Num::DInt)),
    ("ACC", Kind::Num(Num::DInt)),
    ("CU", Kind::Bool),
    ("CD", Kind::Bool),
    ("DN", Kind::Bool),
    ("OV", Kind::Bool),
    ("UN", Kind::Bool),
];

const fn members(k: Kind) -> &'static [(&'static str, Kind)] {
    match k {
        Kind::Timer => TIMER_MEMBERS,
        Kind::Counter => COUNTER_MEMBERS,
        _ => &[],
    }
}

type TagId = usize;

#[derive(Debug, Clone)]
struct TagDef {
    name: String,
    /// `None`: controller scope.
    program: Option<usize>,
    /// Why the tag cannot be used, if it cannot.
    bad: Option<String>,
    kind: Kind,
    dims: Vec<u32>,
    constant: bool,
    alias: Option<String>,
    usage: Option<String>,
    data: Option<l5x::TagData>,
}

/// The sentinel a tag's Structured Text name stands behind until names
/// are assigned.
fn token(id: TagId) -> String {
    format!("\u{1}{id}\u{2}")
}

/// A typed place a rung reads or writes.
#[derive(Debug, Clone)]
struct Place {
    st: String,
    kind: Kind,
    tag: TagId,
    /// A whole array (no subscript).
    whole: bool,
    /// For COP: element offset and elements left in a one-dimensional
    /// array (or 0 and 1 for a scalar).
    run: Option<(usize, usize)>,
    /// The subscripted element's key, for pseudo-operand initial values.
    elem: Option<usize>,
}

/// A piece of generated code: text, or a JSR to inline.
#[derive(Debug, Clone)]
enum Piece {
    Line(String),
    Jsr(usize),
}

#[derive(Debug, Clone, Default)]
struct RungCode {
    code: Vec<Piece>,
    pre: Vec<Piece>,
    depth: usize,
}

#[derive(Debug, Clone, Default)]
struct RoutineCode {
    rungs: Vec<(u32, String, RungCode)>,
}

/// Helpers in `lib.st`, by marker.
const LIB: &str = include_str!("lib.st");

fn lib_part(name: &str) -> &'static str {
    let marker = format!("(* @{name} *)\n");
    let start = LIB.find(&marker).map_or(0, |i| i + marker.len());
    let end = LIB[start..].find("(* @").map_or(LIB.len(), |i| start + i);
    &LIB[start..end]
}

fn lib_head() -> &'static str {
    let end = LIB.find("(* @").unwrap_or(0);
    &LIB[..end]
}

/// Names a tag cannot have in the converted file.
fn reserved(upper: &str) -> bool {
    upper.starts_with("LOGIX_")
        || upper.contains("_TO_")
        || crate::parser::is_reserved(upper)
        || crate::check::is_builtin(upper)
        || crate::types::Ty::from_name(upper).is_some()
        || matches!(
            upper,
            "TRUE" | "FALSE" | "STRING" | "WSTRING" | "DATE" | "TOD" | "DT"
        )
        || crate::library()
            .pous
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(upper))
}

struct Cx {
    p: Project,
    tags: Vec<TagDef>,
    /// Upper-cased name -> tag, controller scope.
    ctl: HashMap<String, TagId>,
    /// Per program: upper-cased name -> tag.
    prog: Vec<HashMap<String, TagId>>,
    problems: Vec<Problem>,
    total: usize,
    notes: Vec<String>,
    /// Tags each program's rungs reference.
    used: Vec<BTreeSet<TagId>>,
    /// Pseudo-operand initial values: (tag, element) -> (PRE, ACC, where).
    pseudo: BTreeMap<(TagId, usize), (Option<i32>, Option<i32>, String)>,
    /// Helper parts the code needs.
    needs: BTreeSet<&'static str>,
    /// Tags written by some rung.
    written: BTreeSet<TagId>,
}

fn at(prog: &str, routine: &str, rung: Option<u32>) -> String {
    match rung {
        Some(n) => format!("program {prog}, routine {routine}, rung {n}"),
        None => format!("program {prog}, routine {routine}"),
    }
}

/// Integer immediate as Structured Text.
fn int_lit(n: i64) -> String {
    if n == i64::from(i32::MIN) {
        "(-2147483647 - 1)".into()
    } else if n < 0 {
        format!("({n})")
    } else {
        n.to_string()
    }
}

/// REAL immediate as Structured Text (exact for an f32).
fn real_lit(x: f32) -> String {
    let mut s = format!("{:?}", f64::from(x));
    if let Some(e) = s.find('e')
        && !s[..e].contains('.')
    {
        s.insert_str(e, ".0");
    }
    if !s.contains('.') && !s.contains('e') {
        s.push_str(".0");
    }
    if x.is_sign_negative() { format!("({s})") } else { s }
}

/// `text` of type `from` as type `to`, by Logix's conversions.
fn conv(text: &str, from: Num, to: Num) -> String {
    if from == to {
        return text.to_string();
    }
    match (from, to) {
        (Num::Real, t) => format!("LINT_TO_{}(LOGIX_R2L({text}))", t.st()),
        (f, t) => format!("{}_TO_{}({text})", f.st(), t.st()),
    }
}

/// A numeric source: its text and type.
struct Src {
    st: String,
    num: Num,
    lit: Option<Operand>,
}

impl Src {
    /// As type `to`; immediates are written in that type directly.
    fn as_num(&self, to: Num) -> String {
        match (&self.lit, to) {
            (Some(Operand::Int(n)), Num::Real) => {
                #[allow(clippy::cast_precision_loss)]
                let r = *n as f32;
                real_lit(r)
            }
            (Some(Operand::Int(n)), Num::DInt) => int_lit(i64::from(*n)),
            _ => conv(&self.st, self.num, to),
        }
    }
}

impl Cx {
    fn problem(&mut self, at: String, msg: String) {
        self.total += 1;
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(Problem { at, msg });
        }
    }

    fn lookup(&self, scope: Option<usize>, upper: &str) -> Option<TagId> {
        scope
            .and_then(|p| self.prog[p].get(upper))
            .or_else(|| self.ctl.get(upper))
            .copied()
    }

    /// Resolves a path, read by program `user`, in `scope` (a program,
    /// then the controller; or the controller alone), following aliases
    /// in the alias's own scope.
    fn place(
        &mut self,
        user: usize,
        scope: Option<usize>,
        path: &Path,
        hops: usize,
    ) -> Result<Place, String> {
        let up = path.base.to_ascii_uppercase();
        let Some(id) = self.lookup(scope, &up) else {
            return Err(format!(
                "{} is not a tag of this program or the controller",
                path.base
            ));
        };
        let t = self.tags[id].clone();
        if let Some(target) = &t.alias {
            if hops >= 4 {
                return Err(format!("alias {} goes through more than 4 aliases", t.name));
            }
            if let Some(b) = &t.bad {
                return Err(b.clone());
            }
            let mut joined = match rung::operand(target) {
                Ok(Operand::Path(p)) => p,
                Ok(_) => return Err(format!("alias {} points at {target}, not a tag", t.name)),
                Err(e) => return Err(format!("alias {} points at {target}: {e}", t.name)),
            };
            joined.segs.extend(path.segs.iter().cloned());
            return self.place(user, t.program, &joined, hops + 1);
        }
        self.place_at(user, id, path)
    }

    fn place_at(&mut self, user: usize, id: TagId, path: &Path) -> Result<Place, String> {
        let t = self.tags[id].clone();
        if let Some(b) = &t.bad {
            return Err(b.clone());
        }
        self.used[user].insert(id);
        let mut st = token(id);
        let mut kind = t.kind;
        let mut segs = path.segs.iter().peekable();
        let mut whole = false;
        let mut run = None;
        let mut elem = None;
        if t.dims.is_empty() {
            run = Some((0, 1));
            elem = Some(0);
        } else {
            match segs.peek() {
                Some(Seg::Index(ix)) => {
                    segs.next();
                    if ix.len() != t.dims.len() {
                        return Err(format!(
                            "{} has {} dimension(s); {path} gives {}",
                            t.name,
                            t.dims.len(),
                            ix.len()
                        ));
                    }
                    let mut nums = Vec::new();
                    for (i, d) in ix.iter().zip(&t.dims) {
                        match i {
                            Index::Const(n) if *n >= 0 && *n < i64::from(*d) => {
                                nums.push(usize::try_from(*n).unwrap_or(0));
                            }
                            Index::Const(n) => {
                                return Err(format!("subscript {n} is outside {}[{d}]", t.name));
                            }
                            Index::Tag(x) => {
                                return Err(format!(
                                    "{path}: a tag as the subscript ({x}, indirect addressing) is not converted"
                                ));
                            }
                        }
                    }
                    let mut lin = 0usize;
                    for (n, d) in nums.iter().zip(&t.dims) {
                        lin = lin * (*d as usize) + n;
                    }
                    elem = Some(lin);
                    if t.dims.len() == 1 {
                        run = Some((nums[0], t.dims[0] as usize - nums[0]));
                    }
                    let list: Vec<String> = nums.iter().map(ToString::to_string).collect();
                    let _ = write!(st, "[{}]", list.join(","));
                }
                _ => whole = true,
            }
        }
        for s in segs {
            if whole {
                return Err(format!("{path}: {} is an array; give a subscript", t.name));
            }
            match (s, kind) {
                (Seg::Member(m), Kind::Timer | Kind::Counter) => {
                    let mu = m.to_ascii_uppercase();
                    let Some((n, k)) = members(kind).iter().find(|(n, _)| *n == mu) else {
                        return Err(format!("{path}: a {} has no member {m}", kind.logix()));
                    };
                    let _ = write!(st, ".{n}");
                    kind = *k;
                    run = None;
                    elem = None;
                }
                (Seg::Bit(b), Kind::Num(n)) if n != Num::Real => {
                    if *b >= n.bits() {
                        return Err(format!("{path}: bit {b} of a {}", n.st()));
                    }
                    let _ = write!(st, ".{b}");
                    kind = Kind::Bool;
                    run = None;
                    elem = None;
                }
                _ => return Err(format!("{path} does not name a part of a {}", kind.logix())),
            }
        }
        Ok(Place {
            st,
            kind,
            tag: id,
            whole,
            run,
            elem,
        })
    }

    fn whole_check(p: &Place, path: &str) -> Result<(), String> {
        if p.whole {
            return Err(format!("{path} is a whole array; give a subscript"));
        }
        Ok(())
    }

    fn bool_place(&mut self, prog: usize, arg: &str, write: bool) -> Result<Place, String> {
        let p = self.path_place(prog, arg)?;
        if p.kind != Kind::Bool {
            return Err(format!("{arg} is a {}, not a BOOL", p.kind.logix()));
        }
        if write {
            self.writable(&p, arg)?;
        }
        Ok(p)
    }

    fn path_place(&mut self, prog: usize, arg: &str) -> Result<Place, String> {
        match rung::operand(arg)? {
            Operand::Path(path) => {
                let p = self.place(prog, Some(prog), &path, 0)?;
                Self::whole_check(&p, arg)?;
                Ok(p)
            }
            Operand::Unknown => Err("'?' where a tag is needed".into()),
            _ => Err(format!("{arg} is an immediate where a tag is needed")),
        }
    }

    fn writable(&mut self, p: &Place, arg: &str) -> Result<(), String> {
        if self.tags[p.tag].constant {
            return Err(format!("{arg} is a constant tag; it cannot be written"));
        }
        self.written.insert(p.tag);
        Ok(())
    }

    fn src(&mut self, prog: usize, arg: &str) -> Result<Src, String> {
        match rung::operand(arg)? {
            Operand::Int(n) => Ok(Src {
                st: int_lit(i64::from(n)),
                num: Num::DInt,
                lit: Some(Operand::Int(n)),
            }),
            Operand::Real(r) => Ok(Src {
                st: real_lit(r),
                num: Num::Real,
                lit: Some(Operand::Real(r)),
            }),
            Operand::Unknown => Err("'?' where a value is needed".into()),
            Operand::Path(path) => {
                let p = self.place(prog, Some(prog), &path, 0)?;
                Self::whole_check(&p, arg)?;
                match p.kind {
                    Kind::Num(n) => Ok(Src {
                        st: p.st,
                        num: n,
                        lit: None,
                    }),
                    k => Err(format!("{arg} is a {}, not a number", k.logix())),
                }
            }
        }
    }

    fn num_dest(&mut self, prog: usize, arg: &str) -> Result<(String, Num), String> {
        let p = self.path_place(prog, arg)?;
        let Kind::Num(n) = p.kind else {
            return Err(format!("{arg} is a {}, not a number", p.kind.logix()));
        };
        self.writable(&p, arg)?;
        Ok((p.st, n))
    }

    fn pseudo(&mut self, p: &Place, pre: &str, acc: &str, where_: &str) -> Result<(), String> {
        let val = |s: &str, what: &str| -> Result<Option<i32>, String> {
            match rung::operand(s) {
                Ok(Operand::Unknown) => Ok(None),
                Ok(Operand::Int(n)) => Ok(Some(n)),
                _ => Err(format!("the {what} {s} is not an integer immediate or '?'")),
            }
        };
        let (pre, acc) = (val(pre, "preset")?, val(acc, "accum")?);
        if pre.is_none() && acc.is_none() {
            return Ok(());
        }
        let Some(e) = p.elem else {
            return Ok(());
        };
        let key = (p.tag, e);
        match self.pseudo.get(&key) {
            Some((p0, a0, w0))
                if (pre.is_some() && p0.is_some() && pre != *p0)
                    || (acc.is_some() && a0.is_some() && acc != *a0) =>
            {
                return Err(format!(
                    "its preset/accum ({}, {}) differ from those at {w0}; pseudo-operands set one tag",
                    pre.map_or("?".into(), |v| v.to_string()),
                    acc.map_or("?".into(), |v| v.to_string())
                ));
            }
            Some((p0, a0, w0)) => {
                let v = (pre.or(*p0), acc.or(*a0), w0.clone());
                self.pseudo.insert(key, v);
            }
            None => {
                self.pseudo.insert(key, (pre, acc, where_.to_string()));
            }
        }
        Ok(())
    }

    /// Types and emits one rung.
    #[allow(clippy::too_many_lines)]
    fn rung(
        &mut self,
        prog: usize,
        routines: &[String],
        elems: &[Elem],
        where_: &str,
    ) -> Result<RungCode, Vec<String>> {
        let mut rc = RungCode::default();
        let mut errs = Vec::new();
        rc.code.push(Piece::Line("LOGIX_RC0 := TRUE;".into()));
        let n = elems.len();
        for (k, e) in elems.iter().enumerate() {
            self.elem(prog, routines, e, 0, k + 1 == n, &mut rc, &mut errs, where_);
        }
        if errs.is_empty() { Ok(rc) } else { Err(errs) }
    }

    #[allow(clippy::too_many_arguments)]
    fn elem(
        &mut self,
        prog: usize,
        routines: &[String],
        e: &Elem,
        depth: usize,
        last: bool,
        rc: &mut RungCode,
        errs: &mut Vec<String>,
        where_: &str,
    ) {
        match e {
            Elem::Instr(i) => {
                if let Err(m) = self.instr(prog, routines, i, depth, last, rc, where_) {
                    let shown = format!("{}({})", i.name, i.args.join(","));
                    errs.push(format!("{shown}: {m}"));
                }
            }
            Elem::Branch(legs) => {
                let d = depth + 1;
                rc.depth = rc.depth.max(d);
                let line = |s: String| Piece::Line(s);
                rc.code.push(line(format!(
                    "LOGIX_BI{d} := LOGIX_RC{depth}; LOGIX_BO{d} := FALSE;"
                )));
                for leg in legs {
                    rc.code.push(line(format!("LOGIX_RC{d} := LOGIX_BI{d};")));
                    for e in leg {
                        self.elem(prog, routines, e, d, false, rc, errs, where_);
                    }
                    rc.code
                        .push(line(format!("LOGIX_BO{d} := LOGIX_BO{d} OR LOGIX_RC{d};")));
                }
                rc.code.push(line(format!("LOGIX_RC{depth} := LOGIX_BO{d};")));
            }
        }
    }

    fn arity(i: &Instr, n: usize) -> Result<(), String> {
        if i.args.len() == n {
            Ok(())
        } else {
            Err(format!("takes {n} operand(s), found {}", i.args.len()))
        }
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn instr(
        &mut self,
        prog: usize,
        routines: &[String],
        i: &Instr,
        depth: usize,
        last: bool,
        rc: &mut RungCode,
        where_: &str,
    ) -> Result<(), String> {
        let r = format!("LOGIX_RC{depth}");
        let mut lines: Vec<String> = Vec::new();
        let mut code = |s: String| lines.push(s);
        let a = &i.args;
        match i.name.as_str() {
            "XIC" | "XIO" => {
                Self::arity(i, 1)?;
                let p = self.bool_place(prog, &a[0], false)?;
                let not = if i.name == "XIO" { "NOT " } else { "" };
                code(format!("{r} := {r} AND {not}{};", p.st));
            }
            "OTE" => {
                Self::arity(i, 1)?;
                let p = self.bool_place(prog, &a[0], true)?;
                code(format!("{} := {r};", p.st));
                rc.pre.push(Piece::Line(format!("{} := FALSE;", p.st)));
            }
            "OTL" | "OTU" => {
                Self::arity(i, 1)?;
                let p = self.bool_place(prog, &a[0], true)?;
                let v = if i.name == "OTL" { "TRUE" } else { "FALSE" };
                code(format!("IF {r} THEN {} := {v}; END_IF;", p.st));
            }
            "ONS" => {
                Self::arity(i, 1)?;
                let s = self.bool_place(prog, &a[0], true)?.st;
                code(format!(
                    "IF {r} THEN IF {s} THEN {r} := FALSE; ELSE {s} := TRUE; END_IF; ELSE {s} := FALSE; END_IF;"
                ));
                rc.pre.push(Piece::Line(format!("{s} := TRUE;")));
            }
            "OSR" | "OSF" => {
                Self::arity(i, 2)?;
                let s = self.bool_place(prog, &a[0], true)?.st;
                let o = self.bool_place(prog, &a[1], true)?.st;
                if i.name == "OSR" {
                    code(format!(
                        "IF {r} THEN IF NOT {s} THEN {s} := TRUE; {o} := TRUE; ELSE {o} := FALSE; END_IF; ELSE {s} := FALSE; {o} := FALSE; END_IF;"
                    ));
                    rc.pre.push(Piece::Line(format!("{s} := TRUE; {o} := FALSE;")));
                } else {
                    code(format!(
                        "IF {r} THEN {s} := TRUE; {o} := FALSE; ELSIF {s} THEN {s} := FALSE; {o} := TRUE; ELSE {o} := FALSE; END_IF;"
                    ));
                    rc.pre.push(Piece::Line(format!("{s} := FALSE; {o} := FALSE;")));
                }
            }
            "TON" | "TOF" | "RTO" | "CTU" | "CTD" => {
                Self::arity(i, 3)?;
                let timer = matches!(i.name.as_str(), "TON" | "TOF" | "RTO");
                let p = self.path_place(prog, &a[0])?;
                let want = if timer { Kind::Timer } else { Kind::Counter };
                if p.kind != want {
                    return Err(format!(
                        "{} is a {}, not a {}",
                        a[0],
                        p.kind.logix(),
                        want.logix()
                    ));
                }
                self.writable(&p, &a[0])?;
                self.pseudo(&p, &a[1], &a[2], where_)?;
                let h = format!("LOGIX_H_{}", i.name);
                self.needs.insert(match i.name.as_str() {
                    "TON" => "TON",
                    "TOF" => "TOF",
                    "RTO" => "RTO",
                    "CTU" => "CTU",
                    _ => "CTD",
                });
                let t = &p.st;
                if timer {
                    self.needs.insert("CLOCK");
                    code(format!("{h}(T := {t}, RUNG := {r}, NOW := LOGIX_NOW);"));
                } else {
                    code(format!("{h}(C := {t}, RUNG := {r});"));
                }
                code(format!("{t} := {h}.Q;"));
                rc.pre.push(Piece::Line(match i.name.as_str() {
                    "TON" => format!("{t}.EN := FALSE; {t}.TT := FALSE; {t}.DN := FALSE; {t}.ACC := 0;"),
                    "TOF" => {
                        format!("{t}.EN := FALSE; {t}.TT := FALSE; {t}.DN := FALSE; {t}.ACC := {t}.PRE;")
                    }
                    "RTO" => format!("{t}.EN := FALSE; {t}.TT := FALSE;"),
                    "CTU" => format!("{t}.CU := TRUE;"),
                    _ => format!("{t}.CD := TRUE;"),
                }));
            }
            "RES" => {
                Self::arity(i, 1)?;
                let p = self.path_place(prog, &a[0])?;
                self.writable(&p, &a[0])?;
                let t = &p.st;
                let clear = match p.kind {
                    Kind::Timer => {
                        format!("{t}.ACC := 0; {t}.EN := FALSE; {t}.TT := FALSE; {t}.DN := FALSE;")
                    }
                    Kind::Counter => format!(
                        "{t}.ACC := 0; {t}.CU := FALSE; {t}.CD := FALSE; {t}.DN := FALSE; {t}.OV := FALSE; {t}.UN := FALSE;"
                    ),
                    k => {
                        return Err(format!(
                            "{} is a {}; RES takes a TIMER or COUNTER here",
                            a[0],
                            k.logix()
                        ));
                    }
                };
                code(format!("IF {r} THEN {clear} END_IF;"));
            }
            "EQU" | "EQ" | "NEQ" | "NE" | "LES" | "LT" | "LEQ" | "LE" | "GRT" | "GT" | "GEQ" | "GE" => {
                Self::arity(i, 2)?;
                let x = self.src(prog, &a[0])?;
                let y = self.src(prog, &a[1])?;
                let t = if x.num == Num::Real || y.num == Num::Real {
                    Num::Real
                } else {
                    Num::DInt
                };
                let op = match i.name.as_str() {
                    "EQU" | "EQ" => "=",
                    "NEQ" | "NE" => "<>",
                    "LES" | "LT" => "<",
                    "LEQ" | "LE" => "<=",
                    "GRT" | "GT" => ">",
                    _ => ">=",
                };
                code(format!("{r} := {r} AND ({} {op} {});", x.as_num(t), y.as_num(t)));
            }
            "LIM" | "LIMIT" => {
                Self::arity(i, 3)?;
                let lo = self.src(prog, &a[0])?;
                let x = self.src(prog, &a[1])?;
                let hi = self.src(prog, &a[2])?;
                let t = if [lo.num, x.num, hi.num].contains(&Num::Real) {
                    Num::Real
                } else {
                    Num::DInt
                };
                let (l, v, h) = (lo.as_num(t), x.as_num(t), hi.as_num(t));
                code(format!(
                    "{r} := {r} AND ((({l} <= {h}) AND ({l} <= {v}) AND ({v} <= {h})) OR (({l} > {h}) AND (({v} >= {l}) OR ({v} <= {h}))));"
                ));
            }
            "ADD" | "SUB" | "MUL" | "DIV" | "MOD" => {
                Self::arity(i, 3)?;
                let x = self.src(prog, &a[0])?;
                let y = self.src(prog, &a[1])?;
                let (d, dn) = self.num_dest(prog, &a[2])?;
                let t = if [x.num, y.num, dn].contains(&Num::Real) {
                    Num::Real
                } else {
                    Num::DInt
                };
                let (xs, ys) = (x.as_num(t), y.as_num(t));
                let e = self.binop(
                    match i.name.as_str() {
                        "ADD" => Op::Add,
                        "SUB" => Op::Sub,
                        "MUL" => Op::Mul,
                        "DIV" => Op::Div,
                        _ => Op::Mod,
                    },
                    &xs,
                    &ys,
                    t,
                );
                code(format!("IF {r} THEN {d} := {}; END_IF;", conv(&e, t, dn)));
            }
            "CPT" => {
                Self::arity(i, 2)?;
                let (d, dn) = self.num_dest(prog, &a[0])?;
                let ex = rung::expr(&a[1])?;
                let mut leaves = Vec::new();
                self.leaves(prog, &ex, &mut leaves)?;
                let t = if dn == Num::Real || leaves.contains(&Num::Real) {
                    Num::Real
                } else {
                    Num::DInt
                };
                let e = self.cpt(prog, &ex, t)?;
                code(format!("IF {r} THEN {d} := {}; END_IF;", conv(&e, t, dn)));
            }
            "MOV" | "MOVE" => {
                Self::arity(i, 2)?;
                let x = self.src(prog, &a[0])?;
                let (d, dn) = self.num_dest(prog, &a[1])?;
                let v = match (&x.lit, dn) {
                    (Some(Operand::Int(_)), Num::DInt | Num::Real) => x.as_num(dn),
                    _ => conv(&x.st, x.num, dn),
                };
                code(format!("IF {r} THEN {d} := {v}; END_IF;"));
            }
            "CLR" => {
                Self::arity(i, 1)?;
                let (d, dn) = self.num_dest(prog, &a[0])?;
                let z = if dn == Num::Real { "0.0" } else { "0" };
                code(format!("IF {r} THEN {d} := {z}; END_IF;"));
            }
            "COP" => {
                Self::arity(i, 3)?;
                let s = self.path_place(prog, &a[0])?;
                let d = self.path_place(prog, &a[1])?;
                self.writable(&d, &a[1])?;
                if s.kind != d.kind {
                    return Err(format!(
                        "copies a {} into a {}: COP copies bytes between types; not converted",
                        s.kind.logix(),
                        d.kind.logix()
                    ));
                }
                let len = match rung::operand(&a[2])? {
                    Operand::Int(n) => i64::from(n),
                    _ => return Err("the length must be an integer immediate".into()),
                };
                let (Some((s0, sleft)), Some((d0, dleft))) = (s.run, d.run) else {
                    return Err(
                        "COP is converted between elements of one-dimensional arrays or whole scalars".into(),
                    );
                };
                if len < 1 || len > MAX_COP {
                    return Err(format!("length {len} is outside 1..{MAX_COP}"));
                }
                if len > sleft as i64 {
                    return Err(format!(
                        "length {len} runs past the end of {} ({sleft} element(s) left); controllers differ there",
                        a[0]
                    ));
                }
                let n = len.min(dleft as i64) as usize;
                let base = |p: &Place| p.st.split('[').next().unwrap_or("").to_string();
                let (sb, db) = (base(&s), base(&d));
                let sarr = !self.tags[s.tag].dims.is_empty();
                let darr = !self.tags[d.tag].dims.is_empty();
                let mut body = String::new();
                for k in 0..n {
                    let sx = if sarr {
                        format!("{sb}[{}]", s0 + k)
                    } else {
                        sb.clone()
                    };
                    let dx = if darr {
                        format!("{db}[{}]", d0 + k)
                    } else {
                        db.clone()
                    };
                    let _ = write!(body, " {dx} := {sx};");
                }
                code(format!("IF {r} THEN{body} END_IF;"));
            }
            "JSR" => {
                if !(a.len() == 1 || (a.len() == 2 && a[1].trim() == "0")) {
                    return Err("JSR with parameters is not converted".into());
                }
                if depth != 0 || !last {
                    return Err(
                        "JSR is converted only as the last instruction of its rung, outside branches".into(),
                    );
                }
                let name = a[0].trim();
                let Some(k) = routines.iter().position(|n| n.eq_ignore_ascii_case(name)) else {
                    return Err(format!("{name} is not a routine of this program"));
                };
                rc.code.push(Piece::Line(format!("IF {r} THEN")));
                rc.code.push(Piece::Jsr(k));
                rc.code.push(Piece::Line("END_IF;".into()));
                rc.pre.push(Piece::Jsr(k));
                return Ok(());
            }
            other => {
                let aoi = self.p.add_ons.iter().any(|n| n.eq_ignore_ascii_case(other));
                return Err(if aoi {
                    format!("{other} is an add-on instruction; add-on instructions are not converted")
                } else {
                    format!("{other} is not an instruction this converter translates")
                });
            }
        }
        rc.code.extend(lines.into_iter().map(Piece::Line));
        Ok(())
    }

    fn binop(&mut self, op: Op, x: &str, y: &str, t: Num) -> String {
        match (op, t) {
            (Op::Add, _) => format!("({x} + {y})"),
            (Op::Sub, _) => format!("({x} - {y})"),
            (Op::Mul, _) => format!("({x} * {y})"),
            (Op::Div, Num::Real) => format!("({x} / {y})"),
            (Op::Div, _) => {
                self.needs.insert("DIVI");
                format!("LOGIX_DIVI({x}, {y})")
            }
            (Op::Mod, Num::Real) => {
                self.needs.insert("MODR");
                format!("LOGIX_MODR({x}, {y})")
            }
            (Op::Mod, _) => {
                self.needs.insert("MODI");
                format!("LOGIX_MODI({x}, {y})")
            }
        }
    }

    fn leaves(&mut self, prog: usize, e: &Expr, out: &mut Vec<Num>) -> Result<(), String> {
        match e {
            Expr::Int(_) => out.push(Num::DInt),
            Expr::Real(_) => out.push(Num::Real),
            Expr::Path(p) => {
                let s = self.src(prog, &p.to_string())?;
                out.push(s.num);
            }
            Expr::Neg(x) => self.leaves(prog, x, out)?,
            Expr::Bin(_, x, y) => {
                self.leaves(prog, x, out)?;
                self.leaves(prog, y, out)?;
            }
        }
        Ok(())
    }

    fn cpt(&mut self, prog: usize, e: &Expr, t: Num) -> Result<String, String> {
        Ok(match e {
            Expr::Int(n) => Src {
                st: int_lit(i64::from(*n)),
                num: Num::DInt,
                lit: Some(Operand::Int(*n)),
            }
            .as_num(t),
            Expr::Real(r) => real_lit(*r),
            Expr::Path(p) => self.src(prog, &p.to_string())?.as_num(t),
            Expr::Neg(x) => format!("(-{})", self.cpt(prog, x, t)?),
            Expr::Bin(op, x, y) => {
                let (xs, ys) = (self.cpt(prog, x, t)?, self.cpt(prog, y, t)?);
                self.binop(*op, &xs, &ys, t)
            }
        })
    }
}

/// An atomic decorated value as a literal of `k`, or why not.
fn value_lit(k: Kind, v: &str) -> Result<String, String> {
    match k {
        Kind::Bool => match v.trim() {
            "0" => Ok("FALSE".into()),
            "1" => Ok("TRUE".into()),
            _ => Err(format!("{v:?} is not a BOOL value")),
        },
        Kind::Num(Num::Real) => {
            let x: f32 = v
                .trim()
                .parse()
                .map_err(|_| format!("{v:?} is not a REAL value"))?;
            if x.is_finite() {
                Ok(real_lit(x))
            } else {
                Err(format!("{v:?} is not a finite REAL"))
            }
        }
        Kind::Num(n) => {
            let x = match rung::operand(v.trim()) {
                Ok(Operand::Int(x)) => i64::from(x),
                _ => return Err(format!("{v:?} is not a {} value", n.st())),
            };
            let (lo, hi) = match n {
                Num::SInt => (-128, 127),
                Num::Int => (-32768, 32767),
                _ => (i64::from(i32::MIN), i64::from(i32::MAX)),
            };
            if x < lo || x > hi {
                return Err(format!("{v} does not fit a {}", n.st()));
            }
            Ok(int_lit(x))
        }
        Kind::Timer | Kind::Counter => Err("a structure value where an atomic one is expected".into()),
    }
}

/// An initial value for one element: `None` when all default.
fn elem_init(
    k: Kind,
    d: Option<&Data>,
    pseudo: Option<&(Option<i32>, Option<i32>, String)>,
) -> Result<Option<String>, String> {
    match k {
        Kind::Timer | Kind::Counter => {
            let mut fields: Vec<(String, String)> = Vec::new();
            if let Some(Data::Structure(ms)) = d {
                for (name, v) in ms {
                    let up = name.to_ascii_uppercase();
                    let Some((n, mk)) = members(k).iter().find(|(n, _)| *n == up) else {
                        continue;
                    };
                    let Data::Value(v) = v else {
                        return Err(format!("member {name} is not an atomic value"));
                    };
                    fields.push(((*n).to_string(), value_lit(*mk, v)?));
                }
            } else if d.is_some() {
                return Err(format!("the value is not a {} structure", k.logix()));
            }
            let has = |f: &[(String, String)], n: &str| f.iter().any(|(x, _)| x == n);
            if let Some((pre, acc, _)) = pseudo {
                if let Some(p) = pre
                    && !has(&fields, "PRE")
                {
                    fields.push(("PRE".into(), int_lit(i64::from(*p))));
                }
                if let Some(a) = acc
                    && !has(&fields, "ACC")
                {
                    fields.push(("ACC".into(), int_lit(i64::from(*a))));
                }
            }
            fields.retain(|(_, v)| v != "0" && v != "FALSE");
            if fields.is_empty() {
                return Ok(None);
            }
            let list: Vec<String> = fields.iter().map(|(n, v)| format!("{n} := {v}")).collect();
            Ok(Some(format!("({})", list.join(", "))))
        }
        _ => match d {
            None => Ok(None),
            Some(Data::Value(v)) => {
                let l = value_lit(k, v)?;
                Ok((l != "0" && l != "FALSE" && l != "0.0").then_some(l))
            }
            Some(_) => Err("a structure or array value for an atomic tag".into()),
        },
    }
}

/// A tag's declaration initialiser, or `None`.
fn tag_init(cx: &Cx, id: TagId) -> Result<Option<String>, String> {
    let t = &cx.tags[id];
    let data = t.data.as_ref().map(|d| &d.value);
    if t.dims.is_empty() {
        return elem_init(t.kind, data, cx.pseudo.get(&(id, 0)));
    }
    let total: usize = t.dims.iter().map(|d| *d as usize).product();
    let mut elems: Vec<Option<&Data>> = vec![None; total];
    match data {
        None => {}
        Some(Data::Array(list)) => {
            for (ix, v) in list {
                let inner = ix
                    .trim()
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or("");
                let nums: Option<Vec<usize>> = inner.split(',').map(|s| s.trim().parse().ok()).collect();
                let Some(nums) = nums.filter(|n| n.len() == t.dims.len()) else {
                    return Err(format!("element index {ix} does not fit {}", t.name));
                };
                let mut lin = 0usize;
                for (n, d) in nums.iter().zip(&t.dims) {
                    if *n >= *d as usize {
                        return Err(format!("element index {ix} does not fit {}", t.name));
                    }
                    lin = lin * (*d as usize) + n;
                }
                elems[lin] = Some(v);
            }
        }
        Some(_) => return Err("an atomic or structure value for an array tag".into()),
    }
    let mut out = Vec::with_capacity(total);
    let mut any = false;
    for (k, d) in elems.iter().enumerate() {
        let v = elem_init(t.kind, *d, cx.pseudo.get(&(id, k)))?;
        any |= v.is_some();
        out.push(v.unwrap_or_else(|| match t.kind {
            Kind::Bool => "FALSE".into(),
            Kind::Num(Num::Real) => "0.0".into(),
            Kind::Num(_) => "0".into(),
            _ => "(ACC := 0)".into(),
        }));
    }
    if !any {
        return Ok(None);
    }
    Ok(Some(format!("[{}]", out.join(", "))))
}

/// One PROGRAM to build: a task, or the whole export without tasks.
struct Unit {
    name: String,
    programs: Vec<usize>,
}

/// Converts an L5X export (see the module documentation).
///
/// # Errors
/// Every problem found, each with where it is.
#[allow(clippy::too_many_lines)]
pub fn convert(src: &str) -> Result<Converted, Refused> {
    let p = match l5x::parse(src) {
        Ok(p) => p,
        Err(e) => {
            return Err(Refused {
                problems: vec![Problem {
                    at: format!("line {}", e.pos.line),
                    msg: e.msg,
                }],
                total: 1,
            });
        }
    };
    let mut cx = Cx {
        tags: Vec::new(),
        ctl: HashMap::new(),
        prog: Vec::new(),
        problems: Vec::new(),
        total: 0,
        notes: Vec::new(),
        used: vec![BTreeSet::new(); p.programs.len()],
        pseudo: BTreeMap::new(),
        needs: BTreeSet::new(),
        written: BTreeSet::new(),
        p,
    };
    for s in cx.p.skipped.clone() {
        cx.problem(
            format!("line {}", s.pos.line),
            format!("left out of the export as read: {}", s.why),
        );
    }
    if cx.p.skipped_total > cx.p.skipped.len() {
        let n = cx.p.skipped_total - cx.p.skipped.len();
        cx.problem(
            String::new(),
            format!("{n} more parts left out of the export as read"),
        );
    }
    if cx.p.programs.is_empty() {
        cx.problem(String::new(), "the export holds no program".into());
    }

    // Tags.
    let add = |cx: &mut Cx, t: &l5x::Tag, program: Option<usize>| {
        let scope = program.map_or_else(
            || "controller tag".to_string(),
            |k| format!("tag of program {}", cx.p.programs[k].name),
        );
        let mut bad = None;
        let ty = t.data_type.clone().unwrap_or_default();
        let kind = Kind::from_logix(&ty);
        match &t.kind {
            TagKind::Produced | TagKind::Consumed => {
                bad = Some(format!(
                    "{} is a produced or consumed {scope} (data shared with other controllers); not converted",
                    t.name
                ));
            }
            TagKind::Base if kind.is_none() => {
                let what = if cx.p.data_types.iter().any(|d| d.name.eq_ignore_ascii_case(&ty)) {
                    format!("the user-defined type {ty}")
                } else {
                    format!("type {ty}")
                };
                bad = Some(format!(
                    "{} is a {scope} of {what}, which is not converted",
                    t.name
                ));
            }
            _ => {}
        }
        if t.usage.as_deref() == Some("InOut") {
            bad = Some(format!(
                "{} is an InOut parameter (a reference to another tag); not converted",
                t.name
            ));
        }
        if t.dimensions.contains(&0) {
            bad = Some(format!("{} has a zero dimension", t.name));
        }
        let alias = match &t.kind {
            TagKind::Alias(a) => Some(a.clone()),
            _ => None,
        };
        let id = cx.tags.len();
        cx.tags.push(TagDef {
            name: t.name.clone(),
            program,
            bad,
            kind: kind.unwrap_or(Kind::Bool),
            dims: t.dimensions.clone(),
            constant: t.constant,
            alias,
            usage: t.usage.clone(),
            data: t.data.clone(),
        });
        let up = t.name.to_ascii_uppercase();
        let map = match program {
            None => &mut cx.ctl,
            Some(k) => &mut cx.prog[k],
        };
        map.insert(up, id)
    };
    for t in cx.p.tags.clone() {
        if add(&mut cx, &t, None).is_some() {
            cx.problem(
                String::new(),
                format!("controller tag {} is declared twice", t.name),
            );
        }
    }
    for k in 0..cx.p.programs.len() {
        cx.prog.push(HashMap::new());
        for t in cx.p.programs[k].tags.clone() {
            if add(&mut cx, &t, Some(k)).is_some() {
                let pn = cx.p.programs[k].name.clone();
                cx.problem(
                    format!("program {pn}"),
                    format!("tag {} is declared twice", t.name),
                );
            }
        }
    }

    // Units: tasks, or the whole export.
    let mut units: Vec<Unit> = Vec::new();
    let mut scheduled: BTreeMap<usize, String> = BTreeMap::new();
    if cx.p.tasks.is_empty() {
        let name =
            cx.p.controller
                .as_ref()
                .map(|c| c.name.clone())
                .or_else(|| cx.p.target.clone().filter(|t| l5x::is_name(t)))
                .unwrap_or_else(|| "L5X".into());
        units.push(Unit {
            name,
            programs: (0..cx.p.programs.len()).collect(),
        });
    } else {
        for task in cx.p.tasks.clone() {
            let mut progs = Vec::new();
            for pn in &task.programs {
                match cx.p.programs.iter().position(|p| p.name.eq_ignore_ascii_case(pn)) {
                    Some(k) => {
                        if let Some(other) = scheduled.insert(k, task.name.clone()) {
                            cx.problem(
                                format!("task {}", task.name),
                                format!("program {pn} is also scheduled in task {other}"),
                            );
                        }
                        progs.push(k);
                    }
                    None => cx.problem(
                        format!("task {}", task.name),
                        format!("schedules program {pn}, which is not in the export"),
                    ),
                }
            }
            match task.kind.as_deref() {
                Some("EVENT") => {
                    cx.problem(
                        format!("task {}", task.name),
                        "event tasks are not converted".into(),
                    );
                    continue;
                }
                Some("PERIODIC") if !progs.is_empty() => {
                    if let Some(ms) = task.rate_ms {
                        cx.notes.push(format!(
                            "Task {} is periodic every {ms} ms in Logix: give PROGRAM {} that scan in project.settings.",
                            task.name, task.name
                        ));
                    }
                }
                _ => {}
            }
            if progs.is_empty() {
                cx.notes.push(format!(
                    "Task {} schedules no program; nothing of it is converted.",
                    task.name
                ));
                continue;
            }
            units.push(Unit {
                name: task.name.clone(),
                programs: progs,
            });
        }
        for (k, pr) in cx.p.programs.clone().iter().enumerate() {
            if !scheduled.contains_key(&k) {
                let n = pr.name.clone();
                cx.problem(
                    format!("program {n}"),
                    "is scheduled in no task, so Logix never runs it; schedule it or export without it"
                        .into(),
                );
            }
        }
    }
    for u in &units {
        if reserved(&u.name.to_ascii_uppercase()) {
            cx.problem(
                format!("task {}", u.name),
                "its name is a Structured Text keyword or standard name".into(),
            );
        }
    }

    // Routines: parse, type and emit every rung of every program.
    let mut code: Vec<Vec<Option<RoutineCode>>> = Vec::new();
    let mut mains: Vec<Option<usize>> = Vec::new();
    for k in 0..cx.p.programs.len() {
        let pr = cx.p.programs[k].clone();
        let names: Vec<String> = pr.routines.iter().map(|r| r.name.clone()).collect();
        if pr.disabled {
            cx.problem(
                format!("program {}", pr.name),
                "is disabled, so Logix does not run it; enable it or export without it".into(),
            );
        }
        let main = pr
            .main_routine
            .as_ref()
            .and_then(|m| names.iter().position(|n| n.eq_ignore_ascii_case(m)));
        if main.is_none() {
            cx.problem(
                format!("program {}", pr.name),
                "has no main routine in the export".into(),
            );
        }
        mains.push(main);
        let mut rcode = Vec::new();
        for r in &pr.routines {
            if r.kind != RoutineKind::Ladder {
                let why = match r.kind {
                    RoutineKind::St => {
                        "a Structured Text routine: Logix ST calls instructions (TON on a TIMER, JSR, ...) that IEC 61131-3 ST does not have, so it is not passed through"
                    }
                    RoutineKind::Fbd => {
                        "a function block diagram routine; only ladder routines are converted"
                    }
                    _ => "a sequential function chart routine; only ladder routines are converted",
                };
                cx.problem(at(&pr.name, &r.name, None), format!("is {why}"));
                rcode.push(None);
                continue;
            }
            let mut out = RoutineCode::default();
            for g in &r.rungs {
                let w = at(&pr.name, &r.name, Some(g.number));
                if g.text.contains("[clipped:") {
                    cx.problem(w, "the rung is longer than this reader keeps".into());
                    continue;
                }
                let elems = match rung::parse(&g.text) {
                    Ok(e) => e,
                    Err(m) => {
                        cx.problem(w, format!("the rung does not read as ladder: {m}"));
                        continue;
                    }
                };
                match cx.rung(k, &names, &elems, &w) {
                    Ok(c) => {
                        // JSR to the program's main routine is a fault (RM003 p.626).
                        if c.code
                            .iter()
                            .any(|p| matches!(p, Piece::Jsr(t) if Some(*t) == main))
                        {
                            cx.problem(
                                w,
                                "JSR calls the program's main routine, which Logix forbids".into(),
                            );
                            continue;
                        }
                        out.rungs.push((g.number, g.text.clone(), c));
                    }
                    Err(errs) => {
                        for m in errs {
                            cx.problem(w.clone(), m);
                        }
                    }
                }
            }
            rcode.push(Some(out));
        }
        code.push(rcode);
    }

    // JSR graph: recursion and depth, per program.
    for (k, pr) in cx.p.programs.clone().iter().enumerate() {
        let Some(m) = mains[k] else { continue };
        let mut seen = BTreeSet::new();
        let mut stack = vec![m];
        walk(&mut cx, &code[k], pr, m, &mut stack, &mut seen);
        for (ri, r) in pr.routines.iter().enumerate() {
            if !seen.contains(&ri) && code[k][ri].is_some() {
                cx.notes.push(format!(
                    "Routine {} of program {} is never called (no JSR reaches it): it was checked, not converted.",
                    r.name, pr.name
                ));
            }
        }
    }

    // Controller tags shared between tasks.
    let mut owner: BTreeMap<TagId, usize> = BTreeMap::new();
    let mut shared = BTreeSet::new();
    for (u, unit) in units.iter().enumerate() {
        for &k in &unit.programs {
            for id in cx.used[k].clone() {
                if cx.tags[id].program.is_some() {
                    continue;
                }
                if let Some(&o) = owner.get(&id)
                    && o != u
                    && shared.insert(id)
                {
                    let (a, b) = (units[o].name.clone(), unit.name.clone());
                    let n = cx.tags[id].name.clone();
                    cx.problem(
                        format!("controller tag {n}"),
                        format!("is used by tasks {a} and {b}; the box's programs share no memory"),
                    );
                }
                owner.entry(id).or_insert(u);
            }
        }
    }

    // Names.
    let mut names: Vec<String> = cx.tags.iter().map(|t| t.name.clone()).collect();
    let mut decl: Vec<Vec<TagId>> = vec![Vec::new(); units.len()];
    for (u, unit) in units.iter().enumerate() {
        let mut taken: BTreeMap<String, TagId> = BTreeMap::new();
        taken.insert(unit.name.to_ascii_uppercase(), usize::MAX);
        let ctl: Vec<TagId> = (0..cx.tags.len())
            .filter(|&id| {
                let t = &cx.tags[id];
                t.program.is_none()
                    && t.alias.is_none()
                    && t.bad.is_none()
                    && owner.get(&id).map_or(u == 0, |&o| o == u)
            })
            .collect();
        let progs: Vec<TagId> = (0..cx.tags.len())
            .filter(|&id| {
                let t = &cx.tags[id];
                t.alias.is_none() && t.bad.is_none() && t.program.is_some_and(|p| unit.programs.contains(&p))
            })
            .collect();
        for &id in ctl.iter().chain(&progs) {
            let t = &cx.tags[id];
            let mut n = t.name.clone();
            if taken.contains_key(&n.to_ascii_uppercase())
                && let Some(p) = t.program
            {
                n = format!("{}_{}", cx.p.programs[p].name, t.name);
            }
            if reserved(&n.to_ascii_uppercase()) && !n.to_ascii_uppercase().starts_with("LOGIX_") {
                // `Log`, `Max`, `Limit`: names IEC ST keeps for itself.
                let renamed = if n.ends_with('_') {
                    format!("{n}tag")
                } else {
                    format!("{n}_tag")
                };
                cx.notes.push(format!(
                    "Tag {} is {renamed} here: {n} is a Structured Text keyword or standard name.",
                    t.name
                ));
                n = renamed;
            }
            let up = n.to_ascii_uppercase();
            if taken.contains_key(&up) {
                let tn = t.name.clone();
                cx.problem(
                    format!("tag {tn}"),
                    format!("its name {n} is taken in PROGRAM {}", unit.name),
                );
                continue;
            }
            if reserved(&up) {
                let tn = t.name.clone();
                cx.problem(
                    format!("tag {tn}"),
                    "its name starts with LOGIX_, which the converter keeps for itself".into(),
                );
                continue;
            }
            taken.insert(up, id);
            names[id].clone_from(&n);
            decl[u].push(id);
        }
    }
    // Unused tags of unconverted kinds are left out, said once.
    let pick = |f: &dyn Fn(&TagDef) -> bool| -> Vec<String> {
        cx.tags.iter().filter(|t| f(t)).map(|t| t.name.clone()).collect()
    };
    let aliases = pick(&|t| t.alias.is_some() && t.bad.is_none());
    let left = pick(&|t| t.bad.is_some());
    if !aliases.is_empty() {
        cx.notes.push(format!(
            "Aliases become the tags they name: {}.",
            aliases.join(", ")
        ));
    }
    if !left.is_empty() {
        cx.notes.push(format!(
            "Left out (no converted rung uses them; their kind is not converted): {}.",
            left.join(", ")
        ));
    }
    for t in &cx.tags {
        if t.data.as_ref().is_some_and(|d| d.forced) && t.bad.is_none() {
            cx.notes.push(format!(
                "Tag {} has a force in the export; forces are not carried over.",
                t.name
            ));
        }
    }

    // Declarations and initial values.
    let mut inits: BTreeMap<TagId, String> = BTreeMap::new();
    for ids in &decl {
        for &id in ids {
            match tag_init(&cx, id) {
                Ok(Some(v)) => {
                    inits.insert(id, v);
                }
                Ok(None) => {}
                Err(m) => {
                    let n = cx.tags[id].name.clone();
                    cx.problem(format!("tag {n}"), format!("its value in the export: {m}"));
                }
            }
        }
    }

    if cx.total > 0 {
        return Err(Refused {
            problems: cx.problems,
            total: cx.total,
        });
    }

    // Assemble.
    let mut body = String::new();
    let mut programs = Vec::new();
    for (u, unit) in units.iter().enumerate() {
        let mut out = Out::default();
        let mut depth = 0usize;
        let mut prescan = String::new();
        for &k in &unit.programs {
            let Some(m) = mains[k] else { continue };
            let pr = &cx.p.programs[k];
            let mut pre_done = BTreeSet::new();
            pre(&code[k], m, &mut pre_done, &mut prescan, &mut depth);
            let _ = writeln!(
                out.s,
                "  (* ---- program {} (main routine {}) ---- *)",
                pr.name, pr.routines[m].name
            );
            if let Err(m) = emit(&code[k], pr, m, 1, &mut out, &mut depth) {
                return Err(Refused {
                    problems: vec![Problem {
                        at: format!("PROGRAM {}", unit.name),
                        msg: m,
                    }],
                    total: 1,
                });
            }
        }
        let mut s = String::new();
        let _ = writeln!(s, "PROGRAM {}", unit.name);
        let (mut ins, mut outs, mut vars) = (String::new(), String::new(), String::new());
        for &id in &decl[u] {
            let t = &cx.tags[id];
            let mut ty = t.kind.st().to_string();
            if !t.dims.is_empty() {
                let d: Vec<String> = t.dims.iter().map(|d| format!("0..{}", d - 1)).collect();
                ty = format!("ARRAY[{}] OF {ty}", d.join(", "));
            }
            let init = inits.get(&id).map(|v| format!(" := {v}")).unwrap_or_default();
            let line = format!("    {} : {ty}{init};\n", names[id]);
            let port =
                t.program.is_some() && t.dims.is_empty() && matches!(t.kind, Kind::Bool | Kind::Num(_));
            match t.usage.as_deref() {
                Some("Input") if port => ins.push_str(&line),
                Some("Output") if port => outs.push_str(&line),
                _ => vars.push_str(&line),
            }
        }
        if !ins.is_empty() {
            let _ = write!(s, "  VAR_INPUT\n{ins}  END_VAR\n");
        }
        if !outs.is_empty() {
            let _ = write!(s, "  VAR_OUTPUT\n{outs}  END_VAR\n");
        }
        let _ = writeln!(s, "  VAR");
        s.push_str(&vars);
        let _ = writeln!(s, "    (* the converter's own *)");
        let _ = writeln!(s, "    LOGIX_PRESCANNED : BOOL;");
        let _ = write!(s, "    LOGIX_RC0");
        for d in 1..=depth {
            let _ = write!(s, ", LOGIX_RC{d}, LOGIX_BI{d}, LOGIX_BO{d}");
        }
        let _ = writeln!(s, " : BOOL;");
        let uses_clock = cx.needs.contains("CLOCK");
        if uses_clock {
            let _ = writeln!(s, "    LOGIX_CLOCK : TON;\n    LOGIX_NOW : DINT;");
        }
        for h in ["TON", "TOF", "RTO", "CTU", "CTD"] {
            if cx.needs.contains(h) {
                let _ = writeln!(s, "    LOGIX_H_{h} : LOGIX_{h};");
            }
        }
        let _ = writeln!(s, "  END_VAR");
        if uses_clock {
            let _ = writeln!(s, "  (* the scan's clock in ms *)");
            let _ = writeln!(s, "  LOGIX_CLOCK(IN := TRUE, PT := T#100000d);");
            let _ = writeln!(s, "  LOGIX_NOW := TIME_TO_DINT(LOGIX_CLOCK.ET);");
        }
        let _ = writeln!(
            s,
            "  (* Logix's prescan, before the first scan (RM003, each instruction's Prescan row) *)"
        );
        let _ = writeln!(s, "  IF NOT LOGIX_PRESCANNED THEN");
        s.push_str(&prescan);
        let _ = writeln!(s, "    LOGIX_PRESCANNED := TRUE;\n  END_IF;");
        s.push_str(&out.s);
        let _ = writeln!(s, "END_PROGRAM\n");
        body.push_str(&s);
        programs.push(unit.name.clone());
    }
    // Names in.
    let mut named = String::with_capacity(body.len());
    let mut rest = body.as_str();
    while let Some(i) = rest.find('\u{1}') {
        named.push_str(&rest[..i]);
        let j = rest[i..].find('\u{2}').map_or(rest.len(), |j| i + j);
        let id: usize = rest[i + 1..j].parse().unwrap_or(0);
        named.push_str(&names[id]);
        rest = &rest[(j + 1).min(rest.len())..];
    }
    named.push_str(rest);

    let mut head = String::new();
    let _ = writeln!(
        head,
        "(* Converted from the L5X export of {} by the box's ladder converter.",
        cx.p.controller
            .as_ref()
            .map_or("a controller", |c| c.name.as_str())
    );
    for n in &cx.notes {
        let _ = writeln!(head, "   {n}");
    }
    let _ = writeln!(head, "*)\n");
    head.push_str(lib_head());
    let needs_types =
        cx.needs
            .iter()
            .any(|n| ["TON", "TOF", "RTO", "CTU", "CTD"].contains(n))
            || cx.tags.iter().any(|t| {
                matches!(t.kind, Kind::Timer | Kind::Counter) && t.bad.is_none() && t.alias.is_none()
            });
    if needs_types {
        head.push_str(lib_part("TYPES"));
    }
    let r2l = named.contains("LOGIX_R2L(");
    for part in ["TON", "RTO", "TOF", "CTU", "CTD", "DIVI", "MODI", "MODR"] {
        if cx.needs.contains(part) {
            head.push_str(lib_part(part));
        }
    }
    if r2l {
        head.push_str(lib_part("R2L"));
    }
    // Every PROGRAM must compile with the helpers; a failure here is the
    // converter's bug, refused rather than shipped.
    let mut problems = Vec::new();
    for prog in named.split_inclusive("END_PROGRAM\n") {
        if !prog.contains("PROGRAM ") {
            continue;
        }
        let unit = format!("{head}{prog}");
        if let Err(diags) = crate::compile(&unit, crate::ImageSizes { i: 0, q: 0, m: 0 }) {
            for d in diags.into_iter().take(5) {
                problems.push(Problem {
                    at: "the converted Structured Text".into(),
                    msg: format!(
                        "does not compile ({}); this is a converter fault, please report it",
                        d.msg
                    ),
                });
            }
        }
    }
    if !problems.is_empty() {
        let total = problems.len();
        return Err(Refused { problems, total });
    }
    Ok(Converted {
        st: format!("{head}{named}"),
        programs,
        notes: cx.notes,
    })
}

#[derive(Default)]
struct Out {
    s: String,
}

fn walk(
    cx: &mut Cx,
    code: &[Option<RoutineCode>],
    pr: &l5x::Program,
    r: usize,
    stack: &mut Vec<usize>,
    seen: &mut BTreeSet<usize>,
) {
    seen.insert(r);
    let Some(Some(rc)) = code.get(r) else { return };
    for (num, _, c) in &rc.rungs {
        for p in &c.code {
            let Piece::Jsr(t) = p else { continue };
            let w = at(&pr.name, &pr.routines[r].name, Some(*num));
            if stack.contains(t) {
                let chain: Vec<&str> = stack
                    .iter()
                    .chain([t])
                    .map(|k| pr.routines[*k].name.as_str())
                    .collect();
                cx.problem(
                    w,
                    format!("JSR recursion ({}) is not converted", chain.join(" -> ")),
                );
                continue;
            }
            if stack.len() > MAX_JSR_DEPTH {
                cx.problem(w, format!("subroutines nest deeper than {MAX_JSR_DEPTH}"));
                continue;
            }
            if code.get(*t).is_some_and(Option::is_none) {
                // The callee is refused already.
                continue;
            }
            stack.push(*t);
            walk(cx, code, pr, *t, stack, seen);
            stack.pop();
        }
    }
}

/// Prescan lines, each routine once (RM003 p.627).
fn pre(
    code: &[Option<RoutineCode>],
    r: usize,
    done: &mut BTreeSet<usize>,
    out: &mut String,
    depth: &mut usize,
) {
    if !done.insert(r) {
        return;
    }
    let Some(Some(rc)) = code.get(r) else { return };
    for (_, _, c) in &rc.rungs {
        *depth = (*depth).max(c.depth);
        for p in &c.pre {
            match p {
                Piece::Line(l) => {
                    let _ = writeln!(out, "    {l}");
                }
                Piece::Jsr(t) => pre(code, *t, done, out, depth),
            }
        }
    }
}

fn comment(s: &str) -> String {
    s.replace("(*", "( *").replace("*)", "* )")
}

fn emit(
    code: &[Option<RoutineCode>],
    pr: &l5x::Program,
    r: usize,
    indent: usize,
    out: &mut Out,
    depth: &mut usize,
) -> Result<(), String> {
    let Some(Some(rc)) = code.get(r) else {
        return Ok(());
    };
    let pad = "  ".repeat(indent);
    for (num, text, c) in &rc.rungs {
        *depth = (*depth).max(c.depth);
        let _ = writeln!(
            out.s,
            "{pad}(* {} rung {num}: {} *)",
            pr.routines[r].name,
            comment(text)
        );
        for p in &c.code {
            match p {
                Piece::Line(l) => {
                    let _ = writeln!(out.s, "{pad}{l}");
                }
                Piece::Jsr(t) => emit(code, pr, *t, indent + 1, out, depth)?,
            }
        }
        if out.s.len() > MAX_ST {
            return Err(format!(
                "inlining the subroutines makes more than {} MiB of Structured Text",
                MAX_ST >> 20
            ));
        }
    }
    Ok(())
}
