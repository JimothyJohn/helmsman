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
use crate::st_view::brand::{BLACK, GREEN, WARN};
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
    warned: Option<String>,
    flash: Option<f64>,
}

// Each flag is its own fact (the node's token, watching a node, the
// Agent's change waiting, ...).
#[allow(clippy::struct_excessive_bools)]
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
    /// The design step on this box (`crate::design_step`, `POST /api/design`).
    pub design: crate::design_step::Designer,
    #[cfg(target_arch = "wasm32")]
    design_http: crate::web::ChatHttp,
    /// Natively: the design requests asked for.
    #[cfg(not(target_arch = "wasm32"))]
    pub design_sent: Vec<crate::box_agent::Request>,
    /// The batch of the kept design the Agent is building: the project
    /// and how many steps it holds.
    design_building: Option<(String, usize)>,
    /// Steps of each project's design built in this page.
    design_built: std::collections::BTreeMap<String, usize>,
    design_cache: crate::design_step::KeptCache,
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
    /// The pane layouts the box keeps (`crate::pane_layout`).
    pub pane_layouts: crate::pane_layout::OnBox,
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
    /// The points whose details are open in the I/O tab, lower case.
    io_open: std::collections::BTreeSet<String>,
    /// The diagram's right-click menu: the chat and what io.map can take
    /// (`crate::assign`); the code is changed in the editor.
    pub dmenu: crate::diagram_menu::Menu,
    /// The question "what value?" open on a literal of the diagram.
    pub value_ask: Option<crate::by_hand::ValueAsk>,
    /// The program's ports as last read, by the text they came from: the
    /// diagram and the I/O tab ask every frame.
    ports_memo: std::cell::RefCell<Option<(String, Vec<crate::device_form::Port>)>>,
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
    /// This browser holds the watched node's token (the sign-in panel's).
    pub node_token: bool,
    /// The page watches a node by address (`?box=`): Deploy sends there.
    pub watches_node: bool,
    /// The Agent's last answer was written and has not been deployed: the
    /// Agent tab's Deploy pulses.
    pub(crate) agent_pending: bool,
    /// A project New project made, until it is deployed or another opens:
    /// the diagram (what runs, another program) is not shown for it.
    fresh: Option<String>,
    /// The revision Back and Forward (the Agent tab, the Code tab) last
    /// showed; `None`: the newest.
    history_at: Option<u32>,
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
    /// The map the Devices and I/O tabs show, and where it is from: the
    /// open project's own when it has one (what Add edits), else the map
    /// the box runs.
    pub(crate) fn map_text(&self) -> (String, String) {
        let open = self.projects.open.as_ref();
        let draft = open
            .and_then(|o| o.bufs.get(helmsman_project::IO_MAP))
            .filter(|b| b.loaded)
            .map(|b| b.text.clone());
        match (draft, &self.state.io_map) {
            (Some(t), _) => (t, format!("io.map of {}", open.map_or("", |o| o.name.as_str()))),
            (None, Some(t)) if self.state.io_map_stale() => (
                t.clone(),
                "the map this box runs, as last read (the box did not answer the newest read)".to_string(),
            ),
            (None, Some(t)) => (t.clone(), "the map this box runs".to_string()),
            (None, None) => (String::new(), String::new()),
        }
    }

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

    /// A project New project made (Nick, 2026-11-25: "The agent history
    /// and panel should reset when a new file is created, the diagram
    /// should also be cleared and the state reset"): the Agent's turns,
    /// history and draft, the last deploy's outcome and the Devices notes
    /// go, and the diagram waits for this project to be deployed.
    fn start_afresh(&mut self, name: String) {
        self.agent.stop();
        self.agent.turns.clear();
        self.instruct = crate::instruct::Instruct::default();
        self.agent_pending = false;
        self.projects.deploy = Deploy::Idle;
        self.devices_ui.add_note = None;
        self.devices_ui.ask_note = None;
        self.history_at = None;
        self.fresh = Some(name);
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
        // The list asked for before anyone signed in was refused (a box
        // with accounts locks its reads). A page whose live view is the
        // box's learns it again when the view unlocks; one watching a
        // Skipper by address never does, so it asks here.
        if self.projects.available.is_none() {
            self.projects.relist();
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
            io_open: std::collections::BTreeSet::new(),
            dmenu: crate::diagram_menu::Menu::default(),
            value_ask: None,
            ports_memo: std::cell::RefCell::new(None),
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
            design: crate::design_step::Designer::boxed(),
            #[cfg(target_arch = "wasm32")]
            design_http: crate::web::ChatHttp::default(),
            #[cfg(not(target_arch = "wasm32"))]
            design_sent: Vec::new(),
            design_building: None,
            design_built: std::collections::BTreeMap::new(),
            design_cache: crate::design_step::KeptCache::default(),
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
            pane_layouts: crate::pane_layout::OnBox::default(),
            restored: None,
            desk_screen: None,
            desk_user: String::new(),
            desk_owner: None,
            #[cfg(not(target_arch = "wasm32"))]
            sent: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            psent: Vec::new(),
            node_token: false,
            watches_node: false,
            agent_pending: false,
            fresh: None,
            history_at: None,
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
        self.trend.local_pins = self.watches_node;
        self.projects.node_map = self.state.io_map.clone().filter(|_| self.state.io_map_node);
        if let Some(name) = self.projects.created.take() {
            self.start_afresh(name);
        }
        let open = self.projects.open.as_ref().map(|o| o.name.as_str());
        if self.fresh.as_deref() != open || matches!(self.projects.deploy, Deploy::Done(_)) {
            self.fresh = None;
        }
        // Scan and add passes over the Skipper's own devices.
        self.devices_ui.skipper = self
            .projects
            .node_map
            .as_deref()
            .filter(|_| self.watches_node)
            .map(crate::skipper_map::devices)
            .unwrap_or_default();
        self.projects.tick(now);
        self.on_box.step(self.session.as_ref());
        for m in msgs {
            self.state.apply(m, now);
        }
        let signed_in = self.session.is_some();
        for (call, a) in answers {
            match call {
                PCall::TrendPins | PCall::SavePins | PCall::Trend => self.trend.answer(&call, &a, now),
                PCall::Desk | PCall::SaveDesk => {
                    if let Some(d) = self.on_box.answer(&call, &a, self.desk.saved_at()) {
                        self.adopt(d);
                    }
                }
                call @ (PCall::PaneLayouts | PCall::KeepPaneLayout(_) | PCall::DropPaneLayout(_)) => {
                    self.pane_layouts.answer(&call, &a);
                }
                PCall::Discover => {
                    self.devices_ui.discovered(a.status, &a.body);
                    let found = crate::devices::hosts(&self.devices_ui.found);
                    self.projects.scanned(found);
                }
                PCall::Ask(address) => self.devices_ui.asked(&address, a.status, &a.body),
                PCall::Library => self.devices_ui.library_answer(a.status, &a.body),
                PCall::Wired(wired) => self.devices_ui.wired_answer(&wired, a.status, &a.body),
                PCall::Wiring(device) => self.devices_ui.wiring_answer(&device, a.status, &a.body),
                PCall::FindWired => {
                    if let Some(p) = &mut self.devices_ui.picking {
                        p.answer(a.status, &a.body);
                    }
                }
                PCall::Config(device, op) => self.devices_ui.config.answer(&device, op, a.status, &a.body),
                call @ (PCall::Proposal
                | PCall::ProposalAccept
                | PCall::ProposalDiscard
                | PCall::ProposalPublish) => {
                    self.devices_ui.proposal.answer(&call, a.status, &a.body);
                }
                PCall::VendorFile => self.devices_ui.vendor.answer(a.status, &a.body),
                PCall::NodeMap(op) => self.devices_ui.node_map.answer(op, a.status, &a.body),
                PCall::DesignProject(name) => self.design_project_answer(&name, &a),
                call => self.projects.answer_for(call, &a, signed_in),
            }
        }
        if let Some(project) = self.devices_ui.proposal.take_saved() {
            self.projects.reread(&project);
        }
        if std::mem::take(&mut self.projects.deployed) {
            self.state.relist();
            self.state.reread_source();
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
        reqs.extend(self.pane_layouts.poll());
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

    /// While the page simulates: takes in the answer to the second look
    /// for the box ([`crate::remote::BoxState::probe_at`]) and sends it
    /// when due. Nothing else of the box moves.
    pub fn probe_tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let (msgs, _) = self.inbox();
        for m in msgs {
            self.state.apply(m, now);
        }
        let cmds = self.state.poll(now);
        self.run(ctx, cmds);
    }

    /// Takes in what arrived and carries out what is due, with nothing
    /// drawn (the Monitor draws from the same state).
    pub fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let (msgs, answers) = self.inbox();
        self.step_all(ctx, msgs, answers, now);
        self.chat_inbox(ctx);
        self.agent_at = now;
        self.settle_agent(ctx);
        self.design_settle(ctx);
        self.watch(now);
    }

    #[cfg(target_arch = "wasm32")]
    fn chat_inbox(&mut self, ctx: &egui::Context) {
        for i in self.help_http.drain() {
            if let crate::web::ChatIn::Turn(inc) = i {
                self.help_answer(inc);
            }
        }
        for i in self.design_http.drain() {
            if let crate::web::ChatIn::Turn(inc) = i {
                self.design_feed(inc.into(), ctx);
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
                    // The box no longer knows this page's session (a
                    // restart): the header signs out, as for any box call.
                    if status == 401 && self.session.is_some() {
                        self.projects.ended = true;
                    }
                    self.agent.status_answer(status, &body, self.agent_at);
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    const fn chat_inbox(&mut self, _ctx: &egui::Context) {}

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
                "Open a program file first: the change goes into the file open in the Code tab.".into(),
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
        // The box's agent reads the devices with its own tools. A
        // Skipper's I/O list is not among them, and its program's ports
        // bind to that list by name: it goes with the request, and an
        // answer off it goes back to the Agent (Nick, 2026-11-24).
        // Until the page has read the Skipper's list there is none to
        // check by (an empty one would turn every port away).
        let node = self.projects.node_map.as_deref().filter(|_| self.watches_node);
        let devices = node.map_or_else(String::new, |n| {
            let made = self
                .projects
                .for_skipper()
                .unwrap_or_else(|| crate::skipper_map::for_skipper(None, n, &|_| None, &|_| None));
            // The same point under the project's other name: the program
            // must use the Skipper's (Nick, 2026-11-24: the Agent kept a
            // 40-character name for the MDrive's `enabled`).
            let same: Vec<String> = made
                .same
                .iter()
                .map(|(p, sp)| format!("\n- {}.", crate::skipper_map::same_words(p, sp)))
                .collect();
            format!(
                "The Skipper's I/O list. The program's VAR_INPUTs and VAR_OUTPUTs bind to its `in` and `out` points by name: use these names for what they are. A VAR_INPUT or VAR_OUTPUT with no point here is fine: the page adds a placeholder (`sim`) point for it, set from the I/O tab, so do not write io.map lines for it. A Skipper takes names of at most {} characters: keep every VAR_INPUT and VAR_OUTPUT name that short.{}\n{}",
                crate::skipper_map::NAME_MAX,
                same.concat(),
                made.map.trim()
            )
        });
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
            let earlier = crate::instruct::history(pages.iter().map(String::as_str), &file);
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
        if let Err(why) = self.agent_send_with(message, &[], fresh, ctx) {
            self.instruct.failed(&why);
        }
    }

    /// One message to the box's agent with `dropins` attached; the turn's
    /// number, or why it could not be sent.
    fn agent_send_with(
        &mut self,
        message: &str,
        dropins: &[u64],
        fresh: bool,
        ctx: &egui::Context,
    ) -> Result<u64, String> {
        let req = self.agent.send_with(message, dropins, fresh)?;
        let seq = req.seq;
        #[cfg(target_arch = "wasm32")]
        self.chat.send(req, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.chat_sent.push(req);
        }
        Ok(seq)
    }

    /// The vendor's file for the agent (`device_vendor.rs`): the picker,
    /// the upload, then the request for a profile carrying the drop-in's id
    /// once the box has kept the file; the agent's turn reported beside the
    /// button; a saved profile remembered for its device.
    fn vendor_file(&mut self, ctx: &egui::Context) {
        if self.devices_ui.vendor.take_pick() {
            #[cfg(target_arch = "wasm32")]
            crate::web::pick_vendor_file(ctx);
        }
        #[cfg(target_arch = "wasm32")]
        {
            for e in crate::web::take_vendor_errors() {
                self.devices_ui.vendor.pick_failed(&e);
            }
            for (name, bytes) in crate::web::take_vendor_picked() {
                self.devices_ui.vendor.picked(&name, bytes);
            }
        }
        for r in self.devices_ui.vendor.take_requests() {
            self.projects.enqueue(r);
            ctx.request_repaint();
        }
        if let Some(ask) = self.devices_ui.vendor.take_ask() {
            let sent = self.agent_send_with(&ask.message, &ask.dropins, true, ctx);
            self.devices_ui.vendor.asked(sent);
        }
        let turn = self.agent.turns.last().map(|t| (t.end.clone(), t.text.clone()));
        if let Some((end, text)) = turn {
            self.devices_ui
                .vendor
                .agent_turn(self.agent.seq(), end.as_ref(), &text);
        }
        if let Some(file) = self.devices_ui.proposal.saved_file().map(str::to_string) {
            self.devices_ui.vendor.saved(&file);
        }
        // The profiles saved for found devices, as the project holds them.
        let open = self.projects.open.as_ref();
        self.devices_ui.mine = self
            .devices_ui
            .vendor
            .built
            .iter()
            .filter_map(|(addr, file)| {
                let b = open?.bufs.get(file).filter(|b| b.loaded)?;
                Some((addr.clone(), (file.clone(), b.text.clone())))
            })
            .collect();
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
                .failed("the open project changed before the answer came");
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
                    _ => format!("the answer stopped ({stop})"),
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
            // A box never asks for a plan (`steps` is the page's).
            Some(Step::Done | Step::Wait) => {
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
        // On a Skipper a port no point backs gets its placeholder now, so
        // the answer runs as soon as it is deployed (Nick, 2026-11-24).
        if self.watches_node {
            let placed = self.projects.add_placeholders();
            self.projects.save_file(helmsman_project::IO_MAP);
            match placed {
                Ok(Some(words)) => self.projects.note(words, false),
                Err(crate::box_projects::Unsent::Refused(why)) => self.projects.note(why, true),
                // Nothing to say, or an address Deploy will scan for.
                Ok(None) | Err(crate::box_projects::Unsent::MayScan(_)) => {}
            }
        }
        // What the change added to the program joins the trend once the
        // box reports it (a change is saved first and deployed after).
        let known = crate::device_form::ports_of(&was);
        let added: Vec<String> = crate::device_form::ports_of(&apply.text)
            .into_iter()
            .map(|p| p.name)
            .filter(|n| !known.iter().any(|k| k.name == *n))
            .collect();
        self.trend.expect(&added);
        let undo = before.map(|revision| crate::instruct::Undo {
            project: apply.project.clone(),
            file: apply.file.clone(),
            revision,
            text: apply.text.clone(),
        });
        self.instruct.applied(apply, undo);
        self.log_agent();
    }

    /// Stop: the answer under way is dropped and nothing is written.
    pub fn stop_run(&mut self) {
        self.agent.stop();
        #[cfg(target_arch = "wasm32")]
        self.chat.abort();
        self.instruct.failed("stopped");
    }

    // ---- the design step (`crate::design_step`) ----

    /// Why the design entry is off, if it is: the reasons the Agent tab's
    /// other controls give, and a project to keep the design in.
    pub fn design_blocked(&self) -> Option<String> {
        if let Err(why) = self.writable() {
            return Some(match why.as_str() {
                "Open a project first." => {
                    "Open a project first: the design is kept in it as design.md, and built there.".into()
                }
                _ => why,
            });
        }
        if !self.agent.status.can_send() {
            return Some(self.agent.status.line());
        }
        if self.instruct.busy() {
            return Some("Still working on the last instruction.".into());
        }
        self.design
            .busy()
            .then(|| "A design is being written.".to_string())
    }

    /// One design request to the box.
    fn design_send_req(&mut self, req: &crate::design_step::Req, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        self.design_http.send(req.for_box(), ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.design_sent.push(req.for_box());
        }
    }

    /// Asks the box for the design of the machine in the description
    /// window: it adds its dialect, its modules, its devices and its
    /// requirement tests itself.
    pub fn send_design(&mut self, ctx: &egui::Context) {
        if let Some(why) = self.design_blocked() {
            self.design.error = Some(why);
            return;
        }
        let now = ctx.input(|i| i.time);
        let description = self.design.description.clone();
        match self.design.start_box(&description, now) {
            Ok(req) => {
                self.design.open = false;
                self.design_send_req(&req, ctx);
            }
            Err(why) => self.design.error = Some(why),
        }
    }

    /// What the box's design stream brought; the next turn goes out at once
    /// while the design is cut at the output limit.
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub fn design_feed(&mut self, feed: crate::design_step::Feed, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if let Some(req) = self.design.receive(feed, now) {
            self.design_send_req(&req, ctx);
        }
    }

    /// Stop, or Discard of a cut design: nothing is kept.
    pub fn stop_design(&mut self) {
        self.design.stop();
        #[cfg(target_arch = "wasm32")]
        self.design_http.abort();
    }

    /// Keep: the box takes a kept file in a whole project only (the file
    /// route writes the files a project uses), and `documents.md` goes
    /// with it. The design is kept in a copy of the open project, as
    /// the box holds it, under a free name: one atomic request, nothing
    /// written over, and the engineer's project as it was.
    pub fn keep_design(&mut self) {
        let Some(read) = self.design.read.clone() else {
            return;
        };
        let why = self
            .writable()
            .err()
            .or_else(|| {
                self.instruct
                    .busy()
                    .then(|| "Still working on the last instruction.".into())
            })
            .or_else(|| {
                self.projects.dirty().then(|| {
                    "Save your edits first: the design is kept in a copy of the project as the box holds it."
                        .into()
                })
            })
            .or_else(|| read.keep_refusal());
        if let Some(why) = why {
            self.design.notice = Some(why);
            return;
        }
        let taken: Vec<String> = self.projects.projects.iter().map(|p| p.name.clone()).collect();
        let name =
            crate::design_step::free_name(&crate::design_step::project_slug(&read.description), &taken);
        let mut files = self
            .projects
            .saved_files((crate::design_step::DESIGN_FILE, &read.text));
        if let Some(text) = read.documents_file() {
            files.insert(crate::documents::DOCUMENTS_FILE.to_string(), text);
        }
        let zip = helmsman_project::Files(files).to_zip();
        match zip {
            Ok(zip) => {
                self.design.notice = None;
                self.projects.design_project(&name, zip);
            }
            Err(e) => self.design.notice = Some(format!("The project could not be packed: {e}")),
        }
    }

    /// The box's answer to the project the kept design makes.
    fn design_project_answer(&mut self, name: &str, a: &Answer) {
        if a.status != 201 {
            let why = serde_json::from_str::<serde_json::Value>(&a.body)
                .ok()
                .and_then(|j| j["error"].as_str().map(str::to_string))
                .unwrap_or_default();
            let why = crate::device_proposal::clean(&why, 200, false);
            self.design.notice = Some(match a.status {
                409 => format!(
                    "A project named {name} is already on the box. Keep it again for another name. Nothing was written."
                ),
                401 => {
                    "The box no longer knows this page's session. Sign in again. Nothing was written.".into()
                }
                403 => "Only an engineer makes a project on the box. Nothing was written.".into(),
                0 => {
                    format!("The box did not answer. Nothing was written. Keep it again to try again. {why}")
                }
                s => format!("The box refused the project ({s}): {why}. Nothing was written."),
            });
            return;
        }
        self.projects.design_made(name);
        self.projects.note(
            format!(
                "Kept the design as {} in {name}, a copy of the project. Build runs in it.",
                crate::design_step::DESIGN_FILE
            ),
            false,
        );
        self.design.discard();
        self.design_built.remove(name);
    }

    /// The open project's kept design, as the tab shows it.
    fn kept_design(&mut self) -> Option<crate::design_ui::Kept> {
        let o = self.projects.open.as_ref()?;
        let text = o
            .bufs
            .get(crate::design_step::DESIGN_FILE)
            .filter(|b| b.loaded)
            .map(|b| b.text.clone())?;
        let built = self.design_built.get(&o.name).copied().unwrap_or(0);
        let blocked = self.agent_blocked().or_else(|| {
            self.instruct
                .busy()
                .then(|| "Still working on the last instruction.".to_string())
        });
        let (d, problems) = self.design_cache.get(&text);
        Some(crate::design_step::kept_view(d, problems, built, blocked))
    }

    /// Build: the next batch of the kept design's build order goes to the
    /// steps run on this box's Agent, `Design::step_brief` as each step's
    /// instruction.
    pub fn build_design(&mut self, ctx: &egui::Context) {
        if self.agent_blocked().is_some() || self.instruct.busy() {
            return;
        }
        let Some((project, text)) = self.projects.open.as_ref().and_then(|o| {
            let b = o.bufs.get(crate::design_step::DESIGN_FILE).filter(|b| b.loaded)?;
            Some((o.name.clone(), b.text.clone()))
        }) else {
            return;
        };
        let (design, problems) = self.design_cache.get(&text);
        if !problems.is_empty() {
            return;
        }
        let built = self.design_built.get(&project).copied().unwrap_or(0);
        let Some(batch) = crate::design_step::batch(design, built) else {
            return;
        };
        let label = batch.label(design);
        let n = batch.to - batch.from;
        let (from, total) = (batch.from, batch.total);
        let main = helmsman_project::MAIN;
        let scope = self.agent_files().map(|(_, s, files)| {
            if files.contains_key(main) {
                crate::scope::Scope::File(main.to_string())
            } else {
                s
            }
        });
        let started = self.with_env(scope, |i, env| {
            i.start_design_steps(env, &label, (batch.steps, batch.briefs), (from, total))
        });
        match started {
            Some(Ok(ask)) => {
                self.design_building = Some((project, n));
                let message = format!("{}\n\n{}", crate::instruct::BOX_RULES, ask.message);
                self.agent_send(&message, true, ctx);
            }
            Some(Err(why)) => self.instruct.failed(&why),
            None => {}
        }
    }

    /// [`Self::kept_design`], for the tests.
    #[cfg(test)]
    pub fn kept_design_for_test(&mut self) -> Option<crate::design_ui::Kept> {
        self.kept_design()
    }

    /// [`Self::design_act`], for the tests.
    #[cfg(test)]
    pub fn design_act_for_test(&mut self, act: crate::design_ui::Act, ctx: &egui::Context) {
        self.design_act(act, ctx);
    }

    /// A batch that ended counts its steps when it was written.
    fn design_settle(&mut self, ctx: &egui::Context) {
        if self.design_building.is_some() && !self.instruct.busy() {
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
        if self.design.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
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
            Act::CarryOn => {
                let now = ctx.input(|i| i.time);
                if let Some(req) = self.design.carry_on(now) {
                    self.design_send_req(&req, ctx);
                }
            }
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
        let blocked = self.design_blocked();
        let mut acts = Vec::new();
        acts.extend(crate::design_ui::ask_window(
            ctx,
            chrome,
            &mut self.design,
            blocked.as_deref(),
        ));
        let keep = self.writable().err();
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

    /// The outcome of the last instruction, in Messages.
    pub(crate) fn log_agent(&mut self) {
        let Some(o) = &self.instruct.outcome else { return };
        // Only an answer that was written waits to be deployed; a refusal,
        // a reply without code, a failure or an Undo does not.
        self.agent_pending = o.kind == crate::instruct::Kind::Applied;
        self.log.outcome(self.agent_at, o);
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
        // Nick 2026-11-25: "I'm signed in but the help agent isn't
        // working": the box had restarted and forgotten the session while
        // the header still showed it. A 401 here signs the header out too.
        if matches!(inc, crate::box_agent::Incoming::Status { status: 401, .. }) && self.session.is_some() {
            self.projects.ended = true;
        }
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

    /// The simulated (`sim`) inputs with a value: what Reset sets on a
    /// Skipper's page.
    fn sims(&self) -> impl Iterator<Item = (&str, serde_json::Value)> {
        self.state
            .signals
            .iter()
            .filter(|s| s.sim && s.dir == "in")
            .filter_map(|s| {
                let zero = match s.value? {
                    Value::Bool(_) => serde_json::json!(false),
                    Value::Int(_) => serde_json::json!(0),
                    Value::Real(_) => serde_json::json!(0.0),
                };
                Some((s.name.as_str(), zero))
            })
    }

    #[must_use]
    pub fn sim_inputs(&self) -> usize {
        self.sims().count()
    }

    /// Reset on a Skipper's page (Nick, 2026-11-25: "The reset state
    /// button should only be applied to simulated I/O when running on an
    /// actual device"): every simulated input back to off or 0, the
    /// program left running and the real points untouched.
    pub fn reset_sims(&mut self) {
        let sims: Vec<(String, serde_json::Value)> = self.sims().map(|(n, v)| (n.to_string(), v)).collect();
        for (name, zero) in &sims {
            self.projects.set_sim(name, zero);
        }
        if !sims.is_empty() {
            self.projects.note(
                format!(
                    "Simulated inputs back to off or 0: {}.",
                    sims.iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                false,
            );
        }
    }

    /// Help mode's Ask: the question and the page's Messages to the box.
    pub fn ask_help(&mut self, ctx: &egui::Context) {
        if self.help_blocked().is_some() || crate::help::refusal(&self.help.draft).is_some() {
            return;
        }
        self.help_seq += 1;
        let req = crate::help::request(&self.help.draft, &self.log.full_text(), self.help_seq);
        self.help.helping = Some(crate::help::Helping::new(self.help_seq));
        #[cfg(target_arch = "wasm32")]
        self.help_http.send(req, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            self.help_sent.push(req);
        }
    }

    /// "Chat about this" on block `n` of the diagram: a chip for it on
    /// the Agent tab, and the tab shown. Nothing is sent. Only where the
    /// diagram's lines are the open project's: the project running, or
    /// the one program file the box runs as it is.
    pub fn chat_about_node(&mut self, n: helmsman_diagram::NodeId, chrome: &mut Chrome) {
        let chip = self.view.as_ref().and_then(|v| {
            let (_, _, files) = self.agent_files()?;
            let locate = box_locate(&v.src, &files, self.diagram_running());
            crate::chat_about::of_node(v.diagram(), n, &locate, &files)
        });
        self.collect(
            chip,
            "That block's statement could not be placed: the diagram is of the project running, deploy the open one first.",
            chrome,
        );
    }

    /// Whether the project open is the one the box runs: the diagram's
    /// lines are its files' only then.
    fn diagram_running(&self) -> bool {
        self.projects.open.as_ref().map(|o| &o.name) == self.projects.running.as_ref()
    }

    /// The blocks selected in the diagram are the Agent's focus: its
    /// chips, made again when the selection, the chips collected or the
    /// diagram change.
    fn sync_focus(&mut self) {
        let Some(v) = self.view.as_mut() else {
            self.instruct.focus.clear();
            return;
        };
        crate::chat_about::apply_off(&mut self.instruct, v);
        let running = self.diagram_running();
        let Some(v) = self.view.as_ref() else { return };
        let sel = v.selection().to_vec();
        let key = (sel.clone(), self.instruct.chips.len(), v.src.len(), running);
        if self.instruct.focus.is_for(&key) {
            return;
        }
        let made = if sel.is_empty() {
            crate::chat_about::Focus::default()
        } else {
            self.agent_files()
                .map_or_else(crate::chat_about::Focus::default, |(_, _, files)| {
                    let locate = box_locate(&v.src, &files, running);
                    crate::chat_about::focus_of(v.diagram(), &sel, &self.instruct.chips, &locate, &files)
                })
        };
        self.instruct.focus.set(key, made);
    }

    /// "Chat about this" on the input, output or variable `name`: a chip
    /// for it (its declaration, else its first use, as the place) on the
    /// Agent tab, and the tab shown. Nothing is sent.
    pub fn chat_about_signal(&mut self, name: &str, chrome: &mut Chrome) {
        let chip = self.agent_files().and_then(|(_, _, files)| {
            let diagrams: Vec<&helmsman_diagram::Diagram> = self
                .view
                .as_ref()
                .map(|v| v.module.pous.iter().map(|p| &p.diagram).collect())
                .unwrap_or_default();
            crate::chat_about::of_signal(name, &files, &diagrams)
        });
        self.collect(
            chip,
            &format!("{name} is not in the open project's files."),
            chrome,
        );
    }

    /// Takes `chip` for the next instruction and shows the Agent tab, on
    /// the program side; the notice `missing` when there is none.
    fn collect(&mut self, chip: Option<crate::chat_about::Chip>, missing: &str, chrome: &mut Chrome) {
        match chip {
            Some(c) => {
                self.instruct.add_chip(c);
                self.help.mode = crate::help::Mode::Code;
                chrome.show(crate::editor::Pane::Claude);
            }
            None => self.projects.note(missing, true),
        }
    }

    /// The Agent tab: its heading (the agent's state as a dot, the mode's
    /// name, the mode toggle), then the shared tab (changing the program)
    /// or Help.
    fn agent_tab(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        use crate::help::Mode;
        use crate::instruct::Act;
        // Where the diagram's selection is being used: a click here keeps it.
        chrome.hit("agent tab", ui.clip_rect());

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
            steps: self.revision_steps(),
            room_below: None,
            progress: None,
            // The question card is the page's for now (TODO.md
            // `agent-wait-questions`: a box after `agent-progress-box`).
            card: None,
            big: None,
            io: None,
            versions: self.versions(),
        };
        let help_view = crate::help::View {
            blocked: self.help_blocked(),
        };
        let design_off = self.design_blocked();
        let mut design_act = None;
        let deploy_state = self.agent_deploy();
        let mut deploy_go = false;
        let (act, help_act) = egui::ScrollArea::vertical()
            .id_salt("box-agent")
            .show(ui, |ui| {
                // Room for the scroll bar (a resizable panel grows to any
                // row that overflows it).
                ui.set_max_width(ui.available_width() - 12.0);
                crate::help::heading(ui, chrome, self.agent.status.dot(), &self.agent.status.line());
                ui.add_space(4.0);
                match self.help.mode {
                    Mode::Code => {
                        let act = crate::instruct::ui(ui, chrome, &mut self.instruct, &view);
                        deploy_go = deploy_row(ui, chrome, &deploy_state);
                        let view = crate::design_ui::EntryView {
                            now: ctx.input(|i| i.time),
                            blocked: design_off.clone(),
                            keep_blocked: self.writable().err(),
                            kept: self.kept_design(),
                        };
                        if let Some(a) = crate::design_ui::entry_ui(ui, chrome, &mut self.design, &view) {
                            design_act = Some(a);
                        }
                        (act, None)
                    }
                    Mode::Help => (None, crate::help::ui(ui, chrome, &mut self.help, &help_view)),
                }
            })
            .inner;
        if deploy_go {
            self.deploy();
        }
        if let Some(a) = design_act {
            self.design_act(a, &ctx);
        }
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
            Some(Act::Stop) => self.stop_run(),
            Some(Act::Undo) => self.undo_change(),
            Some(Act::Review(id)) => self.wants_review = Some(id),
            // A box's change goes through a deploy: no arrows here.
            // A box never plans in steps (`steps` is the page's).
            Some(Act::Offer(k)) => self.take_offer(k, &ctx),
            Some(Act::Revision(n) | Act::Details(n)) => self.show_revision(n),
            Some(Act::RunSteps | Act::Answer(_) | Act::Split | Act::OneGo) | None => {}
        }
    }

    /// History in the Agent tab and the Code tab (Nick, 2026-11-25: "move
    /// it to 2 places: agent so that you can easily move anywhere in the
    /// history to see the prompt and output as well as in the code tab so
    /// you can do the same thing but with all of the code"): Back and
    /// Forward from the revision last shown, the newest at first.
    fn revision_steps(&self) -> Option<crate::instruct::Steps> {
        let revs = &self.projects.history;
        let k = match self.history_at {
            Some(n) => revs.iter().position(|r| r.n == n)?,
            None => revs.len().checked_sub(1)?,
        };
        let tip = |j: usize, way: &str| {
            revs.get(j).map(|r| {
                (
                    r.n,
                    format!(
                        "{way}: shows revision {}, {}: its instruction, the answer and the code.",
                        r.n, r.what
                    ),
                )
            })
        };
        Some(crate::instruct::Steps {
            back: k.checked_sub(1).and_then(|j| tip(j, "Back")),
            forward: tip(k + 1, "Forward"),
        })
    }

    /// The project's versions for the clock's list, in the Agent tab and
    /// the Code tab alike; `None` while there are none.
    fn versions(&self) -> Option<crate::instruct::Versions> {
        (!self.projects.history.is_empty()).then(|| crate::instruct::Versions {
            rows: self.projects.history.clone(),
            at: self
                .history_at
                .or_else(|| self.projects.history.last().map(|r| r.n)),
        })
    }

    /// Revision `n` in the middle of the screen: its instruction and
    /// answer (the Agent page), each file against the project now, and
    /// Restore for an engineer.
    fn show_revision(&mut self, n: u32) {
        self.history_at = Some(n);
        self.project_act(crate::projects::Act::ShowRevision(n));
    }

    /// Offer `k` under the last answer, clicked ([`crate::offers`]): an
    /// instruction is sent as the engineer's own; a rename is saved into
    /// every file it touches with its History entry, as a described
    /// change is.
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

    /// A literal of the diagram written `new` (`text` is its file after),
    /// saved on the box with its History entry.
    pub fn set_value(&mut self, lit: &crate::by_hand::Literal, new: &str, text: String) {
        let Some(project) = self.projects.open.as_ref().map(|o| o.name.clone()) else {
            return;
        };
        let before = self.projects.history.last().map(|r| r.n);
        let record = crate::by_hand::value_record(lit, new);
        self.projects.save_agent(&lit.file, &text, &record, &[]);
        let undo = before.map(|revision| crate::instruct::Undo {
            project,
            file: lit.file.clone(),
            revision,
            text,
        });
        self.instruct.value_set(&lit.text, new, &lit.file, undo);
        self.log_agent();
    }

    /// What block `n` of the diagram lets the engineer change by hand
    /// ([`crate::by_hand`]): nothing for someone who may not write, or
    /// while the diagram's lines are not the open project's.
    fn by_hand_target(&self, n: helmsman_diagram::NodeId) -> Option<crate::by_hand::Target> {
        if self.assign_write().is_err() {
            return None;
        }
        let v = self.view.as_ref()?;
        let (_, _, files) = self.agent_files()?;
        let locate = box_locate(&v.src, &files, self.diagram_running());
        crate::by_hand::target(v.diagram(), n, &locate, &files)
    }

    /// Opens the question for a name or a value.
    fn by_hand_ask(&mut self, t: crate::by_hand::Target) {
        match t {
            crate::by_hand::Target::Name(n) => self.devices_ui.renaming = Some((n.clone(), n)),
            crate::by_hand::Target::Value(l) => self.value_ask = Some(crate::by_hand::ValueAsk::new(l)),
        }
    }

    /// `old` renamed to `new` in the open project, saved on the box in
    /// every file with one History entry.
    pub fn rename_everywhere(&mut self, old: &str, new: &str) {
        let files: crate::scope::Files = self.loaded_files().into_iter().collect();
        let plan = match crate::offers::plan(&files, old, new) {
            Ok(p) => p,
            Err(why) => {
                self.instruct.offer_refused(&why);
                return self.log_agent();
            }
        };
        let Some(project) = self.projects.open.as_ref().map(|o| o.name.clone()) else {
            return;
        };
        let before = self.projects.history.last().map(|r| r.n);
        let record = crate::instruct::Instruct::rename_record(old, new, &plan.lines());
        let count = plan.files.len();
        // The program file carries the History entry; the others go with it.
        let lead = plan
            .files
            .iter()
            .position(|(f, ..)| {
                helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St)
                    && !helmsman_iec::spec::is_test(f)
            })
            .or_else(|| {
                plan.files.iter().position(|(f, ..)| {
                    helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St)
                })
            })
            .unwrap_or(0);
        let mut rest: Vec<(String, String)> = Vec::new();
        let mut first = None;
        for (k, (f, t, _)) in plan.files.into_iter().enumerate() {
            if k == lead {
                first = Some((f, t));
            } else {
                rest.push((f, t));
            }
        }
        let Some((file, text)) = first else { return };
        self.projects.save_agent(&file, &text, &record, &rest);
        let undo = before.map(|revision| crate::instruct::Undo {
            project,
            file,
            revision,
            text,
        });
        self.instruct.renamed(old, new, count, undo);
        self.log_agent();
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
                // The flash below logs it.
                Deploy::Done(_) => self.agent_pending = false,
                Deploy::Failed(why) => self.log.error(now, format!("Deploy failed: {why}")),
                _ => {}
            }
            self.seen.deploy = self.projects.deploy.clone();
        }
        let warned = projects::warning_summary(&self.projects.diags);
        if warned != self.seen.warned {
            if let Some(w) = &warned {
                self.log.warning(now, w.clone());
            }
            self.seen.warned = warned;
        }
        let fault = self.state.scan.as_ref().and_then(|s| s.fault.clone());
        if fault != self.seen.fault {
            if let Some(f) = &fault {
                self.log.error(now, format!("Fault: {f}"));
            }
            self.seen.fault = fault;
        }
        let flash = self.projects.flash.as_ref().map(|f| f.at);
        if flash != self.seen.flash {
            if let Some(f) = &self.projects.flash {
                self.log.info(now, f.line.clone());
            }
            self.seen.flash = flash;
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
        self.devices_ui.config.modal_ui(&ui.ctx().clone(), chrome);
        self.devices_ui.node_map.window(&ui.ctx().clone(), chrome);
        for r in self.devices_ui.node_map.take_requests() {
            self.projects.enqueue(r);
            ui.ctx().request_repaint();
        }
        for r in self.devices_ui.config.take_requests() {
            self.projects.enqueue(r);
            ui.ctx().request_repaint();
        }
        for r in self.devices_ui.proposal.take_requests() {
            self.projects.enqueue(r);
            ui.ctx().request_repaint();
        }
        self.vendor_file(&ui.ctx().clone());
        self.design_windows(&ui.ctx().clone(), chrome);
        if let Some(crate::devices::DeviceAct::Add(w)) = self.devices_ui.form_ui(&ui.ctx().clone(), chrome) {
            self.apply_wiring(&w);
        }
        if let Some(crate::devices::DeviceAct::Quick(q)) = self.devices_ui.quick_ui(&ui.ctx().clone(), chrome)
        {
            self.quick_add(&q);
        }
        if let Some(crate::devices::DeviceAct::Add(w)) = self.devices_ui.add_io_ui(&ui.ctx().clone(), chrome)
        {
            self.apply_wiring(&w);
        }
        self.devices_ui.wire_it_ui(&ui.ctx().clone(), chrome);
        if let Some(crate::devices::DeviceAct::Remove(names)) =
            self.devices_ui.remove_ui(&ui.ctx().clone(), chrome)
            && let Some(b) = self
                .projects
                .open
                .as_mut()
                .and_then(|o| o.bufs.get_mut(helmsman_project::IO_MAP))
                .filter(|b| b.loaded)
        {
            b.text = crate::device_form::remove_signals(&b.text, &names);
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
        {
            self.rename_everywhere(&plan.old, &plan.new);
        }
        if let Some(mut ask) = self.value_ask.take() {
            let files = self.agent_files().map(|(_, _, f)| f).unwrap_or_default();
            let mut open = true;
            match ask.ui(&ui.ctx().clone(), chrome, &files, &mut open) {
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
            Some(crate::wired::PickAct::Search(q)) => {
                self.projects.find_wired(&q);
                ui.ctx().request_repaint();
            }
            Some(crate::wired::PickAct::Set { signal, wired }) => {
                let buf = self
                    .projects
                    .open
                    .as_mut()
                    .and_then(|o| o.bufs.get_mut(helmsman_project::IO_MAP))
                    .filter(|b| b.loaded);
                match buf
                    .and_then(|b| Some((crate::wired::set_wired(&b.text, &signal, wired.as_deref())?, b)))
                {
                    Some((text, b)) => b.text = text,
                    None => self.projects.note(
                        format!("{signal} has no line in the open project's io.map to record it on."),
                        true,
                    ),
                }
            }
            None => {}
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

    /// The quick form's Add: the profile into the open project as
    /// `NAME.device` (a new file, or the draft of one that exists), then
    /// the device and its channels into io.map.
    fn quick_add(&mut self, q: &crate::devices::QuickAdd) {
        let Some(o) = &mut self.projects.open else {
            self.projects.note("Open a project to add the device to.", true);
            return;
        };
        match o.bufs.get_mut(&q.file) {
            Some(b) if b.loaded => b.text.clone_from(&q.text),
            Some(_) => {}
            None => self.projects.new_file_with(&q.file, &q.text),
        }
        self.apply_wiring(&q.wiring);
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

    /// The open project's program's inputs and outputs (none while its
    /// files are not loaded).
    fn program_ports(&self) -> Vec<crate::device_form::Port> {
        let files = self.loaded_files();
        let program = crate::device_form::program_file(files.iter().map(|(n, t)| (n.as_str(), t.as_str())))
            .and_then(|f| files.iter().find(|(n, _)| *n == f).map(|(_, t)| t.clone()))
            .unwrap_or_default();
        let mut memo = self.ports_memo.borrow_mut();
        if let Some((text, ports)) = memo.as_ref()
            && *text == program
        {
            return ports.clone();
        }
        let ports = crate::device_form::ports_of(&program);
        *memo = Some((program, ports.clone()));
        ports
    }

    /// Why io.map cannot be written, when it cannot.
    fn assign_write(&self) -> Result<(), String> {
        if self.has_projects() {
            self.writable()
        } else {
            Err("This box runs without projects: its map is a file on the box.".into())
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

    /// Writes an assignment (or its undoing) into the open project's
    /// io.map buffer, as the form's Add does: it is then unsaved, and goes
    /// to the box with Save or Deploy. Refused, with nothing changed and
    /// the reason in a note, when it cannot be made.
    fn assign_act(&mut self, a: &crate::assign::AssignAct) {
        if let Err(why) = self.assign_write() {
            self.projects.note(why, true);
            return;
        }
        self.assign_context();
        let ports = self.program_ports();
        let Some(o) = &mut self.projects.open else {
            self.projects.note("Open a project first.", true);
            return;
        };
        let Some(b) = o.bufs.get_mut(helmsman_project::IO_MAP) else {
            self.projects.note(
                "This project has no io.map to assign in. Add a device in the Devices tab first.",
                true,
            );
            return;
        };
        if !b.loaded {
            self.projects.note("io.map is not loaded yet.", true);
            return;
        }
        match a.run(&b.text, &ports, &|d| self.devices_ui.channels_of(d)) {
            Ok(c) => {
                b.text = c.text;
                // A port that went to a device has its `plant.sim` line
                // switched off (kept), and back on with the way back.
                let (port, off) = match a {
                    crate::assign::AssignAct::Assign { port, .. } => (port, true),
                    crate::assign::AssignAct::Back { port } => (port, false),
                };
                if let Some(p) = o.bufs.get_mut(helmsman_project::PLANT)
                    && p.loaded
                    && let Some(new) = helmsman_sim::set_off(&p.text, port, off)
                {
                    p.text = new;
                }
                self.projects.note(c.summary, false);
            }
            Err(why) => self.projects.note(why, true),
        }
    }

    /// The map and the program's ports, as the question about an
    /// assignment reads them this frame.
    fn assign_context(&mut self) {
        self.devices_ui.map = helmsman_project::iomap::skim(&self.map_text().0);
        self.devices_ui.program = self.program_ports();
        let files = self.loaded_files();
        self.devices_ui.set_profiles(
            files
                .iter()
                .filter(|(n, _)| n.ends_with(helmsman_project::DEVICE_EXT))
                .map(|(n, t)| (n.as_str(), t.as_str())),
        );
    }

    fn add_device_line(&mut self, line: &str) {
        let Some(o) = &mut self.projects.open else { return };
        match o.bufs.get_mut(helmsman_project::IO_MAP) {
            Some(b) if b.loaded => b.text = crate::devices::appended(&b.text, line),
            Some(_) => {}
            None => self.projects.new_file_with(helmsman_project::IO_MAP, line),
        }
    }

    /// Whether the running module may be edited and deployed here: on a
    /// box or Skipper without projects, for an engineer, once its source
    /// came.
    fn module_writable(&self) -> Result<(), String> {
        if !self.node_token {
            is_engineer(self.session.as_ref()).map_err(|_| {
                "Enter the Skipper's token to edit and deploy: Sign in, top right.".to_string()
            })?;
        }
        if self.projects.available != Some(false) {
            return Err("Waiting for the box to say what it holds.".into());
        }
        if self.state.selected.is_none() {
            return Err("Pick a module first.".into());
        }
        if self.state.source().is_none() {
            return Err("Waiting for the module's source.".into());
        }
        Ok(())
    }

    /// The running module's source, editable, with Deploy at the header's
    /// end: a Skipper, or a box run without projects, takes the program
    /// straight (`PUT /api/modules/{name}`), compiles it and keeps the old
    /// one running if it will not.
    fn module_editor(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        name: &str,
        write: &Result<(), String>,
    ) {
        let deploy = if self.projects.deploy.busy() {
            Err("Deploying…".to_string())
        } else {
            write.clone()
        };
        let dirty = self
            .projects
            .module
            .as_ref()
            .is_some_and(super::box_projects::ModuleEdit::dirty);
        let errors = self.projects.diags.iter().filter(|d| !d.is_warning()).count();
        let first = self.projects.diags.first().map(|d| d.message.clone());
        let mut go = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            editor::file_header(
                ui,
                chrome,
                name,
                "The module that runs. Edit it here; Deploy sends it, and the old version keeps running if it does not compile.",
            );
            editor::wrap_toggle(ui, chrome);
            if dirty {
                let t = theme(ui);
                let r = ui
                    .add(crate::theme::whole(RichText::new("edited").color(t.accent)))
                    .tip("Not running yet: Deploy sends it.");
                chrome.hit("module edited", r.rect);
            }
            go = editor::header_right(
                ui,
                chrome,
                &editor::Status {
                    errors,
                    first: first.as_deref(),
                    deploy: &deploy,
                    deploy_tip: "Ctrl Shift Enter: deploy this version.\nThe box compiles it and runs it in place of the old one.",
                    busy: self.projects.deploy.stage(self.watches_node),
                    steps: None,
                    history: false,
                },
            )
            .deploy;
        });
        if go {
            self.deploy();
        }
        let id = editor::source_id("box-st-module", "", name);
        let Some(m) = self.projects.module.as_mut() else {
            return;
        };
        editor::st_editor(ui, id, &mut m.text, &[], &[], MAX_FILE, chrome);
    }

    /// The Agent tab's Deploy: whether it may be pressed, whether what is
    /// saved is not running yet, and where it goes.
    fn agent_deploy(&self) -> AgentDeploy {
        // It pulses for the Agent's written answer only (Nick, 2026-11-24:
        // "only pulse once the agent has a successful output").
        let pending = self.agent_pending;
        let ready = if self.has_projects() {
            self.writable()
        } else {
            self.module_writable()
        };
        let ready = if self.projects.deploy.busy() {
            Err("Deploying…".to_string())
        } else {
            ready
        };
        AgentDeploy {
            ready,
            pending,
            to_node: self.watches_node,
            busy: self.projects.deploy.stage(self.watches_node),
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
        if let Some(name) = &self.fresh {
            return format!("{name} has not run yet. Deploy it to see it here.");
        }
        let s = &self.state;
        if let Some(p) = &s.problem {
            return p.clone();
        }
        match (&s.selected, &s.scan) {
            _ if s.link() == Link::SignIn => "Sign in to watch this box (Sign in, top right).".into(),
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
            Link::Unreachable { at, .. } => {
                (format!("● box unreachable (retry in {}s)", retry(at)), t.ink(RED))
            }
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

    /// The module shown, when it is stopped or has faulted.
    fn idle_module(&self) -> Option<&crate::remote::ModuleRow> {
        let shown = self.state.selected.as_ref()?;
        self.state
            .modules
            .iter()
            .find(|m| &m.name == shown && matches!(m.state.as_str(), "stopped" | "faulted"))
    }

    /// Forces input `name` to `v` (an engineer's click on its block); a
    /// placeholder (`sim`) input is set instead, held until set again.
    pub fn force(&mut self, name: &str, v: Value) {
        if is_engineer(self.session.as_ref()).is_err() {
            return;
        }
        let json_of = |v: Value| match v {
            Value::Bool(b) => json!(b),
            Value::Int(n) => i64::try_from(n).map_or_else(|_| json!(n.to_string()), |n| json!(n)),
            Value::Real(r) => json!(r),
        };
        if self
            .state
            .signals
            .iter()
            .any(|s| s.sim && s.name.eq_ignore_ascii_case(name))
        {
            self.projects.set_sim(name, &json_of(v));
            return;
        }
        // A program that is not running reads no input: the force would
        // sit on the box and change nothing until someone starts it. Say
        // so and leave the input alone (Nick, 2026-11-16).
        if let Some(m) = self.idle_module() {
            self.projects.note(
                format!(
                    "{} is {}, so forcing {} would change nothing. Start the program first.",
                    m.name,
                    m.state,
                    name.to_ascii_lowercase()
                ),
                false,
            );
            return;
        }
        self.projects.force(name, &json_of(v));
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

    /// What a card of the I/O tab asked for.
    fn io_asked(
        &mut self,
        name: &str,
        input: bool,
        a: crate::io_cards::Asked,
        chrome: &mut Chrome,
        now: f64,
    ) {
        if a.details {
            let key = name.to_ascii_lowercase();
            if !self.io_open.remove(&key) {
                self.io_open.insert(key);
            }
        }
        if a.code {
            self.find_in_code(name, chrome);
        }
        if a.trend {
            // The pin as the Trend names it, whatever case the card has.
            let pin = self
                .trend
                .pins
                .iter()
                .find(|p| p.eq_ignore_ascii_case(name))
                .cloned()
                .unwrap_or_else(|| name.to_string());
            let added = !self.trend.pins.contains(&pin);
            self.trend.toggle(&pin, self.session.as_ref(), now);
            if added {
                chrome.trend = true;
                chrome.msgs = false;
            }
        }
        if let Some(v) = &mut self.view {
            if a.diagram {
                v.show_signal(name);
            }
            if a.tapped && !input {
                v.show_output(name);
            }
        }
    }

    /// The `< >` glyph of a point's card: the Code tab with the find box
    /// on its name, the first place it is named selected; asked again,
    /// the next place. The file that declares it opens first when the
    /// project open is the one running.
    fn find_in_code(&mut self, name: &str, chrome: &mut Chrome) {
        let again = chrome
            .find
            .as_ref()
            .is_some_and(|f| f.word && f.query.eq_ignore_ascii_case(name));
        let running = self.has_projects()
            && self.projects.open.as_ref().map(|o| &o.name) == self.projects.running.as_ref();
        if !again
            && running
            && let Some(v) = &self.view
            && let Some(line) = v.declared_line(name)
            && let Some((file, _)) = projects::file_line(&v.src, line)
        {
            self.projects.open_file(&file);
        }
        chrome.show_code();
        crate::find::Find::name(&mut chrome.find, &name.to_ascii_lowercase());
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

/// A placeholder (`sim`) input's control on its I/O card, on a line of
/// its own (a row beside the value would widen the panel): a BOOL is a
/// Turn on / Turn off button, a number a field to drag or type. The value
/// to set it to, if one was asked for.
fn sim_control(
    ui: &mut egui::Ui,
    value: Option<helmsman_iec::Value>,
) -> Option<(Rect, Option<helmsman_iec::Value>)> {
    use helmsman_iec::Value as V;
    let tip = "A placeholder input, no device yet: set it here. It holds until you change it or it restarts.";
    Some(match value? {
        V::Bool(on) => {
            let r = ui.button(if on { "Turn off" } else { "Turn on" }).tip(tip);
            (r.rect, r.clicked().then_some(V::Bool(!on)))
        }
        V::Int(n) => {
            let mut x = i64::try_from(n).unwrap_or(0);
            let r = ui.add(egui::DragValue::new(&mut x)).tip(tip);
            (r.rect, r.changed().then_some(V::Int(i128::from(x))))
        }
        V::Real(f) => {
            let mut x = f;
            let r = ui.add(egui::DragValue::new(&mut x).speed(0.1)).tip(tip);
            (r.rect, r.changed().then_some(V::Real(x)))
        }
    })
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
            Act::NewProject(p) => self.projects.create_started(&p),
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
            self.projects.to_node = self.watches_node;
            self.projects.deploy();
        } else if self.module_writable().is_ok() {
            self.projects.deploy_module();
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
                RichText::new(format!(
                    "t={}s",
                    helmsman_iec::shown_real(s.now_ms as f64 / 1000.0)
                ))
                .monospace()
                .color(t.muted),
            )
            .tip(format!("The box's clock since start, at scan {}.", s.scan));
        }
    }

    fn has_notices(&self) -> bool {
        self.state.scan.as_ref().is_some_and(|s| s.fault.is_some())
            || !matches!(self.projects.deploy, Deploy::Idle | Deploy::Done(_))
            || self.projects.notice.is_some()
            || self.desk.problem.is_some()
            || self.on_box.problem.is_some()
    }

    fn alarm(&self) -> bool {
        self.state.scan.as_ref().is_some_and(|s| s.fault.is_some())
            || matches!(self.projects.deploy, Deploy::Failed(_))
            || self.projects.notice.as_ref().is_some_and(|(_, error)| *error)
    }

    fn notices(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        if let Some(f) = self.state.scan.as_ref().and_then(|s| s.fault.as_ref()) {
            crate::messages::copyable(
                ui,
                chrome,
                "fault",
                &format!("fault: {f}"),
                crate::theme::theme(ui).ink(RED),
            );
        }
        match &self.projects.deploy {
            // A deploy that went through is the save glyph's popup and a
            // line in the log (Nick, 2026-11-26), not a notice.
            Deploy::Idle | Deploy::Done(_) => {}
            Deploy::Saving => {
                ui.label(RichText::new("saving…").color(t.muted));
            }
            Deploy::Scanning => {
                ui.label(RichText::new("finding the devices on the network…").color(t.muted));
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
            Deploy::Failed(why) => {
                let why = format!("✖ {why}");
                crate::messages::copyable(
                    ui,
                    chrome,
                    "deploy failed",
                    &why,
                    crate::theme::theme(ui).ink(RED),
                );
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
                crate::messages::copyable(ui, chrome, "notice", &text, crate::theme::theme(ui).ink(RED));
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
                place: projects::Place::Box,
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
            let text = RichText::new(format!("{arrow} {}  {} · {} scans", m.name, m.state, m.scans)).color(
                if m.state == "faulted" {
                    crate::theme::theme(ui).ink(RED)
                } else {
                    t.fg
                },
            );
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
        let (text, source) = self.map_text();
        let open = self.projects.open.as_ref();
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
                    "up" => LinkState::Up {
                        address: d.address.clone(),
                        transport: d.transport.clone(),
                        health: d.health.clone(),
                    },
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
        self.devices_ui.program = self.program_ports();
        self.devices_ui.can_declare = !program.is_empty();
        self.devices_ui.set_profiles(
            refs.iter()
                .copied()
                .filter(|(n, _)| n.ends_with(helmsman_project::DEVICE_EXT)),
        );
        self.devices_ui.map = map.clone();
        self.devices_ui.eds_files = files
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| crate::devices::is_eds(n))
            .collect();
        // Which devices have a configuration to show: by their profile,
        // when the project holds it; else the box is asked and says.
        let asks = nodes
            .iter()
            .filter_map(|n| {
                let profile = n.driver.strip_prefix("device:")?;
                let text = open
                    .and_then(|o| o.bufs.get(&format!("{profile}{}", helmsman_project::DEVICE_EXT)))
                    .filter(|b| b.loaded)
                    .map(|b| b.text.as_str());
                match text {
                    Some(t) if !crate::device_config::profile_configures(t) => None,
                    Some(_) => Some((n.name.clone(), true)),
                    None => Some((n.name.clone(), false)),
                }
            })
            .collect();
        self.devices_ui
            .config
            .set_context(is_engineer(self.session.as_ref()), asks);
        // What is wired to the map's points: one library lookup a frame
        // for each `wired=` value not asked about yet.
        let wanted = self.devices_ui.wired_wanted();
        let saved_text = self
            .devices_ui
            .proposal
            .saved_file()
            .and_then(|f| open.and_then(|o| o.bufs.get(f)))
            .filter(|b| b.loaded)
            .map(|b| b.text.clone());
        self.devices_ui.vendor.set_context(crate::device_vendor::Context {
            on_box: true,
            engineer: is_engineer(self.session.as_ref()),
            agent: self.agent.refusal("x").map_or(Ok(()), Err),
        });
        self.devices_ui
            .proposal
            .set_context(crate::device_proposal::Context {
                signed_in: self.session.is_some(),
                engineer: is_engineer(self.session.as_ref()),
                project: open.map(|o| o.name.clone()),
                saved_text,
            });
        self.devices_ui.node_map.set_context(
            self.state.io_map_node,
            self.session.is_some(),
            self.state.io_map.as_deref(),
        );
        self.devices_ui.node_map.row(ui, chrome);
        match self.devices_ui.show(ui, &nodes, &source, chrome, &write) {
            Some(crate::devices::DeviceAct::Scan) => {
                self.devices_ui.scan = crate::devices::Scan::Running;
                self.projects.discover();
            }
            Some(crate::devices::DeviceAct::Ask(address)) => {
                self.devices_ui.ask_note = Some(format!("Asking {address}…"));
                self.projects.ask(address);
            }
            Some(crate::devices::DeviceAct::Use(d)) => self.devices_ui.open(d),
            Some(crate::devices::DeviceAct::AddIo(device)) => self.devices_ui.open_add_io(&device),
            Some(crate::devices::DeviceAct::OpenMap) => {
                self.projects.open_file(helmsman_project::IO_MAP);
                chrome.show_code();
            }
            Some(crate::devices::DeviceAct::Library { vendor, product }) => {
                self.projects.library(vendor, product);
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
        if let Some(wired) = wanted {
            self.projects.wired(&wired);
        }
        if let Some((device, query)) = self.devices_ui.wiring_wanted() {
            self.projects.wiring(&device, &query);
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
            let steps = self.revision_steps();
            let versions = self.versions();
            let mut revision = None;
            // The header is the open file's name (Nick, 2026-11-08), then
            // word wrap, then the → that deploys this version.
            let name = o.current.clone().unwrap_or_else(|| "Program".to_string());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                editor::file_header(
                    ui,
                    chrome,
                    &name,
                    "The open file of the project. Save keeps it on the box; Deploy runs it.",
                );
                editor::find_toggle(ui, chrome);
                editor::wrap_toggle(ui, chrome);
                match &write {
                    Ok(()) => {}
                    Err(why) => {
                        let r = ui
                            .add(crate::theme::whole(RichText::new("read-only").color(t.muted)))
                            .tip(why.clone());
                        chrome.hit("read-only", r.rect);
                    }
                }
                let pressed = editor::header_right(
                    ui,
                    chrome,
                    &editor::Status {
                        errors: 0,
                        first: None,
                        deploy: &deploy,
                        deploy_tip: if self.watches_node {
                            "Ctrl Shift Enter: deploy this version.\nSaves and checks the project on this box, then runs it on the Skipper this page watches."
                        } else {
                            "Ctrl Shift Enter: deploy this version.\nSaves, checks and runs the project on the box."
                        },
                        busy: self.projects.deploy.stage(self.watches_node),
                        steps: steps.as_ref(),
                        history: versions.is_some(),
                    },
                );
                go = pressed.deploy;
                revision = pressed.revision;
            });
            if go {
                self.deploy();
            }
            if let Some(n) = revision {
                self.show_revision(n);
            }
            if chrome.history
                && let Some(v) = &versions
                && let Some(crate::instruct::Act::Revision(n) | crate::instruct::Act::Details(n)) =
                    editor::versions_panel(ui, chrome, &mut self.instruct, v)
            {
                self.show_revision(n);
            }
            let Some(file) = o.current.clone() else {
                ui.label(RichText::new("No files in this project yet.").color(t.muted));
                return;
            };
            let diags = projects::marks_in(&self.projects.diags, &file, false);
            let warns = projects::marks_in(&self.projects.diags, &file, true);
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
                    editor::st_editor(ui, id, &mut buf.text, &diags, &warns, MAX_FILE, chrome);
                } else {
                    let mut text: &str = &buf.text;
                    editor::st_editor(ui, id, &mut text, &diags, &warns, usize::MAX, chrome);
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
            .unwrap_or_else(|| "Program".to_string());
        if let (Some(m), Some(src)) = (
            self.state.selected.clone(),
            self.state.source().map(str::to_string),
        ) {
            self.projects.track_module(&m, &src);
        }
        let write = self.module_writable();
        if write.is_ok() {
            self.module_editor(ui, chrome, &name, &write);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            editor::file_header(ui, chrome, &name, "The source of the module that runs.");
            editor::wrap_toggle(ui, chrome);
            let why = write.err().unwrap_or_default();
            let r = ui
                .add(crate::theme::whole(RichText::new("read-only").color(t.muted)))
                .tip(why);
            chrome.hit("read-only", r.rect);
        });
        if self.view.is_none()
            && let Some(src) = self.state.source()
        {
            {
                let mut src: &str = src;
                editor::st_editor(
                    ui,
                    editor::source_id("box-st-module", "", &name),
                    &mut src,
                    &[],
                    &[],
                    usize::MAX,
                    chrome,
                );
                return;
            }
        }
        // With no source yet, the canvas beside this pane says what the
        // box waits for; said here too it was on screen twice.
        if let Some(v) = &self.view {
            let mut src: &str = &v.src;
            editor::st_editor(
                ui,
                egui::Id::unique("box-st"),
                &mut src,
                &[],
                &[],
                usize::MAX,
                chrome,
            );
        }
    }

    fn trend(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let map = self.map_text().0;
        self.trend
            .ui(ui, chrome, &self.state, &map, self.session.as_ref());
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
        self.assign_context();
        let scan = live_scan(&self.state);
        // What is wired to each signal, from the map and the library.
        self.devices_ui.map = helmsman_project::iomap::skim(&self.map_text().0);
        if let Some(w) = self.devices_ui.wired_wanted() {
            self.projects.wired(&w);
        }
        let wired = crate::wired::by_signal(&self.devices_ui.map);
        // Which signals are the open project's program ports, where each
        // is, and what a right-click on its row offers.
        let menus = self.devices_ui.port_menus(&self.assign_write());
        let map = self.devices_ui.map.clone();
        let mut asks: Vec<(String, crate::assign_ui::PortAsk)> = Vec::new();
        let mut chat = crate::assign_ui::RowChat {
            full: self.instruct.chips.len() >= crate::chat_about::MAX_CHIPS,
            asked: Vec::new(),
            rename_off: self.assign_write().err(),
            rename: Vec::new(),
        };
        let known = &self.devices_ui.wired;
        // What the diagram says of each point; nothing while there is none.
        let cards = self.view.as_ref().map(Viewer::output_cards);
        // What is on the Trend: its cards show their trend glyph on.
        let pinned = self.trend.pins.clone();
        let lit = self
            .view
            .as_ref()
            .and_then(|v| v.because.clone())
            .unwrap_or_default();
        let src = self.view.as_ref().map(|v| v.src.clone());
        let open: Vec<String> = self.io_open.iter().cloned().collect();
        let is_open = |name: &str| open.iter().any(|o| o.eq_ignore_ascii_case(name));
        let signals = &self.state.signals;
        let mut asked: Vec<(String, bool, crate::io_cards::Asked)> = Vec::new();
        // A placeholder (`sim`) input's control on its card: an engineer
        // sets it where it runs (Nick, 2026-11-24).
        let engineer = is_engineer(self.session.as_ref()).is_ok();
        let mut sets: Vec<(String, helmsman_iec::Value)> = Vec::new();
        egui::ScrollArea::vertical().id_salt("box-io").show(ui, |ui| {
            use crate::io_cards::{Form, Links, Row};
            // Inputs, then Outputs: the simulation's I/O tab has the same
            // two headings and the same cards (`crate::io_cards`).
            for (head, dir, tip, id) in [
                (
                    "Inputs",
                    "in",
                    "What the box reads from its devices.",
                    "box-io-in",
                ),
                (
                    "Outputs",
                    "out",
                    "What the box writes to its devices.",
                    "box-io-out",
                ),
            ] {
                let input = dir == "in";
                // The box's signals of this direction; before them, among
                // the outputs, every output of the program (a signal of
                // the map or not), valued from the scan when no signal is.
                let of_dir = || signals.iter().filter(|s| s.dir == dir);
                let mut rows: Vec<(String, Option<&crate::remote::SignalRow>)> = Vec::new();
                if !input {
                    for c in cards.iter().flatten() {
                        let sig = of_dir().find(|s| s.name.eq_ignore_ascii_case(&c.name));
                        rows.push((
                            sig.map_or_else(|| c.name.to_ascii_lowercase(), |s| s.name.clone()),
                            sig,
                        ));
                    }
                }
                for s in of_dir() {
                    if !rows.iter().any(|(n, _)| n.eq_ignore_ascii_case(&s.name)) {
                        rows.push((s.name.clone(), Some(s)));
                    }
                }
                crate::io_cards::heading(ui, chrome, head, rows.len(), input, tip, input);
                if if input {
                    chrome.io_fold_in
                } else {
                    chrome.io_fold_out
                } {
                    continue;
                }
                if rows.is_empty() {
                    ui.label(RichText::new("none").color(t.muted)).tip(tip);
                }
                let section = ui.vertical(|ui| {
                    crate::io_cards::lay(
                        ui,
                        id,
                        rows.len(),
                        chrome.io_grid,
                        &|k| is_open(&rows[k].0),
                        &mut |ui, k, form| {
                            let (name, sig) = (&rows[k].0, rows[k].1);
                            let value = match sig {
                                Some(s) => s.value,
                                None => scan.and_then(|s| s.get(&name.to_ascii_uppercase())),
                            };
                            let unit = sig
                                .and_then(|s| s.unit.as_deref())
                                .map_or(String::new(), |u| format!(" {u}"));
                            let is_forced = sig.is_some_and(|s| s.forced);
                            let forced = if is_forced { "  FORCED" } else { "" };
                            let text = format!("{name} = {}{unit}{forced}", show(value));
                            let on = value == Some(helmsman_iec::Value::Bool(true));
                            let color = match sig.map(|s| s.quality.as_str()) {
                                _ if is_forced => t.ink(WARN),
                                Some("missing") => t.ink(RED),
                                _ if on && !input => t.ink(crate::lamps::colour(name).unwrap_or(GREEN)),
                                _ if on => t.ink(GREEN),
                                _ => t.fg,
                            };
                            let bool_ty = matches!(value, Some(helmsman_iec::Value::Bool(_)));
                            let glyph = crate::glyph_lib::for_point(name, input, bool_ty);
                            // A signal the open project's program has as
                            // a port: the map says where it is, and the
                            // diagram what it drives.
                            let is_port = menus.iter().any(|m| m.port.eq_ignore_ascii_case(name));
                            let place = if is_port {
                                crate::assign::place_of(&map, name)
                            } else {
                                crate::assign::Place::Simulated
                            };
                            let mut said = if is_port {
                                place.describe()
                            } else {
                                tip.to_string()
                            };
                            if let Some(e) = sig.and_then(|s| s.point_error.as_ref()) {
                                said.push('\n');
                                said.push_str(e);
                            }
                            let line = wired
                                .get(&name.to_ascii_lowercase())
                                .map(|w| crate::wired::line(w, known.get(w)));
                            let card = cards
                                .as_ref()
                                .and_then(|c| c.iter().find(|c| c.name.eq_ignore_ascii_case(name)));
                            let links = if input {
                                Links::Input(
                                    cards
                                        .as_ref()
                                        .filter(|_| is_port)
                                        .map(|c| crate::io_cards::affects(c, name)),
                                )
                            } else {
                                Links::Output(card)
                            };
                            let open = is_open(name);
                            let row = Row {
                                key: name,
                                input,
                                place: &said,
                                real: crate::io_cards::real(line, &place),
                                links,
                                selected: (!input && lit.eq_ignore_ascii_case(name))
                                    || (form == Form::Tile && open),
                                open: form == Form::Detail || (form == Form::Row && open),
                                tile: form == Form::Tile,
                                uses: src.as_ref().map(|c| crate::find::names(c, name).len()),
                                simulate: false,
                                designation: crate::assign::designate(&map, name, is_port),
                                trended: pinned.iter().any(|p| p.eq_ignore_ascii_case(name)),
                            };
                            let sim_input = input && engineer && sig.is_some_and(|s| s.sim);
                            let (a, card, control) = crate::io_cards::show(ui, chrome, &row, |ui| {
                                crate::io_cards::value(
                                    ui,
                                    (form != Form::Tile).then_some(glyph),
                                    color,
                                    &text,
                                    form != Form::Tile,
                                );
                                sim_input.then(|| sim_control(ui, value)).flatten()
                            });
                            if let Some((rect, set)) = control {
                                chrome.hit(format!("sim {name}"), rect);
                                if let Some(v) = set {
                                    sets.push((name.clone(), v));
                                }
                            }
                            crate::assign_ui::row_menu(&card, chrome, &menus, name, &mut asks, &mut chat);
                            asked.push((name.clone(), input, a));
                        },
                    );
                });
                if !input && !rows.is_empty() {
                    chrome.hit("io outputs", section.response.rect);
                }
                ui.add_space(6.0);
            }
        });
        let now = ui.input(|i| i.time);
        for (name, input, a) in asked {
            self.io_asked(&name, input, a, chrome, now);
        }
        for (name, v) in sets {
            self.force(&name, v);
        }
        for name in chat.asked {
            self.chat_about_signal(&name, chrome);
        }
        if let Some(name) = chat.rename.pop() {
            let files = self.agent_files().map(|(_, _, f)| f).unwrap_or_default();
            let name = crate::by_hand::spelled(&files, &name);
            self.devices_ui.renaming = Some((name.clone(), name));
        }
        for (port, ask) in asks {
            self.assign_ask(port, ask);
        }
    }

    fn diagram(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome) {
        let t = theme(ui);
        self.banner(ui, chrome);
        self.assign_context();
        let scan = live_scan(&self.state);
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
        let menus = self.devices_ui.port_menus(&self.assign_write());
        match &mut self.view {
            Some(v) if v.drawable() && self.fresh.is_none() => {
                v.icons.update(icons.as_deref());
                v.assign = menus;
                v.menu_open = self.dmenu.is_open();
                // The block's own lines are placed (or refused with a notice) by the chat item.
                v.chat_ok = true;
                let outer = vec![
                    "This box".to_string(),
                    self.state.selected.clone().unwrap_or_default(),
                ];
                if v.crumbs_ui(ui, &outer).is_some() {
                    v.open(0);
                }
                area = ui.available_rect_before_wrap();
                v.canvas(ui, scan);
                reads = v.reads_on_screen();
                // The force buttons over the Read blocks take the right-click first.
                v.secondary_on_controls(ui, &reads);
                if let Some(g) = v.gesture.take() {
                    self.dmenu.open(v, g);
                }
                self.dmenu.chat_full = self.instruct.chips.len() >= crate::chat_about::MAX_CHIPS;
                self.dmenu.show(ui.ctx(), chrome, area);
                if v.pick != self.last_pick {
                    self.last_pick = v.pick;
                    if let Some((line, _)) = v.picked() {
                        jump = Some((projects::file_line(&v.src, line), line));
                    }
                }
            }
            _ => {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(self.waiting()).color(t.muted)));
            }
        }
        self.sync_focus();
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
        let asked = self.view.as_mut().and_then(|v| v.edit_ask.take());
        if let Some(t) = asked
            .and_then(|n| self.by_hand_target(n))
            .or_else(|| self.dmenu.take_edit())
        {
            self.by_hand_ask(t);
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

/// Where a line of the diagram built from `src` is in `files`: its place
/// in the project when that is the one running, else (a line of the one
/// program file the box runs as it is) in that file.
fn box_locate<'a>(
    src: &'a str,
    files: &'a crate::scope::Files,
    running: bool,
) -> impl Fn(u32) -> Option<(String, u32)> + 'a {
    let single = move || {
        let mut same = files.iter().filter(|(f, t)| {
            helmsman_project::file_name(f) == Some(helmsman_project::FileKind::St)
                && !helmsman_iec::spec::is_test(f)
                && t.trim_end() == src.trim_end()
        });
        match (same.next(), same.next()) {
            (Some((f, _)), None) => Some(f.clone()),
            _ => None,
        }
    };
    move |l: u32| match projects::file_line(src, l) {
        Some(at) => running.then_some(at),
        None => single().map(|f| (f, l)),
    }
}

/// What the Agent tab's Deploy row shows ([`BoxView::agent_deploy`]).
struct AgentDeploy {
    ready: Result<(), String>,
    /// Saved changes that are not running yet: the button pulses.
    pending: bool,
    /// Deploy sends to the Skipper this page watches.
    to_node: bool,
    /// A deploy under way: what it is doing.
    busy: Option<&'static str>,
}

/// The Agent tab's Deploy (Nick, 2026-11-24: "there needs to be a deploy
/// button on the agent tab as well ... make it pulse or something to
/// indicate that it's required"): the Code tab's yellow arrow, with a ring
/// that pulses while what the agent (or anyone) saved is not running; the
/// pulse is the whole reminder (Nick, 2026-11-26: no line beside it).
/// Whether it was pressed.
fn deploy_row(ui: &mut egui::Ui, chrome: &mut Chrome, s: &AgentDeploy) -> bool {
    let where_to = if s.to_node { "the Skipper" } else { "the box" };
    let tip = match &s.ready {
        Ok(()) => format!(
            "Deploy: checks this version, replays it in the simulation if the policy asks, and runs it on {where_to}."
        ),
        Err(why) => why.clone(),
    };
    let pulse = s.pending && s.ready.is_ok();
    ui.add_space(4.0);
    // At the tab's right edge, as Send is (Nick, 2026-11-24).
    // One row of one button's height: a bare right-to-left layout takes
    // all the height below it and pushed the rest of the tab off the end.
    let r = ui
        .allocate_ui_with_layout(
            Vec2::new(ui.available_width(), crate::glyphs::ROW),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let r = crate::glyphs::deploy(ui, s.ready.is_ok()).tip(tip);
                if let Some(stage) = s.busy {
                    crate::editor::busy(ui, chrome, stage, "agent deploying");
                }
                r
            },
        )
        .inner;
    chrome.hit("agent deploy", r.rect);
    if pulse {
        // A ring that breathes about once a second, and nothing else
        // moves: the eye finds it without the page flashing.
        let time = ui.input(|i| i.time);
        #[allow(clippy::cast_possible_truncation)]
        let phase = ((time * std::f64::consts::TAU).sin() * 0.5 + 0.5) as f32;
        ui.painter().rect_stroke(
            r.rect.expand(1.0 + 4.0 * phase),
            egui::CornerRadius::same(7),
            egui::Stroke::new(
                2.0,
                crate::st_view::brand::YELLOW.gamma_multiply(1.0 - 0.7 * phase),
            ),
            egui::StrokeKind::Outside,
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    r.clicked()
}

/// What the diagram and the I/O tab read values from: the module's trace
/// on a box, the I/O points' values on a Skipper, which keeps no trace.
fn live_scan(state: &crate::remote::BoxState) -> Option<&dyn Scan> {
    state
        .scan
        .as_ref()
        .map(|s| s as &dyn Scan)
        .or_else(|| state.node_values.as_ref().map(|n| n as &dyn Scan))
}
