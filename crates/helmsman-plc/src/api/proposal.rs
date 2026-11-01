//! Checking a proposal before it is stored. A proposal is a draft: a new
//! or replacement module (manifest + ST, or manifest + WASM binary as
//! base64), one param's new value, a new
//! I/O map or calendar file (applied as a reload; `map-reload`), or a new
//! or replacement WASM driver (name + binary as base64, which the map
//! names as `wasm:NAME`; `wasm-driver-map`). It is
//! checked here exactly as the runtime would check it at load (manifest
//! parse, compile against its windows or load under the default limits,
//! manifest against the program's ports, signals known), so the agent
//! iterates on the same errors a person would get. Nothing here applies
//! anything.

use helmsman_iec::{ImageSizes, Port, compile};
use helmsman_io::{Ty, Value};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::catalog::Catalog;
use super::photo::unbase64;
use super::render;
use crate::manifest::{self, Manifest};
use crate::store::{self, ChangeSet, Code, ModuleFiles};
use crate::wasm::{Limits, WasmProgram};

/// What a client posts to `/api/proposals`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct New {
    pub kind: String,
    pub manifest: Option<String>,
    pub source: Option<String>,
    /// A WASM module's `module.wasm`, in place of `source`: standard
    /// base64, padded, nothing else.
    pub wasm: Option<String>,
    pub module: Option<String>,
    pub param: Option<String>,
    pub value: Option<Json>,
    /// `io_map` and `calendar`: the whole new file.
    pub text: Option<String>,
    /// `driver`: the driver's name, as the map names it (`wasm:NAME`).
    pub name: Option<String>,
    pub note: Option<String>,
    /// A watch module: it reads and reports, and drives no output signal
    /// and no `%Q` (refused otherwise).
    #[serde(default)]
    pub watch: Option<bool>,
    /// The drop-ins the draft was made from (see `api::dropin`); recorded
    /// on the proposal for the engineer.
    #[serde(default)]
    pub dropins: Option<Vec<u64>>,
    /// Of `dropins`, the ones the relay refused (a PDF it would not
    /// read): cited, but the model never saw them. Recorded on the
    /// proposal as `refused_dropins` (`refused-dropin-binding`); each must
    /// be one of `dropins`.
    #[serde(default)]
    pub refused_dropins: Option<Vec<u64>>,
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

/// The refused drop-ins `new` cites, sorted and once each: every one
/// must be among its `dropins`.
fn refused_dropins(new: &New) -> Result<Vec<u64>, Refusal> {
    let cited = new.dropins.as_deref().unwrap_or_default();
    let mut refused = new.refused_dropins.clone().unwrap_or_default();
    refused.sort_unstable();
    refused.dedup();
    let stray: Vec<String> = refused
        .iter()
        .filter(|id| !cited.contains(id))
        .map(|id| format!("drop-in {id} is marked refused but not among dropins"))
        .collect();
    if stray.is_empty() {
        Ok(refused)
    } else {
        Err((
            400,
            json!({ "error": "refused_dropins must be a subset of dropins", "problems": stray }),
        ))
    }
}

/// What a proposal changes in the running change set.
enum Edit {
    Module(String, ModuleFiles),
    IoMap(String),
    Calendar(String),
    Driver(String, Vec<u8>),
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
    let watch = new.watch == Some(true);
    let (mut p, edit) = match new.kind.as_str() {
        "module" => as_edit(module(new, catalog, memory)?),
        "param" | "io_map" | "calendar" | "driver" if watch => {
            return Err((400, json!({ "error": "watch is for module proposals" })));
        }
        "param" => as_edit(param(new, catalog, running)?),
        "io_map" | "calendar" => file(new, catalog, running)?,
        "driver" => driver(new, running)?,
        _ => {
            return Err((
                400,
                json!({ "error": "kind must be \"module\", \"param\", \"io_map\", \"calendar\" or \"driver\"" }),
            ));
        }
    };
    p["note"] = json!(note);
    p["watch"] = json!(watch);
    p["dropins"] = json!(new.dropins.clone().unwrap_or_default());
    p["refused_dropins"] = json!(refused_dropins(new)?);
    let cs = if let (Some(base), Some(edit)) = (running, edit) {
        let mut cs = base.clone();
        match edit {
            Edit::Module(name, f) => {
                cs.modules.insert(name, f);
            }
            Edit::IoMap(text) => cs.io_map = Some(text),
            Edit::Calendar(text) => cs.calendar = Some(text),
            Edit::Driver(name, bytes) => {
                cs.drivers.insert(name, bytes);
                // The running map against the new driver set: a new
                // version must still parse every point its devices have.
                crate::layout::Layout::build_with(cs.io_map.as_deref(), cs.calendar.as_deref(), &cs.drivers)
                    .map_err(|errs| refuse("the running I/O map does not load with this driver", &errs))?;
            }
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
        ("wasm", new.wasm.is_some()),
        ("module", new.module.is_some()),
        ("param", new.param.is_some()),
        ("value", new.value.is_some()),
        ("name", new.name.is_some()),
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

/// The program a module proposal carries: ST source or a WASM binary.
fn code(new: &New) -> Result<Code, Refusal> {
    match (&new.source, &new.wasm) {
        (Some(s), None) => Ok(Code::St(s.clone())),
        (None, Some(b64)) => unbase64(b64).map(Code::Wasm).ok_or_else(|| {
            (
                400,
                json!({ "error": "wasm must be the module.wasm file as standard base64 (padded, nothing else)" }),
            )
        }),
        (Some(_), Some(_)) => Err((
            400,
            json!({ "error": "a module proposal carries source or wasm, not both" }),
        )),
        (None, None) => Err((
            400,
            json!({ "error": "a module proposal needs manifest and source (or wasm)" }),
        )),
    }
}

fn module(new: &New, catalog: &Catalog, memory: usize) -> Result<(Json, Files), Refusal> {
    only(new, &["manifest", "source", "wasm"])?;
    let Some(text) = &new.manifest else {
        return Err((
            400,
            json!({ "error": "a module proposal needs manifest and source (or wasm)" }),
        ));
    };
    let code = code(new)?;
    let cap = usize::try_from(manifest::MAX_FILE).unwrap_or(usize::MAX);
    if text.len() > cap || code.bytes().len() > cap {
        return Err(refuse(
            &format!("manifest or {} larger than a module file may be", code_key(&code)),
            &[format!("at most {cap} bytes each")],
        ));
    }
    let m = Manifest::parse(text).map_err(|errs| {
        let lines: Vec<String> = errs
            .iter()
            .map(|(n, e)| format!("{}:{n}: {e}", manifest::FILE))
            .collect();
        refuse("manifest does not parse", &lines)
    })?;
    m.check(&ports(&code, &m, memory)?).map_err(|errs| {
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
    if new.watch == Some(true) {
        let mut drives: Vec<String> = m
            .drives
            .iter()
            .filter(|n| {
                catalog
                    .signal(n)
                    .is_none_or(|i| catalog.signals[i].dir == helmsman_io::Dir::Out)
            })
            .map(|n| format!("drives output signal {n}"))
            .collect();
        if q.len > 0 {
            drives.push(format!("has a %Q window of {} bytes", q.len));
        }
        if !drives.is_empty() {
            return Err(refuse(
                "a watch module reads and reports; it drives no output",
                &drives,
            ));
        }
    }
    let j = json!({
        "kind": "module",
        "name": m.name,
        "replaces": catalog.module(&m.name).is_some(),
        "touches_outputs": !m.drives.is_empty() || q.len > 0,
        "manifest": render::manifest(&m),
        "source": code.st(),
        "wasm": code.wasm().map(render::wasm),
    });
    let files = ModuleFiles {
        manifest: text.clone(),
        code,
    };
    Ok((j, Some((m.name, files))))
}

/// The program's ports: ST compiled against the manifest's windows, WASM
/// loaded under the default limits (and it has no windows).
fn ports(code: &Code, m: &Manifest, memory: usize) -> Result<Vec<Port>, Refusal> {
    match code {
        Code::St(source) => {
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
            Ok(program.ports())
        }
        Code::Wasm(bytes) => {
            let program = WasmProgram::load(bytes, Limits::default()).map_err(|errs| {
                let lines: Vec<String> = errs.iter().map(|e| format!("{}: {e}", manifest::WASM)).collect();
                refuse("wasm does not load", &lines)
            })?;
            if m.i.is_some() || m.q.is_some() || m.m.is_some() {
                return Err(refuse(
                    "manifest does not match the program",
                    &["a WASM module has no i, q or m windows: it reads and drives signals only".into()],
                ));
            }
            Ok(program.ports())
        }
    }
}

const fn code_key(c: &Code) -> &'static str {
    match c {
        Code::St(_) => "source",
        Code::Wasm(_) => "wasm",
    }
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
                code: f.code.clone(),
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
        "unit": p.unit,
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
    // A map names WASM drivers only by approved ones: the running set.
    let layout = crate::layout::Layout::build_with(map, cal, &base.drivers)
        .map_err(|errs| refuse("does not load", &errs))?;
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

/// A new or replacement WASM driver: a driver name, and a binary that
/// exports `helmsman-driver/v1`, imports nothing else and starts under the
/// driver sandbox's limits ([`crate::drivers::check`]). The running map
/// must still load with it (in [`check`], once the change set is built).
fn driver(new: &New, running: Option<&ChangeSet>) -> Result<(Json, Option<Edit>), Refusal> {
    only(new, &["name", "wasm"])?;
    let (Some(name), Some(b64)) = (&new.name, &new.wasm) else {
        return Err((
            400,
            json!({ "error": "a driver proposal needs name and wasm (the binary as base64)" }),
        ));
    };
    if !crate::spec::name_ok(name) {
        return Err(refuse("not a driver name", &["1..=64 of A-Z a-z 0-9 _ -".into()]));
    }
    let bytes = unbase64(b64).ok_or_else(|| {
        (
            400,
            json!({ "error": "wasm must be the driver binary as standard base64 (padded, nothing else)" }),
        )
    })?;
    let cap = usize::try_from(manifest::MAX_FILE).unwrap_or(usize::MAX);
    if bytes.len() > cap {
        return Err(refuse(
            "driver larger than a change set may hold",
            &[format!("at most {cap} bytes")],
        ));
    }
    crate::drivers::check(&bytes).map_err(|errs| {
        let lines: Vec<String> = errs
            .iter()
            .map(|e| format!("{}: {e}", helmsman_io::wasm::ABI))
            .collect();
        refuse("not a driver this box can run", &lines)
    })?;
    let Some(base) = running else {
        return Err(refuse(
            "this box does not run a change set (module directories or a store), so it cannot take a driver",
            &[],
        ));
    };
    let named = format!("{}{name}", helmsman_io::driver::WASM);
    let devices: Vec<String> = base
        .io_map
        .as_deref()
        .and_then(|m| helmsman_io::text::parse(m).ok())
        .map(|f| {
            f.map
                .devices
                .into_iter()
                .filter(|d| d.driver == named)
                .map(|d| d.id)
                .collect()
        })
        .unwrap_or_default();
    let j = json!({
        "kind": "driver",
        "name": name,
        "map_name": named,
        "replaces": base.drivers.contains_key(name),
        "touches_outputs": true,
        "devices": devices,
        "wasm": render::driver(&bytes),
    });
    Ok((j, Some(Edit::Driver(name.clone(), bytes))))
}
