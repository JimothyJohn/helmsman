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
//! PUT  /api/signals/{name}        token: SetSim {"value": true | 12 | 3.5}: sets a sim input (409 for any other signal)
//! GET  /api/signals  /api/devices  /api/io_map  /api/site  /api/node  /api/settings
//! PUT  /api/settings              token: SaveSettings from JSON
//! PUT  /api/io_map                token: SaveIoMap from {"text"} or text/plain; 422 names the bad line; 409 until the check of that text passed
//! POST /api/io_map/check          token: CheckIoMap: the node asks every device of the map (202; poll the GET)
//! GET  /api/io_map/check          the last check: who answered, and whether the map may be saved
//! GET  /api/firmware              the running image, its trial and the fetch (404 on a board with one slot)
//! POST /api/firmware              token: FirmwareFetch {"url", "sha256"}; 202, the node fetches and restarts
//! POST /api/firmware/confirm      token: FirmwareConfirm, the running image works
//! OPTIONS *                       204 with the CORS headers
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde_json::json;

use crate::api::{self, NodeView, SignalValue};
use crate::http::{Request, Response, form_pairs};
use crate::iomap::{Dir, Point, Ty};
use crate::ota;
use crate::settings::Settings;

/// What the firmware must do before the reply.
#[derive(Debug, Clone, PartialEq)]
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
    /// Ask every device of the map `text` from this node
    /// ([`check_started`]); the text parsed before it got here. The
    /// result is [`crate::check::Check`], read with `GET
    /// /api/io_map/check` or on the page.
    CheckIoMap { text: String, page: bool },
    /// Set a sim input's value in the node's input image ([`sim_set`]).
    /// `point` is the map's sim `in` point; `value` is of its kind and in
    /// its range, so the image takes it as given.
    SetSim { point: Point, value: SignalValue },
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
#[derive(Debug, Clone, PartialEq)]
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
        ("POST", ["api", "io_map", "check"]) => io_map_check(req, view, settings),
        ("GET", ["api", "io_map", "check"]) => Outcome::Reply(match &view.check {
            Some(c) => Response::json(200, &c.json(view.uptime_ms)),
            None => Response::error(404, "this board saves a map without asking its devices"),
        }),
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
        ("PUT", ["api", "signals", name]) => sim_put(req, view, settings, name),
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

/// `PUT /api/signals/{name}`: `{"value": ...}`, a sim input only.
fn sim_put(req: &Request, view: &NodeView, settings: &Settings, name: &str) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let Ok(map) = crate::iomap::parse(&view.io_map) else {
        return Outcome::Reply(Response::error(500, "the node's I/O map does not parse"));
    };
    let Some(point) = map.points.iter().find(|p| p.name.eq_ignore_ascii_case(name)) else {
        // A program's own port is a signal, but not one to set.
        return Outcome::Reply(
            if view.signals.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
                not_sim_input()
            } else {
                Response::error(404, "no such signal on this node")
            },
        );
    };
    if !point.is_sim() || point.dir != Dir::In {
        return Outcome::Reply(not_sim_input());
    }
    let value = match sim_value(&req.body, point.ty) {
        Ok(v) => v,
        Err(why) => return Outcome::Reply(Response::error(400, why)),
    };
    Outcome::Effect(Effect::SetSim {
        point: point.clone(),
        value,
    })
}

fn not_sim_input() -> Response {
    Response::error(409, "only a sim input can be set")
}

/// The `value` of a body `{"value": ...}` as a point of kind `ty` holds it:
/// a bool for a bool, an integer that fits an `i32` for an int, a number
/// that is finite as an `f32` for a real.
fn sim_value(body: &[u8], ty: Ty) -> Result<SignalValue, &'static str> {
    let Ok(serde_json::Value::Object(mut obj)) = serde_json::from_slice::<serde_json::Value>(body) else {
        return Err("want {\"value\": ...}");
    };
    if obj.len() != 1 {
        return Err("want {\"value\": ...} and nothing else");
    }
    let Some(v) = obj.remove("value") else {
        return Err("want {\"value\": ...}");
    };
    match (ty, v) {
        (Ty::Bool, serde_json::Value::Bool(b)) => Ok(SignalValue::Bool(b)),
        (Ty::Bool, _) => Err("this point is bool: value is true or false"),
        (Ty::Int, serde_json::Value::Number(n)) => n
            .as_i64()
            .filter(|i| i32::try_from(*i).is_ok())
            .map(SignalValue::Int)
            .ok_or("this point is int: value is an integer from -2147483648 to 2147483647"),
        (Ty::Int, _) => Err("this point is int: value is an integer"),
        (Ty::Real, serde_json::Value::Number(n)) => n
            .as_f64()
            .map(|x| {
                #[allow(clippy::cast_possible_truncation)] // held as f32, as every real on a node is
                let held = x as f32;
                held
            })
            .filter(|x| x.is_finite())
            .map(|x| SignalValue::Real(f64::from(x)))
            .ok_or("this point is real: value is a finite number that fits 32 bits"),
        (Ty::Real, _) => Err("this point is real: value is a number"),
    }
}

/// The reply to an [`Effect::SetSim`] the firmware has stored.
#[must_use]
pub fn sim_set(name: &str, value: SignalValue) -> Response {
    let v = match value {
        SignalValue::Bool(b) => json!(b),
        SignalValue::Int(i) => json!(i),
        SignalValue::Real(x) => json!(x),
    };
    Response::json(200, &json!({ "name": name, "value": v }))
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
        // The form is one button: a text that has not passed is checked,
        // and the same text sent again once it has is saved.
        Ok(()) => match save_or_check(&text, view) {
            Ok(()) => Outcome::Effect(Effect::SaveIoMap { text, page: true }),
            Err(crate::check::Refusal::Running) => Outcome::Reply(Response::see_other("/settings")),
            Err(_) => Outcome::Effect(Effect::CheckIoMap { text, page: true }),
        },
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

/// A request's map: `{"text": "..."}` or the body as `text/plain`.
fn map_text(req: &Request) -> Result<String, Response> {
    let is_json = req
        .header("content-type")
        .is_some_and(|t| t.starts_with("application/json"));
    if is_json {
        match serde_json::from_slice::<serde_json::Value>(&req.body) {
            Ok(j) => match j["text"].as_str() {
                Some(t) => Ok(String::from(t)),
                None => Err(Response::error(400, "want {\"text\": \"...\"}")),
            },
            Err(_) => Err(Response::error(400, "body is not JSON")),
        }
    } else {
        match core::str::from_utf8(&req.body) {
            Ok(t) => Ok(String::from(t)),
            Err(_) => Err(Response::error(400, "the map is not UTF-8")),
        }
    }
}

/// `PUT /api/io_map`: `{"text": "..."}` or the map as `text/plain`.
fn io_map_put(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let text = match map_text(req) {
        Ok(t) => t,
        Err(r) => return Outcome::Reply(r),
    };
    if let Err(bad) = map_this_board_runs(&text, view) {
        return Outcome::Reply(Response::error(422, &alloc::format!("{bad}")));
    }
    match save_or_check(&text, view) {
        Ok(()) => Outcome::Effect(Effect::SaveIoMap { text, page: false }),
        Err(why) => Outcome::Reply(not_checked(&why, view)),
    }
}

/// Whether `text` may be saved now: always on a board that does not
/// check, else by the rule of [`crate::check::Check::allows`].
fn save_or_check(text: &str, view: &NodeView) -> Result<(), crate::check::Refusal> {
    match &view.check {
        Some(c) => c.allows(text, view.uptime_ms),
        None => Ok(()),
    }
}

/// 409: the map was not saved, why, and the check as it stands.
fn not_checked(why: &crate::check::Refusal, view: &NodeView) -> Response {
    let check = view
        .check
        .as_ref()
        .map_or(serde_json::Value::Null, |c| c.json(view.uptime_ms));
    Response::json(409, &json!({ "error": alloc::format!("{why}"), "check": check }))
}

/// `POST /api/io_map/check`: `{"text": "..."}` or the map as `text/plain`.
fn io_map_check(req: &Request, view: &NodeView, settings: &Settings) -> Outcome {
    if let Err(e) = authorize(req.bearer(), settings) {
        return Outcome::Reply(e);
    }
    let Some(check) = &view.check else {
        return Outcome::Reply(Response::error(
            404,
            "this board saves a map without asking its devices",
        ));
    };
    let text = match map_text(req) {
        Ok(t) => t,
        Err(r) => return Outcome::Reply(r),
    };
    if let Err(bad) = map_this_board_runs(&text, view) {
        return Outcome::Reply(Response::error(422, &alloc::format!("{bad}")));
    }
    if check.running {
        return Outcome::Reply(Response::error(
            409,
            "a check is already running; read GET /api/io_map/check",
        ));
    }
    Outcome::Effect(Effect::CheckIoMap { text, page: false })
}

/// The reply to a [`Effect::CheckIoMap`] the firmware has started.
#[must_use]
pub fn check_started(page: bool) -> Response {
    if page {
        Response::see_other("/settings")
    } else {
        Response::json(
            202,
            &json!({
                "checking": true,
                "next": "poll GET /api/io_map/check; when may_save is true, PUT /api/io_map with the same text",
            }),
        )
    }
}

/// The reply when the firmware could not start a check.
#[must_use]
pub fn check_not_started(why: &str) -> Response {
    Response::error(409, why)
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

    const CHECKED: &str = "device rio modbus-tcp 192.168.3.60\ndevice bench modbus-tcp 192.168.3.10:1502\nin estop bool rio di:0\n";

    /// A view of a board that checks, with the check of [`CHECKED`] in
    /// the state `verdicts` leave it (`None`: still running).
    fn checking(verdicts: Option<&[crate::check::Verdict]>, done_ms: u64, now_ms: u64) -> NodeView {
        let mut c = crate::check::Check::begin(&crate::iomap::parse(CHECKED).unwrap());
        if let Some(v) = verdicts {
            for (k, v) in v.iter().enumerate() {
                c.set(k, v.clone());
            }
            c.finish(done_ms);
        }
        let mut v = sample_view();
        v.uptime_ms = now_ms;
        v.check = Some(c);
        v
    }

    fn put_map(auth: &str, body: &str) -> Request {
        req(&format!(
            "PUT /api/io_map HTTP/1.1\r\n{auth}Content-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    fn post_check(auth: &str, body: &str) -> Request {
        req(&format!(
            "POST /api/io_map/check HTTP/1.1\r\n{auth}Content-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    fn form_map(text: &str) -> Request {
        let enc: String = text
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => char::from(b).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        let form = format!("map={enc}&auth=tok");
        req(&format!(
            "POST /io_map HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{form}",
            form.len()
        ))
    }

    const AUTH: &str = "Authorization: Bearer tok\r\n";

    /// The download is blocked until every device answered the node: a
    /// map nobody checked, one whose check is running, one a device was
    /// silent for, one checked too long ago and one that differs from
    /// the checked text by a byte are all refused with a 409 that says
    /// why and carries the check; only the passed text is handed on.
    #[test]
    fn a_map_is_saved_only_after_its_devices_answered_the_node() {
        use crate::check::{FRESH_MS, Verdict};
        let s = settings();
        let ok = Verdict::Answered { ms: 4 };
        let refused = |v: &NodeView, text: &str| {
            let r = reply(handle(&put_map(AUTH, text), v, &s));
            assert_eq!(r.status, 409, "{}", String::from_utf8_lossy(&r.body));
            serde_json::from_slice::<serde_json::Value>(&r.body).unwrap()
        };
        // Never checked.
        let mut never = sample_view();
        never.check = Some(crate::check::Check::default());
        let j = refused(&never, CHECKED);
        assert!(j["error"].as_str().unwrap().contains("has not been checked"));
        assert_eq!(j["check"]["state"], "none");
        // Running.
        let j = refused(&checking(None, 0, 10), CHECKED);
        assert!(j["error"].as_str().unwrap().contains("still running"));
        // One device silent: named.
        let silent = checking(
            Some(&[ok.clone(), Verdict::Failed("link: connect timed out".into())]),
            5,
            10,
        );
        let j = refused(&silent, CHECKED);
        assert!(
            j["error"].as_str().unwrap().contains("bench did not answer"),
            "{j}"
        );
        assert_eq!(j["check"]["devices"][1]["error"], "link: connect timed out");
        // Passed: this text, and only this text, while it is fresh.
        let passed = checking(Some(&[ok.clone(), ok.clone()]), 5, 10);
        assert_eq!(
            handle(&put_map(AUTH, CHECKED), &passed, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: CHECKED.into(),
                page: false
            })
        );
        let other = CHECKED.replace("di:0", "di:1");
        assert!(
            refused(&passed, &other)["error"]
                .as_str()
                .unwrap()
                .contains("has not been checked")
        );
        let stale = checking(Some(&[ok.clone(), ok]), 5, 5 + FRESH_MS);
        assert!(
            refused(&stale, CHECKED)["error"]
                .as_str()
                .unwrap()
                .contains("check it again")
        );
        // The token and the parser still come first.
        assert_eq!(reply(handle(&put_map("", CHECKED), &passed, &s)).status, 401);
        assert_eq!(
            reply(handle(&put_map(AUTH, "device rio enip 1.1.1.1\n"), &passed, &s)).status,
            422
        );
    }

    #[test]
    fn a_check_is_started_with_the_token_for_a_map_the_board_runs() {
        let s = settings();
        let mut idle = sample_view();
        idle.check = Some(crate::check::Check::default());
        assert_eq!(reply(handle(&post_check("", CHECKED), &idle, &s)).status, 401);
        assert_eq!(
            reply(handle(&post_check(AUTH, "nonsense here\n"), &idle, &s)).status,
            422
        );
        assert_eq!(
            handle(&post_check(AUTH, CHECKED), &idle, &s),
            Outcome::Effect(Effect::CheckIoMap {
                text: CHECKED.into(),
                page: false
            })
        );
        let json = serde_json::json!({ "text": CHECKED }).to_string();
        let as_json = req(&format!(
            "POST /api/io_map/check HTTP/1.1\r\n{AUTH}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}",
            json.len()
        ));
        assert!(matches!(
            handle(&as_json, &idle, &s),
            Outcome::Effect(Effect::CheckIoMap { page: false, .. })
        ));
        // One at a time.
        let busy = reply(handle(&post_check(AUTH, CHECKED), &checking(None, 0, 1), &s));
        assert_eq!(busy.status, 409);
        // A board that does not check has neither route, and saves as before.
        let plain = sample_view();
        assert_eq!(reply(handle(&post_check(AUTH, CHECKED), &plain, &s)).status, 404);
        assert_eq!(
            reply(handle(&req("GET /api/io_map/check HTTP/1.1\r\n\r\n"), &plain, &s)).status,
            404
        );
        assert!(matches!(
            handle(&put_map(AUTH, CHECKED), &plain, &s),
            Outcome::Effect(Effect::SaveIoMap { .. })
        ));
        assert_eq!(check_started(false).status, 202);
        assert_eq!(check_started(true).status, 303);
        assert_eq!(check_not_started("x").status, 409);
    }

    #[test]
    fn the_check_reads_without_a_token_and_the_page_shows_it() {
        use crate::check::Verdict;
        let s = settings();
        let silent = checking(
            Some(&[
                Verdict::Answered { ms: 4 },
                Verdict::Failed("<b>refused</b>".into()),
            ]),
            5,
            10,
        );
        let r = reply(handle(
            &req("GET /api/io_map/check HTTP/1.1\r\n\r\n"),
            &silent,
            &s,
        ));
        assert_eq!(r.status, 200);
        let j: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(
            (j["state"].as_str(), j["may_save"].as_bool()),
            (Some("done"), Some(false))
        );
        let page = String::from_utf8(reply(handle(&req("GET /settings HTTP/1.1\r\n\r\n"), &silent, &s)).body)
            .unwrap();
        assert!(page.contains("Device check"));
        assert!(page.contains("answered in 4 ms"));
        assert!(
            page.contains("&lt;b&gt;refused&lt;/b&gt;"),
            "a device's words are escaped"
        );
        assert!(page.contains("bench did not answer"));
        assert!(page.contains(">Check the devices</button>"));
        assert!(!page.contains("http-equiv=\"refresh\""));
        // The form holds the checked text, not the running map.
        assert!(page.contains("device bench modbus-tcp 192.168.3.10:1502"));
        let running = String::from_utf8(
            reply(handle(
                &req("GET /settings HTTP/1.1\r\n\r\n"),
                &checking(None, 0, 1),
                &s,
            ))
            .body,
        )
        .unwrap();
        assert!(running.contains("http-equiv=\"refresh\""));
        assert!(running.contains("waiting"));
        let ok = Verdict::Answered { ms: 1 };
        let passed = checking(Some(&[ok.clone(), ok]), 5, 10);
        let page = String::from_utf8(reply(handle(&req("GET /settings HTTP/1.1\r\n\r\n"), &passed, &s)).body)
            .unwrap();
        assert!(page.contains(">Save this map and restart</button>"));
        // A board that does not check keeps its old button and no check block.
        let plain =
            String::from_utf8(reply(handle(&req("GET /settings HTTP/1.1\r\n\r\n"), &sample_view(), &s)).body)
                .unwrap();
        assert!(plain.contains(">Save map and restart</button>"));
        assert!(!plain.contains("Device check"));
    }

    /// The page's one button: a text that has not passed is checked, the
    /// same text once it has passed is saved, and a press while the
    /// check runs goes back to the page.
    #[test]
    fn the_form_checks_first_and_saves_what_passed() {
        use crate::check::Verdict;
        let s = settings();
        let mut idle = sample_view();
        idle.check = Some(crate::check::Check::default());
        assert_eq!(
            handle(&form_map(CHECKED), &idle, &s),
            Outcome::Effect(Effect::CheckIoMap {
                text: CHECKED.into(),
                page: true
            })
        );
        assert_eq!(
            reply(handle(&form_map(CHECKED), &checking(None, 0, 1), &s)).status,
            303
        );
        let ok = Verdict::Answered { ms: 1 };
        let passed = checking(Some(&[ok.clone(), ok.clone()]), 5, 10);
        assert_eq!(
            handle(&form_map(CHECKED), &passed, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: CHECKED.into(),
                page: true
            })
        );
        // Edited after the pass: checked again, not saved.
        let edited = CHECKED.replace("estop", "e_stop");
        assert_eq!(
            handle(&form_map(&edited), &passed, &s),
            Outcome::Effect(Effect::CheckIoMap {
                text: edited.clone(),
                page: true
            })
        );
        // A failed check of the same text: pressed again, asked again.
        let failed = checking(Some(&[ok, Verdict::Failed("no".into())]), 5, 10);
        assert!(matches!(
            handle(&form_map(CHECKED), &failed, &s),
            Outcome::Effect(Effect::CheckIoMap { page: true, .. })
        ));
    }

    /// A map with `share` lines is saved like any other, a bad one is
    /// refused with its line, and a board that serves no Modbus says so.
    #[test]
    fn a_map_with_shares_is_saved_and_a_bad_share_is_refused_with_its_line() {
        let s = settings();
        let good = "device rio modbus-tcp 192.168.3.60\nshare speed int hr:0\nshare run bool coil:0\n";
        assert_eq!(
            handle(&put_map(AUTH, good), &sample_view(), &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: good.into(),
                page: false
            })
        );
        let bad = "device rio modbus-tcp 192.168.3.60\nshare speed int hr:0\nshare run bool hr:1\n";
        let r = reply(handle(&put_map(AUTH, bad), &sample_view(), &s));
        assert_eq!(r.status, 422);
        assert!(
            String::from_utf8_lossy(&r.body).contains("line 3"),
            "{}",
            String::from_utf8_lossy(&r.body)
        );
        let mut one = sample_view();
        one.one_block = true;
        let r = reply(handle(&put_map(AUTH, good), &one, &s));
        assert_eq!(r.status, 422);
        assert!(
            String::from_utf8_lossy(&r.body).contains("share"),
            "a share line is not for the ESP32 board"
        );
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

    const SIM_MAP: &str = "device rio modbus-tcp 192.168.3.60\nin estop bool rio di:0\nout motor bool rio coil:0\nin go bool sim\nin speed int sim\nin level real sim\nout lamp bool sim\n";

    fn sim_view() -> NodeView {
        let mut v = sample_view();
        v.io_map = SIM_MAP.into();
        v
    }

    fn put_sim(name: &str, auth: &str, body: &str) -> Request {
        req(&format!(
            "PUT /api/signals/{name} HTTP/1.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    const TOK: &str = "Authorization: Bearer tok\r\n";

    fn sim_status(name: &str, auth: &str, body: &str) -> u16 {
        reply(handle(&put_sim(name, auth, body), &sim_view(), &settings())).status
    }

    fn sim_effect(name: &str, body: &str) -> (String, SignalValue) {
        match handle(&put_sim(name, TOK, body), &sim_view(), &settings()) {
            Outcome::Effect(Effect::SetSim { point, value }) => (point.name, value),
            other => panic!("{name} {body}: {other:?}"),
        }
    }

    #[test]
    fn a_sim_input_is_set_with_the_token() {
        assert_eq!(
            sim_effect("go", r#"{"value": true}"#),
            ("go".into(), SignalValue::Bool(true))
        );
        assert_eq!(sim_effect("go", r#"{"value":false}"#).1, SignalValue::Bool(false));
        assert_eq!(sim_effect("speed", r#"{"value": 12}"#).1, SignalValue::Int(12));
        assert_eq!(
            sim_effect("speed", r#"{"value": -2147483648}"#).1,
            SignalValue::Int(i64::from(i32::MIN))
        );
        assert_eq!(
            sim_effect("speed", r#"{"value": 2147483647}"#).1,
            SignalValue::Int(i64::from(i32::MAX))
        );
        assert_eq!(sim_effect("level", r#"{"value": 3.5}"#).1, SignalValue::Real(3.5));
        assert_eq!(sim_effect("level", r#"{"value": 3}"#).1, SignalValue::Real(3.0));
        // Names are ST names: any case finds the point, and the point's own spelling is kept.
        assert_eq!(sim_effect("GO", r#"{"value": true}"#).0, "go");
        // The point holds an f32; the value the reply and the list show is that one.
        assert_eq!(
            sim_effect("level", r#"{"value": 0.1}"#).1,
            SignalValue::Real(f64::from(0.1f32))
        );
        let r = sim_set("level", SignalValue::Real(3.5));
        assert_eq!(r.status, 200);
        let j: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(j, json!({"name": "level", "value": 3.5}));
    }

    #[test]
    fn a_sim_write_without_the_right_token_changes_nothing() {
        assert_eq!(sim_status("go", "", r#"{"value": true}"#), 401);
        assert_eq!(
            sim_status("go", "Authorization: Bearer nope\r\n", r#"{"value": true}"#),
            401
        );
        assert_eq!(
            sim_status("go", "Authorization: Bearer toK\r\n", r#"{"value": true}"#),
            401
        );
        // The token comes before the name: an unknown name does not tell a stranger anything.
        assert_eq!(sim_status("nothing", "", r#"{"value": true}"#), 401);
        let mut none = settings();
        none.token.clear();
        assert_eq!(
            reply(handle(
                &put_sim("go", TOK, r#"{"value": true}"#),
                &sim_view(),
                &none
            ))
            .status,
            403
        );
    }

    #[test]
    fn only_a_sim_input_can_be_set() {
        assert_eq!(sim_status("nothing", TOK, r#"{"value": true}"#), 404);
        // A device input, a device output, a sim output: 409 and the box's words.
        for name in ["estop", "motor", "lamp"] {
            let r = reply(handle(
                &put_sim(name, TOK, r#"{"value": true}"#),
                &sim_view(),
                &settings(),
            ));
            assert_eq!(r.status, 409, "{name}");
            let j: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            assert_eq!(j["error"], "only a sim input can be set");
        }
        // A program's own port is a signal, and not one to set.
        let mut v = sim_view();
        v.signals.push(crate::api::SignalView {
            name: "counter".into(),
            dir: "out",
            value: Some(SignalValue::Int(1)),
            quality: "good",
            point_error: None,
            sim: false,
        });
        let r = reply(handle(
            &put_sim("counter", TOK, r#"{"value": 1}"#),
            &v,
            &settings(),
        ));
        assert_eq!(r.status, 409);
        // The kind is checked after the name: a bad body on a device input is still a 409.
        assert_eq!(sim_status("estop", TOK, "junk"), 409);
        // Other methods and a name with a path under it are not routes.
        let get = req("GET /api/signals/go HTTP/1.1\r\n\r\n");
        assert_eq!(reply(handle(&get, &sim_view(), &settings())).status, 404);
        let post = req(&format!(
            "POST /api/signals/go HTTP/1.1\r\n{TOK}Content-Length: 0\r\n\r\n"
        ));
        assert_eq!(reply(handle(&post, &sim_view(), &settings())).status, 405);
    }

    #[test]
    fn a_sim_value_of_the_wrong_kind_or_shape_is_refused() {
        let refused = [
            ("go", r#"{"value": 1}"#),
            ("go", r#"{"value": "true"}"#),
            ("go", r#"{"value": null}"#),
            ("speed", r#"{"value": true}"#),
            ("speed", r#"{"value": 1.5}"#),
            ("speed", r#"{"value": 2.0}"#),
            ("speed", r#"{"value": "12"}"#),
            ("speed", r#"{"value": 2147483648}"#),
            ("speed", r#"{"value": -2147483649}"#),
            ("speed", r#"{"value": 18446744073709551615}"#),
            ("speed", r#"{"value": 1e3}"#),
            ("level", r#"{"value": true}"#),
            ("level", r#"{"value": "NaN"}"#),
            ("level", r#"{"value": "inf"}"#),
            ("level", r#"{"value": null}"#),
            // NaN and infinity are not JSON; a number too big for 32 bits is not finite there.
            ("level", r#"{"value": NaN}"#),
            ("level", r#"{"value": Infinity}"#),
            ("level", r#"{"value": 1e999}"#),
            ("level", r#"{"value": 1e39}"#),
            ("level", r#"{"value": -1e39}"#),
            // Not {"value": ...}, or more than it.
            ("go", r#"{"value": true, "extra": 1}"#),
            ("go", r#"{"value": true, "value": false, "x": 1}"#),
            ("go", r#"{"val": true}"#),
            ("go", r"{}"),
            ("go", r"true"),
            ("go", r"[true]"),
            ("go", r#"{"value": true"#),
            ("go", "value=true"),
            ("go", ""),
            ("go", "\u{0}"),
        ];
        for (name, body) in refused {
            assert_eq!(sim_status(name, TOK, body), 400, "{name} {body:?}");
        }
        // The edges that are in.
        assert_eq!(
            sim_effect("level", r#"{"value": 3.4028234e38}"#).1,
            SignalValue::Real(f64::from(f32::MAX))
        );
        assert_eq!(sim_effect("speed", r#"{"value": 0}"#).1, SignalValue::Int(0));
    }

    #[test]
    fn the_signals_list_says_which_points_are_sim() {
        let mut v = sample_view();
        v.signals[1].sim = true;
        let j = api::signals(&v);
        assert_eq!(j[0]["sim"], false);
        assert_eq!(j[1]["sim"], true);
        // Nothing else in an entry changed.
        assert_eq!(j[0]["name"], "di0");
        assert_eq!(j[0]["forced"], false);
    }

    #[test]
    fn a_map_of_sim_points_alone_is_saved_through_the_check() {
        let map = "in go bool sim\nout lamp bool sim\n";
        let s = settings();
        let mut v = sample_view();
        v.check = Some(crate::check::Check::begin(&crate::iomap::parse(map).unwrap()));
        v.check.as_mut().unwrap().finish(5);
        v.uptime_ms = 10;
        assert_eq!(
            handle(&put_map(TOK, map), &v, &s),
            Outcome::Effect(Effect::SaveIoMap {
                text: map.into(),
                page: false
            })
        );
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
