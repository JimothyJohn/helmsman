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

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The transports a Skipper drives, as a profile and a map name them.
const RUNS: [&str; 2] = ["enip-io", "modbus-tcp"];

/// How a map names a device by its profile: `device:STEM`.
const BY_PROFILE: &str = "device:";

/// A device line from its parts, with no trailing space.
fn device_line(name: &str, driver: &str, at: &str, params: &[String]) -> String {
    let mut l = format!("device {name} {driver} {at}");
    for p in params {
        l.push(' ');
        l.push_str(p);
    }
    l
}

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

/// One profile, as far as a Skipper needs it.
#[derive(Debug, Default)]
struct Profile {
    /// `transport DRIVER k=v ...`, in the profile's order.
    transports: Vec<(String, Vec<(String, String)>)>,
    /// Channel name (upper case) to its address on each transport.
    channels: BTreeMap<String, BTreeMap<String, String>>,
}

fn read_profile(text: &str) -> Profile {
    let mut p = Profile::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            ["transport", driver, rest @ ..] => p.transports.push((
                (*driver).to_string(),
                rest.iter()
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )),
            ["in" | "out", name, _ty, rest @ ..] => {
                let at: BTreeMap<String, String> = rest
                    .iter()
                    .filter_map(|kv| kv.split_once('='))
                    .filter(|(k, _)| p.transports.iter().any(|(d, _)| d == k) || RUNS.contains(k))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                p.channels.insert(name.to_ascii_uppercase(), at);
            }
            _ => {}
        }
    }
    p
}

/// The host of an endpoint, `HOST[:PORT]`; `None` for an identity
/// (`cip=`, `mac=`, `modbus=`) or nothing.
fn host_of(endpoint: &str) -> Option<&str> {
    if endpoint.is_empty() || endpoint.contains('=') {
        return None;
    }
    Some(
        endpoint
            .rsplit_once(':')
            .filter(|(_, p)| p.parse::<u16>().is_ok())
            .map_or(endpoint, |(h, _)| h),
    )
}

/// A device line's words: name, driver, endpoint, params.
fn device_words(line: &str) -> Option<(&str, &str, &str, Vec<&str>)> {
    let mut w = line.split_whitespace();
    if w.next()? != "device" {
        return None;
    }
    let name = w.next()?;
    let driver = w.next()?;
    let endpoint = w.next().unwrap_or("");
    Some((name, driver, endpoint, w.collect()))
}

/// What became of a project device.
enum Became {
    /// Sent under its own name: the transport its points are on, and the
    /// profile's channels when it was by its profile.
    Sent {
        driver: String,
        channels: Option<BTreeMap<String, BTreeMap<String, String>>>,
    },
    /// The Skipper has it already, as device `name` on `driver`; the
    /// project's profile's channels for it, when it was by its profile.
    Skippers {
        name: String,
        driver: String,
        channels: Option<BTreeMap<String, BTreeMap<String, String>>>,
    },
    /// Not sent (refused above).
    Not,
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
    /// The devices among them named by an identity no scan has given an
    /// address for: a scan may find them.
    pub unfound: Vec<String>,
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

/// The longest point or device name a Skipper takes
/// (`helmsman_nodecore::iomap::NAME_MAX`).
pub const NAME_MAX: usize = 32;

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
    let node_devices = devices(node);
    let node_points: Vec<String> = node
        .lines()
        .filter_map(|l| point_name(l.split('#').next().unwrap_or("")))
        .map(str::to_ascii_lowercase)
        .collect();
    // The Skipper's points: name, `in`, device (lower case), address.
    let node_at: Vec<(String, bool, String, String)> = node
        .lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split('#').next().unwrap_or("").split_whitespace().collect();
            match w.as_slice() {
                [d @ ("in" | "out"), name, _, dev, addr, ..] => Some((
                    (*name).to_string(),
                    *d == "in",
                    dev.to_ascii_lowercase(),
                    (*addr).to_string(),
                )),
                _ => None,
            }
        })
        .collect();
    let node_driver = |dev: &str| {
        node.lines()
            .filter_map(|l| device_words(l.split('#').next().unwrap_or("")))
            .find(|(n, ..)| n.eq_ignore_ascii_case(dev))
            .map(|(_, d, ..)| d.to_string())
            .unwrap_or_default()
    };
    let mut map = node.trim_end().to_string();
    let mut added = String::new();
    let mut became: BTreeMap<String, Became> = BTreeMap::new();
    let mut points: Vec<String> = node_points.clone();
    for l in project.unwrap_or("").lines() {
        let code = l.split('#').next().unwrap_or("").trim();
        if code.is_empty() {
            if l.trim() == crate::placeholders::HEADER && !map.contains(crate::placeholders::HEADER) {
                let _ = writeln!(added, "{}", l.trim());
            }
            continue;
        }
        if node.lines().any(|n| n.trim() == code) {
            // The Skipper's own line, copied into the project before.
            if let Some((name, ..)) = device_words(code) {
                became.insert(
                    name.to_ascii_lowercase(),
                    Became::Skippers {
                        name: name.to_string(),
                        driver: node_driver(name),
                        channels: None,
                    },
                );
            }
            continue;
        }
        if let Some((name, driver, endpoint, params)) = device_words(code) {
            let key = name.to_ascii_lowercase();
            let host = host_of(endpoint).map(str::to_string).or_else(|| found(endpoint));
            if let Some((n, _)) = node_devices
                .iter()
                .find(|(n, h)| n.eq_ignore_ascii_case(name) || host.as_deref().is_some_and(|x| x == h))
            {
                if !n.eq_ignore_ascii_case(name) {
                    out.notes.push(format!(
                        "{name} is the Skipper's own {n}{}: its points are the Skipper's.",
                        host.as_deref().map_or_else(String::new, |h| format!(" ({h})"))
                    ));
                }
                let channels = driver
                    .strip_prefix(BY_PROFILE)
                    .and_then(profile)
                    .map(|t| read_profile(&t).channels);
                became.insert(
                    key,
                    Became::Skippers {
                        name: n.clone(),
                        driver: node_driver(n),
                        channels,
                    },
                );
                continue;
            }
            let Some(host) = host else {
                out.refused.push(format!(
                    "{name}: the network scan did not find it at {endpoint}. Check it is powered and on the Skipper's network"
                ));
                out.unfound.push(name.to_string());
                became.insert(key, Became::Not);
                continue;
            };
            let port = endpoint
                .rsplit_once(':')
                .filter(|(_, p)| p.parse::<u16>().is_ok() && !endpoint.contains('='))
                .map(|(_, p)| format!(":{p}"))
                .unwrap_or_default();
            if RUNS.contains(&driver) {
                let kept: Vec<String> = params
                    .iter()
                    .filter(|kv| kv.split_once('=').is_some_and(|(k, _)| param_kept(driver, k)))
                    .map(|kv| (*kv).to_string())
                    .collect();
                let _ = writeln!(
                    added,
                    "{}",
                    device_line(name, driver, &format!("{host}{port}"), &kept)
                );
                became.insert(
                    key,
                    Became::Sent {
                        driver: driver.to_string(),
                        channels: None,
                    },
                );
                continue;
            }
            let Some(stem) = driver.strip_prefix(BY_PROFILE) else {
                out.refused.push(format!(
                    "{name}: its driver {driver} is not one (a Skipper drives Modbus TCP and EtherNet/IP devices)"
                ));
                became.insert(key, Became::Not);
                continue;
            };
            let Some(text) = profile(stem) else {
                out.refused.push(format!(
                    "{name}: the project has no {stem}.device to read its channels from"
                ));
                became.insert(key, Became::Not);
                continue;
            };
            let p = read_profile(&text);
            let Some((t, tparams)) = p.transports.iter().find(|(d, _)| RUNS.contains(&d.as_str())) else {
                out.refused.push(format!(
                    "{name}: {stem} has no Modbus TCP or EtherNet/IP transport"
                ));
                became.insert(key, Became::Not);
                continue;
            };
            // The line's own `TRANSPORT.KEY=VALUE` over the profile's.
            let mut ps: Vec<(String, String)> = tparams.clone();
            for kv in &params {
                if let Some((k, v)) = kv.split_once('=')
                    && let Some(k) = k.strip_prefix(&format!("{t}."))
                {
                    ps.retain(|(x, _)| x != k);
                    ps.push((k.to_string(), v.to_string()));
                }
            }
            let ps: Vec<String> = ps
                .iter()
                .filter(|(k, _)| param_kept(t, k))
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            let _ = writeln!(added, "{}", device_line(name, t, &format!("{host}{port}"), &ps));
            became.insert(
                key,
                Became::Sent {
                    driver: t.clone(),
                    channels: Some(p.channels),
                },
            );
            continue;
        }
        let Some(pname) = point_name(code) else {
            // Not a device or a point: the Skipper says what it is.
            let _ = writeln!(added, "{code}");
            continue;
        };
        if points.iter().any(|n| n.eq_ignore_ascii_case(pname)) {
            continue;
        }
        let w: Vec<&str> = code.split_whitespace().collect();
        // A point on a Skipper device, or one not sent, is not sent itself.
        let on_skippers = w.get(3).is_some_and(|d| {
            matches!(
                became.get(&d.to_ascii_lowercase()),
                Some(Became::Skippers { .. } | Became::Not)
            )
        });
        // A placeholder the page wrote under a name a Skipper cannot take
        // goes: the program's port is what has to change, and the deploy
        // says so by the port ([`crate::box_projects`]).
        if pname.len() > NAME_MAX && w.get(3) == Some(&"sim") {
            continue;
        }
        if pname.len() > NAME_MAX && !on_skippers {
            out.refused.push(format!(
                "{pname}: a Skipper names a point in at most {NAME_MAX} characters, and this is {}. Give it a shorter name",
                pname.len()
            ));
            continue;
        }
        let (dir, ty, dev, addr, rest) = match w.as_slice() {
            [dir, _, ty, dev, addr, rest @ ..] => (*dir, *ty, *dev, *addr, rest),
            [_, _, _, "sim", ..] => {
                let _ = writeln!(added, "{code}");
                points.push(pname.to_ascii_lowercase());
                continue;
            }
            _ => {
                let _ = writeln!(added, "{code}");
                continue;
            }
        };
        if dev == "sim" {
            let _ = writeln!(added, "{code}");
            points.push(pname.to_ascii_lowercase());
            continue;
        }
        let dir = if dir == "in" { "in " } else { "out" };
        match became.get(&dev.to_ascii_lowercase()) {
            Some(Became::Sent { driver, channels }) => {
                let addr = match (channels, addr.strip_prefix("ch:")) {
                    (Some(ch), Some(c)) => {
                        let Some(a) = ch.get(&c.to_ascii_uppercase()).and_then(|a| a.get(driver)) else {
                            out.refused
                                .push(format!("{pname}: {dev} has no channel {c} on {driver}"));
                            continue;
                        };
                        a.clone()
                    }
                    _ => addr.to_string(),
                };
                let mut line = format!("{dir} {pname} {ty} {dev} {addr}");
                for r in rest {
                    line.push(' ');
                    line.push_str(r);
                }
                let _ = writeln!(added, "{line}");
                points.push(pname.to_ascii_lowercase());
            }
            Some(Became::Skippers {
                name: n,
                driver,
                channels,
            }) => {
                let at = match (channels, addr.strip_prefix("ch:")) {
                    (Some(ch), Some(c)) => ch
                        .get(&c.to_ascii_uppercase())
                        .and_then(|a| a.get(driver))
                        .cloned(),
                    (_, None) => Some(addr.to_string()),
                    (None, Some(_)) => None,
                };
                let input = dir.trim() == "in";
                let theirs = at.and_then(|at| {
                    node_at
                        .iter()
                        .find(|(_, i, d, a)| *i == input && d.eq_ignore_ascii_case(n) && *a == at)
                });
                match theirs {
                    Some((sp, ..)) => {
                        out.notes.push(format!(
                            "{pname} is the Skipper's {sp} (the same channel of {n}): the program should use {sp}."
                        ));
                        out.same.push((pname.to_string(), sp.clone()));
                    }
                    None => out.notes.push(format!(
                        "{pname} is left out: its device is the Skipper's {n}, whose points are its own."
                    )),
                }
            }
            Some(Became::Not) => {}
            None => {
                // On a Skipper device by name, or one not declared: the
                // Skipper says which.
                let _ = writeln!(added, "{code}");
                points.push(pname.to_ascii_lowercase());
            }
        }
    }
    let added = added.trim_end();
    if !added.is_empty() {
        if !map.is_empty() {
            map.push('\n');
        }
        map.push_str(added);
    }
    map.push('\n');
    out.map = map;
    out
}

/// The devices of a map: name and host (empty for an identity).
#[must_use]
pub fn devices(map: &str) -> Vec<(String, String)> {
    map.lines()
        .filter_map(|l| device_words(l.split('#').next().unwrap_or("")))
        .map(|(name, _, endpoint, _)| (name.to_string(), host_of(endpoint).unwrap_or("").to_string()))
        .collect()
}

/// The name of an `in` or `out` line.
fn point_name(code: &str) -> Option<&str> {
    let mut w = code.split_whitespace();
    match w.next()? {
        "in" | "out" => w.next(),
        _ => None,
    }
}
