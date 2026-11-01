//! The box's WASM driver files ([`crate::driver_dir`]) through the API.
//!
//! ```text
//! GET    /api/drivers          engineer session: [{name, bytes, sha256, problems}]
//! PUT    /api/drivers/{name}   engineer session: the .wasm bytes (application/wasm
//!                              or application/octet-stream); 201 new, 200 replaced
//! DELETE /api/drivers/{name}   engineer session: removes the file
//! ```
//!
//! A driver reaches its device, so every call needs an engineer's login
//! session (the box token and lesser roles are 403), as editing a project
//! does; writes are audited with the person. An upload is held to what a
//! load checks (`helmsman-driver/v1`, nothing else imported, an instance
//! starts under the default limits) and written atomically; nothing that
//! runs changes until the next deploy or map reload loads it. A file the
//! running map loads from the box (no project driver in front of it) is
//! not deleted (409): the next load would refuse the map.

use serde_json::{Value as Json, json};

use super::{Ctx, Request, Response, audited, clean};
use crate::driver_dir::{DriverDir, Info, Refusal, Sources, driver_name};
use crate::policy::Role;
use crate::policy::account::Principal;

fn info_json(i: &Info) -> Json {
    json!({
        "name": i.name,
        "map_name": format!("{}{}", helmsman_io::driver::WASM, i.name),
        "bytes": i.bytes,
        "sha256": i.sha256,
        "ok": i.problems.is_empty(),
        "problems": i.problems,
    })
}

/// Why the call is refused unless it carries an engineer's (or above)
/// login session.
fn not_engineer(session: Option<&Principal>, what: &str) -> Option<String> {
    match session {
        Some(p) if p.role >= Role::Engineer => None,
        Some(p) => Some(format!(
            "{what} needs an engineer login; this session is {}",
            p.label()
        )),
        None => Some(format!(
            "{what} needs an engineer login session (POST /api/login); the box token does not manage drivers"
        )),
    }
}

/// `GET /api/drivers`. A `GET` is served before authentication, so the
/// session is looked up here: the bearer when one is sent, else the one
/// cookie (as `GET /api/session` does).
pub(super) fn list(req: &Request, ctx: &Ctx) -> Response {
    let Some(dir) = &ctx.driver_dir else {
        return Response::error(404, "this box keeps no driver files");
    };
    let token = if req.authorization {
        req.bearer.as_deref()
    } else {
        match req.cookies.as_slice() {
            [t] => Some(t.as_str()),
            _ => None,
        }
    };
    let session = token.and_then(|t| super::logins(ctx)?.session(t).ok());
    if let Some(why) = not_engineer(session.as_ref(), "listing drivers") {
        return Response::error(403, &why);
    }
    match dir.list() {
        Ok(all) => Response::json(
            200,
            &json!({ "drivers": all.iter().map(info_json).collect::<Vec<_>>() }),
        ),
        Err(r) => Response::error(r.status, &r.why),
    }
}

fn refusal(r: &Refusal) -> Json {
    if r.problems.is_empty() {
        json!({ "error": r.why })
    } else {
        json!({ "error": r.why, "problems": r.problems })
    }
}

/// The non-`GET` routes under `/api/drivers`; every outcome audited.
pub(super) fn mutate(req: &Request, ctx: &Ctx, rest: &[&str]) -> Response {
    let method = req.method.as_str();
    let done = |status: u16, body: &Json, detail: String| audited(ctx, req, status, body, &detail);
    let name = match rest {
        ["drivers", name] if matches!(method, "PUT" | "DELETE") => *name,
        ["drivers"] => {
            return done(
                405,
                &json!({ "error": "method not allowed" }),
                "driver: method not allowed".into(),
            )
            .with("Allow", "GET".into());
        }
        ["drivers", _] => {
            return done(
                405,
                &json!({ "error": "method not allowed" }),
                "driver: method not allowed".into(),
            )
            .with("Allow", "PUT, DELETE".into());
        }
        _ => {
            return done(
                404,
                &json!({ "error": "no such resource" }),
                "driver: no such resource".into(),
            );
        }
    };
    let what = format!("driver {}", clean(name));
    let Some(dir) = &ctx.driver_dir else {
        return done(
            404,
            &json!({ "error": "this box keeps no driver files" }),
            format!("{what}: no driver folder on this box"),
        );
    };
    if let Some(why) = not_engineer(
        req.session.as_ref(),
        if method == "PUT" {
            "uploading a driver"
        } else {
            "deleting a driver"
        },
    ) {
        return done(
            403,
            &json!({ "error": why }),
            format!("{what}: refused, not an engineer"),
        );
    }
    if method == "PUT" {
        put(req, dir, name, &what, &done)
    } else {
        delete(ctx, dir, name, &what, &done)
    }
}

fn put(
    req: &Request,
    dir: &DriverDir,
    name: &str,
    what: &str,
    done: &dyn Fn(u16, &Json, String) -> Response,
) -> Response {
    let wasm_type = req
        .content_type
        .as_deref()
        .is_some_and(|t| matches!(t, "application/wasm" | "application/octet-stream"));
    if !wasm_type {
        return done(
            415,
            &json!({ "error": "a driver upload is application/wasm (or application/octet-stream): the .wasm bytes" }),
            format!("{what}: upload not a binary"),
        );
    }
    match dir.put(name, &req.body) {
        Ok((info, replaced)) => {
            let mut body = info_json(&info);
            body["replaced"] = json!(replaced);
            body["takes_effect"] = json!("at the next deploy or map reload; what runs now is untouched");
            done(
                if replaced { 200 } else { 201 },
                &body,
                format!(
                    "{what}: {} ({} bytes, sha256 {})",
                    if replaced { "replaced" } else { "uploaded" },
                    info.bytes,
                    info.sha256
                ),
            )
        }
        Err(r) => done(
            r.status,
            &refusal(&r),
            format!("{what}: upload refused: {}", r.why),
        ),
    }
}

/// Whether the running map loads driver `name` from the box's folder: it
/// names `wasm:NAME` and no project driver comes first.
fn in_use(ctx: &Ctx, name: &str) -> bool {
    let Some(map) = ctx
        .running()
        .and_then(|cs| cs.io_map)
        .and_then(|m| helmsman_io::text::parse(&m).ok())
    else {
        return false;
    };
    let named = format!("{}{name}", helmsman_io::driver::WASM);
    if !map.map.devices.iter().any(|d| d.driver == named) {
        return false;
    }
    let Sources { project, .. } = ctx.sources(None);
    let from_project = project.is_some_and(|(_, dir)| {
        crate::driver_dir::read_folder(&dir.join(crate::driver_dir::DIR)).is_ok_and(|d| d.contains_key(name))
    });
    !from_project
}

fn delete(
    ctx: &Ctx,
    dir: &DriverDir,
    name: &str,
    what: &str,
    done: &dyn Fn(u16, &Json, String) -> Response,
) -> Response {
    if driver_name(name) && in_use(ctx, name) {
        return done(
            409,
            &json!({ "error": format!("the running I/O map names wasm:{name}, loaded from this file; deploy a map without it first") }),
            format!("{what}: not deleted, the running map uses it"),
        );
    }
    match dir.delete(name) {
        Ok(()) => done(200, &json!({ "deleted": name }), format!("{what}: deleted")),
        Err(r) => done(r.status, &refusal(&r), format!("{what}: not deleted: {}", r.why)),
    }
}
