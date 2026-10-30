//! One assistant turn, assembled from the model's stream events as the
//! relay forwards them (`message_start`, `content_block_start`,
//! `content_block_delta`, `content_block_stop`, `message_delta`,
//! `message_stop`, `ping`) plus the relay's own `relay_usage` (the last
//! event of a completed stream) and `relay_error`.
//!
//! The content blocks are kept as the model sent them, `thinking` and
//! its `signature` included, because the relay requires history to go
//! back unchanged. Unknown event types and delta types are ignored, as
//! the protocol asks; an event that breaks the shape (a delta for a block
//! that is not open, data that is not JSON) is [`Malformed`].

use serde_json::{Map, Value as Json, json};

/// Most bytes of tool input accumulated for one block.
pub const MAX_TOOL_INPUT: usize = 2 << 20;
/// Most content blocks in one turn.
pub const MAX_BLOCKS: usize = 256;

/// The stream broke its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub String);

/// An error the stream itself reported (`relay_error`, or the model API's
/// own `error` event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamError {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

/// Token use, from `relay_usage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A tool call in the turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The input, parsed strictly; `Err` holds the text received.
    pub input: Result<Json, String>,
}

/// A finished turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// The content blocks, as they go back into history.
    pub content: Vec<Json>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
}

/// What a caller sees as the turn streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta<'a> {
    Text(&'a str),
    /// A tool call began (its input is still streaming).
    ToolStart {
        id: &'a str,
        name: &'a str,
    },
}

/// What one event did.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Continue,
    /// `relay_usage`: the stream completed.
    Done(Turn),
    /// `relay_error` or `error`: discard the turn.
    Failed(StreamError),
}

struct Block {
    json: Map<String, Json>,
    /// Accumulated `partial_json` for a `tool_use`.
    input: String,
    open: bool,
}

/// Assembles a turn event by event.
#[derive(Default)]
pub struct Builder {
    started: bool,
    stopped: bool,
    blocks: Vec<Block>,
    stop_reason: Option<String>,
}

impl std::fmt::Debug for Builder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Builder")
            .field("blocks", &self.blocks.len())
            .finish_non_exhaustive()
    }
}

fn bad(why: impl Into<String>) -> Malformed {
    Malformed(why.into())
}

fn stream_error(data: &Json) -> StreamError {
    let e = &data["error"];
    let kind = e["type"].as_str().unwrap_or("error").to_string();
    let retryable = e["retryable"].as_bool().unwrap_or(matches!(
        kind.as_str(),
        "overloaded_error" | "api_error" | "upstream_unavailable" | "relay_error"
    ));
    StreamError {
        message: e["message"]
            .as_str()
            .unwrap_or("the stream reported an error")
            .to_string(),
        kind,
        retryable,
    }
}

impl Builder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one event (`data` is its JSON text; the type is read from
    /// it, falling back to the event name).
    ///
    /// # Errors
    /// [`Malformed`].
    pub fn feed(&mut self, name: &str, data: &str, on: &mut dyn FnMut(Delta<'_>)) -> Result<Step, Malformed> {
        let d: Json =
            serde_json::from_str(data).map_err(|_| bad(format!("{name} event: data is not JSON")))?;
        let ty = d["type"].as_str().unwrap_or(name).to_string();
        match ty.as_str() {
            "relay_error" | "error" => return Ok(Step::Failed(stream_error(&d))),
            "relay_usage" => return self.finish(&d).map(Step::Done),
            "message_start" => {
                if self.started {
                    return Err(bad("a second message_start"));
                }
                self.started = true;
            }
            "content_block_start" => self.start_block(&d, on)?,
            "content_block_delta" => self.delta(&d, on)?,
            "content_block_stop" => {
                let b = self.open_block(&d)?;
                b.open = false;
            }
            "message_delta" => {
                self.need_started()?;
                if let Some(r) = d["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(r.to_string());
                }
            }
            "message_stop" => {
                self.need_started()?;
                self.stopped = true;
            }
            _ => {}
        }
        Ok(Step::Continue)
    }

    fn need_started(&self) -> Result<(), Malformed> {
        if self.started {
            Ok(())
        } else {
            Err(bad("an event before message_start"))
        }
    }

    fn index(d: &Json) -> Result<usize, Malformed> {
        d["index"]
            .as_u64()
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| bad("a content block event without an index"))
    }

    fn open_block(&mut self, d: &Json) -> Result<&mut Block, Malformed> {
        self.need_started()?;
        let i = Self::index(d)?;
        match self.blocks.get_mut(i) {
            Some(b) if b.open => Ok(b),
            _ => Err(bad(format!("an event for content block {i}, which is not open"))),
        }
    }

    fn start_block(&mut self, d: &Json, on: &mut dyn FnMut(Delta<'_>)) -> Result<(), Malformed> {
        self.need_started()?;
        let i = Self::index(d)?;
        if i != self.blocks.len() {
            return Err(bad(format!("content block {i} started out of order")));
        }
        if i >= MAX_BLOCKS {
            return Err(bad("too many content blocks"));
        }
        let Some(json) = d["content_block"].as_object().cloned() else {
            return Err(bad("content_block_start without a content_block"));
        };
        let ty = json.get("type").and_then(Json::as_str).unwrap_or_default();
        if ty == "tool_use" {
            let (Some(id), Some(name)) = (
                json.get("id").and_then(Json::as_str),
                json.get("name").and_then(Json::as_str),
            ) else {
                return Err(bad("a tool_use block without an id or a name"));
            };
            on(Delta::ToolStart { id, name });
        }
        if ty == "text"
            && let Some(t) = json.get("text").and_then(Json::as_str).filter(|t| !t.is_empty())
        {
            on(Delta::Text(t));
        }
        self.blocks.push(Block {
            json,
            input: String::new(),
            open: true,
        });
        Ok(())
    }

    fn delta(&mut self, d: &Json, on: &mut dyn FnMut(Delta<'_>)) -> Result<(), Malformed> {
        let delta = &d["delta"];
        let b = self.open_block(d)?;
        let append = |b: &mut Block, key: &str, s: &str| {
            let cur = b.json.get(key).and_then(Json::as_str).unwrap_or_default();
            let joined = format!("{cur}{s}");
            b.json.insert(key.to_string(), Json::String(joined));
        };
        match delta["type"].as_str().unwrap_or_default() {
            "text_delta" => {
                let s = delta["text"]
                    .as_str()
                    .ok_or_else(|| bad("text_delta without text"))?;
                append(b, "text", s);
                on(Delta::Text(s));
            }
            "thinking_delta" => {
                let s = delta["thinking"]
                    .as_str()
                    .ok_or_else(|| bad("thinking_delta without thinking"))?;
                append(b, "thinking", s);
            }
            "signature_delta" => {
                let s = delta["signature"]
                    .as_str()
                    .ok_or_else(|| bad("signature_delta without a signature"))?;
                append(b, "signature", s);
            }
            "input_json_delta" => {
                let s = delta["partial_json"]
                    .as_str()
                    .ok_or_else(|| bad("input_json_delta without partial_json"))?;
                if b.input.len() + s.len() > MAX_TOOL_INPUT {
                    return Err(bad("a tool input over 2 MiB"));
                }
                b.input.push_str(s);
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self, usage: &Json) -> Result<Turn, Malformed> {
        if !self.started || !self.stopped {
            return Err(bad("relay_usage before the message stopped"));
        }
        if self.blocks.iter().any(|b| b.open) {
            return Err(bad("the message stopped with a content block still open"));
        }
        let mut content = Vec::new();
        let mut tool_calls = Vec::new();
        for b in std::mem::take(&mut self.blocks) {
            let mut json = b.json;
            match json.get("type").and_then(Json::as_str) {
                // An empty text block is refused if sent back; it says nothing.
                Some("text") if json.get("text").and_then(Json::as_str).is_none_or(str::is_empty) => continue,
                Some("tool_use") => {
                    let raw = b.input;
                    let input = if raw.trim().is_empty() {
                        // No fragments: the input is the start event's object.
                        Ok(json.get("input").cloned().unwrap_or_else(|| json!({})))
                    } else {
                        serde_json::from_str::<Json>(&raw)
                            .ok()
                            .filter(Json::is_object)
                            .ok_or(raw)
                    };
                    // History needs an object even when the input was broken.
                    json.insert("input".into(), input.clone().unwrap_or_else(|_| json!({})));
                    tool_calls.push(ToolCall {
                        id: json
                            .get("id")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: json
                            .get("name")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input,
                    });
                }
                _ => {}
            }
            content.push(Json::Object(json));
        }
        Ok(Turn {
            content,
            tool_calls,
            stop_reason: self.stop_reason.clone(),
            usage: Usage {
                input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
            },
        })
    }
}
