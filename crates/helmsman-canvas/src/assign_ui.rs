//! The drawing of `crate::assign`: the menu items on a program port's row
//! and the question that lists what to assign it to (or what feeds a
//! device channel). The question is the Devices tab's own state
//! ([`DevicesUi::assigning`]) so that the I/O tab, the diagram and the
//! Devices tab all open the same one; its answer goes to the view, which
//! writes io.map.

use eframe::egui::{self, RichText, Vec2};

use crate::assign::{AssignAct, PortMenu, channels_for, point_label, ports_for};
use crate::device_form::Dir;
use crate::devices::DevicesUi;
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

/// What the question is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A program port: which free device channel does it take?
    Port(String),
    /// A device channel: which unassigned program port does it feed?
    Channel { device: String, point: String },
}

/// What a menu on a port asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortAsk {
    Assign,
    Back,
}

/// The assignment items of a context menu on a program port: "Assign to
/// device I/O…", and "Back to simulated I/O" when the port is on a device.
pub fn port_items(ui: &mut egui::Ui, chrome: &mut Chrome, m: &PortMenu) -> Option<PortAsk> {
    let mut asked = None;
    let tip = m.assign.as_ref().err().map_or(
        "Pick a free channel of a device, of the same direction and type, added to the project or not yet. Its line in io.map is named for this variable.",
        String::as_str,
    );
    if crate::devices::menu_item(
        ui,
        chrome,
        "menu assign port",
        "Assign to device I/O…",
        m.assign.is_ok(),
        tip,
    ) {
        asked = Some(PortAsk::Assign);
    }
    if let Some(back) = &m.back {
        let tip = back.as_ref().err().map_or(
            "Puts this back on a simulated input or output. The device channel goes back under + Add I/O, as Remove leaves it.",
            String::as_str,
        );
        if crate::devices::menu_item(
            ui,
            chrome,
            "menu back to simulated",
            "Back to simulated I/O",
            back.is_ok(),
            tip,
        ) {
            asked = Some(PortAsk::Back);
        }
    }
    asked
}

/// "Chat about this" asked on rows of the I/O tab ([`crate::chat_about`]).
#[derive(Debug, Default)]
pub struct RowChat {
    /// As many things are collected as one instruction carries.
    pub full: bool,
    /// The points it was chosen on this frame.
    pub asked: Vec<String>,
}

/// A right-click menu on a point's row or card in the I/O tab: `anchor`
/// is what the row draws. It offers "Chat about this" (pushed on `chat`)
/// and, for a program port, what `menus` holds for it: a choice is pushed
/// on `asks` as (port, what it asked for). The row is a hit for the tests
/// and the tour, by its point's name.
pub fn row_menu(
    anchor: &egui::Response,
    chrome: &mut Chrome,
    menus: &[PortMenu],
    name: &str,
    asks: &mut Vec<(String, PortAsk)>,
    chat: &mut RowChat,
) {
    chrome.hit(format!("io row {}", name.to_ascii_lowercase()), anchor.rect);
    let m = menus.iter().find(|m| m.port.eq_ignore_ascii_case(name));
    anchor.context_menu(|ui| {
        let tip = if chat.full {
            crate::chat_about::FULL
        } else {
            "Puts this point and where the code declares it in the Describe tab, with what drives it and what it drives. Nothing is sent until you type what to ask and send it."
        };
        if crate::devices::menu_item(ui, chrome, "menu chat about", "Chat about this", !chat.full, tip) {
            chat.asked.push(name.to_string());
        }
        if let Some(m) = m
            && let Some(a) = port_items(ui, chrome, m)
        {
            asks.push((m.port.clone(), a));
        }
    });
}

impl DevicesUi {
    /// What a right-click on each program port offers, from the map, the
    /// program and the devices' channels as the tab holds them now.
    #[must_use]
    pub fn port_menus(&self, write: &Result<(), String>) -> Vec<PortMenu> {
        crate::assign::menus(&self.map, &self.program, &|d| self.channels_of(d), write)
    }

    /// The question, while one is open: a box where the pointer was, with
    /// a button for each choice. Returns what to write, once one is picked.
    pub fn assign_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<AssignAct> {
        let ask = self.assigning.clone()?;
        let mut out = None;
        let mut done = false;
        let (title, choices): (String, Vec<(String, String, String, AssignAct)>) = match &ask {
            Ask::Port(port) => {
                let p = self.program.iter().find(|p| p.name.eq_ignore_ascii_case(port));
                let list = p.map_or_else(Vec::new, |p| {
                    channels_for(&self.map, &self.program, &|d| self.channels_of(d), p)
                });
                (
                    format!("Assign {port} to a device channel"),
                    list.into_iter()
                        .map(|c| {
                            let tip = match &c.name {
                                Some(n) => format!(
                                    "Renames {n}'s line in io.map to {port} and takes {port}'s simulated line out."
                                ),
                                None => format!(
                                    "Adds this channel to io.map as {port}, after {}'s other lines, and takes {port}'s simulated line out.",
                                    c.device
                                ),
                            };
                            (
                                format!("assign channel {} {}", c.device, c.point),
                                format!("{}   {}", c.label(), c.name.as_deref().unwrap_or("not added yet")),
                                tip,
                                AssignAct::Assign {
                                    port: port.clone(),
                                    device: c.device,
                                    point: c.point,
                                },
                            )
                        })
                        .collect(),
                )
            }
            Ask::Channel { device, point } => {
                let list = ports_for(&self.map, &self.program, &|d| self.channels_of(d), device, point);
                let dir = list.first().map_or("input or output", |p| match p.dir {
                    Dir::In => "input",
                    Dir::Out => "output",
                });
                (
                    format!("Assign {device}, {} to a program {dir}", point_label(point)),
                    list.into_iter()
                        .map(|p| {
                            let tip = format!(
                                "Renames this channel's line in io.map to {} and takes {}'s simulated line out.",
                                p.name, p.name
                            );
                            (
                                format!("assign port {}", p.name),
                                format!("{}   {}", p.name, p.ty.word()),
                                tip,
                                AssignAct::Assign {
                                    port: p.name,
                                    device: device.clone(),
                                    point: point.clone(),
                                },
                            )
                        })
                        .collect(),
                )
            }
        };
        let m = crate::theme::near_pointer(ctx, "assign-ask").show(ctx, |ui| {
            let t = theme(ui);
            ui.set_width(420.0);
            ui.add(egui::Label::new(RichText::new(title).strong().size(20.0)).wrap());
            ui.add_space(6.0);
            if choices.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new("Nothing is free for this any more: the map changed.").color(t.muted),
                    )
                    .wrap(),
                )
                .tip("The channels and ports are read from io.map and the program as they are now.");
            }
            for (key, text, tip, act) in choices {
                let r = ui
                    .add(
                        egui::Button::new(text)
                            .wrap()
                            .min_size(Vec2::new(ui.available_width(), TAP * 0.7)),
                    )
                    .tip(tip);
                chrome.hit(key, r.rect);
                if r.clicked() {
                    out = Some(act);
                    done = true;
                }
            }
            ui.add_space(10.0);
            let no = ui
                .add(egui::Button::new("Cancel").min_size(Vec2::new(0.0, TAP * 0.7)))
                .tip("Leaves io.map as it is.");
            chrome.hit("assign cancel", no.rect);
            done |= no.clicked();
        });
        chrome.hit("assign form", m.response.rect);
        if done || crate::theme::escaped(&m) {
            self.assigning = None;
        }
        out
    }
}
