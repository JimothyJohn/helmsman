//! The design step on the Agent tab (card `design-step-canvas`): the
//! engineer describes a whole machine, a strong model writes the design
//! (`crate::design`), the page shows it, keeps it with the project as
//! [`DESIGN_FILE`] and the Agent builds from it a batch of steps at a time.
//!
//! This is the state and the streams, free of I/O like `crate::box_agent`:
//! the page (`playground_view.rs`) and the box's page (`box_view.rs`) carry
//! the requests out and hand the answer back as [`Feed`]s; the screens are
//! `crate::design_ui`.
//!
//! - **Two routes, one entry.** On a box the page posts `{description}` to
//!   `POST /api/design` and `{session}` again while `done.more` is true
//!   (helmsman-plc's `api::design`; the box adds the dialect and its
//!   project). In the simulation the page asks the relay's design entry
//!   itself, a second address ([`DESIGN_URL`], `HELMSMAN_DESIGN_URL` when
//!   the page is built; unset, there is no entry): `POST
//!   {url}/demo/v1/messages` with `{"agent": "design", "messages"}`, a
//!   continued design sending the first request again, the answer as
//!   blocks exactly as the stream gave them (the thinking block and its
//!   signature included) and the one word `Continue` (agent-relay
//!   docs/demo.md, "The design entry").
//! - **Documents.** The description can be left out or be short when the
//!   engineer pastes or adds `.md` / `.txt` documents in the same window
//!   ([`crate::documents`]): [`Designer::docs`] holds them, the request
//!   carries them inside the defanged `<specification>` envelope after the
//!   description, and [`Read::documents`] keeps them for the project
//!   (`documents.md`).
//! - **A turn is minutes.** The elapsed time and the thinking summary come
//!   from [`Designer::progress`]; a stream that ends without the relay's
//!   `relay_usage` (or the box's `done`) was cut off and says so, and the
//!   engineer carries on or discards. Stop leaves no draft.
//! - **A design is several turns.** A turn that stopped at `max_tokens`
//!   is followed by the next one at once; the answers put end to end are the
//!   design ([`crate::design::join`]). The relay takes at most
//!   [`MAX_MESSAGES`] messages and [`MAX_BODY`] bytes: past them the page
//!   says the design is too large for the playground.
//! - **Building.** [`batch`] hands the design's build order to the steps
//!   run ([`crate::instruct::Instruct::start_design_steps`]) at most
//!   [`crate::steps::MAX_STEPS`] steps at a time.

use serde_json::{Value as Json, json};

use crate::claude::{Event, Sse};
use crate::design::{self, Design};

/// The design in the project, kept beside the documents it was made from
/// ([`crate::documents::DOCUMENTS_FILE`]).
pub const DESIGN_FILE: &str = "design.md";
/// The relay's design entry, set when the page is built
/// (`HELMSMAN_DESIGN_URL`, no trailing slash). Unset: the simulation has no
/// design entry.
pub const DESIGN_URL: Option<&str> = option_env!("HELMSMAN_DESIGN_URL");
/// The relay's key for the design entry.
pub const AGENT: &str = "design";
/// Most messages the relay takes in a design's request.
pub const MAX_MESSAGES: usize = 9;
/// Largest request body the relay takes, bytes.
pub const MAX_BODY: usize = 1024 * 1024;
/// Longest description of a machine, in characters.
pub const MAX_DESCRIPTION: usize = 8_000;
/// The most one turn's blocks may hold, bytes: a runaway stream is cut.
const MAX_TURN: usize = 700 * 1024;
/// Where the box takes a design.
pub const BOX_URL: &str = "/api/design";
/// The playground's note when a design does not fit the relay's request.
pub const TOO_LARGE: &str = "This design is too large for the playground: the design entry takes at most five turns and 1 MiB of a design in one go. Describe a smaller machine, or one part of it.";
/// The one word that asks for the rest of a design.
const CONTINUE: &str = design::CONTINUE;

/// Where the design entry's requests go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// The simulation in the page: the relay's design entry.
    Page,
    /// A box's page: `POST /api/design`.
    Box,
}

/// What the design entry is, as far as the page knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Avail {
    /// Not asked yet.
    Unknown,
    /// Asked; no answer yet.
    Asking,
    /// Not served here (no address, the relay does not serve it, or it
    /// could not be asked): the page shows no entry.
    Hidden,
    Open,
    /// Closed. `reopens`: it comes back by itself.
    Closed {
        reopens: bool,
    },
}

impl Avail {
    /// From the design entry's status reply: `{"open": true, "agents":
    /// ["design"], "design": {"open": true}}`, or closed with
    /// `reopens_by_itself`. A reply that does not say, or that does not
    /// list the agent while open, is not served.
    #[must_use]
    pub fn from_status(body: &str) -> Self {
        let Ok(v) = serde_json::from_str::<Json>(body) else {
            return Self::Hidden;
        };
        match v["design"]["open"].as_bool() {
            Some(true) => {
                let listed = v["agents"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|s| s == AGENT));
                if listed { Self::Open } else { Self::Hidden }
            }
            Some(false) => Self::Closed {
                reopens: v["design"]["reopens_by_itself"]
                    .as_bool()
                    .or_else(|| v["reopens_by_itself"].as_bool())
                    .unwrap_or(false),
            },
            None => Self::Hidden,
        }
    }
}

/// What the transport hands back, for either route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feed {
    Status { seq: u64, status: u16, body: String },
    Chunk { seq: u64, text: String },
    End { seq: u64 },
    Failed { seq: u64, why: String },
}

impl Feed {
    /// What the page's own transport brought, when it is a piece of an
    /// answer.
    #[must_use]
    pub fn from_claude(i: crate::claude::Incoming) -> Option<Self> {
        use crate::claude::Incoming as I;
        match i {
            I::Status { seq, status, body } => Some(Self::Status { seq, status, body }),
            I::Chunk { seq, text } => Some(Self::Chunk { seq, text }),
            I::End { seq } => Some(Self::End { seq }),
            I::Unreachable { seq, why } => Some(Self::Failed { seq, why }),
            I::Proxy(_) | I::Free(_) | I::Takes(_) | I::DesignStatus(_) => None,
        }
    }
}

impl From<crate::box_agent::Incoming> for Feed {
    fn from(i: crate::box_agent::Incoming) -> Self {
        use crate::box_agent::Incoming as I;
        match i {
            I::Status { seq, status, body } => Self::Status { seq, status, body },
            I::Chunk { seq, text } => Self::Chunk { seq, text },
            I::End { seq } => Self::End { seq },
            I::Failed { seq, why } => Self::Failed { seq, why },
        }
    }
}

/// A request to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    /// Ties the answer's pieces to this turn.
    pub seq: u64,
    pub url: String,
    pub body: String,
}

impl Req {
    /// For the page's transport.
    #[must_use]
    pub fn for_page(&self) -> crate::claude::Request {
        crate::claude::json_post(self.url.clone(), self.body.clone())
    }

    /// For the box's transport.
    #[must_use]
    pub fn for_box(&self) -> crate::box_agent::Request {
        crate::box_agent::Request {
            seq: self.seq,
            url: self.url.clone(),
            body: self.body.clone(),
        }
    }
}

/// One block of the model's answer, as the stream gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Thinking { thinking: String, signature: String },
    Redacted { data: String },
    Text { text: String },
}

impl Block {
    /// The block as the relay takes it back; `None` for one it would
    /// refuse (empty text, a thinking block without its signature).
    fn json(&self) -> Option<Json> {
        match self {
            Self::Thinking { thinking, signature } if !signature.is_empty() => {
                Some(json!({"type": "thinking", "thinking": thinking, "signature": signature}))
            }
            Self::Redacted { data } if !data.is_empty() => {
                Some(json!({"type": "redacted_thinking", "data": data}))
            }
            Self::Text { text } if !text.is_empty() => Some(json!({"type": "text", "text": text})),
            _ => None,
        }
    }

    const fn size(&self) -> usize {
        match self {
            Self::Thinking { thinking, signature } => thinking.len() + signature.len(),
            Self::Redacted { data } => data.len(),
            Self::Text { text } => text.len(),
        }
    }
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ended {
    /// Finished; `more`: it stopped at the output limit.
    Whole {
        more: bool,
    },
    /// The stream stopped before the end marker.
    Cut,
    Failed(Failure),
}

/// Why a turn failed, in words, and what it says about the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Failure {
    words: String,
    effect: Effect,
}

/// What a refusal says about the design entry itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    None,
    Closed {
        reopens: bool,
    },
    /// This address's allowance for the hour is used; seconds to wait.
    Allowance(Option<u64>),
    /// The box's relay does not serve the design profile.
    Unavailable,
}

impl Failure {
    fn plain(words: impl Into<String>) -> Self {
        Self {
            words: words.into(),
            effect: Effect::None,
        }
    }
}

/// One turn as it streams in (either route).
#[derive(Debug)]
struct Turn {
    place: Place,
    sse: Sse,
    /// The page route's blocks by the stream's index.
    blocks: Vec<(u64, Block)>,
    /// The box route's text.
    text: String,
    /// The thinking summary so far (the box route's, or the page's).
    thinking: String,
    stop: Option<String>,
    /// The page route's end markers.
    stopped: bool,
    usage: bool,
    /// The box's `done`: whether more is to come.
    done: Option<bool>,
    session: Option<String>,
    failure: Option<Failure>,
    /// Any byte arrived.
    got: bool,
}

impl Turn {
    fn new(place: Place) -> Self {
        Self {
            place,
            sse: Sse::default(),
            blocks: Vec::new(),
            text: String::new(),
            thinking: String::new(),
            stop: None,
            stopped: false,
            usage: false,
            done: None,
            session: None,
            failure: None,
            got: false,
        }
    }

    fn size(&self) -> usize {
        self.text.len() + self.thinking.len() + self.blocks.iter().map(|(_, b)| b.size()).sum::<usize>()
    }

    /// The design's words in this turn so far.
    fn words(&self) -> String {
        match self.place {
            Place::Box => self.text.clone(),
            Place::Page => self
                .blocks
                .iter()
                .filter_map(|(_, b)| match b {
                    Block::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
        }
    }

    fn chars(&self) -> usize {
        match self.place {
            Place::Box => self.text.len(),
            Place::Page => self
                .blocks
                .iter()
                .map(|(_, b)| {
                    if let Block::Text { text } = b {
                        text.len()
                    } else {
                        0
                    }
                })
                .sum(),
        }
    }

    fn push(&mut self, chunk: &str) {
        if self.failure.is_some() {
            return;
        }
        self.got = true;
        match self.sse.push(chunk) {
            Ok(events) => {
                for e in &events {
                    match self.place {
                        Place::Page => self.page_event(e),
                        Place::Box => self.box_event(e),
                    }
                }
            }
            Err(e) => self.failure = Some(Failure::plain(format!("The design stream broke: {e}."))),
        }
        if self.size() > MAX_TURN && self.failure.is_none() {
            self.failure = Some(Failure::plain(
                "One turn of the design is larger than this page takes (700 KiB).",
            ));
        }
    }

    fn block(&mut self, i: u64) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|(k, _)| *k == i).map(|(_, b)| b)
    }

    /// One event of the relay's stream: the model's own, then
    /// `relay_usage`, or `relay_error`.
    fn page_event(&mut self, e: &Event) {
        let Ok(d) = serde_json::from_str::<Json>(&e.data) else {
            self.failure = Some(Failure::plain(
                "The design stream sent something that is not JSON.",
            ));
            return;
        };
        let kind = d["type"].as_str().unwrap_or(e.name.as_str());
        let index = d["index"].as_u64();
        match kind {
            "content_block_start" => {
                let b = &d["content_block"];
                let s = |k: &str| b[k].as_str().unwrap_or("").to_string();
                let block = match b["type"].as_str() {
                    Some("thinking") => Some(Block::Thinking {
                        thinking: s("thinking"),
                        signature: s("signature"),
                    }),
                    Some("redacted_thinking") => Some(Block::Redacted { data: s("data") }),
                    Some("text") => Some(Block::Text { text: s("text") }),
                    _ => None,
                };
                if let (Some(i), Some(block)) = (index, block) {
                    if let Block::Thinking { thinking, .. } = &block {
                        self.thinking.push_str(thinking);
                    }
                    self.blocks.push((i, block));
                }
            }
            "content_block_delta" => {
                let delta = &d["delta"];
                let Some(i) = index else { return };
                let piece = |k: &str| delta[k].as_str().unwrap_or("").to_string();
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(Block::Text { text }) = self.block(i) {
                            text.push_str(&piece("text"));
                        }
                    }
                    Some("thinking_delta") => {
                        let t = piece("thinking");
                        if let Some(Block::Thinking { thinking, .. }) = self.block(i) {
                            thinking.push_str(&t);
                            self.thinking.push_str(&t);
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(Block::Thinking { signature, .. }) = self.block(i) {
                            signature.push_str(&piece("signature"));
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = d["delta"]["stop_reason"].as_str() {
                    self.stop = Some(r.to_string());
                }
            }
            "message_stop" => self.stopped = true,
            "relay_usage" => self.usage = true,
            "relay_error" | "error" => {
                let words = d["error"]["message"]
                    .as_str()
                    .or_else(|| d["message"].as_str())
                    .unwrap_or("the stream failed");
                self.failure = Some(Failure::plain(format!(
                    "The design entry stopped the turn: {}.",
                    clip(words, 300).trim_end_matches('.')
                )));
            }
            _ => {}
        }
    }

    /// One event of the box's stream.
    fn box_event(&mut self, e: &Event) {
        let Ok(d) = serde_json::from_str::<Json>(&e.data) else {
            self.failure = Some(Failure::plain("The box sent something that is not JSON."));
            return;
        };
        let delta = || d["delta"].as_str().unwrap_or("").to_string();
        match e.name.as_str() {
            "session" => self.session = d["id"].as_str().map(str::to_string),
            "thinking" => self.thinking.push_str(&delta()),
            "text" => self.text.push_str(&delta()),
            "done" => {
                self.stop = d["stop"].as_str().map(str::to_string);
                self.done = Some(d["more"].as_bool().unwrap_or(false));
            }
            "error" => {
                let kind = d["kind"].as_str().unwrap_or("");
                let message = d["message"].as_str().unwrap_or("the design failed");
                self.failure = Some(if kind == "design_unavailable" {
                    Failure {
                        words: "The design step is not available on this relay.".into(),
                        effect: Effect::Unavailable,
                    }
                } else {
                    Failure::plain(format!(
                        "The box could not finish the design: {}.",
                        clip(message, 300).trim_end_matches('.')
                    ))
                });
            }
            _ => {}
        }
    }

    /// How the stream ended, once its body has.
    fn ended(&self) -> Ended {
        if let Some(f) = &self.failure {
            return Ended::Failed(f.clone());
        }
        let stop = self.stop.as_deref();
        let whole = match self.place {
            Place::Page => self.usage && self.stopped,
            Place::Box => self.done.is_some(),
        };
        if !whole {
            return Ended::Cut;
        }
        match stop {
            Some("max_tokens") => Ended::Whole { more: true },
            Some("refusal") => Ended::Failed(Failure::plain(
                "The model declined to design this. Try describing the machine differently.",
            )),
            Some("model_context_window_exceeded") => Ended::Failed(Failure::plain(
                "The design ran out of room in the model's context. Describe a smaller machine, or one part of it.",
            )),
            _ => Ended::Whole { more: false },
        }
    }
}

/// `s` on one line cut to `n` characters.
fn clip(s: &str, n: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if s.chars().count() <= n {
        s
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

/// A turn of the design that came whole.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Done {
    text: String,
    blocks: Vec<Block>,
}

/// The design under way.
#[derive(Debug)]
struct Run {
    /// What the design is called: the description, or the documents' names.
    description: String,
    /// The text the box is sent: the description and the documents.
    message: String,
    /// The documents the design was asked for with, kept with it.
    docs: Vec<crate::documents::Doc>,
    /// The page route's first user message.
    first: String,
    /// The box's session.
    session: Option<String>,
    done: Vec<Done>,
    cur: Turn,
    seq: u64,
    /// Page time the design was asked for, s.
    started: f64,
    /// The stream stopped before its end: waiting for Carry on or Discard.
    cut: Option<String>,
}

/// What a design under way shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// Since the design was asked for, s.
    pub secs: f64,
    /// The turn under way, from 1.
    pub turn: usize,
    /// The end of the thinking summary so far.
    pub thinking: String,
    /// The design text written so far, bytes.
    pub written: usize,
    /// The stream ended early: why, in words.
    pub cut: Option<String>,
}

/// A design that came whole, for reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    /// The answers put end to end.
    pub text: String,
    pub design: Design,
    /// What stops the design being built from ([`Design::problems`]).
    pub problems: Vec<String>,
    /// How many turns it took.
    pub turns: usize,
    /// What the engineer asked for.
    pub description: String,
    /// The documents it was asked for with, kept in the project beside it.
    pub documents: Vec<crate::documents::Doc>,
    /// The section on show in the window (into [`sections`]).
    pub section: usize,
    /// The section on show, read as Markdown: which section and its blocks.
    pub shown: Option<(usize, Vec<crate::md_view::Block>)>,
}

impl Read {
    #[must_use]
    pub fn new(text: String, turns: usize, description: String) -> Self {
        let design = design::parse(&text);
        Self {
            problems: design.problems(),
            design,
            text,
            turns,
            description,
            documents: Vec::new(),
            section: 0,
            shown: None,
        }
    }

    /// The documents the design was asked for with.
    #[must_use]
    pub fn with_documents(mut self, documents: Vec<crate::documents::Doc>) -> Self {
        self.documents = documents;
        self
    }

    /// The documents the design was asked for with, as the project's
    /// [`crate::documents::DOCUMENTS_FILE`]; `None` when there were none.
    #[must_use]
    pub fn documents_file(&self) -> Option<String> {
        (!self.documents.is_empty()).then(|| crate::documents::documents_md(&self.documents))
    }

    /// Why the design cannot be kept as a project file, if it cannot.
    #[must_use]
    pub fn keep_refusal(&self) -> Option<String> {
        if self
            .documents_file()
            .is_some_and(|d| d.len() > helmsman_project::MAX_FILE)
        {
            return Some(format!(
                "The documents are over {} KiB, which a project file is at most, so the design cannot be kept with them.",
                helmsman_project::MAX_FILE / 1024
            ));
        }
        if self.text.len() > helmsman_project::MAX_FILE {
            return Some(format!(
                "The design is {} KiB and a project file is at most {} KiB, so it cannot be kept.",
                self.text.len().div_ceil(1024),
                helmsman_project::MAX_FILE / 1024
            ));
        }
        helmsman_project::check_text(self.text.as_bytes())
            .err()
            .map(|why| format!("The design cannot be kept: {why}."))
    }
}

/// The parts of a design the window shows, by name and the text under it:
/// the sections the profile promises, as written.
#[must_use]
pub fn sections(d: &Design, text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut fenced = false;
    let mut cur: Option<(String, String)> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let head = (!fenced)
            .then(|| line.strip_prefix("## "))
            .flatten()
            .filter(|h| design::HEADINGS.iter().any(|k| k.eq_ignore_ascii_case(h.trim())));
        if let Some(h) = head {
            out.extend(cur.take());
            cur = Some((h.trim().to_string(), String::new()));
        } else if let Some((_, body)) = &mut cur {
            body.push_str(line);
            body.push('\n');
        }
    }
    out.extend(cur);
    let _ = d;
    out.into_iter().map(|(h, b)| (h, b.trim().to_string())).collect()
}

/// The design entry on one page.
#[derive(Debug)]
pub struct Designer {
    /// Off the page for now (Nick, 2026-11-25: "Design a machine and add a
    /// spec should be removed for now, we really need to get incremental
    /// progress working"): no entry, whatever the relay offers. Tests that
    /// exercise the design flow turn it back on ([`Self::set_base`]).
    pub shelved: bool,
    pub place: Place,
    base: Option<String>,
    pub avail: Avail,
    /// The description window is up.
    pub open: bool,
    /// The description as typed.
    pub description: String,
    /// The documents added or pasted beside it.
    pub docs: crate::documents::Documents,
    /// Under the description: why it cannot go.
    pub error: Option<String>,
    /// In the tab: how the last design ended, when it did not come whole.
    pub notice: Option<String>,
    /// This address's allowance is used: (page time it was said, seconds).
    wait: Option<(f64, u64)>,
    run: Option<Run>,
    /// A design that came whole and is on show.
    pub read: Option<Read>,
    /// The design's window is up.
    pub window: bool,
    seq: u64,
}

impl Designer {
    /// The simulation's entry: at the address the page was built with.
    #[must_use]
    pub fn page() -> Self {
        let mut d = Self::new(Place::Page);
        d.base = DESIGN_URL
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());
        d.avail = if d.base.is_some() {
            Avail::Unknown
        } else {
            Avail::Hidden
        };
        d
    }

    /// A box's entry: always there; the box answers for itself.
    #[must_use]
    pub fn boxed() -> Self {
        let mut d = Self::new(Place::Box);
        d.docs.on_box = true;
        d.avail = Avail::Open;
        d
    }

    fn new(place: Place) -> Self {
        Self {
            shelved: true,
            place,
            base: None,
            avail: Avail::Hidden,
            open: false,
            description: String::new(),
            docs: crate::documents::Documents::default(),
            error: None,
            notice: None,
            wait: None,
            run: None,
            read: None,
            window: false,
            seq: 0,
        }
    }

    /// Makes this the entry at `base` (tests; a build sets
    /// [`DESIGN_URL`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_base(&mut self, base: &str) {
        self.base = Some(base.trim().trim_end_matches('/').to_string());
        self.avail = Avail::Unknown;
        self.shelved = false;
    }

    /// The address to ask for the status, once: the status is then
    /// being asked for until [`Self::status_body`] hears it.
    pub fn ask_status(&mut self) -> Option<String> {
        if self.avail != Avail::Unknown {
            return None;
        }
        let base = self.base.clone()?;
        self.avail = Avail::Asking;
        Some(base)
    }

    /// The status reply came.
    pub fn status_body(&mut self, body: &str) {
        self.avail = Avail::from_status(body);
    }

    /// Whether the tab shows the entry.
    #[must_use]
    pub fn visible(&self) -> bool {
        !self.shelved && matches!(self.avail, Avail::Open | Avail::Closed { .. })
    }

    #[must_use]
    pub const fn busy(&self) -> bool {
        self.run.is_some()
    }

    /// The design entry is closed or its allowance used, in words, said in
    /// the tab beside the entry (the chat goes on without it).
    #[must_use]
    pub fn closed_line(&self, now: f64) -> Option<String> {
        if let Avail::Closed { reopens } = self.avail {
            return Some(if reopens {
                "The design entry is closed for now. It opens again by itself.".into()
            } else {
                "The design entry is closed for now. It stays closed until someone reopens it.".into()
            });
        }
        let (at, secs) = self.wait?;
        let left = (at + secs as f64 - now).max(0.0);
        (left > 0.0).then(|| {
            format!(
                "This hour's design allowance is used. Try again in {}.",
                wait_words(left)
            )
        })
    }

    /// Why the entry cannot ask for a design now, if it cannot.
    #[must_use]
    pub fn blocked(&self, now: f64) -> Option<String> {
        if self.run.is_some() {
            return Some("A design is being written.".into());
        }
        self.closed_line(now)
    }

    /// The turn under way, for the tab.
    #[must_use]
    pub fn progress(&self, now: f64) -> Option<Progress> {
        let r = self.run.as_ref()?;
        let thinking = last_paragraph(&r.cur.thinking);
        Some(Progress {
            secs: (now - r.started).max(0.0),
            turn: r.done.len() + 1,
            thinking,
            written: r.done.iter().map(|d| d.text.len()).sum::<usize>() + r.cur.chars(),
            cut: r.cut.clone(),
        })
    }

    /// Asks for a design in the simulation: `first` is the request's first
    /// message ([`design::request`]).
    ///
    /// # Errors
    /// A design is under way, the description is not one, or the request
    /// does not fit what the relay takes.
    pub fn start_page(&mut self, first: String, description: &str, now: f64) -> Result<Req, String> {
        self.start(description, first, now)
    }

    /// Asks the box for a design.
    ///
    /// # Errors
    /// A design is under way, or the description is not one.
    pub fn start_box(&mut self, description: &str, now: f64) -> Result<Req, String> {
        self.start(description, String::new(), now)
    }

    fn start(&mut self, description: &str, first: String, now: f64) -> Result<Req, String> {
        if let Some(why) = self.blocked(now) {
            return Err(why);
        }
        let description = description.trim();
        let n = description.chars().count();
        if n == 0 && self.docs.is_empty() {
            return Err("Describe the machine, or add a document, first.".into());
        }
        if n > MAX_DESCRIPTION {
            return Err(format!(
                "A description is at most {MAX_DESCRIPTION} characters ({n} now)."
            ));
        }
        if description
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err("A description may not carry control characters.".into());
        }
        if let Some(why) = self.docs.refusal(description) {
            return Err(why);
        }
        let title = if n == 0 {
            self.docs.title()
        } else {
            description.to_string()
        };
        self.notice = None;
        self.error = None;
        self.read = None;
        self.window = false;
        self.run = Some(Run {
            description: title,
            message: self.docs.message(description),
            docs: self.docs.documents(),
            first,
            session: None,
            done: Vec::new(),
            cur: Turn::new(self.place),
            seq: 0,
            started: now,
            cut: None,
        });
        match self.next_request() {
            Ok(req) => Ok(req),
            Err(why) => {
                self.run = None;
                Err(why)
            }
        }
    }

    /// The request for the turn after the ones done.
    fn next_request(&mut self) -> Result<Req, String> {
        let place = self.place;
        let url = match place {
            Place::Page => format!("{}/demo/v1/messages", self.base.as_deref().unwrap_or("")),
            Place::Box => BOX_URL.to_string(),
        };
        let r = self.run.as_mut().ok_or("No design is being written.")?;
        let body = match place {
            Place::Page => page_body(&r.first, &r.done)?,
            Place::Box => match (&r.session, r.done.is_empty()) {
                (Some(id), false) => json!({ "session": id }).to_string(),
                _ => json!({ "description": r.message }).to_string(),
            },
        };
        self.seq += 1;
        r.seq = self.seq;
        r.cur = Turn::new(place);
        r.cut = None;
        Ok(Req {
            seq: self.seq,
            url,
            body,
        })
    }

    /// Takes in what the transport brought; the request that follows, when
    /// the turn ended at the output limit and the design goes on.
    pub fn receive(&mut self, feed: Feed, now: f64) -> Option<Req> {
        let place = self.place;
        let r = self.run.as_mut()?;
        let (Feed::Status { seq, .. }
        | Feed::Chunk { seq, .. }
        | Feed::End { seq }
        | Feed::Failed { seq, .. }) = &feed;
        if *seq != r.seq || r.cut.is_some() {
            return None;
        }
        let ended = match feed {
            Feed::Status { status, body, .. } => Some(Ended::Failed(refusal(place, status, &body))),
            Feed::Chunk { text, .. } => {
                r.cur.push(&text);
                r.cur.failure.clone().map(Ended::Failed)
            }
            Feed::End { .. } => Some(r.cur.ended()),
            Feed::Failed { why, .. } => {
                let why = clip(&why, 200);
                let why = why.trim_end_matches('.');
                if r.cur.got {
                    r.cut = Some(format!(
                        "The connection broke ({why}) before the design was finished."
                    ));
                    None
                } else {
                    Some(Ended::Failed(Failure::plain(format!(
                        "The design entry could not be reached: {why}."
                    ))))
                }
            }
        };
        match ended? {
            Ended::Cut => {
                self.run.as_mut()?.cut = Some("The stream stopped before the design was finished.".into());
                None
            }
            Ended::Failed(f) => {
                self.fail(&f, now);
                None
            }
            Ended::Whole { more } => self.turn_over(more),
        }
    }

    /// A turn came whole.
    fn turn_over(&mut self, more: bool) -> Option<Req> {
        let r = self.run.as_mut()?;
        let cur = std::mem::replace(&mut r.cur, Turn::new(self.place));
        if cur.session.is_some() {
            r.session.clone_from(&cur.session);
        }
        let blocks: Vec<Block> = cur.blocks.iter().map(|(_, b)| b.clone()).collect();
        r.done.push(Done {
            text: cur.words(),
            blocks,
        });
        if !more {
            let r = self.run.take()?;
            let text = design::join(r.done.iter().map(|d| d.text.as_str()));
            self.read = Some(Read::new(text, r.done.len(), r.description).with_documents(r.docs));
            self.window = true;
            return None;
        }
        match self.next_request() {
            Ok(req) => Some(req),
            Err(why) => {
                self.run = None;
                self.notice = Some(why);
                None
            }
        }
    }

    /// The turn failed: the design ends with nothing kept, and what it
    /// says of the entry is remembered.
    fn fail(&mut self, f: &Failure, now: f64) {
        self.run = None;
        match f.effect {
            Effect::None | Effect::Unavailable => {}
            Effect::Closed { reopens } => self.avail = Avail::Closed { reopens },
            Effect::Allowance(secs) => self.wait = Some((now, secs.unwrap_or(3600))),
        }
        self.notice = Some(f.words.clone());
    }

    /// Carry on after a stream that stopped early: the turn is asked for
    /// again.
    pub fn carry_on(&mut self, _now: f64) -> Option<Req> {
        self.run.as_ref()?.cut.as_ref()?;
        match self.next_request() {
            Ok(req) => Some(req),
            Err(why) => {
                self.run = None;
                self.notice = Some(why);
                None
            }
        }
    }

    /// Stop, or Discard of a cut design: nothing is kept.
    pub fn stop(&mut self) {
        self.run = None;
        self.seq += 1;
    }

    /// Drops the design on show.
    pub fn discard(&mut self) {
        self.read = None;
        self.window = false;
    }
}

/// How long to wait, in words: whole minutes, rounded up.
fn wait_words(secs: f64) -> String {
    let minutes = (secs / 60.0).ceil().max(1.0) as u64;
    if minutes == 1 {
        "about a minute".into()
    } else if minutes >= 90 {
        format!("about {} hours", minutes.div_ceil(60))
    } else {
        format!("about {minutes} minutes")
    }
}

/// The last paragraph of a thinking summary, at most 400 characters from
/// its end.
fn last_paragraph(s: &str) -> String {
    let last = s
        .rsplit("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty())
        .unwrap_or("");
    let n = last.chars().count();
    if n <= 400 {
        return last.to_string();
    }
    let tail: String = last.chars().skip(n - 400).collect();
    format!("…{}", tail.trim_start())
}

/// The relay's request for a design: the first message, then each answer
/// as it came and the word [`CONTINUE`] after it.
fn page_body(first: &str, done: &[Done]) -> Result<String, String> {
    let mut messages = vec![json!({"role": "user", "content": first})];
    for d in done {
        let blocks: Vec<Json> = d.blocks.iter().filter_map(Block::json).collect();
        if blocks.is_empty() {
            return Err("The last turn of the design came back empty.".into());
        }
        messages.push(json!({"role": "assistant", "content": blocks}));
        messages.push(json!({"role": "user", "content": CONTINUE}));
    }
    if messages.len() > MAX_MESSAGES {
        return Err(TOO_LARGE.into());
    }
    let body = json!({"agent": AGENT, "messages": messages}).to_string();
    if body.len() > MAX_BODY {
        return Err(TOO_LARGE.into());
    }
    Ok(body)
}

/// A refusal in words, from the status and the body of an answer that is
/// not a stream.
fn refusal(place: Place, status: u16, body: &str) -> Failure {
    let j = serde_json::from_str::<Json>(body).unwrap_or(Json::Null);
    match place {
        Place::Page => {
            let e = &j["error"];
            let kind = e["type"].as_str().unwrap_or("");
            let message = clip(e["message"].as_str().unwrap_or(""), 200);
            let reopens = e["retryable"].as_bool().unwrap_or(false);
            match (status, kind) {
                (503, "design_closed") => Failure {
                    words: if reopens {
                        "The design entry is closed for now. It opens again by itself. The chat is not affected.".into()
                    } else {
                        "The design entry is closed for now. It stays closed until someone reopens it. The chat is not affected.".into()
                    },
                    effect: Effect::Closed { reopens },
                },
                (429, "design_allowance_used") => {
                    let secs = e["retry_after_s"].as_u64();
                    let wait = secs.map_or("a while".to_string(), |s| wait_words(s as f64));
                    Failure {
                        words: format!(
                            "This hour's design allowance is used. Try again in {wait}. The chat is not affected."
                        ),
                        effect: Effect::Allowance(secs),
                    }
                }
                (413, _) => Failure::plain(
                    "The design request is larger than the design entry takes (1 MiB). Describe a smaller machine, or one part of it.",
                ),
                (403, _) => Failure::plain("The design entry did not accept this page (403)."),
                (422, _) => Failure::plain(format!(
                    "The model service refused the design request: {}",
                    if message.is_empty() {
                        "no reason given.".into()
                    } else {
                        message
                    }
                )),
                (503, "upstream_unavailable") => {
                    Failure::plain("The model service is busy or cannot be reached. Try again in a moment.")
                }
                (400, _) => Failure::plain(format!(
                    "The design entry refused the request: {}",
                    if message.is_empty() {
                        "no reason given.".into()
                    } else {
                        message
                    }
                )),
                _ => Failure::plain(format!("The design entry answered HTTP {status}.")),
            }
        }
        Place::Box => {
            let why = clip(j["error"].as_str().unwrap_or(""), 200);
            let words = match status {
                401 => "The box no longer knows this page's session. Sign in again.".to_string(),
                403 => "Only an engineer asks the box for a design.".to_string(),
                404 => "The design expired on the box. Ask for it again.".to_string(),
                0 => format!("The box did not answer. {why}"),
                s => format!(
                    "The box refused the design request ({s}){}",
                    if why.is_empty() {
                        ".".to_string()
                    } else {
                        format!(": {why}")
                    }
                ),
            };
            Failure::plain(words.trim().to_string())
        }
    }
}

// ---- building from a kept design ----

/// A batch of a design's build order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// Steps built before it.
    pub from: usize,
    /// The step after its last.
    pub to: usize,
    /// Steps in the design.
    pub total: usize,
    /// Each step as shown in the tab.
    pub steps: Vec<String>,
    /// What the Agent is asked for each step ([`Design::step_brief`]).
    pub briefs: Vec<String>,
}

impl Batch {
    /// The History's name for the instruction.
    #[must_use]
    pub fn label(&self, d: &Design) -> String {
        let head = format!("Design steps {} to {} of {}", self.from + 1, self.to, self.total);
        let said = clip(&d.summary, 120);
        let mut s = if said.is_empty() {
            head
        } else {
            format!("{head}: {said}")
        };
        if s.chars().count() > 200 {
            s = s.chars().take(199).collect();
            s.push('…');
        }
        s
    }
}

/// The next batch of `d`'s build order after `built` steps, at most
/// [`crate::steps::MAX_STEPS`] steps; `None` when every step is built or
/// the design has none.
#[must_use]
pub fn batch(d: &Design, built: usize) -> Option<Batch> {
    let total = d.steps.len();
    if built >= total {
        return None;
    }
    let to = (built + crate::steps::MAX_STEPS).min(total);
    let mut steps = Vec::new();
    let mut briefs = Vec::new();
    for k in built..to {
        briefs.push(d.step_brief(k)?);
        steps.push(clip(&d.steps[k].text, 160));
    }
    Some(Batch {
        from: built,
        to,
        total,
        steps,
        briefs,
    })
}

/// The project's `design.md` as last read: the design and what stops it
/// being built from, read again only when the text changed.
#[derive(Debug, Default)]
pub struct KeptCache(Option<(u64, Design, Vec<String>)>);

impl KeptCache {
    /// The design `text` holds.
    pub fn get(&mut self, text: &str) -> (&Design, &[String]) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        let key = h.finish();
        if self.0.as_ref().is_none_or(|c| c.0 != key) {
            let d = design::parse(text);
            let problems = d.problems();
            self.0 = Some((key, d, problems));
        }
        let (_, d, p) = self.0.as_ref().expect("just read");
        (d, p)
    }
}

/// What the tab shows of a kept design `d` of which `built` steps are
/// built; `build_blocked` is why Build is off for a reason of the page's.
#[must_use]
pub fn kept_view(
    d: &Design,
    problems: &[String],
    built: usize,
    build_blocked: Option<String>,
) -> crate::design_ui::Kept {
    let total = d.steps.len();
    let built = built.min(total);
    let next =
        (built < total && problems.is_empty()).then(|| (built, (built + crate::steps::MAX_STEPS).min(total)));
    crate::design_ui::Kept {
        parts: d.parts.len(),
        total,
        built,
        problems: problems.to_vec(),
        next,
        build_blocked,
    }
}

/// A project name that is free among `taken`, starting from `want`.
#[must_use]
pub(crate) fn free_name(want: &str, taken: &[String]) -> String {
    let base: String = want.chars().take(44).collect();
    let is_taken = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if !is_taken(&base) {
        return base;
    }
    (2..=400)
        .map(|k| format!("{base}-{k}"))
        .find(|n| !is_taken(n))
        .unwrap_or(base)
}

/// A project name for a design described as `description`: its first
/// words, a-z 0-9 and `-`.
#[must_use]
pub fn project_slug(description: &str) -> String {
    let mut out = String::new();
    for w in description
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 2)
        .take(4)
    {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&w.to_ascii_lowercase());
    }
    out.truncate(30);
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "design".into() } else { out }
}

/// What the page tells the relay about the dialect, for the design
/// request: the playground's, in the words the box gives its own.
pub const PAGE_DIALECT: &str = "The playground runs Structured Text in the page. A project is files; the program is one PROGRAM, plus any FUNCTION, FUNCTION_BLOCK and TYPE declarations it uses, and it runs once per scan (10 ms by default). Each VAR_INPUT of the PROGRAM is a signal the program reads (a BOOL is a button, a number a slider or field) and each VAR_OUTPUT one it drives (a lamp or a readout); an initial value on a VAR_INPUT is its starting position. Located variables (AT %IX0.0, AT %QX0.0, bytes 0 to 7, bits 0 to 7) are I/O as well. Variable sections are VAR, VAR_TEMP, VAR_INPUT, VAR_OUTPUT, VAR CONSTANT, VAR_GLOBAL and VAR_EXTERNAL; VAR_IN_OUT exists on function blocks only; a function's VAR_OUTPUTs are bound with => at every call. Types are BOOL, SINT to LINT, USINT to ULINT, BYTE to LWORD, REAL, LREAL and TIME, STRUCTs, enumerations, aliases and ARRAYs; STRING, DATE, DT, TOD, pointers, references and OOP are not supported. Statements are assignment, IF, CASE, FOR, WHILE, REPEAT, EXIT, RETURN and calls. Standard functions are ABS, SQRT, LN, LOG, EXP, the trigonometric functions, EXPT, MIN, MAX, LIMIT, SEL, MUX, SHL, SHR, ROL, ROR, TRUNC, MOVE and the type conversions. Standard function blocks are TON, TOF, TP, CTU, CTD, CTUD, R_TRIG, F_TRIG, SR and RS, and BLINK_PATTERN for a stack light's patterns. Timers advance between scans, so their outputs hold still within one scan. One project has one PROGRAM: the parts of a design are FUNCTION_BLOCKs it calls. Requirement tests are programs named test_NAME run beside the program on a simulated clock.";

#[cfg(test)]
#[path = "design_step_tests.rs"]
mod tests;
