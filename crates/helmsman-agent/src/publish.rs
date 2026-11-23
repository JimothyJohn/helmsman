//! `publish_device_profile`, the relay's second tool (card
//! `device-profile-publish`; the relay's `docs/relay-protocol.md`, "`POST
//! /v1/agents/{agent}/tools/publish_device_profile`"): the box asks the
//! relay to open a pull request into the public device library with a
//! profile an engineer saved. The relay holds the only GitHub credential;
//! this module sends the box's relay token to the relay and nothing else
//! anywhere.
//!
//! A person's action, never the model's: no tool of the agent calls it
//! (`tools::NAMES` does not list it), only the box's own route does.

use std::fmt;

use serde_json::{Value as Json, json};

use crate::http::{self, Request};
use crate::relay;

/// Largest answer read from the relay.
const MAX_ANSWER: usize = 64 * 1024;
/// Longest text kept from the relay, characters.
const MAX_WORDS: usize = 400;

/// The pull request the relay opened, or already had open for this
/// profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub number: u64,
    /// `https://github.com/OWNER/REPO/pull/N`, rebuilt here from the
    /// relay's words only if it has that shape.
    pub url: String,
    /// `false`: the same profile already had an open pull request.
    pub created: bool,
    pub draft: bool,
    /// The library folder the profile went beside.
    pub folder: String,
}

/// Why nothing was published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The relay could not be reached, or did not answer in time.
    Unreachable(String),
    /// The relay answered with an error: its status, its error type and
    /// its words (cleaned and cut).
    Refused {
        status: u16,
        kind: String,
        message: String,
        retryable: bool,
    },
    /// The relay's answer is not what the protocol says.
    Malformed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(why) => write!(f, "the answer service could not be reached: {why}"),
            Self::Refused { message, .. } => f.write_str(message),
            Self::Malformed => f.write_str("the answer service's answer was not understood"),
        }
    }
}

impl std::error::Error for Error {}

/// Printable text of at most [`MAX_WORDS`] characters.
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
        })
        .take(MAX_WORDS)
        .collect()
}

/// A library pull request's page: `https://github.com/OWNER/REPO/pull/N`
/// with plain owner and repository names.
fn pull_url_ok(url: &str, number: u64) -> bool {
    let plain = |p: &str| {
        (1..=100).contains(&p.len())
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && !p.bytes().all(|b| b == b'.')
    };
    url.strip_prefix("https://github.com/")
        .and_then(|r| r.strip_suffix(&format!("/pull/{number}")))
        .and_then(|r| r.split_once('/'))
        .is_some_and(|(o, n)| plain(o) && plain(n))
}

/// Asks the relay (`cfg`: its URL, the box's token, and the agent version
/// whose profile lists the tool) to publish the profile `name` with `text`.
///
/// # Errors
/// See [`Error`]. Nothing is retried here.
pub fn publish(cfg: &relay::Config, name: &str, text: &str) -> Result<Published, Error> {
    let body = json!({ "name": name, "text": text }).to_string();
    let version = cfg
        .version
        .split('/')
        .map(http::segment)
        .collect::<Vec<_>>()
        .join("/");
    let path = format!(
        "{}/v1/agents/{version}/tools/publish_device_profile",
        cfg.url.prefix
    );
    let auth = format!("Bearer {}", cfg.token);
    let req = Request {
        method: "POST",
        path: &path,
        headers: &[("Authorization", &auth)],
        json: Some(body.as_bytes()),
        raw: None,
    };
    let resp = http::send(&cfg.url, &req, cfg.timeouts).map_err(|e| Error::Unreachable(e.to_string()))?;
    let status = resp.status;
    let bytes = resp.read_body(MAX_ANSWER).map_err(|_| Error::Malformed)?;
    let j: Json = serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
    if matches!(status, 200 | 201) {
        let number = j["number"].as_u64().ok_or(Error::Malformed)?;
        let url = j["url"]
            .as_str()
            .filter(|u| pull_url_ok(u, number))
            .ok_or(Error::Malformed)?;
        return Ok(Published {
            number,
            url: url.to_owned(),
            created: status == 201 && j["created"] == true,
            draft: j["draft"] == true,
            folder: clean(j["folder"].as_str().unwrap_or_default()),
        });
    }
    let e = &j["error"];
    Err(Error::Refused {
        status,
        kind: clean(e["type"].as_str().unwrap_or("relay_error")),
        message: clean(e["message"].as_str().unwrap_or("no reason given")),
        retryable: e["retryable"] == true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_library_pull_request_page_is_a_link() {
        assert!(pull_url_ok(
            "https://github.com/JimothyJohn/helmsman-devices/pull/12",
            12
        ));
        for bad in [
            "https://github.com/JimothyJohn/helmsman-devices/pull/13",
            "http://github.com/o/r/pull/12",
            "https://evil.example/o/r/pull/12",
            "https://github.com.evil.example/o/r/pull/12",
            "https://github.com/o/r/pull/12?x=1",
            "https://github.com/o/r/pull/12/files",
            "https://github.com/o/r/issues/12",
            "https://github.com/o/pull/12",
            "https://github.com/o/r s/pull/12",
            "https://github.com/../r/pull/12",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!pull_url_ok(bad, 12), "{bad}");
        }
    }

    #[test]
    fn the_relays_words_are_cut_and_cleaned() {
        let long = "x".repeat(1000);
        assert_eq!(clean(&long).len(), MAX_WORDS);
        assert_eq!(clean("a\u{0}b\u{202e}c\nd"), "abcd");
    }
}
