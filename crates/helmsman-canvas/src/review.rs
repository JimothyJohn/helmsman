//! The proposal review screen's knowledge and decisions, free of I/O like
//! [`crate::remote`]: what to ask the box for ([`Request`]), what to make
//! of each answer ([`Answer`]), and what the screen shows. The browser's
//! `fetch` (`web.rs`) carries the requests out; tests carry them out
//! natively against the real API, so the calls tested are the calls made.
//!
//! - `GET /api/proposals`, `GET /api/proposals/{id}`: the list and one
//!   draft, with its tier (`approval`), always the box's: nothing here
//!   classifies a change.
//! - `GET /api/modules/{name}`: what runs now, for the source and
//!   diagram diffs and the manifest's old values.
//! - `GET /api/signals`: which signals are box outputs.
//! - `GET /api/policy`: what each tier needs.
//! - `POST /api/proposals/{id}/replay`, `POST /api/snapshots/{id}/replay`:
//!   the sim replay, charted.
//! - `POST /api/proposals/{id}/apply`, `POST /api/snapshots/{id}/rollback`:
//!   approve and undo, `{replay}` only: the session's role approves.
//! - `POST /api/login`, `POST /api/logout`, `GET /api/session`: signing
//!   in, with a user and password on a box with accounts (`GET
//!   /api/policy` says `credentials: accounts`), else with a role and its
//!   PIN; signing out; and, after a reload, who the page is signed in as.
//!
//! **Secrets.** The page never holds a credential. The box answers a
//! login with an `HttpOnly` session cookie the browser keeps and sends
//! (no script can read it), and every call this screen makes is
//! same-origin with the `X-Helmsman-Request: 1` header the box wants
//! beside the cookie. The API token is for machines; nothing here takes
//! it. A PIN or password is held only while it is typed:
//! [`Review::login`] and [`Review::login_pin`] move it into the request
//! body and leave the field empty, whatever the answer; it is never in a
//! `Debug` output ([`Request`]'s redacts the body) or a log line. A 401
//! means the session ended (signed out elsewhere, expired, the box
//! restarted): the screen forgets who was signed in and asks again.

use std::collections::BTreeMap;
use std::fmt;

use helmsman_diagram::diff::{self, Line, ModuleDiff};
use helmsman_diagram::{Module, lower};
use helmsman_iec::{ImageSizes, compile};
use serde_json::{Value as Json, json};

/// The roles a PIN can be for, least first.
pub const ROLES: [&str; 3] = ["operator", "engineer", "integrator"];

/// `%M` bytes assumed for a draft whose manifest names none (the box's
/// default is larger only on purpose-built boxes; this is just for
/// drawing).
const DRAW_M: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// What an apply or replay is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Proposal(u64),
    /// A rollback to this snapshot (the undo).
    Snapshot(String),
}

impl Target {
    #[must_use]
    pub fn replay_url(&self) -> String {
        match self {
            Self::Proposal(id) => format!("/api/proposals/{id}/replay"),
            Self::Snapshot(s) => format!("/api/snapshots/{}/replay", hex(s)),
        }
    }

    #[must_use]
    pub fn apply_url(&self) -> String {
        match self {
            Self::Proposal(id) => format!("/api/proposals/{id}/apply"),
            Self::Snapshot(s) => format!("/api/snapshots/{}/rollback", hex(s)),
        }
    }
}

/// A snapshot id as a path segment: hex digits only (anything else is
/// dropped, so a hostile id cannot add a segment).
fn hex(s: &str) -> String {
    s.chars().filter(char::is_ascii_hexdigit).collect()
}

/// What a request is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    List,
    Proposal(u64),
    Module(String),
    Signals,
    Policy,
    Snapshots,
    Replay(Target),
    Apply(Target),
    Login,
    Logout,
    /// `GET /api/session`: who the page's cookie is.
    Session,
}

/// Who this page is signed in as (the box holds the session; the page
/// only knows its name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The person; `None` for a role-PIN session.
    pub user: Option<String>,
    pub role: String,
}

impl Session {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            user: j["user"].as_str().map(str::to_string),
            role: j["role"].as_str()?.to_string(),
        })
    }

    /// `alice (engineer)`, or `operator (PIN)`.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.user {
            Some(u) => format!("{u} ({})", self.role),
            None => format!("{} (PIN)", self.role),
        }
    }
}

/// One HTTP call for the transport: same-origin, with the browser's
/// session cookie and, for anything but a `GET`, the `X-Helmsman-Request:
/// 1` header. Its body never shows in `Debug` (a login's carries the PIN
/// or password).
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub call: Call,
    pub method: Method,
    pub url: String,
    /// A JSON body (`Content-Type: application/json`).
    pub body: Option<String>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("call", &self.call)
            .field("method", &self.method)
            .field("url", &self.url)
            .field("body", &self.body.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Request {
    fn get(call: Call, url: String) -> Self {
        Self {
            call,
            method: Method::Get,
            url,
            body: None,
        }
    }

    fn post(call: Call, url: String, body: Option<String>) -> Self {
        Self {
            call,
            method: Method::Post,
            url,
            body,
        }
    }
}

/// What came back. `status` 0: the request never got an answer (`body`
/// says why).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// `Retry-After`, seconds.
    pub retry_after: Option<u64>,
    pub body: String,
}

/// A proposal as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: u64,
    pub title: String,
    /// `draft`, `applied`.
    pub status: String,
    /// The box's tier: `param`, `logic`, `output-affecting`.
    pub tier: Option<String>,
}

impl Row {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            id: j["id"].as_u64()?,
            title: title(j),
            status: j["status"].as_str().unwrap_or("?").to_string(),
            tier: j["approval"]["tier"].as_str().map(str::to_string),
        })
    }
}

fn title(j: &Json) -> String {
    match j["kind"].as_str() {
        Some("module") => {
            let verb = if j["replaces"].as_bool() == Some(false) {
                "new module"
            } else {
                "module"
            };
            format!("{verb} {}", j["name"].as_str().unwrap_or("?"))
        }
        Some("param") => format!(
            "{}.{} = {}",
            j["module"].as_str().unwrap_or("?"),
            j["param"].as_str().unwrap_or("?"),
            j["to"]
        ),
        _ => "proposal".into(),
    }
}

/// How a manifest entry changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delta {
    Same,
    Added,
    Removed,
    Changed,
}

/// One line of the manifest comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRow {
    /// `drives`, `reads`, `q`, `i`, `scan_ms`, `param`.
    pub field: &'static str,
    pub name: String,
    pub old: Option<String>,
    pub new: Option<String>,
    pub delta: Delta,
    /// It can move an output: a driven signal, the `%Q` window.
    pub output: bool,
}

/// A param's old and new value, and its declared range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamChange {
    pub module: String,
    pub name: String,
    pub ty: String,
    pub from: String,
    pub to: String,
    pub min: Option<String>,
    pub max: Option<String>,
}

/// One signal's replay, for the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalChart {
    pub signal: String,
    pub current: Option<Track>,
    pub proposed: Option<Track>,
    pub rises: (u64, u64),
    pub on_ms: (u64, u64),
    pub identical: bool,
    pub extra_rises: i64,
}

impl SignalChart {
    /// "would have fired N more/fewer times", from the box's `SignalDiff`.
    #[must_use]
    pub fn summary(&self) -> String {
        let times = |n: i64| if n.unsigned_abs() == 1 { "time" } else { "times" };
        match self.extra_rises {
            _ if self.identical => "no difference over the window".into(),
            0 => format!(
                "would have fired as often ({}), at other moments or for other spans",
                self.rises.1
            ),
            n if n > 0 => format!(
                "would have fired {n} more {} ({} → {})",
                times(n),
                self.rises.0,
                self.rises.1
            ),
            n => format!(
                "would have fired {} fewer {} ({} → {})",
                n.unsigned_abs(),
                times(n),
                self.rises.0,
                self.rises.1
            ),
        }
    }
}

/// A replayed signal over the window: a step function.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// `bool`, `int`, `real`.
    pub ty: String,
    /// `(at_ms, value)` after each change; booleans as 0 and 1; `None`
    /// while its module was held.
    pub steps: Vec<(u64, Option<f64>)>,
    pub truncated: bool,
}

impl Track {
    fn parse(j: &Json) -> Option<Self> {
        let steps = j["steps"]
            .as_array()?
            .iter()
            .filter_map(|s| {
                let at = s[0].as_u64()?;
                let v = match &s[1] {
                    Json::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                    Json::Number(n) => n.as_f64(),
                    _ => None,
                };
                Some((at, v))
            })
            .collect();
        Some(Self {
            ty: j["type"].as_str().unwrap_or("?").to_string(),
            steps,
            truncated: j["truncated"].as_bool().unwrap_or(false),
        })
    }

    /// When a boolean track turned on (after its first step): the rises
    /// the chart marks. Counts the same as the box's `Track::rises`.
    #[must_use]
    pub fn rises(&self) -> Vec<u64> {
        self.steps
            .windows(2)
            .filter(|w| w[1].1 == Some(1.0) && w[0].1 != Some(1.0))
            .map(|w| w[1].0)
            .collect()
    }
}

/// A sim replay, as kept by the box.
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    /// The evidence number an apply cites.
    pub id: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub signals: Vec<SignalChart>,
    /// `t=…s module: fault`.
    pub faults: Vec<String>,
}

impl Chart {
    /// `POST .../replay`'s body.
    ///
    /// # Errors
    /// Not the shape the box answers with.
    pub fn parse(j: &Json) -> Result<Self, String> {
        let n = |v: &Json| v.as_u64().unwrap_or(0);
        let signals = j["signals"]
            .as_array()
            .ok_or("replay without signals")?
            .iter()
            .map(|s| SignalChart {
                signal: s["signal"].as_str().unwrap_or("?").to_string(),
                current: Track::parse(&s["current_track"]),
                proposed: Track::parse(&s["proposed_track"]),
                rises: (n(&s["current"]["rises"]), n(&s["proposed"]["rises"])),
                on_ms: (n(&s["current"]["on_ms"]), n(&s["proposed"]["on_ms"])),
                identical: s["identical"].as_bool().unwrap_or(false),
                extra_rises: s["extra_rises"].as_i64().unwrap_or(0),
            })
            .collect();
        let faults = j["faults"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|f| {
                        format!(
                            "t={:.1}s {}: {}",
                            n(&f["at_ms"]) as f64 / 1000.0,
                            f["module"].as_str().unwrap_or("?"),
                            f["fault"]["text"].as_str().unwrap_or("fault")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            id: j["replay"].as_u64().ok_or("replay without its number")?,
            start_ms: n(&j["start_ms"]),
            end_ms: n(&j["end_ms"]),
            signals,
            faults,
        })
    }
}

/// Why the box (or the screen) refused, with the box's words verbatim
/// and what to do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub why: Why,
    /// The box's `error`, exactly; or the transport's reason.
    pub error: String,
    /// What to do next, in plain words.
    pub hint: String,
    /// Page time PIN entry opens again (429).
    pub retry_at: Option<f64>,
    /// The tier the box named in its refusal.
    pub tier: Option<String>,
}

/// The kinds of refusal the screen treats differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Not signed in, or the box no longer knows the session (401).
    SignIn,
    /// The role is below what the tier needs (403).
    Role,
    /// Wrong PIN (403).
    Pin,
    /// A sim replay of exactly this change is missing or stale (428).
    Replay,
    /// PIN entry locked (429).
    Locked,
    /// The box changed since the draft (409, stale).
    Stale,
    /// Never reached the box.
    Network,
    /// Anything else, said as the box said it.
    Other,
}

/// What an answer to an apply, rollback or replay that was not a success
/// means for the screen, at page time `now`.
#[must_use]
pub fn refusal(a: &Answer, now: f64) -> Refusal {
    let j: Json = serde_json::from_str(&a.body).unwrap_or(Json::Null);
    let error = j["error"]
        .as_str()
        .map_or_else(|| a.body.trim().chars().take(300).collect(), str::to_string);
    let tier = j["tier"].as_str().map(str::to_string);
    let (why, hint) = match a.status {
        0 => (
            Why::Network,
            "The box did not answer. Nothing was changed by this screen; check the connection and try again.".into(),
        ),
        401 => (
            Why::SignIn,
            "The box no longer knows this page's session (signed out, expired after an hour or 15 minutes unused, or the box restarted). Sign in again (left panel); nothing was changed.".into(),
        ),
        403 if error == "wrong PIN" => (
            Why::Pin,
            "That PIN was not right. Wrong PINs count toward a lockout, so check it before trying again.".into(),
        ),
        403 => (
            Why::Role,
            "This change needs a higher role than the one signed in. Someone with that role signs in and approves it.".into(),
        ),
        428 => (
            Why::Replay,
            "Run the sim replay of this exact change (above), look at it, then approve again.".into(),
        ),
        429 => (
            Why::Locked,
            "Too many wrong PINs: PIN entry is locked for a while. Nothing was checked.".into(),
        ),
        409 if error.starts_with("stale") => (
            Why::Stale,
            "The box changed since this was drafted. Ask the agent to draft it again against what runs now.".into(),
        ),
        409 => (
            Why::Other,
            "The box could not take this change as it stands; nothing was changed.".into(),
        ),
        _ => (Why::Other, "Nothing was changed.".into()),
    };
    let wait = a.retry_after.or_else(|| j["retry_after_s"].as_u64());
    Refusal {
        status: a.status,
        why,
        error,
        hint,
        retry_at: (a.status == 429).then(|| now + wait.unwrap_or(30) as f64),
        tier,
    }
}

/// An apply that landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// What runs now.
    pub snapshot: String,
    /// What ran before: the undo's target.
    pub previous: String,
    pub tier: String,
    pub role: String,
    /// The person, when a login session approved it.
    pub user: Option<String>,
}

impl Applied {
    fn parse(j: &Json) -> Option<Self> {
        Some(Self {
            snapshot: j["snapshot"].as_str()?.to_string(),
            previous: j["previous"].as_str()?.to_string(),
            tier: j["tier"].as_str().unwrap_or("?").to_string(),
            role: j["role"].as_str().unwrap_or("?").to_string(),
            user: j["user"].as_str().map(str::to_string),
        })
    }
}

/// Where an approval stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Sending,
    Applied(Applied),
    Refused(Refusal),
}

/// Where a replay stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Replay {
    None,
    Running,
    Done(Chart),
    Failed(Refusal),
}

/// Approving one target: a proposal, or the undo of one.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub target: Target,
    pub replay: Replay,
    pub phase: Phase,
}

impl Step {
    const fn new(target: Target) -> Self {
        Self {
            target,
            replay: Replay::None,
            phase: Phase::Idle,
        }
    }

    /// Seconds until the box takes an approval again, while locked.
    #[must_use]
    pub fn locked_for(&self, now: f64) -> Option<u64> {
        match &self.phase {
            Phase::Refused(Refusal {
                retry_at: Some(at), ..
            }) if *at > now => Some((at - now).ceil() as u64),
            _ => None,
        }
    }
}

/// Both sides of the diagram diff.
#[derive(Debug, Clone)]
pub struct Drawn {
    /// What runs (`None`: a new module).
    pub old: Option<Module>,
    pub new: Module,
    pub diff: ModuleDiff,
}

/// The proposal on screen.
#[derive(Debug, Clone)]
pub struct Open {
    pub id: u64,
    pub json: Json,
    /// The running module (`GET /api/modules/{name}`): `None` until it
    /// came; `Some(Null)` for a new module.
    pub current: Option<Json>,
    pub lines: Vec<Line>,
    pub drawn: Option<Drawn>,
    /// Why the diagram cannot be drawn.
    pub problem: Option<String>,
    pub manifest: Vec<ManifestRow>,
    pub param: Option<ParamChange>,
    pub step: Step,
    /// After an apply: rolling back to what ran before.
    pub undo: Option<Step>,
}

impl Open {
    /// The box's tier for this draft; never worked out here.
    #[must_use]
    pub fn tier(&self) -> Option<&str> {
        self.json["approval"]["tier"].as_str()
    }

    /// Why the box gave that tier.
    #[must_use]
    pub fn reasons(&self) -> Vec<String> {
        self.json["approval"]["reasons"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| Some(format!("{}: {}", r["tier"].as_str()?, r["why"].as_str()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What the policy wants for it: least role, and whether a replay.
    #[must_use]
    pub fn needs(&self) -> Option<(String, bool)> {
        let n = &self.json["approval"]["needs"];
        Some((n["role"].as_str()?.to_string(), n["replay"].as_bool()?))
    }

    #[must_use]
    pub fn module_name(&self) -> Option<&str> {
        match self.json["kind"].as_str() {
            Some("module") => self.json["name"].as_str(),
            Some("param") => self.json["module"].as_str(),
            _ => None,
        }
    }

    #[must_use]
    pub fn applied(&self) -> bool {
        self.json["status"].as_str() == Some("applied") || matches!(self.step.phase, Phase::Applied(_))
    }
}

/// Everything the review screen knows.
#[derive(Debug, Default)]
pub struct Review {
    pub list: Vec<Row>,
    /// Why the list could not be read.
    pub list_error: Option<String>,
    pub open: Option<Open>,
    pub policy: Option<Json>,
    /// Box output signals, upper-cased.
    pub outputs: Vec<String>,
    /// The history entry for what runs, after an apply.
    pub head: Option<Json>,
    /// Who this page is signed in as.
    pub session: Option<Session>,
    /// Why the last sign-in did not work, or why the session ended.
    pub login_problem: Option<String>,
    /// A login is on its way.
    pub logging_in: bool,
    queue: Vec<Request>,
}

impl Review {
    /// Starts: the list, the outputs and the policy.
    #[must_use]
    pub fn new() -> Self {
        let mut r = Self::default();
        r.refresh();
        r
    }

    /// Asks for the list (and signals and policy) again.
    pub fn refresh(&mut self) {
        self.queue.push(Request::get(Call::List, "/api/proposals".into()));
        self.queue
            .push(Request::get(Call::Signals, "/api/signals".into()));
        self.queue.push(Request::get(Call::Policy, "/api/policy".into()));
        self.queue
            .push(Request::get(Call::Session, "/api/session".into()));
    }

    /// What to send now.
    pub fn poll(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.queue)
    }

    /// Whether the box signs people in by account (it has an accounts
    /// file) rather than by role PIN, as its policy says.
    #[must_use]
    pub fn accounts(&self) -> bool {
        self.credentials() == Some("accounts")
    }

    /// What the box's policy says it signs in with: `pins`, `accounts`, or
    /// `unavailable`; `None` until the policy came.
    #[must_use]
    pub fn credentials(&self) -> Option<&str> {
        self.policy.as_ref()?["credentials"].as_str()
    }

    /// `alice (engineer)` or `operator (PIN)`, while signed in.
    #[must_use]
    pub fn signed_in(&self) -> Option<String> {
        self.session.as_ref().map(Session::label)
    }

    fn sign_in(&mut self, body: String) {
        self.login_problem = None;
        self.logging_in = true;
        self.queue
            .push(Request::post(Call::Login, "/api/login".into(), Some(body)));
    }

    /// Signs `user` in; the password goes into the request body and
    /// nowhere else.
    pub fn login(&mut self, user: &str, password: String) {
        let user = user.trim();
        if user.is_empty() || password.is_empty() || self.logging_in {
            self.login_problem = Some("Type your user name and password.".into());
            return;
        }
        let body = json!({ "user": user, "password": password }).to_string();
        drop(password);
        self.sign_in(body);
    }

    /// Signs in as `role` with its PIN (boxes without accounts); the PIN
    /// goes into the request body and nowhere else.
    pub fn login_pin(&mut self, role: &str, pin: String) {
        if !ROLES.contains(&role) || pin.is_empty() || self.logging_in {
            self.login_problem = Some("Pick a role and type its PIN.".into());
            return;
        }
        let body = json!({ "role": role, "pin": pin }).to_string();
        drop(pin);
        self.sign_in(body);
    }

    /// Ends the session here at once, and on the box (which clears the
    /// cookie).
    pub fn logout(&mut self) {
        if self.session.take().is_some() {
            self.queue
                .push(Request::post(Call::Logout, "/api/logout".into(), None));
        }
    }

    /// The box no longer knows the session (401): back to signing in.
    fn session_ended(&mut self) {
        if self.session.take().is_some() {
            self.login_problem =
                Some("The session ended (signed out, expired, or the box restarted). Sign in again.".into());
        }
    }

    /// Opens proposal `id`.
    pub fn open(&mut self, id: u64) {
        if self.open.as_ref().is_some_and(|o| o.id == id) {
            return;
        }
        self.open = None;
        self.queue
            .push(Request::get(Call::Proposal(id), format!("/api/proposals/{id}")));
    }

    fn step_mut(&mut self, target: &Target) -> Option<&mut Step> {
        let o = self.open.as_mut()?;
        if o.step.target == *target {
            return Some(&mut o.step);
        }
        o.undo.as_mut().filter(|u| u.target == *target)
    }

    /// Asks the box to replay `target` (the open proposal, or its undo).
    pub fn replay(&mut self, target: &Target) {
        let signed_in = self.session.is_some();
        let Some(step) = self.step_mut(target) else { return };
        if !signed_in {
            step.replay = Replay::Failed(not_signed_in());
            return;
        }
        step.replay = Replay::Running;
        self.queue.push(Request::post(
            Call::Replay(target.clone()),
            target.replay_url(),
            Some("{}".into()),
        ));
    }

    /// Sends the approval of `target` as whoever is signed in: their role
    /// is the session's, so only the replay it cites is sent.
    pub fn approve(&mut self, target: &Target, now: f64) {
        let signed_in = self.session.is_some();
        let Some(step) = self.step_mut(target) else { return };
        if step.locked_for(now).is_some() || matches!(step.phase, Phase::Sending) {
            return;
        }
        if !signed_in {
            step.phase = Phase::Refused(not_signed_in());
            return;
        }
        let mut body = json!({});
        if let Replay::Done(c) = &step.replay {
            body["replay"] = json!(c.id);
        }
        step.phase = Phase::Sending;
        self.queue.push(Request::post(
            Call::Apply(target.clone()),
            target.apply_url(),
            Some(body.to_string()),
        ));
    }

    /// Takes in an answer at page time `now` (seconds).
    pub fn answer(&mut self, call: Call, a: &Answer, now: f64) {
        let ok = (200..300).contains(&a.status);
        let j: Option<Json> = serde_json::from_str(&a.body).ok();
        match call {
            Call::List => match j.as_ref().and_then(Json::as_array) {
                Some(list) if ok => {
                    self.list_error = None;
                    let mut rows: Vec<Row> = list.iter().filter_map(Row::parse).collect();
                    rows.sort_by_key(|r| std::cmp::Reverse(r.id));
                    self.list = rows;
                }
                _ => self.list_error = Some(format!("cannot read the proposals: {}", short(a))),
            },
            Call::Signals => {
                if let (true, Some(list)) = (ok, j.as_ref().and_then(Json::as_array)) {
                    self.outputs = list
                        .iter()
                        .filter(|s| s["dir"] == "out")
                        .filter_map(|s| s["name"].as_str().map(str::to_ascii_uppercase))
                        .collect();
                    if let Some(o) = &mut self.open {
                        o.manifest = manifest_rows(&o.json, o.current.as_ref(), &self.outputs);
                    }
                }
            }
            Call::Policy => self.policy = j.filter(|_| ok),
            Call::Snapshots => {
                self.head = j
                    .filter(|_| ok)
                    .map(|j| j["log"][0].clone())
                    .filter(|h| !h.is_null());
            }
            Call::Proposal(id) => {
                let Some(j) = j.filter(|_| ok) else {
                    self.list_error = Some(format!("cannot open proposal {id}: {}", short(a)));
                    return;
                };
                self.opened(id, j);
            }
            Call::Module(name) => {
                let Some(o) = self.open.as_mut() else { return };
                if o.module_name() != Some(name.as_str()) {
                    return;
                }
                o.current = Some(match (a.status, j) {
                    (200, Some(j)) => j,
                    // A new module: nothing runs under that name.
                    (404, _) => Json::Null,
                    _ => {
                        o.problem = Some(format!("cannot read what runs now: {}", short(a)));
                        return;
                    }
                });
                compare(o, &self.outputs);
            }
            Call::Replay(target) => {
                let Some(step) = self.step_mut(&target) else {
                    return;
                };
                step.replay = match (a.status, j.as_ref().map(Chart::parse)) {
                    (201, Some(Ok(c))) => Replay::Done(c),
                    (201, _) => Replay::Failed(Refusal {
                        status: a.status,
                        why: Why::Other,
                        error: "the replay's answer could not be read".into(),
                        hint: "Try the replay again.".into(),
                        retry_at: None,
                        tier: None,
                    }),
                    _ => Replay::Failed(refusal(a, now)),
                };
                if matches!(&step.replay, Replay::Failed(r) if r.why == Why::SignIn) {
                    self.session_ended();
                }
            }
            Call::Apply(target) => {
                let Some(step) = self.step_mut(&target) else {
                    return;
                };
                let applied = j.as_ref().filter(|_| a.status == 200).and_then(Applied::parse);
                if let Some(done) = applied {
                    let undo = matches!(target, Target::Proposal(_))
                        .then(|| Step::new(Target::Snapshot(done.previous.clone())));
                    step.phase = Phase::Applied(done);
                    if let Some(o) = self.open.as_mut()
                        && undo.is_some()
                    {
                        o.json["status"] = json!("applied");
                        o.undo = undo;
                    }
                    self.queue
                        .push(Request::get(Call::Snapshots, "/api/snapshots".into()));
                    self.queue.push(Request::get(Call::List, "/api/proposals".into()));
                } else {
                    let r = refusal(a, now);
                    let ended = r.why == Why::SignIn;
                    step.phase = Phase::Refused(r);
                    if ended {
                        self.session_ended();
                    }
                }
            }
            Call::Session => {
                // A reload while signed in: the browser still has the cookie.
                self.session = j.as_ref().filter(|_| a.status == 200).and_then(Session::parse);
            }
            Call::Login => {
                self.logging_in = false;
                let session = j.as_ref().filter(|_| a.status == 200).and_then(Session::parse);
                if let Some(s) = session {
                    self.session = Some(s);
                    self.login_problem = None;
                    return;
                }
                let error = j
                    .as_ref()
                    .and_then(|j| j["error"].as_str())
                    .map_or_else(|| short(a), str::to_string);
                let wait = a
                    .retry_after
                    .or_else(|| j.as_ref().and_then(|j| j["retry_after_s"].as_u64()));
                self.login_problem = Some(match (a.status, wait) {
                    (429, Some(s)) => format!("{error}. Try again in {s} s."),
                    _ => error,
                });
            }
            // Ended here already; the box's answer changes nothing.
            Call::Logout => {}
        }
    }

    fn opened(&mut self, id: u64, j: Json) {
        let param = (j["kind"] == "param").then(|| ParamChange {
            module: j["module"].as_str().unwrap_or("?").into(),
            name: j["param"].as_str().unwrap_or("?").into(),
            ty: j["type"].as_str().unwrap_or("?").into(),
            from: j["from"].to_string(),
            to: j["to"].to_string(),
            min: (!j["min"].is_null()).then(|| j["min"].to_string()),
            max: (!j["max"].is_null()).then(|| j["max"].to_string()),
        });
        let mut o = Open {
            id,
            current: None,
            lines: Vec::new(),
            drawn: None,
            problem: None,
            manifest: Vec::new(),
            param,
            step: Step::new(Target::Proposal(id)),
            undo: None,
            json: j,
        };
        if o.json["kind"] == "module"
            && let Some(name) = o.module_name().map(str::to_string)
        {
            self.queue.push(Request::get(
                Call::Module(name.clone()),
                crate::remote::module_url(&name),
            ));
        } else {
            o.current = Some(Json::Null);
        }
        self.open = Some(o);
    }
}

fn not_signed_in() -> Refusal {
    Refusal {
        status: 0,
        why: Why::SignIn,
        error: "not signed in".into(),
        hint: "Sign in first (left panel): with your user name and password on a box with accounts, else with your role's PIN. Nothing was sent.".into(),
        retry_at: None,
        tier: None,
    }
}

fn short(a: &Answer) -> String {
    if a.status == 0 {
        return a.body.clone();
    }
    let j: Option<Json> = serde_json::from_str(&a.body).ok();
    match j.as_ref().and_then(|j| j["error"].as_str()) {
        Some(e) => format!("HTTP {}: {e}", a.status),
        None => format!("HTTP {}", a.status),
    }
}

/// Image sizes for drawing a module from its manifest JSON (or the
/// module's windows), so its located variables compile.
fn sizes(m: &Json, module: &Json) -> ImageSizes {
    let len = |w: &Json| w["len"].as_u64().and_then(|n| usize::try_from(n).ok());
    let pick = |k: &str| len(&m[k]).or_else(|| len(&module[k])).unwrap_or(0);
    ImageSizes {
        i: pick("i"),
        q: pick("q"),
        m: m["m"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(DRAW_M),
    }
}

/// `src` compiled and lowered for drawing.
///
/// # Errors
/// It does not compile or lower, as text.
pub fn lower_source(src: &str, sizes: ImageSizes) -> Result<Module, String> {
    let program = compile(src, sizes).map_err(|d| {
        d.iter()
            .map(|d| format!("{}: {}", d.pos, d.msg))
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    lower(src, &program.trace_map()).map_err(|e| e.to_string())
}

/// Works out the source and diagram diffs and the manifest rows, once
/// both sides are in.
fn compare(o: &mut Open, outputs: &[String]) {
    o.manifest = manifest_rows(&o.json, o.current.as_ref(), outputs);
    if o.json["kind"] != "module" {
        return;
    }
    let cur = o.current.clone().unwrap_or(Json::Null);
    let old_src = cur["source"].as_str().unwrap_or("");
    let new_src = o.json["source"].as_str().unwrap_or("");
    o.lines = diff::lines(old_src, new_src);
    if new_src.trim_start().starts_with('<') {
        o.problem =
            Some("this module is a PLCopen XML project; the diagram diff draws Structured Text".into());
        return;
    }
    let new = match lower_source(new_src, sizes(&o.json["manifest"], &Json::Null)) {
        Ok(m) => m,
        Err(e) => {
            o.problem = Some(format!("cannot draw the proposed module: {e}"));
            return;
        }
    };
    let old = if cur.is_null() || old_src.trim_start().starts_with('<') {
        None
    } else {
        match lower_source(old_src, sizes(&cur["manifest"], &cur)) {
            Ok(m) => Some(m),
            Err(e) => {
                o.problem = Some(format!("cannot draw the running module: {e}"));
                return;
            }
        }
    };
    let d = diff::modules(old.as_ref().unwrap_or(&Module { pous: Vec::new() }), &new);
    o.drawn = Some(Drawn { old, new, diff: d });
}

fn list(j: &Json) -> Vec<String> {
    j.as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn window(j: &Json) -> Option<String> {
    let (s, l) = (j["start"].as_u64()?, j["len"].as_u64()?);
    Some(if l == 0 {
        "none".into()
    } else {
        format!("bytes {s}..{}", s + l - 1)
    })
}

fn row(
    field: &'static str,
    name: &str,
    old: Option<String>,
    new: Option<String>,
    output: bool,
) -> ManifestRow {
    let delta = match (&old, &new) {
        (None, Some(_)) => Delta::Added,
        (Some(_), None) => Delta::Removed,
        (a, b) if a == b => Delta::Same,
        _ => Delta::Changed,
    };
    ManifestRow {
        field,
        name: name.into(),
        old,
        new,
        delta,
        output,
    }
}

/// The manifest, old against new: driven signals first (each can move an
/// output), then `%Q`, reads, `%I`, scan and params. Rows that did not
/// change are kept, marked [`Delta::Same`].
#[must_use]
pub fn manifest_rows(p: &Json, current: Option<&Json>, outputs: &[String]) -> Vec<ManifestRow> {
    if p["kind"] != "module" {
        return Vec::new();
    }
    let new = &p["manifest"];
    let old = current.map_or(&Json::Null, |c| &c["manifest"]);
    let mut rows = Vec::new();
    let is_output = |s: &str| outputs.iter().any(|o| o.eq_ignore_ascii_case(s));
    let sets = |field: &'static str, rows: &mut Vec<ManifestRow>, out: bool| {
        let (a, b) = (list(&old[field]), list(&new[field]));
        let mut names: Vec<&String> = b.iter().chain(&a).collect();
        names.sort_by_key(|n| n.to_ascii_uppercase());
        names.dedup_by(|x, y| x.eq_ignore_ascii_case(y));
        for n in names {
            let has = |v: &[String]| v.iter().any(|x| x.eq_ignore_ascii_case(n)).then(|| n.clone());
            let driven = out || is_output(n);
            rows.push(row(field, n, has(&a), has(&b), field == "drives" && driven));
        }
    };
    sets("drives", &mut rows, true);
    rows.push(row("q", "%Q window", window(&old["q"]), window(&new["q"]), true));
    sets("reads", &mut rows, false);
    rows.push(row("i", "%I window", window(&old["i"]), window(&new["i"]), false));
    let scan = |m: &Json| m["scan_ms"].as_u64().map(|n| format!("{n} ms"));
    rows.push(row("scan_ms", "scan", scan(old), scan(new), false));
    let params = |m: &Json| -> BTreeMap<String, String> {
        m["params"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|q| {
                        let range = match (&q["min"], &q["max"]) {
                            (Json::Null, Json::Null) => String::new(),
                            (lo, hi) => format!("  [{lo} .. {hi}]"),
                        };
                        Some((q["name"].as_str()?.to_string(), format!("{}{range}", q["value"])))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let (pa, pb) = (params(old), params(new));
    let mut names: Vec<&String> = pa.keys().chain(pb.keys()).collect();
    names.sort();
    names.dedup();
    for n in names {
        rows.push(row("param", n, pa.get(n).cloned(), pb.get(n).cloned(), false));
    }
    // A new module: everything is added; there is no "old".
    if current.is_some_and(Json::is_null) {
        for r in &mut rows {
            if r.new.is_some() && r.new.as_deref() != Some("none") {
                r.delta = Delta::Added;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ans(status: u16, body: &Json) -> Answer {
        Answer {
            status,
            retry_after: None,
            body: body.to_string(),
        }
    }

    /// Signed in as `role` by PIN, as the box answers a browser login.
    fn sign_in(r: &mut Review, role: &str) {
        r.login_pin(role, "123456".into());
        let sent = r.poll();
        assert_eq!(sent.len(), 1);
        r.answer(
            Call::Login,
            &ans(
                200,
                &json!({ "user": null, "role": role, "credential": "pin", "expires_in_s": 3600, "idle_s": 900, "cookie": true }),
            ),
            0.0,
        );
        assert_eq!(r.signed_in(), Some(format!("{role} (PIN)")));
    }

    fn opened() -> Review {
        let mut r = Review::new();
        r.poll();
        r.answer(
            Call::Proposal(7),
            &ans(
                200,
                &json!({ "id": 7, "kind": "param", "module": "m", "param": "speed", "type": "real",
                         "from": 1.5, "to": 2.0, "min": 0.0, "max": 10.0, "status": "draft",
                         "approval": { "tier": "param", "reasons": [], "needs": { "role": "operator", "replay": false } } }),
            ),
            0.0,
        );
        r
    }

    #[test]
    fn every_api_status_has_its_own_screen_state_with_the_box_words_verbatim() {
        let cases = [
            (
                401,
                json!({ "error": "not logged in, or the session ended; log in again" }),
                Why::SignIn,
            ),
            (403, json!({ "error": "wrong PIN", "tier": "logic" }), Why::Pin),
            (
                403,
                json!({ "error": "a logic change needs the engineer role or above", "tier": "logic" }),
                Why::Role,
            ),
            (
                428,
                json!({ "error": "a logic change needs a sim replay of exactly this change first (POST .../replay)" }),
                Why::Replay,
            ),
            (
                429,
                json!({ "error": "too many wrong PINs; PIN entry is locked", "retry_after_s": 40 }),
                Why::Locked,
            ),
            (
                409,
                json!({ "error": "stale: the box has changed since this proposal was drafted; draft it again" }),
                Why::Stale,
            ),
            (
                409,
                json!({ "error": "changing the I/O map or the calendar on a running box is not built (card map-reload); nothing was changed" }),
                Why::Other,
            ),
            (500, json!({ "error": "boom" }), Why::Other),
        ];
        for (status, body, why) in cases {
            let r = refusal(&ans(status, &body), 100.0);
            assert_eq!(r.why, why, "{status} {body}");
            assert_eq!(r.error, body["error"].as_str().unwrap(), "verbatim");
            assert_eq!(r.status, status);
            assert_ne!(r.hint, "");
            assert_eq!(r.retry_at.is_some(), status == 429);
        }
        let net = refusal(
            &Answer {
                status: 0,
                retry_after: None,
                body: "connection refused".into(),
            },
            0.0,
        );
        assert_eq!(
            (net.why, net.error.as_str()),
            (Why::Network, "connection refused")
        );
        // Not JSON: the text itself, cut short.
        let raw = refusal(
            &Answer {
                status: 502,
                retry_after: None,
                body: "x".repeat(1000),
            },
            0.0,
        );
        assert_eq!(raw.error.len(), 300);
    }

    #[test]
    fn a_lockout_counts_down_from_retry_after_and_blocks_sending() {
        let mut r = opened();
        sign_in(&mut r, "operator");
        let t = Target::Proposal(7);
        r.approve(&t, 10.0);
        let sent = r.poll();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].url, "/api/proposals/7/apply");
        assert_eq!(sent[0].method, Method::Post);
        let body: Json = serde_json::from_str(sent[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({}), "the session's role: no role, no PIN");
        r.answer(
            Call::Apply(t.clone()),
            &Answer {
                status: 429,
                retry_after: Some(30),
                body: json!({ "error": "too many wrong PINs; PIN entry is locked", "retry_after_s": 30 })
                    .to_string(),
            },
            10.0,
        );
        let step = &r.open.as_ref().unwrap().step;
        assert_eq!(step.locked_for(10.0), Some(30));
        assert_eq!(step.locked_for(39.2), Some(1));
        assert_eq!(step.locked_for(40.0), None);
        r.approve(&t, 20.0);
        assert!(r.poll().is_empty(), "nothing is sent while locked");
        r.approve(&t, 41.0);
        assert_eq!(r.poll().len(), 1, "after the countdown it goes");
    }

    #[test]
    fn not_signed_in_nothing_is_sent_and_a_401_signs_the_page_out() {
        let mut r = opened();
        let t = Target::Proposal(7);
        r.approve(&t, 0.0);
        r.replay(&t);
        assert_eq!(r.poll(), Vec::<Request>::new());
        let step = &r.open.as_ref().unwrap().step;
        assert!(matches!(&step.phase, Phase::Refused(x) if x.why == Why::SignIn));
        assert!(matches!(&step.replay, Replay::Failed(x) if x.why == Why::SignIn));

        // Signing in with a PIN: role and PIN in the body, redacted in Debug.
        r.login_pin("engineer", "eng-135797".into());
        let sent = r.poll();
        assert_eq!((sent.len(), sent[0].url.as_str()), (1, "/api/login"));
        let body: Json = serde_json::from_str(sent[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({ "role": "engineer", "pin": "eng-135797" }));
        assert!(!format!("{sent:?}").contains("eng-135797"), "never in Debug");
        assert!(r.logging_in);
        r.login_pin("engineer", "again".into());
        assert_eq!(r.poll(), Vec::<Request>::new(), "one sign-in at a time");
        // A wrong PIN: the box's words; locked: when to try again.
        r.answer(Call::Login, &ans(403, &json!({ "error": "wrong PIN" })), 0.0);
        assert_eq!(
            (r.login_problem.as_deref(), r.session.as_ref()),
            (Some("wrong PIN"), None)
        );
        r.login_pin("engineer", "x".into());
        r.poll();
        r.answer(
            Call::Login,
            &Answer {
                status: 429,
                retry_after: Some(30),
                body: json!({ "error": "too many wrong PINs; PIN entry is locked" }).to_string(),
            },
            0.0,
        );
        assert_eq!(
            r.login_problem.as_deref(),
            Some("too many wrong PINs; PIN entry is locked. Try again in 30 s.")
        );
        // Nothing to send without a role or a PIN.
        for (role, pin) in [("root", "123456"), ("engineer", ""), ("", "123456")] {
            r.login_pin(role, pin.into());
            assert_eq!(r.poll(), Vec::<Request>::new(), "{role:?} {pin:?}");
            assert!(r.login_problem.is_some());
        }

        sign_in(&mut r, "engineer");
        assert_eq!(r.login_problem, None);
        r.replay(&t);
        let sent = r.poll();
        assert_eq!(sent[0].url, "/api/proposals/7/replay");
        r.answer(
            Call::Replay(t.clone()),
            &ans(401, &json!({ "error": "session expired; log in again" })),
            0.0,
        );
        assert_eq!(r.session, None, "a session the box ended is forgotten");
        assert!(r.login_problem.as_deref().unwrap().contains("Sign in again"));
        r.approve(&t, 0.0);
        assert_eq!(r.poll(), Vec::<Request>::new(), "signed out: nothing sent");
    }

    #[test]
    fn a_reload_finds_the_cookie_session_and_sign_out_ends_it_on_the_box() {
        let mut r = Review::new();
        let calls: Vec<Call> = r.poll().into_iter().map(|q| q.call).collect();
        assert_eq!(calls, [Call::List, Call::Signals, Call::Policy, Call::Session]);
        r.answer(
            Call::Session,
            &ans(401, &json!({ "error": "not logged in" })),
            0.0,
        );
        assert_eq!(r.signed_in(), None);
        r.answer(
            Call::Session,
            &ans(
                200,
                &json!({ "user": "alice", "role": "engineer", "credential": "account", "via": "cookie" }),
            ),
            0.0,
        );
        assert_eq!(r.signed_in().as_deref(), Some("alice (engineer)"));
        r.logout();
        assert_eq!(r.signed_in(), None, "gone here at once");
        let sent = r.poll();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (
                &sent[0].call,
                sent[0].method,
                sent[0].url.as_str(),
                sent[0].body.as_deref()
            ),
            (&Call::Logout, Method::Post, "/api/logout", None)
        );
        r.logout();
        assert_eq!(r.poll(), Vec::<Request>::new(), "nothing to end twice");
        // The policy says which sign-in the panel offers.
        r.answer(
            Call::Policy,
            &ans(200, &json!({ "credentials": "accounts" })),
            0.0,
        );
        assert!(r.accounts());
        r.answer(Call::Policy, &ans(200, &json!({ "credentials": "pins" })), 0.0);
        assert_eq!((r.accounts(), r.credentials()), (false, Some("pins")));
    }

    #[test]
    fn an_apply_offers_the_undo_through_the_same_flow_and_cites_the_replay() {
        let mut r = opened();
        sign_in(&mut r, "engineer");
        let t = Target::Proposal(7);
        r.replay(&t);
        r.poll();
        r.answer(
            Call::Replay(t.clone()),
            &ans(
                201,
                &json!({ "replay": 3, "start_ms": 0, "end_ms": 1000, "signals": [], "faults": [] }),
            ),
            0.0,
        );
        r.approve(&t, 0.0);
        let sent = r.poll();
        let body: Json = serde_json::from_str(sent[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({ "replay": 3 }));
        let prev = "ab".repeat(32);
        r.answer(
            Call::Apply(t),
            &ans(
                200,
                &json!({ "applied": true, "tier": "param", "role": "engineer", "snapshot": "cd".repeat(32),
                         "previous": prev, "undo": format!("/api/snapshots/{prev}/rollback") }),
            ),
            1.0,
        );
        let o = r.open.as_ref().unwrap();
        assert!(o.applied());
        let undo = o.undo.as_ref().expect("an undo is offered");
        assert_eq!(undo.target, Target::Snapshot(prev.clone()));
        assert_eq!(undo.target.apply_url(), format!("/api/snapshots/{prev}/rollback"));
        assert_eq!(undo.target.replay_url(), format!("/api/snapshots/{prev}/replay"));
        let calls: Vec<Call> = r.poll().into_iter().map(|q| q.call).collect();
        assert_eq!(calls, [Call::Snapshots, Call::List]);
        // A 409 for the undo lands on the undo, not the apply.
        let ut = Target::Snapshot(prev);
        r.approve(&ut, 2.0);
        r.poll();
        r.answer(Call::Apply(ut), &ans(409, &json!({ "error": "stale: x" })), 2.0);
        let o = r.open.as_ref().unwrap();
        assert!(matches!(o.step.phase, Phase::Applied(_)));
        assert!(matches!(&o.undo.as_ref().unwrap().phase, Phase::Refused(x) if x.why == Why::Stale));
        // A snapshot id cannot smuggle path segments.
        assert_eq!(
            Target::Snapshot("../x/ab".into()).apply_url(),
            "/api/snapshots/ab/rollback"
        );
    }

    #[test]
    fn answers_for_something_not_on_screen_are_ignored() {
        let mut r = opened();
        r.answer(Call::Apply(Target::Proposal(99)), &ans(200, &json!({})), 0.0);
        r.answer(
            Call::Module("other".into()),
            &ans(200, &json!({ "source": "x" })),
            0.0,
        );
        let o = r.open.as_ref().unwrap();
        assert_eq!(o.step.phase, Phase::Idle);
        assert_eq!(o.lines, []);
        r.answer(Call::List, &ans(200, &json!({ "not": "a list" })), 0.0);
        assert!(r.list_error.is_some());
        r.answer(Call::List, &ans(200, &json!([{ "id": 1, "kind": "module", "name": "a", "replaces": false, "status": "draft", "approval": { "tier": "output-affecting" } }, { "id": 2 }, { "nope": 1 }])), 0.0);
        assert_eq!(r.list.len(), 2, "rows without an id are dropped");
        assert_eq!(r.list[0].id, 2, "newest first");
        assert_eq!(r.list[1].title, "new module a");
        assert_eq!(r.list[1].tier.as_deref(), Some("output-affecting"));
    }

    #[test]
    fn rises_and_summaries_read_like_the_box_counts() {
        let t = Track {
            ty: "bool".into(),
            steps: vec![
                (0, Some(1.0)),
                (5, Some(0.0)),
                (9, Some(1.0)),
                (12, None),
                (20, Some(1.0)),
            ],
            truncated: false,
        };
        assert_eq!(t.rises(), [9, 20], "the first step is not a rise");
        let mut s = SignalChart {
            signal: "lamp".into(),
            current: None,
            proposed: Some(t),
            rises: (1, 4),
            on_ms: (0, 0),
            identical: false,
            extra_rises: 3,
        };
        assert_eq!(s.summary(), "would have fired 3 more times (1 → 4)");
        s.extra_rises = -1;
        s.rises = (2, 1);
        assert_eq!(s.summary(), "would have fired 1 fewer time (2 → 1)");
        s.identical = true;
        assert_eq!(s.summary(), "no difference over the window");
    }

    #[test]
    fn new_driven_signals_and_the_q_window_stand_out_in_the_manifest() {
        let p = json!({ "kind": "module", "name": "c", "manifest": {
            "drives": ["lamp", "horn"], "reads": ["start"], "q": null, "i": null, "scan_ms": 10,
            "params": [{ "name": "speed", "value": 2.0, "min": 0.0, "max": 10.0 }] } });
        let cur = json!({ "manifest": { "drives": ["lamp"], "reads": ["start", "stop"], "q": null, "i": null,
            "scan_ms": 10, "params": [{ "name": "speed", "value": 1.5, "min": 0.0, "max": 10.0 }] } });
        let rows = manifest_rows(&p, Some(&cur), &["HORN".into(), "LAMP".into()]);
        let find = |f: &str, n: &str| rows.iter().find(|r| r.field == f && r.name == n).unwrap();
        assert_eq!(find("drives", "horn").delta, Delta::Added);
        assert!(find("drives", "horn").output);
        assert_eq!(find("drives", "lamp").delta, Delta::Same);
        assert_eq!(find("reads", "stop").delta, Delta::Removed);
        assert!(!find("reads", "start").output);
        let speed = find("param", "speed");
        assert_eq!(speed.delta, Delta::Changed);
        assert_eq!(speed.old.as_deref(), Some("1.5  [0.0 .. 10.0]"));
        assert_eq!(speed.new.as_deref(), Some("2.0  [0.0 .. 10.0]"));
        assert_eq!(rows[0].field, "drives", "outputs first");
    }
}
