//! The API's JSON shapes. Hand-built with `json!` rather than derived, so
//! the runtime's types stay free of serde and the wire shape is written
//! down in one place.

use helmsman_iec::trace::{Shape, StmtId};
use helmsman_iec::{Fault, FaultKind, Value as StValue};
use helmsman_io::{Dir, Ty, Value};
use serde_json::{Value as Json, json};

use super::catalog::{Catalog, ModuleInfo, SignalInfo};
use super::live::{ModuleLive, View};
use crate::manifest::Manifest;
use crate::store::{ChangeSet, Diff, Entry, Kind, ModuleChange};
use crate::{Sample, Status, Window};

#[must_use]
pub const fn ty(t: Ty) -> &'static str {
    match t {
        Ty::Bool => "bool",
        Ty::Int => "int",
        Ty::Real => "real",
    }
}

#[must_use]
pub const fn dir(d: Dir) -> &'static str {
    match d {
        Dir::In => "in",
        Dir::Out => "out",
    }
}

/// A signal value; a non-finite real (never stored, but) is `null`.
#[must_use]
pub fn value(v: Option<Value>) -> Json {
    match v {
        Some(Value::Bool(b)) => json!(b),
        Some(Value::Int(n)) => json!(n),
        Some(Value::Real(x)) if x.is_finite() => json!(x),
        _ => Json::Null,
    }
}

/// An ST value: integers past 64 bits as decimal strings.
#[must_use]
pub fn st_value(v: StValue) -> Json {
    match v {
        StValue::Bool(b) => json!(b),
        StValue::Int(n) => i64::try_from(n)
            .map(|n| json!(n))
            .or_else(|_| u64::try_from(n).map(|n| json!(n)))
            .unwrap_or_else(|_| json!(n.to_string())),
        StValue::Real(x) if x.is_finite() => json!(x),
        StValue::Real(_) => Json::Null,
    }
}

const fn fault_kind(k: FaultKind) -> &'static str {
    match k {
        FaultKind::DivisionByZero => "division_by_zero",
        FaultKind::IndexOutOfRange => "index_out_of_range",
        FaultKind::Watchdog => "watchdog",
        FaultKind::OutputRange => "output_range",
    }
}

#[must_use]
pub fn fault(f: Fault) -> Json {
    json!({ "kind": fault_kind(f.kind), "line": f.line, "text": f.to_string() })
}

#[must_use]
pub fn window(w: Window) -> Json {
    json!({ "start": w.start, "len": w.len })
}

fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A signal with its live value. Inputs are `good` with a value or
/// `missing` without (device down, not read yet, source module held);
/// outputs carry their command and are `commanded`.
#[must_use]
pub fn signal(s: &SignalInfo, v: Option<Value>) -> Json {
    let quality = match (s.dir, v) {
        (Dir::Out, _) => "commanded",
        (Dir::In, Some(_)) => "good",
        (Dir::In, None) => "missing",
    };
    json!({
        "name": s.name,
        "type": ty(s.ty),
        "dir": dir(s.dir),
        "unit": s.unit,
        "binding": s.binding,
        "writer": s.writer,
        "value": value(v),
        "quality": quality,
    })
}

#[must_use]
pub fn signals(c: &Catalog, view: &View) -> Json {
    Json::Array(
        c.signals
            .iter()
            .enumerate()
            .map(|(i, s)| signal(s, view.signals.get(i).copied().flatten()))
            .collect(),
    )
}

/// The outputs: each output signal's command and safe value, and the
/// output image.
#[must_use]
pub fn outputs(c: &Catalog, view: &View) -> Json {
    let sigs: Vec<Json> = c
        .signals
        .iter()
        .enumerate()
        .filter(|(_, s)| s.dir == Dir::Out)
        .map(|(i, s)| {
            json!({
                "name": s.name,
                "type": ty(s.ty),
                "unit": s.unit,
                "binding": s.binding,
                "writer": s.writer,
                "command": value(view.signals.get(i).copied().flatten()),
                "safe": value(s.safe),
            })
        })
        .collect();
    json!({ "signals": sigs, "image": view.outputs, "seq": view.seq })
}

fn status(live: Option<&ModuleLive>) -> Json {
    match live.map(|l| l.status) {
        None => json!({ "state": "starting" }),
        Some(Status::Running) => json!({ "state": "running" }),
        Some(Status::Held) => json!({ "state": "held" }),
        Some(Status::Faulted(f)) => json!({ "state": "faulted", "fault": fault(f) }),
    }
}

/// A module's summary: schedule, status, stats.
#[must_use]
pub fn module(m: &ModuleInfo, live: Option<&ModuleLive>) -> Json {
    let stats = live.map(|l| l.stats).unwrap_or_default();
    json!({
        "name": m.name,
        "origin": m.origin,
        "scan_ms": millis(m.period),
        "i": window(m.i),
        "q": window(m.q),
        "status": status(live),
        "stats": { "scans": stats.scans, "overruns": stats.overruns, "held": stats.held },
        "has_manifest": m.manifest.is_some(),
    })
}

#[must_use]
pub fn modules(c: &Catalog, view: &View) -> Json {
    Json::Array(
        c.modules
            .iter()
            .enumerate()
            .map(|(k, m)| module(m, view.modules.get(k)))
            .collect(),
    )
}

/// A manifest, parsed, with its text.
#[must_use]
pub fn manifest(m: &Manifest) -> Json {
    let params: Vec<Json> = m
        .params
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "type": ty(p.ty()),
                "value": value(Some(p.value)),
                "min": value(p.range.map(|r| r.0)),
                "max": value(p.range.map(|r| r.1)),
            })
        })
        .collect();
    json!({
        "name": m.name,
        "author": m.author,
        "prompt": m.prompt,
        "scan_ms": millis(m.scan),
        "i": m.i.map(window),
        "q": m.q.map(window),
        "m": m.m,
        "reads": m.reads,
        "drives": m.drives,
        "params": params,
        "text": m.render(),
    })
}

/// A module in full: the summary, its ports, source and manifest.
#[must_use]
pub fn module_detail(m: &ModuleInfo, live: Option<&ModuleLive>) -> Json {
    let mut j = module(m, live);
    let ports: Vec<Json> = m
        .ports
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "dir": match p.dir { helmsman_iec::PortDir::Input => "in", helmsman_iec::PortDir::Output => "out" },
                "type": p.ty.to_string(),
            })
        })
        .collect();
    j["ports"] = Json::Array(ports);
    j["source"] = json!(m.source);
    j["manifest"] = m.manifest.as_ref().map_or(Json::Null, manifest);
    j
}

const fn shape(s: Shape) -> (&'static str, Option<usize>) {
    match s {
        Shape::Assign => ("assign", None),
        Shape::Call => ("call", None),
        Shape::If { arms } => ("if", Some(arms)),
        Shape::Case { arms } => ("case", Some(arms)),
        Shape::For => ("for", None),
        Shape::While => ("while", None),
        Shape::Repeat => ("repeat", None),
        Shape::Exit => ("exit", None),
        Shape::Continue => ("continue", None),
        Shape::Return => ("return", None),
    }
}

/// One traced scan: every statement with its counters, every scalar's
/// value at the end of the scan, and the fault if there was one.
#[must_use]
pub fn trace(module: &str, s: &Sample) -> Json {
    let t = &s.trace;
    let stmts: Vec<Json> = t
        .map()
        .stmts()
        .iter()
        .enumerate()
        .map(|(k, info)| {
            let id = StmtId(u32::try_from(k).unwrap_or(u32::MAX));
            let (shape, arms) = shape(info.shape);
            json!({
                "id": k,
                "pou": info.pou,
                "system": info.system,
                "line": info.line,
                "col": info.col,
                "shape": shape,
                "arms": arms,
                "parent": info.parent.map(|(p, arm)| json!([p.0, arm])),
                "hits": t.hits(id),
                "arm": t.arm(id),
                "arm_hits": t.arm_hits(id),
                "passes": t.passes(id),
            })
        })
        .collect();
    let values: serde_json::Map<String, Json> = t
        .values()
        .map(|(path, v)| (path.to_string(), st_value(v)))
        .collect();
    json!({
        "module": module,
        "scan": s.scan,
        "now_ms": millis(t.now()),
        "fault": s.fault.map(fault),
        "faulted_at": t.faulted_at().map(|id| id.0),
        "stmts": stmts,
        "values": values,
    })
}

fn presence<T>(name: &str, from: Option<&T>, to: Option<&T>) -> Json {
    let change = match (from, to) {
        (None, _) => "added",
        (_, None) => "removed",
        _ => "changed",
    };
    json!({ "name": name, "change": change })
}

/// What changed between two change sets.
#[must_use]
pub fn diff(d: &Diff) -> Json {
    let modules: Vec<Json> = d
        .modules
        .iter()
        .map(|m| match m {
            ModuleChange::Added(n) => json!({ "name": n, "change": "added" }),
            ModuleChange::Removed(n) => json!({ "name": n, "change": "removed" }),
            ModuleChange::Changed {
                name,
                source,
                manifest,
                params,
            } => {
                let params: Vec<Json> = params
                    .iter()
                    .map(|f| {
                        json!({
                            "name": f.name,
                            "from": value(f.from.as_ref().map(|p| p.value)),
                            "to": value(f.to.as_ref().map(|p| p.value)),
                        })
                    })
                    .collect();
                json!({
                    "name": name, "change": "changed",
                    "source": source, "manifest": manifest, "params": params,
                })
            }
        })
        .collect();
    let named = |fields: Vec<Json>| Json::Array(fields);
    json!({
        "empty": d.is_empty(),
        "modules": modules,
        "io_map": d.io_map,
        "devices": named(d.devices.iter().map(|f| presence(&f.name, f.from.as_ref(), f.to.as_ref())).collect()),
        "bindings": named(d.bindings.iter().map(|f| presence(&f.name, f.from.as_ref(), f.to.as_ref())).collect()),
        "calendar": d.calendar,
    })
}

/// A change set's content: each module's two files, the map, the calendar.
#[must_use]
pub fn change_set(cs: &ChangeSet) -> Json {
    let modules: serde_json::Map<String, Json> = cs
        .modules
        .iter()
        .map(|(n, f)| (n.clone(), json!({ "manifest": f.manifest, "source": f.source })))
        .collect();
    json!({
        "id": cs.id().to_string(),
        "modules": modules,
        "io_map": cs.io_map,
        "calendar": cs.calendar,
    })
}

/// One history entry.
#[must_use]
pub fn entry(e: &Entry) -> Json {
    let kind = match e.kind {
        Kind::Apply => json!({ "kind": "apply" }),
        Kind::Rollback { to } => json!({ "kind": "rollback", "to": to.to_string() }),
    };
    json!({
        "seq": e.seq,
        "id": e.id.to_string(),
        "kind": kind,
        "author": e.meta.author,
        "prompt": e.meta.prompt,
        "approver": e.meta.approver,
        "time_ms": e.meta.time_ms,
    })
}
