//! The Editor on the simulation in the page (docs/playground/, or any page
//! no box serves): an ST editor that checks as you type (errors marked
//! after a pause; what runs is unchanged until Deploy, as on a box), the
//! program running in the page with Run / Stop / Step, a virtual I/O
//! panel built from the program's ports, a trend, the live diagram, and
//! Claude with the visitor's own key. No box and no server: the state is
//! [`crate::playground::Session`] and [`crate::claude::Chat`].

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::{Ty, Value};

use crate::arrange::Layers;
use crate::claude::{Chat, Model, Phase, Proxy, Request, Segment, segments};
use crate::editor::{self, Body, Chrome, Tab};
use crate::playground::{MAX_SOURCE, Point, Session, TREND_SIGNALS, Widget, nice_range, tick_label};
use crate::projects::{self, Act, Diag, FilesUi, Listing, Workspace};
use crate::sim_projects::{SimProjects, SimRevision, Storage};
use crate::st_view::brand::{BLACK, GREEN, OFF_RED, WARN, WHITE, YELLOW};
use crate::st_view::{Pick, TAP, Viewer};
use crate::theme::{Theme, theme};
use crate::tips::Tip;

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// The ST editor's widget id (its selection is the Agent's scope).
const SOURCE_ID: &str = "pg-source";
/// Quiet time after the last keystroke before the editor checks, s.
const DEBOUNCE: f64 = 0.35;
/// Trend line colours, in signal order (the last, white, is the page's ink
/// in light mode).
const TREND: [Color32; TREND_SIGNALS] = [
    YELLOW,
    Color32::from_rgb(0x6E, 0xC6, 0xFF),
    Color32::from_rgb(0x9B, 0xE5, 0x8C),
    Color32::from_rgb(0xFF, 0x8A, 0x65),
    Color32::from_rgb(0xD5, 0x9C, 0xFF),
    WHITE,
];

/// Where Claude requests go: the browser, or (natively) a list tests read.
#[derive(Default)]
pub struct Link {
    #[cfg(target_arch = "wasm32")]
    web: crate::web::ClaudeLink,
    /// Requests "sent" (native builds have no network here).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<(u64, Request)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub inbox: Vec<crate::claude::Incoming>,
    /// Answers stopped.
    #[cfg(not(target_arch = "wasm32"))]
    pub aborted: usize,
}

impl Link {
    fn send(&mut self, seq: u64, req: Request, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.send(seq, req, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.sent.push((seq, req));
        }
    }

    fn abort(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.web.abort();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.aborted += 1;
        }
    }

    fn check_proxy(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.check_proxy(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.inbox.push(crate::claude::Incoming::Proxy(false));
        }
    }

    fn drain(&mut self) -> Vec<crate::claude::Incoming> {
        #[cfg(target_arch = "wasm32")]
        return self.web.drain();
        #[cfg(not(target_arch = "wasm32"))]
        std::mem::take(&mut self.inbox)
    }
}

pub struct PlaygroundView {
    pub session: Session,
    /// The Messages tab's log.
    pub log: crate::messages::Log,
    /// What the log last saw, to report changes once.
    seen: Seen,
    /// The diagram of the program running, rebuilt when it changes.
    pub viewer: Option<Viewer>,
    /// Its shared default layout (the project's) and this browser's
    /// tweaks (`arrange.rs`).
    pub layers: Layers,
    /// The viewer was just built: lay it out from the layers.
    fresh: bool,
    viewer_of: String,
    /// When the editor last changed and is not checked yet.
    pending: Option<f64>,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
    pub chat: Chat,
    pub link: Link,
    proxy_asked: bool,
    /// The device library's index, fetched when the Devices tab scans.
    #[cfg(target_arch = "wasm32")]
    library: crate::web::TextFetch,
    /// A one-line note under the top bar (loaded an example, …).
    pub notice: Option<String>,
    /// Why the last "Ask" could not be sent.
    pub ask_error: Option<String>,
    /// The projects, kept in the browser's storage.
    pub proj: SimProjects,
    pub files_ui: FilesUi,
    pub devices_ui: crate::devices::DevicesUi,
    /// What `session.sensors` was bound from: the program running and the
    /// project's files, so a frame rebinds only after a change.
    sensors_from: Option<(String, Vec<(String, String)>)>,
    /// The question "what is this program called?", asked once per
    /// conversation before its first PROGRAM answer lands.
    pub program_name: Option<ProgramName>,
    /// The revision on view (the History modal).
    pub revision: Option<crate::history_ui::RevisionView>,
    /// The project's requirement tests, as last run in this page
    /// (`crate::requirements`): after every build change, before Deploy
    /// and "Use this code".
    pub tests: crate::requirements::Results,
    /// The Requirements tab's field and open row.
    pub req_ui: crate::requirements::ReqUi,
    /// The requirement being added, and the last one's outcome.
    pub adding: Option<crate::requirements::Adding>,
    /// The next requirement request's seq: far from the Agent's, so the
    /// shared transport's answers go to the right one.
    req_seq: u64,
    /// The lines selected in the Code tab (file, first, last), as the
    /// editor last drew them: the Agent's scope when there are some.
    pub selection: Option<(String, u32, u32)>,
    /// Why the last "Use this code" was refused, shown under the answer.
    pub use_error: Option<String>,
    /// Downloads made (natively nothing saves them: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub downloads: Vec<(String, Vec<u8>)>,
}

impl PlaygroundView {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let projects: Box<dyn Storage> = Box::new(crate::web::LocalProjects);
        #[cfg(not(target_arch = "wasm32"))]
        let projects: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_storage(projects)
    }

    /// The simulation with its projects kept in `projects`; it opens the
    /// project open last (else the first example), running.
    #[must_use]
    pub fn with_storage(projects: Box<dyn Storage>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let tweaks: Box<dyn Storage> = Box::new(crate::web::LocalArrangements);
        #[cfg(not(target_arch = "wasm32"))]
        let tweaks: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_stores(projects, tweaks)
    }

    /// [`Self::with_storage`], with the diagram's personal layouts kept in
    /// `tweaks`.
    #[must_use]
    pub fn with_stores(projects: Box<dyn Storage>, tweaks: Box<dyn Storage>) -> Self {
        #[cfg(target_arch = "wasm32")]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::web::LocalKeyStore);
        #[cfg(not(target_arch = "wasm32"))]
        let store: Box<dyn crate::claude::KeyStore> = Box::new(crate::claude::NoStore);
        let proj = SimProjects::load(projects);
        let mut v = Self {
            session: Session::new(&proj.build_src()),
            viewer: None,
            layers: Layers::load(tweaks),
            fresh: false,
            viewer_of: String::new(),
            pending: None,
            last_pick: None,
            chat: Chat::new(store),
            link: Link::default(),
            proxy_asked: false,
            #[cfg(target_arch = "wasm32")]
            library: crate::web::TextFetch::default(),
            log: crate::messages::Log::default(),
            seen: Seen::default(),
            notice: None,
            ask_error: None,
            proj,
            files_ui: FilesUi::default(),
            devices_ui: crate::devices::DevicesUi::default(),
            sensors_from: None,
            program_name: None,
            revision: None,
            tests: crate::requirements::Results::default(),
            req_ui: crate::requirements::ReqUi::default(),
            adding: None,
            req_seq: 1 << 40,
            selection: None,
            use_error: None,
            #[cfg(not(target_arch = "wasm32"))]
            downloads: Vec::new(),
        };
        v.session.set_running(true);
        v.proj.running = Some(v.proj.open.clone());
        v.load_pages();
        v.sync_viewer();
        v.run_tests();
        v
    }

    /// The open project's Agent pages into the chat (a project opened).
    fn load_pages(&mut self) {
        let pages = self
            .proj
            .agent_text()
            .map(helmsman_project::agent::parse)
            .unwrap_or_default();
        self.chat.load_pages(pages);
    }

    /// The chat's pages into the open project, when they changed: written
    /// as they come, so a reload finds them.
    fn save_pages(&mut self) {
        if !self.chat.pages_dirty {
            return;
        }
        self.chat.pages_dirty = false;
        let text = helmsman_project::agent::render(&self.chat.pages());
        self.proj.set_agent_text(&text);
    }

    /// How this page runs the project's requirement tests: the session's
    /// scan period, the simulation's image sizes.
    fn test_settings(&self) -> helmsman_iec::spec::Settings {
        helmsman_iec::spec::Settings::new(crate::playground::SIZES)
            .period(std::time::Duration::from_millis(self.session.period_ms.max(1)))
    }

    /// Runs the open project's requirement tests against its build and
    /// keeps the results for the Requirements tab.
    pub fn run_tests(&mut self) {
        let plan = self.proj.test_plan();
        let v = helmsman_iec::spec::run_all(&plan, self.test_settings());
        self.tests = crate::requirements::Results::from_verdicts(&v);
    }

    /// The gate "Use this code" and Deploy share: why the project (with
    /// `extra` added or replacing a file, for a candidate) must not land,
    /// if a requirement test is red. Runs the tests on the candidate and,
    /// when it is the project as it stands, keeps the results.
    fn tests_refuse(&mut self, extra: Option<(&str, &str)>) -> Option<String> {
        let settings = self.test_settings();
        let results = match extra {
            None => {
                self.run_tests();
                self.tests.clone()
            }
            Some((file, text)) => {
                let mut files: std::collections::BTreeMap<&str, &str> = self
                    .proj
                    .files
                    .iter()
                    .map(|(n, t)| (n.as_str(), t.as_str()))
                    .collect();
                files.insert(file, text);
                let plan = helmsman_iec::spec::plan(
                    files
                        .iter()
                        .filter(|(n, _)| {
                            helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St)
                        })
                        .map(|(n, t)| (*n, *t)),
                );
                crate::requirements::Results::from_verdicts(&helmsman_iec::spec::run_all(&plan, settings))
            }
        };
        results.refusal()
    }

    /// Loads the open project's build into the session, compiled and
    /// stopped.
    fn reload(&mut self) {
        let src = self.proj.build_src();
        self.session.load(&src);
        self.pending = None;
        self.sync_viewer();
        self.run_tests();
    }

    /// Opens project `name` and runs it if it compiles (Nick, 2026-11-01:
    /// an example that opens stopped looks dead); an edit still waits for
    /// Deploy.
    pub fn open_project(&mut self, name: &str) {
        self.save_pages();
        self.proj.open_project(name);
        self.load_pages();
        self.reload();
        self.notice = None;
        if self.session.build.as_ref().is_some_and(|b| b.module.is_some()) {
            self.session.set_running(true);
            self.proj.running = Some(self.proj.open.clone());
        }
    }

    /// The editor's text changed: the session's source follows at once
    /// and is checked after the pause. What runs waits for Deploy.
    fn edited(&mut self, t: f64) {
        self.session.src = self.proj.build_src();
        self.pending = Some(t);
        self.notice = None;
    }

    /// The compile errors, each with its file and line.
    #[must_use]
    pub fn diags(&self) -> Vec<Diag> {
        self.session
            .diags
            .iter()
            .map(|d| {
                let (file, line) = self
                    .proj
                    .locate(d.pos.line)
                    .unwrap_or_else(|| (self.proj.current.clone(), d.pos.line));
                Diag {
                    file,
                    line,
                    col: d.pos.col,
                    message: d.msg.clone(),
                }
            })
            .collect()
    }

    /// Opens `file` at `line` in the ST editor.
    fn goto(&mut self, file: &str, line: u32, chrome: &mut Chrome) {
        self.proj.open_file(file);
        chrome.goto(line);
    }

    /// Carries out what the Files sidebar asked.
    fn act(&mut self, a: Act) {
        match a {
            Act::UseAnswer { name, text, save } => {
                // A block answer lands in the open project: the project's
                // requirement tests must stay green with it in. (A PROGRAM
                // answer opens a new project, which has no tests yet.)
                if !text.to_ascii_uppercase().contains("PROGRAM ") {
                    let file = self.proj.free_file(&format!("{name}.st"));
                    if let Some(why) = self.tests_refuse(Some((&file, &text))) {
                        self.notice = Some(why.clone());
                        self.log.error(
                            self.session.now_ms as f64 / 1000.0,
                            format!("Use this code: {why}"),
                        );
                        return;
                    }
                }
                // Unsaved edits first (a save is a revision: none when
                // there is nothing to save).
                if save && self.proj.dirty() {
                    self.proj.save();
                } else if !save {
                    let open = self.proj.open.clone();
                    self.proj.open_project(&open);
                }
                let mut dropped = 0;
                if text.to_ascii_uppercase().contains("PROGRAM ") {
                    dropped = self.land_program(&name, text);
                } else {
                    let file = self.proj.free_file(&format!("{name}.st"));
                    self.proj.new_file(&file);
                    if let Some(t) = self.proj.current_text_mut() {
                        *t = text;
                    }
                }
                // Recorded with the page that produced it.
                let page = self
                    .chat
                    .history
                    .last()
                    .map(|e| format!("{}\n\n{}", e.question, e.answer));
                let what = format!("took the Agent's answer into {}", self.proj.current);
                self.proj.save_as(&what, page.as_deref());
                self.reload();
                let size = if dropped > 0 {
                    format!(" {dropped} oldest iteration(s) dropped: the project was past its size.")
                } else {
                    String::new()
                };
                self.notice = Some(format!(
                    "{} opened from the Agent's answer; Deploy runs it.{size}",
                    self.proj.current
                ));
            }
            Act::OpenProject(p) => self.open_project(&p),
            Act::OpenFile(f) => self.proj.open_file(&f),
            Act::ShowRevision(n) => {
                self.revision = self
                    .proj
                    .revisions()
                    .iter()
                    .find(|r| r.n == n)
                    .map(SimRevision::view);
            }
            Act::NewProject(p) => {
                self.proj.create(&p, None);
                self.reload();
            }
            Act::CopyProject { from, to } | Act::FromStarter { from, to } => {
                self.proj.create(&to, Some(&from));
                self.reload();
            }
            Act::RenameProject { from, to } => self.proj.rename(&from, &to),
            Act::DeleteProject(p) => {
                let open = self.proj.open == p;
                self.proj.delete(&p);
                if open {
                    self.reload();
                }
            }
            Act::NewFile(f) => self.proj.new_file(&f),
            Act::RenameFile { from, to } => {
                self.proj.rename_file(&from, &to);
                self.edited(f64::NEG_INFINITY);
            }
            Act::DeleteFile(f) => {
                self.proj.delete_file(&f);
                self.edited(f64::NEG_INFINITY);
            }
            Act::CopyFile(f) => {
                if let Some(text) = self.proj.files.get(&f) {
                    self.files_ui.clipboard = Some((f, text.clone()));
                }
            }
            Act::DuplicateFile(f) => {
                if let Some(text) = self.proj.files.get(&f).cloned() {
                    let name = self.proj.free_file(&f);
                    self.proj.add_file(&name, text);
                    self.edited(f64::NEG_INFINITY);
                }
            }
            Act::PasteFile { name, text } => {
                let name = self.proj.free_file(&name);
                self.proj.add_file(&name, text);
                self.edited(f64::NEG_INFINITY);
            }
            Act::Deploy => self.deploy(),
        }
    }

    /// Rebuilds the diagram when the program running changed.
    fn sync_viewer(&mut self) {
        let Some(b) = &self.session.build else {
            self.viewer = None;
            return;
        };
        if b.src == self.viewer_of && self.viewer.is_some() {
            return;
        }
        self.viewer_of.clone_from(&b.src);
        let old = self.viewer.as_ref().and_then(|v| v.because.clone());
        // Blocks the user moved stay moved across edits: kept as tweaks,
        // and the new diagram is laid out from the layers.
        if let Some(v) = &self.viewer {
            self.layers.track(v);
        }
        self.fresh = true;
        self.viewer = b.module.clone().map(|m| {
            let mut v = Viewer::new(&b.src, m);
            // Opens with no path lit; the user's pick survives an edit
            // while its variable does.
            v.because = old.filter(|o| v.knows(o));
            v
        });
    }

    /// Checks the editor if it has been quiet long enough (or `now`):
    /// errors are marked, and the program running (and its diagram) stay
    /// as deployed.
    fn check_due(&mut self, t: f64, now: bool) {
        if let Some(at) = self.pending
            && (now || t - at >= DEBOUNCE)
        {
            self.pending = None;
            let src = self.session.src.clone();
            self.session.check(&src);
            self.run_tests();
        }
    }

    /// Picks output `name`, as a tap on it does.
    pub fn pick(&mut self, name: &str) {
        if let Some(v) = &mut self.viewer {
            v.because = Some(name.to_string());
        }
    }

    /// What the header names: the open project.
    #[must_use]
    pub fn title(&self) -> String {
        self.proj.open.clone()
    }

    /// The PROGRAM running, in lower case (the Monitor's name for it);
    /// the project's name while nothing compiled.
    #[must_use]
    pub fn program_name(&self) -> String {
        self.viewer
            .as_ref()
            .and_then(|v| v.module.program())
            .map_or_else(|| self.title(), |p| p.name.to_ascii_lowercase())
    }

    /// The project's devices and points as text, the agent's context
    /// (what the Devices tab shows, values included).
    #[must_use]
    pub fn devices_text(&self) -> String {
        let text = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        let map = helmsman_project::iomap::skim(&text);
        let points: Vec<(String, Value)> = self
            .session
            .inputs()
            .chain(self.session.outputs())
            .map(|p| (p.name.clone(), p.value))
            .collect();
        let live = |name: &str| {
            points
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| crate::devices::Live {
                    value: Some(*v),
                    quality: None,
                    forced: false,
                })
        };
        let mut text = crate::devices::describe(&crate::devices::tree(&map, &live, &|_| {
            crate::devices::LinkState::Simulated
        }));
        text.push_str(&crate::devices::describe_found(&self.devices_ui.found));
        text.push_str(&crate::plant::describe(&self.plant_states(&map)));
        text
    }

    /// The project's plant-model states (`{state}` declarations), each
    /// with the simulation's value and the map signal that measures it.
    #[must_use]
    pub fn plant_states(&self, map: &helmsman_project::iomap::MapText) -> Vec<crate::plant::PlantState> {
        let files: Vec<(&str, &str)> = self
            .proj
            .files
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        let value = |name: &str| {
            self.session
                .trace
                .as_ref()
                .and_then(|t| crate::plant::value_in_trace(t, name))
        };
        let live = |name: &str| self.session.value(name);
        let mut states = crate::plant::states(&files, map, &value, &live);
        for s in &mut states {
            s.from_sensor = s.measured_by.as_ref().is_some_and(|m| {
                self.session
                    .sensors
                    .iter()
                    .any(|(_, signal)| signal.eq_ignore_ascii_case(&m.signal))
            });
        }
        states
    }

    /// Points the simulation's measured plant states at their sensors
    /// (`Session::sensors`): a state whose map `in` signal is one of the
    /// program's inputs runs from that input instead of the model.
    pub fn bind_sensors(&mut self) {
        let src = self.session.build.as_ref().map_or("", |b| b.src.as_str());
        if self.sensors_from.as_ref().is_some_and(|(s, f)| {
            s == src
                && f.len() == self.proj.files.len()
                && f.iter()
                    .zip(&self.proj.files)
                    .all(|((a, x), (b, y))| a == b && x == y)
        }) {
            return;
        }
        let src = src.to_string();
        let files: Vec<(String, String)> = self
            .proj
            .files
            .iter()
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        let map = helmsman_project::iomap::skim(
            self.proj
                .files
                .get(helmsman_project::IO_MAP)
                .map_or("", String::as_str),
        );
        let states = self.plant_states(&map);
        let mut sensors = crate::plant::substitutions(&states, &src);
        sensors.retain(|(_, signal)| self.session.inputs().any(|p| p.name.eq_ignore_ascii_case(signal)));
        self.session.sensors = sensors;
        self.sensors_from = Some((src, files));
    }

    /// Sends the typed question about the editor's program.
    pub fn ask(&mut self, ctx: &egui::Context) {
        let diags = self.session.diag_lines();
        let devices = self.devices_text();
        self.chat.edit_scope = self.scope().envelope(&self.proj.open, &self.proj.files);
        self.use_error = None;
        match self.chat.ask(&self.session.src, &diags, &devices) {
            Ok((seq, req)) => {
                self.ask_error = None;
                self.link.send(seq, req, ctx);
            }
            Err(e) => self.ask_error = Some(e),
        }
    }

    /// Adds a requirement: the Agent writes its test when it can be
    /// reached (the visitor's key or the local proxy); else it is written
    /// as "no test yet".
    pub fn add_requirement(&mut self, text: &str, ctx: &egui::Context) {
        use crate::requirements::{Adding, Phase, name_for, page_request};
        if self.proj.is_example() || crate::requirements::refusal(text).is_some() {
            return;
        }
        let taken: Vec<String> = self.proj.files.keys().cloned().collect();
        let name = name_for(text, &taken);
        self.req_seq += 1;
        let seq = self.req_seq;
        let mut a = Adding::in_page(text, &name, seq);
        match self.chat.route() {
            Ok(route) => {
                let programs: Vec<(String, String)> = self
                    .proj
                    .files
                    .iter()
                    .filter(|(f, _)| {
                        helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St)
                            && !helmsman_iec::spec::is_test(f)
                    })
                    .map(|(f, t)| (f.clone(), t.clone()))
                    .collect();
                let req = page_request(&route, self.chat.model, text, &name, &programs);
                self.link.send(seq, req, ctx);
            }
            Err(why) if matches!(self.chat.proxy, crate::claude::Proxy::Checking) => {
                a.phase = Phase::Failed(why.into());
            }
            Err(_) => {
                let file = self.write_test(&name, &helmsman_iec::spec::placeholder(&name, text));
                a.phase = Phase::Untested {
                    file,
                    why: "No agent here: paste an API key in the Agent tab, or open the row and write the test yourself."
                        .into(),
                };
            }
        }
        self.adding = Some(a);
    }

    /// Writes `test_<name>.st` into the open project, saved, and runs the
    /// tests; the file's name.
    fn write_test(&mut self, name: &str, text: &str) -> String {
        let file = format!("test_{name}.st");
        self.proj.files.insert(file.clone(), text.to_string());
        self.proj.save();
        self.run_tests();
        file
    }

    /// An answer for the requirement being added: taken (`true`) or not
    /// one of its pieces.
    fn requirement_answer(&mut self, m: &crate::claude::Incoming) -> bool {
        use crate::claude::Incoming;
        use crate::requirements::{Phase, State};
        let Some(a) = self.adding.as_mut() else {
            return false;
        };
        let seq = match m {
            Incoming::Status { seq, .. }
            | Incoming::Chunk { seq, .. }
            | Incoming::End { seq }
            | Incoming::Unreachable { seq, .. } => *seq,
            Incoming::Proxy(_) => return false,
        };
        if seq != a.seq {
            return seq > 1 << 40;
        }
        match m {
            Incoming::Chunk { text, .. } => a.chunk(text),
            Incoming::Status { status, body, .. } => {
                a.phase = Phase::Failed(format!(
                    "the Agent answered {status}: {}",
                    body.chars().take(200).collect::<String>()
                ));
            }
            Incoming::Unreachable { why, .. } => a.end(Some(why)),
            Incoming::End { .. } => {
                a.end(None);
                if a.busy() {
                    let (name, requirement) = (a.name.clone(), a.requirement.clone());
                    let program = a.page_program();
                    let text = match &program {
                        Some(p) => format!("{}{}", helmsman_iec::spec::header(&requirement), p.trim_start()),
                        None => helmsman_iec::spec::placeholder(&name, &requirement),
                    };
                    let file = self.write_test(&name, &text);
                    let row = self.tests.rows.iter().find(|r| r.file == file).cloned();
                    let a = self.adding.as_mut().expect("still adding");
                    a.phase = match (program, row) {
                        (None, _) => Phase::Untested {
                            file,
                            why: "the Agent's answer held no test; open the row to write one".into(),
                        },
                        (Some(_), Some(r)) if r.state == State::Untested => Phase::Untested {
                            file,
                            why: String::new(),
                        },
                        (Some(_), Some(r)) => Phase::Written {
                            file,
                            state: r.state,
                            why: r.why,
                        },
                        (Some(_), None) => Phase::Written {
                            file,
                            state: State::Failed,
                            why: "it did not run".into(),
                        },
                    };
                }
            }
            Incoming::Proxy(_) => {}
        }
        true
    }

    pub fn stop_answer(&mut self) {
        self.chat.stop();
        self.link.abort();
    }

    /// Loads the program in Claude's answer (never runs it).
    /// Loads `src` into the editor (the open file, or the project's first
    /// program file when a map or settings file is open), compiled and
    /// stopped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn load(&mut self, src: &str, what: &str) {
        if crate::sim_projects::is_st(&self.proj.current) {
            if let Some(t) = self.proj.current_text_mut() {
                src.clone_into(t);
            }
        } else if let Some(f) = self
            .proj
            .files
            .keys()
            .find(|f| crate::sim_projects::is_st(f))
            .cloned()
        {
            self.proj.files.insert(f.clone(), src.to_string());
            self.proj.current = f;
        } else {
            self.proj.files.insert("main.st".into(), src.to_string());
            self.proj.current = "main.st".into();
        }
        let ok = self.session.load(&self.proj.build_src());
        self.pending = None;
        self.sync_viewer();
        self.notice = Some(if ok {
            format!("Loaded {what}. Press Run to start it.")
        } else {
            format!("Loaded {what}; it does not compile yet (see the errors under the editor).")
        });
    }

    /// "Use this code" on the live answer: [`Self::use_page`] on the page
    /// it made (card agent-scoped-edits: into the scope it was asked
    /// with). Nothing runs until Deploy. Whether the answer had code.
    pub fn use_answer(&mut self) -> bool {
        let Some(a) = self.chat.answer.as_ref().filter(|a| a.phase == Phase::Done) else {
            return false;
        };
        if !has_code(&a.text) {
            return false;
        }
        let page = match self.chat.history.last() {
            Some(p) if p.answer == a.text => p.clone(),
            _ => crate::claude::Exchange {
                answer: a.text.clone(),
                ..Default::default()
            },
        };
        self.use_page(&page);
        true
    }

    /// The code into a new project (a PROGRAM, from a built-in example)
    /// as before scopes (Nick, 2026-11-01). A PROGRAM goes to
    /// the conversation's program (Nick, 2026-11-01: named once, asked
    /// here the first time, see [`ProgramName`]); a block goes into the
    /// open project as a file.
    pub fn take_code(&mut self, code: String) {
        let is_program = code.to_ascii_uppercase().contains("PROGRAM ");
        let name = match (&self.chat.program, is_program) {
            (Some(p), true) => p.clone(),
            (None, true) => {
                // The PROGRAM's name to start from; past a built-in example's
                // name (read-only), the next free one.
                let want = answer_name(&code);
                let input = if self.proj.examples.contains_key(&want) {
                    self.proj.free(&want)
                } else {
                    want
                };
                self.program_name = Some(ProgramName {
                    text: code.clone(),
                    input,
                    error: None,
                });
                return;
            }
            (_, false) => answer_name(&code),
        };
        self.use_named(name, code);
    }

    /// The code into project (or file) `name`, after the unsaved-edits
    /// question when there is one.
    fn use_named(&mut self, name: String, code: String) {
        let act = Act::UseAnswer {
            name,
            text: code,
            save: true,
        };
        if self.proj.dirty() {
            self.files_ui.confirm = Some(act);
        } else {
            self.act(act);
        }
    }

    /// The name typed for the conversation's program is taken: the
    /// pending code lands there (the modal's OK; tests call it).
    pub fn confirm_program_name(&mut self) {
        let Some(mut p) = self.program_name.take() else {
            return;
        };
        let name = p.input.trim().to_ascii_lowercase();
        if !helmsman_project::project_name(&name) {
            p.error =
                Some("A program's name is 1 to 48 of a-z 0-9 _ -, starting with a letter or digit.".into());
            self.program_name = Some(p);
            return;
        }
        if self.proj.examples.contains_key(&name) {
            p.error = Some(format!(
                "{name} is a built-in example and read-only: pick another name."
            ));
            self.program_name = Some(p);
            return;
        }
        self.chat.program = Some(name.clone());
        self.use_named(name, p.text);
    }

    /// A PROGRAM answer into project `project` (made if there is none):
    /// the newest is always `main.st`; the one it replaced becomes the
    /// next `vN.st`, kept under "earlier" and never built. When the
    /// project would outgrow its size, the oldest iterations go first;
    /// returns how many went.
    fn land_program(&mut self, project: &str, text: String) -> usize {
        let fresh = !self.proj.exists(project);
        if fresh {
            self.proj.create(project, None);
            // `create` opens it with the starter program; the answer takes its place.
            self.proj.files.clear();
        } else if self.proj.open != project {
            self.proj.open_project(project);
        }
        let main = helmsman_project::MAIN;
        if let Some(old) = self.proj.files.remove(main) {
            let n = helmsman_project::Files(self.proj.files.clone()).next_iteration();
            self.proj.files.insert(helmsman_project::iteration_name(n), old);
        }
        self.proj.files.insert(main.to_string(), text);
        self.proj.current = main.to_string();
        let mut dropped = 0;
        while helmsman_project::Files(self.proj.files.clone()).total() > helmsman_project::MAX_TOTAL {
            let oldest = helmsman_project::Files(self.proj.files.clone())
                .iterations()
                .next()
                .map(|(_, f)| f.to_string());
            let Some(oldest) = oldest else { break };
            self.proj.files.remove(&oldest);
            dropped += 1;
        }
        dropped
    }

    /// The ST file the Agent's scope starts from: the open one, or the
    /// project's first program file when a map or settings file is open.
    fn scope_file(&self) -> Option<String> {
        if crate::sim_projects::is_st(&self.proj.current) && self.proj.files.contains_key(&self.proj.current)
        {
            return Some(self.proj.current.clone());
        }
        self.proj
            .files
            .keys()
            .find(|f| crate::sim_projects::is_st(f))
            .cloned()
    }

    /// Where the paused scan stands, as the files show it: `12` or
    /// `12 of motor.st`.
    fn paused_line(&self) -> Option<String> {
        let id = self.session.paused_at()?;
        let line = self.session.trace.as_ref()?.map().stmt(id)?.line;
        Some(match self.proj.locate(line) {
            Some((file, l)) => format!("{l} of {file}"),
            None => line.to_string(),
        })
    }

    /// The block picked in the diagram as a scope: its file, its name and
    /// the lines of the statement it was lowered from. Only while the
    /// diagram is of the text in the editor (an edit not yet deployed
    /// moves the lines under it).
    fn picked_block(&self) -> Option<(String, String, u32, u32)> {
        let v = self.viewer.as_ref()?;
        if v.src != self.proj.build_src() {
            return None;
        }
        let d = v.diagram();
        let (stmt, name) = match v.pick? {
            Pick::Node(n) => {
                let n = d.nodes.get(n.0 as usize)?;
                let name = match &n.kind {
                    helmsman_diagram::NodeKind::Fb { instance, .. } => instance.clone(),
                    helmsman_diagram::NodeKind::Write { path }
                    | helmsman_diagram::NodeKind::Read { path } => path.clone(),
                    _ if !n.label.trim().is_empty() => n.label.clone(),
                    _ => n.text.clone(),
                };
                (n.stmt, name)
            }
            Pick::Region(r) => {
                let r = d.regions.get(r.0 as usize)?;
                (r.stmt, r.label.clone())
            }
        };
        let s = d.stmts.iter().find(|s| s.id == stmt)?;
        let (file, line) = self.proj.locate(s.pos.line)?;
        let text = self.proj.files.get(&file)?;
        let end = crate::scope::statement_end(text, line, s.pos.col);
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let name = if name.chars().count() > 32 {
            format!("{}…", name.chars().take(31).collect::<String>())
        } else {
            name
        };
        Some((file, name, line, end))
    }

    /// What the Agent may change now (`crate::scope`): the lines selected
    /// in the Code tab, else the block picked in the diagram, else the
    /// open file; a new project while a built-in example is open.
    #[must_use]
    pub fn scope(&self) -> crate::scope::Scope {
        crate::scope::detect(&crate::scope::Focus {
            writable: !self.proj.is_example(),
            open: self.scope_file(),
            selection: self.selection.clone(),
            block: self.picked_block(),
        })
    }

    /// Back to the whole file: the selection and the pick go.
    pub fn whole_file(&mut self, ctx: &egui::Context) {
        self.selection = None;
        editor::deselect(ctx, egui::Id::unique(SOURCE_ID));
        if let Some(v) = &mut self.viewer {
            v.pick = None;
        }
    }

    /// "Use this code" on `page` (the live answer's, or an earlier one):
    /// the answer taken into the scope its question went with (the scope
    /// now, for a page older than scopes), or refused with what it tried
    /// to touch. A new project goes the way it always has (named once
    /// per conversation); anything else lands in the open project
    /// through [`Self::land_scoped`].
    pub fn use_page(&mut self, page: &crate::claude::Exchange) {
        let asked = crate::scope::from_asked(&page.asked);
        let scope = match asked {
            _ if self.proj.is_example() => crate::scope::Scope::NewProject,
            Some((_, crate::scope::Scope::NewProject)) | None => self.scope(),
            Some((project, _)) if project != self.proj.open => {
                return self.refuse(&format!(
                    "This answer is for {project}; open it first, then use the code."
                ));
            }
            Some((_, s)) => s,
        };
        match crate::scope::apply(&scope, &self.proj.files, &page.answer) {
            Err(r) => self.refuse(&r.why),
            Ok(files) if scope == crate::scope::Scope::NewProject => {
                self.use_error = None;
                if let Some(code) = files.get(helmsman_project::MAIN) {
                    self.take_code(code.clone());
                }
            }
            Ok(files) => self.land_scoped(&scope, &files, page),
        }
    }

    /// A refused "Use this code": said under the answer, on the line under
    /// the header and in Messages.
    fn refuse(&mut self, why: &str) {
        self.use_error = Some(why.to_string());
        self.notice = Some(format!("Use this code refused: {why}"));
        self.log.error(
            self.session.now_ms as f64 / 1000.0,
            format!("Use this code refused: {why}"),
        );
    }

    /// `files` (the project with the answer in its scope) lands, if the
    /// requirement tests pass with it: unsaved edits are saved first as
    /// their own revision; a whole `main.st` with its PROGRAM is the next
    /// iteration (the one it replaces kept as `vN.st`), anything else the
    /// file edited in place; then one revision with the page.
    fn land_scoped(
        &mut self,
        scope: &crate::scope::Scope,
        files: &crate::scope::Files,
        page: &crate::claude::Exchange,
    ) {
        let Some(file) = scope.file().map(str::to_string) else {
            return;
        };
        let Some(text) = files.get(&file).cloned() else {
            return;
        };
        if let Some(why) = self.tests_refuse(Some((&file, &text))) {
            return self.refuse(&why);
        }
        self.use_error = None;
        if self.proj.dirty() {
            self.proj.save();
        }
        let iteration = matches!(scope, crate::scope::Scope::File(_))
            && file == helmsman_project::MAIN
            && !crate::scope::pous(&text).is_empty()
            && text.to_ascii_uppercase().contains("PROGRAM ");
        let open = self.proj.open.clone();
        let dropped = if iteration {
            self.land_program(&open, text)
        } else {
            self.proj.files.insert(file.clone(), text);
            self.proj.current.clone_from(&file);
            0
        };
        let into = scope.label().replacen("Editing: ", "", 1);
        let what = if iteration {
            format!("took the Agent's answer into {into} as its next iteration")
        } else {
            format!("took the Agent's answer into {into}")
        };
        let record = format!("{}\n\n{}", page.question, page.answer);
        self.proj.save_as(&what, Some(&record));
        self.reload();
        let size = if dropped > 0 {
            format!(" {dropped} oldest iteration(s) dropped: the project was past its size.")
        } else {
            String::new()
        };
        self.notice = Some(format!(
            "The Agent's change is in {into}, saved, and the requirement tests pass; Deploy runs it.{size}"
        ));
    }

    /// The revision on view, in the middle of the screen, until Close or
    /// Restore (a built-in example is read-only).
    fn revision_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(view) = self.revision.clone() else {
            return;
        };
        let current: std::collections::BTreeMap<String, String> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        let write = if self.proj.is_example() {
            Err("A built-in example is read-only: copy the project first.".into())
        } else {
            Ok(())
        };
        match crate::history_ui::modal(ctx, chrome, &view, &current, &write) {
            Some(crate::history_ui::HistoryAct::Close) => self.revision = None,
            Some(crate::history_ui::HistoryAct::Restore(n)) => {
                self.revision = None;
                if self.proj.restore(n) {
                    self.reload();
                    self.notice = Some(format!("Revision {n} restored."));
                }
            }
            None => {}
        }
    }

    /// The "Name the program" question as a box in the middle of the
    /// screen (the view draws it each frame after the Editor).
    fn program_name_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(mut p) = self.program_name.clone() else {
            return;
        };
        let mut done = false;
        let mut ok = false;
        let m = egui::Modal::new(egui::Id::unique("pg-program-name")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            let why = "The Agent's answers in this conversation land in a project of this name: \
                       the newest as main.st, the earlier ones kept under it.";
            ui.label(RichText::new("Name this program").strong().size(16.0))
                .tip(why);
            ui.add_space(8.0);
            let field = ui
                .add(
                    egui::TextEdit::singleline(&mut p.input)
                        .hint_text("a-z 0-9 _ -")
                        .desired_width(f32::INFINITY),
                )
                .tip(format!("{why} Enter confirms."));
            chrome.hit("program name", field.rect);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(e) = &p.error {
                ui.colored_label(RED, e);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let tall = Vec2::new(0.0, TAP * 0.7);
                let yes = ui
                    .add(egui::Button::new(RichText::new("Use this code").strong()).min_size(tall))
                    .tip("Makes the project under this name and takes the code into it (Enter).");
                chrome.hit("program ok", yes.rect);
                let no = ui
                    .add(egui::Button::new("Cancel").min_size(tall))
                    .tip("Leaves the code in the answer. Nothing is made.");
                chrome.hit("program cancel", no.rect);
                if yes.clicked() || enter {
                    ok = true;
                } else if no.clicked() {
                    done = true;
                }
            });
        });
        chrome.hit("program form", m.response.rect);
        // OK first: the click that pressed it also reads as a click on
        // the modal's backdrop, which would otherwise shut the question.
        if ok {
            self.program_name = Some(p);
            self.confirm_program_name();
        } else if done || m.should_close() {
            self.program_name = None;
        } else {
            self.program_name = Some(p);
        }
    }

    /// One frame of the Editor.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.tick(&ui.ctx().clone());
        editor::show(ui, chrome, self);
        if self.devices_ui.take_import() {
            #[cfg(target_arch = "wasm32")]
            crate::web::pick_file(crate::app::DEVICE_ACCEPT, &ui.ctx().clone());
        }
        if let Some(crate::devices::DeviceAct::Add(line)) = self.devices_ui.form_ui(&ui.ctx().clone(), chrome)
        {
            self.add_device_line(&line);
        }
        if let Some(line) = self.devices_ui.plant.form_ui(&ui.ctx().clone(), chrome) {
            self.add_device_line(&line);
        }
        if let Some(a) = self.files_ui.naming_ui(&ui.ctx().clone(), chrome) {
            self.act(a);
        }
        if let Some(a) = self.files_ui.confirm_ui(&ui.ctx().clone(), chrome) {
            self.act(a);
        }
        // Drawn after the unsaved-edits question: a modal that appears in
        // the frame a click is released in reads that release as a click
        // on its backdrop and shuts itself.
        self.program_name_ui(&ui.ctx().clone(), chrome);
        self.revision_ui(&ui.ctx().clone(), chrome);
    }

    /// Runs the program (and Claude's answer) on with nothing drawn: the
    /// Monitor watches the same session.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.proxy_asked {
            self.proxy_asked = true;
            self.link.check_proxy(ctx);
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(r) = self.library.take() {
            match r.and_then(|t| helmsman_project::devlib::Index::parse(&t)) {
                Ok(index) => self.devices_ui.attach_library(&index),
                Err(why) => {
                    self.devices_ui.library_note =
                        Some(format!("the device library could not be read: {why}"))
                }
            }
        }
        for m in self.link.drain() {
            if !self.requirement_answer(&m) {
                self.chat.receive(m);
            }
        }
        self.save_pages();
        let t = ctx.input(|i| i.time);
        self.check_due(t, false);
        self.arrange();
        self.bind_sensors();
        self.session.advance(t);
        self.watch(t);
        if self.session.running || self.pending.is_some() || self.chat.busy() {
            ctx.request_repaint();
        }
    }
}

/// What [`PlaygroundView::watch`] compares against.
#[derive(Default)]
struct Seen {
    fault: Option<String>,
    diags: Vec<String>,
    notice: Option<String>,
    storage: Option<String>,
    running: Option<bool>,
}

impl PlaygroundView {
    /// Reports to the log what changed since the last frame: compile
    /// errors (as they appear), a fault, a notice, a storage problem,
    /// the program starting or stopping.
    fn watch(&mut self, t: f64) {
        let fault = self.session.fault.as_ref().map(ToString::to_string);
        if fault != self.seen.fault {
            if let Some(f) = &fault {
                self.log
                    .error(t, format!("Fault: {f}. Run or Step restarts the program."));
            }
            self.seen.fault = fault;
        }
        let diags = self.session.diag_lines();
        if diags != self.seen.diags {
            if !diags.is_empty() {
                let n = diags.len();
                self.log
                    .error(t, format!("{n} problem(s): {}", diags.join(" · ")));
            } else if !self.seen.diags.is_empty() {
                self.log.info(t, "Compiles.");
            }
            self.seen.diags = diags;
        }
        let notice = self.proj.notice.as_ref().map(|(n, _)| n.clone());
        if notice != self.seen.notice {
            if let Some((n, error)) = &self.proj.notice {
                if *error {
                    self.log.error(t, n.clone());
                } else {
                    self.log.info(t, n.clone());
                }
            }
            self.seen.notice = notice;
        }
        if self.proj.storage_problem != self.seen.storage {
            if let Some(w) = &self.proj.storage_problem {
                self.log.warning(t, w.clone());
            }
            self.seen.storage.clone_from(&self.proj.storage_problem);
        }
        let running = Some(self.session.running);
        if running != self.seen.running {
            if self.seen.running.is_some() {
                self.log.info(
                    t,
                    if self.session.running {
                        "Running."
                    } else {
                        "Stopped."
                    },
                );
            }
            self.seen.running = running;
        }
    }

    /// Lays the diagram out from its layers, or keeps a finished move.
    pub fn arrange(&mut self) {
        let fresh = std::mem::take(&mut self.fresh);
        if let Some(v) = &mut self.viewer {
            let key = format!("sim:{}", self.proj.open);
            self.layers.show(v, &key, self.proj.layout(), fresh);
        }
    }

    /// Saves the diagram as it stands as the open project's default.
    pub fn save_layout(&mut self) {
        let Some(v) = &self.viewer else { return };
        let text = v.arrangement().to_text();
        if self.proj.save_layout(&text) {
            self.layers.adopt(v, &text);
        }
    }
}

impl Body for PlaygroundView {
    fn toolbar(&mut self, ui: &mut egui::Ui, _chrome: &mut Chrome) {
        // Run, step and restart are glyphs over the diagram (app::glyphs);
        // here only what state the program is in.
        let s = &self.session;
        let state = if s.fault.is_some() {
            "faulted"
        } else if s.paused.is_some() {
            "paused"
        } else if s.running {
            "running"
        } else {
            "stopped"
        };
        #[allow(clippy::cast_precision_loss)]
        let secs = s.now_ms as f64 / 1000.0;
        ui.label(
            RichText::new(format!("{state} · {secs:.1}s"))
                .monospace()
                .strong()
                .color(if s.running && s.fault.is_none() {
                    GREEN
                } else {
                    RED
                }),
        )
        .tip(format!(
            "The simulation's state and clock, after {} scans.",
            s.scans
        ));
    }

    fn has_notices(&self) -> bool {
        self.session.fault.is_some()
            || self.session.paused.is_some()
            || self.notice.is_some()
            || self.proj.notice.is_some()
            || self.proj.storage_problem.is_some()
    }

    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if let Some(f) = &self.session.fault {
            ui.colored_label(RED, format!("Fault: {f}. Run or Step restarts the program."));
        }
        if self.session.paused.is_some() {
            let at = self
                .paused_line()
                .map_or_else(String::new, |l| format!(" before line {l}"));
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(WARN, format!("Paused at a breakpoint{at}."))
                    .tip("The diagram and the values show the scan as it stood just before that statement.");
                ui.label(
                    RichText::new("F10 runs the rest of the scan. Ctrl Enter goes on to the next stop.")
                        .color(t.muted),
                );
            });
        }
        if let Some(n) = &self.notice {
            ui.label(RichText::new(n).color(t.muted));
        }
        if let Some((n, error)) = self.proj.notice.clone() {
            let r = ui.label(RichText::new(n).color(if error { RED } else { t.muted }));
            chrome.hit("notice", r.rect);
            if error && self.proj.pending.is_some() {
                let r = ui
                    .button("Replace")
                    .tip("Saves your version over the one kept in this browser.");
                chrome.hit("replace", r.rect);
                if r.clicked() {
                    self.proj.replace();
                    self.reload();
                }
            }
        }
        if let Some(why) = &self.proj.storage_problem {
            let r = ui.label(RichText::new(format!("⚠ {why}")).color(t.muted));
            chrome.hit("storage", r.rect);
        }
    }

    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let now = ui.ctx().input(|i| i.time);
        let stale = self.session.stale();
        let errors = self.session.diags.len();
        // An edit is only checked: what runs is the last deploy, as on a box.
        let undeployed = stale && self.pending.is_none() && self.session.build.is_some();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Structured Text").strong())
                .tip("The open file. It is checked as you type; Deploy runs it.");
            if self.pending.is_some() {
                ui.label(RichText::new("…").color(t.muted))
                    .tip("Checking the edit.");
            } else if errors > 0 {
                ui.label(RichText::new(format!("✖ {errors} error(s)")).color(RED))
                    .tip("The list under the code has each one. Click an error to go to its line.");
            } else {
                ui.label(RichText::new("✔ compiles").color(t.muted))
                    .tip("The program has no errors.");
            }
            if undeployed {
                let note = if errors > 0 {
                    "The program running is unchanged. Fix the errors, then Deploy."
                } else {
                    "The program running is unchanged. Deploy runs this version."
                };
                ui.label(RichText::new("· not deployed").color(t.muted)).tip(note);
            }
        });
        let all = self.diags();
        let current = self.proj.current.clone();
        let diags: Vec<(u32, u32)> = all
            .iter()
            .filter(|d| d.file == current)
            .map(|d| (d.line, d.col))
            .collect();
        let avail = ui.available_height();
        let list_h = if self.session.diags.is_empty() { 0.0 } else { 110.0 };
        let size = Vec2::new(ui.available_width(), (avail - list_h).max(120.0));
        let Some(text) = self.proj.current_text_mut() else {
            ui.label(RichText::new("No file open.").color(t.muted))
                .tip("Open or make one in Files.");
            return;
        };
        let r = ui
            .allocate_ui(size, |ui| {
                editor::st_editor(ui, egui::Id::unique(SOURCE_ID), text, &diags, MAX_SOURCE, chrome)
            })
            .inner;
        if r.changed() {
            self.edited(now);
        }
        // The selected lines, the Agent's scope while there are some. Read
        // while the editor has focus: an editor without it drops its
        // selection, and moving to the Agent tab takes the focus.
        let id = egui::Id::unique(SOURCE_ID);
        if ui.ctx().memory(|m| m.has_focus(id)) {
            self.selection = editor::selection(ui.ctx(), id).and_then(|(a, b)| {
                let text = self.proj.files.get(&current)?;
                crate::scope::selected_lines(text, a, b).map(|(from, to)| (current.clone(), from, to))
            });
        } else if self.selection.as_ref().is_some_and(|(f, ..)| *f != current) {
            self.selection = None;
        }
        if !self.session.diags.is_empty() {
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("pg-diags")
                .max_height(list_h)
                .show(ui, |ui| {
                    if let Some(d) = projects::diags_ui(ui, &all, chrome) {
                        self.goto(&d.file, d.line, chrome);
                    }
                });
        }
    }

    /// The projects (the visitor's, then the examples), the open one's
    /// files, and under them the program's outline: every block instance
    /// it calls, nested; a click shows that one's diagram.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let rows = self.proj.rows();
        let files: Vec<(String, bool)> = self
            .proj
            .files
            .keys()
            // The Agent's pages are the Agent tab's, not a file to open.
            .filter(|f| !helmsman_project::agent::is_agent_file(f))
            .map(|f| (f.clone(), self.proj.file_dirty(f)))
            .collect();
        let open = self.proj.open.clone();
        let current = self.proj.current.clone();
        let history: Vec<crate::history_ui::RevisionRow> =
            self.proj.revisions().iter().map(SimRevision::row).collect();
        let listing = Listing {
            projects: &rows,
            open: Some(&open),
            files,
            current: Some(&current),
            write: Ok(()),
            dirty: self.proj.dirty(),
            starters: false,
            running: self.proj.running.as_deref() == Some(open.as_str()),
            history: &history,
            // The page deploys at once: never busy.
            deploy: Ok(()),
        };
        if let Some(a) = self.files_ui.show(ui, &listing, chrome) {
            self.act(a);
        }
        ui.separator();
        ui.label(RichText::new("Program").strong().color(t.muted))
            .tip("The compiled program: each POU and the instances it calls. Click one to open its diagram.");
        match &mut self.viewer {
            Some(v) => {
                ui.indent("pg-tree", |ui| v.tree_ui(ui));
            }
            None => {
                ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            }
        }
        // The blank space under it all: a right-click offers a new project
        // (and a paste into the open one).
        if let Some(a) = self.files_ui.blank_menu(ui, chrome, &Ok(()), true) {
            self.act(a);
        }
    }

    fn devices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::devices::{LinkState, Live, tree};
        let text = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        let map = helmsman_project::iomap::skim(&text);
        let points: Vec<(String, Value)> = self
            .session
            .inputs()
            .chain(self.session.outputs())
            .map(|p| (p.name.clone(), p.value))
            .collect();
        let live = |name: &str| {
            points
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| Live {
                    value: Some(*v),
                    quality: None,
                    forced: false,
                })
        };
        let nodes = tree(&map, &live, &|_| LinkState::Simulated);
        let source = if text.is_empty() {
            String::new()
        } else {
            format!("io.map of {}", self.proj.open)
        };
        let write = if self.proj.is_example() {
            Err("The examples are read-only: copy the project first (File › Copy project).".into())
        } else {
            Ok(())
        };
        self.devices_ui.plant.states = self.plant_states(&map);
        self.devices_ui.plant.devices = crate::plant::device_choices(&map);
        self.devices_ui.plant.simulated = true;
        if self.devices_ui.show(ui, &nodes, &source, chrome, &write) == Some(crate::devices::DeviceAct::Scan)
        {
            self.devices_ui.simulated_scan(&map);
            #[cfg(target_arch = "wasm32")]
            self.library
                .fetch(helmsman_project::devlib::DEFAULT_URL, &ui.ctx().clone());
        }
    }

    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.trend_tab(ui, chrome);
    }

    fn messages(&mut self) -> &mut crate::messages::Log {
        &mut self.log
    }

    fn tabs(&self) -> &'static [Tab] {
        &[Tab::Io, Tab::Requirements, Tab::Claude]
    }

    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome) {
        match tab {
            Tab::Io => self.io(ui),
            Tab::Requirements => {
                let ctx = ui.ctx().clone();
                let act = egui::ScrollArea::vertical()
                    .id_salt("pg-tests")
                    .show(ui, |ui| {
                        let files = &self.proj.files;
                        let source = |f: &str| files.get(f).cloned();
                        let cannot_add = self.proj.is_example().then(|| {
                            "Make this example your own first: click it in Files and name it.".to_string()
                        });
                        let view = crate::requirements::View {
                            results: &self.tests,
                            adding: self.adding.as_ref(),
                            busy: false,
                            cannot_add,
                            source: &source,
                            agent: "the Agent (your API key or the local proxy)",
                        };
                        crate::requirements::ui(ui, chrome, &view, &mut self.req_ui)
                    })
                    .inner;
                match act {
                    Some(crate::requirements::Act::Add(text)) => self.add_requirement(&text, &ctx),
                    Some(crate::requirements::Act::Run) => self.run_tests(),
                    Some(crate::requirements::Act::Edit(f)) => {
                        self.proj.open_file(&f);
                        self.reload();
                        chrome.show_code();
                    }
                    Some(crate::requirements::Act::Delete(f)) => {
                        self.proj.delete_file(&f);
                        self.req_ui.open = None;
                        self.run_tests();
                    }
                    None => {}
                }
            }
            Tab::Claude => self.claude(ui, chrome),
        }
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        if let Some(v) = &mut self.viewer {
            v.icons
                .update(self.proj.files.get(crate::icons::FILE).map(String::as_str));
            v.crumbs_ui(ui, &[]);
            let area = ui.available_rect_before_wrap();
            v.breaks.clone_from(&self.session.breaks);
            v.paused = self.session.paused_at();
            v.canvas(ui, scan);
            let mut jump = None;
            if v.pick != self.last_pick {
                self.last_pick = v.pick;
                if let Some((line, _)) = v.picked() {
                    jump = Some(line);
                    // The later act wins: a pick is the scope now.
                    self.selection = None;
                }
            }
            let reads = v.reads_on_screen();
            self.map_inputs(ui, area, &reads);
            if let Some(line) = jump {
                match self.proj.locate(line) {
                    Some((file, l)) => self.goto(&file, l, chrome),
                    None => chrome.goto(line),
                }
            }
            return;
        }
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg);
        let why = self
            .session
            .build
            .as_ref()
            .and_then(|b| b.module_error.clone())
            .unwrap_or_else(|| "Write a PROGRAM that compiles to see its diagram.".into());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            why,
            FontId::proportional(crate::theme::BODY),
            t.muted,
        );
    }
}

impl PlaygroundView {
    fn io(&mut self, ui: &mut egui::Ui) {
        let t = theme(ui);
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            return;
        }
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        let outputs: Vec<Point> = self.session.outputs().cloned().collect();
        let on_map = self.on_map();
        let off_map: Vec<&Point> = inputs
            .iter()
            .filter(|p| !on_map.iter().any(|r| r.eq_ignore_ascii_case(&p.name)))
            .collect();
        egui::ScrollArea::vertical().id_salt("pg-io").show(ui, |ui| {
            let inputs_tip = if inputs.is_empty() {
                "No VAR_INPUT or %IX bit: add one to get a control."
            } else if off_map.len() < inputs.len() {
                "Switch, press or drag the inputs here, or on their blocks in the diagram."
            } else {
                "Switch, press or drag the inputs here."
            };
            ui.label(RichText::new("Inputs").strong()).tip(inputs_tip);
            if inputs.is_empty() {
                ui.label(RichText::new("none").color(t.muted)).tip(inputs_tip);
            }
            // Two columns when the panel is wide enough for them (Nick,
            // 2026-11-01: denser); each cell capped at its column's width,
            // or the row overflows and the resizable panel grows to fit.
            let (cols, col) = columns(ui);
            let mut set = Vec::new();
            egui::Grid::new("pg-io-in").num_columns(cols).show(ui, |ui| {
                for pair in off_map.chunks(cols) {
                    for p in pair {
                        ui.scope(|ui| {
                            ui.set_max_width(col);
                            if let Some(v) = input_ui(ui, p) {
                                set.push((p.name.clone(), v));
                            }
                        });
                    }
                    ui.end_row();
                }
            });
            for (name, v) in set {
                self.session.set_input(&name, v);
            }
            ui.add_space(8.0);
            let outputs_tip = if outputs.is_empty() {
                "No VAR_OUTPUT or %QX bit: add one to get a lamp."
            } else {
                "Click an output to pick its block in the diagram."
            };
            ui.label(RichText::new("Outputs").strong()).tip(outputs_tip);
            if outputs.is_empty() {
                ui.label(RichText::new("none").color(t.muted)).tip(outputs_tip);
            }
            let mut picked = None;
            let (cols, col) = columns(ui);
            egui::Grid::new("pg-io-out").num_columns(cols).show(ui, |ui| {
                for pair in outputs.chunks(cols) {
                    for p in pair {
                        ui.scope(|ui| {
                            ui.set_max_width(col);
                            if output_ui(ui, p) {
                                picked = Some(p.name.clone());
                            }
                        });
                    }
                    ui.end_row();
                }
            });
            if let Some(name) = picked {
                self.pick(&name);
            }
        });
    }

    /// The signals to trend (outputs, then inputs) and the chart.
    fn trend_tab(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            return;
        }
        ui.label(RichText::new("Signals").strong().color(t.muted))
            .tip(format!(
                "Click a signal to pin it to the trend, up to {TREND_SIGNALS}."
            ));
        let points: Vec<String> = self
            .session
            .outputs()
            .chain(self.session.inputs())
            .map(|p| p.name.clone())
            .collect();
        ui.horizontal_wrapped(|ui| {
            for name in &points {
                let on = self.session.trend.signals.contains(name);
                let tip = if on {
                    "On the trend. Click to take it off."
                } else {
                    "Click to pin it to the trend."
                };
                if ui
                    .selectable_label(on, name.to_ascii_lowercase())
                    .tip(tip)
                    .clicked()
                {
                    self.session.trend.toggle(name);
                }
            }
        });
        window_slider(ui, chrome, &mut self.session.trend.window_ms);
        ui.add_space(6.0);
        self.trend_plot(ui);
    }

    fn claude(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().id_salt("pg-claude").show(ui, |ui| {
            // Room for the scroll bar: a full-width field plus the bar is
            // wider than the panel, and a resizable panel grows to fit.
            ui.set_max_width(ui.available_width() - 12.0);
            match self.chat.proxy {
                Proxy::Yes => {
                    ui.label(RichText::new("Using the local proxy").color(t.muted))
                        .tip("The API key stays on this machine.");
                }
                Proxy::Checking => {
                    ui.label(RichText::new("Checking for a local proxy…").color(t.muted))
                        .tip("With a local proxy the API key stays on this machine.");
                }
                Proxy::No => self.key_ui(ui),
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Model").color(t.muted))
                    .tip("The model that answers. A larger one is slower and costs more.");
                // No wider than the panel: a row that overflows a resizable
                // panel widens it every frame.
                egui::ComboBox::from_id_salt("pg-model")
                    .width((ui.available_width() - 40.0).max(60.0))
                    .selected_text(self.chat.model.label())
                    .show_ui(ui, |ui| {
                        for m in Model::ALL {
                            ui.selectable_value(&mut self.chat.model, m, m.label())
                                .tip(format!("Answers with {}.", m.label()));
                        }
                    })
                    .response
                    .tip("The model that answers. A larger one is slower and costs more.");
            });
            ui.add_space(4.0);
            self.scope_ui(ui, chrome);
            ui.add(
                egui::TextEdit::multiline(&mut self.chat.question)
                    .id_salt("pg-question")
                    .hint_text("e.g. Add a stop button that resets the counter")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .tip(
                "Ask for a program (\"a conveyor that stops on a jam\"), a fix for the errors, or \
                 what a line does. The Agent sees the editor's program and changes only what the \
                 line above names.",
            );
            ui.horizontal(|ui| {
                let busy = self.chat.busy();
                let ask = egui::Button::new(RichText::new("Ask").strong().color(BLACK))
                    .fill(YELLOW)
                    .min_size(Vec2::new(TAP * 1.5, TAP * 0.75));
                if ui
                    .add_enabled(!busy, ask)
                    .tip(if busy {
                        "Wait for the answer, or Stop it."
                    } else {
                        "Sends the question with the editor's program and its errors."
                    })
                    .clicked()
                {
                    self.ask(&ctx);
                }
                if busy && ui.button("Stop").tip("Stops the answer under way.").clicked() {
                    self.stop_answer();
                }
                if !self.chat.history.is_empty()
                    && !busy
                    && ui
                        .button("New conversation")
                        .tip("The next question starts a conversation of its own.")
                        .clicked()
                {
                    self.chat.clear();
                }
            });
            if let Some(e) = &self.ask_error {
                ui.colored_label(RED, e);
            }
            ui.separator();
            self.pages_ui(ui, chrome);
            match self.chat.viewing().cloned() {
                Some(page) => self.page_ui(ui, &page, chrome),
                None => self.answer_ui(ui, chrome),
            }
        });
    }

    /// The page strip (Nick, 2026-11-01): ‹ › through the kept pages,
    /// "page k of n", Delete for the one on view. Every kept page goes to
    /// the model as context, so deleting one shapes what it sees.
    fn pages_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let n = self.chat.history.len();
        if n == 0 {
            return;
        }
        let at = self.chat.page;
        ui.horizontal_wrapped(|ui| {
            let prev = ui
                .add_enabled(at != Some(0), egui::Button::new("‹"))
                .tip(if at == Some(0) {
                    "This is the first page."
                } else {
                    "The page before."
                });
            chrome.hit("page prev", prev.rect);
            if prev.clicked() {
                self.chat.prev_page();
            }
            let next = ui
                .add_enabled(at.is_some(), egui::Button::new("›"))
                .tip(if at.is_some() {
                    "The page after. Past the last, the new answer."
                } else {
                    "This is the newest answer."
                });
            chrome.hit("page next", next.rect);
            if next.clicked() {
                self.chat.next_page();
            }
            let where_ = match at {
                Some(k) => format!("page {} of {n}", k + 1),
                None => format!("{n} page(s) kept"),
            };
            let l = ui
                .label(RichText::new(where_).color(t.muted))
                .tip("Every kept page goes to the model with the next question.");
            chrome.hit("page where", l.rect);
            if at.is_some() {
                let del = ui
                    .button("Delete page")
                    .tip("Drops this page from the conversation and from what the model sees.");
                chrome.hit("page delete", del.rect);
                if del.clicked() {
                    self.chat.delete_page();
                }
            }
        });
        ui.separator();
    }

    /// The Agent tab's scope line (card agent-scoped-edits): what the
    /// next question may change and which rule gave it, with Whole file
    /// while a selection or a pick narrows it.
    fn scope_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let scope = self.scope();
        let mut whole = false;
        // Wrapped: a long name in a plain row would widen the panel.
        ui.horizontal_wrapped(|ui| {
            let r = ui.label(RichText::new(scope.label()).strong()).tip(scope.rule());
            chrome.hit("agent scope", r.rect);
            if scope.lines().is_some() {
                let b = ui
                    .small_button("Whole file")
                    .tip("Drops the selection and the pick: the Agent edits the open file.");
                chrome.hit("scope whole file", b.rect);
                whole = b.clicked();
            }
        });
        if whole {
            self.whole_file(&ui.ctx().clone());
        }
        ui.add_space(4.0);
    }

    /// "Use this code" and, under it, why the last one was refused.
    fn use_button(&self, ui: &mut egui::Ui, chrome: &mut Chrome) -> bool {
        let b = egui::Button::new(RichText::new("Use this code").strong().color(BLACK))
            .fill(YELLOW)
            .min_size(Vec2::new(TAP * 3.0, TAP * 0.8));
        let r = ui.add(b).tip(
            "Takes the code into what it was asked about, if the requirement tests pass, and saves \
             it as a revision. Deploy runs it.",
        );
        chrome.hit("use code", r.rect);
        if let Some(why) = &self.use_error {
            let l = ui.colored_label(RED, why);
            chrome.hit("use refused", l.rect);
        }
        r.clicked()
    }

    /// An earlier page: its question, its answer, and Use this code when
    /// the answer holds code.
    fn page_ui(&mut self, ui: &mut egui::Ui, page: &crate::claude::Exchange, chrome: &mut Chrome) {
        let t = theme(ui);
        ui.label(RichText::new(page.question.trim()).strong().color(t.fg));
        ui.add_space(4.0);
        markdown(ui, &page.answer);
        if has_code(&page.answer) && self.use_button(ui, chrome) {
            self.use_page(page);
        }
    }

    fn key_ui(&mut self, ui: &mut egui::Ui) {
        let held = "Held in this page only; sent only to api.anthropic.com, billed to your account.";
        let key_tip = if self.chat.from_env {
            format!("{held} Preloaded from ANTHROPIC_API_KEY in this machine's environment.")
        } else {
            held.to_string()
        };
        ui.label(RichText::new("Anthropic API key").strong())
            .tip(&key_tip);
        let r = ui
            .add(
                egui::TextEdit::singleline(self.chat.key_mut())
                    .id_salt("pg-key")
                    .password(true)
                    .hint_text("sk-ant-…")
                    .desired_width(f32::INFINITY),
            )
            .tip(&key_tip);
        if r.changed() {
            self.chat.key_changed();
        }
        if self.chat.has_key() && !self.chat.key_looks_right() {
            ui.label(
                RichText::new("That does not look like an Anthropic key (sk-ant-…).")
                    .small()
                    .color(RED),
            );
        }
        let mut remember = self.chat.remember;
        if ui
            .checkbox(&mut remember, "Remember on this device")
            .tip("Stores the key in this browser's localStorage.")
            .changed()
        {
            self.chat.set_remember(remember);
        }
        if remember {
            ui.label(
                RichText::new(
                    "Stored in this browser's localStorage, readable by any script on this site. \
                     Untick, or Forget, on a shared computer.",
                )
                .small()
                .color(RED),
            );
        }
        // Wrapped: in a plain row a label extends, and a row wider than a
        // resizable panel widens it every frame.
        ui.horizontal_wrapped(|ui| {
            if self.chat.has_key()
                && ui
                    .button("Forget key")
                    .tip("Clears the key from this page and this browser.")
                    .clicked()
            {
                self.chat.forget_key();
            }
        });
        ui.add_space(6.0);
    }

    fn answer_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let Some(a) = &self.chat.answer else {
            return;
        };
        if let Some(m) = &a.model
            && m != self.chat.model.id()
        {
            ui.label(RichText::new(format!("answered by {m}")).small().color(t.muted));
        }
        markdown(ui, &a.text);
        if a.cut {
            ui.label(RichText::new("(answer cut: too long)").small().color(t.muted));
        }
        match &a.phase {
            Phase::Waiting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("thinking…").color(t.muted));
                });
            }
            Phase::Streaming => {
                ui.spinner();
            }
            Phase::Failed(why) => {
                ui.colored_label(RED, why);
            }
            Phase::Done => {}
        }
        if a.stop_reason.as_deref() == Some("max_tokens") {
            ui.label(
                RichText::new("(the answer hit its length limit)")
                    .small()
                    .color(t.muted),
            );
        }
        if a.phase == Phase::Done && has_code(&a.text) && self.use_button(ui, chrome) {
            self.use_answer();
        }
    }

    fn trend_plot(&self, ui: &mut egui::Ui) {
        let t = theme(ui);
        let trend = &self.session.trend;
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, t.chart);
        if trend.signals.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Pick signals to trend above",
                FontId::proportional(crate::theme::MIN_TEXT),
                t.muted,
            );
            return;
        }
        let n = trend.signals.len();
        #[allow(clippy::cast_precision_loss)]
        let lane = rect.height() / n as f32;
        let end = trend.samples.back().map_or(0, |s| s.t_ms);
        let window = trend.window_ms.max(1);
        let start = end.saturating_sub(window);
        // Samples older than the window are kept for a wider one: not drawn.
        let shown = trend.samples.iter().filter(|s| s.t_ms >= start);
        #[allow(clippy::cast_precision_loss)]
        let x_of = |t: u64| rect.left() + 90.0 + (rect.width() - 96.0) * ((t - start) as f32 / window as f32);
        for (k, name) in trend.signals.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let top = rect.top() + lane * k as f32;
            let color = match k % TREND.len() {
                5 if !t.dark => t.fg,
                i => TREND[i],
            };
            let (lo, hi) = shown
                .clone()
                .filter_map(|s| s.values.get(k).copied().filter(|v| v.is_finite()))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let last = trend.samples.back().and_then(|s| s.values.get(k).copied());
            painter.text(
                Pos2::new(rect.left() + 6.0, top + lane / 2.0),
                egui::Align2::LEFT_CENTER,
                format!(
                    "{}\n{}",
                    name.to_ascii_lowercase(),
                    last.filter(|v| v.is_finite())
                        .map_or("–".into(), |v| format!("{v}"))
                ),
                FontId::monospace(crate::theme::MIN_TEXT),
                color,
            );
            painter.line_segment(
                [Pos2::new(rect.left(), top), Pos2::new(rect.right(), top)],
                Stroke::new(1.0, t.rule),
            );
            let (lo, hi, step) = nice_range(lo, hi);
            scale_labels(&painter, &t, rect, top, lane, (lo, hi, step));
            let y_of = |v: f64| {
                #[allow(clippy::cast_possible_truncation)]
                let f = ((v - lo) / (hi - lo)) as f32;
                top + lane - 4.0 - (lane - 8.0) * f.clamp(0.0, 1.0)
            };
            let mut pts: Vec<Pos2> = Vec::new();
            for s in shown.clone() {
                let Some(v) = s.values.get(k).copied().filter(|v| v.is_finite()) else {
                    continue;
                };
                let p = Pos2::new(x_of(s.t_ms), y_of(v));
                // Steps, not slopes: a PLC value holds until the next scan.
                if let Some(prev) = pts.last().copied() {
                    pts.push(Pos2::new(p.x, prev.y));
                }
                pts.push(p);
            }
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
            }
        }
    }
}

/// The narrowest trend window, ms (Nick, 2026-11-02: down to 100 ms).
pub const WINDOW_MIN_MS: u64 = 100;
/// The widest, ms: the box's whole history (`box_trend::WINDOW_MS`).
pub const WINDOW_MAX_MS: u64 = 600_000;

/// A window's width as the slider reads it: `100 ms`, `2.5 s`, `100 s`,
/// whole minutes as `1 min`, `10 min`.
#[must_use]
pub fn window_label(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    let secs = ms / 1000;
    if ms < 10_000 && !ms.is_multiple_of(1000) {
        #[allow(clippy::cast_precision_loss)]
        return format!("{:.1} s", ms as f64 / 1000.0);
    }
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// The window's width as one slider, logarithmic from [`WINDOW_MIN_MS`]
/// to [`WINDOW_MAX_MS`], its value beside it (both trends; Nick,
/// 2026-11-02: a slider instead of buttons, exponential, changing the
/// plot as it moves). The hit is `trend window`. Returns whether it moved.
pub fn window_slider(ui: &mut egui::Ui, chrome: &mut Chrome, window_ms: &mut u64) -> bool {
    let t = theme(ui);
    let before = *window_ms;
    #[allow(clippy::cast_precision_loss)]
    let mut v = (*window_ms).clamp(WINDOW_MIN_MS, WINDOW_MAX_MS) as f64;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Window").color(t.muted))
            .tip("How much time the trend shows.");
        // The value label at the right needs ~52 pt; the slider takes the
        // rest, so the row never widens the panel.
        let label_w = 56.0;
        let w = (ui.available_width() - label_w - 12.0).max(40.0);
        ui.spacing_mut().slider_width = w;
        #[allow(clippy::cast_precision_loss)]
        let r = ui
            .add(
                egui::Slider::new(&mut v, WINDOW_MIN_MS as f64..=WINDOW_MAX_MS as f64)
                    .logarithmic(true)
                    .show_value(false),
            )
            .tip("Drag to show more or less time on the trend.");
        chrome.hit("trend window", r.rect);
        ui.add(
            egui::Label::new(
                RichText::new(window_label(round_window(v)))
                    .monospace()
                    .color(t.muted),
            )
            .truncate(),
        );
    });
    *window_ms = round_window(v);
    *window_ms != before
}

/// A slider position as a window: two significant digits, so the label
/// reads `2.5 s`, `25 s`, `250 s` rather than `2.473 s`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn round_window(v: f64) -> u64 {
    let v = v.clamp(WINDOW_MIN_MS as f64, WINDOW_MAX_MS as f64);
    let mag = 10f64.powf(v.log10().floor() - 1.0);
    ((v / mag).round() * mag)
        .round()
        .clamp(WINDOW_MIN_MS as f64, WINDOW_MAX_MS as f64) as u64
}

/// The lane's scale: its top and bottom values at the right edge, so a
/// line can be read (both trends draw them).
pub fn scale_labels(
    painter: &egui::Painter,
    t: &Theme,
    rect: Rect,
    top: f32,
    lane: f32,
    (lo, hi, step): (f64, f64, f64),
) {
    let x = rect.right() - 4.0;
    for (v, y, align) in [
        (hi, top + 3.0, egui::Align2::RIGHT_TOP),
        (lo, top + lane - 3.0, egui::Align2::RIGHT_BOTTOM),
    ] {
        painter.text(
            Pos2::new(x, y),
            align,
            tick_label(v, step),
            FontId::monospace(crate::theme::MIN_TEXT),
            t.muted,
        );
    }
}

impl PlaygroundView {
    /// Each input's control, drawn on its blocks in the diagram: a click
    /// flips a switch, a push button is on while held, a number drags
    /// sideways (or takes a typed value on a click).
    fn map_inputs(&mut self, ui: &mut egui::Ui, area: egui::Rect, reads: &[(String, egui::Rect)]) {
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        ui.set_clip_rect(area);
        // A push button read in two places is held if either is.
        let mut held: Vec<(String, bool)> = Vec::new();
        let mut set = Vec::new();
        for (k, (path, rect)) in reads.iter().enumerate() {
            let Some(p) = inputs.iter().find(|p| p.name.eq_ignore_ascii_case(path)) else {
                continue;
            };
            if !area.intersects(*rect) {
                continue;
            }
            match map_input_ui(ui, *rect, p, k) {
                MapInput::Held(on) => match held.iter_mut().find(|(n, _)| *n == p.name) {
                    Some((_, h)) => *h |= on,
                    None => held.push((p.name.clone(), on)),
                },
                MapInput::Set(v) => set.push((p.name.clone(), v)),
                MapInput::Nothing => {}
            }
        }
        for (name, on) in held {
            self.session.set_input(&name, Value::Bool(on));
        }
        for (name, v) in set {
            self.session.set_input(&name, v);
        }
    }

    /// Import device…: a vendor's EDS joins the project as `NAME.eds`
    /// (replacing one of that name) and the New device form opens filled
    /// in from it; the address is what is left to type.
    fn import_eds(&mut self, file: &str, bytes: &[u8]) {
        if self.proj.is_example() {
            self.proj.note(
                "The examples are read-only: copy the project first (File › Copy project).",
                true,
            );
            return;
        }
        let text = match helmsman_project::check_text(bytes) {
            Ok(t) => t.to_string(),
            Err(e) => {
                self.proj.note(format!("{file} was refused: {e}"), true);
                return;
            }
        };
        let Some(target) = helmsman_project::eds_file_name(file) else {
            self.proj.note(
                format!("{file}: nothing of the name survives as a file name (a-z 0-9 _ -)"),
                true,
            );
            return;
        };
        match self.proj.files.get_mut(&target) {
            Some(old) => old.clone_from(&text),
            None => self.proj.add_file(&target, text.clone()),
        }
        self.proj
            .note(format!("{file} is in {} as {target}.", self.proj.open), false);
        self.devices_ui.adding = Some(crate::devices::NewDevice::from_eds(&target, &text));
    }

    /// The Devices tab's Add: one `device` line onto the project's
    /// `io.map`, made if it has none.
    fn add_device_line(&mut self, line: &str) {
        match self.proj.files.get_mut(helmsman_project::IO_MAP) {
            Some(text) => *text = crate::devices::appended(text, line),
            None => self.proj.add_file(helmsman_project::IO_MAP, line.to_string()),
        }
    }

    /// The inputs the diagram shows a control for.
    pub(crate) fn on_map(&self) -> Vec<String> {
        let Some(v) = &self.viewer else { return Vec::new() };
        v.reads_on_screen().into_iter().map(|(path, _)| path).collect()
    }
}

/// What an input's control on the diagram did this frame.
enum MapInput {
    Nothing,
    /// A push button: whether it is held.
    Held(bool),
    Set(Value),
}

/// The "what is this program called?" question: the code waiting to
/// land, the name typed so far, and why the last one was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramName {
    pub text: String,
    pub input: String,
    pub error: Option<String>,
}

/// Whether an answer offers "Use this code": it has a closed code block
/// with something in it ([`crate::scope::apply`] decides the rest).
#[must_use]
pub fn has_code(answer: &str) -> bool {
    crate::claude::fences(answer)
        .iter()
        .any(|f| !f.body.trim().is_empty())
}

/// A name for the Agent's answer: the `PROGRAM`, `FUNCTION_BLOCK` or
/// `FUNCTION` it declares, lower-cased, else `agent`.
#[must_use]
pub fn answer_name(code: &str) -> String {
    let mut words = code.split_whitespace();
    while let Some(w) = words.next() {
        if matches!(
            w.to_ascii_uppercase().as_str(),
            "PROGRAM" | "FUNCTION_BLOCK" | "FUNCTION"
        ) && let Some(name) = words.next()
        {
            let name: String = name
                .trim_end_matches(':')
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .take(40)
                .collect::<String>()
                .to_ascii_lowercase();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "agent".into()
}

/// One input's control filling its block at `rect`.
fn map_input_ui(ui: &mut egui::Ui, rect: egui::Rect, p: &Point, k: usize) -> MapInput {
    let label = p.name.to_ascii_lowercase();
    let size = (rect.height() * 0.4).clamp(7.0, 18.0);
    let builder = egui::UiBuilder::new().max_rect(rect).id_salt(("pg-map", k));
    let id = egui::Id::unique(("pg-map-number", k));
    ui.scope_builder(builder, |ui| match p.widget {
        Some(Widget::Push) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(RichText::new(&label).strong().size(size).color(if on {
                BLACK
            } else {
                WHITE
            }))
            .fill(if on { GREEN } else { OFF_RED })
            // Never wrapped: a narrow block at low zoom would stack the
            // letters one a row.
            .wrap_mode(egui::TextWrapMode::Truncate)
            .sense(Sense::click_and_drag());
            let r = ui
                .add_sized(rect.size(), b)
                .on_hover_text("push button: on while held");
            MapInput::Held(r.is_pointer_button_down_on())
        }
        Some(Widget::Toggle) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(format!("{label} {}", if on { "TRUE" } else { "FALSE" }))
                    .strong()
                    .size(size)
                    .color(if on { BLACK } else { WHITE }),
            )
            .fill(if on { GREEN } else { OFF_RED })
            .wrap_mode(egui::TextWrapMode::Truncate);
            if ui
                .add_sized(rect.size(), b)
                .on_hover_text("switch: click to flip")
                .clicked()
            {
                MapInput::Set(Value::Bool(!on))
            } else {
                MapInput::Nothing
            }
        }
        Some(Widget::Int { min, max }) => {
            let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
            let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
            let v = match p.value {
                Value::Int(n) => i64::try_from(n).unwrap_or(0),
                _ => 0,
            };
            let c = crate::commit::number(ui, id, &v.to_string(), "", rect.size(), TYPE_HINT);
            match crate::commit::int(&c, lo, hi) {
                Some(n) if n != v => MapInput::Set(Value::Int(i128::from(n))),
                _ => MapInput::Nothing,
            }
        }
        Some(Widget::Real) => {
            let v = match p.value {
                Value::Real(r) => r,
                _ => 0.0,
            };
            let c = crate::commit::number(ui, id, &v.to_string(), "", rect.size(), TYPE_HINT);
            match crate::commit::real(&c) {
                Some(x) if x.to_bits() != v.to_bits() => MapInput::Set(Value::Real(x)),
                _ => MapInput::Nothing,
            }
        }
        Some(Widget::Time) => {
            let ms = match p.value {
                Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                _ => 0,
            };
            let c = crate::commit::number(ui, id, &ms.to_string(), "ms", rect.size(), TYPE_HINT);
            match crate::commit::int(&c, 0, 86_400_000) {
                Some(n) if n != ms => MapInput::Set(Value::Int(i128::from(n) * 1_000_000)),
                _ => MapInput::Nothing,
            }
        }
        None => MapInput::Nothing,
    })
    .inner
}

const TYPE_HINT: &str = "type a value, then Enter (Esc puts it back)";

impl Workspace for PlaygroundView {
    fn dirty(&self) -> bool {
        self.proj.dirty()
    }

    fn new_project(&mut self, chrome: &mut Chrome) {
        self.files_ui.new_project();
        chrome.show_files();
    }

    fn copy_project(&mut self, chrome: &mut Chrome) {
        let open = self.proj.open.clone();
        self.files_ui.copy_project(&open);
        chrome.show_files();
    }

    fn layout_menu(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let can = self.proj.can_save_layout();
        if crate::arrange::menu_items(ui, chrome, &mut self.layers, self.viewer.as_mut(), can) {
            self.save_layout();
        }
    }

    fn zoom_to_fit(&mut self) {
        if let Some(v) = &mut self.viewer {
            v.refit();
        }
    }

    fn go_to_definition(&mut self, chrome: &mut Chrome) -> bool {
        let Some(line) = self.viewer.as_ref().and_then(Viewer::definition_line) else {
            return false;
        };
        match self.proj.locate(line) {
            Some((file, l)) => self.goto(&file, l, chrome),
            None => chrome.goto(line),
        }
        chrome.show_code();
        true
    }

    fn cross_references(&mut self) -> bool {
        self.viewer.as_mut().is_some_and(Viewer::cross_references)
    }

    fn force_picked(&mut self, ctx: &egui::Context) -> bool {
        let Some((k, path)) = self.viewer.as_ref().and_then(Viewer::picked_read) else {
            return false;
        };
        let Some(p) = self
            .session
            .inputs()
            .find(|p| p.name.eq_ignore_ascii_case(&path))
            .cloned()
        else {
            return false;
        };
        match (p.widget, p.value) {
            (Some(Widget::Toggle), Value::Bool(on)) => self.session.set_input(&p.name, Value::Bool(!on)),
            // A push button is on only while held.
            (Some(Widget::Push) | None, _) => false,
            _ => {
                ctx.memory_mut(|m| m.request_focus(egui::Id::unique(("pg-map-number", k))));
                true
            }
        }
    }

    fn step_into(&mut self) -> bool {
        let opened = self.viewer.as_mut().is_some_and(Viewer::step_into);
        if opened && !self.session.running {
            self.session.step();
        }
        opened
    }

    fn step_out(&mut self) -> bool {
        let up = self.viewer.as_mut().is_some_and(Viewer::step_out);
        if up && !self.session.running {
            self.session.step();
        }
        up
    }

    fn toggle_breakpoint(&mut self) -> bool {
        let Some(id) = self.viewer.as_ref().and_then(Viewer::picked_stmt) else {
            return false;
        };
        self.session.toggle_break(id);
        true
    }

    fn undo(&mut self, ctx: &egui::Context, redo: bool) -> bool {
        let Some(text) = self.proj.current_text_mut() else {
            return false;
        };
        let done = editor::undo(ctx, egui::Id::unique("pg-source"), text, redo);
        if done {
            let t = ctx.input(|i| i.time);
            self.edited(t);
        }
        done
    }

    fn writable(&self) -> Result<(), String> {
        Ok(())
    }

    fn save(&mut self) {
        self.proj.save();
    }

    /// Compiles the open project and runs it in the page: the one place
    /// an edit reaches the program running. One that does not compile is
    /// refused, its errors listed, and the program running is unchanged.
    fn deploy(&mut self) {
        // A red requirement test refuses the deploy, naming it.
        if let Some(why) = self.tests_refuse(None) {
            self.notice = Some(why.clone());
            self.log.error(
                self.session.now_ms as f64 / 1000.0,
                format!("Deploy of {} refused. {why}", self.proj.open),
            );
            return;
        }
        let src = self.proj.build_src();
        let ok = self.session.set_source(&src);
        self.pending = None;
        self.sync_viewer();
        if ok {
            self.session.set_running(true);
            self.proj.running = Some(self.proj.open.clone());
            let program = self
                .viewer
                .as_ref()
                .and_then(|v| v.module.program())
                .map_or_else(String::new, |p| format!(": {}", p.name.to_ascii_lowercase()));
            self.notice = Some(format!("{} is running in this page{program}.", self.proj.open));
            self.log.info(
                self.session.now_ms as f64 / 1000.0,
                format!("Deployed {}: running in this page{program}.", self.proj.open),
            );
        } else {
            self.notice = Some(format!(
                "{} does not compile: {} problem(s), listed under the editor; click one to go there.",
                self.proj.open,
                self.session.diags.len()
            ));
            self.log.error(
                self.session.now_ms as f64 / 1000.0,
                format!(
                    "Deploy of {} refused: {} problem(s).",
                    self.proj.open,
                    self.session.diags.len()
                ),
            );
        }
    }

    fn busy(&self) -> bool {
        false
    }

    fn upload(&mut self, file: &str, bytes: Vec<u8>) {
        if crate::devices::is_eds(file) {
            self.import_eds(file, &bytes);
            return;
        }
        let before = (self.proj.open.clone(), self.proj.build_src());
        self.proj.upload(file, &bytes);
        if before.0 != self.proj.open {
            self.reload();
        } else if before.1 != self.proj.build_src() {
            self.session.src = self.proj.build_src();
            self.pending = Some(f64::NEG_INFINITY);
        }
    }

    fn download(&mut self) {
        match self.proj.zip() {
            Ok((name, bytes)) => {
                #[cfg(target_arch = "wasm32")]
                crate::web::download_bytes(&name, &bytes, "application/zip");
                #[cfg(not(target_arch = "wasm32"))]
                self.downloads.push((name, bytes));
            }
            Err(e) => self.proj.notice = Some((format!("Cannot download: {e}"), true)),
        }
    }
}

impl Default for PlaygroundView {
    fn default() -> Self {
        Self::new()
    }
}

/// One input's control; the value it sets, if it changed (a push button
/// reports every frame).
/// How many columns an I/O grid gets in `ui`, and each one's width: two
/// once the panel is wide enough for two controls, else one.
fn columns(ui: &egui::Ui) -> (usize, f32) {
    let avail = ui.available_width();
    let gap = ui.spacing().item_spacing.x;
    if avail >= 2.0 * TAP * 3.0 + gap {
        (2, (avail - gap) / 2.0)
    } else {
        (1, avail)
    }
}

fn input_ui(ui: &mut egui::Ui, p: &Point) -> Option<Value> {
    let t = theme(ui);
    let label = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let mut out = None;
    // Wrapped, with a slider that fits: a row wider than the panel grows it.
    ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(40.0, 100.0);
    ui.horizontal_wrapped(|ui| match p.widget {
        Some(Widget::Push) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(&label)
                    .strong()
                    .color(if on { BLACK } else { t.fg }),
            )
            .fill(if on { YELLOW } else { t.rule })
            .sense(Sense::click_and_drag())
            .wrap_mode(egui::TextWrapMode::Truncate)
            .min_size(Vec2::new(ui.available_width().min(TAP * 3.0), TAP * 0.8));
            let r = ui.add(b).tip("Push button: on while held.");
            out = Some(Value::Bool(r.is_pointer_button_down_on()));
        }
        Some(Widget::Toggle) => {
            let on = p.value == Value::Bool(true);
            let b = egui::Button::new(
                RichText::new(format!("{label}: {}", if on { "ON" } else { "OFF" }))
                    .strong()
                    .color(if on { BLACK } else { t.fg }),
            )
            .fill(if on { YELLOW } else { t.rule })
            .wrap_mode(egui::TextWrapMode::Truncate)
            .min_size(Vec2::new(ui.available_width().min(TAP * 3.0), TAP * 0.8));
            if ui.add(b).tip("Switch: click to flip.").clicked() {
                out = Some(Value::Bool(!on));
            }
        }
        Some(Widget::Int { min, max }) => {
            ui.label(&label);
            let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
            let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
            let (slo, shi) = (lo.max(-1000), hi.min(1000));
            let mut v = match p.value {
                Value::Int(n) => i64::try_from(n).unwrap_or(0),
                _ => 0,
            };
            let before = v;
            ui.add(egui::Slider::new(&mut v, slo..=shi).show_value(false))
                .tip(format!(
                    "Drag to set the input, {slo} to {shi}. Type any value in the box."
                ));
            if v != before {
                out = Some(Value::Int(i128::from(v)));
            }
            let id = egui::Id::unique(("io-int", &p.name));
            let c = crate::commit::number(
                ui,
                id,
                &before.to_string(),
                "",
                Vec2::new(84.0, TAP * 0.6),
                TYPE_HINT,
            );
            if let Some(n) = crate::commit::int(&c, lo, hi)
                && n != before
            {
                out = Some(Value::Int(i128::from(n)));
            }
        }
        Some(Widget::Real) => {
            ui.label(&label);
            let mut v = match p.value {
                Value::Real(r) => r,
                _ => 0.0,
            };
            let before = v;
            ui.add(egui::Slider::new(&mut v, -100.0..=100.0).show_value(false))
                .tip("Drag to set the input, -100 to 100. Type any value in the box.");
            if v.to_bits() != before.to_bits() && v.is_finite() {
                out = Some(Value::Real(v));
            }
            let id = egui::Id::unique(("io-real", &p.name));
            let c = crate::commit::number(
                ui,
                id,
                &before.to_string(),
                "",
                Vec2::new(84.0, TAP * 0.6),
                TYPE_HINT,
            );
            if let Some(x) = crate::commit::real(&c)
                && x.to_bits() != before.to_bits()
            {
                out = Some(Value::Real(x));
            }
        }
        Some(Widget::Time) => {
            ui.label(&label);
            let mut ms = match p.value {
                Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                _ => 0,
            };
            let before = ms;
            ui.add(egui::Slider::new(&mut ms, 0..=10_000).show_value(false))
                .tip("Drag to set the time, up to 10 s.");
            ui.add(egui::DragValue::new(&mut ms).range(0..=86_400_000).suffix(" ms"))
                .tip("The time in ms. Drag, or double-click to type it, up to a day.");
            if ms != before {
                out = Some(Value::Int(i128::from(ms) * 1_000_000));
            }
        }
        None => {}
    });
    out
}

/// One output: a lamp (BOOL) or a readout. Returns whether it was clicked.
fn output_ui(ui: &mut egui::Ui, p: &Point) -> bool {
    let t = theme(ui);
    let on = p.value == Value::Bool(true);
    let name = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let text = if p.ty == Ty::Bool {
        format!("{} {name}", if on { "●" } else { "○" })
    } else {
        format!("{name} = {}", show(p.ty, p.value))
    };
    let b = egui::Button::new(
        RichText::new(text)
            .monospace()
            .size(crate::theme::MIN_TEXT)
            .color(if on { t.accent } else { t.fg }),
    )
    .wrap_mode(egui::TextWrapMode::Truncate)
    .min_size(Vec2::new(ui.available_width(), TAP * 0.8));
    ui.add(b).tip("Click to pick its block in the diagram.").clicked()
}

/// A value for a readout (TIME in ms).
#[must_use]
pub fn show(ty: Ty, v: Value) -> String {
    match (ty, v) {
        (Ty::Time, Value::Int(ns)) => format!("{} ms", ns / 1_000_000),
        _ => v.to_string(),
    }
}

/// Answer text: paragraphs as labels, fenced code in monospace blocks.
fn markdown(ui: &mut egui::Ui, text: &str) {
    let t = theme(ui);
    for (k, s) in segments(text).into_iter().enumerate() {
        match s {
            Segment::Prose(p) => {
                let para = p.trim();
                if !para.is_empty() {
                    ui.label(RichText::new(para).color(t.fg));
                }
            }
            Segment::Code { body, .. } => {
                // A long line scrolls inside the block: a label that
                // extends past the panel widens it every frame (Nick,
                // 2026-11-01), and wrapping code misreads.
                let width = ui.available_width();
                egui::Frame::new()
                    .fill(t.bg)
                    .stroke(Stroke::new(1.0, t.rule))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.set_max_width(width - 12.0);
                        egui::ScrollArea::horizontal()
                            .id_salt(("pg-code", k))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(body).monospace().size(12.0))
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                    });
            }
        }
    }
}
