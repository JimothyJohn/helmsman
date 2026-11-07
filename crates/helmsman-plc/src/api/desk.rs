//! The desk on the box (card `desk-on-box`): what a person had in front of
//! them on the box's page, kept by the box per person so it follows them
//! from one screen to another (Nick, 2026-11-07: "kept on the box per
//! person when signed in, browser as the fallback").
//!
//! ```text
//! GET /api/desk    a login session: that person's desk, or null
//! PUT /api/desk    a login session: the desk, at most MAX_DESK bytes
//! ```
//!
//! - **Whose.** The session's owner, as the trend pins have it
//!   ([`super::trend`]): an account (`user:NAME`), or for a role-PIN
//!   session the role (`role:NAME`). Neither route takes an owner from the
//!   request: there is no way to name another person's desk.
//! - **What is kept.** Only the fields the page's desk has (the open
//!   project and file, the unsaved edits with the version each was made
//!   against, the Agent's and the Requirements' inputs, the screen, the
//!   sign-in name, and `saved_at`, when its work last changed): the body
//!   is read into that shape and written back from it, so any other field
//!   (a password, a PIN) is dropped, never stored.
//! - **Newer wins.** A desk whose `saved_at` is older than the one held is
//!   refused (409, with the held one's `saved_at`): the page reads the
//!   newer one instead of writing over it.
//! - **Bounds.** A body over [`MAX_DESK`] bytes is refused (413); at most
//!   [`MAX_OWNERS`] people's desks (a new person past that is refused, 507,
//!   rather than dropping someone's unsaved work). Kept across restarts in
//!   the state directory's `desks/` folder, one file per person (named by
//!   the owner in hex, written whole and atomically, read under the same
//!   bound); a file that is not a desk reads as none.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use super::{Ctx, Request, Response, audited, is_json};

/// The most one desk holds, bytes (the request body's limit is larger).
pub const MAX_DESK: usize = 512 * 1024;
/// People whose desks the box keeps.
pub const MAX_OWNERS: usize = 64;

/// An unsaved edit: the text, and the box's version it was made against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub version: Option<String>,
}

/// A desk, as the page keeps it. Every other field is dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Desk {
    /// When its work (edits, inputs) last changed, Unix ms (the page's
    /// clock).
    pub saved_at: u64,
    pub project: Option<String>,
    pub file: Option<String>,
    /// Project, then file, then the edit.
    pub drafts: BTreeMap<String, BTreeMap<String, Draft>>,
    pub agent: String,
    pub requirement: String,
    pub screen: Option<String>,
    pub user: String,
}

impl Desk {
    /// Reads a desk from `bytes`, at most [`MAX_DESK`]; anything else is
    /// `None`.
    #[must_use]
    pub fn read(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_DESK {
            return None;
        }
        match serde_json::from_slice::<Json>(bytes).ok()? {
            j @ Json::Object(_) => serde_json::from_value(j).ok(),
            _ => None,
        }
    }

    fn text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Why a desk was not set.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The box holds a newer one (its `saved_at`).
    Older(u64),
    /// [`MAX_OWNERS`] people have desks already.
    Full,
    /// Kept in memory, not in the folder (why).
    NotKept(String),
}

/// Every person's desk: in memory, or in a folder.
#[derive(Debug, Default)]
pub struct Desks {
    /// Where they are kept; `None`: memory only.
    dir: Option<PathBuf>,
    /// In memory (always; the folder's are read into it as asked).
    held: BTreeMap<String, Desk>,
}

/// `owner`'s file name: its bytes in hex, so no owner names a path.
fn file_name(owner: &str) -> String {
    let mut s = String::with_capacity(owner.len() * 2 + 5);
    for b in owner.bytes() {
        let _ = write!(s, "{b:02x}");
    }
    s.push_str(".json");
    s
}

impl Desks {
    /// Desks kept in `dir` (the box's `desks/`), created when the first is
    /// set.
    #[must_use]
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            held: BTreeMap::new(),
        }
    }

    /// `owner`'s desk, if the box has one.
    pub fn get(&mut self, owner: &str) -> Option<Desk> {
        if let Some(d) = self.held.get(owner) {
            return Some(d.clone());
        }
        let path = self.dir.as_ref()?.join(file_name(owner));
        let bytes = crate::project::read_file(&path, MAX_DESK).ok()?;
        let d = Desk::read(&bytes)?;
        self.held.insert(owner.to_string(), d.clone());
        Some(d)
    }

    /// How many people have a desk (held or in the folder).
    fn owners(&self) -> usize {
        let on_disk = self
            .dir
            .as_ref()
            .and_then(|d| std::fs::read_dir(d).ok())
            .map_or(0, |r| {
                r.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .count()
            });
        on_disk.max(self.held.len())
    }

    /// Sets `owner`'s desk (checked by the caller), unless the box holds a
    /// newer one or is full.
    ///
    /// # Errors
    /// [`Refused`]: nothing was set, except for `NotKept` (set in memory,
    /// the folder refused it).
    pub fn set(&mut self, owner: &str, desk: Desk) -> Result<(), Refused> {
        match self.get(owner) {
            Some(held) if held.saved_at > desk.saved_at => return Err(Refused::Older(held.saved_at)),
            None if self.owners() >= MAX_OWNERS => return Err(Refused::Full),
            Some(_) | None => {}
        }
        let text = desk.text();
        self.held.insert(owner.to_string(), desk);
        let Some(dir) = &self.dir else { return Ok(()) };
        let written = (|| {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
            b.create(dir)?;
            crate::project::write_atomic(dir, &file_name(owner), text.as_bytes())
        })();
        written.map_err(|e| Refused::NotKept(format!("{}: {e}", dir.display())))
    }
}

const NOT_SIGNED_IN: &str = "the desk is a signed-in person's; signed out, the browser keeps it";

/// `GET /api/desk`.
pub(super) fn get(req: &Request, ctx: &Ctx) -> Response {
    match super::trend::owner(req, ctx) {
        Some(o) => {
            let d = ctx.desks().get(&o);
            Response::json(200, &json!({ "owner": o, "desk": d }))
        }
        None => Response::error(401, NOT_SIGNED_IN),
    }
}

/// `PUT /api/desk` (authenticated by the caller).
pub(super) fn put(req: &Request, ctx: &Ctx) -> Response {
    let Some(who) = req.session.as_ref() else {
        return audited(
            ctx,
            req,
            400,
            &json!({ "error": "a desk is a person's: sign in (the box token is no one)" }),
            "desk: not a login session",
        );
    };
    let owner = super::trend::owner_of(who);
    if req.body.len() > MAX_DESK {
        let detail = format!("desk of {owner}: {} bytes refused", req.body.len());
        return audited(
            ctx,
            req,
            413,
            &json!({ "error": format!("a desk is at most {MAX_DESK} bytes") }),
            &super::clean(&detail),
        );
    }
    let Some(desk) = is_json(req).then(|| Desk::read(&req.body)).flatten() else {
        return audited(
            ctx,
            req,
            400,
            &json!({ "error": "expected application/json: a desk object" }),
            "desk: bad body",
        );
    };
    let saved_at = desk.saved_at;
    let result = ctx.desks().set(&owner, desk);
    let detail = format!("desk of {owner}: {} bytes, saved_at {saved_at}", req.body.len());
    match result {
        Ok(()) => audited(
            ctx,
            req,
            200,
            &json!({ "owner": owner, "saved_at": saved_at, "kept": true }),
            &super::clean(&detail),
        ),
        Err(Refused::NotKept(e)) => audited(
            ctx,
            req,
            200,
            &json!({ "owner": owner, "saved_at": saved_at, "kept": false }),
            &super::clean(&format!("{detail}; not kept across a restart: {e}")),
        ),
        Err(Refused::Older(held)) => audited(
            ctx,
            req,
            409,
            &json!({ "error": "the box holds a newer desk", "saved_at": held }),
            &super::clean(&format!("{detail}: older than the box's ({held})")),
        ),
        Err(Refused::Full) => audited(
            ctx,
            req,
            507,
            &json!({ "error": format!("the box keeps desks for at most {MAX_OWNERS} people") }),
            &super::clean(&format!("{detail}: the box is full")),
        ),
    }
}
