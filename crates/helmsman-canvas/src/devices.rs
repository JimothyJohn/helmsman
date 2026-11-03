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

use std::collections::BTreeSet;
use std::fmt::Write as _;

use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use helmsman_iec::Value;
use helmsman_project::iomap::{self, MapText};

use crate::editor::Chrome;
use crate::st_view::{TAP, show};
use crate::theme::theme;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// How the box is doing with a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    Up,
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
        LinkState::Up => "up".into(),
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
}

/// The tab's state: which devices are folded, the form if one is open,
/// an import asked for.
#[derive(Debug, Default)]
pub struct DevicesUi {
    folded: BTreeSet<String>,
    pub adding: Option<NewDevice>,
    /// "Import device…" was pressed: the view opens the file picker.
    pub import: bool,
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
    ) {
        let t = theme(ui);
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
                LinkState::Up => RichText::new("●").color(t.accent),
                LinkState::Down(_) => RichText::new("●").color(RED),
                LinkState::Unknown => RichText::new("○").color(t.muted),
                LinkState::Simulated => RichText::new("◌").color(t.muted),
            };
            let arrow = if folded { "▸" } else { "▾" };
            let head = format!("{arrow} {}  {} {}", n.name, n.driver, n.endpoint);
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
            "e1213" => LinkState::Up,
            "box" => LinkState::Unknown,
            _ => LinkState::Simulated,
        };
        let nodes = tree(&map(), &live, &state);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["e1213", "screen", "module", "box", "nowhere"]);
        assert_eq!(nodes[0].points.len(), 2);
        assert_eq!(nodes[0].points[0].value, Some(Value::Bool(true)));
        assert_eq!(nodes[0].state, LinkState::Up);
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
