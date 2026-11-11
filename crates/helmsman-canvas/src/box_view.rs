//! The Editor on a real box: the page was served by the box's API
//! (`--ui`), so it talks to the same origin. [`BoxState`] decides what to
//! watch and [`BoxProjects`] what to edit; in the browser `web::Driver`
//! and `web::ProjHttp` ask (fetch, `EventSource`) and hand back what came,
//! so this whole view also runs headless in tests, fed messages instead.
//!
//! On a box with projects (`--projects`) the Files sidebar lists them and
//! the ST editor edits the open project's files, for an engineer; anyone
//! else reads. The diagram is always what runs, live, and an engineer
//! forces an input by clicking its block. On a box without projects the
//! sidebar lists its modules and the ST is read-only, as before.
//!
//! The diagram's arrangement is the running project's shared default with
//! this browser's tweaks on top (`arrange.rs`); an engineer saves it as
//! the default.

use eframe::egui::{self, Color32, Rect, RichText, Sense, Vec2};
use helmsman_diagram::Scan;
use helmsman_iec::Value;
use serde_json::json;

use crate::arrange::{self, Layers};
use crate::box_projects::{Answer, BoxProjects, Deploy, PCall, PReq};
use crate::box_trend::BoxTrend;
use crate::editor::{self, Body, Chrome, Tab};
use crate::projects::{self, Act, FilesUi, Listing, Workspace};
use crate::remote::{BoxState, Cmd, Link, Msg, SignalRow};
use crate::review::Session;
use crate::sim_projects::Storage;
use crate::st_view::brand::{BLACK, WARN};
use crate::st_view::{Pick, Viewer, show};
use crate::theme::{Theme, theme};
use crate::tips::Tip;

const RED: Color32 = Color32::from_rgb(255, 110, 100);
/// Largest file the editor takes (the box's `project::MAX_FILE`).
const MAX_FILE: usize = 256 * 1024;

/// What [`BoxView::watch`] compares against.
#[derive(Default)]
struct BoxSeen {
    link: Option<u8>,
    deploy: Deploy,
    fault: Option<String>,
    notice: Option<String>,
}

pub struct BoxView {
    pub state: BoxState,
    /// The Messages tab's log.
    pub log: crate::messages::Log,
    /// What the log last saw, to report changes once.
    seen: BoxSeen,
    /// The box's projects, when it runs with them.
    pub projects: BoxProjects,
    pub files_ui: FilesUi,
    pub devices_ui: crate::devices::DevicesUi,
    /// Who the page is signed in as: the header's session, set by the app
    /// before every frame.
    pub session: Option<Session>,
    pub view: Option<Viewer>,
    /// The Agent's last change, marked in the code until the file is
    /// edited, and on the diagram once the box runs it
    /// ([`crate::agent_marks`]).
    pub agent_change: Option<crate::agent_marks::AgentChange>,
    /// The Trend tab: pinned signals from the box's history.
    pub trend: BoxTrend,
    /// The way to the agent the box runs (`POST /api/chat`).
    pub agent: crate::box_agent::BoxAgent,
    /// The Agent tab (`crate::instruct`, as in the page).
    pub instruct: crate::instruct::Instruct,
    /// The Agent tab's Help mode (`POST /api/help`).
    pub help: crate::help::HelpUi,
    help_seq: u64,
    #[cfg(target_arch = "wasm32")]
    help_http: crate::web::ChatHttp,
    /// Natively: the help requests asked for.
    #[cfg(not(target_arch = "wasm32"))]
    pub help_sent: Vec<crate::box_agent::Request>,
    /// The agent turn [`Self::settle_agent`] last took.
    answered_seq: u64,
    /// Page time of the last frame, s (for Messages).
    agent_at: f64,
    /// A proposal the Agent tab asked to open under Proposals; the app
    /// takes it.
    pub wants_review: Option<u64>,
    /// The box's proposals already decided (not drafts), by id, with
    /// their status: the app sets it from the Proposals list, so a link
    /// the Agent tab kept over a reload is not offered again.
    pub decided: std::collections::BTreeMap<u64, String>,
    /// Keeps what is in front of the person in this browser (`crate::desk`).
    desk: crate::desk::Keeper,
    /// Keeps it on the box too, for whoever is signed in.
    pub on_box: crate::desk::OnBox,
    /// The desk as it was before the page loaded, for the App to take
    /// the screen and the sign-in name from (once).
    pub restored: Option<crate::desk::Desk>,
    /// The screen and the sign-in name, as the App last set them.
    pub desk_screen: Option<String>,
    pub desk_user: String,
    /// Whose work the desk on this page is: the last person signed in.
    desk_owner: Option<String>,
    /// The diagram's shared default and this browser's tweaks.
    pub layers: Layers,
    /// The lowering the view was built from.
    generation: u64,
    /// Set when the operator, with the box out of reach, asks for the
    /// simulation in this page instead.
    pub wants_sim: bool,
    /// The diagram's pick last frame (a new one sends the ST there).
    last_pick: Option<Pick>,
    /// The scope (an instance prefix, `""` for the program) to open once
    /// the diagram is there: the Monitor's "Open diagram".
    pub open_at: Option<String>,
    #[cfg(target_arch = "wasm32")]
    driver: crate::web::Driver,
    #[cfg(target_arch = "wasm32")]
    http: crate::web::ProjHttp,
    #[cfg(target_arch = "wasm32")]
    chat: crate::web::ChatHttp,
    /// Natively: the chat requests asked for, and whether the status was.
    #[cfg(not(target_arch = "wasm32"))]
    pub chat_sent: Vec<crate::box_agent::Request>,
    /// Commands not carried out (no transport here: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<Cmd>,
    /// Project and force requests not carried out (tests carry them out).
    #[cfg(not(target_arch = "wasm32"))]
    pub psent: Vec<PReq>,
}

impl Default for BoxView {
    fn default() -> Self {
        Self::new()
    }
}

/// What the projects put back from `d`: its project, file and edits.
fn restore_of(d: &crate::desk::Desk) -> crate::box_projects::Restore {
    crate::box_projects::Restore {
        project: d.project.clone(),
        file: d.project.clone().zip(d.file.clone()),
        drafts: d.drafts.clone(),
    }
}

/// Whether `s` may edit, save, deploy and force: an engineer or above.
pub fn is_engineer(s: Option<&Session>) -> Result<(), String> {
    match s {
        None => Err("Sign in as an engineer (top right) to edit, save, deploy or force.".into()),
        Some(s) if matches!(s.role.as_str(), "engineer" | "integrator") => Ok(()),
        Some(s) => Err(format!(
            "Signed in as {}: only an engineer edits, saves, deploys or forces.",
            s.label()
        )),
    }
}

impl BoxView {
    #[must_use]
    pub fn new() -> Self {
        #[cfg(target_arch = "wasm32")]
        let tweaks: Box<dyn Storage> = Box::new(crate::web::LocalArrangements);
        #[cfg(target_arch = "wasm32")]
        let pins: Box<dyn Storage> = Box::new(crate::web::LocalSlot("helmsman-canvas/trend-pins"));
        #[cfg(not(target_arch = "wasm32"))]
        let tweaks: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        #[cfg(not(target_arch = "wasm32"))]
        let pins: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        #[cfg(target_arch = "wasm32")]
        let desk: Box<dyn Storage> = Box::new(crate::web::LocalSlot(crate::desk::SLOT));
        #[cfg(not(target_arch = "wasm32"))]
        let desk: Box<dyn Storage> = Box::new(crate::sim_projects::MemStorage::default());
        Self::with_stores(tweaks, pins).with_desk(desk)
    }

    /// Keeps the desk in `store`, and puts back what it holds: the
    /// project, the file, the unsaved edits (as the box's answers come
    /// in), and the Agent's input.
    #[must_use]
    pub fn with_desk(mut self, store: Box<dyn Storage>) -> Self {
        let (keeper, d) = crate::desk::Keeper::load(store);
        self.desk = keeper;
        self.desk_owner.clone_from(&d.owner);
        self.projects.restore = restore_of(&d);
        self.put_back(d, false);
        self
    }

    /// The inputs, the Agent's conversation, the device form, the screen
    /// and the sign-in name from `d`. A conversation under way here is
    /// left as it is. With `keep_here`, what this page holds (an input
    /// typed, a conversation, a half-filled form) stays and `d` only
    /// fills what is empty.
    fn put_back(&mut self, d: crate::desk::Desk, keep_here: bool) {
        if !keep_here || self.instruct.draft.is_empty() {
            self.instruct.draft.clone_from(&d.agent);
        }
        let has_conversation = !self.agent.turns.is_empty() || self.instruct.outcome.is_some();
        let keep_conversation = keep_here && has_conversation;
        if !self.instruct.busy() && !self.agent.busy() && !keep_conversation {
            let c = d.conversation.clone().unwrap_or_default();
            c.put_back(&mut self.agent, &mut self.instruct);
        }
        if !(keep_here && self.devices_ui.adding.is_some()) {
            self.devices_ui.adding.clone_from(&d.device);
        }
        self.desk_screen.clone_from(&d.screen);
        self.desk_user.clone_from(&d.user);
        self.restored = Some(d);
    }

    /// Puts the box's desk, newer than the one in front of the person, in
    /// its place (the person signed in on another screen since): the
    /// edits merged with this page's, which stay.
    fn adopt(&mut self, d: crate::desk::Desk) {
        let edited_here = self.desk.changed(&self.desk());
        self.desk.adopt(&d);
        self.projects.adopt(restore_of(&d));
        self.put_back(d, edited_here);
    }

    /// Another person signed in on this page: the last one's open project,
    /// edits, inputs, conversation and form are not theirs. They stay on
    /// the last one's desk (the box's, as of the last second), and this
    /// page starts empty and asks for the new person's.
    fn start_over(&mut self) {
        self.agent.stop();
        self.agent.turns.clear();
        self.instruct = crate::instruct::Instruct::default();
        self.devices_ui.adding = None;
        self.desk_screen = None;
        self.desk_user.clear();
        self.projects.start_over();
    }

    /// Follows who is signed in: the desk on this page belongs to the last
    /// person to sign in on it. A different person starts from their own
    /// desk in this browser (or an empty one), never the last one's; the
    /// first to sign in after work typed signed out takes that work.
    fn follow_owner(&mut self) {
        use crate::desk::Bound;
        let Some(who) = crate::desk::owner(self.session.as_ref()) else {
            return;
        };
        let was = self.desk_owner.replace(who.clone());
        if was.as_deref() == Some(who.as_str()) {
            return;
        }
        // The form's name is the person who just signed in.
        if let Some(u) = self.session.as_ref().and_then(|s| s.user.clone()) {
            self.desk_user = u;
        }
        match self.desk.bind(&who, &self.desk_user, was.is_none()) {
            Bound::Claimed => {}
            Bound::Theirs(d) => {
                let d = *d;
                self.start_over();
                self.projects.adopt(restore_of(&d));
                self.put_back(d, false);
            }
            Bound::Unsupported => {
                if was.is_some() {
                    self.start_over();
                    self.desk.adopt(&crate::desk::Desk::default());
                }
            }
        }
    }

    /// What the desk keeps, as it stands now.
    #[must_use]
    pub fn desk(&self) -> crate::desk::Desk {
        let mut d = crate::desk::Desk {
            agent: self.instruct.draft.clone(),
            screen: self.desk_screen.clone(),
            user: self.desk_user.clone(),
            owner: self.desk_owner.clone(),
            conversation: crate::desk::Conversation::of(
                &self.agent.turns,
                self.instruct.outcome.as_ref(),
                self.instruct.run.as_ref().map(crate::instruct::Run::target),
            ),
            device: self.devices_ui.adding.as_ref().map(crate::desk::form_of),
            ..crate::desk::Desk::default()
        };
        self.projects.to_desk(&mut d);
        d
    }

    /// The Editor, the Trend's pins kept in `pins` while signed out.
    #[cfg(test)]
    #[must_use]
    pub fn with_pins(pins: Box<dyn Storage>) -> Self {
        Self::with_stores(Box::new(crate::sim_projects::MemStorage::default()), pins)
    }

    /// The Editor, the diagram's personal layouts kept in `tweaks` and
    /// the Trend's pins (signed out) in `pins`.
    #[must_use]
    pub fn with_stores(tweaks: Box<dyn Storage>, pins: Box<dyn Storage>) -> Self {
        Self {
            trend: BoxTrend::new(pins),
            log: crate::messages::Log::default(),
            seen: BoxSeen::default(),
            state: BoxState::new(),
            projects: BoxProjects::new(),
            files_ui: FilesUi::default(),
            devices_ui: crate::devices::DevicesUi::default(),
            session: None,
            view: None,
            agent_change: None,
            layers: Layers::load(tweaks),
            generation: 0,
            wants_sim: false,
            last_pick: None,
            open_at: None,
            #[cfg(target_arch = "wasm32")]
            driver: crate::web::Driver::default(),
            #[cfg(target_arch = "wasm32")]
            http: crate::web::ProjHttp::default(),
            #[cfg(target_arch = "wasm32")]
            chat: crate::web::ChatHttp::default(),
            #[cfg(not(target_arch = "wasm32"))]
            chat_sent: Vec::new(),
            agent: crate::box_agent::BoxAgent::new(),
            instruct: crate::instruct::Instruct::default(),
            help: crate::help::HelpUi::default(),
            help_seq: 0,
            #[cfg(target_arch = "wasm32")]
            help_http: crate::web::ChatHttp::default(),
            #[cfg(not(target_arch = "wasm32"))]
            help_sent: Vec::new(),
            answered_seq: 0,
            agent_at: 0.0,
            wants_review: None,
            decided: std::collections::BTreeMap::new(),
            desk: crate::desk::Keeper::load(Box::new(crate::sim_projects::MemStorage::default())).0,
            on_box: crate::desk::OnBox::default(),
            restored: None,
            desk_screen: None,
            desk_user: String::new(),
            desk_owner: None,
            #[cfg(not(target_arch = "wasm32"))]
            sent: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            psent: Vec::new(),
        }
    }

    /// Takes in `msgs` at page time `now`, then carries out what is due.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn step(&mut self, ctx: &egui::Context, msgs: Vec<Msg>, now: f64) {
        self.step_all(ctx, msgs, Vec::new(), now);
    }

    /// [`Self::step`], with the project and force answers too.
    pub fn step_all(&mut self, ctx: &egui::Context, msgs: Vec<Msg>, answers: Vec<(PCall, Answer)>, now: f64) {
        // Who is signed in first, so an answer for the person before is
        // told from one for this person.
        self.follow_owner();
        self.on_box.step(self.session.as_ref());
        for m in msgs {
            self.state.apply(m, now);
        }
        for (call, a) in answers {
            match call {
                PCall::TrendPins | PCall::SavePins | PCall::Trend => self.trend.answer(&call, &a, now),
                PCall::Desk | PCall::SaveDesk => {
                    if let Some(d) = self.on_box.answer(&call, &a, self.desk.saved_at()) {
                        self.adopt(d);
                    }
                }
                PCall::Discover => self.devices_ui.discovered(a.status, &a.body),
                call => self.projects.answer(call, &a),
            }
        }
        if std::mem::take(&mut self.projects.deployed) {
            self.state.relist();
        }
        if let Some(j) = self.state.take_agent() {
            self.agent.status = crate::box_agent::Status::parse(&j);
        }
        if let Some(f) = self.projects.forces.take() {
            for s in &mut self.state.signals {
                s.forced = f.iter().any(|f| f.signal.as_deref() == Some(s.name.as_str()));
            }
            self.state.forces = f;
        }
        let cmds = self.state.poll(now);
        self.run(ctx, cmds);
        self.trend.step(self.session.as_ref(), &self.state, now);
        if self.desk.due(now) {
            let desk = self.desk();
            if let Some(d) = self.desk.keep(&desk, now) {
                self.on_box.keep(&d, now);
            }
        }
        let mut reqs = self.projects.poll();
        reqs.extend(self.trend.poll());
        reqs.extend(self.on_box.poll());
        self.send(ctx, reqs);
        self.downloads();
        let mut fresh = false;
        match &self.state.lowered {
            Some(l) if l.generation != self.generation => {
                self.generation = l.generation;
                let mut v = Viewer::new(&l.src, l.module.clone());
                // The same source again (refetched): keep what was being
                // looked at, as arranged. A new version is laid out from
                // the layers (the moves so far kept as tweaks first).
                // Opens with no path lit; the operator's pick survives a
                // new version while its variable does.
                if let Some(old) = self.view.take() {
                    self.layers.track(&old);
                    if old.src == v.src {
                        v = old;
                    } else {
                        v.because = old.because.filter(|b| v.knows(b));
                        fresh = true;
                    }
                } else {
                    fresh = true;
                }
                self.last_pick = v.pick;
                self.view = Some(v);
            }
            None => self.view = None,
            Some(_) => {}
        }
        let key = self.layout_key();
        if let Some(v) = &mut self.view {
            if let Some(text) = self.projects.layout_saved.take() {
                self.layers.adopt(v, &text);
            }
            self.layers.show(v, &key, self.projects.shared_layout(), fresh);
        }
        if let (Some(v), Some(p)) = (&mut self.view, &self.open_at) {
            if let Some(k) = v.scopes.iter().position(|s| s.prefix == *p) {
                v.open(k);
            }
            self.open_at = None;
        }
        if let Some(at) = self.state.next_retry() {
            // Every second while waiting, so the status line's countdown
            // ticks and the page is visibly alive (Nick, 2026-11-01).
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((at - now).clamp(0.05, 1.0)));
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn run(&mut self, ctx: &egui::Context, cmds: Vec<Cmd>) {
        for c in cmds {
            self.driver.run(c, ctx);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn run(&mut self, _ctx: &egui::Context, cmds: Vec<Cmd>) {
        self.sent.extend(cmds);
    }

    #[cfg(target_arch = "wasm32")]
    fn send(&mut self, ctx: &egui::Context, reqs: Vec<PReq>) {
        for r in reqs {
            self.http.send(r, ctx);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send(&mut self, _ctx: &egui::Context, reqs: Vec<PReq>) {
        self.psent.extend(reqs);
    }

    #[cfg(target_arch = "wasm32")]
    fn downloads(&mut self) {
        for url in std::mem::take(&mut self.projects.downloads) {
            let name = format!(
                "{}.zip",
                self.projects.open.as_ref().map_or("project", |o| o.name.as_str())
            );
            crate::web::download_url(&url, &name);
        }
    }

    /// Natively the links stay in `projects.downloads` for tests.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    const fn downloads(&mut self) {}

    #[cfg(target_arch = "wasm32")]
    fn inbox(&mut self) -> (Vec<Msg>, Vec<(PCall, Answer)>) {
        (self.driver.drain(), self.http.drain())
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn inbox(&mut self) -> (Vec<Msg>, Vec<(PCall, Answer)>) {
        (Vec::new(), Vec::new())
    }

    /// Closes the box's streams while the page simulates in the browser:
    /// nobody would take what they carry.
    pub fn park(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.state.pause();
        let cmds = self.state.poll(now);
        self.run(ctx, cmds);
    }

    /// Takes in what arrived and carries out what is due, with nothing
    /// drawn (the Monitor draws from the same state).
    pub fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let (msgs, answers) = self.inbox();
        self.step_all(ctx, msgs, answers, now);
        self.chat_inbox();
        self.agent_at = now;
        self.settle_agent(ctx);
        self.watch(now);
    }

    #[cfg(target_arch = "wasm32")]
    fn chat_inbox(&mut self) {
        for i in self.help_http.drain() {
            if let crate::web::ChatIn::Turn(inc) = i {
                self.help_answer(inc);
            }
        }
        for i in self.chat.drain() {
            match i {
                crate::web::ChatIn::Turn(inc) => {
                    let was = self.agent.busy();
                    self.agent.apply(inc);
                    // A turn that ended may have changed the agent's state.
                    if was && !self.agent.busy() {
                        self.agent.status = crate::box_agent::Status::Unread;
                    }
                }
                crate::web::ChatIn::Status(status, body) => {
                    self.agent.status_answer(status, &body, self.agent_at);
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    const fn chat_inbox(&mut self) {}

    /// The project as the agent sees it, in the scope of an instruction:
    /// the open file (else its first program file), every file as read,
    /// the program as it builds, and its compile errors.
    fn agent_files(&self) -> Option<(String, crate::scope::Scope, crate::scope::Files)> {
        let o = self.projects.open.as_ref().filter(|_| self.has_projects())?;
        let files: crate::scope::Files = o
            .bufs
            .iter()
            .filter(|(n, b)| b.loaded && !helmsman_project::agent::is_agent_file(n))
            .map(|(n, b)| (n.clone(), b.text.clone()))
            .collect();
        let is_program = |f: &str| {
            helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St)
                && !helmsman_iec::spec::is_test(f)
        };
        let file = o
            .current
            .clone()
            .filter(|f| is_program(f) && files.contains_key(f))
            .or_else(|| files.keys().find(|f| is_program(f)).cloned())?;
        Some((o.name.clone(), crate::scope::Scope::File(file), files))
    }

    /// Why an instruction cannot go to the box's agent now, if it cannot.
    fn agent_blocked(&self) -> Option<String> {
        if let Err(why) = self.writable() {
            return Some(why);
        }
        if !self.agent.status.can_send() {
            return Some(self.agent.status.line());
        }
        if self.agent_files().is_none() {
            return Some(
                "Open a program file first: the Agent changes the file open in the Code tab.".into(),
            );
        }
        None
    }

    /// Runs `f` on the project as the agent sees it, in `scope`.
    fn with_env<R>(
        &mut self,
        scope: Option<crate::scope::Scope>,
        f: impl FnOnce(&mut crate::instruct::Instruct, &crate::instruct::Env<'_>) -> R,
    ) -> Option<R> {
        let (project, open_scope, files) = self.agent_files()?;
        // Checked the way the box will run it: its image sizes and scan
        // period, as it reported them for this project (the page's
        // simulation sizes only until it has).
        let settings = self.projects.open.as_ref().and_then(|o| o.runs).map_or_else(
            || helmsman_iec::spec::Settings::new(crate::playground::SIZES),
            |r| helmsman_iec::spec::Settings::new(r.sizes).period(r.period),
        );
        let plan = helmsman_iec::spec::plan(
            files
                .iter()
                .filter(|(n, _)| helmsman_project::file_name(n) == Some(helmsman_project::FileKind::St))
                .map(|(n, t)| (n.as_str(), t.as_str())),
        );
        let diags: Vec<String> = helmsman_iec::compile(&plan.main, settings.sizes)
            .err()
            .map(|d| d.iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        // The box's agent reads the devices with its own tools.
        let devices = String::new();
        let env = crate::instruct::Env {
            project: &project,
            scope: scope.unwrap_or(open_scope),
            files: &files,
            source: &plan.main,
            diagnostics: &diags,
            devices: &devices,
            settings,
        };
        Some(f(&mut self.instruct, &env))
    }

    /// Sends the typed instruction to the box's agent: the answer format
    /// ([`crate::instruct::BOX_RULES`]) and then the same message the page
    /// sends, in a new conversation on the box.
    pub fn send_instruction(&mut self, ctx: &egui::Context) {
        if let Some(why) = self.agent_blocked() {
            self.instruct.failed(&why);
            return;
        }
        let pages: Vec<String> = self
            .projects
            .agent_pages()
            .into_iter()
            .map(str::to_string)
            .collect();
        let started = self.with_env(None, |i, env| {
            let file = crate::instruct::target(&env.scope);
            let earlier = crate::instruct::recent(pages.iter().map(String::as_str), &file);
            i.start(env, &earlier)
        });
        match started {
            Some(Ok(ask)) => {
                let message = format!("{}\n\n{}", crate::instruct::BOX_RULES, ask.message);
                self.agent_send(&message, true, ctx);
            }
            Some(Err(why)) => self.instruct.failed(&why),
            None => {}
        }
    }

    /// One message of the instruction under way to the box; `fresh` opens
    /// a new conversation there.
    fn agent_send(&mut self, message: &str, fresh: bool, ctx: &egui::Context) {
        match self.agent.send(message, fresh) {
            Ok(req) => {
                #[cfg(target_arch = "wasm32")]
                self.chat.send(req, ctx);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = ctx;
                    self.chat_sent.push(req);
                }
            }
            Err(why) => self.instruct.failed(&why),
        }
    }

    /// A turn of the box's agent that ended goes through
    /// [`crate::instruct::Instruct::answered`], as in the page: a retry
    /// continues the conversation on the box with what failed; a green
    /// answer is saved into the file; proposals the agent stored instead
    /// are offered for review.
    fn settle_agent(&mut self, ctx: &egui::Context) {
        use crate::box_agent::End;
        use crate::instruct::Step;
        if !self.instruct.busy() {
            return;
        }
        // An instruction belongs to the project it was typed in: another
        // open before the answer is in ends the run with nothing written,
        // so no retry is built from the other project's files.
        let open = self.projects.open.as_ref().map(|o| o.name.as_str());
        if self
            .instruct
            .run
            .as_ref()
            .is_some_and(|r| Some(r.project.as_str()) != open)
        {
            self.agent.stop();
            #[cfg(target_arch = "wasm32")]
            self.chat.abort();
            self.instruct
                .failed("the open project changed while the Agent was answering");
            return self.log_agent();
        }
        if self.agent.seq() == self.answered_seq {
            return;
        }
        let Some(turn) = self.agent.turns.last().filter(|t| t.end.is_some()).cloned() else {
            return;
        };
        self.answered_seq = self.agent.seq();
        let proposals = turn.proposals();
        match turn.end {
            Some(End::Done { stop, .. }) if stop == "end_turn" => {}
            Some(End::Done { why, stop }) => {
                // Any other stop ended the turn early: whatever text there
                // is may be half an answer, and half a file is never written.
                let said = match stop.as_str() {
                    "budget" | "max_tokens" => format!("the answer was cut off ({stop})"),
                    _ => format!("the agent stopped ({stop})"),
                };
                self.instruct.failed(&why.unwrap_or(said));
                self.instruct.proposed(&proposals);
                return;
            }
            Some(End::Failed(why)) => {
                self.instruct.failed(&why);
                self.instruct.proposed(&proposals);
                return;
            }
            Some(End::Stopped) | None => {
                self.instruct.failed("stopped");
                return;
            }
        }
        let scope = self.instruct.run.as_ref().map(|r| r.scope.clone());
        let step = self.with_env(scope, |i, env| i.answered(&turn.text, env));
        match step {
            Some(Step::Ask(ask)) => self.agent_send(&ask.text, false, ctx),
            Some(Step::Apply(apply)) => self.agent_write(&apply),
            Some(Step::Done) => {
                self.instruct.proposed(&proposals);
                self.log_agent();
            }
            None => self.instruct.failed("the project is no longer open"),
        }
    }

    /// The Agent's last change, marked while its file is what the Agent
    /// wrote: the editor's line marks for the file open, the diagram's
    /// baseline (the diagram is what runs, so its marks show once the
    /// change is deployed). The first edit clears them.
    fn agent_marks(&mut self, chrome: &mut Chrome) {
        let open = self.projects.open.as_ref();
        let text = self
            .agent_change
            .as_ref()
            .and_then(|c| open.and_then(|o| o.bufs.get(&c.file)))
            .map(|b| b.text.as_str());
        let project = open.map_or("", |o| o.name.as_str());
        if self
            .agent_change
            .as_ref()
            .is_some_and(|c| !c.holds(project, text))
        {
            self.agent_change = None;
        }
        let current = open.and_then(|o| o.current.as_deref());
        let c = self.agent_change.as_ref();
        chrome.st_marks = c
            .filter(|c| Some(c.file.as_str()) == current)
            .map(|c| c.lines().clone());
        if let Some(v) = &mut self.view {
            v.baseline = c.and_then(|c| c.baseline.clone());
        }
    }

    /// A checked answer saved into the box's project: the file with the
    /// change's History entry, then its scenarios as requirement tests.
    fn agent_write(&mut self, apply: &crate::instruct::Apply) {
        if self
            .projects
            .open
            .as_ref()
            .is_none_or(|o| o.name != apply.project)
        {
            let why = format!("{} is no longer open.", apply.project);
            self.instruct.not_written(&apply.file, &why);
            return self.log_agent();
        }
        let before = self.projects.history.last().map(|r| r.n);
        let was = self
            .projects
            .open
            .as_ref()
            .and_then(|o| o.bufs.get(&apply.file))
            .map(|b| b.text.clone())
            .unwrap_or_default();
        let baseline = self.view.as_ref().map(|v| std::rc::Rc::new(v.module.clone()));
        self.agent_change = Some(crate::agent_marks::AgentChange::new(
            &apply.project,
            &apply.file,
            &was,
            apply.text.clone(),
            baseline,
        ));
        self.projects
            .save_agent(&apply.file, &apply.text, &apply.record, &apply.tests);
        let undo = before.map(|revision| crate::instruct::Undo {
            project: apply.project.clone(),
            file: apply.file.clone(),
            revision,
            text: apply.text.clone(),
        });
        self.instruct.applied(apply, undo);
        self.log_agent();
    }

    /// The outcome of the last instruction, in Messages.
    fn log_agent(&mut self) {
        use crate::instruct::Kind;
        let Some(o) = &self.instruct.outcome else { return };
        let mut line = o.status.clone();
        for p in &o.problems {
            line.push(' ');
            line.push_str(p);
        }
        let at = self.agent_at;
        match o.kind {
            Kind::Applied | Kind::Undone | Kind::Said => self.log.info(at, line),
            Kind::Refused | Kind::Failed => self.log.warning(at, line),
        }
    }

    /// Why Undo is off now, if it is.
    fn undo_blocked(&self) -> Option<String> {
        let u = self.instruct.undo()?;
        if let Err(why) = is_engineer(self.session.as_ref()) {
            return Some(why);
        }
        let o = self.projects.open.as_ref().filter(|o| o.name == u.project)?;
        let current = o.bufs.get(&u.file).map(|b| b.text.as_str());
        self.instruct.undo_refusal(current, self.projects.dirty())
    }

    /// Undo: the box restores the revision before the Agent's change (as
    /// a new revision; the change stays in History).
    pub fn undo_change(&mut self) {
        let Some(u) = self.instruct.undo().cloned() else {
            return;
        };
        if self.undo_blocked().is_some() || self.projects.open.as_ref().is_none_or(|o| o.name != u.project) {
            return;
        }
        self.projects.restore(&u.project, u.revision);
        self.instruct.undone(&u);
        self.log_agent();
    }

    /// A piece of the box's answer to the help request under way.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn help_answer(&mut self, inc: crate::box_agent::Incoming) {
        let Some(h) = self.help.helping.as_mut() else {
            return;
        };
        let was = h.busy();
        h.apply(inc);
        if was && let crate::help::Phase::Answered { answer, issue, .. } = &h.phase {
            let line = match (answer, issue) {
                (Some(_), None) => "Help: answered from the help pages.".to_string(),
                (Some(_), Some(i)) => format!("Help: answered, and an issue was written ({}).", i.title),
                (None, Some(i)) => format!("Help: an issue was written ({}).", i.title),
                (None, None) => return,
            };
            self.log.info(self.agent_at, line);
        }
    }

    /// Why Help cannot be asked now, if it cannot: the box takes a request
    /// from someone signed in, whatever their role.
    fn help_blocked(&self) -> Option<String> {
        self.session
            .is_none()
            .then(|| "Sign in (top right) to ask for help.".to_string())
    }

    /// Help mode's Ask: the question and the page's Messages to the box.
    pub fn ask_help(&mut self, ctx: &egui::Context) {
        if self.help_blocked().is_some() || crate::help::refusal(&self.help.draft).is_some() {
            return;
        }
        self.help_seq += 1;
        let req = crate::help::request(&self.help.draft, &self.log.text(), self.help_seq);
        self.help.helping = Some(crate::help::Helping::new(self.help_seq));
        #[cfg(target_arch = "wasm32")]
        self.help_http.send(req, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.help_sent.push(req);
        }
    }

    /// The Agent tab: its heading (the agent's state as a dot, the mode's
    /// name, the mode toggle), then the shared tab (changing the program)
    /// or Help.
    fn agent_tab(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::help::Mode;
        use crate::instruct::Act;

        if self.agent.status_due(ui.ctx().input(|i| i.time)) {
            #[cfg(target_arch = "wasm32")]
            self.chat.status(ui.ctx());
        }
        let ctx = ui.ctx().clone();
        let view = crate::instruct::View {
            target: self
                .agent_files()
                .map_or_else(|| "the open file".into(), |(_, s, _)| crate::instruct::target(&s)),
            blocked: self.agent_blocked(),
            undo_blocked: self.undo_blocked(),
            decided: self.decided.clone(),
        };
        let help_view = crate::help::View {
            blocked: self.help_blocked(),
        };
        let (act, help_act) = egui::ScrollArea::vertical()
            .id_salt("box-agent")
            .show(ui, |ui| {
                // Room for the scroll bar (a resizable panel grows to any
                // row that overflows it).
                ui.set_max_width(ui.available_width() - 12.0);
                crate::help::heading(
                    ui,
                    chrome,
                    &mut self.help.mode,
                    self.agent.status.dot(),
                    &self.agent.status.line(),
                );
                ui.add_space(4.0);
                match self.help.mode {
                    Mode::Code => (crate::instruct::ui(ui, chrome, &mut self.instruct, &view), None),
                    Mode::Help => (None, crate::help::ui(ui, chrome, &mut self.help, &help_view)),
                }
            })
            .inner;
        match help_act {
            Some(crate::help::Act::Ask) => self.ask_help(&ctx),
            Some(crate::help::Act::Stop) => {
                #[cfg(target_arch = "wasm32")]
                self.help_http.abort();
                if let Some(h) = self.help.helping.as_mut() {
                    h.stop();
                }
            }
            None => {}
        }
        match act {
            Some(Act::Send) => self.send_instruction(&ctx),
            Some(Act::Stop) => {
                self.agent.stop();
                #[cfg(target_arch = "wasm32")]
                self.chat.abort();
                self.instruct.failed("stopped");
            }
            Some(Act::Undo) => self.undo_change(),
            Some(Act::Review(id)) => self.wants_review = Some(id),
            None => {}
        }
    }

    /// Reports to the log what changed: the link, a deploy's outcome, a
    /// fault, a notice.
    fn watch(&mut self, now: f64) {
        let link = match self.state.link() {
            Link::Connecting => 0,
            Link::Connected => 1,
            Link::Reconnecting { .. } => 2,
            Link::Unreachable { .. } => 3,
            Link::SignIn => 4,
        };
        if Some(link) != self.seen.link {
            match link {
                1 => self.log.info(now, "Connected to the box."),
                2 => self.log.warning(now, "Lost the box: reconnecting."),
                3 => self.log.error(now, "The box is unreachable."),
                4 => self
                    .log
                    .info(now, "This box needs a sign in before it shows anything."),
                _ => {}
            }
            self.seen.link = Some(link);
        }
        if self.projects.deploy != self.seen.deploy {
            match &self.projects.deploy {
                Deploy::Done(what) => self.log.info(now, format!("Deployed: {what}")),
                Deploy::Failed(why) => self.log.error(now, format!("Deploy failed: {why}")),
                _ => {}
            }
            self.seen.deploy = self.projects.deploy.clone();
        }
        let fault = self.state.scan.as_ref().and_then(|s| s.fault.clone());
        if fault != self.seen.fault {
            if let Some(f) = &fault {
                self.log.error(now, format!("Fault: {f}"));
            }
            self.seen.fault = fault;
        }
        let notice = self.projects.notice.as_ref().map(|(n, _)| n.clone());
        if notice != self.seen.notice {
            if let Some((n, error)) = &self.projects.notice {
                if *error {
                    self.log.error(now, n.clone());
                } else {
                    self.log.info(now, n.clone());
                }
            }
            self.seen.notice = notice;
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
        if self.devices_ui.renaming.is_some() {
            let files = self.loaded_files();
            self.devices_ui.plan_rename(&files);
        }
        if let Some(crate::devices::DeviceAct::Delete(name)) =
            self.devices_ui.delete_ui(&ui.ctx().clone(), chrome)
            && let Some(b) = self
                .projects
                .open
                .as_mut()
                .and_then(|o| o.bufs.get_mut(helmsman_project::IO_MAP))
                .filter(|b| b.loaded)
        {
            b.text = crate::device_form::remove_device(&b.text, &name);
        }
        if let Some(crate::devices::DeviceAct::Rename(plan)) =
            self.devices_ui.rename_ui(&ui.ctx().clone(), chrome)
            && let Some(o) = &mut self.projects.open
        {
            for (f, text, _) in plan.files {
                if let Some(b) = o.bufs.get_mut(&f).filter(|b| b.loaded) {
                    b.text = text;
                }
            }
        }
        if let Some(line) = self.devices_ui.plant.form_ui(&ui.ctx().clone(), chrome) {
            self.add_device_line(&line);
        }
        self.revision_ui(&ui.ctx().clone(), chrome);
        if let Some(a) = self.files_ui.naming_ui(&ui.ctx().clone(), chrome) {
            self.project_act(a);
        }
        if let Some(a) = self.files_ui.confirm_ui(&ui.ctx().clone(), chrome) {
            self.project_act(a);
        }
    }

    /// Whose personal layout the diagram shows: the running project's, or
    /// on a box without one the modules'.
    #[must_use]
    pub fn layout_key(&self) -> String {
        match &self.projects.running {
            Some(p) => format!("box:{p}"),
            None => "box-modules".into(),
        }
    }

    /// Whether this page may save the arrangement as the shared default.
    fn can_save_layout(&self) -> Result<(), String> {
        is_engineer(self.session.as_ref())?;
        if !self.has_projects() {
            return Err("This box runs without projects (helmsman-plc --projects): there is no project to keep a default layout in.".into());
        }
        if self.projects.running.is_none() {
            return Err("Deploy a project first: its default layout is kept with it.".into());
        }
        if self.projects.layout.is_none() {
            return Err("Reading the project's default layout…".into());
        }
        Ok(())
    }

    /// Saves the diagram as it stands as the running project's default.
    pub fn save_layout(&mut self) {
        if self.can_save_layout().is_err() {
            return;
        }
        if let Some(v) = &self.view {
            self.projects.save_layout(v.arrangement().to_text());
        }
    }

    /// The box runs with projects.
    /// Import device…: a vendor's EDS joins the open project as
    /// `NAME.eds` (a new file on the box, or the draft of one that exists)
    /// and the New device form opens filled in from it.
    fn import_eds(&mut self, file: &str, bytes: &[u8]) {
        let Some(o) = &mut self.projects.open else {
            self.projects
                .note(format!("Open a project to import {file} into it."), true);
            return;
        };
        let text = match helmsman_project::check_text(bytes) {
            Ok(t) => t.to_string(),
            Err(e) => {
                self.projects.note(format!("{file} was refused: {e}"), true);
                return;
            }
        };
        let Some(target) = helmsman_project::eds_file_name(file) else {
            self.projects.note(
                format!("{file}: nothing of the name survives as a file name (a-z 0-9 _ -)"),
                true,
            );
            return;
        };
        match o.bufs.get_mut(&target) {
            Some(b) if b.loaded => b.text.clone_from(&text),
            Some(_) => {}
            None => self.projects.new_file_with(&target, &text),
        }
        self.devices_ui.adding = Some(crate::devices::NewDevice::from_eds(&target, &text));
    }

    /// The Devices tab's Add: one `device` line onto the open project's
    /// `io.map`, made if the project has none (a draft; Save writes it).
    /// The open project's files that are loaded, name and text.
    fn loaded_files(&self) -> Vec<(String, String)> {
        self.projects
            .open
            .as_ref()
            .map(|o| {
                o.bufs
                    .iter()
                    .filter(|(_, b)| b.loaded)
                    .map(|(n, b)| (n.clone(), b.text.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The form's Add: new variables into the program file (refused, with
    /// nothing changed, if it would not read after), then the device and
    /// its signals into io.map.
    fn apply_wiring(&mut self, w: &crate::device_form::Wiring) {
        match self.wire_in(w) {
            Ok(()) => self.projects.note(w.summary(), false),
            Err(e) => self.projects.note(e, true),
        }
    }

    fn wire_in(&mut self, w: &crate::device_form::Wiring) -> Result<(), String> {
        let files = self.loaded_files();
        let Some(o) = &mut self.projects.open else {
            return Err("Open a project first.".into());
        };
        if !w.declare.is_empty() {
            let Some(file) =
                crate::device_form::program_file(files.iter().map(|(n, t)| (n.as_str(), t.as_str())))
            else {
                return Err("No PROGRAM in this project to add the new variables to.".into());
            };
            let Some(b) = o.bufs.get_mut(&file) else {
                return Err(format!("{file} is not loaded yet."));
            };
            let mut src = b.text.clone();
            for (dir, name, ty) in &w.declare {
                match crate::device_form::declare_var(&src, *dir, name, *ty) {
                    Some(s) => src = s,
                    None => return Err(format!("{file} has no PROGRAM to add {name} to.")),
                }
            }
            if helmsman_iec::ast::parse(&b.text).is_ok() && helmsman_iec::ast::parse(&src).is_err() {
                return Err("A new variable's name is an ST keyword: rename it and Add again.".into());
            }
            b.text = src;
        }
        match o.bufs.get_mut(helmsman_project::IO_MAP) {
            Some(b) if b.loaded => b.text = crate::device_form::apply_map(&b.text, w),
            Some(_) => {}
            None => self
                .projects
                .new_file_with(helmsman_project::IO_MAP, &crate::device_form::apply_map("", w)),
        }
        Ok(())
    }

    fn add_device_line(&mut self, line: &str) {
        let Some(o) = &mut self.projects.open else { return };
        match o.bufs.get_mut(helmsman_project::IO_MAP) {
            Some(b) if b.loaded => b.text = crate::devices::appended(&b.text, line),
            Some(_) => {}
            None => self.projects.new_file_with(helmsman_project::IO_MAP, line),
        }
    }

    #[must_use]
    pub fn has_projects(&self) -> bool {
        self.projects.available == Some(true)
    }

    /// What the header names: the open project, else the module open.
    #[must_use]
    pub fn title(&self) -> String {
        if let Some(o) = self.projects.open.as_ref().filter(|_| self.has_projects()) {
            return o.name.clone();
        }
        self.state.selected.clone().unwrap_or_else(|| "This box".into())
    }

    /// What the empty canvas says.
    #[must_use]
    pub fn waiting(&self) -> String {
        let s = &self.state;
        if let Some(p) = &s.problem {
            return p.clone();
        }
        match (&s.selected, &s.scan) {
            _ if s.link() == Link::SignIn => {
                "Sign in to watch this box: Sign in in the header, or Alt F8.".into()
            }
            _ if !matches!(s.link(), Link::Connected) && s.modules.is_empty() => {
                "Looking for the box…".into()
            }
            (None, _) if s.modules.is_empty() && self.has_projects() => {
                "Nothing runs yet: open a project and Deploy it.".into()
            }
            (None, _) if s.modules.is_empty() => "No modules on this box.".into(),
            (None, _) => "Pick a module.".into(),
            (Some(m), None) => format!("Waiting for a traced scan of {m}…"),
            (Some(m), Some(_)) => format!("Loading {m}'s source…"),
        }
    }

    /// The connection status line.
    #[must_use]
    pub fn status(&self, now: f64, t: &Theme) -> (String, Color32) {
        let retry = |at: f64| (at - now).max(0.0).ceil();
        match self.state.link() {
            Link::Connecting => ("● connecting…".into(), t.muted),
            Link::Connected => ("● connected".into(), t.accent),
            Link::SignIn => ("● sign in to watch".into(), t.fg),
            Link::Reconnecting { at, .. } => (format!("● reconnecting (retry in {}s)", retry(at)), t.fg),
            Link::Unreachable { at, .. } => (format!("● box unreachable (retry in {}s)", retry(at)), RED),
        }
    }

    /// The input signal a diagram read of `path` is, if the box forces it.
    fn input(&self, path: &str) -> Option<&SignalRow> {
        self.state
            .signals
            .iter()
            .find(|s| s.dir == "in" && s.name.eq_ignore_ascii_case(path))
    }

    /// Whether `name` is forced, as the box last said.
    #[must_use]
    pub fn forced(&self, name: &str) -> bool {
        self.state
            .forces
            .iter()
            .any(|f| f.signal.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(name)))
            || self
                .state
                .signals
                .iter()
                .any(|s| s.forced && s.name.eq_ignore_ascii_case(name))
    }

    /// Forces input `name` to `v` (an engineer's click on its block).
    pub fn force(&mut self, name: &str, v: Value) {
        if is_engineer(self.session.as_ref()).is_err() {
            return;
        }
        let value = match v {
            Value::Bool(b) => json!(b),
            Value::Int(n) => i64::try_from(n).map_or_else(|_| json!(n.to_string()), |n| json!(n)),
            Value::Real(r) => json!(r),
        };
        self.projects.force(name, &value);
    }

    /// Opens `file` of the open project at `line`.
    fn goto(&mut self, file: &str, line: u32, chrome: &mut Chrome) {
        self.projects.open_file(file);
        if self.projects.current().is_some() {
            chrome.show_code();
            chrome.goto(line);
        }
    }

    /// The forced inputs, each with release, and release all.
    fn banner(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        if self.state.forces.is_empty() {
            return;
        }
        let can = is_engineer(self.session.as_ref());
        let mut release = None;
        // The glyph strip floats over the diagram's top right, 8 pt in: the
        // banner wraps short of it, or the strip would cover its buttons
        // and take their clicks. Its width is last frame's (its place lags
        // a frame when a panel opens; its width does not).
        let strip = ui.ctx().memory(|m| m.area_rect(egui::Id::unique("glyphs")));
        egui::Frame::new()
            .fill(WARN)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                if let Some(s) = strip {
                    let left = ui.max_rect().right() - 8.0 - s.width() - 16.0;
                    ui.set_max_width((left - ui.min_rect().left()).max(160.0));
                }
                ui.horizontal_wrapped(|ui| {
                    let r = ui.label(RichText::new("FORCED").strong().color(BLACK)).tip(
                        "These inputs hold the value an engineer forced, whatever the module reads. \
                         A force ends on release, logout or deploy.",
                    );
                    chrome.hit("forces", r.rect);
                    for f in &self.state.forces {
                        ui.label(
                            RichText::new(format!("{} = {} ({})", f.point(), show(f.value), f.by))
                                .monospace()
                                .color(BLACK),
                        )
                        .tip("The input, the value it is forced to, and who forced it.");
                        let b = egui::Button::new("release");
                        let r = match &can {
                            Ok(()) => ui
                                .add(b)
                                .tip("Releases this force: the input reads its module again."),
                            Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
                        };
                        chrome.hit(format!("release {}", f.id), r.rect);
                        if r.clicked() {
                            release = Some(Some(f.id));
                        }
                    }
                    let b = egui::Button::new(RichText::new("Release all").strong());
                    let r = match &can {
                        Ok(()) => ui.add(b).tip("Releases every force on the box."),
                        Err(why) => ui.add_enabled(false, b).on_disabled_hover_text(why),
                    };
                    chrome.hit("release all", r.rect);
                    if r.clicked() {
                        release = Some(None);
                    }
                });
            });
        if let Some(id) = release {
            self.projects.release(id);
        }
    }

    /// Over each input's block: FORCED in the warning colour when forced,
    /// and for an engineer a control that forces it.
    fn input_controls(
        &mut self,
        ui: &mut egui::Ui,
        area: Rect,
        reads: &[(String, Rect)],
        chrome: &mut Chrome,
    ) {
        let engineer = is_engineer(self.session.as_ref()).is_ok();
        let mut wants = Vec::new();
        for (k, (path, rect)) in reads.iter().enumerate() {
            if !area.intersects(*rect) {
                continue;
            }
            let Some(sig) = self.input(path).cloned() else {
                continue;
            };
            let forced = self.forced(&sig.name);
            if engineer {
                let builder = egui::UiBuilder::new().max_rect(*rect).id_salt(("box-force", k));
                let r = ui
                    .scope_builder(builder, |ui| force_ui(ui, *rect, &sig, forced))
                    .inner;
                chrome.hit(format!("force {}", sig.name), *rect);
                if let Some(v) = r {
                    wants.push((sig.name.clone(), v));
                }
            }
            if forced {
                let tag = Rect::from_min_size(
                    egui::pos2(rect.max.x - 58.0, rect.min.y - 16.0),
                    Vec2::new(58.0, 15.0),
                );
                ui.painter().rect_filled(tag, 2.0, WARN);
                ui.painter().text(
                    tag.center(),
                    egui::Align2::CENTER_CENTER,
                    "FORCED",
                    egui::FontId::monospace(crate::theme::MIN_TEXT),
                    BLACK,
                );
                chrome.hit(format!("forced {}", sig.name), tag);
            }
        }
        for (name, v) in wants {
            self.force(&name, v);
        }
    }
}

/// An input's force control over its block: a BOOL flips, a number drags.
/// The value to force it to, if one was asked for.
fn force_ui(ui: &mut egui::Ui, rect: Rect, s: &SignalRow, forced: bool) -> Option<Value> {
    let t = theme(ui);
    let id = egui::Id::unique(("box-force-number", &s.name));
    let label = s.name.to_ascii_lowercase();
    let size = (rect.height() * 0.35).clamp(7.0, 16.0);
    let tip = "force this input on the box (an engineer only; ends on logout or deploy)";
    match s.value {
        Some(Value::Bool(on)) => {
            let fill = if forced { WARN } else { t.rule };
            let ink = if forced { BLACK } else { t.fg };
            let b = egui::Button::new(
                RichText::new(format!("{label} {}", if on { "TRUE" } else { "FALSE" }))
                    .strong()
                    .size(size)
                    .color(ink),
            )
            .fill(fill)
            // Never wrapped: a narrow block at low zoom would stack the
            // letters one a row.
            .wrap_mode(egui::TextWrapMode::Truncate)
            .sense(Sense::click());
            ui.add_sized(rect.size(), b)
                .on_hover_text(tip)
                .clicked()
                .then_some(Value::Bool(!on))
        }
        Some(Value::Int(n)) => {
            let v = i64::try_from(n).unwrap_or(0);
            let c = crate::commit::number(ui, id, &v.to_string(), "", rect.size(), tip);
            crate::commit::int(&c, i64::MIN, i64::MAX)
                .filter(|x| *x != v)
                .map(|x| Value::Int(i128::from(x)))
        }
        Some(Value::Real(r)) => {
            let c = crate::commit::number(ui, id, &r.to_string(), "", rect.size(), tip);
            crate::commit::real(&c)
                .filter(|x| x.to_bits() != r.to_bits())
                .map(Value::Real)
        }
        _ => None,
    }
}

impl BoxView {
    /// What the Files tab asked for, done through the project API.
    fn project_act(&mut self, a: Act) {
        let open = self.projects.open.clone();
        match a {
            Act::OpenProject(p) => self.projects.open_project(&p),
            Act::OpenFile(f) => self.projects.open_file(&f),
            Act::NewProject(p) => self.projects.create(&p, None),
            Act::CopyProject { from, to } => self.projects.create(&to, Some(&from)),
            Act::FromStarter { from, to } => self.projects.create_from(&to, &from),
            Act::RenameProject { from, to } => self.projects.rename(&from, &to),
            Act::DeleteProject(p) => self.projects.delete(&p),
            Act::NewFile(f) => self.projects.new_file(&f),
            Act::RenameFile { from, to } => self.projects.rename_file(&from, &to),
            Act::DeleteFile(f) => self.projects.delete_file(&f),
            Act::CopyFile(f) => {
                if let Some(b) = open.as_ref().and_then(|o| o.bufs.get(&f)).filter(|b| b.loaded) {
                    self.files_ui.clipboard = Some((f, b.text.clone()));
                }
            }
            Act::DuplicateFile(f) => {
                if let Some(o) = &open
                    && let Some(b) = o.bufs.get(&f).filter(|b| b.loaded)
                {
                    let name = projects::free_file_name(&f, |n| o.bufs.contains_key(n));
                    self.projects.new_file_with(&name, &b.text);
                }
            }
            Act::PasteFile { name, text } => {
                if let Some(o) = &open {
                    let name = projects::free_file_name(&name, |n| o.bufs.contains_key(n));
                    self.projects.new_file_with(&name, &text);
                }
            }
            // The Agent's answer lands in the simulation's projects.
            Act::UseAnswer { .. } => {}
            Act::Deploy => self.deploy(),
            Act::ShowRevision(n) => {
                if let Some(o) = &open {
                    self.projects.read_revision(&o.name, n);
                }
            }
        }
    }

    /// The revision on view, in the middle of the screen, until Close or
    /// Restore (an engineer's; the box records the restore).
    fn revision_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) {
        let Some(view) = self.projects.revision.clone() else {
            return;
        };
        let current: std::collections::BTreeMap<String, String> = self
            .projects
            .open
            .as_ref()
            .map(|o| {
                o.bufs
                    .iter()
                    .filter(|(n, b)| b.loaded && !helmsman_project::agent::is_agent_file(n))
                    .map(|(n, b)| (n.clone(), b.text.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let write = self.writable();
        match crate::history_ui::modal(ctx, chrome, &view, &current, &write) {
            Some(crate::history_ui::HistoryAct::Close) => self.projects.revision = None,
            Some(crate::history_ui::HistoryAct::Restore(n)) => {
                if let Some(name) = self.projects.open.as_ref().map(|o| o.name.clone()) {
                    self.projects.restore(&name, n);
                }
            }
            None => {}
        }
    }
}

impl Workspace for BoxView {
    fn dirty(&self) -> bool {
        self.projects.dirty()
    }

    fn new_project(&mut self, chrome: &mut Chrome) {
        if self.has_projects() {
            self.files_ui.new_project();
            chrome.show_files();
        }
    }

    fn copy_project(&mut self, chrome: &mut Chrome) {
        if let Some(o) = &self.projects.open {
            let name = o.name.clone();
            self.files_ui.copy_project(&name);
            chrome.show_files();
        }
    }

    fn layout_menu(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let can = self.can_save_layout();
        if arrange::menu_items(ui, chrome, &mut self.layers, self.view.as_mut(), can) {
            self.save_layout();
        }
    }

    fn zoom_to_fit(&mut self) {
        if let Some(v) = &mut self.view {
            v.refit();
        }
    }

    fn go_to_definition(&mut self, chrome: &mut Chrome) -> bool {
        let Some(v) = &self.view else { return false };
        let Some(line) = v.definition_line() else {
            return false;
        };
        // As a block click: the file and line of the project build when the
        // project open is the one running; the build's line without projects.
        match projects::file_line(&v.src, line) {
            Some((file, l))
                if self.has_projects()
                    && self.projects.open.as_ref().map(|o| &o.name) == self.projects.running.as_ref() =>
            {
                self.goto(&file, l, chrome);
            }
            None if !self.has_projects() => {
                chrome.show_code();
                chrome.goto(line);
            }
            _ => return false,
        }
        true
    }

    fn cross_references(&mut self) -> bool {
        self.view.as_mut().is_some_and(Viewer::cross_references)
    }

    fn force_picked(&mut self, ctx: &egui::Context) -> bool {
        if is_engineer(self.session.as_ref()).is_err() {
            return false;
        }
        let Some((_, path)) = self.view.as_ref().and_then(Viewer::picked_read) else {
            return false;
        };
        let Some(sig) = self.input(&path).cloned() else {
            return false;
        };
        match sig.value {
            Some(Value::Bool(on)) => self.force(&sig.name, Value::Bool(!on)),
            Some(Value::Int(_) | Value::Real(_)) => {
                ctx.memory_mut(|m| m.request_focus(egui::Id::unique(("box-force-number", &sig.name))));
            }
            None => return false,
        }
        true
    }

    fn step_into(&mut self) -> bool {
        self.view.as_mut().is_some_and(Viewer::step_into)
    }

    fn step_out(&mut self) -> bool {
        self.view.as_mut().is_some_and(Viewer::step_out)
    }

    fn release_all(&mut self) -> bool {
        if is_engineer(self.session.as_ref()).is_err() || self.state.forces.is_empty() {
            return false;
        }
        self.projects.release(None);
        true
    }

    fn undo(&mut self, ctx: &egui::Context, redo: bool) -> bool {
        if self.writable().is_err() {
            return false;
        }
        let Some((name, file)) = self
            .projects
            .open
            .as_ref()
            .and_then(|o| o.current.clone().map(|f| (o.name.clone(), f)))
        else {
            return false;
        };
        let id = editor::source_id("box-st", &name, &file);
        let Some(buf) = self.projects.current_mut() else {
            return false;
        };
        editor::undo(ctx, id, &mut buf.text, redo)
    }

    fn writable(&self) -> Result<(), String> {
        is_engineer(self.session.as_ref())?;
        match self.projects.available {
            Some(true) if self.projects.open.is_some() => Ok(()),
            Some(true) => Err("Open a project first.".into()),
            _ => Err("This box runs without projects (helmsman-plc --projects): nothing to edit.".into()),
        }
    }

    fn save(&mut self) {
        if self.writable().is_ok() {
            self.projects.save();
        }
    }

    fn deploy(&mut self) {
        if self.writable().is_ok() {
            self.projects.deploy();
        }
    }

    fn busy(&self) -> bool {
        self.projects.deploy.busy()
            || self
                .projects
                .open
                .as_ref()
                .is_some_and(|o| o.bufs.values().any(|b| b.saving.is_some()))
    }

    fn upload(&mut self, file: &str, bytes: Vec<u8>) {
        if is_engineer(self.session.as_ref()).is_ok() && self.has_projects() {
            if crate::devices::is_eds(file) {
                self.import_eds(file, &bytes);
            } else {
                self.projects.upload(file, bytes, false);
            }
        }
    }

    fn download(&mut self) {
        self.projects.download();
    }
}

impl Body for BoxView {
    fn toolbar(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        let now = ui.ctx().input(|i| i.time);
        let (text, color) = self.status(now, &t);
        ui.label(RichText::new(text).monospace().color(color))
            .tip("The page's live connection to the box.");
        if matches!(self.state.link(), Link::Unreachable { .. }) {
            let r = ui
                .button("Simulate in this browser")
                .tip("Runs the program in this page instead. No box needed.");
            chrome.hit("simulate", r.rect);
            if r.clicked() {
                self.wants_sim = true;
            }
        }
        if let Some(s) = &self.state.scan {
            ui.separator();
            ui.label(
                RichText::new(format!("t={:.1}s", s.now_ms as f64 / 1000.0))
                    .monospace()
                    .color(t.muted),
            )
            .tip(format!("The box's clock since start, at scan {}.", s.scan));
        }
    }

    fn has_notices(&self) -> bool {
        self.state.scan.as_ref().is_some_and(|s| s.fault.is_some())
            || self.projects.deploy != Deploy::Idle
            || self.projects.notice.is_some()
            || self.desk.problem.is_some()
            || self.on_box.problem.is_some()
    }

    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if let Some(f) = self.state.scan.as_ref().and_then(|s| s.fault.as_ref()) {
            crate::messages::copyable(ui, chrome, "fault", &format!("fault: {f}"), RED);
        }
        match &self.projects.deploy {
            Deploy::Idle => {}
            Deploy::Saving => {
                ui.label(RichText::new("saving…").color(t.muted));
            }
            Deploy::Checking => {
                ui.label(RichText::new("checking…").color(t.muted));
            }
            Deploy::Replaying => {
                ui.label(RichText::new("replaying in the sim…").color(t.muted));
            }
            Deploy::Deploying => {
                ui.label(RichText::new("deploying…").color(t.muted));
            }
            Deploy::Done(what) => {
                let r = ui.label(RichText::new(format!("✔ {what}")).color(t.accent));
                chrome.hit("deployed", r.rect);
            }
            Deploy::Failed(why) => {
                let why = format!("✖ {why}");
                crate::messages::copyable(ui, chrome, "deploy failed", &why, RED);
            }
        }
        if let Some(p) = &self.desk.problem {
            let r = ui.label(RichText::new(format!("⚠ {p}")).color(t.muted));
            chrome.hit("desk problem", r.rect);
        }
        if let Some(p) = &self.on_box.problem {
            let r = ui.label(RichText::new(format!("⚠ {p}")).color(t.muted));
            chrome.hit("desk on the box problem", r.rect);
        }
        if let Some((text, error)) = self.projects.notice.clone() {
            ui.separator();
            if error {
                crate::messages::copyable(ui, chrome, "notice", &text, RED);
            } else {
                let r = ui.label(RichText::new(text).color(t.muted));
                chrome.hit("notice", r.rect);
            }
            if self.projects.replace_offered() && error {
                let r = ui
                    .button("Replace")
                    .tip("Saves your version over the one on the box.");
                chrome.hit("replace", r.rect);
                if r.clicked() {
                    self.projects.replace();
                }
            }
            if let Some(f) = self.projects.conflict.clone() {
                let r = ui
                    .button(format!("Reload {f}"))
                    .tip("Drops your edits to this file and loads the box's version.");
                chrome.hit("reload", r.rect);
                if r.clicked() {
                    self.projects.reload(&f);
                    self.projects.clear_notice();
                }
            }
        }
    }

    /// The box's projects and the open one's files; under them the
    /// modules that run, each a program, the open one with every block
    /// instance it calls, nested; a click opens any of them.
    fn files(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if self.has_projects() {
            let write = self.writable().or_else(|e| {
                // Opening is a read; only the writes need an engineer.
                if self.projects.open.is_none() && is_engineer(self.session.as_ref()).is_ok() {
                    Ok(())
                } else {
                    Err(e)
                }
            });
            let open = self.projects.open.clone();
            let files: Vec<(String, bool)> = open
                .as_ref()
                .map(|o| {
                    o.bufs
                        .iter()
                        // The Agent's pages are the Agent tab's, not a file to open.
                        .filter(|(n, _)| !helmsman_project::agent::is_agent_file(n))
                        .map(|(n, b)| (n.clone(), b.dirty()))
                        .collect()
                })
                .unwrap_or_default();
            let listing = Listing {
                projects: &self.projects.projects,
                open: open.as_ref().map(|o| o.name.as_str()),
                files,
                current: open.as_ref().and_then(|o| o.current.as_deref()),
                write,
                dirty: self.projects.dirty(),
                starters: true,
                running: open
                    .as_ref()
                    .is_some_and(|o| self.projects.running.as_deref() == Some(o.name.as_str())),
                history: &self.projects.history,
                deploy: self.writable().and_then(|()| {
                    if self.busy() {
                        Err("A save or deploy is under way.".into())
                    } else {
                        Ok(())
                    }
                }),
            };
            if let Some(a) = self.files_ui.show(ui, &listing, chrome) {
                self.project_act(a);
            }
            ui.separator();
            ui.label(RichText::new("Running").strong().color(t.muted))
                .tip("The modules the box runs now, each a program. Click one to see it live.");
        }
        let mut chosen = None;
        let modules = self.state.modules.clone();
        if modules.is_empty() {
            ui.label(RichText::new(self.waiting()).color(t.muted));
        }
        for m in &modules {
            let selected = self.state.selected.as_deref() == Some(m.name.as_str());
            let arrow = if selected { "▾" } else { "▸" };
            let text = RichText::new(format!("{arrow} {}  {} · {} scans", m.name, m.state, m.scans))
                .monospace()
                .color(if m.state == "faulted" { RED } else { t.fg });
            let r = ui.selectable_label(selected, text);
            chrome.hit(format!("module {}", m.name), r.rect);
            let r = match &m.fault {
                Some(f) => r.tip(format!("Faulted: {f}")),
                None => r.tip("A module: its state and scans so far. Click to open its diagram."),
            };
            if r.clicked() {
                chosen = Some(m.name.clone());
            }
            if selected && let Some(v) = &mut self.view {
                ui.indent(("box-tree", &m.name), |ui| v.tree_ui(ui));
            }
        }
        if let Some(name) = chosen {
            self.state.select(&name);
        }
        // The blank space under it all: a right-click offers a new project
        // (and a paste into the open one).
        if self.has_projects() {
            let write = self.writable();
            let can_paste = self.projects.open.is_some();
            if let Some(a) = self.files_ui.blank_menu(ui, chrome, &write, can_paste) {
                self.project_act(a);
            }
        }
    }

    fn devices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::devices::{LinkState, Live, tree};
        // The open project's own map when it has one (what Add edits),
        // else the map the box runs.
        let open = self.projects.open.as_ref();
        let draft = open
            .and_then(|o| o.bufs.get(helmsman_project::IO_MAP))
            .filter(|b| b.loaded)
            .map(|b| b.text.clone());
        let (text, source) = match (draft, &self.state.io_map) {
            (Some(t), _) => (t, format!("io.map of {}", open.map_or("", |o| o.name.as_str()))),
            (None, Some(t)) if self.state.io_map_stale() => (
                t.clone(),
                "the map this box runs, as last read (the box did not answer the newest read)".to_string(),
            ),
            (None, Some(t)) => (t.clone(), "the map this box runs".to_string()),
            (None, None) => (String::new(), String::new()),
        };
        let map = helmsman_project::iomap::skim(&text);
        let signals = &self.state.signals;
        let live = |name: &str| {
            signals
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(name))
                .map(|s| Live {
                    value: s.value,
                    quality: Some(s.quality.clone()),
                    forced: s.forced,
                })
        };
        let devices = &self.state.devices;
        // Link states the newest read failed to renew are not shown as
        // current: they read as unknown, as a stale value greys out.
        let devices_stale = self.state.devices_stale();
        let state = |name: &str| {
            devices
                .iter()
                .find(|d| d.name == name)
                .filter(|_| !devices_stale)
                .map_or(LinkState::Unknown, |d| match d.state.as_str() {
                    "up" => LinkState::Up(d.address.clone()),
                    "down" => LinkState::Down(d.error.clone().unwrap_or_default()),
                    _ => LinkState::Unknown,
                })
        };
        let nodes = tree(&map, &live, &state);
        let write = if self.has_projects() {
            self.writable()
        } else {
            Err("This box runs without projects: its map is a file on the box.".into())
        };
        // The open project's plant model, read as text: no plant model
        // runs on a box, so no value; the measuring signals are live.
        let files: Vec<(String, String)> = open
            .map(|o| {
                o.bufs
                    .iter()
                    .filter(|(_, b)| b.loaded)
                    .map(|(n, b)| (n.clone(), b.text.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let refs: Vec<(&str, &str)> = files.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
        let signal_value = |name: &str| live(name).and_then(|l| l.value);
        self.devices_ui.plant.states = crate::plant::states(&refs, &map, &|_, _| None, &signal_value);
        self.devices_ui.plant.devices = crate::plant::device_choices(&map);
        self.devices_ui.plant.simulated = false;
        self.devices_ui.copy_first = false;
        self.devices_ui.adapter = self
            .state
            .adapter
            .as_ref()
            .and_then(|a| a.listening())
            .map(str::to_string);
        let program = crate::device_form::program_file(refs.iter().copied())
            .and_then(|f| files.iter().find(|(n, _)| *n == f).map(|(_, t)| t.clone()))
            .unwrap_or_default();
        self.devices_ui.program = crate::device_form::ports_of(&program);
        self.devices_ui.map = map.clone();
        self.devices_ui.eds_files = files
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| crate::devices::is_eds(n))
            .collect();
        match self.devices_ui.show(ui, &nodes, &source, chrome, &write) {
            Some(crate::devices::DeviceAct::Scan) => {
                self.devices_ui.scan = crate::devices::Scan::Running;
                self.projects.discover();
            }
            Some(crate::devices::DeviceAct::Use(d)) => self.devices_ui.open(d),
            Some(crate::devices::DeviceAct::OpenMap) => {
                self.projects.open_file(helmsman_project::IO_MAP);
                chrome.show_code();
            }
            Some(
                crate::devices::DeviceAct::Add(_)
                | crate::devices::DeviceAct::Rename(_)
                | crate::devices::DeviceAct::Delete(_),
            )
            | None => {}
        }
    }

    fn st(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if self.has_projects() {
            let write = self.writable();
            let Some(o) = self.projects.open.clone() else {
                ui.label(RichText::new("Open a project in Files.").color(t.muted));
                return;
            };
            let deploy = write.clone().and_then(|()| {
                if self.busy() {
                    Err("A save or deploy is under way.".into())
                } else {
                    Ok(())
                }
            });
            let mut go = false;
            // The header is the open file's name (Nick, 2026-11-08), then
            // word wrap, then the → that deploys this version.
            let name = o.current.clone().unwrap_or_else(|| "Structured Text".to_string());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                editor::file_header(
                    ui,
                    chrome,
                    &name,
                    "The open file of the project. Save keeps it on the box; Deploy runs it.",
                );
                editor::wrap_toggle(ui, chrome);
                go = projects::deploy_arrow(
                    ui,
                    chrome,
                    "code deploy",
                    &deploy,
                    "Deploys this version: saves, checks and runs the project on the box (Ctrl Shift Enter, File › Deploy).",
                );
                match &write {
                    Ok(()) => {}
                    Err(why) => {
                        let r = ui
                            .add(crate::theme::whole(RichText::new("read-only").color(t.muted)))
                            .tip(why.clone());
                        chrome.hit("read-only", r.rect);
                    }
                }
            });
            if go {
                self.deploy();
            }
            let Some(file) = o.current.clone() else {
                ui.label(RichText::new("No files in this project yet.").color(t.muted));
                return;
            };
            let diags: Vec<(u32, u32)> = self
                .projects
                .diags
                .iter()
                .filter(|d| d.file == file)
                .map(|d| (d.line, d.col))
                .collect();
            let list_h = if self.projects.diags.is_empty() {
                0.0
            } else {
                110.0
            };
            let size = Vec2::new(ui.available_width(), (ui.available_height() - list_h).max(120.0));
            let id = editor::source_id("box-st", &o.name, &file);
            let Some(buf) = self.projects.current_mut() else {
                return;
            };
            if !buf.loaded {
                ui.spinner();
                return;
            }
            ui.allocate_ui(size, |ui| {
                if write.is_ok() {
                    editor::st_editor(ui, id, &mut buf.text, &diags, MAX_FILE, chrome);
                } else {
                    let mut text: &str = &buf.text;
                    editor::st_editor(ui, id, &mut text, &diags, usize::MAX, chrome);
                }
            });
            if !self.projects.diags.is_empty() {
                ui.separator();
                let diags = self.projects.diags.clone();
                let mut go = None;
                egui::ScrollArea::vertical()
                    .id_salt("box-diags")
                    .max_height(list_h)
                    .show(ui, |ui| go = projects::diags_ui(ui, &diags, chrome));
                if let Some(d) = go {
                    self.goto(&d.file, d.line, chrome);
                }
            }
            return;
        }
        let name = self
            .state
            .selected
            .clone()
            .unwrap_or_else(|| "Structured Text".to_string());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            editor::file_header(ui, chrome, &name, "The source of the module that runs.");
            editor::wrap_toggle(ui, chrome);
            ui.add(crate::theme::whole(RichText::new("read-only").color(t.muted)))
                .tip("Changes reach the box as proposals.");
        });
        match &self.view {
            Some(v) => {
                let mut src: &str = &v.src;
                editor::st_editor(ui, egui::Id::unique("box-st"), &mut src, &[], usize::MAX, chrome);
            }
            None => {
                ui.label(RichText::new(self.waiting()).color(t.muted));
            }
        }
    }

    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        self.trend.ui(ui, chrome, &self.state, self.session.as_ref());
    }

    fn messages(&mut self) -> &mut crate::messages::Log {
        &mut self.log
    }

    fn tabs(&self) -> &'static [Tab] {
        &[Tab::Io, Tab::Claude]
    }

    fn tab(&mut self, ui: &mut egui::Ui, tab: Tab, chrome: &mut Chrome) {
        if tab == Tab::Claude {
            self.agent_tab(ui, chrome);
            return;
        }
        let t = theme(ui);
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        egui::ScrollArea::vertical().id_salt("box-io").show(ui, |ui| {
            if let Some(v) = &mut self.view {
                v.outputs_ui(ui, scan);
                ui.separator();
            }
            ui.label(RichText::new("Signals").strong());
            // Two columns (Nick, 2026-11-01: denser).
            egui::Grid::new("box-io-signals").num_columns(2).show(ui, |ui| {
                for pair in self.state.signals.chunks(2) {
                    for s in pair {
                        let unit = s.unit.as_deref().map_or(String::new(), |u| format!(" {u}"));
                        let forced = if s.forced { "  FORCED" } else { "" };
                        let text = format!("{} {} = {}{unit}{forced}", s.dir, s.name, show(s.value));
                        let color = match s.quality.as_str() {
                            _ if s.forced => WARN,
                            "missing" => RED,
                            _ if s.value == Some(helmsman_iec::Value::Bool(true)) => t.accent,
                            _ => t.fg,
                        };
                        let l = ui.label(RichText::new(text).monospace().color(color));
                        if let Some(e) = &s.point_error {
                            l.on_hover_text(e);
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        self.banner(ui, chrome);
        let scan = self.state.scan.as_ref().map(|s| s as &dyn Scan);
        let mut jump = None;
        let mut reads = Vec::new();
        let mut area = Rect::NOTHING;
        let icons = self
            .projects
            .open
            .as_ref()
            .and_then(|o| o.bufs.get(crate::icons::FILE))
            .filter(|b| b.loaded)
            .map(|b| b.text.clone());
        match &mut self.view {
            Some(v) if v.drawable() => {
                v.icons.update(icons.as_deref());
                let outer = vec![
                    "This box".to_string(),
                    self.state.selected.clone().unwrap_or_default(),
                ];
                if v.crumbs_ui(ui, &outer).is_some() {
                    v.open(0);
                }
                area = ui.available_rect_before_wrap();
                v.canvas(ui, scan);
                if v.pick != self.last_pick {
                    self.last_pick = v.pick;
                    if let Some((line, _)) = v.picked() {
                        jump = Some((projects::file_line(&v.src, line), line));
                    }
                }
                reads = v.reads_on_screen();
            }
            _ => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(self.waiting()).color(t.muted)));
            }
        }
        if let Some((at, line)) = jump {
            match at {
                // A project build: the file and line, when the project
                // open is the one running.
                Some((file, l))
                    if self.has_projects()
                        && self.projects.open.as_ref().map(|o| &o.name) == self.projects.running.as_ref() =>
                {
                    self.goto(&file, l, chrome);
                }
                None if !self.has_projects() => chrome.goto(line),
                _ => {}
            }
        }
        if !reads.is_empty() {
            self.input_controls(ui, area, &reads, chrome);
        }
    }
}
