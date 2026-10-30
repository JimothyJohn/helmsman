//! Checking a proposal before it is stored. A proposal is a draft: a new
//! or replacement module (manifest + ST), or one param's new value. It is
//! checked here exactly as the runtime would check it at load (manifest
//! parse, compile against its windows, manifest against the program's
//! ports, signals known), so the agent iterates on the same errors a
//! person would get. Nothing here applies anything.

use helmsman_iec::{ImageSizes, compile};
use helmsman_io::{Ty, Value};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::catalog::Catalog;
use super::render;
use crate::manifest::{self, Manifest};

/// What a client posts to `/api/proposals`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct New {
    pub kind: String,
    pub manifest: Option<String>,
    pub source: Option<String>,
    pub module: Option<String>,
    pub param: Option<String>,
    pub value: Option<Json>,
    pub note: Option<String>,
}

/// Longest `note`, in characters.
pub const MAX_NOTE: usize = 2000;

/// Why a proposal was refused: status and body.
pub type Refusal = (u16, Json);

fn refuse(why: &str, problems: &[String]) -> Refusal {
    (422, json!({ "error": why, "problems": problems }))
}

fn note(n: Option<&str>) -> Result<Option<String>, Refusal> {
    match n {
        None => Ok(None),
        Some(n) if n.chars().count() > MAX_NOTE => Err(refuse(
            "note too long",
            &[format!("at most {MAX_NOTE} characters")],
        )),
        Some(n) if n.chars().any(manifest::hidden) => Err(refuse(
            "note carries control or invisible formatting characters",
            &[],
        )),
        Some(n) => Ok(Some(n.to_string())),
    }
}

/// Checks `new` against the box as `catalog` describes it; `memory` is
/// the default `%M` size. The proposal as JSON, without id, time or
/// status, or why not.
///
/// # Errors
/// A [`Refusal`]: 422 for a draft that does not check, 400 for a
/// malformed request.
pub fn check(new: &New, catalog: &Catalog, memory: usize) -> Result<Json, Refusal> {
    let note = note(new.note.as_deref())?;
    let mut p = match new.kind.as_str() {
        "module" => module(new, catalog, memory)?,
        "param" => param(new, catalog)?,
        _ => return Err((400, json!({ "error": "kind must be \"module\" or \"param\"" }))),
    };
    p["note"] = json!(note);
    Ok(p)
}

fn only(new: &New, allowed: &[&str]) -> Result<(), Refusal> {
    let given = [
        ("manifest", new.manifest.is_some()),
        ("source", new.source.is_some()),
        ("module", new.module.is_some()),
        ("param", new.param.is_some()),
        ("value", new.value.is_some()),
    ];
    let extra: Vec<String> = given
        .iter()
        .filter(|(k, g)| *g && !allowed.contains(k))
        .map(|(k, _)| (*k).to_string())
        .collect();
    if extra.is_empty() {
        Ok(())
    } else {
        Err((
            400,
            json!({ "error": "fields that do not belong to this kind", "fields": extra }),
        ))
    }
}

fn module(new: &New, catalog: &Catalog, memory: usize) -> Result<Json, Refusal> {
    only(new, &["manifest", "source"])?;
    let (Some(text), Some(source)) = (&new.manifest, &new.source) else {
        return Err((
            400,
            json!({ "error": "a module proposal needs manifest and source" }),
        ));
    };
    let cap = usize::try_from(manifest::MAX_FILE).unwrap_or(usize::MAX);
    if text.len() > cap || source.len() > cap {
        return Err(refuse("manifest or source larger than a module file may be", &[]));
    }
    let m = Manifest::parse(text).map_err(|errs| {
        let lines: Vec<String> = errs
            .iter()
            .map(|(n, e)| format!("{}:{n}: {e}", manifest::FILE))
            .collect();
        refuse("manifest does not parse", &lines)
    })?;
    let sizes = ImageSizes {
        i: m.i.map_or(0, |w| w.len),
        q: m.q.map_or(0, |w| w.len),
        m: m.m.unwrap_or(memory),
    };
    let program = compile(source, sizes).map_err(|diags| {
        let lines: Vec<String> = diags
            .iter()
            .map(|d| format!("{}:{d}", manifest::SOURCE))
            .collect();
        refuse("source does not compile", &lines)
    })?;
    m.check(&program.ports()).map_err(|errs| {
        let lines: Vec<String> = errs.iter().map(ToString::to_string).collect();
        refuse("manifest does not match the program", &lines)
    })?;
    let unknown: Vec<String> = m
        .reads
        .iter()
        .chain(&m.drives)
        .filter(|n| catalog.signal(n).is_none())
        .map(|n| format!("no signal {n} on this box"))
        .collect();
    if !unknown.is_empty() {
        return Err(refuse("names signals the box does not have", &unknown));
    }
    let q = m.q.unwrap_or(crate::Window::new(0, 0));
    Ok(json!({
        "kind": "module",
        "name": m.name,
        "replaces": catalog.module(&m.name).is_some(),
        "touches_outputs": !m.drives.is_empty() || q.len > 0,
        "manifest": render::manifest(&m),
        "source": source,
    }))
}

fn param(new: &New, catalog: &Catalog) -> Result<Json, Refusal> {
    only(new, &["module", "param", "value"])?;
    let (Some(module), Some(name), Some(given)) = (&new.module, &new.param, &new.value) else {
        return Err((
            400,
            json!({ "error": "a param proposal needs module, param and value" }),
        ));
    };
    let Some(at) = catalog.module(module) else {
        return Err(refuse("no such module", &[]));
    };
    let Some(m) = &catalog.modules[at].manifest else {
        return Err(refuse("module has no manifest, so no params", &[]));
    };
    let Some(p) = m.params.iter().find(|q| q.name.eq_ignore_ascii_case(name)) else {
        return Err(refuse("module has no such param", &[]));
    };
    let to = match (p.ty(), given) {
        (Ty::Bool, Json::Bool(b)) => Some(Value::Bool(*b)),
        (Ty::Int, Json::Number(n)) => n.as_i64().map(Value::Int),
        (Ty::Real, Json::Number(n)) => n.as_f64().filter(|x| x.is_finite()).map(Value::Real),
        _ => None,
    };
    let Some(to) = to else {
        return Err(refuse(&format!("value must be a {}", render::ty(p.ty())), &[]));
    };
    if !p.admits(to) {
        return Err(refuse("value outside the param's range", &[]));
    }
    Ok(json!({
        "kind": "param",
        "module": catalog.modules[at].name,
        "param": p.name,
        "type": render::ty(p.ty()),
        "from": render::value(Some(p.value)),
        "to": render::value(Some(to)),
        "min": render::value(p.range.map(|r| r.0)),
        "max": render::value(p.range.map(|r| r.1)),
        "touches_outputs": !m.drives.is_empty() || m.q.is_some_and(|w| w.len > 0),
    }))
}
