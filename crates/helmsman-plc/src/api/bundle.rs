//! Bundles over the box API.
//!
//! - `GET /api/snapshots/{id}/bundle`: that snapshot as an **unsigned**
//!   bundle (`application/octet-stream`), its capability manifest computed
//!   from the content. Its id is the snapshot's. Signing happens off the
//!   box, with the publisher's key (`helmsman-plc bundle sign`); no key is
//!   ever on a box.
//! - `POST /api/bundles` (token; the bundle file as the body,
//!   `application/octet-stream`): [`crate::bundle::verify`] against the
//!   box's trust store, then the bundle's modules **bound to this box's
//!   signals by name** (the bundle's I/O map is a template: it is never
//!   installed; its signal types and directions are checked against the
//!   box's), then, if everything binds, stored as a **proposal** that adds
//!   or replaces those modules in what runs. It is never applied here: the
//!   approval policy takes it like any other proposal (classified from the
//!   diff alone, so one that drives anything is output-affecting). A
//!   bundle that does not bind is refused with the whole binding report.
//!
//! Every import, refused or not, is in the audit log.

use std::collections::BTreeSet;

use serde_json::{Value as Json, json};

use super::catalog::Catalog;
use super::{Ctx, Request, Response, approve, audit, audit_full, audit_with, render, snapshot, store, wall};
use crate::Window;
use crate::bundle::{self, Bundle, BundleManifest, Trust, Verified, capabilities};
use crate::spec::Defaults;
use crate::store::{ChangeSet, diff};
use helmsman_io::Dir;

/// What modules are checked with when the box has no approval path.
fn defaults(ctx: &Ctx) -> Defaults {
    ctx.approval.as_ref().map_or(
        Defaults {
            period: std::time::Duration::from_millis(10),
            i: Window::new(0, 0),
            q: Window::new(0, 0),
            m: ctx.cfg.memory,
        },
        |a| a.plant.defaults,
    )
}

/// `GET /api/snapshots/{id}/bundle`.
pub(super) fn export(ctx: &Ctx, id: &str) -> Response {
    let cs = match snapshot(ctx, id) {
        Ok(cs) => cs,
        Err(r) => return r,
    };
    let caps = match capabilities(&cs, &defaults(ctx)) {
        Ok(c) => c,
        Err(problems) => {
            return Response::json(
                422,
                &json!({ "error": "this snapshot cannot be bundled", "problems": problems }),
            );
        }
    };
    let id = cs.id().to_string();
    let manifest = BundleManifest {
        name: format!("snapshot-{}", &id[..12]),
        version: "0.0.0".into(),
        author: "helmsman-plc export".into(),
        description: Some(format!("snapshot {id}, exported unsigned")),
        caps,
    };
    let b = Bundle::new(manifest, cs);
    Response {
        status: 200,
        content_type: "application/octet-stream",
        headers: vec![
            (
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", bundle::cli::file_name(&id)),
            ),
            ("X-Bundle-Id", id),
        ],
        body: b.encode(),
    }
}

/// How one signal the bundle names meets this box.
fn bind(v: &Verified, catalog: &Catalog) -> (Vec<Json>, Vec<Json>) {
    let template = v
        .bundle
        .content
        .io_map
        .as_deref()
        .and_then(|m| helmsman_io::text::parse(m).ok());
    let replaced: BTreeSet<String> = v.caps.modules.iter().map(|m| m.to_ascii_lowercase()).collect();
    let mut bound = Vec::new();
    let mut unbound = Vec::new();
    let names: BTreeSet<&String> = v.caps.reads.iter().chain(&v.caps.drives).collect();
    for name in names {
        let drives = v.caps.drives.contains(name);
        let use_ = if drives { "drives" } else { "reads" };
        let Some(i) = catalog.signal(name) else {
            unbound
                .push(json!({ "signal": name, "use": use_, "why": format!("no signal {name} on this box") }));
            continue;
        };
        let s = &catalog.signals[i];
        let mut why = Vec::new();
        if let Some(t) = template
            .as_ref()
            .and_then(|f| f.map.signals.iter().find(|t| t.name.eq_ignore_ascii_case(name)))
        {
            if t.ty != s.ty {
                why.push(format!(
                    "the template says {}, this box's {} is {}",
                    render::ty(t.ty),
                    s.name,
                    render::ty(s.ty)
                ));
            }
            if t.dir != s.dir {
                why.push(format!(
                    "the template says {}, this box's {} is {}",
                    dir(t.dir),
                    s.name,
                    dir(s.dir)
                ));
            }
        }
        if drives && s.dir != Dir::Out {
            why.push(format!(
                "the bundle drives it, but this box's {} is an input",
                s.name
            ));
        }
        if drives
            && let Some(w) = &s.writer
            && !replaced.contains(&w.to_ascii_lowercase())
        {
            why.push(format!("already driven by {w} on this box"));
        }
        if why.is_empty() {
            bound.push(json!({
                "signal": name, "use": use_, "to": s.name, "type": render::ty(s.ty), "dir": dir(s.dir),
            }));
        } else {
            unbound.push(json!({ "signal": name, "use": use_, "why": why.join("; ") }));
        }
    }
    (bound, unbound)
}

const fn dir(d: Dir) -> &'static str {
    match d {
        Dir::In => "an input",
        Dir::Out => "an output",
    }
}

fn caps_json(v: &Verified) -> Json {
    let c = &v.caps;
    json!({
        "modules": c.modules, "reads": c.reads, "drives": c.drives, "q_bytes": c.q_bytes,
        "devices": c.devices, "drivers": c.drivers,
        "drives_anything": c.drives_anything(),
        "summary": format!(
            "reads {} signal(s), drives {} output signal(s) and {} %Q byte(s)",
            c.reads.len(), c.drives.len(), c.q_bytes
        ),
    })
}

fn bundle_json(v: &Verified) -> Json {
    let m = &v.bundle.manifest;
    let (trust, publisher) = match &v.trust {
        Trust::Publisher { name, key } => ("publisher", json!({ "name": name, "key": key })),
        Trust::Dev { .. } => ("dev", Json::Null),
    };
    json!({
        "id": v.bundle.id().to_string(),
        "name": m.name, "version": m.version, "author": m.author, "description": m.description,
        "capabilities": caps_json(v),
        "trust": trust,
        "dev": matches!(v.trust, Trust::Dev { .. }),
        "publisher": publisher,
        "trust_line": v.trust.describe(),
        "signers": v.bundle.signatures.iter().map(|s| bundle::sign::hex(&s.key)).collect::<Vec<_>>(),
    })
}

/// `POST /api/bundles`.
#[allow(clippy::too_many_lines)]
pub(super) fn import(req: &Request, ctx: &Ctx) -> Response {
    let refused = |status: u16, body: &Json, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::json(status, body),
        None => audit_full(),
    };
    let octets = req
        .content_type
        .as_deref()
        .is_some_and(|t| t == "application/octet-stream");
    if !octets {
        return refused(
            415,
            &json!({ "error": "Content-Type must be application/octet-stream (the bundle file)" }),
            "bundle: not application/octet-stream",
        );
    }
    let Some(trust) = &ctx.trust else {
        return refused(
            501,
            &json!({ "error": "bundle import is not set up on this box (no trust store)" }),
            "bundle: no trust store",
        );
    };
    let v = match bundle::verify(&req.body, trust, &defaults(ctx)) {
        Ok(v) => v,
        Err(e) => {
            let status = match e {
                bundle::Error::Unsigned | bundle::Error::Untrusted(_) => 403,
                _ => 422,
            };
            return refused(
                status,
                &json!({ "error": e.to_string(), "refused": e.kind(), "problems": e.problems() }),
                &format!("bundle refused: {}", e.kind()),
            );
        }
    };
    let what = format!(
        "bundle {} {} ({})",
        v.bundle.manifest.name,
        v.bundle.manifest.version,
        v.trust.describe()
    );
    let Some(running) = ctx.running() else {
        return refused(
            409,
            &json!({ "error": "the box does not run a change set, so a bundle has nothing to join" }),
            &format!("{what}: no running change set"),
        );
    };
    let (bound, unbound) = bind(&v, &ctx.catalog());
    let template = match (&v.bundle.content.io_map, &v.bundle.content.calendar) {
        (None, None) => Json::Null,
        (m, c) => json!({
            "io_map": m.as_ref().map(|_| "a template only: signals bind by name; this box's map is unchanged"),
            "calendar": c.as_ref().map(|_| "not imported: the calendar is this box's own"),
        }),
    };
    let bindings = json!({ "bound": bound, "unbound": unbound, "template": template });
    if !unbound.is_empty() {
        return refused(
            422,
            &json!({
                "error": "the bundle's signals do not all bind to this box",
                "bundle": bundle_json(&v),
                "bindings": bindings,
            }),
            &format!("{what}: {} signal(s) do not bind", unbound.len()),
        );
    }
    let mut target: ChangeSet = running.clone();
    let mut replaces = Vec::new();
    for (name, f) in &v.bundle.content.modules {
        if target.modules.insert(name.clone(), f.clone()).is_some() {
            replaces.push(name.clone());
        }
    }
    if let Err(errs) = target.validate() {
        return refused(
            422,
            &json!({ "error": "the change set would not be stored", "problems": errs }),
            &format!("{what}: change set refused"),
        );
    }
    if target == running {
        return refused(
            409,
            &json!({ "error": "this box already runs exactly this bundle's modules" }),
            &format!("{what}: nothing to change"),
        );
    }
    let Ok(id) = ctx.box_.proposals.reserve() else {
        return refused(507, &json!({ "error": "too many proposals" }), "proposals full");
    };
    let (base, snap) = (running.id(), target.id());
    let m = &v.bundle.manifest;
    let note = format!(
        "bundle {} {} by {}, {} (bundle id {})",
        m.name,
        m.version,
        m.author,
        v.trust.describe(),
        v.bundle.id()
    );
    let p = json!({
        "id": id,
        "kind": "bundle",
        "status": "draft",
        "at": wall(),
        "bundle": bundle_json(&v),
        "bindings": bindings,
        "modules": v.caps.modules,
        "replaces": replaces,
        "touches_outputs": v.caps.drives_anything(),
        // What the store's history records as author and prompt on apply.
        "manifest": { "author": format!("{} (bundle {} {})", m.author, m.name, m.version), "prompt": [] },
        "note": note,
        "base": base.to_string(),
        "snapshot": snap.to_string(),
        "diff": render::diff(&diff(&running, &target)),
        "approval": approve::tier_json(&running, &target, ctx.approval.as_ref().map(|a| &a.policy)),
    });
    let detail = format!("proposal {id}: {what}: snapshot {base} -> {snap}");
    if audit_with(ctx, req, 201, &detail, Some(snap.to_string())).is_none() {
        return audit_full();
    }
    ctx.box_.proposals.insert(store::Stored {
        id,
        json: p.clone(),
        change: Some(target),
    });
    Response::json(201, &p).with("Location", format!("/api/proposals/{id}"))
}
