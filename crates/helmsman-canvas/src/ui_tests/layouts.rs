//! The pane layouts on the screen (`crate::pane_layout`): a tab dragged
//! along its row, hidden from its own menu and brought back, and the
//! View menu's popup of ready-made and saved layouts.

use eframe::egui::{Event, Pos2, vec2};

use super::{App, Harness};
use crate::editor::{LEFT_W, Pane};
use crate::pane_layout::{Layouts, presets};

fn sim() -> Harness {
    let mut h = Harness::new(App::sim());
    h.wait(0.3);
    h
}

fn open_popup(h: &mut Harness) {
    h.view_menu("view layouts");
    h.wait(0.2);
    assert!(h.app.layouts.open && h.has("layouts"), "the popup is up");
}

#[test]
fn a_tab_dragged_along_its_row_takes_that_place() {
    use Pane::{Claude, Code, Devices, Files, History, Io};
    let mut h = sim();
    assert_eq!(h.app.chrome.lefts, [Claude, Code, Io, Devices, Files, History]);
    // Onto the left half of the first tab: before it.
    let io = h.rect("tab io").center();
    let first = h.rect("tab claude");
    h.drag(io, first.left_center() + vec2(3.0, 0.0));
    h.wait(0.2);
    assert_eq!(h.app.chrome.lefts, [Io, Claude, Code, Devices, Files, History]);
    assert_eq!(
        h.app.chrome.pane, Claude,
        "a drag is not a click: the Agent still shows"
    );
    assert!(h.app.chrome.rights.is_empty(), "nothing docked");
    // The row is drawn in the new order.
    assert!(h.rect("tab io").left() < h.rect("tab claude").left());
    // Onto the right half of the last tab: the end.
    let io = h.rect("tab io").center();
    let last = h.rect("pane history");
    h.drag(io, last.right_center() - vec2(3.0, 0.0));
    h.wait(0.2);
    assert_eq!(h.app.chrome.lefts, [Claude, Code, Devices, Files, History, Io]);
    // Onto the right half of a tab in the middle: after it.
    let files = h.rect("pane files").center();
    let code = h.rect("pane code");
    h.drag(files, code.right_center() - vec2(3.0, 0.0));
    h.wait(0.2);
    assert_eq!(h.app.chrome.lefts, [Claude, Code, Files, Devices, History, Io]);
    // Dropped on itself, or let go over the pane's body: nothing moves.
    let was = h.app.chrome.lefts.clone();
    let code = h.rect("pane code");
    h.drag(code.center(), code.center() + vec2(6.0, 0.0));
    h.drag(code.center(), code.center() + vec2(0.0, 240.0));
    h.wait(0.2);
    assert_eq!(h.app.chrome.lefts, was);
    // The order is what the browser keeps.
    let kept = crate::editor::Chrome::restored(&h.app.chrome.saved());
    assert_eq!(kept.lefts, was);
}

#[test]
fn a_tab_dragged_into_the_other_panes_row_docks_at_that_place() {
    use Pane::{Code, Devices, Io};
    let mut h = sim();
    h.app.chrome.dock(Io, true);
    h.app.chrome.dock(Devices, true);
    h.wait(0.3);
    let code = h.rect("pane code").center();
    let devices = h.rect("pane devices");
    h.drag(code, devices.left_center() + vec2(3.0, 0.0));
    h.wait(0.3);
    assert_eq!(h.app.chrome.rights, [Io, Code, Devices], "between the two");
    assert!(h.app.chrome.showing(Code), "shown where it landed");
    // And back, to the front of the left pane's row.
    let code = h.rect("pane code").center();
    let first = h.rect("tab claude");
    h.drag(code, first.left_center() + vec2(3.0, 0.0));
    h.wait(0.3);
    assert_eq!(h.app.chrome.lefts.first(), Some(&Code));
    assert_eq!(h.app.chrome.rights, [Io, Devices]);
}

#[test]
fn a_tab_hidden_from_its_menu_comes_back_from_the_view_menu() {
    let mut h = sim();
    let at = h.rect("pane history").center();
    h.right_click(at);
    h.press("hide history");
    h.wait(0.2);
    assert_eq!(h.app.chrome.hidden, [Pane::History]);
    assert!(!h.has("pane history"), "its button is gone from the row");
    assert!(
        h.has("tab claude") && h.app.chrome.pane == Pane::Claude,
        "the rest stay"
    );
    // The shown tab hidden: the one under it shows.
    let at = h.rect("tab claude").center();
    h.right_click(at);
    h.press("hide claude");
    h.wait(0.2);
    assert_eq!(h.app.chrome.pane, Pane::Code);
    assert!(h.has("pane code") && !h.has("tab claude"));
    // View › History, View › Agent: back, at the end, shown.
    h.view_menu("view history");
    h.wait(0.2);
    assert!(h.app.chrome.hidden == [Pane::Claude] && h.app.chrome.showing(Pane::History));
    h.view_menu("view tab claude");
    h.wait(0.2);
    assert!(h.app.chrome.hidden.is_empty() && h.app.chrome.showing(Pane::Claude));
    assert_eq!(h.app.chrome.lefts[4..], [Pane::History, Pane::Claude]);
    // Every tab hidden: no pane is drawn, the diagram has the width, and
    // the View menu still opens one.
    for p in Pane::ALL {
        h.app.chrome.hide(p);
    }
    h.wait(0.3);
    assert!(!h.has("left") && !h.has("right"));
    assert!(h.app.chrome.diagram.left() < 1.0);
    h.view_menu("view files");
    h.wait(0.3);
    assert!(h.has("pane files") && h.app.chrome.files_open());
}

#[test]
fn a_ready_made_layout_arranges_the_panes_and_sizes_them() {
    let mut h = sim();
    let wide = h.app.chrome.diagram;
    open_popup(&mut h);
    for p in presets() {
        assert!(
            h.has(&format!("layouts preset {}", p.name.to_lowercase())),
            "{}",
            p.name
        );
    }
    h.press("layouts preset programming");
    h.wait(0.4);
    assert!(
        !h.app.layouts.open && !h.has("layouts"),
        "picking one shuts the list"
    );
    let c = &h.app.chrome;
    assert_eq!(c.rights, [Pane::Code, Pane::Io, Pane::Devices]);
    assert!(c.showing(Pane::Claude) && c.showing(Pane::Code));
    assert_eq!(c.sizes.right, Some(420.0), "the right pane at the layout's width");
    assert_eq!(c.sizes.left, Some(LEFT_W));
    let d = c.diagram;
    assert!((h.size.x - d.right() - 420.0).abs() < 2.0, "{d:?}");
    // Monitoring: both panes to their strips.
    open_popup(&mut h);
    h.press("layouts preset monitoring");
    h.wait(0.4);
    assert!(!h.app.chrome.left && h.app.chrome.diagram.width() > wide.width());
    // Default: the page as it first opened.
    open_popup(&mut h);
    h.press("layouts preset default");
    h.wait(0.4);
    assert_eq!(h.app.chrome.layout().lefts, Pane::ALL);
    assert!((h.app.chrome.diagram.left() - wide.left()).abs() < 2.0);
}

#[test]
fn the_panes_as_they_are_save_by_name_and_come_back() {
    let mut h = sim();
    // An arrangement of one's own: I/O on the right, History hidden, the
    // left pane widened to 400 pt.
    h.app.chrome.dock(Pane::Io, true);
    h.app.chrome.hide(Pane::History);
    let mut l = h.app.chrome.layout();
    l.sizes.left = Some(400.0);
    h.app.chrome.apply(&l);
    h.wait(0.4);
    assert_eq!(h.app.chrome.sizes.left, Some(400.0));
    let mine = h.app.chrome.layout();

    open_popup(&mut h);
    assert!(h.has("layouts save") && !h.has("layouts mine Start-up"));
    // No name yet: Save does nothing.
    h.press("layouts save");
    assert_eq!(h.app.layouts.mine, []);
    h.press("layouts name");
    h.frame(vec![Event::Text("Start-up".into())]);
    h.frame(vec![]);
    h.press("layouts save");
    h.wait(0.2);
    assert_eq!(h.app.layouts.get("Start-up"), Some(&mine));
    assert!(h.has("layouts mine Start-up") && h.has("layouts delete Start-up"));
    assert!(h.app.layouts.name.is_empty(), "the field is ready for the next");
    // A ready-made layout's name is refused, and says so.
    h.press("layouts name");
    h.frame(vec![Event::Text("Default".into())]);
    h.frame(vec![]);
    h.press("layouts save");
    h.wait(0.2);
    let (note, bad) = h.app.layouts.note.clone().unwrap();
    assert!(bad && note.contains("ready-made"), "{note}");
    assert_eq!(h.app.layouts.mine.len(), 1);
    // Nothing the popup draws is under the smallest text or off its edge.
    assert_eq!(h.small_text(), vec![], "small text in the popup");
    let popup = h.rect("layouts");
    for (k, r) in h.app.chrome.hits.clone() {
        if k.starts_with("layouts ") {
            assert!(
                popup.expand(1.0).contains_rect(r),
                "{k} at {r:?} leaves the popup {popup:?}"
            );
        }
    }

    // Another arrangement, then the saved one by its button.
    h.press("layouts preset monitoring");
    h.wait(0.4);
    assert_ne!(h.app.chrome.layout(), mine);
    open_popup(&mut h);
    h.press("layouts mine Start-up");
    h.wait(0.4);
    assert_eq!(h.app.chrome.layout(), mine, "tabs, panes and sizes as saved");

    // What the browser stores brings it back on the next visit.
    let next = Layouts::restored(&h.app.layouts.saved());
    assert_eq!(next.get("start-up"), Some(&mine));

    // A file from another browser comes in beside it; a bad one says why.
    let mut other = Layouts::default();
    other.save("Wall screen", presets()[3].layout.clone()).unwrap();
    open_popup(&mut h);
    h.app.layouts.imported("layouts.json", &other.export());
    h.app.layouts.imported("notes.json", b"hello");
    h.wait(0.2);
    assert!(h.has("layouts mine Wall screen") && h.has("layouts note"));
    assert!(
        h.app.layouts.note.clone().unwrap().1,
        "the bad file is the last word"
    );

    // Delete, then Close.
    h.press("layouts delete Start-up");
    h.wait(0.2);
    assert!(h.app.layouts.get("Start-up").is_none() && !h.has("layouts mine Start-up"));
    h.press("layouts close");
    h.wait(0.2);
    assert!(!h.app.layouts.open && !h.has("layouts"));
    // Escape shuts it too, and a click on the page behind does not.
    open_popup(&mut h);
    h.click(Pos2::new(h.size.x - 10.0, h.size.y - 10.0));
    assert!(h.app.layouts.open, "a stray click keeps the typed name");
    h.frame(vec![Event::Key {
        key: eframe::egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: eframe::egui::Modifiers::NONE,
    }]);
    h.wait(0.2);
    assert!(!h.app.layouts.open);
}

fn boxv(h: &mut Harness) -> &mut crate::box_view::BoxView {
    h.app.boxv.as_mut().expect("the box view")
}

/// On a box's page the popup lists what the box keeps: one click arranges
/// the panes, and only an engineer is offered To box and Delete.
#[test]
fn on_a_box_the_popup_offers_the_boxs_layouts_and_an_engineer_keeps_one_there() {
    use crate::box_projects::PCall;
    use crate::review::Session;
    let mut h = Harness::new(App::boxed());
    h.wait(0.3);
    boxv(&mut h).psent.clear();
    open_popup(&mut h);
    h.wait(0.2);
    let asked: Vec<_> = boxv(&mut h)
        .psent
        .iter()
        .filter(|q| q.call == PCall::PaneLayouts)
        .map(|q| (q.method, q.url.clone()))
        .collect();
    assert_eq!(
        asked,
        [("GET", "/api/pane-layouts".to_string())],
        "read once on opening"
    );
    assert!(!h.has("layouts box Wall screen"));
    // The box's answer, and a layout of one's own.
    let wall = presets()[3].layout.clone();
    boxv(&mut h).pane_layouts.list = vec![("Wall screen".into(), wall.clone())];
    h.app.layouts.save("Mine", presets()[1].layout.clone()).unwrap();
    h.wait(0.2);
    assert!(h.has("layouts box Wall screen") && h.has("layouts keep Mine"));
    // Signed out: To box and the box's Delete do nothing.
    boxv(&mut h).psent.clear();
    h.press("layouts keep Mine");
    h.press("layouts forget Wall screen");
    h.wait(0.2);
    assert!(
        !boxv(&mut h).psent.iter().any(|q| q.method != "GET"),
        "nothing is sent for someone who may not change the box's list"
    );
    // Nothing leaves the popup at its width with the third button in a row.
    assert_eq!(h.small_text(), vec![], "small text in the popup");
    let popup = h.rect("layouts");
    for (k, r) in h.app.chrome.hits.clone() {
        if k.starts_with("layouts ") {
            assert!(
                popup.expand(1.0).contains_rect(r),
                "{k} at {r:?} leaves the popup {popup:?}"
            );
        }
    }
    // An engineer: To box sends the layout, Delete names it in the path.
    h.app
        .review
        .as_mut()
        .expect("a box page has its review")
        .state
        .session = Some(Session {
        user: Some("alice".into()),
        role: "engineer".into(),
    });
    h.wait(0.2);
    boxv(&mut h).psent.clear();
    h.press("layouts keep Mine");
    h.wait(0.2);
    h.press("layouts forget Wall screen");
    h.wait(0.2);
    let sent: Vec<_> = boxv(&mut h)
        .psent
        .iter()
        .filter(|q| q.method != "GET")
        .map(|q| (q.method, q.url.clone()))
        .collect();
    assert_eq!(
        sent,
        [
            ("PUT", "/api/pane-layouts".to_string()),
            ("DELETE", "/api/pane-layouts/Wall%20screen".to_string())
        ]
    );
    let put = boxv(&mut h)
        .psent
        .iter()
        .find(|q| q.method == "PUT")
        .unwrap()
        .clone();
    let body: serde_json::Value = serde_json::from_slice(put.body.as_ref().unwrap()).unwrap();
    assert_eq!(body["name"], "Mine");
    assert_eq!(body["right"], serde_json::json!(["code", "io", "devices"]));
    // The box's layout by its button.
    h.press("layouts box Wall screen");
    h.wait(0.4);
    assert!(!h.app.layouts.open);
    assert_eq!(h.app.chrome.layout().left, wall.left);
    assert!(!h.app.chrome.left, "Monitoring: the left pane shut");
    // The simulation's popup has no box section.
    let mut s = sim();
    open_popup(&mut s);
    assert!(!s.has("layouts box note") && s.app.boxv.is_none());
}
