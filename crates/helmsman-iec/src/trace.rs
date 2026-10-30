//! What one scan did, for a live view of the program: which statements ran
//! and how often, which arm each IF and CASE took, how many passes each
//! loop made, where a fault struck, and every variable's value when the
//! scan ended.
//!
//! Recording is opt-in per scan ([`crate::Program::scan_traced`]); a plain
//! scan runs the interpreter monomorphised over `()`, which records nothing
//! and compiles to the untraced interpreter. A host samples: it traces one
//! scan when a viewer asks, not every scan.
//!
//! Statements are identified by [`StmtId`], an index into
//! [`TraceMap::stmts`], in source order per POU, POUs in declaration order
//! (the standard library's first). Each carries its source position, so a
//! view built from the syntax tree finds its trace by `(line, col)`.
//!
//! Statements inside a `FUNCTION_BLOCK` or `FUNCTION` body are shared by
//! every instance and call: their counts are totals over all of them. Each
//! instance's own state is in the values (`timer.Q`, `sums[2].total`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::check::{AggKind, Checked, Loc, PouCode, St, StKind, Type};
use crate::interp::{Fault, Image, from_raw};
use crate::types::{Ty, Value};

/// A statement: an index into [`TraceMap::stmts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StmtId(pub u32);

/// What kind of statement, and how many arms a branch has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `x := e` (scalar or whole struct/array/instance).
    Assign,
    /// A function block or function called as a statement.
    Call,
    /// `arms` conditions (IF, then each ELSIF). Arm index `arms` is the
    /// ELSE, or nothing when there is no ELSE.
    If {
        arms: usize,
    },
    /// `arms` label lists. Arm index `arms` is the ELSE, or nothing.
    Case {
        arms: usize,
    },
    For,
    While,
    Repeat,
    Exit,
    Continue,
    Return,
}

impl Shape {
    const fn arms(self) -> Option<usize> {
        match self {
            Self::If { arms } | Self::Case { arms } => Some(arms),
            _ => None,
        }
    }

    /// Counters this statement owns: hits, then for a branch the last arm
    /// taken (+1) and a count per arm, for a loop its passes.
    const fn counters(self) -> u32 {
        match self {
            #[allow(clippy::cast_possible_truncation)]
            Self::If { arms } | Self::Case { arms } => 2 + arms as u32 + 1,
            Self::For | Self::While | Self::Repeat => 2,
            _ => 1,
        }
    }

    fn of(kind: &StKind) -> Self {
        match kind {
            StKind::Store(..) | StKind::Copy(..) => Self::Assign,
            StKind::Call(_) => Self::Call,
            StKind::If(arms, _) => Self::If { arms: arms.len() },
            StKind::Case(_, arms, _) => Self::Case { arms: arms.len() },
            StKind::For { .. } => Self::For,
            StKind::While(..) => Self::While,
            StKind::Repeat(..) => Self::Repeat,
            StKind::Exit => Self::Exit,
            StKind::Continue => Self::Continue,
            StKind::Return => Self::Return,
        }
    }
}

/// A statement's place in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StmtInfo {
    /// The POU whose body holds it, upper-cased.
    pub pou: String,
    /// From the built-in library (TON, CTU, ...): its position is in
    /// [`crate::LIBRARY`], not the user's source.
    pub system: bool,
    pub line: u32,
    pub col: u32,
    pub shape: Shape,
    /// The statement this one is nested in and the arm of it (0 for a
    /// loop body); `None` at the top of its POU's body.
    pub parent: Option<(StmtId, usize)>,
    counter: u32,
}

/// Where a variable's value lives.
#[derive(Debug, Clone, Copy)]
enum At {
    /// Slot within the program's frame.
    Frame(usize),
    Image(Loc),
}

#[derive(Debug, Clone)]
struct Leaf {
    path: String,
    ty: Ty,
    at: At,
}

/// The static half of a trace, shared by every trace of one program: the
/// statement table and the program's variables, flattened to scalars.
#[derive(Debug)]
pub struct TraceMap {
    stmts: Vec<StmtInfo>,
    leaves: Vec<Leaf>,
    by_path: HashMap<String, usize>,
}

impl TraceMap {
    /// Every statement, indexed by [`StmtId`].
    #[must_use]
    pub fn stmts(&self) -> &[StmtInfo] {
        &self.stmts
    }

    #[must_use]
    pub fn stmt(&self, id: StmtId) -> Option<&StmtInfo> {
        self.stmts.get(id.0 as usize)
    }

    /// The user's statements that start on `line` (not the library's).
    pub fn at_line(&self, line: u32) -> impl Iterator<Item = StmtId> + '_ {
        self.ids().filter(move |&id| {
            let s = &self.stmts[id.0 as usize];
            !s.system && s.line == line
        })
    }

    /// The user's statements of `pou` (case-insensitive), in source order.
    pub fn in_pou<'a>(&'a self, pou: &str) -> impl Iterator<Item = StmtId> + 'a {
        let pou = pou.to_ascii_uppercase();
        self.ids().filter(move |&id| {
            let s = &self.stmts[id.0 as usize];
            !s.system && s.pou == pou
        })
    }

    fn ids(&self) -> impl Iterator<Item = StmtId> {
        #[allow(clippy::cast_possible_truncation)]
        (0..self.stmts.len()).map(|i| StmtId(i as u32))
    }

    /// Every scalar variable of the program, by the path [`Trace::get`]
    /// takes (`COUNT`, `T.ET`, `GRID[1,2]`, `SUMS[0].TOTAL`), with its
    /// type (enumerations as DINT). Function block internals included.
    pub fn vars(&self) -> impl Iterator<Item = (&str, Ty)> {
        self.leaves.iter().map(|l| (l.path.as_str(), l.ty))
    }

    fn by_counter(&self, counter: u32) -> Option<StmtId> {
        let i = self
            .stmts
            .partition_point(|s| s.counter <= counter)
            .checked_sub(1)?;
        #[allow(clippy::cast_possible_truncation)]
        (self.stmts[i].counter == counter).then_some(StmtId(i as u32))
    }

    pub(crate) fn build(code: &Checked) -> Self {
        let mut stmts = Vec::new();
        for (p, pou) in code.pous.iter().enumerate() {
            walk(pou, p < code.library, &pou.body, None, &mut stmts);
        }
        let mut leaves = Vec::new();
        let prog = &code.pous[code.program];
        for v in &prog.vars {
            match v.loc {
                Some(loc) => {
                    if let Type::Elem(ty) = v.ty {
                        leaves.push(Leaf {
                            path: v.name.clone(),
                            ty,
                            at: At::Image(loc),
                        });
                    }
                }
                None => flatten(code, &v.name, v.ty, v.off, &mut leaves),
            }
        }
        let by_path = leaves
            .iter()
            .enumerate()
            .map(|(i, l)| (l.path.clone(), i))
            .collect();
        Self {
            stmts,
            leaves,
            by_path,
        }
    }
}

/// Numbers every statement's counters, in the order [`TraceMap::build`]
/// lists them. Returns how many counters a trace needs.
pub(crate) fn number(code: &mut Checked) -> u32 {
    fn go(body: &mut [St], next: &mut u32) {
        for s in body {
            s.tr = *next;
            *next += Shape::of(&s.kind).counters();
            match &mut s.kind {
                StKind::If(arms, other) => {
                    for (_, b) in arms {
                        go(b, next);
                    }
                    go(other, next);
                }
                StKind::Case(_, arms, other) => {
                    for (_, b) in arms {
                        go(b, next);
                    }
                    go(other, next);
                }
                StKind::For { body, .. } | StKind::While(_, body) | StKind::Repeat(body, _) => go(body, next),
                _ => {}
            }
        }
    }
    let mut next = 0;
    for pou in &mut code.pous {
        go(&mut pou.body, &mut next);
    }
    next
}

fn walk(pou: &PouCode, system: bool, body: &[St], parent: Option<(StmtId, usize)>, out: &mut Vec<StmtInfo>) {
    for s in body {
        #[allow(clippy::cast_possible_truncation)]
        let id = StmtId(out.len() as u32);
        out.push(StmtInfo {
            pou: pou.name.clone(),
            system,
            line: s.line,
            col: s.col,
            shape: Shape::of(&s.kind),
            parent,
            counter: s.tr,
        });
        match &s.kind {
            StKind::If(arms, other) => {
                for (k, (_, b)) in arms.iter().enumerate() {
                    walk(pou, system, b, Some((id, k)), out);
                }
                walk(pou, system, other, Some((id, arms.len())), out);
            }
            StKind::Case(_, arms, other) => {
                for (k, (_, b)) in arms.iter().enumerate() {
                    walk(pou, system, b, Some((id, k)), out);
                }
                walk(pou, system, other, Some((id, arms.len())), out);
            }
            StKind::For { body, .. } | StKind::While(_, body) | StKind::Repeat(body, _) => {
                walk(pou, system, body, Some((id, 0)), out);
            }
            _ => {}
        }
    }
}

/// Every scalar under `path` of type `ty` at frame slot `off`.
fn flatten(code: &Checked, path: &str, ty: Type, off: usize, out: &mut Vec<Leaf>) {
    let a = match ty {
        Type::Elem(ty) => {
            out.push(Leaf {
                path: path.to_string(),
                ty,
                at: At::Frame(off),
            });
            return;
        }
        Type::Enum(_) => {
            out.push(Leaf {
                path: path.to_string(),
                ty: Ty::DInt,
                at: At::Frame(off),
            });
            return;
        }
        Type::Agg(a) => a,
    };
    match &code.aggs[a].kind {
        AggKind::Struct { fields } => {
            for f in fields {
                flatten(code, &format!("{path}.{}", f.name), f.ty, off + f.off, out);
            }
        }
        AggKind::Fb(fb) => {
            for v in &code.pous[*fb].vars {
                flatten(code, &format!("{path}.{}", v.name), v.ty, off + v.off, out);
            }
        }
        AggKind::Array { elem, dims } => {
            let esize = match elem {
                Type::Agg(e) => code.aggs[*e].size,
                _ => 1,
            };
            let mut ix: Vec<i128> = dims.iter().map(|&(lo, _)| lo).collect();
            let mut slot = off;
            if dims.iter().any(|&(lo, hi)| lo > hi) {
                return;
            }
            loop {
                let index: Vec<String> = ix.iter().map(ToString::to_string).collect();
                flatten(code, &format!("{path}[{}]", index.join(",")), *elem, slot, out);
                slot += esize;
                // Row-major: the last index moves fastest.
                let mut k = dims.len();
                loop {
                    if k == 0 {
                        return;
                    }
                    k -= 1;
                    if ix[k] < dims[k].1 {
                        ix[k] += 1;
                        break;
                    }
                    ix[k] = dims[k].0;
                }
            }
        }
    }
}

const NONE: u32 = u32::MAX;

/// The traced scan's [`Tap`]: one counter run per statement.
pub(crate) struct Recorder {
    pub counts: Vec<u32>,
    /// Counter of the innermost statement running, [`NONE`] outside any.
    pub cur: u32,
}

impl Recorder {
    pub fn new(counters: u32) -> Self {
        Self {
            counts: vec![0; counters as usize],
            cur: NONE,
        }
    }

    fn bump(&mut self, at: usize) {
        if let Some(c) = self.counts.get_mut(at) {
            *c = c.saturating_add(1);
        }
    }
}

impl Recorder {
    /// Statement `tr` starts; returns the statement it runs inside of, to
    /// restore into `cur` when it completes. A statement that faults never
    /// completes, so `cur` is left on the innermost one.
    pub fn enter(&mut self, tr: u32) -> u32 {
        self.bump(tr as usize);
        std::mem::replace(&mut self.cur, tr)
    }

    /// IF or CASE `tr` took arm `k` (`k` = the number of arms: none held).
    pub fn arm(&mut self, tr: u32, k: usize) {
        let at = tr as usize;
        if let Some(last) = self.counts.get_mut(at + 1) {
            #[allow(clippy::cast_possible_truncation)]
            {
                *last = k as u32 + 1;
            }
        }
        self.bump(at + 2 + k);
    }

    /// Loop `tr` starts one more pass through its body.
    pub fn pass(&mut self, tr: u32) {
        self.bump(tr as usize + 1);
    }
}

/// One scan, as it ran. Cheap to move; cloning copies the counters and
/// the program's variables.
#[derive(Debug, Clone)]
pub struct Trace {
    map: Arc<TraceMap>,
    now: Duration,
    counts: Vec<u32>,
    frame: Vec<Value>,
    image: Image,
    fault: Option<Fault>,
    fault_at: Option<StmtId>,
}

impl Trace {
    pub(crate) fn new(
        map: Arc<TraceMap>,
        now: Duration,
        rec: Recorder,
        frame: Vec<Value>,
        image: Image,
        fault: Option<Fault>,
    ) -> Self {
        let fault_at = fault.and_then(|_| map.by_counter(rec.cur));
        Self {
            map,
            now,
            counts: rec.counts,
            frame,
            image,
            fault,
            fault_at,
        }
    }

    /// The statement table and variable list.
    #[must_use]
    pub fn map(&self) -> &TraceMap {
        &self.map
    }

    /// The scan's clock (as passed to [`crate::Program::scan_at`]).
    #[must_use]
    pub const fn now(&self) -> Duration {
        self.now
    }

    fn info(&self, id: StmtId) -> Option<&StmtInfo> {
        self.map.stmt(id)
    }

    /// Times the statement started this scan (0: it did not run). A
    /// statement in a loop body counts each pass; in a function block,
    /// each call of every instance.
    #[must_use]
    pub fn hits(&self, id: StmtId) -> u32 {
        self.info(id).map_or(0, |s| self.counts[s.counter as usize])
    }

    /// Whether the statement started at least once this scan.
    #[must_use]
    pub fn ran(&self, id: StmtId) -> bool {
        self.hits(id) > 0
    }

    /// The arm an IF or CASE took the last time it ran this scan (see
    /// [`Shape::If`]); `None` if it did not run, faulted while choosing,
    /// or is not a branch.
    #[must_use]
    pub fn arm(&self, id: StmtId) -> Option<usize> {
        let s = self.info(id)?;
        s.shape.arms()?;
        self.counts[s.counter as usize + 1]
            .checked_sub(1)
            .map(|k| k as usize)
    }

    /// How many times each arm of an IF or CASE was taken this scan, the
    /// ELSE (or none) last; empty for other statements.
    #[must_use]
    pub fn arm_hits(&self, id: StmtId) -> &[u32] {
        match self.info(id) {
            Some(s) => match s.shape.arms() {
                Some(arms) => &self.counts[s.counter as usize + 2..][..=arms],
                None => &[],
            },
            None => &[],
        }
    }

    /// Passes a FOR, WHILE or REPEAT made through its body this scan,
    /// summed over every time it ran; `None` for other statements.
    #[must_use]
    pub fn passes(&self, id: StmtId) -> Option<u32> {
        let s = self.info(id)?;
        matches!(s.shape, Shape::For | Shape::While | Shape::Repeat)
            .then(|| self.counts[s.counter as usize + 1])
    }

    /// The fault that ended the scan, if one did. Everything else in the
    /// trace is the scan up to that point.
    #[must_use]
    pub const fn fault(&self) -> Option<Fault> {
        self.fault
    }

    /// The innermost statement running when the scan faulted (a loop
    /// itself for a watchdog fault between passes).
    #[must_use]
    pub const fn faulted_at(&self) -> Option<StmtId> {
        self.fault_at
    }

    /// A scalar's value when the scan ended, by path (`count`, `t.ET`,
    /// `grid[1, 2]`, `p.x`; case-insensitive). Located variables read
    /// the image as the scan left it.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<Value> {
        let key: String = path
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let &i = self.map.by_path.get(&key)?;
        Some(self.value(&self.map.leaves[i]))
    }

    /// Every scalar of the program and its value when the scan ended, in
    /// declaration order.
    pub fn values(&self) -> impl Iterator<Item = (&str, Value)> {
        self.map.leaves.iter().map(|l| (l.path.as_str(), self.value(l)))
    }

    /// The module's process image as the scan left it.
    #[must_use]
    pub const fn image(&self) -> &Image {
        &self.image
    }

    fn value(&self, l: &Leaf) -> Value {
        match l.at {
            At::Frame(slot) => self.frame[slot],
            At::Image(loc) => from_raw(l.ty, self.image.read(loc)),
        }
    }
}
