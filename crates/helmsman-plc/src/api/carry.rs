//! What a deploy does to the running state, by name and type
//! ([`helmsman_iec::Carry`]): which values it keeps, which start again at
//! their initial value (a new variable, or one whose type changed) and
//! which it drops (no longer declared). Card deploy-state-preview.
//!
//! - **Preview**, on the proposal (`state`, `preview: true`): worked out
//!   once, when the proposal is stored. Both change sets are compiled
//!   afresh, here on the API thread, and the target's programs restore
//!   from the running build's: the same restore the scan thread runs, by
//!   the same names and types, on programs nobody scans. The scan
//!   thread's live state is never read. That is exact for names and types,
//!   and the approval refuses a proposal drafted against another running
//!   change set (stale, 409), so what the preview was worked out against
//!   is what an apply swaps from. It is still a preview: a live value
//!   that no longer fits its slot also resets, which only the deploy sees.
//! - **What happened**, in the apply's answer (`state`, `preview: false`)
//!   and its audit entry: the carry the scan thread reported for the swap
//!   it made ([`crate::Runtime::last_carry`]).

use helmsman_iec::Carry;
use serde_json::{Value as Json, json};

use super::swap::compile_set;
use crate::Module;
use crate::spec::Defaults;
use crate::store::ChangeSet;

/// Most names one list of a [`summary`] spells out; past it, "and N more".
pub const SHOWN: usize = 8;

/// One module's state names, by module name.
pub type Names = Vec<(String, Vec<String>)>;

/// Each module's state names, in order.
pub fn names(modules: &[Module]) -> Names {
    modules
        .iter()
        .map(|m| (m.name.clone(), m.program.state_names()))
        .collect()
}

/// The preview of deploying `target` over `running` (see the module
/// docs).
pub fn preview(running: &ChangeSet, target: &ChangeSet, d: &Defaults) -> Json {
    let (old, new) = match (compile_set(running, d), compile_set(target, d)) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(e), _) | (_, Err(e)) => {
            return json!({
                "preview": true,
                "running": true,
                "modules": [],
                "error": e,
                "summary": "cannot tell what the deploy keeps: a module does not compile",
            });
        }
    };
    let before = names(&old);
    let after: Vec<String> = new.iter().map(|m| m.name.clone()).collect();
    let carried: Vec<(String, Carry)> = new
        .into_iter()
        .filter_map(|mut m| {
            let o = old.iter().find(|o| o.name == m.name)?;
            let c = m.program.restore(&o.program);
            Some((m.name, c))
        })
        .collect();
    describe(true, &before, &after, &carried)
}

/// Nothing runs, so nothing is kept.
pub fn nothing(preview: bool) -> Json {
    json!({
        "preview": preview,
        "running": false,
        "kept": 0,
        "modules": [],
        "summary": "nothing running to keep: every value starts at its initial value",
    })
}

/// The state JSON of a step from modules `before` (with their state
/// names) to modules named `after`, `carried` being what each kept
/// module's restore did.
pub fn describe(preview: bool, before: &Names, after: &[String], carried: &[(String, Carry)]) -> Json {
    let mut modules = Vec::new();
    let mut kept = 0;
    for name in after {
        let Some((_, c)) = carried.iter().find(|(n, _)| n == name) else {
            modules.push(json!({ "module": name, "new": true, "kept": [], "reset": [], "dropped": [] }));
            continue;
        };
        let old: &[String] = before
            .iter()
            .find(|(n, _)| n == name)
            .map_or(&[], |(_, v)| v.as_slice());
        let reset: Vec<Json> = c
            .reset
            .iter()
            .map(|v| {
                let why = if old.contains(v) { "type changed" } else { "new" };
                json!({ "name": v, "why": why })
            })
            .collect();
        kept += c.kept.len();
        modules.push(json!({
            "module": name,
            "new": false,
            "kept": c.kept,
            "reset": reset,
            "dropped": c.dropped,
        }));
    }
    for (name, vars) in before.iter().filter(|(n, _)| !after.contains(n)) {
        modules.push(json!({ "module": name, "removed": true, "kept": [], "reset": [], "dropped": vars }));
    }
    let mut j = json!({
        "preview": preview,
        "running": true,
        "kept": kept,
        "modules": modules,
    });
    j["summary"] = json!(summary(&j));
    j
}

/// `a, b, c and 4 more`.
fn list(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(SHOWN).map(String::as_str).collect();
    let more = names.len().saturating_sub(SHOWN);
    if more > 0 {
        format!("{} and {more} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

/// The plain sentence for a state JSON (as [`describe`] makes it): "keeps
/// 12 values, resets COUNT (type changed), drops OLD_FLAG". Names are
/// `MODULE.NAME` when more than one module is in the step.
#[must_use]
pub fn summary(j: &Json) -> String {
    if j["running"] == false {
        return "nothing running to keep: every value starts at its initial value".into();
    }
    let modules = j["modules"].as_array().map_or(&[][..], Vec::as_slice);
    let qualify = modules.len() > 1;
    let name = |m: &Json, v: &str| {
        if qualify {
            format!("{}.{v}", m["module"].as_str().unwrap_or("?"))
        } else {
            v.to_string()
        }
    };
    let strs = |m: &Json, key: &str| -> Vec<String> {
        m[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(|v| name(m, v))
            .collect()
    };
    let mut retyped = Vec::new();
    let mut fresh = Vec::new();
    let mut dropped = Vec::new();
    let mut new_modules = Vec::new();
    let mut removed = Vec::new();
    for m in modules {
        let module = m["module"].as_str().unwrap_or("?").to_string();
        if m["new"] == true {
            new_modules.push(module);
            continue;
        }
        if m["removed"] == true {
            removed.push(module);
            continue;
        }
        for r in m["reset"].as_array().into_iter().flatten() {
            let v = name(m, r["name"].as_str().unwrap_or("?"));
            if r["why"] == "type changed" {
                retyped.push(v);
            } else {
                fresh.push(v);
            }
        }
        dropped.extend(strs(m, "dropped"));
    }
    let kept = j["kept"].as_u64().unwrap_or(0);
    let mut parts = vec![match kept {
        1 => "keeps 1 value".to_string(),
        n => format!("keeps {n} values"),
    }];
    if !retyped.is_empty() {
        parts.push(format!("resets {} (type changed)", list(&retyped)));
    }
    if !fresh.is_empty() {
        parts.push(format!("starts {} at the initial value (new)", list(&fresh)));
    }
    if !dropped.is_empty() {
        parts.push(format!("drops {}", list(&dropped)));
    }
    for m in &new_modules {
        parts.push(format!("new module {m}: nothing running to keep"));
    }
    for m in &removed {
        parts.push(format!("removes module {m} and its values"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn the_sentence_names_resets_and_drops_and_counts_the_kept() {
        let before = vec![("m".to_string(), s(&["A", "B", "COUNT", "OLD_FLAG"]))];
        let carry = Carry {
            kept: s(&["A", "B"]),
            reset: s(&["COUNT", "FRESH"]),
            dropped: s(&["OLD_FLAG"]),
        };
        let j = describe(true, &before, &s(&["m"]), &[("m".into(), carry)]);
        assert_eq!(
            j["summary"],
            "keeps 2 values, resets COUNT (type changed), starts FRESH at the initial value (new), drops OLD_FLAG"
        );
        assert_eq!(
            j["modules"][0]["reset"][0],
            json!({ "name": "COUNT", "why": "type changed" })
        );
    }

    #[test]
    fn a_long_list_is_capped_and_modules_are_named_when_several() {
        let many: Vec<String> = (0..50).map(|n| format!("V{n}")).collect();
        let before = vec![("a".to_string(), many.clone()), ("gone".to_string(), s(&["X"]))];
        let carry = Carry {
            kept: Vec::new(),
            reset: Vec::new(),
            dropped: many,
        };
        let j = describe(false, &before, &s(&["a", "b"]), &[("a".into(), carry)]);
        let text = j["summary"].as_str().unwrap();
        assert!(text.starts_with("keeps 0 values, drops a.V0, a.V1,"), "{text}");
        assert!(text.contains("a.V7 and 42 more"), "{text}");
        assert!(!text.contains("a.V8"), "{text}");
        assert!(text.contains("new module b: nothing running to keep"), "{text}");
        assert!(text.contains("removes module gone and its values"), "{text}");
        assert_eq!(j["preview"], false);
    }

    #[test]
    fn nothing_running_says_so() {
        assert_eq!(
            nothing(true)["summary"],
            "nothing running to keep: every value starts at its initial value"
        );
    }
}
