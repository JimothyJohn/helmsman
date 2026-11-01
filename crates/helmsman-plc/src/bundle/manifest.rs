//! A bundle's manifest: who made it, what it is, and its **capability
//! manifest**, what its content can touch. The capabilities are never taken
//! on the bundle's word: [`capabilities`] computes them from the content
//! (every module compiled and held to its own manifest, as the box does at
//! load), and a bundle whose manifest says anything else, more or less, is
//! refused. So a reviewer, a person or the policy can read "this bundle
//! drives 3 outputs" here and trust it without reading the ST.
//!
//! ```text
//! helmsman-bundle-manifest 1
//! name         conveyor-kit
//! version      1.2.0
//! author       Acme Automation <support@acme.example>
//! description  Belt start/stop with jam stop and a stack light.
//! modules      conveyor lamp
//! reads        jam start
//! drives       lamp motor
//! q-bytes      0
//! devices      remote-io
//! drivers      modbus-tcp
//! ```
//!
//! Every key once, in this order; `description` may be left out. Lists
//! are sorted, without repeats, and may be empty (the key alone). Signal
//! names are lowercase (ST is case-insensitive). `reads` and `drives` are
//! the union over the modules (plus the calendar's `drive` lines), `q-bytes`
//! the total of their `%Q` windows, `devices` what the bundle's signal-map
//! template names, and `drivers` the drivers those devices use (a WASM
//! driver as a map names it, `wasm:NAME`: the bundle carries none, the
//! box loads its own file). The file is canonical: one space
//! after each key, no comments, a newline after every line, so one meaning
//! has one byte string and a signature covers exactly what is shown.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::api::swap::compile_set;
use crate::manifest::{Manifest, hidden};
use crate::spec::{Defaults, name_ok};
use crate::store::ChangeSet;

/// The manifest's first line.
pub const HEADER: &str = "helmsman-bundle-manifest 1";
/// Longest `author`, in bytes.
pub const MAX_AUTHOR: usize = 256;
/// Longest `description`, in bytes.
pub const MAX_DESCRIPTION: usize = 1024;
/// Longest `version`, in bytes.
pub const MAX_VERSION: usize = 32;
/// Most `%Q` bytes a manifest may declare.
pub const MAX_Q: usize = 1 << 16;

/// What a bundle's content can touch, as computed from it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Module names.
    pub modules: BTreeSet<String>,
    /// Signals any module reads, lowercase.
    pub reads: BTreeSet<String>,
    /// Signals any module or the calendar drives, lowercase.
    pub drives: BTreeSet<String>,
    /// `%Q` bytes, summed over the modules' windows.
    pub q_bytes: usize,
    /// Device ids the signal-map template declares.
    pub devices: BTreeSet<String>,
    /// Driver names those devices use, and `wasm:NAME` for each WASM
    /// driver the bundle carries.
    pub drivers: BTreeSet<String>,
}

impl Capabilities {
    /// Whether the content can move an output at all.
    #[must_use]
    pub fn drives_anything(&self) -> bool {
        !self.drives.is_empty() || self.q_bytes > 0
    }

    /// Every way `declared` (a manifest's) differs from `self` (the
    /// content's), one line each; empty when they agree.
    #[must_use]
    pub fn differences(&self, declared: &Self) -> Vec<String> {
        let mut out = Vec::new();
        let mut set = |key: &str, actual: &BTreeSet<String>, said: &BTreeSet<String>| {
            for n in actual.difference(said) {
                out.push(format!(
                    "{key}: the content has {n}, which the manifest does not declare (understated)"
                ));
            }
            for n in said.difference(actual) {
                out.push(format!(
                    "{key}: the manifest declares {n}, which the content does not have (overstated)"
                ));
            }
        };
        set("modules", &self.modules, &declared.modules);
        set("reads", &self.reads, &declared.reads);
        set("drives", &self.drives, &declared.drives);
        set("devices", &self.devices, &declared.devices);
        set("drivers", &self.drivers, &declared.drivers);
        if self.q_bytes != declared.q_bytes {
            let how = if self.q_bytes > declared.q_bytes {
                "understated"
            } else {
                "overstated"
            };
            out.push(format!(
                "q-bytes: the modules drive {} %Q bytes, the manifest declares {} ({how})",
                self.q_bytes, declared.q_bytes
            ));
        }
        out
    }
}

/// A parsed bundle manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleManifest {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    pub caps: Capabilities,
}

fn version_ok(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= MAX_VERSION
        && v.as_bytes()[0].is_ascii_alphanumeric()
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'+' || b == b'-')
}

fn text_ok(v: &str, max: usize) -> Result<(), String> {
    if v.trim().is_empty() {
        Err("is blank".into())
    } else if v.len() > max {
        Err(format!("is longer than {max} bytes"))
    } else if v.chars().any(hidden) {
        Err("holds a control or invisible character".into())
    } else if v.trim() != v {
        Err("has blanks at an end".into())
    } else {
        Ok(())
    }
}

fn signal_ok(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A device id or driver name a manifest can carry: printable ASCII, no
/// `#`, 1..=64 bytes.
fn word_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_graphic() && b != b'#')
}

impl BundleManifest {
    /// The canonical text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = format!("{HEADER}\n");
        let _ = writeln!(s, "name {}", self.name);
        let _ = writeln!(s, "version {}", self.version);
        let _ = writeln!(s, "author {}", self.author);
        if let Some(d) = &self.description {
            let _ = writeln!(s, "description {d}");
        }
        let list = |s: &mut String, key: &str, v: &BTreeSet<String>| {
            s.push_str(key);
            for n in v {
                s.push(' ');
                s.push_str(n);
            }
            s.push('\n');
        };
        list(&mut s, "modules", &self.caps.modules);
        list(&mut s, "reads", &self.caps.reads);
        list(&mut s, "drives", &self.caps.drives);
        let _ = writeln!(s, "q-bytes {}", self.caps.q_bytes);
        list(&mut s, "devices", &self.caps.devices);
        list(&mut s, "drivers", &self.caps.drivers);
        s
    }

    /// Parses the canonical text, and only that.
    ///
    /// # Errors
    /// Every problem, as `line: why` (line 0 for the file as a whole).
    pub fn parse(src: &str) -> Result<Self, Vec<String>> {
        let mut errs = Vec::new();
        let mut lines = src.split('\n').enumerate().map(|(n, l)| (n + 1, l));
        if lines.next().map(|(_, l)| l) != Some(HEADER) {
            return Err(vec![format!("1: the first line must be {HEADER:?}")]);
        }
        let mut name = None;
        let mut version = None;
        let mut author = None;
        let mut description = None;
        let mut caps = Capabilities::default();
        let mut seen = BTreeSet::new();
        for (n, line) in lines {
            if line.is_empty() {
                continue;
            }
            let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
            if !seen.insert(key.to_string()) {
                errs.push(format!("{n}: {key} given twice"));
                continue;
            }
            let words = || rest.split(' ').filter(|w| !w.is_empty()).map(str::to_string);
            let mut bad = |why: String| errs.push(format!("{n}: {key} {why}"));
            match key {
                "name" if name_ok(rest) => name = Some(rest.to_string()),
                "name" => bad("is not a name (1..=64 of A-Z a-z 0-9 _ -)".into()),
                "version" if version_ok(rest) => version = Some(rest.to_string()),
                "version" => bad(format!(
                    "is not a version (1..={MAX_VERSION} of A-Z a-z 0-9 . + -, starting alphanumeric)"
                )),
                "author" => match text_ok(rest, MAX_AUTHOR) {
                    Ok(()) => author = Some(rest.to_string()),
                    Err(e) => bad(e),
                },
                "description" => match text_ok(rest, MAX_DESCRIPTION) {
                    Ok(()) => description = Some(rest.to_string()),
                    Err(e) => bad(e),
                },
                "modules" => {
                    caps.modules = words().collect();
                    if let Some(w) = caps.modules.iter().find(|w| !name_ok(w)) {
                        bad(format!("{w:?} is not a module name"));
                    }
                }
                "reads" | "drives" => {
                    let set: BTreeSet<String> = words().collect();
                    if let Some(w) = set.iter().find(|w| !signal_ok(w)) {
                        bad(format!("{w:?} is not a lowercase signal name"));
                    }
                    if key == "reads" {
                        caps.reads = set;
                    } else {
                        caps.drives = set;
                    }
                }
                "q-bytes" => match rest.parse::<usize>() {
                    Ok(q) if q <= MAX_Q && q.to_string() == rest => caps.q_bytes = q,
                    _ => bad(format!("is not a whole number up to {MAX_Q}")),
                },
                "devices" | "drivers" => {
                    let set: BTreeSet<String> = words().collect();
                    if let Some(w) = set.iter().find(|w| !word_ok(w)) {
                        bad(format!("{w:?} is not a device or driver name"));
                    }
                    if key == "devices" {
                        caps.devices = set;
                    } else {
                        caps.drivers = set;
                    }
                }
                _ => errs.push(format!("{n}: unknown key {key:?}")),
            }
        }
        for key in [
            "name", "version", "author", "modules", "reads", "drives", "q-bytes", "devices", "drivers",
        ] {
            if !seen.contains(key) {
                errs.push(format!("0: {key} is missing"));
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        let (Some(name), Some(version), Some(author)) = (name, version, author) else {
            return Err(vec!["0: name, version and author are required".into()]);
        };
        let m = Self {
            name,
            version,
            author,
            description,
            caps,
        };
        if m.render() != src {
            return Err(vec![
                "0: not in canonical form (keys in order, one space, sorted lists without repeats, a newline after each line)"
                    .into(),
            ]);
        }
        Ok(m)
    }
}

/// What `cs` can touch, computed from it: the change set as the store
/// takes it, every module compiled against its windows (`d` for `%M`) and
/// held to its manifest, the calendar and the map template parsed.
///
/// # Errors
/// Every reason the content is not one a box would run.
pub fn capabilities(cs: &ChangeSet, d: &Defaults) -> Result<Capabilities, Vec<String>> {
    cs.validate()?;
    compile_set(cs, d)?;
    let mut caps = Capabilities::default();
    let mut errs = Vec::new();
    for (name, f) in &cs.modules {
        let m = Manifest::parse(&f.manifest)
            .map_err(|_| vec![format!("module {name}: manifest does not parse")])?;
        caps.modules.insert(name.clone());
        caps.reads.extend(m.reads.iter().map(|s| s.to_ascii_lowercase()));
        caps.drives
            .extend(m.drives.iter().map(|s| s.to_ascii_lowercase()));
        caps.q_bytes += m.q.map_or(0, |w| w.len);
    }
    if caps.q_bytes > MAX_Q {
        errs.push(format!(
            "the modules drive {} %Q bytes, more than {MAX_Q}",
            caps.q_bytes
        ));
    }
    if let Some(c) = &cs.calendar {
        match crate::calendar::text::parse(c) {
            Ok(k) => caps
                .drives
                .extend(k.drives.iter().map(|d| d.signal.to_ascii_lowercase())),
            Err(e) => {
                let lines: Vec<String> = e.iter().map(|(n, w)| format!("{n}: {w}")).collect();
                errs.push(format!("calendar does not parse: {}", lines.join("; ")));
            }
        }
    }
    if let Some(m) = &cs.io_map {
        // validate() already parsed it.
        if let Ok(f) = helmsman_io::text::parse(m) {
            for dev in &f.map.devices {
                if !word_ok(&dev.id) || !word_ok(&dev.driver) {
                    errs.push(format!(
                        "device {:?} (driver {:?}) cannot be named in a bundle manifest",
                        dev.id, dev.driver
                    ));
                }
                caps.devices.insert(dev.id.clone());
                caps.drivers.insert(dev.driver.clone());
            }
        }
    }
    for s in caps.reads.iter().chain(&caps.drives) {
        if !signal_ok(s) {
            errs.push(format!("signal {s:?} cannot be named in a bundle manifest"));
        }
    }
    if errs.is_empty() { Ok(caps) } else { Err(errs) }
}
