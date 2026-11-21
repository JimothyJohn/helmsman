//! The device journeys, on simulators only (the Moxa `E1200` sim, the `OpENer` container), never a bench device (docs/PARITY.md).
//!
//! `device-config` stays pending: the box reaches a unit's web console on
//! port 80 of the address its map names (`api::devconf::Setup::in_state_dir`
//! has no flag or variable to move it), and no unprivileged process can
//! serve port 80, so the console simulator cannot stand in for a unit
//! behind a real box. Aiming it needs one box-side setting (card
//! `parity-ship`).

use serde_json::{Value as Json, json};

use super::{COUNTER, Ctx, Journey, MAP, expect, wait_for};

/// This area's journeys; `None` is pending.
pub const JOURNEYS: &[(&str, Option<Journey>)] = &[
    ("devices", Some(devices)),
    ("device-config", None),
    ("device-profile", Some(device_profile)),
];

/// A Modbus TCP device at a loopback port nothing listens on: the link is
/// there and reports down, and no unit on any network is asked.
const DEVICE_LINE: &str = "device rio modbus-tcp 127.0.0.1:1 timeout_ms=200 poll_ms=50\n";

/// A small valid profile, as the agent would propose one.
const PROFILE: &str = "\
helmsman-device 1
vendor Parity
model Probe
kind remote-io
transport modbus-tcp unit=1
in  A bool modbus-tcp=di:0
out B bool modbus-tcp=coil:0
";

fn devices(c: &Ctx) -> Result<String, String> {
    let p = "parity-devices";
    expect(c, "POST", "/api/projects", Some(&json!({ "name": p })), 201)?;
    let map = format!("{DEVICE_LINE}{MAP}in  sensor bool rio di:0\n");
    for (file, text) in [("main.st", COUNTER), ("io.map", map.as_str())] {
        expect(
            c,
            "PUT",
            &format!("/api/projects/{p}/files/{file}"),
            Some(&json!({ "text": text })),
            200,
        )?;
    }
    let check = expect(c, "POST", &format!("/api/projects/{p}/check"), Some(&json!({})), 200)?;
    if check["ok"] != true {
        return Err(format!("check of a map naming a device: {check}"));
    }
    let r = expect(c, "POST", &format!("/api/projects/{p}/replay"), Some(&json!({})), 201)?;
    let n = r["replay"]
        .as_u64()
        .ok_or_else(|| format!("replay: no number in {r}"))?;
    expect(
        c,
        "POST",
        &format!("/api/projects/{p}/deploy"),
        Some(&json!({ "replay": n })),
        200,
    )?;
    wait_for("the project running", || {
        Ok(expect(c, "GET", "/api/running", None, 200)?["project"] == p)
    })?;

    // The link: listed by name, with a state (this one is down: nothing
    // listens at its address; the box says why in `error`).
    let mut seen = Json::Null;
    wait_for("the device listed", || {
        let list = expect(c, "GET", "/api/devices", None, 200)?;
        let found = list
            .as_array()
            .or_else(|| list["devices"].as_array())
            .and_then(|l| l.iter().find(|d| d["name"] == "rio" || d["id"] == "rio"))
            .cloned();
        Ok(found.is_some_and(|d| {
            seen = d;
            true
        }))
    })?;
    if !matches!(seen["state"].as_str(), Some("up" | "down" | "unknown")) {
        return Err(format!("GET /api/devices: rio has no link state: {seen}"));
    }

    // The running map names the device and the signal wired to it.
    let map_back = expect(c, "GET", "/api/io_map", None, 200)?;
    let text = map_back["text"].as_str().map_or_else(|| map_back.to_string(), str::to_string);
    if !text.contains("device rio") || !text.contains("sensor") {
        return Err(format!("GET /api/io_map is not the deployed map: {map_back}"));
    }

    // The adapter answers; the DHCP server is off on a box started without
    // one (404), and on otherwise (200): either way it answers in JSON.
    let adapter = expect(c, "GET", "/api/adapter", None, 200)?;
    if adapter["protocol"].is_null() {
        return Err(format!("GET /api/adapter: no protocol: {adapter}"));
    }
    let (st, dhcp) = crate::client::call(&c.url, "GET", "/api/dhcp", Some(&c.session), None)?;
    if st != 200 && st != 404 {
        return Err(format!("GET /api/dhcp: {st} {dhcp}"));
    }
    Ok(format!("rio listed ({seen}); running map names it; adapter {adapter}; dhcp {st}"))
}

fn device_profile(c: &Ctx) -> Result<String, String> {
    let p = "parity-profile";
    expect(c, "POST", "/api/projects", Some(&json!({ "name": p })), 201)?;
    let held = expect(
        c,
        "POST",
        "/api/device-profile/proposal",
        Some(&json!({ "name": "parity-probe", "text": PROFILE, "summary": "from the parity journey" })),
        201,
    )?;
    let id = held["id"]
        .as_u64()
        .ok_or_else(|| format!("POST proposal: no id in {held}"))?;
    let pending = expect(c, "GET", "/api/device-profile/proposal", None, 200)?;
    if pending["id"] != id || pending["name"] != "parity-probe" {
        return Err(format!("GET proposal is not the one held: {pending}"));
    }
    let saved = expect(
        c,
        "POST",
        "/api/device-profile/proposal/accept",
        Some(&json!({ "id": id, "project": p })),
        201,
    )?;
    if saved["file"] != "parity-probe.device" {
        return Err(format!("accept: wrong file in {saved}"));
    }
    let file = expect(c, "GET", &format!("/api/projects/{p}/files/parity-probe.device"), None, 200)?;
    if !file.to_string().contains("model Probe") {
        return Err(format!("the saved profile is not the proposed text: {file}"));
    }
    // Publishing goes through the agent service. A box with none refuses;
    // a file that is not in the project is refused on any box, so a box
    // that has a relay is never asked to open a pull request here.
    let (st, why) = crate::client::call(
        &c.url,
        "POST",
        "/api/device-profile/publish",
        Some(&c.session),
        Some(&json!({ "project": p, "file": "parity-absent.device" })),
    )?;
    if !(400..500).contains(&st) {
        return Err(format!("publish of a file the project lacks: {st} {why}"));
    }
    Ok(format!("proposed, held, saved as parity-probe.device; publish refused ({st})"))
}
