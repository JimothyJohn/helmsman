//! Which [`Tier`] a step from one change set to another is in, from the
//! two change sets alone.
//!
//! The rules, each applied to every module, the map and the calendar;
//! the step's tier is the highest any of them gives:
//!
//! - **Output-affecting**: the I/O map changed at all (bindings, devices,
//!   safe values, units: whatever drives a point lives there); a WASM
//!   driver was added, removed or changed (it is what reaches a device:
//!   whatever it writes there, its outputs included, is its own); the
//!   calendar's `drive` lines changed; a module was added or removed that
//!   drives a signal or has a `%Q` window; a module's `drives` or `%Q`
//!   window changed; any file that should parse does not (a manifest, the
//!   calendar), as nothing can then be said about it.
//! - **Logic**: a module's program (`module.st` or `module.wasm`, or one
//!   for the other), or anything in its manifest but param values,
//!   changed; a module that drives nothing was added or removed; the
//!   calendar changed but drives the same outputs.
//! - **Param**: only param values changed (the manifest may be re-rendered,
//!   its program byte for byte the same), each new value of the param's
//!   type and within the range the *running* manifest declares.
//!
//! When in doubt, up: a step this cannot read is output-affecting.
//!
//! The manifest stands for what a program can touch, ST or WASM alike:
//! nothing is applied until every module is compiled (or loaded) and held
//! to its manifest (`api::swap::compile_set`), which refuses a port, an ST
//! `VAR_OUTPUT` or a WASM `helmsman/v1/out` import, that the manifest does
//! not declare. A manifest cannot understate a program into a lower tier.

use std::collections::BTreeSet;

use crate::calendar;
use crate::manifest::Manifest;
use crate::store::{ChangeSet, ModuleFiles};

use super::Tier;

/// A step's tier and why: one line per thing that changed, each with the
/// tier it alone would give.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    pub tier: Tier,
    pub reasons: Vec<(Tier, String)>,
}

impl Class {
    fn add(&mut self, tier: Tier, why: String) {
        self.tier = self.tier.max(tier);
        self.reasons.push((tier, why));
    }
}

/// What can drive an output, as a manifest says: the signals it drives
/// (case-insensitive, as ST is) and its `%Q` bytes.
fn output_surface(m: &Manifest) -> (BTreeSet<String>, (usize, usize)) {
    let drives = m.drives.iter().map(|d| d.to_ascii_lowercase()).collect();
    let q = m.q.map_or((0, 0), |w| (w.start, w.len));
    (drives, q)
}

fn drives_anything(m: &Manifest) -> bool {
    !m.drives.is_empty() || m.q.is_some_and(|w| w.len > 0)
}

/// The step from `base` (what runs) to `target`.
#[must_use]
pub fn classify(base: &ChangeSet, target: &ChangeSet) -> Class {
    let mut c = Class {
        tier: Tier::Param,
        reasons: Vec::new(),
    };
    let names: BTreeSet<&String> = base.modules.keys().chain(target.modules.keys()).collect();
    for name in names {
        match (base.modules.get(name), target.modules.get(name)) {
            (Some(a), Some(b)) if a != b => module(&mut c, name, a, b),
            (Some(f), None) | (None, Some(f)) => {
                let verb = if base.modules.contains_key(name) {
                    "removed"
                } else {
                    "added"
                };
                match Manifest::parse(&f.manifest) {
                    Ok(m) if !drives_anything(&m) => {
                        c.add(Tier::Logic, format!("module {name} {verb}; it drives nothing"));
                    }
                    Ok(_) => c.add(
                        Tier::OutputAffecting,
                        format!("module {name} {verb}; it drives outputs"),
                    ),
                    Err(_) => c.add(
                        Tier::OutputAffecting,
                        format!("module {name} {verb}; its manifest does not parse"),
                    ),
                }
            }
            _ => {}
        }
    }
    if base.io_map != target.io_map {
        c.add(
            Tier::OutputAffecting,
            "the I/O map changed (bindings, devices or safe values)".into(),
        );
    }
    let guests: BTreeSet<&String> = base.drivers.keys().chain(target.drivers.keys()).collect();
    for name in guests {
        let verb = match (base.drivers.get(name), target.drivers.get(name)) {
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            (Some(a), Some(b)) if a != b => "changed",
            _ => continue,
        };
        c.add(
            Tier::OutputAffecting,
            format!("WASM driver {name} {verb}; a driver reaches its devices"),
        );
    }
    if base.calendar != target.calendar {
        let drives = |s: Option<&str>| {
            s.map_or(Some(Vec::new()), |s| {
                calendar::text::parse(s).ok().map(|k| k.drives)
            })
        };
        match (
            drives(base.calendar.as_deref()),
            drives(target.calendar.as_deref()),
        ) {
            (Some(a), Some(b)) if a == b => {
                c.add(
                    Tier::Logic,
                    "the calendar changed; it drives the same outputs".into(),
                );
            }
            (Some(_), Some(_)) => c.add(Tier::OutputAffecting, "the calendar's drive lines changed".into()),
            _ => c.add(
                Tier::OutputAffecting,
                "the calendar changed and does not parse".into(),
            ),
        }
    }
    if c.reasons.is_empty() {
        c.reasons.push((Tier::Param, "no change".into()));
    }
    c
}

fn module(c: &mut Class, name: &str, a: &ModuleFiles, b: &ModuleFiles) {
    let (Ok(ma), Ok(mb)) = (Manifest::parse(&a.manifest), Manifest::parse(&b.manifest)) else {
        c.add(
            Tier::OutputAffecting,
            format!("module {name}: a manifest does not parse"),
        );
        return;
    };
    if output_surface(&ma) != output_surface(&mb) {
        c.add(
            Tier::OutputAffecting,
            format!("module {name}: what it drives (drives or %Q window) changed"),
        );
        return;
    }
    if a.code != b.code {
        c.add(Tier::Logic, format!("module {name}: program changed"));
    }
    // The target manifest with the running param values: anything left
    // that differs is more than a param value.
    let mut same = mb.clone();
    let mut changed = Vec::new();
    if same.params.len() == ma.params.len() {
        for (p, q) in same.params.iter_mut().zip(&ma.params) {
            if p.name == q.name && p.value != q.value {
                changed.push((q.clone(), p.value));
                p.value = q.value;
            }
        }
    }
    if same != ma {
        c.add(
            Tier::Logic,
            format!("module {name}: manifest changed beyond param values"),
        );
        return;
    }
    for (p, to) in changed {
        if p.admits(to) {
            c.add(
                Tier::Param,
                format!("module {name}: param {} within its declared range", p.name),
            );
        } else {
            c.add(
                Tier::Logic,
                format!("module {name}: param {} outside its declared range", p.name),
            );
        }
    }
}
