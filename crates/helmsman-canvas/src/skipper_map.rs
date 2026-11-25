//! The I/O list a deploy sends a Skipper (Nick, 2026-11-24: Deploy failed
//! with `driver "device:moxa-iologik-e1213": a node drives modbus-tcp and
//! enip-io devices` after Scan and add had put the E1213 in the project).
//!
//! A box finds a device by its profile (`device X device:STEM cip=...`):
//! it reads the profile, picks a transport and finds the unit on the
//! network by its identity. A Skipper does none of that: it runs
//! `modbus-tcp` and `enip-io` lines with an IPv4 address and the
//! transport's own point addresses. So the map a deploy sends is made
//! here, from two things:
//!
//! - **The Skipper's own list, kept whole.** Its devices already answer it
//!   and its program may use their points. A project device that is one
//!   of them (the same name, or the same address) is left to the
//!   Skipper's line, and its points with it.
//! - **The project's `io.map` on top**, each device the Skipper can run:
//!   a `device:STEM` line becomes the first of the profile's transports a
//!   Skipper drives, at the address its endpoint names (a host, or an
//!   identity the last scan found), with that transport's params; each
//!   `ch:NAME` point becomes that transport's address for the channel.
//!   `sim` points are kept. A point the Skipper's list already names is
//!   the Skipper's.
//!
//! What cannot be made is refused, by device, in words, before anything
//! is sent: a deploy that would fail on the Skipper fails here instead,
//! saying what to do.

use std::collections::{BTreeMap, BTreeSet};

use helmsman_project::iomap::{self, MapDevice, MapSignal};

/// The transports a Skipper drives, as a profile and a map name them.
const RUNS: [&str; 2] = ["enip-io", "modbus-tcp"];

/// How a map names a device by its profile: `device:STEM`.
const BY_PROFILE: &str = "device:";

/// The longest point or device name a Skipper takes
/// (`helmsman_nodecore::iomap::NAME_MAX`).
pub const NAME_MAX: usize = 32;

/// The device params a Skipper reads, by transport (`helmsman-nodecore`'s
/// `iomap.rs`): the rest are a box's (`cycle_ms`, `eds=`, ...).
fn param_kept(driver: &str, key: &str) -> bool {
    match driver {
        "enip-io" => matches!(
            key,
            "profile"
                | "connection"
                | "config"
                | "config_data"
                | "o2t"
                | "t2o"
                | "run_idle"
                | "rpi_ms"
                | "multiplier"
                | "timeout_ms"
        ),
        "modbus-tcp" => matches!(key, "unit" | "timeout_ms"),
        _ => false,
    }
}

/// A channel (upper case) to its address on each transport.
type Channels = BTreeMap<String, BTreeMap<String, String>>;

/// One profile, as far as a Skipper needs it. `profile_skim` keeps neither
/// a transport's params nor a channel's per-transport address, which is
/// what a Skipper's line is made of.
#[derive(Debug, Default)]
struct Profile {
    /// `transport DRIVER k=v ...`, in the profile's order.
    transports: Vec<(String, Vec<(String, String)>)>,
    channels: Channels,
}

fn read_profile(text: &str) -> Profile {
    let kv = |rest: &[&str]| -> Vec<(String, String)> {
        rest.iter()
            .filter_map(|t| t.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    let mut p = Profile::default();
    for line in text.lines() {
        let w: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
        match w.as_slice() {
            ["transport", driver, rest @ ..] => p.transports.push(((*driver).to_string(), kv(rest))),
            ["in" | "out", name, _ty, rest @ ..] => {
                let at = kv(rest)
                    .into_iter()
                    .filter(|(k, _)| RUNS.contains(&k.as_str()) || p.transports.iter().any(|(d, _)| d == k))
                    .collect();
                p.channels.insert(name.to_ascii_uppercase(), at);
            }
            _ => {}
        }
    }
    p
}

/// An endpoint's host and `:PORT` (empty without one); `None` for an
/// identity (`cip=`, `mac=`, `modbus=`).
fn host_port(endpoint: &str) -> Option<(&str, &str)> {
    if endpoint.contains('=') {
        return None;
    }
    Some(
        endpoint
            .rsplit_once(':')
            .filter(|(_, p)| p.parse::<u16>().is_ok())
            .map_or((endpoint, ""), |(h, _)| (h, &endpoint[h.len()..])),
    )
}

/// A device line, with no trailing space.
fn device_line(name: &str, driver: &str, at: &str, params: &[(String, String)]) -> String {
    let mut l = format!("device {name} {driver} {at}");
    for (k, v) in params.iter().filter(|(k, _)| param_kept(driver, k)) {
        l.push_str(&format!(" {k}={v}"));
    }
    l
}

/// The words for a project point that is a Skipper point under another
/// name: one wording for the Messages, the deploy and the Agent.
#[must_use]
pub fn same_words(project: &str, skipper: &str) -> String {
    format!(
        "{project} is the Skipper's {skipper} (the same channel): use {skipper}, not {project}, in the program"
    )
}

/// Where a project device's points go: its transport and the profile's
/// channels, under the Skipper's device when it is one of the Skipper's.
struct Via {
    /// The Skipper's device it is, or `None` when it is sent itself.
    skippers: Option<String>,
    driver: String,
    channels: Option<Channels>,
}

/// What [`for_skipper`] made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForSkipper {
    /// The map to send, when nothing was refused.
    pub map: String,
    /// What was left to the Skipper's own list, in words.
    pub notes: Vec<String>,
    /// What cannot go to a Skipper, in words, one a device or point.
    pub refused: Vec<String>,
    /// Among them a device named by an identity no scan has given an
    /// address for: a scan may find it.
    pub unfound: bool,
    /// A project point that is a Skipper point under another name (the
    /// same channel of the same device): project name, Skipper name. The
    /// program has to use the Skipper's.
    pub same: Vec<(String, String)>,
}

impl ForSkipper {
    /// The words for the Messages: what was refused (the deploy stops),
    /// else what was left to the Skipper.
    #[must_use]
    pub fn words(&self) -> Option<String> {
        if !self.refused.is_empty() {
            return Some(format!(
                "Not deployed: the Skipper cannot run {}",
                self.refused.join("; ")
            ));
        }
        (!self.notes.is_empty()).then(|| self.notes.join(" "))
    }
}

/// The devices of a map: name and host (empty for an identity).
#[must_use]
pub fn devices(map: &str) -> Vec<(String, String)> {
    iomap::skim(map)
        .devices
        .into_iter()
        .map(|d| {
            let host = host_port(&d.endpoint).map_or("", |(h, _)| h).to_string();
            (d.name, host)
        })
        .collect()
}

/// The map a deploy sends a Skipper that runs `node`, for the project's
/// `project` map ([module docs](self)). `profile` gives a profile's text
/// by its stem (`moxa-iologik-e1213`), `found` the address the last scan
/// saw at an identity (`cip=...`).
#[must_use]
pub fn for_skipper(
    project: Option<&str>,
    node: &str,
    profile: &dyn Fn(&str) -> Option<String>,
    found: &dyn Fn(&str) -> Option<String>,
) -> ForSkipper {
    let mut out = ForSkipper::default();
    let theirs = iomap::skim(node);
    let node_lines: BTreeSet<&str> = node.lines().map(str::trim).collect();
    let mut map = node.trim_end().to_string();
    let mut added: Vec<String> = Vec::new();
    // Each project device by lower-case name: where its points go, `None`
    // when it was refused.
    let mut via: BTreeMap<String, Option<Via>> = BTreeMap::new();
    let mut points: BTreeSet<String> = theirs
        .signals
        .iter()
        .map(|s| s.name.to_ascii_lowercase())
        .collect();
    for l in project.unwrap_or("").lines() {
        let code = l.split('#').next().unwrap_or("").trim();
        if code.is_empty() {
            if l.trim() == crate::placeholders::HEADER && !map.contains(crate::placeholders::HEADER) {
                added.push(l.trim().to_string());
            }
            continue;
        }
        let one = iomap::skim(code);
        if let Some(d) = one.devices.first() {
            let v = device(
                d,
                node_lines.contains(code),
                &theirs.devices,
                profile,
                found,
                &mut out,
                &mut added,
            );
            via.insert(d.name.to_ascii_lowercase(), v);
        } else if let Some(s) = one.signals.first() {
            point(s, code, &theirs.signals, &via, &mut points, &mut out, &mut added);
        } else {
            // Not a device or a point: the Skipper says what it is.
            added.push(code.to_string());
        }
    }
    if !added.is_empty() {
        if !map.is_empty() {
            map.push('\n');
        }
        map.push_str(&added.join("\n"));
    }
    map.push('\n');
    out.map = map;
    out
}

/// A project device line: sent in the Skipper's form, left to the
/// Skipper's own, or refused (`None`).
fn device(
    d: &MapDevice,
    copied: bool,
    theirs: &[MapDevice],
    profile: &dyn Fn(&str) -> Option<String>,
    found: &dyn Fn(&str) -> Option<String>,
    out: &mut ForSkipper,
    added: &mut Vec<String>,
) -> Option<Via> {
    let name = d.name.as_str();
    let stem = d.driver.strip_prefix(BY_PROFILE);
    let channels = || stem.and_then(profile).map(|t| read_profile(&t).channels);
    let (host, port) = match host_port(&d.endpoint) {
        Some((h, p)) => (Some(h.to_string()), p),
        None => (found(&d.endpoint), ""),
    };
    // One of the Skipper's own (copied in before, the same name, or the
    // same address): its line stays the Skipper's.
    if let Some(n) = theirs.iter().find(|n| {
        copied
            || n.name.eq_ignore_ascii_case(name)
            || host
                .as_deref()
                .is_some_and(|h| host_port(&n.endpoint).is_some_and(|(x, _)| x == h))
    }) {
        if !copied && !n.name.eq_ignore_ascii_case(name) {
            out.notes.push(format!(
                "{name} is the Skipper's own {}{}: its points are the Skipper's.",
                n.name,
                host.as_deref().map_or_else(String::new, |h| format!(" ({h})"))
            ));
        }
        return Some(Via {
            skippers: Some(n.name.clone()),
            driver: n.driver.clone(),
            channels: channels(),
        });
    }
    let refuse = |out: &mut ForSkipper, why: String| {
        out.refused.push(format!("{name}: {why}"));
        None
    };
    let Some(host) = host else {
        out.unfound = true;
        return refuse(
            out,
            format!(
                "the network scan did not find it at {}. Check it is powered and on the Skipper's network",
                d.endpoint
            ),
        );
    };
    let at = format!("{host}{port}");
    if RUNS.contains(&d.driver.as_str()) {
        added.push(device_line(name, &d.driver, &at, &d.params));
        return Some(Via {
            skippers: None,
            driver: d.driver.clone(),
            channels: None,
        });
    }
    let Some(stem) = stem else {
        return refuse(
            out,
            format!(
                "its driver {} is not one (a Skipper drives Modbus TCP and EtherNet/IP devices)",
                d.driver
            ),
        );
    };
    let Some(text) = profile(stem) else {
        return refuse(
            out,
            format!("the project has no {stem}.device to read its channels from"),
        );
    };
    let p = read_profile(&text);
    let Some((t, tparams)) = p.transports.iter().find(|(d, _)| RUNS.contains(&d.as_str())) else {
        return refuse(out, format!("{stem} has no Modbus TCP or EtherNet/IP transport"));
    };
    // The line's own `TRANSPORT.KEY=VALUE` over the profile's.
    let mut ps = tparams.clone();
    for (k, v) in &d.params {
        if let Some(k) = k.strip_prefix(&format!("{t}.")) {
            ps.retain(|(x, _)| x != k);
            ps.push((k.to_string(), v.clone()));
        }
    }
    added.push(device_line(name, t, &at, &ps));
    Some(Via {
        skippers: None,
        driver: t.clone(),
        channels: Some(p.channels),
    })
}

/// A project point line: sent (its channel as the transport's address),
/// left to the Skipper's, or refused.
fn point(
    s: &MapSignal,
    code: &str,
    theirs: &[MapSignal],
    via: &BTreeMap<String, Option<Via>>,
    points: &mut BTreeSet<String>,
    out: &mut ForSkipper,
    added: &mut Vec<String>,
) {
    let name = s.name.as_str();
    if points.contains(&name.to_ascii_lowercase()) {
        return;
    }
    let dev = via.get(&s.device.to_ascii_lowercase());
    // On a refused device: nothing to send it on, and said there.
    if matches!(dev, Some(None)) {
        return;
    }
    let skippers = dev.and_then(|v| v.as_ref()).and_then(|v| v.skippers.as_ref());
    if name.len() > NAME_MAX && skippers.is_none() {
        // A placeholder the page wrote under such a name goes quietly: the
        // program's port is what has to change, and the deploy says so.
        if s.device != "sim" {
            out.refused.push(format!(
                "{name}: a Skipper names a point in at most {NAME_MAX} characters, and this is {}. Give it a shorter name",
                name.len()
            ));
        }
        return;
    }
    let Some(Some(v)) = dev else {
        // A placeholder, or a point on a device the project does not
        // declare (the Skipper says which).
        added.push(code.to_string());
        points.insert(name.to_ascii_lowercase());
        return;
    };
    // The point's address on the device's transport.
    let at = match (&v.channels, s.point.strip_prefix("ch:")) {
        (Some(ch), Some(c)) => ch
            .get(&c.to_ascii_uppercase())
            .and_then(|a| a.get(&v.driver))
            .cloned(),
        (_, None) => Some(s.point.clone()),
        (None, Some(_)) => None,
    };
    if let Some(n) = &v.skippers {
        let same = at.and_then(|at| {
            theirs
                .iter()
                .find(|p| p.dir == s.dir && p.device.eq_ignore_ascii_case(n) && p.point == at)
        });
        match same {
            Some(p) => {
                out.notes.push(format!("{}.", same_words(name, &p.name)));
                out.same.push((name.to_string(), p.name.clone()));
            }
            None => out.notes.push(format!(
                "{name} is left out: its device is the Skipper's {n}, whose points are its own."
            )),
        }
        return;
    }
    let Some(at) = at else {
        let c = s.point.strip_prefix("ch:").unwrap_or(&s.point);
        out.refused
            .push(format!("{name}: {} has no channel {c} on {}", s.device, v.driver));
        return;
    };
    let dir = if s.dir == "in" { "in " } else { "out" };
    let mut line = format!("{dir} {name} {} {} {at}", s.ty, s.device);
    for t in &s.tail {
        line.push(' ');
        line.push_str(t);
    }
    added.push(line);
    points.insert(name.to_ascii_lowercase());
}
