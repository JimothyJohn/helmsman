//! Forcing inputs ([`crate::force`]) through the API.
//!
//! ```text
//! GET    /api/forces          every active force: {id, signal | address, value, by, at}
//! POST   /api/forces          engineer session: {signal, value} or {address, value}
//! DELETE /api/forces/{id}     engineer session: release one
//! DELETE /api/forces          engineer session: release all
//! ```
//!
//! A force belongs to the session that made it: it ends when that session
//! logs out, expires, or is revoked (the API sweeps for those every
//! second), and every force ends when a project is deployed. Making,
//! replacing and releasing are each audited, an automatic release too
//! (`by the box`). Reads mark what is forced: `forced: true` on a forced
//! signal (`/api/signals`, the values stream), and a `forces` list on the
//! values stream and on every trace.

use serde_json::{Value as Json, json};

use super::{Ctx, Request, Response, audit_full, audited, clean, is_json, wall};
use crate::force::{Address, Force, Point};
use crate::policy::Role;
use crate::policy::account::session_key;

/// A force as the API shows it.
#[must_use]
pub fn force_json(f: &Force) -> Json {
    let (signal, address) = match &f.point {
        Point::Signal(s) => (Some(s.clone()), None),
        Point::Address(a) => (None, Some(a.to_string())),
    };
    json!({
        "id": f.id,
        "signal": signal,
        "address": address,
        "value": super::render::value(Some(f.value)),
        "by": f.by,
        "at": f.at,
    })
}

/// Every force, for reads.
pub(super) fn list(ctx: &Ctx) -> Json {
    Json::Array(
        ctx.forces
            .as_ref()
            .map(|f| f.list().iter().map(force_json).collect())
            .unwrap_or_default(),
    )
}

/// `GET /api/forces`.
pub(super) fn get(ctx: &Ctx) -> Response {
    match &ctx.forces {
        Some(_) => Response::json(200, &json!({ "forces": list(ctx) })),
        None => Response::error(404, "this box does not force inputs"),
    }
}

/// `body` with `forced` set on each signal object in it (an array of
/// signals, or one signal).
pub(super) fn mark(ctx: &Ctx, mut body: Json) -> Json {
    let Some(forces) = &ctx.forces else {
        return body;
    };
    let mark_one = |s: &mut Json| {
        if let Some(name) = s["name"].as_str() {
            let on = forces.is_forced(name);
            s["forced"] = json!(on);
        }
    };
    match &mut body {
        Json::Array(items) => items.iter_mut().for_each(mark_one),
        Json::Object(_) => mark_one(&mut body),
        _ => {}
    }
    body
}

fn session_token(req: &Request) -> Option<&str> {
    if req.by_cookie {
        req.cookies.first().map(String::as_str)
    } else {
        req.bearer.as_deref()
    }
}

fn value_of(v: &Json) -> Option<helmsman_io::Value> {
    match v {
        Json::Bool(b) => Some(helmsman_io::Value::Bool(*b)),
        Json::Number(n) => n
            .as_i64()
            .map(helmsman_io::Value::Int)
            .or_else(|| n.as_f64().filter(|x| x.is_finite()).map(helmsman_io::Value::Real)),
        _ => None,
    }
}

/// The non-`GET` routes under `/api/forces`; every outcome audited.
pub(super) fn mutate(req: &Request, ctx: &Ctx, rest: &[&str]) -> Response {
    let refuse =
        |status: u16, why: &str, detail: &str| audited(ctx, req, status, &json!({ "error": why }), detail);
    let Some(forces) = &ctx.forces else {
        return refuse(404, "this box does not force inputs", "force: not on this box");
    };
    let who = match &req.session {
        Some(p) if p.role >= Role::Engineer => p.clone(),
        Some(p) => {
            return refuse(
                403,
                &format!("forcing needs an engineer login; this session is {}", p.label()),
                "force: refused, not an engineer",
            );
        }
        None => {
            return refuse(
                403,
                "forcing needs an engineer login session (POST /api/login); the box token does not force",
                "force: refused, not an engineer",
            );
        }
    };
    match (req.method.as_str(), rest) {
        ("POST", ["forces"]) => {
            let Some(token) = session_token(req) else {
                return refuse(403, "forcing needs a login session", "force: no session");
            };
            let point = match parse(req, ctx) {
                Ok(p) => p,
                Err((status, why)) => return refuse(status, &why, "force: refused"),
            };
            let (point, value) = point;
            let made = forces.add(Force {
                id: 0,
                point,
                value,
                by: who.label(),
                session: session_key(token),
                at: wall(),
            });
            match made {
                Ok((f, replaced)) => {
                    let detail = format!(
                        "force {}: {} := {}{}",
                        f.id,
                        f.point,
                        show(f.value),
                        replaced.map_or_else(String::new, |r| format!(" (replaces force {})", r.id))
                    );
                    audited(ctx, req, 201, &force_json(&f), &detail)
                }
                Err(e) => refuse(507, &e, "force: too many"),
            }
        }
        ("DELETE", ["forces"]) => {
            let gone = forces.release_all();
            let ids: Vec<u64> = gone.iter().map(|f| f.id).collect();
            audited(
                ctx,
                req,
                200,
                &json!({ "released": ids }),
                &format!("forces released: all ({})", ids.len()),
            )
        }
        ("DELETE", ["forces", id]) => {
            let id = (!id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit()))
                .then(|| id.parse::<u64>().ok())
                .flatten();
            match id.and_then(|id| forces.release(id)) {
                Some(f) => audited(
                    ctx,
                    req,
                    200,
                    &json!({ "released": [f.id] }),
                    &format!("force {} released: {}", f.id, f.point),
                ),
                None => refuse(404, "no such force", "force release: no such force"),
            }
        }
        _ => {
            let r = Response::error(405, "method not allowed").with(
                "Allow",
                if rest.len() == 1 {
                    "GET, POST, DELETE"
                } else {
                    "DELETE"
                }
                .into(),
            );
            match super::audit(ctx, req, 405, "method not allowed") {
                Some(_) => r,
                None => audit_full(),
            }
        }
    }
}

fn show(v: helmsman_io::Value) -> String {
    match v {
        helmsman_io::Value::Bool(b) => b.to_string(),
        helmsman_io::Value::Int(n) => n.to_string(),
        helmsman_io::Value::Real(x) => x.to_string(),
    }
}

/// `{signal, value}` or `{address, value}`, checked against what runs.
fn parse(req: &Request, ctx: &Ctx) -> Result<(Point, helmsman_io::Value), (u16, String)> {
    if !is_json(req) {
        return Err((415, "Content-Type must be application/json".into()));
    }
    let Ok(Json::Object(m)) = serde_json::from_slice::<Json>(&req.body) else {
        return Err((400, "expected {signal, value} or {address, value}".into()));
    };
    if let Some(k) = m
        .keys()
        .find(|k| !matches!(k.as_str(), "signal" | "address" | "value"))
    {
        return Err((400, format!("unknown field {:?}", clean(k))));
    }
    let value = m
        .get("value")
        .and_then(value_of)
        .ok_or((400, "value: true/false or a number".to_string()))?;
    match (
        m.get("signal").and_then(Json::as_str),
        m.get("address").and_then(Json::as_str),
    ) {
        (Some(name), None) => {
            let c = ctx.catalog();
            let s = c
                .signal(name)
                .map(|i| &c.signals[i])
                .ok_or((404, "no such signal".to_string()))?;
            if s.dir != helmsman_io::Dir::In || s.writer.as_deref().is_some_and(|w| w != "calendar") {
                return Err((409, format!("{} is not an input: only inputs are forced", s.name)));
            }
            let value = match (s.ty, value) {
                (helmsman_io::Ty::Real, helmsman_io::Value::Int(n)) => {
                    #[allow(clippy::cast_precision_loss)] // a typed-in number
                    helmsman_io::Value::Real(n as f64)
                }
                (_, v) => v,
            };
            if value.ty() != s.ty {
                return Err((
                    422,
                    format!("{} is {}, the value is not", s.name, super::render::ty(s.ty)),
                ));
            }
            Ok((Point::Signal(s.name.clone()), value))
        }
        (None, Some(a)) => {
            let len = ctx.approval.as_ref().map_or(0, |a| a.plant.i_len);
            let addr = Address::parse(a, len).map_err(|e| (422, e))?;
            if !addr.admits(value) {
                return Err((422, format!("{addr} cannot hold that value")));
            }
            Ok((Point::Address(addr), value))
        }
        _ => Err((400, "name one of signal or address".into())),
    }
}

/// Ends the forces of sessions that are gone (logged out, expired,
/// revoked), each audited as the box's. Called by the API about once a
/// second, and at logout.
pub(super) fn sweep(ctx: &Ctx) {
    let (Some(forces), Some(logins)) = (&ctx.forces, super::logins(ctx)) else {
        return;
    };
    let dead: Vec<[u8; 32]> = forces
        .sessions()
        .into_iter()
        .filter(|k| !logins.live(k))
        .collect();
    if dead.is_empty() {
        return;
    }
    let gone = forces.release_where(|f| dead.contains(&f.session));
    for f in gone {
        system_audit(ctx, &f, "its session ended");
    }
}

/// Releases every force (a deploy): each audited as the box's.
pub(super) fn release_all(ctx: &Ctx, why: &str) {
    if let Some(forces) = &ctx.forces {
        for f in forces.release_all() {
            system_audit(ctx, &f, why);
        }
    }
}

fn system_audit(ctx: &Ctx, f: &Force, why: &str) {
    let _ = ctx.box_.audit.append(super::AuditEntry {
        seq: 0,
        at: wall(),
        method: "-".into(),
        path: format!("/api/forces/{}", f.id),
        status: 200,
        detail: format!(
            "force {} released by the box ({why}): {} (forced by {})",
            f.id, f.point, f.by
        ),
        snapshot: None,
        base: None,
        role: None,
        source: "box".into(),
        user: None,
    });
    (ctx.cfg.log)(&format!("api: force {} released ({why}): {}", f.id, f.point));
}
