//! The devices of a [`Spec`] in motion: each scan, [`Plant::step`] reads
//! the program's outputs, moves every device on by the scan's time and
//! says what each input it drives reads, through that input's mode.

use std::collections::BTreeMap;

use crate::text::{Device, Kind, Mode, Role, Spec};

/// A value crossing between the program and the plant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Val {
    Bool(bool),
    Num(f64),
}

impl Val {
    /// As a number: TRUE is 1.
    #[must_use]
    pub fn num(self) -> f64 {
        match self {
            Self::Bool(b) => f64::from(u8::from(b)),
            Self::Num(x) => x,
        }
    }

    /// As a BOOL: a number is TRUE from 0.5.
    #[must_use]
    pub fn on(self) -> bool {
        match self {
            Self::Bool(b) => b,
            Self::Num(x) => x >= 0.5,
        }
    }

    /// As a command from 0 to 1: a BOOL is all or nothing, a number is a
    /// percentage.
    fn command(self) -> f64 {
        match self {
            Self::Bool(b) => f64::from(u8::from(b)),
            Self::Num(x) => (x / 100.0).clamp(0.0, 1.0),
        }
    }
}

/// What one input reads after a step.
#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    /// The program variable, as the file names it.
    pub name: String,
    /// What it reads; `None` while absent: it holds its last value, as a
    /// box's input does while its link is down.
    pub value: Option<Val>,
    /// FALSE while absent: what its `NAME_ok` input reads.
    pub ok: bool,
}

/// A small seeded generator (`SplitMix64`), so a noisy run is the same run
/// every time.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let x = (self.next() >> 11) as f64;
        x / 9_007_199_254_740_992.0
    }
}

/// A device's state between steps.
#[derive(Debug, Clone, PartialEq)]
struct State {
    kind: Kind,
    /// Its own clock, ms (whole while it is not slowed, so a step at 3 s
    /// comes at 3 s): slowed with its device.
    clock_ms: f64,
    /// Its quantity: temperature, level, speed, position, a source's
    /// value, an eye's belt time in ms.
    x: f64,
    /// A walk's time to its next step, s.
    due: f64,
}

/// One input's state between steps.
#[derive(Debug, Clone, PartialEq, Default)]
struct Input {
    /// The mode it was in last step and when that began, ms.
    mode: Option<Mode>,
    since_ms: u64,
    /// What it stuck at.
    stuck: Option<Val>,
    /// The clean BOOL last step, for chatter.
    clean: Option<bool>,
    /// Chatter lasts until, ms.
    until_ms: u64,
    /// What it read last.
    last: Option<Val>,
}

/// The plant of a [`Spec`], running.
#[derive(Debug, Clone)]
pub struct Plant {
    spec: Spec,
    t_ms: u64,
    rng: Rng,
    states: Vec<State>,
    /// Modes set while running ([`Plant::set_mode`]), over the file's.
    modes: BTreeMap<String, (String, Mode)>,
    inputs: BTreeMap<String, Input>,
}

impl Plant {
    /// The devices of `spec` at their start.
    #[must_use]
    pub fn new(spec: Spec) -> Self {
        let states = spec.devices.iter().map(start).collect();
        let modes = spec
            .modes
            .iter()
            .map(|m| (m.input.to_ascii_uppercase(), (m.input.clone(), m.mode)))
            .collect();
        Self {
            rng: Rng(spec.seed),
            spec,
            t_ms: 0,
            states,
            modes,
            inputs: BTreeMap::new(),
        }
    }

    /// `spec` running on from where this plant is: a device of the same
    /// name and kind keeps its state, the clock runs on.
    #[must_use]
    pub fn carried(&self, spec: Spec) -> Self {
        let mut p = Self::new(spec);
        p.t_ms = self.t_ms;
        p.rng = self.rng.clone();
        for (d, s) in p.spec.devices.iter().zip(p.states.iter_mut()) {
            if let Some(i) = self
                .spec
                .devices
                .iter()
                .position(|o| o.name.eq_ignore_ascii_case(&d.name) && o.kind == d.kind)
            {
                *s = self.states[i].clone();
            }
        }
        p.inputs.clone_from(&self.inputs);
        p
    }

    /// The file this plant runs.
    #[must_use]
    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Time since the start, ms.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.t_ms
    }

    /// The mode `input` is in.
    #[must_use]
    pub fn mode(&self, input: &str) -> Mode {
        self.modes
            .get(&input.to_ascii_uppercase())
            .map_or(Mode::Normal, |(_, m)| *m)
    }

    /// Puts `input` in `mode` from the next step on (a requirement test's
    /// fault, the popup's choice), over what the file says.
    pub fn set_mode(&mut self, input: &str, mode: Mode) {
        self.modes
            .insert(input.to_ascii_uppercase(), (input.to_string(), mode));
    }

    /// The range `input` reads over, when the plant knows one: what
    /// [`Mode::Over`] and [`Mode::Under`] go past.
    #[must_use]
    pub fn range(&self, input: &str) -> Option<(f64, f64)> {
        let (d, key, role) = self.spec.owner(input)?;
        (role == Role::InNum).then(|| range_of(d, key))
    }

    /// Back to the start: every device as the file starts it, the clock at
    /// 0. Modes stay.
    pub fn reset(&mut self) {
        self.t_ms = 0;
        self.rng = Rng(self.spec.seed);
        self.states = self.spec.devices.iter().map(start).collect();
        self.inputs.clear();
    }

    /// Moves the plant on by `dt_ms` with the program's outputs (and the
    /// hand-set inputs of buttons, switches and selectors) as `read`
    /// gives them, and says what every input it drives reads now.
    pub fn step(&mut self, dt_ms: u64, read: &dyn Fn(&str) -> Option<Val>) -> Vec<Feed> {
        self.t_ms = self.t_ms.saturating_add(dt_ms);
        #[allow(clippy::cast_precision_loss)]
        let dt_ms = dt_ms as f64;
        // Devices first, then the sensors that read them.
        for pass in [false, true] {
            for i in 0..self.spec.devices.len() {
                let d = &self.spec.devices[i];
                if (d.kind == Kind::Sensor) != pass {
                    continue;
                }
                let slow = d
                    .inputs()
                    .filter_map(|(_, v, _)| match self.mode(v) {
                        Mode::Slow(f) => Some(f),
                        _ => None,
                    })
                    .fold(1.0_f64, f64::max);
                let mut s = self.states[i].clone();
                advance(d, &mut s, dt_ms / slow, read, &mut self.rng);
                self.states[i] = s;
            }
        }
        let mut clean: Vec<(String, Val)> = Vec::new();
        for (d, s) in self.spec.devices.iter().zip(&self.states) {
            for (key, var, role) in d.inputs() {
                let v = if d.kind.manual() {
                    read(var).unwrap_or(match role {
                        Role::InBool => Val::Bool(false),
                        _ => Val::Num(0.0),
                    })
                } else if d.kind == Kind::Sensor {
                    Val::Num(sensor(d, &self.spec, &self.states, read))
                } else {
                    port_value(d, s, key)
                };
                clean.push((var.to_string(), v));
            }
        }
        // An input with a mode and no device: set by hand, like a button.
        for (name, _) in self.modes.values() {
            if self.spec.owner(name).is_none()
                && let Some(v) = read(name)
            {
                clean.push((name.clone(), v));
            }
        }
        clean.into_iter().map(|(name, v)| self.through(name, v)).collect()
    }

    /// `v`, the clean value of `name`, through its mode.
    fn through(&mut self, name: String, v: Val) -> Feed {
        let mode = self.mode(&name);
        let range = self.range(&name);
        let key = name.to_ascii_uppercase();
        let t = self.t_ms;
        let st = self.inputs.entry(key).or_default();
        if st.mode != Some(mode) {
            st.mode = Some(mode);
            st.since_ms = t;
            st.stuck = None;
            st.until_ms = 0;
        }
        #[allow(clippy::cast_precision_loss)]
        let held = (t - st.since_ms) as f64 / 1000.0;
        let edge = matches!(v, Val::Bool(_)) && st.clean.is_some_and(|c| c != v.on());
        if let Val::Bool(b) = v {
            st.clean = Some(b);
        }
        let out = match mode {
            Mode::Normal | Mode::Slow(_) => Some(v),
            Mode::Stuck(at) => {
                let at = *st.stuck.get_or_insert(match (at, v) {
                    (Some(x), Val::Bool(_)) => Val::Bool(x >= 0.5),
                    (Some(x), Val::Num(_)) => Val::Num(x),
                    (None, _) => st.last.unwrap_or(v),
                });
                Some(at)
            }
            Mode::Drift(r) => Some(match v {
                Val::Num(x) => Val::Num(x + r * held),
                b @ Val::Bool(_) => b,
            }),
            Mode::Noise(a) => Some(match v {
                Val::Num(x) => Val::Num(x + a * (2.0 * self.rng.unit() - 1.0)),
                b @ Val::Bool(_) => b,
            }),
            Mode::Over | Mode::Under => Some(match (v, range) {
                (Val::Num(_), Some((lo, hi))) => Val::Num(if mode == Mode::Over {
                    hi + 0.1 * (hi - lo)
                } else {
                    lo - 0.1 * (hi - lo)
                }),
                (v, _) => v,
            }),
            Mode::Chatter(s) => Some(match v {
                Val::Bool(b) => {
                    if edge {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let ms = (s * 1000.0).round() as u64;
                        st.until_ms = t + ms;
                    }
                    if t < st.until_ms {
                        Val::Bool(self.rng.unit() < 0.5)
                    } else {
                        Val::Bool(b)
                    }
                }
                n @ Val::Num(_) => n,
            }),
            Mode::Absent => None,
        };
        let st = self.inputs.entry(name.to_ascii_uppercase()).or_default();
        if out.is_some() {
            st.last = out;
        }
        Feed {
            name,
            value: out,
            ok: mode != Mode::Absent,
        }
    }
}

fn start(d: &Device) -> State {
    let x = match d.kind {
        Kind::Heater => {
            let ambient = d.num("ambient", 20.0);
            d.num("start", ambient)
        }
        Kind::Tank => d.num("start", 0.0).clamp(0.0, d.num("max", 1.0).max(0.0)),
        Kind::Valve => d.num("start", 0.0).clamp(0.0, 100.0),
        Kind::Source => d.num("from", 0.0),
        _ => 0.0,
    };
    State {
        kind: d.kind,
        clock_ms: 0.0,
        x,
        due: d.num("every", 1.0),
    }
}

/// The command a device's output port carries, 0 to 1 (0 when unnamed).
fn command(d: &Device, key: &str, read: &dyn Fn(&str) -> Option<Val>) -> f64 {
    d.port(key).and_then(read).map_or(0.0, |v| finite(v.command()))
}

fn finite(x: f64) -> f64 {
    if x.is_finite() { x } else { 0.0 }
}

fn advance(d: &Device, s: &mut State, dt_ms: f64, read: &dyn Fn(&str) -> Option<Val>, rng: &mut Rng) {
    s.clock_ms += dt_ms;
    let dt = dt_ms / 1000.0;
    match d.kind {
        Kind::Heater => {
            let ambient = d.num("ambient", 20.0);
            let target = ambient + (d.num("max", 100.0) - ambient) * command(d, "heat", read);
            let tau = d.num("tau", 60.0);
            if tau <= 0.0 {
                s.x = target;
            } else {
                s.x += (target - s.x) * (1.0 - (-dt / tau).exp());
            }
        }
        Kind::Tank => {
            let rate = d.num("fill_rate", 0.1) * command(d, "fill", read)
                - d.num("drain_rate", 0.1) * command(d, "drain", read)
                - d.num("leak", 0.0);
            s.x = (s.x + rate * dt).clamp(0.0, d.num("max", 1.0).max(0.0));
        }
        Kind::Motor => {
            let rated = d.num("rated", 1500.0);
            let target = rated * command(d, "run", read);
            let ramp = d.num("ramp", 2.0);
            let step = if ramp <= 0.0 {
                f64::INFINITY
            } else {
                rated.abs() / ramp * dt
            };
            s.x = toward(s.x, target, step);
        }
        Kind::Valve => {
            let open = command(d, "open", read) >= 0.5;
            let target = if open {
                100.0
            } else if d.port("close").is_some() {
                if command(d, "close", read) >= 0.5 {
                    0.0
                } else {
                    s.x
                }
            } else {
                0.0
            };
            let travel = d.num("travel", 2.0);
            let step = if travel <= 0.0 {
                f64::INFINITY
            } else {
                100.0 / travel * dt
            };
            s.x = toward(s.x, target, step);
        }
        Kind::Eye => {
            let running = d.port("belt").is_none() || command(d, "belt", read) >= 0.5;
            if running {
                s.x += dt_ms;
            }
        }
        Kind::Source => source(d, s, dt, rng),
        Kind::Button | Kind::Switch | Kind::Selector | Kind::Sensor => {}
    }
}

fn toward(x: f64, target: f64, step: f64) -> f64 {
    if (target - x).abs() <= step {
        target
    } else {
        x + step.copysign(target - x)
    }
}

#[allow(clippy::many_single_char_names)]
fn source(d: &Device, s: &mut State, dt: f64, rng: &mut Rng) {
    let from = d.num("from", 0.0);
    let to = d.num("to", 100.0);
    let t = s.clock_ms / 1000.0;
    let (lo, hi) = (from.min(to), from.max(to));
    s.x = match d.get("shape").unwrap_or("ramp").to_ascii_lowercase().as_str() {
        "exp" => {
            let tau = d.num("tau", 10.0);
            if tau <= 0.0 {
                to
            } else {
                to + (from - to) * (-t / tau).exp()
            }
        }
        "geometric" => {
            let every = d.num("every", 1.0);
            let n = if every <= 0.0 { 0.0 } else { (t / every).floor() };
            // Capped at `to`, whichever way it grows; at most 1e12.
            let v = from * d.num("ratio", 2.0).powf(n.min(1e4));
            let v = if v.is_finite() { v } else { to };
            if from.abs() <= to.abs() {
                v.clamp(-to.abs(), to.abs())
            } else {
                v
            }
        }
        "pulse" => {
            let (on, off) = (d.num("on", 1.0), d.num("off", 1.0));
            let cycle = on + off;
            if cycle <= 0.0 || t % cycle < on { to } else { from }
        }
        "step" => {
            if t >= d.num("at", 1.0) {
                to
            } else {
                from
            }
        }
        "sine" => {
            let period = d.num("period", 10.0);
            let mid = f64::midpoint(from, to);
            if period <= 0.0 {
                mid
            } else {
                mid + (to - from) / 2.0 * (std::f64::consts::TAU * t / period).sin()
            }
        }
        "walk" => {
            let every = d.num("every", 1.0).max(0.001);
            s.due -= dt;
            let mut x = s.x;
            // At most a step a scan's worth of catching up, bounded.
            let mut n = 0;
            while s.due <= 0.0 && n < 10_000 {
                s.due += every;
                let dir = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
                x = (x + dir * d.num("step", 1.0)).clamp(lo, hi);
                n += 1;
            }
            if s.due <= 0.0 {
                s.due = every;
            }
            x
        }
        // A ramp.
        _ => {
            let over = d.num("over", 10.0);
            let repeat = d.get("repeat").is_some_and(|r| r.eq_ignore_ascii_case("yes"));
            let t = if repeat && over > 0.0 { t % over } else { t };
            if over <= 0.0 {
                to
            } else {
                from + (to - from) * (t / over).min(1.0)
            }
        }
    };
}

/// What a device's input `key` reads.
fn port_value(d: &Device, s: &State, key: &str) -> Val {
    match (d.kind, key) {
        (Kind::Tank, "high") => {
            let max = d.num("max", 1.0);
            Val::Bool(s.x >= d.num("high_at", 0.9 * max))
        }
        (Kind::Tank, "low") => {
            let max = d.num("max", 1.0);
            Val::Bool(s.x <= d.num("low_at", 0.1 * max))
        }
        (Kind::Motor, "running") => Val::Bool(s.x.abs() > 0.05 * d.num("rated", 1500.0).abs()),
        (Kind::Valve, "opened") => Val::Bool(s.x >= 100.0),
        (Kind::Valve, "closed") => Val::Bool(s.x <= 0.0),
        (Kind::Eye, _) => {
            let every = (d.num("every", 5.0) * 1000.0).round();
            let length = (d.num("length", 0.5) * 1000.0).round();
            Val::Bool(every > 0.0 && s.x % every >= every - length)
        }
        _ => Val::Num(s.x),
    }
}

/// The range a device's number input reads over.
fn range_of(d: &Device, key: &str) -> (f64, f64) {
    let ordered = |a: f64, b: f64| if a <= b { (a, b) } else { (b, a) };
    match d.kind {
        Kind::Heater => ordered(d.num("ambient", 20.0), d.num("max", 100.0)),
        Kind::Tank => (0.0, d.num("max", 1.0)),
        Kind::Motor => ordered(0.0, d.num("rated", 1500.0)),
        Kind::Valve => (0.0, 100.0),
        Kind::Selector => (0.0, (d.num("positions", 3.0) - 1.0).max(1.0)),
        Kind::Source => ordered(d.num("from", 0.0), d.num("to", 100.0)),
        Kind::Sensor => match d.get("signal").unwrap_or("4-20mA") {
            s if s.eq_ignore_ascii_case("0-10V") => (0.0, 10.0),
            s if s.eq_ignore_ascii_case("counts") => d.range("counts", (0.0, 32767.0)),
            _ => (4.0, 20.0),
        },
        _ => {
            let _ = key;
            (0.0, 1.0)
        }
    }
}

/// What a sensor's raw input reads: its quantity scaled from its range to
/// its signal, held inside the signal's span.
fn sensor(d: &Device, spec: &Spec, states: &[State], read: &dyn Fn(&str) -> Option<Val>) -> f64 {
    let of = d.get("of").unwrap_or("");
    let (dev, qty) = of.split_once('.').map_or((of, None), |(a, b)| (a, Some(b)));
    let eng = spec
        .devices
        .iter()
        .position(|o| o.name.eq_ignore_ascii_case(dev))
        .filter(|&i| {
            let k = spec.devices[i].kind;
            qty.is_none_or(|q| k.quantity().is_some_and(|w| w.eq_ignore_ascii_case(q)))
        })
        .map(|i| states[i].x)
        .or_else(|| read(of).map(Val::num))
        .unwrap_or(0.0);
    let (lo, hi) = d.range("range", (0.0, 100.0));
    let frac = finite((eng - lo) / (hi - lo)).clamp(0.0, 1.0);
    let (a, b) = range_of(d, "raw");
    let raw = a + (b - a) * frac;
    if d.get("signal").is_some_and(|s| s.eq_ignore_ascii_case("counts")) {
        raw.round()
    } else {
        raw
    }
}
