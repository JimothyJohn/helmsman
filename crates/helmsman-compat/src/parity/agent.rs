//! The agent routes: explain, requirements, chat, design and help, each a turn through the relay (the stub model on the candidate, one short real turn on live; docs/PARITY.md).
//!
//! A run says which agent side it has ([`AgentSide`]): none (these
//! journeys are not run, which reports like pending), the stub (an
//! `agent-relay-local` in front of a stub model, the candidate), or live
//! (the deployed relay, one short real model turn per route). A turn is
//! judged by its event shape, as `relay::judge_turn` does, never by the
//! model's words.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value as Json, json};

use super::{Ctx, Journey, ensure_running, expect, wait_for};
use crate::client;
use crate::relay::{self, RelayProc, StubLog};
use crate::web::Server;

/// This area's journeys; `None` is pending.
pub const JOURNEYS: &[(&str, Option<Journey>)] = &[
    ("explain", Some(explain)),
    ("requirements", Some(requirements)),
    ("chat", Some(chat)),
    ("design", Some(design)),
    ("help", Some(help)),
];

/// Whether journey `id` is one that needs a relay.
#[must_use]
pub fn needs_relay(id: &str) -> bool {
    JOURNEYS.iter().any(|(j, _)| *j == id)
}

/// Which agent side a run has.
#[derive(Debug, Clone)]
pub enum AgentSide {
    /// No relay: the agent journeys are not run.
    None,
    /// `agent-relay-local` at `relay_bin` in front of the stub model.
    Stub { relay_bin: PathBuf },
    /// The deployed relay at `url`, its box token in `token`.
    Live { url: String, token: PathBuf },
}

/// What stands behind the box's agent routes, as a journey sees it.
#[derive(Clone)]
pub enum Agent {
    None,
    /// The stub model's log: what the relay asked it.
    Stub(Arc<Mutex<StubLog>>),
    Live,
}

impl Agent {
    /// How many model requests the stub has seen so far.
    fn mark(&self) -> usize {
        match self {
            Self::Stub(l) => l.lock().unwrap_or_else(PoisonError::into_inner).requests.len(),
            _ => 0,
        }
    }

    /// The model requests since `mark` (`None` against a real model).
    fn since(&self, mark: usize) -> Option<Vec<Json>> {
        match self {
            Self::Stub(l) => Some(l.lock().unwrap_or_else(PoisonError::into_inner).requests[mark..].to_vec()),
            _ => None,
        }
    }
}

/// The relay (and stub) of a run, alive while it is held.
pub struct Behind {
    /// The stub model and the relay in front of it: held, never read, so
    /// they stop when the run's `Behind` drops.
    _held: Option<(Server, RelayProc)>,
    log: Option<Arc<Mutex<StubLog>>>,
    flags: Vec<OsString>,
    live: bool,
}

impl Behind {
    /// The box's arguments that point it at the relay.
    #[must_use]
    pub fn args(&self) -> Vec<OsString> {
        self.flags.clone()
    }

    /// The journeys' view of it.
    #[must_use]
    pub fn agent(&self) -> Agent {
        match (&self.log, self.live) {
            (Some(l), _) => Agent::Stub(Arc::clone(l)),
            (None, true) => Agent::Live,
            (None, false) => Agent::None,
        }
    }
}

/// Starts what `side` needs.
///
/// # Errors
/// The stub, the relay or the token file could not be made.
pub fn start(side: &AgentSide, work: &Path) -> Result<Behind, String> {
    let mut behind = Behind {
        _held: None,
        log: None,
        flags: Vec::new(),
        live: false,
    };
    match side {
        AgentSide::None => {}
        AgentSide::Stub { relay_bin } => {
            let (stub, log) = relay::start_stub().map_err(|e| e.to_string())?;
            let proc = RelayProc::start(relay_bin, &stub.url(""), work)?;
            let token = relay::write_token(work, relay::TOKEN)?;
            behind.flags = vec![
                "--relay".into(),
                proc.url.to_string().into(),
                "--relay-token".into(),
                token.into(),
            ];
            behind.log = Some(log);
            behind = Behind {
                _held: Some((stub, proc)),
                ..behind
            };
        }
        AgentSide::Live { url, token } => {
            behind.flags = vec![
                "--relay".into(),
                url.into(),
                "--relay-token".into(),
                token.as_os_str().to_owned(),
            ];
            behind.live = true;
        }
    }
    Ok(behind)
}

// ---- journeys --------------------------------------------------------------

/// One streamed turn: its events, held to the relay contract's shape
/// (done, no error or fallback, and on the stub every offered tool known
/// to the box).
fn turn(c: &Ctx, path: &str, body: &Json) -> Result<Vec<(String, Json)>, String> {
    let mark = c.agent.mark();
    let (status, events) = client::post_stream(&c.url, path, Some(&c.session), body, 180)?;
    let seen = c.agent.since(mark);
    let bad: Vec<String> = relay::judge_turn(path, status, &events, seen.as_deref())
        .into_iter()
        .filter(|f| !f.ok)
        .map(|f| format!("{}: {}", f.check, f.detail))
        .collect();
    if bad.is_empty() {
        Ok(events)
    } else {
        let last = events.last().map(|(n, d)| format!("{n} {d}")).unwrap_or_default();
        Err(format!(
            "{}\n  events: {}; last: {last}",
            bad.join("\n"),
            names(&events)
        ))
    }
}

fn names(events: &[(String, Json)]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for (n, _) in events {
        if seen.last() != Some(&n.as_str()) {
            seen.push(n);
        }
    }
    seen.join(" ")
}

fn event<'a>(events: &'a [(String, Json)], name: &str) -> Result<&'a Json, String> {
    events
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d)
        .ok_or_else(|| format!("no {name} event in: {}", names(events)))
}

fn chat(c: &Ctx) -> Result<String, String> {
    let events = turn(
        c,
        "/api/chat",
        &json!({ "message": "This is an automated parity check. Reply with the single word ok and call no tools." }),
    )?;
    wait_for("chat status online", || {
        let s = expect(c, "GET", "/api/chat/status", None, 200)?;
        Ok(s["state"] == "online")
    })?;
    let note = "Parity check: the lamp should come on when start is pressed.";
    let (st, posted) = client::post_raw(
        &c.url,
        "/api/dropins?name=parity-note.txt&kind=prose",
        Some(&c.session),
        "text/plain",
        note.as_bytes(),
    )?;
    if st != 201 {
        return Err(format!("POST /api/dropins: {st} {posted}"));
    }
    let id = posted["id"].as_str().map(str::to_string);
    let list = expect(c, "GET", "/api/dropins", None, 200)?;
    let listed = list
        .as_array()
        .or_else(|| list["dropins"].as_array())
        .is_some_and(|l| {
            l.iter()
                .any(|d| d["name"] == "parity-note.txt" && d["id"].as_str() == id.as_deref())
        });
    if !listed {
        return Err(format!(
            "GET /api/dropins does not list the posted note ({posted}): {list}"
        ));
    }
    Ok(format!(
        "chat turn: {}; status online; drop-in posted and listed",
        names(&events)
    ))
}

fn explain(c: &Ctx) -> Result<String, String> {
    let module = ensure_running(c)?;
    let trace = format!("/api/modules/{module}/trace");
    wait_for("a traced scan", || {
        let (st, j) = client::get(&c.url, &trace, Some(&c.session))?;
        match st {
            200 => Ok(true),
            202 => Ok(false),
            _ => Err(format!("GET {trace}: {st} {j}")),
        }
    })?;
    let events = turn(c, "/api/explain", &json!({ "module": module, "path": "lamp" }))?;
    let head = event(&events, "explain")?;
    if events.first().map(|(n, _)| n.as_str()) != Some("explain") || !head["sentences"].is_array() {
        return Err(format!(
            "the box's own explanation does not come first: {}",
            names(&events)
        ));
    }
    if !events.iter().any(|(n, _)| n == "text") {
        return Err(format!(
            "the agent said nothing after the box: {}",
            names(&events)
        ));
    }
    Ok(format!("{module}: traced scan, then {}", names(&events)))
}

fn requirements(c: &Ctx) -> Result<String, String> {
    let p = "parity-req";
    // The program only: the requirement test runs against it, and the
    // module already running stays as it is.
    expect(c, "POST", "/api/projects", Some(&json!({ "name": p })), 201)?;
    expect(
        c,
        "PUT",
        &format!("/api/projects/{p}/files/main.st"),
        Some(&json!({ "text": super::COUNTER })),
        200,
    )?;
    expect(
        c,
        "PUT",
        &format!("/api/projects/{p}/files/io.map"),
        Some(&json!({ "text": super::MAP })),
        200,
    )?;
    let events = turn(
        c,
        &format!("/api/projects/{p}/requirements"),
        &json!({ "text": "The lamp is on a few scans after start is on.", "name": "parity_lamp" }),
    )?;
    let test = event(&events, "test")?;
    let state = test["state"].as_str().unwrap_or_default();
    if !matches!(state, "passed" | "failed") {
        return Err(format!("the agent wrote no test that compiled and ran: {test}"));
    }
    let file = test["file"].as_str().unwrap_or_default().to_string();
    let back = expect(c, "GET", &format!("/api/projects/{p}/files/{file}"), None, 200)?;
    if !back.to_string().contains("ASSERT") {
        return Err(format!("{file} is saved without an ASSERT: {back}"));
    }
    let run = expect(
        c,
        "POST",
        &format!("/api/projects/{p}/tests"),
        Some(&json!({})),
        200,
    )?;
    if !run.to_string().contains("parity_lamp") {
        return Err(format!(
            "POST /api/projects/{p}/tests does not run the new test: {run}"
        ));
    }
    Ok(format!("{file} written and run: {state} ({})", names(&events)))
}

fn design(c: &Ctx) -> Result<String, String> {
    let events = turn(
        c,
        "/api/design",
        &json!({ "description": "Automated parity check: one lamp that comes on with a start button. Answer in two short lines." }),
    )?;
    event(&events, "session")?;
    Ok(format!("design turn: {}", names(&events)))
}

fn help(c: &Ctx) -> Result<String, String> {
    let events = turn(
        c,
        "/api/help",
        &json!({ "question": "Automated parity check: nothing is wrong. How do I load a program?" }),
    )?;
    event(&events, "help")?;
    Ok(format!("help turn: {}", names(&events)))
}
