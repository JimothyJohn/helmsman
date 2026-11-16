//! `POST /api/design`: the design step. An engineer describes a machine;
//! the relay's design profile answers with a control system design in
//! Markdown (no program, no tools). The box builds the first user message
//! itself: the description, then four tagged blocks of box data
//! (`<dialect>`, `<project>`, `<io_devices>`, `<requirement_tests>`).
//!
//! The request is `{"description": "..."}` to start, or `{"session": "d1"}`
//! to continue a design whose last turn stopped at `max_tokens`. One
//! request is one relay turn; the box never loops by itself, the page
//! posts the session again while `done.more` is true. A continue sends the
//! conversation again with the assistant turn exactly as it came (thinking
//! blocks and their signatures included) and a user message that is the
//! single word `Continue`.
//!
//! ```text
//! event: session   {"id": "d1"}
//! event: thinking  {"delta": "..."}     summarized thinking, as it streams
//! event: text      {"delta": "..."}
//! event: done      {"stop", "output_tokens", "turns", "more"}   more: stop is max_tokens
//! event: error     {"kind", "message", "retryable"}
//! ```
//!
//! A relay without the profile answers 404 `unknown_agent_version`; that
//! is the error kind `design_unavailable`. The route does not read a
//! failed relay as the agent being offline (chat's state is left alone):
//! the design step has no rule-based fallback.
//!
//! Designs live in memory, at most [`MAX_SESSIONS`], each used by one
//! request at a time and dropped after the chat's idle time. A continue
//! for an unknown or expired session is 404; for one that is answering, or
//! whose last turn did not stop at `max_tokens`, 409.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use helmsman_agent::relay;
use helmsman_agent::turn::Delta;
use helmsman_iec::spec;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::Ctx;
use super::chat::{Chat, MAX_MESSAGE, event};
use super::http::{self, Conn, Response};

/// The design profile the box asks for unless told otherwise.
pub const PROFILE: &str = "helmsman/design-v1";
/// A design turn thinks for minutes: no byte for this long is a dead relay.
pub const IDLE: Duration = Duration::from_secs(180);
/// The longest one design turn may take (the relay's function allows 600 s).
pub const TOTAL: Duration = Duration::from_secs(660);
/// Designs held at once.
pub const MAX_SESSIONS: usize = 4;
/// Largest `<project>` block, bytes.
pub const MAX_PROJECT: usize = 200 * 1024;
/// Largest `<io_devices>` and `<requirement_tests>` block, bytes each.
pub const MAX_DATA: usize = 100 * 1024;
/// The user message that continues a turn cut at `max_tokens`.
pub const CONTINUE: &str = "Continue";

/// What the box's Structured Text accepts, for the designer. Written from
/// `helmsman-iec`'s crate docs, its parser and checker, `library.st`, and
/// docs/ARCHITECTURE.md (multi-module, signals, module directories).
pub const DIALECT: &str = "The box runs Structured Text. A module is one file with one PROGRAM. It runs once per scan at its own period (the box default is 10 ms) and must finish within an instruction budget; a fault such as division by zero, an index out of range or an endless loop zeroes that module's outputs and stops it. A file may also hold FUNCTIONs, FUNCTION_BLOCKs, TYPE declarations (STRUCT, enumerations, aliases) and ARRAYs. Variable sections are VAR, VAR_TEMP, VAR_INPUT, VAR_OUTPUT, VAR CONSTANT, VAR_GLOBAL and VAR_EXTERNAL. VAR_GLOBAL is seen by every POU of the same module and by nothing outside it. VAR_IN_OUT exists on function blocks only. A function's VAR_OUTPUTs are bound with => at every call. Located variables (AT %IX0.1) exist. Types are BOOL, SINT to LINT, USINT to ULINT, BYTE to LWORD, REAL, LREAL and TIME; STRING, DATE, DT, TOD, pointers and references are not supported. Statements are assignment, IF, CASE, FOR, WHILE, REPEAT, EXIT, CONTINUE, RETURN and calls. Standard functions are ABS, SQRT, LN, LOG, EXP, the trigonometric functions, EXPT, MIN, MAX, LIMIT, SEL, MUX, SHL, SHR, ROL, ROR, TRUNC, MOVE and the type conversions. Standard function blocks are TON, TOF, TP, CTU, CTD, CTUD, R_TRIG, F_TRIG, SR and RS, and BLINK_PATTERN for a stack light's patterns. Timers advance between scans, so their outputs hold still within one scan. Modules share nothing but signals. A module's VAR_INPUTs are the signals it reads and its VAR_OUTPUTs the signals it drives, matched by name to the I/O map's signals (points on devices) or to another module's VAR_OUTPUT. A signal has one writer. A module cannot read its own output, and modules cannot read each other in a loop. A VAR_INPUT may instead be a param, a typed value with a range that an engineer sets. A module that reads a signal with no value is held, not scanned, and its outputs go to their safe values until the value is back.";

/// One design in memory.
#[derive(Debug)]
struct Session {
    /// The conversation, assistant turns exactly as the relay sent them.
    /// Empty while a request has it.
    messages: Vec<Json>,
    /// How the last finished turn stopped; `None` before the first.
    stop: Option<String>,
    busy: bool,
    used: Instant,
}

/// The design conversations.
#[derive(Debug, Default)]
pub struct Designs {
    map: Mutex<BTreeMap<String, Session>>,
    next: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A refusal: status and words.
type Refused = (u16, &'static str);

impl Designs {
    /// Opens a new, busy design; the oldest idle one goes when full.
    fn open(&self, idle: Duration) -> Result<String, Refused> {
        let mut map = lock(&self.map);
        let now = Instant::now();
        map.retain(|_, s| s.busy || now.duration_since(s.used) < idle);
        if map.len() >= MAX_SESSIONS {
            let oldest = map
                .iter()
                .filter(|(_, s)| !s.busy)
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    map.remove(&k);
                }
                None => return Err((503, "every design is answering; try again")),
            }
        }
        let id = format!("d{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        map.insert(
            id.clone(),
            Session {
                messages: Vec::new(),
                stop: None,
                busy: true,
                used: now,
            },
        );
        Ok(id)
    }

    /// Takes a design whose last turn stopped at `max_tokens` out of the
    /// store, for one continue.
    fn resume(&self, id: &str, idle: Duration) -> Result<Vec<Json>, Refused> {
        let mut map = lock(&self.map);
        let now = Instant::now();
        map.retain(|_, s| s.busy || now.duration_since(s.used) < idle);
        let Some(s) = map.get_mut(id) else {
            return Err((
                404,
                "no such design session (it may have expired); start a new one",
            ));
        };
        if s.busy {
            return Err((409, "this design is still answering; wait for it"));
        }
        if s.stop.as_deref() != Some("max_tokens") {
            return Err((
                409,
                "this design is finished (its last turn did not stop at max_tokens); start a new one",
            ));
        }
        s.busy = true;
        s.used = now;
        Ok(std::mem::take(&mut s.messages))
    }

    /// Puts a design back after a turn. `stop` is how the turn ended, or
    /// `None` to keep how the earlier one did (the turn failed).
    fn give_back(&self, id: &str, messages: Vec<Json>, stop: Option<String>) {
        if let Some(s) = lock(&self.map).get_mut(id) {
            s.messages = messages;
            if stop.is_some() {
                s.stop = stop;
            }
            s.busy = false;
            s.used = Instant::now();
        }
    }

    fn remove(&self, id: &str) {
        lock(&self.map).remove(id);
    }

    /// Designs held (for tests of the bound).
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.map).len()
    }

    /// Whether none is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    session: Option<String>,
}

/// A checked request, its design taken out of the store.
#[derive(Debug)]
pub enum Job {
    /// A new design: the id opened, the description (trimmed).
    Start { id: String, description: String },
    /// One more turn of a design: the conversation so far.
    Continue { id: String, messages: Vec<Json> },
}

impl Job {
    /// The design's id.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Start { id, .. } | Self::Continue { id, .. } => id,
        }
    }
}

/// Checks a request and opens or takes its design.
///
/// # Errors
/// The response to send instead, and a word for the audit log.
pub fn take(chat: &Chat, body: &[u8]) -> Result<Job, (Response, String)> {
    let refuse =
        |status: u16, why: &str, detail: &str| Err((Response::error(status, why), detail.to_string()));
    let ask: Ask = match serde_json::from_slice(body) {
        Ok(a) => a,
        Err(e) => {
            return Err((
                Response::json(400, &json!({ "error": format!("malformed design request: {e}") })),
                "malformed design request".to_string(),
            ));
        }
    };
    match (ask.description, ask.session) {
        (Some(_), Some(_)) | (None, None) => refuse(
            422,
            "send {description} to start a design, or {session} to continue one",
            "design: description or session",
        ),
        (Some(d), None) => {
            let description = d.trim().to_string();
            let n = description.chars().count();
            if n == 0 || n > MAX_MESSAGE {
                return refuse(
                    422,
                    &format!("a description is 1 to {MAX_MESSAGE} characters"),
                    "design: description length",
                );
            }
            if description
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            {
                return refuse(
                    422,
                    "a description may not carry control characters",
                    "design: control characters",
                );
            }
            match chat.designs().open(chat.idle()) {
                Ok(id) => Ok(Job::Start { id, description }),
                Err((s, why)) => refuse(s, why, "design: sessions full"),
            }
        }
        (None, Some(id)) => match chat.designs().resume(&id, chat.idle()) {
            Ok(messages) => Ok(Job::Continue { id, messages }),
            Err((s, why)) => refuse(s, why, "design: cannot continue"),
        },
    }
}

/// Returns a design the request could not use.
pub fn release(chat: &Chat, job: Job) {
    match job {
        Job::Start { id, .. } => chat.designs().remove(&id),
        Job::Continue { id, messages } => chat.designs().give_back(&id, messages, None),
    }
}

/// Writes `<` as `&lt;` where it would start or end one of the message's
/// blocks (`dialect`, `project`, `io_devices`, `requirement_tests`, any
/// case, whitespace allowed), so data can neither close its own block nor
/// open another. Every other `<` (ST's `<`, `<=`, `<>`) is untouched.
#[must_use]
pub fn neutralise(text: &str) -> String {
    const TAGS: [&str; 4] = ["dialect", "project", "io_devices", "requirement_tests"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let after = rest[at + 1..].trim_start_matches(['/', ' ', '\t', '\r', '\n']);
        let head: String = after.chars().take(17).collect::<String>().to_ascii_lowercase();
        if TAGS.iter().any(|t| head.starts_with(t)) {
            out.push_str("&lt;");
        } else {
            out.push('<');
        }
        rest = &rest[at + 1..];
    }
    out.push_str(rest);
    out
}

/// `data` as compact JSON with `<`, `>` and `&` written as JSON `\u`
/// escapes (the same text as [`helmsman_agent::tools::envelope`] makes), so
/// it cannot hold a tag.
fn json_text(data: &Json) -> String {
    let raw = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
    let mut safe = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '<' => safe.push_str("\\u003c"),
            '>' => safe.push_str("\\u003e"),
            '&' => safe.push_str("\\u0026"),
            c => safe.push(c),
        }
    }
    safe
}

/// Whole `(name, text)` entries in order up to `max` bytes, each under
/// `== name ==`; the ones that did not fit are named at the end, never
/// dropped silently.
fn entries(items: &[(String, String)], max: usize, what: &str) -> String {
    let mut out = String::new();
    let mut left = Vec::new();
    for (name, text) in items {
        let part = format!("== {} ==\n{}\n", neutralise(name), neutralise(text));
        if out.len() + part.len() <= max {
            out.push_str(&part);
        } else {
            left.push(neutralise(name));
        }
    }
    if !left.is_empty() {
        let note = format!(
            "[left out: {what} past {max} bytes; not sent: {}]\n",
            left.join(", ")
        );
        out.push_str(&note);
    }
    out
}

/// `text` cut to at most `max` bytes on a character boundary, saying so.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[cut: {} more bytes left out]",
        &text[..end],
        text.len() - end
    )
}

/// The first user message: the description, then the four blocks.
#[must_use]
pub fn first_message(description: &str, project: &str, io_devices: &str, tests: &str) -> String {
    format!(
        "{}\n\n<dialect>\n{DIALECT}\n</dialect>\n<project>\n{project}</project>\n<io_devices>\n{io_devices}\n</io_devices>\n<requirement_tests>\n{tests}</requirement_tests>",
        neutralise(description)
    )
}

/// The running modules' sources, under their module names.
fn project_block(ctx: &Ctx) -> String {
    let items: Vec<(String, String)> = ctx
        .catalog()
        .modules
        .iter()
        .map(|m| (format!("module {}", m.name), m.source.clone()))
        .collect();
    entries(&items, MAX_PROJECT, "the project block")
}

/// The requirement tests of the running project, if the box has one.
fn tests_block(ctx: &Ctx) -> String {
    let Some(p) = &ctx.projects else {
        return String::new();
    };
    let Some(r) = p.running() else {
        return String::new();
    };
    let Ok(files) = p.read(&r.name) else {
        return String::new();
    };
    let items: Vec<(String, String)> = files
        .st()
        .filter(|(n, _)| spec::is_test(n))
        .map(|(n, t)| (format!("file {n}"), t.to_string()))
        .collect();
    entries(&items, MAX_DATA, "the requirement tests")
}

/// What the agent's `list_devices` returns, as text.
fn devices_block(chat: &Chat) -> String {
    let o = chat.list_devices();
    if o.ok {
        clip(&json_text(&o.data), MAX_DATA)
    } else {
        format!("[unavailable: {}]", neutralise(&o.summary))
    }
}

/// Runs one design turn on `stream`.
pub(super) fn serve(ctx: &Ctx, chat: &Chat, stream: &mut Conn, job: Job) {
    let Some(_slot) = chat.slot() else {
        release(chat, job);
        let _ = http::respond(
            stream,
            &Response::error(503, "the agent is busy with other chats; try again"),
        );
        return;
    };
    let mut alive = http::start_stream(stream).is_ok();
    let mut send = |name: &str, data: &Json| {
        if alive {
            alive = event(stream, name, data).is_ok();
        }
    };
    let id = job.id().to_string();
    send("session", &json!({ "id": id }));
    let mut messages = match job {
        Job::Start { description, .. } => {
            let text = first_message(
                &description,
                &project_block(ctx),
                &devices_block(chat),
                &tests_block(ctx),
            );
            vec![json!({ "role": "user", "content": [{ "type": "text", "text": text }] })]
        }
        Job::Continue { mut messages, .. } => {
            messages.push(json!({ "role": "user", "content": [{ "type": "text", "text": CONTINUE }] }));
            messages
        }
    };
    let continuing = messages.len() > 1;
    if serde_json::to_vec(&messages).map_or(usize::MAX, |v| v.len()) > chat.history_bytes() {
        send(
            "error",
            &json!({ "kind": "design_too_long", "message": "this design is too long to send to the relay; start a new one with a shorter description", "retryable": false }),
        );
        fail(chat, &id, messages, continuing);
        return;
    }
    let cfg = chat.design_relay();
    let result = relay::turn(&cfg, &messages, &mut |d| match d {
        Delta::Text(t) => send("text", &json!({ "delta": t })),
        Delta::Thinking(t) => send("thinking", &json!({ "delta": t })),
        Delta::ToolStart { .. } => {}
    });
    match result {
        Ok(turn) => {
            let stop = turn.stop_reason.clone().unwrap_or_else(|| "end_turn".into());
            messages.push(json!({ "role": "assistant", "content": turn.content }));
            let turns = messages.iter().filter(|m| m["role"] == "assistant").count();
            chat.designs().give_back(&id, messages, Some(stop.clone()));
            send(
                "done",
                &json!({
                    "stop": stop, "output_tokens": turn.usage.output_tokens,
                    "turns": turns, "more": stop == "max_tokens",
                }),
            );
        }
        Err(e) => {
            fail(chat, &id, messages, continuing);
            let (kind, message) = match &e {
                relay::Error::Status {
                    status: 404, kind, ..
                } if kind == "unknown_agent_version" => (
                    "design_unavailable".to_string(),
                    "the design step is not available on this relay".to_string(),
                ),
                other => (other.kind().to_string(), other.to_string()),
            };
            send(
                "error",
                &json!({ "kind": kind, "message": message, "retryable": e.retryable() }),
            );
        }
    }
}

/// A turn that did not complete: a first turn drops its design, a continue
/// gives the conversation back without the `Continue` it added.
fn fail(chat: &Chat, id: &str, mut messages: Vec<Json>, continuing: bool) {
    if continuing {
        messages.pop();
        chat.designs().give_back(id, messages, None);
    } else {
        chat.designs().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::{DIALECT, neutralise};

    #[test]
    fn the_dialect_text_fits_its_budget() {
        assert!(DIALECT.len() <= 2500, "{}", DIALECT.len());
    }

    #[test]
    fn a_block_tag_in_data_cannot_close_or_open_a_block() {
        for tag in [
            "</project>",
            "<dialect>",
            "</ DIALECT >",
            "<io_devices>",
            "<REQUIREMENT_TESTS>",
            "<\n/project>",
        ] {
            let out = neutralise(&format!("a {tag} b"));
            assert!(
                !out.contains(tag) && !out.to_ascii_lowercase().contains("<project"),
                "{out}"
            );
            assert!(out.contains("&lt;"), "{out}");
        }
    }

    #[test]
    fn structured_text_comparisons_are_left_alone() {
        let st = "IF a < b AND c <= d OR e <> f THEN x := 1; END_IF; (* <note> *)";
        assert_eq!(neutralise(st), st);
    }
}
