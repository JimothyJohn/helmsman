//! The box's client for the relay (protocol v1, `docs/relay-protocol.md`
//! in the relay repo): `POST /v1/agents/{version}/messages` with the
//! conversation, a per-box bearer token, and the model's stream back as
//! server-sent events.
//!
//! The box never names a model, a system prompt, tools or `max_tokens`:
//! the relay owns them per agent version. The box never holds an
//! model API key either; the token here is the relay's per-box token.

use std::fmt;
use std::time::Duration;

use serde_json::{Value as Json, json};

pub use crate::http::Url;
use crate::http::{self, Timeouts};
use crate::sse;
use crate::turn::{Builder, Delta, Step, StreamError, Turn};

/// Largest error body read from the relay.
const MAX_ERROR_BODY: usize = 64 * 1024;

/// How to reach the relay.
#[derive(Clone)]
pub struct Config {
    pub url: Url,
    /// `hmr1_` and 64 hex digits; sent only as `Authorization: Bearer`.
    pub token: String,
    /// The agent version asked for (`v1`).
    pub version: String,
    /// The relay protocol the version speaks: 1 (the box keeps the
    /// conversation) or 2 (the relay keeps it, [`crate::session`]). Set
    /// from [`lookup`]; a config starts at 1.
    pub protocol: u8,
    pub timeouts: Timeouts,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("version", &self.version)
            .field("protocol", &self.protocol)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Defaults: 10 s to connect, 120 s without a byte (the model API
    /// pings well within that), 15 min for one turn.
    #[must_use]
    pub fn new(url: Url, token: String, version: String) -> Self {
        Self {
            url,
            token,
            version,
            protocol: 1,
            timeouts: Timeouts {
                connect: Duration::from_secs(10),
                idle: Duration::from_secs(120),
                total: Duration::from_secs(900),
            },
        }
    }
}

/// Whether `t` is a relay box token: `hmr1_` and 64 lowercase hex digits.
#[must_use]
pub fn token_ok(t: &str) -> bool {
    t.strip_prefix("hmr1_").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Whether `v` is a plausible agent version: an alias (`v7`,
/// `explain-v2`) or a profile id, `<namespace>/<name>`
/// (`helmsman/help-v1`), each part 1 to 32 of `A-Z a-z 0-9 - _ .`.
#[must_use]
pub fn version_ok(v: &str) -> bool {
    let part = |p: &str| {
        (1..=32).contains(&p.len())
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    };
    match v.split_once('/') {
        Some((ns, name)) => part(ns) && part(name),
        None => part(v),
    }
}

/// Why a turn did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The relay could not be reached at all.
    Unreachable(String),
    /// No byte for the idle timeout, or the turn ran past its deadline.
    Timeout,
    /// A plain JSON error before the stream started.
    Status {
        status: u16,
        kind: String,
        message: String,
        retryable: bool,
    },
    /// `relay_error` (or the model API's `error`) mid-stream.
    Stream(StreamError),
    /// The stream ended without `relay_usage` or `relay_error`.
    CutOff,
    /// The stream or the response broke the protocol.
    Malformed(String),
}

impl Error {
    /// Whether the protocol says to retry it (with backoff).
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::Unreachable(_) | Self::Timeout | Self::CutOff => true,
            Self::Status { retryable, .. } => *retryable,
            Self::Stream(e) => e.retryable,
            Self::Malformed(_) => false,
        }
    }

    /// Whether it means the agent is offline for this box (the relay
    /// unreachable, silent, failing, or refusing the box token), as
    /// opposed to one turn going wrong.
    #[must_use]
    pub fn offline(&self) -> bool {
        match self {
            Self::Unreachable(_) | Self::Timeout => true,
            Self::Status { status, .. } => *status >= 500 || *status == 401 || *status == 403,
            Self::Stream(_) | Self::CutOff | Self::Malformed(_) => false,
        }
    }

    /// A short machine word for the chat panel.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::Unreachable(_) => "relay_unreachable",
            Self::Timeout => "relay_timeout",
            Self::Status { kind, .. } => kind,
            Self::Stream(e) => &e.kind,
            Self::CutOff => "relay_cut_off",
            Self::Malformed(_) => "relay_malformed",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(why) => write!(
                f,
                "the relay cannot be reached ({why}); the agent is offline, installed modules keep running"
            ),
            Self::Timeout => f.write_str("the relay stopped answering (timed out)"),
            Self::Status { status, message, .. } => {
                write!(f, "the relay refused the turn ({status}): {message}")
            }
            Self::Stream(e) => write!(f, "the turn failed mid-stream ({}): {}", e.kind, e.message),
            Self::CutOff => f.write_str("the relay's stream ended early"),
            Self::Malformed(why) => write!(f, "the relay's stream is malformed: {why}"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn from_http(e: http::Error) -> Error {
    match e {
        http::Error::Unreachable(why) => Error::Unreachable(why),
        http::Error::Timeout => Error::Timeout,
        http::Error::Tls(why) => Error::Unreachable(format!("TLS: {why}")),
        http::Error::NoTls => Error::Unreachable("https:// needs a build with the https feature".into()),
        http::Error::Protocol(why) => Error::Malformed(why.into()),
        http::Error::Io(k) => Error::Unreachable(format!("connection failed: {k}")),
    }
}

/// What the relay says an agent profile is: its id and the protocol it
/// speaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// The profile id (`helmsman/v8`), whichever name was asked for.
    pub id: String,
    /// 1 or 2 (`GET /v1/agents/{agent}` carries `"protocol"`; an older
    /// relay says nothing, and that is 1).
    pub protocol: u8,
}

/// The one rule for which protocol an agent uses: ask the relay.
/// `GET /v1/agents/{version}` names the profile's `protocol` (relay
/// protocol v2, "Everything in protocol v1 still holds"). A bare name the
/// relay does not know (`v8`: the aliases stop at `v7`) is asked again as
/// `helmsman/{name}`, this box's own namespace. Nothing here is a list of
/// versions.
///
/// # Errors
/// [`Error`] as for a turn: unreachable, a status, or an answer that is
/// not a profile.
pub fn lookup(cfg: &Config) -> Result<Profile, Error> {
    let ask = |name: &str| -> Result<Profile, Error> {
        let path = format!("{}/v1/agents/{}", cfg.url.prefix, http::segment(name));
        let auth = format!("Bearer {}", cfg.token);
        let req = http::Request {
            method: "GET",
            path: &path,
            headers: &[("Authorization", &auth)],
            json: None,
            raw: None,
        };
        let t = Timeouts {
            connect: cfg.timeouts.connect.min(PROBE_TIMEOUT),
            idle: cfg.timeouts.idle.min(PROBE_TIMEOUT),
            total: cfg.timeouts.total.min(PROBE_TIMEOUT),
        };
        let resp = http::send(&cfg.url, &req, t).map_err(from_http)?;
        if resp.status != 200 {
            return Err(status_error(resp));
        }
        let body = resp.read_body(MAX_PROFILE_BODY).map_err(from_http)?;
        let j: Json = serde_json::from_slice(&body)
            .map_err(|_| Error::Malformed("the agent profile is not JSON".into()))?;
        let id = j["version"].as_str().filter(|v| version_ok(v));
        let protocol = match &j["protocol"] {
            Json::Null => 1,
            Json::Number(n) if n.as_u64() == Some(1) => 1,
            Json::Number(n) if n.as_u64() == Some(2) => 2,
            _ => {
                return Err(Error::Malformed(
                    "the agent profile names a protocol this box does not speak".into(),
                ));
            }
        };
        Ok(Profile {
            id: id.unwrap_or(name).to_string(),
            protocol,
        })
    };
    match ask(&cfg.version) {
        Err(Error::Status {
            status: 404,
            ref kind,
            ..
        }) if kind == "unknown_agent_version" && !cfg.version.contains('/') => {
            ask(&format!("helmsman/{}", cfg.version)).map_err(|e| match e {
                Error::Status { status: 404, .. } => Error::Status {
                    status: 404,
                    kind: "unknown_agent_version".into(),
                    message: format!("the relay serves no agent {:?}", cfg.version),
                    retryable: false,
                },
                e => e,
            })
        }
        r => r,
    }
}

/// Largest agent profile body read (the profile carries its tool schemas).
const MAX_PROFILE_BODY: usize = 1 << 20;

/// A non-200 answer as an [`Error::Status`].
pub(crate) fn status_error(resp: http::Response) -> Error {
    let status = resp.status;
    let body = resp.read_body(MAX_ERROR_BODY).unwrap_or_default();
    let j: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
    let e = &j["error"];
    Error::Status {
        status,
        kind: e["type"].as_str().unwrap_or("relay_error").to_string(),
        message: e["message"]
            .as_str()
            .map_or_else(|| format!("status {status}"), str::to_string),
        retryable: e["retryable"].as_bool().unwrap_or(status >= 500),
    }
}

/// Longest a [`probe`] may take, whatever the turn timeouts are.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether the relay can serve turns: `GET {prefix}/v1/health` with the
/// box token. Only a 2xx counts (relay protocol v1: 200 when the relay
/// reaches the model API with its key, 503 when it cannot); a relay
/// without the route (404) or any other answer is not up. Never sends a
/// conversation and costs no tokens.
///
/// # Errors
/// [`Error::Unreachable`] or [`Error::Timeout`] when nothing answers;
/// [`Error::Status`] for anything but a 2xx: a 5xx is the relay's own
/// error kind and message (`upstream_unavailable`, naming the `upstream`
/// category) when its body has them, else `relay_unavailable`; 401 and
/// 403 are `relay_refused_token`; the rest `relay_unhealthy`.
pub fn probe(cfg: &Config) -> Result<(), Error> {
    let path = format!("{}/v1/health", cfg.url.prefix);
    let auth = format!("Bearer {}", cfg.token);
    let req = http::Request {
        method: "GET",
        path: &path,
        headers: &[("Authorization", &auth)],
        json: None,
        raw: None,
    };
    let t = Timeouts {
        connect: cfg.timeouts.connect.min(PROBE_TIMEOUT),
        idle: cfg.timeouts.idle.min(PROBE_TIMEOUT),
        total: cfg.timeouts.total.min(PROBE_TIMEOUT),
    };
    let resp = http::send(&cfg.url, &req, t).map_err(from_http)?;
    let status = resp.status;
    if (200..300).contains(&status) {
        return Ok(());
    }
    let said = format!("the relay answered {status} to a health check");
    if status >= 500 {
        let body = resp.read_body(MAX_ERROR_BODY).unwrap_or_default();
        let j: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
        let e = &j["error"];
        let (kind, message) = match (e["type"].as_str(), e["message"].as_str()) {
            (Some(kind), Some(message)) => {
                let upstream = j["upstream"]
                    .as_str()
                    .map(|u| format!(" (upstream: {u})"))
                    .unwrap_or_default();
                (kind.to_string(), format!("{said}: {message}{upstream}"))
            }
            _ => ("relay_unavailable".to_string(), said),
        };
        return Err(Error::Status {
            status,
            kind,
            message,
            retryable: true,
        });
    }
    let (kind, message) = if status == 401 || status == 403 {
        ("relay_refused_token", said)
    } else {
        (
            "relay_unhealthy",
            format!(
                "{said}; only a 2xx means it can serve turns (a relay without GET /v1/health is too old)"
            ),
        )
    };
    Err(Error::Status {
        status,
        kind: kind.into(),
        message,
        retryable: false,
    })
}

/// Whether `c` is a pairing code as the relay prints it: 10 symbols of
/// Crockford's base32, as typed (case, one dash or blanks between the two
/// groups of five are the relay's to read). Only a shape check, so a
/// mistyped file is caught before the relay counts a try against the box.
#[must_use]
pub fn pair_code_ok(c: &str) -> bool {
    let symbols = c
        .chars()
        .filter(|ch| *ch != '-' && !ch.is_whitespace())
        .collect::<Vec<_>>();
    symbols.len() == 10
        && symbols
            .iter()
            .all(|ch| ch.is_ascii_alphanumeric() && !matches!(ch.to_ascii_uppercase(), 'U'))
}

/// Whether `id` is a box id the relay takes: 1 to 64 of `a-z 0-9 -`, not
/// starting with `-`.
#[must_use]
pub fn box_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Trades a one-time pairing code for this box's relay token
/// (`POST {prefix}/v1/pair`, relay protocol "Pairing by code"): no bearer
/// token, the code is the credential. The token comes back once and is
/// checked to be a box token for `box_id` before it is returned.
///
/// The code and the token are secrets: neither appears in an [`Error`]
/// (the relay's own refusal text is fixed and names neither).
///
/// # Errors
/// [`Error::Unreachable`] before anything was sent (safe to retry);
/// [`Error::Timeout`] or [`Error::Malformed`] after it was (the code may
/// be spent; ask for a new one); [`Error::Status`] for the relay's refusal
/// (403: the code is wrong, used, expired or for another id; 429: slow
/// down; 404: this relay has no pairing).
pub fn pair(url: &Url, code: &str, box_id: &str, timeouts: Timeouts) -> Result<String, Error> {
    let body = serde_json::to_vec(&json!({ "code": code, "box_id": box_id })).unwrap_or_default();
    let path = format!("{}/v1/pair", url.prefix);
    let req = http::Request {
        method: "POST",
        path: &path,
        headers: &[],
        json: Some(&body),
        raw: None,
    };
    let resp = http::send(url, &req, timeouts).map_err(from_http)?;
    if resp.status != 200 {
        return Err(status_error(resp));
    }
    let bytes = resp
        .read_body(MAX_ERROR_BODY)
        .map_err(|_| Error::Malformed("the pairing answer could not be read".into()))?;
    let j: Json = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Malformed("the pairing answer is not JSON".into()))?;
    match (j["token"].as_str(), j["box_id"].as_str()) {
        (Some(t), Some(id)) if token_ok(t) && id == box_id => Ok(t.to_string()),
        _ => Err(Error::Malformed(
            "the pairing answer is not a box token for this box".into(),
        )),
    }
}

/// Runs one model turn over `messages`, calling `on` as text and tool
/// calls stream in.
///
/// # Errors
/// [`Error`]; a failed turn's partial content is discarded.
pub fn turn(cfg: &Config, messages: &[Json], on: &mut dyn FnMut(Delta<'_>)) -> Result<Turn, Error> {
    let body = serde_json::to_vec(&json!({ "messages": messages })).unwrap_or_default();
    let path = format!(
        "{}/v1/agents/{}/messages",
        cfg.url.prefix,
        http::segment(&cfg.version)
    );
    let auth = format!("Bearer {}", cfg.token);
    let req = http::Request {
        method: "POST",
        path: &path,
        headers: &[("Authorization", &auth), ("Accept", "text/event-stream")],
        json: Some(&body),
        raw: None,
    };
    let resp = http::send(&cfg.url, &req, cfg.timeouts).map_err(from_http)?;
    if resp.status != 200 {
        return Err(status_error(resp));
    }
    if !resp
        .header("content-type")
        .is_some_and(|t| t.starts_with("text/event-stream"))
    {
        return Err(Error::Malformed("the turn is not an event stream".into()));
    }
    let mut body = resp.body;
    let mut reader = sse::Reader::new();
    let mut builder = Builder::new();
    loop {
        let ev = match reader.next(&mut body) {
            Ok(Some(ev)) => ev,
            Ok(None) => return Err(Error::CutOff),
            Err(sse::Error::Read(e)) => return Err(from_http(e)),
            Err(e) => return Err(Error::Malformed(e.to_string())),
        };
        match builder.feed(&ev.name, &ev.data, on) {
            Ok(Step::Continue) => {}
            Ok(Step::Done(t)) => return Ok(t),
            Ok(Step::Failed(e)) => return Err(Error::Stream(e)),
            Err(m) => return Err(Error::Malformed(m.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::version_ok;

    /// Relay protocol v1 "Naming an agent profile": an alias or a
    /// `<namespace>/<name>` id.
    #[test]
    fn a_version_is_an_alias_or_a_profile_id() {
        for ok in ["v7", "explain-v2", "tests-v1", "helmsman/help-v1", "helmsman/v7"] {
            assert!(version_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "/",
            "helmsman/",
            "/v1",
            "a/b/c",
            "a b",
            "v1?x",
            &"x".repeat(33),
        ] {
            assert!(!version_ok(bad), "{bad}");
        }
    }
}
