//! Tying a program's inputs and outputs to the real world, one line of
//! io.map at a time. A program's ports start on simulated or on-screen
//! I/O (`in start bool box di:0`, `out speed int screen speed safe=0`);
//! as devices are added each of their channels gets a signal line with a
//! generated name (`in rio_di_04 bool rio ch:DI-04`). The box binds a port
//! to the map signal of the same name, so assigning port `start` to
//! channel DI-04 of `rio` renames that channel's line to `start` and takes
//! the port's old simulated line out; "back to simulated I/O" does the
//! reverse. Everything else in the file stays as it was, byte for byte.
//!
//! Plain data and text, no egui: the three places that offer the gesture
//! (the I/O tab, the diagram, the Devices tab) draw their own menus from
//! the answers here, and the two views (simulation and box) apply a
//! change by writing the text this returns.

use helmsman_project::iomap::{self, MapText, RESERVED};

use crate::device_form::{Dir, Port, SigTy};

/// Where a program port is bound now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// On a device's channel.
    Device { device: String, point: String },
    /// Another module's output (`module`).
    Module(String),
    /// On the box's own channels or the screen, or no line at all.
    Simulated,
}

impl Place {
    /// What the I/O tab's row says about it.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Device { device, point } => format!("On {device}, {}", point_label(point)),
            Self::Module(m) => format!("From module {m}"),
            Self::Simulated => "Simulated: not assigned to a device yet".to_string(),
        }
    }
}

/// A point as a person reads it: `ch:DI-04` is channel DI-04.
#[must_use]
pub fn point_label(point: &str) -> &str {
    point.strip_prefix("ch:").unwrap_or(point)
}

/// One signal of a device, as the map lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub dir: Dir,
    /// The signal's name now.
    pub name: String,
    pub ty: Option<SigTy>,
    pub device: String,
    pub point: String,
}

impl Channel {
    /// `rio, DI-04`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}, {}", self.device, point_label(&self.point))
    }
}

fn dir_of(s: &str) -> Dir {
    if s == "out" { Dir::Out } else { Dir::In }
}

fn ty_of(s: &str) -> Option<SigTy> {
    SigTy::ALL.into_iter().find(|t| t.word() == s)
}

fn reserved(device: &str) -> bool {
    RESERVED.contains(&device)
}

fn port_named<'a>(ports: &'a [Port], name: &str) -> Option<&'a Port> {
    ports.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

fn word(d: Dir) -> &'static str {
    match d {
        Dir::In => "input",
        Dir::Out => "output",
    }
}

/// Where `port` is bound in `map`.
#[must_use]
pub fn place_of(map: &MapText, port: &str) -> Place {
    for s in &map.signals {
        if !s.name.eq_ignore_ascii_case(port) {
            continue;
        }
        if s.device == "module" {
            return Place::Module(s.point.clone());
        }
        if !reserved(&s.device) {
            return Place::Device {
                device: s.device.clone(),
                point: s.point.clone(),
            };
        }
    }
    Place::Simulated
}

/// Every signal of a declared device that no program port names.
#[must_use]
pub fn free_channels(map: &MapText, ports: &[Port]) -> Vec<Channel> {
    map.signals
        .iter()
        .filter(|s| !reserved(&s.device))
        .filter(|s| map.devices.iter().any(|d| d.name == s.device))
        .filter(|s| port_named(ports, &s.name).is_none())
        .map(|s| Channel {
            dir: dir_of(&s.dir),
            name: s.name.clone(),
            ty: ty_of(&s.ty),
            device: s.device.clone(),
            point: s.point.clone(),
        })
        .collect()
}

/// The free channels `port` can take: the same direction and type.
#[must_use]
pub fn channels_for(map: &MapText, ports: &[Port], port: &Port) -> Vec<Channel> {
    free_channels(map, ports)
        .into_iter()
        .filter(|c| c.dir == port.dir && c.ty == Some(port.ty))
        .collect()
}

/// Whether the map lists `port` on a simulated channel or not at all.
fn unassigned(map: &MapText, port: &Port) -> bool {
    place_of(map, &port.name) == Place::Simulated
}

/// The program ports that can take the channel on (`device`, `point`):
/// unassigned, of the channel's direction and type.
#[must_use]
pub fn ports_for(map: &MapText, ports: &[Port], device: &str, point: &str) -> Vec<Port> {
    let Some(c) = free_channels(map, ports)
        .into_iter()
        .find(|c| c.device == device && c.point == point)
    else {
        return Vec::new();
    };
    ports
        .iter()
        .filter(|p| p.dir == c.dir && Some(p.ty) == c.ty && unassigned(map, p))
        .cloned()
        .collect()
}

/// The program port the channel on (`device`, `point`) feeds, if any.
#[must_use]
pub fn port_on<'a>(map: &MapText, ports: &'a [Port], device: &str, point: &str) -> Option<&'a Port> {
    let s = map
        .signals
        .iter()
        .find(|s| s.device == device && s.point == point)?;
    port_named(ports, &s.name)
}

/// What the gesture offers on one port: the same answers for the I/O tab
/// and the diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMenu {
    /// Lower case, as the map writes it.
    pub port: String,
    pub dir: Dir,
    pub ty: SigTy,
    pub place: Place,
    /// "Assign to device I/O…": `Err` is the reason it is off.
    pub assign: Result<(), String>,
    /// "Back to simulated I/O": offered only on a port that is on a
    /// device; `Err` is the reason it is off.
    pub back: Option<Result<(), String>>,
}

/// The menu of every port. `write` is why the map cannot be written, when
/// it cannot.
#[must_use]
pub fn menus(map: &MapText, ports: &[Port], write: &Result<(), String>) -> Vec<PortMenu> {
    ports
        .iter()
        .map(|p| {
            let place = place_of(map, &p.name);
            let free = channels_for(map, ports, p).len();
            let assign = match (&place, write) {
                (_, Err(why)) => Err(why.clone()),
                (Place::Device { device, point }, _) => Err(format!(
                    "{} is on {device}, {}. Put it back on simulated I/O first.",
                    p.name,
                    point_label(point)
                )),
                (Place::Module(m), _) => Err(format!(
                    "{} reads an output of module {m}: that is not a device channel.",
                    p.name
                )),
                (Place::Simulated, _) if free == 0 => Err(format!(
                    "No free {} {} channel on a device. Add a device in the Devices tab first.",
                    p.ty.word(),
                    word(p.dir)
                )),
                (Place::Simulated, _) => Ok(()),
            };
            let back = matches!(place, Place::Device { .. }).then(|| write.clone());
            PortMenu {
                port: p.name.clone(),
                dir: p.dir,
                ty: p.ty,
                place,
                assign,
                back,
            }
        })
        .collect()
}

/// A change of io.map: its new text and a line saying what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub text: String,
    pub summary: String,
}

/// What a person asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignAct {
    /// Port `port` to the channel on (`device`, `point`).
    Assign {
        port: String,
        device: String,
        point: String,
    },
    /// Port `port` back to simulated I/O.
    Back { port: String },
}

impl AssignAct {
    /// The new map text for this act.
    ///
    /// # Errors
    /// A sentence saying why nothing was changed.
    pub fn run(&self, text: &str, ports: &[Port]) -> Result<Change, String> {
        match self {
            Self::Assign { port, device, point } => assign(text, ports, port, device, point),
            Self::Back { port } => unassign(text, ports, port),
        }
    }
}

/// One line of the file, taken apart.
struct Row<'a> {
    /// The whole line, its ending included.
    raw: &'a str,
    sig: Option<Sig<'a>>,
}

struct Sig<'a> {
    dir: &'a str,
    name: &'a str,
    /// Byte range of the name token in `raw`, and where the next token
    /// starts.
    name_at: (usize, usize),
    next_at: usize,
    ty: &'a str,
    device: &'a str,
    point: &'a str,
}

fn rows(text: &str) -> Vec<Row<'_>> {
    text.split_inclusive('\n').map(row).collect()
}

fn row(raw: &str) -> Row<'_> {
    let body = raw.strip_suffix('\n').unwrap_or(raw);
    let body = body.strip_suffix('\r').unwrap_or(body);
    let code = body.find('#').map_or(body, |i| &body[..i]);
    let mut toks: Vec<(usize, &str)> = Vec::new();
    let mut start = None;
    for (i, c) in code.char_indices() {
        match (start, c.is_whitespace()) {
            (None, false) => start = Some(i),
            (Some(s), true) => {
                toks.push((s, &code[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        toks.push((s, &code[s..]));
    }
    let sig = match toks.as_slice() {
        [
            (_, dir @ ("in" | "out")),
            (ns, name),
            (_, ty),
            (_, device),
            (_, point),
            ..,
        ] => Some(Sig {
            dir,
            name,
            name_at: (*ns, ns + name.len()),
            next_at: toks[2].0,
            ty,
            device,
            point,
        }),
        _ => None,
    };
    Row { raw, sig }
}

/// `row` with its signal's name token replaced by `new`, the columns after
/// it kept where they were when the line was aligned with spaces.
fn renamed(r: &Row<'_>, new: &str) -> String {
    let Some(s) = &r.sig else {
        return r.raw.to_string();
    };
    let gap = &r.raw[s.name_at.1..s.next_at];
    let gap = if gap.len() >= 2 && gap.bytes().all(|b| b == b' ') {
        " ".repeat((gap.len() + s.name.len()).saturating_sub(new.len()).max(1))
    } else {
        gap.to_string()
    };
    format!("{}{new}{gap}{}", &r.raw[..s.name_at.0], &r.raw[s.next_at..])
}

fn is_port_line(r: &Row<'_>, port: &Port) -> bool {
    r.sig
        .as_ref()
        .is_some_and(|s| s.name.eq_ignore_ascii_case(&port.name) && dir_of(s.dir) == port.dir)
}

/// Assigns `port` to the channel on (`device`, `point`).
///
/// # Errors
/// A sentence saying why nothing was changed: the port or channel is not
/// there, the direction or type differ, the channel feeds another port, or
/// the port is on a device already.
pub fn assign(text: &str, ports: &[Port], port: &str, device: &str, point: &str) -> Result<Change, String> {
    let nothing = "Nothing was changed.";
    let p = port_named(ports, port)
        .ok_or_else(|| format!("{port} is not an input or output of the program. {nothing}"))?;
    if reserved(device) {
        return Err(format!(
            "{device} is the box's own channels, not a device. {nothing}"
        ));
    }
    let map = iomap::skim(text);
    if !map.devices.iter().any(|d| d.name == device) {
        return Err(format!("io.map declares no device {device}. {nothing}"));
    }
    let all = rows(text);
    let target = all
        .iter()
        .position(|r| {
            r.sig
                .as_ref()
                .is_some_and(|s| s.device == device && s.point == point)
        })
        .ok_or_else(|| {
            format!(
                "{device} has no channel {} in io.map. {nothing}",
                point_label(point)
            )
        })?;
    let ch = all[target].sig.as_ref().expect("a signal row");
    let label = format!("{}, {}", device, point_label(point));
    if dir_of(ch.dir) != p.dir {
        return Err(format!(
            "{} is an {} and {label} is an {}. {nothing}",
            p.name,
            word(p.dir),
            word(dir_of(ch.dir))
        ));
    }
    if ty_of(ch.ty) != Some(p.ty) {
        return Err(format!(
            "{} is {} {} and {label} is {} {}. {nothing}",
            p.name,
            article(p.ty.word()),
            p.ty.word(),
            article(ch.ty),
            ch.ty
        ));
    }
    if let Some(other) = port_named(ports, ch.name) {
        return Err(if other.name == p.name {
            format!("{label} already feeds {}. {nothing}", p.name)
        } else {
            format!("{label} already feeds {}. {nothing}", other.name)
        });
    }
    if let Place::Device { device: d, point: pt } = place_of(&map, &p.name) {
        return Err(format!(
            "{} is on {d}, {} already: put it back on simulated I/O first. {nothing}",
            p.name,
            point_label(&pt)
        ));
    }
    if let Place::Module(m) = place_of(&map, &p.name) {
        return Err(format!(
            "{} reads an output of module {m}, not a device channel. {nothing}",
            p.name
        ));
    }
    let mut out = String::with_capacity(text.len());
    let mut dropped = 0;
    for (i, r) in all.iter().enumerate() {
        if i == target {
            out.push_str(&renamed(r, &p.name));
        } else if is_port_line(r, p) {
            dropped += 1;
        } else {
            out.push_str(r.raw);
        }
    }
    let tail = if dropped > 0 {
        " Its simulated line is gone."
    } else {
        ""
    };
    Ok(Change {
        text: out,
        summary: format!("{} is now {label} in io.map.{tail}", p.name),
    })
}

fn article(word: &str) -> &'static str {
    if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    }
}

/// The name a channel gets when its port lets go of it: a profile's
/// channel (`ch:DI-04` on `rio`) is `rio_di_04`, the generated name of
/// `helmsman_project::profile_skim::signal_name`; any other point is the
/// device and the point with each run of other characters as one `_`.
fn generated_name(device: &str, point: &str) -> String {
    match point.strip_prefix("ch:") {
        Some(ch) if !ch.is_empty() => helmsman_project::profile_skim::signal_name(device, ch),
        _ => {
            let mut s = format!("{device}_");
            for c in point.chars() {
                let c = c.to_ascii_lowercase();
                if c.is_ascii_alphanumeric() {
                    s.push(c);
                } else if !s.ends_with('_') {
                    s.push('_');
                }
            }
            s.trim_end_matches('_').to_string()
        }
    }
}

/// Puts `port` back on simulated I/O: its channel's line gets the generated
/// name again and the port gets a `box di:N` / `box do:N` line (a bool) or
/// a `screen` line, as a new project's map writes it, at the end.
///
/// # Errors
/// A sentence saying why nothing was changed.
pub fn unassign(text: &str, ports: &[Port], port: &str) -> Result<Change, String> {
    let nothing = "Nothing was changed.";
    let p = port_named(ports, port)
        .ok_or_else(|| format!("{port} is not an input or output of the program. {nothing}"))?;
    let map = iomap::skim(text);
    match place_of(&map, &p.name) {
        Place::Device { .. } => {}
        Place::Module(m) => {
            return Err(format!(
                "{} reads an output of module {m}, not a device channel. {nothing}",
                p.name
            ));
        }
        Place::Simulated => {
            return Err(format!("{} is on simulated I/O already. {nothing}", p.name));
        }
    }
    let all = rows(text);
    let target = all
        .iter()
        .position(|r| is_port_line(r, p) && r.sig.as_ref().is_some_and(|s| !reserved(s.device)))
        .ok_or_else(|| format!("{} has no line on a device. {nothing}", p.name))?;
    let s = all[target].sig.as_ref().expect("a signal row");
    let (device, point) = (s.device, s.point);
    // A name no other signal or port has.
    let taken = |n: &str| {
        ports.iter().any(|q| q.name.eq_ignore_ascii_case(n))
            || map
                .signals
                .iter()
                .any(|x| x.name.eq_ignore_ascii_case(n) && !x.name.eq_ignore_ascii_case(&p.name))
    };
    let base = generated_name(device, point);
    let mut name = base.clone();
    let mut n = 2;
    while taken(&name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    // The next free index among the box's own channels of that kind.
    let kind = match p.dir {
        Dir::In => "di:",
        Dir::Out => "do:",
    };
    let used: Vec<u32> = map
        .signals
        .iter()
        .filter(|x| x.device == "box")
        .filter_map(|x| x.point.strip_prefix(kind)?.split(':').next()?.parse().ok())
        .collect();
    let free = (0..).find(|i| !used.contains(i)).unwrap_or(0);
    let line = match (p.dir, p.ty) {
        (Dir::In, SigTy::Bool) => format!("in  {} bool box di:{free}", p.name),
        (Dir::Out, SigTy::Bool) => format!("out {} bool box do:{free} safe=false", p.name),
        (Dir::In, t) => format!("in  {} {} screen {}", p.name, t.word(), p.name),
        (Dir::Out, t) => format!("out {} {} screen {} safe=0", p.name, t.word(), p.name),
    };
    let eol = if all[target].raw.ends_with("\r\n") || text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = String::with_capacity(text.len() + line.len() + 2);
    for (i, r) in all.iter().enumerate() {
        if i == target {
            out.push_str(&renamed(r, &name));
        } else {
            out.push_str(r.raw);
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&line);
    out.push_str(eol);
    Ok(Change {
        text: out,
        summary: format!(
            "{} is back on simulated I/O; {device}, {} is {name} again in io.map.",
            p.name,
            point_label(point)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "\
# the bench
device rio modbus-tcp 192.168.3.50:502 unit=1 poll_ms=50
in  rio_di_04 bool rio ch:DI-04 poll_ms=20  # the E-stop contact
in  rio_di_05 bool rio ch:DI-05
in  rio_ai_0  int  rio hr:5:i16 unit=mm
out rio_do_00 bool rio ch:DO-00 safe=true
out rio_ao_0  int  rio hr:9:i16 safe=0

in  start bool box di:0   # the button
in  stop bool box di:1
in  level int screen level
out motor bool box do:0 safe=false
out speed int screen speed safe=0
";

    fn ports() -> Vec<Port> {
        let p = |dir, name: &str, ty| Port {
            dir,
            name: name.into(),
            ty,
        };
        vec![
            p(Dir::In, "start", SigTy::Bool),
            p(Dir::In, "stop", SigTy::Bool),
            p(Dir::In, "level", SigTy::Int),
            p(Dir::Out, "motor", SigTy::Bool),
            p(Dir::Out, "speed", SigTy::Int),
        ]
    }

    fn loads(text: &str) {
        let parsed = helmsman_io::text::parse(text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        let _ = parsed;
    }

    #[test]
    fn the_map_the_tests_start_from_loads() {
        loads(MAP);
    }

    #[test]
    fn assigning_renames_the_channels_line_and_removes_the_ports_old_one() {
        let c = assign(MAP, &ports(), "start", "rio", "ch:DI-04").unwrap();
        let expect = MAP
            .replace(
                "in  rio_di_04 bool rio ch:DI-04 poll_ms=20  # the E-stop contact",
                "in  start bool rio ch:DI-04 poll_ms=20  # the E-stop contact",
            )
            .replace("in  start bool box di:0   # the button\n", "");
        assert_eq!(
            c.text, expect,
            "options, comment and column kept; only the old line gone"
        );
        assert!(c.summary.contains("start is now rio, DI-04"), "{}", c.summary);
        loads(&c.text);
        let m = iomap::skim(&c.text);
        let s = m.signals.iter().find(|s| s.name == "start").unwrap();
        assert_eq!((s.device.as_str(), s.point.as_str()), ("rio", "ch:DI-04"));
        assert!(s.tail.contains(&"poll_ms=20".to_string()));
    }

    #[test]
    fn names_are_case_blind_and_the_ports_spelling_is_written() {
        let c = assign(MAP, &ports(), "START", "rio", "ch:DI-04").unwrap();
        assert!(c.text.contains("in  start bool rio ch:DI-04"), "{}", c.text);
        let shouted = MAP.replace("in  start bool box di:0", "IN  START bool box di:0");
        // `IN` is not a keyword of the format: the line is not a signal, so
        // nothing of the port's is removed and the rename still lands.
        let c = assign(&shouted, &ports(), "start", "rio", "ch:DI-04").unwrap();
        assert!(c.text.contains("IN  START bool box di:0"));
        let upper = MAP.replace("in  start bool box di:0", "in  START bool box di:0");
        let c = assign(&upper, &ports(), "start", "rio", "ch:DI-04").unwrap();
        assert!(
            !c.text.contains("START"),
            "the port's line goes whatever its case: {}",
            c.text
        );
    }

    #[test]
    fn a_port_without_a_line_is_assigned_by_renaming_only() {
        let bare: String = MAP
            .lines()
            .filter(|l| !l.contains(" start "))
            .map(|l| format!("{l}\n"))
            .collect();
        let c = assign(&bare, &ports(), "start", "rio", "ch:DI-04").unwrap();
        assert_eq!(c.text, bare.replace("rio_di_04 bool", "start bool"));
        loads(&c.text);
    }

    #[test]
    fn every_other_line_stays_byte_for_byte_with_crlf_and_no_final_newline() {
        let crlf = MAP.replace('\n', "\r\n");
        let c = assign(&crlf, &ports(), "stop", "rio", "ch:DI-05").unwrap();
        assert!(!c.text.contains('\n') || c.text.matches('\n').count() == c.text.matches("\r\n").count());
        let expect = crlf
            .replace("in  rio_di_05 bool", "in  stop bool")
            .replace("in  stop bool box di:1\r\n", "");
        assert_eq!(c.text, expect);
        loads(&c.text);
        let no_end = MAP.trim_end().to_string();
        let c = assign(&no_end, &ports(), "speed", "rio", "hr:9:i16").unwrap();
        assert!(c.text.starts_with("# the bench\n"));
        assert!(!c.text.contains("screen speed"));
        loads(&c.text);
    }

    #[test]
    fn a_name_inside_another_token_or_a_comment_is_left_alone() {
        let map = "\
device rio modbus-tcp 1.2.3.4:502
in  rio_di_04 bool rio ch:DI-04 # start is the old name; restart_start too
in  restart bool box di:0
in  start bool box di:1 # not the start_button
in  start_button bool rio ch:DI-05
";
        let ports = vec![
            Port {
                dir: Dir::In,
                name: "start".into(),
                ty: SigTy::Bool,
            },
            Port {
                dir: Dir::In,
                name: "restart".into(),
                ty: SigTy::Bool,
            },
        ];
        let c = assign(map, &ports, "start", "rio", "ch:DI-04").unwrap();
        assert_eq!(
            c.text,
            "\
device rio modbus-tcp 1.2.3.4:502
in  start bool rio ch:DI-04 # start is the old name; restart_start too
in  restart bool box di:0
in  start_button bool rio ch:DI-05
"
        );
    }

    #[test]
    fn refusals_say_why_and_change_nothing() {
        let p = ports();
        for (port, device, point, needle) in [
            (
                "start",
                "rio",
                "ch:DO-00",
                "is an input and rio, DO-00 is an output",
            ),
            (
                "level",
                "rio",
                "ch:DI-04",
                "level is an int and rio, DI-04 is a bool",
            ),
            (
                "speed",
                "rio",
                "ch:DI-04",
                "speed is an output and rio, DI-04 is an input",
            ),
            ("start", "rio", "ch:DI-99", "no channel DI-99"),
            ("nothing", "rio", "ch:DI-04", "not an input or output"),
            ("start", "box", "di:1", "not a device"),
            ("start", "screen", "level", "not a device"),
            ("start", "module", "x", "not a device"),
            ("start", "nodev", "ch:DI-04", "declares no device nodev"),
        ] {
            let e = assign(MAP, &p, port, device, point).unwrap_err();
            assert!(e.contains(needle), "{port} {device} {point}: {e}");
            assert!(e.contains("Nothing was changed."), "{e}");
        }
    }

    #[test]
    fn a_channel_that_feeds_a_port_is_taken() {
        let c = assign(MAP, &ports(), "start", "rio", "ch:DI-04").unwrap();
        let e = assign(&c.text, &ports(), "stop", "rio", "ch:DI-04").unwrap_err();
        assert!(e.contains("rio, DI-04 already feeds start"), "{e}");
        let e = assign(&c.text, &ports(), "start", "rio", "ch:DI-05").unwrap_err();
        assert!(e.contains("start is on rio, DI-04 already"), "{e}");
        assert!(
            free_channels(&iomap::skim(&c.text), &ports())
                .iter()
                .all(|c| c.point != "ch:DI-04")
        );
    }

    #[test]
    fn a_port_on_a_module_is_not_assigned_over() {
        let map =
            "device rio modbus-tcp 1.2.3.4:502\nin x bool rio ch:DI-00\nin start bool module conveyor\n";
        let e = assign(map, &ports(), "start", "rio", "ch:DI-00").unwrap_err();
        assert!(e.contains("module conveyor"), "{e}");
    }

    #[test]
    fn the_candidates_match_direction_and_type_and_skip_taken_channels() {
        let map = iomap::skim(MAP);
        let p = ports();
        let start = port_named(&p, "start").unwrap();
        let names: Vec<String> = channels_for(&map, &p, start).iter().map(Channel::label).collect();
        assert_eq!(names, ["rio, DI-04", "rio, DI-05"]);
        let speed = port_named(&p, "speed").unwrap();
        assert_eq!(channels_for(&map, &p, speed).len(), 1);
        let rev: Vec<String> = ports_for(&map, &p, "rio", "ch:DI-05")
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(rev, ["start", "stop"], "unassigned bool inputs");
        assert!(
            ports_for(&map, &p, "rio", "ch:DO-00")
                .iter()
                .all(|p| p.name == "motor")
        );
        assert!(
            ports_for(&map, &p, "rio", "hr:5:i16")
                .iter()
                .all(|p| p.name == "level")
        );
        assert!(ports_for(&map, &p, "rio", "nope").is_empty());
        assert!(
            ports_for(&map, &p, "box", "di:1").is_empty(),
            "a box line is not a device channel"
        );
    }

    #[test]
    fn back_to_simulated_round_trips_to_a_map_the_box_loads() {
        let a = assign(MAP, &ports(), "start", "rio", "ch:DI-04").unwrap();
        let b = unassign(&a.text, &ports(), "start").unwrap();
        loads(&b.text);
        let m = iomap::skim(&b.text);
        let back = m.signals.iter().find(|s| s.name == "start").unwrap();
        assert_eq!(
            (back.device.as_str(), back.point.as_str(), back.ty.as_str()),
            ("box", "di:0", "bool")
        );
        let ch = m.signals.iter().find(|s| s.point == "ch:DI-04").unwrap();
        assert_eq!(ch.name, "rio_di_04", "the generated name again");
        assert!(ch.tail.contains(&"poll_ms=20".to_string()));
        assert!(b.text.contains("# the E-stop contact"));
        assert_eq!(place_of(&m, "start"), Place::Simulated);
        // And forward again.
        let again = assign(&b.text, &ports(), "start", "rio", "ch:DI-04").unwrap();
        loads(&again.text);
    }

    #[test]
    fn back_writes_outputs_and_numbers_as_a_new_projects_map_does() {
        let a = assign(MAP, &ports(), "motor", "rio", "ch:DO-00").unwrap();
        let b = unassign(&a.text, &ports(), "motor").unwrap();
        assert!(
            b.text.ends_with("out motor bool box do:0 safe=false\n"),
            "{}",
            b.text
        );
        loads(&b.text);
        let a = assign(MAP, &ports(), "speed", "rio", "hr:9:i16").unwrap();
        let b = unassign(&a.text, &ports(), "speed").unwrap();
        assert!(
            b.text.ends_with("out speed int screen speed safe=0\n"),
            "{}",
            b.text
        );
        assert!(
            b.text.contains("rio_hr_9_i16"),
            "the generated name is back: {}",
            b.text
        );
        loads(&b.text);
        let a = assign(MAP, &ports(), "level", "rio", "hr:5:i16").unwrap();
        let b = unassign(&a.text, &ports(), "level").unwrap();
        assert!(b.text.ends_with("in  level int screen level\n"), "{}", b.text);
        assert!(b.text.contains("rio_hr_5_i16"), "{}", b.text);
        loads(&b.text);
    }

    #[test]
    fn back_names_the_channel_uniquely_and_keeps_line_endings() {
        let map = "device rio modbus-tcp 1.2.3.4:502\r\nin start bool rio ch:DI-04\r\nin rio_di_04 bool rio ch:DI-05\r\n";
        let p = vec![Port {
            dir: Dir::In,
            name: "start".into(),
            ty: SigTy::Bool,
        }];
        let b = unassign(map, &p, "start").unwrap();
        assert_eq!(
            b.text,
            "device rio modbus-tcp 1.2.3.4:502\r\nin rio_di_04_2 bool rio ch:DI-04\r\nin rio_di_04 bool rio ch:DI-05\r\nin  start bool box di:0\r\n"
        );
        loads(&b.text);
        let e = unassign(MAP, &ports(), "start").unwrap_err();
        assert!(e.contains("simulated I/O already"), "{e}");
        let e = unassign(MAP, &ports(), "ghost").unwrap_err();
        assert!(e.contains("not an input or output"), "{e}");
    }

    #[test]
    fn a_point_that_is_not_a_profile_channel_gets_a_name_from_its_address() {
        assert_eq!(generated_name("plc", "hr:5:i16"), "plc_hr_5_i16");
        assert_eq!(generated_name("e1213", "ch:DI-04-count"), "e1213_di_04_count");
        assert_eq!(generated_name("m", "Coil:0"), "m_coil_0");
    }

    #[test]
    fn the_menus_say_where_a_port_is_and_why_an_item_is_off() {
        let map = iomap::skim(MAP);
        let m = menus(&map, &ports(), &Ok(()));
        let start = m.iter().find(|m| m.port == "start").unwrap();
        assert_eq!(start.place.describe(), "Simulated: not assigned to a device yet");
        assert!(start.assign.is_ok() && start.back.is_none());
        let a = assign(MAP, &ports(), "start", "rio", "ch:DI-04").unwrap();
        let map = iomap::skim(&a.text);
        let m = menus(&map, &ports(), &Ok(()));
        let start = m.iter().find(|m| m.port == "start").unwrap();
        assert_eq!(start.place.describe(), "On rio, DI-04");
        assert!(
            start
                .assign
                .as_ref()
                .unwrap_err()
                .contains("Put it back on simulated I/O first")
        );
        assert_eq!(start.back, Some(Ok(())));
        // No device at all: the reason says to add one.
        let none = iomap::skim("in start bool box di:0\n");
        let m = menus(&none, &ports(), &Ok(()));
        let why = m[0].assign.as_ref().unwrap_err();
        assert!(why.contains("Add a device in the Devices tab first"), "{why}");
        // Read-only: every item says why.
        let ro = menus(&map, &ports(), &Err("The examples are read-only.".into()));
        assert!(
            ro.iter()
                .all(|m| m.assign == Err("The examples are read-only.".into()))
        );
        assert_eq!(
            ro.iter().find(|m| m.port == "start").unwrap().back,
            Some(Err("The examples are read-only.".into()))
        );
    }
}
