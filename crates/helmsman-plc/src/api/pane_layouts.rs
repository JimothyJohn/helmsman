//! Pane layouts kept by the box (Nick, 2026-11-15: "a box also keeps
//! layouts, so any browser that opens it offers them"): named
//! arrangements of the editor's side tabs and panes (helmsman-canvas
//! `pane_layout`), one list for the box, not one per person.
//!
//! ```text
//! GET    /api/pane-layouts          the box's layouts
//! PUT    /api/pane-layouts          engineer session: one layout, by its name
//! DELETE /api/pane-layouts/{name}   engineer session: drop one
//! ```
//!
//! - **The shape.** `GET` answers `{"format": "helmsman-pane-layouts 1",
//!   "layouts": [...]}`, the file the page's Export writes, so the page
//!   reads both with one reader. A layout is its `name`, the tabs of the
//!   `left` and `right` panes and the `hidden` ones in order, which tab
//!   each side shows, which panes are open, the bottom panel's tab, and
//!   the three sizes in points.
//! - **What is kept.** Only those fields: the body is read into that
//!   shape and written back from it. A layout that breaks a rule is
//!   refused whole (400), never mended: a name of 1 to [`MAX_NAME`]
//!   characters with no control character and no space at either end,
//!   tabs from [`TABS`] each at most once, the shown tabs among their
//!   pane's, sizes within [`MIN_SIZE`] to [`MAX_SIZE`].
//! - **Bounds.** A body over [`MAX_BODY`] bytes is refused (413); at most
//!   [`MAX_LAYOUTS`] layouts (a new name past that is refused, 507; a name
//!   the box has is replaced, matched without regard to case). Kept
//!   across restarts in the state directory's `pane-layouts.json`,
//!   written whole and atomically; a file that does not read is none.
//! - **Who.** Reading is anyone's who can read the box; a change needs an
//!   engineer's login session, and every change or refusal is audited.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use super::{Ctx, Request, Response, audited, is_json};

/// The first field of the list, as the page's export file has it.
pub const FORMAT: &str = "helmsman-pane-layouts 1";
/// Layouts the box keeps.
pub const MAX_LAYOUTS: usize = 32;
/// Longest name, characters.
pub const MAX_NAME: usize = 40;
/// The most one layout's body holds, bytes.
pub const MAX_BODY: usize = 4096;
/// The sizes a layout may ask a pane for, points.
pub const MIN_SIZE: f64 = 120.0;
pub const MAX_SIZE: f64 = 1600.0;
/// The side tabs, by the names the page keeps them under.
pub const TABS: [&str; 6] = ["claude", "code", "io", "devices", "files", "history"];
/// The bottom panel's tabs.
pub const BOTTOMS: [&str; 2] = ["trend", "messages"];

/// One layout, as the page writes it. Every other field is dropped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub name: String,
    pub left: Vec<String>,
    pub right: Vec<String>,
    pub hidden: Vec<String>,
    pub left_shows: String,
    pub right_shows: String,
    pub left_open: bool,
    pub right_open: bool,
    pub bottom_open: bool,
    pub bottom_shows: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom_height: Option<f64>,
}

/// Whether `name` is one the box keeps a layout under.
#[must_use]
pub fn name_ok(name: &str) -> bool {
    let n = name.chars().count();
    (1..=MAX_NAME).contains(&n) && name.trim() == name && !name.chars().any(char::is_control)
}

impl Layout {
    /// Reads a layout from `bytes`, at most [`MAX_BODY`].
    ///
    /// # Errors
    /// Why it is not one, in words for the caller.
    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BODY {
            return Err(format!("a pane layout is at most {MAX_BODY} bytes"));
        }
        let l: Self = match serde_json::from_slice::<Json>(bytes) {
            Ok(j @ Json::Object(_)) => {
                serde_json::from_value(j).map_err(|_| "a field of the layout has the wrong type")?
            }
            _ => return Err("expected a pane layout object".into()),
        };
        l.check()?;
        Ok(l)
    }

    fn check(&self) -> Result<(), String> {
        if !name_ok(&self.name) {
            return Err(format!(
                "name: 1 to {MAX_NAME} characters, no control characters, no space at either end"
            ));
        }
        let mut seen: Vec<&str> = Vec::new();
        for tab in self.left.iter().chain(&self.right).chain(&self.hidden) {
            if !TABS.contains(&tab.as_str()) {
                return Err("an unknown tab (the tabs are claude, code, io, devices, files, history)".into());
            }
            if seen.contains(&tab.as_str()) {
                return Err(format!("the tab {tab} is placed twice"));
            }
            seen.push(tab);
        }
        for (shown, pane, what) in [
            (&self.left_shows, &self.left, "left_shows"),
            (&self.right_shows, &self.right, "right_shows"),
        ] {
            if !pane.is_empty() && !pane.contains(shown) {
                return Err(format!("{what} is not a tab of that pane"));
            }
            if pane.is_empty() && !shown.is_empty() && !TABS.contains(&shown.as_str()) {
                return Err(format!("{what} is not a tab"));
            }
        }
        if !BOTTOMS.contains(&self.bottom_shows.as_str()) {
            return Err("bottom_shows is trend or messages".into());
        }
        for (size, what) in [
            (self.left_width, "left_width"),
            (self.right_width, "right_width"),
            (self.bottom_height, "bottom_height"),
        ] {
            if size.is_some_and(|s| !(MIN_SIZE..=MAX_SIZE).contains(&s)) {
                return Err(format!("{what} is {MIN_SIZE} to {MAX_SIZE} points"));
            }
        }
        Ok(())
    }
}

/// Why a layout was not set.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// [`MAX_LAYOUTS`] are kept already.
    Full,
    /// Kept in memory, not in the file (why).
    NotKept(String),
}

/// The box's layouts: in memory, or in a file.
#[derive(Debug, Default)]
pub struct Kept {
    /// Where they are kept; `None`: memory only.
    file: Option<PathBuf>,
    /// In memory, once read.
    held: Option<Vec<Layout>>,
}

impl Kept {
    /// Layouts kept in `file` (the box's `pane-layouts.json`), created
    /// when the first is set.
    #[must_use]
    pub fn in_file(file: PathBuf) -> Self {
        Self {
            file: Some(file),
            held: None,
        }
    }

    /// What `text` holds: the layouts that pass the rules, the first of a
    /// name, at most [`MAX_LAYOUTS`]. Anything else is none.
    fn parse(bytes: &[u8]) -> Vec<Layout> {
        let Ok(j) = serde_json::from_slice::<Json>(bytes) else {
            return Vec::new();
        };
        if j["format"].as_str() != Some(FORMAT) {
            return Vec::new();
        }
        let mut out: Vec<Layout> = Vec::new();
        for entry in j["layouts"].as_array().into_iter().flatten() {
            let Ok(l) = serde_json::from_value::<Layout>(entry.clone()) else {
                continue;
            };
            if l.check().is_ok()
                && out.len() < MAX_LAYOUTS
                && !out.iter().any(|o| o.name.eq_ignore_ascii_case(&l.name))
            {
                out.push(l);
            }
        }
        out
    }

    fn held(&mut self) -> &mut Vec<Layout> {
        let file = self.file.clone();
        self.held.get_or_insert_with(|| {
            file.and_then(|f| crate::project::read_file(&f, MAX_LAYOUTS * MAX_BODY * 2).ok())
                .map(|bytes| Self::parse(&bytes))
                .unwrap_or_default()
        })
    }

    /// Every layout, in the order they were first kept.
    pub fn list(&mut self) -> Vec<Layout> {
        self.held().clone()
    }

    /// The list as `GET` answers it and as the file holds it.
    pub fn json(&mut self) -> Json {
        json!({ "format": FORMAT, "layouts": self.list() })
    }

    fn write(&mut self) -> Result<(), Refused> {
        let text = self.json().to_string();
        let Some(file) = &self.file else { return Ok(()) };
        let written = (|| {
            let dir = file.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| std::io::Error::other("no file name"))?;
            std::fs::create_dir_all(dir)?;
            crate::project::write_atomic(dir, name, text.as_bytes())
        })();
        written.map_err(|e| Refused::NotKept(format!("{}: {e}", file.display())))
    }

    /// Keeps `layout` (checked by the caller) in place of one of its
    /// name, or as a new one.
    ///
    /// # Errors
    /// [`Refused`]: nothing was set, except for `NotKept` (set in memory,
    /// the file refused it).
    pub fn set(&mut self, layout: Layout) -> Result<(), Refused> {
        let held = self.held();
        match held
            .iter()
            .position(|l| l.name.eq_ignore_ascii_case(&layout.name))
        {
            Some(k) => held[k] = layout,
            None if held.len() >= MAX_LAYOUTS => return Err(Refused::Full),
            None => held.push(layout),
        }
        self.write()
    }

    /// Drops the layout named `name`; whether there was one.
    ///
    /// # Errors
    /// `NotKept`: dropped in memory, the file refused the write.
    pub fn remove(&mut self, name: &str) -> Result<bool, Refused> {
        let held = self.held();
        let Some(k) = held.iter().position(|l| l.name.eq_ignore_ascii_case(name)) else {
            return Ok(false);
        };
        held.remove(k);
        self.write().map(|()| true)
    }
}

/// `GET /api/pane-layouts`.
pub(super) fn get(ctx: &Ctx) -> Response {
    Response::json(200, &ctx.pane_layouts().json())
}

/// `PUT /api/pane-layouts` (authenticated by the caller).
pub(super) fn put(req: &Request, ctx: &Ctx) -> Response {
    if let Err(r) = super::projects::engineer(ctx, req, "keeping a pane layout on the box") {
        return r;
    }
    if req.body.len() > MAX_BODY {
        return audited(
            ctx,
            req,
            413,
            &json!({ "error": format!("a pane layout is at most {MAX_BODY} bytes") }),
            &format!("pane layout: {} bytes refused", req.body.len()),
        );
    }
    let read = if is_json(req) {
        Layout::read(&req.body)
    } else {
        Err("expected application/json: a pane layout object".into())
    };
    let layout = match read {
        Ok(l) => l,
        Err(why) => {
            return audited(ctx, req, 400, &json!({ "error": why }), "pane layout: bad body");
        }
    };
    let name = layout.name.clone();
    let detail = super::clean(&format!("pane layout {name:?}"));
    let result = ctx.pane_layouts().set(layout);
    match result {
        Ok(()) => audited(ctx, req, 200, &json!({ "name": name, "kept": true }), &detail),
        Err(Refused::NotKept(e)) => audited(
            ctx,
            req,
            200,
            &json!({ "name": name, "kept": false }),
            &super::clean(&format!("{detail}; not kept across a restart: {e}")),
        ),
        Err(Refused::Full) => audited(
            ctx,
            req,
            507,
            &json!({ "error": format!("the box keeps at most {MAX_LAYOUTS} pane layouts: delete one first") }),
            &format!("{detail}: the box is full"),
        ),
    }
}

/// `DELETE /api/pane-layouts/{name}` (authenticated by the caller).
pub(super) fn delete(req: &Request, ctx: &Ctx, name: &str) -> Response {
    if let Err(r) = super::projects::engineer(ctx, req, "deleting a pane layout from the box") {
        return r;
    }
    let detail = super::clean(&format!("pane layout {name:?}"));
    if !name_ok(name) {
        return audited(
            ctx,
            req,
            404,
            &json!({ "error": "no such pane layout" }),
            "pane layout: no such name",
        );
    }
    let result = ctx.pane_layouts().remove(name);
    match result {
        Ok(true) => audited(ctx, req, 200, &json!({ "name": name, "deleted": true }), &detail),
        Ok(false) => audited(
            ctx,
            req,
            404,
            &json!({ "error": "no such pane layout" }),
            &format!("{detail}: none of that name"),
        ),
        Err(Refused::NotKept(e)) => audited(
            ctx,
            req,
            200,
            &json!({ "name": name, "deleted": true, "kept": false }),
            &super::clean(&format!("{detail}: deleted; not kept across a restart: {e}")),
        ),
        Err(Refused::Full) => Response::error(500, "unreachable: a delete never fills the box"),
    }
}
