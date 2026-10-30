//! The sinks file, one declaration per line:
//!
//! ```text
//! sink   NAME URL [timeout_ms=N] [queue_bytes=N] [retry_min_ms=N] [retry_max_ms=N] [allow_local=true]
//! header SINK NAME env:VAR | file:PATH | text:VALUE
//! emit   SINK SIGNAL change | rise | fall | every_ms=N
//! ```
//!
//! - `sink`: NAME is an identifier (it names the queue file). URL is
//!   checked by [`Url::parse`]. Defaults: `timeout_ms=5000` (1..=60000),
//!   `queue_bytes=1048576` (4096..=67108864), `retry_min_ms=1000`
//!   (1..=600000), `retry_max_ms=300000` (`retry_min_ms`..=3600000).
//! - `header`: a value from an environment variable, a file (trimmed of
//!   trailing whitespace, at most 8 KiB) or the file itself. `Authorization`,
//!   `Proxy-Authorization`, `Cookie` and any name containing `token`, `key`,
//!   `secret`, `auth` or `password` must come from `env:` or `file:`, so
//!   a credential never sits in a config file that gets copied around.
//!   Header values never appear in a log line or an error message. The
//!   headers the client writes itself (`Host`, `Content-Length`,
//!   `Content-Type`, `Connection`, `Idempotency-Key`, ...) cannot be set.
//! - `emit`: publishes SIGNAL (any signal of the I/O map or the calendar,
//!   by name in any case) to SINK; see [`super`] for the triggers.
//!
//! `#` starts a comment (so no value from `text:` contains one); tokens are
//! separated by spaces or tabs.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::emit::Trigger;
use super::secret::Secret;
use super::url::{Scheme, Url};
use super::worker::{Backoff, SinkConfig};

/// Where a header value comes from.
#[derive(Clone, PartialEq, Eq)]
pub enum HeaderSource {
    Env(String),
    File(PathBuf),
    Text(Secret),
}

impl fmt::Debug for HeaderSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(v) => write!(f, "env:{v}"),
            Self::File(p) => write!(f, "file:{}", p.display()),
            Self::Text(_) => f.write_str("text:<redacted>"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkDecl {
    pub line: usize,
    pub name: String,
    pub url: Url,
    pub timeout: Duration,
    pub queue_bytes: usize,
    pub backoff: Backoff,
    pub allow_local: bool,
    pub headers: Vec<(String, HeaderSource)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitDecl {
    pub line: usize,
    pub sink: String,
    pub signal: String,
    pub trigger: Trigger,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SinksFile {
    pub sinks: Vec<SinkDecl>,
    pub emits: Vec<EmitDecl>,
}

/// Most bytes of a header value read from a file.
pub const MAX_HEADER_FILE: u64 = 8 * 1024;

const RESERVED: &[&str] = &[
    "host",
    "content-length",
    "content-type",
    "connection",
    "transfer-encoding",
    "idempotency-key",
    "user-agent",
    "expect",
    "upgrade",
    "te",
    "trailer",
    "keep-alive",
];

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An RFC 9110 token.
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Whether a header carries a credential and so must not be `text:`.
#[must_use]
pub fn sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "authorization" | "proxy-authorization" | "cookie")
        || ["token", "key", "secret", "auth", "password"]
            .iter()
            .any(|w| n.contains(w))
}

/// A header value as the wire allows it: visible ASCII, spaces and tabs,
/// no CR or LF (which would let a value inject headers).
fn value_ok(v: &str) -> bool {
    v.bytes().all(|b| b == b' ' || b == b'\t' || b.is_ascii_graphic())
}

fn number(k: &str, v: &str, lo: u64, hi: u64) -> Result<u64, String> {
    v.parse::<u64>()
        .ok()
        .filter(|n| (lo..=hi).contains(n) && v.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("{k}={v}: expected {lo}..={hi}"))
}

fn once<T>(slot: &mut Option<T>, k: &str, v: T) -> Result<(), String> {
    if slot.replace(v).is_some() {
        return Err(format!("{k} given twice"));
    }
    Ok(())
}

/// Parses a sinks file; checks syntax and what needs no signal table.
///
/// # Errors
/// Every bad line, as `(line number from 1, why)`.
pub fn parse(src: &str) -> Result<SinksFile, Vec<(usize, String)>> {
    let mut out = SinksFile::default();
    let mut errs = Vec::new();
    for (n, raw) in src.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default();
        let words: Vec<&str> = line.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            continue;
        }
        if let Err(e) = declaration(n + 1, &words, &mut out) {
            errs.push((n + 1, e));
        }
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

fn declaration(line: usize, words: &[&str], out: &mut SinksFile) -> Result<(), String> {
    match words {
        ["sink", name, url, opts @ ..] => sink(line, name, url, opts, out),
        ["header", sink, name, source] => header(sink, name, source, out),
        ["header", sink, name, ..] => {
            if out.sinks.iter().any(|s| s.name == *sink) && is_token(name) {
                Err(format!(
                    "header {name}: one value, without spaces (use env: or file: for more)"
                ))
            } else {
                Err("header: expected header SINK NAME SOURCE".into())
            }
        }
        ["emit", sink, signal, trigger] => {
            if !is_ident(signal) {
                return Err(format!("emit: signal {signal:?} is not an identifier"));
            }
            let trigger = match *trigger {
                "change" => Trigger::Change,
                "rise" => Trigger::Rise,
                "fall" => Trigger::Fall,
                t => match t.strip_prefix("every_ms=") {
                    Some(v) => Trigger::Every(Duration::from_millis(number("every_ms", v, 100, 86_400_000)?)),
                    None => return Err(format!("{t:?}: expected change, rise, fall or every_ms=N")),
                },
            };
            out.emits.push(EmitDecl {
                line,
                sink: (*sink).into(),
                signal: (*signal).into(),
                trigger,
            });
            Ok(())
        }
        [w, ..] if matches!(*w, "sink" | "header" | "emit") => Err(format!("{w}: wrong number of words")),
        // The line is not echoed: a mistyped line may hold a credential.
        [_, ..] => Err("expected a sink, header or emit line".into()),
        [] => Ok(()),
    }
}

fn sink(line: usize, name: &str, url: &str, opts: &[&str], out: &mut SinksFile) -> Result<(), String> {
    if !is_ident(name) || name.len() > 64 {
        return Err(format!(
            "sink name {name:?}: an identifier of at most 64 characters"
        ));
    }
    if out.sinks.iter().any(|s| s.name == name) {
        return Err(format!("sink {name} declared twice"));
    }
    let url = Url::parse(url).map_err(|e| format!("sink {name}: {e}"))?;
    let (mut timeout, mut queue, mut min, mut max, mut local) = (None, None, None, None, None);
    for o in opts {
        let (k, v) = o.split_once('=').unwrap_or((o, ""));
        match k {
            "timeout_ms" => once(&mut timeout, k, number(k, v, 1, 60_000)?)?,
            "queue_bytes" => once(&mut queue, k, number(k, v, 4096, 64 << 20)?)?,
            "retry_min_ms" => once(&mut min, k, number(k, v, 1, 600_000)?)?,
            "retry_max_ms" => once(&mut max, k, number(k, v, 1, 3_600_000)?)?,
            "allow_local" => match v {
                "true" => once(&mut local, k, true)?,
                "false" => once(&mut local, k, false)?,
                _ => return Err(format!("allow_local={v}: true or false")),
            },
            _ => {
                return Err(format!(
                    "sink {name}: unknown option (not shown); expected timeout_ms=, queue_bytes=, retry_min_ms=, retry_max_ms= or allow_local="
                ));
            }
        }
    }
    let min = min.unwrap_or(1000);
    let max = max.unwrap_or(300_000.max(min));
    if max < min {
        return Err(format!("retry_max_ms={max} is below retry_min_ms={min}"));
    }
    out.sinks.push(SinkDecl {
        line,
        name: name.into(),
        url,
        timeout: Duration::from_millis(timeout.unwrap_or(5000)),
        queue_bytes: usize::try_from(queue.unwrap_or(1 << 20)).unwrap_or(1 << 20),
        backoff: Backoff {
            min: Duration::from_millis(min),
            max: Duration::from_millis(max),
        },
        allow_local: local.unwrap_or(false),
        headers: Vec::new(),
    });
    Ok(())
}

fn header(sink: &str, name: &str, source: &str, out: &mut SinksFile) -> Result<(), String> {
    let Some(decl) = out.sinks.iter_mut().find(|s| s.name == sink) else {
        return Err(format!("header for {sink:?}: no sink of that name above"));
    };
    if !is_token(name) || name.len() > 64 {
        return Err("header name: letters, digits and !#$%&'*+-.^_`|~, at most 64".into());
    }
    if RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(format!("header {name}: set by the box itself"));
    }
    if decl.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Err(format!("header {name} given twice for sink {sink}"));
    }
    let src = if let Some(v) = source.strip_prefix("env:") {
        if v.is_empty() {
            return Err(format!("header {name}: env: needs a variable name"));
        }
        HeaderSource::Env(v.into())
    } else if let Some(p) = source.strip_prefix("file:") {
        if p.is_empty() {
            return Err(format!("header {name}: file: needs a path"));
        }
        HeaderSource::File(p.into())
    } else if let Some(v) = source.strip_prefix("text:") {
        if sensitive(name) {
            return Err(format!(
                "header {name} carries a credential: load it with env: or file:, not text:"
            ));
        }
        HeaderSource::Text(Secret::new(v.into()))
    } else {
        return Err(format!(
            "header {name}: value must be env:VAR, file:PATH or text:VALUE"
        ));
    };
    decl.headers.push((name.into(), src));
    Ok(())
}

impl SinksFile {
    /// Loads every header value and checks what the build can do; returns
    /// each sink ready to start.
    ///
    /// # Errors
    /// Every header that cannot be loaded or is malformed (named, never
    /// shown), `https` URLs in a build without TLS, and URLs whose host is
    /// an address or name [`super::url`] refuses (names that resolve to one
    /// are refused at each send, since DNS can change).
    pub fn resolve(
        &self,
        env: &dyn Fn(&str) -> Option<String>,
        read: &dyn Fn(&Path) -> std::io::Result<String>,
    ) -> Result<Vec<SinkConfig>, Vec<String>> {
        let mut errs = Vec::new();
        let mut out = Vec::new();
        for s in &self.sinks {
            if s.url.scheme == Scheme::Https && !cfg!(feature = "https") {
                errs.push(format!(
                    "line {}: sink {}: https:// needs a build with the https feature",
                    s.line, s.name
                ));
            }
            if s.url.is_localhost() && !s.allow_local {
                errs.push(format!(
                    "line {}: sink {}: localhost refused (set allow_local=true to allow)",
                    s.line, s.name
                ));
            }
            if let Ok(ip) = s.url.host.parse::<std::net::IpAddr>()
                && let Some(why) = super::url::refused(ip, s.allow_local)
            {
                errs.push(format!(
                    "line {}: sink {}: address {ip} refused: {why}",
                    s.line, s.name
                ));
            }
            let mut headers = Vec::new();
            for (name, src) in &s.headers {
                let what = format!("line {}: sink {} header {name}", s.line, s.name);
                let value = match src {
                    HeaderSource::Env(v) => {
                        let Some(x) = env(v) else {
                            errs.push(format!("{what}: environment variable {v} is not set"));
                            continue;
                        };
                        x
                    }
                    HeaderSource::File(p) => match read(p) {
                        Ok(x) if x.len() as u64 <= MAX_HEADER_FILE => x.trim_end().to_string(),
                        Ok(_) => {
                            errs.push(format!("{what}: {} is over {MAX_HEADER_FILE} bytes", p.display()));
                            continue;
                        }
                        Err(e) => {
                            errs.push(format!("{what}: cannot read {}: {}", p.display(), e.kind()));
                            continue;
                        }
                    },
                    HeaderSource::Text(v) => v.expose().to_string(),
                };
                if value.is_empty() || !value_ok(&value) {
                    errs.push(format!(
                        "{what}: the value is empty or has characters a header cannot carry (not shown)"
                    ));
                    continue;
                }
                headers.push((name.clone(), Secret::new(value)));
            }
            out.push(SinkConfig {
                name: s.name.clone(),
                url: s.url.clone(),
                headers,
                timeout: s.timeout,
                queue_bytes: s.queue_bytes,
                backoff: s.backoff,
                allow_local: s.allow_local,
            });
        }
        if errs.is_empty() { Ok(out) } else { Err(errs) }
    }
}

/// Reads a header file, refusing anything over [`MAX_HEADER_FILE`] without
/// reading it all.
///
/// # Errors
/// What opening or reading returns; `InvalidData` if not UTF-8.
pub fn read_header_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut s = String::new();
    std::fs::File::open(path)?
        .take(MAX_HEADER_FILE + 1)
        .read_to_string(&mut s)?;
    Ok(s)
}
