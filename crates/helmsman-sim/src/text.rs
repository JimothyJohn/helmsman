//! The `plant.sim` file: one device a line, `KIND NAME key=value ...`, and
//! one `mode INPUT ...` line for each input that is not behaving normally.
//!
//! ```text
//! # The oven: heat_on drives temp_c toward 250 with a 60 s lag.
//! heater oven heat=heat_on temp=temp_c ambient=20 max=250 tau=60s
//! sensor pt1 of=oven.temp raw=temp_ma range=0..300 signal=4-20mA
//! mode temp_c stuck 80
//! seed 7
//! ```
//!
//! A key that names a program variable is a **port**: an output the device
//! reads (`heat=`) or an input it drives (`temp=`). The other keys are
//! settings, each with a default ([`Kind::keys`]). Times take `ms`, `s`,
//! `m` or `h` (a bare number is seconds), ranges are `LO..HI`. `#` starts
//! a comment. Nothing here touches I/O: the file is read, never run.

use std::fmt::Write as _;

/// Most lines a file may have.
pub const MAX_LINES: usize = 2000;
/// Longest name or value kept, in characters.
const MAX_WORD: usize = 64;

/// A kind of simulated device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A heater and the temperature it drives, a first-order lag.
    Heater,
    /// A tank: inflow and outflow, a level and its switches.
    Tank,
    /// A motor: speed ramps to the command, running feedback.
    Motor,
    /// A valve or cylinder: travel time, limit switches, position.
    Valve,
    /// A photo-eye on a conveyor: blocked as each part passes.
    Eye,
    /// A push button, on while held.
    Button,
    /// A maintained switch.
    Switch,
    /// A selector of a few positions.
    Selector,
    /// A free-running signal: ramp, exponential, geometric, pulse, step,
    /// sine or random walk.
    Source,
    /// An analog transmitter: a quantity scaled to 4-20 mA, 0-10 V or
    /// counts.
    Sensor,
}

/// What a key of a device line holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A program output the device reads (BOOL, or a number in percent).
    Out,
    /// A BOOL input the device drives.
    InBool,
    /// A number input the device drives.
    InNum,
    /// A number setting.
    Number,
    /// A duration setting, held in seconds.
    Time,
    /// A `LO..HI` setting.
    Range,
    /// One of a few words.
    Word(&'static [&'static str]),
    /// Another device's quantity (`oven.temp`, `oven`) or a program
    /// output.
    Of,
}

impl Role {
    /// Whether the key names a program variable.
    #[must_use]
    pub fn is_port(self) -> bool {
        matches!(self, Self::Out | Self::InBool | Self::InNum)
    }

    /// Whether the key names a program input the device drives.
    #[must_use]
    pub fn is_input(self) -> bool {
        matches!(self, Self::InBool | Self::InNum)
    }
}

/// One key a kind takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub name: &'static str,
    pub role: Role,
    pub required: bool,
    /// The value when the line leaves it out (empty: none).
    pub default: &'static str,
    /// What it is, in a few words, for the popup.
    pub about: &'static str,
}

const fn key(name: &'static str, role: Role, default: &'static str, about: &'static str) -> Key {
    Key {
        name,
        role,
        required: false,
        default,
        about,
    }
}

const fn must(name: &'static str, role: Role, about: &'static str) -> Key {
    Key {
        name,
        role,
        required: true,
        default: "",
        about,
    }
}

/// The shapes of a [`Kind::Source`].
pub const SHAPES: &[&str] = &["ramp", "exp", "geometric", "pulse", "step", "sine", "walk"];
/// The signals of a [`Kind::Sensor`].
pub const SIGNALS: &[&str] = &["4-20mA", "0-10V", "counts"];
const YES_NO: &[&str] = &["no", "yes"];

const HEATER: &[Key] = &[
    must("heat", Role::Out, "output that heats (BOOL, or 0-100 %)"),
    must("temp", Role::InNum, "input that reads the temperature"),
    key("ambient", Role::Number, "20", "temperature with the heat off"),
    key("max", Role::Number, "100", "temperature at full heat"),
    key("tau", Role::Time, "60s", "time to 63 % of a change"),
    key("start", Role::Number, "", "temperature at the start (ambient)"),
];
const TANK: &[Key] = &[
    must("fill", Role::Out, "output that fills (BOOL, or 0-100 %)"),
    key("drain", Role::Out, "", "output that drains (BOOL, or 0-100 %)"),
    key("level", Role::InNum, "", "input that reads the level"),
    key("high", Role::InBool, "", "high level switch"),
    key("low", Role::InBool, "", "low level switch"),
    key("max", Role::Number, "1", "level when full"),
    key("start", Role::Number, "0", "level at the start"),
    key("fill_rate", Role::Number, "0.1", "level gained a second, filling"),
    key("drain_rate", Role::Number, "0.1", "level lost a second, draining"),
    key("leak", Role::Number, "0", "level lost a second, always"),
    key(
        "high_at",
        Role::Number,
        "",
        "level the high switch makes at (90 %)",
    ),
    key(
        "low_at",
        Role::Number,
        "",
        "level the low switch makes below (10 %)",
    ),
];
const MOTOR: &[Key] = &[
    must("run", Role::Out, "output that runs it (BOOL, or 0-100 % speed)"),
    key("speed", Role::InNum, "", "input that reads the speed"),
    key("running", Role::InBool, "", "running feedback"),
    key("rated", Role::Number, "1500", "speed at full command"),
    key("ramp", Role::Time, "2s", "time from stop to rated speed"),
];
const VALVE: &[Key] = &[
    must("open", Role::Out, "output that opens or extends it"),
    key(
        "close",
        Role::Out,
        "",
        "output that closes it (none: it springs closed)",
    ),
    key("opened", Role::InBool, "", "open limit switch"),
    key("closed", Role::InBool, "", "closed limit switch"),
    key(
        "position",
        Role::InNum,
        "",
        "input that reads its position, 0-100 %",
    ),
    key("travel", Role::Time, "2s", "time from closed to open"),
    key("start", Role::Number, "0", "position at the start, %"),
];
const EYE: &[Key] = &[
    must("seen", Role::InBool, "input that is on while a part blocks it"),
    key(
        "belt",
        Role::Out,
        "",
        "output that runs the conveyor (none: always)",
    ),
    key("every", Role::Time, "5s", "belt time between parts"),
    key("length", Role::Time, "0.5s", "belt time a part blocks the eye"),
];
const BUTTON: &[Key] = &[must("input", Role::InBool, "the button's input")];
const SWITCH: &[Key] = &[must("input", Role::InBool, "the switch's input")];
const SELECTOR: &[Key] = &[
    must("input", Role::InNum, "the selector's input"),
    key("positions", Role::Number, "3", "how many positions, from 0"),
];
const SOURCE: &[Key] = &[
    must("value", Role::InNum, "the input it drives"),
    key("shape", Role::Word(SHAPES), "ramp", "the signal's shape"),
    key("from", Role::Number, "0", "value at the start, or the low value"),
    key("to", Role::Number, "100", "value at the end, or the high value"),
    key("over", Role::Time, "10s", "ramp: time from `from` to `to`"),
    key("repeat", Role::Word(YES_NO), "no", "ramp: start over at the end"),
    key("tau", Role::Time, "10s", "exp: time to 63 % of the way"),
    key("ratio", Role::Number, "2", "geometric: factor each step"),
    key(
        "every",
        Role::Time,
        "1s",
        "geometric and walk: time between steps",
    ),
    key("on", Role::Time, "1s", "pulse: time high"),
    key("off", Role::Time, "1s", "pulse: time low"),
    key("at", Role::Time, "1s", "step: when it steps"),
    key("period", Role::Time, "10s", "sine: one cycle"),
    key("step", Role::Number, "1", "walk: size of a step"),
];
const SENSOR: &[Key] = &[
    must("of", Role::Of, "what it measures (oven.temp, or an output)"),
    must("raw", Role::InNum, "the input that reads the signal"),
    key("range", Role::Range, "0..100", "measured range, low..high"),
    key("signal", Role::Word(SIGNALS), "4-20mA", "what the input reads"),
    key("counts", Role::Range, "0..32767", "counts: the input's span"),
];

impl Kind {
    pub const ALL: [Self; 10] = [
        Self::Heater,
        Self::Tank,
        Self::Motor,
        Self::Valve,
        Self::Eye,
        Self::Button,
        Self::Switch,
        Self::Selector,
        Self::Source,
        Self::Sensor,
    ];

    /// The word that starts its line.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Heater => "heater",
            Self::Tank => "tank",
            Self::Motor => "motor",
            Self::Valve => "valve",
            Self::Eye => "eye",
            Self::Button => "button",
            Self::Switch => "switch",
            Self::Selector => "selector",
            Self::Source => "source",
            Self::Sensor => "sensor",
        }
    }

    /// The kind a line's first word names (`cylinder` is a valve).
    #[must_use]
    pub fn from_word(w: &str) -> Option<Self> {
        let w = w.to_ascii_lowercase();
        if w == "cylinder" {
            return Some(Self::Valve);
        }
        Self::ALL.into_iter().find(|k| k.word() == w)
    }

    /// The keys its line takes, ports first.
    #[must_use]
    pub fn keys(self) -> &'static [Key] {
        match self {
            Self::Heater => HEATER,
            Self::Tank => TANK,
            Self::Motor => MOTOR,
            Self::Valve => VALVE,
            Self::Eye => EYE,
            Self::Button => BUTTON,
            Self::Switch => SWITCH,
            Self::Selector => SELECTOR,
            Self::Source => SOURCE,
            Self::Sensor => SENSOR,
        }
    }

    /// One sentence on what it simulates.
    #[must_use]
    pub fn about(self) -> &'static str {
        match self {
            Self::Heater => {
                "A heater: the temperature rises toward max while it heats and falls back to ambient when it stops."
            }
            Self::Tank => {
                "A tank: the level rises while it fills, falls while it drains, with high and low switches."
            }
            Self::Motor => {
                "A motor: the speed ramps to the command and back to a stop, with running feedback."
            }
            Self::Valve => {
                "A valve or cylinder: it travels open and closed in its travel time, with limit switches."
            }
            Self::Eye => "A photo-eye on a conveyor: a part blocks it at regular belt intervals.",
            Self::Button => "A push button, on while held.",
            Self::Switch => "A maintained switch: click to flip.",
            Self::Selector => "A selector switch of a few positions.",
            Self::Source => {
                "A signal that runs by itself: ramp, exponential, geometric, pulse, step, sine or random walk."
            }
            Self::Sensor => "A transmitter: a measured quantity scaled to 4-20 mA, 0-10 V or counts.",
        }
    }

    /// Whether its input is set by hand (a button, a switch, a selector).
    #[must_use]
    pub fn manual(self) -> bool {
        matches!(self, Self::Button | Self::Switch | Self::Selector)
    }

    /// The quantity [`Kind::Sensor`]'s `of=NAME` reads when it names the
    /// device alone.
    #[must_use]
    pub fn quantity(self) -> Option<&'static str> {
        match self {
            Self::Heater => Some("temp"),
            Self::Tank => Some("level"),
            Self::Motor => Some("speed"),
            Self::Valve => Some("position"),
            Self::Source => Some("value"),
            _ => None,
        }
    }
}

/// One device line.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub kind: Kind,
    pub name: String,
    /// From 1.
    pub line: usize,
    /// The keys as written (keys lower case), each checked against its
    /// role.
    pub values: Vec<(String, String)>,
}

impl Device {
    /// The value of `key`, as written or its default; `None` when there
    /// is neither.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .or_else(|| {
                self.kind
                    .keys()
                    .iter()
                    .find(|k| k.name == key && !k.default.is_empty())
                    .map(|k| k.default)
            })
    }

    /// A number or time setting (time in seconds), or `fallback`.
    #[must_use]
    pub fn num(&self, key: &str, fallback: f64) -> f64 {
        self.get(key)
            .and_then(|v| number(v).or_else(|| seconds(v)))
            .unwrap_or(fallback)
    }

    /// A range setting, or `fallback`.
    #[must_use]
    pub fn range(&self, key: &str, fallback: (f64, f64)) -> (f64, f64) {
        self.get(key).and_then(range).unwrap_or(fallback)
    }

    /// The program variable a port names, if the line names one.
    #[must_use]
    pub fn port(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The program inputs it drives: (key, variable, role).
    pub fn inputs(&self) -> impl Iterator<Item = (&'static str, &str, Role)> + '_ {
        self.kind
            .keys()
            .iter()
            .filter(|k| k.role.is_input())
            .filter_map(|k| self.port(k.name).map(|v| (k.name, v, k.role)))
    }

    /// The line as the file writes it.
    #[must_use]
    pub fn to_line(&self) -> String {
        let mut s = format!("{} {}", self.kind.word(), self.name);
        for (k, v) in &self.values {
            let _ = write!(s, " {k}={v}");
        }
        s
    }
}

/// How an input behaves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Normal,
    /// Frozen: at the value given, else where it was when it stuck.
    Stuck(Option<f64>),
    /// An offset growing by this much a second.
    Drift(f64),
    /// Random noise of up to this much either way (seeded).
    Noise(f64),
    /// Past the top of its range.
    Over,
    /// Below the bottom of its range.
    Under,
    /// Its device responds this many times slower.
    Slow(f64),
    /// A BOOL that bounces for this many seconds on each change.
    Chatter(f64),
    /// The device is lost: the input holds its last value and its `_ok`
    /// reads FALSE, as on a box whose link is down.
    Absent,
}

impl Mode {
    /// The word of its `mode` line.
    #[must_use]
    pub fn word(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Stuck(_) => "stuck",
            Self::Drift(_) => "drift",
            Self::Noise(_) => "noise",
            Self::Over => "over",
            Self::Under => "under",
            Self::Slow(_) => "slow",
            Self::Chatter(_) => "chatter",
            Self::Absent => "absent",
        }
    }

    /// Whether this is one of the abnormal behaviours (not normal, not
    /// absent).
    #[must_use]
    pub fn abnormal(&self) -> bool {
        !matches!(self, Self::Normal | Self::Absent)
    }

    /// Reads `WORD [VALUE]`.
    ///
    /// # Errors
    /// An unknown word, or a value missing or not a number.
    pub fn parse(word: &str, value: Option<&str>) -> Result<Self, String> {
        let need = |what: &str| -> Result<f64, String> {
            value
                .and_then(number)
                .ok_or_else(|| format!("`{word}` needs {what}"))
        };
        let positive = |x: f64| {
            if x > 0.0 {
                Ok(x)
            } else {
                Err(format!("`{word}` needs a value above 0"))
            }
        };
        let m = match word.to_ascii_lowercase().as_str() {
            "normal" => Self::Normal,
            "stuck" => Self::Stuck(match value {
                None => None,
                Some(v) => Some(
                    bool_word(v)
                        .map(|b| if b { 1.0 } else { 0.0 })
                        .or_else(|| number(v))
                        .ok_or_else(|| format!("`stuck {v}`: a number, TRUE or FALSE"))?,
                ),
            }),
            "drift" => Self::Drift(need("a rate a second (`drift 0.5`)")?),
            "noise" => Self::Noise(positive(need("a size (`noise 2`)")?)?),
            "over" => Self::Over,
            "under" => Self::Under,
            "slow" => Self::Slow(positive(need("a factor (`slow 4`)")?)?),
            "chatter" => Self::Chatter(positive(
                value
                    .and_then(seconds)
                    .ok_or_else(|| format!("`{word}` needs a time (`chatter 50ms`)"))?,
            )?),
            "absent" => Self::Absent,
            w => {
                return Err(format!(
                    "`{w}` is no mode: normal, stuck, drift, noise, over, under, slow, chatter or absent"
                ));
            }
        };
        Ok(m)
    }

    /// Its `mode` line for `input`.
    #[must_use]
    pub fn to_line(&self, input: &str) -> String {
        let v = match self {
            Self::Stuck(Some(v)) | Self::Drift(v) | Self::Noise(v) | Self::Slow(v) => {
                format!(" {}", trim_num(*v))
            }
            Self::Chatter(s) => format!(" {}ms", trim_num(s * 1000.0)),
            _ => String::new(),
        };
        format!("mode {input} {}{v}", self.word())
    }
}

/// A `mode` line.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeLine {
    pub input: String,
    pub mode: Mode,
    pub line: usize,
}

/// What the file says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spec {
    pub devices: Vec<Device>,
    pub modes: Vec<ModeLine>,
    /// The noise and random walk's seed (`seed N`; 1 without one).
    pub seed: u64,
}

impl Spec {
    /// The device that drives `input`, and the key it drives it by.
    #[must_use]
    pub fn owner(&self, input: &str) -> Option<(&Device, &'static str, Role)> {
        self.devices.iter().find_map(|d| {
            d.inputs()
                .find(|(_, v, _)| v.eq_ignore_ascii_case(input))
                .map(|(k, _, r)| (d, k, r))
        })
    }

    /// The device named `name`.
    #[must_use]
    pub fn device(&self, name: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// The mode the file gives `input` (normal without a line).
    #[must_use]
    pub fn mode(&self, input: &str) -> Mode {
        self.modes
            .iter()
            .find(|m| m.input.eq_ignore_ascii_case(input))
            .map_or(Mode::Normal, |m| m.mode)
    }
}

/// A line the file could not take, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// From 1.
    pub line: usize,
    pub msg: String,
}

/// Reads a `plant.sim` file. A line with a problem is left out and
/// listed; the rest still runs.
#[must_use]
pub fn parse(text: &str) -> (Spec, Vec<Problem>) {
    let mut spec = Spec {
        seed: 1,
        ..Spec::default()
    };
    let mut problems = Vec::new();
    for (i, raw) in text.lines().enumerate().take(MAX_LINES) {
        let line = i + 1;
        let body = raw.split('#').next().unwrap_or("");
        let words: Vec<&str> = body.split_whitespace().collect();
        let Some(first) = words.first() else { continue };
        let mut say = |msg: String| problems.push(Problem { line, msg });
        match first.to_ascii_lowercase().as_str() {
            "seed" => match words.get(1).and_then(|w| w.parse::<u64>().ok()) {
                Some(n) if words.len() == 2 => spec.seed = n,
                _ => say("`seed` takes one whole number".into()),
            },
            "mode" => {
                let (Some(input), Some(word)) = (words.get(1), words.get(2)) else {
                    say("`mode INPUT WORD`, e.g. `mode temp_c stuck 80`".into());
                    continue;
                };
                if !ident(input) {
                    say(format!("`{}` is no variable name", cut(input)));
                    continue;
                }
                if words.len() > 4 {
                    say("a mode line takes one value at most".into());
                    continue;
                }
                match Mode::parse(word, words.get(3).copied()) {
                    Ok(mode) => {
                        if spec.modes.iter().any(|m| m.input.eq_ignore_ascii_case(input)) {
                            say(format!("{input} has a mode line already"));
                        } else {
                            spec.modes.push(ModeLine {
                                input: (*input).to_string(),
                                mode,
                                line,
                            });
                        }
                    }
                    Err(e) => say(e),
                }
            }
            w => match Kind::from_word(w) {
                None => say(format!(
                    "`{}` is no device kind: {}",
                    cut(w),
                    Kind::ALL.map(Kind::word).join(", ")
                )),
                Some(kind) => match device(kind, &words[1..], line, &spec) {
                    Ok(d) => spec.devices.push(d),
                    Err(e) => say(e),
                },
            },
        }
    }
    (spec, problems)
}

fn device(kind: Kind, words: &[&str], line: usize, spec: &Spec) -> Result<Device, String> {
    let Some(name) = words.first().filter(|n| !n.contains('=')) else {
        return Err(format!("a {} line starts with its name", kind.word()));
    };
    if !ident(name) {
        return Err(format!("`{}` is no name: letters, digits and _", cut(name)));
    }
    if spec.device(name).is_some() {
        return Err(format!("there is a device named {name} already"));
    }
    let mut values: Vec<(String, String)> = Vec::new();
    for w in &words[1..] {
        let Some((k, v)) = w.split_once('=') else {
            return Err(format!("`{}` is no key=value", cut(w)));
        };
        let k = k.to_ascii_lowercase();
        let Some(spec_key) = kind.keys().iter().find(|x| x.name == k) else {
            return Err(format!(
                "a {} takes no `{}`: {}",
                kind.word(),
                cut(&k),
                kind.keys().iter().map(|x| x.name).collect::<Vec<_>>().join(", ")
            ));
        };
        if values.iter().any(|(x, _)| *x == k) {
            return Err(format!("`{k}` is given twice"));
        }
        check(spec_key, v)?;
        values.push((k, v.to_string()));
    }
    for k in kind.keys().iter().filter(|k| k.required) {
        if !values.iter().any(|(x, _)| x == k.name) {
            return Err(format!("a {} needs `{}=` ({})", kind.word(), k.name, k.about));
        }
    }
    let d = Device {
        kind,
        name: (*name).to_string(),
        line,
        values,
    };
    for (_, input, _) in d.inputs() {
        if let Some((other, _, _)) = spec.owner(input) {
            return Err(format!("{input} is driven by {} already", other.name));
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for (_, input, _) in d.inputs() {
        if seen.iter().any(|s| s.eq_ignore_ascii_case(input)) {
            return Err(format!("{input} is named twice on one line"));
        }
        seen.push(input);
    }
    Ok(d)
}

fn check(key: &Key, v: &str) -> Result<(), String> {
    let ok = match key.role {
        Role::Out | Role::InBool | Role::InNum => ident(v),
        Role::Of => v.split('.').count() <= 2 && v.split('.').all(ident),
        Role::Number => number(v).is_some(),
        Role::Time => seconds(v).is_some(),
        Role::Range => range(v).is_some(),
        Role::Word(ws) => ws.iter().any(|w| w.eq_ignore_ascii_case(v)),
    };
    if ok {
        return Ok(());
    }
    let want = match key.role {
        Role::Out | Role::InBool | Role::InNum => "a variable name".to_string(),
        Role::Of => "a device (oven, oven.temp) or an output".into(),
        Role::Number => "a number".into(),
        Role::Time => "a time (500ms, 2s, 1.5m)".into(),
        Role::Range => "a range, low..high".into(),
        Role::Word(ws) => format!("one of {}", ws.join(", ")),
    };
    Err(format!("`{}={}`: {want}", key.name, cut(v)))
}

/// A name of letters, digits and `_`, not starting with a digit.
fn ident(s: &str) -> bool {
    let mut c = s.chars();
    s.len() <= MAX_WORD
        && c.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn cut(s: &str) -> String {
    s.chars().take(MAX_WORD).collect()
}

/// A finite number, of at most 1e12 either way.
#[must_use]
pub fn number(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|x| x.is_finite() && x.abs() <= 1e12)
}

/// A time in seconds: `500ms`, `2s`, `1.5m`, `1h` or a bare number of
/// seconds; not negative, at most a year.
#[must_use]
pub fn seconds(s: &str) -> Option<f64> {
    let lower = s.to_ascii_lowercase();
    let (n, scale) = if let Some(n) = lower.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = lower.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = lower.strip_suffix('m') {
        (n, 60.0)
    } else if let Some(n) = lower.strip_suffix('h') {
        (n, 3600.0)
    } else {
        (lower.as_str(), 1.0)
    };
    number(n)
        .map(|x| x * scale)
        .filter(|x| (0.0..=31_536_000.0).contains(x))
}

/// `LO..HI` with LO below HI.
#[must_use]
pub fn range(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once("..")?;
    let (a, b) = (number(a)?, number(b)?);
    (a < b).then_some((a, b))
}

fn bool_word(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "on" => Some(true),
        "false" | "off" => Some(false),
        _ => None,
    }
}

/// A number as short as it reads (`80`, `0.5`).
fn trim_num(x: f64) -> String {
    let s = format!("{x:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// `text` with `input`'s `mode` line set to `mode`: the old line goes, a
/// normal mode leaves none, any other is written where the old one was
/// (or at the end). Every other line stays as it was.
#[must_use]
pub fn set_mode(text: &str, input: &str, mode: Mode) -> String {
    let is_it = |l: &str| {
        let mut w = l.split('#').next().unwrap_or("").split_whitespace();
        w.next().is_some_and(|w| w.eq_ignore_ascii_case("mode"))
            && w.next().is_some_and(|w| w.eq_ignore_ascii_case(input))
    };
    let new = (mode != Mode::Normal).then(|| mode.to_line(input));
    let mut out: Vec<String> = Vec::new();
    let mut placed = false;
    for l in text.lines() {
        if is_it(l) {
            if !placed && let Some(n) = &new {
                out.push(n.clone());
            }
            placed = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !placed && let Some(n) = new {
        out.push(n);
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// `text` with the device line named `name` replaced by `line` (or
/// appended when there is none; removed when `line` is empty).
#[must_use]
pub fn set_device(text: &str, name: &str, line: &str) -> String {
    let is_it = |l: &str| {
        let mut w = l.split('#').next().unwrap_or("").split_whitespace();
        w.next().and_then(Kind::from_word).is_some() && w.next().is_some_and(|w| w.eq_ignore_ascii_case(name))
    };
    let mut out: Vec<String> = Vec::new();
    let mut placed = false;
    for l in text.lines() {
        if is_it(l) {
            if !placed && !line.is_empty() {
                out.push(line.to_string());
            }
            placed = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !placed && !line.is_empty() {
        out.push(line.to_string());
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}
