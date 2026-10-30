//! The floor screen (`/?andon`): a full-screen andon view of the box that
//! served the page, readable from across a floor. Its tiles are output
//! signals the box drives (`screen` bindings in the I/O map); how they look
//! is the box's layout (`GET /api/screen`, parsed with the box's own
//! parser, [`helmsman_plc::screen`]); their values come from the same
//! `/api/events` stream the live view watches.
//!
//! [`AndonState`] decides what to ask for and what is known, with no I/O;
//! [`tiles`] turns a layout and the signals into what each tile shows, as
//! data. Both run (and are tested) natively.
//!
//! **Stale data never shows as a colour.** Once the stream fails, or no
//! values event arrives for [`STALE_AFTER`] seconds (the box publishes
//! several a second), every tile shows "NO DATA" on black and a banner
//! says why: the last known green is never left on screen.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};
use helmsman_iec::Value;
use helmsman_plc::screen::{self, Colour, Layout};

use crate::remote::{Cmd, Fetch, Msg, Retry, SignalRow, Stream, UNREACHABLE_AFTER, parse, rows};
use crate::st_view::brand;

/// Seconds without a values event after which what is on screen is stale.
pub const STALE_AFTER: f64 = 3.0;
/// Seconds without one after which the stream is reopened: a peer that
/// vanished without closing (a pulled cable) may never fail it.
pub const REOPEN_AFTER: f64 = 10.0;

const EVENTS: &str = "/api/events";
const SCREEN: &str = "/api/screen";

/// What a tile shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Face {
    /// A state the layout names: its colour and message.
    Known { colour: Colour, text: String },
    /// A state code the layout does not name.
    Unknown { code: i64 },
    /// Nothing current to show: the box is unreachable, the data stale, or
    /// the signal missing.
    NoData,
}

/// One tile, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileView {
    pub id: String,
    pub title: String,
    /// Column, row, width, height, in grid cells.
    pub cell: [u8; 4],
    pub face: Face,
    /// The tile's number and label (`1234 parts today`), when current.
    pub value: Option<String>,
}

/// A state code from a signal value: a `BOOL` is 0 or 1.
fn code(v: Option<Value>) -> Option<i64> {
    match v? {
        Value::Bool(b) => Some(i64::from(b)),
        Value::Int(n) => i64::try_from(n).ok(),
        Value::Real(_) => None,
    }
}

fn number(v: Value) -> Option<String> {
    match v {
        Value::Int(n) => Some(n.to_string()),
        Value::Real(x) if x.is_finite() => Some(crate::look::fmt_num(x)),
        _ => None,
    }
}

/// What each tile of `layout` shows, given the box's `signals`. With
/// `fresh` false (unreachable or stale), every tile is [`Face::NoData`]
/// whatever the signals say.
#[must_use]
pub fn tiles(layout: &Layout, signals: &HashMap<String, SignalRow>, fresh: bool) -> Vec<TileView> {
    let value = |name: &str| {
        signals
            .get(name)
            .filter(|s| fresh && s.quality != "missing")
            .and_then(|s| s.value)
    };
    layout
        .tiles
        .iter()
        .map(|t| {
            let face = match code(value(&t.state)) {
                None => Face::NoData,
                Some(c) => match layout.state(t, c) {
                    Some(s) => Face::Known {
                        colour: s.colour,
                        text: s.text.clone(),
                    },
                    None => Face::Unknown { code: c },
                },
            };
            TileView {
                id: t.id.clone(),
                title: t.title.clone(),
                cell: [t.col, t.row, t.w, t.h],
                face,
                value: t
                    .value
                    .as_ref()
                    .and_then(|v| Some(format!("{} {}", number(value(&v.signal)?)?, v.label))),
            }
        })
        .collect()
}

/// The shift line, from the calendar's signals, if the box has them.
#[must_use]
pub fn shift(signals: &HashMap<String, SignalRow>, fresh: bool) -> Option<String> {
    if !fresh {
        return None;
    }
    let get = |n: &str| {
        signals
            .get(n)
            .filter(|s| s.quality != "missing")
            .and_then(|s| s.value)
    };
    if get("planned_down") == Some(Value::Bool(true)) {
        return Some("PLANNED DOWNTIME".into());
    }
    match (get("in_shift")?, get("shift_id")) {
        (Value::Bool(true), id) => {
            let mut s = match id {
                Some(Value::Int(n)) => format!("SHIFT {n}"),
                _ => "IN SHIFT".into(),
            };
            if get("on_break") == Some(Value::Bool(true)) {
                s.push_str(" · BREAK");
            }
            Some(s)
        }
        _ => Some("NO SHIFT".into()),
    }
}

/// The floor screen's knowledge of the box.
#[derive(Debug)]
pub struct AndonState {
    pub layout: Option<Layout>,
    /// Why there is no layout to draw.
    pub problem: Option<String>,
    pub signals: HashMap<String, SignalRow>,
    /// Page time of the last values event.
    last: Option<f64>,
    /// Page time the stream was last reopened for going quiet.
    kicked: Option<f64>,
    connected: bool,
    values: Retry,
    screen: Retry,
    queue: Vec<Cmd>,
}

impl Default for AndonState {
    fn default() -> Self {
        Self::new()
    }
}

/// How the floor screen is doing at reaching the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Health {
    /// Values arriving.
    Live,
    /// Nothing yet.
    Connecting,
    /// Connected, but nothing new for this many seconds.
    Stale(f64),
    /// The stream failed (this many times in a row); retrying.
    Lost(u32),
}

impl AndonState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            layout: None,
            problem: None,
            signals: HashMap::new(),
            last: None,
            kicked: None,
            connected: false,
            values: Retry::default(),
            screen: Retry::default(),
            queue: vec![
                Cmd::Get(Fetch::Screen, SCREEN.into()),
                Cmd::Listen(Stream::Values, EVENTS.into()),
            ],
        }
    }

    /// What to do now: queued commands, retries that are due, and a
    /// reopened stream when it went quiet.
    pub fn poll(&mut self, now: f64) -> Vec<Cmd> {
        if self.values.due(now) {
            self.queue.push(Cmd::Listen(Stream::Values, EVENTS.into()));
        }
        let quiet = self.connected && self.last.or(self.kicked).is_some_and(|t| now - t > REOPEN_AFTER);
        if quiet && self.kicked.is_none_or(|k| now - k > REOPEN_AFTER) {
            self.kicked = Some(now);
            self.queue.push(Cmd::Listen(Stream::Values, EVENTS.into()));
        }
        if self.screen.due(now) {
            self.queue.push(Cmd::Get(Fetch::Screen, SCREEN.into()));
        }
        std::mem::take(&mut self.queue)
    }

    /// The soonest retry.
    #[must_use]
    pub fn next_retry(&self) -> Option<f64> {
        [self.values.at, self.screen.at]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    #[must_use]
    pub fn health(&self, now: f64) -> Health {
        match (self.connected, self.last) {
            (false, _) if self.values.failures > 0 => Health::Lost(self.values.failures),
            (false, _) | (true, None) => Health::Connecting,
            (true, Some(t)) if now - t > STALE_AFTER => Health::Stale(now - t),
            (true, Some(_)) => Health::Live,
        }
    }

    /// Whether what the signals say is current.
    #[must_use]
    pub fn fresh(&self, now: f64) -> bool {
        self.health(now) == Health::Live
    }

    /// What every tile shows at `now`.
    #[must_use]
    pub fn tiles(&self, now: f64) -> Vec<TileView> {
        self.layout
            .as_ref()
            .map_or_else(Vec::new, |l| tiles(l, &self.signals, self.fresh(now)))
    }

    /// The banner across the top, when the screen is not live.
    #[must_use]
    pub fn banner(&self, now: f64) -> Option<String> {
        match self.health(now) {
            Health::Live => None,
            Health::Connecting => Some("CONNECTING TO THE BOX…".into()),
            Health::Stale(s) => Some(format!("NO DATA FROM THE BOX FOR {}s", s.floor())),
            Health::Lost(n) if n >= UNREACHABLE_AFTER => Some("BOX UNREACHABLE · RETRYING".into()),
            Health::Lost(_) => Some("CONNECTION LOST · RECONNECTING".into()),
        }
    }

    /// Takes in what the transport saw at page time `now`.
    pub fn apply(&mut self, msg: Msg, now: f64) {
        match msg {
            Msg::Got(Fetch::Screen, r) => {
                let parsed = match r {
                    Ok(body) => match parse(&body).as_ref().and_then(|j| j["source"].as_str()) {
                        Some(src) => screen::parse(src).map_err(|errs| {
                            let first = errs.first().map(ToString::to_string).unwrap_or_default();
                            format!("The box's screen layout does not parse: {first}")
                        }),
                        None => Err("The box answered /api/screen with something else.".into()),
                    },
                    Err(e) if e == "HTTP 404" => {
                        Err("This box has no floor screen: start it with --screen FILE.".into())
                    }
                    Err(e) => Err(format!("Cannot reach the box ({e}).")),
                };
                match parsed {
                    Ok(l) => {
                        self.screen.ok();
                        self.layout = Some(l);
                        self.problem = None;
                    }
                    Err(why) => {
                        self.screen.failed(now);
                        if self.layout.is_none() {
                            self.problem = Some(why);
                        }
                    }
                }
            }
            Msg::Got(..)
            | Msg::Opened(Stream::Trace)
            | Msg::Failed(Stream::Trace)
            | Msg::Event(Stream::Trace, ..) => {}
            Msg::Opened(Stream::Values) => {
                self.values.ok();
                self.connected = true;
            }
            Msg::Failed(Stream::Values) => {
                self.connected = false;
                // Nothing known survives a lost stream.
                self.signals.clear();
                self.last = None;
                self.values.failed(now);
            }
            Msg::Event(Stream::Values, name, data) => {
                let Some(j) = (name == "values").then(|| parse(&data)).flatten() else {
                    return;
                };
                let Some(list) = rows(&j["signals"], SignalRow::parse) else {
                    return;
                };
                self.values.ok();
                self.connected = true;
                self.last = Some(now);
                self.signals = list.into_iter().map(|s| (s.name.clone(), s)).collect();
            }
        }
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// The full-screen view.
pub struct AndonView {
    pub state: AndonState,
    #[cfg(target_arch = "wasm32")]
    driver: crate::web::Driver,
    /// Commands not carried out (no transport here: tests read them).
    #[cfg(not(target_arch = "wasm32"))]
    pub sent: Vec<Cmd>,
}

impl Default for AndonView {
    fn default() -> Self {
        Self::new()
    }
}

/// The smallest font drawn.
const MIN_FONT: f32 = 6.0;

/// Largest font size, at most `max` (and at least [`MIN_FONT`]), at which `text` fits `width`.
fn fit(ui: &egui::Ui, text: &str, max: f32, width: f32) -> f32 {
    // Never below a legible floor, never zero or negative (a tiny window).
    let max = max.max(MIN_FONT);
    let w = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(max), Color32::WHITE)
        .size()
        .x;
    if w <= width || w <= 0.0 {
        max
    } else {
        (max * width / w).max(MIN_FONT)
    }
}

impl AndonView {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AndonState::new(),
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
        // Repaint at least once a second, so staleness shows without input.
        let next = self
            .state
            .next_retry()
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

    /// One frame: the whole window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let msgs = self.inbox();
        self.step(&ctx, msgs, now);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(brand::BLACK))
            .show(ui, |ui| self.draw(ui, now));
    }

    fn draw(&self, ui: &mut egui::Ui, now: f64) {
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        let s = &self.state;
        let unit = (full.height() / 12.0).clamp(16.0, 96.0);

        // Header: title left, shift right, both in brand yellow on black.
        let header = Rect::from_min_size(full.min, Vec2::new(full.width(), unit * 1.4));
        let pad = unit * 0.4;
        let title = s
            .layout
            .as_ref()
            .and_then(|l| l.title.clone())
            .unwrap_or_else(|| "HELMSMAN".into());
        let size = fit(ui, &title, unit, header.width() * 0.6);
        painter.text(
            Pos2::new(header.min.x + pad, header.center().y),
            Align2::LEFT_CENTER,
            &title,
            FontId::proportional(size),
            brand::YELLOW,
        );
        if let Some(shift) = shift(&s.signals, s.fresh(now)) {
            painter.text(
                Pos2::new(header.max.x - pad, header.center().y),
                Align2::RIGHT_CENTER,
                shift,
                FontId::proportional(unit * 0.8),
                brand::WHITE,
            );
        }
        let mut body = Rect::from_min_max(Pos2::new(full.min.x, header.max.y), full.max);

        // The banner: not live is said in full width, in red.
        if let Some(b) = s.banner(now) {
            let r = Rect::from_min_size(body.min, Vec2::new(body.width(), unit * 1.2));
            painter.rect_filled(r, 0.0, rgb(Colour::Red.fill()));
            let size = fit(ui, &b, unit * 0.8, r.width() - 2.0 * pad);
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                b,
                FontId::proportional(size),
                Color32::WHITE,
            );
            body.min.y = r.max.y;
        }
        let body = body.shrink(pad * 0.5);

        let Some(layout) = &s.layout else {
            let why = s
                .problem
                .clone()
                .unwrap_or_else(|| "Loading the floor screen…".into());
            let size = fit(ui, &why, unit * 0.6, body.width() * 0.9);
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                why,
                FontId::proportional(size),
                brand::STEEL,
            );
            return;
        };
        let cw = body.width() / f32::from(layout.cols);
        let rh = body.height() / f32::from(layout.rows);
        for t in s.tiles(now) {
            let [c, r, w, h] = t.cell.map(f32::from);
            let rect = Rect::from_min_size(
                Pos2::new(body.min.x + c * cw, body.min.y + r * rh),
                Vec2::new(w * cw, h * rh),
            )
            .shrink(pad * 0.5);
            Self::tile(ui, &painter, rect, &t);
        }
    }

    fn tile(ui: &egui::Ui, painter: &egui::Painter, rect: Rect, t: &TileView) {
        let (fill, ink, message) = match &t.face {
            Face::Known { colour, text } => (rgb(colour.fill()), rgb(colour.ink()), text.clone()),
            Face::Unknown { code } => (brand::BLACK, brand::YELLOW, format!("UNKNOWN STATE {code}")),
            Face::NoData => (brand::BLACK, brand::WHITE, "NO DATA".into()),
        };
        painter.rect_filled(rect, 6.0, fill);
        if !matches!(t.face, Face::Known { .. }) {
            // Black tiles get a steel outline, so the grid still reads.
            painter.rect_stroke(rect, 6.0, Stroke::new(3.0, brand::STEEL), StrokeKind::Inside);
        }
        let inner = rect.shrink(rect.height().min(rect.width()) * 0.06);
        let line = inner.height() / 5.0;
        let width = inner.width();
        let title_size = fit(ui, &t.title, line * 0.9, width);
        painter.text(
            Pos2::new(inner.center().x, inner.min.y),
            Align2::CENTER_TOP,
            &t.title,
            FontId::proportional(title_size),
            ink,
        );
        let msg_size = fit(ui, &message, line * 1.6, width);
        painter.text(
            inner.center(),
            Align2::CENTER_CENTER,
            &message,
            FontId::proportional(msg_size),
            ink,
        );
        if let Some(v) = &t.value {
            let size = fit(ui, v, line * 0.9, width);
            painter.text(
                Pos2::new(inner.center().x, inner.max.y),
                Align2::CENTER_BOTTOM,
                v,
                FontId::proportional(size),
                ink,
            );
        }
    }
}
