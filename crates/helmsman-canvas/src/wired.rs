//! What is wired to a point, beyond the Devices tab's own line (TODO.md
//! `iodd-metadata`): the same line under a signal in the I/O tab, the
//! picker that finds a sensor or actuator by the number on its label
//! and writes `wired=VENDOR/MODEL` into its map line, and the
//! playground's own reading of the device library, which it does with
//! no box: the index, then each wired device's `record.json` beside it.
//!
//! The picker asks its view to search ([`PickAct::Search`]): a box
//! answers from `GET /api/devices/library?q=` ([`Pick::answer`]), the
//! playground from the index it read ([`Pick::found_in`]). Only records
//! that sit on a digital channel are offered: a device on the network is
//! added from the Scan, not named on a point.

use std::collections::VecDeque;

use eframe::egui::{self, RichText, Vec2};
use helmsman_project::devlib::{Entry, Index};
use helmsman_project::devrecord::Record;
use helmsman_project::iomap;
use serde_json::Value as Json;

use crate::devices::{DevicesUi, Wired, WiredBrief};
use crate::editor::Chrome;
use crate::st_view::TAP;
use crate::theme::theme;
use crate::tips::Tip;

/// Longest query the picker sends (the box's own limit).
pub const QUERY_MAX: usize = 120;
/// Most matches the picker lists.
const HITS: usize = 10;
/// Most order numbers a match shows on its line.
const PRODUCTS_SHOWN: usize = 4;
/// How many list entries of a record the brief keeps (the box's
/// `RECORD_BRIEF`).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser build fetches; native tests drive it
const BRIEF: usize = 16;

/// `map` with `signal`'s line carrying `wired=` (`Some`) or without it
/// (`None`): its other tokens in their order, its comment kept, every
/// other line as it was. `None` when no `in`/`out` line names the signal
/// or the value is not a library folder.
#[must_use]
pub fn set_wired(map: &str, signal: &str, wired: Option<&str>) -> Option<String> {
    if wired.is_some_and(|w| !iomap::wired_ok(w)) {
        return None;
    }
    let mut out = String::with_capacity(map.len() + 32);
    let mut found = false;
    for line in map.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(b) => (
                b.strip_suffix('\r').unwrap_or(b),
                &line[b.trim_end_matches('\r').len()..],
            ),
            None => (line, ""),
        };
        let (code, comment) = match body.find('#') {
            Some(i) => (&body[..i], Some(&body[i..])),
            None => (body, None),
        };
        let t: Vec<&str> = code.split_whitespace().collect();
        let ours =
            !found && t.len() >= 4 && (t[0] == "in" || t[0] == "out") && t[1].eq_ignore_ascii_case(signal);
        if !ours {
            out.push_str(line);
            continue;
        }
        found = true;
        let lead = &code[..code.len() - code.trim_start().len()];
        let mut kept: Vec<String> = t
            .iter()
            .filter(|w| !w.starts_with("wired="))
            .map(|w| (*w).to_string())
            .collect();
        if let Some(w) = wired {
            kept.push(format!("wired={w}"));
        }
        out.push_str(lead);
        out.push_str(&kept.join(" "));
        if let Some(c) = comment {
            out.push_str("  ");
            out.push_str(c);
        }
        out.push_str(end);
    }
    found.then_some(out)
}

/// One record the picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The `wired=` value.
    pub wired: String,
    pub vendor: String,
    pub model: String,
    pub kind: String,
    pub products: Vec<String>,
}

fn bounded(v: &Json) -> Option<String> {
    v.as_str()
        .map(|s| s.chars().filter(|c| !c.is_control()).take(120).collect())
}

impl Hit {
    /// An index entry, when it is something that sits on a digital
    /// channel (no protocol) with a folder a map line can name.
    #[must_use]
    pub fn of_entry(e: &Entry) -> Option<Self> {
        (!e.networked() && iomap::wired_ok(e.wired())).then(|| Self {
            wired: e.wired().to_string(),
            vendor: e.vendor.clone(),
            model: e.model.clone(),
            kind: e.kind.clone(),
            products: e.products.clone(),
        })
    }

    /// A match of the box's `GET /api/devices/library?q=`, on the same
    /// terms.
    #[must_use]
    pub fn of_match(m: &Json) -> Option<Self> {
        let wired = bounded(&m["wired"]).filter(|w| iomap::wired_ok(w))?;
        if m["networked"].as_bool() != Some(false) {
            return None;
        }
        Some(Self {
            wired,
            vendor: bounded(&m["vendor"])?,
            model: bounded(&m["model"]).unwrap_or_default(),
            kind: bounded(&m["kind"]).unwrap_or_else(|| "other".into()),
            products: m["products"]
                .as_array()
                .map(|a| a.iter().filter_map(bounded).take(128).collect())
                .unwrap_or_default(),
        })
    }

    /// The match's line: vendor, the order numbers the query hit first
    /// (or the model when none did), the kind.
    #[must_use]
    pub fn title(&self, query: &str) -> String {
        let words: Vec<String> = query.split_whitespace().map(str::to_ascii_lowercase).collect();
        let mut shown: Vec<&str> = self
            .products
            .iter()
            .filter(|p| words.iter().any(|w| p.to_ascii_lowercase().contains(w.as_str())))
            .map(String::as_str)
            .take(PRODUCTS_SHOWN)
            .collect();
        if shown.is_empty() {
            shown.push(&self.model);
        }
        format!("{} {} · {}", self.vendor, shown.join(", "), self.kind)
    }
}

/// Where the picker's search stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hits {
    /// Nothing searched yet.
    Idle,
    /// Asked; the answer has not come.
    Searching(String),
    /// The query and its matches, with how many more there were.
    Found(String, Vec<Hit>, usize),
    /// Why there is no answer, in words.
    Failed(String),
}

/// The picker on one signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub signal: String,
    /// What the map names on the point now.
    pub current: Option<String>,
    /// The text typed.
    pub query: String,
    pub hits: Hits,
}

/// What the picker asks of its view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickAct {
    /// Search the library for this text.
    Search(String),
    /// Write `wired=` (or take it out, `None`) on `signal`'s map line.
    Set { signal: String, wired: Option<String> },
}

/// Why a query is not sent, if it is not.
fn query_error(q: &str) -> Option<&'static str> {
    if q.is_empty() {
        Some("Type the number on its label, or its vendor and model.")
    } else if q.chars().count() > QUERY_MAX || q.chars().any(char::is_control) {
        Some("At most 120 characters, on one line.")
    } else {
        None
    }
}

fn list(query: &str, mut hits: Vec<Hit>) -> Hits {
    let more = hits.len().saturating_sub(HITS);
    hits.truncate(HITS);
    Hits::Found(query.to_string(), hits, more)
}

impl Pick {
    #[must_use]
    pub fn new(signal: &str, current: Option<&str>) -> Self {
        Self {
            signal: signal.to_string(),
            current: current.map(str::to_string),
            query: String::new(),
            hits: Hits::Idle,
        }
    }

    /// The query being searched, while one is.
    #[must_use]
    pub fn searching(&self) -> Option<&str> {
        match &self.hits {
            Hits::Searching(q) => Some(q),
            _ => None,
        }
    }

    /// The playground's answer: the matches in the index it read.
    pub fn found_in(&mut self, index: &Index) {
        let Some(q) = self.searching().map(str::to_string) else {
            return;
        };
        let hits = index.search(&q).into_iter().filter_map(Hit::of_entry).collect();
        self.hits = list(&q, hits);
    }

    /// The box's answer to `GET /api/devices/library?q=`.
    pub fn answer(&mut self, status: u16, body: &str) {
        let Some(q) = self.searching().map(str::to_string) else {
            return;
        };
        let j: Json = serde_json::from_str(body).unwrap_or(Json::Null);
        self.hits = match status {
            200..=299 => match bounded(&j["unavailable"]) {
                Some(why) => Hits::Failed(format!("The device library is unavailable: {why}")),
                None => list(
                    &q,
                    j["matches"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Hit::of_match).collect())
                        .unwrap_or_default(),
                ),
            },
            404 => Hits::Failed("This box does not search the device library.".into()),
            s => Hits::Failed(format!(
                "The box could not search the library ({s}): {}",
                bounded(&j["error"]).unwrap_or_else(|| "no answer".into())
            )),
        };
    }

    /// The playground could not read the library.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser build fetches; native tests drive it
    pub fn failed(&mut self, why: &str) {
        if self.searching().is_some() {
            self.hits = Hits::Failed(format!("The device library could not be read: {why}"));
        }
    }
}

impl DevicesUi {
    /// The picker, while one is open: a box in the middle of the screen,
    /// as the Rename question is. Returns what the view should do.
    pub fn pick_ui(&mut self, ctx: &egui::Context, chrome: &mut Chrome) -> Option<PickAct> {
        let mut p = self.picking.clone()?;
        let mut out = None;
        let mut done = false;
        let m = crate::theme::near_pointer(ctx, "wired-pick").show(ctx, |ui| {
            let t = theme(ui);
            ui.set_width(460.0);
            ui.label(RichText::new(format!("What is wired to {}?", p.signal)).strong().size(20.0));
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    "Type the order number on its label, or its vendor and model, and pick it from the device library. The library says what it is and what its signal means; the program and Helmsman read that.",
                )
                .wrap(),
            );
            if let Some(c) = &p.current {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Now: {c}")).color(t.muted))
                        .tip("The wired= value on this signal's io.map line.");
                    let clear = ui
                        .add(egui::Button::new("Clear").min_size(Vec2::new(0.0, TAP * 0.6)))
                        .tip("Takes wired= off this signal's line in io.map.");
                    chrome.hit("wired clear", clear.rect);
                    if clear.clicked() {
                        out = Some(PickAct::Set { signal: p.signal.clone(), wired: None });
                        done = true;
                    }
                });
            }
            ui.add_space(8.0);
            let mut go = false;
            ui.horizontal(|ui| {
                let field_tip = "The number printed on the device's label (IF6123), or its vendor and model (ifm O5D).";
                let f = ui
                    .add(
                        egui::TextEdit::singleline(&mut p.query)
                            .hint_text("IF6123")
                            .char_limit(QUERY_MAX)
                            .desired_width(300.0),
                    )
                    .tip(field_tip);
                chrome.hit("wired query", f.rect);
                if !f.has_focus() && p.query.is_empty() && matches!(p.hits, Hits::Idle) {
                    f.request_focus();
                }
                go |= f.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let s = ui
                    .add(egui::Button::new("Search").min_size(Vec2::new(0.0, TAP * 0.6)))
                    .tip("Searches the device library.");
                chrome.hit("wired search", s.rect);
                go |= s.clicked();
            });
            let q = p.query.trim().to_string();
            if go {
                if let Some(why) = query_error(&q) {
                    p.hits = Hits::Failed(why.into());
                } else {
                    p.hits = Hits::Searching(q.clone());
                    out = Some(PickAct::Search(q));
                }
            }
            ui.add_space(6.0);
            match &p.hits {
                Hits::Idle => {}
                Hits::Searching(q) => {
                    ui.label(RichText::new(format!("Searching for {q}…")).color(t.muted));
                }
                Hits::Failed(why) => {
                    ui.add(egui::Label::new(RichText::new(why).color(t.muted)).wrap());
                }
                Hits::Found(q, hits, _) if hits.is_empty() => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "Nothing in the library for {q}. Try the number without its suffix, or the vendor's name."
                            ))
                            .color(t.muted),
                        )
                        .wrap(),
                    );
                }
                Hits::Found(q, hits, more) => {
                    for h in hits {
                        let tip = format!(
                            "{} {}\n{}\nCovers {}.\nWrites wired={} on {}'s line in io.map.",
                            h.vendor,
                            h.model,
                            h.kind,
                            if h.products.is_empty() { h.model.clone() } else { h.products.join(", ") },
                            h.wired,
                            p.signal
                        );
                        let r = ui
                            .add(
                                egui::Button::new(h.title(q))
                                    .wrap()
                                    .min_size(Vec2::new(ui.available_width(), TAP * 0.6)),
                            )
                            .tip(tip);
                        chrome.hit(format!("wired hit {}", h.wired), r.rect);
                        if r.clicked() {
                            out = Some(PickAct::Set { signal: p.signal.clone(), wired: Some(h.wired.clone()) });
                            done = true;
                        }
                    }
                    if *more > 0 {
                        ui.label(RichText::new(format!("{more} more: add to the search to narrow it.")).color(t.muted));
                    }
                }
            }
            ui.add_space(10.0);
            let no = ui
                .add(egui::Button::new("Cancel").min_size(Vec2::new(0.0, TAP * 0.7)))
                .tip("Leaves the line as it is.");
            chrome.hit("wired cancel", no.rect);
            done |= no.clicked();
        });
        chrome.hit("wired form", m.response.rect);
        self.picking = if done || crate::theme::escaped(&m) {
            None
        } else {
            Some(p)
        };
        out
    }
}

/// Each signal's `wired=` value in `map`, by its name in lower case
/// (ST names are case-blind).
#[must_use]
pub fn by_signal(map: &iomap::MapText) -> std::collections::BTreeMap<String, String> {
    map.signals
        .iter()
        .filter_map(|s| Some((s.name.to_ascii_lowercase(), s.wired()?.to_string())))
        .collect()
}

/// The line under a signal saying what is wired to it, and its tooltip,
/// from what the library answered for `wired`.
#[must_use]
pub fn line(wired: &str, known: Option<&Wired>) -> (String, String) {
    match known {
        Some(Wired::Known(b)) => (b.title(), b.tip()),
        Some(Wired::Unavailable(why)) => (
            wired.to_string(),
            format!("Wired to this point: {wired} in the device library.\nNo record to show: {why}."),
        ),
        Some(Wired::Asked) | None => (
            wired.to_string(),
            format!("Wired to this point: {wired} in the device library."),
        ),
    }
}

/// Draws [`line`], indented by `indent`, wrapped to the column, and
/// names it `wired SIGNAL` for the tests.
pub fn show_line(
    ui: &mut egui::Ui,
    chrome: &mut Chrome,
    signal: &str,
    wired: &str,
    known: Option<&Wired>,
    indent: f32,
) {
    let (text, tip) = line(wired, known);
    let muted = theme(ui).muted;
    let r = ui
        .horizontal_wrapped(|ui| {
            ui.add_space(indent);
            ui.add(egui::Label::new(RichText::new(text).color(muted)).wrap())
        })
        .inner
        .tip(tip);
    chrome.hit(format!("wired {signal}"), r.rect);
}

/// What the playground asks the network for next.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser build fetches; native tests drive it
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetch {
    /// The library's index.
    Index(String),
    /// One wired device's `record.json`: its `wired=` value and the URL.
    Record(String, String),
}

/// The playground's reading of the device library, with no box: the
/// index once, then each record a map names, one at a time. A pure state
/// machine; the view does the fetching (`web::TextFetch`) and hands the
/// text back.
#[derive(Debug, Default)]
pub struct Library {
    pub index: Option<Index>,
    /// Why the index could not be read.
    pub failed: Option<String>,
    /// Something needs the index (a wired value, the picker, a Scan).
    index_wanted: bool,
    index_asked: bool,
    /// `wired=` values waiting for the index or for their turn.
    waiting: VecDeque<String>,
    /// The record being fetched.
    fetching: Option<String>,
}

impl Library {
    /// Ask for the index: once, and again only after a failure.
    pub fn want_index(&mut self) {
        self.index_wanted = true;
        if self.failed.is_some() {
            self.failed = None;
            self.index_asked = false;
        }
    }

    /// Look `wired` up when the index is here.
    pub fn want(&mut self, wired: String) {
        self.index_wanted = true;
        if !self.waiting.contains(&wired) && self.fetching.as_ref() != Some(&wired) {
            self.waiting.push_back(wired);
        }
    }

    /// The picker's search: answered at once from the index read, else
    /// once it comes ([`Self::index_answer`]).
    pub fn search(&mut self, ui: &mut DevicesUi) {
        match (&self.index, &mut ui.picking) {
            (Some(index), Some(p)) => p.found_in(index),
            _ => self.want_index(),
        }
    }

    /// The next fetch to start, if one is due; what the index answers
    /// without a fetch goes into `ui.wired` on the way.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser build fetches; native tests drive it
    pub fn next(&mut self, url: &str, ui: &mut DevicesUi) -> Option<Fetch> {
        let Some(index) = &self.index else {
            if !self.index_wanted || self.index_asked || self.failed.is_some() {
                return None;
            }
            self.index_asked = true;
            return Some(Fetch::Index(url.to_string()));
        };
        if self.fetching.is_some() {
            return None;
        }
        let base = url.rsplit_once('/').map_or(url, |(b, _)| b);
        while let Some(w) = self.waiting.pop_front() {
            match index.by_wired(&w) {
                Some(e) if e.record => {
                    self.fetching = Some(w.clone());
                    return Some(Fetch::Record(
                        w,
                        e.file_url(base, helmsman_project::devrecord::FILE),
                    ));
                }
                Some(_) => {
                    ui.wired.insert(
                        w,
                        Wired::Unavailable("the library has no record for it yet".into()),
                    );
                }
                None => {
                    ui.wired
                        .insert(w, Wired::Unavailable("not in the device library".into()));
                }
            }
        }
        None
    }

    /// The index's text, or why it did not come.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser build fetches; native tests drive it
    pub fn index_answer(&mut self, r: Result<String, String>, ui: &mut DevicesUi) {
        match r.and_then(|t| Index::parse(&t)) {
            Ok(index) => {
                ui.attach_library(&index);
                if let Some(p) = &mut ui.picking {
                    p.found_in(&index);
                }
                self.index = Some(index);
            }
            Err(why) => {
                ui.library_note = Some(format!("the device library could not be read: {why}"));
                if let Some(p) = &mut ui.picking {
                    p.failed(&why);
                }
                for w in self.waiting.drain(..) {
                    ui.wired.insert(
                        w,
                        Wired::Unavailable(format!("the device library could not be read: {why}")),
                    );
                }
                self.failed = Some(why);
            }
        }
    }

    /// The record's text, or why it did not come.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser build fetches; native tests drive it
    pub fn record_answer(&mut self, r: Result<String, String>, ui: &mut DevicesUi) {
        let Some(w) = self.fetching.take() else { return };
        let got = match r.and_then(|t| Record::parse(&t)) {
            Ok(rec) if rec.wired() == w => match WiredBrief::of(&rec.brief(BRIEF)) {
                Some(b) => Wired::Known(b),
                None => Wired::Unavailable("its record could not be read".into()),
            },
            Ok(rec) => Wired::Unavailable(format!("the library's record is for {}", rec.wired())),
            Err(why) => Wired::Unavailable(format!("its record could not be read: {why}")),
        };
        ui.wired.insert(w, got);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_wired_touches_only_the_signals_line() {
        let map = "# map\ndevice rio modbus-tcp 10.0.0.7\nin part bool rio di:3 unit=x   # the prox\r\nin Ready bool rio di:4\nout clamp bool rio coil:0 safe=false";
        let added = set_wired(map, "part", Some("ifm/if6123")).unwrap();
        assert_eq!(
            added,
            "# map\ndevice rio modbus-tcp 10.0.0.7\nin part bool rio di:3 unit=x wired=ifm/if6123  # the prox\r\nin Ready bool rio di:4\nout clamp bool rio coil:0 safe=false"
        );
        // Replaced, not doubled; case-blind like the ST name; the last line has no newline.
        let again = set_wired(&added, "PART", Some("sick/wtb4f-did8389194")).unwrap();
        assert!(
            again.contains("di:3 unit=x wired=sick/wtb4f-did8389194  # the prox\r\n"),
            "{again}"
        );
        assert_eq!(again.matches("wired=").count(), 1);
        let out = set_wired(map, "clamp", Some("ifm/kq6005")).unwrap();
        assert!(
            out.ends_with("out clamp bool rio coil:0 safe=false wired=ifm/kq6005"),
            "{out}"
        );
        // Taken off again, the line is as it was but for the spacing before the comment.
        let cleared = set_wired(&added, "part", None).unwrap();
        assert!(
            cleared.contains("\nin part bool rio di:3 unit=x  # the prox\r\n"),
            "{cleared}"
        );
    }

    #[test]
    fn set_wired_refuses_what_it_cannot_place() {
        let map = "in part bool rio di:3\n# in ghost bool rio di:9\n";
        assert_eq!(
            set_wired(map, "ghost", Some("ifm/if6123")),
            None,
            "a comment is not a line"
        );
        assert_eq!(set_wired(map, "nobody", Some("ifm/if6123")), None);
        for bad in [
            "../etc",
            "ifm",
            "IFM/x",
            "ifm/a b",
            "ifm/..",
            "ifm/x\nout y bool box do:0",
        ] {
            assert_eq!(set_wired(map, "part", Some(bad)), None, "{bad:?}");
        }
    }

    const MATCHES: &str = r#"{"matches": [
        {"folder": "devices/ifm/if6123", "vendor": "ifm", "model": "IF6123, IF6124", "kind": "sensor",
         "networked": false, "wired": "ifm/if6123", "products": ["IF6123", "IF6124", "IG6615"]},
        {"folder": "devices/moxa/iologik-e1213", "vendor": "Moxa", "model": "ioLogik E1213", "kind": "remote-io",
         "networked": true, "driver": "enip-io", "wired": "moxa/iologik-e1213", "products": []},
        {"vendor": "evil", "networked": false, "wired": "../../etc"}
    ]}"#;

    #[test]
    fn the_box_answer_lists_only_what_sits_on_a_channel() {
        let mut p = Pick::new("part", None);
        p.answer(200, MATCHES);
        assert_eq!(p.hits, Hits::Idle, "no answer is taken unasked");
        p.hits = Hits::Searching("IF6124".into());
        p.answer(200, MATCHES);
        let Hits::Found(q, hits, 0) = &p.hits else {
            panic!("{:?}", p.hits)
        };
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].wired, "ifm/if6123");
        assert_eq!(
            hits[0].title(q),
            "ifm IF6124 · sensor",
            "the number searched is the one shown"
        );
        assert_eq!(hits[0].title("ifm"), "ifm IF6123, IF6124 · sensor");
        for (status, body, says) in [
            (
                200,
                r#"{"matches": [], "unavailable": "offline"}"#,
                "unavailable: offline",
            ),
            (404, "", "does not search"),
            (500, r#"{"error": "boom"}"#, "(500): boom"),
            (200, "<html>", "Nothing"),
        ] {
            p.hits = Hits::Searching("x".into());
            p.answer(status, body);
            match &p.hits {
                Hits::Failed(why) => assert!(why.contains(says), "{why}"),
                Hits::Found(_, h, 0) if h.is_empty() => assert_eq!(says, "Nothing"),
                h => panic!("{h:?}"),
            }
        }
    }

    #[test]
    fn many_matches_are_cut_with_a_count() {
        let many: Vec<Json> = (0..13)
            .map(|i| serde_json::json!({"vendor": "v", "networked": false, "wired": format!("v/m{i}")}))
            .collect();
        let mut p = Pick::new("part", None);
        p.hits = Hits::Searching("v".into());
        p.answer(200, &serde_json::json!({ "matches": many }).to_string());
        let Hits::Found(_, hits, more) = &p.hits else {
            panic!()
        };
        assert_eq!((hits.len(), *more), (10, 3));
    }

    #[test]
    fn queries_are_held_to_the_boxs_limits() {
        assert!(query_error("").is_some());
        assert!(query_error(&"x".repeat(121)).is_some());
        assert!(query_error("IF\u{7}6123").is_some());
        assert!(query_error(&"x".repeat(120)).is_none());
    }

    fn index() -> Index {
        Index::parse(include_str!("fixtures/devices-index.json")).unwrap()
    }

    /// The library's own record, as `main` served it on 2026-11-15.
    const RECORD: &str = include_str!("fixtures/if6123-record.json");

    #[test]
    fn the_playground_reads_the_index_then_each_record_once() {
        let url = "https://example.test/lib/index.json";
        let mut ui = DevicesUi::default();
        let mut lib = Library::default();
        assert_eq!(
            lib.next(url, &mut ui),
            None,
            "nothing is fetched until something needs it"
        );
        lib.want("ifm/if6123".into());
        lib.want("ifm/if6123".into());
        lib.want("nobody/here".into());
        assert_eq!(lib.next(url, &mut ui), Some(Fetch::Index(url.into())));
        assert_eq!(lib.next(url, &mut ui), None, "the index is asked for once");
        lib.index_answer(Ok(include_str!("fixtures/devices-index.json").into()), &mut ui);
        assert_eq!(
            lib.next(url, &mut ui),
            Some(Fetch::Record(
                "ifm/if6123".into(),
                "https://example.test/lib/devices/ifm/if6123/record.json".into()
            ))
        );
        assert_eq!(lib.next(url, &mut ui), None, "one record at a time");
        lib.record_answer(Ok(RECORD.into()), &mut ui);
        let Some(Wired::Known(b)) = ui.wired.get("ifm/if6123") else {
            panic!("{:?}", ui.wired)
        };
        assert_eq!((b.vendor.as_str(), b.kind.as_str()), ("ifm", "sensor"));
        assert_eq!(lib.next(url, &mut ui), None);
        assert_eq!(
            ui.wired.get("nobody/here"),
            Some(&Wired::Unavailable("not in the device library".into()))
        );
    }

    #[test]
    fn a_record_for_another_folder_or_a_failed_read_is_not_shown() {
        let url = "https://example.test/index.json";
        let mut ui = DevicesUi::default();
        let mut lib = Library {
            index: Some(index()),
            ..Library::default()
        };
        lib.want("ifm/kq6005".into());
        assert!(matches!(lib.next(url, &mut ui), Some(Fetch::Record(..))));
        lib.record_answer(Ok(RECORD.into()), &mut ui);
        assert!(
            matches!(ui.wired.get("ifm/kq6005"), Some(Wired::Unavailable(w)) if w.contains("ifm/if6123"))
        );
        lib.want("ifm/o5d100".into());
        assert!(matches!(lib.next(url, &mut ui), Some(Fetch::Record(..))));
        lib.record_answer(Err("HTTP 404".into()), &mut ui);
        assert!(matches!(ui.wired.get("ifm/o5d100"), Some(Wired::Unavailable(w)) if w.contains("HTTP 404")));
    }

    #[test]
    fn a_failed_index_says_why_and_can_be_asked_again() {
        let url = "https://example.test/index.json";
        let mut ui = DevicesUi::default();
        ui.picking = Some(Pick {
            hits: Hits::Searching("IF6123".into()),
            ..Pick::new("part", None)
        });
        let mut lib = Library::default();
        lib.want("ifm/if6123".into());
        assert!(matches!(lib.next(url, &mut ui), Some(Fetch::Index(_))));
        lib.index_answer(Err("HTTP 503".into()), &mut ui);
        assert!(matches!(ui.wired.get("ifm/if6123"), Some(Wired::Unavailable(w)) if w.contains("503")));
        assert!(matches!(&ui.picking.as_ref().unwrap().hits, Hits::Failed(w) if w.contains("503")));
        assert_eq!(lib.next(url, &mut ui), None, "not again by itself");
        lib.want_index();
        assert!(matches!(lib.next(url, &mut ui), Some(Fetch::Index(_))));
    }

    #[test]
    fn the_playground_picker_searches_the_index_for_channel_devices_only() {
        let mut p = Pick::new("part", Some("ifm/if6123"));
        p.hits = Hits::Searching("1105505".into());
        p.found_in(&index());
        let Hits::Found(_, hits, _) = &p.hits else {
            panic!("{:?}", p.hits)
        };
        assert_eq!(
            hits.iter().map(|h| h.wired.as_str()).collect::<Vec<_>>(),
            ["sick/wtb4f-did8389194"]
        );
        p.hits = Hits::Searching("moxa".into());
        p.found_in(&index());
        assert_eq!(
            p.hits,
            Hits::Found("moxa".into(), Vec::new(), 0),
            "a networked device is not offered"
        );
    }
}
