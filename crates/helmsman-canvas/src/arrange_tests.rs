//! The diagram's two layers (`arrange.rs`): the project's shared default
//! and this browser's tweaks on top, across reloads, new defaults, edits
//! that renumber the blocks, resets, and storage that fails.

use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui::Vec2;

use crate::arrange::{Layers, MAX_VIEWS};
use crate::playground::Session;
use crate::sim_projects::Storage;
use crate::st_view::{Arrangement, Viewer};

/// A storage two page loads share (a browser's `localStorage`).
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Option<String>>>);

impl Storage for Shared {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.0.borrow().clone())
    }

    fn save(&mut self, text: &str) -> Result<(), String> {
        *self.0.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

/// Storage that throws on every access (a private window).
struct Broken;

impl Storage for Broken {
    fn load(&self) -> Result<Option<String>, String> {
        Err("SecurityError".into())
    }

    fn save(&mut self, _: &str) -> Result<(), String> {
        Err("QuotaExceededError".into())
    }
}

const SRC: &str = "PROGRAM main
VAR_INPUT start : BOOL; stop : BOOL; END_VAR
VAR_OUTPUT lamp : BOOL; horn : BOOL; END_VAR
  lamp := start AND NOT stop;
  horn := stop;
END_PROGRAM
";

fn viewer(src: &str) -> Viewer {
    let s = Session::new(src);
    let m = s.build.as_ref().and_then(|b| b.module.clone()).expect("compiles");
    Viewer::new(src, m)
}

/// Where the block whose text is `text` sits (the first one).
fn at(v: &Viewer, text: &str) -> (f32, f32) {
    let n = v
        .diagram()
        .nodes
        .iter()
        .find(|n| n.text == text)
        .unwrap_or_else(|| panic!("no {text}"));
    (n.rect.x, n.rect.y)
}

fn id(v: &Viewer, text: &str) -> helmsman_diagram::NodeId {
    v.diagram().nodes.iter().find(|n| n.text == text).unwrap().id
}

fn moved(p: (f32, f32), by: (f32, f32)) -> (f32, f32) {
    (p.0 + by.0, p.1 + by.1)
}

/// The text of the two blocks the tests move.
fn blocks(v: &Viewer) -> (String, String) {
    let texts: Vec<String> = v.diagram().nodes.iter().map(|n| n.text.clone()).collect();
    let lamp = texts.iter().find(|t| t.starts_with("LAMP :=")).unwrap().clone();
    let horn = texts
        .iter()
        .find(|t| t.starts_with("HORN := STOP"))
        .unwrap()
        .clone();
    (lamp, horn)
}

/// The engineer's default: the lamp block 100 down.
fn engineer_default(lamp: &str, by: f32) -> String {
    let mut v = viewer(SRC);
    v.move_node(id(&v, lamp), Vec2::new(0.0, by));
    v.arrangement().to_text()
}

#[test]
fn a_tweak_survives_a_reload_on_top_of_the_shared_default() {
    let store = Shared::default();
    let auto = viewer(SRC);
    let (lamp, horn) = blocks(&auto);
    let (lamp0, horn0) = (at(&auto, &lamp), at(&auto, &horn));
    let default = engineer_default(&lamp, 100.0);

    // A page opens on the shared default and moves the horn.
    let mut l = Layers::load(Box::new(store.clone()));
    let mut v = viewer(SRC);
    l.show(&mut v, "box:line", &default, true);
    assert_eq!(at(&v, &lamp), moved(lamp0, (0.0, 100.0)), "the default applies");
    assert!(!l.tweaked() && l.has_default());
    v.move_node(id(&v, &horn), Vec2::new(50.0, 0.0));
    l.show(&mut v, "box:line", &default, false);
    assert!(l.tweaked());

    // Reloaded: both layers.
    let mut l = Layers::load(Box::new(store.clone()));
    let mut v = viewer(SRC);
    l.show(&mut v, "box:line", &default, true);
    assert_eq!(at(&v, &lamp), moved(lamp0, (0.0, 100.0)));
    assert_eq!(at(&v, &horn), moved(horn0, (50.0, 0.0)), "the tweak, kept");

    // The engineer saves a new default: it reaches this screen, the tweak
    // still on top.
    let newer = engineer_default(&lamp, 200.0);
    l.show(&mut v, "box:line", &newer, false);
    assert_eq!(at(&v, &lamp), moved(lamp0, (0.0, 200.0)));
    assert_eq!(at(&v, &horn), moved(horn0, (50.0, 0.0)));

    // Another project's view has no tweak of this one.
    let mut w = viewer(SRC);
    l.show(&mut w, "box:other", "", true);
    assert_eq!((at(&w, &lamp), at(&w, &horn)), (lamp0, horn0));
}

#[test]
fn a_renumbering_edit_keeps_both_layers() {
    let store = Shared::default();
    let auto = viewer(SRC);
    let (lamp, horn) = blocks(&auto);
    let default = engineer_default(&lamp, 100.0);
    let mut l = Layers::load(Box::new(store.clone()));
    let mut v = viewer(SRC);
    l.show(&mut v, "sim:line", &default, true);
    v.move_node(id(&v, &horn), Vec2::new(60.0, 10.0));
    l.track(&v);

    // A statement on top: every block id after it moves up by one.
    let edited = SRC.replace("  lamp :=", "  horn := FALSE;\n  lamp :=");
    let mut e = viewer(&edited);
    let auto_e = viewer(&edited);
    assert_ne!(id(&e, &lamp), id(&auto, &lamp), "renumbered");
    l.show(&mut e, "sim:line", &default, true);
    assert_eq!(
        at(&e, &lamp),
        moved(at(&auto_e, &lamp), (0.0, 100.0)),
        "the default"
    );
    // The new `HORN := FALSE` shares the label, not the text: the moved
    // horn block is still found.
    assert_eq!(
        at(&e, &horn),
        moved(at(&auto_e, &horn), (60.0, 10.0)),
        "the tweak"
    );
}

#[test]
fn reset_goes_to_the_default_then_to_the_automatic_layout_and_stays() {
    let store = Shared::default();
    let auto = viewer(SRC);
    let (lamp, horn) = blocks(&auto);
    let (lamp0, horn0) = (at(&auto, &lamp), at(&auto, &horn));
    let default = engineer_default(&lamp, 100.0);
    let mut l = Layers::load(Box::new(store.clone()));
    let mut v = viewer(SRC);
    l.show(&mut v, "k", &default, true);
    v.move_node(id(&v, &horn), Vec2::new(50.0, 0.0));
    l.track(&v);

    l.reset(&mut v);
    assert_eq!(at(&v, &lamp), moved(lamp0, (0.0, 100.0)), "the default");
    assert_eq!(at(&v, &horn), horn0, "the tweak gone");
    assert!(!l.tweaked());
    l.reset(&mut v);
    assert_eq!((at(&v, &lamp), at(&v, &horn)), (lamp0, horn0), "automatic");
    assert!(!v.rearranged());

    // A reload keeps the automatic layout, and Reset brings the default
    // back.
    let mut l = Layers::load(Box::new(store));
    let mut v = viewer(SRC);
    l.show(&mut v, "k", &default, true);
    assert_eq!(at(&v, &lamp), lamp0);
    l.reset(&mut v);
    assert_eq!(at(&v, &lamp), moved(lamp0, (0.0, 100.0)));
}

#[test]
fn saving_as_default_moves_nothing_and_leaves_no_tweak() {
    let store = Shared::default();
    let auto = viewer(SRC);
    let (lamp, horn) = blocks(&auto);
    let mut l = Layers::load(Box::new(store));
    let mut v = viewer(SRC);
    l.show(&mut v, "k", "", true);
    v.move_node(id(&v, &lamp), Vec2::new(0.0, 80.0));
    v.move_node(id(&v, &horn), Vec2::new(30.0, 0.0));
    l.track(&v);
    let before = (at(&v, &lamp), at(&v, &horn));
    let text = v.arrangement().to_text();
    l.adopt(&v, &text);
    l.show(&mut v, "k", &text, false);
    assert_eq!((at(&v, &lamp), at(&v, &horn)), before);
    assert!(!l.tweaked() && l.has_default());
    assert_eq!(Arrangement::from_text(&text).unwrap().len(), 2);
}

#[test]
fn storage_that_fails_or_holds_garbage_still_works_in_memory() {
    let auto = viewer(SRC);
    let (_, horn) = blocks(&auto);
    let mut l = Layers::load(Box::new(Broken));
    assert!(l.problem.as_deref().unwrap().contains("does not keep"));
    let mut v = viewer(SRC);
    l.show(&mut v, "k", "", true);
    v.move_node(id(&v, &horn), Vec2::new(40.0, 0.0));
    l.track(&v);
    assert!(l.tweaked(), "kept in memory");

    for junk in ["{", "{\"format\":\"other\"}", "[1,2]"] {
        let s = Shared::default();
        *s.0.borrow_mut() = Some(junk.into());
        let l = Layers::load(Box::new(s));
        assert!(l.problem.as_deref().unwrap().contains("not readable"), "{junk}");
    }
    // A bad shared default (hand-edited) is no default, not a crash.
    let mut l = Layers::load(Box::new(Shared::default()));
    let mut v = viewer(SRC);
    l.show(&mut v, "k", "helmsman-layout 9\n", true);
    assert!(!l.has_default() && !v.rearranged());
}

#[test]
fn a_browser_keeps_at_most_max_views_and_drops_the_oldest() {
    let store = Shared::default();
    let auto = viewer(SRC);
    let (_, horn) = blocks(&auto);
    let mut l = Layers::load(Box::new(store.clone()));
    for k in 0..=MAX_VIEWS {
        let mut v = viewer(SRC);
        l.show(&mut v, &format!("view{k}"), "", true);
        v.move_node(id(&v, &horn), Vec2::new(10.0, 0.0));
        l.track(&v);
    }
    let text = store.0.borrow().clone().unwrap();
    assert!(!text.contains("\"view0\""), "the oldest went");
    assert!(text.contains(&format!("\"view{MAX_VIEWS}\"")));
    let mut l = Layers::load(Box::new(store));
    let mut v = viewer(SRC);
    l.show(&mut v, "view0", "", true);
    assert!(!l.tweaked());
}
