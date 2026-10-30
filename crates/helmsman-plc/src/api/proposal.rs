//! Checking a proposal before it is stored. A proposal is a draft: a new
//! or replacement module (manifest + ST), one param's new value, or a new
//! I/O map or calendar file (applied as a reload; `map-reload`). It is
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
use crate::store::{self, ChangeSet, ModuleFiles};

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
    /// `io_map` and `calendar`: the whole new file.
    pub text: Option<String>,
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

/// What a proposal changes in the running change set.
enum Edit {
    Module(String, ModuleFiles),
    IoMap(String),
    Calendar(String),
}

/// A module's files as a proposal would leave them.
type Files = Option<(String, ModuleFiles)>;

/// Checks `new` against the box as `catalog` describes it; `memory` is
/// the default `%M` size. `running` is the change set the box runs (the
/// store's head, or its files), when it has one: the proposal then
/// carries the change set applying it would commit, its snapshot id and
/// its diff from `running` (the audit log names both ids). The proposal
/// as JSON, without id, time or status, and its change set; or why not.
///
/// # Errors
/// A [`Refusal`]: 422 for a draft that does not check, 400 for a
/// malformed request.
pub fn check(
    new: &New,
    catalog: &Catalog,
    memory: usize,
    running: Option<&ChangeSet>,
) -> Result<(Json, Option<ChangeSet>), Refusal> {
    let note = note(new.note.as_deref())?;
    let as_edit = |(p, f): (Json, Files)| (p, f.map(|(n, f)| Edit::Module(n, f)));
    let (mut p, edit) = match new.kind.as_str() {
        "module" => as_edit(module(new, catalog, memory)?),
        "param" => as_edit(param(new, catalog, running)?),
        "io_map" | "calendar" => file(new, catalog, running)?,
        _ => {
            return Err((
                400,
                json!({ "error": "kind must be \"module\", \"param\", \"io_map\" or \"calendar\"" }),
            ));
        }
    };
    p["note"] = json!(note);
    let cs = if let (Some(base), Some(edit)) = (running, edit) {
        let mut cs = base.clone();
        match edit {
            Edit::Module(name, f) => {
                cs.modules.insert(name, f);
            }
            Edit::IoMap(text) => cs.io_map = Some(text),
            Edit::Calendar(text) => cs.calendar = Some(text),
        }
        cs.validate()
            .map_err(|errs| refuse("the change set would not be stored", &errs))?;
        p["base"] = json!(base.id().to_string());
        p["snapshot"] = json!(cs.id().to_string());
        p["diff"] = render::diff(&store::diff(base, &cs));
        Some(cs)
    } else {
        p["base"] = Json::Null;
        p["snapshot"] = Json::Null;
        p["diff"] = Json::Null;
        None
    };
    Ok((p, cs))
}

fn only(new: &New, allowed: &[&str]) -> Result<(), Refusal> {
    let given = [
        ("text", new.text.is_some()),
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

fn module(new: &New, catalog: &Catalog, memory: usize) -> Result<(Json, Files), Refusal> {
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
    let files = ModuleFiles {
        manifest: text.clone(),
        source: source.clone(),
    };
    let j = json!({
        "kind": "module",
        "name": m.name,
        "replaces": catalog.module(&m.name).is_some(),
        "touches_outputs": !m.drives.is_empty() || q.len > 0,
        "manifest": render::manifest(&m),
        "source": source,
    });
    Ok((j, Some((m.name, files))))
}

fn param(new: &New, catalog: &Catalog, running: Option<&ChangeSet>) -> Result<(Json, Files), Refusal> {
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
    let name = &catalog.modules[at].name;
    // The module's files with the one value changed. The manifest is
    // re-rendered, so comments and spacing in it are normalized.
    let files = running.and_then(|r| r.modules.get(name)).and_then(|f| {
        let mut m = Manifest::parse(&f.manifest).ok()?;
        m.params.iter_mut().find(|q| q.name == p.name)?.value = to;
        Some((
            name.clone(),
            ModuleFiles {
                manifest: m.render(),
                source: f.source.clone(),
            },
        ))
    });
    let j = json!({
        "kind": "param",
        "module": catalog.modules[at].name,
        "param": p.name,
        "type": render::ty(p.ty()),
        "from": render::value(Some(p.value)),
        "to": render::value(Some(to)),
        "min": render::value(p.range.map(|r| r.0)),
        "max": render::value(p.range.map(|r| r.1)),
        "touches_outputs": !m.drives.is_empty() || m.q.is_some_and(|w| w.len > 0),
    });
    Ok((j, files))
}

/// A new I/O map or calendar file: it must load beside the running other
/// one ([`crate::layout::Layout::build`], as the box loads it), and every
/// signal a running module's manifest reads or drives must still be there.
/// The full check (every module bound to the new table) is the apply's.
fn file(new: &New, catalog: &Catalog, running: Option<&ChangeSet>) -> Result<(Json, Option<Edit>), Refusal> {
    only(new, &["text"])?;
    let Some(text) = &new.text else {
        return Err((
            400,
            json!({ "error": "an io_map or calendar proposal needs text" }),
        ));
    };
    let cap = usize::try_from(manifest::MAX_FILE).unwrap_or(usize::MAX);
    if text.len() > cap {
        return Err(refuse("file larger than a change set may hold", &[]));
    }
    let Some(base) = running else {
        return Err(refuse(
            "this box does not run a change set (module directories or a store), so its map and calendar cannot be proposed",
            &[],
        ));
    };
    let map = base.io_map.as_deref();
    let cal = base.calendar.as_deref();
    let (map, cal, edit) = if new.kind == "io_map" {
        (Some(text.as_str()), cal, Edit::IoMap(text.clone()))
    } else {
        (map, Some(text.as_str()), Edit::Calendar(text.clone()))
    };
    let layout = crate::layout::Layout::build(map, cal).map_err(|errs| refuse("does not load", &errs))?;
    let missing: Vec<String> = catalog
        .modules
        .iter()
        .filter_map(|m| m.manifest.as_ref().map(|man| (&m.name, man)))
        .flat_map(|(name, man)| man.reads.iter().chain(&man.drives).map(move |s| (name, s)))
        .filter(|(_, s)| layout.table.index(s).is_none())
        .map(|(name, s)| format!("module {name} names {s}, which this would remove"))
        .collect();
    if !missing.is_empty() {
        return Err(refuse("running modules name signals this would remove", &missing));
    }
    let j = json!({
        "kind": new.kind,
        "touches_outputs": true,
        "signals": layout.table.len(),
        "devices": layout.map.as_ref().map_or(0, |m| m.devices.len()),
        "text": text,
    });
    Ok((j, Some(edit)))
}
