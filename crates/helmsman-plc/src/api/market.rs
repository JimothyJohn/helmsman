//! The marketplace skeleton on the box: list a signed index, install one
//! of its bundles through the bundle import path and the approval policy.
//!
//! - `POST /api/market/list` (token, JSON `{"index": URL}`): fetches the
//!   index and its signature ([`crate::bundle::market::list`]), verifies the
//!   signature against the box's trust store, and answers the entries.
//! - `POST /api/market/install` (token, JSON `{"index": URL, "name": N,
//!   "version"?: V}`): the same, then fetches the bundle the entry names,
//!   checks its size and hash against the index, and hands the bytes to the
//!   bundle import ([`super::bundle::import_bytes`]): verified, bound by
//!   name, stored as a draft for the policy to classify. Nothing runs
//!   without the approval the policy asks for.
//!
//! Both make the box fetch a URL a client chose, so they take the token
//! (never a read) and refuse hosts that are not public unless the box was
//! started with `--market-allow-local` ([`crate::bundle::market`]). Every
//! call is audited with the URL.

use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::{Ctx, Request, Response, audit, audit_full};
use crate::bundle::market;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    index: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    index: String,
    name: String,
    version: Option<String>,
}

/// Longest URL or name a request may carry.
const MAX_FIELD: usize = 2048;

fn body<T: for<'de> Deserialize<'de>>(req: &Request) -> Result<T, String> {
    if !req
        .content_type
        .as_deref()
        .is_some_and(|t| t.starts_with("application/json"))
    {
        return Err("Content-Type must be application/json".into());
    }
    serde_json::from_slice(&req.body).map_err(|e| format!("body: {e}"))
}

fn entry_json(e: &crate::bundle::index::Entry) -> Json {
    json!({
        "name": e.name, "version": e.version, "author": e.author, "description": e.description,
        "id": e.id, "sha256": e.sha256, "size": e.size, "file": e.file, "signed_by": e.signed_by,
    })
}

fn listing(ctx: &Ctx, url: &str) -> Result<market::Listing, (u16, String)> {
    if url.len() > MAX_FIELD {
        return Err((400, "index URL too long".into()));
    }
    let Some(trust) = &ctx.trust else {
        return Err((
            501,
            "the marketplace is not set up on this box (no trust store)".into(),
        ));
    };
    market::list(url, trust, ctx.market_local).map_err(|e| (422, e))
}

/// `POST /api/market/list`.
pub(super) fn list(req: &Request, ctx: &Ctx) -> Response {
    let refused = |status: u16, why: &str, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::json(status, &json!({ "error": why })),
        None => audit_full(),
    };
    let b: List = match body(req) {
        Ok(b) => b,
        Err(e) => return refused(400, &e, "market list: bad request"),
    };
    let l = match listing(ctx, &b.index) {
        Ok(l) => l,
        Err((status, e)) => return refused(status, &e, &format!("market list {}: refused", b.index)),
    };
    let out = json!({
        "index": l.url.to_string(),
        "publisher": l.publisher,
        "bundles": l.index.bundles.iter().map(entry_json).collect::<Vec<_>>(),
    });
    match audit(
        ctx,
        req,
        200,
        &format!(
            "market list {}: {} bundle(s) by {}",
            l.url,
            l.index.bundles.len(),
            l.publisher
        ),
    ) {
        Some(_) => Response::json(200, &out),
        None => audit_full(),
    }
}

/// `POST /api/market/install`.
pub(super) fn install(req: &Request, ctx: &Ctx) -> Response {
    let refused = |status: u16, why: &str, detail: &str| match audit(ctx, req, status, detail) {
        Some(_) => Response::json(status, &json!({ "error": why })),
        None => audit_full(),
    };
    let b: Install = match body(req) {
        Ok(b) => b,
        Err(e) => return refused(400, &e, "market install: bad request"),
    };
    if b.name.len() > MAX_FIELD || b.version.as_ref().is_some_and(|v| v.len() > MAX_FIELD) {
        return refused(400, "name or version too long", "market install: bad request");
    }
    let l = match listing(ctx, &b.index) {
        Ok(l) => l,
        Err((status, e)) => {
            return refused(status, &e, &format!("market install {}: refused", b.index));
        }
    };
    let Some(entry) = l.index.find(&b.name, b.version.as_deref()) else {
        let have: Vec<String> = l
            .index
            .bundles
            .iter()
            .map(|e| format!("{} {}", e.name, e.version))
            .collect();
        return refused(
            404,
            &format!(
                "{} is not in {}'s index; it has: {}",
                b.name,
                l.publisher,
                have.join(", ")
            ),
            &format!("market install {}: {} not in the index", l.url, b.name),
        );
    };
    let bytes = match market::fetch(&l, entry, ctx.market_local) {
        Ok(b) => b,
        Err(e) => {
            return refused(
                422,
                &e,
                &format!("market install {}: {} not fetched", l.url, entry.file),
            );
        }
    };
    let via = format!(
        "market {} ({} {} from {}, index signed by {})",
        l.url, entry.name, entry.version, entry.file, l.publisher
    );
    super::bundle::import_bytes(req, ctx, &bytes, &via)
}
