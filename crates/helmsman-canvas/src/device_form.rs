//! The "New device" form's model (Nick, 2026-11-05): typed settings for
//! each protocol in place of one `KEY=VALUE` line, the device's inputs and
//! outputs as rows to tick and name (a known device lists its channels),
//! each bound to the program (a variable it has, a new one, or wiring
//! only), and renaming a wired signal in `io.map` and the ST together.
//! The form's drawing is in `device_form_ui.rs`; everything here is pure.

use std::fmt::Write as _;

use helmsman_project::iomap::MapText;

/// An input or an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    In,
    Out,
}

impl Dir {
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }

    /// The ST section a variable of this direction is declared in.
    #[must_use]
    pub fn section(self) -> &'static str {
        match self {
            Self::In => "VAR_INPUT",
            Self::Out => "VAR_OUTPUT",
        }
    }
}

/// A signal's type in the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigTy {
    Bool,
    Int,
    Real,
}

impl SigTy {
    pub const ALL: [Self; 3] = [Self::Bool, Self::Int, Self::Real];

    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Real => "real",
        }
    }

    /// The ST type a new variable is declared with.
    #[must_use]
    pub fn st(self) -> &'static str {
        match self {
            Self::Bool => "BOOL",
            Self::Int => "INT",
            Self::Real => "REAL",
        }
    }

    /// What an output is driven to when nothing healthy drives it.
    #[must_use]
    pub fn safe(self) -> &'static str {
        match self {
            Self::Bool => "safe=false",
            Self::Int | Self::Real => "safe=0",
        }
    }

    #[must_use]
    pub fn of(ty: helmsman_iec::Ty) -> Option<Self> {
        use helmsman_iec::Ty;
        match ty {
            Ty::Bool => Some(Self::Bool),
            Ty::Real | Ty::LReal => Some(Self::Real),
            Ty::SInt | Ty::Int | Ty::DInt | Ty::LInt | Ty::USInt | Ty::UInt | Ty::UDInt | Ty::ULInt => {
                Some(Self::Int)
            }
            _ => None,
        }
    }
}

/// One of the program's `VAR_INPUT`s or `VAR_OUTPUT`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub dir: Dir,
    /// Lower case, as the map writes it (ST names ignore case).
    pub name: String,
    pub ty: SigTy,
}

/// The program's inputs and outputs a map signal can bind, from its ST.
#[must_use]
pub fn ports_of(src: &str) -> Vec<Port> {
    use crate::playground::{Dir as PDir, Place};
    let s = crate::playground::Session::new(src);
    s.inputs()
        .chain(s.outputs())
        .filter(|p| p.place == Place::Port)
        .filter_map(|p| {
            Some(Port {
                dir: if p.dir == PDir::In { Dir::In } else { Dir::Out },
                name: p.name.to_ascii_lowercase(),
                ty: SigTy::of(p.ty)?,
            })
        })
        .collect()
}

/// What a point row does in the program.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Use {
    /// A line in io.map only; the program does not read or drive it yet.
    Wiring,
    /// The program gets a new variable of the row's name.
    Declare,
    /// One of the program's own variables (moved off where it was wired).
    Existing(String),
}

/// One input or output in the form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PointRow {
    pub on: bool,
    pub dir: Dir,
    pub name: String,
    pub ty: SigTy,
    /// In the driver's syntax: `di:0`, `coil:3`, `hr:5:i16`.
    pub point: String,
    #[serde(rename = "use")]
    pub use_: Use,
    /// A known device's channel: its type and point are fixed.
    pub fixed: bool,
    /// The rest of the row's line in io.map as it is (`safe=true unit=mm`),
    /// for a signal read back to edit: Save writes it as it was. `None`
    /// for a row made here, which gets the defaults.
    #[serde(default)]
    pub tail: Option<String>,
}

impl PointRow {
    /// A row the visitor adds by hand.
    #[must_use]
    pub fn custom(dir: Dir, n: usize) -> Self {
        Self {
            on: true,
            dir,
            name: format!("{}{n}", if dir == Dir::In { "in" } else { "out" }),
            ty: SigTy::Bool,
            point: String::new(),
            use_: Use::Declare,
            fixed: false,
            tail: None,
        }
    }

    /// The `in`/`out` line on `device`.
    #[must_use]
    pub fn line(&self, device: &str) -> String {
        let mut l = format!(
            "{:<3} {} {} {device} {}",
            self.dir.word(),
            self.name,
            self.ty.word(),
            self.point
        );
        match &self.tail {
            Some(t) if !t.is_empty() => {
                l.push(' ');
                l.push_str(t);
            }
            Some(_) => {}
            None if self.dir == Dir::Out => {
                l.push(' ');
                l.push_str(self.ty.safe());
            }
            None => {}
        }
        l
    }
}

/// A device the form knows the channels of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Catalog {
    /// Moxa ioLogik E1213: DI-00..11, DO-00..07 (the four DIO as DO, the
    /// factory jumper); docs/devices/moxa-iologik-e1213.md.
    E1213,
}

impl Catalog {
    /// Moxa's vendor 991, product 515, or a product name with E1213 in it.
    #[must_use]
    pub fn identify(vendor: u16, product: u16, name: &str) -> Option<Self> {
        ((vendor, product) == (991, 515) || name.to_ascii_uppercase().contains("E1213"))
            .then_some(Self::E1213)
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::E1213 => "ioLogik E1213: 12 inputs (DI-00..11) and 8 outputs (DO-00..07)",
        }
    }

    /// Every channel as a row, off, named `di0`.., `do0`.., on `driver`.
    #[must_use]
    pub fn rows(self, driver: &str) -> Vec<PointRow> {
        let Self::E1213 = self;
        let row = |dir, name: String, point: String| PointRow {
            on: false,
            dir,
            name,
            ty: SigTy::Bool,
            point,
            use_: Use::Declare,
            fixed: true,
            tail: None,
        };
        let out_kind = out_bit(driver);
        (0..12)
            .map(|n| row(Dir::In, format!("di{n}"), format!("di:{n}")))
            .chain((0..8).map(|n| row(Dir::Out, format!("do{n}"), format!("{out_kind}:{n}"))))
            .collect()
    }
}

/// An output bit's point kind: `coil` on Modbus, `do` on EtherNet/IP.
#[must_use]
pub fn out_bit(driver: &str) -> &'static str {
    if driver.starts_with("modbus") {
        "coil"
    } else {
        "do"
    }
}

/// A setting's input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A whole number in a range, with its unit.
    Num { min: u64, max: u64, unit: &'static str },
    /// One of [`PROFILES`], or none.
    Profile,
    /// One of the project's `.eds` files, or none.
    Eds,
}

/// One setting of a driver (the keys and ranges are the drivers' own:
/// helmsman-io's modbus.rs `from_params`, enip.rs `Params::parse`,
/// helmsman-plc's link.rs `poll_ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub tip: &'static str,
    pub kind: Kind,
}

const POLL: Setting = Setting {
    key: "poll_ms",
    label: "Poll every",
    tip: "How often the controller reads the device's inputs, in ms (1 to 60000).",
    kind: Kind::Num {
        min: 1,
        max: 60_000,
        unit: "ms",
    },
};

const MODBUS: [Setting; 3] = [
    Setting {
        key: "unit",
        label: "Unit id",
        tip: "The Modbus unit (slave) id the device answers to, 1 to 247. Most Ethernet devices use 1.",
        kind: Kind::Num {
            min: 1,
            max: 247,
            unit: "",
        },
    },
    Setting {
        key: "timeout_ms",
        label: "Timeout",
        tip: "How long to wait for an answer before the link counts as down, in ms (1 to 60000).",
        kind: Kind::Num {
            min: 1,
            max: 60_000,
            unit: "ms",
        },
    },
    POLL,
];

const ENIP: [Setting; 4] = [
    Setting {
        key: "profile",
        label: "Profile",
        tip: "A device this controller knows the connection of. Pick one, or the device's EDS file below.",
        kind: Kind::Profile,
    },
    Setting {
        key: "eds",
        label: "EDS file",
        tip: "The vendor's EDS file in this project (Import device… adds one). Use it when no profile fits.",
        kind: Kind::Eds,
    },
    Setting {
        key: "rpi_ms",
        label: "Update every",
        tip: "The requested packet interval: how often the device and the controller exchange I/O, in ms. The E1213 takes 100 to 3000.",
        kind: Kind::Num {
            min: 1,
            max: 3_600_000,
            unit: "ms",
        },
    },
    POLL,
];

/// The EtherNet/IP profiles the controller has (enip.rs `Params::parse`).
pub const PROFILES: [&str; 1] = ["moxa-e1213"];

/// The settings `driver` takes, in the order the form shows them.
#[must_use]
pub fn schema(driver: &str) -> &'static [Setting] {
    match driver {
        "modbus-tcp" => &MODBUS,
        "enip-io" => &ENIP,
        _ => &[],
    }
}

/// The value of `key` in a `KEY=VALUE ...` string.
#[must_use]
pub fn param<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    params
        .split_whitespace()
        .filter_map(|t| t.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// `params` with `key` set to `value` where it was (else at the end), or
/// taken out when `value` is empty. Every other token stays as it was.
#[must_use]
pub fn set_param(params: &str, key: &str, value: &str) -> String {
    let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for t in params.split_whitespace() {
        if t.split_once('=').is_some_and(|(k, _)| k == key) {
            if !done && !value.is_empty() {
                out.push(format!("{key}={value}"));
            }
            done = true;
        } else {
            out.push(t.to_string());
        }
    }
    if !done && !value.is_empty() {
        out.push(format!("{key}={value}"));
    }
    out.join(" ")
}

/// Why `params` will not do for `driver`, if they will not.
///
/// # Errors
/// The first setting out of range, an unknown profile, or EtherNet/IP
/// with neither (or both) of a profile and an EDS.
pub fn check_settings(driver: &str, params: &str) -> Result<(), String> {
    for s in schema(driver) {
        let Some(v) = param(params, s.key) else { continue };
        match s.kind {
            Kind::Num { min, max, unit } => {
                if !v.parse::<u64>().is_ok_and(|n| (min..=max).contains(&n)) {
                    return Err(format!(
                        "{}: a whole number from {min} to {max}{}",
                        s.label,
                        unit_suffix(unit)
                    ));
                }
            }
            Kind::Profile => {
                if !PROFILES.contains(&v) {
                    return Err(format!("Profile: one of {}", PROFILES.join(", ")));
                }
            }
            Kind::Eds => {}
        }
    }
    if driver == "enip-io" {
        match (param(params, "profile"), param(params, "eds")) {
            (None, None) => return Err("EtherNet/IP needs a profile or the device's EDS file.".into()),
            (Some(_), Some(_)) => return Err("A profile or an EDS file, not both.".into()),
            _ => {}
        }
    }
    Ok(())
}

fn unit_suffix(unit: &str) -> String {
    if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    }
}

/// A signal name: a letter, then letters, digits or `_`, at most 48.
#[must_use]
pub fn name_ok(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic())
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
        && !s.contains("__")
        && s.len() <= 48
}

/// Binds the program's inputs and outputs to a known device's channels,
/// in order: a BOOL port that the map has nowhere, or on the box's own
/// channels, takes the next free channel of its direction (the program
/// moves onto the device as a whole; the visitor can untick any).
pub fn prebind(rows: &mut [PointRow], ports: &[Port], map: &MapText) {
    for p in ports.iter().filter(|p| p.ty == SigTy::Bool) {
        let wired = map.signals.iter().find(|s| s.name.eq_ignore_ascii_case(&p.name));
        if wired.is_some_and(|s| s.device != "box") {
            continue;
        }
        if let Some(r) = rows
            .iter_mut()
            .find(|r| r.fixed && !r.on && r.dir == p.dir && r.ty == SigTy::Bool)
        {
            r.on = true;
            r.name.clone_from(&p.name);
            r.use_ = Use::Existing(p.name.clone());
        }
    }
}

/// Whether `point` is an address `driver` reads (the drivers' own
/// syntax, helmsman-io's `modbus::Point::parse` and `enip::Point::parse`,
/// which the page cannot link): `coil:N`, `di:N`, `hr:N[:FMT]`,
/// `ir:N[:FMT]` on Modbus; `di:N`, `do:N`, `in:BYTE.BIT`, `out:BYTE.BIT`,
/// `in:OFFSET:FMT`, `out:OFFSET:FMT` on EtherNet/IP. Any other driver's
/// points are the box's to check.
///
/// # Errors
/// What is wrong with it, in words.
pub fn point_ok(driver: &str, point: &str) -> Result<(), String> {
    let num = |s: &str| {
        !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u16>().is_ok()
    };
    match driver {
        "modbus-tcp" => {
            let mut it = point.split(':');
            let (table, addr, fmt) = (it.next().unwrap_or(""), it.next(), it.next());
            let words = match (table, fmt) {
                ("coil" | "di", None) => 1,
                ("hr" | "ir", None | Some("u16" | "i16")) => 1,
                ("hr" | "ir", Some("u32" | "u32sw" | "i32" | "i32sw" | "f32" | "f32sw")) => 2,
                _ => {
                    return Err(format!(
                        "{point:?} is not a Modbus point: coil:N, di:N, hr:N[:FMT] or ir:N[:FMT]"
                    ));
                }
            };
            let addr = addr.filter(|a| num(a)).and_then(|a| a.parse::<u16>().ok());
            match addr {
                Some(a) if it.next().is_none() && u32::from(a) + words <= 65536 => Ok(()),
                _ => Err(format!(
                    "{point:?} is not a Modbus point: an address from 0 to 65535 follows the table"
                )),
            }
        }
        "enip-io" => {
            let bad = || {
                format!(
                    "{point:?} is not an EtherNet/IP point: di:N, do:N, in:BYTE.BIT, out:BYTE.BIT, in:OFFSET:FMT or out:OFFSET:FMT"
                )
            };
            let (head, rest) = point.split_once(':').ok_or_else(bad)?;
            match head {
                "di" | "do" if num(rest) => Ok(()),
                "in" | "out" => match rest.split_once('.') {
                    Some((byte, bit)) if num(byte) && num(bit) && bit.parse::<u8>().is_ok_and(|b| b < 8) => {
                        Ok(())
                    }
                    Some(_) => Err(bad()),
                    None => match rest.split_once(':') {
                        Some((off, fmt))
                            if num(off)
                                && matches!(fmt, "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "f32") =>
                        {
                            Ok(())
                        }
                        _ => Err(bad()),
                    },
                },
                _ => Err(bad()),
            }
        }
        _ => Ok(()),
    }
}

/// What Add does: the device and its signals into io.map, the lines of
/// the program's variables taken from where they were, new variables
/// declared in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wiring {
    pub device: String,
    pub device_line: String,
    pub signals: Vec<String>,
    /// Signal names whose old lines go (they are on the device now).
    pub replace: Vec<String>,
    /// New variables: direction, name, type.
    pub declare: Vec<(Dir, String, SigTy)>,
    /// Edit: the device this one replaces, its line and its signals'.
    pub replaces: Option<String>,
}

impl Wiring {
    /// What it changes, in a line, for the notice.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut s = format!("{} added with {} point(s)", self.device, self.signals.len());
        if !self.replace.is_empty() {
            let _ = write!(s, "; {} moved onto it", self.replace.join(", "));
        }
        if !self.declare.is_empty() {
            let names: Vec<&str> = self.declare.iter().map(|(_, n, _)| n.as_str()).collect();
            let _ = write!(s, "; new variable(s) {}", names.join(", "));
        }
        s.push('.');
        s
    }
}

/// The rows ticked, checked: each a good name, used once, not wired
/// already (but for a variable that moves), a new variable's name not the
/// program's already, a point given. Then the [`Wiring`].
///
/// # Errors
/// The first row that will not do, named.
pub fn wiring(
    device: &str,
    device_line: String,
    rows: &[PointRow],
    ports: &[Port],
    map: &MapText,
) -> Result<Wiring, String> {
    let mut w = Wiring {
        device: device.to_string(),
        device_line,
        signals: Vec::new(),
        replace: Vec::new(),
        declare: Vec::new(),
        replaces: None,
    };
    let driver = w.device_line.split_whitespace().nth(2).unwrap_or("").to_string();
    let mut seen: Vec<String> = Vec::new();
    for r in rows.iter().filter(|r| r.on) {
        let what = format!("{} {}", r.dir.word(), r.point);
        if !name_ok(&r.name) {
            return Err(format!(
                "{what}: {:?} is not a name (a letter, then letters, digits or _).",
                r.name
            ));
        }
        let low = r.name.to_ascii_lowercase();
        if seen.contains(&low) {
            return Err(format!("{} is used twice.", r.name));
        }
        seen.push(low.clone());
        if r.point.trim().is_empty() || r.point.contains(char::is_whitespace) {
            return Err(format!("{}: give its point, e.g. di:0.", r.name));
        }
        if r.point.contains('#') {
            return Err(format!(
                "{}: a # starts a comment in io.map, so a point cannot hold one.",
                r.name
            ));
        }
        point_ok(&driver, &r.point).map_err(|e| format!("{}: {e}", r.name))?;
        if let Some(t) = &r.tail
            && t.contains('#')
        {
            return Err(format!("{}: a # starts a comment in io.map.", r.name));
        }
        let wired = map.signals.iter().any(|s| s.name.eq_ignore_ascii_case(&r.name));
        let port = ports.iter().find(|p| p.name.eq_ignore_ascii_case(&r.name));
        match &r.use_ {
            Use::Existing(v) => {
                let Some(p) = ports.iter().find(|p| p.name.eq_ignore_ascii_case(v)) else {
                    return Err(format!("The program has no variable {v}."));
                };
                if p.dir != r.dir || p.ty != r.ty {
                    return Err(format!(
                        "{v} is a {} {}, not a {} {}.",
                        p.ty.word(),
                        p.dir.word(),
                        r.ty.word(),
                        r.dir.word()
                    ));
                }
                if wired {
                    w.replace.push(low);
                }
            }
            Use::Declare => {
                if port.is_some() {
                    return Err(format!(
                        "The program already has {}: pick it in the Program list.",
                        r.name
                    ));
                }
                if wired {
                    return Err(format!("{} is wired already in io.map.", r.name));
                }
                w.declare.push((r.dir, r.name.clone(), r.ty));
            }
            Use::Wiring => {
                if wired {
                    return Err(format!("{} is wired already in io.map.", r.name));
                }
            }
        }
        w.signals.push(r.line(device));
    }
    Ok(w)
}

/// `map` with the replaced signals' lines taken out and the device and
/// its signals added at the end. An edit (`replaces`) is made in place:
/// the device's line and each of its signals' lines keep their place and
/// their trailing comment, the lines above them stay above them, signals
/// it no longer has go, and new ones follow the last it had.
#[must_use]
pub fn apply_map(map: &str, w: &Wiring) -> String {
    let mut out = String::new();
    for line in map.lines() {
        let t: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        let gone = matches!(t.as_slice(), [d, name, ..] if (*d == "in" || *d == "out") && w.replace.iter().any(|r| r.eq_ignore_ascii_case(name)));
        if !gone {
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some(old) = &w.replaces
        && let Some(edited) = edit_device(&out, old, w)
    {
        return edited;
    }
    if let Some(old) = &w.replaces {
        out = remove_device(&out, old);
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(w.device_line.trim_end());
    out.push('\n');
    for s in &w.signals {
        out.push_str(s);
        out.push('\n');
    }
    out
}

/// `line` (one line of io.map, its newline included) with `new` in place
/// of what is before its comment, the comment kept.
fn with_comment_of(line: &str, new: &str) -> String {
    match line.find('#') {
        Some(at) => format!("{new}  {}", line[at..].trim_end_matches(['\n', '\r']).trim_end()) + "\n",
        None => format!("{new}\n"),
    }
}

/// `map` with device `old` rewritten as `w` has it, in place; `None` when
/// the map has no such device line.
fn edit_device(map: &str, old: &str, w: &Wiring) -> Option<String> {
    // A row is the line it was read from by its name, else by its point.
    let key = |l: &str| {
        let t: Vec<&str> = l.split_whitespace().collect();
        (
            t.get(1).map(|n| n.to_ascii_lowercase()),
            t.first()
                .map(|d| (*d).to_string())
                .zip(t.get(4).map(|p| (*p).to_string())),
        )
    };
    let mut fresh: Vec<_> = w.signals.iter().map(|l| (key(l), l)).collect();
    let mut out = String::with_capacity(map.len() + 64);
    // Where the device's last line ends in `out`.
    let mut after = None;
    for line in map.split_inclusive('\n') {
        let t: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        match t.as_slice() {
            ["device", d, ..] if *d == old && after.is_none() => {
                out.push_str(&with_comment_of(line, w.device_line.trim_end()));
                after = Some(out.len());
            }
            [dir, name, _, d, point, ..] if (*dir == "in" || *dir == "out") && *d == old => {
                let lower = name.to_ascii_lowercase();
                let by_point = ((*dir).to_string(), (*point).to_string());
                let found = fresh
                    .iter()
                    .position(|((n, _), _)| n.as_deref() == Some(lower.as_str()))
                    .or_else(|| fresh.iter().position(|((_, p), _)| p.as_ref() == Some(&by_point)));
                if let Some(k) = found {
                    let (_, new) = fresh.remove(k);
                    out.push_str(&with_comment_of(line, new));
                    after = Some(out.len());
                }
            }
            _ => out.push_str(line),
        }
    }
    let at = after?;
    let mut added = String::new();
    for (_, l) in fresh {
        added.push_str(l);
        added.push('\n');
    }
    out.insert_str(at, &added);
    Some(out)
}

/// `map` without device `name`: its `device` line and every signal on it.
/// Comments, blank lines and everything else stay as they were. Device ids
/// are exact, as on the box (`Pump` and `pump` are two devices).
#[must_use]
pub fn remove_device(map: &str, name: &str) -> String {
    let mut out = String::with_capacity(map.len());
    for line in map.split_inclusive('\n') {
        let t: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        let gone = match t.as_slice() {
            ["device", d, ..] => *d == name,
            [dir, _, _, d, ..] if *dir == "in" || *dir == "out" => *d == name,
            _ => false,
        };
        if !gone {
            out.push_str(line);
        }
    }
    out
}

/// The map without `device`'s signals, for checking an edit's rows (they
/// are the device's own, not wired elsewhere), and without the device.
#[must_use]
pub fn map_without(map: &MapText, device: &str) -> MapText {
    let mut m = map.clone();
    m.devices.retain(|d| d.name != device);
    m.signals.retain(|s| s.device != device);
    m
}

impl crate::devices::NewDevice {
    /// The form for device `name` as `map` has it, to edit: its settings,
    /// and its signals as rows (on a known device, ticked on their
    /// channels; else rows of their own), each the program's variable
    /// when it has one of that name. `None` for a driver the form does not
    /// offer (edit those in io.map) or a device the map does not declare.
    #[must_use]
    pub fn from_map(map: &MapText, name: &str, ports: &[Port]) -> Option<Self> {
        use crate::devices::PROTOCOLS;
        let dev = map.devices.iter().find(|d| d.name == name)?;
        let protocol = PROTOCOLS.iter().position(|p| p.0 == dev.driver)?;
        let mut params: String = dev.settings.join(" ");
        let (vendor, product) = dev
            .endpoint
            .strip_prefix("cip=")
            .map(|c| {
                let mut it = c.split(':').map(|x| x.parse::<u16>().unwrap_or(0));
                (it.next().unwrap_or(0), it.next().unwrap_or(0))
            })
            .unwrap_or_default();
        let known_profile = param(&params, "profile") == Some("moxa-e1213");
        let catalog = if known_profile {
            Some(Catalog::E1213)
        } else {
            Catalog::identify(vendor, product, &dev.name)
        };
        // An E1213 on EtherNet/IP added before the form knew profiles.
        if catalog.is_some()
            && dev.driver == "enip-io"
            && param(&params, "profile").is_none()
            && param(&params, "eds").is_none()
        {
            params = set_param(&params, "profile", "moxa-e1213");
        }
        let mut points = catalog.map(|c| c.rows(&dev.driver)).unwrap_or_default();
        let mut extra = 0;
        for s in map.signals.iter().filter(|s| s.device == dev.name) {
            let dir = if s.dir == "out" { Dir::Out } else { Dir::In };
            let ty = match s.ty.to_ascii_lowercase().as_str() {
                "int" => SigTy::Int,
                "real" => SigTy::Real,
                _ => SigTy::Bool,
            };
            let use_ = if ports.iter().any(|p| p.name.eq_ignore_ascii_case(&s.name)) {
                Use::Existing(s.name.to_ascii_lowercase())
            } else {
                Use::Wiring
            };
            if let Some(r) = points
                .iter_mut()
                .find(|r| r.fixed && !r.on && r.dir == dir && r.point == s.point)
            {
                r.on = true;
                r.name.clone_from(&s.name);
                r.use_ = use_;
                r.tail = Some(s.tail.join(" "));
            } else {
                let mut r = PointRow::custom(dir, extra);
                extra += 1;
                r.name.clone_from(&s.name);
                r.ty = ty;
                r.point.clone_from(&s.point);
                r.use_ = use_;
                r.tail = Some(s.tail.join(" "));
                points.push(r);
            }
        }
        Some(Self {
            name: dev.name.clone(),
            protocol,
            endpoint: dev.endpoint.clone(),
            eds: param(&params, "eds").map(str::to_string),
            params,
            error: None,
            points,
            catalog,
            editing: Some(dev.name.clone()),
        })
    }
}

/// `src` with `name : TYPE;` declared in its program's first section for
/// `dir` (`VAR_INPUT` / `VAR_OUTPUT`), or a new section after the
/// `PROGRAM` line when it has none. `None` when there is no program.
#[must_use]
pub fn declare_var(src: &str, dir: Dir, name: &str, ty: SigTy) -> Option<String> {
    let decl = format!("  {name} : {};\n", ty.st());
    let first_word = |l: &str| {
        l.split(|c: char| c.is_whitespace() || c == '(')
            .find(|w| !w.is_empty())
            .map(str::to_ascii_uppercase)
    };
    let mut out = String::with_capacity(src.len() + decl.len() + 24);
    let mut done = false;
    let mut program_at = None;
    // Only the program's own sections count: a function block ahead of it
    // has a VAR_INPUT of its own.
    let mut in_program = false;
    for line in src.split_inclusive('\n') {
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
        let w = first_word(line);
        if in_program && !done && w.as_deref() == Some(dir.section()) {
            out.push_str(&decl);
            done = true;
        }
        match w.as_deref() {
            Some("PROGRAM") if program_at.is_none() => {
                program_at = Some(out.len());
                in_program = true;
            }
            Some("END_PROGRAM") => in_program = false,
            _ => {}
        }
    }
    if done {
        if !src.ends_with('\n') {
            out.pop();
        }
        return Some(out);
    }
    let at = program_at?;
    out.insert_str(at, &format!("{}\n{decl}END_VAR\n", dir.section()));
    if !src.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

fn is_st(file: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("st"))
}

/// The project's file with the `PROGRAM` in it: a `.st` that is not a
/// requirement test (`test_*.st`).
#[must_use]
pub fn program_file<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<String> {
    files
        .into_iter()
        .filter(|(n, _)| is_st(n) && !n.starts_with("test_"))
        .find(|(_, t)| {
            t.lines()
                .any(|l| l.trim_start().to_ascii_uppercase().starts_with("PROGRAM "))
        })
        .map(|(n, _)| n.to_string())
}

/// Renaming a wired signal: its io.map line and the variable everywhere
/// in the ST, file by file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenamePlan {
    pub old: String,
    pub new: String,
    /// Each file that changes: name, new text, how many uses.
    pub files: Vec<(String, String, usize)>,
}

impl RenamePlan {
    /// What changes, a line a file.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|(f, _, n)| {
                if f == helmsman_project::IO_MAP {
                    format!("{f}: the wiring line")
                } else {
                    format!("{f}: {n} use(s)")
                }
            })
            .collect()
    }
}

/// The plan to rename signal `old` to `new` across `files` (io.map and
/// every `.st`, the requirement tests too).
///
/// # Errors
/// A bad or taken name, or an ST file that does not read (fix it first).
pub fn rename_plan(files: &[(String, String)], old: &str, new: &str) -> Result<RenamePlan, String> {
    if !name_ok(new) {
        return Err(format!(
            "{new:?} is not a name: a letter, then letters, digits or _."
        ));
    }
    if new.eq_ignore_ascii_case(old) && new != old {
        // Only the case changes: allowed, ST ignores it.
    } else if new == old {
        return Err("That is its name already.".into());
    }
    let mut plan = RenamePlan {
        old: old.to_string(),
        new: new.to_string(),
        files: Vec::new(),
    };
    for (f, text) in files {
        if f == helmsman_project::IO_MAP {
            let mut n = 0;
            let mut out = String::with_capacity(text.len());
            for line in text.split_inclusive('\n') {
                let body = line.split('#').next().unwrap_or("");
                let t: Vec<&str> = body.split_whitespace().collect();
                match t.as_slice() {
                    [d, name, ..] if (*d == "in" || *d == "out") && name.eq_ignore_ascii_case(old) => {
                        // Swap the name token in place, spacing kept.
                        let lead = line.len() - line.trim_start().len();
                        let after = lead + d.len();
                        let off = after + (line[after..].len() - line[after..].trim_start().len());
                        out.push_str(&line[..off]);
                        out.push_str(new);
                        out.push_str(&line[off + name.len()..]);
                        n += 1;
                    }
                    [d, name, ..]
                        if (*d == "in" || *d == "out")
                            && name.eq_ignore_ascii_case(new)
                            && !new.eq_ignore_ascii_case(old) =>
                    {
                        return Err(format!("{new} is wired already in io.map."));
                    }
                    _ => out.push_str(line),
                }
            }
            if n > 0 {
                plan.files.push((f.clone(), out, n));
            }
        } else if is_st(f) {
            if !new.eq_ignore_ascii_case(old)
                && ports_of(text).iter().any(|p| p.name.eq_ignore_ascii_case(new))
            {
                return Err(format!("The program already has a variable {new}."));
            }
            let (out, n) = helmsman_iec::rename_ident(text, old, new)
                .map_err(|_| format!("{f} does not read as ST: fix it first, then rename."))?;
            if n == 0 {
                continue;
            }
            if helmsman_iec::ast::parse(text).is_ok() && helmsman_iec::ast::parse(&out).is_err() {
                return Err(format!("{new} cannot be a variable's name (an ST keyword?)."));
            }
            plan.files.push((f.clone(), out, n));
        }
    }
    if plan.files.is_empty() {
        return Err(format!("{old} is nowhere to rename."));
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helmsman_project::iomap::skim;

    const LATCH: &str = "PROGRAM latch\nVAR_INPUT\n  start : BOOL;\n  stop : BOOL;\nEND_VAR\nVAR_OUTPUT\n  motor : BOOL;\nEND_VAR\nmotor := (start OR motor) AND NOT stop;\nEND_PROGRAM\n";
    const BOX_MAP: &str = "# the example's\nin  start bool box di:0\nin  stop bool box di:1\nout motor bool box do:0 safe=false\n";

    #[test]
    fn settings_are_set_in_place_and_checked_against_the_drivers_ranges() {
        let p = "unit=1 timeout_ms=300 poll_ms=20 extra=x";
        assert_eq!(param(p, "timeout_ms"), Some("300"));
        assert_eq!(
            set_param(p, "timeout_ms", "500"),
            "unit=1 timeout_ms=500 poll_ms=20 extra=x"
        );
        assert_eq!(set_param(p, "unit", ""), "timeout_ms=300 poll_ms=20 extra=x");
        assert_eq!(
            set_param(p, "new", "a b"),
            "unit=1 timeout_ms=300 poll_ms=20 extra=x new=ab"
        );
        assert_eq!(check_settings("modbus-tcp", p), Ok(()));
        assert!(
            check_settings("modbus-tcp", "unit=0")
                .unwrap_err()
                .contains("1 to 247")
        );
        assert!(check_settings("modbus-tcp", "poll_ms=abc").is_err());
        assert!(check_settings("modbus-tcp", "timeout_ms=60001").is_err());
        assert_eq!(check_settings("enip-io", "profile=moxa-e1213 rpi_ms=100"), Ok(()));
        assert!(
            check_settings("enip-io", "rpi_ms=100")
                .unwrap_err()
                .contains("profile or")
        );
        assert!(
            check_settings("enip-io", "profile=moxa-e1213 eds=a.eds")
                .unwrap_err()
                .contains("not both")
        );
        assert!(check_settings("enip-io", "profile=other").is_err());
        assert_eq!(check_settings("enip-io", "eds=moxa.eds"), Ok(()));
    }

    #[test]
    fn the_e1213_lists_its_channels_and_takes_the_programs_box_io() {
        let mut rows = Catalog::E1213.rows("enip-io");
        assert_eq!(rows.iter().filter(|r| r.dir == Dir::In).count(), 12);
        assert_eq!(rows.iter().filter(|r| r.dir == Dir::Out).count(), 8);
        assert_eq!(Catalog::E1213.rows("modbus-tcp")[12].point, "coil:0");
        let ports = ports_of(LATCH);
        assert_eq!(ports.len(), 3, "{ports:?}");
        prebind(&mut rows, &ports, &skim(BOX_MAP));
        let on: Vec<(&str, &str)> = rows
            .iter()
            .filter(|r| r.on)
            .map(|r| (r.name.as_str(), r.point.as_str()))
            .collect();
        assert_eq!(on, [("start", "di:0"), ("stop", "di:1"), ("motor", "do:0")]);
        let w = wiring(
            "e1213",
            "device e1213 enip-io cip=991:515 profile=moxa-e1213\n".into(),
            &rows,
            &ports,
            &skim(BOX_MAP),
        )
        .unwrap();
        assert_eq!(w.replace, ["start", "stop", "motor"]);
        assert_eq!(w.declare, []);
        let map = apply_map(BOX_MAP, &w);
        assert!(!map.contains(" box "), "{map}");
        assert!(
            map.contains("device e1213 enip-io cip=991:515 profile=moxa-e1213\nin  start bool e1213 di:0\n"),
            "{map}"
        );
        assert!(map.contains("out motor bool e1213 do:0 safe=false\n"), "{map}");
        assert_eq!(skim(&map).odd, 0, "{map}");
    }

    #[test]
    fn a_program_wired_elsewhere_is_left_where_it_is() {
        let mut rows = Catalog::E1213.rows("enip-io");
        prebind(&mut rows, &ports_of(LATCH), &skim("in start bool rio di:4\n"));
        let on: Vec<&str> = rows.iter().filter(|r| r.on).map(|r| r.name.as_str()).collect();
        assert_eq!(on, ["stop", "motor"]);
    }

    #[test]
    fn rows_that_will_not_do_are_refused_by_name() {
        let ports = ports_of(LATCH);
        let map = skim(BOX_MAP);
        let w = |rows: &[PointRow]| {
            wiring(
                "d",
                "device d enip-io 1.2.3.4 profile=moxa-e1213\n".into(),
                rows,
                &ports,
                &map,
            )
        };
        let mut r = PointRow::custom(Dir::In, 0);
        r.point = "di:3".into();
        r.name = "2x".into();
        assert!(w(&[r.clone()]).unwrap_err().contains("not a name"));
        r.name = "start".into();
        assert!(
            w(&[r.clone()])
                .unwrap_err()
                .contains("pick it in the Program list")
        );
        r.name = "level".into();
        r.point = String::new();
        assert!(w(&[r.clone()]).unwrap_err().contains("give its point"));
        r.point = "di:3".into();
        assert!(w(&[r.clone(), r.clone()]).unwrap_err().contains("used twice"));
        let mut e = r.clone();
        e.use_ = Use::Existing("motor".into());
        e.name = "motor".into();
        assert!(w(&[e]).unwrap_err().contains("not a bool in"));
        let ok = w(&[r]).unwrap();
        assert_eq!(ok.declare, [(Dir::In, "level".to_string(), SigTy::Bool)]);
        assert_eq!(ok.signals, ["in  level bool d di:3"]);
    }

    #[test]
    fn a_new_variable_is_declared_in_its_section_or_a_new_one() {
        let out = declare_var(LATCH, Dir::In, "door", SigTy::Bool).unwrap();
        assert!(
            out.contains("VAR_INPUT\n  door : BOOL;\n  start : BOOL;"),
            "{out}"
        );
        assert!(helmsman_iec::ast::parse(&out).is_ok());
        let bare = "PROGRAM p\nVAR\n  n : INT;\nEND_VAR\nn := n + 1;\nEND_PROGRAM";
        let out = declare_var(bare, Dir::Out, "lamp", SigTy::Bool).unwrap();
        assert!(
            out.starts_with("PROGRAM p\nVAR_OUTPUT\n  lamp : BOOL;\nEND_VAR\nVAR\n"),
            "{out}"
        );
        assert!(!out.ends_with('\n'), "the file's last line kept as it was");
        assert!(helmsman_iec::ast::parse(&out).is_ok(), "{out}");
        assert_eq!(
            declare_var("FUNCTION_BLOCK f\nEND_FUNCTION_BLOCK\n", Dir::In, "x", SigTy::Int),
            None
        );
        // VAR_IN_OUT is not VAR_INPUT.
        let io = "PROGRAM p\nVAR_IN_OUT\n  a : INT;\nEND_VAR\nEND_PROGRAM\n";
        assert!(
            declare_var(io, Dir::In, "x", SigTy::Int)
                .unwrap()
                .contains("PROGRAM p\nVAR_INPUT\n  x : INT;\nEND_VAR\nVAR_IN_OUT")
        );
    }

    #[test]
    fn rename_changes_the_wiring_and_the_st_together() {
        let files = vec![
            (
                "io.map".to_string(),
                "in  start bool e1213 di:0 # the green one\nout motor bool e1213 do:0 safe=false\n"
                    .to_string(),
            ),
            ("main.st".to_string(), LATCH.to_string()),
            ("test_start.st".to_string(), "(* start starts it *)\n".to_string()),
        ];
        let plan = rename_plan(&files, "start", "green_button").unwrap();
        assert_eq!(plan.lines(), ["io.map: the wiring line", "main.st: 2 use(s)"]);
        assert_eq!(
            plan.files[0].1,
            "in  green_button bool e1213 di:0 # the green one\nout motor bool e1213 do:0 safe=false\n"
        );
        assert!(
            plan.files[1]
                .1
                .contains("motor := (green_button OR motor) AND NOT stop;")
        );
        assert!(
            rename_plan(&files, "start", "motor")
                .unwrap_err()
                .contains("wired already")
        );
        assert!(
            rename_plan(&files, "start", "stop")
                .unwrap_err()
                .contains("already has")
        );
        assert!(rename_plan(&files, "start", "start").is_err());
        assert!(rename_plan(&files, "start", "9lives").is_err());
        assert!(
            rename_plan(&files, "start", "END_VAR").is_err(),
            "a keyword breaks the program"
        );
        assert!(rename_plan(&files, "nothing", "x").is_err());
        // Only the case: allowed.
        assert!(rename_plan(&files, "start", "Start").is_ok());
    }

    #[test]
    fn delete_takes_the_device_and_its_signals_and_nothing_else() {
        let map = "# bench\ndevice e1213 enip-io 10.0.0.57 profile=moxa-e1213\nin  start bool e1213 di:0\nout lamp bool box do:0 safe=false\nout motor bool E1213 do:0 # caps\n\ndevice rio modbus-tcp 10.0.0.9 unit=1\nin  e1213x bool rio di:3\n";
        // Exact, as the box reads ids: `E1213` is another device.
        let out = remove_device(map, "e1213");
        assert_eq!(
            out,
            "# bench\nout lamp bool box do:0 safe=false\nout motor bool E1213 do:0 # caps\n\ndevice rio modbus-tcp 10.0.0.9 unit=1\nin  e1213x bool rio di:3\n"
        );
        assert_eq!(remove_device(map, "nothing"), map);
        assert_eq!(remove_device("", "x"), "");
    }

    #[test]
    fn edit_reads_an_old_e1213_back_into_the_form_and_saves_in_place() {
        use crate::devices::NewDevice;
        // As #237 wrote it: no profile, the program's start on DI-00, a
        // hand-made counter row.
        let text = "in  stop bool box di:1\ndevice e1213 enip-io cip=991:515:0x00e12130 rpi_ms=100 poll_ms=20\nin  start bool e1213 di:0\nin  parts int e1213 in:2:u32\n";
        let map = skim(text);
        let ports = ports_of(LATCH);
        let d = NewDevice::from_map(&map, "e1213", &ports).unwrap();
        assert_eq!(d.editing.as_deref(), Some("e1213"));
        assert_eq!(d.catalog, Some(Catalog::E1213), "known by its identity");
        assert_eq!(
            d.params, "rpi_ms=100 poll_ms=20 profile=moxa-e1213",
            "the profile it lacked"
        );
        assert_eq!(check_settings(d.driver(), &d.params), Ok(()));
        let on: Vec<(&str, &str)> = d
            .points
            .iter()
            .filter(|r| r.on)
            .map(|r| (r.name.as_str(), r.point.as_str()))
            .collect();
        assert_eq!(on, [("start", "di:0"), ("parts", "in:2:u32")]);
        assert_eq!(d.points[0].use_, Use::Existing("start".into()));
        assert_eq!(
            d.points.last().unwrap().use_,
            Use::Wiring,
            "parts is not a program variable"
        );
        // Save: rename it and add stop; the old lines go, the new ones come.
        let mut rows = d.points.clone();
        rows[1].on = true;
        rows[1].name = "stop".into();
        rows[1].use_ = Use::Existing("stop".into());
        let mut w = wiring(
            "rio",
            "device rio enip-io 10.0.0.57 profile=moxa-e1213\n".into(),
            &rows,
            &ports,
            &map_without(&map, "e1213"),
        )
        .unwrap();
        w.replaces = Some("e1213".into());
        let out = apply_map(text, &w);
        assert!(!out.contains("device e1213") && !out.contains(" e1213 "), "{out}");
        assert!(out.contains("device rio enip-io 10.0.0.57 profile=moxa-e1213\nin  start bool rio di:0\nin  parts int rio in:2:u32\nin  stop bool rio di:1\n"), "{out}");
        assert!(!out.contains("box di:1"), "stop moved off the box: {out}");
        assert!(out.contains("in  parts int rio in:2:u32\n"), "{out}");
        assert_eq!(skim(&out).odd, 0, "{out}");
        // A driver the form does not offer is not edited here.
        assert!(NewDevice::from_map(&skim("device plc logix-tag 10.0.0.2\n"), "plc", &ports).is_none());
    }

    #[test]
    fn rename_in_the_map_keeps_the_spacing_and_a_name_inside_another_token() {
        let files = vec![(
            "io.map".to_string(),
            "in   st bool box di:0 unit=st\nin start bool box di:1\n".to_string(),
        )];
        let plan = rename_plan(&files, "st", "go").unwrap();
        assert_eq!(
            plan.files[0].1,
            "in   go bool box di:0 unit=st\nin start bool box di:1\n"
        );
    }

    const BLOCK_FIRST: &str = "FUNCTION_BLOCK fb\nVAR_INPUT\n  a : INT;\nEND_VAR\nEND_FUNCTION_BLOCK\n\nPROGRAM p\nVAR_INPUT\n  s : BOOL;\nEND_VAR\nVAR_OUTPUT\n  o : BOOL;\nEND_VAR\no := s;\nEND_PROGRAM\n";

    #[test]
    fn a_variable_goes_in_the_programs_section_not_a_function_blocks_ahead_of_it() {
        let out = declare_var(BLOCK_FIRST, Dir::In, "door", SigTy::Bool).unwrap();
        assert!(
            out.starts_with("FUNCTION_BLOCK fb\nVAR_INPUT\n  a : INT;\nEND_VAR\n"),
            "the block's inputs are as they were: {out}"
        );
        assert!(
            out.contains("PROGRAM p\nVAR_INPUT\n  door : BOOL;\n  s : BOOL;\n"),
            "{out}"
        );
        let ports = ports_of(&out);
        assert!(
            ports.iter().any(|p| p.name == "door" && p.dir == Dir::In),
            "{ports:?}"
        );
        // No section of that kind in the program: a new one after PROGRAM.
        let no_out = BLOCK_FIRST.replace("VAR_OUTPUT\n  o : BOOL;\nEND_VAR\no := s;\n", "");
        let no_out = no_out.replace(
            "FUNCTION_BLOCK fb\n",
            "FUNCTION_BLOCK fb\nVAR_OUTPUT\n  q : BOOL;\nEND_VAR\n",
        );
        let out = declare_var(&no_out, Dir::Out, "lamp", SigTy::Bool).unwrap();
        assert!(
            out.contains("PROGRAM p\nVAR_OUTPUT\n  lamp : BOOL;\nEND_VAR\n"),
            "{out}"
        );
        assert!(
            out.starts_with("FUNCTION_BLOCK fb\nVAR_OUTPUT\n  q : BOOL;\nEND_VAR\n"),
            "{out}"
        );
        assert!(helmsman_iec::ast::parse(&out).is_ok(), "{out}");
        // A function block after the program is left alone too.
        let after =
            "PROGRAM p\nEND_PROGRAM\nFUNCTION_BLOCK fb\nVAR_INPUT\n  a : INT;\nEND_VAR\nEND_FUNCTION_BLOCK\n";
        let out = declare_var(after, Dir::In, "x", SigTy::Int).unwrap();
        assert!(
            out.starts_with("PROGRAM p\nVAR_INPUT\n  x : INT;\nEND_VAR\nEND_PROGRAM\n"),
            "{out}"
        );
    }

    const RICH: &str = "# the bench\ndevice other modbus-tcp 10.0.0.1:502\n\n# the pump's I/O block\ndevice Rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300\nin  start bool Rio di:0  # green\nout motor bool Rio coil:0 safe=true unit=run\nin  level real Rio ir:2:f32 unit=mm poll_ms=500\n\nin  stop bool other di:1\n";

    #[test]
    fn saving_an_edit_keeps_what_the_form_has_no_field_for_and_the_lines_where_they_were() {
        use crate::devices::NewDevice;
        let map = skim(RICH);
        let ports = ports_of(LATCH);
        let d = NewDevice::from_map(&map, "Rio", &ports).unwrap();
        let tails: Vec<_> = d.points.iter().map(|r| r.tail.clone()).collect();
        assert_eq!(
            tails,
            [
                Some(String::new()),
                Some("safe=true unit=run".into()),
                Some("unit=mm poll_ms=500".into())
            ]
        );
        // Save with the endpoint changed and nothing else.
        let w = wiring(
            "Rio",
            "device Rio modbus-tcp 10.0.0.58:502 unit=1 timeout_ms=300\n".into(),
            &d.points,
            &ports,
            &map_without(&map, "Rio"),
        )
        .map(|mut w| {
            w.replaces = Some("Rio".into());
            w
        })
        .unwrap();
        let out = apply_map(RICH, &w);
        assert_eq!(out, RICH.replace("10.0.0.57:502 unit=1", "10.0.0.58:502 unit=1"));
        // A safe value, a unit and the comments survive a rename of a signal's row too.
        let mut rows = d.points.clone();
        rows[0].name = "go".into();
        rows[0].use_ = Use::Wiring;
        let mut w = wiring(
            "Rio",
            "device Rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300\n".into(),
            &rows[..2],
            &ports,
            &map_without(&map, "Rio"),
        )
        .unwrap();
        w.replaces = Some("Rio".into());
        let out = apply_map(RICH, &w);
        assert!(out.contains("# the pump's I/O block\ndevice Rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300\nin  go bool Rio di:0  # green\nout motor bool Rio coil:0 safe=true unit=run\n"), "{out}");
        assert!(!out.contains("level"), "the row left out is gone: {out}");
        assert!(out.contains("# the bench\ndevice other"), "{out}");
        assert!(out.ends_with("in  stop bool other di:1\n"), "{out}");
        assert_eq!(skim(&out).odd, 0, "{out}");
        // A new row follows the device's last line.
        let mut rows = d.points.clone();
        let mut extra = PointRow::custom(Dir::In, 0);
        extra.name = "door".into();
        extra.point = "di:5".into();
        rows.push(extra);
        let mut w = wiring(
            "Rio",
            "device Rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300\n".into(),
            &rows,
            &ports,
            &map_without(&map, "Rio"),
        )
        .unwrap();
        w.replaces = Some("Rio".into());
        let out = apply_map(RICH, &w);
        assert!(
            out.contains("poll_ms=500\nin  door bool Rio di:5\n\nin  stop bool other di:1\n"),
            "{out}"
        );
    }

    #[test]
    fn a_device_line_keeps_a_setting_the_form_does_not_know_and_says_when_one_is_not_a_pair() {
        use crate::devices::NewDevice;
        let map = skim("device Rio modbus-tcp 10.0.0.57:502 unit=1 vendor_flag=7 stray\n");
        let d = NewDevice::from_map(&map, "Rio", &[]).unwrap();
        assert_eq!(d.params, "unit=1 vendor_flag=7 stray", "nothing dropped");
        assert!(d.line().unwrap_err().contains("KEY=VALUE"), "so the form says so");
    }

    #[test]
    fn device_names_are_exact_as_on_the_box() {
        use crate::devices::NewDevice;
        let text = "device Pump modbus-tcp 10.0.0.9:502\nin  run bool Pump di:0\nin  spare bool pump di:1\ndevice pump modbus-tcp 10.0.0.10:502\n";
        assert_eq!(
            remove_device(text, "Pump"),
            "in  spare bool pump di:1\ndevice pump modbus-tcp 10.0.0.10:502\n"
        );
        let map = skim(text);
        let d = NewDevice::from_map(&map, "Pump", &[]).unwrap();
        let w = wiring(
            "Pump",
            "device Pump modbus-tcp 10.0.0.9:503\n".into(),
            &d.points,
            &[],
            &map_without(&map, "Pump"),
        )
        .map(|mut w| {
            w.replaces = Some("Pump".into());
            w
        })
        .unwrap();
        let out = apply_map(text, &w);
        assert!(out.contains("in  spare bool pump di:1\n"), "{out}");
        assert!(out.contains("device pump modbus-tcp 10.0.0.10:502\n"), "{out}");
        assert!(
            out.contains("device Pump modbus-tcp 10.0.0.9:503\nin  run bool Pump di:0\n"),
            "{out}"
        );
        assert_eq!(map_without(&map, "Pump").devices.len(), 1);
    }

    #[test]
    fn points_are_checked_against_the_drivers_syntax_and_a_hash_is_refused() {
        let ports = ports_of(LATCH);
        let map = skim("");
        let line = |driver: &str| format!("device d {driver} 10.0.0.9\n");
        let w = |driver: &str, point: &str| {
            let mut r = PointRow::custom(Dir::In, 0);
            r.name = "level".into();
            r.point = point.into();
            wiring("d", line(driver), &[r], &ports, &map).map(|_| ())
        };
        for good in [
            "coil:0",
            "di:65535",
            "hr:5",
            "hr:5:i16",
            "ir:7:f32",
            "ir:65534:u32sw",
        ] {
            assert_eq!(w("modbus-tcp", good), Ok(()), "{good}");
        }
        for bad in [
            "coil",
            "coil:",
            "coil:x",
            "coil:65536",
            "coil:1:u16",
            "hr:1:q",
            "hr:65535:f32",
            "hr:1:i16:x",
            "xx:1",
            "di:0#",
            "# di:0",
            "hr:-1",
        ] {
            assert!(w("modbus-tcp", bad).is_err(), "{bad}");
        }
        for good in ["di:3", "do:7", "in:2.1", "out:0.7", "in:4:u32", "out:0:f32"] {
            assert_eq!(w("enip-io", good), Ok(()), "{good}");
        }
        for bad in [
            "di", "di:x", "in:2.8", "in:2", "in:2:q", "in:x.1", "out", "a:1", "do:99999",
        ] {
            assert!(w("enip-io", bad).is_err(), "{bad}");
        }
        // A driver the page has no checker for is the box's to judge.
        assert_eq!(w("logix-tag", "Program:Main.Tag[3]"), Ok(()));
        assert!(w("logix-tag", "Tag#1").unwrap_err().contains('#'));
        let mut r = PointRow::custom(Dir::Out, 0);
        r.name = "lamp".into();
        r.point = "coil:0".into();
        r.tail = Some("safe=true # x".into());
        assert!(wiring("d", line("modbus-tcp"), &[r], &ports, &map).is_err());
    }
}
