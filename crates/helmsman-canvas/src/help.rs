//! Help mode on a box's Agent tab, beside changing the program (Nick,
//! 2026-11-07, interview): for someone in trouble who did not write the
//! program. They say what went wrong; the page sends it to the box with
//! its recent Messages (`messages::Log::text`), and the box attaches the
//! rest itself (its build, the project it runs, its devices and their
//! errors), leaving out every password, PIN and token
//! (`helmsman-plc`'s `api::help`). The answer is a plain diagnosis and a
//! Markdown issue with a Copy button ([`crate::messages::copyable`]), to
//! paste to GitHub or to another agent. Nothing is filed.
//!
//! Free of I/O like [`crate::box_agent`]: the request goes out through
//! the same transport as a chat turn (`web::ChatHttp`), and the event
//! stream comes back as [`Incoming`] pieces; tests feed them natively.
//!
//! - `POST /api/help` `{question, messages}`; an event stream back:
//!   `help`, `text`, `tool_call`, `tool_result`, `retrying`, `answer`,
//!   `pages`, `issue`, `done`, `error`. A question about how things work
//!   ("how do I load a program?") gets an `answer` from the help pages
//!   (Nick, 2026-11-08) and no issue; trouble gets an `issue`, from the
//!   agent or (with no agent, or a relay that does not offer Help yet)
//!   from the box alone, and `done` then says why there is no diagnosis;
//!   with no agent the box may add `pages`, the help sections whose
//!   headings match a question.

// Natively nothing carries the answers back but the tests.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use eframe::egui::{self, RichText, Vec2};
use serde_json::{Value as Json, json};

use crate::box_agent::{Incoming, Request};
use crate::claude::{Event, Sse};
use crate::tips::Tip;

pub const HELP_URL: &str = "/api/help";
/// Longest question, in characters (`api::help::MAX_QUESTION`).
pub const MAX_QUESTION: usize = 2000;
/// Most of the Messages text sent (the newest), bytes; the box keeps 16 KiB.
pub const MAX_MESSAGES: usize = 16 * 1024;
/// Most of the agent's words kept while it works.
const MAX_SAID: usize = 4096;
/// Longest issue kept, bytes (the box's is far shorter).
const MAX_ISSUE: usize = 128 * 1024;

/// What the Agent tab does: change the program, or help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Code,
    Help,
}

/// The issue the box sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Written by the agent (else by the box alone, with no diagnosis).
    pub by_agent: bool,
    pub title: String,
    pub diagnosis: String,
    pub markdown: String,
}

/// A help page, or a section of one, on the site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLink {
    pub title: String,
    pub url: String,
}

/// The agent's answer to a question, from the help pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    /// The pages it came from.
    pub pages: Vec<PageLink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Asking,
    /// The stream ended: the answer to a question and/or the issue (at
    /// least one), the pages the box itself suggests, and why there is
    /// no diagnosis when there is none.
    Answered {
        answer: Option<Answer>,
        issue: Option<Issue>,
        pages: Vec<PageLink>,
        why: Option<String>,
    },
    Failed(String),
}

/// One help request, as it streams.
#[derive(Debug)]
pub struct Helping {
    pub seq: u64,
    pub phase: Phase,
    /// What the agent said while it worked.
    pub said: String,
    answer: Option<Answer>,
    issue: Option<Issue>,
    pages: Vec<PageLink>,
    why: Option<String>,
    error: Option<String>,
    sse: Sse,
}

/// Most page links kept from one event.
const MAX_LINKS: usize = 5;

/// `d["pages"]` as links: title and an `https://` url, each at most once.
fn links(d: &Json) -> Vec<PageLink> {
    let mut out = Vec::new();
    for p in d["pages"].as_array().into_iter().flatten() {
        let title: String = p["title"].as_str().unwrap_or("").chars().take(120).collect();
        let url = p["url"].as_str().unwrap_or("");
        if title.trim().is_empty() || !url.starts_with("https://") || url.len() > 400 {
            continue;
        }
        let l = PageLink {
            title,
            url: url.to_string(),
        };
        if !out.contains(&l) {
            out.push(l);
        }
        if out.len() == MAX_LINKS {
            break;
        }
    }
    out
}

/// The tab's Help side: the mode, the question, the request.
#[derive(Debug, Default)]
pub struct HelpUi {
    pub mode: Mode,
    pub draft: String,
    pub helping: Option<Helping>,
}

/// Why `text` cannot be asked, if it cannot.
#[must_use]
pub fn refusal(text: &str) -> Option<String> {
    let n = text.trim().chars().count();
    if n == 0 {
        Some("Say what went wrong first.".into())
    } else if n > MAX_QUESTION {
        Some(format!("At most {MAX_QUESTION} characters ({n} now)."))
    } else {
        None
    }
}

/// The newest `max` bytes of `text`, from a line start.
fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut from = text.len() - max;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let t = &text[from..];
    t.find('\n').map_or(t, |k| &t[k + 1..])
}

/// The request for `question`, with the page's `messages` (the newest
/// [`MAX_MESSAGES`] bytes). Control characters other than line breaks and
/// tabs are dropped: the box refuses them.
#[must_use]
pub fn request(question: &str, messages: &str, seq: u64) -> Request {
    let q: String = question
        .trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    Request {
        seq,
        url: HELP_URL.into(),
        body: json!({ "question": q, "messages": tail(messages, MAX_MESSAGES) }).to_string(),
    }
}

impl Helping {
    #[must_use]
    pub fn new(seq: u64) -> Self {
        Self {
            seq,
            phase: Phase::Asking,
            said: String::new(),
            answer: None,
            issue: None,
            pages: Vec::new(),
            why: None,
            error: None,
            sse: Sse::default(),
        }
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        self.phase == Phase::Asking
    }

    /// A piece of the box's answer; one for another request is ignored.
    pub fn apply(&mut self, inc: Incoming) {
        match inc {
            Incoming::Status { seq, status, body } if seq == self.seq => self.refused(status, &body),
            Incoming::Chunk { seq, text } if seq == self.seq => self.chunk(&text),
            Incoming::End { seq } if seq == self.seq => self.end(None),
            Incoming::Failed { seq, why } if seq == self.seq => self.end(Some(&why)),
            _ => {}
        }
    }

    /// Stopped by the person.
    pub fn stop(&mut self) {
        if self.busy() {
            self.phase = Phase::Failed("Stopped.".into());
        }
    }

    fn refused(&mut self, status: u16, body: &str) {
        if !self.busy() {
            return;
        }
        let said = serde_json::from_str::<Json>(body)
            .ok()
            .and_then(|j| j["error"].as_str().map(str::to_string));
        self.phase = Phase::Failed(match (status, said) {
            (401, _) => "Sign in (top right) to ask for help.".into(),
            (_, Some(s)) => s,
            (s, None) => format!("the box answered {s}"),
        });
    }

    fn chunk(&mut self, text: &str) {
        if !self.busy() {
            return;
        }
        match self.sse.push(text) {
            Ok(events) => {
                for e in &events {
                    self.event(e);
                }
            }
            Err(why) => self.phase = Phase::Failed(why),
        }
    }

    fn end(&mut self, why: Option<&str>) {
        if !self.busy() {
            return;
        }
        if let Ok(events) = self.sse.push("\n\n") {
            for e in &events {
                self.event(e);
            }
        }
        let (answer, issue) = (self.answer.take(), self.issue.take());
        self.phase = match (answer.is_some() || issue.is_some(), self.error.take(), why) {
            (true, error, _) => Phase::Answered {
                answer,
                issue,
                pages: std::mem::take(&mut self.pages),
                why: error.or_else(|| self.why.take()),
            },
            (false, Some(e), _) => Phase::Failed(e),
            (false, None, Some(w)) => Phase::Failed(w.to_string()),
            (false, None, None) => {
                Phase::Failed("the box closed the answer before it wrote the issue".into())
            }
        };
    }

    fn event(&mut self, e: &Event) {
        let Ok(d) = serde_json::from_str::<Json>(&e.data) else {
            return;
        };
        let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
        match e.name.as_str() {
            "text" if self.said.len() < MAX_SAID => self.said.push_str(&s("delta")),
            "answer" => {
                let text: String = s("text").chars().take(MAX_ISSUE).collect();
                if !text.trim().is_empty() {
                    self.answer = Some(Answer {
                        text,
                        pages: links(&d),
                    });
                }
            }
            "pages" => self.pages = links(&d),
            "issue" => {
                let mut markdown = s("markdown");
                if markdown.len() > MAX_ISSUE {
                    let mut k = MAX_ISSUE;
                    while !markdown.is_char_boundary(k) {
                        k -= 1;
                    }
                    markdown.truncate(k);
                }
                self.issue = Some(Issue {
                    by_agent: s("by") == "agent",
                    title: s("title"),
                    diagnosis: s("diagnosis"),
                    markdown,
                });
            }
            "done" => self.why = d["why"].as_str().map(|w| w.chars().take(600).collect()),
            "error" => self.error = Some(s("message").chars().take(600).collect()),
            _ => {}
        }
    }
}

/// What the owner tells the Help side.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// Why Ask is off, if it is (signed out).
    pub blocked: Option<String>,
}

/// What the person asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Ask,
    Stop,
}

impl Mode {
    /// The heading of the Agent tab in this mode.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Code => "Change the program",
            Self::Help => "Get help",
        }
    }
}

/// The Agent tab's heading (Nick, 2026-11-08): a dot for the agent's
/// state (`dot`, its words on hover), the mode's name, and at the right
/// a toggle of two glyphs, < > for changing the program and ? for help,
/// the one in use tinted.
pub fn heading(
    ui: &mut egui::Ui,
    chrome: &mut crate::editor::Chrome,
    mode: &mut Mode,
    dot: egui::Color32,
    dot_tip: &str,
) {
    use crate::glyphs::{self, Icon};
    let t = crate::theme::theme(ui);
    ui.horizontal(|ui| {
        let (rect, r) = ui.allocate_exact_size(Vec2::splat(glyphs::ROW * 0.6), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), rect.width() * 0.3, dot);
        let r = r.tip(dot_tip.to_string());
        chrome.hit("agent status", r.rect);
        // The title is capped to the room left of the toggle: a resizable
        // panel grows to any row that overflows it.
        let room = ui.available_width() - 2.0 * glyphs::ROW - 2.0 - 2.0 * ui.spacing().item_spacing.x;
        ui.scope(|ui| {
            ui.set_max_width(room.max(40.0));
            ui.add(egui::Label::new(RichText::new(mode.title()).strong().color(t.fg)).truncate())
                .tip("What the Agent does here. The toggle at the right switches it.");
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (m, icon, key, tip) in [
                (
                    Mode::Help,
                    Icon::Help,
                    "agent mode help",
                    "Get help: say what went wrong, or ask how to do something. The Agent looks at this box's messages and devices, tells you what is likely wrong and writes an issue you can copy to report it, or answers your question from the help pages.",
                ),
                (
                    Mode::Code,
                    Icon::Code,
                    "agent mode code",
                    "Change the program: tell the Agent what it should do. It changes the open file and checks the change before it is written.",
                ),
            ] {
                let r = glyphs::button_sized(ui, icon, glyphs::ROW, true, *mode == m, tip);
                chrome.hit(key, r.rect);
                if r.clicked() {
                    *mode = m;
                }
            }
        });
    });
}

/// The question, Ask or Stop, and the answer: the diagnosis and the issue
/// with its Copy button.
pub fn ui(ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome, s: &mut HelpUi, v: &View) -> Option<Act> {
    use crate::st_view::brand::{RED, WARN};

    let t = crate::theme::theme(ui);
    let mut act = None;
    let (field, enter) = crate::instruct::chat_box(
        ui,
        &mut s.draft,
        "agent-help",
        "What went wrong, or what do you want to do? e.g. The conveyor stopped, the lamp is red / How do I load a program?"
            .to_string(),
        "Say what you saw, or ask how to do something, in your own words. The box adds its recent \
         messages, its devices and their errors, the program it runs and its version. Passwords, PINs \
         and tokens are left out. Enter asks; Shift+Enter or Ctrl+Enter starts a new line."
            .to_string(),
    );
    chrome.hit("help question", field.rect);
    let busy = s.helping.as_ref().is_some_and(Helping::busy);
    let why_not = v.blocked.clone().or_else(|| refusal(&s.draft));
    ui.horizontal_wrapped(|ui| {
        if busy {
            let r = crate::glyphs::action(ui, crate::glyphs::Icon::Stop, true, false)
                .tip("Stop: stops waiting for the answer.");
            chrome.hit("help stop", r.rect);
            if r.clicked() {
                act = Some(Act::Stop);
            }
        } else {
            let r = crate::glyphs::action(ui, crate::glyphs::Icon::Send, why_not.is_none(), true).tip(
                why_not.clone().unwrap_or_else(|| {
                    "Ask for help: sends what you wrote and the box's details to the Agent. Nothing is changed and nothing is filed."
                        .to_string()
                }),
            );
            chrome.hit("help ask", r.rect);
            if r.clicked() || (enter && why_not.is_none()) {
                act = Some(Act::Ask);
            }
        }
    });
    ui.add_space(6.0);
    let Some(h) = &s.helping else { return act };
    match &h.phase {
        Phase::Asking => {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                let r = ui
                    .label(RichText::new("The Agent is looking at the box…").color(t.muted))
                    .tip("It reads the box's messages and devices. Nothing on the box changes.");
                chrome.hit("help working", r.rect);
            });
        }
        Phase::Failed(why) => {
            crate::messages::copyable(ui, chrome, "help failed", why, crate::theme::theme(ui).ink(RED));
        }
        Phase::Answered {
            answer,
            issue,
            pages,
            why,
        } => {
            if let Some(a) = answer {
                let r = ui
                    .label(RichText::new("How to do it").strong().color(t.fg))
                    .tip("The Agent's answer, from the help pages. Nothing on the box changed.");
                chrome.hit("help answer title", r.rect);
                crate::messages::copyable(ui, chrome, "help answer", &a.text, t.fg);
                page_links(ui, chrome, "Read more", &a.pages);
            }
            if let Some(issue) = issue {
                if issue.diagnosis.is_empty() {
                    if let Some(w) = why {
                        let r = ui
                            .add(
                                egui::Label::new(RichText::new(w).color(crate::theme::theme(ui).ink(WARN)))
                                    .wrap(),
                            )
                            .tip("Why there is no diagnosis. The issue still carries the box's details.");
                        chrome.hit("help why", r.rect);
                    }
                } else {
                    let r = ui
                        .label(RichText::new("What is likely wrong").strong().color(t.fg))
                        .tip("The Agent's diagnosis, from the box's messages and devices.");
                    chrome.hit("help diagnosis title", r.rect);
                    let r = ui
                        .add(egui::Label::new(RichText::new(&issue.diagnosis).color(t.fg)).wrap())
                        .tip("The Agent's diagnosis, from the box's messages and devices.");
                    chrome.hit("help diagnosis", r.rect);
                }
                page_links(ui, chrome, "These help pages may answer it", pages);
                ui.add_space(6.0);
                // The Copy button sits by the heading, not after the long text
                // (as `messages::copyable` puts it), so it is in view.
                ui.horizontal_wrapped(|ui| {
                    let r = ui
                        .label(RichText::new("Issue to report it").strong().color(t.fg))
                        .tip("Copy it and paste it into a new GitHub issue, or give it to another agent. Nothing was filed.");
                    chrome.hit("help issue title", r.rect);
                    let b = ui
                        .button("Copy")
                        .tip("Copies the whole issue as Markdown, to paste into a new GitHub issue or to another agent.");
                    chrome.hit("help issue copy", b.rect);
                    if b.clicked() {
                        ui.ctx().copy_text(issue.markdown.clone());
                    }
                });
                let r = ui.add(
                    egui::Label::new(RichText::new(&issue.markdown).monospace().color(t.fg))
                        .wrap()
                        .selectable(true),
                );
                chrome.hit("help issue", r.rect);
            }
        }
    }
    act
}

/// Links to help pages under a heading; nothing when there are none.
fn page_links(ui: &mut egui::Ui, chrome: &mut crate::editor::Chrome, heading: &str, pages: &[PageLink]) {
    if pages.is_empty() {
        return;
    }
    let t = crate::theme::theme(ui);
    ui.add_space(4.0);
    let r = ui
        .label(RichText::new(heading).strong().color(t.fg))
        .tip("Help pages on the site, in a new tab.");
    chrome.hit("help pages title", r.rect);
    for p in pages {
        let r = ui
            .hyperlink_to(RichText::new(&p.title), &p.url)
            .tip(format!("Opens {} in a new tab.", p.url));
        chrome.hit("help page link", r.rect);
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    /// `n` lines as the Messages tab writes them.
    fn numbered(n: usize) -> String {
        (0..n).fold(String::new(), |mut s, k| {
            let _ = writeln!(s, "{k:>8}.0s info    line {k}");
            s
        })
    }

    fn stream(issue_by: &str, diagnosis: &str, why: Option<&str>) -> String {
        let issue = json!({
            "by": issue_by, "title": "Conveyor stops", "diagnosis": diagnosis,
            "markdown": "# Conveyor stops\n\n### What happened\n\nIt stopped.\n",
        });
        let done = json!({ "stop": "end_turn", "why": why, "steps": 2, "output_tokens": 9 });
        format!(
            "event: help\ndata: {{\"context\":{{}}}}\n\nevent: text\ndata: {{\"delta\":\"Looking.\"}}\n\nevent: issue\ndata: {issue}\n\nevent: done\ndata: {done}\n\n"
        )
    }

    #[test]
    fn the_request_carries_the_question_and_the_newest_messages() {
        let messages = numbered(5000);
        let r = request("  The lamp is red.\u{1b}[31m\nSince 9.  ", &messages, 7);
        assert_eq!((r.seq, r.url.as_str()), (7, HELP_URL));
        let b: Json = serde_json::from_str(&r.body).unwrap();
        assert_eq!(b["question"], "The lamp is red.[31m\nSince 9.");
        let m = b["messages"].as_str().unwrap();
        assert!(
            m.len() <= MAX_MESSAGES && m.ends_with("line 4999\n"),
            "{}",
            m.len()
        );
        assert!(
            m.starts_with(' ') || m.starts_with(char::is_numeric),
            "from a line start: {m:.40}"
        );
        assert_eq!(b.as_object().unwrap().len(), 2, "{b}");
    }

    #[test]
    fn a_question_is_one_to_two_thousand_characters() {
        assert!(refusal("").is_some() && refusal("  \n ").is_some());
        assert!(refusal("It stopped.").is_none());
        assert!(refusal(&"x".repeat(MAX_QUESTION + 1)).is_some());
    }

    #[test]
    fn the_agent_answer_becomes_a_diagnosis_and_an_issue() {
        let mut h = Helping::new(3);
        let text = stream("agent", "The rio link is down.", None);
        let (a, b) = text.split_at(37);
        h.apply(Incoming::Chunk {
            seq: 3,
            text: a.into(),
        });
        h.apply(Incoming::Chunk {
            seq: 9,
            text: "event: issue\ndata: {}\n\n".into(),
        });
        h.apply(Incoming::Chunk {
            seq: 3,
            text: b.into(),
        });
        assert!(h.busy(), "until the stream ends");
        h.apply(Incoming::End { seq: 3 });
        assert_eq!(h.said, "Looking.");
        let Phase::Answered {
            answer,
            issue: Some(issue),
            pages,
            why,
        } = &h.phase
        else {
            panic!("{:?}", h.phase)
        };
        assert!(issue.by_agent);
        assert_eq!(issue.diagnosis, "The rio link is down.");
        assert!(issue.markdown.starts_with("# Conveyor stops\n"));
        assert_eq!(*why, None);
        assert!(answer.is_none() && pages.is_empty());
    }

    /// A question about how things work (Nick, 2026-11-08): the agent's
    /// answer from the help pages, with links to them, and no issue.
    #[test]
    fn a_question_gets_an_answer_from_the_help_pages_and_no_issue() {
        let answer = json!({
            "text": "Sign in as an engineer, open the project in Files and press Deploy (Ctrl Shift Enter).",
            "pages": [
                { "page": "box", "title": "On a box", "url": "https://helmsman.advin.io/help/box.html" },
                { "page": "box", "title": "On a box", "url": "https://helmsman.advin.io/help/box.html" },
                { "page": "x", "title": "", "url": "https://helmsman.advin.io/help/x.html" },
                { "page": "evil", "title": "Click", "url": "javascript:alert(1)" },
            ],
        });
        let done = json!({ "stop": "end_turn", "why": null, "steps": 2, "output_tokens": 9 });
        let mut h = Helping::new(1);
        h.apply(Incoming::Chunk {
            seq: 1,
            text: format!(
                "event: help\ndata: {{\"context\":{{}}}}\n\nevent: answer\ndata: {answer}\n\nevent: done\ndata: {done}\n\n"
            ),
        });
        h.apply(Incoming::End { seq: 1 });
        let Phase::Answered {
            answer: Some(a),
            issue,
            pages,
            why,
        } = &h.phase
        else {
            panic!("{:?}", h.phase)
        };
        assert!(a.text.starts_with("Sign in as an engineer"));
        // Each page once; no empty title; https only.
        assert_eq!(
            a.pages,
            vec![PageLink {
                title: "On a box".into(),
                url: "https://helmsman.advin.io/help/box.html".into()
            }]
        );
        assert!(issue.is_none() && pages.is_empty() && why.is_none());
    }

    /// No agent, a question: the box's issue, why, and the pages the box
    /// itself suggests.
    #[test]
    fn without_the_agent_a_question_gets_the_pages_the_box_suggests() {
        let pages = json!({ "pages": [
            { "page": "box", "title": "On a box: Deploying", "url": "https://helmsman.advin.io/help/box.html#deploying" },
        ]});
        let issue = json!({
            "by": "box", "title": "How do I deploy?", "diagnosis": "",
            "markdown": "# How do I deploy?\n",
        });
        let done = json!({ "stop": "offline", "why": "no relay", "steps": 0, "output_tokens": 0 });
        let mut h = Helping::new(1);
        h.apply(Incoming::Chunk {
            seq: 1,
            text: format!(
                "event: help\ndata: {{\"context\":{{}}}}\n\nevent: pages\ndata: {pages}\n\nevent: issue\ndata: {issue}\n\nevent: done\ndata: {done}\n\n"
            ),
        });
        h.apply(Incoming::End { seq: 1 });
        let Phase::Answered {
            answer: None,
            issue: Some(issue),
            pages,
            why,
        } = &h.phase
        else {
            panic!("{:?}", h.phase)
        };
        assert!(!issue.by_agent);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].url, "https://helmsman.advin.io/help/box.html#deploying");
        assert_eq!(why.as_deref(), Some("no relay"));
    }

    /// The relay does not offer Help yet: the box's issue and why.
    #[test]
    fn without_the_agent_the_box_issue_and_why_are_kept() {
        let mut h = Helping::new(1);
        h.apply(Incoming::Chunk {
            seq: 1,
            text: stream(
                "box",
                "",
                Some("Help is not available from the agent service yet"),
            ),
        });
        h.apply(Incoming::End { seq: 1 });
        let Phase::Answered {
            issue: Some(issue),
            why,
            ..
        } = &h.phase
        else {
            panic!("{:?}", h.phase)
        };
        assert!(!issue.by_agent && issue.diagnosis.is_empty());
        assert_eq!(
            why.as_deref(),
            Some("Help is not available from the agent service yet")
        );
    }

    #[test]
    fn a_refusal_or_a_broken_stream_says_why() {
        let mut h = Helping::new(1);
        h.apply(Incoming::Status {
            seq: 1,
            status: 401,
            body: "{}".into(),
        });
        assert_eq!(
            h.phase,
            Phase::Failed("Sign in (top right) to ask for help.".into())
        );
        let mut h = Helping::new(2);
        h.apply(Incoming::Status {
            seq: 2,
            status: 422,
            body: json!({ "error": "say what went wrong in 1 to 2000 characters" }).to_string(),
        });
        assert_eq!(
            h.phase,
            Phase::Failed("say what went wrong in 1 to 2000 characters".into())
        );
        let mut h = Helping::new(3);
        h.apply(Incoming::Chunk {
            seq: 3,
            text: "event: help\ndata: {}\n\n".into(),
        });
        h.apply(Incoming::Failed {
            seq: 3,
            why: "network error".into(),
        });
        assert_eq!(h.phase, Phase::Failed("network error".into()));
        let mut h = Helping::new(4);
        h.stop();
        h.apply(Incoming::End { seq: 4 });
        assert_eq!(h.phase, Phase::Failed("Stopped.".into()));
    }
}
