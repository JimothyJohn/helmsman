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
use helmsman_iec::{Diagnostic, Image, ImageSizes, PortDir, Program, Stopped, Ty, Value, compile};

/// Process image: 8 bytes each way (`%IX0.0` .. `%IX7.7`), some memory.
pub const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };
/// Scan period bounds, ms.
#[cfg_attr(not(test), allow(dead_code))]
pub const MIN_PERIOD: u64 = 1;
#[cfg_attr(not(test), allow(dead_code))]
pub const MAX_PERIOD: u64 = 1000;
/// At most this many scans per frame (a slow tab catches up gradually).
pub const MAX_CATCH_UP: u64 = 500;
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

/// The examples (Nick, 2026-11-05): what the first chapters of a
/// Structured Text book teach, one idea each, a few lines long. Inputs are
/// `VAR_INPUT`s the I/O tab switches; outputs are `VAR_OUTPUT`s it shows.
pub const EXAMPLES: [Example; 5] = [
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
    /// Compiles `src`.
    ///
    /// # Errors
    /// The compiler's diagnostics.
    pub fn new(src: &str) -> Result<Self, Vec<Diagnostic>> {
        let program = compile(src, SIZES)?;
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
        Ok(Self {
            program,
            image: Image::new(SIZES),
            module,
            module_error,
            points,
            src: src.to_string(),
        })
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

/// The trend's history: one time per scan and, for each, every trended
/// signal's value, in two flat buffers (a scan allocates nothing).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Samples {
    t_ms: VecDeque<u64>,
    /// Row after row, `width` values each.
    values: VecDeque<f64>,
    width: usize,
}

impl Samples {
    /// How many scans are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.t_ms.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.t_ms.is_empty()
    }

    pub fn clear(&mut self) {
        self.t_ms.clear();
        self.values.clear();
    }

    /// Program time of the oldest scan held, ms.
    #[must_use]
    pub fn first_t(&self) -> Option<u64> {
        self.t_ms.front().copied()
    }

    /// Program time of the newest scan held, ms.
    #[must_use]
    pub fn last_t(&self) -> Option<u64> {
        self.t_ms.back().copied()
    }

    /// Signal `k`'s value at scan `i`.
    #[must_use]
    pub fn value(&self, i: usize, k: usize) -> Option<f64> {
        (k < self.width).then(|| self.values.get(i * self.width + k).copied())?
    }

    /// Signal `k`'s newest value.
    #[must_use]
    pub fn last_value(&self, k: usize) -> Option<f64> {
        self.value(self.len().checked_sub(1)?, k)
    }

    /// Signal `k`'s history from time `from_ms` on: (time, value).
    pub fn lane(&self, k: usize, from_ms: u64) -> impl Iterator<Item = (u64, f64)> + '_ {
        let first = self.t_ms.partition_point(|&t| t < from_ms);
        (first..self.len()).filter_map(move |i| Some((self.t_ms[i], self.value(i, k)?)))
    }

    fn push(&mut self, t_ms: u64, values: impl ExactSizeIterator<Item = f64>) {
        if self.is_empty() {
            self.width = values.len();
        }
        self.t_ms.push_back(t_ms);
        self.values.extend(values);
    }

    fn pop_front(&mut self) {
        if self.t_ms.pop_front().is_some() {
            self.values.drain(..self.width.min(self.values.len()));
        }
    }

    /// Lane `from`'s column moves to `to` in every scan.
    fn move_lane(&mut self, from: usize, to: usize) {
        let w = self.width;
        if from >= w || to >= w {
            return;
        }
        for i in 0..self.len() {
            let row = i * w;
            let v = self.values[row + from];
            if from < to {
                for j in from..to {
                    self.values[row + j] = self.values[row + j + 1];
                }
            } else {
                for j in (to + 1..=from).rev() {
                    self.values[row + j] = self.values[row + j - 1];
                }
            }
            self.values[row + to] = v;
        }
    }
}

/// The trend: which signals, their recent history, and how much of it
/// the pane shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Trend {
    pub signals: Vec<String>,
    pub samples: Samples,
    /// The width of the window drawn, ms (`WINDOW_MIN_MS..=WINDOW_MAX_MS`).
    pub window_ms: u64,
}

impl Default for Trend {
    fn default() -> Self {
        Self {
            signals: Vec::new(),
            samples: Samples::default(),
            window_ms: TREND_MS,
        }
    }
}

impl Trend {
    fn record(&mut self, t_ms: u64, points: &[Point]) {
        let values = self.signals.iter().map(|s| {
            points
                .iter()
                .find(|p| &p.name == s)
                .map_or(f64::NAN, Point::number)
        });
        self.samples.push(t_ms, values);
        while self.samples.len() > TREND_CAP
            || self
                .samples
                .first_t()
                .is_some_and(|f| t_ms.saturating_sub(f) > TREND_KEEP_MS)
        {
            self.samples.pop_front();
        }
    }

    /// Adds or removes `name` (at most [`TREND_SIGNALS`]); history restarts.
    pub fn toggle(&mut self, name: &str) {
        if let Some(k) = self.signals.iter().position(|s| s == name) {
            self.signals.remove(k);
        } else if self.signals.len() < TREND_SIGNALS {
            self.signals.push(name.to_string());
        }
        self.samples.clear();
    }

    /// Moves the signal in lane `from` to lane `to`, the others closing
    /// up; the history moves with it. Out of range does nothing.
    pub fn move_signal(&mut self, from: usize, to: usize) {
        let n = self.signals.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let s = self.signals.remove(from);
        self.signals.insert(to, s);
        self.samples.move_lane(from, to);
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
    pub trend: Trend,
    /// Measured plant states (`crate::plant::substitutions`): the model's
    /// variable path and the input point that stands in for it. Before each
    /// scan the variable takes the point's value, so the model runs from
    /// the sensor instead of its own state.
    pub sensors: Vec<(String, String)>,
    /// Breakpoints (Ctrl B): statements of the running build a scan stops
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
            build: None,
            running: false,
            period_ms: 10,
            now_ms: 0,
            scans: 0,
            fault: None,
            trace: None,
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
        match compile(&self.src, SIZES) {
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
        match Build::new(&self.src) {
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
                let kept = if hot {
                    self.trend.signals.clone()
                } else {
                    Vec::new()
                };
                let names: Vec<String> = b.points.iter().map(|p| p.name.clone()).collect();
                self.trend.signals.retain(|s| names.contains(s));
                if self.trend.signals.is_empty() {
                    // Outputs first, then inputs, up to the cap.
                    let outs = b.points.iter().filter(|p| p.dir == Dir::Out);
                    let ins = b.points.iter().filter(|p| p.dir == Dir::In);
                    self.trend.signals = outs.chain(ins).take(4).map(|p| p.name.clone()).collect();
                }
                if self.trend.signals != kept {
                    self.trend.samples.clear();
                }
                self.diags.clear();
                let had = self.build.is_some();
                self.build = Some(b);
                self.fault = None;
                self.trace = None;
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

    /// Run or stop. Starting clears a fault and restarts the program cold
    /// if it had faulted. Starting from a breakpoint goes on to the
    /// scan's next breakpoint hit, and runs on from there if there is
    /// none; stopping leaves a paused scan paused.
    pub fn set_running(&mut self, on: bool) {
        if on && self.fault.is_some() {
            self.restart();
        }
        self.last = None;
        if !on && self.running {
            self.silence();
        }
        if on
            && let Some(skip) = self.paused
            && self.scan_or_pause(skip + 1)
        {
            return;
        }
        self.running = on;
    }

    /// Sets or clears a breakpoint on `id` (Ctrl B). Clearing the one a
    /// scan is paused at leaves it paused there (F10 finishes the scan).
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
        self.paused = None;
        self.trend.samples.clear();
    }

    /// One scan, traced, whether running or not (Step). It stops at a
    /// breakpoint; from one, it runs the rest of the scan.
    pub fn step(&mut self) {
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

    /// Runs the scans due by page time `t` (s) at the scan period, the
    /// last one traced. Nothing while stopped.
    pub fn advance(&mut self, t: f64) {
        let last = self.last.replace(t).unwrap_or(t);
        if !self.running {
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
        for k in 0..due {
            if !self.breaks.is_empty() && self.peek(0) {
                self.running = false;
                return;
            }
            self.scan(k + 1 == due);
            if self.fault.is_some() {
                break;
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
