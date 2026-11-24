//! A Skipper's I/O map, edited and sent from the Devices tab (card
//! node-comms-check, part 2; docs/EDGE.md "A map is saved once its devices
//! answered the node").
//!
//! A Skipper keeps no projects, so the project editor cannot change its
//! map. Instead the tab has one row, "Skipper map", whose button opens a
//! window: the map's text to edit, **Check devices** (the Skipper asks
//! every device in the text from its own port, `POST /api/io_map/check`,
//! read back with `GET`), and **Download** (`PUT /api/io_map`), which is
//! offered only when the last check passed and was of exactly the text in
//! the window. The Skipper enforces the same rule; this is so the engineer
//! is not offered a button that will be refused.
//!
//! Every call is a [`PReq`], so it goes where Deploy goes: to the origin of
//! `?box=` with the token the sign-in panel holds. Every answer comes back
//! through [`NodeMapUi::answer`], which is the one place a status is read.

use eframe::egui::{self, Color32, RichText};
use serde_json::{Value as Json, json};

use crate::box_projects::{PCall, PReq};
use crate::editor::Chrome;
use crate::theme::theme;
use crate::tips::Tip;

const RED: Color32 = Color32::from_rgb(255, 110, 100);

/// The longest map text taken from the Skipper's check reply: far above any
/// map a board holds, so a hostile or broken reply cannot fill the page.
const MAX_TEXT: usize = 64 * 1024;
/// Most devices read from one reply.
const MAX_DEVICES: usize = 128;
const MAX_NAME: usize = 48;
const MAX_ADDRESS: usize = 64;
const MAX_WORDS: usize = 240;

/// How often the check is read while it runs, and how long it is waited for
/// (a device that does not answer takes the Skipper's own timeout).
const POLL_EVERY: f64 = 1.0;
const POLL_FOR: f64 = 120.0;

const CHECK_URL: &str = "/api/io_map/check";
const MAP_URL: &str = "/api/io_map";

/// What a call is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeOp {
    /// `POST /api/io_map/check`: start asking the devices.
    Check,
    /// `GET /api/io_map/check`: the last check, or the one running.
    Poll,
    /// `PUT /api/io_map`: download the map and restart.
    Save,
}

/// Text from the Skipper, cut to printable characters and a length.
fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

/// What one device said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Waiting,
    /// Answered, in this many milliseconds when it said.
    Answered(Option<u32>),
    /// Did not answer; the Skipper's own words.
    Failed(String),
}

impl Verdict {
    /// The row's words.
    #[must_use]
    pub fn words(&self) -> String {
        match self {
            Self::Waiting => "waiting".into(),
            Self::Answered(Some(ms)) => format!("answered in {ms} ms"),
            Self::Answered(None) => "answered".into(),
            Self::Failed(why) if why.is_empty() => "did not answer".into(),
            Self::Failed(why) => format!("did not answer: {why}"),
        }
    }
}

/// One device of the checked map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCheck {
    pub name: String,
    pub address: String,
    pub verdict: Verdict,
}

/// Where the Skipper's check stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// None was ever run.
    None,
    Running,
    Done,
}

/// `GET /api/io_map/check`, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub stage: Stage,
    pub passed: bool,
    /// The Skipper's word that this text may be downloaded now.
    pub may_save: bool,
    pub age_ms: Option<u64>,
    /// The text that was checked, whole.
    pub text: String,
    pub devices: Vec<DeviceCheck>,
}

impl Report {
    /// Reads the Skipper's answer. `None` when it is not a check at all (no
    /// stage, or one this page does not know, or no text): nothing is then
    /// guessed, and no download is offered on it. A device that is missing
    /// or odd reads as one that did not answer.
    #[must_use]
    pub fn parse(j: &Json) -> Option<Self> {
        let stage = match j["state"].as_str()? {
            "none" => Stage::None,
            "running" => Stage::Running,
            "done" => Stage::Done,
            _ => return None,
        };
        let text = j["text"].as_str().filter(|t| t.len() <= MAX_TEXT)?.to_string();
        let devices = j["devices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|d| d.is_object())
                    .take(MAX_DEVICES)
                    .map(|d| {
                        let verdict = match d["state"].as_str() {
                            Some("waiting") => Verdict::Waiting,
                            Some("answered") => {
                                Verdict::Answered(d["ms"].as_u64().and_then(|m| u32::try_from(m).ok()))
                            }
                            Some("failed") => {
                                Verdict::Failed(clean(d["error"].as_str().unwrap_or(""), MAX_WORDS))
                            }
                            _ => Verdict::Failed("the Skipper's answer was not understood".into()),
                        };
                        DeviceCheck {
                            name: clean(d["name"].as_str().unwrap_or("?"), MAX_NAME),
                            address: clean(d["address"].as_str().unwrap_or(""), MAX_ADDRESS),
                            verdict,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            stage,
            passed: j["passed"].as_bool() == Some(true),
            may_save: j["may_save"].as_bool() == Some(true),
            age_ms: j["age_ms"].as_u64(),
            text,
            devices,
        })
    }
}

/// The Skipper's reason in an error reply (`{"error": "..."}`, or plain
/// text), as one bounded line of printable characters.
fn why(body: &str) -> String {
    let words = serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|j| j["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.to_string());
    clean(words.trim(), 300)
}

/// A line to say, and whether it is a problem.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Note {
    bad: bool,
    text: String,
}

/// What the page is waiting for.
#[derive(Debug, Clone, PartialEq, Default)]
enum Job {
    #[default]
    Idle,
    /// The check of `text` is asked for; the Skipper has not said yes.
    Starting(String),
    /// The check runs. `text`: the one to wait for (`None`: another
    /// person's check was already running, so any result will do).
    Polling {
        text: Option<String>,
        since: Option<f64>,
        last: Option<f64>,
        in_flight: bool,
    },
    /// The download is asked for.
    Saving,
}

/// The state of the Skipper-map window and its row. Its five flags are
/// independent facts (open, a Skipper, a token, no check route, probed),
/// not the states of one machine, which is what the lint suggests.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct NodeMapUi {
    /// The window is open.
    pub open: bool,
    /// The edited text, when it differs from the Skipper's.
    draft: Option<String>,
    /// The map the Skipper runs, as last read (the view sets it each frame).
    running: String,
    /// The page watches a Skipper.
    skipper: bool,
    /// The page holds a token for it.
    signed_in: bool,
    report: Option<Report>,
    /// The Skipper answered 404 to the check route: it saves a map without
    /// asking its devices.
    no_check: bool,
    probed: bool,
    job: Job,
    note: Option<Note>,
    /// The Skipper's reason for refusing the text (422, with the line).
    reason: Option<String>,
    /// The text just downloaded, until the Skipper is seen running it.
    restarting: Option<String>,
    queue: Vec<PReq>,
}

const NEED_TOKEN: &str =
    "Enter the Skipper's token in the sign-in panel (top right) to check and download a map.";

impl NodeMapUi {
    /// What the view knows each frame: whether this is a Skipper, whether
    /// a token is held for it, and the map it runs.
    pub fn set_context(&mut self, skipper: bool, signed_in: bool, running: Option<&str>) {
        self.skipper = skipper;
        self.signed_in = signed_in;
        if !skipper {
            self.open = false;
        }
        if let Some(t) = running {
            if self.running != t {
                self.running = t.to_string();
            }
            if self.restarting.as_deref() == Some(t) {
                self.restarting = None;
                self.note = Some(Note {
                    bad: false,
                    text: "The Skipper runs this map.".into(),
                });
            }
        }
        if self.draft.as_deref() == Some(self.running.as_str()) {
            self.draft = None;
        }
    }

    /// The text the window holds: the edit, or the Skipper's own.
    #[must_use]
    pub fn text(&self) -> &str {
        self.draft.as_deref().unwrap_or(&self.running)
    }

    /// The text differs from what the Skipper runs.
    #[must_use]
    pub const fn edited(&self) -> bool {
        self.draft.is_some()
    }

    /// The window's text changed: what was said about the old one no longer
    /// applies.
    pub fn edit(&mut self, text: String) {
        if text == self.text() {
            return;
        }
        self.draft = (text != self.running).then_some(text);
        self.reason = None;
        self.note = None;
    }

    /// Throws the edits away.
    pub fn discard(&mut self) {
        self.draft = None;
        self.reason = None;
        self.note = None;
    }

    /// The last check as read.
    #[cfg(test)]
    #[must_use]
    pub const fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }

    /// The Skipper does not check (it answered 404).
    #[cfg(test)]
    #[must_use]
    pub const fn no_check(&self) -> bool {
        self.no_check
    }

    /// What the window last said, as text.
    #[cfg(test)]
    #[must_use]
    pub fn note_text(&self) -> Option<&str> {
        self.note.as_ref().map(|n| n.text.as_str())
    }

    /// The Skipper's reason for refusing the text, as shown.
    #[cfg(test)]
    #[must_use]
    pub fn reason_text(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    fn busy(&self) -> bool {
        self.job != Job::Idle
    }

    /// Whether the text may be checked now, or why not.
    ///
    /// # Errors
    /// What is missing.
    pub fn can_check(&self) -> Result<(), String> {
        if self.no_check {
            return Err(
                "This Skipper saves a map without asking its devices, so there is nothing to check.".into(),
            );
        }
        if !self.signed_in {
            return Err(NEED_TOKEN.into());
        }
        if self.busy() {
            return Err("Wait for the Skipper to finish.".into());
        }
        if self.text().trim().is_empty() {
            return Err("The map is empty.".into());
        }
        Ok(())
    }

    /// Download is offered: the Skipper said this text may be saved, and
    /// the text in the window is exactly the one it checked. One more
    /// character in the window takes it away.
    #[must_use]
    pub fn can_download(&self) -> bool {
        self.signed_in
            && !self.no_check
            && !self.busy()
            && self
                .report
                .as_ref()
                .is_some_and(|r| r.may_save && r.text == self.text())
    }

    /// Asks the Skipper to check the window's text.
    pub fn check(&mut self) {
        if self.can_check().is_err() {
            return;
        }
        let text = self.text().to_string();
        self.reason = None;
        self.note = None;
        self.queue.push(PReq::json(
            PCall::NodeMap(NodeOp::Check),
            "POST",
            CHECK_URL.into(),
            &json!({ "text": text }),
        ));
        self.job = Job::Starting(text);
    }

    /// Downloads the window's text, when [`Self::can_download`].
    pub fn download(&mut self) {
        if !self.can_download() {
            return;
        }
        self.reason = None;
        self.note = None;
        self.queue.push(PReq::json(
            PCall::NodeMap(NodeOp::Save),
            "PUT",
            MAP_URL.into(),
            &json!({ "text": self.text() }),
        ));
        self.job = Job::Saving;
    }

    /// Once a frame, with the clock: reads the check while it runs (once a
    /// second, for two minutes), and asks once, when the window opens,
    /// whether this Skipper checks at all.
    pub fn tick(&mut self, now: f64) {
        if self.open && self.skipper && !self.probed {
            self.probed = true;
            self.queue
                .push(PReq::get(PCall::NodeMap(NodeOp::Poll), CHECK_URL.into()));
        }
        let Job::Polling {
            since,
            last,
            in_flight,
            ..
        } = &mut self.job
        else {
            return;
        };
        let began = *since.get_or_insert(now);
        if now - began > POLL_FOR {
            self.job = Job::Idle;
            self.note = Some(Note {
                bad: true,
                text: "The Skipper did not finish the check in two minutes. Check again.".into(),
            });
            return;
        }
        if !*in_flight && last.is_none_or(|l| now - l >= POLL_EVERY) {
            *in_flight = true;
            *last = Some(now);
            self.queue
                .push(PReq::get(PCall::NodeMap(NodeOp::Poll), CHECK_URL.into()));
        }
    }

    /// The calls to make.
    pub fn take_requests(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    /// The page is waiting on the Skipper, so a frame is wanted soon.
    #[must_use]
    pub fn waiting(&self) -> bool {
        self.busy()
    }

    fn say(&mut self, bad: bool, text: impl Into<String>) {
        self.note = Some(Note {
            bad,
            text: text.into(),
        });
    }

    fn refused_token(&mut self) {
        self.say(
            true,
            "The Skipper did not take the token. Enter it again in the sign-in panel (top right).",
        );
    }

    /// Takes the Skipper's answer to a call.
    pub fn answer(&mut self, op: NodeOp, status: u16, body: &str) {
        match op {
            NodeOp::Check => self.checked(status, body),
            NodeOp::Poll => self.polled(status, body),
            NodeOp::Save => self.saved(status, body),
        }
    }

    fn checked(&mut self, status: u16, body: &str) {
        let sent = match std::mem::take(&mut self.job) {
            Job::Starting(t) => Some(t),
            other => {
                self.job = other;
                return;
            }
        };
        let polling = |text| Job::Polling {
            text,
            since: None,
            last: None,
            in_flight: false,
        };
        match status {
            202 => self.job = polling(sent),
            409 => {
                self.say(
                    false,
                    "A check is already running on the Skipper. Its result is below when it ends; check again then.",
                );
                self.job = polling(None);
            }
            422 => self.reason = Some(why(body)),
            404 => {
                self.no_check = true;
                self.report = None;
            }
            401 | 403 => self.refused_token(),
            0 => self.say(true, format!("The Skipper did not answer: {}", clean(body, 200))),
            s => self.say(
                true,
                format!("The Skipper did not start the check (HTTP {s}): {}", why(body)),
            ),
        }
    }

    fn polled(&mut self, status: u16, body: &str) {
        if let Job::Polling { in_flight, .. } = &mut self.job {
            *in_flight = false;
        }
        match status {
            200 => {}
            404 => {
                self.no_check = true;
                self.report = None;
                self.job = Job::Idle;
                return;
            }
            // A read that failed is tried again, until the deadline.
            _ => return,
        }
        let Some(r) = serde_json::from_str::<Json>(body)
            .ok()
            .as_ref()
            .and_then(Report::parse)
        else {
            if self.busy() {
                self.job = Job::Idle;
            }
            self.say(true, "The Skipper's answer about the check could not be read.");
            return;
        };
        match &self.job {
            Job::Polling { text, .. } => {
                // The result of an earlier text is not this one's.
                if text.as_ref().is_some_and(|t| *t != r.text) {
                    return;
                }
                let ended = r.stage != Stage::Running;
                self.report = Some(r);
                if ended {
                    self.job = Job::Idle;
                }
            }
            Job::Idle => self.report = Some(r),
            Job::Starting(_) | Job::Saving => {}
        }
    }

    fn saved(&mut self, status: u16, body: &str) {
        if self.job != Job::Saving {
            return;
        }
        let text = self.text().to_string();
        self.job = Job::Idle;
        match status {
            200 => {
                self.report = None;
                self.restarting = Some(text);
                self.say(
                    false,
                    "Downloaded. The Skipper is restarting with this map; this page reads it again when the Skipper is back.",
                );
            }
            409 => {
                let j = serde_json::from_str::<Json>(body).ok();
                if let Some(r) = j.as_ref().and_then(|j| Report::parse(&j["check"])) {
                    self.report = Some(r);
                }
                let reason = j
                    .as_ref()
                    .and_then(|j| j["error"].as_str().map(|e| clean(e, 300)))
                    .unwrap_or_else(|| why(body));
                self.say(true, format!("The Skipper did not take the map: {reason}"));
            }
            422 => self.reason = Some(why(body)),
            401 | 403 => self.refused_token(),
            0 => self.say(true, format!("The Skipper did not answer: {}", clean(body, 200))),
            s => self.say(
                true,
                format!("The Skipper did not take the map (HTTP {s}): {}", why(body)),
            ),
        }
    }

    /// The Devices tab's row: where the Skipper's map is opened.
    pub fn row(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        if !self.skipper {
            return;
        }
        let t = theme(ui);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Skipper map").strong().color(t.muted))
                .tip("The I/O map this Skipper runs: edit it, check its devices, download it.");
            let b = ui
                .button("Edit map…")
                .tip("Opens the map in a window, where the Skipper can ask each device in it.");
            chrome.hit("node map open", b.rect);
            if b.clicked() {
                self.open = true;
            }
        });
    }

    /// The window, while open.
    pub fn window(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        if !self.open || !self.skipper {
            return;
        }
        self.tick(ctx.input(|i| i.time));
        if self.waiting() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        let mut open = true;
        let w = egui::Window::new("Skipper map")
            .id(egui::Id::unique("node-map-window"))
            .open(&mut open)
            .collapsible(false)
            .default_width(480.0)
            .constrain(true)
            .show(ctx, |ui| self.body(ui, chrome));
        if let Some(w) = w {
            chrome.hit("node map window", w.response.rect);
        }
        if !open {
            self.open = false;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if self.no_check {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "This Skipper saves a map without asking its devices, so there is no check here. Change its map on the Skipper's own page.",
                    )
                    .color(t.muted),
                )
                .wrap(),
            )
            .tip("This board takes a map straight away; it has no device check to run.");
        } else if !self.signed_in {
            ui.add(egui::Label::new(RichText::new(NEED_TOKEN).color(t.ink(RED))).wrap())
                .tip("The Skipper takes a map only from someone holding its token.");
        }
        let mut buf = self.text().to_string();
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let out = egui::TextEdit::multiline(&mut buf)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(10)
                    .char_limit(MAX_TEXT)
                    .lock_focus(true)
                    .interactive(self.signed_in && !self.no_check && !self.busy())
                    .show(ui)
                    .tip("The Skipper's I/O map, one line each. A change needs a new check before Download.");
                chrome.hit("node map text", out.response.rect);
            });
        self.edit(buf);
        if let Some(why) = &self.reason {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("The Skipper does not take this map: {why}")).color(t.ink(RED)),
                )
                .wrap(),
            )
            .tip("The Skipper read the map and refused it. The line it names is the one to change.");
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let checkable = self.can_check();
            let b = ui
                .add_enabled(checkable.is_ok(), egui::Button::new("Check devices"))
                .tip(match &checkable {
                    Ok(()) => "The Skipper asks each device in this map from its own port. Nothing is written to a device.".to_string(),
                    Err(why) => why.clone(),
                });
            chrome.hit("node map check", b.rect);
            if b.clicked() {
                self.check();
            }
            if self.can_download() {
                let d = ui
                    .add(egui::Button::new(RichText::new("Download").strong()))
                    .tip("Sends this map to the Skipper, which saves it and restarts to run it.");
                chrome.hit("node map download", d.rect);
                if d.clicked() {
                    self.download();
                }
            }
            if self.edited() {
                let x = ui
                    .add_enabled(!self.busy(), egui::Button::new("Discard edits"))
                    .tip("Puts back the map the Skipper runs.");
                chrome.hit("node map discard", x.rect);
                if x.clicked() {
                    self.discard();
                }
            }
        });
        if !self.no_check && self.signed_in && !self.can_download() && !self.busy() {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Download appears once every device has answered a check of this exact text.",
                    )
                    .color(t.muted),
                )
                .wrap(),
            )
            .tip("The Skipper saves a map only after it has reached every device in it.");
        }
        if let Some(n) = &self.note {
            let c = if n.bad { t.ink(RED) } else { t.fg };
            ui.add(egui::Label::new(RichText::new(&n.text).color(c)).wrap())
                .tip("What the Skipper last said.");
            chrome.hit("node map note", ui.min_rect());
        }
        if self.busy() {
            ui.add(egui::Label::new(RichText::new("Waiting for the Skipper…").color(t.muted)).wrap())
                .tip("The Skipper is working on the last request.");
        }
        self.devices_ui(ui, chrome);
    }

    fn devices_ui(&self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let Some(r) = &self.report else {
            return;
        };
        if r.devices.is_empty() {
            return;
        }
        let t = theme(ui);
        ui.separator();
        if r.text != self.text() {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "This is the result for an earlier text. Check the devices again for the text above.",
                    )
                    .color(t.muted),
                )
                .wrap(),
            )
            .tip("The Skipper's last check was of a map that is not the one in the window.");
        }
        for d in &r.devices {
            let (dot, color) = match &d.verdict {
                Verdict::Answered(_) => ("●", t.accent),
                Verdict::Failed(_) => ("●", t.ink(RED)),
                Verdict::Waiting => ("○", t.muted),
            };
            let row = ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(dot).color(color));
                ui.label(RichText::new(&d.name).strong());
                if !d.address.is_empty() {
                    ui.label(RichText::new(&d.address).color(t.muted));
                }
                ui.add(egui::Label::new(RichText::new(d.verdict.words()).color(color)).wrap());
            });
            chrome.hit(format!("node map device {}", d.name), row.response.rect);
            row.response
                .on_hover_text("What this device said when the Skipper asked it.");
        }
    }
}
