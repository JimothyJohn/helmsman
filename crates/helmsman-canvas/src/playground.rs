//! The ST playground's state without a screen: the source being edited,
//! its compile, the program running in the page, the virtual I/O built
//! from the program's ports and located bits, and a short trend of
//! chosen signals. `playground_view` draws it; tests drive it directly.
//!
//! The program runs on `helmsman_iec::Program` itself (no runtime around
//! it): its `VAR_INPUT`s are set from the panel before every scan, its
//! `%IX` bits are written into the image, and the last scan of every
//! frame is traced for the diagram and "on because".

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::Duration;

use helmsman_diagram::Module;
use helmsman_iec::trace::{StmtId, Trace};
use helmsman_iec::{Diagnostic, Image, ImageSizes, PortDir, Program, Stopped, Ty, Value, compile_warned};

/// Process image: 8 bytes each way (`%IX0.0` .. `%IX7.7`), some memory.
pub const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };
/// Scan period bounds, ms.
#[cfg_attr(not(test), allow(dead_code))]
pub const MIN_PERIOD: u64 = 1;
#[cfg_attr(not(test), allow(dead_code))]
pub const MAX_PERIOD: u64 = 1000;
/// At most this many scans per frame (a slow tab catches up gradually).
pub const MAX_CATCH_UP: u64 = 500;
/// How far a Step looks for a change, ms of program time.
pub const STEP_MS: u64 = 60_000;

/// Where a simulation's run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Not running, nothing of a run kept: initial values, clock at zero.
    Stopped,
    Running,
    /// Not running, the state held (or a scan stopped at a breakpoint).
    Paused,
    Faulted,
}

impl RunState {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Faulted => "faulted",
        }
    }
}

/// What a Step compares between scans: which of the program's own
/// statements ran and which arms they took, and every value except a
/// TIME's. A timer's elapsed
/// time moves every scan while nothing else does; its output turning on
/// is the change.
#[must_use]
pub fn picture(t: &Trace) -> (Vec<bool>, Vec<Value>) {
    let map = t.map();
    let mut path = Vec::new();
    for (k, info) in map.stmts().iter().enumerate() {
        // The library's own statements (a TON's start branch) are not the
        // program's path.
        if info.system {
            continue;
        }
        let id = StmtId(u32::try_from(k).unwrap_or(u32::MAX));
        path.push(t.hits(id) > 0);
        path.extend(t.arm_hits(id).iter().map(|&n| n > 0));
    }
    let values = map
        .vars()
        .zip(t.values())
        .filter(|((_, ty), _)| *ty != Ty::Time)
        .map(|(_, (_, v))| v)
        .collect();
    (path, values)
}
/// How many scans before a frame's last are traced too
/// ([`Session::earlier`]): a frame's worth at the shortest period, and
/// never the whole of a long catch-up.
pub const EARLIER: u64 = 32;
/// The window a project opens with, ms of program time (the slider goes
/// from `playground_view::WINDOW_MIN_MS` to `WINDOW_MAX_MS`; Nick,
/// 2026-11-01: how fast the line crosses the pane). Both trends, the
/// box's too (Nick, 2026-11-07: 30 s).
pub const TREND_MS: u64 = 30_000;
/// The trend keeps this much history, ms of program time: the widest
/// window (10 min), so widening it shows what already ran.
pub const TREND_KEEP_MS: u64 = 600_000;
/// And at most this many samples (10 min at the 10 ms scan).
pub const TREND_CAP: usize = 60_000;
/// Points the whole history keeps, every signal together (about 10 MB):
/// ten signals that change every 10 ms scan for the whole 10 minutes.
/// Past it, the longest lane gives up its oldest points first.
pub const TREND_BUDGET: usize = 600_000;
/// Signals the trend draws at most.
pub const TREND_SIGNALS: usize = 6;
/// The editor refuses text past this: a project's total, so a program
/// the project accepts deploys whole.
pub const MAX_SOURCE: usize = helmsman_project::MAX_TOTAL;

/// A starter program.
#[derive(Clone, Copy, Debug)]
pub struct Example {
    pub name: &'static str,
    pub src: &'static str,
}

/// The example called `name` (the tour is built on "Blink", whichever
/// example opens first).
#[must_use]
pub fn example(name: &str) -> &'static Example {
    EXAMPLES
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no example named {name}"))
}

/// The examples (Nick, 2026-11-05): what the first chapters of a
/// Structured Text book teach, one idea each, a few lines long. Inputs are
/// `VAR_INPUT`s the I/O tab switches; outputs are `VAR_OUTPUT`s it shows.
/// The traffic light opens first (Nick, 2026-11-12): its three lamps are
/// the diagram's fixture (`lamps`), so the first screen shows a program
/// doing something anyone knows.
pub const EXAMPLES: [Example; 6] = [
    Example {
        name: "Traffic light",
        src: "PROGRAM traffic
VAR_INPUT
  run : BOOL := TRUE;
END_VAR
VAR_OUTPUT
  red : BOOL;
  yellow : BOOL;
  green : BOOL;
END_VAR
VAR
  phase : INT;  (* 0 green, 1 yellow, 2 red *)
  t : TON;
END_VAR
(* Green and red hold 3 s, yellow 1 s; the timer steps the phase. *)
t(IN := run AND NOT t.Q, PT := SEL(phase = 1, T#3s, T#1s));
IF t.Q THEN
  phase := (phase + 1) MOD 3;
END_IF;
green := run AND (phase = 0);
yellow := run AND (phase = 1);
red := NOT run OR (phase = 2);
END_PROGRAM
",
    },
    Example {
        name: "Blink",
        src: "PROGRAM blink
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
(* The timer restarts itself every 500 ms; each time, the lamp flips. *)
pulse(IN := NOT pulse.Q, PT := T#500ms);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
END_PROGRAM
",
    },
    Example {
        name: "Read and write",
        src: "PROGRAM read_write
VAR_INPUT
  switch : BOOL;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
(* Read an input, write an output: the lamp follows the switch. *)
lamp := switch;
END_PROGRAM
",
    },
    Example {
        name: "Logic: AND, OR, NOT",
        src: "PROGRAM logic
VAR_INPUT
  a : BOOL;
  b : BOOL;
END_VAR
VAR_OUTPUT
  both : BOOL;
  either : BOOL;
  neither : BOOL;
END_VAR
both := a AND b;
either := a OR b;
neither := NOT (a OR b);
END_PROGRAM
",
    },
    Example {
        name: "Start/stop latch",
        src: "PROGRAM latch
VAR_INPUT
  start : BOOL;
  stop : BOOL;
END_VAR
VAR_OUTPUT
  motor : BOOL;
END_VAR
(* Start turns the motor on and it stays on; stop turns it off. *)
motor := (start OR motor) AND NOT stop;
END_PROGRAM
",
    },
    Example {
        name: "Counter",
        src: "PROGRAM counter
VAR_INPUT
  pulse : BOOL;
  reset : BOOL;
END_VAR
VAR_OUTPUT
  count : INT;
  done : BOOL;
END_VAR
VAR
  c : CTU;
END_VAR
(* Counts rising edges of pulse; done at 10; reset starts over. *)
c(CU := pulse, R := reset, PV := 10);
count := c.CV;
done := c.Q;
END_PROGRAM
",
    },
];

/// Which way an I/O point faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

/// Where a point lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// A `VAR_INPUT` / `VAR_OUTPUT` of the program.
    Port,
    /// A `%IX` / `%QX` bit: byte and bit.
    Bit(usize, u8),
}

/// How the panel shows an input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Widget {
    /// BOOL, on while held.
    Push,
    /// BOOL, flips on a click.
    Toggle,
    /// An integer type, in its range.
    Int { min: i128, max: i128 },
    /// REAL or LREAL.
    Real,
    /// TIME, in ms.
    Time,
}

/// One point of the virtual I/O panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// The name shown and traced: the variable's, or the address for a
    /// direct address (`%QX0.1`). Upper case, as ST names are.
    pub name: String,
    pub dir: Dir,
    pub place: Place,
    pub ty: Ty,
    /// Inputs only.
    pub widget: Option<Widget>,
    /// An input: what the panel sets; an output: what the last scan left.
    pub value: Value,
}

impl Point {
    /// The address, when located.
    #[must_use]
    pub fn address(&self) -> Option<String> {
        match self.place {
            Place::Port => None,
            Place::Bit(byte, bit) => Some(format!(
                "%{}X{byte}.{bit}",
                if self.dir == Dir::In { 'I' } else { 'Q' }
            )),
        }
    }

    /// The value as a number for the trend (BOOL as 0/1, TIME in ms).
    #[must_use]
    pub fn number(&self) -> f64 {
        number(self.ty, self.value)
    }
}

#[allow(clippy::cast_precision_loss)]
fn number(ty: Ty, v: Value) -> f64 {
    match v {
        Value::Bool(b) => f64::from(u8::from(b)),
        Value::Int(n) if ty == Ty::Time => n as f64 / 1e6,
        Value::Int(n) => n as f64,
        Value::Real(r) => r,
    }
}

/// Names that read as momentary buttons.
fn momentary(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "start", "stop", "reset", "push", "pb", "button", "btn", "pulse", "ack", "trigger", "jog",
    ]
    .iter()
    .any(|w| n == *w || n.starts_with(&format!("{w}_")) || n.ends_with(&format!("_{w}")))
}

fn widget(name: &str, ty: Ty) -> Widget {
    match ty {
        Ty::Bool if momentary(name) => Widget::Push,
        Ty::Bool => Widget::Toggle,
        Ty::Time => Widget::Time,
        t if t.is_real() => Widget::Real,
        t => {
            let (min, max) = t.range();
            Widget::Int { min, max }
        }
    }
}

/// `%IX3.2` / `%QX0.0` (any case): area, byte, bit.
fn parse_bit(addr: &str) -> Option<(Dir, usize, u8)> {
    let a = addr.trim().to_ascii_uppercase();
    // The parser keeps an AT address without its '%'.
    let rest = a.strip_prefix('%').unwrap_or(&a);
    let (dir, rest) = match rest.split_at_checked(2)? {
        ("IX", r) => (Dir::In, r),
        ("QX", r) => (Dir::Out, r),
        _ => return None,
    };
    let (byte, bit) = rest.split_once('.')?;
    let (byte, bit): (usize, u8) = (byte.parse().ok()?, bit.parse().ok()?);
    (bit < 8 && byte < SIZES.i.min(SIZES.q)).then_some((dir, byte, bit))
}

/// Direct `%IXn.m` / `%QXn.m` addresses written in the code itself (not
/// in an `AT`), outside comments and strings, in order of first use.
fn direct_bits(src: &str) -> Vec<(Dir, usize, u8)> {
    let mut out = Vec::new();
    for d in helmsman_iec::ast::direct_addresses(src) {
        if let Some(p) = parse_bit(&d)
            && !out.contains(&p)
        {
            out.push(p);
        }
    }
    out
}

/// A good compile: the program, its diagram and its I/O.
pub struct Build {
    pub program: Program,
    pub image: Image,
    /// `None` when the diagram could not be drawn (the program still runs).
    pub module: Option<Module>,
    pub module_error: Option<String>,
    pub points: Vec<Point>,
    /// The source it was built from.
    pub src: String,
}

impl std::fmt::Debug for Build {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Build")
            .field("program", &self.program.name())
            .field("points", &self.points)
            .finish_non_exhaustive()
    }
}

impl Build {
    /// Compiles `src`: the build, or the compiler's diagnostics; and in
    /// both cases the checker's warnings.
    pub fn new(src: &str) -> (Result<Self, Vec<Diagnostic>>, Vec<Diagnostic>) {
        let (compiled, warnings) = compile_warned(src, SIZES);
        (compiled.map(|program| Self::from_program(src, program)), warnings)
    }

    fn from_program(src: &str, program: Program) -> Self {
        let (module, module_error) = match helmsman_diagram::lower(src, &program.trace_map()) {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut points = Vec::new();
        for p in program.ports() {
            let dir = if p.dir == PortDir::Input {
                Dir::In
            } else {
                Dir::Out
            };
            points.push(Point {
                widget: (dir == Dir::In).then(|| widget(&p.name, p.ty)),
                value: program.get(&p.name).unwrap_or(Value::zero(p.ty)),
                name: p.name,
                dir,
                place: Place::Port,
                ty: p.ty,
            });
        }
        // Located bits declared with AT, then direct addresses in the code.
        let mut bits: Vec<(Dir, usize, u8)> = Vec::new();
        if let Ok(unit) = helmsman_iec::ast::parse(src)
            && let Some(pou) = unit
                .pous
                .iter()
                .find(|p| p.kind == helmsman_iec::ast::PouKind::Program)
        {
            for v in &pou.vars {
                if let Some((at, _)) = &v.at
                    && let Some((dir, byte, bit)) = parse_bit(at)
                    && !bits.contains(&(dir, byte, bit))
                {
                    bits.push((dir, byte, bit));
                    let name = v.name.to_ascii_uppercase();
                    points.push(Point {
                        widget: (dir == Dir::In).then(|| widget(&name, Ty::Bool)),
                        name,
                        dir,
                        place: Place::Bit(byte, bit),
                        ty: Ty::Bool,
                        value: Value::Bool(false),
                    });
                }
            }
        }
        for (dir, byte, bit) in direct_bits(src) {
            if bits.contains(&(dir, byte, bit)) {
                continue;
            }
            bits.push((dir, byte, bit));
            let name = format!("%{}X{byte}.{bit}", if dir == Dir::In { 'I' } else { 'Q' });
            points.push(Point {
                widget: (dir == Dir::In).then_some(Widget::Toggle),
                name,
                dir,
                place: Place::Bit(byte, bit),
                ty: Ty::Bool,
                value: Value::Bool(false),
            });
        }
        Self {
            program,
            image: Image::new(SIZES),
            module,
            module_error,
            points,
            src: src.to_string(),
        }
    }
}

/// `breaks` of `old`'s program, as statements of `new`'s: the ones
/// whose POU, line and column are still a statement's. An edit that
/// moves a statement drops its breakpoint.
fn moved_breaks(old: &Build, new: &Build, breaks: &[StmtId]) -> Vec<StmtId> {
    let (was, now) = (old.program.trace_map(), new.program.trace_map());
    let mut out: Vec<StmtId> = breaks
        .iter()
        .filter_map(|&id| {
            let s = was.stmt(id)?;
            now.stmts()
                .iter()
                .position(|t| t.pou == s.pou && t.line == s.line && t.col == s.col)
                .and_then(|k| u32::try_from(k).ok())
                .map(StmtId)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The output points as the program and the image hold them now.
fn read_outputs(b: &mut Build) {
    for p in b.points.iter_mut().filter(|p| p.dir == Dir::Out) {
        p.value = match p.place {
            Place::Port => b.program.get(&p.name).unwrap_or(p.value),
            Place::Bit(byte, bit) => Value::Bool(b.image.q[byte] >> bit & 1 == 1),
        };
    }
}

/// What a deploy did not carry into the new program ([`Session::deploy`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Carried {
    /// Variables whose type changed: they start from their initial value.
    pub fresh: Vec<String>,
    /// Variables the new program no longer has: their values are gone.
    pub gone: Vec<String>,
}

impl Carried {
    /// A sentence for the notice, empty when everything was carried.
    #[must_use]
    pub fn describe(&self) -> String {
        let list = |v: &[String]| {
            v.iter()
                .map(|n| n.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut out = String::new();
        if !self.fresh.is_empty() {
            let _ = write!(out, " Started fresh, their type changed: {}.", list(&self.fresh));
        }
        if !self.gone.is_empty() {
            let _ = write!(out, " Not carried, no longer declared: {}.", list(&self.gone));
        }
        out
    }
}

/// The trend's history (Nick, 2026-11-13: "you should ideally be able to
/// capture everything all of the time"): every signal of the program,
/// pinned or not, kept from the first scan as its changes, each with its
/// program time. Pinning a signal only chooses which lanes are drawn, so
/// a new pin comes with its history and the others go on undisturbed.
/// Change-only keeps it cheap: a BOOL that flips every few seconds is a
/// few hundred points over [`TREND_KEEP_MS`], a value that moves every
/// scan costs what one lane always did ([`TREND_CAP`] points), and a scan
/// compares one value per signal and allocates only when one changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    /// One lane per signal, in the order the program's points came.
    lanes: Vec<Lane>,
    /// Program time of the newest scan recorded, ms.
    last_t: Option<u64>,
    /// Points held, every lane together (at most [`TREND_BUDGET`]).
    total: usize,
}

/// One signal's changes, oldest first.
#[derive(Clone, Debug, PartialEq)]
struct Lane {
    name: String,
    points: VecDeque<(u64, f64)>,
}

impl History {
    /// Whether nothing has been recorded.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.last_t.is_none()
    }

    pub fn clear(&mut self) {
        self.lanes.clear();
        self.last_t = None;
        self.total = 0;
    }

    /// Points held, every lane together.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Program time of the newest scan recorded, ms.
    #[must_use]
    pub const fn last_t(&self) -> Option<u64> {
        self.last_t
    }

    fn lane_of(&self, name: &str) -> Option<&Lane> {
        self.lanes.iter().find(|l| l.name == name)
    }

    /// `name`'s value at the newest scan.
    #[must_use]
    pub fn last_value(&self, name: &str) -> Option<f64> {
        self.lane_of(name)?.points.back().map(|&(_, v)| v)
    }

    /// How many changes of `name` are kept.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn changes(&self, name: &str) -> usize {
        self.lane_of(name).map_or(0, |l| l.points.len())
    }

    /// `name`'s history from `from_ms` to the newest scan, as a step
    /// function: the value it held at `from_ms` (dated `from_ms`), every
    /// change after, and the value it holds now dated the newest scan,
    /// so the line runs the whole window.
    pub fn lane(&self, name: &str, from_ms: u64) -> impl Iterator<Item = (u64, f64)> + '_ {
        let (points, end) = match (self.lane_of(name), self.last_t) {
            (Some(l), Some(end)) => (&l.points, end),
            _ => (&EMPTY, 0),
        };
        let first = points.partition_point(|&(t, _)| t <= from_ms);
        let held = first.checked_sub(1).map(|k| points[k]).map(|(_, v)| (from_ms, v));
        let changes = points.range(first..).copied();
        let now = points.back().filter(|&&(t, _)| t < end).map(|&(_, v)| (end, v));
        held.into_iter().chain(changes).chain(now)
    }

    /// One scan at program time `t_ms`: each point whose value differs
    /// from its lane's last is added; history past [`TREND_KEEP_MS`] goes,
    /// but a lane keeps the change that holds at the window's start.
    fn record(&mut self, t_ms: u64, points: &[Point]) {
        // A clock gone back is a new run (a cold deploy, a restart): its
        // history is not this one's.
        if self.last_t.is_some_and(|last| t_ms < last) {
            self.clear();
        }
        for (k, p) in points.iter().enumerate() {
            let v = p.number();
            // The program's order, so this is one comparison a point.
            let at = if self.lanes.get(k).is_some_and(|l| l.name == p.name) {
                k
            } else if let Some(at) = self.lanes.iter().position(|l| l.name == p.name) {
                at
            } else {
                self.lanes.push(Lane {
                    name: p.name.clone(),
                    points: VecDeque::new(),
                });
                self.lanes.len() - 1
            };
            let lane = &mut self.lanes[at];
            // NaN equals NaN here: an unreadable value is one state.
            if lane
                .points
                .back()
                .is_none_or(|&(_, last)| last.to_bits() != v.to_bits())
            {
                lane.points.push_back((t_ms, v));
                self.total += 1;
            }
            let keep_from = t_ms.saturating_sub(TREND_KEEP_MS);
            while lane.points.len() > TREND_CAP || (lane.points.len() > 1 && lane.points[1].0 <= keep_from) {
                lane.points.pop_front();
                self.total -= 1;
            }
        }
        self.last_t = Some(t_ms);
        // Over budget (rare: many signals that move every scan), the
        // longest lane gives up a tenth of itself, oldest first, its
        // newest point always kept.
        while self.total > TREND_BUDGET {
            let Some(lane) = self.lanes.iter_mut().max_by_key(|l| l.points.len()) else {
                break;
            };
            let drop = (lane.points.len() / 10).clamp(1, lane.points.len().saturating_sub(1));
            if lane.points.len() < 2 {
                break;
            }
            lane.points.drain(..drop);
            self.total -= drop;
        }
    }
}

#[cfg(test)]
impl History {
    /// [`Self::record`], for the property test.
    pub fn record_for_test(&mut self, t_ms: u64, points: &[Point]) {
        self.record(t_ms, points);
    }
}

static EMPTY: VecDeque<(u64, f64)> = VecDeque::new();

/// The trend: which signals are drawn, every signal's history, and how
/// much of it the pane shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    pub signals: Vec<String>,
    pub history: History,
    /// The width of the window drawn, ms (`WINDOW_MIN_MS..=WINDOW_MAX_MS`).
    pub window_ms: u64,
}

impl Default for Trend {
    fn default() -> Self {
        Self {
            signals: Vec::new(),
            history: History::default(),
            window_ms: TREND_MS,
        }
    }
}

impl Trend {
    fn record(&mut self, t_ms: u64, points: &[Point]) {
        self.history.record(t_ms, points);
    }

    /// Adds or removes `name` (at most [`TREND_SIGNALS`]). The history
    /// is every signal's and stays: a new lane is drawn with its past,
    /// the others go on (Nick, 2026-11-13: adding one reset them all).
    pub fn toggle(&mut self, name: &str) {
        if let Some(k) = self.signals.iter().position(|s| s == name) {
            self.signals.remove(k);
        } else if self.signals.len() < TREND_SIGNALS {
            self.signals.push(name.to_string());
        }
    }

    /// Moves the signal in lane `from` to lane `to`, the others closing
    /// up. Out of range does nothing.
    pub fn move_signal(&mut self, from: usize, to: usize) {
        let n = self.signals.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let s = self.signals.remove(from);
        self.signals.insert(to, s);
    }
}

/// The axis a trend lane draws `lo..=hi` on: the values rounded out to
/// a step of 1, 2 or 5 times a power of ten, about four of them to the
/// lane, so a rising value climbs through a scale that holds still
/// between steps instead of being pinned to the top of a lane refitted
/// every frame (Nick, 2026-11-01: a count that grew jumped to the top
/// and the line beneath it flattened as the window filled). A constant
/// gets a span above it; nothing finite gets `0..=1`. Returns the
/// bounds and the step, for the labels.
#[must_use]
pub fn nice_range(lo: f64, hi: f64) -> (f64, f64, f64) {
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0, 0.5);
    }
    let (lo, hi) = if lo < hi { (lo, hi) } else { (hi, lo) };
    let hi = if hi > lo {
        hi
    } else {
        lo + (lo.abs() * 0.1).max(1.0)
    };
    let raw = (hi - lo) / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let unit = raw / mag;
    let step = if unit <= 1.0 {
        1.0
    } else if unit <= 2.0 {
        2.0
    } else if unit <= 5.0 {
        5.0
    } else {
        10.0
    } * mag;
    let lo = (lo / step).floor() * step;
    let hi = (hi / step).ceil() * step;
    (lo, hi, step)
}

/// A scale label for `v` on an axis of `step`: as many decimals as the
/// step has, no more.
#[must_use]
pub fn tick_label(v: f64, step: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let decimals = (-step.log10().floor()).max(0.0) as usize;
    format!("{v:.decimals$}")
}

/// The whole playground.
#[derive(Debug)]
pub struct Session {
    /// The editor's text.
    pub src: String,
    /// Diagnostics for `src` (empty: it compiles).
    pub diags: Vec<Diagnostic>,
    /// What the checker says of `src` that does not stop it (a RETAIN
    /// that is not kept yet), in source order; set with `diags` by every
    /// check and every deploy, errors or not.
    pub warnings: Vec<Diagnostic>,
    /// The program running: the last source deployed (or loaded). An
    /// edit only reaches it through [`Self::set_source`]; [`Self::check`]
    /// leaves it alone.
    pub build: Option<Build>,
    pub running: bool,
    pub period_ms: u64,
    /// Program clock, ms.
    pub now_ms: u64,
    pub scans: u64,
    /// The last scan's fault; the program stops on one.
    pub fault: Option<String>,
    /// The last traced scan, for the diagram.
    pub trace: Option<Trace>,
    /// The scans run since the frame before, ahead of [`Self::trace`],
    /// oldest first (the newest [`EARLIER`] of them); see
    /// [`Self::earlier`].
    earlier: Vec<Trace>,
    pub trend: Trend,
    /// Measured plant states (`crate::plant::substitutions`): the model's
    /// variable path and the input point that stands in for it. Before each
    /// scan the variable takes the point's value, so the model runs from
    /// the sensor instead of its own state.
    pub sensors: Vec<(String, String)>,
    /// Breakpoints (Shift F9): statements of the running build a scan stops
    /// before, sorted. A statement in a function block's body stops in
    /// every instance.
    pub breaks: Vec<StmtId>,
    /// The scan stopped at a breakpoint, not yet committed: how many
    /// breakpoint hits it passed first. [`Self::trace`] is that moment;
    /// the program and the image are still as the scan found them.
    pub paused: Option<u32>,
    /// Page time the clock last caught up to, s.
    last: Option<f64>,
    acc_ms: f64,
}

impl Session {
    /// `src` compiled (or not), stopped.
    #[must_use]
    pub fn new(src: &str) -> Self {
        let mut s = Self {
            src: String::new(),
            diags: Vec::new(),
            warnings: Vec::new(),
            build: None,
            running: false,
            period_ms: 10,
            now_ms: 0,
            scans: 0,
            fault: None,
            trace: None,
            earlier: Vec::new(),
            trend: Trend::default(),
            sensors: Vec::new(),
            breaks: Vec::new(),
            paused: None,
            last: None,
            acc_ms: 0.0,
        };
        s.set_source(src);
        s
    }

    /// Whether the program running is not what the editor shows (the
    /// editor's text was not deployed, does not compile, or was not
    /// compiled yet).
    #[must_use]
    pub fn stale(&self) -> bool {
        self.build.as_ref().is_none_or(|b| b.src != self.src)
    }

    /// Takes `src` as the editor's text. Over [`MAX_SOURCE`] it is kept
    /// whole (so line numbers stay the editor's) but not compiled: `diags`
    /// name the limit. Whether it may be compiled.
    fn take(&mut self, src: &str) -> bool {
        self.src = src.to_string();
        if src.len() <= MAX_SOURCE {
            return true;
        }
        self.warnings.clear();
        self.diags = vec![Diagnostic {
            pos: helmsman_iec::Pos { line: 1, col: 1 },
            msg: format!(
                "the program is {} bytes; at most {MAX_SOURCE} are compiled",
                src.len()
            ),
        }];
        false
    }

    /// Checks `src`: `diags` say whether it compiles. What runs is
    /// untouched either way, as on a box, where an edit runs only once
    /// deployed ([`Self::deploy`]).
    ///
    /// Returns whether it compiles.
    pub fn check(&mut self, src: &str) -> bool {
        if !self.take(src) {
            return false;
        }
        let (compiled, warnings) = compile_warned(&self.src, SIZES);
        self.warnings = warnings;
        match compiled {
            Ok(_) => {
                self.diags.clear();
                true
            }
            Err(d) => {
                self.diags = d;
                false
            }
        }
    }

    /// Compiles `src`. If it compiles, it replaces the program from a
    /// cold start, keeping the panel's input values where a point of the
    /// same name and type survives, and the trend's signals that still
    /// exist; `running` is unchanged. If not (or if it is over
    /// [`MAX_SOURCE`]), the previous program keeps running and `diags`
    /// say why.
    ///
    /// Returns whether it compiled.
    pub fn set_source(&mut self, src: &str) -> bool {
        self.install(src, false).is_some()
    }

    /// Deploys `src` as the box does: the new program replaces the old
    /// one with its retained state carried by name and type (timers,
    /// counters, `RETAIN`), the process image kept and the clock running.
    /// What could not be carried is in the answer. `None` when it does not
    /// compile: the previous program keeps running and `diags` say why.
    pub fn deploy(&mut self, src: &str) -> Option<Carried> {
        self.install(src, true)
    }

    fn install(&mut self, src: &str, hot: bool) -> Option<Carried> {
        if !self.take(src) {
            return None;
        }
        let (built, warnings) = Build::new(&self.src);
        self.warnings = warnings;
        match built {
            Ok(mut b) => {
                self.breaks = self
                    .build
                    .as_ref()
                    .map_or_else(Vec::new, |old| moved_breaks(old, &b, &self.breaks));
                self.paused = None;
                let mut carried = Carried::default();
                if let Some(old) = &self.build {
                    for p in b.points.iter_mut().filter(|p| p.dir == Dir::In) {
                        if let Some(o) = old
                            .points
                            .iter()
                            .find(|o| o.dir == Dir::In && o.name == p.name && o.ty == p.ty)
                        {
                            p.value = o.value;
                        }
                    }
                    if hot {
                        let state = old.program.state();
                        let c = b.program.restore(&state);
                        carried.fresh = c
                            .reset
                            .into_iter()
                            .filter(|n| state.names().any(|o| o == n))
                            .collect();
                        carried.gone = c.dropped;
                        b.image = old.image.clone();
                        read_outputs(&mut b);
                    }
                }
                let names: Vec<String> = b.points.iter().map(|p| p.name.clone()).collect();
                self.trend.signals.retain(|s| names.contains(s));
                if self.trend.signals.is_empty() {
                    // Outputs first, then inputs, up to the cap.
                    let outs = b.points.iter().filter(|p| p.dir == Dir::Out);
                    let ins = b.points.iter().filter(|p| p.dir == Dir::In);
                    self.trend.signals = outs.chain(ins).take(4).map(|p| p.name.clone()).collect();
                }
                self.diags.clear();
                let had = self.build.is_some();
                self.build = Some(b);
                self.fault = None;
                self.trace = None;
                self.earlier.clear();
                if !(hot && had) {
                    self.now_ms = 0;
                    self.scans = 0;
                    self.acc_ms = 0.0;
                }
                Some(carried)
            }
            Err(d) => {
                self.diags = d;
                None
            }
        }
    }

    /// Loads an example or Claude's program: compiled, stopped. Nothing
    /// new runs until the user presses Run.
    pub fn load(&mut self, src: &str) -> bool {
        self.running = false;
        self.set_source(src)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_period(&mut self, ms: u64) {
        self.period_ms = ms.clamp(MIN_PERIOD, MAX_PERIOD);
    }

    /// Sets an input point by name; ignored for outputs, unknown names,
    /// and values of the wrong kind or out of the type's range.
    pub fn set_input(&mut self, name: &str, v: Value) -> bool {
        self.try_input(name, v).is_ok()
    }

    /// [`Self::set_input`], with the reason it refused. A REAL is stored
    /// as the program holds it (a REAL is an `f32`), so the panel never
    /// shows a value the program does not have.
    ///
    /// # Errors
    /// No such input, or a value of the wrong kind or out of range.
    pub fn try_input(&mut self, name: &str, v: Value) -> Result<(), String> {
        let Some(b) = &mut self.build else {
            return Err("no program is running".into());
        };
        let Some(p) = b
            .points
            .iter_mut()
            .find(|p| p.dir == Dir::In && p.name.eq_ignore_ascii_case(name))
        else {
            return Err(format!("no input named {name}"));
        };
        let stored = match (&p.place, p.ty, v) {
            (Place::Port, _, _) => b
                .program
                .set_input(&p.name, v)
                .ok()
                .and_then(|()| b.program.get(&p.name)),
            (_, Ty::Bool, Value::Bool(_)) => Some(v),
            _ => None,
        };
        let Some(stored) = stored else {
            return Err(format!("{} refused {v}: not a {} value in range", p.name, p.ty));
        };
        p.value = stored;
        Ok(())
    }

    /// The value of a point by name.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn value(&self, name: &str) -> Option<Value> {
        self.build
            .as_ref()?
            .points
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.value)
    }

    /// Run or pause (Nick, 2026-11-11: a pause holds the state, a stop
    /// is [`Self::stop`]). Paused, every variable and output stays as the
    /// last scan left it and the clock stands. Starting clears a fault
    /// and restarts the program cold if it had faulted. Starting from a
    /// breakpoint goes on to the scan's next breakpoint hit, and runs on
    /// from there if there is none; pausing leaves a paused scan paused.
    pub fn set_running(&mut self, on: bool) {
        if on && self.fault.is_some() {
            self.restart();
        }
        self.last = None;
        if on
            && let Some(skip) = self.paused
            && self.scan_or_pause(skip + 1)
        {
            return;
        }
        self.running = on;
    }

    /// Stop: the program no longer runs and nothing of its run is left.
    /// Every variable is back at its initial value, the outputs are off,
    /// the clock and the trend are at zero and there is no scan to show,
    /// so the diagram and the panels read as with nothing running.
    pub fn stop(&mut self) {
        self.running = false;
        self.last = None;
        self.acc_ms = 0.0;
        self.restart();
        self.silence();
    }

    /// Where the run stands, for the label beside the run glyphs.
    #[must_use]
    pub const fn state(&self) -> RunState {
        if self.fault.is_some() {
            RunState::Faulted
        } else if self.running {
            RunState::Running
        } else if self.paused.is_some() || self.scans > 0 {
            RunState::Paused
        } else {
            RunState::Stopped
        }
    }

    /// Step (Nick, 2026-11-11: to the next change, not one scan on):
    /// scans until one differs from the scan shown in the path it took
    /// through the program or in a value ([`picture`]), and shows that
    /// one. A breakpoint or a fault on the way ends it there. With no
    /// scan shown yet it runs one. Returns whether it got somewhere: a
    /// program in which nothing changes within [`STEP_MS`] of its own
    /// time stops there.
    pub fn step_to_change(&mut self) -> bool {
        let before = self
            .trace
            .as_ref()
            .filter(|_| self.paused.is_none() && self.fault.is_none())
            .map(picture);
        self.step();
        let Some(before) = before else { return true };
        let most = (STEP_MS / self.period_ms.max(1)).max(1);
        for _ in 1..most {
            if self.fault.is_some() || self.paused.is_some() {
                return true;
            }
            if self.trace.as_ref().map(picture).as_ref() != Some(&before) {
                return true;
            }
            self.step();
        }
        self.fault.is_some()
            || self.paused.is_some()
            || self.trace.as_ref().map(picture).as_ref() != Some(&before)
    }

    /// Sets or clears a breakpoint on `id` (Shift F9). Clearing the one a
    /// scan is paused at leaves it paused there (F8 finishes the scan).
    pub fn toggle_break(&mut self, id: StmtId) {
        match self.breaks.binary_search(&id) {
            Ok(k) => {
                self.breaks.remove(k);
            }
            Err(k) => self.breaks.insert(k, id),
        }
    }

    /// The statement the paused scan stopped before.
    #[must_use]
    pub fn paused_at(&self) -> Option<StmtId> {
        self.paused?;
        self.trace.as_ref()?.paused_at()
    }

    /// Outputs off, as the box leaves them when a module stops or
    /// faults: every output point reads zero and the `%Q` image is
    /// cleared, until the next scan writes them again.
    fn silence(&mut self) {
        let Some(b) = &mut self.build else { return };
        b.image.q.fill(0);
        for p in b.points.iter_mut().filter(|p| p.dir == Dir::Out) {
            p.value = Value::zero(p.ty);
        }
    }

    /// Cold restart: initial values, clock to zero, trend cleared.
    pub fn restart(&mut self) {
        if let Some(b) = &mut self.build {
            b.program.reset();
            b.image = Image::new(SIZES);
            read_outputs(b);
        }
        self.now_ms = 0;
        self.scans = 0;
        self.fault = None;
        self.trace = None;
        self.earlier.clear();
        self.paused = None;
        self.trend.history.clear();
    }

    /// One scan, traced, whether running or not (Step). It stops at a
    /// breakpoint; from one, it runs the rest of the scan.
    pub fn step(&mut self) {
        self.earlier.clear();
        if self.fault.is_some() {
            self.restart();
        }
        if self.paused.is_some() {
            self.scan(true);
        } else {
            self.scan_or_pause(0);
        }
    }

    /// A scan that stops before its `skip + 1`-th breakpoint hit (the
    /// program untouched, the moment in [`Self::trace`]), or runs to its
    /// end, traced, when it has no such hit. Whether it stopped.
    fn scan_or_pause(&mut self, skip: u32) -> bool {
        if !self.breaks.is_empty() && self.peek(skip) {
            self.running = false;
            return true;
        }
        self.scan(true);
        false
    }

    /// Runs the coming scan as far as its `skip + 1`-th breakpoint hit;
    /// if it gets there, the moment is the trace and the scan is paused.
    fn peek(&mut self, skip: u32) -> bool {
        self.feed();
        let Some(b) = &self.build else { return false };
        let now = Duration::from_millis(self.now_ms);
        let (stopped, trace) = b.program.scan_to(&b.image, now, &self.breaks, skip);
        if !matches!(stopped, Stopped::Before(_)) {
            return false;
        }
        self.trace = Some(trace);
        self.paused = Some(skip);
        true
    }

    /// The scans this frame ran before the one in [`Self::trace`], oldest
    /// first. A frame is longer than a scan, so a statement that runs for
    /// a single scan (a one-shot's body) is in the last scan of a frame
    /// only some of the time: a diagram that looked at the last scan alone
    /// would light it on some of its runs and miss the rest. Empty while a
    /// scan is paused, which is then the only thing to show.
    #[must_use]
    pub fn earlier(&self) -> &[Trace] {
        match &self.trace {
            // Scans of the program the last one ran, never a build before.
            Some(last) if self.paused.is_none() => {
                let from = self
                    .earlier
                    .iter()
                    .rposition(|e| !std::ptr::eq(e.map(), last.map()))
                    .map_or(0, |k| k + 1);
                &self.earlier[from..]
            }
            _ => &[],
        }
    }

    /// Runs the scans due by page time `t` (s) at the scan period, the
    /// last one traced into [`Self::trace`] and the [`EARLIER`] before it
    /// into [`Self::earlier`]. Nothing while stopped.
    pub fn advance(&mut self, t: f64) {
        let last = self.last.replace(t).unwrap_or(t);
        if !self.running {
            self.earlier.clear();
            // Paused: the moment follows the inputs as they change. If it
            // no longer comes, the scan stays paused where it was shown.
            if let Some(skip) = self.paused {
                self.peek(skip);
            }
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let period = self.period_ms as f64;
        self.acc_ms += ((t - last) * 1000.0).clamp(0.0, 1000.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let due = ((self.acc_ms / period) as u64).min(MAX_CATCH_UP);
        #[allow(clippy::cast_precision_loss)]
        {
            self.acc_ms = (self.acc_ms - due as f64 * period).max(0.0);
        }
        if due == 0 && self.trace.is_none() {
            // Something to draw from the first frame.
            self.scan_or_pause(0);
            return;
        }
        if due > 0 {
            self.earlier.clear();
        }
        for k in 0..due {
            if !self.breaks.is_empty() && self.peek(0) {
                self.running = false;
                return;
            }
            let left = due - k - 1;
            self.scan(left <= EARLIER);
            if self.fault.is_some() {
                break;
            }
            // Not the frame's last: kept beside it. A faulted scan stays
            // the one shown.
            if left > 0
                && left <= EARLIER
                && let Some(t) = self.trace.take()
            {
                self.earlier.push(t);
            }
        }
    }

    /// Puts the panel's inputs and the measured plant states into the
    /// program and its image, as the coming scan reads them.
    fn feed(&mut self) {
        let Some(b) = &mut self.build else { return };
        for p in b.points.iter().filter(|p| p.dir == Dir::In) {
            match p.place {
                Place::Port => {
                    // Values were range-checked when set.
                    let _ = b.program.set_input(&p.name, p.value);
                }
                Place::Bit(byte, bit) => {
                    let on = p.value == Value::Bool(true);
                    let m = 1u8 << bit;
                    b.image.i[byte] = if on {
                        b.image.i[byte] | m
                    } else {
                        b.image.i[byte] & !m
                    };
                }
            }
        }
        for (path, signal) in &self.sensors {
            let Some(p) = b
                .points
                .iter()
                .find(|p| p.dir == Dir::In && p.name.eq_ignore_ascii_case(signal))
            else {
                continue;
            };
            #[allow(clippy::cast_precision_loss)]
            let v = match (p.value, b.program.ty(path)) {
                (Value::Int(n), Some(t)) if t.is_real() => Value::Real(n as f64),
                (v, _) => v,
            };
            // A kind or range the variable cannot hold leaves the model's own.
            let _ = b.program.set(path, v);
        }
    }

    /// Runs the coming scan to its end, committed: the program, the
    /// image, the outputs and the clock move on.
    fn scan(&mut self, traced: bool) {
        self.feed();
        self.paused = None;
        let Some(b) = &mut self.build else { return };
        let now = Duration::from_millis(self.now_ms);
        let r = if traced {
            let (r, trace) = b.program.scan_traced(&mut b.image, now);
            self.trace = Some(trace);
            r
        } else {
            b.program.scan_at(&mut b.image, now)
        };
        read_outputs(b);
        self.scans += 1;
        self.trend.record(self.now_ms, &b.points);
        self.now_ms += self.period_ms;
        if let Err(f) = r {
            self.fault = Some(f.to_string());
            self.running = false;
            self.silence();
        }
    }

    /// Diagnostics as `line:col: message`, for the panel and for Claude.
    #[must_use]
    pub fn diag_lines(&self) -> Vec<String> {
        self.diags.iter().map(ToString::to_string).collect()
    }

    /// The inputs, in panel order.
    pub fn inputs(&self) -> impl Iterator<Item = &Point> {
        self.points().filter(|p| p.dir == Dir::In)
    }

    /// The outputs, in panel order.
    pub fn outputs(&self) -> impl Iterator<Item = &Point> {
        self.points().filter(|p| p.dir == Dir::Out)
    }

    fn points(&self) -> impl Iterator<Item = &Point> {
        self.build.iter().flat_map(|b| b.points.iter())
    }
}
