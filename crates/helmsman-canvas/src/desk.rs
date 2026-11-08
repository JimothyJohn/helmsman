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
//! name; the Agent's conversation (its turns, the newest first within
//! [`CONVERSATION_BYTES`], and the last instruction's outcome with its
//! proposal links) and the half-filled New device form (card
//! `desk-agent-and-forms`). Never kept: a password or PIN, or a device
//! setting that names a credential ([`is_secret_key`]).
//!
//! One `localStorage` slot (each box serves its own page, so the slot is
//! per box), written at most every [`EVERY_S`] and only when something
//! changed; at most [`MAX_BYTES`], the Agent's conversation and the device
//! form going first past it, then the biggest drafts.
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

use crate::box_agent::{BoxAgent, End, Item, Turn};
use crate::box_projects::{Answer, PCall, PReq};
use crate::devices::NewDevice;
use crate::instruct::{Instruct, Kind, Outcome};
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
    /// `editor` or `monitor`.
    pub screen: Option<String>,
    /// The sign-in form's name (never a password or PIN).
    pub user: String,
    /// Whose work this is when it was kept (as the box names owners), so a
    /// page reloaded after one person signed out does not hand it to the
    /// next. Kept in this browser only: the box drops it.
    pub owner: Option<String>,
    /// The Agent tab's conversation and the last instruction's outcome
    /// ([`Conversation::of`], capped).
    pub conversation: Option<Conversation>,
    /// The New device form, half filled ([`form_of`]: no credential, the
    /// EDS by its file name only).
    pub device: Option<NewDevice>,
}

/// The most the Agent's conversation takes of a desk, bytes: past it the
/// oldest turns go first. Well under [`BOX_BYTES`], so the edits keep room.
pub const CONVERSATION_BYTES: usize = 128 * 1024;
/// Of each turn kept: the answer's text and the message, bytes; the
/// tool, proposal and notice rows (proposals kept first).
pub const TURN_TEXT: usize = 16 * 1024;
pub const TURN_USER: usize = 2 * 1024;
pub const TURN_ITEMS: usize = 40;
/// Of each row kept, its summary, bytes.
pub const ITEM_LINE: usize = 240;
/// Undo is kept with the outcome while the file it puts back is at most
/// this, bytes (History has the revision either way).
pub const UNDO_BYTES: usize = 64 * 1024;
/// The device form: its rows, and the characters of each field kept.
pub const FORM_POINTS: usize = 256;
pub const FORM_FIELD: usize = 256;

/// The Agent tab as the desk keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    /// The turns with the box's agent, oldest first.
    pub turns: Vec<Turn>,
    /// How the last instruction came out.
    pub outcome: Option<Outcome>,
    /// An instruction was under way (what it changes, in words): after a
    /// reload it is over, with nothing written.
    pub interrupted: Option<String>,
}

/// At most `n` bytes of `s`, cut on a character, `more` added when cut.
fn clip_bytes(s: &str, n: usize, more: &str) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut k = n;
    while !s.is_char_boundary(k) {
        k -= 1;
    }
    format!("{}{more}", &s[..k])
}

/// `turn` as the desk keeps it: its text and message cut, its rows at most
/// [`TURN_ITEMS`] with every proposal among them.
fn kept_turn(turn: &Turn) -> Turn {
    let offers = |i: &Item| matches!(i, Item::Proposal { .. } | Item::Fallback { .. });
    let n_offers = turn.items.iter().filter(|i| offers(i)).count().min(TURN_ITEMS);
    // Room left for the other rows, and for the proposals.
    let mut room = (TURN_ITEMS - n_offers, n_offers);
    let mut t = Turn::new(clip_bytes(&turn.user, TURN_USER, " […]"));
    t.text = clip_bytes(
        &turn.text,
        TURN_TEXT,
        "\n[the rest of this answer is not kept over a reload]",
    );
    let line = |s: &str| clip_bytes(s, ITEM_LINE, "…");
    for i in &turn.items {
        let left = if offers(i) { &mut room.1 } else { &mut room.0 };
        if *left == 0 {
            continue;
        }
        *left -= 1;
        t.items.push(match i {
            Item::Tool { id, name, result } => Item::Tool {
                id: line(id),
                name: line(name),
                result: result.as_ref().map(|(ok, s)| (*ok, line(s))),
            },
            Item::Proposal { ids, summary } => Item::Proposal {
                ids: ids.iter().copied().take(TURN_ITEMS).collect(),
                summary: line(summary),
            },
            Item::Retrying(s) => Item::Retrying(line(s)),
            Item::Fallback { .. } => i.clone(),
        });
    }
    t.end = Some(
        turn.end
            .clone()
            .unwrap_or_else(|| End::Failed("the page was reloaded before this answer ended".into())),
    );
    t
}

impl Conversation {
    /// What the desk keeps of the Agent tab: `turns`, the newest first
    /// until [`CONVERSATION_BYTES`] (each cut by [`kept_turn`]), the
    /// `outcome` (its Undo only within [`UNDO_BYTES`]), and the
    /// instruction `under_way`, if one is. `None`: nothing to keep.
    #[must_use]
    pub fn of(turns: &[Turn], outcome: Option<&Outcome>, under_way: Option<String>) -> Option<Self> {
        if turns.is_empty() && outcome.is_none() && under_way.is_none() {
            return None;
        }
        let outcome = outcome.map(|o| {
            let mut o = o.clone();
            o.undo = o.undo.filter(|u| u.text.len() <= UNDO_BYTES);
            o.problems.truncate(20);
            for p in &mut o.problems {
                *p = clip_bytes(p, 1024, "…");
            }
            o.status = clip_bytes(&o.status, 1024, "…");
            o.why = o.why.map(|w| clip_bytes(&w, 1024, "…"));
            o.review.truncate(TURN_ITEMS);
            o
        });
        let under_way = under_way.map(|w| clip_bytes(&w, FORM_FIELD, "…"));
        let mut room = CONVERSATION_BYTES
            .saturating_sub(size(&outcome))
            .saturating_sub(size(&under_way));
        let mut kept = Vec::new();
        for t in turns.iter().rev().take(crate::box_agent::MAX_TURNS) {
            let t = kept_turn(t);
            let n = size(&t) + 1;
            if n > room {
                break;
            }
            room -= n;
            kept.push(t);
        }
        kept.reverse();
        Some(Self {
            turns: kept,
            outcome,
            interrupted: under_way,
        })
    }

    /// Puts the conversation back on the Agent tab: its turns, and its
    /// outcome; an instruction that was under way ends, nothing written,
    /// with the proposals its last turn stored still offered.
    pub fn put_back(self, agent: &mut BoxAgent, instruct: &mut Instruct) {
        agent.turns = self.turns;
        instruct.run = None;
        instruct.outcome = self.outcome;
        if let Some(target) = self.interrupted {
            let mut o = Outcome::new(
                Kind::Failed,
                format!("The page was reloaded while the Agent worked on {target}: nothing was written."),
            );
            o.review = agent.turns.last().map(Turn::proposals).unwrap_or_default();
            instruct.outcome = Some(o);
        }
    }
}

/// `v`'s size as JSON, bytes.
fn size<T: Serialize>(v: &T) -> usize {
    serde_json::to_string(v).map_or(usize::MAX / 4, |t| t.len())
}

/// Whether a device setting's key names a credential (a password, PIN,
/// token, key or secret): the desk never keeps its value.
#[must_use]
pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "pass",
        "pwd",
        "secret",
        "token",
        "auth",
        "credential",
        "apikey",
        "api_key",
    ]
    .iter()
    .any(|s| k.contains(s))
        || k == "pin"
        || k == "key"
        || k.ends_with("_pin")
        || k.ends_with("_key")
}

/// The device form as the desk keeps it: every field cut to
/// [`FORM_FIELD`] characters, at most [`FORM_POINTS`] rows, any credential
/// setting ([`is_secret_key`]) taken out of its settings, the refusal
/// left out. The EDS is its file name in the project, never its bytes.
#[must_use]
pub fn form_of(f: &NewDevice) -> NewDevice {
    let cut = |s: &str| s.chars().take(FORM_FIELD).collect::<String>();
    let params: Vec<&str> = f
        .params
        .split_whitespace()
        .filter(|t| !t.split_once('=').is_some_and(|(k, _)| is_secret_key(k)))
        .collect();
    NewDevice {
        name: cut(&f.name),
        protocol: f.protocol,
        endpoint: cut(&f.endpoint),
        params: cut(&params.join(" ")),
        error: None,
        eds: f.eds.as_deref().map(cut),
        points: f
            .points
            .iter()
            .take(FORM_POINTS)
            .map(|p| {
                let mut p = p.clone();
                p.name = cut(&p.name);
                p.point = cut(&p.point);
                if let crate::device_form::Use::Existing(v) = &mut p.use_ {
                    *v = cut(v);
                }
                p
            })
            .collect(),
        catalog: f.catalog,
        editing: f.editing.as_deref().map(cut),
    }
}

impl Desk {
    /// Reads a desk; anything that does not parse is an empty desk, and a
    /// field that does not read (a newer page's variant) is empty without
    /// taking the others down.
    #[cfg(test)]
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self::read(text).0
    }

    /// [`Self::parse`], and whether part of it did not read (`text` was
    /// not a desk at all, or a field was the wrong shape). Empty text is
    /// no desk yet, not a failure.
    #[must_use]
    pub fn read(text: &str) -> (Self, bool) {
        if text.trim().is_empty() {
            return (Self::default(), false);
        }
        if let Ok(d) = serde_json::from_str(text) {
            return (d, false);
        }
        let Ok(Json::Object(fields)) = serde_json::from_str::<Json>(text) else {
            return (Self::default(), true);
        };
        Self::from_fields(&fields)
    }

    /// The fields of `j` that read, each on its own.
    fn from_fields(fields: &serde_json::Map<String, Json>) -> (Self, bool) {
        fn field<T: serde::de::DeserializeOwned + Default>(
            fields: &serde_json::Map<String, Json>,
            name: &str,
            lost: &mut bool,
        ) -> T {
            fields.get(name).map_or_else(T::default, |v| {
                serde_json::from_value(v.clone()).unwrap_or_else(|_| {
                    *lost = true;
                    T::default()
                })
            })
        }
        let mut lost = false;
        let l = &mut lost;
        let d = Self {
            saved_at: field(fields, "saved_at", l),
            project: field(fields, "project", l),
            file: field(fields, "file", l),
            drafts: field(fields, "drafts", l),
            agent: field(fields, "agent", l),
            screen: field(fields, "screen", l),
            user: field(fields, "user", l),
            owner: field(fields, "owner", l),
            conversation: field(fields, "conversation", l),
            device: field(fields, "device", l),
        };
        (d, lost)
    }

    /// The person's work in it, which `saved_at` dates: the edits, the two
    /// inputs, the Agent's conversation and the device form (the project,
    /// file, screen and name follow the work).
    fn work(&self) -> String {
        serde_json::to_string(&(&self.drafts, &self.agent, &self.conversation, &self.device))
            .unwrap_or_default()
    }

    /// As text, at most `max` bytes. Past it the Agent's conversation goes
    /// first and the New device form next (both can be had again), and only
    /// then whole drafts, largest first; the inputs are always kept. The
    /// second value names what was dropped.
    #[must_use]
    pub fn text_within(&self, max: usize) -> (String, Vec<String>) {
        let mut d = self.clone();
        let mut dropped = Vec::new();
        loop {
            let t = serde_json::to_string(&d).unwrap_or_default();
            if t.len() <= max {
                return (t, dropped);
            }
            if d.conversation.take().is_some() {
                dropped.push("the Agent conversation".into());
                continue;
            }
            if d.device.take().is_some() {
                dropped.push("the New device form".into());
                continue;
            }
            let biggest = d
                .drafts
                .iter()
                .flat_map(|(p, m)| m.iter().map(move |(f, dr)| (dr.text.len(), p.clone(), f.clone())))
                .max();
            let Some((_, p, f)) = biggest else {
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
    /// The work of the desk this page last loaded or took from the box.
    taken: String,
    saved_at: u64,
    /// The slot held something that did not read in full: it is left as it
    /// is until the person changes something, so a newer page's fields are
    /// not overwritten by this page's emptier desk.
    unread: bool,
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
        let (desk, unread) = Desk::read(&last);
        (
            Self {
                store,
                last,
                at: None,
                work: desk.work(),
                taken: desk.work(),
                saved_at: desk.saved_at,
                unread,
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

    /// Whether the work in `desk` differs from the desk this page loaded
    /// or last took from the box: the person changed something here since.
    #[must_use]
    pub fn changed(&self, desk: &Desk) -> bool {
        desk.work() != self.taken
    }

    /// Takes `desk` (the box's, newer) as the one in front of the person:
    /// its `saved_at` stands until the work changes again.
    pub fn adopt(&mut self, desk: &Desk) {
        self.work = desk.work();
        self.taken = self.work.clone();
        self.saved_at = desk.saved_at;
        self.unread = false;
    }

    /// Whether [`Self::keep`] would look at a desk at `now` (the throttle
    /// lets it through): the desk is only built then.
    #[must_use]
    pub fn due(&self, now: f64) -> bool {
        !self.at.is_some_and(|a| now - a < EVERY_S)
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
            self.unread = false;
            // Never back in time, even on a clock behind the box's desk.
            self.saved_at = wall_ms().max(self.saved_at + 1);
        }
        desk.saved_at = self.saved_at;
        let (text, dropped) = desk.text_within(MAX_BYTES);
        // Looked at: the next look is a second away, written or not.
        self.at = Some(now);
        if text == self.last || self.unread {
            return Some(desk);
        }
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
pub(crate) fn owner(s: Option<&Session>) -> Option<String> {
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
        // Whose it was in this browser is the box's own business (the
        // session): not sent.
        let (text, dropped) = Desk {
            owner: None,
            ..desk.clone()
        }
        .text_within(BOX_BYTES);
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
                // Another person's desk (asked before the sign-in changed)
                // is not this one's.
                if j["owner"]
                    .as_str()
                    .is_some_and(|o| Some(o) != self.owner.as_deref())
                {
                    return None;
                }
                self.sent.clear();
                let d = match &j["desk"] {
                    Json::Null if j.is_object() => Desk::default(),
                    Json::Object(fields) => Desk::from_fields(fields).0,
                    _ => {
                        // Not a desk this page can read, and the box
                        // refuses a PUT older than it: the browser keeps
                        // the desk, and the box is left alone.
                        self.ready = false;
                        self.problem = Some(
                            "the desk on the box is not readable by this page, so it is kept in this browser only"
                                .into(),
                        );
                        return None;
                    }
                };
                self.ready = true;
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
            Desk {
                project: Some("line".into()),
                ..Desk::default()
            },
            "a wrong type in one field takes only that field"
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

    /// A page that wrote a newer shape of the conversation (a variant this
    /// page does not know) must not take the edits down with it
    /// (2026-11-08 review): the drafts, project and file still read, and
    /// the parse says it was partial.
    #[test]
    fn one_unreadable_field_does_not_take_the_drafts_down() {
        let text = r#"{"saved_at":9,"project":"line","file":"a.st",
            "drafts":{"line":{"a.st":{"text":"mine","version":"v1"}}},
            "conversation":{"turns":[{"user":"hi","items":[{"FromTheFuture":1}]}]},
            "device":"not a form"}"#;
        let d = Desk::parse(text);
        assert_eq!(d.project.as_deref(), Some("line"));
        assert_eq!(d.file.as_deref(), Some("a.st"));
        assert_eq!(d.drafts["line"]["a.st"].text, "mine");
        assert_eq!(d.saved_at, 9);
        assert_eq!((d.conversation, d.device), (None, None));
        assert!(Desk::read(text).1, "said to be partly unreadable");
        assert!(!Desk::read(r#"{"project":"line"}"#).1);
        assert!(!Desk::read("").1, "no desk yet is not a failure");
    }

    /// The slot that failed to parse stays as it is until the person edits.
    #[test]
    fn a_slot_that_did_not_parse_is_not_overwritten_until_an_edit() {
        let text = r#"{"saved_at":9,"drafts":{"line":{"a.st":{"text":"mine","version":"v1"}}},
            "conversation":{"turns":7}}"#;
        let store = Cell::default();
        *store.0.borrow_mut() = Some(text.to_string());
        let (mut k, mut d) = Keeper::load(Box::new(store.clone()));
        k.keep(&d, 1.0);
        k.keep(&d, 3.0);
        assert_eq!(store.0.borrow().as_deref(), Some(text), "untouched");
        d.agent = "typing".into();
        k.keep(&d, 5.0);
        let now = Desk::parse(store.0.borrow().as_deref().unwrap());
        assert_eq!(now.agent, "typing");
        assert_eq!(
            now.drafts["line"]["a.st"].text, "mine",
            "the drafts survive the first write"
        );
    }

    /// A box desk newer than this page's that it cannot read: not fought
    /// over with a PUT the box refuses, a GET, a PUT ... each second. The
    /// browser keeps the desk, and the person is told.
    #[test]
    fn a_box_desk_this_page_cannot_read_ends_the_put_get_loop() {
        let mut b = OnBox::default();
        let mine = Desk {
            saved_at: 10,
            agent: "here".into(),
            ..Desk::default()
        };
        b.step(Some(&session(Some("alice"), "engineer")));
        b.poll();
        let bad = r#"{"owner":"user:alice","desk":7}"#;
        assert_eq!(b.answer(&PCall::Desk, &answer(200, bad), 10), None);
        assert!(
            b.problem.as_deref().unwrap().contains("not readable"),
            "{:?}",
            b.problem
        );
        for t in 1..6 {
            b.keep(&mine, f64::from(t) * 2.0);
        }
        assert!(b.poll().is_empty(), "no PUT to be refused");
        // Parts that do read are taken: the drafts of a newer page.
        let mut b = OnBox::default();
        b.step(Some(&session(Some("alice"), "engineer")));
        b.poll();
        let part = r#"{"owner":"user:alice","desk":{"saved_at":50,"project":"line",
            "drafts":{"line":{"a.st":{"text":"theirs","version":"v1"}}},"conversation":{"turns":7}}}"#;
        let took = b.answer(&PCall::Desk, &answer(200, part), 10).unwrap();
        assert_eq!(took.drafts["line"]["a.st"].text, "theirs");
        assert_eq!(took.conversation, None);
    }

    /// An answer for the person who was signed in before is not the new
    /// person's desk.
    #[test]
    fn an_answer_for_another_owner_is_dropped() {
        let mut b = OnBox::default();
        b.step(Some(&session(Some("alice"), "engineer")));
        b.poll();
        b.step(Some(&session(Some("bob"), "operator")));
        assert_eq!(b.poll().len(), 1, "bob's desk is asked for");
        let alices = r#"{"owner":"user:alice","desk":{"saved_at":99,"agent":"alice's"}}"#;
        assert_eq!(b.answer(&PCall::Desk, &answer(200, alices), 0), None);
        b.keep(&Desk::default(), 5.0);
        assert!(b.poll().is_empty(), "still waiting for bob's own");
        let bobs = r#"{"owner":"user:bob","desk":{"saved_at":3,"agent":"bob's"}}"#;
        assert_eq!(
            b.answer(&PCall::Desk, &answer(200, bobs), 0).unwrap().agent,
            "bob's"
        );
    }

    /// Past the byte cap the Agent's conversation and the device form go
    /// before any edit: the biggest edit is often the file being worked on.
    #[test]
    fn past_the_cap_the_conversation_goes_before_the_edits() {
        let mut d = Desk {
            conversation: Conversation::of(
                &[turn("hello", &"w".repeat(900), vec![], Some(done()))],
                None,
                None,
            ),
            device: Some(form_of(&form())),
            ..Desk::default()
        };
        d.drafts
            .entry("p".into())
            .or_default()
            .insert("main.st".into(), draft(&"x".repeat(600)));
        let (t, dropped) = d.text_within(900);
        assert_eq!(dropped, ["the Agent conversation", "the New device form"]);
        assert_eq!(Desk::parse(&t).drafts["p"]["main.st"].text.len(), 600);
    }

    /// An unchanged desk is looked at once a second, not every frame.
    #[test]
    fn an_unchanged_desk_is_checked_once_a_second() {
        let (mut k, mut d) = Keeper::load(Box::new(MemStorage(None)));
        d.agent = "a".into();
        k.keep(&d, 1.0);
        assert!(k.keep(&d, 3.0).is_some());
        assert!(!k.due(3.5), "just checked");
        assert!(k.due(4.1));
    }

    /// A storage two handles share, to read what the keeper wrote.
    #[derive(Clone, Default)]
    struct Cell(std::rc::Rc<std::cell::RefCell<Option<String>>>);

    impl Storage for Cell {
        fn load(&self) -> Result<Option<String>, String> {
            Ok(self.0.borrow().clone())
        }

        fn save(&mut self, text: &str) -> Result<(), String> {
            *self.0.borrow_mut() = Some(text.to_string());
            Ok(())
        }
    }

    fn turn(user: &str, text: &str, items: Vec<Item>, end: Option<End>) -> Turn {
        let mut t = Turn::new(user.into());
        t.text = text.into();
        t.items = items;
        t.end = end;
        t
    }

    fn done() -> End {
        End::Done {
            stop: "end_turn".into(),
            why: None,
        }
    }

    fn form() -> NewDevice {
        let mut f = NewDevice::from_eds("e1213.eds", "[Device]\nProdName = \"E1213\";\n");
        f.endpoint = "192.168.1.60".into();
        f.points[0].on = true;
        f.points[0].name = "start_pb".into();
        f.points.push(crate::device_form::PointRow {
            use_: crate::device_form::Use::Existing("motor".into()),
            ..crate::device_form::PointRow::custom(crate::device_form::Dir::Out, 1)
        });
        f
    }

    /// The box keeps 512 KiB of a desk: the longest conversation the page
    /// can hold (every turn the most text and rows it takes, a big Undo)
    /// keeps its newest turns and their proposals within
    /// [`CONVERSATION_BYTES`], so with the device form and a large edit
    /// the whole desk still goes to the box with nothing left out.
    #[test]
    fn a_long_conversation_still_fits_on_the_box_with_its_newest_turns() {
        use crate::box_agent::{MAX_ITEMS, MAX_TEXT};
        let tool = |n: usize| Item::Tool {
            id: format!("t{n}"),
            name: "read_file".into(),
            result: Some((true, "r".repeat(600))),
        };
        let turns: Vec<Turn> = (0..60u64)
            .map(|k| {
                let mut items: Vec<Item> = (0..MAX_ITEMS - 1).map(tool).collect();
                items.push(Item::Proposal {
                    ids: vec![k],
                    summary: "speed to 2".into(),
                });
                // Two bytes a character: cut on a character, not in one.
                let text = "é".repeat(MAX_TEXT / 2);
                turn(&format!("{}{k}", "m".repeat(9000)), &text, items, Some(done()))
            })
            .collect();
        let mut outcome = Outcome::new(Kind::Applied, "Main.st is updated.");
        outcome.undo = Some(crate::instruct::Undo {
            project: "line".into(),
            file: "main.st".into(),
            revision: 3,
            text: "x".repeat(UNDO_BYTES + 1),
        });
        outcome.review = vec![59];
        let c = Conversation::of(&turns, Some(&outcome), None).unwrap();
        assert!(size(&c) <= CONVERSATION_BYTES, "{}", size(&c));
        assert!(c.turns.len() >= 2, "{}", c.turns.len());
        let last = c.turns.last().unwrap();
        assert_eq!(last.proposals(), [59], "the newest turn, its proposal kept");
        assert!(c.turns.iter().all(|t| t.items.len() <= TURN_ITEMS));
        assert!(c.turns.iter().all(|t| t.text.len() <= TURN_TEXT + 64));
        assert!(last.text.ends_with("is not kept over a reload]"));
        assert_eq!(c.outcome.as_ref().unwrap().undo, None, "too big an Undo");
        let mut small = outcome.clone();
        small.undo.as_mut().unwrap().text = "x".into();
        let kept = Conversation::of(&turns[..1], Some(&small), None).unwrap();
        assert!(kept.outcome.unwrap().undo.is_some(), "a small Undo is kept");

        let mut d = Desk {
            conversation: Some(c.clone()),
            device: Some(form_of(&form())),
            ..Desk::default()
        };
        d.drafts
            .entry("line".into())
            .or_default()
            .insert("main.st".into(), draft(&"y".repeat(300 * 1024)));
        let (t, dropped) = d.text_within(BOX_BYTES);
        assert_eq!(dropped, Vec::<String>::new());
        assert!(t.len() <= BOX_BYTES);
        let back = Desk::parse(&t);
        assert_eq!(back.conversation, Some(c));
        assert_eq!(back.device, d.device);
        // Past the limit with no edits left, the conversation goes, then
        // the form, and the inputs stay.
        let (t, dropped) = Desk {
            drafts: BTreeMap::new(),
            agent: "half typed".into(),
            ..d
        }
        .text_within(1000);
        assert_eq!(dropped, ["the Agent conversation", "the New device form"]);
        assert_eq!(Desk::parse(&t).agent, "half typed");
    }

    /// The conversation and the device form are work: a change to either
    /// dates the desk, like an edit.
    #[test]
    fn a_new_turn_or_a_form_change_moves_saved_at() {
        let (mut k, mut d) = Keeper::load(Box::new(MemStorage(None)));
        d.agent = "a".into();
        let t0 = k.keep(&d, 1.0).unwrap().saved_at;
        assert!(!k.due(1.5) && k.due(2.1), "the throttle");
        assert_eq!(k.keep(&d, 3.0).unwrap().saved_at, t0, "nothing changed");
        d.conversation = Conversation::of(
            &[turn("speed up", "", vec![], None)],
            None,
            Some("main.st".into()),
        );
        let t1 = k.keep(&d, 5.0).unwrap().saved_at;
        assert!(t1 > t0);
        d.conversation = Conversation::of(
            &[turn("speed up", "Why: faster", vec![], Some(done()))],
            None,
            None,
        );
        let t2 = k.keep(&d, 7.0).unwrap().saved_at;
        assert!(t2 > t1, "the answer streamed in");
        let mut f = form();
        d.device = Some(form_of(&f));
        let t3 = k.keep(&d, 9.0).unwrap().saved_at;
        assert!(t3 > t2);
        f.name = "rio".into();
        d.device = Some(form_of(&f));
        assert!(k.keep(&d, 11.0).unwrap().saved_at > t3, "a field typed");
    }

    /// The form has no credential field, and any setting whose key names
    /// one is left out of the desk, value and all; the EDS is kept by its
    /// file name, and a refusal is not kept.
    #[test]
    fn the_device_form_keeps_no_credential() {
        let mut f = form();
        f.params = "eds=e1213.eds password=hunter2 Token=tok-77 api_key=k-55 pin=1234 \
                    auth_user=au-1 secret=s-9 key=kk-3 rpi_ms=100 poll_ms=20"
            .into();
        f.error = Some("the name is taken".into());
        let kept = form_of(&f);
        assert_eq!(kept.params, "eds=e1213.eds rpi_ms=100 poll_ms=20");
        assert_eq!(kept.error, None);
        assert_eq!(kept.eds.as_deref(), Some("e1213.eds"));
        let d = Desk {
            device: Some(kept.clone()),
            ..Desk::default()
        };
        let text = d.text_within(BOX_BYTES).0;
        for secret in [
            "hunter2", "tok-77", "k-55", "1234", "au-1", "s-9", "kk-3", "taken",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert_eq!(Desk::parse(&text).device, Some(kept));
        for (k, secret) in [
            ("password", true),
            ("PIN", true),
            ("ssh_key", true),
            ("bearer_token", true),
            ("unit", false),
            ("poll_ms", false),
            ("profile", false),
            ("eds", false),
            ("timeout_ms", false),
        ] {
            assert_eq!(is_secret_key(k), secret, "{k}");
        }
        // A form far past its bounds is cut to them.
        f.name = "n".repeat(10_000);
        f.points = (0..1000)
            .map(|n| crate::device_form::PointRow::custom(crate::device_form::Dir::In, n))
            .collect();
        let kept = form_of(&f);
        assert_eq!(kept.name.len(), FORM_FIELD);
        assert_eq!(kept.points.len(), FORM_POINTS);
    }

    /// A reload while the Agent answered: the turns come back, the one
    /// that was streaming ended, the instruction over with nothing
    /// written, and the proposal its turn stored still offered.
    #[test]
    fn a_conversation_cut_by_a_reload_ends_and_keeps_its_proposal() {
        let turns = [
            turn("first", "Why: done", vec![], Some(done())),
            turn(
                "second",
                "Looking",
                vec![Item::Proposal {
                    ids: vec![7],
                    summary: "speed to 2".into(),
                }],
                None,
            ),
        ];
        let c = Conversation::of(&turns, None, Some("main.st".into())).unwrap();
        let mut agent = BoxAgent::new();
        let mut instruct = Instruct::default();
        Desk::parse(
            &Desk {
                conversation: Some(c),
                ..Desk::default()
            }
            .text_within(MAX_BYTES)
            .0,
        )
        .conversation
        .unwrap()
        .put_back(&mut agent, &mut instruct);
        assert_eq!(agent.turns.len(), 2);
        assert_eq!(agent.turns[0].text, "Why: done");
        assert!(
            matches!(&agent.turns[1].end, Some(End::Failed(w)) if w.contains("reloaded")),
            "{:?}",
            agent.turns[1].end
        );
        assert!(!agent.busy() && !instruct.busy());
        let o = instruct.outcome.unwrap();
        assert_eq!(o.kind, Kind::Failed);
        assert!(o.status.contains("main.st") && o.status.contains("nothing was written"));
        assert_eq!(o.review, [7]);
    }
}
