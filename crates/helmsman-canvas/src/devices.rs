//! The left pane's Devices tab (Nick, 2026-11-02): the devices the I/O map
//! names, each with its points under it, in a tree like the Files tab's.
//! Built from a map's text (the open project's `io.map`, or the map the box
//! runs) joined with live values and each link's state; the same listing,
//! as text, is what the agent reads ([`describe`]; the box's `list_devices`
//! tool renders the same from the API). A right-click on the blank space
//! adds a device the way a file is added: a form in the middle of the
//! screen, then one `device` line in the map. "Import device…" takes a
//! vendor's EDS: the file joins the project as `NAME.eds` and the form
//! opens filled in with `eds=NAME.eds` (the box reads it when the map
//! loads; `device-import`). One map mixes protocols (`modbus-tcp`,
//! `enip-io`, `logix-tag`); the box runs every link at once.
//!
//! Under the map's devices, "On the network" (`devices-repo`): Scan asks
//! the box for one `ListIdentity` sweep (`POST /api/devices/discover`; the
//! simulation answers with its simulated E1213) and lists every device
//! that answered, with whether a map device already binds it and what the
//! public device library knows of it (identity first, then vendor and
//! model text; `helmsman_project::devlib`). A right-click on one not in
//! the map offers "Add to the map…": the New device form opens filled in
//! from the library's match, the device named by its identity.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use helmsman_iec::Value;
use helmsman_project::devlib::Index;
use helmsman_project::iomap::{self, MapText};
use serde_json::Value as Json;

use crate::editor::Chrome;
use crate::st_view::{TAP, show};
use crate::theme::theme;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// How the box is doing with a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// Up, and where the box found the device when its driver said (a
    /// `mac=` or `cip=` endpoint resolved; `device-by-identity`).
    Up(Option<String>),
    Down(String),
    /// The box has not said yet.
    Unknown,
    /// The simulation: no link to have.
    Simulated,
}

/// One point of a device: a signal of the map, with what it holds now.
#[derive(Debug, Clone, PartialEq)]
pub struct PointNode {
    pub signal: String,
    /// `in` or `out`.
    pub dir: String,
    pub ty: String,
    pub point: String,
    pub unit: Option<String>,
    pub value: Option<Value>,
    pub quality: Option<String>,
    pub forced: bool,
}

/// One device, with its points.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceNode {
    pub name: String,
    pub driver: String,
    pub endpoint: String,
    pub params: Vec<(String, String)>,
    pub state: LinkState,
    pub points: Vec<PointNode>,
}

/// A signal's live side, from whoever runs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub value: Option<Value>,
    pub quality: Option<String>,
    pub forced: bool,
}

/// What a reserved device id stands for.
fn pseudo(name: &str) -> &'static str {
    match name {
        "box" => "this box's own channels",
        "screen" => "the floor screen",
        "module" => "other modules' outputs",
        _ => "not declared in the map",
    }
}

/// The map's devices in their order, then the ids its signals name that no
/// `device` line declares (`box`, `screen`, `module`, or a typo), each
/// with its points; `live` gives a signal's value, `state` a device's link.
pub fn tree(
    map: &MapText,
    live: &dyn Fn(&str) -> Option<Live>,
    state: &dyn Fn(&str) -> LinkState,
) -> Vec<DeviceNode> {
    let mut nodes: Vec<DeviceNode> = map
        .devices
        .iter()
        .map(|d| DeviceNode {
            name: d.name.clone(),
            driver: d.driver.clone(),
            endpoint: d.endpoint.clone(),
            params: d.params.clone(),
            state: state(&d.name),
            points: Vec::new(),
        })
        .collect();
    for s in &map.signals {
        let at = nodes.iter().position(|n| n.name == s.device).unwrap_or_else(|| {
            nodes.push(DeviceNode {
                name: s.device.clone(),
                driver: pseudo(&s.device).to_string(),
                endpoint: String::new(),
                params: Vec::new(),
                state: state(&s.device),
                points: Vec::new(),
            });
            nodes.len() - 1
        });
        let l = live(&s.name).unwrap_or_default();
        nodes[at].points.push(PointNode {
            signal: s.name.clone(),
            dir: s.dir.clone(),
            ty: s.ty.clone(),
            point: s.point.clone(),
            unit: s.unit.clone(),
            value: l.value,
            quality: l.quality,
            forced: l.forced,
        });
    }
    nodes
}

fn state_text(s: &LinkState) -> String {
    match s {
        LinkState::Up(None) => "up".into(),
        LinkState::Up(Some(a)) => format!("up at {a}"),
        LinkState::Down(why) => format!("down: {why}"),
        LinkState::Unknown => "not reached yet".into(),
        LinkState::Simulated => "simulated".into(),
    }
}

/// The tree as text, one device a line and its points indented under it:
/// what the agent gets as context.
#[must_use]
pub fn describe(nodes: &[DeviceNode]) -> String {
    let mut out = String::new();
    for n in nodes {
        let params: Vec<String> = n.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
        out.push_str(&n.name);
        out.push_str("  ");
        out.push_str(&n.driver);
        if !n.endpoint.is_empty() {
            out.push(' ');
            out.push_str(&n.endpoint);
        }
        if !params.is_empty() {
            out.push(' ');
            out.push_str(&params.join(" "));
        }
        out.push_str("  [");
        out.push_str(&state_text(&n.state));
        out.push_str("]\n");
        for p in &n.points {
            let _ = write!(out, "  {:<3} {} {} {}", p.dir, p.signal, p.ty, p.point);
            if let Some(u) = &p.unit {
                let _ = write!(out, " unit={u}");
            }
            if p.value.is_some() {
                let _ = write!(out, " = {}", show(p.value));
            }
            if p.forced {
                out.push_str(" FORCED");
            }
            out.push('\n');
        }
    }
    out
}

/// A protocol the form offers: the driver, what a person calls it, an
/// address to show as a hint, the settings to start from.
pub const PROTOCOLS: [(&str, &str, &str, &str); 2] = [
    (
        "modbus-tcp",
        "Modbus TCP",
        "192.168.1.50:502",
        "unit=1 timeout_ms=300 poll_ms=20",
    ),
    (
        "enip-io",
        "EtherNet/IP",
        "192.168.1.60",
        "profile=moxa-e1213 rpi_ms=100 poll_ms=20",
    ),
];

/// The "New device" form's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDevice {
    pub name: String,
    /// An index into [`PROTOCOLS`].
    pub protocol: usize,
    pub endpoint: String,
    pub params: String,
    /// Why the last Add was refused.
    pub error: Option<String>,
    /// The project's EDS file this device comes from (Import device…).
    pub eds: Option<String>,
}

impl Default for NewDevice {
    fn default() -> Self {
        Self {
            name: String::new(),
            protocol: 0,
            endpoint: String::new(),
            params: PROTOCOLS[0].3.to_string(),
            error: None,
            eds: None,
        }
    }
}

/// Whether an uploaded file is an EDS, by its extension.
#[must_use]
pub fn is_eds(file: &str) -> bool {
    file.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case("eds"))
}

/// The `ProdName` of an EDS's `[Device]` section, as the file has it:
/// `ProdName = "E1213";` with the generator's quoting and `$` comments.
/// A skim, not a parse: the box parses the file when the map loads.
#[must_use]
pub fn eds_product(text: &str) -> Option<String> {
    let mut in_device = false;
    for line in text.lines().take(4000) {
        let line = line.split('$').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_device = line.eq_ignore_ascii_case("[Device]");
            continue;
        }
        if !in_device {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "ProdName" {
            continue;
        }
        let v = value.trim().trim_end_matches(';').trim().trim_matches('"').trim();
        return (!v.is_empty() && v.len() <= 80).then(|| v.to_string());
    }
    None
}

impl NewDevice {
    /// The form for a device read from the project's EDS `file`
    /// (`NAME.eds`): EtherNet/IP, named after the EDS's product (else the
    /// file), `eds=` set; the address is what is left to type.
    #[must_use]
    pub fn from_eds(file: &str, text: &str) -> Self {
        let stem = file.strip_suffix(".eds").unwrap_or(file);
        let name = eds_product(text)
            .map(|p| slug(&p))
            .filter(|s| iomap::device_name_ok(s))
            .unwrap_or_else(|| slug(stem));
        Self {
            name,
            protocol: 1,
            endpoint: String::new(),
            params: format!("eds={file} rpi_ms=100 poll_ms=20"),
            error: None,
            eds: Some(file.to_string()),
        }
    }

    /// The `device` line, or why not.
    ///
    /// # Errors
    /// As [`iomap::device_line`].
    pub fn line(&self) -> Result<String, String> {
        let driver = PROTOCOLS.get(self.protocol).map_or("", |p| p.0);
        iomap::device_line(&self.name, driver, &self.endpoint, &self.params)
    }
}

/// A product name as a device id: lower case, `a-z 0-9 _`, runs of
/// anything else one `_`, starting with a letter (`E1213` → `e1213`,
/// `ioLogik E1213-T` → `iologik_e1213_t`).
#[must_use]
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').to_string();
    let out = if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("d{out}")
    } else {
        out
    };
    out.chars().take(48).collect()
}

/// What the tab asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceAct {
    /// Append this `device` line to the map (`io.map`, made if missing).
    Add(String),
    /// Scan the network (the box: one `ListIdentity` sweep; the
    /// simulation: its simulated E1213).
    Scan,
}

/// What the device library knows of a device found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryHit {
    pub folder: String,
    pub vendor: String,
    pub model: String,
    pub driver: Option<String>,
    pub profile: Option<String>,
}

impl LibraryHit {
    /// The library's entry as the row shows it (the browser fetches the
    /// index itself; a box answers with the hit already made).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[must_use]
    pub fn of(e: &helmsman_project::devlib::Entry) -> Self {
        let p = e.protocol("enip-io").or_else(|| e.protocols.first());
        Self {
            folder: e.folder.clone(),
            vendor: e.vendor.clone(),
            model: e.model.clone(),
            driver: p.map(|p| p.driver.clone()),
            profile: p.and_then(|p| p.profile.clone()),
        }
    }
}

/// A device that answered a scan: what the wire gave, whether the map
/// has it, what the library knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub address: String,
    pub mac: Option<String>,
    pub vendor_id: u16,
    pub product_code: u16,
    pub revision: String,
    pub serial: String,
    pub product_name: String,
    /// `cip=V:P:S`: the identity a map may name it by.
    pub cip: String,
    /// The map device that binds it, if one does.
    pub mapped: Option<String>,
    pub library: Option<LibraryHit>,
}

impl Found {
    /// The simulation's E1213 (`helmsman_adapter::DeviceConfig::moxa_e1213`'s
    /// identity, at Moxa's factory address), `mapped` against `map`.
    #[must_use]
    pub fn simulated_e1213(map: &MapText) -> Self {
        let mut f = Self {
            address: "192.168.127.254".into(),
            mac: None,
            vendor_id: 991,
            product_code: 515,
            revision: "4.1".into(),
            serial: "0x00e12130".into(),
            product_name: "E1213".into(),
            cip: "cip=991:515:0x00e12130".into(),
            mapped: None,
            library: None,
        };
        f.mapped = mapped_in(map, &f);
        f
    }

    /// Reads a `POST /api/devices/discover` body; `None` if it is not one.
    /// Returns the devices and, when the box had no library, why.
    #[must_use]
    pub fn parse_discovery(j: &Json) -> Option<(Vec<Self>, Option<String>)> {
        let list = j.get("devices")?.as_array()?;
        let text = |v: &Json| {
            v.as_str().map(|s| {
                s.chars()
                    .filter(|c| !c.is_control())
                    .take(120)
                    .collect::<String>()
            })
        };
        let mut out = Vec::new();
        for d in list.iter().take(256) {
            let num = |k: &str| {
                d.get(k)
                    .and_then(Json::as_u64)
                    .and_then(|n| u16::try_from(n).ok())
            };
            let (Some(address), Some(vendor_id), Some(product_code)) = (
                d.get("address").and_then(text),
                num("vendor_id"),
                num("product_code"),
            ) else {
                continue;
            };
            let library = d.get("library").filter(|l| l.is_object()).map(|l| LibraryHit {
                folder: l.get("folder").and_then(text).unwrap_or_default(),
                vendor: l.get("vendor").and_then(text).unwrap_or_default(),
                model: l.get("model").and_then(text).unwrap_or_default(),
                driver: l.get("driver").and_then(text),
                profile: l.get("profile").and_then(text),
            });
            out.push(Self {
                address,
                mac: d.get("mac").and_then(text),
                vendor_id,
                product_code,
                revision: d.get("revision").and_then(text).unwrap_or_default(),
                serial: d.get("serial").and_then(text).unwrap_or_default(),
                product_name: d.get("product_name").and_then(text).unwrap_or_default(),
                cip: d
                    .get("cip")
                    .and_then(text)
                    .unwrap_or_else(|| format!("cip={vendor_id}:{product_code}")),
                mapped: d.get("mapped").and_then(text),
                library,
            });
        }
        let note = j.get("library").and_then(|l| l.get("unavailable")).and_then(text);
        Some((out, note))
    }
}

/// The map device that binds the device at `f`, by host, `mac=` or `cip=`.
#[must_use]
pub fn mapped_in(map: &MapText, f: &Found) -> Option<String> {
    map.devices
        .iter()
        .find(|d| {
            let e = d.endpoint.as_str();
            if let Some(m) = e.strip_prefix("mac=") {
                return f
                    .mac
                    .as_deref()
                    .is_some_and(|fm| fm.replace(':', "-").eq_ignore_ascii_case(&m.replace(':', "-")));
            }
            if let Some(c) = e.strip_prefix("cip=") {
                let mut it = c.split(':');
                let v = it.next().and_then(|x| x.parse::<u16>().ok());
                let p = it.next().and_then(|x| x.parse::<u16>().ok());
                let s = it.next();
                return v == Some(f.vendor_id)
                    && p == Some(f.product_code)
                    && s.is_none_or(|s| s.eq_ignore_ascii_case(f.cip.rsplit(':').next().unwrap_or("")));
            }
            e.rsplit_once(':')
                .map_or(e, |(h, p)| if p.parse::<u16>().is_ok() { h } else { e })
                == f.address
        })
        .map(|d| d.name.clone())
}

impl NewDevice {
    /// The form for a device found on the network: EtherNet/IP, named by
    /// its identity (`cip=V:P:S`, so a lease change never loses it), the
    /// name and profile from the library's match when there is one.
    #[must_use]
    pub fn from_found(f: &Found) -> Self {
        let model = f
            .library
            .as_ref()
            .map(|l| l.model.as_str())
            .filter(|m| !m.is_empty())
            .unwrap_or(f.product_name.as_str());
        let name = Some(slug(model))
            .filter(|s| iomap::device_name_ok(s))
            .unwrap_or_else(|| format!("device_{}_{}", f.vendor_id, f.product_code));
        let params = match f.library.as_ref().and_then(|l| l.profile.as_deref()) {
            Some(p) => format!("profile={p} rpi_ms=100 poll_ms=20"),
            None => "rpi_ms=100 poll_ms=20".to_string(),
        };
        Self {
            name,
            protocol: 1,
            endpoint: f.cip.clone(),
            params,
            error: None,
            eds: None,
        }
    }
}

/// The scan's state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scan {
    #[default]
    Idle,
    Running,
    Done,
    Failed(String),
}

/// The found devices as text, for the agent: one a line.
#[must_use]
pub fn describe_found(found: &[Found]) -> String {
    let mut out = String::new();
    if found.is_empty() {
        return out;
    }
    out.push_str("On the network (ListIdentity):\n");
    for f in found {
        let _ = write!(
            out,
            "  {} {} vendor {} product {} rev {} serial {}",
            f.product_name, f.address, f.vendor_id, f.product_code, f.revision, f.serial
        );
        match &f.mapped {
            Some(n) => {
                let _ = write!(out, "  in the map as {n}");
            }
            None => out.push_str("  not in the map"),
        }
        if let Some(l) = &f.library {
            let _ = write!(out, "  library: {} {} ({})", l.vendor, l.model, l.folder);
        }
        out.push('\n');
    }
    out
}

/// The tab's state: which devices are folded, the form if one is open,
/// an import asked for, the last scan.
#[derive(Debug, Default)]
pub struct DevicesUi {
    folded: BTreeSet<String>,
    pub adding: Option<NewDevice>,
    /// "Import device…" was pressed: the view opens the file picker.
    pub import: bool,
    pub found: Vec<Found>,
    pub scan: Scan,
    /// Why the box had no library, when it said.
    pub library_note: Option<String>,
    /// The plant model's states (`crate::plant`): the view fills it in
    /// each frame, the section draws under the network.
    pub plant: crate::plant::PlantUi,
}

impl DevicesUi {
    /// Takes in the box's answer to `POST /api/devices/discover`.
    pub fn discovered(&mut self, status: u16, body: &str) {
        let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        match (status, Found::parse_discovery(&j)) {
            (200..=299, Some((found, note))) => {
                self.found = found;
                self.library_note = note;
                self.scan = Scan::Done;
            }
            (409, _) => self.scan = Scan::Failed("a scan is already running; try again shortly".into()),
            (501, _) => self.scan = Scan::Failed("this box does not scan its network".into()),
            (s, _) => {
                let why = j["error"].as_str().unwrap_or("no answer").to_string();
                self.scan = Scan::Failed(if s == 0 { why } else { format!("{s}: {why}") });
            }
        }
    }

    /// The simulation's scan: its one simulated E1213.
    pub fn simulated_scan(&mut self, map: &MapText) {
        self.found = vec![Found::simulated_e1213(map)];
        self.scan = Scan::Done;
    }

    /// Fills in the library's matches from `index` for the found devices
    /// that lack one (the simulation fetches the index itself, in a
    /// browser; natively the tests call this).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn attach_library(&mut self, index: &Index) {
        for f in &mut self.found {
            if f.library.is_none()
                && let Some(e) = index.lookup(f.vendor_id, f.product_code, &f.product_name)
            {
                f.library = Some(LibraryHit::of(e));
            }
        }
        self.library_note = None;
    }
}

fn menu_item(ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, enabled: bool) -> bool {
    let b = egui::Button::new(text).min_size(Vec2::new(110.0, TAP * 0.6));
    let r = ui.add_enabled(enabled, b);
    chrome.hit(key, r.rect);
    if r.clicked() {
        ui.close();
        return true;
    }
    false
}

/// One row of the tree, cut at the pane's edge so a long address never
/// widens it (CLAUDE.md: a resizable egui panel grows to any row that
/// overflows it).
fn row(ui: &mut egui::Ui, text: RichText, indent: f32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        ui.add(egui::Label::new(text).truncate().sense(Sense::click()))
    })
    .inner
}

impl DevicesUi {
    /// The tree, then the blank space whose right-click adds a device.
    /// `write`: why a device cannot be added, if it cannot. `source`: a
    /// line over the tree saying which map this is.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        nodes: &[DeviceNode],
        source: &str,
        chrome: &mut Chrome,
        write: &Result<(), String>,
    ) -> Option<DeviceAct> {
        let t = theme(ui);
        let mut act = None;
        ui.set_max_width(ui.available_width());
        if !source.is_empty() {
            ui.add(egui::Label::new(RichText::new(source).small().color(t.muted)).truncate());
        }
        if nodes.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new("No devices in the I/O map yet. Right-click here to add one.")
                        .color(t.muted),
                )
                .wrap(),
            );
        }
        let mut flip = None;
        for n in nodes {
            let folded = self.folded.contains(&n.name);
            let dot = match &n.state {
                LinkState::Up(_) => RichText::new("●").color(t.accent),
                LinkState::Down(_) => RichText::new("●").color(RED),
                LinkState::Unknown => RichText::new("○").color(t.muted),
                LinkState::Simulated => RichText::new("◌").color(t.muted),
            };
            let arrow = if folded { "▸" } else { "▾" };
            // An identity endpoint reads with where it was found.
            let found = match (
                &n.state,
                n.endpoint.starts_with("mac=") || n.endpoint.starts_with("cip="),
            ) {
                (LinkState::Up(Some(a)), true) => format!(" → {a}"),
                _ => String::new(),
            };
            let head = format!("{arrow} {}  {} {}{found}", n.name, n.driver, n.endpoint);
            let r = ui.horizontal(|ui| {
                ui.label(dot);
                ui.add(
                    egui::Label::new(RichText::new(head).monospace().strong())
                        .truncate()
                        .sense(Sense::click()),
                )
            });
            let r = r.inner;
            chrome.hit(format!("device {}", n.name), r.rect);
            let params: Vec<String> = n.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let hover = format!(
                "{}\n{}\n{} point(s)",
                state_text(&n.state),
                if params.is_empty() {
                    "no settings".to_string()
                } else {
                    params.join(" ")
                },
                n.points.len()
            );
            if r.on_hover_text(hover).clicked() {
                flip = Some(n.name.clone());
            }
            if folded {
                continue;
            }
            for p in &n.points {
                let value = p.value.map_or(String::new(), |v| format!(" = {}", show(Some(v))));
                let text = format!("{:<3} {}  {}{value}", p.dir, p.signal, p.point);
                let color = match (&p.value, p.quality.as_deref()) {
                    _ if p.forced => Color32::from_rgb(255, 196, 60),
                    (_, Some("missing")) => RED,
                    (Some(Value::Bool(true)), _) => t.accent,
                    _ => t.fg,
                };
                let r = row(ui, RichText::new(text).monospace().color(color), 22.0);
                chrome.hit(format!("point {}", p.signal), r.rect);
                let mut hover = format!("{} {}", p.ty, p.point);
                if let Some(u) = &p.unit {
                    let _ = write!(hover, " in {u}");
                }
                if let Some(q) = &p.quality {
                    let _ = write!(hover, "\n{q}");
                }
                if p.forced {
                    hover.push_str("\nforced by an engineer");
                }
                r.on_hover_text(hover);
            }
        }
        if let Some(name) = flip
            && !self.folded.remove(&name)
        {
            self.folded.insert(name);
        }
        // On the network: a scan, then every device that answered.
        ui.add_space(6.0);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("On the network").strong().color(t.muted));
            let busy = self.scan == Scan::Running;
            let b = ui.add_enabled(!busy, egui::Button::new(if busy { "Scanning…" } else { "Scan" }));
            chrome.hit("devices scan", b.rect);
            if b.on_hover_text("One ListIdentity on the LAN: every EtherNet/IP device answers")
                .clicked()
            {
                act = Some(DeviceAct::Scan);
            }
        });
        match &self.scan {
            Scan::Idle => {
                ui.add(
                    egui::Label::new(
                        RichText::new("Scan to list the devices that answer.")
                            .small()
                            .color(t.muted),
                    )
                    .wrap(),
                );
            }
            Scan::Done if self.found.is_empty() => {
                ui.add(
                    egui::Label::new(
                        RichText::new("No EtherNet/IP device answered.")
                            .small()
                            .color(t.muted),
                    )
                    .wrap(),
                );
            }
            Scan::Running | Scan::Done => {}
            Scan::Failed(why) => {
                ui.add(egui::Label::new(RichText::new(why).small().color(RED)).wrap());
            }
        }
        let mut add = None;
        for f in &self.found {
            let dot = if f.mapped.is_some() {
                RichText::new("●").color(t.accent)
            } else {
                RichText::new("○").color(t.muted)
            };
            let where_ = match &f.mapped {
                Some(n) => format!("in the map as {n}"),
                None => "not in the map".to_string(),
            };
            let text = format!(
                "{}  {}  {}:{}  {where_}",
                f.product_name, f.address, f.vendor_id, f.product_code
            );
            let r = ui.horizontal(|ui| {
                ui.label(dot);
                ui.add(
                    egui::Label::new(RichText::new(text).monospace())
                        .truncate()
                        .sense(Sense::click()),
                )
            });
            let r = r.inner;
            chrome.hit(format!("network {}", f.address), r.rect);
            let mut hover = format!("revision {}  serial {}", f.revision, f.serial);
            if let Some(m) = &f.mac {
                let _ = write!(hover, "\nMAC {m}");
            }
            match &f.library {
                Some(l) => {
                    let _ = write!(hover, "\nlibrary: {} {} ({})", l.vendor, l.model, l.folder);
                }
                None => hover.push_str("\nnot in the device library"),
            }
            if let Some(n) = &self.library_note {
                let _ = write!(hover, "\n{n}");
            }
            let can_add = write.is_ok() && f.mapped.is_none();
            r.on_hover_text(hover).context_menu(|ui| {
                if menu_item(ui, chrome, "menu add network", "Add to the map…", can_add) {
                    add = Some(NewDevice::from_found(f));
                }
                if let Some(n) = &f.mapped {
                    ui.label(
                        RichText::new(format!("already in the map as {n}"))
                            .small()
                            .color(t.muted),
                    );
                } else if let Err(why) = write {
                    ui.label(RichText::new(why).small().color(t.muted));
                }
            });
        }
        if add.is_some() {
            self.adding = add;
        }
        // The plant model's states, each with its sensor or the one it wants.
        self.plant.show(ui, chrome, write);
        // The blank space under it all: a right-click offers a new device.
        let left = (ui.clip_rect().bottom() - ui.cursor().top()).max(TAP);
        let size = Vec2::new(ui.available_width(), left);
        let r = ui.allocate_response(size, Sense::click());
        chrome.hit("devices blank", r.rect);
        r.context_menu(|ui| {
            if menu_item(ui, chrome, "menu new device", "New device…", write.is_ok()) {
                self.adding = Some(NewDevice::default());
            }
            if menu_item(ui, chrome, "menu import device", "Import device…", write.is_ok()) {
                self.import = true;
            }
            if let Err(why) = write {
                ui.label(RichText::new(why).small().color(t.muted));
            }
        });
        act
    }

    /// Takes the pending "Import device…" request, once.
    pub fn take_import(&mut self) -> bool {
        std::mem::take(&mut self.import)
    }

    /// The "New device" form as a box in the middle of the screen,
    /// whichever tab is up (the view draws it each frame after the
    /// Editor, as the Files tab's confirmations are). Returns the line to
    /// add once Add is pressed with fields that pass.
    pub fn form_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        let mut d = self.adding.clone()?;
        let mut out = None;
        let mut done = false;
        let m = egui::Modal::new(egui::Id::unique("device-new")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let title = match &d.eds {
                Some(f) => format!("New device from {f}"),
                None => "New device".to_string(),
            };
            ui.label(RichText::new(title).strong().size(16.0));
            ui.add_space(8.0);
            ui.label("Name (what the program's I/O map calls it)");
            let n = ui.add(
                egui::TextEdit::singleline(&mut d.name)
                    .hint_text("rio")
                    .desired_width(f32::INFINITY),
            );
            chrome.hit("device name", n.rect);
            ui.add_space(6.0);
            ui.label("Protocol");
            ui.horizontal_wrapped(|ui| {
                for (k, (driver, title, _, defaults)) in PROTOCOLS.iter().enumerate() {
                    let r = ui.add(egui::Button::selectable(d.protocol == k, *title));
                    chrome.hit(format!("device {driver}"), r.rect);
                    if r.clicked() && d.protocol != k {
                        d.protocol = k;
                        d.params = (*defaults).to_string();
                    }
                }
            });
            ui.add_space(6.0);
            ui.label("Address");
            let hint = PROTOCOLS.get(d.protocol).map_or("", |p| p.2);
            let e = ui.add(
                egui::TextEdit::singleline(&mut d.endpoint)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
            chrome.hit("device endpoint", e.rect);
            ui.add_space(6.0);
            ui.label("Settings (KEY=VALUE, the driver's)");
            let p = ui.add(egui::TextEdit::singleline(&mut d.params).desired_width(f32::INFINITY));
            chrome.hit("device params", p.rect);
            if let Some(err) = &d.error {
                ui.add_space(4.0);
                ui.label(RichText::new(err).color(RED));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let ok = ui.add(egui::Button::new(RichText::new("Add").strong()).min_size(tall));
                chrome.hit("device ok", ok.rect);
                let no = ui.add(egui::Button::new("Cancel").min_size(tall));
                chrome.hit("device cancel", no.rect);
                if ok.clicked() {
                    match d.line() {
                        Ok(line) => {
                            out = Some(DeviceAct::Add(line));
                            done = true;
                        }
                        Err(e) => d.error = Some(e),
                    }
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("device form", m.response.rect);
        if done || m.should_close() {
            self.adding = None;
        } else {
            self.adding = Some(d);
        }
        out
    }
}

/// `line` appended to a map's text: on a line of its own, after a trailing
/// newline the text may lack.
#[must_use]
pub fn appended(map: &str, line: &str) -> String {
    let mut out = map.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDS: &str = "$ a test EDS\r\n[File]\r\n    DescText = \"x\";\r\n[Device]\r\n    VendCode = 991;   $ Moxa\r\n    ProdName = \"E1213\";\r\n[Params]\r\n";

    #[test]
    fn an_eds_fills_the_form_from_its_product_name() {
        assert_eq!(eds_product(EDS).as_deref(), Some("E1213"));
        assert_eq!(
            eds_product("[Device]\nProdName = \"ioLogik E1213-T\";"),
            Some("ioLogik E1213-T".into())
        );
        assert_eq!(eds_product("[Params]\nProdName = \"not here\";"), None);
        assert_eq!(eds_product(""), None);
        assert_eq!(
            eds_product(&"ProdName = \"x\";\n".repeat(10_000)),
            None,
            "never in a section"
        );
        assert_eq!(eds_product("[Device]\nProdName = \"\";"), None);
        let d = NewDevice::from_eds("moxa.eds", EDS);
        assert_eq!(d.name, "e1213");
        assert_eq!(PROTOCOLS[d.protocol].0, "enip-io");
        assert_eq!(d.params, "eds=moxa.eds rpi_ms=100 poll_ms=20");
        assert_eq!(d.eds.as_deref(), Some("moxa.eds"));
        let mut d = d;
        d.endpoint = "10.0.0.57".into();
        assert_eq!(
            d.line().unwrap(),
            "device e1213 enip-io 10.0.0.57 eds=moxa.eds rpi_ms=100 poll_ms=20\n"
        );
        // No product name: the file's stem names the device.
        let d = NewDevice::from_eds("my-block.eds", "[File]\n");
        assert_eq!(d.name, "my_block");
        // A hostile product name becomes a plain id.
        let d = NewDevice::from_eds("x.eds", "[Device]\nProdName = \"../../etc; rm -rf\";");
        assert_eq!(d.name, "etc_rm_rf");
        assert_eq!(slug("1213"), "d1213");
        assert_eq!(slug(""), "");
    }

    fn map() -> MapText {
        iomap::skim(
            "device e1213 modbus-tcp 10.0.0.57:502 unit=1\n\
             in  di0 bool e1213 di:0\n\
             out do0 bool e1213 coil:0 safe=false\n\
             out step int screen step\n\
             in  rate real module conveyor unit=m/s\n\
             out lamp bool box do:0\n\
             in  ghost bool nowhere di:9\n",
        )
    }

    #[test]
    fn the_tree_groups_points_under_declared_then_implied_devices() {
        let live = |name: &str| match name {
            "di0" => Some(Live {
                value: Some(Value::Bool(true)),
                quality: Some("good".into()),
                forced: false,
            }),
            "lamp" => Some(Live {
                value: Some(Value::Bool(false)),
                quality: None,
                forced: true,
            }),
            _ => None,
        };
        let state = |name: &str| match name {
            "e1213" => LinkState::Up(None),
            "box" => LinkState::Unknown,
            _ => LinkState::Simulated,
        };
        let nodes = tree(&map(), &live, &state);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["e1213", "screen", "module", "box", "nowhere"]);
        assert_eq!(nodes[0].points.len(), 2);
        assert_eq!(nodes[0].points[0].value, Some(Value::Bool(true)));
        assert_eq!(nodes[0].state, LinkState::Up(None));
        assert_eq!(nodes[4].driver, "not declared in the map");
        assert!(nodes[3].points[0].forced);
        let text = describe(&nodes);
        assert!(
            text.starts_with("e1213  modbus-tcp 10.0.0.57:502 unit=1  [up]\n  in  di0 bool di:0 = TRUE\n"),
            "{text}"
        );
        assert!(text.contains("out lamp bool do:0 = FALSE FORCED\n"), "{text}");
        assert!(
            text.contains("module  other modules' outputs  [simulated]\n  in  rate real conveyor unit=m/s\n"),
            "{text}"
        );
    }

    /// `device-by-identity`: a device named by its MAC or CIP identity
    /// lists with the address the box found it at.
    #[test]
    fn an_identity_endpoint_reads_with_where_it_was_found() {
        let m = iomap::skim("device rio enip-io mac=00-90-e8-7e-af-be rpi_ms=100\nin di0 bool rio di:0\n");
        let nodes = tree(&m, &|_| None, &|_| LinkState::Up(Some("10.0.0.57:44818".into())));
        assert_eq!(nodes[0].endpoint, "mac=00-90-e8-7e-af-be");
        assert_eq!(
            describe(&nodes),
            "rio  enip-io mac=00-90-e8-7e-af-be rpi_ms=100  [up at 10.0.0.57:44818]\n  in  di0 bool di:0\n"
        );
    }

    #[test]
    fn an_empty_map_is_an_empty_tree() {
        assert_eq!(tree(&iomap::skim(""), &|_| None, &|_| LinkState::Unknown), []);
        assert_eq!(describe(&[]), "");
    }

    #[test]
    fn the_form_writes_one_device_line_and_refuses_bad_fields() {
        let mut d = NewDevice::default();
        assert_eq!(d.params, PROTOCOLS[0].3);
        assert!(d.line().is_err(), "no name yet");
        d.name = "rio".into();
        d.endpoint = "10.0.0.57:502".into();
        assert_eq!(
            d.line().unwrap(),
            "device rio modbus-tcp 10.0.0.57:502 unit=1 timeout_ms=300 poll_ms=20\n"
        );
        d.protocol = 1;
        d.params = String::new();
        assert_eq!(d.line().unwrap(), "device rio enip-io 10.0.0.57:502\n");
        d.protocol = 99;
        assert!(d.line().is_err(), "no such protocol");
    }

    #[test]
    fn appending_keeps_one_device_a_line() {
        assert_eq!(appended("", "device a b c\n"), "device a b c\n");
        assert_eq!(
            appended("in x bool box di:0", "device a b c\n"),
            "in x bool box di:0\ndevice a b c\n"
        );
        assert_eq!(
            appended("in x bool box di:0\n", "device a b c\n"),
            "in x bool box di:0\ndevice a b c\n"
        );
    }
}
