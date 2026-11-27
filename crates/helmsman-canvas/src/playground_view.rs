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
use crate::claude::{Chat, Phase, Proxy, Request};
use crate::editor::{self, Body, Chrome, Tab};
use crate::playground::{MAX_SOURCE, Point, Session, TREND_SIGNALS, Widget, nice_range, tick_label};
use crate::projects::{self, Act, Diag, FilesUi, Listing, Workspace};
use crate::sim_projects::{SimProjects, SimRevision, Storage};
use crate::st_view::brand::{BLACK, GREEN, OFF_RED, WARN, WHITE, YELLOW};
use crate::st_view::{Pick, TAP, Viewer};
use crate::theme::{Theme, theme};
use crate::tips::Tip;

/// The widest the card that says what the Agent noticed gets, pt.
const NOTICED_WIDTH: f32 = 620.0;

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Quiet time after the last keystroke before the editor checks, s.
const DEBOUNCE: f64 = 0.35;
/// Trend line colours, in signal order (the last, white, is the page's ink
/// in light mode).
pub const TREND: [Color32; TREND_SIGNALS] = [
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
    /// Requests sent with no answer expected (the survey).
    #[cfg(not(target_arch = "wasm32"))]
    pub posted: Vec<Request>,
    /// The free chats asked whether they are open, by address.
    #[cfg(not(target_arch = "wasm32"))]
    pub free_checks: Vec<String>,
    /// The design entries asked for their status, by address.
    #[cfg(not(target_arch = "wasm32"))]
    pub design_checks: Vec<String>,
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
            // Natively there is no proxy to find; the tests start with
            // one answering, so the Agent tab has a route (a test that
            // wants none says so with `Incoming::Proxy(false)`).
            self.inbox.push(crate::claude::Incoming::Proxy(cfg!(test)));
        }
    }

    /// Asks the free chat at `base` whether it is open. Natively there is
    /// no network here: a test hands the answer in through `inbox`.
    fn check_free(&mut self, base: &str, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.check_free(base, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.free_checks.push(base.to_string());
        }
    }

    /// Sends a request that expects no answer (the survey). In the
    /// browser nothing of the link is needed for that; natively the
    /// request is kept for the tests.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::unused_self))]
    fn post(&mut self, req: Request) {
        #[cfg(target_arch = "wasm32")]
        crate::web::post_and_forget(req);
        #[cfg(not(target_arch = "wasm32"))]
        self.posted.push(req);
    }

    /// Asks the design entry at `base` for its status. Natively a test
    /// hands the answer in through `inbox`.
    fn check_design(&mut self, base: &str, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.web.check_design(base, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.design_checks.push(base.to_string());
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
    /// The Agent's last change, marked on the diagram and in the code
    /// until the file is edited ([`crate::agent_marks`]).
    pub agent_change: Option<crate::agent_marks::AgentChange>,
    /// The revision the History tab runs, when it is not the newest.
    pub history_at: Option<u32>,
    /// The revision the Describe tab's arrows show, the newest included
    /// (`history_at` is clear there).
    pub exchange_at: Option<u32>,
    /// Its shared default layout (the project's) and this browser's
    /// tweaks (`arrange.rs`).
    pub layers: Layers,
    /// The viewer was just built: lay it out from the layers.
    fresh: bool,
    /// The menu a right click on the diagram opened
    /// (`crate::diagram_menu`).
    pub dmenu: crate::diagram_menu::Menu,
    /// The question "what value?" open on a literal of the diagram.
    pub value_ask: Option<crate::by_hand::ValueAsk>,
    /// The program's ports as last read, by the source they came from:
    /// the I/O tab and the diagram ask every frame.
    ports_memo: std::cell::RefCell<Option<(String, Vec<crate::device_form::Port>)>>,
    viewer_of: String,
    /// When the editor last changed and is not checked yet.
    pending: Option<f64>,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
    pub chat: Chat,
    pub link: Link,
    proxy_asked: bool,
    /// The free chat was asked whether it is open.
    free_asked: bool,
    /// The survey's "other" field while it is typed.
    survey_other: String,
    /// The device library's index, fetched when the Devices tab scans.
    #[cfg(target_arch = "wasm32")]
    library: crate::web::TextFetch,
    /// One wired device's `record.json`, fetched for the line under its point.
    #[cfg(target_arch = "wasm32")]
    record: crate::web::TextFetch,
    /// What the playground has read of the device library: the index and
    /// the records the map names (`wired.rs`).
    pub(crate) wired_lib: crate::wired::Library,
    /// A one-line note under the top bar (loaded an example, …).
    pub notice: Option<String>,
    /// The text of `notice` while it says the program is running in this
    /// page: it goes when the program stops, faults or restarts.
    running_notice: Option<String>,
    /// The text of `notice` while it says an input was set with nothing
    /// running ([`Self::idle_note`]): it goes when the program runs.
    idle_notice: Option<String>,
    /// The projects, kept in the browser's storage.
    pub proj: SimProjects,
    pub files_ui: FilesUi,
    pub devices_ui: crate::devices::DevicesUi,
    /// What `session.sensors` was bound from: the program running and the
    /// project's files, so a frame rebinds only after a change.
    sensors_from: Option<(String, Vec<(String, String)>)>,
    /// The question "what is this program called?", asked before the
    /// first program the Agent writes from a built-in example lands.
    pub program_name: Option<ProgramName>,
    /// The simulate-this-input popup a double-click on an input opened
    /// (`crate::sim_popup`).
    pub sim_popup: Option<crate::sim_popup::Popup>,
    /// Where each input's control was drawn last frame, by name (the
    /// diagram's blocks, then the I/O panel's rows): what a test clicks.
    input_rects: Vec<(String, Rect)>,
    /// The push buttons drawn this frame and whether each is held
    /// ([`Self::hold`]).
    pushed: Vec<(String, bool)>,
    /// The points whose details are open in the I/O tab, lower case.
    io_open: std::collections::BTreeSet<String>,
    /// The project the Agent's new programs land in, once named.
    pub new_program: Option<String>,
    /// The Agent tab (`crate::instruct`).
    pub instruct: crate::instruct::Instruct,
    /// The Agent's answer [`Self::settle`] last took.
    answered_seq: u64,
    /// A second way to the relay, for the question card's requests
    /// (`crate::ask`): they run beside the Agent's answer.
    pub side: Link,
    side_answer: crate::claude::Side,
    /// The question that goes with the instruction under way or just
    /// done.
    pub card: Option<crate::ask::Card>,
    /// The I/O tab's field for a start state in words, and its own way to
    /// the relay (`crate::start_state`): it never shares the question
    /// card's link, so neither drops the other's answer.
    pub start: crate::start_state::Card,
    pub start_link: Link,
    start_answer: crate::claude::Side,
    /// A third way to the relay, for the summary of a file's oldest
    /// changes (`instruct::compaction`): asked beside an instruction once
    /// the file's history is past what goes with one, never in its way.
    pub summary_link: Link,
    summary_answer: crate::claude::Side,
    /// The summary asked for and not yet answered, with its project.
    summarising: Option<(String, crate::instruct::Compact)>,
    /// A change that passed its checks and waits to be compared with the
    /// engineer's answer, and when that was asked (page time, s).
    held: Option<(crate::instruct::Apply, f64)>,
    /// A big ask held back for the engineer's pick: the draft it is for
    /// and the time it takes in one go ([`crate::instruct::estimate_secs`]).
    pub big: Option<(String, (u64, u64))>,
    /// That change, kept while the Agent corrects it to do what was
    /// answered: written after all if the correction does not pass.
    fallback: Option<crate::instruct::Apply>,
    /// When the request in flight went (page time, s): the Agent tab's clock.
    asked_at: f64,
    /// A checked new program waiting for its project's name.
    to_name: Option<crate::instruct::Apply>,
    /// The revision on view (the History modal).
    pub revision: Option<crate::history_ui::RevisionView>,
    /// The project's requirement tests, as last run in this page
    /// (`crate::requirements`): after every build change, before Deploy
    /// and "Use this code".
    pub tests: crate::requirements::Results,
    /// The points named ahead of a new program (`crate::io_names`).
    pub new_io: crate::io_names::IoList,
    /// The design step's entry (`crate::design_step`).
    pub design: crate::design_step::Designer,
    /// Its way to the relay's design entry.
    pub design_link: Link,
    /// The batch of the kept design the Agent is building: the project
    /// and how many steps it holds.
    design_building: Option<(String, usize)>,
    /// Steps of each project's design built in this page.
    design_built: std::collections::BTreeMap<String, usize>,
    /// The open project's `design.md` as last read: its hash, the design
    /// and what stops it being built from.
    design_cache: crate::design_step::KeptCache,
    /// When to say what went wrong (`crate::watch`).
    watching: crate::watch::Watching,
    /// What the Agent noticed, said under the top bar this frame.
    pub noticed: Option<crate::watch::Noticed>,
    /// The lines selected in the Code tab (file, first, last), as the
    /// editor last drew them: the Agent's scope when there are some.
    pub selection: Option<(String, u32, u32)>,
    /// The program file last open in the editor, with its project: the
    /// Agent's scope while a test, a map or a settings file is open.
    last_program: Option<(String, String)>,
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
        crate::web::clear_old_key();
        let proj = SimProjects::load(projects);
        let mut v = Self {
            session: Session::new(&proj.build_src()),
            viewer: None,
            agent_change: None,
            history_at: None,
            exchange_at: None,

            layers: Layers::load(tweaks),
            fresh: false,
            dmenu: crate::diagram_menu::Menu::default(),
            value_ask: None,
            ports_memo: std::cell::RefCell::new(None),
            viewer_of: String::new(),
            pending: None,
            last_pick: None,
            chat: Chat::new(),
            link: Link::default(),
            proxy_asked: false,
            free_asked: false,
            survey_other: String::new(),
            #[cfg(target_arch = "wasm32")]
            library: crate::web::TextFetch::default(),
            #[cfg(target_arch = "wasm32")]
            record: crate::web::TextFetch::default(),
            wired_lib: crate::wired::Library::default(),
            log: crate::messages::Log::default(),
            seen: Seen::default(),
            notice: None,
            running_notice: None,
            idle_notice: None,
            proj,
            files_ui: FilesUi::default(),
            devices_ui: crate::devices::DevicesUi::default(),
            sensors_from: None,
            program_name: None,
            sim_popup: None,
            input_rects: Vec::new(),
            pushed: Vec::new(),
            io_open: std::collections::BTreeSet::new(),
            new_program: None,
            instruct: crate::instruct::Instruct::default(),
            answered_seq: 0,
            side: Link::default(),
            side_answer: crate::claude::Side::default(),
            card: None,
            start: crate::start_state::Card::default(),
            start_link: Link::default(),
            start_answer: crate::claude::Side::default(),
            summary_link: Link::default(),
            summary_answer: crate::claude::Side::default(),
            summarising: None,
            held: None,
            big: None,
            fallback: None,
            asked_at: 0.0,
            to_name: None,
            revision: None,
            tests: crate::requirements::Results::default(),
            new_io: crate::io_names::IoList::default(),
            design: crate::design_step::Designer::page(),
            design_link: Link::default(),
            design_building: None,
            design_built: std::collections::BTreeMap::new(),
            design_cache: crate::design_step::KeptCache::default(),
            watching: crate::watch::Watching::default(),
            noticed: None,
            selection: None,
            last_program: None,
            #[cfg(not(target_arch = "wasm32"))]
            downloads: Vec::new(),
        };
        v.session.set_running(true);
        v.proj.running = Some(v.proj.open.clone());
        v.sync_viewer();
        v.run_tests();
        v
    }

    /// How this page runs the project's requirement tests: the session's
    /// scan period, the simulation's image sizes.
    fn test_settings(&self) -> helmsman_iec::spec::Settings {
        helmsman_iec::spec::Settings::new(crate::playground::SIZES)
            .period(std::time::Duration::from_millis(self.session.period_ms.max(1)))
    }

    /// Runs the open project's requirement tests against its build and
    /// keeps the results (the deploy gate reads them).
    pub fn run_tests(&mut self) {
        let plan = self.proj.test_plan();
        let plant = self
            .proj
            .files
            .get(helmsman_project::PLANT)
            .map_or("", String::as_str);
        let v = helmsman_sim::iec::run_all(&plan, self.test_settings(), plant);
        self.tests = crate::requirements::Results::from_verdicts(&v);
    }

    /// What "Use this code" and Deploy say of the project's requirement
    /// tests (with `extra` added or replacing a file, for a candidate):
    /// the red ones, named. A warning, never a refusal. Runs the tests on
    /// the candidate and, when it is the project as it stands, keeps the
    /// results.
    fn tests_warn(&mut self, extra: Option<(&str, &str)>) -> Option<String> {
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
                let plant = files.get(helmsman_project::PLANT).copied().unwrap_or("");
                crate::requirements::Results::from_verdicts(&helmsman_sim::iec::run_all(
                    &plan, settings, plant,
                ))
            }
        };
        results.warning()
    }

    /// Checks a PROGRAM answer (`text`) landing as `main.st` of the saved
    /// project `name`, with the answer's `scenarios` added: the project's
    /// own files with them, compiled and its requirement tests run, as the
    /// project will stand. The scenarios come back named free among the
    /// project's files (they were named free among the files of the open
    /// example the answer was checked against), with what there is to say
    /// of a red test (a warning). `Err`: it does not compile there, so it
    /// must not land.
    #[allow(clippy::type_complexity)]
    fn program_into(
        &self,
        name: &str,
        text: &str,
        scenarios: &[(String, String)],
    ) -> Result<(Vec<(String, String)>, Option<String>), String> {
        let Some(target) = self.proj.saved.get(name) else {
            return Ok((scenarios.to_vec(), None));
        };
        let mut files = target.0.clone();
        files.insert(helmsman_project::MAIN.to_string(), text.to_string());
        let mut taken: Vec<String> = files.keys().cloned().collect();
        let mut named = Vec::new();
        for (file, t) in scenarios {
            let want = file
                .trim_end_matches(".st")
                .trim_start_matches(helmsman_iec::spec::TEST_PREFIX);
            let free = format!(
                "{}{}.st",
                helmsman_iec::spec::TEST_PREFIX,
                crate::requirements::name_for(want, &taken)
            );
            taken.push(free.clone());
            files.insert(free.clone(), t.clone());
            named.push((free, t.clone()));
        }
        let plan = helmsman_iec::spec::plan(
            files
                .iter()
                .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
                .map(|(n, t)| (n.as_str(), t.as_str())),
        );
        if let Err(d) = helmsman_iec::compile(&plan.main, crate::playground::SIZES) {
            let first = d.first().map_or_else(String::new, ToString::to_string);
            return Err(format!(
                "Refused: with {name}'s other files the program does not compile: {first}"
            ));
        }
        let plant = files.get(helmsman_project::PLANT).map_or("", String::as_str);
        let verdicts = helmsman_sim::iec::run_all(&plan, self.test_settings(), plant);
        Ok((
            named,
            crate::requirements::Results::from_verdicts(&verdicts).warning(),
        ))
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
        self.proj.open_project(name);
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

    /// The compile errors, then the checker's warnings, each with its
    /// file and line.
    #[must_use]
    pub fn diags(&self) -> Vec<Diag> {
        let at = |d: &helmsman_iec::Diagnostic| {
            let (file, line) = self
                .proj
                .locate(d.pos.line)
                .unwrap_or_else(|| (self.proj.current.clone(), d.pos.line));
            (file, line, d.pos.col, d.msg.clone())
        };
        let errors = self.session.diags.iter().map(|d| {
            let (f, l, c, m) = at(d);
            Diag::error(f, l, c, m)
        });
        let warnings = self.session.warnings.iter().map(|d| {
            let (f, l, c, m) = at(d);
            Diag::warning(f, l, c, m)
        });
        let plant = self
            .session
            .plant_problems
            .iter()
            .map(|p| (p.line, p.msg.clone()))
            .chain(self.session.plant_unbound())
            .map(|(line, msg)| {
                Diag::warning(
                    helmsman_project::PLANT,
                    u32::try_from(line).unwrap_or(u32::MAX),
                    1,
                    msg,
                )
            });
        errors.chain(warnings).chain(plant).collect()
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
                // requirement tests run with it in, and a red one is said.
                // (A PROGRAM answer opens a new project, which has no
                // tests yet.)
                let mut red = None;
                if !text.to_ascii_uppercase().contains("PROGRAM ") {
                    let file = self.proj.free_file(&format!("{name}.st"));
                    red = self.tests_warn(Some((&file, &text)));
                }
                // A PROGRAM answer into a project that exists replaces its
                // main.st: it must compile with that project's files, its
                // tests run with it, and the scenarios take names free
                // there.
                let mut scenarios = self.to_name.as_ref().map(|a| a.tests.clone()).unwrap_or_default();
                if text.to_ascii_uppercase().contains("PROGRAM ") && self.proj.saved.contains_key(&name) {
                    match self.program_into(&name, &text, &scenarios) {
                        Ok((named, warn)) => {
                            scenarios = named;
                            red = warn;
                        }
                        Err(why) => {
                            self.notice = Some(why.clone());
                            self.log.error(
                                self.session.now_ms as f64 / 1000.0,
                                format!("Use this code: {why}"),
                            );
                            return;
                        }
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
                    // The scenarios its answer passed, kept as its tests.
                    for (f, t) in scenarios {
                        self.proj.files.insert(f, t);
                    }
                } else {
                    let file = self.proj.free_file(&format!("{name}.st"));
                    self.proj.new_file(&file);
                    if let Some(t) = self.proj.current_text_mut() {
                        *t = text;
                    }
                }
                // Recorded with the instruction that produced it.
                let apply = self.to_name.take();
                let what = apply.as_ref().map_or_else(
                    || format!("took the described change into {}", self.proj.current),
                    |a| {
                        format!("Described: {}", a.instruction)
                            .chars()
                            .take(120)
                            .collect()
                    },
                );
                self.proj
                    .save_as(&what, apply.as_ref().map(|a| a.record.as_str()));
                self.reload();
                if let Some(a) = &apply {
                    self.instruct.applied(a, None);
                    self.log_outcome();
                }
                let size = if dropped > 0 {
                    format!(" {dropped} oldest iteration(s) dropped: the project was past its size.")
                } else {
                    String::new()
                };
                let red = red.map_or_else(String::new, |why| {
                    self.log.warning(
                        self.session.now_ms as f64 / 1000.0,
                        format!("Use this code: {why}"),
                    );
                    format!(" {why}")
                });
                self.notice = Some(format!(
                    "{} opened from the answer; Deploy runs it.{size}{red}",
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
            v.show_output(name);
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
        text.push_str(&crate::device_io::describe_free(&map, &|f| {
            self.proj.files.get(f).cloned()
        }));
        text.push_str(&crate::devices::describe_found(&self.devices_ui.found));
        text.push_str(&crate::plant::describe(&self.plant_states(&map)));
        if let Some(plant) = self
            .proj
            .files
            .get(helmsman_project::PLANT)
            .filter(|t| !t.trim().is_empty())
        {
            text.push_str("\nSimulated plant (plant.sim): the devices the simulation and the requirement tests run the program against.\n");
            text.push_str(plant.trim_end());
            text.push('\n');
        }
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
        let unit = std::cell::OnceCell::new();
        let value = |pou: &str, name: &str| {
            let trace = self.session.trace.as_ref()?;
            let src = self.session.build.as_ref().map_or("", |b| b.src.as_str());
            let unit = unit.get_or_init(|| helmsman_iec::ast::parse(src).ok()).as_ref()?;
            crate::plant::value_in_trace(trace, unit, pou, name)
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
        self.session.set_plant(
            self.proj
                .files
                .get(helmsman_project::PLANT)
                .map_or("", String::as_str),
        );
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

    /// The project as the Agent sees it, in `scope`: what
    /// [`crate::instruct`] checks an answer against.
    fn agent_env(&self) -> (Vec<String>, String, helmsman_iec::spec::Settings) {
        (
            self.session.diag_lines(),
            self.devices_text(),
            self.test_settings(),
        )
    }

    /// Sends the typed instruction about the file in scope, with the last
    /// instructions carried out on that file (read from History).
    pub fn send_instruction(&mut self, ctx: &egui::Context) {
        self.send_instruction_as(ctx, crate::instruct::Way::Ask);
    }

    /// [`Self::send_instruction`], by `way`. A big ask ([`crate::steps::big_ask`])
    /// sent as [`Way::Ask`] goes nowhere: the draft stays and the tab
    /// offers the two ways ([`crate::instruct::big_ui`]). Sent again
    /// unchanged it goes in steps, the way on offer first.
    pub fn send_instruction_as(&mut self, ctx: &egui::Context, way: crate::instruct::Way) {
        use crate::instruct::Way;
        if let Err(why) = self.chat.route() {
            self.instruct.failed(why);
            return;
        }
        let scope = self.scope();
        // A new program: the points the engineer named go ahead of what
        // was typed, and a bad name stops it here.
        if scope == crate::scope::Scope::NewProject
            && let Some(problem) = self.new_io.problems().first()
        {
            self.instruct.failed(&format!("the I/O list: {problem}"));
            return;
        }
        let asked = if scope == crate::scope::Scope::NewProject {
            self.new_io.instruction(&self.instruct.draft)
        } else {
            self.instruct.draft.clone()
        };
        let lines = scope
            .file()
            .and_then(|f| self.proj.files.get(f))
            .map(|t| t.lines().count());
        // An Agent that answers with the lines that change is asked for
        // those, and a long file is then no reason to plan in steps: the
        // answer is as short as the change.
        let edits = self.chat.serves(crate::instruct::AGENT);
        self.instruct.edits = edits;
        let big = crate::steps::big_ask(&asked, lines.filter(|_| !edits));
        let plan = match way {
            Way::Steps => true,
            Way::Ask if big => {
                if let Some(why) = self.instruct.refusal() {
                    self.instruct.failed(&why);
                    return;
                }
                let seen = self.big.as_ref().is_some_and(|b| b.0 == self.instruct.draft);
                if !seen {
                    // Nothing goes to the model until the engineer picks.
                    let estimate = crate::instruct::estimate_secs(lines);
                    self.big = Some((self.instruct.draft.clone(), estimate));
                    return;
                }
                true
            }
            Way::OneGo | Way::Ask => false,
        };
        self.big = None;
        self.instruct.draft = asked;
        let file = crate::instruct::target(&scope);
        // From the project's history log, which outlives its revisions.
        let log = self.proj.log();
        let earlier = crate::instruct::history(log.iter().map(String::as_str), &file);
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope,
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        match self.instruct.start_with(&env, &earlier, plan) {
            Ok(ask) => {
                self.asked_at = ctx.input(|i| i.time);
                self.send_ask(&ask, ctx);
                self.ask_question(plan, ctx);
                self.ask_summary(&earlier, &file, log.len(), ctx);
            }
            Err(why) => self.instruct.failed(&why),
        }
    }

    /// Beside the instruction just sent, when `file`'s history (`earlier`)
    /// is past what goes with one, or the log (`logged` entries) is near
    /// what the project keeps: asks the relay's `summary` agent to write
    /// up the file's oldest changes, once (`instruct::compaction`). The
    /// instruction itself went with what fits; the summary is for the
    /// next one.
    fn ask_summary(&mut self, earlier: &[String], file: &str, logged: usize, ctx: &egui::Context) {
        use crate::instruct::{HISTORY_TOKENS, SUMMARY_AGENT};
        if self.summarising.is_some() || self.proj.is_example() || !self.chat.serves(SUMMARY_AGENT) {
            return;
        }
        // The log holds so many pages (`agent::MAX_PAGES`); the oldest
        // would fall off its front unsummarised. Three quarters full,
        // half of this file's history is written up.
        let size: usize = earlier.iter().map(|e| crate::instruct::tokens(e) + 2).sum();
        let budget = if logged * 4 >= helmsman_project::agent::MAX_PAGES * 3 {
            HISTORY_TOKENS.min(size / 2)
        } else {
            HISTORY_TOKENS
        };
        let Some(ask) = crate::instruct::compaction(earlier, file, budget) else {
            return;
        };
        let Ok(route) = self.chat.route() else { return };
        let turn = self.chat.takes.allow(crate::claude::Turn {
            agent: Some(SUMMARY_AGENT),
            quick: false,
        });
        let req = crate::claude::request_turn(&route, &[], &ask.message, turn);
        let seq = self.summary_answer.start();
        self.summary_link.send(seq, req, ctx);
        self.summarising = Some((self.proj.open.clone(), ask));
    }

    /// The summary's answer, once it is whole, takes the place of the
    /// changes it covers in its project's history log. One for a project
    /// no longer open, or an answer that is no summary, is dropped: it is
    /// asked for again with the next instruction there.
    fn summary_settle(&mut self) {
        for m in self.summary_link.drain() {
            self.summary_answer.receive(m);
        }
        if let Some(done) = self.summary_answer.take()
            && let Some((project, ask)) = self.summarising.take()
            && project == self.proj.open
            && let Ok(text) = done
            && let Some(page) = crate::instruct::summary_page(&ask, &text)
        {
            self.proj.log_summarise(&ask, &page);
        }
    }

    /// Beside the instruction just sent: asks the relay's `ask` agent for
    /// one question about it (`crate::ask`). Not for one planned in
    /// steps.
    fn ask_question(&mut self, plan: bool, ctx: &egui::Context) {
        self.drop_question();
        if plan || !self.chat.serves(crate::ask::AGENT) {
            return;
        }
        let Some(run) = &self.instruct.run else { return };
        let program = if run.scope == crate::scope::Scope::NewProject {
            ""
        } else {
            self.session.src.as_str()
        };
        let message = crate::ask::message(&run.instruction, program);
        self.side_send(crate::ask::AGENT, &message, ctx);
        self.card = Some(crate::ask::Card::default());
    }

    /// One request to `agent` on the second link.
    fn side_send(&mut self, agent: &str, message: &str, ctx: &egui::Context) {
        let Ok(route) = self.chat.route() else { return };
        let turn = self.chat.takes.allow(crate::claude::Turn {
            agent: Some(agent),
            quick: false,
        });
        let req = crate::claude::request_turn(&route, &[], message, turn);
        let seq = self.side_answer.start();
        self.side.send(seq, req, ctx);
    }

    /// The question and everything waiting on it are dropped.
    fn drop_question(&mut self) {
        self.side_answer.stop();
        self.side.abort();
        self.card = None;
        self.held = None;
        self.fallback = None;
    }

    /// The instruction's environment for the run under way, as
    /// [`Self::settle`] builds it.
    fn with_env<T>(
        &mut self,
        f: impl FnOnce(&mut crate::instruct::Instruct, &crate::instruct::Env<'_>) -> T,
    ) -> Option<T> {
        let scope = self.instruct.run.as_ref().map(|r| r.scope.clone())?;
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope,
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        Some(f(&mut self.instruct, &env))
    }

    /// The engineer clicked answer `k` of the question on show. Before
    /// the Agent has written anything the instruction goes out again with
    /// the answer in it; later the answer waits for the change
    /// ([`Self::landed`]).
    fn answer_question(&mut self, k: usize, ctx: &egui::Context) {
        let drafted = self.held.is_some()
            || self
                .chat
                .answer
                .as_ref()
                .is_none_or(|a| !a.running() || !a.text.trim().is_empty());
        let Some(card) = &mut self.card else { return };
        if !card.pick(k, drafted) {
            return;
        }
        let Some((q, k)) = card.question.clone().zip(card.picked) else {
            return;
        };
        let Some(text) = self
            .instruct
            .run
            .as_ref()
            .map(|r| crate::ask::with_answer(&r.instruction, &q, k))
        else {
            return;
        };
        match self.with_env(|ins, env| ins.resend(env, &text)).flatten() {
            Some(ask) => {
                self.chat.stop();
                self.link.abort();
                self.asked_at = ctx.input(|i| i.time);
                self.send_ask(&ask, ctx);
            }
            // It cannot go out again (a retry is under way): compared
            // with the change instead.
            None => {
                if let Some(card) = &mut self.card {
                    card.state = crate::ask::State::Waiting;
                }
            }
        }
    }

    /// A checked change written into the open project, for the tests.
    #[cfg(test)]
    pub fn write_for_test(&mut self, apply: crate::instruct::Apply) {
        self.write(apply);
    }

    /// A click on answer `k`, for the tests.
    #[cfg(test)]
    pub fn answer_question_for_test(&mut self, k: usize) {
        self.answer_question(k, &egui::Context::default());
    }

    /// A change passed its checks. With an answer still to compare it
    /// with, it is held while the relay's `check` agent looks
    /// ([`Self::verdict`]); else it is written.
    fn landed(&mut self, apply: crate::instruct::Apply, ctx: &egui::Context) {
        use crate::ask::State;
        if self.fallback.take().is_some()
            && let Some(card) = &mut self.card
        {
            // The Agent's correction passed.
            card.state = State::Covered;
        }
        let direct = self
            .instruct
            .run
            .as_ref()
            .is_some_and(|r| r.mode == crate::instruct::Mode::Direct);
        let to_check = self
            .card
            .as_ref()
            .and_then(|c| c.to_check())
            .map(|(q, k)| crate::ask::check_message(&apply.instruction, q, k, &apply.text));
        match to_check {
            Some(message) if direct && self.chat.serves(crate::ask::CHECK_AGENT) => {
                self.side_answer.stop();
                self.side.abort();
                self.side_send(crate::ask::CHECK_AGENT, &message, ctx);
                if let Some(card) = &mut self.card {
                    card.state = State::Checking;
                }
                self.held = Some((apply, ctx.input(|i| i.time)));
            }
            Some(_) => {
                if let Some(card) = &mut self.card {
                    card.state = State::Unchecked;
                }
                self.write_answered(apply);
            }
            None => self.write_answered(apply),
        }
    }

    /// Writes `apply`, with the question and the answer it was made or
    /// compared with on its History page.
    fn write_answered(&mut self, mut apply: crate::instruct::Apply) {
        if let Some((q, k)) = self.card.as_ref().and_then(|c| c.answered()) {
            if !apply.record.ends_with('\n') {
                apply.record.push('\n');
            }
            apply.record.push_str(&crate::ask::record(q, k));
            apply.record.push('\n');
        }
        self.write(apply);
    }

    /// What the `check` agent found for the change on hold: one that
    /// differs from the answer goes back to the Agent once; anything else
    /// (it agrees, no clear verdict, no answer in time) is written.
    fn verdict(&mut self, verdict: Option<crate::ask::Verdict>, ctx: &egui::Context) {
        use crate::ask::{State, Verdict};
        let Some((apply, _)) = self.held.take() else {
            return;
        };
        let picked = self.card.as_ref().and_then(|c| c.question.clone().zip(c.picked));
        if verdict == Some(Verdict::Differs)
            && let Some((q, k)) = picked
        {
            let answer = self
                .chat
                .answer
                .as_ref()
                .map(|a| a.text.clone())
                .unwrap_or_default();
            let text = crate::ask::differs(&q, k);
            if let Some(ask) = self.with_env(|ins, env| ins.amend(env, &answer, &text)).flatten() {
                if let Some(card) = &mut self.card {
                    card.state = State::Amending;
                }
                self.fallback = Some(apply);
                self.send_ask(&ask, ctx);
                return;
            }
        }
        if let Some(card) = &mut self.card {
            card.state = match verdict {
                Some(Verdict::Agrees) => State::Covered,
                Some(Verdict::Differs) => State::Kept,
                None => State::Unchecked,
            };
        }
        self.write_answered(apply);
    }

    /// The question card's requests, each frame: the question when it
    /// arrives, the verdict when it does (or when it has taken too long:
    /// the check is never what the engineer waits for), and a question
    /// nobody answered goes with its instruction.
    fn side_settle(&mut self, ctx: &egui::Context) {
        for m in self.side.drain() {
            self.side_answer.receive(m);
        }
        let now = ctx.input(|i| i.time);
        let done = self.side_answer.take();
        if let Some((_, since)) = &self.held {
            if let Some(text) = done {
                self.verdict(text.ok().as_deref().and_then(crate::ask::verdict), ctx);
            } else if now - since > crate::ask::CHECK_PATIENCE {
                self.side_answer.stop();
                self.side.abort();
                self.verdict(None, ctx);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        } else if let Some(text) = done
            && let Some(card) = &mut self.card
        {
            match text {
                Ok(text) => card.asked(&text),
                Err(_) => self.card = None,
            }
        }
        self.tidy_question();
    }

    /// Whether the I/O tab offers a start state in words: only on the free
    /// chat whose status lists the `start` agent (a page that cannot ask
    /// it shows no field).
    #[must_use]
    pub fn offers_start(&self) -> bool {
        self.session.build.is_some() && self.chat.serves(crate::start_state::AGENT)
    }

    /// Sends the field's words to the relay's `start` agent with the
    /// program's inputs. Not while the program runs.
    fn start_send(&mut self, ctx: &egui::Context) {
        if self.session.running
            || self.start.draft.trim().is_empty()
            || self.start.phase != crate::start_state::Phase::Idle
            || !self.offers_start()
        {
            return;
        }
        let Ok(route) = self.chat.route() else { return };
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        let outputs: Vec<Point> = self.session.outputs().cloned().collect();
        let message = crate::start_state::message_for(&self.start.draft, &inputs, &outputs);
        let turn = self.chat.takes.allow(crate::claude::Turn {
            agent: Some(crate::start_state::AGENT),
            quick: false,
        });
        let req = crate::claude::request_turn(&route, &[], &message, turn);
        let seq = self.start_answer.start();
        self.start_link.send(seq, req, ctx);
        self.start.asked();
    }

    /// Stops the request, or drops what it proposed.
    fn start_stop(&mut self) {
        self.start_answer.stop();
        self.start_link.abort();
        self.start.stopped();
    }

    /// The request's answer, once it is whole; a proposal made for a
    /// program that has since changed goes; so does anything under way
    /// once the program runs (an Agent never moves a running machine).
    fn start_settle(&mut self, ctx: &egui::Context) {
        use crate::start_state::Phase;
        for m in self.start_link.drain() {
            self.start_answer.receive(m);
        }
        if self.session.running && self.start.phase == Phase::Working {
            self.start_stop();
            self.start.line = Some("The program started, so the request was dropped.".into());
        }
        if let Some(done) = self.start_answer.take() {
            if self.start.phase == Phase::Working {
                let inputs: Vec<Point> = self.session.inputs().cloned().collect();
                let outputs: Vec<Point> = self.session.outputs().cloned().collect();
                let src = self.session.src.clone();
                self.start.answered(
                    done.as_deref().map_err(String::as_str),
                    &src,
                    &inputs,
                    &outputs,
                    &self.proj.files,
                );
            }
        } else if self.start.phase == Phase::Working {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if matches!(&self.start.phase, Phase::Proposed(p) if p.program != self.session.src) {
            self.start.stopped();
            self.start.line = Some("The program changed, so the proposal was dropped.".into());
        }
    }

    /// Why the start-state field cannot ask now, in a line, when it
    /// cannot: there is nobody to ask, or the relay does not serve it.
    #[must_use]
    pub fn start_unavailable(&self) -> Option<&'static str> {
        if self.chat.serves(crate::start_state::AGENT) {
            return None;
        }
        Some(
            if matches!(self.chat.route(), Ok(crate::claude::Route::Free(_))) {
                "This relay does not serve start states, so nothing can be asked. Set the inputs by hand."
            } else {
                "Start states are asked of Helmsman's free chat, which is not open here. Set the inputs by hand."
            },
        )
    }

    /// Rename `k` of the proposal, clicked: the whole project is renamed
    /// ([`Self::rename_everywhere`]) and the proposal follows the new name.
    fn start_rename(&mut self, k: usize) {
        use crate::start_state::Phase;
        let Phase::Proposed(p) = &self.start.phase else {
            return;
        };
        let Some(crate::offers::What::Rename { old, new }) = p.offers.get(k).map(|o| o.what.clone()) else {
            return;
        };
        let before = self.proj.files.clone();
        let was_running = self.session.running;
        self.rename_everywhere(&old, &new);
        // A rename is no reason to start the machine: values are set
        // before it starts, and never on a running one.
        if !was_running {
            self.session.set_running(false);
        }
        if self.proj.files == before {
            self.start.line = Some(format!("{old} was not renamed. Nothing changed."));
            return;
        }
        let src = self.session.src.clone();
        self.start.renamed(k, &src);
        let names: Vec<String> = self.session.inputs().map(|p| p.name.clone()).collect();
        self.start.respell(&names);
    }

    /// Sets what the Agent proposed, each value as set by hand, if the
    /// program is still not running and still the text it was made for.
    fn start_accept(&mut self) {
        let src = self.session.src.clone();
        let set: Vec<(String, Value)> = match self.start.accept(self.session.running, &src) {
            Ok(p) => p.changes.iter().map(|c| (c.name.clone(), c.new)).collect(),
            Err(why) => {
                self.start.line = Some(why);
                return;
            }
        };
        let n = set.len();
        for (name, v) in set {
            self.set_by_hand(&name, v);
        }
        self.start.stopped();
        self.start.draft.clear();
        self.start.line = Some(format!("Set {n} input{}.", if n == 1 { "" } else { "s" }));
    }

    /// The start-state field above the Inputs, and under it the working
    /// state, the quiet line or the proposal. Every row wraps and is
    /// capped to the tab's width, with 12 pt left for the scroll bar.
    fn start_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::start_state::Phase;
        let t = theme(ui);
        let off = self.start_unavailable();
        let running = self.session.running;
        let working = self.start.phase == Phase::Working;
        let width = (ui.available_width() - 12.0).max(120.0);
        ui.set_max_width(width);
        let mut send = false;
        let mut stop = false;
        let mut say_off = false;
        let glyph = crate::st_view::TAP * 1.5;
        ui.horizontal(|ui| {
            let field_w = (width - glyph - ui.spacing().item_spacing.x - 4.0).max(80.0);
            let tip = if let Some(why) = off {
                why
            } else if running {
                "Stop the program first: start values are never changed on a running machine."
            } else {
                "Describe the state the machine should start in, in your own words, for example \
                 part seated, guard open, speed 40. The answer is input values, as a \
                 proposal; nothing is set until you accept it. Enter sends."
            };
            let out = ui.add_enabled_ui(!running && !working && off.is_none(), |ui| {
                egui::TextEdit::singleline(&mut self.start.draft)
                    .id_salt("start-state")
                    .hint_text(if off.is_some() { "Not available here" } else { "Describe a start state…" })
                    .char_limit(crate::start_state::MAX_DESCRIPTION)
                    .desired_width(field_w)
                    .show(ui)
                    .tip(tip)
            });
            let field = out.inner;
            chrome.hit("start field", field.response.response.rect);
            let enter = field.response.response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if working {
                let r = crate::glyphs::action(ui, crate::glyphs::Icon::Stop, true, false)
                    .tip("Stop: drops the request. Nothing is set.");
                chrome.hit("start stop", r.rect);
                stop = r.clicked();
            } else {
                let why_not = if let Some(why) = off {
                    Some(why)
                } else if running {
                    Some("Stop the program first: start values are never changed on a running machine.")
                } else if self.start.draft.trim().is_empty() {
                    Some("Write a start state first.")
                } else {
                    None
                };
                // Off, the send glyph stays live to say why in a line.
                let live = why_not.is_none() || off.is_some();
                let r = crate::glyphs::action(ui, crate::glyphs::Icon::Send, live, true).tip(
                    why_not.unwrap_or(
                        "Send: asks for input values that match this description. They are shown as a proposal first.",
                    ),
                );
                chrome.hit("start send", r.rect);
                send = (r.clicked() || enter) && why_not.is_none();
                say_off = r.clicked() && off.is_some();
            }
        });
        if say_off {
            self.start.line = off.map(str::to_owned);
        }
        if working {
            let r = ui
                .add(
                    egui::Label::new(
                        RichText::new("Working…")
                            .color(t.muted)
                            .size(crate::theme::MIN_TEXT),
                    )
                    .wrap(),
                )
                .tip("Reading your description. Stop it with the square beside the field.");
            chrome.hit("start working", r.rect);
        }
        if let Some(line) = self.start.line.clone() {
            let r = ui
                .add(egui::Label::new(RichText::new(line).color(t.muted).size(crate::theme::MIN_TEXT)).wrap())
                .tip("What happened to your description. Nothing was set.");
            chrome.hit("start line", r.rect);
        }
        let mut accept = false;
        let mut discard = false;
        let mut pick = None;
        if let Phase::Proposed(p) = &self.start.phase {
            let frame = egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, t.muted.gamma_multiply(0.5)))
                .corner_radius(4.0)
                .inner_margin(6.0);
            let r = frame
                .show(ui, |ui| {
                    ui.set_width(width - 14.0);
                    let r = ui
                        .add(
                            egui::Label::new(
                                RichText::new(if p.changes.is_empty() {
                                    "Proposed names:"
                                } else {
                                    "Proposed start values:"
                                })
                                    .strong()
                                    .size(crate::theme::MIN_TEXT),
                            )
                            .wrap(),
                        )
                        .tip("Nothing is set yet. Set these puts every value below into its input, as if you had set it by hand.");
                    chrome.hit("start proposal", r.rect);
                    if !p.offers.is_empty() {
                        pick = crate::offers::ui(ui, chrome, &p.offers);
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("start-proposal")
                        .max_height(160.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width() - 12.0);
                            for c in &p.changes {
                                let line = format!(
                                    "{}: {} → {}",
                                    c.name,
                                    show(c.ty, c.old),
                                    show(c.ty, c.new)
                                );
                                ui.add(egui::Label::new(RichText::new(line).size(crate::theme::MIN_TEXT)).wrap())
                                    .tip("An input this would change: now → proposed.");
                            }
                        });
                    ui.horizontal_wrapped(|ui| {
                        if !p.changes.is_empty() {
                            let go = ui
                                .add_enabled(
                                    !running,
                                    egui::Button::new(RichText::new("Set these").size(crate::theme::MIN_TEXT)),
                                )
                                .tip(if running {
                                    "Stop the program first: start values are never changed on a running machine."
                                } else {
                                    "Put these values into the inputs. The program then starts with them."
                                });
                            chrome.hit("start set", go.rect);
                            accept = go.clicked();
                        }
                        let no = ui
                            .add(egui::Button::new(RichText::new("Discard").size(crate::theme::MIN_TEXT)))
                            .tip("Drop the proposal. Nothing is set.");
                        chrome.hit("start discard", no.rect);
                        discard = no.clicked();
                    });
                })
                .response;
            chrome.hit("start card", r.rect);
        }
        ui.add_space(4.0);
        match pick {
            Some(crate::offers::Pick::Take(k)) => self.start_rename(k),
            Some(crate::offers::Pick::Leave) => {
                if let Phase::Proposed(p) = &mut self.start.phase {
                    p.offers.clear();
                    if p.changes.is_empty() {
                        self.start.phase = Phase::Idle;
                    }
                }
            }
            None => {}
        }
        if stop || discard {
            self.start_stop();
        } else if accept {
            self.start_accept();
        } else if send {
            self.start_send(&ui.ctx().clone());
        }
    }

    /// A card with nothing on it, and one nobody answered once its
    /// instruction is over, go.
    fn tidy_question(&mut self) {
        let unanswered = self.card.as_ref().is_some_and(|c| c.picked.is_none());
        let no_question = self
            .card
            .as_ref()
            .is_some_and(|c| c.state != crate::ask::State::Asking && c.shown().is_none());
        if no_question || (unanswered && !self.instruct.busy()) {
            self.drop_question();
        }
    }

    // ---- the design step (`crate::design_step`) ----

    /// Why a design cannot be asked for now, if it cannot: what is not the
    /// design entry's own words (those are said beside the entry).
    fn design_blocked(&self) -> Option<String> {
        if self.instruct.busy() {
            return Some("Still working on the last instruction.".into());
        }
        self.design
            .busy()
            .then(|| "A design is being written.".to_string())
    }

    /// Asks the relay's design entry for the design of the machine in the
    /// description window: the project's program, I/O and requirement
    /// tests go with it as data.
    pub fn send_design(&mut self, ctx: &egui::Context) {
        if let Some(why) = self.design_blocked() {
            self.design.error = Some(why);
            return;
        }
        let now = ctx.input(|i| i.time);
        let description = self.design.description.clone();
        // The documents travel with the description, inside their envelope.
        let message = self.design.docs.message(&description);
        let is_agent = |n: &str| helmsman_project::agent::is_agent_file(n);
        let files: Vec<(String, String)> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| {
                helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St)
                    && !helmsman_iec::spec::is_test(n)
                    && !is_agent(n)
            })
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        let tests: Vec<(String, String)> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| helmsman_iec::spec::is_test(n))
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        let devices = self.devices_text();
        let first = crate::design::request(&crate::design::Ask {
            description: &message,
            dialect: crate::design_step::PAGE_DIALECT,
            files: &files,
            devices: &devices,
            tests: &tests,
        });
        match self.design.start_page(first, &description, now) {
            Ok(req) => {
                self.design.open = false;
                self.design_link.send(req.seq, req.for_page(), ctx);
            }
            Err(why) => self.design.error = Some(why),
        }
    }

    /// Stop, or Discard of a cut design: nothing is kept.
    pub fn stop_design(&mut self) {
        self.design.stop();
        self.design_link.abort();
    }

    /// Carry on after a stream that stopped early.
    pub fn carry_on_design(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Some(req) = self.design.carry_on(now) {
            self.design_link.send(req.seq, req.for_page(), ctx);
        }
    }

    /// Why the design on show cannot be kept now, if it cannot.
    fn design_keep_blocked(&self) -> Option<String> {
        self.instruct
            .busy()
            .then(|| "Still working on the last instruction.".to_string())
    }

    /// Keep: the design goes into the open project as `design.md`, one
    /// revision. A built-in example is read-only: a project of its own is
    /// made first, named for the machine.
    pub fn keep_design(&mut self) {
        let Some(read) = self.design.read.clone() else {
            return;
        };
        if let Some(why) = self.design_keep_blocked().or_else(|| read.keep_refusal()) {
            self.design.notice = Some(why);
            return;
        }
        if self.proj.is_example() {
            let name = self
                .proj
                .free(&crate::design_step::project_slug(&read.description));
            self.proj.create(&name, None);
        }
        if self.proj.dirty() {
            self.proj.save();
        }
        self.proj
            .files
            .insert(crate::design_step::DESIGN_FILE.to_string(), read.text.clone());
        if let Some(text) = read.documents_file() {
            self.proj
                .files
                .insert(crate::documents::DOCUMENTS_FILE.to_string(), text);
        }
        let what = if read.design.summary.is_empty() {
            "Kept the design".to_string()
        } else {
            format!(
                "Kept the design: {}",
                read.design.summary.chars().take(100).collect::<String>()
            )
        };
        self.proj.save_as(&what, None);
        self.reload();
        self.design.discard();
        self.design_built.remove(&self.proj.open);
        self.notice = Some(format!(
            "Kept the design as {} in {}.",
            crate::design_step::DESIGN_FILE,
            self.proj.open
        ));
    }

    /// The open project's kept design: what the tab shows of it. Read
    /// again only when the file changed.
    fn kept_design(&mut self) -> Option<crate::design_ui::Kept> {
        let text = self.proj.files.get(crate::design_step::DESIGN_FILE)?;
        let (d, problems) = self.design_cache.get(text);
        let built = self.design_built.get(&self.proj.open).copied().unwrap_or(0);
        let blocked = if self.instruct.busy() {
            Some("Still working on the last instruction.".to_string())
        } else {
            self.chat.route().err().map(str::to_string)
        };
        Some(crate::design_step::kept_view(d, problems, built, blocked))
    }

    /// [`Self::kept_design`], for the tests.
    #[cfg(test)]
    pub fn kept_design_for_test(&mut self) -> Option<crate::design_ui::Kept> {
        self.kept_design()
    }

    /// Sets how many steps of the open project's design are built, for
    /// the tests.
    #[cfg(test)]
    pub fn design_built_for_test(&mut self, n: usize) {
        self.design_built.insert(self.proj.open.clone(), n);
    }

    /// Build: the next batch of the kept design's build order goes to the
    /// steps run, `Design::step_brief` as each step's instruction.
    pub fn build_design(&mut self, ctx: &egui::Context) {
        if self.instruct.busy() {
            return;
        }
        if let Err(why) = self.chat.route() {
            self.instruct.failed(why);
            return;
        }
        let Some(text) = self.proj.files.get(crate::design_step::DESIGN_FILE) else {
            return;
        };
        let (design, problems) = self.design_cache.get(text);
        if !problems.is_empty() {
            return;
        }
        let built = self.design_built.get(&self.proj.open).copied().unwrap_or(0);
        let Some(batch) = crate::design_step::batch(design, built) else {
            return;
        };
        let label = batch.label(design);
        let file = if self.proj.files.contains_key(helmsman_project::MAIN) {
            helmsman_project::MAIN.to_string()
        } else {
            self.proj.current.clone()
        };
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope: crate::scope::Scope::File(file),
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        self.instruct.edits = self.chat.serves(crate::instruct::AGENT);
        let n = batch.to - batch.from;
        let (from, total) = (batch.from, batch.total);
        match self
            .instruct
            .start_design_steps(&env, &label, (batch.steps, batch.briefs), (from, total))
        {
            Ok(ask) => {
                self.design_building = Some((self.proj.open.clone(), n));
                self.asked_at = ctx.input(|i| i.time);
                self.drop_question();
                self.send_ask(&ask, ctx);
            }
            Err(why) => self.instruct.failed(&why),
        }
    }

    /// Each frame: the design entry's status when it is not known, what
    /// the transport brought, and a batch that ended.
    fn design_tick(&mut self, ctx: &egui::Context) {
        if let Some(base) = self.design.ask_status() {
            self.design_link.check_design(&base, ctx);
        }
        let now = ctx.input(|i| i.time);
        for m in self.design_link.drain() {
            if let crate::claude::Incoming::DesignStatus(body) = &m {
                self.design.status_body(body);
            } else if let Some(feed) = crate::design_step::Feed::from_claude(m)
                && let Some(req) = self.design.receive(feed, now)
            {
                self.design_link.send(req.seq, req.for_page(), ctx);
            }
        }
        if self.design.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    /// A batch of the design that was written counts its steps; one that
    /// was stopped or refused counts none.
    fn design_count(&mut self) {
        if self.design_building.is_none() || self.instruct.busy() {
            return;
        }
        let applied = self
            .instruct
            .outcome
            .as_ref()
            .is_some_and(|o| o.kind == crate::instruct::Kind::Applied);
        if let Some((project, n)) = self.design_building.take()
            && applied
        {
            *self.design_built.entry(project).or_default() += n;
        }
    }

    /// What an act of the design screens asks for.
    fn design_act(&mut self, act: crate::design_ui::Act, ctx: &egui::Context) {
        use crate::design_ui::Act;
        match act {
            Act::Send => self.send_design(ctx),
            Act::Stop | Act::Discard => {
                if self.design.busy() {
                    self.stop_design();
                } else {
                    self.design.discard();
                }
            }
            Act::CarryOn => self.carry_on_design(ctx),
            Act::Keep => self.keep_design(),
            Act::Build => self.build_design(ctx),
            Act::AddFile => {
                #[cfg(target_arch = "wasm32")]
                crate::web::pick_document_file(ctx);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.design.docs.error = Some("Files are added in the browser page.".into());
                }
            }
        }
    }

    /// The description window and the window the design is read in.
    fn design_windows(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        if !self.design.visible() {
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            for e in crate::web::take_document_errors() {
                self.design.docs.error = Some(e);
            }
            for (name, bytes) in crate::web::take_documents_picked() {
                if let Err(e) = self.design.docs.add_file(&name, &bytes) {
                    self.design.docs.error = Some(e);
                }
            }
        }
        let blocked = self
            .design_blocked()
            .or_else(|| self.design.closed_line(ctx.input(|i| i.time)));
        let mut acts = Vec::new();
        acts.extend(crate::design_ui::ask_window(
            ctx,
            chrome,
            &mut self.design,
            blocked.as_deref(),
        ));
        let keep = self.design_keep_blocked();
        acts.extend(crate::design_ui::read_window(
            ctx,
            chrome,
            &mut self.design,
            keep.as_deref(),
        ));
        for a in acts {
            self.design_act(a, ctx);
        }
    }

    /// Runs the plan waiting for approval, or tries its paused step again.
    pub fn run_steps(&mut self, ctx: &egui::Context) {
        let Some(scope) = self.instruct.run.as_ref().map(|r| r.scope.clone()) else {
            return;
        };
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope,
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        if let Some(ask) = self.instruct.approve(&env) {
            self.asked_at = ctx.input(|i| i.time);
            self.send_ask(&ask, ctx);
        }
    }

    /// One request of the instruction under way, to the Agent.
    fn send_ask(&mut self, ask: &crate::instruct::Ask, ctx: &egui::Context) {
        let turn = crate::claude::Turn {
            agent: Some(crate::instruct::AGENT),
            quick: false,
        };
        match self.chat.send_turn(&ask.message, &ask.context, turn) {
            Ok((seq, req)) => self.link.send(seq, req, ctx),
            Err(why) => self.instruct.failed(&why),
        }
    }

    /// Stop: the answer under way is dropped and nothing is written.
    pub fn stop_answer(&mut self) {
        self.stop_answer_with("stopped");
    }

    /// [`Self::stop_answer`], saying `why` in the instruction's outcome.
    fn stop_answer_with(&mut self, why: &str) {
        self.drop_question();
        self.chat.stop();
        self.link.abort();
        if self.instruct.busy() {
            self.instruct.failed(why);
        }
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

    /// The code into a new project (a PROGRAM, from a built-in example).
    /// A PROGRAM goes to the project named for the Agent's programs
    /// (Nick, 2026-11-01: named once, asked here the first time, see
    /// [`ProgramName`]); a block goes into the open project as a file.
    pub fn take_code(&mut self, code: String) {
        let is_program = code.to_ascii_uppercase().contains("PROGRAM ");
        let name = match (&self.new_program, is_program) {
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
        self.new_program = Some(name.clone());
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

    /// A file an instruction may change: ST, and not a requirement test.
    fn is_program(file: &str) -> bool {
        crate::sim_projects::is_st(file) && !helmsman_iec::spec::is_test(file)
    }

    /// The ST file the Agent's scope starts from: the open one, or, when
    /// a map, a settings file or a requirement test is open, the program
    /// last open in this project, else its first program file. A test is
    /// never the scope: it goes with every instruction as the program's
    /// check, and with one open the Agent was told the program was out of
    /// its reach (2026-11-22). The first program by name would be the
    /// wrong one as often as not in a project with several.
    fn scope_file(&self) -> Option<String> {
        let here = |f: &str| Self::is_program(f) && self.proj.files.contains_key(f);
        if here(&self.proj.current) {
            return Some(self.proj.current.clone());
        }
        if let Some((project, file)) = &self.last_program
            && *project == self.proj.open
            && here(file)
        {
            return Some(file.clone());
        }
        self.proj.files.keys().find(|f| Self::is_program(f)).cloned()
    }

    /// Remembers the program in the editor, for [`Self::scope_file`].
    fn note_program(&mut self) {
        if Self::is_program(&self.proj.current) && self.proj.files.contains_key(&self.proj.current) {
            self.last_program = Some((self.proj.open.clone(), self.proj.current.clone()));
        }
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
                (n.stmt, crate::chat_about::node_name(n))
            }
            Pick::Region(r) => {
                let r = d.regions.get(r.0 as usize)?;
                (r.stmt, r.label.clone())
            }
        };
        let (file, line, end) =
            crate::chat_about::statement_at(d, stmt, &|l| self.proj.locate(l), &self.proj.files)?;
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let name = if name.chars().count() > 32 {
            format!("{}…", name.chars().take(31).collect::<String>())
        } else {
            name
        };
        Some((file, name, line, end))
    }

    /// "Chat about this" on block `n` of the diagram: a chip for it on
    /// the Agent tab, and the tab shown. Nothing is sent.
    pub fn chat_about_node(&mut self, n: helmsman_diagram::NodeId, chrome: &mut Chrome) {
        let chip = self
            .viewer
            .as_mut()
            .filter(|v| v.src == self.proj.build_src())
            .and_then(|v| {
                v.pick = Some(Pick::Node(n));
                crate::chat_about::of_node(v.diagram(), n, &|l| self.proj.locate(l), &self.proj.files)
            });
        self.collect(
            chip,
            "That block's statement could not be placed: deploy the code, then ask again.",
            chrome,
        );
    }

    /// "Chat about this" on the input, output or variable `name`: a chip
    /// for it (its declaration, else its first use, as the place) on the
    /// Agent tab, and the tab shown. Nothing is sent.
    pub fn chat_about_signal(&mut self, name: &str, chrome: &mut Chrome) {
        let chip = {
            let diagrams: Vec<&helmsman_diagram::Diagram> = self
                .viewer
                .as_ref()
                .map(|v| v.module.pous.iter().map(|p| &p.diagram).collect())
                .unwrap_or_default();
            crate::chat_about::of_signal(name, &self.proj.files, &diagrams)
        };
        self.collect(chip, &format!("{name} is not in the program's files."), chrome);
    }

    /// Takes `chip` for the next instruction and shows the Agent tab; the
    /// notice `missing` when there is none.
    fn collect(&mut self, chip: Option<crate::chat_about::Chip>, missing: &str, chrome: &mut Chrome) {
        match chip {
            Some(c) => {
                self.instruct.add_chip(c);
                chrome.show(editor::Pane::Claude);
            }
            None => self.proj.note(missing, true),
        }
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

    /// The ST editor's widget id for the open file (its selection is the
    /// Agent's scope).
    pub fn source_id(&self) -> egui::Id {
        editor::source_id("pg-source", &self.proj.open, &self.proj.current)
    }

    /// Back to the whole file: the selection and the pick go.
    pub fn whole_file(&mut self, ctx: &egui::Context) {
        self.selection = None;
        editor::deselect(ctx, self.source_id());
        if let Some(v) = &mut self.viewer {
            v.pick = None;
        }
    }

    /// An instruction belongs to the project it was typed in: when another
    /// is open before the answer is in, the run ends with nothing written,
    /// so no retry is built from the other project's source and tests.
    fn drop_run_of_another_project(&mut self) {
        let moved = self
            .instruct
            .run
            .as_ref()
            .is_some_and(|r| r.project != self.proj.open);
        if moved {
            self.stop_answer_with("the open project changed before the answer came");
            self.log_outcome();
        }
    }

    /// How the answer to the instruction under way is coming along, at page
    /// time `now`; `None` while no instruction is under way.
    fn progress(&self, now: f64) -> Option<crate::instruct::Progress> {
        if !self.instruct.busy() {
            return None;
        }
        if self.to_name.is_some() {
            return Some(crate::instruct::Progress {
                naming: true,
                ..Default::default()
            });
        }
        let lines = self
            .instruct
            .run
            .as_ref()
            .and_then(|r| r.scope.file())
            .and_then(|f| self.proj.files.get(f))
            .map(|t| t.lines().count());
        Some(crate::instruct::Progress {
            estimate: Some(crate::instruct::estimate_secs(lines)),
            secs: now - self.asked_at,
            naming: false,
        })
    }

    /// The code the Agent is writing into the open file, while it streams:
    /// the Code tab shows it in place of the file, read-only, until the
    /// answer is checked (written) or not (the file comes back as it was).
    fn agent_draft(&self) -> Option<String> {
        let run = self.instruct.run.as_ref()?;
        if run.scope.file() != Some(self.proj.current.as_str()) {
            return None;
        }
        let a = self.chat.answer.as_ref().filter(|a| a.running())?;
        crate::instruct::partial_code(&a.text).map(str::to_string)
    }

    /// A finished answer to the instruction under way goes through
    /// [`crate::instruct::Instruct::answered`]: back to the Agent while it
    /// fails its checks, written when it passes, refused when it still
    /// fails. A broken or stopped answer ends the instruction.
    fn settle(&mut self, ctx: &egui::Context) {
        use crate::instruct::Step;
        if !self.instruct.busy() || self.to_name.is_some() {
            return;
        }
        let Some(a) = &self.chat.answer else { return };
        if a.running() || self.chat.seq() == self.answered_seq {
            return;
        }
        self.answered_seq = self.chat.seq();
        let ran_out = a.ran_out();
        let text = match &a.phase {
            Phase::Done => Ok(a.text.clone()),
            Phase::Failed(why) if ran_out => Err(why.clone()),
            Phase::Failed(why) => {
                let why = why.clone();
                self.instruct.failed(&why);
                return self.log_outcome();
            }
            Phase::Waiting | Phase::Streaming => return,
        };
        let Some(scope) = self.instruct.run.as_ref().map(|r| r.scope.clone()) else {
            return;
        };
        let (diags, devices, settings) = self.agent_env();
        let env = crate::instruct::Env {
            project: &self.proj.open,
            scope,
            files: &self.proj.files,
            source: &self.session.src,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        // Out of room: split into steps instead of failing (Nick,
        // 2026-11-14).
        let step = match text {
            Ok(text) => self.instruct.answered(&text, &env),
            Err(why) => self.instruct.cut_off(&env, &why),
        };
        match step {
            Step::Ask(ask) => {
                // The clock runs through a retry (the range it is read
                // against covers one); a plan's next step starts it again.
                let next_step = self.instruct.run.as_ref().is_some_and(|r| {
                    r.mode != crate::instruct::Mode::Direct && r.retries == 0 && !r.asked_scenarios
                });
                if next_step {
                    self.asked_at = ctx.input(|i| i.time);
                }
                self.send_ask(&ask, ctx);
            }
            Step::Apply(apply) => self.landed(apply, ctx),
            Step::Done => {
                // The Agent's correction did not pass: the change that
                // did is written after all.
                if let Some(apply) = self.fallback.take()
                    && self
                        .instruct
                        .outcome
                        .as_ref()
                        .is_some_and(|o| o.kind == crate::instruct::Kind::Refused)
                {
                    if let Some(card) = &mut self.card {
                        card.state = crate::ask::State::Kept;
                    }
                    self.write_answered(apply);
                } else {
                    self.log_outcome();
                }
            }
            Step::Wait => {}
        }
    }

    /// The outcome of the last instruction, in Messages.
    fn log_outcome(&mut self) {
        let Some(o) = &self.instruct.outcome else { return };
        let at = self.session.now_ms as f64 / 1000.0;
        self.log.outcome(at, o);
    }

    /// The Agent's last change, marked while its file is what the Agent
    /// wrote: the editor's line marks for the file open, the diagram's
    /// baseline. The first edit (any other text) clears them.
    pub(crate) fn agent_marks(&mut self, chrome: &mut Chrome) {
        let text = self
            .agent_change
            .as_ref()
            .and_then(|c| self.proj.files.get(&c.file))
            .map(String::as_str);
        if self
            .agent_change
            .as_ref()
            .is_some_and(|c| !c.holds(&self.proj.open, text))
        {
            self.agent_change = None;
        }
        let c = self.agent_change.as_ref();
        chrome.st_marks = c
            .filter(|c| c.file == self.proj.current)
            .map(|c| c.lines().clone());
        if let Some(v) = &mut self.viewer {
            v.baseline = c.and_then(crate::agent_marks::AgentChange::against);
        }
    }

    /// [`Self::reload`], then running if it builds: a change is seen
    /// working on the canvas at once.
    fn reload_running(&mut self) {
        self.reload();
        if self.session.build.as_ref().is_some_and(|b| b.module.is_some()) {
            self.session.set_running(true);
        }
    }

    /// The project's versions for the clock's list, in the Describe tab
    /// and the Code tab alike.
    fn versions(&self) -> crate::instruct::Versions {
        crate::instruct::Versions {
            rows: self.proj.revisions().iter().map(SimRevision::row).collect(),
            // By number, so a searched list knows which is on show.
            at: self
                .history_at
                .filter(|n| self.shows_revision(*n))
                .or_else(|| self.proj.revisions().last().map(|r| r.n)),
        }
    }

    /// The arrows' steps: while the Agent's change still shows in its
    /// file, or a revision picked with them (or in History) still shows,
    /// the revisions either side of the one shown, each tipped with what
    /// it was.
    fn steps(&self) -> Option<crate::instruct::Steps> {
        let at = self.history_at.filter(|n| self.shows_revision(*n));
        let change = self.agent_change.as_ref().is_some_and(|c| {
            c.project == self.proj.open
                && c.holds(&self.proj.open, self.proj.files.get(&c.file).map(String::as_str))
        });
        if at.is_none() && !change {
            return None;
        }
        let revs = self.proj.revisions();
        let k = match at {
            Some(n) => revs.iter().position(|r| r.n == n)?,
            None => revs.len().checked_sub(1)?,
        };
        let tip = |j: usize, way: &str| {
            revs.get(j).map(|r| {
                (
                    r.n,
                    format!(
                        "{way}: runs revision {}, {}, marked green and red with what it changed. Saving keeps the version shown.",
                        r.n, r.what
                    ),
                )
            })
        };
        Some(crate::instruct::Steps {
            back: k.checked_sub(1).and_then(|j| tip(j, "Back")),
            forward: tip(k + 1, "Forward"),
            shown: None,
        })
    }

    /// The Describe tab's arrows: through the revisions that carry an
    /// exchange (an instruction and what came of it), newest last; past
    /// the newest the tab is live again. `None` while an answer is under
    /// way, and while there is no exchange to go back to.
    fn exchange_steps(&self) -> Option<crate::instruct::Steps> {
        let revs = self.proj.revisions();
        let ex: Vec<_> = revs.iter().filter(|r| r.page.is_some()).collect();
        if ex.is_empty() || self.instruct.busy() {
            return None;
        }
        let at = self.exchange_at.filter(|n| self.shows_revision(*n));
        // The exchange shown; none while live (and past the newest).
        let k = at.and_then(|n| ex.iter().position(|r| r.n == n));
        let tip = |r: &SimRevision, way: &str| {
            (
                r.n,
                format!(
                    "{way}: the instruction and the answer of revision {}, {}. The project runs as it was then; Saving keeps it.",
                    r.n, r.what
                ),
            )
        };
        let back = match k {
            Some(0) => None,
            Some(j) => Some(tip(ex[j - 1], "Back")),
            None => Some(tip(ex[ex.len() - 1], "Back")),
        };
        let forward = k.map(|j| {
            ex.get(j + 1).map_or_else(
                || {
                    (
                        crate::instruct::LIVE,
                        "Forward: back to the project and the box as you left them.".to_string(),
                    )
                },
                |r| tip(r, "Forward"),
            )
        });
        Some(crate::instruct::Steps {
            back,
            forward,
            shown: k.map(|j| ex[j].n),
        })
    }

    /// H: into the Describe tab's history, at the newest exchange.
    pub fn open_history(&mut self) {
        if self.exchange_at.is_none()
            && let Some(n) = self.exchange_steps().and_then(|s| s.back).map(|(n, _)| n)
        {
            self.show_revision(n);
        }
    }

    /// A checked answer into the open project: unsaved edits are saved
    /// first as their own revision, so the version before the change is
    /// one; then the file (and the scenarios it passed, as requirement
    /// tests) are saved as one revision whose History entry is the
    /// instruction's record. A new program from a built-in example goes
    /// the way it always has, into a project named once.
    fn write(&mut self, apply: crate::instruct::Apply) {
        if apply.scope == crate::scope::Scope::NewProject {
            let code = apply.text.clone();
            self.to_name = Some(apply);
            self.take_code(code);
            return;
        }
        if self.proj.open != apply.project {
            let why = format!("{} is no longer open.", apply.project);
            self.instruct.not_written(&apply.file, &why);
            return self.log_outcome();
        }
        if self.proj.dirty() {
            self.proj.save();
        }
        let saved: std::collections::BTreeMap<String, String> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        if self.proj.revisions().last().is_none_or(|r| r.files != saved) {
            self.proj.save_as("saved before the described change", None);
        }
        let before = self.proj.revisions().last().map(|r| r.n);
        let was = self.proj.files.get(&apply.file).cloned().unwrap_or_default();
        let baseline = self.viewer.as_ref().map(|v| std::rc::Rc::new(v.module.clone()));
        self.agent_change = Some(crate::agent_marks::AgentChange::new(
            &apply.project,
            &apply.file,
            &was,
            apply.text.clone(),
            baseline,
        ));
        self.proj.files.insert(apply.file.clone(), apply.text.clone());
        for (f, t) in &apply.tests {
            self.proj.files.insert(f.clone(), t.clone());
        }
        self.proj.current.clone_from(&apply.file);
        let what: String = format!("Described: {}", apply.instruction)
            .chars()
            .take(120)
            .collect();
        self.proj.save_as(&what, Some(&apply.record));
        let known: Vec<String> = self
            .session
            .inputs()
            .chain(self.session.outputs())
            .map(|p| p.name.clone())
            .collect();
        // A described change runs at once (Nick, 2026-11-23: no stop and
        // reset, not as a switch either).
        self.reload_running();
        // What the change added to the program joins the trend.
        let added: Vec<String> = self
            .session
            .inputs()
            .chain(self.session.outputs())
            .map(|p| p.name.clone())
            .filter(|n| !known.contains(n))
            .collect();
        self.session.trend.pin_added(&added);
        let undo = before.map(|revision| crate::instruct::Undo {
            project: apply.project.clone(),
            file: apply.file.clone(),
            revision,
            text: apply.text.clone(),
        });
        self.instruct.applied(&apply, undo);
        self.log_outcome();
    }

    /// Offer `k` under the last answer, clicked ([`crate::offers`]): an
    /// instruction is sent as the engineer's own; a rename is written
    /// into every file it touches and saved as one revision with its
    /// History entry, as a described change is, so Undo and the arrows
    /// take it back the same way.
    pub fn take_offer(&mut self, k: usize, ctx: &egui::Context) {
        use crate::offers::What;
        match self.instruct.offer(k).cloned() {
            Some(What::Instead { text }) => {
                self.instruct.draft = text;
                self.send_instruction(ctx);
            }
            Some(What::Rename { old, new }) => self.rename_everywhere(&old, &new),
            None => {}
        }
    }

    /// `old` renamed to `new` in the open project, saved as one revision.
    pub fn rename_everywhere(&mut self, old: &str, new: &str) {
        let plan = match crate::offers::plan(&self.proj.files, old, new) {
            Ok(p) => p,
            Err(why) => {
                self.instruct.offer_refused(&why);
                return self.log_outcome();
            }
        };
        let record = crate::instruct::Instruct::rename_record(old, new, &plan.lines());
        let count = plan.files.len();
        let changed = plan.files.into_iter().map(|(f, t, _)| (f, t)).collect();
        let undo = self.commit_by_hand(changed, &format!("Renamed {old} to {new}"), &record);
        self.instruct.renamed(old, new, count, undo);
        self.log_outcome();
    }

    /// A literal of the diagram written `new` (`text` is its file after),
    /// saved as one revision.
    pub fn set_value(&mut self, lit: &crate::by_hand::Literal, new: &str, text: String) {
        let record = crate::by_hand::value_record(lit, new);
        let what = format!("Set {} to {new}", lit.text);
        let undo = self.commit_by_hand(vec![(lit.file.clone(), text)], &what, &record);
        self.instruct.value_set(&lit.text, new, &lit.file, undo);
        self.log_outcome();
    }

    /// Files changed by hand (a rename, a set value) into the open
    /// project as one revision with its History entry, the way a
    /// described change is written: unsaved edits are saved first as
    /// their own revision, and the program runs on with the change.
    /// Returns what Undo restores.
    fn commit_by_hand(
        &mut self,
        changed: Vec<(String, String)>,
        what: &str,
        record: &str,
    ) -> Option<crate::instruct::Undo> {
        if self.proj.dirty() {
            self.proj.save();
        }
        let saved: std::collections::BTreeMap<String, String> = self
            .proj
            .files
            .iter()
            .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
            .map(|(n, t)| (n.clone(), t.clone()))
            .collect();
        if self.proj.revisions().last().is_none_or(|r| r.files != saved) {
            self.proj.save_as("saved before the change by hand", None);
        }
        let before = self.proj.revisions().last().map(|r| r.n);
        // Undo is offered while the file on screen is as the change left it.
        let is_st = |f: &str| helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St);
        let shown = changed
            .iter()
            .find(|(f, _)| *f == self.proj.current)
            .or_else(|| changed.iter().find(|(f, _)| is_st(f)))
            .or(changed.first())
            .cloned();
        for (f, text) in changed {
            self.proj.files.insert(f, text);
        }
        self.proj.save_as(what, Some(record));
        self.reload_running();
        before
            .zip(shown)
            .map(|(revision, (file, text))| crate::instruct::Undo {
                project: self.proj.open.clone(),
                file,
                revision,
                text,
            })
    }

    /// What block `n` of the diagram lets the engineer change by hand
    /// ([`crate::by_hand`]): nothing in a read-only example, or while the
    /// diagram is older than the code.
    fn by_hand_target(&self, n: helmsman_diagram::NodeId) -> Option<crate::by_hand::Target> {
        if self.assign_write().is_err() {
            return None;
        }
        let v = self.viewer.as_ref().filter(|v| v.src == self.proj.build_src())?;
        crate::by_hand::target(v.diagram(), n, &|l| self.proj.locate(l), &self.proj.files)
    }

    /// Opens the question for a name or a value.
    fn by_hand_ask(&mut self, t: crate::by_hand::Target) {
        match t {
            crate::by_hand::Target::Name(n) => self.devices_ui.renaming = Some((n.clone(), n)),
            crate::by_hand::Target::Value(l) => self.value_ask = Some(crate::by_hand::ValueAsk::new(l)),
        }
    }

    /// Why Undo is off now, if it is.
    fn undo_blocked(&self) -> Option<String> {
        let u = self.instruct.undo()?;
        if self.proj.open != u.project {
            return Some(format!("Open {} first.", u.project));
        }
        self.instruct.undo_refusal(
            self.proj.files.get(&u.file).map(String::as_str),
            self.proj.dirty(),
        )
    }

    /// The open project's files are revision `n`'s (the Agent's pages
    /// aside).
    fn shows_revision(&self, n: u32) -> bool {
        let mine = |f: &std::collections::BTreeMap<String, String>| {
            f.iter()
                .filter(|(k, _)| !helmsman_project::agent::is_agent_file(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>()
        };
        self.proj
            .revisions()
            .iter()
            .find(|r| r.n == n)
            .is_some_and(|r| mine(&r.files) == mine(&self.proj.files))
    }

    /// Runs revision `n` of the open project (the History tab): its files
    /// in place, compiled and running, the open file marked green and red
    /// against the revision before it, the diagram too. Nothing is saved:
    /// Save keeps it as the newest; the newest is a step away.
    pub fn show_revision(&mut self, n: u32) {
        let revs = self.proj.revisions();
        // Past the newest exchange: the newest revision, and the tab as
        // the engineer left it.
        if n == crate::instruct::LIVE {
            if let Some(last) = revs.last().map(|r| r.n) {
                self.show_revision(last);
            }
            self.instruct.leave_steps();
            self.exchange_at = None;
            return;
        }
        self.exchange_at = Some(n);
        let Some(k) = revs.iter().position(|r| r.n == n) else {
            return;
        };
        let rev = revs[k].clone();
        let prev = k.checked_sub(1).map(|j| revs[j].clone());
        let newest = k + 1 == revs.len();
        let agent = self.proj.files.get(helmsman_project::AGENT).cloned();
        self.proj.files = rev.files.clone();
        if let Some(a) = agent {
            self.proj.files.insert(helmsman_project::AGENT.into(), a);
        }
        if !self.proj.files.contains_key(&self.proj.current) {
            self.proj.current = self
                .proj
                .files
                .keys()
                .find(|f| *f == helmsman_project::MAIN)
                .or_else(|| self.proj.files.keys().find(|f| crate::sim_projects::is_st(f)))
                .cloned()
                .unwrap_or_default();
        }
        self.reload_running();
        // What this revision changed, marked as the Agent's changes are.
        let file = self.proj.current.clone();
        let after = self.proj.files.get(&file).cloned().unwrap_or_default();
        self.agent_change = prev.map(|p| {
            let before = p.files.get(&file).cloned().unwrap_or_default();
            let src = crate::sim_projects::SimProjects::build_src_of(&p.files);
            let baseline = crate::playground::Build::new(&src)
                .0
                .ok()
                .and_then(|b| b.module)
                .map(std::rc::Rc::new);
            crate::agent_marks::AgentChange::new(&self.proj.open, &file, &before, after, baseline)
        });
        self.history_at = (!newest).then_some(n);
        // The tab as it stood for this step, and nothing of it in the
        // Messages: what an old version's compile or run says is not news.
        self.instruct.show_step(rev.page.as_deref(), &rev.what);
        self.seen.quiet = true;
    }

    /// Undo: the project back to the revision before the Agent's change,
    /// saved as a new revision (the change stays in History).
    pub fn undo_change(&mut self) {
        let Some(u) = self.instruct.undo().cloned() else {
            return;
        };
        if self.undo_blocked().is_some() {
            return;
        }
        if self.proj.restore(u.revision) {
            if self.proj.files.contains_key(&u.file) {
                self.proj.current.clone_from(&u.file);
            }
            self.reload();
            self.instruct.undone(&u);
            self.log_outcome();
        }
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
        let m = crate::theme::near_pointer(ctx, "pg-program-name").show(ctx, |ui| {
            ui.set_max_width(420.0);
            let why = "The answers in this conversation land in a project of this name: \
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
                ui.colored_label(crate::theme::theme(ui).ink(RED), e);
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
        // Only Cancel drops the answer: a click on the page behind or
        // Escape leaves the question open, since the checked program
        // behind it took a minute to get and is gone once this shuts.
        if ok {
            self.program_name = Some(p);
            self.confirm_program_name();
        } else if done {
            self.program_name = None;
            if let Some(a) = self.to_name.take() {
                self.instruct
                    .not_written(&a.file, "No name was given, so no project was made.");
            }
        } else {
            self.program_name = Some(p);
        }
    }

    /// One frame of the Editor.
    pub fn show(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.tick(&ui.ctx().clone());
        self.agent_marks(chrome);
        editor::show(ui, chrome, self);
        if self.devices_ui.take_import() {
            #[cfg(target_arch = "wasm32")]
            crate::web::pick_file(crate::app::DEVICE_ACCEPT, &ui.ctx().clone());
        }
        if let Some(crate::devices::DeviceAct::Add(w)) = self.devices_ui.form_ui(&ui.ctx().clone(), chrome) {
            self.apply_wiring(&w);
        }
        if let Some(crate::devices::DeviceAct::Quick(q)) = self.devices_ui.quick_ui(&ui.ctx().clone(), chrome)
            && self.own_project()
        {
            self.proj.add_file(&q.file, q.text.clone());
            self.apply_wiring(&q.wiring);
        }
        if let Some(crate::devices::DeviceAct::Add(w)) = self.devices_ui.add_io_ui(&ui.ctx().clone(), chrome)
        {
            self.apply_wiring(&w);
        }
        self.devices_ui.wire_it_ui(&ui.ctx().clone(), chrome);
        if let Some(crate::devices::DeviceAct::Remove(names)) =
            self.devices_ui.remove_ui(&ui.ctx().clone(), chrome)
            && let Some(t) = self.proj.files.get_mut(helmsman_project::IO_MAP)
        {
            *t = crate::device_form::remove_signals(t, &names);
            self.proj.note(
                match names.as_slice() {
                    [one] => format!("{one} removed from io.map."),
                    _ => format!("{} points removed from io.map.", names.len()),
                },
                false,
            );
            self.edited(f64::NEG_INFINITY);
        }
        if let Some(line) = self.devices_ui.plant.form_ui(&ui.ctx().clone(), chrome) {
            self.add_device_line(&line);
        }
        if self.devices_ui.renaming.is_some() {
            let files: Vec<(String, String)> = self
                .proj
                .files
                .iter()
                .map(|(n, t)| (n.clone(), t.clone()))
                .collect();
            self.devices_ui.plan_rename(&files);
        }
        if let Some(crate::devices::DeviceAct::Delete(name)) =
            self.devices_ui.delete_ui(&ui.ctx().clone(), chrome)
            && let Some(t) = self.proj.files.get_mut(helmsman_project::IO_MAP)
        {
            *t = crate::device_form::remove_device(t, &name);
            self.proj.note(format!("{name} deleted from io.map."), false);
            self.edited(f64::NEG_INFINITY);
        }
        if let Some(crate::devices::DeviceAct::Rename(plan)) =
            self.devices_ui.rename_ui(&ui.ctx().clone(), chrome)
        {
            self.rename_everywhere(&plan.old, &plan.new);
        }
        if let Some(mut ask) = self.value_ask.take() {
            let mut open = true;
            match ask.ui(&ui.ctx().clone(), chrome, &self.proj.files, &mut open) {
                Some(text) => self.set_value(&ask.lit, ask.text.trim(), text),
                None if open => self.value_ask = Some(ask),
                None => {}
            }
        }
        if self.devices_ui.assigning.is_some() {
            self.assign_context();
        }
        if let Some(a) = self.devices_ui.assign_ui(&ui.ctx().clone(), chrome) {
            self.assign_act(&a);
        }
        match self.devices_ui.pick_ui(&ui.ctx().clone(), chrome) {
            Some(crate::wired::PickAct::Search(_)) => self.wired_lib.search(&mut self.devices_ui),
            Some(crate::wired::PickAct::Set { signal, wired }) => self.set_wired(&signal, wired.as_deref()),
            None => {}
        }
        if let Some(a) = self.files_ui.naming_ui(&ui.ctx().clone(), chrome) {
            self.act(a);
        }
        if let Some(a) = self.files_ui.confirm_ui(&ui.ctx().clone(), chrome) {
            self.act(a);
        }
        self.program_name_ui(&ui.ctx().clone(), chrome);
        self.design_windows(&ui.ctx().clone(), chrome);
        self.revision_ui(&ui.ctx().clone(), chrome);
        for (name, r) in std::mem::take(&mut self.input_rects) {
            chrome.hit(format!("input {name}"), r);
        }
        // Every push button of this frame has spoken (the diagram's and
        // the I/O tab's): an input is on while any of its buttons is held.
        for (name, on) in std::mem::take(&mut self.pushed) {
            let was = self.session.value(&name);
            if self.session.set_input(&name, Value::Bool(on)) && on && was != Some(Value::Bool(true)) {
                self.idle_note(&name, true);
            }
        }
        self.sim_popup_ui(&ui.ctx().clone(), chrome);
    }

    /// The program's outputs as (name, is BOOL), lower case.
    fn output_list(&self) -> Vec<(String, bool)> {
        self.session
            .outputs()
            .map(|p| (p.name.to_ascii_lowercase(), p.ty == Ty::Bool))
            .collect()
    }

    /// Opens the simulate-this-input popup on input `name`.
    pub fn open_sim_popup(&mut self, name: &str) {
        let Some(p) = self.session.inputs().find(|p| p.name.eq_ignore_ascii_case(name)) else {
            return;
        };
        let is_bool = p.ty == Ty::Bool;
        let text = self
            .proj
            .files
            .get(helmsman_project::PLANT)
            .cloned()
            .unwrap_or_default();
        let (spec, _) = helmsman_sim::parse(&text);
        let mode = self
            .session
            .plant
            .as_ref()
            .map_or_else(|| spec.mode(name), |pl| pl.mode(name));
        let inputs: Vec<String> = self
            .session
            .inputs()
            .map(|p| p.name.to_ascii_lowercase())
            .collect();
        self.sim_popup = Some(crate::sim_popup::Popup::open(
            name,
            is_bool,
            &spec,
            mode,
            &self.output_list(),
            &inputs,
        ));
    }

    /// The popup, and its Apply: the project's `plant.sim` rewritten (a
    /// read-only example copied first), run from the next scan.
    fn sim_popup_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(mut popup) = self.sim_popup.take() else {
            return;
        };
        let text = self
            .proj
            .files
            .get(helmsman_project::PLANT)
            .cloned()
            .unwrap_or_default();
        let (spec, _) = helmsman_sim::parse(&text);
        let outputs = self.output_list();
        match popup.show(ctx, chrome, &spec, &outputs) {
            None => self.sim_popup = Some(popup),
            Some(crate::sim_popup::Act::Close) => {}
            Some(crate::sim_popup::Act::Apply) => match popup.apply(&text) {
                Err(why) => {
                    popup.error = Some(why);
                    self.sim_popup = Some(popup);
                }
                Ok(new) => {
                    if !self.own_project() {
                        popup.error = Some("The example could not be copied to write plant.sim into.".into());
                        self.sim_popup = Some(popup);
                        return;
                    }
                    match self.proj.files.get_mut(helmsman_project::PLANT) {
                        Some(t) => *t = new,
                        None => self.proj.add_file(helmsman_project::PLANT, new),
                    }
                    self.session.set_plant(
                        self.proj
                            .files
                            .get(helmsman_project::PLANT)
                            .map_or("", String::as_str),
                    );
                    self.proj.note(format!("plant.sim: {}", popup.explain()), false);
                }
            },
        }
    }

    /// Runs the program (and Claude's answer) on with nothing drawn: the
    /// Monitor watches the same session.
    /// The inputs whose device is lost while the program is held for it.
    fn held_by(&self) -> Vec<String> {
        let (true, Some(plant), Some(b)) = (self.session.held, &self.session.plant, &self.session.build)
        else {
            return Vec::new();
        };
        b.points
            .iter()
            .filter(|p| p.dir == crate::playground::Dir::In)
            .filter(|p| plant.mode(&p.name) == helmsman_sim::Mode::Absent)
            .map(|p| p.name.to_ascii_lowercase())
            .collect()
    }

    /// What the Agent says unasked at page time `t` (`crate::watch`):
    /// the one thing wrong with the open project, once the typing has
    /// stopped and nothing else is under way.
    pub fn notice_problems(&mut self, t: f64) {
        let diags = self.diags();
        let held_by = self.held_by();
        let noticed = crate::watch::noticed(&crate::watch::Seen {
            diags: &diags,
            fault: self.session.fault.as_deref(),
            held_by: &held_by,
            tests: &self.tests,
        });
        let busy = self.pending.is_some() || self.instruct.busy() || self.program_name.is_some();
        self.noticed = self.watching.say(noticed, busy, t);
    }

    /// [`Self::idle_note`]'s line while it stands, low on the diagram
    /// where it is seen whichever tab the click was in: muted, with no
    /// button, and it takes no click from the blocks under it.
    fn idle_ui(&self, ui: &egui::Ui, chrome: &mut Chrome) {
        let Some(note) = self
            .idle_notice
            .as_ref()
            .filter(|n| self.notice.as_ref() == Some(n))
        else {
            return;
        };
        let t = theme(ui);
        let area = ui.available_rect_before_wrap();
        let width = (area.width() - 24.0).clamp(160.0, NOTICED_WIDTH);
        let shown = egui::Area::new(egui::Id::unique("idle note"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(Pos2::new(area.center().x, area.bottom() - 12.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.panel)
                    .stroke(Stroke::new(1.0, t.rule))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_max_width(width - 20.0);
                        ui.add(
                            egui::Label::new(RichText::new(note).color(t.muted).size(crate::theme::MIN_TEXT))
                                .wrap(),
                        );
                    });
            });
        chrome.hit("idle note", shown.response.rect);
    }

    /// What the Agent noticed, as a card over the top of the diagram:
    /// on screen whichever tab is open, and in nothing's way (it is not
    /// modal and takes no room from a panel). What went wrong, the
    /// better way, Fix it and Not now.
    fn noticed_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let Some(n) = self.noticed.clone() else {
            return;
        };
        let t = theme(ui);
        let area = ui.available_rect_before_wrap();
        let width = (area.width() - 24.0).clamp(160.0, NOTICED_WIDTH);
        let size = crate::theme::MIN_TEXT;
        let (mut fix, mut not_now) = (false, false);
        let shown = egui::Area::new(egui::Id::unique("agent noticed"))
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(area.center().x - width / 2.0, area.top() + 44.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.panel)
                    .stroke(Stroke::new(1.5, WARN))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(width - 24.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("Helmsman noticed: {}", n.what))
                                    .color(t.ink(WARN))
                                    .size(size),
                            )
                            .wrap(),
                        );
                        if let Some(better) = &n.better {
                            ui.add(egui::Label::new(RichText::new(better).color(t.fg).size(size)).wrap());
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            let r = ui
                                .button(RichText::new("Fix it").size(size))
                                .tip("Asks for a fix. Its change is checked like any other before it lands.");
                            chrome.hit("agent fix it", r.rect);
                            fix = r.clicked();
                            let r = ui
                                .button(RichText::new("Not now").size(size))
                                .tip("Hides this until something else goes wrong.");
                            chrome.hit("agent not now", r.rect);
                            not_now = r.clicked();
                        });
                    });
            });
        chrome.hit("agent noticed", shown.response.rect);
        if fix {
            let ctx = ui.ctx().clone();
            self.fix_noticed(chrome, &ctx);
        } else if not_now {
            self.watching.dismiss(&n.key);
            self.noticed = None;
        }
    }

    /// Fix it: what was noticed goes to the Agent as an instruction, on
    /// the Agent tab, through the same checks as any other.
    pub fn fix_noticed(&mut self, chrome: &mut Chrome, ctx: &egui::Context) {
        let Some(n) = self.noticed.take() else {
            return;
        };
        self.watching.dismiss(&n.key);
        self.instruct.draft = n.ask;
        chrome.show(editor::Pane::Claude);
        self.send_instruction(ctx);
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        self.note_program();
        self.notice_problems(ctx.input(|i| i.time));
        if !self.proxy_asked {
            self.proxy_asked = true;
            self.link.check_proxy(ctx);
        }
        if !self.free_asked
            && let Some(base) = self.chat.free_base().map(str::to_string)
        {
            self.free_asked = true;
            self.link.check_free(&base, ctx);
        }
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(r) = self.library.take() {
                self.wired_lib.index_answer(r, &mut self.devices_ui);
            }
            if let Some(r) = self.record.take() {
                self.wired_lib.record_answer(r, &mut self.devices_ui);
            }
            match self
                .wired_lib
                .next(helmsman_project::devlib::DEFAULT_URL, &mut self.devices_ui)
            {
                Some(crate::wired::Fetch::Index(url)) => self.library.fetch(&url, ctx),
                Some(crate::wired::Fetch::Record(_, url)) => self.record.fetch(&url, ctx),
                None => {}
            }
        }
        for m in self.link.drain() {
            self.chat.receive(m);
        }
        self.design_tick(ctx);
        self.drop_run_of_another_project();
        self.side_settle(ctx);
        self.start_settle(ctx);
        self.summary_settle();
        self.settle(ctx);
        self.design_count();
        self.tidy_question();
        let t = ctx.input(|i| i.time);
        self.check_due(t, false);
        self.arrange();
        self.bind_sensors();
        self.session.advance(t);
        self.watch(t);
        if self.session.running || self.pending.is_some() || self.chat.busy() || self.instruct.busy() {
            ctx.request_repaint();
        }
    }
}

impl PlaygroundView {
    /// The simulation's state and clock as the label beside the run
    /// glyphs says them (`running · 12.3s`), and whether that is the
    /// good state (running, no fault).
    #[must_use]
    pub fn run_status(&self) -> (String, bool) {
        let s = &self.session;
        #[allow(clippy::cast_precision_loss)]
        let secs = s.now_ms as f64 / 1000.0;
        let state = s.state();
        (
            format!("{} · {}s", state.word(), helmsman_iec::shown_real(secs)),
            state == crate::playground::RunState::Running,
        )
    }

    /// What the guided tour reads of this view (`crate::tour`), with the
    /// Editor on screen and the trend panel as `chrome` has it.
    #[must_use]
    pub fn tour_seen(&self, chrome: &Chrome) -> crate::tour::Seen {
        use std::hash::{Hash, Hasher};
        let hash = |f: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f(&mut h);
            h.finish()
        };
        let blink = crate::sim_projects::example_project(crate::playground::example("Blink").src).0;
        let trend = &self.session.trend;
        crate::tour::Seen {
            editor: true,
            naming_blink: self.files_ui.naming == Some(projects::Naming::FromStarter(blink)),
            open: self.proj.open.clone(),
            example: self.proj.is_example(),
            source: hash(&|h| self.proj.files.hash(h)),
            compiles: self.pending.is_none() && self.session.diags.is_empty(),
            live: self.session.running
                && !self.session.stale()
                && self.proj.running.as_deref() == Some(self.proj.open.as_str()),
            picked: self
                .viewer
                .as_ref()
                .filter(|v| v.pick.is_some() || v.because.is_some())
                .map(|v| format!("{:?} {:?}", v.pick, v.because)),
            trend: hash(&|h| {
                trend.window_ms.hash(h);
                trend.signals.hash(h);
            }),
            asks: self.chat.seq(),
            trend_open: chrome.trend,
        }
    }
}

/// What [`PlaygroundView::watch`] compares against.
#[derive(Default)]
struct Seen {
    /// A revision was just put on show: the next watch logs nothing.
    quiet: bool,
    fault: Option<String>,
    diags: Vec<String>,
    warned: Option<String>,
    notice: Option<String>,
    storage: Option<String>,
    running: Option<bool>,
    scans: u64,
}

impl PlaygroundView {
    /// Reports to the log what changed since the last frame: compile
    /// errors (as they appear), a fault, a notice, a storage problem,
    /// the program starting or stopping.
    fn watch(&mut self, t: f64) {
        // Stepping through History changes what compiles and runs: the
        // watch takes note of it and says nothing.
        self.log.muted = std::mem::take(&mut self.seen.quiet) || self.history_at.is_some();
        self.watch_changes(t);
        self.log.muted = false;
    }

    fn watch_changes(&mut self, t: f64) {
        let fault = self.session.fault.as_ref().map(ToString::to_string);
        let ended = (self.seen.running == Some(true) && !self.session.running)
            || (fault.is_some() && fault != self.seen.fault)
            || self.session.scans < self.seen.scans;
        self.seen.scans = self.session.scans;
        if ended
            && self
                .running_notice
                .take()
                .is_some_and(|n| self.notice.as_ref() == Some(&n))
        {
            self.notice = None;
        }
        if fault != self.seen.fault {
            if let Some(f) = &fault {
                self.log
                    .error(t, format!("Fault: {f}. Run or Step restarts the program."));
            }
            self.seen.fault = fault;
        }
        let warned = projects::warning_summary(&self.diags());
        if warned != self.seen.warned {
            if let Some(w) = &warned {
                self.log.warning(t, w.clone());
            }
            self.seen.warned = warned;
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
        if self.session.running
            && self
                .idle_notice
                .take()
                .is_some_and(|n| self.notice.as_ref() == Some(&n))
        {
            self.notice = None;
        }
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
    fn toolbar(&mut self, _ui: &mut egui::Ui, _chrome: &mut Chrome) {
        // Nothing of the simulation's is in the header: its state and
        // clock sit beside the run glyphs over the diagram
        // (`App::glyphs`, [`Self::run_status`]).
    }

    fn has_notices(&self) -> bool {
        self.session.fault.is_some()
            || self.session.paused.is_some()
            || self.notice.is_some()
            || self.proj.notice.is_some()
            || self.proj.storage_problem.is_some()
    }

    fn alarm(&self) -> bool {
        self.session.fault.is_some() || self.proj.notice.as_ref().is_some_and(|(_, error)| *error)
    }

    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if let Some(f) = &self.session.fault {
            let text = format!("Fault: {f}. Run or Step restarts the program.");
            crate::messages::copyable(ui, chrome, "fault", &text, crate::theme::theme(ui).ink(RED));
        }
        if self.session.paused.is_some() {
            let at = self
                .paused_line()
                .map_or_else(String::new, |l| format!(" before line {l}"));
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    crate::theme::theme(ui).ink(WARN),
                    format!("Paused at a breakpoint{at}."),
                )
                .tip("The diagram and the values show the scan as it stood just before that statement.");
                ui.label(
                    RichText::new("F8 runs the rest of the scan. Ctrl Enter goes on to the next stop.")
                        .color(t.muted),
                );
            });
        }
        if let Some(n) = &self.notice {
            ui.label(RichText::new(n).color(t.muted));
        }
        if let Some((n, error)) = self.proj.notice.clone() {
            if error {
                crate::messages::copyable(ui, chrome, "notice", &n, crate::theme::theme(ui).ink(RED));
            } else {
                let r = ui.label(RichText::new(n).color(t.muted));
                chrome.hit("notice", r.rect);
            }
            if error && (self.proj.pending.is_some() || self.proj.pending_file.is_some()) {
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
        let mut go = false;
        let steps = self.steps();
        let versions = self.versions();
        let mut revision = None;
        let mut to_error: Option<(String, u32)> = None;
        // The header is the open file's name (Nick, 2026-11-08), the
        // magnifier and word wrap; at its right end Deploy, and before it
        // the errors when there are any.
        let name = self.proj.current.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            editor::file_header(
                ui,
                chrome,
                &name,
                "The open file. It is checked as you type; Deploy runs it.",
            );
            editor::find_toggle(ui, chrome);
            editor::wrap_toggle(ui, chrome);
            if undeployed {
                let note = if errors > 0 {
                    "The program running is unchanged. Fix the errors, then Deploy."
                } else {
                    "The program running is unchanged. Deploy runs this version."
                };
                ui.add(crate::theme::whole(RichText::new("· not deployed").color(t.muted)))
                    .tip(note);
            }
            // Nothing while the code compiles (Nick, 2026-11-23); a red ✖
            // with the first error in its popup while it does not.
            let first = self
                .diags()
                .into_iter()
                .find(|d| d.severity == projects::Severity::Error);
            let said = first
                .as_ref()
                .map(|d| format!("{} line {}: {}", d.file, d.line, d.message));
            let pressed = editor::header_right(
                ui,
                chrome,
                &editor::Status {
                    errors: if self.pending.is_some() { 0 } else { errors },
                    first: said.as_deref(),
                    deploy: &Ok(()),
                    deploy_tip: "Ctrl Shift Enter: deploy this version.\nCompiles the open project and runs it in the page.",
                    busy: None,
                    steps: steps.as_ref(),
                    history: !versions.rows.is_empty(),
                },
            );
            go = pressed.deploy;
            revision = pressed.revision;
            if pressed.errors
                && let Some(d) = first
            {
                to_error = Some((d.file, d.line));
            }
        });
        if let Some(n) = revision {
            self.show_revision(n);
        }
        if chrome.history && !versions.rows.is_empty() {
            match editor::versions_panel(ui, chrome, &mut self.instruct, &versions) {
                Some(crate::instruct::Act::Revision(n)) => self.show_revision(n),
                Some(crate::instruct::Act::Details(n)) => self.act(Act::ShowRevision(n)),
                _ => {}
            }
        }
        if go {
            self.deploy();
        }
        if let Some((file, line)) = to_error {
            self.goto(&file, line, chrome);
        }
        let all = self.diags();
        let current = self.proj.current.clone();
        let diags = projects::marks_in(&all, &current, false);
        let warns = projects::marks_in(&all, &current, true);
        let avail = ui.available_height();
        let list_h = if all.is_empty() { 0.0 } else { 110.0 };
        let size = Vec2::new(ui.available_width(), (avail - list_h).max(120.0));
        let id = self.source_id();
        if let Some(draft) = self.agent_draft() {
            let r = ui
                .label(
                    RichText::new("The draft, as it is written. It is checked before it goes into the file.")
                        .color(t.muted)
                        .italics(),
                )
                .tip("The file itself is unchanged until the checks pass. Stop in the Describe tab ends it.");
            chrome.hit("st draft note", r.rect);
            let mut shown: &str = &draft;
            let r = ui
                .allocate_ui(size, |ui| {
                    editor::st_editor(
                        ui,
                        id.with("agent-draft"),
                        &mut shown,
                        &[],
                        &[],
                        MAX_SOURCE,
                        chrome,
                    )
                })
                .inner;
            chrome.hit("st draft", r.rect);
            return;
        }
        let Some(text) = self.proj.current_text_mut() else {
            ui.label(RichText::new("No file open.").color(t.muted))
                .tip("Open or make one in Files.");
            return;
        };
        let r = ui
            .allocate_ui(size, |ui| {
                editor::st_editor(ui, id, text, &diags, &warns, MAX_SOURCE, chrome)
            })
            .inner;
        chrome.hit("st editor", r.rect);
        if r.changed() {
            self.edited(now);
        }
        // The selected lines, the Agent's scope while there are some. Read
        // while the editor has focus: an editor without it drops its
        // selection, and moving to the Agent tab takes the focus.
        if ui.ctx().memory(|m| m.has_focus(id)) {
            self.selection = editor::selection(ui.ctx(), id).and_then(|(a, b)| {
                let text = self.proj.files.get(&current)?;
                crate::scope::selected_lines(text, a, b).map(|(from, to)| (current.clone(), from, to))
            });
        } else if self.selection.as_ref().is_some_and(|(f, ..)| *f != current) {
            self.selection = None;
        }
        if !all.is_empty() {
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
        let listing = Listing {
            place: projects::Place::Browser,
            projects: &rows,
            open: Some(&open),
            files,
            current: Some(&current),
            write: Ok(()),
            dirty: self.proj.dirty(),
            starters: false,
            running: self.proj.running.as_deref() == Some(open.as_str()),
            // The page deploys at once: never busy.
            deploy: Ok(()),
        };
        if let Some(a) = self.files_ui.show(ui, &listing, chrome) {
            self.act(a);
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
        self.devices_ui.copy_first = self.proj.is_example();
        self.devices_ui.program = crate::device_form::ports_of(&self.proj.build_src());
        self.devices_ui.can_declare =
            crate::device_form::program_file(self.proj.files.iter().map(|(n, t)| (n.as_str(), t.as_str())))
                .is_some();
        self.devices_ui.set_profiles(
            self.proj
                .files
                .iter()
                .filter(|(n, _)| n.ends_with(helmsman_project::DEVICE_EXT))
                .map(|(n, t)| (n.as_str(), t.as_str())),
        );
        self.devices_ui.map = map.clone();
        self.devices_ui.eds_files = self
            .proj
            .files
            .keys()
            .filter(|f| crate::devices::is_eds(f))
            .cloned()
            .collect();
        self.want_wired();
        match self.devices_ui.show(ui, &nodes, &source, chrome, &write) {
            Some(crate::devices::DeviceAct::Scan) => {
                self.devices_ui.simulated_scan(&map);
                self.wired_lib.scan(&mut self.devices_ui);
                // Scan and add queues what the library knows.
                self.devices_ui.found_to_add_all();
            }
            Some(crate::devices::DeviceAct::Ask(_)) => self.devices_ui.simulated_ask(),
            Some(crate::devices::DeviceAct::Use(d)) => self.use_device(d),
            Some(crate::devices::DeviceAct::AddIo(device)) => {
                // A read-only example is copied first; the copy keeps the
                // device's name.
                if self.own_project() {
                    self.devices_ui.open_add_io(&device);
                }
            }
            Some(crate::devices::DeviceAct::Library { .. }) => self.devices_ui.simulated_library_answer(),
            Some(crate::devices::DeviceAct::OpenMap) => {
                self.proj.open_file(helmsman_project::IO_MAP);
                chrome.show_code();
            }
            Some(
                crate::devices::DeviceAct::Add(_)
                | crate::devices::DeviceAct::Rename(_)
                | crate::devices::DeviceAct::Delete(_)
                | crate::devices::DeviceAct::Remove(_)
                | crate::devices::DeviceAct::Quick(_),
            )
            | None => {}
            Some(crate::devices::DeviceAct::Assign(a)) => self.assign_act(&a),
        }
    }

    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.trend_tab(ui, chrome);
    }

    fn messages(&mut self) -> &mut crate::messages::Log {
        &mut self.log
    }

    fn tabs(&self) -> &'static [Tab] {
        &[Tab::Io, Tab::Claude]
    }

    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome) {
        match tab {
            Tab::Io => self.io(ui, chrome),
            Tab::Claude => self.claude(ui, chrome),
        }
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        self.noticed_ui(ui, chrome);
        self.idle_ui(ui, chrome);
        let menus = self.assign_menus();
        let scan = self.session.trace.as_ref().map(|t| t as &dyn Scan);
        if let Some(v) = &mut self.viewer {
            // A block's lines are the files' only while the diagram is of the text.
            v.chat_ok = v.src == self.proj.build_src();
            v.assign = menus;
            v.icons
                .update(self.proj.files.get(crate::icons::FILE).map(String::as_str));
            v.crumbs_ui(ui, &[]);
            let area = ui.available_rect_before_wrap();
            v.breaks.clone_from(&self.session.breaks);
            v.paused = self.session.paused_at();
            let now = ui.input(|i| i.time);
            for t in self.session.earlier() {
                v.saw(t, now);
            }
            v.menu_open = self.dmenu.is_open() || self.sim_popup.is_some();
            v.canvas(ui, scan);
            let reads = v.reads_on_screen();
            // The switches over the Read blocks take the right-click first.
            v.secondary_on_controls(ui, &reads);
            if let Some(g) = v.gesture.take() {
                self.dmenu.open(v, g);
            }
            // The blocks selected are the Agent's focus.
            crate::chat_about::apply_off(&mut self.instruct, v);
            let current = v.src == self.proj.build_src();
            let sel = if current {
                v.selection().to_vec()
            } else {
                Vec::new()
            };
            let key = (sel.clone(), self.instruct.chips.len(), v.src.len(), current);
            if !self.instruct.focus.is_for(&key) {
                let made = crate::chat_about::focus_of(
                    v.diagram(),
                    &sel,
                    &self.instruct.chips,
                    &|l| self.proj.locate(l),
                    &self.proj.files,
                );
                self.instruct.focus.set(key, made);
            }
            self.dmenu.chat_full = self.instruct.chips.len() >= crate::chat_about::MAX_CHIPS;
            self.dmenu.show(ui.ctx(), chrome, area);

            let mut jump = None;
            if v.pick != self.last_pick {
                self.last_pick = v.pick;
                if let Some((line, _)) = v.picked() {
                    jump = Some(line);
                    // The later act wins: a pick is the scope now.
                    self.selection = None;
                }
            }
            if let Some((port, ask)) = self.dmenu.take_assign() {
                self.assign_ask(port, ask);
            }
            if let Some(n) = self.dmenu.take_chat() {
                self.chat_about_node(n, chrome);
            }
            if let Some(n) = self.dmenu.take_edit_query() {
                let t = self.by_hand_target(n);
                self.dmenu.set_edit(t);
            }
            self.map_inputs(ui, area, &reads);
            // A double click on an input's block is "Simulate this input"
            // (above); its name is in the right-click menu.
            let asked = self
                .viewer
                .as_mut()
                .and_then(|v| v.edit_ask.take())
                .filter(|_| self.sim_popup.is_none());
            if let Some(t) = asked
                .and_then(|n| self.by_hand_target(n))
                .or_else(|| self.dmenu.take_edit())
            {
                self.by_hand_ask(t);
            }
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
    fn io(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::io_cards::{Form, Links, Row};
        let t = theme(ui);
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            return;
        }
        let inputs: Vec<Point> = self.session.inputs().cloned().collect();
        let outputs: Vec<Point> = self.session.outputs().cloned().collect();
        // Above the cards, not among them: it keeps its place while they scroll.
        self.start_ui(ui, chrome);
        // What is wired to each point, from io.map and the library; where
        // each port is, and what a right-click on its row offers.
        self.assign_context();
        self.want_wired();
        let wired = crate::wired::by_signal(&self.devices_ui.map);
        let menus = self.devices_ui.port_menus(&self.assign_write());
        let map = self.devices_ui.map.clone();
        let mut asks: Vec<(String, crate::assign_ui::PortAsk)> = Vec::new();
        let mut chat = crate::assign_ui::RowChat {
            full: self.instruct.chips.len() >= crate::chat_about::MAX_CHIPS,
            asked: Vec::new(),
            rename_off: self.assign_write().err(),
            rename: Vec::new(),
        };
        // What the diagram says of each point; nothing while there is none.
        let cards = self.viewer.as_ref().map(crate::st_view::Viewer::output_cards);
        // What is on the Trend: its cards show their trend glyph on.
        let pinned = self.session.trend.signals.clone();
        let lit = self
            .viewer
            .as_ref()
            .and_then(|v| v.because.clone())
            .unwrap_or_default();
        let src = self.viewer.as_ref().map(|v| v.src.clone());
        let uses = |name: &str| src.as_ref().map(|s| crate::find::names(s, name).len());
        let open: Vec<String> = self.io_open.iter().cloned().collect();
        let is_open = |name: &str| open.iter().any(|o| o.eq_ignore_ascii_case(name));
        // What the library says is wired to `name`, as a card's line.
        let known = &self.devices_ui.wired;
        let real = |name: &str, place: &crate::assign::Place| {
            let line = wired
                .get(&name.to_ascii_lowercase())
                .map(|w| crate::wired::line(w, known.get(w)));
            crate::io_cards::real(line, place)
        };
        let plant = self.session.plant.as_ref();
        let mut asked: Vec<(String, bool, crate::io_cards::Asked)> = Vec::new();
        let mut set = Vec::new();
        let mut held = Vec::new();
        let mut rects = Vec::new();
        egui::ScrollArea::vertical().id_salt("pg-io").show(ui, |ui| {
            let inputs_tip = if inputs.is_empty() {
                "No VAR_INPUT or %IX bit: add one to get a control."
            } else {
                "Switch, press or drag the inputs here, or on their blocks in the diagram."
            };
            crate::io_cards::heading(ui, chrome, "Inputs", inputs.len(), true, inputs_tip, true);
            if !chrome.io_fold_in {
                if inputs.is_empty() {
                    ui.label(RichText::new("none").color(t.muted)).tip(inputs_tip);
                }
                crate::io_cards::lay(
                    ui,
                    "pg-io-in",
                    inputs.len(),
                    chrome.io_grid,
                    &|k| is_open(&inputs[k].name),
                    &mut |ui, k, form| {
                        let p = &inputs[k];
                        let place = crate::assign::place_of(&map, &p.name);
                        let said = place.describe();
                        let open = is_open(&p.name);
                        let row = Row {
                            key: &p.name,
                            input: true,
                            place: &said,
                            real: real(&p.name, &place),
                            links: Links::Input(cards.as_ref().map(|c| crate::io_cards::affects(c, &p.name))),
                            selected: form == Form::Tile && open,
                            open: form == Form::Detail || (form == Form::Row && open),
                            tile: form == Form::Tile,
                            uses: uses(&p.name),
                            simulate: true,
                            designation: crate::assign::Designation::of(&place),
                            trended: pinned.iter().any(|t| t.eq_ignore_ascii_case(&p.name)),
                        };
                        let (a, card, control) = crate::io_cards::show(ui, chrome, &row, |ui| {
                            if form == Form::Detail {
                                // Its control is on its tile.
                                let glyph = crate::glyph_lib::for_point(&p.name, true, p.ty == Ty::Bool);
                                let text =
                                    format!("{} = {}", p.name.to_ascii_lowercase(), show(p.ty, p.value));
                                crate::io_cards::value(ui, Some(glyph), state_ink(&t, p.value), &text, true);
                                return None;
                            }
                            let r = ui.scope(|ui| input_ui(ui, p, &said, form == Form::Tile));
                            Some((r.inner.0, r.inner.1, r.response.rect))
                        });
                        if let Some((value, anchor, rect)) = control {
                            match (p.widget, value) {
                                (Some(Widget::Push), Some(Value::Bool(on))) => {
                                    held.push((p.name.clone(), on));
                                }
                                (_, Some(v)) => set.push((p.name.clone(), v)),
                                (_, None) => {}
                            }
                            crate::assign_ui::row_menu(
                                &anchor, chrome, &menus, &p.name, &mut asks, &mut chat,
                            );
                            rects.push((p.name.to_ascii_lowercase(), rect));
                            mark_mode(
                                ui,
                                rect,
                                plant.map_or(helmsman_sim::Mode::Normal, |pl| pl.mode(&p.name)),
                            );
                        }
                        crate::assign_ui::row_menu(&card, chrome, &menus, &p.name, &mut asks, &mut chat);
                        asked.push((p.name.clone(), true, a));
                    },
                );
            }
            ui.add_space(8.0);
            let outputs_tip = if outputs.is_empty() {
                "No VAR_OUTPUT or %QX bit: add one to get a lamp."
            } else {
                "Click an output to light what drives it in the diagram."
            };
            crate::io_cards::heading(ui, chrome, "Outputs", outputs.len(), false, outputs_tip, false);
            if chrome.io_fold_out {
                return;
            }
            if outputs.is_empty() {
                ui.label(RichText::new("none").color(t.muted)).tip(outputs_tip);
                return;
            }
            let section = ui.vertical(|ui| {
                crate::io_cards::lay(
                    ui,
                    "pg-io-out",
                    outputs.len(),
                    chrome.io_grid,
                    &|k| is_open(&outputs[k].name),
                    &mut |ui, k, form| {
                        let p = &outputs[k];
                        let place = crate::assign::place_of(&map, &p.name);
                        let said = place.describe();
                        let open = is_open(&p.name);
                        let card = cards
                            .as_ref()
                            .and_then(|c| c.iter().find(|c| c.name.eq_ignore_ascii_case(&p.name)));
                        let row = Row {
                            key: &p.name,
                            input: false,
                            place: &said,
                            real: real(&p.name, &place),
                            links: Links::Output(card),
                            selected: lit.eq_ignore_ascii_case(&p.name) || (form == Form::Tile && open),
                            open: form == Form::Detail || (form == Form::Row && open),
                            tile: form == Form::Tile,
                            uses: uses(&p.name),
                            simulate: false,
                            designation: crate::assign::Designation::of(&place),
                            trended: pinned.iter().any(|t| t.eq_ignore_ascii_case(&p.name)),
                        };
                        let (a, card, ()) = crate::io_cards::show(ui, chrome, &row, |ui| {
                            output_ui(ui, p, form == Form::Tile);
                        });
                        crate::assign_ui::row_menu(&card, chrome, &menus, &p.name, &mut asks, &mut chat);
                        asked.push((p.name.clone(), false, a));
                    },
                );
            });
            chrome.hit("io outputs", section.response.rect);
        });
        self.input_rects.extend(rects);
        for (name, v) in set {
            self.set_by_hand(&name, v);
        }
        for (name, on) in held {
            self.hold(&name, on);
        }
        for (name, input, a) in asked {
            self.io_asked(&name, input, a, chrome);
        }
        for name in chat.asked {
            self.chat_about_signal(&name, chrome);
        }
        if let Some(name) = chat.rename.pop() {
            let name = crate::by_hand::spelled(&self.proj.files, &name);
            self.devices_ui.renaming = Some((name.clone(), name));
        }
        for (port, ask) in asks {
            self.assign_ask(port, ask);
        }
    }

    /// An input set by hand, in the I/O tab or on its block: the value
    /// is the input's from now on, running or not, so a machine is set
    /// up before it starts. A refusal is said; with nothing running, so
    /// is that nothing reacts yet ([`Self::idle_note`]).
    fn set_by_hand(&mut self, name: &str, v: Value) {
        match self.session.try_input(name, v) {
            Ok(()) => self.idle_note(name, false),
            Err(why) => self.notice = Some(why),
        }
    }

    /// The quiet line under the top bar after an input is set while the
    /// program is not running (Nick, 2026-11-16): nothing reacts until
    /// it runs, and the input keeps what it was set to. It goes when the
    /// program runs. `push`: a push button, which is on only while held.
    fn idle_note(&mut self, name: &str, push: bool) {
        use crate::playground::RunState;
        let state = self.session.state();
        if state == RunState::Running {
            return;
        }
        let low = name.to_ascii_lowercase();
        let now = self
            .session
            .inputs()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map_or_else(String::new, |p| show(p.ty, p.value));
        let word = state.word();
        let note = match (push, state) {
            (true, _) => format!(
                "The program is {word}, so nothing reacts to {low} yet. A push button is on only while it is held."
            ),
            (false, RunState::Paused) => {
                format!("The program is paused, so nothing reacts yet. It goes on with {low} at {now}.")
            }
            (false, _) => {
                format!("The program is {word}, so nothing reacts yet. It starts with {low} at {now}.")
            }
        };
        self.idle_notice = Some(note.clone());
        self.notice = Some(note);
    }

    /// A push button, on the diagram or in the I/O tab, says whether it
    /// is held this frame. The input is on while either is: both are
    /// drawn for one input, and the one not held must not release it.
    fn hold(&mut self, name: &str, on: bool) {
        match self.pushed.iter_mut().find(|(n, _)| n == name) {
            Some((_, h)) => *h |= on,
            None => self.pushed.push((name.to_string(), on)),
        }
    }

    /// What a card of the I/O tab asked for.
    fn io_asked(&mut self, name: &str, input: bool, a: crate::io_cards::Asked, chrome: &mut Chrome) {
        if a.details {
            let key = name.to_ascii_lowercase();
            if !self.io_open.remove(&key) {
                self.io_open.insert(key);
            }
        }
        if a.simulate {
            self.open_sim_popup(name);
        }
        if a.code {
            self.find_in_code(name, chrome);
        }
        if a.trend {
            let pin = self
                .session
                .trend
                .signals
                .iter()
                .find(|t| t.eq_ignore_ascii_case(name))
                .cloned()
                .unwrap_or_else(|| name.to_string());
            let added = !self.session.trend.signals.contains(&pin);
            self.session.trend.toggle(&pin);
            if added {
                chrome.trend = true;
                chrome.msgs = false;
            }
        }
        if a.diagram
            && let Some(v) = &mut self.viewer
        {
            v.show_signal(name);
        }
        if a.tapped && !input {
            self.pick(name);
        }
        // An input's card goes to where it is, as an output's does
        // (Nick, 2026-11-26).
        if a.tapped
            && input
            && let Some(v) = &mut self.viewer
        {
            v.show_signal(name);
        }
    }

    /// The `< >` glyph of a point's card: the file that declares it
    /// opens in the Code tab with the find box on its name, the first
    /// place it is named selected. Asked again, the next place.
    fn find_in_code(&mut self, name: &str, chrome: &mut Chrome) {
        let again = chrome
            .find
            .as_ref()
            .is_some_and(|f| f.word && f.query.eq_ignore_ascii_case(name));
        if !again
            && let Some(line) = self.viewer.as_ref().and_then(|v| v.declared_line(name))
            && let Some((file, _)) = self.proj.locate(line)
        {
            self.proj.open_file(&file);
        }
        chrome.show_code();
        crate::find::Find::name(&mut chrome.find, &name.to_ascii_lowercase());
    }

    /// The signals to trend (outputs, then inputs) and the chart.
    fn trend_tab(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        chrome.hit("trend tab", ui.max_rect());
        if self.session.build.is_none() {
            ui.label(RichText::new("Nothing compiled yet.").color(t.muted));
            return;
        }
        let points: Vec<String> = self
            .session
            .outputs()
            .chain(self.session.inputs())
            .map(|p| p.name.clone())
            .collect();
        // Which of them are still simulated: the legend says so.
        self.assign_context();
        let simulated: Vec<bool> = points
            .iter()
            .map(|n| {
                crate::assign::designate(&self.devices_ui.map, n, true).is_some_and(|d| d.is_simulated())
            })
            .collect();
        // The heading shares the signals' row: on a line of its own it
        // and the slider left the plot the smallest part of the panel.
        // The rows wrap, and scroll past a third of the tab: an Agent's
        // program has dozens of points, and unbounded their rows took the
        // whole panel and left the plot no height (Nick, 2026-11-22; the
        // third, 2026-11-23).
        // Whole rows only: a cap that cuts a row in half shows a sliver.
        let (row, gap) = (
            (ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y).ceil(),
            ui.spacing().item_spacing.y,
        );
        // Three rows while they take no more than half the tab: a handful
        // of signals all show (Nick, 2026-11-23) at the width the wider
        // left pane leaves (2026-11-26), where a third held two.
        let three = 3.0 * (row + gap) - gap;
        let cap = (ui.available_height() * SIGNAL_ROWS_SHARE)
            .max(three.min(ui.available_height() * 0.5))
            .max(SIGNAL_ROWS_MIN);
        let rows = ((cap + gap) / (row + gap)).floor().max(1.0);
        let rows_h = rows * (row + gap) - gap;
        let legend = egui::ScrollArea::vertical()
            .id_salt("pg-trend-signals")
            .max_height(rows_h)
            .show(ui, |ui| {
                // Room for the scroll bar, or the panel grows to fit it.
                ui.set_max_width(ui.available_width() - 12.0);
                ui.horizontal_wrapped(|ui| {
                    // A button that does not fit moves to the next row
                    // whole; left to wrap its text it became a sliver.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.label(RichText::new("Signals").strong().color(t.muted))
                        .tip(format!(
                            "Click a signal to pin it to the trend, up to {TREND_SIGNALS}."
                        ));
                    for (name, sim) in points.iter().zip(&simulated) {
                        let on = self.session.trend.signals.contains(name);
                        let tip = match (on, sim) {
                            (true, false) => "On the trend. Click to take it off.".to_string(),
                            (false, false) => "Click to pin it to the trend.".to_string(),
                            (true, true) => "Simulated. On the trend. Click to take it off.".to_string(),
                            (false, true) => "Simulated. Click to pin it to the trend.".to_string(),
                        };
                        let label = name.to_ascii_lowercase();
                        let label = if *sim {
                            format!("{label} {}", crate::assign::Designation::BADGE)
                        } else {
                            label
                        };
                        let b = crate::theme::tab(ui, on, &label, egui::Vec2::ZERO).tip(tip);
                        chrome.hit(format!("trend signal {}", name.to_ascii_lowercase()), b.rect);
                        if *sim {
                            chrome.hit(format!("trend sim {}", name.to_ascii_lowercase()), b.rect);
                        }
                        if b.clicked() {
                            self.session.trend.toggle(name);
                        }
                    }
                });
            });
        chrome.hit("trend signals", legend.inner_rect);
        window_slider(ui, chrome, &mut self.session.trend.window_ms);
        ui.add_space(6.0);
        if self.session.trend.flashes.tick(ui.input(|i| i.time)) {
            ui.ctx().request_repaint();
        }
        let plot = self.trend_plot(ui, chrome);
        chrome.hit("trend plot", plot);
        if let Some((from, to)) = lane_handles(ui, chrome, plot, &self.session.trend.signals) {
            self.session.trend.move_signal(from, to);
        }
    }

    fn claude(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        // Where the diagram's selection is being used: a click here keeps it.
        chrome.hit("agent tab", ui.clip_rect());
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().id_salt("pg-claude").show(ui, |ui| {
            // Room for the scroll bar: a full-width field plus the bar is
            // wider than the panel, and a resizable panel grows to fit.
            ui.set_max_width(ui.available_width() - 12.0);
            // The page never asks for a key or a model. The free chat,
            // closed, says so and asks its one question in place of the
            // chat box; with no route at all the tab says so.
            let free = self.chat.free_base().is_some();
            if free && self.free_closed_ui(ui, chrome) {
                return;
            }
            if self.chat.unavailable() {
                let r = ui.label(RichText::new(crate::claude::UNAVAILABLE_TEXT).color(t.muted));
                chrome.hit("agent unavailable", r.rect);
                return;
            }
            match self.chat.proxy {
                _ if free => {}
                Proxy::Yes => {
                    ui.label(RichText::new("Using the local proxy").color(t.muted))
                        .tip("The API key stays on this machine.");
                }
                Proxy::Checking => {
                    ui.label(RichText::new("Checking for a local proxy…").color(t.muted))
                        .tip("With a local proxy the API key stays on this machine.");
                }
                Proxy::No => {}
            }
            ui.add_space(4.0);
            self.scope_ui(ui, chrome);
            // A new program starts from its I/O, named by the engineer.
            if self.scope() == crate::scope::Scope::NewProject {
                crate::io_names::ui(ui, chrome, &mut self.new_io);
            }
            let view = crate::design_ui::EntryView {
                now: ctx.input(|i| i.time),
                blocked: self.design_blocked(),
                keep_blocked: self.design_keep_blocked(),
                kept: self.kept_design(),
            };
            if let Some(act) = crate::design_ui::entry_ui(ui, chrome, &mut self.design, &view) {
                self.design_act(act, &ctx);
            }
            let view = crate::instruct::View {
                target: self.scope().label().replacen("Editing: ", "", 1),
                blocked: self.chat.route().err().map(str::to_string),
                undo_blocked: self.undo_blocked(),
                decided: std::collections::BTreeMap::new(),
                steps: self.exchange_steps(),
                room_below: Some(0.0),
                progress: self.progress(ctx.input(|i| i.time)),
                card: self.card.clone(),
                big: self
                    .big
                    .as_ref()
                    .filter(|b| b.0 == self.instruct.draft)
                    .map(|b| b.1),
                io: Some(self.phrasing_io()),
                versions: Some(self.versions()),
            };
            match crate::instruct::ui(ui, chrome, &mut self.instruct, &view) {
                Some(crate::instruct::Act::Send) => self.send_instruction(&ctx),
                Some(crate::instruct::Act::Split) => {
                    self.send_instruction_as(&ctx, crate::instruct::Way::Steps);
                }
                Some(crate::instruct::Act::OneGo) => {
                    self.send_instruction_as(&ctx, crate::instruct::Way::OneGo);
                }
                Some(crate::instruct::Act::Stop) => self.stop_answer(),
                Some(crate::instruct::Act::Undo) => self.undo_change(),
                Some(crate::instruct::Act::RunSteps) => self.run_steps(&ctx),
                Some(crate::instruct::Act::Revision(n)) => self.show_revision(n),
                Some(crate::instruct::Act::Details(n)) => self.act(Act::ShowRevision(n)),
                Some(crate::instruct::Act::Answer(k)) => self.answer_question(k, &ctx),
                Some(crate::instruct::Act::Offer(k)) => self.take_offer(k, &ctx),
                Some(crate::instruct::Act::Review(_)) | None => {}
            }
        });
    }

    /// The inputs and outputs of the program on screen as its source
    /// spells them, for the phrasing help in the Describe tab (a built-in
    /// example's too: a change to it starts from its code); none when no
    /// program is open.
    fn phrasing_io(&self) -> Vec<crate::phrasing::Io> {
        let src = &self.session.src;
        self.session
            .inputs()
            .chain(self.session.outputs())
            // A bare address (`%QX0.1`) is no name to spell.
            .filter(|p| !p.name.starts_with('%'))
            .map(|p| crate::phrasing::Io {
                name: crate::phrasing::spelled(src, &p.name),
                input: p.dir == crate::playground::Dir::In,
                boolean: p.ty == Ty::Bool,
            })
            .collect()
    }

    /// The free chat while it is closed or cannot be reached: what
    /// happened, and once, which service the visitor would want here.
    /// `false` when it is open or still being asked (the chat box shows).
    fn free_closed_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) -> bool {
        use crate::claude::{Free, SURVEY_CHOICES, SURVEY_OTHER_CHARS};
        let t = theme(ui);
        let (text, tip) = match self.chat.free {
            Free::Open | Free::Checking => return false,
            Free::Closed { reopens: true } => (
                "The free chat has used up its turns for now. It opens again by itself, usually within a day.",
                "The free chat runs on a small daily allowance shared by everyone who tries the playground.",
            ),
            Free::Closed { reopens: false } => (
                "The free chat is closed for now. It opens again once we have looked at some unusual traffic.",
                "The free chat closes itself when it sees far more requests than people trying the playground would send.",
            ),
            Free::Unreachable => (
                "The free chat cannot be reached from here. The editor, the diagram and the simulation work without it.",
                "The page could not reach the service that writes the changes. A firewall or an ad blocker can cause this.",
            ),
        };
        let r = ui.label(RichText::new(text).strong()).tip(tip);
        chrome.hit("agent free closed", r.rect);
        ui.add_space(8.0);
        if self.chat.surveyed {
            ui.label(RichText::new("Thank you. That is counted.").color(t.muted))
                .tip("One answer is counted from each visitor.");
            return true;
        }
        if matches!(self.chat.free, Free::Unreachable) {
            return true;
        }
        let ask = "Which AI service would you want to use here?";
        ui.label(ask)
            .tip("One click, counted with everyone else's. Nothing else about you is kept.");
        let mut pick: Option<&str> = None;
        // Wrapped: a row of buttons wider than the panel would widen it.
        ui.horizontal_wrapped(|ui| {
            for (id, label) in SURVEY_CHOICES {
                let b = ui.button(label).tip(format!("Counts one answer for: {label}."));
                chrome.hit(format!("survey {id}"), b.rect);
                if b.clicked() {
                    pick = Some(id);
                }
            }
        });
        ui.add_space(4.0);
        let field = ui
            .add(
                egui::TextEdit::singleline(&mut self.survey_other)
                    .id_salt("pg-survey-other")
                    .char_limit(SURVEY_OTHER_CHARS)
                    .hint_text("Another one")
                    .desired_width(f32::INFINITY),
            )
            .tip("Name another service, in a few words. These words are kept for 90 days.");
        chrome.hit("survey other field", field.rect);
        let send = ui
            .add_enabled(!self.survey_other.trim().is_empty(), egui::Button::new("Send"))
            .tip("Sends the name you typed.");
        chrome.hit("survey other send", send.rect);
        if send.clicked() {
            pick = Some("other");
        }
        if let Some(choice) = pick
            && let Some(req) = self.chat.survey(choice, &self.survey_other)
        {
            self.link.post(req);
        }
        true
    }

    /// The Agent tab's scope line (card agent-scoped-edits): what the next
    /// instruction may change and which rule gave it. Shown while a
    /// selection or a pick narrows it, with Whole file, and while the file
    /// it names is not the one in the editor (a test is open, and the
    /// instruction changes the program).
    fn scope_ui(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let scope = self.scope();
        let elsewhere = scope.file().is_some_and(|f| f != self.proj.current);
        if scope.lines().is_none() && !elsewhere {
            return;
        }
        let rule = if elsewhere && scope.lines().is_none() {
            "the program, because the file open in the Code tab is not one an instruction changes"
        } else {
            scope.rule()
        };
        let mut whole = false;
        // Wrapped: a long name in a plain row would widen the panel.
        ui.horizontal_wrapped(|ui| {
            let r = ui.label(RichText::new(scope.label()).strong()).tip(rule);
            chrome.hit("agent scope", r.rect);
            if scope.lines().is_some() {
                let b = ui
                    .small_button("Whole file")
                    .tip("Drops the selection and the pick: the change goes into the whole open file.");
                chrome.hit("scope whole file", b.rect);
                whole = b.clicked();
            }
        });
        if whole {
            self.whole_file(&ui.ctx().clone());
        }
        ui.add_space(4.0);
    }

    /// Draws the trend; returns the rect it filled (the lanes' handles go
    /// over it).
    fn trend_plot(&self, ui: &mut egui::Ui, chrome: &Chrome) -> Rect {
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
            return rect;
        }
        if trend.history.is_empty() && !self.session.running {
            // Loaded and not run yet: say so, where empty lanes read as
            // a trend that broke.
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Nothing recorded yet. Run the program to draw its signals.",
                FontId::proportional(crate::theme::MIN_TEXT),
                t.muted,
            );
            return rect;
        }
        let n = trend.signals.len();
        #[allow(clippy::cast_precision_loss)]
        let lane = rect.height() / n as f32;
        // The head sits at the newest scan; stopped, it stands still.
        let sweep = crate::ekg::Sweep::new(trend.window_ms, trend.history.last_t().unwrap_or(0));
        let now = ui.input(|i| i.time);
        for (k, name) in trend.signals.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let top = rect.top() + lane * k as f32;
            let runs = sweep.runs(trend.history.lane(name, sweep.oldest_ms()));
            let (lo, hi) = runs
                .iter()
                .flatten()
                .map(|p| p.1)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let number = trend
                .history
                .last_value(name)
                .filter(|v| v.is_finite())
                .map(helmsman_iec::shown_real);
            crate::ekg::paint_lane(
                &painter,
                &t,
                rect,
                (top, lane),
                &crate::ekg::Lane {
                    name,
                    color: lane_color(&t, chrome, name, k),
                    number,
                    pop: trend.flashes.strength(name, now),
                },
                &sweep,
                &runs,
                nice_range(lo, hi),
            );
        }
        rect
    }
}

/// One pixel column of a lane's samples: its first, lowest, highest and
/// last, each with its place in the column's order.
struct Column {
    x: f32,
    n: usize,
    first: (usize, f32, f32),
    lo: (usize, f32, f32),
    hi: (usize, f32, f32),
    last: (usize, f32, f32),
}

impl Column {
    fn new(c: f32, x: f32, y: f32) -> Self {
        let e = (0, x, y);
        Self {
            x: c,
            n: 1,
            first: e,
            lo: e,
            hi: e,
            last: e,
        }
    }

    fn add(&mut self, x: f32, y: f32) {
        let e = (self.n, x, y);
        self.n += 1;
        if y < self.lo.2 {
            self.lo = e;
        }
        if y > self.hi.2 {
            self.hi = e;
        }
        self.last = e;
    }

    /// Its kept samples onto `pts`, in time order, each as a step.
    fn flush(&self, pts: &mut Vec<Pos2>) {
        let mut kept = [self.first, self.lo, self.hi, self.last];
        kept.sort_by_key(|e| e.0);
        let mut prev = None;
        for (k, x, y) in kept {
            if prev == Some(k) {
                continue;
            }
            prev = Some(k);
            // Steps, not slopes: a PLC value holds until the next scan.
            if let Some(before) = pts.last().copied() {
                pts.push(Pos2::new(x, before.y));
            }
            pts.push(Pos2::new(x, y));
        }
    }
}

/// The polyline of a lane drawn from its samples (x, y in points, x
/// rising): a step per sample, but each pixel column of them reduced to
/// its first, lowest, highest and last, in time order. A 10 minute
/// window of 10 ms scans is 60 000 samples and draws a few thousand
/// points, the same picture.
pub fn lane_points(samples: impl Iterator<Item = (f32, f32)>) -> Vec<Pos2> {
    let mut pts = Vec::new();
    let mut column: Option<Column> = None;
    for (x, y) in samples {
        match &mut column {
            Some(c) if c.x.to_bits() == x.floor().to_bits() => c.add(x, y),
            _ => {
                if let Some(c) = column.take() {
                    c.flush(&mut pts);
                }
                column = Some(Column::new(x.floor(), x, y));
            }
        }
    }
    if let Some(c) = column {
        c.flush(&mut pts);
    }
    pts
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

/// Width of a lane's label column, which is also its drag handle.
pub const LANE_HANDLE_W: f32 = 88.0;

/// The Trend tab's signal buttons wrap, and take at most this share of its
/// height before they scroll, so the plot keeps the rest.
const SIGNAL_ROWS_SHARE: f32 = 1.0 / 3.0;
/// ... and never less than one row of them.
const SIGNAL_ROWS_MIN: f32 = 34.0;

/// A place in the trend's palette as it is drawn on this page: the brand
/// colours, as ink on the light page, where the last (white) is the
/// page's own ink.
#[must_use]
pub fn palette_color(t: &Theme, index: usize) -> Color32 {
    match index % TREND.len() {
        5 if !t.dark => t.fg,
        i => t.ink(TREND[i]),
    }
}

/// The colour of trend signal `name` in lane `k` (both trends): the one
/// picked for it ([`Chrome::set_color`]), else the colour its name asks
/// for (`red`, `lamp_green`: the lamp's own, Nick, 2026-11-12), else its
/// lane's.
#[must_use]
pub fn lane_color(t: &Theme, chrome: &Chrome, name: &str, k: usize) -> Color32 {
    match (chrome.color(name), crate::lamps::colour(name)) {
        (Some(i), _) => palette_color(t, i),
        (None, Some(c)) => t.ink(c),
        (None, None) => palette_color(t, k),
    }
}

/// The lane labels of a trend drawn in `rect`, one lane per name, as
/// drag handles (both trends; Nick, 2026-11-07): drag a label up or down
/// and its signal moves to the lane it is dropped on, a bar marking
/// where it will land. A click on a label opens the palette, and the
/// swatch picked is that signal's colour from then on, on either trend
/// (Nick, 2026-11-11). The hits are `trend lane <name>` and, while the
/// palette is open, `trend color <k>`. Returns `(from, to)` when a drag
/// ends on another lane.
pub fn lane_handles(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    rect: Rect,
    names: &[String],
) -> Option<(usize, usize)> {
    let n = names.len();
    if n == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let lane = rect.height() / n as f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lane_at = |y: f32| (((y - rect.top()) / lane).floor().max(0.0) as usize).min(n - 1);
    let accent = theme(ui).accent;
    let mut moved = None;
    for (k, name) in names.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let top = rect.top() + lane * k as f32;
        let handle = Rect::from_min_size(Pos2::new(rect.left(), top), Vec2::new(LANE_HANDLE_W, lane));
        let r = ui
            .interact(
                handle,
                ui.scope_id().with(("trend lane", k)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Grab)
            .tip("Click to pick this signal's colour. Drag up or down to move it to another lane.");
        chrome.hit(format!("trend lane {}", name.to_ascii_lowercase()), handle);
        let picked = chrome.color(name).unwrap_or(k) % TREND.len();
        let mut pick = None;
        let mut swatches = Vec::new();
        egui::Popup::menu(&r).show(|ui| {
            let t = theme(ui);
            ui.horizontal(|ui| {
                for i in 0..TREND.len() {
                    let (rect, s) = ui.allocate_exact_size(Vec2::splat(TAP * 0.75), Sense::click());
                    let s = s.tip("This signal's line and label take this colour.");
                    ui.painter()
                        .rect_filled(rect.shrink(4.0), CornerRadius::same(4), palette_color(&t, i));
                    if i == picked || s.hovered() {
                        ui.painter().rect_stroke(
                            rect.shrink(1.0),
                            CornerRadius::same(6),
                            Stroke::new(2.0, t.fg),
                            egui::StrokeKind::Inside,
                        );
                    }
                    swatches.push((format!("trend color {i}"), rect));
                    if s.clicked() {
                        pick = Some(i);
                    }
                }
            });
        });
        chrome.hits.extend(swatches);
        if let Some(i) = pick {
            chrome.set_color(name, i);
        }
        let at = ui.ctx().input(|i| i.pointer.latest_pos());
        if r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(p) = at {
                let to = lane_at(p.y);
                // The bar sits on the edge the signal will take: above
                // the target lane moving up, below it moving down.
                #[allow(clippy::cast_precision_loss)]
                let y = rect.top() + lane * (if to > k { to + 1 } else { to }) as f32;
                ui.painter_at(rect).line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    Stroke::new(3.0, accent),
                );
            }
        }
        if r.drag_stopped()
            && let Some(p) = at
        {
            let to = lane_at(p.y);
            if to != k {
                moved = Some((k, to));
            }
        }
    }
    moved
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

/// The room kept at a trend's right edge for [`scale_labels`]: a trace
/// that ran to the edge was drawn under them once the window filled.
pub const SCALE_W: f32 = 64.0;

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
        let mut open = None;
        let mut set = Vec::new();
        for (k, (path, rect)) in reads.iter().enumerate() {
            let Some(p) = inputs.iter().find(|p| p.name.eq_ignore_ascii_case(path)) else {
                continue;
            };
            if !area.intersects(*rect) {
                continue;
            }
            self.input_rects.push((p.name.to_ascii_lowercase(), *rect));
            mark_mode(
                ui,
                *rect,
                self.session
                    .plant
                    .as_ref()
                    .map_or(helmsman_sim::Mode::Normal, |pl| pl.mode(&p.name)),
            );
            if double_clicked(ui, *rect) {
                open = Some(p.name.clone());
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
            self.hold(&name, on);
        }
        if let Some(name) = open {
            self.open_sim_popup(&name);
        }
        for (name, v) in set {
            self.set_by_hand(&name, v);
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

    /// "Use this device": a read-only example is copied into a project of
    /// the visitor's own first, a project without an `io.map` gets the one
    /// its program's I/O implies (on the box's own channels, which the
    /// rewire question then moves), and the form opens filled in.
    /// A project of the visitor's own to wire a device into: a read-only
    /// example is copied first, and the copy gets the io.map its program
    /// implies. `false` when the copy could not be made.
    fn own_project(&mut self) -> bool {
        if self.proj.is_example() {
            let from = self.proj.open.clone();
            let to = (1..=99)
                .map(|n| {
                    if n == 1 {
                        format!("my-{from}")
                    } else {
                        format!("my-{from}-{n}")
                    }
                })
                .map(|n| n.chars().take(48).collect::<String>())
                .find(|n| !self.proj.exists(n))
                .unwrap_or_default();
            self.proj.create(&to, Some(&from));
            self.reload();
            if self.proj.is_example() {
                return false;
            }
            self.proj.note(
                format!("{from} is a read-only example, so it was copied as {to} to wire the device in."),
                false,
            );
        }
        if !self.proj.files.contains_key(helmsman_project::IO_MAP) {
            let map = crate::sim_projects::example_io_map(&self.proj.build_src());
            self.proj.add_file(helmsman_project::IO_MAP, map);
        }
        true
    }

    fn use_device(&mut self, d: crate::devices::NewDevice) {
        if !self.own_project() {
            return;
        }
        // The copy's program and map, for the form's channels.
        let map = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        self.devices_ui.map = helmsman_project::iomap::skim(&map);
        self.devices_ui.program = crate::device_form::ports_of(&self.proj.build_src());
        self.devices_ui.open(d);
    }

    /// The form's Add: new variables into the program file (refused, with
    /// nothing changed, if the program would not read after), then the
    /// device and its signals into io.map.
    fn apply_wiring(&mut self, w: &crate::device_form::Wiring) {
        if !w.declare.is_empty() {
            let Some(file) = crate::device_form::program_file(
                self.proj.files.iter().map(|(n, t)| (n.as_str(), t.as_str())),
            ) else {
                self.proj.note("No PROGRAM to add the new variables to.", true);
                return;
            };
            let mut src = self.proj.files[&file].clone();
            for (dir, name, ty) in &w.declare {
                let Some(s) = crate::device_form::declare_var(&src, *dir, name, *ty) else {
                    self.proj
                        .note(format!("{file} has no PROGRAM to add {name} to."), true);
                    return;
                };
                src = s;
            }
            if helmsman_iec::ast::parse(&self.proj.files[&file]).is_ok()
                && helmsman_iec::ast::parse(&src).is_err()
            {
                self.proj.note(
                    "A new variable's name is an ST keyword: rename it and Add again.",
                    true,
                );
                return;
            }
            if let Some(t) = self.proj.files.get_mut(&file) {
                *t = src;
            }
        }
        let map = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        let map = crate::device_form::apply_map(&map, w);
        match self.proj.files.get_mut(helmsman_project::IO_MAP) {
            Some(t) => *t = map,
            None => self.proj.add_file(helmsman_project::IO_MAP, map),
        }
        self.proj.note(w.summary(), false);
        self.edited(f64::NEG_INFINITY);
    }

    /// Why io.map cannot be written, when it cannot.
    fn assign_write(&self) -> Result<(), String> {
        if self.proj.is_example() {
            Err("The examples are read-only: copy the project first (File › Copy project).".into())
        } else {
            Ok(())
        }
    }

    /// What a menu on `port` asked for: the question of which channel, or
    /// the port back on simulated I/O at once.
    fn assign_ask(&mut self, port: String, ask: crate::assign_ui::PortAsk) {
        match ask {
            crate::assign_ui::PortAsk::Assign => {
                self.devices_ui.assigning = Some(crate::assign_ui::Ask::Port(port));
            }
            crate::assign_ui::PortAsk::Back => self.assign_act(&crate::assign::AssignAct::Back { port }),
        }
    }

    /// Writes an assignment (or its undoing) into io.map: refused, with
    /// nothing changed and the reason in a note, when it cannot be made.
    fn assign_act(&mut self, a: &crate::assign::AssignAct) {
        if let Err(why) = self.assign_write() {
            self.proj.note(why, true);
            return;
        }
        let text = self
            .proj
            .files
            .get(helmsman_project::IO_MAP)
            .cloned()
            .unwrap_or_default();
        self.assign_context();
        let ports = crate::device_form::ports_of(&self.proj.build_src());
        match a.run(&text, &ports, &|d| self.devices_ui.channels_of(d)) {
            Ok(c) => {
                match self.proj.files.get_mut(helmsman_project::IO_MAP) {
                    Some(t) => *t = c.text,
                    None => self.proj.add_file(helmsman_project::IO_MAP, c.text),
                }
                self.proj.note(c.summary, false);
                self.edited(f64::NEG_INFINITY);
                self.plant_off(a);
            }
            Err(why) => self.proj.note(why, true),
        }
    }

    /// A port that went to a device has its `plant.sim` line switched off
    /// (kept, not deleted), and back on when it returns to simulated I/O.
    fn plant_off(&mut self, a: &crate::assign::AssignAct) {
        let (port, off) = match a {
            crate::assign::AssignAct::Assign { port, .. } => (port, true),
            crate::assign::AssignAct::Back { port } => (port, false),
        };
        let Some(text) = self.proj.files.get_mut(helmsman_project::PLANT) else {
            return;
        };
        if let Some(new) = helmsman_sim::set_off(text, port, off) {
            *text = new;
            let text = text.clone();
            self.session.set_plant(&text);
        }
    }

    /// What a right-click on each port's block in the diagram offers.
    fn assign_menus(&mut self) -> Vec<crate::assign::PortMenu> {
        self.assign_context();
        self.devices_ui.port_menus(&self.assign_write())
    }

    /// The open project's program's inputs and outputs, as its files stand
    /// now (a compile only when they changed).
    fn program_ports(&self) -> Vec<crate::device_form::Port> {
        let src = self.proj.build_src();
        let mut memo = self.ports_memo.borrow_mut();
        if let Some((text, ports)) = memo.as_ref()
            && *text == src
        {
            return ports.clone();
        }
        let ports = crate::device_form::ports_of(&src);
        *memo = Some((src, ports.clone()));
        ports
    }

    /// The map and the program's ports, as the question about an
    /// assignment reads them this frame.
    fn assign_context(&mut self) {
        self.devices_ui.map = helmsman_project::iomap::skim(
            self.proj
                .files
                .get(helmsman_project::IO_MAP)
                .map_or("", String::as_str),
        );
        self.devices_ui.program = self.program_ports();
        self.devices_ui.set_profiles(
            self.proj
                .files
                .iter()
                .filter(|(n, _)| n.ends_with(helmsman_project::DEVICE_EXT))
                .map(|(n, t)| (n.as_str(), t.as_str())),
        );
    }

    /// The inputs the diagram shows a control for.
    /// Every `wired=` value of the map not looked up yet, queued for the
    /// library (the Devices and I/O tabs' line under a point).
    fn want_wired(&mut self) {
        // The walkthrough's note comes from the index read in the page.
        if self.devices_ui.wiring_wanted().is_some() {
            self.wired_lib.want_index();
        }
        if let Some(index) = &self.wired_lib.index {
            self.devices_ui.wiring_from_index(index);
        }
        while let Some(w) = self.devices_ui.wired_wanted() {
            self.wired_lib.want(w);
        }
    }

    /// The picker's choice: `wired=` written on (or taken off) `signal`'s
    /// line in io.map.
    fn set_wired(&mut self, signal: &str, wired: Option<&str>) {
        let Some(t) = self.proj.files.get_mut(helmsman_project::IO_MAP) else {
            return;
        };
        match crate::wired::set_wired(t, signal, wired) {
            Some(new) => {
                *t = new;
                let what = match wired {
                    Some(w) => format!("{signal} is wired to {w} in io.map."),
                    None => format!("{signal}'s wired= taken out of io.map."),
                };
                self.proj.note(what, false);
                self.edited(f64::NEG_INFINITY);
            }
            None => self
                .proj
                .note(format!("{signal} has no line in io.map to record it on."), true),
        }
    }

    #[cfg(test)]
    pub(crate) fn on_map(&self) -> Vec<String> {
        let Some(v) = &self.viewer else { return Vec::new() };
        v.reads_on_screen().into_iter().map(|(path, _)| path).collect()
    }
}

/// Whether the pointer's primary button was double-clicked inside `rect`
/// this frame.
fn double_clicked(ui: &egui::Ui, rect: Rect) -> bool {
    ui.input(|i| {
        i.pointer.button_double_clicked(egui::PointerButton::Primary)
            && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
    })
}

/// An input not behaving normally is outlined: yellow while abnormal, red
/// while absent.
fn mark_mode(ui: &egui::Ui, rect: Rect, mode: helmsman_sim::Mode) {
    let color = match mode {
        helmsman_sim::Mode::Normal => return,
        helmsman_sim::Mode::Absent => RED,
        _ => YELLOW,
    };
    ui.painter().rect_stroke(
        rect.expand(2.0),
        3.0,
        Stroke::new(2.0, color),
        egui::StrokeKind::Outside,
    );
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
                .on_hover_text("push button: on while held · double-click: simulate it");
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
                .on_hover_text("switch: click to flip · double-click: simulate it")
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

const TYPE_HINT: &str =
    "type a value, then Enter (Esc puts it back); double-click to simulate it as a device";

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

    fn go_history(&mut self) {
        self.open_history();
    }

    fn undo(&mut self, ctx: &egui::Context, redo: bool) -> bool {
        let id = self.source_id();
        let Some(text) = self.proj.current_text_mut() else {
            return false;
        };
        let done = editor::undo(ctx, id, text, redo);
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
    /// typed text reaches the program running. One that does not compile is
    /// refused, its errors listed, and the program running is unchanged.
    fn deploy(&mut self) {
        // A red requirement test is said with the deploy, which goes on.
        let red = self.tests_warn(None);
        // What runs is what the files say, in the one layout.
        if self.proj.format() > 0 {
            self.edited(f64::NEG_INFINITY);
        }
        let src = self.proj.build_src();
        let carried = self.session.deploy(&src);
        self.pending = None;
        self.sync_viewer();
        if let Some(carried) = carried {
            self.session.set_running(true);
            self.seen.running = Some(true);
            self.seen.scans = self.session.scans;
            self.proj.running = Some(self.proj.open.clone());
            let program = self
                .viewer
                .as_ref()
                .and_then(|v| v.module.program())
                .map_or_else(String::new, |p| format!(": {}", p.name.to_ascii_lowercase()));
            let note = format!(
                "{} is running in this page{program}.{}",
                self.proj.open,
                carried.describe()
            );
            self.running_notice = Some(note.clone());
            self.notice = Some(
                red.as_ref()
                    .map_or_else(|| note.clone(), |why| format!("{note} {why}")),
            );
            self.log.info(
                self.session.now_ms as f64 / 1000.0,
                format!("Deployed {}: running in this page{program}.", self.proj.open),
            );
            if let Some(why) = red {
                self.log.warning(
                    self.session.now_ms as f64 / 1000.0,
                    format!("Deployed {} with a test red. {why}", self.proj.open),
                );
            }
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

/// One input: its control, and the response a right-click on its row
/// opens a menu from. `place` is where the point is now, in its tooltip.
/// An input's glyph says its state: green on, red off (Nick, 2026-11-26);
/// a number's is plain.
fn state_ink(t: &crate::theme::Theme, v: Value) -> egui::Color32 {
    match v {
        Value::Bool(true) => t.ink(crate::st_view::brand::GREEN),
        Value::Bool(false) => t.ink(crate::st_view::brand::RED),
        _ => t.fg,
    }
}

fn input_ui(ui: &mut egui::Ui, p: &Point, place: &str, tile: bool) -> (Option<Value>, egui::Response) {
    let t = theme(ui);
    let label = match p.address() {
        Some(a) => format!("{}  {a}", p.name.to_ascii_lowercase()),
        None => p.name.to_ascii_lowercase(),
    };
    let mut out = None;
    let mut anchor: Option<egui::Response> = None;
    // Wrapped, with a slider that fits: a row wider than the panel grows it.
    ui.spacing_mut().slider_width = (ui.available_width() - 130.0).clamp(40.0, 100.0);
    let glyph = crate::glyph_lib::for_point(&p.name, true, p.ty == Ty::Bool);
    // What a button gets beside the glyph: any wider and it wraps under it.
    let beside = (ui.available_width() - crate::glyphs::ROW - ui.spacing().item_spacing.x - 1.0).max(40.0);
    // A button is that wide and no wider: its name is cut, never the row.
    let size = Vec2::new(if tile { beside.min(TAP * 3.0) } else { beside }, TAP * 0.8);
    // A tile is too narrow for a slider beside the box: the box alone.
    let slider = ui.available_width() >= 150.0;
    let boxed = Vec2::new((ui.available_width() - 2.0).clamp(40.0, 84.0), TAP * 0.6);
    ui.horizontal_wrapped(|ui| {
        crate::glyph_lib::show(
            ui,
            glyph,
            crate::glyphs::ROW,
            state_ink(&t, p.value),
            crate::glyph_lib::tip(glyph),
        );
        match p.widget {
            Some(Widget::Push) => {
                let on = p.value == Value::Bool(true);
                let b =
                    egui::Button::new(
                        RichText::new(&label)
                            .strong()
                            .color(if on { BLACK } else { t.fg }),
                    )
                    .fill(if on { YELLOW } else { t.rule })
                    .sense(Sense::click_and_drag());
                let r = control_button(ui, b, size, tile).tip(format!(
                    "Push button: on while held. Click the card for its details.\n{place}"
                ));
                out = Some(Value::Bool(r.is_pointer_button_down_on()));
                anchor = Some(r);
            }
            Some(Widget::Toggle) => {
                let on = p.value == Value::Bool(true);
                let b = egui::Button::new(
                    RichText::new(format!("{label}: {}", if on { "ON" } else { "OFF" }))
                        .strong()
                        .color(if on { BLACK } else { t.fg }),
                )
                .fill(if on { YELLOW } else { t.rule });
                let r = control_button(ui, b, size, tile).tip(format!(
                    "Switch: click to flip. Click the card for its details.\n{place}"
                ));
                if r.clicked() {
                    out = Some(Value::Bool(!on));
                }
                anchor = Some(r);
            }
            Some(Widget::Int { min, max }) => {
                anchor = Some(row_label(ui, &label, place));
                let lo = i64::try_from(min.max(-1_000_000)).unwrap_or(i64::MIN);
                let hi = i64::try_from(max.min(1_000_000)).unwrap_or(i64::MAX);
                let (slo, shi) = (lo.max(-1000), hi.min(1000));
                let mut v = match p.value {
                    Value::Int(n) => i64::try_from(n).unwrap_or(0),
                    _ => 0,
                };
                let before = v;
                if slider {
                    ui.add(
                        egui::Slider::new(&mut v, slo..=shi)
                            .clamping(egui::SliderClamping::Edits)
                            .show_value(false),
                    )
                    .tip(format!(
                        "Drag to set the input, {slo} to {shi}. Type any value in the box."
                    ));
                }
                if v != before {
                    out = Some(Value::Int(i128::from(v)));
                }
                let id = egui::Id::unique(("io-int", &p.name));
                let c = crate::commit::number(ui, id, &before.to_string(), "", boxed, TYPE_HINT);
                if let Some(n) = crate::commit::int(&c, lo, hi)
                    && n != before
                {
                    out = Some(Value::Int(i128::from(n)));
                }
            }
            Some(Widget::Real) => {
                anchor = Some(row_label(ui, &label, place));
                let mut v = match p.value {
                    Value::Real(r) => r,
                    _ => 0.0,
                };
                let before = v;
                if slider {
                    ui.add(
                        egui::Slider::new(&mut v, -100.0..=100.0)
                            .clamping(egui::SliderClamping::Edits)
                            .show_value(false),
                    )
                    .tip("Drag to set the input, -100 to 100. Type any value in the box.");
                }
                if v.to_bits() != before.to_bits() && v.is_finite() {
                    out = Some(Value::Real(v));
                }
                let id = egui::Id::unique(("io-real", &p.name));
                let c = crate::commit::number(ui, id, &before.to_string(), "", boxed, TYPE_HINT);
                if let Some(x) = crate::commit::real(&c)
                    && x.to_bits() != before.to_bits()
                {
                    out = Some(Value::Real(x));
                }
            }
            Some(Widget::Time) => {
                anchor = Some(row_label(ui, &label, place));
                let mut ms = match p.value {
                    Value::Int(ns) => i64::try_from(ns / 1_000_000).unwrap_or(0),
                    _ => 0,
                };
                let before = ms;
                if slider {
                    ui.add(
                        egui::Slider::new(&mut ms, 0..=10_000)
                            .clamping(egui::SliderClamping::Edits)
                            .show_value(false),
                    )
                    .tip("Drag to set the time, up to 10 s.");
                }
                ui.add(egui::DragValue::new(&mut ms).range(0..=86_400_000).suffix(" ms"))
                    .tip("The time in ms. Drag, or double-click to type it, up to a day.");
                if ms != before {
                    out = Some(Value::Int(i128::from(ms) * 1_000_000));
                }
            }
            None => anchor = Some(row_label(ui, &label, place)),
        }
    });
    let anchor = anchor.unwrap_or_else(|| row_label(ui, &label, place));
    (out, anchor)
}

/// An input's button on its card. A tile's is `size`, its name cut; a
/// row's is at least that and grows to a second line when the name needs
/// it, so the name shows whole.
fn control_button(ui: &mut egui::Ui, b: egui::Button<'_>, size: Vec2, tile: bool) -> egui::Response {
    if tile {
        ui.add_sized(size, b.wrap_mode(egui::TextWrapMode::Truncate))
    } else {
        let w = size.x;
        ui.allocate_ui_with_layout(
            Vec2::new(w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_width(w);
                ui.add(b.wrap_mode(egui::TextWrapMode::Wrap).min_size(size))
            },
        )
        .inner
    }
}

/// A row's name, where a right-click lands, saying where the point is.
fn row_label(ui: &mut egui::Ui, label: &str, place: &str) -> egui::Response {
    ui.add(egui::Label::new(label).sense(Sense::click()))
        .tip(place.to_string())
}

/// One output's first line on its card: a lamp (BOOL) or a readout.
fn output_ui(ui: &mut egui::Ui, p: &Point, tile: bool) {
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
    // On is the lamp's own colour when its name has one (red, yellow,
    // green...; `lamps`), else green, as a TRUE value in the diagram and
    // a lamp on the Monitor are. Its glyph takes the same colour.
    let color = if on {
        t.ink(crate::lamps::colour(&p.name).unwrap_or(GREEN))
    } else {
        t.fg
    };
    // A tile has the lamp and the name; the glyph would take half of it.
    let glyph = (!tile).then(|| crate::glyph_lib::for_point(&p.name, false, p.ty == Ty::Bool));
    crate::io_cards::value(ui, glyph, color, &text, !tile);
}

/// A value for a readout (TIME in ms).
#[must_use]
pub fn show(ty: Ty, v: Value) -> String {
    match (ty, v) {
        (Ty::Time, Value::Int(ns)) => format!("{} ms", ns / 1_000_000),
        _ => v.shown(),
    }
}
