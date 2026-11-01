//! The main proof of the L5X ladder converter: random rungs built from the
//! converted subset run through a small reference evaluator written here
//! from RM003's execution tables and flow charts (Logix semantics: rungs
//! left to right, top to bottom, rung-condition-in/out, branches OR their
//! legs), and through the converter's Structured Text on the IEC
//! interpreter. Over random inputs and scan times, every tag must match
//! after every scan, and a major fault must happen on the same scan.
//!
//! The evaluator shares only the rung *parser* with the converter; what
//! each instruction does is coded again here, independently.

// The evaluator uses `as` on purpose: Logix truncates and wraps where
// these casts do (RM003 pp.851-856).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::struct_excessive_bools,
    clippy::match_same_arms
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use helmsman_iec::ladder::rung::{self, Elem, Expr, Index, Op, Operand, Path, Seg};
use helmsman_iec::{Image, ImageSizes, Value};
use proptest::prelude::*;

// ---- a tiny deterministic generator, seeded by proptest ----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[usize::try_from(self.below(xs.len() as u64)).unwrap()]
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

// ---- the tag set every random program declares ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Bool,
    SInt,
    Int,
    DInt,
    Real,
    Timer,
    Counter,
}

/// (name, kind, dims, Usage=Input)
const TAGS: &[(&str, K, u32, bool)] = &[
    ("I0", K::Bool, 0, true),
    ("I1", K::Bool, 0, true),
    ("I2", K::Bool, 0, true),
    ("N0", K::DInt, 0, true),
    ("R0", K::Real, 0, true),
    ("B0", K::Bool, 0, false),
    ("B1", K::Bool, 0, false),
    ("B2", K::Bool, 0, false),
    ("B3", K::Bool, 0, false),
    ("B4", K::Bool, 0, false),
    ("D0", K::DInt, 0, false),
    ("D1", K::DInt, 0, false),
    ("D2", K::DInt, 0, false),
    ("S0", K::SInt, 0, false),
    ("K0", K::Int, 0, false),
    ("F0", K::Real, 0, false),
    ("F1", K::Real, 0, false),
    ("T0", K::Timer, 0, false),
    ("T1", K::Timer, 0, false),
    ("C0", K::Counter, 0, false),
    ("C1", K::Counter, 0, false),
    ("A", K::DInt, 4, false),
];

const BOOL_IN: &[&str] = &[
    "I0", "I1", "I2", "B0", "B1", "B2", "B3", "B4", "T0.DN", "T0.TT", "T0.EN", "T1.DN", "C0.DN", "C1.DN",
    "C0.OV", "C1.UN", "D1.0", "D1.31", "K0.15", "S0.7",
];
const BOOL_OUT: &[&str] = &[
    "B0", "B1", "B2", "B3", "B4", "D1.0", "D1.5", "D1.31", "K0.15", "S0.7", "T0.DN",
];
const NUM_IN: &[&str] = &[
    "N0",
    "R0",
    "D0",
    "D1",
    "D2",
    "S0",
    "K0",
    "F0",
    "F1",
    "T0.ACC",
    "T0.PRE",
    "T1.ACC",
    "C0.ACC",
    "C1.PRE",
    "A[0]",
    "A[3]",
    "0",
    "1",
    "-1",
    "7",
    "300",
    "2147483647",
    "-2147483648",
    "16#FFFF",
    "2.5",
    "-0.5",
    "1.0e10",
    "0.1",
];
const NUM_OUT: &[&str] = &[
    "D0", "D1", "D2", "S0", "K0", "F0", "F1", "A[1]", "A[2]", "T0.PRE", "T1.PRE", "C0.ACC", "C1.PRE",
    "T0.ACC",
];

fn gen_expr(r: &mut Rng, depth: u32) -> String {
    if depth == 0 || r.chance(40) {
        let leaf = r.pick(NUM_IN);
        return if leaf.starts_with('-') {
            format!("({leaf})")
        } else {
            leaf.to_string()
        };
    }
    match r.below(6) {
        0 => format!("-({})", gen_expr(r, depth - 1)),
        1 => format!("({})", gen_expr(r, depth - 1)),
        k => {
            let op = ["+", "-", "*", "/", " MOD "][usize::try_from(k).unwrap() % 5];
            format!("{} {op} {}", gen_expr(r, depth - 1), gen_expr(r, depth - 1))
        }
    }
}

fn gen_instr(r: &mut Rng) -> String {
    match r.below(26) {
        0..=3 => format!("XIC({})", r.pick(BOOL_IN)),
        4..=5 => format!("XIO({})", r.pick(BOOL_IN)),
        6..=7 => format!("OTE({})", r.pick(BOOL_OUT)),
        8 => format!("OTL({})", r.pick(BOOL_OUT)),
        9 => format!("OTU({})", r.pick(BOOL_OUT)),
        10 => format!("ONS({})", r.pick(&["B3", "B4"])),
        11 => format!("OSR({},{})", r.pick(&["B3", "B4"]), r.pick(&["B0", "B1"])),
        12 => format!("OSF({},{})", r.pick(&["B3", "B4"]), r.pick(&["B0", "B2"])),
        13 => format!(
            "{}({},?,?)",
            r.pick(&["TON", "TOF", "RTO"]),
            r.pick(&["T0", "T1"])
        ),
        14 => format!("{}({},?,?)", r.pick(&["CTU", "CTD"]), r.pick(&["C0", "C1"])),
        15 => format!("RES({})", r.pick(&["T0", "T1", "C0", "C1"])),
        16..=17 => format!(
            "{}({},{})",
            r.pick(&[
                "EQU", "NEQ", "LES", "LEQ", "GRT", "GEQ", "EQ", "NE", "LT", "LE", "GT", "GE"
            ]),
            r.pick(NUM_IN),
            r.pick(NUM_IN)
        ),
        18 => format!(
            "{}({},{},{})",
            r.pick(&["LIM", "LIMIT"]),
            r.pick(NUM_IN),
            r.pick(NUM_IN),
            r.pick(NUM_IN)
        ),
        19..=20 => format!(
            "{}({},{},{})",
            r.pick(&["ADD", "SUB", "MUL", "DIV", "MOD"]),
            r.pick(NUM_IN),
            r.pick(NUM_IN),
            r.pick(NUM_OUT)
        ),
        21 => format!("CPT({},{})", r.pick(NUM_OUT), gen_expr(r, 3)),
        22 => format!(
            "{}({},{})",
            r.pick(&["MOV", "MOVE"]),
            r.pick(NUM_IN),
            r.pick(NUM_OUT)
        ),
        23 => format!("CLR({})", r.pick(NUM_OUT)),
        24 => {
            let s = r.below(4);
            let d = r.below(4);
            let len = 1 + r.below(4 - s);
            format!("COP(A[{s}],A[{d}],{len})")
        }
        _ => format!("XIC({})", r.pick(BOOL_IN)),
    }
}

fn gen_elems(r: &mut Rng, depth: u32, n: u64) -> String {
    let mut s = String::new();
    for _ in 0..n {
        if depth < 3 && r.chance(15) {
            let legs = 1 + r.below(3);
            s.push('[');
            for k in 0..legs {
                if k > 0 {
                    s.push(',');
                }
                let m = r.below(3);
                s.push_str(&gen_elems(r, depth + 1, m));
            }
            s.push(']');
        } else {
            s.push_str(&gen_instr(r));
        }
    }
    s
}

fn gen_rung(r: &mut Rng, jsr: bool) -> String {
    let n = 1 + r.below(5);
    let mut s = gen_elems(r, 0, n);
    if jsr {
        s.push_str("JSR(Sub,0)");
    }
    s.push(';');
    s
}

fn l5x(main: &[String], sub: &[String], init: &BTreeMap<&str, String>) -> String {
    let mut tags = String::new();
    for (name, k, dims, input) in TAGS {
        let ty = format!("{k:?}").to_ascii_uppercase();
        let d = if *dims > 0 {
            format!(" Dimensions=\"{dims}\"")
        } else {
            String::new()
        };
        let usage = if *input { " Usage=\"Input\"" } else { "" };
        let data = init.get(name).cloned().unwrap_or_default();
        let _ = writeln!(
            tags,
            "<Tag Name=\"{name}\" TagType=\"Base\" DataType=\"{ty}\"{d}{usage} Constant=\"false\">{data}</Tag>"
        );
    }
    let rungs = |rs: &[String]| {
        let mut out = String::new();
        for (k, t) in rs.iter().enumerate() {
            let _ = write!(
                out,
                "<Rung Number=\"{k}\" Type=\"N\"><Text><![CDATA[{t}]]></Text></Rung>"
            );
        }
        out
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<RSLogix5000Content SchemaRevision=\"1.0\" SoftwareRevision=\"33.00\" TargetName=\"Q\" TargetType=\"Controller\">\n<Controller Use=\"Target\" Name=\"Q\">\n<Programs><Program Name=\"P\" MainRoutineName=\"Main\">\n<Tags>\n{tags}</Tags>\n<Routines>\n<Routine Name=\"Main\" Type=\"RLL\"><RLLContent>{}</RLLContent></Routine>\n<Routine Name=\"Sub\" Type=\"RLL\"><RLLContent>{}</RLLContent></Routine>\n</Routines></Program></Programs>\n<Tasks><Task Name=\"T\" Type=\"CONTINUOUS\"><ScheduledPrograms><ScheduledProgram Name=\"P\"/></ScheduledPrograms></Task></Tasks>\n</Controller>\n</RSLogix5000Content>\n",
        rungs(main),
        rungs(sub)
    )
}

// ---- the reference evaluator ----

#[derive(Clone, Copy, Debug)]
enum V {
    B(bool),
    I(i64),
    R(f32),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Num {
    SInt,
    Int,
    DInt,
    Real,
}

#[derive(Debug, Clone, Copy, Default)]
struct Timer {
    pre: i32,
    acc: i32,
    en: bool,
    tt: bool,
    dn: bool,
    last: i32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Counter {
    pre: i32,
    acc: i32,
    cu: bool,
    cd: bool,
    dn: bool,
    ov: bool,
    un: bool,
}

#[derive(Debug, Clone, Default)]
struct Mem {
    bools: BTreeMap<String, bool>,
    ints: BTreeMap<String, (Num, i64)>,
    reals: BTreeMap<String, f32>,
    timers: BTreeMap<String, Timer>,
    counters: BTreeMap<String, Counter>,
}

#[derive(Debug)]
struct Fault;

fn wrap(v: i64, n: Num) -> i64 {
    match n {
        Num::SInt => i64::from(v as i8),
        Num::Int => i64::from(v as i16),
        _ => i64::from(v as i32),
    }
}

/// REAL to an integer: nearest, ties to even, low bits kept (RM003 p.853).
fn r2i(x: f32, n: Num) -> i64 {
    let y = f64::from(x);
    if y.is_nan() || y.abs() >= 9.0e18 {
        return 0;
    }
    wrap(y.round_ties_even() as i64, n)
}

struct Eval<'a> {
    m: Mem,
    main: &'a [rung::Rung],
    sub: &'a [rung::Rung],
    now: i32,
}

impl Eval<'_> {
    fn split(p: &Path) -> (String, Option<String>, Option<u32>) {
        // base[idx] , member , bit
        let mut base = p.base.to_ascii_uppercase();
        let mut member = None;
        let mut bit = None;
        for s in &p.segs {
            match s {
                Seg::Index(ix) => {
                    let Index::Const(n) = ix[0] else {
                        panic!("dynamic index")
                    };
                    let _ = write!(base, "[{n}]");
                }
                Seg::Member(m) => member = Some(m.to_ascii_uppercase()),
                Seg::Bit(b) => bit = Some(*b),
            }
        }
        (base, member, bit)
    }

    fn get_b(&self, s: &str) -> bool {
        let Ok(Operand::Path(p)) = rung::operand(s) else {
            panic!("{s}")
        };
        let (base, member, bit) = Self::split(&p);
        if let Some(b) = bit {
            let (_, v) = self.m.ints[&base];
            return (v >> b) & 1 == 1;
        }
        if let Some(m) = member {
            if let Some(t) = self.m.timers.get(&base) {
                return match m.as_str() {
                    "EN" => t.en,
                    "TT" => t.tt,
                    "DN" => t.dn,
                    _ => unreachable!(),
                };
            }
            let c = self.m.counters[&base];
            return match m.as_str() {
                "CU" => c.cu,
                "CD" => c.cd,
                "DN" => c.dn,
                "OV" => c.ov,
                "UN" => c.un,
                _ => unreachable!(),
            };
        }
        self.m.bools[&base]
    }

    fn set_b(&mut self, s: &str, v: bool) {
        let Ok(Operand::Path(p)) = rung::operand(s) else {
            panic!("{s}")
        };
        let (base, member, bit) = Self::split(&p);
        if let Some(b) = bit {
            let e = self.m.ints.get_mut(&base).unwrap();
            let mask = 1i64 << b;
            let raw = if v { e.1 | mask } else { e.1 & !mask };
            e.1 = wrap(raw, e.0);
            return;
        }
        if let Some(m) = member {
            let t = self.m.timers.get_mut(&base).unwrap();
            match m.as_str() {
                "DN" => t.dn = v,
                _ => unreachable!(),
            }
            return;
        }
        *self.m.bools.get_mut(&base).unwrap() = v;
    }

    fn get_n(&self, s: &str) -> (V, Num) {
        match rung::operand(s).unwrap() {
            Operand::Int(n) => (V::I(i64::from(n)), Num::DInt),
            Operand::Real(x) => (V::R(x), Num::Real),
            Operand::Path(p) => self.get_path_n(&p),
            Operand::Unknown => unreachable!(),
        }
    }

    fn get_path_n(&self, p: &Path) -> (V, Num) {
        let (base, member, _) = Self::split(p);
        if let Some(m) = member {
            if let Some(t) = self.m.timers.get(&base) {
                return (V::I(i64::from(if m == "PRE" { t.pre } else { t.acc })), Num::DInt);
            }
            let c = self.m.counters[&base];
            return (V::I(i64::from(if m == "PRE" { c.pre } else { c.acc })), Num::DInt);
        }
        if let Some(x) = self.m.reals.get(&base) {
            return (V::R(*x), Num::Real);
        }
        let (n, v) = self.m.ints[&base];
        (V::I(v), n)
    }

    fn dest_num(&self, s: &str) -> Num {
        self.get_n(s).1
    }

    /// Stores `v` (of type `from`) into `s`, by Logix's conversions.
    fn store(&mut self, s: &str, v: V) {
        let Ok(Operand::Path(p)) = rung::operand(s) else {
            panic!("{s}")
        };
        let (base, member, _) = Self::split(&p);
        let to = self.dest_num(s);
        let conv = |v: V| -> V {
            match (v, to) {
                (V::R(x), Num::Real) => V::R(x),
                (V::I(i), Num::Real) => V::R(i as f32),
                (V::R(x), n) => V::I(r2i(x, n)),
                (V::I(i), n) => V::I(wrap(i, n)),
                (V::B(_), _) => unreachable!(),
            }
        };
        let v = conv(v);
        if let Some(m) = member {
            let V::I(i) = v else { unreachable!() };
            let i = i32::try_from(i).unwrap();
            if let Some(t) = self.m.timers.get_mut(&base) {
                if m == "PRE" { t.pre = i } else { t.acc = i }
            } else {
                let c = self.m.counters.get_mut(&base).unwrap();
                if m == "PRE" { c.pre = i } else { c.acc = i }
            }
            return;
        }
        match v {
            V::R(x) => *self.m.reals.get_mut(&base).unwrap() = x,
            V::I(i) => self.m.ints.get_mut(&base).unwrap().1 = i,
            V::B(_) => unreachable!(),
        }
    }

    fn as_t(v: V, t: Num) -> V {
        match (v, t) {
            (V::I(i), Num::Real) => V::R(i as f32),
            (V::I(i), _) => V::I(i64::from(i as i32)),
            (v, _) => v,
        }
    }

    fn arith(op: Op, a: V, b: V) -> V {
        match (a, b) {
            (V::R(x), V::R(y)) => V::R(match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                Op::Mul => x * y,
                Op::Div => x / y,
                Op::Mod => {
                    if y == 0.0 {
                        0.0
                    } else {
                        let q = (x / y).trunc();
                        x - q * y
                    }
                }
            }),
            (V::I(x), V::I(y)) => {
                let (x, y) = (x as i32, y as i32);
                V::I(i64::from(match op {
                    Op::Add => x.wrapping_add(y),
                    Op::Sub => x.wrapping_sub(y),
                    Op::Mul => x.wrapping_mul(y),
                    // RM003 p.410: integer divide by zero gives Source A.
                    Op::Div => {
                        if y == 0 {
                            x
                        } else {
                            x.wrapping_div(y)
                        }
                    }
                    // RM003 p.416: MOD by zero gives 0.
                    Op::Mod => {
                        if y == 0 {
                            0
                        } else {
                            x.wrapping_rem(y)
                        }
                    }
                }))
            }
            _ => unreachable!(),
        }
    }

    fn cmp(a: (V, Num), b: (V, Num)) -> Option<std::cmp::Ordering> {
        let t = a.1.max(b.1).max(Num::DInt);
        match (Self::as_t(a.0, t), Self::as_t(b.0, t)) {
            (V::R(x), V::R(y)) => x.partial_cmp(&y),
            (V::I(x), V::I(y)) => Some(x.cmp(&y)),
            _ => unreachable!(),
        }
    }

    fn leaves(&self, e: &Expr, t: &mut Num) {
        match e {
            Expr::Real(_) => *t = Num::Real,
            Expr::Path(p) => {
                if self.get_path_n(p).1 == Num::Real {
                    *t = Num::Real;
                }
            }
            Expr::Int(_) => {}
            Expr::Neg(x) => self.leaves(x, t),
            Expr::Bin(_, x, y) => {
                self.leaves(x, t);
                self.leaves(y, t);
            }
        }
    }

    fn cpt(&self, e: &Expr, t: Num) -> V {
        match e {
            Expr::Int(n) => Self::as_t(V::I(i64::from(*n)), t),
            Expr::Real(x) => V::R(*x),
            Expr::Path(p) => Self::as_t(self.get_path_n(p).0, t),
            Expr::Neg(x) => match self.cpt(x, t) {
                V::R(r) => V::R(-r),
                V::I(i) => V::I(i64::from((i as i32).wrapping_neg())),
                V::B(_) => unreachable!(),
            },
            Expr::Bin(op, x, y) => Self::arith(*op, self.cpt(x, t), self.cpt(y, t)),
        }
    }

    // -- timers and counters, from RM003's flow charts --

    fn timer_true(t: &mut Timer, now: i32, tof: bool) -> Result<(), Fault> {
        if t.pre < 0 || t.acc < 0 {
            return Err(Fault);
        }
        t.tt = true;
        let stopped = if tof { !t.dn } else { t.dn };
        let first = if tof { t.en } else { !t.en };
        if stopped {
            t.tt = false;
            t.last = now;
        } else if first {
            t.last = now;
        } else {
            let delta = now.wrapping_sub(t.last);
            t.last = now;
            match t.acc.checked_add(delta) {
                None => {
                    t.acc = i32::MAX;
                    t.tt = false;
                    t.dn = !tof;
                }
                Some(a) => {
                    t.acc = a;
                    if a >= t.pre {
                        t.tt = false;
                        t.dn = !tof;
                    }
                }
            }
        }
        t.en = !tof;
        Ok(())
    }

    fn timer(&mut self, kind: &str, name: &str, rc: bool) -> Result<(), Fault> {
        let now = self.now;
        let t = self.m.timers.get_mut(&name.to_ascii_uppercase()).unwrap();
        match (kind, rc) {
            ("TON", false) => {
                t.en = false;
                t.tt = false;
                t.dn = false;
                t.acc = 0;
            }
            ("RTO", false) => {
                t.en = false;
                t.tt = false;
            }
            ("TOF", true) => {
                t.en = true;
                t.tt = false;
                t.dn = true;
                t.acc = 0;
            }
            ("TOF", false) => Self::timer_true(t, now, true)?,
            (_, true) => Self::timer_true(t, now, false)?,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn counter(&mut self, up: bool, name: &str, rc: bool) {
        let c = self.m.counters.get_mut(&name.to_ascii_uppercase()).unwrap();
        let edge = if up { &mut c.cu } else { &mut c.cd };
        if rc {
            if !*edge {
                *edge = true;
                let (v, wrapped) = if up {
                    c.acc.overflowing_add(1)
                } else {
                    c.acc.overflowing_sub(1)
                };
                c.acc = v;
                if wrapped {
                    let other = if up { c.un } else { c.ov };
                    if !other {
                        if up {
                            c.ov = true;
                        } else {
                            c.un = true;
                        }
                        return;
                    }
                    c.un = false;
                    c.ov = false;
                }
            }
        } else {
            *edge = false;
        }
        if !c.ov && !c.un {
            c.dn = c.acc >= c.pre;
        }
    }

    fn elems(&mut self, es: &[Elem], mut rc: bool) -> Result<bool, Fault> {
        for e in es {
            rc = match e {
                Elem::Branch(legs) => {
                    let mut out = false;
                    for leg in legs {
                        out |= self.elems(leg, rc)?;
                    }
                    out
                }
                Elem::Instr(i) => self.instr(&i.name, &i.args, rc)?,
            };
        }
        Ok(rc)
    }

    #[allow(clippy::too_many_lines)]
    fn instr(&mut self, name: &str, a: &[String], rc: bool) -> Result<bool, Fault> {
        use std::cmp::Ordering::{Equal, Greater, Less};
        Ok(match name {
            "XIC" => rc && self.get_b(&a[0]),
            "XIO" => rc && !self.get_b(&a[0]),
            "OTE" => {
                self.set_b(&a[0], rc);
                rc
            }
            "OTL" | "OTU" => {
                if rc {
                    self.set_b(&a[0], name == "OTL");
                }
                rc
            }
            "ONS" => {
                if rc {
                    if self.get_b(&a[0]) {
                        false
                    } else {
                        self.set_b(&a[0], true);
                        true
                    }
                } else {
                    self.set_b(&a[0], false);
                    false
                }
            }
            "OSR" => {
                if rc {
                    if self.get_b(&a[0]) {
                        self.set_b(&a[1], false);
                    } else {
                        self.set_b(&a[0], true);
                        self.set_b(&a[1], true);
                    }
                } else {
                    self.set_b(&a[0], false);
                    self.set_b(&a[1], false);
                }
                rc
            }
            "OSF" => {
                if rc {
                    self.set_b(&a[0], true);
                    self.set_b(&a[1], false);
                } else if self.get_b(&a[0]) {
                    self.set_b(&a[0], false);
                    self.set_b(&a[1], true);
                } else {
                    self.set_b(&a[1], false);
                }
                rc
            }
            "TON" | "TOF" | "RTO" => {
                self.timer(name, &a[0], rc)?;
                rc
            }
            "CTU" | "CTD" => {
                self.counter(name == "CTU", &a[0], rc);
                rc
            }
            "RES" => {
                if rc {
                    let n = a[0].to_ascii_uppercase();
                    if let Some(t) = self.m.timers.get_mut(&n) {
                        let pre = t.pre;
                        let last = t.last;
                        *t = Timer {
                            pre,
                            last,
                            ..Timer::default()
                        };
                    } else {
                        let c = self.m.counters.get_mut(&n).unwrap();
                        *c = Counter {
                            pre: c.pre,
                            ..Counter::default()
                        };
                    }
                }
                rc
            }
            "EQU" | "EQ" | "NEQ" | "NE" | "LES" | "LT" | "LEQ" | "LE" | "GRT" | "GT" | "GEQ" | "GE" => {
                let o = Self::cmp(self.get_n(&a[0]), self.get_n(&a[1]));
                rc && match name {
                    "EQU" | "EQ" => o == Some(Equal),
                    "NEQ" | "NE" => o != Some(Equal),
                    "LES" | "LT" => o == Some(Less),
                    "LEQ" | "LE" => matches!(o, Some(Less | Equal)),
                    "GRT" | "GT" => o == Some(Greater),
                    _ => matches!(o, Some(Greater | Equal)),
                }
            }
            "LIM" | "LIMIT" => {
                let (lo, x, hi) = (self.get_n(&a[0]), self.get_n(&a[1]), self.get_n(&a[2]));
                // RM003 pp.852-853: an integer operand mixed with a REAL one
                // in the same instruction is converted to REAL before the
                // instruction executes, so all three compare in the
                // instruction's type, not pairwise (a DINT Low and High one
                // apart can round to the same REAL).
                let t = lo.1.max(x.1).max(hi.1).max(Num::DInt);
                let (lo, x, hi) = ((lo.0, t), (x.0, t), (hi.0, t));
                let le = |p: (V, Num), q: (V, Num)| matches!(Self::cmp(p, q), Some(Less | Equal));
                let gt = |p: (V, Num), q: (V, Num)| Self::cmp(p, q) == Some(Greater);
                // RM003 p.368: low <= high: inside or on the limits; low >
                // high: on or outside them. NaN: false.
                let ok = if le(lo, hi) {
                    le(lo, x) && le(x, hi)
                } else if gt(lo, hi) {
                    matches!(Self::cmp(x, lo), Some(Greater | Equal)) || le(x, hi)
                } else {
                    false
                };
                rc && ok
            }
            "ADD" | "SUB" | "MUL" | "DIV" | "MOD" => {
                if rc {
                    let (x, y) = (self.get_n(&a[0]), self.get_n(&a[1]));
                    let t = x.1.max(y.1).max(self.dest_num(&a[2])).max(Num::DInt);
                    let op = match name {
                        "ADD" => Op::Add,
                        "SUB" => Op::Sub,
                        "MUL" => Op::Mul,
                        "DIV" => Op::Div,
                        _ => Op::Mod,
                    };
                    let v = Self::arith(op, Self::as_t(x.0, t), Self::as_t(y.0, t));
                    self.store(&a[2], v);
                }
                rc
            }
            "CPT" => {
                if rc {
                    let e = rung::expr(&a[1]).unwrap();
                    let mut t = self.dest_num(&a[0]).max(Num::DInt);
                    if t != Num::Real {
                        t = Num::DInt;
                    }
                    self.leaves(&e, &mut t);
                    let v = self.cpt(&e, t);
                    self.store(&a[0], v);
                }
                rc
            }
            "MOV" | "MOVE" => {
                if rc {
                    let v = self.get_n(&a[0]).0;
                    self.store(&a[1], v);
                }
                rc
            }
            "CLR" => {
                if rc {
                    self.store(&a[0], V::I(0));
                }
                rc
            }
            "COP" => {
                if rc {
                    let idx = |s: &str| -> usize { s[2..s.len() - 1].parse().unwrap() };
                    let (s0, d0) = (idx(&a[0]), idx(&a[1]));
                    let len: usize = a[2].parse().unwrap();
                    let n = len.min(4 - d0);
                    for k in 0..n {
                        let v = self.m.ints[&format!("A[{}]", s0 + k)].1;
                        self.m.ints.get_mut(&format!("A[{}]", d0 + k)).unwrap().1 = v;
                    }
                }
                rc
            }
            "JSR" => {
                if rc {
                    let sub = self.sub;
                    for r in sub {
                        self.elems(r, true)?;
                    }
                }
                rc
            }
            other => panic!("generator made {other}"),
        })
    }

    /// RM003's Prescan rows, each routine once.
    fn prescan(&mut self) {
        let mut sub_done = false;
        let main = self.main;
        for r in main {
            self.pre_elems(r, &mut sub_done);
        }
    }

    fn pre_elems(&mut self, es: &[Elem], sub_done: &mut bool) {
        for e in es {
            match e {
                Elem::Branch(legs) => {
                    for l in legs {
                        self.pre_elems(l, sub_done);
                    }
                }
                Elem::Instr(i) => {
                    let a = &i.args;
                    match i.name.as_str() {
                        "OTE" => self.set_b(&a[0], false),
                        "ONS" => self.set_b(&a[0], true),
                        "OSR" => {
                            self.set_b(&a[0], true);
                            self.set_b(&a[1], false);
                        }
                        "OSF" => {
                            self.set_b(&a[0], false);
                            self.set_b(&a[1], false);
                        }
                        "TON" | "TOF" | "RTO" => {
                            let t = self.m.timers.get_mut(&a[0].to_ascii_uppercase()).unwrap();
                            t.en = false;
                            t.tt = false;
                            if i.name != "RTO" {
                                t.dn = false;
                                t.acc = if i.name == "TOF" { t.pre } else { 0 };
                            }
                        }
                        "CTU" => self.m.counters.get_mut(&a[0].to_ascii_uppercase()).unwrap().cu = true,
                        "CTD" => self.m.counters.get_mut(&a[0].to_ascii_uppercase()).unwrap().cd = true,
                        "JSR" if !*sub_done => {
                            *sub_done = true;
                            let sub = self.sub;
                            for r in sub {
                                self.pre_elems(r, sub_done);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn scan(&mut self) -> Result<(), Fault> {
        let main = self.main;
        for r in main {
            self.elems(r, true)?;
        }
        Ok(())
    }
}

fn mem_init(init: &BTreeMap<&str, (i32, i32)>) -> Mem {
    let mut m = Mem::default();
    for (name, k, dims, _) in TAGS {
        let names: Vec<String> = if *dims > 0 {
            (0..*dims).map(|i| format!("{name}[{i}]")).collect()
        } else {
            vec![(*name).to_string()]
        };
        for n in names {
            match k {
                K::Bool => {
                    m.bools.insert(n, false);
                }
                K::SInt => {
                    m.ints.insert(n, (Num::SInt, 0));
                }
                K::Int => {
                    m.ints.insert(n, (Num::Int, 0));
                }
                K::DInt => {
                    m.ints.insert(n, (Num::DInt, 0));
                }
                K::Real => {
                    m.reals.insert(n, 0.0);
                }
                K::Timer => {
                    let (pre, acc) = init.get(name).copied().unwrap_or_default();
                    m.timers.insert(
                        n,
                        Timer {
                            pre,
                            acc,
                            ..Timer::default()
                        },
                    );
                }
                K::Counter => {
                    let (pre, acc) = init.get(name).copied().unwrap_or_default();
                    m.counters.insert(
                        n,
                        Counter {
                            pre,
                            acc,
                            ..Counter::default()
                        },
                    );
                }
            }
        }
    }
    m
}

fn same(a: Option<Value>, b: V) -> bool {
    match (a, b) {
        (Some(Value::Bool(x)), V::B(y)) => x == y,
        (Some(Value::Int(x)), V::I(y)) => x == i128::from(y),
        #[allow(clippy::cast_possible_truncation)]
        (Some(Value::Real(x)), V::R(y)) => (x as f32).to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        _ => false,
    }
}

fn compare(p: &helmsman_iec::Program, m: &Mem, scan: usize, st: &str) {
    let fail = |what: &str, got: Option<Value>, want: &dyn std::fmt::Debug| {
        panic!("scan {scan}: {what}: ST {got:?}, reference {want:?}\n{st}");
    };
    for (k, v) in &m.bools {
        if !same(p.get(k), V::B(*v)) {
            fail(k, p.get(k), v);
        }
    }
    for (k, (_, v)) in &m.ints {
        if !same(p.get(k), V::I(*v)) {
            fail(k, p.get(k), v);
        }
    }
    for (k, v) in &m.reals {
        if !same(p.get(k), V::R(*v)) {
            fail(k, p.get(k), v);
        }
    }
    for (k, t) in &m.timers {
        for (f, v) in [
            ("PRE", V::I(t.pre.into())),
            ("ACC", V::I(t.acc.into())),
            ("EN", V::B(t.en)),
            ("TT", V::B(t.tt)),
            ("DN", V::B(t.dn)),
        ] {
            let path = format!("{k}.{f}");
            if !same(p.get(&path), v) {
                fail(&path, p.get(&path), &v);
            }
        }
    }
    for (k, c) in &m.counters {
        for (f, v) in [
            ("PRE", V::I(c.pre.into())),
            ("ACC", V::I(c.acc.into())),
            ("CU", V::B(c.cu)),
            ("CD", V::B(c.cd)),
            ("DN", V::B(c.dn)),
            ("OV", V::B(c.ov)),
            ("UN", V::B(c.un)),
        ] {
            let path = format!("{k}.{f}");
            if !same(p.get(&path), v) {
                fail(&path, p.get(&path), &v);
            }
        }
    }
}

/// Runs one random program; returns the scans compared and whether it
/// ended on a fault both sides raised.
fn run_case(seed: u64) -> (usize, bool) {
    let mut r = Rng(seed | 1);
    let jsr = r.chance(40);
    let n_main = 1 + r.below(5);
    let main: Vec<String> = (0..n_main)
        .map(|k| gen_rung(&mut r, jsr && k + 1 == n_main))
        .collect();
    let sub: Vec<String> = (0..=r.below(3)).map(|_| gen_rung(&mut r, false)).collect();
    // Initial PRE/ACC of the timers and counters, through decorated data.
    let mut pre_acc: BTreeMap<&str, (i32, i32)> = BTreeMap::new();
    let mut data: BTreeMap<&str, String> = BTreeMap::new();
    for (name, timer) in [("T0", true), ("T1", true), ("C0", false), ("C1", false)] {
        let pre = if timer {
            r.below(60) as i32
        } else {
            r.below(5) as i32 - 1
        };
        let acc = if r.chance(10) { 2_147_483_646 } else { 0 };
        pre_acc.insert(name, (pre, acc));
        let ty = if timer { "TIMER" } else { "COUNTER" };
        data.insert(
            name,
            format!(
                "<Data Format=\"Decorated\"><Structure DataType=\"{ty}\"><DataValueMember Name=\"PRE\" DataType=\"DINT\" Value=\"{pre}\"/><DataValueMember Name=\"ACC\" DataType=\"DINT\" Value=\"{acc}\"/></Structure></Data>"
            ),
        );
    }
    let src = l5x(&main, &sub, &data);
    let conv = match helmsman_iec::ladder::convert(&src) {
        Ok(c) => c,
        Err(e) => panic!("refused: {e}\nmain: {main:?}\nsub: {sub:?}"),
    };
    let sizes = ImageSizes { i: 0, q: 0, m: 0 };
    let mut prog = helmsman_iec::compile(&conv.st, sizes).unwrap_or_else(|d| panic!("{d:?}\n{}", conv.st));
    let parsed = |rs: &[String]| -> Vec<rung::Rung> { rs.iter().map(|t| rung::parse(t).unwrap()).collect() };
    let (pm, ps) = (parsed(&main), parsed(&sub));
    let mut ev = Eval {
        m: mem_init(&pre_acc),
        main: &pm,
        sub: &ps,
        now: 0,
    };
    ev.prescan();
    let mut img = Image::new(sizes);
    let mut t_us: u64 = 1_000 + r.below(5_000);
    let t0 = t_us;
    for scan in 0..40 {
        // Inputs.
        for i in ["I0", "I1", "I2"] {
            let v = r.chance(50);
            prog.set_input(i, Value::Bool(v)).unwrap();
            ev.m.bools.insert(i.into(), v);
        }
        let n: i64 = match r.below(6) {
            0 => i64::from(i32::MAX),
            1 => i64::from(i32::MIN),
            _ => r.below(41) as i64 - 20,
        };
        prog.set_input("N0", Value::Int(i128::from(n))).unwrap();
        ev.m.ints.insert("N0".into(), (Num::DInt, n));
        let x: f32 = [0.0, 1.5, -2.5, 0.5, 3.0e9, -7.25][usize::try_from(r.below(6)).unwrap()];
        prog.set_input("R0", Value::Real(f64::from(x))).unwrap();
        ev.m.reals.insert("R0".into(), x);
        // Clock: whole ms since the first scan, as the ST's TON counts it.
        ev.now = i32::try_from((t_us - t0) / 1000).unwrap();
        let st = prog.scan_at(&mut img, Duration::from_micros(t_us));
        let rf = ev.scan();
        match (st, rf) {
            (Ok(()), Ok(())) => compare(&prog, &ev.m, scan, &conv.st),
            (Err(_), Err(Fault)) => return (scan, true),
            (s, f) => panic!("scan {scan}: ST {s:?}, reference {f:?}\n{}", conv.st),
        }
        t_us += match r.below(4) {
            0 => r.below(1000),
            1 => 1000 * r.below(20),
            _ => r.below(15_000),
        };
    }
    (40, false)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// Random rungs from the subset: the ST and the reference agree on
    /// every tag after every scan.
    #[test]
    fn converted_ladder_matches_the_reference(seed in any::<u64>()) {
        let _ = run_case(seed);
    }
}

#[test]
fn fixed_seeds_match_the_reference() {
    let (mut scans, mut faults) = (0, 0);
    for seed in 0..200u64 {
        let (n, f) = run_case(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        scans += n;
        faults += usize::from(f);
    }
    // Not vacuous: most programs run all their scans.
    assert!(scans > 200 * 30, "only {scans} scans compared");
    assert!(faults < 40, "{faults} of 200 programs faulted");
}
