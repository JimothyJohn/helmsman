//! Placeholder I/O for a Skipper (Nick, 2026-11-24: "you really need a way
//! to create simulated, place holder inputs and outputs when starting a
//! program, it doesn't always need to map to an actual device").
//!
//! A Skipper binds each `VAR_INPUT` of its program to an `in` point of its
//! I/O list by name, and each `VAR_OUTPUT` to an `out` point. A port no
//! point backs gets a placeholder: an `in NAME TYPE sim` or `out NAME TYPE
//! sim` line (the `sim` points of the box and the Skipper, held in memory;
//! a sim input is set from the page). The page adds them to the project's
//! `io.map` itself, after an Agent answer lands and again at Deploy, so a
//! program never waits on a point an engineer has not wired yet. The map
//! it adds to is the one the deploy sends ([`crate::skipper_map`]: the
//! Skipper's own list, the project's devices on top).

use helmsman_iec::{PortDir, Ty};

/// The comment line above the placeholders the page adds.
pub const HEADER: &str = "# placeholders: no device has these points yet (set the inputs on the I/O tab)";

/// The points of an I/O list: lower-cased name, `true` for an `in`.
#[must_use]
pub fn points(list: &str) -> Vec<(String, bool)> {
    list.lines()
        .filter_map(|l| {
            let l = l.split('#').next().unwrap_or("");
            let mut w = l.split_whitespace();
            let input = match w.next()? {
                "in" => true,
                "out" => false,
                _ => return None,
            };
            Some((w.next()?.to_ascii_lowercase(), input))
        })
        .collect()
}

/// A point's type in a map line, for a port of type `ty`; `None` for one
/// no point can carry (TIME, the calendar types).
#[must_use]
pub const fn map_type(ty: Ty) -> Option<&'static str> {
    if matches!(ty, Ty::Bool) {
        Some("bool")
    } else if ty.is_real() {
        Some("real")
    } else if ty.is_integral() {
        Some("int")
    } else {
        None
    }
}

/// What adding placeholders to `list` for `program` gives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Added {
    /// The map with the placeholder lines at its end.
    pub map: String,
    /// The placeholder lines added, as written.
    pub lines: Vec<String>,
    /// Ports that cannot have one: a point of that name faces the other
    /// way, or the port's type has no point type. Said, never fixed here.
    pub cannot: Vec<String>,
}

/// The placeholders `program` (the program as it builds, test files left
/// out) needs on `list`, the map the deploy will send. `None` when it
/// needs none (every port has its point) or the program does not build
/// (the build says why).
#[must_use]
pub fn add(program: &str, sizes: helmsman_iec::ImageSizes, list: &str) -> Option<Added> {
    let compiled = helmsman_iec::compile(program, sizes).ok()?;
    let have = points(list);
    let mut added = Added::default();
    for p in compiled.ports() {
        let name = p.name.to_ascii_lowercase();
        let input = p.dir == PortDir::Input;
        match have.iter().find(|(n, _)| *n == name) {
            Some((_, i)) if *i == input => {}
            Some((_, i)) => added.cannot.push(format!(
                "{name} is a {} in the program and an {} point on the I/O list",
                if input { "VAR_INPUT" } else { "VAR_OUTPUT" },
                if *i { "in" } else { "out" }
            )),
            None => match map_type(p.ty) {
                Some(ty) => added
                    .lines
                    .push(format!("{} {name} {ty} sim", if input { "in " } else { "out" })),
                None => added
                    .cannot
                    .push(format!("{name}: a {:?} has no I/O point type", p.ty)),
            },
        }
    }
    if added.lines.is_empty() && added.cannot.is_empty() {
        return None;
    }
    let mut map = list.trim_end().to_string();
    if !added.lines.is_empty() {
        if !map.is_empty() {
            map.push('\n');
        }
        if !map.lines().any(|l| l.trim() == HEADER) {
            map.push_str(HEADER);
            map.push('\n');
        }
        map.push_str(&added.lines.join("\n"));
    }
    map.push('\n');
    added.map = map;
    Some(added)
}

/// The words for what [`add`] did, for the Messages.
#[must_use]
pub fn said(a: &Added) -> Option<String> {
    let mut parts = Vec::new();
    if !a.lines.is_empty() {
        let names: Vec<&str> = a
            .lines
            .iter()
            .filter_map(|l| l.split_whitespace().nth(1))
            .collect();
        parts.push(format!(
            "Placeholder points added to io.map for {}: no device has them yet. Set the inputs on the I/O tab.",
            names.join(", ")
        ));
    }
    if !a.cannot.is_empty() {
        parts.push(format!("No placeholder for {}.", a.cannot.join("; ")));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}
