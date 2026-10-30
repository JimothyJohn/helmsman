//! The box's client for the relay (protocol v1, `docs/relay-protocol.md`
//! in the relay repo): `POST /v1/agents/{version}/messages` with the
//! conversation, a per-box bearer token, and the model's stream back as
//! server-sent events.
//!
//! The box never names a model, a system prompt, tools or `max_tokens`:
//! the relay owns them per agent version. The box never holds an
//! Anthropic key either; the token here is the relay's per-box token.

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
    pub timeouts: Timeouts,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("version", &self.version)
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

/// Whether `v` is a plausible agent version (`v1`, `v2`, ...).
#[must_use]
pub fn version_ok(v: &str) -> bool {
    (1..=32).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
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

fn from_http(e: http::Error) -> Error {
    match e {
        http::Error::Unreachable(why) => Error::Unreachable(why),
        http::Error::Timeout => Error::Timeout,
        http::Error::Tls(why) => Error::Unreachable(format!("TLS: {why}")),
        http::Error::NoTls => Error::Unreachable("https:// needs a build with the https feature".into()),
        http::Error::Protocol(why) => Error::Malformed(why.into()),
        http::Error::Io(k) => Error::Unreachable(format!("connection failed: {k}")),
    }
}

/// Longest a [`probe`] may take, whatever the turn timeouts are.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether the relay answers: `GET {prefix}/v1/health` with the box
/// token. Any HTTP answer below 500 other than 401/403 counts (a relay
/// without the route answers 404, which still means it is there); the
/// body is never read. Never sends a conversation and costs no tokens.
///
/// # Errors
/// [`Error::Unreachable`] or [`Error::Timeout`] when nothing answers;
/// [`Error::Status`] for a 5xx, 401 or 403.
pub fn probe(cfg: &Config) -> Result<(), Error> {
    let path = format!("{}/v1/health", cfg.url.prefix);
    let auth = format!("Bearer {}", cfg.token);
    let req = http::Request {
        method: "GET",
        path: &path,
        headers: &[("Authorization", &auth)],
        json: None,
    };
    let t = Timeouts {
        connect: cfg.timeouts.connect.min(PROBE_TIMEOUT),
        idle: cfg.timeouts.idle.min(PROBE_TIMEOUT),
        total: cfg.timeouts.total.min(PROBE_TIMEOUT),
    };
    let resp = http::send(&cfg.url, &req, t).map_err(from_http)?;
    let status = resp.status;
    if status >= 500 || status == 401 || status == 403 {
        return Err(Error::Status {
            status,
            kind: if status >= 500 {
                "relay_unavailable"
            } else {
                "relay_refused_token"
            }
            .into(),
            message: format!("the relay answered {status} to a health check"),
            retryable: status >= 500,
        });
    }
    Ok(())
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
    };
    let resp = http::send(&cfg.url, &req, cfg.timeouts).map_err(from_http)?;
    if resp.status != 200 {
        let status = resp.status;
        let body = resp.read_body(MAX_ERROR_BODY).unwrap_or_default();
        let j: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
        let e = &j["error"];
        return Err(Error::Status {
            status,
            kind: e["type"].as_str().unwrap_or("relay_error").to_string(),
            message: e["message"]
                .as_str()
                .map_or_else(|| format!("status {status}"), str::to_string),
            retryable: e["retryable"].as_bool().unwrap_or(status >= 500),
        });
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
