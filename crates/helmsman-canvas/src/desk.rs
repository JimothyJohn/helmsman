//! The desk: what the person had in front of them on a box's page, kept in
//! this browser so a reload, a box restart, an error or a change of role
//! puts it back (Nick, 2026-11-07: "the interface and inputs need to be
//! more persistent so they don't just disappear"). Permissions still apply:
//! a restored edit is only a draft until someone allowed to saves it.
//!
//! Kept: the open project and file, every unsaved edit (with the box's
//! version it was made against, so a file someone changed since comes
//! back as a conflict instead of overwriting their change), the Agent's
//! and the Requirements' half-typed inputs, the screen, and the sign-in
//! name. Never kept: a password or PIN.
//!
//! One `localStorage` slot (each box serves its own page, so the slot is
//! per box), written at most every [`EVERY_S`] and only when something
//! changed; at most [`MAX_BYTES`], the oldest drafts going first past it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sim_projects::Storage;

/// The `localStorage` slot.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const SLOT: &str = "helmsman-canvas/desk";
/// Seconds between writes at most.
pub const EVERY_S: f64 = 1.0;
/// The most the slot holds, bytes.
pub const MAX_BYTES: usize = 1 << 20;

/// An unsaved edit: the text, and the box's version of the file it was
/// made against (`None`: a file the box did not have yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub version: Option<String>,
}

/// Everything the desk keeps. Unknown or missing fields read as empty, so
/// an older or newer page's desk still loads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Desk {
    pub project: Option<String>,
    pub file: Option<String>,
    /// Project, then file, then the edit.
    pub drafts: BTreeMap<String, BTreeMap<String, Draft>>,
    /// The Agent tab's input.
    pub agent: String,
    /// The Requirements tab's input.
    pub requirement: String,
    /// `editor` or `monitor`.
    pub screen: Option<String>,
    /// The sign-in form's name (never a password or PIN).
    pub user: String,
}

impl Desk {
    /// Reads a desk; anything that does not parse is an empty desk.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// As text, at most `max` bytes: past it, whole drafts are dropped
    /// (largest first) until it fits; the rest is always kept. The second
    /// value names the files dropped.
    #[must_use]
    pub fn text_within(&self, max: usize) -> (String, Vec<String>) {
        let mut d = self.clone();
        let mut dropped = Vec::new();
        loop {
            let t = serde_json::to_string(&d).unwrap_or_default();
            if t.len() <= max {
                return (t, dropped);
            }
            let biggest = d
                .drafts
                .iter()
                .flat_map(|(p, m)| m.iter().map(move |(f, dr)| (dr.text.len(), p.clone(), f.clone())))
                .max();
            let Some((_, p, f)) = biggest else {
                // No drafts left and still too big: keep nothing.
                return ("{}".into(), dropped);
            };
            if let Some(m) = d.drafts.get_mut(&p) {
                m.remove(&f);
                if m.is_empty() {
                    d.drafts.remove(&p);
                }
            }
            dropped.push(format!("{p}/{f}"));
        }
    }
}

/// Writes the desk to its store when it changed, at most every
/// [`EVERY_S`]; every store access is wrapped.
pub struct Keeper {
    store: Box<dyn Storage>,
    /// What the store holds, as far as this page knows.
    last: String,
    at: Option<f64>,
    /// Why the desk is not kept, or what was left out.
    pub problem: Option<String>,
}

impl std::fmt::Debug for Keeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keeper")
            .field("problem", &self.problem)
            .finish_non_exhaustive()
    }
}

impl Keeper {
    /// The keeper of `store`, and the desk it holds.
    pub fn load(store: Box<dyn Storage>) -> (Self, Desk) {
        let (last, problem) = match store.load() {
            Ok(t) => (t.unwrap_or_default(), None),
            Err(e) => (
                String::new(),
                Some(format!("this browser does not keep your work ({e})")),
            ),
        };
        let desk = Desk::parse(&last);
        (
            Self {
                store,
                last,
                at: None,
                problem,
            },
            desk,
        )
    }

    /// Keeps `desk` if it changed and the last write is [`EVERY_S`] old.
    pub fn keep(&mut self, desk: &Desk, now: f64) {
        if self.at.is_some_and(|a| now - a < EVERY_S) {
            return;
        }
        let (text, dropped) = desk.text_within(MAX_BYTES);
        if text == self.last {
            return;
        }
        self.at = Some(now);
        match self.store.save(&text) {
            Ok(()) => {
                self.last = text;
                self.problem = (!dropped.is_empty()).then(|| {
                    format!(
                        "too large to keep in this browser: unsaved edits to {}",
                        dropped.join(", ")
                    )
                });
            }
            Err(e) => self.problem = Some(format!("this browser does not keep your work ({e})")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_projects::MemStorage;

    fn draft(text: &str) -> Draft {
        Draft {
            text: text.into(),
            version: Some("v1".into()),
        }
    }

    #[test]
    fn garbage_or_another_pages_desk_reads_as_far_as_it_can() {
        for bad in ["", "{not json", "[]", "null", "\u{0}"] {
            assert_eq!(Desk::parse(bad), Desk::default(), "{bad:?}");
        }
        let d = Desk::parse(r#"{"project":"line","future_field":[1,2],"agent":7}"#);
        assert_eq!(
            d,
            Desk::default(),
            "a wrong type anywhere: nothing restored rather than half"
        );
        let d = Desk::parse(r#"{"project":"line","future_field":[1,2]}"#);
        assert_eq!(d.project.as_deref(), Some("line"), "an unknown field is ignored");
    }

    #[test]
    fn past_the_limit_the_biggest_drafts_go_first_and_the_rest_stays() {
        let mut d = Desk {
            project: Some("p".into()),
            agent: "half typed".into(),
            ..Desk::default()
        };
        d.drafts
            .entry("p".into())
            .or_default()
            .insert("big.st".into(), draft(&"x".repeat(900)));
        d.drafts
            .entry("p".into())
            .or_default()
            .insert("small.st".into(), draft("y"));
        let (t, dropped) = d.text_within(400);
        assert!(t.len() <= 400);
        assert_eq!(dropped, ["p/big.st"]);
        let back = Desk::parse(&t);
        assert_eq!(back.agent, "half typed");
        assert!(back.drafts["p"].contains_key("small.st"));
        let (t, _) = d.text_within(5);
        assert_eq!(t, "{}");
    }

    #[test]
    fn the_keeper_writes_changes_at_most_once_a_second_and_round_trips() {
        let (mut k, d) = Keeper::load(Box::new(MemStorage(None)));
        assert_eq!(d, Desk::default());
        let mut d = Desk {
            agent: "a".into(),
            ..Desk::default()
        };
        k.keep(&d, 10.0);
        let first = k.last.clone();
        d.agent = "ab".into();
        k.keep(&d, 10.5);
        assert_eq!(k.last, first, "not within the second");
        k.keep(&d, 11.1);
        assert_eq!(Desk::parse(&k.last).agent, "ab");
        let (_, back) = Keeper::load(Box::new(MemStorage(Some(k.last.clone()))));
        assert_eq!(back, d);
    }

    #[test]
    fn a_store_that_refuses_is_said_and_nothing_breaks() {
        struct Refuses;
        impl Storage for Refuses {
            fn load(&self) -> Result<Option<String>, String> {
                Err("blocked".into())
            }
            fn save(&mut self, _: &str) -> Result<(), String> {
                Err("quota".into())
            }
        }
        let (mut k, d) = Keeper::load(Box::new(Refuses));
        assert_eq!(d, Desk::default());
        assert!(k.problem.as_deref().unwrap().contains("blocked"));
        k.keep(
            &Desk {
                agent: "x".into(),
                ..Desk::default()
            },
            1.0,
        );
        assert!(k.problem.as_deref().unwrap().contains("quota"));
    }
}
