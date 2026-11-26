//! The wiring walkthrough of one device (Nick, 2026-11-25: "I want the
//! agent, on the device level, to be able to help the user run through
//! the proper wiring of their system and confirm the connections needed
//! to wire in the inputs they selected").
//!
//! **Wire it** on a device's row in the I/O tab opens a window listing
//! the device's inputs the open program uses, in channel order, one step
//! each: the signal, the terminal the device labels it with and, when the
//! map line names a sensor with `wired=`, the sensor's connector with each
//! wire's pin, colour and function. Above them, the device's own wiring
//! note.
//!
//! Every fact on the page comes from the device library and from nothing
//! else: where the library has none the window says so and points to the
//! manual. No model is asked, and the walkthrough never writes, forces or
//! energises anything: it reads the values the page already has.
//!
//! On a box page the window also confirms: the first step not done waits
//! for its signal to change and ticks with what was seen; another input
//! of the same device changing meanwhile is named against the step and
//! does not tick it (a sensor on the wrong terminal). In the simulation
//! there is no hardware, so the steps are a checklist the user ticks.
//! The state lives in the page until it reloads.

use std::collections::BTreeMap;

use eframe::egui::{self, RichText, Vec2};
use helmsman_iec::Value;
use helmsman_project::devlib::Index;
use helmsman_project::iomap::MapDevice;
use serde_json::Value as Json;

use crate::devices::{DeviceNode, DevicesUi, LinkState, Wired};
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

const RED: egui::Color32 = egui::Color32::from_rgb(255, 110, 100);
/// The window's width.
const WIDTH: f32 = 460.0;
/// The smallest change of a real number that counts as the input moving
/// (below it, a reading is noise).
pub const REAL_NOISE: f64 = 0.1;
/// Most steps one window lists.
const MAX_STEPS: usize = 256;
/// Most inputs named against one step as having changed instead.
const MAX_WRONG: usize = 8;
/// Most changes kept of what was seen on a step.
const MAX_SEEN: usize = 4;
/// Longest note or source kept, and longest sensor text.
const NOTE: usize = 600;
const TEXT: usize = 160;
/// Most connectors, order numbers and wires of a sensor shown.
const MAX_CONNECTORS: usize = 8;
const MAX_PRODUCTS: usize = 16;
const MAX_PINS: usize = 9;

/// What the library says about wiring the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub note: String,
    pub source: Option<String>,
}

/// How a device is looked up in the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// The EtherNet/IP vendor id and product code.
    Ident(u16, u16),
    /// The vendor and model a profile names.
    Text(String, String),
}

impl Key {
    /// The library route's query for it, asking for the wiring note.
    #[must_use]
    pub fn query(&self) -> String {
        match self {
            Self::Ident(v, p) => format!("vendor={v}&product={p}&wiring=1"),
            Self::Text(v, m) => format!("q={}&wiring=1", crate::remote::segment(&format!("{v} {m}"))),
        }
    }
}

/// Where the lookup of a device's note stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Facts {
    Asked(Key),
    /// The library answered: its note, or none.
    Known(Option<Note>),
    /// Why there is nothing to show, in words.
    Unavailable(String),
}

/// One wire of a sensor's connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub n: u8,
    pub function: Option<String>,
    pub color: Option<String>,
}

/// One connector of a sensor, as its record gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    pub kind: String,
    pub products: Vec<String>,
    pub pins: Vec<Pin>,
}

fn clean(v: &Json, most: usize) -> Option<String> {
    let s: String = v
        .as_str()?
        .chars()
        .filter(|c| !c.is_control())
        .take(most)
        .collect();
    (!s.is_empty()).then_some(s)
}

impl Connector {
    /// The connectors of a record's brief (`connections`), bounded.
    #[must_use]
    pub fn of_brief(j: &Json) -> Vec<Self> {
        j["connections"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(MAX_CONNECTORS)
                    .filter_map(|c| {
                        let pins: Vec<Pin> = c["wires"]
                            .as_array()?
                            .iter()
                            .take(MAX_PINS)
                            .filter_map(|w| {
                                Some(Pin {
                                    n: u8::try_from(w["n"].as_u64()?)
                                        .ok()
                                        .filter(|n| (1..=9).contains(n))?,
                                    function: clean(&w["function"], TEXT),
                                    color: clean(&w["color"], TEXT),
                                })
                            })
                            .collect();
                        (!pins.is_empty()).then(|| Self {
                            kind: clean(&c["type"], TEXT).unwrap_or_else(|| "Connector".into()),
                            products: c["products"]
                                .as_array()
                                .map(|p| {
                                    p.iter()
                                        .take(MAX_PRODUCTS)
                                        .filter_map(|p| clean(p, TEXT))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            pins,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `Cable (KQ6001, KQ6002): pin 1 BN L+, pin 3 BU L-`.
    #[must_use]
    pub fn line(&self) -> String {
        let pins: Vec<String> = self
            .pins
            .iter()
            .map(|p| {
                let mut s = format!("pin {}", p.n);
                for part in [&p.color, &p.function].into_iter().flatten() {
                    s.push(' ');
                    s.push_str(part);
                }
                s
            })
            .collect();
        let products = match self.products.as_slice() {
            [] => String::new(),
            [a, b, c, rest @ ..] if !rest.is_empty() => format!(" ({a}, {b}, {c} and {} more)", rest.len()),
            all => format!(" ({})", all.join(", ")),
        };
        format!("{}{products}: {}", self.kind, pins.join(", "))
    }
}

/// One step: an input of the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub signal: String,
    /// The terminal as the device labels it (`DI-00`), else its point.
    pub channel: String,
    /// The sensor on it (`wired=`).
    pub wired: Option<String>,
}

/// How a step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Done {
    /// Its signal changed on the live box.
    Seen,
    Hand,
    Skipped,
}

/// What the window can know of the device's inputs this frame.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    /// No hardware: a checklist.
    Simulated,
    Live,
    /// The device does not answer.
    Down,
    /// The box has not said.
    Unknown,
}

/// One device's progress.
#[derive(Debug, Clone, Default, PartialEq)]
struct Walk {
    done: BTreeMap<String, Done>,
    /// What each step saw, in order (`went on`, `off`).
    seen: BTreeMap<String, Vec<String>>,
    /// Inputs that changed while the step waited.
    wrong: BTreeMap<String, Vec<String>>,
    /// The last value of each input, to see a change against.
    last: BTreeMap<String, Value>,
}

/// What one step is drawn against.
#[derive(Clone, Copy)]
struct StepCtx<'a> {
    walk: &'a Walk,
    mode: &'a Mode,
    values: &'a BTreeMap<String, Option<Value>>,
    /// The step the window waits on.
    active: bool,
}

/// The walkthrough's state, in the page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WireIt {
    /// The device whose window is open.
    pub open: Option<String>,
    walks: BTreeMap<String, Walk>,
    /// What the library said of each device that was asked.
    pub facts: BTreeMap<String, Facts>,
    /// The connectors of each `wired=` value's record.
    pub sensors: BTreeMap<String, Vec<Connector>>,
    /// The open device's inputs this frame: the mode, each input's value
    /// and its channel's name.
    now: Option<(Mode, BTreeMap<String, Option<Value>>)>,
    fed: bool,
}

/// What an input's change reads as: `went on`, `went off`, `changed to 4.2`.
fn change(old: Value, new: Value) -> Option<String> {
    match (old, new) {
        (Value::Bool(a), Value::Bool(b)) if a != b => Some(format!("went {}", if b { "on" } else { "off" })),
        (Value::Int(a), Value::Int(b)) if a != b => Some(format!("changed to {b}")),
        (Value::Real(a), Value::Real(b)) if (a - b).abs() >= REAL_NOISE => Some(format!("changed to {b}")),
        _ => None,
    }
}

fn shown(v: Option<Value>) -> String {
    match v {
        Some(Value::Bool(b)) => if b { "on" } else { "off" }.into(),
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Real(r)) => format!("{r}"),
        None => "no value yet".into(),
    }
}

/// The seen list as words: `went on, then off`.
fn seen_text(list: &[String]) -> String {
    let mut out = String::new();
    for (k, s) in list.iter().enumerate() {
        if k > 0 {
            out.push_str(", then ");
            out.push_str(s.strip_prefix("went ").unwrap_or(s));
        } else {
            out.push_str(s);
        }
    }
    out
}

impl WireIt {
    /// The first step of `steps` still waiting.
    fn active<'a>(&self, device: &str, steps: &'a [Step]) -> Option<&'a Step> {
        let w = self.walks.get(device);
        steps
            .iter()
            .find(|s| w.is_none_or(|w| !w.done.contains_key(&s.signal)))
    }

    /// Takes in one frame of the device's inputs (`inputs`: signal, value,
    /// name for the report) against its `steps`.
    fn observe(
        &mut self,
        device: &str,
        steps: &[Step],
        inputs: &[(String, Option<Value>)],
        channels: &BTreeMap<String, String>,
    ) {
        let active = self.active(device, steps).map(|s| s.signal.clone());
        let walk = self.walks.entry(device.to_string()).or_default();
        let mut changes: Vec<(String, String)> = Vec::new();
        for (signal, value) in inputs {
            let Some(now) = *value else {
                walk.last.remove(signal);
                continue;
            };
            if let Some(old) = walk.last.insert(signal.clone(), now)
                && let Some(c) = change(old, now)
            {
                changes.push((signal.clone(), c));
            }
        }
        for (signal, c) in changes {
            match walk.done.get(&signal) {
                Some(Done::Seen) => {
                    let list = walk.seen.entry(signal).or_default();
                    if list.len() < MAX_SEEN {
                        list.push(c);
                    }
                }
                _ if active.as_deref() == Some(signal.as_str()) => {
                    walk.done.insert(signal.clone(), Done::Seen);
                    walk.seen.insert(signal, vec![c]);
                }
                _ => {
                    if let Some(a) = &active {
                        let at = channels
                            .get(&signal)
                            .map_or(String::new(), |c| format!(" It is on {c}."));
                        let note = format!("{signal} changed instead: wired to the wrong terminal?{at}");
                        let list = walk.wrong.entry(a.clone()).or_default();
                        if list.len() < MAX_WRONG && !list.contains(&note) {
                            list.push(note);
                        }
                    }
                }
            }
        }
    }
}

/// A point address split for ordering: its text and the number it ends
/// in, so `di:2` comes before `di:10`.
fn natural(point: &str) -> (String, u64) {
    let head = point.trim_end_matches(|c: char| c.is_ascii_digit());
    let n = point[head.len()..].parse().unwrap_or(0);
    (head.to_ascii_lowercase(), n)
}

/// The library key of a device the map declares, from what the map and
/// the project's profiles say of it; `None` when nothing identifies it.
fn key_of(dev: &MapDevice, profile: Option<&str>) -> Option<Key> {
    let cip = dev.endpoint.strip_prefix("cip=").or_else(|| {
        dev.params
            .iter()
            .find(|(k, _)| k == "cip")
            .map(|(_, v)| v.as_str())
    });
    if let Some(c) = cip {
        let mut it = c.split(':').map(|x| x.parse::<u16>().ok());
        if let (Some(Some(v)), Some(Some(p))) = (it.next(), it.next()) {
            return Some(Key::Ident(v, p));
        }
    }
    if dev
        .params
        .iter()
        .any(|(k, v)| k == "profile" && v == "moxa-e1213")
    {
        return Some(Key::Ident(991, 515));
    }
    let skim = helmsman_project::profile_skim::skim(profile?).ok()?;
    (!skim.vendor.trim().is_empty() && !skim.model.trim().is_empty())
        .then_some(Key::Text(skim.vendor, skim.model))
}

fn note_of(m: &Json) -> Option<Note> {
    Some(Note {
        note: clean(&m["wiring"]["note"], NOTE)?,
        source: clean(&m["wiring"]["source"], NOTE),
    })
}

impl DevicesUi {
    /// Opens the walkthrough on `device`.
    pub fn open_wire_it(&mut self, device: &str) {
        self.wire.open = Some(device.to_string());
    }

    /// The inputs of `device` the program uses, in the order the device
    /// numbers its channels (an input the device does not list comes
    /// after, in the map's order).
    #[must_use]
    pub fn wire_steps(&self, device: &str) -> Vec<Step> {
        let channels = self.channels_of(device).unwrap_or_default();
        let at = |point: &str| channels.iter().position(|c| c.point.eq_ignore_ascii_case(point));
        let mut steps: Vec<((usize, String, u64), Step)> = self
            .map
            .signals
            .iter()
            .filter(|s| {
                s.device == device
                    && s.dir == "in"
                    && self.program.iter().any(|p| p.name.eq_ignore_ascii_case(&s.name))
            })
            .take(MAX_STEPS)
            .map(|s| {
                let k = at(&s.point);
                (
                    (k.unwrap_or(usize::MAX), natural(&s.point).0, natural(&s.point).1),
                    Step {
                        signal: s.name.clone(),
                        channel: k.map_or_else(|| s.point.clone(), |k| channels[k].label.clone()),
                        wired: s.wired().map(str::to_string),
                    },
                )
            })
            .collect();
        steps.sort_by(|a, b| a.0.cmp(&b.0));
        steps.into_iter().map(|(_, s)| s).collect()
    }

    /// Takes in this frame's tree: the open device's inputs and its link.
    /// Called each frame by [`Self::show`].
    pub(crate) fn wire_feed(&mut self, nodes: &[DeviceNode]) {
        let Some(device) = self.wire.open.clone() else {
            self.wire.now = None;
            return;
        };
        let Some(node) = nodes.iter().find(|n| n.name == device) else {
            self.wire.now = None;
            return;
        };
        let mode = match &node.state {
            LinkState::Simulated => Mode::Simulated,
            LinkState::Up { .. } => Mode::Live,
            LinkState::Down(_) => Mode::Down,
            LinkState::Unknown => Mode::Unknown,
        };
        let inputs: Vec<(String, Option<Value>)> = node
            .points
            .iter()
            .filter(|p| p.dir == "in")
            .map(|p| {
                let good = p.quality.as_deref().is_none_or(|q| q == "good");
                (p.signal.clone(), p.value.filter(|_| good))
            })
            .collect();
        let steps = self.wire_steps(&device);
        let inputs: Vec<(String, Option<Value>)> = inputs
            .into_iter()
            .filter(|(s, _)| steps.iter().any(|st| &st.signal == s))
            .collect();
        if mode == Mode::Live {
            let channels: BTreeMap<String, String> = {
                let all = self.channels_of(&device).unwrap_or_default();
                node.points
                    .iter()
                    .filter_map(|p| {
                        let c = all.iter().find(|c| c.point.eq_ignore_ascii_case(&p.point))?;
                        Some((p.signal.clone(), c.label.clone()))
                    })
                    .collect()
            };
            self.wire.observe(&device, &steps, &inputs, &channels);
        } else if let Some(w) = self.wire.walks.get_mut(&device) {
            // Nothing watched: the next live frame is a new baseline.
            w.last.clear();
        }
        self.wire.now = Some((mode, inputs.into_iter().collect()));
        self.wire.fed = true;
    }

    /// The library lookup the open device needs, as its device name and
    /// the library route's query: once. Without anything that identifies
    /// the device there is nothing to ask, and the window says so.
    pub fn wiring_wanted(&mut self) -> Option<(String, String)> {
        let device = self.wire.open.clone()?;
        if self.wire.facts.contains_key(&device) {
            return None;
        }
        let dev = self.map.devices.iter().find(|d| d.name == device)?;
        let profile = dev
            .driver
            .strip_prefix("device:")
            .and_then(|stem| {
                self.profiles
                    .get(&format!("{stem}{}", helmsman_project::DEVICE_EXT))
            })
            .map(|(t, _)| t.as_str());
        let Some(key) = key_of(dev, profile) else {
            self.wire.facts.insert(device, Facts::Known(None));
            return None;
        };
        let query = key.query();
        self.wire.facts.insert(device.clone(), Facts::Asked(key));
        Some((device, query))
    }

    /// Takes in the box's answer to `GET /api/devices/library?...&wiring=1`
    /// for `device`.
    pub fn wiring_answer(&mut self, device: &str, status: u16, body: &str) {
        let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        let Some(Facts::Asked(key)) = self.wire.facts.get(device).cloned() else {
            return;
        };
        let got = match status {
            200..=299 => {
                let pick = j["matches"].as_array().and_then(|a| {
                    a.iter().find(|m| match &key {
                        Key::Ident(..) => true,
                        Key::Text(v, md) => {
                            m["vendor"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(v))
                                && m["model"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(md))
                        }
                    })
                });
                match pick {
                    Some(m) => match (note_of(m), clean(&m["wiring_unavailable"], TEXT)) {
                        (Some(n), _) => Facts::Known(Some(n)),
                        (None, Some(why)) => Facts::Unavailable(why),
                        (None, None) => Facts::Known(None),
                    },
                    None => clean(&j["unavailable"], TEXT).map_or(Facts::Known(None), Facts::Unavailable),
                }
            }
            404 => Facts::Unavailable("this box does not look up the device library".into()),
            s => Facts::Unavailable(format!("the box could not read the library ({s})")),
        };
        self.wire.facts.insert(device.to_string(), got);
    }

    /// The simulation's lookup: the library's index, read in the page,
    /// answers for the open device when it carries a note.
    pub fn wiring_from_index(&mut self, index: &Index) {
        let Some(Facts::Asked(key)) = self
            .wire
            .open
            .as_ref()
            .and_then(|d| self.wire.facts.get(d))
            .cloned()
        else {
            return;
        };
        let entry = match &key {
            Key::Ident(v, p) => index.by_identity(*v, *p).into_iter().next(),
            Key::Text(v, m) => index
                .search(&format!("{v} {m}"))
                .into_iter()
                .find(|e| e.vendor.eq_ignore_ascii_case(v) && e.model.eq_ignore_ascii_case(m)),
        };
        let note = entry.and_then(|e| e.wiring.as_ref()).map(|w| Note {
            note: w.note.clone(),
            source: w.source.clone(),
        });
        if let Some(d) = self.wire.open.clone() {
            self.wire.facts.insert(d, Facts::Known(note));
        }
    }

    /// The window. Nothing in it sends anything: it reads what the page
    /// already has.
    pub fn wire_it_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(device) = self.wire.open.clone() else {
            return;
        };
        if !std::mem::take(&mut self.wire.fed) {
            return;
        }
        let steps = self.wire_steps(&device);
        let walk = self.wire.walks.get(&device).cloned().unwrap_or_default();
        let (mode, values) = self.wire.now.clone().unwrap_or((Mode::Unknown, BTreeMap::new()));
        let facts = self.wire.facts.get(&device).cloned();
        let active = self.wire.active(&device, &steps).map(|s| s.signal.clone());
        let mut hand: Vec<(String, Option<Done>)> = Vec::new();
        let mut close = false;
        let m = crate::theme::near_pointer(ctx, "device-wire-it").show(ctx, |ui| {
            let t = theme(ui);
            ui.set_min_width(WIDTH);
            ui.set_max_width(WIDTH);
            ui.label(RichText::new(format!("Wire {device}")).strong().size(20.0));
            ui.add_space(4.0);
            let line = match mode {
                Mode::Simulated => "Simulation: no hardware here. Tick each step when you have wired it.",
                Mode::Live => "Switch each input when its step is waiting. The box is watched and the step ticks when it sees the change.",
                Mode::Down => "The device is not answering, so there is nothing to watch yet. Tick the steps by hand, or wait for it to come up.",
                Mode::Unknown => "The box has not said whether the device is up. Tick the steps by hand, or wait.",
            };
            ui.add(egui::Label::new(RichText::new(line).color(t.muted)).wrap());
            ui.add_space(6.0);
            let rows_h = (ctx.content_rect().height() * 0.5).clamp(140.0, 420.0);
            egui::ScrollArea::vertical()
                .id_salt("wire-steps")
                .max_height(rows_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_max_width(WIDTH - 16.0);
                match &facts {
                    Some(Facts::Known(Some(n))) => {
                        ui.label(RichText::new("Wiring note").strong()).tip("What the device library says about wiring this device.");
                        ui.add(egui::Label::new(n.note.as_str()).wrap());
                        if let Some(s) = &n.source {
                            ui.add(egui::Label::new(RichText::new(format!("Source: {s}")).color(t.muted)).wrap());
                        }
                    }
                    Some(Facts::Unavailable(why)) => {
                        ui.add(egui::Label::new(format!(
                            "The device library could not be read ({why}). Check the device's manual."
                        )).wrap());
                    }
                    Some(Facts::Asked(_)) => {
                        ui.label(RichText::new("Looking in the device library…").color(t.muted));
                    }
                    Some(Facts::Known(None)) | None => {
                        ui.add(egui::Label::new(
                            "The device library has no wiring facts for this device. Check the device's manual.",
                        ).wrap());
                    }
                }
                ui.add_space(8.0);
                if steps.is_empty() {
                    ui.add(egui::Label::new(RichText::new(
                        "The open program uses no input of this device, so there is nothing to wire.",
                    ).color(t.muted)).wrap());
                }
                    for (k, s) in steps.iter().enumerate() {
                        let r = ui.vertical(|ui| {
                            let ctx = StepCtx { walk: &walk, mode: &mode, values: &values, active: active.as_deref() == Some(s.signal.as_str()) };
                            self.wire_step(ui, chrome, (k + 1, s), &ctx, &mut hand);
                        });
                        chrome.hit(format!("wire step {}", s.signal), r.response.rect);
                        ui.add_space(6.0);
                    }
                });
            ui.add_space(8.0);
            let ok = ui
                .add(egui::Button::new(RichText::new("Done").strong()).min_size(Vec2::new(0.0, TAP * 0.7)))
                .tip("Closes the list. What you ticked stays until the page is reloaded.");
            chrome.hit("wire done", ok.rect);
            close = ok.clicked();
        });
        chrome.hit("wire form", m.response.rect);
        let walk = self.wire.walks.entry(device.clone()).or_default();
        for (signal, how) in hand {
            if let Some(d) = how {
                walk.done.insert(signal, d);
            } else {
                walk.done.remove(&signal);
                walk.seen.remove(&signal);
                walk.wrong.remove(&signal);
            }
        }
        if close || crate::theme::escaped(&m) {
            walk.last.clear();
            self.wire.open = None;
        }
    }

    fn wire_step(
        &self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        (n, s): (usize, &Step),
        c: &StepCtx,
        hand: &mut Vec<(String, Option<Done>)>,
    ) {
        let StepCtx {
            walk,
            mode,
            values,
            active,
        } = *c;
        let t = theme(ui);
        ui.add(
            egui::Label::new(
                RichText::new(format!("{n}. {}   {}", s.channel, s.signal))
                    .strong()
                    .monospace(),
            )
            .wrap(),
        );
        if let Some(w) = &s.wired {
            let facts = match self.wired.get(w) {
                Some(Wired::Known(b)) => {
                    ui.add(egui::Label::new(RichText::new(b.title()).color(t.muted)).wrap());
                    self.wire.sensors.get(w).filter(|c| !c.is_empty())
                }
                Some(Wired::Asked) => {
                    ui.label(RichText::new("Looking in the device library…").color(t.muted));
                    None
                }
                _ => None,
            };
            match facts {
                Some(list) => {
                    for c in list {
                        ui.add(egui::Label::new(c.line()).wrap());
                    }
                }
                None if matches!(self.wired.get(w), Some(Wired::Asked)) => {}
                None => {
                    ui.add(
                        egui::Label::new(
                            "The library has no wiring facts for this sensor. Check its manual.",
                        )
                        .wrap(),
                    );
                }
            }
        }
        let status = match (walk.done.get(&s.signal), mode) {
            (Some(Done::Seen), _) => format!(
                "Seen: {}",
                seen_text(walk.seen.get(&s.signal).map_or(&[][..], Vec::as_slice))
            ),
            (Some(Done::Hand), _) => "Ticked by hand".to_string(),
            (Some(Done::Skipped), _) => "Skipped".to_string(),
            (None, Mode::Live) if active => format!(
                "Waiting for it to change. Now: {}",
                shown(values.get(&s.signal).copied().flatten())
            ),
            (None, Mode::Live) => "Not yet".to_string(),
            (None, _) => "Not ticked yet".to_string(),
        };
        let done = walk.done.contains_key(&s.signal);
        let color = if done { t.accent } else { t.muted };
        ui.add(egui::Label::new(RichText::new(status).color(color)).wrap());
        for w in walk.wrong.get(&s.signal).into_iter().flatten() {
            ui.add(egui::Label::new(RichText::new(w).color(theme(ui).ink(RED))).wrap());
        }
        ui.horizontal_wrapped(|ui| {
            if done {
                let b = ui
                    .add(egui::Button::new("Reset").small())
                    .tip("Takes the tick off, to check this step again.");
                chrome.hit(format!("wire reset {}", s.signal), b.rect);
                if b.clicked() {
                    hand.push((s.signal.clone(), None));
                }
            } else {
                let tick = ui
                    .add(egui::Button::new("Tick").small())
                    .tip("Ticks this step by hand: you have wired it and checked it another way.");
                chrome.hit(format!("wire tick {}", s.signal), tick.rect);
                if tick.clicked() {
                    hand.push((s.signal.clone(), Some(Done::Hand)));
                }
                let skip = ui
                    .add(egui::Button::new("Skip").small())
                    .tip("Leaves this step out for now. It is marked as skipped.");
                chrome.hit(format!("wire skip {}", s.signal), skip.rect);
                if skip.clicked() {
                    hand.push((s.signal.clone(), Some(Done::Skipped)));
                }
            }
        });
    }
}
