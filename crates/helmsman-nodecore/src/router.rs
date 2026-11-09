//! Which route does what. [`handle`] reads one request against the
//! node's view and settings and answers with a reply, or names the
//! stream to open, or the [`Effect`] the firmware must carry out before
//! replying (a deploy, a module control, a settings save), since the
//! router cannot compile a program or write flash.
//!
//! Routes, the box's where a node has the thing:
//!
//! ```text
//! GET  /                          the node's page (status and the settings form)
//! GET  /settings                  the same
//! POST /settings                  the form: SaveSettings (token in the form or as a bearer)
//! GET  /api/modules               the one module
//! GET  /api/modules/{name}        with its source
//! PUT  /api/modules/{name}        token: Deploy the body (ST text, or {"source": ...})
//! POST /api/modules/{name}/run|stop|restart   token: Control
//! GET  /api/events                the values stream
//! GET  /api/signals  /api/devices  /api/io_map  /api/site  /api/node  /api/settings
//! PUT  /api/settings              token: SaveSettings from JSON
//! OPTIONS *                       204 with the CORS headers
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde_json::json;

use crate::api::{self, NodeView};
use crate::http::{Request, Response, form_pairs};
use crate::settings::Settings;

/// What the firmware must do before the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Compile `source` as module `name`; run it and keep it if it
    /// compiles ([`deployed`]), else [`deploy_refused`].
    Deploy { name: String, source: String },
    /// Run, stop or restart module `name` ([`controlled`]).
    Control { name: String, action: Control },
    /// Write the record and restart the network ([`settings_saved`]); the
    /// request came from the page when `page` is set, so the reply is a
    /// redirect to it.
    SaveSettings { settings: Settings, page: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Run,
    Stop,
    Restart,
}

impl Control {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// How a request is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Send this and close.
    Reply(Response),
    /// Send [`crate::http::sse_head`], then a `values` event whenever the
    /// view's `seq` changes, until the peer goes away.
    Stream,
    /// Carry this out, then reply with the matching helper.
    Effect(Effect),
}

/// Compares two tokens in time that does not depend on where they differ.
fn same_token(given: &str, want: &str) -> bool {
    if given.len() != want.len() {
        return false;
    }
    given
        .bytes()
        .zip(want.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn authorize(token: Option<&str>, settings: &Settings) -> Result<(), Response> {
    if settings.token.is_empty() {
        return Err(Response::error(
            403,
            "this node has no API token set; set one on its page over the console first",
        ));
    }
    match token {
        Some(t) if same_token(t, &settings.token) => Ok(()),
        Some(_) => Err(Response::error(401, "wrong token")),
        None => Err(Response::error(401, "writes need Authorization: Bearer <token>")),
    }
}

fn module_named<'a>(view: &'a NodeView, name: &str) -> Option<&'a api::ModuleView> {
    view.module.as_ref().filter(|m| m.name == name)
}

/// Reads one request.
#[must_use]
pub fn handle(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    if req.method == "OPTIONS" {
        return Outcome::Reply(Response::new(204, "text/plain", Vec::new()));
    }
    let segments = req.segments();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", [] | ["settings" | "index.html"]) => {
            Outcome::Reply(Response::html(200, api::settings_page(view, settings, None)))
        }
        ("POST", ["settings"]) => settings_form(req, view, settings),
        ("GET", ["api", "modules"]) => Outcome::Reply(Response::json(200, &api::modules(view))),
        ("GET", ["api", "modules", name]) => match module_named(view, name) {
            Some(m) => Outcome::Reply(Response::json(200, &api::module_detail(m))),
            None => Outcome::Reply(Response::error(404, "no such module on this node")),
        },
        ("PUT", ["api", "modules", name]) => deploy(req, settings, name),
        ("POST", ["api", "modules", name, action]) => {
            if let Err(e) = authorize(req.bearer(), settings) {
                return Outcome::Reply(e);
            }
            if module_named(view, name).is_none() {
                return Outcome::Reply(Response::error(404, "no such module on this node"));
            }
            let action = match *action {
                "run" => Control::Run,
                "stop" => Control::Stop,
                "restart" => Control::Restart,
                _ => return Outcome::Reply(Response::error(404, "run, stop or restart")),
            };
            Outcome::Effect(Effect::Control {
                name: String::from(*name),
                action,
            })
        }
        (
            "GET",
            ["api", "modules", _, "trace"]
            | ["api", "modules", _, "trace", "events"]
            | ["api", "traces", "events"],
        ) => Outcome::Reply(Response::error(404, "a node keeps no traces; watch its values")),
        ("GET", ["api", "events"]) => Outcome::Stream,
        ("GET", ["api", "signals"]) => Outcome::Reply(Response::json(200, &api::signals(view))),
        ("GET", ["api", "devices"]) => Outcome::Reply(Response::json(200, &api::devices(view))),
        ("GET", ["api", "io_map"]) => Outcome::Reply(Response::json(200, &api::io_map(view))),
        ("GET", ["api", "site"]) => Outcome::Reply(Response::json(200, &api::site(view, settings))),
        ("GET", ["api", "node"]) => Outcome::Reply(Response::json(200, &api::node(view, settings))),
        ("GET", ["api", "settings"]) => Outcome::Reply(Response::json(200, &api::settings_json(settings))),
        ("PUT", ["api", "settings"]) => settings_put(req, settings),
        ("GET", ["api", ..]) => Outcome::Reply(Response::error(
            404,
            "not on a node (a node serves modules, events, signals, devices, io_map, site, node, settings)",
        )),
        ("GET", _) => Outcome::Reply(Response::error(404, "not found")),
        _ => Outcome::Reply(Response::error(405, "method not allowed")),
    }
}

/// `POST /settings`: the page's form.
fn settings_form(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    let pairs = form_pairs(&req.body);
    // The form carries the current token as `auth` (the page asks for it
    // on save); a bearer header works too.
    let given = req.bearer().or_else(|| {
        pairs
            .iter()
            .find(|(k, _)| k == "auth")
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    });
    // On a fresh node no token is set yet: the first save sets it.
    if let Err(e) = authorize(given, settings)
        && !settings.token.is_empty()
    {
        return Outcome::Reply(Response::html(
            e.status,
            api::settings_page(
                view,
                settings,
                Some("not saved: the current API token is needed to change settings"),
            ),
        ));
    }
    match api::settings_from_pairs(settings, &pairs) {
        Ok(new) => Outcome::Effect(Effect::SaveSettings {
            settings: new,
            page: true,
        }),
        Err(e) => Outcome::Reply(Response::html(
            422,
            api::settings_page(view, settings, Some(&format!("not saved: {}", e.0))),
        )),
    }
}

/// `PUT /api/settings`: JSON.
fn settings_put(req: &Request, settings: &Settings) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let Ok(j) = serde_json::from_slice::<serde_json::Value>(&req.body) else {
        return Outcome::Reply(Response::error(400, "body is not JSON"));
    };
    match api::settings_from_pairs(settings, &api::settings_pairs_from_json(&j)) {
        Ok(new) => Outcome::Effect(Effect::SaveSettings {
            settings: new,
            page: false,
        }),
        Err(e) => Outcome::Reply(Response::error(422, &e.0)),
    }
}

/// `PUT /api/modules/{name}`: the body is the program.
fn deploy(req: &Request, settings: &Settings, name: &str) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let is_json = req
        .header("content-type")
        .is_some_and(|t| t.starts_with("application/json"));
    let source = if is_json {
        match serde_json::from_slice::<serde_json::Value>(&req.body) {
            Ok(j) => match j["source"].as_str() {
                Some(s) => String::from(s),
                None => return Outcome::Reply(Response::error(400, "want {\"source\": \"...\"}")),
            },
            Err(_) => return Outcome::Reply(Response::error(400, "body is not JSON")),
        }
    } else {
        match core::str::from_utf8(&req.body) {
            Ok(s) => String::from(s),
            Err(_) => return Outcome::Reply(Response::error(400, "source is not UTF-8")),
        }
    };
    if source.trim().is_empty() {
        return Outcome::Reply(Response::error(400, "empty program"));
    }
    if name.len() > 32
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Outcome::Reply(Response::error(
            400,
            "module name: letters, digits, '_' and '-', up to 32",
        ));
    }
    Outcome::Effect(Effect::Deploy {
        name: String::from(name),
        source,
    })
}

/// The reply once a [`Effect::Deploy`] compiled and runs.
#[must_use]
pub fn deployed(name: &str, source_bytes: usize) -> Response {
    Response::json(
        201,
        &json!({ "module": name, "status": { "state": "running" }, "bytes": source_bytes }),
    )
}

/// The reply when a [`Effect::Deploy`] did not compile; `errors` are the
/// checker's lines.
#[must_use]
pub fn deploy_refused(errors: &[String]) -> Response {
    Response::json(
        422,
        &json!({ "error": "the program does not compile", "errors": errors }),
    )
}

/// The reply when a [`Effect::Deploy`] compiled but could not be kept
/// (no flash to write it to, or too big for the slot): it runs until the
/// next restart.
#[must_use]
pub fn deployed_not_kept(name: &str, why: &str) -> Response {
    Response::json(
        202,
        &json!({ "module": name, "status": { "state": "running" }, "kept": false, "why": why }),
    )
}

/// The reply to a [`Effect::Control`].
#[must_use]
pub fn controlled(name: &str, action: Control, state: &str) -> Response {
    Response::json(
        200,
        &json!({ "module": name, "action": action.name(), "status": { "state": state } }),
    )
}

/// The reply once a [`Effect::SaveSettings`] is on flash: a redirect back
/// to the page, or JSON.
#[must_use]
pub fn settings_saved(page: bool, restarting: bool) -> Response {
    if page {
        Response::see_other("/settings?saved")
    } else {
        Response::json(200, &json!({ "saved": true, "restarting": restarting }))
    }
}

/// The reply when the record could not be written.
#[must_use]
pub fn settings_not_saved(page: bool, view: &NodeView, settings: &Settings, why: &str) -> Response {
    if page {
        Response::html(
            500,
            api::settings_page(view, settings, Some(&format!("not saved: {why}"))),
        )
    } else {
        Response::error(500, why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::sample_view;
    use crate::http::{Read, parse};

    fn settings() -> Settings {
        Settings {
            name: "press-4".into(),
            site: "line-a".into(),
            token: "tok".into(),
            ..Settings::default()
        }
    }

    fn req(raw: &str) -> Request {
        match parse(raw.as_bytes()).unwrap() {
            Read::Done(r, _) => r,
            Read::Incomplete => panic!("incomplete"),
        }
    }

    fn reply(o: Outcome) -> Response {
        match o {
            Outcome::Reply(r) => r,
            other => panic!("not a reply: {other:?}"),
        }
    }

    #[test]
    fn reads_are_open_and_shaped() {
        let v = sample_view();
        let s = settings();
        let r = reply(handle(&req("GET /api/modules HTTP/1.1\r\n\r\n"), &v, &s));
        assert_eq!(r.status, 200);
        let j: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(j[0]["name"], "chase");
        let r = reply(handle(&req("GET /api/modules/chase HTTP/1.1\r\n\r\n"), &v, &s));
        assert_eq!(r.status, 200);
        let r = reply(handle(&req("GET /api/modules/other HTTP/1.1\r\n\r\n"), &v, &s));
        assert_eq!(r.status, 404);
        for path in [
            "/api/signals",
            "/api/devices",
            "/api/io_map",
            "/api/site",
            "/api/node",
            "/api/settings",
        ] {
            let r = reply(handle(&req(&format!("GET {path} HTTP/1.1\r\n\r\n")), &v, &s));
            assert_eq!(r.status, 200, "{path}");
        }
        assert_eq!(
            handle(&req("GET /api/events HTTP/1.1\r\n\r\n"), &v, &s),
            Outcome::Stream
        );
        let r = reply(handle(
            &req("GET /api/modules/chase/trace/events HTTP/1.1\r\n\r\n"),
            &v,
            &s,
        ));
        assert_eq!(r.status, 404);
        let r = reply(handle(&req("GET /api/proposals HTTP/1.1\r\n\r\n"), &v, &s));
        assert_eq!(r.status, 404);
        let r = reply(handle(&req("GET / HTTP/1.1\r\n\r\n"), &v, &s));
        assert_eq!(r.status, 200);
        assert_eq!(r.content_type, "text/html; charset=utf-8");
        let r = reply(handle(
            &req("OPTIONS /api/modules/chase HTTP/1.1\r\n\r\n"),
            &v,
            &s,
        ));
        assert_eq!(r.status, 204);
    }

    #[test]
    fn writes_need_the_token() {
        let v = sample_view();
        let s = settings();
        let body = "PROGRAM p END_PROGRAM";
        let no = req(&format!(
            "PUT /api/modules/p HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(reply(handle(&no, &v, &s)).status, 401);
        let wrong = req(&format!(
            "PUT /api/modules/p HTTP/1.1\r\nAuthorization: Bearer nope\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(reply(handle(&wrong, &v, &s)).status, 401);
        let close = req(&format!(
            "PUT /api/modules/p HTTP/1.1\r\nAuthorization: Bearer toK\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(reply(handle(&close, &v, &s)).status, 401);
        let ok = req(&format!(
            "PUT /api/modules/p HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(
            handle(&ok, &v, &s),
            Outcome::Effect(Effect::Deploy {
                name: "p".into(),
                source: body.into()
            })
        );
        let mut none = s.clone();
        none.token.clear();
        assert_eq!(reply(handle(&ok, &v, &none)).status, 403);
    }

    #[test]
    fn deploy_bodies() {
        let v = sample_view();
        let s = settings();
        let json_body = r#"{"source": "PROGRAM q END_PROGRAM"}"#;
        let r = req(&format!(
            "PUT /api/modules/q HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json_body}",
            json_body.len()
        ));
        assert_eq!(
            handle(&r, &v, &s),
            Outcome::Effect(Effect::Deploy {
                name: "q".into(),
                source: "PROGRAM q END_PROGRAM".into()
            })
        );
        let empty =
            req("PUT /api/modules/q HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Length: 1\r\n\r\n ");
        assert_eq!(reply(handle(&empty, &v, &s)).status, 400);
        let bad_name =
            req("PUT /api/modules/a%20b HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Length: 1\r\n\r\nx");
        assert_eq!(reply(handle(&bad_name, &v, &s)).status, 400);
        let bad_json = req(
            "PUT /api/modules/q HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{]",
        );
        assert_eq!(reply(handle(&bad_json, &v, &s)).status, 400);
    }

    #[test]
    fn controls_and_settings() {
        let v = sample_view();
        let s = settings();
        let r = req("POST /api/modules/chase/stop HTTP/1.1\r\nAuthorization: Bearer tok\r\n\r\n");
        assert_eq!(
            handle(&r, &v, &s),
            Outcome::Effect(Effect::Control {
                name: "chase".into(),
                action: Control::Stop
            })
        );
        let r = req("POST /api/modules/chase/dance HTTP/1.1\r\nAuthorization: Bearer tok\r\n\r\n");
        assert_eq!(reply(handle(&r, &v, &s)).status, 404);
        let r = req("POST /api/modules/nope/run HTTP/1.1\r\nAuthorization: Bearer tok\r\n\r\n");
        assert_eq!(reply(handle(&r, &v, &s)).status, 404);
        let body = r#"{"name": "press-5"}"#;
        let r = req(&format!(
            "PUT /api/settings HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        match handle(&r, &v, &s) {
            Outcome::Effect(Effect::SaveSettings {
                settings,
                page: false,
            }) => {
                assert_eq!(settings.name, "press-5");
                assert_eq!(settings.token, "tok");
            }
            other => panic!("{other:?}"),
        }
        let bad = r#"{"name": "has space"}"#;
        let r = req(&format!(
            "PUT /api/settings HTTP/1.1\r\nAuthorization: Bearer tok\r\nContent-Length: {}\r\n\r\n{bad}",
            bad.len()
        ));
        assert_eq!(reply(handle(&r, &v, &s)).status, 422);
    }

    #[test]
    fn the_form_needs_the_token_once_one_is_set_and_not_before() {
        let v = sample_view();
        let s = settings();
        let body = "name=press-6&mode=dhcp&auth=tok";
        let r = req(&format!(
            "POST /settings HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        match handle(&r, &v, &s) {
            Outcome::Effect(Effect::SaveSettings { settings, page: true }) => {
                assert_eq!(settings.name, "press-6");
            }
            other => panic!("{other:?}"),
        }
        let body = "name=press-6&mode=dhcp";
        let r = req(&format!(
            "POST /settings HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        let rep = reply(handle(&r, &v, &s));
        assert_eq!(rep.status, 401);
        assert_eq!(rep.content_type, "text/html; charset=utf-8");
        // A fresh node: the first save sets the token with no token given.
        let mut fresh = s.clone();
        fresh.token.clear();
        let body = "name=new&mode=dhcp&token=first";
        let r = req(&format!(
            "POST /settings HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        match handle(&r, &v, &fresh) {
            Outcome::Effect(Effect::SaveSettings { settings, page: true }) => {
                assert_eq!(settings.token, "first");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reply_helpers() {
        assert_eq!(deployed("p", 10).status, 201);
        assert_eq!(deploy_refused(&["1:1: nope".to_string()]).status, 422);
        assert_eq!(deployed_not_kept("p", "no flash").status, 202);
        assert_eq!(controlled("p", Control::Run, "running").status, 200);
        assert_eq!(settings_saved(true, true).status, 303);
        assert_eq!(settings_saved(false, false).status, 200);
        assert_eq!(
            settings_not_saved(false, &sample_view(), &settings(), "flash").status,
            500
        );
    }
}
