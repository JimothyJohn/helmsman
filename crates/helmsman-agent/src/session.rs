//! The box's client for relay protocol v2 (`docs/relay-protocol-v2.md`):
//! the relay keeps the conversation. A session is opened once
//! (`POST /v2/sessions`); each step (`POST /v2/sessions/{id}/steps`) sends
//! only new input (a user message, the results of the client tools a step
//! yielded, or `continue`) and answers with an event stream: the model's
//! text as it streams, the relay's own tool runs, any document it stored,
//! and last `harness_yield`, which names the client tools to run next.
//!
//! Nothing the relay sends is trusted: events are bounded in size and
//! count, ids are checked against the protocol's shapes before they go
//! into a path, a yield names at most [`MAX_CALLS`] tools, and an event
//! this box does not know is ignored while a yield reason it does not
//! know is refused.

use serde_json::{Value as Json, json};

use crate::http::{self, Request};
use crate::relay::{Config, Error, from_http, status_error};
use crate::sse;
use crate::turn::{Builder, Delta, Step, ToolCall, stream_error};

/// Most client tool calls one yield may name.
pub const MAX_CALLS: usize = 64;
/// Most stream events read in one step.
pub const MAX_EVENTS: usize = 500_000;
/// Most documents one step may store (the relay runs at most 16 tools).
pub const MAX_DOCS: usize = 16;
/// Largest JSON body read outside a stream.
const MAX_JSON: usize = 256 * 1024;
/// Longest tool id or name taken from a yield.
const MAX_ID: usize = 200;

/// The tools the relay itself runs in a step (protocol v2, "Relay-run
/// tools"). A yield never names one; if it does, the box does not run it.
pub const RELAY_TOOLS: [&str; 6] = [
    "doc_list",
    "doc_grep",
    "doc_read",
    "history_search",
    "history_read",
    "fetch_document",
];

/// New input for a session.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A person's message: content blocks.
    User(Vec<Json>),
    /// One result per waiting client tool call.
    ToolResults(Vec<Json>),
    /// Resume a session that is ready.
    Continue,
}

impl Input {
    fn json(&self) -> Json {
        match self {
            Self::User(blocks) => json!({ "type": "user", "content": blocks }),
            Self::ToolResults(results) => json!({ "type": "tool_results", "results": results }),
            Self::Continue => json!({ "type": "continue" }),
        }
    }
}

/// What the stream tells the caller as it goes.
#[derive(Debug, PartialEq, Eq)]
pub enum Event<'a> {
    Text(&'a str),
    /// A tool the relay ran: its id, name (one of [`RELAY_TOOLS`], else
    /// `relay_tool`), whether it failed.
    RelayTool {
        id: &'a str,
        name: &'a str,
        is_error: bool,
    },
}

/// How a step ended (`harness_yield`).
#[derive(Debug, Clone, PartialEq)]
pub struct Yield {
    pub reason: String,
    /// The client tool calls to run, for `tool_use`.
    pub calls: Vec<ToolCall>,
}

/// A finished step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepEnd {
    pub yielded: Yield,
    /// `harness_document` facts, at most [`MAX_DOCS`].
    pub documents: Vec<Json>,
    /// Output tokens over the step's model calls (`relay_usage`).
    pub output_tokens: u64,
}

/// Where a session stands (`GET /v2/sessions/{id}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Phase {
    pub state: String,
    pub calls: Vec<ToolCall>,
}

/// A session id: `s` and 32 lowercase hex digits.
#[must_use]
pub fn session_id_ok(id: &str) -> bool {
    hex_id(id, 's', 32)
}

/// A document id: `d` and 16 lowercase hex digits. Checked before it is
/// put in a path.
#[must_use]
pub fn doc_id_ok(id: &str) -> bool {
    hex_id(id, 'd', 16)
}

fn hex_id(id: &str, lead: char, n: usize) -> bool {
    id.strip_prefix(lead).is_some_and(|h| {
        h.len() == n
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn bad(why: impl Into<String>) -> Error {
    Error::Malformed(why.into())
}

fn send(
    cfg: &Config,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    accept: &str,
) -> Result<http::Response, Error> {
    let path = format!("{}{path}", cfg.url.prefix);
    let auth = format!("Bearer {}", cfg.token);
    let req = Request {
        method,
        path: &path,
        headers: &[("Authorization", &auth), ("Accept", accept)],
        json: body,
        raw: None,
    };
    http::send(&cfg.url, &req, cfg.timeouts).map_err(from_http)
}

fn json_body(resp: http::Response) -> Result<Json, Error> {
    let b = resp.read_body(MAX_JSON).map_err(from_http)?;
    serde_json::from_slice(&b).map_err(|_| bad("the relay's answer is not JSON"))
}

/// Opens a session for `cfg.version`.
///
/// # Errors
/// [`Error`]: unreachable, a status (a v1 profile is a 400, an unknown one
/// a 404), or an answer without a well-formed session id.
pub fn open(cfg: &Config) -> Result<String, Error> {
    let body = serde_json::to_vec(&json!({ "agent": cfg.version })).unwrap_or_default();
    let resp = send(cfg, "POST", "/v2/sessions", Some(&body), "application/json")?;
    if resp.status != 201 && resp.status != 200 {
        return Err(status_error(resp));
    }
    let j = json_body(resp)?;
    match j["session_id"].as_str() {
        Some(id) if session_id_ok(id) => Ok(id.to_string()),
        _ => Err(bad("the relay opened a session with an id that is not one")),
    }
}

/// Where the session stands.
///
/// # Errors
/// [`Error`]; a 404 is a session the relay no longer has.
pub fn phase(cfg: &Config, session: &str) -> Result<Phase, Error> {
    if !session_id_ok(session) {
        return Err(bad("not a session id"));
    }
    let resp = send(
        cfg,
        "GET",
        &format!("/v2/sessions/{session}"),
        None,
        "application/json",
    )?;
    if resp.status != 200 {
        return Err(status_error(resp));
    }
    let j = json_body(resp)?;
    let state = j["phase"]["state"]
        .as_str()
        .ok_or_else(|| bad("a session without a phase"))?;
    let calls = match j["phase"]["tool_uses"].as_array() {
        Some(a) => calls_of(a)?,
        None => Vec::new(),
    };
    Ok(Phase {
        state: state.to_string(),
        calls,
    })
}

/// Sends one step's input and reads its stream to `harness_yield`.
///
/// # Errors
/// [`Error`]. A status before the stream is the relay's refusal (`404`:
/// no such session, `409`: `session_busy` or `conflict`); `relay_error`
/// in the stream is [`Error::Stream`]; a stream that ends without a yield
/// is [`Error::CutOff`].
pub fn step(
    cfg: &Config,
    session: &str,
    input: &Input,
    on: &mut dyn FnMut(Event<'_>),
) -> Result<StepEnd, Error> {
    if !session_id_ok(session) {
        return Err(bad("not a session id"));
    }
    let body = serde_json::to_vec(&json!({ "input": input.json() })).unwrap_or_default();
    let resp = send(
        cfg,
        "POST",
        &format!("/v2/sessions/{session}/steps"),
        Some(&body),
        "text/event-stream",
    )?;
    if resp.status != 200 {
        return Err(status_error(resp));
    }
    if !resp
        .header("content-type")
        .is_some_and(|t| t.starts_with("text/event-stream"))
    {
        return Err(bad("the step is not an event stream"));
    }
    let mut body = resp.body;
    let mut reader = sse::Reader::new();
    let mut turn = Builder::new();
    let mut documents: Vec<Json> = Vec::new();
    let mut output_tokens = 0u64;
    for _ in 0..MAX_EVENTS {
        let ev = match reader.next(&mut body) {
            Ok(Some(ev)) => ev,
            Ok(None) => return Err(Error::CutOff),
            Err(sse::Error::Read(e)) => return Err(from_http(e)),
            Err(e) => return Err(bad(e.to_string())),
        };
        let d: Json = serde_json::from_str(&ev.data)
            .map_err(|_| bad(format!("{} event: data is not JSON", ev.name)))?;
        let ty = d["type"].as_str().unwrap_or(&ev.name).to_string();
        match ty.as_str() {
            "harness_session" | "harness_compaction" => {}
            // A new model call: a fresh turn. (A recap call forwards no
            // message events.)
            "harness_call" => turn = Builder::new(),
            // The relay keeps the assistant turn; the box only wants the
            // usage, so the turn builder is never finished.
            "relay_usage" => {
                output_tokens = output_tokens.saturating_add(d["output_tokens"].as_u64().unwrap_or(0));
            }
            "relay_error" | "error" => return Err(Error::Stream(stream_error(&d))),
            "harness_tool" => {
                let name = d["name"]
                    .as_str()
                    .filter(|n| RELAY_TOOLS.contains(n))
                    .unwrap_or("relay_tool");
                let id = d["tool_use_id"]
                    .as_str()
                    .filter(|i| i.len() <= MAX_ID)
                    .unwrap_or("");
                on(Event::RelayTool {
                    id,
                    name,
                    is_error: d["is_error"].as_bool().unwrap_or(false),
                });
            }
            "harness_document" => {
                if documents.len() < MAX_DOCS && d["doc"].is_object() {
                    documents.push(d["doc"].clone());
                }
            }
            "harness_yield" => {
                return Ok(StepEnd {
                    yielded: parse_yield(&d)?,
                    documents,
                    output_tokens,
                });
            }
            // The model's own events, and anything the protocol adds
            // later (the builder ignores types it does not know).
            _ => {
                let mut forward = |x: Delta<'_>| {
                    if let Delta::Text(t) = x {
                        on(Event::Text(t));
                    }
                };
                match turn.feed(&ev.name, &ev.data, &mut forward) {
                    Ok(Step::Continue) => {}
                    Ok(_) => return Err(bad("an event that ends a v1 turn")),
                    Err(m) => return Err(Error::Malformed(m.0)),
                }
            }
        }
    }
    Err(bad("a step with too many events"))
}

fn calls_of(a: &[Json]) -> Result<Vec<ToolCall>, Error> {
    if a.len() > MAX_CALLS {
        return Err(bad(format!("a yield naming more than {MAX_CALLS} tool calls")));
    }
    a.iter()
        .map(|u| {
            let (Some(id), Some(name)) = (u["id"].as_str(), u["name"].as_str()) else {
                return Err(bad("a tool call without an id or a name"));
            };
            if id.is_empty() || id.len() > MAX_ID || name.is_empty() || name.len() > MAX_ID {
                return Err(bad("a tool call with an id or name of the wrong length"));
            }
            let input = if u["input"].is_object() {
                Ok(u["input"].clone())
            } else {
                Err(u["input"].to_string())
            };
            Ok(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                input,
            })
        })
        .collect()
}

fn parse_yield(d: &Json) -> Result<Yield, Error> {
    let reason = d["reason"].as_str().unwrap_or_default();
    if ![
        "tool_use",
        "end_turn",
        "max_tokens",
        "refusal",
        "stop_sequence",
        "continue",
    ]
    .contains(&reason)
    {
        return Err(bad(format!(
            "a yield with a reason this box does not know ({:?})",
            reason.chars().take(40).collect::<String>()
        )));
    }
    let calls = match d["tool_uses"].as_array() {
        Some(a) if reason == "tool_use" => calls_of(a)?,
        _ => Vec::new(),
    };
    if reason == "tool_use" && calls.is_empty() {
        return Err(bad("a tool_use yield that names no tool"));
    }
    Ok(Yield {
        reason: reason.to_string(),
        calls,
    })
}

/// `GET /v2/docs/{id}/raw`: the document's original bytes, with the
/// headers `x-document-name`, `-sha256` and `-kind`. The caller checks
/// and stores it ([`crate::tools::store_fetched`]).
///
/// # Errors
/// [`Error`]; an id that is not a document id is refused before any
/// request.
pub fn raw_document(cfg: &Config, id: &str) -> Result<http::Response, Error> {
    if !doc_id_ok(id) {
        return Err(bad("not a document id"));
    }
    let resp = send(cfg, "GET", &format!("/v2/docs/{id}/raw"), None, "*/*")?;
    if resp.status != 200 {
        return Err(status_error(resp));
    }
    Ok(resp)
}
