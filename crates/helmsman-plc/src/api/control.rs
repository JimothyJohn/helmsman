//! Stop, run and restart one module of the running box (card
//! monitor-box-run-stop: the Monitor's buttons).
//!
//! ```text
//! POST /api/modules/{name}/stop      not scanned; outputs zero, signals safe
//! POST /api/modules/{name}/run       scanned again, state as it was (clears a fault)
//! POST /api/modules/{name}/restart   a cold restart: initial values, then run
//! ```
//!
//! Each moves outputs, so each takes what the approval policy asks of an
//! output-affecting change, without the replay: an engineer login session
//! (the box token and a PIN alone do not) of at least the role the policy
//! names for `output-affecting`. Every outcome is audited, a refusal too.
//! The scan thread does it between two scans ([`super::swap::Control`]).
//!
//! A stop lasts until a run or a restart: a deploy keeps the module
//! stopped, and the box restarting starts every module running. Reads
//! show it: `state: "stopped"` with `stopped_by` and `stopped_at` on the
//! module (`/api/modules`, `/api/modules/{name}`, the values stream).

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};

use super::swap::{Control, SwapError};
use super::{Ctx, Request, Response, audited, wall};
use crate::Status;
use crate::policy::Tier;

/// Who stopped which module, and when (seconds since the epoch), for
/// reads; an entry whose module runs again is not shown.
pub(super) type Stops = BTreeMap<String, (String, u64)>;

/// `POST /api/modules/{name}/{op}`; every outcome audited.
pub(super) fn post(req: &Request, ctx: &Ctx, name: &str, op: &str) -> Response {
    let refuse =
        |status: u16, why: &str, detail: &str| audited(ctx, req, status, &json!({ "error": why }), detail);
    let Some(control) = Control::parse(op) else {
        return refuse(
            404,
            "a module takes stop, run or restart",
            "module control: no such control",
        );
    };
    let Some(approval) = &ctx.approval else {
        return refuse(
            404,
            "this box takes no changes, so its modules cannot be stopped from here",
            "module control: not on this box",
        );
    };
    let least = approval.policy.rule(Tier::OutputAffecting).role;
    let who = match &req.session {
        Some(p) if p.role >= least && p.role >= crate::policy::Role::Engineer => p.clone(),
        Some(p) => {
            return refuse(
                403,
                &format!(
                    "stopping or starting a module needs a {least} login; this session is {}",
                    p.label()
                ),
                &format!("module control: {op} refused, role below {least}"),
            );
        }
        None => {
            return refuse(
                403,
                &format!(
                    "stopping or starting a module needs a {least} login session (POST /api/login); the box token does not"
                ),
                &format!("module control: {op} refused, no session"),
            );
        }
    };
    let known = ctx.catalog().modules.iter().any(|m| m.name == name);
    if !known {
        return refuse(
            404,
            "no such module",
            &format!("module control: {op}, no such module"),
        );
    }
    match approval.swap.control(name, control) {
        Ok(status) => {
            let by = who.label();
            let at = wall();
            {
                let mut stops = ctx
                    .stops
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if status == Status::Stopped {
                    stops.insert(name.to_string(), (by.clone(), at));
                } else {
                    stops.remove(name);
                }
            }
            let mut state = super::render::state(status);
            if status == Status::Stopped {
                state["stopped_by"] = json!(by);
                state["stopped_at"] = json!(at);
            }
            let body = json!({ "module": name, "control": op, "status": state });
            let did = match control {
                Control::Stop => "stopped",
                Control::Run => "run",
                Control::Restart => "restarted",
            };
            audited(ctx, req, 200, &body, &format!("module {name} {did}"))
        }
        Err(SwapError::Refused(errs)) => refuse(
            404,
            &errs.join("; "),
            &format!("module control: {op} {name} refused"),
        ),
        Err(e) => refuse(
            503,
            &e.to_string(),
            &format!("module control: {op} {name} not taken"),
        ),
    }
}

/// `module` (one rendered module: `name`, `status.state`) with who
/// stopped it in its `status`, if it is stopped and the box knows.
fn mark_one(stops: &Stops, module: &mut Json) {
    if module["status"]["state"] != "stopped" {
        return;
    }
    let Some(name) = module["name"].as_str() else {
        return;
    };
    if let Some((by, at)) = stops.get(name) {
        module["status"]["stopped_by"] = json!(by);
        module["status"]["stopped_at"] = json!(at);
    }
}

/// Every module as `/api/modules` and the values stream show it.
pub(super) fn modules(ctx: &Ctx, c: &super::Catalog, view: &super::View) -> Json {
    mark(ctx, super::render::modules(c, view))
}

/// Module `k` as `/api/modules/{name}` shows it.
pub(super) fn detail(ctx: &Ctx, c: &super::Catalog, view: &super::View, k: usize) -> Json {
    mark(
        ctx,
        super::render::module_detail(&c.modules[k], view.modules.get(k)),
    )
}

/// [`mark_one`] over an array of modules, or one module.
fn mark(ctx: &Ctx, mut body: Json) -> Json {
    let stops = ctx
        .stops
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match &mut body {
        Json::Array(items) => items.iter_mut().for_each(|m| mark_one(&stops, m)),
        Json::Object(_) => mark_one(&stops, &mut body),
        _ => {}
    }
    body
}
