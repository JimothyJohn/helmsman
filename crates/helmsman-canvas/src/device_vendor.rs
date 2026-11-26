//! The vendor's file for a device the library does not know (card
//! `device-profile-propose-box`, option (a), Nick 2026-11-15): a button on
//! the New device form (From the vendor's file…) picks the file (an EDS, an IODD, a manual), posts
//! it to the box as a drop-in (`POST /api/dropins`) and, once the box has
//! kept it, asks the agent in chat for a device profile, carrying the
//! drop-in's id (`POST /api/chat` `{message, session, dropins: [id]}`). The
//! agent proposes through `propose_device_profile`; the proposal shows on
//! the Devices tab ([`crate::device_proposal`]).
//!
//! Free of the transport like [`crate::device_proposal`]: the requests it
//! wants sent are taken by the view, and so is the chat message.
//!
//! - **Nothing reaches the agent unless the file was kept.** The message
//!   is built only from a 201 that names the drop-in's id.
//! - **A file name is user input.** The box gets a name cut to
//!   `[A-Za-z0-9._-]` and 100 characters, and the chat message names the
//!   drop-in by its id and kind only, never by the name it came with. The
//!   scan's own words (a product name any device on the LAN can choose)
//!   are not in the message either: only numbers and a checked address.
//! - **Roles.** What an engineer or integrator may do, like saving the
//!   proposal; everyone else is told why there is no button.

// Natively only the tests carry a pick and an answer in.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eframe::egui::{self, RichText, Vec2};
use serde_json::Value as Json;

use crate::box_projects::{PCall, PReq};
use crate::device_proposal::clean;
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

/// Room left for the scroll bar.
const BAR: f32 = 12.0;
const RED: egui::Color32 = egui::Color32::from_rgb(255, 110, 100);

pub const URL: &str = "/api/dropins";
/// What the picker offers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const ACCEPT: &str = ".eds,.xml,.iodd,.pdf,.txt,.md";
/// The box's limit for a text drop-in (`api::dropin::MAX_DROPIN`), bytes.
pub const MAX_TEXT: usize = 1 << 20;
/// The box's limit for a PDF (`api::pdf::MAX_PDF`), bytes.
pub const MAX_PDF: usize = 2 << 20;
/// Longest file name sent, characters.
const MAX_NAME: usize = 100;
/// The agent's answer shown beside the button, characters.
const MAX_SAID: usize = 400;

/// The button's text.
pub const BUTTON: &str = "From the vendor's file…";

/// What kind of file it is, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum File {
    Pdf,
    Text,
}

/// A file name safe to send and show: its last path component, every
/// character but `A-Z a-z 0-9 . _ -` turned into `_`, no leading dot, at
/// most [`MAX_NAME`] characters with the extension kept.
#[must_use]
pub fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mapped: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mapped = mapped.trim_start_matches('.');
    let (stem, ext) = match mapped.rsplit_once('.') {
        Some((s, e)) if e.len() <= 8 => (s, format!(".{e}")),
        _ => (mapped, String::new()),
    };
    let room = MAX_NAME.saturating_sub(ext.len());
    let stem: String = stem.chars().take(room).collect();
    if stem.is_empty() {
        format!("vendor-file{ext}")
    } else {
        format!("{stem}{ext}")
    }
}

/// The file's kind by its extension, or why the agent cannot read it.
///
/// # Errors
/// The extension is not one of an EDS, an IODD, a PDF, text or Markdown.
pub fn classify(name: &str) -> Result<File, String> {
    let safe = safe_name(name);
    let ext = safe.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Ok(File::Pdf),
        "eds" | "xml" | "iodd" | "txt" | "md" => Ok(File::Text),
        _ => Err(format!(
            "{} is not a file Helmsman can read here: pick an EDS, an IODD (XML), a PDF manual, or a text file.",
            clean(&safe, 60, false)
        )),
    }
}

/// Why a picked file of `size` bytes is not read at all, if it is not.
#[must_use]
pub fn pick_refusal(name: &str, size: f64) -> Option<String> {
    let shown = clean(&safe_name(name), 60, false);
    let kind = match classify(name) {
        Ok(k) => k,
        Err(why) => return Some(why),
    };
    let (max, what) = match kind {
        File::Pdf => (MAX_PDF, "a PDF is at most 2 MB"),
        File::Text => (MAX_TEXT, "a text file is at most 1 MB"),
    };
    #[allow(clippy::cast_precision_loss)]
    let over = size > max as f64;
    over.then(|| {
        format!(
            "{shown} is {:.1} MB: {what}. Nothing was sent. Send the pages that matter, or the EDS.",
            size / 1_048_576.0
        )
    })
}

/// What the box's refusal of an upload (`status`, its `body`) says, in
/// words for the engineer, naming `name`.
#[must_use]
pub fn refusal_words(status: u16, body: &str, name: &str) -> String {
    let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
    let why = clean(j["error"].as_str().unwrap_or_default(), 200, false);
    let none = "Nothing was sent.";
    let (_, text) = match status {
        0 => (
            Tone::Error,
            format!("The box did not answer ({}). {none}", clean(body, 120, false)),
        ),
        401 => (
            Tone::Error,
            format!("The box no longer knows this page's session. Sign in again. {none}"),
        ),
        403 => (
            Tone::Error,
            format!(
                "The box refused the file (403){}. {none}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(": {why}")
                }
            ),
        ),
        413 => (
            Tone::Error,
            format!(
                "{name} is too large for the box{}. {none} Send the pages that matter, or the EDS.",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                }
            ),
        ),
        415 => (
            Tone::Error,
            format!(
                "The box does not take {name} as that kind of file{}. {none}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                }
            ),
        ),
        422 => (
            Tone::Error,
            format!(
                "The box could not read {name}{}. {none}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(": {why}")
                }
            ),
        ),
        s => (
            Tone::Error,
            format!(
                "The box refused {name} ({s}){}. {none}",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(": {why}")
                }
            ),
        ),
    };
    text
}

/// The request that keeps `bytes` on the box as a drop-in of `kind`
/// under the safe form of `name`; `call` says who is answered.
#[must_use]
pub fn upload_request(call: PCall, name: &str, bytes: Vec<u8>, kind: File) -> PReq {
    let (kind_word, ctype) = match kind {
        File::Pdf => ("pdf", "application/pdf"),
        File::Text => ("prose", "text/plain"),
    };
    PReq {
        call,
        method: "POST",
        url: format!(
            "{URL}?name={}&kind={kind_word}",
            crate::remote::segment(&safe_name(name))
        ),
        body: Some(bytes),
        ctype,
    }
}

/// The device the file is for: what the New device form has typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub address: String,
    vendor_id: u16,
    product_code: u16,
    /// `cip=...`, `modbus=...`, `mac=...`: what the scan saw.
    identity: String,
}

impl Target {
    #[must_use]
    pub fn new(address: &str, vendor_id: u16, product_code: u16, identity: &str) -> Self {
        Self {
            address: address.to_string(),
            vendor_id,
            product_code,
            identity: identity.to_string(),
        }
    }

    /// The device in the agent's request: numbers and a checked address
    /// only, never a name the device sent.
    fn words(&self) -> String {
        let plain = |s: &str, extra: &[char]| {
            !s.is_empty()
                && s.len() <= 64
                && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
        };
        let mut out = String::new();
        if self.identity.starts_with("cip=") {
            let _ = write!(
                out,
                " It is an EtherNet/IP device: vendor ID {}, product code {}.",
                self.vendor_id, self.product_code
            );
        } else if plain(&self.identity, &['=', ':', '.', '-', '_']) {
            let _ = write!(out, " The scan identified it as {}.", self.identity);
        }
        if plain(&self.address, &['.', ':', '[', ']', '-']) {
            let _ = write!(out, " It answers at {}.", self.address);
        }
        out
    }
}

/// What to say to the agent, and the drop-in it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub message: String,
    pub dropins: Vec<u64>,
}

/// The agent's request for a profile, naming the drop-in by id.
#[must_use]
pub fn ask_for(target: &Target, id: u64, kind: File) -> Ask {
    let what = match kind {
        File::Pdf => "a PDF",
        File::Text => "a text file",
    };
    let how = match kind {
        File::Pdf => "You see it in this message.",
        File::Text => "Read it with read_dropin.",
    };
    Ask {
        message: format!(
            "Build a device profile for a device on my network that the device library does not have. The vendor's file is attached as drop-in {id} ({what}). {how}{} Take every channel name, type and address from that file, check the profile with propose_device_profile, and ask me about anything the file does not say.",
            target.words()
        ),
        dropins: vec![id],
    }
}

/// How the note beside the button reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Done,
    Error,
}

/// A line beside a found device's button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub address: String,
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    Idle,
    /// The picker is open.
    Picking,
    /// The file is on its way to the box.
    Uploading(File),
    /// The agent has the request: its turn number.
    Waiting(u64),
}

/// What the page knows that this section needs each frame.
#[derive(Debug, Clone)]
pub struct Context {
    /// A box with an agent behind this page (the simulation has none).
    pub on_box: bool,
    /// Whether this session may hand the agent a file, and why not.
    pub engineer: Result<(), String>,
    /// Whether the agent can be asked now, and why not.
    pub agent: Result<(), String>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            on_box: false,
            engineer: Err(String::new()),
            agent: Ok(()),
        }
    }
}

/// The button's state, for every found device without a library profile.
#[derive(Debug, Default)]
pub struct VendorUi {
    stage: Stage,
    target: Option<Target>,
    /// The name sent, for the words beside the button.
    sent_name: String,
    pub note: Option<Note>,
    ctx_: Option<Context>,
    queue: Vec<PReq>,
    ask: Option<Ask>,
    want_pick: bool,
    /// The device the last request was for, until its profile is saved.
    asked_for: Option<String>,
    /// A found device's address, to the profile saved from its file.
    pub built: BTreeMap<String, String>,
}

impl VendorUi {
    pub fn set_context(&mut self, c: Context) {
        self.ctx_ = Some(c);
    }

    fn context(&self) -> Context {
        self.ctx_.clone().unwrap_or_default()
    }

    /// Why the button is not offered, or `Ok` when it is.
    ///
    /// # Errors
    /// The words beside the disabled button.
    pub fn offer(&self) -> Result<(), String> {
        let c = self.context();
        if !c.on_box {
            return Err("Reading a vendor's file needs a box. This page has none to ask.".into());
        }
        if let Err(why) = c.engineer {
            return Err(if why.is_empty() {
                "Sign in as an engineer to hand Helmsman a file.".into()
            } else {
                why
            });
        }
        c.agent?;
        match self.stage {
            Stage::Idle => Ok(()),
            Stage::Picking | Stage::Uploading(_) => Err("Waiting for the file to reach the box.".into()),
            Stage::Waiting(_) => Err("Reading it.".into()),
        }
    }

    /// The button was pressed for `target`.
    pub fn start(&mut self, target: Target) {
        if self.offer().is_err() {
            return;
        }
        self.note = None;
        self.target = Some(target);
        self.stage = Stage::Picking;
        self.want_pick = true;
    }

    /// The picker is open and nothing came back yet.
    #[cfg(test)]
    #[must_use]
    pub fn is_picking(&self) -> bool {
        self.stage == Stage::Picking
    }

    /// The view opens the picker once.
    pub fn take_pick(&mut self) -> bool {
        std::mem::take(&mut self.want_pick)
    }

    /// The requests to send now.
    pub fn take_requests(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    /// The message for the agent, once the box kept the file.
    pub fn take_ask(&mut self) -> Option<Ask> {
        self.ask.take()
    }

    fn say(&mut self, tone: Tone, text: String) {
        let address = self
            .target
            .as_ref()
            .map(|t| t.address.clone())
            .unwrap_or_default();
        self.note = Some(Note { address, text, tone });
    }

    /// The picker could not give a file.
    pub fn pick_failed(&mut self, why: &str) {
        if self.stage == Stage::Picking {
            self.stage = Stage::Idle;
            self.say(Tone::Error, clean(why, 300, false));
        }
    }

    /// The file the user picked: checked, then sent to the box.
    pub fn picked(&mut self, name: &str, bytes: Vec<u8>) {
        if self.stage != Stage::Picking {
            return;
        }
        let shown = clean(&safe_name(name), 60, false);
        if let Err(why) = self.offer_after_pick() {
            self.stage = Stage::Idle;
            return self.say(Tone::Error, why);
        }
        let kind = match classify(name) {
            Ok(k) => k,
            Err(why) => {
                self.stage = Stage::Idle;
                return self.say(Tone::Error, why);
            }
        };
        if bytes.is_empty() {
            self.stage = Stage::Idle;
            return self.say(Tone::Error, format!("{shown} is empty. Nothing was sent."));
        }
        if let Some(why) = pick_refusal(name, bytes.len() as f64) {
            self.stage = Stage::Idle;
            return self.say(Tone::Error, why);
        }
        self.queue
            .push(upload_request(PCall::VendorFile, name, bytes, kind));
        self.sent_name.clone_from(&shown);
        self.stage = Stage::Uploading(kind);
        self.say(Tone::Info, format!("Sending {shown} to the box…"));
    }

    /// The same gates as [`Self::offer`], with the picker's own stage.
    fn offer_after_pick(&self) -> Result<(), String> {
        let c = self.context();
        c.engineer.and(c.agent)
    }

    /// The box's answer to the upload.
    pub fn answer(&mut self, status: u16, body: &str) {
        let Stage::Uploading(kind) = self.stage else {
            return;
        };
        let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        let name = self.sent_name.clone();
        let none = "Nothing was sent.";
        let (tone, text) = match status {
            201 => {
                let Some(id) = j["id"].as_u64() else {
                    self.stage = Stage::Idle;
                    return self.say(
                        Tone::Error,
                        format!("The box kept {name} but did not say where, so nothing was asked. {none}"),
                    );
                };
                let Some(t) = &self.target else {
                    self.stage = Stage::Idle;
                    return;
                };
                self.ask = Some(ask_for(t, id, kind));
                self.asked_for = Some(t.address.clone());
                self.say(Tone::Info, format!("The box has {name}. Asking Helmsman…"));
                return;
            }
            _ => (Tone::Error, refusal_words(status, body, &name)),
        };
        self.stage = Stage::Idle;
        self.asked_for = None;
        self.say(tone, text);
    }

    /// The view sent the request to the agent (`Ok`: its turn number) or
    /// could not (`Err`: why).
    pub fn asked(&mut self, sent: Result<u64, String>) {
        match sent {
            Ok(seq) => {
                self.stage = Stage::Waiting(seq);
                let name = self.sent_name.clone();
                self.say(
                    Tone::Info,
                    format!("Reading {name}. Its profile shows here when it has one."),
                );
            }
            Err(why) => {
                self.stage = Stage::Idle;
                self.asked_for = None;
                let name = self.sent_name.clone();
                self.say(
                    Tone::Error,
                    format!(
                        "The box has {name}, but Helmsman could not be asked: {}. Press the button again.",
                        clean(&why, 200, false)
                    ),
                );
            }
        }
    }

    /// The agent's turn `seq` ended (or has not): what it said, or why not.
    pub fn agent_turn(&mut self, seq: u64, end: Option<&crate::box_agent::End>, text: &str) {
        use crate::box_agent::End;
        let Stage::Waiting(want) = self.stage else {
            return;
        };
        if want != seq {
            // Another message went, or the turn was stopped from the page.
            self.stage = Stage::Idle;
            return self.say(Tone::Error, "Stopped.".into());
        }
        let Some(end) = end else { return };
        self.stage = Stage::Idle;
        match end {
            End::Done { stop, why } if stop == "end_turn" => {
                let said = clean(text, MAX_SAID, true);
                let text = if said.is_empty() {
                    "Finished without a profile and without a word.".to_string()
                } else {
                    format!("Finished. Helmsman said: {said}")
                };
                let _ = why;
                self.say(Tone::Done, text);
            }
            End::Done { why, stop } => {
                let why = why.clone().unwrap_or_else(|| format!("it stopped ({stop})"));
                self.say(
                    Tone::Error,
                    format!("Did not finish: {}.", clean(&why, 200, false)),
                );
            }
            End::Failed(why) => {
                self.say(Tone::Error, format!("No answer: {}.", clean(why, 200, false)));
            }
            End::Stopped => self.say(Tone::Error, "Stopped.".into()),
        }
    }

    /// A profile was saved into the project: it belongs to the device the
    /// file was sent for, if one is waiting.
    pub fn saved(&mut self, file: &str) {
        if let Some(addr) = self.asked_for.take() {
            self.built.insert(addr, file.to_string());
        }
    }

    /// The button and the words beside it, in the New device form. `target`
    /// is what the form has typed (an address, or none).
    pub fn show_row(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, target: &Target) {
        let t = theme(ui);
        let offer = self.offer();
        let busy = !matches!(self.stage, Stage::Idle);
        let tip = match &offer {
            Ok(()) => "Pick the vendor's file for this device: an EDS, an IODD, or a manual (a PDF up to 2 MB, text up to 1 MB). The box keeps it in memory, and Helmsman reads it and proposes a profile with the channel names. You check the names on the Devices tab before anything is saved.".to_string(),
            Err(why) => why.clone(),
        };
        ui.horizontal_wrapped(|ui| {
            let b = ui
                .add_enabled(
                    offer.is_ok(),
                    egui::Button::new(RichText::new(if busy { "Working…" } else { BUTTON }).strong())
                        .min_size(Vec2::new(0.0, TAP * 0.7)),
                )
                .tip(tip.as_str());
            chrome.hit("vendor file", b.rect);
            if b.clicked() {
                self.start(target.clone());
            }
        });
        if let Err(why) = &offer
            && !busy
            && !why.is_empty()
        {
            ui.indent("vendor why", |ui| {
                ui.set_max_width(ui.available_width() - BAR);
                ui.add(egui::Label::new(RichText::new(why).color(t.muted)).wrap());
            });
        }
        if let Some(n) = self.note.clone() {
            ui.indent("vendor note", |ui| {
                ui.set_max_width(ui.available_width() - BAR);
                let color = match n.tone {
                    Tone::Info => t.muted,
                    Tone::Done => t.accent,
                    Tone::Error => theme(ui).ink(RED),
                };
                let r = ui.add(egui::Label::new(RichText::new(&n.text).color(color)).wrap());
                chrome.hit("vendor note", r.rect);
            });
        }
    }
}
