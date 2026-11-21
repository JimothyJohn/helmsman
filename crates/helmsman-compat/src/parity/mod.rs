//! Feature parity (docs/PARITY.md): every journey docs/FEATURES.md
//! names, walked on one box, one finding per feature. `make ship` runs it
//! on the candidate (a box built at dev) and on live (a box built at
//! master, the deployed relay) and reports a feature that passes on one
//! and not the other.
//!
//! A journey is what a user does with a feature, done through the box's
//! API with the box's own client: sign in, make a project, deploy it, see
//! it run. It checks the main path and the answer's shape, not the edge
//! cases (those are each crate's own tests). A journey not written yet is
//! `pending`, which is no failure and no pass: both sides report it the
//! same, so it never shows as a parity gap, and the report counts them.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use helmsman_agent::http::Url;
use serde_json::{Value as Json, json};

use crate::boxp::{self, BoxProc};
use crate::client;
use crate::features::Manifest;
use crate::report::Finding;

/// The engineer the journeys sign in as.
pub const USER: &str = "parity";
/// Its password: long enough for the box's floor, and only ever on loopback.
pub const PASSWORD: &str = "parity-engineer-password";
/// The project the journeys make.
pub const PROJECT: &str = "parity";

/// A program on the box's own process image, so it deploys on any box:
/// it counts scans and echoes an input to an output.
const COUNTER: &str = "\
PROGRAM counter
VAR_INPUT start : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; END_VAR
VAR n : UDINT; END_VAR
  n := n + 1;
  lamp := start;
END_PROGRAM
";

/// Its I/O map: the two ports on the box's own image.
const MAP: &str = "\
in  start bool box di:0
out lamp  bool box do:0 safe=false
";

/// What a journey runs against.
pub struct Ctx {
    pub url: Url,
    /// The box's API token (`state/api-token`).
    pub token: String,
    /// An engineer's login session, for the routes that want one.
    pub session: String,
    pub bin: PathBuf,
    pub work: PathBuf,
    /// What stands behind the box's agent routes.
    pub agent: Agent,
}

/// What a journey came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass(String),
    Fail(String),
    Pending,
    /// Written, and not run on this side: the reason. Reported like
    /// [`Self::Pending`] (no finding, counted), never a pass.
    NotRun(String),
}

/// A journey: what it saw when it passed, or why it failed.
pub type Journey = fn(&Ctx) -> Result<String, String>;

mod agent;
mod devices;
mod more;

pub use agent::{Agent, AgentSide};

/// The box's own journeys (this file). Each area keeps its journeys in
/// its own file, so the areas are written apart: [`agent`] (the relay
/// routes), [`devices`] (on simulators) and [`more`] (the rest).
const BOX: &[(&str, Option<Journey>)] = &[
    ("health", Some(health)),
    ("sign-in", Some(sign_in)),
    ("live-values", Some(live_values)),
    ("module-control", Some(module_control)),
    ("projects", Some(projects)),
    ("deploy", Some(deploy)),
];

/// Every journey, by the id docs/FEATURES.md gives it; `None` is
/// pending. `every_manifest_journey_is_in_the_table_and_no_other` holds
/// the table and the manifest to each other.
pub fn journeys() -> impl Iterator<Item = &'static (&'static str, Option<Journey>)> {
    BOX.iter()
        .chain(agent::JOURNEYS)
        .chain(devices::JOURNEYS)
        .chain(more::JOURNEYS)
}

/// Walk every journey of the manifest, in its order.
#[must_use]
pub fn run(manifest: &Manifest, ctx: &Ctx) -> Vec<(String, Outcome)> {
    manifest
        .features
        .iter()
        .map(|f| {
            let outcome = match journeys().find(|(id, _)| *id == f.journey) {
                None => Outcome::Fail(format!(
                    "no journey `{}` in helmsman-compat parity::journeys()",
                    f.journey
                )),
                Some((_, None)) => Outcome::Pending,
                Some((id, Some(_))) if agent::needs_relay(id) && matches!(ctx.agent, Agent::None) => {
                    Outcome::NotRun("needs a relay (--relay-bin or --url and --token)".into())
                }
                Some((_, Some(j))) => match j(ctx) {
                    Ok(d) => Outcome::Pass(d),
                    Err(e) => Outcome::Fail(boxp::redact(&e)),
                },
            };
            (f.journey.clone(), outcome)
        })
        .collect()
}

/// The outcomes as findings (a pending or not-run journey is left out)
/// and the count of those.
#[must_use]
pub fn findings(outcomes: &[(String, Outcome)]) -> (Vec<Finding>, usize) {
    let mut pending = 0;
    let mut out = Vec::new();
    for (id, o) in outcomes {
        let check = format!("parity/{id}");
        match o {
            Outcome::Pass(d) => out.push(Finding::pass(check, d.clone())),
            Outcome::Fail(d) => out.push(Finding::fail(check, d.clone())),
            Outcome::Pending | Outcome::NotRun(_) => pending += 1,
        }
    }
    (out, pending)
}

/// Start `bin` with an engineer account and projects on, sign in, and
/// walk the manifest.
///
/// # Errors
/// The box did not start or the engineer could not sign in: nothing was
/// walked.
pub fn check(
    bin: &Path,
    manifest: &Path,
    work: &Path,
    extra: &[OsString],
) -> Result<Vec<(String, Outcome)>, String> {
    check_with(bin, manifest, work, extra, &AgentSide::None)
}

/// [`check`] with an agent side: the stub model behind a candidate
/// relay, or the deployed relay. With [`AgentSide::None`] the agent
/// journeys are not run.
///
/// # Errors
/// As [`check`], or the relay did not start.
pub fn check_with(
    bin: &Path,
    manifest: &Path,
    work: &Path,
    extra: &[OsString],
    side: &AgentSide,
) -> Result<Vec<(String, Outcome)>, String> {
    let text = std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let manifest =
        Manifest::parse(&text).map_err(|e| format!("{}:\n{}", manifest.display(), e.join("\n")))?;
    let mut args: Vec<OsString> = vec!["--projects".into()];
    args.extend(extra.iter().cloned());
    let behind = agent::start(side, work)?;
    args.extend(behind.args());
    let b = BoxProc::start_with(bin, None, work, &args, |state| {
        boxp::add_account(bin, state, USER, "engineer", PASSWORD)
    })?;
    let session = login(&b.url).map_err(|e| format!("{e}\n  box log:\n{}", b.log_tail()))?;
    let ctx = Ctx {
        url: b.url.clone(),
        token: b.token.clone(),
        session,
        bin: bin.to_path_buf(),
        work: work.to_path_buf(),
        agent: behind.agent(),
    };
    let mut out = run(&manifest, &ctx);
    // The box's log once, under the first failure: it is the same box.
    if let Some((_, Outcome::Fail(d))) = out.iter_mut().find(|(_, o)| matches!(o, Outcome::Fail(_))) {
        d.push_str("\n  box log:\n");
        d.push_str(&b.log_tail());
    }
    Ok(out)
}

/// Sign in as the journeys' engineer: the session token.
pub(crate) fn login(url: &Url) -> Result<String, String> {
    let (st, j) = client::call(
        url,
        "POST",
        "/api/login",
        None,
        Some(&json!({ "user": USER, "password": PASSWORD })),
    )?;
    if st != 200 {
        return Err(format!("POST /api/login as {USER}: {st} {j}"));
    }
    j["token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("POST /api/login: no token in {j}"))
}

// ---- helpers ---------------------------------------------------------------

/// One request with the engineer's session; the status must be `want`.
pub(crate) fn expect(
    c: &Ctx,
    method: &str,
    path: &str,
    body: Option<&Json>,
    want: u16,
) -> Result<Json, String> {
    let (st, j) = client::call(&c.url, method, path, Some(&c.session), body)?;
    if st == want {
        Ok(j)
    } else {
        Err(format!("{method} {path}: {st}, wanted {want}: {j}"))
    }
}

/// Wait up to 20 s for `ok` (the box's reads lag a control by a scan
/// tick, CLAUDE.md "Timing in tests").
pub(crate) fn wait_for(what: &str, mut ok: impl FnMut() -> Result<bool, String>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = String::new();
    while Instant::now() < deadline {
        match ok() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => last = e,
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("{what}: not within 20 s {last}"))
}

pub(crate) fn module_status(c: &Ctx, name: &str) -> Result<String, String> {
    let j = expect(c, "GET", &format!("/api/modules/{name}"), None, 200)?;
    Ok(j["status"]["state"].as_str().unwrap_or_default().to_string())
}

pub(crate) fn module_names(c: &Ctx) -> Result<BTreeSet<String>, String> {
    let j = expect(c, "GET", "/api/modules", None, 200)?;
    let list = j
        .as_array()
        .ok_or_else(|| format!("GET /api/modules: not a list: {j}"))?;
    Ok(list
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_string))
        .collect())
}

// ---- journeys --------------------------------------------------------------

fn health(c: &Ctx) -> Result<String, String> {
    let (st, j) = client::get(&c.url, "/api/health", None)?;
    if st != 200 {
        return Err(format!("GET /api/health without a session: {st} {j}"));
    }
    let running = expect(c, "GET", "/api/running", None, 200)?;
    Ok(format!("health answers open; running: {running}"))
}

fn sign_in(c: &Ctx) -> Result<String, String> {
    let (st, how) = client::get(&c.url, "/api/login", None)?;
    if st != 200 || how["credentials"] != "accounts" {
        return Err(format!(
            "GET /api/login: {st} {how}, wanted credentials: accounts"
        ));
    }
    let (st, _) = client::call(
        &c.url,
        "POST",
        "/api/login",
        None,
        Some(&json!({ "user": USER, "password": "not-the-password" })),
    )?;
    if st == 200 {
        return Err("a wrong password signed in".into());
    }
    let s = login(&c.url)?;
    let (st, who) = client::get(&c.url, "/api/session", Some(&s))?;
    if st != 200 || who["user"] != USER || who["role"] != "engineer" {
        return Err(format!("GET /api/session: {st} {who}"));
    }
    let (st, policy) = client::get(&c.url, "/api/policy", Some(&s))?;
    if st != 200 {
        return Err(format!("GET /api/policy: {st} {policy}"));
    }
    let (st, out) = client::call(&c.url, "POST", "/api/logout", Some(&s), Some(&json!({})))?;
    if st != 200 || out["logged_out"] != true {
        return Err(format!("POST /api/logout: {st} {out}"));
    }
    let (st, _) = client::get(&c.url, "/api/session", Some(&s))?;
    if st != 401 {
        return Err(format!("GET /api/session after logout: {st}, wanted 401"));
    }
    Ok("wrong password refused, signed in, session read, signed out".into())
}

fn live_values(c: &Ctx) -> Result<String, String> {
    ensure_running(c)?;
    let j = expect(c, "GET", "/api/signals", None, 200)?;
    let list = j
        .as_array()
        .or_else(|| j["signals"].as_array())
        .ok_or_else(|| format!("GET /api/signals: no list: {j}"))?;
    let first = list
        .first()
        .and_then(|s| s["name"].as_str())
        .ok_or_else(|| format!("GET /api/signals: no signal with a name: {j}"))?;
    let one = expect(c, "GET", &format!("/api/signals/{first}"), None, 200)?;
    expect(c, "GET", "/api/outputs", None, 200)?;
    Ok(format!("{} signals; {first} reads {}", list.len(), one["value"]))
}

fn module_control(c: &Ctx) -> Result<String, String> {
    let name = ensure_running(c)?;
    let ops = [("stop", "stopped"), ("run", "running"), ("restart", "running")];
    for (op, want) in ops {
        expect(
            c,
            "POST",
            &format!("/api/modules/{name}/{op}"),
            Some(&json!({})),
            200,
        )?;
        wait_for(&format!("{name} {want} after {op}"), || {
            Ok(module_status(c, &name)? == want)
        })?;
    }
    Ok(format!("{name}: stop, run, restart each took"))
}

fn projects(c: &Ctx) -> Result<String, String> {
    let p = "parity-files";
    expect(c, "POST", "/api/projects", Some(&json!({ "name": p })), 201)?;
    expect(
        c,
        "PUT",
        &format!("/api/projects/{p}/files/main.st"),
        Some(&json!({ "text": COUNTER })),
        200,
    )?;
    let (st, j) = client::call(
        &c.url,
        "GET",
        &format!("/api/projects/{p}/files/main.st"),
        Some(&c.session),
        None,
    )?;
    if st != 200 || !j.to_string().contains("n := n + 1") {
        return Err(format!("GET the file back: {st} {j}"));
    }
    let list = expect(c, "GET", "/api/projects", None, 200)?;
    if !list.to_string().contains(p) {
        return Err(format!("GET /api/projects does not list {p}: {list}"));
    }
    let q = "parity-renamed";
    expect(
        c,
        "POST",
        &format!("/api/projects/{p}/rename"),
        Some(&json!({ "to": q })),
        200,
    )?;
    expect(c, "GET", &format!("/api/projects/{q}"), None, 200)?;
    expect(c, "DELETE", &format!("/api/projects/{q}"), None, 200)?;
    let (st, _) = client::call(
        &c.url,
        "GET",
        &format!("/api/projects/{q}"),
        Some(&c.session),
        None,
    )?;
    if st != 404 {
        return Err(format!("GET a deleted project: {st}, wanted 404"));
    }
    Ok("made, wrote and read a file, listed, renamed, deleted".into())
}

/// Make project `name` with the counter, check, replay and deploy it,
/// and wait for it to run: the replay number.
pub(crate) fn deploy_project(c: &Ctx, name: &str) -> Result<u64, String> {
    expect(c, "POST", "/api/projects", Some(&json!({ "name": name })), 201)?;
    expect(
        c,
        "PUT",
        &format!("/api/projects/{name}/files/main.st"),
        Some(&json!({ "text": COUNTER })),
        200,
    )?;
    expect(
        c,
        "PUT",
        &format!("/api/projects/{name}/files/io.map"),
        Some(&json!({ "text": MAP })),
        200,
    )?;
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
    let d = expect(
        c,
        "POST",
        &format!("/api/projects/{name}/deploy"),
        Some(&json!({ "replay": n })),
        200,
    )?;
    if d["project"]["name"] != name {
        return Err(format!("deploy: {d}"));
    }
    wait_for("counter running", || Ok(module_names(c)?.contains("counter")))?;
    let running = expect(c, "GET", "/api/running", None, 200)?;
    if running["project"] != name {
        return Err(format!("GET /api/running after deploy: {running}"));
    }
    Ok(n)
}

/// A module running, deployed from project `parity-run` when none is.
pub(crate) fn ensure_running(c: &Ctx) -> Result<String, String> {
    if let Some(m) = module_names(c)?.into_iter().next() {
        return Ok(m);
    }
    deploy_project(c, "parity-run")?;
    Ok("counter".into())
}

fn deploy(c: &Ctx) -> Result<String, String> {
    let n = deploy_project(c, PROJECT)?;
    Ok(format!(
        "checked, replayed ({n}), deployed; counter runs from {PROJECT}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/FEATURES.md");
        Manifest::parse(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn every_manifest_journey_is_in_the_table_and_no_other() {
        let named: BTreeSet<String> = manifest().features.iter().map(|f| f.journey.clone()).collect();
        let table: BTreeSet<String> = journeys().map(|(id, _)| (*id).to_string()).collect();
        assert_eq!(journeys().count(), table.len(), "a journey is in the table twice");
        let missing: Vec<_> = named.difference(&table).collect();
        let extra: Vec<_> = table.difference(&named).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "docs/FEATURES.md journeys with no entry: {missing:?}; entries with no feature: {extra:?}"
        );
    }

    #[test]
    fn a_pending_journey_is_counted_and_never_a_finding() {
        let outcomes = vec![
            ("a".to_string(), Outcome::Pass("ok".into())),
            ("b".to_string(), Outcome::Pending),
            ("c".to_string(), Outcome::Fail("no".into())),
        ];
        let (f, pending) = findings(&outcomes);
        assert_eq!(pending, 1);
        assert_eq!(
            f.iter().map(|f| (f.check.as_str(), f.ok)).collect::<Vec<_>>(),
            [("parity/a", true), ("parity/c", false)]
        );
    }
}
