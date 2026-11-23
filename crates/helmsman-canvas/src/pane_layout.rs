//! Named arrangements of the Editor's panes (Nick, 2026-11-15: "reorder
//! tabs in the side pane and save the layouts as well as download/export
//! so that you can re-use the same layout based on your use case").
//!
//! A [`PaneLayout`] is where the side tabs sit (left, right or hidden, in
//! order), which one shows on each side, which panes are open, the bottom
//! panel, and the panes' sizes. Light mode, word wrap, the trend's colours
//! and the diagram's block arrangement (`crate::arrange`) are not part of
//! it: those are preferences or the project's.
//!
//! - **Ready-made** layouts ([`presets`]) come with the page.
//! - **Yours** ([`Layouts`]) are kept in this browser, by name.
//! - **A file** ([`Layouts::export`], [`Layouts::import`]) carries yours
//!   to another browser. It is read as hostile: capped in size and count,
//!   names cleaned, unknown tabs dropped, sizes clamped.

use eframe::egui::{self, Rect, RichText, Vec2};
use serde_json::{Value as Json, json};

use crate::box_projects::{Answer, PCall, PReq};
use crate::editor::{BOTTOM_H, Bottom, Chrome, LEFT_W, Pane, RIGHT_W};
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

/// Where the app keeps [`Layouts`] in the browser's storage.
pub const LAYOUTS_KEY: &str = "helmsman-canvas/pane-layouts";
/// The first field of the file and of the stored text.
pub const FORMAT: &str = "helmsman-pane-layouts 1";
/// The name the export downloads as.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const FILE: &str = "helmsman-pane-layouts.json";
/// What the import's file picker offers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const ACCEPT: &str = ".json";
/// Most layouts one browser keeps.
pub const MAX_LAYOUTS: usize = 32;
/// Longest name, characters.
pub const MAX_NAME: usize = 40;
/// Largest file read, bytes: 32 layouts take about 12 KB.
pub const MAX_FILE: usize = 64 * 1024;
/// The sizes a layout may ask a pane for, points.
pub const MIN_SIZE: f32 = 120.0;
pub const MAX_SIZE: f32 = 1600.0;

/// The panes' sizes, points: the side panes' widths and the bottom
/// panel's height. `None`: the size the pane first opens at.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Sizes {
    pub left: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
}

/// One arrangement of the panes.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneLayout {
    /// The left pane's tabs, in order.
    pub lefts: Vec<Pane>,
    /// The right pane's tabs, in order.
    pub rights: Vec<Pane>,
    /// The tabs in neither pane.
    pub hidden: Vec<Pane>,
    /// The tab each side shows.
    pub pane: Pane,
    pub side: Pane,
    /// Which panes are open.
    pub left: bool,
    pub right: bool,
    pub trend: bool,
    /// The bottom panel's tab.
    pub bottom: Bottom,
    pub sizes: Sizes,
}

impl Default for PaneLayout {
    /// The page as it first opens.
    fn default() -> Self {
        Chrome::default().layout()
    }
}

impl Chrome {
    /// The panes as they are now.
    #[must_use]
    pub fn layout(&self) -> PaneLayout {
        PaneLayout {
            lefts: self.lefts.clone(),
            rights: self.rights.clone(),
            hidden: self.hidden.clone(),
            pane: self.pane,
            side: self.side,
            left: self.left,
            right: self.right,
            trend: self.trend,
            bottom: self.bottom,
            sizes: self.sizes,
        }
    }

    /// Arranges the panes as `l` has them. A size it does not give goes
    /// back to the one the pane first opens at.
    pub fn apply(&mut self, l: &PaneLayout) {
        self.lefts.clone_from(&l.lefts);
        self.rights.clone_from(&l.rights);
        self.hidden.clone_from(&l.hidden);
        self.pane = l.pane;
        self.side = l.side;
        self.left = l.left;
        self.right = l.right;
        self.trend = l.trend;
        self.bottom = l.bottom;
        self.tidy();
        self.resize = Some(Sizes {
            left: Some(l.sizes.left.unwrap_or(LEFT_W)),
            right: Some(l.sizes.right.unwrap_or(RIGHT_W)),
            bottom: Some(l.sizes.bottom.unwrap_or(BOTTOM_H)),
        });
    }
}

/// A layout that comes with the page.
pub struct Preset {
    pub name: &'static str,
    pub tip: &'static str,
    pub layout: PaneLayout,
}

/// The ready-made layouts, the page's first arrangement among them.
#[must_use]
pub fn presets() -> Vec<Preset> {
    use Pane::{Claude, Code, Devices, Files, History, Io};
    let base = PaneLayout::default();
    vec![
        Preset {
            name: "Default",
            tip: "Every tab in the left pane with the Describe tab showing, the trend along the bottom.",
            layout: base.clone(),
        },
        Preset {
            name: "Programming",
            tip: "The Describe tab on the left and the code on the right, with the messages along the bottom.",
            layout: PaneLayout {
                lefts: vec![Claude, Files, History],
                rights: vec![Code, Io, Devices],
                pane: Claude,
                side: Code,
                right: true,
                bottom: Bottom::Messages,
                sizes: Sizes {
                    right: Some(420.0),
                    ..Sizes::default()
                },
                ..base.clone()
            },
        },
        Preset {
            name: "Commissioning",
            tip: "The devices on the left and the live I/O on the right, with the trend along the bottom.",
            layout: PaneLayout {
                lefts: vec![Devices, Files, Claude, Code, History],
                rights: vec![Io],
                pane: Devices,
                side: Io,
                right: true,
                ..base.clone()
            },
        },
        Preset {
            name: "Monitoring",
            tip: "Both side panes shut, so the diagram has the width, with the trend along the bottom.",
            layout: PaneLayout {
                left: false,
                right: false,
                ..base
            },
        },
    ]
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
/// Why the picked file `name` of `size` bytes is not read into the page
/// at all, if it is not: asked before the browser loads it.
#[must_use]
pub fn pick_refusal(name: &str, size: f64) -> Option<String> {
    #[allow(clippy::cast_precision_loss)]
    let most = MAX_FILE as f64;
    (size > most).then(|| {
        format!(
            "{} was not read: a pane layout file is at most {} KB.",
            clean_name(name),
            MAX_FILE / 1024
        )
    })
}

fn is_preset(name: &str) -> bool {
    presets().iter().any(|p| p.name.eq_ignore_ascii_case(name))
}

/// A name as it is kept: no control characters, trimmed, at most
/// [`MAX_NAME`] characters. Empty: not a name.
#[must_use]
pub fn clean_name(name: &str) -> String {
    let kept: String = name.chars().filter(|c| !c.is_control()).collect();
    let cut: String = kept.trim().chars().take(MAX_NAME).collect();
    cut.trim().to_string()
}

fn keys(list: &[Pane]) -> Json {
    Json::Array(list.iter().map(|p| json!(p.key())).collect())
}

fn panes(v: &Json) -> Vec<Pane> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .filter_map(Pane::from_key)
                .collect()
        })
        .unwrap_or_default()
}

/// A size a file gives: a finite number, held to what a pane can take,
/// in whole points.
fn size(v: &Json) -> Option<f32> {
    let n = v.as_f64().filter(|n| n.is_finite())?;
    #[allow(clippy::cast_possible_truncation)]
    Some((n as f32).clamp(MIN_SIZE, MAX_SIZE).round())
}

impl PaneLayout {
    pub(crate) fn to_json(&self, name: &str) -> Json {
        let mut v = json!({
            "name": name,
            "left": keys(&self.lefts),
            "right": keys(&self.rights),
            "hidden": keys(&self.hidden),
            "left_shows": self.pane.key(),
            "right_shows": self.side.key(),
            "left_open": self.left,
            "right_open": self.right,
            "bottom_open": self.trend,
            "bottom_shows": self.bottom.key(),
        });
        for (k, s) in [
            ("left_width", self.sizes.left),
            ("right_width", self.sizes.right),
            ("bottom_height", self.sizes.bottom),
        ] {
            if let Some(s) = s {
                v[k] = json!(s.round());
            }
        }
        v
    }

    /// A layout and its name out of a file's entry. Anything missing or
    /// unknown takes the first arrangement's value; an entry with no
    /// usable name is not one.
    fn from_json(v: &Json) -> Option<(String, Self)> {
        let name = clean_name(v["name"].as_str()?);
        if name.is_empty() {
            return None;
        }
        let mut c = Chrome::default();
        c.lefts = panes(&v["left"]);
        c.rights = panes(&v["right"]);
        c.hidden = panes(&v["hidden"]);
        c.pane = v["left_shows"]
            .as_str()
            .and_then(Pane::from_key)
            .unwrap_or_default();
        c.side = v["right_shows"]
            .as_str()
            .and_then(Pane::from_key)
            .unwrap_or_default();
        c.left = v["left_open"].as_bool().unwrap_or(c.left);
        c.right = v["right_open"].as_bool().unwrap_or(c.right);
        c.trend = v["bottom_open"].as_bool().unwrap_or(c.trend);
        c.bottom = v["bottom_shows"]
            .as_str()
            .and_then(Bottom::from_key)
            .unwrap_or_default();
        c.sizes = Sizes {
            left: size(&v["left_width"]),
            right: size(&v["right_width"]),
            bottom: size(&v["bottom_height"]),
        };
        // Every tab once, the shown ones among their pane's.
        c.tidy();
        Some((name, c.layout()))
    }
}

/// The layouts in `text` (a file, or what the browser stored), in order.
///
/// # Errors
/// The text is too long, is not JSON, or is not a pane layout file of
/// this format.
pub fn parse(text: &str) -> Result<Vec<(String, PaneLayout)>, String> {
    if text.len() > MAX_FILE {
        return Err(format!(
            "it is {} KB; a pane layout file is at most {} KB",
            text.len() / 1024 + 1,
            MAX_FILE / 1024
        ));
    }
    let v: Json = serde_json::from_str(text).map_err(|_| "it is not a pane layout file".to_string())?;
    match v["format"].as_str() {
        Some(FORMAT) => {}
        Some(f) if f.starts_with("helmsman-pane-layouts ") => {
            return Err("it was saved by a newer version of this page".into());
        }
        _ => return Err("it is not a pane layout file".into()),
    }
    let list = v["layouts"]
        .as_array()
        .ok_or_else(|| "it holds no layouts".to_string())?;
    Ok(list.iter().filter_map(PaneLayout::from_json).collect())
}

fn render(list: &[(String, PaneLayout)]) -> String {
    let all: Vec<Json> = list.iter().map(|(n, l)| l.to_json(n)).collect();
    json!({ "format": FORMAT, "layouts": all }).to_string()
}

/// What the popup asks the app to do: the browser's download and file
/// picker are the app's, and so is the box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Export,
    Import,
    /// Keep the layout of yours of this name on the box too.
    Keep(String),
    /// Delete the box's layout of this name.
    Forget(String),
}

/// The box's side of the popup, on a page a box serves.
pub struct BoxSide<'a> {
    /// The layouts the box keeps.
    pub list: &'a [(String, PaneLayout)],
    /// What the box last answered, and whether it is an error.
    pub note: Option<&'a (String, bool)>,
    /// Whether whoever is signed in may change the box's list (why not).
    pub can: Result<(), String>,
}

const BOX_URL: &str = "/api/pane-layouts";

/// The pane layouts the box keeps (`GET` and `PUT /api/pane-layouts`,
/// `DELETE /api/pane-layouts/{name}`; helmsman-plc `api::pane_layouts`):
/// one list for the box, so any browser that opens it offers them. Read
/// when the popup opens and after each change; read as a file is, as
/// hostile.
#[derive(Debug, Default)]
pub struct OnBox {
    pub list: Vec<(String, PaneLayout)>,
    /// What the box last answered, for the popup, and whether it is an
    /// error.
    pub note: Option<(String, bool)>,
    /// Asked for since the popup opened.
    asked: bool,
    queue: Vec<PReq>,
}

impl OnBox {
    /// What to send now.
    pub fn poll(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    /// The popup is open: the list is read, once.
    pub fn want(&mut self) {
        if !self.asked {
            self.asked = true;
            self.queue.push(PReq::get(PCall::PaneLayouts, BOX_URL.into()));
        }
    }

    /// The popup shut: the next opening reads the list again.
    pub fn shut(&mut self) {
        self.asked = false;
        self.note = None;
    }

    /// Keeps `layout` on the box as `name`.
    pub fn keep(&mut self, name: &str, layout: &PaneLayout) {
        self.queue.push(PReq::json(
            PCall::KeepPaneLayout(name.to_string()),
            "PUT",
            BOX_URL.into(),
            &layout.to_json(name),
        ));
    }

    /// Deletes the box's layout `name`.
    pub fn forget(&mut self, name: &str) {
        self.queue.push(PReq::delete(
            PCall::DropPaneLayout(name.to_string()),
            format!("{BOX_URL}/{}", crate::remote::segment(name)),
        ));
    }

    /// Takes in the box's answer to `call`.
    pub fn answer(&mut self, call: &PCall, a: &Answer) {
        let ok = (200..300).contains(&a.status);
        let why = || crate::box_projects::error_of(a);
        match call {
            PCall::PaneLayouts if ok => match parse(&a.body) {
                Ok(list) => {
                    // The box's own rules again, on what it sent.
                    let mut read = Layouts::default();
                    for (name, layout) in list {
                        if read.find(&name).is_none() && read.mine.len() < MAX_LAYOUTS {
                            read.mine.push((name, layout));
                        }
                    }
                    self.list = read.mine;
                }
                Err(why) => {
                    self.note = Some((format!("The box's layouts were not read: {why}."), true));
                }
            },
            PCall::PaneLayouts => {
                self.note = Some((format!("The box's layouts were not read: {}", why()), true));
            }
            PCall::KeepPaneLayout(name) if ok => {
                self.note = Some((format!("{name} is kept on the box."), false));
                self.asked = false;
            }
            PCall::KeepPaneLayout(name) => {
                self.note = Some((format!("{name} was not kept on the box: {}", why()), true));
            }
            // Gone already (someone else deleted it) is gone.
            PCall::DropPaneLayout(name) if ok || a.status == 404 => {
                self.note = Some((format!("Deleted {name} from the box."), false));
                self.asked = false;
            }
            PCall::DropPaneLayout(name) => {
                self.note = Some((format!("{name} was not deleted from the box: {}", why()), true));
            }
            _ => {}
        }
    }
}

/// This browser's named layouts, and the popup that lists them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layouts {
    /// By name, in the order they were saved.
    pub mine: Vec<(String, PaneLayout)>,
    /// The popup is up (View › Pane layouts…).
    pub open: bool,
    /// The name typed in the popup.
    pub name: String,
    /// What the last save, delete or import came to, and whether it is
    /// an error.
    pub note: Option<(String, bool)>,
}

impl Layouts {
    /// What the browser stored, read back; text it cannot read is none.
    #[must_use]
    pub fn restored(text: &str) -> Self {
        let mut l = Self::default();
        for (name, layout) in parse(text).unwrap_or_default() {
            // A stored name is one `save` took; a text someone edited is
            // held to the same rules.
            let _ = l.save(&name, layout);
        }
        l
    }

    /// What is kept per browser, and what an export downloads.
    #[must_use]
    pub fn saved(&self) -> String {
        render(&self.mine)
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.mine.iter().position(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// The layout saved as `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PaneLayout> {
        self.find(name).map(|k| &self.mine[k].1)
    }

    /// Keeps `layout` as `name`, in place of one of that name. The name
    /// as kept.
    ///
    /// # Errors
    /// The name is empty or a ready-made layout's, or [`MAX_LAYOUTS`]
    /// are kept already.
    pub fn save(&mut self, name: &str, layout: PaneLayout) -> Result<String, String> {
        let name = clean_name(name);
        if name.is_empty() {
            return Err("Give the layout a name.".into());
        }
        if is_preset(&name) {
            return Err(format!("{name} is a ready-made layout: pick another name."));
        }
        match self.find(&name) {
            Some(k) => self.mine[k] = (name.clone(), layout),
            None if self.mine.len() >= MAX_LAYOUTS => {
                return Err(format!(
                    "This browser keeps {MAX_LAYOUTS} layouts: delete one first."
                ));
            }
            None => self.mine.push((name.clone(), layout)),
        }
        Ok(name)
    }

    /// Drops the layout saved as `name`; whether there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        let Some(k) = self.find(name) else {
            return false;
        };
        self.mine.remove(k);
        true
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    /// The file an export downloads: every layout of yours.
    #[must_use]
    pub fn export(&self) -> Vec<u8> {
        self.saved().into_bytes()
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    /// Takes the layouts of the picked file `file`: one whose name is
    /// kept already replaces it, one named as a ready-made layout comes
    /// in as "NAME (imported)". How many came in.
    ///
    /// # Errors
    /// The file is not a pane layout file, holds none, or none fit
    /// (a sentence naming `file`, for the popup).
    pub fn import(&mut self, file: &str, bytes: &[u8]) -> Result<usize, String> {
        let shown: String = clean_name(file);
        let text =
            std::str::from_utf8(bytes).map_err(|_| format!("{shown} was not read: it is not text."))?;
        let list = parse(text).map_err(|why| format!("{shown} was not read: {why}."))?;
        if list.is_empty() {
            return Err(format!("{shown} holds no layouts."));
        }
        let (mut took, mut full) = (0, false);
        for (name, layout) in list {
            let name = if is_preset(&name) {
                // Room for the suffix within the longest name.
                let stem: String = name.chars().take(MAX_NAME - 11).collect();
                format!("{stem} (imported)")
            } else {
                name
            };
            match self.save(&name, layout) {
                Ok(_) => took += 1,
                Err(_) => full = true,
            }
        }
        if took == 0 {
            return Err(format!(
                "Nothing came in from {shown}: this browser keeps {MAX_LAYOUTS} layouts. Delete one first."
            ));
        }
        self.note = Some((
            if full {
                format!("{took} came in from {shown}; the rest did not fit ({MAX_LAYOUTS} are kept at most).")
            } else if took == 1 {
                format!("1 layout came in from {shown}.")
            } else {
                format!("{took} layouts came in from {shown}.")
            },
            full,
        ));
        Ok(took)
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    /// [`Self::import`] with its refusal shown in the popup.
    pub fn imported(&mut self, file: &str, bytes: &[u8]) {
        if let Err(why) = self.import(file, bytes) {
            self.note = Some((why, true));
        }
    }

    /// The popup: the ready-made layouts and yours, each a button that
    /// arranges the panes; a name and Save for the panes as they are;
    /// Export and Import. On a box's page (`on_box`), the layouts the box
    /// keeps too, with To box on each of yours. What it leaves to the
    /// app, if anything.
    #[allow(clippy::too_many_lines)]
    pub fn popup(
        &mut self,
        ctx: &egui::Context,
        chrome: &mut Chrome,
        hits: &mut Vec<(String, Rect)>,
        on_box: Option<&BoxSide<'_>>,
    ) -> Option<Act> {
        let mut act = None;
        let mut shut = false;
        let wide = Vec2::new(220.0, TAP * 0.7);
        // With To box beside Delete, a row of yours has less for the name.
        let mine_wide = Vec2::new(if on_box.is_some() { 150.0 } else { wide.x }, wide.y);
        let list_h = if on_box.is_some() { 150.0 } else { 240.0 };
        let m = egui::Modal::new(egui::Id::unique("pane-layouts")).show(ctx, |ui| {
            let t = theme(ui);
            ui.set_max_width(360.0);
            ui.label(RichText::new("Pane layouts").strong().size(20.0))
                .tip("Where the side tabs sit, which panes are open and how wide they are, kept by name.");
            ui.add_space(6.0);
            ui.label(RichText::new("Ready-made").color(t.muted))
                .tip("Layouts that come with the page.");
            for p in presets() {
                let r = ui.add(egui::Button::new(p.name).min_size(wide)).tip(p.tip);
                hits.push((format!("layouts preset {}", p.name.to_lowercase()), r.rect));
                if r.clicked() {
                    chrome.apply(&p.layout);
                    shut = true;
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Yours").color(t.muted))
                .tip("Layouts saved in this browser. Export carries them to another one.");
            if self.mine.is_empty() {
                ui.label("None saved yet.")
                    .tip("Arrange the panes, type a name below and press Save.");
            }
            let mut gone = None;
            egui::ScrollArea::vertical()
                .id_salt("pane-layouts-mine")
                .max_height(list_h)
                .show(ui, |ui| {
                    for (name, layout) in &self.mine {
                        ui.horizontal(|ui| {
                            let r = ui
                                .add_sized(mine_wide, egui::Button::new(name.as_str()).truncate())
                                .tip(format!("Arranges the panes as {name} has them."));
                            hits.push((format!("layouts mine {name}"), r.rect));
                            if r.clicked() {
                                chrome.apply(layout);
                                shut = true;
                            }
                            let d = ui
                                .add(egui::Button::new("Delete"))
                                .tip(format!("Deletes {name} from this browser."));
                            hits.push((format!("layouts delete {name}"), d.rect));
                            if d.clicked() {
                                gone = Some(name.clone());
                            }
                            if let Some(b) = on_box {
                                let k = ui
                                    .add_enabled(b.can.is_ok(), egui::Button::new("To box"))
                                    .tip(match &b.can {
                                        Ok(()) => format!(
                                            "Keeps {name} on this box too: any browser that opens the box offers it."
                                        ),
                                        Err(why) => why.clone(),
                                    });
                                hits.push((format!("layouts keep {name}"), k.rect));
                                if k.clicked() {
                                    act = Some(Act::Keep(name.clone()));
                                }
                            }
                        });
                    }
                });
            if let Some(name) = gone {
                self.remove(&name);
                self.note = Some((format!("Deleted {name}."), false));
            }
            if let Some(b) = on_box {
                ui.add_space(6.0);
                ui.label(RichText::new("On this box").color(t.muted))
                    .tip("Layouts this box keeps: any browser that opens it offers them. An engineer adds and deletes them.");
                if b.list.is_empty() {
                    ui.label("None kept yet.")
                        .tip("Save a layout of yours, then press To box beside it.");
                }
                egui::ScrollArea::vertical()
                    .id_salt("pane-layouts-box")
                    .max_height(list_h)
                    .show(ui, |ui| {
                        for (name, layout) in b.list {
                            ui.horizontal(|ui| {
                                let r = ui
                                    .add_sized(wide, egui::Button::new(name.as_str()).truncate())
                                    .tip(format!("Arranges the panes as the box's {name} has them."));
                                hits.push((format!("layouts box {name}"), r.rect));
                                if r.clicked() {
                                    chrome.apply(layout);
                                    shut = true;
                                }
                                let d = ui
                                    .add_enabled(b.can.is_ok(), egui::Button::new("Delete"))
                                    .tip(match &b.can {
                                        Ok(()) => format!("Deletes {name} from this box, for everyone."),
                                        Err(why) => why.clone(),
                                    });
                                hits.push((format!("layouts forget {name}"), d.rect));
                                if d.clicked() {
                                    act = Some(Act::Forget(name.clone()));
                                }
                            });
                        }
                    });
                if let Some((text, bad)) = b.note {
                    let ink = if *bad {
                        t.ink(crate::st_view::brand::RED)
                    } else {
                        t.muted
                    };
                    let r = ui.label(RichText::new(text).color(ink));
                    hits.push(("layouts box note".to_string(), r.rect));
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let f = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text("Name")
                            .char_limit(MAX_NAME)
                            .desired_width(wide.x - 8.0),
                    )
                    .tip("A name for the panes as they are now. A name already in the list replaces that layout.");
                hits.push(("layouts name".to_string(), f.rect));
                let entered = f.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let named = !clean_name(&self.name).is_empty();
                let s = ui.add_enabled(named, egui::Button::new("Save")).tip(if named {
                    "Keeps the panes as they are now under this name, in this browser."
                } else {
                    "Type a name first."
                });
                hits.push(("layouts save".to_string(), s.rect));
                if s.clicked() || (entered && named) {
                    let name = self.name.clone();
                    match self.save(&name, chrome.layout()) {
                        Ok(kept) => {
                            self.note = Some((format!("Saved {kept}."), false));
                            self.name.clear();
                        }
                        Err(why) => self.note = Some((why, true)),
                    }
                }
            });
            if let Some((text, bad)) = &self.note {
                let ink = if *bad {
                    t.ink(crate::st_view::brand::RED)
                } else {
                    t.muted
                };
                let r = ui.label(RichText::new(text).color(ink));
                hits.push(("layouts note".to_string(), r.rect));
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let some = !self.mine.is_empty();
                let e = ui.add_enabled(some, egui::Button::new("Export…")).tip(if some {
                    "Downloads your layouts as one file, to import in another browser."
                } else {
                    "Save a layout first: the file holds yours."
                });
                hits.push(("layouts export".to_string(), e.rect));
                if e.clicked() {
                    act = Some(Act::Export);
                }
                let i = ui.add(egui::Button::new("Import…")).tip(
                    "Reads layouts from a file exported here. One with a name you already have replaces it.",
                );
                hits.push(("layouts import".to_string(), i.rect));
                if i.clicked() {
                    act = Some(Act::Import);
                }
                let c = ui.add(egui::Button::new("Close")).tip("Shuts this list (Escape).");
                hits.push(("layouts close".to_string(), c.rect));
                if c.clicked() {
                    shut = true;
                }
            });
        });
        hits.push(("layouts".to_string(), m.response.rect));
        if shut || crate::theme::escaped(&m) {
            self.open = false;
            self.note = None;
        }
        act
    }
}
