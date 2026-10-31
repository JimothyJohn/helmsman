//! The tool loop behind the chat panel: send the conversation to the
//! relay, stream the assistant turn, run its tool calls on the box, send
//! the results, repeat until the model ends its turn or a budget runs out.
//!
//! Rules from the relay protocol, kept here:
//! - history is append-only, assistant turns go back exactly as received
//!   (`thinking` blocks included); a failed turn is discarded whole and
//!   none of its tool calls run;
//! - `refusal`: nothing runs; `max_tokens`: a tool call in that turn may be
//!   cut off, so none runs;
//! - every tool input is parsed strictly and validated before it runs; a
//!   bad one is answered `is_error` with `{"INVALID_JSON": <text>}`;
//! - all results for one turn go back in one user message;
//! - retryable errors are retried with backoff (2 s doubling to 60 s,
//!   jittered), others are not.
//!
//! What the loop can do is fixed by [`crate::tools`]: no tool applies a
//! proposal, and nothing in a tool result can add one.

use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use crate::relay::{self, Error as RelayError};
use crate::tools::{self, BoxApi, Drafts};
use crate::turn::{Delta, Turn};

/// What one user message may cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Model turns (relay calls that completed).
    pub steps: usize,
    /// Output tokens, summed over the turns (`relay_usage`).
    pub output_tokens: u64,
    /// Tool calls run per assistant turn; the rest are answered unrun.
    pub tools_per_turn: usize,
    /// Wall time from the user's message; checked before each turn.
    pub time: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            steps: 24,
            output_tokens: 200_000,
            tools_per_turn: 16,
            time: Duration::from_secs(15 * 60),
        }
    }
}

/// Retries of retryable relay errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// Extra attempts after the first.
    pub attempts: u32,
    pub first: Duration,
    pub cap: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 2,
            first: Duration::from_secs(2),
            cap: Duration::from_secs(60),
        }
    }
}

/// Bounds on one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Serialized history, bytes; under the relay's 4 MiB body limit
    /// (relay protocol v1 since PDF documents), with room for one 2 MiB
    /// PDF (2.7 MiB as base64) and the rest of the chat.
    pub history_bytes: usize,
    /// Messages; the relay takes at most 500.
    pub messages: usize,
    /// One tool result's envelope, bytes; longer ones are cut.
    pub result_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            history_bytes: 3584 * 1024,
            messages: 480,
            result_bytes: 48 * 1024,
        }
    }
}

/// The agent: a relay to think with, the box to act on.
#[derive(Debug, Clone)]
pub struct Agent {
    pub relay: relay::Config,
    pub api: BoxApi,
    pub budget: Budget,
    pub retry: Retry,
    pub limits: Limits,
}

/// One conversation's state.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// The messages as sent to the relay.
    pub messages: Vec<Json>,
    pub drafts: Drafts,
}

impl Conversation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialized size of the history.
    #[must_use]
    pub fn bytes(&self) -> usize {
        serde_json::to_vec(&self.messages).map_or(usize::MAX, |v| v.len())
    }

    /// Adds `blocks` to the trailing user message, or starts one. A user
    /// message stays last after a failed turn or a spent budget (its tool
    /// results included), so the next one joins it and roles alternate.
    fn push_user(&mut self, blocks: Vec<Json>) {
        if let Some(last) = self.messages.last_mut()
            && last["role"] == "user"
            && let Some(c) = last["content"].as_array_mut()
        {
            c.extend(blocks);
            return;
        }
        self.messages.push(json!({ "role": "user", "content": blocks }));
    }
}

/// The most of a relay's refusal message kept in a note or shown.
const REASON_CHARS: usize = 300;

/// The start of the box's note after an attached PDF's document block
/// (`helmsman-plc`'s chat route writes `... drop-in {id} (PDF, ...`).
const PDF_NOTE: &str = "[box] The user attached drop-in ";

/// A refusal's reason as a person reads it: without the block's address
/// in front (stale once the block is replaced), at most [`REASON_CHARS`].
fn reason(message: &str) -> &str {
    let s = match message.split_once(": ") {
        Some((at, why)) if located(at).is_some() => why,
        _ => message,
    };
    s.char_indices().nth(REASON_CHARS).map_or(s, |(k, _)| &s[..k])
}

/// The content block a relay refusal names, if it names one: the relay's
/// own checks say `messages[i].content[j]`, the model API's (passed
/// through as `upstream_rejected`) `messages.i.content.j`.
fn located(message: &str) -> Option<(usize, usize)> {
    fn index<'a>(s: &'a str, open: char, close: &str) -> Option<(usize, &'a str)> {
        let s = s.strip_prefix(open)?;
        let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        let n = s[..end].parse().ok()?;
        Some((n, s[end..].strip_prefix(close).unwrap_or(&s[end..])))
    }
    message.match_indices("messages").find_map(|(k, _)| {
        let rest = &message[k + "messages".len()..];
        let (open, close) = if rest.starts_with('[') {
            ('[', "]")
        } else {
            ('.', "")
        };
        let (i, rest) = index(rest, open, close)?;
        let (j, _) = index(rest.strip_prefix(".content")?, open, close)?;
        Some((i, j))
    })
}

impl Conversation {
    /// After a failed turn: when the relay refused it because of a PDF
    /// document block, replace that block (and the box's note after it)
    /// with a note saying it was refused and why, so the conversation can
    /// go on. Returns how many were replaced (0: nothing was touched).
    ///
    /// Only a refusal of the request itself (`invalid_request`,
    /// `upstream_rejected`) counts. When it names a block
    /// (`messages[i].content[j]`), that block must be a `document` in a
    /// user message, else the refusal is about something else. When it
    /// names none but is about PDFs (the relay's per-request page and
    /// document caps), the documents in the trailing user message, the
    /// ones added since the model last answered, are replaced.
    pub fn drop_refused_documents(&mut self, e: &RelayError) -> usize {
        let RelayError::Status { kind, message, .. } = e else {
            return 0;
        };
        if kind != "invalid_request" && kind != "upstream_rejected" {
            return 0;
        }
        let is_doc = |m: &Json, j: usize| m["role"] == "user" && m["content"][j]["type"] == "document";
        let (i, js) = if let Some((i, j)) = located(message) {
            match self.messages.get(i) {
                Some(m) if is_doc(m, j) => (i, vec![j]),
                _ => return 0,
            }
        } else {
            let lower = message.to_ascii_lowercase();
            if !lower.contains("pdf") && !lower.contains("document") {
                return 0;
            }
            let Some(i) = self.messages.len().checked_sub(1) else {
                return 0;
            };
            let n = self.messages[i]["content"].as_array().map_or(0, Vec::len);
            (i, (0..n).filter(|&j| is_doc(&self.messages[i], j)).collect())
        };
        let reason = reason(message);
        let Some(content) = self.messages[i]["content"].as_array_mut() else {
            return 0;
        };
        for &j in js.iter().rev() {
            let id = content
                .get(j + 1)
                .and_then(|b| b["text"].as_str())
                .and_then(|t| t.strip_prefix(PDF_NOTE))
                .and_then(|t| t.split_once(" (PDF, "))
                .and_then(|(id, _)| id.parse::<u64>().ok());
            let note = if let Some(id) = id {
                content.remove(j + 1);
                format!("[box] The user attached drop-in {id} (PDF) here, but the relay refused it")
            } else {
                "[box] A PDF the user attached here was refused by the relay".to_string()
            };
            content[j] = json!({ "type": "text", "text": format!(
                "{note}: {reason}. It is not in this conversation and you have not seen it. If it matters, ask the user for a copy that is not encrypted, or for only the pages that matter."
            ) });
        }
        js.len()
    }
}

/// What the chat panel is told as the loop runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Text(String),
    /// The model asked for a tool.
    ToolCall {
        id: String,
        name: String,
        input: Json,
    },
    /// A tool ran, or was refused.
    ToolResult {
        id: String,
        name: String,
        ok: bool,
        summary: String,
    },
    /// Proposals stored for approval, with the model's summary.
    Proposal {
        ids: Vec<u64>,
        summary: String,
    },
    /// A retryable relay error; trying again after `after`.
    Retrying {
        error: String,
        after: Duration,
    },
}

/// Why the loop stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    /// The model ended its turn.
    Done,
    /// A budget ran out; the next message continues the conversation.
    Budget(String),
    /// The model declined; nothing ran.
    Refusal,
    /// The relay failed; the turn was discarded.
    Relay(RelayError),
    /// The relay refused the turn because of the PDF document blocks in
    /// it (encrypted, too many pages, or one the model API would not
    /// read). Each was replaced in the history by a note saying so, so the
    /// next message goes through. `reason` is the relay's message.
    DocumentRefused { reason: String, dropped: usize },
    /// The conversation is too long to send; start a new one.
    TooLong,
}

/// How a user message went.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub stop: Stop,
    pub steps: usize,
    pub output_tokens: u64,
    pub proposals: Vec<u64>,
}

/// A cheap jitter source: not for anything secret.
fn jitter(max: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let max_ms = u64::try_from(max.as_millis()).unwrap_or(u64::MAX).max(1);
    Duration::from_millis(u64::from(nanos) % max_ms)
}

fn error_result(id: &str, tool: &str, data: &Json) -> Json {
    json!({
        "type": "tool_result",
        "tool_use_id": id,
        "is_error": true,
        "content": tools::envelope(tool, data),
    })
}

/// The tool's name as written into an envelope: one of ours, or `unknown`
/// (a made-up name is model text and goes nowhere near the markup).
fn label(name: &str) -> &str {
    tools::NAMES
        .iter()
        .find(|n| **n == name)
        .copied()
        .unwrap_or("unknown")
}

impl Agent {
    /// One relay turn, retried while retryable.
    fn turn(
        &self,
        messages: &[Json],
        until: Instant,
        on: &mut dyn FnMut(Progress),
    ) -> Result<Turn, RelayError> {
        let mut wait = self.retry.first;
        let mut attempt = 0;
        loop {
            let mut forward = |d: Delta<'_>| {
                if let Delta::Text(t) = d {
                    on(Progress::Text(t.to_string()));
                }
            };
            match relay::turn(&self.relay, messages, &mut forward) {
                Ok(t) => return Ok(t),
                Err(e) if e.retryable() && attempt < self.retry.attempts => {
                    let after = wait + jitter(wait / 2 + Duration::from_millis(1));
                    if Instant::now() + after >= until {
                        return Err(e);
                    }
                    on(Progress::Retrying {
                        error: e.to_string(),
                        after,
                    });
                    std::thread::sleep(after);
                    attempt += 1;
                    wait = (wait * 2).min(self.retry.cap);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Answers one tool call: run it, or say why not.
    fn answer(
        &self,
        conv: &mut Conversation,
        call: &crate::turn::ToolCall,
        set: &[&str],
        allowed: Result<(), String>,
        on: &mut dyn FnMut(Progress),
        proposals: &mut Vec<u64>,
    ) -> Json {
        let tool = label(&call.name);
        on(Progress::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call
                .input
                .clone()
                .unwrap_or_else(|raw| json!({ "unparsed": raw })),
        });
        let refuse = |why: String, data: Json, on: &mut dyn FnMut(Progress)| {
            on(Progress::ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                ok: false,
                summary: why,
            });
            error_result(&call.id, tool, &data)
        };
        if let Err(why) = allowed {
            return refuse(why.clone(), json!({ "error": why, "ran": false }), on);
        }
        let input = match &call.input {
            Ok(i) => i,
            Err(raw) => {
                return refuse(
                    "the tool input is not valid JSON; not run".into(),
                    json!({ "INVALID_JSON": raw }),
                    on,
                );
            }
        };
        if !tools::NAMES.contains(&call.name.as_str()) {
            let why = format!(
                "no tool named {:?} on this box; the tools are {}. Nothing was run.",
                call.name,
                set.join(", ")
            );
            return refuse(why.clone(), json!({ "error": why, "ran": false }), on);
        }
        if !set.contains(&call.name.as_str()) {
            let why = format!(
                "{} is not available in this mode (read-only); the tools here are {}. Nothing was run.",
                call.name,
                set.join(", ")
            );
            return refuse(why.clone(), json!({ "error": why, "ran": false }), on);
        }
        if let Err(e) = tools::validate(&call.name, input) {
            let raw = serde_json::to_string(input).unwrap_or_default();
            return refuse(
                format!("the input does not fit the schema ({e}); not run"),
                json!({ "INVALID_JSON": raw }),
                on,
            );
        }
        let out = tools::run(&self.api, &mut conv.drafts, &call.name, input);
        if !out.proposals.is_empty() {
            proposals.extend(&out.proposals);
            let summary = input["summary"].as_str().unwrap_or_default().to_string();
            on(Progress::Proposal {
                ids: out.proposals.clone(),
                summary,
            });
        }
        on(Progress::ToolResult {
            id: call.id.clone(),
            name: call.name.clone(),
            ok: out.ok,
            summary: out.summary.clone(),
        });
        let mut text = tools::envelope(tool, &out.data);
        if text.len() > self.limits.result_bytes {
            let cut = json!({
                "truncated": true,
                "bytes": text.len(),
                "why": "the result is larger than one tool result may be; ask for less (a prefix, fewer names)",
            });
            text = tools::envelope(tool, &cut);
        }
        let mut r = json!({ "type": "tool_result", "tool_use_id": call.id, "content": text });
        if !out.ok {
            r["is_error"] = json!(true);
        }
        r
    }

    /// Runs the loop for one user message.
    pub fn chat(&self, conv: &mut Conversation, text: &str, on: &mut dyn FnMut(Progress)) -> Outcome {
        self.chat_blocks(conv, vec![json!({ "type": "text", "text": text })], on)
    }

    /// [`Agent::chat`] for a user message of several content blocks (the
    /// user's text, then the box's notes about attached drop-ins), with
    /// every tool.
    pub fn chat_blocks(
        &self,
        conv: &mut Conversation,
        blocks: Vec<Json>,
        on: &mut dyn FnMut(Progress),
    ) -> Outcome {
        self.chat_with(conv, blocks, &tools::NAMES, on)
    }

    /// Runs the loop for one user message of `blocks`, running only the
    /// tools in `set` (a subset of [`tools::NAMES`]); a call to any other
    /// is answered "not available", unrun. Explain mode passes
    /// [`tools::READ_ONLY`].
    pub fn chat_with(
        &self,
        conv: &mut Conversation,
        blocks: Vec<Json>,
        set: &[&str],
        on: &mut dyn FnMut(Progress),
    ) -> Outcome {
        let until = Instant::now() + self.budget.time;
        conv.push_user(blocks);
        let mut out = Outcome {
            stop: Stop::Done,
            steps: 0,
            output_tokens: 0,
            proposals: Vec::new(),
        };
        loop {
            if out.steps >= self.budget.steps {
                out.stop = Stop::Budget(format!("{} model turns", self.budget.steps));
                return out;
            }
            if Instant::now() >= until {
                out.stop = Stop::Budget("time".into());
                return out;
            }
            if conv.messages.len() + 2 > self.limits.messages || conv.bytes() > self.limits.history_bytes {
                out.stop = Stop::TooLong;
                return out;
            }
            let turn = match self.turn(&conv.messages, until, on) {
                Ok(t) => t,
                Err(e) => {
                    let dropped = conv.drop_refused_documents(&e);
                    out.stop = match e {
                        RelayError::Status { message, .. } if dropped > 0 => Stop::DocumentRefused {
                            reason: reason(&message).to_string(),
                            dropped,
                        },
                        e => Stop::Relay(e),
                    };
                    return out;
                }
            };
            out.steps += 1;
            out.output_tokens += turn.usage.output_tokens;
            let stop_reason = turn.stop_reason.as_deref().unwrap_or("end_turn");
            if stop_reason == "refusal" {
                out.stop = Stop::Refusal;
                return out;
            }
            if !turn.content.is_empty() {
                conv.messages
                    .push(json!({ "role": "assistant", "content": turn.content }));
            }
            if turn.tool_calls.is_empty() {
                out.stop = Stop::Done;
                return out;
            }
            let mut results = Vec::with_capacity(turn.tool_calls.len());
            for (k, call) in turn.tool_calls.iter().enumerate() {
                let allowed = if stop_reason == "max_tokens" {
                    Err("the turn hit its output limit, so this call may be cut off; not run".to_string())
                } else if k >= self.budget.tools_per_turn {
                    Err(format!(
                        "at most {} tool calls run per turn; not run",
                        self.budget.tools_per_turn
                    ))
                } else {
                    Ok(())
                };
                results.push(self.answer(conv, call, set, allowed, on, &mut out.proposals));
            }
            conv.push_user(results);
            if out.output_tokens >= self.budget.output_tokens {
                out.stop = Stop::Budget(format!("{} output tokens", self.budget.output_tokens));
                return out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn refusal(kind: &str, message: &str) -> RelayError {
        RelayError::Status {
            status: 400,
            kind: kind.into(),
            message: message.into(),
            retryable: false,
        }
    }

    fn pdf_blocks() -> Vec<Json> {
        vec![
            json!({ "type": "text", "text": "read this" }),
            json!({ "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0=" } }),
            json!({ "type": "text", "text": "[box] The user attached drop-in 7 (PDF, 5 bytes): the document just above." }),
        ]
    }

    fn with_pdf() -> Conversation {
        let mut c = Conversation::new();
        c.push_user(pdf_blocks());
        c
    }

    #[test]
    fn a_refusal_names_its_block_in_either_spelling() {
        assert_eq!(located("messages[3].content[12]: encrypted"), Some((3, 12)));
        assert_eq!(located("messages.0.content.1.pdf.source: bad"), Some((0, 1)));
        assert_eq!(located("1 to 500 messages; messages[2] has unknown field"), None);
        assert_eq!(located("at most 100 PDF pages per request"), None);
        assert_eq!(located("messages[99999999999999999999].content[0]"), None);
    }

    #[test]
    fn only_a_refused_document_is_replaced() {
        let mut c = with_pdf();
        let before = c.messages.clone();
        for e in [
            refusal(
                "invalid_request",
                "messages[0].content[0]: a text block needs text",
            ),
            refusal("invalid_request", "messages[5].content[1]: encrypted"),
            refusal("invalid_request", "1 to 500 messages"),
            refusal("unauthorized", "missing or invalid box token"),
            refusal("relay_error", "PDF pages"),
            RelayError::Timeout,
        ] {
            assert_eq!(c.drop_refused_documents(&e), 0, "{e}");
            assert_eq!(c.messages, before, "{e}");
        }
        let e = refusal(
            "invalid_request",
            "messages[0].content[1]: encrypted or password-protected PDFs are not supported",
        );
        assert_eq!(c.drop_refused_documents(&e), 1);
        let content = c.messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2, "the note after it went with it: {content:?}");
        let note = content[1]["text"].as_str().unwrap();
        assert!(
            note.starts_with(
                "[box] The user attached drop-in 7 (PDF) here, but the relay refused it: encrypted"
            ),
            "{note}"
        );
        assert_eq!(c.drop_refused_documents(&e), 0, "nothing left to replace");
    }

    #[test]
    fn an_unlocated_pdf_refusal_replaces_the_trailing_messages_documents_only() {
        let mut c = with_pdf();
        c.messages
            .push(json!({ "role": "assistant", "content": [{ "type": "text", "text": "ok" }] }));
        let earlier = c.messages.clone();
        let e = refusal("invalid_request", "at most 100 PDF pages per request");
        assert_eq!(c.drop_refused_documents(&e), 0, "an answered PDF stays");
        c.push_user(pdf_blocks());
        c.push_user(pdf_blocks());
        assert_eq!(c.drop_refused_documents(&e), 2);
        assert_eq!(c.messages[..2], earlier[..]);
        assert!(!c.messages[2].to_string().contains("\"document\""));
        assert_eq!(c.messages[2]["content"].as_array().unwrap().len(), 4);
    }

    proptest! {
        #[test]
        fn any_refusal_message_is_safe(message in ".{0,80}", i in 0usize..3, j in 0usize..5) {
            let mut c = with_pdf();
            let _ = located(&message);
            let _ = c.drop_refused_documents(&refusal("invalid_request", &message));
            let m = format!("messages[{i}].content[{j}]: {message}");
            let n = c.drop_refused_documents(&refusal("upstream_rejected", &m));
            prop_assert!(n <= 1);
            prop_assert_eq!(c.messages.len(), 1);
        }
    }
}
