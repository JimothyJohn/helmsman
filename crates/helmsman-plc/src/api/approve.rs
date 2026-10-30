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
//! 6. Every module of the target compiles (else 422); the I/O map and
//!    calendar are the running ones (a map or calendar reload is not built:
//!    409).
//! 7. The scan thread swaps the whole module set between ticks
//!    ([`super::swap`]); if it refuses, nothing changed (409).
//! 8. The snapshot store commits the target with author, prompt, approver
//!    and time; if that fails the scan thread is swapped back (500).
//!
//! Applies are serialized: two racing applies of conflicting proposals see
//! one win and the other refused as stale, never half of each. Every
//! outcome is in the audit log with the role, the tier and both snapshot
//! ids; an apply that acts writes an entry before (so a full log stops it)
//! and one after.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::store::Stored;
use super::swap::{Plant, SwapError, SwapHandle, compile_set};
use super::{Ctx, Note, Request, Response, audit_full, record, wall};
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
}

/// What the approver sends. The PIN never leaves this struct except into
/// the [`Gate`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    role: String,
    pin: String,
    replay: Option<u64>,
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
    let Some(tap) = &appr.plant.record else {
        return refuse(
            ctx,
            req,
            409,
            &err("nothing is recorded on this box"),
            &format!("{label}: no recording"),
            note,
        );
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
    let run = |m| replay::replay(&plant.table, plant.i_len, plant.q_len, m, &rec);
    let (current, proposed) = match run(cur).and_then(|c| run(new).map(|n| (c, n))) {
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
            })
        })
        .collect();
    let faults: Vec<Json> = proposed
        .faults
        .iter()
        .map(|(at, m, f)| json!({ "at_ms": ms(*at), "module": m, "fault": super::render::fault(*f) }))
        .collect();
    let window = rec.end.saturating_sub(rec.start);
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

/// Who approved, for the store's history.
fn meta(p: Option<&Stored>, role: Role) -> Meta {
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
        approver: Some(format!("{role} (PIN)")),
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
                &err("expected JSON {\"role\": ..., \"pin\": ..., \"replay\": N}"),
                &format!("{label}: malformed approval"),
                Note::target(content, None),
            );
        }
    };
    let Some(role) = Role::parse(&body.role) else {
        return refuse(
            ctx,
            req,
            400,
            &err("role must be operator, engineer or integrator"),
            &format!("{label}: unknown role"),
            Note::target(content, None),
        );
    };
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
            &format!("{label} refused ({tier}, {role}): {why}"),
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
        let Some(n) = body.replay else {
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
    if running.io_map != target.io_map || running.calendar != target.calendar {
        return refused(
            409,
            "changing the I/O map or the calendar on a running box is not built (card map-reload); nothing was changed",
        );
    }
    match appr.gate.check(role, &body.pin) {
        Ok(()) => {}
        Err(Refused::Wrong) => return refused(403, "wrong PIN"),
        Err(Refused::Locked(wait)) => {
            let secs = wait.as_secs().max(1);
            return match record(
                ctx,
                req,
                429,
                &format!("{label} refused ({tier}, {role}): PIN entry locked"),
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
                &format!("{label} failed ({tier}, {role}): does not compile; nothing was changed"),
                note,
            );
        }
    };
    if record(
        ctx,
        req,
        202,
        &format!("{label}: approved ({tier}) by {role}; applying"),
        note,
    )
    .is_none()
    {
        return audit_full();
    }
    let catalog = match appr.swap.run(new, target.clone()) {
        Ok(c) => c,
        Err(e) => {
            let why = match e {
                SwapError::Refused(errs) => format!("the running box refused it: {}", errs.join("; ")),
                SwapError::NotTaken => "the scan thread did not take it in time".into(),
            };
            let _ = record(
                ctx,
                req,
                409,
                &format!("{label} failed ({tier}, {role}): {why}; nothing was changed"),
                note,
            );
            return Response::json(
                409,
                &json!({ "error": why, "tier": tier.as_str(), "changed": false }),
            );
        }
    };
    let m = meta(proposal.as_ref(), role);
    let committed = {
        let store = ctx.box_.snapshots.as_ref().expect("checked above");
        let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
        match target_ref {
            Target::Proposal(_) => store.commit(&target, &m),
            Target::Snapshot(_) => store.rollback(&content, &m),
        }
    };
    if let Err(e) = committed {
        let back = appr.swap.run(old, running.clone());
        let undone = if back.is_ok() {
            "the running box was swapped back"
        } else {
            "SWAPPING BACK FAILED: the box runs the new modules uncommitted"
        };
        let why = format!("the snapshot store refused the commit ({e}); {undone}");
        let _ = record(
            ctx,
            req,
            500,
            &format!("{label} failed ({tier}, {role}): {why}"),
            note,
        );
        return Response::json(500, &json!({ "error": why, "changed": back.is_err() }));
    }
    ctx.set_running(target, catalog);
    if let Some(mut p) = proposal {
        p.json["status"] = json!("applied");
        p.json["applied_by"] = json!(role.as_str());
        ctx.box_.proposals.update(p);
    }
    let undo = format!("/api/snapshots/{base}/rollback");
    let detail = format!("{label}: applied ({tier}) by {role}, {base} -> {content}");
    let _ = record(ctx, req, 200, &detail, note);
    Response::json(
        200,
        &json!({
            "applied": true,
            "tier": tier.as_str(),
            "reasons": reasons,
            "role": role.as_str(),
            "snapshot": content.to_string(),
            "previous": base.to_string(),
            "undo": undo,
        }),
    )
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
