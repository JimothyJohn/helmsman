//! A device's inputs and outputs, added to the project one at a time
//! (Nick, 2026-11-15: "it should be assumed that I want access to all of
//! the data ... but that datapoints don't show up in the actual project
//! device pane until I add them").
//!
//! Adding a device writes its line and nothing else. What it has is known
//! from its profile (`NAME.device`, a project file) or, for an E1213 on a
//! typed protocol, from the form's catalog: every channel is there to
//! take, and none is in io.map until it is asked for. Under a device the
//! Devices tab offers **+ Add I/O**: a list of the channels the project
//! does not have yet, narrowed as you type, and a click adds one (its
//! signal in io.map, named after the device and the channel, and a
//! variable of that name in the program). The program's own variables
//! that are on no device can take a channel instead ("For"). A device
//! whose channels nobody knows takes a point by its address.
//!
//! A point leaves the same way: Remove on its row. A device that holds
//! points the program has no variable for (every channel, as Add wrote
//! them before this) offers to remove those in one go; it asks first.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eframe::egui::{self, RichText, Vec2};
use helmsman_project::iomap::{MapDevice, MapText};

use crate::device_form::{self, Catalog, Dir, PointRow, Port, SigTy, Use, Wiring};
use crate::devices::{DeviceAct, DevicesUi};
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

const RED: egui::Color32 = egui::Color32::from_rgb(255, 110, 100);
/// The picker's width.
const WIDTH: f32 = 460.0;
/// Most channels of one device the agent's listing names.
const LISTED: usize = 80;
/// Most channels one profile is read for.
const MAX_CHANNELS: usize = 2048;

/// One channel a device has: what a signal on it would be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub dir: Dir,
    /// What the device calls it: `DI-04`, or the point itself.
    pub label: String,
    pub ty: SigTy,
    /// In the map's syntax: `ch:DI-04`, `di:4`.
    pub point: String,
    /// The rest of its line (`unit=mm safe=false`); `None` for the
    /// defaults.
    pub tail: Option<String>,
}

/// Every channel of `dev`, when something says what they are: the
/// profile it is declared by (`device:STEM`, `profile` its text), else
/// the form's catalog for an E1213 on a typed protocol. `None` when
/// nobody knows.
#[must_use]
pub fn channels(dev: &MapDevice, profile: Option<&str>) -> Option<Vec<Channel>> {
    if dev.driver.starts_with("device:") {
        return Some(of_profile(profile?));
    }
    let known = dev
        .params
        .iter()
        .any(|(k, v)| k == "profile" && v == "moxa-e1213");
    let (vendor, product) = dev
        .endpoint
        .strip_prefix("cip=")
        .map(|c| {
            let mut it = c.split(':').map(|x| x.parse::<u16>().unwrap_or(0));
            (it.next().unwrap_or(0), it.next().unwrap_or(0))
        })
        .unwrap_or_default();
    let catalog = if known {
        Some(Catalog::E1213)
    } else {
        Catalog::identify(vendor, product, &dev.name)
    }?;
    if !matches!(dev.driver.as_str(), "modbus-tcp" | "enip-io") {
        return None;
    }
    Some(
        catalog
            .rows(&dev.driver)
            .into_iter()
            .map(|r| Channel {
                dir: r.dir,
                label: r.name.to_ascii_uppercase(),
                ty: r.ty,
                point: r.point,
                tail: None,
            })
            .collect(),
    )
}

/// The channels a profile lists; none when it does not read.
fn of_profile(text: &str) -> Vec<Channel> {
    let Ok(skim) = helmsman_project::profile_skim::skim(text) else {
        return Vec::new();
    };
    skim.channels
        .iter()
        .take(MAX_CHANNELS)
        .map(|c| {
            let mut tail = String::new();
            if let Some(u) = &c.unit {
                let _ = write!(tail, "unit={u}");
            }
            if let Some(v) = &c.safe {
                if !tail.is_empty() {
                    tail.push(' ');
                }
                let _ = write!(tail, "safe={v}");
            }
            Channel {
                dir: if c.dir == "out" { Dir::Out } else { Dir::In },
                label: c.name.clone(),
                ty: match c.ty.as_str() {
                    "int" => SigTy::Int,
                    "real" => SigTy::Real,
                    _ => SigTy::Bool,
                },
                point: format!("ch:{}", c.name),
                tail: Some(tail),
            }
        })
        .collect()
}

/// The channels of `all` no signal of `map` is on yet.
#[must_use]
pub fn free(all: &[Channel], map: &MapText, device: &str) -> Vec<Channel> {
    all.iter()
        .filter(|c| {
            !map.signals.iter().any(|s| {
                s.device == device && s.dir == c.dir.word() && s.point.eq_ignore_ascii_case(&c.point)
            })
        })
        .cloned()
        .collect()
}

/// Whether `c` is one the typed `query` asks for: every word of it in
/// the channel's name, direction or type, whatever the case.
#[must_use]
pub fn matches(c: &Channel, query: &str) -> bool {
    let hay = format!("{} {} {}", c.label, c.dir.word(), c.ty.word()).to_ascii_lowercase();
    query
        .split_whitespace()
        .all(|w| hay.contains(&w.to_ascii_lowercase()))
}

/// The program's variables a channel can be given to: the ones the map
/// has nowhere, or on the box's own channels.
#[must_use]
pub fn movable(ports: &[Port], map: &MapText) -> Vec<Port> {
    ports
        .iter()
        .filter(|p| {
            map.signals
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(&p.name))
                .is_none_or(|s| s.device == "box")
        })
        .cloned()
        .collect()
}

/// A name for a new signal on `device`'s channel `label` that neither
/// the map nor the program has: the channel's own, `di_04` (Nick,
/// 2026-11-26: short names; the map line already says which device),
/// then with the device in front when that is taken, `rio_di_04`, then
/// `rio_di_04_2`, ... At most [`crate::skipper_map::NAME_MAX`]
/// characters, the longest a Skipper takes: a long device name is cut,
/// never the channel's (`liberty_lexium_mdrive_et_enabled`).
fn fresh_name(device: &str, label: &str, ports: &[Port], map: &MapText) -> String {
    let (tries, base) = names(device, label);
    tries
        .into_iter()
        .find(|n| !taken(n, ports, map))
        .unwrap_or_else(|| {
            (2..100)
                .map(|k| format!("{base}_{k}"))
                .find(|n| !taken(n, ports, map))
                .unwrap_or(base)
        })
}

/// Whether the map or the program has a signal called `n`.
fn taken(n: &str, ports: &[Port], map: &MapText) -> bool {
    map.signals.iter().any(|s| s.name.eq_ignore_ascii_case(n))
        || ports.iter().any(|p| p.name.eq_ignore_ascii_case(n))
}

/// The names [`fresh_name`] tries for `device`'s channel `label`, in
/// order: the channel's own (up to [`crate::skipper_map::NAME_MAX`]),
/// then with the device in front; and the base a `_N` goes after, short
/// enough that it fits too.
fn names(device: &str, label: &str) -> (Vec<String>, String) {
    // Room for a `_N` after it.
    const ROOM: usize = crate::skipper_map::NAME_MAX - 3;
    // An identifier whatever it is called: anything else is a `_`, and
    // never two in a row.
    let ident = |s: &str| {
        let mut out = String::new();
        for c in s.chars() {
            let c = if c.is_ascii_alphanumeric() { c } else { '_' };
            if !(c == '_' && out.ends_with('_')) {
                out.push(c);
            }
        }
        out.trim_matches('_').to_string()
    };
    let full = ident(&helmsman_project::profile_skim::signal_name("", label));
    let own: String = full.chars().take(crate::skipper_map::NAME_MAX).collect();
    let channel: String = full.chars().take(ROOM).collect();
    let keep = ROOM.saturating_sub(channel.len() + 1);
    let device: String = ident(device).chars().take(keep).collect();
    let device = device.trim_end_matches('_');
    if channel.is_empty() {
        let device = device.to_string();
        return (vec![device.clone()], device);
    }
    if device.is_empty() {
        return (vec![own], channel);
    }
    let long = format!("{device}_{channel}");
    (vec![own, long.clone()], long)
}

/// What a click on channel `c` of `dev` writes: its signal after the
/// device's lines, for the program's variable `for_var` (moved off the
/// box's channels) or under a name of its own, which `declare` makes a
/// variable of the program. A variable the program already has under
/// that name, of the channel's direction and type and on no device,
/// takes it back (a point removed and added again).
///
/// # Errors
/// Why the point cannot be added, in words.
pub fn add(
    dev: &MapDevice,
    c: &Channel,
    for_var: Option<&str>,
    declare: bool,
    ports: &[Port],
    map: &MapText,
) -> Result<Wiring, String> {
    // Either name a point of this channel was given: the short one or,
    // before 2026-11-26 or on a clash, the one with the device in front.
    let mut own = names(&dev.name, &c.label).0;
    own.push(helmsman_project::profile_skim::signal_name(&dev.name, &c.label));
    let back = movable(ports, map)
        .into_iter()
        .find(|p| own.iter().any(|o| p.name.eq_ignore_ascii_case(o)) && p.dir == c.dir && p.ty == c.ty);
    let (name, use_) = match (for_var, back) {
        (Some(v), _) => (v.to_string(), Use::Existing(v.to_string())),
        (None, Some(p)) => (p.name.clone(), Use::Existing(p.name)),
        (None, None) => (
            fresh_name(&dev.name, &c.label, ports, map),
            if declare { Use::Declare } else { Use::Wiring },
        ),
    };
    let row = PointRow {
        on: true,
        dir: c.dir,
        name,
        ty: c.ty,
        point: c.point.clone(),
        use_,
        fixed: true,
        tail: c.tail.clone(),
    };
    let line = format!("device {} {}", dev.name, dev.driver);
    let mut w = device_form::wiring(&dev.name, line, &[row], ports, map)?;
    w.extend = true;
    Ok(w)
}

/// The signals on `device` the program has no variable for.
#[must_use]
pub fn unused(map: &MapText, device: &str, ports: &[Port]) -> Vec<String> {
    map.signals
        .iter()
        .filter(|s| s.device == device && !ports.iter().any(|p| p.name.eq_ignore_ascii_case(&s.name)))
        .map(|s| s.name.clone())
        .collect()
}

/// The channels the map's devices have and the project has not added,
/// as text for the agent: one device a line, each channel as the rest
/// of the io.map line that would add it.
#[must_use]
pub fn describe_free(map: &MapText, profile: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    for d in &map.devices {
        let text = d
            .driver
            .strip_prefix("device:")
            .and_then(|stem| profile(&format!("{stem}{}", helmsman_project::DEVICE_EXT)));
        let Some(all) = channels(d, text.as_deref()) else {
            continue;
        };
        let rest = free(&all, map, &d.name);
        if rest.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(
                "Channels the devices have that the project has not added (a line `in|out NAME TYPE DEVICE POINT` in io.map adds one):\n",
            );
        }
        let _ = write!(out, "  {} ({}):", d.name, rest.len());
        for c in rest.iter().take(LISTED) {
            let _ = write!(out, " {} {} {};", c.dir.word(), c.ty.word(), c.point);
        }
        if rest.len() > LISTED {
            let _ = write!(out, " and {} more", rest.len() - LISTED);
        }
        out.push('\n');
    }
    out
}

/// The picker, while it is open on a device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddIo {
    pub device: String,
    pub search: String,
    /// The program's variable the next channel is for; `None`: a new one.
    pub for_var: Option<String>,
    /// A point by its address, for a device nobody knows the channels of.
    pub custom: Option<PointRow>,
    pub error: Option<String>,
    /// The search field takes the keyboard once, as the picker opens.
    focused: bool,
}

/// The Remove question: which signals of which device, and whether they
/// are the ones the program does not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub device: String,
    pub signals: Vec<String>,
    pub unused: bool,
}

/// Every channel of `map`'s device `name`, by the profiles read (see
/// [`DevicesUi::channels_of`]).
#[must_use]
pub fn channels_in(map: &MapText, profiles: &Profiles, name: &str) -> Option<Vec<Channel>> {
    let dev = map.devices.iter().find(|d| d.name == name)?;
    match dev.driver.strip_prefix("device:") {
        Some(stem) => profiles
            .get(&format!("{stem}{}", helmsman_project::DEVICE_EXT))
            .map(|(_, c)| c.clone()),
        None => channels(dev, None),
    }
}

/// The project's device profiles as the tab last saw them: file name to
/// its text and the channels it lists.
pub type Profiles = BTreeMap<String, (String, Vec<Channel>)>;

impl DevicesUi {
    /// Takes in the project's `.device` files (name and text), each frame:
    /// a profile is read again only when its text changed.
    pub fn set_profiles<'a>(&mut self, files: impl Iterator<Item = (&'a str, &'a str)>) {
        let mut next = Profiles::new();
        for (name, text) in files {
            let kept = self.profiles.remove(name).filter(|(t, _)| t == text);
            next.insert(
                name.to_string(),
                kept.unwrap_or_else(|| (text.to_string(), of_profile(text))),
            );
        }
        self.profiles = next;
    }

    /// Every channel of the map's device `name`; `None` when nobody
    /// knows them (or the map has no such device).
    #[must_use]
    pub fn channels_of(&self, name: &str) -> Option<Vec<Channel>> {
        channels_in(&self.map, &self.profiles, name)
    }

    /// Opens the picker on `device`.
    pub fn open_add_io(&mut self, device: &str) {
        self.adding_io = Some(AddIo {
            device: device.to_string(),
            ..AddIo::default()
        });
    }

    /// The row under a device's points: + Add I/O with how many channels
    /// are left, and Remove unused when the device holds points the
    /// program has no variable for. `enabled`: the map can be written (or
    /// the project copied first); `why_not` otherwise.
    pub(crate) fn io_row(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        device: &str,
        write: &Result<(), String>,
    ) -> Option<DeviceAct> {
        let t = theme(ui);
        let mut act = None;
        let left = self
            .channels_of(device)
            .map(|all| free(&all, &self.map, device).len());
        let idle = if self.can_declare {
            unused(&self.map, device, &self.program)
        } else {
            Vec::new()
        };
        let can = write.is_ok() || self.copy_first;
        ui.horizontal_wrapped(|ui| {
            ui.add_space(22.0);
            let tip = match (write, left) {
                (Err(why), _) if !self.copy_first => why.clone(),
                (_, Some(0)) => "Every channel of this device is in the project already.".to_string(),
                (_, Some(_)) => "Lists the channels this device has that the project does not use yet. A click adds one: its signal, and a variable of that name in the program.".to_string(),
                (_, None) => "Adds an input or an output of this device by its address.".to_string(),
            };
            let b = ui
                .add_enabled(can && left != Some(0), egui::Button::new("+ Add I/O").small())
                .tip(tip);
            chrome.hit(format!("add io {device}"), b.rect);
            if b.clicked() {
                act = Some(DeviceAct::AddIo(device.to_string()));
            }
            let wire = ui
                .add(egui::Button::new("Wire it").small())
                .tip("Steps through wiring this device's inputs and checks each as you switch it. Changes nothing.");
            chrome.hit(format!("wire it {device}"), wire.rect);
            if wire.clicked() {
                self.open_wire_it(device);
            }
            if let Some(n) = left.filter(|n| *n > 0) {
                ui.label(RichText::new(format!("{n} more available")).color(t.muted))
                    .tip("Channels the device has that are not in the project. They stay out of the lists until you add them.");
            }
            if !idle.is_empty() {
                let b = ui
                    .add_enabled(
                        write.is_ok(),
                        egui::Button::new(format!("Remove {} unused", idle.len())).small(),
                    )
                    .tip(write.as_ref().err().map_or(
                        "Takes the points the program has no variable for out of io.map. Asks first; the channels stay available under + Add I/O.",
                        String::as_str,
                    ));
                chrome.hit(format!("remove unused {device}"), b.rect);
                if b.clicked() {
                    self.removing = Some(Removal {
                        device: device.to_string(),
                        signals: idle.clone(),
                        unused: true,
                    });
                }
            }
        });
        act
    }

    /// The picker. Returns the wiring of the channel just clicked; it
    /// stays open for the next until Done.
    pub fn add_io_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        let mut a = self.adding_io.clone()?;
        let Some(dev) = self.map.devices.iter().find(|d| d.name == a.device).cloned() else {
            self.adding_io = None;
            return None;
        };
        let all = self.channels_of(&dev.name);
        let rest = all.as_ref().map(|all| free(all, &self.map, &dev.name));
        let vars = movable(&self.program, &self.map);
        // A variable picked before the program changed under the picker.
        if a.for_var
            .as_ref()
            .is_some_and(|v| !vars.iter().any(|p| &p.name == v))
        {
            a.for_var = None;
        }
        let target = a
            .for_var
            .as_ref()
            .and_then(|v| vars.iter().find(|p| &p.name == v))
            .cloned();
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "device-add-io").show(ctx, |ui| {
            let t = theme(ui);
            ui.set_min_width(WIDTH);
            ui.set_max_width(WIDTH);
            ui.label(RichText::new(format!("Add I/O to {}", dev.name)).strong().size(20.0));
            ui.add_space(6.0);
            let mut picked: Option<Channel> = None;
            if let Some(rest) = &rest {
                ui.horizontal_wrapped(|ui| {
                    let tip = "Narrows the list as you type: a channel's name, in or out, bool, int or real.";
                    ui.label("Find").tip(tip);
                    let f = ui
                        .add(
                            egui::TextEdit::singleline(&mut a.search)
                                .hint_text("DI-0")
                                .desired_width(220.0),
                        )
                        .tip(tip);
                    chrome.hit("io search", f.rect);
                    if !a.focused {
                        f.request_focus();
                        a.focused = true;
                    }
                });
                if !vars.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        let tip = "What the next channel is for: a new variable named after the device and the channel, or one the program already has that is on no device yet.";
                        ui.label("For").tip(tip);
                        let shown = a.for_var.clone().unwrap_or_else(|| "a new variable".into());
                        let c = egui::ComboBox::from_id_salt("io-for")
                            .width(220.0)
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut a.for_var, None, "a new variable")
                                    .tip("The channel gets a signal and a program variable named after it.");
                                for p in &vars {
                                    ui.selectable_value(
                                        &mut a.for_var,
                                        Some(p.name.clone()),
                                        format!("{}  ({} {})", p.name, p.dir.word(), p.ty.word()),
                                    )
                                    .tip("The program's own variable: the channel you click next is wired to it.");
                                }
                            })
                            .response
                            .tip(tip);
                        chrome.hit("io for", c.rect);
                    });
                }
                ui.add_space(6.0);
                let fits = |c: &&Channel| {
                    matches(c, &a.search) && target.as_ref().is_none_or(|p| p.dir == c.dir && p.ty == c.ty)
                };
                let shown: Vec<&Channel> = rest.iter().filter(fits).collect();
                if rest.is_empty() {
                    ui.label(RichText::new("Every channel of this device is in the project.").color(t.muted));
                } else if shown.is_empty() {
                    ui.label(RichText::new("No channel matches.").color(t.muted));
                }
                let rows_h = (ctx.content_rect().height() * 0.4).clamp(140.0, 360.0);
                egui::ScrollArea::vertical()
                    .id_salt("io-channels")
                    .max_height(rows_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (dir, title) in [(Dir::In, "Inputs"), (Dir::Out, "Outputs")] {
                            let group: Vec<&&Channel> = shown.iter().filter(|c| c.dir == dir).collect();
                            if group.is_empty() {
                                continue;
                            }
                            ui.label(RichText::new(format!("{title} ({})", group.len())).color(t.muted));
                            for c in group {
                                let text = format!("{}   {}", c.label, c.ty.word());
                                let b = ui
                                    .add(
                                        egui::Button::new(RichText::new(text).monospace())
                                            .min_size(Vec2::new(ui.available_width() - 12.0, TAP * 0.6))
                                            .truncate(),
                                    )
                                    .tip(match &target {
                                        Some(p) => format!("Wires {} to {} of {}.", p.name, c.label, dev.name),
                                        None => format!("Adds {} of {} to the project.", c.label, dev.name),
                                    });
                                chrome.hit(format!("io add {}", c.label), b.rect);
                                if b.clicked() {
                                    picked = Some((**c).clone());
                                }
                            }
                        }
                    });
            }
            // A point by its address: the only way in for a device whose
            // channels nobody knows, and Advanced's for the others.
            if rest.is_none() || chrome.advanced {
                picked = picked.or(custom_row(ui, chrome, &mut a, &dev));
            }
            if let Some(c) = picked {
                match add(&dev, &c, a.for_var.as_deref(), self.can_declare, &self.program, &self.map) {
                    Ok(w) => {
                        out = Some(DeviceAct::Add(w));
                        a.error = None;
                        a.for_var = None;
                        a.custom = None;
                    }
                    Err(e) => a.error = Some(e),
                }
            }
            if let Some(err) = &a.error {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(err).color(theme(ui).ink(RED))).wrap());
            }
            ui.add_space(12.0);
            let ok = ui
                .add(egui::Button::new(RichText::new("Done").strong()).min_size(Vec2::new(0.0, TAP * 0.7)))
                .tip("Closes the list. What you added is under the device.");
            chrome.hit("io done", ok.rect);
            done = ok.clicked();
        });
        chrome.hit("io form", m.response.rect);
        self.adding_io = if done || crate::theme::escaped(&m) {
            None
        } else {
            Some(a)
        };
        out
    }

    /// The Remove question. Returns the signals to take out of io.map
    /// once Remove is pressed.
    pub fn remove_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        let r = self.removing.clone()?;
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "device-remove-io").show(ctx, |ui| {
            ui.set_max_width(WIDTH);
            let title = match (r.unused, r.signals.as_slice()) {
                (false, [one]) => format!("Remove {one} from {}?", r.device),
                _ => format!("Remove {} unused point(s) from {}?", r.signals.len(), r.device),
            };
            ui.label(RichText::new(title).strong().size(20.0));
            ui.add_space(8.0);
            let body = if r.unused {
                let mut names: Vec<&str> = r.signals.iter().take(6).map(String::as_str).collect();
                if r.signals.len() > names.len() {
                    names.push("…");
                }
                format!(
                    "The program has no variable for these: {}. Their lines go out of io.map. The channels stay on the device, under + Add I/O.",
                    names.join(", ")
                )
            } else {
                "Takes its line out of io.map. The program's variable stays, and the channel is back under + Add I/O.".to_string()
            };
            ui.add(egui::Label::new(body).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let ok = ui
                    .add(egui::Button::new(RichText::new("Remove").strong().color(theme(ui).ink(RED))).min_size(tall))
                    .tip("Removes the lines. Save keeps the change; Deploy runs it.");
                chrome.hit("remove io ok", ok.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Keeps them.");
                chrome.hit("remove io cancel", no.rect);
                if ok.clicked() {
                    out = Some(DeviceAct::Remove(r.signals.clone()));
                    done = true;
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("remove io form", m.response.rect);
        if done || m.should_close() {
            self.removing = None;
        }
        out
    }
}

/// The typed point's row: in or out, its type, its address, Add. Returns
/// the channel once Add is pressed.
fn custom_row(ui: &mut egui::Ui, chrome: &mut Chrome, a: &mut AddIo, dev: &MapDevice) -> Option<Channel> {
    let t = theme(ui);
    let mut r = a.custom.clone().unwrap_or_else(|| PointRow::custom(Dir::In, 0));
    let mut out = None;
    ui.add_space(6.0);
    ui.label(RichText::new("A point by its address").color(t.muted))
        .tip("For a channel the list does not have: say whether the program reads or drives it, its type, and where it is on the device.");
    ui.horizontal_wrapped(|ui| {
        for dir in [Dir::In, Dir::Out] {
            let b = ui
                .add(egui::Button::selectable(r.dir == dir, dir.word()))
                .tip(if dir == Dir::In {
                    "An input: the program reads it."
                } else {
                    "An output: the program drives it."
                });
            chrome.hit(format!("io custom {}", dir.word()), b.rect);
            if b.clicked() {
                r.dir = dir;
            }
        }
        egui::ComboBox::from_id_salt("io-custom-ty")
            .width(64.0)
            .selected_text(r.ty.word())
            .show_ui(ui, |ui| {
                for ty in SigTy::ALL {
                    ui.selectable_value(&mut r.ty, ty, ty.word())
                        .tip("The signal's type in io.map.");
                }
            })
            .response
            .tip("bool for a contact or a coil, int for a count or a register, real for a measurement.");
        let hint = match r.dir {
            Dir::In => "di:0".to_string(),
            Dir::Out => format!("{}:0", device_form::out_bit(&dev.driver)),
        };
        let p = ui
            .add(egui::TextEdit::singleline(&mut r.point).hint_text(hint).desired_width(120.0))
            .tip("The point on the device: di:N, coil:N, hr:N:i16 on Modbus; di:N, do:N, in:OFFSET:FMT on EtherNet/IP.");
        chrome.hit("io custom point", p.rect);
        let b = ui
            .add_enabled(!r.point.trim().is_empty(), egui::Button::new("Add"))
            .tip("Adds this point to the project.");
        chrome.hit("io custom add", b.rect);
        if b.clicked() {
            let point = r.point.trim().to_string();
            out = Some(Channel {
                dir: r.dir,
                label: point.replace(':', "_"),
                ty: r.ty,
                point,
                tail: None,
            });
        }
    });
    a.custom = Some(r);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_form::{apply_map, ports_of, remove_signals};
    use helmsman_project::iomap::skim;

    const E1213: &str = include_str!("../../../docs/devices/moxa-iologik-e1213.device");
    const LATCH: &str = "PROGRAM latch\nVAR_INPUT\n  start : BOOL;\n  stop : BOOL;\nEND_VAR\nVAR_OUTPUT\n  motor : BOOL;\nEND_VAR\nmotor := (start OR motor) AND NOT stop;\nEND_PROGRAM\n";
    const MAP: &str = "# cell 4\nin  start bool box di:0\nin  stop bool box di:1\nout motor bool box do:0 safe=false\ndevice rio device:moxa-iologik-e1213 cip=991:515:0x00e12130  # the block\nin  part bool rio ch:DI-04\n\ndevice other modbus-tcp 10.0.0.9:502 unit=1\n";

    fn ch<'a>(all: &'a [Channel], label: &str) -> &'a Channel {
        all.iter().find(|c| c.label == label).unwrap()
    }

    #[test]
    fn a_profile_lists_every_channel_and_the_map_takes_them_one_by_one() {
        let map = skim(MAP);
        let rio = &map.devices[0];
        let all = channels(rio, Some(E1213)).unwrap();
        assert_eq!(all.len(), 40);
        let rest = free(&all, &map, "rio");
        assert_eq!(rest.len(), 39, "DI-04 is in the project");
        assert!(!rest.iter().any(|c| c.label == "DI-04"));
        // A status input and the output it reports are two channels.
        assert!(rest.iter().any(|c| c.label == "DO-00" && c.dir == Dir::Out));
        assert!(rest.iter().any(|c| c.label == "DO-00-status" && c.dir == Dir::In));
        // The point is matched whatever its case, as the box reads it.
        let lower = skim("device rio device:moxa-iologik-e1213 cip=1:2\nin x bool rio ch:di-04\n");
        assert_eq!(free(&all, &lower, "rio").len(), 39);
        // No profile in the project: nobody knows its channels.
        assert_eq!(channels(rio, None), None);
        assert_eq!(channels(rio, Some("not a profile")), Some(Vec::new()));
        // A typed device nobody knows, and a typed E1213.
        assert_eq!(channels(&map.devices[1], None), None);
        let typed = skim("device e1213 enip-io cip=991:515:0x1 rpi_ms=100\n");
        let known = channels(&typed.devices[0], None).unwrap();
        assert_eq!(known.len(), 20);
        assert_eq!(ch(&known, "DO0").point, "do:0");
    }

    #[test]
    fn a_click_adds_one_signal_after_the_devices_lines_and_declares_it() {
        let map = skim(MAP);
        let ports = ports_of(LATCH);
        let all = channels(&map.devices[0], Some(E1213)).unwrap();
        let w = add(&map.devices[0], ch(&all, "DO-00"), None, true, &ports, &map).unwrap();
        assert!(w.extend);
        assert_eq!(w.signals, ["out do_00 bool rio ch:DO-00 safe=false"]);
        assert_eq!(w.declare, [(Dir::Out, "do_00".to_string(), SigTy::Bool)]);
        assert_eq!(w.summary(), "do_00 added to rio; new variable(s) do_00.");
        let out = apply_map(MAP, &w);
        assert!(
            out.contains("device rio device:moxa-iologik-e1213 cip=991:515:0x00e12130  # the block\nin  part bool rio ch:DI-04\nout do_00 bool rio ch:DO-00 safe=false\n\ndevice other"),
            "{out}"
        );
        assert_eq!(
            out.matches("device rio").count(),
            1,
            "the device's line is not written again: {out}"
        );
        assert!(out.starts_with("# cell 4\nin  start bool box di:0\n"), "{out}");
        assert_eq!(skim(&out).odd, 0, "{out}");
        // Without a program to declare in, it is wiring only.
        let w = add(&map.devices[0], ch(&all, "DI-00-count"), None, false, &[], &map).unwrap();
        assert_eq!(w.signals, ["in  di_00_count int rio ch:DI-00-count"]);
        assert_eq!(w.declare, []);
        // A device with no line after it, and a map with no final newline.
        let bare = "device rio device:moxa-iologik-e1213 cip=1:2";
        let w = add(
            &skim(bare).devices[0],
            ch(&all, "DI-00"),
            None,
            false,
            &[],
            &skim(bare),
        )
        .unwrap();
        assert_eq!(
            apply_map(bare, &w),
            "device rio device:moxa-iologik-e1213 cip=1:2\nin  di_00 bool rio ch:DI-00\n"
        );
    }

    #[test]
    fn a_channel_goes_to_the_programs_own_variable_and_moves_it_off_the_box() {
        let map = skim(MAP);
        let ports = ports_of(LATCH);
        let vars: Vec<String> = movable(&ports, &map).into_iter().map(|p| p.name).collect();
        assert_eq!(vars, ["start", "stop", "motor"]);
        let all = channels(&map.devices[0], Some(E1213)).unwrap();
        let w = add(
            &map.devices[0],
            ch(&all, "DI-00"),
            Some("start"),
            true,
            &ports,
            &map,
        )
        .unwrap();
        assert_eq!(w.replace, ["start"]);
        assert_eq!(w.declare, []);
        let out = apply_map(MAP, &w);
        assert!(!out.contains("box di:0"), "moved, not doubled: {out}");
        assert!(
            out.contains("in  part bool rio ch:DI-04\nin  start bool rio ch:DI-00\n"),
            "{out}"
        );
        // A variable of the wrong direction or type is refused by name.
        let e = add(
            &map.devices[0],
            ch(&all, "DO-00"),
            Some("start"),
            true,
            &ports,
            &map,
        )
        .unwrap_err();
        assert!(e.contains("start is a bool in"), "{e}");
        let e = add(
            &map.devices[0],
            ch(&all, "DI-00"),
            Some("ghost"),
            true,
            &ports,
            &map,
        )
        .unwrap_err();
        assert!(e.contains("no variable ghost"), "{e}");
        // Once on a device, a variable is not offered again.
        let after = skim(&out);
        let vars: Vec<String> = movable(&ports, &after).into_iter().map(|p| p.name).collect();
        assert_eq!(vars, ["stop", "motor"]);
    }

    #[test]
    fn a_name_that_is_taken_gets_a_number_and_a_removed_point_comes_back_to_its_variable() {
        // Only rio_di_00 is taken: the channel's own name is free.
        let text = "device rio device:moxa-iologik-e1213 cip=1:2\nin rio_di_00 bool box di:0\n";
        let map = skim(text);
        let all = channels(&map.devices[0], Some(E1213)).unwrap();
        let w = add(&map.devices[0], ch(&all, "DI-00"), None, false, &[], &map).unwrap();
        assert_eq!(w.signals, ["in  di_00 bool rio ch:DI-00"]);
        // The short name taken (whatever its case) puts the device in
        // front; that taken too gives a number after it.
        let text = "device rio device:moxa-iologik-e1213 cip=1:2\nin DI_00 bool box di:0\n";
        let map = skim(text);
        let w = add(&map.devices[0], ch(&all, "DI-00"), None, false, &[], &map).unwrap();
        assert_eq!(w.signals, ["in  rio_di_00 bool rio ch:DI-00"]);
        let text = "device rio device:moxa-iologik-e1213 cip=1:2\nin DI_00 bool box di:0\nin Rio_Di_00 bool box di:1\n";
        let map = skim(text);
        let w = add(&map.devices[0], ch(&all, "DI-00"), None, false, &[], &map).unwrap();
        assert_eq!(w.signals, ["in  rio_di_00_2 bool rio ch:DI-00"]);
        let text = "device rio device:moxa-iologik-e1213 cip=1:2\nin di_00 bool box di:0\nin rio_di_00 bool box di:1\nin rio_di_00_2 bool box di:2\n";
        let w = add(
            &skim(text).devices[0],
            ch(&all, "DI-00"),
            None,
            false,
            &[],
            &skim(text),
        )
        .unwrap();
        assert_eq!(w.signals, ["in  rio_di_00_3 bool rio ch:DI-00"]);
        // A program port of the short name takes it too, even one whose
        // type does not match the channel.
        let map = skim("device rio device:moxa-iologik-e1213 cip=1:2\n");
        let ports = ports_of("PROGRAM p\nVAR_INPUT\n  DI_03 : INT;\nEND_VAR\nEND_PROGRAM\n");
        let w = add(&map.devices[0], ch(&all, "DI-03"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["in  rio_di_03 bool rio ch:DI-03"]);
        assert_eq!(w.declare.len(), 1, "a new variable, the INT left alone");
        // The program kept rio_di_01 after its point was removed: adding
        // the channel again wires that variable, and declares nothing.
        // The same for a variable under the short name (di_05).
        let map = skim(text);
        let st = "PROGRAM p\nVAR_INPUT\n  rio_di_01 : BOOL;\n  rio_di_02 : INT;\n  di_05 : BOOL;\nEND_VAR\nVAR_OUTPUT\n  do_01 : BOOL;\n  rio_do_02 : BOOL;\nEND_VAR\nEND_PROGRAM\n";
        let ports = ports_of(st);
        let w = add(&map.devices[0], ch(&all, "DI-01"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["in  rio_di_01 bool rio ch:DI-01"]);
        assert_eq!(w.declare, []);
        let w = add(&map.devices[0], ch(&all, "DI-05"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["in  di_05 bool rio ch:DI-05"]);
        assert_eq!(w.declare, []);
        let w = add(&map.devices[0], ch(&all, "DO-01"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["out do_01 bool rio ch:DO-01 safe=false"]);
        assert_eq!(w.declare, []);
        let w = add(&map.devices[0], ch(&all, "DO-02"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["out rio_do_02 bool rio ch:DO-02 safe=false"]);
        assert_eq!(w.declare, []);
        // A variable of that name but another type is left alone: the
        // short name is free, so the point takes it.
        let w = add(&map.devices[0], ch(&all, "DI-02"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["in  di_02 bool rio ch:DI-02"]);
        assert_eq!(w.declare.len(), 1);
        // Another direction is left alone too: an output di_06 is no
        // input's variable.
        let ports = ports_of("PROGRAM p\nVAR_OUTPUT\n  di_06 : BOOL;\nEND_VAR\nEND_PROGRAM\n");
        let w = add(&map.devices[0], ch(&all, "DI-06"), None, true, &ports, &map).unwrap();
        assert_eq!(w.signals, ["in  rio_di_06 bool rio ch:DI-06"]);
        assert_eq!(w.declare.len(), 1);
    }

    #[test]
    fn hostile_points_and_names_are_refused_or_made_safe() {
        let map = skim("device m modbus-tcp 10.0.0.9:502 unit=1\n");
        let dev = &map.devices[0];
        let typed = |point: &str| Channel {
            dir: Dir::In,
            label: point.replace(':', "_"),
            ty: SigTy::Bool,
            point: point.to_string(),
            tail: None,
        };
        for bad in [
            "",
            "di:0 # x",
            "di:99999",
            "di:0\nin evil bool box di:1",
            "coil",
            "../x",
        ] {
            assert!(add(dev, &typed(bad), None, false, &[], &map).is_err(), "{bad:?}");
        }
        let w = add(dev, &typed("hr:5:i16"), None, false, &[], &map).unwrap();
        assert_eq!(w.signals, ["in  hr_5_i16 bool m hr:5:i16"]);
        // A channel name from a profile with odd characters still gives
        // an identifier, and a long one is cut.
        let odd = Channel {
            dir: Dir::In,
            label: format!("Zone 1/Temp.{}", "x".repeat(80)),
            ty: SigTy::Real,
            point: "ch:Z1".into(),
            tail: Some(String::new()),
        };
        let by_profile = skim("device rio device:acme cip=1:2\n");
        let w = add(&by_profile.devices[0], &odd, None, false, &[], &by_profile).unwrap();
        let name = w.signals[0].split_whitespace().nth(1).unwrap().to_string();
        assert!(device_form::name_ok(&name), "{name}");
    }

    #[test]
    fn the_search_matches_every_word_whatever_the_case() {
        let all = of_profile(E1213);
        let hits = |q: &str| all.iter().filter(|c| matches(c, q)).count();
        assert_eq!(hits(""), 40);
        assert_eq!(hits("di-0"), 20, "DI-00..09 and their counters");
        assert_eq!(hits("DI-04"), 2);
        assert_eq!(hits("out"), 8);
        assert_eq!(hits("count int"), 12);
        assert_eq!(hits("status do-07"), 1);
        assert_eq!(hits("nothing"), 0);
    }

    #[test]
    fn unused_points_are_the_ones_the_program_has_no_variable_for() {
        let text = "device rio device:moxa-iologik-e1213 cip=1:2\nin  START bool rio ch:DI-00\nin  rio_di_01 bool rio ch:DI-01  # spare\nout rio_do_00 bool rio ch:DO-00 safe=false\nin  stop bool box di:1\n";
        let map = skim(text);
        let ports = ports_of(LATCH);
        let idle = unused(&map, "rio", &ports);
        assert_eq!(
            idle,
            ["rio_di_01", "rio_do_00"],
            "start is the program's, whatever its case"
        );
        let out = remove_signals(text, &idle);
        assert_eq!(
            out,
            "device rio device:moxa-iologik-e1213 cip=1:2\nin  START bool rio ch:DI-00\nin  stop bool box di:1\n"
        );
        assert_eq!(remove_signals(text, &[]), text);
        assert_eq!(remove_signals("", &idle), "");
        // A device line or a comment that names a signal is not a signal.
        let odd = "device rio_di_01 modbus-tcp 1.2.3.4:502\n# in rio_di_01 bool\n";
        assert_eq!(remove_signals(odd, &idle), odd);
    }

    #[test]
    fn the_agent_reads_what_is_left_to_add() {
        let map = skim(MAP);
        let profile = |f: &str| (f == "moxa-iologik-e1213.device").then(|| E1213.to_string());
        let text = describe_free(&map, &profile);
        assert!(text.contains("  rio (39): in bool ch:DI-00; "), "{text}");
        assert!(!text.contains("ch:DI-04;"), "{text}");
        assert!(text.contains("out bool ch:DO-07;"), "{text}");
        assert!(
            !text.contains("other"),
            "a device nobody knows is not listed: {text}"
        );
        assert_eq!(describe_free(&skim("in a bool box di:0\n"), &profile), "");
        assert_eq!(describe_free(&map, &|_| None), "");
    }

    #[test]
    fn profiles_are_read_once_and_again_when_their_text_changes() {
        let mut ui = DevicesUi::default();
        ui.map = skim(MAP);
        assert_eq!(ui.channels_of("rio"), None, "no profile yet");
        ui.set_profiles([("moxa-iologik-e1213.device", E1213)].into_iter());
        assert_eq!(ui.channels_of("rio").unwrap().len(), 40);
        let shorter: String = E1213
            .lines()
            .filter(|l| !l.contains("-count"))
            .collect::<Vec<_>>()
            .join("\n");
        ui.set_profiles([("moxa-iologik-e1213.device", shorter.as_str())].into_iter());
        assert_eq!(ui.channels_of("rio").unwrap().len(), 28);
        ui.set_profiles(std::iter::empty());
        assert_eq!(ui.channels_of("rio"), None, "the file left the project");
        assert_eq!(ui.channels_of("ghost"), None);
    }

    /// Nick 2026-11-24: Add I/O on the drive named a point
    /// `liberty_lexium_mdrive_ethernet_lmd_ether` (40 characters), which a
    /// Skipper refuses. A long device name is cut; the channel is kept.
    #[test]
    fn a_new_points_name_fits_a_skipper_and_keeps_its_channel() {
        let map = MapText::default();
        let long = "liberty_lexium_mdrive_ethernet_lmd_ethernet_ip";
        // Nick 2026-11-26: the channel's own name, nothing in front.
        assert_eq!(fresh_name(long, "enabled", &[], &map), "enabled");
        assert_eq!(fresh_name("rio", "DI-04", &[], &map), "di_04");
        // Taken: the device goes in front, cut to fit; the channel is kept.
        let map = helmsman_project::iomap::skim("in  enabled bool sim\n");
        let n = fresh_name(long, "enabled", &[], &map);
        assert!(n.len() <= crate::skipper_map::NAME_MAX, "{n}");
        assert!(n.ends_with("_enabled") && n.starts_with("liberty_"), "{n}");
        // A taken prefixed name gets a number and still fits.
        let map = helmsman_project::iomap::skim(&format!("in  enabled bool sim\nin  {n} bool sim\n"));
        let second = fresh_name(long, "enabled", &[], &map);
        assert!(second.len() <= crate::skipper_map::NAME_MAX, "{second}");
        assert!(
            second.ends_with("_enabled_2") && second.starts_with("liberty_"),
            "{second}"
        );
    }
}
