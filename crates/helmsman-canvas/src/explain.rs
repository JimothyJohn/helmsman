//! Explain mode in the canvas, free of I/O: what the side panel knows
//! about an explanation and an "Ask why" in flight.
//!
//! - [`Steady`]: how long each explained variable has shown its value in
//!   the scans this page received (the templates' "unchanged in the scans
//!   seen over the last 12 s").
//! - [`Sse`]: the event-stream parser for `POST /api/explain`'s body,
//!   fed whatever chunks the browser's reader hands over.
//! - [`Ask`]: one "Ask why": whether it can be asked at all (relay, link,
//!   token), and the box's and the agent's answers as they stream in.

use std::collections::HashMap;

use helmsman_iec::Value;
use serde_json::{Value as Json, json};

/// Per variable: the value last seen and since when (ms on the module's
/// clock).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Steady {
    seen: HashMap<String, (Option<Value>, u64)>,
    last: Option<u64>,
}

impl Steady {
    /// Notes `path` at `value` at `now_ms`.
    pub fn observe(&mut self, path: &str, value: Option<Value>, now_ms: u64) {
        if self.last.is_some_and(|l| now_ms < l) {
            // The clock went back (a new box run, another module): start over.
            self.seen.clear();
        }
        self.last = Some(now_ms);
        match self.seen.get_mut(path) {
            Some((v, _)) if *v == value => {}
            Some(slot) => *slot = (value, now_ms),
            None => {
                self.seen.insert(path.to_string(), (value, now_ms));
            }
        }
    }

    /// How long `path` has shown its current value, if it was seen in
    /// more than one scan.
    #[must_use]
    pub fn ms(&self, path: &str) -> Option<u64> {
        let (_, since) = self.seen.get(path)?;
        let d = self.last?.checked_sub(*since)?;
        (d > 0).then_some(d)
    }
}

/// An event-stream parser over arbitrary chunks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sse {
    buf: String,
}

/// Longest buffered text before the stream is called malformed.
const MAX_SSE: usize = 1 << 20;

impl Sse {
    /// Adds `chunk`; the events it completed, as (name, data).
    ///
    /// # Errors
    /// More than a MiB without an event boundary.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<(String, String)>, String> {
        self.buf.push_str(&chunk.replace("\r\n", "\n"));
        let mut out = Vec::new();
        while let Some(end) = self.buf.find("\n\n") {
            let block: String = self.buf.drain(..end + 2).collect();
            let mut name = String::from("message");
            let mut data: Vec<&str> = Vec::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    name = v.trim_start().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push(v.strip_prefix(' ').unwrap_or(v));
                }
            }
            if !data.is_empty() {
                out.push((name, data.join("\n")));
            }
        }
        if self.buf.len() > MAX_SSE {
            return Err("the answer's stream is malformed".into());
        }
        Ok(out)
    }
}

/// What the transport reports about an "Ask why".
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub enum Reply {
    /// The response was not 200: its status and body.
    Status(u16, String),
    /// Part of the 200 body.
    Chunk(String),
    /// The body ended.
    End,
    /// The request failed (network, aborted).
    Failed(String),
}

/// What the transport hands the explain panel (beside the box view's own
/// [`crate::remote::Msg`]s).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub enum Side {
    /// `GET /api`: its body, or why not.
    Index(Result<String, String>),
    /// The "Ask why" in flight.
    Ask(Reply),
}

/// What the explain panel asks the transport for.
#[derive(Clone, PartialEq, Eq)]
pub enum SideCmd {
    /// `GET /api` (whether the box has an agent); answered with
    /// [`Side::Index`].
    Index,
    /// `POST /api/explain` with this token and body; answered with
    /// [`Side::Ask`]s.
    Ask { token: String, body: String },
}

impl std::fmt::Display for SideCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index => f.write_str("GET /api"),
            // Never the token.
            Self::Ask { body, .. } => write!(f, "POST /api/explain {body}"),
        }
    }
}

impl std::fmt::Debug for SideCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

/// Whether the box has an agent, from `GET /api`'s body.
#[must_use]
pub fn agent(index: &str) -> Option<bool> {
    serde_json::from_str::<Json>(index).ok()?["agent"].as_bool()
}

/// Where an "Ask why" is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Sent; nothing back yet.
    Waiting,
    /// The agent is answering.
    Streaming,
    Done,
    Failed(String),
}

/// One "Ask why" and its answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub module: String,
    pub path: String,
    pub phase: Phase,
    /// The box's own sentences for the scan the agent was given.
    pub sentences: Vec<String>,
    /// The scan the agent was given.
    pub scan: Option<u64>,
    /// The agent's answer so far.
    pub text: String,
    /// One line per tool the agent ran (or was refused).
    pub notes: Vec<String>,
    sse: Sse,
}

/// Why "Ask why" cannot be used now, in the operator's words; `None`: it can.
#[must_use]
pub fn blocked(connected: bool, agent: Option<bool>, token: &str) -> Option<&'static str> {
    if !connected {
        return Some("The box is offline: asking needs the box. The explanation above still works.");
    }
    match agent {
        Some(false) => {
            return Some("No agent on this box (no relay configured). The explanation above still works.");
        }
        None => return Some("Checking whether this box has an agent…"),
        Some(true) => {}
    }
    if !token_ok(token) {
        return Some("Enter the box's API token to ask the agent.");
    }
    None
}

/// Whether `t` looks like the box API token (64 hex digits).
#[must_use]
pub fn token_ok(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The request body for `module` and `path`.
#[must_use]
pub fn body(module: &str, path: &str) -> String {
    json!({ "module": module, "path": path }).to_string()
}

impl Ask {
    #[must_use]
    pub fn new(module: &str, path: &str) -> Self {
        Self {
            module: module.into(),
            path: path.into(),
            phase: Phase::Waiting,
            sentences: Vec::new(),
            scan: None,
            text: String::new(),
            notes: Vec::new(),
            sse: Sse::default(),
        }
    }

    /// Whether it is still going.
    #[must_use]
    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Waiting | Phase::Streaming)
    }

    fn fail(&mut self, why: String) {
        if self.running() {
            self.phase = Phase::Failed(why);
        }
    }

    /// Takes in what the transport saw.
    pub fn apply(&mut self, r: Reply) {
        match r {
            Reply::Status(code, body) => {
                let why = serde_json::from_str::<Json>(&body)
                    .ok()
                    .and_then(|j| j["error"].as_str().map(str::to_string))
                    .unwrap_or_else(|| format!("the box answered {code}"));
                let why = match code {
                    401 => "The API token was refused; check it and ask again.".to_string(),
                    _ => why,
                };
                self.fail(why);
            }
            Reply::Chunk(c) => match self.sse.push(&c) {
                Ok(evs) => {
                    for (name, data) in evs {
                        self.event(&name, &data);
                    }
                }
                Err(e) => self.fail(e),
            },
            Reply::End => {
                if self.running() {
                    self.phase = if self.text.is_empty() {
                        Phase::Failed("The answer ended before the agent said anything.".into())
                    } else {
                        Phase::Done
                    };
                }
            }
            Reply::Failed(why) => self.fail(format!("Could not reach the box: {why}")),
        }
    }

    fn event(&mut self, name: &str, data: &str) {
        let Ok(j) = serde_json::from_str::<Json>(data) else {
            return self.fail("the answer's stream is malformed".into());
        };
        match name {
            "explain" => {
                self.scan = j["scan"].as_u64();
                self.sentences = j["sentences"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s["text"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                self.phase = Phase::Streaming;
            }
            "text" => {
                if let Some(d) = j["delta"].as_str() {
                    self.text.push_str(d);
                    self.phase = Phase::Streaming;
                }
            }
            "tool_result" => {
                let tool = j["name"].as_str().unwrap_or("tool");
                let ok = j["ok"].as_bool().unwrap_or(false);
                let summary = j["summary"].as_str().unwrap_or("");
                let mark = if ok { "read" } else { "refused" };
                self.notes.push(format!("{mark} {tool}: {summary}"));
            }
            "retrying" => self.notes.push("relay busy, retrying…".into()),
            "done" => {
                self.phase = match j["why"].as_str() {
                    Some(w) if self.text.is_empty() => Phase::Failed(w.to_string()),
                    _ => Phase::Done,
                };
            }
            "error" => {
                let msg = j["message"].as_str().unwrap_or("the agent failed").to_string();
                self.phase = Phase::Failed(msg);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_counts_from_the_first_scan_that_showed_the_value() {
        let mut s = Steady::default();
        s.observe("START", Some(Value::Bool(false)), 1000);
        assert_eq!(s.ms("START"), None, "one scan says nothing about how long");
        s.observe("START", Some(Value::Bool(true)), 2000);
        s.observe("START", Some(Value::Bool(true)), 14_000);
        assert_eq!(s.ms("START"), Some(12_000));
        s.observe("START", Some(Value::Bool(false)), 15_000);
        assert_eq!(s.ms("START"), None, "just changed");
        s.observe("START", Some(Value::Bool(false)), 500);
        assert_eq!(s.ms("START"), None, "a clock that went back starts over");
        assert_eq!(s.ms("NEVER"), None);
    }

    #[test]
    fn the_sse_parser_takes_any_chunking() {
        let raw = "event: explain\ndata: {\"scan\":3,\"sentences\":[{\"text\":\"Lamp is ON.\"}]}\n\nevent: text\r\ndata: {\"delta\":\"Hi\"}\r\n\r\n: ping\n\nevent: done\ndata: {\"stop\":\"end_turn\"}\n\n";
        let whole = Sse::default().push(raw).unwrap();
        assert_eq!(whole.len(), 3);
        for size in [1, 2, 7, 64] {
            let mut p = Sse::default();
            let mut got = Vec::new();
            let chars: Vec<char> = raw.chars().collect();
            for c in chars.chunks(size) {
                got.extend(p.push(&c.iter().collect::<String>()).unwrap());
            }
            assert_eq!(got, whole, "chunks of {size}");
        }
        assert!(Sse::default().push(&"x".repeat(MAX_SSE + 1)).is_err());
    }

    #[test]
    fn an_ask_streams_the_box_answer_the_agent_and_tool_notes() {
        let mut a = Ask::new("cell", "CONVEYOR");
        assert!(a.running());
        a.apply(Reply::Chunk(
            "event: explain\ndata: {\"scan\":42,\"sentences\":[{\"text\":\"Conveyor is ON.\",\"depth\":0}]}\n\n".into(),
        ));
        assert_eq!(a.scan, Some(42));
        assert_eq!(a.sentences, ["Conveyor is ON."]);
        assert_eq!(a.phase, Phase::Streaming);
        a.apply(Reply::Chunk("event: tool_result\ndata: {\"name\":\"read_signals\",\"ok\":true,\"summary\":\"1 value(s)\"}\n\nevent: text\ndata: {\"delta\":\"The conveyor \"}\n\nevent: te".into()));
        a.apply(Reply::Chunk("xt\ndata: {\"delta\":\"runs.\"}\n\nevent: tool_result\ndata: {\"name\":\"propose_change\",\"ok\":false,\"summary\":\"not available\"}\n\n".into()));
        assert_eq!(a.text, "The conveyor runs.");
        assert_eq!(
            a.notes,
            [
                "read read_signals: 1 value(s)",
                "refused propose_change: not available"
            ]
        );
        a.apply(Reply::Chunk(
            "event: done\ndata: {\"stop\":\"end_turn\",\"why\":null}\n\n".into(),
        ));
        a.apply(Reply::End);
        assert_eq!(a.phase, Phase::Done);
        assert!(!a.running());
    }

    #[test]
    fn failures_say_what_to_do() {
        let mut a = Ask::new("cell", "X");
        a.apply(Reply::Status(401, "{\"error\":\"wrong token\"}".into()));
        assert_eq!(
            a.phase,
            Phase::Failed("The API token was refused; check it and ask again.".into())
        );
        let mut a = Ask::new("cell", "X");
        a.apply(Reply::Status(
            503,
            "{\"error\":\"no relay configured on this box\"}".into(),
        ));
        assert_eq!(a.phase, Phase::Failed("no relay configured on this box".into()));
        let mut a = Ask::new("cell", "X");
        a.apply(Reply::Chunk("event: explain\ndata: {\"scan\":1,\"sentences\":[]}\n\nevent: error\ndata: {\"kind\":\"relay_unreachable\",\"message\":\"the agent is offline\"}\n\n".into()));
        a.apply(Reply::End);
        assert_eq!(a.phase, Phase::Failed("the agent is offline".into()));
        let mut a = Ask::new("cell", "X");
        a.apply(Reply::Chunk("event: text\ndata: not json\n\n".into()));
        assert!(matches!(a.phase, Phase::Failed(_)));
        let mut a = Ask::new("cell", "X");
        a.apply(Reply::End);
        assert!(
            matches!(a.phase, Phase::Failed(_)),
            "an empty answer is not an answer"
        );
        let mut a = Ask::new("cell", "X");
        a.apply(Reply::Failed("TypeError".into()));
        assert_eq!(
            a.phase,
            Phase::Failed("Could not reach the box: TypeError".into())
        );
    }

    #[test]
    fn asking_is_blocked_with_a_reason_until_box_agent_and_token_are_there() {
        let token = "a".repeat(64);
        assert!(blocked(false, Some(true), &token).unwrap().contains("offline"));
        assert!(blocked(true, Some(false), &token).unwrap().contains("No agent"));
        assert!(blocked(true, None, &token).is_some());
        assert!(blocked(true, Some(true), "").unwrap().contains("token"));
        assert!(blocked(true, Some(true), &"g".repeat(64)).is_some());
        assert_eq!(blocked(true, Some(true), &token), None);
        assert_eq!(body("cell", "M.ON"), r#"{"module":"cell","path":"M.ON"}"#);
    }
}
