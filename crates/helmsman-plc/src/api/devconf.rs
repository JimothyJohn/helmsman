//! Configuring a device through its own configuration protocol
//! (`helmsman-devconf`): the box API's side of docs/ARCHITECTURE.md
//! "Device configuration". Contract in `tests/devconf.rs`.
//!
//! ```text
//! GET    /api/devices/{id}/config            facts, the unit's export, the saved configuration and the drift
//! PUT    /api/devices/{id}/config/password   engineer: {password}: kept for this device, write-only
//! DELETE /api/devices/{id}/config/password   engineer: forget it
//! POST   /api/devices/{id}/config/set        engineer: {page, changes}: staged on the unit
//! POST   /api/devices/{id}/config/restart    engineer: {interrupts?}: Save/Restart, waits for the unit
//! POST   /api/devices/{id}/config/import     engineer: {from: "snapshot"} | {text}, overwrite_network?, interrupts?
//! POST   /api/devices/{id}/config/password   engineer: {new}: change the unit's password (the old one is the kept one)
//! POST   /api/devices/{id}/config/snapshot   engineer: {version?}: save the unit's export as ID.devconf in the running project
//! ```
//!
//! A device has these routes when it is in the **running map**, reached by
//! a profile (`device:NAME`) whose `configure` line names a configurator;
//! anything else is 404. Reads need what `GET /api/devices` needs; every
//! write needs an engineer login session (the box token is 403). One
//! operation per device at a time, reads included: a second one is 409.
//! A restart or an import takes the unit off the network for about 21 s,
//! so one that would interrupt a module that reads or drives the device is
//! 409 unless the request says `interrupts: true`; while it runs the
//! device's link is listed lost with the reason "restarting for its
//! configuration" (`Devices::hold`).
//!
//! **Passwords.** A device's console password is kept in
//! `<state-dir>/devconf/ID.password` (directory 0700, file 0600, written
//! atomically), held in memory only as [`crate::sink::Secret`] (whose
//! `Debug` prints nothing). It is set by `PUT`, never returned by any
//! route, and is in no project, snapshot, export, log line, audit entry,
//! event or error: the configurator's errors never carry one, and the
//! one place it is read is [`State::connect`], which hands it to the
//! client. A change through the box updates the kept one.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use helmsman_devconf::moxa_e1200 as moxa;
use helmsman_io::profile::Profile;
use helmsman_io::profile_driver::{file_name as profile_file, profile_name};
use serde_json::{Value as Json, json};

use super::projects::{by, engineer, json_body};
use super::{Ctx, Devices, Request, Response, audited, clean};
use crate::sink::Secret;

/// The longest console password the box keeps (the login page's field).
const MAX_PASSWORD: usize = 24;
/// The reason a device's link gives while the box restarts it.
pub const RESTARTING: &str = "restarting for its configuration";

/// How the box reaches consoles; the defaults are a real unit's. Tests
/// aim it at the simulator.
#[derive(Debug, Clone)]
pub struct Setup {
    /// Where the kept passwords live (`<state-dir>/devconf`).
    pub dir: PathBuf,
    /// The console's TCP port (80).
    pub http_port: u16,
    /// Timings and the port that says the unit is back
    /// ([`moxa::Config::modbus_port`]); its `password` is ignored (the
    /// kept one is used).
    pub console: moxa::Config,
}

impl Setup {
    /// The defaults, with the passwords under `state`.
    #[must_use]
    pub fn in_state_dir(state: &Path) -> Self {
        Self {
            dir: state.join("devconf"),
            http_port: 80,
            console: moxa::Config::default(),
        }
    }
}

/// What the routes keep: the setup and which devices are busy.
pub struct State {
    setup: Setup,
    busy: Mutex<BTreeMap<String, &'static str>>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").finish_non_exhaustive()
    }
}

/// Why a call was refused: the status, a word for programs, the text,
/// and anything else the answer carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub status: u16,
    pub code: &'static str,
    pub why: String,
    pub extra: Option<Json>,
}

impl Failure {
    fn new(status: u16, code: &'static str, why: impl Into<String>) -> Self {
        Self {
            status,
            code,
            why: why.into(),
            extra: None,
        }
    }

    /// The answer's body.
    #[must_use]
    pub fn json(&self) -> Json {
        let mut j = json!({ "error": self.why, "code": self.code });
        if let (Some(Json::Object(extra)), Some(o)) = (&self.extra, j.as_object_mut()) {
            o.extend(extra.clone());
        }
        j
    }
}

fn from_console(e: &moxa::Error) -> Failure {
    use moxa::Error as E;
    let (status, code) = match e {
        E::Refused(_) => (422, "refused"),
        E::Unreachable(_) | E::Timeout | E::Io(_) | E::NoResponse => (502, "unreachable"),
        E::Protocol(_) | E::TooLarge | E::Redirect(_) | E::Status(_) | E::Page(_) => (502, "protocol"),
        E::NotAccepted => (422, "not_accepted"),
        E::PasswordUnconfirmed => (422, "password_unconfirmed"),
        E::PasswordFailed => (422, "password_failed"),
        E::PasswordRequired => (409, "password_required"),
        E::LoginRefused => (409, "login_refused"),
        E::LoginBeforeWrite => (409, "login_before_write"),
        E::RestartNotObserved => (502, "restart_not_observed"),
        E::RestartTimeout => (502, "restart_timeout"),
    };
    Failure::new(status, code, e.to_string())
}

/// A device that can be configured: its id, profile, and where its
/// console is.
struct Target {
    id: String,
    configurator: String,
    host: String,
}

/// A device id the password store can use as a file name.
fn id_ok(id: &str) -> bool {
    (1..=48).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `host` of an address that may carry a `:port`.
fn host_of(addr: &str) -> &str {
    match addr.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') && p.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => addr,
    }
}

/// Clears a hold when dropped.
struct Held<'a> {
    devices: &'a Devices,
    id: String,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.devices.release(&self.id);
    }
}

/// Frees a device's operation slot when dropped.
struct Busy<'a> {
    state: &'a State,
    id: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.state.slots().remove(&self.id);
    }
}

impl State {
    #[must_use]
    pub fn new(setup: Setup) -> Self {
        Self {
            setup,
            busy: Mutex::new(BTreeMap::new()),
        }
    }

    fn slots(&self) -> MutexGuard<'_, BTreeMap<String, &'static str>> {
        self.busy.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One operation per device at a time.
    fn acquire(&self, id: &str, op: &'static str) -> Result<Busy<'_>, Failure> {
        let mut slots = self.slots();
        if let Some(running) = slots.get(id) {
            let mut f = Failure::new(
                409,
                "busy",
                format!("another operation on this device is running ({running}); one at a time"),
            );
            f.extra = Some(json!({ "busy": running }));
            return Err(f);
        }
        slots.insert(id.to_string(), op);
        Ok(Busy {
            state: self,
            id: id.to_string(),
        })
    }

    fn secret_path(&self, id: &str) -> PathBuf {
        self.setup.dir.join(format!("{id}.password"))
    }

    /// The kept password of device `id`, if there is one.
    fn kept(&self, id: &str) -> Option<Secret> {
        if !id_ok(id) {
            return None;
        }
        let text = fs::read_to_string(self.secret_path(id)).ok()?;
        let pw = text.strip_suffix('\n').unwrap_or(&text);
        (!pw.is_empty()).then(|| Secret::new(pw.to_string()))
    }

    /// Keeps `password` for device `id`: directory 0700, file 0600,
    /// written whole beside the old one and renamed over it.
    fn keep(&self, id: &str, password: &str) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.setup.dir)?;
        let path = self.secret_path(id);
        let tmp = path.with_extension("password.tmp");
        let _ = fs::remove_file(&tmp);
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(password.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        fs::rename(&tmp, &path)
    }

    fn forget(&self, id: &str) -> io::Result<()> {
        match fs::remove_file(self.secret_path(id)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// A console session for `t`, logged in with the kept password if the
    /// console asks for one.
    fn connect(&self, t: &Target) -> Result<moxa::Console, Failure> {
        let mut cfg = self.setup.console.clone();
        cfg.password = self.kept(&t.id).map(|s| moxa::Secret::new(s.expose()));
        moxa::Console::connect(&format!("{}:{}", t.host, self.setup.http_port), cfg)
            .map_err(|e| from_console(&e))
    }
}

/// Device `id` of the running map, with a configurator.
fn resolve(ctx: &Ctx, id: &str) -> Result<Target, Failure> {
    let none = |why: &str| Failure::new(404, "not_configurable", why);
    let map = ctx
        .running()
        .and_then(|cs| cs.io_map)
        .and_then(|t| helmsman_io::text::parse(&t).ok())
        .ok_or_else(|| Failure::new(404, "no_device", "no such device in the running map"))?
        .map;
    let d = map
        .devices
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| Failure::new(404, "no_device", "no such device in the running map"))?;
    let name = profile_name(&d.driver)
        .ok_or_else(|| none("this device is not reached by a profile, so it names no configurator"))?;
    let drivers = ctx.running_drivers();
    let bytes = drivers
        .get(&profile_file(name))
        .ok_or_else(|| none("this device's profile is not loaded"))?;
    let profile = std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| Profile::parse(t).ok())
        .ok_or_else(|| none("this device's profile does not read"))?;
    let configurator = profile
        .configure
        .ok_or_else(|| none("this device's profile has no configure line"))?;
    // Where its link found it, else the map's endpoint.
    let host = ctx
        .box_
        .devices
        .address(id)
        .map_or_else(|| host_of(&d.endpoint).to_string(), |a| host_of(&a).to_string());
    Ok(Target {
        id: id.to_string(),
        configurator,
        host,
    })
}

/// The modules of the running project that read or drive device `id`.
fn users(ctx: &Ctx, id: &str) -> Vec<String> {
    let c = ctx.catalog();
    let mine: Vec<&str> = c
        .signals
        .iter()
        .filter(|s| s.binding["kind"] == "device" && s.binding["device"] == id)
        .map(|s| s.name.as_str())
        .collect();
    c.modules
        .iter()
        .filter(|m| {
            m.ports
                .iter()
                .any(|p| mine.iter().any(|s| s.eq_ignore_ascii_case(&p.name)))
        })
        .map(|m| m.name.clone())
        .collect()
}

/// A restart or an import is refused while a module needs the device,
/// unless the caller accepts the interruption.
fn gate(ctx: &Ctx, id: &str, what: &str, interrupts: bool) -> Result<(), Failure> {
    let modules = users(ctx, id);
    if modules.is_empty() || interrupts {
        return Ok(());
    }
    let mut f = Failure::new(
        409,
        "interrupts",
        format!(
            "{what} takes {id} off the network for about 21 s, and module(s) {} read or drive it; send \"interrupts\": true to go ahead",
            modules.join(", ")
        ),
    );
    f.extra = Some(json!({ "modules": modules }));
    Err(f)
}

// ---- the read path ------------------------------------------------------

/// One value of the export with its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub value: String,
    pub label: Option<String>,
}

/// One DO channel's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub channel: String,
    pub safe: Option<Reading>,
    pub power_on: Option<Reading>,
}

/// What a person wants to know of a unit, read out of its export.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    pub firmware: Option<String>,
    pub watchdog: Option<Reading>,
    pub eip_enabled: Option<bool>,
    pub rest_enabled: Option<bool>,
    pub outputs: Vec<Output>,
}

fn reading(e: &moxa::Entry) -> Reading {
    Reading {
        value: e.value.clone(),
        label: e.label.clone(),
    }
}

impl Facts {
    /// From the export. The firmware is the first `[System Information]`
    /// entry whose key says firmware or version (the key's exact name has
    /// not been seen); the rest are keys the write-up cites.
    #[must_use]
    pub fn of(e: &moxa::Export) -> Self {
        let flag = |k: &str| e.find(k).map(|x| x.value.trim() == "1");
        let firmware = e
            .sections()
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case("System Information"))
            .and_then(|s| {
                s.entries.iter().find(|x| {
                    let k = x.key.to_ascii_lowercase();
                    k.contains("firmware") || k.contains("version")
                })
            })
            .map(|x| x.value.clone());
        let mut outputs: BTreeMap<String, Output> = BTreeMap::new();
        for x in e.sections().iter().flat_map(|s| &s.entries) {
            let Some(rest) = x.key.strip_prefix("DO") else {
                continue;
            };
            let Some((n, kind)) = rest.split_once('_') else {
                continue;
            };
            if n.len() != 2 || !n.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let out = outputs.entry(n.to_string()).or_insert_with(|| Output {
                channel: format!("DO-{n}"),
                safe: None,
                power_on: None,
            });
            match kind {
                "SAFE" => out.safe = Some(reading(x)),
                "PWN" => out.power_on = Some(reading(x)),
                _ => {}
            }
        }
        Self {
            firmware,
            watchdog: e.find("CONNECTION_WATCHDOG").map(reading),
            eip_enabled: flag("EIP_ENABLE"),
            rest_enabled: flag("REST_ENABLE"),
            outputs: outputs.into_values().collect(),
        }
    }

    #[must_use]
    pub fn json(&self) -> Json {
        let r = |x: &Option<Reading>| {
            x.as_ref()
                .map_or(Json::Null, |r| json!({ "value": r.value, "label": r.label }))
        };
        json!({
            "firmware": self.firmware,
            "watchdog": self.watchdog.as_ref().map_or(Json::Null, |w| json!({
                "enabled": w.value.trim() != "0",
                "value": w.value,
                "label": w.label,
            })),
            "eip_enabled": self.eip_enabled,
            "rest_enabled": self.rest_enabled,
            "outputs": self.outputs.iter().map(|o| json!({
                "channel": o.channel,
                "safe": r(&o.safe),
                "power_on": r(&o.power_on),
            })).collect::<Vec<_>>(),
        })
    }
}

/// The saved configuration of a device in the running project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub file: String,
    pub version: String,
    pub bytes: usize,
}

/// What the unit's configuration is, next to what was saved. **No field
/// can hold the console password**: the types here are strings read from
/// the export, the configurator's name, and flags.
#[derive(Debug, Clone)]
pub struct ConfigView {
    pub device: String,
    pub configurator: String,
    pub endpoint: String,
    pub password_stored: bool,
    pub facts: Facts,
    /// The unit's export text.
    pub export: String,
    pub snapshot: Option<SnapshotInfo>,
    /// Saved against the unit now: `before` is the saved line.
    pub drift: Option<moxa::Diff>,
}

fn diff_json(d: &moxa::Diff) -> Json {
    json!({
        "changed": d.changed.iter().map(|(b, a)| json!({ "before": b, "after": a })).collect::<Vec<_>>(),
        "added": d.added,
        "removed": d.removed,
    })
}

impl ConfigView {
    #[must_use]
    pub fn json(&self) -> Json {
        json!({
            "device": self.device,
            "configurator": self.configurator,
            "endpoint": self.endpoint,
            "password_stored": self.password_stored,
            "facts": self.facts.json(),
            "export": self.export,
            "snapshot": self.snapshot.as_ref().map(|s| json!({
                "file": s.file, "version": s.version, "bytes": s.bytes,
            })),
            "drift": self.drift.as_ref().map(diff_json),
        })
    }
}

/// The saved configuration of `id`: the running project and its text.
fn saved(ctx: &Ctx, id: &str) -> Option<(String, String)> {
    let p = ctx.projects.as_ref()?;
    let run = p.running()?;
    let file = format!("{id}.devconf");
    let text = p.file(&run.name, &file).ok()?;
    Some((file, text))
}

/// Reads device `device`'s configuration from its unit: facts, export,
/// the saved configuration and the drift from it. The one read path:
/// `GET /api/devices/{id}/config` and the agent's tools both call it.
///
/// # Errors
/// [`Failure`]: 404 for a device without a configurator, 409 while another
/// operation on it runs, 502 when the unit does not answer.
pub(super) fn config_view(ctx: &Ctx, device: &str) -> Result<ConfigView, Failure> {
    let st = ctx
        .devconf
        .as_ref()
        .ok_or_else(|| Failure::new(404, "no_devconf", "this box does not configure devices"))?;
    let t = resolve(ctx, device)?;
    let _busy = st.acquire(device, "read")?;
    let mut c = st.connect(&t)?;
    let export = c.export().map_err(|e| from_console(&e))?;
    let (snapshot, drift) = match saved(ctx, device) {
        Some((file, text)) => {
            let info = SnapshotInfo {
                version: crate::project::version(text.as_bytes()),
                bytes: text.len(),
                file,
            };
            let drift = moxa::Export::parse(&text)
                .ok()
                .map(|s| moxa::Diff::between(&s, &export));
            (Some(info), drift)
        }
        None => (None, None),
    };
    Ok(ConfigView {
        device: device.to_string(),
        configurator: t.configurator,
        endpoint: format!("{}:{}", t.host, st.setup.http_port),
        password_stored: st.kept(device).is_some(),
        facts: Facts::of(&export),
        export: export.to_text(),
        snapshot,
        drift,
    })
}

/// `GET /api/devices/{id}/config`.
pub(super) fn get(ctx: &Ctx, id: &str) -> Response {
    match config_view(ctx, id) {
        Ok(v) => Response::json(200, &v.json()),
        Err(f) => Response::json(f.status, &f.json()),
    }
}

// ---- the write path -----------------------------------------------------

fn flag(m: &serde_json::Map<String, Json>, key: &str) -> Result<bool, Failure> {
    match m.get(key) {
        None => Ok(false),
        Some(Json::Bool(b)) => Ok(*b),
        Some(_) => Err(Failure::new(400, "malformed", format!("{key} is true or false"))),
    }
}

fn only(m: &serde_json::Map<String, Json>, allowed: &[&str]) -> Result<(), Failure> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(Failure::new(
            400,
            "malformed",
            format!("unknown field {:?}; expected {}", clean(k), allowed.join(", ")),
        )),
        None => Ok(()),
    }
}

fn string<'a>(m: &'a serde_json::Map<String, Json>, key: &str) -> Result<&'a str, Failure> {
    m.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| Failure::new(400, "malformed", format!("expected {{{key}: text}}")))
}

fn body(req: &Request) -> Result<serde_json::Map<String, Json>, Failure> {
    if req.body.is_empty() && req.content_type.is_none() {
        return Ok(serde_json::Map::new());
    }
    json_body(req).map_err(|e| Failure::new(400, "malformed", e))
}

fn password_ok(p: &str, max: usize) -> Result<(), Failure> {
    if p.is_empty() || p.len() > max || !p.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err(Failure::new(
            400,
            "malformed",
            format!("a password is 1 to {max} printable ASCII characters"),
        ));
    }
    Ok(())
}

/// The non-`GET` routes under `/api/devices/{id}/config`; every outcome is
/// audited.
pub(super) fn mutate(req: &Request, ctx: &Ctx, id: &str, rest: &[&str]) -> Response {
    let what = format!("device {} config", clean(id));
    let done = |status: u16, body: &Json, detail: &str| {
        audited(ctx, req, status, body, &format!("{what}: {detail}"))
    };
    let Some(st) = &ctx.devconf else {
        return done(
            404,
            &json!({ "error": "this box does not configure devices", "code": "no_devconf" }),
            "not on this box",
        );
    };
    if let Err(r) = engineer(ctx, req, &what) {
        return r;
    }
    let started = Instant::now();
    let out = (|| -> Result<(Json, String), Failure> {
        let m = match (req.method.as_str(), rest) {
            ("DELETE", _) => serde_json::Map::new(),
            _ => body(req)?,
        };
        match (req.method.as_str(), rest) {
            ("PUT", ["password"]) => put_password(ctx, st, id, &m),
            ("DELETE", ["password"]) => {
                resolve(ctx, id)?;
                let _busy = st.acquire(id, "password")?;
                st.forget(id)
                    .map_err(|_| Failure::new(500, "store", "the box could not remove the kept password"))?;
                Ok((
                    json!({ "password_stored": false }),
                    "kept password forgotten".into(),
                ))
            }
            ("POST", ["set"]) => set(ctx, st, id, &m),
            ("POST", ["restart"]) => restart(ctx, st, id, &m, started),
            ("POST", ["import"]) => import(ctx, st, id, &m, started),
            ("POST", ["password"]) => change_password(ctx, st, id, &m),
            ("POST", ["snapshot"]) => snapshot(ctx, st, id, &m, req),
            _ => Err(Failure::new(404, "no_route", "no such resource")),
        }
    })();
    match out {
        Ok((j, detail)) => done(200, &j, &detail),
        Err(f) => done(f.status, &f.json(), &format!("refused: {}", f.code)),
    }
}

fn put_password(
    ctx: &Ctx,
    st: &State,
    id: &str,
    m: &serde_json::Map<String, Json>,
) -> Result<(Json, String), Failure> {
    only(m, &["password"])?;
    let pw = string(m, "password")?;
    password_ok(pw, MAX_PASSWORD)?;
    resolve(ctx, id)?;
    if !id_ok(id) {
        return Err(Failure::new(
            422,
            "refused",
            "this device id cannot name a kept password",
        ));
    }
    let _busy = st.acquire(id, "password")?;
    st.keep(id, pw)
        .map_err(|_| Failure::new(500, "store", "the box could not keep the password"))?;
    Ok((json!({ "password_stored": true }), "password kept".into()))
}

fn set(
    ctx: &Ctx,
    st: &State,
    id: &str,
    m: &serde_json::Map<String, Json>,
) -> Result<(Json, String), Failure> {
    only(m, &["page", "changes"])?;
    let page = string(m, "page")?;
    let changes: Vec<(String, String)> = match m.get("changes") {
        Some(Json::Object(c)) if !c.is_empty() && c.len() <= 64 => c
            .iter()
            .map(|(k, v)| {
                v.as_str().map(|v| (k.clone(), v.to_string())).ok_or_else(|| {
                    Failure::new(400, "malformed", "each change is a field name and a text value")
                })
            })
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(Failure::new(
                400,
                "malformed",
                "expected {page, changes: {field: value, ...}} with 1 to 64 changes",
            ));
        }
    };
    let t = resolve(ctx, id)?;
    let _busy = st.acquire(id, "set")?;
    let mut c = st.connect(&t)?;
    let refs: Vec<(&str, &str)> = changes.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let diff = c.set(page, &refs).map_err(|e| from_console(&e))?;
    Ok((
        json!({ "staged": true, "diff": diff_json(&diff) }),
        format!("set {} field(s) on {}", changes.len(), clean(page)),
    ))
}

fn held<'a>(ctx: &'a Ctx, id: &str) -> Held<'a> {
    ctx.box_.devices.hold(id, RESTARTING);
    Held {
        devices: &ctx.box_.devices,
        id: id.to_string(),
    }
}

fn restart(
    ctx: &Ctx,
    st: &State,
    id: &str,
    m: &serde_json::Map<String, Json>,
    started: Instant,
) -> Result<(Json, String), Failure> {
    only(m, &["interrupts"])?;
    let interrupts = flag(m, "interrupts")?;
    let t = resolve(ctx, id)?;
    gate(ctx, id, "a restart", interrupts)?;
    let _busy = st.acquire(id, "restart")?;
    let _hold = held(ctx, id);
    let mut c = st.connect(&t)?;
    c.restart().map_err(|e| from_console(&e))?;
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok((json!({ "restarted": true, "took_ms": ms }), "restarted".into()))
}

fn import(
    ctx: &Ctx,
    st: &State,
    id: &str,
    m: &serde_json::Map<String, Json>,
    started: Instant,
) -> Result<(Json, String), Failure> {
    only(m, &["from", "text", "overwrite_network", "interrupts"])?;
    let overwrite = flag(m, "overwrite_network")?;
    let interrupts = flag(m, "interrupts")?;
    let t = resolve(ctx, id)?;
    let (file, source) = match (m.get("from"), m.get("text")) {
        (Some(Json::String(f)), None) if f == "snapshot" => {
            let (_, text) = saved(ctx, id).ok_or_else(|| {
                Failure::new(
                    404,
                    "no_snapshot",
                    "no saved configuration for this device in the running project",
                )
            })?;
            (text, "the saved configuration")
        }
        (None, Some(Json::String(text))) => (text.clone(), "the text sent"),
        _ => {
            return Err(Failure::new(
                400,
                "malformed",
                "expected {from: \"snapshot\"} or {text: the export}, one of them",
            ));
        }
    };
    gate(ctx, id, "an import", interrupts)?;
    let _busy = st.acquire(id, "import")?;
    let _hold = held(ctx, id);
    let mut c = st.connect(&t)?;
    let diff = c
        .import(file.as_bytes(), overwrite)
        .map_err(|e| from_console(&e))?;
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok((
        json!({ "imported": true, "diff": diff_json(&diff), "took_ms": ms }),
        format!(
            "imported {source}{}",
            if overwrite {
                " with its network settings"
            } else {
                ""
            }
        ),
    ))
}

fn change_password(
    ctx: &Ctx,
    st: &State,
    id: &str,
    m: &serde_json::Map<String, Json>,
) -> Result<(Json, String), Failure> {
    only(m, &["new"])?;
    let new = string(m, "new")?;
    password_ok(new, 16)?;
    let t = resolve(ctx, id)?;
    let _busy = st.acquire(id, "password")?;
    let old = st.kept(id);
    let mut c = st.connect(&t)?;
    c.change_password(old.as_ref().map_or("", Secret::expose), new)
        .map_err(|e| from_console(&e))?;
    // The unit has it now: what the box keeps follows.
    st.keep(id, new).map_err(|_| {
        Failure::new(
            500,
            "store",
            "the unit's password changed, but the box could not keep the new one; set it with PUT",
        )
    })?;
    Ok((json!({ "changed": true }), "password changed".into()))
}

fn snapshot(
    ctx: &Ctx,
    st: &State,
    id: &str,
    m: &serde_json::Map<String, Json>,
    req: &Request,
) -> Result<(Json, String), Failure> {
    only(m, &["version"])?;
    let if_match = match m.get("version") {
        None => None,
        Some(Json::Null) => Some(None),
        Some(Json::String(v)) => Some(Some(v.as_str())),
        Some(_) => return Err(Failure::new(400, "malformed", "version is text or null")),
    };
    let t = resolve(ctx, id)?;
    let no_project = || {
        Failure::new(
            409,
            "no_project",
            "no project is running to hold the saved configuration",
        )
    };
    let p = ctx.projects.as_ref().ok_or_else(no_project)?;
    let run = p.running().ok_or_else(no_project)?;
    let file = format!("{id}.devconf");
    if helmsman_project::file_name(&file) != Some(helmsman_project::FileKind::Devconf) {
        return Err(Failure::new(
            422,
            "refused",
            "this device's id cannot name a project file (a-z 0-9 _ -, starting with a letter or digit)",
        ));
    }
    let _busy = st.acquire(id, "snapshot")?;
    let mut c = st.connect(&t)?;
    let text = c.export().map_err(|e| from_console(&e))?.to_text();
    let version = p.save(&run.name, &file, text.as_bytes(), if_match).map_err(|r| {
        Failure::new(
            if r.status == 409 { 409 } else { r.status },
            if r.status == 409 { "stale" } else { "refused" },
            r.why,
        )
    })?;
    let revision = p
        .record_revision(
            &run.name,
            &by(req),
            &format!("saved the configuration of {id}"),
            None,
        )
        .ok();
    Ok((
        json!({ "file": file, "version": version, "bytes": text.len(), "revision": revision }),
        format!("saved {file} in project {}", clean(&run.name)),
    ))
}
