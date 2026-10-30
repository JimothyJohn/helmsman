//! A module on disk: a directory holding [`SOURCE`] (one ST `PROGRAM`) and
//! [`FILE`], its manifest, one declaration per line:
//!
//! ```text
//! # conveyor/module.manifest
//! name     conveyor
//! author   Nick <nick@example.com>
//! prompt   Run the belt while start is held; stop on the jam sensor.
//! prompt   Ramp to speed over ramp_ms.
//! scan_ms  50
//! q        0+2
//! reads    start jam
//! drives   motor lamp
//! param    speed   real 1.5 min=0 max=10 unit=m/s
//! param    ramp_ms int  500 min=0 max=5000 unit=ms
//! param    beacon  bool true
//! ```
//!
//! - `name` (1..=64 of `A-Z a-z 0-9 _ -`), `author`, `scan_ms` (1..=10000),
//!   `reads` and `drives` are required, once each. `reads` / `drives` list
//!   signal names and may be empty: a module that reads nothing says so.
//! - `prompt` is optional and repeatable, one line of the prompt that
//!   produced the module per line; none means written by hand. `author`
//!   and `prompt` are free text to the end of the line (any Unicode but
//!   control and invisible formatting characters, so a manifest cannot
//!   carry an escape sequence or a bidi override into a log or a review);
//!   their surrounding blanks are trimmed and `#` in them is text.
//! - `i` / `q` (`START+LEN` bytes) are the module's `%I` / `%Q` windows,
//!   relative addressing as with `--module`; a module that leaves one out
//!   has none (zero bytes). `m` sizes the private `%M` (default as
//!   `--memory`).
//! - `param NAME TYPE VALUE [min=X max=Y] [unit=U]`: a typed value the
//!   engineer or the agent may change without touching the logic. `int`
//!   and `real` need both bounds and `min <= VALUE <= max`; `bool` takes
//!   none. `unit` (numbers only, 1..=16 printable ASCII, as a signal's) is
//!   metadata, never converted: the offline chat checks a typed unit
//!   against it ([`crate::fallback`]).
//!
//! Every other line is blank, a `#` comment, or an error; structured lines
//! may end in a `#` comment and are printable ASCII only.
//!
//! **Ports.** A module's interface is its `PROGRAM`'s `VAR_INPUT`s and
//! `VAR_OUTPUT`s. Each `VAR_INPUT` is either a signal it `reads` or a
//! `param`; each `VAR_OUTPUT` is a signal it `drives`. [`Manifest::check`]
//! holds the manifest to exactly that (names case-insensitive, as ST is),
//! so whatever reads the manifest (policy, the box API, a reviewer) can
//! trust it without reading the ST. A param is set before every scan; the
//! program's own initial value for that input is never used.
//!
//! [`parse`](Manifest::parse) checks the file alone; [`Manifest::check`]
//! checks it against the compiled program; the runtime then binds `reads`
//! and `drives` to the I/O map.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use helmsman_iec::{Port, PortDir, Program, Ty as StTy};
use helmsman_io::{Ty, Value};

use crate::spec::{Defaults, Spec, name_ok, window};
use crate::{Module, Window};

/// The manifest's file name inside a module directory.
pub const FILE: &str = "module.manifest";
/// The program's file name inside a module directory.
pub const SOURCE: &str = "module.st";
/// Largest manifest or program [`ModuleDir::read`] accepts, in bytes.
pub const MAX_FILE: u64 = 1 << 20;

/// A `param` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    /// The value the module runs with.
    pub value: Value,
    /// `(min, max)`, inclusive, of the value's type; `None` for `bool`.
    pub range: Option<(Value, Value)>,
    /// Engineering unit (`unit=`), metadata only; never on a `bool`.
    pub unit: Option<String>,
}

impl Param {
    #[must_use]
    pub fn ty(&self) -> Ty {
        self.value.ty()
    }

    /// Whether `v` is a value this param may take: same type, in range.
    #[must_use]
    pub fn admits(&self, v: Value) -> bool {
        v.ty() == self.ty()
            && match (self.range, v) {
                (None, _) => true,
                (Some((Value::Int(lo), Value::Int(hi))), Value::Int(n)) => lo <= n && n <= hi,
                (Some((Value::Real(lo), Value::Real(hi))), Value::Real(x)) => {
                    x.is_finite() && lo <= x && x <= hi
                }
                _ => false,
            }
    }
}

/// A parsed manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub author: String,
    /// The prompt that produced the module, line by line; empty if written
    /// by hand.
    pub prompt: Vec<String>,
    pub scan: Duration,
    pub i: Option<Window>,
    pub q: Option<Window>,
    pub m: Option<usize>,
    /// Signals the module reads (its signal `VAR_INPUT`s).
    pub reads: Vec<String>,
    /// Signals the module drives (its `VAR_OUTPUT`s).
    pub drives: Vec<String>,
    pub params: Vec<Param>,
}

/// Where a manifest and its program disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// A port the manifest does not declare.
    Undeclared { port: String, dir: PortDir },
    /// A name under `key` (`reads`, `drives` or `param`) with no port of
    /// that direction in the program.
    NoPort { name: String, key: &'static str },
    /// A param whose type or range the port's ST type cannot hold.
    ParamType { name: String, st: StTy },
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeclared { port, dir } => {
                let (sec, key) = match dir {
                    PortDir::Input => ("VAR_INPUT", "reads or param"),
                    PortDir::Output => ("VAR_OUTPUT", "drives"),
                };
                write!(f, "{sec} {port} is not in the manifest (want it under {key})")
            }
            Self::NoPort { name, key } => {
                let sec = if *key == "drives" {
                    "VAR_OUTPUT"
                } else {
                    "VAR_INPUT"
                };
                write!(f, "{key} {name}: the program has no {sec} of that name")
            }
            Self::ParamType { name, st } => {
                write!(f, "param {name}: its type or range does not fit {st}")
            }
        }
    }
}

impl std::error::Error for CheckError {}

const REQUIRED: [&str; 5] = ["name", "author", "scan_ms", "reads", "drives"];

/// Characters free text may not carry: controls, and the invisible
/// formatting characters that can make text read differently than it is
/// (bidi overrides and isolates, zero-width characters, the BOM).
pub(crate) fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{00AD}' | '\u{061C}' | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}')
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn ty(s: &str) -> Option<Ty> {
    match s {
        "bool" => Some(Ty::Bool),
        "int" => Some(Ty::Int),
        "real" => Some(Ty::Real),
        _ => None,
    }
}

const fn ty_name(t: Ty) -> &'static str {
    match t {
        Ty::Bool => "bool",
        Ty::Int => "int",
        Ty::Real => "real",
    }
}

fn parse_value(ty: Ty, s: &str) -> Option<Value> {
    match ty {
        Ty::Bool => match s {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        Ty::Int => {
            let digits = s.strip_prefix('-').unwrap_or(s);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse().ok().map(Value::Int)
        }
        Ty::Real => s.parse::<f64>().ok().filter(|x| x.is_finite()).map(Value::Real),
    }
}

fn show(v: Value) -> String {
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Real(x) => format!("{x:?}"),
    }
}

/// What a manifest is being built into while parsing.
#[derive(Default)]
struct Draft {
    name: Option<String>,
    author: Option<String>,
    prompt: Vec<String>,
    scan: Option<Duration>,
    i: Option<Window>,
    q: Option<Window>,
    m: Option<usize>,
    reads: Option<Vec<String>>,
    drives: Option<Vec<String>>,
    params: Vec<Param>,
    /// Every port name declared so far, upper-cased, with its key.
    names: BTreeMap<String, &'static str>,
}

impl Draft {
    fn port(&mut self, name: &str, key: &'static str) -> Result<(), String> {
        if !is_ident(name) {
            return Err(format!("{key} {name:?}: not an identifier"));
        }
        let up = name.to_ascii_uppercase();
        if let Some(prev) = self.names.get(&up) {
            return Err(format!("{name} declared twice (under {prev} and {key})"));
        }
        self.names.insert(up, key);
        Ok(())
    }
}

fn once<T>(slot: &mut Option<T>, key: &str, v: T) -> Result<(), String> {
    if slot.replace(v).is_some() {
        return Err(format!("{key} given twice"));
    }
    Ok(())
}

impl Manifest {
    /// Parses a manifest.
    ///
    /// # Errors
    /// Every problem, as `(line number from 1, why)`; problems with the
    /// file as a whole (a missing key) are on line 0.
    pub fn parse(src: &str) -> Result<Self, Vec<(usize, String)>> {
        let mut d = Draft::default();
        let mut errs = Vec::new();
        for (n, raw) in src.lines().enumerate() {
            if let Err(e) = line(raw, &mut d) {
                errs.push((n + 1, e));
            }
        }
        let have = [
            d.name.is_some(),
            d.author.is_some(),
            d.scan.is_some(),
            d.reads.is_some(),
            d.drives.is_some(),
        ];
        for (key, have) in REQUIRED.iter().zip(have) {
            if !have {
                errs.push((0, format!("missing {key}")));
            }
        }
        match d {
            Draft {
                name: Some(name),
                author: Some(author),
                scan: Some(scan),
                reads: Some(reads),
                drives: Some(drives),
                prompt,
                i,
                q,
                m,
                params,
                ..
            } if errs.is_empty() => Ok(Self {
                name,
                author,
                prompt,
                scan,
                i,
                q,
                m,
                reads,
                drives,
                params,
            }),
            _ => Err(errs),
        }
    }

    /// Writes a manifest that [`Manifest::parse`] reads back as `self`.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "name     {}", self.name);
        let _ = writeln!(s, "author   {}", self.author);
        for p in &self.prompt {
            let _ = writeln!(s, "prompt   {p}");
        }
        let _ = writeln!(s, "scan_ms  {}", self.scan.as_millis());
        for (key, w) in [("i", self.i), ("q", self.q)] {
            if let Some(w) = w {
                let _ = writeln!(s, "{key}        {}+{}", w.start, w.len);
            }
        }
        if let Some(m) = self.m {
            let _ = writeln!(s, "m        {m}");
        }
        let _ = writeln!(s, "reads    {}", self.reads.join(" "));
        let _ = writeln!(s, "drives   {}", self.drives.join(" "));
        for p in &self.params {
            let _ = write!(s, "param    {} {} {}", p.name, ty_name(p.ty()), show(p.value));
            if let Some((lo, hi)) = p.range {
                let _ = write!(s, " min={} max={}", show(lo), show(hi));
            }
            if let Some(u) = &p.unit {
                let _ = write!(s, " unit={u}");
            }
            s.push('\n');
        }
        s
    }

    /// The `--module` equivalent: where the program is and how it is
    /// scheduled. Windows the manifest leaves out are empty; `m` comes
    /// from `d`.
    #[must_use]
    pub fn spec(&self, dir: &Path, d: &Defaults) -> Spec {
        let none = Window::new(0, 0);
        Spec {
            path: dir.join(SOURCE),
            name: self.name.clone(),
            period: self.scan,
            i: self.i.unwrap_or(none),
            q: self.q.unwrap_or(none),
            m: self.m.unwrap_or(d.m),
        }
    }

    /// Holds the manifest to the program's ports: every `VAR_INPUT` is
    /// under `reads` or `param`, every `VAR_OUTPUT` under `drives`, every
    /// declared name is a port facing the right way, and every param fits
    /// its port's type over its whole range.
    ///
    /// # Errors
    /// Every disagreement, ports first in declaration order.
    pub fn check(&self, ports: &[Port]) -> Result<(), Vec<CheckError>> {
        let mut errs = Vec::new();
        let declared = |name: &str, list: &[String]| list.iter().any(|n| n.eq_ignore_ascii_case(name));
        for p in ports {
            let ok = match p.dir {
                PortDir::Input => {
                    declared(&p.name, &self.reads)
                        || self.params.iter().any(|q| q.name.eq_ignore_ascii_case(&p.name))
                }
                PortDir::Output => declared(&p.name, &self.drives),
            };
            if !ok {
                errs.push(CheckError::Undeclared {
                    port: p.name.clone(),
                    dir: p.dir,
                });
            }
        }
        let port = |name: &str, dir: PortDir| {
            ports
                .iter()
                .find(|p| p.dir == dir && p.name.eq_ignore_ascii_case(name))
        };
        for (key, list, dir) in [
            ("reads", &self.reads, PortDir::Input),
            ("drives", &self.drives, PortDir::Output),
        ] {
            for name in list {
                if port(name, dir).is_none() {
                    errs.push(CheckError::NoPort {
                        name: name.clone(),
                        key,
                    });
                }
            }
        }
        for p in &self.params {
            match port(&p.name, PortDir::Input) {
                None => errs.push(CheckError::NoPort {
                    name: p.name.clone(),
                    key: "param",
                }),
                Some(port) if !fits(p, port.ty) => errs.push(CheckError::ParamType {
                    name: p.name.clone(),
                    st: port.ty,
                }),
                Some(_) => {}
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    /// The params as the runtime takes them: name and value.
    #[must_use]
    pub fn param_values(&self) -> Vec<(String, Value)> {
        self.params.iter().map(|p| (p.name.clone(), p.value)).collect()
    }
}

/// Whether every value `p` admits can be set on a `VAR_INPUT` of type `st`.
fn fits(p: &Param, st: StTy) -> bool {
    match (p.ty(), p.range) {
        (Ty::Bool, _) => st == StTy::Bool,
        (Ty::Int, Some((Value::Int(lo), Value::Int(hi)))) => {
            let (a, b) = st.range();
            st.is_integral() && st != StTy::Bool && a <= i128::from(lo) && i128::from(hi) <= b
        }
        (Ty::Real, Some((Value::Real(lo), Value::Real(hi)))) => {
            st.is_real() && (st != StTy::Real || (lo.abs().max(hi.abs()) <= f64::from(f32::MAX)))
        }
        _ => false,
    }
}

fn line(raw: &str, d: &mut Draft) -> Result<(), String> {
    let trimmed = raw.trim_matches([' ', '\t']);
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(());
    }
    let (key, rest) = trimmed.split_once([' ', '\t']).unwrap_or((trimmed, ""));
    if matches!(key, "author" | "prompt") {
        let text = rest.trim_matches([' ', '\t']);
        if let Some(c) = text.chars().find(|&c| hidden(c)) {
            return Err(format!("{key}: no control or invisible characters, got {c:?}"));
        }
        if key == "author" {
            if text.is_empty() {
                return Err("author: who wrote it, not empty".into());
            }
            return once(&mut d.author, key, text.into());
        }
        d.prompt.push(text.into());
        return Ok(());
    }
    let code = trimmed.split('#').next().unwrap_or_default();
    if let Some(c) = code
        .chars()
        .find(|c| !c.is_ascii() || (c.is_ascii_control() && *c != '\t'))
    {
        return Err(format!("{c:?}: only printable ASCII outside author and prompt"));
    }
    let words: Vec<&str> = code.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        ["name", v] => {
            if !name_ok(v) {
                return Err(format!("name: 1..=64 of A-Z a-z 0-9 _ -, got {v:?}"));
            }
            once(&mut d.name, "name", (*v).into())
        }
        ["scan_ms", v] => match v.parse::<u64>() {
            Ok(ms @ 1..=10_000) if v.bytes().all(|b| b.is_ascii_digit()) => {
                once(&mut d.scan, "scan_ms", Duration::from_millis(ms))
            }
            _ => Err(format!("scan_ms: expected 1..=10000, got {v:?}")),
        },
        [k @ ("i" | "q"), v] => {
            let w = window(v).ok_or_else(|| format!("{k}: expected START+LEN in bytes, got {v:?}"))?;
            once(if *k == "i" { &mut d.i } else { &mut d.q }, k, w)
        }
        ["m", v] => match v.parse::<usize>() {
            Ok(n @ 0..=65_536) if v.bytes().all(|b| b.is_ascii_digit()) => once(&mut d.m, "m", n),
            _ => Err(format!("m: expected 0..=65536 bytes, got {v:?}")),
        },
        [k @ ("reads" | "drives"), names @ ..] => {
            let key: &'static str = if *k == "reads" { "reads" } else { "drives" };
            let list: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
            // Present even if a name below is bad, so one bad name is one
            // error, not also a "missing" one.
            let slot = if key == "reads" {
                &mut d.reads
            } else {
                &mut d.drives
            };
            once(slot, key, list)?;
            names.iter().try_for_each(|n| d.port(n, key))
        }
        ["param", name, t, v, bounds @ ..] => {
            let p = param(name, t, v, bounds)?;
            d.port(name, "param")?;
            d.params.push(p);
            Ok(())
        }
        ["param", ..] => Err("param: expected NAME TYPE VALUE [min=X max=Y]".into()),
        [k, ..] if REQUIRED.contains(k) || matches!(*k, "i" | "q" | "m") => {
            Err(format!("{k}: wrong number of values"))
        }
        [k, ..] => Err(format!("unknown key {k:?}")),
        [] => Ok(()),
    }
}

fn param(name: &str, tyname: &str, text: &str, bounds: &[&str]) -> Result<Param, String> {
    let ty = ty(tyname).ok_or_else(|| format!("param {name}: type {tyname:?}: bool, int or real"))?;
    let value = parse_value(ty, text).ok_or_else(|| format!("param {name}: {text:?} is not a {tyname}"))?;
    if ty == Ty::Bool {
        if !bounds.is_empty() {
            return Err(format!("param {name}: a bool has no range"));
        }
        return Ok(Param {
            name: name.into(),
            value,
            range: None,
            unit: None,
        });
    }
    let (mut lo, mut hi, mut unit) = (None, None, None);
    for b in bounds {
        let (key, raw) = b.split_once('=').unwrap_or((b, ""));
        let slot = match key {
            "min" => &mut lo,
            "max" => &mut hi,
            "unit" => {
                if !helmsman_io::unit_ok(raw) {
                    return Err(format!(
                        "param {name}: unit must be 1..={} printable ASCII characters",
                        helmsman_io::MAX_UNIT
                    ));
                }
                once(&mut unit, key, raw.to_string()).map_err(|e| format!("param {name}: {e}"))?;
                continue;
            }
            _ => return Err(format!("param {name}: {b:?}: expected min=X, max=X or unit=U")),
        };
        let bound =
            parse_value(ty, raw).ok_or_else(|| format!("param {name}: {key}: {raw:?} is not a {tyname}"))?;
        once(slot, key, bound).map_err(|e| format!("param {name}: {e}"))?;
    }
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Err(format!("param {name}: an {tyname} needs min= and max="));
    };
    let param = Param {
        name: name.into(),
        value,
        range: Some((lo, hi)),
        unit,
    };
    let ordered = match (lo, hi) {
        (Value::Int(lo), Value::Int(hi)) => lo <= hi,
        (Value::Real(lo), Value::Real(hi)) => lo <= hi,
        _ => false,
    };
    if !ordered {
        return Err(format!("param {name}: min is above max"));
    }
    if !param.admits(value) {
        return Err(format!(
            "param {name}: {text} is outside {}..={}",
            show(lo),
            show(hi)
        ));
    }
    Ok(param)
}

/// A module directory, read but not compiled.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDir {
    pub manifest: Manifest,
    /// Where `module.st` is and how the module is scheduled.
    pub spec: Spec,
    pub source: String,
}

pub(crate) fn read_capped(path: &Path) -> Result<String, String> {
    let file = path.display();
    let f = fs::File::open(path).map_err(|e| format!("{file}: {e}"))?;
    let mut buf = Vec::new();
    f.take(MAX_FILE + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{file}: {e}"))?;
    if buf.len() as u64 > MAX_FILE {
        return Err(format!("{file}: larger than {MAX_FILE} bytes"));
    }
    String::from_utf8(buf).map_err(|_| format!("{file}: not UTF-8"))
}

impl ModuleDir {
    /// Reads `dir/module.manifest` and `dir/module.st`.
    ///
    /// # Errors
    /// A file missing, too large or not UTF-8, or a manifest that does not
    /// parse (every problem, one per line of the message).
    pub fn read(dir: &Path, d: &Defaults) -> Result<Self, String> {
        let mut md = Self::from_text(dir, &read_capped(&dir.join(FILE))?, String::new(), d)?;
        md.source = read_capped(&md.spec.path)?;
        Ok(md)
    }

    /// A module directory whose two files are already in memory (a
    /// snapshot's); `dir` is only where messages say they are.
    ///
    /// # Errors
    /// A manifest that does not parse (every problem, one per line of the
    /// message).
    pub fn from_text(dir: &Path, manifest: &str, source: String, d: &Defaults) -> Result<Self, String> {
        let manifest = Manifest::parse(manifest).map_err(|errs| {
            let path = dir.join(FILE);
            let file = path.display();
            let lines: Vec<String> = errs.iter().map(|(n, e)| format!("\n  {file}:{n}: {e}")).collect();
            format!("{file} does not parse:{}", lines.concat())
        })?;
        let spec = manifest.spec(dir, d);
        Ok(Self {
            manifest,
            spec,
            source,
        })
    }

    /// The module, from `program` compiled from [`ModuleDir::source`]
    /// against the spec's window sizes, once the manifest is checked
    /// against it.
    ///
    /// # Errors
    /// Every way the manifest and the program disagree.
    pub fn module(&self, program: Program) -> Result<Module, String> {
        self.manifest.check(&program.ports()).map_err(|errs| {
            let file = self.spec.path.with_file_name(FILE);
            let file = file.display();
            let lines: Vec<String> = errs.iter().map(|e| format!("\n  {file}: {e}")).collect();
            format!("{file} does not match {SOURCE}:{}", lines.concat())
        })?;
        Ok(Module {
            name: self.spec.name.clone(),
            program,
            period: self.spec.period,
            i: self.spec.i,
            q: self.spec.q,
            params: self.manifest.param_values(),
        })
    }
}
