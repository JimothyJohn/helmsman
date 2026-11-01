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

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, RichText, Sense, Stroke};
use helmsman_iec::Value;
use serde_json::{Value as Json, json};

use crate::box_projects::{Answer, PCall, PReq};
use crate::editor::Chrome;
use crate::remote::{BoxState, segment};
use crate::review::Session;
use crate::sim_projects::Storage;
use crate::st_view::brand::{BLACK, GRAPHITE, STEEL};

/// Signals one trend draws at most (as in the simulation).
pub const PINS: usize = crate::playground::TREND_SIGNALS;
/// History drawn, ms (the box keeps 10 min).
pub const WINDOW_MS: u64 = 600_000;
/// Points a signal keeps here.
pub const MAX_POINTS: usize = 2400;
/// Seconds between history asks while the tab is open.
pub const RENEW_S: f64 = 600.0;

/// Line colours, in pin order.
const LINES: [Color32; 6] = [
    Color32::from_rgb(0xFF, 0xC2, 0x0E),
    Color32::from_rgb(0x4F, 0xC3, 0xF7),
    Color32::from_rgb(0x81, 0xC7, 0x84),
    Color32::from_rgb(0xFF, 0x8A, 0x65),
    Color32::from_rgb(0xBA, 0x68, 0xC8),
    Color32::from_rgb(0xE0, 0xE0, 0xE0),
];

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
    /// What the box said about the last ask or save.
    pub notice: Option<String>,
    /// Why this browser does not keep pins, if it does not.
    pub problem: Option<String>,
    store: Box<dyn Storage>,
    /// The owner the pins were loaded for (`""`: the browser).
    loaded: Option<String>,
    /// Page time of the last history ask.
    asked: Option<f64>,
    seq: Option<u64>,
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
            notice: None,
            problem: None,
            store,
            loaded: None,
            asked: None,
            seq: None,
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
        let list: Vec<String> = self.pins.iter().map(|p| segment(p)).collect();
        self.queue.push(PReq::get(
            PCall::Trend,
            format!("/api/trends?signals={}", list.join(",")),
        ));
    }

    /// Every frame: the session (pins follow whoever is signed in), the
    /// newest values, and a renewal when due.
    pub fn step(&mut self, session: Option<&Session>, state: &BoxState, now: f64) {
        let who = owner(session);
        let key = who.clone().unwrap_or_default();
        if self.loaded.as_deref() != Some(key.as_str()) {
            self.loaded = Some(key);
            self.series.clear();
            if who.is_some() {
                self.queue
                    .push(PReq::get(PCall::TrendPins, "/api/trends/pins".into()));
            } else {
                self.pins = self.browser_pins();
                self.ask(now);
            }
        }
        if self.asked.is_some_and(|a| now - a >= RENEW_S) {
            self.ask(now);
        }
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
        if let Some(k) = self.pins.iter().position(|p| p == name) {
            self.pins.remove(k);
            self.series.remove(name);
        } else if self.pins.len() < PINS {
            self.pins.push(name.to_string());
        } else {
            return;
        }
        if session.is_some() {
            self.queue.push(PReq::json(
                PCall::SavePins,
                "PUT",
                "/api/trends/pins".into(),
                &json!({ "signals": self.pins }),
            ));
        } else if let Err(e) = self.store.save(&json!(self.pins).to_string()) {
            self.problem = Some(format!("this browser does not keep pins ({e})"));
        }
        self.ask(now);
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

    /// The tab: the signals to pin, then the chart.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &mut Chrome,
        state: &BoxState,
        session: Option<&Session>,
    ) {
        let now = ui.ctx().input(|i| i.time);
        let who = owner(session).unwrap_or_else(|| "this browser".into());
        ui.label(RichText::new(format!("Pin up to {PINS} signals (kept for {who})")).color(STEEL));
        let mut flip = None;
        ui.horizontal_wrapped(|ui| {
            for s in &state.signals {
                let on = self.pins.contains(&s.name);
                let r = ui.selectable_label(on, s.name.to_ascii_lowercase());
                chrome.hit(format!("pin {}", s.name), r.rect);
                if r.clicked() {
                    flip = Some(s.name.clone());
                }
            }
        });
        if let Some(n) = flip {
            self.toggle(&n, session, now);
        }
        for t in [&self.notice, &self.problem].into_iter().flatten() {
            ui.label(RichText::new(t).color(STEEL));
        }
        ui.add_space(6.0);
        self.chart(ui);
    }

    fn chart(&self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, BLACK);
        if self.pins.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Pick signals to trend above",
                FontId::proportional(13.0),
                STEEL,
            );
            return;
        }
        let end = self.now_ms;
        let start = end.saturating_sub(WINDOW_MS);
        #[allow(clippy::cast_precision_loss)]
        let lane = rect.height() / self.pins.len() as f32;
        #[allow(clippy::cast_precision_loss)]
        let x_of = |t: u64| {
            rect.left() + 90.0 + (rect.width() - 96.0) * (t.max(start) - start) as f32 / WINDOW_MS as f32
        };
        for (k, name) in self.pins.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let top = rect.top() + lane * k as f32;
            let color = LINES[k % LINES.len()];
            let line: &[(u64, f64)] = self.series.get(name).map_or(&[], Vec::as_slice);
            let (lo, hi) = line
                .iter()
                .map(|p| p.1)
                .filter(|v| v.is_finite())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let last = line.last().map(|p| p.1).filter(|v| v.is_finite());
            painter.text(
                Pos2::new(rect.left() + 6.0, top + lane / 2.0),
                egui::Align2::LEFT_CENTER,
                format!(
                    "{}\n{}",
                    name.to_ascii_lowercase(),
                    last.map_or("–".into(), |v| format!("{v}"))
                ),
                FontId::monospace(10.5),
                color,
            );
            painter.line_segment(
                [Pos2::new(rect.left(), top), Pos2::new(rect.right(), top)],
                Stroke::new(1.0, GRAPHITE),
            );
            let span = if hi > lo { hi - lo } else { 1.0 };
            let y_of = |v: f64| {
                #[allow(clippy::cast_possible_truncation)]
                let f = ((v - lo) / span) as f32;
                top + lane - 4.0 - (lane - 8.0) * f.clamp(0.0, 1.0)
            };
            let mut pts: Vec<Pos2> = Vec::new();
            for &(t, v) in line {
                if !v.is_finite() {
                    continue;
                }
                let p = Pos2::new(x_of(t), y_of(v));
                // Steps: a value holds until the next change.
                if let Some(prev) = pts.last().copied() {
                    pts.push(Pos2::new(p.x, prev.y));
                }
                pts.push(p);
            }
            // The last value holds to now.
            if let Some(prev) = pts.last().copied() {
                pts.push(Pos2::new(x_of(end), prev.y));
            }
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
            }
        }
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
}
