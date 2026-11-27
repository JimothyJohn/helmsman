//! The Trend tab on a box (card `box-trend-source`): pinned signals drawn
//! from the box's own history (`GET /api/trends`), so the last minutes are
//! there on open, then extended by every values event.
//!
//! - **Pins** are the signed-in person's, kept by the box (`GET`/`PUT
//!   /api/trends/pins`), so they follow the person to any screen; signed
//!   out, this browser keeps them ([`Storage`], every access wrapped).
//! - **History** is asked for on open, whenever the pins change, and again
//!   every [`RENEW_S`] (the ask keeps the signals tracked on the box for a
//!   day). Between asks, values events extend it. At most [`WINDOW_MS`]
//!   and [`MAX_POINTS`] points a signal are drawn, as the box keeps.

use std::collections::BTreeMap;

use eframe::egui::{self, CornerRadius, FontId, RichText, Sense};
use helmsman_iec::Value;
use serde_json::{Value as Json, json};

use crate::box_projects::{Answer, PCall, PReq};
use crate::editor::Chrome;
use crate::glyphs::Icon;
use crate::playground::nice_range;
use crate::playground_view::{lane_handles, window_slider};
use crate::remote::{BoxState, segment};
use crate::review::Session;
use crate::sim_projects::Storage;
use crate::theme::theme;
use crate::tips::Tip;

/// Signals one trend draws at most (as in the simulation).
pub const PINS: usize = crate::playground::TREND_SIGNALS;
/// History drawn, ms (the box keeps 10 min).
pub const WINDOW_MS: u64 = 600_000;
/// The widths the chart can show of it: 10 s, 100 s, the whole 10 min.
/// Points a signal keeps here.
pub const MAX_POINTS: usize = 2400;
/// Seconds between history asks while the tab is open.
pub const RENEW_S: f64 = 600.0;
/// How long, page seconds, a variable a change added is waited for: the
/// change is saved first and runs once it is deployed.
pub const EXPECT_S: f64 = 1800.0;

/// The width of the box column left of the plot.
const CHECK_W: f32 = 28.0;

/// Whose pins these are: a person signed in (their label), or the browser.
fn owner(s: Option<&Session>) -> Option<String> {
    s.map(|s| s.user.clone().unwrap_or_else(|| format!("{} (PIN)", s.role)))
}

/// A value as a number to draw (BOOL as 0/1).
#[allow(clippy::cast_precision_loss)]
fn number(v: Value) -> f64 {
    match v {
        Value::Bool(b) => f64::from(u8::from(b)),
        Value::Int(n) => n as f64,
        Value::Real(r) => r,
    }
}

fn number_json(j: &Json) -> f64 {
    match j {
        Json::Bool(b) => f64::from(u8::from(*b)),
        Json::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Json::String(s) => s.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

/// The Trend tab's state on a box.
pub struct BoxTrend {
    pub pins: Vec<String>,
    /// Per pin: (box time ms, value) points, oldest first.
    pub series: BTreeMap<String, Vec<(u64, f64)>>,
    /// The box's clock at the newest point.
    pub now_ms: u64,
    /// The width of the window drawn, ms (the slider's range).
    pub window_ms: u64,
    /// What the box said about the last ask or save.
    pub notice: Option<String>,
    /// Why this browser does not keep pins, if it does not.
    pub problem: Option<String>,
    /// The signals are another node's (a page watching a Skipper by
    /// address): the box behind the page does not know their names and
    /// refuses them as pins (422), so this browser keeps them.
    pub local_pins: bool,
    /// The pins nobody chose, oldest first ([`crate::ekg::pin_auto`]).
    auto: Vec<String>,
    /// The lanes that just joined, popping.
    flashes: crate::ekg::Flashes,
    /// Variables a described change added to the open program, waiting to
    /// be pinned once the box reports them (`(name, page time first
    /// looked for)`): a change is saved before it is deployed.
    expecting: Vec<(String, Option<f64>)>,
    /// The signal groups (a device's name) opened in the picker.
    pub picker: crate::signal_picker::Picker,
    /// The pins ticked in the box left of the plot, lower case.
    pub checked: std::collections::BTreeSet<String>,
    store: Box<dyn Storage>,
    /// The owner the pins were loaded for (`""`: the browser).
    loaded: Option<String>,
    /// Page time of the last history ask.
    asked: Option<f64>,
    seq: Option<u64>,
    /// The box's restarts seen so far ([`BoxState::epoch`]).
    epoch: u64,
    queue: Vec<PReq>,
}

impl std::fmt::Debug for BoxTrend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxTrend")
            .field("pins", &self.pins)
            .field("loaded", &self.loaded)
            .finish_non_exhaustive()
    }
}

impl BoxTrend {
    /// The tab, the browser's pins kept in `store`.
    pub fn new(store: Box<dyn Storage>) -> Self {
        Self {
            pins: Vec::new(),
            series: BTreeMap::new(),
            now_ms: 0,
            window_ms: crate::playground::TREND_MS,
            notice: None,
            problem: None,
            local_pins: false,
            auto: Vec::new(),
            flashes: crate::ekg::Flashes::default(),
            expecting: Vec::new(),
            picker: crate::signal_picker::Picker::default(),
            checked: std::collections::BTreeSet::new(),
            store,
            loaded: None,
            asked: None,
            seq: None,
            epoch: 0,
            queue: Vec::new(),
        }
    }

    /// What to send now.
    pub fn poll(&mut self) -> Vec<PReq> {
        std::mem::take(&mut self.queue)
    }

    fn browser_pins(&mut self) -> Vec<String> {
        match self.store.load() {
            Ok(Some(t)) => serde_json::from_str::<Vec<String>>(&t)
                .map(|mut p| {
                    p.truncate(PINS);
                    p
                })
                .unwrap_or_default(),
            Ok(None) => Vec::new(),
            Err(e) => {
                self.problem = Some(format!("this browser does not keep pins ({e})"));
                Vec::new()
            }
        }
    }

    /// Asks the box for the pins' history.
    fn ask(&mut self, now: f64) {
        self.asked = Some(now);
        if self.pins.is_empty() {
            self.series.clear();
            return;
        }
        // Another node's signals: the box behind the page has no history
        // of them, and its answer carries its own clock, which moved the
        // window off the node's points and froze the line (2026-11-24,
        // Nick: "The trend line isn't moving even though the values are
        // updating"). The line is drawn from the values as they come.
        if self.local_pins {
            return;
        }
        let list: Vec<String> = self.pins.iter().map(|p| segment(p)).collect();
        self.queue.push(PReq::get(
            PCall::Trend,
            format!("/api/trends?signals={}", list.join(",")),
        ));
    }

    /// Variables a described change just added to the program, to be
    /// pinned when the box reports them ([`Self::step`]).
    pub fn expect(&mut self, added: &[String]) {
        for n in added {
            self.expecting.retain(|(e, _)| !e.eq_ignore_ascii_case(n));
            self.expecting.push((n.clone(), None));
        }
    }

    /// The expected variables the box reports now join the pins, at the
    /// bottom, popping; the pins the engineer chose stay, the oldest
    /// automatic one makes room. The ones left unreported for
    /// [`EXPECT_S`] are given up.
    fn pin_expected(&mut self, session: Option<&Session>, state: &BoxState, now: f64) {
        if self.expecting.is_empty() {
            return;
        }
        for (_, at) in &mut self.expecting {
            at.get_or_insert(now);
        }
        let mut found = Vec::new();
        self.expecting.retain(|(n, at)| {
            match state.signals.iter().find(|s| s.name.eq_ignore_ascii_case(n)) {
                Some(s) => {
                    found.push(s.name.clone());
                    false
                }
                None => at.is_some_and(|at| now - at < EXPECT_S),
            }
        });
        if found.is_empty() {
            return;
        }
        let before = self.pins.clone();
        let pinned = crate::ekg::pin_auto(&mut self.pins, &mut self.auto, &found, PINS);
        for n in &pinned {
            self.flashes.add(n);
        }
        self.series.retain(|n, _| self.pins.contains(n));
        if self.pins != before {
            self.keep_pins(session);
            self.ask(now);
        }
    }

    /// Every frame: the session (pins follow whoever is signed in), the
    /// newest values, and a renewal when due.
    pub fn step(&mut self, session: Option<&Session>, state: &BoxState, now: f64) {
        let who = owner(session);
        let key = who.clone().unwrap_or_default();
        if self.loaded.as_deref() != Some(key.as_str()) {
            self.loaded = Some(key);
            self.series.clear();
            if who.is_some() && !self.local_pins {
                self.queue
                    .push(PReq::get(PCall::TrendPins, "/api/trends/pins".into()));
            } else {
                self.pins = self.browser_pins();
                self.ask(now);
            }
        }
        if state.epoch() != self.epoch {
            // The box restarted: its clock starts again, so what was drawn
            // (on the old one) would interleave with the new.
            self.epoch = state.epoch();
            self.series.clear();
            self.now_ms = 0;
            self.ask(now);
        }
        if self.asked.is_some_and(|a| now - a >= RENEW_S) {
            self.ask(now);
        }
        self.pin_expected(session, state, now);
        if state.seq != self.seq {
            self.seq = state.seq;
            if let Some(at) = state.values_at_ms {
                for p in &self.pins {
                    let Some(v) = state.signals.iter().find(|s| &s.name == p).map(|s| s.value) else {
                        continue;
                    };
                    let line = self.series.entry(p.clone()).or_default();
                    if line.last().is_none_or(|(t, _)| *t <= at) {
                        line.push((at, v.map_or(f64::NAN, number)));
                    }
                    let from = at.saturating_sub(WINDOW_MS);
                    let old = line.iter().take_while(|(t, _)| *t < from).count();
                    line.drain(..old.saturating_sub(1));
                    let over = line.len().saturating_sub(MAX_POINTS);
                    line.drain(..over);
                }
                self.now_ms = self.now_ms.max(at);
            }
        }
    }

    /// Pins or unpins `name` (at most [`PINS`]), kept for the person or
    /// in this browser, and asks for the new set's history.
    pub fn toggle(&mut self, name: &str, session: Option<&Session>, now: f64) {
        self.auto.retain(|a| a != name);
        if let Some(k) = self.pins.iter().position(|p| p == name) {
            self.pins.remove(k);
            self.series.remove(name);
        } else {
            // At the cap a pin of the engineer's takes the place of the
            // oldest one nobody chose.
            if self.pins.len() >= PINS && !self.auto.is_empty() {
                let old = self.auto.remove(0);
                self.pins.retain(|p| *p != old);
                self.series.remove(&old);
            }
            if self.pins.len() >= PINS {
                return;
            }
            self.pins.push(name.to_string());
        }
        self.keep_pins(session);
        self.ask(now);
    }

    /// Moves the pin in lane `from` to lane `to`, the others closing up,
    /// and keeps the new order as [`toggle`](Self::toggle) keeps a pin.
    /// The history is per signal, so nothing is asked again.
    pub fn move_pin(&mut self, from: usize, to: usize, session: Option<&Session>) {
        let n = self.pins.len();
        if from >= n || to >= n || from == to {
            return;
        }
        let p = self.pins.remove(from);
        self.auto.retain(|a| *a != p);
        self.pins.insert(to, p);
        self.keep_pins(session);
    }

    /// Takes every pin in `names` off the trend, asking for the history
    /// once.
    pub fn remove_many(&mut self, names: &[String], session: Option<&Session>, now: f64) {
        let before = self.pins.len();
        self.pins.retain(|p| !names.contains(p));
        if self.pins.len() == before {
            return;
        }
        self.auto.retain(|a| !names.contains(a));
        self.series.retain(|k, _| !names.contains(k));
        self.keep_pins(session);
        self.ask(now);
    }

    /// Moves the pins in `names` one lane up (or down), keeping their
    /// order among themselves; a block already at the edge stays.
    pub fn move_many(&mut self, names: &[String], up: bool, session: Option<&Session>) {
        let n = self.pins.len();
        let mut order: Vec<usize> = (0..n).collect();
        if !up {
            order.reverse();
        }
        let mut moved = false;
        for k in order {
            if !names.contains(&self.pins[k]) {
                continue;
            }
            let to = if up {
                k.checked_sub(1)
            } else {
                Some(k + 1).filter(|t| *t < n)
            };
            if let Some(to) = to
                && !names.contains(&self.pins[to])
            {
                self.pins.swap(k, to);
                moved = true;
            }
        }
        if moved {
            self.auto.retain(|a| !names.contains(a));
            self.keep_pins(session);
        }
    }

    /// Keeps the pins, in order: on the box for a person signed in, in
    /// this browser otherwise.
    fn keep_pins(&mut self, session: Option<&Session>) {
        if session.is_some() && !self.local_pins {
            self.queue.push(PReq::json(
                PCall::SavePins,
                "PUT",
                "/api/trends/pins".into(),
                &json!({ "signals": self.pins }),
            ));
        } else if let Err(e) = self.store.save(&json!(self.pins).to_string()) {
            self.problem = Some(format!("this browser does not keep pins ({e})"));
        }
    }

    /// Takes in an answer to one of this tab's calls.
    pub fn answer(&mut self, call: &PCall, a: &Answer, now: f64) {
        let ok = (200..300).contains(&a.status);
        let j: Json = serde_json::from_str(&a.body).unwrap_or(Json::Null);
        let error = || j["error"].as_str().unwrap_or("no answer").to_string();
        match call {
            PCall::TrendPins => {
                if ok {
                    self.pins = j["pins"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|p| p.as_str().map(str::to_string))
                                .take(PINS)
                                .collect()
                        })
                        .unwrap_or_default();
                } else {
                    // The session is gone: the browser's pins.
                    self.pins = self.browser_pins();
                }
                self.ask(now);
            }
            PCall::SavePins if !ok => self.notice = Some(format!("pins not kept: {}", error())),
            PCall::Trend if ok => {
                self.now_ms = j["now_ms"].as_u64().unwrap_or(self.now_ms);
                for s in j["signals"].as_array().into_iter().flatten() {
                    let Some(name) = s["name"].as_str().filter(|n| self.pins.iter().any(|p| p == n)) else {
                        continue;
                    };
                    let mut pts: Vec<(u64, f64)> = s["points"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| Some((p[0].as_u64()?, number_json(&p[1]))))
                        .collect();
                    pts.sort_by_key(|p| p.0);
                    let over = pts.len().saturating_sub(MAX_POINTS);
                    pts.drain(..over);
                    // Values events newer than the history stay.
                    let last = pts.last().map_or(0, |p| p.0);
                    if let Some(live) = self.series.get(name) {
                        pts.extend(live.iter().filter(|p| p.0 > last));
                    }
                    self.series.insert(name.to_string(), pts);
                }
                let missing: Vec<&str> = j["unknown"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(j["refused"].as_array().into_iter().flatten())
                    .filter_map(Json::as_str)
                    .collect();
                self.notice = (!missing.is_empty()).then(|| format!("no history for {}", missing.join(", ")));
            }
            PCall::Trend => self.notice = Some(format!("no history: {}", error())),
            _ => {}
        }
    }

    /// The tab: the devices to open for their signals, the pins' actions,
    /// then the chart with a box left of each lane.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        state: &BoxState,
        map: &str,
        session: Option<&Session>,
    ) {
        let now = ui.ctx().input(|i| i.time);
        let who = owner(session).unwrap_or_else(|| "this browser".into());
        let t = theme(ui);
        let groups = crate::signal_picker::signal_groups(&state.signals, map);
        let pins = self.pins.clone();
        let flip = self.picker.ui(
            ui,
            chrome,
            "trend",
            &groups,
            &|n| pins.iter().any(|p| p == n),
            &format!(
                "Open a device to see its signals, then click one to pin it, up to {PINS}. Pins are kept for {who}."
            ),
        );
        if let Some(n) = flip {
            self.toggle(&n, session, now);
        }
        for text in [&self.notice, &self.problem].into_iter().flatten() {
            ui.label(RichText::new(text).color(t.muted));
        }
        self.checked
            .retain(|c| self.pins.iter().any(|p| p.to_ascii_lowercase() == *c));
        self.actions(ui, chrome, session, now);
        window_slider(ui, chrome, &mut self.window_ms);
        ui.add_space(6.0);
        if self.flashes.tick(now) {
            ui.ctx().request_repaint();
        }
        let full = ui.available_rect_before_wrap();
        let boxes = egui::Rect::from_min_max(full.min, egui::pos2(full.left() + CHECK_W, full.bottom()));
        let plot_rect = egui::Rect::from_min_max(egui::pos2(full.left() + CHECK_W, full.top()), full.max);
        let plot = ui
            .scope_builder(egui::UiBuilder::new().max_rect(plot_rect), |ui| {
                self.chart(ui, chrome)
            })
            .inner;
        if !self.pins.is_empty() {
            #[allow(clippy::cast_precision_loss)]
            let lane = plot.height() / self.pins.len() as f32;
            for (k, name) in self.pins.clone().iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let at = egui::Rect::from_center_size(
                    egui::pos2(boxes.center().x, plot.top() + lane * (k as f32 + 0.5)),
                    egui::Vec2::splat(CHECK_W.min(lane)),
                );
                let key = name.to_ascii_lowercase();
                let mut on = self.checked.contains(&key);
                let r = ui
                    .put(at, |ui: &mut egui::Ui| crate::theme::check(ui, &mut on, ""))
                    .tip("Tick to move, color or remove this signal with the others ticked.");
                chrome.hit(format!("trend check {key}"), r.rect);
                if r.changed() {
                    if on {
                        self.checked.insert(key);
                    } else {
                        self.checked.remove(&key);
                    }
                }
            }
        }
        if let Some((from, to)) = lane_handles(ui, chrome, plot, &self.pins) {
            self.move_pin(from, to, session);
        }
    }

    /// The row over the plot: select all, and what to do with the ticked.
    fn actions(&mut self, ui: &mut egui::Ui, chrome: &mut Chrome, session: Option<&Session>, now: f64) {
        if self.pins.is_empty() {
            return;
        }
        let t = theme(ui);
        let ticked: Vec<String> = self
            .pins
            .iter()
            .filter(|p| self.checked.contains(&p.to_ascii_lowercase()))
            .cloned()
            .collect();
        let all = ticked.len() == self.pins.len();
        let mut act = None;
        let mut pick = None;
        ui.horizontal_wrapped(|ui| {
            let mut v = all;
            let r = crate::theme::check(ui, &mut v, "All").tip("Tick every signal on the trend, or none.");
            chrome.hit("trend check all", r.rect);
            if r.changed() {
                self.checked = if v {
                    self.pins.iter().map(|p| p.to_ascii_lowercase()).collect()
                } else {
                    std::collections::BTreeSet::new()
                };
            }
            if ticked.is_empty() {
                return;
            }
            ui.label(RichText::new(format!("{} ticked", ticked.len())).color(t.muted));
            // Glyphs, not words (Nick, 2026-11-26).
            let side = crate::glyphs::ROW;
            for (key, icon, tip) in [
                (
                    "remove",
                    Icon::Clear,
                    "Remove: takes the ticked signals off the trend.",
                ),
                ("up", Icon::Up, "Up: moves the ticked signals up a lane."),
                ("down", Icon::Down, "Down: moves the ticked signals down a lane."),
            ] {
                let r = crate::glyphs::button_sized(ui, icon, side, true, false, tip);
                chrome.hit(format!("trend {key}"), r.rect);
                if r.clicked() {
                    act = Some(key);
                }
            }
            let c = crate::glyphs::button_sized(
                ui,
                Icon::Palette,
                side,
                true,
                false,
                "Color: gives the ticked signals one color.",
            );
            chrome.hit("trend color all", c.rect);
            let mut swatches = Vec::new();
            egui::Popup::menu(&c).show(|ui| {
                let t = theme(ui);
                ui.horizontal(|ui| {
                    for i in 0..crate::playground_view::TREND.len() {
                        let (rect, s) = ui.allocate_exact_size(egui::Vec2::splat(30.0), Sense::click());
                        let s = s.tip("The ticked signals take this color.");
                        ui.painter().rect_filled(
                            rect.shrink(4.0),
                            CornerRadius::same(4),
                            crate::playground_view::palette_color(&t, i),
                        );
                        swatches.push((format!("trend color {i}"), rect));
                        if s.clicked() {
                            pick = Some(i);
                        }
                    }
                });
            });
            chrome.hits.extend(swatches);
        });
        if let Some(i) = pick {
            for n in &ticked {
                chrome.set_color(n, i);
            }
        }
        match act {
            Some("remove") => {
                self.remove_many(&ticked, session, now);
                self.checked.clear();
            }
            Some(d) => self.move_many(&ticked, d == "up", session),
            None => {}
        }
    }

    /// Draws the chart; returns the rect it filled (the lanes' handles go
    /// over it).
    fn chart(&self, ui: &mut egui::Ui, chrome: &Chrome) -> egui::Rect {
        let t = theme(ui);
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, t.chart);
        if self.pins.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Pick signals to trend above",
                FontId::proportional(crate::theme::MIN_TEXT),
                t.muted,
            );
            return rect;
        }
        let sweep = crate::ekg::Sweep::new(self.window_ms, self.now_ms);
        let now = ui.input(|i| i.time);
        #[allow(clippy::cast_precision_loss)]
        let lane = rect.height() / self.pins.len() as f32;
        for (k, name) in self.pins.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let top = rect.top() + lane * k as f32;
            let line: &[(u64, f64)] = self.series.get(name).map_or(&[], Vec::as_slice);
            // The point before the window holds its value into it; the
            // scale is the window's.
            let runs = sweep.runs(line.iter().copied());
            let (lo, hi) = runs
                .iter()
                .flatten()
                .map(|p| p.1)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let number = line
                .last()
                .map(|p| p.1)
                .filter(|v| v.is_finite())
                .map(helmsman_iec::shown_real);
            crate::ekg::paint_lane(
                &painter,
                &t,
                rect,
                (top, lane),
                &crate::ekg::Lane {
                    name,
                    color: crate::playground_view::lane_color(&t, chrome, name, k),
                    number,
                    pop: self.flashes.strength(name, now),
                },
                &sweep,
                &runs,
                nice_range(lo, hi),
            );
        }
        rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::SignalRow;
    use crate::sim_projects::MemStorage;

    fn row(name: &str, v: bool) -> SignalRow {
        SignalRow {
            name: name.into(),
            dir: "out".into(),
            unit: None,
            value: Some(Value::Bool(v)),
            quality: "good".into(),
            point_error: None,
            forced: false,
            sim: false,
        }
    }

    fn history(body: &Json) -> Answer {
        Answer {
            status: 200,
            retry_after: None,
            body: body.to_string(),
        }
    }

    /// The box's history first, then each values event extends it; an
    /// event older than the history adds nothing, and the window holds.
    #[test]
    fn history_then_the_stream_within_the_window() {
        let mut t = BoxTrend::new(Box::new(MemStorage(Some(r#"["lamp"]"#.into()))));
        let mut s = BoxState::new();
        t.step(None, &s, 0.0);
        assert_eq!(t.pins, ["lamp"]);
        let asked = t.poll();
        assert_eq!(asked[0].url, "/api/trends?signals=lamp");
        t.answer(
            &PCall::Trend,
            &history(&json!({
                "now_ms": 5000, "window_ms": 600_000,
                "signals": [{ "name": "lamp", "points": [[1000, false], [4000, true]] },
                            { "name": "other", "points": [[1, true]] }],
                "unknown": ["gone"], "refused": [],
            })),
            0.1,
        );
        assert_eq!(t.series["lamp"], [(1000, 0.0), (4000, 1.0)]);
        assert!(!t.series.contains_key("other"), "only what is pinned");
        assert!(t.notice.as_deref().unwrap().contains("gone"));
        for (seq, at, v) in [(1, 6000, false), (2, 3000, true), (3, 6_000_000, true)] {
            s.seq = Some(seq);
            s.values_at_ms = Some(at);
            s.signals = vec![row("lamp", v)];
            t.step(None, &s, 1.0);
        }
        let line = &t.series["lamp"];
        assert!(!line.iter().any(|p| p.0 == 3000), "older than what is drawn");
        assert!(
            line.iter().all(|p| p.0 + WINDOW_MS >= 6_000_000 || p == &line[0]),
            "{line:?}"
        );
        assert_eq!(line.last(), Some(&(6_000_000, 1.0)));
        // Renewed every RENEW_S.
        t.poll();
        t.step(None, &s, RENEW_S + 1.0);
        assert_eq!(t.poll().len(), 1);
    }

    /// The values stream's `at_ms` is the box's clock since its process
    /// started: after a restart it is small again, and a series that kept
    /// the old (larger) times would drop every new point.
    #[test]
    fn a_box_restart_starts_the_trend_over() {
        use crate::remote::{Msg, Stream};
        let mut t = BoxTrend::new(Box::new(MemStorage(Some(r#"["lamp"]"#.into()))));
        let mut s = BoxState::new();
        let ev = |seq: u64, at: u64| {
            Msg::Event(
                Stream::Values,
                "values".into(),
                json!({ "seq": seq, "at_ms": at, "modules": [],
                        "signals": [{ "name": "lamp", "dir": "out", "value": true, "quality": "good" }] })
                .to_string(),
            )
        };
        t.step(None, &s, 0.0);
        t.poll();
        for (k, at) in [(100, 600_000), (101, 600_100)] {
            s.apply(ev(k, at), 1.0);
            t.step(None, &s, 1.0);
        }
        assert_eq!(t.series["lamp"].last(), Some(&(600_100, 1.0)));
        // The box restarts; its clock is at 200 ms.
        s.apply(ev(1, 200), 2.0);
        t.step(None, &s, 2.0);
        assert_eq!(t.series["lamp"], [(200, 1.0)], "the old clock's points are void");
        assert_eq!(t.now_ms, 200);
        assert_eq!(t.poll().len(), 1, "the box's history is asked for again");
        s.apply(ev(2, 300), 2.1);
        t.step(None, &s, 2.1);
        assert_eq!(t.series["lamp"], [(200, 1.0), (300, 1.0)], "and it extends");
    }

    /// Nick, 2026-11-24: "The trend line isn't moving even though the
    /// values are updating". On a page a box served that watches a Skipper,
    /// the box's history answer set the window to the box's clock, so the
    /// Skipper's points fell outside it. There the box is not asked, the
    /// pins stay in this browser, and the window follows the Skipper.
    #[test]
    fn a_skipper_page_trends_the_skippers_values_on_the_skippers_clock() {
        use crate::remote::{Msg, Stream};
        let me = Session {
            user: Some("nick".into()),
            role: "engineer".into(),
        };
        let mut t = BoxTrend::new(Box::new(MemStorage(Some(r#"["position"]"#.into()))));
        t.local_pins = true;
        let mut s = BoxState::new();
        t.step(Some(&me), &s, 0.0);
        assert!(
            t.poll().is_empty(),
            "neither the box's pins nor its history are asked"
        );
        assert_eq!(t.pins, ["position"], "the browser's pins");
        let ev = |seq: u64, at: u64, v: i64| {
            Msg::Event(
                Stream::Values,
                "values".into(),
                json!({ "seq": seq, "at_ms": at, "modules": [],
                        "signals": [{ "name": "position", "dir": "in", "value": v, "quality": "good" }] })
                .to_string(),
            )
        };
        for (k, at, v) in [(1, 1_914_892, 10), (2, 1_914_992, 20), (3, 1_915_092, 30)] {
            s.apply(ev(k, at, v), 1.0);
            t.step(Some(&me), &s, 1.0);
        }
        assert_eq!(
            t.now_ms, 1_915_092,
            "the window ends at the Skipper's newest point"
        );
        assert_eq!(t.series["position"].len(), 3);
        t.toggle("slew", Some(&me), 2.0);
        assert!(
            t.poll().is_empty(),
            "a new pin asks the box for nothing (its answer would carry its own clock)"
        );
    }

    #[test]
    fn at_most_pins_signals_and_garbage_pins_are_none() {
        let mut t = BoxTrend::new(Box::new(MemStorage(Some("{not json".into()))));
        t.step(None, &BoxState::new(), 0.0);
        assert_eq!(t.pins, Vec::<String>::new());
        for k in 0..=PINS {
            t.toggle(&format!("s{k}"), None, 0.0);
        }
        assert_eq!(t.pins.len(), PINS);
        t.toggle("s0", None, 0.0);
        assert_eq!(t.pins.len(), PINS - 1);
    }

    /// Nick, 2026-11-07: the lanes reorder. The new order is kept where the
    /// pins are kept, the history stays, and a bad lane moves nothing.
    #[test]
    fn moving_a_pin_keeps_the_new_order_for_the_person_and_the_browser() {
        let me = Session {
            user: Some("nick".into()),
            role: "engineer".into(),
        };
        let mut t = BoxTrend::new(Box::new(MemStorage(Some(r#"["a","b","c"]"#.into()))));
        t.step(None, &BoxState::new(), 0.0);
        t.series.insert("a".into(), vec![(1, 1.0)]);
        t.poll();
        t.move_pin(0, 2, None);
        assert_eq!(t.pins, ["b", "c", "a"]);
        assert!(
            t.poll().is_empty(),
            "the history is per signal: nothing asked again"
        );
        assert_eq!(t.series["a"], [(1, 1.0)]);
        for (from, to) in [(3, 0), (0, 3), (1, 1), (usize::MAX, 0)] {
            t.move_pin(from, to, None);
            assert_eq!(t.pins, ["b", "c", "a"], "{from} -> {to}");
        }
        // Signed in and out again: the browser kept the order.
        t.step(Some(&me), &BoxState::new(), 1.0);
        t.answer(
            &PCall::TrendPins,
            &Answer {
                status: 401,
                retry_after: None,
                body: "{}".into(),
            },
            1.0,
        );
        t.step(None, &BoxState::new(), 2.0);
        assert_eq!(t.pins, ["b", "c", "a"], "the browser's pins, in the new order");
        // Signed in: the box keeps the order.
        t.step(Some(&me), &BoxState::new(), 3.0);
        t.answer(
            &PCall::TrendPins,
            &Answer {
                status: 200,
                retry_after: None,
                body: r#"{"pins":["x","y"]}"#.into(),
            },
            3.0,
        );
        t.poll();
        t.move_pin(1, 0, Some(&me));
        let sent = t.poll();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (sent[0].method, sent[0].url.as_str()),
            ("PUT", "/api/trends/pins")
        );
        let body: Json = serde_json::from_slice(sent[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({ "signals": ["y", "x"] }));
    }

    /// A described change on a box is saved first: its new variables join
    /// the pins (kept the way pins are kept, the box's `PUT`) when the box
    /// reports them, at the bottom, popping. Ones it never reports are
    /// given up, a change that adds nothing pins nothing, and the
    /// engineer's own pins stay at the cap.
    #[test]
    fn variables_a_change_added_are_pinned_when_the_box_reports_them() {
        let me = Session {
            user: Some("nick".into()),
            role: "engineer".into(),
        };
        let mut t = BoxTrend::new(Box::new(MemStorage(None)));
        let mut s = BoxState::new();
        t.step(Some(&me), &s, 0.0);
        t.answer(
            &PCall::TrendPins,
            &Answer {
                status: 200,
                retry_after: None,
                body: r#"{"pins":["a","b"]}"#.into(),
            },
            0.0,
        );
        t.poll();
        t.expect(&[]);
        t.step(Some(&me), &s, 1.0);
        assert!(t.poll().is_empty(), "nothing added, nothing asked or saved");
        // Saved, not deployed yet: the box does not report them.
        t.expect(&["fan".to_string(), "horn".to_string()]);
        t.step(Some(&me), &s, 2.0);
        assert_eq!(t.pins, ["a", "b"]);
        // Deployed: it reports FAN (in its own case) and not HORN.
        s.signals = vec![row("FAN", true)];
        t.step(Some(&me), &s, 3.0);
        assert_eq!(t.pins, ["a", "b", "FAN"], "at the bottom, as the box spells it");
        let sent = t.poll();
        let put = sent.iter().find(|r| r.method == "PUT").expect("kept for her");
        assert_eq!(put.url, "/api/trends/pins");
        assert!(t.flashes.tick(3.0) && t.flashes.strength("FAN", 3.0) > 0.9);
        assert!(!t.flashes.tick(4.6), "settled");
        // HORN is given up after EXPECT_S; later it pins nothing.
        t.step(Some(&me), &s, 3.0 + EXPECT_S + 1.0);
        s.signals.push(row("horn", false));
        t.step(Some(&me), &s, 3.0 + EXPECT_S + 2.0);
        assert_eq!(t.pins, ["a", "b", "FAN"]);
        // At the cap: the engineer's pins (a, b by the box) stay, FAN, the
        // oldest automatic pin, makes room.
        let names: Vec<String> = (0..PINS).map(|k| format!("n{k}")).collect();
        s.signals = names.iter().map(|n| row(n, true)).collect();
        t.expect(&names);
        t.step(Some(&me), &s, 4000.0);
        assert_eq!(t.pins.len(), PINS);
        assert_eq!(t.pins[..2], ["a", "b"], "the box's pins are the engineer's own");
        assert!(
            !t.pins.contains(&"FAN".to_string()),
            "the automatic pin made room"
        );
        assert_eq!(t.pins.last().map(String::as_str), Some("n5"));
    }

    #[test]
    fn the_box_trend_opens_on_30_s() {
        let t = BoxTrend::new(Box::new(MemStorage(None)));
        assert_eq!(t.window_ms, 30_000);
    }
}
