//! What a node says about itself, in the box API's shapes. The firmware
//! fills a [`NodeView`] from its counters each scan; these functions
//! render it. Field names are the box's
//! (`crates/helmsman-plc/src/api/render.rs`, `mod.rs`), and the canvas's
//! tests parse both, so a node and a box read the same to it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value as Json, json};

use crate::http::html_escape;
use crate::settings::{Net, Settings};

/// Smallest, mean, largest and 99th percentile of a set of times, µs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spread {
    pub min: f64,
    pub mean: f64,
    pub max: f64,
    pub p99: f64,
}

/// A module's scan load over its newest scans.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Timing {
    pub window: u64,
    pub exec: Spread,
    pub late: Spread,
}

/// A module's state, as the box names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Starting,
    Running,
    Held,
    Stopped,
    Faulted(String),
}

/// The one module a node runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleView {
    pub name: String,
    pub scan_ms: f64,
    pub state: State,
    pub scans: u64,
    pub overruns: u64,
    /// How long ago the last overrun was, ms.
    pub last_overrun_ms_ago: Option<u64>,
    pub timing: Option<Timing>,
    pub source: String,
}

/// A signal's value: the node's are booleans and integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalValue {
    Bool(bool),
    Int(i64),
}

/// One named point as the canvas lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalView {
    pub name: String,
    /// `in` or `out`.
    pub dir: &'static str,
    pub value: Option<SignalValue>,
    /// `good`, `bad` or `stale`.
    pub quality: &'static str,
    pub point_error: Option<String>,
}

/// One fieldbus link as the canvas lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub name: String,
    /// `up`, `down` or `unknown`.
    pub state: &'static str,
    pub error: Option<String>,
    pub address: Option<String>,
}

/// Everything the node knows about itself right now.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeView {
    pub module: Option<ModuleView>,
    pub signals: Vec<SignalView>,
    pub devices: Vec<DeviceView>,
    /// The live view's publication count and time, as the box's.
    pub seq: u64,
    pub at_ms: u64,
    pub uptime_ms: u64,
    pub heap_used: usize,
    pub heap_peak: usize,
    pub heap_size: usize,
    pub mac: [u8; 6],
    pub ip: Option<[u8; 4]>,
    pub link_up: bool,
    pub version: &'static str,
    pub board: &'static str,
    /// The I/O map text, for `GET /api/io_map`.
    pub io_map: String,
}

impl NodeView {
    /// The node's id: the MAC's hex, which is stable and already public
    /// on the LAN.
    #[must_use]
    pub fn id(&self) -> String {
        let m = self.mac;
        format!(
            "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            m[0], m[1], m[2], m[3], m[4], m[5]
        )
    }
}

fn spread(s: Spread) -> Json {
    json!({ "min": s.min, "mean": s.mean, "max": s.max, "p99": s.p99 })
}

fn status(state: &State) -> Json {
    match state {
        State::Starting => json!({ "state": "starting" }),
        State::Running => json!({ "state": "running" }),
        State::Held => json!({ "state": "held" }),
        State::Stopped => json!({ "state": "stopped" }),
        State::Faulted(text) => json!({ "state": "faulted", "fault": { "text": text } }),
    }
}

/// A module's summary, as `GET /api/modules` lists it.
#[must_use]
pub fn module(m: &ModuleView) -> Json {
    json!({
        "name": m.name,
        "origin": "node",
        "scan_ms": m.scan_ms,
        "status": status(&m.state),
        "stats": {
            "scans": m.scans,
            "overruns": m.overruns,
            "held": 0,
            "last_overrun_ms_ago": m.last_overrun_ms_ago,
        },
        "timing": m.timing.map_or(Json::Null, |t| json!({
            "window": t.window,
            "exec_us": spread(t.exec),
            "late_us": spread(t.late),
        })),
        "has_manifest": false,
    })
}

/// `GET /api/modules`.
#[must_use]
pub fn modules(view: &NodeView) -> Json {
    Json::Array(view.module.iter().map(module).collect())
}

/// `GET /api/modules/{name}`: the summary with the source; a node has no
/// ports beyond its located variables and no manifest.
#[must_use]
pub fn module_detail(m: &ModuleView) -> Json {
    let mut j = module(m);
    j["ports"] = Json::Array(Vec::new());
    j["source"] = json!(m.source);
    j["manifest"] = Json::Null;
    j
}

fn value(v: Option<SignalValue>) -> Json {
    match v {
        Some(SignalValue::Bool(b)) => json!(b),
        Some(SignalValue::Int(i)) => json!(i),
        None => Json::Null,
    }
}

/// `GET /api/signals`, and the `signals` of a values event.
#[must_use]
pub fn signals(view: &NodeView) -> Json {
    Json::Array(
        view.signals
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "dir": s.dir,
                    "value": value(s.value),
                    "quality": s.quality,
                    "point_error": s.point_error,
                    "forced": false,
                })
            })
            .collect(),
    )
}

/// `GET /api/devices`.
#[must_use]
pub fn devices(view: &NodeView) -> Json {
    Json::Array(
        view.devices
            .iter()
            .map(|d| {
                let mut j = json!({ "name": d.name, "state": d.state });
                if let Some(e) = &d.error {
                    j["error"] = json!(e);
                }
                if let Some(a) = &d.address {
                    j["address"] = json!(a);
                }
                j
            })
            .collect(),
    )
}

/// `GET /api/io_map`.
#[must_use]
pub fn io_map(view: &NodeView) -> Json {
    json!({ "snapshot": "node", "text": view.io_map })
}

/// The `values` event the canvas's live view is built from.
#[must_use]
pub fn values_event(view: &NodeView) -> Json {
    json!({
        "seq": view.seq,
        "at_ms": view.at_ms,
        "signals": signals(view),
        "modules": modules(view),
        "outputs": [],
        "forces": [],
    })
}

/// `GET /api/site`: the box's shape, with no peers (a node lists none).
#[must_use]
pub fn site(view: &NodeView, settings: &Settings) -> Json {
    let (mut running, mut held, mut faulted, mut stopped) = (0, 0, 0, 0);
    if let Some(m) = &view.module {
        match m.state {
            State::Running => running += 1,
            State::Held => held += 1,
            State::Faulted(_) => faulted += 1,
            State::Stopped => stopped += 1,
            State::Starting => {}
        }
    }
    let (mut up, mut down, mut unknown) = (0, 0, 0);
    for d in &view.devices {
        match d.state {
            "up" => up += 1,
            "down" => down += 1,
            _ => unknown += 1,
        }
    }
    json!({
        "site": settings.site,
        "self": {
            "id": view.id(),
            "name": settings.name,
            "api_port": settings.api_port,
            "version": view.version,
            "kind": "node",
            "status": {
                "published": view.seq > 0,
                "modules": view.module.iter().count(),
                "running": running,
                "held": held,
                "faulted": faulted,
                "stopped": stopped,
                "devices": { "up": up, "down": down, "unknown": unknown },
            },
        },
        "peers": [],
        "max_peers": 0,
        "dropped": {},
    })
}

fn ip_text(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn net_json(net: Net) -> Json {
    match net {
        Net::Dhcp => json!({ "mode": "dhcp" }),
        Net::Static { ip, prefix, gateway } => json!({
            "mode": "static", "ip": ip_text(ip), "prefix": prefix, "gateway": ip_text(gateway),
        }),
    }
}

/// `GET /api/node`: the node itself, beyond what a box has.
#[must_use]
pub fn node(view: &NodeView, settings: &Settings) -> Json {
    let m = view.mac;
    json!({
        "id": view.id(),
        "name": settings.name,
        "site": settings.site,
        "board": view.board,
        "version": view.version,
        "uptime_ms": view.uptime_ms,
        "heap": { "used": view.heap_used, "peak": view.heap_peak, "size": view.heap_size },
        "mac": format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]),
        "ip": view.ip.map(ip_text),
        "link": if view.link_up { "up" } else { "down" },
        "net": net_json(settings.net),
        "program_bytes": view.module.as_ref().map_or(0, |m| m.source.len()),
        "log_sink": settings.log_sink.map(|(ip, port)| format!("{}:{port}", ip_text(ip))),
    })
}

/// `GET /api/settings`: the record without the token (only whether one is
/// set).
#[must_use]
pub fn settings_json(settings: &Settings) -> Json {
    json!({
        "net": net_json(settings.net),
        "name": settings.name,
        "site": settings.site,
        "token_set": !settings.token.is_empty(),
        "log_sink": settings.log_sink.map(|(ip, port)| format!("{}:{port}", ip_text(ip))),
        "api_port": settings.api_port,
    })
}

/// Why a settings body was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadSettings(pub String);

fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut n = 0;
    for part in s.trim().split('.') {
        if n == 4 {
            return None;
        }
        out[n] = part.parse().ok()?;
        n += 1;
    }
    (n == 4).then_some(out)
}

fn parse_sink(s: &str) -> Result<Option<([u8; 4], u16)>, BadSettings> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| BadSettings("log sink: want ip:port".into()))?;
    let ip = parse_ip(host).ok_or_else(|| BadSettings("log sink: not an IPv4 address".into()))?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| BadSettings("log sink: bad port".into()))?;
    Ok(Some((ip, port)))
}

/// Builds the settings a form or a JSON body asks for, field by field,
/// from `current`: a field left out keeps its value, and a blank token
/// keeps the token (a page never echoes it, so blank means "unchanged").
///
/// # Errors
/// The first field that does not parse or does not fit.
pub fn settings_from_pairs(current: &Settings, pairs: &[(String, String)]) -> Result<Settings, BadSettings> {
    let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.trim());
    let mut s = current.clone();
    if let Some(mode) = get("mode") {
        s.net = match mode {
            "dhcp" => Net::Dhcp,
            "static" => {
                let ip = parse_ip(get("ip").unwrap_or(""))
                    .ok_or_else(|| BadSettings("static: ip is not an IPv4 address".into()))?;
                let prefix = get("prefix")
                    .unwrap_or("24")
                    .parse::<u8>()
                    .ok()
                    .filter(|p| *p <= 32)
                    .ok_or_else(|| BadSettings("static: prefix must be 0 to 32".into()))?;
                let gateway = match get("gateway").filter(|g| !g.is_empty()) {
                    Some(g) => parse_ip(g)
                        .ok_or_else(|| BadSettings("static: gateway is not an IPv4 address".into()))?,
                    None => [0; 4],
                };
                Net::Static { ip, prefix, gateway }
            }
            other => return Err(BadSettings(format!("mode {other:?}: dhcp or static"))),
        };
    }
    if let Some(name) = get("name") {
        s.name = name.into();
    }
    if let Some(site) = get("site") {
        s.site = site.into();
    }
    if let Some(token) = get("token").filter(|t| !t.is_empty()) {
        s.token = token.into();
    }
    if let Some(sink) = get("log_sink") {
        s.log_sink = parse_sink(sink)?;
    }
    if let Some(port) = get("api_port").filter(|p| !p.is_empty()) {
        s.api_port = port
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| BadSettings("api_port: 1 to 65535".into()))?;
    }
    s.validate().map_err(|e| BadSettings(e.to_string()))?;
    Ok(s)
}

/// A JSON settings body (`PUT /api/settings`) as pairs: strings and
/// numbers become text, `net` flattens to `mode`, `ip`, `prefix`,
/// `gateway`, `null` means blank.
#[must_use]
pub fn settings_pairs_from_json(j: &Json) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let text = |v: &Json| -> String {
        match v {
            Json::String(s) => s.clone(),
            Json::Null => String::new(),
            other => other.to_string(),
        }
    };
    if let Some(obj) = j.as_object() {
        for (k, v) in obj {
            if k == "net" {
                if let Some(net) = v.as_object() {
                    for (nk, nv) in net {
                        pairs.push((nk.clone(), text(nv)));
                    }
                }
            } else {
                pairs.push((k.clone(), text(v)));
            }
        }
    }
    pairs
}

/// The beacon a node multicasts when it has a site: the box's six lines
/// (`crates/helmsman-plc/src/site.rs`).
#[must_use]
pub fn beacon(view: &NodeView, settings: &Settings) -> String {
    format!(
        "helmsman-site 1\nsite {}\nid {}\nname {}\napi {}\nversion {}\n",
        settings.site,
        view.id(),
        if settings.name.is_empty() {
            "node"
        } else {
            &settings.name
        },
        settings.api_port,
        view.version
    )
}

/// The node's own page: what it is, how it is, and the settings form.
/// Plain HTML, no scripts, so it reads on a phone at the panel.
#[must_use]
pub fn settings_page(view: &NodeView, settings: &Settings, notice: Option<&str>) -> String {
    let (mode_dhcp, mode_static, ip, prefix, gateway) = match settings.net {
        Net::Dhcp => ("checked", "", String::new(), String::from("24"), String::new()),
        Net::Static { ip, prefix, gateway } => {
            ("", "checked", ip_text(ip), prefix.to_string(), ip_text(gateway))
        }
    };
    let module = view.module.as_ref().map_or_else(
        || String::from("<p>No program deployed.</p>"),
        |m| {
            let state = match &m.state {
                State::Faulted(t) => format!("faulted: {}", html_escape(t)),
                State::Running => "running".into(),
                State::Held => "held".into(),
                State::Stopped => "stopped".into(),
                State::Starting => "starting".into(),
            };
            format!(
                "<p><b>{}</b> {state}, every {} ms, {} scans, {} overruns.</p>",
                html_escape(&m.name),
                m.scan_ms,
                m.scans,
                m.overruns
            )
        },
    );
    let notice = notice.map_or(String::new(), |n| {
        format!("<p class=\"notice\">{}</p>", html_escape(n))
    });
    let sink = settings
        .log_sink
        .map_or(String::new(), |(ip, port)| format!("{}:{port}", ip_text(ip)));
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{name} (Helmsman node)</title>\
<style>body{{font:18px/1.5 system-ui,sans-serif;max-width:40rem;margin:2rem auto;padding:0 1rem;color:#222}}\
label{{display:block;margin:.6rem 0 .2rem}}input[type=text]{{width:100%;font-size:18px;padding:.4rem}}\
button{{font-size:18px;padding:.5rem 1rem;margin-top:1rem}}.notice{{background:#eef;padding:.6rem}}\
code{{background:#f4f4f4;padding:0 .3rem}}</style></head><body>\
<h1>{name}</h1>\
<p>Helmsman node {version} on a {board}. Up {up} s. Heap {heap_used} of {heap_size} bytes (peak {heap_peak}). \
Link {link}{ip}.</p>\
{module}\
{notice}\
<h2>Settings</h2>\
<form method=\"post\" action=\"/settings\">\
<label>Name <input type=\"text\" name=\"name\" value=\"{name_attr}\" maxlength=\"32\"></label>\
<label>Site (blank: announce nothing) <input type=\"text\" name=\"site\" value=\"{site}\" maxlength=\"32\"></label>\
<p>Address: <label><input type=\"radio\" name=\"mode\" value=\"dhcp\" {mode_dhcp}> DHCP</label>\
<label><input type=\"radio\" name=\"mode\" value=\"static\" {mode_static}> Static</label></p>\
<label>IP <input type=\"text\" name=\"ip\" value=\"{ip_v}\"></label>\
<label>Prefix length <input type=\"text\" name=\"prefix\" value=\"{prefix}\"></label>\
<label>Gateway <input type=\"text\" name=\"gateway\" value=\"{gateway}\"></label>\
<label>Log sink (ip:port, blank for none) <input type=\"text\" name=\"log_sink\" value=\"{sink}\"></label>\
<label>API token (blank keeps the current one{token_state}) <input type=\"text\" name=\"token\" value=\"\" maxlength=\"64\" autocomplete=\"off\"></label>\
<label>API port <input type=\"text\" name=\"api_port\" value=\"{api_port}\"></label>\
<button type=\"submit\">Save and restart</button>\
</form>\
<p>The canvas reads this node at <code>http://{host}:{api_port}/api/</code>; writes need the token as <code>Authorization: Bearer</code>.</p>\
</body></html>",
        name = html_escape(if settings.name.is_empty() {
            "Unnamed node"
        } else {
            &settings.name
        }),
        name_attr = html_escape(&settings.name),
        site = html_escape(&settings.site),
        version = html_escape(view.version),
        board = html_escape(view.board),
        up = view.uptime_ms / 1000,
        heap_used = view.heap_used,
        heap_size = view.heap_size,
        heap_peak = view.heap_peak,
        link = if view.link_up { "up" } else { "down" },
        ip = view.ip.map_or(String::new(), |ip| format!(", {}", ip_text(ip))),
        module = module,
        notice = notice,
        mode_dhcp = mode_dhcp,
        mode_static = mode_static,
        ip_v = html_escape(&ip),
        prefix = prefix,
        gateway = html_escape(&gateway),
        sink = html_escape(&sink),
        token_state = if settings.token.is_empty() {
            "; none is set, so every write is refused"
        } else {
            ""
        },
        api_port = settings.api_port,
        host = view.ip.map_or_else(|| String::from("<node>"), ip_text),
    )
}

#[cfg(test)]
pub(crate) fn sample_view() -> NodeView {
    use alloc::vec;
    NodeView {
        module: Some(ModuleView {
            name: "chase".into(),
            scan_ms: 10.0,
            state: State::Running,
            scans: 1234,
            overruns: 2,
            last_overrun_ms_ago: Some(5000),
            timing: Some(Timing {
                window: 1000,
                exec: Spread {
                    min: 100.0,
                    mean: 150.0,
                    max: 400.0,
                    p99: 300.0,
                },
                late: Spread {
                    min: 0.0,
                    mean: 10.0,
                    max: 90.0,
                    p99: 60.0,
                },
            }),
            source: "PROGRAM chase\nEND_PROGRAM\n".into(),
        }),
        signals: vec![
            SignalView {
                name: "di0".into(),
                dir: "in",
                value: Some(SignalValue::Bool(true)),
                quality: "good",
                point_error: None,
            },
            SignalView {
                name: "do0".into(),
                dir: "out",
                value: Some(SignalValue::Bool(false)),
                quality: "good",
                point_error: None,
            },
        ],
        devices: vec![DeviceView {
            name: "rio".into(),
            state: "up",
            error: None,
            address: Some("192.168.3.60:502".into()),
        }],
        seq: 42,
        at_ms: 123_456,
        uptime_ms: 600_000,
        heap_used: 50_000,
        heap_peak: 60_000,
        heap_size: 112 * 1024,
        mac: [0x24, 0x4c, 0xab, 0x03, 0xcf, 0x64],
        ip: Some([10, 0, 0, 50]),
        link_up: true,
        version: "0.1.0",
        board: "ESP32-POE-ISO",
        io_map: "device rio modbus-tcp 192.168.3.60\n".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn settings() -> Settings {
        Settings {
            name: "press-4".into(),
            site: "line-a".into(),
            token: "tok".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn module_json_has_the_box_fields() {
        let v = sample_view();
        let j = modules(&v);
        let m = &j[0];
        assert_eq!(m["name"], "chase");
        assert_eq!(m["status"]["state"], "running");
        assert_eq!(m["scan_ms"], 10.0);
        assert_eq!(m["stats"]["scans"], 1234);
        assert_eq!(m["stats"]["overruns"], 2);
        assert_eq!(m["stats"]["last_overrun_ms_ago"], 5000);
        assert_eq!(m["timing"]["window"], 1000);
        assert_eq!(m["timing"]["exec_us"]["p99"], 300.0);
        assert_eq!(m["timing"]["late_us"]["max"], 90.0);
        let d = module_detail(v.module.as_ref().unwrap());
        assert_eq!(d["source"], "PROGRAM chase\nEND_PROGRAM\n");
        assert_eq!(d["ports"], json!([]));
        assert!(d["manifest"].is_null());
    }

    #[test]
    fn faulted_and_missing_modules() {
        let mut v = sample_view();
        v.module.as_mut().unwrap().state = State::Faulted("division by zero".into());
        v.module.as_mut().unwrap().timing = None;
        let m = &modules(&v)[0];
        assert_eq!(m["status"]["state"], "faulted");
        assert_eq!(m["status"]["fault"]["text"], "division by zero");
        assert!(m["timing"].is_null());
        v.module = None;
        assert_eq!(modules(&v), json!([]));
    }

    #[test]
    fn values_event_and_signals() {
        let v = sample_view();
        let e = values_event(&v);
        assert_eq!(e["seq"], 42);
        assert_eq!(e["at_ms"], 123_456);
        assert_eq!(e["signals"][0]["name"], "di0");
        assert_eq!(e["signals"][0]["value"], true);
        assert_eq!(e["signals"][0]["quality"], "good");
        assert_eq!(e["signals"][1]["dir"], "out");
        assert_eq!(e["modules"][0]["name"], "chase");
        assert_eq!(e["forces"], json!([]));
    }

    #[test]
    fn devices_site_node_and_io_map() {
        let v = sample_view();
        let d = devices(&v);
        assert_eq!(d[0]["state"], "up");
        assert_eq!(d[0]["address"], "192.168.3.60:502");
        assert!(d[0].get("error").is_none());
        let s = site(&v, &settings());
        assert_eq!(s["site"], "line-a");
        assert_eq!(s["self"]["id"], "244cab03cf64");
        assert_eq!(s["self"]["status"]["running"], 1);
        assert_eq!(s["self"]["status"]["devices"]["up"], 1);
        assert_eq!(s["peers"], json!([]));
        let n = node(&v, &settings());
        assert_eq!(n["mac"], "24:4c:ab:03:cf:64");
        assert_eq!(n["ip"], "10.0.0.50");
        assert_eq!(n["heap"]["peak"], 60_000);
        assert_eq!(n["net"]["mode"], "dhcp");
        assert_eq!(io_map(&v)["text"], "device rio modbus-tcp 192.168.3.60\n");
        let sj = settings_json(&settings());
        assert_eq!(sj["token_set"], true);
        assert!(sj.get("token").is_none());
    }

    #[test]
    fn beacon_is_the_box_format() {
        let b = beacon(&sample_view(), &settings());
        let lines: Vec<&str> = b.lines().collect();
        assert_eq!(
            lines,
            [
                "helmsman-site 1",
                "site line-a",
                "id 244cab03cf64",
                "name press-4",
                "api 8080",
                "version 0.1.0"
            ]
        );
    }

    #[test]
    fn settings_from_a_form() {
        let pairs = vec![
            ("mode".to_string(), "static".to_string()),
            ("ip".into(), "10.0.0.60".into()),
            ("prefix".into(), "24".into()),
            ("gateway".into(), "10.0.0.1".into()),
            ("name".into(), "press-5".into()),
            ("site".into(), "line-a".into()),
            ("token".into(), String::new()),
            ("log_sink".into(), "10.0.0.241:9514".into()),
            ("api_port".into(), "8080".into()),
        ];
        let s = settings_from_pairs(&settings(), &pairs).unwrap();
        assert_eq!(
            s.net,
            Net::Static {
                ip: [10, 0, 0, 60],
                prefix: 24,
                gateway: [10, 0, 0, 1]
            }
        );
        assert_eq!(s.name, "press-5");
        assert_eq!(s.token, "tok", "a blank token keeps the current one");
        assert_eq!(s.log_sink, Some(([10, 0, 0, 241], 9514)));
        let bad = vec![
            ("mode".to_string(), "static".to_string()),
            ("ip".into(), "nope".into()),
        ];
        assert!(settings_from_pairs(&settings(), &bad).is_err());
        let bad_name = vec![("name".to_string(), "has space".to_string())];
        assert!(settings_from_pairs(&settings(), &bad_name).is_err());
    }

    #[test]
    fn settings_from_json() {
        let j = json!({ "net": { "mode": "dhcp" }, "name": "n1", "token": "new", "log_sink": null, "api_port": 8081 });
        let s = settings_from_pairs(&settings(), &settings_pairs_from_json(&j)).unwrap();
        assert_eq!(s.net, Net::Dhcp);
        assert_eq!(s.name, "n1");
        assert_eq!(s.token, "new");
        assert_eq!(s.log_sink, None);
        assert_eq!(s.api_port, 8081);
    }

    #[test]
    fn the_page_escapes_and_never_echoes_the_token() {
        let mut s = settings();
        s.name = "a<b".into();
        s.token = "sup3r".into();
        let page = settings_page(&sample_view(), &s, Some("saved <ok>"));
        assert!(page.contains("a&lt;b"));
        assert!(page.contains("saved &lt;ok&gt;"));
        assert!(!page.contains("sup3r"));
        assert!(page.contains("name=\"mode\" value=\"dhcp\" checked"));
    }
}
