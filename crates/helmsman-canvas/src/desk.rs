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
//!
//! **On the box, per person** (card `desk-on-box`, Nick 2026-11-07: "kept
//! on the box per person when signed in, browser as the fallback"):
//! signed in, the desk is also kept by the box (`GET`/`PUT /api/desk`,
//! [`OnBox`]), so it follows the person from one screen to another. On
//! sign-in the page reads the box's and takes it when it is newer than
//! the one in front of it (`saved_at`: when the work in it, the edits and
//! the inputs, last changed); from then on each change goes to both,
//! through the same throttle. The box's desk is the session's owner's (an
//! account, or the role for a role-PIN session); a change of role on the
//! same account keeps it. Signed out, or with the box not answering, the
//! browser's is the desk.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use serde_json::Value as Json;

use crate::box_projects::{Answer, PCall, PReq};
use crate::review::Session;
use crate::sim_projects::{Storage, wall_ms};

/// The `localStorage` slot.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const SLOT: &str = "helmsman-canvas/desk";
/// Seconds between writes at most.
pub const EVERY_S: f64 = 1.0;
/// The most the slot holds, bytes.
pub const MAX_BYTES: usize = 1 << 20;
/// The most the box keeps of one desk, bytes (its `api::desk::MAX_DESK`).
pub const BOX_BYTES: usize = 512 * 1024;

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
    /// When the work in it (the edits and the inputs) last changed, Unix
    /// ms; 0: never. The newer of the box's and the browser's wins.
    pub saved_at: u64,
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

    /// The person's work in it, which `saved_at` dates: the edits and the
    /// two inputs (the project, file, screen and name follow the work).
    fn work(&self) -> String {
        serde_json::to_string(&(&self.drafts, &self.agent, &self.requirement)).unwrap_or_default()
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
    /// The work as last seen ([`Desk::work`]), and when it changed.
    work: String,
    saved_at: u64,
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
                work: desk.work(),
                saved_at: desk.saved_at,
                problem,
            },
            desk,
        )
    }

    /// When the desk's work last changed ([`Desk::saved_at`]).
    #[must_use]
    pub fn saved_at(&self) -> u64 {
        self.saved_at
    }

    /// Takes `desk` (the box's, newer) as the one in front of the person:
    /// its `saved_at` stands until the work changes again.
    pub fn adopt(&mut self, desk: &Desk) {
        self.work = desk.work();
        self.saved_at = desk.saved_at;
    }

    /// Keeps `desk` if it changed and the last write is [`EVERY_S`] old,
    /// dated when its work changed; that desk, unless the throttle held
    /// it back (for the box to keep too, [`OnBox::keep`]).
    pub fn keep(&mut self, desk: &Desk, now: f64) -> Option<Desk> {
        if self.at.is_some_and(|a| now - a < EVERY_S) {
            return None;
        }
        let mut desk = desk.clone();
        let work = desk.work();
        if work != self.work {
            self.work = work;
            // Never back in time, even on a clock behind the box's desk.
            self.saved_at = wall_ms().max(self.saved_at + 1);
        }
        desk.saved_at = self.saved_at;
        let (text, dropped) = desk.text_within(MAX_BYTES);
        if text == self.last {
            return Some(desk);
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
        Some(desk)
    }
}

/// Whose desk the box keeps for `s`: the account, or the role for a
/// role-PIN session (as the box has it, `api::trend::owner_of`).
fn owner(s: Option<&Session>) -> Option<String> {
    s.map(|s| match &s.user {
        Some(u) => format!("user:{u}"),
        None => format!("role:{}", s.role),
    })
}

/// The desk on the box for whoever is signed in (`GET`/`PUT /api/desk`).
#[derive(Debug, Default)]
pub struct OnBox {
    /// Whose desk was asked for.
    owner: Option<String>,
    /// The box's answer is in: changes go to it.
    ready: bool,
    /// What was last sent.
    sent: String,
    at: Option<f64>,
    /// Why the box does not keep the desk now, or what it left out.
    pub problem: Option<String>,
    queue: Vec<PReq>,
}

impl OnBox {
    /// What to send now.
    pub fn poll(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    fn ask(&mut self) {
        self.ready = false;
        self.queue.push(PReq::get(PCall::Desk, "/api/desk".into()));
    }

    /// Every frame: a new person signed in asks the box for their desk;
    /// signed out, the browser's is the desk.
    pub fn step(&mut self, session: Option<&Session>) {
        let who = owner(session);
        if who != self.owner {
            self.owner = who;
            self.ready = false;
            self.sent.clear();
            self.problem = None;
            if self.owner.is_some() {
                self.ask();
            }
        }
    }

    /// Sends `desk` (dated by [`Keeper::keep`]) when it changed, the box
    /// has answered and the last send is [`EVERY_S`] old; past
    /// [`BOX_BYTES`] the biggest edits stay in the browser only.
    pub fn keep(&mut self, desk: &Desk, now: f64) {
        if !self.ready || self.owner.is_none() || self.at.is_some_and(|a| now - a < EVERY_S) {
            return;
        }
        let (text, dropped) = desk.text_within(BOX_BYTES);
        if text == self.sent {
            return;
        }
        self.at = Some(now);
        self.problem = (!dropped.is_empty()).then(|| {
            format!(
                "too large to keep on the box, kept in this browser only: unsaved edits to {}",
                dropped.join(", ")
            )
        });
        self.queue.push(PReq {
            call: PCall::SaveDesk,
            method: "PUT",
            url: "/api/desk".into(),
            body: Some(text.clone().into_bytes()),
            ctype: "application/json",
        });
        self.sent = text;
    }

    /// Takes in the box's answer; the box's desk when it is newer than the
    /// one in front of the person (dated `local`), to put in front of them.
    pub fn answer(&mut self, call: &PCall, a: &Answer, local: u64) -> Option<Desk> {
        let j: Json = serde_json::from_str(&a.body).unwrap_or(Json::Null);
        let error = || j["error"].as_str().unwrap_or("no answer").to_string();
        match call {
            PCall::Desk if a.status == 200 => {
                self.ready = true;
                self.sent.clear();
                let d: Desk = serde_json::from_value(j["desk"].clone()).ok()?;
                (d.saved_at > local).then_some(d)
            }
            PCall::Desk => {
                // Signed out since, or the box keeps no desks: the
                // browser's.
                if a.status != 401 {
                    self.problem = Some(format!("the box does not keep your desk: {}", error()));
                }
                None
            }
            // The box holds a newer one (another screen's): read it.
            PCall::SaveDesk if a.status == 409 => {
                self.ask();
                None
            }
            PCall::SaveDesk if (200..300).contains(&a.status) => {
                if j["kept"] == Json::Bool(false) {
                    self.problem = Some("the box keeps your desk only until it restarts".into());
                }
                None
            }
            PCall::SaveDesk => {
                self.problem = Some(format!("your desk is not kept on the box: {}", error()));
                // Sent again with the next change.
                self.sent.clear();
                None
            }
            _ => None,
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
        let (k2, back) = Keeper::load(Box::new(MemStorage(Some(k.last.clone()))));
        assert!(back.saved_at > 0, "dated when the work changed");
        assert_eq!(k2.saved_at(), back.saved_at);
        assert_eq!(Desk { saved_at: 0, ..back }, d);
    }

    /// `saved_at` dates the work (the edits and the inputs), so a page that
    /// only opens a project or changes screen never makes an older desk
    /// look newer than the one on the box.
    #[test]
    fn saved_at_moves_with_the_work_only_and_never_back() {
        let stored = Desk {
            saved_at: 5,
            agent: "a".into(),
            ..Desk::default()
        };
        let (mut k, d) = Keeper::load(Box::new(MemStorage(Some(stored.text_within(MAX_BYTES).0))));
        let mut d = Desk {
            project: Some("line".into()),
            screen: Some("monitor".into()),
            user: "alice".into(),
            ..d
        };
        assert_eq!(k.keep(&d, 1.0).unwrap().saved_at, 5, "not work");
        d.agent = "ab".into();
        let later = k.keep(&d, 3.0).unwrap().saved_at;
        assert!(later > 5, "{later}");
        // A box desk dated ahead of this clock: the next change still
        // comes after it.
        let ahead = Desk {
            saved_at: u64::MAX / 2,
            ..d.clone()
        };
        k.adopt(&ahead);
        d.agent = "abc".into();
        assert_eq!(k.keep(&d, 5.0).unwrap().saved_at, u64::MAX / 2 + 1);
    }

    fn answer(status: u16, body: &str) -> Answer {
        Answer {
            status,
            retry_after: None,
            body: body.into(),
        }
    }

    fn session(user: Option<&str>, role: &str) -> Session {
        Session {
            user: user.map(str::to_string),
            role: role.into(),
        }
    }

    /// Signed in, the page asks for the box's desk first and sends nothing
    /// until it answers; a newer one is taken, an older one is written
    /// over. A role switch on the same account keeps the desk without
    /// asking again; signed out, nothing goes to the box.
    #[test]
    fn on_the_box_the_newer_desk_wins_and_a_role_switch_keeps_it() {
        let mut b = OnBox::default();
        let mine = Desk {
            saved_at: 10,
            agent: "here".into(),
            ..Desk::default()
        };
        b.step(None);
        b.keep(&mine, 0.0);
        assert!(b.poll().is_empty(), "signed out: the browser only");
        let alice = session(Some("alice"), "engineer");
        b.step(Some(&alice));
        b.keep(&mine, 0.0);
        let asked = b.poll();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!((asked[0].method, asked[0].call.clone()), ("GET", PCall::Desk));
        // The box's is older: kept here, and sent.
        let older = r#"{"owner":"user:alice","desk":{"saved_at":3,"agent":"there"}}"#;
        assert_eq!(b.answer(&PCall::Desk, &answer(200, older), 10), None);
        b.keep(&mine, 1.0);
        let sent = b.poll();
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].method, sent[0].url.as_str()), ("PUT", "/api/desk"));
        assert_eq!(
            Desk::parse(&String::from_utf8_lossy(sent[0].body.as_deref().unwrap())),
            mine
        );
        b.keep(&mine, 5.0);
        assert!(b.poll().is_empty(), "unchanged: not sent again");
        // The same account in another role: the same desk, nothing asked.
        b.step(Some(&session(Some("alice"), "operator")));
        assert!(b.poll().is_empty(), "the same desk: nothing asked");
        // A newer one on the box (a 409 on save): read and taken.
        let changed = Desk {
            agent: "here, more".into(),
            ..mine.clone()
        };
        b.keep(&changed, 9.0);
        assert_eq!(b.poll().len(), 1);
        assert_eq!(
            b.answer(&PCall::SaveDesk, &answer(409, r#"{"saved_at":20}"#), 10),
            None
        );
        let reread = b.poll();
        assert_eq!(reread[0].call, PCall::Desk);
        let newer = r#"{"owner":"user:alice","desk":{"saved_at":20,"agent":"from the Studio"}}"#;
        let took = b.answer(&PCall::Desk, &answer(200, newer), 10).unwrap();
        assert_eq!(took.agent, "from the Studio");
        // A PIN role switch is another owner: asked again, and this desk
        // goes to it when the box has none newer.
        b.step(Some(&session(None, "operator")));
        assert_eq!(b.poll()[0].call, PCall::Desk);
        let none = r#"{"owner":"role:operator","desk":null}"#;
        assert_eq!(b.answer(&PCall::Desk, &answer(200, none), 20), None);
        b.keep(&took, 20.0);
        assert_eq!(b.poll().len(), 1, "the desk on screen kept for the role");
        // Signed out: nothing more to the box.
        b.step(None);
        b.keep(&mine, 30.0);
        assert!(b.poll().is_empty(), "signed out: nothing sent");
    }

    #[test]
    fn a_box_that_refuses_or_does_not_answer_leaves_the_browsers_desk() {
        let mut b = OnBox::default();
        b.step(Some(&session(Some("alice"), "engineer")));
        b.poll();
        assert_eq!(b.answer(&PCall::Desk, &answer(401, "{}"), 0), None);
        assert_eq!(b.problem, None, "signed out since: quietly the browser");
        b.keep(&Desk::default(), 1.0);
        assert!(b.poll().is_empty(), "never answered: nothing sent");
        let mut b = OnBox::default();
        b.step(Some(&session(Some("alice"), "engineer")));
        b.poll();
        assert_eq!(b.answer(&PCall::Desk, &answer(0, "refused"), 0), None);
        assert!(b.problem.as_deref().unwrap().contains("does not keep"));
        // Garbage from the box is no desk to take.
        let mut b = OnBox::default();
        b.step(Some(&session(Some("alice"), "engineer")));
        b.poll();
        for bad in ["{not json", r#"{"desk":7}"#, r#"{"desk":{"agent":7}}"#] {
            assert_eq!(b.answer(&PCall::Desk, &answer(200, bad), 0), None, "{bad}");
        }
        // An oversized desk keeps its biggest edits in the browser only.
        let mut big = Desk::default();
        big.drafts
            .entry("p".into())
            .or_default()
            .insert("big.st".into(), draft(&"x".repeat(BOX_BYTES)));
        b.answer(&PCall::Desk, &answer(200, r#"{"desk":null}"#), 0);
        b.keep(&big, 1.0);
        let sent = b.poll();
        assert!(sent[0].body.as_ref().unwrap().len() <= BOX_BYTES);
        assert!(b.problem.as_deref().unwrap().contains("p/big.st"));
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
