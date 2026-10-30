//! Explain mode's first layer: an [`explain()`](crate::explain()) answer
//! phrased in floor language by fixed templates, no model. It works with
//! the box offline and says nothing the trace does not show: every
//! sentence names only variables of the explanation it was built from
//! (listed in [`Sentence::paths`]), quotes source only as the source, and
//! a "what would change it" sentence only ever says which line would stop
//! or start running, never what the value would become.
//!
//! ```text
//! Conveyor is ON because line 21 set it: `CONVEYOR := M.ON`.
//!   m.on is ON because line 6 set it: `ON := TRUE`.
//!     It runs because run is ON and fault is OFF.
//!     Line 6 stops setting it if run turns OFF or fault turns ON.
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use helmsman_iec::Value;

use crate::explain::{Because, Blocked, Step, Test, Why};
use crate::lower::Module;
use crate::model::RegionKind;

/// What the templates know beside the trace: units and kinds of the
/// variables, and how long an input has been seen at its value. All keys
/// are program paths, upper-cased (`START`, `T.PT`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Words {
    /// The signal model's unit for a variable bound to a signal.
    pub units: HashMap<String, String>,
    /// Variables holding a TIME (nanoseconds).
    pub times: HashSet<String>,
    /// Variables that come from the field: signals and located inputs.
    pub field: HashSet<String>,
    /// How long a variable has been seen at its current value, at least,
    /// in ms (from the scans the viewer received; absent when unknown).
    pub steady_ms: HashMap<String, u64>,
}

impl Words {
    /// What a module's declarations say: TIME variables (the program's,
    /// and the PT and ET of every timer instance) and located inputs.
    #[must_use]
    pub fn of(m: &Module) -> Self {
        let mut w = Self::default();
        let Some(p) = m.program() else {
            return w;
        };
        let mut scopes = vec![(String::new(), p)];
        for (path, fb) in m.instances() {
            if let Some(q) = m.pou(&fb) {
                scopes.push((format!("{path}."), q));
            }
        }
        for (prefix, pou) in scopes {
            for (name, v) in &pou.vars {
                let path = format!("{prefix}{name}");
                if v.ty == "TIME" {
                    w.times.insert(path.clone());
                }
                if prefix.is_empty() && v.located && v.section != helmsman_iec::ast::Section::Output {
                    w.field.insert(path.clone());
                }
                if matches!(v.fb.as_deref(), Some("TON" | "TOF" | "TP")) {
                    w.times.insert(format!("{path}.PT"));
                    w.times.insert(format!("{path}.ET"));
                }
            }
        }
        w
    }
}

/// One sentence of the explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub text: String,
    /// How deep in the explanation it sits (0: the variable asked about).
    pub depth: usize,
    /// The source line it is about, if any (the panel highlights it).
    pub line: Option<u32>,
    /// Every variable the text names (upper-cased paths).
    pub paths: Vec<String>,
}

/// The explanation, sentence by sentence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Phrased {
    pub sentences: Vec<Sentence>,
}

impl Phrased {
    /// The sentences, indented two spaces per level, one per line.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for s in &self.sentences {
            let _ = writeln!(out, "{}{}", "  ".repeat(s.depth), s.text);
        }
        out
    }

    /// The source lines the explanation is about, outermost first, once each.
    #[must_use]
    pub fn lines(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for l in self.sentences.iter().filter_map(|s| s.line) {
            if !out.contains(&l) {
                out.push(l);
            }
        }
        out
    }
}

/// How deep the sentences follow inputs (the data may go deeper).
pub const DEPTH: usize = 3;
/// At most this many skipped assignments are told for a held variable.
const MAX_BLOCKED: usize = 4;

/// `b` in floor language.
#[must_use]
pub fn phrase(b: &Because, w: &Words) -> Phrased {
    let mut p = Phraser {
        w,
        out: Vec::new(),
        told: HashSet::new(),
    };
    p.because(b, 0, &mut Vec::new());
    Phrased { sentences: p.out }
}

/// Every variable path `b` holds data for (itself, inputs, gate inputs,
/// block ports), upper-cased.
#[must_use]
pub fn paths(b: &Because) -> HashSet<String> {
    fn go(b: &Because, out: &mut HashSet<String>) {
        out.insert(b.path.clone());
        match &b.why {
            Why::Assigned { gates, inputs, .. } => {
                for g in gates {
                    step(g, out);
                }
                for i in inputs {
                    go(i, out);
                }
            }
            Why::Held { blocked, .. } => {
                for g in blocked.iter().filter_map(|x| x.gate.as_ref()) {
                    step(g, out);
                }
            }
            Why::Block {
                inputs,
                ports,
                instance,
                ..
            } => {
                out.insert(instance.clone());
                for i in inputs {
                    go(i, out);
                }
                out.extend(ports.iter().map(|(p, _)| p.clone()));
            }
            Why::Argument { inputs, instance, .. } => {
                out.insert(instance.clone());
                for i in inputs {
                    go(i, out);
                }
            }
            Why::Input | Why::Elided => {}
        }
    }
    fn step(s: &Step, out: &mut HashSet<String>) {
        for t in &s.tests {
            for i in &t.inputs {
                go(i, out);
            }
        }
    }
    let mut out = HashSet::new();
    go(b, &mut out);
    out
}

/// A variable as the floor reads it: its path, lower case.
#[must_use]
pub fn name(path: &str) -> String {
    path.to_ascii_lowercase()
}

/// A duration given in nanoseconds: `400 ms`, `5 s`, `2.5 s`, `3 min 10 s`.
#[must_use]
pub fn duration(ns: i128) -> String {
    let neg = if ns < 0 { "-" } else { "" };
    let ns = ns.unsigned_abs();
    let ms = ns / 1_000_000;
    if ms < 1000 {
        if ns.is_multiple_of(1_000_000) || ms >= 100 {
            return format!("{neg}{ms} ms");
        }
        #[allow(clippy::cast_precision_loss)]
        let f = ns as f64 / 1e6;
        return format!("{neg}{} ms", trim(f, 3));
    }
    if ms < 60_000 {
        #[allow(clippy::cast_precision_loss)]
        let s = ms as f64 / 1000.0;
        return format!("{neg}{} s", trim(s, 1));
    }
    let (min, s) = (ms / 60_000, (ms % 60_000) / 1000);
    if min >= 60 {
        return format!("{neg}{} h {} min", min / 60, min % 60);
    }
    if s == 0 {
        format!("{neg}{min} min")
    } else {
        format!("{neg}{min} min {s} s")
    }
}

/// `x` with at most `places` decimals, trailing zeros dropped.
fn trim(x: f64, places: usize) -> String {
    let s = format!("{x:.places$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// How long, from milliseconds.
fn since(ms: u64) -> String {
    duration(i128::from(ms) * 1_000_000)
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// `a`, `a and b`, `a, b and c`.
fn and(items: &[String], word: &str) -> String {
    match items {
        [] => String::new(),
        [a] => a.clone(),
        [rest @ .., last] => format!("{} {word} {last}", rest.join(", ")),
    }
}

struct Phraser<'a> {
    w: &'a Words,
    out: Vec<Sentence>,
    /// Variables already explained (each gets its sentences once).
    told: HashSet<String>,
}

/// A sentence under construction: text and the variables it names.
#[derive(Default)]
struct Say {
    text: String,
    paths: Vec<String>,
}

impl Say {
    fn push(&mut self, s: &str) -> &mut Self {
        self.text.push_str(s);
        self
    }

    /// Names `path` in the text.
    fn var(&mut self, path: &str) -> &mut Self {
        self.text.push_str(&name(path));
        if !self.paths.iter().any(|p| p == path) {
            self.paths.push(path.to_string());
        }
        self
    }

    fn noted(&mut self, path: &str) {
        if !self.paths.iter().any(|p| p == path) {
            self.paths.push(path.to_string());
        }
    }
}

impl Phraser<'_> {
    fn emit(&mut self, s: Say, depth: usize, line: Option<u32>) {
        if s.text.is_empty() {
            return;
        }
        let mut text = cap(&s.text);
        if !text.ends_with('.') {
            text.push('.');
        }
        self.out.push(Sentence {
            text,
            depth,
            line,
            paths: s.paths,
        });
    }

    /// A value as the floor reads it: ON/OFF, a duration, a number with
    /// its unit.
    fn value(&self, path: &str, v: Option<Value>) -> String {
        match v {
            None => "unknown".into(),
            Some(Value::Bool(true)) => "ON".into(),
            Some(Value::Bool(false)) => "OFF".into(),
            Some(Value::Int(n)) if self.w.times.contains(path) => duration(n),
            Some(v) => {
                let n = match v {
                    Value::Real(x) => trim(x, 3),
                    v => v.to_string(),
                };
                match self.w.units.get(path) {
                    Some(u) if !u.is_empty() => format!("{n} {u}"),
                    _ => n,
                }
            }
        }
    }

    /// `start is ON`, `speed is 1.5 m/s`.
    fn is(&self, s: &mut Say, path: &str, v: Option<Value>) {
        s.var(path).push(" is ").push(&self.value(path, v));
    }

    /// `start (ON)`, or `running (its own value)` for a variable read by
    /// its own assignment (the trace has only the value after the scan).
    fn short(&self, s: &mut Say, b: &Because, stack: &[String]) {
        s.var(&b.path);
        if stack.contains(&b.path) {
            s.push(" (its own value from before)");
        } else {
            s.push(" (").push(&self.value(&b.path, b.value)).push(")");
        }
    }

    fn list(&self, s: &mut Say, items: &[Because], stack: &[String]) {
        let n = items.len();
        for (k, i) in items.iter().enumerate() {
            if k > 0 {
                s.push(if k + 1 == n { " and " } else { ", " });
            }
            self.short(s, i, stack);
        }
    }

    fn because(&mut self, b: &Because, depth: usize, stack: &mut Vec<String>) {
        if depth > DEPTH || !self.told.insert(b.path.clone()) {
            return;
        }
        stack.push(b.path.clone());
        match &b.why {
            Why::Assigned {
                line,
                text,
                gates,
                inputs,
                ..
            } => self.assigned(b, *line, text, gates, inputs, depth, stack),
            Why::Held { blocked, .. } => self.held(b, blocked, depth),
            Why::Block {
                instance,
                fb,
                call,
                inputs,
                ports,
            } => self.block(b, instance, fb, *call, inputs, ports, depth, stack),
            Why::Argument {
                instance,
                call,
                inputs,
            } => {
                let mut s = Say::default();
                self.is(&mut s, &b.path, b.value);
                match call {
                    Some((_, line)) => {
                        let _ = write!(s.text, ": it is what line {line} passed into ");
                        s.var(instance);
                        if !inputs.is_empty() {
                            s.push(", from ");
                            self.list(&mut s, inputs, stack);
                        }
                        self.emit(s, depth, Some(*line));
                    }
                    None => {
                        s.push(": ").var(instance);
                        s.push(" was not called this scan, so it keeps what it was last given");
                        self.emit(s, depth, None);
                    }
                }
                self.inputs(inputs, depth + 1, stack);
            }
            Why::Input => {
                let mut s = Say::default();
                self.is(&mut s, &b.path, b.value);
                if let Some(ms) = self.w.steady_ms.get(&b.path) {
                    let _ = write!(s.text, " and has been for at least {}", since(*ms));
                }
                if self.w.field.contains(&b.path) {
                    s.push(": it comes from the field");
                } else {
                    s.push(": nothing in this program sets it");
                }
                self.emit(s, depth, None);
            }
            Why::Elided => {
                if depth == 0 {
                    let mut s = Say::default();
                    self.is(&mut s, &b.path, b.value);
                    self.emit(s, depth, None);
                }
            }
        }
        stack.pop();
    }

    /// Explains the inputs that have more to say than their value.
    fn inputs(&mut self, inputs: &[Because], depth: usize, stack: &mut Vec<String>) {
        for i in inputs {
            if stack.contains(&i.path) {
                continue;
            }
            match i.why {
                Why::Elided => {}
                Why::Input if !self.w.steady_ms.contains_key(&i.path) && !self.w.field.contains(&i.path) => {}
                _ => self.because(i, depth, stack),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn assigned(
        &mut self,
        b: &Because,
        line: u32,
        text: &str,
        gates: &[Step],
        inputs: &[Because],
        depth: usize,
        stack: &mut Vec<String>,
    ) {
        let mut s = Say::default();
        self.is(&mut s, &b.path, b.value);
        let _ = write!(s.text, " because line {line} set it: `{text}`");
        self.emit(s, depth, Some(line));
        if !inputs.is_empty() {
            let mut s = Say::default();
            s.push("it reads ");
            self.list(&mut s, inputs, stack);
            self.emit(s, depth + 1, Some(line));
        }
        if !gates.is_empty() {
            let mut why = Say::default();
            let mut parts = Vec::new();
            let mut flips = Vec::new();
            for g in gates {
                self.gate_open(g, &mut why, &mut parts, &mut flips);
            }
            if !parts.is_empty() {
                why.text = format!("it runs because {}", and(&parts, "and"));
                self.emit(why, depth + 1, Some(line));
            }
            if !flips.is_empty() {
                let mut s = Say::default();
                for (p, _) in flips.iter().filter(|(p, _)| !p.is_empty()) {
                    s.noted(p);
                }
                let words: Vec<String> = flips.into_iter().map(|(_, t)| t).collect();
                s.text = format!("line {line} stops setting it if {}", and(&words, "or"));
                self.emit(s, depth + 1, Some(line));
            }
        }
        self.inputs(inputs, depth + 1, stack);
        for g in gates {
            for t in &g.tests {
                self.inputs(&t.inputs, depth + 1, stack);
            }
        }
    }

    /// Why an open gate let its line run: phrases into `parts` (their
    /// variables noted in `say`), and what would close it into `flips`
    /// (a variable and the words).
    fn gate_open(&self, g: &Step, say: &mut Say, parts: &mut Vec<String>, flips: &mut Vec<(String, String)>) {
        match g.kind {
            RegionKind::If | RegionKind::Elsif | RegionKind::Else | RegionKind::While => {
                for t in &g.tests {
                    if let Some((part, flip)) = self.test(t, say) {
                        parts.push(part);
                        flips.extend(flip);
                    }
                }
            }
            RegionKind::Case | RegionKind::CaseElse => {
                if let Some(t) = g.tests.first() {
                    let v = t.value.map_or_else(|| "unknown".into(), |v| v.to_string());
                    parts.push(format!("`{}` is {v}, which picks `{}`", t.text, g.label));
                    for i in &t.inputs {
                        say.noted(&i.path);
                    }
                }
            }
            RegionKind::For | RegionKind::Repeat => {
                parts.push(format!("it sits in the loop `{}`, which ran", g.label));
            }
        }
    }

    /// One condition of an open gate: `run is ON` (or `` `a > 3` is TRUE
    /// ``), and for a single-variable condition, the change that would
    /// close the gate.
    fn test(&self, t: &Test, say: &mut Say) -> Option<(String, Option<(String, String)>)> {
        let Some(Value::Bool(held)) = t.value else {
            return None;
        };
        // A bare variable (`run`) or its negation (`NOT fault`): name it.
        let bare = single(t);
        if let Some(path) = bare {
            say.noted(&path);
            let input = t.inputs.first().and_then(|i| i.value);
            let v = self.value(&path, input);
            let part = format!("{} is {v}", name(&path));
            let flip = match input {
                Some(Value::Bool(x)) => Some((
                    path.clone(),
                    format!("{} turns {}", name(&path), if x { "OFF" } else { "ON" }),
                )),
                _ => None,
            };
            return Some((part, flip));
        }
        let mut s = format!("`{}` is {}", t.text, if held { "TRUE" } else { "FALSE" });
        if !t.inputs.is_empty() {
            let mut inner = Say::default();
            self.list(&mut inner, &t.inputs, &[]);
            let _ = write!(s, " ({})", inner.text);
            for p in inner.paths {
                say.noted(&p);
            }
        }
        let flip = format!("`{}` turns {}", t.text, if held { "FALSE" } else { "TRUE" });
        Some((s, Some((String::new(), flip))))
    }

    fn held(&mut self, b: &Because, blocked: &[Blocked], depth: usize) {
        let mut s = Say::default();
        self.is(&mut s, &b.path, b.value);
        s.push(" and holding: no line set it this scan");
        self.emit(s, depth, None);
        let mut needs = Vec::new();
        let mut need_paths = Vec::new();
        for x in blocked.iter().take(MAX_BLOCKED) {
            let mut s = Say::default();
            let _ = write!(s.text, "line {} (`{}`) ", x.line, x.text);
            match &x.gate {
                None => {
                    s.push("was not reached: an EXIT, RETURN or fault ended the scan first");
                }
                Some(g) => {
                    s.push("was skipped: ");
                    let (why, need) = self.gate_closed(g, &mut s);
                    s.push(&why);
                    if let Some(n) = need {
                        needs.push(format!("line {} runs ({n})", x.line));
                    }
                    need_paths.extend(s.paths.iter().cloned());
                }
            }
            self.emit(s, depth + 1, Some(x.line));
        }
        if blocked.len() > MAX_BLOCKED {
            let mut s = Say::default();
            let _ = write!(
                s.text,
                "{} more lines that set it were skipped too",
                blocked.len() - MAX_BLOCKED
            );
            self.emit(s, depth + 1, None);
        }
        if !needs.is_empty() {
            let mut s = Say::default();
            for p in need_paths {
                s.noted(&p);
            }
            s.text = format!("it changes only when {}", and(&needs, "or"));
            self.emit(s, depth + 1, None);
        }
        let mut stack = vec![b.path.clone()];
        for g in blocked.iter().take(MAX_BLOCKED).filter_map(|x| x.gate.as_ref()) {
            for t in &g.tests {
                self.inputs(&t.inputs, depth + 1, &mut stack);
            }
        }
    }

    /// Why a closed gate kept its line from running, and what it needs.
    fn gate_closed(&self, g: &Step, say: &mut Say) -> (String, Option<String>) {
        match g.kind {
            RegionKind::If | RegionKind::Elsif | RegionKind::Else | RegionKind::While => {
                // The first test that went the wrong way.
                for t in &g.tests {
                    let Some(Value::Bool(v)) = t.value else { continue };
                    if v == t.wanted {
                        continue;
                    }
                    let need = if t.wanted { "TRUE" } else { "FALSE" };
                    if let Some(path) = single(t) {
                        say.noted(&path);
                        let input = t.inputs.first().and_then(|i| i.value);
                        let now = self.value(&path, input);
                        let want = match input {
                            Some(Value::Bool(x)) => {
                                format!("{} to turn {}", name(&path), if x { "OFF" } else { "ON" })
                            }
                            _ => format!("`{}` to be {need}", t.text),
                        };
                        return (
                            format!(
                                "{} is {now} (`{}` is {})",
                                name(&path),
                                g.label,
                                if v { "TRUE" } else { "FALSE" }
                            ),
                            Some(format!("needs {want}")),
                        );
                    }
                    let mut s = format!("`{}` is {}", t.text, if v { "TRUE" } else { "FALSE" });
                    if !t.inputs.is_empty() {
                        let mut inner = Say::default();
                        self.list(&mut inner, &t.inputs, &[]);
                        let _ = write!(s, " ({})", inner.text);
                        for p in inner.paths {
                            say.noted(&p);
                        }
                    }
                    return (s, Some(format!("needs `{}` to be {need}", t.text)));
                }
                (format!("its branch `{}` was not reached", g.label), None)
            }
            RegionKind::Case | RegionKind::CaseElse => match g.tests.first() {
                Some(t) => {
                    for i in &t.inputs {
                        say.noted(&i.path);
                    }
                    let v = t.value.map_or_else(|| "unknown".into(), |v| v.to_string());
                    (
                        format!("`{}` is {v}, which does not pick `{}`", t.text, g.label),
                        Some(format!("needs `{}` to pick `{}`", t.text, g.label)),
                    )
                }
                None => (format!("the branch `{}` was not taken", g.label), None),
            },
            RegionKind::For | RegionKind::Repeat => (format!("the loop `{}` made no pass", g.label), None),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn block(
        &mut self,
        b: &Because,
        instance: &str,
        fb: &str,
        call: Option<(helmsman_iec::trace::StmtId, u32)>,
        inputs: &[Because],
        ports: &[(String, Option<Value>)],
        depth: usize,
        stack: &mut Vec<String>,
    ) {
        let line = call.map(|(_, l)| l);
        let port = |p: &str| ports.iter().find(|(k, _)| k.rsplit('.').next() == Some(p));
        let mut s = Say::default();
        self.is(&mut s, &b.path, b.value);
        s.noted(instance);
        let inst = name(instance);
        let Some(line_no) = line else {
            let _ = write!(
                s.text,
                ": it comes from {inst} ({fb}), which was not called this scan, so it keeps its last value"
            );
            self.emit(s, depth, None);
            return;
        };
        let _ = write!(s.text, ": it comes from {inst} ({fb}), called on line {line_no}");
        self.emit(s, depth, line);
        let mut d = Say::default();
        d.noted(instance);
        // Name a port by its path and value: `t.in (ON)`.
        let pv = |this: &Self, d: &mut Say, p: &str| -> bool {
            match port(p) {
                Some((path, v)) => {
                    d.var(path).push(" is ").push(&this.value(path, *v));
                    true
                }
                None => false,
            }
        };
        let val = |p: &str| port(p).and_then(|(_, v)| *v);
        let time = |this: &Self, p: &str| port(p).map(|(path, v)| this.value(path, *v));
        match fb {
            "TON" => {
                let pt = time(self, "PT").unwrap_or_else(|| "its preset".into());
                let _ = write!(d.text, "{inst} turns ON after {pt} of its input being ON. Now ");
                pv(self, &mut d, "IN");
                match (val("IN"), val("Q")) {
                    (Some(Value::Bool(true)), Some(Value::Bool(true))) => {
                        let _ = write!(d.text, " and the full {pt} has passed");
                    }
                    (Some(Value::Bool(true)), _) => {
                        d.push(" and ");
                        pv(self, &mut d, "ET");
                        let _ = write!(d.text, " of {pt} so far");
                    }
                    (Some(Value::Bool(false)), _) => {
                        d.push(", so the timer is reset");
                    }
                    _ => {}
                }
            }
            "TOF" => {
                let pt = time(self, "PT").unwrap_or_else(|| "its preset".into());
                let _ = write!(d.text, "{inst} stays ON for {pt} after its input goes OFF. Now ");
                pv(self, &mut d, "IN");
                match (val("IN"), val("Q")) {
                    (Some(Value::Bool(false)), Some(Value::Bool(true))) => {
                        d.push(" and ");
                        pv(self, &mut d, "ET");
                        let _ = write!(d.text, " of {pt} have passed");
                    }
                    (Some(Value::Bool(false)), Some(Value::Bool(false))) => {
                        let _ = write!(d.text, " and the {pt} have passed");
                    }
                    _ => {}
                }
            }
            "TP" => {
                let pt = time(self, "PT").unwrap_or_else(|| "its preset".into());
                let _ = write!(
                    d.text,
                    "{inst} gives one pulse of {pt} when its input turns ON. Now "
                );
                pv(self, &mut d, "IN");
                if val("Q") == Some(Value::Bool(true)) {
                    d.push(" and ");
                    pv(self, &mut d, "ET");
                    let _ = write!(d.text, " of the {pt} pulse");
                }
            }
            "CTU" | "CTD" | "CTUD" => {
                let verb = match fb {
                    "CTU" => "counts up on each rising edge of its count input",
                    "CTD" => "counts down on each rising edge of its count input",
                    _ => "counts up and down on rising edges of its count inputs",
                };
                let _ = write!(d.text, "{inst} {verb}. Now ");
                pv(self, &mut d, "CV");
                if port("PV").is_some() {
                    d.push(" and its preset ");
                    pv(self, &mut d, "PV");
                }
                for r in ["R", "LD"] {
                    if val(r) == Some(Value::Bool(true)) {
                        d.push("; ");
                        pv(self, &mut d, r);
                        d.push(if r == "R" {
                            " (resetting it)"
                        } else {
                            " (loading the preset)"
                        });
                    }
                }
            }
            "R_TRIG" | "F_TRIG" => {
                let edge = if fb == "R_TRIG" { "turns ON" } else { "turns OFF" };
                let _ = write!(d.text, "{inst} is ON for one scan when its input {edge}. Now ");
                pv(self, &mut d, "CLK");
            }
            "SR" | "RS" => {
                let (set, reset) = if fb == "SR" { ("S1", "R") } else { ("S", "R1") };
                let wins = if fb == "SR" { "set" } else { "reset" };
                let _ = write!(d.text, "{inst} is a latch ({wins} wins). Now ");
                pv(self, &mut d, set);
                d.push(" and ");
                pv(self, &mut d, reset);
            }
            _ => {}
        }
        self.emit(d, depth + 1, line);
        if !inputs.is_empty() {
            let mut s = Say::default();
            s.push("its call reads ");
            self.list(&mut s, inputs, stack);
            self.emit(s, depth + 1, line);
        }
        self.inputs(inputs, depth + 1, stack);
    }
}

/// The variable a condition is, alone or negated: `RUN`, `NOT FAULT`.
fn single(t: &Test) -> Option<String> {
    let [i] = t.inputs.as_slice() else {
        return None;
    };
    let text: String = t.text.chars().filter(|c| !c.is_whitespace()).collect();
    let text = text.to_ascii_uppercase();
    let path = i.path.to_ascii_uppercase();
    let rest = text.strip_prefix("NOT").unwrap_or(&text);
    let rest = rest
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or(rest);
    (text == path || rest == path).then(|| i.path.clone())
}
