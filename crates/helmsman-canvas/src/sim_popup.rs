//! The popup a double-click on an input opens in the simulation (Nick,
//! 2026-11-14: "for input nodes you need to have common types, that when
//! double-clicked, will allow you to simulate their behavior in normal,
//! abnormal, and absent scenarios"): what device the input stands for,
//! its settings, and how the input behaves. Apply writes the project's
//! `plant.sim` (helmsman-sim), which the simulation runs from the next
//! scan.
//!
//! [`Popup::open`] fills it from the device already in the file, or from a
//! guess on the input's name and type ([`guess`]); [`Popup::apply`] makes
//! the new file text and refuses one the file would not take. Both are
//! tested natively; [`Popup::show`] draws it.

use eframe::egui::{self, RichText, Vec2};
use helmsman_sim::{Kind, Mode, Role, Spec};

use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

/// Most devices a name is looked for among (`heater1` ..).
pub const MAX_DEVICES: usize = 2000;

/// How the input behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behaviour {
    Normal,
    Abnormal,
    Absent,
}

/// The abnormal behaviours an input of a kind can take: (mode word, what
/// its value is, a hint for the field).
#[must_use]
pub fn faults(is_bool: bool) -> &'static [(&'static str, &'static str, &'static str)] {
    if is_bool {
        &[
            ("stuck", "Stuck at TRUE or FALSE (empty: where it was).", "TRUE"),
            ("chatter", "Bounces for this long on each change.", "50ms"),
            ("slow", "Its device answers this many times slower.", "4"),
        ]
    } else {
        &[
            ("stuck", "Stuck at this value (empty: where it was).", "80"),
            ("drift", "Off by this much more each second.", "0.5"),
            ("noise", "Random noise of up to this much either way.", "2"),
            ("over", "Past the top of its range.", ""),
            ("under", "Below the bottom of its range.", ""),
            ("slow", "Its device answers this many times slower.", "4"),
        ]
    }
}

/// The popup's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    /// The program input, lower case.
    pub input: String,
    pub is_bool: bool,
    /// The device it stands for; `None`: set by hand, as without a plant.
    pub kind: Option<Kind>,
    pub name: String,
    /// The device line in the file, by name, when there is one.
    pub old_name: Option<String>,
    /// Every key of `kind` and what is typed for it (empty: its default).
    pub values: Vec<(String, String)>,
    pub behaviour: Behaviour,
    /// The abnormal mode's word and value.
    pub fault: String,
    pub fault_value: String,
    /// Whether the program has `INPUT_ok`.
    pub has_ok: bool,
    pub error: Option<String>,
}

/// What was pressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Write it into `plant.sim` ([`Popup::apply`]).
    Apply,
    Close,
}

/// The kind an input of this name and type most likely stands for, and
/// the key it is on that kind's line.
#[must_use]
pub fn guess(input: &str, is_bool: bool) -> (Kind, &'static str) {
    let lower = input.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c == '_' || c.is_ascii_digit())
        .filter(|w| !w.is_empty())
        .collect();
    let any = |ws: &[&str]| words.iter().any(|w| ws.contains(w));
    if is_bool {
        if any(&["pb", "button", "push", "start", "stop", "reset", "ack", "estop"]) {
            (Kind::Button, "input")
        } else if any(&["closed", "retracted", "zsc"]) {
            (Kind::Valve, "closed")
        } else if any(&["open", "opened", "extended", "limit", "ls", "zso"]) {
            (Kind::Valve, "opened")
        } else if any(&["eye", "pe", "photo", "seen", "part", "prox", "present"]) {
            (Kind::Eye, "seen")
        } else if any(&["running", "rdy", "aux", "fb"]) {
            (Kind::Motor, "running")
        } else if any(&["high", "hi", "full", "lsh"]) {
            (Kind::Tank, "high")
        } else if any(&["low", "lo", "empty", "lsl"]) {
            (Kind::Tank, "low")
        } else {
            (Kind::Switch, "input")
        }
    } else if any(&["ma", "raw", "ai", "counts", "volts", "v"]) {
        (Kind::Sensor, "raw")
    } else if any(&["temp", "tc", "rtd", "degc", "degf", "heat", "t"]) {
        (Kind::Heater, "temp")
    } else if any(&["level", "lvl", "height", "depth"]) {
        (Kind::Tank, "level")
    } else if any(&["speed", "rpm", "hz", "velocity"]) {
        (Kind::Motor, "speed")
    } else if any(&["pos", "position", "stroke"]) {
        (Kind::Valve, "position")
    } else if any(&["sel", "selector", "mode"]) {
        (Kind::Selector, "input")
    } else {
        (Kind::Source, "value")
    }
}

/// Words that suggest the output a device's `key` is wired to.
fn output_hints(kind: Kind, key: &str) -> &'static [&'static str] {
    match (kind, key) {
        (Kind::Heater, "heat") => &["heat", "heater", "burner", "element"],
        (Kind::Tank, "fill") => &["fill", "inlet", "pump", "valve"],
        (Kind::Tank, "drain") => &["drain", "outlet", "discharge"],
        (Kind::Motor, "run") => &["run", "motor", "start", "fwd", "conveyor", "belt"],
        (Kind::Valve, "open") => &["open", "valve", "extend", "ext", "sol"],
        (Kind::Valve, "close") => &["close", "retract", "ret"],
        (Kind::Eye, "belt") => &["belt", "conv", "conveyor", "motor", "run"],
        _ => &[],
    }
}

/// The output among `outputs` (name, is BOOL) that `key` of `kind` is most
/// likely wired to.
fn wire(kind: Kind, key: &str, outputs: &[(String, bool)]) -> Option<String> {
    let hints = output_hints(kind, key);
    outputs
        .iter()
        .find(|(o, _)| {
            let o = o.to_ascii_lowercase();
            hints.iter().any(|h| o.contains(h))
        })
        .map(|(o, _)| o.to_ascii_lowercase())
}

impl Popup {
    /// The popup for `input`, from its device in `spec` if it has one, or
    /// from a guess, wired to the likeliest of `outputs` (name, is BOOL).
    /// `inputs` (lower case) say whether the program has `input_ok`.
    #[must_use]
    pub fn open(
        input: &str,
        is_bool: bool,
        spec: &Spec,
        mode: Mode,
        outputs: &[(String, bool)],
        inputs: &[String],
    ) -> Self {
        let input = input.to_ascii_lowercase();
        let ok = format!("{input}_ok");
        let (behaviour, fault, fault_value) = match mode {
            Mode::Normal => (Behaviour::Normal, String::new(), String::new()),
            Mode::Absent => (Behaviour::Absent, String::new(), String::new()),
            m => {
                let line = m.to_line("x");
                let mut w = line.split_whitespace().skip(2);
                (
                    Behaviour::Abnormal,
                    w.next().unwrap_or_default().to_string(),
                    w.next().unwrap_or_default().to_string(),
                )
            }
        };
        let mut p = Self {
            is_bool,
            kind: None,
            name: String::new(),
            old_name: None,
            values: Vec::new(),
            behaviour,
            fault,
            fault_value,
            has_ok: inputs.iter().any(|i| i.eq_ignore_ascii_case(&ok)),
            error: None,
            input,
        };
        if let Some((d, _, _)) = spec.owner(&p.input) {
            p.kind = Some(d.kind);
            p.name.clone_from(&d.name);
            p.old_name = Some(d.name.clone());
            p.values = d
                .kind
                .keys()
                .iter()
                .map(|k| (k.name.to_string(), d.port(k.name).unwrap_or("").to_string()))
                .collect();
        } else {
            let (kind, key) = guess(&p.input, is_bool);
            p.set_kind(Some(kind), key, spec, outputs);
        }
        if p.fault.is_empty() {
            p.fault = faults(is_bool)[0].0.to_string();
        }
        p
    }

    /// Makes it a `kind` device, the input on `key`, its outputs wired to
    /// the likeliest of `outputs`, named after the kind (free in `spec`).
    pub fn set_kind(&mut self, kind: Option<Kind>, key: &str, spec: &Spec, outputs: &[(String, bool)]) {
        self.kind = kind;
        let Some(kind) = kind else {
            self.values.clear();
            return;
        };
        if self.old_name.is_none() {
            self.name = (1..=crate::sim_popup::MAX_DEVICES)
                .map(|n| format!("{}{n}", kind.word()))
                .find(|n| spec.device(n).is_none())
                .unwrap_or_default();
        }
        self.values = kind
            .keys()
            .iter()
            .map(|k| {
                let v = if k.name == key {
                    self.input.clone()
                } else if k.role == Role::Out && k.required {
                    wire(kind, k.name, outputs)
                        .or_else(|| {
                            outputs
                                .iter()
                                .find(|(_, b)| *b)
                                .map(|(o, _)| o.to_ascii_lowercase())
                        })
                        .unwrap_or_default()
                } else if k.role == Role::Out {
                    wire(kind, k.name, outputs).unwrap_or_default()
                } else if k.role == Role::Of {
                    spec.devices
                        .iter()
                        .find(|d| d.kind.quantity().is_some())
                        .map(|d| d.name.clone())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                (k.name.to_string(), v)
            })
            .collect();
    }

    /// The input's mode as the popup says it.
    ///
    /// # Errors
    /// The abnormal value does not read.
    pub fn mode(&self) -> Result<Mode, String> {
        match self.behaviour {
            Behaviour::Normal => Ok(Mode::Normal),
            Behaviour::Absent => Ok(Mode::Absent),
            Behaviour::Abnormal => {
                let v = self.fault_value.trim();
                Mode::parse(&self.fault, (!v.is_empty()).then_some(v))
            }
        }
    }

    /// The device line as the popup says it (`None`: set by hand).
    #[must_use]
    pub fn line(&self) -> Option<String> {
        let kind = self.kind?;
        let mut s = format!("{} {}", kind.word(), self.name.trim());
        for (k, v) in &self.values {
            let v = v.trim();
            if !v.is_empty() {
                s.push(' ');
                s.push_str(k);
                s.push('=');
                s.push_str(v);
            }
        }
        Some(s)
    }

    /// `text`, the project's `plant.sim`, with what the popup says.
    ///
    /// # Errors
    /// What the file would not take, in its words.
    pub fn apply(&self, text: &str) -> Result<String, String> {
        let mode = self.mode()?;
        let line = self.line().unwrap_or_default();
        // A rename replaces the old line where it stands.
        let at = self.old_name.as_deref().unwrap_or(self.name.trim());
        let mut out = helmsman_sim::set_device(text, at, &line);
        out = helmsman_sim::set_mode(&out, &self.input, mode);
        let (_, before) = helmsman_sim::parse(text);
        let (_, after) = helmsman_sim::parse(&out);
        if let Some(p) = after.iter().find(|p| !before.iter().any(|b| b.msg == p.msg)) {
            return Err(p.msg.clone());
        }
        Ok(out)
    }

    /// What the chosen behaviour does, in a sentence.
    #[must_use]
    pub fn explain(&self) -> String {
        match self.behaviour {
            Behaviour::Normal => match self.kind {
                Some(k) => format!("{} reads what the {} does.", self.input, k.word()),
                None => format!("{} is set by hand, as before.", self.input),
            },
            Behaviour::Abnormal => faults(self.is_bool)
                .iter()
                .find(|(w, _, _)| *w == self.fault)
                .map_or_else(String::new, |(_, about, _)| (*about).to_string()),
            Behaviour::Absent => {
                let ok = format!("{}_ok", self.input);
                if self.has_ok {
                    format!(
                        "The device is lost: the program runs on with {} at its last value, and {ok} reads FALSE.",
                        self.input
                    )
                } else {
                    format!(
                        "The device is lost: the program is held, its outputs off, as on a box. Declare {ok} : BOOL as an input to handle the loss of {} in the program.",
                        self.input
                    )
                }
            }
        }
    }

    /// Draws it; what was pressed, if anything.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        chrome: &mut Chrome,
        spec: &Spec,
        outputs: &[(String, bool)],
    ) -> Option<Act> {
        let mut act = None;
        let m = crate::theme::near_pointer(ctx, "sim-device").show(ctx, |ui| {
            ui.set_max_width(440.0);
            let t = theme(ui);
            ui.label(RichText::new(&self.input).strong().size(18.0))
                .tip("The program input this simulates.");
            ui.label(RichText::new("Simulate this input").color(t.muted));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Device")
                    .tip("What stands behind this input on the machine.");
                let shown = self.kind.map_or("set by hand", Kind::word);
                let mut pick: Option<Option<Kind>> = None;
                let r = egui::ComboBox::from_id_salt("sim-device-kind")
                    .selected_text(shown)
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.kind.is_none(), "set by hand")
                            .tip("No device: the input's control sets it, as without a plant.")
                            .clicked()
                        {
                            pick = Some(None);
                        }
                        for k in Kind::ALL {
                            let fits = k
                                .keys()
                                .iter()
                                .any(|key| key.role == if self.is_bool { Role::InBool } else { Role::InNum });
                            if fits
                                && ui
                                    .selectable_label(self.kind == Some(k), k.word())
                                    .tip(k.about())
                                    .clicked()
                            {
                                pick = Some(Some(k));
                            }
                        }
                    });
                chrome.hit("sim device kind", r.response.rect);
                if let Some(k) = pick {
                    let key = k
                        .and_then(|k| {
                            let want = if self.is_bool { Role::InBool } else { Role::InNum };
                            let (g, gk) = guess(&self.input, self.is_bool);
                            if g == k {
                                Some(gk)
                            } else {
                                k.keys().iter().find(|x| x.role == want).map(|x| x.name)
                            }
                        })
                        .unwrap_or("");
                    self.set_kind(k, key, spec, outputs);
                }
            });
            if let Some(k) = self.kind {
                ui.label(RichText::new(k.about()).color(t.muted));
                ui.horizontal_wrapped(|ui| {
                    ui.label("Name").tip("The device's name in plant.sim.");
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(140.0))
                        .tip("Letters, digits and _.");
                });
                egui::Grid::new("sim-device-keys").num_columns(2).show(ui, |ui| {
                    for (key, v) in &mut self.values {
                        let Some(spec_key) = k.keys().iter().find(|x| x.name == key) else {
                            continue;
                        };
                        ui.label(key.as_str()).tip(spec_key.about);
                        let hint = if spec_key.role.is_port() {
                            if spec_key.required { "required" } else { "none" }
                        } else {
                            spec_key.default
                        };
                        ui.add(egui::TextEdit::singleline(v).hint_text(hint).desired_width(180.0))
                            .tip(spec_key.about);
                        ui.end_row();
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                for (b, word, tip) in [
                    (Behaviour::Normal, "Normal", "It behaves as the device does."),
                    (
                        Behaviour::Abnormal,
                        "Abnormal",
                        "Stuck, drifting, noisy, out of range, slow or chattering.",
                    ),
                    (
                        Behaviour::Absent,
                        "Absent",
                        "The device is lost, as when its link is down.",
                    ),
                ] {
                    let r = ui
                        .add(
                            egui::Button::selectable(self.behaviour == b, RichText::new(word).strong())
                                .min_size(Vec2::new(0.0, TAP * 0.7)),
                        )
                        .tip(tip);
                    chrome.hit(format!("sim {}", word.to_ascii_lowercase()), r.rect);
                    if r.clicked() {
                        self.behaviour = b;
                    }
                }
            });
            if self.behaviour == Behaviour::Abnormal {
                ui.horizontal_wrapped(|ui| {
                    let list = faults(self.is_bool);
                    egui::ComboBox::from_id_salt("sim-fault")
                        .selected_text(self.fault.as_str())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for (w, about, _) in list {
                                if ui.selectable_label(self.fault == *w, *w).tip(*about).clicked() {
                                    (*w).clone_into(&mut self.fault);
                                    self.fault_value.clear();
                                }
                            }
                        });
                    let hint = list
                        .iter()
                        .find(|(w, _, _)| *w == self.fault)
                        .map_or("", |(_, _, h)| *h);
                    if !hint.is_empty() {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.fault_value)
                                .hint_text(hint)
                                .desired_width(90.0),
                        )
                        .tip("The value the behaviour takes.");
                    }
                });
            }
            ui.label(self.explain());
            if let Some(e) = &self.error {
                ui.colored_label(t.ink(egui::Color32::from_rgb(255, 110, 100)), e);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let yes = ui
                    .add(egui::Button::new(RichText::new("Apply").strong()).min_size(tall))
                    .tip(
                        "Writes it into the project's plant.sim; the simulation runs it from the next scan.",
                    );
                chrome.hit("sim apply", yes.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Changes nothing.");
                if yes.clicked() {
                    act = Some(Act::Apply);
                } else if no.clicked() {
                    act = Some(Act::Close);
                }
            });
        });
        chrome.hit("sim popup", m.response.rect);
        if act.is_none() && m.should_close() {
            act = Some(Act::Close);
        }
        act
    }
}
