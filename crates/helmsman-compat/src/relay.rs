//! helmsman ↔ agent-relay. A box names relay profiles by default, one
//! per surface (`--relay-agent`, `--relay-explain-agent`, ... in its
//! `--help`), and speaks the relay protocol (agent-relay
//! docs/relay-protocol.md, and v2 sessions for harness profiles). Here a
//! box at one ref talks to `agent-relay-local` built at another, which
//! talks to a stub model on loopback (the Messages API's stream, as the
//! relay's own fixtures record it):
//!
//! - every profile the box asks for by default is served
//!   (`GET /v1/agents/{agent}`), for the surfaces with no turn below
//!   (explain, tests) as much as for the others;
//! - a chat, help and (where the box has it) design turn completes: the
//!   box streams `done`, no `error` or `fallback`, and the relay asked the
//!   model at least once;
//! - on its first request of a turn the stub calls every tool the profile
//!   offers, with an empty input. The box refuses most of them as input
//!   that does not fit the schema, which is fine; a tool the box answers
//!   with `no tool named` is a tool the relay offers and the box does not
//!   have (or not in that mode), and fails;
//! - with `latest`, the newest `helmsman/vN` the relay serves is run as
//!   the chat profile too (a harness profile, relay-held sessions).
//!
//! [`live`] asks the deployed relay for the same profiles and runs one
//! chat turn through it (one short model turn is spent).

use std::ffi::OsString;
use std::fmt::Write as _;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use helmsman_agent::http::{Url, segment};
use serde_json::{Value as Json, json};

use crate::boxp::{self, BoxProc};
use crate::client;
use crate::report::Finding;
use crate::web::{Resp, Server};

/// The box token the local relay is given (`hmr1_` and 64 hex digits).
pub const TOKEN: &str = "hmr1_c0c0a7c0c0a7c0c0a7c0c0a7c0c0a7c0c0a7c0c0a7c0c0a7c0c0a7c0c0a7c0c0";
/// What the stub model answers with when it is done.
pub const STUB_TEXT: &str = "compat ok";
/// What the box answers a tool it does not have with (helmsman-agent).
pub const UNKNOWN_TOOL: &str = "no tool named";

/// The profiles a box asks for by default: (flag, id) from its `--help`.
#[must_use]
pub fn default_agents(help: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<(String, String)> = None;
    let flush = |c: Option<(String, String)>, out: &mut Vec<(String, String)>| {
        if let Some((flag, text)) = c
            && let Some(i) = text.find("default ")
        {
            let id: String = text[i + 8..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
                .collect();
            let id = id.trim_end_matches('.').to_string();
            if !id.is_empty() {
                out.push((flag, id));
            }
        }
    };
    for line in help.lines() {
        let t = line.trim_start();
        if t.starts_with("--") {
            flush(current.take(), &mut out);
            let flag = t.split_whitespace().next().unwrap_or_default();
            if flag.starts_with("--relay") && flag.ends_with("-agent") {
                current = Some((flag.to_string(), t[flag.len()..].to_string()));
            }
        } else if let Some((_, text)) = current.as_mut() {
            text.push(' ');
            text.push_str(t);
        }
    }
    flush(current, &mut out);
    out
}

/// Every request the stub model was sent.
#[derive(Default)]
pub struct StubLog {
    pub requests: Vec<Json>,
}

/// One Server-Sent Event in the Messages API's framing.
fn ev(out: &mut String, data: &Json) {
    let _ = write!(
        out,
        "event: {}\ndata: {data}\n\n",
        data["type"].as_str().unwrap_or("message")
    );
}

/// Whether the request's last message carries tool results.
fn answers_tools(body: &Json) -> bool {
    body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_array())
        .is_some_and(|c| c.iter().any(|b| b["type"] == "tool_result"))
}

/// The stub model's streamed answer to one request: every offered tool
/// called once with `{}` on a turn's first request, then text.
#[must_use]
pub fn stub_reply(body: &Json) -> String {
    let tools: Vec<&str> = body["tools"]
        .as_array()
        .map(|t| t.iter().filter_map(|t| t["name"].as_str()).collect())
        .unwrap_or_default();
    let model = body["model"].as_str().unwrap_or("stub");
    let mut s = String::new();
    ev(
        &mut s,
        &json!({ "type": "message_start", "message": { "id": "msg_compat", "type": "message", "role": "assistant",
        "content": [], "model": model, "stop_reason": null, "stop_sequence": null,
        "usage": { "input_tokens": 10, "output_tokens": 1 } } }),
    );
    let stop = if tools.is_empty() || answers_tools(body) {
        ev(
            &mut s,
            &json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
        );
        ev(
            &mut s,
            &json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": STUB_TEXT } }),
        );
        ev(&mut s, &json!({ "type": "content_block_stop", "index": 0 }));
        "end_turn"
    } else {
        for (i, name) in tools.iter().enumerate() {
            ev(
                &mut s,
                &json!({ "type": "content_block_start", "index": i, "content_block":
                { "type": "tool_use", "id": format!("toolu_compat{i:04}"), "name": name, "input": {} } }),
            );
            ev(
                &mut s,
                &json!({ "type": "content_block_delta", "index": i, "delta": { "type": "input_json_delta", "partial_json": "{}" } }),
            );
            ev(&mut s, &json!({ "type": "content_block_stop", "index": i }));
        }
        "tool_use"
    };
    ev(
        &mut s,
        &json!({ "type": "message_delta", "delta": { "stop_reason": stop, "stop_sequence": null }, "usage": { "output_tokens": 5 } }),
    );
    ev(&mut s, &json!({ "type": "message_stop" }));
    s
}

/// Starts the stub model.
///
/// # Errors
/// The listener cannot be made.
pub fn start_stub() -> std::io::Result<(Server, Arc<Mutex<StubLog>>)> {
    let log = Arc::new(Mutex::new(StubLog::default()));
    let sink = Arc::clone(&log);
    let server = Server::start(move |req| {
        if req.method == "GET" && (req.path.starts_with("/v1/models") || req.path.starts_with("/v1/model/")) {
            let id = req.path.rsplit('/').next().unwrap_or("stub");
            return Resp::json(200, &json!({ "id": id, "type": "model", "display_name": id }));
        }
        if req.method == "POST" && req.path.starts_with("/v1/messages") {
            let body: Json = serde_json::from_slice(&req.body).unwrap_or(Json::Null);
            let reply = stub_reply(&body);
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .requests
                .push(body);
            return Resp {
                status: 200,
                content_type: "text/event-stream",
                body: reply.into_bytes(),
            };
        }
        Resp::not_found()
    })?;
    Ok((server, log))
}

/// A running `agent-relay-local`.
pub struct RelayProc {
    child: Child,
    pub url: Url,
}

impl Drop for RelayProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl RelayProc {
    /// Starts `bin` against the stub at `upstream`, in an empty working
    /// directory (the relay reads `./.env`, and a real one must not
    /// reach this run), with one dev token ([`TOKEN`]).
    ///
    /// # Errors
    /// It did not start or did not answer health in 60 s.
    pub fn start(bin: &Path, upstream: &str, work: &Path) -> Result<Self, String> {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        let cwd = boxp::fresh_dir(work, "relay")?;
        let log = std::fs::File::create(cwd.join("relay.log")).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(bin);
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("RELAY_BIND", format!("127.0.0.1:{port}"))
            .env("RELAY_UPSTREAM_KIND", "anthropic")
            .env("RELAY_UPSTREAM_URL", upstream)
            .env("ANTHROPIC_API_KEY", "compat-stub-key")
            .env("RELAY_DEV_BOX_ID", "compat")
            .env("RELAY_DEV_BOX_TOKEN", TOKEN)
            .env("RUST_LOG", "warn")
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        let child = cmd.spawn().map_err(|e| format!("start {}: {e}", bin.display()))?;
        let mut me = Self {
            child,
            url: Url::parse(&format!("http://127.0.0.1:{port}"))?,
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok((200, _)) = client::get(&me.url, "/v1/health", None) {
                return Ok(me);
            }
            if let Ok(Some(st)) = me.child.try_wait() {
                let log = std::fs::read_to_string(cwd.join("relay.log")).unwrap_or_default();
                return Err(format!("the relay exited ({st}):\n{log}"));
            }
            if Instant::now() > deadline {
                return Err("the relay never answered /v1/health".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// One profile lookup's finding.
fn lookup(relay: &Url, token: &str, flag: &str, id: &str, check_prefix: &str) -> Finding {
    let check = format!("{check_prefix}profile {id}");
    match client::get(relay, &format!("/v1/agents/{}", segment(id)), Some(token)) {
        Ok((200, j)) => {
            let tools = j["tools"].as_array().map_or(0, Vec::len);
            let protocol = j["protocol"].as_u64().unwrap_or(1);
            Finding::pass(
                check,
                format!(
                    "{flag} default; served as {} (protocol {protocol}, {tools} tools)",
                    j["version"].as_str().unwrap_or("?")
                ),
            )
        }
        Ok((status, j)) => Finding::fail(
            check,
            format!(
                "the box's {flag} default is not served: {status} {}",
                j["error"]["type"].as_str().unwrap_or(&j.to_string())
            ),
        ),
        Err(e) => Finding::fail(check, e),
    }
}

/// One turn's events, and the stub's requests made during it (`None`
/// against a real model), held to the contract.
#[must_use]
pub fn judge_turn(
    check: &str,
    status: u16,
    events: &[(String, Json)],
    seen: Option<&[Json]>,
) -> Vec<Finding> {
    let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
    if status != 200 {
        let body = events.first().map(|(_, j)| j.to_string()).unwrap_or_default();
        return vec![Finding::fail(check, format!("the box answered {status}: {body}"))];
    }
    let mut out = Vec::new();
    for bad in ["error", "fallback", "stream_error"] {
        if let Some((_, d)) = events.iter().find(|(n, _)| n == bad) {
            out.push(Finding::fail(check, format!("the box streamed {bad}: {d}")));
        }
    }
    if !names.contains(&"done") {
        out.push(Finding::fail(
            check,
            format!("the stream ended without done: {names:?}"),
        ));
    }
    if let Some(seen) = seen {
        if seen.is_empty() {
            out.push(Finding::fail(check, "the relay never asked the model"));
        }
        let mut unknown: Vec<String> = Vec::new();
        for body in seen {
            for m in body["messages"].as_array().into_iter().flatten() {
                for b in m["content"].as_array().into_iter().flatten() {
                    if b["type"] != "tool_result" {
                        continue;
                    }
                    let text = match &b["content"] {
                        Json::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if text.contains(UNKNOWN_TOOL) {
                        let name = tool_name_of(seen, b["tool_use_id"].as_str().unwrap_or_default());
                        if !unknown.contains(&name) {
                            unknown.push(name);
                        }
                    }
                }
            }
        }
        if !unknown.is_empty() {
            out.push(Finding::fail(
                format!("{check} tools"),
                format!(
                    "the relay offers tools the box does not run here: {}",
                    unknown.join(", ")
                ),
            ));
        } else if let Some(first) = seen.first() {
            let n = first["tools"].as_array().map_or(0, Vec::len);
            out.push(Finding::pass(
                format!("{check} tools"),
                format!("all {n} offered tools are known to the box"),
            ));
        }
    }
    if out.iter().all(|f| f.ok) {
        out.insert(0, Finding::pass(check, format!("completed: {}", names.join(" "))));
    }
    out
}

/// The tool a `tool_use_id` named, from the stub's own replies' ids.
fn tool_name_of(seen: &[Json], id: &str) -> String {
    let i: Option<usize> = id.strip_prefix("toolu_compat").and_then(|n| n.parse().ok());
    seen.iter()
        .find_map(|b| {
            i.and_then(|i| b["tools"].get(i))
                .and_then(|t| t["name"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| id.to_string())
}

/// The box routes with a model turn, and what each is sent.
fn turns(help: &str) -> Vec<(&'static str, &'static str, Json)> {
    let mut t = vec![
        (
            "chat",
            "/api/chat",
            json!({ "message": "Compatibility check: say ok." }),
        ),
        (
            "help",
            "/api/help",
            json!({ "question": "Compatibility check: nothing is wrong." }),
        ),
    ];
    if help.contains("--relay-design-agent") {
        t.push((
            "design",
            "/api/design",
            json!({ "description": "Compatibility check: one conveyor with a start button." }),
        ));
    }
    t
}

fn run_turns(b: &BoxProc, help: &str, log: Option<&Arc<Mutex<StubLog>>>, prefix: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (name, path, body) in turns(help) {
        let before = log.map(|l| l.lock().unwrap_or_else(PoisonError::into_inner).requests.len());
        let check = format!("{prefix}turn {name}");
        match client::post_stream(&b.url, path, Some(&b.token), &body, 180) {
            Ok((status, events)) => {
                let seen: Option<Vec<Json>> = log
                    .zip(before)
                    .map(|(l, n)| l.lock().unwrap_or_else(PoisonError::into_inner).requests[n..].to_vec());
                let mut f = judge_turn(&check, status, &events, seen.as_deref());
                if f.iter().any(|f| !f.ok)
                    && let Some(last) = f.last_mut()
                {
                    let _ = write!(last.detail, "\n  box log:\n{}", b.log_tail());
                }
                out.extend(f);
            }
            Err(e) => out.push(Finding::fail(check, format!("{e}\n  box log:\n{}", b.log_tail()))),
        }
    }
    out
}

fn write_token(work: &Path, token: &str) -> Result<std::path::PathBuf, String> {
    let p = boxp::fresh_dir(work, "token")?.join("relay-token");
    std::fs::write(&p, token).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(p)
}

/// The newest `helmsman/vN` a relay serves.
fn latest_chat(relay: &Url, token: &str) -> Option<String> {
    let (_, j) = client::get(relay, "/v1/agents", Some(token)).ok()?;
    j["agents"]
        .as_array()?
        .iter()
        .filter_map(|a| a["version"].as_str())
        .filter_map(|v| v.strip_prefix("helmsman/v").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .map(|n| format!("helmsman/v{n}"))
}

/// Runs the contract: box `bin` (of checkout `helmsman`) against relay
/// `relay_bin`, both on loopback behind the stub model.
///
/// # Errors
/// Something here could not start.
pub fn check(
    bin: &Path,
    helmsman: &Path,
    relay_bin: &Path,
    work: &Path,
    latest: bool,
) -> Result<Vec<Finding>, String> {
    let help = boxp::help_text(bin)?;
    let defaults = default_agents(&help);
    if defaults.is_empty() {
        return Err("the box's --help names no default relay profiles".into());
    }
    let (stub, log) = start_stub().map_err(|e| e.to_string())?;
    let relay = RelayProc::start(relay_bin, &stub.url(""), work)?;
    let mut out = vec![Finding::pass(
        "relay health",
        "the local relay is up against the stub model",
    )];
    for (flag, id) in &defaults {
        out.push(lookup(&relay.url, TOKEN, flag, id, ""));
    }
    let token = write_token(work, TOKEN)?;
    let relay_args = |extra: &[&str]| -> Vec<OsString> {
        let mut a: Vec<OsString> = vec![
            "--relay".into(),
            relay.url.to_string().into(),
            "--relay-token".into(),
            token.clone().into(),
        ];
        a.extend(extra.iter().map(OsString::from));
        a
    };
    let b = BoxProc::start(bin, helmsman, work, &relay_args(&[]))?;
    out.extend(run_turns(&b, &help, Some(&log), ""));
    drop(b);
    if latest {
        let chat_default = defaults
            .iter()
            .find(|(f, _)| f == "--relay-agent")
            .map(|(_, id)| id.as_str());
        match latest_chat(&relay.url, TOKEN) {
            Some(newest)
                if chat_default.is_some_and(|d| newest.ends_with(&format!("/{d}")) || newest == d) => {}
            Some(newest) => {
                let b = BoxProc::start(bin, helmsman, work, &relay_args(&["--relay-agent", &newest]))?;
                let chat_only = help.replace("--relay-design-agent", "");
                let turns = run_turns(&b, &chat_only, Some(&log), &format!("{newest} "));
                out.extend(turns.into_iter().filter(|f| f.check.contains("turn chat")));
            }
            None => out.push(Finding::fail(
                "latest chat profile",
                "the relay lists no helmsman/vN",
            )),
        }
    }
    drop(relay);
    drop(stub);
    Ok(out)
}

/// The deployed relay at `url`: health, every default profile of box
/// `bin`, and one chat turn through it (one short model turn is spent).
///
/// # Errors
/// The token cannot be read or the box cannot start.
pub fn live(
    bin: &Path,
    helmsman: &Path,
    url: &str,
    token_file: &Path,
    work: &Path,
) -> Result<Vec<Finding>, String> {
    let token =
        std::fs::read_to_string(token_file).map_err(|e| format!("{}: {}", token_file.display(), e.kind()))?;
    let token = token.trim();
    let relay = Url::parse(url)?;
    let help = boxp::help_text(bin)?;
    let mut out = Vec::new();
    out.push(match client::get(&relay, "/v1/health", None) {
        Ok((200, j)) if j["ok"] == true => {
            Finding::pass("live health", format!("upstream {}", j["upstream"]))
        }
        Ok((s, j)) => Finding::fail("live health", format!("{s} {j}")),
        Err(e) => Finding::fail("live health", e),
    });
    for (flag, id) in default_agents(&help) {
        out.push(lookup(&relay, token, &flag, &id, "live "));
    }
    let args: Vec<OsString> = vec![
        "--relay".into(),
        url.into(),
        "--relay-token".into(),
        token_file.as_os_str().to_owned(),
    ];
    let b = BoxProc::start(bin, helmsman, work, &args)?;
    let body = json!({ "message": "This is an automated compatibility check. Reply with the single word ok and call no tools." });
    out.extend(
        match client::post_stream(&b.url, "/api/chat", Some(&b.token), &body, 180) {
            Ok((status, events)) => judge_turn("live turn chat", status, &events, None),
            Err(e) => vec![Finding::fail("live turn chat", e)],
        },
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP: &str = "  --relay URL          the relay
  --relay-agent V      the agent version to ask the relay for (default v7); a
                       harness profile (v8 and later, the relay says which)
  --relay-explain-agent V
                       the agent version POST /api/explain asks for (an
                       explain profile; default explain-v2). The box runs
  --relay-help-agent V the agent version POST /api/help asks for (a help
                       profile; default helmsman/help-v1). The box runs only
  --devices-index URL  the device library's index (default the public one)
";

    #[test]
    fn defaults_come_from_each_relay_agent_flag_only() {
        assert_eq!(
            default_agents(HELP),
            [
                ("--relay-agent".to_string(), "v7".to_string()),
                ("--relay-explain-agent".into(), "explain-v2".into()),
                ("--relay-help-agent".into(), "helmsman/help-v1".into()),
            ]
        );
    }

    #[test]
    fn the_stub_calls_every_tool_then_answers_text() {
        let first = json!({ "model": "m", "tools": [ { "name": "a" }, { "name": "b" } ],
            "messages": [ { "role": "user", "content": "hi" } ] });
        let s = stub_reply(&first);
        assert!(
            s.contains(r#""name":"a""#)
                && s.contains(r#""name":"b""#)
                && s.contains(r#""stop_reason":"tool_use""#),
            "{s}"
        );
        let second = json!({ "model": "m", "tools": [ { "name": "a" } ], "messages": [
            { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": "toolu_compat0000", "content": "x" } ] } ] });
        let s = stub_reply(&second);
        assert!(
            s.contains(STUB_TEXT) && s.contains("end_turn") && !s.contains("tool_use\""),
            "{s}"
        );
        assert!(stub_reply(&json!({ "model": "m", "messages": [] })).contains("end_turn"));
    }

    fn ev(name: &str) -> (String, Json) {
        (name.to_string(), json!({}))
    }

    fn seen_with_result(text: &str) -> Vec<Json> {
        vec![
            json!({ "tools": [ { "name": "read_signals" }, { "name": "make_coffee" } ], "messages": [] }),
            json!({ "tools": [ { "name": "read_signals" }, { "name": "make_coffee" } ], "messages": [ { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_compat0000", "content": "input does not fit" },
                { "type": "tool_result", "tool_use_id": "toolu_compat0001", "content": text } ] } ] }),
        ]
    }

    #[test]
    fn a_turn_that_completes_with_known_tools_passes() {
        let f = judge_turn(
            "turn chat",
            200,
            &[ev("session"), ev("text"), ev("done")],
            Some(&seen_with_result("ok")),
        );
        assert!(f.iter().all(|f| f.ok), "{f:?}");
    }

    #[test]
    fn a_tool_the_box_does_not_have_is_named() {
        let seen = seen_with_result("no tool named \"make_coffee\" on this box; the tools are ...");
        let f = judge_turn("turn chat", 200, &[ev("done")], Some(&seen));
        let bad = f.iter().find(|f| !f.ok).unwrap();
        assert_eq!(bad.check, "turn chat tools");
        assert!(bad.detail.ends_with("make_coffee"), "{bad:?}");
    }

    #[test]
    fn error_fallback_no_done_and_no_model_call_each_fail() {
        let f = judge_turn("t", 200, &[ev("error")], Some(&[]));
        let details: Vec<&str> = f.iter().filter(|f| !f.ok).map(|f| f.detail.as_str()).collect();
        assert_eq!(details.len(), 3, "{f:?}");
        assert!(
            judge_turn("t", 200, &[ev("fallback"), ev("done")], None)
                .iter()
                .any(|f| !f.ok)
        );
        let refused = judge_turn("t", 503, &[("body".into(), json!({ "error": "offline" }))], None);
        assert!(!refused[0].ok && refused[0].detail.contains("503"), "{refused:?}");
    }
}
