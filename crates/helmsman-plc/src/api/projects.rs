//! The project routes ([`crate::project`]): the editor's files on the box,
//! and deploying one project.
//!
//! ```text
//! GET    /api/projects                        every project, and which runs
//! POST   /api/projects                        engineer: {name} a new empty project, or {name, from} a copy
//! GET    /api/projects/{p}                    its files: name, bytes, version (SHA-256)
//! DELETE /api/projects/{p}                    engineer: delete it (never the running one)
//! POST   /api/projects/{p}/rename             engineer: {to}
//! GET    /api/projects/{p}/files/{f}          {name, text, version}
//! PUT    /api/projects/{p}/files/{f}          engineer: {text, version?}: save (a draft); version = what was read, null = new
//! DELETE /api/projects/{p}/files/{f}          engineer
//! POST   /api/projects/{p}/check              token or session: build it as a deploy would; nothing changes
//! POST   /api/projects/{p}/replay             engineer: sim replay of deploying it (evidence a deploy can cite)
//! POST   /api/projects/{p}/deploy             engineer: {replay?}: build, apply through the approval policy, commit, hot-swap
//! GET    /api/projects/{p}/zip                the project as a .zip
//! POST   /api/projects/{p}/zip[?replace=true] engineer: a .zip as project p (new, or in place of it)
//! POST   /api/projects/{p}/import?file=F[&replace=true]
//!                                             engineer: one file into p: F.st as it is, F.xml (PLCopen) lowered to F.st
//! GET    /api/running                         which project was deployed last, and whether it is what runs
//! ```
//!
//! **The engineer gate.** Every route that writes (create, copy, rename,
//! delete, save, upload, import, replay, deploy) needs a login session
//! whose role is engineer or above: an account's, or (on a box without
//! accounts) the engineer's role-PIN session. The box token alone is a
//! machine's and never edits a project (403), so the agent can draft
//! proposals but not touch the editor's files. Reads follow the API's
//! read rule (open, like every `GET`). Deploy goes through the same
//! approval path as an apply ([`super::approve`]): the policy's rule for
//! the change's tier (role, and a sim replay where the policy wants one),
//! compile, swap between scans, commit; a deploy that does not build
//! changes nothing. Every call is audited.

use serde_json::{Value as Json, json};

use super::approve::{self, Approval, Target};
use super::{Ctx, Request, Response, audit, audit_full, audited, clean, is_json};
use crate::policy::Role;
use crate::project::build::{self, Built, Shape};
use crate::project::{self, FileKind, Files, Projects, Refusal, Running};
use crate::store::SnapshotId;

fn store(ctx: &Ctx) -> Result<&Projects, Response> {
    ctx.projects
        .as_ref()
        .ok_or_else(|| Response::error(404, "no projects on this box (--projects)"))
}

fn refusal(r: &Refusal) -> Response {
    Response::error(r.status, &r.why)
}

fn running_json(ctx: &Ctx, p: &Projects) -> Json {
    let now = ctx.running().map(|cs| cs.id().to_string());
    match p.running() {
        Some(Running { name, snapshot }) => json!({
            "project": name,
            "snapshot": snapshot,
            "running": now,
            "current": now.as_deref() == Some(snapshot.as_str()),
        }),
        None => json!({ "project": null, "snapshot": null, "running": now, "current": false }),
    }
}

/// The `GET` routes under `/api/projects` and `/api/running`.
pub(super) fn get(ctx: &Ctx, rest: &[&str]) -> Response {
    let p = match store(ctx) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let current = p.running().map(|r| r.name);
    match rest {
        ["running"] => Response::json(200, &running_json(ctx, p)),
        ["projects"] => match p.list() {
            Ok(names) => Response::json(
                200,
                &json!({
                    "projects": names.iter().map(|n| json!({
                        "name": n,
                        "running": current.as_deref() == Some(n.as_str()),
                    })).collect::<Vec<_>>(),
                    "running": running_json(ctx, p),
                }),
            ),
            Err(r) => refusal(&r),
        },
        ["projects", name] => match p.files(name) {
            Ok(files) => Response::json(
                200,
                &json!({
                    "name": name,
                    "running": current.as_deref() == Some(*name),
                    "files": files.iter().map(|f| json!({
                        "name": f.name, "bytes": f.bytes, "version": f.version,
                    })).collect::<Vec<_>>(),
                }),
            ),
            Err(r) => refusal(&r),
        },
        ["projects", name, "files", file] => match p.file(name, file) {
            Ok(text) => Response::json(
                200,
                &json!({ "name": file, "version": project::version(text.as_bytes()), "text": text }),
            ),
            Err(r) => refusal(&r),
        },
        ["projects", name, "zip"] => match p.zip(name) {
            Ok(bytes) => Response {
                status: 200,
                content_type: "application/zip",
                headers: vec![(
                    "Content-Disposition",
                    format!("attachment; filename=\"{name}.zip\""),
                )],
                body: bytes,
            },
            Err(r) => refusal(&r),
        },
        _ => Response::error(404, "no such resource"),
    }
}

/// The methods a project path takes, for 405s.
pub(super) fn allowed(path: &[&str]) -> Option<&'static str> {
    match path {
        ["api", "projects"] | ["api", "projects", _, "zip"] => Some("GET, POST"),
        ["api", "projects", _] => Some("GET, DELETE"),
        ["api", "projects", _, "files", _] => Some("GET, PUT, DELETE"),
        [
            "api",
            "projects",
            _,
            "rename" | "check" | "replay" | "deploy" | "import",
        ] => Some("POST"),
        ["api", "running"] => Some("GET"),
        _ => None,
    }
}

/// Refused unless the call carries a login session of an engineer or
/// above.
fn engineer(ctx: &Ctx, req: &Request, what: &str) -> Result<(), Response> {
    let why = match &req.session {
        Some(p) if p.role >= Role::Engineer => return Ok(()),
        Some(p) => format!("{what} needs an engineer login; this session is {}", p.label()),
        None => format!(
            "{what} needs an engineer login session (POST /api/login); the box token does not edit projects"
        ),
    };
    Err(audited(
        ctx,
        req,
        403,
        &json!({ "error": why }),
        &format!("{what}: refused, not an engineer"),
    ))
}

fn json_body(req: &Request) -> Result<serde_json::Map<String, Json>, &'static str> {
    if !is_json(req) {
        return Err("Content-Type must be application/json");
    }
    match serde_json::from_slice(&req.body) {
        Ok(Json::Object(m)) => Ok(m),
        _ => Err("expected a JSON object"),
    }
}

/// Only these keys, each a string (or, with `nullable`, null).
fn keys(m: &serde_json::Map<String, Json>, allowed: &[&str]) -> Result<(), String> {
    for (k, v) in m {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("unknown field {:?}", clean(k)));
        }
        if !(v.is_string() || v.is_null() && k == "version") {
            return Err(format!("{k} must be a string"));
        }
    }
    Ok(())
}

fn query<'a>(req: &'a Request, key: &str) -> Option<&'a str> {
    req.query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn replace(req: &Request) -> Result<bool, &'static str> {
    match query(req, "replace") {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err("replace is true or false"),
    }
}

/// The non-`GET` routes under `/api/projects`; every outcome audited.
#[allow(clippy::too_many_lines)]
pub(super) fn mutate(req: &Request, ctx: &Ctx, rest: &[&str]) -> Response {
    let method = req.method.as_str();
    let what = match (method, rest) {
        ("POST", ["projects"]) => "create project".to_string(),
        (_, ["projects", name, ..]) => format!("project {}", clean(name)),
        _ => "projects".to_string(),
    };
    let Some(p) = &ctx.projects else {
        let r = Response::error(404, "no projects on this box (--projects)");
        return match audit(ctx, req, 404, &format!("{what}: no projects on this box")) {
            Some(_) => r,
            None => audit_full(),
        };
    };
    if let ("POST", ["projects", _, "check"]) = (method, rest) {
        return check(req, ctx, rest[1]);
    }
    if let Err(r) = engineer(ctx, req, &what) {
        return r;
    }
    let done = |status: u16, body: Json, detail: String| audited(ctx, req, status, &body, &detail);
    let failed = |r: Refusal, detail: &str| {
        done(
            r.status,
            json!({ "error": r.why }),
            format!("{what}: {detail}: {}", r.why),
        )
    };
    match (method, rest) {
        ("POST", ["projects"]) => {
            let m = match json_body(req) {
                Ok(m) => m,
                Err(e) => return done(400, json!({ "error": e }), format!("{what}: malformed")),
            };
            if let Err(e) = keys(&m, &["name", "from"]) {
                return done(400, json!({ "error": e }), format!("{what}: malformed"));
            }
            let Some(name) = m.get("name").and_then(Json::as_str) else {
                return done(
                    400,
                    json!({ "error": "expected {name, from?}" }),
                    format!("{what}: no name"),
                );
            };
            let made = match m.get("from").and_then(Json::as_str) {
                Some(from) => p.copy(from, name),
                None => p.create(name),
            };
            match made {
                Ok(()) => done(
                    201,
                    json!({ "name": name }),
                    format!("project {}: created", clean(name)),
                ),
                Err(r) => failed(r, "not created"),
            }
        }
        ("DELETE", ["projects", name]) => match p.delete(name) {
            Ok(()) => done(200, json!({ "deleted": name }), format!("{what}: deleted")),
            Err(r) => failed(r, "not deleted"),
        },
        ("POST", ["projects", name, "rename"]) => {
            let to = json_body(req)
                .ok()
                .filter(|m| keys(m, &["to"]).is_ok())
                .and_then(|m| m.get("to").and_then(Json::as_str).map(str::to_string));
            let Some(to) = to else {
                return done(
                    400,
                    json!({ "error": "expected {to}" }),
                    format!("{what}: malformed rename"),
                );
            };
            match p.rename(name, &to) {
                Ok(()) => done(
                    200,
                    json!({ "name": to }),
                    format!("{what}: renamed to {}", clean(&to)),
                ),
                Err(r) => failed(r, "not renamed"),
            }
        }
        ("PUT", ["projects", name, "files", file]) => save(req, p, name, file, &done, &failed, &what),
        ("DELETE", ["projects", name, "files", file]) => match p.remove(name, file) {
            Ok(()) => done(
                200,
                json!({ "deleted": file }),
                format!("{what}: file {} deleted", clean(file)),
            ),
            Err(r) => failed(r, "file not deleted"),
        },
        ("POST", ["projects", name, "zip"]) => upload(req, p, name, &done, &what),
        ("POST", ["projects", name, "import"]) => import(req, p, name, &done, &failed, &what),
        ("POST", ["projects", name, "replay"]) => approve::replay(req, ctx, Target::Project(name)),
        ("POST", ["projects", name, "deploy"]) => approve::apply(req, ctx, Target::Project(name)),
        (_, path) => {
            let full: Vec<&str> = std::iter::once("api").chain(path.iter().copied()).collect();
            match allowed(&full) {
                Some(allow) => {
                    let r = Response::error(405, "method not allowed").with("Allow", allow.into());
                    match audit(ctx, req, 405, "method not allowed") {
                        Some(_) => r,
                        None => audit_full(),
                    }
                }
                None => done(
                    404,
                    json!({ "error": "no such resource" }),
                    "no such resource".into(),
                ),
            }
        }
    }
}

fn save(
    req: &Request,
    p: &Projects,
    name: &str,
    file: &str,
    done: &dyn Fn(u16, Json, String) -> Response,
    failed: &dyn Fn(Refusal, &str) -> Response,
    what: &str,
) -> Response {
    let m = match json_body(req) {
        Ok(m) => m,
        Err(e) => return done(400, json!({ "error": e }), format!("{what}: malformed save")),
    };
    if let Err(e) = keys(&m, &["text", "version"]) {
        return done(400, json!({ "error": e }), format!("{what}: malformed save"));
    }
    let Some(text) = m.get("text").and_then(Json::as_str) else {
        return done(
            400,
            json!({ "error": "expected {text, version?}" }),
            format!("{what}: malformed save"),
        );
    };
    let if_match = m.get("version").map(Json::as_str);
    match p.save(name, file, text.as_bytes(), if_match) {
        Ok(version) => done(
            200,
            json!({ "name": file, "version": version, "bytes": text.len() }),
            format!("{what}: file {} saved ({} bytes)", clean(file), text.len()),
        ),
        Err(r) => failed(r, &format!("file {} not saved", clean(file))),
    }
}

fn upload(
    req: &Request,
    p: &Projects,
    name: &str,
    done: &dyn Fn(u16, Json, String) -> Response,
    what: &str,
) -> Response {
    let zip_type = req.content_type.as_deref().is_some_and(|t| {
        matches!(
            t,
            "application/zip" | "application/x-zip-compressed" | "application/octet-stream"
        )
    });
    if !zip_type {
        return done(
            415,
            json!({ "error": "a project upload is application/zip" }),
            format!("{what}: upload not a zip"),
        );
    }
    let replace = match replace(req) {
        Ok(r) => r,
        Err(e) => return done(400, json!({ "error": e }), format!("{what}: malformed upload")),
    };
    let files = match project::zip::read(&req.body).and_then(project::from_zip) {
        Ok(f) => f,
        Err(e) => {
            return done(
                422,
                json!({ "error": format!("the archive is refused: {e}") }),
                format!("{what}: upload refused"),
            );
        }
    };
    match p.install(name, &files, replace) {
        Ok(()) => done(
            if replace { 200 } else { 201 },
            json!({ "name": name, "files": files.0.keys().collect::<Vec<_>>() }),
            format!("{what}: uploaded ({} files)", files.0.len()),
        ),
        Err(r) => done(
            r.status,
            json!({ "error": r.why }),
            format!("{what}: upload not installed: {}", r.why),
        ),
    }
}

/// The ST a single uploaded file becomes, and the file it goes in.
fn imported(file: &str, body: &[u8]) -> Result<(String, String), (u16, String)> {
    let lower = file.to_ascii_lowercase();
    let (stem, ext) = lower
        .rsplit_once('.')
        .map(|(s, e)| (&file[..s.len()], e))
        .ok_or((400, "file needs an extension: .st or .xml".to_string()))?;
    let target = format!("{stem}.st");
    if project::file_name(&target) != Some(FileKind::St) {
        return Err((
            400,
            "not a project file name (NAME.st, NAME 1 to 64 of A-Z a-z 0-9 _ -)".into(),
        ));
    }
    match ext {
        "st" => {
            let text = project::check_text(body).map_err(|e| (422, format!("{file}: {e}")))?;
            Ok((target, text.to_string()))
        }
        "xml" => {
            let text = std::str::from_utf8(body).map_err(|_| (422, format!("{file}: not UTF-8")))?;
            let lowered = helmsman_iec::plcopen::lower(text).map_err(|diags| {
                let lines: Vec<String> = diags.iter().take(20).map(ToString::to_string).collect();
                (422, format!("{file} is not a PLCopen project this box reads: {}", lines.join("; ")))
            })?;
            Ok((target, lowered.st().to_string()))
        }
        "l5x" => Err((
            415,
            "L5X import is not built yet: this box reads an L5X only as an inventory for the agent (POST /api/dropins)"
                .into(),
        )),
        _ => Err((415, "import takes .st or .xml (PLCopen TC6)".into())),
    }
}

fn import(
    req: &Request,
    p: &Projects,
    name: &str,
    done: &dyn Fn(u16, Json, String) -> Response,
    failed: &dyn Fn(Refusal, &str) -> Response,
    what: &str,
) -> Response {
    let Some(file) = query(req, "file") else {
        return done(
            400,
            json!({ "error": "import needs ?file=NAME.st or NAME.xml" }),
            format!("{what}: import without a file name"),
        );
    };
    let replace = match replace(req) {
        Ok(r) => r,
        Err(e) => return done(400, json!({ "error": e }), format!("{what}: malformed import")),
    };
    let (target, text) = match imported(file, &req.body) {
        Ok(x) => x,
        Err((status, e)) => {
            return done(status, json!({ "error": e }), format!("{what}: import refused"));
        }
    };
    let if_match = if replace { None } else { Some(None) };
    match p.save(name, &target, text.as_bytes(), if_match) {
        Ok(version) => done(
            201,
            json!({ "name": target, "version": version, "bytes": text.len() }),
            format!("{what}: imported {} as {target}", clean(file)),
        ),
        Err(r) => failed(r, "import not saved"),
    }
}

/// `POST /api/projects/{p}/check`: what a deploy would build, its
/// modules, its tier against what runs, or every problem with its file
/// and line. Nothing changes.
fn check(req: &Request, ctx: &Ctx, name: &str) -> Response {
    let what = format!("project {}", clean(name));
    match built(ctx, name) {
        Ok(b) => {
            let tier = ctx
                .running()
                .map(|r| approve::tier_json(&r, &b.change, ctx.approval.as_ref().map(|a| &a.policy)));
            let body = json!({
                "ok": true,
                "snapshot": b.change.id().to_string(),
                "modules": b.modules.iter().map(|m| json!({
                    "name": m.name, "file": m.file, "reads": m.reads, "drives": m.drives,
                })).collect::<Vec<_>>(),
                "approval": tier,
                "diagnostics": [],
            });
            audited(ctx, req, 200, &body, &format!("{what}: checked, builds"))
        }
        Err((status, body, why)) => audited(ctx, req, status, &body, &format!("{what}: checked: {why}")),
    }
}

fn shape(appr: &Approval) -> Shape {
    Shape {
        defaults: appr.plant.defaults,
        i_len: appr.plant.i_len,
        q_len: appr.plant.q_len,
    }
}

/// Project `name` built against what runs: its change set, or the status,
/// body and audit detail of why not (422 with every diagnostic when it
/// does not compile).
pub(super) fn built(ctx: &Ctx, name: &str) -> Result<Built, (u16, Json, String)> {
    let Some(p) = &ctx.projects else {
        return Err((
            404,
            json!({ "error": "no projects on this box (--projects)", "changed": false }),
            "no projects on this box".into(),
        ));
    };
    let Some(appr) = &ctx.approval else {
        return Err((
            501,
            json!({ "error": "this box has no approval path configured; nothing was changed", "changed": false }),
            "no approval path".into(),
        ));
    };
    let files: Files = p.read(name).map_err(|r| {
        (
            r.status,
            json!({ "error": r.why, "changed": false }),
            r.why.clone(),
        )
    })?;
    let running = ctx.running();
    build::build(name, &files, running.as_ref(), &shape(appr)).map_err(|diags| {
        let n = diags.len();
        (
            422,
            json!({
                "error": "the project does not build",
                "changed": false,
                "diagnostics": diags.iter().map(|d| json!({
                    "file": d.file, "line": d.line, "col": d.col, "message": d.message,
                })).collect::<Vec<_>>(),
            }),
            format!("does not build ({n} problem(s))"),
        )
    })
}

/// After a deploy committed `snapshot`: records `name` as the running
/// project. What the response says about it.
pub(super) fn deployed(ctx: &Ctx, name: &str, snapshot: SnapshotId) -> Json {
    let Some(p) = &ctx.projects else {
        return Json::Null;
    };
    let r = Running {
        name: name.to_string(),
        snapshot: snapshot.to_string(),
    };
    match p.record(&r) {
        Ok(()) => json!({ "name": name, "recorded": true }),
        Err(e) => {
            (ctx.cfg.log)(&format!(
                "projects: deployed {name}, but the running-project record was not written: {}",
                e.kind()
            ));
            json!({ "name": name, "recorded": false })
        }
    }
}
