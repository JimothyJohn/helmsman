//! Drawing the "New device" form and the Rename question (the model is
//! `device_form.rs`): a box in the middle of the screen, whichever tab is
//! up (the view draws it each frame after the Editor, as the Files tab's
//! confirmations are). The forms hold a device's settings and none of its
//! inputs and outputs: those are added one at a time under the device on
//! the Devices tab (`device_io.rs`; Nick, 2026-11-15).

use eframe::egui::{self, RichText, Vec2};

use crate::device_form::{self, Kind, PROFILES, check_settings_for, param, schema, set_param};
use crate::devices::{DeviceAct, DevicesUi, NewDevice, PROTOCOLS, QuickAdd};
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

const RED: egui::Color32 = egui::Color32::from_rgb(255, 110, 100);
/// The form's width: room for the longest setting and its unit.
const WIDTH: f32 = 520.0;

impl DevicesUi {
    /// Opens the form on `d`.
    pub fn open(&mut self, d: NewDevice) {
        self.adding = Some(d);
    }

    /// The form. Returns the wiring once Add is pressed with rows that pass.
    pub fn form_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        let mut d = self.adding.clone()?;
        let mut out = None;
        let mut done = false;
        let program = self.program.clone();
        let eds_files = self.eds_files.clone();
        let m = crate::theme::near_pointer(ctx, "device-new").show(ctx, |ui| {
            // A fixed width: the form is centred, so a width that follows
            // its content moves Add under the pointer as fields change.
            ui.set_min_width(WIDTH);
            ui.set_max_width(WIDTH);
            let title = match (&d.editing, &d.eds) {
                (Some(n), _) => format!("Edit {n}"),
                (None, Some(f)) => format!("New device from {f}"),
                (None, None) => "New device".to_string(),
            };
            ui.label(RichText::new(title).strong().size(20.0));
            ui.add_space(8.0);
            let simple = d.editing.is_some() && !chrome.advanced;
            fields(ui, chrome, &mut d, &eds_files, simple);
            if d.editing.is_none() {
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        RichText::new("Its inputs and outputs are added under it on the Devices tab, with + Add I/O.")
                            .color(theme(ui).muted),
                    )
                    .wrap(),
                );
            }
            if let Some(err) = &d.error {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(err).color(crate::theme::theme(ui).ink(RED))).wrap());
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let (word, tip) = if d.editing.is_some() {
                    ("Save", "Writes these settings on the device's line in io.map. Its inputs and outputs stay as they are, under its new name if it got one.")
                } else {
                    ("Add", "Adds the device to io.map. Its inputs and outputs are added under it on the Devices tab.")
                };
                let ok = ui
                    .add(egui::Button::new(RichText::new(word).strong()).min_size(tall))
                    .tip(tip);
                chrome.hit("device ok", ok.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Closes the form. Nothing is added.");
                chrome.hit("device cancel", no.rect);
                if ok.clicked() {
                    // An edit's own lines are not "wired already".
                    let map = match &d.editing {
                        Some(old) => device_form::map_without(&self.map, old),
                        None => self.map.clone(),
                    };
                    // Spaces are stored as underscores.
                    d.name = d.name.trim().replace(' ', "_");
                    let taken = map.devices.iter().any(|x| x.name == d.name);
                    let on_skipper = !self.skipper.is_empty();
                    let w = check_settings_for(d.driver(), &d.params, d.editing.is_some())
                        .and_then(|()| {
                            if on_skipper && d.name.len() > crate::skipper_map::NAME_MAX {
                                Err(format!(
                                    "A Skipper names a device in at most {} characters.",
                                    crate::skipper_map::NAME_MAX
                                ))
                            } else if taken {
                                Err(format!("A device called {} is in io.map already.", d.name))
                            } else {
                                d.line()
                            }
                        })
                        .and_then(|line| device_form::wiring(&d.name, line, &d.points, &program, &map))
                        .map(|mut w| {
                            w.replaces.clone_from(&d.editing);
                            w
                        });
                    match w {
                        Ok(w) => {
                            out = Some(DeviceAct::Add(w));
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
        if done || crate::theme::escaped(&m) {
            self.adding = None;
        } else {
            self.adding = Some(d);
        }
        out
    }

    /// The quick form (add by click, PROTOCOLS.md step 3): the device's
    /// name, the channels it will get once the profile came, Add or
    /// Cancel. Returns what Add writes once the name passes.
    pub fn quick_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        // Scan and add fills the form itself: its Add, nothing to show.
        if let Some(add) = self.add_next.take() {
            return Some(DeviceAct::Quick(add));
        }
        if self.add_all.is_some() {
            return None;
        }
        let mut q = self.quick.clone()?;
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "device-quick").show(ctx, |ui| {
            ui.set_min_width(WIDTH * 0.75);
            ui.set_max_width(WIDTH * 0.75);
            let what = format!("{} {}", q.vendor, q.model).trim().to_string();
            ui.label(RichText::new(format!("Add {what}")).strong().size(20.0));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Name").tip("What the program calls the device. A channel you add is named after it: rio and DI-04 give rio_di_04.");
                let r = ui
                    .add(egui::TextEdit::singleline(&mut q.name).desired_width(200.0))
                    .tip("The device's name in the program: a letter, then letters, digits or _.");
                chrome.hit("quick name", r.rect);
            });
            ui.add_space(6.0);
            match &q.profile {
                None => {
                    ui.label(RichText::new("Reading its channels from the device library…").color(theme(ui).muted));
                }
                Some(Err(why)) => {
                    ui.add(egui::Label::new(RichText::new(why).color(theme(ui).ink(RED))).wrap());
                }
                Some(Ok((_, skim))) => {
                    let n = skim.channels.len();
                    let over = if skim.transports.is_empty() {
                        String::new()
                    } else {
                        format!(" over {}", skim.transports.join(", then "))
                    };
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{n} channel(s){over}. All of them are yours to use: add the ones the project needs under the device, with + Add I/O."
                            ))
                            .color(theme(ui).muted),
                        )
                        .wrap(),
                    )
                    .tip("Nothing is listed in the project until you add it. The box picks the protocol from the list, best first, and falls back by itself.");
                }
            }
            if let Some(err) = &q.error {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(err).color(theme(ui).ink(RED))).wrap());
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let ready = matches!(q.profile, Some(Ok(_)));
                let ok = ui
                    .add_enabled(ready, egui::Button::new(RichText::new("Add").strong()).min_size(tall))
                    .tip("Copies the device's profile into the project and adds the device to io.map. Its channels are added under it, one click each.");
                chrome.hit("quick ok", ok.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Closes the form. Nothing is added.");
                chrome.hit("quick cancel", no.rect);
                if ok.clicked() {
                    match quick_add(&q, &self.map, !self.skipper.is_empty()) {
                        Ok(a) => {
                            out = Some(DeviceAct::Quick(a));
                            done = true;
                        }
                        Err(e) => q.error = Some(e),
                    }
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("quick form", m.response.rect);
        if done || crate::theme::escaped(&m) {
            self.quick = None;
        } else {
            self.quick = Some(q);
        }
        out
    }

    /// The Rename question: the new name, what it changes (the view's
    /// [`RenamePlan`] in `rename_plan`), Rename or Cancel.
    pub fn rename_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        let (old, mut text) = self.renaming.clone()?;
        let plan = self.rename_plan.clone();
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "signal-rename").show(ctx, |ui| {
            ui.set_max_width(460.0);
            let t = theme(ui);
            ui.label(RichText::new(format!("Rename {old}")).strong().size(20.0));
            ui.add_space(8.0);
            let tip = "The new name, in io.map and in the program. A letter, then letters, digits or _.";
            ui.label("New name").tip(tip);
            let e = ui
                .add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY))
                .tip(tip);
            chrome.hit("rename name", e.rect);
            ui.add_space(6.0);
            let ready = match &plan {
                Some(Ok(p)) => {
                    ui.label(RichText::new("This changes:").color(t.muted));
                    for l in p.lines() {
                        ui.label(RichText::new(format!("  {l}")).monospace());
                    }
                    true
                }
                Some(Err(why)) if text != old => {
                    ui.add(
                        egui::Label::new(RichText::new(why).color(crate::theme::theme(ui).ink(RED))).wrap(),
                    );
                    false
                }
                _ => false,
            };
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let ok = ui
                    .add_enabled(
                        ready,
                        egui::Button::new(RichText::new("Rename").strong()).min_size(tall),
                    )
                    .tip("Renames it in every file listed and saves that as one revision.");
                chrome.hit("rename ok", ok.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Leaves the name as it is.");
                chrome.hit("rename cancel", no.rect);
                if ok.clicked()
                    && let Some(Ok(p)) = &plan
                {
                    out = Some(DeviceAct::Rename(p.clone()));
                    done = true;
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("rename form", m.response.rect);
        if done || crate::theme::escaped(&m) {
            self.renaming = None;
            self.rename_plan = None;
        } else {
            self.renaming = Some((old, text));
        }
        out
    }

    /// The Delete question on a device. Returns the device to delete once
    /// Delete is pressed.
    pub fn delete_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<DeviceAct> {
        let name = self.deleting.clone()?;
        let n = self.map.signals.iter().filter(|s| s.device == name).count();
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "device-delete").show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label(RichText::new(format!("Delete {name}?")).strong().size(20.0));
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(format!(
                    "Takes its line and its {n} input and output line(s) out of io.map. The program's variables stay; wire them to another device or the box's own channels."
                ))
                .wrap(),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let ok = ui
                    .add(egui::Button::new(RichText::new("Delete").strong().color(crate::theme::theme(ui).ink(RED))).min_size(tall))
                    .tip("Deletes the lines. Save keeps the change; Deploy runs it.");
                chrome.hit("delete ok", ok.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Keeps the device.");
                chrome.hit("delete cancel", no.rect);
                if ok.clicked() {
                    out = Some(DeviceAct::Delete(name.clone()));
                    done = true;
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("delete form", m.response.rect);
        if done || m.should_close() {
            self.deleting = None;
        }
        out
    }

    /// The plan for the rename being typed, from the project's files.
    pub fn plan_rename(&mut self, files: &[(String, String)]) {
        self.rename_plan = self
            .renaming
            .as_ref()
            .map(|(old, new)| device_form::rename_plan(files, old, new.trim()));
    }
}

/// Name, protocol, address, then the protocol's settings: one grid,
/// labels on the left, and the device's io.map line under it. `simple`
/// (an edit with Advanced off): the name alone; how the device is
/// reached stays as it is.
fn fields(ui: &mut egui::Ui, chrome: &mut Chrome, d: &mut NewDevice, eds_files: &[String], simple: bool) {
    let t = theme(ui);
    egui::Grid::new("device-fields")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            let name_tip = "What io.map calls the device: a letter, then letters, digits or _.";
            ui.label("Name").tip(name_tip);
            let n = ui
                .add(egui::TextEdit::singleline(&mut d.name).hint_text("rio").desired_width(300.0))
                .tip(name_tip);
            chrome.hit("device name", n.rect);
            ui.end_row();
            if simple {
                return;
            }
            ui.label("Protocol").tip("How the controller talks to the device.");
            ui.horizontal_wrapped(|ui| {
                for (k, (driver, title, _, _)) in PROTOCOLS.iter().enumerate() {
                    let r = ui
                        .add(egui::Button::selectable(d.protocol == k, *title))
                        .tip(format!("{title}, over Ethernet. Fills in its usual settings."));
                    chrome.hit(format!("device {driver}"), r.rect);
                    if r.clicked() {
                        d.set_protocol(k);
                    }
                }
            });
            ui.end_row();
            let hint = PROTOCOLS.get(d.protocol).map_or("", |p| p.2);
            let addr_tip = format!(
                "The device's IP address, e.g. {hint}, or its identity (mac=… or cip=…) so a new address never loses it."
            );
            ui.label("Address").tip(&addr_tip);
            let e = ui
                .add(egui::TextEdit::singleline(&mut d.endpoint).hint_text(hint).desired_width(300.0))
                .tip(&addr_tip);
            chrome.hit("device endpoint", e.rect);
            ui.end_row();
            let driver = d.driver();
            for s in schema(driver) {
                ui.label(s.label).tip(s.tip);
                let now = param(&d.params, s.key).unwrap_or("").to_string();
                match s.kind {
                    Kind::Num { unit, .. } => {
                        let mut v = now.clone();
                        ui.horizontal(|ui| {
                            let r = ui
                                .add(egui::TextEdit::singleline(&mut v).desired_width(110.0))
                                .tip(s.tip);
                            chrome.hit(format!("setting {}", s.key), r.rect);
                            if !unit.is_empty() {
                                ui.label(RichText::new(unit).color(t.muted));
                            }
                        });
                        if v != now {
                            d.params = set_param(&d.params, s.key, &v);
                        }
                    }
                    Kind::Profile | Kind::Eds => {
                        let choices: Vec<String> = match s.kind {
                            Kind::Profile => PROFILES.iter().map(ToString::to_string).collect(),
                            _ => eds_files.to_vec(),
                        };
                        let mut v = now.clone();
                        let r = egui::ComboBox::from_id_salt(("setting", s.key))
                            .width(220.0)
                            .selected_text(if v.is_empty() { "none" } else { v.as_str() })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut v, String::new(), "none")
                                    .tip("No value: the setting is left out of the line.");
                                for c in &choices {
                                    ui.selectable_value(&mut v, c.clone(), c).tip(s.tip);
                                }
                            })
                            .response
                            .tip(s.tip);
                        chrome.hit(format!("setting {}", s.key), r.rect);
                        if v != now {
                            d.params = set_param(&d.params, s.key, &v);
                            d.eds = param(&d.params, "eds").map(str::to_string);
                        }
                    }
                }
                ui.end_row();
            }
        });
    if simple {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "How it is reached (protocol, address, timing) is under Advanced on the Devices tab.",
                )
                .color(t.muted),
            )
            .wrap(),
        );
        return;
    }
    if let Err(e) = check_settings_for(d.driver(), &d.params, d.editing.is_some()) {
        ui.add(egui::Label::new(RichText::new(e).color(crate::theme::theme(ui).ink(RED))).wrap());
    }
    // The line io.map gets, read-only.
    // One line, cut at the edge: a wrap would change the centred form's
    // height under the pointer as the address is typed.
    let line = d.line().unwrap_or_else(|e| e);
    let line = line.trim_end();
    ui.add(egui::Label::new(RichText::new(line).monospace().color(t.muted)).truncate())
        .tip(format!(
            "The device's line in io.map, as these fields make it:\n{line}"
        ));
}

/// What the quick form's Add writes, or why not: the name an identifier
/// the map does not have yet, the profile as a project file, and the
/// device line `device NAME device:PROFILE IDENTITY`, or the address it
/// answered at when `at_address` (a Skipper's page: a Skipper does not
/// look a device up by its identity). No signal: every channel is in the
/// profile, and joins io.map when it is added under the device
/// (`device_io.rs`).
///
/// # Errors
/// The name, in words.
pub fn quick_add(
    q: &crate::devices::Quick,
    map: &helmsman_project::iomap::MapText,
    at_address: bool,
) -> Result<QuickAdd, String> {
    let name = q.name.trim();
    if !helmsman_project::iomap::device_name_ok(name) {
        return Err(format!(
            "A device name is a letter, then letters, digits or _ (not {name:?})."
        ));
    }
    if map.devices.iter().any(|d| d.name.eq_ignore_ascii_case(name)) {
        return Err(format!("A device called {name} is in io.map already."));
    }
    let Some(Ok((text, _))) = &q.profile else {
        return Err("The device's channels have not come yet.".into());
    };
    let stem = q
        .file
        .strip_suffix(helmsman_project::DEVICE_EXT)
        .ok_or_else(|| format!("{}: not a profile file name", q.file))?;
    Ok(QuickAdd {
        file: q.file.clone(),
        text: text.clone(),
        wiring: device_form::Wiring {
            device: name.to_string(),
            device_line: format!(
                "device {name} device:{stem} {}",
                if at_address { &q.address } else { &q.identity }
            ),
            signals: Vec::new(),
            replace: Vec::new(),
            declare: Vec::new(),
            replaces: None,
            extend: false,
        },
    })
}
