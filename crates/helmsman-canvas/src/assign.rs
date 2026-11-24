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

/// Whether a signal is still simulated or is a device's channel: the one
/// truth the I/O tab's cards, the diagram's I/O blocks and the trend's
/// legend ask (Nick, 2026-11-24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Designation {
    /// Still on the simulation (the box's own channels, the screen, or no
    /// line): drawn dashed with a SIM badge.
    Simulated,
    /// On a device's channel: drawn solid, naming both.
    Physical { device: String, channel: String },
}

impl Designation {
    /// The word of the badge.
    pub const BADGE: &'static str = "SIM";

    /// What `place` is; `None` for another module's output, which is
    /// neither a simulation nor a device.
    #[must_use]
    pub fn of(place: &Place) -> Option<Self> {
        match place {
            Place::Device { device, point } => Some(Self::Physical {
                device: device.clone(),
                channel: point_label(point).to_string(),
            }),
            Place::Module(_) => None,
            Place::Simulated => Some(Self::Simulated),
        }
    }

    #[must_use]
    pub const fn is_simulated(&self) -> bool {
        matches!(self, Self::Simulated)
    }

    /// `rio DI-04` for a physical signal, else `SIM`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Simulated => Self::BADGE.to_string(),
            Self::Physical { device, channel } => format!("{device} {channel}"),
        }
    }
}

/// How the map designates the signal `name` (any case). A signal the map
/// puts on a device is physical whatever else says it is simulated (a
/// kept `plant.sim` line is switched off then); one it does not list at
/// all counts as simulated only when `port` says it is a port of the
/// program, since a signal nobody knows is not ours to badge.
#[must_use]
pub fn designate(map: &MapText, name: &str, port: bool) -> Option<Designation> {
    let place = place_of(map, name);
    if place == Place::Simulated && !port && !map.signals.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
        return None;
    }
    Designation::of(&place)
}

/// A point as a person reads it: `ch:DI-04` is channel DI-04.
#[must_use]
pub fn point_label(point: &str) -> &str {
    point.strip_prefix("ch:").unwrap_or(point)
}

/// What names the channels a device has (`DevicesUi::channels_of`: its
/// profile, or the catalog), as `device_io` reads them; `None` when
/// nobody knows.
pub type Chans<'a> = &'a dyn Fn(&str) -> Option<Vec<crate::device_io::Channel>>;

/// A channel of a declared device a port could take: a signal the map
/// already has on it, or a channel of the device no signal is on yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub dir: Dir,
    /// The signal's name now; `None` when the channel has no line yet.
    pub name: Option<String>,
    pub ty: Option<SigTy>,
    pub device: String,
    pub point: String,
    /// What the device calls it (`DI-04`).
    pub what: String,
}

impl Channel {
    /// `rio, DI-04`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}, {}", self.device, self.what)
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

/// Every channel of a declared device a port could take: the signals on
/// it that no program port names, and the channels it has that no signal
/// is on yet (`device_io::free`, the Add I/O picker's list).
#[must_use]
pub fn free_channels(map: &MapText, ports: &[Port], chans: Chans<'_>) -> Vec<Channel> {
    let mut out: Vec<Channel> = map
        .signals
        .iter()
        .filter(|s| !reserved(&s.device))
        .filter(|s| map.devices.iter().any(|d| d.name == s.device))
        .filter(|s| port_named(ports, &s.name).is_none())
        .map(|s| Channel {
            dir: dir_of(&s.dir),
            name: Some(s.name.clone()),
            ty: ty_of(&s.ty),
            device: s.device.clone(),
            point: s.point.clone(),
            what: point_label(&s.point).to_string(),
        })
        .collect();
    for d in &map.devices {
        if reserved(&d.name) {
            continue;
        }
        let Some(all) = chans(&d.name) else { continue };
        out.extend(
            crate::device_io::free(&all, map, &d.name)
                .into_iter()
                .map(|c| Channel {
                    dir: c.dir,
                    name: None,
                    ty: Some(c.ty),
                    device: d.name.clone(),
                    point: c.point,
                    what: c.label,
                }),
        );
    }
    out
}

/// The free channels `port` can take: the same direction and type.
#[must_use]
pub fn channels_for(map: &MapText, ports: &[Port], chans: Chans<'_>, port: &Port) -> Vec<Channel> {
    free_channels(map, ports, chans)
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
pub fn ports_for(map: &MapText, ports: &[Port], chans: Chans<'_>, device: &str, point: &str) -> Vec<Port> {
    let Some(c) = free_channels(map, ports, chans)
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
pub fn menus(map: &MapText, ports: &[Port], chans: Chans<'_>, write: &Result<(), String>) -> Vec<PortMenu> {
    ports
        .iter()
        .map(|p| {
            let place = place_of(map, &p.name);
            let free = channels_for(map, ports, chans, p).len();
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
    pub fn run(&self, text: &str, ports: &[Port], chans: Chans<'_>) -> Result<Change, String> {
        match self {
            Self::Assign { port, device, point } => assign(text, ports, chans, port, device, point),
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

/// Assigns `port` to the channel on (`device`, `point`). A channel the map
/// has a line for takes the port's name on that line, the rest of the
/// line as it was. A channel of the device no signal is on yet gets its
/// line from `device_io::add`, named for the port, after the device's
/// other lines (`device_form::insert_signals`). The port's simulated line
/// goes either way.
///
/// # Errors
/// A sentence saying why nothing was changed: the port or channel is not
/// there, the direction or type differ, the channel feeds another port, or
/// the port is on a device already.
pub fn assign(
    text: &str,
    ports: &[Port],
    chans: Chans<'_>,
    port: &str,
    device: &str,
    point: &str,
) -> Result<Change, String> {
    let nothing = "Nothing was changed.";
    let p = port_named(ports, port)
        .ok_or_else(|| format!("{port} is not an input or output of the program. {nothing}"))?;
    if reserved(device) {
        return Err(format!(
            "{device} is the box's own channels, not a device. {nothing}"
        ));
    }
    let map = iomap::skim(text);
    let Some(dev) = map.devices.iter().find(|d| d.name == device) else {
        return Err(format!("io.map declares no device {device}. {nothing}"));
    };
    let all = rows(text);
    let target = all.iter().position(|r| {
        r.sig
            .as_ref()
            .is_some_and(|s| s.device == device && s.point == point)
    });
    let new = if target.is_none() {
        chans(device).and_then(|l| l.into_iter().find(|c| c.point.eq_ignore_ascii_case(point)))
    } else {
        None
    };
    // The channel's direction, type and label, whichever way it is found.
    let (cdir, cty, what, line_name) = match (target, &new) {
        (Some(t), _) => {
            let ch = all[t].sig.as_ref().expect("a signal row");
            (
                dir_of(ch.dir),
                ch.ty.to_string(),
                point_label(point).to_string(),
                Some(ch.name),
            )
        }
        (None, Some(c)) => (c.dir, c.ty.word().to_string(), c.label.clone(), None),
        (None, None) => {
            return Err(format!(
                "{device} has no channel {} in io.map. {nothing}",
                point_label(point)
            ));
        }
    };
    let label = format!("{device}, {what}");
    if cdir != p.dir {
        return Err(format!(
            "{} is {} {} and {label} is {} {}. {nothing}",
            p.name,
            article(word(p.dir)),
            word(p.dir),
            article(word(cdir)),
            word(cdir)
        ));
    }
    if ty_of(&cty) != Some(p.ty) {
        return Err(format!(
            "{} is {} {} and {label} is {} {cty}. {nothing}",
            p.name,
            article(p.ty.word()),
            p.ty.word(),
            article(&cty),
        ));
    }
    if let Some(other) = line_name.and_then(|n| port_named(ports, n)) {
        return Err(format!("{label} already feeds {}. {nothing}", other.name));
    }
    match place_of(&map, &p.name) {
        Place::Device { device: d, point: pt } => {
            return Err(format!(
                "{} is on {d}, {} already: put it back on simulated I/O first. {nothing}",
                p.name,
                point_label(&pt)
            ));
        }
        Place::Module(m) => {
            return Err(format!(
                "{} reads an output of module {m}, not a device channel. {nothing}",
                p.name
            ));
        }
        Place::Simulated => {}
    }
    let dropped = all.iter().filter(|r| is_port_line(r, p)).count();
    let tail = if dropped > 0 {
        " Its simulated line in io.map is gone; a plant.sim line for it is kept and switched off."
    } else {
        ""
    };
    if let (Some(c), None) = (&new, target) {
        let w = crate::device_io::add(dev, c, Some(&p.name), false, ports, &map)
            .map_err(|e| format!("{e} {nothing}"))?;
        let eol = if text.contains("\r\n") { "\r" } else { "" };
        let signals: Vec<String> = w.signals.iter().map(|l| format!("{l}{eol}")).collect();
        let kept = crate::device_form::remove_signals(text, &w.replace);
        return Ok(Change {
            text: crate::device_form::insert_signals(&kept, device, &signals),
            summary: format!(
                "{} is now {label}: a new line after {device}'s in io.map.{tail}",
                p.name
            ),
        });
    }
    let target = target.expect("a line, or a channel without one");
    let mut out = String::with_capacity(text.len());
    for (i, r) in all.iter().enumerate() {
        if i == target {
            out.push_str(&renamed(r, &p.name));
        } else if !is_port_line(r, p) {
            out.push_str(r.raw);
        }
    }
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

/// Puts `port` back on simulated I/O: its line on the device goes, as
/// Remove on a point does (the channel is back in the Add I/O list and the
/// program's variable stays), and the port gets a `box di:N` / `box do:N`
/// line (a bool) or a `screen` line, as a new project's map writes it, at
/// the end.
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
    let mut free = 0;
    while used.contains(&free) {
        free += 1;
    }
    // An int or real input has no simulated line a box accepts (`screen`
    // is for outputs, `box` channels are bits): the port has no line, which
    // the simulation reads as its on-screen control.
    let line = match (p.dir, p.ty) {
        (Dir::In, SigTy::Bool) => Some(format!("in  {} bool box di:{free}", p.name)),
        (Dir::Out, SigTy::Bool) => Some(format!("out {} bool box do:{free} safe=false", p.name)),
        (Dir::In, _) => None,
        (Dir::Out, t) => Some(format!("out {} {} screen {} safe=0", p.name, t.word(), p.name)),
    };
    let eol = if all[target].raw.ends_with("\r\n") || text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = String::with_capacity(text.len() + 64);
    for (i, r) in all.iter().enumerate() {
        if i != target {
            out.push_str(r.raw);
        }
    }
    if let Some(line) = &line {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        out.push_str(line);
        out.push_str(eol);
    }
    Ok(Change {
        text: out,
        summary: format!(
            "{} is back on simulated I/O; {device}, {} is free again.",
            p.name,
            point_label(point)
        ),
    })
}

/// What the tests share: a map has to load the way a box loads it.
#[cfg(test)]
pub(crate) mod testing {
    /// `text` parses and validates as a map with no device profile.
    pub fn loads(text: &str) {
        let parsed = helmsman_io::text::parse(text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        if let Err(e) = parsed.map.validate() {
            panic!("{e:?}\n{text}");
        }
    }

    /// `text` parses and validates, a `device:` device's points read on
    /// `profile` (its text), as the box does.
    pub fn loads_with(text: &str, profile: &str) {
        use helmsman_io::Driver as _;
        let parsed = helmsman_io::text::parse(text).unwrap_or_else(|e| panic!("{e:?}\n{text}"));
        let prof = helmsman_io::profile::Profile::parse(profile).unwrap_or_else(|e| panic!("{e:?}"));
        let mut parse = |dev: &helmsman_io::Device, addr: &str| {
            if helmsman_io::profile_driver::profile_name(&dev.driver).is_some() {
                let refuse: helmsman_io::profile_driver::Open =
                    Box::new(|_| Err(helmsman_io::Error::UnknownDriver("not opened".into())));
                helmsman_io::profile_driver::ProfileDriver::new(prof.clone(), dev, refuse)?.parse(addr)
            } else {
                helmsman_io::driver::parse_point_on(dev, addr)
            }
        };
        if let Err(e) = parsed.map.validate_with(&mut parse) {
            panic!("{e:?}\n{text}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No device's channels known: only the signals the map has.
    const NONE: Chans<'static> = &|_| None;

    const MAP: &str = "\
# the bench
device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130 poll_ms=50
in  rio_di_04 bool rio ch:DI-04 poll_ms=20  # the E-stop contact
in  rio_di_05 bool rio ch:DI-05
in  rio_cnt   int  rio ch:DI-00-count unit=mm
out rio_do_00 bool rio ch:DO-00 safe=true

in  start bool box di:0   # the button
in  stop bool box di:1
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

    use super::testing::loads_with;

    /// A map loads: the E1213's profile for a `device:` device.
    fn loads(text: &str) {
        if text.contains("device:moxa") {
            loads_with(text, PROFILE);
        } else {
            super::testing::loads(text);
        }
    }

    #[test]
    fn the_map_the_tests_start_from_loads() {
        loads(MAP);
    }

    #[test]
    fn assigning_renames_the_channels_line_and_removes_the_ports_old_one() {
        let c = assign(MAP, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
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
        let c = assign(MAP, &ports(), NONE, "START", "rio", "ch:DI-04").unwrap();
        assert!(c.text.contains("in  start bool rio ch:DI-04"), "{}", c.text);
        let shouted = MAP.replace("in  start bool box di:0", "IN  START bool box di:0");
        // `IN` is not a keyword of the format: the line is not a signal, so
        // nothing of the port's is removed and the rename still lands.
        let c = assign(&shouted, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        assert!(c.text.contains("IN  START bool box di:0"));
        let upper = MAP.replace("in  start bool box di:0", "in  START bool box di:0");
        let c = assign(&upper, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        assert!(
            !c.text.contains("START"),
            "the port's line goes whatever its case: {}",
            c.text
        );
    }

    #[test]
    fn a_port_without_a_line_is_assigned_by_renaming_only() {
        let mut bare = String::new();
        for l in MAP.lines().filter(|l| !l.contains(" start ")) {
            bare.push_str(l);
            bare.push('\n');
        }
        let c = assign(&bare, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        assert_eq!(c.text, bare.replace("rio_di_04 bool", "start bool"));
        loads(&c.text);
    }

    #[test]
    fn every_other_line_stays_byte_for_byte_with_crlf_and_no_final_newline() {
        let crlf = MAP.replace('\n', "\r\n");
        let c = assign(&crlf, &ports(), NONE, "stop", "rio", "ch:DI-05").unwrap();
        assert!(!c.text.contains('\n') || c.text.matches('\n').count() == c.text.matches("\r\n").count());
        let expect = crlf
            .replace("in  rio_di_05 bool", "in  stop bool")
            .replace("in  stop bool box di:1\r\n", "");
        assert_eq!(c.text, expect);
        loads(&c.text);
        let no_end = MAP.trim_end().to_string();
        let c = assign(&no_end, &ports(), NONE, "motor", "rio", "ch:DO-00").unwrap();
        assert!(c.text.starts_with("# the bench\n"));
        assert!(!c.text.contains("out motor bool box"));
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
        let c = assign(map, &ports, NONE, "start", "rio", "ch:DI-04").unwrap();
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
            let e = assign(MAP, &p, NONE, port, device, point).unwrap_err();
            assert!(e.contains(needle), "{port} {device} {point}: {e}");
            assert!(e.contains("Nothing was changed."), "{e}");
        }
    }

    #[test]
    fn a_channel_that_feeds_a_port_is_taken() {
        let c = assign(MAP, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        let e = assign(&c.text, &ports(), NONE, "stop", "rio", "ch:DI-04").unwrap_err();
        assert!(e.contains("rio, DI-04 already feeds start"), "{e}");
        let e = assign(&c.text, &ports(), NONE, "start", "rio", "ch:DI-05").unwrap_err();
        assert!(e.contains("start is on rio, DI-04 already"), "{e}");
        assert!(
            free_channels(&iomap::skim(&c.text), &ports(), NONE)
                .iter()
                .all(|c| c.point != "ch:DI-04")
        );
    }

    #[test]
    fn a_port_on_a_module_is_not_assigned_over() {
        let map =
            "device rio modbus-tcp 1.2.3.4:502\nin x bool rio ch:DI-00\nin start bool module conveyor\n";
        let e = assign(map, &ports(), NONE, "start", "rio", "ch:DI-00").unwrap_err();
        assert!(e.contains("module conveyor"), "{e}");
    }

    #[test]
    fn the_candidates_match_direction_and_type_and_skip_taken_channels() {
        let map = iomap::skim(MAP);
        let p = ports();
        let start = port_named(&p, "start").unwrap();
        let names: Vec<String> = channels_for(&map, &p, NONE, start)
            .iter()
            .map(Channel::label)
            .collect();
        assert_eq!(names, ["rio, DI-04", "rio, DI-05"]);
        let speed = port_named(&p, "speed").unwrap();
        assert_eq!(
            channels_for(&map, &p, NONE, speed).len(),
            0,
            "the E1213 has no int output"
        );
        let rev: Vec<String> = ports_for(&map, &p, NONE, "rio", "ch:DI-05")
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(rev, ["start", "stop"], "unassigned bool inputs");
        assert!(
            ports_for(&map, &p, NONE, "rio", "ch:DO-00")
                .iter()
                .all(|p| p.name == "motor")
        );
        assert!(
            ports_for(&map, &p, NONE, "rio", "ch:DI-00-count")
                .iter()
                .all(|p| p.name == "level")
        );
        assert_eq!(ports_for(&map, &p, NONE, "rio", "nope").len(), 0);
        assert!(
            ports_for(&map, &p, NONE, "box", "di:1").is_empty(),
            "a box line is not a device channel"
        );
    }

    #[test]
    fn back_to_simulated_takes_the_channels_line_out_as_remove_does() {
        let a = assign(MAP, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        let b = unassign(&a.text, &ports(), "start").unwrap();
        loads(&b.text);
        // The channel's line is gone (the channel is the Add I/O list's
        // again, no line under a made-up name) and the port is on a box channel.
        let want = MAP
            .replace(
                "in  rio_di_04 bool rio ch:DI-04 poll_ms=20  # the E-stop contact\n",
                "",
            )
            .replace("in  start bool box di:0   # the button\n", "")
            + "in  start bool box di:0\n";
        assert_eq!(b.text, want);
        let m = iomap::skim(&b.text);
        assert_eq!(place_of(&m, "start"), Place::Simulated);
        assert!(m.signals.iter().all(|s| s.point != "ch:DI-04"));
        // And forward again, to the same channel, now with no line.
        let again = assign(&b.text, &ports(), NONE, "start", "rio", "ch:DI-04");
        assert!(
            again.unwrap_err().contains("no channel DI-04"),
            "NONE knows no channels"
        );
    }

    #[test]
    fn back_writes_outputs_and_numbers_as_a_new_projects_map_does() {
        let a = assign(MAP, &ports(), NONE, "motor", "rio", "ch:DO-00").unwrap();
        let b = unassign(&a.text, &ports(), "motor").unwrap();
        assert!(
            b.text.ends_with("out motor bool box do:0 safe=false\n"),
            "{}",
            b.text
        );
        assert!(!b.text.contains("rio_do_00"), "the line is gone: {}", b.text);
        loads(&b.text);
        let map = "device rio modbus-tcp 1.2.3.4:502\nout speed int rio hr:9:i16 safe=0\n";
        let b = unassign(map, &ports(), "speed").unwrap();
        assert_eq!(
            b.text,
            "device rio modbus-tcp 1.2.3.4:502\nout speed int screen speed safe=0\n"
        );
        loads(&b.text);
        let a = assign(MAP, &ports(), NONE, "level", "rio", "ch:DI-00-count").unwrap();
        let b = unassign(&a.text, &ports(), "level").unwrap();
        // No line a box accepts for a simulated int input: it has none.
        assert!(
            !b.text.contains("level") && !b.text.contains("ch:DI-00-count"),
            "{}",
            b.text
        );
        assert_eq!(place_of(&iomap::skim(&b.text), "level"), Place::Simulated);
        loads(&b.text);
    }

    #[test]
    fn back_keeps_line_endings_and_refuses_what_is_not_on_a_device() {
        let map = "device rio modbus-tcp 1.2.3.4:502\r\nin start bool rio di:4\r\nin x bool rio di:5\r\n";
        let p = vec![Port {
            dir: Dir::In,
            name: "start".into(),
            ty: SigTy::Bool,
        }];
        let b = unassign(map, &p, "start").unwrap();
        assert_eq!(
            b.text,
            "device rio modbus-tcp 1.2.3.4:502\r\nin x bool rio di:5\r\nin  start bool box di:0\r\n"
        );
        loads(&b.text);
        let e = unassign(MAP, &ports(), "start").unwrap_err();
        assert!(e.contains("simulated I/O already"), "{e}");
        let e = unassign(MAP, &ports(), "ghost").unwrap_err();
        assert!(e.contains("not an input or output"), "{e}");
    }

    /// The E1213's own profile, as a project holds it.
    const PROFILE: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");
    /// A fresh device: its line and no signal on it, beside simulated ports.
    const FRESH: &str = "\
# the cell
in  start bool box di:0  # the button
in  stop bool box di:1
out motor bool box do:0 safe=false
out lamp bool box do:1 safe=false

device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130  # the block
in  part bool rio ch:DI-02

device other modbus-tcp 10.0.0.9:502 unit=1
";

    /// The channels of the map's devices, from the E1213's profile.
    fn profiled(map: &str) -> impl Fn(&str) -> Option<Vec<crate::device_io::Channel>> {
        let m = iomap::skim(map);
        move |name: &str| {
            let d = m.devices.iter().find(|d| d.name == name)?;
            crate::device_io::channels(d, Some(PROFILE))
        }
    }

    #[test]
    fn a_channel_with_no_line_gets_one_named_for_the_port_after_its_device() {
        let c = profiled(FRESH);
        let a = assign(FRESH, &ports(), &c, "start", "rio", "ch:DI-04").unwrap();
        let want = FRESH
            .replace("in  start bool box di:0  # the button\n", "")
            .replace(
                "in  part bool rio ch:DI-02\n",
                "in  part bool rio ch:DI-02\nin  start bool rio ch:DI-04\n",
            );
        // The line is the one the Add I/O picker writes, wherever it puts it.
        let line = a
            .text
            .lines()
            .find(|l| l.contains("ch:DI-04"))
            .unwrap()
            .to_string();
        assert!(
            line.starts_with("in") && line.contains(" start bool rio ch:DI-04"),
            "{line}"
        );
        assert_eq!(a.text.replace(&line, "in  start bool rio ch:DI-04"), want);
        assert!(!a.text.contains("box di:0"), "the simulated line is gone");
        loads_with(&a.text, PROFILE);
        // An output carries the options the picker writes for that channel.
        let o = assign(FRESH, &ports(), &c, "motor", "rio", "ch:DO-00").unwrap();
        let line = o.text.lines().find(|l| l.contains("ch:DO-00")).unwrap();
        assert!(
            line.contains(" motor bool rio ch:DO-00") && line.contains("safe=false"),
            "{line}"
        );
        assert!(!o.text.contains("out motor bool box"), "{}", o.text);
        loads_with(&o.text, PROFILE);
        // The other lines, comments and blanks are as they were.
        assert!(
            o.text
                .starts_with("# the cell\nin  start bool box di:0  # the button\n")
        );
        assert!(o.text.contains("device other modbus-tcp 10.0.0.9:502 unit=1\n"));
    }

    #[test]
    fn a_port_with_no_line_at_all_gets_the_new_line_too_and_crlf_is_kept() {
        let map = FRESH
            .replace("in  start bool box di:0  # the button\n", "")
            .replace('\n', "\r\n");
        let c = profiled(&map);
        let a = assign(&map, &ports(), &c, "start", "rio", "ch:DI-04").unwrap();
        assert_eq!(
            a.text.matches('\n').count(),
            a.text.matches("\r\n").count(),
            "{:?}",
            a.text
        );
        assert!(a.text.contains("ch:DI-04"));
        loads_with(&a.text, PROFILE);
    }

    #[test]
    fn the_free_channels_are_the_lines_no_port_names_and_the_channels_without_one() {
        let c = profiled(FRESH);
        let map = iomap::skim(FRESH);
        let p = ports();
        let start = port_named(&p, "start").unwrap();
        let list = channels_for(&map, &p, &c, start);
        let labels: Vec<String> = list.iter().map(Channel::label).collect();
        assert!(labels.contains(&"rio, DI-02".to_string()), "the line the map has");
        assert!(
            labels.contains(&"rio, DI-04".to_string()),
            "a channel with no line"
        );
        assert!(
            !labels.contains(&"rio, DO-00".to_string()),
            "an output is not offered an input"
        );
        assert!(
            !labels.contains(&"rio, DI-00-count".to_string()),
            "an int is not offered a bool"
        );
        assert!(list.iter().find(|c| c.what == "DI-04").unwrap().name.is_none());
        assert!(
            list.iter().all(|c| c.device == "rio"),
            "no channels are known for `other`"
        );
        // Once assigned the channel is no longer free; the others stay.
        let a = assign(FRESH, &p, &c, "start", "rio", "ch:DI-04").unwrap();
        let after = iomap::skim(&a.text);
        let c2 = profiled(&a.text);
        let again = channels_for(&after, &p, &c2, port_named(&p, "stop").unwrap());
        assert!(again.iter().all(|c| c.point != "ch:DI-04"));
        assert!(again.iter().any(|c| c.point == "ch:DI-05"));
    }

    #[test]
    fn a_fresh_device_makes_the_menus_offer_not_say_add_a_device() {
        let map = iomap::skim(FRESH);
        let c = profiled(FRESH);
        let m = menus(&map, &ports(), &c, &Ok(()));
        assert!(m.iter().find(|m| m.port == "start").unwrap().assign.is_ok());
        assert!(m.iter().find(|m| m.port == "motor").unwrap().assign.is_ok());
        // `speed` is an int output: the E1213 has none.
        let speed = m.iter().find(|m| m.port == "speed").unwrap();
        assert!(
            speed.assign.as_ref().unwrap_err().contains("No free int output"),
            "{speed:?}"
        );
    }

    #[test]
    fn going_back_returns_a_new_line_to_the_picker_as_remove_does() {
        let c = profiled(FRESH);
        let a = assign(FRESH, &ports(), &c, "start", "rio", "ch:DI-04").unwrap();
        let b = unassign(&a.text, &ports(), "start").unwrap();
        loads_with(&b.text, PROFILE);
        let m = iomap::skim(&b.text);
        assert!(m.signals.iter().all(|s| s.point != "ch:DI-04"), "no stray line");
        let c2 = profiled(&b.text);
        let free = crate::device_io::free(&c2("rio").unwrap(), &m, "rio");
        assert!(
            free.iter().any(|c| c.point == "ch:DI-04"),
            "back in the picker's list"
        );
    }

    #[test]
    fn the_menus_say_where_a_port_is_and_why_an_item_is_off() {
        let map = iomap::skim(MAP);
        let m = menus(&map, &ports(), NONE, &Ok(()));
        let start = m.iter().find(|m| m.port == "start").unwrap();
        assert_eq!(start.place.describe(), "Simulated: not assigned to a device yet");
        assert!(start.assign.is_ok() && start.back.is_none());
        let a = assign(MAP, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        let map = iomap::skim(&a.text);
        let m = menus(&map, &ports(), NONE, &Ok(()));
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
        let m = menus(&none, &ports(), NONE, &Ok(()));
        let why = m[0].assign.as_ref().unwrap_err();
        assert!(why.contains("Add a device in the Devices tab first"), "{why}");
        // Read-only: every item says why.
        let ro = menus(&map, &ports(), NONE, &Err("The examples are read-only.".into()));
        assert!(
            ro.iter()
                .all(|m| m.assign == Err("The examples are read-only.".into()))
        );
        assert_eq!(
            ro.iter().find(|m| m.port == "start").unwrap().back,
            Some(Err("The examples are read-only.".into()))
        );
    }

    #[test]
    fn a_signal_is_simulated_on_the_box_or_the_screen_and_physical_on_a_device() {
        let map = iomap::skim(MAP);
        let d = |n: &str, port| designate(&map, n, port);
        assert_eq!(d("start", true), Some(Designation::Simulated));
        assert_eq!(d("speed", true), Some(Designation::Simulated), "a screen line");
        assert_eq!(
            d("rio_di_04", false),
            Some(Designation::Physical {
                device: "rio".into(),
                channel: "DI-04".into()
            })
        );
        assert_eq!(d("rio_di_04", false).unwrap().label(), "rio DI-04");
        assert_eq!(Designation::Simulated.label(), "SIM");
        // Case does not matter, as the map's own match does not.
        assert_eq!(d("START", true), Some(Designation::Simulated));
        assert!(matches!(d("RIO_DI_04", false), Some(Designation::Physical { .. })));
    }

    #[test]
    fn a_name_the_map_does_not_list_is_simulated_only_when_it_is_a_port() {
        let map = iomap::skim(MAP);
        assert_eq!(designate(&map, "nope", true), Some(Designation::Simulated));
        assert_eq!(designate(&map, "nope", false), None);
        // Hostile names: empty, spaces, a prefix of a real one, markup.
        for n in ["", " ", "rio_di_0", "rio_di_04 ", "ch:DI-04", "<b>start</b>", "start\n"] {
            assert_eq!(designate(&map, n, false), None, "{n:?}");
        }
        assert_eq!(designate(&iomap::skim(""), "start", true), Some(Designation::Simulated));
    }

    #[test]
    fn a_signal_on_a_device_and_in_the_simulation_is_physical_and_another_module_is_neither() {
        // The same name twice: a box line, then a device line. The device wins,
        // as `place_of` reads it; the plant.sim line has no say here.
        let map = iomap::skim(
            "device rio device:x cip=1:2:3\nin start bool box di:0\nin start bool rio ch:DI-04\nin up bool module cell.up\n",
        );
        assert_eq!(
            designate(&map, "start", true),
            Some(Designation::Physical {
                device: "rio".into(),
                channel: "DI-04".into()
            })
        );
        assert_eq!(designate(&map, "up", true), None, "a module's output");
    }

    #[test]
    fn assigning_and_going_back_keeps_the_plant_line_and_turns_it_off_and_on() {
        use helmsman_sim::set_off;
        const PLANT: &str = "# the line\nbutton b input=start\n";
        let a = assign(MAP, &ports(), NONE, "start", "rio", "ch:DI-04").unwrap();
        assert!(a.summary.contains("kept and switched off"), "{}", a.summary);
        let off = set_off(PLANT, "start", true).expect("b names start");
        assert!(off.starts_with(PLANT) && off.ends_with("off start\n"), "{off:?}");
        let back = unassign(&a.text, &ports(), "start").unwrap();
        assert!(back.text.contains("start bool box di:"));
        assert_eq!(set_off(&off, "start", false).as_deref(), Some(PLANT), "exactly as it was");
    }
}
