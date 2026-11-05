//! The Agent tab on a box, free of I/O like [`crate::remote`]: the
//! conversation the page shows, the `POST /api/chat` request it sends,
//! and what it makes of the event stream the box answers with
//! (`helmsman-plc`'s `api::chat`). The browser's `fetch` (`web.rs`)
//! carries the request out and hands back [`Incoming`] pieces; tests
//! carry it out natively against the real box API and a scripted relay.
//!
//! The box runs the agent: the page sends one message (and the session
//! it continues) and shows what streams back, the text, each tool the
//! agent ran, each proposal it stored. Nothing here changes the box: a
//! proposal is a draft that goes through the review and the approval
//! policy like any other (the Proposals screen opens it).
//!
//! - `POST /api/chat` `{message, session}`: same-origin with the session
//!   cookie and `X-Helmsman-Request: 1`, like every mutation the page
//!   makes; an event stream back (`session`, `text`, `tool_call`,
//!   `tool_result`, `proposal`, `retrying`, `status`, `fallback`, `done`,
//!   `error`).
//! - `GET /api/chat/status`: online, offline since a time and why,
//!   unknown, or no relay configured; shown above the conversation.
//!
//! Bounded: a turn keeps at most [`MAX_TEXT`] bytes of text and
//! [`MAX_ITEMS`] tool, proposal and notice rows; the conversation at most
//! [`MAX_TURNS`] turns, the oldest dropped first (the box keeps the
//! model's side of it).

// Natively nothing carries the answers back but the tests.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use eframe::egui;
use serde_json::{Value as Json, json};

use crate::claude::{Event, Sse};
use crate::tips::Tip;

/// The box's longest message, in characters (`api::chat::MAX_MESSAGE`).
pub const MAX_MESSAGE: usize = 8000;
/// Most text kept per turn; a runaway stream is cut here.
pub const MAX_TEXT: usize = 256 * 1024;
/// Most tool, proposal and notice rows per turn.
pub const MAX_ITEMS: usize = 200;
/// Most turns kept on the page.
pub const MAX_TURNS: usize = 50;
/// Longest summary or error shown, in characters.
const MAX_LINE: usize = 600;

pub const CHAT_URL: &str = "/api/chat";
pub const STATUS_URL: &str = "/api/chat/status";

/// The agent as the box reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not asked yet.
    Unread,
    /// No relay on this box (`--relay`): there is no agent to ask.
    Unconfigured(String),
    /// Configured, not reached yet.
    Unknown,
    Online,
    /// Offline: chat reads param edits by rule; the box's words why.
    Offline(String),
}

impl Status {
    /// From `GET /api/chat/status` or a `status` event.
    #[must_use]
    pub fn parse(j: &Json) -> Self {
        let message = clip(j["message"].as_str().unwrap_or(""), MAX_LINE);
        match j["state"].as_str() {
            Some("unconfigured") => Self::Unconfigured(message),
            Some("online") => Self::Online,
            Some("offline") => Self::Offline(if message.is_empty() {
                "the agent is offline".into()
            } else {
                message
            }),
            _ => Self::Unknown,
        }
    }

    /// One line for the top of the tab.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Unread | Self::Unknown => "The agent runs on this box, through its relay.".into(),
            Self::Unconfigured(m) => {
                if m.is_empty() {
                    "This box has no relay configured, so there is no agent to ask.".into()
                } else {
                    format!("No agent on this box: {m}.")
                }
            }
            Self::Online => "The agent is online.".into(),
            Self::Offline(m) => format!(
                "The agent is offline ({m}). A message is read by rule: a param edit becomes a proposal, nothing else runs."
            ),
        }
    }

    /// The status in a few words, for the tab (its tooltip is [`Self::line`]).
    #[must_use]
    pub const fn short(&self) -> &'static str {
        match self {
            Self::Unread => "Agent on this box",
            Self::Unknown => "Agent status unknown",
            Self::Unconfigured(_) => "No agent on this box",
            Self::Online => "Agent online",
            Self::Offline(_) => "Agent offline",
        }
    }

    /// Whether a message can be sent at all.
    #[must_use]
    pub const fn can_send(&self) -> bool {
        !matches!(self, Self::Unconfigured(_))
    }
}

/// A row of a turn other than its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A tool the agent ran; `result` once it answered.
    Tool {
        id: String,
        name: String,
        result: Option<(bool, String)>,
    },
    /// Drafts stored for approval; nothing applied.
    Proposal { ids: Vec<u64>, summary: String },
    /// The relay was retried.
    Retrying(String),
    /// Offline: what the rules made of the message.
    Fallback {
        proposed: Option<u64>,
        kind: Option<String>,
    },
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// `done`: its stop (`end_turn`, `budget`, `refusal`, `offline`) and
    /// the box's words for anything but a plain end.
    Done { stop: String, why: Option<String> },
    /// `error`, a refused request, or a broken stream.
    Failed(String),
    /// Stopped from the page.
    Stopped,
}

/// One message and what came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub user: String,
    pub text: String,
    pub items: Vec<Item>,
    /// `None` while it streams.
    pub end: Option<End>,
    cut: bool,
}

impl Turn {
    fn new(user: String) -> Self {
        Self {
            user,
            text: String::new(),
            items: Vec::new(),
            end: None,
            cut: false,
        }
    }

    fn push_text(&mut self, delta: &str) {
        if self.cut {
            return;
        }
        let room = MAX_TEXT.saturating_sub(self.text.len());
        if delta.len() <= room {
            self.text.push_str(delta);
        } else {
            let mut k = room;
            while !delta.is_char_boundary(k) {
                k -= 1;
            }
            self.text.push_str(&delta[..k]);
            self.text.push_str("\n[the rest of this answer was cut]");
            self.cut = true;
        }
    }

    fn push(&mut self, item: Item) {
        if self.items.len() < MAX_ITEMS {
            self.items.push(item);
        }
    }

    /// Every proposal id this turn stored, in order.
    #[cfg(test)]
    #[must_use]
    pub fn proposals(&self) -> Vec<u64> {
        let mut ids = Vec::new();
        for i in &self.items {
            match i {
                Item::Proposal { ids: p, .. } => ids.extend(p),
                Item::Fallback {
                    proposed: Some(id), ..
                } => ids.push(*id),
                _ => {}
            }
        }
        ids.dedup();
        ids
    }
}

/// The request the page sends; carried out by `web.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Ties the answer's pieces to this turn.
    pub seq: u64,
    pub url: &'static str,
    pub body: String,
}

/// What the transport hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// A status other than 200, and its body.
    Status { seq: u64, status: u16, body: String },
    /// Part of the event stream, as text.
    Chunk { seq: u64, text: String },
    /// The stream ended.
    End { seq: u64 },
    /// The request could not be made or the stream broke.
    Failed { seq: u64, why: String },
}

impl Incoming {
    const fn seq(&self) -> u64 {
        match self {
            Self::Status { seq, .. }
            | Self::Chunk { seq, .. }
            | Self::End { seq }
            | Self::Failed { seq, .. } => *seq,
        }
    }
}

/// The tab's state.
#[derive(Debug)]
pub struct BoxAgent {
    pub status: Status,
    /// The conversation the box holds for this page; `None` until the
    /// first `session` event, and after "New conversation".
    pub session: Option<String>,
    pub turns: Vec<Turn>,
    /// The message being typed.
    pub draft: String,
    seq: u64,
    sse: Option<Sse>,
    /// Asked for `GET /api/chat/status` and not answered yet.
    pub status_asked: bool,
}

impl Default for BoxAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxAgent {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            status: Status::Unread,
            session: None,
            turns: Vec::new(),
            draft: String::new(),
            seq: 0,
            sse: None,
            status_asked: false,
        }
    }

    /// A turn is streaming.
    #[must_use]
    pub const fn busy(&self) -> bool {
        self.sse.is_some()
    }

    /// Why the draft cannot be sent now, if it cannot.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        let n = self.draft.trim().chars().count();
        if self.busy() {
            Some("The agent is still answering.".into())
        } else if !self.status.can_send() {
            Some(self.status.line())
        } else if n == 0 {
            Some("Type a message first.".into())
        } else if n > MAX_MESSAGE {
            Some(format!(
                "A message is at most {MAX_MESSAGE} characters ({n} now)."
            ))
        } else {
            None
        }
    }

    /// Sends the draft: a new turn and the request to carry out, or
    /// `None` (see [`Self::refusal`]). The draft is emptied.
    pub fn send(&mut self) -> Option<Request> {
        if self.refusal().is_some() {
            return None;
        }
        let message = std::mem::take(&mut self.draft).trim().to_string();
        self.seq += 1;
        self.sse = Some(Sse::default());
        if self.turns.len() >= MAX_TURNS {
            self.turns.remove(0);
        }
        self.turns.push(Turn::new(message.clone()));
        Some(Request {
            seq: self.seq,
            url: CHAT_URL,
            body: json!({ "message": message, "session": self.session }).to_string(),
        })
    }

    /// Starts over: the next message opens a new conversation on the box.
    /// Refused while a turn streams.
    pub fn new_conversation(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        self.session = None;
        self.turns.clear();
        true
    }

    /// Stops the streaming turn on the page (the transport aborts the
    /// fetch); what arrives for it afterwards is ignored.
    pub fn stop(&mut self) {
        if self.sse.take().is_some() {
            self.seq += 1;
            if let Some(t) = self.turns.last_mut()
                && t.end.is_none()
            {
                t.end = Some(End::Stopped);
            }
        }
    }

    /// `GET /api/chat/status` answered.
    pub fn status_answer(&mut self, status: u16, body: &str) {
        self.status_asked = false;
        if status == 200
            && let Ok(j) = serde_json::from_str::<Json>(body)
        {
            self.status = Status::parse(&j);
        }
    }

    /// What the transport handed back for a request.
    pub fn apply(&mut self, inc: Incoming) {
        if inc.seq() != self.seq || self.sse.is_none() {
            return;
        }
        match inc {
            Incoming::Status { status, body, .. } => {
                let why = error_text(status, &body);
                if status == 503 && body.contains("no relay configured") {
                    self.status = Status::Unconfigured(String::new());
                }
                self.fail(why);
            }
            Incoming::Chunk { text, .. } => {
                let events = match self.sse.as_mut().map(|s| s.push(&text)) {
                    Some(Ok(e)) => e,
                    Some(Err(why)) => {
                        self.fail(why);
                        return;
                    }
                    None => return,
                };
                for e in events {
                    self.event(&e);
                    if self.sse.is_none() {
                        return;
                    }
                }
            }
            Incoming::End { .. } => {
                // A last event without its blank line still counts.
                if let Some(mut sse) = self.sse.take()
                    && let Ok(events) = sse.push("\n\n")
                {
                    for e in events {
                        self.event(&e);
                    }
                }
                if let Some(t) = self.turns.last_mut()
                    && t.end.is_none()
                {
                    t.end = Some(End::Failed(
                        "the box closed the stream before the turn ended".into(),
                    ));
                }
            }
            Incoming::Failed { why, .. } => self.fail(clip(&why, MAX_LINE)),
        }
    }

    fn fail(&mut self, why: String) {
        self.sse = None;
        if let Some(t) = self.turns.last_mut()
            && t.end.is_none()
        {
            t.end = Some(End::Failed(why));
        }
    }

    fn event(&mut self, e: &Event) {
        let Ok(d) = serde_json::from_str::<Json>(&e.data) else {
            return;
        };
        if e.name == "session" {
            if let Some(id) = d["id"].as_str().filter(|s| s.len() <= 64) {
                self.session = Some(id.to_string());
            }
            return;
        }
        if e.name == "status" {
            self.status = Status::parse(&d);
            return;
        }
        let Some(t) = self.turns.last_mut() else { return };
        if t.end.is_some() {
            return;
        }
        let s = |k: &str| clip(d[k].as_str().unwrap_or(""), MAX_LINE);
        match e.name.as_str() {
            "text" => t.push_text(d["delta"].as_str().unwrap_or("")),
            "tool_call" => t.push(Item::Tool {
                id: s("id"),
                name: s("name"),
                result: None,
            }),
            "tool_result" => {
                let (id, ok, summary) = (s("id"), d["ok"].as_bool().unwrap_or(false), s("summary"));
                let open = t.items.iter_mut().rev().find_map(|i| match i {
                    Item::Tool { id: i_id, result, .. } if *i_id == id && result.is_none() => Some(result),
                    _ => None,
                });
                match open {
                    Some(r) => *r = Some((ok, summary)),
                    None => t.push(Item::Tool {
                        id,
                        name: s("name"),
                        result: Some((ok, summary)),
                    }),
                }
            }
            "proposal" => {
                let ids: Vec<u64> = d["ids"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Json::as_u64).take(MAX_ITEMS).collect())
                    .unwrap_or_default();
                if !ids.is_empty() {
                    t.push(Item::Proposal {
                        ids,
                        summary: s("summary"),
                    });
                }
            }
            "retrying" => t.push(Item::Retrying(s("error"))),
            "fallback" => t.push(Item::Fallback {
                proposed: d["proposal"].as_u64(),
                kind: d["kind"].as_str().map(|k| clip(k, 64)),
            }),
            "done" => {
                let stop = clip(d["stop"].as_str().unwrap_or("end_turn"), 32);
                let why = d["why"].as_str().map(|w| clip(w, MAX_LINE));
                t.end = Some(End::Done { stop, why });
                self.sse = None;
            }
            "error" => {
                let m = s("message");
                t.end = Some(End::Failed(if m.is_empty() {
                    "the turn failed".into()
                } else {
                    m
                }));
                self.sse = None;
            }
            _ => {}
        }
    }
}

/// A refused request in words: the box's `{"error": ...}` when it sent
/// one, else the status.
fn error_text(status: u16, body: &str) -> String {
    let said = serde_json::from_str::<Json>(body)
        .ok()
        .and_then(|j| j["error"].as_str().map(|s| clip(s, MAX_LINE)));
    match (status, said) {
        (401, _) => "Sign in (top right) to talk to the agent: the session ended or was never opened.".into(),
        (403, Some(s)) => format!("The box refused: {s}"),
        (_, Some(s)) => s,
        (0, None) => "the box did not answer".into(),
        (s, None) => format!("the box answered {s}"),
    }
}

/// At most `n` characters of `s`, control characters but newlines and
/// tabs removed (the box's words reach the screen as they are).
fn clip(s: &str, n: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(n)
        .collect()
}

// ------------------------------------------------------------------ view

/// What the tab asks its owner to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Send,
    Stop,
    New,
    /// Open this proposal on the Proposals screen.
    Review(u64),
}

/// Draws the Agent tab; `who` is why this page may not send (not signed
/// in), shown as the message field's hint and the Send button's tooltip.
pub fn ui(
    ui: &mut egui::Ui,
    chrome: &mut crate::editor::Chrome,
    a: &mut BoxAgent,
    who: Option<&str>,
) -> Option<Act> {
    use egui::{RichText, Vec2};

    use crate::st_view::TAP;
    use crate::st_view::brand::{BLACK, RED, YELLOW};

    let t = crate::theme::theme(ui);
    let mut act = None;
    egui::ScrollArea::vertical().id_salt("box-agent").show(ui, |ui| {
        // Room for the scroll bar (a resizable panel grows to any row
        // that overflows it).
        ui.set_max_width(ui.available_width() - 12.0);
        let l = ui
            .label(RichText::new(a.status.short()).color(t.muted))
            .tip(format!(
                "{} Every change it makes is a proposal: you review and approve it under Proposals.",
                a.status.line()
            ));
        chrome.hit("agent status", l.rect);
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::multiline(&mut a.draft)
                .id_salt("box-agent-message")
                .hint_text(who.unwrap_or("e.g. Why does the conveyor stop after a jam clears?"))
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        )
        .tip(format!(
            "Your message to the agent on this box, at most {MAX_MESSAGE} characters."
        ));
        ui.horizontal_wrapped(|ui| {
            let send = egui::Button::new(RichText::new("Send").strong().color(BLACK))
                .fill(YELLOW)
                .min_size(Vec2::new(TAP * 1.5, TAP * 0.75));
            let why = who.map(str::to_string).or_else(|| a.refusal());
            let r = ui
                .add_enabled(why.is_none(), send)
                .tip(why.unwrap_or_else(|| "Sends the message to the agent on this box.".to_string()));
            chrome.hit("agent send", r.rect);
            if r.clicked() {
                act = Some(Act::Send);
            }
            if a.busy() {
                let r = ui.button("Stop").tip("Stops the answer under way.");
                chrome.hit("agent stop", r.rect);
                if r.clicked() {
                    act = Some(Act::Stop);
                }
            } else if !a.turns.is_empty() {
                let r = ui
                    .button("New conversation")
                    .tip("The next message starts a conversation of its own.");
                chrome.hit("agent new", r.rect);
                if r.clicked() {
                    act = Some(Act::New);
                }
            }
        });
        ui.separator();
        for (k, turn) in a.turns.iter().enumerate().rev() {
            ui.push_id(("box-agent-turn", k), |ui| {
                ui.label(RichText::new(&turn.user).strong());
                if !turn.text.is_empty() {
                    for seg in crate::claude::segments(&turn.text) {
                        match seg {
                            crate::claude::Segment::Prose(p) => {
                                ui.label(p.trim());
                            }
                            crate::claude::Segment::Code { body, .. } => {
                                ui.label(RichText::new(body.trim_end()).monospace());
                            }
                        }
                    }
                }
                for item in &turn.items {
                    match item {
                        Item::Tool { name, result, .. } => {
                            let (mark, words) = match result {
                                None => ("…", String::new()),
                                Some((true, s)) => ("✓", s.clone()),
                                Some((false, s)) => ("✗", s.clone()),
                            };
                            ui.label(
                                RichText::new(format!("{mark} {name}  {words}"))
                                    .small()
                                    .color(t.muted),
                            );
                        }
                        Item::Proposal { ids, summary } => {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(format!("Proposed: {summary}")).strong());
                                for id in ids {
                                    let r = ui
                                        .small_button(format!("Review #{id}"))
                                        .tip("Opens this proposal under Proposals.");
                                    chrome.hit("agent review", r.rect);
                                    if r.clicked() {
                                        act = Some(Act::Review(*id));
                                    }
                                }
                            });
                        }
                        Item::Retrying(e) => {
                            ui.label(RichText::new(format!("retrying: {e}")).small().color(t.muted));
                        }
                        Item::Fallback { proposed, .. } => {
                            if let Some(id) = proposed {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("Read by rule: a param proposal.")
                                        .tip("The agent was offline, so the box read the message by rule.");
                                    let r = ui
                                        .small_button(format!("Review #{id}"))
                                        .tip("Opens this proposal under Proposals.");
                                    chrome.hit("agent review", r.rect);
                                    if r.clicked() {
                                        act = Some(Act::Review(*id));
                                    }
                                });
                            }
                        }
                    }
                }
                match &turn.end {
                    None => {
                        ui.label(RichText::new("answering…").color(t.muted));
                    }
                    Some(End::Done { why: Some(w), .. }) => {
                        ui.label(RichText::new(w).small().color(t.muted));
                    }
                    Some(End::Done { .. }) => {}
                    Some(End::Failed(w)) => {
                        ui.colored_label(RED, w);
                    }
                    Some(End::Stopped) => {
                        ui.label(RichText::new("stopped").small().color(t.muted));
                    }
                }
                ui.separator();
            });
        }
    });
    act
}
