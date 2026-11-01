//! The box's projects as the Editor sees them, free of I/O like
//! [`crate::remote`] and [`crate::review`]: what to ask the box's project
//! and force routes for ([`PReq`]), and what to make of each answer. The
//! browser's `fetch` (`web.rs`) carries the requests out; tests carry them
//! out natively against the real API.
//!
//! - `GET /api/projects` (404: the box runs without `--projects`, and the
//!   Editor shows its modules as before), `GET /api/projects/{p}` and every
//!   file of the open project: the Files sidebar and the ST editor.
//! - `PUT .../files/{f}` with the version read: Save. A 409 means someone
//!   else saved that file since; the page offers to reload it.
//! - `POST .../check`, then `.../replay` when the policy wants one, then
//!   `.../deploy` citing it: Deploy. A 422 carries every compile error
//!   with its file and line.
//! - `POST .../zip`, `POST .../import?file=F`: Upload. `GET .../zip`:
//!   Download (the browser fetches it itself).
//! - `POST /api/forces`, `DELETE /api/forces[/{id}]`: forcing.
//! - `GET` and `PUT .../files/diagram.layout` of the running project: the
//!   diagram's shared default arrangement (`arrange.rs`), saved by an
//!   engineer like any project file.
//!
//! Every write needs an engineer's login session, which the browser holds
//! as an `HttpOnly` cookie; the page never sees a credential. A 401 means
//! the session ended, and the page goes back to signing in.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Value as Json, json};

use crate::projects::{Diag, Group, ProjectRow};
use crate::remote::{ForceRow, segment};

/// What a request is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PCall {
    List,
    Project(String),
    File {
        project: String,
        file: String,
    },
    Create(String),
    Rename {
        from: String,
        to: String,
    },
    Delete(String),
    Save {
        project: String,
        file: String,
    },
    NewFile {
        project: String,
        file: String,
    },
    /// The first half of a file rename: the new name written.
    MoveFile {
        project: String,
        from: String,
        to: String,
    },
    DeleteFile {
        project: String,
        file: String,
    },
    Check(String),
    Replay(String),
    Deploy {
        project: String,
        replay: Option<u64>,
    },
    Upload(String),
    Import {
        project: String,
        file: String,
    },
    Forces,
    Force,
    Release,
    /// The signed-in person's trend pins (`box_trend.rs`), read.
    TrendPins,
    /// ... and saved.
    SavePins,
    /// The pins' history.
    Trend,
    /// The running project's shared diagram layout, read.
    Layout(String),
    /// ... and saved.
    SaveLayout(String),
}

/// The running project's shared diagram layout, as the box holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedLayout {
    pub project: String,
    /// The file's text (`""`: the project has none).
    pub text: String,
    /// Its version (`None`: no file yet).
    pub version: Option<String>,
}

/// One HTTP call: same-origin, with the browser's session cookie and, for
/// anything but a `GET`, the `X-Helmsman-Request: 1` header.
#[derive(Clone, PartialEq, Eq)]
pub struct PReq {
    pub call: PCall,
    pub method: &'static str,
    pub url: String,
    pub body: Option<Vec<u8>>,
    /// The body's `Content-Type`.
    pub ctype: &'static str,
}

impl fmt::Debug for PReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PReq")
            .field("call", &self.call)
            .field("method", &self.method)
            .field("url", &self.url)
            .field("body", &self.body.as_ref().map(Vec::len))
            .field("ctype", &self.ctype)
            .finish()
    }
}

/// What came back (`status` 0: no answer; `body` says why).
pub use crate::review::Answer;

const JSON: &str = "application/json";

impl PReq {
    pub(crate) fn get(call: PCall, url: String) -> Self {
        Self {
            call,
            method: "GET",
            url,
            body: None,
            ctype: JSON,
        }
    }

    pub(crate) fn json(call: PCall, method: &'static str, url: String, body: &Json) -> Self {
        Self {
            call,
            method,
            url,
            body: Some(body.to_string().into_bytes()),
            ctype: JSON,
        }
    }

    fn delete(call: PCall, url: String) -> Self {
        Self {
            call,
            method: "DELETE",
            url,
            body: None,
            ctype: JSON,
        }
    }
}

fn project_url(p: &str) -> String {
    format!("/api/projects/{}", segment(p))
}

fn file_url(p: &str, f: &str) -> String {
    format!("/api/projects/{}/files/{}", segment(p), segment(f))
}

/// The download link of project `p` (a `GET`, open like every read).
#[must_use]
pub fn zip_url(p: &str) -> String {
    format!("/api/projects/{}/zip", segment(p))
}

/// One file of the open project, as edited here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Buf {
    /// What the editor holds.
    pub text: String,
    /// What the box holds, as last read or saved.
    pub base: String,
    /// The box's version of `base`; `None` until read.
    pub version: Option<String>,
    /// The text of a save on its way.
    pub saving: Option<String>,
    /// Read from the box at least once.
    pub loaded: bool,
}

impl Buf {
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.loaded && self.text != self.base
    }
}

/// The project open in the Editor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenProject {
    pub name: String,
    pub bufs: BTreeMap<String, Buf>,
    /// The file in the ST editor.
    pub current: Option<String>,
}

/// Where a Deploy is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Deploy {
    #[default]
    Idle,
    /// Saving the edits first.
    Saving,
    Checking,
    Replaying,
    Deploying,
    /// What runs now.
    Done(String),
    Failed(String),
}

impl Deploy {
    #[must_use]
    pub const fn busy(&self) -> bool {
        matches!(
            self,
            Self::Saving | Self::Checking | Self::Replaying | Self::Deploying
        )
    }
}

/// An upload the box refused because the name is taken, kept so the page
/// can offer to replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub file: String,
    pub bytes: Vec<u8>,
    /// The project it goes to (a `.zip`'s) or into (a file's).
    pub project: String,
}

/// Everything the Editor knows about the box's projects.
#[derive(Debug, Default)]
pub struct BoxProjects {
    /// `None` until the box answered; `Some(false)`: no `--projects`.
    pub available: Option<bool>,
    pub projects: Vec<ProjectRow>,
    /// The project last deployed, while it is still what runs.
    pub running: Option<String>,
    pub open: Option<OpenProject>,
    /// The last check's or deploy's compile errors.
    pub diags: Vec<Diag>,
    pub deploy: Deploy,
    /// A file whose save the box refused as stale (409).
    pub conflict: Option<String>,
    /// A one-line note: what happened, or why not (`true`: an error).
    pub notice: Option<(String, bool)>,
    /// An upload waiting for "Replace".
    pub pending: Option<Pending>,
    /// A 401 said the box no longer knows this page's session.
    pub ended: bool,
    /// The forces list, when a force call just changed it.
    pub forces: Option<Vec<ForceRow>>,
    /// A deploy changed what runs: the module list should be read again.
    pub deployed: bool,
    /// Links to hand the browser as downloads.
    pub downloads: Vec<String>,
    /// The running project's shared diagram layout, once read.
    pub layout: Option<SharedLayout>,
    /// A layout this page saved as the default, for the view to adopt.
    pub layout_saved: Option<String>,
    /// The layout being saved.
    saving_layout: Option<String>,
    /// The modules the last check said the project builds.
    modules: Vec<String>,
    queue: Vec<PReq>,
}

fn error_of(a: &Answer) -> String {
    serde_json::from_str::<Json>(&a.body)
        .ok()
        .and_then(|j| j["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| match a.status {
            0 => format!("the box did not answer: {}", a.body),
            s => format!("HTTP {s}"),
        })
}

fn diags_of(j: &Json) -> Vec<Diag> {
    j["diagnostics"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| Diag {
                    file: d["file"].as_str().unwrap_or("?").to_string(),
                    line: d["line"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0),
                    col: d["col"].as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
                    message: d["message"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `file` ends in `.ext`, in any case.
fn has_ext(file: &str, ext: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn short(s: &str) -> &str {
    s.get(..8).unwrap_or(s)
}

impl BoxProjects {
    /// Starts: the project list.
    #[must_use]
    pub fn new() -> Self {
        let mut s = Self::default();
        s.relist();
        s
    }

    /// What to send now.
    pub fn poll(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    fn relist(&mut self) {
        self.queue.push(PReq::get(PCall::List, "/api/projects".into()));
    }

    fn note(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some((text.into(), error));
    }

    /// The open project has edits not saved.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|o| o.bufs.values().any(Buf::dirty))
    }

    /// The file in the ST editor.
    #[must_use]
    pub fn current(&self) -> Option<&Buf> {
        let o = self.open.as_ref()?;
        o.bufs.get(o.current.as_ref()?)
    }

    pub fn current_mut(&mut self) -> Option<&mut Buf> {
        let o = self.open.as_mut()?;
        o.bufs.get_mut(o.current.as_ref()?)
    }

    /// Opens project `name` (every file is read).
    pub fn open_project(&mut self, name: &str) {
        self.open = Some(OpenProject {
            name: name.into(),
            ..OpenProject::default()
        });
        self.diags.clear();
        self.conflict = None;
        self.queue
            .push(PReq::get(PCall::Project(name.into()), project_url(name)));
    }

    /// Shows `file` of the open project in the ST editor.
    pub fn open_file(&mut self, file: &str) {
        if let Some(o) = &mut self.open
            && o.bufs.contains_key(file)
        {
            o.current = Some(file.into());
        }
    }

    pub fn create(&mut self, name: &str, from: Option<&str>) {
        let body = match from {
            Some(f) => json!({ "name": name, "from": f }),
            None => json!({ "name": name }),
        };
        self.queue.push(PReq::json(
            PCall::Create(name.into()),
            "POST",
            "/api/projects".into(),
            &body,
        ));
    }

    /// A new project `name` with the files of a built-in machine or
    /// template, sent as the project's `.zip` so it arrives all at once.
    pub fn create_from(&mut self, name: &str, from: &str) {
        let Some(s) = helmsman_project::starters::starter(from) else {
            return;
        };
        match s.project().to_zip() {
            Ok(zip) => self.upload(&format!("{name}.zip"), zip, false),
            Err(e) => self.note(format!("{name} was not made: {e}"), true),
        }
    }

    pub fn rename(&mut self, from: &str, to: &str) {
        self.queue.push(PReq::json(
            PCall::Rename {
                from: from.into(),
                to: to.into(),
            },
            "POST",
            format!("{}/rename", project_url(from)),
            &json!({ "to": to }),
        ));
    }

    pub fn delete(&mut self, name: &str) {
        self.queue
            .push(PReq::delete(PCall::Delete(name.into()), project_url(name)));
    }

    /// A new, empty `.st` file in the open project.
    pub fn new_file(&mut self, file: &str) {
        self.new_file_with(file, "");
    }

    /// A new file in the open project holding `text` (a paste or a
    /// duplicate), written only if no file of that name exists.
    pub fn new_file_with(&mut self, file: &str, text: &str) {
        let Some(o) = &self.open else { return };
        let project = o.name.clone();
        self.queue.push(PReq::json(
            PCall::NewFile {
                project: project.clone(),
                file: file.into(),
            },
            "PUT",
            file_url(&project, file),
            &json!({ "text": text, "version": null }),
        ));
    }

    /// Renames a file of the open project: written under the new name
    /// (never over another file), then the old one deleted.
    pub fn rename_file(&mut self, from: &str, to: &str) {
        let Some(o) = &self.open else { return };
        let Some(b) = o.bufs.get(from) else { return };
        let project = o.name.clone();
        let text = b.text.clone();
        self.queue.push(PReq::json(
            PCall::MoveFile {
                project: project.clone(),
                from: from.into(),
                to: to.into(),
            },
            "PUT",
            file_url(&project, to),
            &json!({ "text": text, "version": null }),
        ));
    }

    pub fn delete_file(&mut self, file: &str) {
        let Some(o) = &self.open else { return };
        let project = o.name.clone();
        self.queue.push(PReq::delete(
            PCall::DeleteFile {
                project: project.clone(),
                file: file.into(),
            },
            file_url(&project, file),
        ));
    }

    /// Saves every edited file, each with the version it was read as.
    pub fn save(&mut self) {
        let Some(o) = &mut self.open else { return };
        let project = o.name.clone();
        let mut reqs = Vec::new();
        for (name, b) in &mut o.bufs {
            if !b.dirty() || b.saving.is_some() {
                continue;
            }
            b.saving = Some(b.text.clone());
            reqs.push(PReq::json(
                PCall::Save {
                    project: project.clone(),
                    file: name.clone(),
                },
                "PUT",
                file_url(&project, name),
                &json!({ "text": b.text, "version": b.version }),
            ));
        }
        self.conflict = None;
        self.notice = None;
        self.queue.extend(reqs);
    }

    /// Reads `file` again from the box, dropping the edits here (after a
    /// 409: someone else saved it).
    pub fn reload(&mut self, file: &str) {
        let Some(o) = &self.open else { return };
        let project = o.name.clone();
        self.conflict = None;
        self.queue.push(PReq::get(
            PCall::File {
                project: project.clone(),
                file: file.into(),
            },
            file_url(&project, file),
        ));
    }

    /// Saves what is edited, then checks, replays if the policy wants it,
    /// and deploys the open project.
    pub fn deploy(&mut self) {
        if self.deploy.busy() || self.open.is_none() {
            return;
        }
        self.diags.clear();
        if self.dirty() {
            self.deploy = Deploy::Saving;
            self.save();
        } else {
            self.check();
        }
    }

    fn check(&mut self) {
        let Some(o) = &self.open else { return };
        let name = o.name.clone();
        self.deploy = Deploy::Checking;
        self.queue.push(PReq::json(
            PCall::Check(name.clone()),
            "POST",
            format!("{}/check", project_url(&name)),
            &json!({}),
        ));
    }

    fn replay(&mut self, name: &str) {
        self.deploy = Deploy::Replaying;
        self.queue.push(PReq::json(
            PCall::Replay(name.into()),
            "POST",
            format!("{}/replay", project_url(name)),
            &json!({}),
        ));
    }

    fn deploy_now(&mut self, name: &str, replay: Option<u64>) {
        self.deploy = Deploy::Deploying;
        let body = replay.map_or_else(|| json!({}), |n| json!({ "replay": n }));
        self.queue.push(PReq::json(
            PCall::Deploy {
                project: name.into(),
                replay,
            },
            "POST",
            format!("{}/deploy", project_url(name)),
            &body,
        ));
    }

    /// A file the user picked: a `.zip` becomes the project its name
    /// says; anything else is imported into the open project (the box
    /// takes `.st` and `.xml`, and says why it refuses the rest).
    pub fn upload(&mut self, file: &str, bytes: Vec<u8>, replace: bool) {
        let r = if replace { "?replace=true" } else { "" };
        if has_ext(file, "zip") {
            let project = crate::projects::slug(file);
            self.pending = Some(Pending {
                file: file.into(),
                bytes: bytes.clone(),
                project: project.clone(),
            });
            self.queue.push(PReq {
                call: PCall::Upload(project.clone()),
                method: "POST",
                url: format!("{}/zip{r}", project_url(&project)),
                body: Some(bytes),
                ctype: "application/zip",
            });
            return;
        }
        let Some(o) = &self.open else {
            self.note(format!("Open a project to upload {file} into it."), true);
            return;
        };
        let project = o.name.clone();
        self.pending = Some(Pending {
            file: file.into(),
            bytes: bytes.clone(),
            project: project.clone(),
        });
        let sep = if replace { "&replace=true" } else { "" };
        self.queue.push(PReq {
            call: PCall::Import {
                project: project.clone(),
                file: file.into(),
            },
            method: "POST",
            url: format!("{}/import?file={}{sep}", project_url(&project), segment(file)),
            body: Some(bytes),
            ctype: "application/octet-stream",
        });
    }

    /// Sends the upload the box refused as taken again, replacing.
    pub fn replace(&mut self) {
        if let Some(p) = self.pending.take() {
            self.upload(&p.file, p.bytes, true);
        }
    }

    /// The open project as a `.zip`, through the browser.
    pub fn download(&mut self) {
        if let Some(o) = &self.open {
            self.downloads.push(zip_url(&o.name));
        }
    }

    /// Forces input signal `signal` to `value`.
    pub fn force(&mut self, signal: &str, value: &Json) {
        self.queue.push(PReq::json(
            PCall::Force,
            "POST",
            "/api/forces".into(),
            &json!({ "signal": signal, "value": value }),
        ));
    }

    /// The running project's shared layout, if it is read (`""`: none).
    #[must_use]
    pub fn shared_layout(&self) -> &str {
        match (&self.layout, &self.running) {
            (Some(l), Some(r)) if l.project == *r => &l.text,
            _ => "",
        }
    }

    /// Saves `text` as the running project's shared diagram layout (an
    /// engineer's; the box refuses anyone else), over the version read.
    pub fn save_layout(&mut self, text: String) {
        let Some(p) = self.running.clone() else {
            self.note(
                "Nothing deployed from a project runs: its default layout is kept with it.",
                true,
            );
            return;
        };
        if self.saving_layout.is_some() {
            return;
        }
        let version = self
            .layout
            .as_ref()
            .filter(|l| l.project == p)
            .and_then(|l| l.version.clone());
        self.queue.push(PReq::json(
            PCall::SaveLayout(p.clone()),
            "PUT",
            file_url(&p, helmsman_project::LAYOUT),
            &json!({ "text": text, "version": version }),
        ));
        self.saving_layout = Some(text);
    }

    fn read_layout(&mut self, p: &str) {
        self.queue.push(PReq::get(
            PCall::Layout(p.to_string()),
            file_url(p, helmsman_project::LAYOUT),
        ));
    }

    /// Releases force `id`, or every force.
    pub fn release(&mut self, id: Option<u64>) {
        let url = id.map_or_else(|| "/api/forces".to_string(), |id| format!("/api/forces/{id}"));
        self.queue.push(PReq::delete(PCall::Release, url));
    }

    /// Takes in an answer.
    #[allow(clippy::too_many_lines)]
    pub fn answer(&mut self, call: PCall, a: &Answer) {
        let ok = (200..300).contains(&a.status);
        let j: Json = serde_json::from_str(&a.body).unwrap_or(Json::Null);
        if a.status == 401 {
            self.ended = true;
            self.saving_layout = None;
            if self.deploy.busy() {
                self.deploy = Deploy::Failed("signed out: sign in again to deploy".into());
            }
            if let Some(o) = &mut self.open {
                for b in o.bufs.values_mut() {
                    b.saving = None;
                }
            }
            self.note(
                "The box no longer knows this page's session (signed out, expired, or the box restarted). Sign in again; nothing was changed.",
                true,
            );
            return;
        }
        match call {
            PCall::List => {
                if a.status == 404 {
                    self.available = Some(false);
                    return;
                }
                if !ok {
                    self.note(format!("cannot read the projects: {}", error_of(a)), true);
                    return;
                }
                self.available = Some(true);
                let current = j["running"]["current"].as_bool() == Some(true);
                self.running = j["running"]["project"]
                    .as_str()
                    .filter(|_| current)
                    .map(str::to_string);
                self.projects = j["projects"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| {
                                let name = p["name"].as_str()?.to_string();
                                Some(ProjectRow {
                                    running: self.running.as_deref() == Some(name.as_str()),
                                    name,
                                    group: Group::Mine,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match self.running.clone() {
                    Some(r) => self.read_layout(&r),
                    None => self.layout = None,
                }
                let keep = self
                    .open
                    .as_ref()
                    .is_some_and(|o| self.projects.iter().any(|p| p.name == o.name));
                if !keep {
                    let first = self
                        .running
                        .clone()
                        .or_else(|| self.projects.first().map(|p| p.name.clone()));
                    match first {
                        Some(n) => self.open_project(&n),
                        None => self.open = None,
                    }
                }
            }
            PCall::Project(name) => {
                let Some(o) = self.open.as_mut().filter(|o| o.name == name) else {
                    return;
                };
                if !ok {
                    let e = error_of(a);
                    self.note(format!("cannot open {name}: {e}"), true);
                    return;
                }
                let files: Vec<(String, Option<String>)> = j["files"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|f| {
                                Some((
                                    f["name"].as_str()?.to_string(),
                                    f["version"].as_str().map(str::to_string),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Files gone from the box go, unless edited here.
                o.bufs
                    .retain(|n, b| b.dirty() || files.iter().any(|(f, _)| f == n));
                let mut reads = Vec::new();
                for (f, version) in files {
                    let b = o.bufs.entry(f.clone()).or_default();
                    if !b.loaded || (b.version != version && !b.dirty()) {
                        reads.push(PReq::get(
                            PCall::File {
                                project: name.clone(),
                                file: f.clone(),
                            },
                            file_url(&name, &f),
                        ));
                    }
                }
                if o.current.as_ref().is_none_or(|c| !o.bufs.contains_key(c)) {
                    // main.st, else the first program file, else the first file.
                    o.current = o
                        .bufs
                        .keys()
                        .find(|f| *f == "main.st")
                        .or_else(|| o.bufs.keys().find(|f| has_ext(f, "st")))
                        .or_else(|| o.bufs.keys().next())
                        .cloned();
                }
                self.queue.extend(reads);
            }
            PCall::File { project, file } => {
                let Some(o) = self.open.as_mut().filter(|o| o.name == project) else {
                    return;
                };
                if !ok {
                    self.note(format!("cannot read {file}: {}", error_of(a)), true);
                    return;
                }
                let text = j["text"].as_str().unwrap_or_default().to_string();
                let b = o.bufs.entry(file).or_default();
                b.text.clone_from(&text);
                b.base = text;
                b.version = j["version"].as_str().map(str::to_string);
                b.loaded = true;
            }
            PCall::Create(name) => {
                if ok {
                    self.note(format!("Made project {name}."), false);
                    self.open_project(&name);
                    self.relist();
                } else {
                    self.note(format!("{name} was not made: {}", error_of(a)), true);
                }
            }
            PCall::Rename { from, to } => {
                if ok {
                    if self.open.as_ref().is_some_and(|o| o.name == from)
                        && let Some(o) = &mut self.open
                    {
                        o.name.clone_from(&to);
                    }
                    self.note(format!("Renamed {from} to {to}."), false);
                    self.relist();
                } else {
                    self.note(format!("{from} was not renamed: {}", error_of(a)), true);
                }
            }
            PCall::Delete(name) => {
                if ok {
                    if self.open.as_ref().is_some_and(|o| o.name == name) {
                        self.open = None;
                    }
                    self.note(format!("Deleted {name}."), false);
                    self.relist();
                } else {
                    self.note(format!("{name} was not deleted: {}", error_of(a)), true);
                }
            }
            PCall::Save { project, file } => self.saved(&project, &file, a, &j),
            PCall::NewFile { project, file } => {
                if ok {
                    if let Some(o) = self.open.as_mut().filter(|o| o.name == project) {
                        o.bufs.insert(
                            file.clone(),
                            Buf {
                                version: j["version"].as_str().map(str::to_string),
                                loaded: true,
                                ..Buf::default()
                            },
                        );
                        o.current = Some(file.clone());
                    }
                    self.note(format!("Made {file}."), false);
                } else {
                    self.note(format!("{file} was not made: {}", error_of(a)), true);
                }
            }
            PCall::MoveFile { project, from, to } => {
                if !ok {
                    self.note(format!("{from} was not renamed: {}", error_of(a)), true);
                    return;
                }
                if let Some(o) = self.open.as_mut().filter(|o| o.name == project) {
                    let mut b = o.bufs.remove(&from).unwrap_or_default();
                    let sent = b.text.clone();
                    b.base = sent;
                    b.version = j["version"].as_str().map(str::to_string);
                    b.loaded = true;
                    o.bufs.insert(to.clone(), b);
                    if o.current.as_deref() == Some(from.as_str()) {
                        o.current = Some(to.clone());
                    }
                }
                self.queue.push(PReq::delete(
                    PCall::DeleteFile {
                        project: project.clone(),
                        file: from.clone(),
                    },
                    file_url(&project, &from),
                ));
                self.note(format!("Renamed {from} to {to}."), false);
            }
            PCall::DeleteFile { project, file } => {
                if ok || a.status == 404 {
                    if let Some(o) = self.open.as_mut().filter(|o| o.name == project) {
                        o.bufs.remove(&file);
                        if o.current.as_deref() == Some(file.as_str()) {
                            o.current = o.bufs.keys().next().cloned();
                        }
                    }
                } else {
                    self.note(format!("{file} was not deleted: {}", error_of(a)), true);
                }
            }
            PCall::Check(name) => {
                if a.status == 422 {
                    self.diags = diags_of(&j);
                    self.deploy = Deploy::Failed(format!(
                        "{name} does not build: {} problem(s); click one to go there",
                        self.diags.len()
                    ));
                    return;
                }
                if !ok {
                    self.deploy = Deploy::Failed(error_of(a));
                    return;
                }
                self.modules = j["modules"]
                    .as_array()
                    .map(|m| {
                        m.iter()
                            .filter_map(|m| m["name"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if j["approval"]["needs"]["replay"].as_bool() == Some(true) {
                    self.replay(&name);
                } else {
                    self.deploy_now(&name, None);
                }
            }
            PCall::Replay(name) => match j["replay"].as_u64().filter(|_| a.status == 201) {
                Some(n) => self.deploy_now(&name, Some(n)),
                None => {
                    self.deploy = Deploy::Failed(format!("the sim replay did not run: {}", error_of(a)));
                }
            },
            PCall::Deploy { project, replay } => {
                if ok {
                    let snap = j["snapshot"].as_str().unwrap_or("?");
                    let mods = if self.modules.is_empty() {
                        "no programs".to_string()
                    } else {
                        self.modules.join(", ")
                    };
                    self.deploy =
                        Deploy::Done(format!("{project} is running: {mods} (snapshot {})", short(snap)));
                    self.running = Some(project);
                    self.deployed = true;
                    self.relist();
                    self.queue.push(PReq::get(PCall::Forces, "/api/forces".into()));
                } else if a.status == 428 && replay.is_none() {
                    // The policy wants a replay the check did not say.
                    self.replay(&project);
                } else if a.status == 422 {
                    self.diags = diags_of(&j);
                    self.deploy = Deploy::Failed(format!("{project} was not deployed: {}", error_of(a)));
                } else {
                    self.deploy = Deploy::Failed(format!("{project} was not deployed: {}", error_of(a)));
                }
            }
            PCall::Upload(project) => {
                if ok {
                    self.pending = None;
                    self.note(format!("Uploaded {project}."), false);
                    self.open_project(&project);
                    self.relist();
                } else if a.status == 409 {
                    self.note(
                        format!("A project named {project} is already on the box. Replace it?"),
                        true,
                    );
                } else {
                    self.pending = None;
                    self.note(format!("Upload refused: {}", error_of(a)), true);
                }
            }
            PCall::Import { project, file } => {
                if ok {
                    self.pending = None;
                    let name = j["name"].as_str().unwrap_or(&file).to_string();
                    self.note(format!("Imported {file} into {project} as {name}."), false);
                    if self.open.as_ref().is_some_and(|o| o.name == project) {
                        if let Some(o) = &mut self.open {
                            if let Some(b) = o.bufs.get_mut(&name) {
                                // Read again below, whatever it held.
                                b.loaded = false;
                            }
                            o.current = Some(name);
                        }
                        self.queue
                            .push(PReq::get(PCall::Project(project.clone()), project_url(&project)));
                    }
                } else if a.status == 409 {
                    self.note(format!("{project} already has that file. Replace it?"), true);
                } else {
                    self.pending = None;
                    self.note(format!("{file} was refused: {}", error_of(a)), true);
                }
            }
            PCall::Forces => {
                if ok {
                    self.forces = j["forces"]
                        .as_array()
                        .map(|a| a.iter().filter_map(ForceRow::parse).collect());
                }
            }
            PCall::Force | PCall::Release => {
                if !ok {
                    self.note(error_of(a), true);
                }
                self.queue.push(PReq::get(PCall::Forces, "/api/forces".into()));
            }
            // The Trend tab's (`box_trend.rs`).
            PCall::TrendPins | PCall::SavePins | PCall::Trend => {}
            PCall::Layout(p) => {
                if self.running.as_deref() != Some(p.as_str()) {
                    return;
                }
                let text = j["text"].as_str().filter(|_| ok);
                if ok || a.status == 404 {
                    self.layout = Some(SharedLayout {
                        project: p,
                        text: text.unwrap_or_default().to_string(),
                        version: j["version"].as_str().filter(|_| ok).map(str::to_string),
                    });
                }
            }
            PCall::SaveLayout(p) => self.layout_answer(&p, a, &j),
        }
    }

    fn layout_answer(&mut self, p: &str, a: &Answer, j: &Json) {
        let sent = self.saving_layout.take().unwrap_or_default();
        if !(200..300).contains(&a.status) {
            if a.status == 409 {
                self.note(
                    format!("Someone else saved {p}'s default layout since this page read it: it was read again; save again to replace it."),
                    true,
                );
                self.read_layout(p);
            } else {
                self.note(format!("The default layout was not saved: {}", error_of(a)), true);
            }
            return;
        }
        let version = j["version"].as_str().map(str::to_string);
        if let Some(o) = self.open.as_mut().filter(|o| o.name == p) {
            let b = o.bufs.entry(helmsman_project::LAYOUT.to_string()).or_default();
            if !b.dirty() {
                b.text.clone_from(&sent);
                b.base.clone_from(&sent);
                b.version.clone_from(&version);
                b.loaded = true;
            }
        }
        self.layout = Some(SharedLayout {
            project: p.to_string(),
            text: sent.clone(),
            version,
        });
        self.layout_saved = Some(sent);
        self.note(format!("Saved this arrangement as {p}'s default layout."), false);
    }

    fn saved(&mut self, project: &str, file: &str, a: &Answer, j: &Json) {
        let ok = (200..300).contains(&a.status);
        let Some(o) = self.open.as_mut().filter(|o| o.name == project) else {
            return;
        };
        let Some(b) = o.bufs.get_mut(file) else { return };
        let sent = b.saving.take();
        if ok {
            if let Some(s) = sent {
                b.base = s;
            }
            b.version = j["version"].as_str().map(str::to_string);
        } else if a.status == 409 {
            self.conflict = Some(file.into());
        }
        let still = o.bufs.values().any(|b| b.saving.is_some());
        if !ok {
            let why = if a.status == 409 {
                format!(
                    "Someone else saved {file} since this page read it. Reload it (your edits to it here are dropped), or copy your edits out first."
                )
            } else {
                format!("{file} was not saved: {}", error_of(a))
            };
            self.note(why, true);
            if self.deploy == Deploy::Saving {
                self.deploy = Deploy::Failed("not deployed: a file was not saved".into());
            }
            return;
        }
        if still {
            return;
        }
        if self.deploy == Deploy::Saving {
            self.check();
        } else if !matches!(&self.notice, Some((_, true))) {
            self.note(format!("Saved {project}."), false);
        }
    }
}
