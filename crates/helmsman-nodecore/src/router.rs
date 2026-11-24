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
//! POST /io_map                    the form: SaveIoMap (the box's io.map format: modbus-tcp and enip-io devices)
//! GET  /api/modules               the one module
//! GET  /api/modules/{name}        with its source
//! PUT  /api/modules/{name}        token: Deploy the body (ST text, or {"source": ...})
//! POST /api/modules/{name}/run|stop|restart   token: Control
//! GET  /api/events                the values stream
//! GET  /api/signals  /api/devices  /api/io_map  /api/site  /api/node  /api/settings
//! PUT  /api/settings              token: SaveSettings from JSON
//! PUT  /api/io_map                token: SaveIoMap from {"text"} or text/plain; 422 names the bad line
//! GET  /api/firmware              the running image, its trial and the fetch (404 on a board with one slot)
//! POST /api/firmware              token: FirmwareFetch {"url", "sha256"}; 202, the node fetches and restarts
//! POST /api/firmware/confirm      token: FirmwareConfirm, the running image works
//! OPTIONS *                       204 with the CORS headers
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde_json::json;

use crate::api::{self, NodeView};
use crate::http::{Request, Response, form_pairs};
use crate::ota;
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
    /// Write the I/O map beside the program and restart, so the I/O task
    /// drives the block it names ([`io_map_saved`]). The text parsed
    /// ([`crate::iomap::parse`]) before it got here.
    SaveIoMap { text: String, page: bool },
    /// Start fetching an image into the idle slot ([`firmware_accepted`]);
    /// the node restarts into it once it is written and checked.
    FirmwareFetch { fetch: ota::request::Fetch },
    /// End the running image's trial ([`firmware_confirmed`]).
    FirmwareConfirm,
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
        ("POST", ["io_map"]) => io_map_form(req, view, settings),
        ("PUT", ["api", "io_map"]) => io_map_put(req, view, settings),
        ("GET", ["api", "firmware"]) => Outcome::Reply(match &view.firmware {
            Some(fw) => Response::json(200, &fw.json()),
            None => Response::error(404, "this board cannot update its firmware over the network"),
        }),
        ("POST", ["api", "firmware"]) => firmware_fetch(req, view, settings),
        ("POST", ["api", "firmware", "confirm"]) => {
            if let Err(e) = authorize(req.bearer(), settings) {
                return Outcome::Reply(e);
            }
            if view.firmware.is_none() {
                return Outcome::Reply(Response::error(
                    404,
                    "this board cannot update its firmware over the network",
                ));
            }
            Outcome::Effect(Effect::FirmwareConfirm)
        }
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
    // on save); a bearer header works too. On a fresh node no token is
    // set yet: the first save sets it.
    if let Err(e) = form_authorized(req, &pairs, settings) {
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

/// The token a form sent (`auth`), or the bearer header; on a fresh node
/// with no token yet, nothing is asked.
fn form_authorized(req: &Request, pairs: &[(String, String)], settings: &Settings) -> Result<(), Response> {
    let given = req.bearer().or_else(|| {
        pairs
            .iter()
            .find(|(k, _)| k == "auth")
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    });
    match authorize(given, settings) {
        Err(e) if !settings.token.is_empty() => Err(e),
        _ => Ok(()),
    }
}

/// `POST /io_map`: the settings page's map form (`map`, and `auth`).
fn io_map_form(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    let pairs = form_pairs(&req.body);
    if let Err(e) = form_authorized(req, &pairs, settings) {
        return Outcome::Reply(Response::html(
            e.status,
            api::settings_page(
                view,
                settings,
                Some("map not saved: the current API token is needed"),
            ),
        ));
    }
    let text = pairs
        .iter()
        .find(|(k, _)| k == "map")
        .map(|(_, v)| v.replace("\r\n", "\n"))
        .unwrap_or_default();
    match map_this_board_runs(&text, view) {
        Ok(()) => Outcome::Effect(Effect::SaveIoMap { text, page: true }),
        Err(bad) => Outcome::Reply(Response::html(
            422,
            api::settings_page(view, settings, Some(&format!("map not saved: {bad}"))),
        )),
    }
}

/// Parses `text` as a map, and as one the board can run: a one-block
/// board ([`NodeView::one_block`]) takes less than the format allows.
fn map_this_board_runs(text: &str, view: &NodeView) -> Result<(), crate::iomap::Bad> {
    let map = crate::iomap::parse(text)?;
    if view.one_block {
        map.one_block()?;
    }
    Ok(())
}

/// `PUT /api/io_map`: `{"text": "..."}` or the map as `text/plain`.
fn io_map_put(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let is_json = req
        .header("content-type")
        .is_some_and(|t| t.starts_with("application/json"));
    let text = if is_json {
        match serde_json::from_slice::<serde_json::Value>(&req.body) {
            Ok(j) => match j["text"].as_str() {
                Some(t) => String::from(t),
                None => return Outcome::Reply(Response::error(400, "want {\"text\": \"...\"}")),
            },
            Err(_) => return Outcome::Reply(Response::error(400, "body is not JSON")),
        }
    } else {
        match core::str::from_utf8(&req.body) {
            Ok(t) => String::from(t),
            Err(_) => return Outcome::Reply(Response::error(400, "the map is not UTF-8")),
        }
    };
    match map_this_board_runs(&text, view) {
        Ok(()) => Outcome::Effect(Effect::SaveIoMap { text, page: false }),
        Err(bad) => Outcome::Reply(Response::error(422, &alloc::format!("{bad}"))),
    }
}

/// `POST /api/firmware`: `{"url": "http://a.b.c.d:port/file.bin", "sha256": "<64 hex>"}`.
fn firmware_fetch(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let Some(fw) = &view.firmware else {
        return Outcome::Reply(Response::error(
            404,
            "this board cannot update its firmware over the network",
        ));
    };
    if fw.fetch.busy() {
        return Outcome::Reply(Response::error(
            409,
            "a firmware update is already running; read GET /api/firmware",
        ));
    }
    match ota::request::Fetch::from_json(&req.body) {
        Ok(fetch) => Outcome::Effect(Effect::FirmwareFetch { fetch }),
        Err(why) => Outcome::Reply(Response::error(422, &format!("{why}"))),
    }
}

/// The reply to a [`Effect::FirmwareFetch`] the node has taken up: the
/// fetch runs on its own, so the answer comes before the image does.
#[must_use]
pub fn firmware_accepted(fetch: &ota::request::Fetch) -> Response {
    Response::json(
        202,
        &json!({
            "accepted": true,
            "from": fetch.authority(),
            "path": fetch.path,
            "sha256": ota::request::encode_hex32(&fetch.sha256),
            "next": "poll GET /api/firmware; the node restarts into the new image when it is written and checked",
        }),
    )
}

/// The reply to a [`Effect::FirmwareFetch`] the node could not start.
#[must_use]
pub fn firmware_not_started(why: &str) -> Response {
    Response::error(409, why)
}

/// The reply to a [`Effect::FirmwareConfirm`]: `ended` says whether a
/// trial was on.
#[must_use]
pub fn firmware_confirmed(ended: bool) -> Response {
    Response::json(200, &json!({ "confirmed": ended, "on_trial": false }))
}

/// The reply once a [`Effect::SaveIoMap`] is on flash: a redirect back to
/// the page, or JSON; the node restarts after sending it.
#[must_use]
pub fn io_map_saved(page: bool, restarting: bool) -> Response {
    if page {
        Response::see_other("/settings?map_saved")
    } else {
        Response::json(200, &json!({ "saved": true, "restarting": restarting }))
    }
}

/// The reply when the map could not be written.
#[must_use]
pub fn io_map_not_saved(page: bool, view: &NodeView, settings: &Settings, why: &str) -> Response {
    if page {
        Response::html(
            500,
            api::settings_page(view, settings, Some(&format!("map not saved: {why}"))),
        )
    } else {
        Response::error(500, why)
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

    /// A one-block board refuses, at the API and at the form, a map that
    /// parses but that it cannot run, and never hands it on to be saved.
    #[test]
    fn a_one_block_board_saves_only_a_map_it_runs() {
        let mut v = sample_view();
        v.one_block = true;
        let s = settings();
        let auth = "Authorization: Bearer tok\r\n";
        let put = |body: &str| {
            req(&format!(
                "PUT /api/io_map HTTP/1.1\r\n{auth}Content-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ))
        };
        let one = "device rio modbus-tcp 192.168.3.60\nin estop bool rio di:0\n";
        assert_eq!(
            handle(&put(one), &v, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: one.into(),
                page: false
            })
        );
        for more in [
            "device a modbus-tcp 10.0.0.1\ndevice b modbus-tcp 10.0.0.2\n",
            "device e enip-io 10.0.0.2 profile=moxa-e1213\n",
            "device rio modbus-tcp 192.168.3.60\nin level real rio hr:5:f32\n",
        ] {
            let r = reply(handle(&put(more), &v, &s));
            assert_eq!(r.status, 422, "{more:?}");
            assert!(
                String::from_utf8_lossy(&r.body).contains("one Modbus TCP block"),
                "{more:?}"
            );
            // The same map is fine on a board that runs it.
            assert!(matches!(
                handle(&put(more), &sample_view(), &s),
                Outcome::Effect(Effect::SaveIoMap { .. })
            ));
            let form = format!(
                "map={}&auth=tok",
                more.replace(' ', "+")
                    .replace('\n', "%0A")
                    .replace(':', "%3A")
                    .replace('=', "%3D")
            );
            let posted = req(&format!(
                "POST /io_map HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}",
                form.len()
            ));
            assert_eq!(reply(handle(&posted, &v, &s)).status, 422, "the form: {more:?}");
        }
    }

    #[test]
    fn the_map_is_saved_with_the_token_and_a_bad_one_names_its_line() {
        let v = sample_view();
        let s = settings();
        let map = "device rio modbus-tcp 192.168.3.60\nin estop bool rio di:0\nout motor bool rio coil:0\n";
        let put = |auth: &str, ctype: &str, body: &str| {
            req(&format!(
                "PUT /api/io_map HTTP/1.1\r\n{auth}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ))
        };
        assert_eq!(reply(handle(&put("", "text/plain", map), &v, &s)).status, 401);
        let auth = "Authorization: Bearer tok\r\n";
        assert_eq!(
            handle(&put(auth, "text/plain", map), &v, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: map.into(),
                page: false
            })
        );
        let json = serde_json::json!({ "text": map }).to_string();
        assert_eq!(
            handle(&put(auth, "application/json", &json), &v, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: map.into(),
                page: false
            })
        );
        let bad = reply(handle(
            &put(auth, "text/plain", "device rio enip 192.168.3.60\n"),
            &v,
            &s,
        ));
        assert_eq!(bad.status, 422);
        let text = String::from_utf8_lossy(&bad.body).into_owned();
        assert!(text.contains("line 1"), "{text}");
        assert_eq!(
            reply(handle(&put(auth, "application/json", "{}"), &v, &s)).status,
            400
        );
        // The page's form: the token as `auth`, CRLF from the browser normalised.
        let form = format!(
            "auth=tok&map={}",
            "device%20rio%20modbus-tcp%2010.0.0.9%0D%0Ain%20x%20bool%20rio%20di%3A1%0D%0A"
        );
        let r = req(&format!(
            "POST /io_map HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}",
            form.len()
        ));
        assert_eq!(
            handle(&r, &v, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: "device rio modbus-tcp 10.0.0.9\nin x bool rio di:1\n".into(),
                page: true
            })
        );
        let no_auth = format!("map={}", "device%20rio%20modbus-tcp%2010.0.0.9%0A");
        let r = req(&format!(
            "POST /io_map HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{no_auth}",
            no_auth.len()
        ));
        let page = reply(handle(&r, &v, &s));
        assert_eq!(page.status, 401);
        assert!(String::from_utf8_lossy(&page.body).contains("map not saved"));
        assert_eq!(io_map_saved(false, true).status, 200);
        assert_eq!(io_map_saved(true, true).status, 303);
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

    const SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn fw_post(auth: &str, path: &str, body: &str) -> Request {
        req(&format!(
            "POST {path} HTTP/1.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    #[test]
    fn firmware_status_is_open_and_shaped() {
        let v = sample_view();
        let r = reply(handle(
            &req("GET /api/firmware HTTP/1.1\r\n\r\n"),
            &v,
            &settings(),
        ));
        assert_eq!(r.status, 200);
        let j: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(j["chip"], "esp32p4");
        assert_eq!(j["slot"], 0);
        assert_eq!(j["trial"]["state"], "confirmed");
        assert_eq!(j["fetch"]["state"], "idle");
        let mut v = sample_view();
        v.firmware = None;
        let r = reply(handle(
            &req("GET /api/firmware HTTP/1.1\r\n\r\n"),
            &v,
            &settings(),
        ));
        assert_eq!(r.status, 404, "a board with one slot has no firmware API");
    }

    #[test]
    fn a_firmware_write_without_the_right_token_changes_nothing() {
        let v = sample_view();
        let s = settings();
        let body = format!(r#"{{"url":"http://10.0.0.9:8000/n.bin","sha256":"{SHA}"}}"#);
        for path in ["/api/firmware", "/api/firmware/confirm"] {
            for (auth, want) in [
                ("", 401),
                ("Authorization: Bearer nope\r\n", 401),
                ("Authorization: Bearer toK\r\n", 401),
                ("Authorization: Bearer tokx\r\n", 401),
                ("Authorization: Bearer \r\n", 401),
                ("Authorization: Basic dG9rOg==\r\n", 401),
            ] {
                let r = reply(handle(&fw_post(auth, path, &body), &v, &s));
                assert_eq!(r.status, want, "{path} with {auth:?}");
            }
            // A node with no token set refuses every write.
            let mut none = s.clone();
            none.token.clear();
            let ok = "Authorization: Bearer tok\r\n";
            assert_eq!(reply(handle(&fw_post(ok, path, &body), &v, &none)).status, 403);
            // The token in the URL or the body is no token.
            let r = reply(handle(&fw_post("", &format!("{path}?token=tok"), &body), &v, &s));
            assert!(r.status == 401 || r.status == 404, "{path}");
        }
        // A bad request is refused the same way before its body is read.
        let r = reply(handle(&fw_post("", "/api/firmware", "not json"), &v, &s));
        assert_eq!(
            r.status, 401,
            "an unauthenticated caller learns nothing about its body"
        );
    }

    #[test]
    fn a_firmware_fetch_with_the_token() {
        let v = sample_view();
        let s = settings();
        let auth = "Authorization: Bearer tok\r\n";
        let body = format!(r#"{{"url":"http://10.0.0.9:8000/n.bin","sha256":"{SHA}"}}"#);
        match handle(&fw_post(auth, "/api/firmware", &body), &v, &s) {
            Outcome::Effect(Effect::FirmwareFetch { fetch }) => {
                assert_eq!(fetch.host, [10, 0, 0, 9]);
                assert_eq!(fetch.port, 8000);
                assert_eq!(fetch.path, "/n.bin");
                let r = firmware_accepted(&fetch);
                assert_eq!(r.status, 202);
                assert!(String::from_utf8_lossy(&r.body).contains(SHA));
            }
            other => panic!("{other:?}"),
        }
        for bad in [
            "not json".to_string(),
            "{}".to_string(),
            format!(r#"{{"url":"https://10.0.0.9/n.bin","sha256":"{SHA}"}}"#),
            format!(r#"{{"url":"http://example.com/n.bin","sha256":"{SHA}"}}"#),
            r#"{"url":"http://10.0.0.9/n.bin","sha256":"abc"}"#.to_string(),
        ] {
            let r = reply(handle(&fw_post(auth, "/api/firmware", &bad), &v, &s));
            assert_eq!(r.status, 422, "{bad}");
        }
        // One at a time.
        let mut busy = sample_view();
        busy.firmware.as_mut().unwrap().fetch = crate::ota::status::Fetch::Fetching {
            received: 10,
            size: 100,
        };
        assert_eq!(
            reply(handle(&fw_post(auth, "/api/firmware", &body), &busy, &s)).status,
            409
        );
        busy.firmware.as_mut().unwrap().fetch = crate::ota::status::Fetch::Restarting { sha256: [0; 32] };
        assert_eq!(
            reply(handle(&fw_post(auth, "/api/firmware", &body), &busy, &s)).status,
            409
        );
        busy.firmware.as_mut().unwrap().fetch = crate::ota::status::Fetch::Failed("x".into());
        assert!(matches!(
            handle(&fw_post(auth, "/api/firmware", &body), &busy, &s),
            Outcome::Effect(Effect::FirmwareFetch { .. })
        ));
        // Confirm.
        assert_eq!(
            handle(&fw_post(auth, "/api/firmware/confirm", ""), &v, &s),
            Outcome::Effect(Effect::FirmwareConfirm)
        );
        let mut one_slot = sample_view();
        one_slot.firmware = None;
        assert_eq!(
            reply(handle(&fw_post(auth, "/api/firmware", &body), &one_slot, &s)).status,
            404
        );
        assert_eq!(
            reply(handle(&fw_post(auth, "/api/firmware/confirm", ""), &one_slot, &s)).status,
            404
        );
        assert_eq!(firmware_confirmed(true).status, 200);
        assert_eq!(firmware_not_started("x").status, 409);
        // Only POST writes.
        let r = reply(handle(
            &req(&format!("PUT /api/firmware HTTP/1.1\r\n{auth}\r\n")),
            &v,
            &s,
        ));
        assert_eq!(r.status, 405);
        let r = reply(handle(
            &req(&format!("GET /api/firmware/confirm HTTP/1.1\r\n{auth}\r\n")),
            &v,
            &s,
        ));
        assert_eq!(r.status, 404);
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
