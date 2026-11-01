//! Applying a proposal and rolling back: the only two ways the box API
//! changes what runs, both through the same checks, in this order:
//!
//! 1. The target change set is known (a stored proposal's, or a snapshot's)
//!    and, for a proposal, it was drafted against what runs now (else 409:
//!    stale, draft it again).
//! 2. The step from what runs to the target is classified
//!    ([`crate::policy::classify`]) and the policy's rule for that tier
//!    looked up.
//! 3. The role named in the body is at least the rule's (else 403, and no
//!    PIN is checked, so no attempt is spent).
//! 4. If the rule wants one, the body names a sim replay
//!    (`POST .../replay`) whose base and content hashes are exactly what
//!    runs and the target, covering at least the policy's window (else 428).
//! 5. The role's PIN (else 403, or 429 while entry is locked).
//! 6. Every module of the target compiles (else 422). If the target
//!    brings another I/O map or calendar, they load (else 422) and the
//!    modules bind to them (else 409).
//! 7. The scan thread swaps the whole module set between ticks
//!    ([`super::swap`]), and with another map or calendar reloads the
//!    table, the calendar and the device links around it
//!    ([`super::swap::SwapHandle::reload`]); a link that will not start or
//!    a refusal there changes nothing (409).
//! 8. The snapshot store commits the target with author, prompt, approver
//!    and time; if that fails the scan thread is swapped back (500).
//!
//! Applies are serialized: two racing applies of conflicting proposals see
//! one win and the other refused as stale, never half of each. Every
//! outcome is in the audit log with the role, the tier and both snapshot
//! ids; an apply that acts writes an entry before (so a full log stops it)
//! and one after.

use std::fmt::Write as _;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::store::Stored;
use super::swap::{Plant, Reloaded, SwapHandle, compile_set};
use super::{Ctx, Note, Request, Response, audit_full, record, wall};
use crate::layout::Layout;
use crate::policy::account::{Logins, Mode, Principal};
use crate::policy::pin::{Gate, Refused};
use crate::policy::{Policy, Role, Tier, classify};
use crate::replay;
use crate::store::{ChangeSet, Meta, SnapshotId};

/// Everything the approval path needs.
#[derive(Debug)]
pub struct Approval {
    pub policy: Policy,
    pub gate: Gate,
    pub plant: Plant,
    pub swap: SwapHandle,
}

/// A sim replay of one step, kept so an apply can cite it.
#[derive(Debug, Clone)]
pub struct Evidence {
    pub id: u64,
    /// What ran when it was made.
    pub base: SnapshotId,
    /// What it replayed.
    pub content: SnapshotId,
    /// The recorded window it covered.
    pub window: Duration,
    pub json: Json,
}

/// Replays, bounded; refuses when full.
#[derive(Debug)]
pub struct Replays {
    cap: usize,
    inner: Mutex<Vec<Evidence>>,
}

impl Replays {
    #[must_use]
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            inner: Mutex::new(Vec::new()),
        }
    }

    fn push(&self, mut e: Evidence) -> Option<Evidence> {
        let mut v = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if v.len() >= self.cap {
            return None;
        }
        e.id = v.len() as u64 + 1;
        e.json["replay"] = json!(e.id);
        v.push(e.clone());
        Some(e)
    }

    #[must_use]
    pub fn get(&self, id: u64) -> Option<Evidence> {
        let v = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        usize::try_from(id)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| v.get(i).cloned())
    }
}

/// What an apply or rollback names.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Proposal(&'a str),
    Snapshot(&'a str),
    /// A project, built from its folder as it is now (a deploy).
    Project(&'a str),
}

/// What the approver sends: `{replay?}` with a login session, `{role, pin,
/// replay?}` with the box token on a box without accounts. The PIN never
/// leaves this struct except into the [`Gate`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    role: Option<String>,
    pin: Option<String>,
    replay: Option<u64>,
}

/// Whose approval this is.
enum Approver {
    /// A login session: the account's role, nothing the body says.
    Session(Principal),
    /// A role PIN (only on a box without accounts), checked at step 5.
    Pin(Role, String),
}

impl Approver {
    const fn role(&self) -> Role {
        match self {
            Self::Session(p) => p.role,
            Self::Pin(r, _) => *r,
        }
    }

    /// The person approving, when an account's session is.
    fn person(&self) -> Option<&str> {
        match self {
            Self::Session(p) => p.person(),
            Self::Pin(..) => None,
        }
    }

    /// For details and the store's history: `alice (engineer)`, `operator
    /// (PIN)` for a role-PIN session, or the PIN's role.
    fn who(&self) -> String {
        match self {
            Self::Session(p) => p.label(),
            Self::Pin(r, _) => r.to_string(),
        }
    }
}

/// Who the request approves as, or the refusal: a session must not name
/// a role or PIN (its account's role, or the role its PIN opened, is the
/// one it has), and a PIN is refused once the box has accounts.
fn approver(req: &Request, appr: &Approval, body: Body) -> Result<Approver, (u16, String)> {
    if let Some(p) = &req.session {
        if body.role.is_some() || body.pin.is_some() {
            let what = match p.person() {
                Some(user) => format!("its account ({user}, {})", p.role),
                None => format!("the role its PIN opened ({})", p.role),
            };
            return Err((
                400,
                format!("a login session approves as {what}; send only {{\"replay\": N}}, no role or PIN"),
            ));
        }
        return Ok(Approver::Session(p.clone()));
    }
    match appr.gate.logins().map(Logins::mode) {
        Some(Mode::Accounts(_)) => {
            return Err((
                403,
                "this box has accounts: role PINs are off; log in (POST /api/login) and approve with that session"
                    .into(),
            ));
        }
        Some(Mode::Broken(_)) => {
            return Err((
                503,
                "the box's accounts file cannot be used, so nothing can be approved (see its journal)".into(),
            ));
        }
        None | Some(Mode::Pins) => {}
    }
    let (Some(role), Some(pin)) = (body.role, body.pin) else {
        return Err((
            400,
            "expected JSON {\"role\": ..., \"pin\": ..., \"replay\": N}".into(),
        ));
    };
    let role =
        Role::parse(&role).ok_or((400, "role must be operator, engineer or integrator".to_string()))?;
    Ok(Approver::Pin(role, pin))
}

/// A refusal: audited, then answered (507 if the audit log is full).
fn refuse(ctx: &Ctx, req: &Request, status: u16, body: &Json, detail: &str, note: Note) -> Response {
    match record(ctx, req, status, detail, note) {
        Some(_) => Response::json(status, body),
        None => audit_full(),
    }
}

fn err(why: &str) -> Json {
    json!({ "error": why, "changed": false })
}

/// What every apply, rollback and replay starts with: the target exists
/// (404), the box has an approval path (501), and the target has a change
/// set (409). The approval path, the target change set, and the proposal
/// it came from.
fn prelude<'c>(
    req: &Request,
    ctx: &'c Ctx,
    target_ref: Target<'_>,
    label: &str,
) -> Result<(&'c Approval, ChangeSet, Option<Stored>), Response> {
    let (cs, proposal) = match target_ref {
        Target::Proposal(id) => {
            let Some(p) = super::proposal_id(id).and_then(|id| ctx.box_.proposals.get(id)) else {
                let why = "no such proposal";
                return Err(refuse(
                    ctx,
                    req,
                    404,
                    &err(why),
                    &format!("{label}: {why}"),
                    Note::default(),
                ));
            };
            (p.change.clone(), Some(p))
        }
        Target::Snapshot(id) => match super::snapshot(ctx, id) {
            Ok(cs) => (Some(cs), None),
            Err(r) => {
                let why = "no such snapshot";
                return Err(refuse(
                    ctx,
                    req,
                    r.status,
                    &err(why),
                    &format!("{label}: {why}"),
                    Note::default(),
                ));
            }
        },
        Target::Project(name) => match super::projects::built(ctx, name) {
            Ok(b) => (Some(b.change), None),
            Err((status, body, why)) => {
                return Err(refuse(
                    ctx,
                    req,
                    status,
                    &body,
                    &format!("{label}: {why}"),
                    Note::default(),
                ));
            }
        },
    };
    let note = cs
        .as_ref()
        .map_or_else(Note::default, |cs| Note::target(cs.id(), None));
    let Some(appr) = &ctx.approval else {
        return Err(refuse(
            ctx,
            req,
            501,
            &err("this box has no approval path (approval-policy) configured; nothing was changed"),
            &format!("{label} refused: no approval path on this box"),
            note,
        ));
    };
    let Some(cs) = cs else {
        let why = "this proposal carries no change set: the box does not run module directories or a store";
        return Err(refuse(
            ctx,
            req,
            409,
            &err(why),
            &format!("{label}: no change set"),
            note,
        ));
    };
    Ok((appr, cs, proposal))
}

fn what(target_ref: Target<'_>) -> String {
    match target_ref {
        Target::Proposal(id) => format!("proposal {}", super::clean(id)),
        Target::Snapshot(id) => format!("rollback to {}", super::clean(id)),
        Target::Project(name) => format!("deploy of project {}", super::clean(name)),
    }
}

/// `POST /api/proposals/{id}/replay`, `POST /api/snapshots/{id}/replay`:
/// replays what runs and the target over the recorded window and keeps
/// the comparison as evidence an apply can cite.
#[allow(clippy::too_many_lines)]
pub(super) fn replay(req: &Request, ctx: &Ctx, target_ref: Target<'_>) -> Response {
    let label = format!("replay of {}", what(target_ref));
    let (appr, target, _) = match prelude(req, ctx, target_ref, &label) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let content = target.id();
    let Some(running) = ctx.running() else {
        return refuse(
            ctx,
            req,
            409,
            &err("the box does not run a change set"),
            &format!("{label}: no running change set"),
            Note::target(content, None),
        );
    };
    let base = running.id();
    let note = Note::target(content, Some(base));
    let shape = ctx.shape();
    let Some(tap) = &shape.record else {
        return refuse(
            ctx,
            req,
            409,
            &err("nothing is recorded on this box"),
            &format!("{label}: no recording"),
            note,
        );
    };
    let layout = match reload_layout(&running, &target) {
        Ok(l) => l,
        Err(problems) => {
            return refuse(
                ctx,
                req,
                422,
                &json!({ "error": "the I/O map or calendar does not load", "problems": problems, "changed": false }),
                &format!("{label}: the I/O map or calendar does not load"),
                note,
            );
        }
    };
    let defaults = &appr.plant.defaults;
    let (cur, new) = match (compile_set(&running, defaults), compile_set(&target, defaults)) {
        (Ok(c), Ok(n)) => (c, n),
        (Err(e), _) | (_, Err(e)) => {
            return refuse(
                ctx,
                req,
                422,
                &json!({ "error": "a module does not compile", "problems": e, "changed": false }),
                &format!("{label}: does not compile"),
                note,
            );
        }
    };
    let rec = tap.snapshot();
    let plant = &appr.plant;
    let replayed = if let Some(l) = &layout {
        // What runs, against what runs, and the target against its own
        // map and calendar: a signal the recording lacks is missing there.
        replay::propose_change(&shape.table, cur, &l.table, new, (plant.i_len, plant.q_len), &rec)
            .map(|(p, cov)| (p.current, p.proposed, Some(cov)))
    } else {
        let run = |m| replay::replay(&shape.table, plant.i_len, plant.q_len, m, &rec);
        run(cur).and_then(|c| run(new).map(|n| (c, n, None)))
    };
    let (current, proposed, coverage) = match replayed {
        Ok(x) => x,
        Err(e) => {
            return refuse(
                ctx,
                req,
                422,
                &err(&format!("the replay cannot run: {e}")),
                &format!("{label}: cannot run"),
                note,
            );
        }
    };
    let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    let summary =
        |s: &replay::Summary| json!({ "rises": s.rises, "changes": s.changes, "on_ms": ms(s.on_time) });
    let signals: Vec<Json> = replay::compare(&current, &proposed)
        .iter()
        .map(|d| {
            json!({
                "signal": d.signal,
                "current": summary(&d.current),
                "proposed": summary(&d.proposed),
                "identical": d.identical,
                "extra_rises": d.extra_rises(),
                "current_track": track_json(current.track(&d.signal)),
                "proposed_track": track_json(proposed.track(&d.signal)),
            })
        })
        .collect();
    let faults: Vec<Json> = proposed
        .faults
        .iter()
        .map(|(at, m, f)| json!({ "at_ms": ms(*at), "module": m, "fault": super::render::fault(*f) }))
        .collect();
    let window = rec.end.saturating_sub(rec.start);
    let reload = coverage.map(|cov| {
        let unrecorded: Vec<Json> = cov
            .unrecorded
            .iter()
            .map(|(m, s)| json!({ "module": m, "signal": s }))
            .collect();
        let retyped: Vec<&str> = cov.changed.iter().map(|m| m.signal.as_str()).collect();
        let held: Vec<&str> = {
            let mut v: Vec<&str> = cov.unrecorded.iter().map(|(m, _)| m.as_str()).collect();
            v.dedup();
            v
        };
        json!({
            "io_map_changed": running.io_map != target.io_map,
            "calendar_changed": running.calendar != target.calendar,
            "drivers_changed": running.drivers != target.drivers,
            "unrecorded": unrecorded,
            "retyped": retyped,
            "complete": cov.is_complete(),
            "note": if cov.is_complete() {
                "the proposed modules ran against the proposed I/O map and calendar; the recording fed every signal they read".to_string()
            } else {
                format!(
                    "the proposed modules ran against the proposed I/O map and calendar; signals the recording does not have were missing (or, for box-driven outputs, safe), so {} held wherever they read one",
                    held.join(", ")
                )
            },
        })
    });
    let ev = Evidence {
        id: 0,
        base,
        content,
        window,
        json: json!({
            "base": base.to_string(),
            "content": content.to_string(),
            "start_ms": ms(rec.start),
            "end_ms": ms(rec.end),
            "window_ms": ms(window),
            "ticks": proposed.ticks,
            "signals": signals,
            "faults": faults,
            "reload": reload,
        }),
    };
    let Some(ev) = ctx.replays.push(ev) else {
        return refuse(
            ctx,
            req,
            507,
            &err("too many replays"),
            &format!("{label}: full"),
            note,
        );
    };
    let detail = format!(
        "{label} (replay {}): {} ms window, {} signal(s) differ",
        ev.id,
        ms(window),
        signals.iter().filter(|s| s["identical"] == json!(false)).count()
    );
    match record(ctx, req, 201, &detail, note) {
        Some(_) => Response::json(201, &ev.json).with("Location", format!("/api/replays/{}", ev.id)),
        None => audit_full(),
    }
}

/// Most steps of one signal's track a replay's JSON carries; the
/// summaries are exact however long the track.
pub const MAX_TRACK: usize = 2000;

/// A replayed output as the chart draws it: its type and the value after
/// each tick at which it changed (`[at_ms, value]`, the first tick
/// included; `null` while its module was held or faulted), at most
/// [`MAX_TRACK`] of them (`truncated` past that). `null` when the replay
/// does not drive the signal.
fn track_json(t: Option<&replay::Track>) -> Json {
    let Some(t) = t else { return Json::Null };
    let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
    let steps: Vec<Json> = t
        .changes
        .iter()
        .take(MAX_TRACK)
        .map(|(at, v)| json!([ms(*at), super::render::value(*v)]))
        .collect();
    json!({
        "module": t.module,
        "type": super::render::ty(t.ty),
        "steps": steps,
        "truncated": t.changes.len() > MAX_TRACK,
    })
}

/// Who approved, for the store's history.
fn meta(target: Target<'_>, p: Option<&Stored>, by: &Approver) -> Meta {
    if let Target::Project(name) = target {
        return Meta {
            author: format!("project {name}"),
            prompt: format!("deployed from project {name}"),
            approver: Some(by.who()),
            time_ms: i64::try_from(wall()).unwrap_or(i64::MAX).saturating_mul(1000),
        };
    }
    let (author, prompt) = p.map_or_else(
        || ("box API (rollback)".to_string(), String::new()),
        |p| {
            let author = p.json["manifest"]["author"]
                .as_str()
                .map_or_else(|| "box API (param change)".to_string(), str::to_string);
            let prompt = p.json["manifest"]["prompt"]
                .as_array()
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(Json::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|s| !s.is_empty())
                .or_else(|| p.json["note"].as_str().map(str::to_string))
                .unwrap_or_default();
            (author, prompt)
        },
    );
    Meta {
        author,
        prompt,
        approver: Some(match by {
            Approver::Session(_) => by.who(),
            Approver::Pin(role, _) => format!("{role} (PIN)"),
        }),
        time_ms: i64::try_from(wall()).unwrap_or(i64::MAX).saturating_mul(1000),
    }
}

/// `POST /api/proposals/{id}/apply` and `POST /api/snapshots/{id}/rollback`.
#[allow(clippy::too_many_lines)]
pub(super) fn apply(req: &Request, ctx: &Ctx, target_ref: Target<'_>) -> Response {
    let label = what(target_ref);
    let (appr, target, proposal) = match prelude(req, ctx, target_ref, &label) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let content = target.id();
    let body: Body = match serde_json::from_slice(&req.body) {
        Ok(b) => b,
        // serde's message can quote the body, PIN included: never echoed.
        Err(_) => {
            return refuse(
                ctx,
                req,
                400,
                &err(
                    "expected JSON {\"replay\": N} with a login session, or {\"role\": ..., \"pin\": ..., \"replay\": N}",
                ),
                &format!("{label}: malformed approval"),
                Note::target(content, None),
            );
        }
    };
    let replay_id = body.replay;
    let by = match approver(req, appr, body) {
        Ok(a) => a,
        Err((status, why)) => {
            return refuse(
                ctx,
                req,
                status,
                &err(&why),
                &format!("{label} refused: {why}"),
                Note::target(content, None),
            );
        }
    };
    let role = by.role();
    let who = by.who();
    if ctx.box_.snapshots.is_none() {
        return refuse(
            ctx,
            req,
            409,
            &err("applying commits a snapshot; this box has no store (--store)"),
            &format!("{label}: no snapshot store"),
            Note::target(content, None).by(role),
        );
    }
    // One apply at a time, from here to the commit.
    let _one = ctx.applying.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(running) = ctx.running() else {
        return refuse(
            ctx,
            req,
            409,
            &err("the box does not run a change set"),
            &format!("{label}: no running change set"),
            Note::target(content, None).by(role),
        );
    };
    let base = running.id();
    let note = Note::target(content, Some(base)).by(role);
    if let Some(p) = &proposal
        && p.json["base"].as_str() != Some(base.to_string().as_str())
    {
        return refuse(
            ctx,
            req,
            409,
            &err("stale: the box has changed since this proposal was drafted; draft it again"),
            &format!("{label}: stale (drafted against another snapshot)"),
            note,
        );
    }
    let class = classify(&running, &target);
    let tier = class.tier;
    let needed = appr.policy.rule(tier);
    let reasons: Vec<Json> = class
        .reasons
        .iter()
        .map(|(t, why)| json!({ "tier": t.as_str(), "why": why }))
        .collect();
    let refused = |status: u16, why: &str| {
        refuse(
            ctx,
            req,
            status,
            &json!({ "error": why, "tier": tier.as_str(), "reasons": reasons, "changed": false }),
            &format!("{label} refused ({tier}, {who}): {why}"),
            note,
        )
    };
    if role < needed.role {
        return refused(
            403,
            &format!("a {tier} change needs the {} role or above", needed.role),
        );
    }
    if needed.replay {
        let Some(n) = replay_id else {
            return refused(
                428,
                &format!("a {tier} change needs a sim replay of exactly this change first (POST .../replay)"),
            );
        };
        let Some(ev) = ctx.replays.get(n) else {
            return refused(428, "no such replay");
        };
        if ev.content != content || ev.base != base {
            return refused(
                428,
                "that replay was of different content or another running box; replay this change",
            );
        }
        if ev.window < appr.policy.min_replay {
            return refused(
                428,
                &format!(
                    "that replay covers {} s; the policy wants at least {} s",
                    ev.window.as_secs(),
                    appr.policy.min_replay.as_secs()
                ),
            );
        }
    }
    // A session's password was checked at login, and the session itself
    // (live, account unchanged) before routing.
    let checked = match &by {
        Approver::Session(_) => Ok(()),
        Approver::Pin(role, pin) => appr.gate.check(*role, pin),
    };
    match checked {
        Ok(()) => {}
        Err(Refused::Wrong) => return refused(403, "wrong PIN"),
        Err(Refused::Locked(wait)) => {
            let secs = wait.as_secs().max(1);
            return match record(
                ctx,
                req,
                429,
                &format!("{label} refused ({tier}, {who}): PIN entry locked"),
                note,
            ) {
                Some(_) => Response::json(
                    429,
                    &json!({ "error": "too many wrong PINs; PIN entry is locked", "retry_after_s": secs, "changed": false }),
                )
                .with("Retry-After", secs.to_string()),
                None => audit_full(),
            };
        }
    }
    let defaults = &appr.plant.defaults;
    let (old, new) = match (compile_set(&running, defaults), compile_set(&target, defaults)) {
        (Ok(o), Ok(n)) => (o, n),
        (_, Err(e)) | (Err(e), _) => {
            return refuse(
                ctx,
                req,
                422,
                &json!({ "error": "a module does not compile", "problems": e, "changed": false }),
                &format!("{label} failed ({tier}, {who}): does not compile; nothing was changed"),
                note,
            );
        }
    };
    let layout = match reload_layout(&running, &target) {
        Ok(l) => l,
        Err(problems) => {
            return refuse(
                ctx,
                req,
                422,
                &json!({ "error": "the I/O map or calendar does not load", "problems": problems, "changed": false }),
                &format!(
                    "{label} failed ({tier}, {who}): the I/O map or calendar does not load; nothing was changed"
                ),
                note,
            );
        }
    };
    if let Some(l) = &layout {
        let fits = crate::Runtime::with_signals(appr.plant.i_len, appr.plant.q_len, new.clone(), &l.table);
        if let Err(errs) = fits {
            let problems: Vec<String> = errs.iter().map(ToString::to_string).collect();
            return refuse(
                ctx,
                req,
                409,
                &json!({ "error": "the modules do not fit the new I/O map or calendar", "problems": problems, "tier": tier.as_str(), "changed": false }),
                &format!(
                    "{label} failed ({tier}, {who}): the modules do not fit the new I/O map or calendar; nothing was changed"
                ),
                note,
            );
        }
    }
    if record(
        ctx,
        req,
        202,
        &format!("{label}: approved ({tier}) by {who}; applying"),
        note,
    )
    .is_none()
    {
        return audit_full();
    }
    let landed = match layout {
        None => appr.swap.run(new, target.clone()).map(|c| (c, None)),
        Some(l) => appr.swap.reload(new, target.clone(), l).map(|r| {
            let Reloaded { swapped, links } = r;
            (swapped.catalog, Some((swapped.table, swapped.record, links)))
        }),
    };
    let (catalog, reloaded) = match landed {
        Ok(x) => x,
        Err(e) => {
            let why = e.to_string();
            let _ = record(
                ctx,
                req,
                409,
                &format!("{label} failed ({tier}, {who}): {why}; nothing was changed"),
                note,
            );
            return Response::json(
                409,
                &json!({ "error": why, "tier": tier.as_str(), "changed": false }),
            );
        }
    };
    if let Some((_, _, links)) = &reloaded {
        ctx.links_changed(links);
    }
    let m = meta(target_ref, proposal.as_ref(), &by);
    let committed = {
        let store = ctx.box_.snapshots.as_ref().expect("checked above");
        let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
        match target_ref {
            Target::Proposal(_) | Target::Project(_) => store.commit(&target, &m),
            Target::Snapshot(_) => store.rollback(&content, &m),
        }
    };
    if let Err(e) = committed {
        let back = swap_back(appr, ctx, old, &running, reloaded.is_some());
        let undone = match &back {
            Ok(()) => "the running box was swapped back".to_string(),
            Err(e) => format!("SWAPPING BACK FAILED ({e}): the box runs the new change set uncommitted"),
        };
        let why = format!("the snapshot store refused the commit ({e}); {undone}");
        let _ = record(
            ctx,
            req,
            500,
            &format!("{label} failed ({tier}, {who}): {why}"),
            note,
        );
        return Response::json(500, &json!({ "error": why, "changed": back.is_err() }));
    }
    ctx.set_running(target, catalog);
    let project = match target_ref {
        Target::Project(name) => {
            super::forces::release_all(ctx, "a project was deployed");
            Some(super::projects::deployed(ctx, name, content))
        }
        _ => None,
    };
    let reload = reloaded.map(|(table, record, links)| {
        if let Some(table) = table {
            ctx.set_shape(Shape { table, record });
        }
        json!({
            "links_removed": links.removed,
            "links_added": links.added,
            "safed": links.safed,
            "notes": links.notes,
        })
    });
    if let Some(mut p) = proposal {
        p.json["status"] = json!("applied");
        p.json["applied_by"] = json!(role.as_str());
        if let Some(user) = by.person() {
            p.json["approved_by"] = json!(user);
        }
        ctx.box_.proposals.update(p);
    }
    let undo = format!("/api/snapshots/{base}/rollback");
    let mut detail = format!("{label}: applied ({tier}) by {who}, {base} -> {content}");
    if let Some(r) = &reload {
        let _ = write!(detail, "; I/O map or calendar reloaded");
        for n in r["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            let _ = write!(detail, "; {n}");
        }
    }
    let _ = record(ctx, req, 200, &detail, note);
    Response::json(
        200,
        &json!({
            "applied": true,
            "tier": tier.as_str(),
            "reasons": reasons,
            "role": role.as_str(),
            "user": by.person(),
            "snapshot": content.to_string(),
            "previous": base.to_string(),
            "undo": undo,
            "reload": reload,
            "project": project,
        }),
    )
}

/// The commit failed after the box took the change: puts `running` back
/// (modules `old`, compiled from it), reloading its map and calendar too
/// if the change had reloaded them.
fn swap_back(
    appr: &Approval,
    ctx: &Ctx,
    old: Vec<crate::Module>,
    running: &ChangeSet,
    reloaded: bool,
) -> Result<(), String> {
    if !reloaded {
        return appr
            .swap
            .run(old, running.clone())
            .map(drop)
            .map_err(|e| e.to_string());
    }
    let layout = Layout::build_with(
        running.io_map.as_deref(),
        running.calendar.as_deref(),
        &running.drivers,
    )
    .map_err(|e| e.join("; "))?;
    let r = appr
        .swap
        .reload(old, running.clone(), layout)
        .map_err(|e| e.to_string())?;
    ctx.links_changed(&r.links);
    Ok(())
}

/// The table and calendar a step from `running` to `target` loads, if it
/// changes either or the WASM drivers the map may name (a new driver
/// version reopens its devices' links): `Ok(None)` when all are the
/// running ones.
///
/// # Errors
/// What does not load, file by file.
fn reload_layout(running: &ChangeSet, target: &ChangeSet) -> Result<Option<Layout>, Vec<String>> {
    if running.io_map == target.io_map
        && running.calendar == target.calendar
        && running.drivers == target.drivers
    {
        return Ok(None);
    }
    Layout::build_with(
        target.io_map.as_deref(),
        target.calendar.as_deref(),
        &target.drivers,
    )
    .map(Some)
}

/// The table and recording the box runs now, which replays use: the
/// [`super::Plant`]'s at start, replaced by each reload.
#[derive(Debug, Clone)]
pub struct Shape {
    pub table: crate::SignalTable,
    pub record: Option<crate::record::RecordTap>,
}

/// The tier a proposal's change set would be, against what it was drafted
/// on, for the draft's JSON.
#[must_use]
pub fn tier_json(base: &ChangeSet, target: &ChangeSet, policy: Option<&Policy>) -> Json {
    let c = classify(base, target);
    let rule = policy.map(|p| p.rule(c.tier));
    json!({
        "tier": c.tier.as_str(),
        "reasons": c.reasons.iter().map(|(t, w)| json!({ "tier": t.as_str(), "why": w })).collect::<Vec<_>>(),
        "needs": rule.map(|r| json!({ "role": r.role.as_str(), "replay": r.replay })),
    })
}

/// The policy as the API serves it.
#[must_use]
pub fn policy_json(p: &Policy) -> Json {
    let rule = |t: Tier| {
        let r = p.rule(t);
        json!({ "role": r.role.as_str(), "replay": r.replay })
    };
    json!({
        "param": rule(Tier::Param),
        "logic": rule(Tier::Logic),
        "output-affecting": rule(Tier::OutputAffecting),
        "lockout": { "attempts": p.lockout.attempts, "first_s": p.lockout.first.as_secs() },
        "min_replay_s": p.min_replay.as_secs(),
        "text": p.render(),
    })
}
