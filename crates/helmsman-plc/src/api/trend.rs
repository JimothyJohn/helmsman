//! Trends on the box (card `box-trend-source`: "box-side, so history is
//! there on open"). The box keeps a bounded history of the signals people
//! pin, so the Editor's Trend tab opens with the last minutes drawn and
//! then streams (`/api/events`).
//!
//! ```text
//! GET /api/trends?signals=a,b     open: each signal's history; keeps them for a day
//! GET /api/trends/pins            a login session: that person's pins
//! PUT /api/trends/pins            a login session: {signals: [...]}, at most 16
//! ```
//!
//! - **What is kept.** A ring per tracked signal, fed from the views the
//!   scan thread publishes (the values stream's source; the scan never
//!   waits on it): a point when the value changes (a step function, as a
//!   PLC value holds until the next scan), on the scheduler's clock. Not
//!   the recorder: it keeps only what modules read, never their outputs,
//!   and a pinned lamp is an output.
//! - **Bounds.** [`WINDOW`] of history (10 min), [`MAX_POINTS`] changes a
//!   signal (2400; older ones go first, the value at the window's start
//!   kept), [`MAX_TRACKED`] signals at once (64; one more is refused, not
//!   swapped for another's). Worst case about 64 × 2400 × 48 bytes, 7 MiB.
//! - **Who pins.** Signed in, pins are the person's (an account, or a
//!   role for a role-PIN session), kept by the box: they follow the
//!   person to any screen, and they survive a restart, in the state
//!   directory's `trend-pins` file ([`Trends::in_file`]; written whole on
//!   each change, read once at boot). History is memory only: a restart
//!   starts it over (card `trend-pins-persist`). Signed out, the browser
//!   keeps them. Either way a page asking for a signal's history keeps it
//!   tracked for [`LEASE`] (a day) after the last ask; a person's pins
//!   keep theirs as long as they are pinned, from the first scan after
//!   boot.
//! - **Hostile input.** At most [`MAX_PINS`] signals a request (400
//!   past that); names the box has no signal for are listed `unknown` and
//!   never tracked; names are matched exactly, never parsed further. The
//!   pins file is read under the same bounds ([`MAX_PINS_FILE`] bytes,
//!   [`MAX_OWNERS`] people, [`MAX_PINS`] each, names up to [`MAX_NAME`]):
//!   anything else in it is dropped, and a file that is not what this box
//!   writes starts it with no pins.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use helmsman_io::Value;
use serde_json::{Value as Json, json};

use super::live::View;
use super::{Ctx, Request, Response, audited, is_json, logins};

/// History kept per signal.
pub const WINDOW: Duration = Duration::from_secs(600);
/// Changes kept per signal.
pub const MAX_POINTS: usize = 2400;
/// Signals tracked at once.
pub const MAX_TRACKED: usize = 64;
/// Signals one request or one person pins.
pub const MAX_PINS: usize = 16;
/// People whose pins the box keeps.
pub const MAX_OWNERS: usize = 256;
/// How long a page's ask keeps a signal tracked.
pub const LEASE: Duration = Duration::from_secs(24 * 3600);
/// The most the pins file is read.
pub const MAX_PINS_FILE: usize = 64 * 1024;
/// The longest owner or signal name read back from the pins file.
pub const MAX_NAME: usize = 256;

/// One signal's history.
#[derive(Debug, Default)]
struct Track {
    /// (scheduler time, value) at each change, oldest first.
    points: VecDeque<(Duration, Option<Value>)>,
    /// Kept until then without a person pinning it.
    until: Duration,
}

/// Every tracked signal's history, and who pinned what.
#[derive(Debug, Default)]
pub struct Trends {
    tracks: BTreeMap<String, Track>,
    /// Pins by owner, least recently set first.
    pins: Vec<(String, Vec<String>)>,
    /// The last view sampled.
    seq: u64,
    /// Its time.
    now: Duration,
    /// Where the pins are kept across restarts, if anywhere.
    file: Option<PathBuf>,
    /// Pins read from the file wait for the first view to be tracked
    /// (the box's signals are known then).
    untracked: bool,
    /// Why the last write of the pins file failed, if it did.
    save_error: Option<String>,
}

/// What [`Trends::want`] made of a list of names.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    /// Tracked (already, or from now).
    pub kept: Vec<String>,
    /// No such signal.
    pub unknown: Vec<String>,
    /// Over [`MAX_TRACKED`].
    pub refused: Vec<String>,
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The pins in `file`, within the bounds, or none.
fn load_pins(file: &Path) -> Vec<(String, Vec<String>)> {
    let Ok(bytes) = crate::project::read_file(file, MAX_PINS_FILE) else {
        return Vec::new();
    };
    if bytes.len() > MAX_PINS_FILE {
        return Vec::new();
    }
    let Ok(Json::Object(owners)) = serde_json::from_slice::<Json>(&bytes) else {
        return Vec::new();
    };
    let name_ok = |n: &str| !n.is_empty() && n.len() <= MAX_NAME;
    let mut pins: Vec<(String, Vec<String>)> = Vec::new();
    for (owner, names) in owners {
        if pins.len() >= MAX_OWNERS {
            break;
        }
        let Some(names) = names.as_array() else { continue };
        if !name_ok(&owner) {
            continue;
        }
        let mut kept: Vec<String> = Vec::new();
        for n in names.iter().filter_map(Json::as_str) {
            if kept.len() >= MAX_PINS {
                break;
            }
            if name_ok(n) && !kept.iter().any(|k| k == n) {
                kept.push(n.to_string());
            }
        }
        if !kept.is_empty() {
            pins.push((owner, kept));
        }
    }
    pins
}

impl Trends {
    /// Trends whose pins are kept in `file` (the box's `trend-pins`):
    /// the ones there now, tracked from the first view sampled, and
    /// every change hereafter. History is never written: a restart
    /// starts it over.
    #[must_use]
    pub fn in_file(file: PathBuf) -> Self {
        let pins = load_pins(&file);
        Self {
            untracked: !pins.is_empty(),
            pins,
            file: Some(file),
            ..Self::default()
        }
    }

    /// Where the pins are kept, if anywhere.
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Why the pins last failed to reach the file, if they did (they are
    /// still held in memory).
    #[must_use]
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// Writes every owner's pins to the file, whole, if there is one.
    fn save(&mut self) {
        let Some(file) = &self.file else { return };
        let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else {
            self.save_error = Some(format!("{}: not a file path", file.display()));
            return;
        };
        let owners: serde_json::Map<String, Json> =
            self.pins.iter().map(|(o, p)| (o.clone(), json!(p))).collect();
        let text = serde_json::to_string(&Json::Object(owners)).unwrap_or_default();
        self.save_error = crate::project::write_atomic(dir, name, text.as_bytes())
            .err()
            .map(|e| format!("{}: {e}", file.display()));
    }

    /// The signals tracked now.
    #[must_use]
    pub fn tracked(&self) -> Vec<String> {
        self.tracks.keys().cloned().collect()
    }

    fn pinned(&self, name: &str) -> bool {
        self.pins.iter().any(|(_, p)| p.iter().any(|s| s == name))
    }

    /// Tracks `names` (the box's signals are `known`) for another
    /// [`LEASE`] from the last sample.
    pub fn want(&mut self, names: &[String], known: &[String]) -> Wanted {
        let mut w = Wanted::default();
        let until = self.now + LEASE;
        for n in names {
            if w.kept.contains(n) || w.unknown.contains(n) || w.refused.contains(n) {
                continue;
            }
            if !known.contains(n) {
                w.unknown.push(n.clone());
                continue;
            }
            let room = self.tracks.len() < MAX_TRACKED;
            match self.tracks.get_mut(n) {
                Some(t) => {
                    t.until = t.until.max(until);
                    w.kept.push(n.clone());
                }
                None if room => {
                    self.tracks.insert(
                        n.clone(),
                        Track {
                            points: VecDeque::new(),
                            until,
                        },
                    );
                    w.kept.push(n.clone());
                }
                None => w.refused.push(n.clone()),
            }
        }
        w
    }

    /// How many points `name`'s history holds (0: not tracked).
    #[must_use]
    pub fn history_len(&self, name: &str) -> usize {
        self.tracks.get(name).map_or(0, |t| t.points.len())
    }

    /// `owner`'s pins (none set: empty).
    #[must_use]
    pub fn pins(&self, owner: &str) -> Vec<String> {
        self.pins
            .iter()
            .find(|(o, _)| o == owner)
            .map(|(_, p)| p.clone())
            .unwrap_or_default()
    }

    /// Sets `owner`'s pins (checked by the caller) and tracks them.
    pub fn set_pins(&mut self, owner: &str, names: &[String], known: &[String]) -> Wanted {
        let w = self.want(names, known);
        self.pins.retain(|(o, _)| o != owner);
        if !w.kept.is_empty() {
            self.pins.push((owner.to_string(), w.kept.clone()));
        }
        while self.pins.len() > MAX_OWNERS {
            self.pins.remove(0);
        }
        self.save();
        w
    }

    /// Takes in a published view: each tracked signal's change, if it
    /// changed. `names` are the view's signals, in its order.
    pub fn sample(&mut self, view: &View, names: &[String]) {
        if view.seq == self.seq {
            return;
        }
        self.seq = view.seq;
        self.now = view.at;
        if self.untracked {
            // Pins read at boot: tracked now that the signals are known.
            self.untracked = false;
            let mut want: Vec<String> = Vec::new();
            for n in self.pins.iter().flat_map(|(_, p)| p.iter()) {
                if !want.contains(n) {
                    want.push(n.clone());
                }
            }
            self.want(&want, names);
        }
        let now = view.at;
        let from = now.saturating_sub(WINDOW);
        for (name, t) in &mut self.tracks {
            let v = names
                .iter()
                .position(|n| n == name)
                .and_then(|i| view.signals.get(i).copied().flatten());
            if t.points.back().is_none_or(|(_, last)| *last != v) {
                t.points.push_back((now, v));
            }
            // Keep one point at or before the window's start: the value
            // the window opens on.
            while t.points.len() > 1 && t.points.get(1).is_some_and(|(at, _)| *at <= from) {
                t.points.pop_front();
            }
            while t.points.len() > MAX_POINTS {
                t.points.pop_front();
            }
        }
        let expired: Vec<String> = self
            .tracks
            .iter()
            .filter(|(n, t)| t.until < now && !self.pinned(n))
            .map(|(n, _)| n.clone())
            .collect();
        for n in expired {
            self.tracks.remove(&n);
        }
    }

    /// `name`'s history as JSON: `[[t_ms, value], ...]`, the first point
    /// clamped to the window's start.
    fn history(&self, name: &str) -> Json {
        let from = self.now.saturating_sub(WINDOW);
        let points: Vec<Json> = self
            .tracks
            .get(name)
            .map(|t| {
                t.points
                    .iter()
                    .map(|(at, v)| json!([ms((*at).max(from)), super::render::value(*v)]))
                    .collect()
            })
            .unwrap_or_default();
        json!({ "name": name, "points": points })
    }

    /// The answer to a history request for `w`.
    fn answer(&self, w: &Wanted) -> Json {
        json!({
            "now_ms": ms(self.now),
            "window_ms": ms(WINDOW),
            "signals": w.kept.iter().map(|n| self.history(n)).collect::<Vec<_>>(),
            "unknown": w.unknown,
            "refused": w.refused,
        })
    }
}

/// The box's signal names, in view order.
fn known(ctx: &Ctx) -> Vec<String> {
    ctx.catalog().signals.iter().map(|s| s.name.clone()).collect()
}

/// Samples the newest view (the API's accept loop, every stream period).
pub(super) fn sample(ctx: &Ctx) {
    let view = ctx.box_.live.view();
    let names = known(ctx);
    ctx.trends().sample(&view, &names);
}

/// Names from `?signals=a,b` (empty parts skipped).
fn names(req: &Request) -> Vec<String> {
    req.query
        .iter()
        .filter(|(k, _)| k == "signals")
        .flat_map(|(_, v)| v.split(','))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn too_many(n: usize) -> Response {
    Response::error(400, &format!("{n} signals; a trend takes at most {MAX_PINS}"))
}

/// `GET /api/trends[...]`.
pub(super) fn get(req: &Request, ctx: &Ctx, rest: &[&str]) -> Response {
    match rest {
        [] => {
            let want = names(req);
            if want.len() > MAX_PINS {
                return too_many(want.len());
            }
            let known = known(ctx);
            let mut t = ctx.trends();
            let w = t.want(&want, &known);
            Response::json(200, &t.answer(&w))
        }
        ["pins"] => match owner(req, ctx) {
            Some(o) => Response::json(200, &json!({ "owner": o, "pins": ctx.trends().pins(&o) })),
            None => Response::error(
                401,
                "pins are a signed-in person's; signed out, the browser keeps them",
            ),
        },
        _ => Response::error(404, "no such resource"),
    }
}

/// Who pins: a person (an account) or, for a role-PIN session, the role.
fn owner_of(p: &crate::policy::account::Principal) -> String {
    match p.person() {
        Some(u) => format!("user:{u}"),
        None => format!("role:{}", p.role.as_str()),
    }
}

/// The session a `GET` sent (cookie or bearer), as an owner.
fn owner(req: &Request, ctx: &Ctx) -> Option<String> {
    let token = if req.authorization {
        req.bearer.as_deref()
    } else {
        match req.cookies.as_slice() {
            [t] => Some(t.as_str()),
            _ => None,
        }
    }?;
    logins(ctx)?.session(token).ok().map(|p| owner_of(&p))
}

/// `PUT /api/trends/pins` (authenticated by the caller).
pub(super) fn put_pins(req: &Request, ctx: &Ctx) -> Response {
    let Some(who) = req.session.as_ref() else {
        return audited(
            ctx,
            req,
            400,
            &json!({ "error": "pins are a person's: sign in (the box token is no one)" }),
            "trend pins: not a login session",
        );
    };
    let owner = owner_of(who);
    let body: Option<Vec<String>> = is_json(req)
        .then(|| serde_json::from_slice::<Json>(&req.body).ok())
        .flatten()
        .and_then(|j| {
            j["signals"]
                .as_array()?
                .iter()
                .map(|s| s.as_str().map(str::to_string))
                .collect()
        });
    let Some(want) = body else {
        return audited(
            ctx,
            req,
            400,
            &json!({ "error": "expected application/json {signals: [names]}" }),
            "trend pins: bad body",
        );
    };
    if want.len() > MAX_PINS {
        let detail = format!("trend pins: {} signals refused", want.len());
        return audited(
            ctx,
            req,
            400,
            &json!({ "error": format!("{} signals; at most {MAX_PINS}", want.len()) }),
            &detail,
        );
    }
    let known = known(ctx);
    let unknown: Vec<&String> = want.iter().filter(|n| !known.contains(n)).collect();
    if !unknown.is_empty() {
        return audited(
            ctx,
            req,
            422,
            &json!({ "error": "no such signal on this box", "unknown": unknown }),
            "trend pins: unknown signals",
        );
    }
    let (w, not_kept) = {
        let mut t = ctx.trends();
        let w = t.set_pins(&owner, &want, &known);
        (w, t.save_error().map(str::to_string))
    };
    let mut detail = format!("trend pins of {owner}: {} signal(s)", w.kept.len());
    if let Some(e) = &not_kept {
        detail.push_str("; not kept across a restart: ");
        detail.push_str(e);
    }
    audited(
        ctx,
        req,
        200,
        &json!({ "owner": owner, "pins": w.kept, "refused": w.refused, "kept": not_kept.is_none() }),
        &super::clean(&detail),
    )
}
