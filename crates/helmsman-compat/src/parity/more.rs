//! The rest of the box: proposals, history, bundles, audit, forces, trends, layouts, drivers, the site view and MCP.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value as Json, json};

use super::{Ctx, Journey, deploy_project, ensure_running, expect, module_names, wait_for};
use crate::client;

/// This area's journeys; `None` is pending.
pub const JOURNEYS: &[(&str, Option<Journey>)] = &[
    ("proposals", Some(proposals)),
    ("history", Some(history)),
    ("bundles", Some(bundles)),
    ("audit", Some(audit)),
    ("forces", Some(forces)),
    ("sim-points", Some(sim_points)),
    ("trends", Some(trends)),
    ("layout", Some(layout)),
    ("drivers", Some(drivers)),
    ("site", Some(site)),
    ("mcp", Some(mcp)),
];

/// A small WASM driver the box carries as a fixture (the Modbus TCP one
/// the box itself ships).
const DRIVER: &[u8] = include_bytes!("../../../helmsman-io/guests/modbus-tcp.wasm");

fn text<'a>(j: &'a Json, key: &str, what: &str) -> Result<&'a str, String> {
    j[key]
        .as_str()
        .ok_or_else(|| format!("{what}: no `{key}` text in {j}"))
}

/// The head snapshot's id.
fn head(c: &Ctx) -> Result<String, String> {
    let j = expect(c, "GET", "/api/snapshots/head", None, 200)?;
    Ok(text(&j, "id", "GET /api/snapshots/head")?.to_string())
}

/// The id of the snapshot the box runs.
fn running_snapshot(c: &Ctx) -> Result<String, String> {
    let j = expect(c, "GET", "/api/snapshots", None, 200)?;
    Ok(text(&j, "running", "GET /api/snapshots")?.to_string())
}

fn counter_source(c: &Ctx) -> Result<String, String> {
    let m = expect(c, "GET", "/api/modules/counter", None, 200)?;
    Ok(text(&m, "source", "GET /api/modules/counter")?.to_string())
}

fn proposals(c: &Ctx) -> Result<String, String> {
    deploy_project(c, "parity-prop")?;
    let m = expect(c, "GET", "/api/modules/counter", None, 200)?;
    let manifest = m["manifest"]["text"]
        .as_str()
        .ok_or_else(|| format!("GET /api/modules/counter: no manifest text in {m}"))?;
    let source = text(&m, "source", "GET /api/modules/counter")?;
    if !source.contains("n := n + 1") {
        return Err(format!("the counter's source is not what was deployed: {source}"));
    }
    let draft = json!({
        "kind": "module",
        "manifest": manifest,
        "source": source.replace("n := n + 1", "n := n + 7"),
    });
    let checked = expect(c, "POST", "/api/proposals/check", Some(&draft), 200)?;
    if checked["kind"] != "module" || checked["diff"]["empty"] != false {
        return Err(format!("check of the draft: {checked}"));
    }
    let made = expect(c, "POST", "/api/proposals", Some(&draft), 201)?;
    let id = made["id"]
        .as_u64()
        .ok_or_else(|| format!("POST /api/proposals: no id in {made}"))?;
    if made["approval"]["tier"].as_str().is_none() {
        return Err(format!("POST /api/proposals: no approval tier in {made}"));
    }
    let one = expect(c, "GET", &format!("/api/proposals/{id}"), None, 200)?;
    if one["id"] != id {
        return Err(format!("GET /api/proposals/{id}: {one}"));
    }
    let list = expect(c, "GET", "/api/proposals", None, 200)?;
    if !list.as_array().is_some_and(|l| l.iter().any(|p| p["id"] == id)) {
        return Err(format!("GET /api/proposals does not list {id}: {list}"));
    }
    let r = expect(
        c,
        "POST",
        &format!("/api/proposals/{id}/replay"),
        Some(&json!({})),
        201,
    )?;
    let n = r["replay"]
        .as_u64()
        .ok_or_else(|| format!("replay: no number in {r}"))?;
    let applied = expect(
        c,
        "POST",
        &format!("/api/proposals/{id}/apply"),
        Some(&json!({ "replay": n })),
        200,
    )?;
    if applied["applied"] != true {
        return Err(format!("apply: {applied}"));
    }
    wait_for("the applied draft runs", || {
        Ok(counter_source(c)?.contains("n := n + 7"))
    })?;
    Ok(format!(
        "draft {id} checked, stored, replayed ({n}), applied; the counter runs it"
    ))
}

fn history(c: &Ctx) -> Result<String, String> {
    deploy_project(c, "parity-hist-a")?;
    let first = head(c)?;
    deploy_project(c, "parity-hist-b")?;
    let second = head(c)?;
    if first == second {
        return Err(format!("two deploys left one snapshot, {first}"));
    }
    let list = expect(c, "GET", "/api/snapshots", None, 200)?;
    let ids: Vec<&str> = list["log"]
        .as_array()
        .ok_or_else(|| format!("GET /api/snapshots: no log in {list}"))?
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    if !ids.contains(&first.as_str()) || !ids.contains(&second.as_str()) {
        return Err(format!(
            "GET /api/snapshots lists {ids:?}, wanted {first} and {second}"
        ));
    }
    let snap = expect(c, "GET", &format!("/api/snapshots/{first}"), None, 200)?;
    if snap["modules"]["counter"].is_null() {
        return Err(format!("GET /api/snapshots/{first}: no counter in {snap}"));
    }
    let diff = expect(
        c,
        "GET",
        &format!("/api/snapshots/{first}/diff/{second}"),
        None,
        200,
    )?;
    if diff["empty"] != false || diff["modules"].as_array().is_none_or(Vec::is_empty) {
        return Err(format!("diff {first}..{second}: {diff}"));
    }
    let rep = expect(
        c,
        "POST",
        &format!("/api/snapshots/{first}/replay"),
        Some(&json!({})),
        201,
    )?;
    let n = rep["replay"]
        .as_u64()
        .ok_or_else(|| format!("rollback replay: no number in {rep}"))?;
    let kept = expect(c, "GET", &format!("/api/replays/{n}"), None, 200)?;
    if kept["replay"] != n {
        return Err(format!("GET /api/replays/{n}: {kept}"));
    }
    let back = expect(
        c,
        "POST",
        &format!("/api/snapshots/{first}/rollback"),
        Some(&json!({ "replay": n })),
        200,
    )?;
    if back["applied"] != true {
        return Err(format!("rollback: {back}"));
    }
    wait_for("the first snapshot runs again", || {
        Ok(running_snapshot(c)? == first)
    })?;
    Ok(format!(
        "two deploys listed, diffed, rolled back to the first ({n}), which runs"
    ))
}

fn bundles(c: &Ctx) -> Result<String, String> {
    deploy_project(c, "parity-bundle")?;
    let id = head(c)?;
    let (st, file) = client::get_bytes(&c.url, &format!("/api/snapshots/{id}/bundle"), Some(&c.session))?;
    if st != 200 || !file.starts_with(b"helmsman-bundle") {
        return Err(format!("GET the bundle of {id}: {st}, {} bytes", file.len()));
    }
    // A test box trusts no publisher and takes no dev bundles, so its own
    // unsigned export is refused: that refusal is the shape checked.
    let (st, j) = client::call_raw(
        &c.url,
        "POST",
        "/api/bundles",
        Some(&c.session),
        "application/octet-stream",
        &file,
    )?;
    if st != 403 || j["refused"] != "unsigned" {
        return Err(format!("POST /api/bundles with an unsigned bundle: {st} {j}"));
    }
    // The marketplace is a public site: a local index is refused before any
    // request is made, so no network is needed.
    let index = json!({ "index": "http://127.0.0.1:1/index" });
    for path in ["/api/market/list", "/api/market/install"] {
        let body = if path.ends_with("install") {
            json!({ "index": index["index"], "name": "parity" })
        } else {
            index.clone()
        };
        let (st, j) = client::call(&c.url, "POST", path, Some(&c.session), Some(&body))?;
        if st != 422 || j["error"].as_str().is_none() {
            return Err(format!("POST {path} with a local index: {st} {j}"));
        }
    }
    Ok(format!(
        "bundle of {} read ({} bytes), unsigned import refused, local marketplace refused",
        &id[..12],
        file.len()
    ))
}

fn audit(c: &Ctx) -> Result<String, String> {
    let name = "parity-audit";
    expect(c, "POST", "/api/projects", Some(&json!({ "name": name })), 201)?;
    let all = expect(c, "GET", "/api/audit", None, 200)?;
    let list = all
        .as_array()
        .ok_or_else(|| format!("GET /api/audit: not a list: {all}"))?;
    let mine = list
        .iter()
        .find(|e| {
            e["method"] == "POST"
                && e["path"] == "/api/projects"
                && e["user"] == super::USER
                && e["detail"].as_str().is_some_and(|d| d.contains(name))
        })
        .ok_or_else(|| format!("GET /api/audit has no entry for creating {name}: {all}"))?;
    let seq = mine["seq"]
        .as_u64()
        .ok_or_else(|| format!("audit entry without a seq: {mine}"))?;
    let page = expect(
        c,
        "GET",
        &format!("/api/audit?after={}&limit=1", seq - 1),
        None,
        200,
    )?;
    if page.as_array().map(|p| (p.len(), p[0]["seq"].as_u64())) != Some((1, Some(seq))) {
        return Err(format!("GET /api/audit?after={}&limit=1: {page}", seq - 1));
    }
    expect(c, "DELETE", &format!("/api/projects/{name}"), None, 200)?;
    Ok(format!(
        "{} entries; creating {name} is entry {seq}, paged by after and limit",
        list.len()
    ))
}

fn signal_value(c: &Ctx, name: &str) -> Result<Json, String> {
    Ok(expect(c, "GET", &format!("/api/signals/{name}"), None, 200)?["value"].clone())
}

fn forces(c: &Ctx) -> Result<String, String> {
    deploy_project(c, "parity-force")?;
    let result = (|| {
        let f = expect(
            c,
            "POST",
            "/api/forces",
            Some(&json!({ "signal": "start", "value": true })),
            201,
        )?;
        let id = f["id"]
            .as_u64()
            .ok_or_else(|| format!("POST /api/forces: no id in {f}"))?;
        wait_for("lamp follows the forced start", || {
            Ok(signal_value(c, "lamp")? == json!(true))
        })?;
        let one = expect(c, "GET", "/api/signals/start", None, 200)?;
        if one["forced"] != true {
            return Err(format!("start is not marked forced: {one}"));
        }
        let list = expect(c, "GET", "/api/forces", None, 200)?;
        if !list["forces"]
            .as_array()
            .is_some_and(|l| l.iter().any(|x| x["id"] == id))
        {
            return Err(format!("GET /api/forces does not list {id}: {list}"));
        }
        expect(c, "DELETE", &format!("/api/forces/{id}"), None, 200)?;
        wait_for("lamp falls once the force is released", || {
            Ok(signal_value(c, "lamp")? == json!(false))
        })?;
        Ok(format!(
            "start forced: lamp followed, released: lamp fell (force {id})"
        ))
    })();
    // Whatever happened, leave no force on the box.
    expect(c, "DELETE", "/api/forces", None, 200)?;
    result
}

/// A program and a map of placeholder points only: no device behind any of them.
const SIM_PROGRAM: &str = "PROGRAM simline\nVAR_INPUT sw : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n  lamp := sw;\nEND_PROGRAM\n";
const SIM_MAP: &str = "in sw bool sim\nout lamp bool sim\n";

fn sim_points(c: &Ctx) -> Result<String, String> {
    ensure_running(c)?;
    let name = "parity-sim";
    expect(c, "POST", "/api/projects", Some(&json!({ "name": name })), 201)?;
    for (f, text) in [("main.st", SIM_PROGRAM), ("io.map", SIM_MAP)] {
        expect(
            c,
            "PUT",
            &format!("/api/projects/{name}/files/{f}"),
            Some(&json!({ "text": text })),
            200,
        )?;
    }
    let check = expect(
        c,
        "POST",
        &format!("/api/projects/{name}/check"),
        Some(&json!({})),
        200,
    )?;
    if check["ok"] != true {
        return Err(format!("check: {check}"));
    }
    let r = expect(
        c,
        "POST",
        &format!("/api/projects/{name}/replay"),
        Some(&json!({})),
        201,
    )?;
    let n = r["replay"]
        .as_u64()
        .ok_or_else(|| format!("replay: no number in {r}"))?;
    expect(
        c,
        "POST",
        &format!("/api/projects/{name}/deploy"),
        Some(&json!({ "replay": n })),
        200,
    )?;
    wait_for(
        "the sim line running",
        || Ok(module_names(c)?.contains("simline")),
    )?;
    let one = expect(c, "GET", "/api/signals/sw", None, 200)?;
    if one["sim"] != true {
        return Err(format!("sw is not marked sim: {one}"));
    }
    let set = expect(c, "PUT", "/api/signals/sw", Some(&json!({ "value": true })), 200)?;
    if set != json!({ "name": "sw", "value": true }) {
        return Err(format!("PUT /api/signals/sw: {set}"));
    }
    wait_for("lamp follows the set sw", || {
        Ok(signal_value(c, "lamp")? == json!(true))
    })?;
    expect(c, "PUT", "/api/signals/sw", Some(&json!({ "value": false })), 200)?;
    wait_for("lamp falls with sw", || {
        Ok(signal_value(c, "lamp")? == json!(false))
    })?;
    expect(
        c,
        "PUT",
        "/api/signals/lamp",
        Some(&json!({ "value": true })),
        409,
    )?;
    Ok("sw set true: lamp followed, set false: lamp fell; an output is refused".into())
}

fn trends(c: &Ctx) -> Result<String, String> {
    ensure_running(c)?;
    let signals = expect(c, "GET", "/api/signals", None, 200)?;
    let name = signals
        .as_array()
        .or_else(|| signals["signals"].as_array())
        .and_then(|l| l.first())
        .and_then(|s| s["name"].as_str())
        .ok_or_else(|| format!("GET /api/signals: no signal with a name: {signals}"))?
        .to_string();
    let put = expect(
        c,
        "PUT",
        "/api/trends/pins",
        Some(&json!({ "signals": [name] })),
        200,
    )?;
    if put["kept"] != true {
        return Err(format!("PUT /api/trends/pins: {put}"));
    }
    let pins = expect(c, "GET", "/api/trends/pins", None, 200)?;
    if pins["pins"] != json!([name]) {
        return Err(format!("GET /api/trends/pins after pinning {name}: {pins}"));
    }
    let t = expect(c, "GET", &format!("/api/trends?signals={name}"), None, 200)?;
    if t["signals"][0]["name"] != name.as_str()
        || !t["signals"][0]["points"].is_array()
        || t["window_ms"].as_u64().is_none()
    {
        return Err(format!("GET /api/trends?signals={name}: {t}"));
    }
    expect(c, "PUT", "/api/trends/pins", Some(&json!({ "signals": [] })), 200)?;
    Ok(format!(
        "{name} pinned and read back, its trend answered, unpinned"
    ))
}

fn layout(c: &Ctx) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let at = u64::try_from(now).map_err(|e| e.to_string())?;
    let put = expect(
        c,
        "PUT",
        "/api/desk",
        Some(&json!({ "saved_at": at, "agent": "parity" })),
        200,
    )?;
    if put["kept"] != true {
        return Err(format!("PUT /api/desk: {put}"));
    }
    let desk = expect(c, "GET", "/api/desk", None, 200)?;
    if desk["desk"]["saved_at"] != at || desk["desk"]["agent"] != "parity" {
        return Err(format!("GET /api/desk after the put: {desk}"));
    }
    let name = "parity-wall";
    let pane = json!({
        "name": name,
        "left": ["devices", "files", "claude", "code"],
        "right": ["io"],
        "hidden": ["history"],
        "left_shows": "devices",
        "right_shows": "io",
        "left_open": true,
        "right_open": true,
        "bottom_open": false,
        "bottom_shows": "messages",
        "left_width": 400.0,
    });
    expect(c, "PUT", "/api/pane-layouts", Some(&pane), 200)?;
    let list = expect(c, "GET", "/api/pane-layouts", None, 200)?;
    let kept = list["layouts"]
        .as_array()
        .and_then(|l| l.iter().find(|x| x["name"] == name))
        .ok_or_else(|| format!("GET /api/pane-layouts does not list {name}: {list}"))?;
    if kept["left_shows"] != "devices" || kept["right"] != json!(["io"]) {
        return Err(format!("the layout came back changed: {kept}"));
    }
    expect(c, "DELETE", &format!("/api/pane-layouts/{name}"), None, 200)?;
    let after = expect(c, "GET", "/api/pane-layouts", None, 200)?;
    if after.to_string().contains(name) {
        return Err(format!("{name} is still listed after the delete: {after}"));
    }
    Ok("desk put and read back; pane layout put, read back, deleted".into())
}

fn drivers(c: &Ctx) -> Result<String, String> {
    let name = "parity-modbus";
    let (st, put) = client::call_raw(
        &c.url,
        "PUT",
        &format!("/api/drivers/{name}"),
        Some(&c.session),
        "application/wasm",
        DRIVER,
    )?;
    if st != 201 || put["ok"] != true {
        return Err(format!("PUT /api/drivers/{name}: {st} {put}"));
    }
    let list = expect(c, "GET", "/api/drivers", None, 200)?;
    let listed = list["drivers"]
        .as_array()
        .and_then(|l| l.iter().find(|d| d["name"] == name))
        .ok_or_else(|| format!("GET /api/drivers does not list {name}: {list}"))?;
    if listed["bytes"] != DRIVER.len() || listed["ok"] != true {
        return Err(format!("the driver as listed: {listed}"));
    }
    expect(c, "DELETE", &format!("/api/drivers/{name}"), None, 200)?;
    let after = expect(c, "GET", "/api/drivers", None, 200)?;
    if after.to_string().contains(name) {
        return Err(format!("{name} is still listed after the delete: {after}"));
    }
    Ok(format!("{name} ({} bytes) put, listed ok, deleted", DRIVER.len()))
}

fn site(c: &Ctx) -> Result<String, String> {
    // The box runs without --site, and the route answers without a session.
    let (st, j) = client::get(&c.url, "/api/site", None)?;
    if st != 404 || !j["error"].as_str().is_some_and(|e| e.contains("--site")) {
        return Err(format!("GET /api/site on a box without --site: {st} {j}"));
    }
    Ok("a box without --site says it is in no site".into())
}

fn mcp(c: &Ctx) -> Result<String, String> {
    const VERSION: &str = "2025-11-25";
    let post = |body: &Json, token: Option<&str>| {
        client::post_with_headers(&c.url, "/mcp", token, &[("MCP-Protocol-Version", VERSION)], body)
    };
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": VERSION,
            "capabilities": {},
            "clientInfo": { "name": "parity", "version": "1" },
        },
    });
    let (st, j) = post(&init, None)?;
    if st != 401 {
        return Err(format!("POST /mcp without the token: {st} {j}"));
    }
    let t = Some(c.token.as_str());
    let (st, j) = post(&init, t)?;
    if st != 200
        || j["result"]["serverInfo"]["name"] != "helmsman-box"
        || j["result"]["protocolVersion"] != VERSION
    {
        return Err(format!("initialize: {st} {j}"));
    }
    let (st, j) = post(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }), t)?;
    let tools = j["result"]["tools"].as_array();
    if st != 200 || !tools.is_some_and(|l| l.iter().any(|x| x["name"] == "list_modules")) {
        return Err(format!("tools/list: {st} {j}"));
    }
    let n = tools.map_or(0, Vec::len);
    let call = json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "list_modules", "arguments": {} },
    });
    let (st, j) = post(&call, t)?;
    let reply = j["result"]["content"][0]["text"].as_str().unwrap_or_default();
    if st != 200 || j["result"]["isError"] != false || !reply.contains("<box-data") {
        return Err(format!("tools/call list_modules: {st} {j}"));
    }
    Ok(format!(
        "{n} tools listed; list_modules answered inside the data envelope; no token, no answer"
    ))
}
