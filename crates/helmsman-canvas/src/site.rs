//! The site view (`/?site`, or the "Site" tab): every box of this box's
//! site (`helmsman-plc --site NAME`), one row each, with a click-through
//! to that box's own canvas.
//!
//! The list is this box's `GET /api/site` (what it heard on the LAN). Each
//! peer's status comes from that peer's own `GET /api/site`, fetched by
//! the browser directly, cross-origin and without credentials: the box
//! never fetches for the page, so a beacon can never make a box connect
//! anywhere, and "reachable" here means reachable from this browser,
//! which is what the click-through needs.
//!
//! What a peer answers is untrusted: an answer over [`MAX_BODY`], one
//! that is not the shape, or one from a box whose id is not the one the
//! beacon gave (someone else at that address) is not shown as a status. A
//! link is built only from an address that parses as `IPv4:port`.
//!
//! [`SiteState`] decides what to fetch and what each row says, with no
//! I/O; it runs (and is tested) natively.

use std::collections::HashMap;
use std::net::SocketAddr;

use eframe::egui::{self, Color32, RichText};
use serde_json::Value as Json;

use crate::remote::{Cmd, Fetch, Msg, parse};
use crate::st_view::brand;

/// Seconds between refreshes of the list and of each peer's status.
pub const REFRESH: f64 = 5.0;
/// Seconds a peer has to answer before the probe counts as failed.
pub const PEER_TIMEOUT: f64 = 8.0;
/// Seconds after which a status is too old to show as current.
pub const STALE_AFTER: f64 = 15.0;
/// Milliseconds since its last beacon after which a peer is "silent".
pub const SILENT_MS: u64 = 15_000;
/// The largest peer answer read, in bytes.
pub const MAX_BODY: usize = 64 * 1024;
/// Longest text shown from an answer, in characters.
const MAX_TEXT: usize = 48;

const SITE: &str = "/api/site";

/// A box's status, as its `/api/site` summarizes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// The box has published a view of its modules.
    pub published: bool,
    pub modules: u64,
    pub running: u64,
    pub held: u64,
    pub faulted: u64,
    pub devices_up: u64,
    pub devices_down: u64,
}

impl Summary {
    fn parse(j: &Json) -> Option<Self> {
        let n = |v: &Json| v.as_u64();
        let d = &j["devices"];
        Some(Self {
            published: j["published"].as_bool()?,
            modules: n(&j["modules"])?,
            running: n(&j["running"])?,
            held: n(&j["held"])?,
            faulted: n(&j["faulted"])?,
            devices_up: n(&d["up"])?,
            devices_down: n(&d["down"])?,
        })
    }
}

/// Text from an answer, safe to show: printable characters, cut short.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect()
}

/// A box as the site's list names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxRow {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Its API, when the list gave an `IPv4:port` (never anything else).
    pub addr: Option<SocketAddr>,
    pub last_seen_ms: Option<u64>,
    /// Another address that claimed its id.
    pub conflict: Option<String>,
}

impl BoxRow {
    fn parse(j: &Json) -> Option<Self> {
        let addr = j["address"]
            .as_str()
            .and_then(|a| a.parse::<SocketAddr>().ok())
            .filter(|a| a.is_ipv4() && !a.ip().is_unspecified() && a.port() != 0);
        Some(Self {
            id: clean(j["id"].as_str()?),
            name: clean(j["name"].as_str()?),
            version: clean(j["version"].as_str().unwrap_or("?")),
            addr,
            last_seen_ms: j["last_seen_ms"].as_u64(),
            conflict: j["conflict"].as_str().map(clean),
        })
    }
}

/// One peer's probing.
#[derive(Debug, Clone, PartialEq)]
struct Probe {
    /// The probe in flight: its number and when it went.
    flight: Option<(u64, f64)>,
    /// When the last probe went.
    sent: Option<f64>,
    /// The last good answer and when it came.
    ok: Option<(Summary, f64)>,
    /// Failures since the last good answer, and the last one's reason.
    failures: u32,
    why: Option<String>,
}

impl Probe {
    const fn new() -> Self {
        Self {
            flight: None,
            sent: None,
            ok: None,
            failures: 0,
            why: None,
        }
    }

    fn failed(&mut self, why: String) {
        self.flight = None;
        self.failures += 1;
        self.why = Some(why);
    }
}

/// How a row reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    /// Held modules, devices down, starting, silent, or an id in conflict.
    Warn,
    /// A faulted module.
    Fault,
    /// Not answering this browser, or answering as another box.
    Unreachable,
    /// Nothing known yet.
    Checking,
}

/// One row, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub id: String,
    pub name: String,
    /// The box that served this page.
    pub this: bool,
    pub address: String,
    /// Its canvas, for the click-through.
    pub url: Option<String>,
    pub version: String,
    pub status: String,
    pub health: Health,
    /// Since its last beacon, as this box heard it.
    pub seen: String,
}

/// What the site view knows.
#[derive(Debug)]
pub struct SiteState {
    pub site: Option<String>,
    /// Why there is no list.
    pub problem: Option<String>,
    me: Option<(BoxRow, Summary)>,
    peers: Vec<BoxRow>,
    probes: HashMap<String, Probe>,
    /// When the list was last asked for, and whether it is in flight.
    list_sent: Option<f64>,
    list_flight: bool,
    n: u64,
}

impl Default for SiteState {
    fn default() -> Self {
        Self::new()
    }
}

/// Text for the status of a box with `s`.
fn status(s: &Summary) -> (String, Health) {
    if !s.published {
        return ("STARTING".into(), Health::Warn);
    }
    let mut parts = Vec::new();
    if s.faulted > 0 {
        parts.push(format!("{} FAULTED", s.faulted));
    }
    if s.held > 0 {
        parts.push(format!("{} HELD", s.held));
    }
    if s.devices_down > 0 {
        parts.push(format!(
            "{} DEVICE{} DOWN",
            s.devices_down,
            if s.devices_down == 1 { "" } else { "S" }
        ));
    }
    let health = if s.faulted > 0 {
        Health::Fault
    } else if parts.is_empty() {
        Health::Ok
    } else {
        Health::Warn
    };
    if parts.is_empty() {
        parts.push(format!("RUNNING · {} OF {} MODULES", s.running, s.modules));
    }
    (parts.join(" · "), health)
}

fn seen(ms: Option<u64>) -> String {
    match ms {
        None => "-".into(),
        Some(ms) if ms < 1000 => "just now".into(),
        Some(ms) => format!("{} s ago", ms / 1000),
    }
}

impl SiteState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            site: None,
            problem: None,
            me: None,
            peers: Vec::new(),
            probes: HashMap::new(),
            list_sent: None,
            list_flight: false,
            n: 0,
        }
    }

    /// What to fetch at page time `now`: the list every [`REFRESH`], and
    /// each listed peer every [`REFRESH`], one probe at a time; a probe
    /// unanswered for [`PEER_TIMEOUT`] has failed.
    pub fn poll(&mut self, now: f64) -> Vec<Cmd> {
        let mut out = Vec::new();
        if !self.list_flight && self.list_sent.is_none_or(|t| now - t >= REFRESH) {
            self.list_flight = true;
            self.list_sent = Some(now);
            out.push(Cmd::Get(Fetch::Site, SITE.into()));
        }
        for peer in &self.peers {
            let Some(addr) = peer.addr else { continue };
            let p = self.probes.entry(peer.id.clone()).or_insert_with(Probe::new);
            if let Some((_, at)) = p.flight {
                if now - at < PEER_TIMEOUT {
                    continue;
                }
                p.failed(format!("no answer in {PEER_TIMEOUT} s"));
            }
            if p.sent.is_some_and(|t| now - t < REFRESH) {
                continue;
            }
            self.n += 1;
            p.flight = Some((self.n, now));
            p.sent = Some(now);
            out.push(Cmd::Get(
                Fetch::Peer {
                    id: peer.id.clone(),
                    n: self.n,
                },
                format!("http://{addr}{SITE}"),
            ));
        }
        out
    }

    /// The soonest page time something is due.
    #[must_use]
    pub fn next_due(&self) -> Option<f64> {
        let list = self.list_sent.map(|t| t + REFRESH);
        let probes = self.probes.values().filter_map(|p| match p.flight {
            Some((_, at)) => Some(at + PEER_TIMEOUT),
            None => p.sent.map(|t| t + REFRESH),
        });
        list.into_iter().chain(probes).reduce(f64::min)
    }

    /// Takes in what the transport saw at page time `now`.
    pub fn apply(&mut self, msg: Msg, now: f64) {
        match msg {
            Msg::Got(Fetch::Site, r) => {
                self.list_flight = false;
                self.take_list(r);
            }
            Msg::Got(Fetch::Peer { id, n }, r) => self.take_probe(&id, n, r, now),
            _ => {}
        }
    }

    fn take_list(&mut self, r: Result<String, String>) {
        let j = match r {
            Ok(body) => parse(&body),
            Err(e) if e == "HTTP 404" => {
                self.problem = Some("This box is in no site: start it with --site NAME.".into());
                return;
            }
            Err(e) => {
                self.problem = Some(format!("Cannot reach this box ({e})."));
                return;
            }
        };
        let parsed = j.as_ref().and_then(|j| {
            let me = BoxRow::parse(&j["self"])?;
            let summary = Summary::parse(&j["self"]["status"])?;
            let peers = j["peers"].as_array()?.iter().filter_map(BoxRow::parse).collect();
            Some((j["site"].as_str().map(clean), me, summary, peers))
        });
        let Some((site, me, summary, peers)) = parsed else {
            self.problem = Some("This box answered /api/site with something else.".into());
            return;
        };
        let peers: Vec<BoxRow> = peers;
        self.probes.retain(|id, _| peers.iter().any(|p| &p.id == id));
        self.site = site;
        self.me = Some((me, summary));
        self.peers = peers;
        self.problem = None;
    }

    fn take_probe(&mut self, id: &str, n: u64, r: Result<String, String>, now: f64) {
        let Some(p) = self.probes.get_mut(id) else { return };
        if p.flight.map(|(k, _)| k) != Some(n) {
            return;
        }
        let body = match r {
            Ok(b) if b.len() > MAX_BODY => return p.failed("answer too large".into()),
            Ok(b) => b,
            Err(e) => return p.failed(clean(&e)),
        };
        let Some(j) = parse(&body) else {
            return p.failed("not a box's answer".into());
        };
        match j["self"]["id"].as_str() {
            Some(got) if got == id => {}
            Some(got) => return p.failed(format!("answered as box {}", clean(got))),
            None => return p.failed("not a box's answer".into()),
        }
        let Some(summary) = Summary::parse(&j["self"]["status"]) else {
            return p.failed("not a box's answer".into());
        };
        p.flight = None;
        p.ok = Some((summary, now));
        p.failures = 0;
        p.why = None;
    }

    /// Every row at `now`: this box first, then its peers by name.
    #[must_use]
    pub fn lines(&self, now: f64) -> Vec<Line> {
        let mut out = Vec::new();
        if let Some((me, summary)) = &self.me {
            let (status, health) = status(summary);
            out.push(Line {
                id: me.id.clone(),
                name: me.name.clone(),
                this: true,
                address: "this box".into(),
                url: None,
                version: me.version.clone(),
                status,
                health,
                seen: "-".into(),
            });
        }
        let mut peers: Vec<&BoxRow> = self.peers.iter().collect();
        peers.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        for peer in peers {
            out.push(self.line(peer, now));
        }
        out
    }

    fn line(&self, peer: &BoxRow, now: f64) -> Line {
        let probe = self.probes.get(&peer.id);
        let (mut status, mut health) = match (peer.addr, probe) {
            (None, _) => ("NO USABLE ADDRESS".into(), Health::Unreachable),
            (
                Some(_),
                Some(Probe {
                    ok: Some((s, at)),
                    failures: 0,
                    ..
                }),
            ) if now - at <= STALE_AFTER => status(s),
            (
                Some(_),
                Some(Probe {
                    failures,
                    why: Some(why),
                    ..
                }),
            ) if *failures > 0 => (format!("UNREACHABLE · {why}"), Health::Unreachable),
            (
                Some(_),
                Some(Probe {
                    ok: Some((_, at)), ..
                }),
            ) => (
                format!("NO ANSWER FOR {} s", (now - at).floor()),
                Health::Unreachable,
            ),
            (Some(_), _) => ("CHECKING…".into(), Health::Checking),
        };
        if peer.last_seen_ms.is_some_and(|ms| ms > SILENT_MS) {
            status.push_str(" · BEACON SILENT");
            if health == Health::Ok {
                health = Health::Warn;
            }
        }
        if let Some(other) = &peer.conflict {
            status = format!("ID CONFLICT (also {other}) · {status}");
            if matches!(health, Health::Ok | Health::Checking) {
                health = Health::Warn;
            }
        }
        Line {
            id: peer.id.clone(),
            name: peer.name.clone(),
            this: false,
            address: peer.addr.map_or_else(|| "?".into(), |a| a.to_string()),
            url: peer.addr.map(|a| format!("http://{a}/")),
            version: peer.version.clone(),
            status,
            health,
            seen: seen(peer.last_seen_ms),
        }
    }
}

/// The colour a row's status is drawn in.
#[must_use]
pub const fn colour(h: Health) -> Color32 {
    match h {
        Health::Ok => Color32::from_rgb(0x2E, 0xB8, 0x4B),
        Health::Warn => brand::YELLOW,
        Health::Fault | Health::Unreachable => Color32::from_rgb(0xE5, 0x3B, 0x2E),
        Health::Checking => brand::STEEL,
    }
}

/// The view.
pub struct SiteView {
    pub state: SiteState,
    #[cfg(target_arch = "wasm32")]
    driver: crate::web::Driver,
    /// Commands not carried out (no transport here: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<Cmd>,
}

impl Default for SiteView {
    fn default() -> Self {
        Self::new()
    }
}

impl SiteView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: SiteState::new(),
            #[cfg(target_arch = "wasm32")]
            driver: crate::web::Driver::default(),
            #[cfg(not(target_arch = "wasm32"))]
            sent: Vec::new(),
        }
    }

    /// Takes in `msgs` at page time `now`, then carries out what is due.
    pub fn step(&mut self, ctx: &egui::Context, msgs: Vec<Msg>, now: f64) {
        for m in msgs {
            self.state.apply(m, now);
        }
        let cmds = self.state.poll(now);
        self.run(ctx, cmds);
        let next = self
            .state
            .next_due()
            .map_or(1.0, |at| (at - now).clamp(0.05, 1.0));
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(next));
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
    fn inbox(&mut self) -> Vec<Msg> {
        self.driver.drain()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::unused_self)]
    fn inbox(&mut self) -> Vec<Msg> {
        Vec::new()
    }

    /// One frame, in the space below the top bar.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let msgs = self.inbox();
        self.step(&ctx, msgs, now);
        egui::CentralPanel::default().show(ui, |ui| self.draw(ui, now));
    }

    fn draw(&self, ui: &mut egui::Ui, now: f64) {
        let s = &self.state;
        ui.horizontal(|ui| {
            ui.label(RichText::new("SITE").strong().color(brand::YELLOW));
            ui.label(
                RichText::new(s.site.as_deref().unwrap_or("…"))
                    .strong()
                    .size(20.0),
            );
        });
        if let Some(p) = &s.problem {
            ui.label(RichText::new(p).color(brand::YELLOW));
            if s.me.is_none() {
                return;
            }
        }
        let lines = s.lines(now);
        ui.label(
            RichText::new(format!(
                "{} box{} · status read by this browser from each box · click a name to open its canvas",
                lines.len(),
                if lines.len() == 1 { "" } else { "es" }
            ))
            .color(brand::STEEL),
        );
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("site-boxes")
                .striped(true)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    for h in ["BOX", "ADDRESS", "STATUS", "VERSION", "BEACON"] {
                        ui.label(RichText::new(h).color(brand::STEEL).small());
                    }
                    ui.end_row();
                    for l in &lines {
                        if let Some(url) = &l.url {
                            ui.hyperlink_to(RichText::new(&l.name).strong(), url)
                                .on_hover_text(format!("open {} (id {})", url, l.id));
                        } else {
                            let name = if l.this {
                                format!("{} (this box)", l.name)
                            } else {
                                l.name.clone()
                            };
                            ui.label(RichText::new(name).strong());
                        }
                        ui.label(&l.address);
                        ui.label(RichText::new(&l.status).strong().color(colour(l.health)));
                        ui.label(RichText::new(&l.version).color(brand::STEEL));
                        ui.label(RichText::new(&l.seen).color(brand::STEEL));
                        ui.end_row();
                    }
                });
        });
    }
}
