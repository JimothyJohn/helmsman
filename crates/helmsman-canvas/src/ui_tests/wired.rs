//! What is wired to a point, outside the Devices tab's own line
//! (TODO.md `iodd-metadata`, `crate::wired`): the picker that finds a
//! sensor by the number on its label and writes `wired=` into io.map,
//! and the same line under a signal in the I/O tab, inside the pane's
//! default width.

use eframe::egui::Event;

use super::{App, Harness};
use crate::box_projects::{Buf, OpenProject, PCall};
use crate::box_tests::{Bench, watch};
use crate::devices::{Wired, WiredBrief};

/// The longest vendor and model text the library has today, as a brief.
fn long_brief() -> Wired {
    Wired::Known(WiredBrief {
        vendor: "ifm electronic gmbh".into(),
        model: "IF6123, IF6124, IG6615, IG6616, II5973, II5974, IM5172, IM5173".into(),
        kind: "sensor".into(),
        draft: true,
        signals: vec![("SSC1".into(), Some("active".into()), Some("inactive".into()))],
        settings: vec![("P-n".into(), "PnP".into())],
        carries: None,
    })
}

fn index() -> helmsman_project::devlib::Index {
    helmsman_project::devlib::Index::parse(include_str!("../fixtures/devices-index.json")).unwrap()
}

/// A playground project of one's own with the latch example's program
/// and `map` as its io.map, compiled and running.
fn playground(map: &str) -> Harness {
    let mut h = Harness::new(App::sim());
    {
        let sim = h.app.sim.as_mut().unwrap();
        let ex = crate::playground::EXAMPLES
            .iter()
            .map(|e| e.src)
            .find(|src| src.contains("PROGRAM latch"))
            .map(|src| crate::sim_projects::example_project(src).0)
            .expect("the latch example");
        sim.open_project(&ex);
        sim.proj.create("mine", Some(&ex));
        sim.proj.add_file(helmsman_project::IO_MAP, map.into());
    }
    h.wait(0.3);
    h
}

fn map(h: &Harness) -> String {
    h.app.sim.as_ref().unwrap().proj.files[helmsman_project::IO_MAP].clone()
}

/// Right-click a point, "What is wired here…", type a label number,
/// Search: the playground answers from the index it has read.
fn search(h: &mut Harness, signal: &str, query: &str) {
    h.press("pane devices");
    h.wait(0.2);
    h.right_click(h.rect(&format!("point {signal}")).center());
    h.press("menu wired point");
    h.wait(0.2);
    assert!(h.has("wired form"), "the picker opens");
    h.press("wired query");
    h.frame(vec![Event::Text(query.into())]);
    h.press("wired search");
    h.wait(0.2);
}

#[test]
fn the_playground_picker_writes_wired_on_the_point_and_clears_it() {
    let mut h =
        playground("in start bool box di:0  # the button\nin stop bool box di:1\nout motor bool box do:0\n");
    h.app.sim.as_mut().unwrap().wired_lib.index = Some(index());
    search(&mut h, "start", "IF6124");
    assert!(h.has("wired hit ifm/if6123"), "the record that covers IF6124");
    assert!(
        !h.has("wired hit moxa/iologik-e1213"),
        "a networked device is never offered"
    );
    h.press("wired hit ifm/if6123");
    h.wait(0.3);
    assert!(!h.has("wired form"), "picked: the picker closes");
    assert_eq!(
        map(&h),
        "in start bool box di:0 wired=ifm/if6123  # the button\nin stop bool box di:1\nout motor bool box do:0\n"
    );
    // The point now says what it is; the record is asked for once.
    assert!(h.has("wired start"));
    // Open again: it says what is there, and Clear takes it off.
    h.right_click(h.rect("point start").center());
    h.press("menu wired point");
    h.wait(0.2);
    h.press("wired clear");
    h.wait(0.3);
    assert_eq!(
        map(&h),
        "in start bool box di:0  # the button\nin stop bool box di:1\nout motor bool box do:0\n"
    );
    assert!(!h.has("wired start"));
}

#[test]
fn a_label_the_library_does_not_know_writes_nothing() {
    let before = "in start bool box di:0\nin stop bool box di:1\nout motor bool box do:0\n";
    let mut h = playground(before);
    h.app.sim.as_mut().unwrap().wired_lib.index = Some(index());
    search(&mut h, "stop", "WL12G-3B2531");
    assert!(h.has("wired form"), "still open, saying nothing matched");
    assert!(
        !h.app
            .sim
            .as_ref()
            .unwrap()
            .devices_ui
            .picking
            .as_ref()
            .is_some_and(|p| matches!(&p.hits, crate::wired::Hits::Found(_, hits, _) if !hits.is_empty())),
        "no match"
    );
    h.press("wired cancel");
    h.wait(0.2);
    assert!(!h.has("wired form"));
    assert_eq!(map(&h), before);
}

#[test]
fn the_playground_io_tab_says_what_is_wired_inside_the_default_width() {
    let mut h = playground(
        "in start bool box di:0 wired=ifm/if6123\nin stop bool box di:1\nout motor bool box do:0 wired=ifm/if6123\n",
    );
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    h.frame(vec![]);
    h.press("tab io");
    h.wait(0.3);
    let tab = h.rect("tab io");
    // Every point has a card, and its second line says what it is in
    // the real world: what is wired to it. The line is named for the
    // program's point.
    assert!(h.has("wired MOTOR"), "the line under the output");
    assert!(
        h.has("wired START"),
        "and under the input, which has a card of its own now"
    );
    assert!(
        !h.has("wired STOP"),
        "nothing wired and no device declared: no line"
    );
    // The playground has no box: what its own reading of the library found.
    h.app
        .sim
        .as_mut()
        .unwrap()
        .devices_ui
        .wired
        .insert("ifm/if6123".into(), long_brief());
    h.wait(0.3);
    let edge = h.size.x - crate::editor::RIGHT_W;
    let line = h.rect("wired MOTOR");
    assert!(
        line.left() >= edge - 8.0 && line.right() <= h.size.x,
        "the line {line:?} stays in the pane's {} pt default width",
        crate::editor::RIGHT_W
    );
    // The card gives the line the room it needs (it is not cut short);
    // the pane keeps its width.
    assert!(line.height() > 30.0, "the long line wraps on the card: {line:?}");
    assert!(!h.elided("ifm"), "and nothing of it is cut");
    // The card's details say it in full.
    let at = line.center();
    h.click(at);
    h.click(at);
    h.wait(0.3);
    let line = h.rect("wired MOTOR");
    assert!(
        line.left() >= edge - 8.0 && line.right() <= h.size.x,
        "the open line {line:?} stays in the pane's {} pt default width",
        crate::editor::RIGHT_W
    );
    assert!(
        line.height() < 120.0,
        "it wraps in words, not a letter a line: {line:?}"
    );
    assert!(line.height() > 25.0, "the long name wraps: {line:?}");
    assert!(
        (h.rect("tab io").left() - tab.left()).abs() <= 1.0,
        "the pane grew: {tab:?} -> {:?}",
        h.rect("tab io")
    );
}

fn buf(text: &str) -> Buf {
    Buf {
        text: text.into(),
        base: text.into(),
        loaded: true,
        ..Buf::default()
    }
}

#[test]
fn on_a_box_the_io_tab_says_what_is_wired_and_the_picker_asks_the_box() {
    let bench = Bench::new();
    let (state, _) = watch(&bench);
    let mut h = Harness::new(App::boxed());
    h.app.chrome.dock(crate::editor::Pane::Io, true);
    let v = h.app.boxv.as_mut().unwrap();
    v.state = state;
    v.projects.available = Some(true);
    v.state.signals.push(crate::remote::SignalRow {
        name: "part".into(),
        dir: "in".into(),
        unit: None,
        value: Some(helmsman_iec::Value::Bool(false)),
        quality: "good".into(),
        point_error: None,
        forced: false,
    });
    // Recording what is on a point is an edit: an engineer's.
    h.app.review.as_mut().unwrap().state.session = Some(crate::review::Session {
        user: Some("alice".into()),
        role: "engineer".into(),
    });
    let v = h.app.boxv.as_mut().unwrap();
    let mut o = OpenProject {
        name: "demo".into(),
        ..OpenProject::default()
    };
    let text = "device rio modbus-tcp 10.0.0.57:502 unit=1\nin part bool rio di:4\n";
    o.bufs.insert("io.map".into(), buf(text));
    v.projects.open = Some(o);
    h.frame(vec![]);
    h.press("pane devices");
    h.wait(0.3);
    // The picker: the search goes to the box, the answer lists what it found.
    h.right_click(h.rect("point part").center());
    h.press("menu wired point");
    h.wait(0.2);
    h.press("wired query");
    h.frame(vec![Event::Text("IF6124 x/y".into())]);
    h.press("wired search");
    h.wait(0.2);
    let asked: Vec<String> = std::mem::take(&mut h.app.boxv.as_mut().unwrap().psent)
        .into_iter()
        .filter(|r| r.call == PCall::FindWired)
        .map(|r| r.url)
        .collect();
    assert_eq!(asked, ["/api/devices/library?q=IF6124%20x%2Fy"]);
    let p = h.app.boxv.as_mut().unwrap().devices_ui.picking.as_mut().unwrap();
    p.answer(
        200,
        r#"{"matches": [{"folder": "devices/ifm/if6123", "vendor": "ifm", "model": "IF6123, IF6124",
            "kind": "sensor", "networked": false, "wired": "ifm/if6123", "products": ["IF6123", "IF6124"]}]}"#,
    );
    h.wait(0.2);
    h.press("wired hit ifm/if6123");
    h.wait(0.3);
    let o = h.app.boxv.as_ref().unwrap().projects.open.as_ref().unwrap();
    assert_eq!(
        o.bufs["io.map"].text,
        "device rio modbus-tcp 10.0.0.57:502 unit=1\nin part bool rio di:4 wired=ifm/if6123\n"
    );
    // The I/O tab: the open project's map names it, so the live row says what it is.
    h.press("tab io");
    h.wait(0.3);
    assert!(h.has("wired part"), "the line under the live row");
    h.app
        .boxv
        .as_mut()
        .unwrap()
        .devices_ui
        .wired
        .insert("ifm/if6123".into(), long_brief());
    h.wait(0.3);
    let line = h.rect("wired part");
    assert!(
        line.left() >= h.size.x - crate::editor::RIGHT_W - 8.0 && line.right() <= h.size.x,
        "the line {line:?} stays in the pane's default width"
    );
    assert!(line.height() < 90.0, "{line:?}");
    assert!(
        h.app
            .boxv
            .as_ref()
            .unwrap()
            .devices_ui
            .wired
            .contains_key("ifm/if6123"),
        "the record is asked for"
    );
}
