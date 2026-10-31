//! `POST /api/explain`: explain mode's second layer. The canvas already
//! shows the box's own explanation of why a variable is in its state
//! (`helmsman_diagram::phrase`, no model); "Ask why" sends the same facts
//! to the agent for a plain-language answer.
//!
//! The request names only `{module, path}`. The box builds everything the
//! agent sees itself, from its own newest traced scan of that module: the
//! structured explanation (`helmsman_diagram::explain`), the template
//! sentences, and the source lines they cite. All of it is plant data and
//! goes to the model inside the `<box-data>` envelope; the words outside
//! the envelope are fixed here and carry no plant text.
//!
//! The agent loop runs with [`tools::READ_ONLY`]: it can read signals and
//! modules, never draft, check, simulate or propose, whatever the model
//! asks for or the plant text says. A conversation is one question; none
//! is kept.
//!
//! ```text
//! event: explain      {"module", "path", "scan", "fault", "sentences": [{text, depth, line}]}
//! event: text         {"delta": "..."}
//! event: tool_call    {"id", "name", "input"}
//! event: tool_result  {"id", "name", "ok", "summary"}
//! event: retrying     {"error", "after_ms"}
//! event: done         {"stop", "why", "steps", "output_tokens"}
//! event: error        {"kind", "message", "retryable"}
//! ```

use std::net::TcpStream;
use std::time::Duration;

use helmsman_agent::tools::{self, READ_ONLY};
use helmsman_agent::{Conversation, Progress, Stop};
use helmsman_diagram::explain::{Because, Blocked, Step, Test, Why};
use helmsman_diagram::{Module, Words, phrase};
use helmsman_iec::Value;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::catalog::Catalog;
use super::chat::{Chat, event};
use super::http::{self, Response};
use crate::Sample;
use helmsman_io::Dir;

/// How deep the facts go.
const DEPTH: usize = 4;
/// Largest envelope payload; deeper facts are dropped to fit.
const MAX_DATA: usize = 32 * 1024;
/// Longest variable path asked about.
pub const MAX_PATH: usize = 128;

/// What explain mode tells the model, outside the data. Fixed: no plant
/// text (names, comments, units) is ever written here.
pub const ASK: &str = "Explain mode. An operator tapped a variable on the live diagram of a module and asked why it is in its current state. The box data below holds the module and variable, the box's own step-by-step explanation from one traced scan (facts and sentences), and the source lines it cites. Answer for an operator on the floor, in a few short plain sentences: what it is, why it is in this state right now, and what would change it. Use only what the data shows (you may read signals and modules for more); where it does not say, say you cannot tell from this scan. In this mode you cannot draft, check, simulate or propose changes, and you should not suggest code edits.";

/// The request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    pub module: String,
    pub path: String,
}

/// One explanation, ready for the agent.
#[derive(Debug, Clone)]
pub struct Job {
    /// The `explain` event.
    pub head: Json,
    /// The user message: the fixed ask, then the data envelope.
    pub blocks: Vec<Json>,
}

/// Whether `p` is a variable path the explainer takes: 1..=128 of
/// `A-Z a-z 0-9 _ . [ ] ,`.
#[must_use]
pub fn path_ok(p: &str) -> bool {
    (1..=MAX_PATH).contains(&p.len())
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'[' | b']' | b','))
}

/// Parses a request.
///
/// # Errors
/// The response to send instead, and a word for the audit log.
pub fn parse(body: &[u8]) -> Result<Ask, (Response, String)> {
    let ask: Ask = serde_json::from_slice(body).map_err(|e| {
        (
            Response::json(
                400,
                &json!({ "error": format!("malformed explain request: {e}") }),
            ),
            "malformed explain request".to_string(),
        )
    })?;
    if !path_ok(&ask.path) {
        return Err((
            Response::error(422, "path: 1 to 128 of A-Z a-z 0-9 _ . [ ] ,"),
            "explain: bad path".into(),
        ));
    }
    Ok(ask)
}

/// What the box knows about its variables beside the trace: units and
/// field inputs from the signal model, kinds from the source.
#[must_use]
pub fn words(m: &Module, c: &Catalog) -> Words {
    let mut w = Words::of(m);
    for s in &c.signals {
        let key = s.name.to_ascii_uppercase();
        if let Some(u) = &s.unit {
            w.units.insert(key.clone(), u.clone());
        }
        if s.dir == Dir::In {
            w.field.insert(key);
        }
    }
    w
}

/// The explanation of `path` in `sample` of module `name` (its `source`).
///
/// # Errors
/// The source does not lower against the trace (it changed under it).
pub fn build(name: &str, source: &str, sample: &Sample, path: &str, c: &Catalog) -> Result<Job, String> {
    let m = helmsman_diagram::lower(source, sample.trace.map()).map_err(|e| e.to_string())?;
    let w = words(&m, c);
    let mut depth = DEPTH;
    loop {
        let b = helmsman_diagram::explain(&m, &sample.trace, path, depth);
        let said = phrase(&b, &w);
        let sentences: Vec<Json> = said
            .sentences
            .iter()
            .map(|s| json!({ "text": s.text, "depth": s.depth, "line": s.line }))
            .collect();
        let lines: Vec<Json> = said
            .lines()
            .into_iter()
            .map(|l| {
                let text = source
                    .lines()
                    .nth(l.saturating_sub(1) as usize)
                    .unwrap_or("")
                    .trim();
                json!({ "line": l, "text": text })
            })
            .collect();
        let fault = sample.fault.as_ref().map(|f| format!("{f:?}"));
        let mut data = json!({
            "module": name,
            "path": b.path,
            "scan": sample.scan,
            "fault": fault,
            "sentences": sentences,
            "source": lines,
        });
        if depth > 0 {
            data["facts"] = because(&b);
        }
        let size = serde_json::to_vec(&data).map_or(usize::MAX, |v| v.len());
        if size > MAX_DATA && depth > 0 {
            depth = if depth > 2 { 2 } else { 0 };
            continue;
        }
        let head = json!({
            "module": name, "path": b.path, "scan": sample.scan, "fault": fault,
            "sentences": sentences,
        });
        let blocks = vec![
            json!({ "type": "text", "text": ASK }),
            json!({ "type": "text", "text": tools::envelope("explain", &data) }),
        ];
        return Ok(Job { head, blocks });
    }
}

fn value(v: Option<Value>) -> Json {
    match v {
        Some(Value::Bool(b)) => json!(b),
        Some(Value::Int(n)) => i64::try_from(n).map_or_else(|_| json!(n.to_string()), |n| json!(n)),
        Some(Value::Real(x)) if x.is_finite() => json!(x),
        _ => Json::Null,
    }
}

/// The facts, as JSON.
#[must_use]
pub fn because(b: &Because) -> Json {
    let list = |v: &[Because]| Json::Array(v.iter().map(because).collect());
    let why = match &b.why {
        Why::Assigned {
            pou,
            line,
            text,
            gates,
            inputs,
            ..
        } => json!({
            "kind": "assigned", "pou": pou, "line": line, "text": text,
            "gates": gates.iter().map(step).collect::<Vec<_>>(), "inputs": list(inputs),
        }),
        Why::Held { pou, blocked } => json!({
            "kind": "held", "pou": pou,
            "blocked": blocked.iter().map(skipped).collect::<Vec<_>>(),
        }),
        Why::Block {
            instance,
            fb,
            call,
            inputs,
            ports,
        } => json!({
            "kind": "block", "instance": instance, "fb": fb, "line": call.map(|(_, l)| l),
            "inputs": list(inputs),
            "ports": ports.iter().map(|(p, v)| json!({ "path": p, "value": value(*v) })).collect::<Vec<_>>(),
        }),
        Why::Argument {
            instance,
            call,
            inputs,
        } => json!({
            "kind": "argument", "instance": instance, "line": call.map(|(_, l)| l),
            "inputs": list(inputs),
        }),
        Why::Input => json!({ "kind": "input" }),
        Why::Elided => json!({ "kind": "not_followed" }),
    };
    json!({ "path": b.path, "value": value(b.value), "why": why })
}

fn step(s: &Step) -> Json {
    json!({
        "line": s.line, "label": s.label, "open": s.open,
        "tests": s.tests.iter().map(test).collect::<Vec<_>>(),
    })
}

fn test(t: &Test) -> Json {
    json!({
        "text": t.text, "value": value(t.value), "needs": t.wanted,
        "inputs": t.inputs.iter().map(because).collect::<Vec<_>>(),
    })
}

fn skipped(b: &Blocked) -> Json {
    json!({ "line": b.line, "text": b.text, "gate": b.gate.as_ref().map(step) })
}

/// Runs one explanation on `stream`: the box's own answer first, then the
/// agent's, streamed. A client that goes away does not stop the turn.
pub fn serve(chat: &Chat, stream: &mut TcpStream, job: Job) {
    let Some(_slot) = chat.slot() else {
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
    send("explain", &job.head);
    let agent = chat.explain_agent();
    let mut conv = Conversation::new();
    let outcome = agent.chat_with(&mut conv, job.blocks, &READ_ONLY, &mut |p| match p {
        Progress::Text(t) => send("text", &json!({ "delta": t })),
        Progress::ToolCall { id, name, input } => {
            send("tool_call", &json!({ "id": id, "name": name, "input": input }));
        }
        Progress::ToolResult {
            id,
            name,
            ok,
            summary,
        } => send(
            "tool_result",
            &json!({ "id": id, "name": name, "ok": ok, "summary": summary }),
        ),
        // No tool in READ_ONLY stores anything.
        Progress::Proposal { .. } => {}
        Progress::Retrying { error, after } => send(
            "retrying",
            &json!({ "error": error, "after_ms": u64::try_from(after.as_millis()).unwrap_or(u64::MAX) }),
        ),
    });
    let done = |stop: &str, why: Option<&str>| json!({ "stop": stop, "why": why, "steps": outcome.steps, "output_tokens": outcome.output_tokens });
    match &outcome.stop {
        Stop::Done => send("done", &done("end_turn", None)),
        Stop::Budget(_) => send(
            "done",
            &done("budget", Some("the explanation ran out of its budget; ask again")),
        ),
        Stop::Refusal => send(
            "done",
            &done(
                "refusal",
                Some("the model declined this request; nothing was run"),
            ),
        ),
        Stop::TooLong => send(
            "error",
            &json!({ "kind": "conversation_too_long", "message": "the explanation is too long to send", "retryable": false }),
        ),
        Stop::Relay(e) => send(
            "error",
            &json!({ "kind": e.kind(), "message": e.to_string(), "retryable": e.retryable() }),
        ),
    }
}

/// What one explanation may cost: a few turns, a few reads.
#[must_use]
pub fn budget() -> helmsman_agent::Budget {
    helmsman_agent::Budget {
        steps: 6,
        output_tokens: 32_000,
        tools_per_turn: 8,
        time: Duration::from_secs(180),
    }
}
