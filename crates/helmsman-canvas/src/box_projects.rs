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

use crate::projects::{Diag, Group, ProjectRow, Severity};
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
    /// A file made: the text written to it, which the editor then holds.
    NewFile {
        project: String,
        file: String,
        text: String,
    },
    /// The first half of a file rename: the new name written with `text`.
    MoveFile {
        project: String,
        from: String,
        to: String,
        text: String,
    },
    /// `moved_to`: the second half of a rename, which wrote that name.
    DeleteFile {
        project: String,
        file: String,
        moved_to: Option<String>,
    },
    Check(String),
    /// `POST /api/projects/{p}/tests`: its requirement tests, run on the box.
    Tests(String),
    Replay(String),
    /// `GET .../history`: the project's revisions; `seq` numbers the asks,
    /// so an older list read late does not replace a newer one.
    History {
        project: String,
        seq: u64,
    },
    /// `GET .../history/{n}`: one revision, whole.
    Revision {
        project: String,
        n: u32,
    },
    /// `GET .../history/{n}` for its Agent page alone (the Agent tab's
    /// earlier instructions, `crate::instruct::recent`).
    Page {
        project: String,
        n: u32,
    },
    /// `POST .../history/{n}/restore`.
    Restore {
        project: String,
        n: u32,
    },
    Deploy {
        project: String,
        replay: Option<u64>,
    },
    Upload(String),
    /// `PUT /api/modules/{name}`: the edited source of a module, on a box
    /// or Skipper without projects.
    DeployModule(String),
    /// A step of sending a project's I/O list to the watched Skipper
    /// before its program: check the devices, read the result, save.
    NodeMapSend(MapStep),
    Import {
        project: String,
        file: String,
    },
    Forces,
    Force,
    Release,
    /// `POST /api/modules/{name}/stop|run|restart` (card
    /// monitor-box-run-stop): the module and the control.
    Control(String, &'static str),
    /// The signed-in person's trend pins (`box_trend.rs`), read.
    TrendPins,
    /// ... and saved.
    SavePins,
    /// The pins' history.
    Trend,
    /// The signed-in person's desk on the box (`crate::desk::OnBox`), read.
    Desk,
    /// ... and saved.
    SaveDesk,
    /// The running project's shared diagram layout, read.
    Layout(String),
    /// ... and saved.
    SaveLayout(String),
    /// `POST /api/devices/discover`: the Devices tab's Scan (answered by
    /// the view, not here).
    Discover,
    /// `POST /api/devices/discover?address=A`: the Devices tab's Ask, one
    /// address (the text is the validated IPv4 address, for the answer's
    /// note).
    Ask(String),
    /// `GET /api/devices/library?vendor=&product=`: a found device's
    /// profile for the quick form (answered by the view, not here).
    Library,
    /// `GET /api/devices/library?wired=VENDOR/MODEL`: what the library
    /// knows of the sensor or actuator a point is wired to (answered by
    /// the view, not here).
    Wired(String),
    /// `GET /api/devices/library?q=`: the picker's search by the number
    /// on a label (answered by the view, not here).
    FindWired,
    /// A device's own configuration (`/api/devices/{id}/config...`),
    /// answered by the Devices tab (`device_config.rs`).
    Config(String, crate::device_config::ConfigOp),
    /// `GET /api/device-profile/proposal`: the profile the agent proposed
    /// (answered by the Devices tab, `device_proposal.rs`).
    Proposal,
    /// `POST /api/device-profile/proposal/accept`: save it into a project.
    ProposalAccept,
    /// `DELETE /api/device-profile/proposal`: throw it away.
    ProposalDiscard,
    /// `POST /api/device-profile/publish`: send the saved profile to the
    /// device library for review (answered by the Devices tab).
    ProposalPublish,
    /// `POST /api/dropins`: the vendor's file for the agent
    /// (answered by the Devices tab, `device_vendor.rs`).
    VendorFile,
    /// `POST /api/dropins`: a PDF of a specification (the name it was
    /// sent under); answered by the view (`box_spec.rs`).
    SpecPdf(String),
    /// `POST /api/projects/{name}/zip`: the project a spec makes, whole;
    /// answered by the view (`box_spec.rs`).
    SpecProject(String),
    /// `POST /api/projects/{name}/zip`: the open project with the kept
    /// design in it, as a project of its own; answered by the view
    /// (`box_view.rs`).
    DesignProject(String),
    /// `GET /api/pane-layouts`: the pane layouts the box keeps (answered
    /// by `pane_layout::OnBox`).
    PaneLayouts,
    /// `PUT /api/pane-layouts`: one of them kept, by name.
    KeepPaneLayout(String),
    /// `DELETE /api/pane-layouts/{name}`.
    DropPaneLayout(String),
    /// A Skipper's map: its check, the reading of it and the download
    /// (answered by the Devices tab, `node_map.rs`).
    NodeMap(crate::node_map::NodeOp),
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

/// The most Agent pages kept for the open project: as many revisions as a
/// box keeps of one (`MAX_REVISIONS` in helmsman-plc's project history),
/// so the whole of what the box has can go with an instruction.
pub const PAGES: usize = 200;

impl PCall {
    /// Whether the call is the watched node's (`?box=`): a module deploy
    /// and the map window's check and download. The rest are the serving
    /// box's.
    #[must_use]
    pub const fn on_node(&self) -> bool {
        matches!(
            self,
            Self::DeployModule(_) | Self::NodeMap(_) | Self::Control(..) | Self::NodeMapSend(_)
        )
    }
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

    pub(crate) fn delete(call: PCall, url: String) -> Self {
        Self {
            call,
            method: "DELETE",
            url,
            body: None,
            ctype: JSON,
        }
    }
}

/// A revision as the box lists it.
fn revision_row(j: &Json) -> Option<crate::history_ui::RevisionRow> {
    Some(crate::history_ui::RevisionRow {
        about: None,
        n: u32::try_from(j["n"].as_u64()?).ok()?,
        at_ms: j["at_ms"].as_u64().unwrap_or(0),
        by: j["by"].as_str().unwrap_or("?").to_string(),
        what: j["what"].as_str().unwrap_or("").to_string(),
        page: j["page"].as_bool() == Some(true) || j["page"].is_string(),
    })
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
    /// How the box runs this project's requirement tests (its image sizes
    /// and scan period), as it reported them when the project was read.
    pub runs: Option<Runs>,
}

/// How a box runs requirement tests: what a change is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runs {
    pub sizes: helmsman_iec::ImageSizes,
    pub period: std::time::Duration,
}

impl Runs {
    /// From the `tests` object of `GET /api/projects/{name}`.
    fn parse(j: &Json) -> Option<Self> {
        let n = |k: &str| j[k].as_u64().and_then(|n| usize::try_from(n).ok());
        Some(Self {
            sizes: helmsman_iec::ImageSizes {
                i: n("i")?,
                q: n("q")?,
                m: n("m")?,
            },
            period: std::time::Duration::from_millis(j["scan_ms"].as_u64().filter(|ms| *ms > 0)?),
        })
    }
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
    /// The note on show offers to replace it (the box said the name is
    /// taken); false while the upload is on its way.
    pub offered: bool,
}

/// Everything the Editor knows about the box's projects.
#[derive(Debug, Default)]
// Each flag is its own fact (available, ended, deployed, to the node).
#[allow(clippy::struct_excessive_bools)]
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
    /// The open project's revisions, oldest first, as last read.
    pub history: Vec<crate::history_ui::RevisionRow>,
    /// The revision on view (the History modal).
    pub revision: Option<crate::history_ui::RevisionView>,
    /// The Agent pages of the open project's latest revisions that carry
    /// one, by revision, as read (`PCall::Page`).
    pub pages: BTreeMap<u32, String>,
    /// The running project's shared diagram layout, once read.
    pub layout: Option<SharedLayout>,
    /// A layout this page saved as the default, for the view to adopt.
    pub layout_saved: Option<String>,
    /// The layout being saved.
    saving_layout: Option<String>,
    /// The running module being edited, on a box or Skipper without
    /// projects (`PUT /api/modules/{name}`).
    pub module: Option<ModuleEdit>,
    /// The watched Skipper's I/O list as the page last read it.
    pub node_map: Option<String>,
    map_send: Option<MapSend>,
    /// Page time as of the last [`Self::tick`].
    now: f64,
    /// Deploy sends the checked project to the watched node (a page a box
    /// served, watching a Skipper with `?box=`) instead of running it on
    /// the box.
    pub to_node: bool,
    /// The modules the last check said the project builds.
    modules: Vec<String>,
    /// How the box would check a project made now (its image sizes and
    /// scan period), from the project list: what a spec run is checked
    /// against before there is a project to ask.
    pub new_runs: Option<Runs>,
    /// The open project's requirement tests as the box last ran them
    /// (`crate::requirements`): asked when the project opens, after each
    /// save, on Run, and reported by a refused deploy.
    pub tests: crate::requirements::Results,
    /// A test run is on its way.
    pub testing: bool,
    /// What the desk (`crate::desk`) had from before a reload, put back
    /// as the box's answers come in: the project to open, the file to
    /// show, and the unsaved edits, each used once.
    pub restore: Restore,
    /// History asks made, and the number of the last answer applied.
    history_asked: u64,
    history_applied: u64,
    queue: Vec<PReq>,
}

/// The desk's part in [`BoxProjects`]: taken as it is used.
#[derive(Debug, Default)]
pub struct Restore {
    pub project: Option<String>,
    /// The file to show, with its project.
    pub file: Option<(String, String)>,
    /// Project, then file, then the edit.
    pub drafts: BTreeMap<String, BTreeMap<String, crate::desk::Draft>>,
}

pub(crate) fn error_of(a: &Answer) -> String {
    serde_json::from_str::<Json>(&a.body)
        .ok()
        .and_then(|j| j["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| match a.status {
            0 => format!("the box did not answer: {}", a.body),
            s => format!("HTTP {s}"),
        })
}

/// What a box answered with that is wrong or worth a look: its
/// `diagnostics` (errors), then its `warnings` (never blocking). A box
/// from before warnings has no `warnings` field: then there are none. An
/// entry's own `severity`, `id` and `blocking` are kept when it has them.
pub(crate) fn diags_of(j: &Json) -> Vec<Diag> {
    let list = |key: &str, default: Severity| -> Vec<Diag> {
        j[key]
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
                        severity: match d["severity"].as_str() {
                            Some("warning") => Severity::Warning,
                            Some("error") => Severity::Error,
                            _ => default,
                        },
                        id: d["id"].as_str().map(str::to_string),
                        blocking: d["blocking"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut all = list("diagnostics", Severity::Error);
    all.extend(list("warnings", Severity::Warning));
    all
}

/// Whether `file` ends in `.ext`, in any case.
fn has_ext(file: &str, ext: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// The ports a box's refusal says have no signal (`port LEVEL: no signal of
/// that name`, `BindError::UnknownSignal` on the box), lower case as
/// `io.map` names them, each once.
fn unbound_ports(text: &str) -> Vec<String> {
    const SAYS: &str = ": no signal of that name";
    let mut out: Vec<String> = Vec::new();
    for (at, _) in text.match_indices(SAYS) {
        let before = &text[..at];
        let name: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let named_a_port = before[..before.len() - name.len()].ends_with("port ");
        let name = name.to_ascii_lowercase();
        if named_a_port && !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// `why`, a box's refusal of a deploy, with what to do about an input that
/// has no signal added: the box's check is as it was, only the words
/// after it say how to go on.
pub(crate) fn deploy_words(why: String) -> String {
    let ports = unbound_ports(&why);
    if ports.is_empty() {
        return why;
    }
    let (names, verb) = (ports.join(", "), if ports.len() == 1 { "has" } else { "have" });
    format!(
        "{why}. Input {names} {verb} no signal on this box: assign each to a device channel (Assign to device I/O in the I/O tab), then deploy again."
    )
}

fn short(s: &str) -> &str {
    s.get(..8).unwrap_or(s)
}

/// The steps of sending an I/O list to a Skipper ([`PCall::NodeMapSend`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapStep {
    /// `POST /api/io_map/check`: the Skipper asks every device.
    Check,
    /// `GET /api/io_map/check`: how the check went.
    Poll,
    /// `PUT /api/io_map`: saved; the Skipper restarts on it.
    Save,
}

/// A project's I/O list on its way to the Skipper, its program after it.
#[derive(Debug, Clone, PartialEq)]
struct MapSend {
    text: String,
    /// When to read the check's result again (page time, s).
    poll_at: Option<f64>,
    /// When to send the program, once the Skipper restarted on the map.
    program_at: Option<f64>,
    /// Reads of the check so far, and sends of the program after a save.
    polls: u32,
    program_tries: u32,
}

/// Seconds a Skipper takes to restart on a saved map before its program
/// is sent (it answered again within 8 s on the bench, 2026-11-24).
const MAP_RESTART_S: f64 = 8.0;
/// Reads of a check's result before giving up (one a second).
const MAP_POLLS: u32 = 60;

/// What a node is sent for a project: every ST file but the requirement
/// tests (`test_*.st`), in name order, one after another.
#[must_use]
pub fn node_source(o: &OpenProject) -> String {
    let mut out = String::new();
    for (name, b) in &o.bufs {
        let st = std::path::Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("st"));
        if !st || name.to_ascii_lowercase().starts_with("test_") {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&b.text);
    }
    out
}

/// A running module's source being edited where there are no projects:
/// a Skipper, or a box run without `--projects`. The box's copy is what it
/// last said it runs; the edits stay until a deploy of them succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEdit {
    pub name: String,
    /// What the box runs, as far as this page knows: what it sent, or what
    /// a deploy from here put there.
    pub base: String,
    /// The editor's text.
    pub text: String,
    /// The box's source as last read, so a re-read that brings nothing new
    /// never undoes a deploy made from here.
    seen: String,
    /// The text on its way in a deploy.
    sent: Option<String>,
}

impl ModuleEdit {
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.text != self.base
    }
}

impl BoxProjects {
    /// Keeps the module editor on `name` with the box's `source`: another
    /// module, or a source changed on the box, replaces the text unless it
    /// holds edits.
    pub fn track_module(&mut self, name: &str, source: &str) {
        match &mut self.module {
            Some(m) if m.name == name => {
                if m.seen != source {
                    m.seen = source.to_string();
                    if !m.dirty() {
                        m.text = source.to_string();
                    }
                    m.base = source.to_string();
                }
            }
            _ => {
                self.module = Some(ModuleEdit {
                    name: name.to_string(),
                    base: source.to_string(),
                    text: source.to_string(),
                    seen: source.to_string(),
                    sent: None,
                });
            }
        }
    }

    /// The checked project, to the watched node: what the box's lint holds
    /// stops it here, a project of more than one program too (a Skipper
    /// runs one); a sim replay comes first when the box's policy asks one
    /// for the change (Nick, 2026-11-24: "require the replay for Skipper
    /// deploys"), as it would before running it on the box.
    fn after_check_for_node(&mut self, project: &str, check: &Json) {
        let held: Vec<&str> = check["held"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .collect();
        if !held.is_empty() {
            self.deploy = Deploy::Failed(format!("{project} was not sent: {}", held.join("; ")));
            return;
        }
        if self.modules.len() != 1 {
            self.deploy = Deploy::Failed(format!(
                "A Skipper runs one program; {project} builds {}: {}.",
                self.modules.len(),
                self.modules.join(", ")
            ));
            return;
        }
        if check["approval"]["needs"]["replay"].as_bool() == Some(true) {
            self.replay(project);
        } else {
            self.send_to_node();
        }
    }

    /// The open project's program, its function blocks with it, to the
    /// watched node under the program's name.
    fn send_to_node(&mut self) {
        let Some(module) = self.modules.first().cloned() else {
            return;
        };
        let Some(o) = &self.open else { return };
        let source = node_source(o);
        self.module = Some(ModuleEdit {
            name: module,
            base: String::new(),
            text: source,
            seen: String::new(),
            sent: None,
        });
        // The project's I/O list first, when the Skipper runs another one
        // (Nick, 2026-11-24: an I/O change the Agent made never reached
        // the Skipper): its devices are checked from the Skipper, it is
        // saved, the Skipper restarts on it, then the program goes.
        let map = self
            .open
            .as_ref()
            .and_then(|o| o.bufs.get(helmsman_project::IO_MAP))
            .map(|b| b.text.clone())
            .filter(|m| !m.trim().is_empty());
        if let Some(map) = map.filter(|m| self.node_map.as_deref().is_none_or(|n| n.trim() != m.trim())) {
            self.deploy = Deploy::Deploying;
            self.queue.push(PReq::json(
                PCall::NodeMapSend(MapStep::Check),
                "POST",
                "/api/io_map/check".into(),
                &json!({ "text": map }),
            ));
            self.map_send = Some(MapSend {
                text: map,
                poll_at: None,
                program_at: None,
                polls: 0,
                program_tries: 0,
            });
            return;
        }
        // The check (and replay) are over; the send is the deploy's last step.
        self.deploy = Deploy::Idle;
        self.deploy_module();
    }

    /// Page time `now`: reads a check's result and sends the program
    /// after a map save when they are due.
    pub fn tick(&mut self, now: f64) {
        self.now = now;
        let Some(m) = &mut self.map_send else { return };
        if m.poll_at.is_some_and(|t| now >= t) {
            m.poll_at = None;
            m.polls += 1;
            self.queue.push(PReq::get(
                PCall::NodeMapSend(MapStep::Poll),
                "/api/io_map/check".into(),
            ));
        }
        if m.program_at.is_some_and(|t| now >= t) {
            m.program_at = None;
            m.program_tries += 1;
            self.deploy = Deploy::Idle;
            self.deploy_module();
        }
    }

    fn map_failed(&mut self, why: String) {
        self.map_send = None;
        if let Some(m) = &mut self.module {
            m.sent = None;
        }
        self.deploy = Deploy::Failed(why);
    }

    fn map_answer(&mut self, step: MapStep, a: &Answer, j: &Json) {
        let ok = (200..300).contains(&a.status);
        let now = self.now;
        let Some(m) = self.map_send.as_mut() else { return };
        match step {
            MapStep::Check if ok || a.status == 409 => m.poll_at = Some(now + 1.0),
            MapStep::Check => {
                self.map_failed(format!(
                    "The Skipper would not check the I/O list: {}",
                    error_of(a)
                ));
            }
            MapStep::Poll if !ok || j["state"].as_str() != Some("done") => {
                if m.polls >= MAP_POLLS {
                    self.map_failed("The Skipper's check of the devices did not finish.".into());
                } else {
                    m.poll_at = Some(now + 1.0);
                }
            }
            MapStep::Poll => {
                let same = j["text"].as_str().is_some_and(|t| t.trim() == m.text.trim());
                if !same {
                    self.map_failed(
                        "Another I/O list was being checked on the Skipper: deploy again.".into(),
                    );
                } else if j["may_save"].as_bool() == Some(true) {
                    let text = m.text.clone();
                    self.queue.push(PReq::json(
                        PCall::NodeMapSend(MapStep::Save),
                        "PUT",
                        "/api/io_map".into(),
                        &json!({ "text": text }),
                    ));
                } else {
                    let silent: Vec<String> = j["devices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|d| d["state"].as_str() != Some("answered"))
                        .map(|d| {
                            format!(
                                "{} ({}): {}",
                                d["name"].as_str().unwrap_or("?"),
                                d["address"].as_str().unwrap_or("?"),
                                d["error"].as_str().unwrap_or("did not answer")
                            )
                        })
                        .collect();
                    self.map_failed(format!(
                        "Not deployed: the Skipper could not reach {}. Its I/O list and program are unchanged.",
                        if silent.is_empty() { "every device".to_string() } else { silent.join("; ") }
                    ));
                }
            }
            MapStep::Save if ok => m.program_at = Some(now + MAP_RESTART_S),
            MapStep::Save => {
                self.map_failed(format!("The Skipper did not save the I/O list: {}", error_of(a)));
            }
        }
    }

    /// Sends the edited module to the box, which compiles it and runs it
    /// in place of the old one, or refuses it and keeps the old one.
    pub fn deploy_module(&mut self) {
        if self.deploy.busy() {
            return;
        }
        let Some(m) = &mut self.module else { return };
        if m.text.trim().is_empty() {
            self.deploy = Deploy::Failed("The program is empty: nothing to deploy.".into());
            return;
        }
        m.sent = Some(m.text.clone());
        let (name, text) = (m.name.clone(), m.text.clone());
        self.diags.clear();
        self.deploy = Deploy::Deploying;
        self.queue.push(PReq::json(
            PCall::DeployModule(name.clone()),
            "PUT",
            crate::remote::module_url(&name),
            &json!({ "source": text }),
        ));
    }

    /// Starts: the project list.
    #[must_use]
    pub fn new() -> Self {
        let mut s = Self::default();
        s.relist();
        s
    }

    /// Queues a call made elsewhere (the Devices tab's configuration).
    pub fn enqueue(&mut self, r: PReq) {
        self.queue.push(r);
    }

    /// What to send now.
    pub fn poll(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    /// Reads the open project's files again (a file the box made, such as
    /// a saved device profile, joins it; edits not saved are kept).
    pub fn reread(&mut self, project: &str) {
        if self.open.as_ref().is_some_and(|o| o.name == project) {
            self.queue
                .push(PReq::get(PCall::Project(project.into()), project_url(project)));
        }
    }

    /// Asks the box for a fresh network scan (the Devices tab's Scan).
    pub fn discover(&mut self) {
        self.queue.push(PReq::json(
            PCall::Discover,
            "POST",
            "/api/devices/discover?fresh=1".into(),
            &json!({}),
        ));
    }

    /// Asks the box about the one device at `address` (the Devices tab's
    /// Ask), for a device that does not answer the scan's broadcast.
    pub fn ask(&mut self, address: std::net::Ipv4Addr) {
        self.queue.push(PReq::json(
            PCall::Ask(address.to_string()),
            "POST",
            format!("/api/devices/discover?address={address}"),
            &json!({}),
        ));
    }

    /// Asks the box for the library's entry on a found device, its
    /// profile text with it (the Devices tab's Add).
    pub fn library(&mut self, vendor: u16, product: u16) {
        self.queue.push(PReq::get(
            PCall::Library,
            format!("/api/devices/library?vendor={vendor}&product={product}"),
        ));
    }

    /// Asks the box what the library knows of the sensor or actuator a
    /// map line names with `wired=` (the Devices tab's line under a
    /// point). The caller has held `wired` to a library folder.
    pub fn wired(&mut self, wired: &str) {
        self.queue.push(PReq::get(
            PCall::Wired(wired.to_string()),
            format!("/api/devices/library?wired={wired}"),
        ));
    }

    /// Searches the device library for the picker (`wired.rs`).
    pub fn find_wired(&mut self, query: &str) {
        self.queue.push(PReq::get(
            PCall::FindWired,
            format!("/api/devices/library?q={}", crate::remote::segment(query)),
        ));
    }

    pub fn relist(&mut self) {
        self.queue.push(PReq::get(PCall::List, "/api/projects".into()));
    }

    /// A note replaces the last one, and with it an offer to replace.
    pub(crate) fn note(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some((text.into(), error));
        self.drop_offer();
    }

    /// Takes the note, and the offer to replace that went with it, away.
    pub fn clear_notice(&mut self) {
        self.notice = None;
        self.drop_offer();
    }

    fn drop_offer(&mut self) {
        if self.pending.as_ref().is_some_and(|p| p.offered) {
            self.pending = None;
        }
    }

    /// Whether the note on show offers to replace what the box refused.
    #[must_use]
    pub fn replace_offered(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| p.offered)
    }

    /// The note that offers `pending`'s Replace, which lasts as long as
    /// that note: `what` is the refusal, the offer is added to it. With
    /// the upload forgotten meanwhile (another project opened), only the
    /// refusal is said.
    fn offer_replace(&mut self, what: &str) {
        let offer = self.pending.take().map(|p| Pending { offered: true, ..p });
        let text = if offer.is_some() {
            format!("{what} Replace it?")
        } else {
            what.to_string()
        };
        self.note(text, true);
        self.pending = offer;
    }

    /// Where an answer found nothing to apply to: its project was left or
    /// its file removed. A Deploy waiting for that save would wait for ever.
    fn save_lost(&mut self, project: &str, file: &str) {
        if self.deploy == Deploy::Saving {
            self.deploy = Deploy::Failed(format!(
                "not deployed: the project changed while {file} of {project} was being saved"
            ));
        }
    }

    /// Drops a restored edit of `project`'s `file` (`None`: of all its files).
    fn drop_draft(&mut self, project: &str, file: Option<&str>) {
        let Some(f) = file else {
            self.restore.drafts.remove(project);
            return;
        };
        if let Some(m) = self.restore.drafts.get_mut(project) {
            m.remove(f);
            if m.is_empty() {
                self.restore.drafts.remove(project);
            }
        }
    }

    /// Opens `name` after the open project's files were replaced under the
    /// page (a restore, a new project, an upload): its unsaved edits wait
    /// as restored ones and come back over the files read, or with the
    /// project when it is opened again.
    fn open_keeping_edits(&mut self, name: &str) {
        if let Some(o) = &self.open {
            let edits: Vec<_> = o
                .bufs
                .iter()
                .filter(|(_, b)| b.dirty())
                .map(|(f, b)| {
                    (
                        f.clone(),
                        crate::desk::Draft {
                            text: b.text.clone(),
                            version: b.version.clone(),
                        },
                    )
                })
                .collect();
            if !edits.is_empty() {
                self.restore
                    .drafts
                    .entry(o.name.clone())
                    .or_default()
                    .extend(edits);
            }
        }
        self.open_project(name);
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
        self.pending = None;
        self.history.clear();
        self.revision = None;
        self.pages.clear();
        self.queue
            .push(PReq::get(PCall::Project(name.into()), project_url(name)));
        self.read_history(name);
    }

    /// Puts the open project, the shown file and every unsaved edit on
    /// `desk`: the open project's edits as they stand, and any restored
    /// edit not yet put back (another project's, or a file not read yet).
    pub fn to_desk(&self, desk: &mut crate::desk::Desk) {
        // Until the box's answers put them back, the restored project and
        // file stand.
        desk.project = self
            .open
            .as_ref()
            .map(|o| o.name.clone())
            .or_else(|| self.restore.project.clone());
        desk.file = self.open.as_ref().and_then(|o| o.current.clone()).or_else(|| {
            self.restore
                .file
                .as_ref()
                .filter(|(p, _)| desk.project.as_ref() == Some(p))
                .map(|(_, f)| f.clone())
        });
        desk.drafts = self.restore.drafts.clone();
        if let Some(o) = &self.open {
            let mine = desk.drafts.entry(o.name.clone()).or_default();
            for (f, b) in &o.bufs {
                if b.dirty() {
                    mine.insert(
                        f.clone(),
                        crate::desk::Draft {
                            text: b.text.clone(),
                            version: b.version.clone(),
                        },
                    );
                } else if b.loaded {
                    mine.remove(f);
                }
            }
        }
        desk.drafts.retain(|_, m| !m.is_empty());
    }

    /// Puts `incoming` (a newer desk, the box's) in front of the person.
    /// Edits are merged per file and an unsaved edit here is never
    /// dropped: for a file edited both here and there, this page's text
    /// stays (saving it is then refused as stale if the box's copy moved).
    /// When this page has unsaved edits in the open project it stays on
    /// that project and file and takes the others' edits in; otherwise the
    /// newer desk's project opens again with its file and edits, as after
    /// a reload.
    pub fn adopt(&mut self, incoming: Restore) {
        let mut here = crate::desk::Desk::default();
        self.to_desk(&mut here);
        let mut drafts = here.drafts;
        let mut clashes = Vec::new();
        for (project, files) in incoming.drafts {
            let slot = drafts.entry(project.clone()).or_default();
            for (file, d) in files {
                match slot.get(&file) {
                    Some(mine) if mine.text != d.text => clashes.push(format!("{project}/{file}")),
                    Some(_) => {}
                    None => {
                        slot.insert(file, d);
                    }
                }
            }
        }
        if self.dirty() {
            self.restore.project = None;
            self.restore.file = None;
            self.restore.drafts = drafts;
            if let Some(o) = &self.open {
                let theirs = self.restore.drafts.get_mut(&o.name);
                let mut reads = Vec::new();
                if let Some(theirs) = theirs {
                    // This page's own are in its buffers already.
                    theirs.retain(|f, _| o.bufs.get(f).is_none_or(|b| !b.dirty()));
                    // A file read and unedited takes the other's edit when
                    // it is read again.
                    reads.extend(
                        theirs
                            .keys()
                            .filter(|f| o.bufs.get(*f).is_some_and(|b| b.loaded))
                            .map(|f| {
                                PReq::get(
                                    PCall::File {
                                        project: o.name.clone(),
                                        file: f.clone(),
                                    },
                                    file_url(&o.name, f),
                                )
                            }),
                    );
                }
                self.queue.extend(reads);
            }
        } else {
            let reopen = incoming.project.is_some();
            if reopen {
                self.restore.project = incoming.project;
                self.restore.file = incoming.file;
            }
            self.restore.drafts = drafts;
            if reopen {
                self.open = None;
                self.conflict = None;
                self.relist();
            }
        }
        if !clashes.is_empty() {
            self.note(
                format!(
                    "Also edited on another screen: {}. Your version here is kept.",
                    clashes.join(", ")
                ),
                false,
            );
        }
    }

    /// Forgets the open project, the edits and the file to show (another
    /// person signed in on this page), and asks for the projects again.
    pub fn start_over(&mut self) {
        self.open = None;
        self.restore = Restore::default();
        self.conflict = None;
        self.history.clear();
        self.revision = None;
        self.pages.clear();
        self.relist();
    }

    /// Asks for project `name`'s revisions (the Files tab's History row).
    pub fn read_history(&mut self, name: &str) {
        self.history_asked += 1;
        self.queue.push(PReq::get(
            PCall::History {
                project: name.into(),
                seq: self.history_asked,
            },
            format!("{}/history", project_url(name)),
        ));
    }

    /// Asks for revision `n` of project `name`, whole, to show.
    pub fn read_revision(&mut self, name: &str, n: u32) {
        self.queue.push(PReq::get(
            PCall::Revision {
                project: name.into(),
                n,
            },
            format!("{}/history/{n}", project_url(name)),
        ));
    }

    /// The project's files back to revision `n` (an engineer; the box
    /// records it as a new revision).
    pub fn restore(&mut self, name: &str, n: u32) {
        self.queue.push(PReq::json(
            PCall::Restore {
                project: name.into(),
                n,
            },
            "POST",
            format!("{}/history/{n}/restore", project_url(name)),
            &json!({}),
        ));
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

    /// A new project `name` holding the built-in example `from` (its
    /// project name), sent as the project's `.zip` so it arrives whole.
    pub fn create_from(&mut self, name: &str, from: &str) {
        let Some(e) = crate::playground::EXAMPLES
            .iter()
            .find(|e| crate::sim_projects::example_project(e.src).0 == from)
        else {
            return;
        };
        let (_, file) = crate::sim_projects::example_project(e.src);
        let files = helmsman_project::Files(std::collections::BTreeMap::from([
            (file, e.src.to_string()),
            ("io.map".to_string(), crate::sim_projects::example_io_map(e.src)),
        ]));
        match files.to_zip() {
            Ok(zip) => self.upload(&format!("{name}.zip"), zip, false),
            Err(e) => self.note(format!("{name} was not made: {e}"), true),
        }
    }

    pub fn rename(&mut self, from: &str, to: &str) {
        // A save or a deploy on its way is answered under the name it was
        // sent with; renamed meanwhile, its answer would find no project.
        let saving = self
            .open
            .as_ref()
            .is_some_and(|o| o.name == from && o.bufs.values().any(|b| b.saving.is_some()));
        if saving || (self.deploy.busy() && self.open.as_ref().is_some_and(|o| o.name == from)) {
            self.note(
                format!(
                    "{from} was not renamed: a save or deploy is on its way. Rename it when that is done."
                ),
                true,
            );
            return;
        }
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
                text: text.into(),
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
        if !b.loaded {
            self.note(
                format!("{from} was not renamed: it has not been read from the box yet."),
                true,
            );
            return;
        }
        let project = o.name.clone();
        let text = b.text.clone();
        self.queue.push(PReq::json(
            PCall::MoveFile {
                project: project.clone(),
                from: from.into(),
                to: to.into(),
                text: text.clone(),
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
                moved_to: None,
            },
            file_url(&project, file),
        ));
    }

    /// The open project's files as the box holds them (what is saved,
    /// not what is being typed), with `extra` among them: the files of a
    /// project made from it. Agent pages and files not read yet are not in
    /// it.
    #[must_use]
    pub fn saved_files(&self, extra: (&str, &str)) -> std::collections::BTreeMap<String, String> {
        let mut files: std::collections::BTreeMap<String, String> = self
            .open
            .iter()
            .flat_map(|o| o.bufs.iter())
            .filter(|(n, b)| b.loaded && !helmsman_project::agent::is_agent_file(n))
            .map(|(n, b)| (n.clone(), b.base.clone()))
            .collect();
        files.insert(extra.0.to_string(), extra.1.to_string());
        files
    }

    /// The project a kept design makes, as one zip (the one atomic write
    /// of many files): a name the box has is refused (409) and nothing is
    /// written. The answer goes to the view ([`PCall::DesignProject`]).
    pub fn design_project(&mut self, name: &str, zip: Vec<u8>) {
        self.queue.push(PReq {
            call: PCall::DesignProject(name.into()),
            method: "POST",
            url: format!("{}/zip", project_url(name)),
            body: Some(zip),
            ctype: "application/zip",
        });
    }

    /// The Agent's checked change (`crate::instruct`): `text` into `file`,
    /// saved with the version it was read as and with `page`, the change's
    /// History entry; then each scenario it passed as a requirement test
    /// file (new, or the test it replaces). The box makes each save a revision.
    pub fn save_agent(&mut self, file: &str, text: &str, page: &str, tests: &[(String, String)]) {
        let Some(o) = &mut self.open else { return };
        let project = o.name.clone();
        let b = o.bufs.entry(file.to_string()).or_default();
        b.text = text.to_string();
        b.saving = Some(b.text.clone());
        let version = b.version.clone();
        if !b.loaded {
            b.loaded = true;
        }
        self.queue.push(PReq::json(
            PCall::Save {
                project: project.clone(),
                file: file.to_string(),
            },
            "PUT",
            file_url(&project, file),
            &json!({ "text": text, "version": version, "page": page }),
        ));
        o.current = Some(file.to_string());
        // Each scenario a new file (`version: null`: only if no file of
        // that name is there), held here as it is sent, the editor left on
        // the file the Agent changed.
        for (f, t) in tests {
            // A test the change replaces goes with the version it was read
            // as; a new one only if no file of that name is there.
            let version = o.bufs.get(f).filter(|b| b.loaded).and_then(|b| b.version.clone());
            o.bufs.insert(
                f.clone(),
                Buf {
                    text: t.clone(),
                    saving: Some(t.clone()),
                    loaded: true,
                    ..Buf::default()
                },
            );
            self.queue.push(PReq::json(
                PCall::Save {
                    project: project.clone(),
                    file: f.clone(),
                },
                "PUT",
                file_url(&project, f),
                &json!({ "text": t, "version": version }),
            ));
        }
        self.conflict = None;
    }

    /// The open project's Agent pages, oldest first, as read so far.
    #[must_use]
    pub fn agent_pages(&self) -> Vec<&str> {
        self.pages.values().map(String::as_str).collect()
    }

    /// Puts the open project's edited Structured Text files in the one
    /// layout (`helmsman_iec::tidy`; Nick, 2026-11-11: on save and on
    /// deploy). A file with no edits, or with a save on its way, is left
    /// as it is: nothing the engineer did not touch is rewritten on the
    /// box. Returns how many files it changed.
    pub fn format(&mut self) -> usize {
        let Some(o) = &mut self.open else { return 0 };
        let mut changed = 0;
        for (name, b) in &mut o.bufs {
            if !crate::sim_projects::is_st(name) || !b.dirty() || b.saving.is_some() {
                continue;
            }
            let tidy = helmsman_iec::tidy::format(&b.text);
            if tidy != b.text {
                b.text = tidy;
                changed += 1;
            }
        }
        changed
    }

    /// Saves every edited file, each with the version it was read as,
    /// its Structured Text in the one layout first ([`Self::format`]).
    pub fn save(&mut self) {
        self.format();
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
        self.clear_notice();
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
        self.format();
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
                offered: false,
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
            offered: false,
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

    /// The project a spec makes, as one zip (the one atomic write of many
    /// files): a name the box has is refused (409) and nothing is written.
    /// The answer goes to the view ([`PCall::SpecProject`]).
    pub fn spec_project(&mut self, name: &str, zip: Vec<u8>) {
        self.queue.push(PReq {
            call: PCall::SpecProject(name.into()),
            method: "POST",
            url: format!("{}/zip", project_url(name)),
            body: Some(zip),
            ctype: "application/zip",
        });
    }

    /// The box made project `name` from a spec: it opens, and the list is
    /// read again.
    pub fn spec_made(&mut self, name: &str) {
        self.note(format!("Made {name} on the box."), false);
        self.open_keeping_edits(name);
        self.relist();
    }

    /// Sends the upload the box refused as taken again, replacing.
    pub fn replace(&mut self) {
        let Some(p) = self.pending.take() else { return };
        // A file goes into the project that refused it, not into whichever
        // is open now.
        if !has_ext(&p.file, "zip") && self.open.as_ref().is_none_or(|o| o.name != p.project) {
            self.note(format!("Open {} to replace {} in it.", p.project, p.file), true);
            return;
        }
        self.upload(&p.file, p.bytes, true);
    }

    /// The open project as a `.zip`, through the browser.
    pub fn download(&mut self) {
        if let Some(o) = &self.open {
            self.downloads.push(zip_url(&o.name));
        }
    }

    /// Forces input signal `signal` to `value`.
    /// Stops, runs or restarts `module` on the box (`op`: `stop`, `run` or
    /// `restart`); the box checks who may and audits it.
    pub fn control(&mut self, module: &str, op: &'static str) {
        self.queue.push(PReq::json(
            PCall::Control(module.to_string(), op),
            "POST",
            format!("/api/modules/{}/{op}", crate::remote::segment(module)),
            &json!({}),
        ));
    }

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

    /// Takes in an answer, as a page somebody is signed in on.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn answer(&mut self, call: PCall, a: &Answer) {
        self.answer_for(call, a, true);
    }

    /// Takes in an answer. `signed_in`: somebody is signed in on this
    /// page; with nobody, a 401 to one of the page's own reads is the box
    /// asking for a sign-in, which the header says, and no error.
    #[allow(clippy::too_many_lines)]
    pub fn answer_for(&mut self, call: PCall, a: &Answer, signed_in: bool) {
        let ok = (200..300).contains(&a.status);
        let j: Json = serde_json::from_str(&a.body).unwrap_or(Json::Null);
        if a.status == 401 && call.on_node() {
            // The watched node refused its token: the box's session is fine.
            if let Some(m) = &mut self.module {
                m.sent = None;
            }
            self.map_send = None;
            if self.deploy.busy() {
                self.deploy = Deploy::Failed(
                    "The Skipper refused the token: enter its token under Sign in (top right).".into(),
                );
            }
            return;
        }
        if a.status == 401 && !call.on_node() {
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
            // The page's own reads, with nobody signed in: the box is
            // asking for a sign-in, which the header already says.
            if !signed_in && matches!(call, PCall::List | PCall::Forces) {
                return;
            }
            let ended = "The box no longer knows this page's session (signed out, expired, or the box restarted). Sign in again;";
            match call {
                // The second half of a rename: the new name is written.
                PCall::DeleteFile {
                    file,
                    moved_to: Some(to),
                    ..
                } => self.note(
                    format!("{ended} {to} was written, but the old {file} is still on the box: delete it once signed in."),
                    true,
                ),
                _ => self.note(format!("{ended} nothing was changed."), true),
            }
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
                self.new_runs = Runs::parse(&j["tests"]);
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
                // A restored edit of a project the box no longer has is gone.
                self.restore
                    .drafts
                    .retain(|p, _| self.projects.iter().any(|r| r.name == *p));
                match self.running.clone() {
                    Some(r) => self.read_layout(&r),
                    None => self.layout = None,
                }
                let keep = self
                    .open
                    .as_ref()
                    .is_some_and(|o| self.projects.iter().any(|p| p.name == o.name));
                if !keep {
                    // The project open before a reload, while it exists.
                    let before = self
                        .restore
                        .project
                        .take()
                        .filter(|n| self.projects.iter().any(|p| &p.name == n));
                    let first = before
                        .or_else(|| self.running.clone())
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
                o.runs = Runs::parse(&j["tests"]);
                // A restored edit of a file the box no longer has is gone.
                if let Some(m) = self.restore.drafts.get_mut(&name) {
                    m.retain(|f, _| files.iter().any(|(n, _)| n == f));
                }
                self.restore.drafts.retain(|_, m| !m.is_empty());
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
                // The file shown before a reload, once its project opens.
                if let Some((_, f)) = self.restore.file.take_if(|(p, _)| *p == name)
                    && o.bufs.contains_key(&f)
                {
                    o.current = Some(f);
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
                self.run_tests();
            }
            PCall::Tests(project) => {
                self.testing = false;
                if self.open.as_ref().is_none_or(|o| o.name != project) {
                    return;
                }
                if ok {
                    self.tests = crate::requirements::Results::from_json(&j);
                } else {
                    self.note(
                        format!("The requirement tests did not run: {}", error_of(a)),
                        true,
                    );
                }
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
                let draft = self
                    .restore
                    .drafts
                    .get_mut(&project)
                    .and_then(|m| m.remove(&file));
                self.restore.drafts.retain(|_, m| !m.is_empty());
                let b = o.bufs.entry(file.clone()).or_default();
                b.text.clone_from(&text);
                b.base = text;
                b.version = j["version"].as_str().map(str::to_string);
                b.loaded = true;
                // An edit not saved before the page reloaded comes back as
                // an edit. Made against an older version of the file, it
                // keeps that version, so saving it is refused as stale and
                // the person chooses (Reload or Replace) instead of
                // overwriting a change made since.
                if let Some(d) = draft.filter(|d| d.text != b.base) {
                    let stale = d.version != b.version;
                    b.text = d.text;
                    if stale {
                        b.version = d.version;
                    }
                    let msg = if stale {
                        format!(
                            "Your unsaved edits to {file} are back; the box's copy changed since, so saving will ask which to keep."
                        )
                    } else {
                        format!("Your unsaved edits to {file} are back.")
                    };
                    self.note(msg, false);
                }
            }
            PCall::Create(name) => {
                if ok {
                    self.note(format!("Made project {name}."), false);
                    self.open_keeping_edits(&name);
                    self.relist();
                } else {
                    self.note(format!("{name} was not made: {}", error_of(a)), true);
                }
            }
            PCall::Rename { from, to } => {
                if ok {
                    if let Some(m) = self.restore.drafts.remove(&from) {
                        self.restore.drafts.insert(to.clone(), m);
                    }
                    if self.open.as_ref().is_some_and(|o| o.name == from)
                        && let Some(o) = &mut self.open
                    {
                        o.name.clone_from(&to);
                        // Reads that were on their way answered the old name.
                        self.queue
                            .push(PReq::get(PCall::Project(to.clone()), project_url(&to)));
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
                    self.drop_draft(&name, None);
                    self.note(format!("Deleted {name}."), false);
                    self.relist();
                } else {
                    self.note(format!("{name} was not deleted: {}", error_of(a)), true);
                }
            }
            PCall::Save { project, file } => {
                self.saved(&project, &file, a, &j);
                if ok {
                    self.read_history(&project);
                }
            }
            PCall::History { project, seq } => {
                if ok && seq >= self.history_applied && self.open.as_ref().is_some_and(|o| o.name == project)
                {
                    self.history_applied = seq;
                    self.history = j["revisions"]
                        .as_array()
                        .map(|a| a.iter().filter_map(revision_row).collect())
                        .unwrap_or_default();
                    // Every Agent page the box keeps, for the history that
                    // goes with the next instruction
                    // (`instruct::history`): each is read once and kept.
                    let want: Vec<u32> = self
                        .history
                        .iter()
                        .rev()
                        .filter(|r| r.page)
                        .take(PAGES)
                        .map(|r| r.n)
                        .filter(|n| !self.pages.contains_key(n))
                        .collect();
                    for n in want {
                        self.queue.push(PReq::get(
                            PCall::Page {
                                project: project.clone(),
                                n,
                            },
                            format!("{}/history/{n}", project_url(&project)),
                        ));
                    }
                }
            }
            PCall::Page { project, n } => {
                if ok
                    && self.open.as_ref().is_some_and(|o| o.name == project)
                    && let Some(page) = j["page"].as_str()
                {
                    self.pages.insert(n, page.to_string());
                    while self.pages.len() > PAGES {
                        self.pages.pop_first();
                    }
                }
            }
            PCall::Revision { project, n } => {
                if !ok {
                    self.note(format!("cannot read revision {n}: {}", error_of(a)), true);
                    return;
                }
                if self.open.as_ref().is_none_or(|o| o.name != project) {
                    return;
                }
                let files = j["files"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Some(row) = revision_row(&j) {
                    self.revision = Some(crate::history_ui::RevisionView {
                        row,
                        files,
                        page: j["page"].as_str().map(str::to_string),
                    });
                }
            }
            PCall::Restore { project, n } => {
                if ok {
                    self.revision = None;
                    self.note(
                        format!("Revision {n} restored (as revision {}).", j["revision"]),
                        false,
                    );
                    if self.open.as_ref().is_some_and(|o| o.name == project) {
                        // The files changed under the page: read them again.
                        self.open_keeping_edits(&project);
                    }
                } else {
                    self.note(format!("revision {n} was not restored: {}", error_of(a)), true);
                }
            }
            PCall::NewFile { project, file, text } => {
                if ok {
                    if let Some(o) = self.open.as_mut().filter(|o| o.name == project) {
                        o.bufs.insert(
                            file.clone(),
                            Buf {
                                base: text.clone(),
                                text,
                                version: j["version"].as_str().map(str::to_string),
                                saving: None,
                                loaded: true,
                            },
                        );
                        o.current = Some(file.clone());
                    }
                    self.note(format!("Made {file}."), false);
                } else {
                    self.note(format!("{file} was not made: {}", error_of(a)), true);
                }
            }
            PCall::MoveFile {
                project,
                from,
                to,
                text,
            } => {
                if !ok {
                    self.note(format!("{from} was not renamed: {}", error_of(a)), true);
                    return;
                }
                if let Some(o) = self.open.as_mut().filter(|o| o.name == project) {
                    let mut b = o.bufs.remove(&from).unwrap_or_default();
                    // Clean as far as what was sent: typing since stays an edit.
                    b.base = text;
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
                        moved_to: Some(to.clone()),
                    },
                    file_url(&project, &from),
                ));
                self.note(format!("Renamed {from} to {to}."), false);
            }
            PCall::DeleteFile {
                project,
                file,
                moved_to,
            } => {
                if ok || a.status == 404 {
                    self.drop_draft(&project, Some(&file));
                    if let Some(o) = self.open.as_mut().filter(|o| o.name == project) {
                        o.bufs.remove(&file);
                        if o.current.as_deref() == Some(file.as_str()) {
                            o.current = o.bufs.keys().next().cloned();
                        }
                    }
                } else if let Some(to) = moved_to {
                    self.note(
                        format!("{file} was not deleted: {}. It is now also {to}; delete {file} to finish the rename.", error_of(a)),
                        true,
                    );
                } else {
                    self.note(format!("{file} was not deleted: {}", error_of(a)), true);
                }
            }
            PCall::Check(name) => {
                if a.status == 422 {
                    self.diags = diags_of(&j);
                    let errors = self.diags.iter().filter(|d| !d.is_warning()).count();
                    self.deploy = Deploy::Failed(format!(
                        "{name} does not build: {errors} problem(s); click one to go there"
                    ));
                    return;
                }
                if !ok {
                    self.deploy = Deploy::Failed(error_of(a));
                    return;
                }
                // The build passed; what it says that never stops a deploy
                // stays listed under the editor.
                self.diags = diags_of(&j);
                self.modules = j["modules"]
                    .as_array()
                    .map(|m| {
                        m.iter()
                            .filter_map(|m| m["name"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if self.to_node {
                    self.after_check_for_node(&name, &j);
                } else if j["approval"]["needs"]["replay"].as_bool() == Some(true) {
                    self.replay(&name);
                } else {
                    self.deploy_now(&name, None);
                }
            }
            PCall::Replay(name) => match j["replay"].as_u64().filter(|_| a.status == 201) {
                Some(_) if self.to_node => self.send_to_node(),
                Some(n) => self.deploy_now(&name, Some(n)),
                // For a Skipper, in plain words (Nick, 2026-11-24, at "the
                // replay cannot run: modules do not fit together: drive:
                // port SEEN: no signal of that name").
                None if self.to_node && !unbound_ports(&error_of(a)).is_empty() => {
                    let ports = unbound_ports(&error_of(a));
                    let (names, verb, them) = (
                        ports.join(", "),
                        if ports.len() == 1 { "is" } else { "are" },
                        if ports.len() == 1 { "it" } else { "them" },
                    );
                    self.deploy = Deploy::Failed(format!(
                        "Not deployed: {names} {verb} not in the I/O list. Add {them} to the I/O list, or declare {them} under VAR in the program."
                    ));
                }
                None => {
                    self.deploy = Deploy::Failed(deploy_words(format!(
                        "the sim replay did not run: {}",
                        error_of(a)
                    )));
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
                    // A red requirement test refuses nothing: the box
                    // names it in its answer, said after what runs.
                    let red: Vec<&str> = j["red"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Json::as_str)
                        .collect();
                    let red = match red.len() {
                        0 => String::new(),
                        1 => format!(". Requirement test {} is red", red[0]),
                        n => format!(". {n} requirement tests are red: {}", red.join("; ")),
                    };
                    self.deploy = Deploy::Done(format!(
                        "{project} is running: {mods} (snapshot {}){red}",
                        short(snap)
                    ));
                    self.running = Some(project);
                    self.deployed = true;
                    self.relist();
                    self.queue.push(PReq::get(PCall::Forces, "/api/forces".into()));
                } else if a.status == 428 && replay.is_none() {
                    // The policy wants a replay the check did not say.
                    self.replay(&project);
                } else {
                    if a.status == 422 {
                        self.diags = diags_of(&j);
                    }
                    self.deploy = Deploy::Failed(deploy_words(format!(
                        "{project} was not deployed: {}",
                        error_of(a)
                    )));
                }
            }
            PCall::DeployModule(_)
                if a.status == 0 && self.map_send.as_ref().is_some_and(|m| m.program_tries < 5) =>
            {
                // Still restarting on the new map: try again shortly.
                if let Some(m) = &mut self.map_send {
                    m.program_at = Some(self.now + 2.0);
                }
                self.deploy = Deploy::Deploying;
            }
            PCall::DeployModule(name) => {
                self.map_send = None;
                let sent = self
                    .module
                    .as_mut()
                    .filter(|m| m.name == name)
                    .and_then(|m| m.sent.take().map(|t| (m, t)));
                if ok {
                    if let Some((m, t)) = sent {
                        m.base = t;
                    }
                    self.deploy = Deploy::Done(format!("{name} is running the version deployed from here."));
                    self.deployed = true;
                } else {
                    if a.status == 422 {
                        self.diags = diags_of(&j);
                    }
                    // A Skipper lists the compiler's lines under `errors`.
                    let lines: Vec<&str> = j["errors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Json::as_str)
                        .collect();
                    let why = if lines.is_empty() {
                        error_of(a)
                    } else {
                        format!("{}: {}", error_of(a), lines.join("; "))
                    };
                    self.deploy = Deploy::Failed(deploy_words(format!(
                        "{name} was not deployed (the old version keeps running): {why}"
                    )));
                }
            }
            PCall::NodeMapSend(step) => self.map_answer(step, a, &j),
            PCall::Upload(project) => {
                if ok {
                    self.note(format!("Uploaded {project}."), false);
                    self.open_keeping_edits(&project);
                    self.relist();
                } else if a.status == 409 {
                    self.offer_replace(&format!("A project named {project} is already on the box."));
                } else {
                    self.note(format!("Upload refused: {}", error_of(a)), true);
                }
            }
            PCall::Import { project, file } => {
                if ok {
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
                    self.offer_replace(&format!("{project} already has that file."));
                } else {
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
            PCall::Control(module, op) => {
                if ok {
                    let did = match op {
                        "stop" => "stopped: its outputs are at their safe values",
                        "restart" => "restarted from its initial values",
                        _ => "running again",
                    };
                    self.note(format!("{module} {did}."), false);
                } else {
                    self.note(format!("{module} was not changed: {}", error_of(a)), true);
                }
            }
            PCall::Force | PCall::Release => {
                if !ok {
                    self.note(error_of(a), true);
                }
                self.queue.push(PReq::get(PCall::Forces, "/api/forces".into()));
            }
            // The Trend tab's (`box_trend.rs`) and the Devices tab's scan
            // (`box_view.rs`).
            PCall::TrendPins
            | PCall::SavePins
            | PCall::Trend
            | PCall::Discover
            | PCall::Ask(_)
            | PCall::Library
            | PCall::Wired(_)
            | PCall::FindWired
            | PCall::Config(..)
            | PCall::Proposal
            | PCall::ProposalAccept
            | PCall::ProposalDiscard
            | PCall::ProposalPublish
            | PCall::VendorFile
            | PCall::SpecPdf(_)
            | PCall::SpecProject(_)
            | PCall::DesignProject(_)
            | PCall::PaneLayouts
            | PCall::KeepPaneLayout(_)
            | PCall::DropPaneLayout(_)
            | PCall::NodeMap(_)
            | PCall::Desk
            | PCall::SaveDesk => {}
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
            self.save_lost(project, file);
            return;
        };
        let Some(b) = o.bufs.get_mut(file) else {
            self.save_lost(project, file);
            return;
        };
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
        // Typing since a save was sent is not on the box: a Deploy saves it
        // too before it checks.
        if self.deploy == Deploy::Saving && self.dirty() {
            self.save();
            return;
        }
        // Saved: the requirement tests run against what is on the box now.
        self.run_tests();
        if self.deploy == Deploy::Saving {
            self.check();
        } else if !matches!(&self.notice, Some((_, true))) {
            self.note(format!("Saved {project}."), false);
        }
    }

    /// Asks the box to run the open project's requirement tests
    /// (`POST /api/projects/{p}/tests`); the Requirements tab shows them.
    pub fn run_tests(&mut self) {
        let Some(o) = &self.open else { return };
        if self.testing {
            return;
        }
        let name = o.name.clone();
        self.testing = true;
        self.queue.push(PReq::json(
            PCall::Tests(name.clone()),
            "POST",
            format!("{}/tests", project_url(&name)),
            &json!({}),
        ));
    }
}
