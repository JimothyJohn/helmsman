//! The Devices tab's "Plant states" section (TODO.md `sim-feedback-devices`):
//! the quantities a project's plant model keeps for the machine it stands
//! in for, marked `{state}` in the Structured Text
//! (`helmsman_project::plant`), each with the value the simulation holds,
//! the map `in` signal that measures it on the real machine when there is
//! one, and otherwise the kind of sensor that would. A right-click on an
//! unmeasured state adds that sensor as an `in` line of the map, the way
//! the tab adds a device. The same list, as text, is the agent's context.

use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use helmsman_iec::Value;
use helmsman_project::iomap::MapText;
use helmsman_project::plant;

use crate::editor::Chrome;
use crate::st_view::{TAP, show};
use crate::theme::theme;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// The map signal that stands for a state on the real machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub signal: String,
    pub device: String,
    pub point: String,
    /// What the signal reads now, when known.
    pub value: Option<Value>,
}

/// One plant-model state variable, as the tab lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlantState {
    pub name: String,
    /// The declared type (`REAL`, `INT`, `BOOL`, ...).
    pub ty: String,
    pub unit: Option<String>,
    pub pou: String,
    pub file: String,
    pub line: usize,
    /// What the model holds now (the simulation; `None` on a box, where
    /// no plant model runs).
    pub value: Option<Value>,
    pub measured_by: Option<Measurement>,
    /// The sensor kind that would measure it, from `plant::suggest`.
    pub suggestion: &'static str,
}

/// Every `{state}` of `files` (name, text), with its measurement from
/// `map` (an `in` signal named as `plant::measurement_names` says, on a
/// device), its model value from `value(name)` and the measuring signal's
/// value from `live(signal)`.
pub fn states(
    files: &[(&str, &str)],
    map: &MapText,
    value: &dyn Fn(&str) -> Option<Value>,
    live: &dyn Fn(&str) -> Option<Value>,
) -> Vec<PlantState> {
    let mut out = Vec::new();
    for (file, text) in files {
        if !file.ends_with(".st") {
            continue;
        }
        for v in plant::skim(text) {
            let measured_by = plant::measurement_names(&v).iter().find_map(|want| {
                map.signals
                    .iter()
                    .find(|s| s.dir == "in" && s.name.eq_ignore_ascii_case(want))
                    .map(|s| Measurement {
                        signal: s.name.clone(),
                        device: s.device.clone(),
                        point: s.point.clone(),
                        value: live(&s.name),
                    })
            });
            out.push(PlantState {
                value: value(&v.name),
                measured_by,
                suggestion: plant::suggest(&v),
                name: v.name,
                ty: v.ty,
                unit: v.unit,
                pou: v.pou,
                file: (*file).to_string(),
                line: v.line,
            });
        }
    }
    out
}

/// A state's value from a traced scan: the scalar whose path ends in the
/// variable's name (`PLANT.LEVEL_M`), the first when several instances
/// hold one.
#[must_use]
pub fn value_in_trace(trace: &helmsman_iec::trace::Trace, name: &str) -> Option<Value> {
    let want = name.to_ascii_uppercase();
    trace.values().find_map(|(path, v)| {
        let last = path.rsplit('.').next().unwrap_or(path);
        (last.eq_ignore_ascii_case(&want) || path.eq_ignore_ascii_case(&want)).then_some(v)
    })
}

fn with_unit(v: Option<Value>, unit: Option<&str>) -> String {
    match (v, unit) {
        (None, _) => String::new(),
        (Some(v), Some(u)) => format!(" = {} {u}", show(Some(v))),
        (Some(v), None) => format!(" = {}", show(Some(v))),
    }
}

/// The list as text, the agent's context.
#[must_use]
pub fn describe(states: &[PlantState]) -> String {
    use std::fmt::Write as _;
    if states.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "plant states (quantities the simulation's plant model keeps; a real machine measures them):\n",
    );
    for s in states {
        let _ = write!(out, "  {} {}", s.name, s.ty);
        if let Some(u) = &s.unit {
            let _ = write!(out, " unit={u}");
        }
        out.push_str(&with_unit(s.value, None));
        let _ = write!(out, "  in {} ({}:{})", s.pou, s.file, s.line);
        match &s.measured_by {
            Some(m) => {
                let _ = write!(
                    out,
                    "  measured by {} ({} {}){}",
                    m.signal,
                    m.device,
                    m.point,
                    with_unit(m.value, None)
                );
            }
            None => {
                let _ = write!(out, "  not measured; {} would", s.suggestion);
            }
        }
        out.push('\n');
    }
    out
}

/// A map device the sensor form offers: its name and driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    pub name: String,
    pub driver: String,
}

/// The map's type for a declared ST type.
fn map_type(ty: &str) -> &'static str {
    match ty {
        "BOOL" => "bool",
        "REAL" | "LREAL" => "real",
        _ => "int",
    }
}

/// A point to start from, in the device's own syntax.
fn point_hint(driver: &str, ty: &str) -> &'static str {
    match (driver, ty) {
        ("box", "bool") => "di:0",
        ("box", _) => "i2c:1:0x48:0:i16",
        ("enip-io", "bool") => "di:0",
        ("enip-io", _) => "in:2:u32",
        (_, "bool") => "di:0",
        (_, "real") => "ir:0:f32",
        _ => "ir:0:i16",
    }
}

/// The "Add a sensor" form's fields: one `in` line for a state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSignal {
    /// The state it measures.
    pub state: String,
    /// The signal's name in the map (the state's, so the program can read
    /// it under the same name).
    pub name: String,
    /// `bool`, `int` or `real`.
    pub ty: String,
    pub device: String,
    pub point: String,
    pub unit: String,
    pub devices: Vec<DeviceChoice>,
    pub error: Option<String>,
}

impl NewSignal {
    /// A form for `s` on the first of `devices` (the box's own channels
    /// when the map declares none).
    #[must_use]
    pub fn for_state(s: &PlantState, devices: &[DeviceChoice]) -> Self {
        let mut devices = devices.to_vec();
        if devices.is_empty() {
            devices.push(DeviceChoice {
                name: "box".into(),
                driver: "box".into(),
            });
        }
        let ty = map_type(&s.ty).to_string();
        let first = devices[0].clone();
        Self {
            state: s.name.clone(),
            name: s.name.clone(),
            ty: ty.clone(),
            point: point_hint(&first.driver, &ty).to_string(),
            device: first.name,
            unit: s.unit.clone().unwrap_or_default(),
            devices,
            error: None,
        }
    }

    /// The driver of the chosen device.
    fn driver(&self) -> String {
        self.devices
            .iter()
            .find(|d| d.name == self.device)
            .map_or_else(|| "box".to_string(), |d| d.driver.clone())
    }

    /// The `in` line, or why not.
    ///
    /// # Errors
    /// A name that is not an identifier, a point or unit with blanks, an
    /// unknown device.
    pub fn line(&self) -> Result<String, String> {
        let name = self.name.trim();
        if !helmsman_project::iomap::device_name_ok(name) && !is_ident(name) {
            return Err(format!(
                "a signal name is letters, digits and _: {name:?} will not do"
            ));
        }
        if !self.devices.iter().any(|d| d.name == self.device) {
            return Err("pick a device of the map".into());
        }
        let point = self.point.trim();
        if point.is_empty() || point.split_whitespace().count() != 1 {
            return Err("the point is one word in the device's syntax, like di:4 or ir:16:u32".into());
        }
        let unit = self.unit.trim();
        if unit.split_whitespace().count() > 1 || unit.len() > 16 || !unit.is_ascii() {
            return Err("a unit is one word of up to 16 ASCII characters".into());
        }
        let mut line = format!("in  {name} {} {} {point}", self.ty, self.device);
        if !unit.is_empty() {
            line.push_str(&format!(" unit={unit}"));
        }
        line.push('\n');
        Ok(line)
    }
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && c.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The section's state: what to list (the view sets it each frame), the
/// map's devices for the form, the form itself.
#[derive(Debug, Default)]
pub struct PlantUi {
    pub states: Vec<PlantState>,
    pub devices: Vec<DeviceChoice>,
    pub adding: Option<NewSignal>,
    folded: bool,
    /// Whether the plant model runs here (the simulation) or only reads
    /// as text (a box).
    pub simulated: bool,
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

impl PlantUi {
    /// The section, when the project has any state; nothing otherwise.
    /// `write`: why a sensor cannot be added, if it cannot.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, write: &Result<(), String>) {
        if self.states.is_empty() {
            return;
        }
        let t = theme(ui);
        ui.add_space(6.0);
        ui.separator();
        let arrow = if self.folded { "▸" } else { "▾" };
        let head = ui.add(
            egui::Label::new(
                RichText::new(format!("{arrow} Plant states"))
                    .strong()
                    .color(t.muted),
            )
            .truncate()
            .sense(Sense::click()),
        );
        chrome.hit("plant states", head.rect);
        let hover = if self.simulated {
            "What the plant model keeps for the machine it stands in for. On the real machine each is measured by a sensor: the map's in signal of the same name, or one you add here."
        } else {
            "The project's plant model (simulated only on a box): each quantity and the sensor that measures it on this machine."
        };
        if head.on_hover_text(hover).clicked() {
            self.folded = !self.folded;
        }
        if self.folded {
            return;
        }
        let mut add = None;
        for s in &self.states {
            let (dot, tail) = match &s.measured_by {
                Some(m) => (
                    RichText::new("●").color(t.accent),
                    format!(
                        "← {} {}{}",
                        m.device,
                        m.point,
                        with_unit(m.value, s.unit.as_deref())
                    ),
                ),
                None => (
                    RichText::new("○").color(t.muted),
                    format!("not measured · {}?", s.suggestion),
                ),
            };
            let value = if self.simulated {
                with_unit(s.value, s.unit.as_deref())
            } else {
                String::new()
            };
            let text = format!("{}{value}  {tail}", s.name);
            let r = ui.horizontal(|ui| {
                ui.label(dot);
                ui.add(
                    egui::Label::new(RichText::new(text).monospace())
                        .truncate()
                        .sense(Sense::click()),
                )
            });
            let r = r.inner;
            chrome.hit(format!("state {}", s.name), r.rect);
            let mut hover = format!("{} in {} ({}:{})", s.ty, s.pou, s.file, s.line);
            if let Some(u) = &s.unit {
                hover.push_str(&format!(", in {u}"));
            }
            match &s.measured_by {
                Some(m) => hover.push_str(&format!(
                    "\nmeasured by the signal {} on {} {}",
                    m.signal, m.device, m.point
                )),
                None => hover.push_str(&format!(
                    "\nnot measured on the machine yet: {} would. Right-click to add one.",
                    s.suggestion
                )),
            }
            if !self.simulated {
                hover.push_str("\nsimulated only: no plant model runs on a box");
            }
            let can_add = write.is_ok() && s.measured_by.is_none();
            r.on_hover_text(hover).context_menu(|ui| {
                if menu_item(ui, chrome, "menu add sensor", "Add a sensor…", can_add) {
                    add = Some(NewSignal::for_state(s, &self.devices));
                }
                if s.measured_by.is_some() {
                    ui.label(RichText::new("already measured").small().color(t.muted));
                } else if let Err(why) = write {
                    ui.label(RichText::new(why).small().color(t.muted));
                }
            });
        }
        if add.is_some() {
            self.adding = add;
        }
    }

    /// The "Add a sensor" form as a box in the middle of the screen (the
    /// view draws it after the Editor, like the device form). Returns the
    /// `in` line to append once Add is pressed with fields that pass.
    pub fn form_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<String> {
        let mut f = self.adding.clone()?;
        let mut out = None;
        let mut done = false;
        let m = egui::Modal::new(egui::Id::unique("sensor-new")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(
                RichText::new(format!("A sensor for {}", f.state))
                    .strong()
                    .size(16.0),
            );
            ui.add_space(8.0);
            ui.label("Signal name (the program reads it under this name)");
            let n = ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY));
            chrome.hit("sensor name", n.rect);
            ui.add_space(6.0);
            ui.label("Device");
            ui.horizontal_wrapped(|ui| {
                let choices = f.devices.clone();
                for d in &choices {
                    let r = ui.add(egui::Button::selectable(f.device == d.name, &d.name));
                    chrome.hit(format!("sensor device {}", d.name), r.rect);
                    if r.clicked() && f.device != d.name {
                        f.device = d.name.clone();
                        f.point = point_hint(&d.driver, &f.ty).to_string();
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(format!("Point ({} syntax; a {} signal)", f.driver(), f.ty));
            let p = ui.add(egui::TextEdit::singleline(&mut f.point).desired_width(f32::INFINITY));
            chrome.hit("sensor point", p.rect);
            ui.add_space(6.0);
            ui.label("Unit (optional)");
            let u = ui.add(
                egui::TextEdit::singleline(&mut f.unit)
                    .hint_text("m")
                    .desired_width(f32::INFINITY),
            );
            chrome.hit("sensor unit", u.rect);
            if let Some(err) = &f.error {
                ui.add_space(4.0);
                ui.label(RichText::new(err).color(RED));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let ok = ui.add(egui::Button::new(RichText::new("Add").strong()).min_size(tall));
                chrome.hit("sensor ok", ok.rect);
                let no = ui.add(egui::Button::new("Cancel").min_size(tall));
                chrome.hit("sensor cancel", no.rect);
                if ok.clicked() {
                    match f.line() {
                        Ok(line) => {
                            out = Some(line);
                            done = true;
                        }
                        Err(e) => f.error = Some(e),
                    }
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("sensor form", m.response.rect);
        if done || m.should_close() {
            self.adding = None;
        } else {
            self.adding = Some(f);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helmsman_project::iomap;

    const MODEL: &str = "FUNCTION_BLOCK plant_model
VAR
  level_m : REAL := 1.5;  {state unit=m}
  temp_c : REAL;  {state unit=degC}
  parts : INT; {state}
END_VAR
END_FUNCTION_BLOCK
";

    fn map() -> MapText {
        iomap::skim(
            "device e1213 modbus-tcp 10.0.0.57:502 unit=1\n\
             in  level_raw real e1213 ir:0:i16 unit=raw\n\
             in  start_pb bool box di:0\n",
        )
    }

    #[test]
    fn states_join_the_measurement_by_name_and_suggest_for_the_rest() {
        let files = [("plant_model.st", MODEL), ("io.map", "not st")];
        let value = |n: &str| (n == "level_m").then_some(Value::Real(1.52));
        let live = |n: &str| (n == "level_raw").then_some(Value::Real(12420.0));
        let s = states(&files, &map(), &value, &live);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].name, "level_m");
        assert_eq!(s[0].value, Some(Value::Real(1.52)));
        let m = s[0].measured_by.as_ref().expect("level_raw stands for level_m");
        assert_eq!(
            (m.signal.as_str(), m.device.as_str(), m.point.as_str()),
            ("level_raw", "e1213", "ir:0:i16")
        );
        assert_eq!(m.value, Some(Value::Real(12420.0)));
        assert_eq!(s[1].measured_by, None);
        assert_eq!(s[1].suggestion, "RTD or thermocouple");
        assert_eq!(s[2].suggestion, "counter input (photo eye or proximity switch)");
        let text = describe(&s);
        assert!(text.contains("level_m REAL unit=m = 1.52  in plant_model (plant_model.st:3)  measured by level_raw (e1213 ir:0:i16) = 12420"), "{text}");
        assert!(text.contains("temp_c REAL unit=degC  in plant_model (plant_model.st:4)  not measured; RTD or thermocouple would"), "{text}");
        assert_eq!(describe(&[]), "");
    }

    #[test]
    fn the_sensor_form_writes_one_in_line_and_refuses_bad_fields() {
        let files = [("plant_model.st", MODEL)];
        let s = states(&files, &map(), &|_| None, &|_| None);
        let devices = [DeviceChoice {
            name: "e1213".into(),
            driver: "modbus-tcp".into(),
        }];
        let mut f = NewSignal::for_state(&s[1], &devices);
        assert_eq!(
            (
                f.name.as_str(),
                f.ty.as_str(),
                f.device.as_str(),
                f.point.as_str(),
                f.unit.as_str()
            ),
            ("temp_c", "real", "e1213", "ir:0:f32", "degC")
        );
        assert_eq!(f.line().unwrap(), "in  temp_c real e1213 ir:0:f32 unit=degC\n");
        f.point = "ir:4:i16".into();
        f.unit.clear();
        assert_eq!(f.line().unwrap(), "in  temp_c real e1213 ir:4:i16\n");
        f.point = "ir 4".into();
        assert!(f.line().is_err());
        f.point = "ir:4".into();
        f.name = "1bad".into();
        assert!(f.line().is_err());
        f.name = "temp_c".into();
        f.device = "nowhere".into();
        assert!(f.line().is_err());
        f.device = "e1213".into();
        f.unit = "degrees celsius".into();
        assert!(f.line().is_err());
        // No device in the map: the box's own channels.
        let g = NewSignal::for_state(&s[2], &[]);
        assert_eq!(g.line().unwrap(), "in  parts int box i2c:1:0x48:0:i16\n");
        let b = NewSignal::for_state(
            &PlantState {
                name: "door".into(),
                ty: "BOOL".into(),
                unit: None,
                pou: "p".into(),
                file: "plant_model.st".into(),
                line: 1,
                value: None,
                measured_by: None,
                suggestion: "a switch",
            },
            &[],
        );
        assert_eq!(b.line().unwrap(), "in  door bool box di:0\n");
    }
}
