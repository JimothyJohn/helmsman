//! A device profile the agent proposed, on the Devices tab (card
//! `device-profile-propose-box`, PROTOCOLS.md step 5): the box holds one
//! proposal (`GET /api/device-profile/proposal`); the tab shows its name,
//! the agent's summary and its channels, lets the engineer rename the
//! device and each channel in place, and saves it into the open project
//! (`POST .../accept`) or throws it away (`DELETE`). Every call is built
//! here and every answer taken in [`ProposalUi::answer`], free of the
//! transport like [`crate::device_config`].
//!
//! - **One reader.** The box holds the text and reads it again after the
//!   rename; this module never writes a profile. It sends the id it read,
//!   the project, the device's name when changed and the renames
//!   (`{FROM: TO}`, changed channels only), and shows what the box says.
//! - **A newer proposal.** The agent may replace the proposal at any time.
//!   A read that brings another id (on the poll, or after a 409) replaces
//!   what is shown and keeps the typed names where the channel still
//!   exists.
//! - **Hostile text.** Everything from the box is cut to printable
//!   characters and a length before it is drawn; rows wrap and the list
//!   scrolls, so no proposal widens the panel.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eframe::egui::{self, RichText, Vec2};
use serde_json::{Value as Json, json};

use crate::box_projects::{PCall, PReq};
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

const RED: egui::Color32 = egui::Color32::from_rgb(255, 110, 100);
const AMBER: egui::Color32 = egui::Color32::from_rgb(255, 196, 60);

pub const URL: &str = "/api/device-profile/proposal";
pub const ACCEPT_URL: &str = "/api/device-profile/proposal/accept";
/// Sends the saved profile to the device library (card
/// `device-profile-publish`).
pub const PUBLISH_URL: &str = "/api/device-profile/publish";
/// Seconds between reads of the box's proposal while the tab is open.
pub const POLL_S: f64 = 3.0;
/// Most channels kept of one proposal (the box's own limit).
const MAX_CHANNELS: usize = 256;
/// Longest summary kept, characters (the box's limit).
const MAX_SUMMARY: usize = 8000;
/// Longest word of a channel's facts kept, characters.
const MAX_WORD: usize = 64;
/// Room left for the scroll bar.
const BAR: f32 = 12.0;
/// The channel list's height before it scrolls.
const LIST_H: f32 = 320.0;
/// The summary's height before it scrolls.
const SUMMARY_H: f32 = 150.0;

/// Text from the box for people to read: control and bidirectional
/// characters (line breaks kept when `lines`) dropped, cut at `max`.
pub(crate) fn clean(s: &str, max: usize, lines: bool) -> String {
    s.chars()
        .filter(|c| {
            let bidi =
                matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}');
            !bidi && (!c.is_control() || (lines && *c == '\n'))
        })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// One proposed channel, as the box lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chan {
    pub name: String,
    /// `in` or `out`.
    pub dir: String,
    pub ty: String,
    pub unit: Option<String>,
    /// An output's value when the device is lost.
    pub safe: Option<String>,
    /// Transport to point, for the hover only.
    pub points: Vec<(String, String)>,
}

impl Chan {
    /// What the row says after the name: direction, type, unit, safe value.
    #[must_use]
    pub fn facts(&self) -> String {
        let dir = if self.dir == "out" { "output" } else { "input" };
        let mut s = format!("{dir}, {}", self.ty);
        if let Some(u) = &self.unit {
            let _ = write!(s, " in {u}");
        }
        if let Some(v) = &self.safe {
            let _ = write!(s, ", safe value {v}");
        }
        s
    }
}

/// The proposal the box holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub id: u64,
    pub name: String,
    pub summary: String,
    pub vendor: String,
    pub model: String,
    pub channels: Vec<Chan>,
    /// For each line of the text, the channel it defines: a problem the
    /// box reports "at line N" is shown beside that channel.
    line_channel: Vec<Option<String>>,
}

impl Proposal {
    /// Reads `GET /api/device-profile/proposal`.
    #[must_use]
    pub fn parse(j: &Json) -> Option<Self> {
        let id = j["id"].as_u64()?;
        let name = clean(j["name"].as_str()?, 64, false);
        let channels = j["channels"]
            .as_array()?
            .iter()
            .take(MAX_CHANNELS)
            .filter_map(|c| {
                let word = |k: &str| c[k].as_str().map(|s| clean(s, MAX_WORD, false));
                Some(Chan {
                    name: word("name")?,
                    dir: word("dir")?,
                    ty: word("type")?,
                    unit: word("unit").filter(|s| !s.is_empty()),
                    safe: word("safe").filter(|s| !s.is_empty()),
                    points: c["points"]
                        .as_object()
                        .map(|m| {
                            m.iter()
                                .take(8)
                                .filter_map(|(k, v)| {
                                    Some((clean(k, MAX_WORD, false), clean(v.as_str()?, MAX_WORD, false)))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect();
        let line_channel = j["text"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .take(1000)
            .map(|l| {
                let code = l.split('#').next().unwrap_or_default();
                let mut w = code.split_whitespace();
                match (w.next(), w.next()) {
                    (Some("in" | "out"), Some(n)) => Some(n.to_string()),
                    _ => None,
                }
            })
            .collect();
        Some(Self {
            id,
            name,
            summary: clean(j["summary"].as_str().unwrap_or_default(), MAX_SUMMARY, true),
            vendor: clean(j["vendor"].as_str().unwrap_or_default(), MAX_WORD, false),
            model: clean(j["model"].as_str().unwrap_or_default(), MAX_WORD, false),
            channels,
            line_channel,
        })
    }
}

/// Where a refusal belongs on the screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Refusals {
    /// Beside the device's name.
    pub name: Option<String>,
    /// Beside the channel (its name as proposed).
    pub channels: BTreeMap<String, Vec<String>>,
    /// Over the buttons.
    pub general: Vec<String>,
}

/// A profile the box saved: its project and file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub project: String,
    pub file: String,
}

/// Where sending the saved profile to the library stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Publishing {
    #[default]
    Idle,
    /// Asked once; waiting for the person to confirm.
    Confirming,
    /// The request is on its way.
    Sending,
    /// The library has a review request for it.
    Sent {
        number: u64,
        /// The page of the request, when the box named a library page.
        link: Option<String>,
        /// `false`: it was already there.
        created: bool,
    },
    /// Nothing was sent; why, in words.
    Refused(String),
}

/// The page of a library review request: `https://github.com/OWNER/REPO/pull/N`
/// with plain names, or nothing. What the box says is data: a link is
/// only drawn when it has this shape.
fn pull_link(url: &str, number: u64) -> Option<String> {
    let plain = |p: &str| {
        (1..=100).contains(&p.len())
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && !p.bytes().all(|b| b == b'.')
    };
    let (owner, repo) = url
        .strip_prefix("https://github.com/")?
        .strip_suffix(&format!("/pull/{number}"))?
        .split_once('/')?;
    (plain(owner) && plain(repo)).then(|| url.to_string())
}

/// What the page knows that this section needs each frame.
#[derive(Debug, Clone)]
pub struct Context {
    /// A person is signed in (reads need a session).
    pub signed_in: bool,
    /// Whether this session may save or discard, and why not.
    pub engineer: Result<(), String>,
    /// The project that is open.
    pub project: Option<String>,
    /// The saved profile's text, once the open project holds it.
    pub saved_text: Option<String>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            signed_in: false,
            engineer: Err(String::new()),
            project: None,
            saved_text: None,
        }
    }
}

/// The tab's section.
#[derive(Debug, Default)]
pub struct ProposalUi {
    /// The proposal as last read.
    pub held: Option<Proposal>,
    /// The device's name as typed.
    pub name: String,
    /// Each channel as proposed, to the text typed for it.
    pub edits: BTreeMap<String, String>,
    pub refusals: Refusals,
    /// Said above the proposal (a newer one replaced it).
    pub notice: Option<String>,
    /// A save is on its way.
    pub saving: bool,
    pub saved: Option<Saved>,
    /// Sending the saved profile to the library.
    pub publishing: Publishing,
    /// To tell the view: the project to read again.
    reread: Option<String>,
    ctx: Context,
    queue: Vec<PReq>,
    /// A read is on its way.
    reading: bool,
    next_read: f64,
    /// A 409 came: the read that follows tells a newer proposal from a
    /// name already taken.
    conflict: bool,
}

impl ProposalUi {
    pub fn set_context(&mut self, c: Context) {
        self.ctx = c;
    }

    /// The saved profile's file name, once one was saved.
    #[must_use]
    pub fn saved_file(&self) -> Option<&str> {
        self.saved.as_ref().map(|s| s.file.as_str())
    }

    /// The requests to send now.
    pub fn take_requests(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    /// The project whose files to read again after a save, once.
    pub fn take_saved(&mut self) -> Option<String> {
        self.reread.take()
    }

    fn read(&mut self) {
        self.reading = true;
        self.queue.push(PReq::get(PCall::Proposal, URL.into()));
    }

    /// Reads the box's proposal when it is time.
    fn poll(&mut self, now: f64) {
        if self.ctx.signed_in && !self.reading && !self.saving && now >= self.next_read {
            self.next_read = now + POLL_S;
            self.read();
        }
    }

    /// The accept's body: the id read, the project, the name if changed,
    /// the renames of the channels that changed.
    #[must_use]
    pub fn accept_body(&self, project: &str) -> Option<Json> {
        let p = self.held.as_ref()?;
        let mut body = json!({ "id": p.id, "project": project });
        let name = self.name.trim();
        if name != p.name {
            body["name"] = json!(name);
        }
        let renames: BTreeMap<&str, &str> = self
            .edits
            .iter()
            .filter(|(from, to)| to.trim() != from.as_str())
            .map(|(from, to)| (from.as_str(), to.trim()))
            .collect();
        if !renames.is_empty() {
            body["channels"] = json!(renames);
        }
        Some(body)
    }

    fn save(&mut self) {
        let Some(project) = self.ctx.project.clone() else {
            return;
        };
        let Some(body) = self.accept_body(&project) else {
            return;
        };
        self.refusals = Refusals::default();
        self.notice = None;
        self.saving = true;
        self.queue.push(PReq::json(
            PCall::ProposalAccept,
            "POST",
            ACCEPT_URL.into(),
            &body,
        ));
    }

    /// Sends the saved profile to the library: the box reads the file
    /// from the project again, so only its project and name go.
    fn publish(&mut self) {
        let Some(s) = self.saved.clone() else {
            return;
        };
        if self.publishing != Publishing::Confirming {
            return;
        }
        self.publishing = Publishing::Sending;
        self.queue.push(PReq::json(
            PCall::ProposalPublish,
            "POST",
            PUBLISH_URL.into(),
            &json!({ "project": s.project, "file": s.file }),
        ));
    }

    fn publish_answer(&mut self, status: u16, j: &Json, body: &str) {
        if self.publishing != Publishing::Sending {
            return;
        }
        self.publishing = match status {
            200 | 201 => match j["number"].as_u64() {
                Some(number) => Publishing::Sent {
                    number,
                    link: j["url"].as_str().and_then(|u| pull_link(u, number)),
                    created: status == 201 && j["created"] == true,
                },
                None => Publishing::Refused("the box's answer was not understood".into()),
            },
            0 => Publishing::Refused(format!("the box did not answer ({})", clean(body, 120, false))),
            401 => Publishing::Refused("the box no longer knows this page's session. Sign in again".into()),
            403 => {
                Publishing::Refused("only an engineer can publish a device profile. Sign in as one".into())
            }
            _ => {
                let why = clean(j["error"].as_str().unwrap_or_default(), 300, false);
                Publishing::Refused(if why.is_empty() {
                    format!("the box refused it ({status})")
                } else {
                    why
                })
            }
        };
    }

    fn discard(&mut self) {
        self.refusals = Refusals::default();
        self.queue.push(PReq::delete(PCall::ProposalDiscard, URL.into()));
    }

    /// Shows `p`, keeping what was typed where the channel is still there.
    fn adopt(&mut self, p: Proposal) {
        let renamed = self.held.as_ref().is_some_and(|old| self.name.trim() != old.name);
        let kept = std::mem::take(&mut self.edits);
        self.edits = p
            .channels
            .iter()
            .map(|c| {
                let text = kept.get(&c.name).cloned().unwrap_or_else(|| c.name.clone());
                (c.name.clone(), text)
            })
            .collect();
        if !renamed {
            self.name.clone_from(&p.name);
        }
        self.refusals = Refusals::default();
        self.held = Some(p);
    }

    /// Takes in an answer to one of this section's calls.
    pub fn answer(&mut self, call: &PCall, status: u16, body: &str) {
        let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        match call {
            PCall::Proposal => self.read_answer(status, &j),
            PCall::ProposalAccept => self.accept_answer(status, &j, body),
            PCall::ProposalPublish => self.publish_answer(status, &j, body),
            PCall::ProposalDiscard => {
                if matches!(status, 200..=299 | 404) {
                    self.forget();
                } else {
                    self.refuse_general(status, &j, "It was not discarded");
                }
            }
            _ => {}
        }
    }

    fn forget(&mut self) {
        self.held = None;
        self.edits.clear();
        self.name.clear();
        self.refusals = Refusals::default();
        self.notice = None;
    }

    fn read_answer(&mut self, status: u16, j: &Json) {
        self.reading = false;
        let conflict = std::mem::take(&mut self.conflict);
        match status {
            200..=299 => {
                let Some(p) = Proposal::parse(j) else { return };
                let same = self.held.as_ref().is_some_and(|h| h.id == p.id);
                if same {
                    if conflict {
                        // The same proposal: the 409 was a name that is taken.
                        let name = self.name.trim().to_string();
                        self.refusals.name = Some(format!(
                            "The project already has a file called {name}.device. Give the device another name: a file that is there is never replaced."
                        ));
                    }
                    return;
                }
                let had = self.held.is_some();
                self.adopt(p);
                if had {
                    self.notice = Some(
                        "Helmsman proposed a newer version. It is shown here, and the names you typed are kept where the channel is still there."
                            .into(),
                    );
                }
            }
            404 => {
                if conflict && self.held.is_some() {
                    self.notice =
                        Some("The proposal is gone: it was discarded on the box. Nothing was saved.".into());
                }
                self.forget_held_only();
            }
            // Nobody signed in, or not allowed to read: nothing to show.
            _ => {}
        }
    }

    fn forget_held_only(&mut self) {
        let notice = self.notice.take();
        self.forget();
        self.notice = notice;
    }

    fn accept_answer(&mut self, status: u16, j: &Json, body: &str) {
        self.saving = false;
        match status {
            201 => {
                let file = j["file"].as_str().map(|s| clean(s, 64, false));
                let project = j["project"].as_str().map(|s| clean(s, 64, false));
                if let (Some(file), Some(project)) = (file, project) {
                    self.reread = Some(project.clone());
                    self.saved = Some(Saved { project, file });
                    self.publishing = Publishing::Idle;
                }
                self.forget();
            }
            409 => {
                // Another proposal, or a file in the way: read it again.
                self.conflict = true;
                self.read();
            }
            422 => self.unprocessable(j),
            403 => self.refusals.general.push(
                "Only an engineer can save a device profile. Sign in as one; nothing was saved.".into(),
            ),
            401 => self.refusals.general.push(
                "The box no longer knows this page's session. Sign in again; nothing was saved.".into(),
            ),
            404 => {
                let why = clean(j["error"].as_str().unwrap_or_default(), 200, false);
                if why.contains("no device profile") {
                    self.notice = Some(
                        "The proposal is gone: it was discarded or replaced on the box. Nothing was saved."
                            .into(),
                    );
                    self.forget_held_only();
                } else {
                    self.refusals.general.push(format!(
                        "Nothing was saved: {}",
                        if why.is_empty() {
                            "the box could not find it"
                        } else {
                            &why
                        }
                    ));
                }
            }
            0 => self.refusals.general.push(format!(
                "The box did not answer; nothing was saved ({}).",
                clean(body, 120, false)
            )),
            s => self.refuse_general(s, j, "Nothing was saved"),
        }
    }

    fn refuse_general(&mut self, status: u16, j: &Json, lead: &str) {
        let why = clean(j["error"].as_str().unwrap_or("no reason given"), 200, false);
        self.refusals
            .general
            .push(format!("{lead}: the box refused it ({status}): {why}"));
    }

    /// A 422: the name, or problems each at a line of the renamed text.
    fn unprocessable(&mut self, j: &Json) {
        let why = clean(j["error"].as_str().unwrap_or_default(), 200, false);
        let problems = j["problems"].as_array().cloned().unwrap_or_default();
        if problems.is_empty() {
            if why.starts_with("the name") {
                self.refusals.name = Some(
                    "A name is 1 to 48 characters: a to z, 0 to 9, _ and -, starting with a letter or a digit.".into(),
                );
            } else {
                self.refusals
                    .general
                    .push(format!("The box did not accept it: {why}"));
            }
            return;
        }
        for p in &problems {
            let line = usize::try_from(p["line"].as_u64().unwrap_or(0)).unwrap_or(0);
            let text = clean(p["error"].as_str().unwrap_or("it does not check"), 200, false);
            let channel = line
                .checked_sub(1)
                .and_then(|k| self.held.as_ref()?.line_channel.get(k)?.clone())
                .filter(|n| self.edits.contains_key(n));
            match channel {
                Some(c) => self.refusals.channels.entry(c).or_default().push(text),
                None => self
                    .refusals
                    .general
                    .push(format!("The profile does not check: {text}")),
            }
        }
    }

    /// The section: the proposal to check, or the note of what was saved.
    /// Nothing at all when the box holds none.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let now = ui.input(|i| i.time);
        self.poll(now);
        if self.ctx.signed_in {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(POLL_S));
        }
        let t = theme(ui);
        if let Some(s) = self.saved.clone() {
            self.saved_ui(ui, chrome, &s);
        }
        let Some(p) = self.held.clone() else {
            // The proposal is gone: say so until it is dismissed.
            if let Some(n) = self.notice.clone() {
                ui.add_space(6.0);
                ui.separator();
                ui.add(egui::Label::new(RichText::new(n).color(AMBER)).wrap());
                let b = ui
                    .add(egui::Button::new("Dismiss").min_size(Vec2::new(0.0, TAP * 0.7)))
                    .tip("Closes this note.");
                chrome.hit("proposal dismiss", b.rect);
                if b.clicked() {
                    self.notice = None;
                }
            }
            return;
        };
        ui.add_space(6.0);
        ui.separator();
        ui.set_max_width(ui.available_width());
        let can = self.ctx.engineer.is_ok();
        let r = ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Proposed device profile").strong().color(t.muted))
                .tip("Helmsman read the vendor's file and proposed this profile. Check the names, then save it into the project.");
        });
        chrome.hit("proposal", r.response.rect);
        if let Some(n) = &self.notice {
            ui.add(egui::Label::new(RichText::new(n).color(AMBER)).wrap());
        }
        let maker = format!("{} {}", p.vendor, p.model).trim().to_string();
        if !maker.is_empty() {
            ui.add(egui::Label::new(RichText::new(maker).strong()).wrap());
        }
        // The device's name.
        // The row's width, taken before it: inside a wrapped row the room
        // left reads as the whole line's.
        let full = ui.available_width();
        ui.horizontal_wrapped(|ui| {
            ui.label("Name").tip("What the program calls the device. Its channels become signals named after it.");
            let w = (full - 70.0 - BAR).clamp(80.0, 240.0);
            let r = ui
                .add_enabled(
                    can,
                    egui::TextEdit::singleline(&mut self.name).desired_width(w).char_limit(64),
                )
                .tip("The device's name in the project: a letter or digit, then letters, digits, _ or -. It is the profile's file name.");
            chrome.hit("proposal name", r.rect);
        });
        if let Some(why) = &self.refusals.name {
            crate::messages::error_link(ui, chrome, "proposal refusal", why, theme(ui).ink(RED));
        }
        // The buttons come before the long parts, so they are in view.
        for line in &self.refusals.general {
            crate::messages::error_link(ui, chrome, "proposal refusal", line, theme(ui).ink(RED));
        }
        self.buttons_ui(ui, chrome, can);
        // What the agent says it read.
        if !p.summary.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("proposal-summary")
                .max_height(SUMMARY_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width() - BAR);
                    ui.add(egui::Label::new(&p.summary).wrap());
                });
        }
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{} channel(s)", p.channels.len())).color(t.muted))
            .tip("Every channel the profile has. Change a name to what the program should call it.");
        self.channels_ui(ui, chrome, &p, can);
    }

    fn channels_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, p: &Proposal, can: bool) {
        let t = theme(ui);
        egui::ScrollArea::vertical()
            .id_salt("proposal-channels")
            .max_height(LIST_H)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - BAR);
                for c in &p.channels {
                    let Some(text) = self.edits.get_mut(&c.name) else {
                        continue;
                    };
                    let mut hover = format!("{}\nProposed as {}.", c.facts(), c.name);
                    for (driver, point) in &c.points {
                        let _ = write!(hover, "\n{driver}: {point}");
                    }
                    ui.horizontal_wrapped(|ui| {
                        let w = (ui.available_width() - BAR - 8.0).clamp(80.0, 190.0);
                        let r = ui
                            .add_enabled(
                                can,
                                egui::TextEdit::singleline(text).desired_width(w).char_limit(32),
                            )
                            .tip(hover.as_str());
                        chrome.hit(format!("proposal channel {}", c.name), r.rect);
                        ui.add(egui::Label::new(RichText::new(c.facts()).color(t.muted)).wrap())
                            .tip(hover.as_str());
                    });
                    if let Some(list) = self.refusals.channels.get(&c.name) {
                        for why in list {
                            crate::messages::error_link(
                                ui,
                                chrome,
                                "proposal refusal",
                                why,
                                theme(ui).ink(RED),
                            );
                        }
                    }
                }
            });
    }

    fn buttons_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, can: bool) {
        ui.add_space(4.0);
        if let Err(why) = &self.ctx.engineer {
            let why = if why.is_empty() {
                "Sign in as an engineer to save or discard it."
            } else {
                why.as_str()
            };
            ui.add(egui::Label::new(why).wrap());
            return;
        }
        let mut save = false;
        let mut discard = false;
        let project = self.ctx.project.clone();
        ui.horizontal_wrapped(|ui| {
            let tall = Vec2::new(0.0, TAP * 0.7);
            let ready = can && project.is_some() && !self.saving;
            let tip = match (&project, self.saving) {
                (_, true) => "Waiting for the box to save it.".to_string(),
                (None, _) => "Open a project in Files first: the profile is saved into it.".to_string(),
                (Some(p), _) => format!(
                    "Saves {}.device as a new file in {p}, with the channel names as typed here. Nothing else in the project changes.",
                    self.name.trim()
                ),
            };
            let b = ui
                .add_enabled(
                    ready,
                    egui::Button::new(RichText::new(if self.saving { "Saving…" } else { "Save profile" }).strong())
                        .min_size(tall),
                )
                .tip(tip);
            chrome.hit("proposal save", b.rect);
            save = b.clicked();
            let d = ui
                .add_enabled(can && !self.saving, egui::Button::new("Discard").min_size(tall))
                .tip("Throws the proposal away. Nothing is saved.");
            chrome.hit("proposal discard", d.rect);
            discard = d.clicked();
        });
        if project.is_none() {
            ui.add(egui::Label::new("Open a project in Files to save the profile into it.").wrap());
        }
        if save {
            self.save();
        } else if discard {
            self.discard();
        }
    }

    /// The note after a save: the file, and its channels as the project
    /// holds them.
    fn saved_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, s: &Saved) {
        let t = theme(ui);
        ui.add_space(6.0);
        ui.separator();
        ui.set_max_width(ui.available_width());
        ui.add(
            egui::Label::new(
                RichText::new(format!("Saved {} in {}.", s.file, s.project))
                    .color(t.accent)
                    .strong(),
            )
            .wrap(),
        )
        .tip("The profile is a file of the project now, like any other. It is listed in Files.");
        match self
            .ctx
            .saved_text
            .as_deref()
            .map(helmsman_project::profile_skim::skim)
        {
            Some(Ok(skim)) => {
                ui.label(
                    RichText::new(format!(
                        "{} channel(s), as the project holds them:",
                        skim.channels.len()
                    ))
                    .color(t.muted),
                )
                .tip("Read back from the saved file.");
                egui::ScrollArea::vertical()
                    .id_salt("proposal-saved")
                    .max_height(LIST_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width() - BAR);
                        for c in skim.channels.iter().take(MAX_CHANNELS) {
                            let mut line = format!(
                                "{}  {}  {}",
                                clean(&c.dir, 8, false),
                                clean(&c.name, 32, false),
                                clean(&c.ty, 8, false)
                            );
                            if let Some(u) = &c.unit {
                                line.push(' ');
                                line.push_str(&clean(u, 16, false));
                            }
                            ui.add(egui::Label::new(RichText::new(line)).wrap());
                        }
                    });
            }
            Some(Err(why)) => {
                crate::messages::error_link(
                    ui,
                    chrome,
                    "proposal refusal",
                    &clean(&why, 200, false),
                    theme(ui).ink(RED),
                );
            }
            None => {
                ui.label(RichText::new("Reading it back from the project…").color(t.muted));
            }
        }
        // Only a profile that was saved and reads back can be sent on.
        if matches!(
            self.ctx
                .saved_text
                .as_deref()
                .map(helmsman_project::profile_skim::skim),
            Some(Ok(_))
        ) {
            self.publish_ui(ui, chrome, s);
        }
        let b = ui
            .add(egui::Button::new("Done").min_size(Vec2::new(0.0, TAP * 0.7)))
            .tip("Closes this note. The profile stays in the project.");
        chrome.hit("proposal done", b.rect);
        if b.clicked() {
            self.saved = None;
            self.publishing = Publishing::Idle;
        }
    }

    /// Sending the saved profile to the device library: one button, a
    /// confirmation that says what goes, and the result.
    fn publish_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, s: &Saved) {
        let t = theme(ui);
        ui.add_space(4.0);
        match self.publishing.clone() {
            Publishing::Idle | Publishing::Refused(_) => {
                if let Publishing::Refused(why) = &self.publishing {
                    crate::messages::error_link(
                        ui,
                        chrome,
                        "proposal refusal",
                        &format!("Not sent: {}.", why.trim_end_matches('.')),
                        theme(ui).ink(RED),
                    );
                }
                let can = self.ctx.engineer.is_ok();
                let b = ui
                    .add_enabled(
                        can,
                        egui::Button::new("Publish to the library…").min_size(Vec2::new(0.0, TAP * 0.7)),
                    )
                    .tip(match &self.ctx.engineer {
                        Ok(()) => "Offers this profile to the public device library, where its maintainers review it before it is added. You confirm first; nothing is sent yet.".to_string(),
                        Err(why) if !why.is_empty() => why.clone(),
                        Err(_) => "Only an engineer can publish a device profile.".to_string(),
                    });
                chrome.hit("proposal publish", b.rect);
                if b.clicked() {
                    self.publishing = Publishing::Confirming;
                }
            }
            Publishing::Confirming => {
                ui.add(
                    egui::Label::new(format!(
                        "Send {} to the public device library? Its maintainers review it before it is added. Only this profile file is sent.",
                        clean(&s.file, 64, false)
                    ))
                    .wrap(),
                );
                ui.horizontal_wrapped(|ui| {
                    let send = ui
                        .add(egui::Button::new("Send for review").min_size(Vec2::new(0.0, TAP * 0.7)))
                        .tip("Sends the profile file as saved in the project. The library's page for it opens when the box answers.");
                    chrome.hit("proposal publish send", send.rect);
                    let cancel = ui
                        .add(egui::Button::new("Cancel").min_size(Vec2::new(0.0, TAP * 0.7)))
                        .tip("Sends nothing.");
                    chrome.hit("proposal publish cancel", cancel.rect);
                    if send.clicked() {
                        self.publish();
                    } else if cancel.clicked() {
                        self.publishing = Publishing::Idle;
                    }
                });
            }
            Publishing::Sending => {
                ui.label(RichText::new("Sending…").color(t.muted));
            }
            Publishing::Sent {
                number,
                link,
                created,
            } => {
                ui.add(
                    egui::Label::new(
                        RichText::new(if created {
                            format!("Sent. The library's maintainers have review request #{number}.")
                        } else {
                            format!("Already sent. Review request #{number} is still open.")
                        })
                        .color(t.accent)
                        .strong(),
                    )
                    .wrap(),
                );
                if let Some(url) = link {
                    let r = ui
                        .hyperlink_to(RichText::new(format!("Open request #{number}")), &url)
                        .tip("Opens the review request on the library's page in a new tab.");
                    chrome.hit("proposal publish link", r.rect);
                }
            }
        }
    }
}
