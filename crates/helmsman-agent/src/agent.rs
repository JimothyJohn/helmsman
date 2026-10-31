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
    /// Serialized history, bytes; under the relay's 1 MiB body limit.
    pub history_bytes: usize,
    /// Messages; the relay takes at most 500.
    pub messages: usize,
    /// One tool result's envelope, bytes; longer ones are cut.
    pub result_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            history_bytes: 900 * 1024,
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
                    out.stop = Stop::Relay(e);
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
