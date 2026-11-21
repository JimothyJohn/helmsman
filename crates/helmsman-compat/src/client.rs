//! Requests to a box or a relay with the box's own client
//! (`helmsman_agent::http`): plain JSON, and a POST whose answer streams.

use std::time::Duration;

use helmsman_agent::http::{self, Timeouts, Url};
use helmsman_agent::sse;
use serde_json::Value as Json;

fn timeouts(total: u64) -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(3),
        idle: Duration::from_secs(total),
        total: Duration::from_secs(total),
    }
}

fn send(
    url: &Url,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Json>,
    total: u64,
) -> Result<http::Response, String> {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("Authorization", a.as_str())).collect();
    let bytes = body.map(|b| b.to_string().into_bytes());
    let full = format!("{}{path}", url.prefix);
    let req = http::Request {
        method,
        path: &full,
        headers: &headers,
        json: bytes.as_deref(),
        raw: None,
    };
    http::send(url, &req, timeouts(total)).map_err(|e| format!("{method} {path}: {e:?}"))
}

/// A GET: the status and the body as JSON (`Null` when it is not JSON).
///
/// # Errors
/// The request could not be made or read.
pub fn get(url: &Url, path: &str, token: Option<&str>) -> Result<(u16, Json), String> {
    let r = send(url, "GET", path, token, None, 30)?;
    let status = r.status;
    let b = r.read_body(16 << 20).map_err(|e| format!("GET {path}: {e:?}"))?;
    Ok((status, serde_json::from_slice(&b).unwrap_or(Json::Null)))
}

/// Any request with an optional JSON body: the status and the body as
/// JSON (`Null` when it is not JSON).
///
/// # Errors
/// The request could not be made or read.
pub fn call(
    url: &Url,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Json>,
) -> Result<(u16, Json), String> {
    let r = send(url, method, path, token, body, 60)?;
    let status = r.status;
    let b = r
        .read_body(16 << 20)
        .map_err(|e| format!("{method} {path}: {e:?}"))?;
    Ok((status, serde_json::from_slice(&b).unwrap_or(Json::Null)))
}

/// A POST whose 200 answer is an event stream: the status and every
/// event (name, data) until it ends; a non-200 answer is one `body` event.
///
/// # Errors
/// The request could not be made.
pub fn post_stream(
    url: &Url,
    path: &str,
    token: Option<&str>,
    body: &Json,
    total: u64,
) -> Result<(u16, Vec<(String, Json)>), String> {
    let r = send(url, "POST", path, token, Some(body), total)?;
    let status = r.status;
    if status != 200 {
        let b = r.read_body(1 << 20).unwrap_or_default();
        let j =
            serde_json::from_slice(&b).unwrap_or_else(|_| Json::String(String::from_utf8_lossy(&b).into()));
        return Ok((status, vec![("body".into(), j)]));
    }
    let mut stream = r.body;
    let mut reader = sse::Reader::new();
    let mut out = Vec::new();
    loop {
        match reader.next(&mut stream) {
            Ok(Some(e)) => {
                let data = serde_json::from_str(&e.data).unwrap_or(Json::String(e.data));
                out.push((e.name, data));
            }
            Ok(None) => break,
            Err(e) => {
                out.push(("stream_error".into(), Json::String(format!("{e:?}"))));
                break;
            }
        }
    }
    Ok((status, out))
}

/// A POST of `body` as it is (a document, not JSON) with `content_type`:
/// the status and the answer as JSON (`Null` when it is not JSON).
///
/// # Errors
/// The request could not be made or read.
pub fn post_raw(
    url: &Url,
    path: &str,
    token: Option<&str>,
    content_type: &str,
    body: &[u8],
) -> Result<(u16, Json), String> {
    let auth = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = auth.iter().map(|a| ("Authorization", a.as_str())).collect();
    let full = format!("{}{path}", url.prefix);
    let req = http::Request {
        method: "POST",
        path: &full,
        headers: &headers,
        json: None,
        raw: Some((content_type, body)),
    };
    let r = http::send(url, &req, timeouts(60)).map_err(|e| format!("POST {path}: {e:?}"))?;
    let status = r.status;
    let b = r.read_body(16 << 20).map_err(|e| format!("POST {path}: {e:?}"))?;
    Ok((status, serde_json::from_slice(&b).unwrap_or(Json::Null)))
}
