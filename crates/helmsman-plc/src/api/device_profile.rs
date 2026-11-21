//! A device profile the agent proposes for a device the library does not
//! know (card `device-profile-propose-box`, PROTOCOLS.md step 5): held on
//! the box for the user to check, rename and save into a project.
//!
//! ```text
//! POST   /api/device-profile/proposal         token or session: {name, text, summary} (the agent's
//!                                             `propose_device_profile`): the text is read by
//!                                             `helmsman_io::profile::Profile::parse`; 201 with the
//!                                             channel list and the proposal's `id`, or 422 with
//!                                             {problems: [{line, error}]} and nothing held
//! GET    /api/device-profile/proposal         the pending proposal: id, name, summary, text, vendor,
//!                                             model, kind, transports, channels; 404 when none
//! POST   /api/device-profile/proposal/accept  engineer session: {id, project, name?, channels?:
//!                                             {FROM: TO}}: the channels renamed, the whole text
//!                                             read again, and `NAME.device` saved into the project
//!                                             as a new file (201, a revision in its history)
//! DELETE /api/device-profile/proposal         engineer session: the proposal dropped (200)
//! POST   /api/device-profile/publish          engineer session: {project, file}: the saved `NAME.device`,
//!                                             read again by `Profile::parse`, sent to the relay's
//!                                             `publish_device_profile` (a draft pull request into the
//!                                             public device library); 201 with the pull request, 200
//!                                             when this profile already has one open
//! ```
//!
//! **Held, never written.** The one pending proposal lives in memory (a
//! restart forgets it, like a drop-in) and replaces an older one. A model's
//! call can only fill that slot: no file exists until an engineer accepts
//! it, and accepting is a project save like any other (the same writer,
//! the same caps, a project revision, an audit entry naming the person).
//! The box token alone, which is the agent's, accepts nothing (403).
//!
//! **One reader.** Every text, proposed or renamed, is accepted only
//! through `Profile::parse`; a rename that does not read, or that makes two
//! channels one, answers 422 and leaves the proposal and the project as
//! they were. `id` ties an accept to the proposal the person looked at: a
//! newer proposal from the agent in between answers 409.
//!
//! **Hostile by default.** The text is a model's words made from a
//! vendor's file: bounded ([`MAX_TEXT`] bytes, [`MAX_LINES`] lines,
//! [`MAX_CHANNELS`] channels), free of control and bidirectional
//! characters, and the name is a project file's stem. Nothing from the
//! text goes to the audit log or the box's log but the proposal's name and
//! counts.

use std::collections::BTreeMap;
use std::time::Duration;

use helmsman_io::model::{Dir, Ty};
use helmsman_io::profile::Profile;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use super::projects::{by, engineer};
use super::{Ctx, Request, Response, audited, clean, is_json, wall};
use crate::project::{self, FileKind};

/// Longest profile text, bytes (the E1213's, with 40 channels, is 3 KiB).
pub const MAX_TEXT: usize = 64 * 1024;
/// Most lines of a profile text.
pub const MAX_LINES: usize = 1000;
/// Most channels of a profile.
pub const MAX_CHANNELS: usize = 256;
/// Longest summary, characters.
pub const MAX_SUMMARY: usize = 8000;
/// Most problems one refusal lists.
const MAX_PROBLEMS: usize = 50;
/// The relay agent a publish asks for unless `--relay-publish-agent` names
/// another: the profile that lists the relay's `publish_device_profile`.
pub const PUBLISH_AGENT: &str = "helmsman/v13";
/// Longest silence from the relay while it makes its calls to GitHub.
pub const PUBLISH_IDLE: Duration = Duration::from_secs(60);
/// Longest a publish may take.
pub const PUBLISH_TOTAL: Duration = Duration::from_secs(90);

/// The one pending proposal.
#[derive(Debug)]
pub(super) struct Pending {
    id: u64,
    name: String,
    summary: String,
    text: String,
    profile: Profile,
    at: u64,
    by: String,
}

/// The slot: the pending proposal and the id the next one takes.
#[derive(Debug, Default)]
pub(super) struct Held {
    next: u64,
    pending: Option<Pending>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposed {
    name: String,
    text: String,
    summary: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    project: String,
    /// `NAME.device`, a file of the project.
    file: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accept {
    id: u64,
    project: String,
    name: Option<String>,
    channels: Option<BTreeMap<String, String>>,
}

/// Whether `name` is a profile's file stem as a project holds it
/// (`NAME.device`).
fn name_ok(name: &str) -> bool {
    project::file_name(&format!("{name}{}", helmsman_project::DEVICE_EXT)) == Some(FileKind::Device)
}

fn problems(list: &[(usize, String)]) -> Json {
    Json::Array(
        list.iter()
            .take(MAX_PROBLEMS)
            .map(|(line, e)| json!({ "line": line, "error": clean(e) }))
            .collect(),
    )
}

/// The profile as the canvas and the agent read it: no text.
fn describe(p: &Profile) -> Json {
    json!({
        "vendor": p.vendor, "model": p.model, "kind": p.kind, "configure": p.configure,
        "transports": p.transports.iter().map(|t| json!({
            "driver": t.driver,
            "params": t.params,
            "cycle_ms": t.limits().map(|(a, b)| json!([a.as_millis(), b.as_millis()])),
        })).collect::<Vec<_>>(),
        "channels": p.channels.iter().map(|c| json!({
            "name": c.name,
            "dir": if c.dir == Dir::In { "in" } else { "out" },
            "type": match c.ty { Ty::Bool => "bool", Ty::Int => "int", Ty::Real => "real" },
            "unit": c.unit, "safe": c.safe, "points": c.points,
        })).collect::<Vec<_>>(),
    })
}

fn refused(
    ctx: &Ctx,
    req: &Request,
    status: u16,
    why: &str,
    list: &[(usize, String)],
    detail: &str,
) -> Response {
    audited(
        ctx,
        req,
        status,
        &json!({ "error": why, "problems": problems(list) }),
        detail,
    )
}

/// The text's own problems before `Profile::parse` sees it: size, lines,
/// characters.
fn shape(text: &str) -> Result<(), String> {
    if text.len() > MAX_TEXT {
        return Err(format!("the profile is larger than {MAX_TEXT} bytes"));
    }
    if text.lines().count() > MAX_LINES {
        return Err(format!("the profile has more than {MAX_LINES} lines"));
    }
    helmsman_project::check_text(text.as_bytes()).map(drop)
}

/// A summary for people to read: control and bidirectional characters
/// (but line breaks) as spaces, cut at [`MAX_SUMMARY`] characters.
fn clean_summary(s: &str) -> String {
    s.chars()
        .take(MAX_SUMMARY)
        .map(|c| {
            let bidi =
                matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}');
            if bidi || (c.is_control() && c != '\n') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `text` with channel names changed as `renames` says: only the name of
/// an `in` or `out` line is touched, the rest of the file as written.
/// Every key must be a channel of the profile.
fn rename(
    text: &str,
    profile: &Profile,
    renames: &BTreeMap<String, String>,
) -> Result<String, Vec<(usize, String)>> {
    let missing: Vec<(usize, String)> = renames
        .keys()
        .filter(|k| !profile.channels.iter().any(|c| &c.name == *k))
        .map(|k| (0, format!("no channel {k:?} to rename")))
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    if renames.is_empty() {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len() + 64);
    for raw in text.split_inclusive('\n') {
        let body = raw.trim_end_matches(['\n', '\r']);
        let eol = &raw[body.len()..];
        let (code, comment) = body.find('#').map_or((body, ""), |i| body.split_at(i));
        let words: Vec<&str> = code.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
        match words.as_slice() {
            [dir @ ("in" | "out"), name, rest @ ..] if renames.contains_key(*name) => {
                out.push_str(dir);
                out.push(' ');
                out.push_str(&renames[*name]);
                for w in rest {
                    out.push(' ');
                    out.push_str(w);
                }
                if !comment.is_empty() {
                    out.push(' ');
                    out.push_str(comment);
                }
                out.push_str(eol);
            }
            _ => out.push_str(raw),
        }
    }
    Ok(out)
}

/// `GET /api/device-profile/proposal`.
pub(super) fn get(ctx: &Ctx) -> Response {
    let held = ctx.device_profile();
    match &held.pending {
        None => Response::error(404, "no device profile is waiting for you"),
        Some(p) => {
            let mut j = describe(&p.profile);
            j["id"] = json!(p.id);
            j["name"] = json!(p.name);
            j["summary"] = json!(p.summary);
            j["text"] = json!(p.text);
            j["at"] = json!(p.at);
            j["by"] = json!(p.by);
            Response::json(200, &j)
        }
    }
}

/// The non-`GET` routes under `/api/device-profile`; every outcome audited.
pub(super) fn mutate(req: &Request, ctx: &Ctx, rest: &[&str]) -> Response {
    match (req.method.as_str(), rest) {
        ("POST", ["proposal"]) => propose(req, ctx),
        ("POST", ["proposal", "accept"]) => accept(req, ctx),
        ("POST", ["publish"]) => publish(req, ctx),
        ("DELETE", ["proposal"]) => discard(req, ctx),
        _ => {
            let allow = match rest {
                ["proposal"] => Some("GET, POST, DELETE"),
                ["proposal", "accept"] | ["publish"] => Some("POST"),
                _ => None,
            };
            let (r, detail) = match allow {
                Some(a) => (
                    Response::error(405, "method not allowed").with("Allow", a.into()),
                    "device profile: method not allowed",
                ),
                None => (Response::error(404, "no such resource"), "no such resource"),
            };
            let status = r.status;
            match super::audit(ctx, req, status, detail) {
                Some(_) => r,
                None => super::audit_full(),
            }
        }
    }
}

fn propose(req: &Request, ctx: &Ctx) -> Response {
    let no = |status: u16, why: &str, list: &[(usize, String)], detail: &str| {
        refused(ctx, req, status, why, list, detail)
    };
    if !is_json(req) {
        return no(
            415,
            "Content-Type must be application/json",
            &[],
            "device profile: not JSON",
        );
    }
    let new: Proposed = match serde_json::from_slice(&req.body) {
        Ok(n) => n,
        Err(e) => {
            return no(
                400,
                &format!("expected {{name, text, summary}}: {e}"),
                &[],
                "device profile: malformed proposal",
            );
        }
    };
    let what = format!("device profile {}", clean(&new.name));
    if !name_ok(&new.name) {
        return no(
            422,
            "the name is a file name without .device: 1 to 48 of a-z 0-9 _ -, starting with a letter or digit",
            &[],
            &format!("{what}: not proposed, bad name"),
        );
    }
    let summary = clean_summary(&new.summary);
    if summary.is_empty() {
        return no(
            422,
            "a proposal needs a summary the user can check it against",
            &[],
            &format!("{what}: not proposed, no summary"),
        );
    }
    if let Err(e) = shape(&new.text) {
        return no(422, &e, &[], &format!("{what}: not proposed, text refused"));
    }
    let profile = match Profile::parse(&new.text) {
        Ok(p) => p,
        Err(list) => {
            return no(
                422,
                "the profile does not check",
                &list,
                &format!("{what}: not proposed, {} problem(s)", list.len()),
            );
        }
    };
    if profile.channels.len() > MAX_CHANNELS {
        return no(
            422,
            &format!("a profile has at most {MAX_CHANNELS} channels"),
            &[],
            &format!("{what}: not proposed, too many channels"),
        );
    }
    let channels = profile.channels.len();
    let mut j = describe(&profile);
    let mut held = ctx.device_profile();
    let id = held.next + 1;
    held.next = id;
    let replaced = held
        .pending
        .replace(Pending {
            id,
            name: new.name.clone(),
            summary,
            text: new.text,
            profile,
            at: wall(),
            by: by(req),
        })
        .is_some();
    drop(held);
    j["id"] = json!(id);
    j["name"] = json!(new.name);
    j["replaced"] = json!(replaced);
    audited(
        ctx,
        req,
        201,
        &j,
        &format!(
            "{what}: held, {channels} channel(s){}",
            if replaced {
                ", replacing the earlier proposal"
            } else {
                ""
            }
        ),
    )
}

#[allow(clippy::too_many_lines)] // one flow: every refusal is its own audited answer
fn accept(req: &Request, ctx: &Ctx) -> Response {
    const WHAT: &str = "device profile accept";
    if let Err(r) = engineer(ctx, req, WHAT) {
        return r;
    }
    let no = |status: u16, why: &str, list: &[(usize, String)], detail: &str| {
        refused(ctx, req, status, why, list, detail)
    };
    let Some(projects) = &ctx.projects else {
        return no(
            404,
            "no projects on this box (--projects)",
            &[],
            "device profile accept: no projects",
        );
    };
    if !is_json(req) {
        return no(
            415,
            "Content-Type must be application/json",
            &[],
            "device profile accept: not JSON",
        );
    }
    let a: Accept = match serde_json::from_slice(&req.body) {
        Ok(a) => a,
        Err(e) => {
            return no(
                400,
                &format!("expected {{id, project, name?, channels?}}: {e}"),
                &[],
                "device profile accept: malformed",
            );
        }
    };
    let mut held = ctx.device_profile();
    let Some(p) = &held.pending else {
        drop(held);
        return no(
            404,
            "no device profile is waiting for you",
            &[],
            "device profile accept: nothing pending",
        );
    };
    if p.id != a.id {
        drop(held);
        return no(
            409,
            "the proposal changed since you looked at it; read it again",
            &[],
            "device profile accept: stale id",
        );
    }
    let name = a.name.as_deref().unwrap_or(&p.name).to_string();
    if !name_ok(&name) {
        drop(held);
        return no(
            422,
            "the name is a file name without .device: 1 to 48 of a-z 0-9 _ -, starting with a letter or digit",
            &[],
            "device profile accept: bad name",
        );
    }
    let what = format!("device profile {}", clean(&name));
    let renamed = match rename(&p.text, &p.profile, &a.channels.unwrap_or_default()) {
        Ok(t) => t,
        Err(list) => {
            drop(held);
            return no(
                422,
                "the rename does not check",
                &list,
                &format!("{what}: not saved, bad rename"),
            );
        }
    };
    // The final text, whole, through the one reader (and the same bounds).
    if let Err(e) = shape(&renamed) {
        drop(held);
        return no(422, &e, &[], &format!("{what}: not saved, text refused"));
    }
    if let Err(list) = Profile::parse(&renamed) {
        drop(held);
        return no(
            422,
            "the profile does not check after the rename",
            &list,
            &format!("{what}: not saved, {} problem(s) after the rename", list.len()),
        );
    }
    let file = format!("{name}{}", helmsman_project::DEVICE_EXT);
    // `Some(None)`: only as a new file; one that exists is never replaced.
    let version = match projects.save(&a.project, &file, renamed.as_bytes(), Some(None)) {
        Ok(v) => v,
        Err(r) => {
            drop(held);
            return no(
                r.status,
                &r.why,
                &[],
                &format!("{what}: not saved: {}", clean(&r.why)),
            );
        }
    };
    let revision = projects
        .record_revision(
            &a.project,
            &by(req),
            &format!("saved {file} from the agent's proposal"),
            None,
        )
        .ok();
    held.pending = None;
    drop(held);
    audited(
        ctx,
        req,
        201,
        &json!({ "project": a.project, "file": file, "version": version, "revision": revision }),
        &format!(
            "{what}: saved as {} in project {}",
            clean(&file),
            clean(&a.project)
        ),
    )
}

fn discard(req: &Request, ctx: &Ctx) -> Response {
    if let Err(r) = engineer(ctx, req, "device profile discard") {
        return r;
    }
    let gone = ctx.device_profile().pending.take();
    match gone {
        Some(p) => audited(
            ctx,
            req,
            200,
            &json!({ "discarded": p.name }),
            &format!("device profile {}: discarded", clean(&p.name)),
        ),
        None => audited(
            ctx,
            req,
            404,
            &json!({ "error": "no device profile is waiting for you" }),
            "device profile discard: nothing pending",
        ),
    }
}

/// The box's status and words for what the relay said.
fn relay_refusal(e: &helmsman_agent::publish::Error) -> (u16, String, String, bool) {
    use helmsman_agent::publish::Error;
    match e {
        Error::Refused {
            status,
            kind,
            message,
            retryable,
        } => {
            let (code, words) = match *status {
                400 => (422, message.clone()),
                409 | 429 | 502 | 503 => (*status, message.clone()),
                401 | 403 | 404 => (
                    502,
                    "the agent service does not offer publishing to this box: the agent version it asks for is not served to it. Nothing was sent"
                        .to_string(),
                ),
                _ => (502, "the agent service could not publish it".to_string()),
            };
            (code, words, kind.clone(), *retryable)
        }
        Error::Unreachable(_) => (
            502,
            "the agent service could not be reached. Nothing was sent".into(),
            "relay_unreachable".into(),
            true,
        ),
        Error::Malformed => (
            502,
            "the agent service's answer was not understood".into(),
            "relay_malformed".into(),
            false,
        ),
    }
}

/// `POST /api/device-profile/publish`.
#[allow(clippy::too_many_lines)] // one flow: every refusal is its own audited answer
fn publish(req: &Request, ctx: &Ctx) -> Response {
    const WHAT: &str = "device profile publish";
    if let Err(r) = engineer(ctx, req, WHAT) {
        return r;
    }
    let no = |status: u16, why: &str, list: &[(usize, String)], detail: &str| {
        refused(ctx, req, status, why, list, detail)
    };
    let (Some(projects), Some(chat)) = (&ctx.projects, &ctx.chat) else {
        return no(
            404,
            if ctx.projects.is_none() {
                "no projects on this box (--projects)"
            } else {
                "publishing goes through the agent service, which this box has none of (--relay)"
            },
            &[],
            "device profile publish: not available",
        );
    };
    if !is_json(req) {
        return no(
            415,
            "Content-Type must be application/json",
            &[],
            "device profile publish: not JSON",
        );
    }
    let p: Publish = match serde_json::from_slice(&req.body) {
        Ok(p) => p,
        Err(e) => {
            return no(
                400,
                &format!("expected {{project, file}}: {e}"),
                &[],
                "device profile publish: malformed",
            );
        }
    };
    let Some(name) = p
        .file
        .strip_suffix(helmsman_project::DEVICE_EXT)
        .filter(|n| name_ok(n))
    else {
        return no(
            422,
            "the file is a device profile of the project: NAME.device",
            &[],
            "device profile publish: not a profile file",
        );
    };
    let what = format!("device profile {}", clean(name));
    // The saved file, never text from the request.
    let text = match projects.file(&p.project, &p.file) {
        Ok(t) => t,
        Err(r) => {
            return no(
                r.status,
                &r.why,
                &[],
                &format!("{what}: not published: {}", clean(&r.why)),
            );
        }
    };
    if let Err(e) = shape(&text) {
        return no(422, &e, &[], &format!("{what}: not published, text refused"));
    }
    let profile = match Profile::parse(&text) {
        Ok(p) => p,
        Err(list) => {
            return no(
                422,
                "the profile does not check",
                &list,
                &format!("{what}: not published, {} problem(s)", list.len()),
            );
        }
    };
    if profile.channels.len() > MAX_CHANNELS {
        return no(
            422,
            &format!("a profile has at most {MAX_CHANNELS} channels"),
            &[],
            &format!("{what}: not published, too many channels"),
        );
    }
    match helmsman_agent::publish::publish(&chat.publish_relay(), name, &text) {
        Ok(done) => {
            let status = if done.created { 201 } else { 200 };
            audited(
                ctx,
                req,
                status,
                &json!({
                    "project": p.project, "file": p.file, "number": done.number, "url": done.url,
                    "created": done.created, "draft": done.draft, "folder": done.folder,
                }),
                &format!(
                    "{what}: published as pull request #{}{}",
                    done.number,
                    if done.created { "" } else { " (already open)" }
                ),
            )
        }
        Err(e) => {
            let (status, words, kind, retryable) = relay_refusal(&e);
            audited(
                ctx,
                req,
                status,
                &json!({ "error": words, "kind": kind, "retryable": retryable }),
                &format!("{what}: not published: {}", clean(&kind)),
            )
        }
    }
}
