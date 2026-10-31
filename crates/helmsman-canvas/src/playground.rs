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
use std::time::Duration;

use helmsman_diagram::Module;
use helmsman_iec::trace::Trace;
use helmsman_iec::{Diagnostic, Image, ImageSizes, PortDir, Program, Ty, Value, compile};

/// Process image: 8 bytes each way (`%IX0.0` .. `%IX7.7`), some memory.
pub const SIZES: ImageSizes = ImageSizes { i: 8, q: 8, m: 64 };
/// Scan period bounds, ms.
pub const MIN_PERIOD: u64 = 1;
pub const MAX_PERIOD: u64 = 1000;
/// At most this many scans per frame (a slow tab catches up gradually).
pub const MAX_CATCH_UP: u64 = 500;
/// The trend keeps this much history, ms of program time.
pub const TREND_MS: u64 = 10_000;
/// And at most this many samples.
pub const TREND_CAP: usize = 4000;
/// Signals the trend draws at most.
pub const TREND_SIGNALS: usize = 6;
/// The editor refuses text past this.
pub const MAX_SOURCE: usize = 64 * 1024;

/// A starter program.
#[derive(Clone, Copy, Debug)]
pub struct Example {
    pub name: &'static str,
    pub src: &'static str,
}

pub const EXAMPLES: [Example; 5] = [
    Example {
        name: "Blink (TON)",
        src: "PROGRAM blink
VAR_INPUT
  enable : BOOL := TRUE;
  period : TIME := T#500ms;
END_VAR
VAR_OUTPUT
  lamp : BOOL;
END_VAR
VAR
  pulse : TON;
END_VAR
(* A timer that restarts itself: Q is TRUE for one scan every period. *)
pulse(IN := enable AND NOT pulse.Q, PT := period);
IF pulse.Q THEN
  lamp := NOT lamp;
END_IF;
IF NOT enable THEN
  lamp := FALSE;
END_IF;
END_PROGRAM
",
    },
    Example {
        name: "Start/stop latch",
        src: "PROGRAM latch
VAR
  start AT %IX0.0 : BOOL;  (* push button *)
  stop AT %IX0.1 : BOOL;   (* push button *)
  motor AT %QX0.0 : BOOL;
END_VAR
(* Seal-in: start latches the motor on, stop wins. *)
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
  target : INT := 5;
END_VAR
VAR_OUTPUT
  count : INT;
  done : BOOL;
END_VAR
VAR
  parts : CTU;
END_VAR
(* Counts rising edges of pulse until it reaches target. *)
parts(CU := pulse, R := reset, PV := target);
count := parts.CV;
done := parts.Q;
END_PROGRAM
",
    },
    Example {
        name: "Traffic light",
        src: "PROGRAM traffic
VAR_INPUT
  run : BOOL := TRUE;
  green_time : TIME := T#3s;
END_VAR
VAR_OUTPUT
  red : BOOL;
  yellow : BOOL;
  green : BOOL;
END_VAR
VAR
  phase : INT;  (* 0 green, 1 yellow, 2 red *)
  dwell : TIME;
  t : TON;
END_VAR
CASE phase OF
  0: dwell := green_time;
  1: dwell := T#1s;
ELSE
  dwell := T#3s;
END_CASE;
t(IN := run AND NOT t.Q, PT := dwell);
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
        name: "Line of stations (nested blocks)",
        src: "(* A line of two stations, each driving two motors: open the
   instances in the tree on the left, or double-click a block. *)
FUNCTION_BLOCK motor
VAR_INPUT
  run : BOOL;
  fault : BOOL;
END_VAR
VAR_OUTPUT
  running : BOOL;
  tripped : BOOL;
END_VAR
VAR
  start_delay : TON;
END_VAR
start_delay(IN := run AND NOT tripped, PT := T#300ms);
IF fault THEN
  tripped := TRUE;
ELSIF NOT run THEN
  tripped := FALSE;
END_IF;
running := start_delay.Q AND NOT tripped;
END_FUNCTION_BLOCK

FUNCTION_BLOCK station
VAR_INPUT
  enable : BOOL;
  part_present : BOOL;
  jam : BOOL;
END_VAR
VAR_OUTPUT
  busy : BOOL;
  alarm : BOOL;
END_VAR
VAR
  infeed : motor;
  outfeed : motor;
END_VAR
infeed(run := enable AND NOT part_present, fault := jam);
outfeed(run := enable AND part_present, fault := FALSE);
busy := infeed.running OR outfeed.running;
alarm := infeed.tripped;
END_FUNCTION_BLOCK

PROGRAM line
VAR_INPUT
  enable : BOOL := TRUE;
  part_at_1 : BOOL;
  part_at_2 : BOOL;
  jam_1 : BOOL;
END_VAR
VAR_OUTPUT
  station1_busy : BOOL;
  station2_busy : BOOL;
  alarm : BOOL;
END_VAR
VAR
  s1 : station;
  s2 : station;
END_VAR
s1(enable := enable, part_present := part_at_1, jam := jam_1);
s2(enable := enable, part_present := part_at_2, jam := FALSE);
station1_busy := s1.busy;
station2_busy := s2.busy;
alarm := s1.alarm OR s2.alarm;
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
/// in an `AT`), outside comments, in order of first use.
fn direct_bits(src: &str) -> Vec<(Dir, usize, u8)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut k = 0;
    while k < b.len() {
        if b[k..].starts_with(b"(*") {
            k = src[k + 2..].find("*)").map_or(b.len(), |e| k + 2 + e + 2);
            continue;
        }
        if b[k..].starts_with(b"//") {
            k = src[k..].find('\n').map_or(b.len(), |e| k + e);
            continue;
        }
        if b[k] == b'%' {
            let end = src[k + 1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
                .map_or(b.len(), |e| k + 1 + e);
            if let Some(p) = parse_bit(&src[k..end])
                && !out.contains(&p)
            {
                out.push(p);
            }
            k = end;
            continue;
        }
        k += 1;
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

/// One trend sample: program time and each trended signal's value.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub t_ms: u64,
    pub values: Vec<f64>,
}

/// The trend: which signals, and their recent history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trend {
    pub signals: Vec<String>,
    pub samples: VecDeque<Sample>,
}

impl Trend {
    fn record(&mut self, t_ms: u64, points: &[Point]) {
        let values = self
            .signals
            .iter()
            .map(|s| {
                points
                    .iter()
                    .find(|p| &p.name == s)
                    .map_or(f64::NAN, Point::number)
            })
            .collect();
        self.samples.push_back(Sample { t_ms, values });
        while self.samples.len() > TREND_CAP
            || self
                .samples
                .front()
                .is_some_and(|f| t_ms.saturating_sub(f.t_ms) > TREND_MS)
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
}

/// The whole playground.
#[derive(Debug)]
pub struct Session {
    /// The editor's text.
    pub src: String,
    /// Diagnostics for `src` (empty: it compiles).
    pub diags: Vec<Diagnostic>,
    /// The program running: the last source that compiled.
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
            last: None,
            acc_ms: 0.0,
        };
        s.set_source(src);
        s
    }

    /// Whether the program running is not what the editor shows (the
    /// editor's text does not compile, or was not compiled yet).
    #[must_use]
    pub fn stale(&self) -> bool {
        self.build.as_ref().is_none_or(|b| b.src != self.src)
    }

    /// Compiles `src` (cut at [`MAX_SOURCE`]). If it compiles, it replaces
    /// the program from a cold start, keeping the panel's input values
    /// where a point of the same name and type survives, and the trend's
    /// signals that still exist; `running` is unchanged. If not, the
    /// previous program keeps running and `diags` say why.
    ///
    /// Returns whether it compiled.
    pub fn set_source(&mut self, src: &str) -> bool {
        let mut cut = MAX_SOURCE.min(src.len());
        while !src.is_char_boundary(cut) {
            cut -= 1;
        }
        self.src = src[..cut].to_string();
        match Build::new(&self.src) {
            Ok(mut b) => {
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
                }
                let names: Vec<String> = b.points.iter().map(|p| p.name.clone()).collect();
                self.trend.signals.retain(|s| names.contains(s));
                if self.trend.signals.is_empty() {
                    // Outputs first, then inputs, up to the cap.
                    let outs = b.points.iter().filter(|p| p.dir == Dir::Out);
                    let ins = b.points.iter().filter(|p| p.dir == Dir::In);
                    self.trend.signals = outs.chain(ins).take(4).map(|p| p.name.clone()).collect();
                }
                self.trend.samples.clear();
                self.diags.clear();
                self.build = Some(b);
                self.now_ms = 0;
                self.scans = 0;
                self.fault = None;
                self.trace = None;
                self.acc_ms = 0.0;
                true
            }
            Err(d) => {
                self.diags = d;
                false
            }
        }
    }

    /// Loads an example or Claude's program: compiled, stopped. Nothing
    /// new runs until the user presses Run.
    pub fn load(&mut self, src: &str) -> bool {
        self.running = false;
        self.set_source(src)
    }

    pub fn set_period(&mut self, ms: u64) {
        self.period_ms = ms.clamp(MIN_PERIOD, MAX_PERIOD);
    }

    /// Sets an input point by name; ignored for outputs, unknown names,
    /// and values of the wrong kind or out of the type's range.
    pub fn set_input(&mut self, name: &str, v: Value) -> bool {
        let Some(b) = &mut self.build else { return false };
        let Some(p) = b
            .points
            .iter_mut()
            .find(|p| p.dir == Dir::In && p.name.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        let ok = match (p.ty, v) {
            (Ty::Bool, Value::Bool(_)) => true,
            (t, Value::Real(x)) if t.is_real() => x.is_finite(),
            (t, Value::Int(n)) if t != Ty::Bool && !t.is_real() => {
                let (lo, hi) = t.range();
                (lo..=hi).contains(&n)
            }
            _ => false,
        };
        if ok {
            p.value = v;
        }
        ok
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
    /// if it had faulted.
    pub fn set_running(&mut self, on: bool) {
        if on && self.fault.is_some() {
            self.restart();
        }
        self.running = on;
        self.last = None;
    }

    /// Cold restart: initial values, clock to zero, trend cleared.
    pub fn restart(&mut self) {
        if let Some(b) = &mut self.build {
            b.program.reset();
            b.image = Image::new(SIZES);
        }
        self.now_ms = 0;
        self.scans = 0;
        self.fault = None;
        self.trace = None;
        self.trend.samples.clear();
    }

    /// One scan, traced, whether running or not (Step).
    pub fn step(&mut self) {
        if self.fault.is_some() {
            self.restart();
        }
        self.scan(true);
    }

    /// Runs the scans due by page time `t` (s) at the scan period, the
    /// last one traced. Nothing while stopped.
    pub fn advance(&mut self, t: f64) {
        let last = self.last.replace(t).unwrap_or(t);
        if !self.running {
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
            self.scan(true);
            return;
        }
        for k in 0..due {
            self.scan(k + 1 == due);
            if self.fault.is_some() {
                break;
            }
        }
    }

    fn scan(&mut self, traced: bool) {
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
        let now = Duration::from_millis(self.now_ms);
        let r = if traced {
            let (r, trace) = b.program.scan_traced(&mut b.image, now);
            self.trace = Some(trace);
            r
        } else {
            b.program.scan_at(&mut b.image, now)
        };
        for p in b.points.iter_mut().filter(|p| p.dir == Dir::Out) {
            p.value = match p.place {
                Place::Port => b.program.get(&p.name).unwrap_or(p.value),
                Place::Bit(byte, bit) => Value::Bool(b.image.q[byte] >> bit & 1 == 1),
            };
        }
        self.scans += 1;
        self.trend.record(self.now_ms, &b.points);
        self.now_ms += self.period_ms;
        if let Err(f) = r {
            self.fault = Some(f.to_string());
            self.running = false;
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
