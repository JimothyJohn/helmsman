//! A device's own configuration on the Devices tab (Nick, 2026-11-13): for a
//! device whose profile has `configure moxa-e1200`, the box reads and sets
//! the device's own web console (`/api/devices/{id}/config...`,
//! docs/ARCHITECTURE.md "Device configuration"). This is the section under
//! the device's row: the facts in plain words first (what the outputs do if
//! this box stops, what they do at power-up, the firmware), how the unit
//! differs from the project's saved configuration, then one Actions control
//! for an engineer. Every call to the box is built in [`ConfigUi::request`]
//! and every answer taken in [`ConfigUi::answer`], so how a restart is
//! waited for is one place.

use std::collections::{BTreeMap, VecDeque};

use eframe::egui::{self, Color32, RichText, Vec2};
use serde_json::{Value as Json, json};

use crate::box_projects::{PCall, PReq};
use crate::editor::Chrome;
use crate::remote::segment;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

const AMBER: Color32 = Color32::from_rgb(255, 196, 60);
const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// The pages whose settings the facts name, and their fields (the
/// console's own names; the box refuses any it does not know).
const GENERAL: &str = "04_1.htm";
const SAFE_MODE: &str = "11_safeM.htm";
const RESTFUL: &str = "13_1.htm";
const ENIP: &str = "14_1.htm";

/// What a call to the configuration routes is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigOp {
    Read,
    Set,
    Restart,
    Import,
    SetPassword,
    ClearPassword,
    ChangePassword,
    Snapshot,
}

/// One digital output's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutFact {
    pub name: String,
    /// Its place on the unit, for the settings page.
    pub index: usize,
    /// What it goes to when the watchdog trips.
    pub safe: Option<bool>,
    /// What it is at power-up.
    pub power_on: Option<bool>,
}

/// What the unit says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    pub firmware: String,
    pub watchdog: Option<bool>,
    pub watchdog_time: Option<u32>,
    pub outputs: Vec<OutFact>,
    pub ethernet_ip: Option<bool>,
    pub restful: Option<bool>,
}

/// The unit against the project's saved configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub count: usize,
    pub lines: Vec<String>,
}

/// What `GET .../config` answered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub facts: Facts,
    /// `None`: the project has no saved configuration of this device.
    pub drift: Option<Drift>,
    pub password_stored: bool,
}

fn pick<'a>(j: &'a Json, keys: &[&str]) -> &'a Json {
    keys.iter()
        .map(|k| &j[*k])
        .find(|v| !v.is_null())
        .unwrap_or(&Json::Null)
}

/// `true`, `1`, `"on"`, `"enabled"` and their opposites.
fn flag(j: &Json) -> Option<bool> {
    match j {
        Json::Bool(b) => Some(*b),
        Json::Number(n) => n.as_u64().map(|n| n != 0),
        Json::String(s) => match s.to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "enabled" | "yes" => Some(true),
            "off" | "false" | "0" | "disabled" | "no" => Some(false),
            _ => None,
        },
        Json::Object(_) => flag(pick(j, &["enabled", "on", "value"])),
        _ => None,
    }
}

/// Text from the box, cut to printable characters and a length.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

impl Config {
    /// Reads the answer of `GET .../config`.
    #[must_use]
    pub fn parse(j: &Json) -> Option<Self> {
        if !j.is_object() {
            return None;
        }
        let f = pick(j, &["facts"]);
        let wd = pick(f, &["watchdog", "communication_watchdog"]);
        let outputs = pick(f, &["outputs", "dos"])
            .as_array()
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, o)| OutFact {
                        name: clean(o["name"].as_str().unwrap_or(""), 24),
                        index: o["index"].as_u64().map_or(i, |n| usize::try_from(n).unwrap_or(i)),
                        safe: flag(pick(o, &["safe", "safe_value", "safe_mode"])),
                        power_on: flag(pick(o, &["power_on", "power_on_state", "poweron"])),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let facts = Facts {
            firmware: clean(f["firmware"].as_str().unwrap_or(""), 32),
            watchdog: flag(wd),
            watchdog_time: pick(wd, &["seconds", "time"])
                .as_u64()
                .or_else(|| pick(f, &["watchdog_time", "watchdog_seconds"]).as_u64())
                .and_then(|n| u32::try_from(n).ok()),
            outputs,
            ethernet_ip: flag(pick(f, &["ethernet_ip", "enip", "ethernetip"])),
            restful: flag(pick(f, &["restful", "rest"])),
        };
        let d = pick(j, &["drift"]);
        let drift = match d {
            Json::Array(_) | Json::Object(_) => {
                let list = if d.is_array() {
                    d
                } else {
                    pick(d, &["lines", "changed", "differences"])
                };
                let lines: Vec<String> = list
                    .as_array()
                    .map(|a| a.iter().filter_map(Json::as_str).map(|s| clean(s, 120)).collect())
                    .unwrap_or_default();
                let count = pick(d, &["count", "differs"])
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(lines.len());
                Some(Drift { count, lines })
            }
            _ => None,
        };
        let stored = pick(j, &["password_stored", "password"]);
        let password_stored = flag(pick(stored, &["stored"]))
            .or_else(|| flag(stored))
            .unwrap_or(false);
        Some(Self {
            facts,
            drift,
            password_stored,
        })
    }
}

/// The names as a sentence part: `A`, `A and B`, `A, B and C`.
fn names(v: &[&str]) -> String {
    match v {
        [] => String::new(),
        [a] => (*a).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The facts as the sentences a controls engineer reads.
#[must_use]
pub fn sentences(f: &Facts) -> Vec<String> {
    let mut out = Vec::new();
    match (f.watchdog, f.watchdog_time) {
        (Some(false), _) => {
            out.push("Outputs hold their state if this box stops: the device's watchdog is off.".to_string())
        }
        (Some(true), t) => {
            let time = t.map_or(String::new(), |t| format!(" (time {t})"));
            out.push(format!(
                "Outputs fall to their safe values if this box stops: the device's watchdog is on{time}."
            ));
        }
        (None, _) => {}
    }
    let with = |pick: fn(&OutFact) -> Option<bool>, want: bool| -> Vec<&str> {
        f.outputs
            .iter()
            .filter(|o| pick(o) == Some(want))
            .map(|o| o.name.as_str())
            .collect()
    };
    if f.outputs.iter().any(|o| o.safe.is_some()) {
        let on = with(|o| o.safe, true);
        out.push(if on.is_empty() {
            "When the watchdog acts, every output switches off.".to_string()
        } else {
            let verb = if on.len() == 1 { "switches" } else { "switch" };
            format!(
                "When the watchdog acts, {} {verb} on and the others switch off.",
                names(&on)
            )
        });
    }
    if f.outputs.iter().any(|o| o.power_on.is_some()) {
        let on = with(|o| o.power_on, true);
        out.push(if on.is_empty() {
            "Every output is off at power-up.".to_string()
        } else {
            let verb = if on.len() == 1 { "switches" } else { "switch" };
            format!("{} {verb} on at power-up.", names(&on))
        });
    }
    if let Some(e) = f.ethernet_ip {
        out.push(format!(
            "EtherNet/IP is {} on the device.",
            if e { "on" } else { "off" }
        ));
    }
    if let Some(r) = f.restful {
        out.push(format!(
            "The device's RESTful interface is {}.",
            if r { "on" } else { "off" }
        ));
    }
    out
}

/// The settings the person edits, a copy of the facts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Draft {
    watchdog: bool,
    time: u32,
    safe: Vec<bool>,
    ethernet_ip: bool,
    restful: bool,
}

impl Draft {
    fn of(f: &Facts) -> Self {
        Self {
            watchdog: f.watchdog.unwrap_or(false),
            time: f.watchdog_time.unwrap_or(0),
            safe: f.outputs.iter().map(|o| o.safe.unwrap_or(false)).collect(),
            ethernet_ip: f.ethernet_ip.unwrap_or(false),
            restful: f.restful.unwrap_or(false),
        }
    }
}

type Change = (String, Vec<(String, String)>);

fn one(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}

/// The `set` calls that take the unit from `f` to `d`: one per page, only
/// what differs.
fn changes(f: &Facts, d: &Draft) -> Vec<Change> {
    let mut out: Vec<Change> = Vec::new();
    let was = Draft::of(f);
    if (d.watchdog, d.time) != (was.watchdog, was.time) && (f.watchdog.is_some() || d.watchdog) {
        let mut c = vec![("EnableWD".to_string(), one(d.watchdog))];
        if d.watchdog {
            c.push(("WDTime".to_string(), d.time.to_string()));
        }
        out.push((GENERAL.into(), c));
    }
    let safe: Vec<(String, String)> = f
        .outputs
        .iter()
        .zip(&d.safe)
        .zip(&was.safe)
        .filter(|((_, now), before)| now != before)
        .map(|((o, now), _)| (format!("SetSM{:02}", o.index), one(*now)))
        .collect();
    if !safe.is_empty() {
        out.push((SAFE_MODE.into(), safe));
    }
    if d.restful != was.restful {
        out.push((RESTFUL.into(), vec![("REST_ENABLE".into(), one(d.restful))]));
    }
    if d.ethernet_ip != was.ethernet_ip {
        out.push((ENIP.into(), vec![("EIP_ENABLE".into(), one(d.ethernet_ip))]));
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Load {
    #[default]
    Idle,
    Asking,
    Got(Box<Config>),
    /// The device has no configuration (the box answered 404).
    None,
    Failed(String),
}

#[derive(Debug, Default)]
struct Dev {
    load: Load,
    /// Settings were staged since the last restart.
    staged: bool,
    /// What is under way, in words.
    busy: Option<&'static str>,
    /// The last result: the words, and whether it is an error.
    notice: Option<(String, bool)>,
    drift_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Settings,
    Restart,
    Restore,
    Password,
    Change,
}

/// The one question on screen.
#[derive(Debug)]
struct Modal {
    device: String,
    kind: Kind,
    draft: Draft,
    /// A password being typed. Never filled from the box.
    text: String,
    /// The box said these modules would be interrupted (409): the second ask.
    modules: Option<String>,
}

/// The tab's configuration state, for every device.
#[derive(Default)]
pub struct ConfigUi {
    /// Who may write: `Ok` for an engineer, else why not.
    engineer: Option<String>,
    can_write: bool,
    /// The devices worth asking the box about: by their profile.
    asks: BTreeMap<String, bool>,
    devs: BTreeMap<String, Dev>,
    modal: Option<Modal>,
    /// The settings still to stage, one page at a time.
    sets: VecDeque<(String, Vec<Change>)>,
    queue: Vec<PReq>,
}

impl std::fmt::Debug for ConfigUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigUi")
            .field("devices", &self.devs.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Whether a profile's text names a configurator.
#[must_use]
pub fn profile_configures(text: &str) -> bool {
    text.lines()
        .map(|l| {
            l.split('#')
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
        })
        .any(|w| matches!(w.as_slice(), ["configure", _, ..]))
}

fn url(device: &str, tail: &str) -> String {
    format!("/api/devices/{}/config{tail}", segment(device))
}

/// The words for a refusal: the box's own, never a bare status.
fn why(status: u16, body: &str) -> String {
    let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
    if let Some(e) = j["error"].as_str().filter(|e| !e.is_empty()) {
        return clean(e, 400);
    }
    match status {
        0 => {
            let b = clean(body, 200);
            if b.is_empty() {
                "The box did not answer.".into()
            } else {
                b
            }
        }
        401 | 403 => "Sign in as an engineer to change a device's configuration.".into(),
        404 => "The box has no configuration for this device.".into(),
        _ => "The box refused and did not say why.".into(),
    }
}

impl ConfigUi {
    /// The view's word each frame: who may write, and which devices are
    /// worth asking about (a device by its profile; `true`: the profile
    /// is loaded and names a configurator).
    pub fn set_context(&mut self, engineer: Result<(), String>, asks: BTreeMap<String, bool>) {
        self.can_write = engineer.is_ok();
        self.engineer = engineer.err();
        self.asks = asks;
    }

    /// The calls made since the last frame.
    pub fn take_requests(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    /// Whether `device` shows a configuration section.
    #[must_use]
    pub fn has(&self, device: &str) -> bool {
        self.asks.contains_key(device) && !matches!(self.devs.get(device).map(|d| &d.load), Some(Load::None))
    }

    /// Every call to the box is made here.
    fn request(device: &str, op: ConfigOp, body: &Json) -> PReq {
        let call = PCall::Config(device.to_string(), op);
        match op {
            ConfigOp::Read => PReq::get(call, url(device, "")),
            ConfigOp::Set => PReq::json(call, "POST", url(device, "/set"), body),
            ConfigOp::Restart => PReq::json(call, "POST", url(device, "/restart"), body),
            ConfigOp::Import => PReq::json(call, "POST", url(device, "/import"), body),
            ConfigOp::SetPassword => PReq::json(call, "PUT", url(device, "/password"), body),
            ConfigOp::ClearPassword => PReq::json(call, "DELETE", url(device, "/password"), body),
            ConfigOp::ChangePassword => PReq::json(call, "POST", url(device, "/password"), body),
            ConfigOp::Snapshot => PReq::json(call, "POST", url(device, "/snapshot"), body),
        }
    }

    fn send(&mut self, device: &str, op: ConfigOp, body: &Json) {
        let busy = match op {
            ConfigOp::Read => None,
            ConfigOp::Set => Some("Staging the change on the device…"),
            ConfigOp::Restart | ConfigOp::Import => Some("The device is restarting, offline for about 21 s…"),
            ConfigOp::Snapshot => Some("Saving the configuration into the project…"),
            ConfigOp::SetPassword | ConfigOp::ClearPassword | ConfigOp::ChangePassword => {
                Some("Updating the password…")
            }
        };
        let d = self.devs.entry(device.to_string()).or_default();
        if op == ConfigOp::Read {
            d.load = Load::Asking;
        } else {
            d.busy = busy;
            d.notice = None;
        }
        self.queue.push(Self::request(device, op, body));
    }

    /// Takes in the box's answer to a configuration call.
    pub fn answer(&mut self, device: &str, op: ConfigOp, status: u16, body: &str) {
        let ok = (200..300).contains(&status);
        let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        let d = self.devs.entry(device.to_string()).or_default();
        if op == ConfigOp::Read {
            d.load = match (status, Config::parse(&j)) {
                (200..=299, Some(c)) => Load::Got(Box::new(c)),
                (404, _) => Load::None,
                _ => Load::Failed(why(status, body)),
            };
            return;
        }
        d.busy = None;
        // A restart or restore that would interrupt running modules: the
        // box names them and waits for a second ask.
        if status == 409 && matches!(op, ConfigOp::Restart | ConfigOp::Import) {
            let named = match j["modules"].as_array() {
                Some(m) if !m.is_empty() => m
                    .iter()
                    .filter_map(Json::as_str)
                    .map(|s| clean(s, 48))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => why(status, body),
            };
            let kind = if op == ConfigOp::Restart {
                Kind::Restart
            } else {
                Kind::Restore
            };
            self.modal = Some(Modal {
                device: device.to_string(),
                kind,
                draft: Draft::default(),
                text: String::new(),
                modules: Some(named),
            });
            return;
        }
        if !ok {
            d.notice = Some((why(status, body), true));
            self.sets.clear();
            return;
        }
        let done = match op {
            ConfigOp::Set => {
                d.staged = true;
                None
            }
            ConfigOp::Restart | ConfigOp::Import => {
                d.staged = false;
                Some(if op == ConfigOp::Restart {
                    "Applied. The device restarted with the staged settings."
                } else {
                    "Restored. The device restarted; its IP address stayed as it was."
                })
            }
            ConfigOp::Snapshot => Some("Saved the device's configuration into the project."),
            ConfigOp::SetPassword => Some("Stored the password for the box to sign in with."),
            ConfigOp::ClearPassword => Some("Removed the stored password."),
            ConfigOp::ChangePassword => Some("Changed the console's password."),
            ConfigOp::Read => None,
        };
        if let Some(s) = done {
            d.notice = Some((s.into(), false));
        }
        if op == ConfigOp::Set
            && let Some((dev, mut rest)) = self.sets.pop_front()
        {
            if rest.is_empty() {
                self.queue_read(&dev);
            } else {
                let (page, changes) = rest.remove(0);
                self.sets.push_front((dev.clone(), rest));
                self.send(
                    &dev,
                    ConfigOp::Set,
                    &json!({"page": page, "changes": changes_json(&changes)}),
                );
            }
            return;
        }
        self.queue_read(device);
    }

    fn queue_read(&mut self, device: &str) {
        self.send(device, ConfigOp::Read, &Json::Null);
    }

    /// The section under a device's row, if it has one. Asks the box for
    /// the configuration the first time.
    pub fn section(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, device: &str) {
        if !self.has(device) {
            return;
        }
        let first = !self.devs.contains_key(device) || self.devs[device].load == Load::Idle;
        if first {
            self.queue_read(device);
        }
        let t = theme(ui);
        let can = self.can_write;
        let why_not = self.engineer.clone().unwrap_or_default();
        let (load, staged, busy, notice, drift_open) = {
            let d = &self.devs[device];
            (d.load.clone(), d.staged, d.busy, d.notice.clone(), d.drift_open)
        };
        let mut act: Option<Kind> = None;
        let mut snapshot = false;
        let mut again = false;
        let mut flip = false;
        ui.horizontal_wrapped(|ui| {
            ui.add_space(22.0);
            ui.label(RichText::new("Configuration").strong().color(t.muted))
                .tip("The device's own settings, read from the device itself. Changing them takes an engineer's sign-in.");
            if let Load::Got(c) = &load {
                if !c.facts.firmware.is_empty() {
                    ui.label(RichText::new(format!("firmware {}", c.facts.firmware)).color(t.muted))
                        .tip("The firmware the device reports.");
                }
                let on = can && busy.is_none();
                let tip = if can { "Save a snapshot, change a setting, apply, restore, or the console password." } else { why_not.as_str() };
                let r = ui.menu_button("Actions", |ui| {
                    let item = |ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, tip: &str| {
                        let b = ui.add_enabled(on, egui::Button::new(text).min_size(Vec2::new(150.0, TAP * 0.6))).tip(tip);
                        chrome.hit(key, b.rect);
                        let c = b.clicked();
                        if c {
                            ui.close();
                        }
                        c
                    };
                    if item(ui, chrome, "config item snapshot", "Save a snapshot", "Saves the device's configuration into the project, to compare with and restore from.") {
                        snapshot = true;
                    }
                    if item(ui, chrome, "config item settings", "Change a setting…", "The watchdog, each output's safe value, EtherNet/IP and the RESTful interface. Staged on the device until you apply.") {
                        act = Some(Kind::Settings);
                    }
                    if item(ui, chrome, "config item apply", "Apply (restart)…", "Restarts the device so staged settings take effect. It is offline for about 21 seconds.") {
                        act = Some(Kind::Restart);
                    }
                    if item(ui, chrome, "config item restore", "Restore from snapshot…", "Puts the project's saved configuration back on the device. The IP address stays as it is.") {
                        act = Some(Kind::Restore);
                    }
                    if item(ui, chrome, "config item password", "Stored console password…", "The password the box signs in to the device's console with. It is write-only.") {
                        act = Some(Kind::Password);
                    }
                    if item(ui, chrome, "config item change", "Change the console's password…", "Sets a new password on the device's console.") {
                        act = Some(Kind::Change);
                    }
                });
                chrome.hit(format!("config actions {device}"), r.response.rect);
                let rect = r.response.rect;
                r.response.tip(tip);
                if !can {
                    chrome.hit(format!("config locked {device}"), rect);
                }
            }
        });
        let line = |ui: &mut egui::Ui, text: String, color: Color32| {
            ui.horizontal(|ui| {
                ui.add_space(22.0);
                ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
            });
        };
        match &load {
            Load::Idle | Load::Asking | Load::None => {
                line(ui, "Reading the device's configuration…".into(), t.muted);
            }
            Load::Failed(why) => {
                line(ui, why.clone(), t.ink(RED));
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let b = ui
                        .link("Read again")
                        .tip("Asks the box to read the device again.");
                    chrome.hit(format!("config again {device}"), b.rect);
                    again = b.clicked();
                });
            }
            Load::Got(c) => {
                for s in sentences(&c.facts) {
                    line(ui, s, t.fg);
                }
                if staged {
                    line(
                        ui,
                        "Staged, not applied yet: the device keeps its running settings until you apply."
                            .into(),
                        AMBER,
                    );
                    chrome.hit(format!("config staged {device}"), ui.min_rect());
                }
                match &c.drift {
                    None => line(
                        ui,
                        "The project has no saved configuration of this device yet.".into(),
                        t.muted,
                    ),
                    Some(d) if d.count == 0 => line(
                        ui,
                        "The device matches the project's saved configuration.".into(),
                        t.muted,
                    ),
                    Some(d) => {
                        let word = if d.count == 1 {
                            "setting differs"
                        } else {
                            "settings differ"
                        };
                        line(
                            ui,
                            format!("{} {word} from the project's saved configuration", d.count),
                            AMBER,
                        );
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            let b = ui
                                .link(if drift_open {
                                    "Hide the lines"
                                } else {
                                    "Show the lines"
                                })
                                .tip("The configuration lines that differ.");
                            chrome.hit(format!("config drift {device}"), b.rect);
                            flip = b.clicked();
                        });
                        if drift_open {
                            for l in d.lines.iter().take(40) {
                                ui.horizontal(|ui| {
                                    ui.add_space(30.0);
                                    ui.add(
                                        egui::Label::new(RichText::new(l).monospace().color(t.muted)).wrap(),
                                    );
                                });
                            }
                        }
                    }
                }
                if c.password_stored {
                    line(ui, "A console password is stored for the box.".into(), t.muted);
                }
            }
        }
        if let Some(b) = busy {
            line(ui, b.into(), AMBER);
        }
        if let Some((n, bad)) = notice {
            line(ui, n, if bad { t.ink(RED) } else { t.accent });
        }
        if flip {
            self.devs
                .entry(device.into())
                .and_modify(|d| d.drift_open = !d.drift_open);
        }
        if again {
            self.queue_read(device);
        }
        if snapshot {
            self.send(device, ConfigOp::Snapshot, &json!({}));
        }
        if let (Some(kind), Load::Got(c)) = (act, &load) {
            self.modal = Some(Modal {
                device: device.into(),
                kind,
                draft: Draft::of(&c.facts),
                text: String::new(),
                modules: None,
            });
        }
    }

    /// The question on screen, if any.
    pub fn modal_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(m) = &mut self.modal else { return };
        let facts = match self.devs.get(&m.device).map(|d| &d.load) {
            Some(Load::Got(c)) => c.facts.clone(),
            _ => Facts::default(),
        };
        let device = m.device.clone();
        let kind = m.kind;
        let mut go: Option<(ConfigOp, Json)> = None;
        let mut sets: Option<Vec<Change>> = None;
        let mut close = false;
        let r = crate::theme::near_pointer(ctx, "device-config").show(ctx, |ui| {
            ui.set_max_width(460.0);
            let title = match kind {
                Kind::Settings => format!("Change a setting of {device}"),
                Kind::Restart => format!("Apply: restart {device}?"),
                Kind::Restore => format!("Restore {device} from the snapshot?"),
                Kind::Password => "Stored console password".to_string(),
                Kind::Change => "Change the console's password".to_string(),
            };
            ui.label(RichText::new(title).strong().size(20.0));
            ui.add_space(8.0);
            let say = |ui: &mut egui::Ui, s: &str| {
                ui.add(egui::Label::new(s).wrap());
            };
            let tall = Vec2::new(0.0, TAP * 0.7);
            let button = |ui: &mut egui::Ui, chrome: &mut Chrome, key: &str, text: &str, tip: &str| {
                let b = ui.add(egui::Button::new(text).min_size(tall)).tip(tip);
                chrome.hit(key, b.rect);
                b.clicked()
            };
            match kind {
                Kind::Settings => {
                    let d = &mut m.draft;
                    ui.checkbox(&mut d.watchdog, "Watchdog on")
                        .tip("On: the outputs go to their safe values when the device stops hearing from this box. Off: they hold their state.");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Watchdog time").tip("How long the device waits for this box before it acts, as the device counts it.");
                        let r = ui.add_enabled(d.watchdog, egui::DragValue::new(&mut d.time).range(0..=65535))
                            .tip("How long the device waits for this box before it acts, as the device counts it.");
                        chrome.hit("config watchdog time", r.rect);
                    });
                    for (o, s) in facts.outputs.iter().zip(&mut d.safe) {
                        let r = ui
                            .checkbox(s, format!("{} switches on when the watchdog acts", o.name))
                            .tip("Ticked: this output goes on when the watchdog acts. Unticked: it goes off.");
                        chrome.hit(format!("config safe {}", o.name), r.rect);
                    }
                    ui.checkbox(&mut d.ethernet_ip, "EtherNet/IP on")
                        .tip("Whether the device answers EtherNet/IP. This box reads it that way when it can.");
                    ui.checkbox(&mut d.restful, "RESTful interface on")
                        .tip("Whether the device answers its RESTful interface.");
                    ui.add_space(8.0);
                    say(ui, "The change is staged on the device. It takes effect when you apply, which restarts the device.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let todo = changes(&facts, d);
                        let b = ui.add_enabled(!todo.is_empty(), egui::Button::new("Stage").min_size(tall))
                            .tip("Stages the change on the device. Nothing runs differently until you apply.");
                        chrome.hit("config stage", b.rect);
                        if b.clicked() {
                            sets = Some(todo);
                            close = true;
                        }
                        if button(ui, chrome, "config cancel", "Cancel", "Leaves the device as it is.") {
                            close = true;
                        }
                    });
                }
                Kind::Restart | Kind::Restore => {
                    let restore = kind == Kind::Restore;
                    if let Some(mods) = &m.modules {
                        say(ui, &format!("The running project uses this device. Restarting it interrupts: {mods}."));
                        say(ui, "Restart anyway?");
                    } else if restore {
                        say(ui, "The device is offline for about 21 seconds while it restarts. Its IP address stays as it is; every other setting comes from the project's snapshot.");
                    } else {
                        say(ui, "The device is offline for about 21 seconds while it restarts. Staged settings take effect.");
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let interrupts = m.modules.is_some();
                        let label = if restore { "Restore" } else { "Restart" };
                        if button(ui, chrome, "config confirm", if interrupts { "Restart anyway" } else { label }, "Goes ahead. The device is offline for about 21 seconds.") {
                            go = Some(if restore {
                                (ConfigOp::Import, json!({"from": "snapshot", "overwrite_network": false, "interrupts": interrupts}))
                            } else if interrupts {
                                (ConfigOp::Restart, json!({"interrupts": true}))
                            } else {
                                (ConfigOp::Restart, json!({}))
                            });
                            close = true;
                        }
                        if button(ui, chrome, "config cancel", "Cancel", "Leaves the device running.") {
                            close = true;
                        }
                    });
                }
                Kind::Password | Kind::Change => {
                    let change = kind == Kind::Change;
                    let stored = matches!(self.devs.get(&device).map(|d| &d.load), Some(Load::Got(c)) if c.password_stored);
                    say(ui, if change {
                        "Sets a new password on the device's console. The box signs in with the password it has stored to do this."
                    } else if stored {
                        "A password is stored. It cannot be shown. Type a new one to replace it, or remove it."
                    } else {
                        "No password is stored. If the device's console asks for one, store it here."
                    });
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut m.text).password(true).desired_width(300.0).hint_text(if change { "new password" } else { "password" }))
                        .tip("Up to 16 characters. It is sent to the box and never shown again.");
                    chrome.hit("config password field", r.rect);
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        let empty = m.text.is_empty();
                        let b = ui.add_enabled(!empty, egui::Button::new(if change { "Change" } else { "Store" }).min_size(tall))
                            .tip(if change { "Changes the console's password now." } else { "Stores this password for the box to sign in with." });
                        chrome.hit("config password go", b.rect);
                        if b.clicked() {
                            go = Some(if change {
                                (ConfigOp::ChangePassword, json!({"new": m.text}))
                            } else {
                                (ConfigOp::SetPassword, json!({"password": m.text}))
                            });
                            close = true;
                        }
                        if !change {
                            let rm = ui.add_enabled(stored, egui::Button::new("Remove").min_size(tall))
                                .tip("Removes the stored password.");
                            chrome.hit("config password remove", rm.rect);
                            if rm.clicked() {
                                go = Some((ConfigOp::ClearPassword, Json::Null));
                                close = true;
                            }
                        }
                        if button(ui, chrome, "config cancel", "Cancel", "Closes this without changing anything.") {
                            close = true;
                        }
                    });
                }
            }
        });
        chrome.hit("config form", r.response.rect);
        if r.should_close() {
            close = true;
        }
        if close {
            // The typed password goes with the form.
            self.modal = None;
        }
        if let Some((op, body)) = go {
            self.send(&device, op, &body);
        }
        if let Some(todo) = sets
            && !todo.is_empty()
        {
            let mut it = todo;
            let (page, ch) = it.remove(0);
            self.sets = VecDeque::from([(device.clone(), it)]);
            self.send(
                &device,
                ConfigOp::Set,
                &json!({"page": page, "changes": changes_json(&ch)}),
            );
        }
    }
}

fn changes_json(c: &[(String, String)]) -> Json {
    Json::Object(
        c.iter()
            .map(|(k, v)| (k.clone(), Json::String(v.clone())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Json {
        json!({
            "facts": {
                "firmware": "V3.1",
                "watchdog": {"enabled": false, "seconds": 0},
                "outputs": [
                    {"name": "DO-00", "safe": false, "power_on": true},
                    {"name": "DO-01", "safe": true, "power_on": true},
                    {"name": "DO-02", "safe": false, "power_on": false}
                ],
                "ethernet_ip": false,
                "restful": true
            },
            "drift": {"count": 3, "lines": ["a=1", "b=2", "c=3"]},
            "password_stored": true
        })
    }

    #[test]
    fn the_facts_read_in_plain_words() {
        let c = Config::parse(&sample()).unwrap();
        let s = sentences(&c.facts);
        assert!(s.contains(
            &"Outputs hold their state if this box stops: the device's watchdog is off.".to_string()
        ));
        assert!(s.contains(&"DO-00 and DO-01 switch on at power-up.".to_string()));
        assert!(s.contains(&"EtherNet/IP is off on the device.".to_string()));
        assert_eq!(c.drift.as_ref().unwrap().count, 3);
        assert!(c.password_stored);
    }

    #[test]
    fn a_hostile_body_reads_as_nothing_or_clean_text() {
        assert!(Config::parse(&json!([1, 2])).is_none());
        let c =
            Config::parse(&json!({"facts": {"firmware": "V\u{7}3", "outputs": [{"name": "x\ny"}]}})).unwrap();
        assert_eq!(c.facts.firmware, "V3");
        assert_eq!(c.facts.outputs[0].name, "xy");
    }

    #[test]
    fn changes_name_only_what_differs_and_one_page_each() {
        let f = Config::parse(&sample()).unwrap().facts;
        let mut d = Draft::of(&f);
        assert!(changes(&f, &d).is_empty());
        d.watchdog = true;
        d.time = 5;
        d.safe[2] = true;
        d.ethernet_ip = true;
        let c = changes(&f, &d);
        let pages: Vec<&str> = c.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(pages, [GENERAL, SAFE_MODE, ENIP]);
        assert_eq!(c[1].1, [("SetSM02".to_string(), "1".to_string())]);
    }

    #[test]
    fn a_profile_names_a_configurator() {
        assert!(profile_configures("helmsman-device 1\nconfigure moxa-e1200\n"));
        assert!(!profile_configures(
            "helmsman-device 1\n# configure moxa-e1200\nkind x\n"
        ));
        assert!(!profile_configures("helmsman-device 1\nconfigure\n"));
    }

    #[test]
    fn a_refusal_uses_the_boxes_words() {
        assert_eq!(
            why(409, r#"{"error":"the project drives it"}"#),
            "the project drives it"
        );
        assert!(!why(500, "").contains("500"));
        assert!(!why(0, "").is_empty());
    }
}
